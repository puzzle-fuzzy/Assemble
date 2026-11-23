//! 客户端的集成测试：对一个真实的本地 HTTP 服务发请求。
//!
//! 服务端把收到的每个请求原样记下来，再按预设给出应答。这样请求头、路径、
//! 查询串与正文都能被逐项核对。

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::origin::ApiOrigin;

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
}

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// 根据收到的请求给出应答；应答可以拖延。
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

struct Server {
    calls: Arc<Mutex<Vec<Captured>>>,
    origin: ApiOrigin,
    _task: tokio::task::JoinHandle<()>,
}

impl Server {
    async fn start(responder: Responder) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
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
            origin: ApiOrigin::parse(&format!("http://127.0.0.1:{port}")).unwrap(),
            _task: task,
        }
    }

    fn client(&self) -> JarvisClient {
        let deployment = Deployment::new(self.origin.as_str(), "http://127.0.0.1:5474/").unwrap();
        JarvisClient::new(deployment).unwrap()
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

fn json_response(status: StatusCode, body: Value) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::to_string(&body).unwrap(),
    )
        .into_response()
}

fn empty_list() -> Value {
    json!({ "items": [], "total": 0 })
}

/// 一条 `files` 记录。
fn file_record(id: &str) -> Value {
    json!({
        "id": id,
        "resource": "files",
        "version": 1,
        "createdAt": "2026-11-23T00:00:00.000Z",
        "updatedAt": "2026-11-23T00:00:00.000Z",
        "data": {
            "name": "a.txt",
            "size": 3,
            "mimeType": "text/plain",
            "sha256": "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            "cloudAvailable": true
        }
    })
}

/// 一个完整的公开账户。
fn account(id: &str) -> Value {
    json!({
        "id": id,
        "username": "example",
        "displayName": "Example",
        "email": null,
        "role": "user",
        "status": "active",
        "passwordChangedAt": null,
        "createdAt": "2026-11-01T00:00:00.000Z",
        "updatedAt": "2026-11-01T00:00:00.000Z",
        "lastLoginAt": null,
        "lastLoginIp": null,
        "loginCount": 1,
        "disabledAt": null,
        "disabledReason": null
    })
}

fn device_record() -> Value {
    json!({
        "id": "device-1",
        "resource": "devices",
        "version": 2,
        "createdAt": "2026-11-23T00:00:00.000Z",
        "updatedAt": "2026-11-23T00:00:00.000Z",
        "data": { "name": "host", "platform": "darwin", "sharingEnabled": true }
    })
}

fn options<'a>(cancel: &'a CancellationToken) -> RequestOptions<'a> {
    RequestOptions::new(cancel)
}

fn http(status: u16, code: &str) -> ClientError {
    ClientError::Http {
        status,
        remote_code: Some(code.to_owned()),
    }
}

fn exchange_input() -> DesktopExchangeInput {
    DesktopExchangeInput {
        code: "c".repeat(43),
        code_verifier: "v".repeat(43),
        redirect_uri: "http://127.0.0.1:54321/callback".into(),
    }
}

#[tokio::test]
async fn health_is_public_and_carries_no_credentials_or_origin() {
    let server = Server::start(instant(
        StatusCode::OK,
        json!({ "status": "ok", "service": "api", "version": "0.1.0",
                "timestamp": "2026-11-23T12:00:00.000Z", "uptime": 1.5 }),
    ))
    .await;
    let client = server.client();
    client.set_bearer(Some(SecretString::new("device-token")));

    let health = client
        .health(&options(&CancellationToken::new()))
        .await
        .unwrap();
    assert_eq!(health.service, "api");
    assert_eq!(health.uptime, 1.5);

    let seen = server.requests(1).await.remove(0);
    assert_eq!(seen.method, Method::GET);
    assert_eq!(seen.path, "/health");
    assert_eq!(seen.header("authorization"), None);
    assert_eq!(seen.header("origin"), None);
    assert_eq!(seen.header("cookie"), None);
    assert_eq!(seen.header("accept"), Some("application/json"));
}

