//! 托管磁力只访问固定第一方来源；每次读取都有总时限与内存上限。
use crate::{Client, ClientError, RequestOptions};
use assemble_protocol::magnets::{self, Control, Manifest, Preview, PreviewInput, Start, Task};
use reqwest::Method;
use serde::{Serialize, de::DeserializeOwned};

fn id(value: &str) -> Result<&str, ClientError> {
    if value.len() == 32 && value.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(value)
    } else {
        Err(ClientError::InvalidRequest)
    }
}

impl Client {
    async fn magnet_request(
        &self,
        method: Method,
        path: &str,
        payload: Option<serde_json::Value>,
        options: &RequestOptions<'_>,
        limit: usize,
    ) -> Result<Vec<u8>, ClientError> {
        let work = async {
            let mut response = self
                .json_request(method, path, Vec::new(), payload, options)
                .await?;
            if response.content_length().is_some_and(|n| n > limit as u64) {
                return Err(ClientError::InvalidResponse);
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| ClientError::Network)? {
                if bytes.len().saturating_add(chunk.len()) > limit {
                    return Err(ClientError::InvalidResponse);
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(bytes)
        };
        tokio::select! {
            biased;
            () = options.cancel.cancelled() => Err(ClientError::Cancelled),
            result = tokio::time::timeout(options.timeout.unwrap_or(std::time::Duration::from_secs(20)), work) => result.map_err(|_| ClientError::Timeout)?,
        }
    }
    async fn magnet_json<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        payload: Option<&impl Serialize>,
        options: &RequestOptions<'_>,
    ) -> Result<T, ClientError> {
        let payload = payload
            .map(serde_json::to_value)
            .transpose()
            .map_err(|_| ClientError::InvalidRequest)?;
        let bytes = self
            .magnet_request(method, path, payload, options, magnets::MAX_REPLY_BYTES)
            .await?;
        serde_json::from_slice(&bytes).map_err(|_| ClientError::InvalidResponse)
    }
    pub async fn magnet_preview(
        &self,
        input: &PreviewInput,
        options: &RequestOptions<'_>,
    ) -> Result<Preview, ClientError> {
        id(&input.request_id)?;
        self.magnet_json(Method::POST, "/v1/magnets/previews", Some(input), options)
            .await
    }
    pub async fn magnet_preview_status(
        &self,
        preview: &str,
        options: &RequestOptions<'_>,
    ) -> Result<Preview, ClientError> {
        self.magnet_json::<Preview>(
            Method::GET,
            &format!("/v1/magnets/previews/{}", id(preview)?),
            None::<&()>,
            options,
        )
        .await
    }
    pub async fn magnet_start(
        &self,
        input: &Start,
        options: &RequestOptions<'_>,
    ) -> Result<Task, ClientError> {
        id(&input.request_id)?;
        self.magnet_json(Method::POST, "/v1/magnets/tasks", Some(input), options)
            .await
    }
    pub async fn magnet_cancel_preview(
        &self,
        preview: &str,
        options: &RequestOptions<'_>,
    ) -> Result<Preview, ClientError> {
        self.magnet_json(
            Method::POST,
            &format!("/v1/magnets/previews/{}/cancel", id(preview)?),
            Some(&()),
            options,
        )
        .await
    }
    pub async fn magnet_status(
        &self,
        task: &str,
        options: &RequestOptions<'_>,
    ) -> Result<Task, ClientError> {
        self.magnet_json(
            Method::GET,
            &format!("/v1/magnets/tasks/{}", id(task)?),
            None::<&()>,
            options,
        )
        .await
    }
    pub async fn magnet_control(
        &self,
        task: &str,
        control: &Control,
        options: &RequestOptions<'_>,
    ) -> Result<Task, ClientError> {
        self.magnet_json(
            Method::POST,
            &format!("/v1/magnets/tasks/{}/control", id(task)?),
            Some(control),
            options,
        )
        .await
    }
    pub async fn magnet_manifest(
        &self,
        task: &str,
        options: &RequestOptions<'_>,
    ) -> Result<Manifest, ClientError> {
        self.magnet_json(
            Method::GET,
            &format!("/v1/magnets/tasks/{}/manifest", id(task)?),
            None::<&()>,
            options,
        )
        .await
    }
    pub async fn magnet_chunk(
        &self,
        task: &str,
        file: u32,
        chunk: u64,
        options: &RequestOptions<'_>,
    ) -> Result<Vec<u8>, ClientError> {
        self.magnet_request(
            Method::GET,
            &format!(
                "/v1/magnets/tasks/{}/files/{file}/chunks/{chunk}",
                id(task)?
            ),
            None,
            options,
            usize::try_from(magnets::CHUNK_BYTES).map_err(|_| ClientError::InvalidRequest)?,
        )
        .await
    }
}
