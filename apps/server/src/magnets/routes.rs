//! 每条操作都重新认证，调用方只能引用本账户的任务标识。
use super::{
    Account, ApiError, DownloadPhase, Duration, Identity, Instant, Preview, PreviewPhase, Row,
    Shared, TTL, Task, clear, error, now, row, valid_id,
};
use assemble_protocol::magnets::{self, Action, Control};
use axum::{
    Json,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderValue, header},
    response::Response,
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};

pub fn routes() -> axum::Router<Shared> {
    axum::Router::new()
        .route("/v1/magnets/previews", post(preview))
        .route("/v1/magnets/previews/{id}", get(preview_status))
        .route("/v1/magnets/previews/{id}/cancel", post(preview_cancel))
        .route("/v1/magnets/tasks", post(start))
        .route("/v1/magnets/tasks/{id}", get(status))
        .route("/v1/magnets/tasks/{id}/control", post(control))
        .route("/v1/magnets/tasks/{id}/manifest", get(manifest))
        .route(
            "/v1/magnets/tasks/{id}/files/{file}/chunks/{chunk}",
            get(chunk),
        )
        .layer(DefaultBodyLimit::max(8 * 1024 * 1024))
        .layer(axum::middleware::from_fn(private))
}
async fn private(request: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
async fn preview(
    State(state): State<Shared>,
    identity: Identity,
    body: Bytes,
) -> Result<Json<Preview>, ApiError> {
    if body.len() > 20 * 1024 {
        return Err(ApiError::invalid("magnet request is too large"));
    }
    let input = crate::http::read_json(&body)?;
    Ok(Json(
        state
            .magnets
            .create_preview(&state, &identity, input)
            .await?,
    ))
}
async fn preview_status(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
) -> Result<Json<Preview>, ApiError> {
    Ok(Json(state.magnets.preview(&identity.account_id, &id)?))
}
async fn preview_cancel(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
) -> Result<Json<Preview>, ApiError> {
    let mut previews = state
        .magnets
        .previews
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let pending = previews
        .get_mut(&(identity.account_id, id))
        .ok_or_else(ApiError::not_found)?;
    pending.cancel.cancel();
    pending.reply.phase = PreviewPhase::Failed;
    pending.reply.metainfo = None;
    pending.reply.error_code = Some("DOWNLOAD_CANCELLED".into());
    Ok(Json(pending.reply.clone()))
}
async fn owned(state: &Shared, identity: &Identity, id: &str) -> Result<Row, ApiError> {
    if !valid_id(id) {
        return Err(ApiError::not_found());
    }
    row(state, &identity.account_id, id)
        .await?
        .ok_or_else(ApiError::not_found)
}
async fn start(
    State(state): State<Shared>,
    identity: Identity,
    body: Bytes,
) -> Result<Json<Task>, ApiError> {
    let input: magnets::Start = crate::http::read_json(&body)?;
    if !valid_id(&input.request_id)
        || input.metainfo.len() > 6 * 1024 * 1024
        || input.file_ids.is_empty()
        || input.file_ids.len() > 10000
    {
        return Err(ApiError::invalid("invalid hosted start"));
    }
    let metadata = STANDARD
        .decode(&input.metainfo)
        .map_err(|_| ApiError::invalid("invalid metainfo encoding"))?;
    let (hash, size) = Account::inspect(&metadata, &input.file_ids).map_err(error)?;
    if hash != input.info_hash {
        return Err(ApiError::invalid("metainfo identity does not match"));
    }
    let mut ids = input.file_ids;
    ids.sort_unstable();
    let selection =
        serde_json::to_string(&ids).map_err(|_| ApiError::internal("selection encoding failed"))?;
    let _operation = state.magnets.operations.lock().await;
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    crate::drive_tree::revalidate(&mut tx, &identity).await?;
    let existing: Option<(String, String, String)> = sqlx::query_as(
        "SELECT info_hash,file_ids,state FROM hosted_magnets WHERE account_id=? AND id=?",
    )
    .bind(&identity.account_id)
    .bind(&input.request_id)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some((old, files, phase)) = existing {
        // 取消可先于迟到的创建请求抵达；只补齐回执身份，不能恢复预留或下载。
        if old.is_empty() && phase == "cancelled" {
            sqlx::query("UPDATE hosted_magnets SET info_hash=?,file_ids=?,selected_bytes=? WHERE account_id=? AND id=?")
                .bind(&hash).bind(&selection).bind(i64::try_from(size).map_err(|_|ApiError::invalid("invalid selection size"))?)
                .bind(&identity.account_id).bind(&input.request_id).execute(&mut *tx).await?;
        } else if old != hash || files != selection {
            return Err(ApiError::conflict(
                "REQUEST_CONFLICT",
                "hosted start changed",
            ));
        }
        tx.commit().await?;
        let record = owned(&state, &identity, &input.request_id).await?;
        return Ok(Json(state.magnets.task(&state, &record).await?));
    }
    let reserve = size
        .checked_mul(2)
        .ok_or_else(|| error(assemble_download::DownloadError::Limit))?;
    let used:(i64,i64,i64,i64,i64,i64)=sqlx::query_as("SELECT (SELECT coalesce(sum(size),0) FROM blobs)+(SELECT coalesce(sum(bytes),0) FROM upload_reservations)+(SELECT coalesce(sum(reserved_bytes),0) FROM hosted_magnets),(SELECT coalesce(sum(selected_bytes),0) FROM hosted_magnets WHERE account_id=? AND state='active'),(SELECT count(*) FROM hosted_magnets WHERE account_id=? AND state='active'),(SELECT count(*) FROM hosted_magnets WHERE state='active'),(SELECT coalesce(sum(length(metadata)),0) FROM hosted_magnets),(SELECT count(*) FROM hosted_magnets)").bind(&identity.account_id).bind(&identity.account_id).fetch_one(&mut*tx).await?;
    if u64::try_from(used.0)
        .unwrap_or(u64::MAX)
        .saturating_add(reserve)
        > state.config.storage_budget_bytes
        || u64::try_from(used.1)
            .unwrap_or(u64::MAX)
            .saturating_add(size)
            > state.config.drive_quota_bytes
    {
        return Err(error(assemble_download::DownloadError::ServerQuota));
    }
    if used.2 >= 8
        || used.3 >= 128
        || used
            .4
            .saturating_add(i64::try_from(metadata.len()).unwrap_or(i64::MAX))
            > 64 * 1024 * 1024
        || used.5 >= 10000
    {
        return Err(error(assemble_download::DownloadError::Busy));
    }
    sqlx::query("INSERT INTO hosted_magnets(account_id,id,info_hash,metadata,file_ids,selected_bytes,reserved_bytes,state,created_at,expires_at) VALUES (?,?,?,?,?,?,?,'active',?,?)").bind(&identity.account_id).bind(&input.request_id).bind(hash).bind(metadata).bind(selection).bind(i64::try_from(size).map_err(|_|ApiError::invalid("invalid selection size"))?).bind(i64::try_from(reserve).map_err(|_|ApiError::invalid("invalid reservation"))?).bind(now()).bind(now()+TTL).execute(&mut*tx).await?;
    tx.commit().await?;
    let record = owned(&state, &identity, &input.request_id).await?;
    Ok(Json(state.magnets.task(&state, &record).await?))
}
async fn status(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
) -> Result<Json<Task>, ApiError> {
    let record = owned(&state, &identity, &id).await?;
    Ok(Json(state.magnets.task(&state, &record).await?))
}
async fn control(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<Task>, ApiError> {
    if body.len() > 1024 {
        return Err(ApiError::invalid("invalid hosted action"));
    }
    let input: Control = crate::http::read_json(&body)?;
    let key = (identity.account_id.clone(), id.clone());
    if matches!(input.action, Action::Heartbeat) {
        return heartbeat(&state, &identity, &id).await;
    }
    let _operation = state.magnets.operations.lock().await;
    if matches!(input.action, Action::Cancel) {
        fence_cancel(&state, &identity, &id).await?;
    }
    let record = owned(&state, &identity, &id).await?;
    if record.state != "active" && record.metadata.is_empty() {
        return Ok(Json(state.magnets.task(&state, &record).await?));
    }
    match input.action {
        Action::Resume => {
            if record.state == "active" && record.expires_at > now() {
                let account = state.magnets.account(&state, &identity.account_id).await?;
                account
                    .install(&id, &record.metadata, &record.ids()?)
                    .await
                    .map_err(error)?;
                account.resume(&id).await.map_err(error)?;
                state
                    .magnets
                    .leases
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(key, Instant::now() + Duration::from_secs(15));
            }
        }
        Action::Pause => {
            state
                .magnets
                .leases
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&key);
            let account = state
                .magnets
                .accounts
                .lock()
                .await
                .get(&identity.account_id)
                .cloned();
            if let Some(account) = account
                && account.task(&id).is_ok()
            {
                account.pause(&id).await.map_err(error)?;
            }
        }
        Action::Release | Action::Cancel => {
            if matches!(input.action, Action::Release)
                && record.state == "active"
                && state.magnets.task(&state, &record).await?.phase != DownloadPhase::Completed
            {
                return Err(ApiError::conflict(
                    "DOWNLOAD_BUSY",
                    "download is not complete",
                ));
            }
            let phase = if record.state != "active" {
                record.state.as_str()
            } else if matches!(input.action, Action::Release) {
                "released"
            } else {
                "cancelled"
            };
            sqlx::query("UPDATE hosted_magnets SET state=? WHERE account_id=? AND id=?")
                .bind(phase)
                .bind(&identity.account_id)
                .bind(&id)
                .execute(&state.pool)
                .await?;
            state
                .magnets
                .leases
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&key);
            state
                .magnets
                .purge(&state, &identity.account_id, &id)
                .await?;
            clear(&state, &identity.account_id, &id).await?;
        }
        Action::Heartbeat => unreachable!("heartbeat handled before the operation lock"),
    }
    let record = owned(&state, &identity, &id).await?;
    Ok(Json(state.magnets.task(&state, &record).await?))
}
async fn heartbeat(state: &Shared, identity: &Identity, id: &str) -> Result<Json<Task>, ApiError> {
    let record = owned(state, identity, id).await?;
    if record.state == "active" && record.expires_at > now() {
        let key = (identity.account_id.clone(), id.into());
        {
            let mut leases = state
                .magnets
                .leases
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(end) = leases.get_mut(&key) {
                *end = Instant::now() + Duration::from_secs(15);
            }
        }
        sqlx::query(
            "UPDATE hosted_magnets SET expires_at=? WHERE account_id=? AND id=? AND state='active'",
        )
        .bind(now() + TTL)
        .bind(&identity.account_id)
        .bind(id)
        .execute(&state.pool)
        .await?;
    }
    Ok(Json(state.magnets.task(state, &record).await?))
}
async fn fence_cancel(state: &Shared, identity: &Identity, id: &str) -> Result<(), ApiError> {
    if !valid_id(id) {
        return Err(ApiError::not_found());
    }
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    crate::drive_tree::revalidate(&mut tx, identity).await?;
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM hosted_magnets WHERE account_id=? AND id=?)",
    )
    .bind(&identity.account_id)
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    if !exists {
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM hosted_magnets")
            .fetch_one(&mut *tx)
            .await?;
        if count >= 10000 {
            return Err(error(assemble_download::DownloadError::Busy));
        }
        sqlx::query("INSERT INTO hosted_magnets(account_id,id,info_hash,metadata,file_ids,selected_bytes,reserved_bytes,state,created_at,expires_at) VALUES (?,?,'',X'','[]',0,0,'cancelled',?,?)")
            .bind(&identity.account_id).bind(id).bind(now()).bind(now()+TTL).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}
