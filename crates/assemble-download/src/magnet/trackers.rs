//! Tracker 的解析与实际连接用同一批公网地址；HTTP 只作短时节点引导，不交给引擎重新解析。
use crate::{
    DownloadError, Result,
    network::{Network, public_ip},
};
use futures_util::{StreamExt, stream};
use librqbit_bencode::BencodeValue;
use std::{
    fmt::Write,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    time::Duration,
};
use tokio_util::sync::CancellationToken;
use url::Url;

type Value<'a> = BencodeValue<&'a [u8]>;

#[derive(Clone)]
pub(super) struct Endpoint {
    pub url: Url,
    pub addresses: Vec<SocketAddr>,
    local_test: bool,
}

pub(super) fn parse(value: &str, pool: bool) -> Result<Url> {
    if value.len() > 2048 || value.chars().any(char::is_control) {
        return Err(DownloadError::InvalidSource);
    }
    let url = Url::parse(value).map_err(|_| DownloadError::InvalidSource)?;
    if !matches!(url.scheme(), "http" | "https" | "udp")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.host_str().is_none()
        || url.port_or_known_default().is_none_or(|p| p == 0)
        || (pool && url.query().is_some())
    {
        return Err(DownloadError::InvalidSource);
    }
    Ok(url)
}

impl Endpoint {
    pub async fn resolve(url: Url, network: &Network, local_test: bool) -> Result<Self> {
        let host = url
            .host_str()
            .ok_or(DownloadError::InvalidSource)?
            .trim_matches(['[', ']']);
        let addresses = network
            .addresses(
                host,
                url.port_or_known_default()
                    .ok_or(DownloadError::InvalidSource)?,
                local_test,
            )
            .await?;
        Ok(Self {
            url,
            addresses,
            local_test,
        })
    }
    pub fn udp_url(&self) -> Result<String> {
        let mut url = self.url.clone();
        let addr = self
            .addresses
            .iter()
            .find(|a| a.is_ipv4())
            .or(self.addresses.first())
            .ok_or(DownloadError::Network)?;
        url.set_ip_host(addr.ip())
            .map_err(|()| DownloadError::InvalidSource)?;
        Ok(url.to_string())
    }
    fn client(&self) -> Result<reqwest::Client> {
        reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .resolve_to_addrs(
                self.url.host_str().ok_or(DownloadError::InvalidSource)?,
                &self.addresses,
            )
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(3))
            .build()
            .map_err(|_| DownloadError::Network)
    }
    async fn body(&self, url: Url) -> Result<Vec<u8>> {
        let mut response = self
            .client()?
            .get(url)
            .send()
            .await
            .map_err(|_| DownloadError::Network)?;
        if response.status().is_redirection() {
            return Err(DownloadError::Network);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| DownloadError::Network)? {
            if bytes.len() + chunk.len() > 65536 {
                return Err(DownloadError::Limit);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
    pub async fn healthy(&self) -> bool {
        if self.url.scheme() == "udp" {
            return self.udp_connect().await.is_ok();
        }
        let Ok(bytes) = self.body(self.url.clone()).await else {
            return false;
        };
        let Ok(Value::Dict(value)) = librqbit_bencode::from_bytes::<Value<'_>>(&bytes) else {
            return false;
        };
        if let Some(Value::Bytes(reason)) = value.get(b"failure reason".as_slice()) {
            let reason = String::from_utf8_lossy(reason).to_ascii_lowercase();
            return reason.contains("info_hash")
                || reason.contains("infohash")
                || reason.contains("peer_id");
        }
        value.contains_key(b"peers".as_slice())
    }
    async fn udp_connect(&self) -> Result<()> {
        let addr = self
            .addresses
            .iter()
            .find(|a| a.is_ipv4())
            .or(self.addresses.first())
            .ok_or(DownloadError::Network)?;
        let bind = if addr.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        };
        let socket = tokio::net::UdpSocket::bind(bind)
            .await
            .map_err(|_| DownloadError::Network)?;
        socket
            .connect(addr)
            .await
            .map_err(|_| DownloadError::Network)?;
        let mut request = [0_u8; 16];
        request[..8].copy_from_slice(&0x0417_2710_1980_u64.to_be_bytes());
        getrandom::fill(&mut request[12..]).map_err(|_| DownloadError::Network)?;
        socket
            .send(&request)
            .await
            .map_err(|_| DownloadError::Network)?;
        let mut response = [0_u8; 256];
        let size = tokio::time::timeout(Duration::from_secs(3), socket.recv(&mut response))
            .await
            .map_err(|_| DownloadError::Network)?
            .map_err(|_| DownloadError::Network)?;
        if size < 16 || response[..4] != [0; 4] || response[4..8] != request[12..] {
            return Err(DownloadError::Network);
        }
        Ok(())
    }
    async fn peers(&self, hash: &[u8; 20], cancel: &CancellationToken) -> Vec<SocketAddr> {
        if cancel.is_cancelled() {
            return Vec::new();
        }
        let mut peer = [0_u8; 20];
        if getrandom::fill(&mut peer).is_err() {
            return Vec::new();
        }
        peer[..8].copy_from_slice(b"-AS0100-");
        let mut url = self.url.clone();
        let mut base = url.clone();
        base.set_query(None);
        for (key, value) in url.query_pairs().filter(|(key, _)| {
            !matches!(
                key.as_ref(),
                "info_hash"
                    | "peer_id"
                    | "port"
                    | "uploaded"
                    | "downloaded"
                    | "left"
                    | "compact"
                    | "numwant"
                    | "event"
            )
        }) {
            base.query_pairs_mut().append_pair(&key, &value);
        }
        let mut query = base.query().map_or_else(String::new, |q| format!("{q}&"));
        query.push_str("info_hash=");
        for byte in hash {
            let _ = write!(query, "%{byte:02X}");
        }
        query.push_str("&peer_id=");
        for byte in peer {
            let _ = write!(query, "%{byte:02X}");
        }
        // 元数据尚未知，left 不能为 0；这个短时引导会话不宣称自己已完成资源。
        query.push_str(
            "&port=4240&uploaded=0&downloaded=0&left=1&compact=1&numwant=64&event=started",
        );
        url.set_query(Some(&query));
        let bytes = tokio::select! { ()=cancel.cancelled()=>Err(DownloadError::Cancelled), result=self.body(url.clone())=>result };
        // 引导查询也成对停止，不让 Tracker 长时间保留一个不接受入站的临时节点。
        let stop = query
            .strip_suffix("started")
            .map_or_default(|q| format!("{q}stopped"));
        url.set_query(Some(&stop));
        let _ = tokio::time::timeout(Duration::from_secs(1), self.body(url)).await;
        bytes
            .ok()
            .and_then(|b| decode_peers(&b, self.local_test).ok())
            .unwrap_or_default()
    }
}

