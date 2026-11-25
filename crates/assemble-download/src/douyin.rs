//! 只解析目标视频的公开元数据；匿名会话不读取系统浏览器或 Assemble 的登录凭据。

use assemble_protocol::primitives::Timestamp;
use md5::{Digest, Md5};
use reqwest::header::{COOKIE, HeaderMap, HeaderValue, SET_COOKIE};
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::network::{Network, headers, source_url};
use crate::{DownloadError, MAX_METADATA, Result, random_id};

fn platform(host: &str) -> bool {
    ["douyin.com", "iesdouyin.com"]
        .iter()
        .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
}

pub(crate) fn shared_url(value: &str) -> Result<Url> {
    if value.len() > 16_384 {
        return Err(DownloadError::InvalidSource);
    }
    let mut found = Vec::new();
    for (index, _) in value.match_indices("http") {
        let tail = &value[index..];
        let end = tail
            .find(|c: char| {
                c.is_whitespace()
                    || matches!(
                        c,
                        '"' | '\'' | '<' | '>' | '，' | '。' | '）' | ')' | '」' | '】'
                    )
            })
            .unwrap_or(tail.len());
        if let Ok(url) = source_url(&tail[..end])
            && url.host_str().is_some_and(platform)
            && !found.contains(&url)
        {
            found.push(url);
        }
    }
    if found.len() != 1 {
        return Err(DownloadError::InvalidSource);
    }
    found.pop().ok_or(DownloadError::InvalidSource)
}

fn item_id(url: &Url) -> Option<String> {
    let parts: Vec<_> = url.path_segments()?.filter(|p| !p.is_empty()).collect();
    let offset = usize::from(parts.first() == Some(&"share"));
    let id = if matches!(parts.get(offset), Some(&"video" | &"note" | &"slides")) {
        parts.get(offset + 1).map(|v| (*v).to_owned())
    } else {
        url.query_pairs()
            .find(|(name, _)| name == "modal_id")
            .map(|(_, value)| value.into_owned())
    }?;
    ((10..=25).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit())).then_some(id)
}

fn router_item(body: &[u8], id: &str) -> Option<Value> {
    let html = std::str::from_utf8(body).ok()?;
    let rest = html.get(html.find("_ROUTER_DATA")? + "_ROUTER_DATA".len()..)?;
    let json = rest.trim_start();
    let json = json.strip_prefix('=')?.trim_start();
    let data = serde_json::Deserializer::from_str(json)
        .into_iter::<Value>()
        .next()?
        .ok()?;
    for (route, value) in data.get("loaderData")?.as_object()? {
        if !["video_(id)/page", "note_(id)/page", "slides_(id)/page"].contains(&route.as_str()) {
            continue;
        }
        for item in value.get("videoInfoRes")?.get("item_list")?.as_array()? {
            if item.get("aweme_id").and_then(Value::as_str) == Some(id) {
                return Some(item.clone());
            }
        }
    }
    None
}

fn media_url(value: Option<&Value>) -> Option<Url> {
    value?
        .get("url_list")?
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .filter_map(|value| {
            source_url(&if value.starts_with("//") {
                format!("https:{value}")
            } else {
                value.to_owned()
            })
            .ok()
        })
        .min_by_key(|url| url.scheme() != "https")
}

pub(crate) fn from_item(item: &Value, id: &str) -> Result<(String, Url)> {
    if item.get("aweme_id").and_then(Value::as_str) != Some(id) {
        return Err(DownloadError::SourceUnavailable);
    }
    if item
        .get("images")
        .and_then(Value::as_array)
        .is_some_and(|images| !images.is_empty())
    {
        return Err(DownloadError::Unsupported);
    }
    let video = item.get("video").ok_or(DownloadError::SourceUnavailable)?;
    let complete = video
        .get("bit_rate")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|variant| variant.get("format").and_then(Value::as_str) == Some("mp4"))
        .find_map(|variant| media_url(variant.get("play_addr")));
    let media = if video.get("format").and_then(Value::as_str) == Some("dash") {
        complete
    } else {
        media_url(video.get("play_addr")).or(complete)
    }
    .ok_or(DownloadError::SourceUnavailable)?;
    let title = item
        .get("desc")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map_or_else(|| format!("Douyin-{id}"), |v| v.chars().take(160).collect());
    Ok((title, media))
}

pub(crate) async fn resolve(
    value: &str,
    network: &Network,
    cancel: &CancellationToken,
) -> Result<(String, Url)> {
    let url = shared_url(value)?;
    let headers = headers(Some("https://www.douyin.com/"))?;
    let id = if let Some(id) = item_id(&url) {
        id
    } else if url.host_str() == Some("v.douyin.com") {
        let (target, _) = network.fetch(&url, &headers, None, None, cancel).await?;
        if !target.host_str().is_some_and(platform) {
            return Err(DownloadError::InvalidSource);
        }
        item_id(&target).ok_or(DownloadError::SourceUnavailable)?
    } else {
        return Err(DownloadError::Unsupported);
    };
    let share = source_url(&format!("https://www.iesdouyin.com/share/video/{id}/"))?;
    if let Ok((_, body)) = network.bytes(&share, &headers, MAX_METADATA, cancel).await
        && let Some(item) = router_item(&body, &id)
    {
        return from_item(&item, &id);
    }
    if cancel.is_cancelled() {
        return Err(DownloadError::Cancelled);
    }
    let item = anonymous_detail(&id, network, cancel).await?;
    from_item(&item, &id)
}