#[tokio::test]
async fn readiness_reports_an_unavailable_service_instead_of_failing() {
    let server = Server::start(instant(
        StatusCode::SERVICE_UNAVAILABLE,
        json!({ "status": "unavailable" }),
    ))
    .await;
    let client = server.client();

    let ready = client
        .ready(&options(&CancellationToken::new()))
        .await
        .unwrap();
    assert_eq!(
        ready.status,
        assemble_protocol::health::Readiness::Unavailable
    );
}

#[tokio::test]
async fn every_authenticated_write_carries_the_bearer_and_the_client_origin() {
    let server = Server::start(instant(StatusCode::OK, json!({ "item": device_record() }))).await;
    let client = server.client();
    client.set_bearer(Some(SecretString::new("device-token")));

    let record = client
        .heartbeat(
            "device-1",
            &HeartbeatInput::sharing(),
            &options(&CancellationToken::new()),
        )
        .await
        .unwrap();
    assert_eq!(record.resource(), ResourceName::Devices);

    let seen = server.requests(1).await.remove(0);
    assert_eq!(seen.method, Method::POST);
    assert_eq!(seen.path, "/api/jarvis/devices/device-1/heartbeat");
    assert_eq!(seen.header("authorization"), Some("Bearer device-token"));
    assert_eq!(seen.header("origin"), Some("http://127.0.0.1:5474"));
    assert_eq!(seen.header("content-type"), Some("application/json"));
    assert_eq!(seen.json(), json!({ "sharingEnabled": true }));
}

#[tokio::test]
async fn reads_carry_the_bearer_but_no_origin() {
    let server = Server::start(instant(
        StatusCode::OK,
        json!({ "items": [file_record("file-1")], "total": 1 }),
    ))
    .await;
    let client = server.client();
    client.set_bearer(Some(SecretString::new("device-token")));

    let page = client
        .list(
            ResourceName::Files,
            &ListQuery {
                search: Some("报告 & notes".into()),
                ..ListQuery::default()
            },
            &options(&CancellationToken::new()),
        )
        .await
        .unwrap();
    assert_eq!(page.total, 1);

    let seen = server.requests(1).await.remove(0);
    assert_eq!(seen.method, Method::GET);
    assert_eq!(seen.path, "/api/jarvis/resources/files");
    assert_eq!(
        seen.query,
        // 空格编码成 `+`，与原客户端的 URLSearchParams 一致。
        "limit=50&offset=0&search=%E6%8A%A5%E5%91%8A+%26+notes"
    );
    assert_eq!(seen.header("authorization"), Some("Bearer device-token"));
    assert_eq!(seen.header("origin"), None);
}

#[tokio::test]
async fn a_session_cookie_is_stored_and_replayed_only_under_the_api() {
    let account_for = |id: &str| json!({ "account": account(id) });
    let server = Server::start(with(move |seen| match seen.path.as_str() {
        "/api/auth/desktop/exchange" => {
            let mut response = json_response(StatusCode::OK, json!({ "account": account("x") }));
            response.headers_mut().insert(
                header::SET_COOKIE,
                HeaderValue::from_static(
                    "starter_session=fresh-session; Path=/api; HttpOnly; SameSite=Lax",
                ),
            );
            response
        }
        "/api/auth/me" => json_response(
            StatusCode::OK,
            account_for(
                if seen
                    .header("cookie")
                    .is_some_and(|cookie| cookie.contains("fresh-session"))
                {
                    "fresh"
                } else {
                    "other"
                },
            ),
        ),
        _ => json_response(
            StatusCode::NOT_FOUND,
            json!({ "error": { "code": "NOT_FOUND", "message": "x" } }),
        ),
    }))
    .await;
    let client = server.client();
    let cancel = CancellationToken::new();

    // 换码：服务端经 Set-Cookie 交来网页会话。
    client
        .desktop_exchange(&exchange_input(), &options(&cancel))
        .await
        .unwrap();
    assert_eq!(client.session_cookie().unwrap().expose(), "fresh-session");

    // 之后的 API 请求自动带上它。
    let me = client.me(&options(&cancel)).await.unwrap();
    assert_eq!(me.account.id, "fresh");
    let seen = server.requests(2).await;
    assert_eq!(seen[0].header("cookie"), None);
    assert_eq!(
        seen[1].header("cookie"),
        Some("starter_session=fresh-session")
    );

    // 清掉之后不再带它。
    client.clear_session_cookie();
    assert!(client.session_cookie().is_none());
    let me = client.me(&options(&cancel)).await.unwrap();
    assert_eq!(me.account.id, "other");
}

