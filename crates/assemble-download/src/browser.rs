//! Chrome 原生宿主与桌面进程之间的本机通道；端口和随机凭据只存系统钥匙串。
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use assemble_protocol::browser::{BrowserDownload, BrowserReply};
use assemble_protocol::primitives::{is_valid_id, to_hex};
use assemble_store::secret::{SecretKey, SecretStore, SecretString};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Query, State, rejection::JsonRejection},
    http::{HeaderMap, Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::DownloadManager;

mod assistant_routes;

pub const NOT_CONNECTED: &str = "BROWSER_NOT_CONNECTED";
pub const KEYCHAIN_UNAVAILABLE: &str = "BROWSER_KEYCHAIN_UNAVAILABLE";
pub const INVALID_REQUEST: &str = "BROWSER_INVALID_REQUEST";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Endpoint {
    port: u16,
    token: SecretStringValue,
}

// 不实现 Debug，序列化的唯一用途是立即交给 SecretStore。
#[derive(Serialize, Deserialize)]
#[serde(transparent)]
struct SecretStringValue(String);

fn key(app_id: &str) -> SecretKey {
    SecretKey::new("assemble.browser.bridge.v1", app_id)
}
fn equal(a: &str, b: &str) -> bool {
    a.len() == b.len() && bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}
fn proof(token: &str, nonce: &str) -> Option<String> {
    let mut mac = Hmac::<Sha256>::new_from_slice(token.as_bytes()).ok()?;
    mac.update(b"assemble-browser-server-v1:");
    mac.update(nonce.as_bytes());
    Some(to_hex(&mac.finalize().into_bytes()))
}

struct BridgeState {
    token: SecretString,
    manager: Arc<DownloadManager>,
    accepted: tokio::sync::Mutex<HashMap<String, (String, Instant, String)>>,
    selected: Arc<dyn Fn() + Send + Sync>,
}

pub struct BrowserServer {
    cancel: CancellationToken,
    handle: Mutex<Option<JoinHandle<()>>>,
    store: Arc<dyn SecretStore>,
    key: SecretKey,
    stored: SecretString,
}

/// 把这一轮的桥接凭据放进去。上一回没来得及删的还留着时（崩溃、被强杀、换新版时没能正常
/// 退出），直接改写它，macOS 会要人输登录钥匙串的密码，应用就停在启动上（2026-11-27 在这台
/// Mac 上真的遇到了）。先删掉再新建：删除和新建都不用问人。
fn replace_credential(
    store: &dyn SecretStore,
    key: &SecretKey,
    stored: &SecretString,
) -> Result<(), &'static str> {
    store.delete(key).map_err(|_| KEYCHAIN_UNAVAILABLE)?;
    store.put(key, stored).map_err(|_| KEYCHAIN_UNAVAILABLE)
}

impl BrowserServer {
    pub async fn start(
        manager: Arc<DownloadManager>,
        store: Arc<dyn SecretStore>,
        app_id: &str,
        selected: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self, &'static str> {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|_| NOT_CONNECTED)?;
        let port = listener.local_addr().map_err(|_| NOT_CONNECTED)?.port();
        let mut bytes = [0_u8; 32];
        getrandom::fill(&mut bytes).map_err(|_| NOT_CONNECTED)?;
        let token = to_hex(&bytes);
        let endpoint = Endpoint {
            port,
            token: SecretStringValue(token.clone()),
        };
        let stored =
            SecretString::new(serde_json::to_string(&endpoint).map_err(|_| NOT_CONNECTED)?);
        let key = key(app_id);
        replace_credential(store.as_ref(), &key, &stored)?;
        let state = Arc::new(BridgeState {
            token: SecretString::new(token),
            manager,
            accepted: tokio::sync::Mutex::new(HashMap::new()),
            selected: Arc::new(selected),
        });
        let protected = Router::new()
            .route("/v1/download", post(download))
            .route("/v1/status", get(status))
            .merge(assistant_routes::router())
            .layer(middleware::from_fn_with_state(
                Arc::clone(&state),
                authorize,
            ));
        let router = Router::new()
            .route("/v1/challenge", get(challenge))
            .merge(protected)
            .layer(DefaultBodyLimit::max(65_536))
            .with_state(state);
        let cancel = CancellationToken::new();
        let stopped = cancel.clone();
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(stopped.cancelled_owned())
                .await;
        });
        Ok(Self {
            cancel,
            handle: Mutex::new(Some(handle)),
            store,
            key,
            stored,
        })
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    pub async fn shutdown(&self) {
        self.cancel();
        let handle = self
            .handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(mut handle) = handle
            && tokio::time::timeout(Duration::from_secs(5), &mut handle)
                .await
                .is_err()
        {
            handle.abort();
            let _ = handle.await;
        }
        if let Ok(Some(current)) = self.store.get(&self.key)
            && equal(current.expose(), self.stored.expose())
        {
            let _ = self.store.delete(&self.key);
        }
    }
}

