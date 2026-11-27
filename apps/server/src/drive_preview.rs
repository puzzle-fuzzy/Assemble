//! 预览复用云盘所属关系与内容服务；实际格式检查不相信扩展名和上传 MIME。
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use assemble_protocol::preview::{FilePreview, PreviewInfo, PreviewKind, TEXT_PREVIEW_BYTES};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::middleware::Next;
use axum::response::Response;
use axum::{Json, Router, middleware, routing::get};
use tokio::io::AsyncReadExt;

use crate::admin::Admin;
use crate::auth::Identity;
use crate::drive::DriveRow;
use crate::error::ApiError;
use crate::http::Shared;

type Cache = VecDeque<(String, Instant, PreviewInfo)>;
static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
static CHECKS: OnceLock<tokio::sync::Semaphore> = OnceLock::new();

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/v1/drive/files/{id}/preview", get(describe))
        .route("/v1/drive/files/{id}/preview/content", get(content))
        .route("/v1/admin/drive/files/{id}/preview", get(admin_describe))
        .route(
            "/v1/admin/drive/files/{id}/preview/content",
            get(admin_content),
        )
        .layer(middleware::from_fn(private))
}

async fn private(request: axum::extract::Request, next: Next) -> Response {
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
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; sandbox"),
    );
    response
}

async fn admin_file(state: &Shared, id: &str) -> Result<DriveRow, ApiError> {
    sqlx::query_as("SELECT * FROM active_drive_files WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(ApiError::not_found)
}

async fn describe(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
) -> Result<Json<FilePreview>, ApiError> {
    let row = crate::drive::load(&state.pool, &identity.account_id, &id).await?;
    Ok(Json(description(&state, &row).await?))
}
async fn admin_describe(
    State(state): State<Shared>,
    _: Admin,
    Path(id): Path<String>,
) -> Result<Json<FilePreview>, ApiError> {
    Ok(Json(
        description(&state, &admin_file(&state, &id).await?).await?,
    ))
}
async fn content(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let row = crate::drive::load(&state.pool, &identity.account_id, &id).await?;
    inline(&state, &row, &headers).await
}
async fn admin_content(
    State(state): State<Shared>,
    _: Admin,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    inline(&state, &admin_file(&state, &id).await?, &headers).await
}

async fn inline(state: &Shared, row: &DriveRow, headers: &HeaderMap) -> Result<Response, ApiError> {
    let info = information(state, row).await?;
    if !matches!(
        info.kind,
        PreviewKind::Image | PreviewKind::Audio | PreviewKind::Video
    ) {
        return Err(ApiError::not_found());
    }
    let mut response = crate::http::file_response(
        &state.blobs.path_for(&row.sha256),
        &row.name,
        info.mime_type.as_deref(),
        headers,
    )
    .await;
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("inline"),
    );
    Ok(response)
}

pub(crate) async fn description(state: &Shared, row: &DriveRow) -> Result<FilePreview, ApiError> {
    let info = information(state, row).await?;
    let (text, truncated) = if info.kind == PreviewKind::Text {
        let bytes = prefix(&state.blobs.path_for(&row.sha256), TEXT_PREVIEW_BYTES + 4).await?;
        let end = bytes.len().min(TEXT_PREVIEW_BYTES);
        let visible = &bytes[..end];
        // 截断不能把 UTF-8 的半个字符作为解码错误展示给用户。
        let visible = match std::str::from_utf8(visible) {
            Ok(text) => text,
            Err(error) if error.error_len().is_none() => {
                std::str::from_utf8(&visible[..error.valid_up_to()])
                    .map_err(|_| ApiError::not_found())?
            }
            Err(_) => return Err(ApiError::not_found()),
        };
        (Some(visible.to_owned()), info.size > visible.len() as u64)
    } else {
        (None, false)
    };
    Ok(FilePreview {
        info,
        text,
        truncated,
    })
}

async fn prefix(path: &std::path::Path, limit: usize) -> Result<Vec<u8>, ApiError> {
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|_| ApiError::not_found())?;
    let mut bytes = Vec::new();
    file.take(limit as u64)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| ApiError::internal("preview source unavailable"))?;
    Ok(bytes)
}

