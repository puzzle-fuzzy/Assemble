//! 匿名下载页的公开发布资料；不包含账户、服务器路径或任意外部地址。

use crate::primitives::Timestamp;
use serde::{Deserialize, Deserializer, Serialize};

pub const RELEASES_PATH: &str = "/v1/releases";
pub const MAX_MANIFEST_BYTES: u64 = 128 * 1024;
pub const MAX_ARTIFACT_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const MAX_ARTIFACTS: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReleasePlatform {
    Windows,
    Macos,
    Ios,
    Android,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReleaseArchitecture {
    X64,
    Arm64,
    Universal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseArtifact {
    pub platform: ReleasePlatform,
    pub architecture: ReleaseArchitecture,
    pub version: String,
    pub file_name: String,
    pub sha256: String,
    pub size: u64,
    #[serde(deserialize_with = "release_time")]
    pub published_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseManifest {
    pub schema_version: u32,
    pub artifacts: Vec<ReleaseArtifact>,
}

pub fn valid_digest(value: &str) -> bool {
    crate::primitives::is_sha256_hex(value)
}

fn release_time<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Timestamp, D::Error> {
    let text = String::deserialize(deserializer)?;
    let parsed = chrono::DateTime::parse_from_rfc3339(&text).map_err(serde::de::Error::custom)?;
    // 与维护脚本的 Date.toISOString 完全一致，不接受滚日、闰秒或其他偏移写法。
    if text.len() != 24
        || text.get(17..19) == Some("60")
        || text
            != parsed
                .with_timezone(&chrono::Utc)
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    {
        return Err(serde::de::Error::custom(
            "release timestamp must be canonical UTC milliseconds",
        ));
    }
    Timestamp::from_millis(parsed.timestamp_millis())
        .ok_or_else(|| serde::de::Error::custom("release timestamp out of range"))
}

impl ReleaseArtifact {
    pub fn is_valid(&self) -> bool {
        use ReleaseArchitecture::{Arm64, Universal, X64};
        use ReleasePlatform::{Android, Ios, Macos, Windows};
        let architecture = matches!(
            (self.platform, self.architecture),
            (Windows, X64 | Arm64) | (Macos | Android, X64 | Arm64 | Universal) | (Ios, Arm64)
        );
        let filename = self.file_name.starts_with("Assemble-")
            && self.file_name.len() <= 128
            && !self.file_name.contains("..")
            && self
                .file_name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte));
        let extension = self
            .file_name
            .rsplit('.')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let format = match self.platform {
            Windows => matches!(extension.as_str(), "exe" | "msi"),
            Macos => matches!(extension.as_str(), "dmg" | "pkg"),
            Ios => extension == "ipa",
            Android => extension == "apk",
        };
        let version = !self.version.is_empty()
            && self.version.len() <= 64
            && self.version.as_bytes()[0].is_ascii_digit()
            && self
                .version
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b".+-".contains(&byte));
        architecture
            && filename
            && format
            && version
            && valid_digest(&self.sha256)
            && self.size > 0
            && self.size <= MAX_ARTIFACT_BYTES
    }
}

impl ReleaseManifest {
    pub fn is_valid(&self) -> bool {
        self.schema_version == 1
            && self.artifacts.len() <= MAX_ARTIFACTS
            && self.artifacts.iter().enumerate().all(|(index, artifact)| {
                artifact.is_valid()
                    && self.artifacts[..index].iter().all(|other| {
                        other.sha256 != artifact.sha256
                            && !(other.platform == artifact.platform
                                && other.architecture == artifact.architecture
                                && other.version == artifact.version)
                    })
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_fixture_pins_the_web_contract_and_rejects_paths_and_urls() {
        let source = include_str!("../../../apps/releases-web/contract/releases.json");
        let manifest: ReleaseManifest = serde_json::from_str(source).unwrap();
        assert!(manifest.is_valid());
        for filename in [
            "../Assemble-test.exe",
            "Assemble-test\r\nX: bad.exe",
            "Assemble-..exe",
        ] {
            let mut bad = manifest.clone();
            bad.artifacts[0].file_name = filename.into();
            assert!(!bad.is_valid());
        }
        assert!(
            serde_json::from_str::<ReleaseManifest>(&source.replace(
                "\"schemaVersion\": 1",
                "\"downloadUrl\": \"https://example.test\", \"schemaVersion\": 1"
            ))
            .is_err()
        );
        let mut duplicate = manifest.clone();
        duplicate.artifacts.push(manifest.artifacts[0].clone());
        assert!(!duplicate.is_valid());
    }

    #[test]
    fn release_dates_use_the_same_strict_utc_milliseconds_as_the_publisher() {
        let source = include_str!("../../../apps/releases-web/contract/releases.json");
        let invalid: Vec<String> = serde_json::from_str(include_str!(
            "../../../apps/releases-web/contract/invalid-times.json"
        ))
        .unwrap();
        let mut value: serde_json::Value = serde_json::from_str(source).unwrap();
        for text in invalid {
            value["artifacts"][0]["publishedAt"] = serde_json::json!(text);
            assert!(serde_json::from_value::<ReleaseManifest>(value.clone()).is_err());
        }
        value["artifacts"][0]["publishedAt"] = serde_json::json!("2024-02-29T23:59:59.123Z");
        assert!(
            serde_json::from_value::<ReleaseManifest>(value)
                .unwrap()
                .is_valid()
        );
    }
}
