//! 日程在接口里的数据形状。
//!
//! [`Repeat`]、[`ScheduleStatus`] 与 [`Location`] 只在 `assemble-protocol` 里定义一份，
//! 这里原样转出：服务端推算与接口说话用的是同一套拼法。
//!
//! [`ScheduleData`] 沿用 jarvis 的那一份（时间是带偏移的字符串），留着对照那两万条旧用例；
//! Assemble 自己的接口用的是 `assemble_protocol::schedule` 里带版本号的那几种类型。

use serde::{Deserialize, Serialize};

pub use assemble_protocol::schedule::{Location, Repeat, ScheduleStatus};

/// 跨时段的日程，可按天、周、月重复到 `repeatUntil`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleData {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<Location>,
    pub start_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_at: Option<String>,
    pub repeat: Repeat,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat_until: Option<String>,
    pub status: ScheduleStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}
