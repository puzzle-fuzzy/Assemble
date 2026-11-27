//! 真实 HTTP、迁移与重启验证；只使用隔离目录和测试身份。
#![allow(clippy::too_many_lines)]
mod common;
use assemble_client::{ClientError, OutgoingFile, TransferOptions};
use assemble_protocol::{
    primitives::Platform,
    uploads::{self as wire, Phase},
};
use common::*;
use tokio::io::AsyncReadExt;

fn code(error: ClientError, expected: &str) {
    assert!(matches!(error,ClientError::Http{remote_code:Some(code),..} if code==expected));
}
async fn source(f: &Fixture, pc: &Registered, size: usize) -> (OutgoingFile, Vec<u8>) {
    let bytes = (0..size)
        .map(|i| u8::try_from(i % 251).unwrap())
        .collect::<Vec<_>>();
    let path = f.dir.path().join("中文源文件.bin");
    tokio::fs::write(&path, &bytes).await.unwrap();
    (OutgoingFile::open(&path, &pc.cancel).await.unwrap(), bytes)
}
#[tokio::test]
async fn lost_receipts_restart_missing_parts_and_completion_do_not_duplicate() {
    let mut f = fixture().await;
    let mut pc = register(&f, 'a', "PC", Platform::Windows).await;
    assert!(pc.upload_supported(&pc.options()).await.unwrap());
    let (file, bytes) = source(&f, &pc, 5 * 1024 * 1024 + 7).await;
    let input = pc
        .upload_prepare(&file, "root", &pc.options())
        .await
        .unwrap();
    let upload = pc.upload_create(&input, &pc.options()).await.unwrap();
    assert_eq!(upload.phase, Phase::Receiving);
    let first = bytes[..usize::try_from(wire::PART_BYTES).unwrap()].to_vec();
    let part = pc
        .upload_part(&upload.id, 0, first.clone(), &pc.options())
        .await
        .unwrap();
    assert_eq!(part.parts.len(), 1);
    assert_eq!(
        pc.upload_part(&upload.id, 0, first, &pc.options())
            .await
            .unwrap()
            .parts,
        part.parts
    );
    code(
        pc.upload_complete(&upload.id, &pc.options())
            .await
            .unwrap_err(),
        "UPLOAD_INCOMPLETE",
    );
    assert_eq!(
        pc.upload_create(&input, &pc.options()).await.unwrap().id,
        upload.id
    );
    f.server.stop().await;
    f.server = assemble_server::http::TestServer::start_default(f.dir.path())
        .await
        .unwrap();
    pc.client = assemble_client::Client::new(f.server.origin.clone()).unwrap();
    pc.client.set_bearer(Some(pc.token.clone()));
    let after = pc.upload_status(&upload.id, &pc.options()).await.unwrap();
    assert_eq!(after.parts, part.parts);
    let mut progress = Vec::new();
    let stored = pc
        .upload_continue(
            &input,
            &file,
            &pc.options(),
            Some(&mut |p| progress.push(p.transferred)),
        )
        .await
        .unwrap();
    assert_eq!(progress[0], wire::PART_BYTES);
    assert_eq!(
        pc.upload_continue(&input, &file, &pc.options(), None)
            .await
            .unwrap(),
        stored
    );
    let completed = pc
        .upload_cancel_request(&input.request_id, &pc.options())
        .await
        .unwrap();
    assert_eq!(completed.phase, Phase::Completed);
    assert_eq!(completed.file, Some(stored.clone()));
    assert_eq!(pc.drive(&pc.options()).await.unwrap().items.len(), 1);
    let downloaded = pc
        .drive_content(&stored.file.id, TransferOptions::new(&pc.cancel), None)
        .await
        .unwrap();
    let mut actual = Vec::new();
    downloaded
        .into_reader()
        .read_to_end(&mut actual)
        .await
        .unwrap();
    assert_eq!(actual, bytes);
    let (refs,reservations,parts):(i64,i64,i64)=sqlx::query_as("SELECT (SELECT refcount FROM blobs LIMIT 1),(SELECT count(*) FROM upload_reservations),(SELECT count(*) FROM drive_upload_parts)").fetch_one(&f.server.state.pool).await.unwrap();
    assert_eq!((refs, reservations, parts), (1, 0, 0));
    f.server.stop().await;
}
#[tokio::test]
async fn ownership_declarations_and_parts_are_strict() {
    let mut f = fixture().await;
    let pc = register(&f, 'a', "PC", Platform::Windows).await;
    let other = register(&f, 'b', "Other", Platform::Macos).await;
    let (file, _) = source(&f, &pc, 100).await;
    let input = pc
        .upload_prepare(&file, "root", &pc.options())
        .await
        .unwrap();
    let upload = pc.upload_create(&input, &pc.options()).await.unwrap();
    let anonymous = send(f.browser.get(&format!("/v1/drive/uploads/{}", upload.id))).await;
    assert_eq!(anonymous.status, 401);
    assert_eq!(anonymous.headers["cache-control"], "no-store");
    code(
        other
            .upload_status(&upload.id, &other.options())
            .await
            .unwrap_err(),
        "NOT_FOUND",
    );
    let mut changed = input.clone();
    changed.name = "changed".into();
    code(
        pc.upload_create(&changed, &pc.options()).await.unwrap_err(),
        "UPLOAD_REQUEST_CONFLICT",
    );
    let raw = |index, body: Vec<u8>, digest: &str| {
        f.browser
            .http
            .put(format!(
                "{}/v1/drive/uploads/{}/parts/{index}",
                f.server.origin, upload.id
            ))
            .bearer_auth(pc.token.expose())
            .header("x-content-sha256", digest)
            .body(body)
    };
    assert_eq!(
        raw(48, vec![0; 100], &file.sha256)
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    assert_eq!(
        raw(0, vec![0; 101], &file.sha256)
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    assert_eq!(
        raw(0, vec![0; 100], &"0".repeat(64))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    assert_eq!(
        pc.upload_status(&upload.id, &pc.options())
            .await
            .unwrap()
            .parts,
        []
    );
    let bytes = tokio::fs::read(&file.path).await.unwrap();
    pc.upload_part(&upload.id, 0, bytes, &pc.options())
        .await
        .unwrap();
    code(
        pc.upload_part(&upload.id, 0, vec![7; 100], &pc.options())
            .await
            .unwrap_err(),
        "UPLOAD_PART_CONFLICT",
    );
    pc.upload_cancel_request(&input.request_id, &pc.options())
        .await
        .unwrap();
    assert_eq!(
        pc.upload_create(&input, &pc.options()).await.unwrap().phase,
        Phase::Cancelled
    );
    code(
        pc.upload_continue(&input, &file, &pc.options(), None)
            .await
            .unwrap_err(),
        "UPLOAD_CANCELLED",
    );
    f.server.stop().await;
}

#[tokio::test]
async fn zero_byte_layout_session_caps_and_idle_progress_are_bounded() {
    let mut f = fixture().await;
    let pc = register(&f, 'a', "PC", Platform::Windows).await;
    let (empty, _) = source(&f, &pc, 0).await;
    let input = pc
        .upload_prepare(&empty, "root", &pc.options())
        .await
        .unwrap();
    let stored = pc
        .upload_continue(&input, &empty, &pc.options(), None)
        .await
        .unwrap();
    assert_eq!(stored.file.size, 0);
    let (file, bytes) = source(&f, &pc, 100).await;
    let input = pc
        .upload_prepare(&file, "root", &pc.options())
        .await
        .unwrap();
    let a = pc.upload_create(&input, &pc.options()).await.unwrap();
    let b_input = pc
        .upload_prepare(&file, "root", &pc.options())
        .await
        .unwrap();
    let b = pc.upload_create(&b_input, &pc.options()).await.unwrap();
    let extra = pc
        .upload_prepare(&file, "root", &pc.options())
        .await
        .unwrap();
    code(
        pc.upload_create(&extra, &pc.options()).await.unwrap_err(),
        "UPLOAD_LIMIT",
    );
    pc.upload_part(&a.id, 0, bytes.clone(), &pc.options())
        .await
        .unwrap();
    let expired_at = assemble_protocol::primitives::Timestamp::now().as_millis() + 10000;
    sqlx::query("UPDATE drive_uploads SET expires_at=? WHERE id=?")
        .bind(expired_at)
        .bind(&a.id)
        .execute(&f.server.state.pool)
        .await
        .unwrap();
    assert_eq!(
        pc.upload_part(&a.id, 0, bytes, &pc.options())
            .await
            .unwrap()
            .expires_at
            .as_millis(),
        expired_at
    );
    assert_eq!(
        pc.upload_status(&a.id, &pc.options())
            .await
            .unwrap()
            .expires_at
            .as_millis(),
        expired_at
    );
    pc.upload_cancel_request(&b_input.request_id, &pc.options())
        .await
        .unwrap();
    assert_eq!(
        pc.upload_status(&b.id, &pc.options()).await.unwrap().phase,
        Phase::Cancelled
    );
    assert!(pc.upload_create(&extra, &pc.options()).await.is_ok());
    f.server.stop().await;
}

#[tokio::test]
async fn concurrent_reservations_enforce_global_budget_on_old_and_new_routes() {
    let mut f = fixture().await;
    let mut pc = register(&f, 'a', "PC", Platform::Windows).await;
    let (file, _) = source(&f, &pc, 100).await;
    f.server.stop().await;
    let pool = assemble_server::db::open(f.dir.path()).await.unwrap();
    let config = assemble_server::Config {
        storage_budget_bytes: 150,
        drive_quota_bytes: 150,
        ..assemble_server::Config::development(f.dir.path())
    };
    f.server = assemble_server::http::TestServer::start(config, pool)
        .await
        .unwrap();
    pc.client = assemble_client::Client::new(f.server.origin.clone()).unwrap();
    pc.client.set_bearer(Some(pc.token.clone()));
    let a = pc
        .upload_prepare(&file, "root", &pc.options())
        .await
        .unwrap();
    let b = pc
        .upload_prepare(&file, "root", &pc.options())
        .await
        .unwrap();
    let options = pc.options();
    let (a, b) = tokio::join!(
        pc.upload_create(&a, &options),
        pc.upload_create(&b, &options)
    );
    assert_ne!(a.is_ok(), b.is_ok());
    code(a.err().or_else(|| b.err()).unwrap(), "QUOTA_EXCEEDED");
    code(
        pc.drive_upload("legacy", &file, TransferOptions::new(&pc.cancel), None)
            .await
            .unwrap_err(),
        "QUOTA_EXCEEDED",
    );
    let reserved: i64 = sqlx::query_scalar("SELECT sum(bytes) FROM upload_reservations")
        .fetch_one(&f.server.state.pool)
        .await
        .unwrap();
    assert_eq!(reserved, 100);
    f.server.stop().await;
}
#[tokio::test]
async fn cancel_before_create_and_completion_races_are_terminal() {
    let mut f = fixture().await;
    let pc = register(&f, 'a', "PC", Platform::Windows).await;
    let (file, bytes) = source(&f, &pc, 200).await;
    let input = pc
        .upload_prepare(&file, "root", &pc.options())
        .await
        .unwrap();
    pc.upload_cancel_request(&input.request_id, &pc.options())
        .await
        .unwrap();
    code(
        pc.upload_create(&input, &pc.options()).await.unwrap_err(),
        "UPLOAD_CANCELLED",
    );
    let input = pc
        .upload_prepare(&file, "root", &pc.options())
        .await
        .unwrap();
    let upload = pc.upload_create(&input, &pc.options()).await.unwrap();
    pc.upload_part(&upload.id, 0, bytes, &pc.options())
        .await
        .unwrap();
    let options = pc.options();
    let (complete, cancel) = tokio::join!(
        pc.upload_complete(&upload.id, &options),
        pc.upload_cancel_request(&input.request_id, &options)
    );
    let cancel = cancel.unwrap();
    let count = pc.drive(&options).await.unwrap().items.len();
    if cancel.phase == Phase::Completed {
        assert_eq!(count, 1);
        assert_eq!(complete.unwrap().file, cancel.file);
    } else {
        assert_eq!(count, 0);
        assert_eq!(cancel.phase, Phase::Cancelled);
        assert!(complete.is_err());
    }
    let again = pc
        .upload_cancel_request(&input.request_id, &options)
        .await
        .unwrap();
    assert_eq!(again.phase, cancel.phase);
    f.server.stop().await;
}
#[tokio::test]
async fn ancestor_changes_source_changes_and_expiry_require_explicit_new_upload() {
    let mut f = fixture().await;
    let pc = register(&f, 'a', "PC", Platform::Windows).await;
    let root = pc
        .drive_folder_create(
            &assemble_protocol::drive_tree::Create {
                request_id: "folder".into(),
                parent_id: "root".into(),
                name: "资料".into(),
            },
            &pc.options(),
        )
        .await
        .unwrap();
    let child = pc
        .drive_folder_create(
            &assemble_protocol::drive_tree::Create {
                request_id: "child".into(),
                parent_id: root.id.clone(),
                name: "子目录".into(),
            },
            &pc.options(),
        )
        .await
        .unwrap();
    let (file, bytes) = source(&f, &pc, 123).await;
    let input = pc
        .upload_prepare(&file, &child.id, &pc.options())
        .await
        .unwrap();
    let upload = pc.upload_create(&input, &pc.options()).await.unwrap();
    pc.drive_tree_rename(
        assemble_protocol::drive_tree::Kind::Folder,
        &root.id,
        &assemble_protocol::drive_tree::Rename {
            name: "新资料".into(),
            version: root.version,
        },
        &pc.options(),
    )
    .await
    .unwrap();
    code(
        pc.upload_part(&upload.id, 0, bytes, &pc.options())
            .await
            .unwrap_err(),
        "UPLOAD_TARGET_CHANGED",
    );
    pc.upload_cancel_request(&input.request_id, &pc.options())
        .await
        .unwrap();
    let input = pc
        .upload_prepare(&file, "root", &pc.options())
        .await
        .unwrap();
    let upload = pc.upload_create(&input, &pc.options()).await.unwrap();
    tokio::fs::write(&file.path, b"changed").await.unwrap();
    assert!(matches!(
        pc.upload_continue(&input, &file, &pc.options(), None).await,
        Err(ClientError::LocalSource)
    ));
    sqlx::query("UPDATE drive_uploads SET expires_at=0 WHERE id=?")
        .bind(&upload.id)
        .execute(&f.server.state.pool)
        .await
        .unwrap();
    assert_eq!(
        pc.upload_status(&upload.id, &pc.options())
            .await
            .unwrap()
            .phase,
        Phase::Expired
    );
    code(
        pc.upload_continue(&input, &file, &pc.options(), None)
            .await
            .unwrap_err(),
        "UPLOAD_EXPIRED",
    );
    assemble_server::maintenance::run_once(&f.server.state)
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM upload_reservations")
        .fetch_one(&f.server.state.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    f.server.stop().await;
}
#[tokio::test]
async fn reservations_block_legacy_bypasses_and_cleanup_retries_after_restart() {
    let mut f = fixture().await;
    let mut pc = register(&f, 'a', "PC", Platform::Windows).await;
    let (file, bytes) = source(&f, &pc, 300).await;
    let input = pc
        .upload_prepare(&file, "root", &pc.options())
        .await
        .unwrap();
    let upload = pc.upload_create(&input, &pc.options()).await.unwrap();
    pc.upload_part(&upload.id, 0, bytes, &pc.options())
        .await
        .unwrap();
    let account = pc.me(&pc.options()).await.unwrap().account.id;
    sqlx::query("INSERT INTO upload_reservations(id,account_id,logical_bytes,bytes,expires_at) VALUES ('quota-test',?,?,?,?)").bind(&account).bind(i64::try_from(f.server.state.config.drive_quota_bytes).unwrap()).bind(0_i64).bind(i64::MAX).execute(&f.server.state.pool).await.unwrap();
    code(
        pc.drive_upload("legacy", &file, TransferOptions::new(&pc.cancel), None)
            .await
            .unwrap_err(),
        "QUOTA_EXCEEDED",
    );
    sqlx::query("DELETE FROM upload_reservations WHERE id='quota-test'")
        .execute(&f.server.state.pool)
        .await
        .unwrap();
    let directory = f.dir.path().join("uploads").join(&upload.id);
    tokio::fs::remove_dir_all(&directory).await.unwrap();
    tokio::fs::write(&directory, b"blocked").await.unwrap();
    assert_eq!(
        pc.upload_cancel_request(&input.request_id, &pc.options())
            .await
            .unwrap()
            .phase,
        Phase::Cancelled
    );
    let attempts: i64 = sqlx::query_scalar("SELECT attempts FROM upload_cleanup WHERE upload_id=?")
        .bind(&upload.id)
        .fetch_one(&f.server.state.pool)
        .await
        .unwrap();
    assert_eq!(attempts, 1);
    f.server.stop().await;
    tokio::fs::remove_file(&directory).await.unwrap();
    f.server = assemble_server::http::TestServer::start_default(f.dir.path())
        .await
        .unwrap();
    pc.client = assemble_client::Client::new(f.server.origin.clone()).unwrap();
    pc.client.set_bearer(Some(pc.token.clone()));
    assemble_server::maintenance::run_once(&f.server.state)
        .await
        .unwrap();
    let (jobs, reservations): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM upload_cleanup),(SELECT count(*) FROM upload_reservations)",
    )
    .fetch_one(&f.server.state.pool)
    .await
    .unwrap();
    assert_eq!((jobs, reservations), (0, 0));
    f.server.stop().await;
}
