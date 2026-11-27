//! 预览读取有严格的内存上限；媒体每次只读取请求的一段。
use assemble_protocol::preview::{FilePreview, PREVIEW_CHUNK_BYTES};
use assemble_protocol::primitives::is_valid_id;
use reqwest::header::{AUTHORIZATION, CONTENT_RANGE, CONTENT_TYPE, RANGE};

use crate::{Client, ClientError, RequestOptions};

pub struct PreviewChunk {
    pub bytes: Vec<u8>,
    pub content_range: String,
    pub mime_type: String,
}

impl Client {
    pub async fn drive_preview(
        &self,
        id: &str,
        options: &RequestOptions<'_>,
    ) -> Result<FilePreview, ClientError> {
        let bytes = self
            .preview_request(id, "", None, options, 8 * 1024 * 1024)
            .await?;
        let preview: FilePreview =
            serde_json::from_slice(&bytes.0).map_err(|_| ClientError::InvalidResponse)?;
        if !preview.valid() {
            return Err(ClientError::InvalidResponse);
        }
        Ok(preview)
    }

    pub async fn drive_preview_chunk(
        &self,
        id: &str,
        start: u64,
        end: u64,
        total: u64,
        options: &RequestOptions<'_>,
    ) -> Result<PreviewChunk, ClientError> {
        // 图片一次最多 16 MiB；音视频调用方仍按 4 MiB 分段。
        if start > end || end >= total || end - start >= 4 * PREVIEW_CHUNK_BYTES {
            return Err(ClientError::InvalidRequest);
        }
        let (bytes, headers, status) = self
            .preview_request(
                id,
                "/content",
                Some(format!("bytes={start}-{end}")),
                options,
                16 * 1024 * 1024,
            )
            .await?;
        let expected = format!("bytes {start}-{end}/{total}");
        if status != 206
            || headers.get(CONTENT_RANGE).and_then(|v| v.to_str().ok()) != Some(&expected)
            || bytes.len() as u64 != end - start + 1
        {
            return Err(ClientError::InvalidResponse);
        }
        let mime_type = headers
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .ok_or(ClientError::InvalidResponse)?
            .to_owned();
        Ok(PreviewChunk {
            bytes,
            content_range: expected,
            mime_type,
        })
    }

    async fn preview_request(
        &self,
        id: &str,
        suffix: &str,
        range: Option<String>,
        options: &RequestOptions<'_>,
        limit: usize,
    ) -> Result<(Vec<u8>, reqwest::header::HeaderMap, u16), ClientError> {
        if !is_valid_id(id) {
            return Err(ClientError::InvalidRequest);
        }
        let authorization = self.authorization().ok_or(ClientError::InvalidRequest)?;
        let mut request = self
            .bare_request(
                reqwest::Method::GET,
                &format!("/v1/drive/files/{id}/preview{suffix}"),
            )?
            .header(AUTHORIZATION, authorization.as_str());
        if let Some(range) = range {
            request = request.header(RANGE, range);
        }
        let work = async {
            let mut response = request.send().await.map_err(|_| ClientError::Network)?;
            if !response.status().is_success() {
                return Err(ClientError::Http {
                    status: response.status().as_u16(),
                    remote_code: None,
                });
            }
            let headers = response.headers().clone();
            let status = response.status().as_u16();
            if response
                .content_length()
                .is_some_and(|length| length > limit as u64)
            {
                return Err(ClientError::InvalidResponse);
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| ClientError::Network)? {
                if bytes.len().saturating_add(chunk.len()) > limit {
                    return Err(ClientError::InvalidResponse);
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok((bytes, headers, status))
        };
        tokio::select! {
            biased;
            () = options.cancel.cancelled() => Err(ClientError::Cancelled),
            result = tokio::time::timeout(options.timeout.unwrap_or(std::time::Duration::from_secs(20)), work) => result.map_err(|_| ClientError::Timeout)?,
        }
    }
}
