//! 已在本机批准的被控会话：输入与画面各自只有一个在途请求，慢画面不阻塞输入。
use crate::{Client, ClientError, RequestOptions};
use assemble_protocol::remote::{RemoteEndReason, RemoteFrame, RemotePhase};
use assemble_remote::worker::{HostLink, HostWorker, WorkerError};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[cfg(test)]
mod tests;

#[derive(Debug, thiserror::Error)]
pub enum HostError<E: std::error::Error + 'static> {
    #[error("remote transport failed: {0}")]
    Transport(#[from] ClientError),
    #[error("native capture failed: {0}")]
    Capture(WorkerError<E>),
    #[error("native input failed: {0}")]
    Input(WorkerError<E>),
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HostStats {
    pub frames_sent: u64,
    pub frames_unchanged: u64,
    pub inputs_applied: u32,
}

fn options(cancel: &CancellationToken) -> RequestOptions<'_> {
    RequestOptions {
        cancel,
        timeout: Some(Duration::from_secs(2)),
    }
}
fn same_picture(a: &RemoteFrame, b: &RemoteFrame) -> bool {
    a.video.is_none()
        && b.video.is_none()
        && a.display_id == b.display_id
        && a.width == b.width
        && a.height == b.height
        && a.jpeg == b.jpeg
        && a.displays == b.displays
        && a.quality == b.quality
        && a.requested_quality == b.requested_quality
}
impl Client {
    /// 此方法只在本机显式批准和 `remote_accept` 成功之后调用；返回前等待原生输入释放。
    pub async fn run_remote_host<E: std::error::Error + Send + 'static>(
        &self,
        id: &str,
        worker: HostWorker<E>,
        cancel: &CancellationToken,
    ) -> Result<HostStats, HostError<E>> {
        let stop = cancel.child_token();
        let link = worker.link();
        let (ready, prepared) = tokio::sync::oneshot::channel();
        let mut stats = HostStats::default();
        let result = {
            let input = self.host_inputs(id, link.clone(), &stop, ready, &mut stats.inputs_applied);
            let audio = self.host_audio(id, link.clone(), &stop);
            let frames = self.host_frames(
                id,
                link,
                &stop,
                prepared,
                &mut stats.frames_sent,
                &mut stats.frames_unchanged,
            );
            tokio::pin!(input, frames, audio);
            tokio::select! {biased;
                ()=stop.cancelled()=>Ok(()),
                result=&mut input=>result,
                result=&mut frames=>result,
                result=&mut audio=>result,
            }
        };
        let result = match result {
            Err(HostError::Transport(error)) if error.remote_code() == Some("REMOTE_ENDED") => {
                Ok(())
            }
            other => other,
        };
        stop.cancel();
        let release = worker.shutdown().await.map_err(HostError::Input);
        if release.is_err() {
            tracing::warn!("remote native input cleanup failed");
        }
        let reason = match result.as_ref().err().or_else(|| release.as_ref().err()) {
            Some(HostError::Capture(_)) => RemoteEndReason::CaptureFailed,
            Some(HostError::Input(_)) => RemoteEndReason::InputFailed,
            Some(HostError::Transport(_)) => RemoteEndReason::Disconnected,
            None => RemoteEndReason::Stopped,
        };
        // 原生释放已完成才发回执；断网也不能延迟松开键鼠。
        let cleanup = CancellationToken::new();
        let _ = self
            .remote_end_with_reason(id, reason, &options(&cleanup))
            .await;
        result?;
        release?;
        Ok(stats)
    }
    async fn host_inputs<E: std::error::Error + Send + 'static>(
        &self,
        id: &str,
        link: HostLink<E>,
        stop: &CancellationToken,
        ready: tokio::sync::oneshot::Sender<()>,
        after: &mut u32,
    ) -> Result<(), HostError<E>> {
        let mut ready = Some(ready);
        let mut clipboard_result: Option<assemble_protocol::remote::ClipboardResult> = None;
        loop {
            let poll = self.remote_poll(id, *after, 0, &options(stop)).await?;
            if poll.session.phase != RemotePhase::Active {
                return Ok(());
            }
            if let Some(ready) = ready.take() {
                let _ = ready.send(());
            }
            if !poll.inputs.is_empty() {
                *after = link.inputs(poll.inputs).await.map_err(HostError::Input)?;
            }
            if let Some(task) = poll.clipboard {
                if clipboard_result
                    .as_ref()
                    .is_none_or(|result| result.id != task.request.id)
                {
                    clipboard_result = Some(link.clipboard(task).await.map_err(HostError::Input)?);
                }
                if let Some(result) = &clipboard_result {
                    self.remote_clipboard_result(id, result, &options(stop))
                        .await?;
                }
            }
            tokio::select! {biased;()=stop.cancelled()=>return Ok(()),()=tokio::time::sleep(Duration::from_millis(50))=>{}}
        }
    }
    async fn host_frames<E: std::error::Error + Send + 'static>(
        &self,
        id: &str,
        link: HostLink<E>,
        stop: &CancellationToken,
        ready: tokio::sync::oneshot::Receiver<()>,
        sent: &mut u64,
        unchanged: &mut u64,
    ) -> Result<(), HostError<E>> {
        if ready.await.is_err() {
            return Ok(());
        }
        let mut previous: Option<RemoteFrame> = None;
        loop {
            let started = std::time::Instant::now();
            let frame = link.frame().await.map_err(HostError::Capture)?;
            self.remote_security.cursor(
                id,
                assemble_protocol::remote::CursorUpdate {
                    display_id: frame.display_id,
                    position: frame.cursor,
                },
            )?;
            let video = frame.video.is_some();
            if previous
                .as_ref()
                .is_some_and(|old| same_picture(old, &frame))
            {
                *unchanged += 1;
            } else {
                self.remote_frame(id, &frame, &options(stop)).await?;
                *sent += 1;
                previous = Some(frame);
            }
            link.feedback(started.elapsed())
                .await
                .map_err(HostError::Capture)?;
            let delay = if video {
                Duration::from_millis(33).saturating_sub(started.elapsed())
            } else {
                Duration::from_millis(100)
            };
            tokio::select! {biased;()=stop.cancelled()=>return Ok(()),()=tokio::time::sleep(delay)=>{}}
        }
    }
    async fn host_audio<E: std::error::Error + Send + 'static>(
        &self,
        id: &str,
        link: HostLink<E>,
        stop: &CancellationToken,
    ) -> Result<(), HostError<E>> {
        let mut previous = assemble_protocol::remote::AudioState::Off;
        loop {
            let audio = link.audio().await.map_err(HostError::Input)?;
            if audio.state != previous || !audio.packets.is_empty() {
                self.remote_audio_upload(id, &audio, &options(stop)).await?;
                previous = audio.state;
            }
            tokio::select! {biased;()=stop.cancelled()=>return Ok(()),()=tokio::time::sleep(Duration::from_millis(20))=>{}}
        }
    }
}
