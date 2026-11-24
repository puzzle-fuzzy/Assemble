//! HTTP 服务的组装与启停。

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use assemble_protocol::events::EVENT_STREAM_PATH;
use assemble_protocol::health::{HEALTH_PATH, HealthResponse, READY_PATH, ReadinessResponse};
use assemble_protocol::login::EXCHANGE_PATH;
use axum::Router;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use serde::de::DeserializeOwned;
use sqlx::SqlitePool;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::blobs::BlobStore;
use crate::config::Config;
use crate::events::{Hub, stream};
use crate::presence::Presence;
use crate::web::LoginThrottle;

/// 所有请求处理器共享的状态。
#[derive(Debug)]
pub struct AppState {
    pub pool: SqlitePool,
    pub config: Arc<Config>,
    /// 谁在线。只在内存里：服务端重启后所有设备都算离线。
    pub presence: Arc<Presence>,
    pub hub: Arc<Hub>,
    pub throttle: LoginThrottle,
    /// 文件内容的存放处，按摘要寻址。
    pub blobs: Arc<BlobStore>,
}

impl AppState {
    pub fn new(pool: SqlitePool, config: Config) -> Self {
        Self {
            blobs: Arc::new(BlobStore::new(&config.data_dir, pool.clone())),
            pool,
            config: Arc::new(config),
            presence: Arc::new(Presence::default()),
            hub: Arc::new(Hub::default()),
            throttle: LoginThrottle::new(),
        }
    }
}

pub type Shared = Arc<AppState>;

/// 请求正文的大小上限：单文件上限。它是 u64 而这里是 usize，32 位平台上取饱和值。
fn upload_limit() -> usize {
    usize::try_from(assemble_protocol::primitives::MAX_FILE_SIZE).unwrap_or(usize::MAX)
}

#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    #[error("the address could not be listened on: {0}")]
    Bind(#[from] std::io::Error),
    #[error("the HTTP server stopped unexpectedly: {0}")]
    Server(std::io::Error),
}

/// 组装全部路由。
pub fn router(state: Shared) -> Router {
    Router::new()
        .route(HEALTH_PATH, get(health))
        .route(READY_PATH, get(ready))
        // 系统浏览器里的页面
        .merge(crate::web::routes())
        .merge(crate::desktop_login::routes())
        // 桌面端的接口
        .route(EXCHANGE_PATH, post(crate::desktop_login::exchange))
        .route("/v1/me", get(crate::devices::me))
        .route("/v1/auth/logout", post(crate::devices::logout))
        .route("/v1/devices", get(crate::devices::list_devices))
        .route(
            "/v1/devices/{id}",
            patch(crate::devices::rename_device).delete(crate::devices::remove_device),
        )
        .route(EVENT_STREAM_PATH, get(stream))
        // 发送到设备：正文是文件的字节
        .route(
            "/v1/sends",
            post(crate::sends::create).get(crate::sends::list),
        )
        .route("/v1/sends/{id}/content", get(crate::sends::content))
        .route("/v1/sends/{id}/accept", post(crate::sends::accept))
        .route("/v1/sends/{id}/decline", post(crate::sends::decline))
        .route("/v1/sends/{id}/cancel", post(crate::sends::cancel))
        .route("/v1/sends/{id}/failed", post(crate::sends::failed))
        .layer(DefaultBodyLimit::max(upload_limit()))
        .with_state(state)
}

/// 解析一个 JSON 正文。不合法的正文按"请求不合法"回答，不回显内容。
pub fn read_json<T: DeserializeOwned>(body: &[u8]) -> Result<T, crate::error::ApiError> {
    serde_json::from_slice(body).map_err(|error| {
        crate::error::ApiError::invalid(format!(
            "the request body is not the JSON this endpoint expects ({:?})",
            error.classify()
        ))
    })
}

