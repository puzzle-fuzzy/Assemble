use super::{assets, auth, collections, jobs, store};
use crate::{
    error::ApiError,
    http::{Shared, read_json},
};
use assemble_protocol::media::{
    MediaAccount, MediaActorCredits, MediaCandidate, MediaCapabilities, MediaCollection,
    MediaCollectionList, MediaEntry, MediaGallery, MediaIdentity, MediaImage, MediaImport,
    MediaItem, MediaItems, MediaJob, MediaList, MediaPersonIdentity, MediaPoster, MediaProvider,
    MediaQuery, MediaScope, MediaSession, MediaStatistics, MediaSync, MediaType, MediaVersion,
};
use axum::extract::{DefaultBodyLimit, Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{get, patch, post};
use axum::{Extension, Json, Router};
use bytes::Bytes;
use serde::Deserialize;

pub fn routes() -> Router<Shared> {
    let public = Router::new()
        .route("/entries", get(public_list))
        .route("/entries/{id}", get(public_detail))
        .route("/entries/{id}/gallery", get(public_gallery));
    let shared = Router::new()
        .route("/entries", get(list))
        .route("/entries/{id}", get(detail))
        .route("/entries/{id}/gallery", get(gallery))
        .layer(Extension(MediaScope::Shared));
    let private = Router::new()
        .route("/entries", get(list).post(create))
        .route("/entries/{id}", get(detail).put(update).delete(remove))
        .route("/entries/{id}/gallery", get(gallery))
        .route("/entries/{id}/gallery/sync", post(sync))
        .route("/entries/{id}/poster", patch(poster))
        .route("/imports", post(import))
        .route("/jobs/{id}", get(job))
        .layer(Extension(MediaScope::Private));
    let admin = Router::new()
        .route("/entries", get(admin_list).post(create))
        .route(
            "/entries/{id}",
            get(admin_detail).put(update).delete(remove),
        )
        .route("/entries/{id}/gallery", get(admin_gallery))
        .route("/entries/{id}/gallery/sync", post(sync))
        .route("/entries/{id}/poster", patch(poster))
        .route("/imports", post(import))
        .route("/jobs/{id}", get(admin_job))
        .route("/collections", post(collection_create))
        .route(
            "/collections/{id}",
            axum::routing::put(collection_update).delete(collection_remove),
        )
        .route("/statistics", get(statistics))
        .layer(Extension(MediaScope::Shared));
    Router::new()
        .nest("/v1/media/shared", shared)
        .nest("/v1/media/public", public)
        .nest("/v1/media/private", private)
        .nest("/v1/admin/media", admin)
        .route("/v1/media/session", get(session))
        .route("/v1/media/capabilities", get(capabilities))
        .route("/v1/media/search", get(search))
        .route("/v1/media/catalog/details", get(catalog))
        .route(
            "/v1/media/catalog/actors/{provider}/{external_id}",
            get(actor),
        )
        .route("/v1/media/assets/{id}/content", get(content))
        .route("/v1/media/collections", get(collection_list))
        .route("/v1/media/collections/{id}", get(collection_detail))
        .layer(DefaultBodyLimit::max(128 * 1024))
        .layer(middleware::from_fn(private_response))
}
async fn public_list(
    State(state): State<Shared>,
    headers: HeaderMap,
    Query(query): Query<MediaQuery>,
) -> Result<Json<MediaList>, ApiError> {
    let principal = auth::optional(&state, &headers).await?;
    Ok(Json(
        store::list_visible(&state.pool, principal.as_ref(), &query, false).await?,
    ))
}
async fn public_detail(
    State(state): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<MediaItem<MediaEntry>>, ApiError> {
    let principal = auth::optional(&state, &headers).await?;
    Ok(Json(MediaItem {
        item: store::public_entry(&state.pool, &id, principal.as_ref()).await?,
    }))
}
async fn public_gallery(
    State(state): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<MediaGallery>, ApiError> {
    let principal = auth::optional(&state, &headers).await?;
    let entry = store::public_entry(&state.pool, &id, principal.as_ref()).await?;
    Ok(Json(assets::gallery_for_entry(&state.pool, &entry).await?))
}
async fn admin_list(
    State(state): State<Shared>,
    headers: HeaderMap,
    Query(query): Query<MediaQuery>,
) -> Result<Json<MediaList>, ApiError> {
    let principal = auth::required(&state, &headers).await?;
    auth::admin_read(&principal, &state, &headers)?;
    Ok(Json(
        store::list_visible(&state.pool, Some(&principal), &query, true).await?,
    ))
}
async fn admin_detail(
    State(state): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<MediaItem<MediaEntry>>, ApiError> {
    let principal = auth::required(&state, &headers).await?;
    auth::admin_read(&principal, &state, &headers)?;
    Ok(Json(MediaItem {
        item: store::project(store::load_any(&state.pool, &id).await?, Some(&principal)),
    }))
}
async fn admin_gallery(
    State(state): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<MediaGallery>, ApiError> {
    let principal = auth::required(&state, &headers).await?;
    auth::admin_read(&principal, &state, &headers)?;
    let entry = store::load_any(&state.pool, &id).await?;
    Ok(Json(assets::gallery_for_entry(&state.pool, &entry).await?))
}
async fn admin_job(
    State(state): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<MediaJob>, ApiError> {
    let mut principal = auth::required(&state, &headers).await?;
    auth::admin_read(&principal, &state, &headers)?;
    store::check_id(&id)?;
    let (key, data): (String, String) =
        sqlx::query_as("SELECT owner_key, data FROM media_jobs WHERE id = ?")
            .bind(&id)
            .fetch_optional(&state.pool)
            .await?
            .ok_or_else(ApiError::not_found)?;
    let job: MediaJob = super::decode(&data)?;
    if job.scope == MediaScope::Private {
        principal.target_owner = Some(key);
    }
    Ok(Json(
        jobs::get(&state.pool, job.scope, &principal, &id).await?,
    ))
}
async fn mutation_target(
    state: &Shared,
    headers: &HeaderMap,
    scope: MediaScope,
    mut principal: auth::Principal,
    id: &str,
) -> Result<(MediaScope, auth::Principal), ApiError> {
    if scope == MediaScope::Shared {
        auth::admin_read(&principal, state, headers)?;
        let entry = store::load_any(&state.pool, id).await?;
        if entry.scope == MediaScope::Private {
            principal.target_owner = Some(
                entry
                    .owner
                    .ok_or_else(|| ApiError::internal("private entry owner missing"))?
                    .id,
            );
        }
        return Ok((entry.scope, principal));
    }
    Ok((scope, principal))
}
async fn private_response(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        "cross-origin-resource-policy",
        HeaderValue::from_static("same-origin"),
    );
    response
}
async fn session(
    State(state): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<MediaSession>, ApiError> {
    let session = crate::auth::web_session(
        &state.pool,
        &axum_extra::extract::CookieJar::from_headers(&headers),
    )
    .await;
    let account = if let Some(session) = session {
        let avatar_url: Option<String> =
            sqlx::query_scalar("SELECT avatar_url FROM accounts WHERE id = ?")
                .bind(&session.account_id)
                .fetch_one(&state.pool)
                .await?;
        Some(MediaAccount {
            id: session.account_id,
            display_name: session.display_name,
            avatar_url,
        })
    } else {
        None
    };
    Ok(Json(MediaSession { account }))
}
async fn capabilities(
    State(state): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<MediaCapabilities>, ApiError> {
    let principal = auth::optional(&state, &headers).await?;
    Ok(Json(MediaCapabilities {
        providers: state.media.providers.capabilities(),
        can_maintain_shared: principal
            .as_ref()
            .is_some_and(|p| p.admin && p.credential_kind == "web"),
        can_maintain_private: principal.is_some(),
    }))
}
async fn list(
    State(state): State<Shared>,
    Extension(scope): Extension<MediaScope>,
    headers: HeaderMap,
    Query(query): Query<MediaQuery>,
) -> Result<Json<MediaList>, ApiError> {
    let principal = auth::optional(&state, &headers).await?;
    Ok(Json(
        store::list(&state.pool, scope, principal.as_ref(), &query).await?,
    ))
}
async fn detail(
    State(state): State<Shared>,
    Extension(scope): Extension<MediaScope>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<MediaItem<MediaEntry>>, ApiError> {
    let principal = auth::optional(&state, &headers).await?;
    let entry = store::load(
        &state.pool,
        scope,
        store::owner(scope, principal.as_ref())?,
        &id,
    )
    .await?;
    Ok(Json(MediaItem {
        item: store::project(entry, principal.as_ref()),
    }))
}
async fn create(
    State(state): State<Shared>,
    Extension(scope): Extension<MediaScope>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<MediaItem<MediaEntry>>, ApiError> {
    let principal = auth::writer(&state, &headers, scope).await?;
    let entry = store::create(&state.pool, scope, &principal, read_json(&body)?).await?;
    Ok(Json(MediaItem { item: entry }))
}
async fn update(
    State(state): State<Shared>,
    Extension(scope): Extension<MediaScope>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<MediaItem<MediaEntry>>, ApiError> {
    let principal = auth::writer(&state, &headers, scope).await?;
    let (scope, principal) = mutation_target(&state, &headers, scope, principal, &id).await?;
    let entry = store::update(&state.pool, scope, &principal, &id, read_json(&body)?).await?;
    Ok(Json(MediaItem { item: entry }))
}
async fn remove(
    State(state): State<Shared>,
    Extension(scope): Extension<MediaScope>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<serde_json::Value>, ApiError> {
    let principal = auth::writer(&state, &headers, scope).await?;
    let (scope, principal) = mutation_target(&state, &headers, scope, principal, &id).await?;
    let input: MediaVersion = read_json(&body)?;
    store::remove(&state.pool, scope, &principal, &id, input.version).await?;
    Ok(Json(serde_json::json!({"ok":true})))
}
async fn gallery(
    State(state): State<Shared>,
    Extension(scope): Extension<MediaScope>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<MediaGallery>, ApiError> {
    let principal = auth::optional(&state, &headers).await?;
    Ok(Json(
        assets::gallery(&state.pool, scope, principal.as_ref(), &id).await?,
    ))
}
async fn poster(
    State(state): State<Shared>,
    Extension(scope): Extension<MediaScope>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<MediaItem<MediaEntry>>, ApiError> {
    let principal = auth::writer(&state, &headers, scope).await?;
    let (scope, principal) = mutation_target(&state, &headers, scope, principal, &id).await?;
    let input: MediaPoster = read_json(&body)?;
    store::check_id(&input.asset_id)?;
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    auth::revalidate(&mut tx, &principal, scope).await?;
    let mut entry =
        store::load(&mut *tx, scope, store::owner(scope, Some(&principal))?, &id).await?;
    if entry.version != input.version {
        return Err(crate::planner::version_conflict());
    }
    let row:Option<(i64,i64,String)>=sqlx::query_as("SELECT a.width,a.height,a.content_type FROM media_assets a JOIN media_entry_assets r ON r.asset_id = a.id WHERE r.entry_id = ? AND a.id = ? AND r.role = 'gallery'").bind(&id).bind(&input.asset_id).fetch_optional(&mut *tx).await?;
    let (width, height, content_type) = row.ok_or_else(ApiError::not_found)?;
    entry.poster = Some(MediaImage {
        asset_id: input.asset_id,
        width: u32::try_from(width).map_err(|_| ApiError::internal("invalid stored image"))?,
        height: u32::try_from(height).map_err(|_| ApiError::internal("invalid stored image"))?,
        content_type,
    });
    let entry = store::save(&mut tx, entry).await?;
    store::audit(&mut tx, &principal, "media.poster_changed", &entry).await?;
    tx.commit().await?;
    Ok(Json(MediaItem { item: entry }))
}
async fn import(
    State(state): State<Shared>,
    Extension(scope): Extension<MediaScope>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<MediaJob>, ApiError> {
    let principal = auth::writer(&state, &headers, scope).await?;
    let input: MediaImport = read_json(&body)?;
    Ok(Json(
        jobs::queue(
            &state,
            scope,
            &principal,
            &input.request_id,
            jobs::Input::Import {
                identity: input.identity,
            },
        )
        .await?,
    ))
}
async fn sync(
    State(state): State<Shared>,
    Extension(scope): Extension<MediaScope>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<MediaJob>, ApiError> {
    let principal = auth::writer(&state, &headers, scope).await?;
    let (scope, principal) = mutation_target(&state, &headers, scope, principal, &id).await?;
    let input: MediaSync = read_json(&body)?;
    let entry = store::load(
        &state.pool,
        scope,
        store::owner(scope, Some(&principal))?,
        &id,
    )
    .await?;
    let identity = entry.identities.first().cloned().ok_or_else(|| {
        ApiError::conflict("MEDIA_SOURCE_REQUIRED", "this entry has no metadata source")
    })?;
    Ok(Json(
        jobs::queue(
            &state,
            scope,
            &principal,
            &input.request_id,
            jobs::Input::Gallery {
                entry_id: id,
                version: input.version,
                identity,
            },
        )
        .await?,
    ))
}
async fn job(
    State(state): State<Shared>,
    Extension(scope): Extension<MediaScope>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<MediaJob>, ApiError> {
    let principal = auth::required(&state, &headers).await?;
    if scope == MediaScope::Shared {
        auth::admin_read(&principal, &state, &headers)?;
    }
    Ok(Json(jobs::get(&state.pool, scope, &principal, &id).await?))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Search {
    provider: MediaProvider,
    media_type: MediaType,
    q: String,
}
async fn search(
    State(state): State<Shared>,
    headers: HeaderMap,
    Query(query): Query<Search>,
) -> Result<Json<MediaItems<MediaCandidate>>, ApiError> {
    auth::required(&state, &headers).await?;
    Ok(Json(MediaItems {
        items: state
            .media
            .providers
            .search(query.provider, query.media_type, &query.q)
            .await?,
    }))
}
async fn catalog(
    State(state): State<Shared>,
    Query(identity): Query<MediaIdentity>,
    headers: HeaderMap,
) -> Result<Json<MediaItem<MediaCandidate>>, ApiError> {
    let item = state
        .media
        .catalog
        .details(&state, &headers, &identity)
        .await?;
    Ok(Json(MediaItem { item }))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActorQuery {
    media_type: MediaType,
    page: Option<u32>,
    page_size: Option<u32>,
}
async fn actor(
    State(state): State<Shared>,
    Path((provider, external_id)): Path<(MediaProvider, String)>,
    Query(query): Query<ActorQuery>,
    headers: HeaderMap,
) -> Result<Json<MediaActorCredits>, ApiError> {
    Ok(Json(
        state
            .media
            .catalog
            .actor(
                &state,
                &headers,
                &MediaPersonIdentity {
                    provider,
                    external_id,
                },
                query.media_type,
                query.page.unwrap_or(1),
                query.page_size.unwrap_or(24),
            )
            .await?,
    ))
}
#[derive(Deserialize)]
struct ImageQuery {
    variant: Option<String>,
}
async fn content(
    State(state): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<ImageQuery>,
) -> Result<Response, ApiError> {
    let mut principal = auth::optional(&state, &headers).await?;
    if let Some(principal) = principal.as_mut().filter(|p| p.admin) {
        // 普通图片请求仍按公开引用/本人判断；只有同源管理访问可以扩展到所有用户。
        if auth::admin_read(principal, &state, &headers).is_err() {
            principal.admin = false;
        }
    }
    assets::content(
        &state,
        principal.as_ref(),
        &id,
        query.variant.as_deref().unwrap_or("original"),
    )
    .await
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CollectionQuery {
    page: Option<u32>,
    page_size: Option<u32>,
}
async fn collection_list(
    State(state): State<Shared>,
    headers: HeaderMap,
    Query(query): Query<CollectionQuery>,
) -> Result<Json<MediaCollectionList>, ApiError> {
    let principal = auth::optional(&state, &headers).await?;
    Ok(Json(
        collections::list(
            &state.pool,
            principal.as_ref(),
            query.page.unwrap_or(1),
            query.page_size.unwrap_or(12),
        )
        .await?,
    ))
}
async fn collection_detail(
    State(state): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<MediaItem<MediaCollection>>, ApiError> {
    let principal = auth::optional(&state, &headers).await?;
    Ok(Json(MediaItem {
        item: collections::get(&state.pool, principal.as_ref(), &id).await?,
    }))
}
async fn collection_create(
    State(state): State<Shared>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<MediaItem<MediaCollection>>, ApiError> {
    let principal = auth::writer(&state, &headers, MediaScope::Shared).await?;
    Ok(Json(MediaItem {
        item: collections::create(&state.pool, &principal, read_json(&body)?).await?,
    }))
}
async fn collection_update(
    State(state): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<MediaItem<MediaCollection>>, ApiError> {
    let principal = auth::writer(&state, &headers, MediaScope::Shared).await?;
    Ok(Json(MediaItem {
        item: collections::update(&state.pool, &principal, &id, read_json(&body)?).await?,
    }))
}
async fn collection_remove(
    State(state): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<serde_json::Value>, ApiError> {
    let principal = auth::writer(&state, &headers, MediaScope::Shared).await?;
    let input: MediaVersion = read_json(&body)?;
    collections::remove(&state.pool, &principal, &id, input.version).await?;
    Ok(Json(serde_json::json!({"ok":true})))
}
async fn statistics(
    State(state): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<MediaStatistics>, ApiError> {
    let principal = auth::required(&state, &headers).await?;
    auth::admin_read(&principal, &state, &headers)?;
    Ok(Json(collections::statistics(&state.pool).await?))
}
