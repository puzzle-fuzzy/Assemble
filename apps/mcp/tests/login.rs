//! 登录这条路对着真实运行的服务端走一遍：授权页、回环回调、换码、存进存储、撤销。
//!
//! "浏览器"是一个会存 Cookie 的 HTTP 客户端；存储是内存里的，不碰系统钥匙串。

mod support;

use std::sync::Arc;

use assemble_client::Client;
use assemble_mcp::browser::BrowserError;
use assemble_mcp::planning::Now;
use assemble_mcp::session::{
    self, LoginFailure, LogoutFailure, Session, SessionError, SessionSource, Status, Stored,
};
use assemble_mcp::tools::{self, ScheduleInput};
use assemble_protocol::device::NewDevice;
use assemble_protocol::primitives::Platform;
use assemble_server::http::TestServer;
use assemble_store::secret::{MemorySecretStore, SecretKey, SecretStore, SecretString};
use tokio_util::sync::CancellationToken;

use support::*;

fn store() -> Arc<MemorySecretStore> {
    Arc::new(MemorySecretStore::new())
}

fn session(server: &TestServer, store: Arc<dyn SecretStore>) -> Session {
    Session::new(server.origin.clone(), ASSISTANT, store)
}

fn device() -> NewDevice {
    NewDevice {
        name: ASSISTANT.into(),
        platform: Platform::Macos,
    }
}

/// 浏览器里那个人：已经登录，打开授权页，点 `choice` 那个按钮，
/// 再照着跳转地址访问本机正在监听的回调。
async fn sign_in(
    server: &TestServer,
    session: &Session,
    choice: &'static str,
) -> Result<Stored, LoginFailure> {
    let browser = browser(server);
    common::log_in(&browser).await;
    // 跟着跳转地址走的那一半"浏览器"：跳转目标是回环地址上的回调。
    let follower = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let prefix = format!("{}/desktop/authorize?request=", server.origin.as_str());
    session::sign_in_fresh(
        session,
        device(),
        move |url: String| async move {
            let encoded = url
                .strip_prefix(&prefix)
                .expect("the address points at the authorization page");
            let request = assemble_server::desktop_login::parse_request(encoded)
                .expect("the page can decode the request");
            let page = common::send(
                browser.get(&assemble_server::desktop_login::authorize_path(&request)),
            )
            .await;
            assert!(
                page.html.contains(ASSISTANT),
                "the page names the device:\n{}",
                page.html
            );
            let confirmed = common::authorize(&browser, &request, choice).await;
            let location = confirmed.location().expect("the page redirects");
            assert!(location.starts_with("http://127.0.0.1:"), "{location}");
            // 回调那头必须已经在监听：要是没人应答，十秒之后就当卡住了，而不是永远等。
            let landed = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                follower.get(&location).send(),
            )
            .await
            .expect("the loopback callback answers")
            .unwrap();
            assert!(
                landed.status().is_success(),
                "the callback answers {}",
                landed.status()
            );
            Ok::<(), BrowserError>(())
        },
        &CancellationToken::new(),
    )
    .await
}

/// 这条令牌服务端还认不认。
async fn token_is_alive(server: &TestServer, token: &SecretString) -> bool {
    let client = Client::new(server.origin.clone()).unwrap();
    client.set_bearer(Some(token.clone()));
    let answer = client.me(&options(&CancellationToken::new())).await;
    !answer.is_err_and(|error| error.is_unauthorized())
}

async fn signed_in(session: &Session) -> assemble_mcp::session::SignedIn {
    SessionSource::signed_in(session).await.unwrap()
}

#[tokio::test]
async fn confirming_in_the_browser_stores_a_session_the_assistant_can_act_with() {
    let fixture = fixture().await;
    let session = session(&fixture.server, store());
    let cancel = CancellationToken::new();

    // 还没登录：拿不到客户端，状态如实说没登录。
    assert!(matches!(
        session::status(&session, &cancel).await,
        Ok(Status::SignedOut)
    ));
    assert!(matches!(
        SessionSource::signed_in(&session).await,
        Err(SessionError::NotLoggedIn)
    ));

    // 本来就没登录：退出不报错，但如实说没有退掉什么。
    assert!(!session::logout(&session, &cancel).await.unwrap());

    let stored = sign_in(&fixture.server, &session, "allow").await.unwrap();
    assert_eq!(stored.device.name, ASSISTANT);
    assert_eq!(stored.account.username, common::USERNAME);

    // 存储里有了一份，核对下来是已登录；账户里多了一台设备，它不连事件流，所以是离线的。
    assert!(session.load().is_ok());
    assert!(matches!(
        session::status(&session, &cancel).await,
        Ok(Status::SignedIn { .. })
    ));
    let devices = signed_in(&session)
        .await
        .client
        .devices(&options(&cancel))
        .await
        .unwrap();
    let listed = devices
        .iter()
        .find(|device| device.name == ASSISTANT)
        .expect("the assistant is a device of the account");
    assert!(!listed.online);

    // 用这份登录真的能干活：建出来的日程记在这台助手名下。
    let signed = signed_in(&session).await;
    let call = tools::Call {
        signed_in: &signed,
        cancel: &cancel,
        now: Now::at(chrono::Local::now()),
    };
    let content: ScheduleInput = serde_json::from_value(serde_json::json!({
        "title": "火车", "start": tomorrow_nine()
    }))
    .unwrap();
    let created = tools::create_schedule(&call, &content).await.unwrap();
    assert!(created.contains("· ZCode 的助手记下的"), "{created}");

    // 退出：服务端撤销之后，本机的登录也没了，旧令牌不再有效。
    let token = session.load().unwrap().token;
    assert!(session::logout(&session, &cancel).await.unwrap());
    assert!(matches!(session.load(), Err(SessionError::NotLoggedIn)));
    assert!(!token_is_alive(&fixture.server, &token).await);
    assert!(matches!(
        session::status(&session, &cancel).await,
        Ok(Status::SignedOut)
    ));
}