#[tokio::test]
async fn the_exchange_never_carries_an_existing_credential() {
    // 先让服务端种下一个会话 Cookie，再在带着 Bearer 的状态下发起换码。
    let server = Server::start(with(move |seen| {
        if seen.path == "/api/jarvis/resources/reminders" {
            let mut response = json_response(StatusCode::OK, empty_list());
            response.headers_mut().insert(
                header::SET_COOKIE,
                HeaderValue::from_static("starter_session=old-session; Path=/api; HttpOnly"),
            );
            return response;
        }
        json_response(StatusCode::OK, json!({ "ok": true }))
    }))
    .await;
    let client = server.client();
    let cancel = CancellationToken::new();
    client.set_bearer(Some(SecretString::new("device-token")));

    client
        .list(
            ResourceName::Reminders,
            &ListQuery::default(),
            &options(&cancel),
        )
        .await
        .unwrap();
    assert_eq!(client.session_cookie().unwrap().expose(), "old-session");

    client
        .desktop_exchange(&exchange_input(), &options(&cancel))
        .await
        .unwrap();

    let exchange_seen = server.requests(2).await.remove(1);
    assert_eq!(exchange_seen.path, "/api/auth/desktop/exchange");
    assert_eq!(exchange_seen.header("cookie"), None);
    assert_eq!(exchange_seen.header("authorization"), None);
    assert_eq!(
        exchange_seen.header("origin"),
        Some("http://127.0.0.1:5474")
    );
    assert_eq!(
        exchange_seen.json(),
        json!({
            "code": "c".repeat(43),
            "codeVerifier": "v".repeat(43),
            "redirectUri": "http://127.0.0.1:54321/callback"
        })
    );
    // 旧会话确实被清掉了，而不是留着等下次请求带出去。
    assert!(client.session_cookie().is_none());

    // 换码请求的形式不对时根本不发出。
    let malformed = DesktopExchangeInput {
        code: "short".into(),
        ..exchange_input()
    };
    let error = client
        .desktop_exchange(&malformed, &options(&cancel))
        .await
        .unwrap_err();
    assert_eq!(error, ClientError::InvalidRequest);
    assert_eq!(server.received(), 2);
}

#[tokio::test]
async fn http_failures_keep_their_status_and_remote_code() {
    let not_found =
        |code: &str| json!({ "error": { "code": code, "message": "x", "requestId": "r" } });
    let server = Server::start(with(move |seen| match seen.path.as_str() {
        "/api/jarvis/commands/cmd-1" => {
            json_response(StatusCode::NOT_FOUND, not_found("COMMAND_NOT_FOUND"))
        }
        "/api/jarvis/commands/cmd-2" => {
            json_response(StatusCode::NOT_FOUND, not_found("NOT_FOUND"))
        }
        "/api/jarvis/commands/cmd-3" => {
            json_response(StatusCode::CONFLICT, not_found("COMMAND_EXPIRED"))
        }
        "/api/jarvis/commands/cmd-4" => (
            StatusCode::BAD_GATEWAY,
            [(header::CONTENT_TYPE, "text/html")],
            "<html>bad gateway</html>",
        )
            .into_response(),
        _ => (
            StatusCode::UNAUTHORIZED,
            [(header::CONTENT_TYPE, "application/json")],
            "not json",
        )
            .into_response(),
    }))
    .await;
    let client = server.client();
    let cancel = CancellationToken::new();
    let none = WaitSeconds::none();

    let gone = client
        .get_command("cmd-1", none, None, &options(&cancel))
        .await
        .unwrap_err();
    assert_eq!(gone, http(404, "COMMAND_NOT_FOUND"));
    assert!(gone.is_command_not_found());

    let missing = client
        .get_command("cmd-2", none, None, &options(&cancel))
        .await
        .unwrap_err();
    assert_eq!(missing, http(404, "NOT_FOUND"));
    assert!(!missing.is_command_not_found());

    let expired = client
        .get_command("cmd-3", none, None, &options(&cancel))
        .await
        .unwrap_err();
    assert_eq!(expired, http(409, "COMMAND_EXPIRED"));
    assert!(!expired.is_retryable());

    // 没有约定错误结构的应答：保留状态码，没有远程错误码。
    let gateway = client
        .get_command("cmd-4", none, None, &options(&cancel))
        .await
        .unwrap_err();
    assert_eq!(
        gateway,
        ClientError::Http {
            status: 502,
            remote_code: None
        }
    );
    assert!(gateway.is_retryable());

    let unauthorized = client
        .list(
            ResourceName::Files,
            &ListQuery::default(),
            &options(&cancel),
        )
        .await
        .unwrap_err();
    assert!(unauthorized.is_unauthorized());
    assert_eq!(unauthorized.remote_code(), None);
}

