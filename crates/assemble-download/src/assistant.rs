//! MCP 只通过桌面进程已公布的钥匙串入口请求下载，不启动应用也不直接修改任务文件。
use assemble_protocol::browser::BrowserDownload;
use assemble_protocol::downloads::{
    AssistantDownloadReply as Reply, AssistantDownloadStart, DownloadInitiator, DownloadInput,
};
use assemble_protocol::primitives::is_valid_id;
use assemble_store::secret::SecretStore;

pub const DESKTOP_APP_ID: &str = "com.puzzlefuzzy.assemble";

async fn exchange(
    store: &dyn SecretStore,
    app_id: &str,
    method: reqwest::Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> Reply {
    match crate::browser::local_request(store, app_id, method, path, body).await {
        Ok(value) => match serde_json::from_value::<Reply>(value) {
            Ok(reply) if reply.version == 1 => reply,
            _ => Reply::failure(crate::browser::NOT_CONNECTED),
        },
        Err(code) => Reply::failure(code),
    }
}

pub async fn preview(store: &dyn SecretStore, app_id: &str, input: DownloadInput) -> Reply {
    let request = BrowserDownload {
        request_id: "preview".into(),
        input,
        via: Some(DownloadInitiator::Assistant),
    };
    let Ok(body) = serde_json::to_value(request) else {
        return Reply::failure(crate::browser::INVALID_REQUEST);
    };
    exchange(
        store,
        app_id,
        reqwest::Method::POST,
        "/v1/assistant/downloads/preview",
        Some(body),
    )
    .await
}

pub async fn start(store: &dyn SecretStore, app_id: &str, input: &AssistantDownloadStart) -> Reply {
    let Ok(body) = serde_json::to_value(input) else {
        return Reply::failure(crate::browser::INVALID_REQUEST);
    };
    exchange(
        store,
        app_id,
        reqwest::Method::POST,
        "/v1/assistant/downloads/start",
        Some(body),
    )
    .await
}

pub async fn status(store: &dyn SecretStore, app_id: &str, id: &str) -> Reply {
    if !is_valid_id(id) || id.len() > 64 {
        return Reply::failure(crate::browser::INVALID_REQUEST);
    }
    exchange(
        store,
        app_id,
        reqwest::Method::GET,
        &format!("/v1/downloads/{id}"),
        None,
    )
    .await
}

pub async fn cancel(store: &dyn SecretStore, app_id: &str, id: &str) -> Reply {
    if !is_valid_id(id) || id.len() > 64 {
        return Reply::failure(crate::browser::INVALID_REQUEST);
    }
    exchange(
        store,
        app_id,
        reqwest::Method::POST,
        &format!("/v1/downloads/{id}/cancel"),
        None,
    )
    .await
}

pub async fn pause(store: &dyn SecretStore, app_id: &str, id: &str) -> Reply {
    control(store, app_id, id, "pause").await
}
pub async fn resume(store: &dyn SecretStore, app_id: &str, id: &str) -> Reply {
    control(store, app_id, id, "resume").await
}
async fn control(store: &dyn SecretStore, app_id: &str, id: &str, action: &str) -> Reply {
    if !is_valid_id(id) || id.len() > 64 {
        return Reply::failure(crate::browser::INVALID_REQUEST);
    }
    exchange(
        store,
        app_id,
        reqwest::Method::POST,
        &format!("/v1/downloads/{id}/{action}"),
        None,
    )
    .await
}
