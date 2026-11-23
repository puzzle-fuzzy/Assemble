//! 命令回执账本：本机副作用先落盘，再向云端确认。
//!
//! 一次命令的"已完成"确认可能因为断网、租约到期或进程退出而丢失，云端随后
//! 会把同一条命令重新发下来。账本保证两件事：
//!
//! - 效果已经做完的命令只重发回执，绝不重复执行——下载不会被写两遍；
//! - 只有"云端已看到终态"或"云端明确回答这条命令不存在"才能注销一条回执，
//!   普通的 404 什么都证明不了。

use std::collections::{BTreeMap, BTreeSet};

use assemble_protocol::commands::{CommandStatus, ReportStatus};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 一条命令在本机已经产生的结果，等待云端确认。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandReceipt {
    pub status: ReportStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl CommandReceipt {
    pub fn completed(result: Value) -> Self {
        Self {
            status: ReportStatus::Completed,
            progress: Some(100.0),
            result: Some(result),
            error: None,
        }
    }

    /// 失败回执只带一句可以对外说明的话。
    pub fn failed(message: impl Into<String>) -> Self {
        Self {
            status: ReportStatus::Failed,
            progress: None,
            result: None,
            error: Some(message.into()),
        }
    }
}

/// 领取一条命令时该怎么处理。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Claim {
    /// 已经在执行，不重复启动。
    Busy,
    /// 本机早已完成，并且云端已经确认过。
    Settled,
    /// 效果已经做完，只差云端确认：重发回执，不重复执行。
    Acknowledge,
    /// 还没有执行过。
    Execute,
}

/// 对账时从云端看到的情况。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Observation {
    /// 云端上的这条命令。
    Command {
        id: String,
        device_id: String,
        status: CommandStatus,
    },
    /// 云端明确回答：这条命令的历史已经不存在。
    MissingConfirmed,
    /// 没能看清：网络错误，或一个没有带明确错误码的 404。
    Unproven,
    /// 会话或连接层面的问题，必须整体停下。
    Fatal,
}

/// 对账的结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reconcile {
    /// 注销回执；`adopt` 是云端已经确认的终态。
    Retire { adopt: Option<CommandStatus> },
    /// 证据不足，回执原样保留。
    Keep,
    /// 停止整轮对账。
    Stop,
}

/// 全部待确认的回执，按命令编号索引。
///
/// 落盘时就是这张表本身，与 Electron 版状态文件里的 `commandReceipts` 相同。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ReceiptLedger {
    receipts: BTreeMap<String, CommandReceipt>,
    /// 本进程里正在执行的命令；不落盘，重启后自然为空。
    #[serde(skip)]
    active: BTreeSet<String>,
}

impl ReceiptLedger {
    pub fn get(&self, command_id: &str) -> Option<&CommandReceipt> {
        self.receipts.get(command_id)
    }

    pub fn is_active(&self, command_id: &str) -> bool {
        self.active.contains(command_id)
    }

    pub fn is_pending(&self, command_id: &str) -> bool {
        self.receipts.contains_key(command_id)
    }

    pub fn pending(&self) -> impl Iterator<Item = (&str, &CommandReceipt)> {
        self.receipts
            .iter()
            .map(|(id, receipt)| (id.as_str(), receipt))
    }

    /// 云端又发来了这条命令：决定是执行、重发回执还是跳过。
    ///
    /// `last_known` 是本机上次看到的云端状态；没有待确认回执时，
    /// 已经是终态的命令不再处理。
    pub fn claim(&self, command_id: &str, last_known: Option<CommandStatus>) -> Claim {
        if self.active.contains(command_id) {
            return Claim::Busy;
        }
        if self.receipts.contains_key(command_id) {
            return Claim::Acknowledge;
        }
        if last_known.is_some_and(CommandStatus::is_terminal) {
            return Claim::Settled;
        }
        Claim::Execute
    }

    /// 开始执行。之后的领取请求一律视为重复。
    pub fn begin(&mut self, command_id: impl Into<String>) {
        self.active.insert(command_id.into());
    }

    pub fn finish(&mut self, command_id: &str) {
        self.active.remove(command_id);
    }

    /// 记下一条已经在本机产生的结果。调用方必须先把账本落盘，再向云端回执。
    pub fn record(&mut self, command_id: impl Into<String>, receipt: CommandReceipt) {
        self.receipts.insert(command_id.into(), receipt);
    }

    /// 云端已经确认，或者明确回答命令不存在：这条回执可以放下了。
    pub fn retire(&mut self, command_id: &str) -> Option<CommandReceipt> {
        self.receipts.remove(command_id)
    }

