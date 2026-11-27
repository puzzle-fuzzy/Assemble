use super::*;
use assemble_store::secret::MemorySecretStore;
use librqbit::{
    AddTorrent, AddTorrentOptions, CreateTorrentOptions, Session, SessionOptions, create_torrent,
};

#[test]
fn rejects_unsafe_paths_and_unsupported_sources() {
    for name in [
        "../secret",
        "/tmp/file",
        "C:/file",
        "a/../file",
        "a\\file",
        "CON",
        "a//b",
    ] {
        assert!(files::relative_path(name).is_err(), "{name}");
    }
    assert!(engine::source("https://example.com/video").is_err());
    assert!(engine::source("magnet:?xt=urn:btih:invalid").is_err());
}

async fn wait_for(
    manager: &MagnetManager,
    id: &str,
    condition: impl Fn(&DownloadTask) -> bool,
) -> DownloadTask {
    tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            let task = manager.record(id).unwrap().view;
            assert_ne!(task.phase, DownloadPhase::Failed, "{:?}", task.error_code);
            if condition(&task) {
                return task;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("torrent did not reach expected state")
}

/// 不依赖公网资源：在回环地址实际交换元数据和分片，暂停后销毁管理器，再读盘恢复。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn selects_files_pauses_and_recovers_after_restart() {
    let seed_dir = tempfile::tempdir().unwrap();
    let receive = tempfile::tempdir().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    let payload: Vec<u8> = (0..1_048_576)
        .map(|i| u8::try_from((i * 17 + i / 16384) % 251).unwrap())
        .collect();
    tokio::fs::create_dir(seed_dir.path().join("目录"))
        .await
        .unwrap();
    tokio::fs::write(seed_dir.path().join("目录/selected.bin"), &payload)
        .await
        .unwrap();
    tokio::fs::write(seed_dir.path().join("excluded.txt"), b"do not publish")
        .await
        .unwrap();
    tokio::fs::write(seed_dir.path().join("empty.txt"), b"")
        .await
        .unwrap();
    let torrent = create_torrent(
        seed_dir.path(),
        CreateTorrentOptions {
            piece_length: Some(16384),
            ..Default::default()
        },
        &librqbit::spawn_utils::BlockingSpawner::new(1),
    )
    .await
    .unwrap();
    let bytes = torrent.as_bytes().unwrap();
    let hash = librqbit::torrent_from_bytes(&bytes)
        .unwrap()
        .info_hash
        .as_string();
    let seed = Session::new_with_opts(
        seed_dir.path().to_owned(),
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
                upload_bps: std::num::NonZeroU32::new(128 * 1024),
                download_bps: None,
            },
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let seeder = seed
        .add_torrent(
            AddTorrent::from_bytes(bytes),
            Some(AddTorrentOptions {
                output_folder: Some(seed_dir.path().to_str().unwrap().to_owned()),
                overwrite: true,
                ..Default::default()
            }),
        )
        .await
        .unwrap()
        .into_handle()
        .unwrap();
    seeder.wait_until_initialized().await.unwrap();
    assert!(seeder.stats().finished);
    let port = seed.listen_addr().unwrap().port();
    // 只让 Tracker 返回种子地址；不提供 x.pe 且禁用 DHT，元数据必须真正经过 tr 找节点。
    let tracker = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let tracker_addr = tracker.local_addr().unwrap();
    let announces = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let calls = Arc::clone(&announces);
    let tracker_task = tokio::spawn(async move {
        let mut request = [0_u8; 2048];
        while let Ok((size, remote)) = tracker.recv_from(&mut request).await {
            if size < 16 {
                continue;
            }
            let action = u32::from_be_bytes(request[8..12].try_into().unwrap());
            let mut response = Vec::new();
            response.extend_from_slice(&action.to_be_bytes());
            response.extend_from_slice(&request[12..16]);
            if action == 0 {
                response.extend_from_slice(&42_u64.to_be_bytes());
            } else if action == 1 {
                calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                response.extend_from_slice(&60_u32.to_be_bytes());
                response.extend_from_slice(&0_u32.to_be_bytes());
                response.extend_from_slice(&1_u32.to_be_bytes());
                response.extend_from_slice(&[127, 0, 0, 1]);
                response.extend_from_slice(&port.to_be_bytes());
            } else {
                continue;
            }
            tracker.send_to(&response, remote).await.unwrap();
        }
    });
    let source =
        format!("magnet:?xt=urn:btih:{hash}&tr=udp://{tracker_addr}/announce&dn=credential-marker");
    let store = Arc::new(MemorySecretStore::default());
    let history = state_dir.path().join("magnet.json");
    let open = || {
        MagnetManager::open_with(
            history.clone(),
            Arc::new(Semaphore::new(2)),
            Arc::new(|_| {}),
            Some(store.clone()),
            engine::Options {
                local_test: true,
                ..engine::Options::default()
            },
        )
        .unwrap()
    };
    let manager = open();
    let preview = manager.prepare(&source, None).await.unwrap();
    assert!(announces.load(std::sync::atomic::Ordering::SeqCst) > 0);
    assert_eq!(preview.files.len(), 3);
    let ids: Vec<_> = preview
        .files
        .iter()
        .filter(|f| f.name != "excluded.txt")
        .map(|f| f.id)
        .collect();
    assert!(
        manager
            .start(&preview.id, &[], receive.path().to_owned(), None)
            .await
            .is_err()
    );
    let id = manager
        .start(&preview.id, &ids, receive.path().to_owned(), None)
        .await
        .unwrap();
    wait_for(&manager, &id, |t| {
        t.received_bytes >= 16384 && t.received_bytes < payload.len() as u64
    })
    .await;
    manager.pause(&id).await.unwrap();
    let paused = manager.record(&id).unwrap().view;
    assert_eq!(paused.phase, DownloadPhase::Paused);
    let uploaded = seeder.stats().uploaded_bytes;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(
        seeder.stats().uploaded_bytes,
        uploaded,
        "paused download still receives pieces"
    );
    let cache = files::cache(receive.path(), &id, false).await.unwrap();
    assert!(cache.exists());
    let json = tokio::fs::read_to_string(&history).await.unwrap();
    assert!(
        !json.contains("magnet:?")
            && !json.contains("credential-marker")
            && !json.contains("127.0.0.1")
    );
    // 恢复时不能相信磁盘上的分片，故意破坏已有数据，必须重新下载并校验。
    let cached = cache.join("data/目录/selected.bin");
    tokio::fs::write(&cached, vec![0; payload.len()])
        .await
        .unwrap();
    manager.shutdown().await;
    drop(manager);
    let recovered = open();
    assert_eq!(
        recovered.record(&id).unwrap().view.phase,
        DownloadPhase::Paused
    );
    // 同名目录是用户已有内容，发布必须换一个名字。
    let title = recovered.record(&id).unwrap().view.title;
    tokio::fs::create_dir(receive.path().join(&title))
        .await
        .unwrap();
    tokio::fs::write(receive.path().join(&title).join("keep"), b"existing")
        .await
        .unwrap();
    recovered.resume(&id).await.unwrap();
    wait_for(&recovered, &id, |t| t.phase == DownloadPhase::Completed).await;
    let published = recovered.completed_path(&id).await.unwrap();
    assert_ne!(published, receive.path().join(&title));
    assert_eq!(
        tokio::fs::read(published.join("目录/selected.bin"))
            .await
            .unwrap(),
        payload
    );
    assert!(published.join("empty.txt").is_file());
    assert!(!published.join("excluded.txt").exists());
    assert_eq!(
        tokio::fs::read(receive.path().join(title).join("keep"))
            .await
            .unwrap(),
        b"existing"
    );
    recovered.shutdown().await;
    assert!(!cache.exists(), "completed cache must be cleaned");
    assert!(store.get(&key(&id)).unwrap().is_none());
    let uploaded = seeder.stats().uploaded_bytes;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(seeder.stats().uploaded_bytes, uploaded);
    seed.stop().await;
    tracker_task.abort();
    let _ = tracker_task.await;
}

