//! 私人备忘录只接受设备身份，不提供管理员代看或跨账户查询。
use assemble_protocol::memo::{
    Memo, MemoContent, MemoCreate, MemoList, MemoQuery, MemoSummary, MemoUpdate,
};
use assemble_protocol::primitives::{Timestamp, is_valid_id};
use assemble_protocol::schedule::Versioned;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::get;
use axum::{Json, Router};
use sha2::{Digest, Sha256};
use sqlx::{Sqlite, Transaction};

use crate::auth::Identity;
use crate::error::ApiError;
use crate::http::{Shared, read_json};

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/v1/memos", get(list).post(create))
        .route("/v1/memos/{id}", get(get_one).put(update).delete(remove))
        .layer(DefaultBodyLimit::max(512 * 1024))
        .layer(middleware::from_fn(private_response))
}

async fn private_response(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        header::HeaderValue::from_static("nosniff"),
    );
    response
}

#[derive(sqlx::FromRow)]
struct Row {
    id: String,
    title: String,
    text: String,
    version: i64,
    created_at: i64,
    updated_at: i64,
}
fn time(value: i64) -> Result<Timestamp, ApiError> {
    Timestamp::from_millis(value).ok_or_else(|| ApiError::internal("invalid memo timestamp"))
}
impl Row {
    fn view(self) -> Result<Memo, ApiError> {
        Ok(Memo {
            locations: Vec::new(),
            id: self.id,
            version: self.version,
            content: MemoContent {
                title: self.title,
                text: self.text,
            },
            created_at: time(self.created_at)?,
            updated_at: time(self.updated_at)?,
        })
    }
}
async fn load(
    executor: impl sqlx::SqliteExecutor<'_>,
    account: &str,
    id: &str,
) -> Result<Memo, ApiError> {
    if !is_valid_id(id) {
        return Err(ApiError::not_found());
    }
    sqlx::query_as::<_, Row>(
        "SELECT id,title,text,version,created_at,updated_at FROM memos WHERE account_id=? AND id=?",
    )
    .bind(account)
    .bind(id)
    .fetch_optional(executor)
    .await?
    .ok_or_else(ApiError::not_found)?
    .view()
}
async fn revalidate(tx: &mut Transaction<'_, Sqlite>, identity: &Identity) -> Result<(), ApiError> {
    // 等待写锁期间可能已退出、移除设备或被停用，落盘前再次核验。
    let allowed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM devices d JOIN accounts a ON a.id=d.account_id WHERE d.id=? AND d.account_id=? AND a.suspended_at IS NULL)")
        .bind(&identity.device_id).bind(&identity.account_id).fetch_one(&mut **tx).await?;
    if allowed {
        Ok(())
    } else {
        Err(ApiError::unauthorized())
    }
}
async fn get_one(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
) -> Result<Json<Memo>, ApiError> {
    let mut tx = state.pool.begin().await?;
    let mut memo = load(&mut *tx, &identity.account_id, &id).await?;
    memo.locations =
        crate::planner::locations::load(&mut *tx, &identity.account_id, "memo", &id).await?;
    tx.commit().await?;
    Ok(Json(memo))
}
async fn list(
    State(state): State<Shared>,
    identity: Identity,
    Query(query): Query<MemoQuery>,
) -> Result<Json<MemoList>, ApiError> {
    if !query.valid() {
        return Err(ApiError::invalid("invalid memo query"));
    }
    let q = query.q.trim().to_lowercase();
    let mut tx = state.pool.begin().await?;
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM memos WHERE account_id=? AND instr(lower(title || ' ' || text),?)>0",
    )
    .bind(&identity.account_id)
    .bind(&q)
    .fetch_one(&mut *tx)
    .await?;
    // 列表仅返回摘要，复制和详情逐次读取完整原文并重新鉴权。
    let rows: Vec<Row> = sqlx::query_as("SELECT id,title,substr(text,1,200) AS text,version,created_at,updated_at FROM memos WHERE account_id=? AND instr(lower(title || ' ' || text),?)>0 ORDER BY updated_at DESC,id DESC LIMIT ? OFFSET ?")
        .bind(&identity.account_id).bind(&q).bind(query.page_size).bind(i64::from(query.page-1)*i64::from(query.page_size)).fetch_all(&mut *tx).await?;
    tx.commit().await?;
    let items = rows
        .into_iter()
        .map(|row| {
            Ok(MemoSummary {
                id: row.id,
                version: row.version,
                title: row.title,
                preview: row.text,
                created_at: time(row.created_at)?,
                updated_at: time(row.updated_at)?,
            })
        })
        .collect::<Result<_, ApiError>>()?;
    Ok(Json(MemoList {
        items,
        total: u64::try_from(total).map_err(|_| ApiError::internal("invalid memo count"))?,
        page: query.page,
        page_size: query.page_size,
    }))
}
async fn create(
    State(state): State<Shared>,
    identity: Identity,
    body: Bytes,
) -> Result<Json<Memo>, ApiError> {
    let input: MemoCreate = read_json(&body)?;
    if !input.valid() {
        return Err(ApiError::invalid("invalid memo content"));
    }
    let content = input
        .content
        .cleaned()
        .ok_or_else(|| ApiError::invalid("invalid memo content"))?;
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    revalidate(&mut tx, &identity).await?;
    let fingerprint = assemble_protocol::primitives::to_hex(&Sha256::digest(
        serde_json::to_vec(&content)
            .map_err(|_| ApiError::internal("memo content could not be encoded"))?,
    ));
    let prior: Option<(String, String)> = sqlx::query_as(
        "SELECT memo_id,fingerprint FROM memo_requests WHERE account_id=? AND request_id=?",
    )
    .bind(&identity.account_id)
    .bind(&input.request_id)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some((id, previous_fingerprint)) = prior {
        if previous_fingerprint != fingerprint {
            return Err(ApiError::invalid(
                "the memo request identifier was used for different content",
            ));
        }
        return Ok(Json(load(&mut *tx, &identity.account_id, &id).await?));
    }
    let id = crate::secrets::random_id()
        .map_err(|_| ApiError::internal("memo identifier unavailable"))?;
    let now = Timestamp::now().as_millis();
    sqlx::query("INSERT INTO memos(id,account_id,title,text,version,created_at,updated_at) VALUES(?,?,?,?,1,?,?)")
        .bind(&id).bind(&identity.account_id).bind(&content.title).bind(&content.text).bind(now).bind(now).execute(&mut *tx).await?;
    sqlx::query(
        "INSERT INTO memo_requests(account_id,request_id,memo_id,fingerprint) VALUES(?,?,?,?)",
    )
    .bind(&identity.account_id)
    .bind(&input.request_id)
    .bind(&id)
    .bind(fingerprint)
    .execute(&mut *tx)
    .await?;
    let memo = load(&mut *tx, &identity.account_id, &id).await?;
    tx.commit().await?;
    Ok(Json(memo))
}
async fn update(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<Memo>, ApiError> {
    let input: MemoUpdate = read_json(&body)?;
    let content = input
        .content
        .cleaned()
        .ok_or_else(|| ApiError::invalid("invalid memo content"))?;
    if input.version < 1 || input.version == i64::MAX {
        return Err(ApiError::invalid("invalid memo version"));
    }
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    revalidate(&mut tx, &identity).await?;
    let previous = load(&mut *tx, &identity.account_id, &id).await?;
    if previous.version != input.version {
        return Err(crate::planner::version_conflict());
    }
    sqlx::query("UPDATE memos SET title=?,text=?,version=version+1,updated_at=? WHERE account_id=? AND id=?")
        .bind(content.title).bind(content.text).bind(Timestamp::now().as_millis()).bind(&identity.account_id).bind(&id).execute(&mut *tx).await?;
    let memo = load(&mut *tx, &identity.account_id, &id).await?;
    tx.commit().await?;
    Ok(Json(memo))
}
async fn remove(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    let input: Versioned = read_json(&body)?;
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    revalidate(&mut tx, &identity).await?;
    let previous = load(&mut *tx, &identity.account_id, &id).await?;
    if previous.version != input.version {
        return Err(crate::planner::version_conflict());
    }
    sqlx::query("DELETE FROM memos WHERE account_id=? AND id=?")
        .bind(&identity.account_id)
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
