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
        store.put(&key, &stored).map_err(|_| KEYCHAIN_UNAVAILABLE)?;
        let state = Arc::new(BridgeState {
            token: SecretString::new(token),
            manager,
            accepted: tokio::sync::Mutex::new(HashMap::new()),
            selected: Arc::new(selected),
        });
        let protected = Router::new()
            .route("/v1/download", post(download))
            .route("/v1/status", get(status))
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
    if !is_valid_id(&request.request_id) || request.request_id.len() > 64 {
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
        .timeout(Duration::from_secs(if request.is_some() { 40 } else { 3 }))
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
    let builder = if let Some(request) = request {
        client.post(format!("{base}/v1/download")).json(request)
    } else {
        client.get(format!("{base}/v1/status"))
    };
    let response = builder
        .bearer_auth(&endpoint.token.0)
        .send()
        .await
        .map_err(|_| NOT_CONNECTED)?;
    let reply: BrowserReply = small_json(response).await?;
    if reply.version != 1 {
        return Err(NOT_CONNECTED);
    }
    Ok(reply)
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
