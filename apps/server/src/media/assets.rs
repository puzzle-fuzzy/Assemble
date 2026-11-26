//! 图像在受限阻塞线程解码，原图与全部缩略图校验后以事务同时发布。
use super::auth::Principal;
use crate::error::ApiError;
use crate::http::Shared;
use assemble_protocol::media::{MediaGallery, MediaImage, MediaScope};
use axum::body::Body;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use image::{GenericImageView, ImageFormat, ImageReader};
use sha2::{Digest, Sha256};
use sqlx::{Sqlite, SqlitePool, Transaction};
use std::io::Cursor;
use tokio::sync::Semaphore;

pub const MAX_IMAGE_BYTES: usize = 12 * 1024 * 1024;
static DECODERS: Semaphore = Semaphore::const_new(2);
pub(super) struct Variant {
    pub(super) name: &'static str,
    pub(super) mime: String,
    pub(super) sha: String,
    pub(super) bytes: Vec<u8>,
}
pub(super) struct Prepared {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) mime: String,
    pub(super) sha: String,
    pub(super) variants: Vec<Variant>,
}
fn image_error() -> ApiError {
    ApiError::invalid("image type, dimensions or content is invalid")
}
fn digest(bytes: &[u8]) -> String {
    assemble_protocol::primitives::to_hex(&Sha256::digest(bytes))
}
fn decode(bytes: &[u8]) -> Result<(image::DynamicImage, String), ApiError> {
    if bytes.is_empty() || bytes.len() > MAX_IMAGE_BYTES {
        return Err(image_error());
    }
    let format = image::guess_format(bytes).map_err(|_| image_error())?;
    let mime = match format {
        ImageFormat::Jpeg => "image/jpeg",
        ImageFormat::Png => "image/png",
        ImageFormat::WebP => "image/webp",
        _ => return Err(image_error()),
    }
    .to_owned();
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(128 * 1024 * 1024);
    let mut reader = ImageReader::with_format(Cursor::new(&bytes), format);
    reader.limits(limits);
    let image = reader.decode().map_err(|_| image_error())?;
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > 24_000_000 {
        return Err(image_error());
    }
    Ok((image, mime))
}
pub(super) fn inspect(bytes: &[u8]) -> Result<(u32, u32, String), ApiError> {
    let (image, mime) = decode(bytes)?;
    let (width, height) = image.dimensions();
    Ok((width, height, mime))
}
pub(super) fn prepare(bytes: Vec<u8>) -> Result<Prepared, ApiError> {
    let (image, mime) = decode(&bytes)?;
    let (width, height) = image.dimensions();
    let sha = digest(&bytes);
    let mut variants = vec![Variant {
        name: "original",
        mime: mime.clone(),
        sha: sha.clone(),
        bytes,
    }];
    for (name, size) in [("200", 200), ("360", 360), ("480", 480)] {
        let thumb = image.thumbnail(size, size);
        let mut output = Cursor::new(Vec::new());
        thumb
            .write_to(&mut output, ImageFormat::Png)
            .map_err(|_| image_error())?;
        let bytes = output.into_inner();
        variants.push(Variant {
            name,
            mime: "image/png".into(),
            sha: digest(&bytes),
            bytes,
        });
    }
    Ok(Prepared {
        width,
        height,
        mime,
        sha,
        variants,
    })
}
pub async fn store(
    pool: &SqlitePool,
    owner_key: &str,
    bytes: Vec<u8>,
    public_catalog: bool,
) -> Result<MediaImage, ApiError> {
    let _permit = DECODERS
        .acquire()
        .await
        .map_err(|_| ApiError::internal("image worker unavailable"))?;
    let prepared = tokio::task::spawn_blocking(move || prepare(bytes))
        .await
        .map_err(|_| ApiError::internal("image worker failed"))??;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let existing: Option<String> =
        sqlx::query_scalar("SELECT id FROM media_assets WHERE owner_key = ? AND sha256 = ?")
            .bind(owner_key)
            .bind(&prepared.sha)
            .fetch_optional(&mut *tx)
            .await?;
    let asset_id = if let Some(id) = existing {
        id
    } else {
        let used:i64=sqlx::query_scalar("SELECT coalesce(sum(v.size),0) FROM media_asset_variants v JOIN media_assets a ON a.id = v.asset_id WHERE a.owner_key = ?").bind(owner_key).fetch_one(&mut *tx).await?;
        let total: i64 =
            sqlx::query_scalar("SELECT coalesce(sum(size),0) FROM media_asset_variants")
                .fetch_one(&mut *tx)
                .await?;
        let added: usize = prepared.variants.iter().map(|v| v.bytes.len()).sum();
        if u64::try_from(used).map_err(|_| image_error())? + added as u64 > 512 * 1024 * 1024
            || u64::try_from(total).map_err(|_| image_error())? + added as u64
                > 2 * 1024 * 1024 * 1024
        {
            return Err(ApiError::conflict(
                "QUOTA_EXCEEDED",
                "media image quota exceeded",
            ));
        }
        let id = super::id()?;
        sqlx::query("INSERT INTO media_assets(id,owner_key,sha256,width,height,content_type,created_at) VALUES(?,?,?,?,?,?,?)").bind(&id).bind(owner_key).bind(&prepared.sha).bind(i64::from(prepared.width)).bind(i64::from(prepared.height)).bind(&prepared.mime).bind(super::now()).execute(&mut *tx).await?;
        for variant in prepared.variants {
            sqlx::query("INSERT INTO media_asset_variants(asset_id,variant,content_type,sha256,size,content) VALUES(?,?,?,?,?,?)").bind(&id).bind(variant.name).bind(variant.mime).bind(variant.sha).bind(i64::try_from(variant.bytes.len()).map_err(|_|image_error())?).bind(variant.bytes).execute(&mut *tx).await?;
        }
        id
    };
    if public_catalog {
        sqlx::query("INSERT INTO media_catalog_assets(asset_id,expires_at) VALUES(?,?) ON CONFLICT(asset_id) DO UPDATE SET expires_at = excluded.expires_at").bind(&asset_id).bind(super::now()+3_600_000).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(MediaImage {
        asset_id,
        width: prepared.width,
        height: prepared.height,
        content_type: prepared.mime,
    })
}
pub(super) async fn link(
    tx: &mut Transaction<'_, Sqlite>,
    entry_id: &str,
    images: &[MediaImage],
    role: &str,
) -> Result<(), ApiError> {
    for (position, image) in images.iter().enumerate() {
        sqlx::query("INSERT OR IGNORE INTO media_entry_assets(entry_id,asset_id,role,position) VALUES(?,?,?,?)").bind(entry_id).bind(&image.asset_id).bind(role).bind(i64::try_from(position).map_err(|_|image_error())?).execute(&mut **tx).await?;
    }
    Ok(())
}
pub(super) async fn gallery(
    pool: &SqlitePool,
    scope: MediaScope,
    principal: Option<&Principal>,
    entry_id: &str,
) -> Result<MediaGallery, ApiError> {
    let entry = super::store::load(
        pool,
        scope,
        super::store::owner(scope, principal)?,
        entry_id,
    )
    .await?;
    let rows:Vec<(String,i64,i64,String)>=sqlx::query_as("SELECT a.id,a.width,a.height,a.content_type FROM media_assets a JOIN media_entry_assets r ON r.asset_id = a.id WHERE r.entry_id = ? AND r.role = 'gallery' ORDER BY r.position,a.id").bind(entry_id).fetch_all(pool).await?;
    let items = rows
        .into_iter()
        .map(|(asset_id, width, height, content_type)| {
            Ok(MediaImage {
                asset_id,
                width: u32::try_from(width).map_err(|_| image_error())?,
                height: u32::try_from(height).map_err(|_| image_error())?,
                content_type,
            })
        })
        .collect::<Result<_, ApiError>>()?;
    Ok(MediaGallery {
        entry_id: entry.id,
        version: entry.version,
        items,
    })
}
pub(super) async fn content(
    state: &Shared,
    principal: Option<&Principal>,
    asset_id: &str,
    variant: &str,
) -> Result<Response, ApiError> {
    super::store::check_id(asset_id)?;
    if !matches!(variant, "original" | "200" | "360" | "480") {
        return Err(ApiError::invalid("unknown image variant"));
    }
    // 可公开引用和私人所有权都由关系表核对，资产存在本身从来不构成读取权限。
    let allowed:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM media_entry_assets r JOIN media_entries e ON e.id = r.entry_id WHERE r.asset_id = ? AND (e.scope = 'shared' OR (e.scope = 'private' AND e.owner_id = ?))) OR EXISTS(SELECT 1 FROM media_catalog_assets WHERE asset_id = ? AND expires_at > ?) OR EXISTS(SELECT 1 FROM media_import_catalog_assets WHERE asset_id = ?)")
        .bind(asset_id).bind(principal.map(|p|p.account_id.as_str())).bind(asset_id).bind(super::now()).bind(asset_id).fetch_one(&state.pool).await?;
    if !allowed {
        return Err(ApiError::not_found());
    }
    let row:Option<(String,String,i64,Vec<u8>)>=sqlx::query_as("SELECT content_type,sha256,size,content FROM media_asset_variants WHERE asset_id = ? AND variant = ?").bind(asset_id).bind(variant).fetch_optional(&state.pool).await?;
    let (mime, sha, size, bytes) = row.ok_or_else(ApiError::not_found)?;
    if bytes.len() as u64 != u64::try_from(size).map_err(|_| image_error())?
        || digest(&bytes) != sha
    {
        return Err(ApiError::internal("stored image integrity failed"));
    }
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .body(Body::from(bytes))
        .map_err(|_| ApiError::internal("image response failed"))?;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&mime).map_err(|_| image_error())?,
    );
    headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&size.to_string()).map_err(|_| image_error())?,
    );
    headers.insert(
        "x-content-sha256",
        HeaderValue::from_str(&sha).map_err(|_| image_error())?,
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        "cross-origin-resource-policy",
        HeaderValue::from_static("same-origin"),
    );
    Ok(response)
}
pub async fn cleanup(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    sqlx::query("DELETE FROM media_catalog_assets WHERE expires_at <= ?")
        .bind(super::now())
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM media_assets WHERE created_at < ? AND NOT EXISTS(SELECT 1 FROM media_entry_assets r WHERE r.asset_id = media_assets.id) AND NOT EXISTS(SELECT 1 FROM media_catalog_assets c WHERE c.asset_id = media_assets.id) AND NOT EXISTS(SELECT 1 FROM media_import_asset_refs r WHERE r.asset_id = media_assets.id)").bind(super::now()-3_600_000).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}