#[test]
fn selection_checks_ids_and_selected_capacity() {
    let all = vec![
        DownloadFile {
            id: 1,
            name: "small.bin".into(),
            size: 10,
        },
        DownloadFile {
            id: 2,
            name: "large.bin".into(),
            size: engine::MAX_SELECTED + 1,
        },
    ];
    assert_eq!(select_files(&all, &[1]).unwrap().1, 10);
    assert!(select_files(&all, &[1, 1]).is_err());
    assert!(select_files(&all, &[3]).is_err());
    assert!(matches!(
        select_files(&all, &[2]),
        Err(DownloadError::Limit)
    ));
}

#[tokio::test]
async fn cancelling_metadata_and_shutdown_wait_for_engine_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    let manager = MagnetManager::open_with(
        dir.path().join("tasks.json"),
        Arc::new(Semaphore::new(2)),
        Arc::new(|_| {}),
        Some(Arc::new(MemorySecretStore::default())),
        engine::Options {
            local_test: true,
            ..engine::Options::default()
        },
    )
    .unwrap();
    let worker = Arc::clone(&manager);
    // 回环节点保持握手沉默，确保取消的是正在执行的解析。
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let uri = format!(
        "magnet:?xt=urn:btih:{}&x.pe={}",
        "a".repeat(40),
        listener.local_addr().unwrap()
    );
    let preparation = tokio::spawn(async move { worker.prepare(&uri, None).await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while manager.lock().preparing.is_none() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    manager.cancel_preparation();
    assert!(matches!(
        preparation.await.unwrap(),
        Err(DownloadError::Cancelled)
    ));
    assert!(manager.lock().preparing.is_none());
    manager.shutdown().await;
    assert!(manager.tasks().is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn cache_rejects_links_without_changing_the_target_permissions() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    std::fs::set_permissions(target.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    symlink(
        target.path(),
        dir.path()
            .join(format!(".assemble-magnet-{}", "a".repeat(32))),
    )
    .unwrap();
    assert!(
        files::cache(dir.path(), &"a".repeat(32), true)
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::metadata(target.path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    assert!(
        files::remove_cache(dir.path(), &"a".repeat(32))
            .await
            .is_err()
    );
    assert!(target.path().exists());
}

#[tokio::test]
async fn active_history_restores_paused_and_cancel_discards_only_owned_cache() {
    let dir = tempfile::tempdir().unwrap();
    let output = tempfile::tempdir().unwrap();
    let fixture = tempfile::tempdir().unwrap();
    tokio::fs::write(fixture.path().join("payload.txt"), b"fixture content")
        .await
        .unwrap();
    let torrent = create_torrent(
        fixture.path(),
        CreateTorrentOptions::default(),
        &librqbit::spawn_utils::BlockingSpawner::new(1),
    )
    .await
    .unwrap();
    let bytes = torrent.as_bytes().unwrap();
    let (metadata, all, title) = engine::describe(&bytes).unwrap();
    let id = "b".repeat(32);
    let source = format!(
        "magnet:?xt=urn:btih:{}",
        librqbit::torrent_from_bytes(&metadata)
            .unwrap()
            .info_hash
            .as_string()
    );
    let store = Arc::new(MemorySecretStore::default());
    credentials::put(store.as_ref(), &id, &SecretString::new(source)).unwrap();
    let history = dir.path().join("tasks.json");
    let record = Record {
        view: DownloadTask {
            id: id.clone(),
            initiated_by: None,
            kind: DownloadKind::Magnet,
            title,
            file_name: None,
            source_host: "BitTorrent".into(),
            phase: DownloadPhase::Downloading,
            received_bytes: 3,
            total_bytes: Some(15),
            completed_segments: 0,
            total_segments: None,
            bytes_per_second: 30,
            media_seconds: None,
            duration_seconds: None,
            error_code: None,
            retryable: false,
            created_at: Timestamp::now(),
        },
        folder: output.path().to_owned(),
        metadata,
        selected: all,
        publication: None,
    };
    write_private_json(&history, &vec![record]).unwrap();
    let cache = files::cache(output.path(), &id, true).await.unwrap();
    tokio::fs::write(cache.join("partial"), b"cached")
        .await
        .unwrap();
    let keep = output.path().join("user-file");
    tokio::fs::write(&keep, b"keep").await.unwrap();
    let manager = MagnetManager::open_with(
        history,
        Arc::new(Semaphore::new(2)),
        Arc::new(|_| {}),
        Some(store.clone()),
        engine::Options {
            local_test: true,
            ..engine::Options::default()
        },
    )
    .unwrap();
    let task = manager.tasks().remove(0);
    assert_eq!(task.phase, DownloadPhase::Paused);
    assert_eq!(task.bytes_per_second, 0);
    assert_eq!(task.received_bytes, 3);
    manager.cancel(&id).await.unwrap();
    assert_eq!(manager.tasks()[0].phase, DownloadPhase::Cancelled);
    assert!(!cache.exists());
    assert!(keep.exists());
    assert!(store.get(&key(&id)).unwrap().is_none());
    manager.shutdown().await;
}

#[test]
fn keychain_chunks_keep_unicode_and_remove_every_part() {
    let store = MemorySecretStore::default();
    let source = SecretString::new(format!("magnet:?dn={}", "文件".repeat(1800)));
    credentials::put(&store, "task", &source).unwrap();
    assert_eq!(
        credentials::get(&store, "task").unwrap().expose(),
        source.expose()
    );
    credentials::delete(&store, "task").unwrap();
    assert!(credentials::get(&store, "task").is_err());
    assert!(store.get(&key("task.0")).unwrap().is_none());
}

/// 仅手工对合法公开资源测速；来源从文件读入，打印数值，不碰真实钥匙串与任务历史。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires an explicitly supplied public magnet and network access"]
async fn public_tracker_download_benchmark() {
    let input = std::env::var("ASSEMBLE_PUBLIC_MAGNET_FILE").expect("public magnet file");
    let source = std::fs::read_to_string(input).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let manager = MagnetManager::open_with(
        dir.path().join("history.json"),
        Arc::new(Semaphore::new(2)),
        Arc::new(|_| {}),
        Some(Arc::new(MemorySecretStore::default())),
        engine::Options {
            pool: Some(tracker_pool::Pool::open(dir.path().join("trackers.json"))),
            ..engine::Options::default()
        },
    )
    .unwrap();
    let started = Instant::now();
    let preview = manager.prepare(source.trim(), None).await;
    let metadata_ms = started.elapsed().as_millis();
    let Ok(preview) = preview else {
        manager.shutdown().await;
        println!(
            "BENCHMARK {{\"metadataMs\":{metadata_ms},\"error\":\"{}\",\"receivedBytes\":0}}",
            preview.err().unwrap().code()
        );
        return;
    };
    println!(
        "BENCHMARK_METADATA {{\"metadataMs\":{metadata_ms},\"files\":{}}}",
        preview.files.len()
    );
    let ids: Vec<_> = preview.files.iter().map(|f| f.id).collect();
    let id = manager
        .start(&preview.id, &ids, dir.path().to_owned(), None)
        .await
        .unwrap();
    let download = Instant::now();
    let mut first_byte_ms = None;
    let mut peak_bps = 0;
    loop {
        let task = manager.record(&id).unwrap().view;
        if task.received_bytes > 0 && first_byte_ms.is_none() {
            first_byte_ms = Some(download.elapsed().as_millis());
        }
        peak_bps = peak_bps.max(task.bytes_per_second);
        if !task.phase.active()
            || download.elapsed() >= Duration::from_secs(90)
            || task.received_bytes >= 128 * 1024 * 1024
        {
            let elapsed_ms = download.elapsed().as_millis().max(1);
            let first = first_byte_ms.map_or_else(|| "null".into(), |v| v.to_string());
            println!(
                "BENCHMARK {{\"metadataMs\":{metadata_ms},\"firstByteMs\":{first},\"downloadMs\":{elapsed_ms},\"receivedBytes\":{},\"averageBps\":{},\"peakBps\":{peak_bps},\"completed\":{}}}",
                task.received_bytes,
                u128::from(task.received_bytes) * 1000 / elapsed_ms,
                task.phase == DownloadPhase::Completed
            );
            if task.phase.active() {
                manager.cancel(&id).await.unwrap();
            }
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires an explicitly supplied public magnet and network access"]
async fn public_tracker_connection_diagnostics() {
    let input = std::env::var("ASSEMBLE_PUBLIC_MAGNET_FILE").expect("public magnet file");
    let value = std::fs::read_to_string(input).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let pool = tracker_pool::Pool::open(dir.path().join("trackers.json"));
    let options = engine::Options {
        pool: Some(Arc::clone(&pool)),
        ..engine::Options::default()
    };
    let result = engine::connection_options(
        &engine::source(value.trim()).unwrap(),
        &options,
        &CancellationToken::new(),
    )
    .await;
    match result {
        Ok((trackers, peers)) => println!(
            "DISCOVERY {{\"udpTrackers\":{},\"httpAndExplicitPeers\":{}}}",
            trackers.len(),
            peers.len()
        ),
        Err(error) => println!("DISCOVERY {{\"error\":\"{}\"}}", error.code()),
    }
    pool.shutdown().await;
}
