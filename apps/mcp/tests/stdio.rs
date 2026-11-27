//! 端到端：像一个真实的 MCP 客户端那样，经协议调用工具。
//!
//! 先在同一个进程里用一条内存里的双向流走一遍（登录存在内存里，不碰系统钥匙串）；
//! 再起一个真正的 `assemble-mcp` 子进程做冒烟：握手、列出工具、调用一次。
//! 子进程那一半只读系统钥匙串里有没有登录，不写。

mod support;

use std::sync::Arc;

use assemble_mcp::server::{self, Assistant};
use assemble_mcp::session::{Session, Stored};
use assemble_store::secret::MemorySecretStore;
use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::{RoleClient, RunningService};
use rmcp::transport::child_process::TokioChildProcess;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use support::*;

const TOOLS: [&str; 18] = [
    "assemble_download_preview",
    "assemble_download_start",
    "assemble_download_status",
    "assemble_memos",
    "assemble_memo_get",
    "assemble_memo_create",
    "assemble_memo_update",
    "assemble_memo_delete",
    "assemble_planner",
    "assemble_schedule_create",
    "assemble_schedule_update",
    "assemble_schedule_set_status",
    "assemble_schedule_delete",
    "assemble_reminder_create",
    "assemble_reminder_update",
    "assemble_reminder_set_status",
    "assemble_reminder_delete",
    "assemble_notifications_read",
];

type Client = RunningService<RoleClient, ()>;

/// 工具名按字母排好，用来和清单比。
fn sorted_tools() -> Vec<String> {
    let mut names: Vec<String> = TOOLS.iter().map(|tool| (*tool).to_owned()).collect();
    names.sort();
    names
}

/// 调一个工具，把它说的话取出来：是不是报错，说了什么。
async fn call(client: &Client, name: &str, arguments: Option<serde_json::Value>) -> (bool, String) {
    let mut request = CallToolRequestParams::new(name.to_owned());
    if let Some(arguments) = arguments {
        request = request.with_arguments(arguments.as_object().unwrap().clone());
    }
    let result: CallToolResult = client
        .call_tool(request)
        .await
        .unwrap_or_else(|error| panic!("{name} should answer: {error}"));
    let said: Vec<String> = result
        .content
        .iter()
        .filter_map(|block| {
            serde_json::to_value(block)
                .ok()
                .filter(|value| value["type"] == "text")
                .and_then(|value| value["text"].as_str().map(str::to_owned))
        })
        .collect();
    (result.is_error.unwrap_or(false), said.join("\n"))
}

/// 起一个连着这个服务端的入口，另一头接上一个客户端。
async fn connected(
    origin: assemble_protocol::origin::ServerOrigin,
    store: Arc<MemorySecretStore>,
) -> Client {
    // 一条内存里的双向流，两头各接一方：和真实客户端经标准输入输出连进来是同一条协议。
    let (client_side, server_side) = tokio::io::duplex(64 * 1024);
    let assistant = Assistant::new(Session::new(origin, ASSISTANT, store));
    tokio::spawn(async move {
        let (read, write) = tokio::io::split(server_side);
        let _ = server::serve_on(assistant, read, write).await;
    });
    let (read, write) = tokio::io::split(client_side);
    ().serve((read, write))
        .await
        .expect("the server answers the handshake")
}

/// 服务端上这个账户现在有几条日程。
async fn schedules_on_the_server(fixture: &Fixture) -> usize {
    let browser = browser(&fixture.server);
    common::log_in(&browser).await;
    let device = register(&fixture.server, &browser, 'p', "别的设备", common::USERNAME).await;
    device
        .planner(&options(&CancellationToken::new()))
        .await
        .unwrap()
        .schedules
        .len()
}

