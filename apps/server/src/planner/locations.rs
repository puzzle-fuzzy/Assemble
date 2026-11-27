//! 地址只能来自当前账户真实存在的记录；打开地图前也重新读取这一份资料。
use assemble_protocol::schedule::{Location, RecordLocations};
use axum::{
    Json,
    extract::{Path, State},
};
use sqlx::SqliteExecutor;

use crate::{auth::Identity, error::ApiError, http::Shared};

pub fn routes() -> axum::Router<Shared> {
    axum::Router::new()
        .route(
            "/v1/planner/locations/{resource}/{id}",
            axum::routing::get(get),
        )
        .layer(axum::middleware::map_response(
            |mut response: axum::response::Response| async move {
                response.headers_mut().insert(
                    axum::http::header::CACHE_CONTROL,
                    axum::http::HeaderValue::from_static("no-store"),
                );
                response
            },
        ))
}

pub async fn load(
    executor: impl SqliteExecutor<'_>,
    account: &str,
    resource: &str,
    id: &str,
) -> Result<Vec<Location>, sqlx::Error> {
    let raw: Option<String> = sqlx::query_scalar("SELECT locations FROM record_locations WHERE account_id=? AND resource=? AND resource_id=?")
        .bind(account).bind(resource).bind(id).fetch_optional(executor).await?;
    raw.map_or_else(
        || Ok(Vec::new()),
        |raw| serde_json::from_str(&raw).map_err(|error| sqlx::Error::Decode(Box::new(error))),
    )
}

pub async fn get(
    State(state): State<Shared>,
    identity: Identity,
    Path((resource, id)): Path<(String, String)>,
) -> Result<Json<Vec<Location>>, ApiError> {
    let mut tx = state.pool.begin().await?;
    let locations = match resource.as_str() {
        "schedule" => {
            let entry = super::schedules::load(&mut *tx, &identity.account_id, &id).await?;
            let mut locations = load(&mut *tx, &identity.account_id, &resource, &id).await?;
            if let Some(location) = entry.content().location {
                locations.retain(|value| value.label != location.label);
                locations.push(location);
            }
            locations
        }
        "reminder" | "memo" => {
            let statement = if resource == "reminder" {
                "SELECT EXISTS(SELECT 1 FROM reminders WHERE account_id=? AND id=?)"
            } else {
                "SELECT EXISTS(SELECT 1 FROM memos WHERE account_id=? AND id=?)"
            };
            let exists: bool = sqlx::query_scalar(statement)
                .bind(&identity.account_id)
                .bind(&id)
                .fetch_one(&mut *tx)
                .await?;
            if !exists {
                return Err(ApiError::not_found());
            }
            load(&mut *tx, &identity.account_id, &resource, &id).await?
        }
        _ => return Err(ApiError::not_found()),
    };
    tx.commit().await?;
    Ok(Json(locations))
}

pub async fn listing(
    pool: &sqlx::SqlitePool,
    account: &str,
) -> Result<Vec<RecordLocations>, sqlx::Error> {
    let rows: Vec<(String,String,String)> = sqlx::query_as("SELECT l.resource,l.resource_id,l.locations FROM record_locations l WHERE l.account_id=? AND ((l.resource='schedule' AND EXISTS(SELECT 1 FROM schedules s WHERE s.account_id=l.account_id AND s.id=l.resource_id)) OR (l.resource='reminder' AND EXISTS(SELECT 1 FROM reminders r WHERE r.account_id=l.account_id AND r.id=l.resource_id))) ORDER BY l.resource,l.resource_id")
        .bind(account).fetch_all(pool).await?;
    rows.into_iter()
        .map(|(resource, id, raw)| {
            Ok(RecordLocations {
                resource,
                id,
                locations: serde_json::from_str(&raw)
                    .map_err(|error| sqlx::Error::Decode(Box::new(error)))?,
            })
        })
        .collect()
}
