//! 任务来源只留内存；历史只保存展示字段与本机发布位置，不保存可重放的网络凭据。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use assemble_protocol::downloads::{
    DownloadInitiator, DownloadInput, DownloadKind, DownloadPhase, DownloadPreview, DownloadTask,
    DownloadVariant, DownloadsView,
};
use assemble_protocol::primitives::Timestamp;
use assemble_store::json::{read_json, write_private_json};
use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::hls::{self, Track, Variant};
use crate::network::{Network, headers, source_url};
use crate::space;
use crate::{DownloadError, MAX_METADATA, MediaTools, Result, douyin, random_id};

/// 一次下载的暂存目录在接收目录里叫这个名字开头。完成了它会连着内容一起消失；
/// 进程被杀掉的时候它就留下了，下次启动时由 [`DownloadManager::sweep_staging`] 清掉。
const STAGING_PREFIX: &str = ".assemble-download-";

#[derive(Clone)]
enum Source {
    Hls(Box<Variant>),
    Video(Url),
}
#[derive(Clone)]
struct Prepared {
    initiated_by: Option<DownloadInitiator>,
    preview: DownloadPreview,
    sources: Vec<(String, Source)>,
    headers: HeaderMap,
    at: Instant,
}
#[derive(Clone)]
struct Job {
    source: Source,
    headers: HeaderMap,
    folder: PathBuf,
}
#[derive(Clone, Serialize, Deserialize)]
struct Record {
    view: DownloadTask,
    path: Option<PathBuf>,
    #[serde(default)]
    sha256: Option<String>,
}
#[derive(Serialize, Deserialize)]
struct History {
    folder: PathBuf,
    records: Vec<Record>,
}
struct State {
    assistant_allowed: bool,
    browser_task_id: Option<String>,
    history: History,
    prepared: HashMap<String, Prepared>,
    jobs: HashMap<String, Job>,
    cancels: HashMap<String, CancellationToken>,
    handles: Vec<JoinHandle<()>>,
    emitted: Instant,
}

pub struct DownloadManager {
    state: Arc<Mutex<State>>,
    history_path: PathBuf,
    pub(crate) network: Network,
    tools: MediaTools,
    slots: Arc<Semaphore>,
    magnet: Arc<crate::magnet::MagnetManager>,
    starting: tokio::sync::Mutex<()>,
    lifetime: CancellationToken,
    changed: Arc<dyn Fn(DownloadsView) + Send + Sync>,
}

impl DownloadManager {
    /// `folder` 只允许调用方从系统下载目录或系统选择器获得。
    pub fn open(
        history_path: PathBuf,
        folder: PathBuf,
        tools: MediaTools,
        changed: impl Fn(DownloadsView) + Send + Sync + 'static,
    ) -> Result<Arc<Self>> {
        let mut history = read_json::<History>(&history_path)
            .map_err(|_| DownloadError::Storage)?
            .unwrap_or(History {
                folder,
                records: Vec::new(),
            });
        history.records.truncate(100);
        for record in &mut history.records {
            if record.view.phase.active() {
                record.view.phase = DownloadPhase::Interrupted;
                record.view.error_code = None;
            }
            record.view.retryable = false;
            record.view.bytes_per_second = 0;
        }
        write_private_json(&history_path, &history).map_err(|_| DownloadError::Storage)?;
        let slots = Arc::new(Semaphore::new(2));
        let state = Arc::new(Mutex::new(State {
            assistant_allowed: true,
            browser_task_id: None,
            history,
            prepared: HashMap::new(),
            jobs: HashMap::new(),
            cancels: HashMap::new(),
            handles: Vec::new(),
            emitted: Instant::now(),
        }));
        let changed: Arc<dyn Fn(DownloadsView) + Send + Sync> = Arc::new(changed);
        let callback = Arc::clone(&changed);
        let current = Arc::clone(&state);
        let available = tools.available();
        let magnet = crate::magnet::MagnetManager::open(
            history_path.with_extension("magnet.json"),
            Arc::clone(&slots),
            Arc::new(move |tasks| {
                let view = {
                    let state = current
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    download_view(&state, available, tasks)
                };
                callback(view);
            }),
        )?;
        let manager = Arc::new(Self {
            state,
            history_path,
            network: Network::default(),
            tools,
            slots,
            magnet,
            starting: tokio::sync::Mutex::new(()),
            lifetime: CancellationToken::new(),
            changed,
        });
        Ok(manager)
    }