async fn authorize(
    State(state): State<Arc<BridgeState>>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let authorized = !request.headers().contains_key("origin")
        && request
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
            .is_some_and(|v| equal(v, state.token.expose()));
    if !authorized {
        return (
            StatusCode::FORBIDDEN,
            Json(BrowserReply::failure(NOT_CONNECTED)),
        )
            .into_response();
    }
    match tokio::time::timeout(Duration::from_secs(45), next.run(request)).await {
        Ok(reply) => reply,
        Err(_) => (
            StatusCode::REQUEST_TIMEOUT,
            Json(BrowserReply::failure(NOT_CONNECTED)),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct Challenge {
    nonce: String,
}
async fn challenge(
    State(state): State<Arc<BridgeState>>,
    headers: HeaderMap,
    query: Result<Query<Challenge>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let Ok(Query(query)) = query else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if headers.contains_key("origin")
        || query.nonce.len() != 64
        || !query.nonce.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(proof) = proof(state.token.expose(), &query.nonce) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    Json(serde_json::json!({"version":1,"proof":proof})).into_response()
}
async fn status() -> Json<BrowserReply> {
    Json(BrowserReply::success(None))
}

async fn download(
    State(state): State<Arc<BridgeState>>,
    body: Result<Json<BrowserDownload>, JsonRejection>,
) -> Json<BrowserReply> {
    let Ok(Json(request)) = body else {
        return Json(BrowserReply::failure(INVALID_REQUEST));
    };
    if request.via.is_some() || !is_valid_id(&request.request_id) || request.request_id.len() > 64 {
        return Json(BrowserReply::failure(INVALID_REQUEST));
    }
    let Ok(encoded) = serde_json::to_vec(&request.input) else {
        return Json(BrowserReply::failure(INVALID_REQUEST));
    };
    let digest = to_hex(&Sha256::digest(encoded));
    // 持锁覆盖一次提交，避免两个宿主同时重试同一请求时创建重复任务。
    let mut accepted = state.accepted.lock().await;
    accepted.retain(|_, (_, at, _)| at.elapsed() < Duration::from_mins(10));
    if let Some((previous, _, task)) = accepted.get(&request.request_id) {
        return Json(if previous == &digest {
            BrowserReply::success(Some(task.clone()))
        } else {
            BrowserReply::failure(INVALID_REQUEST)
        });
    }
    let preview = match state.manager.prepare(request.input).await {
        Ok(value) => value,
        Err(error) => return Json(BrowserReply::failure(error.code())),
    };
    let Some(variant) = preview.variants.first() else {
        return Json(BrowserReply::failure(INVALID_REQUEST));
    };
    let id = match state.manager.start(&preview.id, &variant.id).await {
        Ok(value) => value,
        Err(error) => return Json(BrowserReply::failure(error.code())),
    };
    if accepted.len() >= 128
        && let Some(oldest) = accepted
            .iter()
            .min_by_key(|(_, (_, at, _))| *at)
            .map(|(key, _)| key.clone())
    {
        accepted.remove(&oldest);
    }
    accepted.insert(request.request_id, (digest, Instant::now(), id.clone()));
    state.manager.focus_browser_task(&id);
    (state.selected)();
    Json(BrowserReply::success(Some(id)))
}

/// 只有原生宿主调用：先验证正在监听的确实是当前桌面进程，再发送来源。
pub async fn send(
    store: &dyn SecretStore,
    app_id: &str,
    request: Option<&BrowserDownload>,
) -> BrowserReply {
    match exchange(store, app_id, request).await {
        Ok(reply) => reply,
        Err(code) => BrowserReply::failure(code),
    }
}

async fn exchange(
    store: &dyn SecretStore,
    app_id: &str,
    request: Option<&BrowserDownload>,
) -> Result<BrowserReply, &'static str> {
    let (method, path, body) = if let Some(request) = request {
        (
            reqwest::Method::POST,
            "/v1/download",
            Some(serde_json::to_value(request).map_err(|_| INVALID_REQUEST)?),
        )
    } else {
        (reqwest::Method::GET, "/v1/status", None)
    };
    let reply: BrowserReply =
        serde_json::from_value(local_request(store, app_id, method, path, body).await?)
            .map_err(|_| NOT_CONNECTED)?;
    if reply.version != 1 {
        return Err(NOT_CONNECTED);
    }
    Ok(reply)
}

pub(crate) async fn local_request(
    store: &dyn SecretStore,
    app_id: &str,
    method: reqwest::Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> Result<serde_json::Value, &'static str> {
    let stored = store
        .get(&key(app_id))
        .map_err(|_| KEYCHAIN_UNAVAILABLE)?
        .ok_or(NOT_CONNECTED)?;
    let endpoint: Endpoint = serde_json::from_str(stored.expose()).map_err(|_| NOT_CONNECTED)?;
    if endpoint.port == 0 || endpoint.token.0.len() != 64 {
        return Err(NOT_CONNECTED);
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(40))
        .build()
        .map_err(|_| NOT_CONNECTED)?;
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| NOT_CONNECTED)?;
    let nonce = to_hex(&bytes);
    let base = format!("http://127.0.0.1:{}", endpoint.port);
    let check = client
        .get(format!("{base}/v1/challenge?nonce={nonce}"))
        .send()
        .await
        .map_err(|_| NOT_CONNECTED)?;
    let verified: serde_json::Value = small_json(check).await?;
    let expected = proof(&endpoint.token.0, &nonce).ok_or(NOT_CONNECTED)?;
    if !verified
        .get("proof")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|value| equal(value, &expected))
    {
        return Err(NOT_CONNECTED);
    }
    let mut builder = client.request(method, format!("{base}{path}"));
    if let Some(body) = body {
        builder = builder.json(&body);
    }
    let response = builder
        .bearer_auth(&endpoint.token.0)
        .send()
        .await
        .map_err(|_| NOT_CONNECTED)?;
    small_json(response).await
}

