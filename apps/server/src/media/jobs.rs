//! 任务阶段以实际工作为准；写入最终条目和完成回执在同一事务提交。
use super::auth::Principal;
use super::{assets, auth, providers, store};
use crate::error::ApiError;
use crate::http::Shared;
use assemble_protocol::media::{
    MediaIdentity, MediaJob, MediaJobStage, MediaJobStatus, MediaScope,
};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::sync::Arc;
use tokio::sync::{Mutex, Semaphore};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(super) enum Input {
    Import {
        identity: MediaIdentity,
    },
    Gallery {
        entry_id: String,
        version: u64,
        identity: MediaIdentity,
    },
}
impl Input {
    fn identity(&self) -> &MediaIdentity {
        match self {
            Self::Import { identity } | Self::Gallery { identity, .. } => identity,
        }
    }
}
#[derive(Debug)]
pub struct Runtime {
    pub providers: Arc<providers::Providers>,
    pub(super) catalog: super::catalog::Catalog,
    stop: CancellationToken,
    slots: Semaphore,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}
impl Runtime {
    pub fn new(config: super::MediaConfig) -> Self {
        Self::with_providers(Arc::new(
            providers::Providers::new(config)
                .expect("fixed metadata HTTP client configuration is valid"),
        ))
    }
    pub fn with_providers(providers: Arc<providers::Providers>) -> Self {
        Self {
            providers,
            catalog: super::catalog::Catalog::default(),
            stop: CancellationToken::new(),
            slots: Semaphore::new(2),
            tasks: Mutex::new(Vec::new()),
        }
    }
    pub async fn recover(self: &Arc<Self>, state: &Shared) -> Result<(), ApiError> {
        let rows:Vec<(String,String,String,String,String,String,String)>=sqlx::query_as("SELECT id,actor_id,owner_key,credential_kind,credential_id,data,input FROM media_jobs WHERE json_extract(data,'$.status') IN ('queued','running')").fetch_all(&state.pool).await?;
        for (_id, account_id, owner_key, credential_kind, credential_id, data, input) in rows {
            let mut job: MediaJob = super::decode(&data)?;
            if job.status == MediaJobStatus::Running {
                job.status = MediaJobStatus::Interrupted;
                job.error_code = Some("MEDIA_JOB_INTERRUPTED".into());
                job.stage = None;
                write_job(&state.pool, &job).await?;
            } else {
                let admin = owner_key == "shared" || owner_key != account_id;
                let principal = Principal {
                    target_owner: (owner_key != "shared" && owner_key != account_id)
                        .then(|| owner_key.clone()),
                    account_id,
                    credential_kind,
                    credential_id,
                    admin,
                    csrf: None,
                };
                let input: Input = super::decode(&input)?;
                self.spawn(state.clone(), principal, job, input).await;
            }
        }
        Ok(())
    }
    async fn spawn(
        self: &Arc<Self>,
        state: Shared,
        principal: Principal,
        job: MediaJob,
        input: Input,
    ) {
        let runtime = Arc::clone(self);
        let handle = tokio::spawn(async move {
            let result = tokio::select! {()=runtime.stop.cancelled()=>Err(ApiError::conflict("MEDIA_JOB_CANCELLED","media job cancelled")),result=async{let _permit=runtime.slots.acquire().await.map_err(|_|ApiError::internal("media worker unavailable"))?;run(&state,&principal,job.clone(),input).await}=>result};
            if let Err(error) = result {
                let mut final_job = job;
                final_job.status = if error.code == "MEDIA_JOB_CANCELLED" {
                    MediaJobStatus::Cancelled
                } else {
                    MediaJobStatus::Failed
                };
                final_job.error_code = Some(error.code.into());
                final_job.stage = None;
                if write_job(&state.pool, &final_job).await.is_err() {
                    tracing::error!("media job terminal state could not be saved");
                }
            }
        });
        let mut tasks = self.tasks.lock().await;
        tasks.retain(|task| !task.is_finished());
        tasks.push(handle);
    }
    pub async fn shutdown(&self, pool: &SqlitePool) {
        self.stop.cancel();
        let tasks = std::mem::take(&mut *self.tasks.lock().await);
        for task in tasks {
            if task.await.is_err() {
                tracing::error!("media worker exited unexpectedly");
            }
        }
        if let Err(_error) = assets::cleanup(pool).await {
            tracing::warn!("media asset cleanup failed");
        }
    }
}
async fn write_job(pool: &SqlitePool, job: &MediaJob) -> Result<(), ApiError> {
    sqlx::query("UPDATE media_jobs SET data = ?, updated_at = ? WHERE id = ? AND json_extract(data,'$.status') IN ('queued','running')").bind(super::encode(job)?).bind(super::now()).bind(&job.id).execute(pool).await?;
    Ok(())
}
async fn progress(
    pool: &SqlitePool,
    job: &mut MediaJob,
    stage: MediaJobStage,
    completed: Option<u32>,
    total: Option<u32>,
) -> Result<(), ApiError> {
    job.status = MediaJobStatus::Running;
    job.stage = Some(stage);
    job.completed_items = completed;
    job.total_items = total;
    write_job(pool, job).await
}
pub(super) async fn get(
    pool: &SqlitePool,
    scope: MediaScope,
    principal: &Principal,
    id: &str,
) -> Result<MediaJob, ApiError> {
    store::check_id(id)?;
    let data: Option<String> =
        sqlx::query_scalar("SELECT data FROM media_jobs WHERE id = ? AND owner_key = ?")
            .bind(id)
            .bind(auth::owner_key(scope, principal))
            .fetch_optional(pool)
            .await?;
    let job: MediaJob = super::decode(&data.ok_or_else(ApiError::not_found)?)?;
    if job.scope != scope {
        return Err(ApiError::not_found());
    }
    Ok(job)
}
pub(super) async fn queue(
    state: &Shared,
    scope: MediaScope,
    principal: &Principal,
    request_id: &str,
    input: Input,
) -> Result<MediaJob, ApiError> {
    providers::validate_identity(input.identity())?;
    let operation = format!("job:{}", super::spelling(&scope));
    let fingerprint = store::fingerprint(&input)?;
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    auth::revalidate(&mut tx, principal, scope).await?;
    if let Some(id) =
        store::replay(&mut tx, principal, request_id, &operation, &fingerprint).await?
    {
        drop(tx);
        return get(&state.pool, scope, principal, &id).await;
    }
    let active:i64=sqlx::query_scalar("SELECT count(*) FROM media_jobs WHERE actor_id = ? AND json_extract(data,'$.status') IN ('queued','running')").bind(&principal.account_id).fetch_one(&mut *tx).await?;
    if active >= 10 {
        return Err(ApiError::conflict(
            "MEDIA_BUSY",
            "too many media jobs are pending",
        ));
    }
    if let Input::Gallery {
        entry_id, version, ..
    } = &input
    {
        let entry = store::load(
            &mut *tx,
            scope,
            store::owner(scope, Some(principal))?,
            entry_id,
        )
        .await?;
        if entry.version != *version {
            return Err(crate::planner::version_conflict());
        }
    }
    let job = MediaJob {
        id: super::id()?,
        scope,
        status: MediaJobStatus::Queued,
        stage: None,
        completed_items: None,
        total_items: None,
        entry_id: None,
        error_code: None,
    };
    sqlx::query("INSERT INTO media_jobs(id,actor_id,owner_key,credential_kind,credential_id,data,input,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?)").bind(&job.id).bind(&principal.account_id).bind(auth::owner_key(scope,principal)).bind(&principal.credential_kind).bind(&principal.credential_id).bind(super::encode(&job)?).bind(super::encode(&input)?).bind(super::now()).bind(super::now()).execute(&mut *tx).await?;
    store::remember(
        &mut tx,
        principal,
        request_id,
        &operation,
        &fingerprint,
        &job.id,
    )
    .await?;
    tx.commit().await?;
    state
        .media
        .spawn(state.clone(), principal.clone(), job.clone(), input)
        .await;
    Ok(job)
}
async fn run(
    state: &Shared,
    principal: &Principal,
    mut job: MediaJob,
    input: Input,
) -> Result<(), ApiError> {
    progress(&state.pool, &mut job, MediaJobStage::Metadata, None, None).await?;
    let mut guard = state.pool.begin().await?;
    auth::revalidate(&mut guard, principal, job.scope).await?;
    drop(guard);
    match input {
        Input::Import { identity } => run_import(state, principal, job, identity).await,
        Input::Gallery {
            entry_id,
            version,
            identity,
        } => run_gallery(state, principal, job, entry_id, version, identity).await,
    }
}
async fn run_import(
    state: &Shared,
    principal: &Principal,
    mut job: MediaJob,
    identity: MediaIdentity,
) -> Result<(), ApiError> {
    let owner = auth::owner_key(job.scope, principal);
    let details = if let Some(entry) = super::archive::cached(&state.pool, &identity).await? {
        super::providers::Details {
            candidate: super::archive::candidate(entry.clone(), identity.clone()),
            content: entry.content,
            cast: entry
                .cast
                .into_iter()
                .map(|cast| super::providers::SourceCast { cast, photo: None })
                .collect(),
            poster: None,
        }
    } else {
        state.media.providers.details(&identity).await?
    };
    let total = details.cast.iter().filter(|c| c.photo.is_some()).count()
        + usize::from(details.poster.is_some());
    progress(
        &state.pool,
        &mut job,
        MediaJobStage::Images,
        Some(0),
        Some(u32::try_from(total).map_err(|_| ApiError::invalid("too many media images"))?),
    )
    .await?;
    let mut entry = store::new_entry(job.scope, details.content)?;
    entry.poster = details.candidate.poster;
    entry.identities = vec![identity.clone()];
    let mut completed = 0;
    if let Some(source) = details.poster {
        let bytes = state.media.providers.image(&source).await?;
        entry.poster = Some(assets::store(&state.pool, &owner, bytes, false).await?);
        completed += 1;
        let total = job.total_items;
        progress(
            &state.pool,
            &mut job,
            MediaJobStage::Images,
            Some(completed),
            total,
        )
        .await?;
    }
    for source in details.cast {
        let mut cast = source.cast;
        if let Some(photo) = source.photo {
            let bytes = state.media.providers.image(&photo).await?;
            cast.photo = Some(assets::store(&state.pool, &owner, bytes, false).await?);
            completed += 1;
            let total = job.total_items;
            progress(
                &state.pool,
                &mut job,
                MediaJobStage::Images,
                Some(completed),
                total,
            )
            .await?;
        }
        entry.cast.push(cast);
    }
    progress(&state.pool, &mut job, MediaJobStage::Saving, None, None).await?;
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    auth::revalidate(&mut tx, principal, job.scope).await?;
    let existing:Option<String>=sqlx::query_scalar("SELECT entry_id FROM media_identities WHERE scope_key = ? AND provider = ? AND media_type = ? AND external_id = ?").bind(&owner).bind(super::spelling(&identity.provider)).bind(super::spelling(&identity.media_type)).bind(&identity.external_id).fetch_optional(&mut *tx).await?;
    if let Some(existing) = existing {
        job.entry_id = Some(existing);
    } else {
        entry.content.published = job.scope == MediaScope::Shared;
        store::insert(&mut tx, principal, &mut entry).await?;
        store::audit(&mut tx, principal, "media.imported", &entry).await?;
        if let Some(poster) = &entry.poster {
            assets::link(&mut tx, &entry.id, std::slice::from_ref(poster), "gallery").await?;
        }
        let portraits: Vec<_> = entry.cast.iter().filter_map(|c| c.photo.clone()).collect();
        assets::link(&mut tx, &entry.id, &portraits, "cast").await?;
        job.entry_id = Some(entry.id);
    }
    complete_in(&mut tx, &mut job).await?;
    tx.commit().await?;
    Ok(())
}
async fn run_gallery(
    state: &Shared,
    principal: &Principal,
    mut job: MediaJob,
    entry_id: String,
    version: u64,
    identity: MediaIdentity,
) -> Result<(), ApiError> {
    let owner = auth::owner_key(job.scope, principal);
    let sources = state.media.providers.gallery(&identity).await?;
    let total =
        u32::try_from(sources.len()).map_err(|_| ApiError::invalid("too many gallery images"))?;
    progress(
        &state.pool,
        &mut job,
        MediaJobStage::Images,
        Some(0),
        Some(total),
    )
    .await?;
    let mut images = Vec::new();
    for source in sources {
        let bytes = state.media.providers.image(&source).await?;
        images.push(assets::store(&state.pool, &owner, bytes, false).await?);
        progress(
            &state.pool,
            &mut job,
            MediaJobStage::Images,
            Some(
                u32::try_from(images.len())
                    .map_err(|_| ApiError::invalid("too many gallery images"))?,
            ),
            Some(total),
        )
        .await?;
    }
    progress(&state.pool, &mut job, MediaJobStage::Saving, None, None).await?;
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    auth::revalidate(&mut tx, principal, job.scope).await?;
    let mut entry = store::load(
        &mut *tx,
        job.scope,
        store::owner(job.scope, Some(principal))?,
        &entry_id,
    )
    .await?;
    if entry.version != version {
        return Err(crate::planner::version_conflict());
    }
    // 完整来源列表全部取回后才替换；当前封面仍作为有效引用保留。
    if let Some(poster) = &entry.poster
        && !images.iter().any(|i| i.asset_id == poster.asset_id)
    {
        images.insert(0, poster.clone());
    }
    sqlx::query("DELETE FROM media_entry_assets WHERE entry_id = ? AND role = 'gallery'")
        .bind(&entry_id)
        .execute(&mut *tx)
        .await?;
    assets::link(&mut tx, &entry_id, &images, "gallery").await?;
    if entry.poster.is_none() {
        entry.poster = images.first().cloned();
    }
    let entry = store::save(&mut tx, entry).await?;
    store::audit(&mut tx, principal, "media.gallery_synced", &entry).await?;
    job.entry_id = Some(entry_id);
    complete_in(&mut tx, &mut job).await?;
    tx.commit().await?;
    Ok(())
}

async fn complete_in(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    job: &mut MediaJob,
) -> Result<(), ApiError> {
    job.status = MediaJobStatus::Completed;
    job.stage = None;
    job.completed_items = None;
    job.total_items = None;
    job.error_code = None;
    sqlx::query("UPDATE media_jobs SET data = ?, updated_at = ? WHERE id = ?")
        .bind(super::encode(job)?)
        .bind(super::now())
        .bind(&job.id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
