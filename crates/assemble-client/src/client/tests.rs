//! 客户端的集成测试：对一个真实的本地 HTTP 服务发请求。
//!
//! 服务端把收到的每个请求原样记下来，再按预设给出应答。这样请求头、路径、
//! 查询串与正文都能被逐项核对。

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::error::ClientError;
use assemble_protocol::device::NewDevice;
use assemble_protocol::login::MeResponse;
use assemble_protocol::primitives::Platform;
use assemble_protocol::sends::{Direction, FailReason, SendState};

#[tokio::test]
async fn resumable_capability_only_falls_back_for_explicit_not_found() {
    for status in [
        StatusCode::NOT_FOUND,
        StatusCode::UNAUTHORIZED,
        StatusCode::SERVICE_UNAVAILABLE,
    ] {
        let server = Server::start(with(move |_| {
            json_response(
                status,
                json!({"error":{"code":"UNAVAILABLE","message":"private diagnostic"}}),
            )
        }))
        .await;
        let client = server.client();
        client.set_bearer(Some(SecretString::new("test-upload-only")));
        let cancel = CancellationToken::new();
        let result = client.upload_supported(&RequestOptions::new(&cancel)).await;
        if status == StatusCode::NOT_FOUND {
            assert!(!result.unwrap());
        } else {
            assert!(result.is_err());
        }
        let requests = server.requests(1).await;
        assert_eq!(requests[0].path, "/v1/drive/uploads");
    }
}

#[tokio::test]
async fn drive_tree_uses_a_separate_namespace_and_never_falls_back_for_writes() {
    use assemble_protocol::drive_tree::{Create, Query, ROOT};
    let server = Server::start(with(|_| {
        json_response(
            StatusCode::NOT_FOUND,
            json!({"error":{"code":"NOT_FOUND","message":"old server"}}),
        )
    }))
    .await;
    let client = server.client();
    client.set_bearer(Some(SecretString::new("test-directory-only")));
    let cancel = CancellationToken::new();
    let options = RequestOptions::new(&cancel);
    let error = client
        .drive_tree(
            &Query {
                q: "中文%_".into(),
                page_size: 1,
                ..Query::default()
            },
            &options,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, ClientError::Http { status: 404, .. }));
    let requests = server.requests(1).await;
    assert_eq!(requests[0].path, "/v1/drive/tree");
    assert_eq!(requests[0].params()["q"], "中文%_");
    assert!(
        client
            .drive_folder_create(
                &Create {
                    request_id: "key".into(),
                    parent_id: ROOT.into(),
                    name: "新文件夹".into()
                },
                &options
            )
            .await
            .is_err()
    );
    assert_eq!(server.requests(2).await[1].path, "/v1/drive/tree/folders");
    assert!(
        client
            .drive_tree(
                &Query {
                    page_size: 101,
                    ..Query::default()
                },
                &options
            )
            .await
            .is_err()
    );
    assert_eq!(server.received(), 2);
    cancel.cancel();
    assert!(matches!(
        client.drive_tree(&Query::default(), &options).await,
        Err(ClientError::Cancelled)
    ));
    assert_eq!(server.received(), 2);
}

/// 服务端收到的一个请求。
#[derive(Debug, Clone)]
struct Captured {
    method: Method,
    path: String,
    query: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Captured {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    /// 查询串解析成对象；同名参数只留最后一个。
    fn params(&self) -> serde_json::Map<String, Value> {
        url::form_urlencoded::parse(self.query.as_bytes())
            .into_iter()
            .map(|(key, value)| (key.into_owned(), Value::String(value.into_owned())))
            .collect()
    }
}

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// 根据收到的请求给出应答；应答可以拖延，也可以是一条不会结束的流。
type Responder = Arc<dyn Fn(Captured) -> BoxFuture<Response> + Send + Sync>;

/// 按请求同步决定应答。
fn with<F>(responder: F) -> Responder
where
    F: Fn(Captured) -> Response + Send + Sync + 'static,
{
    Arc::new(move |captured| {
        let response = responder(captured);
        Box::pin(async move { response })
    })
}

fn json_response(status: StatusCode, body: Value) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::to_string(&body).unwrap(),
    )
        .into_response()
}

/// 每次都给同一个 JSON 应答。
fn instant(status: StatusCode, body: Value) -> Responder {
    with(move |_| json_response(status, body.clone()))
}

/// 收到请求后拖延 `delay` 再给一个 JSON 应答。
fn delayed(delay: Duration, body: Value) -> Responder {
    Arc::new(move |_| {
        let body = body.clone();
        Box::pin(async move {
            tokio::time::sleep(delay).await;
            json_response(StatusCode::OK, body)
        })
    })
}

