//! 收发：别的设备发来的文件落到本机；本机挑好的文件发到别的设备。
//!
//! 一个任务按先后处理排队的文件。收到的文件走这条路：
//!
//! ```text
//! 取内容 ─▶ 边写暂存边核对长度与摘要 ─▶ 再确认接收目录 ─▶ 发布 ─▶ 通知用户 ─▶ 回执"已收下"
//! ```
//!
//! 发出去的文件走那条路：
//!
//! ```text
//! 算摘要 ─▶ 上传并报进度 ─▶ 服务端收下 ─▶ 等对方接收 ─▶ 对面收下、拒收，或者过期
//! ```
//!
//! 几条规则决定了它的形状：
//!
//! - **本机效果先落盘再回执。** 服务端一收到"已收下"就释放文件，所以这句话只能在文件
//!   真的存好之后说。回执没发出去不算完：之后每次与服务端对账都会发现它还在等，再补一次。
//! - **收到的内容不信任。** 长度或摘要对不上就不发布，并如实告诉发送方"校验没通过"，
//!   而不是说对方的电脑出了问题。
//! - **网络断了不算失败。** 隔一会儿从头再来。同一个文件永远只有一份记录，
//!   排队、重试、补回执都由它驱动，不会把同一个文件收两遍、发两遍。
//! - **取消要取消到底。** 上传途中按了取消，文件其实可能已经到了服务端；对账时会把
//!   这样的发送撤回，不让对面收到一个本机已经放弃的文件。
//!
//! 事件流不补送断线期间的事件，所以每次连上、以及之后每隔一段时间，
//! 都向服务端问一遍"还有哪些文件在等我"和"哪些发出去的还悬着"。

use std::path::PathBuf;
use std::time::Duration;

use assemble_client::{ClientError, RequestOptions, TransferOptions, TransferProgress};
use assemble_device::Received;
use assemble_device::inbox::{ReceiveFailure, accept, is_source_failure, resolve, sweep_staging};
use assemble_device::receive::default_directory;
use assemble_protocol::error::codes;
use assemble_protocol::primitives::MAX_FILE_SIZE;
use assemble_protocol::sends::{
    Direction, FailInput, FailReason, SEND_PAGE_MAX, Send as SendRecord, SendQuery, SendState,
};
use assemble_store::json::{StoreError, read_json, write_private_json};
use assemble_transfer::TransferError;
use assemble_transfer::download::SaveOptions;
use tauri::Manager;
use tokio_util::sync::CancellationToken;

use crate::registry::{
    Cancellation, DRIVE_NAME, Delivery, Destination, Job, Next, TransferFailure,
};
use crate::session;
use crate::state::{AppState, Notice, Settings};

/// 设置文件的名字，在应用数据目录里。
const SETTINGS_FILE: &str = "settings.json";

/// 回执与对账的时限。
const SHORT_TIMEOUT: Duration = Duration::from_secs(6);

/// 隔多久与服务器对一次账。比事件流的心跳长得多，平时只是一次很小的请求。
pub(crate) const RESYNC_INTERVAL: Duration = Duration::from_mins(5);

// ---- 设置 ----

fn settings_path<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> PathBuf {
    // 应用数据目录在启动时已经建好；万一拿不到，后面的读写会如实失败。
    app.path()
        .app_data_dir()
        .map_or_default(|dir| dir.join(SETTINGS_FILE))
}

/// 读出保存过的设置。读不懂就当没设置过：这只是"文件存到哪里"的偏好，
/// 用默认位置比卡住不用强。
pub fn load_settings<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Settings {
    let path = settings_path(app);
    match read_json(&path) {
        Ok(settings) => settings.unwrap_or_default(),
        Err(error) => {
            tracing::warn!(%error, "the settings could not be read; defaults are used");
            Settings::default()
        }
    }
}

pub fn save_settings<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    settings: &Settings,
) -> Result<(), StoreError> {
    write_private_json(&settings_path(app), settings)
}

