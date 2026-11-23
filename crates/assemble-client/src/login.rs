//! 桌面端登录：系统浏览器授权，经回环地址交回单次授权码。
//!
//! 桌面端不保存密码，也不在自己的窗口里加载登录页。流程是：
//!
//! 1. 本机监听 `127.0.0.1` 的随机端口；
//! 2. 生成 PKCE 校验器与 state，在系统浏览器打开服务端的授权页；
//! 3. 用户在浏览器里登录并确认，服务端把单次授权码交回回调地址；
//! 4. 本机收回调，核对 state 后用授权码换取设备令牌。
//!
//! 回调只认一次：方法、路径、主机头、参数个数与 state 任何一项不符都直接拒绝。
//! 用户在网页上点了"取消"时，服务端带 `error=access_denied` 回来，
//! 这里把它变成 [`LoginError::Denied`]，应用据此不显示错误。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use assemble_protocol::device::NewDevice;
use assemble_protocol::login::{
    AuthorizationRequest, CALLBACK_PATH, ExchangeInput, LOGIN_DEADLINE, is_proof,
};
use assemble_store::secret::SecretString;

/// 回调成功后给浏览器看的页面。页面上不出现授权码。
const CALLBACK_PAGE: &str = "<!doctype html><html lang=\"zh-CN\"><head><meta charset=\"utf-8\"><title>登录完成</title></head><body><p>已完成网页登录，请返回 Assemble。可以关闭此页面。</p></body></html>";

/// 用户在网页上取消之后给浏览器看的页面。
const DENIED_PAGE: &str = "<!doctype html><html lang=\"zh-CN\"><head><meta charset=\"utf-8\"><title>已取消</title></head><body><p>已取消登录 Assemble。可以关闭此页面。</p></body></html>";

/// 用户在授权页上点"取消"时，服务端带回的错误码。
const ACCESS_DENIED: &str = "access_denied";

const PAGE_HEADERS: [(&str, &str); 3] = [
    ("content-type", "text/html; charset=utf-8"),
    ("cache-control", "no-store"),
    (
        "content-security-policy",
        "default-src 'none'; frame-ancestors 'none'",
    ),
];

