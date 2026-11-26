//! 写入权限、事务审计和连接失效只使用本机临时账户验证。
mod common;

use assemble_protocol::admin::{AdminOverview, AdminSession, AdminUser};
use assemble_protocol::device::NewDevice;
use assemble_protocol::primitives::Platform;
use assemble_server::{SecretString, accounts, admin, devices, secrets};
use common::*;
use futures_util::StreamExt as _;
use reqwest::StatusCode;
use serde_json::Value;

async fn owner(f: &Fixture) -> AdminSession {
    admin::grant_super_admin(&f.server.state.pool, EMAIL)
        .await
        .unwrap();
    log_in(&f.browser).await;
    f.browser
        .get("/v1/admin/session")
        .header("origin", &f.browser.origin)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn target(f: &Fixture) -> String {
    let account = accounts::create(
        &f.server.state.pool,
        "member",
        "成员",
        SecretString::new(PASSWORD),
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE accounts SET email = 'member@example.test', email_verified_at = 1 WHERE id = ?",
    )
    .bind(&account.id)
    .execute(&f.server.state.pool)
    .await
    .unwrap();
    account.id
}

fn change(f: &Fixture, id: &str, action: &str, csrf: &str) -> reqwest::RequestBuilder {
    f.browser
        .http
        .post(format!("{}/v1/admin/users/{id}/{action}", f.browser.origin))
        .header("origin", &f.browser.origin)
        .header("x-csrf-token", csrf)
}

async fn device_status(f: &Fixture, token: &str) -> StatusCode {
    reqwest::Client::new()
        .get(format!("{}/v1/me", f.browser.origin))
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn overview_storage_uses_real_totals_without_paths_or_credentials() {
    let mut f = fixture().await;
    let session = owner(&f).await;
    let member = target(&f).await;
    for (digest, size) in [("shared", 512_i64), ("awaiting-sweep", 128)] {
        sqlx::query("INSERT INTO blobs(sha256,size,refcount,created_at) VALUES (?,?,1,1)")
            .bind(digest)
            .bind(size)
            .execute(&f.server.state.pool)
            .await
            .unwrap();
    }
    for (id, account) in [
        ("admin-file", session.account.id.as_str()),
        ("member-file", member.as_str()),
        ("member-copy", member.as_str()),
    ] {
        sqlx::query("INSERT INTO drive_files(id,account_id,name,size,sha256,created_at) VALUES (?,?,'file',512,'shared',1)")
            .bind(id).bind(account).execute(&f.server.state.pool).await.unwrap();
    }
    let response = f
        .browser
        .get("/v1/admin/overview")
        .header("origin", &f.browser.origin)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let raw: Value = response.json().await.unwrap();
    assert_eq!(raw["storage"].as_object().unwrap().len(), 8);
    let serialized = raw.to_string();
    assert!(!serialized.contains(f.dir.path().to_str().unwrap()));
    for field in ["password", "token", "digest", "csrf", "sha256"] {
        assert!(!serialized.to_lowercase().contains(field));
    }
    let overview: AdminOverview = serde_json::from_value(raw).unwrap();
    assert_eq!(overview.storage.content_bytes, 640);
    assert_eq!(
        overview.storage.budget_bytes,
        f.server.state.config.storage_budget_bytes
    );
    assert_eq!(overview.storage.warning_percent, 80);
    assert!(!overview.storage.budget_warning);
    assert!(overview.storage.database_bytes.is_some_and(|size| size > 0));
    assert!(overview.storage.disk_available_bytes.is_some());
    assert_eq!(overview.storage.top_accounts.len(), 2);
    assert!(
        overview
            .storage
            .top_accounts
            .iter()
            .all(|account| account.content_bytes == 512)
    );
    assert!(
        overview
            .storage
            .top_accounts
            .iter()
            .any(|account| account.account.email.as_deref() == Some("member@example.test"))
    );
    f.server.stop().await;
}

#[tokio::test]
async fn suspension_is_idempotent_audited_and_closes_existing_device_streams() {
    let mut f = fixture().await;
    let session = owner(&f).await;
    let id = target(&f).await;
    let member = Browser::new(&f.browser.origin);
    assert_eq!(
        try_login(&member, "member@example.test", PASSWORD, "/web/account")
            .await
            .status,
        StatusCode::SEE_OTHER
    );
    let token = "synthetic-member-device-token";
    let device = devices::register(
        &f.server.state.pool,
        &id,
        &NewDevice {
            name: "成员设备".into(),
            platform: Platform::Windows,
        },
        &secrets::token_digest(token),
    )
    .await
    .unwrap();
    let response = reqwest::Client::new()
        .get(format!("{}/v1/events", f.browser.origin))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.bytes_stream();
    assert!(stream.next().await.unwrap().unwrap().starts_with(b"data:"));
    assert!(f.server.state.presence.is_online(&id, &device.id));
    for _ in 0..2 {
        let response = change(&f, &id, "suspend", &session.csrf_token)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["cache-control"], "no-store");
        let body: Value = response.json().await.unwrap();
        assert!(
            body.as_object()
                .unwrap()
                .keys()
                .all(|key| !key.to_lowercase().contains("token")
                    && !key.to_lowercase().contains("password"))
        );
        let user: AdminUser = serde_json::from_value(body).unwrap();
        assert!(user.suspended);
        assert_eq!(user.online_device_count, 0);
    }
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while let Some(chunk) = stream.next().await {
            chunk.unwrap();
        }
    })
    .await
    .expect("suspension must end the existing stream promptly");
    assert!(!f.server.state.presence.is_online(&id, &device.id));
    assert_eq!(device_status(&f, token).await, StatusCode::UNAUTHORIZED);
    let sessions: i64 =
        sqlx::query_scalar("SELECT count(*) FROM web_sessions WHERE account_id = ?")
            .bind(&id)
            .fetch_one(&f.server.state.pool)
            .await
            .unwrap();
    assert_eq!(sessions, 0);
    assert_eq!(
        try_login(&member, "member@example.test", PASSWORD, "")
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    for _ in 0..2 {
        let response = change(&f, &id, "resume", &session.csrf_token)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(!response.json::<AdminUser>().await.unwrap().suspended);
    }
    let rows: Vec<(String,String,String)> = sqlx::query_as("SELECT account_id,action,target FROM audit_log WHERE action IN ('account.suspended','account.resumed') ORDER BY id")
        .fetch_all(&f.server.state.pool).await.unwrap();
    assert_eq!(
        rows,
        [
            (
                session.account.id.clone(),
                "account.suspended".into(),
                id.clone()
            ),
            (session.account.id, "account.resumed".into(), id.clone())
        ]
    );
    assert_eq!(device_status(&f, token).await, StatusCode::OK);
    f.server.stop().await;
}

