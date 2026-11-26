//! 事件中心与事件流。
//!
//! 事件中心给每个账户一条广播，谁想收事件就订阅它。
//! 设备发给服务端的内容都走普通 HTTP，所以这里只需要单向推送。
//! 广播是"发了就忘"的：没人在听的时候发出去的事件不会留着等人来取。
//! 设备每次连上都要重新拉一遍设备列表与待接收的发送，不能指望补送。
//!
//! 事件流（`GET /v1/events`）是一条不会主动结束的 SSE 响应。持有这条连接
//! 就算在线；连接一断，[`crate::presence::Connection`] 被丢弃，设备就离线了。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use assemble_protocol::events::{EVENT_HEARTBEAT, Event};
use axum::extract::State;
use axum::http::{HeaderValue, header};
use axum::response::IntoResponse;
use axum::response::Response;
use axum::response::sse::{Event as SseEvent, Sse};
use futures_util::StreamExt;
use tokio::sync::{broadcast, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;

use crate::auth::Identity;
use crate::error::ApiError;
use crate::http::Shared;
use crate::presence::{Connection, Presence};

/// 每条广播最多攒这么多条消息。攒满了还没被读走，读得慢的那条连接会漏消息，
/// 它会因此断开重连、重新拉取——比悄悄漏掉一条要好。
const CHANNEL_CAPACITY: usize = 64;

/// 一条连接上最多攒这么多条还没写出去的事件。
const OUTGOING_CAPACITY: usize = 16;

/// 多久发一次心跳。它同时用来探活：发不出去就说明对端已经不在了。
const HEARTBEAT: Duration = EVENT_HEARTBEAT;

/// 广播里传的东西。
#[derive(Debug, Clone)]
pub enum Message {
    /// 只在服务端内部传递：停用之后及时结束已有的设备连接。
    AccountSuspended,
    /// 账户里有设备上线、下线或被改动。具体列表由每条连接自己查。
    DevicesChanged,
    /// 一个给某台设备（`audience`）或全账户（`None`）的事件。
    Event {
        audience: Option<String>,
        event: Arc<Event>,
    },
}

#[derive(Debug, Default)]
pub struct Hub {
    channels: Mutex<HashMap<String, broadcast::Sender<Arc<Message>>>>,
    /// 服务端退出时触发。事件流是永不主动结束的长连接，不主动收掉它们，
    /// "优雅退出"就会一直等下去——真机上见过：旧进程收到 SIGTERM 后迟迟不退出。
    closing: CancellationToken,
}

impl Hub {
    /// 通知所有事件流：服务端要退出了，各自收尾。
    pub fn close(&self) {
        self.closing.cancel();
    }

    /// 订阅一个账户的广播。
    pub fn subscribe(&self, account_id: &str) -> broadcast::Receiver<Arc<Message>> {
        let mut channels = self.channels.lock().unwrap_or_else(PoisonError::into_inner);
        let sender = channels.entry(account_id.to_owned()).or_insert_with(|| {
            let (sender, _) = broadcast::channel(CHANNEL_CAPACITY);
            sender
        });
        sender.subscribe()
    }

    /// 发一条消息。没有订阅者时什么都不发生。
    pub fn publish(&self, account_id: &str, message: Message) {
        let mut channels = self.channels.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(sender) = channels.get(account_id) else {
            return;
        };
        // 发不出去只说明没人在听。
        let _ = sender.send(Arc::new(message));
        // 已经没人订阅的频道随手清掉，账户多了也不占内存。
        if sender.receiver_count() == 0 {
            channels.remove(account_id);
        }
    }

    fn closing_token(&self) -> CancellationToken {
        self.closing.clone()
    }

    /// 通知一个账户里的所有设备：设备情况变了。
    pub fn devices_changed(&self, account_id: &str) {
        self.publish(account_id, Message::DevicesChanged);
    }

    /// 此刻有多少条连接在听这个账户。
    pub fn listeners(&self, account_id: &str) -> usize {
        self.channels
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(account_id)
            .map_or(0, broadcast::Sender::receiver_count)
    }
}

/// `GET /v1/events`：打开一条事件流。
pub async fn stream(State(state): State<Shared>, identity: Identity) -> Result<Response, ApiError> {
    // 先订阅再登记：自己上线引发的那次通知也要收到。
    let receiver = state.hub.subscribe(&identity.account_id);
    // 认证与订阅之间账户可能被停用；订阅后再核对，避免漏掉停用广播。
    if !crate::devices::exists(&state.pool, &identity.device_id).await? {
        return Err(ApiError::unauthorized());
    }
    let (sender, outgoing) = mpsc::channel::<SseEvent>(OUTGOING_CAPACITY);
    let connection = Connection::open(
        Arc::clone(&state.presence),
        Arc::clone(&state.hub),
        state.pool.clone(),
        &identity.account_id,
        &identity.device_id,
    );
    let feed = Feed {
        receiver,
        sender,
        connection: Some(connection),
        presence: Arc::clone(&state.presence),
        pool: state.pool.clone(),
        account_id: identity.account_id,
        device_id: identity.device_id,
        closing: state.hub.closing_token(),
    };
    // 连接登记在这个任务里：任务一结束，设备就离线。
    tokio::spawn(feed.run());

    // 事件流不会以错误结束：它要么一直开着，要么直接断开让设备重连。
    let body = ReceiverStream::new(outgoing).map(Ok::<_, std::convert::Infallible>);
    let mut response = Sse::new(body).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    // 告诉 nginx 这条响应不要缓冲，否则事件会被它攒着不及时转发。
    response.headers_mut().insert(
        header::HeaderName::from_static("x-accel-buffering"),
        HeaderValue::from_static("no"),
    );
    Ok(response)
}

/// 一条事件流连接上的工作：把广播里的消息写成 SSE，并维持这条连接。
struct Feed {
    receiver: broadcast::Receiver<Arc<Message>>,
    sender: mpsc::Sender<SseEvent>,
    /// 持有它就是在线；`run` 结束时随 `self` 一起被丢弃。
    connection: Option<Connection>,
    presence: Arc<Presence>,
    pool: sqlx::SqlitePool,
    account_id: String,
    device_id: String,
    /// 服务端退出时触发。
    closing: CancellationToken,
}

impl Feed {
    async fn run(mut self) {
        // 把字段拆开：循环里的几个分支各自借用不同的字段。
        let Feed {
            receiver,
            sender,
            connection,
            presence,
            pool,
            account_id,
            device_id,
            closing,
        } = &mut self;
        // 第一条永远是 hello：设备据此知道服务端认定的它是谁。
        if !emit(
            sender,
            Event::Hello {
                device_id: device_id.clone(),
            },
        )
        .await
        {
            return;
        }
        // 第一次心跳在一个周期之后，不是马上：刚连上不必立刻查一遍。
        let mut ticker =
            tokio::time::interval_at(tokio::time::Instant::now() + HEARTBEAT, HEARTBEAT);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                message = receiver.recv() => {
                    let keep_going = match message {
                        Ok(message) => {
                            deliver(sender, presence, pool, account_id, device_id, &message).await
                        }
                        // 漏了消息：断开，让设备重连之后重新拉取。
                        Err(broadcast::error::RecvError::Lagged(skipped)) => {
                            tracing::debug!(skipped, "an event stream fell behind; reconnecting");
                            false
                        }
                        Err(broadcast::error::RecvError::Closed) => false,
                    };
                    if !keep_going {
                        break;
                    }
                }
                _ = ticker.tick() => {
                    // 心跳兼探活：发不出去说明对端已经不在了。
                    if sender.send(SseEvent::default().comment("ping")).await.is_err() {
                        break;
                    }
                    // 设备可能在别的设备上被撤销了：撤销要及时反映到在线状态上。
                    match crate::devices::exists(pool, device_id).await {
                        Ok(true) => {}
                        Ok(false) => break,
                        Err(error) => tracing::debug!(%error, "device existence could not be checked"),
                    }
                }
                // 响应正文被丢掉，也就是设备断开了：不必等到下一次心跳。
                () = sender.closed() => break,
                // 服务端要退出了：正常注销（写离线时间、通知别的设备），然后结束响应。
                () = closing.cancelled() => break,
            }
        }
        // 注销登记：先写离线时间，再通知别的设备。
        if let Some(connection) = connection.as_mut() {
            connection.close().await;
        }
    }
}

