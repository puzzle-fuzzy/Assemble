//! 各协议域共用的值原语。

use std::borrow::Cow;
use std::ops::Add;
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// 单个文件的大小上限（含）：100,000,000 字节。
pub const MAX_FILE_SIZE: u64 = 100_000_000;

/// 资源、设备与发送的标识：1 到 200 个 `A-Z a-z 0-9 _ . -` 字符。
pub fn is_valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 200
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
}

/// 文件内容摘要：64 位小写十六进制。
pub fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// 字节的经典小写十六进制编码。摘要、随机令牌与分段文件名都用它。
pub fn to_hex(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;

        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// 非空且不超过上限的 UTF-16 单元数。
pub fn is_text(value: &str, max_units: usize) -> bool {
    !value.trim().is_empty() && value.encode_utf16().count() <= max_units
}

/// 运行桌面端的平台。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Windows,
    Macos,
    Linux,
}

impl Platform {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::Macos => "macos",
            Self::Linux => "linux",
        }
    }

    /// 当前编译目标对应的平台。
    pub const fn current() -> Self {
        if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::Macos
        } else {
            Self::Linux
        }
    }
}

/// 一个时间点。线上是 UTC 的 RFC 3339 字符串，精确到毫秒。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(DateTime<Utc>);

impl Timestamp {
    pub fn now() -> Self {
        Self(Utc::now())
    }

    /// 从 Unix 纪元起的毫秒数；超出可表示范围时为 `None`。
    pub fn from_millis(millis: i64) -> Option<Self> {
        DateTime::from_timestamp_millis(millis).map(Self)
    }

    /// 从 Unix 纪元起的毫秒数。
    pub fn as_millis(self) -> i64 {
        self.0.timestamp_millis()
    }

    /// 比另一个时间点晚多久；对方更晚时为零。
    pub fn elapsed_since(self, earlier: Self) -> Duration {
        self.0
            .signed_duration_since(earlier.0)
            .to_std()
            .unwrap_or_default()
    }
}

impl Add<Duration> for Timestamp {
    type Output = Self;

    fn add(self, duration: Duration) -> Self::Output {
        Self(self.0 + chrono::Duration::from_std(duration).unwrap_or(chrono::Duration::MAX))
    }
}

impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_rfc3339_opts(SecondsFormat::Millis, true))
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // 借得到就借，借不到（比如从 `Value` 里读）再复制一份。
        let text = Cow::<str>::deserialize(deserializer)?;
        let parsed = DateTime::parse_from_rfc3339(&text).map_err(serde::de::Error::custom)?;
        Ok(Self(parsed.with_timezone(&Utc)))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn id_accepts_word_characters_dot_and_dash_only() {
        assert!(is_valid_id("a"));
        assert!(is_valid_id("Device_01.test-x"));
        assert!(is_valid_id(&"a".repeat(200)));
        assert!(!is_valid_id(""));
        assert!(!is_valid_id(&"a".repeat(201)));
        assert!(!is_valid_id("has space"));
        assert!(!is_valid_id("slash/"));
        assert!(!is_valid_id("中文"));
    }

    #[test]
    fn sha256_requires_64_lowercase_hex_characters() {
        assert!(is_sha256_hex(&"a".repeat(64)));
        assert!(!is_sha256_hex(&"A".repeat(64)));
        assert!(!is_sha256_hex(&"g".repeat(64)));
        assert!(!is_sha256_hex(&"a".repeat(63)));
    }

    #[test]
    fn hex_encoding_is_lowercase_and_zero_padded() {
        assert_eq!(to_hex(&[]), "");
        assert_eq!(to_hex(&[0]), "00");
        assert_eq!(to_hex(&[0x0a, 0xff, 0x10]), "0aff10");
        assert_eq!(to_hex(&[0xab; 32]).len(), 64);
        assert!(is_sha256_hex(&to_hex(&[0xab; 32])));
    }

    #[test]
    fn text_must_be_non_blank_and_bounded() {
        assert!(is_text("标题", 1_000));
        assert!(!is_text("   ", 1_000));
        assert!(!is_text(&"a".repeat(1_001), 1_000));
        assert!(!is_text("备注", 0));
    }

    #[test]
    fn platform_round_trips_with_its_spelling() {
        for (platform, spelling) in [
            (Platform::Windows, "\"windows\""),
            (Platform::Macos, "\"macos\""),
            (Platform::Linux, "\"linux\""),
        ] {
            assert_eq!(serde_json::to_string(&platform).unwrap(), spelling);
            assert_eq!(platform.as_str(), spelling.trim_matches('"'));
            assert_eq!(
                serde_json::from_str::<Platform>(spelling).unwrap(),
                platform
            );
        }
    }

    #[test]
    fn timestamps_travel_as_utc_rfc3339_with_milliseconds() {
        let at = Timestamp::from_millis(1_794_400_000_123).unwrap();
        assert_eq!(at.as_millis(), 1_794_400_000_123);
        assert_eq!(
            serde_json::to_value(at).unwrap(),
            json!("2026-11-11T12:26:40.123Z")
        );
        // 带时区偏移的写法读进来之后换算成 UTC。
        let parsed: Timestamp =
            serde_json::from_value(json!("2026-11-11T20:26:40.123+08:00")).unwrap();
        assert_eq!(parsed, at);
        for rejected in [
            json!("2026-11-11T12:26:40"),
            json!(1_794_400_000_123_i64),
            json!("yesterday"),
        ] {
            assert!(serde_json::from_value::<Timestamp>(rejected).is_err());
        }
        assert!(Timestamp::from_millis(i64::MAX).is_none());
    }

    #[test]
    fn timestamps_add_durations_and_measure_elapsed_time() {
        let at = Timestamp::from_millis(1_794_400_000_000).unwrap();
        let later = at + Duration::from_millis(86_400_000);
        assert_eq!(later.as_millis() - at.as_millis(), 86_400_000);
        assert_eq!(later.elapsed_since(at), Duration::from_millis(86_400_000));
        // 对方更晚时为零，不出负数。
        assert_eq!(at.elapsed_since(later), Duration::ZERO);
        assert!(later > at);
        assert!(Timestamp::now() > at);
    }
}
