//! 云盘的集成测试：对真实运行的服务走完整条链路。
//!
//! 这里核对的是几方拼起来的行为：空间怎么算、同一份内容存了几遍、
//! 发送与云盘共用一份内容时什么时候才真的删掉、别的账户看不看得到。

mod common;

use assemble_client::{Client, OutgoingFile, TransferOptions};
use assemble_protocol::device::{Device, NewDevice};
use assemble_protocol::drive::RenameInput;
use assemble_protocol::primitives::{MAX_FILE_SIZE, Platform, Timestamp};
use assemble_server::http::TestServer;
use assemble_store::secret::SecretString;
use common::*;
use reqwest::StatusCode;
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

// ---- 工具 ----

fn payload(length: usize) -> Vec<u8> {
    (0..length)
        .map(|index| u8::try_from(index % 251).expect("below 251"))
        .collect()
}

async fn outgoing(dir: &tempfile::TempDir, name: &str, bytes: &[u8]) -> OutgoingFile {
    let path = dir.path().join("outgoing").join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    tokio::fs::write(&path, bytes).await.unwrap();
    OutgoingFile::open(&path, &CancellationToken::new())
        .await
        .unwrap()
}

fn http(status: u16, code: &str) -> assemble_client::ClientError {
    assemble_client::ClientError::Http {
        status,
        remote_code: Some(code.to_owned()),
    }
}

async fn refcount(server: &TestServer, digest: &str) -> Option<i64> {
    sqlx::query_scalar::<_, i64>("SELECT refcount FROM blobs WHERE sha256 = ?")
        .bind(digest)
        .fetch_optional(&server.state.pool)
        .await
        .unwrap()
}

/// 旧删除入口现在只移入回收站；释放空间需要明确永久清理并完成内容回收。
async fn permanently_remove(fixture: &Fixture, pc: &Registered, id: &str) {
    pc.drive_delete(id, &pc.options()).await.unwrap();
    let listing = pc
        .drive_trash_list("", 1, 100, &pc.options())
        .await
        .unwrap();
    let batch = listing
        .items
        .iter()
        .find(|batch| batch.root_id == id)
        .unwrap();
    pc.drive_trash_purge(&batch.id, batch.version, &pc.options())
        .await
        .unwrap();
    fixture.server.state.blobs.cleanup().await.unwrap();
}

/// 登记另一台设备，不经浏览器：登记要写数据库，只在这里的测试环境里用。
async fn device_of_another_account(server: &TestServer, username: &str) -> Registered {
    let account = assemble_server::accounts::find_by_username(&server.state.pool, username)
        .await
        .unwrap()
        .expect("the account exists");
    let token = assemble_server::secrets::random_token().unwrap();
    let device = assemble_server::devices::register(
        &server.state.pool,
        &account.id,
        &NewDevice {
            name: "别的账户的电脑".into(),
            platform: Platform::Macos,
        },
        &assemble_server::secrets::token_digest(token.expose()),
    )
    .await
    .unwrap();
    let client = Client::new(server.origin.clone()).unwrap();
    client.set_bearer(Some(token.clone()));
    Registered {
        client,
        token,
        device: Device {
            id: device.id,
            name: device.name,
            platform: Platform::Macos,
            online: false,
            last_seen_at: None,
            created_at: Timestamp::now(),
        },
        cancel: CancellationToken::new(),
    }
}

