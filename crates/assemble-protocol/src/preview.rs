//! 云盘预览描述；不携带读取凭据、服务器路径或资源授权 URL。
use serde::{Deserialize, Serialize};

pub const TEXT_PREVIEW_BYTES: usize = 1024 * 1024;
pub const PREVIEW_CHUNK_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PreviewKind {
    Image,
    Text,
    Audio,
    Video,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewInfo {
    pub kind: PreviewKind,
    pub mime_type: Option<String>,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilePreview {
    #[serde(flatten)]
    pub info: PreviewInfo,
    pub text: Option<String>,
    pub truncated: bool,
}

impl FilePreview {
    pub fn valid(&self) -> bool {
        let expected = match self.info.kind {
            PreviewKind::Image => ["image/jpeg", "image/png", "image/webp"].as_slice(),
            PreviewKind::Audio => ["audio/mpeg", "audio/wav", "audio/mp4", "audio/webm"].as_slice(),
            PreviewKind::Video => ["video/mp4", "video/webm"].as_slice(),
            PreviewKind::Text => ["text/plain; charset=utf-8"].as_slice(),
            PreviewKind::Unsupported => [].as_slice(),
        };
        let mime_valid = if self.info.kind == PreviewKind::Unsupported {
            self.info.mime_type.is_none()
        } else {
            self.info
                .mime_type
                .as_deref()
                .is_some_and(|mime| expected.contains(&mime))
        };
        mime_valid
            && if self.info.kind == PreviewKind::Text {
                self.text
                    .as_ref()
                    .is_some_and(|text| text.len() <= TEXT_PREVIEW_BYTES)
            } else {
                self.text.is_none() && !self.truncated
            }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_and_typescript_share_the_preview_contract() {
        let samples: Vec<FilePreview> =
            serde_json::from_str(include_str!("../../../apps/desktop/contract/previews.json"))
                .unwrap();
        assert_eq!(samples.len(), 5);
        for sample in &samples {
            assert!(sample.valid());
            assert_eq!(
                serde_json::from_value::<FilePreview>(serde_json::to_value(sample).unwrap())
                    .unwrap(),
                *sample
            );
        }
        let mut invalid = samples[0].clone();
        invalid.info.mime_type = Some("text/html".into());
        assert!(!invalid.valid());
        invalid.info.mime_type = Some("text/plain; charset=utf-8".into());
        invalid.text = Some("中".repeat(TEXT_PREVIEW_BYTES));
        assert!(!invalid.valid());
        let mut invalid = samples[1].clone();
        invalid.text = Some("private content".into());
        assert!(!invalid.valid());
    }
}
