use super::{capacity, storage};
use crate::{
    auth::Identity,
    error::ApiError,
    http::{Shared, read_json},
};
use assemble_protocol::{
    drive_tree::{ROOT, TreeFile},
    primitives::{Timestamp, is_sha256_hex, is_valid_id},
    uploads::{self as wire, Create, FolderRevision, Part, Phase, Upload},
};
use axum::{
    Json, Router,
    body::{Bytes, to_bytes},
    extract::{DefaultBodyLimit, Path, Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::Response,
    routing::{delete, get},
};
use sha2::{Digest, Sha256};
use sqlx::{Sqlite, Transaction};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/v1/drive/uploads", get(capabilities).post(create))
        .route("/v1/drive/uploads/requests/{key}", delete(cancel_key))
        .route("/v1/drive/uploads/{id}", get(status).delete(cancel))
        .route(
            "/v1/drive/uploads/{id}/parts/{index}",
            axum::routing::put(part),
        )
        .route(
            "/v1/drive/uploads/{id}/complete",
            axum::routing::post(complete),
        )
        .layer(DefaultBodyLimit::max(16 * 1024))
        .layer(middleware::from_fn(private))
}
async fn private(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        "cross-origin-resource-policy",
        HeaderValue::from_static("same-origin"),
    );
    response
}
async fn capabilities(_identity: Identity) -> Json<wire::Capabilities> {
    Json(wire::Capabilities::default())
}
fn conflict(code: &'static str) -> ApiError {
    ApiError::conflict(code, "the upload cannot proceed in its current state")
}
fn hex(bytes: &[u8]) -> String {
    assemble_protocol::primitives::to_hex(bytes)
}
fn random() -> Result<String, ApiError> {
    crate::secrets::random_id().map_err(|_| ApiError::internal("random generation failed"))
}
fn encode<T: serde::Serialize>(value: &T) -> Result<String, ApiError> {
    serde_json::to_string(value).map_err(|_| ApiError::internal("upload encoding failed"))
}
fn decode<T: serde::de::DeserializeOwned>(value: &str) -> Result<T, ApiError> {
    serde_json::from_str(value).map_err(|_| ApiError::internal("stored upload is invalid"))
}
fn key(identity: &Identity, request: &str) -> String {
    format!("{}/{}/{}", identity.account_id, identity.device_id, request)
}
#[derive(sqlx::FromRow)]
struct Row {
    id: String,
    request_id: String,
    input_json: Option<String>,
    state: String,
    expires_at: i64,
    created_at: i64,
    file_json: Option<String>,
}
impl Row {
    fn input(&self) -> Result<Create, ApiError> {
        decode(
            self.input_json
                .as_deref()
                .ok_or_else(|| conflict("UPLOAD_CANCELLED"))?,
        )
    }
    fn receiving(&self) -> Result<(), ApiError> {
        match self.state.as_str() {
            "receiving" => Ok(()),
            "expired" => Err(conflict("UPLOAD_EXPIRED")),
            _ => Err(conflict("UPLOAD_FINISHED")),
        }
    }
}
async fn load(state: &Shared, identity: &Identity, id: &str) -> Result<Row, ApiError> {
    if !is_valid_id(id) {
        return Err(ApiError::not_found());
    }
    sqlx::query_as("SELECT * FROM drive_uploads WHERE id=? AND account_id=? AND device_id=?")
        .bind(id)
        .bind(&identity.account_id)
        .bind(&identity.device_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(ApiError::not_found)
}
async fn current(state: &Shared, identity: &Identity, id: &str) -> Result<Row, ApiError> {
    let mut row = load(state, identity, id).await?;
    if row.state == "receiving" && row.expires_at <= Timestamp::now().as_millis() {
        let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query("UPDATE drive_uploads SET state='expired' WHERE id=? AND state='receiving'")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        storage::queue(&mut tx, id).await?;
        tx.commit().await?;
        row.state = "expired".into();
    }
    Ok(row)
}
async fn view(state: &Shared, row: &Row) -> Result<Upload, ApiError> {
    let parts:Vec<(i64,i64,String)>=sqlx::query_as("SELECT part_index,size,sha256 FROM drive_upload_parts WHERE upload_id=? ORDER BY part_index")
        .bind(&row.id).fetch_all(&state.pool).await?;
    Ok(Upload {
        id: row.id.clone(),
        phase: match row.state.as_str() {
            "receiving" => Phase::Receiving,
            "completed" => Phase::Completed,
            "cancelled" => Phase::Cancelled,
            "expired" => Phase::Expired,
            _ => return Err(ApiError::internal("invalid upload state")),
        },
        part_bytes: wire::PART_BYTES,
        parts: parts
            .into_iter()
            .map(|(index, size, sha256)| {
                Ok(Part {
                    index: u32::try_from(index)
                        .map_err(|_| ApiError::internal("invalid part index"))?,
                    size: u64::try_from(size)
                        .map_err(|_| ApiError::internal("invalid part size"))?,
                    sha256,
                })
            })
            .collect::<Result<_, ApiError>>()?,
        expires_at: Timestamp::from_millis(row.expires_at)
            .ok_or_else(|| ApiError::internal("invalid upload expiry"))?,
        file: row.file_json.as_deref().map(decode).transpose()?,
    })
}
async fn location(
    tx: &mut Transaction<'_, Sqlite>,
    account: &str,
    input: &Create,
) -> Result<(), ApiError> {
    let folders: Vec<(String, Option<String>, i64)> =
        sqlx::query_as("SELECT id,parent_id,version FROM active_drive_folders WHERE account_id=?")
            .bind(account)
            .fetch_all(&mut **tx)
            .await?;
    let mut chain = Vec::new();
    let mut parent = (input.parent_id != ROOT).then_some(input.parent_id.as_str());
    while let Some(id) = parent {
        if chain.len() >= assemble_protocol::drive_tree::MAX_DEPTH {
            return Err(conflict("UPLOAD_TARGET_CHANGED"));
        }
        let Some((id, next, version)) = folders.iter().find(|(fid, _, _)| fid == id) else {
            return Err(conflict("UPLOAD_TARGET_CHANGED"));
        };
        chain.push(FolderRevision {
            id: id.clone(),
            version: *version,
        });
        parent = next.as_deref();
    }
    chain.reverse();
    if chain != input.location {
        return Err(conflict("UPLOAD_TARGET_CHANGED"));
    }
    Ok(())
}
async fn precheck(
    state: &Shared,
    identity: &Identity,
    input: &Create,
    id: &str,
) -> Result<(), ApiError> {
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    crate::drive_tree::revalidate(&mut tx, identity).await?;
    location(&mut tx, &identity.account_id, input).await?;
    capacity::check(
        &mut tx,
        state,
        &identity.account_id,
        input.size,
        input.size,
        id,
    )
    .await?;
    tx.commit().await?;
    Ok(())
}
async fn create(
    State(state): State<Shared>,
    identity: Identity,
    body: Bytes,
) -> Result<Json<Upload>, ApiError> {
    let input: Create = read_json(&body)?;
    if !input.valid() {
        return Err(ApiError::invalid("invalid upload declaration"));
    }
    let _lock = state.uploads.lock(&key(&identity, &input.request_id)).await;
    let old: Option<String> = sqlx::query_scalar(
        "SELECT id FROM drive_uploads WHERE account_id=? AND device_id=? AND request_id=?",
    )
    .bind(&identity.account_id)
    .bind(&identity.device_id)
    .bind(&input.request_id)
    .fetch_optional(&state.pool)
    .await?;
    if let Some(id) = old {
        let row = current(&state, &identity, &id).await?;
        if row.input_json.is_none() {
            return Err(conflict("UPLOAD_CANCELLED"));
        }
        if row.input()? != input {
            return Err(conflict("UPLOAD_REQUEST_CONFLICT"));
        }
        if row.state == "receiving" {
            precheck(&state, &identity, &input, &id).await?;
        }
        return Ok(Json(view(&state, &row).await?));
    }
    // 分片和合并短暂共存；已有预留按两倍保守保留，不依赖声明摘要猜测去重。
    let reserved: i64 =
        sqlx::query_scalar("SELECT coalesce(sum(bytes),0) FROM upload_reservations")
            .fetch_one(&state.pool)
            .await?;
    state
        .uploads
        .headroom(
            u64::try_from(reserved)
                .unwrap_or(u64::MAX)
                .saturating_add(input.size)
                .saturating_mul(2),
        )
        .await?;
    let now = Timestamp::now().as_millis();
    let expires = now + wire::IDLE_MILLIS;
    let id = random()?;
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    crate::drive_tree::revalidate(&mut tx, &identity).await?;
    location(&mut tx, &identity.account_id, &input).await?;
    let (device,account,global,receipts):(i64,i64,i64,i64)=sqlx::query_as(
        "SELECT (SELECT count(*) FROM drive_uploads WHERE account_id=?1 AND device_id=?2 AND state='receiving'),
         (SELECT count(*) FROM drive_uploads WHERE account_id=?1 AND state='receiving'),
         (SELECT count(*) FROM drive_uploads WHERE state='receiving'),
         (SELECT count(*) FROM drive_uploads WHERE account_id=?1)").bind(&identity.account_id).bind(&identity.device_id).fetch_one(&mut *tx).await?;
    if device >= wire::DEVICE_LIMIT
        || account >= wire::ACCOUNT_LIMIT
        || global >= wire::GLOBAL_LIMIT
        || receipts >= wire::REQUEST_LIMIT
    {
        return Err(conflict("UPLOAD_LIMIT"));
    }
    capacity::reserve(
        &mut tx, &state, &identity, &id, input.size, input.size, expires,
    )
    .await?;
    sqlx::query("INSERT INTO drive_uploads(id,account_id,device_id,request_id,input_json,state,size,created_at,progressed_at,expires_at) VALUES (?,?,?,?,?,'receiving',?,?,?,?)")
        .bind(&id).bind(&identity.account_id).bind(&identity.device_id).bind(&input.request_id).bind(encode(&input)?)
        .bind(i64::try_from(input.size).map_err(|_|ApiError::invalid("invalid size"))?).bind(now).bind(now).bind(expires).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(
        view(&state, &load(&state, &identity, &id).await?).await?,
    ))
}
async fn status(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
) -> Result<Json<Upload>, ApiError> {
    let row = load(&state, &identity, &id).await?;
    let _lock = state.uploads.lock(&key(&identity, &row.request_id)).await;
    Ok(Json(
        view(&state, &current(&state, &identity, &id).await?).await?,
    ))
}
async fn part(
    State(state): State<Shared>,
    identity: Identity,
    Path((id, index)): Path<(String, u32)>,
    request: Request,
) -> Result<Json<Upload>, ApiError> {
    let _permit = state.uploads.requests.try_acquire().map_err(|_| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "UNAVAILABLE",
            "upload workers are busy",
        )
    })?;
    let row = load(&state, &identity, &id).await?;
    let _lock = state.uploads.lock(&key(&identity, &row.request_id)).await;
    let row = current(&state, &identity, &id).await?;
    row.receiving()?;
    let input = row.input()?;
    let length = wire::part_size(input.size, index)
        .ok_or_else(|| ApiError::invalid("invalid part index"))?;
    let digest = request
        .headers()
        .get("x-content-sha256")
        .and_then(|v| v.to_str().ok())
        .filter(|v| is_sha256_hex(v))
        .ok_or_else(|| ApiError::invalid("invalid part digest"))?
        .to_owned();
    precheck(&state, &identity, &input, &id).await?;
    let bytes = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        to_bytes(request.into_body(), usize::try_from(length).unwrap_or(0)),
    )
    .await
    .map_err(|_| {
        ApiError::new(
            StatusCode::REQUEST_TIMEOUT,
            "UNAVAILABLE",
            "upload part timed out",
        )
    })?
    .map_err(|_| ApiError::invalid("part exceeds its declared length"))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != length
        || hex(&Sha256::digest(&bytes)) != digest
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "HASH_MISMATCH",
            "part content does not match",
        ));
    }
    let old: Option<(i64, String)> = sqlx::query_as(
        "SELECT size,sha256 FROM drive_upload_parts WHERE upload_id=? AND part_index=?",
    )
    .bind(&id)
    .bind(i64::from(index))
    .fetch_optional(&state.pool)
    .await?;
    if old
        .as_ref()
        .is_some_and(|(size, sha)| u64::try_from(*size).ok() != Some(length) || sha != &digest)
    {
        return Err(conflict("UPLOAD_PART_CONFLICT"));
    }
    let directory = state.uploads.directory(&id);
    tokio::fs::create_dir_all(&directory).await?;
    let path = directory.join(index.to_string());
    if old.is_some() {
        match tokio::fs::read(&path).await {
            Ok(stored) if stored == bytes => return Ok(Json(view(&state, &row).await?)),
            Ok(_) => return Err(conflict("UPLOAD_PART_CONFLICT")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    state.uploads.headroom(length).await?;
    persist_part(&directory, index, &bytes).await?;
    let now = Timestamp::now().as_millis();
    if now >= row.expires_at {
        return Err(conflict("UPLOAD_EXPIRED"));
    }
    let expires = (now + wire::IDLE_MILLIS).min(row.created_at + wire::LIFETIME_MILLIS);
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    crate::drive_tree::revalidate(&mut tx, &identity).await?;
    location(&mut tx, &identity.account_id, &input).await?;
    sqlx::query("INSERT OR IGNORE INTO drive_upload_parts(upload_id,part_index,size,sha256) VALUES (?,?,?,?)").bind(&id).bind(i64::from(index)).bind(i64::try_from(length).unwrap_or(i64::MAX)).bind(digest).execute(&mut *tx).await?;
    if old.is_none() {
        sqlx::query("UPDATE drive_uploads SET progressed_at=?,expires_at=? WHERE id=?")
            .bind(now)
            .bind(expires)
            .bind(&id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE upload_reservations SET expires_at=? WHERE id=?")
            .bind(expires)
            .bind(&id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(Json(
        view(&state, &load(&state, &identity, &id).await?).await?,
    ))
}
async fn complete(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
) -> Result<Json<Upload>, ApiError> {
    let _permit = state.uploads.completions.try_acquire().map_err(|_| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "UNAVAILABLE",
            "upload completion workers are busy",
        )
    })?;
    let row = load(&state, &identity, &id).await?;
    let _lock = state.uploads.lock(&key(&identity, &row.request_id)).await;
    let row = current(&state, &identity, &id).await?;
    if row.state == "completed" {
        return Ok(Json(view(&state, &row).await?));
    }
    row.receiving()?;
    let input = row.input()?;
    precheck(&state, &identity, &input, &id).await?;
    let uploaded = view(&state, &row).await?;
    if uploaded.parts.len()
        != usize::try_from(wire::part_count(input.size).unwrap_or(0)).unwrap_or(0)
    {
        return Err(conflict("UPLOAD_INCOMPLETE"));
    }
    state.uploads.headroom(input.size.saturating_mul(2)).await?;
    let directory = state.uploads.directory(&id);
    tokio::fs::create_dir_all(&directory).await?;
    let merged = directory.join("merged.tmp");
    let mut output = tokio::fs::File::create(&merged).await?;
    let mut buffer = vec![0; 64 * 1024];
    let mut hash = Sha256::new();
    let mut total = 0_u64;
    for part in &uploaded.parts {
        let mut source = tokio::fs::File::open(directory.join(part.index.to_string())).await?;
        let mut part_hash = Sha256::new();
        let mut size = 0_u64;
        loop {
            let count = source.read(&mut buffer).await?;
            if count == 0 {
                break;
            }
            size += u64::try_from(count).unwrap_or(u64::MAX);
            if size > part.size {
                return Err(conflict("UPLOAD_PART_CONFLICT"));
            }
            part_hash.update(&buffer[..count]);
            hash.update(&buffer[..count]);
            output.write_all(&buffer[..count]).await?;
        }
        if size != part.size || hex(&part_hash.finalize()) != part.sha256 {
            return Err(conflict("UPLOAD_PART_CONFLICT"));
        }
        total += size;
    }
    if total != input.size || hex(&hash.finalize()) != input.sha256 {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "HASH_MISMATCH",
            "complete content does not match",
        ));
    }
    output.sync_all().await?;
    drop(output);
    let source = tokio::fs::File::open(&merged).await?;
    let lease = state
        .blobs
        .store(
            source,
            input.size,
            &input.sha256,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .map_err(|e| {
            let (code, status) = e.rejection();
            ApiError::new(
                StatusCode::from_u16(status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE),
                code,
                "upload publication failed",
            )
        })?;
    let result = record(&state, &identity, &row, &input).await;
    drop(lease);
    if result.is_err() {
        let _ = state.blobs.discard_unreferenced(&input.sha256).await;
    }
    result?;
    storage::cleanup(&state, &id).await?;
    Ok(Json(
        view(&state, &load(&state, &identity, &id).await?).await?,
    ))
}
async fn record(
    state: &Shared,
    identity: &Identity,
    row: &Row,
    input: &Create,
) -> Result<(), ApiError> {
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    let phase: String = sqlx::query_scalar("SELECT state FROM drive_uploads WHERE id=?")
        .bind(&row.id)
        .fetch_one(&mut *tx)
        .await?;
    if phase == "completed" {
        return Ok(());
    }
    if phase != "receiving" {
        return Err(conflict("UPLOAD_FINISHED"));
    }
    capacity::commit_check(
        &mut tx,
        state,
        identity,
        &row.id,
        input.size,
        &input.sha256,
        true,
    )
    .await?;
    location(&mut tx, &identity.account_id, input).await?;
    if Timestamp::now().as_millis() >= row.expires_at {
        return Err(conflict("UPLOAD_EXPIRED"));
    }
    let now = Timestamp::now();
    let id = random()?;
    crate::blobs::retain(&mut *tx, &input.sha256, input.size).await?;
    sqlx::query("INSERT INTO drive_files(id,account_id,name,size,mime_type,sha256,created_at,parent_id) VALUES (?,?,?,?,?,?,?,?)")
        .bind(&id).bind(&identity.account_id).bind(&input.name).bind(i64::try_from(input.size).unwrap_or(i64::MAX)).bind(&input.mime_type).bind(&input.sha256).bind(now.as_millis())
        .bind(assemble_protocol::drive_tree::parent_id(&input.parent_id)).execute(&mut *tx).await?;
    let file = TreeFile {
        file: assemble_protocol::drive::DriveFile {
            id,
            name: input.name.clone(),
            size: input.size,
            mime_type: input.mime_type.clone(),
            sha256: input.sha256.clone(),
            created_at: now,
        },
        parent_id: assemble_protocol::drive_tree::parent_id(&input.parent_id).map(str::to_owned),
        version: 1,
        updated_at: None,
    };
    sqlx::query(
        "UPDATE drive_uploads SET state='completed',file_json=? WHERE id=? AND state='receiving'",
    )
    .bind(encode(&file)?)
    .bind(&row.id)
    .execute(&mut *tx)
    .await?;
    storage::queue(&mut tx, &row.id).await?;
    sqlx::query("INSERT INTO audit_log(account_id,device_id,action,target,created_at) VALUES (?,?,'drive.created',?,?)")
        .bind(&identity.account_id).bind(&identity.device_id).bind(&file.file.id).bind(now.as_millis()).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}
async fn cancel(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
) -> Result<Json<Upload>, ApiError> {
    let row = load(&state, &identity, &id).await?;
    cancel_request(&state, &identity, &row.request_id).await
}
async fn cancel_key(
    State(state): State<Shared>,
    identity: Identity,
    Path(request): Path<String>,
) -> Result<Json<Upload>, ApiError> {
    if !is_valid_id(&request) {
        return Err(ApiError::not_found());
    }
    cancel_request(&state, &identity, &request).await
}
async fn cancel_request(
    state: &Shared,
    identity: &Identity,
    request: &str,
) -> Result<Json<Upload>, ApiError> {
    let _lock = state.uploads.lock(&key(identity, request)).await;
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    crate::drive_tree::revalidate(&mut tx, identity).await?;
    let found: Option<(String, String)> = sqlx::query_as(
        "SELECT id,state FROM drive_uploads WHERE account_id=? AND device_id=? AND request_id=?",
    )
    .bind(&identity.account_id)
    .bind(&identity.device_id)
    .bind(request)
    .fetch_optional(&mut *tx)
    .await?;
    let id = if let Some((id, phase)) = found {
        if phase == "receiving" {
            sqlx::query("UPDATE drive_uploads SET state='cancelled' WHERE id=?")
                .bind(&id)
                .execute(&mut *tx)
                .await?;
            storage::queue(&mut tx, &id).await?;
        }
        id
    } else {
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM drive_uploads WHERE account_id=?")
                .bind(&identity.account_id)
                .fetch_one(&mut *tx)
                .await?;
        if count >= wire::REQUEST_LIMIT {
            return Err(conflict("UPLOAD_LIMIT"));
        }
        let id = random()?;
        let now = Timestamp::now().as_millis();
        sqlx::query("INSERT INTO drive_uploads(id,account_id,device_id,request_id,state,size,created_at,progressed_at,expires_at) VALUES (?,?,?,?,'cancelled',0,?,?,?)")
            .bind(&id).bind(&identity.account_id).bind(&identity.device_id).bind(request).bind(now).bind(now).bind(now).execute(&mut *tx).await?;
        id
    };
    tx.commit().await?;
    if load(state, identity, &id).await?.state == "cancelled" {
        storage::cleanup(state, &id).await?;
    }
    Ok(Json(view(state, &load(state, identity, &id).await?).await?))
}

async fn persist_part(
    directory: &std::path::Path,
    index: u32,
    bytes: &[u8],
) -> Result<(), ApiError> {
    let temporary = directory.join(format!("{index}.tmp"));
    let mut file = tokio::fs::File::create(&temporary).await?;
    file.write_all(bytes).await?;
    file.sync_all().await?;
    drop(file);
    tokio::fs::rename(temporary, directory.join(index.to_string())).await?;
    Ok(())
}
