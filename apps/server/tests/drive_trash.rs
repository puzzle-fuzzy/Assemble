//! 隔离真实服务验证回收、恢复、内容引用与重启，不使用正式账户。
#![allow(clippy::too_many_lines)]
mod common;
use assemble_client::{ClientError, OutgoingFile, TransferOptions};
use assemble_protocol::{
    drive_trash as trash,
    drive_tree::{self as tree, Kind},
    primitives::Platform,
};
use common::*;
use tokio::io::AsyncReadExt;

async fn folder(pc: &Registered, parent: &str, name: &str, key: &str) -> tree::Folder {
    pc.drive_folder_create(
        &tree::Create {
            request_id: key.into(),
            parent_id: parent.into(),
            name: name.into(),
        },
        &pc.options(),
    )
    .await
    .unwrap()
}
async fn upload(f: &Fixture, pc: &Registered, parent: &str, name: &str) -> tree::TreeFile {
    let path = f.dir.path().join("source.txt");
    tokio::fs::write(&path, b"safe <script>literal</script>")
        .await
        .unwrap();
    let file = OutgoingFile::open(&path, &pc.cancel).await.unwrap();
    pc.drive_tree_upload(parent, name, &file, TransferOptions::new(&pc.cancel), None)
        .await
        .unwrap()
}
fn target(kind: Kind, id: &str, version: i64) -> trash::Target {
    trash::Target {
        kind,
        id: id.into(),
        version,
    }
}
async fn recycle(pc: &Registered, target: trash::Target, key: &str) -> trash::Batch {
    let preview = pc
        .drive_trash_preview(&target, &pc.options())
        .await
        .unwrap();
    pc.drive_trash_move(
        &trash::Move {
            target,
            revision: preview.revision,
            request_id: key.into(),
        },
        &pc.options(),
    )
    .await
    .unwrap()
}
async fn restore(pc: &Registered, id: &str) -> trash::Restored {
    let preview = pc
        .drive_trash_restore_preview(id, &pc.options())
        .await
        .unwrap();
    pc.drive_trash_restore(
        id,
        &trash::Restore {
            version: preview.version,
            revision: preview.revision,
        },
        &pc.options(),
    )
    .await
    .unwrap()
}
fn code(error: ClientError, wanted: &str) {
    match error {
        ClientError::Http {
            status: 404,
            remote_code: None,
        } if wanted == "NOT_FOUND" => {}
        ClientError::Http { remote_code, .. } => assert_eq!(remote_code.as_deref(), Some(wanted)),
        other => panic!("expected {wanted}, got {other:?}"),
    }
}

#[tokio::test]
async fn recycle_is_atomic_survives_restart_and_restores_original_identity_and_bytes() {
    let mut f = fixture().await;
    let mut pc = register(&f, 'a', "PC", Platform::Windows).await;
    let parent = folder(&pc, tree::ROOT, "父目录", "parent").await;
    let child = folder(&pc, &parent.id, "子<目录>", "child").await;
    let file = upload(&f, &pc, &child.id, "中文.txt").await;
    let batch = recycle(&pc, target(Kind::Folder, &parent.id, 1), "delete-parent").await;
    assert_eq!(
        (batch.folder_count, batch.file_count, batch.size),
        (2, 1, file.file.size)
    );
    assert_eq!(pc.drive(&pc.options()).await.unwrap().items.len(), 0);
    assert_eq!(
        pc.drive(&pc.options()).await.unwrap().used_bytes,
        file.file.size
    );
    assert!(matches!(
        pc.drive_content(&file.file.id, TransferOptions::new(&pc.cancel), None)
            .await
            .unwrap_err(),
        ClientError::Http { status: 404, .. }
    ));
    code(
        pc.drive_preview(&file.file.id, &pc.options())
            .await
            .unwrap_err(),
        "NOT_FOUND",
    );
    let detail = pc
        .drive_trash_detail(&batch.id, &tree::Query::default(), &pc.options())
        .await
        .unwrap();
    assert_eq!(detail.items[0].id, child.id);
    let encoded = serde_json::to_string(&detail).unwrap();
    for secret in ["sha256", "password", "authorization", "token", "source.txt"] {
        assert!(!encoded.contains(secret));
    }
    f.server.stop().await;
    f.server = assemble_server::http::TestServer::start_default(f.dir.path())
        .await
        .unwrap();
    pc.client = assemble_client::Client::new(f.server.origin.clone()).unwrap();
    pc.client.set_bearer(Some(pc.token.clone()));
    assert_eq!(
        pc.drive_trash_list("", 1, 20, &pc.options())
            .await
            .unwrap()
            .items[0],
        batch
    );
    let restored = restore(&pc, &batch.id).await;
    assert_eq!(restored.id, parent.id);
    assert_eq!(restored.parent_id, None);
    let mut bytes = Vec::new();
    pc.drive_content(&file.file.id, TransferOptions::new(&pc.cancel), None)
        .await
        .unwrap()
        .into_reader()
        .read_to_end(&mut bytes)
        .await
        .unwrap();
    assert_eq!(bytes, b"safe <script>literal</script>");
    code(
        pc.drive_delete(&file.file.id, &pc.options())
            .await
            .unwrap_err(),
        "DRIVE_TRASH_UPGRADE_REQUIRED",
    );
    assert_eq!(
        pc.drive(&pc.options()).await.unwrap().items[0].id,
        file.file.id
    );
    f.server.stop().await;
}

