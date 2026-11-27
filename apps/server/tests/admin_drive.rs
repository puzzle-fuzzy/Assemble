//! 管理云盘读取跨账户目录和真实磁盘余量，使用临时目录、真实上传与本机 HTTP 服务。
mod common;

use assemble_client::{OutgoingFile, TransferOptions};
use assemble_protocol::admin::{AdminDriveFile, AdminPage, AdminStorage};
use assemble_protocol::primitives::Platform;
use assemble_server::{SecretString, accounts, admin};
use common::*;
use reqwest::StatusCode;
use serde_json::Value;

async fn read(f: &Fixture, path: &str) -> Value {
    let response = f
        .browser
        .get(path)
        .header("origin", &f.browser.origin)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(
        response.headers()["cross-origin-resource-policy"],
        "same-origin"
    );
    response.json().await.unwrap()
}

async fn seeded() -> (Fixture, Registered, Vec<String>, u64) {
    let f = fixture().await;
    admin::grant_super_admin(&f.server.state.pool, EMAIL)
        .await
        .unwrap();
    let device = register(&f, 'a', "测试电脑", Platform::Windows).await;
    let path = f.dir.path().join("upload.txt");
    tokio::fs::write(&path, b"cloud storage contents")
        .await
        .unwrap();
    let outgoing = OutgoingFile::open(&path, &device.cancel).await.unwrap();
    let stored = device
        .drive_upload(
            "合同.txt",
            &outgoing,
            TransferOptions::new(&device.cancel),
            None,
        )
        .await
        .unwrap();
    let other = accounts::create(
        &f.server.state.pool,
        "other",
        "Other owner",
        SecretString::new(PASSWORD),
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE accounts SET email = 'private@example.test', suspended_at = 1 WHERE id = ?",
    )
    .bind(&other.id)
    .execute(&f.server.state.pool)
    .await
    .unwrap();
    for (id, name) in [("a-file", "预算%合同.pdf"), ("z-file", "archive.zip")] {
        sqlx::query("INSERT INTO drive_files(id,account_id,name,size,mime_type,sha256,created_at) VALUES (?,?,?, ?,NULL,?,1)")
            .bind(id).bind(&other.id).bind(name).bind(i64::try_from(stored.size).unwrap()).bind(&stored.sha256)
            .execute(&f.server.state.pool).await.unwrap();
    }
    sqlx::query("UPDATE drive_files SET created_at = 1")
        .execute(&f.server.state.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE blobs SET refcount = refcount + 2 WHERE sha256 = ?")
        .bind(&stored.sha256)
        .execute(&f.server.state.pool)
        .await
        .unwrap();
    let mut ids = vec![stored.id, "a-file".into(), "z-file".into()];
    ids.sort_by(|a, b| b.cmp(a));
    (f, device, ids, stored.size)
}

#[tokio::test]
async fn all_accounts_are_listed_with_stable_pages_and_only_verified_public_metadata() {
    let (mut f, _, ids, size) = seeded().await;
    let all = read(&f, "/v1/admin/drive/files").await;
    let page: AdminPage<AdminDriveFile> = serde_json::from_value(all.clone()).unwrap();
    assert_eq!(page.total, 3);
    assert_eq!(
        page.items.iter().map(|file| &file.id).collect::<Vec<_>>(),
        ids.iter().collect::<Vec<_>>()
    );
    assert_eq!(
        page.items
            .iter()
            .filter(|file| file.account_display_name == "Other owner")
            .count(),
        2
    );
    for file in all["items"].as_array().unwrap() {
        assert_eq!(file.as_object().unwrap().len(), 8);
        for field in ["sha256", "path", "passwordHash", "token", "csrfToken"] {
            assert!(file.get(field).is_none());
        }
        assert_eq!(file["createdAt"], "1970-01-01T00:00:00.001Z");
    }
    assert!(!all.to_string().contains("private@example.test"));
    for (index, id) in ids.iter().enumerate() {
        let part = read(
            &f,
            &format!("/v1/admin/drive/files?page={}&pageSize=1", index + 1),
        )
        .await;
        assert_eq!(part["items"][0]["id"], *id);
        assert_eq!(part["total"], 3);
    }
    for (query, expected) in [
        ("OTHER", 2),
        ("alex%40example.test", 1),
        ("private%40example.test", 0),
        ("%25", 1),
        ("absent", 0),
    ] {
        assert_eq!(
            read(&f, &format!("/v1/admin/drive/files?q={query}")).await["total"],
            expected
        );
    }
    assert_eq!(
        read(&f, "/v1/admin/drive/files?page=4000000000&pageSize=100").await["items"],
        serde_json::json!([])
    );
    let storage: AdminStorage =
        serde_json::from_value(read(&f, "/v1/admin/storage").await).unwrap();
    assert_eq!(
        storage.content_bytes, size,
        "shared contents count once even with three file references"
    );
    assert!(storage.database_bytes.is_some_and(|bytes| bytes > 0));
    assert!(storage.disk_available_bytes.is_some_and(|bytes| bytes > 0));
    assert_eq!(
        storage.budget_bytes,
        f.server.state.config.storage_budget_bytes
    );
    f.server.stop().await;
}

#[tokio::test]
async fn verified_administrators_download_cross_account_files_with_native_browser_headers_and_ranges()
 {
    let (mut f, device, _, _) = seeded().await;
    let path = "/v1/admin/drive/files/a-file/content";
    let response = f
        .browser
        .get(path)
        .header("sec-fetch-site", "same-origin")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(
        response.headers()["cross-origin-resource-policy"],
        "same-origin"
    );
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    let disposition = response.headers()["content-disposition"].to_str().unwrap();
    assert!(disposition.starts_with("attachment; filename*=UTF-8''"));
    assert!(disposition.contains("%25"));
    assert_eq!(
        response.bytes().await.unwrap().as_ref(),
        b"cloud storage contents"
    );
    let part = f
        .browser
        .get(path)
        .header("origin", &f.browser.origin)
        .header("range", "bytes=0-4")
        .send()
        .await
        .unwrap();
    assert_eq!(part.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(part.bytes().await.unwrap().as_ref(), b"cloud");
    assert_eq!(
        f.browser
            .get("/v1/admin/drive/files/missing/content")
            .header("origin", &f.browser.origin)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    let ordinary = reqwest::Client::new()
        .get(format!(
            "{}/v1/drive/files/a-file/content",
            f.browser.origin
        ))
        .bearer_auth(device.token.expose())
        .send()
        .await
        .unwrap();
    assert_eq!(
        ordinary.status(),
        StatusCode::NOT_FOUND,
        "ordinary device access stays scoped to its own account"
    );
    f.server.stop().await;
}

#[tokio::test]
async fn administrator_download_rejects_anonymous_device_cross_site_unverified_and_revoked_sessions()
 {
    let (mut f, device, _, _) = seeded().await;
    let path = "/v1/admin/drive/files/a-file/content";
    for token in [None, Some(device.token.expose())] {
        let request = reqwest::Client::new()
            .get(format!("{}{path}", f.browser.origin))
            .header("origin", &f.browser.origin);
        let response = if let Some(token) = token {
            request.bearer_auth(token)
        } else {
            request
        }
        .send()
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(!response.headers().contains_key("content-disposition"));
        assert!(
            !response
                .text()
                .await
                .unwrap()
                .contains("cloud storage contents")
        );
    }
    for (origin, site) in [
        (None, None),
        (Some("https://foreign.example.test"), Some("same-origin")),
        (None, Some("cross-site")),
        (None, Some("same-site")),
    ] {
        let mut request = f.browser.get(path);
        if let Some(origin) = origin {
            request = request.header("origin", origin);
        }
        if let Some(site) = site {
            request = request.header("sec-fetch-site", site);
        }
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(!response.headers().contains_key("content-disposition"));
    }
    for change in [
        "UPDATE accounts SET is_super_admin = 0 WHERE email = ?",
        "UPDATE accounts SET is_super_admin = 1, email_verified_at = NULL WHERE email = ?",
    ] {
        sqlx::query(change)
            .bind(EMAIL)
            .execute(&f.server.state.pool)
            .await
            .unwrap();
        assert_eq!(
            f.browser
                .get(path)
                .header("origin", &f.browser.origin)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    sqlx::query("UPDATE accounts SET is_super_admin = 1, email_verified_at = 1 WHERE email = ?")
        .bind(EMAIL)
        .execute(&f.server.state.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE web_sessions SET expires_at = 0")
        .execute(&f.server.state.pool)
        .await
        .unwrap();
    assert_eq!(
        f.browser
            .get(path)
            .header("origin", &f.browser.origin)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    f.server.stop().await;
}

#[tokio::test]
async fn cloud_inventory_remains_read_only_same_origin_and_inaccessible_to_device_tokens() {
    let (mut f, device, _, _) = seeded().await;
    for path in ["/v1/admin/drive/files", "/v1/admin/storage"] {
        assert_eq!(
            f.browser
                .get(path)
                .header("origin", "https://foreign.example.test")
                .header("sec-fetch-site", "same-origin")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            f.browser.get(path).send().await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            reqwest::Client::new()
                .get(format!("{}{path}", f.browser.origin))
                .header("origin", &f.browser.origin)
                .bearer_auth(device.token.expose())
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            send(f.browser.post_form(path, &[])).await.status,
            StatusCode::METHOD_NOT_ALLOWED
        );
    }
    for query in [
        "page=0",
        "pageSize=0",
        "pageSize=101",
        "page=-1",
        "q=%0Aline",
    ] {
        assert_eq!(
            f.browser
                .get(&format!("/v1/admin/drive/files?{query}"))
                .header("origin", &f.browser.origin)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        f.browser
            .get(&format!("/v1/admin/drive/files?q={}", "a".repeat(201)))
            .header("origin", &f.browser.origin)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM drive_files")
        .fetch_one(&f.server.state.pool)
        .await
        .unwrap();
    assert_eq!(count, 3);
    f.server.stop().await;
}
