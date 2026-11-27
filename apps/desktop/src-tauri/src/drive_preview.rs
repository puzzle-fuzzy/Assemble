//! 专用 `WebView` 媒体入口；URL 仅标识页面代次，不承载授权，凭据始终由客户端持有。
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock, PoisonError};

use assemble_client::{ClientError, RequestOptions};
use assemble_protocol::preview::{FilePreview, PREVIEW_CHUNK_BYTES, PreviewKind};
use tauri::http::{Request, Response, StatusCode, header};
use tauri::{AppHandle, Manager, Runtime};
use tokio_util::sync::CancellationToken;

use crate::commands::CommandError;
use crate::state::{AppState, MAIN_WINDOW};

static READS: OnceLock<tokio::sync::Semaphore> = OnceLock::new();

#[derive(Clone)]
struct Lease {
    request_id: String,
    generation: u64,
    epoch: u64,
    file_id: String,
    cancel: CancellationToken,
    preview: Option<FilePreview>,
}
#[derive(Default)]
struct Current {
    generation: u64,
    active: Option<Lease>,
    closed: VecDeque<String>,
}
#[derive(Default)]
pub struct Previews(Mutex<Current>);

impl Previews {
    fn begin(
        &self,
        file_id: &str,
        request_id: &str,
        epoch: u64,
        cancel: &CancellationToken,
    ) -> Lease {
        let mut current = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(old) = current.active.take() {
            old.cancel.cancel();
        }
        current.generation += 1;
        let lease = Lease {
            request_id: request_id.to_owned(),
            generation: current.generation,
            epoch,
            file_id: file_id.to_owned(),
            cancel: cancel.child_token(),
            preview: None,
        };
        // IPC 异步打开可能尚未开始执行，同步关闭已经抵达；记住有界的取消关联值。
        if current.closed.iter().any(|id| id == request_id) {
            lease.cancel.cancel();
        }
        current.active = Some(lease.clone());
        lease
    }
    pub fn close(&self, generation: Option<u64>) {
        let mut current = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if (generation.is_none()
            || current
                .active
                .as_ref()
                .is_some_and(|v| Some(v.generation) == generation))
            && let Some(old) = current.active.take()
        {
            old.cancel.cancel();
        }
    }
    fn finish(&self, lease: &Lease, preview: FilePreview) -> bool {
        let mut current = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(active) = current
            .active
            .as_mut()
            .filter(|v| v.generation == lease.generation && !v.cancel.is_cancelled())
        {
            active.preview = Some(preview);
            true
        } else {
            false
        }
    }
    pub fn close_request(&self, request_id: &str) {
        let mut current = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if !current.closed.iter().any(|id| id == request_id) {
            if current.closed.len() >= 32 {
                current.closed.pop_front();
            }
            current.closed.push_back(request_id.to_owned());
        }
        if current
            .active
            .as_ref()
            .is_some_and(|v| v.request_id == request_id)
            && let Some(old) = current.active.take()
        {
            old.cancel.cancel();
        }
    }
    fn get(&self, generation: u64, id: &str, epoch: u64) -> Option<Lease> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .active
            .as_ref()
            .filter(|v| {
                v.generation == generation
                    && v.epoch == epoch
                    && v.file_id == id
                    && !v.cancel.is_cancelled()
            })
            .cloned()
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenedPreview {
    generation: u64,
    preview: FilePreview,
    src: Option<String>,
}

pub async fn open<R: Runtime>(
    state: &AppState<R>,
    file_id: &str,
    request_id: &str,
) -> Result<OpenedPreview, CommandError> {
    if state.device_id().is_none() {
        return Err(CommandError::local(
            "UNAUTHORIZED",
            "preview requires a signed-in account",
        ));
    }
    let epoch = *state.session_changes().borrow();
    if !assemble_protocol::primitives::is_valid_id(request_id)
        || !assemble_protocol::primitives::is_valid_id(file_id)
    {
        return Err(CommandError::local(
            "INVALID_INPUT",
            "invalid preview request identifier",
        ));
    }
    let lease = state
        .file_previews
        .begin(file_id, request_id, epoch, &state.session_cancel());
    let result = state
        .client
        .drive_preview(file_id, &RequestOptions::new(&lease.cancel))
        .await;
    if lease.cancel.is_cancelled() || *state.session_changes().borrow() != epoch {
        return Err(CommandError::local("CANCELLED", "preview session changed"));
    }
    let preview = result.map_err(|error| {
        state.file_previews.close(Some(lease.generation));
        if error.is_unauthorized() {
            crate::session::revoked(state);
        }
        CommandError::from(&error)
    })?;
    if !state.file_previews.finish(&lease, preview.clone()) {
        return Err(CommandError::local("CANCELLED", "preview closed"));
    }
    let src = matches!(
        preview.info.kind,
        PreviewKind::Image | PreviewKind::Audio | PreviewKind::Video
    )
    .then(|| {
        let origin = if cfg!(windows) {
            "http://assemble-preview.localhost"
        } else {
            "assemble-preview://localhost"
        };
        format!("{origin}/{}/{file_id}", lease.generation)
    });
    Ok(OpenedPreview {
        generation: lease.generation,
        preview,
        src,
    })
}

fn range(value: Option<&str>, size: u64, image: bool) -> Option<(u64, u64)> {
    if size == 0 {
        return None;
    }
    let limit = if image {
        16 * 1024 * 1024
    } else {
        PREVIEW_CHUNK_BYTES
    };
    let (start, end) = match value {
        Some(value) => {
            let (start, end) = value.strip_prefix("bytes=")?.split_once('-')?;
            if start.is_empty() {
                let suffix = end.parse::<u64>().ok()?;
                if suffix == 0 {
                    return None;
                }
                (size.saturating_sub(suffix), size - 1)
            } else {
                (
                    start.parse::<u64>().ok()?,
                    if end.is_empty() {
                        size - 1
                    } else {
                        end.parse::<u64>().ok()?.min(size - 1)
                    },
                )
            }
        }
        None => (0, size - 1),
    };
    (start <= end && start < size).then_some((start, end.min(start.saturating_add(limit - 1))))
}

fn response(status: StatusCode, bytes: Vec<u8>) -> Response<Vec<u8>> {
    let mut response = Response::new(bytes);
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    // 媒体跨 scheme 读取仅允许本应用来源；没有通用 fetch 或本机文件读取能力。
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        app_origin().parse().unwrap(),
    );
    response
}

