//! 回收批次与活动目录隔离；永久清理只在明确确认后解除引用。
mod read;
mod restore;

use crate::{
    auth::Identity,
    drive_tree,
    error::ApiError,
    http::{Shared, read_json},
};
use assemble_protocol::{
    drive_trash as wire,
    drive_tree::{Kind, ROOT},
    primitives::{Timestamp, is_sha256_hex, is_valid_id},
};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderValue, header},
    middleware::{self, Next},
    response::Response,
    routing::{get, post},
};
use read::{BatchRow, Record};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::{Sqlite, Transaction};

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/v1/drive/trash", get(read::list).post(move_handler))
        .route("/v1/drive/trash/preview", post(preview_handler))
        .route(
            "/v1/drive/trash/{id}",
            get(read::detail).delete(restore::purge),
        )
        .route(
            "/v1/drive/trash/{id}/restore-preview",
            post(restore::preview_handler),
        )
        .route(
            "/v1/drive/trash/{id}/restore",
            post(restore::restore_handler),
        )
        .route("/v1/admin/drive/trash", get(read::admin_list))
        .route("/v1/admin/drive/trash/{id}", get(read::admin_detail))
        .layer(DefaultBodyLimit::max(16 * 1024))
        .layer(middleware::from_fn(private))
}
async fn private(request: axum::extract::Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    for (key, value) in [
        (header::CACHE_CONTROL, "no-store"),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (
            header::HeaderName::from_static("cross-origin-resource-policy"),
            "same-origin",
        ),
    ] {
        response
            .headers_mut()
            .insert(key, HeaderValue::from_static(value));
    }
    response
}
fn conflict(code: &'static str) -> ApiError {
    ApiError::conflict(
        code,
        "the recycle bin operation conflicts with its current state",
    )
}
fn number(value: i64) -> Result<u64, ApiError> {
    u64::try_from(value).map_err(|_| ApiError::internal("invalid recycle bin count"))
}
fn timestamp(value: i64) -> Result<Timestamp, ApiError> {
    Timestamp::from_millis(value).ok_or_else(|| ApiError::internal("invalid recycle bin timestamp"))
}
fn kind(value: &str) -> Result<Kind, ApiError> {
    match value {
        "file" => Ok(Kind::File),
        "folder" => Ok(Kind::Folder),
        _ => Err(ApiError::internal("invalid recycle bin kind")),
    }
}
fn kind_name(value: Kind) -> &'static str {
    match value {
        Kind::File => "file",
        Kind::Folder => "folder",
    }
}
fn hash(value: &impl serde::Serialize) -> Result<String, ApiError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|_| ApiError::internal("revision could not be encoded"))?;
    Ok(assemble_protocol::primitives::to_hex(&Sha256::digest(
        bytes,
    )))
}
fn version(actual: i64, expected: i64) -> Result<(), ApiError> {
    if expected < 1 {
        Err(ApiError::invalid("invalid recycle bin version"))
    } else if actual != expected {
        Err(conflict("VERSION_CONFLICT"))
    } else {
        Ok(())
    }
}
async fn preview_handler(
    State(state): State<Shared>,
    identity: Identity,
    body: Bytes,
) -> Result<Json<wire::Preview>, ApiError> {
    let target: wire::Target = read_json(&body)?;
    let mut tx = state.pool.begin().await?;
    let (preview, _) = preview(&mut tx, &identity.account_id, &target).await?;
    tx.commit().await?;
    Ok(Json(preview))
}
async fn preview(
    tx: &mut Transaction<'_, Sqlite>,
    account: &str,
    target: &wire::Target,
) -> Result<(wire::Preview, Vec<Record>), ApiError> {
    if !is_valid_id(&target.id) {
        return Err(ApiError::invalid("invalid recycle bin target"));
    }
    let records = read::branch(tx, account, target).await?;
    let root = records
        .iter()
        .find(|row| row.id == target.id && row.kind == kind_name(target.kind))
        .ok_or_else(ApiError::not_found)?;
    version(root.version, target.version)?;
    let file_count = records.iter().filter(|row| row.kind == "file").count();
    let size = records
        .iter()
        .try_fold(0u64, |sum, row| -> Result<_, ApiError> {
            sum.checked_add(number(row.size.unwrap_or(0))?)
                .ok_or_else(|| ApiError::internal("recycle bin size overflow"))
        })?;
    let result = wire::Preview {
        target: target.clone(),
        name: root.name.clone(),
        file_count: file_count as u64,
        folder_count: (records.len() - file_count) as u64,
        size,
        revision: hash(&records)?,
    };
    Ok((result, records))
}
async fn move_handler(
    State(state): State<Shared>,
    identity: Identity,
    body: Bytes,
) -> Result<Json<wire::Batch>, ApiError> {
    move_to(&state, &identity, &read_json(&body)?)
        .await
        .map(Json)
}
pub(crate) async fn move_to(
    state: &Shared,
    identity: &Identity,
    input: &wire::Move,
) -> Result<wire::Batch, ApiError> {
    if !is_valid_id(&input.request_id) || !is_sha256_hex(&input.revision) {
        return Err(ApiError::invalid("invalid recycle bin request"));
    }
    let fingerprint = hash(&(&input.target, &input.revision))?;
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    drive_tree::revalidate(&mut tx, identity).await?;
    if let Some(prior) = sqlx::query_as::<_, BatchRow>(
        "SELECT * FROM drive_trash WHERE account_id=? AND request_id=?",
    )
    .bind(&identity.account_id)
    .bind(&input.request_id)
    .fetch_optional(&mut *tx)
    .await?
    {
        if prior.fingerprint != fingerprint {
            return Err(conflict("DRIVE_REQUEST_CONFLICT"));
        }
        if prior.state != "trashed" {
            return Err(conflict("DRIVE_TRASH_FINISHED"));
        }
        return prior.view();
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM drive_trash WHERE account_id=?")
        .bind(&identity.account_id)
        .fetch_one(&mut *tx)
        .await?;
    if count >= wire::MAX_BATCHES {
        return Err(conflict("DRIVE_TRASH_LIMIT"));
    }
    let (preview, records) = preview(&mut tx, &identity.account_id, &input.target).await?;
    if preview.revision != input.revision {
        return Err(conflict("DRIVE_TRASH_CHANGED"));
    }
    let root = records
        .iter()
        .find(|row| row.id == input.target.id && row.kind == kind_name(input.target.kind))
        .ok_or_else(ApiError::not_found)?;
    let location = read::location(&mut tx, &identity.account_id, root.parent_id.as_deref()).await?;
    let id = crate::secrets::random_id()
        .map_err(|_| ApiError::internal("recycle bin identifier unavailable"))?;
    let now = Timestamp::now().as_millis();
    sqlx::query("INSERT INTO drive_trash(id,account_id,request_id,fingerprint,kind,root_id,name,original_parent_id,original_location,deleted_at,version,file_count,folder_count,size,state) VALUES(?,?,?,?,?,?,?,?,?,?,1,?,?,?,'trashed')")
        .bind(&id).bind(&identity.account_id).bind(&input.request_id).bind(fingerprint).bind(kind_name(input.target.kind)).bind(&root.id).bind(&root.name).bind(&root.parent_id)
        .bind(serde_json::to_string(&location).map_err(|_|ApiError::internal("location could not be encoded"))?).bind(now)
        .bind(i64::try_from(preview.file_count).map_err(|_|ApiError::internal("invalid recycle bin count"))?)
        .bind(i64::try_from(preview.folder_count).map_err(|_|ApiError::internal("invalid recycle bin count"))?)
        .bind(i64::try_from(preview.size).map_err(|_|ApiError::internal("invalid recycle bin size"))?).execute(&mut *tx).await?;
    for (wanted, sql) in [
        ("file", "UPDATE drive_files SET trash_id="),
        ("folder", "UPDATE drive_folders SET trash_id="),
    ] {
        let selected: Vec<_> = records.iter().filter(|row| row.kind == wanted).collect();
        if selected.is_empty() {
            continue;
        }
        let mut query = sqlx::QueryBuilder::new(sql);
        query
            .push_bind(&id)
            .push(",version=version+1,updated_at=")
            .push_bind(now)
            .push(",parent_id=CASE WHEN id=")
            .push_bind(&root.id)
            .push(" THEN NULL ELSE parent_id END WHERE account_id=")
            .push_bind(&identity.account_id)
            .push(" AND trash_id IS NULL AND id IN (");
        let mut values = query.separated(",");
        for row in selected {
            values.push_bind(&row.id);
        }
        values.push_unseparated(")");
        query.build().execute(&mut *tx).await?;
    }
    let result = read::batch(&mut tx, &identity.account_id, &id)
        .await?
        .view()?;
    tx.commit().await?;
    audit(
        state,
        identity,
        input.target.kind,
        "trashed",
        &input.target.id,
    )
    .await;
    Ok(result)
}
pub(crate) async fn legacy_delete(
    state: &Shared,
    identity: &Identity,
    kind: Kind,
    id: &str,
    expected: Option<i64>,
) -> Result<(), ApiError> {
    let mut tx = state.pool.begin().await?;
    let root = read::node(&mut tx, &identity.account_id, kind, id, None).await?;
    if kind == Kind::File && expected.is_none() {
        let restored: Option<i64> =
            sqlx::query_scalar("SELECT restored_at FROM drive_files WHERE account_id=? AND id=?")
                .bind(&identity.account_id)
                .bind(id)
                .fetch_one(&mut *tx)
                .await?;
        if restored.is_some() {
            return Err(conflict("DRIVE_TRASH_UPGRADE_REQUIRED"));
        }
    }
    let target = wire::Target {
        kind,
        id: id.to_owned(),
        version: expected.unwrap_or(root.version),
    };
    let (preview, records) = preview(&mut tx, &identity.account_id, &target).await?;
    if kind == Kind::Folder && records.len() != 1 {
        return Err(conflict("DRIVE_FOLDER_NOT_EMPTY"));
    }
    tx.commit().await?;
    let request_id = crate::secrets::random_id()
        .map_err(|_| ApiError::internal("recycle bin request unavailable"))?;
    move_to(
        state,
        identity,
        &wire::Move {
            target,
            revision: preview.revision,
            request_id,
        },
    )
    .await?;
    Ok(())
}
async fn audit(state: &Shared, identity: &Identity, kind: Kind, action: &str, id: &str) {
    let action = match kind {
        Kind::File => format!("drive.{action}"),
        Kind::Folder => format!("drive.folder_{action}"),
    };
    crate::audit::record(
        &state.pool,
        Some(&identity.account_id),
        Some(&identity.device_id),
        &action,
        Some(id),
    )
    .await;
}
