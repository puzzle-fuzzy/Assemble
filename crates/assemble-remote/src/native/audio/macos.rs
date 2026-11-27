//! macOS 13+ 使用 ScreenCaptureKit 系统音频，不创建或选择麦克风输入设备。
use super::{Arc, AtomicBool, Mutex, Ordering, Samples};
use screencapturekit::{prelude::*, stream::delegate_trait::SCStreamDelegateTrait};
pub(super) struct Stream(SCStream);
struct Handler {
    samples: Arc<Mutex<Samples>>,
    failed: Arc<AtomicBool>,
}
struct Delegate(Arc<AtomicBool>);
impl SCStreamDelegateTrait for Delegate {
    fn did_stop_with_error(&self, _error: screencapturekit::error::SCError) {
        self.0.store(true, Ordering::Relaxed);
    }
}
impl SCStreamOutputTrait for Handler {
    fn did_output_sample_buffer(&self, sample: CMSampleBuffer, output_type: SCStreamOutputType) {
        if output_type != SCStreamOutputType::Audio {
            return;
        }
        let Ok(count) = usize::try_from(sample.num_samples()) else {
            return;
        };
        if count == 0 || count > 9600 {
            return;
        }
        let Ok(list) = sample.audio_buffer_list() else {
            self.failed.store(true, Ordering::Relaxed);
            return;
        };
        let buffers: Vec<_> = list.iter().collect();
        let Ok(mut samples) = self.samples.try_lock() else {
            return;
        };
        if buffers.len() == 2
            && buffers
                .iter()
                .all(|buffer| buffer.data_byte_size() == count * 4)
        {
            samples.push(
                48_000,
                buffers[0]
                    .data()
                    .chunks_exact(4)
                    .zip(buffers[1].data().chunks_exact(4))
                    .map(|(left, right)| [float(left), float(right)]),
            );
        } else if buffers.len() == 1 && buffers[0].data_byte_size() == count * 8 {
            samples.push(
                48_000,
                buffers[0]
                    .data()
                    .chunks_exact(8)
                    .map(|frame| [float(&frame[..4]), float(&frame[4..])]),
            );
        } else {
            self.failed.store(true, Ordering::Relaxed);
        }
    }
}
fn float(bytes: &[u8]) -> f32 {
    f32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}
impl Stream {
    pub fn open(
        samples: Arc<Mutex<Samples>>,
        failed: Arc<AtomicBool>,
    ) -> Result<Self, &'static str> {
        let content = SCShareableContent::get().map_err(|_| "AUDIO_CONTENT_UNAVAILABLE")?;
        let display = content
            .displays()
            .into_iter()
            .next()
            .ok_or("AUDIO_NO_DISPLAY")?;
        let filter = SCContentFilter::create()
            .with_display(&display)
            .with_excluding_windows(&[])
            .build()
            .map_err(|_| "AUDIO_FILTER_UNAVAILABLE")?;
        let config = SCStreamConfiguration::new()
            .with_width(2)
            .with_height(2)
            .with_captures_audio(true)
            .with_sample_rate(48_000)
            .with_channel_count(2);
        let mut stream = SCStream::new_with_delegate(&filter, &config, Delegate(failed.clone()))
            .map_err(|_| "AUDIO_STREAM_UNAVAILABLE")?;
        stream
            .add_output_handler(Handler { samples, failed }, SCStreamOutputType::Audio)
            .map_err(|_| "AUDIO_HANDLER_UNAVAILABLE")?;
        stream.start_capture().map_err(|_| "AUDIO_START_FAILED")?;
        Ok(Self(stream))
    }
    pub fn stop(&mut self) {
        let _ = self.0.stop_capture();
    }
}
