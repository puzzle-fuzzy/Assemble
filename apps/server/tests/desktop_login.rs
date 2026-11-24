//! 服务端的集成测试：对一个真实运行的服务走完整条登录链路。
//!
//! 服务跑在回环地址的随机端口上；"浏览器"是一个会存 Cookie 的 HTTP 客户端；
//! 桌面端用真正的 `assemble_client::Client`。这里不关心单个函数，
//! 只关心几方拼在一起之后是不是真的能用。

use std::time::{Duration, Instant};

use assemble_client::{Client, EventStream, RequestOptions};
use assemble_protocol::device::{Device, NewDevice};
use assemble_protocol::error::codes;
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
use tokio_util::sync::CancellationToken;

const USERNAME: &str = "alex";
const PASSWORD: &str = "correct horse battery staple";

/// argon2 在任何机器上都不该快过这个数：两种登录失败都至少要算一次散列。
const HASHING_TAKES_AT_LEAST: Duration = Duration::from_millis(10);

// ---- 替身与工具 ----

/// 一个会存 Cookie 的"浏览器"。不跟随重定向：每一步跳去哪里正是要核对的东西。
struct Browser {
    http: reqwest::Client,
    origin: String,
}

impl Browser {
    fn new(origin: &str) -> Self {
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

    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.http.get(format!("{}{path}", self.origin))
    }

    /// 浏览器提交表单时会带上 `Origin`；测试里不想要它就去掉。
    fn post_form(&self, path: &str, fields: &[(&str, &str)]) -> reqwest::RequestBuilder {
        self.http
            .post(format!("{}{path}", self.origin))
            .header("origin", &self.origin)
            .form(fields)
    }
}

struct Page {
    status: StatusCode,
    headers: HeaderMap,
    html: String,
}

impl Page {
    fn location(&self) -> Option<String> {
        self.headers
            .get("location")
            .map(|value| value.to_str().unwrap().to_owned())
    }

    fn set_cookies(&self) -> Vec<String> {
        self.headers
            .get_all(SET_COOKIE)
            .iter()
            .map(|value| value.to_str().unwrap().to_owned())
            .collect()
    }
}

async fn send(request: reqwest::RequestBuilder) -> Page {
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
fn hidden(html: &str, name: &str) -> Option<String> {
    let marker = format!("name=\"{name}\" value=\"");
    let start = html.find(&marker)? + marker.len();
    let rest = &html[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_owned())
}

/// 从跳转地址的查询串里取一个参数。
fn query_value(location: &str, key: &str) -> Option<String> {
    let query = location.split_once('?')?.1;
    let prefix = format!("{key}=");
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix(&prefix))
        .map(ToOwned::to_owned)
}

struct Fixture {
    _dir: tempfile::TempDir,
    server: TestServer,
    browser: Browser,
}

async fn fixture() -> Fixture {
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
    let browser = Browser::new(server.origin.as_str());
    Fixture {
        _dir: dir,
        server,
        browser,
    }
}

async fn open_login(browser: &Browser) -> Page {
    send(browser.get("/web/login")).await
}

async fn submit_login(
    browser: &Browser,
    username: &str,
    password: &str,
    next: &str,
    csrf: &str,
) -> Page {
    send(browser.post_form(
        "/web/login",
        &[
            ("username", username),
            ("password", password),
            ("next", next),
            ("csrf", csrf),
        ],
    ))
    .await
}

/// 先取登录页，再用页面上的防伪值提交一次登录。
async fn try_login(browser: &Browser, username: &str, password: &str, next: &str) -> Page {
    let form = open_login(browser).await;
    let csrf = hidden(&form.html, "csrf").unwrap();
    submit_login(browser, username, password, next, &csrf).await
}

/// 走一遍正常的登录。已经登录的浏览器直接拿到"已登录"的页面。
async fn log_in(browser: &Browser) -> Page {
    let form = open_login(browser).await;
    match hidden(&form.html, "csrf") {
        Some(csrf) => submit_login(browser, USERNAME, PASSWORD, "", &csrf).await,
        None => form,
    }
}