/// 系统的"下载"目录。系统给不出来时退回家目录下的 `Downloads`。
pub fn system_downloads_dir<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> PathBuf {
    app.path().download_dir().unwrap_or_else(|error| {
        tracing::debug!(%error, "the system has no downloads folder");
        app.path().home_dir().unwrap_or_default().join("Downloads")
    })
}

// ---- 接收任务 ----

/// 接收任务：一直运行到应用退出。
pub async fn run<R: tauri::Runtime>(app: tauri::AppHandle<R>) {
    let state = app.state::<AppState<R>>();
    sweep(&state).await;
    loop {
        let job = tokio::select! {
            biased;
            () = state.lifetime.cancelled() => return,
            job = ready(&state) => job,
        };
        let Some(job) = job else {
            continue;
        };
        match job {
            Job::Receive { id } => receive(&state, &id).await,
            Job::Send { id } => send(&state, &id).await,
            Job::Settle { id } => settle(&state, &id).await,
            Job::AbortUpload { id } => uploads::abort(&state, &id).await,
            Job::CleanupDownload { id } => cloud_downloads::cleanup(&state, &id).await,
        }
    }
}

/// 等到有事可做；中途记录变了（来了新文件、对面撤回了）就重新看一遍。
async fn ready<R: tauri::Runtime>(state: &AppState<R>) -> Option<Job> {
    match state.next_job() {
        Next::Work(job) => Some(job),
        Next::Wait(delay) => {
            tokio::select! {
                () = tokio::time::sleep(delay) => {}
                () = state.wait_for_transfers() => {}
            }
            None
        }
        Next::Nothing => {
            state.wait_for_transfers().await;
            None
        }
    }
}

/// 启动时清掉上次中断留下的暂存文件。接收还没开始，这时不可能有正在写的。
async fn sweep<R: tauri::Runtime>(state: &AppState<R>) {
    let mut folders = vec![default_directory(state.downloads_dir())];
    if let Some(configured) = &state.settings().receive_directory
        && !folders.contains(configured)
    {
        folders.push(configured.clone());
    }
    for folder in folders {
        if let Some(protected) = state.protected_staging() {
            let _ = assemble_device::resume::sweep(&folder, &protected).await;
        }
        match sweep_staging(&folder).await {
            Ok(0) => {}
            Ok(count) => tracing::info!(
                count,
                "staging files left by an interrupted run were removed"
            ),
            // 目录不存在、或者现在读不了：都不妨碍启动，下次再清。
            Err(error) => tracing::debug!(%error, "a receive folder could not be swept"),
        }
    }
}

/// 与服务端对账：问一遍"还有哪些文件在等我"，把没接上的接上；
/// 再看一遍发出去还悬着的，把本机已经放弃的撤回。
///
/// 断线期间送不到的 `send_offered`、没送到的回执，都靠它补上。
pub async fn resync<R: tauri::Runtime>(
    state: &AppState<R>,
    cancel: &CancellationToken,
) -> Result<(), ClientError> {
    let options = RequestOptions {
        cancel,
        timeout: Some(SHORT_TIMEOUT),
    };
    let waiting = SendQuery {
        direction: Direction::In,
        state: Some(SendState::Offered),
        limit: SEND_PAGE_MAX,
    };
    for send in &state.client.sends(&waiting, &options).await? {
        state.offer(send);
    }
    let outgoing = SendQuery {
        direction: Direction::Out,
        // 应用关闭期间的最终回执也要接上，不能只看仍在等待的发送。
        state: None,
        limit: SEND_PAGE_MAX,
    };
    for send in &state.client.sends(&outgoing, &options).await? {
        if send.state == SendState::Offered && state.abandoned(send) {
            // 撤回失败不影响这一轮的其余部分：下一次对账还会看到它。
            let _ = state
                .client
                .send_cancel(&send.id, &options)
                .await
                .map(|send| state.updated(&send));
        } else {
            state.updated(send);
        }
    }
    Ok(())
}

