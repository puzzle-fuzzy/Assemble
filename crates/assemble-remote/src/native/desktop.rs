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
    quality: crate::quality::QualityPolicy,
    video_enabled: bool,
    video: Option<super::video::Video>,
    video_stream: u32,
    audio: Option<super::audio::Capture>,
    audio_state: assemble_protocol::remote::AudioState,
    audio_sequence: u32,
    audio_error: Option<&'static str>,
}
impl crate::worker::Backend for Host {
    type Error = NativeError;
    fn display_id(&self) -> Option<u32> {
        self.monitors
            .get(self.selected)
            .and_then(|monitor| monitor.id().ok())
    }
    fn frame(&mut self) -> Result<RemoteFrame, NativeError> {
        Self::frame(self)
    }
    fn apply(&mut self, input: &RemoteInput) -> Result<(), NativeError> {
        Self::apply(self, input)
    }
    fn release_all(&mut self) -> Result<(), NativeError> {
        Self::release_all(self)
    }
    fn clipboard(
        &mut self,
        task: &assemble_protocol::remote::ClipboardTask,
    ) -> assemble_protocol::remote::ClipboardResult {
        if self.release_all().is_err() {
            return assemble_protocol::remote::ClipboardResult {
                id: task.request.id.clone(),
                status: assemble_protocol::remote::ClipboardStatus::Unavailable,
                text: None,
            };
        }
        super::clipboard::exchange(task, || {
            let modifier = if cfg!(target_os = "macos") {
                "Meta"
            } else {
                "Control"
            };
            let mut ok = true;
            for (key, pressed) in [
                (modifier, true),
                ("v", true),
                ("v", false),
                (modifier, false),
            ] {
                if self
                    .apply(&RemoteInput::Key {
                        key: key.into(),
                        pressed,
                    })
                    .is_err()
                {
                    ok = false;
                    break;
                }
            }
            self.release_all().is_ok() && ok
        })
    }
    fn feedback(&mut self, elapsed: std::time::Duration) {
        self.quality.observe(elapsed);
    }
    fn audio(&mut self) -> assemble_protocol::remote::RemoteAudio {
        use assemble_protocol::remote::{AudioState, RemoteAudio};
        if let Some(audio) = &mut self.audio {
            if let Ok(packets) = audio.take(&mut self.audio_sequence) {
                return RemoteAudio {
                    state: AudioState::Active,
                    packets,
                };
            }
            audio.stop();
            self.audio = None;
            self.audio_state = AudioState::Unavailable;
            self.audio_error = Some("AUDIO_CAPTURE_FAILED");
        }
        RemoteAudio {
            state: self.audio_state,
            packets: Vec::new(),
        }
    }
    fn shutdown(&mut self) -> Result<(), NativeError> {
        if let Some(mut audio) = self.audio.take() {
            audio.stop();
        }
        self.video = None;
        self.release_all()
    }
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
            quality: crate::quality::QualityPolicy::default(),
            video_enabled: false,
            video: None,
            video_stream: 0,
            audio: None,
            audio_state: assemble_protocol::remote::AudioState::Off,
            audio_sequence: 0,
            audio_error: None,
        })
    }
    pub fn frame(&mut self) -> Result<RemoteFrame, NativeError> {
        let monitor = &self.monitors[self.selected];
        let display_id = monitor.id().map_err(|_| NativeError::Capture)?;
        let source_width = monitor.width().map_err(|_| NativeError::Capture)?;
        let source_height = monitor.height().map_err(|_| NativeError::Capture)?;
        let cursor = self.enigo.location().ok().and_then(|(x, y)| {
            Some(assemble_protocol::remote::RemoteCursor {
                x: crate::normalize_coordinate(monitor.x().ok()?, source_width, x)?,
                y: crate::normalize_coordinate(monitor.y().ok()?, source_height, y)?,
            })
        });
        if u64::from(source_width) * u64::from(source_height) > 64_000_000 {
            return Err(NativeError::Capture);
        }
        let captured = monitor.capture_image().map_err(|_| NativeError::Capture)?;
        let (max_width, max_height, quality) = self.quality.profile();
        let mut image = DynamicImage::ImageRgba8(captured).thumbnail(max_width, max_height);
        if self.video_enabled {
            image = image.resize_exact(
                (image.width() & !1).max(2),
                (image.height() & !1).max(2),
                image::imageops::FilterType::Triangle,
            );
        }
        let mut width = image.width();
        let mut height = image.height();
        let mut jpeg = Vec::new();
        let video = if self.video_enabled {
            self.encode_video(&image).ok()
        } else {
            None
        };
        if video.is_none() {
            self.video_enabled = false;
            self.video = None;
            JpegEncoder::new_with_quality(&mut jpeg, quality)
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
            display_id,
            width,
            height,
            jpeg: STANDARD.encode(jpeg),
            displays,
            quality: self.quality.effective(),
            requested_quality: self.quality.requested(),
            video,
            cursor,
        })
    }
    /// 手动探针只输出有限错误码，不输出设备名称或平台错误中的私有信息。
    pub fn audio_diagnostic(&self) -> Option<&'static str> {
        self.audio_error
    }
    fn encode_video(
        &mut self,
        image: &DynamicImage,
    ) -> Result<assemble_protocol::remote::RemoteVideo, NativeError> {
        let quality = self.quality.effective();
        if self
            .video
            .as_ref()
            .is_none_or(|video| !video.matches(image, quality))
        {
            self.video_stream = self
                .video_stream
                .checked_add(1)
                .ok_or(NativeError::Capture)?;
            self.video = Some(super::video::Video::new(image, quality, self.video_stream)?);
        }
        self.video
            .as_mut()
            .ok_or(NativeError::Capture)?
            .encode(image)
    }
    pub fn apply(&mut self, input: &RemoteInput) -> Result<(), NativeError> {
        if !input.valid() {
            return Err(NativeError::Invalid);
        }
        if let RemoteInput::Quality { quality } = input {
            self.quality.select(*quality);
            return Ok(());
        }
        if let RemoteInput::Video { enabled } = input {
            self.video_enabled = *enabled;
            self.video = None;
            return Ok(());
        }
        if let RemoteInput::Audio { enabled } = input {
            if let Some(mut audio) = self.audio.take() {
                audio.stop();
            }
            self.audio_state = assemble_protocol::remote::AudioState::Off;
            self.audio_error = None;
            if *enabled {
                match super::audio::Capture::open() {
                    Ok(audio) => {
                        self.audio = Some(audio);
                        self.audio_state = assemble_protocol::remote::AudioState::Active;
                    }
                    Err(error) => {
                        self.audio_error = Some(error);
                        self.audio_state = assemble_protocol::remote::AudioState::Unavailable;
                    }
                }
            }
            return Ok(());
        }
        if matches!(input, RemoteInput::Keyframe) {
            if let Some(video) = &mut self.video {
                video.keyframe();
            }
            return Ok(());
        }
        if let RemoteInput::Display { id } = input {
            self.release_all()?;
            // 选择器的名单可能已经过时；目标消失不应破坏仍可采集的当前屏幕。
            if let Ok(monitors) = Monitor::all()
                && !monitors.is_empty()
                && monitors.len() <= 16
                && let Some(index) = monitors.iter().position(|m| m.id().ok() == Some(*id))
            {
                self.monitors = monitors;
                self.selected = index;
                self.video = None;
            }
            return Ok(());
        }
        if matches!(input, RemoteInput::ReleaseAll) {
            return self.release_all();
        }
        if matches!(input, RemoteInput::Text { .. }) {
            self.release_all()?;
        }
        self.pressed.before_inject(input);
        let result = self.inject(input);
        self.pressed.after_inject(input, result.is_ok());
        result
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
            RemoteInput::Display { .. }
            | RemoteInput::ReleaseAll
            | RemoteInput::Quality { .. }
            | RemoteInput::Video { .. }
            | RemoteInput::Audio { .. }
            | RemoteInput::Keyframe => {
                return Err(NativeError::Invalid);
            }
        };
        result.map_err(|_| NativeError::Input)
    }
    /// 显式释放全部输入；某个释放失败时仍尝试其余项目。
    pub fn release_all(&mut self) -> Result<(), NativeError> {
        let mut failed = false;
        for event in self.pressed.releases() {
            let released = self.inject(&event).is_ok();
            self.pressed.after_inject(&event, released);
            if !released {
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
