//! [`Client`]：账户、设备、云盘与发送的接口。
//!
//! 所有方法都经过同一个发送管线，由它统一处理超时、取消、凭据头、
//! 来源固定与错误归类。接口的路径、请求体与响应结构都在 `assemble-protocol` 里。
//!
//! 上传与下载都是流式的：上传时文件只读一遍、边读边报进度；下载交出的是一个
//! 读取器，由调用方交给 `assemble-transfer` 落盘。两边都盯着"卡住"——
//! 超过一段时间没有任何字节流动，就按超时处理，而不是干等。

use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use futures_core::Stream;
use futures_util::TryStreamExt;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE};
use reqwest::{Body, Client as HttpClient, Method, RequestBuilder, redirect};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, ReadBuf};
use tokio_util::io::StreamReader;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

use assemble_protocol::device::{Device, DeviceListing, RenameDeviceInput};
use assemble_protocol::drive::{DriveFile, DriveListing, RenameInput};
use assemble_protocol::error::ApiErrorBody;
use assemble_protocol::health::{HEALTH_PATH, HealthResponse};
use assemble_protocol::login::{EXCHANGE_PATH, ExchangeInput, ExchangeResponse, MeResponse};
use assemble_protocol::memo::{Memo, MemoCreate, MemoList, MemoQuery, MemoUpdate};
use assemble_protocol::primitives::{MAX_FILE_SIZE, is_valid_id};
use assemble_protocol::schedule::{
    Planner, ReadInput, Reminder, ReminderContent, ReminderUpdate, Schedule, ScheduleContent,
    ScheduleUpdate, Versioned,
};
// 协议里的"一次发送"叫 `Send`，与 `std::marker::Send` 重名；这里换个名字引进来。
use assemble_protocol::sends::{FailInput, Send as SendRecord, SendListing, SendQuery};
use assemble_store::secret::SecretString;

use crate::error::ClientError;
use assemble_protocol::origin::ServerOrigin;

/// 普通请求的时限。
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

/// 传输时多久没有任何字节流动就按超时处理。
pub const TRANSFER_STALL: Duration = Duration::from_secs(60);

/// 上传时每次读取的字节数。
const UPLOAD_CHUNK_BYTES: usize = 64 * 1024;

/// 上传时汇报进度的间隔。
const PROGRESS_INTERVAL: Duration = Duration::from_millis(150);

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

    fn budget(self) -> Duration {
        self.timeout.unwrap_or(DEFAULT_TIMEOUT)
    }
}

/// 一次传输（上传或下载）的退出条件。传输没有总时限：文件多大、网络多慢
/// 都不该让它超时；只在卡住太久时放弃。
#[derive(Debug, Clone, Copy)]
pub struct TransferOptions<'a> {
    pub cancel: &'a CancellationToken,
    /// 多久没有任何字节流动就按超时处理。
    pub stall: Duration,
}

impl<'a> TransferOptions<'a> {
    pub fn new(cancel: &'a CancellationToken) -> Self {
        Self {
            cancel,
            stall: TRANSFER_STALL,
        }
    }
}

/// 传输进度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferProgress {
    /// 已经传过去的字节数。
    pub transferred: u64,
    /// 总字节数。
    pub total: u64,
}

impl TransferProgress {
    /// 0 到 1 之间的完成比例；总量为零时为 1。
    pub fn fraction(self) -> f64 {
        if self.total == 0 {
            1.0
        } else {
            // 文件不超过 100 MB，两个数都放得进 u32，再转 f64 不会丢精度。
            let transferred = u32::try_from(self.transferred).unwrap_or(u32::MAX);
            let total = u32::try_from(self.total).unwrap_or(u32::MAX);
            (f64::from(transferred) / f64::from(total)).clamp(0.0, 1.0)
        }
    }
}

/// 进度回调。
pub type ProgressCallback<'a> = &'a mut (dyn FnMut(TransferProgress) + Send);

/// 一个准备发出去的本机文件：已经确认过大小与摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutgoingFile {
    pub path: PathBuf,
    pub size: u64,
    pub sha256: String,
}

impl OutgoingFile {
    /// 确认一个本机文件可以发出去：是普通文件、不超过单文件上限，并算出摘要。
    ///
    /// 摘要要放在请求里，所以必须先把整个文件读一遍；100 MB 以内的文件这一步很快。
    pub async fn open(path: &Path, cancel: &CancellationToken) -> Result<Self, ClientError> {
        let metadata = tokio::fs::metadata(path)
            .await
            .map_err(|_| ClientError::LocalSource)?;
        if !metadata.is_file() {
            return Err(ClientError::LocalSource);
        }
        // 先看大小再读内容：太大的文件不必白读一遍。
        if metadata.len() > MAX_FILE_SIZE {
            return Err(ClientError::TooLarge);
        }
        let file = assemble_transfer::source::inspect(path, cancel)
            .await
            .map_err(|error| local_failure(&error))?;
        if file.size > MAX_FILE_SIZE {
            return Err(ClientError::TooLarge);
        }
        Ok(Self {
            path: file.path,
            size: file.size,
            sha256: file.sha256,
        })
    }

    /// 发出去时用的名字：本机文件名。
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().trim().to_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "file".to_owned())
    }

    /// 按扩展名猜的媒体类型；猜不出就是二进制。
    pub fn mime_type(&self) -> String {
        mime_guess::from_path(&self.path)
            .first()
            .map_or("application/octet-stream".to_owned(), |guess| {
                guess.essence_str().to_owned()
            })
    }
}