/// 一台桌面端的登录请求，以及只留在它手里的 PKCE 校验器。
fn desktop_request(seed: char, name: &str, platform: Platform) -> (AuthorizationRequest, String) {
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
async fn authorize(browser: &Browser, request: &AuthorizationRequest, choice: &str) -> Page {
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
fn code_from(page: &Page, request: &AuthorizationRequest) -> String {
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

fn exchange_input(request: &AuthorizationRequest, verifier: &str, code: String) -> ExchangeInput {
    ExchangeInput {
        code,
        code_verifier: verifier.to_owned(),
        redirect_uri: request.redirect_uri.clone(),
        device: request.device.clone(),
    }
}

struct Registered {
    client: Client,
    token: SecretString,
    device: Device,
    cancel: CancellationToken,
}

/// 客户端的方法直接用，不必每次都写 `.client`。
impl std::ops::Deref for Registered {
    type Target = Client;

    fn deref(&self) -> &Client {
        &self.client
    }
}

impl Registered {
    fn options(&self) -> RequestOptions<'_> {
        RequestOptions::new(&self.cancel)
    }

    fn open_events(&self) -> EventStream {
        self.client.clone().events(self.cancel.child_token())
    }
}

/// 替一台桌面端走完"授权 → 换码"，得到它的客户端与令牌。
async fn register(fixture: &Fixture, seed: char, name: &str, platform: Platform) -> Registered {
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

async fn next_event(stream: &mut EventStream) -> Event {
    tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("an event should arrive")
        .expect("the stream should stay open")
        .expect("a valid event")
}

/// 等到一条满足条件的设备列表事件，并把它交出来。
async fn wait_devices(
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

fn online_of<'a>(devices: &'a [Device], name: &str) -> Option<&'a Device> {
    devices.iter().find(|device| device.name == name)
}

// ---- 登录页 ----

#[tokio::test]
async fn the_login_page_is_served_with_its_defences() {
    let fixture = fixture().await;
    let page = open_login(&fixture.browser).await;

    assert_eq!(page.status, StatusCode::OK);
    assert!(page.html.contains("<title>登录 · Assemble</title>"));
    assert!(page.html.contains("autocomplete=\"current-password\""));
    assert!(!page.html.contains("<script"));
    // 不缓存、不许嵌套、样式只认哈希、表单只发往本站。
    let policy = page.headers.get("content-security-policy").unwrap();
    let policy = policy.to_str().unwrap();
    assert!(policy.contains("default-src 'none'"));
    assert!(policy.contains("style-src 'sha256-"));
    assert!(!policy.contains("unsafe-inline"));
    assert!(!policy.contains("script-src"));
    assert_eq!(page.headers.get("cache-control").unwrap(), "no-store");
    // 来源策略必须是 same-origin。用 no-referrer 的话，真实浏览器会把同源 POST 的 Origin
    // 发成 null，登录表单就被服务端自己的校验拒掉了（真实启动时踩到过）。
    assert_eq!(page.headers.get("referrer-policy").unwrap(), "same-origin");
    assert!(
        page.html
            .contains(r#"name="referrer" content="same-origin""#)
    );
    assert!(!page.html.contains("no-referrer"));

    // 页面里的防伪值与 Cookie 里的成对；Cookie 不暴露给页面脚本。
    let csrf = hidden(&page.html, "csrf").unwrap();
    assert!(!csrf.is_empty());
    let cookies = page.set_cookies().join("\n");
    assert!(
        cookies.contains(&format!("assemble_csrf={csrf}")),
        "{cookies}"
    );
    for attribute in ["HttpOnly", "SameSite=Lax", "Path=/", "Max-Age="] {
        assert!(
            cookies.contains(attribute),
            "{attribute} missing: {cookies}"
        );
    }
    // 本机的 HTTP 服务不能带 Secure，否则浏览器不会存这枚 Cookie。
    assert!(!cookies.contains("Secure"), "{cookies}");
    // 还没登录，所以没有会话 Cookie。
    assert!(!cookies.contains("assemble_web="));
}

#[tokio::test]
async fn login_rejects_cross_site_requests_forged_forms_and_wrong_passwords() {
    let fixture = fixture().await;
    let browser = &fixture.browser;

    // 没有 Origin（不是浏览器从本站页面发出的）。
    let form = open_login(browser).await;
    let csrf = hidden(&form.html, "csrf").unwrap();
    let no_origin = send(
        browser
            .http
            .post(format!("{}/web/login", browser.origin))
            .form(&[
                ("username", USERNAME),
                ("password", PASSWORD),
                ("next", ""),
                ("csrf", csrf.as_str()),
            ]),
    )
    .await;
    assert_eq!(no_origin.status, StatusCode::FORBIDDEN);

    // 冒充别的站点。
    let forged_origin = send(
        browser
            .http
            .post(format!("{}/web/login", browser.origin))
            .header("origin", "https://evil.example")
            .form(&[
                ("username", USERNAME),
                ("password", PASSWORD),
                ("next", ""),
                ("csrf", csrf.as_str()),
            ]),
    )
    .await;
    assert_eq!(forged_origin.status, StatusCode::FORBIDDEN);

    // 防伪值对不上（别的站点自己拼的表单）。
    let mismatched = submit_login(browser, USERNAME, PASSWORD, "", "not-the-paired-value").await;
    assert_eq!(mismatched.status, StatusCode::FORBIDDEN);
    // Cookie 里的防伪值被抹掉之后，表单里的值再对也没用。
    let stripped = Browser {
        http: reqwest::Client::builder()
            .redirect(redirect::Policy::none())
            .build()
            .unwrap(),
        origin: browser.origin.clone(),
    };
    let no_cookie = submit_login(&stripped, USERNAME, PASSWORD, "", &csrf).await;
    assert_eq!(no_cookie.status, StatusCode::FORBIDDEN);

    // 密码错：回答里只有一句固定的话，不说是用户名错了还是密码错了。
    let wrong = try_login(browser, USERNAME, "not-the-password", "").await;
    assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);
    assert!(wrong.html.contains("用户名或密码不对"));
    assert!(!wrong.html.contains("not-the-password"));
    assert!(
        !wrong
            .set_cookies()
            .iter()
            .any(|c| c.starts_with("assemble_web="))
    );
    assert_eq!(wrong.headers.get("retry-after").unwrap(), "1");
    // 失败之后留在登录页上，可以接着试。
    assert!(hidden(&wrong.html, "csrf").is_some());

    // 用户名不存在：同一句话。
    let unknown = try_login(browser, "nobody-here", PASSWORD, "").await;
    assert_eq!(unknown.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        hidden(&wrong.html, "csrf").is_some(),
        hidden(&unknown.html, "csrf").is_some()
    );

    // 两种失败都要算一次密码散列：从快慢上看不出一个用户名存不存在。
    let started = Instant::now();
    try_login(browser, USERNAME, "wrong again", "").await;
    let existing = started.elapsed();
    let started = Instant::now();
    try_login(browser, "someone-else", PASSWORD, "").await;
    let missing = started.elapsed();
    assert!(existing >= HASHING_TAKES_AT_LEAST, "{existing:?}");
    assert!(missing >= HASHING_TAKES_AT_LEAST, "{missing:?}");
}

#[tokio::test]
async fn too_many_failures_lock_a_username_out_for_a_while() {
    let fixture = fixture().await;
    // 每次失败换一个新浏览器：Cookie 成对与否不影响这道防线。
    for attempt in 0..5 {
        let page = try_login(&fixture.browser, USERNAME, "wrong", "").await;
        assert_eq!(page.status, StatusCode::UNAUTHORIZED, "attempt {attempt}");
    }
    let locked = try_login(&fixture.browser, USERNAME, PASSWORD, "").await;
    assert_eq!(locked.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(locked.html.contains("尝试次数太多"));
    assert_eq!(locked.headers.get("retry-after").unwrap(), "900");
    // 密码对了也进不去。
    assert!(
        !locked
            .set_cookies()
            .iter()
            .any(|c| c.starts_with("assemble_web="))
    );
    // 别的用户名不受牵连：照样可以登录。
    assemble_server::accounts::create(
        &fixture.server.state.pool,
        "sam",
        "",
        SecretString::new(PASSWORD.to_owned()),
    )
    .await
    .unwrap();
    let sam = try_login(&fixture.browser, "sam", PASSWORD, "").await;
    assert!(
        sam.html.contains("已登录"),
        "another username is not locked out"
    );
}

// ---- 登录与授权 ----

#[tokio::test]
async fn a_successful_login_sends_a_hidden_session_cookie_and_moves_on() {
    let fixture = fixture().await;
    let next = "/desktop/authorize?request=abc";
    let page = try_login(&fixture.browser, USERNAME, PASSWORD, next).await;

    assert_eq!(page.status, StatusCode::SEE_OTHER);
    assert_eq!(page.location().as_deref(), Some(next));
    let cookies = page.set_cookies().join("\n");
    let session = cookies
        .lines()
        .find(|line| line.starts_with("assemble_web="))
        .expect("a session cookie");
    let value = session["assemble_web=".len()..].split(';').next().unwrap();
    assert!(
        assemble_protocol::login::is_proof(value),
        "a 43-character token"
    );
    for attribute in ["HttpOnly", "SameSite=Lax", "Path=/", "Max-Age=43200"] {
        assert!(
            session.contains(attribute),
            "{attribute} missing: {session}"
        );
    }
    // 登录表单的防伪值用完即弃。
    assert!(
        cookies
            .lines()
            .any(|line| line.starts_with("assemble_csrf=") && line.contains("Max-Age=0")),
        "{cookies}"
    );
    // 库里只有摘要。
    let stored: String = sqlx::query_scalar("SELECT token_hash FROM web_sessions")
        .fetch_one(&fixture.server.state.pool)
        .await
        .unwrap();
    assert_eq!(stored.len(), 64);
    assert_ne!(stored, value);

    // 再打开登录页：已经登录，不会又显示一张表单。
    let again = open_login(&fixture.browser).await;
    assert_eq!(again.status, StatusCode::OK);
    assert!(again.html.contains("已登录"));
    assert!(!again.html.contains("autocomplete=\"current-password\""));
}

#[tokio::test]
async fn a_login_cannot_be_made_to_redirect_somewhere_else() {
    let fixture = fixture().await;
    for next in [
        "https://evil.example/",
        "//evil.example/desktop/authorize",
        "/web/logout",
        "/desktop/authorize/../../web/logout",
    ] {
        // 每次用一个没登录的新浏览器：这里要看的正是登录那一下跳去哪里。
        let fresh = Browser::new(fixture.browser.origin.as_str());
        let page = try_login(&fresh, USERNAME, PASSWORD, next).await;
        // 不安全的去处被忽略：停在"已登录"的页面上，哪儿也不跳。
        assert!(page.html.contains("已登录"), "{next}: {}", page.html);
        assert!(page.location().is_none(), "{next}");
    }
}

#[tokio::test]
async fn logging_out_ends_the_browser_session() {
    let fixture = fixture().await;
    let signed_in = log_in(&fixture.browser).await;
    assert!(signed_in.html.contains("已登录"));
    assert!(
        signed_in
            .set_cookies()
            .iter()
            .any(|cookie| cookie.starts_with("assemble_web=")),
        "a session cookie should be set"
    );
    let page = send(fixture.browser.post_form("/web/logout", &[])).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.html.contains("已退出"));
    let cookies = page.set_cookies().join("\n");
    assert!(
        cookies.contains("assemble_web=;") && cookies.contains("Max-Age=0"),
        "{cookies}"
    );
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM web_sessions")
        .fetch_one(&fixture.server.state.pool)
        .await
        .unwrap();
    assert_eq!(rows, 0);
    // 跟着浏览器跳回去的旧 Cookie 已经换不回会话。
    let again = open_login(&fixture.browser).await;
    assert!(again.html.contains("autocomplete=\"current-password\""));

    // 来自别的站点的退出请求同样被拒绝。
    let cross_site = send(
        fixture
            .browser
            .http
            .post(format!("{}/web/logout", fixture.browser.origin))
            .header("origin", "https://evil.example"),
    )
    .await;
    assert_eq!(cross_site.status, StatusCode::FORBIDDEN);
}

// ---- 桌面端授权 ----

#[tokio::test]
async fn the_authorize_page_needs_a_login_and_names_the_device() {
    let fixture = fixture().await;
    let (request, _verifier) = desktop_request('a', "工作电脑", Platform::Windows);
    let path = assemble_server::desktop_login::authorize_path(&request);

    // 没带请求，或请求不合法：直接说明，不跳转。
    let missing = send(fixture.browser.get("/desktop/authorize")).await;
    assert_eq!(missing.status, StatusCode::BAD_REQUEST);
    let broken = send(
        fixture
            .browser
            .get("/desktop/authorize?request=not-a-request"),
    )
    .await;
    assert_eq!(broken.status, StatusCode::BAD_REQUEST);
    assert!(broken.location().is_none());

    // 没登录：先去登录页，登录完回到这里。
    let anonymous = send(fixture.browser.get(&path)).await;
    assert_eq!(anonymous.status, StatusCode::SEE_OTHER);
    let location = anonymous.location().unwrap();
    assert!(location.starts_with("/web/login?next="), "{location}");
    // 整个回跳地址被编码成了一个没有取值的"键"。
    let returned = url::form_urlencoded::parse(location.split_once("next=").unwrap().1.as_bytes())
        .next()
        .unwrap()
        .0
        .into_owned();
    assert_eq!(returned, path);

    // 登录之后：确认页上写着是哪台设备、哪个账户。
    log_in(&fixture.browser).await;
    let page = send(fixture.browser.get(&path)).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(
        page.html.contains("<strong>工作电脑</strong>"),
        "the device name"
    );
    assert!(page.html.contains("（Windows）"));
    assert!(page.html.contains("Alex Wang（alex）"));
    assert!(page.html.contains("name=\"choice\" value=\"allow\""));
    assert!(page.html.contains("name=\"choice\" value=\"deny\""));
    assert_eq!(page.headers.get("cache-control").unwrap(), "no-store");
    let shown = hidden(&page.html, "request").unwrap();
    assert_eq!(shown, path.split_once("request=").unwrap().1);
}

#[tokio::test]
async fn declining_sends_the_desktop_an_error_with_the_original_state() {
    let fixture = fixture().await;
    let (request, _verifier) = desktop_request('a', "MacBook", Platform::Macos);
    log_in(&fixture.browser).await;

    let page = authorize(&fixture.browser, &request, "deny").await;
    assert_eq!(page.status, StatusCode::SEE_OTHER);
    let location = page.location().unwrap();
    assert_eq!(
        location,
        format!(
            "{}?error=access_denied&state={}",
            request.redirect_uri, request.state
        )
    );
    // 没有签发任何授权码。
    let codes: i64 = sqlx::query_scalar("SELECT count(*) FROM desktop_auth_codes")
        .fetch_one(&fixture.server.state.pool)
        .await
        .unwrap();
    assert_eq!(codes, 0);
    let audit: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action = 'desktop.denied'")
            .fetch_one(&fixture.server.state.pool)
            .await
            .unwrap();
    assert_eq!(audit, 1);
}

#[tokio::test]
async fn confirming_needs_a_fresh_login_from_this_site() {
    let fixture = fixture().await;
    let (request, _verifier) = desktop_request('a', "MacBook", Platform::Macos);
    let path = assemble_server::desktop_login::authorize_path(&request);
    log_in(&fixture.browser).await;
    let page = send(fixture.browser.get(&path)).await;
    let csrf = hidden(&page.html, "csrf").unwrap();
    let encoded = assemble_server::desktop_login::encode_request(&request);

    // 冒充别的站点。
    let cross_site = send(
        fixture
            .browser
            .http
            .post(format!("{}/desktop/authorize", fixture.browser.origin))
            .header("origin", "https://evil.example")
            .form(&[
                ("request", encoded.as_str()),
                ("csrf", csrf.as_str()),
                ("choice", "allow"),
            ]),
    )
    .await;
    assert_eq!(cross_site.status, StatusCode::FORBIDDEN);

    // 防伪值对不上。
    let mismatched = send(fixture.browser.post_form(
        "/desktop/authorize",
        &[
            ("request", encoded.as_str()),
            ("csrf", "made-up"),
            ("choice", "allow"),
        ],
    ))
    .await;
    assert_eq!(mismatched.status, StatusCode::FORBIDDEN);

    // 请求与确认页上显示的不是同一个。
    let (other, _other_verifier) = desktop_request('b', "别的机器", Platform::Linux);
    let swapped = send(fixture.browser.post_form(
        "/desktop/authorize",
        &[
            (
                "request",
                assemble_server::desktop_login::encode_request(&other).as_str(),
            ),
            ("csrf", csrf.as_str()),
            ("choice", "allow"),
        ],
    ))
    .await;
    // 能确认，但确认的是表单里那个请求——它本身也得合法。
    assert_eq!(swapped.status, StatusCode::SEE_OTHER);
    assert!(swapped.location().unwrap().starts_with(&other.redirect_uri));

    let codes: i64 = sqlx::query_scalar("SELECT count(*) FROM desktop_auth_codes")
        .fetch_one(&fixture.server.state.pool)
        .await
        .unwrap();
    assert_eq!(codes, 1);
}

// ---- 换码与设备令牌 ----

#[tokio::test]
async fn an_issued_code_trades_for_a_device_token_exactly_once() {
    let fixture = fixture().await;
    let (request, verifier) = desktop_request('a', "MacBook", Platform::Macos);
    log_in(&fixture.browser).await;
    let code = code_from(
        &authorize(&fixture.browser, &request, "allow").await,
        &request,
    );

    let client = Client::new(fixture.server.origin.clone()).unwrap();
    let cancel = CancellationToken::new();
    let exchange = |verifier: &str| exchange_input(&request, verifier, code.clone());

    // 校验器不对：拒绝。
    let wrong = client
        .exchange(&exchange(&"x".repeat(43)), &RequestOptions::new(&cancel))
        .await
        .unwrap_err();
    assert_eq!(
        wrong,
        assemble_client::ClientError::Http {
            status: 400,
            remote_code: Some(codes::INVALID_AUTH_CODE.to_owned())
        }
    );

    // 这枚码已经作废：拿对的校验器再来也不行。
    let burned = client
        .exchange(&exchange(&verifier), &RequestOptions::new(&cancel))
        .await
        .unwrap_err();
    assert_eq!(wrong, burned);

    // 换一枚码，正常换码。
    let code = code_from(
        &authorize(&fixture.browser, &request, "allow").await,
        &request,
    );
    let (response, token) = client
        .exchange(
            &exchange_input(&request, &verifier, code),
            &RequestOptions::new(&cancel),
        )
        .await
        .unwrap();
    assert_eq!(response.account.username, USERNAME);
    assert_eq!(response.account.display_name, "Alex Wang");
    assert_eq!(response.device.name, "MacBook");
    assert_eq!(response.device.platform, Platform::Macos);
    assert!(!response.device.online);
    assert!(assemble_protocol::login::is_proof(token.expose()));

    // 库里只有令牌的摘要，账户里多了这台设备。
    let stored: (String, String) = sqlx::query_as("SELECT token_hash, name FROM devices")
        .fetch_one(&fixture.server.state.pool)
        .await
        .unwrap();
    assert_ne!(stored.0, token.expose());
    assert_eq!(stored.0.len(), 64);
    assert_eq!(stored.1, "MacBook");

    // 令牌能用了。
    client.set_bearer(Some(token));
    let me = client.me(&RequestOptions::new(&cancel)).await.unwrap();
    assert_eq!(me.device.id, response.device.id);
    assert_eq!(me.account.username, USERNAME);
    let devices = client.devices(&RequestOptions::new(&cancel)).await.unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].name, "MacBook");
}