/// 事件流应答：按顺序发若干段，然后关掉连接（`hang` 为假）或永远挂着。
fn event_stream(chunks: Vec<&'static str>, hang: bool) -> Responder {
    with(move |_| {
        let chunks = chunks.clone();
        let sent = futures_util::stream::iter(
            chunks
                .into_iter()
                .map(|chunk| Ok::<_, std::io::Error>(Bytes::from_static(chunk.as_bytes()))),
        );
        let stream: Pin<
            Box<dyn futures_core::Stream<Item = Result<Bytes, std::io::Error>> + Send>,
        > = if hang {
            Box::pin(sent.chain(futures_util::stream::pending()))
        } else {
            Box::pin(sent)
        };
        (
            [(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")],
            Body::from_stream(stream),
        )
            .into_response()
    })
}

struct Server {
    calls: Arc<Mutex<Vec<Captured>>>,
    origin: ServerOrigin,
    _task: tokio::task::JoinHandle<()>,
}

impl Server {
    async fn start(responder: Responder) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let state = Arc::new(Shared {
            calls: Arc::clone(&calls),
            responder,
        });
        let router: Router = Router::new().fallback(handle).with_state(state);
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            calls,
            origin: ServerOrigin::parse(&format!("http://127.0.0.1:{port}")).unwrap(),
            _task: task,
        }
    }

    fn client(&self) -> Client {
        Client::new(self.origin.clone()).unwrap()
    }

    /// 等到服务端收到第 `count` 个请求，再把它们都交出来。
    async fn requests(&self, count: usize) -> Vec<Captured> {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let calls = self.calls.lock().unwrap().clone();
                if calls.len() >= count {
                    return calls;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the server should receive the request")
    }

    fn received(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

struct Shared {
    calls: Arc<Mutex<Vec<Captured>>>,
    responder: Responder,
}

async fn handle(State(state): State<Arc<Shared>>, request: Request<Body>) -> Response {
    let (parts, body) = request.into_parts();
    let bytes = axum::body::to_bytes(body, 8 * 1024 * 1024)
        .await
        .unwrap_or_default();
    let captured = Captured {
        method: parts.method,
        path: parts.uri.path().to_owned(),
        query: parts.uri.query().unwrap_or_default().to_owned(),
        headers: parts
            .headers
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_ascii_lowercase(),
                    String::from_utf8_lossy(value.as_bytes()).into_owned(),
                )
            })
            .collect(),
        body: bytes.to_vec(),
    };
    // 先记录、再应答：应答拖延的时候调用方也能看到这条请求已经到达。
    let responder = Arc::clone(&state.responder);
    state.calls.lock().unwrap().push(captured.clone());
    responder(captured).await
}

fn options<'a>(cancel: &'a CancellationToken) -> RequestOptions<'a> {
    RequestOptions::new(cancel)
}

#[tokio::test]
async fn preview_rejects_wrong_ranges_and_does_not_follow_redirects() {
    for (status, range, length) in [
        (StatusCode::OK, "bytes 0-2/10", 3),
        (StatusCode::PARTIAL_CONTENT, "bytes 1-3/10", 3),
        (StatusCode::PARTIAL_CONTENT, "bytes 0-2/10", 2),
        (StatusCode::FOUND, "bytes 0-2/10", 3),
    ] {
        let server = Server::start(with(move |_| {
            (
                status,
                [
                    (header::CONTENT_RANGE, range),
                    (header::CONTENT_TYPE, "image/png"),
                    (header::LOCATION, "http://127.0.0.1:1/credential-leak"),
                ],
                vec![0_u8; length],
            )
                .into_response()
        }))
        .await;
        let client = server.client();
        client.set_bearer(Some(SecretString::new("test-preview-only")));
        let cancel = CancellationToken::new();
        assert!(
            client
                .drive_preview_chunk("file-1", 0, 2, 10, &options(&cancel))
                .await
                .is_err()
        );
        assert_eq!(server.received(), 1);
        assert_eq!(
            server.requests(1).await[0].header("range"),
            Some("bytes=0-2")
        );
    }
}

#[tokio::test]
async fn closing_preview_cancels_a_pending_read_and_invalid_ranges_do_not_send_requests() {
    let server = Server::start(delayed(Duration::from_secs(60), Value::Null)).await;
    let client = server.client();
    client.set_bearer(Some(SecretString::new("test-preview-only")));
    let cancel = CancellationToken::new();
    assert!(matches!(
        client
            .drive_preview_chunk("file-1", 2, 1, 10, &options(&cancel))
            .await,
        Err(ClientError::InvalidRequest)
    ));
    assert_eq!(server.received(), 0);
    let settings = options(&cancel);
    let read = client.drive_preview("file-1", &settings);
    let abort = async {
        server.requests(1).await;
        cancel.cancel();
    };
    let (result, ()) = tokio::join!(read, abort);
    assert!(matches!(result, Err(ClientError::Cancelled)));
}

fn http(status: u16, code: &str) -> ClientError {
    ClientError::Http {
        status,
        remote_code: Some(code.to_owned()),
    }
}

fn account() -> Value {
    json!({
        "id": "a1",
        "username": "example",
        "displayName": "Example",
        "createdAt": "2026-11-01T00:00:00.000Z"
    })
}

fn device(id: &str, online: bool) -> Value {
    json!({
        "id": id,
        "name": "工作电脑",
        "platform": "windows",
        "online": online,
        "createdAt": "2026-11-01T00:00:00.000Z"
    })
}

fn me_response() -> Value {
    json!({ "account": account(), "device": device("d1", true) })
}

fn exchange_input() -> ExchangeInput {
    ExchangeInput {
        code: "c".repeat(43),
        code_verifier: "v".repeat(43),
        redirect_uri: "http://127.0.0.1:54321/callback".into(),
        device: NewDevice {
            name: "MacBook".into(),
            platform: Platform::Macos,
        },
    }
}

/// 写一个测试文件，并等它真正落盘。tokio 的文件写入先经过缓冲；
/// 不等的话，紧接着读取的线程会看到一个还在变化的文件。
async fn write_file(path: &std::path::Path, bytes: &[u8]) {
    use tokio::io::AsyncWriteExt;

    let mut file = tokio::fs::File::create(path).await.unwrap();
    file.write_all(bytes).await.unwrap();
    file.sync_all().await.unwrap();
}

fn pattern(length: usize) -> Vec<u8> {
    (0..length).map(|index| (index % 251) as u8).collect()
}

fn sha256(bytes: &[u8]) -> String {
    assemble_transfer::hash::digest(bytes)
}

async fn read_all(reader: impl tokio::io::AsyncRead) -> std::io::Result<Vec<u8>> {
    let mut buffer = Vec::new();
    let mut reader = std::pin::pin!(reader);
    reader.read_to_end(&mut buffer).await?;
    Ok(buffer)
}

// ---- 探活、换码、账户 ----

