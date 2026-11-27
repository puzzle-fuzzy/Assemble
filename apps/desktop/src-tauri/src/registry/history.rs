//! 可落盘的历史只带已确认的元数据，不带草稿、源文件句柄和待执行任务。

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use super::{
    MAX_TRANSFERS, Origin, Registry, Timestamp, TransferFailure, TransferPhase, TransferRecord,
    TransferTarget, TransferView,
};
use assemble_protocol::sends::Send as SendRecord;

const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct History {
    version: u32,
    origin: String,
    account_id: String,
    next_local: u64,
    entries: Vec<Entry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    view: TransferView,
    send: Option<SendRecord>,
    saved_path: Option<std::path::PathBuf>,
    recorded_at: Timestamp,
}

impl History {
    pub fn is_supported(&self) -> bool {
        self.version == VERSION
    }

    /// 来源也属于归属：同一台电脑切到另一套服务不能读到前一套记录。
    pub fn belongs_to(&self, origin: &str, account_id: &str) -> bool {
        self.is_supported() && self.origin == origin && self.account_id == account_id
    }
}

impl Registry {
    pub fn history(&self, origin: &str, account_id: &str) -> History {
        let entries = self
            .transfers
            .iter()
            .take(MAX_TRANSFERS)
            .map(|record| {
                let mut view = record.view.clone();
                // 百分比变化不值得反复写盘，恢复时活动任务统一转成中断。
                view.progress = if view.phase == TransferPhase::Complete {
                    1.0
                } else {
                    0.0
                };
                view.retryable = false;
                let send = match &record.origin {
                    Origin::Incoming(send) => Some(send.clone()),
                    Origin::Outgoing(outgoing) => outgoing.send.clone(),
                    Origin::History { send } => send.clone(),
                    Origin::FromDrive(_) => None,
                };
                Entry {
                    view,
                    send,
                    saved_path: record.saved_path.clone(),
                    recorded_at: record.recorded_at,
                }
            })
            .collect();
        History {
            version: VERSION,
            origin: origin.to_owned(),
            account_id: account_id.to_owned(),
            next_local: self.next_local,
            entries,
        }
    }

    /// 历史不是工作队列：只有后续对账收到服务端的有效发送记录才会接着收。
    pub fn from_history(history: History, origin: &str, account_id: &str) -> Self {
        if !history.belongs_to(origin, account_id) {
            return Self::default();
        }
        let mut result = Self::default();
        let mut ids = HashSet::new();
        for entry in history.entries.into_iter().take(MAX_TRANSFERS) {
            let mut view = entry.view;
            if view.id.is_empty() || !ids.insert(view.id.clone()) {
                continue;
            }
            if let Some(number) = view
                .id
                .strip_prefix("out-")
                .and_then(|id| id.parse::<u64>().ok())
            {
                let Some(next) = number.checked_add(1) else {
                    continue;
                };
                result.next_local = result.next_local.max(next);
            }
            if view.phase.is_active() {
                view.phase = TransferPhase::Failed;
                view.failure = Some(TransferFailure::Interrupted);
                view.progress = 0.0;
            }
            view.retryable = false;
            let saved_path = entry
                .saved_path
                .filter(|path| view.phase == TransferPhase::Complete && path.is_absolute());
            let send = entry.send.filter(|send| {
                view.target == TransferTarget::Device
                    && send.direction == view.direction
                    && send.name == view.name
                    && send.size == view.size
            });
            result.transfers.push(TransferRecord {
                view,
                origin: Origin::History { send },
                attempts: 0,
                not_before: None,
                settled: true,
                saved_path,
                // 系统通知的句柄只在本次进程里有效，不能沿用旧进程的编号。
                handle: None,
                recorded_at: entry.recorded_at,
            });
        }
        result.next_local = result.next_local.max(history.next_local.min(u64::MAX - 1));
        result
    }
}
