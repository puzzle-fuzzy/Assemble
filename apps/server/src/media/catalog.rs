//! 匿名来源查询有独立成本预算；缓存只接收纯资料模型，并逐次确认公开图像引用仍有效。
use crate::error::ApiError;
use crate::http::Shared;
use assemble_protocol::media::{
    MediaActorCredits, MediaCandidate, MediaIdentity, MediaPersonIdentity, MediaType,
};
use axum::http::StatusCode;
use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};

const BUDGET: usize = 60;
const WINDOW_MILLIS: i64 = 60_000;
const CACHE_TTL_MILLIS: i64 = 15 * 60_000;
const CACHE_MAX_BYTES: usize = 8 * 1024 * 1024;
#[derive(Debug, Clone)]
enum PureCatalog {
    Candidate(MediaCandidate),
    Actor(MediaActorCredits),
}
impl PureCatalog {
    fn image_id(&self) -> Option<&str> {
        match self {
            Self::Candidate(v) => v.poster.as_ref(),
            Self::Actor(v) => v.actor.photo.as_ref(),
        }
        .map(|i| i.asset_id.as_str())
    }
    fn size(&self) -> Result<usize, ApiError> {
        match self {
            Self::Candidate(v) => super::encode(v),
            Self::Actor(v) => super::encode(v),
        }
        .map(|s| s.len())
    }
}
#[derive(Debug)]
pub struct Catalog {
    slots: Arc<Semaphore>,
    budget: Mutex<VecDeque<i64>>,
    cache: Mutex<BTreeMap<String, (i64, usize, PureCatalog)>>,
}
impl Default for Catalog {
    fn default() -> Self {
        Self {
            slots: Arc::new(Semaphore::new(2)),
            budget: Mutex::new(VecDeque::new()),
            cache: Mutex::new(BTreeMap::new()),
        }
    }
}
impl Catalog {
    async fn begin(&self) -> Result<OwnedSemaphorePermit, ApiError> {
        let permit = Arc::clone(&self.slots)
            .try_acquire_owned()
            .map_err(|_| limited())?;
        let now = super::now();
        let mut budget = self.budget.lock().await;
        while budget.front().is_some_and(|at| *at <= now - WINDOW_MILLIS) {
            budget.pop_front();
        }
        if budget.len() >= BUDGET {
            return Err(limited());
        }
        budget.push_back(now);
        Ok(permit)
    }
    async fn cached(&self, state: &Shared, key: &str) -> Result<Option<PureCatalog>, ApiError> {
        let cached = self
            .cache
            .lock()
            .await
            .get(key)
            .filter(|(expires, _, _)| *expires > super::now())
            .map(|(_, _, value)| value.clone());
        if let Some(cached) = cached {
            if let Some(asset) = cached.image_id() {
                let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM media_catalog_assets WHERE asset_id = ? AND expires_at > ?)").bind(asset).bind(super::now()).fetch_one(&state.pool).await?;
                if !valid {
                    return Ok(None);
                }
            }
            return Ok(Some(cached));
        }
        Ok(None)
    }
    async fn remember(&self, key: String, value: PureCatalog) -> Result<(), ApiError> {
        let bytes = value.size()?;
        if bytes > CACHE_MAX_BYTES {
            return Ok(());
        }
        let mut cache = self.cache.lock().await;
        cache.retain(|_, (at, _, _)| *at > super::now());
        while cache.len() >= 100
            || cache
                .values()
                .map(|(_, size, _)| size)
                .sum::<usize>()
                .saturating_add(bytes)
                > CACHE_MAX_BYTES
        {
            if cache.pop_first().is_none() {
                break;
            }
        }
        cache.insert(key, (super::now() + CACHE_TTL_MILLIS, bytes, value));
        Ok(())
    }
    pub async fn details(
        &self,
        state: &Shared,
        identity: &MediaIdentity,
    ) -> Result<MediaCandidate, ApiError> {
        super::providers::validate_identity(identity)?;
        let key = format!("details:{}", super::encode(identity)?);
        if let Some(PureCatalog::Candidate(item)) = self.cached(state, &key).await? {
            return Ok(item);
        }
        let _permit = self.begin().await?;
        let details = state.media.providers.details(identity).await?;
        let mut item = details.candidate;
        if let Some(source) = details.poster {
            let bytes = state.media.providers.image(&source).await?;
            item.poster = Some(super::assets::store(&state.pool, "catalog", bytes, true).await?);
        }
        self.remember(key, PureCatalog::Candidate(item.clone()))
            .await?;
        Ok(item)
    }
    pub async fn actor(
        &self,
        state: &Shared,
        identity: &MediaPersonIdentity,
        kind: MediaType,
    ) -> Result<MediaActorCredits, ApiError> {
        super::providers::validate_external_id(&identity.external_id)?;
        let key = format!(
            "actor:{}:{}",
            super::encode(identity)?,
            super::spelling(&kind)
        );
        if let Some(PureCatalog::Actor(item)) = self.cached(state, &key).await? {
            return Ok(item);
        }
        let _permit = self.begin().await?;
        let (mut actor, photo, items) = state.media.providers.actor(identity, kind).await?;
        if let Some(photo) = photo {
            let bytes = state.media.providers.image(&photo).await?;
            actor.photo = Some(super::assets::store(&state.pool, "catalog", bytes, true).await?);
        }
        let item = MediaActorCredits { actor, items };
        self.remember(key, PureCatalog::Actor(item.clone())).await?;
        Ok(item)
    }
}
fn limited() -> ApiError {
    ApiError::new(
        StatusCode::TOO_MANY_REQUESTS,
        "MEDIA_CATALOG_RATE_LIMITED",
        "public metadata lookup budget is exhausted; retry later",
    )
}
