//! 服务端集成测试共用的工具：真实的服务、会存 Cookie 的"浏览器"、注册好的设备。
//!
//! 服务跑在回环地址的随机端口上；"浏览器"是一个会存 Cookie 的 HTTP 客户端；
//! 桌面端用真正的 `assemble_client::Client`。这里不关心单个函数，
//! 只关心几方拼在一起之后是不是真的能用。
//!
// 每个测试文件只会用到其中一部分工具，所以在这里统一放过"未被使用"的提示。
#![allow(dead_code, reason = "每个测试文件只用到共用工具的一部分")]

use assemble_client::{Client, EventStream, RequestOptions};
use assemble_protocol::device::{Device, NewDevice};
use assemble_protocol::events::Event;
use assemble_protocol::login::{AuthorizationRequest, ExchangeInput};
use assemble_protocol::primitives::Platform;
use assemble_server::http::TestServer;
use assemble_store::secret::SecretString;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use futures_util::StreamExt;
use reqwest::header::{HeaderMap, SET_COOKIE};
use reqwest::{StatusCode, redirect};
use sha2::{Digest, Sha256};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub const USERNAME: &str = "alex";
pub const EMAIL: &str = "alex@example.test";
pub const PASSWORD: &str = "correct horse battery staple";

// ---- 替身与工具 ----

/// 一个会存 Cookie 的"浏览器"。不跟随重定向：每一步跳去哪里正是要核对的东西。
pub struct Browser {
    pub http: reqwest::Client,
    pub origin: String,
}

impl Browser {
    pub fn new(origin: &str) -> Self {
        let http = reqwest::Client::builder()
            .redirect(redirect::Policy::none())
            .cookie_store(true)
            .build()
            .unwrap();
        Self {
            http,
            origin: origin.to_owned(),
        }
    }

    pub fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.http.get(format!("{}{path}", self.origin))
    }

    /// 浏览器提交表单时会带上 `Origin`；测试里不想要它就去掉。
    pub fn post_form(&self, path: &str, fields: &[(&str, &str)]) -> reqwest::RequestBuilder {
        self.http
            .post(format!("{}{path}", self.origin))
            .header("origin", &self.origin)
            .form(fields)
    }
}

pub struct Page {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub html: String,
}

impl Page {
    pub fn location(&self) -> Option<String> {
        self.headers
            .get("location")
            .map(|value| value.to_str().unwrap().to_owned())
    }

    pub fn set_cookies(&self) -> Vec<String> {
        self.headers
            .get_all(SET_COOKIE)
            .iter()
            .map(|value| value.to_str().unwrap().to_owned())
            .collect()
    }
}

pub async fn send(request: reqwest::RequestBuilder) -> Page {
    let response = request.send().await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let html = response.text().await.unwrap();
    Page {
        status,
        headers,
        html,
    }
}

/// 从页面里取出一个隐藏字段的值。
pub fn hidden(html: &str, name: &str) -> Option<String> {
    let marker = format!("name=\"{name}\" value=\"");
    let start = html.find(&marker)? + marker.len();
    let rest = &html[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_owned())
}

/// 从跳转地址的查询串里取一个参数。
pub fn query_value(location: &str, key: &str) -> Option<String> {
    let query = location.split_once('?')?.1;
    let prefix = format!("{key}=");
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix(&prefix))
        .map(ToOwned::to_owned)
}

pub struct Fixture {
    /// 临时目录必须活得比服务久：放掉它，数据库文件就没了。
    pub dir: tempfile::TempDir,
    pub server: TestServer,
    pub browser: Browser,
}

pub async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let server = TestServer::start_default(dir.path()).await.unwrap();
    assemble_server::accounts::create(
        &server.state.pool,
        USERNAME,
        "Alex Wang",
        SecretString::new(PASSWORD.to_owned()),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE accounts SET email = 'alex@example.test', email_verified_at = 1 WHERE username = 'alex'").execute(&server.state.pool).await.unwrap();
    let browser = Browser::new(server.origin.as_str());
    Fixture {
        dir,
        server,
        browser,
    }
}

pub async fn open_login(browser: &Browser) -> Page {
    send(browser.get("/web/login")).await
}

pub async fn submit_login(
    browser: &Browser,
    username: &str,
    password: &str,
    next: &str,
    csrf: &str,
) -> Page {
    let email = if username.contains('@') {
        username.to_owned()
    } else {
        format!("{username}@example.test")
    };
    send(browser.post_form(
        "/web/login",
        &[
            ("email", email.as_str()),
            ("password", password),
            ("next", next),
            ("csrf", csrf),
        ],
    ))
    .await
}

/// 先取登录页，再用页面上的防伪值提交一次登录。
pub async fn try_login(browser: &Browser, username: &str, password: &str, next: &str) -> Page {
    let form = open_login(browser).await;
    let csrf = hidden(&form.html, "csrf").unwrap();
    submit_login(browser, username, password, next, &csrf).await
}

/// 走一遍正常的登录。已经登录的浏览器直接拿到"已登录"的页面。
pub async fn log_in(browser: &Browser) -> Page {
    let form = open_login(browser).await;
    match hidden(&form.html, "csrf") {
        Some(csrf) => submit_login(browser, USERNAME, PASSWORD, "", &csrf).await,
        None => form,
    }
}

