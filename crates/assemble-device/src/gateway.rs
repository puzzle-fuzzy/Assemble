//! 本机网关：MCP 服务器经回环端口调用桌面端。
//!
//! 协议必须与 Electron 版完全一致，否则现有的 `apps/mcp` 会全部失效：
//!
//! - 监听 `127.0.0.1` 的随机端口，每次启动生成新的随机令牌；
//! - `GET /status`、`GET /capabilities`、`POST /rpc`，一律返回 `{ result }` 或 `{ error }`；
//! - 只接受带着正确令牌、主机头精确匹配、且不带 `Origin` 的请求，也就是
//!   只接受本机的原生客户端——浏览器页面发不出这样的请求；
//! - 发现文件 `bridge.json` 写入 `{ url, token }`，仅本人可读写，停止时删除。

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::{Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use subtle::ConstantTimeEq;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use assemble_protocol::gateway::{
    GatewayEnvelope, GatewayError, LocalRpcRequest, MAX_GATEWAY_BODY_BYTES, operation_readonly,
};
use assemble_store::json::{StoreError, write_private_json};

/// 网关把操作交给运行时处理。
pub trait GatewayBackend: Send + Sync + 'static {
    /// 运行时是否已经准备好；没有准备好的请求得到 503。
    fn ready(&self) -> bool {
        true
    }

    /// `GET /status` 的内容。
    fn status(&self) -> Value;

    /// 执行一个 RPC 操作。
    ///
    /// 用返回 `impl Future` 的写法而不是 `async fn`，是为了要求这个 future
    /// 可以在线程间转移：网关的处理函数因此能在多线程运行时上工作。
    /// 实现方照常写成 `async fn` 即可。
    fn invoke(
        &self,
        operation: &str,
        input: Map<String, Value>,
    ) -> impl Future<Output = Result<Value, GatewayError>> + Send;
}

#[derive(Debug, thiserror::Error)]
pub enum GatewayStartError {
    #[error("the local gateway could not listen on loopback: {0}")]
    Bind(std::io::Error),
    #[error("a random bridge token could not be generated")]
    RandomUnavailable,
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// 发现文件的内容：MCP 服务器据此找到网关并完成认证。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BridgeFile {
    pub url: String,
    pub token: String,
}

/// 停止网关时等待在途请求结束的时间。
const DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

/// 一次运行的网关句柄。
pub struct Gateway {
    /// 提供给本机客户端的地址，形如 `http://127.0.0.1:54321`。
    pub url: String,
    bridge_path: PathBuf,
    shutdown: CancellationToken,
    task: Option<JoinHandle<()>>,
}

impl Gateway {
    /// 启动网关并写入发现文件。令牌只进发现文件，不经过任何返回值。
    pub async fn start<B: GatewayBackend>(
        bridge_path: &Path,
        backend: Arc<B>,
    ) -> Result<Self, GatewayStartError> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(GatewayStartError::Bind)?;
        let port = listener
            .local_addr()
            .map_err(GatewayStartError::Bind)?
            .port();
        let host = format!("127.0.0.1:{port}");
        let url = format!("http://{host}");

        let token = random_token()?;
        write_private_json(
            bridge_path,
            &BridgeFile {
                url: url.clone(),
                token: token.clone(),
            },
        )?;

        let state = Arc::new(GatewayState::<B> {
            backend,
            expected_authorization: format!("Bearer {token}").into_bytes(),
            host,
        });
        let shutdown = CancellationToken::new();
        let serve_until = shutdown.clone();
        let router = Router::new().fallback(respond::<B>).with_state(state);
        let task = tokio::spawn(async move {
            let server =
                axum::serve(listener, router).with_graceful_shutdown(serve_until.cancelled_owned());
            if let Err(error) = server.await {
                tracing::debug!(%error, "local gateway stopped serving");
            }
        });

        Ok(Self {
            url,
            bridge_path: bridge_path.to_owned(),
            shutdown,
            task: Some(task),
        })
    }

    /// 停止网关并删除发现文件。在途请求最多等待片刻，之后直接断开。
    pub async fn stop(mut self) -> Result<(), std::io::Error> {
        self.shutdown.cancel();
        if let Some(task) = self.task.take() {
            let abort = task.abort_handle();
            if tokio::time::timeout(DRAIN_TIMEOUT, task).await.is_err() {
                abort.abort();
            }
        }
        match std::fs::remove_file(&self.bridge_path) {
            // 发现文件已经不在，同样算停止成功。
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }
}