const REFUSAL_HEADERS: [(&str, &str); 2] = [
    ("content-type", "text/plain; charset=utf-8"),
    ("cache-control", "no-store"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LoginError {
    #[error("the login callback could not listen on loopback")]
    Bind,
    #[error("login was cancelled")]
    Cancelled,
    /// 用户在浏览器的授权页上点了"取消"。这不是故障。
    #[error("the user declined the login in the browser")]
    Denied,
    #[error("no login confirmation arrived in time")]
    TimedOut,
    #[error("a random login secret could not be generated")]
    RandomUnavailable,
}

/// 一次登录请求：交给浏览器的东西，和只留在本机的校验器。
#[derive(Debug)]
pub struct LoginRequest {
    redirect_uri: String,
    code_challenge: String,
    state: String,
    verifier: SecretString,
}

impl LoginRequest {
    /// 为一个回调地址生成新的请求。校验器与 state 各取 32 字节随机数。
    pub fn generate(redirect_uri: impl Into<String>) -> Result<Self, LoginError> {
        let verifier = random_proof()?;
        let state = random_proof()?;
        // S256：挑战是校验器的 SHA-256。
        let code_challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        Ok(Self {
            redirect_uri: redirect_uri.into(),
            code_challenge,
            state,
            verifier: SecretString::new(verifier),
        })
    }

    /// 发给授权页的三个字段。
    pub fn authorization_request(&self) -> AuthorizationRequest {
        AuthorizationRequest {
            redirect_uri: self.redirect_uri.clone(),
            code_challenge: self.code_challenge.clone(),
            state: self.state.clone(),
        }
    }

    /// 授权页地址里携带的请求：JSON 的 base64url 编码。
    pub fn encoded_request(&self) -> String {
        // 三个普通字符串字段的序列化不会失败。
        let json = serde_json::to_vec(&self.authorization_request()).unwrap_or_default();
        URL_SAFE_NO_PAD.encode(json)
    }

    /// 用收回的授权码组装换码请求。换码的同时注册这台设备。
    pub fn exchange_input(&self, code: String, device: NewDevice) -> ExchangeInput {
        ExchangeInput {
            code,
            code_verifier: self.verifier.expose().to_owned(),
            redirect_uri: self.redirect_uri.clone(),
            device,
        }
    }
}

fn random_proof() -> Result<String, LoginError> {
    let mut random = [0u8; 32];
    getrandom::fill(&mut random).map_err(|_| LoginError::RandomUnavailable)?;
    Ok(URL_SAFE_NO_PAD.encode(random))
}

/// 等待回调时的两个退出条件。
#[derive(Debug, Clone, Copy)]
pub struct WaitOptions<'a> {
    pub cancel: &'a CancellationToken,
    pub deadline: Duration,
}

impl<'a> WaitOptions<'a> {
    /// 在规定的最长时间内等待用户完成登录。
    pub fn new(cancel: &'a CancellationToken, deadline: Duration) -> Self {
        Self { cancel, deadline }
    }

    /// 使用产品约定的等待上限：五分钟。
    pub fn standard(cancel: &'a CancellationToken) -> Self {
        Self::new(cancel, LOGIN_DEADLINE)
    }
}

/// 一次登录用的回环监听。
pub struct LoginListener {
    listener: TcpListener,
    host: String,
}

impl LoginListener {
    /// 在回环地址的随机端口上监听。
    pub async fn bind() -> Result<Self, LoginError> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|_| LoginError::Bind)?;
        let port = listener
            .local_addr()
            .map(|address| address.port())
            .map_err(|_| LoginError::Bind)?;
        Ok(Self {
            listener,
            host: format!("127.0.0.1:{port}"),
        })
    }

    /// 交给授权页的回调地址。
    pub fn redirect_uri(&self) -> String {
        format!("http://{}{CALLBACK_PATH}", self.host)
    }

    /// 等待浏览器交回授权码。成功、取消或超时之后监听都会结束。
    pub async fn wait(
        self,
        request: &LoginRequest,
        options: WaitOptions<'_>,
    ) -> Result<String, LoginError> {
        if options.cancel.is_cancelled() {
            return Err(LoginError::Cancelled);
        }
        let (sender, receiver) = tokio::sync::oneshot::channel::<Result<String, LoginError>>();
        let stop = CancellationToken::new();
        let state = Arc::new(CallbackState {
            expected_state: request.state.as_bytes().to_vec(),
            host: self.host.clone(),
            sender: Mutex::new(Some(sender)),
            completed: AtomicBool::new(false),
            stop: stop.clone(),
        });

        let router = Router::new().fallback(callback).with_state(state);
        let serve_stop = stop.clone();
        let serving = async move {
            let server = axum::serve(self.listener, router)
                .with_graceful_shutdown(serve_stop.cancelled_owned());
            if let Err(error) = server.await {
                tracing::debug!(%error, "login callback listener stopped");
            }
        };

        let outcome = tokio::select! {
            biased;
            () = options.cancel.cancelled() => Err(LoginError::Cancelled),
            () = tokio::time::sleep(options.deadline) => Err(LoginError::TimedOut),
            outcome = receiver => outcome.map_err(|_| LoginError::Cancelled)?,
            () = serving => Err(LoginError::Cancelled),
        };
        stop.cancel();
        outcome
    }
}

struct CallbackState {
    expected_state: Vec<u8>,
    host: String,
    sender: Mutex<Option<tokio::sync::oneshot::Sender<Result<String, LoginError>>>>,
    completed: AtomicBool,
    stop: CancellationToken,
}