/// 以助手的身份在服务端登记一台设备，把它的登录放进内存里的存储：等价于这台助手登录过。
async fn signed_up_assistant(fixture: &Fixture) -> (Arc<MemorySecretStore>, String) {
    let store = Arc::new(MemorySecretStore::new());
    let device = register(
        &fixture.server,
        &browser(&fixture.server),
        'p',
        ASSISTANT,
        common::USERNAME,
    )
    .await;
    let me = device.me(&device.options()).await.unwrap();
    let device_id = me.device.id.clone();
    Session::new(fixture.server.origin.clone(), ASSISTANT, store.clone())
        .save(&Stored {
            token: device.token.clone(),
            account: me.account,
            device: me.device,
        })
        .unwrap();
    (store, device_id)
}

#[tokio::test]
async fn the_client_is_told_what_this_is_and_which_tools_exist() {
    let fixture = fixture().await;
    let client = connected(
        fixture.server.origin.clone(),
        Arc::new(MemorySecretStore::new()),
    )
    .await;
    let info = client.peer_info().expect("the server introduced itself");
    assert_eq!(info.server_info.as_ref().unwrap().name, "assemble");
    assert!(info.capabilities.tools.is_some());
    let instructions = info
        .instructions
        .as_ref()
        .expect("the server explains itself");
    assert!(instructions.contains("assemble_planner"), "{instructions}");
    assert!(instructions.contains("+08:00"), "{instructions}");

    let listed = client.list_tools(None).await.unwrap().tools;
    let mut names: Vec<String> = listed
        .iter()
        .map(|tool| tool.name.as_ref().to_owned())
        .collect();
    names.sort();
    assert_eq!(names, sorted_tools());
    // 每个工具都说得清自己是干什么的、要什么参数。
    for tool in &listed {
        assert!(
            !tool.description.as_deref().unwrap_or("").is_empty(),
            "{}",
            tool.name
        );
        assert_eq!(tool.input_schema["type"], "object", "{}", tool.name);
    }
    // 客户端靠这些标注决定要不要先问用户：只读的、会删东西的、重复调用无妨的，各自标明。
    let hint = |name: &str| {
        let tool = listed.iter().find(|tool| tool.name == name).unwrap();
        let annotations = tool.annotations.as_ref().expect(name);
        (
            annotations.read_only_hint,
            annotations.destructive_hint,
            annotations.idempotent_hint,
        )
    };
    assert_eq!(hint("assemble_planner"), (Some(true), None, None));
    assert_eq!(
        hint("assemble_schedule_create"),
        (Some(false), Some(false), None)
    );
    assert_eq!(hint("assemble_schedule_delete"), (None, Some(true), None));
    assert_eq!(
        hint("assemble_notifications_read"),
        (None, Some(false), Some(true))
    );
    assert_eq!(
        hint("assemble_schedule_set_status"),
        (None, None, Some(true))
    );
}

#[tokio::test]
async fn without_a_stored_session_the_tools_say_so_instead_of_failing() {
    let fixture = fixture().await;
    let client = connected(
        fixture.server.origin.clone(),
        Arc::new(MemorySecretStore::new()),
    )
    .await;
    let (is_error, text) = call(&client, "assemble_planner", None).await;
    assert!(is_error);
    assert!(text.contains("还没有登录 Assemble"), "{text}");
    assert!(text.contains("assemble-mcp login"), "{text}");
    // 不写任何令牌，也不写钥匙串里的条目名。
    assert!(!text.contains("assemble.mcp-session"), "{text}");
}

#[tokio::test]
async fn a_stored_session_lets_the_tools_work_through_the_protocol() {
    let fixture = fixture().await;
    let (store, _) = signed_up_assistant(&fixture).await;
    let client = connected(fixture.server.origin.clone(), store).await;

    let (is_error, text) = call(&client, "assemble_planner", None).await;
    assert!(!is_error, "{text}");
    assert!(
        text.contains("你在账户里是一台叫“ZCode 的助手”的设备。"),
        "{text}"
    );

    // 建一条日程，从名单里找到它，再删掉。
    let (is_error, created) = call(
        &client,
        "assemble_schedule_create",
        Some(json!({ "title": "火车", "start": tomorrow_nine(), "notes": "带身份证" })),
    )
    .await;
    assert!(!is_error, "{created}");
    assert!(created.starts_with("已保存：火车 [id "), "{created}");
    let id = id_of(&created);
    let (_, listed) = call(&client, "assemble_planner", None).await;
    assert!(listed.contains(&format!("- 火车 [id {id} v1]")), "{listed}");
    assert!(listed.contains("ZCode 的助手记下的"), "{listed}");
    let (is_error, removed) = call(
        &client,
        "assemble_schedule_delete",
        Some(json!({ "id": id, "version": 1 })),
    )
    .await;
    assert!(!is_error, "{removed}");
    assert_eq!(removed, "已删除日程「火车」。");
    assert_eq!(schedules_on_the_server(&fixture).await, 0);
}

