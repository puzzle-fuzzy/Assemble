//! 登录之后常驻的后台任务。
//!
//! 三个循环，都随应用退出结束：
//!
//! - [`watch_health`]：隔一段时间探一次服务，让界面上的"是否可达"不过期；
//! - [`follow_events`]：登录期间维持一条事件流，把设备列表与待接收的文件同步到状态里。
//!   流断了由客户端自动重连；登录失效时收摊，并请用户重新登录；
//! - [`transfers::run`]：把别的设备发来的文件一个一个收到本机，把排队的文件发出去。
//!
//! 事件流不补送断线期间的变化，所以每次连上（收到 `hello`）都重新拉一遍设备列表、云盘名单、
//! 问一遍还有哪些文件在等我；连着的时候也隔一段时间问一次。

use std::time::Duration;

use assemble_client::{ClientError, RequestOptions};
use assemble_protocol::device::Device;
use assemble_protocol::events::Event;
use assemble_protocol::schedule::Planner;
use assemble_protocol::sends::Send as SendRecord;
use futures_util::StreamExt;
use tauri::{AppHandle, Manager};
use tokio_util::sync::CancellationToken;

use crate::planner;
use crate::session;
use crate::state::{AppState, DeviceView, StreamState};
use crate::transfers;

/// 探活的间隔。
const HEALTH_INTERVAL: Duration = Duration::from_secs(30);

/// 探活与拉设备列表的时限。
const SHORT_TIMEOUT: Duration = Duration::from_secs(6);

/// 启动后台循环。
pub fn start(app: &AppHandle) {
    let health = app.clone();
    tauri::async_runtime::spawn(async move { watch_health(health).await });
    let events = app.clone();
    tauri::async_runtime::spawn(async move { follow_events(events).await });
    let receiver = app.clone();
    tauri::async_runtime::spawn(async move { transfers::run(receiver).await });
}

/// 隔一段时间探一次服务。界面据此决定能不能发起登录。
async fn watch_health(app: AppHandle) {
    let state = app.state::<AppState>();
    let mut ticker = tokio::time::interval(HEALTH_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        let reachable = state
            .client
            .health(&RequestOptions {
                cancel: &state.lifetime,
                timeout: Some(SHORT_TIMEOUT),
            })
            .await
            .is_ok();
        state.set_reachable(reachable);
        tokio::select! {
            () = state.lifetime.cancelled() => return,
            _ = ticker.tick() => {}
        }
    }
}

/// 一条事件流上收到的东西之后该做什么。
#[derive(Debug, PartialEq)]
enum Next {
    /// 什么都不用做：流本身在维持，内容暂时用不上。
    Nothing,
    /// 连上了：标记在线状态，并重新拉一遍设备列表。
    Connected,
    /// 换上新的设备列表。
    Devices(Vec<DeviceView>),
    /// 别的设备发来了一个文件。
    Offered(SendRecord),
    /// 一次发送的状态变了。
    Updated(SendRecord),
    /// 服务端给了一份新的日程名单。
    Planner(Planner),
    /// 断了，客户端正在重连。
    Reconnecting,
    /// 服务端不再认这台设备的登录。
    Rejected,
    /// 流结束了（被取消），回到"没登录"重新判断。
    Finished,
}

/// 把事件流上的一条内容翻译成该做的事。
fn handle(item: Option<Result<Event, ClientError>>, own_device: Option<&str>) -> Next {
    match item {
        Some(Ok(event)) => match event {
            // 服务端认定的自己是另一台设备时，这条流不是我们的：当没连上处理。
            Event::Hello { device_id } if own_device == Some(device_id.as_str()) => Next::Connected,
            Event::DevicesChanged { devices } => {
                Next::Devices(devices.iter().map(DeviceView::from).collect())
            }
            Event::SendOffered { send } => Next::Offered(send),
            Event::SendUpdated { send } => Next::Updated(send),
            Event::PlannerChanged { planner } => Next::Planner(planner),
            // 服务端认定的自己是另一台设备时，这条流不是我们的：当没连上处理。
            Event::Hello { .. } => Next::Nothing,
        },
        Some(Err(error)) if error.is_unauthorized() => Next::Rejected,
        Some(Err(_)) => Next::Reconnecting,
        None => Next::Finished,
    }
}

