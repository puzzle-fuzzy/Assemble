//! 只按服务端确认的片集续传；稳定请求键由调用者保留到任务终态。
use crate::{Client, ClientError, OutgoingFile, RequestOptions, TransferProgress};
use assemble_protocol::{
    drive_tree::TreeFile,
    primitives::is_valid_id,
    uploads::{self as wire, Create, Phase, Upload},
};
use reqwest::Method;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

fn segment(id: &str) -> Result<&str, ClientError> {
    if is_valid_id(id) {
        Ok(id)
    } else {
        Err(ClientError::InvalidRequest)
    }
}
async fn response_body<T: DeserializeOwned>(
    mut response: reqwest::Response,
) -> Result<T, ClientError> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| ClientError::Network)? {
        if bytes.len().saturating_add(chunk.len()) > 32 * 1024 {
            return Err(ClientError::InvalidResponse);
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| ClientError::InvalidResponse)
}
async fn response<T: DeserializeOwned>(
    answer: reqwest::Response,
    options: &RequestOptions<'_>,
) -> Result<T, ClientError> {
    tokio::select! {biased;()=options.cancel.cancelled()=>Err(ClientError::Cancelled),
    result=tokio::time::timeout(options.timeout.unwrap_or(crate::client::DEFAULT_TIMEOUT),response_body(answer))=>result.map_err(|_|ClientError::Timeout)?}
}