#[tokio::test]
async fn writes_require_an_admin_cookie_exact_origin_and_csrf_and_cannot_suspend_self() {
    let mut f = fixture().await;
    let id = target(&f).await;
    assert_eq!(
        change(&f, &id, "suspend", "unused")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    log_in(&f.browser).await;
    assert_eq!(
        change(&f, &id, "suspend", "unused")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    let session = owner(&f).await;
    let path = format!("{}/v1/admin/users/{id}/suspend", f.browser.origin);
    for request in [
        f.browser
            .http
            .post(&path)
            .header("x-csrf-token", &session.csrf_token),
        f.browser
            .http
            .post(&path)
            .header("origin", "https://foreign.example.test")
            .header("x-csrf-token", &session.csrf_token),
        f.browser
            .http
            .post(&path)
            .header("origin", &f.browser.origin),
        change(&f, &id, "suspend", "incorrect"),
        change(&f, &id, "suspend", &session.csrf_token)
            .header("origin", "https://foreign.example.test"),
        change(&f, &id, "suspend", &session.csrf_token).header("x-csrf-token", "incorrect"),
        change(&f, &id, "suspend", &session.csrf_token).header("sec-fetch-site", "cross-site"),
    ] {
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(response.headers()["cache-control"], "no-store");
        let body: Value = response.json().await.unwrap();
        assert!(!body.to_string().contains("member@example.test"));
    }
    let registered = register(&f, 'z', "管理员设备", Platform::Windows).await;
    assert_eq!(
        reqwest::Client::new()
            .post(&path)
            .bearer_auth(registered.token.expose())
            .header("origin", &f.browser.origin)
            .header("x-csrf-token", &session.csrf_token)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        change(&f, &session.account.id, "suspend", &session.csrf_token)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        change(&f, "missing-account", "suspend", &session.csrf_token)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action = 'account.suspended'")
            .fetch_one(&f.server.state.pool)
            .await
            .unwrap();
    assert_eq!(count, 0);
    let suspended: bool =
        sqlx::query_scalar("SELECT suspended_at IS NOT NULL FROM accounts WHERE id = ?")
            .bind(&id)
            .fetch_one(&f.server.state.pool)
            .await
            .unwrap();
    assert!(!suspended);
    f.server.stop().await;
}

#[tokio::test]
async fn an_audit_failure_rolls_back_account_and_session_changes() {
    let mut f = fixture().await;
    let session = owner(&f).await;
    let id = target(&f).await;
    let browser = Browser::new(&f.browser.origin);
    try_login(&browser, "member@example.test", PASSWORD, "").await;
    sqlx::query("CREATE TRIGGER fail_suspension_audit BEFORE INSERT ON audit_log WHEN NEW.action = 'account.suspended' BEGIN SELECT RAISE(ABORT, 'synthetic failure'); END")
        .execute(&f.server.state.pool).await.unwrap();
    assert_eq!(
        change(&f, &id, "suspend", &session.csrf_token)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    let suspended: bool =
        sqlx::query_scalar("SELECT suspended_at IS NOT NULL FROM accounts WHERE id = ?")
            .bind(&id)
            .fetch_one(&f.server.state.pool)
            .await
            .unwrap();
    let sessions: i64 =
        sqlx::query_scalar("SELECT count(*) FROM web_sessions WHERE account_id = ?")
            .bind(&id)
            .fetch_one(&f.server.state.pool)
            .await
            .unwrap();
    assert!(!suspended);
    assert_eq!(sessions, 1);
    f.server.stop().await;
}
