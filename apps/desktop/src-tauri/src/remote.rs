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
    transition: tokio::sync::Mutex<()>,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    playback: Mutex<Option<(String, assemble_remote::native::Playback)>>,
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
        // 先取消，再等前一次批准/收尾；新会话不能抢在旧会话松键之前开始。
        if let Some(worker) = self
            .worker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            && id.is_none_or(|id| id == worker.id)
        {
            worker.cancel.cancel();
        }
        let _transition = self.transition.lock().await;
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
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        {
            let mut playback = self.playback.lock().unwrap_or_else(PoisonError::into_inner);
            if playback
                .as_ref()
                .is_some_and(|(session, _)| id.is_none_or(|id| id == session))
            {
                *playback = None;
            }
        }
    }
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    pub fn decode_audio(
        &self,
        id: &str,
        audio: assemble_protocol::remote::RemoteAudio,
    ) -> Result<assemble_protocol::remote::AudioPlayback, CommandError> {
        let mut playback = self.playback.lock().unwrap_or_else(PoisonError::into_inner);
        if playback.as_ref().is_none_or(|(session, _)| session != id) {
            *playback = Some((
                id.into(),
                assemble_remote::native::Playback::open().map_err(|_| {
                    CommandError::local("REMOTE_AUDIO_FAILED", "audio decoder unavailable")
                })?,
            ));
        }
        playback
            .as_mut()
            .ok_or_else(|| CommandError::local("REMOTE_AUDIO_FAILED", "audio decoder unavailable"))?
            .1
            .decode(audio)
            .map_err(|_| CommandError::local("REMOTE_AUDIO_FAILED", "audio decoding failed"))
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
        let _transition = state.remote_runtime.transition.lock().await;
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
            let task = tokio::spawn(async move {
                host_worker(worker_app, client, worker_id, stop, ready).await;
            });
            *slot = Some(Worker { id, cancel, task });
        }
        answer
            .await
            .map_err(|_| CommandError::local("REMOTE_CAPTURE_FAILED", "host worker stopped"))?
    }
}
#[cfg(any(target_os = "windows", target_os = "macos"))]
async fn host_worker(
    app: AppHandle,
    client: Client,
    id: String,
    cancel: CancellationToken,
    ready: tokio::sync::oneshot::Sender<Result<RemoteSession, CommandError>>,
) {
    use assemble_client::remote_host::HostError;
    use assemble_remote::worker::{HostWorker, WorkerError};
    let worker =
        match HostWorker::start(assemble_remote::native::Host::open, cancel.child_token()).await {
            Ok(worker) => worker,
            Err(error) => {
                let code = match &error {
                    WorkerError::Backend(error) => error.code(),
                    WorkerError::Stopped => "CANCELLED",
                    _ => "REMOTE_INPUT_FAILED",
                };
                let _ = ready.send(Err(CommandError::local(
                    code,
                    "native remote worker could not start",
                )));
                return;
            }
        };
    // 原生线程准备好以后仍需明确批准，网络驱动首先确认 active 才提交采集任务。
    let session = match client.remote_accept(&id, &options(&cancel)).await {
        Ok(session) => session,
        Err(error) => {
            let _ = worker.shutdown().await;
            let _ = ready.send(Err(CommandError::from(&error)));
            let cleanup = CancellationToken::new();
            let _ = client.remote_end(&id, &options(&cleanup)).await;
            return;
        }
    };
    if ready.send(Ok(session)).is_err() {
        cancel.cancel();
    }
    let result = client.run_remote_host(&id, worker, &cancel).await;
    if !cancel.is_cancelled() {
        let message = match result {
            Err(HostError::Capture(_)) => Some("远程屏幕采集失败，请检查屏幕录制权限后重新连接。"),
            Err(HostError::Input(_)) => Some("远程输入已停止，请检查辅助功能权限后重新连接。"),
            Err(HostError::Transport(_)) => Some("远程连接中断，键鼠控制已停止，请重新连接。"),
            Ok(_) => None,
        };
        if let Some(message) = message {
            app.state::<AppState>()
                .set_notice(Some(crate::state::Notice::error(message)));
        }
    }
}
pub fn start(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        loop {
            let cancel = state.session_cancel();
            let own = state.device_id();
            if own.is_some() {
                if let Ok(mut view) = state.client.remote_sessions(&options(&cancel)).await
                    && !cancel.is_cancelled()
                    && state.device_id() == own
                {
                    for session in &mut view.sessions {
                        if session.host_id == own.as_deref().unwrap_or_default()
                            && session.phase == RemotePhase::Pending
                        {
                            let _ = state
                                .client
                                .remote_prepare_host(session, &options(&cancel))
                                .await;
                            session.verification = state.client.remote_verification(&session.id);
                        }
                    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    #[tokio::test]
    async fn stopping_keeps_new_sessions_out_until_native_cleanup_has_finished() {
        let runtime = Arc::new(Runtime::default());
        let cancel = CancellationToken::new();
        let until = cancel.clone();
        let (release, cleanup) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            until.cancelled().await;
            let _ = cleanup.await;
        });
        *runtime.worker.lock().unwrap() = Some(Worker {
            id: "old".into(),
            cancel: cancel.clone(),
            task,
        });
        let stopped = tokio::spawn({
            let runtime = runtime.clone();
            async move {
                runtime.stop(Some("old")).await;
            }
        });
        cancel.cancelled().await;
        // stop 在等待工作线程时仍持有会话切换权。
        assert!(runtime.transition.try_lock().is_err());
        release.send(()).unwrap();
        stopped.await.unwrap();
        assert!(runtime.transition.try_lock().is_ok());
        assert!(runtime.worker.lock().unwrap().is_none());
    }
}