/// 一次接收尝试的结局。
#[derive(Debug, PartialEq)]
enum Outcome {
    Saved {
        received: Received,
        fell_back: bool,
    },
    /// 网络的事：过一会儿从头再收。
    Retry,
    Failed(ReceiveFailure),
    /// 被打断了：登录结束，或者对面撤回。记录已经由它们处理。
    Interrupted,
    Unauthorized,
    /// 服务端已经没有这条记录了。
    Gone,
    /// 记录还在，但已经不是"等着接收"。
    Changed,
}

async fn receive<R: tauri::Runtime>(state: &AppState<R>, id: &str) {
    // 先记下这一次登录：领取之后登录可能就换了，那时不该再收上一个账户的文件。
    let login = state.session_cancel();
    let epoch = *state.session_changes().borrow();
    let Some(delivery) = state
        .transfer_session(epoch, &login, |r| r.begin_receive(id))
        .flatten()
    else {
        return;
    };
    let claim = claim(&delivery);
    let cancel = login.child_token();
    let _tracked = state.track_transfer(id, cancel.clone());

    if let Delivery::FromDrive(file) = &delivery {
        cloud_downloads::receive(state, id, file, epoch, &login, &cancel).await;
        return;
    }

    match pull(state, &claim, &cancel).await {
        Outcome::Saved {
            received,
            fell_back,
        } => {
            // 文件已经在磁盘上了。先登记，再通知；别的设备发来的，最后才告诉服务端"已收下"。
            let handle = state.fresh_handle();
            state.receive_completed(id, &received.name, received.path.clone(), handle);
            state.notify_received(handle, &claim.from, &received.name, fell_back);
            if fell_back {
                state.set_notice(Some(Notice::info(
                    "原来设置的接收文件夹不可用，这个文件已存到默认位置。",
                )));
            }
            // 从云盘取回的不欠服务端回执：存好了就是存好了。
            if !claim.on_drive {
                settle(state, id).await;
            }
        }
        Outcome::Retry => state.retry_receive(id),
        Outcome::Failed(failure) => {
            tracing::debug!(
                transfer = %id,
                failure = failure.as_str(),
                "a file could not be received"
            );
            state.receive_failed(id, failure.into());
            if !claim.on_drive {
                settle(state, id).await;
            }
        }
        Outcome::Interrupted => {}
        Outcome::Unauthorized => session::revoked(state),
        // 云盘里的文件在取之前就被删了：说出来，而不是让这一条悄悄消失。
        Outcome::Gone | Outcome::Changed if claim.on_drive => {
            state.receive_failed(id, TransferFailure::Vanished);
        }
        // 一次发送已经不在了（对面的记录被清掉了）：本机也不必再留着它。
        Outcome::Gone => state.forget_transfer(id),
        Outcome::Changed => refresh(state, id).await,
    }
}

/// 要取的东西在服务端上的位置，以及核对它要用的声明。
struct Claim {
    id: String,
    name: String,
    size: u64,
    sha256: String,
    /// 另一头在界面上的名字：发来文件的设备，或者云盘。
    from: String,
    on_drive: bool,
}

fn claim(delivery: &Delivery) -> Claim {
    match delivery {
        Delivery::FromDevice(send) => Claim {
            id: send.id.clone(),
            name: send.name.clone(),
            size: send.size,
            sha256: send.sha256.clone(),
            from: send.peer_device_name.clone(),
            on_drive: false,
        },
        Delivery::FromDrive(file) => Claim {
            id: file.id.clone(),
            name: file.name.clone(),
            size: file.size,
            sha256: file.sha256.clone(),
            from: DRIVE_NAME.to_owned(),
            on_drive: true,
        },
    }
}

