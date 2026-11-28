//! 服务端账户目录与 BT 核心的受控适配；公开方法不接受任意文件路径。
use super::{MagnetManager, Record, credentials, engine, files, select_files, tracker_pool};
use crate::{DownloadError, Result};
use assemble_protocol::magnets::{self, Manifest};
use assemble_protocol::{
    downloads::{DownloadKind, DownloadPhase, DownloadTask},
    primitives::Timestamp,
};
use assemble_store::secret::MemorySecretStore;
use assemble_store::{json::read_json, secret::SecretString};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

pub struct Torrent {
    pub metadata: Vec<u8>,
    pub files: Vec<assemble_protocol::downloads::DownloadFile>,
    pub title: String,
    pub info_hash: String,
}
pub struct Service {
    options: engine::Options,
    slots: Arc<Semaphore>,
    resolving: Semaphore,
}
impl Service {
    pub fn info_hash(source: &str) -> Result<String> {
        let source = engine::source(source)?;
        librqbit::Magnet::parse(source.expose())
            .map_err(|_| DownloadError::InvalidSource)?
            .as_id20()
            .map(|hash| hash.as_string())
            .ok_or(DownloadError::Unsupported)
    }
    /// 缓存也重新核验哈希、私有标记与路径，不能让数据库记录绕过下载边界。
    pub fn validate_metadata(source: &str, metadata: &[u8]) -> Result<Vec<u8>> {
        let source = engine::source(source)?;
        engine::check_identity(&source, metadata)?;
        engine::describe(metadata).map(|(metadata, _, _)| metadata)
    }
    pub async fn open(root: PathBuf) -> Result<Arc<Self>> {
        tokio::fs::create_dir_all(&root)
            .await
            .map_err(|_| DownloadError::Storage)?;
        Ok(Arc::new(Self {
            options: engine::Options {
                discovery: Some(crate::hosted::Discovery::open(&root).await?),
                pool: Some(tracker_pool::Pool::open(root.join("trackers.json"))),
                pool_shared: true,
                ..engine::Options::default()
            },
            slots: Arc::new(Semaphore::new(4)),
            resolving: Semaphore::new(2),
        }))
    }
    fn options(&self) -> engine::Options {
        self.options.clone()
    }
    /// 只供隔离的真实协议测试；生产构建没有此入口，界面和网络配置也不能启用。
    #[cfg(any(test, feature = "test-fixtures"))]
    pub fn for_test(peers: Vec<std::net::SocketAddr>) -> Arc<Self> {
        Arc::new(Self {
            options: engine::Options {
                local_test: true,
                test_peers: peers,
                ..engine::Options::default()
            },
            slots: Arc::new(Semaphore::new(4)),
            resolving: Semaphore::new(2),
        })
    }
    /// 只把种子用于元数据握手；下载必须使用生产发现链路传下来的节点。
    #[cfg(any(test, feature = "test-fixtures"))]
    pub async fn for_test_discovery(
        root: &std::path::Path,
        peers: Vec<std::net::SocketAddr>,
    ) -> Result<Arc<Self>> {
        Ok(Arc::new(Self {
            options: engine::Options {
                discovery: Some(crate::hosted::Discovery::for_test(root, peers).await?),
                local_test: true,
                ..engine::Options::default()
            },
            slots: Arc::new(Semaphore::new(4)),
            resolving: Semaphore::new(2),
        }))
    }
    #[cfg(any(test, feature = "test-fixtures"))]
    pub fn discovered_for_test(&self, hash: [u8; 20], address: std::net::SocketAddr) {
        self.options
            .discovery
            .as_ref()
            .expect("discovery fixture")
            .discovered_for_test(hash, address);
    }
    pub async fn resolve(&self, source: &str, cancel: &CancellationToken) -> Result<Torrent> {
        let _slot = tokio::select! {()=cancel.cancelled()=>return Err(DownloadError::Cancelled),slot=self.resolving.acquire()=>slot.map_err(|_|DownloadError::Cancelled)?};
        let prepared = engine::prepare(source, cancel, &self.options()).await?;
        let info_hash = librqbit::torrent_from_bytes(&prepared.metadata)
            .map_err(|_| DownloadError::InvalidMedia)?
            .info_hash
            .as_string();
        Ok(Torrent {
            metadata: prepared.metadata,
            files: prepared.files,
            title: prepared.title,
            info_hash,
        })
    }
    pub async fn shutdown(&self) {
        if let Some(pool) = &self.options.pool {
            pool.shutdown().await;
        }
        if let Some(discovery) = &self.options.discovery {
            discovery.shutdown().await;
        }
    }
}

