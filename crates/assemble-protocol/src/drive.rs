//! 云盘：账户在服务端的一份文件。第一版是平铺的一层，没有文件夹。

use serde::{Deserialize, Serialize};

use crate::primitives::{Timestamp, is_text};

/// 文件名字的上限（UTF-16 单元），与常见文件系统的一致。
pub const FILE_NAME_MAX_UNITS: usize = 255;

/// 一段可以安全地当成文件名的文字：不是路径。
///
/// 不能为空、不能超上限、不含控制字符与路径分隔符，也不是 `.` 或 `..`。
/// 两端的空白先去掉再判断，保存的也是去掉之后的名字。
pub fn clean_file_name(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    let is_path_piece = |part: &str| matches!(part, "." | "..");
    if !is_text(trimmed, FILE_NAME_MAX_UNITS)
        || trimmed.contains(['/', '\\'])
        || trimmed.chars().any(char::is_control)
        || is_path_piece(trimmed)
    {
        return None;
    }
    Some(trimmed)
}

/// 媒体类型的上限；超过的一律当作二进制。
pub const MIME_TYPE_MAX_BYTES: usize = 127;

/// 一段像样的媒体类型：`类型/子类型`，可带参数，全是可打印的 ASCII。
pub fn is_mime_type(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MIME_TYPE_MAX_BYTES
        && value.contains('/')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_graphic() || byte == b' ')
}

/// 云盘里的一个文件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveFile {
    pub id: String,
    pub name: String,
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    pub sha256: String,
    pub created_at: Timestamp,
}

/// `GET /v1/drive/files` 的结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveListing {
    pub items: Vec<DriveFile>,
    /// 已经占用的字节数。
    pub used_bytes: u64,
    /// 这个账户的上限。
    pub limit_bytes: u64,
}

/// 改名字。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameInput {
    pub name: String,
}

impl RenameInput {
    pub fn clean_name(&self) -> Option<&str> {
        clean_file_name(&self.name)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn file_names_must_be_single_safe_segments() {
        assert_eq!(clean_file_name("  周报.docx  "), Some("周报.docx"));
        assert_eq!(clean_file_name("a.b.c"), Some("a.b.c"));
        let longest = "名".repeat(255);
        assert_eq!(clean_file_name(&longest), Some(longest.as_str()));
        for rejected in ["", "   ", ".", "..", "a/b", "a\\b", "a\nb", "a\0b"] {
            assert_eq!(clean_file_name(rejected), None, "{rejected:?}");
        }
        assert!(clean_file_name(&"名".repeat(256)).is_none());
    }

    #[test]
    fn mime_types_are_short_printable_tokens_with_a_slash() {
        assert!(is_mime_type("application/octet-stream"));
        assert!(is_mime_type("text/plain; charset=utf-8"));
        assert!(!is_mime_type("text"));
        assert!(!is_mime_type(""));
        assert!(!is_mime_type("text/plain\n"));
        assert!(!is_mime_type(&format!("{}/x", "a".repeat(127))));
    }

    #[test]
    fn drive_files_round_trip_without_empty_mime_types() {
        let listing: DriveListing = serde_json::from_value(json!({
            "items": [
                {
                    "id": "f1",
                    "name": "周报.docx",
                    "size": 12,
                    "mimeType": "application/octet-stream",
                    "sha256": "a".repeat(64),
                    "createdAt": "2026-11-01T00:00:00.000Z"
                },
                {
                    "id": "f2",
                    "name": "notes",
                    "size": 0,
                    "sha256": "b".repeat(64),
                    "createdAt": "2026-11-01T00:00:00.000Z"
                }
            ],
            "usedBytes": 12,
            "limitBytes": 100
        }))
        .unwrap();
        assert_eq!(listing.items.len(), 2);
        assert!(listing.items[0].mime_type.is_some());
        assert!(listing.items[1].mime_type.is_none());
        // 没有媒体类型的文件不会写出一个 null 字段。
        let encoded = serde_json::to_value(&listing).unwrap();
        assert!(encoded["items"][1].get("mimeType").is_none());
        assert_eq!(encoded["items"][0]["mimeType"], "application/octet-stream");
        assert_eq!(encoded["usedBytes"], 12);
    }

    #[test]
    fn rename_cleans_the_name_or_rejects_it() {
        assert_eq!(
            RenameInput {
                name: " 新名字 ".into()
            }
            .clean_name(),
            Some("新名字")
        );
        assert_eq!(RenameInput { name: "a/b".into() }.clean_name(), None);
        assert_eq!(
            serde_json::to_value(RenameInput { name: "x".into() }).unwrap(),
            json!({ "name": "x" })
        );
    }
}