/// 取一次内容并落盘。名字在下载途中被抢走时，换一个名字从头再来一次。
async fn pull<R: tauri::Runtime>(
    state: &AppState<R>,
    claim: &Claim,
    cancel: &CancellationToken,
) -> Outcome {
    let epoch = *state.session_changes().borrow();
    let login = state.session_cancel();
    let transfer_id = if claim.on_drive {
        format!("drive-{}", claim.id)
    } else {
        claim.id.clone()
    };
    // 连默认目录都准备不出来：是这台电脑的问题。
    let Ok(inbox) = resolve(
        state.settings().receive_directory.as_deref(),
        state.downloads_dir(),
    )
    .await
    else {
        return Outcome::Failed(ReceiveFailure::WriteFailed);
    };

    for attempt in 0..2 {
        let mut report = |progress: TransferProgress| {
            state.transfer_session(epoch, &login, |r| {
                r.progress(&transfer_id, progress.fraction());
            });
        };
        let options = TransferOptions::new(cancel);
        let fetched = if claim.on_drive {
            state
                .client
                .drive_content(&claim.id, options, Some(&mut report))
                .await
        } else {
            state
                .client
                .send_content(&claim.id, options, Some(&mut report))
                .await
        };
        let download = match fetched {
            Ok(download) => download,
            Err(error) => return when_unreachable(&error),
        };
        let mut reader = download.into_reader();
        let saved = accept(
            &inbox.directory,
            &claim.name,
            &mut reader,
            SaveOptions {
                expected_sha256: Some(&claim.sha256),
                content_length: Some(claim.size),
                max_bytes: Some(MAX_FILE_SIZE),
                ..SaveOptions::new(cancel)
            },
        )
        .await;
        match saved {
            Ok(received) => {
                return Outcome::Saved {
                    received,
                    fell_back: inbox.fell_back,
                };
            }
            // 名字在下载途中被别的程序占了：换一个名字从头再收一次。
            Err(TransferError::FileExists) if attempt == 0 => {}
            // 连接断了、对端不再送数据：网络的事，过一会儿从头再收，不怪谁的磁盘。
            Err(error) if is_source_failure(&error) => return Outcome::Retry,
            Err(TransferError::Cancelled) => return Outcome::Interrupted,
            Err(error) => return Outcome::Failed(ReceiveFailure::from(&error)),
        }
    }
    // 第二次仍然撞名：只能如实报告。
    Outcome::Failed(ReceiveFailure::WriteFailed)
}

/// 取不到内容时，这次接收该怎么收场。
///
/// 只有文件确实存不下来才算"失败"并报告给对面。取不到内容的原因说不准是谁的问题
/// （网络、服务端、中间的代理），那就过一会儿再试——既不怪这台电脑的磁盘，
/// 也不让对面的用户白等一个"重发"；真的一直收不到，服务端到时间会把它标成过期。
fn when_unreachable(error: &ClientError) -> Outcome {
    match error {
        ClientError::Cancelled => Outcome::Interrupted,
        error if error.is_unauthorized() => Outcome::Unauthorized,
        ClientError::Http { status: 404, .. } => Outcome::Gone,
        ClientError::Http {
            status: 403 | 409, ..
        } => Outcome::Changed,
        _ => Outcome::Retry,
    }
}

/// 把本机已经有的结局告诉服务端：收下了，或者收不下来。
async fn settle<R: tauri::Runtime>(state: &AppState<R>, id: &str) {
    let Some((send, report)) = state.settlement_of(id) else {
        return;
    };
    let cancel = state.session_cancel();
    let options = RequestOptions {
        cancel: &cancel,
        timeout: Some(SHORT_TIMEOUT),
    };
    let delivered = match report {
        None => state
            .client
            .send_accept(&send.id, &options)
            .await
            .map(|_| ()),
        Some(failure) => state
            .client
            .send_failed(
                &send.id,
                FailInput {
                    reason: reported(failure),
                },
                &options,
            )
            .await
            .map(|_| ()),
    };
    match delivered {
        // 回执送到了；或者服务端已经把它结了（对面抢先撤回、已经过期）：没什么可补的了。
        Ok(())
        | Err(ClientError::Http {
            status: 403 | 404 | 409,
            ..
        }) => state.transfer_settled(id),
        Err(ClientError::Cancelled) => {}
        Err(error) if error.is_unauthorized() => session::revoked(state),
        Err(error) => {
            tracing::debug!(%error, "the receipt for a received file was not delivered");
            state.retry_settle(id);
        }
    }
}

