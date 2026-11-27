//! 远程编排：本机批准才创建工作线程，断线、退出或取消都显式释放输入。
use crate::{commands::CommandError, state::AppState};
use assemble_client::{Client, RequestOptions};
use assemble_protocol::remote::{RemotePhase, RemoteSession, RemoteView};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;
use tauri::{AppHandle, Manager};
use tokio_util::sync::CancellationToken;

struct Worker {
    id: String,
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}
#[derive(Default)]
pub struct Runtime {
    worker: Mutex<Option<Worker>>,
}
impl Runtime {
    pub fn cancel_all(&self) {
        if let Some(w) = self
            .worker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            w.cancel.cancel();
        }
    }
    pub async fn stop(&self, id: Option<&str>) {
        let worker = {
            let mut slot = self.worker.lock().unwrap_or_else(PoisonError::into_inner);
            if slot
                .as_ref()
                .is_some_and(|w| id.is_none_or(|id| id == w.id))
            {
                slot.take()
            } else {
                None
            }
        };
        if let Some(worker) = worker {
            worker.cancel.cancel();
            let _ = worker.task.await;
        }
    }
}
pub fn options(cancel: &CancellationToken) -> RequestOptions<'_> {
    RequestOptions {
        cancel,
        timeout: Some(Duration::from_secs(2)),
    }
}

pub async fn approve(app: &AppHandle, id: String) -> Result<RemoteSession, CommandError> {
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = (app, id);
        Err(CommandError::local(
            "REMOTE_UNSUPPORTED",
            "remote hosting requires Windows or macOS",
        ))
    }
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        let state = app.state::<AppState>();
        let cancel = state.session_cancel().child_token();
        let sessions = state
            .client
            .remote_sessions(&options(&cancel))
            .await
            .map_err(|e| CommandError::from(&e))?;
        let own = state.device_id();
        if !sessions.sessions.iter().any(|s| {
            s.id == id
                && Some(s.host_id.as_str()) == own.as_deref()
                && s.phase == RemotePhase::Pending
        }) {
            return Err(CommandError::local(
                "REMOTE_ENDED",
                "no pending local request",
            ));
        }
        let (ready, answer) = tokio::sync::oneshot::channel();
        {
            let mut slot = state
                .remote_runtime
                .worker
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if slot.as_ref().is_some_and(|w| !w.task.is_finished()) {
                return Err(CommandError::local(
                    "REMOTE_BUSY",
                    "a local worker is active",
                ));
            }
            let client = state.client.clone();
            let worker_id = id.clone();
            let stop = cancel.clone();
            let worker_app = app.clone();
            let task = tokio::task::spawn_blocking(move || {
                host_worker(worker_app, client, worker_id, stop, ready);
            });
            *slot = Some(Worker { id, cancel, task });
        }
        answer
            .await
            .map_err(|_| CommandError::local("REMOTE_CAPTURE_FAILED", "host worker stopped"))?
    }
}
#[cfg(any(target_os = "windows", target_os = "macos"))]
fn host_worker(
    app: AppHandle,
    client: Client,
    id: String,
    cancel: CancellationToken,
    ready: tokio::sync::oneshot::Sender<Result<RemoteSession, CommandError>>,
) {
    let mut host = match assemble_remote::native::Host::open() {
        Ok(host) => host,
        Err(error) => {
            let _ = ready.send(Err(CommandError::local(error.code(), error.to_string())));
            return;
        }
    };
    // 创建能力不等于批准：服务端确认之前，不调用采集与输入方法。
    tauri::async_runtime::block_on(async {
        let session = match client.remote_accept(&id, &options(&cancel)).await {
            Ok(session) => session,
            Err(error) => {
                let _ = ready.send(Err(CommandError::from(&error)));
                return;
            }
        };
        if ready.send(Ok(session)).is_err() {
            cancel.cancel();
        }
        let mut after = 0_u32;
        let mut native_error = None;
        loop {
            if cancel.is_cancelled() {
                break;
            }
            let Ok(poll) = client.remote_poll(&id, after, 0, &options(&cancel)).await else {
                break;
            };
            if poll.session.phase != RemotePhase::Active {
                break;
            }
            let mut failed = false;
            for event in poll.inputs {
                if event.sequence <= after {
                    continue;
                }
                if after.checked_add(1) != Some(event.sequence) || cancel.is_cancelled() {
                    failed = true;
                    break;
                }
                if let Err(error) = host.apply(&event.input) {
                    native_error = Some(error);
                    failed = true;
                    break;
                }
                after = event.sequence;
            }
            if failed || cancel.is_cancelled() {
                break;
            }
            let frame = match host.frame() {
                Ok(frame) => frame,
                Err(error) => {
                    native_error = Some(error);
                    break;
                }
            };
            if cancel.is_cancelled()
                || client
                    .remote_frame(&id, &frame, &options(&cancel))
                    .await
                    .is_err()
            {
                break;
            }
            tokio::select! {biased;()=cancel.cancelled()=>break,()=tokio::time::sleep(Duration::from_millis(100))=>{}}
        }
        // 释放先于网络回执，网络已经断开也必须恢复本机输入。
        if host.release_all().is_err() {
            tracing::warn!("remote input release failed");
        }
        if let Some(error) = native_error
            && !cancel.is_cancelled()
        {
            let message = if error.code() == "REMOTE_CAPTURE_FAILED" {
                "远程屏幕采集失败，请检查屏幕录制权限后重新连接。"
            } else {
                "远程输入已停止，请检查辅助功能权限后重新连接。"
            };
            app.state::<AppState>()
                .set_notice(Some(crate::state::Notice::error(message)));
        }
        let cleanup = CancellationToken::new();
        let _ = client.remote_end(&id, &options(&cleanup)).await;
    });
}
pub fn start(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        loop {
            let cancel = state.session_cancel();
            let own = state.device_id();
            if own.is_some() {
                if let Ok(view) = state.client.remote_sessions(&options(&cancel)).await
                    && !cancel.is_cancelled()
                    && state.device_id() == own
                {
                    let incoming = view.sessions.iter().any(|s| {
                        s.host_id == own.as_deref().unwrap_or_default()
                            && s.phase == RemotePhase::Pending
                            && !state
                                .snapshot()
                                .remote
                                .sessions
                                .iter()
                                .any(|old| old.id == s.id)
                    });
                    state.set_remote(view);
                    if incoming {
                        crate::state::show_main(&app);
                    }
                }
            } else {
                state.set_remote(RemoteView::default());
            }
            tokio::select! {()=state.lifetime.cancelled()=>return,()=tokio::time::sleep(Duration::from_secs(1))=>{}}
        }
    });
}

pub fn close_window(app: &AppHandle) {
    let state = app.state::<AppState>();
    state.remote_runtime.cancel_all();
    let sessions = state.snapshot().remote.sessions;
    let cancel = state.session_cancel();
    let client = state.client.clone();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        app.state::<AppState>().remote_runtime.stop(None).await;
        for session in sessions {
            let _ = client.remote_end(&session.id, &options(&cancel)).await;
        }
    });
}