#[tokio::test]
async fn independent_deleted_children_survive_parent_purge_and_name_conflicts_keep_both() {
    let mut f = fixture().await;
    let pc = register(&f, 'a', "PC", Platform::Windows).await;
    let parent = folder(&pc, tree::ROOT, "父", "a").await;
    let child = folder(&pc, &parent.id, "资料", "b").await;
    let file = upload(&f, &pc, &child.id, "saved.txt").await;
    let child_batch = recycle(&pc, target(Kind::Folder, &child.id, 1), "child-trash").await;
    let parent_batch = recycle(&pc, target(Kind::Folder, &parent.id, 1), "parent-trash").await;
    assert_eq!((parent_batch.folder_count, parent_batch.file_count), (1, 0));
    pc.drive_trash_purge(&parent_batch.id, 1, &pc.options())
        .await
        .unwrap();
    folder(&pc, tree::ROOT, "资料", "conflict").await;
    let preview = pc
        .drive_trash_restore_preview(&child_batch.id, &pc.options())
        .await
        .unwrap();
    assert_eq!(
        preview.restored.reason,
        trash::RestoreReason::ParentUnavailable
    );
    assert_eq!(preview.restored.name, "资料（恢复）");
    folder(&pc, tree::ROOT, "资料（恢复）", "conflict-again").await;
    code(
        pc.drive_trash_restore(
            &child_batch.id,
            &trash::Restore {
                version: 1,
                revision: preview.revision,
            },
            &pc.options(),
        )
        .await
        .unwrap_err(),
        "DRIVE_TRASH_CHANGED",
    );
    let restored = restore(&pc, &child_batch.id).await;
    assert_eq!(restored.name, "资料（恢复 2）");
    assert_eq!(
        pc.drive(&pc.options()).await.unwrap().items[0].sha256,
        file.file.sha256
    );
    f.server.stop().await;
}

#[tokio::test]
async fn subtree_changes_are_reconfirmed_and_request_replay_cannot_delete_restored_content() {
    let mut f = fixture().await;
    let pc = register(&f, 'a', "PC", Platform::Windows).await;
    let folder = folder(&pc, tree::ROOT, "项目", "folder").await;
    let target = target(Kind::Folder, &folder.id, 1);
    let preview = pc
        .drive_trash_preview(&target, &pc.options())
        .await
        .unwrap();
    upload(&f, &pc, &folder.id, "late.txt").await;
    code(
        pc.drive_trash_move(
            &trash::Move {
                target: target.clone(),
                revision: preview.revision,
                request_id: "old-confirmation".into(),
            },
            &pc.options(),
        )
        .await
        .unwrap_err(),
        "DRIVE_TRASH_CHANGED",
    );
    assert_eq!(pc.drive(&pc.options()).await.unwrap().items.len(), 1);
    let preview = pc
        .drive_trash_preview(&target, &pc.options())
        .await
        .unwrap();
    let input = trash::Move {
        target,
        revision: preview.revision,
        request_id: "confirmed".into(),
    };
    let a = pc.drive_trash_move(&input, &pc.options()).await.unwrap();
    assert_eq!(pc.drive_trash_move(&input, &pc.options()).await.unwrap(), a);
    restore(&pc, &a.id).await;
    code(
        pc.drive_trash_move(&input, &pc.options())
            .await
            .unwrap_err(),
        "DRIVE_TRASH_FINISHED",
    );
    assert_eq!(pc.drive(&pc.options()).await.unwrap().items.len(), 1);
    f.server.stop().await;
}