#[tokio::test]
async fn health_is_public_and_must_say_ok() {
    let server = Server::start(instant(StatusCode::OK, json!({ "status": "ok" }))).await;
    let client = server.client();
    client.set_bearer(Some(SecretString::new("device-token")));

    client
        .health(&options(&CancellationToken::new()))
        .await
        .unwrap();

    let requests = server.requests(1).await;
    let request = &requests[0];
    assert_eq!(request.method, Method::GET);
    assert_eq!(request.path, "/healthz");
    assert_eq!(request.header("authorization"), None);
    assert_eq!(request.header("accept"), Some("application/json"));

    // 服务端没说 ok 就不算健康。
    let odd = Server::start(instant(StatusCode::OK, json!({ "status": "starting" }))).await;
    let error = odd
        .client()
        .health(&options(&CancellationToken::new()))
        .await;
    assert_eq!(error.unwrap_err(), ClientError::InvalidResponse);

    // 探活失败如实报告状态码。
    let down = Server::start(instant(StatusCode::NOT_FOUND, json!({ "x": 1 }))).await;
    let error = down
        .client()
        .health(&options(&CancellationToken::new()))
        .await;
    assert_eq!(error.unwrap_err().status(), Some(404));
}

#[tokio::test]
async fn exchange_posts_the_input_without_credentials_and_hands_back_a_token() {
    let server = Server::start(instant(
        StatusCode::OK,
        json!({
            "account": account(),
            "device": device("d1", false),
            "token": "t0ken-value"
        }),
    ))
    .await;
    let client = server.client();
    // 即使手里已有旧令牌，换码请求也不带它。
    client.set_bearer(Some(SecretString::new("old-token")));

    let (response, token) = client
        .exchange(&exchange_input(), &options(&CancellationToken::new()))
        .await
        .unwrap();
    assert_eq!(token.expose(), "t0ken-value");
    assert_eq!(response.account.username, "example");
    assert_eq!(response.device.id, "d1");
    assert!(
        response.token.is_empty(),
        "the token moved out of the response"
    );

    let requests = server.requests(1).await;
    let request = &requests[0];
    assert_eq!(request.method, Method::POST);
    assert_eq!(request.path, "/v1/auth/desktop/exchange");
    assert_eq!(request.header("authorization"), None);
    assert_eq!(request.header("content-type"), Some("application/json"));
    assert_eq!(
        request.json(),
        json!({
            "code": "c".repeat(43),
            "codeVerifier": "v".repeat(43),
            "redirectUri": "http://127.0.0.1:54321/callback",
            "device": { "name": "MacBook", "platform": "macos" }
        })
    );

    // 不合法的输入在本地拦下，不发出去。
    let foreign = ExchangeInput {
        redirect_uri: "http://evil.example/callback".into(),
        ..exchange_input()
    };
    let error = client
        .exchange(&foreign, &options(&CancellationToken::new()))
        .await;
    assert_eq!(error.unwrap_err(), ClientError::InvalidRequest);
    assert_eq!(server.received(), 1);
}

#[tokio::test]
async fn authenticated_requests_carry_the_bearer_and_keep_the_rejection_details() {
    let server = Server::start(instant(StatusCode::OK, me_response())).await;
    let client = server.client();

    // 还没登录：在本地拦下，不发请求。
    let error = client.me(&options(&CancellationToken::new())).await;
    assert_eq!(error.unwrap_err(), ClientError::InvalidRequest);
    assert_eq!(server.received(), 0);

    client.set_bearer(Some(SecretString::new("device-token")));
    let me: MeResponse = client
        .me(&options(&CancellationToken::new()))
        .await
        .unwrap();
    assert_eq!(me.device.platform, Platform::Windows);
    assert_eq!(me.account.username, "example");

    let requests = server.requests(1).await;
    assert_eq!(requests[0].path, "/v1/me");
    assert_eq!(
        requests[0].header("authorization"),
        Some("Bearer device-token")
    );

    // 退出登录之后令牌不再随之发送。
    client.set_bearer(None);
    assert!(!client.has_bearer());
    let error = client.me(&options(&CancellationToken::new())).await;
    assert_eq!(error.unwrap_err(), ClientError::InvalidRequest);

    // 服务端的拒绝保留状态码与稳定错误码。
    let rejecting = Server::start(instant(
        StatusCode::CONFLICT,
        json!({ "error": { "code": "DEVICE_OFFLINE", "message": "gone" } }),
    ))
    .await;
    let client = rejecting.client();
    client.set_bearer(Some(SecretString::new("device-token")));
    let error = client
        .me(&options(&CancellationToken::new()))
        .await
        .unwrap_err();
    assert_eq!(error, http(409, "DEVICE_OFFLINE"));
    assert!(!error.is_retryable());

    // 看不懂的成功应答算无效应答。
    let broken = Server::start(instant(StatusCode::OK, json!({ "unexpected": true }))).await;
    let client = broken.client();
    client.set_bearer(Some(SecretString::new("device-token")));
    let error = client.me(&options(&CancellationToken::new())).await;
    assert_eq!(error.unwrap_err(), ClientError::InvalidResponse);
}

#[tokio::test]
async fn logout_is_an_empty_post() {
    let server = Server::start(instant(StatusCode::NO_CONTENT, Value::Null)).await;
    let client = server.client();
    client.set_bearer(Some(SecretString::new("device-token")));

    client
        .logout(&options(&CancellationToken::new()))
        .await
        .unwrap();

    let requests = server.requests(1).await;
    assert_eq!(requests[0].method, Method::POST);
    assert_eq!(requests[0].path, "/v1/auth/logout");
    assert!(requests[0].body.is_empty());
    assert_eq!(
        requests[0].header("authorization"),
        Some("Bearer device-token")
    );
}

// ---- 设备 ----

