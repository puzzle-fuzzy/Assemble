//! 有界区间读取；正文离开客户端之前就核对同一表示的强验证值。
use crate::{Client, ClientError, RequestOptions};
use assemble_protocol::{drive::DriveFile, primitives::is_valid_id};
use reqwest::{Method, StatusCode, header};

pub const CHUNK_BYTES: u64 = 1024 * 1024;

impl Client {
    pub async fn drive_metadata(
        &self,
        id: &str,
        options: &RequestOptions<'_>,
    ) -> Result<DriveFile, ClientError> {
        if !is_valid_id(id) {
            return Err(ClientError::InvalidRequest);
        }
        let response = self
            .json_request(
                Method::GET,
                &format!("/v1/drive/files/{id}"),
                vec![],
                None,
                options,
            )
            .await?;
        tokio::select! { biased;
            ()=options.cancel.cancelled()=>Err(ClientError::Cancelled),
            result=tokio::time::timeout(options.timeout.unwrap_or(crate::client::DEFAULT_TIMEOUT),response.json::<DriveFile>())=>result.map_err(|_|ClientError::Timeout)?.map_err(|_|ClientError::InvalidResponse),
        }
    }
    /// 每次最多读取一片。`None` 只在首片遇到不支持强区间的旧服务时返回。
    pub async fn drive_chunk(
        &self,
        file: &DriveFile,
        start: u64,
        options: &RequestOptions<'_>,
    ) -> Result<Option<Vec<u8>>, ClientError> {
        if !is_valid_id(&file.id) || start >= file.size {
            return Err(ClientError::InvalidRequest);
        }
        let end = start.saturating_add(CHUNK_BYTES).min(file.size) - 1;
        let etag = format!("\"{}\"", file.sha256);
        let mut request = self
            .bare_request(Method::GET, &format!("/v1/drive/files/{}/content", file.id))?
            .header(header::RANGE, format!("bytes={start}-{end}"))
            .header(header::IF_RANGE, &etag)
            .header(header::ACCEPT_ENCODING, "identity");
        if let Some(auth) = self.authorization() {
            request = request.header(header::AUTHORIZATION, auth.as_str());
        }
        let work = async {
            let mut response = request.send().await.map_err(|_| ClientError::Network)?;
            let status = response.status();
            if !status.is_success() {
                return Err(crate::client::rejection(status.as_u16(), response).await);
            }
            if start == 0 && status == StatusCode::OK {
                return Ok(None);
            }
            let range = format!("bytes {start}-{end}/{}", file.size);
            if !valid_headers(status, response.headers(), &etag, &range, end - start + 1) {
                return Err(ClientError::InvalidResponse);
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| ClientError::Network)? {
                if bytes.len() as u64 + chunk.len() as u64 > end - start + 1 {
                    return Err(ClientError::InvalidResponse);
                }
                bytes.extend_from_slice(&chunk);
            }
            if bytes.len() as u64 != end - start + 1 {
                return Err(ClientError::InvalidResponse);
            }
            Ok(Some(bytes))
        };
        tokio::select! { biased;
            () = options.cancel.cancelled() => Err(ClientError::Cancelled),
            result = tokio::time::timeout(options.timeout.unwrap_or(crate::client::DEFAULT_TIMEOUT), work) => result.map_err(|_|ClientError::Timeout)?,
        }
    }
}

fn valid_headers(
    status: StatusCode,
    headers: &header::HeaderMap,
    etag: &str,
    range: &str,
    length: u64,
) -> bool {
    status == StatusCode::PARTIAL_CONTENT
        && headers
            .get(header::ETAG)
            .is_some_and(|v| v.as_bytes() == etag.as_bytes())
        && headers
            .get(header::CONTENT_RANGE)
            .is_some_and(|v| v.as_bytes() == range.as_bytes())
        && headers
            .get(header::CONTENT_ENCODING)
            .is_none_or(|v| v == "identity")
        && headers
            .get(header::CONTENT_LENGTH)
            .is_none_or(|v| v.to_str().ok().and_then(|v| v.parse::<u64>().ok()) == Some(length))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_exact_strong_ranges_can_be_combined() {
        let mut headers = header::HeaderMap::new();
        headers.insert(header::ETAG, "\"abc\"".parse().unwrap());
        headers.insert(header::CONTENT_RANGE, "bytes 10-19/30".parse().unwrap());
        assert!(valid_headers(
            StatusCode::PARTIAL_CONTENT,
            &headers,
            "\"abc\"",
            "bytes 10-19/30",
            10
        ));
        for status in [StatusCode::OK, StatusCode::RANGE_NOT_SATISFIABLE] {
            assert!(!valid_headers(
                status,
                &headers,
                "\"abc\"",
                "bytes 10-19/30",
                10
            ));
        }
        for value in ["W/\"abc\"", "\"changed\""] {
            headers.insert(header::ETAG, value.parse().unwrap());
            assert!(!valid_headers(
                StatusCode::PARTIAL_CONTENT,
                &headers,
                "\"abc\"",
                "bytes 10-19/30",
                10
            ));
        }
        headers.insert(header::ETAG, "\"abc\"".parse().unwrap());
        for value in ["bytes 0-9/30", "bytes 10-19/31", "bytes */30"] {
            headers.insert(header::CONTENT_RANGE, value.parse().unwrap());
            assert!(!valid_headers(
                StatusCode::PARTIAL_CONTENT,
                &headers,
                "\"abc\"",
                "bytes 10-19/30",
                10
            ));
        }
    }
}
