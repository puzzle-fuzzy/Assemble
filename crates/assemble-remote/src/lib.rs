//! 脱离窗口与网络的远程输入规则；原生能力通过独立 feature 接入。
#![cfg_attr(test, allow(clippy::pedantic))]
use assemble_protocol::remote::{RemoteButton, RemoteInput};

#[cfg(feature = "native")]
pub mod native;

pub fn coordinate(origin: i32, length: u32, value: u16) -> i32 {
    let offset = u64::from(length.saturating_sub(1)) * u64::from(value) / u64::from(u16::MAX);
    origin.saturating_add(i32::try_from(offset).unwrap_or(i32::MAX))
}

#[derive(Debug, Default)]
pub struct Pressed {
    keys: Vec<String>,
    buttons: Vec<RemoteButton>,
}
impl Pressed {
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
    pub fn releases(&mut self) -> Vec<RemoteInput> {
        let mut result: Vec<_> = self
            .keys
            .drain(..)
            .rev()
            .map(|key| RemoteInput::Key {
                key,
                pressed: false,
            })
            .collect();
        result.extend(self.buttons.drain(..).map(|button| RemoteInput::Button {
            button,
            pressed: false,
        }));
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
        assert!(held.releases().is_empty());
    }
}
