//! 管理系统通知：维护任务产生问题，读取不改变状态，已读只属于当前管理员。

use assemble_protocol::admin::AdminStorage;
use assemble_protocol::primitives::Timestamp;
use assemble_protocol::system_notifications::{
    SystemNotificationContent as Content, SystemNotificationLevel as Level, SystemNotifications,
    SystemNotificationsRead,
};
use assemble_store::notifications::{NotificationLog, Signal};
use sqlx::{Sqlite, SqlitePool, Transaction};

use crate::auth::WebSession;
use crate::error::ApiError;

const SCOPE: &str = "administrators";

async fn load(tx: &mut Transaction<'_, Sqlite>) -> Result<NotificationLog, sqlx::Error> {
    let state: Option<String> =
        sqlx::query_scalar("SELECT state_json FROM system_notification_state WHERE id=1")
            .fetch_optional(&mut **tx)
            .await?;
    state.map_or_else(
        || Ok(NotificationLog::default()),
        |state| {
            serde_json::from_str(&state)
                .map_err(|_| sqlx::Error::Protocol("invalid system notification state".into()))
        },
    )
}

pub async fn observe_storage(pool: &SqlitePool, storage: &AdminStorage) -> Result<(), sqlx::Error> {
    let mut signals = Vec::new();
    if storage.budget_warning {
        signals.push(Signal {
            key: "storage-budget".into(),
            content: Content::StorageBudget {
                used_bytes: storage.content_bytes,
                budget_bytes: storage.budget_bytes,
                warning_percent: storage.warning_percent,
            },
            level: if storage.content_bytes >= storage.budget_bytes {
                Level::Error
            } else {
                Level::Warning
            },
        });
    }
    if storage.disk_warning
        && let Some(available_bytes) = storage.disk_available_bytes
    {
        signals.push(Signal {
            key: "storage-disk".into(),
            content: Content::StorageDisk { available_bytes },
            level: Level::Error,
        });
    }
    if storage.database_bytes.is_none() || storage.disk_available_bytes.is_none() {
        signals.push(Signal {
            key: "storage-unavailable".into(),
            content: Content::StorageUnavailable,
            level: Level::Warning,
        });
    }
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let mut log = load(&mut tx).await?;
    let previous = log.clone();
    log.observe(SCOPE, &signals, Timestamp::now());
    if previous != log {
        let state = serde_json::to_string(&log)
            .map_err(|_| sqlx::Error::Protocol("invalid system notification state".into()))?;
        sqlx::query("INSERT INTO system_notification_state(id,state_json) VALUES (1,?) ON CONFLICT(id) DO UPDATE SET state_json=excluded.state_json").bind(state).execute(&mut *tx).await?;
    }
    sqlx::query("DELETE FROM system_notification_reads WHERE read_at < ?")
        .bind(Timestamp::now().as_millis() - 90 * 24 * 60 * 60 * 1000)
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}

async fn view(
    tx: &mut Transaction<'_, Sqlite>,
    account: &str,
) -> Result<SystemNotifications, sqlx::Error> {
    let mut notifications = load(tx).await?.view(&[SCOPE]);
    let reads: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT notification_id,revision,read_at FROM system_notification_reads WHERE account_id=?",
    )
    .bind(account)
    .fetch_all(&mut **tx)
    .await?;
    for item in &mut notifications.items {
        item.read_at = reads
            .iter()
            .find(|(id, revision, _)| {
                id == &item.id && u64::try_from(*revision) == Ok(item.revision)
            })
            .and_then(|(_, _, at)| Timestamp::from_millis(*at));
    }
    Ok(notifications)
}

pub async fn list(pool: &SqlitePool, account: &str) -> Result<SystemNotifications, ApiError> {
    let mut tx = pool.begin().await?;
    let result = view(&mut tx, account).await?;
    tx.commit().await?;
    Ok(result)
}

pub async fn mark_read(
    pool: &SqlitePool,
    actor: &WebSession,
    input: SystemNotificationsRead,
) -> Result<SystemNotifications, ApiError> {
    if input.items.is_empty()
        || input.items.len() > 100
        || input
            .items
            .iter()
            .any(|item| item.id.len() > 100 || item.id.is_empty() || item.revision == 0)
    {
        return Err(ApiError::invalid("invalid system notification selection"));
    }
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    crate::admin::verify_writer_in(&mut tx, actor).await?;
    let current = load(&mut tx).await?.view(&[SCOPE]);
    let now = Timestamp::now().as_millis();
    for item in input.items {
        if current.items.iter().any(|notification| {
            notification.id == item.id && notification.revision == item.revision
        }) {
            let revision = i64::try_from(item.revision)
                .map_err(|_| ApiError::invalid("invalid system notification version"))?;
            sqlx::query("INSERT INTO system_notification_reads(notification_id,account_id,revision,read_at) VALUES (?,?,?,?) ON CONFLICT(notification_id,account_id) DO UPDATE SET revision=excluded.revision,read_at=excluded.read_at WHERE excluded.revision>system_notification_reads.revision")
                .bind(item.id).bind(&actor.account_id).bind(revision).bind(now).execute(&mut *tx).await?;
        }
    }
    let result = view(&mut tx, &actor.account_id).await?;
    tx.commit().await?;
    Ok(result)
}