impl Gateway {
    /// 立即停止并删除发现文件，不等待在途请求。
    ///
    /// 应用退出时用它。那时没有机会再等异步收尾；而把一个指向已关闭端口的
    /// 发现文件留在磁盘上，会让本机工具去连一个不存在的地址。
    pub fn abort(&mut self) {
        self.shutdown.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
        let _ = std::fs::remove_file(&self.bridge_path);
    }
}

// 句柄被丢弃时不再继续监听，并尽力清掉发现文件。
//
// 应用退出不能指望这里：框架托管的状态在进程结束前未必会被释放。
// 实际运行时验证过——正常退出后文件仍在。退出时要显式调用 [`Gateway::abort`]。
impl Drop for Gateway {
    fn drop(&mut self) {
        self.abort();
    }
}

struct GatewayState<B: GatewayBackend> {
    backend: Arc<B>,
    expected_authorization: Vec<u8>,
    host: String,
}

fn random_token() -> Result<String, GatewayStartError> {
    let mut random = [0u8; 32];
    getrandom::fill(&mut random).map_err(|_| GatewayStartError::RandomUnavailable)?;
    Ok(assemble_protocol::primitives::to_hex(&random))
}

async fn respond<B: GatewayBackend>(
    State(state): State<Arc<GatewayState<B>>>,
    request: Request<Body>,
) -> Response {
    let mut status = StatusCode::OK;
    let outcome = route(&state, request, &mut status).await;
    let envelope = match outcome {
        Ok(result) => GatewayEnvelope::<Value>::Ok { result },
        Err(error) => GatewayEnvelope::<Value>::Err { error },
    };
    (
        status,
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        Json(envelope),
    )
        .into_response()
}

async fn route<B: GatewayBackend>(
    state: &GatewayState<B>,
    request: Request<Body>,
    status: &mut StatusCode,
) -> Result<Value, GatewayError> {
    let (parts, body) = request.into_parts();

    // 1. 令牌必须逐字节一致；长度先比，内容用恒定时间比较。
    let given: &[u8] = parts
        .headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map_or(&[], |value| value.as_bytes());
    let authorized = given.len() == state.expected_authorization.len()
        && bool::from(given.ct_eq(state.expected_authorization.as_slice()));
    if !authorized {
        *status = StatusCode::UNAUTHORIZED;
        return Err(error(
            "UNAUTHORIZED",
            "Local bridge authentication required",
        ));
    }

    // 2. 只接受本机原生客户端：主机头精确匹配，且不带浏览器才会发的 Origin。
    let host_matches = parts
        .headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        == Some(state.host.as_str());
    if !host_matches || parts.headers.contains_key(header::ORIGIN) {
        *status = StatusCode::FORBIDDEN;
        return Err(error(
            "FORBIDDEN_ORIGIN",
            "Only local native clients may use this bridge",
        ));
    }

    // 3. 运行时没准备好时不执行任何操作。
    if !state.backend.ready() {
        *status = StatusCode::SERVICE_UNAVAILABLE;
        return Err(error("NOT_READY", "Device runtime is not ready"));
    }

    // 4. 路由。路径必须精确匹配，带查询串的不算。
    let plain = parts.uri.query().is_none();
    let path = parts.uri.path();
    match (&parts.method, path, plain) {
        (&Method::GET, "/status", true) => Ok(state.backend.status()),
        // 提供哪些操作由后端决定。网关并不知道这个构建实现了什么，
        // 不能替它把契约里的全部操作都报出去。
        (&Method::GET, "/capabilities", true) => {
            dispatch(state, status, "capabilities", Map::new()).await
        }
        (&Method::POST, "/rpc", true) => {
            // 读正文失败只可能是超过了大小上限。
            let Ok(bytes) = to_bytes(body, MAX_GATEWAY_BODY_BYTES).await else {
                *status = StatusCode::PAYLOAD_TOO_LARGE;
                return Err(error("BODY_TOO_LARGE", "Maximum request body is 1 MiB"));
            };
            let request = match parse_rpc(&bytes) {
                Ok(request) => request,
                Err((code, message)) => {
                    *status = StatusCode::BAD_REQUEST;
                    return Err(error(code, message));
                }
            };
            dispatch(state, status, &request.operation, request.input).await
        }
        _ => {
            *status = StatusCode::NOT_FOUND;
            Err(error("NOT_FOUND", "Unknown bridge route"))
        }
    }
}