#[tokio::test]
async fn devices_are_listed_renamed_and_removed() {
    let server = Server::start(with(|request: Captured| {
        match (request.method, request.path.as_str()) {
            (Method::GET, "/v1/devices") => json_response(
                StatusCode::OK,
                json!({ "items": [device("d1", true), device("d2", false)] }),
            ),
            (Method::PATCH, "/v1/devices/d2") => json_response(StatusCode::OK, device("d2", false)),
            (Method::DELETE, "/v1/devices/d2") => {
                (StatusCode::NO_CONTENT, [(header::CONTENT_LENGTH, "0")]).into_response()
            }
            _ => json_response(
                StatusCode::NOT_FOUND,
                json!({ "error": { "code": "NOT_FOUND" } }),
            ),
        }
    }))
    .await;
    let client = server.client();
    client.set_bearer(Some(SecretString::new("device-token")));
    let cancel = CancellationToken::new();

    let devices = client.devices(&options(&cancel)).await.unwrap();
    assert_eq!(devices.len(), 2);
    assert!(devices[0].online);
    assert!(!devices[1].online);

    let renamed = client
        .rename_device(
            "d2",
            &RenameDeviceInput {
                name: "家里的电脑".into(),
            },
            &options(&cancel),
        )
        .await
        .unwrap();
    assert_eq!(renamed.id, "d2");

    client.remove_device("d2", &options(&cancel)).await.unwrap();

    let requests = server.requests(3).await;
    assert_eq!(requests[1].json(), json!({ "name": "家里的电脑" }));
    assert_eq!(requests[2].method, Method::DELETE);
    assert_eq!(requests[2].path, "/v1/devices/d2");

    // 会被 URL 规范化吃掉一段的标识，在本地拦下。
    for bad_id in ["..", ".", "a/b", "a b", ""] {
        let error = client.remove_device(bad_id, &options(&cancel)).await;
        assert_eq!(
            error.unwrap_err(),
            ClientError::InvalidRequest,
            "{bad_id:?}"
        );
    }
    assert_eq!(server.received(), 3);

    // 不合法的设备名也不发出去。
    let error = client
        .rename_device(
            "d2",
            &RenameDeviceInput {
                name: "a\nb".into(),
            },
            &options(&cancel),
        )
        .await;
    assert_eq!(error.unwrap_err(), ClientError::InvalidRequest);
    assert_eq!(server.received(), 3);
}

// ---- 上传 ----