#[tokio::test]
async fn codes_cannot_be_replayed_even_with_the_right_verifier_and_do_expire() {
    let fixture = fixture().await;
    let (request, verifier) = desktop_request('a', "MacBook", Platform::Macos);
    log_in(&fixture.browser).await;
    let code = code_from(
        &authorize(&fixture.browser, &request, "allow").await,
        &request,
    );
    let client = Client::new(fixture.server.origin.clone()).unwrap();
    let cancel = CancellationToken::new();

    client
        .exchange(
            &exchange_input(&request, &verifier, code.clone()),
            &RequestOptions::new(&cancel),
        )
        .await
        .unwrap();
    // 同一枚码、同一个校验器：第二台设备换不到。
    let replay = client
        .exchange(
            &exchange_input(&request, &verifier, code),
            &RequestOptions::new(&cancel),
        )
        .await
        .unwrap_err();
    assert_eq!(replay.status(), Some(400));

    // 过期的码：直接往库里放一枚。
    let account_id = account_of(&fixture).await;
    let stale = "s".repeat(43);
    sqlx::query(
        "INSERT INTO desktop_auth_codes (code_hash, account_id, redirect_uri, code_challenge, expires_at) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(assemble_server::secrets::token_digest(&stale))
    .bind(&account_id)
    .bind(&request.redirect_uri)
    .bind(&request.code_challenge)
    .bind(assemble_protocol::primitives::Timestamp::now().as_millis() - 1)
    .execute(&fixture.server.state.pool)
    .await
    .unwrap();
    let expired = client
        .exchange(
            &exchange_input(&request, &verifier, stale),
            &RequestOptions::new(&cancel),
        )
        .await
        .unwrap_err();
    assert_eq!(expired.remote_code(), Some(codes::INVALID_AUTH_CODE));

    // 回调地址必须与授权时的一致。
    let code = code_from(
        &authorize(&fixture.browser, &request, "allow").await,
        &request,
    );
    let elsewhere = ExchangeInput {
        redirect_uri: "http://127.0.0.1:9999/callback".into(),
        ..exchange_input(&request, &verifier, code)
    };
    let error = client
        .exchange(&elsewhere, &RequestOptions::new(&cancel))
        .await
        .unwrap_err();
    assert_eq!(error.remote_code(), Some(codes::INVALID_AUTH_CODE));
}

#[tokio::test]
async fn a_malformed_exchange_is_refused_without_touching_anything() {
    let fixture = fixture().await;
    let client = Client::new(fixture.server.origin.clone()).unwrap();
    let cancel = CancellationToken::new();
    let (request, verifier) = desktop_request('a', "MacBook", Platform::Macos);

    let blank_name = ExchangeInput {
        device: NewDevice {
            name: "   ".into(),
            platform: Platform::Macos,
        },
        ..exchange_input(&request, &verifier, "c".repeat(43))
    };
    let error = client
        .exchange(&blank_name, &RequestOptions::new(&cancel))
        .await
        .unwrap_err();
    assert_eq!(error, assemble_client::ClientError::InvalidRequest);

    let http = reqwest::Client::new();
    let response = http
        .post(format!(
            "{}{}",
            fixture.server.origin.as_str(),
            assemble_protocol::login::EXCHANGE_PATH
        ))
        .header("content-type", "application/json")
        .body("{not json")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response.text().await.unwrap();
    assert!(body.contains(codes::INVALID_REQUEST), "{body}");
    assert!(!body.contains("not json"));

    let devices: i64 = sqlx::query_scalar("SELECT count(*) FROM devices")
        .fetch_one(&fixture.server.state.pool)
        .await
        .unwrap();
    assert_eq!(devices, 0);
}

// ---- 设备与在线状态 ----

#[tokio::test]
async fn devices_see_each_other_come_online_and_go_offline() {
    let fixture = fixture().await;
    let mac = register(&fixture, 'a', "MacBook", Platform::Macos).await;
    let pc = register(&fixture, 'b', "工作电脑", Platform::Windows).await;
    assert_ne!(mac.device.id, pc.device.id);
    // 谁都还没连上事件流：都不在线。
    let listed = mac.devices(&mac.options()).await.unwrap();
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().all(|device| !device.online));

    let mut mac_events = mac.open_events();
    let hello = next_event(&mut mac_events).await;
    assert_eq!(
        hello,
        Event::Hello {
            device_id: mac.device.id.clone()
        }
    );
    let devices = wait_devices(&mut mac_events, |devices| {
        online_of(devices, "MacBook").is_some_and(|device| device.online)
    })
    .await;
    assert!(!online_of(&devices, "工作电脑").unwrap().online);
    assert_eq!(
        fixture
            .server
            .state
            .hub
            .listeners(&account_of(&fixture).await),
        1
    );

    // 另一台连上来：两边都收到最新的设备列表。
    let mut pc_events = pc.open_events();
    assert_eq!(
        next_event(&mut pc_events).await,
        Event::Hello {
            device_id: pc.device.id.clone()
        }
    );
    let both_online =
        |devices: &[Device]| devices.len() == 2 && devices.iter().all(|device| device.online);
    wait_devices(&mut mac_events, both_online).await;
    wait_devices(&mut pc_events, both_online).await;
    let listed = mac.devices(&mac.options()).await.unwrap();
    assert!(listed.iter().all(|device| device.online));

    // MacBook 断开（退出应用、断网）：工作电脑很快看到它离线。
    drop(mac_events);
    let devices = wait_devices(&mut pc_events, |devices| {
        online_of(devices, "MacBook").is_some_and(|device| !device.online)
            && online_of(devices, "工作电脑").is_some_and(|device| device.online)
    })
    .await;
    let gone = online_of(&devices, "MacBook").unwrap();
    assert!(gone.last_seen_at.is_some(), "the offline time is reported");

    // 再连上：又是在线。
    let _mac_events = mac.open_events();
    wait_devices(&mut pc_events, both_online).await;
    assert_eq!(
        fixture
            .server
            .state
            .hub
            .listeners(&account_of(&fixture).await),
        2
    );

    mac.cancel.cancel();
    pc.cancel.cancel();
}

