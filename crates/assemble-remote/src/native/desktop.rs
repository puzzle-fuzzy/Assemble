//! xcap/enigo 封装平台 API，工作区内不需要 unsafe。采集与输入始终留在同一个工作线程。
use crate::{Pressed, coordinate};
use assemble_protocol::remote::{
    MAX_FRAME_BYTES, RemoteButton, RemoteDisplay, RemoteFrame, RemoteInput,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use enigo::{Axis, Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};
use image::{DynamicImage, codecs::jpeg::JpegEncoder};
use xcap::Monitor;

#[derive(Debug, thiserror::Error)]
pub enum NativeError {
    #[error("screen capture is unavailable; check screen recording permission")]
    Capture,
    #[error("input control is unavailable; check accessibility permission")]
    Input,
    #[error("remote input is invalid")]
    Invalid,
}
impl NativeError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Capture => "REMOTE_CAPTURE_FAILED",
            Self::Input => "REMOTE_INPUT_FAILED",
            Self::Invalid => "INVALID_INPUT",
        }
    }
}
pub struct Host {
    monitors: Vec<Monitor>,
    selected: usize,
    enigo: Enigo,
    pressed: Pressed,
    sequence: u32,
}
impl Host {
    pub fn open() -> Result<Self, NativeError> {
        let monitors = Monitor::all().map_err(|_| NativeError::Capture)?;
        if monitors.is_empty() || monitors.len() > 16 {
            return Err(NativeError::Capture);
        }
        let selected = monitors
            .iter()
            .position(|m| m.is_primary().unwrap_or(false))
            .unwrap_or(0);
        let enigo = Enigo::new(&Settings::default()).map_err(|_| NativeError::Input)?;
        Ok(Self {
            monitors,
            selected,
            enigo,
            pressed: Pressed::default(),
            sequence: 0,
        })
    }
    pub fn frame(&mut self) -> Result<RemoteFrame, NativeError> {
        let monitor = &self.monitors[self.selected];
        let source_width = monitor.width().map_err(|_| NativeError::Capture)?;
        let source_height = monitor.height().map_err(|_| NativeError::Capture)?;
        if u64::from(source_width) * u64::from(source_height) > 64_000_000 {
            return Err(NativeError::Capture);
        }
        let captured = monitor.capture_image().map_err(|_| NativeError::Capture)?;
        let image = DynamicImage::ImageRgba8(captured).thumbnail(1600, 1000);
        let mut width = image.width();
        let mut height = image.height();
        let mut jpeg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpeg, 65)
            .encode_image(&image.to_rgb8())
            .map_err(|_| NativeError::Capture)?;
        if jpeg.len() > MAX_FRAME_BYTES {
            jpeg.clear();
            let small = image.thumbnail(1000, 700);
            width = small.width();
            height = small.height();
            JpegEncoder::new_with_quality(&mut jpeg, 50)
                .encode_image(&small.to_rgb8())
                .map_err(|_| NativeError::Capture)?;
        }
        if jpeg.len() > MAX_FRAME_BYTES {
            return Err(NativeError::Capture);
        }
        let displays = self
            .monitors
            .iter()
            .map(|m| {
                Ok(RemoteDisplay {
                    id: m.id().map_err(|_| NativeError::Capture)?,
                    name: m
                        .friendly_name()
                        .unwrap_or_else(|_| "Display".into())
                        .chars()
                        .take(64)
                        .collect(),
                    width: m.width().map_err(|_| NativeError::Capture)?,
                    height: m.height().map_err(|_| NativeError::Capture)?,
                })
            })
            .collect::<Result<Vec<_>, NativeError>>()?;
        self.sequence = self.sequence.checked_add(1).ok_or(NativeError::Capture)?;
        Ok(RemoteFrame {
            sequence: self.sequence,
            display_id: monitor.id().map_err(|_| NativeError::Capture)?,
            width,
            height,
            jpeg: STANDARD.encode(jpeg),
            displays,
        })
    }
    pub fn apply(&mut self, input: &RemoteInput) -> Result<(), NativeError> {
        if !input.valid() {
            return Err(NativeError::Invalid);
        }
        if let RemoteInput::Display { id } = input {
            self.release_all()?;
            let monitors = Monitor::all().map_err(|_| NativeError::Capture)?;
            let index = monitors
                .iter()
                .position(|m| m.id().ok() == Some(*id))
                .ok_or(NativeError::Invalid)?;
            if monitors.len() > 16 {
                return Err(NativeError::Capture);
            }
            self.monitors = monitors;
            self.selected = index;
            return Ok(());
        }
        if matches!(input, RemoteInput::ReleaseAll) {
            return self.release_all();
        }
        if matches!(input, RemoteInput::Text { .. }) {
            self.release_all()?;
        }
        self.pressed.observe(input);
        self.inject(input)
    }
    fn inject(&mut self, input: &RemoteInput) -> Result<(), NativeError> {
        let result = match input {
            RemoteInput::Move { x, y } => {
                let m = &self.monitors[self.selected];
                self.enigo.move_mouse(
                    coordinate(
                        m.x().map_err(|_| NativeError::Capture)?,
                        m.width().map_err(|_| NativeError::Capture)?,
                        *x,
                    ),
                    coordinate(
                        m.y().map_err(|_| NativeError::Capture)?,
                        m.height().map_err(|_| NativeError::Capture)?,
                        *y,
                    ),
                    Coordinate::Abs,
                )
            }
            RemoteInput::Button { button, pressed } => self.enigo.button(
                match button {
                    RemoteButton::Left => Button::Left,
                    RemoteButton::Right => Button::Right,
                    RemoteButton::Middle => Button::Middle,
                },
                direction(*pressed),
            ),
            RemoteInput::Key { key, pressed } => self.enigo.key(
                key_code(key).ok_or(NativeError::Invalid)?,
                direction(*pressed),
            ),
            RemoteInput::Scroll { x, y } => {
                self.enigo
                    .scroll(i32::from(*x), Axis::Horizontal)
                    .map_err(|_| NativeError::Input)?;
                self.enigo.scroll(i32::from(*y), Axis::Vertical)
            }
            RemoteInput::Text { text } => self.enigo.text(text),
            RemoteInput::Display { .. } | RemoteInput::ReleaseAll => {
                return Err(NativeError::Invalid);
            }
        };
        result.map_err(|_| NativeError::Input)
    }
    /// 显式释放全部输入；某个释放失败时仍尝试其余项目。
    pub fn release_all(&mut self) -> Result<(), NativeError> {
        let mut failed = false;
        for event in self.pressed.releases() {
            if self.inject(&event).is_err() {
                failed = true;
            }
        }
        if failed {
            Err(NativeError::Input)
        } else {
            Ok(())
        }
    }
}
fn direction(pressed: bool) -> Direction {
    if pressed {
        Direction::Press
    } else {
        Direction::Release
    }
}
fn key_code(key: &str) -> Option<Key> {
    Some(match key {
        "Enter" => Key::Return,
        "Tab" => Key::Tab,
        "Backspace" => Key::Backspace,
        "Escape" => Key::Escape,
        "Delete" => Key::Delete,
        #[cfg(target_os = "windows")]
        "Insert" => Key::Insert,
        // Mac 的这颗物理键是 Help，enigo 不在该平台导出 Insert。
        #[cfg(target_os = "macos")]
        "Insert" => Key::Help,
        "Home" => Key::Home,
        "End" => Key::End,
        "PageUp" => Key::PageUp,
        "PageDown" => Key::PageDown,
        "ArrowUp" => Key::UpArrow,
        "ArrowDown" => Key::DownArrow,
        "ArrowLeft" => Key::LeftArrow,
        "ArrowRight" => Key::RightArrow,
        "Control" => Key::Control,
        "Shift" => Key::Shift,
        "Alt" => Key::Alt,
        "Meta" => Key::Meta,
        "F1" => Key::F1,
        "F2" => Key::F2,
        "F3" => Key::F3,
        "F4" => Key::F4,
        "F5" => Key::F5,
        "F6" => Key::F6,
        "F7" => Key::F7,
        "F8" => Key::F8,
        "F9" => Key::F9,
        "F10" => Key::F10,
        "F11" => Key::F11,
        "F12" => Key::F12,
        other if other.chars().count() == 1 => Key::Unicode(other.chars().next()?),
        _ => return None,
    })
}