/// 进程能应答。不碰数据库：它回答的是"这个进程还活着吗"。
async fn health() -> axum::Json<HealthResponse> {
    axum::Json(HealthResponse {
        status: "ok".to_owned(),
    })
}

/// 数据库可用。不可用时回答 503，让反向代理把它摘出去。
async fn ready(State(state): State<Shared>) -> Response {
    match sqlx::query_scalar::<_, i64>("SELECT 1")
        .fetch_one(&state.pool)
        .await
    {
        Ok(_) => (
            StatusCode::OK,
            axum::Json(ReadinessResponse {
                status: "ok".to_owned(),
            }),
        )
            .into_response(),
        Err(error) => {
            tracing::warn!(%error, "readiness check failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                axum::Json(ReadinessResponse {
                    status: "unavailable".to_owned(),
                }),
            )
                .into_response()
        }
    }
}

/// 在配置的地址上提供服务，直到 `stop` 被触发。
pub async fn serve(
    config: Config,
    state: Shared,
    stop: CancellationToken,
) -> Result<(), ServeError> {
    let listener = TcpListener::bind(config.addr).await?;
    tracing::info!(address = %listener.local_addr()?.to_string(), "listening");
    let closing = Arc::clone(&state);
    let shutdown = async move {
        stop.cancelled().await;
        tracing::info!("shutting down");
        // 事件流是永不主动结束的长连接。不先收掉它们，优雅退出会一直等下去。
        closing.hub.close();
    };
    // 优雅退出最多等这么久：某个连接再怎么拖，也不能让进程卡在退出这一步。
    let drained = tokio::time::timeout(
        SHUTDOWN_GRACE,
        axum::serve(listener, router(state)).with_graceful_shutdown(shutdown),
    )
    .await;
    if let Ok(result) = drained {
        result.map_err(ServeError::Server)
    } else {
        tracing::warn!("some connections did not close in time; exiting anyway");
        Ok(())
    }
}

/// 优雅退出等待在途连接收尾的上限。
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

/// 绑定到一个空闲的回环端口，在后台开始服务。测试与开发工具用它拿到真实的服务。
pub struct TestServer {
    pub address: SocketAddr,
    pub origin: assemble_protocol::origin::ServerOrigin,
    /// 供测试直接查库、改在线状态。
    pub state: Shared,
    stop: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl TestServer {
    /// 用给定的配置与数据库启动。
    pub async fn start(config: Config, pool: SqlitePool) -> std::io::Result<Self> {
        let listener = TcpListener::bind(config.addr).await?;
        Self::serve(listener, config, pool)
    }

    /// 用开发配置与 `data_dir` 里的数据库启动；对外地址跟着实际端口走。
    pub async fn start_default(data_dir: &std::path::Path) -> std::io::Result<Self> {
        let pool = crate::db::open(data_dir)
            .await
            .map_err(std::io::Error::other)?;
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let origin = assemble_protocol::origin::ServerOrigin::parse(&format!("http://{address}"))
            .expect("a loopback address is a valid origin");
        let config = Config::development(data_dir).with_public_origin(origin);
        Self::serve(listener, config, pool)
    }

    fn serve(listener: TcpListener, mut config: Config, pool: SqlitePool) -> std::io::Result<Self> {
        let address = listener.local_addr()?;
        let origin = assemble_protocol::origin::ServerOrigin::parse(&format!("http://{address}"))
            .expect("a loopback address is a valid origin");
        // 测试服务的对外地址就是它实际监听的那个：网页登录要拿它核对 Origin。
        config.public_origin = origin.clone();
        let stop = CancellationToken::new();
        let triggered = stop.clone();
        let state = Arc::new(AppState::new(pool, config));
        let served = Arc::clone(&state);
        let task = tokio::spawn(async move {
            let shutdown = async move {
                triggered.cancelled().await;
            };
            if let Err(error) = axum::serve(listener, router(served))
                .with_graceful_shutdown(shutdown)
                .await
            {
                tracing::debug!(%error, "test server stopped");
            }
        });
        Ok(Self {
            address,
            origin,
            state,
            stop,
            task,
        })
    }

    /// 停止服务，并等它真正退出。
    ///
    /// 优雅退出会等还在用的连接自己收尾；测试里的客户端可能留着空闲的长连接
    /// 或事件流，所以最多等几秒，之后直接收掉。
    pub async fn stop(&mut self) {
        self.state.hub.close();
        self.stop.cancel();
        if tokio::time::timeout(Duration::from_secs(5), &mut self.task)
            .await
            .is_err()
        {
            tracing::debug!("the test server did not stop in time; aborting it");
            self.task.abort();
        }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        // 忘了调用 `stop` 也不能把后台任务留到测试结束之后。
        self.stop.cancel();
    }
}

#[cfg(test)]
mod tests {
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use super::*;