/// 登录期间维持一条事件流。
async fn follow_events(app: AppHandle) {
    let state = app.state::<AppState>();
    let mut sessions = state.session_changes();
    loop {
        // 没登录：等到登录状态变了再说。
        if !state.client.has_bearer() {
            let still_watched = tokio::select! {
                () = state.lifetime.cancelled() => return,
                result = sessions.changed() => result.is_ok(),
            };
            // 通道被关掉只可能是因为应用在退出。
            if !still_watched {
                return;
            }
            continue;
        }
        // 这一次登录的更替到此为止已经看过了；不然下面会把它当成"又登录了一次"。
        sessions.borrow_and_update();

        let cancel = state.session_cancel();
        let own_device = state.device_id();
        state.set_stream(StreamState::Connecting);
        let mut stream = state.client.clone().events(cancel.child_token());
        let mut resync = tokio::time::interval(transfers::RESYNC_INTERVAL);
        // 第一次对账跟着 `hello` 走，不要一上来就多发一次。
        resync.tick().await;
        loop {
            tokio::select! {
                biased;
                () = cancel.cancelled() => break,
                // 重新登录了：这条流属于上一次登录，丢掉它回到外层。
                _ = sessions.changed() => break,
                _ = resync.tick() => {
                    if catch_up(&state, &cancel).await {
                        break;
                    }
                }
                item = stream.next() => match handle(item, own_device.as_deref()) {
                    Next::Nothing => {}
                    Next::Connected => {
                        state.set_stream(StreamState::Connected);
                        // 断线期间的变化不会补送：连上就重新拉一遍。
                        // 拉不到不致命，下一次 `hello` 还会再拉。
                        if let Err(error) = pull_devices(&state, &cancel).await
                            && error.is_unauthorized()
                        {
                            break;
                        }
                        if catch_up(&state, &cancel).await {
                            break;
                        }
                        transfers::refresh_drive(&state).await;
                        // 断线期间到点的日程与提醒也在名单里：拉到就补弹。
                        if let Err(error) = planner::refresh(&state, &cancel).await {
                            tracing::debug!(%error, "the planner could not be listed yet");
                        }
                    }
                    Next::Devices(devices) => state.set_devices(devices),
                    Next::Planner(planner) => planner::arrived(&state, planner),
                    Next::Offered(send) => state.offer(&send),
                    Next::Updated(send) => {
                        // 本机已经放弃了这次发送，文件却到了服务端：去撤回，别让对面收到。
                        if state.updated(&send) {
                            let app = state.app().clone();
                            let send_id = send.id.clone();
                            tauri::async_runtime::spawn(async move {
                                let state = app.state::<AppState>();
                                transfers::withdraw(&state, &send_id, &state.lifetime).await;
                            });
                        }
                    }
                    Next::Reconnecting => state.set_stream(StreamState::Reconnecting),
                    // 流自己断了：登录失效，或者被取消。哪一种都由下面来分辨。
                    Next::Rejected | Next::Finished => break,
                },
            }
        }
        // 丢掉流：连接随之断开，服务端会把这台设备记成离线。
        drop(stream);

        // 流是自己结束的而不是被取消的：多半是登录失效。确认一下再决定。
        if !cancel.is_cancelled() {
            match pull_devices(&state, &state.lifetime).await {
                Err(error) if error.is_unauthorized() => {
                    tracing::info!("the device is no longer signed in on the server");
                    session::revoked(&state);
                }
                _ => {}
            }
        }
    }
}

