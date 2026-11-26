//! 真实 HTTP 与客户端验证账户隔离、原文保存、重试及版本冲突。
mod common;
use assemble_client::RequestOptions;
use assemble_protocol::memo::{MemoContent, MemoCreate, MemoQuery, MemoUpdate};
use assemble_protocol::primitives::Platform;
use common::*;
use reqwest::StatusCode;

#[tokio::test]
async fn text_is_private_persistent_and_retries_do_not_duplicate_or_resurrect() {
    let mut f = fixture().await;
    let device = register(&f, 'a', "电脑", Platform::Windows).await;
    let options = RequestOptions::new(&device.cancel);
    let input = MemoCreate {
        request_id: "same-request".into(),
        content: MemoContent {
            title: "  常用文本  ".into(),
            text: " \t第一行\r\n<script>plain text</script> 🙂\n ".into(),
        },
    };
    let memo = device.memo_create(&input, &options).await.unwrap();
    assert_eq!(memo.content.text, input.content.text);
    assert_eq!(memo.content.title, "常用文本");
    assert_eq!(device.memo_create(&input, &options).await.unwrap(), memo);
    let second = register(&f, 'b', "另一台", Platform::Macos).await;
    assert_eq!(
        second
            .memo_get(&memo.id, &RequestOptions::new(&second.cancel))
            .await
            .unwrap(),
        memo
    );
    let query = MemoQuery {
        q: "<script>".into(),
        ..MemoQuery::default()
    };
    let page = device.memo_list(&query, &options).await.unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].id, memo.id);
    let updated = device
        .memo_update(
            &memo.id,
            &MemoUpdate {
                version: memo.version,
                content: MemoContent {
                    title: "已改标题".into(),
                    text: "新正文".into(),
                },
            },
            &options,
        )
        .await
        .unwrap();
    assert_eq!(updated.version, 2);
    assert!(matches!(
        second
            .memo_remove(&memo.id, 1, &RequestOptions::new(&second.cancel))
            .await,
        Err(assemble_client::ClientError::Http { status: 409, .. })
    ));
    device.memo_remove(&memo.id, 2, &options).await.unwrap();
    assert!(matches!(
        device.memo_create(&input, &options).await,
        Err(assemble_client::ClientError::Http { status: 404, .. })
    ));
    assert_eq!(
        device
            .memo_list(&MemoQuery::default(), &options)
            .await
            .unwrap()
            .total,
        0
    );
    f.server.stop().await;
}

#[tokio::test]
async fn another_account_web_sessions_and_suspended_devices_cannot_access_text() {
    let mut f = fixture().await;
    let owner = register(&f, 'a', "电脑", Platform::Windows).await;
    let options = RequestOptions::new(&owner.cancel);
    let memo = owner
        .memo_create(
            &MemoCreate {
                request_id: "test".into(),
                content: MemoContent {
                    title: "私人".into(),
                    text: "不可外泄".into(),
                },
            },
            &options,
        )
        .await
        .unwrap();
    let other = assemble_server::accounts::create(
        &f.server.state.pool,
        "other",
        "Other",
        assemble_server::SecretString::new(PASSWORD),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE accounts SET email='other@example.test',email_verified_at=1 WHERE id=?")
        .bind(&other.id)
        .execute(&f.server.state.pool)
        .await
        .unwrap();
    let outsider = register(&f, 'c', "别人的电脑", Platform::Linux).await;
    sqlx::query("UPDATE devices SET account_id=? WHERE id=?")
        .bind(&other.id)
        .bind(&outsider.device.id)
        .execute(&f.server.state.pool)
        .await
        .unwrap();
    let theirs = RequestOptions::new(&outsider.cancel);
    assert_eq!(
        outsider
            .memo_list(&MemoQuery::default(), &theirs)
            .await
            .unwrap()
            .total,
        0
    );
    assert!(matches!(
        outsider.memo_get(&memo.id, &theirs).await,
        Err(assemble_client::ClientError::Http { status: 404, .. })
    ));
    assert!(matches!(
        outsider.memo_remove(&memo.id, 1, &theirs).await,
        Err(assemble_client::ClientError::Http { status: 404, .. })
    ));
    assert_eq!(
        f.browser.get("/v1/memos").send().await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    sqlx::query(
        "UPDATE accounts SET suspended_at=1 WHERE id=(SELECT account_id FROM memos WHERE id=?)",
    )
    .bind(&memo.id)
    .execute(&f.server.state.pool)
    .await
    .unwrap();
    assert!(matches!(
        owner.memo_get(&memo.id, &options).await,
        Err(assemble_client::ClientError::Http { status: 401, .. })
    ));
    f.server.stop().await;
}

#[tokio::test]
async fn restarting_the_server_keeps_the_account_memo_and_validates_raw_requests() {
    let mut f = fixture().await;
    let device = register(&f, 'a', "电脑", Platform::Windows).await;
    let input = MemoCreate {
        request_id: "restart-request".into(),
        content: MemoContent {
            title: "持久保存".into(),
            text: "纯文本\r\n🙂".into(),
        },
    };
    let memo = device.memo_create(&input, &device.options()).await.unwrap();
    let changed = MemoCreate {
        content: MemoContent {
            text: "不同正文".into(),
            ..input.content.clone()
        },
        ..input.clone()
    };
    assert!(matches!(
        device.memo_create(&changed, &device.options()).await,
        Err(assemble_client::ClientError::Http { status: 400, .. })
    ));
    for (path, expected) in [
        ("/v1/memos?page=0", StatusCode::BAD_REQUEST),
        ("/v1/memos?pageSize=101", StatusCode::BAD_REQUEST),
        ("/v1/memos?q=%0A", StatusCode::BAD_REQUEST),
    ] {
        assert_eq!(
            reqwest::Client::new()
                .get(format!("{}{path}", f.server.origin.as_str()))
                .bearer_auth(device.token.expose())
                .send()
                .await
                .unwrap()
                .status(),
            expected
        );
    }
    assert_eq!(reqwest::Client::new().post(format!("{}/v1/memos",f.server.origin.as_str())).bearer_auth(device.token.expose()).json(&serde_json::json!({"requestId":"large","content":{"title":"大文本","text":"a".repeat(600_000)}})).send().await.unwrap().status(),StatusCode::PAYLOAD_TOO_LARGE);
    f.server.stop().await;
    f.server = assemble_server::http::TestServer::start_default(f.dir.path())
        .await
        .unwrap();
    let client = assemble_client::Client::new(f.server.origin.clone()).unwrap();
    client.set_bearer(Some(device.token.clone()));
    assert_eq!(
        client.memo_get(&memo.id, &device.options()).await.unwrap(),
        memo
    );
    f.server.stop().await;
}