#[tokio::test]
async fn arguments_that_do_not_fit_the_tool_are_turned_away() {
    let fixture = fixture().await;
    let (store, _) = signed_up_assistant(&fixture).await;
    let client = connected(fixture.server.origin.clone(), store).await;

    // 多出来的字段：协议层按参数的形状拦下了，服务端什么也没收到。
    let (is_error, text) = call(
        &client,
        "assemble_schedule_create",
        Some(json!({ "title": "火车", "start": tomorrow_nine(), "colour": "red" })),
    )
    .await;
    assert!(is_error, "{text}");
    // 时间没带时区：工具自己拦下，说的是给助手听的话。
    let (is_error, text) = call(
        &client,
        "assemble_schedule_create",
        Some(json!({ "title": "火车", "start": "2026-11-26T09:00:00" })),
    )
    .await;
    assert!(is_error, "{text}");
    assert!(text.contains("没有带时区"), "{text}");
    // 两次都没有建出任何东西。
    assert_eq!(schedules_on_the_server(&fixture).await, 0);

    // 不存在的工具：协议层直接报错，不会悄悄当成成功。
    let missing = client
        .call_tool(CallToolRequestParams::new("assemble_teleport"))
        .await;
    assert!(missing.is_err());
}

#[tokio::test]
async fn a_login_withdrawn_at_the_server_is_reported_as_such() {
    let fixture = fixture().await;
    let (store, device_id) = signed_up_assistant(&fixture).await;
    let client = connected(fixture.server.origin.clone(), store).await;
    let (_, listed) = call(&client, "assemble_planner", None).await;
    assert!(listed.contains("Alex Wang"), "{listed}");

    // 在另一台设备上把这台助手从账户里移除了。
    let browser = browser(&fixture.server);
    common::log_in(&browser).await;
    let other = register(&fixture.server, &browser, 'p', "别的设备", common::USERNAME).await;
    other
        .remove_device(&device_id, &options(&CancellationToken::new()))
        .await
        .unwrap();

    // 入口不攥着一份失效的登录不放：如实告诉助手登录被收回了。
    let (is_error, text) = call(&client, "assemble_planner", None).await;
    assert!(is_error, "{text}");
    assert!(text.contains("登录已经被收回"), "{text}");
    assert!(text.contains("assemble-mcp login"), "{text}");
}

#[tokio::test]
async fn the_real_binary_shakes_hands_over_stdio() {
    // 这个地址上没有人监听：握手和列出工具用不到服务端。
    // 调试构建才认这个环境变量，所以它不会出现在发布构建里。
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_assemble-mcp"));
    command
        .args(["serve", "--name", "冒烟测试"])
        .env("ASSEMBLE_MCP_ORIGIN", "http://127.0.0.1:9")
        .env("RUST_LOG", "warn");
    let client =
        ().serve(TokioChildProcess::new(command).unwrap())
            .await
            .expect("the binary answers the handshake over stdio");

    let listed = client.list_tools(None).await.unwrap().tools;
    let mut names: Vec<String> = listed
        .iter()
        .map(|tool| tool.name.as_ref().to_owned())
        .collect();
    names.sort();
    assert_eq!(names, sorted_tools());

    // 本机没有这台助手的登录：如实地让用户去登录。这一步只读系统钥匙串，不写。
    let (is_error, text) = call(&client, "assemble_planner", None).await;
    assert!(is_error, "{text}");
    assert!(text.contains("assemble-mcp"), "{text}");
}
