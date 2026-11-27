//! 远程桌面只走固定来源的 Rust 请求管线，设备凭据从不交给画面窗口。
use crate::{Client, ClientError, RequestOptions};
use assemble_protocol::remote::{
    RemoteBatch, RemoteFrame, RemotePoll, RemoteRequest, RemoteSession, RemoteView,
};
use assemble_protocol::remote::{RemoteEnd, RemoteEndReason};
use reqwest::Method;
use serde::{Serialize, de::DeserializeOwned};

fn path(id: &str, action: &str) -> Result<String, ClientError> {
    if !assemble_protocol::primitives::is_valid_id(id) || matches!(id, "." | "..") {
        return Err(ClientError::InvalidRequest);
    }
    Ok(format!("/v1/remote/sessions/{id}/{action}"))
}
impl Client {
    async fn remote_call(
        &self,
        method: Method,
        path: &str,
        query: Vec<(&'static str, String)>,
        body: Option<serde_json::Value>,
        options: &RequestOptions<'_>,
    ) -> Result<reqwest::Response, ClientError> {
        if !self.has_bearer() {
            return Err(ClientError::InvalidRequest);
        }
        self.json_request(method, path, query, body, options).await
    }
    async fn remote_json<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        query: Vec<(&'static str, String)>,
        body: Option<serde_json::Value>,
        options: &RequestOptions<'_>,
    ) -> Result<T, ClientError> {
        let response = self.remote_call(method, path, query, body, options).await?;
        let bytes = crate::media::bounded_body(
            response,
            assemble_protocol::remote::MAX_FRAME_BASE64 + 64 * 1024,
            options,
        )
        .await?;
        serde_json::from_slice(&bytes).map_err(|_| ClientError::InvalidResponse)
    }
    pub async fn remote_sessions(
        &self,
        options: &RequestOptions<'_>,
    ) -> Result<RemoteView, ClientError> {
        self.remote_json(Method::GET, "/v1/remote/sessions", vec![], None, options)
            .await
    }
    pub async fn remote_request(
        &self,
        input: &RemoteRequest,
        options: &RequestOptions<'_>,
    ) -> Result<RemoteSession, ClientError> {
        self.remote_json(
            Method::POST,
            "/v1/remote/sessions",
            vec![],
            Some(payload(input)?),
            options,
        )
        .await
    }
    pub async fn remote_accept(
        &self,
        id: &str,
        options: &RequestOptions<'_>,
    ) -> Result<RemoteSession, ClientError> {
        self.remote_json(Method::POST, &path(id, "accept")?, vec![], None, options)
            .await
    }
    pub async fn remote_end(
        &self,
        id: &str,
        options: &RequestOptions<'_>,
    ) -> Result<(), ClientError> {
        self.remote_call(Method::POST, &path(id, "end")?, vec![], None, options)
            .await?;
        Ok(())
    }
    pub async fn remote_end_with_reason(
        &self,
        id: &str,
        reason: RemoteEndReason,
        options: &RequestOptions<'_>,
    ) -> Result<(), ClientError> {
        self.remote_call(
            Method::POST,
            &path(id, "end")?,
            vec![],
            Some(payload(&RemoteEnd { reason })?),
            options,
        )
        .await?;
        Ok(())
    }
    pub async fn remote_poll(
        &self,
        id: &str,
        after: u32,
        frame_after: u32,
        options: &RequestOptions<'_>,
    ) -> Result<RemotePoll, ClientError> {
        let result: RemotePoll = self
            .remote_json(
                Method::GET,
                &path(id, "poll")?,
                vec![
                    ("after", after.to_string()),
                    ("frameAfter", frame_after.to_string()),
                ],
                None,
                options,
            )
            .await?;
        if result.session.id != id
            || result.frame.as_ref().is_some_and(|f| !f.valid())
            || result.inputs.len() > assemble_protocol::remote::MAX_BATCH
            || result.inputs.iter().any(|e| !e.input.valid())
        {
            return Err(ClientError::InvalidResponse);
        }
        Ok(result)
    }
    pub async fn remote_frame(
        &self,
        id: &str,
        frame: &RemoteFrame,
        options: &RequestOptions<'_>,
    ) -> Result<(), ClientError> {
        if !frame.valid() {
            return Err(ClientError::InvalidRequest);
        }
        self.remote_call(
            Method::POST,
            &path(id, "frame")?,
            vec![],
            Some(payload(frame)?),
            options,
        )
        .await?;
        Ok(())
    }
    pub async fn remote_inputs(
        &self,
        id: &str,
        batch: &RemoteBatch,
        options: &RequestOptions<'_>,
    ) -> Result<(), ClientError> {
        if batch.events.is_empty()
            || batch.events.len() > assemble_protocol::remote::MAX_BATCH
            || batch.events.iter().any(|e| !e.input.valid())
        {
            return Err(ClientError::InvalidRequest);
        }
        self.remote_call(
            Method::POST,
            &path(id, "inputs")?,
            vec![],
            Some(payload(batch)?),
            options,
        )
        .await?;
        Ok(())
    }
}
fn payload(value: &impl Serialize) -> Result<serde_json::Value, ClientError> {
    serde_json::to_value(value).map_err(|_| ClientError::InvalidRequest)
}
