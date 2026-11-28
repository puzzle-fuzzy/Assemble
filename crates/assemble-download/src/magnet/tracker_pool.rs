//! 只缓存无凭据的公共候选；刷新从任务触发，失败仍用最后有效结果，退出显式等待取消。
use super::trackers::{self, Endpoint};
use crate::network::Network;
use assemble_store::json::{read_json, write_private_json};
use futures_util::{StreamExt, stream};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const SOURCE: &str = "https://raw.githubusercontent.com/adysec/tracker/main/trackers_best.txt";
const SEEDS: &str = include_str!("tracker-seeds.txt");
const TTL: u64 = 24 * 60 * 60;
pub(super) const MAX_POOL: usize = 12;

#[derive(Clone, Serialize, Deserialize)]
struct Cache {
    checked_at: u64,
    urls: Vec<String>,
}
struct State {
    cache: Cache,
    next_attempt: Instant,
    refresh: Option<JoinHandle<()>>,
}
pub(crate) struct Pool {
    path: PathBuf,
    state: Mutex<State>,
    network: Network,
    cancel: CancellationToken,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn candidates(text: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    text.lines()
        .filter_map(|line| trackers::parse(line.trim(), true).ok())
        .filter(|url| matches!(url.scheme(), "udp" | "https"))
        .map(|url| url.to_string())
        .filter(|url| seen.insert(url.clone()))
        .take(512)
        .collect()
}
impl Pool {
    pub fn open(path: PathBuf) -> Arc<Self> {
        let fallback = Cache {
            checked_at: 0,
            urls: candidates(SEEDS),
        };
        let cache = if std::fs::metadata(&path).is_ok_and(|m| m.len() <= 65536) {
            read_json::<Cache>(&path).ok().flatten().filter(|cache| {
                cache.checked_at <= now()
                    && cache.urls.len() <= MAX_POOL
                    && !cache.urls.is_empty()
                    && cache.urls.iter().all(|u| candidates(u) == [u.clone()])
            })
        } else {
            None
        }
        .unwrap_or(fallback);
        Arc::new(Self {
            path,
            state: Mutex::new(State {
                cache,
                next_attempt: Instant::now(),
                refresh: None,
            }),
            network: Network::default(),
            cancel: CancellationToken::new(),
        })
    }
    pub fn get(self: &Arc<Self>) -> Vec<String> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self.cancel.is_cancelled()
            && now().saturating_sub(state.cache.checked_at) >= TTL
            && Instant::now() >= state.next_attempt
            && state.refresh.as_ref().is_none_or(JoinHandle::is_finished)
        {
            state.next_attempt = Instant::now() + Duration::from_secs(600);
            let pool = Arc::clone(self);
            state.refresh = Some(tokio::spawn(async move {
                pool.refresh().await;
            }));
        }
        state.cache.urls.clone()
    }
    async fn fetch(&self) -> Option<Vec<String>> {
        let url = url::Url::parse(SOURCE).ok()?;
        let (_, mut response) = self
            .network
            .fetch(
                &url,
                &reqwest::header::HeaderMap::new(),
                None,
                None,
                &self.cancel,
            )
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.ok()? {
            if body.len() + chunk.len() > 262_144 {
                return None;
            }
            body.extend_from_slice(&chunk);
        }
        let values = candidates(std::str::from_utf8(&body).ok()?);
        (!values.is_empty()).then_some(values)
    }
    async fn refresh(&self) {
        let mut urls = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .cache
            .urls
            .clone();
        let remote = tokio::select! { ()=self.cancel.cancelled()=>return, result=tokio::time::timeout(Duration::from_secs(8), self.fetch())=>result };
        if let Ok(Some(remote)) = remote {
            let step = remote.len().div_ceil(64).max(1);
            urls.extend(remote.into_iter().step_by(step).take(64));
        }
        let mut seen = HashSet::new();
        urls.retain(|url| seen.insert(url.clone()));
        let mut probes = stream::iter(urls)
            .map(|value| async move {
                let at = Instant::now();
                let url = trackers::parse(&value, true).ok()?;
                let endpoint = Endpoint::resolve(url, &self.network, false).await.ok()?;
                endpoint
                    .healthy()
                    .await
                    .then_some((value, at.elapsed(), endpoint.addresses))
            })
            .buffer_unordered(8)
            .filter_map(|v| async move { v })
            .boxed();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
        let mut healthy = Vec::new();
        loop {
            tokio::select! {
                ()=self.cancel.cancelled()=>return,
                ()=tokio::time::sleep_until(deadline)=>break,
                result=probes.next()=>match result {Some(value)=>healthy.push(value),None=>break},
            }
        }
        healthy.sort_by_key(|(_, latency, _)| *latency);
        let mut ips = HashSet::new();
        let mut udp = 0;
        let mut https = 0;
        let urls: Vec<_> = healthy
            .into_iter()
            .filter_map(|(url, _, addrs)| {
                let count = if url.starts_with("udp:") {
                    &mut udp
                } else {
                    &mut https
                };
                let limit = if url.starts_with("udp:") { 8 } else { 4 };
                if *count >= limit || addrs.iter().any(|a| ips.contains(&a.ip())) {
                    return None;
                }
                ips.extend(addrs.iter().map(std::net::SocketAddr::ip));
                *count += 1;
                Some(url)
            })
            .take(MAX_POOL)
            .collect();
        if urls.is_empty() || self.cancel.is_cancelled() {
            return;
        }
        let cache = Cache {
            checked_at: now(),
            urls,
        };
        if write_private_json(&self.path, &cache).is_ok() {
            self.state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .cache = cache;
        }
    }
    pub async fn shutdown(&self) {
        self.cancel.cancel();
        let handle = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .refresh
            .take();
        if let Some(handle) = handle {
            let _ = handle.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_credentials_unsupported_protocols_and_bounded_cache() {
        let text = "udp://8.8.8.8:80/announce\nhttps://example.com/announce\nhttps://example.com/announce?key=secret\nwss://example.com\nhttps://user:pass@example.com/announce\nudp://8.8.8.8:80/announce";
        assert_eq!(candidates(text).len(), 2);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trackers.json");
        write_private_json(
            &path,
            &Cache {
                checked_at: now() + 60,
                urls: vec!["https://example.com/announce?key=secret".into()],
            },
        )
        .unwrap();
        assert_eq!(Pool::open(path).state.lock().unwrap().cache.urls.len(), 12);
    }
    #[tokio::test]
    async fn shutdown_does_not_start_or_leave_a_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.json");
        let urls = vec!["udp://8.8.8.8:80/announce".to_owned()];
        write_private_json(
            &path,
            &Cache {
                checked_at: now().saturating_sub(TTL + 1),
                urls: urls.clone(),
            },
        )
        .unwrap();
        let before = std::fs::read(&path).unwrap();
        let pool = Pool::open(path.clone());
        assert_eq!(pool.get(), urls);
        pool.shutdown().await;
        assert_eq!(pool.get(), urls);
        assert!(pool.state.lock().unwrap().refresh.is_none());
        assert_eq!(std::fs::read(path).unwrap(), before);
    }
}
