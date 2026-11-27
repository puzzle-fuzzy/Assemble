//! 所有重定向、清单与分片都走同一个网络边界；不让远程清单探测本机网络。

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use reqwest::header::{HeaderMap, HeaderValue, LOCATION, RANGE, REFERER};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::{DownloadError, Result};

type DnsCache = Arc<Mutex<HashMap<String, (Instant, Vec<IpAddr>)>>>;

#[derive(Clone, Default)]
pub(crate) struct Network {
    resolved: DnsCache,
    #[cfg(test)]
    pub(crate) test_origin: Option<String>,
}

pub(crate) fn source_url(value: &str) -> Result<Url> {
    if value.len() > 16_384 || value.chars().any(char::is_control) {
        return Err(DownloadError::InvalidSource);
    }
    let mut url = Url::parse(value).map_err(|_| DownloadError::InvalidSource)?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.host_str().is_none()
    {
        return Err(DownloadError::InvalidSource);
    }
    url.set_fragment(None);
    Ok(url)
}

pub(crate) fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(matches!(a, 0 | 10 | 127 | 224..=255)
                || (a == 100 && (64..=127).contains(&b))
                || (a == 169 && b == 254)
                || (a == 172 && (16..=31).contains(&b))
                || (a == 192 && (b == 168 || (b == 0 && matches!(c, 0 | 2))))
                || (a == 198 && (matches!(b, 18 | 19) || (b == 51 && c == 100)))
                || (a == 203 && b == 0 && c == 113))
        }
        IpAddr::V6(ip) => {
            if let Some(mapped) = ip.to_ipv4_mapped() {
                return public_ip(IpAddr::V4(mapped));
            }
            let words = ip.segments();
            // 只接受全球单播，拒绝本机、链路本地、ULA、组播和文档/隧道特殊范围。
            (words[0] & 0xe000) == 0x2000
                && words[0] != 0x2002
                && !(words[0] == 0x2001 && (words[1] < 0x0200 || words[1] == 0x0db8))
        }
    }
}

pub(crate) fn fake_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.octets()[0] == 198 && matches!(ip.octets()[1], 18 | 19),
        IpAddr::V6(ip) => ip.segments()[..3] == [0x2001, 2, 0],
    }
}

fn needs_real_dns(host: &str, addresses: &[SocketAddr]) -> bool {
    host.parse::<IpAddr>().is_err()
        && !addresses.is_empty()
        && addresses.iter().all(|a| fake_ip(a.ip()))
}

fn dns_answers(body: &[u8]) -> Result<Vec<IpAddr>> {
    let value: serde_json::Value =
        serde_json::from_slice(body).map_err(|_| DownloadError::Network)?;
    if value.get("Status").and_then(serde_json::Value::as_u64) != Some(0) {
        return Err(DownloadError::Network);
    }
    let mut addresses = Vec::new();
    for answer in value
        .get("Answer")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        if matches!(
            answer.get("type").and_then(serde_json::Value::as_u64),
            Some(1 | 28)
        ) {
            let ip = answer
                .get("data")
                .and_then(serde_json::Value::as_str)
                .and_then(|v| v.parse().ok())
                .ok_or(DownloadError::Network)?;
            if !public_ip(ip) {
                return Err(DownloadError::InvalidSource);
            }
            addresses.push(ip);
        }
    }
    Ok(addresses)
}

async fn doh_addresses(client: &reqwest::Client, host: &str, record: &str) -> Result<Vec<IpAddr>> {
    let mut url =
        Url::parse("https://cloudflare-dns.com/dns-query").map_err(|_| DownloadError::Network)?;
    url.query_pairs_mut()
        .append_pair("name", host)
        .append_pair("type", record);
    let mut response = client
        .get(url)
        .header("accept", "application/dns-json")
        .send()
        .await
        .map_err(|_| DownloadError::Network)?;
    if !response.status().is_success() {
        return Err(DownloadError::Network);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| DownloadError::Network)? {
        if body.len() + chunk.len() > 65_536 {
            return Err(DownloadError::Limit);
        }
        body.extend_from_slice(&chunk);
    }
    dns_answers(&body)
}
fn merge_dns(v4: Result<Vec<IpAddr>>, v6: Result<Vec<IpAddr>>) -> Result<Vec<IpAddr>> {
    let mut addresses = Vec::new();
    for result in [v4, v6] {
        match result {
            Ok(ips) => addresses.extend(ips),
            Err(DownloadError::InvalidSource) => return Err(DownloadError::InvalidSource),
            Err(DownloadError::Limit) => return Err(DownloadError::Limit),
            _ => {}
        }
    }
    addresses.sort_unstable();
    addresses.dedup();
    if addresses.is_empty() {
        return Err(DownloadError::Network);
    }
    Ok(addresses)
}

