//! 服务端的集成测试：邀请码注册。对着真实运行的服务走一遍。
//!
//! 核对的是几道防线拼在一起之后是不是真的挡得住：没配邀请码就没有注册页；
//! 邀请码不对建不出账户；连续错太多次谁也注册不了；填对了就算登录，接着去授权确认页。

mod common;

use assemble_client::{Client, RequestOptions};
use assemble_protocol::primitives::Platform;
use assemble_store::secret::SecretString;
use common::*;
use reqwest::StatusCode;

const INVITE: &str = "welcome-to-assemble";
/// 与登录失败共用一张表：15 分钟内最多错 5 次。
const MAX_FAILURES: usize = 5;

async fn account_count(server: &assemble_server::http::TestServer) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM accounts")
        .fetch_one(&server.state.pool)
        .await
        .unwrap()
}

/// 配了邀请码、还没有任何账户的服务。
async fn open_registration() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let pool = assemble_server::db::open(dir.path()).await.unwrap();
    let config = assemble_server::Config::development(dir.path()).with_invite_code(INVITE);
    let server = assemble_server::http::TestServer::start(config, pool)
        .await
        .unwrap();
    let browser = Browser::new(server.origin.as_str());
    Fixture {
        dir,
        server,
        browser,
    }
}

async fn audit_actions(server: &assemble_server::http::TestServer) -> Vec<String> {
    sqlx::query_scalar("SELECT action FROM audit_log ORDER BY id")
        .fetch_all(&server.state.pool)
        .await
        .unwrap()
}

/// 先取注册页，再用页面上的防伪值提交一次。`display_name` 固定为 Alex Wang。
async fn try_register(
    browser: &Browser,
    username: &str,
    password: &str,
    password2: &str,
    invite: &str,
    next: &str,
) -> Page {
    let form = send(browser.get("/web/register")).await;
    assert_eq!(form.status, StatusCode::OK, "the register page should open");
    let csrf = hidden(&form.html, "csrf").unwrap();
    send(browser.post_form(
        "/web/register",
        &[
            ("username", username),
            ("display_name", "Alex Wang"),
            ("password", password),
            ("password2", password2),
            ("invite", invite),
            ("next", next),
            ("csrf", &csrf),
        ],
    ))
    .await
}

#[tokio::test]
async fn there_is_no_registration_unless_an_invite_code_is_configured() {
    let fixture = fixture().await;
    let page = send(fixture.browser.get("/web/register")).await;
    assert_eq!(page.status, StatusCode::NOT_FOUND);
    assert!(page.html.contains("没有开放注册"));
    // 硬提交也不行：连表单都不认。
    let submitted = send(fixture.browser.post_form(
        "/web/register",
        &[("username", "sam"), ("password", PASSWORD)],
    ))
    .await;
    assert_eq!(submitted.status, StatusCode::NOT_FOUND);
    // 登录页上也没有去注册的入口。
    let login = open_login(&fixture.browser).await;
    assert!(!login.html.contains("/web/register"));
    assert_eq!(account_count(&fixture.server).await, 1);
}

#[tokio::test]
async fn the_register_page_is_paired_with_its_cookie_and_linked_from_the_login_page() {
    let fixture = open_registration().await;
    let login = open_login(&fixture.browser).await;
    assert!(
        login.html.contains("href=\"/web/register\""),
        "{}",
        login.html
    );
    // 登录页带着"之后要回去"的参数时，注册的入口也带着。
    let login = send(
        fixture
            .browser
            .get("/web/login?next=%2Fdesktop%2Fauthorize"),
    )
    .await;
    assert!(
        login
            .html
            .contains("href=\"/web/register?next=%2Fdesktop%2Fauthorize\"")
    );

    let page = send(fixture.browser.get("/web/register")).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.html.contains("<title>注册 · Assemble</title>"));
    assert!(page.html.contains("action=\"/web/register\""));
    assert!(page.html.contains("autocomplete=\"new-password\""));
    assert!(!page.html.contains("<script"));
    // 与登录页同一套头：不缓存、样式只认哈希、表单只发往本站。
    assert_eq!(page.headers.get("cache-control").unwrap(), "no-store");
    let policy = page.headers.get("content-security-policy").unwrap();
    assert!(policy.to_str().unwrap().contains("form-action 'self'"));
    // 防伪值与 Cookie 成对；页面与 Cookie 里都没有邀请码。
    let csrf = hidden(&page.html, "csrf").unwrap();
    let cookies = page.set_cookies().join("\n");
    assert!(
        cookies.contains(&format!("assemble_csrf={csrf}")),
        "{cookies}"
    );
    assert!(cookies.contains("HttpOnly"));
    assert!(!page.html.contains(INVITE));
    assert!(!cookies.contains(INVITE));
    // "之后要回去"的参数只认本站的授权页；别处的地址被丢掉。
    let hostile = send(
        fixture
            .browser
            .get("/web/register?next=https%3A%2F%2Fevil.example%2F"),
    )
    .await;
    assert_eq!(hostile.status, StatusCode::OK);
    assert_eq!(hidden(&hostile.html, "next").as_deref(), Some(""));
    let kept = send(
        fixture
            .browser
            .get("/web/register?next=%2Fdesktop%2Fauthorize%3Frequest%3Dabc"),
    )
    .await;
    assert_eq!(
        hidden(&kept.html, "next").as_deref(),
        Some("/desktop/authorize?request=abc")
    );
    assert_eq!(account_count(&fixture.server).await, 0);
}

