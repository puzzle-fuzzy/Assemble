//! HTTP 服务的组装与启停。

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use assemble_protocol::events::EVENT_STREAM_PATH;
use assemble_protocol::health::{HEALTH_PATH, HealthResponse, READY_PATH, ReadinessResponse};
use assemble_protocol::login::EXCHANGE_PATH;
use axum::Router;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::header;
use axum::http::header::HeaderValue;
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post, put};
use serde::de::DeserializeOwned;
use sqlx::SqlitePool;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;
use tower_http::services::ServeFile;

use crate::blobs::BlobStore;
use crate::config::Config;
use crate::error::ApiError;
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
    pub mailer: Arc<dyn crate::mail::Mailer>,
    pub github_flows: crate::github::Flows,
    pub github_provider: Arc<dyn crate::github::Provider>,
    pub media: Arc<crate::media::jobs::Runtime>,
}

impl AppState {
    pub fn new(pool: SqlitePool, config: Config) -> Self {
        Self {
            media: Arc::new(crate::media::jobs::Runtime::new(config.media.clone())),
            mailer: Arc::new(crate::mail::SmtpMailer(config.smtp.clone())),
            github_flows: crate::github::Flows::default(),
            github_provider: Arc::new(crate::github::GithubProvider(config.github.clone())),
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
    #[error("media job recovery failed")]
    MediaRecovery,
    #[error("the address could not be listened on: {0}")]
    Bind(#[from] std::io::Error),
    #[error("the HTTP server stopped unexpectedly: {0}")]
    Server(std::io::Error),
}

/// 组装全部路由。
pub fn router(state: Shared) -> Router {
    let browser_routes = Router::new()
        .merge(crate::web::routes())
        .merge(crate::register::routes())
        .merge(crate::account::routes())
        .merge(crate::identity_web::routes())
        .merge(crate::github::routes())
        .merge(crate::desktop_login::routes())
        .layer(DefaultBodyLimit::max(16 * 1024));
    Router::new()
        .route(HEALTH_PATH, get(health))
        .route(READY_PATH, get(ready))
        .merge(browser_routes)
        .merge(crate::admin::routes())
        .merge(crate::media::routes())
        .merge(crate::admin_assets::routes(&state.config.admin_assets_dir))
        .merge(crate::media_web_assets::routes(
            &state.config.media_web_assets_dir,
        ))
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
        // 云盘：存文件时正文是文件的字节
        .route(
            "/v1/drive/files",
            post(crate::drive::upload).get(crate::drive::list),
        )
        .route("/v1/drive/files/{id}/content", get(crate::drive::content))
        .route(
            "/v1/drive/files/{id}",
            patch(crate::drive::rename).delete(crate::drive::remove),
        )
        // 日程规划：日程、提醒与通知。改动带着版本号，新建可以带幂等键。
        .route("/v1/planner", get(crate::planner::planner))
        .route("/v1/schedules", post(crate::planner::schedules::create))
        .route(
            "/v1/schedules/{id}",
            put(crate::planner::schedules::update).delete(crate::planner::schedules::remove),
        )
        .route(
            "/v1/schedules/{id}/complete",
            post(crate::planner::schedules::complete),
        )
        .route(
            "/v1/schedules/{id}/cancel",
            post(crate::planner::schedules::cancel),
        )
        .route(
            "/v1/schedules/{id}/reopen",
            post(crate::planner::schedules::reopen),
        )
        .route("/v1/reminders", post(crate::planner::reminders::create))
        .route(
            "/v1/reminders/{id}",
            put(crate::planner::reminders::update).delete(crate::planner::reminders::remove),
        )
        .route(
            "/v1/reminders/{id}/dismiss",
            post(crate::planner::reminders::dismiss),
        )
        .route(
            "/v1/reminders/{id}/cancel",
            post(crate::planner::reminders::cancel),
        )
        .route(
            "/v1/notifications/read",
            post(crate::planner::notifications::read),
        )
        .route(
            "/v1/notifications/read-all",
            post(crate::planner::notifications::read_all),
        )
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
    serve_until(listener, state, stop, SHUTDOWN_GRACE).await
}

/// 在已经绑定的监听地址上提供服务，直到 `stop` 被触发；之后最多再等 `grace` 这么久。
///
/// 正式服务与测试服务走的是同一段代码：退出逻辑只写一遍，测试测到的就是真实运行的那条路。
///
/// 计时从收到退出信号开始，而不是从启动开始。曾经把上限包在整个服务外面，
/// 结果服务端启动十秒后自己退出——而测试用的是另一条启动路径，完全没碰到。
pub async fn serve_until(
    listener: TcpListener,
    state: Shared,
    stop: CancellationToken,
    grace: Duration,
) -> Result<(), ServeError> {
    state
        .media
        .recover(&state)
        .await
        .map_err(|_| ServeError::MediaRecovery)?;
    let closing = Arc::clone(&state);
    let waiting = stop.clone();
    let shutdown = async move {
        waiting.cancelled().await;
        tracing::info!("shutting down");
        // 事件流是永不主动结束的长连接。不先收掉它们，优雅退出会一直等下去。
        closing.hub.close();
        closing.media.shutdown(&closing.pool).await;
    };
    let patience = async move {
        stop.cancelled().await;
        tokio::time::sleep(grace).await;
    };
    tokio::select! {
        result = axum::serve(listener, router(state)).with_graceful_shutdown(shutdown) => {
            result.map_err(ServeError::Server)
        }
        // 某个连接再怎么拖，也不能让进程卡在退出这一步。
        () = patience => {
            tracing::warn!("some connections did not close in time; exiting anyway");
            Ok(())
        }
    }
}

/// 把磁盘上的一个文件当作下载交出去：可以只取一段，文件名与类型按记录里说的。
///
/// 发送与云盘的内容都从这里出去，所以两边的下载行为完全一样：
/// 分段、条件请求、不嗅探类型、以附件的形式保存。
pub(crate) async fn file_response(
    path: &std::path::Path,
    name: &str,
    mime: Option<&str>,
    headers: &axum::http::HeaderMap,
) -> Response {
    let mut forwarded = axum::http::Request::builder().method(Method::GET).uri("/");
    for header_name in FILE_REQUEST_HEADERS {
        if let Some(value) = headers.get(&header_name) {
            forwarded = forwarded.header(header_name, value);
        }
    }
    let served = ServeFile::new(path)
        .oneshot(
            forwarded
                .body(Body::empty())
                .expect("a fixed request always builds"),
        )
        .await;
    let mut response = match served {
        Ok(response) => response,
        Err(error) => {
            tracing::error!(%error, "stored content could not be served");
            return ApiError::internal("the stored content could not be read").into_response();
        }
    };
    // 只有正文真的跟着回去时（全部或一段），才改它声明的类型。
    let with_body = matches!(
        response.status(),
        StatusCode::OK | StatusCode::PARTIAL_CONTENT
    );
    let out = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(&disposition(name)) {
        out.insert(header::CONTENT_DISPOSITION, value);
    }
    out.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    if with_body
        && let Some(mime) = mime
        && let Ok(value) = HeaderValue::from_str(mime)
    {
        out.insert(header::CONTENT_TYPE, value);
    }
    response.into_response()
}

/// 在读完正文之前就要拒绝一个上传：等对方把正文送完、边收边丢，再把拒绝回答出去。
///
/// 客户端是流式上传的。不等它说完就回绝，连接会在它还在发送时被关掉，它看不到那个
/// 回答，只会报"连不上服务"——而真实的原因是"对方不在线"。让客户端白传一遍并不好看，
/// 但这种情况只出现在对方刚好在最后一刻下线时，比起说错原因要强。
///
/// 正文不进内存；超过单文件上限还不肯停的，或者太久不送数据的，就不等了。
pub(crate) async fn refuse_upload(
    body: axum::body::Body,
    error: crate::error::ApiError,
) -> crate::error::ApiError {
    use futures_util::StreamExt;

    let mut stream = body.into_data_stream();
    let mut discarded = 0u64;
    while discarded <= assemble_protocol::primitives::MAX_FILE_SIZE {
        let chunk = tokio::time::timeout(crate::blobs::UPLOAD_STALL, stream.next()).await;
        match chunk {
            Ok(Some(Ok(bytes))) => discarded += bytes.len() as u64,
            // 送完了、出错了，或者太久没送：都到此为止。
            _ => break,
        }
    }
    error
}

/// 优雅退出等待在途连接收尾的上限。
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

/// 测试服务退出时等待的上限：测试要快，拖着的连接由 [`TestServer::stop`] 直接收掉。
const TEST_GRACE: Duration = Duration::from_secs(2);

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

    /// 用替身发送器验证完整网页流程，不发送真实邮件、不访问外部 OAuth。
    pub async fn start_with_services(
        config: Config,
        pool: SqlitePool,
        mailer: Arc<dyn crate::mail::Mailer>,
        github: Arc<dyn crate::github::Provider>,
    ) -> std::io::Result<Self> {
        let listener = TcpListener::bind(config.addr).await?;
        Self::serve_custom(listener, config, pool, Some((mailer, github)))
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

    fn serve(listener: TcpListener, config: Config, pool: SqlitePool) -> std::io::Result<Self> {
        Self::serve_custom(listener, config, pool, None)
    }

    fn serve_custom(
        listener: TcpListener,
        mut config: Config,
        pool: SqlitePool,
        services: Option<(
            Arc<dyn crate::mail::Mailer>,
            Arc<dyn crate::github::Provider>,
        )>,
    ) -> std::io::Result<Self> {
        let address = listener.local_addr()?;
        let origin = assemble_protocol::origin::ServerOrigin::parse(&format!("http://{address}"))
            .expect("a loopback address is a valid origin");
        // 测试服务的对外地址就是它实际监听的那个：网页登录要拿它核对 Origin。
        config.public_origin = origin.clone();
        let stop = CancellationToken::new();
        let triggered = stop.clone();
        let mut state = AppState::new(pool, config);
        if let Some((mailer, github)) = services {
            state.mailer = mailer;
            state.github_provider = github;
        }
        let state = Arc::new(state);
        let served = Arc::clone(&state);
        let task = tokio::spawn(async move {
            if let Err(error) = serve_until(listener, served, triggered, TEST_GRACE).await {
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

/// 取内容时只把与"取一段文件"有关的请求头转交给文件服务。
const FILE_REQUEST_HEADERS: [header::HeaderName; 4] = [
    header::RANGE,
    header::IF_RANGE,
    header::IF_NONE_MATCH,
    header::IF_MODIFIED_SINCE,
];

/// `Content-Disposition` 里的文件名：ASCII 里安全的字符原样放，其余按字节百分号编码。
pub(crate) fn disposition(name: &str) -> String {
    const SAFE: &[u8] = b"!#$&+-.^_`|~";
    let mut encoded = String::with_capacity(name.len());
    for byte in name.bytes() {
        if byte.is_ascii_alphanumeric() || SAFE.contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    format!("attachment; filename*=UTF-8''{encoded}")
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

    /// 上限只管退出，不给服务定寿命；拖着连接不放，到上限也会退出。
    #[tokio::test]
    async fn only_the_shutdown_has_a_time_limit() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpStream;

        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open(dir.path()).await.unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let origin = assemble_protocol::origin::ServerOrigin::parse(&format!("http://{address}"))
            .expect("a loopback address is a valid origin");
        let state = Arc::new(AppState::new(
            pool,
            Config::development(dir.path()).with_public_origin(origin),
        ));
        let stop = CancellationToken::new();
        let grace = Duration::from_millis(300);
        let triggered = stop.clone();
        let served =
            tokio::spawn(async move { serve_until(listener, state, triggered, grace).await });
        let health = format!("http://{address}/healthz");

        // 几个"上限"过去了，服务还在、还应答。
        tokio::time::sleep(grace * 3).await;
        let response = reqwest::get(&health).await.unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);

        // 留着一条空闲的长连接再让它退出：到上限也会退出，不会一直等下去。
        let mut idle = TcpStream::connect(address).await.unwrap();
        idle.write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let mut scratch = [0u8; 64];
        assert!(idle.read(&mut scratch).await.unwrap() > 0);
        stop.cancel();
        let finished = tokio::time::timeout(grace + Duration::from_secs(5), served).await;
        assert!(matches!(finished, Ok(Ok(Ok(())))), "{finished:?}");
    }
}