/// 用一个容量很小的云盘起服务。返回的夹具拥有数据目录：放掉它，数据库文件就没了。
async fn cramped(quota: u64) -> (Fixture, Registered) {
    let dir = tempfile::tempdir().unwrap();
    let pool = assemble_server::db::open(dir.path()).await.unwrap();
    let config = assemble_server::Config::development(dir.path());
    let config = assemble_server::Config {
        drive_quota_bytes: quota,
        ..config
    };
    let server = TestServer::start(config, pool).await.unwrap();
    assemble_server::accounts::create(
        &server.state.pool,
        USERNAME,
        "Alex Wang",
        SecretString::new(PASSWORD.to_owned()),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE accounts SET email = username || '@example.test', email_verified_at = 1 WHERE username = 'alex'").execute(&server.state.pool).await.unwrap();
    let fixture = Fixture {
        dir,
        browser: Browser::new(server.origin.as_str()),
        server,
    };
    let registered = register(&fixture, 'a', "MacBook", Platform::Macos).await;
    (fixture, registered)
}

// ---- 一个文件的一生 ----

#[tokio::test]
async fn a_file_is_stored_listed_and_downloaded_in_whole_or_in_part() {
    let fixture = fixture().await;
    let pc = register(&fixture, 'a', "工作电脑", Platform::Windows).await;
    let bytes = payload(70_000);
    let file = outgoing(&fixture.dir, "周报.docx", &bytes).await;

    let stored = pc
        .drive_upload("周报.docx", &file, TransferOptions::new(&pc.cancel), None)
        .await
        .unwrap();
    assert_eq!(stored.name, "周报.docx");
    assert_eq!(stored.size, bytes.len() as u64);
    assert_eq!(stored.sha256, file.sha256);
    // 媒体类型是按名字猜的，猜出来的是文档类型，不是二进制。
    assert_eq!(
        stored.mime_type.as_deref(),
        Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document")
    );

    let listing = pc.drive(&pc.options()).await.unwrap();
    assert_eq!(listing.items, vec![stored.clone()]);
    assert_eq!(listing.used_bytes, bytes.len() as u64);
    assert_eq!(
        listing.limit_bytes,
        fixture.server.state.config.drive_quota_bytes
    );

    // 取内容：一字不差，并且是当作下载交出去的。
    let download = pc
        .drive_content(&stored.id, TransferOptions::new(&pc.cancel), None)
        .await
        .unwrap();
    assert_eq!(download.content_length, Some(bytes.len() as u64));
    let mut received = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(&mut download.into_reader(), &mut received)
        .await
        .unwrap();
    assert_eq!(received, bytes);

    let raw = reqwest::Client::new();
    let url = format!(
        "{}/v1/drive/files/{}/content",
        fixture.server.origin.as_str(),
        stored.id
    );
    let response = raw
        .get(&url)
        .bearer_auth(pc.token.expose())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers().clone();
    assert_eq!(
        headers
            .get("x-content-type-options")
            .and_then(|v| v.to_str().ok()),
        Some("nosniff")
    );
    let disposition = headers
        .get("content-disposition")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    assert!(disposition.starts_with("attachment; "), "{disposition}");
    assert!(disposition.contains("filename*=UTF-8''"), "{disposition}");
    assert_eq!(
        headers.get("content-type").and_then(|v| v.to_str().ok()),
        stored.mime_type.as_deref()
    );

    // 只取一段。
    let response = raw
        .get(&url)
        .header(reqwest::header::RANGE, "bytes=2-5")
        .bearer_auth(pc.token.expose())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(response.bytes().await.unwrap(), &bytes[2..=5]);
}

#[tokio::test]
async fn a_file_can_be_renamed_and_removed_and_the_newest_is_listed_first() {
    let fixture = fixture().await;
    let pc = register(&fixture, 'a', "工作电脑", Platform::Windows).await;
    let bytes = payload(9_000);
    let file = outgoing(&fixture.dir, "周报.docx", &bytes).await;
    let stored = pc
        .drive_upload("周报.docx", &file, TransferOptions::new(&pc.cancel), None)
        .await
        .unwrap();

    // 改名字：内容不变。
    let renamed = pc
        .drive_rename(
            &stored.id,
            &RenameInput {
                name: "  第九周 周报.docx ".to_owned(),
            },
            &pc.options(),
        )
        .await
        .unwrap();
    assert_eq!(renamed.name, "第九周 周报.docx");
    assert_eq!(renamed.sha256, stored.sha256);
    let listing = pc.drive(&pc.options()).await.unwrap();
    assert_eq!(listing.items, vec![renamed.clone()]);

    // 删掉：列表里没有了，空间还回来了，内容也取不到了。
    permanently_remove(&fixture, &pc, &stored.id).await;
    let listing = pc.drive(&pc.options()).await.unwrap();
    assert_eq!(listing.items, Vec::new());
    assert_eq!(listing.used_bytes, 0);
    let error = pc
        .drive_content(&stored.id, TransferOptions::new(&pc.cancel), None)
        .await
        .unwrap_err();
    assert_eq!(error, http(404, "NOT_FOUND"));
    assert!(
        !fixture.server.state.blobs.path_for(&file.sha256).exists(),
        "the content is gone with its last reference"
    );
    // 再删一次：它已经不在了。
    assert_eq!(
        pc.drive_delete(&stored.id, &pc.options())
            .await
            .unwrap_err(),
        http(404, "NOT_FOUND")
    );
    // 新的在前：后存的排在上面。
    let first = pc
        .drive_upload("a.txt", &file, TransferOptions::new(&pc.cancel), None)
        .await
        .unwrap();
    let second = pc
        .drive_upload("b.txt", &file, TransferOptions::new(&pc.cancel), None)
        .await
        .unwrap();
    let listing = pc.drive(&pc.options()).await.unwrap();
    assert_eq!(
        listing
            .items
            .iter()
            .map(|item| item.id.clone())
            .collect::<Vec<_>>(),
        vec![second.id.clone(), first.id.clone()]
    );
}

// ---- 一份内容只存一遍 ----

#[tokio::test]
async fn identical_content_is_stored_once_and_lives_until_the_last_copy_is_removed() {
    let fixture = fixture().await;
    let pc = register(&fixture, 'a', "工作电脑", Platform::Windows).await;
    let bytes = payload(9_000);
    let file = outgoing(&fixture.dir, "合同.pdf", &bytes).await;

    let original = pc
        .drive_upload("合同.pdf", &file, TransferOptions::new(&pc.cancel), None)
        .await
        .unwrap();
    let copy = pc
        .drive_upload(
            "合同 - 副本.pdf",
            &file,
            TransferOptions::new(&pc.cancel),
            None,
        )
        .await
        .unwrap();
    assert_ne!(original.id, copy.id);
    assert_eq!(refcount(&fixture.server, &file.sha256).await, Some(2));
    // 空间按每个文件算：两个文件就是两份的空间，尽管磁盘上只有一份内容。
    let listing = pc.drive(&pc.options()).await.unwrap();
    assert_eq!(listing.used_bytes, 2 * bytes.len() as u64);

    permanently_remove(&fixture, &pc, &original.id).await;
    assert_eq!(refcount(&fixture.server, &file.sha256).await, Some(1));
    // 另一份照常能下载。
    let received = fetched(&pc, &copy.id).await;
    assert_eq!(received, bytes);

    permanently_remove(&fixture, &pc, &copy.id).await;
    assert_eq!(refcount(&fixture.server, &file.sha256).await, None);
    assert!(!fixture.server.state.blobs.path_for(&file.sha256).exists());
}

#[tokio::test]
async fn a_file_kept_in_the_drive_and_sent_to_a_device_shares_one_copy() {
    let fixture = fixture().await;
    let pc = register(&fixture, 'a', "工作电脑", Platform::Windows).await;
    let mac = register(&fixture, 'b', "MacBook", Platform::Macos).await;
    let bytes = payload(9_000);
    let file = outgoing(&fixture.dir, "合同.pdf", &bytes).await;

    let kept = pc
        .drive_upload("合同.pdf", &file, TransferOptions::new(&pc.cancel), None)
        .await
        .unwrap();
    assert_eq!(refcount(&fixture.server, &file.sha256).await, Some(1));

    // 同一份内容再发给另一台设备：磁盘上还是只有一份。
    let mut mac_events = mac.open_events();
    wait_devices(&mut mac_events, |devices| {
        online_of(devices, "MacBook").is_some_and(|device| device.online)
    })
    .await;
    let sent = pc
        .send_to_device(
            &mac.device.id,
            &file,
            TransferOptions::new(&pc.cancel),
            None,
        )
        .await
        .unwrap();
    assert_eq!(refcount(&fixture.server, &file.sha256).await, Some(2));

    // 发送被撤回：那一份引用放掉，云盘里的文件不受影响。
    pc.send_cancel(&sent.id, &pc.options()).await.unwrap();
    assert_eq!(refcount(&fixture.server, &file.sha256).await, Some(1));
    assert_eq!(fetched(&pc, &kept.id).await, bytes);

    // 反过来：云盘里的删了，还悬着的发送仍然取得到内容。
    let sent = pc
        .send_to_device(
            &mac.device.id,
            &file,
            TransferOptions::new(&pc.cancel),
            None,
        )
        .await
        .unwrap();
    permanently_remove(&fixture, &pc, &kept.id).await;
    assert_eq!(refcount(&fixture.server, &file.sha256).await, Some(1));
    let download = mac
        .send_content(&sent.id, TransferOptions::new(&mac.cancel), None)
        .await
        .unwrap();
    let mut received = Vec::new();
    download
        .into_reader()
        .read_to_end(&mut received)
        .await
        .unwrap();
    assert_eq!(received, bytes);
    mac.send_accept(&sent.id, &mac.options()).await.unwrap();
    assert_eq!(refcount(&fixture.server, &file.sha256).await, None);
}

/// 全站的内容总量有上限：每个账户各有各的配额，磁盘却是所有账户共用的一块。
/// 注册是公开的，只按账户设限，攒起来仍能把服务器写满。
#[tokio::test]
async fn the_storage_budget_is_shared_by_every_account_and_by_sends() {
    let dir = tempfile::tempdir().unwrap();
    let pool = assemble_server::db::open(dir.path()).await.unwrap();
    let config = assemble_server::Config::development(dir.path());
    let config = assemble_server::Config {
        // 每个账户放得下 1000，全站只放得下 1500。
        drive_quota_bytes: 1_000,
        storage_budget_bytes: 1_500,
        ..config
    };
    let server = TestServer::start(config, pool).await.unwrap();
    for (username, display_name) in [("alex", "Alex Wang"), ("blake", "Blake")] {
        assemble_server::accounts::create(
            &server.state.pool,
            username,
            display_name,
            SecretString::new(PASSWORD.to_owned()),
        )
        .await
        .unwrap();
    }
    // 登录页只认邮箱：给 alex 补上，替身设备才登得进去。
    sqlx::query(
        "UPDATE accounts SET email = 'alex@example.test', email_verified_at = 1 \
         WHERE username = 'alex'",
    )
    .execute(&server.state.pool)
    .await
    .unwrap();
    let fixture = Fixture {
        dir,
        browser: Browser::new(server.origin.as_str()),
        server,
    };
    let alex = register(&fixture, 'a', "工作电脑", Platform::Windows).await;
    let mac = register(&fixture, 'b', "MacBook", Platform::Macos).await;
    let blake = device_of_another_account(&fixture.server, "blake").await;

    // alex 把自己的配额放满。
    let big = outgoing(&fixture.dir, "big.bin", &payload(1_000)).await;
    let kept = alex
        .drive_upload("big.bin", &big, TransferOptions::new(&alex.cancel), None)
        .await
        .unwrap();

    // blake 的云盘是空的，可全站只剩 500 了：600 字节放不下，什么都没留下。
    let other = outgoing(&fixture.dir, "other.bin", &payload(600)).await;
    assert_eq!(
        blake
            .drive_upload(
                "other.bin",
                &other,
                TransferOptions::new(&blake.cancel),
                None
            )
            .await
            .unwrap_err(),
        http(413, "QUOTA_EXCEEDED")
    );
    assert_eq!(refcount(&fixture.server, &other.sha256).await, None);
    assert!(!fixture.server.state.blobs.path_for(&other.sha256).exists());

    // 同一份内容已经在库里，不多占地方：重传进自己的云盘照样可以。
    let copy = outgoing(&fixture.dir, "copy.bin", &payload(1_000)).await;
    assert_eq!(copy.sha256, big.sha256);
    let copied = blake
        .drive_upload("copy.bin", &copy, TransferOptions::new(&blake.cancel), None)
        .await
        .unwrap();
    assert_eq!(refcount(&fixture.server, &big.sha256).await, Some(2));

    // 发给设备的内容也算在这道闸里：收之前就拒绝，不留暂存文件。
    let mut mac_events = mac.open_events();
    wait_devices(&mut mac_events, |devices| {
        online_of(devices, "MacBook").is_some_and(|device| device.online)
    })
    .await;
    let more = outgoing(&fixture.dir, "more.bin", &payload(600)).await;
    assert_eq!(
        alex.send_to_device(
            &mac.device.id,
            &more,
            TransferOptions::new(&alex.cancel),
            None
        )
        .await
        .unwrap_err(),
        http(413, "QUOTA_EXCEEDED")
    );
    assert_eq!(refcount(&fixture.server, &more.sha256).await, None);
    assert!(!fixture.server.state.blobs.path_for(&more.sha256).exists());

    // 两个人都删掉之后才腾得出地方：那时 600 字节就放得下了。
    permanently_remove(&fixture, &alex, &kept.id).await;
    permanently_remove(&fixture, &blake, &copied.id).await;
    assert_eq!(refcount(&fixture.server, &big.sha256).await, None);
    blake
        .drive_upload(
            "other.bin",
            &other,
            TransferOptions::new(&blake.cancel),
            None,
        )
        .await
        .unwrap();
}

/// 从云盘里取回一个文件的全部内容。
async fn fetched(pc: &Registered, id: &str) -> Vec<u8> {
    let download = pc
        .drive_content(id, TransferOptions::new(&pc.cancel), None)
        .await
        .unwrap();
    let mut received = Vec::new();
    download
        .into_reader()
        .read_to_end(&mut received)
        .await
        .unwrap();
    received
}

// ---- 空间 ----

#[tokio::test]
async fn the_quota_is_checked_before_the_body_is_read() {
    let (fixture, pc) = cramped(1_500).await;
    let dir = tempfile::tempdir().unwrap();
    let thousand = outgoing(&dir, "a.bin", &payload(1_000)).await;
    let four_hundred = outgoing(&dir, "b.bin", &payload(400)).await;
    let hundred = outgoing(&dir, "c.bin", &payload(101)).await;

    let first = pc
        .drive_upload("a.bin", &thousand, TransferOptions::new(&pc.cancel), None)
        .await
        .unwrap();

    // 再放一个同样大的放不下：正文还没开始读就拒绝了。
    let error = pc
        .drive_upload(
            "a-again.bin",
            &thousand,
            TransferOptions::new(&pc.cancel),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(error, http(413, "QUOTA_EXCEEDED"));

    // 放得下的还能放；超出一个字节也是放不下。
    pc.drive_upload(
        "b.bin",
        &four_hundred,
        TransferOptions::new(&pc.cancel),
        None,
    )
    .await
    .unwrap();
    let error = pc
        .drive_upload("c.bin", &hundred, TransferOptions::new(&pc.cancel), None)
        .await
        .unwrap_err();
    assert_eq!(error, http(413, "QUOTA_EXCEEDED"));

    // 被拒绝的那两次什么都没留下：列表里只有放得下的两个，磁盘上也只有它们的内容。
    let listing = pc.drive(&pc.options()).await.unwrap();
    assert_eq!(listing.used_bytes, 1_400);
    assert_eq!(listing.limit_bytes, 1_500);
    let mut names: Vec<String> = listing.items.iter().map(|item| item.name.clone()).collect();
    names.sort();
    assert_eq!(names, ["a.bin", "b.bin"]);
    assert_eq!(refcount(&fixture.server, &thousand.sha256).await, Some(1));
    assert_eq!(
        refcount(&fixture.server, &four_hundred.sha256).await,
        Some(1)
    );
    assert_eq!(refcount(&fixture.server, &hundred.sha256).await, None);

    // 腾出地方之后就放得下了。
    permanently_remove(&fixture, &pc, &first.id).await;
    pc.drive_upload("c.bin", &hundred, TransferOptions::new(&pc.cancel), None)
        .await
        .unwrap();
    assert_eq!(pc.drive(&pc.options()).await.unwrap().used_bytes, 501);
    assert_eq!(refcount(&fixture.server, &thousand.sha256).await, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_large_upload_that_would_not_fit_is_refused_and_leaves_nothing_behind() {
    // 整个云盘只有 1 KB：一个 3 MB 的文件无论如何都放不下。发送方要听到的是"空间不够"，
    // 而不是"连不上服务"。与发送那边一样，文件得够大、两边得真的同时运行才复现得出来。
    let (fixture, pc) = cramped(1_000).await;
    let dir = tempfile::tempdir().unwrap();
    let big = outgoing(&dir, "big.bin", &payload(3_000_000)).await;

    let error = pc
        .drive_upload("big.bin", &big, TransferOptions::new(&pc.cancel), None)
        .await
        .unwrap_err();
    assert_eq!(error, http(413, "QUOTA_EXCEEDED"));
    let listing = pc.drive(&pc.options()).await.unwrap();
    assert_eq!(listing.items, Vec::new());
    assert_eq!(listing.used_bytes, 0);
    assert!(!fixture.server.state.blobs.path_for(&big.sha256).exists());
}

// ---- 不合法的上传 ----

#[tokio::test]
async fn uploads_are_refused_before_any_content_is_stored() {
    let fixture = fixture().await;
    let pc = register(&fixture, 'a', "工作电脑", Platform::Windows).await;
    let bytes = payload(1_000);
    let file = outgoing(&fixture.dir, "ok.bin", &bytes).await;
    let base = fixture.server.origin.as_str();
    let raw = reqwest::Client::new();
    let token = pc.token.expose();
    let sha = &file.sha256;

    // 名字不能是个路径、不能是空的；摘要得像摘要；媒体类型得像媒体类型。
    for query in [
        format!("name=a%2Fb&size=1&sha256={sha}"),
        "name=b&size=1&sha256=nope".to_owned(),
        format!("name=b&size=1&sha256={sha}&mimeType=bad%20mime"),
        format!("name=&size=1&sha256={sha}"),
    ] {
        let response = raw
            .post(format!("{base}/v1/drive/files?{query}"))
            .bearer_auth(token)
            .body(bytes.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{query}");
        let body = response.json::<serde_json::Value>().await.unwrap();
        assert_eq!(body["error"]["code"], "INVALID_REQUEST", "{query}");
    }

    // 声明的长度超过单文件上限：不必读正文就拒绝。
    let response = raw
        .post(format!(
            "{base}/v1/drive/files?name=x.bin&size={}&sha256={sha}",
            MAX_FILE_SIZE + 1
        ))
        .bearer_auth(token)
        .body(bytes.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap()["error"]["code"],
        "FILE_TOO_LARGE"
    );

    // 摘要声明得不对：内容收完了也拒绝，什么都不落盘。
    let response = raw
        .post(format!(
            "{base}/v1/drive/files?name=x.bin&size={}&sha256={}",
            bytes.len(),
            "0".repeat(64)
        ))
        .bearer_auth(token)
        .body(bytes.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap()["error"]["code"],
        "HASH_MISMATCH"
    );

    // 没带令牌。
    let response = raw
        .post(format!(
            "{base}/v1/drive/files?name=x.bin&size={}&sha256={sha}",
            bytes.len()
        ))
        .body(bytes.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    assert_eq!(pc.drive(&pc.options()).await.unwrap().items, Vec::new());
    assert!(
        !fixture
            .server
            .state
            .blobs
            .path_for(&"0".repeat(64))
            .exists()
    );
}

#[tokio::test]
async fn a_drive_is_private_to_its_account() {
    let fixture = fixture().await;
    let pc = register(&fixture, 'a', "工作电脑", Platform::Windows).await;
    assemble_server::accounts::create(
        &fixture.server.state.pool,
        "blake",
        "Blake",
        SecretString::new(PASSWORD.to_owned()),
    )
    .await
    .unwrap();
    let stranger = device_of_another_account(&fixture.server, "blake").await;

    let file = outgoing(&fixture.dir, "私人文档.pdf", &payload(500)).await;
    let kept = pc
        .drive_upload(
            "私人文档.pdf",
            &file,
            TransferOptions::new(&pc.cancel),
            None,
        )
        .await
        .unwrap();

    // 别的账户：云盘是空的，那个文件等于不存在。
    assert_eq!(
        stranger.drive(&stranger.options()).await.unwrap().items,
        Vec::new()
    );
    assert_eq!(
        stranger
            .drive_content(&kept.id, TransferOptions::new(&stranger.cancel), None)
            .await
            .unwrap_err(),
        http(404, "NOT_FOUND")
    );
    assert_eq!(
        stranger
            .drive_rename(
                &kept.id,
                &RenameInput {
                    name: "改掉.pdf".to_owned()
                },
                &stranger.options(),
            )
            .await
            .unwrap_err(),
        http(404, "NOT_FOUND")
    );
    assert_eq!(
        stranger
            .drive_delete(&kept.id, &stranger.options())
            .await
            .unwrap_err(),
        http(404, "NOT_FOUND")
    );
    // 原来的文件完好无损。
    let listing = pc.drive(&pc.options()).await.unwrap();
    assert_eq!(listing.items, vec![kept.clone()]);
}

#[tokio::test]
async fn names_are_trimmed_and_duplicates_are_told_apart_by_their_identifiers() {
    let fixture = fixture().await;
    let pc = register(&fixture, 'a', "工作电脑", Platform::Windows).await;
    let file = outgoing(&fixture.dir, "说明.txt", b"notes").await;

    let first = pc
        .drive_upload("  说明.txt ", &file, TransferOptions::new(&pc.cancel), None)
        .await
        .unwrap();
    assert_eq!(first.name, "说明.txt");
    // 同名不冲突：靠标识区分。
    let second = pc
        .drive_upload("说明.txt", &file, TransferOptions::new(&pc.cancel), None)
        .await
        .unwrap();
    assert_ne!(first.id, second.id);
    assert_eq!(pc.drive(&pc.options()).await.unwrap().items.len(), 2);

    // 改成一个不像文件名的名字：不改。
    assert_eq!(
        pc.drive_rename(
            &first.id,
            &RenameInput {
                name: "../x".to_owned()
            },
            &pc.options(),
        )
        .await
        .unwrap_err(),
        // 名字不合规在客户端就拦下了，请求没有发出去。
        assemble_client::ClientError::InvalidRequest
    );
    assert_eq!(
        pc.drive(&pc.options()).await.unwrap().items[1].name,
        "说明.txt"
    );
}

#[tokio::test]
async fn a_maintenance_round_leaves_kept_files_alone() {
    let fixture = fixture().await;
    let pc = register(&fixture, 'a', "工作电脑", Platform::Windows).await;
    let bytes = payload(2_048);
    let file = outgoing(&fixture.dir, "a.bin", &bytes).await;
    let kept = pc
        .drive_upload("a.bin", &file, TransferOptions::new(&pc.cancel), None)
        .await
        .unwrap();

    // 云盘里的内容不会自己过期：清理一轮之后它还在，也没被当成孤儿。
    let report = assemble_server::maintenance::run_once(&fixture.server.state)
        .await
        .unwrap();
    assert_eq!(report.blobs.orphan_files, 0);
    assert_eq!(report.blobs.stale_parts, 0);
    assert_eq!(report.blobs.missing_files, 0);
    assert_eq!(fetched(&pc, &kept.id).await, bytes);
}
