//! BT 引擎只拿 Rust 内部来源；关闭它自己的历史和局域网发现，恢复资料由本模块管理。
use super::files::relative_path;
use super::{tracker_pool, trackers};
use crate::{DownloadError, MAX_METADATA, Result};
use assemble_protocol::downloads::DownloadFile;
use assemble_store::secret::SecretString;
use futures_util::{StreamExt, TryStreamExt};
use librqbit::{
    AddTorrent, AddTorrentOptions, AddTorrentResponse, DhtSessionConfig, Session, SessionOptions,
};
use std::{net::SocketAddr, num::NonZeroU32, path::PathBuf, sync::Arc};
use tokio_util::sync::CancellationToken;
use url::Url;

pub(crate) const MAX_FILES: usize = 10_000;
pub(crate) const MAX_TOTAL: u64 = 1024 * 1024 * 1024 * 1024;
pub(crate) const MAX_SELECTED: u64 = 100 * 1024 * 1024 * 1024;
pub(crate) const UPLOAD_BPS: u32 = 256 * 1024;

#[derive(Clone, Default)]
pub(crate) struct Options {
    pub server: Option<assemble_client::Client>,
    pub require_server: bool,
    pub discovery: Option<Arc<crate::hosted::Discovery>>,
    pub local_test: bool,
    pub network: crate::network::Network,
    pub pool: Option<Arc<tracker_pool::Pool>>,
    pub pool_shared: bool,
    #[cfg(any(test, feature = "test-fixtures"))]
    pub test_peers: Vec<SocketAddr>,
    pub bind_device_name: Option<String>,
    #[cfg(test)]
    pub download_bps: Option<NonZeroU32>,
}
#[derive(Clone)]
pub(crate) struct Prepared {
    pub remote: Option<super::remote::RemoteRecord>,
    pub source: SecretString,
    pub metadata: Vec<u8>,
    pub files: Vec<DownloadFile>,
    pub title: String,
    pub peers: Vec<SocketAddr>,
}

pub(crate) fn source(value: &str) -> Result<SecretString> {
    if value.len() > 16384 || value.chars().any(char::is_control) {
        return Err(DownloadError::InvalidSource);
    }
    let url = Url::parse(value.trim()).map_err(|_| DownloadError::InvalidSource)?;
    if url.scheme() != "magnet" || url.fragment().is_some() || url.host().is_some() {
        return Err(DownloadError::InvalidSource);
    }
    let magnet = librqbit::Magnet::parse(value.trim()).map_err(|_| DownloadError::InvalidSource)?;
    if magnet.as_id20().is_none() {
        return Err(DownloadError::Unsupported);
    }
    Ok(SecretString::new(value.trim()))
}

pub(crate) async fn connection_options(
    source: &SecretString,
    options: &Options,
    cancel: &CancellationToken,
) -> Result<(Vec<String>, Vec<SocketAddr>)> {
    if options.require_server {
        return Err(DownloadError::ServerUnavailable);
    }
    let url = Url::parse(source.expose()).map_err(|_| DownloadError::InvalidSource)?;
    let mut candidates = Vec::new();
    #[cfg(any(test, feature = "test-fixtures"))]
    let mut peers = options.test_peers.clone();
    #[cfg(not(any(test, feature = "test-fixtures")))]
    let mut peers = Vec::new();
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "tr" => {
                if candidates.len() >= 16 {
                    return Err(DownloadError::Limit);
                }
                candidates.push((trackers::parse(&value, false)?, true));
            }
            "x.pe" => {
                if peers.len() >= 32 {
                    return Err(DownloadError::Limit);
                }
                let peer = value
                    .parse::<SocketAddr>()
                    .map_err(|_| DownloadError::InvalidSource)?;
                if peer.port() == 0
                    || (!options.local_test && !crate::network::public_ip(peer.ip()))
                {
                    return Err(DownloadError::InvalidSource);
                }
                peers.push(peer);
            }
            // 不把 xs/as 等可触发任意 HTTP 请求的扩展交给引擎。
            _ => {}
        }
    }
    if let Some(pool) = &options.pool {
        candidates.extend(
            pool.get()
                .into_iter()
                .filter_map(|v| trackers::parse(&v, true).ok())
                .map(|url| (url, false)),
        );
    }
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|(url, _)| seen.insert(url.to_string()));
    let resolving = futures_util::stream::iter(candidates)
        .map(|(url, explicit)| async move {
            match trackers::Endpoint::resolve(url, &options.network, options.local_test).await {
                Ok(endpoint) => Ok(Some(endpoint)),
                Err(DownloadError::InvalidSource) if explicit => Err(DownloadError::InvalidSource),
                // 单个失效候选不阻断 DHT 和其余 Tracker；明确指向私网的用户输入仍拒绝。
                _ => Ok(None),
            }
        })
        .buffer_unordered(8)
        .try_collect::<Vec<_>>();
    let endpoints = tokio::select! { ()=cancel.cancelled()=>return Err(DownloadError::Cancelled), result=resolving=>result? };
    let mut udp = Vec::new();
    let mut http = Vec::new();
    for endpoint in endpoints.into_iter().flatten() {
        if endpoint.url.scheme() == "udp" {
            udp.extend(endpoint.udp_urls()?);
        }
        http.push(endpoint);
    }
    let hash = librqbit::Magnet::parse(source.expose())
        .map_err(|_| DownloadError::InvalidSource)?
        .as_id20()
        .ok_or(DownloadError::Unsupported)?;
    // HTTPS 通过固定地址的客户端短时找节点；引擎持续使用固定公网 IP 的 UDP Tracker。
    peers.extend(trackers::bootstrap(http, &hash.0, cancel).await);
    if let Some(discovery) = &options.discovery {
        let mut known = discovery.peers(hash.0, cancel).await?;
        known.append(&mut peers);
        peers = known;
    }
    if cancel.is_cancelled() {
        return Err(DownloadError::Cancelled);
    }
    let peers = trackers::bounded_peers(peers);
    udp.sort();
    udp.dedup();
    Ok((udp, peers))
}
pub(crate) async fn download_connections(
    source: &SecretString,
    options: &Options,
    cancel: &CancellationToken,
    mut peers: Vec<SocketAddr>,
) -> Result<(Vec<String>, Vec<SocketAddr>)> {
    let (trackers, initial) = connection_options(source, options, cancel).await?;
    peers.extend(initial);
    if options.discovery.is_some() {
        tracing::info!(
            initial_peers = peers.len(),
            "hosted magnet payload nodes ready"
        );
    }
    Ok((trackers, trackers::bounded_peers(peers)))
}

