//! 匿名来源查询有独立成本预算；缓存只接收纯资料模型，并逐次确认公开图像引用仍有效。
use crate::error::ApiError;
use crate::http::Shared;
use crate::source::Source;
use assemble_protocol::media::{
    MediaActorCredits, MediaCandidate, MediaIdentity, MediaPersonIdentity, MediaType,
};
use axum::http::{HeaderMap, StatusCode};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};

const BUDGET: usize = 60;
/// 来源已知时，同一个来源每分钟最多这么多次。全站那份是所有人共用的：不按来源分，
/// 谁都能用脚本把它用光，别人的查询就都成了"稍后再试"。
const SOURCE_BUDGET: usize = 20;
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
    /// 来源到它最近一分钟的查询时刻。只记来源已知的。
    sources: Mutex<HashMap<String, VecDeque<i64>>>,
    cache: Mutex<BTreeMap<String, (i64, usize, PureCatalog)>>,
}
impl Default for Catalog {
    fn default() -> Self {
        Self {
            slots: Arc::new(Semaphore::new(2)),
            budget: Mutex::new(VecDeque::new()),
            sources: Mutex::new(HashMap::new()),
            cache: Mutex::new(BTreeMap::new()),
        }
    }
}
impl Catalog {
    async fn begin(&self, source: &Source) -> Result<OwnedSemaphorePermit, ApiError> {
        let permit = Arc::clone(&self.slots)
            .try_acquire_owned()
            .map_err(|_| limited())?;
        let now = super::now();
        // 先看来源那一份：被拦的查询不去占全站的额度。
        if let Some(source) = source.known() {
            let mut sources = self.sources.lock().await;
            // 久未活动的来源顺手清掉，内存不随来访者的数目增长。
            sources.retain(|_, at| at.back().is_some_and(|last| *last > now - WINDOW_MILLIS));
            let entries = sources.entry(source.to_owned()).or_default();
            while entries.front().is_some_and(|at| *at <= now - WINDOW_MILLIS) {
                entries.pop_front();
            }
            if entries.len() >= SOURCE_BUDGET {
                return Err(limited());
            }
            entries.push_back(now);
        }
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
        headers: &HeaderMap,
        identity: &MediaIdentity,
    ) -> Result<MediaCandidate, ApiError> {
        super::providers::validate_identity(identity)?;
        let key = format!("details:{}", super::encode(identity)?);
        if let Some(PureCatalog::Candidate(item)) = self.cached(state, &key).await? {
            return Ok(item);
        }
        let source = Source::read(headers, state.config.client_ip_header.as_ref());
        let _permit = self.begin(&source).await?;
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
        headers: &HeaderMap,
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
        let source = Source::read(headers, state.config.client_ip_header.as_ref());
        let _permit = self.begin(&source).await?;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 一个来源已知的请求：请求头里带着代理记下的地址。
    fn from(address: &'static str) -> Source {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", axum::http::HeaderValue::from_static(address));
        let trusted = axum::http::HeaderName::from_static("x-real-ip");
        Source::read(&headers, Some(&trusted))
    }

    /// 公开的目录查询谁都能发。不按来源分的话，一个来源就能把全站每分钟的额度用光。
    #[tokio::test]
    async fn one_source_cannot_use_up_the_lookup_budget_of_everyone_else() {
        let catalog = Catalog::default();
        let stranger = from("203.0.113.7");
        let myself = from("198.51.100.9");
        for _ in 0..SOURCE_BUDGET {
            let _permit = catalog.begin(&stranger).await.unwrap();
        }
        assert_eq!(
            catalog.begin(&stranger).await.unwrap_err().code,
            "MEDIA_CATALOG_RATE_LIMITED"
        );
        // 被拦的那一次没有占全站的额度：别的来源照常。
        let _permit = catalog.begin(&myself).await.unwrap();
        assert_eq!(catalog.budget.lock().await.len(), SOURCE_BUDGET + 1);
        // 来源未知时不按来源分，只剩全站那一层。
        let unknown = Source::unknown();
        for _ in 0..(BUDGET - SOURCE_BUDGET - 1) {
            let _permit = catalog.begin(&unknown).await.unwrap();
        }
        assert_eq!(
            catalog.begin(&unknown).await.unwrap_err().code,
            "MEDIA_CATALOG_RATE_LIMITED"
        );
        // 久未活动的来源被清掉，内存不随来访者的数目增长。换一个新实例：上面刚把额度用满。
        let catalog = Catalog::default();
        catalog.sources.lock().await.clear();
        catalog
            .sources
            .lock()
            .await
            .insert("stale".to_owned(), [i64::MIN].into());
        let _permit = catalog.begin(&from("198.51.100.10")).await.unwrap();
        assert!(!catalog.sources.lock().await.contains_key("stale"));
    }
}
