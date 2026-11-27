use super::*;
use assemble_protocol::{primitives::Timestamp, remote::*};
use assemble_remote::worker::Backend;
use assemble_store::secret::SecretString;
use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    routing::{get, post},
};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug, thiserror::Error)]
#[error("test native failure")]
struct Failed;
#[derive(Default)]
struct Probe {
    offer_input: AtomicBool,
    block_upload: AtomicBool,
    fail_capture: AtomicBool,
    pending: AtomicBool,
    captured: AtomicU32,
    uploaded: AtomicU32,
    applied: AtomicU32,
    released: AtomicU32,
    display: AtomicU32,
    upload_entered: tokio::sync::Notify,
    unblock_upload: tokio::sync::Notify,
    changed: tokio::sync::Notify,
    end_reason: Mutex<Option<RemoteEndReason>>,
}
struct Fake(Arc<Probe>);
impl Backend for Fake {
    type Error = Failed;
    fn frame(&mut self) -> Result<RemoteFrame, Failed> {
        if self.0.fail_capture.load(Ordering::SeqCst) {
            return Err(Failed);
        }
        let sequence = self.0.captured.fetch_add(1, Ordering::SeqCst) + 1;
        self.0.changed.notify_one();
        Ok(RemoteFrame {
            sequence,
            display_id: self.0.display.load(Ordering::SeqCst),
            width: 1,
            height: 1,
            jpeg: "AA==".into(),
            displays: (1..=2)
                .map(|id| RemoteDisplay {
                    id,
                    name: format!("Test {id}"),
                    width: 1,
                    height: 1,
                })
                .collect(),
        })
    }
    fn apply(&mut self, input: &RemoteInput) -> Result<(), Failed> {
        if let RemoteInput::Display { id } = input {
            self.0.display.store(*id, Ordering::SeqCst);
        }
        self.0.applied.fetch_add(1, Ordering::SeqCst);
        self.0.changed.notify_one();
        Ok(())
    }
    fn release_all(&mut self) -> Result<(), Failed> {
        self.0.released.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
fn session(pending: bool) -> RemoteSession {
    RemoteSession {
        id: "session".into(),
        controller_id: "controller".into(),
        controller_name: "Controller".into(),
        host_id: "host".into(),
        host_name: "Host".into(),
        phase: if pending {
            RemotePhase::Pending
        } else {
            RemotePhase::Active
        },
        created_at: Timestamp::now(),
        end_reason: None,
    }
}
#[derive(serde::Deserialize)]
struct Ack {
    #[serde(default)]
    after: u32,
}
async fn poll(State(probe): State<Arc<Probe>>, Query(query): Query<Ack>) -> Json<RemotePoll> {
    let inputs = if probe.offer_input.load(Ordering::SeqCst) && query.after == 0 {
        vec![SequencedInput {
            sequence: 1,
            input: RemoteInput::Display { id: 2 },
        }]
    } else {
        Vec::new()
    };
    Json(RemotePoll {
        session: session(probe.pending.load(Ordering::SeqCst)),
        frame: None,
        inputs,
    })
}
async fn upload(State(probe): State<Arc<Probe>>) -> StatusCode {
    probe.uploaded.fetch_add(1, Ordering::SeqCst);
    probe.upload_entered.notify_one();
    probe.changed.notify_one();
    if probe.block_upload.load(Ordering::SeqCst) {
        probe.unblock_upload.notified().await;
    }
    StatusCode::NO_CONTENT
}
async fn end(State(probe): State<Arc<Probe>>, Json(input): Json<RemoteEnd>) -> StatusCode {
    *probe.end_reason.lock().unwrap() = Some(input.reason);
    StatusCode::NO_CONTENT
}
struct Server {
    client: Client,
    probe: Arc<Probe>,
    stop: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}
impl Server {
    async fn start() -> Self {
        let probe = Arc::new(Probe::default());
        probe.display.store(1, Ordering::SeqCst);
        let router = Router::new()
            .route("/v1/remote/sessions/session/poll", get(poll))
            .route("/v1/remote/sessions/session/frame", post(upload))
            .route("/v1/remote/sessions/session/end", post(end))
            .with_state(probe.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin =
            crate::ServerOrigin::parse(&format!("http://{}", listener.local_addr().unwrap()))
                .unwrap();
        let stop = CancellationToken::new();
        let until = stop.clone();
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(until.cancelled_owned())
                .await
                .unwrap();
        });
        let client = Client::new(origin).unwrap();
        client.set_bearer(Some(SecretString::new("synthetic-test-session")));
        Self {
            client,
            probe,
            stop,
            task,
        }
    }
    async fn worker(&self) -> HostWorker<Failed> {
        let probe = self.probe.clone();
        HostWorker::start(move || Ok(Fake(probe)), CancellationToken::new())
            .await
            .unwrap()
    }
    async fn wait(&self, condition: impl Fn() -> bool) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while !condition() {
                self.probe.changed.notified().await;
            }
        })
        .await
        .unwrap();
    }
    async fn close(self) {
        self.probe.unblock_upload.notify_waiters();
        self.stop.cancel();
        self.task.await.unwrap();
    }
}

