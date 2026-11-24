//! 登录之后常驻的后台任务。
//!
//! 两个循环，都随应用退出结束：
//!
//! - [`watch_health`]：隔一段时间探一次服务，让界面上的"是否可达"不过期；
//! - [`follow_events`]：登录期间维持一条事件流，把设备列表同步到状态里。
//!   流断了由客户端自动重连；登录失效时收摊，并请用户重新登录。
//!
//! 事件流不补送断线期间的变化，所以每次连上（收到 `hello`）都重新拉一遍设备列表。

use std::time::Duration;

use assemble_client::{ClientError, RequestOptions};
use assemble_protocol::device::Device;
use assemble_protocol::events::Event;
use futures_util::StreamExt;
use tauri::{AppHandle, Manager};
use tokio_util::sync::CancellationToken;

use crate::session;
use crate::state::{AppState, DeviceView, StreamState};

/// 探活的间隔。
const HEALTH_INTERVAL: Duration = Duration::from_secs(30);

/// 探活与拉设备列表的时限。
const SHORT_TIMEOUT: Duration = Duration::from_secs(6);

/// 启动两个后台循环。
pub fn start(app: &AppHandle) {
    let health = app.clone();
    tauri::async_runtime::spawn(async move { watch_health(health).await });
    let events = app.clone();
    tauri::async_runtime::spawn(async move { follow_events(events).await });
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
            // 发送相关的事件在传输接进来之后再处理。
            Event::Hello { .. } | Event::SendOffered { .. } | Event::SendUpdated { .. } => {
                Next::Nothing
            }
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
        loop {
            tokio::select! {
                biased;
                () = cancel.cancelled() => break,
                // 重新登录了：这条流属于上一次登录，丢掉它回到外层。
                _ = sessions.changed() => break,
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
                    }
                    Next::Devices(devices) => state.set_devices(devices),
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

    #[test]
    fn events_about_transfers_are_ignored_until_transfers_are_wired_up() {
        let send = assemble_protocol::sends::Send {
            id: "s1".into(),
            direction: assemble_protocol::sends::Direction::In,
            peer_device_id: "other".into(),
            peer_device_name: "别的设备".into(),
            name: "a.txt".into(),
            size: 1,
            mime_type: None,
            sha256: "a".repeat(64),
            state: assemble_protocol::sends::SendState::Offered,
            failure: None,
            created_at: Timestamp::now(),
            updated_at: Timestamp::now(),
        };
        assert_eq!(
            handle(
                Some(Ok(Event::SendOffered { send: send.clone() })),
                Some("me")
            ),
            Next::Nothing
        );
        assert_eq!(
            handle(Some(Ok(Event::SendUpdated { send })), Some("me")),
            Next::Nothing
        );
    }
}
