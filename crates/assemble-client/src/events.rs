//! 事件流：服务端推给设备的事件（SSE），断了自动重连。
//!
//! 流只会因为两件事结束：调用方取消，或服务端表示登录已失效（401）。
//! 其它断开都先报告一次 `Err`，再等一小段时间重连。每次连上之后服务端都以
//! `hello` 开头，所以调用方应当在收到 `hello` 时重新拉一遍设备列表与
//! 待接收的发送——断线期间的事件不会补送。
//!
//! 不认识的事件类型直接跳过：服务端升级后新增的事件不会让旧桌面端断流。

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use futures_core::Stream;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;

use assemble_protocol::events::{EVENT_HEARTBEAT, EVENT_STREAM_PATH, Event};

use crate::client::{Client, rejection, transport};
use crate::error::ClientError;

/// 一条连接上多久没有任何内容就认为它死了：三个心跳周期。
pub const EVENT_IDLE_TIMEOUT: Duration = Duration::from_secs(3 * EVENT_HEARTBEAT.as_secs());

/// 重连等待的起点与上限。实际等待在此基础上浮动两成，避免齐步重连。
const RECONNECT_MIN: Duration = Duration::from_secs(1);
const RECONNECT_MAX: Duration = Duration::from_secs(30);

/// 一条连接最多攒多少个事件没人取。攒满说明调用方已经不读了。
const EVENT_BUFFER: usize = 64;

/// 半行数据的上限。服务端的一行永远比它小；超过说明对端不是在说 SSE。
const MAX_PENDING_BYTES: usize = 1024 * 1024;

/// 事件流。读法与任何 `Stream` 一样：
///
/// ```ignore
/// while let Some(result) = stream.next().await { … }
/// ```
pub struct EventStream {
    inner: ReceiverStream<Result<Event, ClientError>>,
}

impl Stream for EventStream {
    type Item = Result<Event, ClientError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.inner).poll_next(cx)
    }
}

impl std::fmt::Debug for EventStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventStream").finish_non_exhaustive()
    }
}

/// 一次连接的结局。
enum Ended {
    /// 调用方取消了。
    Cancelled,
    /// 服务端表示登录已失效。
    Unauthorized(ClientError),
    /// 连接结束或中断。`error` 为 `None` 表示服务端自己正常地关了连接。
    Closed {
        healthy: bool,
        error: Option<ClientError>,
    },
}

impl Client {
    /// 打开事件流。必须在 tokio 运行时里调用。
    ///
    /// 调用方丢掉返回的流，后台的连接就会结束。
    pub fn events(self, cancel: CancellationToken) -> EventStream {
        self.events_with(cancel, EVENT_IDLE_TIMEOUT)
    }

    /// 同 [`Client::events`]，但自己指定多久没有内容算连接已死。
    pub fn events_with(self, cancel: CancellationToken, idle: Duration) -> EventStream {
        let (sender, receiver) = mpsc::channel(EVENT_BUFFER);
        let client = self;
        tokio::spawn(async move {
            let mut wait = RECONNECT_MIN;
            loop {
                if cancel.is_cancelled() {
                    break;
                }
                let ended = client.connect_events(&cancel, &sender, idle).await;
                match ended {
                    Ended::Cancelled => break,
                    Ended::Unauthorized(error) => {
                        let _ = sender.send(Err(error)).await;
                        break;
                    }
                    Ended::Closed { healthy, error } => {
                        if let Some(error) = error {
                            // 送不出去说明没人读了。
                            if sender.send(Err(error)).await.is_err() {
                                break;
                            }
                        }
                        if healthy {
                            wait = RECONNECT_MIN;
                        }
                        let jitter = 0.8 + random_unit() * 0.4;
                        tokio::select! {
                            () = cancel.cancelled() => break,
                            () = tokio::time::sleep(wait.mul_f64(jitter)) => {}
                        }
                        wait = (wait * 2).min(RECONNECT_MAX);
                    }
                }
            }
        });
        EventStream {
            inner: ReceiverStream::new(receiver),
        }
    }

    /// 连一次，把收到的事件送进通道，直到连接结束。
    async fn connect_events(
        &self,
        cancel: &CancellationToken,
        sender: &mpsc::Sender<Result<Event, ClientError>>,
        idle: Duration,
    ) -> Ended {
        let mut request = self
            .bare_request(reqwest::Method::GET, EVENT_STREAM_PATH)
            .expect("the event stream path is a fixed, allowed path")
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .header(reqwest::header::CACHE_CONTROL, "no-cache");
        if let Some(authorization) = self.authorization() {
            request = request.header(reqwest::header::AUTHORIZATION, authorization.as_str());
        }
        let response = tokio::select! {
            biased;
            () = cancel.cancelled() => return Ended::Cancelled,
            outcome = request.send() => match outcome {
                Ok(response) => response,
                Err(error) => {
                    return Ended::Closed {
                        healthy: false,
                        error: Some(transport(&error)),
                    }
                }
            },
        };
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Ended::Unauthorized(rejection(status.as_u16(), response).await);
        }
        if !status.is_success() {
            return Ended::Closed {
                healthy: false,
                error: Some(rejection(status.as_u16(), response).await),
            };
        }
        let is_event_stream = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value
                    .trim_start()
                    .to_ascii_lowercase()
                    .starts_with("text/event-stream")
            });
        if !is_event_stream {
            return Ended::Closed {
                healthy: false,
                error: Some(ClientError::InvalidResponse),
            };
        }

        let mut pending: Vec<u8> = Vec::new();
        let mut healthy = false;
        let mut stream = response.bytes_stream();
        loop {
            let chunk: Bytes = tokio::select! {
                biased;
                () = cancel.cancelled() => return Ended::Cancelled,
                chunk = tokio::time::timeout(idle, futures_util::StreamExt::next(&mut stream)) => {
                    match chunk {
                        Ok(Some(Ok(bytes))) => bytes,
                        // 服务端关了连接。
                        Ok(Some(Err(error))) => {
                            return Ended::Closed {
                                healthy,
                                error: Some(transport(&error)),
                            }
                        }
                        Ok(None) => return Ended::Closed { healthy, error: None },
                        // 三个心跳周期没有任何内容：这条连接已经死了。
                        Err(_) => return Ended::Closed { healthy: false, error: Some(ClientError::Timeout) },
                    }
                }
            };
            pending.extend_from_slice(&chunk);
            if pending.len() > MAX_PENDING_BYTES {
                return Ended::Closed {
                    healthy: false,
                    error: Some(ClientError::InvalidResponse),
                };
            }
            for event in take_events(&mut pending) {
                if let Event::Hello { .. } = event {
                    healthy = true;
                }
                if sender.send(Ok(event)).await.is_err() {
                    return Ended::Cancelled;
                }
            }
        }
    }
}

