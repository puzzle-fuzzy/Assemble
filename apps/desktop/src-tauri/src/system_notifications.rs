//! 本机系统通知适配。文件只在 Rust 内访问，账户记录不进入其他账户的快照。

use std::path::PathBuf;

use assemble_protocol::downloads::DownloadPhase;
use assemble_protocol::primitives::Timestamp;
use assemble_protocol::sends::Direction;
use assemble_protocol::system_notifications::{
    SystemNotificationContent as Content, SystemNotificationLevel as Level, SystemNotificationRead,
    SystemNotifications,
};
use assemble_store::json::{read_json, write_private_json};
use assemble_store::notifications::{NotificationLog, Signal};

use crate::registry::{TransferFailure, TransferPhase};
use crate::state::{SessionView, Snapshot};

#[derive(Default)]
pub struct Inbox {
    log: NotificationLog,
    file: Option<PathBuf>,
    storage: StorageState,
    expired: bool,
    downloads_ready: bool,
}

#[derive(Default, PartialEq, Eq)]
enum StorageState {
    #[default]
    Ready,
    WriteFailed,
    Unreadable,
}

impl Inbox {
    pub fn downloads_ready(&mut self) {
        self.downloads_ready = true;
    }
    pub fn attach(&mut self, file: PathBuf) {
        self.file = Some(file);
        self.storage = StorageState::Unreadable;
        self.refresh();
    }

    pub fn expired(&mut self) {
        self.expired = true;
    }

    pub fn refresh(&mut self) {
        if self.storage == StorageState::Unreadable {
            if let Some(Ok(log)) = self.file.as_deref().map(read_json::<NotificationLog>) {
                self.log = log.unwrap_or_default();
                self.storage = StorageState::Ready;
                self.expired = self.log.view(&["local"]).items.iter().any(|item| {
                    item.resolved_at.is_none() && matches!(item.content, Content::SessionExpired)
                });
            } else {
                tracing::warn!("system notifications could not be loaded");
            }
        } else {
            let _ = self.persist();
        }
    }

    fn persist(&mut self) -> Result<(), ()> {
        if self.storage == StorageState::Unreadable {
            return Err(());
        }
        if self
            .file
            .as_deref()
            .is_some_and(|file| write_private_json(file, &self.log).is_err())
        {
            self.storage = StorageState::WriteFailed;
            tracing::warn!("system notifications could not be saved");
            return Err(());
        }
        self.storage = StorageState::Ready;
        Ok(())
    }

