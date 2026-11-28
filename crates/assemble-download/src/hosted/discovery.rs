//! 托管服务使用只接受公网地址的 DHT 与元数据读取器，不把任意节点交给元数据隐式连接。
use crate::{DownloadError, Result, magnet::engine};
use assemble_dht::{
    InfoHash,
    network::{Engine, Handle},
    wire,
};
use assemble_store::secret::SecretString;
use futures_util::{StreamExt, stream::FuturesUnordered};
use std::{collections::HashSet, path::Path, sync::Arc, time::Duration};
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

pub(crate) struct Discovery {
    engine: Mutex<Option<Engine>>,
    handle: Handle,
    pub allowlist_url: String,
}
impl Discovery {
    pub async fn open(root: &Path) -> Result<Arc<Self>> {
        tokio::fs::create_dir_all(root)
            .await
            .map_err(|_| DownloadError::Storage)?;
        let path = root.join("public-peers.txt");
        tokio::fs::write(&path, public_ranges())
            .await
            .map_err(|_| DownloadError::Storage)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .await
                .map_err(|_| DownloadError::Storage)?;
        }
        let allowlist_url = url::Url::from_file_path(
            tokio::fs::canonicalize(path)
                .await
                .map_err(|_| DownloadError::Storage)?,
        )
        .map_err(|()| DownloadError::Storage)?
        .to_string();
        let engine = Engine::start(
            "0.0.0.0:0".parse().map_err(|_| DownloadError::Network)?,
            false,
            vec![
                "router.bittorrent.com:6881".into(),
                "dht.transmissionbt.com:6881".into(),
                "dht.libtorrent.org:25401".into(),
            ],
        )
        .await
        .map_err(|_| DownloadError::Network)?;
        Ok(Arc::new(Self {
            handle: engine.handle.clone(),
            engine: Mutex::new(Some(engine)),
            allowlist_url,
        }))
    }
    pub async fn shutdown(&self) {
        if let Some(engine) = self.engine.lock().await.take() {
            engine.shutdown().await;
        }
    }
    pub async fn peers(
        &self,
        hash: InfoHash,
        cancel: &CancellationToken,
    ) -> Result<Vec<std::net::SocketAddr>> {
        let mut incoming = self.handle.subscribe();
        self.handle
            .lookup(hash)
            .await
            .map_err(|_| DownloadError::Network)?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
        let mut peers = Vec::new();
        loop {
            tokio::select! {
                ()=cancel.cancelled()=>return Err(DownloadError::Cancelled),
                ()=tokio::time::sleep_until(deadline)=>break,
                reply=incoming.recv()=>match reply {Ok(peer) if peer.hash==hash && assemble_dht::public_address(peer.address)=>{peers.push(peer.address); if peers.len()>=64 {break;}},Err(tokio::sync::broadcast::error::RecvError::Closed)=>break,_=>{}}
            }
        }
        Ok(crate::magnet::trackers::bounded_peers(peers))
    }
    pub async fn prepare(
        &self,
        source: SecretString,
        options: &engine::Options,
        cancel: &CancellationToken,
    ) -> Result<engine::Prepared> {
        let hash = librqbit::Magnet::parse(source.expose())
            .map_err(|_| DownloadError::InvalidSource)?
            .as_id20()
            .ok_or(DownloadError::Unsupported)?
            .0;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
        let started = std::time::Instant::now();
        let (_, initial) = tokio::time::timeout_at(
            deadline,
            engine::connection_options(&source, options, cancel),
        )
        .await
        .map_err(|_| DownloadError::MetadataUnavailable)??;
        tracing::info!(
            initial_peers = initial.len(),
            "hosted magnet discovery started"
        );
        let mut incoming = self.handle.subscribe();
        self.handle
            .lookup(hash)
            .await
            .map_err(|_| DownloadError::Network)?;
        let (hints, mut hinted) = mpsc::channel(128);
        let mut pending = std::collections::VecDeque::from(initial);
        let mut tried = HashSet::new();
        let mut workers = FuturesUnordered::new();
        loop {
            while workers.len() < 4 && tried.len() < 128 {
                let Some(peer) = pending.pop_front() else {
                    break;
                };
                if !assemble_dht::public_address(peer) || !tried.insert(peer) {
                    continue;
                }
                workers.push(wire::fetch_raw(hash, peer, true, hints.clone()));
            }
            tokio::select! {
                ()=cancel.cancelled()=>return Err(DownloadError::Cancelled),
                ()=tokio::time::sleep_until(deadline)=>{
                    tracing::info!(tried_peers=tried.len(),elapsed_ms=started.elapsed().as_millis(),"hosted magnet metadata timed out");
                    return Err(DownloadError::MetadataUnavailable);
                },
                Some(reply)=workers.next(),if !workers.is_empty()=>{
                    if let Ok((_,raw))=reply {
                        let mut encoded=b"d4:info".to_vec();encoded.extend_from_slice(&raw);encoded.push(b'e');
                        let (metadata,files,title)=engine::describe(&encoded)?;
                        tracing::info!(tried_peers=tried.len(),elapsed_ms=started.elapsed().as_millis(),"hosted magnet metadata resolved");
                        return Ok(engine::Prepared{source,metadata,files,title,peers:tried.into_iter().collect(),remote:None});
                    }
                },
                Some(peer)=hinted.recv()=>{if pending.len()<128 {pending.push_back(peer);}},
                reply=incoming.recv()=>match reply {Ok(peer) if peer.hash==hash && pending.len()<128=>pending.push_back(peer.address),Err(tokio::sync::broadcast::error::RecvError::Closed)=>return Err(DownloadError::MetadataUnavailable),_=>{}}
            }
        }
    }
}