/// 从攒下的字节里取出所有完整的事件，剩下的半截留在原地。
fn take_events(pending: &mut Vec<u8>) -> Vec<Event> {
    let mut events = Vec::new();
    let mut data: Vec<Vec<u8>> = Vec::new();
    let mut start = 0;
    while let Some(offset) = pending[start..].iter().position(|&byte| byte == b'\n') {
        let mut line = &pending[start..start + offset];
        if line.last() == Some(&b'\r') {
            line = &line[..line.len() - 1];
        }
        start += offset + 1;
        if line.is_empty() {
            if let Some(event) = decode_event(&data) {
                events.push(event);
            }
            data.clear();
        } else if let Some(value) = line.strip_prefix(b"data:") {
            data.push(value.strip_prefix(b" ").unwrap_or(value).to_vec());
        }
        // `event:`、`id:`、`retry:` 与冒号开头的注释（心跳）都不用。
    }
    pending.drain(..start);
    events
}

/// 按 SSE 的规则把多行 `data` 拼回去再解析。解析不了的事件丢弃。
fn decode_event(data: &[Vec<u8>]) -> Option<Event> {
    let mut joined = Vec::new();
    for line in data {
        joined.extend_from_slice(line);
        joined.push(b'\n');
    }
    joined.pop();
    serde_json::from_slice(&joined).ok()
}

/// 把 53 位整数映射到 `[0, 1)` 的双精度浮点数。
const UNIT_BIAS: u64 = 1023 << 52;

fn random_unit() -> f64 {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).unwrap_or(());
    f64::from_bits((u64::from_be_bytes(bytes) & ((1u64 << 53) - 1)) | UNIT_BIAS) - 1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events_from(bytes: &[u8]) -> Vec<Event> {
        let mut pending = bytes.to_vec();
        take_events(&mut pending)
    }

    #[test]
    fn frames_split_on_blank_lines_and_ignores_everything_but_data() {
        let wire = concat!(
            ": ping\n\n",
            "event: hello\n",
            "id: 7\n",
            "retry: 15000\n",
            "data: {\"type\":\"hello\",\"deviceId\":\"d1\"}\n\n",
            "data: {\"type\":\"devices_changed\",\"devices\":[]}\r\n\r\n",
            "retry: 1000\n",
        );
        let events = events_from(wire.as_bytes());
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].kind(), "hello");
        assert_eq!(events[1].kind(), "devices_changed");
    }

    #[test]
    fn a_frame_cut_across_chunks_is_held_until_it_is_complete() {
        let wire =
            b"data: {\"type\":\"hello\",\"deviceId\":\"d1\"}\n\ndata: {\"type\":\"send_offered\"";
        let mut pending = wire.to_vec();
        assert_eq!(take_events(&mut pending).len(), 1);
        assert_eq!(pending, b"data: {\"type\":\"send_offered\"");
        pending.extend_from_slice(b",\"send\":{}}\n\n");
        let rest = take_events(&mut pending);
        // 结构不完整的发送解析不了：整条事件丢弃，不影响后面的。
        assert!(rest.is_empty());
        assert!(pending.is_empty());
    }

    #[test]
    fn events_this_version_does_not_know_are_skipped_not_fatal() {
        let wire = concat!(
            "data: {\"type\":\"future_thing\",\"x\":1}\n\n",
            "data: {\"type\":\"hello\",\"deviceId\":\"d1\"}\n\n",
        );
        let events = events_from(wire.as_bytes());
        assert_eq!(
            events,
            vec![Event::Hello {
                device_id: "d1".into()
            }]
        );
    }

    #[test]
    fn multi_line_data_is_joined_with_newlines() {
        let wire = "data: {\"type\":\"hello\",\ndata: \"deviceId\":\"d1\"}\n\n";
        let events = events_from(wire.as_bytes());
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind(), "hello");
    }

    #[test]
    fn a_line_that_does_not_start_with_a_field_name_is_not_data() {
        // 行首多了一个空格：它不是 `data:` 字段，这一帧没有数据。
        let wire = "data: x\n data: {\"type\":\"hello\",\"deviceId\":\"d1\"}\n\n";
        assert!(events_from(wire.as_bytes()).is_empty());
    }

    #[test]
    fn the_idle_timeout_is_three_heartbeats() {
        assert_eq!(EVENT_IDLE_TIMEOUT, Duration::from_secs(45));
        assert!(RECONNECT_MIN < RECONNECT_MAX);
        assert!(random_unit() >= 0.0 && random_unit() < 1.0);
    }
}