/// 处理一条广播消息。返回这条连接是否应当继续。
async fn deliver(
    sender: &mpsc::Sender<SseEvent>,
    presence: &Presence,
    pool: &sqlx::SqlitePool,
    account_id: &str,
    device_id: &str,
    message: &Message,
) -> bool {
    match message {
        Message::AccountSuspended => false,
        Message::DevicesChanged => {
            let devices = match crate::devices::list(pool, presence, account_id).await {
                Ok(devices) => devices,
                Err(error) => {
                    tracing::error!(%error, "devices could not be listed for an event");
                    // 列不出来就断开：设备重连时会自己再拉一次。
                    return false;
                }
            };
            emit(sender, Event::DevicesChanged { devices }).await
        }
        // 给别的设备的事件不转发。
        Message::Event { audience, event } => {
            if audience
                .as_deref()
                .is_some_and(|target| target != device_id)
            {
                return true;
            }
            emit(sender, event.as_ref().clone()).await
        }
    }
}

/// 写出一条事件。写不出去说明对端已经不在了。
async fn emit(sender: &mpsc::Sender<SseEvent>, event: Event) -> bool {
    // 事件都是普通的类型，序列化不会失败；万一失败，宁可不发也不发半截。
    let Ok(payload) = serde_json::to_string(&event) else {
        return false;
    };
    sender.send(SseEvent::default().data(payload)).await.is_ok()
}