#[tokio::test]
async fn slow_upload_does_not_delay_inputs_or_native_cleanup() {
    let server = Server::start().await;
    server.probe.block_upload.store(true, Ordering::SeqCst);
    let worker = server.worker().await;
    let cancel = CancellationToken::new();
    let until = cancel.clone();
    let client = server.client.clone();
    let task = tokio::spawn(async move { client.run_remote_host("session", worker, &until).await });
    tokio::time::timeout(
        Duration::from_secs(3),
        server.probe.upload_entered.notified(),
    )
    .await
    .unwrap();
    server.probe.offer_input.store(true, Ordering::SeqCst);
    server
        .wait(|| server.probe.applied.load(Ordering::SeqCst) == 1)
        .await;
    // 上传仍被服务端挡住，原生输入必须已经完成；停止也不等上传超时。
    assert_eq!(server.probe.released.load(Ordering::SeqCst), 0);
    cancel.cancel();
    let result = tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap();
    assert!(result.is_ok());
    assert_eq!(server.probe.released.load(Ordering::SeqCst), 1);
    server.close().await;
}
#[tokio::test]
async fn static_pictures_are_not_uploaded_again_but_display_changes_are() {
    let server = Server::start().await;
    let worker = server.worker().await;
    let cancel = CancellationToken::new();
    let until = cancel.clone();
    let client = server.client.clone();
    let task = tokio::spawn(async move { client.run_remote_host("session", worker, &until).await });
    server
        .wait(|| server.probe.captured.load(Ordering::SeqCst) >= 3)
        .await;
    assert_eq!(server.probe.uploaded.load(Ordering::SeqCst), 1);
    server.probe.offer_input.store(true, Ordering::SeqCst);
    server
        .wait(|| server.probe.uploaded.load(Ordering::SeqCst) == 2)
        .await;
    let next_capture = server.probe.captured.load(Ordering::SeqCst) + 1;
    server
        .wait(|| server.probe.captured.load(Ordering::SeqCst) >= next_capture)
        .await;
    cancel.cancel();
    let stats = task.await.unwrap().unwrap();
    assert_eq!(stats.frames_sent, 2);
    assert!(stats.frames_unchanged >= 1);
    assert_eq!(stats.inputs_applied, 1);
    assert_eq!(server.probe.released.load(Ordering::SeqCst), 1);
    server.close().await;
}
#[tokio::test]
async fn capture_failure_is_reported_and_no_capture_happens_before_activation() {
    for pending in [false, true] {
        let server = Server::start().await;
        server.probe.pending.store(pending, Ordering::SeqCst);
        server.probe.fail_capture.store(true, Ordering::SeqCst);
        let result = server
            .client
            .run_remote_host("session", server.worker().await, &CancellationToken::new())
            .await;
        if pending {
            assert!(result.is_ok());
        } else {
            assert!(matches!(result, Err(HostError::Capture(_))));
        }
        assert_eq!(
            *server.probe.end_reason.lock().unwrap(),
            Some(if pending {
                RemoteEndReason::Stopped
            } else {
                RemoteEndReason::CaptureFailed
            })
        );
        assert_eq!(server.probe.released.load(Ordering::SeqCst), 1);
        server.close().await;
    }
}