#[tokio::test]
async fn redirects_are_never_followed() {
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&hits);
    let server = Server::start(with(move |seen| {
        counter.fetch_add(1, Ordering::SeqCst);
        if seen.path == "/api/jarvis/resources/files" {
            (
                StatusCode::FOUND,
                [(header::LOCATION, "http://127.0.0.1:1/elsewhere")],
                "",
            )
                .into_response()
        } else {
            json_response(StatusCode::OK, empty_list())
        }
    }))
    .await;
    let client = server.client();

    let error = client
        .list(
            ResourceName::Files,
            &ListQuery::default(),
            &options(&CancellationToken::new()),
        )
        .await
        .unwrap_err();
    assert_eq!(error.status(), Some(302));
    // 只有最初那一个请求被发出。
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_successful_response_with_the_wrong_shape_is_rejected() {
    let not_json = Server::start(with(|_| {
        (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            "{ not json",
        )
            .into_response()
    }))
    .await;
    let error = not_json
        .client()
        .list(
            ResourceName::Files,
            &ListQuery::default(),
            &options(&CancellationToken::new()),
        )
        .await
        .unwrap_err();
    assert_eq!(error, ClientError::InvalidResponse);

    let wrong_document = Server::start(instant(
        StatusCode::OK,
        json!({ "items": "not a list", "total": "many" }),
    ))
    .await;
    let error = wrong_document
        .client()
        .list(
            ResourceName::Files,
            &ListQuery::default(),
            &options(&CancellationToken::new()),
        )
        .await
        .unwrap_err();
    assert_eq!(error, ClientError::InvalidResponse);
}

#[tokio::test]
async fn cancelling_stops_an_in_flight_request() {
    let server = Server::start(delayed(Duration::from_secs(30), empty_list())).await;
    let client = server.client();
    let cancel = CancellationToken::new();

    let started = Instant::now();
    let waiter = tokio::spawn({
        let client = client.clone();
        let cancel = cancel.clone();
        async move {
            client
                .list(
                    ResourceName::Files,
                    &ListQuery::default(),
                    &options(&cancel),
                )
                .await
        }
    });
    server.requests(1).await;
    cancel.cancel();

    let error = waiter.await.unwrap().unwrap_err();
    assert_eq!(error, ClientError::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(5));

    // 已取消时的新请求立刻返回。
    let error = client
        .list(
            ResourceName::Files,
            &ListQuery::default(),
            &options(&cancel),
        )
        .await
        .unwrap_err();
    assert_eq!(error, ClientError::Cancelled);
    assert_eq!(server.received(), 1);
}

