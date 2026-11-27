//! 脱离窗口与网络的远程输入规则；原生能力通过独立 feature 接入。
#![cfg_attr(test, allow(clippy::pedantic))]
use assemble_protocol::remote::{RemoteButton, RemoteInput};

pub mod crypto;
pub mod quality;
pub mod worker;

#[cfg(feature = "native")]
pub mod native;

pub fn coordinate(origin: i32, length: u32, value: u16) -> i32 {
    let offset = u64::from(length.saturating_sub(1)) * u64::from(value) / u64::from(u16::MAX);
    origin.saturating_add(i32::try_from(offset).unwrap_or(i32::MAX))
}
pub fn normalize_coordinate(origin: i32, length: u32, point: i32) -> Option<u16> {
    let offset = i64::from(point) - i64::from(origin);
    if length == 0 || offset < 0 || offset >= i64::from(length) {
        return None;
    }
    u16::try_from(offset * i64::from(u16::MAX) / i64::from(length.saturating_sub(1).max(1))).ok()
}

#[derive(Debug, Default)]
pub struct Pressed {
    keys: Vec<String>,
    buttons: Vec<RemoteButton>,
}
impl Pressed {
    pub fn before_inject(&mut self, input: &RemoteInput) {
        if matches!(
            input,
            RemoteInput::Key { pressed: true, .. } | RemoteInput::Button { pressed: true, .. }
        ) {
            self.observe(input);
        }
    }
    pub fn after_inject(&mut self, input: &RemoteInput, accepted: bool) {
        if accepted {
            self.observe(input);
        }
    }
    pub fn observe(&mut self, input: &RemoteInput) {
        match input {
            RemoteInput::Key { key, pressed } => {
                self.keys.retain(|held| held != key);
                if *pressed {
                    self.keys.push(key.clone());
                }
            }
            RemoteInput::Button { button, pressed } => {
                self.buttons.retain(|held| held != button);
                if *pressed {
                    self.buttons.push(*button);
                }
            }
            _ => {}
        }
    }
    pub fn releases(&self) -> Vec<RemoteInput> {
        let mut result: Vec<_> = self
            .keys
            .iter()
            .cloned()
            .rev()
            .map(|key| RemoteInput::Key {
                key,
                pressed: false,
            })
            .collect();
        result.extend(
            self.buttons
                .iter()
                .copied()
                .map(|button| RemoteInput::Button {
                    button,
                    pressed: false,
                }),
        );
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn coordinates_include_negative_monitors_and_edges() {
        assert_eq!(coordinate(-1920, 1920, 0), -1920);
        assert_eq!(coordinate(-1920, 1920, u16::MAX), -1);
        assert_eq!(coordinate(0, 1, u16::MAX), 0);
        assert_eq!(normalize_coordinate(-1920, 1920, -1920), Some(0));
        assert_eq!(normalize_coordinate(-1920, 1920, -1), Some(u16::MAX));
        assert_eq!(normalize_coordinate(-1920, 1920, 0), None);
        assert_eq!(normalize_coordinate(0, 1, 0), Some(0));
        assert_eq!(normalize_coordinate(0, 0, 0), None);
    }
    #[test]
    fn releases_are_reverse_order_and_idempotent() {
        let mut held = Pressed::default();
        for key in ["Control", "a", "a"] {
            held.observe(&RemoteInput::Key {
                key: key.into(),
                pressed: true,
            });
        }
        assert_eq!(
            held.releases(),
            vec![
                RemoteInput::Key {
                    key: "a".into(),
                    pressed: false
                },
                RemoteInput::Key {
                    key: "Control".into(),
                    pressed: false
                }
            ]
        );
        for input in held.releases() {
            held.after_inject(&input, true);
        }
        assert!(held.releases().is_empty());
    }
    #[test]
    fn failed_releases_remain_tracked_for_explicit_cleanup() {
        let press = RemoteInput::Button {
            button: RemoteButton::Left,
            pressed: true,
        };
        let release = RemoteInput::Button {
            button: RemoteButton::Left,
            pressed: false,
        };
        let mut held = Pressed::default();
        held.before_inject(&press);
        held.after_inject(&press, false);
        held.before_inject(&release);
        held.after_inject(&release, false);
        assert_eq!(held.releases(), vec![release.clone()]);
        held.after_inject(&release, true);
        assert!(held.releases().is_empty());
    }
}
