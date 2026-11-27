//! 磁力任务与视频共用队列外观，但独立保存可恢复分片，避免视频清理器误删。
mod credentials;
mod engine;
mod files;
#[cfg(test)]
mod tests;
mod tracker_pool;
mod trackers;

use crate::{DownloadError, Result, random_id};
use assemble_protocol::downloads::{
    DownloadFile, DownloadInitiator, DownloadKind, DownloadPhase, DownloadPreview, DownloadTask,
    DownloadVariant,
};
use assemble_protocol::primitives::Timestamp;
use assemble_store::{
    json::{read_json, write_private_json},
    secret::{SecretKey, SecretStore, SecretString},
};
use librqbit::{AddTorrent, AddTorrentOptions, TorrentStatsState};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};
use tokio::{
    sync::{Semaphore, watch},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Serialize, Deserialize)]
struct Record {
    view: DownloadTask,
    folder: PathBuf,
    metadata: Vec<u8>,
    selected: Vec<DownloadFile>,
    publication: Option<files::Publication>,
}
struct Preview {
    engine: engine::Prepared,
    view: DownloadPreview,
    by: Option<DownloadInitiator>,
    at: Instant,
}
#[derive(Clone, Copy)]
enum Stop {
    Pause,
    Cancel,
}
struct Running {
    cancel: CancellationToken,
    done: watch::Receiver<bool>,
    intent: Stop,
}
struct State {
    records: Vec<Record>,
    previews: HashMap<String, Preview>,
    running: HashMap<String, Running>,
    handles: Vec<JoinHandle<()>>,
    preparing: Option<CancellationToken>,
}

struct Preparation<'a>(&'a MagnetManager);
impl Drop for Preparation<'_> {
    fn drop(&mut self) {
        self.0.lock().preparing = None;
        self.0.prepared_changed.notify_waiters();
    }
}

pub(crate) struct MagnetManager {
    state: Mutex<State>,
    history: PathBuf,
    slots: Arc<Semaphore>,
    lifetime: CancellationToken,
    changed: Arc<dyn Fn(Vec<DownloadTask>) + Send + Sync>,
    store: Option<Arc<dyn SecretStore>>,
    options: engine::Options,
    operations: tokio::sync::Mutex<()>,
    prepared_changed: tokio::sync::Notify,
}

fn system_store() -> Option<Arc<dyn SecretStore>> {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        assemble_store::secret::system_secret_store()
            .map(|store| Arc::new(store) as Arc<dyn SecretStore>)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        None
    }
}
fn key(id: &str) -> SecretKey {
    SecretKey::new("assemble.download.magnet.v1", id)
}

