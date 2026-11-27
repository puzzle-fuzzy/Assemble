//! 云盘下载独立于设备回执：检查点先持久化，完整校验发布后才记完成。
use super::{Outcome, claim, pull};
use crate::{
    registry::{Delivery, TransferFailure},
    session,
    state::AppState,
};
use assemble_client::{ClientError, RequestOptions};
use assemble_device::resume::Checkpoint;
use assemble_protocol::drive::DriveFile;
use assemble_transfer::TransferError;
use tokio_util::sync::CancellationToken;

enum Failure {
    Remote(ClientError),
    Local(TransferError),
    Target,
    Stopped,
}
impl From<ClientError> for Failure {
    fn from(e: ClientError) -> Self {
        Self::Remote(e)
    }
}
impl From<TransferError> for Failure {
    fn from(e: TransferError) -> Self {
        Self::Local(e)
    }
}

pub(super) async fn receive<R: tauri::Runtime>(
    state: &AppState<R>,
    id: &str,
    file: &DriveFile,
    epoch: u64,
    login: &CancellationToken,
    cancel: &CancellationToken,
) {
    let result = download(state, id, file, epoch, login, cancel).await;
    if login.is_cancelled() || *state.session_changes().borrow() != epoch {
        return;
    }
    match result {
        Ok(Some(received)) => {
            let handle = state.fresh_handle();
            let recorded = state.transfer_session(epoch, login, |r| {
                r.receive_completed(id, &received.name, received.path.clone(), handle);
                r.views()
                    .iter()
                    .any(|t| t.id == id && t.phase == crate::registry::TransferPhase::Complete)
            });
            if recorded == Some(true) {
                state.notify_received(handle, crate::registry::DRIVE_NAME, &received.name, false);
            }
        }
        Ok(None) | Err(Failure::Stopped | Failure::Remote(ClientError::Cancelled)) => {}
        Err(Failure::Remote(error)) if error.is_unauthorized() => session::revoked(state),
        Err(Failure::Remote(error)) if error.is_retryable() => {
            state.transfer_session(epoch, login, |r| {
                r.retry_receive(id, std::time::Instant::now());
            });
        }
        Err(error) => {
            let reason = match error {
                Failure::Target => TransferFailure::DestinationChanged,
                Failure::Remote(ClientError::Http { status: 404, .. }) => TransferFailure::Vanished,
                Failure::Local(ref error) => assemble_device::ReceiveFailure::from(error).into(),
                _ => TransferFailure::VerificationFailed,
            };
            state.transfer_session(epoch, login, |r| r.receive_failed(id, reason));
        }
    }
}
fn current<R: tauri::Runtime>(
    state: &AppState<R>,
    id: &str,
    epoch: u64,
    login: &CancellationToken,
    cancel: &CancellationToken,
) -> bool {
    !cancel.is_cancelled()
        && state.transfer_session(epoch, login, |r| {
            r.views()
                .iter()
                .any(|t| t.id == id && t.phase == crate::registry::TransferPhase::InProgress)
        }) == Some(true)
}
async fn download<R: tauri::Runtime>(
    state: &AppState<R>,
    id: &str,
    file: &DriveFile,
    epoch: u64,
    login: &CancellationToken,
    cancel: &CancellationToken,
) -> Result<Option<assemble_device::Received>, Failure> {
    let options = RequestOptions::new(cancel);
    let existing = state
        .transfer_session(epoch, login, |r| r.download_checkpoint(id))
        .flatten();
    let inbox = assemble_device::resolve(
        state.settings().receive_directory.as_deref(),
        state.downloads_dir(),
    )
    .await
    .map_err(|_| Failure::Target)?;
    if inbox.fell_back {
        return Err(Failure::Target);
    }
    let mut checkpoint = if let Some(cp) = existing {
        if cp.directory != inbox.directory || cp.size != file.size || cp.sha256 != file.sha256 {
            return Err(Failure::Target);
        }
        cp.verify().await?;
        cp
    } else {
        // 先探测强区间能力；旧服务保持完整下载，不把普通响应混入检查点。
        let first = first_chunk(&state.client, file, &options).await?;
        let Some(first) = first else {
            if state.transfer_session(epoch, login, |r| r.mark_legacy(id)) != Some(true) {
                return Err(Failure::Stopped);
            }
            return legacy(state, file, cancel).await;
        };
        if !current(state, id, epoch, login, cancel) {
            return Err(Failure::Stopped);
        }
        let mut cp =
            Checkpoint::create(&inbox.directory, &file.name, file.size, &file.sha256).await?;
        if state.transfer_session(epoch, login, |r| r.remember_download(id, cp.clone()))
            != Some(true)
        {
            let _ = cp.cleanup().await;
            return Err(Failure::Stopped);
        }
        if !first.is_empty() {
            cp.append(&first).await?;
            if state.transfer_session(epoch, login, |r| r.remember_download(id, cp.clone()))
                != Some(true)
            {
                return Err(Failure::Stopped);
            }
        }
        cp
    };
    while checkpoint.offset() < file.size {
        if !current(state, id, epoch, login, cancel) {
            return Err(Failure::Stopped);
        }
        let bytes = state
            .client
            .drive_chunk(file, checkpoint.offset(), &options)
            .await?
            .ok_or(Failure::Remote(ClientError::InvalidResponse))?;
        if !current(state, id, epoch, login, cancel) {
            return Err(Failure::Stopped);
        }
        checkpoint.append(&bytes).await?;
        if state.transfer_session(epoch, login, |r| {
            r.remember_download(id, checkpoint.clone())
        }) != Some(true)
        {
            return Err(Failure::Stopped);
        }
    }
    let active = state.client.drive_metadata(&file.id, &options).await?;
    if active.id != file.id || active.size != file.size || active.sha256 != file.sha256 {
        return Err(Failure::Remote(ClientError::InvalidResponse));
    }
    let inbox = assemble_device::resolve(
        state.settings().receive_directory.as_deref(),
        state.downloads_dir(),
    )
    .await
    .map_err(|_| Failure::Target)?;
    if inbox.fell_back || inbox.directory != checkpoint.directory {
        return Err(Failure::Target);
    }
    if !current(state, id, epoch, login, cancel) {
        return Err(Failure::Stopped);
    }
    Ok(Some(checkpoint.publish_with_cancel(cancel).await?))
}