#[tokio::test]
async fn registration_refuses_foreign_sites_forged_forms_and_wrong_invite_codes() {
    let fixture = open_registration().await;
    let browser = &fixture.browser;
    let form = send(browser.get("/web/register")).await;
    let csrf = hidden(&form.html, "csrf").unwrap();
    let fields = [
        ("username", "sam"),
        ("display_name", "Sam Lee"),
        ("password", PASSWORD),
        ("password2", PASSWORD),
        ("invite", INVITE),
        ("next", ""),
        ("csrf", csrf.as_str()),
    ];

    // 没有 Origin（不是浏览器从本站页面发出的）。
    let no_origin = send(
        browser
            .http
            .post(format!("{}/web/register", browser.origin))
            .form(&fields),
    )
    .await;
    assert_eq!(no_origin.status, StatusCode::FORBIDDEN);
    // 冒充别的站点。
    let forged = send(
        browser
            .http
            .post(format!("{}/web/register", browser.origin))
            .header("origin", "https://evil.example")
            .form(&fields),
    )
    .await;
    assert_eq!(forged.status, StatusCode::FORBIDDEN);
    // 防伪值对不上（别的站点自己拼的表单）。
    let mut mismatched = fields;
    mismatched[6] = ("csrf", "not-the-paired-value");
    let forged_form = send(browser.post_form("/web/register", &mismatched)).await;
    assert_eq!(forged_form.status, StatusCode::FORBIDDEN);
    assert_eq!(account_count(&fixture.server).await, 0);

    // 邀请码不对：只有一句话，账户没有建出来。
    let wrong = try_register(browser, "sam", PASSWORD, PASSWORD, "not-the-invite", "").await;
    assert_eq!(wrong.status, StatusCode::FORBIDDEN);
    assert!(wrong.html.contains("邀请码不对"));
    assert!(!wrong.html.contains("not-the-invite"));
    // 用户名与显示名摆回去了，密码与邀请码没有。
    assert!(wrong.html.contains("name=\"username\" value=\"sam\""));
    assert!(
        wrong
            .html
            .contains("name=\"display_name\" value=\"Alex Wang\"")
    );
    assert!(!wrong.html.contains(PASSWORD));
    assert!(
        wrong
            .set_cookies()
            .iter()
            .all(|cookie| !cookie.starts_with("assemble_web="))
    );
    assert_eq!(account_count(&fixture.server).await, 0);
    assert_eq!(
        audit_actions(&fixture.server).await,
        ["web.register_failed"]
    );

    // 邀请码对了，别的填错：各自说各自的，账户还是没有建出来。
    let mismatch = try_register(browser, "sam", PASSWORD, "something-else", INVITE, "").await;
    assert_eq!(mismatch.status, StatusCode::BAD_REQUEST);
    assert!(mismatch.html.contains("两次输入的密码不一致"));
    let short = try_register(browser, "sam", "short", "short", INVITE, "").await;
    assert_eq!(short.status, StatusCode::BAD_REQUEST);
    assert!(short.html.contains("密码至少 8 个字符"));
    let bad_name = try_register(browser, "sam!", PASSWORD, PASSWORD, INVITE, "").await;
    assert_eq!(bad_name.status, StatusCode::BAD_REQUEST);
    assert!(bad_name.html.contains("用户名不合法"));
    // 这些失败不计入邀请码的失败次数：下面再错几次仍然是从零数。
    assemble_server::accounts::create(
        &fixture.server.state.pool,
        "taken",
        "",
        SecretString::new(PASSWORD.to_owned()),
    )
    .await
    .unwrap();
    let taken = try_register(browser, "taken", PASSWORD, PASSWORD, INVITE, "").await;
    assert_eq!(taken.status, StatusCode::CONFLICT);
    assert!(taken.html.contains("这个用户名已经被占用了"));
    assert_eq!(account_count(&fixture.server).await, 1);
    // 这些原因不是"邀请码错了"，不记那一笔：审计里还是只有邀请码错的那一次。
    assert_eq!(
        audit_actions(&fixture.server).await,
        ["web.register_failed"]
    );
}

