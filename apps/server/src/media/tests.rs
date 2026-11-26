use super::*;
use crate::http::{AppState, Shared};
use assemble_protocol::media::*;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Cursor;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

fn png() -> Vec<u8> {
    let image = image::DynamicImage::new_rgba8(12, 16);
    let mut buffer = Cursor::new(Vec::new());
    image
        .write_to(&mut buffer, image::ImageFormat::Png)
        .unwrap();
    buffer.into_inner()
}
fn content(status: MediaStatus) -> MediaContent {
    MediaContent {
        media_type: MediaType::Movie,
        title: "电影".into(),
        original_title: "A Film".into(),
        year: Some(2026),
        release_date: Some("2026-11-25".into()),
        overview: "简介".into(),
        genres: vec!["剧情".into()],
        episode_count: None,
        status,
        watched_at: Some("2026-11-25".into()),
        review: Some("review-secret".into()),
        notes: Some("notes-secret".into()),
        magnet_uri: Some("magnet:?xt=urn:btih:0123456789012345678901234567890123456789".into()),
    }
}

async fn account(pool: &sqlx::SqlitePool, id: &str, admin: bool) {
    sqlx::query("INSERT INTO accounts(id,username,display_name,password_hash,created_at,email,email_verified_at,is_super_admin) VALUES(?,?,?,'unused',?,?,?,?)").bind(id).bind(id).bind(id).bind(now()).bind(format!("{id}@example.test")).bind(now()).bind(admin).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO web_sessions(id,account_id,token_hash,csrf_token,created_at,expires_at) VALUES(?,?,?,?,?,?)").bind(format!("session-{id}")).bind(id).bind(crate::secrets::token_digest(&format!("cookie-{id}"))).bind(format!("csrf-{id}")).bind(now()).bind(now()+3_600_000).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO devices(id,account_id,name,platform,token_hash,created_at) VALUES(?,?,?,'windows',?,?)").bind(format!("device-{id}")).bind(id).bind(id).bind(crate::secrets::token_digest(&format!("bearer-{id}"))).bind(now()).execute(pool).await.unwrap();
}
struct Harness {
    _dir: tempfile::TempDir,
    state: Shared,
    origin: String,
    client: reqwest::Client,
    stop: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}