    pub fn observe(&mut self, snapshot: &Snapshot, origin: &str) -> SystemNotifications {
        let previous = self.log.clone();
        let now = Timestamp::now();
        let mut local = Vec::new();
        if matches!(snapshot.session, SessionView::SignedIn { .. }) {
            self.expired = false;
        }
        if self.expired {
            local.push(Signal {
                key: "session-expired".into(),
                content: Content::SessionExpired,
                level: Level::Warning,
            });
        }
        let old_local = self.log.view(&["local"]);
        if !self.downloads_ready {
            for item in old_local.items.iter().filter(|item| {
                item.resolved_at.is_none() && matches!(item.content, Content::DownloadFailed { .. })
            }) {
                if let Content::DownloadFailed { task_id, .. } = &item.content {
                    local.push(Signal {
                        key: format!("download:{task_id}"),
                        content: item.content.clone(),
                        level: item.level,
                    });
                }
            }
        }
        for task in &snapshot.downloads.tasks {
            if matches!(
                task.phase,
                DownloadPhase::Failed | DownloadPhase::Interrupted
            ) {
                local.push(Signal {
                    key: format!("download:{}", task.id),
                    content: Content::DownloadFailed {
                        task_id: task.id.clone(),
                        name: task.file_name.clone().unwrap_or_else(|| task.title.clone()),
                        local_disk_full: task.error_code.as_deref() == Some("DOWNLOAD_NO_SPACE"),
                    },
                    level: Level::Error,
                });
            } else if task.phase.active() && let Some(item)=old_local.items.iter().find(|item|item.resolved_at.is_none() && matches!(&item.content,Content::DownloadFailed{task_id,..} if task_id==&task.id)) {
                    local.push(Signal{key:format!("download:{}",task.id),content:item.content.clone(),level:item.level});
            }
        }
        self.log.observe("local", &local, now);
        if let Some(scope) = account_scope(snapshot, origin) {
            let old = self.log.view(&[&scope]);
            let mut signals = Vec::new();
            for transfer in &snapshot.transfers {
                if transfer.phase == TransferPhase::Failed
                    && transfer.failure != Some(TransferFailure::Cancelled)
                {
                    signals.push(Signal {
                        key: format!("transfer:{}", transfer.id),
                        content: Content::TransferFailed {
                            transfer_id: transfer.id.clone(),
                            name: transfer.name.clone(),
                            local_disk_full: transfer.direction == Direction::In
                                && transfer.failure == Some(TransferFailure::DiskFull),
                        },
                        level: Level::Error,
                    });
                } else if matches!(
                    transfer.phase,
                    TransferPhase::Queued | TransferPhase::InProgress | TransferPhase::Waiting
                ) && let Some(item)=old.items.iter().find(|item|item.resolved_at.is_none() && matches!(&item.content,Content::TransferFailed{transfer_id,..} if transfer_id==&transfer.id)) {signals.push(Signal{key:format!("transfer:{}",transfer.id),content:item.content.clone(),level:item.level});
                }
            }
            // 登出和刚恢复会话时会暂时清空名单，不能把它误报为文件问题已经解决。
            if !snapshot.transfers.is_empty() {
                self.log.observe(&scope, &signals, now);
            }
        }
        if previous != self.log {
            let _ = self.persist();
        }
        self.view(snapshot, origin)
    }

    pub fn view(&self, snapshot: &Snapshot, origin: &str) -> SystemNotifications {
        let scope = account_scope(snapshot, origin);
        let mut scopes = vec!["local"];
        if let Some(scope) = scope.as_deref() {
            scopes.push(scope);
        }
        let mut view = self.log.view(&scopes);
        view.available = self.storage == StorageState::Ready;
        view
    }

    pub fn read(
        &mut self,
        snapshot: &Snapshot,
        origin: &str,
        items: &[SystemNotificationRead],
    ) -> Result<(), ()> {
        if items.is_empty() || items.len() > 100 {
            return Err(());
        }
        let scope = account_scope(snapshot, origin);
        let mut scopes = vec!["local"];
        if let Some(scope) = scope.as_deref() {
            scopes.push(scope);
        }
        let previous = self.log.clone();
        self.log.read(&scopes, items, Timestamp::now());
        if self.persist().is_err() {
            self.log = previous;
            return Err(());
        }
        Ok(())
    }
}

