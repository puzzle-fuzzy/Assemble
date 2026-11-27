//! 使用稳定封装的跨平台软件编码；编码器和 YUV 缓冲留在采集线程，不调用 raw API。
use super::NativeError;
use assemble_protocol::remote::{MAX_FRAME_BYTES, RemoteQuality, RemoteVideo};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use image::DynamicImage;
use openh264::{
    OpenH264API,
    encoder::{
        BitRate, Encoder, EncoderConfig, FrameRate, FrameType, IntraFramePeriod, Profile, UsageType,
    },
    formats::{RgbaSliceU8, YUVBuffer},
};

pub(super) struct Video {
    encoder: Encoder,
    yuv: YUVBuffer,
    dimensions: (u32, u32),
    quality: RemoteQuality,
    stream: u32,
    codec: String,
    started: std::time::Instant,
}
impl Video {
    pub fn new(
        image: &DynamicImage,
        quality: RemoteQuality,
        stream: u32,
    ) -> Result<Self, NativeError> {
        let bps = match quality {
            RemoteQuality::Smooth => 1_000_000,
            RemoteQuality::Sharp => 6_000_000,
            _ => 3_000_000,
        };
        let config = EncoderConfig::new()
            .bitrate(BitRate::from_bps(bps))
            .max_frame_rate(FrameRate::from_hz(30.0))
            .usage_type(UsageType::ScreenContentRealTime)
            .profile(Profile::Baseline)
            .skip_frames(false)
            .intra_frame_period(IntraFramePeriod::from_num_frames(30))
            .num_threads(2)
            .debug(false);
        let encoder = Encoder::with_api_config(OpenH264API::from_source(), config)
            .map_err(|_| NativeError::Capture)?;
        Ok(Self {
            encoder,
            yuv: YUVBuffer::new(image.width() as usize, image.height() as usize),
            dimensions: (image.width(), image.height()),
            quality,
            stream,
            codec: String::new(),
            started: std::time::Instant::now(),
        })
    }
    pub fn matches(&self, image: &DynamicImage, quality: RemoteQuality) -> bool {
        self.dimensions == (image.width(), image.height()) && self.quality == quality
    }
    pub fn keyframe(&mut self) {
        self.encoder.force_intra_frame();
    }
    pub fn encode(&mut self, image: &DynamicImage) -> Result<RemoteVideo, NativeError> {
        let rgba = image.to_rgba8();
        self.yuv.read_rgba8(RgbaSliceU8::new(
            rgba.as_raw(),
            (rgba.width() as usize, rgba.height() as usize),
        ));
        let encoded = self
            .encoder
            .encode(&self.yuv)
            .map_err(|_| NativeError::Capture)?;
        let keyframe = encoded.frame_type() == FrameType::IDR;
        let bytes = encoded.to_vec();
        if bytes.is_empty() || bytes.len() > MAX_FRAME_BYTES {
            return Err(NativeError::Capture);
        }
        if keyframe {
            self.codec = codec(&bytes).ok_or(NativeError::Capture)?;
        }
        Ok(RemoteVideo {
            stream: self.stream,
            codec: self.codec.clone(),
            keyframe,
            timestamp: u64::try_from(self.started.elapsed().as_micros())
                .map_err(|_| NativeError::Capture)?,
            data: STANDARD.encode(bytes),
        })
    }
}
fn codec(bytes: &[u8]) -> Option<String> {
    for index in 0..bytes.len().saturating_sub(7) {
        let prefix = if bytes[index..].starts_with(&[0, 0, 0, 1]) {
            4
        } else if bytes[index..].starts_with(&[0, 0, 1]) {
            3
        } else {
            continue;
        };
        if bytes.get(index + prefix).is_some_and(|nal| nal & 31 == 7) {
            return Some(format!(
                "avc1.{:02x}{:02x}{:02x}",
                bytes[index + prefix + 1],
                bytes[index + prefix + 2],
                bytes[index + prefix + 3]
            ));
        }
    }
    None
}
#[cfg(test)]
mod tests {
    use super::*;
    use openh264::formats::YUVSource;
    #[test]
    fn real_h264_roundtrip_and_forced_keyframes() {
        let image = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            64,
            48,
            image::Rgba([32, 64, 96, 255]),
        ));
        let mut encoder = Video::new(&image, RemoteQuality::Balanced, 1).unwrap();
        let mut decoder = openh264::decoder::Decoder::new().unwrap();
        for index in 0..4 {
            if index == 3 {
                encoder.keyframe();
            }
            let frame = encoder.encode(&image).unwrap();
            assert!(frame.valid());
            assert_eq!(frame.keyframe, index == 0 || index == 3);
            let bytes = STANDARD.decode(frame.data).unwrap();
            let decoded = decoder.decode(&bytes).unwrap().unwrap();
            assert_eq!(decoded.dimensions(), (64, 48));
        }
    }
}
