//! 日程在接口里的数据形状。
//!
//! 推算只关心其中的时间与重复规则；标题、地点、备注是为了让这份形状
//! 能原样在线上传输。字段名沿用 camelCase 的时间字符串，读的时候按各自的
//! 时区偏移换算成时间戳。

use serde::{Deserialize, Serialize};

/// 一处显式地点：`label` 是可点击地名，`address` 是导航目的地。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Location {
    pub label: String,
    pub address: String,
}

/// 一条日程的重复规则。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Repeat {
    None,
    Daily,
    Weekly,
    Monthly,
}

/// 一条日程的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScheduleStatus {
    Upcoming,
    Done,
    Cancelled,
}

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
