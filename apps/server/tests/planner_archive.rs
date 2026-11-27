#![allow(clippy::pedantic)]
mod common;
use assemble_protocol::{
    primitives::{Platform, Timestamp, to_hex},
    schedule::{Repeat, ScheduleContent},
};
use assemble_server::planner::archive;
use common::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn bundle() -> Value {
    let now = Timestamp::now().as_millis();
    let start = Timestamp::from_millis(now + 86_400_000).unwrap();
    json!({"version":1,"sourceOrigin":"https://jarvis.yxswy.com","sourceAccountId":"source-owner","records":[
        {"id":"schedule-one","resource":"schedules","version":5,"createdAt":now-2000,"updatedAt":now-1000,"data":{"title":"上海东方艺术中心与东方艺术中心","location":{"label":"上海东方艺术中心","address":"上海市丁香路425号"},"startAt":start,"repeat":"none","status":"upcoming","notes":"第一行\n东方艺术中心  https://example.test/东方艺术中心"}},
        {"id":"reminder-one","resource":"reminders","version":3,"createdAt":now-2000,"updatedAt":now-1000,"data":{"title":"出发到东方艺术中心","location":{"label":"东方艺术中心","address":"上海市丁香路425号"},"dueAt":start,"timezone":"Asia/Shanghai","status":"scheduled","notes":"带好门票"}},
        {"id":"memory-one","resource":"preferences","version":2,"createdAt":now-2000,"updatedAt":now-1000,"data":{"key":"personal.memory:2026-11-20","value":{"description":"原文\n不能裁剪","location":"东方艺术中心","unknownNested":{"count":3}}}}
    ],"navigation":[{"resource":"schedules","id":"schedule-one","sourceVersion":5,"locations":[{"label":"上海东方艺术中心","address":"上海市丁香路425号"},{"label":"东方艺术中心","address":"上海市丁香路425号"}]},{"resource":"preferences","id":"memory-one","sourceVersion":2,"locations":[{"label":"东方艺术中心","address":"上海市丁香路425号"}]}],"baselines":[],"occurrences":[]})
}
async fn write_bundle(dir: &std::path::Path, value: &Value) {
    let bytes = serde_json::to_vec(value).unwrap();
    tokio::fs::write(dir.join("archive.sha256"), to_hex(&Sha256::digest(&bytes)))
        .await
        .unwrap();
    tokio::fs::write(dir.join("archive.json"), bytes)
        .await
        .unwrap();
}

#[tokio::test]
async fn planner_hotfix_can_later_accept_the_pending_media_migration() {
    // 正式服务只补本次日程，不能把别的会话尚未发布的影视升级一起带上。
    // 后续完整升级仍须能接纳版本号较小、但之前尚未执行的迁移。
    let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let subset = tempfile::tempdir().unwrap();
    for entry in std::fs::read_dir(&folder).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name().to_str().unwrap().ends_with(".sql")
            && !entry.file_name().to_str().unwrap().starts_with("0012_")
        {
            std::fs::copy(entry.path(), subset.path().join(entry.file_name())).unwrap();
        }
    }
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::migrate::Migrator::new(subset.path())
        .await
        .unwrap()
        .run(&pool)
        .await
        .unwrap();
    let versions: Vec<i64> =
        sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert!(versions.contains(&13));
    assert!(!versions.contains(&12));
    sqlx::migrate::Migrator::new(folder.as_path())
        .await
        .unwrap()
        .run(&pool)
        .await
        .unwrap();
    let versions: Vec<i64> =
        sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&pool)
            .await
            .unwrap();
    let expected: Vec<i64> = sqlx::migrate!("./migrations")
        .migrations
        .iter()
        .map(|migration| migration.version)
        .collect();
    assert_eq!(versions, expected);
    pool.close().await;
}