async fn manifest(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
) -> Result<Json<magnets::Manifest>, ApiError> {
    let record = owned(&state, &identity, &id).await?;
    if record.state != "active" || record.expires_at <= now() {
        return Err(ApiError::not_found());
    }
    Ok(Json(
        state
            .magnets
            .account(&state, &identity.account_id)
            .await?
            .manifest(&id)
            .await
            .map_err(error)?,
    ))
}
async fn chunk(
    State(state): State<Shared>,
    identity: Identity,
    Path((id, file, chunk)): Path<(String, u32, u64)>,
) -> Result<Response, ApiError> {
    let record = owned(&state, &identity, &id).await?;
    if record.state != "active" || record.expires_at <= now() {
        return Err(ApiError::not_found());
    }
    #[cfg(test)]
    {
        state.magnets.chunk_calls.lock().unwrap().push(chunk);
        if state
            .magnets
            .fail_chunk
            .compare_exchange(
                chunk,
                u64::MAX,
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
            )
            .is_ok()
        {
            return Response::builder()
                .body(Body::from("corrupt test response"))
                .map_err(|_| ApiError::internal("test fault response"));
        }
    }
    let bytes = state
        .magnets
        .account(&state, &identity.account_id)
        .await?
        .chunk(&id, file, chunk)
        .await
        .map_err(error)?;
    Response::builder()
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_LENGTH, bytes.len())
        .body(Body::from(bytes))
        .map_err(|_| ApiError::internal("chunk response failed"))
}
