//! 来源请求只发往固定 HTTPS 主机，不带浏览器 Cookie，不跟随重定向。
use super::MediaConfig;
use crate::error::ApiError;
use assemble_protocol::media::{
    MediaActor, MediaCandidate, MediaCast, MediaContent, MediaIdentity, MediaPersonIdentity,
    MediaProvider, MediaProviders, MediaStatus, MediaType,
};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, Semaphore};
use url::Url;

pub const MAX_IMAGES: usize = 100;
#[derive(Debug, Clone)]
pub struct SourceImage {
    pub provider: MediaProvider,
    pub url: String,
}
#[derive(Debug, Clone)]
pub struct SourceCast {
    pub cast: MediaCast,
    pub photo: Option<SourceImage>,
}
#[derive(Debug, Clone)]
pub struct SourceCredit {
    pub candidate: MediaCandidate,
    pub poster: Option<SourceImage>,
}
#[derive(Debug, Clone)]
pub struct Details {
    pub candidate: MediaCandidate,
    pub content: MediaContent,
    pub cast: Vec<SourceCast>,
    pub poster: Option<SourceImage>,
}
#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    pub content_type: String,
    pub bytes: Vec<u8>,
}
type FetchFuture<'a> = Pin<Box<dyn Future<Output = Result<Reply, ApiError>> + Send + 'a>>;
pub trait Transport: std::fmt::Debug + Send + Sync {
    fn get(&self, url: Url, headers: HeaderMap, maximum: usize) -> FetchFuture<'_>;
}
#[derive(Debug)]
pub(super) struct HttpTransport {
    pub(super) client: reqwest::Client,
}
impl Transport for HttpTransport {
    fn get(&self, url: Url, headers: HeaderMap, maximum: usize) -> FetchFuture<'_> {
        Box::pin(async move {
            let mut response = self
                .client
                .get(url)
                .headers(headers)
                .send()
                .await
                .map_err(|_| unavailable("MEDIA_SOURCE_UNAVAILABLE"))?;
            let status = response.status().as_u16();
            let content_type = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_owned();
            if response
                .content_length()
                .is_some_and(|n| n > maximum as u64)
            {
                return Err(invalid());
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| unavailable("MEDIA_SOURCE_UNAVAILABLE"))?
            {
                if bytes.len().saturating_add(chunk.len()) > maximum {
                    return Err(invalid());
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(Reply {
                status,
                content_type,
                bytes,
            })
        })
    }
}
#[derive(Debug)]
pub struct Providers {
    config: MediaConfig,
    transport: Arc<dyn Transport>,
    slots: Semaphore,
    cache: Mutex<BTreeMap<String, (i64, usize, Value)>>,
}
impl Providers {
    pub fn new(config: MediaConfig) -> Result<Self, ApiError> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(15))
            .user_agent("Mozilla/5.0 AssembleMedia/1.0")
            .build()
            .map_err(|_| unavailable("MEDIA_SOURCE_UNAVAILABLE"))?;
        Ok(Self::with_transport(
            config,
            Arc::new(HttpTransport { client }),
        ))
    }
    pub fn with_transport(config: MediaConfig, transport: Arc<dyn Transport>) -> Self {
        Self {
            config,
            transport,
            slots: Semaphore::new(4),
            cache: Mutex::new(BTreeMap::new()),
        }
    }
    pub fn capabilities(&self) -> MediaProviders {
        MediaProviders {
            tmdb: self.config.tmdb_token.is_some(),
            douban: true,
        }
    }
    async fn json(
        &self,
        provider: MediaProvider,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<Value, ApiError> {
        let mut url = Url::parse(match provider {
            MediaProvider::Tmdb => "https://api.tmdb.org/3/",
            MediaProvider::Douban => "https://m.douban.com/rexxar/api/v2/",
        })
        .map_err(|_| invalid())?
        .join(path)
        .map_err(|_| invalid())?;
        {
            let mut pairs = url.query_pairs_mut();
            if provider == MediaProvider::Tmdb && !query.iter().any(|(key, _)| *key == "language") {
                pairs.append_pair("language", "zh-CN");
            }
            for (key, value) in query {
                pairs.append_pair(key, value);
            }
        }
        let key = url.as_str().to_owned();
        {
            let cache = self.cache.lock().await;
            if let Some((expires, _, value)) = cache
                .get(&key)
                .filter(|(expires, _, _)| *expires > super::now())
            {
                let _ = expires;
                return Ok(value.clone());
            }
        }
        let mut headers = HeaderMap::new();
        headers.insert("accept", HeaderValue::from_static("application/json"));
        match provider {
            MediaProvider::Tmdb => {
                let token = self.config.tmdb_token.as_ref().ok_or_else(|| {
                    ApiError::new(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "TMDB_NOT_CONFIGURED",
                        "the metadata provider is not configured",
                    )
                })?;
                headers.insert(
                    "authorization",
                    HeaderValue::from_str(&format!("Bearer {}", token.expose()))
                        .map_err(|_| unavailable("TMDB_AUTH_FAILED"))?,
                );
            }
            MediaProvider::Douban => {
                headers.insert("referer", HeaderValue::from_static("https://m.douban.com/"));
            }
        }
        let _slot = self
            .slots
            .acquire()
            .await
            .map_err(|_| unavailable("MEDIA_SOURCE_UNAVAILABLE"))?;
        let reply = self.transport.get(url, headers, 4 * 1024 * 1024).await?;
        check_status(reply.status, provider)?;
        if !reply
            .content_type
            .to_ascii_lowercase()
            .starts_with("application/json")
        {
            return Err(invalid());
        }
        let value: Value = serde_json::from_slice(&reply.bytes).map_err(|_| invalid())?;
        let mut cache = self.cache.lock().await;
        cache.retain(|_, (at, _, _)| *at > super::now());
        let size = reply.bytes.len();
        while cache.len() >= 100
            || cache
                .values()
                .map(|(_, size, _)| size)
                .sum::<usize>()
                .saturating_add(size)
                > 16 * 1024 * 1024
        {
            if cache.pop_first().is_none() {
                break;
            }
        }
        cache.insert(key, (super::now() + 300_000, size, value.clone()));
        Ok(value)
    }
    pub async fn search(
        &self,
        provider: MediaProvider,
        media_type: MediaType,
        q: &str,
    ) -> Result<Vec<MediaCandidate>, ApiError> {
        let q = q.trim();
        if q.is_empty() || q.chars().count() > 200 {
            return Err(ApiError::invalid("invalid media search term"));
        }
        let kind = super::spelling(&media_type);
        let data = match provider {
            MediaProvider::Tmdb => {
                self.json(
                    provider,
                    &format!("search/{kind}"),
                    &[
                        ("query", q.into()),
                        ("include_adult", "false".into()),
                        ("page", "1".into()),
                    ],
                )
                .await?
            }
            MediaProvider::Douban => {
                self.json(
                    provider,
                    "search/movie",
                    &[
                        ("q", q.into()),
                        ("start", "0".into()),
                        ("count", "20".into()),
                    ],
                )
                .await?
            }
        };
        let values = match provider {
            MediaProvider::Tmdb => data.get("results"),
            MediaProvider::Douban => data.get("items").or_else(|| data.get("subjects")),
        }
        .and_then(Value::as_array)
        .ok_or_else(invalid)?;
        let mut seen = BTreeSet::new();
        let mut result = Vec::new();
        for raw in values.iter().take(100) {
            let mut value = raw.get("target").unwrap_or(raw).clone();
            if provider == MediaProvider::Douban {
                if value.get("type").is_none() {
                    value["type"] = raw.get("target_type").cloned().unwrap_or(Value::Null);
                }
                if value.get("type").and_then(Value::as_str) != Some(&kind) {
                    continue;
                }
            }
            let (candidate, _) = summary(provider, media_type, &value)?;
            if seen.insert(candidate.identity.external_id.clone()) {
                result.push(candidate);
            }
            if result.len() == 20 {
                break;
            }
        }
        Ok(result)
    }
    pub async fn details(&self, identity: &MediaIdentity) -> Result<Details, ApiError> {
        validate_identity(identity)?;
        let kind = super::spelling(&identity.media_type);
        let provider = identity.provider;
        let append = if identity.media_type == MediaType::Tv {
            "aggregate_credits"
        } else {
            "credits"
        };
        let params = if provider == MediaProvider::Tmdb {
            vec![("append_to_response", append.into())]
        } else {
            vec![]
        };
        let data = self
            .json(
                provider,
                &format!("{kind}/{}", identity.external_id),
                &params,
            )
            .await?;
        if external_id(&data["id"])? != identity.external_id
            || provider == MediaProvider::Douban && data["type"].as_str() != Some(&kind)
        {
            return Err(invalid());
        }
        let (candidate, poster) = summary(provider, identity.media_type, &data)?;
        let content = parse_content(identity, &data, &candidate)?;
        let mut cast = Vec::new();
        if provider == MediaProvider::Tmdb {
            let actors = data
                .get(append)
                .and_then(|v| v.get("cast"))
                .and_then(Value::as_array)
                .ok_or_else(invalid)?;
            for actor in actors.iter().take(40) {
                cast.push(parse_cast(provider, actor)?);
            }
        } else if data
            .get("actors")
            .is_some_and(|v| v.as_array().is_some_and(|a| !a.is_empty()))
        {
            let actors = self
                .json(
                    provider,
                    &format!("{kind}/{}/celebrities", identity.external_id),
                    &[("count", "100".into())],
                )
                .await?;
            let actors = actors
                .get("actors")
                .and_then(Value::as_array)
                .ok_or_else(invalid)?;
            for actor in actors.iter().take(40) {
                cast.push(parse_cast(provider, actor)?);
            }
        }
        super::store::validate_content(&content).map_err(|_| invalid())?;
        Ok(Details {
            candidate,
            content,
            cast,
            poster,
        })
    }
    pub async fn actor(
        &self,
        identity: &MediaPersonIdentity,
        media_type: MediaType,
    ) -> Result<(MediaActor, Option<SourceImage>, Vec<SourceCredit>), ApiError> {
        validate_external_id(&identity.external_id)?;
        let provider = identity.provider;
        let kind = super::spelling(&media_type);
        let path = if provider == MediaProvider::Tmdb {
            format!("person/{}", identity.external_id)
        } else {
            format!("celebrity/{}", identity.external_id)
        };
        let params = if provider == MediaProvider::Tmdb {
            vec![("append_to_response", format!("{kind}_credits"))]
        } else {
            vec![]
        };
        let data = self.json(provider, &path, &params).await?;
        let name = data
            .get("name")
            .or_else(|| data.get("title"))
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty() && s.chars().count() <= 300)
            .ok_or_else(invalid)?
            .to_owned();
        let photo = if provider == MediaProvider::Tmdb {
            tmdb_image(data.get("profile_path").and_then(Value::as_str))?
        } else {
            douban_image(
                data.pointer("/cover/normal/url")
                    .or_else(|| data.pointer("/avatar/large"))
                    .or_else(|| data.pointer("/avatar/normal"))
                    .and_then(Value::as_str),
            )?
        };
        let mut items = BTreeMap::new();
        if provider == MediaProvider::Tmdb {
            if external_id(&data["id"])? != identity.external_id {
                return Err(invalid());
            }
            let values = data
                .get(format!("{kind}_credits"))
                .and_then(|v| v.get("cast"))
                .and_then(Value::as_array)
                .ok_or_else(invalid)?;
            if values.len() > 2000 {
                return Err(unavailable("MEDIA_FILMOGRAPHY_TOO_LARGE"));
            }
            for value in values {
                let (candidate, poster) = summary(provider, media_type, value)?;
                items.insert(
                    candidate.identity.external_id.clone(),
                    SourceCredit { candidate, poster },
                );
            }
        } else {
            items = self
                .douban_credits(&identity.external_id, media_type)
                .await?;
        }
        let mut items: Vec<_> = items.into_values().collect();
        items.sort_by(|a, b| {
            b.candidate.year.cmp(&a.candidate.year).then_with(|| {
                b.candidate
                    .identity
                    .external_id
                    .cmp(&a.candidate.identity.external_id)
            })
        });
        Ok((
            MediaActor {
                identity: identity.clone(),
                name,
                photo: None,
            },
            photo,
            items,
        ))
    }
    async fn douban_credits(
        &self,
        id: &str,
        media_type: MediaType,
    ) -> Result<BTreeMap<String, SourceCredit>, ApiError> {
        let provider = MediaProvider::Douban;
        let kind = super::spelling(&media_type);
        let mut items = BTreeMap::new();
        let mut start = 0_u64;
        let mut expected = None;
        loop {
            let page = self
                .json(
                    provider,
                    &format!("celebrity/{id}/works"),
                    &[
                        ("start", start.to_string()),
                        ("count", "10".into()),
                        ("sort", "time".into()),
                    ],
                )
                .await?;
            let total = page["total"].as_u64().ok_or_else(invalid)?;
            let works = page["works"].as_array().ok_or_else(invalid)?;
            if total > 1000
                || expected.is_some_and(|v| v != total)
                || page["start"].as_u64() != Some(start)
                || page["count"].as_u64() != Some(10)
                || works.len() as u64 != 10.min(total.saturating_sub(start))
            {
                return Err(invalid());
            }
            expected = Some(total);
            for value in works {
                let roles = value["roles"].as_array().ok_or_else(invalid)?;
                if !roles.iter().any(|v| {
                    v.as_str().is_some_and(|r| {
                        r == "演员"
                            || r.starts_with("演员 ")
                            || r.starts_with("演员（")
                            || r.starts_with("演员(")
                    })
                }) {
                    continue;
                }
                let work = &value["work"];
                if work["type"].as_str() != Some(&kind) {
                    continue;
                }
                let (candidate, poster) = summary(provider, media_type, work)?;
                items.insert(
                    candidate.identity.external_id.clone(),
                    SourceCredit { candidate, poster },
                );
            }
            start += 10;
            if start >= total {
                break;
            }
        }
        Ok(items)
    }
    pub async fn gallery(&self, identity: &MediaIdentity) -> Result<Vec<SourceImage>, ApiError> {
        validate_identity(identity)?;
        let provider = identity.provider;
        let kind = super::spelling(&identity.media_type);
        if provider == MediaProvider::Tmdb {
            let data = self
                .json(
                    provider,
                    &format!("{kind}/{}/images", identity.external_id),
                    &[("language", String::new())],
                )
                .await?;
            if external_id(&data["id"])? != identity.external_id {
                return Err(invalid());
            }
            let posters = data["posters"].as_array().ok_or_else(invalid)?;
            let mut seen = BTreeSet::new();
            let mut images = Vec::new();
            for poster in posters {
                if images.len() == MAX_IMAGES {
                    break;
                }
                let image = tmdb_image(poster.get("file_path").and_then(Value::as_str))?
                    .ok_or_else(invalid)?;
                if seen.insert(image.url.clone()) {
                    images.push(image);
                }
            }
            return Ok(images);
        }
        let mut ids = BTreeSet::new();
        let mut start = 0_usize;
        loop {
            let url = Url::parse(&format!(
                "https://movie.douban.com/subject/{}/photos?type=R&start={start}",
                identity.external_id
            ))
            .map_err(|_| invalid())?;
            let mut headers = HeaderMap::new();
            headers.insert("accept", HeaderValue::from_static("text/html"));
            headers.insert(
                "referer",
                HeaderValue::from_static("https://movie.douban.com/"),
            );
            let _slot = self.slots.acquire().await.map_err(|_| invalid())?;
            let reply = self.transport.get(url, headers, 1024 * 1024).await?;
            check_status(reply.status, provider)?;
            if !reply.content_type.starts_with("text/html") {
                return Err(unavailable("DOUBAN_VERIFICATION_REQUIRED"));
            }
            let html = String::from_utf8(reply.bytes).map_err(|_| invalid())?;
            let (page, more) = parse_douban_posters(&html, &identity.external_id)?;
            for photo_id in page {
                if ids.len() == MAX_IMAGES {
                    break;
                }
                ids.insert(photo_id);
            }
            // 上限约束本次取图预算，不把仍有下一页当成同步失败；重复页面也不能无限翻页。
            if !more || ids.len() == MAX_IMAGES || start + 30 > MAX_IMAGES {
                break;
            }
            start += 30;
        }
        let mut result = Vec::new();
        for id in ids {
            let data = self.json(provider, &format!("photo/{id}"), &[]).await?;
            if external_id(&data["id"])? != id
                || data["owner_uri"].as_str()
                    != Some(&format!(
                        "douban://douban.com/{kind}/{}",
                        identity.external_id
                    ))
                || data["photo_type"].as_str() != Some("poster")
            {
                return Err(invalid());
            }
            result.push(
                douban_image(data.pointer("/image/large/url").and_then(Value::as_str))?
                    .ok_or_else(invalid)?,
            );
        }
        Ok(result)
    }
    pub async fn image(&self, image: &SourceImage) -> Result<Vec<u8>, ApiError> {
        validate_image_url(image)?;
        let _slot = self.slots.acquire().await.map_err(|_| invalid())?;
        let mut headers = HeaderMap::new();
        headers.insert(
            "accept",
            HeaderValue::from_static("image/jpeg,image/png,image/webp"),
        );
        if image.provider == MediaProvider::Douban {
            headers.insert(
                "referer",
                HeaderValue::from_static("https://movie.douban.com/"),
            );
        }
        let reply = self
            .transport
            .get(
                Url::parse(&image.url).map_err(|_| invalid())?,
                headers,
                12 * 1024 * 1024,
            )
            .await?;
        check_status(reply.status, image.provider)?;
        if !["image/jpeg", "image/png", "image/webp"]
            .contains(&reply.content_type.split(';').next().unwrap_or_default())
        {
            return Err(invalid());
        }
        Ok(reply.bytes)
    }
}
pub fn validate_external_id(id: &str) -> Result<(), ApiError> {
    if id.is_empty()
        || id.len() > 20
        || id.starts_with('0')
        || !id.bytes().all(|b| b.is_ascii_digit())
        || id.parse::<u64>().is_err()
    {
        Err(ApiError::invalid("invalid media source id"))
    } else {
        Ok(())
    }
}
pub fn validate_identity(identity: &MediaIdentity) -> Result<(), ApiError> {
    validate_external_id(&identity.external_id)
}
fn external_id(value: &Value) -> Result<String, ApiError> {
    let id = value
        .as_u64()
        .map(|v| v.to_string())
        .or_else(|| value.as_str().map(str::to_owned))
        .ok_or_else(invalid)?;
    validate_external_id(&id).map_err(|_| invalid())?;
    Ok(id)
}
fn text(data: &Value, keys: &[&str], required: bool, max: usize) -> Result<String, ApiError> {
    let value = keys
        .iter()
        .find_map(|key| data.get(key).and_then(Value::as_str))
        .unwrap_or_default();
    if value.chars().count() > max || required && value.trim().is_empty() || value.contains('\0') {
        return Err(invalid());
    }
    Ok(value.to_owned())
}
fn source_date(value: Option<&Value>) -> Option<String> {
    let values: Vec<&str> = match value {
        Some(Value::String(s)) => vec![s],
        Some(Value::Array(v)) => v.iter().filter_map(Value::as_str).collect(),
        _ => vec![],
    };
    values.into_iter().find_map(|s| {
        let date = s.get(..10)?;
        chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .ok()
            .map(|_| date.to_owned())
    })
}
fn summary(
    provider: MediaProvider,
    kind: MediaType,
    data: &Value,
) -> Result<(MediaCandidate, Option<SourceImage>), ApiError> {
    let id = external_id(&data["id"])?;
    let title = text(
        data,
        if provider == MediaProvider::Tmdb {
            if kind == MediaType::Movie {
                &["title"]
            } else {
                &["name"]
            }
        } else {
            &["title"]
        },
        true,
        300,
    )?;
    let original_title = text(data, &["original_title", "original_name"], false, 300)?;
    let overview = text(data, &["overview", "intro"], false, 20_000)?;
    let released = source_date(
        data.get("release_date")
            .or_else(|| data.get("first_air_date"))
            .or_else(|| data.get("pubdate")),
    );
    let year = data
        .get("year")
        .and_then(|v| {
            v.as_i64()
                .and_then(|y| i32::try_from(y).ok())
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })
        .or_else(|| released.as_ref().and_then(|d| d[..4].parse().ok()))
        .filter(|y| (1800..=9999).contains(y));
    let poster = if provider == MediaProvider::Tmdb {
        tmdb_image(data.get("poster_path").and_then(Value::as_str))?
    } else {
        douban_image(
            data.pointer("/cover/image/large/url")
                .or_else(|| data.get("cover_url"))
                .or_else(|| data.pointer("/pic/large"))
                .or_else(|| data.pointer("/cover/url"))
                .and_then(Value::as_str),
        )?
    };
    Ok((
        MediaCandidate {
            identity: MediaIdentity {
                provider,
                media_type: kind,
                external_id: id,
            },
            title,
            original_title,
            year,
            overview,
            poster: None,
        },
        poster,
    ))
}
fn parse_cast(provider: MediaProvider, actor: &Value) -> Result<SourceCast, ApiError> {
    let name = text(actor, &["name", "title"], true, 300)?;
    let character = if let Some(roles) = actor.get("roles").and_then(Value::as_array) {
        roles
            .iter()
            .filter_map(|r| r.get("character").and_then(Value::as_str))
            .take(20)
            .collect::<Vec<_>>()
            .join(" / ")
    } else {
        text(actor, &["character", "role"], false, 1000)?
    };
    let identity = actor
        .get("id")
        .filter(|v| !v.is_null())
        .map(external_id)
        .transpose()?
        .map(|external_id| MediaPersonIdentity {
            provider,
            external_id,
        });
    let photo = if provider == MediaProvider::Tmdb {
        tmdb_image(actor.get("profile_path").and_then(Value::as_str))?
    } else {
        douban_image(
            actor
                .pointer("/avatar/large")
                .or_else(|| actor.pointer("/avatar/normal"))
                .and_then(Value::as_str),
        )?
    };
    Ok(SourceCast {
        cast: MediaCast {
            name,
            character,
            identity,
            photo: None,
        },
        photo,
    })
}
fn tmdb_image(path: Option<&str>) -> Result<Option<SourceImage>, ApiError> {
    path.filter(|p| !p.is_empty())
        .map(|p| {
            if !p.starts_with('/') || p.contains("..") || p.contains(['?', '#', '\\']) {
                return Err(invalid());
            }
            let image = SourceImage {
                provider: MediaProvider::Tmdb,
                url: format!("https://image.tmdb.org/t/p/original{p}"),
            };
            validate_image_url(&image)?;
            Ok(image)
        })
        .transpose()
}
fn douban_image(url: Option<&str>) -> Result<Option<SourceImage>, ApiError> {
    url.filter(|p| !p.is_empty())
        .map(|url| {
            let image = SourceImage {
                provider: MediaProvider::Douban,
                url: url.into(),
            };
            validate_image_url(&image)?;
            Ok(image)
        })
        .transpose()
}
pub fn validate_image_url(image: &SourceImage) -> Result<(), ApiError> {
    let url = Url::parse(&image.url).map_err(|_| invalid())?;
    let host = url.host_str().unwrap_or_default();
    let allowed = match image.provider {
        MediaProvider::Tmdb => host == "image.tmdb.org" && url.path().starts_with("/t/p/"),
        MediaProvider::Douban => {
            [
                "img1.doubanio.com",
                "img2.doubanio.com",
                "img3.doubanio.com",
                "img9.doubanio.com",
            ]
            .contains(&host)
                && ["/view/photo/", "/view/celebrity/", "/view/personage/"]
                    .iter()
                    .any(|prefix| url.path().starts_with(prefix))
        }
    };
    if url.scheme() != "https"
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !allowed
        || image.url.len() > 2048
    {
        return Err(invalid());
    }
    Ok(())
}
fn parse_douban_posters(html: &str, id: &str) -> Result<(Vec<String>, bool), ApiError> {
    if !html.contains("海报") || !html.contains(&format!("/subject/{id}/")) {
        return Err(unavailable("DOUBAN_VERIFICATION_REQUIRED"));
    }
    let (_, list) = html
        .split_once("poster-col3")
        .ok_or_else(|| unavailable("DOUBAN_VERIFICATION_REQUIRED"))?;
    let list = list.split_once("</ul>").ok_or_else(invalid)?.0;
    let mut ids = Vec::new();
    for segment in list.split("data-id=").skip(1) {
        let quote = segment.chars().next().ok_or_else(invalid)?;
        if !['\'', '"'].contains(&quote) {
            return Err(invalid());
        }
        let id = segment[1..].split(quote).next().ok_or_else(invalid)?;
        validate_external_id(id).map_err(|_| invalid())?;
        ids.push(id.into());
    }
    let more = html.contains("class=\"next\"") && !html.contains("class=\"next\"><span")
        || html.contains("rel=\"next\"")
        || html.contains("rel='next'");
    Ok((ids, more))
}
fn check_status(status: u16, provider: MediaProvider) -> Result<(), ApiError> {
    match status {
        200 => Ok(()),
        404 => Err(ApiError::not_found()),
        429 => Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "MEDIA_SOURCE_RATE_LIMITED",
            "metadata source rate limit reached",
        )),
        301..=399 | 401 | 403 | 418 if provider == MediaProvider::Douban => {
            Err(unavailable("DOUBAN_VERIFICATION_REQUIRED"))
        }
        401 | 403 => Err(unavailable("TMDB_AUTH_FAILED")),
        _ => Err(unavailable("MEDIA_SOURCE_UNAVAILABLE")),
    }
}
fn unavailable(code: &'static str) -> ApiError {
    ApiError::new(
        StatusCode::BAD_GATEWAY,
        code,
        "metadata source could not provide the requested information",
    )
}
fn invalid() -> ApiError {
    unavailable("MEDIA_SOURCE_INVALID_RESPONSE")
}