#[tokio::test]
async fn a_valid_registration_signs_you_in_and_carries_on_to_the_device() {
    let fixture = open_registration().await;
    let browser = &fixture.browser;
    let (request, verifier) = desktop_request('r', "MacBook", Platform::Macos);
    let next = assemble_server::desktop_login::authorize_path(&request);

    let page = try_register(browser, "Alex", PASSWORD, PASSWORD, INVITE, &next).await;
    assert_eq!(page.status, StatusCode::SEE_OTHER, "{}", page.html);
    // 回到原来要去的授权确认页。
    assert_eq!(page.location().as_deref(), Some(next.as_str()));
    let cookies = page.set_cookies().join("\n");
    assert!(cookies.contains("assemble_web="), "{cookies}");
    // 注册表单的防伪值用完即弃。
    assert!(cookies.contains("assemble_csrf=;"), "{cookies}");
    assert!(cookies.contains("Max-Age=0"), "{cookies}");
    assert!(!page.html.contains(INVITE));

    let account = assemble_server::accounts::find_by_username(&fixture.server.state.pool, "alex")
        .await
        .unwrap()
        .expect("the account was created");
    assert_eq!(account.display_name, "Alex Wang");
    assert!(account.password_hash.starts_with("$argon2id$"));
    assert_eq!(audit_actions(&fixture.server).await, ["account.registered"]);

    // 已经是登录状态：直接在确认页上点"确认登录"，桌面端换到令牌。
    let code = code_from(&authorize(browser, &request, "allow").await, &request);
    let client = Client::new(fixture.server.origin.clone()).unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    let (response, _token) = client
        .exchange(
            &exchange_input(&request, &verifier, code),
            &RequestOptions::new(&cancel),
        )
        .await
        .unwrap();
    assert_eq!(response.account.username, "alex");
    assert_eq!(response.account.id, account.id);
    assert_eq!(response.device.name, "MacBook");
}

#[tokio::test]
async fn without_a_destination_a_registration_ends_signed_in() {
    let fixture = open_registration().await;
    let page = try_register(&fixture.browser, "sam", PASSWORD, PASSWORD, INVITE, "").await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.html.contains("注册成功"));
    assert!(page.html.contains("已经帮你登录"));
    assert!(
        page.set_cookies()
            .iter()
            .any(|c| c.starts_with("assemble_web="))
    );

    // 已登录的浏览器再去注册页：不会又摆一张表单，而是回到登录页（那里会显示"已登录"）。
    let again = send(fixture.browser.get("/web/register")).await;
    assert_eq!(again.status, StatusCode::SEE_OTHER);
    assert_eq!(again.location().as_deref(), Some("/web/login"));
    let (request, _verifier) = desktop_request('x', "MacBook", Platform::Macos);
    let next = assemble_server::desktop_login::authorize_path(&request);
    let carried = send(
        fixture
            .browser
            .get(&format!("/web/register?next={}", encode(&next))),
    )
    .await;
    assert_eq!(carried.status, StatusCode::SEE_OTHER);
    assert_eq!(carried.location().as_deref(), Some(next.as_str()));
    assert_eq!(account_count(&fixture.server).await, 1);
}

/// 查询串里的地址要转义。
fn encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

#[tokio::test]
async fn wrong_invite_codes_are_throttled_for_everyone() {
    let fixture = open_registration().await;
    for attempt in 1..=MAX_FAILURES {
        let page = try_register(
            &fixture.browser,
            "sam",
            PASSWORD,
            PASSWORD,
            &format!("wrong-number-{attempt}"),
            "",
        )
        .await;
        assert_eq!(page.status, StatusCode::FORBIDDEN, "{attempt}");
        assert_eq!(page.headers.get("retry-after").unwrap(), "1");
    }
    // 次数用完：这回邀请码是对的也不收。
    let blocked = try_register(&fixture.browser, "sam", PASSWORD, PASSWORD, INVITE, "").await;
    assert_eq!(blocked.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(blocked.html.contains("尝试次数太多"));
    assert_eq!(blocked.headers.get("retry-after").unwrap(), "900");
    assert_eq!(account_count(&fixture.server).await, 0);
    // 另一个"人"（不带任何 Cookie 的浏览器）同样被挡：次数是全局的。
    let stranger = Browser::new(fixture.server.origin.as_str());
    let also = try_register(&stranger, "jo", PASSWORD, PASSWORD, INVITE, "").await;
    assert_eq!(also.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(account_count(&fixture.server).await, 0);
}
