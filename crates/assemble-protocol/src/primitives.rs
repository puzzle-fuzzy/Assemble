//! 各契约域共用的值原语。

use serde::{Deserialize, Serialize};

/// 单个文件的字节数上限（不含）：严格小于该值才可扫描上云。
pub const MAX_FILE_SIZE: u64 = 100_000_000;

/// `text` 字段的最大长度（以 UTF-16 单元计）。
pub const TEXT_MAX_UNITS: usize = 1_000;

/// `notes` 字段的最大长度（以 UTF-16 单元计）。
pub const NOTES_MAX_UNITS: usize = 20_000;

/// 资源、设备与命令标识：1 到 200 个 `A-Z a-z 0-9 _ . -` 字符。
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

/// 非空且不超过上限的 UTF-16 单元数，对应 zod 的 `.trim().min(1).max(n)`。
pub fn is_text(value: &str, max_units: usize) -> bool {
    !value.trim().is_empty() && value.encode_utf16().count() <= max_units
}

/// 相对路径在契约层的安全形式：不以分隔符或盘符开头，不含空段、`.` 与 `..`。
///
/// 本机落盘前还要再过一遍 `assemble-device` 的文件系统校验；
/// 这里只保证不会把明显越界的路径发到服务端。
pub fn is_safe_relative_path(value: &str) -> bool {
    if value.is_empty()
        || value.encode_utf16().count() > 4_096
        || value.contains('\0')
        || value.starts_with(['/', '\\'])
    {
        return false;
    }
    // `^[a-z]:`（不区分大小写）被当作盘符前缀拒绝。
    if let Some(prefix) = value.get(..2) {
        let is_drive = prefix.ends_with(':')
            && prefix
                .chars()
                .next()
                .is_some_and(|first| first.to_lowercase().any(|lower| lower.is_ascii_lowercase()));
        if is_drive {
            return false;
        }
    }
    !value
        .split(['/', '\\'])
        .any(|part| matches!(part, "" | "." | ".."))
}

/// 运行桌面端的平台，对应服务端的 `platformSchema`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Win32,
    Darwin,
    Linux,
}

impl Platform {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Win32 => "win32",
            Self::Darwin => "darwin",
            Self::Linux => "linux",
        }
    }

    /// 当前编译目标对应的平台。
    pub const fn current() -> Self {
        if cfg!(target_os = "windows") {
            Self::Win32
        } else if cfg!(target_os = "macos") {
            Self::Darwin
        } else {
            Self::Linux
        }
    }
}

#[cfg(test)]
mod tests {
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
        assert!(is_text("标题", TEXT_MAX_UNITS));
        assert!(!is_text("   ", TEXT_MAX_UNITS));
        assert!(!is_text(&"a".repeat(TEXT_MAX_UNITS + 1), TEXT_MAX_UNITS));
        assert!(!is_text("备注", 0));
    }

    #[test]
    fn relative_path_rejects_escapes_and_drive_prefixes() {
        assert!(is_safe_relative_path("a/b.txt"));
        assert!(is_safe_relative_path("a\\b.txt"));
        assert!(!is_safe_relative_path(""));
        assert!(!is_safe_relative_path("/etc/passwd"));
        assert!(!is_safe_relative_path("\\\\server\\share"));
        assert!(!is_safe_relative_path("C:/Windows"));
        assert!(!is_safe_relative_path("c:relative"));
        assert!(!is_safe_relative_path("a/../b"));
        assert!(!is_safe_relative_path("./a"));
        assert!(!is_safe_relative_path("a//b"));
        assert!(!is_safe_relative_path("a/b/"));
        assert!(!is_safe_relative_path("a\0b"));
    }

    #[test]
    fn platform_round_trips_with_server_spelling() {
        for (platform, spelling) in [
            (Platform::Win32, "\"win32\""),
            (Platform::Darwin, "\"darwin\""),
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
}