/// 问一遍还有哪些文件在等我。返回 `true` 表示服务端不再认这台设备的登录。
async fn catch_up(state: &AppState, cancel: &CancellationToken) -> bool {
    match transfers::resync(state, cancel).await {
        Ok(()) => false,
        Err(error) => {
            tracing::debug!(%error, "the pending files could not be listed yet");
            error.is_unauthorized()
        }
    }
}

/// 拉一次账户里的设备列表。
async fn pull_devices(
    state: &AppState,
    cancel: &CancellationToken,
) -> Result<Vec<Device>, ClientError> {
    let devices = state
        .client
        .devices(&RequestOptions {
            cancel,
            timeout: Some(SHORT_TIMEOUT),
        })
        .await?;
    state.set_devices(devices.iter().map(DeviceView::from));
    Ok(devices)
}

#[cfg(test)]
mod tests {
    use assemble_protocol::primitives::{Platform, Timestamp};
    use assemble_protocol::sends::{Direction, SendState};

    use super::*;

    fn device(id: &str, online: bool) -> Device {
        Device {
            id: id.into(),
            name: format!("设备 {id}"),
            platform: Platform::Macos,
            online,
            last_seen_at: None,
            created_at: Timestamp::now(),
        }
    }

    #[test]
    fn a_hello_for_this_device_means_connected_and_a_new_device_list_replaces_the_old_one() {
        assert_eq!(
            handle(
                Some(Ok(Event::Hello {
                    device_id: "me".into()
                })),
                Some("me")
            ),
            Next::Connected
        );
        // 服务端说这条流属于别的设备：不认。
        assert_eq!(
            handle(
                Some(Ok(Event::Hello {
                    device_id: "someone-else".into()
                })),
                Some("me")
            ),
            Next::Nothing
        );
        assert_eq!(
            handle(
                Some(Ok(Event::Hello {
                    device_id: "me".into()
                })),
                None
            ),
            Next::Nothing
        );

        let listed = Next::Devices(vec![DeviceView::from(&device("me", true))]);
        assert_eq!(
            handle(
                Some(Ok(Event::DevicesChanged {
                    devices: vec![device("me", true)]
                })),
                Some("me")
            ),
            listed
        );
    }

    #[test]
    fn failures_are_told_apart_by_whether_the_login_is_still_valid() {
        let rejected = ClientError::Http {
            status: 401,
            remote_code: None,
        };
        assert_eq!(handle(Some(Err(rejected)), Some("me")), Next::Rejected);
        assert_eq!(
            handle(Some(Err(ClientError::Timeout)), Some("me")),
            Next::Reconnecting
        );
        assert_eq!(
            handle(Some(Err(ClientError::Network)), Some("me")),
            Next::Reconnecting
        );
        assert_eq!(handle(None, Some("me")), Next::Finished);
    }

    fn send(state: SendState) -> SendRecord {
        SendRecord {
            id: "s1".into(),
            direction: Direction::In,
            peer_device_id: "other".into(),
            peer_device_name: "别的设备".into(),
            name: "a.txt".into(),
            size: 1,
            mime_type: None,
            sha256: "a".repeat(64),
            state,
            failure: None,
            created_at: Timestamp::now(),
            updated_at: Timestamp::now(),
        }
    }

    #[test]
    fn events_about_files_are_handed_over_as_they_are() {
        // 别人发来一个文件、一次发送有了结局：交给状态去接，事件循环自己不改记录。
        let offered = send(SendState::Offered);
        assert_eq!(
            handle(
                Some(Ok(Event::SendOffered {
                    send: offered.clone()
                })),
                Some("me")
            ),
            Next::Offered(offered)
        );
        let cancelled = send(SendState::Cancelled);
        assert_eq!(
            handle(
                Some(Ok(Event::SendUpdated {
                    send: cancelled.clone()
                })),
                None
            ),
            Next::Updated(cancelled)
        );
    }
}
