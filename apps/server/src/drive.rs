//! 云盘：账户在服务端的一份文件。第一版是平铺的一层，没有文件夹。
//!
//! - **内容按摘要存放、按引用计数**（见 `blobs`）：同一个文件存两回，磁盘上只有一份；
//!   删掉其中一个，另一个照常能下载。
//! - **每个账户有配额**，按文件声明的长度算。收正文之前先按"收下之后会不会超"拒绝，
//!   免得白收一遍；内容收完、核对过之后再算一次——那一次在事务里，才是准数。
//! - **同名不冲突**：两个文件可以同名，靠标识区分。
//!
//! 与发送不同，云盘里的内容不会自己消失，只有删除才释放。

use assemble_protocol::drive::{
    DriveFile, DriveListing, RenameInput, clean_file_name, is_mime_type,
};
use assemble_protocol::error::codes;
use assemble_protocol::primitives::{MAX_FILE_SIZE, Timestamp, is_sha256_hex};
use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Query, Request, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::TryStreamExt;
use serde::Deserialize;
use tokio_util::io::StreamReader;
use tokio_util::sync::CancellationToken;

use crate::audit::{self, actions};
use crate::auth::Identity;
use crate::blobs::{self, StoreError};
use crate::error::ApiError;
use crate::http::{self, Shared};

/// 云盘里一个文件在库里的样子。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DriveRow {
    pub id: String,
    pub account_id: String,
    pub name: String,
    pub size: i64,
    pub mime_type: Option<String>,
    pub sha256: String,
    pub created_at: i64,
}

impl DriveRow {
    fn view(&self) -> DriveFile {
        DriveFile {
            id: self.id.clone(),
            name: self.name.clone(),
            size: u64::try_from(self.size).unwrap_or(u64::MAX),
            mime_type: self.mime_type.clone(),
            sha256: self.sha256.clone(),
            created_at: Timestamp::from_millis(self.created_at).unwrap_or_else(Timestamp::now),
        }
    }
}

/// 存一个文件时的参数，放在查询串里；正文就是文件的字节。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewDriveFile {
    name: String,
    size: u64,
    sha256: String,
    #[serde(default)]
    mime_type: Option<String>,
}

/// 这个账户的云盘已经占了多少字节。
async fn used_bytes<'e, E>(executor: E, account_id: &str) -> Result<u64, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let used: Option<i64> =
        sqlx::query_scalar("SELECT SUM(size) FROM drive_files WHERE account_id = ?")
            .bind(account_id)
            .fetch_one(executor)
            .await?;
    // 大小在入库时就是非负的；万一存成了负数，按没占地方算，也不让它变成一个天文数字。
    Ok(u64::try_from(used.unwrap_or(0)).unwrap_or(0))
}

fn quota_exceeded() -> ApiError {
    ApiError::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        codes::QUOTA_EXCEEDED,
        "the drive has no room for this file",
    )
}

fn too_large() -> ApiError {
    ApiError::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        codes::FILE_TOO_LARGE,
        "the file exceeds the single-file size limit",
    )
}

fn rejected(error: &StoreError) -> ApiError {
    let (code, status) = error.rejection();
    if matches!(error, StoreError::Io(_)) {
        tracing::warn!(%error, "an upload could not be stored");
    }
    ApiError::new(
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        code,
        match error {
            StoreError::TooLarge => "the file exceeds the single-file size limit",
            StoreError::Mismatch => "the content does not match the declared digest or size",
            StoreError::InvalidDigest => "the declared digest is not a SHA-256 hex string",
            StoreError::Stalled => "the upload stopped producing data",
            StoreError::Cancelled => "the upload was cancelled",
            StoreError::Io(_) => "the content could not be stored",
        },
    )
}

/// 核对存文件的参数与请求头。返回去掉两端空白后的文件名。
fn check_new_file(
    input: &NewDriveFile,
    headers: &axum::http::HeaderMap,
) -> Result<String, ApiError> {
    let name = clean_file_name(&input.name)
        .ok_or_else(|| ApiError::invalid("the file name is not acceptable"))?
        .to_owned();
    if !is_sha256_hex(&input.sha256) {
        return Err(ApiError::invalid("the digest is not a SHA-256 hex string"));
    }
    if input
        .mime_type
        .as_deref()
        .is_some_and(|mime| !is_mime_type(mime))
    {
        return Err(ApiError::invalid("the media type is not acceptable"));
    }
    // 请求头已经说明正文有多大：不必读下去就能拒绝。
    let announced = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    if input.size > MAX_FILE_SIZE || announced.is_some_and(|length| length > MAX_FILE_SIZE) {
        return Err(too_large());
    }
    Ok(name)
}