fn app_origin() -> &'static str {
    if cfg!(debug_assertions) {
        "http://localhost:1420"
    } else if cfg!(windows) {
        "http://tauri.localhost"
    } else {
        "tauri://localhost"
    }
}

pub async fn read<R: Runtime>(app: &AppHandle<R>, request: Request<Vec<u8>>) -> Response<Vec<u8>> {
    let state = app.state::<AppState<R>>();
    let epoch = *state.session_changes().borrow();
    let parts: Vec<_> = request
        .uri()
        .path()
        .trim_start_matches('/')
        .split('/')
        .collect();
    let Some((generation, id)) = parts
        .first()
        .and_then(|v| v.parse::<u64>().ok())
        .zip(parts.get(1))
        .filter(|_| parts.len() == 2 && request.uri().query().is_none())
    else {
        return response(StatusCode::NOT_FOUND, Vec::new());
    };
    if request.method() != tauri::http::Method::GET
        || request
            .headers()
            .get(header::ORIGIN)
            .is_some_and(|v| v != app_origin())
    {
        return response(StatusCode::FORBIDDEN, Vec::new());
    }
    let Some(lease) = state.file_previews.get(generation, id, epoch) else {
        return response(StatusCode::NOT_FOUND, Vec::new());
    };
    let Some(preview) = &lease.preview else {
        return response(StatusCode::NOT_FOUND, Vec::new());
    };
    let info = &preview.info;
    if !matches!(
        info.kind,
        PreviewKind::Image | PreviewKind::Audio | PreviewKind::Video
    ) {
        return response(StatusCode::NOT_FOUND, Vec::new());
    }
    let Some((start, end)) = range(
        request
            .headers()
            .get(header::RANGE)
            .and_then(|v| v.to_str().ok()),
        info.size,
        info.kind == PreviewKind::Image,
    ) else {
        let mut out = response(StatusCode::RANGE_NOT_SATISFIABLE, Vec::new());
        out.headers_mut().insert(
            header::CONTENT_RANGE,
            format!("bytes */{}", info.size).parse().unwrap(),
        );
        return out;
    };
    // 自定义协议需要返回有界字节；限制并发，避免播放器的并行 Range 累积整份媒体。
    let Ok(_permit) = READS
        .get_or_init(|| tokio::sync::Semaphore::new(2))
        .acquire()
        .await
    else {
        return response(StatusCode::SERVICE_UNAVAILABLE, Vec::new());
    };
    let result = state
        .client
        .drive_preview_chunk(
            &lease.file_id,
            start,
            end,
            info.size,
            &RequestOptions::new(&lease.cancel),
        )
        .await;
    if lease.cancel.is_cancelled()
        || state
            .file_previews
            .get(generation, id, *state.session_changes().borrow())
            .is_none()
    {
        return response(StatusCode::NOT_FOUND, Vec::new());
    }
    match result {
        Ok(chunk) if Some(&chunk.mime_type) == info.mime_type.as_ref() => {
            let length = chunk.bytes.len();
            let mut out = response(StatusCode::PARTIAL_CONTENT, chunk.bytes);
            let headers = out.headers_mut();
            headers.insert(header::CONTENT_TYPE, chunk.mime_type.parse().unwrap());
            headers.insert(header::CONTENT_LENGTH, length.to_string().parse().unwrap());
            headers.insert(header::CONTENT_RANGE, chunk.content_range.parse().unwrap());
            headers.insert(header::ACCEPT_RANGES, "bytes".parse().unwrap());
            out
        }
        Err(ClientError::Http { status: 401, .. }) => {
            crate::session::revoked(&state);
            response(StatusCode::UNAUTHORIZED, Vec::new())
        }
        _ => response(StatusCode::NOT_FOUND, Vec::new()),
    }
}

