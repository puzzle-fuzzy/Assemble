use super::{METADATA_BUDGET, QUEUE_LIMIT, WORKERS, store};
use crate::{
    admin::{Admin, AdminWrite},
    error::ApiError,
    http::Shared,
};
use assemble_protocol::{
    admin::AdminPage,
    dht::{DhtControl, DhtDetails, DhtMode, DhtObservation, DhtQuery, DhtStatus},
    primitives::Timestamp,
};
use axum::{
    Json,
    extract::{DefaultBodyLimit, Path, Query, State},
    routing::{get, post},
};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;

pub fn routes() -> axum::Router<Shared> {
    axum::Router::new()
        .route("/v1/admin/dht/status", get(status))
        .route("/v1/admin/dht/observations", get(list))
        .route("/v1/admin/dht/observations/{hash}", get(detail))
        .route("/v1/admin/dht/query", post(query))
        .route("/v1/admin/dht/control", post(control))
        .layer(DefaultBodyLimit::max(8192))
}

async fn status(State(state): State<Shared>, _: Admin) -> Result<Json<DhtStatus>, ApiError> {
    let mut statuses = BTreeMap::new();
    for (status, count) in sqlx::query_as::<_, (String, i64)>(
        "SELECT status, count(*) FROM dht_observations GROUP BY status",
    )
    .fetch_all(&state.pool)
    .await?
    {
        statuses.insert(
            status,
            u64::try_from(count).map_err(|_| ApiError::internal("invalid DHT count"))?,
        );
    }
    let (announces, samples, last, stored): (i64, i64, Option<i64>, i64) = sqlx::query_as("SELECT coalesce(sum(announce_count),0), coalesce(sum(sample_count),0), max(last_seen), coalesce(sum(CASE WHEN status = 'verified' THEN length(CAST(files_json AS BLOB)) ELSE 0 END),0) FROM dht_observations").fetch_one(&state.pool).await?;
    Ok(Json(DhtStatus {
        enabled: state.config.dht.enabled,
        mode: store::mode(&state.pool).await?,
        running: state.dht.running.load(Ordering::Relaxed),
        last_error: state.dht.last_error.lock().expect("DHT error lock").clone(),
        total: statuses.values().sum(),
        announces: u64::try_from(announces).unwrap_or(0),
        samples: u64::try_from(samples).unwrap_or(0),
        last_observed_at: last.and_then(Timestamp::from_millis),
        statuses,
        queue_limit: u32::try_from(QUEUE_LIMIT).expect("a fixed queue limit fits"),
        max_records: u64::try_from(state.config.dht.max_records)
            .expect("configuration is positive"),
        workers: u32::try_from(WORKERS).expect("a fixed worker count fits"),
        metadata_bytes: u64::try_from(stored).unwrap_or(0),
        metadata_budget_bytes: u64::try_from(METADATA_BUDGET).expect("a fixed budget fits"),
        counters: state
            .dht
            .counters
            .lock()
            .expect("DHT counters lock")
            .clone(),
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Parameters {
    #[serde(default)]
    q: String,
    #[serde(default = "one")]
    page: u32,
    #[serde(default = "twenty")]
    page_size: u32,
    status: Option<String>,
    source: Option<String>,
}
fn one() -> u32 {
    1
}
fn twenty() -> u32 {
    20
}

async fn list(
    State(state): State<Shared>,
    _: Admin,
    Query(parameters): Query<Parameters>,
) -> Result<Json<AdminPage<DhtObservation>>, ApiError> {
    if parameters.q.len() > 600
        || parameters.page == 0
        || !(1..=100).contains(&parameters.page_size)
    {
        return Err(ApiError::invalid("invalid DHT pagination"));
    }
    let status = parameters.status.unwrap_or_default();
    if !status.is_empty() && store::status(&status).is_err() {
        return Err(ApiError::invalid("invalid metadata status"));
    }
    let source = parameters.source.unwrap_or_default();
    if !["", "announce", "sample", "query"].contains(&source.as_str()) {
        return Err(ApiError::invalid("invalid DHT source"));
    }
    let search = parameters
        .q
        .trim()
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    let pattern = format!("%{search}%");
    let predicate = "WHERE (? = '' OR id LIKE ? ESCAPE '\\' OR name LIKE ? ESCAPE '\\') AND (? = '' OR status = ?) AND (? = '' OR (? = 'announce' AND announce_count > 0) OR (? = 'sample' AND sample_count > 0) OR (? = 'query' AND announce_count = 0 AND sample_count = 0))";
    let mut tx = state.pool.begin().await?;
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT count(*) FROM dht_observations {predicate}"
    )))
    .bind(&search)
    .bind(&pattern)
    .bind(&pattern)
    .bind(&status)
    .bind(&status)
    .bind(&source)
    .bind(&source)
    .bind(&source)
    .bind(&source)
    .fetch_one(&mut *tx)
    .await?;
    let total = u64::try_from(total).map_err(|_| ApiError::internal("invalid DHT count"))?;
    let page =
        u64::from(parameters.page).min(total.div_ceil(u64::from(parameters.page_size)).max(1));
    let rows: Vec<store::Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {} FROM dht_observations {predicate} ORDER BY last_seen DESC, id LIMIT ? OFFSET ?",
        store::COLUMNS
    )))
    .bind(&search)
    .bind(&pattern)
    .bind(&pattern)
    .bind(&status)
    .bind(&status)
    .bind(&source)
    .bind(&source)
    .bind(&source)
    .bind(&source)
    .bind(i64::from(parameters.page_size))
    .bind(
        i64::try_from((page - 1) * u64::from(parameters.page_size))
            .map_err(|_| ApiError::invalid("invalid DHT offset"))?,
    )
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(AdminPage {
        items: rows
            .into_iter()
            .map(store::Row::view)
            .collect::<Result<_, _>>()?,
        total,
        page: u32::try_from(page).map_err(|_| ApiError::invalid("invalid DHT page"))?,
        page_size: parameters.page_size,
    }))
}

