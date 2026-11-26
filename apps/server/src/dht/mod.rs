//! 只供网页超级管理员使用的观测、队列与运行编排。
pub mod config;
mod routes;
pub mod runtime;
pub mod store;
pub use routes::routes;

pub const QUEUE_LIMIT: i64 = 2000;
pub const WORKERS: usize = 4;
pub const METADATA_BUDGET: i64 = 512 * 1024 * 1024;

pub fn now() -> i64 {
    assemble_protocol::primitives::Timestamp::now().as_millis()
}

/// 网页与服务器运维共用模式变更；不绕过运行配置，重复请求不重复记审计。
pub async fn set_mode(
    pool: &sqlx::SqlitePool,
    enabled: bool,
    mode: assemble_protocol::dht::DhtMode,
    actor: Option<&str>,
) -> Result<(), crate::error::ApiError> {
    if !enabled && mode != assemble_protocol::dht::DhtMode::Stopped {
        return Err(crate::error::ApiError::conflict(
            "DHT_UNAVAILABLE",
            "DHT is not configured",
        ));
    }
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let changed = sqlx::query("UPDATE dht_control SET mode = ? WHERE id = 1 AND mode != ?")
        .bind(mode.as_str())
        .bind(mode.as_str())
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if changed > 0 {
        sqlx::query("INSERT INTO audit_log (account_id, action, target, created_at) VALUES (?, 'dht.mode_changed', ?, ?)")
            .bind(actor).bind(mode.as_str()).bind(now()).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}