pub(super) struct PayloadSession {
    pub inner: Arc<Session>,
    relay: Option<crate::hosted::PeerRelay>,
}
impl PayloadSession {
    pub async fn stop(self) {
        self.inner.stop().await;
        if let Some(relay) = self.relay {
            relay.shutdown().await;
        }
    }
}
pub(super) async fn payload_session(
    folder: PathBuf,
    cancel: CancellationToken,
    options: &Options,
    source: &SecretString,
    trackers: &mut Vec<String>,
) -> Result<PayloadSession> {
    let hash = librqbit::Magnet::parse(source.expose())
        .map_err(|_| DownloadError::InvalidSource)?
        .as_id20()
        .ok_or(DownloadError::Unsupported)?
        .0;
    let inner = session(folder, cancel.clone(), options).await?;
    let relay = if let Some(discovery) = &options.discovery {
        match crate::hosted::PeerRelay::open(
            Arc::clone(discovery),
            hash,
            options.local_test,
            cancel,
        )
        .await
        {
            Ok(relay) => {
                trackers.push(relay.url.clone());
                Some(relay)
            }
            Err(error) => {
                inner.stop().await;
                return Err(error);
            }
        }
    } else {
        None
    };
    Ok(PayloadSession { inner, relay })
}

pub(super) struct Diagnostics(std::time::Instant);
impl Diagnostics {
    pub fn new() -> Self {
        Self(std::time::Instant::now())
    }
    pub fn observe(&mut self, hosted: bool, stats: &librqbit::TorrentStats) {
        if !hosted || self.0.elapsed() < std::time::Duration::from_secs(10) {
            return;
        }
        self.0 = std::time::Instant::now();
        if let Some(live) = &stats.live {
            let peers = &live.snapshot.peer_stats;
            tracing::info!(
                live_peers = peers.live,
                connecting_peers = peers.connecting,
                dead_peers = peers.dead,
                fetched_bytes = live.snapshot.fetched_bytes,
                checked_bytes = stats.progress_bytes,
                uploaded_bytes = stats.uploaded_bytes,
                "hosted magnet payload progress"
            );
        }
    }
}

pub(crate) async fn session(
    folder: PathBuf,
    cancel: CancellationToken,
    options: &Options,
) -> Result<Arc<Session>> {
    if options.require_server {
        return Err(DownloadError::ServerUnavailable);
    }
    let bootstrap = if options.local_test || options.discovery.is_some() {
        Vec::new()
    } else {
        tokio::select! {
            ()=cancel.cancelled()=>return Err(DownloadError::Cancelled),
            addresses=async { tokio::join!(options.network.addresses("dht.transmissionbt.com",6881,false),options.network.addresses("dht.libtorrent.org",25401,false)) }=>[addresses.0.unwrap_or_default(),addresses.1.unwrap_or_default()].into_iter().flatten().map(|a|a.to_string()).collect(),
        }
    };
    Session::new_with_opts(
        folder,
        SessionOptions {
            bind_device_name: options.bind_device_name.clone(),
            dht: if options.local_test || options.discovery.is_some() {
                None
            } else {
                Some(DhtSessionConfig {
                    persistence: None,
                    bootstrap_addrs: Some(bootstrap),
                    ..DhtSessionConfig::default()
                })
            },
            listen: None,
            persistence: None,
            fastresume: false,
            cancellation_token: Some(cancel),
            peer_limit: Some(64),
            disable_local_service_discovery: true,
            allowlist_url: options.discovery.as_ref().map(|d| d.allowlist_url.clone()),
            ratelimits: librqbit::limits::LimitsConfig {
                upload_bps: NonZeroU32::new(UPLOAD_BPS),
                #[cfg(test)]
                download_bps: options.download_bps,
                #[cfg(not(test))]
                download_bps: None,
            },
            ..SessionOptions::default()
        },
    )
    .await
    .map_err(|_| DownloadError::Network)
}