#[tokio::test]
async fn a_slow_server_hits_the_time_limit() {
    let server = Server::start(delayed(Duration::from_secs(30), empty_list())).await;
    let client = server.client();
    let cancel = CancellationToken::new();

    let started = Instant::now();
    let error = client
        .list(
            ResourceName::Files,
            &ListQuery::default(),
            &RequestOptions {
                timeout: Some(Duration::from_millis(60)),
                cancel: &cancel,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error, ClientError::Timeout);
    assert!(error.is_retryable());
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(server.received(), 1);
}

#[tokio::test]
async fn identifiers_are_checked_before_they_reach_a_path() {
    let server = Server::start(instant(
        StatusCode::OK,
        json!({ "item": file_record("file-1.v2_x") }),
    ))
    .await;
    let client = server.client();
    let cancel = CancellationToken::new();

    for bad in ["", "a/b", "..", "a?b", "a b", "a%2Fb", "a\\b"] {
        let error = client
            .get(ResourceName::Files, bad, &options(&cancel))
            .await
            .unwrap_err();
        assert_eq!(error, ClientError::InvalidRequest, "{bad:?}");
        let error = client
            .heartbeat(bad, &HeartbeatInput::sharing(), &options(&cancel))
            .await
            .unwrap_err();
        assert_eq!(error, ClientError::InvalidRequest, "{bad:?}");
    }
    assert_eq!(server.received(), 0);

    // 一个正常标识的请求则会原样到达。
    client
        .get(ResourceName::Files, "file-1.v2_x", &options(&cancel))
        .await
        .unwrap();
    assert_eq!(
        server.requests(1).await[0].path,
        "/api/jarvis/resources/files/file-1.v2_x"
    );
}

#[tokio::test]
async fn uploads_stream_the_file_with_its_metadata() {
    let server = Server::start(instant(
        StatusCode::OK,
        json!({ "item": file_record("file-1") }),
    ))
    .await;
    let client = server.client();
    let dir = tempfile::tempdir().unwrap();
    let payload: Vec<u8> = (0..300_000u32).map(|index| (index % 251) as u8).collect();
    let path = dir.path().join("笔记.bin");
    std::fs::write(&path, &payload).unwrap();

    let source = LocalSource {
        path: path.clone(),
        size: payload.len() as u64,
        mime_type: Some("application/x-example".into()),
    };
    let metadata = UploadMetadata {
        device_id: Some("device-1".into()),
        root_id: Some("root-1".into()),
        relative_path: Some("docs/笔记.bin".into()),
        source_modified_at: Some("2026-11-23T00:00:00.000Z".into()),
        ..UploadMetadata::default()
    };
    let record = client
        .upload(&source, &metadata, &options(&CancellationToken::new()))
        .await
        .unwrap();
    assert_eq!(record.resource(), ResourceName::Files);

    let seen = server.requests(1).await.remove(0);
    assert_eq!(seen.method, Method::POST);
    assert_eq!(seen.path, "/api/jarvis/files/upload");
    let content_type = seen.header("content-type").unwrap().to_owned();
    assert!(
        content_type.starts_with("multipart/form-data; boundary="),
        "{content_type}"
    );
    let boundary = content_type.split("boundary=").nth(1).unwrap().to_owned();
    let text = String::from_utf8_lossy(&seen.body);
    for expected in [
        format!("--{boundary}"),
        "Content-Disposition: form-data; name=\"file\"; filename=\"笔记.bin\"".to_owned(),
        "Content-Type: application/x-example".to_owned(),
        "name=\"deviceId\"".to_owned(),
        "device-1".to_owned(),
        "name=\"rootId\"".to_owned(),
        "root-1".to_owned(),
        "name=\"relativePath\"".to_owned(),
        "docs/笔记.bin".to_owned(),
        "name=\"sourceModifiedAt\"".to_owned(),
        "2026-11-23T00:00:00.000Z".to_owned(),
    ] {
        assert!(text.contains(&expected), "missing {expected:?}");
    }
    // 没有给出的元数据字段不会以空值出现。
    assert!(!text.contains("name=\"name\""));
    // 文件内容原样到达。
    assert!(
        seen.body
            .windows(payload.len())
            .any(|window| window == payload.as_slice())
    );

    // 超过上限的文件不会发出。
    let oversized = LocalSource {
        size: MAX_FILE_SIZE,
        ..source
    };
    let error = client
        .upload(&oversized, &metadata, &options(&CancellationToken::new()))
        .await
        .unwrap_err();
    assert_eq!(error, ClientError::InvalidRequest);

    // 读不出来的文件是本机错误，不是网络错误。
    let missing = LocalSource {
        path: dir.path().join("absent.bin"),
        size: 3,
        mime_type: None,
    };
    let error = client
        .upload(&missing, &metadata, &options(&CancellationToken::new()))
        .await
        .unwrap_err();
    assert_eq!(error, ClientError::LocalSource);
    assert_eq!(server.received(), 1);
}

#[tokio::test]
async fn downloads_stream_the_bytes_and_report_the_declared_length() {
    let payload: Vec<u8> = (0..150_000u32).map(|index| (index % 7) as u8).collect();
    let bytes = Bytes::copy_from_slice(&payload);
    let server = Server::start(with(move |seen| {
        assert_eq!(seen.path, "/api/jarvis/files/file-1/content");
        let length = bytes.len().to_string();
        (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "application/octet-stream".to_owned()),
                (header::CONTENT_LENGTH, length),
            ],
            bytes.clone(),
        )
            .into_response()
    }))
    .await;
    let client = server.client();

    let download = client
        .download("file-1", &options(&CancellationToken::new()))
        .await
        .unwrap();
    assert_eq!(download.content_length, Some(payload.len() as u64));
    let mut reader = download.into_reader();
    let mut received = Vec::new();
    reader.read_to_end(&mut received).await.unwrap();
    assert_eq!(received, payload);
    assert_eq!(
        server.requests(1).await[0].header("accept"),
        Some("application/octet-stream")
    );
}