#[tokio::test]
async fn drive_upload_streams_the_file_with_its_declared_size_and_digest() {
    let server = Server::start(instant(
        StatusCode::CREATED,
        json!({
            "id": "f1",
            "name": "周报.docx",
            "size": 3,
            "sha256": "a".repeat(64),
            "createdAt": "2026-11-01T00:00:00.000Z"
        }),
    ))
    .await;
    let client = server.client();
    client.set_bearer(Some(SecretString::new("device-token")));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("周报.docx");
    let payload = pattern(300_000);
    write_file(&path, &payload).await;
    let file = OutgoingFile::open(&path, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(file.sha256, sha256(&payload));

    let mut reports = Vec::new();
    let uploaded = client
        .drive_upload(
            "周报.docx",
            &file,
            TransferOptions::new(&CancellationToken::new()),
            Some(&mut |progress: TransferProgress| reports.push(progress)),
        )
        .await
        .unwrap();
    assert_eq!(uploaded.name, "周报.docx");

    let requests = server.requests(1).await;
    let request = &requests[0];
    assert_eq!(request.method, Method::POST);
    assert_eq!(request.path, "/v1/drive/files");
    // 正文就是文件的字节，长度在请求头里声明，不是分块传输。
    assert_eq!(request.body, payload);
    assert_eq!(
        request.header("content-length"),
        Some(payload.len().to_string().as_str())
    );
    assert_eq!(request.header("transfer-encoding"), None);
    assert_eq!(
        request.header("content-type"),
        Some("application/octet-stream")
    );
    assert_eq!(request.header("authorization"), Some("Bearer device-token"));
    assert_eq!(
        request.params(),
        serde_json::from_value(json!({
            "name": "周报.docx",
            "size": payload.len().to_string(),
            "sha256": sha256(&payload),
            "mimeType": "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        }))
        .unwrap()
    );

    // 回环网络上传完 300 KB 用不了一个进度周期，所以这里只保证：至少报过一次，
    // 进度单调不减，最后一次是全部传完。周期性的中途汇报见下一个测试。
    assert!(!reports.is_empty(), "progress should be reported");
    assert!(
        reports
            .windows(2)
            .all(|pair| pair[0].transferred <= pair[1].transferred)
    );
    assert_eq!(reports.last().unwrap().transferred, payload.len() as u64);
    assert_eq!(reports.last().unwrap().fraction(), 1.0);
}

#[tokio::test]
async fn progress_is_reported_periodically_while_an_upload_is_in_flight() {
    // 服务端收完正文后拖一秒再应答。这段时间里进度要按周期汇报，而不是只在结束时报一次。
    let server = Server::start(delayed(
        Duration::from_millis(1000),
        json!({
            "id": "f1",
            "name": "notes.txt",
            "size": 5,
            "sha256": sha256(b"notes"),
            "createdAt": "2026-11-01T00:00:00.000Z"
        }),
    ))
    .await;
    let client = server.client();
    client.set_bearer(Some(SecretString::new("device-token")));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.txt");
    write_file(&path, b"notes").await;
    let file = OutgoingFile::open(&path, &CancellationToken::new())
        .await
        .unwrap();
    let cancel = CancellationToken::new();

    let mut reports = Vec::new();
    client
        .drive_upload(
            "notes.txt",
            &file,
            TransferOptions::new(&cancel),
            Some(&mut |progress: TransferProgress| reports.push(progress)),
        )
        .await
        .unwrap();

    assert!(
        reports.len() >= 3,
        "expected periodic reports, got {reports:?}"
    );
    // 第一次汇报在开头（此时一个字节都还没发），之后正文早已发完，每次都是全部字节。
    assert!(
        reports
            .iter()
            .all(|report| report.total == 5 && report.transferred <= 5)
    );
    assert!(
        reports
            .windows(2)
            .all(|pair| pair[0].transferred <= pair[1].transferred)
    );
    assert_eq!(reports.last().unwrap().transferred, 5);
}

#[tokio::test]
async fn send_to_device_names_its_target_in_the_query() {
    let server = Server::start(instant(
        StatusCode::CREATED,
        json!({
            "id": "s1",
            "direction": "out",
            "peerDeviceId": "d2",
            "peerDeviceName": "工作电脑",
            "name": "notes.txt",
            "size": 5,
            "sha256": "a".repeat(64),
            "state": "offered",
            "createdAt": "2026-11-01T00:00:00.000Z",
            "updatedAt": "2026-11-01T00:00:00.000Z"
        }),
    ))
    .await;
    let client = server.client();
    client.set_bearer(Some(SecretString::new("device-token")));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.txt");
    write_file(&path, b"notes").await;
    let file = OutgoingFile::open(&path, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(file.name(), "notes.txt");
    assert_eq!(file.mime_type(), "text/plain");

    let sent = client
        .send_to_device(
            "d2",
            &file,
            TransferOptions::new(&CancellationToken::new()),
            None,
        )
        .await
        .unwrap();
    assert_eq!(sent.state, SendState::Offered);

    let requests = server.requests(1).await;
    assert_eq!(requests[0].path, "/v1/sends");
    assert_eq!(requests[0].body, b"notes");
    assert_eq!(requests[0].params()["toDeviceId"], "d2");
    assert_eq!(requests[0].params()["name"], "notes.txt");
    assert_eq!(requests[0].params()["size"], "5");
    assert_eq!(requests[0].params()["sha256"], sha256(b"notes"));
    assert_eq!(requests[0].params()["mimeType"], "text/plain");

    // 目标标识不合法：本地拦下。
    let error = client
        .send_to_device(
            "../d2",
            &file,
            TransferOptions::new(&CancellationToken::new()),
            None,
        )
        .await;
    assert_eq!(error.unwrap_err(), ClientError::InvalidRequest);
    assert_eq!(server.received(), 1);
}

#[tokio::test]
async fn files_that_cannot_be_sent_are_refused_before_any_request() {
    let server = Server::start(instant(StatusCode::CREATED, json!({}))).await;
    let client = server.client();
    client.set_bearer(Some(SecretString::new("device-token")));
    let dir = tempfile::tempdir().unwrap();
    let cancel = CancellationToken::new();

    // 超过上限：先看大小，不读内容。用一个稀疏文件，不占磁盘。
    let huge = dir.path().join("huge.bin");
    let file = std::fs::File::create(&huge).unwrap();
    file.set_len(MAX_FILE_SIZE + 1).unwrap();
    drop(file);
    assert_eq!(
        OutgoingFile::open(&huge, &cancel).await.unwrap_err(),
        ClientError::TooLarge
    );
    // 恰好等于上限的文件可以发。
    let edge = dir.path().join("edge.bin");
    let file = std::fs::File::create(&edge).unwrap();
    file.set_len(MAX_FILE_SIZE).unwrap();
    drop(file);
    let edge = OutgoingFile::open(&edge, &cancel).await.unwrap();
    assert_eq!(edge.size, MAX_FILE_SIZE);

    // 目录、不存在的路径、读不出来的路径。
    for missing in [dir.path().to_path_buf(), dir.path().join("nope.bin")] {
        assert_eq!(
            OutgoingFile::open(&missing, &cancel).await.unwrap_err(),
            ClientError::LocalSource
        );
    }

    // 上传时再次核对上限：伪造一个超限的描述也发不出去。
    let forged = OutgoingFile {
        path: dir.path().join("edge.bin"),
        size: MAX_FILE_SIZE + 1,
        sha256: "a".repeat(64),
    };
    let error = client
        .drive_upload("edge.bin", &forged, TransferOptions::new(&cancel), None)
        .await;
    assert_eq!(error.unwrap_err(), ClientError::TooLarge);

    // 要发的文件在准备好之后不见了。
    let vanished = OutgoingFile {
        path: dir.path().join("gone.bin"),
        size: 1,
        sha256: "a".repeat(64),
    };
    let error = client
        .drive_upload("gone.bin", &vanished, TransferOptions::new(&cancel), None)
        .await;
    assert_eq!(error.unwrap_err(), ClientError::LocalSource);

    // 名字不能是个路径。
    let error = client
        .drive_upload("a/b.txt", &edge, TransferOptions::new(&cancel), None)
        .await;
    assert_eq!(error.unwrap_err(), ClientError::InvalidRequest);

    // 整个过程一个请求都没发出去。
    assert_eq!(server.received(), 0);
}

#[tokio::test]
async fn a_cancelled_upload_stops_without_waiting_for_the_server() {
    let server = Server::start(delayed(Duration::from_secs(30), json!({}))).await;
    let client = server.client();
    client.set_bearer(Some(SecretString::new("device-token")));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.txt");
    write_file(&path, b"notes").await;
    let file = OutgoingFile::open(&path, &CancellationToken::new())
        .await
        .unwrap();
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        trigger.cancel();
    });

    let started = std::time::Instant::now();
    let error = client
        .drive_upload("notes.txt", &file, TransferOptions::new(&cancel), None)
        .await
        .unwrap_err();
    assert_eq!(error, ClientError::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn a_rejected_upload_keeps_the_servers_reason() {
    let server = Server::start(instant(
        StatusCode::CONFLICT,
        json!({ "error": { "code": "DEVICE_OFFLINE", "message": "gone" } }),
    ))
    .await;
    let client = server.client();
    client.set_bearer(Some(SecretString::new("device-token")));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.txt");
    write_file(&path, b"notes").await;
    let file = OutgoingFile::open(&path, &CancellationToken::new())
        .await
        .unwrap();

    let error = client
        .send_to_device(
            "d2",
            &file,
            TransferOptions::new(&CancellationToken::new()),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(error, http(409, "DEVICE_OFFLINE"));
}

// ---- 下载 ----

#[tokio::test]
async fn downloads_stream_their_bytes_and_report_progress() {
    let payload = pattern(256 * 1024 + 17);
    let served = payload.clone();
    let server = Server::start(with(move |_: Captured| {
        (
            StatusCode::OK,
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/octet-stream"),
            )],
            served.clone(),
        )
            .into_response()
    }))
    .await;
    let client = server.client();
    client.set_bearer(Some(SecretString::new("device-token")));
    let cancel = CancellationToken::new();
    let mut reports = Vec::new();
    let mut report = |progress: TransferProgress| reports.push(progress);
    let download = client
        .drive_content("f1", TransferOptions::new(&cancel), Some(&mut report))
        .await
        .unwrap();
    assert_eq!(download.content_length, Some(payload.len() as u64));
    let received = read_all(download.into_reader()).await.unwrap();
    assert_eq!(received, payload);
    assert_eq!(reports.last().unwrap().transferred, payload.len() as u64);
    assert!(
        reports
            .windows(2)
            .all(|pair| pair[0].transferred <= pair[1].transferred)
    );

    let requests = server.requests(1).await;
    assert_eq!(requests[0].path, "/v1/drive/files/f1/content");
    assert_eq!(
        requests[0].header("authorization"),
        Some("Bearer device-token")
    );
}

#[tokio::test]
async fn a_download_that_stalls_times_out_instead_of_hanging() {
    let server = Server::start(with(|_: Captured| {
        let stream =
            futures_util::stream::iter([Ok::<_, std::io::Error>(Bytes::from_static(b"partial"))])
                .chain(futures_util::stream::pending());
        (
            [(header::CONTENT_TYPE, "application/octet-stream")],
            Body::from_stream(stream),
        )
            .into_response()
    }))
    .await;
    let client = server.client();
    client.set_bearer(Some(SecretString::new("device-token")));
    let cancel = CancellationToken::new();

    let download = client
        .send_content(
            "s1",
            TransferOptions {
                cancel: &cancel,
                stall: Duration::from_millis(300),
            },
            None,
        )
        .await
        .unwrap();
    let outcome =
        tokio::time::timeout(Duration::from_secs(5), read_all(download.into_reader())).await;
    let error = outcome.expect("the read should end by itself").unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
}

#[tokio::test]
async fn a_cancelled_download_stops_early() {
    let server = Server::start(delayed(Duration::from_secs(30), json!({}))).await;
    let client = server.client();
    client.set_bearer(Some(SecretString::new("device-token")));
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        trigger.cancel();
    });

    let error = client
        .send_content("s1", TransferOptions::new(&cancel), None)
        .await
        .unwrap_err();
    assert_eq!(error, ClientError::Cancelled);
}

// ---- 发送记录 ----

#[tokio::test]
async fn sends_are_listed_and_acted_on() {
    let send = |state: &str| {
        json!({
            "id": "s1",
            "direction": "in",
            "peerDeviceId": "d2",
            "peerDeviceName": "工作电脑",
            "name": "合同.pdf",
            "size": 1024,
            "sha256": "a".repeat(64),
            "state": state,
            "createdAt": "2026-11-01T00:00:00.000Z",
            "updatedAt": "2026-11-01T00:00:00.000Z"
        })
    };
    let server = Server::start(with(move |request: Captured| match request.path.as_str() {
        "/v1/sends" => json_response(StatusCode::OK, json!({ "items": [send("offered")] })),
        "/v1/sends/s1/accept" => json_response(StatusCode::OK, send("delivered")),
        "/v1/sends/s1/decline" => json_response(StatusCode::OK, send("declined")),
        "/v1/sends/s1/cancel" => json_response(StatusCode::OK, send("cancelled")),
        "/v1/sends/s1/failed" => {
            let mut value = send("failed");
            value["failure"] = json!("disk_full");
            json_response(StatusCode::OK, value)
        }
        _ => json_response(StatusCode::NOT_FOUND, json!({})),
    }))
    .await;
    let client = server.client();
    client.set_bearer(Some(SecretString::new("device-token")));
    let cancel = CancellationToken::new();

    let listed = client
        .sends(
            &SendQuery {
                direction: Direction::In,
                state: Some(SendState::Offered),
                limit: 50,
            },
            &options(&cancel),
        )
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].peer_device_name, "工作电脑");
    assert!(listed[0].content_available());

    // 不带状态筛选时查询串里没有 state。
    client
        .sends(
            &SendQuery {
                direction: Direction::Out,
                state: None,
                limit: 10,
            },
            &options(&cancel),
        )
        .await
        .unwrap();
    // 页大小越界：本地拦下。
    let error = client
        .sends(
            &SendQuery {
                direction: Direction::Out,
                state: None,
                limit: 0,
            },
            &options(&cancel),
        )
        .await;
    assert_eq!(error.unwrap_err(), ClientError::InvalidRequest);

    assert_eq!(
        client
            .send_accept("s1", &options(&cancel))
            .await
            .unwrap()
            .state,
        SendState::Delivered
    );
    assert_eq!(
        client
            .send_decline("s1", &options(&cancel))
            .await
            .unwrap()
            .state,
        SendState::Declined
    );
    assert_eq!(
        client
            .send_cancel("s1", &options(&cancel))
            .await
            .unwrap()
            .state,
        SendState::Cancelled
    );
    let failed = client
        .send_failed(
            "s1",
            FailInput {
                reason: FailReason::DiskFull,
            },
            &options(&cancel),
        )
        .await
        .unwrap();
    assert_eq!(failed.failure, Some(FailReason::DiskFull));

    let requests = server.requests(6).await;
    let paths: Vec<&str> = requests.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(
        paths,
        vec![
            "/v1/sends",
            "/v1/sends",
            "/v1/sends/s1/accept",
            "/v1/sends/s1/decline",
            "/v1/sends/s1/cancel",
            "/v1/sends/s1/failed"
        ]
    );
    // 参数的先后由 HTTP 库决定，按解析后的内容核对。
    assert_eq!(
        requests[0].params(),
        serde_json::from_value::<serde_json::Map<String, Value>>(json!({
            "direction": "in", "state": "offered", "limit": "50"
        }))
        .unwrap()
    );
    assert_eq!(
        requests[1].params(),
        serde_json::from_value::<serde_json::Map<String, Value>>(json!({
            "direction": "out", "limit": "10"
        }))
        .unwrap()
    );
    assert!(requests[2].body.is_empty(), "actions carry no body");
    assert_eq!(requests[5].json(), json!({ "reason": "disk_full" }));
    assert_eq!(requests[5].header("content-type"), Some("application/json"));
}