pub(super) async fn bootstrap(
    endpoints: Vec<Endpoint>,
    hash: &[u8; 20],
    cancel: &CancellationToken,
) -> Vec<SocketAddr> {
    let groups = stream::iter(endpoints)
        .map(|endpoint| async move { endpoint.peers(hash, cancel).await })
        .buffer_unordered(8)
        .collect::<Vec<_>>()
        .await;
    let mut peers: Vec<_> = groups.into_iter().flatten().collect();
    peers.sort_unstable();
    peers.dedup();
    peers.truncate(128);
    peers
}

fn decode_peers(bytes: &[u8], local_test: bool) -> Result<Vec<SocketAddr>> {
    let Value::Dict(value) = librqbit_bencode::from_bytes::<Value<'_>>(bytes)
        .map_err(|_| DownloadError::InvalidMedia)?
    else {
        return Err(DownloadError::InvalidMedia);
    };
    if value.contains_key(b"failure reason".as_slice()) {
        return Err(DownloadError::Network);
    }
    let mut peers = Vec::new();
    for (key, length) in [(b"peers".as_slice(), 6), (b"peers6".as_slice(), 18)] {
        match value.get(key) {
            Some(Value::Bytes(bytes)) => {
                if bytes.len() % length != 0 {
                    return Err(DownloadError::InvalidMedia);
                }
                for entry in bytes.chunks_exact(length).take(128) {
                    let ip = if length == 6 {
                        IpAddr::V4(Ipv4Addr::new(entry[0], entry[1], entry[2], entry[3]))
                    } else {
                        IpAddr::V6(Ipv6Addr::from(
                            <[u8; 16]>::try_from(&entry[..16])
                                .map_err(|_| DownloadError::InvalidMedia)?,
                        ))
                    };
                    peers.push(SocketAddr::new(
                        ip,
                        u16::from_be_bytes([entry[length - 2], entry[length - 1]]),
                    ));
                }
            }
            Some(Value::List(entries)) => {
                for entry in entries.iter().take(128) {
                    let Value::Dict(entry) = entry else {
                        continue;
                    };
                    if let (Some(Value::Bytes(ip)), Some(Value::Integer(port))) =
                        (entry.get(b"ip".as_slice()), entry.get(b"port".as_slice()))
                        && let (Ok(ip), Ok(port)) = (
                            std::str::from_utf8(ip).unwrap_or("").parse(),
                            u16::try_from(*port),
                        )
                    {
                        peers.push(SocketAddr::new(ip, port));
                    }
                }
            }
            _ => {}
        }
    }
    peers.retain(|peer| {
        peer.port() != 0 && (public_ip(peer.ip()) || (local_test && peer.ip().is_loopback()))
    });
    peers.truncate(128);
    Ok(peers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    #[tokio::test]
    async fn http_discovery_pins_connection_preserves_host_and_stops_on_cancel() {
        let stopped = Arc::new(AtomicUsize::new(0));
        let stopped_server = Arc::clone(&stopped);
        let entered = Arc::new(tokio::sync::Notify::new());
        let entered_server = Arc::clone(&entered);
        let app = axum::Router::new().route(
            "/announce",
            axum::routing::get(move |request: axum::extract::Request| {
                let stopped = Arc::clone(&stopped_server);
                let entered = Arc::clone(&entered_server);
                async move {
                    assert!(
                        request
                            .headers()
                            .get("host")
                            .unwrap()
                            .to_str()
                            .unwrap()
                            .starts_with("tracker.invalid:")
                    );
                    let query = request.uri().query().unwrap_or("");
                    assert!(!query.contains("wrong"));
                    if query.contains("event=stopped") {
                        stopped.fetch_add(1, Ordering::SeqCst);
                    } else {
                        assert!(query.contains(&format!("info_hash={}", "%00".repeat(20))));
                        if query.contains("slow=1") {
                            entered.notify_one();
                            tokio::time::sleep(Duration::from_secs(30)).await;
                        }
                    }
                    let mut body = b"d8:intervali60e5:peers6:".to_vec();
                    body.extend_from_slice(&[8, 8, 8, 8, 0x1a, 0xe1]);
                    body.push(b'e');
                    body
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = Endpoint {
            url: Url::parse(&format!(
                "http://tracker.invalid:{}/announce?info_hash=wrong",
                listener.local_addr().unwrap().port()
            ))
            .unwrap(),
            addresses: vec![listener.local_addr().unwrap()],
            local_test: false,
        };
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        assert_eq!(
            endpoint.peers(&[0; 20], &CancellationToken::new()).await,
            vec!["8.8.8.8:6881".parse().unwrap()]
        );
        assert_eq!(stopped.load(Ordering::SeqCst), 1);
        let mut slow = endpoint;
        slow.url.set_query(Some("slow=1"));
        let cancel = CancellationToken::new();
        let signal = cancel.clone();
        let cancel_task = tokio::spawn(async move {
            entered.notified().await;
            signal.cancel();
        });
        assert!(slow.peers(&[0; 20], &cancel).await.is_empty());
        cancel_task.await.unwrap();
        assert_eq!(stopped.load(Ordering::SeqCst), 2);
        server.abort();
        let _ = server.await;
    }
    #[test]
    fn parses_compact_peers_and_filters_private_addresses() {
        let mut body = b"d8:intervali60e5:peers12:".to_vec();
        body.extend_from_slice(&[8, 8, 8, 8, 0x1a, 0xe1, 127, 0, 0, 1, 0x1a, 0xe1]);
        body.push(b'e');
        assert_eq!(
            decode_peers(&body, false).unwrap(),
            vec!["8.8.8.8:6881".parse().unwrap()]
        );
        assert_eq!(decode_peers(&body, true).unwrap().len(), 2);
        assert!(decode_peers(b"d5:peers1:xe", false).is_err());
        assert!(parse("https://example.com/announce?passkey=secret", true).is_err());
    }
}
