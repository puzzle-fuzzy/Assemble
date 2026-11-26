//! 临时账户和真实 HTTP 验证系统通知权限、恢复、版本与各管理员独立已读。
mod common;

use assemble_protocol::admin::{AdminSession, AdminStorage};
use assemble_protocol::system_notifications::{
    SystemNotificationRead, SystemNotifications, SystemNotificationsRead,
};
use assemble_server::{admin, system_notifications};
use common::*;
use reqwest::StatusCode;

fn storage() -> AdminStorage {
    AdminStorage {
        content_bytes: 80,
        budget_bytes: 100,
        database_bytes: Some(4096),
        disk_available_bytes: Some(5 * 1024 * 1024 * 1024),
        warning_percent: 80,
        budget_warning: true,
        disk_warning: false,
        top_accounts: Vec::new(),
    }
}
async fn list(browser: &Browser) -> SystemNotifications {
    let response = browser
        .get("/v1/admin/notifications")
        .header("origin", &browser.origin)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let raw = response.text().await.unwrap();
    for secret in ["password", "token", "sha256", "digest", "dataDir"] {
        assert!(!raw.contains(secret));
    }
    serde_json::from_str(&raw).unwrap()
}
async fn read(
    browser: &Browser,
    session: &AdminSession,
    id: &str,
    revision: u64,
) -> SystemNotifications {
    let response = browser
        .http
        .post(format!("{}/v1/admin/notifications/read", browser.origin))
        .header("origin", &browser.origin)
        .header("x-csrf-token", &session.csrf_token)
        .json(&SystemNotificationsRead {
            items: vec![SystemNotificationRead {
                id: id.into(),
                revision,
            }],
        })
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.json().await.unwrap()
}
async fn session(browser: &Browser) -> AdminSession {
    browser
        .get("/v1/admin/session")
        .header("origin", &browser.origin)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

#[tokio::test]
async fn notifications_are_admin_only_and_reads_require_same_origin_and_csrf() {
    let mut f = fixture().await;
    system_notifications::observe_storage(&f.server.state.pool, &storage())
        .await
        .unwrap();
    assert_eq!(
        send(
            f.browser
                .get("/v1/admin/notifications")
                .header("origin", &f.browser.origin)
        )
        .await
        .status,
        StatusCode::UNAUTHORIZED
    );
    log_in(&f.browser).await;
    assert_eq!(
        send(
            f.browser
                .get("/v1/admin/notifications")
                .header("origin", &f.browser.origin)
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
    admin::grant_super_admin(&f.server.state.pool, EMAIL)
        .await
        .unwrap();
    let session = session(&f.browser).await;
    let item = list(&f.browser).await.items.remove(0);
    for (origin, csrf) in [
        (None, Some(session.csrf_token.as_str())),
        (
            Some("https://other.example"),
            Some(session.csrf_token.as_str()),
        ),
        (Some(f.browser.origin.as_str()), None),
        (Some(f.browser.origin.as_str()), Some("wrong")),
    ] {
        let mut request = f
            .browser
            .http
            .post(format!("{}/v1/admin/notifications/read", f.browser.origin))
            .json(&SystemNotificationsRead {
                items: vec![SystemNotificationRead {
                    id: item.id.clone(),
                    revision: item.revision,
                }],
            });
        if let Some(origin) = origin {
            request = request.header("origin", origin);
        }
        if let Some(csrf) = csrf {
            request = request.header("x-csrf-token", csrf);
        }
        assert_eq!(
            request.send().await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
    }
    assert!(list(&f.browser).await.items[0].read_at.is_none());
    assert_eq!(
        reqwest::Client::new()
            .get(format!("{}/v1/admin/notifications", f.browser.origin))
            .bearer_auth("synthetic-device-token")
            .header("origin", &f.browser.origin)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.browser
            .get("/v1/admin/notifications")
            .header("origin", "https://other.example")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    f.server.stop().await;
}

#[tokio::test]
async fn sampling_deduplicates_recovery_is_unread_and_two_admins_have_independent_receipts() {
    let mut f = fixture().await;
    admin::grant_super_admin(&f.server.state.pool, EMAIL)
        .await
        .unwrap();
    log_in(&f.browser).await;
    let first_admin = session(&f.browser).await;
    assemble_server::accounts::create(
        &f.server.state.pool,
        "second-admin",
        "第二位管理员",
        assemble_server::SecretString::new(PASSWORD),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE accounts SET email='second@example.test',email_verified_at=1 WHERE username='second-admin'").execute(&f.server.state.pool).await.unwrap();
    admin::grant_super_admin(&f.server.state.pool, "second@example.test")
        .await
        .unwrap();
    let other = Browser::new(&f.browser.origin);
    assert_eq!(
        try_login(&other, "second@example.test", PASSWORD, "/admin/")
            .await
            .status,
        StatusCode::SEE_OTHER
    );
    let mut sample = storage();
    system_notifications::observe_storage(&f.server.state.pool, &sample)
        .await
        .unwrap();
    let first = list(&f.browser).await.items.remove(0);
    assert!(
        read(&f.browser, &first_admin, &first.id, first.revision)
            .await
            .items[0]
            .read_at
            .is_some()
    );
    assert!(list(&other).await.items[0].read_at.is_none());
    sample.content_bytes = 90;
    system_notifications::observe_storage(&f.server.state.pool, &sample)
        .await
        .unwrap();
    let updated = list(&f.browser).await;
    assert_eq!(updated.items.len(), 1);
    assert_eq!(updated.items[0].id, first.id);
    assert!(updated.items[0].read_at.is_some());
    sample.content_bytes = 20;
    sample.budget_warning = false;
    system_notifications::observe_storage(&f.server.state.pool, &sample)
        .await
        .unwrap();
    let recovered = list(&f.browser).await.items.remove(0);
    assert!(recovered.resolved_at.is_some());
    assert!(recovered.read_at.is_none());
    assert!(
        read(&f.browser, &first_admin, &first.id, first.revision)
            .await
            .items[0]
            .read_at
            .is_none()
    );
    assert!(
        read(&f.browser, &first_admin, &first.id, recovered.revision)
            .await
            .items[0]
            .read_at
            .is_some()
    );
    f.server.stop().await;
    let reopened = assemble_server::db::open(f.dir.path()).await.unwrap();
    let persisted = system_notifications::list(&reopened, &first_admin.account.id)
        .await
        .unwrap();
    assert_eq!(persisted.items[0].id, first.id);
    assert!(persisted.items[0].read_at.is_some());
    assert!(persisted.items[0].resolved_at.is_some());
    reopened.close().await;
}