async fn small_json<T: serde::de::DeserializeOwned>(
    mut response: reqwest::Response,
) -> Result<T, &'static str> {
    if !response.status().is_success() {
        return Err(NOT_CONNECTED);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| NOT_CONNECTED)? {
        if bytes.len() + chunk.len() > 8192 {
            return Err(NOT_CONNECTED);
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| NOT_CONNECTED)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MediaTools;
    use assemble_protocol::downloads::{DownloadInput, DownloadKind};
    use assemble_store::secret::MemorySecretStore;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 记下调用先后的存储：要验的是"先删后写"，不是在旧凭据上改。
    struct Recording {
        inner: MemorySecretStore,
        calls: Mutex<Vec<&'static str>>,
    }

    impl SecretStore for Recording {
        fn put(&self, key: &SecretKey, value: &SecretString) -> assemble_store::secret::Result<()> {
            self.calls.lock().unwrap().push("put");
            self.inner.put(key, value)
        }

        fn get(&self, key: &SecretKey) -> assemble_store::secret::Result<Option<SecretString>> {
            self.inner.get(key)
        }

        fn delete(&self, key: &SecretKey) -> assemble_store::secret::Result<()> {
            self.calls.lock().unwrap().push("delete");
            self.inner.delete(key)
        }
    }

    #[tokio::test]
    async fn a_leftover_bridge_credential_is_replaced_by_deleting_it_first() {
        let folder = tempfile::tempdir().unwrap();
        let store = Arc::new(Recording {
            inner: MemorySecretStore::new(),
            calls: Mutex::new(Vec::new()),
        });
        // 上一回没删掉的凭据：应用被强杀、崩溃，或者换新版时没能正常退出。
        store
            .put(
                &key("test.leftover"),
                &SecretString::new("{\"port\":1,\"token\":\"old\"}"),
            )
            .unwrap();
        store.calls.lock().unwrap().clear();
        let manager = DownloadManager::open(
            folder.path().join("history.json"),
            folder.path().to_owned(),
            MediaTools::test_available(),
            |_| {},
        )
        .unwrap();

        let bridge = BrowserServer::start(manager, store.clone(), "test.leftover", || {})
            .await
            .unwrap();

        assert_eq!(*store.calls.lock().unwrap(), vec!["delete", "put"]);
        assert!(
            !store
                .get(&key("test.leftover"))
                .unwrap()
                .unwrap()
                .expose()
                .contains("old")
        );
        bridge.shutdown().await;
    }

    #[tokio::test]
    async fn assistant_cancel_only_accepts_its_own_authenticated_task() {
        let folder = tempfile::tempdir().unwrap();
        let store = Arc::new(MemorySecretStore::new());
        let mut manager = DownloadManager::open(
            folder.path().join("history.json"),
            folder.path().to_owned(),
            MediaTools::test_available(),
            |_| {},
        )
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let source = format!(
            "http://127.0.0.1:{}/video.mp4",
            listener.local_addr().unwrap().port()
        );
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route(
                    "/video.mp4",
                    get(|| async { std::future::pending::<StatusCode>().await }),
                ),
            )
            .await
            .unwrap();
        });
        Arc::get_mut(&mut manager).unwrap().network =
            crate::network::Network::test(source.trim_end_matches("/video.mp4").to_owned());
        let bridge =
            BrowserServer::start(Arc::clone(&manager), store.clone(), "test.cancel", || {})
                .await
                .unwrap();
        let input = DownloadInput {
            kind: DownloadKind::Video,
            url: source,
            page_url: None,
            title: None,
        };
        let preview = crate::assistant::preview(store.as_ref(), "test.cancel", input.clone())
            .await
            .preview
            .unwrap();
        let start = assemble_protocol::downloads::AssistantDownloadStart {
            request_id: "cancel-me".into(),
            preview_id: preview.id,
            variant_id: preview.variants[0].id.clone(),
        };
        let id = crate::assistant::start(store.as_ref(), "test.cancel", &start)
            .await
            .task_id
            .unwrap();
        assert!(
            crate::assistant::cancel(store.as_ref(), "test.cancel", &id)
                .await
                .ok
        );
        let ordinary = manager.prepare(input).await.unwrap();
        let forbidden_start = assemble_protocol::downloads::AssistantDownloadStart {
            request_id: "ordinary-preview".into(),
            preview_id: ordinary.id.clone(),
            variant_id: ordinary.variants[0].id.clone(),
        };
        assert_eq!(
            crate::assistant::start(store.as_ref(), "test.cancel", &forbidden_start)
                .await
                .error_code
                .as_deref(),
            Some("DOWNLOAD_EXPIRED")
        );
        let ordinary_id = manager
            .start(&ordinary.id, &ordinary.variants[0].id)
            .await
            .unwrap();
        assert_eq!(
            crate::assistant::status(store.as_ref(), "test.cancel", &ordinary_id)
                .await
                .error_code
                .as_deref(),
            Some("DOWNLOAD_EXPIRED")
        );
        assert_eq!(
            crate::assistant::cancel(store.as_ref(), "test.cancel", &ordinary_id)
                .await
                .error_code
                .as_deref(),
            Some("DOWNLOAD_EXPIRED")
        );
        manager.shutdown().await;
        assert_eq!(
            crate::assistant::status(store.as_ref(), "test.cancel", &id)
                .await
                .task
                .unwrap()
                .phase,
            assemble_protocol::downloads::DownloadPhase::Cancelled
        );
        bridge.shutdown().await;
        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn assistant_requires_two_steps_deduplicates_and_obeys_the_local_switch() {
        use assemble_protocol::downloads::{
            AssistantDownloadStart, DownloadInitiator, DownloadPhase,
        };
        let folder = tempfile::tempdir().unwrap();
        let history = folder.path().join("history.json");
        let store = Arc::new(MemorySecretStore::new());
        let manager = DownloadManager::open(
            history.clone(),
            folder.path().to_owned(),
            MediaTools::test_available(),
            |_| {},
        )
        .unwrap();
        let bridge =
            BrowserServer::start(Arc::clone(&manager), store.clone(), "test.assistant", || {})
                .await
                .unwrap();
        let input = DownloadInput {
            kind: DownloadKind::Video,
            url: "http://127.0.0.1:99/video.mp4?private=source".into(),
            page_url: None,
            title: Some("中文 <影片>".into()),
        };
        manager.set_assistant_allowed(false);
        assert_eq!(
            crate::assistant::preview(store.as_ref(), "test.assistant", input.clone())
                .await
                .error_code
                .as_deref(),
            Some("DOWNLOAD_ASSISTANT_DISABLED")
        );
        assert!(manager.view().tasks.is_empty());
        manager.set_assistant_allowed(true);
        let preview = crate::assistant::preview(store.as_ref(), "test.assistant", input)
            .await
            .preview
            .unwrap();
        assert!(manager.view().tasks.is_empty());
        let start = AssistantDownloadStart {
            request_id: "once".into(),
            preview_id: preview.id,
            variant_id: preview.variants[0].id.clone(),
        };
        let first = crate::assistant::start(store.as_ref(), "test.assistant", &start).await;
        assert!(first.ok);
        let second = crate::assistant::start(store.as_ref(), "test.assistant", &start).await;
        assert_eq!(first.task_id, second.task_id);
        assert_eq!(manager.view().tasks.len(), 1);
        let id = first.task_id.unwrap();
        let reply = crate::assistant::status(store.as_ref(), "test.assistant", &id).await;
        assert_eq!(
            reply.task.unwrap().initiated_by,
            Some(DownloadInitiator::Assistant)
        );
        let changed = AssistantDownloadStart {
            variant_id: "changed".into(),
            ..start
        };
        assert_eq!(
            crate::assistant::start(store.as_ref(), "test.assistant", &changed)
                .await
                .error_code
                .as_deref(),
            Some(INVALID_REQUEST)
        );
        let endpoint: Endpoint =
            serde_json::from_str(store.get(&key("test.assistant")).unwrap().unwrap().expose())
                .unwrap();
        let response = reqwest::Client::new()
            .get(format!(
                "http://127.0.0.1:{}/v1/downloads/{id}",
                endpoint.port
            ))
            .bearer_auth(&endpoint.token.0)
            .header("origin", "https://example.test")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        manager.shutdown().await;
        let task = crate::assistant::status(store.as_ref(), "test.assistant", &id)
            .await
            .task
            .unwrap();
        assert!(matches!(
            task.phase,
            DownloadPhase::Failed | DownloadPhase::Cancelled
        ));
        let text = serde_json::to_string(&task).unwrap();
        for forbidden in [
            "127.0.0.1:99",
            "private=source",
            "history.json",
            "token",
            "cookie",
        ] {
            assert!(!text.contains(forbidden));
        }
        manager.set_assistant_allowed(false);
        assert_eq!(
            crate::assistant::start(store.as_ref(), "test.assistant", &changed)
                .await
                .error_code
                .as_deref(),
            Some("DOWNLOAD_ASSISTANT_DISABLED")
        );
        assert_eq!(
            crate::assistant::cancel(store.as_ref(), "test.assistant", &id)
                .await
                .error_code
                .as_deref(),
            Some("DOWNLOAD_ASSISTANT_DISABLED")
        );
        assert_eq!(
            crate::assistant::status(store.as_ref(), "test.assistant", &id)
                .await
                .error_code
                .as_deref(),
            Some("DOWNLOAD_ASSISTANT_DISABLED")
        );
        bridge.shutdown().await;
        assert_eq!(
            crate::assistant::status(store.as_ref(), "test.assistant", &id)
                .await
                .error_code
                .as_deref(),
            Some(NOT_CONNECTED)
        );
        let reopened = DownloadManager::open(
            history,
            folder.path().to_owned(),
            MediaTools::test_available(),
            |_| {},
        )
        .unwrap();
        assert_eq!(
            reopened.view().tasks[0].initiated_by,
            Some(DownloadInitiator::Assistant)
        );
        reopened.shutdown().await;
    }

    #[tokio::test]
    async fn local_bridge_requires_auth_rejects_web_origins_and_deduplicates() {
        let folder = tempfile::tempdir().unwrap();
        let store = Arc::new(MemorySecretStore::new());
        let manager = DownloadManager::open(
            folder.path().join("history.json"),
            folder.path().to_owned(),
            MediaTools::test_available(),
            |_| {},
        )
        .unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let selected = Arc::clone(&count);
        let bridge = BrowserServer::start(
            Arc::clone(&manager),
            store.clone(),
            "test.bridge",
            move || {
                selected.fetch_add(1, Ordering::Relaxed);
            },
        )
        .await
        .unwrap();
        let stored = store.get(&key("test.bridge")).unwrap().unwrap();
        let endpoint: Endpoint = serde_json::from_str(stored.expose()).unwrap();
        let url = format!("http://127.0.0.1:{}/v1/status", endpoint.port);
        let client = reqwest::Client::new();
        let response = client.get(&url).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let response = client
            .get(&url)
            .bearer_auth(&endpoint.token.0)
            .header("origin", "https://example.test")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(send(store.as_ref(), "test.bridge", None).await.ok);
        let mut request = BrowserDownload {
            request_id: "once".into(),
            via: None,
            input: DownloadInput {
                kind: DownloadKind::Video,
                url: "http://127.0.0.1:99/video.mp4".into(),
                page_url: None,
                title: Some("公开视频".into()),
            },
        };
        let first = send(store.as_ref(), "test.bridge", Some(&request)).await;
        assert!(first.ok);
        let second = send(store.as_ref(), "test.bridge", Some(&request)).await;
        assert_eq!(first, second);
        assert_eq!(count.load(Ordering::Relaxed), 1);
        assert_eq!(manager.view().tasks.len(), 1);
        assert_eq!(manager.view().browser_task_id, first.task_id);
        request.input.title = Some("changed".into());
        assert_eq!(
            send(store.as_ref(), "test.bridge", Some(&request))
                .await
                .error_code
                .as_deref(),
            Some(INVALID_REQUEST)
        );
        let text = serde_json::to_string(&first).unwrap();
        for forbidden in ["token", "cookie", "http:", "公开视频"] {
            assert!(!text.contains(forbidden));
        }
        manager.shutdown().await;
        bridge.shutdown().await;
        assert!(store.get(&key("test.bridge")).unwrap().is_none());
    }

    #[tokio::test]
    async fn a_reused_port_cannot_receive_download_sources_or_credentials() {
        let store = MemorySecretStore::new();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = Endpoint {
            port: listener.local_addr().unwrap().port(),
            token: SecretStringValue("a".repeat(64)),
        };
        store
            .put(
                &key("test.fake"),
                &SecretString::new(serde_json::to_string(&endpoint).unwrap()),
            )
            .unwrap();
        let router = Router::new().route(
            "/v1/challenge",
            get(|headers: HeaderMap| async move {
                assert!(!headers.contains_key("authorization"));
                Json(serde_json::json!({"version":1,"proof":"wrong"}))
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        assert_eq!(
            send(&store, "test.fake", None).await.error_code.as_deref(),
            Some(NOT_CONNECTED)
        );
        server.abort();
        let _ = server.await;
    }
}