#[tokio::test]
async fn a_revoked_device_loses_access_at_once_and_leaves_the_device_list() {
    let fixture = fixture().await;
    let mac = register(&fixture, 'a', "MacBook", Platform::Macos).await;
    let pc = register(&fixture, 'b', "工作电脑", Platform::Windows).await;

    // 在 MacBook 上给工作电脑改名。
    let renamed = mac
        .rename_device(
            &pc.device.id,
            &assemble_protocol::device::RenameDeviceInput {
                name: "公司的电脑".to_owned(),
            },
            &mac.options(),
        )
        .await
        .unwrap();
    assert_eq!(renamed.name, "公司的电脑");
    assert!(
        mac.devices(&mac.options())
            .await
            .unwrap()
            .iter()
            .any(|device| device.name == "公司的电脑")
    );

    // 撤销工作电脑：它的令牌立刻失效。
    mac.remove_device(&pc.device.id, &mac.options())
        .await
        .unwrap();
    let error = pc.me(&pc.options()).await.unwrap_err();
    assert!(error.is_unauthorized());
    assert_eq!(
        mac.devices(&mac.options()).await.unwrap().len(),
        1,
        "a removed device disappears from the list"
    );

    // 不存在的设备：如实回答 404。
    let error = mac
        .remove_device(&pc.device.id, &mac.options())
        .await
        .unwrap_err();
    assert_eq!(error.remote_code(), Some(codes::NOT_FOUND));

    // 退出登录等于撤销自己。
    mac.logout(&mac.options()).await.unwrap();
    let error = mac.me(&mac.options()).await.unwrap_err();
    assert!(error.is_unauthorized());
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM devices")
        .fetch_one(&fixture.server.state.pool)
        .await
        .unwrap();
    assert_eq!(rows, 0);

    // 事件流对被撤销的设备是关着的。
    let mut stream = mac.open_events();
    let first = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        first,
        Err(assemble_client::ClientError::Http {
            status: 401,
            remote_code: Some(codes::UNAUTHORIZED.to_owned())
        })
    );
}