#[tokio::test]
async fn purge_is_durable_idempotent_and_cannot_remove_new_or_shared_references() {
    let mut f = fixture().await;
    let pc = register(&f, 'a', "PC", Platform::Windows).await;
    let first = upload(&f, &pc, tree::ROOT, "one.txt").await;
    let second = upload(&f, &pc, tree::ROOT, "two.txt").await;
    let a = recycle(&pc, target(Kind::File, &first.file.id, 1), "first").await;
    let b = recycle(&pc, target(Kind::File, &second.file.id, 1), "second").await;
    let storage = assemble_server::storage::report(&f.server.state.config, &f.server.state.pool)
        .await
        .unwrap();
    assert_eq!(storage.trash_bytes, first.file.size * 2);
    assert_eq!(storage.reclaimable_bytes, Some(first.file.size));
    pc.drive_trash_purge(&a.id, 1, &pc.options()).await.unwrap();
    pc.drive_trash_purge(&a.id, 1, &pc.options()).await.unwrap();
    f.server.state.blobs.cleanup().await.unwrap();
    let blob = f.server.state.blobs.path_for(&first.file.sha256);
    assert!(blob.exists());
    let refs: i64 = sqlx::query_scalar("SELECT refcount FROM blobs WHERE sha256=?")
        .bind(&first.file.sha256)
        .fetch_one(&f.server.state.pool)
        .await
        .unwrap();
    assert_eq!(refs, 1);
    pc.drive_trash_purge(&b.id, 1, &pc.options()).await.unwrap();
    assert_eq!(pc.drive(&pc.options()).await.unwrap().used_bytes, 0);
    assert!(blob.exists(), "physical deletion waits for durable cleanup");
    let backup = f.dir.path().join("content-backup");
    tokio::fs::rename(&blob, &backup).await.unwrap();
    tokio::fs::create_dir(&blob).await.unwrap();
    f.server.state.blobs.cleanup().await.unwrap();
    let attempts: i64 = sqlx::query_scalar("SELECT attempts FROM blob_cleanup WHERE sha256=?")
        .bind(&first.file.sha256)
        .fetch_one(&f.server.state.pool)
        .await
        .unwrap();
    assert_eq!(attempts, 1);
    let failures: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action='drive.cleanup_failed' AND target=?",
    )
    .bind(&second.file.id)
    .fetch_one(&f.server.state.pool)
    .await
    .unwrap();
    assert_eq!(failures, 1);
    f.server.state.blobs.cleanup().await.unwrap();
    let failures: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action='drive.cleanup_failed' AND target=?",
    )
    .bind(&second.file.id)
    .fetch_one(&f.server.state.pool)
    .await
    .unwrap();
    assert_eq!(failures, 1);
    f.server.stop().await;
    f.server = assemble_server::http::TestServer::start_default(f.dir.path())
        .await
        .unwrap();
    let persisted: i64 = sqlx::query_scalar("SELECT attempts FROM blob_cleanup WHERE sha256=?")
        .bind(&first.file.sha256)
        .fetch_one(&f.server.state.pool)
        .await
        .unwrap();
    assert_eq!(persisted, 2);
    tokio::fs::remove_dir(&blob).await.unwrap();
    tokio::fs::rename(&backup, &blob).await.unwrap();
    let source = f.dir.path().join("source.txt");
    let bytes = tokio::fs::read(&source).await.unwrap();
    let lease = f
        .server
        .state
        .blobs
        .store(
            bytes.as_slice(),
            first.file.size,
            &first.file.sha256,
            &pc.cancel,
        )
        .await
        .unwrap();
    let cleaning = {
        let store = f.server.state.blobs.clone();
        tokio::spawn(async move { store.cleanup().await.unwrap() })
    };
    assemble_server::blobs::retain(&f.server.state.pool, &first.file.sha256, first.file.size)
        .await
        .unwrap();
    drop(lease);
    cleaning.await.unwrap();
    assert!(blob.exists());
    f.server
        .state
        .blobs
        .release(&first.file.sha256)
        .await
        .unwrap();
    assert!(!blob.exists());
    f.server.stop().await;
}

