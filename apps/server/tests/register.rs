//! 真实 HTTP、数据库和 Cookie 串起邮箱与 OAuth；只有外部邮件与 GitHub 使用替身。

mod common;

use assemble_server::email_identity::{self as identity, Action};
use assemble_server::github::{Exchange, GithubConfig, Profile, Provider};
use assemble_server::http::TestServer;
use assemble_server::mail::{Delivery, Mail, Mailer};
use assemble_server::source::Source;
use assemble_server::{Config, SecretString};
use common::*;
use reqwest::StatusCode;
use std::sync::{Arc, Mutex};

#[derive(Debug, Default)]
struct Inbox(Mutex<Vec<Mail>>);
impl Mailer for Inbox {
    fn send(&self, mail: Mail) -> Delivery<'_> {
        self.0.lock().unwrap().push(mail);
        Box::pin(async { Ok(()) })
    }
}
impl Inbox {
    fn token(&self) -> String {
        let mail = self.0.lock().unwrap();
        mail.last()
            .unwrap()
            .body
            .expose()
            .split("#token=")
            .nth(1)
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .to_owned()
    }
    fn recipient(&self) -> String {
        self.0.lock().unwrap().last().unwrap().to.clone()
    }
}

#[derive(Debug)]
struct BrokenMailer;
impl Mailer for BrokenMailer {
    fn send(&self, _mail: Mail) -> Delivery<'_> {
        Box::pin(async { Err(assemble_server::mail::MailError) })
    }
}

#[derive(Debug)]
struct Github(Mutex<Profile>);
impl Provider for Github {
    fn exchange(
        &self,
        code: SecretString,
        verifier: SecretString,
        callback: String,
    ) -> Exchange<'_> {
        assert_eq!(code.expose(), "test-code");
        assert_eq!(verifier.expose().len(), 43);
        assert!(callback.ends_with("/web/github/callback"));
        let profile = self.0.lock().unwrap().clone();
        Box::pin(async { Ok(profile) })
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    server: TestServer,
    browser: Browser,
    inbox: Arc<Inbox>,
    github: Arc<Github>,
}
async fn setup() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let pool = assemble_server::db::open(dir.path()).await.unwrap();
    let inbox = Arc::new(Inbox::default());
    let github = Arc::new(Github(Mutex::new(Profile {
        id: "123".into(),
        login: "octocat".into(),
        name: "Octocat".into(),
        verified_emails: vec!["github@example.test".into()],
    })));
    let mut config = Config::development(dir.path());
    config.github = Some(GithubConfig {
        client_id: "test-client".into(),
        client_secret: SecretString::new("test-secret"),
    });
    let server = TestServer::start_with_services(config, pool, inbox.clone(), github.clone())
        .await
        .unwrap();
    let browser = Browser::new(server.origin.as_str());
    Fixture {
        _dir: dir,
        server,
        browser,
        inbox,
        github,
    }
}

async fn request(browser: &Browser, path: &str, email: &str, extra: &[(&str, &str)]) -> Page {
    let page = send(browser.get(path)).await;
    let csrf = hidden(&page.html, "csrf").unwrap();
    let mut fields = vec![("email", email), ("csrf", csrf.as_str())];
    fields.extend_from_slice(extra);
    send(browser.post_form(path, &fields)).await
}

async fn confirm(browser: &Browser, token: &str, password: &str) -> Page {
    let entry = send(browser.get("/web/verify")).await;
    let csrf = hidden(&entry.html, "csrf").unwrap();
    let form =
        send(browser.post_form("/web/verify/prepare", &[("token", token), ("csrf", &csrf)])).await;
    if form.status != StatusCode::OK {
        return form;
    }
    send(browser.post_form(
        "/web/verify",
        &[
            ("token", token),
            ("csrf", &csrf),
            ("password", password),
            ("password2", password),
            ("display_name", "Alex Wang"),
        ],
    ))
    .await
}

async fn count(f: &Fixture) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM accounts")
        .fetch_one(&f.server.state.pool)
        .await
        .unwrap()
}

async fn register(f: &Fixture, email: &str) -> String {
    assert_eq!(
        request(&f.browser, "/web/register", email, &[])
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        confirm(&f.browser, &f.inbox.token(), PASSWORD).await.status,
        StatusCode::SEE_OTHER
    );
    identity::by_email(&f.server.state.pool, email)
        .await
        .unwrap()
        .unwrap()
        .id
}

