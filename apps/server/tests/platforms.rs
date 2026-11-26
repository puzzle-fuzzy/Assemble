//! 设备的平台。手机是其中之一；这一版不认识的值也不能让设备名单读不出来。
//!
//! 这些行为要几方拼在一起才看得出来：授权页、换码、设备名单、发送。

mod common;

use assemble_client::{OutgoingFile, TransferOptions};
use assemble_protocol::primitives::Platform;
use assemble_protocol::sends::{Direction, SendQuery};
use common::*;
use reqwest::StatusCode;
use tokio_util::sync::CancellationToken;

fn http(status: u16, code: &str) -> assemble_client::ClientError {
    assemble_client::ClientError::Http {
        status,
        remote_code: Some(code.to_owned()),
    }
}

#[tokio::test]
async fn a_phone_signs_in_the_same_way_as_a_computer_and_joins_the_account() {
    let fixture = fixture().await;
    let mac = register(&fixture, 'm', "MacBook", Platform::Macos).await;
    let phone = register(&fixture, 'h', "我的 iPhone", Platform::Iphone).await;

    assert_eq!(phone.device.platform, Platform::Iphone);
    let devices = mac.devices(&mac.options()).await.unwrap();
    let listed = devices
        .iter()
        .find(|device| device.id == phone.device.id)
        .expect("the phone is in the account's device list");
    assert_eq!(listed.platform, Platform::Iphone);
    // 手机第一期不开事件流：在名单里是离线的。
    assert!(!listed.online);
}

#[tokio::test]
async fn the_authorize_page_names_the_platform_that_is_asking_to_sign_in() {
    let fixture = fixture().await;
    log_in(&fixture.browser).await;

    let (request, _verifier) = desktop_request('h', "我的 iPhone", Platform::Iphone);
    let page = send(
        fixture
            .browser
            .get(&assemble_server::desktop_login::authorize_path(&request)),
    )
    .await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.html.contains("我的 iPhone"));
    assert!(page.html.contains("iPhone"));

    let (request, _verifier) = desktop_request('u', "新设备", Platform::Other);
    let page = send(
        fixture
            .browser
            .get(&assemble_server::desktop_login::authorize_path(&request)),
    )
    .await;
    assert_eq!(page.status, StatusCode::OK);
    // 这里的代码不认识这个平台：如实说不知道，不猜一个名字。
    assert!(page.html.contains("未知平台"));
}

#[tokio::test]
async fn a_file_cannot_be_sent_to_a_phone() {
    let fixture = fixture().await;
    let pc = register(&fixture, 'p', "工作电脑", Platform::Windows).await;
    let phone = register(&fixture, 'h', "我的 iPhone", Platform::Iphone).await;
    let path = fixture.dir.path().join("合同.pdf");
    tokio::fs::write(&path, b"a contract").await.unwrap();
    let file = OutgoingFile::open(&path, &CancellationToken::new())
        .await
        .unwrap();

    let error = pc
        .send_to_device(
            &phone.device.id,
            &file,
            TransferOptions::new(&pc.cancel),
            None,
        )
        .await
        .unwrap_err();
    // 不是"对方不在线"：手机应用这一期收不了文件，说法要准。
    assert_eq!(error, http(400, "INVALID_REQUEST"));
    let sends = pc
        .sends(
            &SendQuery {
                direction: Direction::Out,
                state: None,
                limit: 10,
            },
            &pc.options(),
        )
        .await
        .unwrap();
    assert!(sends.is_empty(), "nothing was stored for a refused send");
}

#[tokio::test]
async fn a_platform_this_version_does_not_know_does_not_break_the_device_list() {
    let fixture = fixture().await;
    let mac = register(&fixture, 'm', "MacBook", Platform::Macos).await;
    let account = account_of(&fixture).await;
    // 更新版本的服务端写进去的一台设备，平台是这一版没见过的。
    sqlx::query(
        "INSERT INTO devices (id, account_id, name, platform, token_hash, created_at)
         VALUES ('from-the-future', ?, '将来的设备', 'visionos', 'not-a-real-token-hash', 0)",
    )
    .bind(&account)
    .execute(&fixture.server.state.pool)
    .await
    .unwrap();

    let devices = mac.devices(&mac.options()).await.unwrap();
    let future = devices
        .iter()
        .find(|device| device.name == "将来的设备")
        .expect("the device is still listed");
    assert_eq!(future.id, "from-the-future");
    assert_eq!(future.platform, Platform::Other);
    // 自己那一条和别人的都还在。
    assert!(devices.iter().any(|device| device.id == mac.device.id));
    // 自检也照常：它会把全库的设备读一遍。
    let me = mac.me(&mac.options()).await.unwrap();
    assert_eq!(me.device.id, mac.device.id);
}