fn local_failure(error: &assemble_transfer::TransferError) -> ClientError {
    match error {
        assemble_transfer::TransferError::Cancelled => ClientError::Cancelled,
        assemble_transfer::TransferError::TooLarge => ClientError::TooLarge,
        _ => ClientError::LocalSource,
    }
}

/// 一次下载的响应体：按到达的顺序流式读取，不把整个文件放进内存。
pub struct Download<'a> {
    /// 服务端声明的总长度；没有声明时为 `None`。
    pub content_length: Option<u64>,
    reader: MeteredReader<'a>,
}

impl<'a> Download<'a> {
    /// 取出可读取的字节流。读取中断或卡住时得到一个 I/O 错误。
    pub fn into_reader(self) -> MeteredReader<'a> {
        self.reader
    }
}

impl std::fmt::Debug for Download<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Download")
            .field("content_length", &self.content_length)
            .field("reader", &"byte stream")
            .finish()
    }
}

/// 下载的字节流：每读到一段就报进度；卡住太久时报超时错误。
pub struct MeteredReader<'a> {
    inner: StreamReader<Pin<Box<dyn Stream<Item = io::Result<Bytes>> + Send>>, Bytes>,
    stall: Duration,
    timer: Option<Pin<Box<tokio::time::Sleep>>>,
    total: Option<u64>,
    received: u64,
    progress: Option<&'a mut (dyn FnMut(TransferProgress) + Send)>,
}