#[tokio::test]
async fn listing_everything_follows_the_pages() {
    let page = |first: usize, count: usize, total: usize| {
        json!({
            "items": (0..count)
                .map(|index| file_record(&format!("file-{}", first + index)))
                .collect::<Vec<_>>(),
            "total": total
        })
    };
    let server = Server::start(with(move |seen| {
        let offset: usize = seen
            .query
            .split('&')
            .find_map(|pair| pair.strip_prefix("offset="))
            .unwrap()
            .parse()
            .unwrap();
        match offset {
            0 => json_response(StatusCode::OK, page(0, 100, 150)),
            100 => json_response(StatusCode::OK, page(100, 50, 150)),
            _ => json_response(StatusCode::OK, page(0, 0, 150)),
        }
    }))
    .await;
    let client = server.client();

    let all = client
        .list_all(ResourceName::Files, &options(&CancellationToken::new()))
        .await
        .unwrap();
    assert_eq!(all.len(), 150);
    assert_eq!(all[0].id, "file-0");
    assert_eq!(all[149].id, "file-149");
    assert_eq!(server.received(), 2);
    let queries: Vec<String> = server
        .requests(2)
        .await
        .into_iter()
        .map(|seen| seen.query)
        .collect();
    assert_eq!(queries, vec!["limit=100&offset=0", "limit=100&offset=100"]);
}

#[tokio::test]
async fn writes_send_exactly_the_document_the_server_expects() {
    let not_found = json!({ "error": { "code": "NOT_FOUND", "message": "x" } });
    let server = Server::start(with(move |seen| match seen.path.as_str() {
        "/api/auth/device-session" => json_response(
            StatusCode::CREATED,
            json!({ "account": account("u1"),
                    "session": { "mode": "bearer", "expiresAt": "2026-11-30T00:00:00.000Z", "token": "device-token" } }),
        ),
        "/api/jarvis/resources/devices" => {
            json_response(StatusCode::CREATED, json!({ "item": device_record() }))
        }
        "/api/jarvis/resources/files/file-1" => {
            json_response(StatusCode::OK, json!({ "item": file_record("file-1"), "ok": true }))
        }
        _ => json_response(StatusCode::NOT_FOUND, not_found.clone()),
    }))
    .await;
    let client = server.client();
    let cancel = CancellationToken::new();

    let data = assemble_protocol::resources::DeviceData {
        name: "host".into(),
        platform: assemble_protocol::primitives::Platform::Darwin,
        sharing_enabled: true,
        last_seen_at: None,
        online: None,
    };
    client
        .create(ResourceName::Devices, &data, &options(&cancel))
        .await
        .unwrap();
    client
        .update(
            ResourceName::Files,
            "file-1",
            &UpdateInput {
                version: 3,
                data: serde_json::from_value(json!({ "name": "b.txt" })).unwrap(),
                unset: None,
            },
            &options(&cancel),
        )
        .await
        .unwrap();
    client
        .remove(ResourceName::Files, "file-1", 4, &options(&cancel))
        .await
        .unwrap();
    client.device_session(&options(&cancel)).await.unwrap();

    let seen = server.requests(4).await;
    assert_eq!(seen[0].method, Method::POST);
    assert_eq!(seen[0].path, "/api/jarvis/resources/devices");
    // 创建的请求体就是数据本身，不包一层。
    assert_eq!(
        seen[0].json(),
        json!({ "name": "host", "platform": "darwin", "sharingEnabled": true })
    );
    assert_eq!(seen[1].method, Method::PATCH);
    assert_eq!(seen[1].path, "/api/jarvis/resources/files/file-1");
    assert_eq!(
        seen[1].json(),
        json!({ "version": 3, "data": { "name": "b.txt" } })
    );
    assert_eq!(seen[2].method, Method::DELETE);
    assert_eq!(seen[2].json(), json!({ "version": 4 }));
    assert_eq!(seen[3].path, "/api/auth/device-session");
    assert_eq!(seen[3].json(), json!({}));
    assert_eq!(seen[3].header("content-type"), Some("application/json"));

    // 没有任何改动的更新不会被发出。
    let error = client
        .update(
            ResourceName::Files,
            "file-1",
            &UpdateInput {
                version: 3,
                data: serde_json::Map::new(),
                unset: None,
            },
            &options(&cancel),
        )
        .await
        .unwrap_err();
    assert_eq!(error, ClientError::InvalidRequest);
    assert_eq!(server.received(), 4);
}

