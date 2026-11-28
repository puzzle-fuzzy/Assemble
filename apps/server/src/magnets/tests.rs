use super::*;
use crate::http::TestServer;
use assemble_client::{Client, RequestOptions};
use assemble_protocol::{
    device::NewDevice,
    magnets::{Action, Control, PreviewInput, Start},
    primitives::Platform,
};
use assemble_store::secret::SecretString;
use librqbit::{
    AddTorrent, AddTorrentOptions, CreateTorrentOptions, Session, SessionOptions, create_torrent,
};

async fn user(server: &TestServer, name: &str) -> (Client, SecretString, String) {
    let account = crate::accounts::create(
        &server.state.pool,
        name,
        name,
        SecretString::new("a test password only"),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE accounts SET email=?,email_verified_at=1 WHERE id=?")
        .bind(format!("{name}@example.test"))
        .bind(&account.id)
        .execute(&server.state.pool)
        .await
        .unwrap();
    let token = crate::secrets::random_token().unwrap();
    crate::devices::register(
        &server.state.pool,
        &account.id,
        &NewDevice {
            name: "test Mac".into(),
            platform: Platform::Macos,
        },
        &crate::secrets::token_digest(token.expose()),
    )
    .await
    .unwrap();
    let client = Client::new(server.origin.clone()).unwrap();
    client.set_bearer(Some(token.clone()));
    (client, token, account.id)
}
fn options(cancel: &CancellationToken) -> RequestOptions<'_> {
    RequestOptions::new(cancel)
}
async fn until_task(
    client: &Client,
    id: &str,
    cancel: &CancellationToken,
    heartbeat: bool,
    condition: impl Fn(&Task) -> bool,
) -> Task {
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let task = if heartbeat {
                client
                    .magnet_control(
                        id,
                        &Control {
                            action: Action::Heartbeat,
                        },
                        &options(cancel),
                    )
                    .await
                    .unwrap()
            } else {
                client.magnet_status(id, &options(cancel)).await.unwrap()
            };
            assert_ne!(task.phase, DownloadPhase::Failed, "{:?}", task.error_code);
            if condition(&task) {
                return task;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("hosted task did not progress")
}
async fn seed(dir: &std::path::Path) -> (Arc<Session>, Vec<u8>, Vec<u8>, std::net::SocketAddr) {
    let bytes: Vec<u8> = (0..9 * 1024 * 1024)
        .map(|i| u8::try_from((i * 17 + i / 16384) % 251).unwrap())
        .collect();
    tokio::fs::write(dir.join("selected.bin"), &bytes)
        .await
        .unwrap();
    tokio::fs::write(dir.join("unselected.txt"), b"must not reach client")
        .await
        .unwrap();
    let torrent = create_torrent(
        dir,
        CreateTorrentOptions {
            piece_length: Some(65536),
            ..Default::default()
        },
        &librqbit::spawn_utils::BlockingSpawner::new(1),
    )
    .await
    .unwrap()
    .as_bytes()
    .unwrap();
    let session = Session::new_with_opts(
        dir.to_owned(),
        SessionOptions {
            dht: None,
            persistence: None,
            disable_local_service_discovery: true,
            listen: Some(librqbit::ListenerOptions {
                listen_addr: "127.0.0.1:0".parse().unwrap(),
                mode: librqbit::ListenerMode::TcpOnly,
                enable_upnp_port_forwarding: false,
                ..Default::default()
            }),
            ratelimits: librqbit::limits::LimitsConfig {
                upload_bps: std::num::NonZeroU32::new(1024 * 1024),
                download_bps: None,
            },
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let handle = session
        .add_torrent(
            AddTorrent::from_bytes(torrent.clone()),
            Some(AddTorrentOptions {
                output_folder: Some(dir.to_str().unwrap().into()),
                overwrite: true,
                ..Default::default()
            }),
        )
        .await
        .unwrap()
        .into_handle()
        .unwrap();
    handle.wait_until_initialized().await.unwrap();
    assert!(handle.stats().finished);
    let address = session.listen_addr().unwrap();
    (session, torrent.to_vec(), bytes, address)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_metadata_selection_pause_lease_restart_and_verified_https() {
    let seed_dir = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let (seed, torrent, payload, peer) = seed(seed_dir.path()).await;
    let mut server = TestServer::start_default(data.path()).await.unwrap();
    assert!(
        server
            .state
            .magnets
            .service
            .set(Service::for_test(vec![peer]))
            .is_ok()
    );
    let (alice, token, owner) = user(&server, "alice").await;
    let (bob, _, _) = user(&server, "bob").await;
    let cancel = CancellationToken::new();
    let hash = librqbit::torrent_from_bytes(&torrent)
        .unwrap()
        .info_hash
        .as_string();
    let preview_id = "b".repeat(32);
    let mut preview = alice
        .magnet_preview(
            &PreviewInput {
                request_id: preview_id.clone(),
                magnet: format!("magnet:?xt=urn:btih:{hash}"),
            },
            &options(&cancel),
        )
        .await
        .unwrap();
    assert_eq!(preview.phase, PreviewPhase::Resolving);
    tokio::time::timeout(Duration::from_secs(15), async {
        while preview.phase == PreviewPhase::Resolving {
            tokio::time::sleep(Duration::from_millis(50)).await;
            preview = alice
                .magnet_preview_status(&preview_id, &options(&cancel))
                .await
                .unwrap();
        }
    })
    .await
    .unwrap();
    assert_eq!(preview.phase, PreviewPhase::Ready);
    assert_eq!(
        bob.magnet_preview_status(&preview_id, &options(&cancel))
            .await
            .err()
            .unwrap()
            .status(),
        Some(404)
    );
    let input = Start {
        request_id: "a".repeat(32),
        info_hash: hash.clone(),
        metainfo: preview.metainfo.unwrap(),
        file_ids: vec![0],
    };
    let task = alice.magnet_start(&input, &options(&cancel)).await.unwrap();
    assert_eq!(task.phase, DownloadPhase::Paused);
    assert_eq!(task.total_bytes, payload.len() as u64);
    assert_eq!(
        alice
            .magnet_start(&input, &options(&cancel))
            .await
            .unwrap()
            .id,
        task.id
    );
    let mut conflict = input.clone();
    conflict.file_ids = vec![1];
    assert_eq!(
        alice
            .magnet_start(&conflict, &options(&cancel))
            .await
            .err()
            .unwrap()
            .status(),
        Some(409)
    );
    assert_eq!(
        bob.magnet_status(&task.id, &options(&cancel))
            .await
            .err()
            .unwrap()
            .status(),
        Some(404)
    );
    assert_eq!(
        bob.magnet_chunk(&task.id, 0, 0, &options(&cancel))
            .await
            .err()
            .unwrap()
            .status(),
        Some(404)
    );
    alice
        .magnet_control(
            &task.id,
            &Control {
                action: Action::Resume,
            },
            &options(&cancel),
        )
        .await
        .unwrap();
    until_task(&alice, &task.id, &cancel, true, |t| {
        t.received_bytes >= 128 * 1024
    })
    .await;
    let paused = alice
        .magnet_control(
            &task.id,
            &Control {
                action: Action::Pause,
            },
            &options(&cancel),
        )
        .await
        .unwrap();
    assert_eq!(paused.phase, DownloadPhase::Paused);
    tokio::time::sleep(Duration::from_millis(350)).await;
    assert_eq!(
        alice
            .magnet_status(&task.id, &options(&cancel))
            .await
            .unwrap()
            .received_bytes,
        paused.received_bytes
    );
    alice
        .magnet_control(
            &task.id,
            &Control {
                action: Action::Resume,
            },
            &options(&cancel),
        )
        .await
        .unwrap();
    until_task(&alice, &task.id, &cancel, true, |t| {
        t.received_bytes > paused.received_bytes
    })
    .await;
    server.state.magnets.leases.lock().unwrap().insert(
        (owner.clone(), task.id.clone()),
        Instant::now() - Duration::from_secs(1),
    );
    let expired = until_task(&alice, &task.id, &cancel, false, |t| {
        t.phase == DownloadPhase::Paused
    })
    .await;
    assert!(expired.received_bytes >= paused.received_bytes);
    server.stop().await;
    drop(server);
    let mut server = TestServer::start_default(data.path()).await.unwrap();
    assert!(
        server
            .state
            .magnets
            .service
            .set(Service::for_test(vec![peer]))
            .is_ok()
    );
    let alice = Client::new(server.origin.clone()).unwrap();
    alice.set_bearer(Some(token));
    let recovered = alice
        .magnet_status(&task.id, &options(&cancel))
        .await
        .unwrap();
    assert_eq!(recovered.phase, DownloadPhase::Paused);
    assert_eq!(recovered.received_bytes, expired.received_bytes);
    alice
        .magnet_control(
            &task.id,
            &Control {
                action: Action::Resume,
            },
            &options(&cancel),
        )
        .await
        .unwrap();
    until_task(&alice, &task.id, &cancel, true, |t| {
        t.phase == DownloadPhase::Completed
    })
    .await;
    let manifest = alice
        .magnet_manifest(&task.id, &options(&cancel))
        .await
        .unwrap();
    assert_eq!(manifest.info_hash, hash);
    assert_eq!(manifest.files.len(), 1);
    assert_eq!(manifest.files[0].file.name, "selected.bin");
    let mut received = Vec::new();
    for (index, digest) in manifest.files[0].chunks.iter().enumerate() {
        let part = alice
            .magnet_chunk(&task.id, 0, index as u64, &options(&cancel))
            .await
            .unwrap();
        assert_eq!(
            assemble_protocol::primitives::to_hex(&Sha256::digest(&part)),
            *digest
        );
        received.extend(part);
    }
    assert_eq!(received.len(), payload.len());
    assert_eq!(Sha256::digest(&received), Sha256::digest(&payload));
    assert_eq!(
        manifest.files[0].sha256,
        assemble_protocol::primitives::to_hex(&Sha256::digest(&payload))
    );
    assert_eq!(
        alice
            .magnet_chunk(&task.id, 1, 0, &options(&cancel))
            .await
            .err()
            .unwrap()
            .status(),
        Some(404)
    );
    let released = alice
        .magnet_control(
            &task.id,
            &Control {
                action: Action::Release,
            },
            &options(&cancel),
        )
        .await
        .unwrap();
    assert!(released.released);
    assert!(
        alice
            .magnet_control(
                &task.id,
                &Control {
                    action: Action::Release
                },
                &options(&cancel)
            )
            .await
            .unwrap()
            .released
    );
    assert!(
        alice
            .magnet_start(&input, &options(&cancel))
            .await
            .unwrap()
            .released
    );
    let reserved: i64 =
        sqlx::query_scalar("SELECT reserved_bytes FROM hosted_magnets WHERE account_id=? AND id=?")
            .bind(owner)
            .bind(&task.id)
            .fetch_one(&server.state.pool)
            .await
            .unwrap();
    assert_eq!(reserved, 0);
    assert_eq!(
        alice
            .magnet_manifest(&task.id, &options(&cancel))
            .await
            .err()
            .unwrap()
            .status(),
        Some(404)
    );
    server.stop().await;
    seed.stop().await;
}

#[tokio::test]
async fn anonymous_and_foreign_calls_never_enter_private_downloads() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = TestServer::start_default(dir.path()).await.unwrap();
    let client = Client::new(server.origin.clone()).unwrap();
    let cancel = CancellationToken::new();
    let error = client
        .magnet_preview(
            &PreviewInput {
                request_id: "a".repeat(32),
                magnet: "magnet:?xt=urn:btih:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            },
            &options(&cancel),
        )
        .await
        .err()
        .unwrap();
    assert_eq!(error.status(), Some(401));
    assert!(server.state.magnets.service.get().is_none());
    let (alice, _, _) = user(&server, "alice").await;
    assert_eq!(
        alice
            .magnet_status(&"a".repeat(32), &options(&cancel))
            .await
            .err()
            .unwrap()
            .status(),
        Some(404)
    );
    assert!(server.state.magnets.service.get().is_none());
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_fences_a_late_start_without_reserving_or_opening_bt() {
    let seed_dir = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let (seed, torrent, _, _) = seed(seed_dir.path()).await;
    let mut server = TestServer::start_default(data.path()).await.unwrap();
    let (alice, _, owner) = user(&server, "alice").await;
    let cancel = CancellationToken::new();
    let id = "c".repeat(32);
    let closed = alice
        .magnet_control(
            &id,
            &Control {
                action: Action::Cancel,
            },
            &options(&cancel),
        )
        .await
        .unwrap();
    assert_eq!(closed.phase, DownloadPhase::Cancelled);
    let input = Start {
        request_id: id.clone(),
        info_hash: librqbit::torrent_from_bytes(&torrent)
            .unwrap()
            .info_hash
            .as_string(),
        metainfo: {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.encode(torrent)
        },
        file_ids: vec![0],
    };
    for _ in 0..2 {
        let late = alice.magnet_start(&input, &options(&cancel)).await.unwrap();
        assert_eq!(late.phase, DownloadPhase::Cancelled);
        assert_eq!(late.info_hash, input.info_hash);
        assert_eq!(late.file_ids, input.file_ids);
    }
    for action in [Action::Resume, Action::Release, Action::Cancel] {
        assert_eq!(
            alice
                .magnet_control(&id, &Control { action }, &options(&cancel))
                .await
                .unwrap()
                .phase,
            DownloadPhase::Cancelled
        );
    }
    let reserved: i64 =
        sqlx::query_scalar("SELECT reserved_bytes FROM hosted_magnets WHERE account_id=? AND id=?")
            .bind(owner)
            .bind(&id)
            .fetch_one(&server.state.pool)
            .await
            .unwrap();
    assert_eq!(reserved, 0);
    assert!(server.state.magnets.service.get().is_none());
    let mut changed = input;
    changed.file_ids = vec![1];
    assert_eq!(
        alice
            .magnet_start(&changed, &options(&cancel))
            .await
            .unwrap_err()
            .status(),
        Some(409)
    );
    server.stop().await;
    seed.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_starts_share_the_budget_and_cancel_releases_the_reservation() {
    let seed_dir = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let (seed, torrent, payload, _) = seed(seed_dir.path()).await;
    let mut config = crate::config::Config::development(data.path());
    config.addr = "127.0.0.1:0".parse().unwrap();
    config.storage_budget_bytes = payload.len() as u64 * 2;
    let pool = crate::db::open(data.path()).await.unwrap();
    let mut server = TestServer::start(config, pool).await.unwrap();
    let (alice, _, _) = user(&server, "alice").await;
    let cancel = CancellationToken::new();
    let first = Start {
        request_id: "a".repeat(32),
        info_hash: librqbit::torrent_from_bytes(&torrent)
            .unwrap()
            .info_hash
            .as_string(),
        metainfo: {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.encode(torrent)
        },
        file_ids: vec![0],
    };
    let mut second = first.clone();
    second.request_id = "b".repeat(32);
    let options = options(&cancel);
    let (a, b) = tokio::join!(
        alice.magnet_start(&first, &options),
        alice.magnet_start(&second, &options)
    );
    let (admitted, rejected) = match (a, b) {
        (Ok(a), Err(b)) | (Err(b), Ok(a)) => (a, b),
        _ => panic!("exactly one concurrent reservation must fit"),
    };
    assert_eq!(rejected.status(), Some(413));
    assert!(server.state.magnets.service.get().is_none());
    alice
        .magnet_control(
            &admitted.id,
            &Control {
                action: Action::Cancel,
            },
            &options,
        )
        .await
        .unwrap();
    let reserved: i64 =
        sqlx::query_scalar("SELECT coalesce(sum(reserved_bytes),0) FROM hosted_magnets")
            .fetch_one(&server.state.pool)
            .await
            .unwrap();
    assert_eq!(reserved, 0);
    let mut fresh = first;
    assert!(server.state.magnets.service.get().is_none());
    fresh.request_id = "d".repeat(32);
    assert_eq!(
        alice.magnet_start(&fresh, &options).await.unwrap().phase,
        DownloadPhase::Paused
    );
    server.stop().await;
    seed.stop().await;
}

async fn local_until(
    manager: &assemble_download::DownloadManager,
    id: &str,
    condition: impl Fn(&assemble_protocol::downloads::DownloadTask) -> bool,
) -> assemble_protocol::downloads::DownloadTask {
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let task = manager
                .view()
                .tasks
                .into_iter()
                .find(|t| t.id == id)
                .unwrap();
            if condition(&task) {
                return task;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("client task did not progress")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn desktop_queue_uses_https_rejects_corruption_and_reuses_verified_cache_after_restart() {
    let seed_dir = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let local = tempfile::tempdir().unwrap();
    let (seed, torrent, payload, peer) = seed(seed_dir.path()).await;
    let mut server = TestServer::start_default(data.path()).await.unwrap();
    assert!(
        server
            .state
            .magnets
            .service
            .set(Service::for_test(vec![peer]))
            .is_ok()
    );
    let (client, _, owner) = user(&server, "alice").await;
    let store = Arc::new(assemble_store::secret::MemorySecretStore::default());
    let history = local.path().join("downloads.json");
    let folder = local.path().join("received");
    let manager = assemble_download::DownloadManager::for_test_server(
        history.clone(),
        folder.clone(),
        client.clone(),
        store.clone(),
    )
    .unwrap();
    let hash = librqbit::torrent_from_bytes(&torrent)
        .unwrap()
        .info_hash
        .as_string();
    let preview = manager
        .prepare(assemble_protocol::downloads::DownloadInput {
            kind: assemble_protocol::downloads::DownloadKind::Magnet,
            url: format!("magnet:?xt=urn:btih:{hash}"),
            page_url: None,
            title: None,
            magnet_network: None,
        })
        .await
        .unwrap();
    assert_eq!(preview.source_host, "Assemble");
    assert_eq!(preview.files.len(), 2);
    let id = manager
        .start_selected(&preview.id, "magnet", &[0])
        .await
        .unwrap();
    local_until(&manager, &id, |t| t.received_bytes > 128 * 1024).await;
    manager.pause(&id).await.unwrap();
    let paused = local_until(&manager, &id, |t| t.phase == DownloadPhase::Paused).await;
    tokio::time::sleep(Duration::from_millis(350)).await;
    assert_eq!(
        manager.view().tasks[0].received_bytes,
        paused.received_bytes
    );
    manager.resume(&id).await.unwrap();
    local_until(&manager, &id, |t| {
        t.phase == DownloadPhase::Downloading && t.received_bytes > paused.received_bytes
    })
    .await;
    manager.shutdown().await;
    drop(manager);
    let manager = assemble_download::DownloadManager::for_test_server(
        history.clone(),
        folder.clone(),
        client.clone(),
        store.clone(),
    )
    .unwrap();
    assert_eq!(manager.view().tasks[0].phase, DownloadPhase::Paused);
    // 第二个 HTTPS 分片被故意损坏：第一片已同步到私有缓存，不能发布坏文件。
    server
        .state
        .magnets
        .fail_chunk
        .store(1, std::sync::atomic::Ordering::Relaxed);
    manager.resume(&id).await.unwrap();
    let failed = local_until(&manager, &id, |t| t.phase == DownloadPhase::Failed).await;
    assert_eq!(failed.error_code.as_deref(), Some("DOWNLOAD_INVALID_MEDIA"));
    assert_eq!(
        server
            .state
            .magnets
            .chunk_calls
            .lock()
            .unwrap()
            .iter()
            .filter(|index| **index == 0)
            .count(),
        1
    );
    assert!(!folder.join("selected.bin").exists());
    manager.shutdown().await;
    drop(manager);
    let manager =
        assemble_download::DownloadManager::for_test_server(history, folder.clone(), client, store)
            .unwrap();
    manager.resume(&id).await.unwrap();
    let complete = local_until(&manager, &id, |t| t.phase == DownloadPhase::Completed).await;
    let name = complete.file_name.unwrap();
    let received = tokio::fs::read(folder.join(name).join("selected.bin"))
        .await
        .unwrap();
    assert_eq!(Sha256::digest(&received), Sha256::digest(&payload));
    assert_eq!(
        server
            .state
            .magnets
            .chunk_calls
            .lock()
            .unwrap()
            .iter()
            .filter(|index| **index == 0)
            .count(),
        1,
        "verified first chunk must be reused"
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let reserved: i64 = sqlx::query_scalar(
                "SELECT coalesce(sum(reserved_bytes),0) FROM hosted_magnets WHERE account_id=?",
            )
            .bind(&owner)
            .fetch_one(&server.state.pool)
            .await
            .unwrap();
            if reserved == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    manager.shutdown().await;
    server.stop().await;
    seed.stop().await;
}