// ---- 时限与取消 ----

#[tokio::test]
async fn requests_time_out_and_can_be_cancelled() {
    let server = Server::start(delayed(Duration::from_secs(30), json!({}))).await;
    let client = server.client();
    client.set_bearer(Some(SecretString::new("device-token")));

    let error = client
        .me(&RequestOptions {
            cancel: &CancellationToken::new(),
            timeout: Some(Duration::from_millis(200)),
        })
        .await
        .unwrap_err();
    assert_eq!(error, ClientError::Timeout);
    assert!(error.is_retryable());

    let cancel = CancellationToken::new();
    cancel.cancel();
    let error = client.me(&options(&cancel)).await.unwrap_err();
    assert_eq!(error, ClientError::Cancelled);
}

#[tokio::test]
async fn redirects_are_never_followed() {
    // 换一个端口当"别的来源"，确认客户端不会跟过去。
    let elsewhere = Server::start(instant(StatusCode::OK, me_response())).await;
    let target = format!("{}/v1/me", elsewhere.origin.as_str());
    let server = Server::start(with(move |_: Captured| {
        Response::builder()
            .status(StatusCode::TEMPORARY_REDIRECT)
            .header(header::LOCATION, target.clone())
            .body(Body::empty())
            .unwrap()
    }))
    .await;
    let client = server.client();
    client.set_bearer(Some(SecretString::new("device-token")));

    let error = client
        .me(&options(&CancellationToken::new()))
        .await
        .unwrap_err();
    assert_eq!(error.status(), Some(307));
    assert_eq!(
        elsewhere.received(),
        0,
        "the redirect target was never contacted"
    );
}