impl Harness {
    async fn start(providers: Option<Arc<providers::Providers>>) -> Self {
        Self::start_seeded(providers, false).await
    }
    async fn start_seeded(providers: Option<Arc<providers::Providers>>, persisted: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open(dir.path()).await.unwrap();
        for (id, admin) in [("admin", true), ("alice", false), ("bob", false)] {
            account(&pool, id, admin).await;
        }
        if persisted {
            seed_jobs(&pool).await;
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let mut config = crate::Config::development(dir.path())
            .with_public_origin(assemble_protocol::origin::ServerOrigin::parse(&origin).unwrap());
        config.media_web_assets_dir = dir.path().join("movies");
        std::fs::create_dir_all(&config.media_web_assets_dir).unwrap();
        std::fs::write(
            config.media_web_assets_dir.join("index.html"),
            "<h1>影视</h1>",
        )
        .unwrap();
        let mut state = AppState::new(pool, config);
        if let Some(providers) = providers {
            state.media = Arc::new(jobs::Runtime::with_providers(providers));
        }
        let state = Arc::new(state);
        let stop = CancellationToken::new();
        let shared = Arc::clone(&state);
        let until = stop.clone();
        let task = tokio::spawn(async move {
            crate::http::serve_until(listener, shared, until, std::time::Duration::from_secs(4))
                .await
                .unwrap();
        });
        let harness = Self {
            _dir: dir,
            state,
            origin,
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            stop,
            task,
        };
        assert_eq!(harness.get("/healthz").send().await.unwrap().status(), 200);
        harness
    }

    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.client.get(format!("{}{path}", self.origin))
    }
    fn write(&self, method: reqwest::Method, path: &str, user: &str) -> reqwest::RequestBuilder {
        self.client
            .request(method, format!("{}{path}", self.origin))
            .header("cookie", format!("assemble_web=cookie-{user}"))
            .header("origin", &self.origin)
            .header("x-csrf-token", format!("csrf-{user}"))
    }
    fn cookie(&self, path: &str, user: &str) -> reqwest::RequestBuilder {
        self.get(path)
            .header("cookie", format!("assemble_web=cookie-{user}"))
    }
    async fn stop(self) {
        self.stop.cancel();
        self.task.await.unwrap();
        self.state.pool.close().await;
    }
}
async fn value(response: reqwest::Response) -> Value {
    let status = response.status();
    let bytes = response.bytes().await.unwrap();
    assert!(
        status.is_success(),
        "{status} {}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice(&bytes).unwrap()
}
async fn create(
    h: &Harness,
    scope: MediaScope,
    user: &str,
    status: MediaStatus,
    key: &str,
) -> Value {
    let path = if scope == MediaScope::Shared {
        "/v1/admin/media/entries"
    } else {
        "/v1/media/private/entries"
    };
    value(
        h.write(reqwest::Method::POST, path, user)
            .json(&MediaCreate {
                content: content(status),
                request_id: key.into(),
            })
            .send()
            .await
            .unwrap(),
    )
    .await["item"]
        .clone()
}

#[tokio::test]
async fn public_projection_private_ownership_and_all_statuses_are_enforced_over_http() {
    let h = Harness::start(None).await;
    let mut ids = Vec::new();
    for (status, key) in [
        (MediaStatus::Planned, "planned"),
        (MediaStatus::Watching, "watching"),
        (MediaStatus::Completed, "completed"),
    ] {
        let entry = create(&h, MediaScope::Shared, "admin", status, key).await;
        ids.push(entry["id"].as_str().unwrap().to_owned());
        create(
            &h,
            MediaScope::Private,
            "alice",
            status,
            &format!("private-{key}"),
        )
        .await;
    }
    let public = value(h.get("/v1/media/shared/entries").send().await.unwrap()).await;
    assert_eq!(public["total"], 3);
    assert_eq!(
        public["counts"],
        json!({"all":3,"planned":1,"watching":1,"completed":1})
    );
    for entry in public["items"].as_array().unwrap() {
        assert!(entry["magnetUri"].is_null());
        assert!(entry["notes"].is_null());
        assert_eq!(entry["magnetAvailable"], true);
        assert_eq!(entry["review"], "review-secret");
    }
    let path = format!("/v1/media/shared/entries/{}", ids[0]);
    let logged = value(h.cookie(&path, "alice").send().await.unwrap()).await;
    assert!(
        logged["item"]["magnetUri"]
            .as_str()
            .unwrap()
            .starts_with("magnet:")
    );
    assert!(logged["item"]["notes"].is_null());
    let admin = value(h.cookie(&path, "admin").send().await.unwrap()).await;
    assert_eq!(admin["item"]["notes"], "notes-secret");
    assert_eq!(
        h.get("/v1/media/private/entries")
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let private = value(
        h.cookie("/v1/media/private/entries", "alice")
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(private["total"], 3);
    let id = private["items"][0]["id"].as_str().unwrap();
    let private_path = format!("/v1/media/private/entries/{id}");
    assert_eq!(
        h.cookie(&private_path, "bob")
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        h.cookie(&format!("{private_path}/gallery"), "bob")
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        h.write(reqwest::Method::PUT, &private_path, "bob")
            .json(&MediaUpdate {
                version: 1,
                content: content(MediaStatus::Completed)
            })
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        h.write(reqwest::Method::DELETE, &private_path, "bob")
            .json(&MediaVersion { version: 1 })
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    let stats = value(
        h.cookie("/v1/admin/media/statistics", "admin")
            .header("sec-fetch-site", "same-origin")
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(stats["total"], 3);
    assert_eq!(stats["byMonth"], json!([{"month":"2026-11","count":1}]));
    assert_eq!(
        h.cookie("/v1/admin/media/statistics", "alice")
            .header("sec-fetch-site", "same-origin")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    h.stop().await;
}

#[tokio::test]
async fn cookies_require_origin_csrf_admin_device_cannot_write_shared_and_replays_are_exact() {
    let h = Harness::start(None).await;
    let body = MediaCreate {
        content: content(MediaStatus::Planned),
        request_id: "same-request".into(),
    };
    let path = "/v1/admin/media/entries";
    for request in [
        h.client
            .post(format!("{}{path}", h.origin))
            .header("cookie", "assemble_web=cookie-admin"),
        h.write(reqwest::Method::POST, path, "admin")
            .header("origin", "https://evil.test"),
        h.write(reqwest::Method::POST, path, "admin")
            .header("x-csrf-token", "wrong"),
    ] {
        assert_eq!(request.json(&body).send().await.unwrap().status(), 403);
    }
    assert_eq!(
        h.client
            .post(format!("{}{path}", h.origin))
            .bearer_auth("bearer-admin")
            .json(&body)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        h.write(reqwest::Method::POST, path, "alice")
            .json(&body)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let first = value(
        h.write(reqwest::Method::POST, path, "admin")
            .json(&body)
            .send()
            .await
            .unwrap(),
    )
    .await;
    let repeated = value(
        h.write(reqwest::Method::POST, path, "admin")
            .json(&body)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(first, repeated);
    let mut changed = body.clone();
    changed.content.title = "Changed".into();
    let conflict = h
        .write(reqwest::Method::POST, path, "admin")
        .json(&changed)
        .send()
        .await
        .unwrap();
    assert_eq!(conflict.status(), 409);
    assert_eq!(
        conflict.json::<Value>().await.unwrap()["error"]["code"],
        "IDEMPOTENCY_CONFLICT"
    );
    let id = first["item"]["id"].as_str().unwrap();
    let update = format!("{path}/{id}");
    let result = value(
        h.write(reqwest::Method::PUT, &update, "admin")
            .json(&MediaUpdate {
                version: 1,
                content: content(MediaStatus::Watching),
            })
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(result["item"]["version"], 2);
    let stale = h
        .write(reqwest::Method::DELETE, &update, "admin")
        .json(&MediaVersion { version: 1 })
        .send()
        .await
        .unwrap();
    assert_eq!(stale.status(), 409);
    assert_eq!(
        stale.json::<Value>().await.unwrap()["error"]["code"],
        "VERSION_CONFLICT"
    );
    let private = value(
        h.client
            .post(format!("{}/v1/media/private/entries", h.origin))
            .bearer_auth("bearer-alice")
            .json(&body)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(private["item"]["scope"], "private");
    let principal = auth::Principal {
        account_id: "alice".into(),
        credential_kind: "device".into(),
        credential_id: "device-alice".into(),
        admin: false,
        csrf: None,
    };
    sqlx::query("DELETE FROM devices WHERE id = 'device-alice'")
        .execute(&h.state.pool)
        .await
        .unwrap();
    let result = store::create(
        &h.state.pool,
        MediaScope::Private,
        &principal,
        MediaCreate {
            content: content(MediaStatus::Planned),
            request_id: "revoked".into(),
        },
    )
    .await;
    assert_eq!(result.unwrap_err().status, 401);
    h.stop().await;
}

#[tokio::test]
async fn images_are_not_readable_by_guessing_ids_and_all_variants_have_integrity_headers() {
    let h = Harness::start(None).await;
    let private = create(
        &h,
        MediaScope::Private,
        "alice",
        MediaStatus::Planned,
        "private",
    )
    .await;
    let private_id = private["id"].as_str().unwrap();
    let image = assets::store(&h.state.pool, "alice", png(), false)
        .await
        .unwrap();
    let mut tx = h.state.pool.begin().await.unwrap();
    assets::link(&mut tx, private_id, std::slice::from_ref(&image), "gallery")
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let path = format!("/v1/media/assets/{}/content", image.asset_id);
    assert_eq!(h.get(&path).send().await.unwrap().status(), 404);
    assert_eq!(h.cookie(&path, "bob").send().await.unwrap().status(), 404);
    let orphan = assets::store(&h.state.pool, "bob", png(), false)
        .await
        .unwrap();
    assert_eq!(
        h.cookie(
            &format!("/v1/media/assets/{}/content", orphan.asset_id),
            "bob"
        )
        .send()
        .await
        .unwrap()
        .status(),
        404
    );
    for variant in ["original", "200", "360", "480"] {
        let response = h
            .cookie(&format!("{path}?variant={variant}"), "alice")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let sha = response.headers()["x-content-sha256"]
            .to_str()
            .unwrap()
            .to_owned();
        let length = response.content_length().unwrap();
        let body = response.bytes().await.unwrap();
        assert_eq!(length, body.len() as u64);
        assert_eq!(
            sha,
            assemble_protocol::primitives::to_hex(&Sha256::digest(&body))
        );
    }
    let shared = create(
        &h,
        MediaScope::Shared,
        "admin",
        MediaStatus::Planned,
        "shared",
    )
    .await;
    let shared_id = shared["id"].as_str().unwrap();
    let mut tx = h.state.pool.begin().await.unwrap();
    assets::link(&mut tx, shared_id, std::slice::from_ref(&image), "gallery")
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(h.get(&path).send().await.unwrap().status(), 200);
    assert!(
        assets::store(
            &h.state.pool,
            "alice",
            b"<svg><script/></svg>".to_vec(),
            false
        )
        .await
        .is_err()
    );
    assert!(
        assets::store(
            &h.state.pool,
            "alice",
            vec![0; assets::MAX_IMAGE_BYTES + 1],
            false
        )
        .await
        .is_err()
    );
    assert_eq!(
        h.get(&format!("{path}?variant=../../file"))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    let cached = assets::store(&h.state.pool, "catalog", png(), true)
        .await
        .unwrap();
    let cache_path = format!("/v1/media/assets/{}/content", cached.asset_id);
    assert_eq!(h.get(&cache_path).send().await.unwrap().status(), 200);
    sqlx::query("UPDATE media_catalog_assets SET expires_at = 0 WHERE asset_id = ?")
        .bind(cached.asset_id)
        .execute(&h.state.pool)
        .await
        .unwrap();
    assert_eq!(h.get(&cache_path).send().await.unwrap().status(), 404);
    h.stop().await;
}

#[derive(Debug)]
struct RewriteTransport {
    origin: url::Url,
    inner: providers::HttpTransport,
}
impl providers::Transport for RewriteTransport {
    fn get(
        &self,
        url: url::Url,
        headers: HeaderMap,
        maximum: usize,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<providers::Reply, crate::error::ApiError>>
                + Send
                + '_,
        >,
    > {
        let mut local = self.origin.clone();
        local.set_path(&format!("/{}{}", url.host_str().unwrap(), url.path()));
        local.set_query(url.query());
        self.inner.get(local, headers, maximum)
    }
}
struct SourceFixture {
    providers: Arc<providers::Providers>,
    stop: CancellationToken,
    task: tokio::task::JoinHandle<()>,
    calls: Arc<tokio::sync::Mutex<Vec<String>>>,
}

#[derive(Debug)]
struct BlockedTransport {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
impl providers::Transport for BlockedTransport {
    fn get(
        &self,
        _url: url::Url,
        _headers: HeaderMap,
        _maximum: usize,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<providers::Reply, crate::error::ApiError>>
                + Send
                + '_,
        >,
    > {
        Box::pin(async move {
            self.entered.notify_one();
            self.release.notified().await;
            Ok(providers::Reply {
                status: 200,
                content_type: "application/json".into(),
                bytes: serde_json::to_vec(&json!({"id":42,"title":"电影","credits":{"cast":[]}}))
                    .unwrap(),
            })
        })
    }
}

#[tokio::test]
async fn jobs_recheck_revocation_before_publication_and_shutdown_records_cancellation() {
    for revoke in [true, false] {
        let transport = Arc::new(BlockedTransport {
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        });
        let providers = Arc::new(providers::Providers::with_transport(
            MediaConfig {
                tmdb_token: Some(assemble_store::secret::SecretString::new("test")),
            },
            transport.clone(),
        ));
        let h = Harness::start(Some(providers)).await;
        let job: MediaJob = serde_json::from_value(
            value(
                h.write(reqwest::Method::POST, "/v1/media/private/imports", "alice")
                    .json(&MediaImport {
                        identity: MediaIdentity {
                            provider: MediaProvider::Tmdb,
                            media_type: MediaType::Movie,
                            external_id: "42".into(),
                        },
                        request_id: "blocked".into(),
                    })
                    .send()
                    .await
                    .unwrap(),
            )
            .await,
        )
        .unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            transport.entered.notified(),
        )
        .await
        .unwrap();
        if revoke {
            sqlx::query("DELETE FROM web_sessions WHERE id = 'session-alice'")
                .execute(&h.state.pool)
                .await
                .unwrap();
            transport.release.notify_one();
            let principal = auth::Principal {
                account_id: "alice".into(),
                credential_kind: "device".into(),
                credential_id: "device-alice".into(),
                admin: false,
                csrf: None,
            };
            let mut final_job = None;
            for _ in 0..100 {
                let current = jobs::get(&h.state.pool, MediaScope::Private, &principal, &job.id)
                    .await
                    .unwrap();
                if current.status == MediaJobStatus::Failed {
                    final_job = Some(current);
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            assert_eq!(
                final_job.unwrap().error_code.as_deref(),
                Some("UNAUTHORIZED")
            );
        } else {
            h.state.media.shutdown(&h.state.pool).await;
            let data: String = sqlx::query_scalar("SELECT data FROM media_jobs WHERE id = ?")
                .bind(&job.id)
                .fetch_one(&h.state.pool)
                .await
                .unwrap();
            let final_job: MediaJob = decode(&data).unwrap();
            assert_eq!(final_job.status, MediaJobStatus::Cancelled);
        }
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM media_entries")
            .fetch_one(&h.state.pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
        h.stop().await;
    }
}

#[tokio::test]
async fn startup_resumes_queued_jobs_and_marks_in_flight_work_interrupted() {
    let source = SourceFixture::start().await;
    let h = Harness::start_seeded(Some(source.providers.clone()), true).await;
    let queued = wait_job(&h, "/v1/media/private/jobs/persisted-queued", "alice").await;
    assert_eq!(queued.status, MediaJobStatus::Completed);
    let running = wait_job(&h, "/v1/media/private/jobs/persisted-running", "alice").await;
    assert_eq!(running.status, MediaJobStatus::Interrupted);
    assert_eq!(running.error_code.as_deref(), Some("MEDIA_JOB_INTERRUPTED"));
    h.stop().await;
    source.stop().await;
}

#[tokio::test]
async fn missing_configuration_and_source_redirects_fail_without_exposing_credentials() {
    let h = Harness::start(None).await;
    let capabilities = value(h.get("/v1/media/capabilities").send().await.unwrap()).await;
    assert_eq!(
        capabilities["providers"],
        json!({"tmdb":false,"douban":true})
    );
    let response = h
        .get("/v1/media/catalog/details?provider=tmdb&mediaType=movie&externalId=42")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 503);
    assert_eq!(
        response.json::<Value>().await.unwrap()["error"]["code"],
        "TMDB_NOT_CONFIGURED"
    );
    h.stop().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = url::Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let app = Router::new().route(
        "/{*path}",
        axum::routing::get(|| async {
            (
                StatusCode::TEMPORARY_REDIRECT,
                [("location", "https://evil.test/token-sink")],
                "sensitive upstream diagnostic",
            )
        }),
    );
    let stop = CancellationToken::new();
    let until = stop.clone();
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(until.cancelled_owned())
            .await
            .unwrap();
    });
    let providers = providers::Providers::with_transport(
        MediaConfig::default(),
        Arc::new(RewriteTransport {
            origin,
            inner: providers::HttpTransport {
                client: reqwest::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .build()
                    .unwrap(),
            },
        }),
    );
    let error = providers
        .image(&providers::SourceImage {
            provider: MediaProvider::Douban,
            url: "https://img3.doubanio.com/view/photo/l/public/p9.png".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, "DOUBAN_VERIFICATION_REQUIRED");
    assert!(!error.to_string().contains("sensitive"));
    assert!(!error.to_string().contains("token-sink"));
    stop.cancel();
    task.await.unwrap();
}
impl SourceFixture {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin =
            url::Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let calls = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let app = Router::new()
            .route("/{*path}", axum::routing::get(source_response))
            .with_state(calls.clone());
        let stop = CancellationToken::new();
        let until = stop.clone();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(until.cancelled_owned())
                .await
                .unwrap();
        });
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(2))
            .build()
            .unwrap();
        let providers = Arc::new(providers::Providers::with_transport(
            MediaConfig {
                tmdb_token: Some(assemble_store::secret::SecretString::new("fixture-token")),
            },
            Arc::new(RewriteTransport {
                origin,
                inner: providers::HttpTransport { client },
            }),
        ));
        Self {
            providers,
            stop,
            task,
            calls,
        }
    }
    async fn stop(self) {
        self.stop.cancel();
        self.task.await.unwrap();
    }
}
async fn source_response(
    State(calls): State<Arc<tokio::sync::Mutex<Vec<String>>>>,
    Path(path): Path<String>,
    Query(query): Query<BTreeMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    assert!(!headers.contains_key("cookie"));
    if path.starts_with("api.tmdb.org/") {
        assert_eq!(headers["authorization"], "Bearer fixture-token");
    } else {
        assert!(!headers.contains_key("authorization"));
    }
    calls.lock().await.push(path.clone());
    if path.starts_with("image.tmdb.org/") || path.starts_with("img3.doubanio.com/") {
        return ([("content-type", "image/png")], png()).into_response();
    }
    let tmdb = json!({"id":42,"title":"来源电影","overview":"来源简介","original_title":"Source Film","release_date":"2026-01-02","poster_path":"/poster.png","genres":[{"id":18,"name":"剧情"}],"credits":{"cast":[{"id":7,"name":"演员","character":"主角","profile_path":"/actor.png"}]}});
    let douban = json!({"id":42,"type":"movie","title":"豆瓣电影","intro":"豆瓣简介","year":"2026","pubdate":["2026-01-02(中国大陆)"],"cover":{"image":{"large":{"url":"https://img3.doubanio.com/view/photo/l/public/p9.png"}}},"genres":["剧情"],"actors":["演员"]});
    let result = if path == "api.tmdb.org/3/search/movie" {
        json!({"results":[tmdb.clone()]})
    } else if path == "api.tmdb.org/3/movie/42" {
        tmdb
    } else if path == "api.tmdb.org/3/movie/42/images" {
        json!({"id":42,"posters":[{"file_path":"/poster.png","width":12,"height":16}]})
    } else if path == "api.tmdb.org/3/person/7" {
        json!({"id":7,"name":"演员","profile_path":"/actor.png","movie_credits":{"cast":[tmdb]},"tv_credits":{"cast":[]}})
    } else if path == "m.douban.com/rexxar/api/v2/search/movie" {
        json!({"items":[{"target_type":"movie","target":douban.clone()}]})
    } else if path == "m.douban.com/rexxar/api/v2/movie/42" {
        douban
    } else if path == "m.douban.com/rexxar/api/v2/movie/42/celebrities" {
        json!({"actors":[{"id":7,"name":"演员","role":"主角","avatar":{"large":"https://img3.doubanio.com/view/personage/m/public/actor.png"}}]})
    } else if path == "m.douban.com/rexxar/api/v2/celebrity/7" {
        json!({"id":999,"name":"演员","avatar":{"large":"https://img3.doubanio.com/view/personage/m/public/actor.png"}})
    } else if path == "m.douban.com/rexxar/api/v2/celebrity/7/works" {
        let start = query.get("start").unwrap().parse::<u64>().unwrap();
        let works:Vec<_>=(start..11.min(start+10)).map(|n|json!({"roles":["演员 (饰 主角)"],"work":{"id":42+n,"type":"movie","title":format!("作品{n}"),"year":"2026"}})).collect();
        json!({"start":start,"count":10,"total":11,"works":works})
    } else if path == "movie.douban.com/subject/42/photos" {
        return ([("content-type","text/html")],"<h1>电影 海报</h1><a href='/subject/42/'>电影</a><ul class='poster-col3'><li data-id='9'></li></ul>").into_response();
    } else if path == "m.douban.com/rexxar/api/v2/photo/9" {
        json!({"id":9,"owner_uri":"douban://douban.com/movie/42","photo_type":"poster","image":{"large":{"url":"https://img3.doubanio.com/view/photo/l/public/p9.png"}}})
    } else if let Some(id) = path
        .strip_prefix("api.tmdb.org/3/movie/")
        .and_then(|v| v.parse::<u64>().ok())
    {
        json!({"id":id,"title":format!("资料{id}"),"credits":{"cast":[]}})
    } else {
        return StatusCode::NOT_FOUND.into_response();
    };
    Json(result).into_response()
}

#[tokio::test]
async fn both_sources_search_metadata_cast_actor_pagination_and_gallery_use_real_http_fixtures() {
    let source = SourceFixture::start().await;
    for provider in [MediaProvider::Tmdb, MediaProvider::Douban] {
        let identity = MediaIdentity {
            provider,
            media_type: MediaType::Movie,
            external_id: "42".into(),
        };
        let found = source
            .providers
            .search(provider, MediaType::Movie, "电影")
            .await
            .unwrap();
        assert_eq!(found.len(), 1);
        assert!(found[0].poster.is_none());
        let details = source.providers.details(&identity).await.unwrap();
        assert!(!details.content.overview.is_empty());
        assert_eq!(details.content.genres, vec!["剧情"]);
        assert_eq!(details.cast.len(), 1);
        assert_eq!(
            details.cast[0].cast.identity.as_ref().unwrap().external_id,
            "7"
        );
        let bytes = source
            .providers
            .image(details.poster.as_ref().unwrap())
            .await
            .unwrap();
        assert_eq!(bytes, png());
        let (actor, photo, credits) = source
            .providers
            .actor(
                &MediaPersonIdentity {
                    provider,
                    external_id: "7".into(),
                },
                MediaType::Movie,
            )
            .await
            .unwrap();
        assert_eq!(actor.identity.external_id, "7");
        assert!(photo.is_some());
        assert_eq!(
            credits.len(),
            if provider == MediaProvider::Tmdb {
                1
            } else {
                11
            }
        );
        let images = source.providers.gallery(&identity).await.unwrap();
        assert_eq!(images.len(), 1);
    }
    assert!(
        source
            .calls
            .lock()
            .await
            .iter()
            .any(|p| p.ends_with("celebrity/7/works"))
    );
    source.stop().await;
}

#[tokio::test]
async fn public_catalog_cache_avoids_refetching_images_and_its_global_budget_returns_retry_after() {
    let source = SourceFixture::start().await;
    let h = Harness::start(Some(source.providers.clone())).await;
    let path = "/v1/media/catalog/details?provider=tmdb&mediaType=movie&externalId=42";
    let first = value(h.get(path).send().await.unwrap()).await;
    let before = source.calls.lock().await.len();
    let second = value(h.get(path).send().await.unwrap()).await;
    assert_eq!(first, second);
    assert_eq!(source.calls.lock().await.len(), before);
    let actor_path = "/v1/media/catalog/actors/tmdb/7?mediaType=movie";
    value(h.get(actor_path).send().await.unwrap()).await;
    let before = source.calls.lock().await.len();
    value(h.get(actor_path).send().await.unwrap()).await;
    assert_eq!(source.calls.lock().await.len(), before);
    // 两条未缓存的公开资料已经消耗预算；其余 58 条均发到真实的来源替身 HTTP 服务。
    for id in 100..158 {
        let response = h
            .get(&format!(
                "/v1/media/catalog/details?provider=tmdb&mediaType=movie&externalId={id}"
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "id={id}");
    }
    let limited = h
        .get("/v1/media/catalog/details?provider=tmdb&mediaType=movie&externalId=999")
        .send()
        .await
        .unwrap();
    assert_eq!(limited.status(), 429);
    assert_eq!(limited.headers()["retry-after"], "60");
    assert_eq!(
        limited.json::<Value>().await.unwrap()["error"]["code"],
        "MEDIA_CATALOG_RATE_LIMITED"
    );
    // 命中的纯资料缓存不占用来源槽，也不消耗外部查询预算。
    assert_eq!(h.get(path).send().await.unwrap().status(), 200);
    h.stop().await;
    source.stop().await;
}

#[tokio::test]
async fn excess_public_metadata_concurrency_is_rejected_without_queueing_external_work() {
    let transport = Arc::new(BlockedTransport {
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    let providers = Arc::new(providers::Providers::with_transport(
        MediaConfig {
            tmdb_token: Some(assemble_store::secret::SecretString::new("test")),
        },
        transport.clone(),
    ));
    let h = Harness::start(Some(providers)).await;
    let url = format!(
        "{}/v1/media/catalog/details?provider=tmdb&mediaType=movie&externalId=42",
        h.origin
    );
    let client = h.client.clone();
    let first_url = url.clone();
    let first = tokio::spawn(async move { client.get(first_url).send().await.unwrap() });
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        transport.entered.notified(),
    )
    .await
    .unwrap();
    let client = h.client.clone();
    let second_url = url.clone();
    let second = tokio::spawn(async move { client.get(second_url).send().await.unwrap() });
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        transport.entered.notified(),
    )
    .await
    .unwrap();
    let third = tokio::time::timeout(std::time::Duration::from_secs(1), h.client.get(url).send())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(third.status(), 429);
    assert_eq!(third.headers()["retry-after"], "60");
    transport.release.notify_waiters();
    assert_eq!(first.await.unwrap().status(), 200);
    assert_eq!(second.await.unwrap().status(), 200);
    h.stop().await;
}
async fn wait_job(h: &Harness, path: &str, user: &str) -> MediaJob {
    for _ in 0..200 {
        let mut request = h.cookie(path, user);
        if path.starts_with("/v1/admin/") {
            request = request.header("sec-fetch-site", "same-origin");
        }
        let response = value(request.send().await.unwrap()).await;
        let job: MediaJob = serde_json::from_value(response).unwrap();
        if !matches!(job.status, MediaJobStatus::Queued | MediaJobStatus::Running) {
            return job;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("job did not finish")
}

#[tokio::test]
async fn import_jobs_are_durable_idempotent_isolated_and_publish_only_verified_images() {
    let source = SourceFixture::start().await;
    let h = Harness::start(Some(source.providers.clone())).await;
    let identity = MediaIdentity {
        provider: MediaProvider::Tmdb,
        media_type: MediaType::Movie,
        external_id: "42".into(),
    };
    let input = MediaImport {
        identity: identity.clone(),
        request_id: "import-one".into(),
    };
    let url = "/v1/media/private/imports";
    let queued: MediaJob = serde_json::from_value(
        value(
            h.write(reqwest::Method::POST, url, "alice")
                .json(&input)
                .send()
                .await
                .unwrap(),
        )
        .await,
    )
    .unwrap();
    let path = format!("/v1/media/private/jobs/{}", queued.id);
    assert_eq!(h.cookie(&path, "bob").send().await.unwrap().status(), 404);
    let done = wait_job(&h, &path, "alice").await;
    assert_eq!(done.status, MediaJobStatus::Completed);
    let entry_id = done.entry_id.unwrap();
    let replay = value(
        h.write(reqwest::Method::POST, url, "alice")
            .json(&input)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(replay["id"], queued.id);
    let first = h
        .write(reqwest::Method::POST, url, "alice")
        .json(&MediaImport {
            identity: identity.clone(),
            request_id: "concurrent-a".into(),
        })
        .send();
    let second = h
        .write(reqwest::Method::POST, url, "alice")
        .json(&MediaImport {
            identity: identity.clone(),
            request_id: "concurrent-b".into(),
        })
        .send();
    let (a, b) = tokio::join!(first, second);
    for response in [a.unwrap(), b.unwrap()] {
        let job: MediaJob = serde_json::from_value(value(response).await).unwrap();
        let done = wait_job(&h, &format!("/v1/media/private/jobs/{}", job.id), "alice").await;
        assert_eq!(done.entry_id.as_deref(), Some(entry_id.as_str()));
    }
    let list = value(
        h.cookie("/v1/media/private/entries", "alice")
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(list["total"], 1);
    let entry = &list["items"][0];
    assert_eq!(entry["cast"][0]["name"], "演员");
    let asset = entry["poster"]["assetId"].as_str().unwrap();
    assert_eq!(
        h.get(&format!("/v1/media/assets/{asset}/content"))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    let sync_path = format!("/v1/media/private/entries/{entry_id}/gallery/sync");
    let sync: MediaJob = serde_json::from_value(
        value(
            h.write(reqwest::Method::POST, &sync_path, "alice")
                .json(&MediaSync {
                    version: 1,
                    request_id: "sync-one".into(),
                })
                .send()
                .await
                .unwrap(),
        )
        .await,
    )
    .unwrap();
    let done = wait_job(&h, &format!("/v1/media/private/jobs/{}", sync.id), "alice").await;
    assert_eq!(done.status, MediaJobStatus::Completed);
    let gallery = value(
        h.cookie(
            &format!("/v1/media/private/entries/{entry_id}/gallery"),
            "alice",
        )
        .send()
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(gallery["version"], 2);
    assert_eq!(gallery["items"].as_array().unwrap().len(), 1);
    let catalog = value(
        h.get("/v1/media/catalog/details?provider=tmdb&mediaType=movie&externalId=42")
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert!(catalog["item"].get("scope").is_none());
    assert!(catalog["item"].get("notes").is_none());
    let asset = catalog["item"]["poster"]["assetId"].as_str().unwrap();
    assert_eq!(
        h.get(&format!("/v1/media/assets/{asset}/content"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    h.stop().await;
    source.stop().await;
}

#[tokio::test]
async fn collections_cannot_contain_private_entries_and_nested_projections_remove_secrets() {
    let h = Harness::start(None).await;
    let shared = create(
        &h,
        MediaScope::Shared,
        "admin",
        MediaStatus::Completed,
        "shared",
    )
    .await;
    let private = create(
        &h,
        MediaScope::Private,
        "admin",
        MediaStatus::Completed,
        "private",
    )
    .await;
    let bad = MediaCollectionCreate {
        title: "精选".into(),
        description: "简介".into(),
        entry_ids: vec![private["id"].as_str().unwrap().into()],
        request_id: "bad-collection".into(),
    };
    assert_eq!(
        h.write(
            reqwest::Method::POST,
            "/v1/admin/media/collections",
            "admin"
        )
        .json(&bad)
        .send()
        .await
        .unwrap()
        .status(),
        404
    );
    let good = MediaCollectionCreate {
        entry_ids: vec![shared["id"].as_str().unwrap().into()],
        request_id: "good-collection".into(),
        ..bad
    };
    let collection = value(
        h.write(
            reqwest::Method::POST,
            "/v1/admin/media/collections",
            "admin",
        )
        .json(&good)
        .send()
        .await
        .unwrap(),
    )
    .await;
    let id = collection["item"]["id"].as_str().unwrap();
    let public = value(
        h.get(&format!("/v1/media/collections/{id}"))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert!(public["item"]["items"][0]["magnetUri"].is_null());
    assert!(public["item"]["items"][0]["notes"].is_null());
    let logged = value(
        h.cookie(&format!("/v1/media/collections/{id}"), "alice")
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert!(logged["item"]["items"][0]["magnetUri"].is_string());
    assert!(logged["item"]["items"][0]["notes"].is_null());
    let movies = h.get("/movies/").send().await.unwrap();
    assert_eq!(movies.status(), 200);
    assert!(movies.headers().contains_key("content-security-policy"));
    assert!(movies.text().await.unwrap().contains("影视"));
    assert_eq!(h.get("/movies").send().await.unwrap().status(), 308);
    for path in [
        "/movies/private",
        "/movies/entries/shared/film-1",
        "/movies/entries/private/film-1",
        "/movies/actors/tmdb/7?mediaType=movie",
        "/movies/catalog/douban/movie/42",
        "/movies/collections",
        "/movies/collections/collection-1",
    ] {
        let response = h.get(path).send().await.unwrap();
        assert_eq!(response.status(), 200, "{path}");
        assert!(
            response.headers()["content-type"]
                .to_str()
                .unwrap()
                .starts_with("text/html")
        );
        assert!(response.headers().contains_key("content-security-policy"));
        assert!(response.text().await.unwrap().contains("影视"));
        let head = h
            .client
            .head(format!("{}{path}", h.origin))
            .send()
            .await
            .unwrap();
        assert_eq!(head.status(), 200);
        assert!(head.headers().contains_key("content-security-policy"));
        assert!(head.bytes().await.unwrap().is_empty());
    }
    for path in [
        "/movies/assets/missing.js",
        "/movies/missing.css",
        "/movies/catalog/evil/movie/42",
        "/movies/unknown/path",
    ] {
        assert_eq!(h.get(path).send().await.unwrap().status(), 404, "{path}");
    }
    h.stop().await;
}

#[tokio::test]
async fn movies_only_cache_successful_fingerprinted_static_assets() {
    let h = Harness::start(None).await;
    let directory = &h.state.config.media_web_assets_dir;
    std::fs::create_dir(directory.join("assets")).unwrap();
    for path in [
        "assets/index-A1b2_C-d.js",
        "assets/index-AbCd1234.css",
        "assets/geist-latin-DC-KSUi6.woff2",
        "assets/poster-aBc123_-.webp",
        "assets/index.js",
        "assets/index-short.js",
        "assets/index-A1b2_C-d.html",
        "display-preferences.js",
    ] {
        std::fs::write(directory.join(path), "fixture").unwrap();
    }
    for path in [
        "/movies/assets/index-A1b2_C-d.js?v=1",
        "/movies/assets/index-AbCd1234.css",
        "/movies/assets/geist-latin-DC-KSUi6.woff2",
        "/movies/assets/poster-aBc123_-.webp",
    ] {
        let response = h.get(path).send().await.unwrap();
        assert_eq!(response.status(), 200, "{path}");
        assert_eq!(
            response.headers()["cache-control"],
            "public, max-age=31536000, immutable",
            "{path}"
        );
        assert!(response.headers().contains_key("content-security-policy"));
    }
    let path = "/movies/assets/index-A1b2_C-d.js";
    let head = h
        .client
        .head(format!("{}{path}", h.origin))
        .send()
        .await
        .unwrap();
    assert_eq!(head.status(), 200);
    assert_eq!(
        head.headers()["cache-control"],
        "public, max-age=31536000, immutable"
    );
    let modified = head.headers()["last-modified"].clone();
    assert!(head.bytes().await.unwrap().is_empty());
    let unchanged = h
        .get(path)
        .header("if-modified-since", modified)
        .send()
        .await
        .unwrap();
    assert_eq!(unchanged.status(), 304);
    assert_eq!(
        unchanged.headers()["cache-control"],
        "public, max-age=31536000, immutable"
    );
    let range = h
        .get(path)
        .header("range", "bytes=0-2")
        .send()
        .await
        .unwrap();
    assert_eq!(range.status(), 206);
    assert_eq!(
        range.headers()["cache-control"],
        "public, max-age=31536000, immutable"
    );
    for (path, status) in [
        ("/movies/", 200),
        ("/movies/private", 200),
        ("/movies/display-preferences.js", 200),
        ("/movies/assets/index.js", 200),
        ("/movies/assets/index-short.js", 200),
        ("/movies/assets/index-A1b2_C-d.html", 200),
        ("/movies/assets/missing-AbCd1234.js", 404),
        ("/movies/assets/index-A1b2_C-d.js/unknown", 404),
    ] {
        let response = h.get(path).send().await.unwrap();
        assert_eq!(response.status(), status, "{path}");
        assert_eq!(response.headers()["cache-control"], "no-store", "{path}");
    }
    let invalid_range = h
        .get(path)
        .header("range", "bytes=999-1000")
        .send()
        .await
        .unwrap();
    assert_eq!(invalid_range.status(), 416);
    assert_eq!(invalid_range.headers()["cache-control"], "no-store");
    // 静态缓存层不应延伸到带会话投影的 API 或私人资产。
    for path in [
        "/v1/media/shared/entries",
        "/v1/media/private/entries",
        "/v1/media/assets/missing/content",
    ] {
        let response = h.cookie(path, "alice").send().await.unwrap();
        assert_ne!(
            response
                .headers()
                .get("cache-control")
                .and_then(|value| value.to_str().ok()),
            Some("public, max-age=31536000, immutable"),
            "{path}"
        );
    }
    h.stop().await;
}

#[test]
fn source_ids_and_image_hosts_are_bounded() {
    assert!(providers::validate_external_id("42").is_ok());
    for id in [
        "0",
        "01",
        "../42",
        "https://localhost",
        "18446744073709551616",
    ] {
        assert!(providers::validate_external_id(id).is_err());
    }
    for url in [
        "http://image.tmdb.org/t/p/original/a.jpg",
        "https://image.tmdb.org.attacker.test/t/p/a.jpg",
        "https://image.tmdb.org:443/t/p/a.jpg?token=x",
        "https://127.0.0.1/private",
    ] {
        assert!(
            providers::validate_image_url(&providers::SourceImage {
                provider: assemble_protocol::media::MediaProvider::Tmdb,
                url: url.into()
            })
            .is_err()
        );
    }
}

async fn seed_jobs(pool: &sqlx::SqlitePool) {
    for (id, status) in [
        ("persisted-queued", MediaJobStatus::Queued),
        ("persisted-running", MediaJobStatus::Running),
    ] {
        let job = MediaJob {
            id: id.into(),
            scope: MediaScope::Private,
            status,
            stage: Some(MediaJobStage::Metadata),
            completed_items: None,
            total_items: None,
            entry_id: None,
            error_code: None,
        };
        let input = jobs::Input::Import {
            identity: MediaIdentity {
                provider: MediaProvider::Tmdb,
                media_type: MediaType::Movie,
                external_id: "42".into(),
            },
        };
        sqlx::query("INSERT INTO media_jobs(id,actor_id,owner_key,credential_kind,credential_id,data,input,created_at,updated_at) VALUES(?,'alice','alice','web','session-alice',?,?,?,?)").bind(id).bind(encode(&job).unwrap()).bind(encode(&input).unwrap()).bind(now()).bind(now()).execute(pool).await.unwrap();
    }
}
