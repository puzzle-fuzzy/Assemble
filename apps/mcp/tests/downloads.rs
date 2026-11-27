//! 用假的桌面桥接走真正的挑战应答与 MCP 协议，不写系统钥匙串、不启动正式客户端。
use assemble_download::assistant::DESKTOP_APP_ID;
use assemble_mcp::server::{self, Assistant};
use assemble_mcp::session::Session;
use assemble_protocol::primitives::to_hex;
use assemble_store::secret::{MemorySecretStore, SecretKey, SecretStore, SecretString};
use axum::{
    Json, Router,
    extract::Query,
    http::HeaderMap,
    routing::{get, post},
};
use hmac::{Hmac, KeyInit, Mac};
use rmcp::{
    ServiceExt,
    model::{CallToolRequestParams, CallToolResult},
};
use serde_json::{Value, json};
use sha2::Sha256;
use std::sync::Arc;

const BRIDGE_SECRET: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

async fn challenge(
    Query(query): Query<std::collections::HashMap<String, String>>,
    headers: HeaderMap,
) -> Json<Value> {
    assert!(!headers.contains_key("authorization"));
    let mut mac = Hmac::<Sha256>::new_from_slice(BRIDGE_SECRET.as_bytes()).unwrap();
    mac.update(b"assemble-browser-server-v1:");
    mac.update(query["nonce"].as_bytes());
    Json(json!({"version":1,"proof":to_hex(&mac.finalize().into_bytes())}))
}

fn authenticated(headers: &HeaderMap) {
    assert_eq!(
        headers["authorization"].to_str().unwrap(),
        format!("Bearer {BRIDGE_SECRET}")
    );
    assert!(!headers.contains_key("origin"));
}

fn reply() -> Value {
    json!({"version":1,"ok":true,"preview":null,"task":null,"taskId":null,"errorCode":null})
}

async fn fake_preview(headers: HeaderMap, Json(body): Json<Value>) -> Json<Value> {
    authenticated(&headers);
    assert_eq!(body["via"], "assistant");
    assert_eq!(body["input"]["kind"], "hls");
    assert!(body["input"].get("path").is_none());
    let mut result = reply();
    result["preview"] = json!({"id":"preview-1","kind":"hls","title":"中文 <影片>","sourceHost":"example.test","durationSeconds":60,"variants":[{"id":"hd","width":1920,"height":1080,"bandwidth":10000}]});
    Json(result)
}

async fn fake_start(headers: HeaderMap, Json(body): Json<Value>) -> Json<Value> {
    authenticated(&headers);
    assert_eq!(
        body,
        json!({"requestId":"once","previewId":"preview-1","variantId":"hd"})
    );
    let mut result = reply();
    result["taskId"] = json!("task-1");
    Json(result)
}

async fn fake_status(headers: HeaderMap) -> Json<Value> {
    authenticated(&headers);
    let mut result = reply();
    result["task"] = json!({"id":"task-1","initiatedBy":"assistant","kind":"hls","title":"中文 <影片>","fileName":null,"sourceHost":"example.test","phase":"failed","receivedBytes":10,"totalBytes":100,"completedSegments":1,"totalSegments":5,"bytesPerSecond":0,"mediaSeconds":null,"durationSeconds":60,"errorCode":"DOWNLOAD_NETWORK","retryable":true,"createdAt":"2026-11-27T01:00:00Z"});
    Json(result)
}

async fn fake_completed(headers: HeaderMap) -> Json<Value> {
    let Json(mut result) = fake_status(headers).await;
    result["task"]["phase"] = json!("completed");
    result["task"]["fileName"] = json!("中文 <影片>.mp4");
    result["task"]["errorCode"] = Value::Null;
    // 内部路径即便误加到应答，协议投影也不能把它透传给助手。
    result["task"]["path"] = json!("C:/private/downloads/movie.mp4");
    Json(result)
}

