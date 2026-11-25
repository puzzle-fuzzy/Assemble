//! 静态管理壳可公开下载，但目录外文件和账户 API 始终受各自边界保护。

use assemble_server::{Config, db, http::TestServer};
use reqwest::{Client, StatusCode, redirect};

#[tokio::test]
async fn admin_assets_are_same_origin_and_do_not_fall_back_for_missing_paths() {
    let dir = tempfile::tempdir().unwrap();
    let assets = dir.path().join("admin");
    std::fs::create_dir_all(assets.join("assets")).unwrap();
    std::fs::write(
        assets.join("index.html"),
        "<!doctype html><title>管理端</title>",
    )
    .unwrap();
    std::fs::write(assets.join("assets/app.js"), "console.log('admin')").unwrap();
    std::fs::write(dir.path().join("outside.txt"), "private-test-data").unwrap();
    let mut config = Config::development(dir.path().join("data"));
    config.admin_assets_dir = assets;
    let pool = db::open(&config.data_dir).await.unwrap();
    let mut server = TestServer::start(config, pool).await.unwrap();
    let client = Client::builder()
        .redirect(redirect::Policy::none())
        .build()
        .unwrap();
    let origin = server.origin.as_str();
    let response = client.get(format!("{origin}/admin")).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);
    assert_eq!(response.headers()["location"], "/admin/");
    for path in ["/admin/", "/admin/assets/app.js"] {
        let response = client.get(format!("{origin}{path}")).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        assert_eq!(
            response.headers()["cross-origin-resource-policy"],
            "same-origin"
        );
        let policy = response.headers()["content-security-policy"]
            .to_str()
            .unwrap();
        assert!(policy.contains("script-src 'self'"));
        assert!(policy.contains("frame-ancestors 'none'"));
        assert!(!response.text().await.unwrap().contains("private-test-data"));
    }
    for path in [
        "/admin/assets/missing.js",
        "/admin/no-such-page",
        "/admin/%2e%2e/outside.txt",
    ] {
        let response = client.get(format!("{origin}{path}")).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        assert!(!response.text().await.unwrap().contains("private-test-data"));
    }
    let response = client
        .get(format!("{origin}/v1/admin/session"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    server.stop().await;
}