    pub fn len(&self) -> usize {
        self.receipts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.receipts.is_empty()
    }

    /// 看看一条待确认的回执现在能不能注销。
    pub fn reconcile(
        &self,
        command_id: &str,
        own_device: &str,
        observation: &Observation,
    ) -> Reconcile {
        // 正在执行的不碰；没有回执的无事可做。
        if self.active.contains(command_id) || !self.receipts.contains_key(command_id) {
            return Reconcile::Keep;
        }
        match observation {
            Observation::Fatal => Reconcile::Stop,
            Observation::Unproven => Reconcile::Keep,
            Observation::MissingConfirmed => Reconcile::Retire { adopt: None },
            Observation::Command {
                id,
                device_id,
                status,
            } => {
                // 必须是本机的这条命令，而且已经到了终态。
                if id != command_id || device_id != own_device || !status.is_terminal() {
                    Reconcile::Keep
                } else {
                    Reconcile::Retire {
                        adopt: Some(*status),
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use assemble_protocol::commands::CommandStatus::{
        Cancelled, Completed, Failed, Queued, Running,
    };
    use serde_json::json;

    use super::*;

    const DEVICE: &str = "device-1";

    fn observed(id: &str, status: CommandStatus) -> Observation {
        Observation::Command {
            id: id.into(),
            device_id: DEVICE.into(),
            status,
        }
    }

    #[test]
    fn a_reissued_lease_after_a_lost_completion_only_reacknowledges() {
        let mut ledger = ReceiptLedger::default();
        assert_eq!(ledger.claim("cmd-1", None), Claim::Execute);

        // 效果做完、回执还没被云端确认时，云端又把同一条命令发了下来。
        ledger.record(
            "cmd-1",
            CommandReceipt::completed(json!({ "path": "/docs/a.zip" })),
        );
        assert_eq!(ledger.claim("cmd-1", Some(Running)), Claim::Acknowledge);
        assert_eq!(ledger.claim("cmd-1", Some(Queued)), Claim::Acknowledge);
        assert_eq!(ledger.claim("cmd-1", None), Claim::Acknowledge);
        // 重发的是当初那份结果，不会被改写。
        assert_eq!(
            ledger.get("cmd-1").unwrap().result.as_ref(),
            Some(&json!({ "path": "/docs/a.zip" }))
        );
    }

    #[test]
    fn a_settled_command_is_not_executed_again() {
        let ledger = ReceiptLedger::default();
        assert_eq!(ledger.claim("cmd-1", Some(Completed)), Claim::Settled);
        assert_eq!(ledger.claim("cmd-1", Some(Failed)), Claim::Settled);
        assert_eq!(ledger.claim("cmd-1", Some(Cancelled)), Claim::Settled);
        assert_eq!(ledger.claim("cmd-1", Some(Queued)), Claim::Execute);
        assert_eq!(ledger.claim("cmd-1", Some(Running)), Claim::Execute);
    }

    #[test]
    fn a_command_in_flight_is_never_started_twice() {
        let mut ledger = ReceiptLedger::default();
        ledger.begin("cmd-1");
        assert!(ledger.is_active("cmd-1"));
        assert_eq!(ledger.claim("cmd-1", None), Claim::Busy);
        ledger.record("cmd-1", CommandReceipt::completed(json!(null)));
        assert_eq!(ledger.claim("cmd-1", None), Claim::Busy);
        // 执行中的命令不参与对账。
        assert_eq!(
            ledger.reconcile("cmd-1", DEVICE, &observed("cmd-1", Completed)),
            Reconcile::Keep
        );
        ledger.finish("cmd-1");
        assert_eq!(ledger.claim("cmd-1", None), Claim::Acknowledge);
    }

    #[test]
    fn only_a_verified_terminal_state_retires_a_receipt() {
        let mut ledger = ReceiptLedger::default();
        ledger.record("cmd-1", CommandReceipt::failed("FILE_EXISTS"));

        let retire = Reconcile::Retire {
            adopt: Some(Completed),
        };
        assert_eq!(
            ledger.reconcile("cmd-1", DEVICE, &observed("cmd-1", Completed)),
            retire
        );
        assert_eq!(
            ledger.reconcile("cmd-1", DEVICE, &observed("cmd-1", Failed)),
            Reconcile::Retire {
                adopt: Some(Failed)
            }
        );
        // 云端仍在进行中：什么都证明不了，保留回执。
        assert_eq!(
            ledger.reconcile("cmd-1", DEVICE, &observed("cmd-1", Running)),
            Reconcile::Keep
        );
        assert_eq!(
            ledger.reconcile("cmd-1", DEVICE, &observed("cmd-1", Queued)),
            Reconcile::Keep
        );
        // 不是本机的命令，或根本是另一条命令：保留。
        assert_eq!(
            ledger.reconcile(
                "cmd-1",
                DEVICE,
                &Observation::Command {
                    id: "cmd-1".into(),
                    device_id: "device-2".into(),
                    status: Completed,
                }
            ),
            Reconcile::Keep
        );
        assert_eq!(
            ledger.reconcile("cmd-1", DEVICE, &observed("cmd-2", Completed)),
            Reconcile::Keep
        );
        // 网络错误与没有明确错误码的 404：保留。
        assert_eq!(
            ledger.reconcile("cmd-1", DEVICE, &Observation::Unproven),
            Reconcile::Keep
        );
        assert!(ledger.is_pending("cmd-1"));

        // 云端确认终态之后才注销，并带回它确认的状态。
        assert_eq!(
            ledger.reconcile("cmd-1", DEVICE, &observed("cmd-1", Completed)),
            retire
        );
        assert_eq!(
            ledger.retire("cmd-1"),
            Some(CommandReceipt::failed("FILE_EXISTS"))
        );
        assert!(ledger.is_empty());
    }

    #[test]
    fn a_verified_missing_command_retires_its_receipt_but_a_plain_404_does_not() {
        let mut ledger = ReceiptLedger::default();
        ledger.record("cmd-1", CommandReceipt::completed(json!(1)));
        assert_eq!(
            ledger.reconcile("cmd-1", DEVICE, &Observation::MissingConfirmed),
            Reconcile::Retire { adopt: None }
        );
        assert_eq!(
            ledger.reconcile("cmd-1", DEVICE, &Observation::Unproven),
            Reconcile::Keep
        );
        assert_eq!(
            ledger.reconcile("cmd-1", DEVICE, &Observation::Fatal),
            Reconcile::Stop
        );
        assert!(ledger.is_pending("cmd-1"));
    }

    #[test]
    fn an_authorisation_failure_stops_the_whole_reconcile_round() {
        let mut ledger = ReceiptLedger::default();
        ledger.record("cmd-1", CommandReceipt::completed(json!(1)));
        ledger.record("cmd-2", CommandReceipt::failed("ROOT_NOT_AUTHORIZED"));
        assert_eq!(
            ledger.reconcile("cmd-2", DEVICE, &Observation::Fatal),
            Reconcile::Stop
        );
        assert_eq!(ledger.len(), 2);
        let pending: Vec<_> = ledger.pending().map(|(id, _)| id).collect();
        assert_eq!(pending, vec!["cmd-1", "cmd-2"]);
    }

    #[test]
    fn receipts_survive_a_restart_without_repeating_the_effect() {
        let mut ledger = ReceiptLedger::default();
        ledger.record(
            "cmd-1",
            CommandReceipt::completed(json!({ "path": "/docs/a.zip", "size": 12 })),
        );
        ledger.begin("cmd-1");
        // 落盘：只写回执，不写"正在执行"。
        let saved = serde_json::to_string(&ledger).unwrap();
        assert_eq!(
            saved,
            r#"{"cmd-1":{"status":"completed","progress":100.0,"result":{"path":"/docs/a.zip","size":12}}}"#
        );

        // 重启后读回来：同一条命令再到时只重发回执。
        let restored: ReceiptLedger = serde_json::from_str(&saved).unwrap();
        assert!(!restored.is_active("cmd-1"));
        assert_eq!(restored.claim("cmd-1", Some(Running)), Claim::Acknowledge);
        assert_eq!(
            restored.get("cmd-1").unwrap().result.as_ref(),
            Some(&json!({ "path": "/docs/a.zip", "size": 12 }))
        );
    }

    #[test]
    fn receipts_match_the_original_wire_shape() {
        // 与 Electron 版 `commandReceipts` 的形状逐字一致，可以彼此读懂。
        let original = r#"{"cmd-9":{"status":"failed","error":"HASH_MISMATCH"}}"#;
        let ledger: ReceiptLedger = serde_json::from_str(original).unwrap();
        assert_eq!(
            ledger.get("cmd-9"),
            Some(&CommandReceipt::failed("HASH_MISMATCH"))
        );
        assert_eq!(serde_json::to_string(&ledger).unwrap(), original);
        assert_eq!(
            serde_json::to_string(&ReceiptLedger::default()).unwrap(),
            "{}"
        );
    }
}