    /// 清掉上次崩溃留下的暂存目录。返回清掉了几个。
    ///
    /// 正常结束的下载会自己删掉暂存目录；进程被杀掉的时候就留下了，里面可能是好几个 G
    /// 的分片。只在启动之后、还没有任何任务的时候做：正在下载的暂存目录也在同一个地方。
    /// 目录先过 [`assemble_device::validate_directory`]，只删名字对得上的目录，不跟着链接走。
    ///
    /// 调用方要在窗口开出来之前把它做完：那之后随时可能有任务开始，这个函数分不清
    /// "上次留下的"和"刚建的"。
    pub async fn sweep_staging(self: &Arc<Self>) -> usize {
        if !self.lock().jobs.is_empty() {
            return 0;
        }
        self.magnet.sweep_completed().await;
        let folder = self.lock().history.folder.clone();
        if assemble_device::validate_directory(&folder).await.is_err() {
            return 0;
        }
        let mut removed = 0;
        let Ok(mut entries) = tokio::fs::read_dir(&folder).await else {
            return 0;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let is_staging = entry
                .file_name()
                .to_string_lossy()
                .starts_with(STAGING_PREFIX)
                && entry.file_type().await.is_ok_and(|kind| kind.is_dir());
            if is_staging && tokio::fs::remove_dir_all(entry.path()).await.is_ok() {
                removed += 1;
            }
        }
        if removed > 0 {
            tracing::info!(
                removed,
                "staging folders left by an interrupted run were removed"
            );
        }
        removed
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    fn view_of(&self, state: &State) -> DownloadsView {
        download_view(state, self.tools.available(), self.magnet.tasks())
    }
    pub fn view(&self) -> DownloadsView {
        self.view_of(&self.lock())
    }

    pub fn set_assistant_allowed(&self, allowed: bool) {
        self.lock().assistant_allowed = allowed;
        self.emit();
    }

    pub fn ensure_assistant_allowed(&self) -> Result<()> {
        if self.lock().assistant_allowed {
            Ok(())
        } else {
            Err(DownloadError::AssistantDisabled)
        }
    }

    pub async fn prepare_assistant(&self, input: DownloadInput) -> Result<DownloadPreview> {
        self.ensure_assistant_allowed()?;
        if input.kind == DownloadKind::Magnet {
            if input.page_url.is_some() || input.title.is_some() {
                return Err(DownloadError::InvalidSource);
            }
            let preview = self
                .magnet
                .prepare(
                    &input.url,
                    Some(DownloadInitiator::Assistant),
                    input.magnet_network.unwrap_or_default(),
                )
                .await?;
            if let Err(error) = self.ensure_assistant_allowed() {
                self.magnet.discard_preview(&preview.id);
                return Err(error);
            }
            return Ok(preview);
        }
        let preview = self.prepare(input).await?;
        let mut state = self.lock();
        if !state.assistant_allowed {
            state.prepared.remove(&preview.id);
            return Err(DownloadError::AssistantDisabled);
        }
        state
            .prepared
            .get_mut(&preview.id)
            .ok_or(DownloadError::Expired)?
            .initiated_by = Some(DownloadInitiator::Assistant);
        Ok(preview)
    }

    pub fn assistant_task(&self, id: &str) -> Result<DownloadTask> {
        self.ensure_assistant_allowed()?;
        self.view()
            .tasks
            .into_iter()
            .find(|r| r.id == id && r.initiated_by == Some(DownloadInitiator::Assistant))
            .ok_or(DownloadError::Expired)
    }
    fn persist(&self, state: &State) -> Result<()> {
        write_private_json(&self.history_path, &state.history).map_err(|_| DownloadError::Storage)
    }
    fn emit(&self) {
        (self.changed)(self.view());
    }

    pub async fn set_folder(&self, folder: PathBuf) -> Result<()> {
        let folder = assemble_device::validate_directory(&folder)
            .await
            .map_err(|_| DownloadError::Storage)?;
        {
            let mut state = self.lock();
            let previous = state.history.folder.clone();
            state.history.folder = folder;
            if let Err(error) = self.persist(&state) {
                state.history.folder = previous;
                return Err(error);
            }
        }
        self.emit();
        Ok(())
    }

    pub async fn prepare(&self, input: DownloadInput) -> Result<DownloadPreview> {
        if input.kind == DownloadKind::Magnet {
            if input.page_url.is_some() || input.title.is_some() {
                return Err(DownloadError::InvalidSource);
            }
            return self
                .magnet
                .prepare(&input.url, None, input.magnet_network.unwrap_or_default())
                .await;
        }
        if input.magnet_network.is_some() {
            return Err(DownloadError::InvalidSource);
        }
        let request_headers = headers(if input.kind == DownloadKind::Douyin {
            Some("https://www.douyin.com/")
        } else {
            input.page_url.as_deref()
        })?;
        let (title, host, sources, duration) = match input.kind {
            DownloadKind::Magnet => return Err(DownloadError::InvalidSource),
            DownloadKind::Hls => {
                let url = source_url(input.url.trim())?;
                let host = url
                    .host_str()
                    .ok_or(DownloadError::InvalidSource)?
                    .to_owned();
                let resolved = hls::resolve(
                    Track { url, body: None },
                    &self.network,
                    &request_headers,
                    &self.lifetime,
                )
                .await?;
                (
                    input.title.unwrap_or_else(|| "HLS".into()),
                    host,
                    resolved
                        .variants
                        .into_iter()
                        .map(|v| (v.view.clone(), Source::Hls(Box::new(v))))
                        .collect(),
                    resolved.duration,
                )
            }
            DownloadKind::Douyin => {
                let video = douyin::resolve(&input.url, &self.network, &self.lifetime).await?;
                (
                    video.0,
                    "douyin.com".into(),
                    vec![(
                        DownloadVariant {
                            id: "v0".into(),
                            width: None,
                            height: None,
                            bandwidth: None,
                        },
                        Source::Video(video.1),
                    )],
                    None,
                )
            }
            DownloadKind::Video => {
                let url = source_url(input.url.trim())?;
                let host = url
                    .host_str()
                    .ok_or(DownloadError::InvalidSource)?
                    .to_owned();
                (
                    input.title.unwrap_or_else(|| "Video".into()),
                    host,
                    vec![(
                        DownloadVariant {
                            id: "v0".into(),
                            width: None,
                            height: None,
                            bandwidth: None,
                        },
                        Source::Video(url),
                    )],
                    None,
                )
            }
        };
        self.remember(input.kind, &title, host, sources, duration, request_headers)
    }

    /// 文件内容由壳的系统选择器读取；不接受来自界面的磁盘路径。
    pub async fn import_hls(
        &self,
        bytes: Vec<u8>,
        base: Option<String>,
        page: Option<String>,
    ) -> Result<DownloadPreview> {
        if bytes.len() as u64 > MAX_METADATA {
            return Err(DownloadError::Limit);
        }
        let base = base.filter(|s| !s.trim().is_empty());
        if base.is_none() {
            let absolute = |value: &str| source_url(value).is_ok();
            let playlist =
                m3u8_rs::parse_playlist_res(&bytes).map_err(|_| DownloadError::Unsupported)?;
            let all_absolute = match playlist {
                m3u8_rs::Playlist::MasterPlaylist(p) => {
                    p.variants.iter().all(|v| absolute(&v.uri))
                        && p.alternatives
                            .iter()
                            .all(|a| a.uri.as_deref().is_none_or(absolute))
                }
                m3u8_rs::Playlist::MediaPlaylist(p) => p.segments.iter().all(|s| {
                    absolute(&s.uri)
                        && s.key
                            .as_ref()
                            .and_then(|k| k.uri.as_deref())
                            .is_none_or(absolute)
                        && s.map.as_ref().is_none_or(|m| absolute(&m.uri))
                }),
            };
            if !all_absolute {
                return Err(DownloadError::InvalidSource);
            }
        }
        let url = source_url(base.as_deref().unwrap_or("https://import.invalid/"))?;
        let request_headers = headers(page.as_deref())?;
        let resolved = hls::resolve(
            Track {
                url,
                body: Some(bytes),
            },
            &self.network,
            &request_headers,
            &self.lifetime,
        )
        .await?;
        self.remember(
            DownloadKind::Hls,
            "HLS",
            "M3U8".into(),
            resolved
                .variants
                .into_iter()
                .map(|v| (v.view.clone(), Source::Hls(Box::new(v))))
                .collect(),
            resolved.duration,
            request_headers,
        )
    }

    fn remember(
        &self,
        kind: DownloadKind,
        title: &str,
        host: String,
        sources: Vec<(DownloadVariant, Source)>,
        duration: Option<f64>,
        headers: HeaderMap,
    ) -> Result<DownloadPreview> {
        let title: String = title
            .chars()
            .filter(|c| !c.is_control())
            .take(160)
            .collect();
        let title = if title.trim().is_empty() {
            match kind {
                DownloadKind::Hls => "HLS",
                DownloadKind::Douyin => "Douyin",
                DownloadKind::Video => "Video",
                DownloadKind::Magnet => "Magnet",
            }
            .to_owned()
        } else {
            title
        };
        let preview = DownloadPreview {
            magnet_network: None,
            id: random_id()?,
            kind,
            title,
            source_host: host,
            duration_seconds: duration,
            variants: sources.iter().map(|(v, _)| v.clone()).collect(),
            files: Vec::new(),
        };
        let mut state = self.lock();
        state
            .prepared
            .retain(|_, p| p.at.elapsed() < Duration::from_secs(600));
        if state.prepared.len() >= 16 {
            return Err(DownloadError::Busy);
        }
        state.prepared.insert(
            preview.id.clone(),
            Prepared {
                initiated_by: None,
                preview: preview.clone(),
                sources: sources.into_iter().map(|(v, s)| (v.id, s)).collect(),
                headers,
                at: Instant::now(),
            },
        );
        Ok(preview)
    }

    pub async fn start(self: &Arc<Self>, preview_id: &str, variant_id: &str) -> Result<String> {
        self.start_selected(preview_id, variant_id, &[]).await
    }

    pub async fn start_assistant(
        self: &Arc<Self>,
        preview_id: &str,
        variant_id: &str,
    ) -> Result<String> {
        self.ensure_assistant_allowed()?;
        self.start_assistant_selected(preview_id, variant_id, &[])
            .await
    }

    pub async fn start_selected(
        self: &Arc<Self>,
        preview_id: &str,
        variant_id: &str,
        file_ids: &[u32],
    ) -> Result<String> {
        self.start_selected_as(preview_id, variant_id, file_ids, None)
            .await
    }
    pub async fn start_assistant_selected(
        self: &Arc<Self>,
        preview_id: &str,
        variant_id: &str,
        file_ids: &[u32],
    ) -> Result<String> {
        self.ensure_assistant_allowed()?;
        self.start_selected_as(
            preview_id,
            variant_id,
            file_ids,
            Some(DownloadInitiator::Assistant),
        )
        .await
    }
    async fn start_selected_as(
        self: &Arc<Self>,
        preview_id: &str,
        variant_id: &str,
        file_ids: &[u32],
        by: Option<DownloadInitiator>,
    ) -> Result<String> {
        let _starting = self.starting.lock().await;
        if by.is_some() {
            self.ensure_assistant_allowed()?;
        }
        if self
            .view()
            .tasks
            .iter()
            .filter(|t| {
                t.phase.active()
                    || (t.kind == DownloadKind::Magnet
                        && matches!(
                            t.phase,
                            DownloadPhase::Paused
                                | DownloadPhase::Failed
                                | DownloadPhase::Interrupted
                        ))
            })
            .count()
            >= 20
        {
            return Err(DownloadError::Busy);
        }
        if self.magnet.owns_preview(preview_id) {
            if variant_id != "magnet" {
                return Err(DownloadError::InvalidSource);
            }
            let folder = self.lock().history.folder.clone();
            return self.magnet.start(preview_id, file_ids, folder, by).await;
        }
        if !file_ids.is_empty() {
            return Err(DownloadError::InvalidSource);
        }
        self.start_as(preview_id, variant_id, by).await
    }
    pub fn cancel_preparation(&self) {
        self.magnet.cancel_preparation();
    }
    pub async fn pause(&self, id: &str) -> Result<()> {
        self.magnet.pause(id).await
    }
    pub async fn resume(self: &Arc<Self>, id: &str) -> Result<()> {
        self.magnet.resume(id).await
    }
    pub async fn cancel_selected(&self, id: &str) -> Result<()> {
        if self.magnet.owns(id) {
            self.magnet.cancel(id).await
        } else {
            self.cancel(id)
        }
    }
    pub async fn retry_selected(self: &Arc<Self>, id: &str) -> Result<()> {
        if self.magnet.owns(id) {
            self.magnet.resume(id).await
        } else {
            self.retry(id)
        }
    }

    async fn start_as(
        self: &Arc<Self>,
        preview_id: &str,
        variant_id: &str,
        initiated_by: Option<DownloadInitiator>,
    ) -> Result<String> {
        if !self.tools.available() {
            return Err(DownloadError::ToolsUnavailable);
        }
        let folder = self.lock().history.folder.clone();
        tokio::fs::create_dir_all(&folder)
            .await
            .map_err(|_| DownloadError::Storage)?;
        let folder = assemble_device::validate_directory(&folder)
            .await
            .map_err(|_| DownloadError::Storage)?;
        // 磁盘快满了就别开这个头：说清楚是空间不够，而不是下到一半才失败。
        space::ensure_room(&folder, 0)?;
        let id = random_id()?;
        {
            let mut state = self.lock();
            if initiated_by.is_some() && !state.assistant_allowed {
                return Err(DownloadError::AssistantDisabled);
            }
            if state
                .history
                .records
                .iter()
                .filter(|r| r.view.phase.active())
                .count()
                >= 20
            {
                return Err(DownloadError::Busy);
            }
            let prepared = state
                .prepared
                .get(preview_id)
                .filter(|p| p.at.elapsed() < Duration::from_secs(600))
                .cloned()
                .ok_or(DownloadError::Expired)?;
            if prepared.initiated_by != initiated_by {
                return Err(DownloadError::Expired);
            }
            let source = prepared
                .sources
                .iter()
                .find(|(id, _)| id == variant_id)
                .map(|(_, s)| s.clone())
                .ok_or(DownloadError::InvalidSource)?;
            state.jobs.insert(
                id.clone(),
                Job {
                    source,
                    headers: prepared.headers,
                    folder,
                },
            );
            state.history.records.insert(
                0,
                Record {
                    view: DownloadTask {
                        magnet_network: None,
                        id: id.clone(),
                        initiated_by,
                        kind: prepared.preview.kind,
                        title: prepared.preview.title,
                        file_name: None,
                        source_host: prepared.preview.source_host,
                        phase: DownloadPhase::Queued,
                        received_bytes: 0,
                        total_bytes: None,
                        completed_segments: 0,
                        total_segments: None,
                        bytes_per_second: 0,
                        media_seconds: None,
                        duration_seconds: prepared.preview.duration_seconds,
                        error_code: None,
                        retryable: false,
                        created_at: Timestamp::now(),
                    },
                    path: None,
                    sha256: None,
                },
            );
            if let Err(error) = self.persist(&state) {
                state.history.records.remove(0);
                state.jobs.remove(&id);
                return Err(error);
            }
            state.prepared.remove(preview_id);
            self.spawn_locked(&mut state, &id)?;
        }
        self.emit();
        Ok(id)
    }

    fn spawn_locked(self: &Arc<Self>, state: &mut State, id: &str) -> Result<()> {
        if self.lifetime.is_cancelled() {
            return Err(DownloadError::Cancelled);
        }
        let job = state.jobs.get(id).cloned().ok_or(DownloadError::Expired)?;
        let cancel = self.lifetime.child_token();
        state.cancels.insert(id.to_owned(), cancel.clone());
        let manager = Arc::clone(self);
        let id = id.to_owned();
        state.handles.retain(|handle| !handle.is_finished());
        state.handles.push(tokio::spawn(async move {
            let result = manager.run(&id, job, &cancel).await;
            manager.finish(&id, result);
        }));
        Ok(())
    }

    pub fn cancel(&self, id: &str) -> Result<()> {
        self.lock()
            .cancels
            .get(id)
            .ok_or(DownloadError::Expired)?
            .cancel();
        Ok(())
    }

    /// 标识只用于浏览器主动发起下载后定位列表，不携带来源地址。
    pub fn focus_browser_task(&self, id: &str) {
        self.lock().browser_task_id = Some(id.to_owned());
        self.emit();
    }
    pub fn retry(self: &Arc<Self>, id: &str) -> Result<()> {
        {
            let mut state = self.lock();
            if !state.jobs.contains_key(id) {
                return Err(DownloadError::Expired);
            }
            let record = state
                .history
                .records
                .iter_mut()
                .find(|r| r.view.id == id)
                .ok_or(DownloadError::Expired)?;
            if record.view.phase.active() {
                return Err(DownloadError::Busy);
            }
            if record.view.phase == DownloadPhase::Completed {
                return Err(DownloadError::Expired);
            }
            record.view.phase = DownloadPhase::Queued;
            record.view.retryable = false;
            record.view.error_code = None;
            record.view.received_bytes = 0;
            record.view.completed_segments = 0;
            record.view.media_seconds = None;
            self.persist(&state)?;
            self.spawn_locked(&mut state, id)?;
        }
        self.emit();
        Ok(())
    }

    fn update(&self, id: &str, force: bool, change: impl FnOnce(&mut DownloadTask)) {
        let emit = {
            let mut state = self.lock();
            if let Some(record) = state.history.records.iter_mut().find(|r| r.view.id == id) {
                change(&mut record.view);
            }
            if force || state.emitted.elapsed() >= Duration::from_millis(250) {
                state.emitted = Instant::now();
                true
            } else {
                false
            }
        };
        if emit {
            self.emit();
        }
    }

    async fn run(
        &self,
        id: &str,
        job: Job,
        cancel: &CancellationToken,
    ) -> Result<(PathBuf, f64, String)> {
        let _slot = tokio::select! { ()=cancel.cancelled()=>return Err(DownloadError::Cancelled), slot=self.slots.acquire()=>slot.map_err(|_|DownloadError::Cancelled)? };
        self.update(id, true, |v| v.phase = DownloadPhase::Downloading);
        let folder = assemble_device::validate_directory(&job.folder)
            .await
            .map_err(|_| DownloadError::Storage)?;
        // 排队的时候别的任务可能已经把磁盘写满了：轮到自己时再看一眼。
        space::ensure_room(&folder, 0)?;
        let directory = tempfile::Builder::new()
            .prefix(STAGING_PREFIX)
            .tempdir_in(&folder)
            .map_err(|error| space::write_failure(&error))?;
        let started = Instant::now();
        let (video, audio, hls) = match &job.source {
            Source::Hls(variant) => {
                let (video, audio, duration) = hls::materialize(
                    variant,
                    directory.path(),
                    &self.network,
                    &job.headers,
                    cancel,
                    &|bytes, done, total| {
                        self.update(id, false, |v| {
                            v.received_bytes = bytes;
                            v.completed_segments = done;
                            v.total_segments = Some(total);
                            v.bytes_per_second = rate(bytes, started);
                        });
                    },
                )
                .await?;
                self.update(id, true, |v| v.duration_seconds = Some(duration));
                (video, audio, true)
            }
            Source::Video(url) => {
                let path = directory.path().join("source.mp4");
                self.network
                    .download(url, &job.headers, None, &path, cancel, &|delta, total| {
                        self.update(id, false, |v| {
                            v.received_bytes = v.received_bytes.saturating_add(delta);
                            v.total_bytes = total;
                            v.bytes_per_second = rate(v.received_bytes, started);
                        });
                    })
                    .await?;
                self.tools.verify(&path, cancel).await?;
                (path, None, false)
            }
        };
        self.update(id, true, |v| {
            v.phase = DownloadPhase::Muxing;
            v.bytes_per_second = 0;
        });
        let output = directory.path().join("output.mp4");
        self.tools
            .mux(&video, audio.as_deref(), hls, &output, cancel, &|seconds| {
                self.update(id, false, |v| v.media_seconds = Some(seconds));
            })
            .await?;
        self.update(id, true, |v| v.phase = DownloadPhase::Verifying);
        let duration = self.tools.verify(&output, cancel).await?;
        if hls {
            let expected = self
                .lock()
                .history
                .records
                .iter()
                .find(|r| r.view.id == id)
                .and_then(|r| r.view.duration_seconds)
                .ok_or(DownloadError::InvalidMedia)?;
            if (duration - expected).abs() > expected * 0.02 + 0.25 {
                return Err(DownloadError::InvalidMedia);
            }
        }
        let digest = seal(&output, cancel).await?;
        if cancel.is_cancelled() {
            return Err(DownloadError::Cancelled);
        }
        let title = self
            .lock()
            .history
            .records
            .iter()
            .find(|r| r.view.id == id)
            .map(|r| r.view.title.clone())
            .ok_or(DownloadError::Expired)?;
        let name = assemble_device::safe_file_name(&format!("{title}.mp4"));
        // 同卷硬链接是严格的原子不覆盖发布；不支持的文件系统明确失败，不能退回存在竞争窗口的 rename。
        for _ in 0..100 {
            let name = assemble_device::choose_file_name(&folder, &name)
                .await
                .map_err(|_| DownloadError::Storage)?;
            let path = folder.join(name);
            match tokio::fs::hard_link(&output, &path).await {
                Ok(()) => return Ok((path, duration, digest)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(_) => return Err(DownloadError::Storage),
            }
        }
        Err(DownloadError::Storage)
    }

    fn finish(&self, id: &str, result: Result<(PathBuf, f64, String)>) {
        {
            let mut state = self.lock();
            state.cancels.remove(id);
            if let Some(record) = state.history.records.iter_mut().find(|r| r.view.id == id) {
                record.view.bytes_per_second = 0;
                match result {
                    Ok((path, duration, digest)) => {
                        record.view.file_name =
                            path.file_name().map(|p| p.to_string_lossy().into_owned());
                        record.path = Some(path);
                        record.sha256 = Some(digest);
                        record.view.duration_seconds = Some(duration);
                        record.view.phase = DownloadPhase::Completed;
                        record.view.retryable = false;
                    }
                    Err(error) => {
                        record.view.phase = if error == DownloadError::Cancelled {
                            DownloadPhase::Cancelled
                        } else {
                            DownloadPhase::Failed
                        };
                        record.view.error_code = Some(error.code().into());
                        record.view.retryable = true;
                    }
                }
            }
            // 只裁掉终态；有界历史不会丢失仍在运行的任务。
            while state.history.records.len() > 100 {
                if let Some(index) = state
                    .history
                    .records
                    .iter()
                    .rposition(|r| !r.view.phase.active())
                {
                    let record = state.history.records.remove(index);
                    state.jobs.remove(&record.view.id);
                } else {
                    break;
                }
            }
            if self.persist(&state).is_err()
                && let Some(record) = state.history.records.iter_mut().find(|r| r.view.id == id)
            {
                record.view.error_code = Some(DownloadError::Storage.code().into());
            }
        }
        self.emit();
    }

    pub async fn completed_path(&self, id: &str) -> Result<PathBuf> {
        if self.magnet.owns(id) {
            return self.magnet.completed_path(id).await;
        }
        let path = self
            .lock()
            .history
            .records
            .iter()
            .find(|r| r.view.id == id && r.view.phase == DownloadPhase::Completed)
            .and_then(|r| r.path.clone())
            .ok_or(DownloadError::Expired)?;
        let metadata = tokio::fs::symlink_metadata(&path)
            .await
            .map_err(|_| DownloadError::Storage)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(DownloadError::Storage);
        }
        Ok(path)
    }

    /// 退出先取消，再等待网络请求与媒体子进程真正结束；不能只依赖析构。
    pub async fn shutdown(&self) {
        self.lifetime.cancel();
        self.magnet.shutdown().await;
        let handles = std::mem::take(&mut self.lock().handles);
        for handle in handles {
            let _ = handle.await;
        }
    }
}

fn download_view(state: &State, available: bool, magnet: Vec<DownloadTask>) -> DownloadsView {
    let mut tasks: Vec<_> = state
        .history
        .records
        .iter()
        .map(|r| r.view.clone())
        .chain(magnet)
        .collect();
    tasks.sort_by_key(|a| std::cmp::Reverse(a.created_at.as_millis()));
    DownloadsView {
        direct_bt_available: cfg!(target_os = "macos"),
        assistant_allowed: state.assistant_allowed,
        browser_task_id: state.browser_task_id.clone(),
        folder_name: state
            .history
            .folder
            .file_name()
            .map_or_default(|v| v.to_string_lossy().into_owned()),
        tools_available: available,
        tasks,
    }
}

fn rate(bytes: u64, started: Instant) -> u64 {
    bytes / started.elapsed().as_secs().max(1)
}

async fn seal(path: &Path, cancel: &CancellationToken) -> Result<String> {
    seal_with_limit(path, cancel, crate::MAX_BYTES, false).await
}
pub(super) async fn seal_with_limit(
    path: &Path,
    cancel: &CancellationToken,
    limit: u64,
    allow_empty: bool,
) -> Result<String> {
    let mut file = tokio::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .await
        .map_err(|_| DownloadError::Storage)?;
    let mut hash = Sha256::new();
    let mut bytes = vec![0; 65_536];
    let mut count = 0_u64;
    loop {
        let length = tokio::select! {()=cancel.cancelled()=>return Err(DownloadError::Cancelled),value=file.read(&mut bytes)=>value.map_err(|_|DownloadError::Storage)?};
        if length == 0 {
            break;
        }
        hash.update(&bytes[..length]);
        count += length as u64;
        if count > limit {
            return Err(DownloadError::Limit);
        }
    }
    if count == 0 && !allow_empty {
        return Err(DownloadError::InvalidMedia);
    }
    let digest = assemble_protocol::primitives::to_hex(&hash.finalize());
    file.sync_all().await.map_err(|_| DownloadError::Storage)?;
    Ok(digest)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn history_interrupts_without_exposing_source_or_paths() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("history.json");
        let manager = DownloadManager::open(
            file.clone(),
            dir.path().to_owned(),
            MediaTools::discover(),
            |_| {},
        )
        .unwrap();
        assert!(
            manager
                .prepare(DownloadInput {
                    magnet_network: None,
                    kind: DownloadKind::Hls,
                    url: "file:///secret".into(),
                    page_url: None,
                    title: None
                })
                .await
                .is_err()
        );
        assert!(
            manager
                .import_hls(
                    b"#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\nrelative.ts\n#EXT-X-ENDLIST\n"
                        .to_vec(),
                    None,
                    None
                )
                .await
                .is_err()
        );
        let text = serde_json::to_string(&manager.view()).unwrap();
        assert!(!text.contains("history.json"));
        assert!(!text.contains("file://"));
        {
            let mut state = manager.lock();
            state.history.records.push(Record {
                view: DownloadTask {
                    magnet_network: None,
                    id: "running".into(),
                    initiated_by: None,
                    kind: DownloadKind::Hls,
                    title: "历史任务".into(),
                    file_name: None,
                    source_host: "example.test".into(),
                    phase: DownloadPhase::Downloading,
                    received_bytes: 50,
                    total_bytes: None,
                    completed_segments: 1,
                    total_segments: Some(3),
                    bytes_per_second: 10,
                    media_seconds: None,
                    duration_seconds: None,
                    error_code: None,
                    retryable: true,
                    created_at: Timestamp::now(),
                },
                path: None,
                sha256: None,
            });
            manager.persist(&state).unwrap();
        }
        manager.shutdown().await;
        assert!(file.exists());
        let restored =
            DownloadManager::open(file, dir.path().to_owned(), MediaTools::discover(), |_| {})
                .unwrap();
        let task = restored.view().tasks.remove(0);
        assert_eq!(task.phase, DownloadPhase::Interrupted);
        assert!(!task.retryable);
        assert_eq!(task.received_bytes, 50);
        assert_eq!(task.bytes_per_second, 0);
        assert_eq!(restored.retry("running"), Err(DownloadError::Expired));
        restored.shutdown().await;
    }
}
