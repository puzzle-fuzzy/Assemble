//! 仅在临时数据库和回环网络里验证管理权限、持久化与显式退出。
#![cfg_attr(test, allow(clippy::pedantic))]
mod common;
use assemble_protocol::{
    admin::{AdminPage, AdminSession},
    dht::{DhtDetails, DhtMode, DhtObservation, DhtStatus, MetadataStatus},
};
use assemble_server::{Config, admin, dht::store, http::TestServer};
use common::*;
use reqwest::{RequestBuilder, StatusCode};
use serde_json::{Value, json};
use std::sync::atomic::Ordering;
use std::time::Duration;

const HASH: &str = "0123456789012345678901234567890123456789";

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
fn write(f: &Fixture, action: &str, csrf: &str, body: Value) -> RequestBuilder {
    f.browser
        .http
        .post(format!("{}/v1/admin/dht/{action}", f.browser.origin))
        .header("origin", &f.browser.origin)
        .header("x-csrf-token", csrf)
        .json(&body)
}

#[tokio::test]
async fn every_dht_route_requires_a_verified_same_origin_web_administrator() {
    let mut f = fixture().await;
    for path in ["status", "observations", &format!("observations/{HASH}")] {
        let response = f
            .browser
            .get(&format!("/v1/admin/dht/{path}"))
            .header("origin", &f.browser.origin)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
    for action in ["query", "control"] {
        assert_eq!(
            write(&f, action, "wrong", json!({}))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    log_in(&f.browser).await;
    assert_eq!(
        f.browser
            .get("/v1/admin/dht/status")
            .header("origin", &f.browser.origin)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    let session = owner(&f).await;
    assert_eq!(
        f.browser
            .get("/v1/admin/dht/status")
            .header("origin", "https://other.example")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        write(&f, "control", "wrong", json!({"mode":"collecting"}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        write(
            &f,
            "control",
            &session.csrf_token,
            json!({"mode":"collecting"})
        )
        .send()
        .await
        .unwrap()
        .status(),
        StatusCode::CONFLICT
    );
    let token = "synthetic-dht-test-device";
    assemble_server::devices::register(
        &f.server.state.pool,
        &session.account.id,
        &assemble_protocol::device::NewDevice {
            name: "测试设备".into(),
            platform: assemble_protocol::primitives::Platform::Windows,
        },
        &assemble_server::secrets::token_digest(token),
    )
    .await
    .unwrap();
    assert_eq!(
        reqwest::Client::new()
            .get(format!("{}/v1/admin/dht/status", f.browser.origin))
            .bearer_auth(token)
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
async fn observations_search_and_details_keep_evidence_and_untrusted_text_separate() {
    let mut f = fixture().await;
    owner(&f).await;
    assert!(
        store::observe(&f.server.state.pool, HASH, true, 10)
            .await
            .unwrap()
    );
    assert!(
        store::observe(&f.server.state.pool, HASH, false, 10)
            .await
            .unwrap()
    );
    let observation: DhtDetails = f
        .browser
        .get(&format!("/v1/admin/dht/observations/{HASH}"))
        .header("origin", &f.browser.origin)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(observation.observation.announce_count, 1);
    assert_eq!(observation.observation.sample_count, 1);
    assert!(observation.files.is_empty());
    assert_eq!(observation.observation.status, MetadataStatus::Pending);
    let page: AdminPage<DhtObservation> = f
        .browser
        .get("/v1/admin/dht/observations?q=012345&source=sample&page=999&pageSize=10")
        .header("origin", &f.browser.origin)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!((page.total, page.page), (1, 1));
    let empty: AdminPage<DhtObservation> = f
        .browser
        .get("/v1/admin/dht/observations?q=%25")
        .header("origin", &f.browser.origin)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(empty.total, 0);
    let status: DhtStatus = f
        .browser
        .get("/v1/admin/dht/status")
        .header("origin", &f.browser.origin)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let serialized = serde_json::to_string(&status).unwrap();
    for forbidden in [
        "peerAddress",
        "listen",
        "token",
        "password",
        f.dir.path().to_str().unwrap(),
    ] {
        assert!(!serialized.contains(forbidden));
    }
    assert_eq!((status.total, status.announces, status.samples), (1, 1, 1));
    for query in ["status=unknown", "source=unknown", "page=0", "pageSize=101"] {
        assert_eq!(
            f.browser
                .get(&format!("/v1/admin/dht/observations?{query}"))
                .header("origin", &f.browser.origin)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    f.server.stop().await;
}

#[tokio::test]
async fn capacity_claiming_retry_and_restart_do_not_lose_work_or_reset_evidence() {
    let f = fixture().await;
    for index in 0..2001 {
        store::observe(&f.server.state.pool, &format!("{index:040x}"), false, 3000)
            .await
            .unwrap();
    }
    let counts: Vec<(String, i64)> = sqlx::query_as(
        "SELECT status,count(*) FROM dht_observations GROUP BY status ORDER BY status",
    )
    .fetch_all(&f.server.state.pool)
    .await
    .unwrap();
    assert_eq!(
        counts,
        vec![("deferred".into(), 1), ("pending".into(), 2000)]
    );
    assert!(
        !store::observe(&f.server.state.pool, HASH, true, 2001)
            .await
            .unwrap()
    );
    let first = store::claim(&f.server.state.pool).await.unwrap().unwrap();
    let second = store::claim(&f.server.state.pool).await.unwrap().unwrap();
    assert_ne!(first, second);
    store::finish(
        &f.server.state.pool,
        &first,
        Err(assemble_dht::wire::Failure::Validation),
    )
    .await
    .unwrap();
    store::promote(&f.server.state.pool).await.unwrap();
    let invalid = store::detail(&f.server.state.pool, &assemble_dht::hash_text(&first))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(invalid.observation.status, MetadataStatus::Invalid);
    store::observe(
        &f.server.state.pool,
        &assemble_dht::hash_text(&first),
        true,
        3000,
    )
    .await
    .unwrap();
    assert_eq!(
        store::detail(&f.server.state.pool, &assemble_dht::hash_text(&first))
            .await
            .unwrap()
            .unwrap()
            .observation
            .attempts,
        1
    );
    let error = store::enqueue_query(&f.server.state.pool, &assemble_dht::hash_text(&first), 3000)
        .await
        .unwrap_err();
    assert_eq!(error.code, "DHT_RETRY_COOLDOWN");
    let state = f.server.state.clone();
    let mut server = f.server;
    server.stop().await;
    let interrupted = store::detail(&state.pool, &assemble_dht::hash_text(&second))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(interrupted.observation.status, MetadataStatus::Pending);
}

#[tokio::test]
async fn real_service_starts_pauses_stops_and_releases_its_udp_port_without_public_traffic() {
    let dir = tempfile::tempdir().unwrap();
    let reserve = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let listen = reserve.local_addr().unwrap();
    drop(reserve);
    let mut config = Config::development(dir.path());
    config.dht.enabled = true;
    config.dht.listen = listen;
    config.dht.bootstrap_hosts.clear();
    let pool = assemble_server::db::open(dir.path()).await.unwrap();
    let server = TestServer::start(config, pool).await.unwrap();
    assemble_server::accounts::create(
        &server.state.pool,
        USERNAME,
        "测试管理员",
        assemble_server::SecretString::new(PASSWORD),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE accounts SET email = ?, email_verified_at = 1 WHERE username = ?")
        .bind(EMAIL)
        .bind(USERNAME)
        .execute(&server.state.pool)
        .await
        .unwrap();
    let browser = Browser::new(server.origin.as_str());
    let mut f = Fixture {
        dir,
        server,
        browser,
    };
    let session = owner(&f).await;
    assert_eq!(
        write(
            &f,
            "control",
            &session.csrf_token,
            json!({"mode":"collecting"})
        )
        .send()
        .await
        .unwrap()
        .status(),
        StatusCode::OK
    );
    wait_running(&f, true).await;
    assert!(tokio::net::UdpSocket::bind(listen).await.is_err());
    let response = write(
        &f,
        "query",
        &session.csrf_token,
        json!({"query":format!("magnet:?xt=urn:btih:{HASH}")}),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let detail: DhtDetails = response.json().await.unwrap();
    assert_eq!(
        (
            detail.observation.announce_count,
            detail.observation.sample_count
        ),
        (0, 0)
    );
    assert_eq!(
        write(
            &f,
            "query",
            &session.csrf_token,
            json!({"query":"../invalid"})
        )
        .send()
        .await
        .unwrap()
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        write(
            &f,
            "control",
            &session.csrf_token,
            json!({"mode":"draining"})
        )
        .send()
        .await
        .unwrap()
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        store::mode(&f.server.state.pool).await.unwrap(),
        DhtMode::Draining
    );
    assert_eq!(
        write(
            &f,
            "control",
            &session.csrf_token,
            json!({"mode":"stopped"})
        )
        .send()
        .await
        .unwrap()
        .status(),
        StatusCode::OK
    );
    wait_running(&f, false).await;
    let socket = tokio::net::UdpSocket::bind(listen).await.unwrap();
    drop(socket);
    assert_eq!(
        write(&f, "query", &session.csrf_token, json!({"query":HASH}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    let audit: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action = 'dht.mode_changed'")
            .fetch_one(&f.server.state.pool)
            .await
            .unwrap();
    assert_eq!(audit, 3);
    f.server.stop().await;
}
async fn wait_running(f: &Fixture, expected: bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if f.server.state.dht.running.load(Ordering::Relaxed) == expected {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn verified_metadata_is_published_atomically_and_a_duplicate_completion_is_ignored() {
    let mut f = fixture().await;
    owner(&f).await;
    store::observe(&f.server.state.pool, HASH, true, 1000)
        .await
        .unwrap();
    let hash = store::claim(&f.server.state.pool).await.unwrap().unwrap();
    let metadata = assemble_dht::metadata::Metadata {
        name: "<img src=x onerror=alert(1)>".into(),
        files: vec![assemble_protocol::dht::DhtFile {
            path: vec!["sample.txt".into()],
            length: 3,
        }],
        total_size: 3,
        piece_length: 16384,
        private: false,
        comment: None,
        source: None,
    };
    assert!(
        store::finish(&f.server.state.pool, &hash, Ok(Some(metadata.clone())))
            .await
            .unwrap()
    );
    assert!(
        !store::finish(&f.server.state.pool, &hash, Ok(Some(metadata)))
            .await
            .unwrap()
    );
    let detail: DhtDetails = f
        .browser
        .get(&format!("/v1/admin/dht/observations/{HASH}"))
        .header("origin", &f.browser.origin)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(detail.observation.status, MetadataStatus::Verified);
    assert_eq!(detail.observation.total_size, Some(3));
    assert_eq!(detail.observation.file_count, Some(1));
    assert_eq!(detail.files[0].length, 3);
    assert!(detail.observation.magnet_uri.contains("%3Cimg"));
    assert!(detail.observation.fetched_at.is_some());
    f.server.stop().await;
}