async fn connected(
    store: Arc<MemorySecretStore>,
) -> (
    rmcp::service::RunningService<rmcp::service::RoleClient, ()>,
    tokio::task::JoinHandle<()>,
) {
    let session = Session::new(
        "https://example.test".parse().unwrap(),
        "test",
        store.clone(),
    );
    let assistant = Assistant::with_download_store(session, store.clone());
    let (client_side, server_side) = tokio::io::duplex(64 * 1024);
    let mcp = tokio::spawn(async move {
        let (read, write) = tokio::io::split(server_side);
        let _ = server::serve_on(assistant, read, write).await;
    });
    let (read, write) = tokio::io::split(client_side);
    let client = ().serve((read, write)).await.unwrap();
    (client, mcp)
}

async fn assert_download_cases(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, ()>,
) {
    let cases = [
        (
            "assemble_download_preview",
            json!({"kind":"hls","url":"https://example.test/video.m3u8"}),
            false,
            "中文 <影片>",
        ),
        (
            "assemble_download_start",
            json!({"requestId":"once","previewId":"preview-1","variantId":"hd"}),
            false,
            "task-1",
        ),
        (
            "assemble_download_status",
            json!({"id":"task-1"}),
            false,
            "DOWNLOAD_NETWORK",
        ),
        (
            "assemble_download_status",
            json!({"id":"task-complete"}),
            false,
            "中文 <影片>.mp4",
        ),
        (
            "assemble_download_status",
            json!({"id":"off"}),
            true,
            "关闭助手下载",
        ),
    ];
    for (name, args, error, expected) in cases {
        let result: CallToolResult = client
            .call_tool(
                CallToolRequestParams::new(name).with_arguments(args.as_object().unwrap().clone()),
            )
            .await
            .unwrap();
        assert_eq!(result.is_error.unwrap_or(false), error);
        let text = serde_json::to_string(&result).unwrap();
        assert!(text.contains(expected), "{text}");
        for forbidden in [
            BRIDGE_SECRET,
            "Bearer",
            "assemble.browser.bridge",
            "video.m3u8",
            "C:/private",
        ] {
            assert!(!text.contains(forbidden));
        }
    }
}

#[tokio::test]
async fn three_mcp_tools_use_the_authenticated_bridge_and_never_return_secrets() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let store = Arc::new(MemorySecretStore::new());
    store
        .put(
            &SecretKey::new("assemble.browser.bridge.v1", DESKTOP_APP_ID),
            &SecretString::new(
                json!({"port":listener.local_addr().unwrap().port(),"token":BRIDGE_SECRET})
                    .to_string(),
            ),
        )
        .unwrap();
    let router = Router::new()
        .route("/v1/challenge", get(challenge))
        .route("/v1/assistant/downloads/preview", post(fake_preview))
        .route("/v1/assistant/downloads/start", post(fake_start))
        .route("/v1/downloads/task-1", get(fake_status))
        .route("/v1/downloads/task-complete", get(fake_completed))
        .route(
            "/v1/downloads/off",
            get(|| async {
                let mut r = reply();
                r["ok"] = json!(false);
                r["errorCode"] = json!("DOWNLOAD_ASSISTANT_DISABLED");
                Json(r)
            }),
        );
    let http = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let (client, mcp) = connected(store.clone()).await;
    assert_download_cases(&client).await;
    // 额外字段被输入契约拒绝，不能把路径/请求头交给本机下载进程。
    assert!(
        serde_json::from_value::<assemble_mcp::downloads::Preview>(
            json!({"kind":"hls","url":"https://example.test/a","path":"C:/private"})
        )
        .is_err()
    );
    http.abort();
    let _ = http.await;
    let result = client
        .call_tool(
            CallToolRequestParams::new("assemble_download_status")
                .with_arguments(json!({"id":"task-1"}).as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    assert!(result.is_error.unwrap_or(false));
    assert!(
        serde_json::to_string(&result)
            .unwrap()
            .contains("桌面应用没有在运行")
    );
    client.cancel().await.unwrap();
    mcp.abort();
    let _ = mcp.await;
}

#[test]
fn errors_are_stable_and_do_not_echo_unknown_diagnostics() {
    let failure = assemble_protocol::downloads::AssistantDownloadReply::failure(
        "C:/private/session-token: secret diagnostic",
    );
    let message = assemble_mcp::downloads::result(&failure)
        .unwrap_err()
        .message();
    assert!(!message.contains("private"));
    assert!(!message.contains("secret"));
}
