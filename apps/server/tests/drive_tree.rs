//! 隔离真实服务覆盖目录事务、旧端兼容和内容引用，不使用正式账号。
#![allow(clippy::too_many_lines)]
mod common;
use assemble_client::{ClientError, OutgoingFile, TransferOptions};
use assemble_protocol::drive_tree::{
    Create, Entry, Folder, Kind, MAX_DEPTH, Move, Query, ROOT, Rename,
};
use assemble_protocol::primitives::Platform;
use common::*;
use tokio::io::AsyncReadExt;

async fn folder(pc: &Registered, parent: &str, name: &str, key: &str) -> Folder {
    pc.drive_folder_create(
        &Create {
            request_id: key.into(),
            parent_id: parent.into(),
            name: name.into(),
        },
        &pc.options(),
    )
    .await
    .unwrap()
}
fn code(error: ClientError, expected: &str) {
    assert!(matches!(error,ClientError::Http{remote_code:Some(code),..} if code==expected));
}
#[tokio::test]
async fn folders_moves_legacy_and_content_references() {
    let mut f = fixture().await;
    let mut pc = register(&f, 'a', "PC", Platform::Windows).await;
    let a = folder(&pc, ROOT, "资料", "a").await;
    assert_eq!(folder(&pc, ROOT, "资料", "a").await, a);
    code(
        pc.drive_folder_create(
            &Create {
                request_id: "a".into(),
                parent_id: ROOT.into(),
                name: "changed".into(),
            },
            &pc.options(),
        )
        .await
        .unwrap_err(),
        "DRIVE_REQUEST_CONFLICT",
    );
    code(
        pc.drive_folder_create(
            &Create {
                request_id: "b".into(),
                parent_id: ROOT.into(),
                name: a.name.clone(),
            },
            &pc.options(),
        )
        .await
        .unwrap_err(),
        "DRIVE_NAME_CONFLICT",
    );
    let b = folder(&pc, &a.id, "子目录", "b").await;
    code(
        pc.drive_tree_move(
            &Move {
                kind: Kind::Folder,
                id: a.id.clone(),
                version: 1,
                parent_id: b.id.clone(),
            },
            &pc.options(),
        )
        .await
        .unwrap_err(),
        "DRIVE_CYCLE",
    );
    code(
        pc.drive_tree_delete(Kind::Folder, &a.id, 1, &pc.options())
            .await
            .unwrap_err(),
        "DRIVE_FOLDER_NOT_EMPTY",
    );
    let path = f.dir.path().join("test.txt");
    tokio::fs::write(&path, b"same content").await.unwrap();
    let source = OutgoingFile::open(&path, &pc.cancel).await.unwrap();
    let stored = pc
        .drive_tree_upload(
            &b.id,
            "中文%_.txt",
            &source,
            TransferOptions::new(&pc.cancel),
            None,
        )
        .await
        .unwrap();
    let legacy = pc.drive(&pc.options()).await.unwrap();
    assert_eq!(legacy.items, vec![stored.file.clone()]);
    assert!(stored.updated_at.is_none());
    let query = Query {
        parent_id: b.id.clone(),
        q: "%_".into(),
        ..Query::default()
    };
    let list = pc.drive_tree(&query, &pc.options()).await.unwrap();
    assert_eq!(list.ancestors, vec![a.clone()]);
    assert_eq!(list.current, Some(b.clone()));
    assert_eq!(list.total, 1);
    pc.drive_rename(
        &stored.file.id,
        &assemble_protocol::drive::RenameInput {
            name: "renamed.txt".into(),
        },
        &pc.options(),
    )
    .await
    .unwrap();
    code(
        pc.drive_tree_move(
            &Move {
                kind: Kind::File,
                id: stored.file.id.clone(),
                version: 1,
                parent_id: ROOT.into(),
            },
            &pc.options(),
        )
        .await
        .unwrap_err(),
        "VERSION_CONFLICT",
    );
    let renamed = pc
        .drive_tree_rename(
            Kind::File,
            &stored.file.id,
            &Rename {
                name: "目录内新名称.txt".into(),
                version: 2,
            },
            &pc.options(),
        )
        .await
        .unwrap();
    assert!(matches!(renamed, Entry::File(ref file) if file.version == 3));
    let moved = pc
        .drive_tree_move(
            &Move {
                kind: Kind::File,
                id: stored.file.id.clone(),
                version: 3,
                parent_id: ROOT.into(),
            },
            &pc.options(),
        )
        .await
        .unwrap();
    let Entry::File(moved) = moved else {
        panic!("file")
    };
    assert_eq!(moved.version, 4);
    assert_eq!(moved.file.sha256, source.sha256);
    let audit: Vec<String> = sqlx::query_scalar(
        "SELECT action FROM audit_log WHERE target=? AND action IN ('drive.renamed','drive.moved') ORDER BY id",
    )
    .bind(&moved.file.id)
    .fetch_all(&f.server.state.pool)
    .await
    .unwrap();
    assert_eq!(audit, ["drive.renamed", "drive.renamed", "drive.moved"]);
    let refs: i64 = sqlx::query_scalar("SELECT refcount FROM blobs WHERE sha256=?")
        .bind(&source.sha256)
        .fetch_one(&f.server.state.pool)
        .await
        .unwrap();
    assert_eq!(refs, 1);
    let content = pc
        .drive_content(&moved.file.id, TransferOptions::new(&pc.cancel), None)
        .await
        .unwrap();
    let mut reader = content.into_reader();
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).await.unwrap();
    assert_eq!(bytes, b"same content");
    f.server.stop().await;
    f.server = assemble_server::http::TestServer::start_default(f.dir.path())
        .await
        .unwrap();
    pc.client = assemble_client::Client::new(f.server.origin.clone()).unwrap();
    pc.client.set_bearer(Some(pc.token.clone()));
    let restored = pc
        .drive_tree(&Query::default(), &pc.options())
        .await
        .unwrap();
    assert!(restored.items.contains(&Entry::Folder(a.clone())));
    assert!(restored.items.contains(&Entry::File(moved.clone())));
    assert_eq!(restored.used_bytes, source.size);
    pc.drive_tree_delete(Kind::Folder, &b.id, 1, &pc.options())
        .await
        .unwrap();
    pc.drive_tree_delete(Kind::Folder, &a.id, 1, &pc.options())
        .await
        .unwrap();
    code(
        pc.drive_folder_create(
            &Create {
                request_id: "a".into(),
                parent_id: ROOT.into(),
                name: a.name,
            },
            &pc.options(),
        )
        .await
        .unwrap_err(),
        "NOT_FOUND",
    );
    pc.drive_tree_delete(Kind::File, &moved.file.id, 4, &pc.options())
        .await
        .unwrap();
    assert_eq!(pc.drive(&pc.options()).await.unwrap().used_bytes, 0);
    assert!(!f.server.state.blobs.path_for(&source.sha256).exists());
    f.server.stop().await;
}

