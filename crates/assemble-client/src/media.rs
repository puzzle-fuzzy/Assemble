//! 桌面影视接口：凭据沿用设备管线，图片返回前核对长度与摘要。
use crate::{Client, ClientError, RequestOptions};
use assemble_protocol::media::{
    MediaActorCredits, MediaCandidate, MediaCapabilities, MediaCollection, MediaCollectionList,
    MediaCreate, MediaEntry, MediaGallery, MediaIdentity, MediaImport, MediaItem, MediaItems,
    MediaJob, MediaList, MediaPersonIdentity, MediaPoster, MediaProvider, MediaQuery, MediaScope,
    MediaSync, MediaType, MediaUpdate, MediaVersion,
};
use reqwest::Method;
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaAsset {
    pub bytes: Vec<u8>,
    pub content_type: String,
    pub sha256: String,
}

fn segment(value: &str) -> Result<&str, ClientError> {
    if assemble_protocol::primitives::is_valid_id(value) && !matches!(value, "." | "..") {
        Ok(value)
    } else {
        Err(ClientError::InvalidRequest)
    }
}
fn scope(scope: MediaScope) -> &'static str {
    match scope {
        MediaScope::Shared => "shared",
        MediaScope::Private => "private",
    }
}
fn base(scope: MediaScope, write: bool) -> String {
    if write && scope == MediaScope::Shared {
        "/v1/admin/media".into()
    } else {
        format!("/v1/media/{}", self::scope(scope))
    }
}
fn payload(value: &impl Serialize) -> Result<serde_json::Value, ClientError> {
    serde_json::to_value(value).map_err(|_| ClientError::InvalidRequest)
}
impl Client {
    async fn media_json<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        query: Vec<(&'static str, String)>,
        body: Option<serde_json::Value>,
        options: &RequestOptions<'_>,
    ) -> Result<T, ClientError> {
        let response = self
            .media_request(method, path, query, body, options)
            .await?;
        let bytes = bounded_body(response, 8 * 1024 * 1024, options).await?;
        serde_json::from_slice(&bytes).map_err(|_| ClientError::InvalidResponse)
    }
    pub async fn media_capabilities(
        &self,
        options: &RequestOptions<'_>,
    ) -> Result<MediaCapabilities, ClientError> {
        self.media_json(Method::GET, "/v1/media/capabilities", vec![], None, options)
            .await
    }
    pub async fn media_list(
        &self,
        scope: MediaScope,
        query: &MediaQuery,
        options: &RequestOptions<'_>,
    ) -> Result<MediaList, ClientError> {
        let mut pairs = Vec::new();
        let fields = payload(query)?;
        for (name, key) in [
            ("q", "q"),
            ("page", "page"),
            ("pageSize", "pageSize"),
            ("mediaType", "mediaType"),
            ("status", "status"),
            ("genre", "genre"),
            ("year", "year"),
            ("sort", "sort"),
        ] {
            if let Some(value) = fields.get(key).filter(|v| !v.is_null()) {
                pairs.push((
                    name,
                    value
                        .as_str()
                        .map_or_else(|| value.to_string(), str::to_owned),
                ));
            }
        }
        self.media_json(
            Method::GET,
            &format!("{}/entries", base(scope, false)),
            pairs,
            None,
            options,
        )
        .await
    }
    pub async fn media_get(
        &self,
        scope: MediaScope,
        id: &str,
        options: &RequestOptions<'_>,
    ) -> Result<MediaEntry, ClientError> {
        let result: MediaItem<MediaEntry> = self
            .media_json(
                Method::GET,
                &format!("{}/entries/{}", base(scope, false), segment(id)?),
                vec![],
                None,
                options,
            )
            .await?;
        Ok(result.item)
    }
    pub async fn media_create(
        &self,
        scope: MediaScope,
        input: &MediaCreate,
        options: &RequestOptions<'_>,
    ) -> Result<MediaEntry, ClientError> {
        let result: MediaItem<MediaEntry> = self
            .media_json(
                Method::POST,
                &format!("{}/entries", base(scope, true)),
                vec![],
                Some(payload(input)?),
                options,
            )
            .await?;
        Ok(result.item)
    }
    pub async fn media_update(
        &self,
        scope: MediaScope,
        id: &str,
        input: &MediaUpdate,
        options: &RequestOptions<'_>,
    ) -> Result<MediaEntry, ClientError> {
        let result: MediaItem<MediaEntry> = self
            .media_json(
                Method::PUT,
                &format!("{}/entries/{}", base(scope, true), segment(id)?),
                vec![],
                Some(payload(input)?),
                options,
            )
            .await?;
        Ok(result.item)
    }
    pub async fn media_remove(
        &self,
        scope: MediaScope,
        id: &str,
        version: u64,
        options: &RequestOptions<'_>,
    ) -> Result<(), ClientError> {
        let _: serde_json::Value = self
            .media_json(
                Method::DELETE,
                &format!("{}/entries/{}", base(scope, true), segment(id)?),
                vec![],
                Some(payload(&MediaVersion { version })?),
                options,
            )
            .await?;
        Ok(())
    }
    pub async fn media_search(
        &self,
        provider: MediaProvider,
        media_type: MediaType,
        q: &str,
        options: &RequestOptions<'_>,
    ) -> Result<Vec<MediaCandidate>, ClientError> {
        let result: MediaItems<MediaCandidate> = self
            .media_json(
                Method::GET,
                "/v1/media/search",
                vec![
                    (
                        "provider",
                        serde_json::to_value(provider)
                            .map_err(|_| ClientError::InvalidRequest)?
                            .as_str()
                            .unwrap_or_default()
                            .into(),
                    ),
                    (
                        "mediaType",
                        serde_json::to_value(media_type)
                            .map_err(|_| ClientError::InvalidRequest)?
                            .as_str()
                            .unwrap_or_default()
                            .into(),
                    ),
                    ("q", q.into()),
                ],
                None,
                options,
            )
            .await?;
        Ok(result.items)
    }
    pub async fn media_catalog(
        &self,
        identity: &MediaIdentity,
        options: &RequestOptions<'_>,
    ) -> Result<MediaCandidate, ClientError> {
        let value = payload(identity)?;
        let pairs = ["provider", "mediaType", "externalId"]
            .into_iter()
            .map(|key| (key, value[key].as_str().unwrap_or_default().into()))
            .collect();
        let result: MediaItem<MediaCandidate> = self
            .media_json(
                Method::GET,
                "/v1/media/catalog/details",
                pairs,
                None,
                options,
            )
            .await?;
        Ok(result.item)
    }
    pub async fn media_import(
        &self,
        scope: MediaScope,
        input: &MediaImport,
        options: &RequestOptions<'_>,
    ) -> Result<MediaJob, ClientError> {
        self.media_json(
            Method::POST,
            &format!("{}/imports", base(scope, true)),
            vec![],
            Some(payload(input)?),
            options,
        )
        .await
    }
    pub async fn media_job(
        &self,
        scope: MediaScope,
        id: &str,
        options: &RequestOptions<'_>,
    ) -> Result<MediaJob, ClientError> {
        self.media_json(
            Method::GET,
            &format!("{}/jobs/{}", base(scope, true), segment(id)?),
            vec![],
            None,
            options,
        )
        .await
    }
    pub async fn media_gallery(
        &self,
        scope: MediaScope,
        id: &str,
        options: &RequestOptions<'_>,
    ) -> Result<MediaGallery, ClientError> {
        self.media_json(
            Method::GET,
            &format!("{}/entries/{}/gallery", base(scope, false), segment(id)?),
            vec![],
            None,
            options,
        )
        .await
    }
    pub async fn media_sync_gallery(
        &self,
        scope: MediaScope,
        id: &str,
        input: &MediaSync,
        options: &RequestOptions<'_>,
    ) -> Result<MediaJob, ClientError> {
        self.media_json(
            Method::POST,
            &format!(
                "{}/entries/{}/gallery/sync",
                base(scope, true),
                segment(id)?
            ),
            vec![],
            Some(payload(input)?),
            options,
        )
        .await
    }
    pub async fn media_select_poster(
        &self,
        scope: MediaScope,
        id: &str,
        input: &MediaPoster,
        options: &RequestOptions<'_>,
    ) -> Result<MediaEntry, ClientError> {
        let result: MediaItem<MediaEntry> = self
            .media_json(
                Method::PATCH,
                &format!("{}/entries/{}/poster", base(scope, true), segment(id)?),
                vec![],
                Some(payload(input)?),
                options,
            )
            .await?;
        Ok(result.item)
    }
    pub async fn media_actor(
        &self,
        identity: &MediaPersonIdentity,
        media_type: MediaType,
        options: &RequestOptions<'_>,
    ) -> Result<MediaActorCredits, ClientError> {
        let provider = payload(&identity.provider)?;
        let kind = payload(&media_type)?;
        self.media_json(
            Method::GET,
            &format!(
                "/v1/media/catalog/actors/{}/{}",
                provider.as_str().unwrap_or_default(),
                segment(&identity.external_id)?
            ),
            vec![("mediaType", kind.as_str().unwrap_or_default().into())],
            None,
            options,
        )
        .await
    }
    pub async fn media_collections(
        &self,
        page: u32,
        page_size: u32,
        options: &RequestOptions<'_>,
    ) -> Result<MediaCollectionList, ClientError> {
        self.media_json(
            Method::GET,
            "/v1/media/collections",
            vec![
                ("page", page.to_string()),
                ("pageSize", page_size.to_string()),
            ],
            None,
            options,
        )
        .await
    }
    pub async fn media_collection(
        &self,
        id: &str,
        options: &RequestOptions<'_>,
    ) -> Result<MediaCollection, ClientError> {
        let result: MediaItem<MediaCollection> = self
            .media_json(
                Method::GET,
                &format!("/v1/media/collections/{}", segment(id)?),
                vec![],
                None,
                options,
            )
            .await?;
        Ok(result.item)
    }
    pub async fn media_asset(
        &self,
        asset_id: &str,
        variant: &str,
        options: &RequestOptions<'_>,
    ) -> Result<MediaAsset, ClientError> {
        if !matches!(variant, "original" | "200" | "360" | "480") {
            return Err(ClientError::InvalidRequest);
        }
        let response = self
            .media_request(
                Method::GET,
                &format!("/v1/media/assets/{}/content", segment(asset_id)?),
                vec![("variant", variant.into())],
                None,
                options,
            )
            .await?;
        let length = response
            .content_length()
            .filter(|n| *n > 0 && *n <= 12 * 1024 * 1024)
            .ok_or(ClientError::InvalidResponse)?;
        let sha256 = response
            .headers()
            .get("x-content-sha256")
            .and_then(|v| v.to_str().ok())
            .filter(|v| v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or(ClientError::InvalidResponse)?
            .to_owned();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .filter(|v| matches!(*v, "image/jpeg" | "image/png" | "image/webp"))
            .ok_or(ClientError::InvalidResponse)?
            .to_owned();
        let bytes = bounded_body(
            response,
            usize::try_from(length).map_err(|_| ClientError::InvalidResponse)?,
            options,
        )
        .await?;
        if bytes.len() as u64 != length
            || assemble_protocol::primitives::to_hex(&Sha256::digest(&bytes)) != sha256
        {
            return Err(ClientError::InvalidResponse);
        }
        Ok(MediaAsset {
            bytes,
            content_type,
            sha256,
        })
    }
}

async fn bounded_body(
    mut response: reqwest::Response,
    maximum: usize,
    options: &RequestOptions<'_>,
) -> Result<Vec<u8>, ClientError> {
    if response
        .content_length()
        .is_some_and(|size| size > maximum as u64)
    {
        return Err(ClientError::InvalidResponse);
    }
    let reading = async {
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| ClientError::Network)? {
            if bytes.len().saturating_add(chunk.len()) > maximum {
                return Err(ClientError::InvalidResponse);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    };
    tokio::select! {biased;()=options.cancel.cancelled()=>Err(ClientError::Cancelled),result=tokio::time::timeout(options.timeout.unwrap_or(crate::client::DEFAULT_TIMEOUT),reading)=>result.map_err(|_|ClientError::Timeout)?}
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{HeaderMap, Response};
    use axum::{Router, routing::get};
    use tokio_util::sync::CancellationToken;
    #[tokio::test]
    async fn image_download_verifies_headers_hash_and_cancel_without_exposing_bearer() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin =
            crate::ServerOrigin::parse(&format!("http://{}", listener.local_addr().unwrap()))
                .unwrap();
        let app=Router::new().route("/v1/media/assets/{id}/content",get(|axum::extract::Path(id):axum::extract::Path<String>,headers:HeaderMap|async move{assert_eq!(headers["authorization"],"Bearer test-private-device");let bytes=b"verified image payload";let hash=if id=="valid"{assemble_protocol::primitives::to_hex(&Sha256::digest(bytes))}else{"0".repeat(64)};Response::builder().header("content-type","image/png").header("x-content-sha256",hash).body(Body::from(bytes.as_slice())).unwrap()}));
        let stop = CancellationToken::new();
        let until = stop.clone();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(until.cancelled_owned())
                .await
                .unwrap();
        });
        let client = Client::new(origin).unwrap();
        client.set_bearer(Some(assemble_store::secret::SecretString::new(
            "test-private-device",
        )));
        let cancel = CancellationToken::new();
        let options = RequestOptions::new(&cancel);
        let valid = client.media_asset("valid", "360", &options).await.unwrap();
        assert_eq!(valid.bytes, b"verified image payload");
        assert_eq!(valid.content_type, "image/png");
        assert_eq!(
            client.media_asset("corrupt", "original", &options).await,
            Err(ClientError::InvalidResponse)
        );
        assert_eq!(
            client.media_asset("../escape", "original", &options).await,
            Err(ClientError::InvalidRequest)
        );
        assert_eq!(
            client.media_asset("valid", "../../file", &options).await,
            Err(ClientError::InvalidRequest)
        );
        cancel.cancel();
        assert_eq!(
            client.media_asset("valid", "360", &options).await,
            Err(ClientError::Cancelled)
        );
        stop.cancel();
        task.await.unwrap();
    }
}
