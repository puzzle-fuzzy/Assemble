//! 音频回调只保存有界 PCM；重采样与 Opus 编码留在原生工作线程，不在实时回调等待网络。
use assemble_protocol::remote::{AUDIO_SAMPLES, AudioPacket, AudioPcm, AudioPlayback, RemoteAudio};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
use windows::Stream;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos::Stream;

#[derive(Default)]
pub(super) struct Samples {
    frames: VecDeque<[f32; 2]>,
    rate: u32,
    phase: u64,
}
impl Samples {
    pub fn push(&mut self, rate: u32, frames: impl Iterator<Item = [f32; 2]>) {
        if !(8_000..=192_000).contains(&rate) {
            return;
        }
        if self.rate != rate {
            self.frames.clear();
            self.phase = 0;
            self.rate = rate;
        }
        let max = usize::try_from(rate / 5).unwrap_or(9600);
        for frame in frames {
            self.frames.push_back(frame.map(|sample| {
                if sample.is_finite() {
                    sample.clamp(-1.0, 1.0)
                } else {
                    0.0
                }
            }));
            if self.frames.len() > max {
                self.frames.pop_front();
            }
        }
    }
    // 相位始终限制在输入缓冲范围内；转换只用于音频插值，不能作为路径、长度或协议值。
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::cast_sign_loss
    )]
    fn resample(&mut self) -> Vec<f32> {
        let mut output = Vec::with_capacity(19_200);
        if self.rate == 0 {
            return output;
        }
        while (self.phase / 48_000) as usize + 1 < self.frames.len() {
            let index = (self.phase / 48_000) as usize;
            let ratio = (self.phase % 48_000) as f32 / 48_000.0;
            for channel in 0..2 {
                output.push(
                    self.frames[index][channel] * (1.0 - ratio)
                        + self.frames[index + 1][channel] * ratio,
                );
            }
            self.phase += u64::from(self.rate);
        }
        let consumed = ((self.phase / 48_000) as usize).min(self.frames.len().saturating_sub(1));
        self.frames.drain(..consumed);
        self.phase -= consumed as u64 * 48_000;
        output
    }
}
pub(super) struct Capture {
    stream: Option<Stream>,
    samples: Arc<Mutex<Samples>>,
    failed: Arc<AtomicBool>,
    encoder: opus2::Encoder,
    pending: VecDeque<f32>,
}
impl Capture {
    pub fn open() -> Result<Self, &'static str> {
        let samples = Arc::new(Mutex::new(Samples::default()));
        let failed = Arc::new(AtomicBool::new(false));
        let mut encoder =
            opus2::Encoder::new(48_000, opus2::Channels::Stereo, opus2::Application::Audio)
                .map_err(|_| "AUDIO_CODEC_UNAVAILABLE")?;
        encoder
            .set_bitrate(opus2::Bitrate::Bits(64_000))
            .map_err(|_| "AUDIO_CODEC_UNAVAILABLE")?;
        encoder
            .set_inband_fec(true)
            .map_err(|_| "AUDIO_CODEC_UNAVAILABLE")?;
        encoder
            .set_packet_loss_perc(5)
            .map_err(|_| "AUDIO_CODEC_UNAVAILABLE")?;
        let stream = Stream::open(samples.clone(), failed.clone())?;
        Ok(Self {
            stream: Some(stream),
            samples,
            failed,
            encoder,
            pending: VecDeque::new(),
        })
    }
    pub fn take(&mut self, sequence: &mut u32) -> Result<Vec<AudioPacket>, ()> {
        if self.failed.load(Ordering::Relaxed) {
            return Err(());
        }
        self.pending
            .extend(self.samples.lock().map_err(|_| ())?.resample());
        while self.pending.len() > AUDIO_SAMPLES * 6 {
            self.pending.drain(..AUDIO_SAMPLES);
        }
        let mut packets = Vec::new();
        while self.pending.len() >= AUDIO_SAMPLES {
            let pcm: Vec<_> = self.pending.drain(..AUDIO_SAMPLES).collect();
            let mut encoded = [0; 1275];
            let len = self
                .encoder
                .encode_float(&pcm, &mut encoded)
                .map_err(|_| ())?;
            *sequence = sequence.checked_add(1).ok_or(())?;
            packets.push(AudioPacket {
                sequence: *sequence,
                data: STANDARD.encode(&encoded[..len]),
            });
        }
        Ok(packets)
    }
    pub fn stop(&mut self) {
        if let Some(mut stream) = self.stream.take() {
            stream.stop();
        }
        self.pending.clear();
    }
}

pub struct Playback {
    decoder: opus2::Decoder,
    after: u32,
}
impl Playback {
    pub fn open() -> Result<Self, super::NativeError> {
        Ok(Self {
            decoder: opus2::Decoder::new(48_000, opus2::Channels::Stereo)
                .map_err(|_| super::NativeError::Input)?,
            after: 0,
        })
    }
    pub fn decode(&mut self, audio: RemoteAudio) -> Result<AudioPlayback, super::NativeError> {
        if !audio.valid() {
            return Err(super::NativeError::Invalid);
        }
        let mut packets = Vec::new();
        for packet in audio.packets {
            if packet.sequence <= self.after {
                continue;
            }
            let bytes = STANDARD
                .decode(&packet.data)
                .map_err(|_| super::NativeError::Invalid)?;
            if self.after != 0 && packet.sequence != self.after + 1 {
                if packet.sequence == self.after + 2 {
                    if let Ok(recovered) = self.decode_packet(&bytes, true) {
                        packets.push(AudioPcm {
                            sequence: packet.sequence - 1,
                            samples: recovered,
                        });
                    }
                } else {
                    self.decoder
                        .reset_state()
                        .map_err(|_| super::NativeError::Input)?;
                }
            }
            let samples = self.decode_packet(&bytes, false)?;
            self.after = packet.sequence;
            packets.push(AudioPcm {
                sequence: packet.sequence,
                samples,
            });
        }
        Ok(AudioPlayback {
            state: audio.state,
            packets,
        })
    }
    fn decode_packet(&mut self, bytes: &[u8], fec: bool) -> Result<Vec<f32>, super::NativeError> {
        let mut samples = vec![0.0; AUDIO_SAMPLES];
        let count = self
            .decoder
            .decode_float(bytes, &mut samples, fec)
            .map_err(|_| super::NativeError::Invalid)?;
        if count != 960 || samples.iter().any(|sample| !sample.is_finite()) {
            return Err(super::NativeError::Invalid);
        }
        Ok(samples)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use assemble_protocol::remote::AudioState;
    #[test]
    fn resampling_and_real_opus_keep_fixed_twenty_millisecond_packets() {
        let mut samples = Samples::default();
        samples.push(44_100, (0..442).map(|_| [0.1, -0.1]));
        assert_eq!(samples.resample().len(), 960);
        let mut encoder =
            opus2::Encoder::new(48_000, opus2::Channels::Stereo, opus2::Application::Audio)
                .unwrap();
        let mut output = [0; 1275];
        let len = encoder
            .encode_float(&vec![0.1; AUDIO_SAMPLES], &mut output)
            .unwrap();
        let mut decoder = Playback::open().unwrap();
        let data = RemoteAudio {
            state: AudioState::Active,
            packets: vec![AudioPacket {
                sequence: 1,
                data: STANDARD.encode(&output[..len]),
            }],
        };
        let pcm = decoder.decode(data.clone()).unwrap();
        assert_eq!(pcm.packets[0].samples.len(), AUDIO_SAMPLES);
        assert!(decoder.decode(data).unwrap().packets.is_empty());
    }
}