#[tokio::test]
async fn import_preserves_existing_rows_original_content_and_private_bindings_and_replays_without_overwriting_edits()
 {
    let mut f = fixture().await;
    let owner = register(&f, 'p', "测试电脑", Platform::Windows).await;
    let existing = owner
        .create_schedule(
            &ScheduleContent {
                title: "已有日程".into(),
                location: None,
                start_at: Timestamp::now(),
                end_at: None,
                repeat: Repeat::None,
                repeat_until: None,
                remind_before: None,
                notes: None,
            },
            None,
            &owner.options(),
        )
        .await
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let input = bundle();
    write_bundle(dir.path(), &input).await;
    let dry = archive::inspect(dir.path()).await.unwrap();
    assert_eq!((dry.schedules, dry.reminders, dry.memos), (1, 1, 1));
    let report = archive::import(&f.server.state.pool, dir.path(), EMAIL)
        .await
        .unwrap();
    assert!(!report.already_imported);
    let listing = owner.planner(&owner.options()).await.unwrap();
    assert_eq!(listing.schedules.len(), 2);
    assert!(listing.schedules.iter().any(|s| s.id == existing.id));
    let migrated = listing
        .schedules
        .iter()
        .find(|s| s.created_by == "Jarvis")
        .unwrap();
    assert_eq!(migrated.version, 5);
    assert_eq!(
        migrated.content.notes.as_deref(),
        input["records"][0]["data"]["notes"].as_str()
    );
    assert_eq!(
        migrated.updated_at.as_millis(),
        input["records"][0]["updatedAt"].as_i64().unwrap()
    );
    let places = owner
        .record_locations("schedule", &migrated.id, &owner.options())
        .await
        .unwrap();
    assert_eq!(places.len(), 2);
    assert_eq!(
        places[0].directions_url().unwrap().host_str(),
        Some("maps.apple.com")
    );
    let memo_id: String = sqlx::query_scalar("SELECT id FROM memos")
        .fetch_one(&f.server.state.pool)
        .await
        .unwrap();
    let memo = owner.memo_get(&memo_id, &owner.options()).await.unwrap();
    assert_eq!(memo.locations.len(), 1);
    assert_eq!(
        serde_json::from_str::<Value>(&memo.content.text).unwrap(),
        input["records"][2]["data"]["value"]
    );
    sqlx::query("UPDATE schedules SET title='用户迁移后修改' WHERE id=?")
        .bind(&migrated.id)
        .execute(&f.server.state.pool)
        .await
        .unwrap();
    assert!(
        owner
            .record_locations("schedule", &migrated.id, &owner.options())
            .await
            .unwrap()
            .iter()
            .all(|l| l.label == "上海东方艺术中心")
    );
    assert!(
        archive::import(&f.server.state.pool, dir.path(), EMAIL)
            .await
            .unwrap()
            .already_imported
    );
    let title: String = sqlx::query_scalar("SELECT title FROM schedules WHERE id=?")
        .bind(&migrated.id)
        .fetch_one(&f.server.state.pool)
        .await
        .unwrap();
    assert_eq!(title, "用户迁移后修改");
    let anonymous = reqwest::get(format!(
        "{}/v1/planner/locations/schedule/{}",
        f.server.origin.as_str().trim_end_matches('/'),
        migrated.id
    ))
    .await
    .unwrap();
    assert_eq!(anonymous.status(), 401);
    let stranger = assemble_server::accounts::create(
        &f.server.state.pool,
        "stranger",
        "其他账户",
        assemble_store::secret::SecretString::new("another-long-enough-password"),
    )
    .await
    .unwrap();
    let token = assemble_server::secrets::random_token().unwrap();
    assemble_server::devices::register(
        &f.server.state.pool,
        &stranger.id,
        &assemble_protocol::device::NewDevice {
            name: "其他电脑".into(),
            platform: Platform::Windows,
        },
        &assemble_server::secrets::token_digest(token.expose()),
    )
    .await
    .unwrap();
    let denied = reqwest::Client::new()
        .get(format!(
            "{}/v1/planner/locations/memo/{memo_id}",
            f.server.origin.as_str().trim_end_matches('/')
        ))
        .bearer_auth(token.expose())
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 404);
    let denied = reqwest::Client::new()
        .get(format!(
            "{}/v1/planner/locations/schedule/{}",
            f.server.origin.as_str().trim_end_matches('/'),
            migrated.id
        ))
        .bearer_auth(token.expose())
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 404);
    let mut changed = input;
    changed["records"][0]["version"] = json!(6);
    write_bundle(dir.path(), &changed).await;
    assert_eq!(
        archive::import(&f.server.state.pool, dir.path(), EMAIL)
            .await
            .unwrap_err()
            .0,
        "PLANNER_IMPORT_SOURCE_CHANGED"
    );
    f.server.stop().await;
}

#[tokio::test]
async fn invalid_package_conflicts_and_wrong_target_never_partially_import() {
    let mut f = fixture().await;
    let dir = tempfile::tempdir().unwrap();
    let mut input = bundle();
    input["records"].as_array_mut().unwrap().push(json!({"id":"bad-notice","resource":"notifications","version":1,"createdAt":1,"updatedAt":2,"data":{"title":"无来源","body":"正文","read":false}}));
    write_bundle(dir.path(), &input).await;
    assert!(
        archive::import(&f.server.state.pool, dir.path(), EMAIL)
            .await
            .is_err()
    );
    let rows:i64=sqlx::query_scalar("SELECT (SELECT count(*) FROM schedules)+(SELECT count(*) FROM reminders)+(SELECT count(*) FROM memos)+(SELECT count(*) FROM record_locations)").fetch_one(&f.server.state.pool).await.unwrap();
    assert_eq!(rows, 0);
    // 后半段遇到与既有备忘录编号冲突时，前面已尝试插入的日程/提醒也回滚。
    let account = account_of(&f).await;
    let material =
        format!("https://jarvis.yxswy.com\0source-owner\0{account}\0preferences\0memory-one");
    let collision = format!("j-{}", &to_hex(&Sha256::digest(material.as_bytes()))[..40]);
    sqlx::query("INSERT INTO memos(id,account_id,title,text,version,created_at,updated_at) VALUES(?,?,'原备忘录','不可覆盖',1,1,1)").bind(&collision).bind(account).execute(&f.server.state.pool).await.unwrap();
    write_bundle(dir.path(), &bundle()).await;
    assert!(
        archive::import(&f.server.state.pool, dir.path(), EMAIL)
            .await
            .is_err()
    );
    let rows: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM schedules)+(SELECT count(*) FROM reminders)",
    )
    .fetch_one(&f.server.state.pool)
    .await
    .unwrap();
    assert_eq!(rows, 0);
    let text: String = sqlx::query_scalar("SELECT text FROM memos WHERE id=?")
        .bind(collision)
        .fetch_one(&f.server.state.pool)
        .await
        .unwrap();
    assert_eq!(text, "不可覆盖");
    assert_eq!(
        archive::import(&f.server.state.pool, dir.path(), "wrong@example.test")
            .await
            .unwrap_err()
            .0,
        "PLANNER_IMPORT_ACCOUNT_UNAVAILABLE"
    );
    tokio::fs::write(dir.path().join("archive.json"), b"tampered")
        .await
        .unwrap();
    assert_eq!(
        archive::inspect(dir.path()).await.unwrap_err().0,
        "PLANNER_IMPORT_DIGEST"
    );
    f.server.stop().await;
}
