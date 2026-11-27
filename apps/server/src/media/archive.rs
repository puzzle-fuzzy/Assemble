//! 显式维护命令导入只读影视快照。先完整核对，再以单事务发布；原始文件不经 HTTP 暴露。
use super::{assets, store};
use crate::error::ApiError;
use assemble_protocol::media::{MediaCandidate, MediaEntry, MediaIdentity, MediaImage, MediaScope};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Bundle {
    version: u32,
    source: String,
    captured_at: i64,
    source_tables: BTreeMap<String, Vec<Value>>,
    files: Vec<File>,
    assets: Vec<Asset>,
    entries: Vec<Entry>,
    collections: Vec<Collection>,
    catalog: Vec<Catalog>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct File {
    key: String,
    sha256: String,
    size: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Asset {
    id: String,
    sha256: String,
    created_at: i64,
    variants: BTreeMap<String, String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Entry {
    entry: MediaEntry,
    gallery: Vec<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Collection {
    id: String,
    version: u64,
    title: String,
    description: String,
    cover_entry_id: Option<String>,
    entry_ids: Vec<String>,
    created_at: i64,
    updated_at: i64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Catalog {
    entry: MediaEntry,
    expires_at: i64,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub fingerprint: String,
    pub entries: usize,
    pub collections: usize,
    pub cached_metadata: usize,
    pub assets: usize,
    pub source_files: usize,
    pub source_bytes: u64,
    pub already_imported: bool,
}
struct Checked {
    bundle: Bundle,
    manifest: String,
    files: BTreeMap<String, u64>,
    report: Report,
}
struct ImageInfo {
    width: u32,
    height: u32,
    mime: String,
}
fn invalid() -> ApiError {
    ApiError::invalid("invalid media import bundle")
}
fn rejected(code: &'static str) -> ApiError {
    ApiError::new(
        axum::http::StatusCode::BAD_REQUEST,
        code,
        "media archive validation failed",
    )
}
fn hash(bytes: &[u8]) -> String {
    assemble_protocol::primitives::to_hex(&Sha256::digest(bytes))
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
fn business_only(value: &Value) -> bool {
    match value {
        Value::Object(fields) => fields.iter().all(|(key, value)| {
            !(matches!(
                key.as_str(),
                "session_id"
                    | "sessionId"
                    | "token"
                    | "password"
                    | "cookie"
                    | "authorization"
                    | "accessToken"
                    | "refreshToken"
                    | "passwordHash"
            ) || key == "scope" && value.as_str() == Some("private"))
                && business_only(value)
        }),
        Value::Array(values) => values.iter().all(business_only),
        _ => true,
    }
}
fn image_ids(entry: &MediaEntry) -> impl Iterator<Item = &MediaImage> {
    entry
        .poster
        .iter()
        .chain(entry.cast.iter().filter_map(|person| person.photo.as_ref()))
}
fn milliseconds(value: &str) -> Result<i64, ApiError> {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|time| time.timestamp_millis())
        .map_err(|_| invalid())
}

async fn regular(path: &Path) -> Result<(), ApiError> {
    let meta = tokio::fs::symlink_metadata(path)
        .await
        .map_err(|_| invalid())?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err(invalid());
    }
    Ok(())
}
async fn load(directory: &Path) -> Result<Checked, ApiError> {
    for path in [directory.to_path_buf(), directory.join("files")] {
        let meta = tokio::fs::symlink_metadata(path)
            .await
            .map_err(|_| rejected("MEDIA_ARCHIVE_DIRECTORY_INVALID"))?;
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err(rejected("MEDIA_ARCHIVE_DIRECTORY_INVALID"));
        }
    }
    let manifest_path = directory.join("manifest.json");
    regular(&manifest_path)
        .await
        .map_err(|_| rejected("MEDIA_ARCHIVE_MANIFEST_UNREADABLE"))?;
    if tokio::fs::metadata(&manifest_path)
        .await
        .map_err(|_| invalid())?
        .len()
        > 64 * 1024 * 1024
    {
        return Err(rejected("MEDIA_ARCHIVE_LIMIT_EXCEEDED"));
    }
    let bytes = tokio::fs::read(manifest_path)
        .await
        .map_err(|_| invalid())?;
    let fingerprint = hash(&bytes);
    let manifest = String::from_utf8(bytes).map_err(|_| invalid())?;
    let bundle: Bundle =
        serde_json::from_str(&manifest).map_err(|_| rejected("MEDIA_ARCHIVE_MANIFEST_INVALID"))?;
    if bundle.version != 1
        || bundle.source != "jarvis-media"
        || bundle.captured_at <= 0
        || bundle.entries.len() > 10_000
        || bundle.files.len() > 100_000
        || bundle.assets.len() > 20_000
        || bundle.catalog.len() > 10_000
        || bundle.collections.len() > 10_000
    {
        return Err(rejected("MEDIA_ARCHIVE_LIMIT_EXCEEDED"));
    }
    if bundle
        .source_tables
        .get("media_entries")
        .is_none_or(|rows| rows.len() != bundle.entries.len())
        || bundle.source_tables.keys().any(|name| {
            !matches!(
                name.as_str(),
                "media_assets"
                    | "media_cache"
                    | "media_cache_assets"
                    | "media_collection_members"
                    | "media_collections"
                    | "media_entries"
                    | "media_entry_assets"
                    | "media_entry_cast"
                    | "media_identities"
                    | "media_jobs"
                    | "media_migration_runs"
            )
        })
        || bundle
            .source_tables
            .values()
            .flatten()
            .any(|row| !business_only(row))
    {
        return Err(rejected("MEDIA_ARCHIVE_SOURCE_INVALID"));
    }
    let (files, total) = load_files(directory, &bundle.files).await?;
    let images = load_images(directory, &bundle, &files)
        .await
        .map_err(|_| rejected("MEDIA_ARCHIVE_IMAGES_INVALID"))?;
    check_references(&bundle, &images)?;
    let report = Report {
        fingerprint,
        entries: bundle.entries.len(),
        collections: bundle.collections.len(),
        cached_metadata: bundle.catalog.len(),
        assets: bundle.assets.len(),
        source_files: bundle.files.len(),
        source_bytes: total,
        already_imported: false,
    };
    Ok(Checked {
        bundle,
        manifest,
        files,
        report,
    })
}
async fn load_files(
    directory: &Path,
    source: &[File],
) -> Result<(BTreeMap<String, u64>, u64), ApiError> {
    let mut source_keys = BTreeSet::new();
    let mut files = BTreeMap::new();
    let mut total = 0_u64;
    for file in source {
        total = total.checked_add(file.size).ok_or_else(invalid)?;
        if !valid_hash(&file.sha256)
            || file.key.is_empty()
            || !source_keys.insert(&file.key)
            || file.size > 64 * 1024 * 1024
            || total > 4 * 1024 * 1024 * 1024
        {
            return Err(rejected("MEDIA_ARCHIVE_FILE_INVALID"));
        }
        if !files.contains_key(&file.sha256) {
            read_verified(directory, &file.sha256, file.size).await?;
            files.insert(file.sha256.clone(), file.size);
        } else if files[&file.sha256] != file.size {
            return Err(invalid());
        }
    }
    Ok((files, total))
}
async fn read_verified(directory: &Path, sha: &str, size: u64) -> Result<Vec<u8>, ApiError> {
    let path = directory.join("files").join(sha);
    regular(&path)
        .await
        .map_err(|_| rejected("MEDIA_ARCHIVE_FILE_UNREADABLE"))?;
    if tokio::fs::metadata(&path)
        .await
        .map_err(|_| invalid())?
        .len()
        != size
    {
        return Err(rejected("MEDIA_ARCHIVE_FILE_CORRUPTED"));
    }
    let content = tokio::fs::read(path)
        .await
        .map_err(|_| rejected("MEDIA_ARCHIVE_FILE_UNREADABLE"))?;
    if content.len() as u64 != size || hash(&content) != sha {
        return Err(rejected("MEDIA_ARCHIVE_FILE_CORRUPTED"));
    }
    Ok(content)
}
async fn load_images(
    directory: &Path,
    bundle: &Bundle,
    files: &BTreeMap<String, u64>,
) -> Result<BTreeMap<String, ImageInfo>, ApiError> {
    let mut images = BTreeMap::new();
    for asset in &bundle.assets {
        store::check_id(&asset.id)?;
        if images.contains_key(&asset.id)
            || !valid_hash(&asset.sha256)
            || asset.variants.get("original") != Some(&asset.sha256)
            || asset
                .variants
                .keys()
                .any(|name| !matches!(name.as_str(), "original" | "200" | "360" | "480"))
        {
            return Err(invalid());
        }
        let mut info = None;
        for sha in asset.variants.values().collect::<BTreeSet<_>>() {
            let bytes = read_verified(directory, sha, *files.get(sha).ok_or_else(invalid)?).await?;
            let (width, height, mime) =
                tokio::task::spawn_blocking(move || assets::inspect(&bytes))
                    .await
                    .map_err(|_| invalid())??;
            if sha == &asset.sha256 {
                info = Some(ImageInfo {
                    width,
                    height,
                    mime,
                });
            }
        }
        images.insert(asset.id.clone(), info.ok_or_else(invalid)?);
    }
    Ok(images)
}
async fn prepare_image(
    directory: &Path,
    asset: &Asset,
    files: &BTreeMap<String, u64>,
) -> Result<assets::Prepared, ApiError> {
    let original = read_verified(
        directory,
        &asset.sha256,
        *files.get(&asset.sha256).ok_or_else(invalid)?,
    )
    .await?;
    let mut supplied = BTreeMap::new();
    for (name, sha) in &asset.variants {
        if name != "original" {
            supplied.insert(
                name.clone(),
                read_verified(directory, sha, *files.get(sha).ok_or_else(invalid)?).await?,
            );
        }
    }
    tokio::task::spawn_blocking(move || {
        let mut prepared = assets::prepare(original.clone())?;
        for variant in &mut prepared.variants {
            if let Some(bytes) = supplied.remove(variant.name) {
                let (_, _, mime) = assets::inspect(&bytes)?;
                variant.mime = mime;
                variant.sha = hash(&bytes);
                variant.bytes = bytes;
            } else if variant.name != "original" {
                let size = variant.name.parse::<u32>().map_err(|_| invalid())?;
                // 小图已经适合该尺寸时直接复用原字节，避免把旧 JPEG 无益地膨胀成 PNG。
                if prepared.width <= size && prepared.height <= size {
                    variant.mime.clone_from(&prepared.mime);
                    variant.sha.clone_from(&prepared.sha);
                    variant.bytes.clone_from(&original);
                }
            }
        }
        Ok(prepared)
    })
    .await
    .map_err(|_| invalid())?
}
fn check_references(bundle: &Bundle, images: &BTreeMap<String, ImageInfo>) -> Result<(), ApiError> {
    let mut entry_ids = BTreeSet::new();
    let mut identities = BTreeSet::new();
    for value in &bundle.entries {
        validate_entry(&value.entry, images)?;
        if !entry_ids.insert(&value.entry.id)
            || value.gallery.iter().any(|id| !images.contains_key(id))
            || value
                .entry
                .poster
                .as_ref()
                .is_some_and(|image| !value.gallery.contains(&image.asset_id))
        {
            return Err(rejected("MEDIA_ARCHIVE_ENTRY_REFERENCES_INVALID"));
        }
        for identity in &value.entry.identities {
            if !identities.insert(super::encode(identity)?) {
                return Err(rejected("MEDIA_ARCHIVE_DUPLICATE_IDENTITY"));
            }
        }
    }
    let mut collection_ids = BTreeSet::new();
    for collection in &bundle.collections {
        store::check_id(&collection.id)?;
        let members = collection.entry_ids.iter().collect::<BTreeSet<_>>();
        if !collection_ids.insert(&collection.id)
            || collection.version == 0
            || collection.version > i64::MAX as u64
            || collection.title.trim().is_empty()
            || collection.title.chars().count() > 300
            || collection.description.chars().count() > 20_000
            || members.len() != collection.entry_ids.len()
            || collection
                .entry_ids
                .iter()
                .any(|id| !entry_ids.contains(id))
            || collection
                .cover_entry_id
                .as_ref()
                .is_some_and(|id| !members.contains(id))
        {
            return Err(invalid());
        }
    }
    let mut cache_ids = BTreeSet::new();
    for cached in &bundle.catalog {
        let entry = &cached.entry;
        validate_entry(entry, images)?;
        if entry.identities.len() != 1
            || entry.content.magnet_uri.is_some()
            || entry.content.review.is_some()
            || entry.content.notes.is_some()
            || entry.content.watched_at.is_some()
            || entry.magnet_available
            || !cache_ids.insert(super::encode(&entry.identities[0])?)
        {
            return Err(invalid());
        }
    }
    Ok(())
}
fn validate_entry(
    entry: &MediaEntry,
    images: &BTreeMap<String, ImageInfo>,
) -> Result<(), ApiError> {
    store::check_id(&entry.id).map_err(|_| rejected("MEDIA_ARCHIVE_ENTRY_ID_INVALID"))?;
    store::validate_content(&entry.content)
        .map_err(|_| rejected("MEDIA_ARCHIVE_CONTENT_INVALID"))?;
    milliseconds(&entry.created_at).map_err(|_| rejected("MEDIA_ARCHIVE_TIMESTAMP_INVALID"))?;
    milliseconds(&entry.updated_at).map_err(|_| rejected("MEDIA_ARCHIVE_TIMESTAMP_INVALID"))?;
    if entry.scope != MediaScope::Shared
        || entry.owner.is_some()
        || entry.version == 0
        || entry.version > i64::MAX as u64
        || entry.cast.len() > 1000
    {
        return Err(invalid());
    }
    for identity in &entry.identities {
        super::providers::validate_identity(identity)
            .map_err(|_| rejected("MEDIA_ARCHIVE_IDENTITY_INVALID"))?;
    }
    for image in image_ids(entry) {
        let asset = images.get(&image.asset_id).ok_or_else(invalid)?;
        if image.width != asset.width
            || image.height != asset.height
            || image.content_type != asset.mime
        {
            return Err(rejected("MEDIA_ARCHIVE_IMAGE_METADATA_INVALID"));
        }
    }
    Ok(())
}
/// 默认只核对输入，不连接或迁移目标数据库。
pub async fn inspect(directory: &Path) -> Result<Report, ApiError> {
    Ok(load(directory).await?.report)
}
/// 目标必须尚无片库；重复同一指纹只回报已导入，不覆盖之后的人工修改。
pub async fn import(pool: &SqlitePool, directory: &Path) -> Result<Report, ApiError> {
    let Checked {
        bundle,
        manifest,
        files,
        mut report,
    } = load(directory).await?;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM media_import_runs WHERE id = ?)")
            .bind(&report.fingerprint)
            .fetch_one(&mut *tx)
            .await?;
    if exists {
        report.already_imported = true;
        return Ok(report);
    }
    let occupied:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM media_entries) OR EXISTS(SELECT 1 FROM media_assets) OR EXISTS(SELECT 1 FROM media_collections) OR EXISTS(SELECT 1 FROM media_import_runs)").fetch_one(&mut *tx).await?;
    if occupied {
        return Err(ApiError::conflict(
            "MEDIA_IMPORT_TARGET_NOT_EMPTY",
            "media import will not overwrite an existing library",
        ));
    }
    sqlx::query("INSERT INTO media_import_runs(id,source,captured_at,imported_at,manifest_json) VALUES(?,?,?,?,?)").bind(&report.fingerprint).bind(&bundle.source).bind(bundle.captured_at).bind(super::now()).bind(manifest).execute(&mut *tx).await?;
    for file in &bundle.files {
        let content = read_verified(directory, &file.sha256, file.size).await?;
        sqlx::query("INSERT INTO media_import_files(run_id,source_key,sha256,size,content) VALUES(?,?,?,?,?)").bind(&report.fingerprint).bind(&file.key).bind(&file.sha256).bind(i64::try_from(file.size).map_err(|_|invalid())?).bind(content).execute(&mut *tx).await?;
    }
    for asset in &bundle.assets {
        let image = prepare_image(directory, asset, &files).await?;
        let id = &asset.id;
        let created = asset.created_at;
        sqlx::query("INSERT INTO media_assets(id,owner_key,sha256,width,height,content_type,created_at) VALUES(?,'shared',?,?,?,?,?)").bind(id).bind(image.sha).bind(i64::from(image.width)).bind(i64::from(image.height)).bind(image.mime).bind(created).execute(&mut *tx).await?;
        for variant in image.variants {
            sqlx::query("INSERT INTO media_asset_variants(asset_id,variant,content_type,sha256,size,content) VALUES(?,?,?,?,?,?)").bind(id).bind(variant.name).bind(variant.mime).bind(variant.sha).bind(i64::try_from(variant.bytes.len()).map_err(|_|invalid())?).bind(variant.bytes).execute(&mut *tx).await?;
        }
        sqlx::query("INSERT INTO media_import_asset_refs(run_id,asset_id) VALUES(?,?)")
            .bind(&report.fingerprint)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    for value in bundle.entries {
        let mut entry = value.entry;
        entry.content.published = true;
        sqlx::query("INSERT INTO media_entries(id,scope,owner_id,version,data,created_at,updated_at) VALUES(?,'shared',NULL,?,?,?,?)").bind(&entry.id).bind(i64::try_from(entry.version).map_err(|_|invalid())?).bind(super::encode(&entry)?).bind(milliseconds(&entry.created_at)?).bind(milliseconds(&entry.updated_at)?).execute(&mut *tx).await?;
        for identity in &entry.identities {
            sqlx::query("INSERT INTO media_identities(entry_id,scope_key,provider,media_type,external_id) VALUES(?,'shared',?,?,?)").bind(&entry.id).bind(super::spelling(&identity.provider)).bind(super::spelling(&identity.media_type)).bind(&identity.external_id).execute(&mut *tx).await?;
        }
        for (position, id) in value.gallery.iter().enumerate() {
            sqlx::query("INSERT OR IGNORE INTO media_entry_assets(entry_id,asset_id,role,position) VALUES(?,?,'gallery',?)").bind(&entry.id).bind(id).bind(i64::try_from(position).map_err(|_|invalid())?).execute(&mut *tx).await?;
        }
        for (position, person) in entry.cast.iter().enumerate() {
            if let Some(photo) = &person.photo {
                sqlx::query("INSERT OR IGNORE INTO media_entry_assets(entry_id,asset_id,role,position) VALUES(?,?,'cast',?)").bind(&entry.id).bind(&photo.asset_id).bind(i64::try_from(position).map_err(|_|invalid())?).execute(&mut *tx).await?;
            }
        }
    }
    for collection in bundle.collections {
        sqlx::query("INSERT INTO media_collections(id,version,title,description,created_at,updated_at,cover_entry_id) VALUES(?,?,?,?,?,?,?)").bind(&collection.id).bind(i64::try_from(collection.version).map_err(|_|invalid())?).bind(collection.title).bind(collection.description).bind(collection.created_at).bind(collection.updated_at).bind(collection.cover_entry_id).execute(&mut *tx).await?;
        for (position, id) in collection.entry_ids.iter().enumerate() {
            sqlx::query("INSERT INTO media_collection_entries(collection_id,entry_id,position) VALUES(?,?,?)").bind(&collection.id).bind(id).bind(i64::try_from(position).map_err(|_|invalid())?).execute(&mut *tx).await?;
        }
    }
    for cached in bundle.catalog {
        let entry = cached.entry;
        let identity = &entry.identities[0];
        sqlx::query("INSERT INTO media_import_catalog(provider,media_type,external_id,run_id,data,source_expires_at) VALUES(?,?,?,?,?,?)").bind(super::spelling(&identity.provider)).bind(super::spelling(&identity.media_type)).bind(&identity.external_id).bind(&report.fingerprint).bind(super::encode(&entry)?).bind(cached.expires_at).execute(&mut *tx).await?;
        for image in image_ids(&entry) {
            sqlx::query(
                "INSERT OR IGNORE INTO media_import_catalog_assets(run_id,asset_id) VALUES(?,?)",
            )
            .bind(&report.fingerprint)
            .bind(&image.asset_id)
            .execute(&mut *tx)
            .await?;
        }
    }
    tx.commit().await?;
    Ok(report)
}
pub(super) async fn cached(
    pool: &SqlitePool,
    identity: &MediaIdentity,
) -> Result<Option<MediaEntry>, ApiError> {
    let value:Option<String>=sqlx::query_scalar("SELECT data FROM media_import_catalog WHERE provider = ? AND media_type = ? AND external_id = ?").bind(super::spelling(&identity.provider)).bind(super::spelling(&identity.media_type)).bind(&identity.external_id).fetch_optional(pool).await?;
    value.map(|value| super::decode(&value)).transpose()
}
pub(super) fn candidate(entry: MediaEntry, identity: MediaIdentity) -> MediaCandidate {
    MediaCandidate {
        identity,
        title: entry.content.title,
        original_title: entry.content.original_title,
        year: entry.content.year,
        overview: entry.content.overview,
        poster: entry.poster,
    }
}

#[cfg(test)]
#[path = "archive_tests.rs"]
mod tests;
