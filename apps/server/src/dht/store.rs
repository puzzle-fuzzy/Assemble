use assemble_dht::{hash_text, magnet, metadata::Metadata};
use assemble_protocol::{
    dht::{DhtDetails, DhtMode, DhtObservation, MetadataStatus},
    primitives::Timestamp,
};
use sqlx::{FromRow, SqlitePool};

use super::{METADATA_BUDGET, QUEUE_LIMIT, now};

#[derive(FromRow)]
pub struct Row {
    id: String,
    first_seen: i64,
    last_seen: i64,
    announce_count: i64,
    sample_count: i64,
    status: String,
    name: Option<String>,
    total_size: Option<i64>,
    file_count: Option<i64>,
    fetched_at: Option<i64>,
    attempts: i64,
    last_error: Option<String>,
}
pub const COLUMNS: &str = "id, first_seen, last_seen, announce_count, sample_count, status, name, total_size, file_count, fetched_at, attempts, last_error";

fn malformed() -> sqlx::Error {
    sqlx::Error::Protocol("invalid persisted DHT state".into())
}
pub fn status(value: &str) -> Result<MetadataStatus, sqlx::Error> {
    match value {
        "pending" => Ok(MetadataStatus::Pending),
        "fetching" => Ok(MetadataStatus::Fetching),
        "deferred" => Ok(MetadataStatus::Deferred),
        "verified" => Ok(MetadataStatus::Verified),
        "not_found" => Ok(MetadataStatus::NotFound),
        "invalid" => Ok(MetadataStatus::Invalid),
        "failed" => Ok(MetadataStatus::Failed),
        _ => Err(malformed()),
    }
}
impl Row {
    pub fn view(self) -> Result<DhtObservation, sqlx::Error> {
        Ok(DhtObservation {
            magnet_uri: magnet(&self.id, self.name.as_deref()),
            id: self.id,
            first_seen: Timestamp::from_millis(self.first_seen).ok_or_else(malformed)?,
            last_seen: Timestamp::from_millis(self.last_seen).ok_or_else(malformed)?,
            announce_count: u64::try_from(self.announce_count).map_err(|_| malformed())?,
            sample_count: u64::try_from(self.sample_count).map_err(|_| malformed())?,
            status: status(&self.status)?,
            name: self.name,
            total_size: self
                .total_size
                .map(u64::try_from)
                .transpose()
                .map_err(|_| malformed())?,
            file_count: self
                .file_count
                .map(u32::try_from)
                .transpose()
                .map_err(|_| malformed())?,
            fetched_at: self
                .fetched_at
                .map(|value| Timestamp::from_millis(value).ok_or_else(malformed))
                .transpose()?,
            attempts: u32::try_from(self.attempts).map_err(|_| malformed())?,
            last_error: self.last_error,
        })
    }
}

pub async fn mode(pool: &SqlitePool) -> Result<DhtMode, sqlx::Error> {
    let mode: String = sqlx::query_scalar("SELECT mode FROM dht_control WHERE id = 1")
        .fetch_one(pool)
        .await?;
    match mode.as_str() {
        "stopped" => Ok(DhtMode::Stopped),
        "collecting" => Ok(DhtMode::Collecting),
        "draining" => Ok(DhtMode::Draining),
        _ => Err(malformed()),
    }
}

pub async fn enqueue_query(
    pool: &SqlitePool,
    hash: &str,
    max_records: i64,
) -> Result<(), crate::error::ApiError> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let existing: Option<(String, Option<i64>)> =
        sqlx::query_as("SELECT status, last_attempt_at FROM dht_observations WHERE id = ?")
            .bind(hash)
            .fetch_optional(&mut *tx)
            .await?;
    if existing
        .as_ref()
        .is_some_and(|(status, _)| ["verified", "pending", "fetching"].contains(&status.as_str()))
    {
        return Ok(());
    }
    if existing
        .as_ref()
        .and_then(|(_, attempted)| *attempted)
        .is_some_and(|at| now() - at < 60_000)
    {
        return Err(crate::error::ApiError::conflict(
            "DHT_RETRY_COOLDOWN",
            "wait before retrying metadata",
        ));
    }
    let queued: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM dht_observations WHERE status IN ('pending','fetching')",
    )
    .fetch_one(&mut *tx)
    .await?;
    if queued >= QUEUE_LIMIT {
        return Err(crate::error::ApiError::conflict(
            "DHT_QUEUE_FULL",
            "metadata queue is full",
        ));
    }
    if existing.is_some() {
        sqlx::query("UPDATE dht_observations SET status = 'pending', attempts = 0, last_error = NULL, next_attempt_at = 0 WHERE id = ?").bind(hash).execute(&mut *tx).await?;
    } else {
        let total: i64 = sqlx::query_scalar("SELECT count(*) FROM dht_observations")
            .fetch_one(&mut *tx)
            .await?;
        if total >= max_records {
            return Err(crate::error::ApiError::conflict(
                "DHT_RECORD_LIMIT",
                "DHT record limit reached",
            ));
        }
        sqlx::query("INSERT INTO dht_observations (id, first_seen, last_seen, status) VALUES (?,?,?,'pending')").bind(hash).bind(now()).bind(now()).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// 容量检查与插入同事务；重复观测只增加证据，不重置终态与尝试次数。
pub async fn observe(
    pool: &SqlitePool,
    hash: &str,
    announce: bool,
    max_records: i64,
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let existing = sqlx::query("UPDATE dht_observations SET last_seen = ?, announce_count = announce_count + ?, sample_count = sample_count + ? WHERE id = ?")
        .bind(now()).bind(i64::from(announce)).bind(i64::from(!announce)).bind(hash).execute(&mut *tx).await?.rows_affected();
    if existing > 0 {
        tx.commit().await?;
        return Ok(true);
    }
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM dht_observations")
        .fetch_one(&mut *tx)
        .await?;
    if total >= max_records {
        return Ok(false);
    }
    let queued: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM dht_observations WHERE status IN ('pending','fetching')",
    )
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO dht_observations (id, first_seen, last_seen, announce_count, sample_count, status) VALUES (?,?,?,?,?,?)")
        .bind(hash).bind(now()).bind(now()).bind(i64::from(announce)).bind(i64::from(!announce)).bind(if queued < QUEUE_LIMIT { "pending" } else { "deferred" }).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(true)
}