#[cfg(test)]
mod tests {
    use assemble_protocol::primitives::Platform;

    use super::*;

    #[tokio::test]
    async fn messages_reach_every_listener_of_the_account_and_nobody_else() {
        let hub = Hub::default();
        let mut alice = hub.subscribe("alice");
        let mut alice_second = hub.subscribe("alice");
        let mut bob = hub.subscribe("bob");
        assert_eq!(hub.listeners("alice"), 2);

        hub.devices_changed("alice");
        for listener in [&mut alice, &mut alice_second] {
            let received = listener.recv().await.unwrap();
            assert!(matches!(&*received, Message::DevicesChanged));
        }
        assert!(bob.try_recv().is_err());

        let hello = Event::Hello {
            device_id: "d1".into(),
        };
        hub.publish(
            "alice",
            Message::Event {
                audience: Some("d1".into()),
                event: Arc::new(hello.clone()),
            },
        );
        match alice.recv().await.unwrap().as_ref().clone() {
            Message::Event { audience, event } => {
                assert_eq!(audience.as_deref(), Some("d1"));
                assert_eq!(*event, hello);
            }
            other => panic!("expected an event, got {other:?}"),
        }
        drop(alice_second);

        // 没人订阅的账户：发了就丢，不报错，也不留频道。
        hub.devices_changed("nobody");
        let devices = vec![assemble_protocol::device::Device {
            id: "d1".into(),
            name: "Mac".into(),
            platform: Platform::Macos,
            online: true,
            last_seen_at: None,
            created_at: assemble_protocol::primitives::Timestamp::now(),
        }];
        hub.publish(
            "alice",
            Message::Event {
                audience: None,
                event: Arc::new(Event::DevicesChanged { devices }),
            },
        );
        let received = alice.recv().await.unwrap();
        assert!(matches!(&*received, Message::Event { audience: None, .. }));
    }

    #[tokio::test]
    async fn a_slow_listener_is_told_it_missed_messages_instead_of_getting_a_subset() {
        let hub = Hub::default();
        let mut slow = hub.subscribe("a");
        for _ in 0..=CHANNEL_CAPACITY {
            hub.devices_changed("a");
        }
        assert!(matches!(
            slow.recv().await,
            Err(broadcast::error::RecvError::Lagged(_))
        ));
    }

    #[tokio::test]
    async fn channels_are_dropped_once_nobody_listens() {
        let hub = Hub::default();
        let listener = hub.subscribe("a");
        assert_eq!(hub.listeners("a"), 1);
        drop(listener);
        hub.devices_changed("a");
        assert_eq!(hub.listeners("a"), 0);
        assert!(hub.channels.lock().unwrap().is_empty());
    }
}