    /// 一份只用于测试的配置：回环地址、极小的云盘上限。
    pub fn test_config(data_dir: &std::path::Path) -> Config {
        Config::development(data_dir)
    }

    async fn state(data_dir: &std::path::Path) -> Shared {
        Arc::new(AppState::new(
            crate::db::open(data_dir).await.unwrap(),
            test_config(data_dir),
        ))
    }

    /// 直接对一个路由器发一个 GET，不经过网络。
    pub async fn get(app: &Router, path: &str) -> axum::response::Response {
        app.clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn health_and_readiness_answer_without_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state(dir.path()).await);

        let health = get(&app, HEALTH_PATH).await;
        assert_eq!(health.status(), StatusCode::OK);
        let body = to_bytes(health.into_body(), 1024).await.unwrap();
        assert_eq!(body, r#"{"status":"ok"}"#);

        let ready = get(&app, READY_PATH).await;
        assert_eq!(ready.status(), StatusCode::OK);
        assert_eq!(
            ready.headers().get("content-type").unwrap(),
            "application/json"
        );

        // 没登记的路径如实回答 404。
        let missing = get(&app, "/nope").await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_closed_database_makes_readiness_fail() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open(dir.path()).await.unwrap();
        pool.close().await;
        let app = router(Arc::new(AppState::new(pool, test_config(dir.path()))));
        let ready = get(&app, READY_PATH).await;
        assert_eq!(ready.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn device_endpoints_refuse_requests_without_a_token() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state(dir.path()).await);
        for (path, method) in [
            ("/v1/me", "GET"),
            ("/v1/devices", "GET"),
            ("/v1/devices/abc", "PATCH"),
            ("/v1/devices/abc", "DELETE"),
            ("/v1/auth/logout", "POST"),
            (EVENT_STREAM_PATH, "GET"),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(path)
                        .header("authorization", "Bearer not-a-real-token")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "{method} {path}"
            );
            let body = to_bytes(response.into_body(), 4096).await.unwrap();
            assert!(
                body.starts_with(br#"{"error":{"code":"UNAUTHORIZED""#),
                "{method} {path}: {}",
                String::from_utf8_lossy(&body)
            );
        }
        let anonymous = get(&app, "/v1/me").await;
        assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn broken_json_is_a_stable_bad_request() {
        let error = read_json::<assemble_protocol::login::ExchangeInput>(b"{").unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert_eq!(error.code, assemble_protocol::error::codes::INVALID_REQUEST);
        assert!(!error.message.contains('{'));
        let error =
            read_json::<assemble_protocol::login::ExchangeInput>(br#"{"code":5}"#).unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn a_test_server_serves_and_stops_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open(dir.path()).await.unwrap();
        let mut server = TestServer::start(test_config(dir.path()), pool)
            .await
            .unwrap();
        assert!(server.address.ip().is_loopback());

        let cancel = CancellationToken::new();
        let client = assemble_client::Client::new(server.origin.clone()).unwrap();
        client
            .health(&assemble_client::RequestOptions::new(&cancel))
            .await
            .unwrap();
        drop(client);
        server.stop().await;
    }
}