async fn github_callback(browser: &Browser) -> Page {
    let start = send(browser.get("/web/github/start")).await;
    let url = url::Url::parse(&start.location().unwrap()).unwrap();
    assert_eq!(url.host_str(), Some("github.com"));
    let pairs = url
        .query_pairs()
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(pairs.get("code_challenge_method").unwrap(), "S256");
    assert_eq!(pairs.get("code_challenge").unwrap().len(), 43);
    assert!(!url.as_str().contains("test-secret"));
    let state = pairs.get("state").unwrap();
    send(browser.get(&format!(
        "/web/github/callback?state={state}&code=test-code"
    )))
    .await
}

async fn github_profile(browser: &Browser, email: &str) -> Page {
    let page = send(browser.get("/web/github/profile")).await;
    let csrf = hidden(&page.html, "csrf").unwrap();
    send(browser.post_form(
        "/web/github/profile",
        &[
            ("email", email),
            ("display_name", "Octocat"),
            ("csrf", &csrf),
        ],
    ))
    .await
}

#[tokio::test]
async fn registration_is_public_and_no_account_exists_until_mail_is_verified() {
    let f = setup().await;
    let page = send(f.browser.get("/web/register")).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(!page.html.contains("invite"));
    assert!(!page.html.contains("type=\"password\""));
    assert_eq!(
        request(
            &f.browser,
            "/web/register",
            " Alex@Example.Test ",
            &[("next", "/web/account")]
        )
        .await
        .status,
        StatusCode::OK
    );
    assert_eq!(count(&f).await, 0);
    let token = f.inbox.token();
    let stored: String = sqlx::query_scalar("SELECT token_hash FROM email_actions")
        .fetch_one(&f.server.state.pool)
        .await
        .unwrap();
    assert_ne!(stored, token);
    let ignored = send(f.browser.get("/web/verify")).await;
    assert_eq!(ignored.status, StatusCode::OK);
    assert_eq!(count(&f).await, 0);
    let confirmed = confirm(&f.browser, &token, PASSWORD).await;
    assert_eq!(confirmed.status, StatusCode::SEE_OTHER);
    assert_eq!(count(&f).await, 1);
    let account = identity::by_email(&f.server.state.pool, "alex@example.test")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(account.email.as_deref(), Some("alex@example.test"));
    assert!(account.email_verified_at.is_some());
    assert!(account.password_hash.starts_with("$argon2id$"));
    assert!(account.avatar_url.is_none());
    assert_eq!(
        confirm(&f.browser, &token, PASSWORD).await.status,
        StatusCode::BAD_REQUEST
    );
    let browser = Browser::new(f.server.origin.as_str());
    assert_eq!(
        try_login(&browser, "ALEX@EXAMPLE.TEST", PASSWORD, "/web/account")
            .await
            .status,
        StatusCode::SEE_OTHER
    );
    let profile = send(browser.get("/web/account")).await;
    assert!(profile.html.contains("默认头像"));
    assert!(profile.html.contains("主邮箱：alex@example.test"));
    let other = Browser::new(f.server.origin.as_str());
    assert_eq!(
        try_login(&other, &account.username, PASSWORD, "")
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn requests_require_csrf_and_mail_limits_survive_concurrent_requests() {
    let f = setup().await;
    assert_eq!(
        send(f.browser.post_form(
            "/web/register",
            &[("email", "a@example.test"), ("csrf", "fake")]
        ))
        .await
        .status,
        StatusCode::FORBIDDEN
    );
    assert!(f.inbox.0.lock().unwrap().is_empty());
    assert_eq!(
        request(&f.browser, "/web/register", "not-an-email", &[])
            .await
            .status,
        StatusCode::BAD_REQUEST
    );
    let pool = &f.server.state.pool;
    let unknown = Source::unknown();
    let (first, second) = tokio::join!(
        identity::reserve_mail(pool, "one@example.test", &unknown),
        identity::reserve_mail(pool, "one@example.test", &unknown)
    );
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    assert!(matches!(
        identity::reserve_mail(pool, "one@example.test", &unknown).await,
        Err(identity::Error::RateLimited)
    ));
}

/// 一个来源已知的请求：请求头里带着代理记下的地址。
fn from(address: &'static str) -> Source {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("x-real-ip", axum::http::HeaderValue::from_static(address));
    let trusted = axum::http::HeaderName::from_static("x-real-ip");
    Source::read(&headers, Some(&trusted))
}

/// 换着邮箱提交，用不掉别人的发信份额：来源已知时，每个来源每小时也最多五封。
/// 不然谁都能让全站一小时收不到验证邮件。
#[tokio::test]
async fn one_source_cannot_use_up_the_mail_budget_of_everyone_else() {
    let f = setup().await;
    let pool = &f.server.state.pool;
    let stranger = from("203.0.113.7");
    let myself = from("198.51.100.9");
    for index in 0..5 {
        identity::reserve_mail(pool, &format!("stranger-{index}@example.test"), &stranger)
            .await
            .unwrap();
    }
    assert!(matches!(
        identity::reserve_mail(pool, "stranger-5@example.test", &stranger).await,
        Err(identity::Error::RateLimited)
    ));
    // 被拦的那一次不占全站的份额：别的来源照常，全站这一小时只少了六封。
    identity::reserve_mail(pool, "mine@example.test", &myself)
        .await
        .unwrap();
    let used: i64 =
        sqlx::query_scalar("SELECT requests FROM email_rate_limits WHERE bucket = 'global'")
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(used, 6);
    // 来源未知时不设这一项：只按邮箱各算各的。
    let unknown = Source::unknown();
    for index in 0..10 {
        identity::reserve_mail(pool, &format!("unknown-{index}@example.test"), &unknown)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn a_verification_is_single_use_even_with_concurrent_submission_and_expiry() {
    let f = setup().await;
    let action = Action::new("register", "new@example.test".into(), String::new());
    let token = identity::issue(&f.server.state.pool, &action)
        .await
        .unwrap();
    let password = || Some(SecretString::new(PASSWORD));
    let (one, two) = tokio::join!(
        identity::complete(
            &f.server.state.pool,
            token.expose(),
            password(),
            "Alex",
            None
        ),
        identity::complete(
            &f.server.state.pool,
            token.expose(),
            password(),
            "Alex",
            None
        )
    );
    assert_eq!(usize::from(one.is_ok()) + usize::from(two.is_ok()), 1);
    assert_eq!(count(&f).await, 1);
    let mut expired = Action::new("register", "expired@example.test".into(), String::new());
    expired.expires_at = 1;
    let token = identity::issue(&f.server.state.pool, &expired)
        .await
        .unwrap();
    assert!(matches!(
        identity::inspect(&f.server.state.pool, token.expose()).await,
        Err(identity::Error::Invalid)
    ));
}

#[tokio::test]
async fn recovery_revokes_web_sessions_and_cannot_overwrite_a_later_password_change() {
    let f = setup().await;
    let id = register(&f, "recover@example.test").await;
    let account = assemble_server::accounts::find_by_id(&f.server.state.pool, &id)
        .await
        .unwrap()
        .unwrap();
    let action =
        Action::new("reset", "recover@example.test".into(), String::new()).for_account(&account);
    let stale = identity::issue(&f.server.state.pool, &action)
        .await
        .unwrap();
    assemble_server::accounts::set_password(
        &f.server.state.pool,
        &id,
        SecretString::new("intermediate-password"),
    )
    .await
    .unwrap();
    assert!(matches!(
        identity::complete(
            &f.server.state.pool,
            stale.expose(),
            Some(SecretString::new(PASSWORD)),
            "",
            None,
        )
        .await,
        Err(identity::Error::Invalid)
    ));
    sqlx::query("DELETE FROM email_rate_limits")
        .execute(&f.server.state.pool)
        .await
        .unwrap();
    let browser = Browser::new(f.server.origin.as_str());
    assert_eq!(
        request(
            &browser,
            "/web/forgot?next=%2Fadmin%2F",
            "recover@example.test",
            &[("next", "/admin/")],
        )
        .await
        .status,
        StatusCode::OK
    );
    let token = f.inbox.token();
    let recovered = confirm(&browser, &token, "the-recovered-password").await;
    assert_eq!(recovered.status, StatusCode::OK);
    assert!(
        recovered
            .html
            .contains("href=\"/web/login?next=%2Fadmin%2F\"")
    );
    assert_eq!(
        send(f.browser.get("/web/account")).await.status,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        try_login(
            &browser,
            "recover@example.test",
            "the-recovered-password",
            "/web/account"
        )
        .await
        .status,
        StatusCode::SEE_OTHER
    );
}

#[tokio::test]
async fn legacy_binding_keeps_the_original_account_and_never_accepts_username_login() {
    let f = setup().await;
    let account = assemble_server::accounts::create(
        &f.server.state.pool,
        "legacy",
        "Old",
        SecretString::new(PASSWORD),
    )
    .await
    .unwrap();
    assert_eq!(
        try_login(&f.browser, "legacy", PASSWORD, "").await.status,
        StatusCode::UNAUTHORIZED
    );
    let page = request(
        &f.browser,
        "/web/legacy",
        "old@example.test",
        &[("username", "legacy"), ("current", PASSWORD)],
    )
    .await;
    assert_eq!(page.status, StatusCode::OK);
    let other_browser = Browser::new(f.server.origin.as_str());
    assert_eq!(
        confirm(&other_browser, &f.inbox.token(), "").await.status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        confirm(&f.browser, &f.inbox.token(), "").await.status,
        StatusCode::SEE_OTHER
    );
    let updated = identity::by_email(&f.server.state.pool, "old@example.test")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(account.id, updated.id);
    assert_eq!(account.password_hash, updated.password_hash);
    assert_eq!(count(&f).await, 1);
}

#[tokio::test]
async fn changing_email_requires_both_mailboxes_and_invalidates_old_recovery() {
    let f = setup().await;
    let id = register(&f, "old@example.test").await;
    sqlx::query("DELETE FROM email_rate_limits")
        .execute(&f.server.state.pool)
        .await
        .unwrap();
    let page = send(f.browser.get("/web/account")).await;
    let csrf = hidden(&page.html, "csrf").unwrap();
    assert_eq!(
        send(f.browser.post_form(
            "/web/account/email",
            &[("email", "new@example.test"), ("csrf", &csrf)]
        ))
        .await
        .status,
        StatusCode::OK
    );
    assert_eq!(f.inbox.recipient(), "old@example.test");
    assert_eq!(
        confirm(&f.browser, &f.inbox.token(), "").await.status,
        StatusCode::OK
    );
    assert_eq!(f.inbox.recipient(), "new@example.test");
    assert!(
        identity::by_email(&f.server.state.pool, "new@example.test")
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        confirm(&f.browser, &f.inbox.token(), "").await.status,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        identity::by_email(&f.server.state.pool, "new@example.test")
            .await
            .unwrap()
            .unwrap()
            .id,
        id
    );
    assert!(
        identity::by_email(&f.server.state.pool, "old@example.test")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn github_verified_email_registers_once_and_later_email_changes_do_not_move_the_account() {
    let f = setup().await;
    assert_eq!(
        github_callback(&f.browser).await.location().as_deref(),
        Some("/web/github/profile")
    );
    assert_eq!(
        github_profile(&f.browser, "github@example.test")
            .await
            .status,
        StatusCode::SEE_OTHER
    );
    let account = identity::by_email(&f.server.state.pool, "github@example.test")
        .await
        .unwrap()
        .unwrap();
    assert!(!account.password_enabled);
    assert_eq!(
        account.avatar_url.as_deref(),
        Some("https://avatars.githubusercontent.com/u/123?s=96")
    );
    f.github.0.lock().unwrap().verified_emails = vec!["different@example.test".into()];
    f.github.0.lock().unwrap().login = "renamed".into();
    let browser = Browser::new(f.server.origin.as_str());
    assert_eq!(
        github_callback(&browser).await.status,
        StatusCode::SEE_OTHER
    );
    assert_eq!(count(&f).await, 1);
    let actions: Vec<String> = sqlx::query_scalar("SELECT action FROM audit_log ORDER BY id")
        .fetch_all(&f.server.state.pool)
        .await
        .unwrap();
    assert_eq!(actions, ["account.registered", "web.login", "web.login"]);
    let methods: Vec<String> = sqlx::query_scalar("SELECT method FROM login_events ORDER BY id")
        .fetch_all(&f.server.state.pool)
        .await
        .unwrap();
    assert_eq!(methods, ["github", "github"]);
    assert_eq!(
        identity::by_email(&f.server.state.pool, "github@example.test")
            .await
            .unwrap()
            .unwrap()
            .id,
        account.id
    );
    assert!(
        identity::by_email(&f.server.state.pool, "different@example.test")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn github_same_email_never_merges_accounts_and_alternate_email_requires_mail() {
    let f = setup().await;
    let id = register(&f, "github@example.test").await;
    let browser = Browser::new(f.server.origin.as_str());
    github_callback(&browser).await;
    assert_eq!(
        github_profile(&browser, "github@example.test").await.status,
        StatusCode::CONFLICT
    );
    let linked: i64 = sqlx::query_scalar("SELECT count(*) FROM github_identities")
        .fetch_one(&f.server.state.pool)
        .await
        .unwrap();
    assert_eq!(linked, 0);
    assert_eq!(count(&f).await, 1);
    assert_eq!(
        github_profile(&browser, "chosen@example.test").await.status,
        StatusCode::OK
    );
    assert_eq!(count(&f).await, 1);
    let unrelated_browser = Browser::new(f.server.origin.as_str());
    // 即使邮箱收件人误点别人发起的验证，也不能给发起者创建可登录的新账户。
    let refused = confirm(&unrelated_browser, &f.inbox.token(), "").await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST);
    assert!(refused.html.contains("同一浏览器"));

    assert_eq!(
        confirm(&browser, &f.inbox.token(), "").await.status,
        StatusCode::SEE_OTHER
    );
    let second = identity::by_email(&f.server.state.pool, "chosen@example.test")
        .await
        .unwrap()
        .unwrap();
    assert_ne!(second.id, id);
    assert!(!second.password_enabled);
}

#[tokio::test]
async fn github_callback_requires_its_browser_and_cannot_be_replayed() {
    let f = setup().await;
    let start = send(f.browser.get("/web/github/start")).await;
    let url = url::Url::parse(&start.location().unwrap()).unwrap();
    let state = url
        .query_pairs()
        .find(|(k, _)| k == "state")
        .unwrap()
        .1
        .into_owned();
    let callback = format!("/web/github/callback?state={state}&code=test-code");
    let foreign = Browser::new(f.server.origin.as_str());
    assert_eq!(
        send(foreign.get(&callback)).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(f.browser.get(&callback)).await.status,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        send(f.browser.get(&callback)).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(count(&f).await, 0);
}

#[tokio::test]
async fn github_binding_requires_original_password_and_preserves_account_id() {
    let f = setup().await;
    let id = register(&f, "owner@example.test").await;
    let page = send(f.browser.get("/web/account")).await;
    let policy = page
        .headers
        .get("content-security-policy")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(policy.contains("form-action 'self' https://github.com"));
    assert!(!policy.contains("form-action *"));
    let csrf = hidden(&page.html, "csrf").unwrap();
    assert_eq!(
        send(f.browser.post_form(
            "/web/account/github",
            &[("csrf", &csrf), ("current", "wrong")]
        ))
        .await
        .status,
        StatusCode::FORBIDDEN
    );
    let start = send(f.browser.post_form(
        "/web/account/github",
        &[("csrf", &csrf), ("current", PASSWORD)],
    ))
    .await;
    let url = url::Url::parse(&start.location().unwrap()).unwrap();
    let state = url
        .query_pairs()
        .find(|(k, _)| k == "state")
        .unwrap()
        .1
        .into_owned();
    assert_eq!(
        send(f.browser.get(&format!(
            "/web/github/callback?state={state}&code=test-code"
        )))
        .await
        .status,
        StatusCode::SEE_OTHER
    );
    let linked: String = sqlx::query_scalar("SELECT account_id FROM github_identities")
        .fetch_one(&f.server.state.pool)
        .await
        .unwrap();
    assert_eq!(linked, id);
    assert_eq!(count(&f).await, 1);
    assert_eq!(
        identity::by_email(&f.server.state.pool, "owner@example.test")
            .await
            .unwrap()
            .unwrap()
            .id,
        id
    );
}

#[tokio::test]
async fn a_github_only_account_can_set_a_password_through_its_primary_email() {
    let f = setup().await;
    github_callback(&f.browser).await;
    github_profile(&f.browser, "github@example.test").await;
    let original = identity::by_email(&f.server.state.pool, "github@example.test")
        .await
        .unwrap()
        .unwrap();
    let browser = Browser::new(f.server.origin.as_str());
    assert_eq!(
        request(&browser, "/web/forgot", "github@example.test", &[])
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        confirm(&browser, &f.inbox.token(), PASSWORD).await.status,
        StatusCode::OK
    );
    let updated = identity::by_email(&f.server.state.pool, "github@example.test")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(original.id, updated.id);
    assert!(updated.password_enabled);
    assert_eq!(
        try_login(&browser, "github@example.test", PASSWORD, "/web/account")
            .await
            .status,
        StatusCode::SEE_OTHER
    );
}

#[tokio::test]
async fn registration_remains_visible_without_smtp_and_delivery_failure_removes_the_action() {
    let dir = tempfile::tempdir().unwrap();
    let server = TestServer::start_default(dir.path()).await.unwrap();
    let browser = Browser::new(server.origin.as_str());
    assert_eq!(
        send(browser.get("/web/register")).await.status,
        StatusCode::OK
    );
    assert_eq!(
        request(&browser, "/web/register", "one@example.test", &[])
            .await
            .status,
        StatusCode::SERVICE_UNAVAILABLE
    );
    let oversized = "x".repeat(20 * 1024);
    assert_eq!(
        send(browser.post_form("/web/register", &[("email", oversized.as_str())]))
            .await
            .status,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    let f = setup().await;
    let pool = assemble_server::db::open(dir.path().join("broken").as_path())
        .await
        .unwrap();
    let failed = TestServer::start_with_services(
        Config::development(dir.path().join("broken")),
        pool,
        Arc::new(BrokenMailer),
        f.github.clone(),
    )
    .await
    .unwrap();
    let browser = Browser::new(failed.origin.as_str());
    assert_eq!(
        request(&browser, "/web/register", "one@example.test", &[])
            .await
            .status,
        StatusCode::SERVICE_UNAVAILABLE
    );
    let pending: i64 = sqlx::query_scalar("SELECT count(*) FROM email_actions")
        .fetch_one(&failed.state.pool)
        .await
        .unwrap();
    assert_eq!(pending, 0);
}

/// 一个来源挤不满 GitHub 登录的那张表。起点不需要登录，谁都能点；不按来源分的话，
/// 点满之后所有人都进不了 GitHub 登录。
#[tokio::test]
async fn one_source_cannot_fill_the_table_of_pending_github_logins() {
    let dir = tempfile::tempdir().unwrap();
    let pool = assemble_server::db::open(dir.path()).await.unwrap();
    let inbox = Arc::new(Inbox::default());
    let github = Arc::new(Github(Mutex::new(Profile {
        id: "123".into(),
        login: "octocat".into(),
        name: "Octocat".into(),
        verified_emails: vec!["github@example.test".into()],
    })));
    let mut config = Config::development(dir.path());
    config.github = Some(GithubConfig {
        client_id: "test-client".into(),
        client_secret: SecretString::new("test-secret"),
    });
    config.client_ip_header = Some(axum::http::HeaderName::from_static("x-real-ip"));
    let server = TestServer::start_with_services(config, pool, inbox, github)
        .await
        .unwrap();
    let browser = Browser::new(server.origin.as_str());

    // 同一个来源连着发起八次，每次都扔下不走。
    for _ in 0..8 {
        let page = send(
            browser
                .get("/web/github/start")
                .header("x-real-ip", "203.0.113.7"),
        )
        .await;
        assert_eq!(page.status, StatusCode::SEE_OTHER);
    }
    // 第九次：请稍后再来，没有跳去 GitHub。
    let ninth = send(
        browser
            .get("/web/github/start")
            .header("x-real-ip", "203.0.113.7"),
    )
    .await;
    assert_eq!(ninth.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(ninth.location().is_none());
    assert_eq!(ninth.headers.get("retry-after").unwrap(), "60");
    // 别的来源不受牵连。
    let mine = send(
        browser
            .get("/web/github/start")
            .header("x-real-ip", "198.51.100.9"),
    )
    .await;
    assert_eq!(mine.status, StatusCode::SEE_OTHER);
    assert!(
        mine.location()
            .unwrap()
            .starts_with("https://github.com/login/oauth/authorize")
    );
}
