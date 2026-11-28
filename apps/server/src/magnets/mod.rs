//! 私有托管下载。管理员观测库不接收这些查询，源地址也不持久保存。
mod routes;
use crate::{auth::Identity, error::ApiError, http::Shared};
use assemble_download::hosted::{Account, Service};
use assemble_protocol::{
    downloads::DownloadPhase,
    magnets::{Preview, PreviewPhase, Task},
    primitives::Timestamp,
};
pub use routes::routes;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, OnceCell};
use tokio_util::sync::CancellationToken;

pub(super) const TTL: i64 = 86_400_000;
pub(super) fn now() -> i64 {
    Timestamp::now().as_millis()
}
pub(super) fn valid_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())
}
pub(super) fn error(error: assemble_download::DownloadError) -> ApiError {
    use assemble_download::DownloadError as D;
    let status = match error {
        D::Busy => axum::http::StatusCode::TOO_MANY_REQUESTS,
        D::Limit | D::NoSpace | D::ServerQuota => axum::http::StatusCode::PAYLOAD_TOO_LARGE,
        D::Expired => axum::http::StatusCode::NOT_FOUND,
        D::InvalidSource | D::InvalidMedia | D::Unsupported => axum::http::StatusCode::BAD_REQUEST,
        _ => axum::http::StatusCode::SERVICE_UNAVAILABLE,
    };
    ApiError::new(status, error.code(), "hosted download operation failed")
}
#[derive(Clone)]
pub(super) struct Row {
    pub account_id: String,
    pub id: String,
    pub info_hash: String,
    pub metadata: Vec<u8>,
    pub file_ids: String,
    pub selected_bytes: i64,
    pub state: String,
    pub expires_at: i64,
}
impl Row {
    pub fn ids(&self) -> Result<Vec<u32>, ApiError> {
        serde_json::from_str(&self.file_ids)
            .map_err(|_| ApiError::internal("invalid hosted file selection"))
    }
}
struct Pending {
    input_digest: [u8; 32],
    reply: Preview,
    expires: Instant,
    cancel: CancellationToken,
}
type Key = (String, String);
pub struct Runtime {
    service: OnceCell<Arc<Service>>,
    accounts: Mutex<HashMap<String, Arc<Account>>>,
    previews: std::sync::Mutex<HashMap<Key, Pending>>,
    leases: std::sync::Mutex<HashMap<Key, Instant>>,
    operations: Mutex<()>,
    stop: CancellationToken,
    workers: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    maintenance: Mutex<Option<tokio::task::JoinHandle<()>>>,
    #[cfg(test)]
    fail_chunk: std::sync::atomic::AtomicU64,
    #[cfg(test)]
    chunk_calls: std::sync::Mutex<Vec<u64>>,
}
impl std::fmt::Debug for Runtime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HostedDownloads")
    }
}
impl Default for Runtime {
    fn default() -> Self {
        Self {
            service: OnceCell::new(),
            accounts: Mutex::new(HashMap::new()),
            previews: std::sync::Mutex::new(HashMap::new()),
            leases: std::sync::Mutex::new(HashMap::new()),
            operations: Mutex::new(()),
            stop: CancellationToken::new(),
            workers: Mutex::new(Vec::new()),
            maintenance: Mutex::new(None),
            #[cfg(test)]
            fail_chunk: std::sync::atomic::AtomicU64::new(u64::MAX),
            #[cfg(test)]
            chunk_calls: std::sync::Mutex::new(Vec::new()),
        }
    }
}
impl Runtime {
    async fn service(&self, state: &Shared) -> Result<Arc<Service>, ApiError> {
        self.service
            .get_or_try_init(|| async {
                Service::open(state.config.data_dir.join("hosted-magnets").join("network"))
                    .await
                    .map_err(error)
            })
            .await
            .cloned()
    }
    async fn account(&self, state: &Shared, owner: &str) -> Result<Arc<Account>, ApiError> {
        let service = self.service(state).await?;
        let mut accounts = self.accounts.lock().await;
        if let Some(account) = accounts.get(owner) {
            return Ok(Arc::clone(account));
        }
        if accounts.len() >= 128 {
            return Err(error(assemble_download::DownloadError::Busy));
        }

        let account = Account::open(account_root(state, owner), &service).map_err(error)?;
        accounts.insert(owner.into(), Arc::clone(&account));
        Ok(account)
    }
    async fn purge(&self, state: &Shared, owner: &str, id: &str) -> Result<(), ApiError> {
        let loaded = self.accounts.lock().await.get(owner).cloned();
        if let Some(account) = loaded {
            account.purge(id).await.map_err(error)
        } else {
            Account::purge_saved(&account_root(state, owner), id)
                .await
                .map_err(error)
        }
    }
    pub async fn start(self: &Arc<Self>, state: &Shared) {
        let mut task = self.maintenance.lock().await;
        if task.is_some() {
            return;
        }
        let runtime = Arc::clone(self);
        let state = Arc::downgrade(state);
        *task = Some(tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {()=runtime.stop.cancelled()=>break,_=tick.tick()=>{}}
                let Some(state) = state.upgrade() else {
                    break;
                };
                if runtime.tick(&state).await.is_err() {
                    tracing::warn!("hosted magnet maintenance failed");
                }
            }
        }));
    }
    pub async fn shutdown(&self) {
        self.stop.cancel();
        for pending in self
            .previews
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
        {
            pending.cancel.cancel();
        }
        if let Some(task) = self.maintenance.lock().await.take() {
            let _ = task.await;
        }
        for task in std::mem::take(&mut *self.workers.lock().await) {
            let _ = task.await;
        }
        let accounts: Vec<_> = self.accounts.lock().await.values().cloned().collect();
        for account in accounts {
            account.shutdown().await;
        }
        if let Some(service) = self.service.get() {
            service.shutdown().await;
        }
    }
    async fn tick(&self, state: &Shared) -> Result<(), ApiError> {
        self.previews
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|_, pending| {
                if pending.expires > Instant::now() {
                    true
                } else {
                    pending.cancel.cancel();
                    false
                }
            });
        let expired: Vec<_> = self
            .leases
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|(_, expires)| **expires <= Instant::now())
            .map(|(key, _)| key.clone())
            .collect();
        for key in expired {
            let _operation = self.operations.lock().await;
            let expired = {
                let mut leases = self
                    .leases
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if leases.get(&key).is_some_and(|end| *end <= Instant::now()) {
                    leases.remove(&key);
                    true
                } else {
                    false
                }
            };
            if expired && let Some(account) = self.accounts.lock().await.get(&key.0).cloned() {
                let _ = account.pause(&key.1).await;
            }
        }
        let due:Vec<(String,String)>=sqlx::query_as("SELECT account_id,id FROM hosted_magnets WHERE (state<>'active' AND reserved_bytes>0) OR (state='active' AND expires_at<=?) LIMIT 16").bind(now()).fetch_all(&state.pool).await?;
        for (owner, id) in due {
            let _operation = self.operations.lock().await;
            if let Some(row) = row(state, &owner, &id).await? {
                if row.state == "active" && row.expires_at > now() {
                    continue;
                }
                sqlx::query("UPDATE hosted_magnets SET state='cancelled' WHERE account_id=? AND id=? AND state='active'").bind(&owner).bind(&id).execute(&state.pool).await?;
                if self.purge(state, &owner, &id).await.is_ok() {
                    clear(state, &owner, &id).await?;
                }
            }
        }
        // 准入与回收共用同一把锁，避免查询后新建的账户管理器被旧结果关闭。
        let _operation = self.operations.lock().await;
        let active:Vec<String>=sqlx::query_scalar("SELECT DISTINCT account_id FROM hosted_magnets WHERE state='active' OR reserved_bytes>0").fetch_all(&state.pool).await?;
        let removed = {
            let mut accounts = self.accounts.lock().await;
            let keys: Vec<_> = accounts
                .keys()
                .filter(|owner| !active.contains(owner))
                .cloned()
                .collect();
            keys.into_iter()
                .filter_map(|owner| accounts.remove(&owner))
                .collect::<Vec<_>>()
        };
        for account in removed {
            account.shutdown().await;
        }
        sqlx::query("DELETE FROM hosted_magnets WHERE state<>'active' AND reserved_bytes=0 AND expires_at<?").bind(now()-30*TTL).execute(&state.pool).await?;
        Ok(())
    }
    fn preview(&self, owner: &str, id: &str) -> Result<Preview, ApiError> {
        self.previews
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&(owner.into(), id.into()))
            .filter(|p| p.expires > Instant::now())
            .map(|p| p.reply.clone())
            .ok_or_else(ApiError::not_found)
    }
    async fn cached_metadata(
        state: &Shared,
        owner: &str,
        source: &str,
    ) -> Result<Option<Vec<u8>>, assemble_download::DownloadError> {
        let hash = Service::info_hash(source)?;
        // 只复用本人仍在租期内的任务；不向其他账户或已清理任务借元数据。
        let cached: Option<Vec<u8>> = sqlx::query_scalar(
            "SELECT metadata FROM hosted_magnets WHERE account_id=? AND info_hash=? AND state='active' AND expires_at>? ORDER BY created_at DESC LIMIT 1",
        )
        .bind(owner)
        .bind(hash)
        .bind(now())
        .fetch_optional(&state.pool)
        .await
        .map_err(|_| assemble_download::DownloadError::Storage)?;
        cached
            .map(|metadata| Service::validate_metadata(source, &metadata))
            .transpose()
    }
    async fn resolve_metadata(
        &self,
        state: &Shared,
        owner: &str,
        source: &str,
        cancel: &CancellationToken,
    ) -> Result<Vec<u8>, assemble_download::DownloadError> {
        if let Some(metadata) = Self::cached_metadata(state, owner, source).await? {
            tracing::info!("hosted magnet metadata reused");
            return Ok(metadata);
        }
        let service = self
            .service(state)
            .await
            .map_err(|_| assemble_download::DownloadError::ServerUnavailable)?;
        service.resolve(source, cancel).await.map(|v| v.metadata)
    }
    async fn create_preview(
        self: &Arc<Self>,
        state: &Shared,
        identity: &Identity,
        input: assemble_protocol::magnets::PreviewInput,
    ) -> Result<Preview, ApiError> {
        if !valid_id(&input.request_id)
            || input.magnet.len() > 16384
            || input.magnet.chars().any(char::is_control)
        {
            return Err(ApiError::invalid("invalid magnet preview"));
        }
        let key = (identity.account_id.clone(), input.request_id.clone());
        let digest: [u8; 32] = Sha256::digest(input.magnet.as_bytes()).into();
        let cancel = self.stop.child_token();
        let reply = {
            let mut previews = self
                .previews
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(old) = previews.get(&key) {
                if old.input_digest != digest {
                    return Err(ApiError::conflict(
                        "REQUEST_CONFLICT",
                        "preview request changed",
                    ));
                }
                return Ok(old.reply.clone());
            }
            if previews.len() >= 16 {
                let oldest = previews
                    .iter()
                    .filter(|(_, p)| p.reply.phase != PreviewPhase::Resolving)
                    .min_by_key(|(_, p)| p.expires)
                    .map(|(key, _)| key.clone());
                if let Some(key) = oldest {
                    previews.remove(&key);
                }
            }
            if previews.len() >= 16
                || previews
                    .iter()
                    .filter(|((owner, _), p)| {
                        owner == &identity.account_id && p.reply.phase == PreviewPhase::Resolving
                    })
                    .count()
                    >= 2
            {
                return Err(error(assemble_download::DownloadError::Busy));
            }
            let reply = Preview {
                id: input.request_id,
                account_id: identity.account_id.clone(),
                phase: PreviewPhase::Resolving,
                metainfo: None,
                error_code: None,
            };
            previews.insert(
                key.clone(),
                Pending {
                    input_digest: digest,
                    reply: reply.clone(),
                    expires: Instant::now() + Duration::from_secs(600),
                    cancel: cancel.clone(),
                },
            );
            reply
        };
        let runtime = Arc::clone(self);
        let state = Arc::clone(state);
        let mut workers = self.workers.lock().await;
        workers.retain(|task| !task.is_finished());
        workers.push(tokio::spawn(async move{
            let result=tokio::select!{()=cancel.cancelled()=>Err(assemble_download::DownloadError::Cancelled),reply=tokio::time::timeout(Duration::from_secs(95),runtime.resolve_metadata(&state,&key.0,&input.magnet,&cancel))=>reply.unwrap_or(Err(assemble_download::DownloadError::MetadataUnavailable))};
            let mut previews=runtime.previews.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(pending)=previews.get_mut(&key){
                match result{Ok(metadata) if !cancel.is_cancelled()=>{use base64::Engine as _;pending.reply.phase=PreviewPhase::Ready;pending.reply.metainfo=Some(base64::engine::general_purpose::STANDARD.encode(metadata));},Ok(_)=>{pending.reply.phase=PreviewPhase::Failed;pending.reply.error_code=Some("DOWNLOAD_CANCELLED".into());},Err(error)=>{pending.reply.phase=PreviewPhase::Failed;pending.reply.error_code=Some(error.code().into());}}
            }
        }));
        Ok(reply)
    }
    async fn task(&self, state: &Shared, row: &Row) -> Result<Task, ApiError> {
        let terminal = row.state != "active" || row.expires_at <= now();
        let view = if terminal {
            None
        } else {
            let loaded = self
                .accounts
                .lock()
                .await
                .get(&row.account_id)
                .cloned()
                .and_then(|account| account.task(&row.id).ok());
            if loaded.is_some() {
                loaded
            } else {
                Account::saved_task(&account_root(state, &row.account_id), &row.id)
                    .map_err(error)?
            }
        };
        Ok(Task {
            id: row.id.clone(),
            account_id: row.account_id.clone(),
            info_hash: row.info_hash.clone(),
            file_ids: row.ids()?,
            phase: view.as_ref().map_or(
                if row.state == "released" {
                    DownloadPhase::Completed
                } else if terminal {
                    DownloadPhase::Cancelled
                } else {
                    DownloadPhase::Paused
                },
                |v| v.phase,
            ),
            received_bytes: view.as_ref().map_or(
                if row.state == "released" {
                    u64::try_from(row.selected_bytes).unwrap_or(0)
                } else {
                    0
                },
                |v| v.received_bytes,
            ),
            total_bytes: u64::try_from(row.selected_bytes)
                .map_err(|_| ApiError::internal("invalid hosted size"))?,
            bytes_per_second: view.as_ref().map_or(0, |v| v.bytes_per_second),
            error_code: view.and_then(|v| v.error_code),
            expires_at: Timestamp::from_millis(row.expires_at)
                .ok_or_else(|| ApiError::internal("invalid hosted expiry"))?,
            released: row.state == "released",
        })
    }
}
pub(super) async fn row(state: &Shared, owner: &str, id: &str) -> Result<Option<Row>, ApiError> {
    type Data = (String, String, String, Vec<u8>, String, i64, String, i64);
    let value:Option<Data>=sqlx::query_as("SELECT account_id,id,info_hash,metadata,file_ids,selected_bytes,state,expires_at FROM hosted_magnets WHERE account_id=? AND id=?").bind(owner).bind(id).fetch_optional(&state.pool).await?;
    Ok(value.map(
        |(account_id, id, info_hash, metadata, file_ids, selected_bytes, state, expires_at)| Row {
            account_id,
            id,
            info_hash,
            metadata,
            file_ids,
            selected_bytes,
            state,
            expires_at,
        },
    ))
}
pub(super) async fn clear(state: &Shared, owner: &str, id: &str) -> Result<(), ApiError> {
    sqlx::query(
        "UPDATE hosted_magnets SET reserved_bytes=0,metadata=X'' WHERE account_id=? AND id=?",
    )
    .bind(owner)
    .bind(id)
    .execute(&state.pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests;

fn account_root(state: &Shared, owner: &str) -> std::path::PathBuf {
    state
        .config
        .data_dir
        .join("hosted-magnets")
        .join("accounts")
        .join(assemble_protocol::primitives::to_hex(&Sha256::digest(
            owner.as_bytes(),
        )))
}
