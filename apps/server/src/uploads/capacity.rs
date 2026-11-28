//! 新旧上传共用的事务准入；预留只计一次，已确认分片不额外扣预算。
use crate::{auth::Identity, error::ApiError, http::Shared};
use axum::http::StatusCode;
use sqlx::{Sqlite, Transaction};

pub(crate) fn full() -> ApiError {
    ApiError::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        "QUOTA_EXCEEDED",
        "upload capacity is exhausted",
    )
}

pub(crate) async fn check(
    tx: &mut Transaction<'_, Sqlite>,
    state: &Shared,
    account: &str,
    size: u64,
    logical: u64,
    exclude: &str,
) -> Result<(), ApiError> {
    let (stored, reserved, used, account_reserved): (i64,i64,i64,i64) = sqlx::query_as(
        "SELECT (SELECT coalesce(sum(size),0) FROM blobs),
         ((SELECT coalesce(sum(bytes),0) FROM upload_reservations WHERE id<>?1)+(SELECT coalesce(sum(reserved_bytes),0) FROM hosted_magnets)),
         (SELECT coalesce(sum(size),0) FROM drive_files WHERE account_id=?2),
         (SELECT coalesce(sum(logical_bytes),0) FROM upload_reservations WHERE account_id=?2 AND id<>?1)")
        .bind(exclude).bind(account).fetch_one(&mut **tx).await?;
    if size > 0
        && u64::try_from(stored.saturating_add(reserved))
            .unwrap_or(u64::MAX)
            .saturating_add(size)
            > state.config.storage_budget_bytes
        || logical > 0
            && u64::try_from(used.saturating_add(account_reserved))
                .unwrap_or(u64::MAX)
                .saturating_add(logical)
                > state.config.drive_quota_bytes
    {
        return Err(full());
    }
    Ok(())
}

pub(crate) async fn reserve(
    tx: &mut Transaction<'_, Sqlite>,
    state: &Shared,
    identity: &Identity,
    id: &str,
    size: u64,
    logical: u64,
    expires: i64,
) -> Result<(), ApiError> {
    check(tx, state, &identity.account_id, size, logical, id).await?;
    sqlx::query("INSERT INTO upload_reservations(id,account_id,logical_bytes,bytes,expires_at) VALUES (?,?,?,?,?)")
        .bind(id).bind(&identity.account_id).bind(i64::try_from(logical).map_err(|_| full())?)
        .bind(i64::try_from(size).map_err(|_| full())?).bind(expires).execute(&mut **tx).await?;
    Ok(())
}

pub(crate) async fn release(tx: &mut Transaction<'_, Sqlite>, id: &str) -> Result<(), ApiError> {
    sqlx::query("DELETE FROM upload_reservations WHERE id=?")
        .bind(id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// 老接口仍完整接收并验摘要；存在内容只影响预算，不形成面向调用者的存在性接口。
pub(crate) async fn legacy_reserve(
    state: &Shared,
    identity: &Identity,
    size: u64,
    digest: &str,
    drive: bool,
) -> Result<String, ApiError> {
    state.uploads.headroom(size.saturating_mul(2)).await?;
    let id =
        crate::secrets::random_id().map_err(|_| ApiError::internal("random generation failed"))?;
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    crate::drive_tree::revalidate(&mut tx, identity).await?;
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM blobs WHERE sha256=?)")
        .bind(digest)
        .fetch_one(&mut *tx)
        .await?;
    reserve(
        &mut tx,
        state,
        identity,
        &id,
        if exists { 0 } else { size },
        if drive { size } else { 0 },
        assemble_protocol::primitives::Timestamp::now().as_millis() + 7_200_000,
    )
    .await?;
    tx.commit().await?;
    Ok(id)
}

pub(crate) async fn legacy_finish(state: &Shared, id: &str) -> Result<(), ApiError> {
    sqlx::query("DELETE FROM upload_reservations WHERE id=?")
        .bind(id)
        .execute(&state.pool)
        .await?;
    Ok(())
}

pub(crate) async fn commit_check(
    tx: &mut Transaction<'_, Sqlite>,
    state: &Shared,
    identity: &Identity,
    id: &str,
    size: u64,
    digest: &str,
    drive: bool,
) -> Result<(), ApiError> {
    crate::drive_tree::revalidate(tx, identity).await?;
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM blobs WHERE sha256=?)")
        .bind(digest)
        .fetch_one(&mut **tx)
        .await?;
    check(
        tx,
        state,
        &identity.account_id,
        if exists { 0 } else { size },
        if drive { size } else { 0 },
        id,
    )
    .await
}