fn account_scope(snapshot: &Snapshot, origin: &str) -> Option<String> {
    match &snapshot.session {
        SessionView::SignedIn { account, .. } => Some(format!("{origin}:{}", account.id)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    const ORIGIN: &str = "https://example.test";
    fn snapshot() -> Snapshot {
        let samples: Vec<Snapshot> =
            serde_json::from_str(include_str!("../../contract/snapshots.json")).unwrap();
        let mut view = samples
            .into_iter()
            .find(|view| matches!(view.session, SessionView::SignedIn { .. }))
            .unwrap();
        view.transfers.clear();
        view.downloads.tasks.clear();
        view
    }
    fn failed_transfer() -> crate::registry::TransferView {
        serde_json::from_value(json!({"id":"transfer-one","direction":"in","peer":"工作电脑","name":"中文 <文件>.pdf","size":100,"phase":"failed","progress":0,"savedAs":null,"failure":"diskFull","retryable":false,"target":"device"})).unwrap()
    }
    #[test]
    fn local_receiving_failures_persist_read_state_and_never_cross_accounts_or_origins() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("notifications.json");
        let mut inbox = Inbox::default();
        inbox.attach(file.clone());
        let mut view = snapshot();
        view.transfers.push(failed_transfer());
        let first = inbox.observe(&view, ORIGIN).items.remove(0);
        assert!(
            matches!(&first.content,Content::TransferFailed{local_disk_full:true,name,..} if name=="中文 <文件>.pdf")
        );
        inbox
            .read(
                &view,
                ORIGIN,
                &[SystemNotificationRead {
                    id: first.id.clone(),
                    revision: first.revision,
                }],
            )
            .unwrap();
        assert!(inbox.observe(&view, ORIGIN).items[0].read_at.is_some());
        let mut reopened = Inbox::default();
        reopened.attach(file);
        assert!(reopened.observe(&view, ORIGIN).items[0].read_at.is_some());
        assert_eq!(reopened.view(&view, "https://another.test").items, vec![]);
        if let SessionView::SignedIn { account, .. } = &mut view.session {
            account.id = "another-account".into();
        }
        assert_eq!(reopened.view(&view, ORIGIN).items, vec![]);
        view.session = SessionView::SignedOut;
        assert_eq!(reopened.view(&view, ORIGIN).items, vec![]);
    }

    #[test]
    fn downloads_are_local_and_restart_does_not_report_recovery_before_tasks_are_loaded() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("notifications.json");
        let mut inbox = Inbox::default();
        inbox.attach(file.clone());
        inbox.downloads_ready();
        let mut view = snapshot();
        view.session = SessionView::SignedOut;
        let task=serde_json::from_value(json!({"id":"download-one","kind":"hls","title":"视频","fileName":"视频 <样例>.mp4","sourceHost":"example.test","phase":"failed","receivedBytes":0,"totalBytes":null,"completedSegments":0,"totalSegments":null,"bytesPerSecond":0,"mediaSeconds":null,"durationSeconds":null,"errorCode":"DOWNLOAD_NO_SPACE","retryable":true,"createdAt":"2026-11-26T01:00:00.000Z"})).unwrap();
        view.downloads.tasks.push(task);
        let first = inbox.observe(&view, ORIGIN).items.remove(0);
        inbox
            .read(
                &view,
                ORIGIN,
                &[SystemNotificationRead {
                    id: first.id.clone(),
                    revision: first.revision,
                }],
            )
            .unwrap();
        let mut reopened = Inbox::default();
        reopened.attach(file);
        let mut loading = view.clone();
        loading.downloads.tasks.clear();
        let before_load = reopened.observe(&loading, ORIGIN);
        assert!(before_load.items[0].resolved_at.is_none());
        assert!(before_load.items[0].read_at.is_some());
        reopened.downloads_ready();
        assert!(reopened.observe(&view, ORIGIN).items[0].read_at.is_some());
        view.downloads.tasks[0].phase = DownloadPhase::Completed;
        let completed = reopened.observe(&view, ORIGIN);
        assert!(completed.items[0].resolved_at.is_some());
        assert!(completed.items[0].read_at.is_none());
        let raw = serde_json::to_string(&completed).unwrap();
        assert!(!raw.contains("DOWNLOAD_NO_SPACE"));
        assert!(!raw.contains("example.test"));
    }

    #[test]
    fn persistence_failure_does_not_acknowledge_read_and_corruption_is_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let mut view = snapshot();
        view.transfers.push(failed_transfer());
        let mut inbox = Inbox::default();
        inbox.attach(dir.path().to_path_buf());
        let item = inbox.observe(&view, ORIGIN).items.remove(0);
        assert!(
            inbox
                .read(
                    &view,
                    ORIGIN,
                    &[SystemNotificationRead {
                        id: item.id,
                        revision: item.revision
                    }]
                )
                .is_err()
        );
        let result = inbox.view(&view, ORIGIN);
        assert!(!result.available);
        assert!(result.items[0].read_at.is_none());
        let corrupt = dir.path().join("corrupt.json");
        std::fs::write(&corrupt, b"not json").unwrap();
        inbox.attach(corrupt.clone());
        inbox.observe(&view, ORIGIN);
        inbox.refresh();
        assert_eq!(std::fs::read(corrupt).unwrap(), b"not json");
    }
}