async fn callback(State(state): State<Arc<CallbackState>>, request: Request<Body>) -> Response {
    let outcome = match validate(&request, &state.host, &state.expected_state) {
        Ok(outcome) => outcome,
        Err(reason) => {
            return (StatusCode::BAD_REQUEST, REFUSAL_HEADERS, reason).into_response();
        }
    };
    // 回调只认一次。
    if state.completed.swap(true, Ordering::SeqCst) {
        return (
            StatusCode::BAD_REQUEST,
            REFUSAL_HEADERS,
            "Login already completed",
        )
            .into_response();
    }
    let (delivered, page) = match outcome {
        Callback::Code(code) => (Ok(code), CALLBACK_PAGE),
        Callback::Denied => (Err(LoginError::Denied), DENIED_PAGE),
    };
    if let Ok(mut sender) = state.sender.lock() {
        // 取走发送端：之后的回调再也送不进来。
        if let Some(sender) = sender.take() {
            let _ = sender.send(delivered);
        }
    }
    state.stop.cancel();
    (StatusCode::OK, PAGE_HEADERS, page).into_response()
}

/// 一个通过了核对的回调。
enum Callback {
    Code(String),
    Denied,
}

/// 核对一个回调请求，通过则取出授权码（或"用户取消了"这个事实）。
fn validate(
    request: &Request<Body>,
    host: &str,
    expected_state: &[u8],
) -> Result<Callback, &'static str> {
    if request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        != Some(host)
    {
        return Err("Unexpected host");
    }
    if request.method() != Method::GET {
        return Err("Expected GET");
    }
    let uri = request.uri();
    if uri.path() != CALLBACK_PATH {
        return Err("Unexpected path");
    }
    let query = uri.query().ok_or("Missing parameters")?;
    // 授权码与 state 都只含 base64url 字符；出现编码说明这不是我们发出去的那一个请求。
    if query.contains(['%', '+']) {
        return Err("Encoded parameters are not accepted");
    }

    let mut code: Option<&str> = None;
    let mut error: Option<&str> = None;
    let mut state: Option<&str> = None;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').ok_or("Malformed parameter")?;
        let slot = match key {
            "code" => &mut code,
            "error" => &mut error,
            "state" => &mut state,
            _ => return Err("Unexpected parameter"),
        };
        if slot.replace(value).is_some() {
            return Err("Duplicate parameter");
        }
    }
    let state = state.ok_or("Missing state")?;
    if !same_secret(state.as_bytes(), expected_state) {
        return Err("Invalid state");
    }
    match (code, error) {
        // 授权码与"取消"互斥；只认这一个错误码，别的错误一律按被拒绝处理。
        (Some(code), None) if is_proof(code) => Ok(Callback::Code(code.to_owned())),
        (None, Some(ACCESS_DENIED)) => Ok(Callback::Denied),
        _ => Err("Invalid parameters"),
    }
}

