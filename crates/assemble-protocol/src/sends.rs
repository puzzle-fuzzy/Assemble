//! 发送到设备：把一个文件经服务端送给同一个账户的另一台在线设备。
//!
//! 一次发送的状态只往前走：
//!
//! ```text
//! offered ──▶ delivered   （接收方收下并确认）
//!        ├──▶ declined    （接收方拒绝）
//!        ├──▶ cancelled   （发送方撤回）
//!        ├──▶ failed      （接收方写盘失败）
//!        └──▶ expired     （过了时限还没送达）
//! ```
//!
//! 到达终态时，服务端释放文件内容。除了 `offered`，其余状态下都不能再下载。

use serde::{Deserialize, Serialize};

use crate::primitives::Timestamp;

/// 一次发送处在哪个阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SendState {
    /// 已经在服务端，等接收方收下。
    Offered,
    Delivered,
    Declined,
    Cancelled,
    Failed,
    Expired,
}

impl SendState {
    /// 终态不会再变，文件内容已经释放。
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Offered)
    }
}

/// 相对于"谁在看"的方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// 别的设备发给这台设备。
    In,
    /// 这台设备发给别的设备。
    Out,
}

/// 接收方报告的失败原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailReason {
    /// 接收目录所在的磁盘写不下了。
    DiskFull,
    /// 写文件时出错：没有权限、目录被删等。
    WriteFailed,
    /// 用户在接收途中取消了。
    Cancelled,
    /// 收到的内容与声明的长度或摘要对不上。这不是接收方磁盘的问题，
    /// 所以单独报出来：发送方看到它就知道该重发，而不是去查对方的电脑。
    VerificationFailed,
}

/// 一次发送在接口里的样子。`direction` 按发起请求的那台设备计算。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Send {
    pub id: String,
    pub direction: Direction,
    /// 对端设备：收到的是发送方，发出的是接收方。名字是当时的快照。
    pub peer_device_id: String,
    pub peer_device_name: String,
    pub name: String,
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    pub sha256: String,
    pub state: SendState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<FailReason>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl Send {
    /// 内容还能不能下载：只有 `offered` 时文件还在服务端。
    pub fn content_available(&self) -> bool {
        self.state == SendState::Offered
    }
}

/// 列出哪些发送。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendQuery {
    pub direction: Direction,
    /// 只看某个状态的发送；`None` 表示全部。
    pub state: Option<SendState>,
    /// 最多返回多少条，1 到 200。
    pub limit: u32,
}

/// 列表一页的最大条数。
pub const SEND_PAGE_MAX: u32 = 200;

impl SendQuery {
    pub fn is_valid(&self) -> bool {
        (1..=SEND_PAGE_MAX).contains(&self.limit)
    }
}

/// `GET /v1/sends` 的结果。新的在前。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendListing {
    pub items: Vec<Send>,
}

/// 接收方报告失败时提交的内容。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FailInput {
    pub reason: FailReason,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn send(state: SendState) -> Send {
        Send {
            id: "s1".into(),
            direction: Direction::In,
            peer_device_id: "d2".into(),
            peer_device_name: "工作电脑".into(),
            name: "合同.pdf".into(),
            size: 1024,
            mime_type: Some("application/pdf".into()),
            sha256: "a".repeat(64),
            state,
            failure: None,
            created_at: Timestamp::from_millis(1_794_400_000_000).unwrap(),
            updated_at: Timestamp::from_millis(1_794_400_000_000).unwrap(),
        }
    }

    #[test]
    fn only_the_offered_state_still_has_content() {
        assert!(send(SendState::Offered).content_available());
        for terminal in [
            SendState::Delivered,
            SendState::Declined,
            SendState::Cancelled,
            SendState::Failed,
            SendState::Expired,
        ] {
            assert!(terminal.is_terminal());
            assert!(!send(terminal).content_available());
        }
        assert!(!SendState::Offered.is_terminal());
    }

    #[test]
    fn sends_round_trip_with_snake_case_enums_and_no_empty_failure() {
        let value = json!({
            "id": "s1",
            "direction": "in",
            "peerDeviceId": "d2",
            "peerDeviceName": "工作电脑",
            "name": "合同.pdf",
            "size": 1024,
            "mimeType": "application/pdf",
            "sha256": "a".repeat(64),
            "state": "offered",
            "createdAt": "2026-11-11T12:26:40.000Z",
            "updatedAt": "2026-11-11T12:26:40.000Z"
        });
        let parsed: Send = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(parsed, send(SendState::Offered));
        let encoded = serde_json::to_value(&parsed).unwrap();
        assert_eq!(encoded, value);

        let failed = json!({
            "id": "s1",
            "direction": "out",
            "peerDeviceId": "d2",
            "peerDeviceName": "工作电脑",
            "name": "合同.pdf",
            "size": 1024,
            "sha256": "a".repeat(64),
            "state": "failed",
            "failure": "disk_full",
            "createdAt": "2026-11-11T12:26:40.000Z",
            "updatedAt": "2026-11-11T12:26:40.000Z"
        });
        let parsed: Send = serde_json::from_value(failed.clone()).unwrap();
        assert_eq!(parsed.failure, Some(FailReason::DiskFull));
        assert!(parsed.mime_type.is_none());
        assert_eq!(serde_json::to_value(&parsed).unwrap(), failed);
    }

    #[test]
    fn send_queries_bound_their_page_size() {
        assert!(
            SendQuery {
                direction: Direction::Out,
                state: None,
                limit: 1
            }
            .is_valid()
        );
        assert!(
            SendQuery {
                direction: Direction::Out,
                state: Some(SendState::Offered),
                limit: SEND_PAGE_MAX
            }
            .is_valid()
        );
        assert!(
            !SendQuery {
                direction: Direction::Out,
                state: None,
                limit: 0
            }
            .is_valid()
        );
        assert!(
            !SendQuery {
                direction: Direction::Out,
                state: None,
                limit: SEND_PAGE_MAX + 1
            }
            .is_valid()
        );
        assert_eq!(
            serde_json::to_value(SendQuery {
                direction: Direction::In,
                state: Some(SendState::Offered),
                limit: 50
            })
            .unwrap(),
            json!({ "direction": "in", "state": "offered", "limit": 50 })
        );
    }

    #[test]
    fn failure_reports_name_their_reason() {
        for (reason, spelling) in [
            (FailReason::DiskFull, "disk_full"),
            (FailReason::WriteFailed, "write_failed"),
            (FailReason::Cancelled, "cancelled"),
            (FailReason::VerificationFailed, "verification_failed"),
        ] {
            let report = serde_json::to_value(FailInput { reason }).unwrap();
            assert_eq!(report, json!({ "reason": spelling }));
            assert_eq!(
                serde_json::from_value::<FailInput>(report).unwrap(),
                FailInput { reason }
            );
        }
    }
}
