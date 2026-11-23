//! [`JarvisClient`]：资源、设备命令、文件与账户接口。
//!
//! 所有方法都经过同一个发送管线，由它统一处理超时、取消、凭据头、
//! 来源固定与错误归类。接口的路径、请求体与响应结构和 Electron 版的
//! TypeScript 客户端一一对应。

use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use bytes::Bytes;
use futures_core::Stream;
use futures_util::TryStreamExt;
use reqwest::cookie::{CookieStore, Jar};
use reqwest::header::{ACCEPT, AUTHORIZATION, ORIGIN};
use reqwest::multipart::{Form, Part};
use reqwest::{Body, Client, Method, redirect};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tokio_util::io::StreamReader;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

use assemble_protocol::auth::{AuthResponse, MeResponse, SESSION_COOKIE, SuccessResponse};
use assemble_protocol::catalog::{CapabilitiesResponse, JarvisCapability};
use assemble_protocol::commands::{
    CommandResponse, CommandsResponse, DeviceCommand, DeviceReport, DirectoryRequestInput,
    HeartbeatInput, WaitSeconds,
};
use assemble_protocol::desktop_login::DesktopExchangeInput;
use assemble_protocol::error::ApiErrorBody;
use assemble_protocol::files::{FileResolution, FileResolveInput, InventoryInput, UploadMetadata};
use assemble_protocol::health::{HealthResponse, ReadinessResponse};
use assemble_protocol::primitives::{MAX_FILE_SIZE, is_valid_id};
use assemble_protocol::resources::OkResponse;
use assemble_protocol::resources::{
    JarvisItem, JarvisList, JarvisRecord, ListQuery, LogData, ResourceName, UpdateInput,
    VersionInput,
};
use assemble_store::secret::SecretString;

use crate::error::ClientError;
use crate::origin::Deployment;

/// 普通请求的时限。
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

/// 命令长轮询的时限：服务端最多压住 25 秒，留出余量。
pub const COMMAND_WAIT_TIMEOUT: Duration = Duration::from_secs(30);

/// 列表接口一页的最大条数。
pub const PAGE_SIZE: u32 = 100;

/// 设备日志一次最多上报的条数。
pub const MAX_LOG_BATCH: usize = 100;

/// 一次请求的退出条件。
#[derive(Debug, Clone, Copy)]
pub struct RequestOptions<'a> {
    pub cancel: &'a CancellationToken,
    /// 覆盖默认时限。
    pub timeout: Option<Duration>,
}

impl<'a> RequestOptions<'a> {
    pub fn new(cancel: &'a CancellationToken) -> Self {
        Self {
            cancel,
            timeout: None,
        }
    }

    /// 这一次请求实际使用的时限：显式指定 > 长轮询时限 > 默认时限。
    fn budget(&self, long_poll: bool) -> Duration {
        self.timeout.unwrap_or(if long_poll {
            COMMAND_WAIT_TIMEOUT
        } else {
            DEFAULT_TIMEOUT
        })
    }
}

/// 一次调用的全部要素，交给发送管线之前先在这里组装好。
struct Call<'a> {
    channel: Channel,
    method: Method,
    path: &'a str,
    query: Vec<(&'static str, String)>,
    payload: Payload,
    accept: Accept,
    long_poll: bool,
}

impl<'a> Call<'a> {
    fn get(path: &'a str) -> Self {
        Self {
            channel: Channel::Authenticated,
            method: Method::GET,
            path,
            query: Vec::new(),
            payload: Payload::None,
            accept: Accept::Json,
            long_poll: false,
        }
    }

    fn with_payload(mut self, method: Method, payload: Payload) -> Self {
        self.method = method;
        self.payload = payload;
        self
    }

    fn post(self, payload: Payload) -> Self {
        self.with_payload(Method::POST, payload)
    }

    fn anonymous(mut self) -> Self {
        self.channel = Channel::Anonymous;
        self
    }

    fn query(mut self, key: &'static str, value: impl Into<String>) -> Self {
        self.query.push((key, value.into()));
        self
    }

    fn long_poll(mut self) -> Self {
        self.long_poll = true;
        self
    }

    fn binary(mut self) -> Self {
        self.accept = Accept::OctetStream;
        self
    }
}

/// 用哪一路凭据发请求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Channel {
    /// 带上 Bearer 设备会话。
    Authenticated,
    /// 不带 Authorization。只有换取会话与公开接口需要它。
    Anonymous,
}