impl AsyncRead for MeteredReader<'_> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = &mut *self;
        // 调用方可能带着已经装了一部分的缓冲区来，只算这一次新添的字节。
        let before = buf.filled().len();
        match Pin::new(&mut this.inner).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {
                let added = (buf.filled().len() - before) as u64;
                if added > 0 {
                    this.timer = None;
                    this.received = this.received.saturating_add(added);
                    if let Some(report) = this.progress.as_deref_mut() {
                        report(TransferProgress {
                            transferred: this.received,
                            total: this.total.unwrap_or(this.received),
                        });
                    }
                }
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Pending => {
                if this.timer.is_none() {
                    this.timer = Some(Box::pin(tokio::time::sleep(this.stall)));
                }
                match this.timer.as_mut().map(Pin::as_mut).unwrap().poll(cx) {
                    Poll::Ready(()) => Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "the download stopped yielding data",
                    ))),
                    Poll::Pending => Poll::Pending,
                }
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ClientBuildError {
    #[error("the HTTP client could not be configured: {0}")]
    Transport(#[from] reqwest::Error),
}

/// 访问服务端的一个客户端。克隆共享同一份令牌。
#[derive(Clone)]
pub struct Client {
    origin: Arc<ServerOrigin>,
    http: HttpClient,
    bearer: Arc<Mutex<Option<SecretString>>>,
    pub(crate) remote_security: crate::remote_secure::Sessions,
}

impl Client {
    /// 为一个服务来源创建客户端。请求只发往这个来源，且永远不跟随重定向。
    pub fn new(origin: ServerOrigin) -> Result<Self, ClientBuildError> {
        // 不带 Cookie 罐：桌面端从不持有网页登录状态，登录只发生在系统浏览器里。
        let http = HttpClient::builder()
            .redirect(redirect::Policy::none())
            .build()?;
        Ok(Self {
            origin: Arc::new(origin),
            http,
            bearer: Arc::new(Mutex::new(None)),
            remote_security: crate::remote_secure::Sessions::default(),
        })
    }

    pub fn origin(&self) -> &ServerOrigin {
        &self.origin
    }

    /// 换用新的设备令牌；传 `None` 表示已退出。
    pub fn set_bearer(&self, token: Option<SecretString>) {
        self.remote_security.clear();
        *self.bearer.lock().unwrap_or_else(PoisonError::into_inner) = token;
    }

    pub fn has_bearer(&self) -> bool {
        self.bearer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    /// `Authorization` 请求头的值：`Bearer <令牌>`。没有令牌时为 `None`。
    pub(crate) fn authorization(&self) -> Option<Zeroizing<String>> {
        self.bearer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|token| Zeroizing::new(format!("Bearer {}", token.expose())))
    }

    /// 只允许访问接口与两个公开的探活地址，且路径里不能有需要特殊处理的片段。
    pub(crate) fn bare_request(
        &self,
        method: Method,
        path: &str,
    ) -> Result<RequestBuilder, ClientError> {
        let allowed = path.starts_with("/v1/") || path == HEALTH_PATH || path == "/readyz";
        let no_relative_segments = path
            .split('/')
            .all(|segment| !matches!(segment, "." | ".."));
        if allowed && no_relative_segments && !path.contains(['%', '\\']) {
            Ok(self.http.request(method, self.origin.join(path)))
        } else {
            Err(ClientError::InvalidRequest)
        }
    }

    // ---- 公开接口 ----

    /// `GET /healthz`：进程能应答。
    pub async fn health(&self, options: &RequestOptions<'_>) -> Result<(), ClientError> {
        let call = Call::get(HEALTH_PATH).anonymous();
        let response: HealthResponse = self.call_json(call, options).await?;
        if response.status == "ok" {
            Ok(())
        } else {
            Err(ClientError::InvalidResponse)
        }
    }

    /// 用授权码换取设备令牌。这个请求不带任何令牌。
    pub async fn exchange(
        &self,
        input: &ExchangeInput,
        options: &RequestOptions<'_>,
    ) -> Result<(ExchangeResponse, SecretString), ClientError> {
        if !input.is_valid() {
            return Err(ClientError::InvalidRequest);
        }
        let call = Call::get(EXCHANGE_PATH)
            .post(Payload::Json(json(input)?))
            .anonymous();
        let mut response: ExchangeResponse = self.call_json(call, options).await?;
        // 令牌立刻离开普通字符串：之后只有零化后的那一份。
        let token = SecretString::new(std::mem::take(&mut response.token));
        Ok((response, token))
    }

    // ---- 账户 ----

    /// `GET /v1/me`：当前令牌对应的账户与设备。
    pub async fn me(&self, options: &RequestOptions<'_>) -> Result<MeResponse, ClientError> {
        self.call_json(Call::get("/v1/me"), options).await
    }

    /// `POST /v1/auth/logout`：撤销这台设备的令牌并删除设备记录。
    pub async fn logout(&self, options: &RequestOptions<'_>) -> Result<(), ClientError> {
        self.call_empty(Call::get("/v1/auth/logout").post(Payload::None), options)
            .await
    }

    // ---- 设备 ----

    pub async fn devices(&self, options: &RequestOptions<'_>) -> Result<Vec<Device>, ClientError> {
        let listing: DeviceListing = self.call_json(Call::get("/v1/devices"), options).await?;
        Ok(listing.items)
    }

    pub async fn rename_device(
        &self,
        id: &str,
        input: &RenameDeviceInput,
        options: &RequestOptions<'_>,
    ) -> Result<Device, ClientError> {
        if !input.is_valid() {
            return Err(ClientError::InvalidRequest);
        }
        let path = format!("/v1/devices/{}", id_segment(id)?);
        let call = Call::get(&path).with_payload(Method::PATCH, Payload::Json(json(input)?));
        self.call_json(call, options).await
    }

    /// 撤销另一台设备：它的令牌立刻失效，也不再出现在设备列表里。
    pub async fn remove_device(
        &self,
        id: &str,
        options: &RequestOptions<'_>,
    ) -> Result<(), ClientError> {
        let path = format!("/v1/devices/{}", id_segment(id)?);
        self.call_empty(Call::get(&path).delete(), options).await
    }

    // ---- 云盘 ----

    pub async fn drive(&self, options: &RequestOptions<'_>) -> Result<DriveListing, ClientError> {
        self.call_json(Call::get("/v1/drive/files"), options).await
    }

    /// 把一个本机文件存进云盘。
    pub async fn drive_upload(
        &self,
        name: &str,
        file: &OutgoingFile,
        options: TransferOptions<'_>,
        progress: Option<ProgressCallback<'_>>,
    ) -> Result<DriveFile, ClientError> {
        let query = upload_query(name, file)?;
        self.upload("/v1/drive/files", query, file, options, progress)
            .await
    }

    /// 取出云盘里一个文件的内容。
    pub async fn drive_content<'a>(
        &self,
        id: &str,
        options: TransferOptions<'a>,
        progress: Option<ProgressCallback<'a>>,
    ) -> Result<Download<'a>, ClientError> {
        let path = format!("/v1/drive/files/{}/content", id_segment(id)?);
        self.download(&path, options, progress).await
    }

    pub async fn drive_rename(
        &self,
        id: &str,
        input: &RenameInput,
        options: &RequestOptions<'_>,
    ) -> Result<DriveFile, ClientError> {
        if input.clean_name().is_none() {
            return Err(ClientError::InvalidRequest);
        }
        let path = format!("/v1/drive/files/{}", id_segment(id)?);
        let call = Call::get(&path).with_payload(Method::PATCH, Payload::Json(json(input)?));
        self.call_json(call, options).await
    }

    pub async fn drive_delete(
        &self,
        id: &str,
        options: &RequestOptions<'_>,
    ) -> Result<(), ClientError> {
        let path = format!("/v1/drive/files/{}", id_segment(id)?);
        self.call_empty(Call::get(&path).delete(), options).await
    }

    // ---- 账户私有备忘录 ----
    pub async fn drive_trash_list(
        &self,
        q: &str,
        page: u32,
        page_size: u32,
        options: &RequestOptions<'_>,
    ) -> Result<assemble_protocol::drive_trash::Listing, ClientError> {
        if q.chars().count() > 200 || page == 0 || !(1..=100).contains(&page_size) {
            return Err(ClientError::InvalidRequest);
        }
        self.call_json(
            Call::get("/v1/drive/trash")
                .query("q", q.to_owned())
                .query("page", page.to_string())
                .query("pageSize", page_size.to_string()),
            options,
        )
        .await
    }
    pub async fn drive_trash_detail(
        &self,
        id: &str,
        query: &assemble_protocol::drive_tree::Query,
        options: &RequestOptions<'_>,
    ) -> Result<assemble_protocol::drive_trash::Detail, ClientError> {
        if !query.valid() {
            return Err(ClientError::InvalidRequest);
        }
        let path = format!("/v1/drive/trash/{}", id_segment(id)?);
        self.call_json(
            Call::get(&path)
                .query("parentId", query.parent_id.clone())
                .query("q", query.q.clone())
                .query("page", query.page.to_string())
                .query("pageSize", query.page_size.to_string()),
            options,
        )
        .await
    }
    pub async fn drive_trash_preview(
        &self,
        target: &assemble_protocol::drive_trash::Target,
        options: &RequestOptions<'_>,
    ) -> Result<assemble_protocol::drive_trash::Preview, ClientError> {
        self.call_json(
            Call::get("/v1/drive/trash/preview").post(Payload::Json(json(target)?)),
            options,
        )
        .await
    }
    pub async fn drive_trash_move(
        &self,
        input: &assemble_protocol::drive_trash::Move,
        options: &RequestOptions<'_>,
    ) -> Result<assemble_protocol::drive_trash::Batch, ClientError> {
        self.call_json(
            Call::get("/v1/drive/trash").post(Payload::Json(json(input)?)),
            options,
        )
        .await
    }
    pub async fn drive_trash_restore_preview(
        &self,
        id: &str,
        options: &RequestOptions<'_>,
    ) -> Result<assemble_protocol::drive_trash::RestorePreview, ClientError> {
        let path = format!("/v1/drive/trash/{}/restore-preview", id_segment(id)?);
        self.call_json(Call::get(&path).post(Payload::None), options)
            .await
    }
    pub async fn drive_trash_restore(
        &self,
        id: &str,
        input: &assemble_protocol::drive_trash::Restore,
        options: &RequestOptions<'_>,
    ) -> Result<assemble_protocol::drive_trash::Restored, ClientError> {
        let path = format!("/v1/drive/trash/{}/restore", id_segment(id)?);
        self.call_json(Call::get(&path).post(Payload::Json(json(input)?)), options)
            .await
    }
    pub async fn drive_trash_purge(
        &self,
        id: &str,
        version: i64,
        options: &RequestOptions<'_>,
    ) -> Result<assemble_protocol::drive_trash::Purged, ClientError> {
        if version < 1 {
            return Err(ClientError::InvalidRequest);
        }
        let path = format!("/v1/drive/trash/{}", id_segment(id)?);
        self.call_json(
            Call::get(&path)
                .query("version", version.to_string())
                .delete(),
            options,
        )
        .await
    }
    /// 新目录接口不存在时由调用方明确回退旧名单，不能静默丢掉目标目录。
    pub async fn drive_tree(
        &self,
        query: &assemble_protocol::drive_tree::Query,
        options: &RequestOptions<'_>,
    ) -> Result<assemble_protocol::drive_tree::Listing, ClientError> {
        if !query.valid() {
            return Err(ClientError::InvalidRequest);
        }
        let call = Call::get("/v1/drive/tree")
            .query("foldersOnly", query.folders_only.to_string())
            .query("parentId", query.parent_id.clone())
            .query("q", query.q.clone())
            .query("page", query.page.to_string())
            .query("pageSize", query.page_size.to_string());
        self.call_json(call, options).await
    }
    pub async fn drive_folder_create(
        &self,
        input: &assemble_protocol::drive_tree::Create,
        options: &RequestOptions<'_>,
    ) -> Result<assemble_protocol::drive_tree::Folder, ClientError> {
        if !input.valid() {
            return Err(ClientError::InvalidRequest);
        }
        self.call_json(
            Call::get("/v1/drive/tree/folders").post(Payload::Json(json(input)?)),
            options,
        )
        .await
    }
    pub async fn drive_tree_upload(
        &self,
        parent: &str,
        name: &str,
        file: &OutgoingFile,
        options: TransferOptions<'_>,
        progress: Option<ProgressCallback<'_>>,
    ) -> Result<assemble_protocol::drive_tree::TreeFile, ClientError> {
        if !assemble_protocol::drive_tree::valid_parent(parent) {
            return Err(ClientError::InvalidRequest);
        }
        let mut query = upload_query(name, file)?;
        query.push(("parentId", parent.to_owned()));
        self.upload("/v1/drive/tree/files", query, file, options, progress)
            .await
    }
    pub async fn drive_tree_rename(
        &self,
        kind: assemble_protocol::drive_tree::Kind,
        id: &str,
        input: &assemble_protocol::drive_tree::Rename,
        options: &RequestOptions<'_>,
    ) -> Result<assemble_protocol::drive_tree::Entry, ClientError> {
        if input.version < 1 || assemble_protocol::drive::clean_file_name(&input.name).is_none() {
            return Err(ClientError::InvalidRequest);
        }
        let path = tree_path(kind, id)?;
        self.call_json(
            Call::get(&path).with_payload(Method::PATCH, Payload::Json(json(input)?)),
            options,
        )
        .await
    }
    pub async fn drive_tree_move(
        &self,
        input: &assemble_protocol::drive_tree::Move,
        options: &RequestOptions<'_>,
    ) -> Result<assemble_protocol::drive_tree::Entry, ClientError> {
        if input.version < 1
            || !is_valid_id(&input.id)
            || !assemble_protocol::drive_tree::valid_parent(&input.parent_id)
        {
            return Err(ClientError::InvalidRequest);
        }
        self.call_json(
            Call::get("/v1/drive/tree/moves").post(Payload::Json(json(input)?)),
            options,
        )
        .await
    }
    pub async fn drive_tree_delete(
        &self,
        kind: assemble_protocol::drive_tree::Kind,
        id: &str,
        version: i64,
        options: &RequestOptions<'_>,
    ) -> Result<(), ClientError> {
        if version < 1 {
            return Err(ClientError::InvalidRequest);
        }
        let path = tree_path(kind, id)?;
        self.call_empty(
            Call::get(&path)
                .query("version", version.to_string())
                .delete(),
            options,
        )
        .await
    }

    // ---- 账户私有备忘录 ----

    pub async fn memo_list(
        &self,
        query: &MemoQuery,
        options: &RequestOptions<'_>,
    ) -> Result<MemoList, ClientError> {
        if !query.valid() {
            return Err(ClientError::InvalidRequest);
        }
        let mut call = Call::get("/v1/memos");
        call.query = vec![
            ("q", query.q.clone()),
            ("page", query.page.to_string()),
            ("pageSize", query.page_size.to_string()),
        ];
        self.call_json(call, options).await
    }

    pub async fn memo_get(
        &self,
        id: &str,
        options: &RequestOptions<'_>,
    ) -> Result<Memo, ClientError> {
        self.call_json(
            Call::get(&format!("/v1/memos/{}", id_segment(id)?)),
            options,
        )
        .await
    }

    pub async fn memo_create(
        &self,
        input: &MemoCreate,
        options: &RequestOptions<'_>,
    ) -> Result<Memo, ClientError> {
        if !input.valid() {
            return Err(ClientError::InvalidRequest);
        }
        self.call_json(
            Call::get("/v1/memos").post(Payload::Json(json(input)?)),
            options,
        )
        .await
    }

    pub async fn memo_update(
        &self,
        id: &str,
        input: &MemoUpdate,
        options: &RequestOptions<'_>,
    ) -> Result<Memo, ClientError> {
        if input.content.cleaned().is_none() || input.version < 1 || input.version == i64::MAX {
            return Err(ClientError::InvalidRequest);
        }
        self.call_json(
            Call::get(&format!("/v1/memos/{}", id_segment(id)?)).put(Payload::Json(json(input)?)),
            options,
        )
        .await
    }

    pub async fn memo_remove(
        &self,
        id: &str,
        version: i64,
        options: &RequestOptions<'_>,
    ) -> Result<(), ClientError> {
        if version < 1 {
            return Err(ClientError::InvalidRequest);
        }
        let path = format!("/v1/memos/{}", id_segment(id)?);
        let call = Call::get(&path).delete_with(Payload::Json(json(&Versioned { version })?));
        self.call_empty(call, options).await
    }

    // ---- 日程规划 ----

    /// `GET /v1/planner`：日程、提醒与还没读的通知。
    pub async fn planner(&self, options: &RequestOptions<'_>) -> Result<Planner, ClientError> {
        self.call_json(Call::get("/v1/planner"), options).await
    }

    /// 按所属记录重新授权导航，不接受调用者提供的目标 URL。
    pub async fn record_locations(
        &self,
        resource: &str,
        id: &str,
        options: &RequestOptions<'_>,
    ) -> Result<Vec<assemble_protocol::schedule::Location>, ClientError> {
        if !matches!(resource, "schedule" | "reminder" | "memo") {
            return Err(ClientError::InvalidRequest);
        }
        self.call_json(
            Call::get(&format!(
                "/v1/planner/locations/{resource}/{}",
                id_segment(id)?
            )),
            options,
        )
        .await
    }

    /// 新建一条日程。`idempotency_key` 用过的话拿回的是原来那一条。
    pub async fn create_schedule(
        &self,
        content: &ScheduleContent,
        idempotency_key: Option<&str>,
        options: &RequestOptions<'_>,
    ) -> Result<Schedule, ClientError> {
        if content.cleaned().is_none() {
            return Err(ClientError::InvalidRequest);
        }
        // 幂等键放在请求头里；内容不合格的请求在上面就已经拦下了。
        let call = idempotent(Call::get("/v1/schedules"), idempotency_key)
            .post(Payload::Json(json(content)?));
        self.call_json(call, options).await
    }

    /// 整份替换一条日程的内容。版本号对不上时得到 `VERSION_CONFLICT`。
    pub async fn update_schedule(
        &self,
        id: &str,
        update: &ScheduleUpdate,
        options: &RequestOptions<'_>,
    ) -> Result<Schedule, ClientError> {
        if update.content.cleaned().is_none() {
            return Err(ClientError::InvalidRequest);
        }
        let path = format!("/v1/schedules/{}", id_segment(id)?);
        let call = Call::get(&path).put(Payload::Json(json(update)?));
        self.call_json(call, options).await
    }

    /// 改一条日程的状态：完成、取消，或者改回进行中。
    pub async fn set_schedule_status(
        &self,
        id: &str,
        action: ScheduleAction,
        version: i64,
        options: &RequestOptions<'_>,
    ) -> Result<Schedule, ClientError> {
        let path = format!("/v1/schedules/{}/{}", id_segment(id)?, action.path());
        let call = Call::get(&path).post(Payload::Json(json(&Versioned { version })?));
        self.call_json(call, options).await
    }

    pub async fn delete_schedule(
        &self,
        id: &str,
        version: i64,
        options: &RequestOptions<'_>,
    ) -> Result<(), ClientError> {
        let path = format!("/v1/schedules/{}", id_segment(id)?);
        let call = Call::get(&path).delete_with(Payload::Json(json(&Versioned { version })?));
        self.call_empty(call, options).await
    }

    /// 新建一条提醒。`idempotency_key` 用过的话拿回的是原来那一条。
    pub async fn create_reminder(
        &self,
        content: &ReminderContent,
        idempotency_key: Option<&str>,
        options: &RequestOptions<'_>,
    ) -> Result<Reminder, ClientError> {
        if content.cleaned().is_none() {
            return Err(ClientError::InvalidRequest);
        }
        // 幂等键放在请求头里；内容不合格的请求在上面就已经拦下了。
        let call = idempotent(Call::get("/v1/reminders"), idempotency_key)
            .post(Payload::Json(json(content)?));
        self.call_json(call, options).await
    }

    /// 整份替换一条提醒的内容。已经到点的提醒不能改，得到 `INVALID_STATE`。
    pub async fn update_reminder(
        &self,
        id: &str,
        update: &ReminderUpdate,
        options: &RequestOptions<'_>,
    ) -> Result<Reminder, ClientError> {
        if update.content.cleaned().is_none() {
            return Err(ClientError::InvalidRequest);
        }
        let path = format!("/v1/reminders/{}", id_segment(id)?);
        let call = Call::get(&path).put(Payload::Json(json(update)?));
        self.call_json(call, options).await
    }

    /// 把一条提醒标成办完了，或者取消它。
    pub async fn set_reminder_status(
        &self,
        id: &str,
        action: ReminderAction,
        version: i64,
        options: &RequestOptions<'_>,
    ) -> Result<Reminder, ClientError> {
        let path = format!("/v1/reminders/{}/{}", id_segment(id)?, action.path());
        let call = Call::get(&path).post(Payload::Json(json(&Versioned { version })?));
        self.call_json(call, options).await
    }

    pub async fn delete_reminder(
        &self,
        id: &str,
        version: i64,
        options: &RequestOptions<'_>,
    ) -> Result<(), ClientError> {
        let path = format!("/v1/reminders/{}", id_segment(id)?);
        let call = Call::get(&path).delete_with(Payload::Json(json(&Versioned { version })?));
        self.call_empty(call, options).await
    }

    /// 把几条通知标成已读。
    pub async fn mark_notifications_read(
        &self,
        ids: &[&str],
        options: &RequestOptions<'_>,
    ) -> Result<(), ClientError> {
        if ids.iter().any(|id| !is_valid_id(id)) {
            return Err(ClientError::InvalidRequest);
        }
        let call = Call::get("/v1/notifications/read").post(Payload::Json(json(&ReadInput {
            ids: ids.iter().map(|id| (*id).to_owned()).collect(),
        })?));
        self.call_empty(call, options).await
    }

    pub async fn mark_all_notifications_read(
        &self,
        options: &RequestOptions<'_>,
    ) -> Result<(), ClientError> {
        self.call_empty(
            Call::get("/v1/notifications/read-all").post(Payload::None),
            options,
        )
        .await
    }

    // ---- 发送到设备 ----

    /// 把一个本机文件发给同一个账户的另一台在线设备。正文就是文件的字节。
    pub async fn send_to_device(
        &self,
        to_device_id: &str,
        file: &OutgoingFile,
        options: TransferOptions<'_>,
        progress: Option<ProgressCallback<'_>>,
    ) -> Result<SendRecord, ClientError> {
        let target = id_segment(to_device_id)?;
        let mut query = upload_query(&file.name(), file)?;
        query.insert(0, ("toDeviceId", target.to_owned()));
        self.upload("/v1/sends", query, file, options, progress)
            .await
    }

    pub async fn sends(
        &self,
        query: &SendQuery,
        options: &RequestOptions<'_>,
    ) -> Result<Vec<SendRecord>, ClientError> {
        if !query.is_valid() {
            return Err(ClientError::InvalidRequest);
        }
        let mut call = Call::get("/v1/sends")
            .query("direction", enum_spelling(query.direction)?)
            .query("limit", query.limit.to_string());
        if let Some(state) = query.state {
            call = call.query("state", enum_spelling(state)?);
        }
        let listing: SendListing = self.call_json(call, options).await?;
        Ok(listing.items)
    }

    /// 取出一次待接收发送的内容。内容只有处于 `offered` 时才在服务端。
    pub async fn send_content<'a>(
        &self,
        id: &str,
        options: TransferOptions<'a>,
        progress: Option<ProgressCallback<'a>>,
    ) -> Result<Download<'a>, ClientError> {
        let path = format!("/v1/sends/{}/content", id_segment(id)?);
        self.download(&path, options, progress).await
    }

    pub async fn send_accept(
        &self,
        id: &str,
        options: &RequestOptions<'_>,
    ) -> Result<SendRecord, ClientError> {
        self.send_action(id, "accept", options).await
    }

    pub async fn send_decline(
        &self,
        id: &str,
        options: &RequestOptions<'_>,
    ) -> Result<SendRecord, ClientError> {
        self.send_action(id, "decline", options).await
    }

    /// 发送方撤回一次还在等待接收的发送。
    pub async fn send_cancel(
        &self,
        id: &str,
        options: &RequestOptions<'_>,
    ) -> Result<SendRecord, ClientError> {
        self.send_action(id, "cancel", options).await
    }

    /// 接收方告诉发送方：这个文件我收不下来。
    pub async fn send_failed(
        &self,
        id: &str,
        input: FailInput,
        options: &RequestOptions<'_>,
    ) -> Result<SendRecord, ClientError> {
        let path = format!("/v1/sends/{}/failed", id_segment(id)?);
        let call = Call::get(&path).post(Payload::Json(json(&input)?));
        self.call_json(call, options).await
    }

    async fn send_action(
        &self,
        id: &str,
        action: &'static str,
        options: &RequestOptions<'_>,
    ) -> Result<SendRecord, ClientError> {
        let path = format!("/v1/sends/{id}/{action}", id = id_segment(id)?);
        self.call_json(Call::get(&path).post(Payload::None), options)
            .await
    }

    // ---- 发送管线 ----

    pub(crate) async fn json_request(
        &self,
        method: Method,
        path: &str,
        query: Vec<(&'static str, String)>,
        payload: Option<Value>,
        options: &RequestOptions<'_>,
    ) -> Result<reqwest::Response, ClientError> {
        let call = Call {
            method,
            path,
            query,
            payload: payload.map_or(Payload::None, Payload::Json),
            authenticated: self.has_bearer(),
            headers: Vec::new(),
        };
        checked(self.send(call, options.budget(), options.cancel).await?).await
    }

    /// 发出请求，校验状态，再按约定结构解析。
    async fn call_json<T: DeserializeOwned>(
        &self,
        call: Call<'_>,
        options: &RequestOptions<'_>,
    ) -> Result<T, ClientError> {
        let response = self.send(call, options.budget(), options.cancel).await?;
        decode(checked(response).await?).await
    }

    /// 发出一个没有正文的写请求，只关心成没成功。
    async fn call_empty(
        &self,
        call: Call<'_>,
        options: &RequestOptions<'_>,
    ) -> Result<(), ClientError> {
        let response = self.send(call, options.budget(), options.cancel).await?;
        checked(response).await?;
        Ok(())
    }

    /// 流式上传：正文是文件的字节，边读边报进度。
    async fn upload<T: DeserializeOwned>(
        &self,
        path: &'static str,
        query: Vec<(&'static str, String)>,
        file: &OutgoingFile,
        options: TransferOptions<'_>,
        mut progress: Option<ProgressCallback<'_>>,
    ) -> Result<T, ClientError> {
        if file.size > MAX_FILE_SIZE {
            return Err(ClientError::TooLarge);
        }
        let source = tokio::fs::File::open(&file.path)
            .await
            .map_err(|_| ClientError::LocalSource)?;
        let sent = Arc::new(AtomicU64::new(0));
        let stalled = Arc::new(AtomicBool::new(false));
        let body = Body::wrap_stream(metered(
            source,
            options.stall,
            Arc::clone(&sent),
            Arc::clone(&stalled),
        ));
        let mut request = self.bare_request(Method::POST, path)?.query(&query);
        request = request
            .header(CONTENT_TYPE, "application/octet-stream")
            .header(CONTENT_LENGTH, file.size)
            .header(ACCEPT, "application/json");
        if let Some(authorization) = self.authorization() {
            request = request.header(AUTHORIZATION, authorization.as_str());
        }

        // 一边发送，一边按固定间隔把已发送的字节数报给调用方。
        let mut sending = Box::pin(request.body(body).send());
        let mut ticker = tokio::time::interval(PROGRESS_INTERVAL);
        let total = file.size;
        let outcome = loop {
            tokio::select! {
                biased;
                () = options.cancel.cancelled() => return Err(ClientError::Cancelled),
                outcome = &mut sending => break outcome,
                _ = ticker.tick() => {
                    if let Some(report) = progress.as_deref_mut() {
                        report(TransferProgress { transferred: sent.load(Ordering::Relaxed), total });
                    }
                }
            }
        };
        let response = match outcome {
            Ok(response) => response,
            Err(error) => {
                return Err(if stalled.load(Ordering::SeqCst) {
                    ClientError::Timeout
                } else {
                    transport(&error)
                });
            }
        };
        let response = checked(response).await?;
        if let Some(report) = progress {
            report(TransferProgress {
                transferred: total,
                total,
            });
        }
        decode(response).await
    }

    /// 一次下载：校验状态后把正文交成一个读取器。
    async fn download<'a>(
        &self,
        path: &str,
        options: TransferOptions<'a>,
        progress: Option<ProgressCallback<'a>>,
    ) -> Result<Download<'a>, ClientError> {
        let mut request = self.bare_request(Method::GET, path)?;
        request = request.header(ACCEPT, "application/octet-stream");
        if let Some(authorization) = self.authorization() {
            request = request.header(AUTHORIZATION, authorization.as_str());
        }
        let response = tokio::select! {
            biased;
            () = options.cancel.cancelled() => return Err(ClientError::Cancelled),
            outcome = request.send() => match outcome {
                Ok(response) => response,
                Err(error) => return Err(transport(&error)),
            },
        };
        let response = checked(response).await?;
        let content_length = response.content_length();
        // 传输错误在离开客户端时转成 I/O 错误：调用方只需要处理一种错误。
        let stream: Pin<Box<dyn Stream<Item = io::Result<Bytes>> + Send>> =
            Box::pin(response.bytes_stream().map_err(io::Error::other));
        Ok(Download {
            content_length,
            reader: MeteredReader {
                inner: StreamReader::new(stream),
                stall: options.stall,
                timer: None,
                total: content_length,
                received: 0,
                progress,
            },
        })
    }

    /// 普通请求的必经之路：带凭据头、有时限、可取消。传输不走这里。
    async fn send(
        &self,
        call: Call<'_>,
        budget: Duration,
        cancel: &CancellationToken,
    ) -> Result<reqwest::Response, ClientError> {
        if cancel.is_cancelled() {
            return Err(ClientError::Cancelled);
        }
        let mut request = self
            .bare_request(call.method, call.path)?
            .query(&call.query)
            .header(ACCEPT, "application/json");
        for (name, value) in &call.headers {
            request = request.header(*name, value);
        }
        if call.authenticated {
            let Some(authorization) = self.authorization() else {
                // 还没登录就发需要登录的请求：在本地拦下，不发给服务端。
                return Err(ClientError::InvalidRequest);
            };
            request = request.header(AUTHORIZATION, authorization.as_str());
        }
        let sending = match call.payload {
            Payload::None => request.send(),
            Payload::Json(value) => request.json(&value).send(),
        };
        tokio::select! {
            biased;
            () = cancel.cancelled() => Err(ClientError::Cancelled),
            outcome = tokio::time::timeout(budget, sending) => match outcome {
                Ok(Ok(response)) => Ok(response),
                Ok(Err(error)) => Err(transport(&error)),
                Err(_) => Err(ClientError::Timeout),
            },
        }
    }
}

