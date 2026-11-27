use super::auth::Principal;
use super::{auth, store};
use crate::error::ApiError;
use assemble_protocol::media::{
    MediaCollection, MediaCollectionCreate, MediaCollectionList, MediaCollectionUpdate, MediaEntry,
    MediaGenreCount, MediaMonthCount, MediaScope, MediaStatistics, MediaStatus, MediaType,
    MediaYearCount,
};
use sqlx::{Sqlite, SqlitePool, Transaction};
use std::collections::{BTreeMap, BTreeSet};

#[derive(sqlx::FromRow)]
struct Row {
    id: String,
    version: i64,
    title: String,
    description: String,
    cover_entry_id: Option<String>,
    created_at: i64,
    updated_at: i64,
}
pub(super) async fn get(
    pool: &SqlitePool,
    principal: Option<&Principal>,
    id: &str,
) -> Result<MediaCollection, ApiError> {
    store::check_id(id)?;
    let row: Row = sqlx::query_as("SELECT * FROM media_collections WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let data:Vec<String>=sqlx::query_scalar("SELECT e.data FROM media_collection_entries r JOIN media_entries e ON e.id = r.entry_id WHERE r.collection_id = ? AND e.scope = 'shared' ORDER BY r.position").bind(id).fetch_all(pool).await?;
    let items: Vec<MediaEntry> = data
        .iter()
        .map(|s| super::decode(s).map(|e| store::project(e, principal)))
        .collect::<Result<_, _>>()?;
    Ok(MediaCollection {
        id: row.id,
        version: u64::try_from(row.version)
            .map_err(|_| ApiError::internal("invalid media version"))?,
        title: row.title,
        description: row.description,
        poster: row
            .cover_entry_id
            .as_ref()
            .and_then(|id| items.iter().find(|entry| &entry.id == id))
            .and_then(|entry| entry.poster.clone())
            .or_else(|| items.iter().find_map(|entry| entry.poster.clone())),
        items,
        created_at: super::timestamp(row.created_at),
        updated_at: super::timestamp(row.updated_at),
    })
}
pub(super) async fn list(
    pool: &SqlitePool,
    principal: Option<&Principal>,
    page: u32,
    page_size: u32,
) -> Result<MediaCollectionList, ApiError> {
    if page == 0 || page > 100_000 || page_size == 0 || page_size > 50 {
        return Err(ApiError::invalid("invalid collection pagination"));
    }
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM media_collections")
        .fetch_one(pool)
        .await?;
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM media_collections ORDER BY updated_at DESC,id LIMIT ? OFFSET ?",
    )
    .bind(i64::from(page_size))
    .bind(i64::from(page - 1) * i64::from(page_size))
    .fetch_all(pool)
    .await?;
    let mut items = Vec::new();
    for id in ids {
        items.push(get(pool, principal, &id).await?);
    }
    Ok(MediaCollectionList {
        items,
        total: u64::try_from(total).map_err(|_| ApiError::internal("invalid collection count"))?,
        page,
        page_size,
    })
}
fn validate(title: &str, description: &str, entry_ids: &[String]) -> Result<(), ApiError> {
    if title.trim().is_empty()
        || title.chars().count() > 200
        || description.chars().count() > 5000
        || entry_ids.len() > 200
        || title.contains('\0')
        || description.contains('\0')
        || entry_ids.iter().collect::<BTreeSet<_>>().len() != entry_ids.len()
    {
        return Err(ApiError::invalid("invalid media collection"));
    }
    Ok(())
}
async fn replace_entries(
    tx: &mut Transaction<'_, Sqlite>,
    id: &str,
    entries: &[String],
) -> Result<(), ApiError> {
    for entry in entries {
        store::load(&mut **tx, MediaScope::Shared, None, entry).await?;
    }
    sqlx::query("DELETE FROM media_collection_entries WHERE collection_id = ?")
        .bind(id)
        .execute(&mut **tx)
        .await?;
    for (position, entry) in entries.iter().enumerate() {
        sqlx::query(
            "INSERT INTO media_collection_entries(collection_id,entry_id,position) VALUES(?,?,?)",
        )
        .bind(id)
        .bind(entry)
        .bind(
            i64::try_from(position)
                .map_err(|_| ApiError::invalid("too many collection entries"))?,
        )
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}
pub(super) async fn create(
    pool: &SqlitePool,
    principal: &Principal,
    input: MediaCollectionCreate,
) -> Result<MediaCollection, ApiError> {
    validate(&input.title, &input.description, &input.entry_ids)?;
    let hash = store::fingerprint(&(&input.title, &input.description, &input.entry_ids))?;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    auth::revalidate(&mut tx, principal, MediaScope::Shared).await?;
    if let Some(id) = store::replay(
        &mut tx,
        principal,
        &input.request_id,
        "collection:create",
        &hash,
    )
    .await?
    {
        drop(tx);
        return get(pool, Some(principal), &id).await;
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM media_collections")
        .fetch_one(&mut *tx)
        .await?;
    if count >= 1000 {
        return Err(ApiError::conflict(
            "QUOTA_EXCEEDED",
            "collection quota exceeded",
        ));
    }
    let id = super::id()?;
    let now = super::now();
    sqlx::query("INSERT INTO media_collections(id,version,title,description,created_at,updated_at) VALUES(?,1,?,?,?,?)").bind(&id).bind(&input.title).bind(&input.description).bind(now).bind(now).execute(&mut *tx).await?;
    replace_entries(&mut tx, &id, &input.entry_ids).await?;
    store::remember(
        &mut tx,
        principal,
        &input.request_id,
        "collection:create",
        &hash,
        &id,
    )
    .await?;
    tx.commit().await?;
    get(pool, Some(principal), &id).await
}
pub(super) async fn update(
    pool: &SqlitePool,
    principal: &Principal,
    id: &str,
    input: MediaCollectionUpdate,
) -> Result<MediaCollection, ApiError> {
    validate(&input.title, &input.description, &input.entry_ids)?;
    store::check_id(id)?;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    auth::revalidate(&mut tx, principal, MediaScope::Shared).await?;
    let version: i64 = sqlx::query_scalar("SELECT version FROM media_collections WHERE id = ?")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if u64::try_from(version).map_err(|_| ApiError::internal("invalid collection version"))?
        != input.version
    {
        return Err(crate::planner::version_conflict());
    }
    sqlx::query("UPDATE media_collections SET version = version + 1,title = ?,description = ?,updated_at = ? WHERE id = ?").bind(input.title).bind(input.description).bind(super::now()).bind(id).execute(&mut *tx).await?;
    replace_entries(&mut tx, id, &input.entry_ids).await?;
    tx.commit().await?;
    get(pool, Some(principal), id).await
}
pub(super) async fn remove(
    pool: &SqlitePool,
    principal: &Principal,
    id: &str,
    version: u64,
) -> Result<(), ApiError> {
    store::check_id(id)?;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    auth::revalidate(&mut tx, principal, MediaScope::Shared).await?;
    let current: i64 = sqlx::query_scalar("SELECT version FROM media_collections WHERE id = ?")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if u64::try_from(current).map_err(|_| ApiError::internal("invalid collection version"))?
        != version
    {
        return Err(crate::planner::version_conflict());
    }
    sqlx::query("DELETE FROM media_collections WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}
pub(super) async fn statistics(pool: &SqlitePool) -> Result<MediaStatistics, ApiError> {
    let data: Vec<String> = sqlx::query_scalar("SELECT data FROM media_entries")
        .fetch_all(pool)
        .await?;
    let mut stats = MediaStatistics::default();
    let mut years = BTreeMap::new();
    let mut genres = BTreeMap::new();
    let mut months = BTreeMap::new();
    for data in data {
        let entry: MediaEntry = super::decode(&data)?;
        stats.total += 1;
        match entry.content.status {
            MediaStatus::Planned => stats.planned += 1,
            MediaStatus::Watching => stats.watching += 1,
            MediaStatus::Completed => stats.completed += 1,
        }
        match entry.content.media_type {
            MediaType::Movie => stats.movies += 1,
            MediaType::Tv => stats.tv += 1,
        }
        *years.entry(entry.content.year).or_insert(0) += 1;
        for genre in entry.content.genres.into_iter().collect::<BTreeSet<_>>() {
            *genres.entry(genre).or_insert(0) += 1;
        }
        if entry.content.status == MediaStatus::Completed
            && let Some(watched) = entry.content.watched_at
            && let Some(month) = watched.get(..7)
        {
            *months.entry(month.to_owned()).or_insert(0) += 1;
        }
    }
    stats.by_year = years
        .into_iter()
        .rev()
        .map(|(year, count)| MediaYearCount { year, count })
        .collect();
    stats.by_genre = genres
        .into_iter()
        .map(|(genre, count)| MediaGenreCount { genre, count })
        .collect();
    stats.by_month = months
        .into_iter()
        .map(|(month, count)| MediaMonthCount { month, count })
        .collect();
    Ok(stats)
}