pub(crate) fn describe(bytes: &[u8]) -> Result<(Vec<u8>, Vec<DownloadFile>, String)> {
    if bytes.len() as u64 > MAX_METADATA {
        return Err(DownloadError::Limit);
    }
    let parsed = librqbit::torrent_from_bytes(bytes).map_err(|_| DownloadError::InvalidMedia)?;
    let raw = parsed.info.raw_bytes.as_ref();
    let validated = parsed
        .info
        .data
        .clone()
        .validate()
        .map_err(|_| DownloadError::InvalidMedia)?;
    if validated.info().private {
        return Err(DownloadError::Unsupported);
    }
    let title = assemble_device::safe_file_name(&validated.name_or_else(|| "Magnet"));
    let mut files = Vec::new();
    let mut names = std::collections::HashSet::new();
    let mut total = 0_u64;
    for (index, file) in validated.iter_file_details().enumerate() {
        if index >= MAX_FILES || file.attrs().symlink {
            return Err(DownloadError::Limit);
        }
        // 展示与恢复契约固定用斜杠，不能把 Windows 的本机分隔符误判为种子中的危险字符。
        let components = file.filename.to_vec();
        if components.iter().any(|part| part.contains(['/', '\\'])) {
            return Err(DownloadError::InvalidMedia);
        }
        let name = components.join("/");
        relative_path(&name)?;
        if !names.insert(name.to_lowercase()) {
            return Err(DownloadError::InvalidMedia);
        }
        total = total.checked_add(file.len).ok_or(DownloadError::Limit)?;
        if total > MAX_TOTAL {
            return Err(DownloadError::Limit);
        }
        if !file.attrs().padding {
            files.push(DownloadFile {
                id: u32::try_from(index).map_err(|_| DownloadError::Limit)?,
                name,
                size: file.len,
            });
        }
    }
    if files.is_empty() {
        return Err(DownloadError::InvalidMedia);
    }
    let mut clean = b"d4:info".to_vec();
    clean.extend_from_slice(raw);
    clean.push(b'e');
    Ok((clean, files, title))
}

pub(crate) async fn prepare(
    value: &str,
    cancel: &CancellationToken,
    options: &Options,
) -> Result<Prepared> {
    let source = source(value)?;
    if let Some(discovery) = &options.discovery {
        return discovery.prepare(source, options, cancel).await;
    }
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(90);
    let (trackers, peers) =
        tokio::time::timeout_at(deadline, connection_options(&source, options, cancel))
            .await
            .map_err(|_| DownloadError::MetadataUnavailable)??;
    let parsed =
        librqbit::Magnet::parse(source.expose()).map_err(|_| DownloadError::InvalidSource)?;
    let magnet = format!(
        "magnet:?xt=urn:btih:{}",
        parsed
            .as_id20()
            .ok_or(DownloadError::Unsupported)?
            .as_string()
    );
    let mut magnet = Url::parse(&magnet).map_err(|_| DownloadError::InvalidSource)?;
    // librqbit 的磁力分支只读 URI 内的 tr，不读取 AddTorrentOptions.trackers。
    for tracker in &trackers {
        magnet.query_pairs_mut().append_pair("tr", tracker);
    }
    let temp = tempfile::tempdir().map_err(|_| DownloadError::Storage)?;
    let engine = session(temp.path().to_path_buf(), cancel.child_token(), options).await?;
    let result = tokio::select! {
        ()=cancel.cancelled()=>Err(DownloadError::Cancelled),
        result=tokio::time::timeout_at(deadline,engine.add_torrent(AddTorrent::from_url(magnet.as_str()),Some(AddTorrentOptions { list_only:true,trackers:Some(trackers),initial_peers:Some(peers),..AddTorrentOptions::default() }))) => match result {
            Ok(Ok(AddTorrentResponse::ListOnly(value))) => describe(&value.torrent_bytes).map(|(metadata,files,title)|Prepared { source,metadata,files,title,peers:value.seen_peers,remote:None }),
            _=>Err(DownloadError::MetadataUnavailable),
        }
    };
    engine.stop().await;
    result
}

pub(crate) fn check_identity(source: &SecretString, bytes: &[u8]) -> Result<()> {
    let magnet =
        librqbit::Magnet::parse(source.expose()).map_err(|_| DownloadError::InvalidSource)?;
    let torrent = librqbit::torrent_from_bytes(bytes).map_err(|_| DownloadError::InvalidMedia)?;
    if magnet.as_id20() != Some(torrent.info_hash) {
        return Err(DownloadError::InvalidMedia);
    }
    Ok(())
}