#[tokio::test]
async fn subtree_limits_stable_pagination_and_ownership() {
    let mut f = fixture().await;
    let pc = register(&f, 'a', "PC", Platform::Windows).await;
    let mut parent = ROOT.to_owned();
    let mut chain = Vec::new();
    for index in 0..MAX_DEPTH {
        let next = folder(&pc, &parent, &format!("目录{index}"), &format!("k{index}")).await;
        parent = next.id.clone();
        chain.push(next);
    }
    code(
        pc.drive_folder_create(
            &Create {
                request_id: "too-deep".into(),
                parent_id: parent,
                name: "x".into(),
            },
            &pc.options(),
        )
        .await
        .unwrap_err(),
        "DRIVE_DEPTH_LIMIT",
    );
    let branch = folder(&pc, ROOT, "另一个分支", "branch").await;
    code(
        pc.drive_tree_move(
            &Move {
                kind: Kind::Folder,
                id: chain[0].id.clone(),
                version: 1,
                parent_id: branch.id.clone(),
            },
            &pc.options(),
        )
        .await
        .unwrap_err(),
        "DRIVE_DEPTH_LIMIT",
    );
    let a = pc
        .drive_tree(
            &Query {
                page_size: 1,
                ..Query::default()
            },
            &pc.options(),
        )
        .await
        .unwrap();
    let b = pc
        .drive_tree(
            &Query {
                page: 2,
                page_size: 1,
                ..Query::default()
            },
            &pc.options(),
        )
        .await
        .unwrap();
    assert_eq!(a.total, 2);
    assert_ne!(a.items, b.items);
    code(
        pc.drive_tree_rename(
            Kind::Folder,
            &branch.id,
            &Rename {
                name: "renamed".into(),
                version: 2,
            },
            &pc.options(),
        )
        .await
        .unwrap_err(),
        "VERSION_CONFLICT",
    );
    let root = pc
        .drive_tree(&Query::default(), &pc.options())
        .await
        .unwrap();
    let serialized = serde_json::to_string(&root).unwrap();
    for sensitive in ["token", "password", "session", "path", "authorization"] {
        assert!(!serialized.contains(sensitive));
    }
    let account = assemble_server::accounts::create(
        &f.server.state.pool,
        "other",
        "Other",
        assemble_store::secret::SecretString::new(PASSWORD),
    )
    .await
    .unwrap();
    let foreign = "foreign-folder";
    sqlx::query("INSERT INTO drive_folders(id,account_id,name,version,created_at,updated_at) VALUES(?,?,?,1,1,1)").bind(foreign).bind(&account.id).bind("私有").execute(&f.server.state.pool).await.unwrap();
    code(
        pc.drive_tree(
            &Query {
                parent_id: foreign.into(),
                ..Query::default()
            },
            &pc.options(),
        )
        .await
        .unwrap_err(),
        "NOT_FOUND",
    );
    code(
        pc.drive_tree_move(
            &Move {
                kind: Kind::Folder,
                id: branch.id.clone(),
                version: 1,
                parent_id: foreign.into(),
            },
            &pc.options(),
        )
        .await
        .unwrap_err(),
        "NOT_FOUND",
    );
    assert!(
        sqlx::query("UPDATE drive_folders SET parent_id=? WHERE id=?")
            .bind(foreign)
            .bind(&branch.id)
            .execute(&f.server.state.pool)
            .await
            .is_err()
    );
    let unauth = reqwest::get(format!("{}/v1/drive/tree", f.server.origin.as_str()))
        .await
        .unwrap();
    assert_eq!(unauth.status(), 401);
    let denied = f
        .browser
        .get(&format!("/v1/admin/drive/tree?accountId={}", pc.device.id))
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
        .get(&format!("/v1/admin/drive/tree?accountId={}", account.id))
        .header("origin", &f.browser.origin)
        .send()
        .await
        .unwrap();
    assert_eq!(admin.status(), 200);
    let crosssite = f
        .browser
        .get(&format!("/v1/admin/drive/tree?accountId={}", account.id))
        .header("origin", "https://other.test")
        .send()
        .await
        .unwrap();
    assert_eq!(crosssite.status(), 403);
    f.server.stop().await;
}

