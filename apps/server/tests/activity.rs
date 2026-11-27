//! 临时数据库、真实 HTTP：记录来自业务成功操作，读取只允许已验证管理员。
#![cfg_attr(test, allow(clippy::pedantic))]
mod common;

use assemble_client::{OutgoingFile, TransferOptions};
use assemble_protocol::{
    activity::{ActivitySnapshot, AdminActivity, AdminActivityTarget},
    admin::{AdminPage, AdminSession},
    primitives::{Platform, Timestamp},
    remote::{RemoteEndReason, RemotePhase, RemoteRequest, RemoteSession},
};
use assemble_server::{activity, admin, audit, auth::Identity};
use common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};

fn get(browser: &Browser, path: &str) -> reqwest::RequestBuilder {
    browser.get(path).header("origin", &browser.origin)
}
async fn value(request: reqwest::RequestBuilder) -> Value {
    let response = request.send().await.unwrap();
    assert!(response.status().is_success(), "{}", response.status());
    response.json().await.unwrap()
}
async fn setup() -> Fixture {
    let f = fixture().await;
    admin::grant_super_admin(&f.server.state.pool, EMAIL)
        .await
        .unwrap();
    log_in(&f.browser).await;
    f
}
async fn listing(f: &Fixture, query: &str) -> AdminPage<AdminActivity> {
    let response = get(&f.browser, &format!("/v1/admin/activity?{query}"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(
        response.headers()["cross-origin-resource-policy"],
        "same-origin"
    );
    response.json().await.unwrap()
}
async fn target(f: &Fixture, kind: &str, id: &str) -> AdminActivityTarget {
    serde_json::from_value(
        value(get(
            &f.browser,
            &format!("/v1/admin/activity/targets/{kind}/{id}"),
        ))
        .await,
    )
    .unwrap()
}

#[tokio::test]
async fn permissions_apply_to_listing_history_and_exact_file_metadata() {
    let mut f = fixture().await;
    let device = register(&f, 'a', "Windows", Platform::Windows).await;
    let paths = [
        "/v1/admin/activity",
        "/v1/admin/activity/targets/file/unknown",
        "/v1/admin/drive/files/unknown",
    ];
    let anonymous = Browser::new(&f.browser.origin);
    for path in paths {
        assert_eq!(get(&anonymous, path).send().await.unwrap().status(), 401);
        assert_eq!(get(&f.browser, path).send().await.unwrap().status(), 403);
        assert_eq!(
            get(&anonymous, path)
                .bearer_auth(device.token.expose())
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
    }
    admin::grant_super_admin(&f.server.state.pool, EMAIL)
        .await
        .unwrap();
    for path in paths {
        assert_eq!(
            f.browser
                .get(path)
                .header("origin", "https://foreign.test")
                .send()
                .await
                .unwrap()
                .status(),
            403
        );
    }
    sqlx::query("UPDATE accounts SET email_verified_at=NULL")
        .execute(&f.server.state.pool)
        .await
        .unwrap();
    assert_eq!(
        get(&f.browser, paths[0]).send().await.unwrap().status(),
        403
    );
    f.server.stop().await;
}

#[tokio::test]
async fn real_upload_rename_delete_and_transfer_keep_actor_and_historical_object_names() {
    let mut f = setup().await;
    let pc = register(&f, 'a', "工作电脑", Platform::Windows).await;
    let mac = register(&f, 'b', "MacBook", Platform::Macos).await;
    let directory = f.dir.path().join("local-secret-directory");
    tokio::fs::create_dir(&directory).await.unwrap();
    let path = directory.join("周报.txt");
    tokio::fs::write(&path, b"private-file-content")
        .await
        .unwrap();
    let file = OutgoingFile::open(&path, &pc.cancel).await.unwrap();
    let kept = pc
        .drive_upload("合同.txt", &file, TransferOptions::new(&pc.cancel), None)
        .await
        .unwrap();
    let exact = value(get(
        &f.browser,
        &format!("/v1/admin/drive/files/{}", kept.id),
    ))
    .await;
    assert_eq!(exact["id"], kept.id);
    assert_eq!(exact["name"], "合同.txt");
    pc.drive_rename(
        &kept.id,
        &assemble_protocol::drive::RenameInput {
            name: "新合同.txt".into(),
        },
        &pc.options(),
    )
    .await
    .unwrap();
    let mut events = mac.open_events();
    wait_devices(&mut events, |devices| {
        online_of(devices, "MacBook").is_some_and(|d| d.online)
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
    mac.send_accept(&sent.id, &mac.options()).await.unwrap();
    pc.drive_delete(&kept.id, &pc.options()).await.unwrap();
    let logs = listing(&f, "category=files").await;
    let upload = logs
        .items
        .iter()
        .find(|row| row.action == "drive.created")
        .unwrap();
    assert_eq!(upload.actor.device_name.as_deref(), Some("工作电脑"));
    assert_eq!(upload.actor.name.as_deref(), Some("Alex Wang"));
    assert_eq!(
        upload.subject.as_ref().unwrap().name.as_deref(),
        Some("合同.txt")
    );
    assert!(!upload.subject.as_ref().unwrap().available);
    assert!(matches!(&upload.snapshot,Some(ActivitySnapshot::File{name,..}) if name=="合同.txt"));
    let deleted = target(&f, "file", &kept.id).await;
    assert!(!deleted.subject.available);
    assert_eq!(deleted.events.len(), 3);
    assert!(matches!(deleted.snapshot,Some(ActivitySnapshot::File{name,..}) if name=="新合同.txt"));
    assert_eq!(
        get(&f.browser, &format!("/v1/admin/drive/files/{}", kept.id))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    let transfer = target(&f, "transfer", &sent.id).await;
    assert_eq!(transfer.events.len(), 2);
    assert_eq!(transfer.events[0].action, "send.accepted");
    assert_eq!(
        transfer.events[0].actor.device_id.as_deref(),
        Some(mac.device.id.as_str())
    );
    assert!(
        matches!(transfer.snapshot,Some(ActivitySnapshot::Transfer{state,from_device_name,to_device_name,..}) if state=="delivered"&&from_device_name=="工作电脑"&&to_device_name=="MacBook")
    );
    let serialized = serde_json::to_string(&listing(&f, "").await).unwrap();
    for secret in [
        "private-file-content",
        "local-secret-directory",
        pc.token.expose(),
        mac.token.expose(),
    ] {
        assert!(!serialized.contains(secret));
    }
    f.server.stop().await;
}

fn content(title: &str) -> Value {
    json!({"published":false,"mediaType":"movie","title":title,"originalTitle":"","year":2026,"releaseDate":null,"overview":"private-overview","genres":[],"episodeCount":null,"status":"planned","watchedAt":null,"review":"private-review","notes":"private-notes","magnetUri":null})
}
#[tokio::test]
async fn media_and_collection_edits_are_audited_including_deletion_without_private_content() {
    let mut f = setup().await;
    let session: AdminSession =
        serde_json::from_value(value(get(&f.browser, "/v1/admin/session")).await).unwrap();
    let write = |method, path: &str| {
        f.browser
            .http
            .request(method, format!("{}{path}", f.browser.origin))
            .header("origin", &f.browser.origin)
            .header("x-csrf-token", &session.csrf_token)
    };
    let created = value(
        write(reqwest::Method::POST, "/v1/admin/media/entries")
            .json(&json!({"content":content("原片名"),"requestId":"activity-film"})),
    )
    .await;
    let id = created["item"]["id"].as_str().unwrap();
    let entry = format!("/v1/admin/media/entries/{id}");
    value(
        write(reqwest::Method::PUT, &entry).json(&json!({"version":1,"content":content("新片名")})),
    )
    .await;
    assert_eq!(
        write(reqwest::Method::PUT, &entry)
            .json(&json!({"version":1,"content":content("不应写入")}))
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    let collection=value(write(reqwest::Method::POST,"/v1/admin/media/collections").json(&json!({"title":"专题","description":"private-description","entryIds":[id],"requestId":"activity-collection"}))).await;
    let cid = collection["item"]["id"].as_str().unwrap();
    let cpath = format!("/v1/admin/media/collections/{cid}");
    let updated = value(write(reqwest::Method::PUT, &cpath).json(
        &json!({"version":1,"title":"新专题","description":"private-description","entryIds":[id]}),
    ))
    .await;
    assert_eq!(updated["item"]["version"], 2);
    assert!(
        write(reqwest::Method::DELETE, &cpath)
            .json(&json!({"version":2}))
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    assert!(
        write(reqwest::Method::DELETE, &entry)
            .json(&json!({"version":2}))
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    let page = listing(&f, "category=media").await;
    assert_eq!(page.total, 6);
    assert!(
        page.items
            .iter()
            .all(|row| row.actor.device_id.is_none() && row.actor.account_id.is_some())
    );
    let movie = target(&f, "media", id).await;
    assert_eq!(movie.events.len(), 3);
    assert_eq!(
        movie.events[2].subject.as_ref().unwrap().name.as_deref(),
        Some("原片名")
    );
    let collection = target(&f, "collection", cid).await;
    assert_eq!(collection.events.len(), 3);
    assert!(!collection.subject.available);
    let serialized = serde_json::to_string(&page).unwrap();
    for secret in [
        "private-overview",
        "private-review",
        "private-notes",
        "private-description",
        session.csrf_token.as_str(),
    ] {
        assert!(!serialized.contains(secret));
    }
    f.server.stop().await;
}

#[tokio::test]
async fn remote_consent_and_repeated_approval_and_end_create_one_record_per_transition() {
    let mut f = setup().await;
    let controller = register(&f, 'a', "Windows", Platform::Windows).await;
    let host = register(&f, 'b', "Mac", Platform::Macos).await;
    let mut stream = host.open_events();
    next_event(&mut stream).await;
    let session = controller
        .remote_request(
            &RemoteRequest {
                host_id: host.device.id.clone(),
            },
            &controller.options(),
        )
        .await
        .unwrap();
    assert_eq!(
        f.browser
            .http
            .post(format!(
                "{}/v1/remote/sessions/{}/accept",
                f.browser.origin, session.id
            ))
            .bearer_auth(host.token.expose())
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    prepare_remote(&controller, &host, &session).await;
    for _ in 0..2 {
        host.remote_accept(&session.id, &host.options())
            .await
            .unwrap();
    }
    for _ in 0..2 {
        controller
            .remote_end(&session.id, &controller.options())
            .await
            .unwrap();
    }
    f.server.state.remote.prune(&f.server.state.pool).await;
    let detail = target(&f, "remote", &session.id).await;
    assert_eq!(detail.events.len(), 3);
    assert_eq!(detail.events[0].action, "remote.ended");
    assert_eq!(detail.events[1].actor.device_name.as_deref(), Some("Mac"));
    assert_eq!(
        detail.events[2].actor.device_name.as_deref(),
        Some("Windows")
    );
    assert!(
        matches!(&detail.events[2].snapshot,Some(ActivitySnapshot::Remote{phase,..}) if phase=="pending")
    );
    assert!(
        matches!(&detail.events[1].snapshot,Some(ActivitySnapshot::Remote{phase,..}) if phase=="active")
    );
    let raw = serde_json::to_string(&detail).unwrap();
    for field in ["verification", "handshake", "nonce", "chunks", "clipboard"] {
        assert!(!raw.contains(field));
    }
    f.server.stop().await;
}

#[tokio::test]
async fn administrators_see_other_accounts_private_edits_without_leaking_sessions_or_notes() {
    let mut f = setup().await;
    let owner = assemble_server::accounts::create(
        &f.server.state.pool,
        "bob",
        "另一位用户",
        assemble_server::SecretString::new(PASSWORD.to_owned()),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE accounts SET email='bob@example.test',email_verified_at=1 WHERE id=?")
        .bind(&owner.id)
        .execute(&f.server.state.pool)
        .await
        .unwrap();
    let browser = Browser::new(&f.browser.origin);
    assert!(
        try_login(&browser, "bob", PASSWORD, "")
            .await
            .status
            .is_success()
    );
    let csrf: String = sqlx::query_scalar("SELECT csrf_token FROM web_sessions WHERE account_id=?")
        .bind(&owner.id)
        .fetch_one(&f.server.state.pool)
        .await
        .unwrap();
    let body = json!({"content":content("本人私人影片"),"requestId":"private-activity"});
    let created = value(
        browser
            .http
            .post(format!("{}/v1/media/private/entries", browser.origin))
            .header("origin", &browser.origin)
            .header("x-csrf-token", &csrf)
            .json(&body),
    )
    .await;
    let id = created["item"]["id"].as_str().unwrap();
    let records = listing(&f, "category=media").await;
    assert_eq!(records.total, 1);
    assert_eq!(records.items[0].actor.name.as_deref(), Some("另一位用户"));
    assert_eq!(
        records.items[0]
            .subject
            .as_ref()
            .unwrap()
            .owner_id
            .as_deref(),
        Some(owner.id.as_str())
    );
    assert_eq!(records.items[0].subject.as_ref().unwrap().id, id);
    assert!(records.items[0].actor.device_id.is_none());
    assert!(!serde_json::to_string(&records).unwrap().contains(&csrf));
    assert!(
        !serde_json::to_string(&records)
            .unwrap()
            .contains("private-notes")
    );
    assert_eq!(
        get(&browser, "/v1/admin/activity")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    f.server.stop().await;
}

#[tokio::test]
async fn delayed_remote_writes_do_not_reopen_ended_sessions_and_restart_ends_unfinished_ones_once()
{
    let mut f = setup().await;
    let pc = register(&f, 'a', "Windows", Platform::Windows).await;
    let owner = sqlx::query_scalar::<_, String>("SELECT id FROM accounts WHERE username='alex'")
        .fetch_one(&f.server.state.pool)
        .await
        .unwrap();
    let actor = Identity {
        account_id: owner.clone(),
        device_id: pc.device.id.clone(),
        device_name: "Windows".into(),
    };
    let at = |offset: i64| Timestamp::from_millis(1_800_000_000_000 + offset).unwrap();
    let mut session = RemoteSession {
        id: "delayed".into(),
        controller_id: pc.device.id.clone(),
        controller_name: "Windows".into(),
        host_id: "host".into(),
        host_name: "Mac".into(),
        phase: RemotePhase::Ended,
        created_at: at(0),
        end_reason: Some(RemoteEndReason::Stopped),
        verification: Some("NEVER-LOG-SAS".into()),
    };
    assert!(
        activity::record_remote(
            &f.server.state.pool,
            &owner,
            Some(&actor),
            "remote.ended",
            &session,
            at(20)
        )
        .await
    );
    session.phase = RemotePhase::Active;
    session.end_reason = None;
    assert!(
        activity::record_remote(
            &f.server.state.pool,
            &owner,
            Some(&actor),
            "remote.accepted",
            &session,
            at(10)
        )
        .await
    );
    session.phase = RemotePhase::Pending;
    assert!(
        activity::record_remote(
            &f.server.state.pool,
            &owner,
            Some(&actor),
            "remote.requested",
            &session,
            at(0)
        )
        .await
    );
    let detail = target(&f, "remote", "delayed").await;
    assert!(matches!(detail.snapshot,Some(ActivitySnapshot::Remote{phase,..}) if phase=="ended"));
    assert_eq!(detail.updated_at, at(20));
    assert_eq!(
        detail
            .events
            .iter()
            .map(|event| event.occurred_at)
            .collect::<Vec<_>>(),
        vec![at(20), at(10), at(0)]
    );
    session.id = "restart".into();
    activity::record_remote(
        &f.server.state.pool,
        &owner,
        Some(&actor),
        "remote.requested",
        &session,
        at(0),
    )
    .await;
    activity::recover_remote(&f.server.state.pool)
        .await
        .unwrap();
    activity::recover_remote(&f.server.state.pool)
        .await
        .unwrap();
    let recovered = target(&f, "remote", "restart").await;
    assert_eq!(recovered.events.len(), 2);
    assert!(
        recovered
            .events
            .iter()
            .find(|event| event.action == "remote.ended")
            .unwrap()
            .actor
            .account_id
            .is_none()
    );
    assert!(
        matches!(&recovered.snapshot,Some(ActivitySnapshot::Remote{phase,end_reason,..}) if phase=="ended"&&end_reason.as_deref()==Some("expired"))
    );
    assert!(
        !serde_json::to_string(&recovered)
            .unwrap()
            .contains("NEVER-LOG-SAS")
    );
    f.server.stop().await;
}

#[tokio::test]
async fn filters_use_literal_search_stable_pagination_and_beijing_calendar_day_boundaries() {
    let mut f = setup().await;
    let pool = &f.server.state.pool;
    for (id, at) in [
        ("before", 1_801_065_599_999_i64),
        ("first", 1_801_065_600_000),
        ("second", 1_801_065_600_000),
        ("after", 1_801_152_000_000),
    ] {
        sqlx::query("INSERT INTO audit_log(action,target,created_at) VALUES('drive.created',?,?)")
            .bind(id)
            .bind(at)
            .execute(pool)
            .await
            .unwrap();
    }
    // 时间由给定 UTC 时间换算为北京时间，再按同一天筛选，避免测试依赖运行机时区。
    let day = chrono::DateTime::from_timestamp_millis(1_801_065_600_000)
        .unwrap()
        .with_timezone(&chrono::FixedOffset::east_opt(8 * 3600).unwrap())
        .date_naive()
        .to_string();
    let date = format!("from={day}&through={day}&pageSize=1");
    let one = listing(&f, &date).await;
    assert_eq!(one.total, 2);
    assert_eq!(one.items[0].subject.as_ref().unwrap().id, "second");
    let two = listing(&f, &format!("{date}&page=2")).await;
    assert_eq!(two.items[0].subject.as_ref().unwrap().id, "first");
    assert_eq!(listing(&f, "q=%25").await.total, 0);
    assert_eq!(listing(&f, "q=FiRsT").await.total, 1);
    assert_eq!(listing(&f, "category=remote").await.total, 0);
    let missing = target(&f, "file", "first").await;
    assert!(missing.snapshot.is_none());
    assert!(!missing.subject.available);
    assert_eq!(missing.events.len(), 1);
    for query in [
        "category=secrets",
        "page=0",
        "pageSize=101",
        "from=2026-02-30",
        "from=2026-11-29&through=2026-11-27",
        "q=%00",
    ] {
        assert_eq!(
            get(&f.browser, &format!("/v1/admin/activity?{query}"))
                .send()
                .await
                .unwrap()
                .status(),
            400,
            "{query}"
        );
    }
    f.server.stop().await;
}

#[tokio::test]
async fn upgrading_old_audit_rows_does_not_invent_historical_snapshots_or_delete_records() {
    use sqlx::{migrate::Migrator, sqlite::SqlitePoolOptions};
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    let before = Migrator {
        migrations: std::borrow::Cow::Owned(
            sqlx::migrate!("./migrations")
                .migrations
                .iter()
                .filter(|m| m.version < 15)
                .cloned()
                .collect(),
        ),
        ..Migrator::DEFAULT
    };
    before.run(&pool).await.unwrap();
    audit::record(
        &pool,
        Some("old-account"),
        Some("old-device"),
        "media.updated",
        Some("private:old-account:entry-1"),
    )
    .await;
    let before_count: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log")
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    let row: (String, String, Option<String>, Option<String>) =
        sqlx::query_as("SELECT category,subject_id,snapshot,actor_name FROM audit_log")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row, ("media".into(), "entry-1".into(), None, None));
    assert_eq!(
        before_count,
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM audit_log")
            .fetch_one(&pool)
            .await
            .unwrap()
    );
}
