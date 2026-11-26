//! 账户私有的纯文本备忘录；不将文本作为 HTML 或文件路径解释。
use serde::{Deserialize, Serialize};

use crate::primitives::Timestamp;

pub const TITLE_MAX_UNITS: usize = 200;
pub const TEXT_MAX_UNITS: usize = 100_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoContent {
    pub title: String,
    pub text: String,
}

impl MemoContent {
    pub fn cleaned(&self) -> Option<Self> {
        let title = self.title.trim();
        if title.is_empty()
            || title.encode_utf16().count() > TITLE_MAX_UNITS
            || title.chars().any(char::is_control)
            || self.text.trim().is_empty()
            || self.text.encode_utf16().count() > TEXT_MAX_UNITS
            || self
                .text
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
        {
            return None;
        }
        // 正文不 trim 或改换行：复制必须拿回用户保存的原文。
        Some(Self {
            title: title.to_owned(),
            text: self.text.clone(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoCreate {
    pub request_id: String,
    pub content: MemoContent,
}

impl MemoCreate {
    pub fn valid(&self) -> bool {
        !self.request_id.is_empty()
            && self.request_id.len() <= 128
            && self
                .request_id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
            && self.content.cleaned().is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoUpdate {
    pub version: i64,
    pub content: MemoContent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Memo {
    pub id: String,
    pub version: i64,
    #[serde(flatten)]
    pub content: MemoContent,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoSummary {
    pub id: String,
    pub version: i64,
    pub title: String,
    pub preview: String,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MemoQuery {
    pub q: String,
    pub page: u32,
    pub page_size: u32,
}

impl Default for MemoQuery {
    fn default() -> Self {
        Self {
            q: String::new(),
            page: 1,
            page_size: 10,
        }
    }
}

impl MemoQuery {
    pub fn valid(&self) -> bool {
        self.page > 0
            && (1..=100).contains(&self.page_size)
            && self.q.encode_utf16().count() <= 200
            && !self.q.chars().any(char::is_control)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoList {
    pub items: Vec<MemoSummary>,
    pub total: u64,
    pub page: u32,
    pub page_size: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn desktop_response_samples_round_trip() {
        let samples: serde_json::Value =
            serde_json::from_str(include_str!("../../../apps/desktop/contract/memos.json"))
                .unwrap();
        let memo: Memo = serde_json::from_value(samples["memo"].clone()).unwrap();
        let listing: MemoList = serde_json::from_value(samples["listing"].clone()).unwrap();
        assert_eq!(serde_json::to_value(memo).unwrap(), samples["memo"]);
        assert_eq!(serde_json::to_value(listing).unwrap(), samples["listing"]);
    }
    #[test]
    fn only_title_is_trimmed_and_text_limits_use_utf16() {
        let input = MemoContent {
            title: " 标题 ".into(),
            text: " \t原文\r\n<em>text</em> \n".into(),
        };
        assert_eq!(input.cleaned().unwrap().text, input.text);
        assert_eq!(input.cleaned().unwrap().title, "标题");
        assert!(
            MemoContent {
                text: "\0".into(),
                ..input.clone()
            }
            .cleaned()
            .is_none()
        );
        assert!(
            MemoContent {
                text: " \r\n\t".into(),
                ..input.clone()
            }
            .cleaned()
            .is_none()
        );
        assert!(
            MemoContent {
                text: "🙂".repeat(50_001),
                ..input
            }
            .cleaned()
            .is_none()
        );
    }
}
