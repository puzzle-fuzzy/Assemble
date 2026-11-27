//! 助手与浏览器共用认证通道，但只能提交自己的预览、开始与查询。
use super::{BridgeState, INVALID_REQUEST};
use assemble_protocol::browser::BrowserDownload;
use assemble_protocol::downloads::{
    AssistantDownloadReply as Reply, AssistantDownloadStart, DownloadInitiator,
};
use assemble_protocol::primitives::is_valid_id;
use axum::{
    Json, Router,
    extract::{Path, State, rejection::JsonRejection},
    routing::{get, post},
};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(super) fn router() -> Router<Arc<BridgeState>> {
    Router::new()
        .route("/v1/assistant/downloads/preview", post(preview))
        .route("/v1/assistant/downloads/start", post(start))
        .route("/v1/downloads/{id}", get(status))
        .route("/v1/downloads/{id}/cancel", post(cancel))
        .route("/v1/downloads/{id}/pause", post(pause))
        .route("/v1/downloads/{id}/resume", post(resume))
}

async fn preview(
    State(state): State<Arc<BridgeState>>,
    body: Result<Json<BrowserDownload>, JsonRejection>,
) -> Json<Reply> {
    let Ok(Json(request)) = body else {
        return Json(Reply::failure(INVALID_REQUEST));
    };
    if request.via != Some(DownloadInitiator::Assistant) || !valid(&request.request_id) {
        return Json(Reply::failure(INVALID_REQUEST));
    }
    Json(match state.manager.prepare_assistant(request.input).await {
        Ok(preview) => Reply {
            preview: Some(preview),
            ..Reply::success()
        },
        Err(error) => Reply::failure(error.code()),
    })
}

fn valid(id: &str) -> bool {
    id.len() <= 64 && is_valid_id(id)
}

async fn start(
    State(state): State<Arc<BridgeState>>,
    body: Result<Json<AssistantDownloadStart>, JsonRejection>,
) -> Json<Reply> {
    let Ok(Json(request)) = body else {
        return Json(Reply::failure(INVALID_REQUEST));
    };
    if !valid(&request.request_id) || !valid(&request.preview_id) || !valid(&request.variant_id) {
        return Json(Reply::failure(INVALID_REQUEST));
    }
    if let Err(error) = state.manager.ensure_assistant_allowed() {
        return Json(Reply::failure(error.code()));
    }
    // 与扩展不同的命名空间；相同键重试只能返回同一任务，不能吞掉参数变化。
    let key = format!("assistant:{}", request.request_id);
    let digest = format!(
        "{}:{}:{:?}",
        request.preview_id, request.variant_id, request.file_ids
    );
    let mut accepted = state.accepted.lock().await;
    accepted.retain(|_, (_, at, _)| at.elapsed() < Duration::from_mins(10));
    if let Some((previous, _, task)) = accepted.get(&key) {
        return Json(if previous == &digest {
            Reply {
                task_id: Some(task.clone()),
                ..Reply::success()
            }
        } else {
            Reply::failure(INVALID_REQUEST)
        });
    }
    if accepted.len() >= 128 {
        return Json(Reply::failure("DOWNLOAD_BUSY"));
    }
    Json(
        match state
            .manager
            .start_assistant_selected(&request.preview_id, &request.variant_id, &request.file_ids)
            .await
        {
            Ok(id) => {
                accepted.insert(key, (digest, Instant::now(), id.clone()));
                state.manager.focus_browser_task(&id);
                (state.selected)();
                Reply {
                    task_id: Some(id),
                    ..Reply::success()
                }
            }
            Err(error) => Reply::failure(error.code()),
        },
    )
}

async fn status(State(state): State<Arc<BridgeState>>, Path(id): Path<String>) -> Json<Reply> {
    Json(if valid(&id) {
        match state.manager.assistant_task(&id) {
            Ok(task) => Reply {
                task: Some(task),
                ..Reply::success()
            },
            Err(error) => Reply::failure(error.code()),
        }
    } else {
        Reply::failure(INVALID_REQUEST)
    })
}

async fn cancel(State(state): State<Arc<BridgeState>>, Path(id): Path<String>) -> Json<Reply> {
    if !valid(&id) {
        return Json(Reply::failure(INVALID_REQUEST));
    }
    if let Err(error) = state.manager.assistant_task(&id) {
        return Json(Reply::failure(error.code()));
    }
    Json(match state.manager.cancel_selected(&id).await {
        Ok(()) => Reply::success(),
        Err(error) => Reply::failure(error.code()),
    })
}
async fn pause(State(state): State<Arc<BridgeState>>, Path(id): Path<String>) -> Json<Reply> {
    if !valid(&id) {
        return Json(Reply::failure(INVALID_REQUEST));
    }
    if let Err(error) = state.manager.assistant_task(&id) {
        return Json(Reply::failure(error.code()));
    }
    Json(match state.manager.pause(&id).await {
        Ok(()) => Reply::success(),
        Err(error) => Reply::failure(error.code()),
    })
}
async fn resume(State(state): State<Arc<BridgeState>>, Path(id): Path<String>) -> Json<Reply> {
    if !valid(&id) {
        return Json(Reply::failure(INVALID_REQUEST));
    }
    if let Err(error) = state.manager.assistant_task(&id) {
        return Json(Reply::failure(error.code()));
    }
    Json(match state.manager.resume(&id).await {
        Ok(()) => Reply::success(),
        Err(error) => Reply::failure(error.code()),
    })
}