/// 把操作交给后端；后端拒绝时用 400 带回它的错误。
async fn dispatch<B: GatewayBackend>(
    state: &GatewayState<B>,
    status: &mut StatusCode,
    operation: &str,
    input: Map<String, Value>,
) -> Result<Value, GatewayError> {
    match state.backend.invoke(operation, input).await {
        Ok(result) => Ok(result),
        Err(failure) => {
            *status = StatusCode::BAD_REQUEST;
            Err(failure)
        }
    }
}

/// 解析一次 RPC：先看是不是 JSON，再看形状，最后看操作名是否在目录里。
fn parse_rpc(bytes: &[u8]) -> Result<LocalRpcRequest, (&'static str, &'static str)> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|_| ("INVALID_JSON", "Expected a JSON request"))?;
    let request: LocalRpcRequest = serde_json::from_value(value)
        .map_err(|_| ("INVALID_REQUEST", "Unknown operation or invalid input"))?;
    if operation_readonly(&request.operation).is_none() {
        return Err(("INVALID_REQUEST", "Unknown operation or invalid input"));
    }
    Ok(request)
}

fn error(code: &str, message: &str) -> GatewayError {
    GatewayError {
        code: code.to_owned(),
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    use serde_json::json;

    use assemble_protocol::gateway::LocalCapability;

    use super::*;

    #[derive(Debug)]
    struct RecordingBackend {
        /// 默认就绪；个别测试把它改成未就绪。
        ready: AtomicBool,
        calls: Mutex<Vec<(String, Value)>>,
        fail_with: Mutex<Option<GatewayError>>,
        /// 对外公布哪些操作；为空时公布契约里的全部操作。
        advertise: Mutex<Vec<(&'static str, bool)>>,
    }

    impl Default for RecordingBackend {
        fn default() -> Self {
            Self {
                ready: AtomicBool::new(true),
                calls: Mutex::new(Vec::new()),
                fail_with: Mutex::new(None),
                advertise: Mutex::new(Vec::new()),
            }
        }
    }

    impl RecordingBackend {
        fn set_ready(&self, ready: bool) {
            self.ready.store(ready, Ordering::SeqCst);
        }
    }

    impl GatewayBackend for RecordingBackend {
        fn ready(&self) -> bool {
            self.ready.load(Ordering::SeqCst)
        }

        fn status(&self) -> Value {
            json!({ "connected": false, "deviceName": "test-host" })
        }

        async fn invoke(
            &self,
            operation: &str,
            input: Map<String, Value>,
        ) -> Result<Value, GatewayError> {
            self.calls
                .lock()
                .unwrap()
                .push((operation.to_owned(), Value::Object(input.clone())));
            if let Some(failure) = self.fail_with.lock().unwrap().clone() {
                return Err(failure);
            }
            if operation == "capabilities" {
                let advertised = self.advertise.lock().unwrap().clone();
                let listed: Vec<LocalCapability> = if advertised.is_empty() {
                    assemble_protocol::gateway::capabilities()
                } else {
                    advertised
                        .into_iter()
                        .map(|(operation, readonly)| LocalCapability {
                            operation: operation.to_owned(),
                            readonly,
                        })
                        .collect()
                };
                return Ok(json!(listed));
            }
            Ok(json!({ "operation": operation, "input": input }))
        }
    }

    /// 一个就绪的测试后端。
    fn backend() -> Arc<RecordingBackend> {
        Arc::new(RecordingBackend::default())
    }

    /// 用原始 TCP 发一个请求：这样主机头、Origin 这些细节才能被精确控制。
    async fn exchange(url: &str, raw: String) -> (u16, Vec<(String, String)>, Value) {
        let mut stream = TcpStream::connect(url.trim_start_matches("http://"))
            .await
            .unwrap();
        stream.write_all(raw.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();
        let text = String::from_utf8_lossy(&response).into_owned();
        let (head, body) = text.split_once("\r\n\r\n").expect("headers end");
        let mut lines = head.lines();
        let status: u16 = lines
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        let headers = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_owned()))
            .collect();
        let body = serde_json::from_str(body).unwrap_or(Value::Null);
        (status, headers, body)
    }

    fn request(
        method: &str,
        target: &str,
        host: &str,
        headers: &[(&str, &str)],
        body: &str,
    ) -> String {
        let mut raw =
            format!("{method} {target} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
        for (name, value) in headers {
            raw.push_str(&format!("{name}: {value}\r\n"));
        }
        raw.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
        raw
    }

    struct Running {
        gateway: Option<Gateway>,
        bridge_path: PathBuf,
        token: String,
        _dir: tempfile::TempDir,
    }

    impl Running {
        async fn start(backend: Arc<RecordingBackend>) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let bridge_path = dir.path().join("state/bridge.json");
            let gateway = Gateway::start(&bridge_path, backend).await.unwrap();
            let token = read_bridge(&bridge_path).await.token;
            Self {
                gateway: Some(gateway),
                bridge_path,
                token,
                _dir: dir,
            }
        }

        fn url(&self) -> &str {
            self.gateway.as_ref().unwrap().url.as_str()
        }

        fn host(&self) -> &str {
            self.url().trim_start_matches("http://")
        }

        fn auth(&self) -> String {
            format!("Bearer {}", self.token)
        }

        /// 发一个带正确令牌、主机头也正确的请求。
        async fn send(&self, method: &str, target: &str, body: &str) -> (u16, Value) {
            let auth = self.auth();
            self.send_with(
                method,
                target,
                self.host(),
                &[("Authorization", auth.as_str())],
                body,
            )
            .await
        }

        /// 发一个自定义主机头与请求头的请求。
        async fn send_with(
            &self,
            method: &str,
            target: &str,
            host: &str,
            headers: &[(&str, &str)],
            body: &str,
        ) -> (u16, Value) {
            let (status, _, body) =
                exchange(self.url(), request(method, target, host, headers, body)).await;
            (status, body)
        }

        async fn stop(&mut self) {
            self.gateway.take().unwrap().stop().await.unwrap();
        }
    }

    async fn read_bridge(path: &Path) -> BridgeFile {
        assemble_store::json::read_json(path)
            .unwrap()
            .expect("bridge file")
    }

    fn is_error(body: &Value, code: &str, message: &str) -> bool {
        body["error"] == json!({ "code": code, "message": message })
    }

    const UNAUTHORIZED: (&str, &str) = ("UNAUTHORIZED", "Local bridge authentication required");
    const FORBIDDEN: (&str, &str) = (
        "FORBIDDEN_ORIGIN",
        "Only local native clients may use this bridge",
    );
    const NOT_READY: (&str, &str) = ("NOT_READY", "Device runtime is not ready");
    const NOT_FOUND: (&str, &str) = ("NOT_FOUND", "Unknown bridge route");
    const BAD_JSON: (&str, &str) = ("INVALID_JSON", "Expected a JSON request");
    const BAD_REQUEST: (&str, &str) = ("INVALID_REQUEST", "Unknown operation or invalid input");
    const TOO_LARGE: (&str, &str) = ("BODY_TOO_LARGE", "Maximum request body is 1 MiB");

    #[tokio::test]
    async fn starting_writes_a_private_bridge_file_with_a_fresh_token() {
        let mut running = Running::start(backend()).await;
        let first = read_bridge(&running.bridge_path).await;
        assert_eq!(first.url, running.url());
        assert_eq!(first.token.len(), 64);
        assert!(
            first
                .token
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')),
            "{}",
            first.token
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mode = std::fs::metadata(&running.bridge_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }

        // 每次启动的令牌都不同，旧令牌不能沿用。
        running.stop().await;
        let mut second = Running::start(backend()).await;
        assert_ne!(read_bridge(&second.bridge_path).await.token, first.token);
        second.stop().await;
    }

    #[tokio::test]
    async fn every_route_requires_the_exact_token() {
        let recorder = backend();
        let running = Running::start(Arc::clone(&recorder)).await;
        let lowercased = running.auth().to_ascii_lowercase();
        let too_long = format!("{}0", running.token);
        let wrong = [
            "".to_owned(),
            "Bearer ".to_owned(),
            "Bearer 0000".to_owned(),
            lowercased,
            format!("Bearer {too_long}"),
            format!("Bearer {}", "0".repeat(64)),
        ];

        for (method, target, body) in [
            ("GET", "/status", ""),
            ("GET", "/capabilities", ""),
            ("POST", "/rpc", r#"{"operation":"status","input":{}}"#),
        ] {
            for value in &wrong {
                let headers: Vec<(&str, &str)> = if value.is_empty() {
                    Vec::new()
                } else {
                    vec![("Authorization", value.as_str())]
                };
                let (status, actual) = running
                    .send_with(method, target, running.host(), &headers, body)
                    .await;
                assert_eq!(status, 401, "{method} {target} with {value:?}: {actual}");
                assert!(
                    is_error(&actual, UNAUTHORIZED.0, UNAUTHORIZED.1),
                    "{actual}"
                );
            }
            // 认证失败发生在任何处理之前；带对令牌才通过。
            let (status, _) = running.send(method, target, body).await;
            assert_eq!(status, 200, "{method} {target}");
        }
        // 整轮里只有带对令牌的 capabilities 与 rpc 两次到达了后端；status 不经过后端。
        let operations: Vec<String> = recorder
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|(operation, _)| operation.clone())
            .collect();
        assert_eq!(operations, vec!["capabilities", "status"]);
    }

    #[tokio::test]
    async fn only_requests_without_a_browser_origin_and_with_the_exact_host_pass() {
        let recorder = backend();
        let running = Running::start(Arc::clone(&recorder)).await;
        let auth = running.auth();

        let (status, body) = running
            .send_with(
                "GET",
                "/status",
                "evil.example:80",
                &[("Authorization", auth.as_str())],
                "",
            )
            .await;
        assert_eq!(status, 403);
        assert!(is_error(&body, FORBIDDEN.0, FORBIDDEN.1), "{body}");

        // 外部页面的 Origin，以及看似同源的 Origin，一律拒绝：网关只服务原生客户端。
        for origin in ["https://evil.example", "http://127.0.0.1:1", running.url()] {
            let (status, body) = running
                .send_with(
                    "GET",
                    "/status",
                    running.host(),
                    &[("Authorization", auth.as_str()), ("Origin", origin)],
                    "",
                )
                .await;
            assert_eq!(status, 403, "Origin {origin}");
            assert!(is_error(&body, FORBIDDEN.0, FORBIDDEN.1), "{body}");
        }
        // 即使 Origin 值是空串，也说明请求来自浏览器环境。
        let (status, _) = running
            .send_with(
                "GET",
                "/status",
                running.host(),
                &[("Authorization", auth.as_str()), ("Origin", "")],
                "",
            )
            .await;
        assert_eq!(status, 403);
        assert!(recorder.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn status_and_capabilities_are_read_only_projections() {
        let running = Running::start(backend()).await;

        let (status, body) = running.send("GET", "/status", "").await;
        assert_eq!(status, 200);
        assert_eq!(
            body,
            json!({ "result": { "connected": false, "deviceName": "test-host" } })
        );

        let (status, body) = running.send("GET", "/capabilities", "").await;
        assert_eq!(status, 200);
        let listed = body["result"].as_array().expect("capability list");
        // 这份完整的目录来自后端（测试替身照契约回答），不是网关写死的。
        assert_eq!(listed.len(), assemble_protocol::gateway::OPERATIONS.len());
        assert_eq!(
            listed[0],
            json!({ "operation": "planning.capabilities", "readonly": true })
        );
        assert_eq!(
            listed[listed.len() - 1],
            json!({ "operation": "cloud.request", "readonly": false })
        );
    }

    #[tokio::test]
    async fn the_backend_decides_which_operations_are_advertised() {
        // 一个只实现了三个只读操作的后端：对外只能报这三个。
        let recorder = backend();
        *recorder.advertise.lock().unwrap() = vec![
            ("capabilities", true),
            ("status", true),
            ("config.get", true),
        ];
        let running = Running::start(Arc::clone(&recorder)).await;

        let (status, body) = running.send("GET", "/capabilities", "").await;
        assert_eq!(status, 200);
        let expected = json!([
            { "operation": "capabilities", "readonly": true },
            { "operation": "status", "readonly": true },
            { "operation": "config.get", "readonly": true },
        ]);
        assert_eq!(body, json!({ "result": expected }));

        // 经 RPC 询问得到的是同一份。
        let (status, body) = running
            .send("POST", "/rpc", r#"{"operation":"capabilities","input":{}}"#)
            .await;
        assert_eq!(status, 200);
        assert_eq!(body, json!({ "result": expected }));

        // 后端答不了的时候如实带回 400，不退回到契约里的全部操作。
        *recorder.fail_with.lock().unwrap() = Some(error("NOT_READY", "Not ready"));
        let (status, body) = running.send("GET", "/capabilities", "").await;
        assert_eq!(status, 400);
        assert!(is_error(&body, "NOT_READY", "Not ready"), "{body}");
    }

    #[tokio::test]
    async fn rpc_passes_operation_and_input_to_the_backend() {
        let recorder = backend();
        let running = Running::start(Arc::clone(&recorder)).await;

        let (status, body) = running
            .send(
                "POST",
                "/rpc",
                r#"{"operation":"files.resolve","input":{"name":"a.txt","unused":[1,2]}}"#,
            )
            .await;
        assert_eq!(status, 200);
        assert_eq!(
            body,
            json!({ "result": { "operation": "files.resolve", "input": { "name": "a.txt", "unused": [1, 2] } } })
        );
        let calls = recorder.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "files.resolve");
        assert_eq!(calls[0].1, json!({ "name": "a.txt", "unused": [1, 2] }));
    }

    #[tokio::test]
    async fn a_backend_failure_is_returned_verbatim_with_status_400() {
        let recorder = backend();
        *recorder.fail_with.lock().unwrap() = Some(error(
            "ROOT_NOT_AUTHORIZED",
            "Choose an enabled shared directory",
        ));
        let running = Running::start(Arc::clone(&recorder)).await;

        let (status, body) = running
            .send("POST", "/rpc", r#"{"operation":"files.upload","input":{}}"#)
            .await;
        assert_eq!(status, 400);
        assert_eq!(
            body,
            json!({ "error": { "code": "ROOT_NOT_AUTHORIZED", "message": "Choose an enabled shared directory" } })
        );
    }

    #[tokio::test]
    async fn malformed_requests_are_rejected_before_reaching_the_backend() {
        let recorder = backend();
        let running = Running::start(Arc::clone(&recorder)).await;

        let cases = [
            ("{ not json", 400, BAD_JSON),
            ("", 400, BAD_JSON),
            (r#"{"operation":42}"#, 400, BAD_REQUEST),
            (
                r#"{"operation":"device.reboot","input":{}}"#,
                400,
                BAD_REQUEST,
            ),
            (r#"{"operation":"status","input":[]}"#, 400, BAD_REQUEST),
            (r#"["status"]"#, 400, BAD_REQUEST),
        ];
        for (body, status, (code, message)) in cases {
            let (actual_status, actual) = running.send("POST", "/rpc", body).await;
            assert_eq!(actual_status, status, "{body:?}");
            assert!(is_error(&actual, code, message), "{body:?} -> {actual}");
        }
        assert!(recorder.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_body_at_the_limit_is_accepted_and_one_byte_more_is_refused() {
        let recorder = backend();
        let running = Running::start(Arc::clone(&recorder)).await;

        // 正文恰好 1 MiB：允许。
        let envelope_overhead = r#"{"operation":"status","input":{"pad":""}}"#.len();
        let at_limit = format!(
            r#"{{"operation":"status","input":{{"pad":"{}"}}}}"#,
            "x".repeat(MAX_GATEWAY_BODY_BYTES - envelope_overhead)
        );
        assert_eq!(at_limit.len(), MAX_GATEWAY_BODY_BYTES);
        let (status, body) = running.send("POST", "/rpc", &at_limit).await;
        assert_eq!(status, 200, "{body}");

        let (status, body) = running
            .send(
                "POST",
                "/rpc",
                &format!(
                    r#"{{"operation":"status","input":{{"pad":"{}"}}}}"#,
                    "x".repeat(MAX_GATEWAY_BODY_BYTES)
                ),
            )
            .await;
        assert_eq!(status, 413);
        assert!(is_error(&body, TOO_LARGE.0, TOO_LARGE.1), "{body}");
        assert_eq!(recorder.calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn an_unready_runtime_refuses_every_route() {
        let recorder = backend();
        recorder.set_ready(false);
        let running = Running::start(Arc::clone(&recorder)).await;

        for (method, target, body) in [
            ("GET", "/status", ""),
            ("GET", "/capabilities", ""),
            ("POST", "/rpc", r#"{"operation":"status","input":{}}"#),
        ] {
            let (status, actual) = running.send(method, target, body).await;
            assert_eq!(status, 503, "{target}");
            assert!(is_error(&actual, NOT_READY.0, NOT_READY.1), "{actual}");
        }
        assert!(recorder.calls.lock().unwrap().is_empty());

        // 准备好之后同一个请求就能通过。
        recorder.set_ready(true);
        let (status, _) = running.send("GET", "/status", "").await;
        assert_eq!(status, 200);
    }

    #[tokio::test]
    async fn unknown_routes_methods_and_query_strings_are_not_found() {
        let running = Running::start(backend()).await;

        for (method, target, body) in [
            ("GET", "/", ""),
            ("GET", "/status?x=1", ""),
            ("POST", "/status", "{}"),
            ("GET", "/rpc", ""),
            ("PUT", "/rpc", "{}"),
            ("GET", "/status/", ""),
            ("DELETE", "/capabilities", ""),
        ] {
            let (status, actual) = running.send(method, target, body).await;
            assert_eq!(status, 404, "{method} {target}");
            assert!(is_error(&actual, NOT_FOUND.0, NOT_FOUND.1), "{actual}");
        }
    }

    #[tokio::test]
    async fn responses_are_uncached_json() {
        let running = Running::start(backend()).await;
        let raw = request(
            "GET",
            "/status",
            running.host(),
            &[("Authorization", &running.auth())],
            "",
        );
        let (status, headers, _) = exchange(running.url(), raw).await;
        assert_eq!(status, 200);
        let header = |name: &str| {
            headers
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
        };
        assert_eq!(header("content-type"), Some("application/json"));
        assert_eq!(header("cache-control"), Some("no-store"));
    }

    /// 等监听真正关闭：中止一个任务要等到它下一次被调度。
    async fn wait_until_closed(url: &str) {
        let address = url.trim_start_matches("http://").to_owned();
        tokio::time::timeout(Duration::from_secs(2), async {
            while TcpStream::connect(&address).await.is_ok() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the listener should close");
    }

    #[tokio::test]
    async fn abort_stops_at_once_and_removes_the_bridge_file() {
        let mut running = Running::start(backend()).await;
        let url = running.url().to_owned();
        assert!(running.bridge_path.exists());

        running.gateway.as_mut().unwrap().abort();
        assert!(!running.bridge_path.exists());
        wait_until_closed(&url).await;

        // 之后正式停止也不会出错。
        running.stop().await;
    }

    #[tokio::test]
    async fn dropping_the_handle_without_stopping_still_cleans_up() {
        let running = Running::start(backend()).await;
        let url = running.url().to_owned();
        let bridge_path = running.bridge_path.clone();
        assert!(bridge_path.exists());

        drop(running);
        assert!(!bridge_path.exists());
        wait_until_closed(&url).await;
    }

    #[tokio::test]
    async fn stopping_removes_the_bridge_file_and_closes_the_listener() {
        let mut running = Running::start(backend()).await;
        let url = running.url().to_owned();
        assert!(running.bridge_path.exists());

        running.stop().await;
        assert!(!running.bridge_path.exists());
        let refused = TcpStream::connect(url.trim_start_matches("http://")).await;
        assert!(refused.is_err(), "the listener should be closed");
    }
}