impl MagnetManager {
    pub(crate) fn open(
        history: PathBuf,
        slots: Arc<Semaphore>,
        changed: Arc<dyn Fn(Vec<DownloadTask>) + Send + Sync>,
    ) -> Result<Arc<Self>> {
        let pool = tracker_pool::Pool::open(history.with_extension("trackers.json"));
        Self::open_with(
            history,
            slots,
            changed,
            system_store(),
            engine::Options {
                pool: Some(pool),
                ..engine::Options::default()
            },
        )
    }
    fn open_with(
        history: PathBuf,
        slots: Arc<Semaphore>,
        changed: Arc<dyn Fn(Vec<DownloadTask>) + Send + Sync>,
        store: Option<Arc<dyn SecretStore>>,
        options: engine::Options,
    ) -> Result<Arc<Self>> {
        let mut records = read_json::<Vec<Record>>(&history)
            .map_err(|_| DownloadError::Storage)?
            .unwrap_or_default();
        if records.len() > 100 {
            return Err(DownloadError::Storage);
        }
        for record in &mut records {
            if record.view.kind != DownloadKind::Magnet
                || record.view.id.len() != 32
                || !record.view.id.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err(DownloadError::Storage);
            }
            if record.view.phase.active() {
                record.view.phase = DownloadPhase::Paused;
            }
            record.view.bytes_per_second = 0;
            record.view.retryable = record.view.phase == DownloadPhase::Failed;
            let (_, all, _) = engine::describe(&record.metadata)?;
            let ids: Vec<_> = record.selected.iter().map(|f| f.id).collect();
            let (selected, size) = select_files(&all, &ids).map_err(|_| DownloadError::Storage)?;
            if selected != record.selected {
                return Err(DownloadError::Storage);
            }
            record.view.total_bytes = Some(size);
            record.view.received_bytes = record.view.received_bytes.min(size);
        }
        write_private_json(&history, &records).map_err(|_| DownloadError::Storage)?;
        Ok(Arc::new(Self {
            state: Mutex::new(State {
                records,
                previews: HashMap::new(),
                running: HashMap::new(),
                handles: Vec::new(),
                preparing: None,
            }),
            history,
            slots,
            lifetime: CancellationToken::new(),
            changed,
            store,
            options,
            operations: tokio::sync::Mutex::new(()),
            prepared_changed: tokio::sync::Notify::new(),
        }))
    }
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    fn persist(&self, state: &State) -> Result<()> {
        write_private_json(&self.history, &state.records).map_err(|_| DownloadError::Storage)
    }
    fn emit(&self) {
        (self.changed)(self.tasks());
    }
    pub(crate) fn tasks(&self) -> Vec<DownloadTask> {
        self.lock().records.iter().map(|r| r.view.clone()).collect()
    }
    pub(crate) fn owns(&self, id: &str) -> bool {
        self.lock().records.iter().any(|r| r.view.id == id)
    }
    pub(crate) fn owns_preview(&self, id: &str) -> bool {
        self.lock().previews.contains_key(id)
    }
    pub(crate) fn discard_preview(&self, id: &str) {
        self.lock().previews.remove(id);
    }
    pub(crate) fn cancel_preparation(&self) {
        if let Some(cancel) = self.lock().preparing.as_ref() {
            cancel.cancel();
        }
    }
    pub(crate) async fn prepare(
        &self,
        value: &str,
        by: Option<DownloadInitiator>,
    ) -> Result<DownloadPreview> {
        if self.lifetime.is_cancelled() {
            return Err(DownloadError::Cancelled);
        }
        let cancel = self.lifetime.child_token();
        {
            let mut state = self.lock();
            if state.preparing.is_some() {
                return Err(DownloadError::Busy);
            }
            state.preparing = Some(cancel.clone());
        }
        let _preparation = Preparation(self);
        let result = engine::prepare(value, &cancel, &self.options).await;
        let prepared = result?;
        let view = DownloadPreview {
            id: random_id()?,
            kind: DownloadKind::Magnet,
            title: prepared.title.clone(),
            source_host: "BitTorrent".into(),
            duration_seconds: None,
            variants: vec![DownloadVariant {
                id: "magnet".into(),
                width: None,
                height: None,
                bandwidth: None,
            }],
            files: prepared.files.clone(),
        };
        let mut state = self.lock();
        state
            .previews
            .retain(|_, p| p.at.elapsed() < Duration::from_secs(600));
        if state.previews.len() >= 16 {
            return Err(DownloadError::Busy);
        }
        state.previews.insert(
            view.id.clone(),
            Preview {
                engine: prepared,
                view: view.clone(),
                by,
                at: Instant::now(),
            },
        );
        Ok(view)
    }
    pub(crate) async fn start(
        self: &Arc<Self>,
        preview_id: &str,
        ids: &[u32],
        folder: PathBuf,
        by: Option<DownloadInitiator>,
    ) -> Result<String> {
        let _operation = self.operations.lock().await;
        if self.lifetime.is_cancelled() {
            return Err(DownloadError::Cancelled);
        }
        let (engine, view) = {
            let state = self.lock();
            let preview = state
                .previews
                .get(preview_id)
                .filter(|p| p.by == by && p.at.elapsed() < Duration::from_secs(600))
                .ok_or(DownloadError::Expired)?;
            (preview.engine.clone(), preview.view.clone())
        };
        let (selected, size) = select_files(&engine.files, ids)?;
        if self
            .lock()
            .records
            .iter()
            .filter(|r| {
                !matches!(
                    r.view.phase,
                    DownloadPhase::Completed | DownloadPhase::Cancelled
                )
            })
            .count()
            >= 20
        {
            return Err(DownloadError::Busy);
        }
        tokio::fs::create_dir_all(&folder)
            .await
            .map_err(|_| DownloadError::Storage)?;
        let folder = files::directory(&folder).await?;
        crate::space::ensure_room(&folder, size.saturating_mul(2))?;
        let id = random_id()?;
        let store = self
            .store
            .as_ref()
            .ok_or(DownloadError::CredentialsUnavailable)?;
        self.reserve_record().await?;
        credentials::put(store.as_ref(), &id, &engine.source)?;
        let record = Record {
            view: DownloadTask {
                id: id.clone(),
                initiated_by: by,
                kind: DownloadKind::Magnet,
                title: view.title,
                file_name: None,
                source_host: "BitTorrent".into(),
                phase: DownloadPhase::Queued,
                received_bytes: 0,
                total_bytes: Some(size),
                completed_segments: 0,
                total_segments: None,
                bytes_per_second: 0,
                media_seconds: None,
                duration_seconds: None,
                error_code: None,
                retryable: false,
                created_at: Timestamp::now(),
            },
            folder,
            metadata: engine.metadata,
            selected,
            publication: None,
        };
        {
            let mut state = self.lock();
            state.records.insert(0, record);
            if let Err(error) = self.persist(&state) {
                state.records.remove(0);
                let _ = credentials::delete(store.as_ref(), &id);
                return Err(error);
            }
            state.previews.remove(preview_id);
        }
        if let Err(error) = self.spawn(&id, engine.source, engine.peers) {
            self.finish(&id, Err(error));
            return Err(error);
        }
        self.emit();
        Ok(id)
    }
    fn spawn(
        self: &Arc<Self>,
        id: &str,
        source: SecretString,
        peers: Vec<std::net::SocketAddr>,
    ) -> Result<()> {
        let cancel = self.lifetime.child_token();
        if cancel.is_cancelled() {
            return Err(DownloadError::Cancelled);
        }
        let (done, sent) = watch::channel(false);
        let mut state = self.lock();
        if state.running.contains_key(id) {
            return Err(DownloadError::Busy);
        }
        state.running.insert(
            id.to_owned(),
            Running {
                cancel: cancel.clone(),
                done: sent,
                intent: Stop::Pause,
            },
        );
        let manager = Arc::clone(self);
        let id = id.to_owned();
        state.handles.retain(|h| !h.is_finished());
        state.handles.push(tokio::spawn(async move {
            let result = manager.run(&id, source, peers, &cancel).await;
            let completed = result.is_ok();
            manager.finish(&id, result);
            if completed
                && manager
                    .record(&id)
                    .is_ok_and(|r| r.view.phase == DownloadPhase::Completed)
            {
                let _ = manager.cleanup(&id).await;
            }
            let _ = done.send(true);
        }));
        Ok(())
    }
    pub(crate) async fn pause(&self, id: &str) -> Result<()> {
        let _operation = self.operations.lock().await;
        self.stop(id, Stop::Pause).await
    }
    async fn stop(&self, id: &str, intent: Stop) -> Result<()> {
        let mut done = {
            let mut state = self.lock();
            let record = state
                .records
                .iter()
                .find(|r| r.view.id == id)
                .ok_or(DownloadError::Expired)?;
            if matches!(
                record.view.phase,
                DownloadPhase::Completed | DownloadPhase::Cancelled
            ) {
                return Err(DownloadError::Expired);
            }
            if let Some(running) = state.running.get_mut(id) {
                running.intent = intent;
                running.cancel.cancel();
                Some(running.done.clone())
            } else {
                None
            }
        };
        if let Some(done) = done.as_mut() {
            let _ = done.wait_for(|value| *value).await;
        }
        if matches!(intent, Stop::Cancel) {
            let record = self
                .lock()
                .records
                .iter()
                .find(|r| r.view.id == id)
                .cloned()
                .ok_or(DownloadError::Expired)?;
            files::remove_cache(&record.folder, id).await?;
            if let Some(store) = &self.store {
                credentials::delete(store.as_ref(), id)?;
            }
            let mut state = self.lock();
            let record = state
                .records
                .iter_mut()
                .find(|r| r.view.id == id)
                .ok_or(DownloadError::Expired)?;
            if record.view.phase != DownloadPhase::Completed {
                record.view.phase = DownloadPhase::Cancelled;
                record.view.retryable = false;
                record.view.error_code = None;
            }
            self.persist(&state)?;
        }
        self.emit();
        Ok(())
    }
    pub(crate) async fn cancel(&self, id: &str) -> Result<()> {
        let _operation = self.operations.lock().await;
        self.stop(id, Stop::Cancel).await
    }
    pub(crate) async fn resume(self: &Arc<Self>, id: &str) -> Result<()> {
        let _operation = self.operations.lock().await;
        if self.lifetime.is_cancelled() {
            return Err(DownloadError::Cancelled);
        }
        {
            let state = self.lock();
            let record = state
                .records
                .iter()
                .find(|r| r.view.id == id)
                .ok_or(DownloadError::Expired)?;
            if !matches!(
                record.view.phase,
                DownloadPhase::Paused | DownloadPhase::Failed | DownloadPhase::Interrupted
            ) || state.running.contains_key(id)
            {
                return Err(DownloadError::Busy);
            }
        }
        let source = credentials::get(
            self.store
                .as_ref()
                .ok_or(DownloadError::CredentialsUnavailable)?
                .as_ref(),
            id,
        )?;
        {
            let mut state = self.lock();
            let record = state
                .records
                .iter_mut()
                .find(|r| r.view.id == id)
                .ok_or(DownloadError::Expired)?;
            record.view.phase = DownloadPhase::Queued;
            record.view.error_code = None;
            record.view.retryable = false;
            self.persist(&state)?;
        }
        if let Err(error) = self.spawn(id, source, Vec::new()) {
            self.finish(id, Err(error));
            return Err(error);
        }
        self.emit();
        Ok(())
    }
    fn record(&self, id: &str) -> Result<Record> {
        self.lock()
            .records
            .iter()
            .find(|r| r.view.id == id)
            .cloned()
            .ok_or(DownloadError::Expired)
    }
    fn update(&self, id: &str, phase: DownloadPhase, bytes: u64, speed: u64) {
        {
            let mut state = self.lock();
            if let Some(record) = state.records.iter_mut().find(|r| r.view.id == id) {
                record.view.phase = phase;
                record.view.received_bytes = bytes;
                record.view.bytes_per_second = speed;
            }
        }
        self.emit();
    }
    async fn run(
        &self,
        id: &str,
        source: SecretString,
        mut peers: Vec<std::net::SocketAddr>,
        cancel: &CancellationToken,
    ) -> Result<PathBuf> {
        let _slot = tokio::select! {()=cancel.cancelled()=>return Err(DownloadError::Cancelled),slot=self.slots.acquire()=>slot.map_err(|_|DownloadError::Cancelled)?};
        let record = self.record(id)?;
        engine::check_identity(&source, &record.metadata)?;
        let cache = files::cache(&record.folder, id, true).await?;
        let data = cache.join("data");
        tokio::fs::create_dir_all(&data)
            .await
            .map_err(|_| DownloadError::Storage)?;
        crate::space::ensure_room(
            &record.folder,
            record
                .view
                .total_bytes
                .unwrap_or(0)
                .saturating_sub(record.view.received_bytes)
                .saturating_add(record.view.total_bytes.unwrap_or(0)),
        )?;
        let (trackers, initial) =
            engine::connection_options(&source, &self.options, cancel).await?;
        peers.extend(initial);
        peers.truncate(128);
        let engine = engine::session(data.clone(), cancel.child_token(), &self.options).await?;
        let outcome=async {
            self.update(id,DownloadPhase::Verifying,record.view.received_bytes,0);
            let added=tokio::select! {()=cancel.cancelled()=>return Err(DownloadError::Cancelled),result=engine.add_torrent(AddTorrent::from_bytes(record.metadata.clone()),Some(AddTorrentOptions {output_folder:Some(data.to_str().ok_or(DownloadError::Storage)?.to_owned()),only_files:Some(record.selected.iter().map(|f|usize::try_from(f.id).map_err(|_|DownloadError::InvalidSource)).collect::<Result<Vec<_>>>()?),overwrite:true,initial_peers:Some(peers),trackers:Some(trackers),..AddTorrentOptions::default()}))=>result.map_err(|_|DownloadError::Network)?};
            let handle=added.into_handle().ok_or(DownloadError::Network)?;
            let mut last_bytes=0;let mut previous=Instant::now();let mut last_persist=Instant::now();
            loop {
                if cancel.is_cancelled() {return Err(DownloadError::Cancelled);}
                let stats=handle.stats();
                if stats.error.is_some() || matches!(stats.state,TorrentStatsState::Error) {return Err(DownloadError::Network);}
                let phase=if matches!(stats.state,TorrentStatsState::Initializing {..}) {DownloadPhase::Verifying}else{DownloadPhase::Downloading};
                let bytes: u64=record.selected.iter().map(|f|stats.file_progress.get(usize::try_from(f.id).unwrap_or(usize::MAX)).copied().unwrap_or(0).min(f.size)).sum();
                let millis=previous.elapsed().as_millis().max(1);
                let speed=u64::try_from(u128::from(bytes.saturating_sub(last_bytes))*1000/millis).unwrap_or(u64::MAX);
                self.update(id,phase,bytes,speed);last_bytes=bytes;previous=Instant::now();
                if stats.finished {break;}
                if last_persist.elapsed()>=Duration::from_secs(2) {self.persist(&self.lock())?;last_persist=Instant::now();}
                if last_persist.elapsed()>=Duration::from_secs(1) {crate::space::ensure_room(&record.folder,0)?;}
                tokio::select! {()=cancel.cancelled()=>return Err(DownloadError::Cancelled),()=tokio::time::sleep(Duration::from_millis(250))=>{}}
            }
            Ok(())
        }.await;
        // 先停止所有节点连接，再发布文件或回执暂停，完成后不进入做种。
        engine.stop().await;
        outcome?;
        if cancel.is_cancelled() {
            return Err(DownloadError::Cancelled);
        }
        self.update(
            id,
            DownloadPhase::Verifying,
            record.view.total_bytes.unwrap_or(0),
            0,
        );
        let publication = if let Some(value) = record.publication {
            value
        } else {
            let publication = files::publication(
                &record.folder,
                &record.view.title,
                &data,
                &record.selected,
                cancel,
            )
            .await?;
            {
                let mut state = self.lock();
                state
                    .records
                    .iter_mut()
                    .find(|r| r.view.id == id)
                    .ok_or(DownloadError::Expired)?
                    .publication = Some(publication.clone());
                self.persist(&state)?;
            }
            publication
        };
        files::publish(
            &publication,
            &record.folder,
            &cache,
            &record.selected,
            cancel,
        )
        .await?;
        Ok(publication.root)
    }
    fn finish(&self, id: &str, result: Result<PathBuf>) {
        {
            let mut state = self.lock();
            let intent = state.running.remove(id).map_or(Stop::Pause, |r| r.intent);
            if let Some(record) = state.records.iter_mut().find(|r| r.view.id == id) {
                record.view.bytes_per_second = 0;
                match result {
                    Ok(path) => {
                        record.view.file_name =
                            path.file_name().map(|v| v.to_string_lossy().into_owned());
                        record.view.phase = DownloadPhase::Completed;
                        record.view.received_bytes = record.view.total_bytes.unwrap_or(0);
                        record.view.error_code = None;
                        record.view.retryable = false;
                    }
                    Err(DownloadError::Cancelled) => {
                        record.view.phase = match intent {
                            Stop::Pause => DownloadPhase::Paused,
                            Stop::Cancel => DownloadPhase::Cancelled,
                        };
                        record.view.error_code = None;
                        record.view.retryable = false;
                    }
                    Err(error) => {
                        record.view.phase = DownloadPhase::Failed;
                        record.view.error_code = Some(error.code().into());
                        record.view.retryable = true;
                    }
                }
            }
            while state.records.len() > 100 {
                if let Some(index) = state.records.iter().rposition(|r| {
                    matches!(
                        r.view.phase,
                        DownloadPhase::Completed | DownloadPhase::Cancelled
                    )
                }) {
                    let removed = state.records.remove(index);
                    if let Some(store) = &self.store {
                        let _ = credentials::delete(store.as_ref(), &removed.view.id);
                    }
                } else {
                    break;
                }
            }
            if self.persist(&state).is_err()
                && let Some(record) = state.records.iter_mut().find(|r| r.view.id == id)
            {
                record.view.phase = DownloadPhase::Failed;
                record.view.error_code = Some(DownloadError::Storage.code().into());
                record.view.retryable = true;
            }
        }
        self.emit();
    }
    pub(crate) async fn completed_path(&self, id: &str) -> Result<PathBuf> {
        let record = self.record(id)?;
        if record.view.phase != DownloadPhase::Completed {
            return Err(DownloadError::Expired);
        }
        let publication = record.publication.ok_or(DownloadError::Storage)?;
        if publication.root.parent() != Some(record.folder.as_path()) {
            return Err(DownloadError::Storage);
        }
        files::directory(&publication.root).await
    }
    async fn reserve_record(&self) -> Result<()> {
        let oldest = {
            let state = self.lock();
            if state.records.len() < 100 {
                return Ok(());
            }
            state
                .records
                .iter()
                .rev()
                .find(|r| {
                    matches!(
                        r.view.phase,
                        DownloadPhase::Completed | DownloadPhase::Cancelled
                    )
                })
                .map(|r| r.view.id.clone())
                .ok_or(DownloadError::Busy)?
        };
        self.cleanup(&oldest).await?;
        let mut state = self.lock();
        state.records.retain(|r| r.view.id != oldest);
        self.persist(&state)
    }
    async fn cleanup(&self, id: &str) -> Result<()> {
        let record = self.record(id)?;
        files::remove_cache(&record.folder, id).await?;
        if let Some(store) = &self.store {
            credentials::delete(store.as_ref(), id)?;
        }
        Ok(())
    }
    pub(crate) async fn sweep_completed(&self) {
        for task in self.tasks() {
            if matches!(
                task.phase,
                DownloadPhase::Completed | DownloadPhase::Cancelled
            ) {
                let _ = self.cleanup(&task.id).await;
            }
        }
    }
    pub(crate) async fn shutdown(&self) {
        self.lifetime.cancel();
        self.cancel_preparation();
        if let Some(pool) = &self.options.pool {
            pool.shutdown().await;
        }
        let _operations = self.operations.lock().await;
        loop {
            let changed = self.prepared_changed.notified();
            if self.lock().preparing.is_none() {
                break;
            }
            changed.await;
        }
        let handles = std::mem::take(&mut self.lock().handles);
        for handle in handles {
            let _ = handle.await;
        }
    }
}

fn select_files(all: &[DownloadFile], ids: &[u32]) -> Result<(Vec<DownloadFile>, u64)> {
    let mut ids = ids.to_vec();
    ids.sort_unstable();
    if ids.is_empty() || ids.len() > engine::MAX_FILES || ids.windows(2).any(|v| v[0] == v[1]) {
        return Err(DownloadError::InvalidSource);
    }
    let selected = ids
        .iter()
        .map(|id| {
            all.iter()
                .find(|f| f.id == *id)
                .cloned()
                .ok_or(DownloadError::InvalidSource)
        })
        .collect::<Result<Vec<_>>>()?;
    let size = selected.iter().try_fold(0_u64, |n, f| {
        n.checked_add(f.size).ok_or(DownloadError::Limit)
    })?;
    if size > engine::MAX_SELECTED {
        return Err(DownloadError::Limit);
    }
    Ok((selected, size))
}