#[tokio::test]
async fn command_polling_uses_the_longer_budget_and_its_parameters() {
    let server = Server::start(with(|seen| {
        if seen.path == "/api/jarvis/commands/cmd-9" {
            return json_response(
                StatusCode::NOT_FOUND,
                json!({ "error": { "code": "COMMAND_NOT_FOUND", "message": "gone", "requestId": "r" } }),
            );
        }
        json_response(StatusCode::OK, json!({ "items": [] }))
    }))
    .await;
    let client = server.client();
    let cancel = CancellationToken::new();

    let commands = client
        .commands("device-1", WaitSeconds::new(20).unwrap(), &options(&cancel))
        .await
        .unwrap();
    assert!(commands.is_empty());
    let seen = server.requests(1).await.remove(0);
    assert_eq!(seen.path, "/api/jarvis/devices/device-1/commands");
    assert_eq!(seen.query, "waitSeconds=20");

    // 观察单条命令时可以带上设备编号，把范围限定在本机。
    let error = client
        .get_command(
            "cmd-9",
            WaitSeconds::none(),
            Some("device-1"),
            &options(&cancel),
        )
        .await
        .unwrap_err();
    assert_eq!(error, http(404, "COMMAND_NOT_FOUND"));
    let seen = server.requests(2).await.remove(1);
    assert_eq!(seen.path, "/api/jarvis/commands/cmd-9");
    assert_eq!(seen.query, "waitSeconds=0&deviceId=device-1");

    let none = RequestOptions::new(&cancel);
    assert_eq!(none.budget(false), DEFAULT_TIMEOUT);
    assert_eq!(none.budget(true), COMMAND_WAIT_TIMEOUT);
    let pinned = RequestOptions {
        timeout: Some(Duration::from_secs(3)),
        cancel: &cancel,
    };
    assert_eq!(pinned.budget(true), Duration::from_secs(3));
}

#[tokio::test]
async fn an_unreachable_service_is_a_retryable_failure() {
    // 丢弃端口：连接几乎总是立刻被拒绝，最坏也是超时。
    let origin = ApiOrigin::parse("http://127.0.0.1:9").unwrap();
    let deployment = Deployment::new(origin.as_str(), "http://127.0.0.1:5474/").unwrap();
    let client = JarvisClient::new(deployment).unwrap();

    let error = client
        .health(&options(&CancellationToken::new()))
        .await
        .unwrap_err();
    assert!(
        matches!(error, ClientError::Network | ClientError::Timeout),
        "{error}"
    );
    assert!(error.is_retryable());
}

#[tokio::test]
async fn a_shared_clone_sees_the_same_session() {
    let server = Server::start(instant(StatusCode::OK, empty_list())).await;
    let client = server.client();
    let clone = client.clone();

    client.set_bearer(Some(SecretString::new("device-token")));
    assert!(clone.has_bearer());
    clone
        .list(
            ResourceName::Files,
            &ListQuery::default(),
            &options(&CancellationToken::new()),
        )
        .await
        .unwrap();
    assert_eq!(
        server.requests(1).await[0].header("authorization"),
        Some("Bearer device-token")
    );

    client.set_bearer(None);
    assert!(!clone.has_bearer());
}