async fn information(state: &Shared, row: &DriveRow) -> Result<PreviewInfo, ApiError> {
    let cache = CACHE.get_or_init(|| Mutex::new(VecDeque::new()));
    if let Some(info) = cache
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .find(|(hash, at, _)| hash == &row.sha256 && at.elapsed() < Duration::from_secs(300))
        .map(|(_, _, info)| info.clone())
    {
        return Ok(info);
    }
    let _permit = CHECKS
        .get_or_init(|| tokio::sync::Semaphore::new(2))
        .acquire()
        .await
        .map_err(|_| ApiError::internal("preview checks unavailable"))?;
    let path = state.blobs.path_for(&row.sha256);
    let bytes = prefix(&path, TEXT_PREVIEW_BYTES + 4).await?;
    let size = u64::try_from(row.size).map_err(|_| ApiError::internal("invalid preview size"))?;
    let mime = if let Ok(format) = image::guess_format(&bytes) {
        let valid = size <= 16 * 1024 * 1024 && valid_image(path.clone(), format).await;
        match format {
            image::ImageFormat::Jpeg if valid => Some((PreviewKind::Image, "image/jpeg")),
            image::ImageFormat::Png if valid => Some((PreviewKind::Image, "image/png")),
            image::ImageFormat::WebP if valid => Some((PreviewKind::Image, "image/webp")),
            _ => None,
        }
    } else if media_signature(&bytes) {
        probe(&path).await
    } else if text_bytes(&bytes, size) {
        Some((PreviewKind::Text, "text/plain; charset=utf-8"))
    } else {
        None
    };
    let info = PreviewInfo {
        kind: mime.map_or(PreviewKind::Unsupported, |v| v.0),
        mime_type: mime.map(|v| v.1.to_owned()),
        size,
    };
    let mut entries = cache.lock().unwrap_or_else(PoisonError::into_inner);
    while entries.len() >= 128 {
        entries.pop_front();
    }
    entries.push_back((row.sha256.clone(), Instant::now(), info.clone()));
    Ok(info)
}

async fn valid_image(path: std::path::PathBuf, format: image::ImageFormat) -> bool {
    tokio::task::spawn_blocking(move || {
        let Ok(file) = std::fs::File::open(path) else {
            return false;
        };
        let mut reader = image::ImageReader::with_format(std::io::BufReader::new(file), format);
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(8192);
        limits.max_image_height = Some(8192);
        limits.max_alloc = Some(64 * 1024 * 1024);
        reader.limits(limits);
        reader.decode().is_ok()
    })
    .await
    .unwrap_or(false)
}

fn text_bytes(bytes: &[u8], total: u64) -> bool {
    let valid = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) if error.error_len().is_none() && total > bytes.len() as u64 => {
            std::str::from_utf8(&bytes[..error.valid_up_to()]).unwrap_or_default()
        }
        Err(_) => return false,
    };
    !valid
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
}

fn media_signature(bytes: &[u8]) -> bool {
    bytes.starts_with(b"ID3")
        || bytes.starts_with(b"RIFF")
        || bytes.starts_with(&[0x1a, 0x45, 0xdf, 0xa3])
        || bytes.get(4..8) == Some(b"ftyp")
        || bytes
            .get(..2)
            .is_some_and(|v| v[0] == 0xff && v[1] & 0xe0 == 0xe0)
}

async fn probe(path: &std::path::Path) -> Option<(PreviewKind, &'static str)> {
    let output = tokio::time::timeout(Duration::from_secs(6), async {
        let mut child = tokio::process::Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-protocol_whitelist",
                "file",
                "-format_whitelist",
                "mp3,wav,mov,matroska,webm",
                "-show_entries",
                "format=format_name:stream=codec_type,codec_name",
                "-of",
                "json",
            ])
            .arg(path)
            .kill_on_drop(true)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()?;
        let mut bytes = Vec::new();
        child
            .stdout
            .take()?
            .take(64 * 1024 + 1)
            .read_to_end(&mut bytes)
            .await
            .ok()?;
        if bytes.len() > 64 * 1024 {
            let _ = child.kill().await;
            return None;
        }
        if !child.wait().await.ok()?.success() {
            return None;
        }
        Some(bytes)
    })
    .await
    .ok()??;
    let data: serde_json::Value = serde_json::from_slice(&output).ok()?;
    let streams = data["streams"].as_array()?;
    if streams.is_empty() || streams.len() > 16 {
        return None;
    }
    let video = streams.iter().any(|s| s["codec_type"] == "video");
    let format = data.pointer("/format/format_name")?.as_str()?;
    let supported = streams.iter().all(|s| match s["codec_type"].as_str() {
        Some("video") => {
            ["h264", "vp8", "vp9", "av1"].contains(&s["codec_name"].as_str().unwrap_or_default())
        }
        Some("audio") => [
            "aac",
            "mp3",
            "opus",
            "vorbis",
            "pcm_s16le",
            "pcm_s24le",
            "pcm_f32le",
        ]
        .contains(&s["codec_name"].as_str().unwrap_or_default()),
        _ => false,
    });
    if !supported {
        return None;
    }
    if format.split(',').any(|v| v == "mp4") {
        Some((
            if video {
                PreviewKind::Video
            } else {
                PreviewKind::Audio
            },
            if video { "video/mp4" } else { "audio/mp4" },
        ))
    } else if format.split(',').any(|v| v == "webm") {
        Some((
            if video {
                PreviewKind::Video
            } else {
                PreviewKind::Audio
            },
            if video { "video/webm" } else { "audio/webm" },
        ))
    } else if !video && format == "mp3" {
        Some((PreviewKind::Audio, "audio/mpeg"))
    } else if !video && format == "wav" {
        Some((PreviewKind::Audio, "audio/wav"))
    } else {
        None
    }
}