pub async fn detail(pool: &SqlitePool, hash: &str) -> Result<Option<DhtDetails>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let row: Option<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM dht_observations WHERE id = ?"
    )))
    .bind(hash)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let (files, piece_length, private, comment, source): (String, Option<i64>, Option<bool>, Option<String>, Option<String>) = sqlx::query_as("SELECT files_json, piece_length, private, comment, source FROM dht_observations WHERE id = ?").bind(hash).fetch_one(&mut *tx).await?;
    tx.commit().await?;
    Ok(Some(DhtDetails {
        observation: row.view()?,
        files: serde_json::from_str(&files).map_err(|_| malformed())?,
        piece_length: piece_length
            .map(u64::try_from)
            .transpose()
            .map_err(|_| malformed())?,
        private,
        comment,
        source,
    }))
}

pub async fn claim(pool: &SqlitePool) -> Result<Option<assemble_dht::InfoHash>, sqlx::Error> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let used: i64 = sqlx::query_scalar("SELECT coalesce(sum(length(CAST(files_json AS BLOB))),0) FROM dht_observations WHERE status = 'verified'").fetch_one(&mut *tx).await?;
    if used >= METADATA_BUDGET {
        return Ok(None);
    }
    let id: Option<String> = sqlx::query_scalar("UPDATE dht_observations SET status = 'fetching', attempts = attempts + 1, last_attempt_at = ? WHERE id = (SELECT id FROM dht_observations WHERE status = 'pending' AND attempts < 3 AND next_attempt_at <= ? ORDER BY announce_count > 0 DESC, first_seen, id LIMIT 1) RETURNING id")
        .bind(now()).bind(now()).fetch_optional(&mut *tx).await?;
    tx.commit().await?;
    id.map(|id| assemble_dht::parse_hash(&id).map_err(|_| malformed()))
        .transpose()
}

pub async fn promote(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let queued: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM dht_observations WHERE status IN ('pending','fetching')",
    )
    .fetch_one(&mut *tx)
    .await?;
    let available = (QUEUE_LIMIT - queued).clamp(0, 50);
    sqlx::query("UPDATE dht_observations SET status = 'pending' WHERE id IN (SELECT id FROM dht_observations WHERE status = 'deferred' ORDER BY announce_count > 0 DESC, first_seen, id LIMIT ?)").bind(available).execute(&mut *tx).await?;
    tx.commit().await
}

pub async fn finish(
    pool: &SqlitePool,
    hash: &assemble_dht::InfoHash,
    result: Result<Option<Metadata>, assemble_dht::wire::Failure>,
) -> Result<bool, sqlx::Error> {
    let id = hash_text(hash);
    if let Ok(Some(metadata)) = result {
        let files = serde_json::to_string(&metadata.files).map_err(|_| malformed())?;
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
        let used: i64 = sqlx::query_scalar("SELECT coalesce(sum(length(CAST(files_json AS BLOB))),0) FROM dht_observations WHERE status = 'verified'").fetch_one(&mut *tx).await?;
        let mut published = false;
        if used + i64::try_from(files.len()).map_err(|_| malformed())? > METADATA_BUDGET {
            sqlx::query("UPDATE dht_observations SET status = 'failed', last_error = 'metadata_storage_full' WHERE id = ? AND status = 'fetching'").bind(id).execute(&mut *tx).await?;
        } else {
            let changed = sqlx::query("UPDATE dht_observations SET status = 'verified', name = ?, total_size = ?, file_count = ?, fetched_at = ?, piece_length = ?, private = ?, comment = ?, source = ?, files_json = ?, last_error = NULL WHERE id = ? AND status = 'fetching'")
                .bind(metadata.name).bind(i64::try_from(metadata.total_size).map_err(|_| malformed())?).bind(i64::try_from(metadata.files.len()).map_err(|_| malformed())?).bind(now()).bind(i64::try_from(metadata.piece_length).map_err(|_| malformed())?).bind(metadata.private).bind(metadata.comment).bind(metadata.source).bind(files).bind(id).execute(&mut *tx).await?.rows_affected();
            published = changed > 0;
        }
        tx.commit().await?;
        Ok(published)
    } else {
        let status = if matches!(result, Err(assemble_dht::wire::Failure::Validation)) {
            "invalid"
        } else if result.is_ok() {
            "not_found"
        } else {
            "failed"
        };
        let error = result
            .err()
            .map_or_else(|| "metadata_no_peers".to_owned(), |error| error.to_string());
        sqlx::query("UPDATE dht_observations SET status = CASE WHEN attempts < 3 AND ? != 'invalid' THEN 'pending' ELSE ? END, last_error = ?, next_attempt_at = ? WHERE id = ? AND status = 'fetching'").bind(status).bind(status).bind(error).bind(now() + 60_000).bind(id).execute(pool).await?;
        Ok(false)
    }
}
