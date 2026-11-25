//! 服务端的集成测试：账户页上改密码。对着真实运行的服务走一遍。
//!
//! 核对的是：只有登录着的人能改；只有知道现在密码的人能改；改完之后别处的网页登录状态
//! 作废、旧密码不能再登录，而电脑上的 Assemble（设备令牌）不受影响；现在的密码不能被没完没了地猜。

mod common;

use assemble_protocol::primitives::Platform;
use common::*;
use reqwest::StatusCode;

const NEW_PASSWORD: &str = "a brand new passphrase";
/// 与登录失败共用一张表：15 分钟内最多错 5 次。
const MAX_FAILURES: usize = 5;

async fn open_account(browser: &Browser) -> Page {
    send(browser.get("/web/account")).await
}

/// 打开账户页，用页面上的防伪值提交一次改密码。
async fn change(browser: &Browser, current: &str, password: &str, password2: &str) -> Page {
    let page = open_account(browser).await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.html);
    let csrf = hidden(&page.html, "csrf").unwrap();
    send(browser.post_form(
        "/web/account/password",
        &[
            ("current", current),
            ("password", password),
            ("password2", password2),
            ("csrf", &csrf),
        ],
    ))
    .await
}

/// 一份填得对的改密码表单，只等防伪值。
fn valid_fields(csrf: &str) -> [(&'static str, &str); 4] {
    [
        ("current", PASSWORD),
        ("password", NEW_PASSWORD),
        ("password2", NEW_PASSWORD),
        ("csrf", csrf),
    ]
}

async fn audit_actions(fixture: &Fixture) -> Vec<String> {
    sqlx::query_scalar("SELECT action FROM audit_log ORDER BY id")
        .fetch_all(&fixture.server.state.pool)
        .await
        .unwrap()
}

async fn stored_hash(fixture: &Fixture) -> String {
    sqlx::query_scalar("SELECT password_hash FROM accounts")
        .fetch_one(&fixture.server.state.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn the_account_page_is_only_for_a_signed_in_browser() {
    let fixture = fixture().await;
    // 没登录：先去登录，登录完回到账户页。
    let away = send(fixture.browser.get("/web/account")).await;
    assert_eq!(away.status, StatusCode::SEE_OTHER);
    assert_eq!(
        away.location().as_deref(),
        Some("/web/login?next=%2Fweb%2Faccount")
    );
    let signed_in = try_login(&fixture.browser, USERNAME, PASSWORD, "/web/account").await;
    assert_eq!(signed_in.status, StatusCode::SEE_OTHER);
    assert_eq!(signed_in.location().as_deref(), Some("/web/account"));

    let page = open_account(&fixture.browser).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.html.contains("<title>账户 · Assemble</title>"));
    assert!(page.html.contains("主邮箱：alex@example.test"));
    assert!(page.html.contains("action=\"/web/account/password\""));
    assert!(page.html.contains("autocomplete=\"current-password\""));
    assert!(page.html.contains("action=\"/web/logout\""));
    assert!(!page.html.contains("<script"));
    assert_eq!(page.headers.get("cache-control").unwrap(), "no-store");
    // 页面上没有任何密码的痕迹。
    assert!(!page.html.contains("$argon2"));
    assert!(!page.html.contains(PASSWORD));

    // 直接打开提交地址：回到账户页，不是一句干巴巴的 405。
    let pointed = send(fixture.browser.get("/web/account/password")).await;
    assert_eq!(pointed.status, StatusCode::SEE_OTHER);
    assert_eq!(pointed.location().as_deref(), Some("/web/account"));
}

#[tokio::test]
async fn a_change_is_refused_without_a_session_a_same_origin_or_a_paired_token() {
    let fixture = fixture().await;
    log_in(&fixture.browser).await;
    let before = stored_hash(&fixture).await;
    let page = open_account(&fixture.browser).await;
    let csrf = hidden(&page.html, "csrf").unwrap();
    // 没有 Origin（不是浏览器从本站页面发出的）。
    let no_origin = send(
        fixture
            .browser
            .http
            .post(format!("{}/web/account/password", fixture.browser.origin))
            .form(&valid_fields(csrf.as_str())),
    )
    .await;
    assert_eq!(no_origin.status, StatusCode::FORBIDDEN);
    // 冒充别的站点。
    let forged_origin = send(
        fixture
            .browser
            .http
            .post(format!("{}/web/account/password", fixture.browser.origin))
            .header("origin", "https://evil.example")
            .form(&valid_fields(csrf.as_str())),
    )
    .await;
    assert_eq!(forged_origin.status, StatusCode::FORBIDDEN);
    // 防伪值对不上。
    let mismatched = send(fixture.browser.post_form(
        "/web/account/password",
        &valid_fields("not-the-paired-value"),
    ))
    .await;
    assert_eq!(mismatched.status, StatusCode::FORBIDDEN);
    // 没登录的浏览器，哪怕防伪值是对的也不行。
    let stranger = Browser::new(fixture.server.origin.as_str());
    let signed_out = send(stranger.post_form("/web/account/password", &valid_fields(&csrf))).await;
    assert_eq!(signed_out.status, StatusCode::FORBIDDEN);
    assert_eq!(stored_hash(&fixture).await, before);
    // 被挡在门外的请求不记"改密码失败"：审计里只有登录那一笔。
    assert_eq!(audit_actions(&fixture).await, ["web.login"]);
}

#[tokio::test]
async fn a_change_needs_the_right_current_password_and_a_usable_new_one() {
    let fixture = fixture().await;
    log_in(&fixture.browser).await;
    let before = stored_hash(&fixture).await;

    // 现在的密码不对。
    let wrong = change(
        &fixture.browser,
        "not-the-current-one",
        NEW_PASSWORD,
        NEW_PASSWORD,
    )
    .await;
    assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);
    assert!(wrong.html.contains("现在的密码不对"));
    assert!(!wrong.html.contains("not-the-current-one"));
    assert_eq!(wrong.headers.get("retry-after").unwrap(), "1");

    // 新密码填得不对。
    let mismatch = change(&fixture.browser, PASSWORD, NEW_PASSWORD, "something-else").await;
    assert_eq!(mismatch.status, StatusCode::BAD_REQUEST);
    assert!(mismatch.html.contains("两次输入的新密码不一致"));
    let same = change(&fixture.browser, PASSWORD, PASSWORD, PASSWORD).await;
    assert_eq!(same.status, StatusCode::BAD_REQUEST);
    assert!(same.html.contains("新密码不能和现在的密码一样"));
    let short = change(&fixture.browser, PASSWORD, "short", "short").await;
    assert_eq!(short.status, StatusCode::BAD_REQUEST);
    assert!(short.html.contains("密码至少 8 个字符"));

    // 哪一次都没改成：散列还是原来那个，页面上没有出现过任何一个密码。
    assert_eq!(stored_hash(&fixture).await, before);
    for page in [&wrong, &mismatch, &same, &short] {
        assert!(!page.html.contains(PASSWORD), "{}", page.html);
        assert!(!page.html.contains(NEW_PASSWORD), "{}", page.html);
    }
    assert_eq!(
        audit_actions(&fixture).await,
        ["web.login", "account.password_change_failed"]
    );
}

#[tokio::test]
async fn the_password_changes_only_for_someone_who_knows_the_current_one() {
    let fixture = fixture().await;
    log_in(&fixture.browser).await;
    // 别处：另一台浏览器登录着，另有一台桌面端。
    let other = Browser::new(fixture.server.origin.as_str());
    log_in(&other).await;
    let device = register(&fixture, 'p', "MacBook", Platform::Macos).await;
    let before = stored_hash(&fixture).await;

    // 改成功：跳回账户页，带着"刚改好"的标记（停在这个提交地址上的话，一刷新就会把
    // 旧密码再提交一遍）。
    let done = change(&fixture.browser, PASSWORD, NEW_PASSWORD, NEW_PASSWORD).await;
    assert_eq!(done.status, StatusCode::SEE_OTHER, "{}", done.html);
    assert_eq!(done.location().as_deref(), Some("/web/account?changed=1"));
    let landed = send(fixture.browser.get(done.location().unwrap().as_str())).await;
    assert_eq!(landed.status, StatusCode::OK);
    assert!(landed.html.contains("密码已改好"));
    assert!(!landed.html.contains("role=\"alert\""));
    // 平时打开账户页不带着这句话。
    assert!(
        !open_account(&fixture.browser)
            .await
            .html
            .contains("密码已改好")
    );
    // 跳转前后，页面上既没有旧密码也没有新密码。
    for page in [&done, &landed] {
        assert!(!page.html.contains(PASSWORD), "{}", page.html);
        assert!(!page.html.contains(NEW_PASSWORD), "{}", page.html);
    }
    let after = stored_hash(&fixture).await;
    assert_ne!(after, before);
    assert!(after.starts_with("$argon2id$"));
    assert_eq!(
        audit_actions(&fixture).await.last().unwrap(),
        "account.password_changed"
    );

    // 别处的网页登录状态作废；正在用的这一次还在。
    let kicked = open_account(&other).await;
    assert_eq!(kicked.status, StatusCode::SEE_OTHER);
    assert_eq!(
        kicked.location().as_deref(),
        Some("/web/login?next=%2Fweb%2Faccount")
    );
    assert_eq!(open_account(&fixture.browser).await.status, StatusCode::OK);

    // 旧密码不能再登录，新密码可以。
    let newcomer = Browser::new(fixture.server.origin.as_str());
    let stale = try_login(&newcomer, USERNAME, PASSWORD, "").await;
    assert_eq!(stale.status, StatusCode::UNAUTHORIZED);
    let fresh = try_login(&newcomer, USERNAME, NEW_PASSWORD, "").await;
    assert_eq!(fresh.status, StatusCode::OK);
    assert!(fresh.html.contains("已登录"));

    // 电脑上的 Assemble 用的是设备令牌：不受影响。
    let devices = device.devices(&device.options()).await.unwrap();
    assert!(devices.iter().any(|listed| listed.name == "MacBook"));
}

#[tokio::test]
async fn the_current_password_cannot_be_guessed_endlessly() {
    let fixture = fixture().await;
    log_in(&fixture.browser).await;
    let before = stored_hash(&fixture).await;
    for attempt in 1..=MAX_FAILURES {
        let page = change(
            &fixture.browser,
            &format!("wrong-current-{attempt}"),
            NEW_PASSWORD,
            NEW_PASSWORD,
        )
        .await;
        assert_eq!(page.status, StatusCode::UNAUTHORIZED, "{attempt}");
    }
    // 次数用完：这回现在的密码是对的也不收。
    let blocked = change(&fixture.browser, PASSWORD, NEW_PASSWORD, NEW_PASSWORD).await;
    assert_eq!(blocked.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(blocked.html.contains("尝试次数太多"));
    assert_eq!(blocked.headers.get("retry-after").unwrap(), "900");
    assert_eq!(stored_hash(&fixture).await, before);
    // 猜不出来的那几次也不放过登录：同一个账户名，15 分钟内登录也被挡。
    let elsewhere = Browser::new(fixture.server.origin.as_str());
    let login = try_login(&elsewhere, USERNAME, PASSWORD, "").await;
    assert_eq!(login.status, StatusCode::TOO_MANY_REQUESTS);
    let failures = audit_actions(&fixture)
        .await
        .into_iter()
        .filter(|action| action == "account.password_change_failed")
        .count();
    assert_eq!(failures, MAX_FAILURES);
}