pub fn register<R: Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    builder.register_asynchronous_uri_scheme_protocol(
        "assemble-preview",
        |context, request, responder| {
            if context.webview_label() != MAIN_WINDOW {
                responder.respond(response(StatusCode::FORBIDDEN, Vec::new()));
                return;
            }
            let app = context.app_handle().clone();
            tauri::async_runtime::spawn(async move {
                responder.respond(read(&app, request).await);
            });
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ranges_are_bounded_and_invalid_ranges_fail() {
        assert_eq!(
            range(None, 20_000_000, false),
            Some((0, PREVIEW_CHUNK_BYTES - 1))
        );
        assert_eq!(range(Some("bytes=10-19"), 100, false), Some((10, 19)));
        assert_eq!(range(Some("bytes=-10"), 100, false), Some((90, 99)));
        for bad in [
            "bytes=1-0",
            "bytes=100-",
            "bytes=0-1,4-5",
            "bytes=foo-2",
            "bytes=-0",
        ] {
            assert!(range(Some(bad), 100, false).is_none());
        }
    }
    #[test]
    fn closing_an_old_dialog_cannot_close_the_next_one() {
        let previews = Previews::default();
        let cancel = CancellationToken::new();
        let old = previews.begin("a", "r1", 1, &cancel);
        let next = previews.begin("b", "r2", 1, &cancel);
        assert!(old.cancel.is_cancelled());
        previews.close(Some(old.generation));
        assert!(previews.get(next.generation, "b", 1).is_some());
        assert!(previews.get(next.generation, "b", 2).is_none());
        cancel.cancel();
        assert!(previews.get(next.generation, "b", 1).is_none());
    }
    #[test]
    fn closing_before_the_description_arrives_cannot_reopen_a_lease() {
        let previews = Previews::default();
        let lease = previews.begin("file", "pending", 1, &CancellationToken::new());
        previews.close_request("pending");
        assert!(lease.cancel.is_cancelled());
        let preview = FilePreview {
            info: assemble_protocol::preview::PreviewInfo {
                kind: PreviewKind::Unsupported,
                mime_type: None,
                size: 0,
            },
            text: None,
            truncated: false,
        };
        assert!(!previews.finish(&lease, preview));
        assert!(previews.get(lease.generation, "file", 1).is_none());
        previews.close_request("not-started");
        let late = previews.begin("file", "not-started", 1, &CancellationToken::new());
        assert!(late.cancel.is_cancelled());
        assert!(previews.get(late.generation, "file", 1).is_none());
    }
}