fn collect_cookies(response: &reqwest::Response, cookies: &mut Vec<(String, String)>) {
    for value in response.headers().get_all(SET_COOKIE) {
        let Some((name, value)) = value
            .to_str()
            .ok()
            .and_then(|v| v.split(';').next())
            .and_then(|v| v.split_once('='))
        else {
            continue;
        };
        if ![
            "ttwid",
            "uifid",
            "uifid_temp",
            "uifidtemp",
            "UIFID",
            "UIFID_TEMP",
            "UIFIDTEMP",
        ]
        .contains(&name)
            || value.len() > 8192
            || value
                .bytes()
                .any(|b| !(0x21..=0x7e).contains(&b) || matches!(b, b';' | b','))
        {
            continue;
        }
        if let Some(entry) = cookies.iter_mut().find(|(key, _)| key == name) {
            value.clone_into(&mut entry.1);
        } else {
            cookies.push((name.into(), value.into()));
        }
    }
}

fn cookie_headers(cookies: &[(String, String)]) -> Result<HeaderMap> {
    let mut headers = headers(Some("https://www.douyin.com/"))?;
    let value = cookies
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("; ");
    headers.insert(
        COOKIE,
        HeaderValue::from_str(&value).map_err(|_| DownloadError::SourceUnavailable)?,
    );
    Ok(headers)
}

async fn anonymous_detail(
    id: &str,
    network: &Network,
    cancel: &CancellationToken,
) -> Result<Value> {
    // 协议参考 Jarvis 已验证的 yt-dlp PR 17826（Unlicense）；不引入 Python/yt-dlp 运行时。
    // https://github.com/yt-dlp/yt-dlp/pull/17826
    let mut cookies = vec![("passport_csrf_token".into(), random_id()?)];
    let (_, registration) = network
        .fetch(
            &source_url("https://www.douyin.com/ttwid/register")?,
            &cookie_headers(&cookies)?,
            None,
            Some("{\"aid\":6383,\"service\":\"www.douyin.com\"}"),
            cancel,
        )
        .await?;
    collect_cookies(&registration, &mut cookies);
    if !cookies.iter().any(|(key, _)| key == "ttwid") {
        return Err(DownloadError::SourceUnavailable);
    }
    let (_, homepage) = network
        .fetch(
            &source_url("https://www.douyin.com/")?,
            &cookie_headers(&cookies)?,
            None,
            None,
            cancel,
        )
        .await?;
    collect_cookies(&homepage, &mut cookies);
    let uifid = cookies
        .iter()
        .find(|(key, _)| key.to_ascii_lowercase().starts_with("uifid"))
        .map(|(_, value)| value.clone())
        .or_else(|| {
            homepage
                .headers()
                .get("x-tt-trace-host")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        })
        .filter(|value| value.len() <= 8192 && value.bytes().all(|b| (0x21..=0x7e).contains(&b)))
        .ok_or(DownloadError::SourceUnavailable)?;
    let timestamp = (Timestamp::now().as_millis() / 1000).to_string();
    let mut url = source_url("https://www.douyin.com/aweme/v1/web/aweme/detail/")?;
    url.query_pairs_mut()
        .append_pair("aweme_id", id)
        .append_pair("aid", "6383")
        .append_pair("uifid", &uifid)
        .append_pair("timestamp", &timestamp);
    let signature = format!(
        "{uifid}_{timestamp}_A96D855A08C0A9707F8BEF0D9A527E4E_{}",
        url.query().unwrap_or_default()
    );
    let digest = assemble_protocol::primitives::to_hex(&Md5::digest(signature.as_bytes()));
    url.query_pairs_mut()
        .append_pair("x-secsdk-web-signature", &digest);
    let (_, body) = network
        .bytes(&url, &cookie_headers(&cookies)?, MAX_METADATA, cancel)
        .await?;
    let detail: Value =
        serde_json::from_slice(&body).map_err(|_| DownloadError::SourceUnavailable)?;
    detail
        .get("aweme_detail")
        .cloned()
        .ok_or(DownloadError::SourceUnavailable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn share_text_and_exact_video_identity() {
        let url = shared_url("3.14 复制打开抖音 https://v.douyin.com/abc123/ 看视频").unwrap();
        assert_eq!(url.host_str(), Some("v.douyin.com"));
        assert!(shared_url("https://douyin.com.evil.test/video/123456789012").is_err());
        assert!(shared_url("https://v.douyin.com/a https://v.douyin.com/b").is_err());
        assert_eq!(
            item_id(&source_url("https://www.douyin.com/jingxuan?modal_id=123456789012").unwrap())
                .as_deref(),
            Some("123456789012")
        );
        let item = json!({"aweme_id":"123456789012","desc":"标题 <2026>","video":{"format":"dash","play_addr":{"url_list":["https://cdn.example/video-only.mp4"]},"bit_rate":[{"format":"mp4","play_addr":{"url_list":["https://cdn.example/complete.mp4"]}}]}});
        assert_eq!(
            from_item(&item, "123456789012").unwrap().1.path(),
            "/complete.mp4"
        );
        assert_eq!(
            from_item(&item, "999999999999").unwrap_err(),
            DownloadError::SourceUnavailable
        );
        let body = format!(
            "<script>window._ROUTER_DATA = {}; </script>",
            json!({"loaderData":{"video_(id)/page":{"videoInfoRes":{"item_list":[item]}}}})
        );
        assert!(router_item(body.as_bytes(), "123456789012").is_some());
        assert!(router_item(body.as_bytes(), "999999999999").is_none());
    }
}
