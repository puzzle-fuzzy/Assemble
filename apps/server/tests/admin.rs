//! 管理权限、同源读取与登录统计使用临时数据库和真实 HTTP，不接触正式账号。

mod common;

use assemble_protocol::admin::{
    AdminOverview, AdminPage, AdminSession, AdminUser, AdminUserDetail,
};
use assemble_protocol::device::NewDevice;
use assemble_protocol::primitives::{Platform, Timestamp};
use assemble_server::admin;
use assemble_server::email_identity::{self as identity, Action};
use assemble_server::http::TestServer;
use assemble_server::{Config, SecretString};
use common::*;
use reqwest::StatusCode;
use serde_json::Value;

const ADMIN_EMAIL: &str = "owner@example.test";
const PATHS: [&str; 8] = [
    "/v1/admin/session",
    "/v1/admin/overview",
    "/v1/admin/users",
    "/v1/admin/users/unknown",
    "/v1/admin/devices",
    "/v1/admin/logins",
    "/v1/admin/drive/files",
    "/v1/admin/storage",
];

#[tokio::test]
async fn admin_login_and_logout_stay_in_the_browser_without_registering_a_device() {
    let mut f = fixture().await;
    admin::grant_super_admin(&f.server.state.pool, EMAIL)
        .await
        .unwrap();
    let form = send(f.browser.get("/web/login?next=%2Fadmin%2F")).await;
    assert!(form.html.contains("<h1>登录管理控制台</h1>"));
    assert!(form.html.contains("登录后会直接进入网页管理控制台"));
    assert!(form.html.contains("/web/forgot?next=%2Fadmin%2F"));
    assert!(!form.html.contains("/desktop/authorize"));
    let csrf = hidden(&form.html, "csrf").unwrap();
    let failed = submit_login(&f.browser, EMAIL, "wrong-password", "/admin/", &csrf).await;
    assert_eq!(failed.status, StatusCode::UNAUTHORIZED);
    assert!(failed.html.contains("<h1>登录管理控制台</h1>"));
    let signed_in = submit_login(&f.browser, EMAIL, PASSWORD, "/admin/", &csrf).await;
    assert_eq!(signed_in.status, StatusCode::SEE_OTHER);
    assert_eq!(signed_in.location().as_deref(), Some("/admin/"));
    let devices: i64 = sqlx::query_scalar("SELECT count(*) FROM devices")
        .fetch_one(&f.server.state.pool)
        .await
        .unwrap();
    assert_eq!(devices, 0);
    let already_signed_in = send(f.browser.get("/web/login?next=%2Fadmin%2F")).await;
    assert_eq!(already_signed_in.location().as_deref(), Some("/admin/"));
    let cross_site = send(
        f.browser
            .http
            .post(format!("{}/web/logout?next=/admin/", f.browser.origin))
            .header("origin", "https://foreign.example.test")
            .form(&[("csrf", &csrf)]),
    )
    .await;
    assert_eq!(cross_site.status, StatusCode::FORBIDDEN);
    assert_eq!(
        get(&f.browser, "/v1/admin/session")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let signed_out = send(f.browser.post_form("/web/logout?next=/admin/", &[])).await;
    assert_eq!(signed_out.status, StatusCode::SEE_OTHER);
    assert_eq!(signed_out.headers["cache-control"], "no-store");
    assert_eq!(
        signed_out.location().as_deref(),
        Some("/web/login?next=%2Fadmin%2F")
    );
    let login = send(f.browser.get(&signed_out.location().unwrap())).await;
    assert!(login.html.contains("<h1>登录管理控制台</h1>"));
    assert_eq!(
        get(&f.browser, "/v1/admin/session")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    f.server.stop().await;
}

#[tokio::test]
async fn logout_never_redirects_to_a_supplied_external_or_device_return_address() {
    let mut f = fixture().await;
    for next in [
        "https://foreign.example.test/admin/",
        "//foreign.example.test/admin/",
        "/admin/../desktop/authorize",
        "/desktop/authorize?request=abc",
        "/admin/?next=https://foreign.example.test",
    ] {
        let query: String = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("next", next)
            .finish();
        let page = send(f.browser.post_form(&format!("/web/logout?{query}"), &[])).await;
        assert_eq!(page.status, StatusCode::OK);
        assert!(page.location().is_none());
        assert!(page.html.contains("你已退出当前网页账户"));
        assert!(page.html.contains("登录管理控制台"));
        assert!(!page.html.contains("可以关闭"));
    }
    f.server.stop().await;
}

#[tokio::test]
async fn a_failed_logout_keeps_the_browser_session_until_revocation_is_confirmed() {
    let mut f = fixture().await;
    admin::grant_super_admin(&f.server.state.pool, EMAIL)
        .await
        .unwrap();
    try_login(&f.browser, EMAIL, PASSWORD, "/admin/").await;
    sqlx::query("CREATE TRIGGER fail_logout BEFORE DELETE ON web_sessions BEGIN SELECT RAISE(ABORT, 'synthetic failure'); END")
        .execute(&f.server.state.pool).await.unwrap();
    let failed = send(f.browser.post_form("/web/logout?next=/admin/", &[])).await;
    assert_eq!(failed.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(failed.location().is_none());
    assert_eq!(failed.set_cookies().len(), 0);
    assert_eq!(
        get(&f.browser, "/v1/admin/session")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    sqlx::query("DROP TRIGGER fail_logout")
        .execute(&f.server.state.pool)
        .await
        .unwrap();
    let retry = send(f.browser.post_form("/web/logout?next=/admin/", &[])).await;
    assert_eq!(retry.status, StatusCode::SEE_OTHER);
    assert_eq!(
        get(&f.browser, "/v1/admin/session")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    f.server.stop().await;
}

fn get(browser: &Browser, path: &str) -> reqwest::RequestBuilder {
    browser.get(path).header("origin", &browser.origin)
}

async fn json(browser: &Browser, path: &str) -> Value {
    let response = get(browser, path).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK, "{path}");
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(
        response.headers()["cross-origin-resource-policy"],
        "same-origin"
    );
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    response.json().await.unwrap()
}

async fn assert_rejected(browser: &Browser, status: StatusCode) {
    for path in PATHS {
        let response = get(browser, path).send().await.unwrap();
        assert_eq!(response.status(), status, "{path}");
        assert_eq!(response.headers()["cache-control"], "no-store");
        let body: Value = response.json().await.unwrap();
        assert_eq!(body.as_object().unwrap().len(), 1);
        assert!(body.get("error").is_some());
        assert!(!body.to_string().contains(EMAIL));
    }
}

#[tokio::test]
async fn every_admin_route_requires_a_verified_admin_browser_and_same_origin() {
    let mut f = fixture().await;
    assert_rejected(&f.browser, StatusCode::UNAUTHORIZED).await;
    log_in(&f.browser).await;
    assert_rejected(&f.browser, StatusCode::FORBIDDEN).await;
    let admin_id = admin::grant_super_admin(&f.server.state.pool, EMAIL)
        .await
        .unwrap();
    let session: AdminSession = serde_json::from_value(json(&f.browser, PATHS[0]).await).unwrap();
    assert_eq!(session.account.id, admin_id);
    assert_eq!(session.account.email.as_deref(), Some(EMAIL));
    assert_ne!(session.csrf_token, "");
    for request in [
        f.browser.get(PATHS[1]),
        f.browser
            .get(PATHS[1])
            .header("origin", "https://foreign.example.test"),
        f.browser
            .get(PATHS[1])
            .header("sec-fetch-site", "same-site"),
        get(&f.browser, PATHS[1]).header("sec-fetch-site", "cross-site"),
        f.browser
            .get(PATHS[1])
            .header("sec-fetch-site", "same-origin")
            .header("origin", "null"),
    ] {
        assert_eq!(
            request.send().await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        f.browser
            .get(PATHS[1])
            .header("sec-fetch-site", "same-origin")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    // 即使管理员自己的设备也不能用设备令牌读取全站账号。
    let registered = register(&f, 'a', "管理员电脑", Platform::Windows).await;
    assert_eq!(
        reqwest::Client::new()
            .get(format!("{}{}", f.browser.origin, PATHS[1]))
            .header("origin", &f.browser.origin)
            .bearer_auth(registered.token.expose())
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    sqlx::query("UPDATE accounts SET email_verified_at = NULL WHERE id = ?")
        .bind(&admin_id)
        .execute(&f.server.state.pool)
        .await
        .unwrap();
    assert_rejected(&f.browser, StatusCode::FORBIDDEN).await;
    sqlx::query("UPDATE accounts SET email_verified_at = 1, is_super_admin = 0 WHERE id = ?")
        .bind(&admin_id)
        .execute(&f.server.state.pool)
        .await
        .unwrap();
    assert_rejected(&f.browser, StatusCode::FORBIDDEN).await;
    sqlx::query("UPDATE web_sessions SET expires_at = 0")
        .execute(&f.server.state.pool)
        .await
        .unwrap();
    assert_rejected(&f.browser, StatusCode::UNAUTHORIZED).await;
    f.server.stop().await;
}

#[tokio::test]
async fn designated_email_is_granted_only_after_real_verification_and_session_creation() {
    let dir = tempfile::tempdir().unwrap();
    let pool = assemble_server::db::open(dir.path()).await.unwrap();
    let mut config = Config::development(dir.path());
    config.super_admin_email = Some(ADMIN_EMAIL.into());
    let mut server = TestServer::start(config, pool).await.unwrap();
    let first = assemble_server::accounts::create(
        &server.state.pool,
        "first",
        "首个普通账户",
        SecretString::new(PASSWORD),
    )
    .await
    .unwrap();
    admin::bootstrap(&server.state.pool, Some(ADMIN_EMAIL))
        .await
        .unwrap();
    let first_admin: bool = sqlx::query_scalar("SELECT is_super_admin FROM accounts WHERE id = ?")
        .bind(&first.id)
        .fetch_one(&server.state.pool)
        .await
        .unwrap();
    assert!(!first_admin);
    let token = identity::issue(
        &server.state.pool,
        &Action::new("register", ADMIN_EMAIL.into(), "/admin/".into()),
    )
    .await
    .unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM accounts WHERE is_super_admin = 1")
        .fetch_one(&server.state.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    let browser = Browser::new(server.origin.as_str());
    let form = send(browser.get("/web/verify")).await;
    let csrf = hidden(&form.html, "csrf").unwrap();
    let confirmed = send(browser.post_form(
        "/web/verify",
        &[
            ("token", token.expose()),
            ("csrf", &csrf),
            ("password", PASSWORD),
            ("password2", PASSWORD),
            ("display_name", "管理员"),
        ],
    ))
    .await;
    assert_eq!(confirmed.status, StatusCode::SEE_OTHER);
    assert_eq!(confirmed.location().as_deref(), Some("/admin/"));
    let session = json(&browser, "/v1/admin/session").await;
    assert_eq!(session["account"]["email"], ADMIN_EMAIL);
    let id = session["account"]["id"].as_str().unwrap();
    let detail: AdminUserDetail =
        serde_json::from_value(json(&browser, &format!("/v1/admin/users/{id}")).await).unwrap();
    assert!(detail.user.is_super_admin);
    assert_eq!(detail.logins.len(), 1);
    assert_eq!(
        detail.user.last_login_at,
        Some(detail.logins[0].occurred_at)
    );
    assert_eq!(
        detail.logins[0].method,
        assemble_protocol::admin::LoginMethod::Email
    );
    admin::bootstrap(&server.state.pool, Some(ADMIN_EMAIL))
        .await
        .unwrap();
    admin::grant_super_admin(&server.state.pool, ADMIN_EMAIL)
        .await
        .unwrap();
    let grants: Vec<String> =
        sqlx::query_scalar("SELECT target FROM audit_log WHERE action = 'admin.granted'")
            .fetch_all(&server.state.pool)
            .await
            .unwrap();
    assert_eq!(grants, ["bootstrap"]);
    server.stop().await;
}

#[tokio::test]
async fn bootstrap_and_cli_refuse_unverified_accounts_and_grants_are_audited() {
    let mut f = fixture().await;
    sqlx::query("UPDATE accounts SET email_verified_at = NULL")
        .execute(&f.server.state.pool)
        .await
        .unwrap();
    assert!(matches!(
        admin::grant_super_admin(&f.server.state.pool, EMAIL).await,
        Err(admin::GrantError::NotVerified)
    ));
    assert!(matches!(
        admin::grant_super_admin(&f.server.state.pool, "absent@example.test").await,
        Err(admin::GrantError::NotVerified)
    ));
    assert!(matches!(
        admin::grant_super_admin(&f.server.state.pool, "invalid").await,
        Err(admin::GrantError::Email)
    ));
    admin::bootstrap(&f.server.state.pool, Some(EMAIL))
        .await
        .unwrap();
    let flag: bool = sqlx::query_scalar("SELECT is_super_admin FROM accounts")
        .fetch_one(&f.server.state.pool)
        .await
        .unwrap();
    assert!(!flag);
    sqlx::query("UPDATE accounts SET email_verified_at = 1")
        .execute(&f.server.state.pool)
        .await
        .unwrap();
    let id = admin::grant_super_admin(&f.server.state.pool, " ALEX@EXAMPLE.TEST ")
        .await
        .unwrap();
    admin::grant_super_admin(&f.server.state.pool, EMAIL)
        .await
        .unwrap();
    let grants: Vec<(String, String)> =
        sqlx::query_as("SELECT account_id, target FROM audit_log WHERE action = 'admin.granted'")
            .fetch_all(&f.server.state.pool)
            .await
            .unwrap();
    assert_eq!(grants, [(id, "cli".into())]);
    f.server.stop().await;
}

fn no_credentials(value: &Value) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                assert!(
                    ![
                        "password",
                        "passwordHash",
                        "token",
                        "tokenHash",
                        "csrfToken",
                        "code",
                        "proofHash",
                        "browserHash"
                    ]
                    .contains(&key.as_str()),
                    "{key}"
                );
                no_credentials(value);
            }
        }
        Value::Array(items) => {
            for item in items {
                no_credentials(item);
            }
        }
        _ => {}
    }
}

async fn device(server: &TestServer, account_id: &str, name: &str, token_hash: &str) -> String {
    assemble_server::devices::register(
        &server.state.pool,
        account_id,
        &NewDevice {
            name: name.into(),
            platform: Platform::Windows,
        },
        token_hash,
    )
    .await
    .unwrap()
    .id
}

#[tokio::test]
async fn user_device_and_online_totals_search_and_stable_pagination_are_read_only() {
    let mut f = fixture().await;
    let owner = admin::grant_super_admin(&f.server.state.pool, EMAIL)
        .await
        .unwrap();
    log_in(&f.browser).await;
    let other = assemble_server::accounts::create(
        &f.server.state.pool,
        "other",
        "其他用户",
        SecretString::new(PASSWORD),
    )
    .await
    .unwrap();
    let legacy = assemble_server::accounts::create(
        &f.server.state.pool,
        "legacy",
        "旧式账户",
        SecretString::new(PASSWORD),
    )
    .await
    .unwrap();
    let now = Timestamp::now().as_millis();
    sqlx::query("UPDATE accounts SET created_at = ? WHERE id = ? OR id = ?")
        .bind(now)
        .bind(&owner)
        .bind(&other.id)
        .execute(&f.server.state.pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE accounts SET created_at = ?, email = 'unverified@example.test' WHERE id = ?",
    )
    .bind(now - 172_800_000)
    .bind(&legacy.id)
    .execute(&f.server.state.pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE accounts SET email = 'other@example.test', email_verified_at = 1 WHERE id = ?",
    )
    .bind(&other.id)
    .execute(&f.server.state.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO github_identities (github_id, account_id, login) VALUES ('123', ?, 'test')",
    )
    .bind(&other.id)
    .execute(&f.server.state.pool)
    .await
    .unwrap();
    let pc = device(&f.server, &owner, "办公室电脑", "private-device-digest-one").await;
    let _offline = device(&f.server, &owner, "便携电脑", "private-device-digest-two").await;
    let _other_pc = device(
        &f.server,
        &other.id,
        "客厅电脑",
        "private-device-digest-three",
    )
    .await;
    assert!(f.server.state.presence.connect(&owner, &pc));
    assert!(!f.server.state.presence.connect(&owner, &pc));
    // 未登记的残留连接不计入数据库中的设备总数或在线数。
    f.server
        .state
        .presence
        .connect(&owner, "already-removed-device");
    assert_account_totals(&f, &owner, &other.id, &legacy.id).await;
    assert_search_and_pagination(&f, &owner, &other.id).await;
    assert_read_only_bounds(&f, &owner).await;
    f.server.stop().await;
}

async fn assert_account_totals(f: &common::Fixture, owner: &str, other: &str, legacy: &str) {
    let overview: AdminOverview =
        serde_json::from_value(json(&f.browser, "/v1/admin/overview").await).unwrap();
    assert_eq!(
        (
            overview.users,
            overview.devices,
            overview.online_devices,
            overview.registrations_today
        ),
        (3, 3, 1, 2)
    );
    let all = json(&f.browser, "/v1/admin/users").await;
    no_credentials(&all);
    let listing: AdminPage<AdminUser> = serde_json::from_value(all).unwrap();
    assert_eq!(listing.total, 3);
    assert_eq!(
        listing
            .items
            .iter()
            .find(|u| u.id == owner)
            .unwrap()
            .device_count,
        2
    );
    assert_eq!(
        listing
            .items
            .iter()
            .find(|u| u.id == owner)
            .unwrap()
            .online_device_count,
        1
    );
    assert!(
        listing
            .items
            .iter()
            .find(|u| u.id == other)
            .unwrap()
            .has_github
    );
    let old = listing.items.iter().find(|u| u.id == legacy).unwrap();
    assert_eq!(old.email, None);
    assert_eq!(old.last_login_at, None);
}

async fn assert_search_and_pagination(f: &common::Fixture, owner: &str, other: &str) {
    let mut expected = [owner.to_owned(), other.to_owned()];
    expected.sort_by(|a, b| b.cmp(a));
    for (i, id) in expected.iter().enumerate() {
        let page = json(
            &f.browser,
            &format!("/v1/admin/users?page={}&pageSize=1", i + 1),
        )
        .await;
        assert_eq!(page["items"][0]["id"], *id);
        assert_eq!(page["total"], 3);
    }
    assert_eq!(
        json(&f.browser, "/v1/admin/users?q=OTHER%40EXAMPLE.TEST").await["total"],
        1
    );
    assert_eq!(
        json(&f.browser, "/v1/admin/users?q=unverified").await["total"],
        0
    );
    assert_eq!(json(&f.browser, "/v1/admin/users?q=%25").await["total"], 0);
    assert_eq!(
        json(&f.browser, "/v1/admin/users?page=4000000000&pageSize=100").await["items"],
        serde_json::json!([])
    );
    let devices = json(&f.browser, "/v1/admin/devices?q=OTHER").await;
    no_credentials(&devices);
    assert_eq!(devices["total"], 1);
    assert_eq!(devices["items"][0]["accountId"], other);
    let device_page = json(&f.browser, "/v1/admin/devices?page=2&pageSize=1").await;
    assert_eq!(device_page["items"].as_array().unwrap().len(), 1);
    assert_eq!(device_page["total"], 3);
    let detail = json(&f.browser, &format!("/v1/admin/users/{owner}")).await;
    no_credentials(&detail);
    assert_eq!(detail["devices"].as_array().unwrap().len(), 2);
    assert_eq!(detail["logins"].as_array().unwrap().len(), 1);
    let serialized = serde_json::to_string(&detail).unwrap();
    assert!(!serialized.contains("private-device-digest"));
}

async fn assert_read_only_bounds(f: &common::Fixture, owner: &str) {
    for path in ["/v1/admin/users", "/v1/admin/devices", "/v1/admin/logins"] {
        for query in [
            "page=0",
            "pageSize=0",
            "pageSize=101",
            "page=-1",
            "q=%0Aline",
        ] {
            assert_eq!(
                get(&f.browser, &format!("{path}?{query}"))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                StatusCode::BAD_REQUEST
            );
        }
        assert_eq!(
            get(&f.browser, &format!("{path}?q={}", "a".repeat(201)))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        get(&f.browser, "/v1/admin/users/no-such-user")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.browser
            .http
            .post(format!("{}/v1/admin/users/{owner}", f.browser.origin))
            .header("origin", &f.browser.origin)
            .json(&serde_json::json!({"isSuperAdmin":true}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::METHOD_NOT_ALLOWED
    );
}

#[tokio::test]
async fn only_successful_logins_update_time_and_api_reads_do_not_count_as_logins() {
    let mut f = fixture().await;
    admin::grant_super_admin(&f.server.state.pool, EMAIL)
        .await
        .unwrap();
    let initial: Option<i64> = sqlx::query_scalar("SELECT last_login_at FROM accounts")
        .fetch_one(&f.server.state.pool)
        .await
        .unwrap();
    assert_eq!(initial, None);
    assert_eq!(
        try_login(&f.browser, EMAIL, "wrong-password", "")
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    let failed: Option<i64> = sqlx::query_scalar("SELECT last_login_at FROM accounts")
        .fetch_one(&f.server.state.pool)
        .await
        .unwrap();
    assert_eq!(failed, None);
    assert_eq!(
        try_login(&f.browser, EMAIL, PASSWORD, "/admin/")
            .await
            .status,
        StatusCode::SEE_OTHER
    );
    let at: i64 = sqlx::query_scalar("SELECT last_login_at FROM accounts")
        .fetch_one(&f.server.state.pool)
        .await
        .unwrap();
    let logins = json(&f.browser, "/v1/admin/logins?q=EMAIL&pageSize=1").await;
    no_credentials(&logins);
    assert_eq!(logins["total"], 1);
    assert_eq!(logins["items"][0]["method"], "email");
    let timestamp: Timestamp =
        serde_json::from_value(logins["items"][0]["occurredAt"].clone()).unwrap();
    assert_eq!(timestamp.as_millis(), at);
    json(&f.browser, "/v1/admin/users").await;
    json(&f.browser, "/v1/admin/overview").await;
    let after: i64 = sqlx::query_scalar("SELECT last_login_at FROM accounts")
        .fetch_one(&f.server.state.pool)
        .await
        .unwrap();
    assert_eq!(after, at);
    assert_eq!(
        json(&f.browser, "/v1/admin/logins?page=2&pageSize=1").await["items"],
        serde_json::json!([])
    );
    let newcomer = Browser::new(f.browser.origin.as_str());
    try_login(&newcomer, EMAIL, PASSWORD, "").await;
    assert_eq!(json(&f.browser, "/v1/admin/logins").await["total"], 2);
    f.server.stop().await;
}

#[tokio::test]
async fn a_failed_login_event_write_rolls_back_the_session_and_login_time() {
    let mut f = fixture().await;
    sqlx::query("CREATE TRIGGER fail_login BEFORE INSERT ON login_events BEGIN SELECT RAISE(ABORT, 'synthetic failure'); END")
        .execute(&f.server.state.pool).await.unwrap();
    assert_eq!(
        log_in(&f.browser).await.status,
        StatusCode::SERVICE_UNAVAILABLE
    );
    let sessions: i64 = sqlx::query_scalar("SELECT count(*) FROM web_sessions")
        .fetch_one(&f.server.state.pool)
        .await
        .unwrap();
    assert_eq!(sessions, 0);
    let at: Option<i64> = sqlx::query_scalar("SELECT last_login_at FROM accounts")
        .fetch_one(&f.server.state.pool)
        .await
        .unwrap();
    assert_eq!(at, None);
    f.server.stop().await;
}
