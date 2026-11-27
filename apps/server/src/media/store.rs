use super::auth::{Principal, revalidate};
use crate::error::ApiError;
use assemble_protocol::media::{
    MediaAccount, MediaContent, MediaCounts, MediaCreate, MediaEntry, MediaList, MediaQuery,
    MediaScope, MediaSort, MediaStatus, MediaUpdate,
};
use sqlx::{QueryBuilder, Sqlite, SqliteExecutor, SqlitePool, Transaction};
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
        MediaScope::Private => {
            let principal = principal.ok_or_else(ApiError::unauthorized)?;
            Ok(Some(
                principal
                    .target_owner
                    .as_deref()
                    .unwrap_or(&principal.account_id),
            ))
        }
    }
}
pub(super) fn project(mut entry: MediaEntry, principal: Option<&Principal>) -> MediaEntry {
    entry.magnet_available = entry.content.magnet_uri.is_some();
    if principal.is_none() {
        entry.content.magnet_uri = None;
    }
    if !principal.is_some_and(|p| {
        p.admin
            || entry
                .owner
                .as_ref()
                .is_some_and(|owner| owner.id == p.account_id)
    }) {
        entry.content.notes = None;
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
pub(super) async fn load_any<'e>(
    executor: impl SqliteExecutor<'e>,
    id: &str,
) -> Result<MediaEntry, ApiError> {
    check_id(id)?;
    let data: Option<String> = sqlx::query_scalar("SELECT data FROM media_entries WHERE id = ?")
        .bind(id)
        .fetch_optional(executor)
        .await?;
    super::decode(&data.ok_or_else(ApiError::not_found)?)
}
pub(super) fn is_public(entry: &MediaEntry) -> bool {
    entry.scope == MediaScope::Shared || entry.content.published
}
pub(super) async fn public_entry(
    pool: &SqlitePool,
    id: &str,
    principal: Option<&Principal>,
) -> Result<MediaEntry, ApiError> {
    let entry = load_any(pool, id).await?;
    if !is_public(&entry) {
        return Err(ApiError::not_found());
    }
    let mut entry = project(entry, principal);
    entry.content.notes = None;
    Ok(entry)
}
pub(super) async fn list(
    pool: &SqlitePool,
    scope: MediaScope,
    principal: Option<&Principal>,
    query: &MediaQuery,
) -> Result<MediaList, ApiError> {
    let account = owner(scope, principal)?;
    let data: Vec<String> = sqlx::query_scalar("SELECT data FROM media_entries WHERE scope = ? AND owner_id IS ? ORDER BY updated_at DESC LIMIT 10001")
        .bind(super::spelling(&scope)).bind(account).fetch_all(pool).await?;
    listing(&data, principal, query, false)
}
pub(super) async fn list_visible(
    pool: &SqlitePool,
    principal: Option<&Principal>,
    query: &MediaQuery,
    administrative: bool,
) -> Result<MediaList, ApiError> {
    if let Some(owner) = &query.owner_id {
        check_id(owner)?;
    }
    let (page, page_size) = query_bounds(query)?;
    // 全站跨多个所有者，不能再沿用单库 10001 条的截断；计数和分页都交给数据库。
    let mut tx = pool.begin().await?;
    let mut count_query =
        QueryBuilder::<Sqlite>::new("SELECT json_extract(data,'$.status'),COUNT(*)");
    visible_where(&mut count_query, administrative, query, false);
    count_query.push(" GROUP BY json_extract(data,'$.status')");
    let groups: Vec<(String, i64)> = count_query.build_query_as().fetch_all(&mut *tx).await?;
    let mut counts = MediaCounts::default();
    for (status, count) in groups {
        let count = u64::try_from(count).map_err(|_| ApiError::internal("invalid media count"))?;
        counts.all += count;
        match status.as_str() {
            "planned" => counts.planned = count,
            "watching" => counts.watching = count,
            "completed" => counts.completed = count,
            _ => return Err(ApiError::internal("invalid stored media status")),
        }
    }
    let mut total_query = QueryBuilder::<Sqlite>::new("SELECT COUNT(*)");
    visible_where(&mut total_query, administrative, query, true);
    let total: i64 = total_query.build_query_scalar().fetch_one(&mut *tx).await?;
    let mut entries_query = QueryBuilder::<Sqlite>::new("SELECT data");
    visible_where(&mut entries_query, administrative, query, true);
    entries_query.push(match query.sort.unwrap_or(MediaSort::Updated) {
        MediaSort::Updated => " ORDER BY updated_at DESC",
        MediaSort::Title => " ORDER BY json_extract(data,'$.title') ASC",
        MediaSort::Year => " ORDER BY json_extract(data,'$.year') DESC",
        MediaSort::Watched => " ORDER BY json_extract(data,'$.watchedAt') DESC",
    });
    entries_query
        .push(",id LIMIT ")
        .push_bind(i64::from(page_size))
        .push(" OFFSET ")
        .push_bind(i64::from(page - 1) * i64::from(page_size));
    let data: Vec<String> = entries_query
        .build_query_scalar()
        .fetch_all(&mut *tx)
        .await?;
    tx.commit().await?;
    let items = data
        .iter()
        .map(|data| {
            let mut entry = project(super::decode(data)?, principal);
            if !administrative {
                entry.content.notes = None;
            }
            Ok(entry)
        })
        .collect::<Result<_, ApiError>>()?;
    Ok(MediaList {
        items,
        total: u64::try_from(total).map_err(|_| ApiError::internal("invalid media count"))?,
        page,
        page_size,
        counts,
    })
}
fn visible_where(
    sql: &mut QueryBuilder<Sqlite>,
    administrative: bool,
    query: &MediaQuery,
    filtered: bool,
) {
    sql.push(" FROM media_entries WHERE (")
        .push_bind(administrative)
        .push(" OR scope='shared' OR json_extract(data,'$.published')=1)");
    if let Some(owner) = &query.owner_id {
        sql.push(" AND owner_id=").push_bind(owner);
    }
    if let Some(scope) = query.scope {
        sql.push(" AND scope=").push_bind(super::spelling(&scope));
    }
    if !filtered {
        return;
    }
    if let Some(media_type) = query.media_type {
        sql.push(" AND json_extract(data,'$.mediaType')=")
            .push_bind(super::spelling(&media_type));
    }
    if let Some(status) = query.status {
        sql.push(" AND json_extract(data,'$.status')=")
            .push_bind(super::spelling(&status));
    }
    if let Some(year) = query.year {
        sql.push(" AND json_extract(data,'$.year')=")
            .push_bind(year);
    }
    if let Some(genre) = &query.genre {
        sql.push(" AND EXISTS(SELECT 1 FROM json_each(data,'$.genres') WHERE value=")
            .push_bind(genre)
            .push(")");
    }
    if let Some(term) = query
        .q
        .as_ref()
        .map(|q| q.trim().to_lowercase())
        .filter(|q| !q.is_empty())
    {
        sql.push(" AND (instr(lower(json_extract(data,'$.title')),")
            .push_bind(term.clone())
            .push(")>0 OR instr(lower(json_extract(data,'$.originalTitle')),")
            .push_bind(term.clone())
            .push(")>0");
        if administrative {
            sql.push(" OR instr(lower(json_extract(data,'$.owner.displayName')),")
                .push_bind(term.clone())
                .push(")>0 OR instr(lower(owner_id),")
                .push_bind(term)
                .push(")>0");
        }
        sql.push(")");
    }
}
fn query_bounds(query: &MediaQuery) -> Result<(u32, u32), ApiError> {
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
    Ok((page, page_size))
}
fn listing(
    data: &[String],
    principal: Option<&Principal>,
    query: &MediaQuery,
    public: bool,
) -> Result<MediaList, ApiError> {
    let (page, page_size) = query_bounds(query)?;
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
                        || (!public
                            && e.owner.as_ref().is_some_and(|owner| {
                                owner.display_name.to_lowercase().contains(q)
                                    || owner.id.to_lowercase().contains(q)
                            }))
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
        .map(|e| {
            let mut entry = project(e, principal);
            if public {
                entry.content.notes = None;
            }
            entry
        })
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
    entry: &mut MediaEntry,
) -> Result<(), ApiError> {
    if entry.scope == MediaScope::Private {
        let account = owner(entry.scope, Some(principal))?.ok_or_else(ApiError::unauthorized)?;
        let (display_name, avatar_url): (String, Option<String>) =
            sqlx::query_as("SELECT display_name, avatar_url FROM accounts WHERE id = ?")
                .bind(account)
                .fetch_one(&mut **tx)
                .await?;
        entry.owner = Some(MediaAccount {
            id: account.to_owned(),
            display_name,
            avatar_url,
        });
    }
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
pub(super) fn new_entry(
    scope: MediaScope,
    mut content: MediaContent,
) -> Result<MediaEntry, ApiError> {
    if scope == MediaScope::Shared {
        content.published = true;
    }
    let at = super::timestamp(super::now());
    Ok(MediaEntry {
        id: super::id()?,
        version: 1,
        scope,
        owner: None,
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
    let mut entry = new_entry(scope, content)?;
    insert(&mut tx, principal, &mut entry).await?;
    remember(
        &mut tx,
        principal,
        &input.request_id,
        &operation,
        &hash,
        &entry.id,
    )
    .await?;
    audit(&mut tx, principal, "media.created", &entry).await?;
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
    if entry.scope == MediaScope::Private
        && !entry.content.published
        && input.content.published
        && entry
            .owner
            .as_ref()
            .is_none_or(|owner| owner.id != principal.account_id)
    {
        // 管理员可治理或撤下内容，但不能代替所有者作出首次/再次公开的选择。
        return Err(super::auth::forbidden());
    }
    entry.content = input.content;
    if scope == MediaScope::Shared {
        entry.content.published = true;
    }
    let entry = save(&mut tx, entry).await?;
    audit(&mut tx, principal, "media.updated", &entry).await?;
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
    audit(&mut tx, principal, "media.deleted", &entry).await?;
    tx.commit().await?;
    Ok(())
}

pub(super) async fn audit(
    tx: &mut Transaction<'_, Sqlite>,
    principal: &Principal,
    action: &str,
    entry: &MediaEntry,
) -> Result<(), ApiError> {
    // 管理跨用户内容必须与成功写入一起留痕；不记录备注、磁力或邮箱。
    let target = format!(
        "{}:{}:{}",
        super::spelling(&entry.scope),
        entry.owner.as_ref().map_or("shared", |o| o.id.as_str()),
        entry.id
    );
    sqlx::query(
        "INSERT INTO audit_log(account_id,device_id,action,target,created_at) VALUES(?,?,?,?,?)",
    )
    .bind(&principal.account_id)
    .bind(if principal.credential_kind == "device" {
        Some(principal.credential_id.as_str())
    } else {
        None
    })
    .bind(action)
    .bind(target)
    .bind(super::now())
    .execute(&mut **tx)
    .await?;
    Ok(())
}