pub(super) async fn cleanup<R: tauri::Runtime>(state: &AppState<R>, id: &str) {
    let epoch = *state.session_changes().borrow();
    let login = state.session_cancel();
    let Some(cp) = state
        .transfer_session(epoch, &login, |r| r.download_checkpoint(id))
        .flatten()
    else {
        return;
    };
    let result = cp.cleanup().await;
    state.transfer_session(epoch, &login, |r| {
        if result.is_ok() {
            r.download_cleaned(id);
        } else {
            r.retry_download_cleanup(id);
        }
    });
}

async fn legacy<R: tauri::Runtime>(
    state: &AppState<R>,
    file: &DriveFile,
    cancel: &CancellationToken,
) -> Result<Option<assemble_device::Received>, Failure> {
    match pull(state, &claim(&Delivery::FromDrive(file.clone())), cancel).await {
        Outcome::Saved { received, .. } => Ok(Some(received)),
        Outcome::Retry => Err(Failure::Remote(ClientError::Network)),
        Outcome::Unauthorized => Err(Failure::Remote(ClientError::Http {
            status: 401,
            remote_code: None,
        })),
        Outcome::Gone | Outcome::Changed => Err(Failure::Remote(ClientError::Http {
            status: 404,
            remote_code: None,
        })),
        Outcome::Interrupted => Err(Failure::Stopped),
        Outcome::Failed(_) => Err(Failure::Local(TransferError::HashMismatch)),
    }
}

async fn first_chunk(
    client: &assemble_client::Client,
    file: &DriveFile,
    options: &RequestOptions<'_>,
) -> Result<Option<Vec<u8>>, ClientError> {
    if file.size > 0 {
        return client.drive_chunk(file, 0, options).await;
    }
    match client.drive_metadata(&file.id, options).await {
        Ok(active) if active.size == 0 && active.sha256 == file.sha256 => Ok(Some(vec![])),
        Err(ClientError::Http { status: 404, .. }) => Ok(None),
        Err(error) => Err(error),
        Ok(_) => Err(ClientError::InvalidResponse),
    }
}