#[tokio::test]
async fn large_subtrees_are_rejected_atomically_and_depth_changes_restore_to_root() {
    let mut f = fixture().await;
    let pc = register(&f, 'a', "PC", Platform::Windows).await;
    let parent = folder(&pc, tree::ROOT, "大目录", "many").await;
    let account = pc.me(&pc.options()).await.unwrap().account.id;
    let mut tx = f.server.state.pool.begin().await.unwrap();
    sqlx::query(
        "INSERT INTO blobs(sha256,size,refcount,created_at) VALUES('fixture-empty',0,5000,1)",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query("WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<5000) INSERT INTO drive_files(id,account_id,parent_id,name,size,sha256,created_at) SELECT 'fixture-'||i,?1,?2,'empty-'||i,0,'fixture-empty',1 FROM n").bind(&account).bind(&parent.id).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    code(
        pc.drive_trash_preview(&target(Kind::Folder, &parent.id, 1), &pc.options())
            .await
            .unwrap_err(),
        "DRIVE_TRASH_TOO_LARGE",
    );
    let active: i64 = sqlx::query_scalar("SELECT count(*) FROM active_drive_files")
        .fetch_one(&f.server.state.pool)
        .await
        .unwrap();
    assert_eq!(active, 5000);
    sqlx::query("DELETE FROM drive_files WHERE id='fixture-5000'")
        .execute(&f.server.state.pool)
        .await
        .unwrap();
    let allowed = pc
        .drive_trash_preview(&target(Kind::Folder, &parent.id, 1), &pc.options())
        .await
        .unwrap();
    assert_eq!((allowed.file_count, allowed.folder_count), (4999, 1));
    let destination = folder(&pc, tree::ROOT, "原父", "destination").await;
    let moving = folder(&pc, &destination.id, "待恢复", "moving").await;
    folder(&pc, &moving.id, "内层", "inner").await;
    let deleted = recycle(&pc, target(Kind::Folder, &moving.id, 1), "depth-restore").await;
    let mut chain = tree::ROOT.to_owned();
    for index in 0..15 {
        chain = folder(
            &pc,
            &chain,
            &format!("层{index}"),
            &format!("depth-{index}"),
        )
        .await
        .id;
    }
    pc.drive_tree_move(
        &tree::Move {
            kind: Kind::Folder,
            id: destination.id,
            version: 1,
            parent_id: chain,
        },
        &pc.options(),
    )
    .await
    .unwrap();
    let preview = pc
        .drive_trash_restore_preview(&deleted.id, &pc.options())
        .await
        .unwrap();
    assert_eq!(preview.restored.reason, trash::RestoreReason::DepthLimit);
    assert_eq!(restore(&pc, &deleted.id).await.parent_id, None);
    f.server.stop().await;
}

#[tokio::test]
async fn administrator_reads_are_same_origin_only_and_cannot_restore_for_an_owner() {
    let mut f = fixture().await;
    let pc = register(&f, 'a', "PC", Platform::Windows).await;
    let file = upload(&f, &pc, tree::ROOT, "protected.txt").await;
    let batch = recycle(&pc, target(Kind::File, &file.file.id, 1), "private-batch").await;
    let owner = pc.me(&pc.options()).await.unwrap().account.id;
    let path = format!("/v1/admin/drive/trash?accountId={owner}");
    let denied = f
        .browser
        .get(&path)
        .header("origin", &f.browser.origin)
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 403);
    assemble_server::admin::grant_super_admin(&f.server.state.pool, EMAIL)
        .await
        .unwrap();
    let admin = f
        .browser
        .get(&path)
        .header("origin", &f.browser.origin)
        .send()
        .await
        .unwrap();
    assert_eq!(admin.status(), 200);
    assert_eq!(admin.headers()["cache-control"], "no-store");
    assert_eq!(
        f.browser
            .get(&path)
            .header("origin", "https://foreign.test")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    for suffix in ["content", "preview", "preview/content"] {
        assert_eq!(
            f.browser
                .get(&format!("/v1/admin/drive/files/{}/{suffix}", file.file.id))
                .header("origin", &f.browser.origin)
                .header("range", "bytes=0-9")
                .header("if-none-match", "*")
                .send()
                .await
                .unwrap()
                .status(),
            404
        );
    }
    assert_eq!(
        f.browser
            .post_form(&format!("/v1/admin/drive/trash/{}/restore", batch.id), &[])
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    f.server.stop().await;
}

#[tokio::test]
async fn foreign_batches_suspended_accounts_and_invalid_paging_are_rejected() {
    let mut f = fixture().await;
    let pc = register(&f, 'a', "PC", Platform::Windows).await;
    let file = upload(&f, &pc, tree::ROOT, "Report<中文>.txt").await;
    let batch = recycle(&pc, target(Kind::File, &file.file.id, 1), "permission").await;
    let owner = pc.me(&pc.options()).await.unwrap().account.id;
    sqlx::query("INSERT INTO accounts(id,username,display_name,password_hash,created_at) VALUES ('foreign-owner','foreign','Foreign','unused-test-hash',1)")
        .execute(&f.server.state.pool).await.unwrap();
    let secret = assemble_server::secrets::random_token().unwrap();
    assemble_server::devices::register(
        &f.server.state.pool,
        "foreign-owner",
        &assemble_protocol::device::NewDevice {
            name: "Foreign PC".into(),
            platform: Platform::Windows,
        },
        &assemble_server::secrets::token_digest(secret.expose()),
    )
    .await
    .unwrap();
    let stranger = assemble_client::Client::new(f.server.origin.clone()).unwrap();
    stranger.set_bearer(Some(secret));
    assert_eq!(
        stranger
            .drive_trash_list("", 1, 20, &pc.options())
            .await
            .unwrap()
            .total,
        0
    );
    code(
        stranger
            .drive_trash_detail(&batch.id, &tree::Query::default(), &pc.options())
            .await
            .unwrap_err(),
        "NOT_FOUND",
    );
    code(
        stranger
            .drive_trash_restore_preview(&batch.id, &pc.options())
            .await
            .unwrap_err(),
        "NOT_FOUND",
    );
    code(
        stranger
            .drive_trash_restore(
                &batch.id,
                &trash::Restore {
                    version: 1,
                    revision: "a".repeat(64),
                },
                &pc.options(),
            )
            .await
            .unwrap_err(),
        "NOT_FOUND",
    );
    code(
        stranger
            .drive_trash_purge(&batch.id, 1, &pc.options())
            .await
            .unwrap_err(),
        "NOT_FOUND",
    );
    let folder = folder(&pc, tree::ROOT, "不可转移所有权", "guard").await;
    assert!(
        sqlx::query("UPDATE drive_folders SET account_id='foreign-owner',trash_id=? WHERE id=?")
            .bind(&batch.id)
            .bind(folder.id)
            .execute(&f.server.state.pool)
            .await
            .is_err()
    );
    for (page, size) in [(0, 20), (1, 0), (1, 101)] {
        assert!(matches!(
            pc.drive_trash_list("", page, size, &pc.options())
                .await
                .unwrap_err(),
            ClientError::InvalidRequest
        ));
        assert_eq!(
            f.browser
                .get(&format!("/v1/drive/trash?page={page}&pageSize={size}"))
                .bearer_auth(pc.token.expose())
                .send()
                .await
                .unwrap()
                .status(),
            400
        );
    }
    let list = pc
        .drive_trash_list("report<中文>", 1, 1, &pc.options())
        .await
        .unwrap();
    assert_eq!(list.items, std::slice::from_ref(&batch));
    assert_eq!(
        pc.drive_trash_list("report<中文>", 2, 1, &pc.options())
            .await
            .unwrap()
            .total,
        1
    );
    assert_eq!(
        pc.drive_trash_list("report<中文>", 2, 1, &pc.options())
            .await
            .unwrap()
            .items
            .len(),
        0
    );
    assert_eq!(
        f.browser
            .get("/v1/drive/trash")
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let preview = pc
        .drive_trash_restore_preview(&batch.id, &pc.options())
        .await
        .unwrap();
    sqlx::query("UPDATE accounts SET suspended_at=1 WHERE id=?")
        .bind(owner)
        .execute(&f.server.state.pool)
        .await
        .unwrap();
    assert!(matches!(
        pc.drive_trash_restore(
            &batch.id,
            &trash::Restore {
                version: 1,
                revision: preview.revision
            },
            &pc.options()
        )
        .await
        .unwrap_err(),
        ClientError::Http { status: 401, .. }
    ));
    let state: String = sqlx::query_scalar("SELECT state FROM drive_trash WHERE id=?")
        .bind(batch.id)
        .fetch_one(&f.server.state.pool)
        .await
        .unwrap();
    assert_eq!(state, "trashed");
    f.server.stop().await;
}