async fn detail(
    State(state): State<Shared>,
    _: Admin,
    Path(hash): Path<String>,
) -> Result<Json<DhtDetails>, ApiError> {
    let hash = assemble_dht::hash_text(
        &assemble_dht::parse_hash(&hash).map_err(|_| ApiError::invalid("invalid info hash"))?,
    );
    store::detail(&state.pool, &hash)
        .await?
        .map(Json)
        .ok_or_else(ApiError::not_found)
}

async fn query(
    State(state): State<Shared>,
    _: AdminWrite,
    Json(input): Json<DhtQuery>,
) -> Result<Json<DhtDetails>, ApiError> {
    let hash = assemble_dht::hash_text(
        &assemble_dht::parse_hash(&input.query)
            .map_err(|_| ApiError::invalid("invalid info hash or magnet"))?,
    );
    if !state.config.dht.enabled || store::mode(&state.pool).await? == DhtMode::Stopped {
        return Err(ApiError::conflict("DHT_UNAVAILABLE", "DHT is stopped"));
    }
    // 主动查询不伪造 announce/sample 证据。重试受相同队列和冷却预算限制。
    store::enqueue_query(&state.pool, &hash, state.config.dht.max_records).await?;
    store::detail(&state.pool, &hash)
        .await?
        .map(Json)
        .ok_or_else(ApiError::not_found)
}

async fn control(
    State(state): State<Shared>,
    AdminWrite(actor): AdminWrite,
    Json(input): Json<DhtControl>,
) -> Result<Json<DhtControl>, ApiError> {
    super::set_mode(
        &state.pool,
        state.config.dht.enabled,
        input.mode,
        Some(&actor.account_id),
    )
    .await?;
    Ok(Json(input))
}
