//! 只打开默认输出端点的 WASAPI loopback，绝不回退到默认输入设备。
use super::{Arc, AtomicBool, Mutex, Ordering, Samples};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
pub(super) struct Stream(cpal::Stream);
impl Stream {
    pub fn open(
        samples: Arc<Mutex<Samples>>,
        failed: Arc<AtomicBool>,
    ) -> Result<Self, &'static str> {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or("AUDIO_NO_OUTPUT")?;
        if device.supports_input() {
            return Err("AUDIO_NOT_OUTPUT_ONLY");
        }
        let config = device
            .default_output_config()
            .map_err(|_| "AUDIO_CONFIG_UNAVAILABLE")?;
        if config.channels() == 0
            || config.channels() > 8
            || !(8_000..=192_000).contains(&config.sample_rate())
        {
            return Err("AUDIO_FORMAT_UNSUPPORTED");
        }
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => build::<f32>(&device, config.config(), samples, failed),
            cpal::SampleFormat::I16 => build::<i16>(&device, config.config(), samples, failed),
            cpal::SampleFormat::I32 => build::<i32>(&device, config.config(), samples, failed),
            _ => return Err("AUDIO_FORMAT_UNSUPPORTED"),
        }?;
        stream.play().map_err(|_| "AUDIO_START_FAILED")?;
        Ok(Self(stream))
    }
    pub fn stop(&mut self) {
        let _ = self.0.pause();
    }
}
fn build<T: cpal::SizedSample>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    samples: Arc<Mutex<Samples>>,
    failed: Arc<AtomicBool>,
) -> Result<cpal::Stream, &'static str>
where
    f32: cpal::FromSample<T>,
{
    let channels = usize::from(config.channels);
    device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                if let Ok(mut samples) = samples.try_lock() {
                    samples.push(
                        config.sample_rate,
                        data.chunks_exact(channels).map(|frame| {
                            let left = frame[0].to_sample::<f32>();
                            [
                                left,
                                if channels == 1 {
                                    left
                                } else {
                                    frame[1].to_sample::<f32>()
                                },
                            ]
                        }),
                    );
                }
            },
            move |error| {
                // CPAL 也通过这个回调报告调度降级、瞬时欠载和已自动切换的音频路由；
                // 这些不使流失效，不能把仍然工作的系统声音立即关掉。
                if fatal(error.kind()) {
                    failed.store(true, Ordering::Relaxed);
                }
            },
            Some(std::time::Duration::from_secs(2)),
        )
        .map_err(|_| "AUDIO_STREAM_UNAVAILABLE")
}
fn fatal(kind: cpal::ErrorKind) -> bool {
    !matches!(
        kind,
        cpal::ErrorKind::RealtimeDenied | cpal::ErrorKind::Xrun | cpal::ErrorKind::DeviceChanged
    )
}
#[cfg(test)]
mod tests {
    use super::fatal;
    use cpal::ErrorKind;
    #[test]
    fn advisory_audio_events_do_not_disable_a_live_stream() {
        assert!(!fatal(ErrorKind::RealtimeDenied));
        assert!(!fatal(ErrorKind::Xrun));
        assert!(!fatal(ErrorKind::DeviceChanged));
        assert!(fatal(ErrorKind::DeviceNotAvailable));
        assert!(fatal(ErrorKind::StreamInvalidated));
        assert!(fatal(ErrorKind::PermissionDenied));
    }
}
