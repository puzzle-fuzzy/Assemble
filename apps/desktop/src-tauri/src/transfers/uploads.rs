//! 续传只持有当前进程内的源文件；重开应用的恢复由后续独立私有任务日志承担。
use crate::{
    registry::{Destination, Outgoing},
    session,
    state::AppState,
};
use assemble_client::{ClientError, RequestOptions, TransferOptions};
use assemble_protocol::uploads::Phase;
use tokio_util::sync::CancellationToken;

pub(super) async fn send<R: tauri::Runtime>(
    state: &AppState<R>,
    id: &str,
    outgoing: &Outgoing,
    epoch: u64,
    login: &CancellationToken,
    cancel: &CancellationToken,
) -> Result<(), ClientError> {
    let options = RequestOptions {
        cancel,
        timeout: Some(std::time::Duration::from_secs(60)),
    };
    let input = if let Some(input) = &outgoing.upload {
        Some(input.as_ref().clone())
    } else if state.client.upload_supported(&options).await? {
        let parent = match &outgoing.destination {
            Destination::Drive => "root",
            Destination::DriveFolder(parent) => parent,
            Destination::Device(_) => return Err(ClientError::InvalidRequest),
        };
        let input = state
            .client
            .upload_prepare(&outgoing.file, parent, &options)
            .await?;
        if state.transfer_session(epoch, login, |r| r.remember_upload(id, input.clone()))
            != Some(true)
        {
            return Err(ClientError::Cancelled);
        }
        Some(input)
    } else {
        None
    };
    if state.transfer_session(epoch, login, |r| r.may_send(id)) != Some(true) {
        return Err(ClientError::Cancelled);
    }
    let mut report = |progress: assemble_client::TransferProgress| {
        state.transfer_session(epoch, login, |registry| {
            registry.progress(id, progress.fraction().min(0.99));
        });
    };
    if let Some(input) = input {
        if cancel.is_cancelled() {
            return Err(ClientError::Cancelled);
        }
        state
            .client
            .upload_continue(&input, &outgoing.file, &options, Some(&mut report))
            .await?;
    } else {
        match &outgoing.destination {
            Destination::Drive => {
                state
                    .client
                    .drive_upload(
                        &outgoing.file.name(),
                        &outgoing.file,
                        TransferOptions::new(cancel),
                        Some(&mut report),
                    )
                    .await?;
            }
            Destination::DriveFolder(parent) => {
                state
                    .client
                    .drive_tree_upload(
                        parent,
                        &outgoing.file.name(),
                        &outgoing.file,
                        TransferOptions::new(cancel),
                        Some(&mut report),
                    )
                    .await?;
            }
            Destination::Device(_) => return Err(ClientError::InvalidRequest),
        }
    }
    Ok(())
}
pub(super) async fn abort<R: tauri::Runtime>(state: &AppState<R>, id: &str) {
    let epoch = *state.session_changes().borrow();
    let login = state.session_cancel();
    let Some(key) = state
        .transfer_session(epoch, &login, |r| r.upload_to_abort(id))
        .flatten()
    else {
        return;
    };
    let result = state
        .client
        .upload_cancel_request(&key, &RequestOptions::new(&login))
        .await;
    if login.is_cancelled() || *state.session_changes().borrow() != epoch {
        return;
    }
    match result {
        Ok(upload) if upload.phase == Phase::Completed && upload.file.is_some() => {
            if state.transfer_session(epoch, &login, |r| r.drive_uploaded(id)) == Some(true) {
                super::refresh_drive(state).await;
            }
        }
        Ok(upload) if matches!(upload.phase, Phase::Cancelled | Phase::Expired) => {
            state.transfer_session(epoch, &login, |r| r.upload_aborted(id));
        }
        Ok(_) => {
            state.transfer_session(epoch, &login, |r| {
                r.retry_upload_abort(id, std::time::Instant::now());
            });
        }
        Err(ClientError::Cancelled) => {}
        Err(e) if e.is_unauthorized() => session::revoked(state),
        Err(ClientError::Http {
            status: 404 | 409, ..
        }) => {
            state.transfer_session(epoch, &login, |r| r.settled(id));
        }
        Err(_) => {
            state.transfer_session(epoch, &login, |r| {
                r.retry_upload_abort(id, std::time::Instant::now());
            });
        }
    }
}