/// 服务端说这个文件不再是"等着接收"了：问清楚它现在的结局，照实更新。
async fn refresh<R: tauri::Runtime>(state: &AppState<R>, id: &str) {
    let cancel = state.session_cancel();
    let listed = state
        .client
        .sends(
            &SendQuery {
                direction: Direction::In,
                state: None,
                limit: SEND_PAGE_MAX,
            },
            &RequestOptions {
                cancel: &cancel,
                timeout: Some(SHORT_TIMEOUT),
            },
        )
        .await;
    match listed {
        Ok(items) => match items.iter().find(|send| send.id == id) {
            // 还是"等着接收"：刚才那一下只是一时的，过会儿再试。
            Some(send) if send.state == SendState::Offered => state.retry_receive(id),
            Some(send) => {
                state.updated(send);
            }
            None => state.forget_transfer(id),
        },
        Err(error) if error.is_unauthorized() => session::revoked(state),
        Err(_) => state.retry_receive(id),
    }
}

/// 本机的失败原因在接口里的拼法。两边一致，由 `assemble-device` 的测试盯着。
fn reported(failure: ReceiveFailure) -> FailReason {
    match failure {
        ReceiveFailure::DiskFull => FailReason::DiskFull,
        ReceiveFailure::WriteFailed => FailReason::WriteFailed,
        ReceiveFailure::VerificationFailed => FailReason::VerificationFailed,
        ReceiveFailure::Cancelled => FailReason::Cancelled,
    }
}

// ---- 发出的 ----

async fn send<R: tauri::Runtime>(state: &AppState<R>, id: &str) {
    // 先记下这一次登录：领取之后登录可能就换了，那时不该再发上一个账户的文件。
    let epoch = *state.session_changes().borrow();
    let login = state.session_cancel();
    let Some(outgoing) = state
        .transfer_session(epoch, &login, |r| r.begin_send(id))
        .flatten()
    else {
        return;
    };
    let cancel = login.child_token();
    let _tracked = state.track_transfer(id, cancel.clone());

    let mut report = |progress: TransferProgress| state.transfer_progress(id, progress.fraction());
    let options = TransferOptions::new(&cancel);
    let uploaded = match &outgoing.destination {
        Destination::Device(device) => state
            .client
            .send_to_device(&device.id, &outgoing.file, options, Some(&mut report))
            .await
            .map(Delivered::ToDevice),
        Destination::Drive | Destination::DriveFolder(_) => {
            uploads::send(state, id, &outgoing, epoch, &login, &cancel)
                .await
                .map(|()| Delivered::ToDrive)
        }
    };
    if login.is_cancelled() || *state.session_changes().borrow() != epoch {
        return;
    }
    let uploaded = match uploaded {
        Ok(uploaded) => uploaded,
        // 用户取消了：记录已经标好了。文件其实到没到服务端，由对账去确认。
        Err(ClientError::Cancelled) => return,
        Err(error) if error.is_unauthorized() => return session::revoked(state),
        Err(ClientError::LocalSource)
            if !matches!(outgoing.destination, Destination::Device(_)) =>
        {
            state.transfer_session(epoch, &login, |r| r.source_missing(id));
            return;
        }
        Err(error) => {
            match refusal(&error) {
                Some(failure) => {
                    state.transfer_session(epoch, &login, |r| r.send_failed(id, failure));
                }
                // 网络的事：过一会儿从头再发一遍。等不及可以取消。
                None => {
                    state.transfer_session(epoch, &login, |r| {
                        r.retry_send(id, std::time::Instant::now());
                    });
                }
            }
            return;
        }
    };
    match uploaded {
        // 存进云盘就完事了，没有"等对方接收"这一步。顺手把云盘的名单换新。
        Delivered::ToDrive => {
            state.transfer_session(epoch, &login, |r| r.drive_uploaded(id));
            refresh_drive(state).await;
        }
        Delivered::ToDevice(send) => {
            // 用户在上传途中按了取消的话，文件其实还是到了：去撤回。
            if state.transfer_session(epoch, &login, |r| r.send_uploaded(id, &send))
                == Some(crate::registry::Updated::Withdraw)
            {
                withdraw(state, &send.id, &state.lifetime).await;
            }
        }
    }
}

