//! 审计：谁在什么时候做了什么。只增不删，写不进去也不影响操作本身。

use sqlx::SqlitePool;

/// 记一笔。失败只进日志：审计写不进去不应该让用户的操作失败。
pub async fn record(
    pool: &SqlitePool,
    account_id: Option<&str>,
    device_id: Option<&str>,
    action: &str,
    target: Option<&str>,
) {
    let written = sqlx::query(
        "INSERT INTO audit_log (account_id, device_id, action, target, created_at) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(account_id)
    .bind(device_id)
    .bind(action)
    .bind(target)
    .bind(assemble_protocol::primitives::Timestamp::now().as_millis())
    .execute(pool)
    .await;
    if let Err(error) = written {
        tracing::error!(%error, action, "audit entry could not be written");
    }
}

/// 账户动作的名字。
pub mod actions {
    pub const ACCOUNT_CREATED: &str = "account.created";
    pub const ACCOUNT_REGISTERED: &str = "account.registered";
    pub const REGISTER_FAILED: &str = "web.register_failed";
    pub const PASSWORD_CHANGED: &str = "account.password_changed";
    pub const PASSWORD_CHANGE_FAILED: &str = "account.password_change_failed";
    pub const PASSWORD_RESET: &str = "account.password_reset";
    pub const ACCOUNT_SUSPENDED: &str = "account.suspended";
    pub const ACCOUNT_RESUMED: &str = "account.resumed";
    pub const WEB_LOGIN: &str = "web.login";
    pub const WEB_LOGIN_FAILED: &str = "web.login_failed";
    pub const WEB_LOGOUT: &str = "web.logout";
    pub const DESKTOP_AUTHORIZED: &str = "desktop.authorized";
    pub const DESKTOP_DENIED: &str = "desktop.denied";
    pub const DEVICE_REGISTERED: &str = "device.registered";
    pub const DEVICE_RENAMED: &str = "device.renamed";
    pub const DEVICE_REMOVED: &str = "device.removed";
    pub const DEVICE_LOGOUT: &str = "device.logout";
    pub const SEND_CREATED: &str = "send.created";
    pub const SEND_ACCEPTED: &str = "send.accepted";
    pub const SEND_DECLINED: &str = "send.declined";
    pub const SEND_CANCELLED: &str = "send.cancelled";
    pub const SEND_FAILED: &str = "send.failed";
    pub const SEND_EXPIRED: &str = "send.expired";
    pub const DRIVE_CREATED: &str = "drive.created";
    pub const DRIVE_RENAMED: &str = "drive.renamed";
    pub const DRIVE_DELETED: &str = "drive.deleted";
    pub const SCHEDULE_CREATED: &str = "schedule.created";
    pub const SCHEDULE_UPDATED: &str = "schedule.updated";
    pub const SCHEDULE_COMPLETED: &str = "schedule.completed";
    pub const SCHEDULE_CANCELLED: &str = "schedule.cancelled";
    pub const SCHEDULE_REOPENED: &str = "schedule.reopened";
    pub const SCHEDULE_DELETED: &str = "schedule.deleted";
    pub const REMINDER_CREATED: &str = "reminder.created";
    pub const REMINDER_UPDATED: &str = "reminder.updated";
    pub const REMINDER_DISMISSED: &str = "reminder.dismissed";
    pub const REMINDER_CANCELLED: &str = "reminder.cancelled";
    pub const REMINDER_DELETED: &str = "reminder.deleted";
    pub const NOTIFICATION_READ: &str = "notification.read";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn entries_record_who_did_what_and_when() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open(dir.path()).await.unwrap();

        record(&pool, Some("a1"), None, actions::WEB_LOGIN, None).await;
        record(
            &pool,
            Some("a1"),
            Some("d1"),
            actions::DEVICE_RENAMED,
            Some("d2"),
        )
        .await;

        // (账户, 设备, 动作, 对象, 时间)
        type Row = (Option<String>, Option<String>, String, Option<String>, i64);
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT account_id, device_id, action, target, created_at FROM audit_log ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].2, "web.login");
        assert_eq!(rows[1].3.as_deref(), Some("d2"));
        assert!(rows[1].4 > 1_700_000_000_000);
    }
}