fn parse_content(
    identity: &MediaIdentity,
    data: &Value,
    candidate: &MediaCandidate,
) -> Result<MediaContent, ApiError> {
    let provider = identity.provider;
    let genres = if let Some(raw) = data.get("genres") {
        let values = raw.as_array().ok_or_else(invalid)?;
        if values.len() > 30 {
            return Err(invalid());
        }
        let mut result = BTreeSet::new();
        for genre in values {
            let name = if provider == MediaProvider::Tmdb {
                genre.get("name")
            } else {
                Some(genre)
            }
            .and_then(Value::as_str)
            .ok_or_else(invalid)?
            .trim();
            if name.is_empty() || name.chars().count() > 80 {
                return Err(invalid());
            }
            result.insert(name.to_owned());
        }
        result.into_iter().collect()
    } else {
        Vec::new()
    };
    let release_date = source_date(if provider == MediaProvider::Tmdb {
        data.get(if identity.media_type == MediaType::Movie {
            "release_date"
        } else {
            "first_air_date"
        })
    } else {
        data.get("pubdate")
    });
    let episode_count = if identity.media_type == MediaType::Tv {
        data.get("number_of_episodes")
            .or_else(|| data.get("episodes_count"))
            .and_then(Value::as_u64)
            .filter(|n| *n > 0)
            .and_then(|n| u32::try_from(n).ok())
    } else {
        None
    };
    let content = MediaContent {
        published: false,
        media_type: identity.media_type,
        title: candidate.title.clone(),
        original_title: candidate.original_title.clone(),
        year: candidate.year,
        release_date,
        overview: candidate.overview.clone(),
        genres,
        episode_count,
        status: MediaStatus::Planned,
        watched_at: None,
        review: None,
        notes: None,
        magnet_uri: None,
    };

    Ok(content)
}