/// 文件送到了哪里。
enum Delivered {
    ToDevice(SendRecord),
    ToDrive,
}

fn refusal(error: &ClientError) -> Option<TransferFailure> {
    match error {
        // 413 也用来回答"云盘空间不够"：靠错误码与"超过单文件上限"区分。
        ClientError::Http {
            remote_code,
            status: 413,
        } if remote_code.as_deref() == Some(codes::QUOTA_EXCEEDED) => {
            Some(TransferFailure::QuotaExceeded)
        }
        ClientError::TooLarge | ClientError::Http { status: 413, .. } => {
            Some(TransferFailure::TooLarge)
        }
        ClientError::LocalSource => Some(TransferFailure::FileUnreadable),
        ClientError::Http {
            status: 409,
            remote_code,
        } if remote_code.as_deref() == Some(codes::DEVICE_OFFLINE) => {
            Some(TransferFailure::DeviceOffline)
        }
        ClientError::Http {
            remote_code: Some(code),
            ..
        } if code == "UPLOAD_TARGET_CHANGED" => Some(TransferFailure::DestinationChanged),
        ClientError::Http {
            remote_code: Some(code),
            ..
        } if matches!(
            code.as_str(),
            "UPLOAD_EXPIRED" | "UPLOAD_CANCELLED" | "UPLOAD_FINISHED"
        ) =>
        {
            Some(TransferFailure::UploadExpired)
        }
        // 服务端明确拒绝：名字不合规、文件在挑选之后变了、发给自己。重试也不会变。
        ClientError::Http {
            status: 400..=499, ..
        }
        | ClientError::InvalidRequest => Some(TransferFailure::Rejected),
        ClientError::Cancelled
        | ClientError::InvalidResponse
        | ClientError::Network
        | ClientError::Timeout
        | ClientError::Http { .. } => None,
    }
}

/// 向服务端要一份云盘的名单。
pub async fn refresh_drive<R: tauri::Runtime>(state: &AppState<R>) {
    let epoch = *state.session_changes().borrow();
    let cancel = state.session_cancel();
    let result = state.client.drive(&RequestOptions::new(&cancel)).await;
    if cancel.is_cancelled() || *state.session_changes().borrow() != epoch {
        return;
    }
    match result {
        Ok(listing) => {
            state.remember_drive(listing, epoch, &cancel);
        }
        Err(e) if e.is_unauthorized() => session::revoked(state),
        Err(e) => tracing::debug!(code=?e,"the drive could not be listed yet"),
    }
}

/// 文件已经在服务端等对方接收，本机却不想发了：去服务端撤回。
pub async fn withdraw<R: tauri::Runtime>(
    state: &AppState<R>,
    send_id: &str,
    cancel: &CancellationToken,
) {
    let options = RequestOptions {
        cancel,
        timeout: Some(SHORT_TIMEOUT),
    };
    match state.client.send_cancel(send_id, &options).await {
        Ok(send) => {
            state.updated(&send);
        }
        Err(error) if error.is_unauthorized() => session::revoked(state),
        // 没撤成：记录停在"等对方接收"，之后对账时还会再试。
        Err(error) => tracing::debug!(%error, "a send could not be withdrawn yet"),
    }
}

/// 用户要取消一条传输。
pub async fn cancel<R: tauri::Runtime>(state: &AppState<R>, id: &str) -> Cancellation {
    let outcome = state.cancel_transfer(id);
    if outcome == Cancellation::Local {
        state.stop_transfer(id);
    }
    if let Cancellation::OnServer(send_id) = &outcome {
        let cancel = state.session_cancel();
        withdraw(state, send_id, &cancel).await;
    }
    outcome
}

/// 再发一次只对"文件根本没到服务端"的失败有用。
pub fn retry_send<R: tauri::Runtime>(state: &AppState<R>, id: &str) -> bool {
    state.retry_failed_send(id)
}

#[cfg(test)]
mod tests;

mod cloud_downloads;
mod uploads;