/// 一条日程能改成的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleAction {
    /// 整条日程标成已完成。
    Complete,
    Cancel,
    /// 完成或取消之后改回进行中。
    Reopen,
}

impl ScheduleAction {
    fn path(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Cancel => "cancel",
            Self::Reopen => "reopen",
        }
    }
}

/// 一条提醒能改成的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReminderAction {
    /// 办完了，或者到点之后看过了。
    Dismiss,
    Cancel,
}

impl ReminderAction {
    fn path(self) -> &'static str {
        match self {
            Self::Dismiss => "dismiss",
            Self::Cancel => "cancel",
        }
    }
}

/// 新建的共用部分：带了幂等键，就把它放进 `Idempotency-Key` 请求头。
fn idempotent<'a>(call: Call<'a>, key: Option<&str>) -> Call<'a> {
    match key {
        Some(key) => call.header("idempotency-key", key.to_owned()),
        None => call,
    }
}

fn tree_path(kind: assemble_protocol::drive_tree::Kind, id: &str) -> Result<String, ClientError> {
    let collection = match kind {
        assemble_protocol::drive_tree::Kind::Folder => "folders",
        assemble_protocol::drive_tree::Kind::File => "files",
    };
    Ok(format!("/v1/drive/tree/{collection}/{}", id_segment(id)?))
}