#[tokio::test]
async fn deleting_a_target_during_upload_leaves_no_file_or_blob_reference() {
    use std::time::Duration;
    let mut f = fixture().await;
    let pc = register(&f, 'a', "PC", Platform::Windows).await;
    let target = folder(&pc, ROOT, "临时目标", "upload-target").await;
    let bytes = vec![7_u8; 200_000];
    let path = f.dir.path().join("source.bin");
    tokio::fs::write(&path, &bytes).await.unwrap();
    let source = OutgoingFile::open(&path, &pc.cancel).await.unwrap();
    let (send, receive) = tokio::sync::mpsc::channel::<Vec<u8>>(2);
    let stream = futures_util::stream::unfold(receive, |mut receive| async move {
        receive
            .recv()
            .await
            .map(|bytes| (Ok::<_, std::io::Error>(bytes), receive))
    });
    let request = reqwest::Client::new()
        .post(format!("{}/v1/drive/tree/files", f.server.origin.as_str()))
        .bearer_auth(pc.token.expose())
        .query(&[
            ("parentId", target.id.clone()),
            ("name", "test.bin".into()),
            ("size", source.size.to_string()),
            ("sha256", source.sha256.clone()),
        ])
        .header("Content-Length", source.size)
        .body(reqwest::Body::wrap_stream(stream));
    let uploading = tokio::spawn(async move { request.send().await.unwrap() });
    send.send(bytes[..128_000].to_vec()).await.unwrap();
    let blob_path = f.server.state.blobs.path_for(&source.sha256);
    let parent = blob_path.parent().unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if uploading.is_finished() {
                break;
            }
            let receiving = std::fs::read_dir(parent).is_ok_and(|entries| {
                entries.flatten().any(|entry| {
                    entry
                        .path()
                        .extension()
                        .is_some_and(|extension| extension == "part")
                })
            });
            if receiving {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the server must be receiving before the target is deleted");
    if uploading.is_finished() {
        let response = uploading.await.unwrap();
        let status = response.status();
        panic!(
            "upload ended before streaming: {status} {}",
            response.text().await.unwrap()
        );
    }
    pc.drive_tree_delete(Kind::Folder, &target.id, 1, &pc.options())
        .await
        .unwrap();
    send.send(bytes[128_000..].to_vec()).await.unwrap();
    drop(send);
    let response = uploading.await.unwrap();
    assert_eq!(response.status(), 404);
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap()["error"]["code"],
        "NOT_FOUND"
    );
    assert_eq!(pc.drive(&pc.options()).await.unwrap().used_bytes, 0);
    let refs: i64 = sqlx::query_scalar("SELECT count(*) FROM blobs WHERE sha256=?")
        .bind(&source.sha256)
        .fetch_one(&f.server.state.pool)
        .await
        .unwrap();
    assert_eq!(refs, 0);
    assert!(!blob_path.exists());
    f.server.stop().await;
}