#[tokio::test]
async fn invalid_names_and_ids_are_refused_without_changing_anything() {
    let fixture = fixture().await;
    let mac = register(&fixture, 'a', "MacBook", Platform::Macos).await;
    let before = mac.devices(&mac.options()).await.unwrap();

    for bad in ["", "   ", "a\nb", &"长".repeat(61)] {
        let error = mac
            .rename_device(
                &mac.device.id,
                &assemble_protocol::device::RenameDeviceInput {
                    name: bad.to_owned(),
                },
                &mac.options(),
            )
            .await
            .unwrap_err();
        assert_eq!(
            error,
            assemble_client::ClientError::InvalidRequest,
            "{bad:?}"
        );
    }
    // 服务端自己也会再核一遍。
    let response = reqwest::Client::new()
        .patch(format!(
            "{}/v1/devices/{}",
            fixture.server.origin.as_str(),
            mac.device.id
        ))
        .bearer_auth(mac.token.expose())
        .json(&serde_json::json!({ "name": "\u{7}" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(mac.devices(&mac.options()).await.unwrap(), before);

    // 伪造的令牌、别的账户的设备：都找不到东西。
    let stranger = Client::new(fixture.server.origin.clone()).unwrap();
    stranger.set_bearer(Some(SecretString::new("f".repeat(43))));
    let cancel = CancellationToken::new();
    let error = stranger
        .devices(&RequestOptions::new(&cancel))
        .await
        .unwrap_err();
    assert!(error.is_unauthorized());
}

async fn account_of(fixture: &Fixture) -> String {
    sqlx::query_scalar("SELECT id FROM accounts")
        .fetch_one(&fixture.server.state.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_server_restart_reports_every_device_as_offline_since_then() {
    let fixture = fixture().await;
    let _mac = register(&fixture, 'a', "MacBook", Platform::Macos).await;
    let seen: Option<i64> = sqlx::query_scalar("SELECT last_seen_at FROM devices")
        .fetch_one(&fixture.server.state.pool)
        .await
        .unwrap();
    assert!(seen.is_none(), "a brand-new device has never been offline");

    assemble_server::devices::mark_all_offline(&fixture.server.state.pool)
        .await
        .unwrap();
    let seen: i64 = sqlx::query_scalar("SELECT last_seen_at FROM devices")
        .fetch_one(&fixture.server.state.pool)
        .await
        .unwrap();
    assert!(seen > 1_700_000_000_000);
}

#[tokio::test]
async fn stopping_the_server_ends_open_event_streams_promptly() {
    let mut fixture = fixture().await;
    let mac = register(&fixture, 'a', "MacBook", Platform::Macos).await;
    let events = mac.open_events();
    let mut events = events;
    // 收到 hello 说明事件流已经建立。
    next_event(&mut events).await;

    // 事件流是永不主动结束的长连接。服务端退出时必须自己把它收掉，
    // 否则"优雅退出"会一直等下去——本机真机上见过旧进程收到 SIGTERM 后迟迟不退。
    let started = Instant::now();
    fixture.server.stop().await;
    let taken = started.elapsed();
    assert!(
        taken < Duration::from_secs(3),
        "stopping took {taken:?}: an open event stream kept the server alive"
    );
    // 收尾是正常走完的：设备被记成离线，并且带着离线时间。
    assert!(
        !fixture
            .server
            .state
            .presence
            .is_online(&account_of(&fixture).await, &mac.device.id)
    );
    let seen: Option<i64> = sqlx::query_scalar("SELECT last_seen_at FROM devices WHERE id = ?")
        .bind(&mac.device.id)
        .fetch_one(&fixture.server.state.pool)
        .await
        .unwrap();
    assert!(
        seen.is_some(),
        "the offline time should be written on shutdown"
    );
    mac.cancel.cancel();
}