#[tokio::test]
async fn declining_in_the_browser_stores_nothing_and_leaves_no_device_behind() {
    let fixture = fixture().await;
    let session = session(&fixture.server, store());

    let failure = sign_in(&fixture.server, &session, "cancel")
        .await
        .unwrap_err();
    assert!(
        matches!(
            failure,
            LoginFailure::Callback(assemble_client::LoginError::Denied)
        ),
        "{failure:?}"
    );
    assert!(matches!(session.load(), Err(SessionError::NotLoggedIn)));
    let devices = register(
        &fixture.server,
        &browser(&fixture.server),
        'p',
        "本机",
        common::USERNAME,
    )
    .await
    .devices(&options(&CancellationToken::new()))
    .await
    .unwrap();
    assert!(
        devices.iter().all(|device| device.name != ASSISTANT),
        "{devices:?}"
    );
}

#[tokio::test]
async fn signing_in_again_retires_the_old_device_first() {
    let fixture = fixture().await;
    let session = session(&fixture.server, store());
    let first = sign_in(&fixture.server, &session, "allow").await.unwrap();
    let second = sign_in(&fixture.server, &session, "allow").await.unwrap();

    // 换了一台新的：旧令牌作废，账户里只有一台叫这个名字的设备。
    assert_ne!(first.device.id, second.device.id);
    assert_ne!(first.token.expose(), second.token.expose());
    assert!(!token_is_alive(&fixture.server, &first.token).await);
    assert!(token_is_alive(&fixture.server, &second.token).await);
    let devices = signed_in(&session)
        .await
        .client
        .devices(&options(&CancellationToken::new()))
        .await
        .unwrap();
    assert_eq!(
        devices
            .iter()
            .filter(|device| device.name == ASSISTANT)
            .count(),
        1
    );
}

#[tokio::test]
async fn when_the_server_cannot_confirm_the_revocation_the_stored_session_stays() {
    let mut fixture = fixture().await;
    let session = session(&fixture.server, store());
    sign_in(&fixture.server, &session, "allow").await.unwrap();
    fixture.server.stop().await;
    let cancel = CancellationToken::new();

    // 服务端没有确认撤销：不退，本机的登录原样留着。
    let failure = session::logout(&session, &cancel).await.unwrap_err();
    assert!(
        matches!(failure, LogoutFailure::Unreachable(_)),
        "{failure:?}"
    );
    assert!(session.load().is_ok());
    assert!(matches!(
        session::status(&session, &cancel).await,
        Ok(Status::Unreachable { .. })
    ));

    // 旧的没能撤销，也不开始新的登录。
    let again = session::sign_in_fresh(
        &session,
        device(),
        |_| async { Ok::<(), BrowserError>(()) },
        &cancel,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(again, LoginFailure::Previous(LogoutFailure::Unreachable(_))),
        "{again:?}"
    );
    assert!(session.load().is_ok());
}

#[tokio::test]
async fn a_session_revoked_elsewhere_is_reported_as_expired_and_does_not_block_a_new_login() {
    let fixture = fixture().await;
    let session = session(&fixture.server, store());
    let stored = sign_in(&fixture.server, &session, "allow").await.unwrap();
    let cancel = CancellationToken::new();
    // 在另一台设备上把它从账户里移除了。
    let other = register(
        &fixture.server,
        &browser(&fixture.server),
        'p',
        "本机",
        common::USERNAME,
    )
    .await;
    other
        .remove_device(&stored.device.id, &options(&CancellationToken::new()))
        .await
        .unwrap();

    assert!(matches!(
        session::status(&session, &cancel).await,
        Ok(Status::Expired { device }) if device.name == ASSISTANT
    ));
    // 退出不碍事：服务端已经不认它，照样把本机的忘掉。
    session::logout(&session, &cancel).await.unwrap();
    assert!(matches!(session.load(), Err(SessionError::NotLoggedIn)));
}

#[tokio::test]
async fn a_broken_record_is_reported_as_such_and_is_not_forgotten() {
    let fixture = fixture().await;
    let store = store();
    // 条目名和桌面应用用的不是同一个：这里写的是助手自己的那一个。
    let key = SecretKey::new(
        format!("assemble.mcp-session:{}", fixture.server.origin),
        ASSISTANT,
    );
    store
        .put(&key, &SecretString::new("not a session".to_owned()))
        .unwrap();
    let session = session(&fixture.server, store);
    let cancel = CancellationToken::new();

    assert!(matches!(
        session::status(&session, &cancel).await,
        Err(SessionError::Corrupt)
    ));
    // 读不出令牌就没法让服务端撤销：不删，让人去设备列表里移除。
    assert!(matches!(
        session::logout(&session, &cancel).await,
        Err(LogoutFailure::Session(SessionError::Corrupt))
    ));
    assert!(matches!(session.load(), Err(SessionError::Corrupt)));
}
