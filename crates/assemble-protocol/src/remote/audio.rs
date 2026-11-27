//! 系统输出音频固定为 48 kHz、双声道、20 ms Opus，不携带麦克风配置。
use serde::{Deserialize, Serialize};

pub const MAX_AUDIO_PACKETS: usize = 12;
pub const MAX_AUDIO_BASE64: usize = 2048;
pub const AUDIO_SAMPLES: usize = 960 * 2;
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AudioState {
    #[default]
    Off,
    Active,
    Unavailable,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AudioPacket {
    pub sequence: u32,
    pub data: String,
}
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteAudio {
    pub state: AudioState,
    pub packets: Vec<AudioPacket>,
}
impl RemoteAudio {
    pub fn valid(&self) -> bool {
        self.packets.len() <= MAX_AUDIO_PACKETS
            && (self.state == AudioState::Active || self.packets.is_empty())
            && self.packets.iter().all(|packet| {
                packet.sequence > 0
                    && !packet.data.is_empty()
                    && packet.data.len() <= MAX_AUDIO_BASE64
            })
            && self
                .packets
                .windows(2)
                .all(|pair| pair[0].sequence < pair[1].sequence)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioPcm {
    pub sequence: u32,
    pub samples: Vec<f32>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioPlayback {
    pub state: AudioState,
    pub packets: Vec<AudioPcm>,
}