/// 一次调用的全部要素，交给发送管线之前先在这里组装好。
struct Call<'a> {
    method: Method,
    path: &'a str,
    query: Vec<(&'static str, String)>,
    payload: Payload,
    authenticated: bool,
    headers: Vec<(&'static str, String)>,
}

impl<'a> Call<'a> {
    fn get(path: &'a str) -> Self {
        Self {
            method: Method::GET,
            path,
            query: Vec::new(),
            payload: Payload::None,
            authenticated: true,
            headers: Vec::new(),
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

    fn delete(mut self) -> Self {
        self.method = Method::DELETE;
        self
    }

    /// 整份替换。
    fn put(self, payload: Payload) -> Self {
        self.with_payload(Method::PUT, payload)
    }

    /// 带着正文的删除：要删的东西也得带上看到的版本号。
    fn delete_with(self, payload: Payload) -> Self {
        self.with_payload(Method::DELETE, payload)
    }

    /// 多带一个请求头。
    fn header(mut self, name: &'static str, value: String) -> Self {
        self.headers.push((name, value));
        self
    }

    fn anonymous(mut self) -> Self {
        self.authenticated = false;
        self
    }

    fn query(mut self, key: &'static str, value: String) -> Self {
        self.query.push((key, value));
        self
    }
}

enum Payload {
    None,
    Json(Value),
}

/// 上传接口共用的查询参数：名字、大小、摘要、媒体类型。
fn upload_query(
    name: &str,
    file: &OutgoingFile,
) -> Result<Vec<(&'static str, String)>, ClientError> {
    if assemble_protocol::drive::clean_file_name(name).is_none() {
        return Err(ClientError::InvalidRequest);
    }
    Ok(vec![
        ("name", name.trim().to_owned()),
        ("size", file.size.to_string()),
        ("sha256", file.sha256.clone()),
        ("mimeType", file.mime_type()),
    ])
}

/// 把本机文件变成一个边读边计数的字节流。
fn metered(
    file: tokio::fs::File,
    stall: Duration,
    sent: Arc<AtomicU64>,
    stalled: Arc<AtomicBool>,
) -> impl Stream<Item = io::Result<Bytes>> + Send + 'static {
    let buffer = vec![0u8; UPLOAD_CHUNK_BYTES];
    futures_util::stream::unfold(
        (file, buffer, stall, sent, stalled),
        |(mut file, mut buffer, stall, sent, stalled)| async move {
            let read = match tokio::time::timeout(stall, file.read(&mut buffer)).await {
                Ok(Ok(read)) => read,
                Ok(Err(error)) => return Some((Err(error), (file, buffer, stall, sent, stalled))),
                Err(_) => {
                    stalled.store(true, Ordering::SeqCst);
                    let error = io::Error::new(
                        io::ErrorKind::TimedOut,
                        "the local file stopped yielding data",
                    );
                    return Some((Err(error), (file, buffer, stall, sent, stalled)));
                }
            };
            if read == 0 {
                return None;
            }
            sent.fetch_add(read as u64, Ordering::Relaxed);
            Some((
                Ok(Bytes::copy_from_slice(&buffer[..read])),
                (file, buffer, stall, sent, stalled),
            ))
        },
    )
}

/// 标识只含 `A-Z a-z 0-9 _ . -`，放进路径不需要再编码。
///
/// `.` 与 `..` 拼进路径后会被 URL 规范化吃掉一段，所以单独挡掉。
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

fn enum_spelling<T: Serialize>(value: T) -> Result<String, ClientError> {
    match json(&value)? {
        Value::String(spelling) => Ok(spelling),
        _ => Err(ClientError::InvalidRequest),
    }
}

pub(crate) fn transport(error: &reqwest::Error) -> ClientError {
    if error.is_timeout() {
        ClientError::Timeout
    } else if error.is_builder() {
        ClientError::InvalidRequest
    } else {
        ClientError::Network
    }
}

/// 非 2xx 一律转成带状态码与远程错误码的错误。
pub(crate) async fn rejection(status: u16, response: reqwest::Response) -> ClientError {
    let body = response.bytes().await.ok();
    let remote_code = body
        .as_deref()
        .and_then(|bytes| serde_json::from_slice::<ApiErrorBody>(bytes).ok())
        .map(|body| body.error.code);
    ClientError::Http {
        status,
        remote_code,
    }
}

async fn checked(response: reqwest::Response) -> Result<reqwest::Response, ClientError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    Err(rejection(status.as_u16(), response).await)
}

async fn decode<T: DeserializeOwned>(response: reqwest::Response) -> Result<T, ClientError> {
    let body = response.bytes().await.map_err(|error| transport(&error))?;
    serde_json::from_slice(&body).map_err(|_| ClientError::InvalidResponse)
}

#[cfg(test)]
mod tests;