/// 一台桌面端的登录请求，以及只留在它手里的 PKCE 校验器。
pub fn desktop_request(
    seed: char,
    name: &str,
    platform: Platform,
) -> (AuthorizationRequest, String) {
    // 43 个 base64url 字符，正好是 PKCE 校验器与 state 的线上形式。
    let verifier: String = std::iter::once(seed)
        .chain(std::iter::repeat_n('v', 42))
        .collect();
    let state: String = std::iter::once(seed)
        .chain(std::iter::repeat_n('s', 42))
        .collect();
    let request = AuthorizationRequest {
        redirect_uri: "http://127.0.0.1:54321/callback".into(),
        code_challenge: URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())),
        state,
        device: NewDevice {
            name: name.to_owned(),
            platform,
        },
    };
    (request, verifier)
}

/// 在已登录的浏览器上打开授权页并点一个按钮。
pub async fn authorize(browser: &Browser, request: &AuthorizationRequest, choice: &str) -> Page {
    let path = assemble_server::desktop_login::authorize_path(request);
    let page = send(browser.get(&path)).await;
    assert_eq!(
        page.status,
        StatusCode::OK,
        "the authorize page should open"
    );
    let csrf = hidden(&page.html, "csrf").unwrap();
    send(browser.post_form(
        "/desktop/authorize",
        &[
            (
                "request",
                &assemble_server::desktop_login::encode_request(request),
            ),
            ("csrf", &csrf),
            ("choice", choice),
        ],
    ))
    .await
}

/// 从跳转地址里取出授权码与 state。
pub fn code_from(page: &Page, request: &AuthorizationRequest) -> String {
    assert_eq!(
        page.status,
        StatusCode::SEE_OTHER,
        "confirming should redirect"
    );
    let location = page.location().expect("confirming redirects the browser");
    assert!(location.starts_with(&request.redirect_uri), "{location}");
    assert_eq!(
        query_value(&location, "state").as_deref(),
        Some(request.state.as_str()),
        "the state must come back untouched"
    );
    query_value(&location, "code").expect("an authorization code")
}

pub fn exchange_input(
    request: &AuthorizationRequest,
    verifier: &str,
    code: String,
) -> ExchangeInput {
    ExchangeInput {
        code,
        code_verifier: verifier.to_owned(),
        redirect_uri: request.redirect_uri.clone(),
        device: request.device.clone(),
    }
}

pub struct Registered {
    pub client: Client,
    pub token: SecretString,
    pub device: Device,
    pub cancel: CancellationToken,
}
/// 测试只完成双方握手与核对，仍保持 pending；批准另行调用，不能用它替代人工核验。
pub async fn prepare_remote(
    controller: &Registered,
    host: &Registered,
    session: &assemble_protocol::remote::RemoteSession,
) {
    for _ in 0..4 {
        host.remote_prepare_host(session, &host.options())
            .await
            .unwrap();
        let pending = controller
            .remote_poll(&session.id, 0, 0, &controller.options())
            .await
            .unwrap();
        assert_eq!(
            pending.session.phase,
            assemble_protocol::remote::RemotePhase::Pending
        );
    }
    let code = controller.remote_verification(&session.id).unwrap();
    assert_eq!(Some(code), host.remote_verification(&session.id));
}

/// 客户端的方法直接用，不必每次都写 `.client`。
impl std::ops::Deref for Registered {
    type Target = Client;

    fn deref(&self) -> &Client {
        &self.client
    }
}

impl Registered {
    pub fn options(&self) -> RequestOptions<'_> {
        RequestOptions::new(&self.cancel)
    }

    pub fn open_events(&self) -> EventStream {
        self.client.clone().events(self.cancel.child_token())
    }
}

/// 替一台桌面端走完"授权 → 换码"，得到它的客户端与令牌。
pub async fn register(fixture: &Fixture, seed: char, name: &str, platform: Platform) -> Registered {
    log_in(&fixture.browser).await;
    let (request, verifier) = desktop_request(seed, name, platform);
    let code = code_from(
        &authorize(&fixture.browser, &request, "allow").await,
        &request,
    );
    let client = Client::new(fixture.server.origin.clone()).unwrap();
    let input = exchange_input(&request, &verifier, code);
    let cancel = CancellationToken::new();
    let (response, token) = client
        .exchange(&input, &RequestOptions::new(&cancel))
        .await
        .unwrap();
    let registered = Registered {
        client,
        token,
        device: response.device,
        cancel,
    };
    registered.client.set_bearer(Some(registered.token.clone()));
    registered
}

pub async fn next_event(stream: &mut EventStream) -> Event {
    tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("an event should arrive")
        .expect("the stream should stay open")
        .expect("a valid event")
}

/// 等到一条满足条件的设备列表事件，并把它交出来。
pub async fn wait_devices(
    stream: &mut EventStream,
    satisfied: impl Fn(&[Device]) -> bool,
) -> Vec<Device> {
    for _ in 0..20 {
        if let Event::DevicesChanged { devices } = next_event(stream).await
            && satisfied(&devices)
        {
            return devices;
        }
    }
    panic!("no device list satisfied the condition")
}

pub fn online_of<'a>(devices: &'a [Device], name: &str) -> Option<&'a Device> {
    devices.iter().find(|device| device.name == name)
}

pub async fn account_of(fixture: &Fixture) -> String {
    sqlx::query_scalar("SELECT id FROM accounts")
        .fetch_one(&fixture.server.state.pool)
        .await
        .unwrap()
}