/// `POST /v1/drive/files?name=&size=&sha256=&mimeType=`：把一个文件存进云盘。
pub async fn upload(
    State(state): State<Shared>,
    identity: Identity,
    query: Result<Query<NewDriveFile>, axum::extract::rejection::QueryRejection>,
    request: Request,
) -> Result<Response, ApiError> {
    let Query(input) = query
        .map_err(|_| ApiError::invalid("the query string is not a valid drive upload request"))?;
    let (parts, body) = request.into_parts();
    let name = check_new_file(&input, &parts.headers);

    // 这几项检查不用看正文。拒绝之前要让对方把话说完，见 [`http::refuse_upload`]。
    let limit = state.config.drive_quota_bytes;
    let admitted = match name {
        Ok(name) => {
            // 先按"收下之后会不会超"拒绝，免得白收一遍。
            let used = used_bytes(&state.pool, &identity.account_id).await?;
            if used.saturating_add(input.size) > limit {
                Err(quota_exceeded())
            } else {
                Ok(name)
            }
        }
        Err(error) => Err(error),
    };
    let name = match admitted {
        Ok(name) => name,
        Err(error) => return Err(http::refuse_upload(body, error).await),
    };

    // 正文里的传输错误在进入存放逻辑之前统一成 I/O 错误。
    let stream = body.into_data_stream().map_err(std::io::Error::other);
    let mut reader = StreamReader::new(stream);
    let cancel = CancellationToken::new();
    state
        .blobs
        .store(&mut reader, input.size, &input.sha256, &cancel)
        .await
        .map_err(|error| rejected(&error))?;

    match record_file(&state, &identity, &input, name).await {
        Ok(row) => {
            audit::record(
                &state.pool,
                Some(&identity.account_id),
                Some(&identity.device_id),
                actions::DRIVE_CREATED,
                Some(&row.id),
            )
            .await;
            tracing::info!(file = %row.id, size = row.size, "file stored in the drive");
            Ok((StatusCode::CREATED, Json(row.view())).into_response())
        }
        // 内容收下了，记录却没记上：内容不能留着没人认。
        Err(error) => {
            let _ = state.blobs.discard_unreferenced(&input.sha256).await;
            Err(error)
        }
    }
}

/// 给内容加引用、记下这个文件：要么都发生，要么都不。配额在这里再算一次。
async fn record_file(
    state: &Shared,
    identity: &Identity,
    input: &NewDriveFile,
    name: String,
) -> Result<DriveRow, ApiError> {
    let id = crate::secrets::random_id().map_err(|error| ApiError::internal(error.to_string()))?;
    let size = i64::try_from(input.size).map_err(|_| ApiError::invalid("the file is too large"))?;
    let now = Timestamp::now().as_millis();
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    // 上传可能花了一阵子，这期间别的文件也占了地方：以事务里看到的为准。
    let used = used_bytes(&mut *tx, &identity.account_id).await?;
    if used.saturating_add(input.size) > state.config.drive_quota_bytes {
        tx.rollback().await?;
        return Err(quota_exceeded());
    }
    blobs::retain(&mut *tx, &input.sha256, input.size).await?;
    sqlx::query(
        "INSERT INTO drive_files (id, account_id, name, size, mime_type, sha256, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&identity.account_id)
    .bind(&name)
    .bind(size)
    .bind(&input.mime_type)
    .bind(&input.sha256)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok(DriveRow {
        id,
        account_id: identity.account_id.clone(),
        name,
        size,
        mime_type: input.mime_type.clone(),
        sha256: input.sha256.clone(),
        created_at: now,
    })
}

/// 按标识取出这个账户云盘里的一个文件；不是这个账户的，与不存在同样回答。
async fn load(pool: &sqlx::SqlitePool, account_id: &str, id: &str) -> Result<DriveRow, ApiError> {
    sqlx::query_as::<_, DriveRow>("SELECT * FROM drive_files WHERE account_id = ? AND id = ?")
        .bind(account_id)
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(ApiError::not_found)
}

/// `GET /v1/drive/files`：账户云盘里的全部文件，新的在前；以及空间用了多少。
pub async fn list(
    State(state): State<Shared>,
    identity: Identity,
) -> Result<Json<DriveListing>, ApiError> {
    let rows: Vec<DriveRow> = sqlx::query_as(
        "SELECT * FROM drive_files WHERE account_id = ? ORDER BY created_at DESC, rowid DESC",
    )
    .bind(&identity.account_id)
    .fetch_all(&state.pool)
    .await?;
    let used = used_bytes(&state.pool, &identity.account_id).await?;
    Ok(Json(DriveListing {
        items: rows.iter().map(DriveRow::view).collect(),
        used_bytes: used,
        limit_bytes: state.config.drive_quota_bytes,
    }))
}

/// `GET /v1/drive/files/{id}/content`：文件的内容，可以只取一段。
pub async fn content(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
    request: Request,
) -> Result<Response, ApiError> {
    let row = load(&state.pool, &identity.account_id, &id).await?;
    Ok(http::file_response(
        &state.blobs.path_for(&row.sha256),
        &row.name,
        row.mime_type.as_deref(),
        request.headers(),
    )
    .await)
}

/// `PATCH /v1/drive/files/{id}`：改名字。
pub async fn rename(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<DriveFile>, ApiError> {
    let input: RenameInput = http::read_json(&body)?;
    let Some(name) = input.clean_name() else {
        return Err(ApiError::invalid("the file name is not acceptable"));
    };
    let row = load(&state.pool, &identity.account_id, &id).await?;
    sqlx::query("UPDATE drive_files SET name = ? WHERE id = ?")
        .bind(name)
        .bind(&row.id)
        .execute(&state.pool)
        .await?;
    audit::record(
        &state.pool,
        Some(&identity.account_id),
        Some(&identity.device_id),
        actions::DRIVE_RENAMED,
        Some(&row.id),
    )
    .await;
    let mut renamed = row;
    name.clone_into(&mut renamed.name);
    Ok(Json(renamed.view()))
}

/// `DELETE /v1/drive/files/{id}`：删掉一个文件。它是内容的最后一个引用时，内容随之释放。
pub async fn remove(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let row = load(&state.pool, &identity.account_id, &id).await?;
    let deleted = sqlx::query("DELETE FROM drive_files WHERE id = ?")
        .bind(&row.id)
        .execute(&state.pool)
        .await?
        .rows_affected();
    if deleted > 0 {
        state.blobs.release(&row.sha256).await?;
    }
    audit::record(
        &state.pool,
        Some(&identity.account_id),
        Some(&identity.device_id),
        actions::DRIVE_DELETED,
        Some(&row.id),
    )
    .await;
    tracing::info!(file = %row.id, "file removed from the drive");
    Ok(StatusCode::NO_CONTENT)
}
