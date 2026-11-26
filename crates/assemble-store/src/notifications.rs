//! 有界通知账本。观察问题、恢复与已读都按版本处理，不把频繁采样变成新消息。

use assemble_protocol::primitives::Timestamp;
use assemble_protocol::system_notifications::{
    SystemNotification, SystemNotificationContent, SystemNotificationLevel, SystemNotificationRead,
    SystemNotifications,
};
use serde::{Deserialize, Serialize};

pub const HISTORY_LIMIT: usize = 100;
const REOPEN_MILLIS: i64 = 5 * 60 * 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signal {
    pub key: String,
    pub content: SystemNotificationContent,
    pub level: SystemNotificationLevel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    scope: String,
    key: String,
    notification: SystemNotification,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationLog {
    next_id: u64,
    entries: Vec<Entry>,
}

impl NotificationLog {
    /// 同一范围内用本次完整观察对账；范围外的账户不受影响。
    pub fn observe(&mut self, scope: &str, signals: &[Signal], now: Timestamp) {
        for entry in self.entries.iter_mut().filter(|entry| entry.scope == scope) {
            if entry.notification.resolved_at.is_none()
                && !signals.iter().any(|signal| signal.key == entry.key)
            {
                entry.notification.resolved_at = Some(now);
                changed(&mut entry.notification, now);
            }
        }
        for signal in signals {
            let recent = self.entries.iter_mut().rev().find(|entry| {
                entry.scope == scope
                    && entry.key == signal.key
                    && entry.notification.resolved_at.is_none_or(|at| {
                        now.as_millis().saturating_sub(at.as_millis()) <= REOPEN_MILLIS
                    })
            });
            if let Some(entry) = recent {
                let notification = &mut entry.notification;
                if notification.resolved_at.take().is_some() || notification.level != signal.level {
                    changed(notification, now);
                }
                if notification.content != signal.content {
                    notification.content = signal.content.clone();
                    notification.updated_at = now;
                }
                notification.level = signal.level;
            } else {
                self.next_id = self.next_id.saturating_add(1);
                self.entries.push(Entry {
                    scope: scope.to_owned(),
                    key: signal.key.clone(),
                    notification: SystemNotification {
                        id: format!("system-{}-{}", now.as_millis(), self.next_id),
                        revision: 1,
                        content: signal.content.clone(),
                        level: signal.level,
                        created_at: now,
                        updated_at: now,
                        resolved_at: None,
                        read_at: None,
                    },
                });
            }
        }
        // 优先保留尚未恢复的问题，历史按最新变化时间保留；多账户总量也有上限。
        self.entries.sort_by(|a, b| {
            b.notification
                .resolved_at
                .is_none()
                .cmp(&a.notification.resolved_at.is_none())
                .then_with(|| b.notification.updated_at.cmp(&a.notification.updated_at))
                .then_with(|| b.notification.id.cmp(&a.notification.id))
        });
        let mut seen = std::collections::HashMap::new();
        self.entries.retain(|entry| {
            let count = seen.entry(entry.scope.clone()).or_insert(0);
            *count += 1;
            *count <= HISTORY_LIMIT
        });
        self.entries.truncate(1000);
    }

    pub fn view(&self, scopes: &[&str]) -> SystemNotifications {
        let mut items: Vec<_> = self
            .entries
            .iter()
            .filter(|entry| scopes.contains(&entry.scope.as_str()))
            .map(|entry| entry.notification.clone())
            .collect();
        items.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| b.id.cmp(&a.id))
        });
        items.truncate(HISTORY_LIMIT);
        SystemNotifications {
            items,
            available: true,
        }
    }

    /// 只承认用户看到的那个版本，避免迟到请求把刚恢复或升级的通知直接标为已读。
    pub fn read(&mut self, scopes: &[&str], items: &[SystemNotificationRead], now: Timestamp) {
        for entry in &mut self.entries {
            if scopes.contains(&entry.scope.as_str())
                && items.iter().any(|item| {
                    item.id == entry.notification.id && item.revision == entry.notification.revision
                })
            {
                entry.notification.read_at.get_or_insert(now);
            }
        }
    }
}

fn changed(notification: &mut SystemNotification, now: Timestamp) {
    notification.revision = notification.revision.saturating_add(1);
    notification.updated_at = now;
    notification.read_at = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    fn at(value: i64) -> Timestamp {
        Timestamp::from_millis(value).unwrap()
    }
    fn signal(used: u64) -> Signal {
        Signal {
            key: "budget".into(),
            content: SystemNotificationContent::StorageBudget {
                used_bytes: used,
                budget_bytes: 100,
                warning_percent: 80,
            },
            level: SystemNotificationLevel::Warning,
        }
    }

    #[test]
    fn measurements_do_not_repeat_or_unread_but_recovery_and_severity_do() {
        let mut log = NotificationLog::default();
        log.observe("admin", &[signal(80)], at(1));
        let first = log.view(&["admin"]).items.remove(0);
        let read = SystemNotificationRead {
            id: first.id.clone(),
            revision: first.revision,
        };
        log.read(&["admin"], std::slice::from_ref(&read), at(2));
        log.observe("admin", &[signal(90)], at(3));
        let sample = log.view(&["admin"]);
        assert_eq!(sample.items.len(), 1);
        assert_eq!(sample.items[0].read_at, Some(at(2)));
        log.observe("admin", &[], at(4));
        let recovered = log.view(&["admin"]).items.remove(0);
        assert_eq!(recovered.resolved_at, Some(at(4)));
        assert!(recovered.read_at.is_none());
        log.read(&["admin"], &[read], at(5));
        assert!(log.view(&["admin"]).items[0].read_at.is_none());
        log.observe("admin", &[signal(85)], at(6));
        assert_eq!(log.view(&["admin"]).items[0].id, first.id);
        log.observe("admin", &[], at(7));
        log.observe("admin", &[signal(82)], at(400_000));
        assert_eq!(log.view(&["admin"]).items.len(), 2);
    }

    #[test]
    fn scope_and_persistence_keep_other_accounts_private_and_history_bounded() {
        let mut log = NotificationLog::default();
        log.observe("account-a", &[signal(80)], at(1));
        log.observe("account-b", &[], at(2));
        assert!(log.view(&["account-b"]).items.is_empty());
        assert!(log.view(&["account-a"]).items[0].resolved_at.is_none());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notifications.json");
        crate::json::write_private_json(&path, &log).unwrap();
        let loaded: NotificationLog = crate::json::read_json(&path).unwrap().unwrap();
        assert_eq!(loaded, log);
        for index in 0..150 {
            let mut next = signal(80);
            next.key = index.to_string();
            log.observe("local", &[next], at(index + 3));
        }
        assert_eq!(log.view(&["local"]).items.len(), HISTORY_LIMIT);
    }
}