fn same_secret(given: &[u8], expected: &[u8]) -> bool {
    given.len() == expected.len() && bool::from(given.ct_eq(expected))
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    use super::*;
    use assemble_protocol::primitives::Platform;

    const CODE: &str = "abcdefghijklmnopqrstuvwxyz0123456789-_ABCDE";

    struct Running {
        host: String,
        state: String,
        outcome: tokio::task::JoinHandle<Result<String, LoginError>>,
        cancel: CancellationToken,
    }

    /// 启动一次登录等待：先建好监听，再在后台等回调。
    async fn start(deadline: Duration) -> Running {
        let cancel = CancellationToken::new();
        let listener = LoginListener::bind().await.unwrap();
        let request = LoginRequest::generate(listener.redirect_uri()).unwrap();
        let state = request.state.clone();
        let host = listener.host.clone();
        let task_cancel = cancel.clone();
        let outcome = tokio::spawn(async move {
            listener
                .wait(&request, WaitOptions::new(&task_cancel, deadline))
                .await
        });
        Running {
            host,
            state,
            outcome,
            cancel,
        }
    }

    fn get(host: &str, target: &str) -> String {
        format!(
            "GET {target} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
        )
    }

    /// 用原始 TCP 发请求并读完应答，这样才能精确控制方法、路径与请求头。
    async fn send(host: &str, raw: &str) -> (u16, Vec<(String, String)>, String) {
        let mut stream = TcpStream::connect(host).await.unwrap();
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
        (status, headers, body.to_owned())
    }

    #[test]
    fn a_login_request_carries_matching_challenge_verifier_and_state() {
        let request = LoginRequest::generate("http://127.0.0.1:54321/callback").unwrap();
        let input = request.authorization_request();
        assert!(input.is_valid());
        assert!(is_proof(&input.code_challenge));
        assert!(is_proof(&request.state));
        assert_ne!(input.code_challenge, request.state);
        assert_eq!(input.redirect_uri, "http://127.0.0.1:54321/callback");
        assert_eq!(request.redirect_uri, "http://127.0.0.1:54321/callback");

        // S256：挑战等于校验器的 SHA-256。
        let exchange = request.exchange_input(
            "c".repeat(43),
            NewDevice {
                name: "MacBook".into(),
                platform: Platform::Macos,
            },
        );
        assert_eq!(
            input.code_challenge,
            URL_SAFE_NO_PAD.encode(Sha256::digest(exchange.code_verifier.as_bytes()))
        );
        assert!(exchange.is_valid());
        assert_eq!(exchange.redirect_uri, "http://127.0.0.1:54321/callback");

        // 授权页地址里的请求能原样解码回来。
        let decoded: serde_json::Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(request.encoded_request()).unwrap())
                .unwrap();
        assert_eq!(decoded["redirectUri"], "http://127.0.0.1:54321/callback");
        assert_eq!(decoded["codeChallenge"], input.code_challenge);
        assert_eq!(decoded["state"], input.state);

        // 每次生成的值都不一样。
        let other = LoginRequest::generate("http://127.0.0.1:54321/callback").unwrap();
        assert_ne!(other.state, request.state);
        assert_ne!(
            other
                .exchange_input(String::new(), exchange.device.clone())
                .code_verifier,
            exchange.code_verifier
        );
    }

    #[tokio::test]
    async fn a_valid_callback_returns_the_code_and_shows_a_clean_page() {
        let running = start(Duration::from_secs(5)).await;
        let host = running.host.clone();
        let target = format!("/callback?code={CODE}&state={}", running.state);

        let (status, headers, body) = send(&host, &get(&host, &target)).await;
        assert_eq!(status, 200);
        assert!(body.contains("已完成网页登录，请返回 Assemble"));
        // 授权码不出现在页面里。
        assert!(!body.contains(CODE));
        let header = |name: &str| {
            headers
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
        };
        assert_eq!(header("cache-control"), Some("no-store"));
        assert_eq!(
            header("content-security-policy"),
            Some("default-src 'none'; frame-ancestors 'none'")
        );
        assert_eq!(header("content-type"), Some("text/html; charset=utf-8"));
        assert_eq!(running.outcome.await.unwrap().unwrap(), CODE);
    }

    #[tokio::test]
    async fn anything_other_than_the_expected_callback_is_refused() {
        let running = start(Duration::from_secs(5)).await;
        let host = running.host.clone();
        let state = &running.state;
        let wrong_state = "A".repeat(43);
        let too_short = "x".repeat(42);

        let attempts = [
            get(&host, &format!("/callback?code={CODE}&state={wrong_state}")),
            get(&host, &format!("/callback?code={CODE}")),
            get(&host, &format!("/callback?state={state}")),
            get(
                &host,
                &format!("/callback?code={CODE}&code={CODE}&state={state}"),
            ),
            get(
                &host,
                &format!("/callback?code={CODE}&state={state}&state={state}"),
            ),
            get(
                &host,
                &format!("/callback?code={CODE}&state={state}&extra=1"),
            ),
            get(
                &host,
                &format!("/callback?error=access_denied&state={wrong_state}"),
            ),
            get(&host, "/callback?error=access_denied"),
            get(
                &host,
                &format!("/callback?error=server_error&state={state}"),
            ),
            get(
                &host,
                &format!("/callback?code={CODE}&error=access_denied&state={state}"),
            ),
            get(&host, &format!("/callback?code={too_short}&state={state}")),
            get(&host, &format!("/callback?code={CODE}&state={state}&")),
            get(&host, &format!("/callback?code{CODE}&state={state}")),
            get(&host, &format!("/other?code={CODE}&state={state}")),
            get(&host, &format!("/callback/?code={CODE}&state={state}")),
            get(
                "evil.example",
                &format!("/callback?code={CODE}&state={state}"),
            ),
            get(
                &host,
                &format!(
                    "/callback?code=%41BCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_abcde&state={state}"
                ),
            ),
            format!(
                "POST /callback?code={CODE}&state={state} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
            ),
        ];
        for (index, raw) in attempts.iter().enumerate() {
            let (status, _, body) = send(&host, raw).await;
            assert_eq!(status, 400, "attempt {index}");
            assert!(!body.contains(CODE), "attempt {index} leaked the code");
        }

        // 被拒绝的请求不影响随后到达的合法回调，参数顺序也无所谓。
        let (status, _, _) = send(
            &host,
            &get(&host, &format!("/callback?state={state}&code={CODE}")),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(running.outcome.await.unwrap().unwrap(), CODE);
    }

    #[tokio::test]
    async fn declining_in_the_browser_ends_the_wait_without_an_error_page() {
        let running = start(Duration::from_secs(5)).await;
        let host = running.host.clone();
        let target = format!("/callback?error=access_denied&state={}", running.state);

        let (status, _, body) = send(&host, &get(&host, &target)).await;
        assert_eq!(status, 200);
        assert!(body.contains("已取消登录 Assemble"));
        assert!(!body.contains("已完成网页登录"));
        assert_eq!(
            running.outcome.await.unwrap().unwrap_err(),
            LoginError::Denied
        );
    }

    #[tokio::test]
    async fn the_callback_is_accepted_only_once() {
        let running = start(Duration::from_secs(5)).await;
        let host = running.host.clone();
        let target = format!("/callback?code={CODE}&state={}", running.state);
        let (first, _, _) = send(&host, &get(&host, &target)).await;
        assert_eq!(first, 200);
        assert_eq!(running.outcome.await.unwrap().unwrap(), CODE);

        // 监听已经结束：第二次回调要么连不上，要么得不到成功应答。
        tokio::task::yield_now().await;
        let mut stream = match TcpStream::connect(&host).await {
            Ok(stream) => stream,
            Err(_) => return,
        };
        let _ = stream.write_all(get(&host, &target).as_bytes()).await;
        let mut response = Vec::new();
        let _ = stream.read_to_end(&mut response).await;
        let text = String::from_utf8_lossy(&response);
        assert!(
            !text.contains("已完成网页登录"),
            "a second callback was accepted: {text}"
        );
    }

    #[tokio::test]
    async fn cancelling_ends_the_wait() {
        let running = start(Duration::from_secs(30)).await;
        running.cancel.cancel();
        assert_eq!(running.outcome.await.unwrap(), Err(LoginError::Cancelled));
    }

    #[tokio::test]
    async fn running_out_of_time_ends_the_wait() {
        let running = start(Duration::from_millis(40)).await;
        let host = running.host.clone();
        assert_eq!(running.outcome.await.unwrap(), Err(LoginError::TimedOut));
        // 超时之后监听确实关掉了。
        tokio::task::yield_now().await;
        let refused = TcpStream::connect(&host).await;
        assert!(refused.is_err(), "the listener should be closed");
    }

    #[tokio::test]
    async fn a_cancelled_wait_returns_before_listening_for_long() {
        let listener = LoginListener::bind().await.unwrap();
        let request = LoginRequest::generate(listener.redirect_uri()).unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();
        let outcome = listener
            .wait(&request, WaitOptions::new(&cancel, Duration::from_secs(30)))
            .await;
        assert_eq!(outcome, Err(LoginError::Cancelled));
    }

    #[test]
    fn the_standard_wait_uses_the_product_deadline() {
        let cancel = CancellationToken::new();
        assert_eq!(WaitOptions::standard(&cancel).deadline, LOGIN_DEADLINE);
        assert_eq!(LOGIN_DEADLINE, Duration::from_secs(300));
        assert!(CALLBACK_PAGE.starts_with("<!doctype html>"));
    }
}