impl Client {
    pub async fn upload_prepare(
        &self,
        source: &OutgoingFile,
        parent: &str,
        options: &RequestOptions<'_>,
    ) -> Result<Create, ClientError> {
        let listing = self
            .drive_tree(
                &assemble_protocol::drive_tree::Query {
                    parent_id: parent.to_owned(),
                    folders_only: true,
                    ..Default::default()
                },
                options,
            )
            .await?;
        let mut random = [0_u8; 32];
        getrandom::fill(&mut random).map_err(|_| ClientError::InvalidRequest)?;
        let mut location = listing
            .ancestors
            .iter()
            .map(wire::FolderRevision::from)
            .collect::<Vec<_>>();
        location.extend(listing.current.as_ref().map(wire::FolderRevision::from));
        Ok(Create {
            request_id: assemble_protocol::primitives::to_hex(&random),
            name: source.name(),
            size: source.size,
            sha256: source.sha256.clone(),
            mime_type: None,
            parent_id: parent.to_owned(),
            location,
        })
    }
    /// 只有明确 404 才表示旧服务；认证和网络错误不能降级成一次新的完整上传。
    pub async fn upload_supported(
        &self,
        options: &RequestOptions<'_>,
    ) -> Result<bool, ClientError> {
        match self
            .json_request(Method::GET, "/v1/drive/uploads", vec![], None, options)
            .await
        {
            Ok(answer) => {
                let caps: wire::Capabilities = response(answer, options).await?;
                if caps.part_bytes != wire::PART_BYTES
                    || caps.max_file_bytes != assemble_protocol::primitives::MAX_FILE_SIZE
                {
                    return Err(ClientError::InvalidResponse);
                }
                Ok(true)
            }
            Err(ClientError::Http { status: 404, .. }) => Ok(false),
            Err(error) => Err(error),
        }
    }
    pub async fn upload_create(
        &self,
        input: &Create,
        options: &RequestOptions<'_>,
    ) -> Result<Upload, ClientError> {
        if !input.valid() {
            return Err(ClientError::InvalidRequest);
        }
        response(
            self.json_request(
                Method::POST,
                "/v1/drive/uploads",
                vec![],
                Some(serde_json::to_value(input).map_err(|_| ClientError::InvalidRequest)?),
                options,
            )
            .await?,
            options,
        )
        .await
    }
    pub async fn upload_status(
        &self,
        id: &str,
        options: &RequestOptions<'_>,
    ) -> Result<Upload, ClientError> {
        response(
            self.json_request(
                Method::GET,
                &format!("/v1/drive/uploads/{}", segment(id)?),
                vec![],
                None,
                options,
            )
            .await?,
            options,
        )
        .await
    }
    pub async fn upload_complete(
        &self,
        id: &str,
        options: &RequestOptions<'_>,
    ) -> Result<Upload, ClientError> {
        response(
            self.json_request(
                Method::POST,
                &format!("/v1/drive/uploads/{}/complete", segment(id)?),
                vec![],
                None,
                options,
            )
            .await?,
            options,
        )
        .await
    }
    pub async fn upload_cancel_request(
        &self,
        key: &str,
        options: &RequestOptions<'_>,
    ) -> Result<Upload, ClientError> {
        response(
            self.json_request(
                Method::DELETE,
                &format!("/v1/drive/uploads/requests/{}", segment(key)?),
                vec![],
                None,
                options,
            )
            .await?,
            options,
        )
        .await
    }
    pub async fn upload_part(
        &self,
        id: &str,
        index: u32,
        bytes: Vec<u8>,
        options: &RequestOptions<'_>,
    ) -> Result<Upload, ClientError> {
        if bytes.is_empty() || bytes.len() > usize::try_from(wire::PART_BYTES).unwrap_or(0) {
            return Err(ClientError::InvalidRequest);
        }
        let sha256 = assemble_protocol::primitives::to_hex(&Sha256::digest(&bytes));
        let request = self
            .bare_request(
                Method::PUT,
                &format!("/v1/drive/uploads/{}/parts/{index}", segment(id)?),
            )?
            .header(
                reqwest::header::AUTHORIZATION,
                self.authorization()
                    .ok_or(ClientError::InvalidRequest)?
                    .as_str(),
            )
            .header("x-content-sha256", sha256)
            .body(bytes);
        let work = async {
            let answer = request
                .send()
                .await
                .map_err(|e| crate::client::transport(&e))?;
            if !answer.status().is_success() {
                return Err(crate::client::rejection(answer.status().as_u16(), answer).await);
            }
            response(answer, options).await
        };
        tokio::select! {biased;()=options.cancel.cancelled()=>Err(ClientError::Cancelled),
        result=tokio::time::timeout(options.timeout.unwrap_or(std::time::Duration::from_secs(60)),work)=>result.map_err(|_|ClientError::Timeout)?}
    }
    /// 重试入口始终是同一初始化请求，服务端回执丢失也不会另建文件。
    pub async fn upload_continue(
        &self,
        input: &Create,
        source: &OutgoingFile,
        options: &RequestOptions<'_>,
        mut progress: Option<crate::client::ProgressCallback<'_>>,
    ) -> Result<TreeFile, ClientError> {
        let mut upload = self.upload_create(input, options).await?;
        if upload.phase == Phase::Completed {
            return completed(upload, input);
        }
        if upload.phase != Phase::Receiving {
            return Err(ClientError::Http {
                status: 409,
                remote_code: Some(
                    if upload.phase == Phase::Expired {
                        "UPLOAD_EXPIRED"
                    } else {
                        "UPLOAD_CANCELLED"
                    }
                    .into(),
                ),
            });
        }
        if upload.part_bytes != wire::PART_BYTES
            || !is_valid_id(&upload.id)
            || upload.parts.len() > 48
        {
            return Err(ClientError::InvalidResponse);
        }
        let actual = OutgoingFile::open(&source.path, options.cancel).await?;
        if actual.size != input.size || actual.sha256 != input.sha256 {
            return Err(ClientError::LocalSource);
        }
        let mut file = tokio::fs::File::open(&source.path)
            .await
            .map_err(|_| ClientError::LocalSource)?;
        let mut confirmed = 0_u64;
        for part in &upload.parts {
            if wire::part_size(input.size, part.index) != Some(part.size)
                || upload
                    .parts
                    .iter()
                    .filter(|p| p.index == part.index)
                    .count()
                    != 1
            {
                return Err(ClientError::InvalidResponse);
            }
            confirmed += part.size;
        }
        if let Some(callback) = progress.as_mut() {
            callback(TransferProgress {
                transferred: confirmed,
                total: input.size,
            });
        }
        for index in 0..wire::part_count(input.size).ok_or(ClientError::InvalidRequest)? {
            if options.cancel.is_cancelled() {
                return Err(ClientError::Cancelled);
            }
            let length = wire::part_size(input.size, index).ok_or(ClientError::InvalidRequest)?;
            let mut bytes =
                vec![0; usize::try_from(length).map_err(|_| ClientError::InvalidRequest)?];
            file.read_exact(&mut bytes)
                .await
                .map_err(|_| ClientError::LocalSource)?;
            if let Some(known) = upload.parts.iter().find(|p| p.index == index) {
                let digest = assemble_protocol::primitives::to_hex(&Sha256::digest(&bytes));
                if digest != known.sha256 {
                    return Err(ClientError::LocalSource);
                }
                continue;
            }
            let reply = self.upload_part(&upload.id, index, bytes, options).await?;
            if reply.id != upload.id
                || reply.phase != Phase::Receiving
                || !reply
                    .parts
                    .iter()
                    .any(|p| p.index == index && p.size == length)
            {
                return Err(ClientError::InvalidResponse);
            }
            confirmed += length;
            if let Some(callback) = progress.as_mut() {
                callback(TransferProgress {
                    transferred: confirmed,
                    total: input.size,
                });
            }
        }
        let id = upload.id.clone();
        upload = self.upload_complete(&id, options).await?;
        if upload.id != id {
            return Err(ClientError::InvalidResponse);
        }
        completed(upload, input)
    }
}

fn completed(upload: Upload, input: &Create) -> Result<TreeFile, ClientError> {
    let file = upload.file.ok_or(ClientError::InvalidResponse)?;
    if upload.phase != Phase::Completed
        || !is_valid_id(&file.file.id)
        || file.file.name != input.name
        || file.file.size != input.size
        || file.file.sha256 != input.sha256
        || file.version < 1
        || file.parent_id.as_deref() != assemble_protocol::drive_tree::parent_id(&input.parent_id)
    {
        return Err(ClientError::InvalidResponse);
    }
    Ok(file)
}
