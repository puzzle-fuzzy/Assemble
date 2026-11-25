//! 集成测试共用的东西：一个真实的服务端，以及替账户和设备走完登录的工具。
//!
//! 起服务、当"浏览器"，直接借用服务端集成测试的那一套。

// 每个测试文件只会用到其中一部分工具，所以在这里统一放过"未被使用"的提示。
#![allow(dead_code, reason = "每个测试文件只用到共用工具的一部分")]

#[path = "../../../server/tests/common/mod.rs"]
pub mod common;

use std::time::Duration;

use assemble_client::{Client, RequestOptions};
use assemble_mcp::planning;
use assemble_protocol::primitives::{Platform, Timestamp};
use assemble_protocol::schedule::{Repeat, ScheduleContent};
use assemble_server::http::TestServer;
use assemble_store::secret::SecretString;
use chrono::TimeZone;
use tokio_util::sync::CancellationToken;

/// 助手在账户里的名字。
pub const ASSISTANT: &str = "ZCode 的助手";

pub const TIMEOUT: Duration = Duration::from_secs(20);

/// 一个真实的服务端，里面已经有一个账户 `alex`。
pub struct Fixture {
    /// 临时目录必须活得比服务久：放掉它，数据库文件就没了。
    pub _dir: tempfile::TempDir,
    pub server: TestServer,
}

pub async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let server = TestServer::start_default(dir.path()).await.unwrap();
    create_account(&server, common::USERNAME, "Alex Wang").await;
    Fixture { _dir: dir, server }
}

pub async fn create_account(server: &TestServer, username: &str, display_name: &str) {
    assemble_server::accounts::create(
        &server.state.pool,
        username,
        display_name,
        SecretString::new(common::PASSWORD.to_owned()),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE accounts SET email = username || '@example.test', email_verified_at = 1 WHERE username = ?").bind(username).execute(&server.state.pool).await.unwrap();
}

pub fn browser(server: &TestServer) -> common::Browser {
    common::Browser::new(server.origin.as_str())
}

/// 替一台设备走完"登录 → 授权 → 换码"。用哪个账户登录由 `username` 决定。
pub async fn register(
    server: &TestServer,
    browser: &common::Browser,
    seed: char,
    name: &str,
    username: &str,
) -> common::Registered {
    // 这个浏览器可能已经登录过：已经登录的登录页上没有防伪字段，所以 alex 走"登录或者已经登录"那一条。
    if username == common::USERNAME {
        common::log_in(browser).await;
    } else {
        common::try_login(browser, username, common::PASSWORD, "").await;
    }
    let (request, verifier) = common::desktop_request(seed, name, Platform::Macos);
    let code = common::code_from(
        &common::authorize(browser, &request, "allow").await,
        &request,
    );
    let client = Client::new(server.origin.clone()).unwrap();
    let cancel = CancellationToken::new();
    let (response, token) = client
        .exchange(
            &common::exchange_input(&request, &verifier, code),
            &RequestOptions::new(&cancel),
        )
        .await
        .unwrap();
    client.set_bearer(Some(token.clone()));
    common::Registered {
        client,
        token,
        device: response.device,
        cancel,
    }
}

/// 一个带超时的请求选项。
pub fn options(cancel: &CancellationToken) -> RequestOptions<'_> {
    RequestOptions {
        cancel,
        timeout: Some(TIMEOUT),
    }
}

/// 明天上午九点，按这台电脑的本地时间写，带时区。
pub fn tomorrow_nine() -> String {
    tomorrow_at(9, 0)
}

pub fn tomorrow_at(hour: u32, minute: u32) -> String {
    let date = chrono::Local::now().date_naive() + chrono::Duration::days(1);
    chrono::Local
        .from_local_datetime(&date.and_hms_opt(hour, minute, 0).unwrap())
        .unwrap()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}

pub fn at(text: &str) -> Timestamp {
    planning::parse_time(text).unwrap()
}

/// 结果里 `[id … vN]` 的那一段标识。
pub fn id_of(text: &str) -> String {
    text.split("[id ")
        .nth(1)
        .and_then(|rest| rest.split(' ').next())
        .unwrap()
        .to_owned()
}

/// 一条天天有的日程，从明天上午九点起。
pub fn daily(title: &str) -> ScheduleContent {
    ScheduleContent {
        title: title.into(),
        location: None,
        start_at: at(&tomorrow_nine()),
        end_at: None,
        repeat: Repeat::Daily,
        repeat_until: None,
        remind_before: None,
        notes: None,
    }
}
