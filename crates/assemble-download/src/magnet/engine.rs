//! BT 引擎只拿 Rust 内部来源；关闭它自己的历史和局域网发现，恢复资料由本模块管理。
use super::files::relative_path;
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

pub(super) const MAX_FILES: usize = 10_000;
pub(super) const MAX_TOTAL: u64 = 1024 * 1024 * 1024 * 1024;
pub(super) const MAX_SELECTED: u64 = 100 * 1024 * 1024 * 1024;
pub(super) const UPLOAD_BPS: u32 = 256 * 1024;

#[derive(Clone, Default)]
pub(super) struct Options {
    pub local_test: bool,
}
#[derive(Clone)]
pub(super) struct Prepared {
    pub source: SecretString,
    pub metadata: Vec<u8>,
    pub files: Vec<DownloadFile>,
    pub title: String,
    pub peers: Vec<SocketAddr>,
}

pub(super) fn source(value: &str) -> Result<SecretString> {
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

pub(super) async fn connection_options(
    source: &SecretString,
    options: &Options,
) -> Result<(Vec<String>, Vec<SocketAddr>)> {
    let url = Url::parse(source.expose()).map_err(|_| DownloadError::InvalidSource)?;
    let mut trackers = Vec::new();
    let mut peers = Vec::new();
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "tr" => {
                if trackers.len() >= 16 {
                    return Err(DownloadError::Limit);
                }
                let tracker = Url::parse(&value).map_err(|_| DownloadError::InvalidSource)?;
                if !matches!(tracker.scheme(), "http" | "https" | "udp")
                    || !tracker.username().is_empty()
                    || tracker.password().is_some()
                    || tracker.fragment().is_some()
                {
                    return Err(DownloadError::InvalidSource);
                }
                tracker.host_str().ok_or(DownloadError::InvalidSource)?;
                tracker
                    .port_or_known_default()
                    .ok_or(DownloadError::InvalidSource)?;
                trackers.push(tracker);
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
    // 坏掉的 Tracker 不应让仍可通过 DHT 找到的资源整体失败；同时限制 DNS 等待总时长。
    let trackers = futures_util::stream::iter(trackers)
        .map(|tracker| async move {
            let host = tracker.host_str().ok_or(DownloadError::InvalidSource)?.trim_matches(['[',']']);
            let port = tracker
                .port_or_known_default()
                .ok_or(DownloadError::InvalidSource)?;
            let addresses = match tokio::time::timeout(
                std::time::Duration::from_secs(3),
                tokio::net::lookup_host((host, port)),
            )
            .await
            {
                Ok(Ok(addresses)) => addresses.collect::<Vec<_>>(),
                _ => return Ok(None),
            };
            // 引擎不能固定 HTTP Tracker 的 DNS；Fake-IP 来源跳过该 Tracker，继续用 DHT，不能当作公网放行。
            if host.parse::<std::net::IpAddr>().is_err() && !addresses.is_empty() && addresses.iter().all(|a|matches!(a.ip(),std::net::IpAddr::V4(ip) if ip.octets()[0]==198 && matches!(ip.octets()[1],18|19))) { return Ok(None); }
            if addresses
                .iter()
                .any(|a| !options.local_test && !crate::network::public_ip(a.ip()))
            {
                return Err(DownloadError::InvalidSource);
            }
            Ok((!addresses.is_empty()).then(|| tracker.to_string()))
        })
        .buffered(8)
        .try_collect::<Vec<_>>()
        .await?
        .into_iter()
        .flatten()
        .collect();
    Ok((trackers, peers))
}
pub(super) async fn session(
    folder: PathBuf,
    cancel: CancellationToken,
    options: &Options,
) -> Result<Arc<Session>> {
    Session::new_with_opts(
        folder,
        SessionOptions {
            dht: if options.local_test {
                None
            } else {
                Some(DhtSessionConfig {
                    persistence: None,
                    ..DhtSessionConfig::default()
                })
            },
            listen: None,
            persistence: None,
            fastresume: false,
            cancellation_token: Some(cancel),
            peer_limit: Some(64),
            disable_local_service_discovery: true,
            ratelimits: librqbit::limits::LimitsConfig {
                upload_bps: NonZeroU32::new(UPLOAD_BPS),
                download_bps: None,
            },
            ..SessionOptions::default()
        },
    )
    .await
    .map_err(|_| DownloadError::Network)
}

pub(super) fn describe(bytes: &[u8]) -> Result<(Vec<u8>, Vec<DownloadFile>, String)> {
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

pub(super) async fn prepare(
    value: &str,
    cancel: &CancellationToken,
    options: &Options,
) -> Result<Prepared> {
    let source = source(value)?;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(90);
    let (trackers, peers) = tokio::select! { ()=cancel.cancelled()=>return Err(DownloadError::Cancelled), result=tokio::time::timeout_at(deadline, connection_options(&source,options))=>result.map_err(|_|DownloadError::MetadataUnavailable)?? };
    let parsed =
        librqbit::Magnet::parse(source.expose()).map_err(|_| DownloadError::InvalidSource)?;
    let magnet = format!(
        "magnet:?xt=urn:btih:{}",
        parsed
            .as_id20()
            .ok_or(DownloadError::Unsupported)?
            .as_string()
    );
    let temp = tempfile::tempdir().map_err(|_| DownloadError::Storage)?;
    let engine = session(temp.path().to_path_buf(), cancel.child_token(), options).await?;
    let result = tokio::select! {
        ()=cancel.cancelled()=>Err(DownloadError::Cancelled),
        result=tokio::time::timeout_at(deadline,engine.add_torrent(AddTorrent::from_url(&magnet),Some(AddTorrentOptions { list_only:true,trackers:Some(trackers),initial_peers:Some(peers),..AddTorrentOptions::default() }))) => match result {
            Ok(Ok(AddTorrentResponse::ListOnly(value))) => describe(&value.torrent_bytes).map(|(metadata,files,title)|Prepared { source,metadata,files,title,peers:value.seen_peers }),
            _=>Err(DownloadError::MetadataUnavailable),
        }
    };
    engine.stop().await;
    result
}

pub(super) fn check_identity(source: &SecretString, bytes: &[u8]) -> Result<()> {
    let magnet =
        librqbit::Magnet::parse(source.expose()).map_err(|_| DownloadError::InvalidSource)?;
    let torrent = librqbit::torrent_from_bytes(bytes).map_err(|_| DownloadError::InvalidMedia)?;
    if magnet.as_id20() != Some(torrent.info_hash) {
        return Err(DownloadError::InvalidMedia);
    }
    Ok(())
}