impl Network {
    #[cfg(test)]
    pub(crate) fn test(origin: String) -> Self {
        Self {
            test_origin: Some(origin),
            ..Self::default()
        }
    }
    /// 仅处理代理软件的 Fake-IP 网段；真实私网解析仍直接拒绝，不能借回退访问私网。
    async fn real_addresses(&self, host: &str) -> Result<Vec<IpAddr>> {
        if let Some((at, ips)) = self
            .resolved
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(host)
            .cloned()
            && at.elapsed() < Duration::from_secs(30)
        {
            return Ok(ips);
        }
        let bootstrap = SocketAddr::from(([1, 1, 1, 1], 443));
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .resolve("cloudflare-dns.com", bootstrap)
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| DownloadError::Network)?;
        // 两种地址族独立查询；A 成功不能截断 AAAA，否则会永远漏掉 IPv6 节点。
        let (v4, v6) = tokio::join!(
            doh_addresses(&client, host, "A"),
            doh_addresses(&client, host, "AAAA")
        );
        let addresses = merge_dns(v4, v6)?;
        let mut cache = self
            .resolved
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if cache.len() >= 128 {
            cache.clear();
        }
        cache.insert(host.to_owned(), (Instant::now(), addresses.clone()));
        Ok(addresses)
    }

    pub(crate) async fn addresses(
        &self,
        host: &str,
        port: u16,
        local_test: bool,
    ) -> Result<Vec<SocketAddr>> {
        let mut addresses: Vec<_> = tokio::time::timeout(
            Duration::from_secs(3),
            tokio::net::lookup_host((host, port)),
        )
        .await
        .map_err(|_| DownloadError::Network)?
        .map_err(|_| DownloadError::Network)?
        .collect();
        // 混合的 v4/v6 Fake-IP 仍只触发真实公网解析，绝不作为可连接地址直接放行。
        if !local_test && needs_real_dns(host, &addresses) {
            addresses = tokio::time::timeout(Duration::from_secs(4), self.real_addresses(host))
                .await
                .map_err(|_| DownloadError::Network)??
                .into_iter()
                .map(|ip| SocketAddr::new(ip, port))
                .collect();
        }
        if addresses.is_empty()
            || addresses
                .iter()
                .any(|a| !(public_ip(a.ip()) || (local_test && a.ip().is_loopback())))
        {
            return Err(DownloadError::InvalidSource);
        }
        addresses.sort_unstable();
        addresses.dedup();
        Ok(addresses)
    }

    async fn client(&self, url: &Url) -> Result<reqwest::Client> {
        let host = url
            .host_str()
            .ok_or(DownloadError::InvalidSource)?
            .trim_matches(['[', ']']);
        let port = url
            .port_or_known_default()
            .ok_or(DownloadError::InvalidSource)?;
        let local_test = {
            #[cfg(test)]
            {
                self.test_origin
                    .as_ref()
                    .is_some_and(|origin| *origin == url.origin().ascii_serialization())
            }
            #[cfg(not(test))]
            {
                false
            }
        };
        if !local_test && !matches!((url.scheme(), port), ("http", 80) | ("https", 443)) {
            return Err(DownloadError::InvalidSource);
        }
        let addresses = self.addresses(host, port, local_test).await?;
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            // 代理会代替本机解析目标，破坏连接固定保证；此独立下载客户端不继承环境代理。
            .no_proxy()
            .resolve_to_addrs(host, &addresses)
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(3600))
            .build()
            .map_err(|_| DownloadError::Network)
    }

    pub(crate) async fn fetch(
        &self,
        url: &Url,
        headers: &HeaderMap,
        range: Option<(u64, u64)>,
        post: Option<&str>,
        cancel: &CancellationToken,
    ) -> Result<(Url, reqwest::Response)> {
        if range.is_some_and(|(_, length)| length == 0) {
            return Err(DownloadError::InvalidMedia);
        }
        let mut url = source_url(url.as_str())?;
        for hop in 0..=5 {
            let client = tokio::select! { () = cancel.cancelled() => return Err(DownloadError::Cancelled), value = self.client(&url) => value? };
            let mut request = if hop == 0 && post.is_some() {
                client
                    .post(url.clone())
                    .header("content-type", "application/json")
                    .body(post.unwrap_or_default().to_owned())
            } else {
                client.get(url.clone())
            };
            request = request.header("accept-encoding", "identity").header("user-agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/130.0.0.0 Safari/537.36");
            for (name, value) in headers {
                // 匿名 Cookie 只交给抖音本身，绝不跟着重定向发给 CDN 或任意其他站点。
                if name == "cookie"
                    && !(url.scheme() == "https" && url.host_str() == Some("www.douyin.com"))
                {
                    continue;
                }
                request = request.header(name, value);
            }
            if let Some((offset, length)) = range {
                request = request.header(
                    RANGE,
                    format!(
                        "bytes={offset}-{}",
                        offset
                            .checked_add(length)
                            .and_then(|v| v.checked_sub(1))
                            .ok_or(DownloadError::InvalidMedia)?
                    ),
                );
            }
            let response = tokio::select! { () = cancel.cancelled() => return Err(DownloadError::Cancelled), value = tokio::time::timeout(Duration::from_secs(30),request.send()) => value.map_err(|_|DownloadError::Network)?.map_err(|_|DownloadError::Network)? };
            if response.status().is_redirection() {
                if hop == 5 {
                    return Err(DownloadError::Network);
                }
                let location = response
                    .headers()
                    .get(LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or(DownloadError::Network)?;
                url = source_url(
                    url.join(location)
                        .map_err(|_| DownloadError::InvalidSource)?
                        .as_str(),
                )?;
                continue;
            }
            if !response.status().is_success() {
                return Err(DownloadError::SourceUnavailable);
            }
            if response
                .headers()
                .get("content-encoding")
                .is_some_and(|v| v != "identity")
            {
                return Err(DownloadError::InvalidMedia);
            }
            if range.is_some() && response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
                return Err(DownloadError::InvalidMedia);
            }
            if let Some((offset, length)) = range {
                let expected = format!("bytes {offset}-{}/", offset + length - 1);
                if !response
                    .headers()
                    .get("content-range")
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|value| value.starts_with(&expected))
                {
                    return Err(DownloadError::InvalidMedia);
                }
            }
            return Ok((url, response));
        }
        Err(DownloadError::Network)
    }

    pub(crate) async fn bytes(
        &self,
        url: &Url,
        headers: &HeaderMap,
        maximum: u64,
        cancel: &CancellationToken,
    ) -> Result<(Url, Vec<u8>)> {
        let (url, mut response) = self.fetch(url, headers, None, None, cancel).await?;
        if response.content_length().is_some_and(|size| size > maximum) {
            return Err(DownloadError::Limit);
        }
        let mut bytes = Vec::new();
        loop {
            let chunk = tokio::select! { () = cancel.cancelled() => return Err(DownloadError::Cancelled), value = response.chunk() => value.map_err(|_|DownloadError::Network)? };
            let Some(chunk) = chunk else {
                break;
            };
            if (bytes.len() as u64).saturating_add(chunk.len() as u64) > maximum {
                return Err(DownloadError::Limit);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok((url, bytes))
    }

    pub(crate) async fn download(
        &self,
        url: &Url,
        headers: &HeaderMap,
        range: Option<(u64, u64)>,
        path: &Path,
        cancel: &CancellationToken,
        progress: &(dyn Fn(u64, Option<u64>) + Sync),
    ) -> Result<u64> {
        let (_, mut response) = self.fetch(url, headers, range, None, cancel).await?;
        let declared = response.content_length();
        let expected = range.map(|(_, length)| length).or(declared);
        if expected.is_some_and(|size| size > crate::MAX_BYTES || size == 0) {
            return Err(DownloadError::Limit);
        }
        if range.is_some() && declared.is_some() && declared != expected {
            return Err(DownloadError::InvalidMedia);
        }
        // 知道要写多少：先看磁盘放不放得下，不够就别开了头。
        crate::space::ensure_room(path, expected.unwrap_or(0))?;
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .await
            .map_err(|error| crate::space::write_failure(&error))?;
        let mut received = 0_u64;
        loop {
            let chunk = tokio::select! { () = cancel.cancelled() => return Err(DownloadError::Cancelled), value = response.chunk() => value.map_err(|_|DownloadError::Network)? };
            let Some(chunk) = chunk else {
                break;
            };
            received = received
                .checked_add(chunk.len() as u64)
                .ok_or(DownloadError::Limit)?;
            if received > crate::MAX_BYTES || expected.is_some_and(|size| received > size) {
                return Err(DownloadError::Limit);
            }
            // 写不进去多半是磁盘满了：那要说成空间不够，不是笼统的"存储失败"。
            file.write_all(&chunk)
                .await
                .map_err(|error| crate::space::write_failure(&error))?;
            progress(chunk.len() as u64, expected);
        }
        if received == 0 || expected.is_some_and(|size| size != received) {
            return Err(DownloadError::InvalidMedia);
        }
        file.sync_all()
            .await
            .map_err(|error| crate::space::write_failure(&error))?;
        Ok(received)
    }
}

pub(crate) fn headers(page: Option<&str>) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    if let Some(page) = page.filter(|v| !v.is_empty()) {
        let page = source_url(page)?;
        headers.insert(
            REFERER,
            HeaderValue::from_str(page.as_str()).map_err(|_| DownloadError::InvalidSource)?,
        );
    }
    Ok(headers)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mixed_fake_ip_requires_real_resolution_but_private_or_mixed_public_does_not() {
        let mixed = vec![
            "198.18.5.1:443".parse().unwrap(),
            "[2001:2::b5]:443".parse().unwrap(),
        ];
        assert!(needs_real_dns("tracker.example", &mixed));
        assert!(!needs_real_dns("198.18.5.1", &mixed));
        assert!(!needs_real_dns(
            "tracker.example",
            &["127.0.0.1:443".parse().unwrap()]
        ));
        assert!(!needs_real_dns(
            "tracker.example",
            &[mixed[0], "8.8.8.8:443".parse().unwrap()]
        ));
        assert!(mixed.iter().all(|a| !public_ip(a.ip())));
        assert_eq!(
            dns_answers(br#"{"Status":0,"Answer":[{"type":28,"data":"2606:4700:4700::1111"}]}"#)
                .unwrap(),
            vec!["2606:4700:4700::1111".parse::<IpAddr>().unwrap()]
        );
        assert!(dns_answers(br#"{"Status":0,"Answer":[{"type":1,"data":"127.0.0.1"}]}"#).is_err());
    }
    #[test]
    fn real_dns_keeps_both_families_and_rejects_any_private_answer() {
        let v4 = "8.8.8.8".parse().unwrap();
        let v6 = "2606:4700:4700::1111".parse().unwrap();
        assert_eq!(merge_dns(Ok(vec![v4]), Ok(vec![v6])).unwrap(), vec![v4, v6]);
        assert_eq!(
            merge_dns(Err(DownloadError::Network), Ok(vec![v6])).unwrap(),
            vec![v6]
        );
        assert!(merge_dns(Ok(vec![v4]), Err(DownloadError::InvalidSource)).is_err());
    }
    #[tokio::test]
    async fn actual_connection_addresses_reject_private_targets() {
        let network = Network::default();
        assert!(network.addresses("127.0.0.1", 8080, false).await.is_err());
        assert!(network.addresses("198.18.1.1", 80, false).await.is_err());
        assert_eq!(
            network.addresses("127.0.0.1", 8080, true).await.unwrap(),
            vec!["127.0.0.1:8080".parse().unwrap()]
        );
    }
    #[test]
    fn source_and_address_boundaries() {
        for value in [
            "file:///etc/passwd",
            "https://user:password@example.com/a",
            "data:text/plain,test",
            "https://example.com/\n",
        ] {
            assert!(source_url(value).is_err());
        }
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "192.0.2.1",
            "198.51.100.1",
            "203.0.113.1",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            "2001:db8::1",
        ] {
            assert!(!public_ip(ip.parse().unwrap()), "{ip}");
        }
        assert!(public_ip("8.8.8.8".parse().unwrap()));
        assert!(public_ip("2606:4700:4700::1111".parse().unwrap()));
    }
}