// ---- 事件流 ----

const HELLO: &str = "data: {\"type\":\"hello\",\"deviceId\":\"d1\"}\n\n";
const DEVICES: &str = "data: {\"type\":\"devices_changed\",\"devices\":[{\"id\":\"d1\",\"name\":\"Mac\",\"platform\":\"macos\",\"online\":true,\"createdAt\":\"2026-11-01T00:00:00.000Z\"}]}\n\n";

#[tokio::test]
async fn the_event_stream_reconnects_after_the_server_closes() {
    let server = Server::start(event_stream(vec![": ping\n\n", HELLO, DEVICES], false)).await;
    let client = server.client();
    client.set_bearer(Some(SecretString::new("device-token")));
    let cancel = CancellationToken::new();

    let mut stream = client.events_with(cancel.clone(), Duration::from_secs(5));
    let mut kinds = Vec::new();
    for _ in 0..4 {
        let event = tokio::time::timeout(Duration::from_secs(10), stream.next())
            .await
            .expect("the stream should keep going")
            .unwrap()
            .unwrap();
        kinds.push(event.kind());
    }
    drop(stream);
    // 每次连接先 hello 再 devices_changed；读到四条说明至少重连过一次。
    assert_eq!(
        kinds,
        vec!["hello", "devices_changed", "hello", "devices_changed"]
    );

    let requests = server.requests(2).await;
    assert_eq!(requests[0].path, "/v1/events");
    assert_eq!(requests[0].method, Method::GET);
    assert_eq!(
        requests[0].header("authorization"),
        Some("Bearer device-token")
    );
    assert_eq!(requests[0].header("accept"), Some("text/event-stream"));
    assert!(requests[0].params().is_empty());
    cancel.cancel();
}

#[tokio::test]
async fn a_silent_event_stream_is_abandoned_and_retried() {
    let server = Server::start(event_stream(vec![HELLO], true)).await;
    let client = server.client();
    client.set_bearer(Some(SecretString::new("device-token")));
    let cancel = CancellationToken::new();

    let mut stream = client.events_with(cancel.clone(), Duration::from_millis(300));
    let first = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(first.kind(), "hello");
    // 之后服务端不再说话：报一次超时，然后重连。
    let second = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(second, ClientError::Timeout);
    let third = tokio::time::timeout(Duration::from_secs(10), stream.next())
        .await
        .expect("the stream should reconnect")
        .unwrap()
        .unwrap();
    assert_eq!(third.kind(), "hello");
    assert!(server.received() >= 2);
    cancel.cancel();
}

#[tokio::test]
async fn an_expired_session_ends_the_event_stream() {
    let server = Server::start(instant(
        StatusCode::UNAUTHORIZED,
        json!({ "error": { "code": "UNAUTHORIZED", "message": "sign in again" } }),
    ))
    .await;
    let client = server.client();
    client.set_bearer(Some(SecretString::new("device-token")));

    let mut stream = client.events(CancellationToken::new());
    let only = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(only, http(401, "UNAUTHORIZED"));
    // 报告之后流就结束，不会无穷重试。
    let ended = tokio::time::timeout(Duration::from_secs(5), stream.next()).await;
    assert!(matches!(ended, Ok(None)), "{ended:?}");
}

/// 一段发完一条就挂住的流。被丢弃时记一笔，用来观察服务端何时发现对端走了。
struct Watched {
    drops: Arc<AtomicUsize>,
    sent: bool,
    chunk: &'static str,
}