// 使用正向列表：映射 IPv4、回环、内网、保留与文档范围都不进入 BT 数据连接。
fn public_ranges() -> String {
    fn ranges(min: u128, max: u128, mut excluded: Vec<(u128, u128)>) -> Vec<(u128, u128)> {
        excluded.sort_unstable();
        let mut next = min;
        let mut out = Vec::new();
        for (start, end) in excluded {
            if start > next {
                out.push((next, start - 1));
            }
            next = next.max(end + 1);
        }
        if next <= max {
            out.push((next, max));
        }
        out
    }
    let ip4 = |s: &str| {
        u128::from(
            s.parse::<std::net::Ipv4Addr>()
                .expect("fixed IPv4")
                .to_bits(),
        )
    };
    let ip6 = |s: &str| {
        s.parse::<std::net::Ipv6Addr>()
            .expect("fixed IPv6")
            .to_bits()
    };
    let excluded = [
        ("10.0.0.0", "10.255.255.255"),
        ("100.64.0.0", "100.127.255.255"),
        ("127.0.0.0", "127.255.255.255"),
        ("169.254.0.0", "169.254.255.255"),
        ("172.16.0.0", "172.31.255.255"),
        ("192.0.0.0", "192.0.0.255"),
        ("192.0.2.0", "192.0.2.255"),
        ("192.88.99.0", "192.88.99.255"),
        ("192.168.0.0", "192.168.255.255"),
        ("198.18.0.0", "198.19.255.255"),
        ("198.51.100.0", "198.51.100.255"),
        ("203.0.113.0", "203.0.113.255"),
    ];
    let mut text = String::new();
    for (start, end) in ranges(
        ip4("1.0.0.0"),
        ip4("223.255.255.255"),
        excluded
            .into_iter()
            .map(|(a, b)| (ip4(a), ip4(b)))
            .collect(),
    ) {
        use std::fmt::Write;
        writeln!(
            &mut text,
            "public4:{}-{}",
            std::net::Ipv4Addr::from_bits(u32::try_from(start).expect("IPv4 range")),
            std::net::Ipv4Addr::from_bits(u32::try_from(end).expect("IPv4 range"))
        )
        .expect("string write");
    }
    let excluded = [
        ("2001::", "2001:1ff:ffff:ffff:ffff:ffff:ffff:ffff"),
        ("2001:db8::", "2001:db8:ffff:ffff:ffff:ffff:ffff:ffff"),
        ("2002::", "2002:ffff:ffff:ffff:ffff:ffff:ffff:ffff"),
        ("3fff::", "3fff:fff:ffff:ffff:ffff:ffff:ffff:ffff"),
    ];
    for (start, end) in ranges(
        ip6("2000::"),
        ip6("3fff:ffff:ffff:ffff:ffff:ffff:ffff:ffff"),
        excluded
            .into_iter()
            .map(|(a, b)| (ip6(a), ip6(b)))
            .collect(),
    ) {
        use std::fmt::Write;
        writeln!(
            &mut text,
            "public6:{}-{}",
            std::net::Ipv6Addr::from_bits(start),
            std::net::Ipv6Addr::from_bits(end)
        )
        .expect("string write");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn payload_allowlist_excludes_private_mapped_and_documentation_ranges() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("allowlist.txt");
        tokio::fs::write(&path, public_ranges()).await.unwrap();
        let session = librqbit::Session::new_with_opts(
            dir.path().join("data"),
            librqbit::SessionOptions {
                dht: None,
                listen: None,
                persistence: None,
                disable_local_service_discovery: true,
                allowlist_url: Some(url::Url::from_file_path(path).unwrap().into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let allowed = session.allowlist.as_ref().unwrap();
        for ip in ["1.1.1.1", "8.8.8.8", "2606:4700:4700::1111"] {
            assert!(allowed.has(ip.parse().unwrap()), "{ip}");
        }
        for ip in [
            "0.1.2.3",
            "10.0.0.1",
            "127.0.0.1",
            "100.64.0.1",
            "169.254.169.254",
            "172.16.0.1",
            "192.168.1.1",
            "192.0.2.1",
            "198.18.1.1",
            "224.0.0.1",
            "255.255.255.255",
            "::1",
            "::ffff:127.0.0.1",
            "::ffff:8.8.8.8",
            "fc00::1",
            "2001:db8::1",
            "2002::1",
            "3fff::1",
        ] {
            assert!(!allowed.has(ip.parse().unwrap()), "{ip}");
        }
        session.stop().await;
    }
}