#[tokio::test]
async fn concurrent_moves_allow_one_version_and_metadata_does_not_charge_bytes() {
    let mut f = fixture().await;
    let pc = register(&f, 'a', "PC", Platform::Windows).await;
    let moving = folder(&pc, ROOT, "moving", "moving").await;
    let target = folder(&pc, ROOT, "target", "target").await;
    let first = Move {
        kind: Kind::Folder,
        id: moving.id.clone(),
        version: 1,
        parent_id: target.id.clone(),
    };
    let second = Move {
        parent_id: ROOT.into(),
        ..first.clone()
    };
    let options = pc.options();
    let (a, b) = tokio::join!(
        pc.drive_tree_move(&first, &options),
        pc.drive_tree_move(&second, &options)
    );
    assert_ne!(a.is_ok(), b.is_ok());
    code(a.err().or_else(|| b.err()).unwrap(), "VERSION_CONFLICT");
    let root = pc
        .drive_tree(&Query::default(), &pc.options())
        .await
        .unwrap();
    assert_eq!(root.used_bytes, 0);
    let files_only = pc
        .drive_tree(
            &Query {
                folders_only: true,
                ..Query::default()
            },
            &pc.options(),
        )
        .await
        .unwrap();
    assert!(
        files_only
            .items
            .iter()
            .all(|entry| matches!(entry, Entry::Folder(_)))
    );
    f.server.stop().await;
}