impl futures_core::Stream for Watched {
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        if self.sent {
            return std::task::Poll::Pending;
        }
        self.sent = true;
        std::task::Poll::Ready(Some(Ok(Bytes::from_static(self.chunk.as_bytes()))))
    }
}

impl Drop for Watched {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn dropping_the_stream_closes_the_connection_without_waiting_for_another_event() {
    // 服务端靠"连接还在不在"判断一台设备在不在线。调用方丢掉流之后，
    // 连接必须马上断，而不是等到下一次有事件要写、或者超时才断。
    let drops = Arc::new(AtomicUsize::new(0));
    let watched = Arc::clone(&drops);
    let server = Server::start(with(move |_: Captured| {
        let stream = Watched {
            drops: Arc::clone(&watched),
            sent: false,
            chunk: HELLO,
        };
        (
            [(header::CONTENT_TYPE, "text/event-stream")],
            Body::from_stream(stream),
        )
            .into_response()
    }))
    .await;
    let client = server.client();
    client.set_bearer(Some(SecretString::new("device-token")));

    let mut stream = client.events_with(CancellationToken::new(), Duration::from_secs(30));
    let hello = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(hello.kind(), "hello");
    assert_eq!(drops.load(Ordering::SeqCst), 0);

    drop(stream);
    tokio::time::timeout(Duration::from_secs(3), async {
        while drops.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the server should notice the closed connection right away");

    // 没人在听了，也就不会再重连。
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(server.received(), 1);
}

#[tokio::test]
async fn cancelling_ends_the_event_stream_and_the_connection() {
    let connections = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&connections);
    let server = Server::start(with(move |_: Captured| {
        seen.fetch_add(1, Ordering::SeqCst);
        let stream = futures_util::stream::iter([Ok::<_, std::io::Error>(Bytes::from_static(
            HELLO.as_bytes(),
        ))])
        .chain(futures_util::stream::pending());
        (
            [(header::CONTENT_TYPE, "text/event-stream")],
            Body::from_stream(stream),
        )
            .into_response()
    }))
    .await;
    let client = server.client();
    client.set_bearer(Some(SecretString::new("device-token")));
    let cancel = CancellationToken::new();

    let mut stream = client.events_with(cancel.clone(), Duration::from_secs(30));
    let hello = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(hello.kind(), "hello");

    cancel.cancel();
    let ended = tokio::time::timeout(Duration::from_secs(5), stream.next()).await;
    assert!(matches!(ended, Ok(None)), "{ended:?}");
    // 取消之后不再重连。
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(connections.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_response_that_is_not_an_event_stream_is_rejected() {
    let server = Server::start(instant(StatusCode::OK, json!({ "status": "ok" }))).await;
    let client = server.client();
    client.set_bearer(Some(SecretString::new("device-token")));
    let cancel = CancellationToken::new();

    let mut stream = client.events_with(cancel.clone(), Duration::from_secs(5));
    let first = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(first, ClientError::InvalidResponse);
    cancel.cancel();
}

#[test]
fn progress_fractions_are_bounded() {
    assert_eq!(
        TransferProgress {
            transferred: 25,
            total: 100
        }
        .fraction(),
        0.25
    );
    assert_eq!(
        TransferProgress {
            transferred: 0,
            total: 0
        }
        .fraction(),
        1.0
    );
    assert_eq!(
        TransferProgress {
            transferred: 150,
            total: 100
        }
        .fraction(),
        1.0
    );
    assert_eq!(TRANSFER_STALL, Duration::from_secs(60));
    assert_eq!(DEFAULT_TIMEOUT, Duration::from_secs(15));
}

#[tokio::test]
async fn bounded_download_ranges_validate_real_http_before_releasing_bytes() {
    use assemble_protocol::{drive::DriveFile, primitives::Timestamp};
    let sha = "a".repeat(64);
    let file = DriveFile {
        id: "file-range".into(),
        name: "资料.bin".into(),
        size: 6,
        sha256: sha.clone(),
        mime_type: None,
        created_at: Timestamp::now(),
    };
    for (status, etag, range, body, valid) in [
        (
            StatusCode::PARTIAL_CONTENT,
            format!("\"{sha}\""),
            "bytes 3-5/6",
            "def",
            true,
        ),
        (
            StatusCode::OK,
            format!("\"{sha}\""),
            "bytes 3-5/6",
            "abcdef",
            false,
        ),
        (
            StatusCode::PARTIAL_CONTENT,
            format!("W/\"{sha}\""),
            "bytes 3-5/6",
            "def",
            false,
        ),
        (
            StatusCode::PARTIAL_CONTENT,
            format!("\"{sha}\""),
            "bytes 0-2/6",
            "abc",
            false,
        ),
        (
            StatusCode::PARTIAL_CONTENT,
            format!("\"{sha}\""),
            "bytes 3-5/6",
            "de",
            false,
        ),
        (
            StatusCode::PARTIAL_CONTENT,
            format!("\"{sha}\""),
            "bytes 3-5/6",
            "defg",
            false,
        ),
        (
            StatusCode::RANGE_NOT_SATISFIABLE,
            format!("\"{sha}\""),
            "bytes */6",
            "",
            false,
        ),
    ] {
        let server = Server::start(with(move |_| {
            Response::builder()
                .status(status)
                .header(header::ETAG, &etag)
                .header(header::CONTENT_RANGE, range)
                .body(Body::from(body))
                .unwrap()
        }))
        .await;
        let client = server.client();
        let cancel = CancellationToken::new();
        let answer = client
            .drive_chunk(&file, 3, &RequestOptions::new(&cancel))
            .await;
        if valid {
            assert_eq!(answer.unwrap(), Some(b"def".to_vec()));
        } else {
            assert!(answer.is_err());
        }
        let requests = server.requests(1).await;
        assert_eq!(requests[0].header("range"), Some("bytes=3-5"));
        assert_eq!(
            requests[0].header("if-range"),
            Some(format!("\"{sha}\"").as_str())
        );
    }
}