enum Payload {
    None,
    Json(Value),
    Multipart(Form),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Accept {
    Json,
    OctetStream,
}

impl Accept {
    fn value(self) -> &'static str {
        match self {
            Self::Json => "application/json",
            Self::OctetStream => "application/octet-stream",
        }
    }
}

/// 要上传的本机文件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalSource {
    pub path: PathBuf,
    /// 已经确认过的大小，必须小于单文件上限。
    pub size: u64,
    pub mime_type: Option<String>,
}

/// 一次下载的响应体：按到达的顺序流式读取，不把整个文件放进内存。
pub struct Download {
    /// 服务端声明的总长度；没有声明时为 `None`。
    pub content_length: Option<u64>,
    reader: StreamReader<Pin<Box<dyn Stream<Item = std::io::Result<Bytes>> + Send>>, Bytes>,
}

impl Download {
    /// 取出可读取的字节流。读取中断时得到一个 I/O 错误。
    pub fn into_reader(self) -> impl tokio::io::AsyncRead + Unpin + Send + 'static {
        self.reader
    }
}

impl std::fmt::Debug for Download {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Download")
            .field("content_length", &self.content_length)
            .field("reader", &"byte stream")
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ClientBuildError {
    #[error("the HTTP client could not be configured: {0}")]
    Transport(#[from] reqwest::Error),
}

/// 访问云端的一个客户端。克隆共享同一份会话。
#[derive(Clone)]
pub struct JarvisClient {
    deployment: Arc<Deployment>,
    http: Client,
    jar: Arc<Jar>,
    bearer: Arc<Mutex<Option<SecretString>>>,
}

impl JarvisClient {
    /// 为一个部署创建客户端。请求只发往这个部署，且永远不跟随重定向。
    pub fn new(deployment: Deployment) -> Result<Self, ClientBuildError> {
        let jar = Arc::new(Jar::default());
        let http = Client::builder()
            .cookie_provider(Arc::clone(&jar))
            .redirect(redirect::Policy::none())
            .build()?;
        Ok(Self {
            deployment: Arc::new(deployment),
            http,
            jar,
            bearer: Arc::new(Mutex::new(None)),
        })
    }

    pub fn deployment(&self) -> &Deployment {
        &self.deployment
    }

    /// 换用新的 Bearer 设备会话；传 `None` 表示已退出。
    pub fn set_bearer(&self, token: Option<SecretString>) {
        *self.bearer.lock().unwrap_or_else(PoisonError::into_inner) = token;
    }

    pub fn has_bearer(&self) -> bool {
        self.bearer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    fn bearer(&self) -> Option<Zeroizing<String>> {
        self.bearer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|token| Zeroizing::new(token.expose().to_owned()))
    }

    /// 当前存着的网页会话 Cookie。只用于核对与撤销，不写进任何日志或返回给界面。
    pub fn session_cookie(&self) -> Option<SecretString> {
        // Cookie 的路径限定在 `/api` 之下，所以要用一个 API 地址去取。
        let probe = self.deployment.api.join("/api/auth/me");
        let header = self.jar.cookies(&probe)?;
        let text = header.to_str().ok()?;
        text.split(';').find_map(|pair| {
            let (name, value) = pair.trim().split_once('=')?;
            (name == SESSION_COOKIE && !value.is_empty()).then(|| SecretString::new(value))
        })
    }

    /// 清掉网页会话 Cookie。退出与重新登录之前都要做。
    pub fn clear_session_cookie(&self) {
        let target = self.deployment.api.join("/api/auth/me");
        self.jar
            .add_cookie_str(&format!("{SESSION_COOKIE}=; Path=/api; Max-Age=0"), &target);
    }

    // ---- 探活 ----

    /// `GET /health`：进程能应答。公开接口。
    pub async fn health(
        &self,
        options: &RequestOptions<'_>,
    ) -> Result<HealthResponse, ClientError> {
        self.call_json(Call::get("/health").anonymous(), options)
            .await
    }

    /// `GET /ready`：依赖已就绪。不可用时服务端返回 503，这里如实给出"不可用"。
    pub async fn ready(
        &self,
        options: &RequestOptions<'_>,
    ) -> Result<ReadinessResponse, ClientError> {
        let call = Call::get("/ready").anonymous();
        let response = self.send(call, options).await?;
        decode(response).await
    }

    // ---- 账户与会话 ----

    /// 用授权码换取网页会话。服务端要求这个请求不带任何 Cookie 或 Authorization。
    ///
    /// 新会话经 `Set-Cookie` 落进客户端自己的 Cookie 罐，不会出现在返回值里。
    pub async fn desktop_exchange(
        &self,
        input: &DesktopExchangeInput,
        options: &RequestOptions<'_>,
    ) -> Result<(), ClientError> {
        if !input.is_valid() {
            return Err(ClientError::InvalidRequest);
        }
        // 旧会话在登录开始时已清除；这里再确认一次，带着旧 Cookie 的换码一律不发出。
        self.clear_session_cookie();
        if self.session_cookie().is_some() {
            return Err(ClientError::InvalidRequest);
        }
        let call = Call::get("/api/auth/desktop/exchange")
            .post(Payload::Json(json(input)?))
            .anonymous();
        let response = self.send(call, options).await?;
        checked(response).await?;
        Ok(())
    }

    /// `GET /api/auth/me`：当前会话对应的账户。
    pub async fn me(&self, options: &RequestOptions<'_>) -> Result<MeResponse, ClientError> {
        self.call_json(Call::get("/api/auth/me"), options).await
    }

    /// 用网页会话换一条独立的 Bearer 设备会话。后台的全部请求只用它。
    pub async fn device_session(
        &self,
        options: &RequestOptions<'_>,
    ) -> Result<AuthResponse, ClientError> {
        // 接口没有输入，但写请求必须带 JSON 内容类型。
        self.call_json(empty_post("/api/auth/device-session"), options)
            .await
    }

    /// `POST /api/auth/logout`：撤销当前凭据对应的会话。
    pub async fn logout(
        &self,
        options: &RequestOptions<'_>,
    ) -> Result<SuccessResponse, ClientError> {
        self.call_json(empty_post("/api/auth/logout"), options)
            .await
    }

    /// `POST /api/auth/sessions/:id/revoke`。
    pub async fn revoke_session(
        &self,
        session_id: &str,
        options: &RequestOptions<'_>,
    ) -> Result<SuccessResponse, ClientError> {
        let path = format!("/api/auth/sessions/{}/revoke", id_segment(session_id)?);
        self.call_json(empty_post(&path), options).await
    }

    // ---- 泛型资源 ----

    pub async fn list(
        &self,
        resource: ResourceName,
        query: &ListQuery,
        options: &RequestOptions<'_>,
    ) -> Result<JarvisList, ClientError> {
        if !query.is_valid() {
            return Err(ClientError::InvalidRequest);
        }
        let path = format!("/api/jarvis/resources/{}", resource.as_str());
        let mut call = Call::get(&path)
            .query("limit", query.limit.to_string())
            .query("offset", query.offset.to_string());
        if let Some(search) = &query.search {
            call = call.query("search", search.clone());
        }
        self.call_json(call, options).await
    }

    /// 一页一页地取出某种资源的全部记录。
    pub async fn list_all(
        &self,
        resource: ResourceName,
        options: &RequestOptions<'_>,
    ) -> Result<Vec<JarvisRecord>, ClientError> {
        let mut items = Vec::new();
        let mut offset = 0u32;
        loop {
            let page = self
                .list(
                    resource,
                    &ListQuery {
                        search: None,
                        limit: PAGE_SIZE,
                        offset,
                    },
                    options,
                )
                .await?;
            let got = page.items.len();
            items.extend(page.items);
            let fetched = u64::from(offset) + got as u64;
            if got < PAGE_SIZE as usize || fetched >= page.total {
                return Ok(items);
            }
            offset += PAGE_SIZE;
        }
    }

    pub async fn get(
        &self,
        resource: ResourceName,
        id: &str,
        options: &RequestOptions<'_>,
    ) -> Result<JarvisRecord, ClientError> {
        let item: JarvisItem = self
            .call_json(Call::get(&record_path(resource, id)?), options)
            .await?;
        Ok(item.item)
    }

    /// 创建一条记录。请求体就是记录的数据本身。
    pub async fn create<D: Serialize>(
        &self,
        resource: ResourceName,
        data: &D,
        options: &RequestOptions<'_>,
    ) -> Result<JarvisRecord, ClientError> {
        let path = format!("/api/jarvis/resources/{}", resource.as_str());
        let call = Call::get(&path).post(Payload::Json(json(data)?));
        let item: JarvisItem = self.call_json(call, options).await?;
        Ok(item.item)
    }

    pub async fn update(
        &self,
        resource: ResourceName,
        id: &str,
        input: &UpdateInput,
        options: &RequestOptions<'_>,
    ) -> Result<JarvisRecord, ClientError> {
        if !input.is_valid() {
            return Err(ClientError::InvalidRequest);
        }
        let path = record_path(resource, id)?;
        let call = Call::get(&path).with_payload(Method::PATCH, Payload::Json(json(input)?));
        let item: JarvisItem = self.call_json(call, options).await?;
        Ok(item.item)
    }

    pub async fn remove(
        &self,
        resource: ResourceName,
        id: &str,
        version: u64,
        options: &RequestOptions<'_>,
    ) -> Result<OkResponse, ClientError> {
        let path = record_path(resource, id)?;
        let call = Call::get(&path).with_payload(
            Method::DELETE,
            Payload::Json(json(&VersionInput { version })?),
        );
        self.call_json(call, options).await
    }

    pub async fn capabilities(
        &self,
        options: &RequestOptions<'_>,
    ) -> Result<Vec<JarvisCapability>, ClientError> {
        let response: CapabilitiesResponse = self
            .call_json(Call::get("/api/jarvis/capabilities"), options)
            .await?;
        Ok(response.resources)
    }

    // ---- 文件 ----

    /// `POST /api/jarvis/files/upload`：以 multipart 流式上传一个本机文件。
    pub async fn upload(
        &self,
        source: &LocalSource,
        metadata: &UploadMetadata,
        options: &RequestOptions<'_>,
    ) -> Result<JarvisRecord, ClientError> {
        if source.size >= MAX_FILE_SIZE {
            return Err(ClientError::InvalidRequest);
        }
        let file = tokio::fs::File::open(&source.path)
            .await
            .map_err(|_| ClientError::LocalSource)?;
        let file_name = metadata.name.clone().or_else(|| {
            source
                .path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        });
        let Some(file_name) = file_name else {
            return Err(ClientError::InvalidRequest);
        };
        let part = Part::stream_with_length(Body::from(file), source.size)
            .file_name(file_name)
            .mime_str(
                source
                    .mime_type
                    .as_deref()
                    .unwrap_or("application/octet-stream"),
            )
            .map_err(|_| ClientError::InvalidRequest)?;

        let mut form = Form::new().part("file", part);
        for (field, value) in [
            ("name", &metadata.name),
            ("deviceId", &metadata.device_id),
            ("rootId", &metadata.root_id),
            ("relativePath", &metadata.relative_path),
            ("sourceModifiedAt", &metadata.source_modified_at),
        ] {
            if let Some(value) = value {
                form = form.text(field, value.clone());
            }
        }
        let call = Call::get("/api/jarvis/files/upload").post(Payload::Multipart(form));
        let item: JarvisItem = self.call_json(call, options).await?;
        Ok(item.item)
    }

    /// `GET /api/jarvis/files/:id/content`：取得云副本的字节流。
    pub async fn download(
        &self,
        id: &str,
        options: &RequestOptions<'_>,
    ) -> Result<Download, ClientError> {
        let path = format!("/api/jarvis/files/{}/content", id_segment(id)?);
        let response = self.send(Call::get(&path).binary(), options).await?;
        let response = checked(response).await?;
        let content_length = response.content_length();
        // 传输错误在离开客户端时转成 I/O 错误：调用方只需要处理一种错误。
        let stream = response.bytes_stream().map_err(std::io::Error::other);
        let stream: Pin<Box<dyn Stream<Item = std::io::Result<Bytes>> + Send>> = Box::pin(stream);
        Ok(Download {
            content_length,
            reader: StreamReader::new(stream),
        })
    }

    pub async fn resolve(
        &self,
        input: &FileResolveInput,
        options: &RequestOptions<'_>,
    ) -> Result<FileResolution, ClientError> {
        if !input.has_identity() {
            return Err(ClientError::InvalidRequest);
        }
        let call = Call::get("/api/jarvis/files/resolve").post(Payload::Json(json(input)?));
        self.call_json(call, options).await
    }

    // ---- 设备 ----

    pub async fn heartbeat(
        &self,
        device_id: &str,
        input: &HeartbeatInput,
        options: &RequestOptions<'_>,
    ) -> Result<JarvisRecord, ClientError> {
        let path = format!("/api/jarvis/devices/{}/heartbeat", id_segment(device_id)?);
        self.device_item(&path, Payload::Json(json(input)?), options)
            .await
    }

    /// 手动脱离：之后只有明确恢复才会再次上线。
    pub async fn disconnect(
        &self,
        device_id: &str,
        options: &RequestOptions<'_>,
    ) -> Result<JarvisRecord, ClientError> {
        self.device_action(device_id, "disconnect", options).await
    }

    /// 正常退出：立刻离线，但保留下次启动的连接意图。
    pub async fn offline(
        &self,
        device_id: &str,
        options: &RequestOptions<'_>,
    ) -> Result<JarvisRecord, ClientError> {
        self.device_action(device_id, "offline", options).await
    }

    async fn device_action(
        &self,
        device_id: &str,
        action: &str,
        options: &RequestOptions<'_>,
    ) -> Result<JarvisRecord, ClientError> {
        let path = format!("/api/jarvis/devices/{}/{}", id_segment(device_id)?, action);
        self.device_item(&path, Payload::Json(empty_object()), options)
            .await
    }

    async fn device_item(
        &self,
        path: &str,
        payload: Payload,
        options: &RequestOptions<'_>,
    ) -> Result<JarvisRecord, ClientError> {
        let call = Call::get(path).post(payload);
        let item: JarvisItem = self.call_json(call, options).await?;
        Ok(item.item)
    }

    /// 领取命令。带上等待秒数时服务端会压住请求，直到有命令或超时。
    pub async fn commands(
        &self,
        device_id: &str,
        wait: WaitSeconds,
        options: &RequestOptions<'_>,
    ) -> Result<Vec<DeviceCommand>, ClientError> {
        let path = format!("/api/jarvis/devices/{}/commands", id_segment(device_id)?);
        let call = Call::get(&path)
            .query("waitSeconds", wait.get().to_string())
            .long_poll();
        let response: CommandsResponse = self.call_json(call, options).await?;
        Ok(response.items)
    }

    /// 观察一条命令；带上等待秒数时压住请求，直到它到达终态。
    pub async fn get_command(
        &self,
        command_id: &str,
        wait: WaitSeconds,
        device_id: Option<&str>,
        options: &RequestOptions<'_>,
    ) -> Result<DeviceCommand, ClientError> {
        let path = format!("/api/jarvis/commands/{}", id_segment(command_id)?);
        let mut call = Call::get(&path)
            .query("waitSeconds", wait.get().to_string())
            .long_poll();
        if let Some(device) = device_id {
            call = call.query("deviceId", id_segment(device)?.to_owned());
        }
        let response: CommandResponse = self.call_json(call, options).await?;
        Ok(response.command)
    }

    pub async fn request_root(
        &self,
        device_id: &str,
        input: &DirectoryRequestInput,
        options: &RequestOptions<'_>,
    ) -> Result<DeviceCommand, ClientError> {
        let path = format!(
            "/api/jarvis/devices/{}/directory-request",
            id_segment(device_id)?
        );
        let call = Call::get(&path).post(Payload::Json(json(input)?));
        let response: CommandResponse = self.call_json(call, options).await?;
        Ok(response.command)
    }

    pub async fn inventory(
        &self,
        device_id: &str,
        input: &InventoryInput,
        options: &RequestOptions<'_>,
    ) -> Result<JarvisList, ClientError> {
        let path = format!("/api/jarvis/devices/{}/inventory", id_segment(device_id)?);
        let call = Call::get(&path).post(Payload::Json(json(input)?));
        self.call_json(call, options).await
    }

    pub async fn append_logs(
        &self,
        device_id: &str,
        logs: &[LogData],
        options: &RequestOptions<'_>,
    ) -> Result<JarvisList, ClientError> {
        if logs.len() > MAX_LOG_BATCH {
            return Err(ClientError::InvalidRequest);
        }
        let path = format!("/api/jarvis/devices/{}/logs", id_segment(device_id)?);
        let call = Call::get(&path).post(Payload::Json(json!({ "items": logs })));
        self.call_json(call, options).await
    }

    pub async fn report(
        &self,
        command_id: &str,
        report: &DeviceReport,
        options: &RequestOptions<'_>,
    ) -> Result<DeviceCommand, ClientError> {
        let path = format!("/api/jarvis/commands/{}/report", id_segment(command_id)?);
        let call = Call::get(&path).post(Payload::Json(json(report)?));
        let response: CommandResponse = self.call_json(call, options).await?;
        Ok(response.command)
    }

    // ---- 发送管线 ----

    /// 发出请求，校验状态，再按约定结构解析。
    async fn call_json<T: DeserializeOwned>(
        &self,
        call: Call<'_>,
        options: &RequestOptions<'_>,
    ) -> Result<T, ClientError> {
        let response = self.send(call, options).await?;
        decode(checked(response).await?).await
    }

    /// 所有请求的必经之路。
    async fn send(
        &self,
        call: Call<'_>,
        options: &RequestOptions<'_>,
    ) -> Result<reqwest::Response, ClientError> {
        ensure_allowed_path(call.path)?;
        if options.cancel.is_cancelled() {
            return Err(ClientError::Cancelled);
        }

        let is_write = call.method != Method::GET;
        let mut request = self
            .http
            .request(call.method, self.deployment.api.join(call.path))
            .header(ACCEPT, call.accept.value())
            .query(&call.query);
        if call.channel == Channel::Authenticated
            && let Some(token) = self.bearer()
        {
            let header = Zeroizing::new(format!("Bearer {}", token.as_str()));
            request = request.header(AUTHORIZATION, header.as_str());
        }
        if is_write {
            // 服务端用 Origin 判断请求是否来自可信的网页来源。
            request = request.header(ORIGIN, self.deployment.client.origin().as_str());
        }
        request = match call.payload {
            Payload::None => request,
            Payload::Json(value) => request.json(&value),
            Payload::Multipart(form) => request.multipart(form),
        };

        let budget = options.budget(call.long_poll);
        tokio::select! {
            biased;
            () = options.cancel.cancelled() => Err(ClientError::Cancelled),
            outcome = tokio::time::timeout(budget, request.send()) => match outcome {
                Ok(Ok(response)) => Ok(response),
                Ok(Err(error)) => Err(transport(&error)),
                Err(_) => Err(ClientError::Timeout),
            },
        }
    }
}

/// 没有输入的写接口也要带一个 JSON 对象作为正文。
fn empty_object() -> Value {
    Value::Object(serde_json::Map::new())
}

fn empty_post(path: &str) -> Call<'_> {
    Call::get(path).post(Payload::Json(empty_object()))
}

fn record_path(resource: ResourceName, id: &str) -> Result<String, ClientError> {
    Ok(format!(
        "/api/jarvis/resources/{}/{}",
        resource.as_str(),
        id_segment(id)?
    ))
}

/// 只允许访问 API 与两个公开的探活地址，且路径里不能有需要特殊处理的片段。
fn ensure_allowed_path(path: &str) -> Result<(), ClientError> {
    let allowed = path.starts_with("/api/") || matches!(path, "/health" | "/ready");
    let no_relative_segments = path
        .split('/')
        .all(|segment| !matches!(segment, "." | ".."));
    if allowed && no_relative_segments && !path.contains(['%', '\\']) {
        Ok(())
    } else {
        Err(ClientError::InvalidRequest)
    }
}

/// 标识只含 `A-Z a-z 0-9 _ . -`，放进路径不需要再编码。
///
/// 契约的正则也接受 `.` 与 `..`，但它们拼进路径后会被 URL 规范化吃掉一段，
/// 所以在这里单独挡掉。
fn id_segment(id: &str) -> Result<&str, ClientError> {
    if is_valid_id(id) && !matches!(id, "." | "..") {
        Ok(id)
    } else {
        Err(ClientError::InvalidRequest)
    }
}

fn json<T: Serialize>(value: &T) -> Result<Value, ClientError> {
    serde_json::to_value(value).map_err(|_| ClientError::InvalidRequest)
}

fn transport(error: &reqwest::Error) -> ClientError {
    if error.is_timeout() {
        ClientError::Timeout
    } else if error.is_builder() {
        ClientError::InvalidRequest
    } else {
        ClientError::Network
    }
}

/// 非 2xx 一律转成带状态码与远程错误码的错误。
async fn checked(response: reqwest::Response) -> Result<reqwest::Response, ClientError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.bytes().await.ok();
    let remote_code = body
        .as_deref()
        .and_then(|bytes| serde_json::from_slice::<ApiErrorBody>(bytes).ok())
        .map(|body| body.error.code);
    Err(ClientError::Http {
        status: status.as_u16(),
        remote_code,
    })
}

async fn decode<T: DeserializeOwned>(response: reqwest::Response) -> Result<T, ClientError> {
    let body = response.bytes().await.map_err(|error| transport(&error))?;
    serde_json::from_slice(&body).map_err(|_| ClientError::InvalidResponse)
}

#[cfg(test)]
mod tests;