pub struct Account {
    manager: Arc<MagnetManager>,
    root: PathBuf,
    manifests: tokio::sync::Mutex<HashMap<String, Manifest>>,
}
impl Account {
    /// 未加载的账户没有运行中会话；取消与过期回收不能依赖外网初始化成功。
    pub async fn purge_saved(root: &std::path::Path, id: &str) -> Result<()> {
        if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(DownloadError::InvalidSource);
        }
        let history = root.join("history.json");
        let saved = read_json::<Vec<Record>>(&history).map_err(|_| DownloadError::Storage)?;
        let existed = saved.is_some();
        let mut records = saved.unwrap_or_default();
        if let Some(record) = records.iter().find(|r| r.view.id == id)
            && record.folder != root.join("jobs").join(id).join("files")
        {
            return Err(DownloadError::Storage);
        }
        let folder = root.join("jobs").join(id);
        if tokio::fs::try_exists(&folder)
            .await
            .map_err(|_| DownloadError::Storage)?
        {
            files::reject_links(&folder).await?;
            tokio::fs::remove_dir_all(folder)
                .await
                .map_err(|_| DownloadError::Storage)?;
        }
        records.retain(|r| r.view.id != id);
        if existed {
            assemble_store::json::write_private_json(&history, &records)
                .map_err(|_| DownloadError::Storage)?;
        }
        Ok(())
    }
    /// 只读重启后的检查点，不为一次状态查询启动 DHT 或 BT 会话。
    pub fn saved_task(root: &std::path::Path, id: &str) -> Result<Option<DownloadTask>> {
        let records = read_json::<Vec<Record>>(&root.join("history.json"))
            .map_err(|_| DownloadError::Storage)?
            .unwrap_or_default();
        let Some(mut record) = records.into_iter().find(|r| r.view.id == id) else {
            return Ok(None);
        };
        if record.folder != root.join("jobs").join(id).join("files") {
            return Err(DownloadError::Storage);
        }
        if record.view.phase.active() {
            record.view.phase = DownloadPhase::Paused;
        }
        record.view.bytes_per_second = 0;
        Ok(Some(record.view))
    }
    pub fn open(root: PathBuf, service: &Service) -> Result<Arc<Self>> {
        std::fs::create_dir_all(&root).map_err(|_| DownloadError::Storage)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
                .map_err(|_| DownloadError::Storage)?;
        }
        let store = Arc::new(MemorySecretStore::default());
        let manager = MagnetManager::open_with(
            root.join("history.json"),
            Arc::clone(&service.slots),
            Arc::new(|_| {}),
            Some(store),
            service.options(),
        )?;
        for record in &manager.lock().records {
            if record.folder != root.join("jobs").join(&record.view.id).join("files") {
                return Err(DownloadError::Storage);
            }
            let hash = librqbit::torrent_from_bytes(&record.metadata)
                .map_err(|_| DownloadError::Storage)?
                .info_hash
                .as_string();
            credentials::put(
                manager
                    .store
                    .as_ref()
                    .ok_or(DownloadError::Storage)?
                    .as_ref(),
                &record.view.id,
                &SecretString::new(format!("magnet:?xt=urn:btih:{hash}")),
            )?;
        }
        Ok(Arc::new(Self {
            manager,
            root,
            manifests: tokio::sync::Mutex::new(HashMap::new()),
        }))
    }
    pub fn inspect(metadata: &[u8], ids: &[u32]) -> Result<(String, u64)> {
        let (metadata, files, _) = engine::describe(metadata)?;
        let (_, size) = select_files(&files, ids)?;
        let hash = librqbit::torrent_from_bytes(&metadata)
            .map_err(|_| DownloadError::InvalidMedia)?
            .info_hash
            .as_string();
        Ok((hash, size))
    }
    /// 先持久为暂停；只有用户明确继续并取得租约后才开始网络。
    pub async fn install(&self, id: &str, metadata: &[u8], ids: &[u32]) -> Result<()> {
        if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(DownloadError::InvalidSource);
        }
        let (metadata, all, title) = engine::describe(metadata)?;
        let (selected, size) = select_files(&all, ids)?;
        if let Ok(existing) = self.manager.record(id) {
            if existing.metadata != metadata || existing.selected != selected {
                return Err(DownloadError::InvalidSource);
            }
            return Ok(());
        }
        let folder = self.root.join("jobs").join(id).join("files");
        tokio::fs::create_dir_all(&folder)
            .await
            .map_err(|_| DownloadError::Storage)?;
        files::directory(&folder).await?;
        let hash = librqbit::torrent_from_bytes(&metadata)
            .map_err(|_| DownloadError::InvalidMedia)?
            .info_hash
            .as_string();
        credentials::put(
            self.manager
                .store
                .as_ref()
                .ok_or(DownloadError::Storage)?
                .as_ref(),
            id,
            &SecretString::new(format!("magnet:?xt=urn:btih:{hash}")),
        )?;
        let record = Record {
            remote: None,
            view: DownloadTask {
                magnet_network: None,
                id: id.into(),
                initiated_by: None,
                kind: DownloadKind::Magnet,
                title,
                file_name: None,
                source_host: "BitTorrent".into(),
                phase: DownloadPhase::Paused,
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
            metadata,
            selected,
            publication: None,
        };
        let mut state = self.manager.lock();
        state.records.insert(0, record);
        self.manager.persist(&state)
    }
    pub fn task(&self, id: &str) -> Result<DownloadTask> {
        Ok(self.manager.record(id)?.view)
    }
    pub async fn resume(&self, id: &str) -> Result<()> {
        let task = self.task(id)?;
        if task.phase.active() || task.phase == DownloadPhase::Completed {
            return Ok(());
        }
        self.manager.resume(id).await
    }
    pub async fn pause(&self, id: &str) -> Result<()> {
        let task = self.task(id)?;
        if task.phase.active() {
            self.manager.pause(id).await?;
        }
        Ok(())
    }
    pub async fn manifest(&self, id: &str) -> Result<Manifest> {
        let mut cached = self.manifests.lock().await;
        if let Some(manifest) = cached.get(id) {
            return Ok(manifest.clone());
        }
        let record = self.manager.record(id)?;
        if record.view.phase != DownloadPhase::Completed {
            return Err(DownloadError::Busy);
        }
        let publication = record.publication.ok_or(DownloadError::Storage)?;
        let mut files = Vec::new();
        let mut buffer =
            vec![0; usize::try_from(magnets::CHUNK_BYTES).map_err(|_| DownloadError::Limit)?];
        for selected in record.selected {
            let path = files::regular(&publication.root, &selected).await?;
            let mut reader = tokio::fs::File::open(path)
                .await
                .map_err(|_| DownloadError::Storage)?;
            let mut whole = Sha256::new();
            let mut remaining = selected.size;
            let mut chunks = Vec::new();
            while remaining > 0 {
                let length = usize::try_from(remaining.min(magnets::CHUNK_BYTES))
                    .map_err(|_| DownloadError::Limit)?;
                reader
                    .read_exact(&mut buffer[..length])
                    .await
                    .map_err(|_| DownloadError::Storage)?;
                whole.update(&buffer[..length]);
                chunks.push(assemble_protocol::primitives::to_hex(&Sha256::digest(
                    &buffer[..length],
                )));
                remaining -= length as u64;
            }
            let sha256 = assemble_protocol::primitives::to_hex(&whole.finalize());
            if publication
                .hashes
                .iter()
                .find(|(file, _)| *file == selected.id)
                .map(|(_, hash)| hash)
                != Some(&sha256)
            {
                return Err(DownloadError::InvalidMedia);
            }
            files.push(magnets::File {
                file: selected,
                sha256,
                chunks,
            });
        }
        let manifest = Manifest {
            info_hash: librqbit::torrent_from_bytes(&record.metadata)
                .map_err(|_| DownloadError::InvalidMedia)?
                .info_hash
                .as_string(),
            chunk_bytes: magnets::CHUNK_BYTES,
            files,
        };
        cached.insert(id.into(), manifest.clone());
        Ok(manifest)
    }
    pub async fn chunk(&self, id: &str, file_id: u32, index: u64) -> Result<Vec<u8>> {
        let manifest = self.manifest(id).await?;
        let file = manifest
            .files
            .iter()
            .find(|file| file.file.id == file_id)
            .ok_or(DownloadError::Expired)?;
        let digest = file
            .chunks
            .get(usize::try_from(index).map_err(|_| DownloadError::Limit)?)
            .ok_or(DownloadError::Expired)?;
        let record = self.manager.record(id)?;
        let publication = record.publication.ok_or(DownloadError::Storage)?;
        let path = files::regular(&publication.root, &file.file).await?;
        let offset = index
            .checked_mul(magnets::CHUNK_BYTES)
            .ok_or(DownloadError::Limit)?;
        let mut reader = tokio::fs::File::open(path)
            .await
            .map_err(|_| DownloadError::Storage)?;
        reader
            .seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(|_| DownloadError::Storage)?;
        let mut bytes = vec![
            0;
            usize::try_from(
                file.file
                    .size
                    .saturating_sub(offset)
                    .min(magnets::CHUNK_BYTES)
            )
            .map_err(|_| DownloadError::Limit)?
        ];
        reader
            .read_exact(&mut bytes)
            .await
            .map_err(|_| DownloadError::Storage)?;
        if assemble_protocol::primitives::to_hex(&Sha256::digest(&bytes)) != *digest {
            return Err(DownloadError::InvalidMedia);
        }
        Ok(bytes)
    }
    pub async fn purge(&self, id: &str) -> Result<()> {
        if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(DownloadError::InvalidSource);
        }
        if let Ok(task) = self.task(id)
            && !matches!(
                task.phase,
                DownloadPhase::Completed | DownloadPhase::Cancelled
            )
        {
            self.manager.cancel(id).await?;
        }
        let folder = self.root.join("jobs").join(id);
        if tokio::fs::try_exists(&folder)
            .await
            .map_err(|_| DownloadError::Storage)?
        {
            files::reject_links(&folder).await?;
            tokio::fs::remove_dir_all(folder)
                .await
                .map_err(|_| DownloadError::Storage)?;
        }
        {
            let mut state = self.manager.lock();
            state.records.retain(|record| record.view.id != id);
            state.cleaned.remove(id);
            self.manager.persist(&state)?;
        }
        self.manifests.lock().await.remove(id);
        Ok(())
    }
    pub async fn shutdown(&self) {
        self.manager.shutdown().await;
    }
}
