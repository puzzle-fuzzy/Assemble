//! HTTP 服务的组装与启停。

use std::net::SocketAddr;
use std::sync::Arc;

use assemble_protocol::health::{HEALTH_PATH, HealthResponse, READY_PATH, ReadinessResponse};
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use sqlx::SqlitePool;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::config::Config;

/// 所有请求处理器共享的状态。
#[derive(Debug, Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub config: Arc<Config>,
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

/// 组装全部路由。顺序无关：每条路由的路径都是唯一的。
pub fn router(state: Shared) -> Router {
    Router::new()
        .route(HEALTH_PATH, get(health))
        .route(READY_PATH, get(ready))
        .layer(axum::extract::DefaultBodyLimit::max(upload_limit()))
        .with_state(state)
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

/// 在配置的地址上提供服务，直到收到退出信号或 `stop` 被触发。
pub async fn serve(
    config: Config,
    pool: SqlitePool,
    stop: CancellationToken,
) -> Result<(), ServeError> {
    let listener = TcpListener::bind(config.addr).await?;
    tracing::info!(address = %listener.local_addr()?.to_string(), "listening");
    let state: Shared = Arc::new(AppState {
        pool,
        config: Arc::new(config),
    });
    let shutdown = async move {
        stop.cancelled().await;
        tracing::info!("shutting down");
    };
    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown)
        .await
        .map_err(ServeError::Server)
}

/// 绑定到一个空闲的回环端口，在后台开始服务。测试与开发工具用它拿到真实的服务。
pub struct TestServer {
    pub address: SocketAddr,
    pub origin: assemble_protocol::origin::ServerOrigin,
    stop: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl TestServer {
    pub async fn start(config: Config, pool: SqlitePool) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let origin = assemble_protocol::origin::ServerOrigin::parse(&format!("http://{address}"))
            .expect("a loopback address is a valid origin");
        let stop = CancellationToken::new();
        let triggered = stop.clone();
        let state: Shared = Arc::new(AppState {
            pool,
            config: Arc::new(config),
        });
        let task = tokio::spawn(async move {
            let shutdown = async move {
                triggered.cancelled().await;
            };
            if let Err(error) = axum::serve(listener, router(state))
                .with_graceful_shutdown(shutdown)
                .await
            {
                tracing::debug!(%error, "test server stopped");
            }
        });
        Ok(Self {
            address,
            origin,
            stop,
            task,
        })
    }

    /// 停止服务，并等它真正退出。
    ///
    /// 优雅退出会等还在用的连接自己收尾；测试里的客户端可能留着空闲的长连接，
    /// 所以最多等几秒，之后直接收掉。
    pub async fn stop(&mut self) {
        self.stop.cancel();
        if tokio::time::timeout(std::time::Duration::from_secs(5), &mut self.task)
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
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use super::*;

    fn config(dir: &std::path::Path) -> Config {
        Config {
            addr: "127.0.0.1:0".parse().unwrap(),
            data_dir: dir.to_path_buf(),
            public_origin: assemble_protocol::origin::ServerOrigin::development(),
            drive_quota_bytes: 1024,
            send_ttl: crate::config::DEFAULT_SEND_TTL,
            send_history: crate::config::DEFAULT_SEND_HISTORY,
            web_session_ttl: crate::config::DEFAULT_WEB_SESSION_TTL,
        }
    }

    #[tokio::test]
    async fn health_and_readiness_answer_without_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open(dir.path()).await.unwrap();
        let state: Shared = Arc::new(AppState {
            pool,
            config: Arc::new(config(dir.path())),
        });
        let app = router(state);

        let health = get(&app, HEALTH_PATH).await;
        assert_eq!(health.status(), StatusCode::OK);
        let body = axum::body::to_bytes(health.into_body(), 1024)
            .await
            .unwrap();
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

    /// 直接对一个路由器发一个 GET，不经过网络。
    async fn get(app: &Router, path: &str) -> axum::response::Response {
        app.clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_closed_database_makes_readiness_fail() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open(dir.path()).await.unwrap();
        pool.close().await;
        let state: Shared = Arc::new(AppState {
            pool,
            config: Arc::new(config(dir.path())),
        });
        let ready = router(state)
            .oneshot(
                Request::builder()
                    .uri(READY_PATH)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ready.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn a_test_server_serves_and_stops_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open(dir.path()).await.unwrap();
        let mut server = TestServer::start(config(dir.path()), pool).await.unwrap();
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
