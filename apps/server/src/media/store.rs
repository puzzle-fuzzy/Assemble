use super::auth::{Principal, revalidate};
use crate::error::ApiError;
use assemble_protocol::media::{
    MediaContent, MediaCounts, MediaCreate, MediaEntry, MediaList, MediaQuery, MediaScope,
    MediaSort, MediaStatus, MediaUpdate,
};
use sqlx::{Sqlite, SqliteExecutor, SqlitePool, Transaction};
use std::collections::BTreeSet;

pub(super) fn validate_content(content: &MediaContent) -> Result<(), ApiError> {
    let bounded = |s: &str, max| s.chars().count() <= max && !s.chars().any(|c| c == '\0');
    if content.title.trim().is_empty()
        || !bounded(&content.title, 300)
        || !bounded(&content.original_title, 300)
        || !bounded(&content.overview, 20_000)
        || content.genres.len() > 30
        || content
            .genres
            .iter()
            .any(|g| g.trim().is_empty() || !bounded(g, 80))
        || content.year.is_some_and(|y| !(1800..=9999).contains(&y))
        || content.episode_count.is_some_and(|v| v == 0 || v > 100_000)
        || content.review.as_ref().is_some_and(|s| !bounded(s, 20_000))
        || content.notes.as_ref().is_some_and(|s| !bounded(s, 20_000))
    {
        return Err(ApiError::invalid("media content exceeds its boundaries"));
    }
    for date in [&content.release_date, &content.watched_at]
        .into_iter()
        .flatten()
    {
        if chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_err() || date.len() != 10 {
            return Err(ApiError::invalid("media dates must use YYYY-MM-DD"));
        }
    }
    if content
        .magnet_uri
        .as_ref()
        .is_some_and(|uri| !valid_magnet(uri))
    {
        return Err(ApiError::invalid("invalid magnet URI"));
    }
    Ok(())
}
fn valid_magnet(value: &str) -> bool {
    if value.len() > 8192 || value.chars().any(char::is_control) {
        return false;
    }
    let Ok(url) = url::Url::parse(value) else {
        return false;
    };
    url.scheme() == "magnet"
        && url.host_str().is_none()
        && url.query_pairs().any(|(key, value)| {
            key == "xt"
                && (value.starts_with("urn:btih:") || value.starts_with("urn:btmh:"))
                && value.len() > 15
        })
}
pub(super) fn check_id(id: &str) -> Result<(), ApiError> {
    if assemble_protocol::primitives::is_valid_id(id) && !matches!(id, "." | "..") {
        Ok(())
    } else {
        Err(ApiError::not_found())
    }
}
pub(super) fn validate_request_id(id: &str) -> Result<(), ApiError> {
    if !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        Ok(())
    } else {
        Err(ApiError::invalid("invalid media request id"))
    }
}
pub(super) fn owner(
    scope: MediaScope,
    principal: Option<&Principal>,
) -> Result<Option<&str>, ApiError> {
    match scope {
        MediaScope::Shared => Ok(None),
        MediaScope::Private => Ok(Some(
            &principal.ok_or_else(ApiError::unauthorized)?.account_id,
        )),
    }
}
pub(super) fn project(mut entry: MediaEntry, principal: Option<&Principal>) -> MediaEntry {
    entry.magnet_available = entry.content.magnet_uri.is_some();
    if entry.scope == MediaScope::Shared {
        if principal.is_none() {
            entry.content.magnet_uri = None;
        }
        if !principal.is_some_and(|p| p.admin) {
            entry.content.notes = None;
        }
    }
    entry
}
pub(super) async fn load<'e>(
    executor: impl SqliteExecutor<'e>,
    scope: MediaScope,
    account: Option<&str>,
    id: &str,
) -> Result<MediaEntry, ApiError> {
    check_id(id)?;
    let data: Option<String> = sqlx::query_scalar(
        "SELECT data FROM media_entries WHERE id = ? AND scope = ? AND owner_id IS ?",
    )
    .bind(id)
    .bind(super::spelling(&scope))
    .bind(account)
    .fetch_optional(executor)
    .await?;
    super::decode(&data.ok_or_else(ApiError::not_found)?)
}
pub(super) async fn list(
    pool: &SqlitePool,
    scope: MediaScope,
    principal: Option<&Principal>,
    query: &MediaQuery,
) -> Result<MediaList, ApiError> {
    let account = owner(scope, principal)?;
    let page = query.page.unwrap_or(1);
    let page_size = query.page_size.unwrap_or(36);
    if page == 0
        || page > 100_000
        || page_size == 0
        || page_size > 100
        || query.q.as_ref().is_some_and(|q| q.chars().count() > 200)
        || query.genre.as_ref().is_some_and(|g| g.chars().count() > 80)
    {
        return Err(ApiError::invalid("invalid media pagination or filter"));
    }
    let data: Vec<String> = sqlx::query_scalar("SELECT data FROM media_entries WHERE scope = ? AND owner_id IS ? ORDER BY updated_at DESC LIMIT 10001")
        .bind(super::spelling(&scope)).bind(account).fetch_all(pool).await?;
    let entries: Vec<MediaEntry> = data
        .iter()
        .map(|s| super::decode(s))
        .collect::<Result<_, _>>()?;
    let mut counts = MediaCounts::default();
    for entry in &entries {
        counts.all += 1;
        match entry.content.status {
            MediaStatus::Planned => counts.planned += 1,
            MediaStatus::Watching => counts.watching += 1,
            MediaStatus::Completed => counts.completed += 1,
        }
    }
    let term = query.q.as_ref().map(|s| s.trim().to_lowercase());
    let mut filtered: Vec<_> = entries
        .into_iter()
        .filter(|e| {
            query.media_type.is_none_or(|v| e.content.media_type == v)
                && query.status.is_none_or(|v| e.content.status == v)
                && query.year.is_none_or(|v| e.content.year == Some(v))
                && query
                    .genre
                    .as_ref()
                    .is_none_or(|v| e.content.genres.contains(v))
                && term.as_ref().is_none_or(|q| {
                    e.content.title.to_lowercase().contains(q)
                        || e.content.original_title.to_lowercase().contains(q)
                })
        })
        .collect();
    filtered.sort_by(|a, b| {
        match query.sort.unwrap_or(MediaSort::Updated) {
            MediaSort::Updated => b.updated_at.cmp(&a.updated_at),
            MediaSort::Title => a.content.title.cmp(&b.content.title),
            MediaSort::Year => b.content.year.cmp(&a.content.year),
            MediaSort::Watched => b.content.watched_at.cmp(&a.content.watched_at),
        }
        .then_with(|| a.id.cmp(&b.id))
    });
    let total = filtered.len() as u64;
    let items = filtered
        .into_iter()
        .skip(((page - 1) * page_size) as usize)
        .take(page_size as usize)
        .map(|e| project(e, principal))
        .collect();
    Ok(MediaList {
        items,
        total,
        page,
        page_size,
        counts,
    })
}
pub(super) async fn replay(
    tx: &mut Transaction<'_, Sqlite>,
    principal: &Principal,
    request_id: &str,
    operation: &str,
    fingerprint: &str,
) -> Result<Option<String>, ApiError> {
    validate_request_id(request_id)?;
    let row: Option<(String,String,String)> = sqlx::query_as("SELECT operation, fingerprint, resource_id FROM media_mutations WHERE actor_id = ? AND request_id = ?").bind(&principal.account_id).bind(request_id).fetch_optional(&mut **tx).await?;
    match row {
        Some((op, hash, id)) if op == operation && hash == fingerprint => Ok(Some(id)),
        Some(_) => Err(ApiError::conflict(
            "IDEMPOTENCY_CONFLICT",
            "media request id was already used with different content",
        )),
        None => Ok(None),
    }
}
pub(super) async fn remember(
    tx: &mut Transaction<'_, Sqlite>,
    principal: &Principal,
    request_id: &str,
    operation: &str,
    fingerprint: &str,
    id: &str,
) -> Result<(), ApiError> {
    sqlx::query("INSERT INTO media_mutations(actor_id,request_id,operation,fingerprint,resource_id) VALUES(?,?,?,?,?)").bind(&principal.account_id).bind(request_id).bind(operation).bind(fingerprint).bind(id).execute(&mut **tx).await?;
    Ok(())
}
pub(super) fn fingerprint<T: serde::Serialize>(value: &T) -> Result<String, ApiError> {
    Ok(crate::secrets::token_digest(&super::encode(value)?))
}
pub(super) async fn insert(
    tx: &mut Transaction<'_, Sqlite>,
    principal: &Principal,
    entry: &MediaEntry,
) -> Result<(), ApiError> {
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM media_entries WHERE scope = ? AND owner_id IS ?")
            .bind(super::spelling(&entry.scope))
            .bind(owner(entry.scope, Some(principal))?)
            .fetch_one(&mut **tx)
            .await?;
    if count >= 10_000 {
        return Err(ApiError::conflict(
            "QUOTA_EXCEEDED",
            "media entry quota exceeded",
        ));
    }
    sqlx::query("INSERT INTO media_entries(id,scope,owner_id,version,data,created_at,updated_at) VALUES(?,?,?,?,?,?,?)")
        .bind(&entry.id).bind(super::spelling(&entry.scope)).bind(owner(entry.scope,Some(principal))?).bind(1_i64).bind(super::encode(entry)?).bind(super::now()).bind(super::now()).execute(&mut **tx).await?;
    let key = super::auth::owner_key(entry.scope, principal);
    for identity in &entry.identities {
        sqlx::query("INSERT INTO media_identities(entry_id,scope_key,provider,media_type,external_id) VALUES(?,?,?,?,?)")
            .bind(&entry.id).bind(&key).bind(super::spelling(&identity.provider)).bind(super::spelling(&identity.media_type)).bind(&identity.external_id).execute(&mut **tx).await?;
    }
    Ok(())
}
pub(super) fn new_entry(scope: MediaScope, content: MediaContent) -> Result<MediaEntry, ApiError> {
    let at = super::timestamp(super::now());
    Ok(MediaEntry {
        id: super::id()?,
        version: 1,
        scope,
        magnet_available: content.magnet_uri.is_some(),
        content,
        cast: vec![],
        identities: vec![],
        poster: None,
        created_at: at.clone(),
        updated_at: at,
    })
}
pub(super) async fn create(
    pool: &SqlitePool,
    scope: MediaScope,
    principal: &Principal,
    input: MediaCreate,
) -> Result<MediaEntry, ApiError> {
    validate_content(&input.content)?;
    let operation = format!("create:{}", super::spelling(&scope));
    let hash = fingerprint(&input.content)?;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    revalidate(&mut tx, principal, scope).await?;
    if let Some(id) = replay(&mut tx, principal, &input.request_id, &operation, &hash).await? {
        return load(&mut *tx, scope, owner(scope, Some(principal))?, &id).await;
    }
    let mut content = input.content;
    content.genres = content
        .genres
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let entry = new_entry(scope, content)?;
    insert(&mut tx, principal, &entry).await?;
    remember(
        &mut tx,
        principal,
        &input.request_id,
        &operation,
        &hash,
        &entry.id,
    )
    .await?;
    tx.commit().await?;
    Ok(entry)
}
pub(super) async fn save(
    tx: &mut Transaction<'_, Sqlite>,
    mut entry: MediaEntry,
) -> Result<MediaEntry, ApiError> {
    let before = entry.version;
    entry.version = entry
        .version
        .checked_add(1)
        .ok_or_else(|| ApiError::invalid("media version overflow"))?;
    entry.updated_at = super::timestamp(super::now());
    entry.magnet_available = entry.content.magnet_uri.is_some();
    let affected = sqlx::query("UPDATE media_entries SET version = ?, data = ?, updated_at = ? WHERE id = ? AND version = ?")
        .bind(i64::try_from(entry.version).map_err(|_|ApiError::invalid("media version overflow"))?).bind(super::encode(&entry)?).bind(super::now()).bind(&entry.id).bind(i64::try_from(before).map_err(|_| ApiError::invalid("invalid media version"))?).execute(&mut **tx).await?.rows_affected();
    if affected == 0 {
        return Err(crate::planner::version_conflict());
    }
    Ok(entry)
}
pub(super) async fn update(
    pool: &SqlitePool,
    scope: MediaScope,
    principal: &Principal,
    id: &str,
    input: MediaUpdate,
) -> Result<MediaEntry, ApiError> {
    validate_content(&input.content)?;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    revalidate(&mut tx, principal, scope).await?;
    let mut entry = load(&mut *tx, scope, owner(scope, Some(principal))?, id).await?;
    if entry.version != input.version {
        return Err(crate::planner::version_conflict());
    }
    entry.content = input.content;
    let entry = save(&mut tx, entry).await?;
    tx.commit().await?;
    Ok(entry)
}
pub(super) async fn remove(
    pool: &SqlitePool,
    scope: MediaScope,
    principal: &Principal,
    id: &str,
    version: u64,
) -> Result<(), ApiError> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    revalidate(&mut tx, principal, scope).await?;
    let entry = load(&mut *tx, scope, owner(scope, Some(principal))?, id).await?;
    if entry.version != version {
        return Err(crate::planner::version_conflict());
    }
    sqlx::query("DELETE FROM media_entries WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}
