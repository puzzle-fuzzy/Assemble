//! 客户端只连接第一方 HTTPS；摘要不一致的缓存和分片都不能发布。
use super::{MagnetManager, Record, engine, files};
use crate::{DownloadError, Result, random_id};
use assemble_client::{Client, ClientError, RequestOptions};
use assemble_protocol::downloads::DownloadPhase;
use assemble_protocol::magnets::{self, Action, Control, PreviewPhase};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct RemoteRecord {
    pub account_id: String,
    pub task_id: Option<String>,
    #[serde(default)]
    pub released: bool,
}
fn failure(error: &ClientError) -> DownloadError {
    match error {
        ClientError::Cancelled => DownloadError::Cancelled,
        ClientError::Http { status: 401, .. } => DownloadError::SignInRequired,
        ClientError::Http { status: 403, .. } => DownloadError::AccountMismatch,
        ClientError::Http { status: 404, .. } => DownloadError::Expired,
        ClientError::Http { status: 413, .. } => DownloadError::ServerQuota,
        ClientError::Http { status: 429, .. } => DownloadError::Busy,
        ClientError::InvalidResponse => DownloadError::InvalidMedia,
        _ => DownloadError::ServerUnavailable,
    }
}
async fn account(client: &Client, cancel: &CancellationToken) -> Result<String> {
    if !client.has_bearer() {
        return Err(DownloadError::SignInRequired);
    }
    Ok(client
        .me(&RequestOptions::new(cancel))
        .await
        .map_err(|error| failure(&error))?
        .account
        .id)
}
pub(super) async fn prepare(
    client: &Client,
    value: &str,
    cancel: &CancellationToken,
) -> Result<engine::Prepared> {
    let source = engine::source(value)?;
    let owner = account(client, cancel).await?;
    let id = random_id()?;
    let work=async {
        let mut preview=client.magnet_preview(&magnets::PreviewInput{request_id:id.clone(),magnet:source.expose().into()},&RequestOptions::new(cancel)).await.map_err(|error|failure(&error))?;
        let deadline=tokio::time::Instant::now()+Duration::from_secs(100);
        loop {
            if preview.id!=id || preview.account_id!=owner{return Err(DownloadError::InvalidMedia);}
            match preview.phase {
                PreviewPhase::Ready=>{
                    let encoded=preview.metainfo.ok_or(DownloadError::InvalidMedia)?;
                    if encoded.len()>6*1024*1024{return Err(DownloadError::Limit);}
                    let metadata=STANDARD.decode(encoded).map_err(|_|DownloadError::InvalidMedia)?;
                    engine::check_identity(&source,&metadata)?;let(metadata,files,title)=engine::describe(&metadata)?;
                    return Ok(engine::Prepared{source,metadata,files,title,peers:Vec::new(),remote:Some(RemoteRecord{account_id:owner,task_id:None,released:false})});
                }
                PreviewPhase::Failed=>return Err(match preview.error_code.as_deref(){
                    Some("DOWNLOAD_SERVER_UNAVAILABLE"|"DOWNLOAD_NETWORK"|"DOWNLOAD_STORAGE")=>DownloadError::ServerUnavailable,
                    Some("DOWNLOAD_BUSY")=>DownloadError::Busy,
                    Some("DOWNLOAD_LIMIT")=>DownloadError::Limit,
                    Some("DOWNLOAD_UNSUPPORTED")=>DownloadError::Unsupported,
                    Some("DOWNLOAD_CANCELLED")=>DownloadError::Cancelled,
                    _=>DownloadError::MetadataUnavailable,
                }),
                PreviewPhase::Resolving=>{}
            }
            tokio::select!{()=cancel.cancelled()=>return Err(DownloadError::Cancelled),()=tokio::time::sleep_until(deadline)=>return Err(DownloadError::MetadataUnavailable),()=tokio::time::sleep(Duration::from_millis(500))=>{}}
            preview=client.magnet_preview_status(&id,&RequestOptions::new(cancel)).await.map_err(|error|failure(&error))?;
        }
    }.await;
    if work.is_err() {
        let cleanup = CancellationToken::new();
        let _ = client
            .magnet_cancel_preview(
                &id,
                &RequestOptions {
                    cancel: &cleanup,
                    timeout: Some(Duration::from_secs(6)),
                },
            )
            .await;
    }
    work
}
fn save(manager: &MagnetManager, id: &str, remote: RemoteRecord) -> Result<()> {
    let mut state = manager.lock();
    state
        .records
        .iter_mut()
        .find(|r| r.view.id == id)
        .ok_or(DownloadError::Expired)?
        .remote = Some(remote);
    manager.persist(&state)
}
fn valid_task(task: &magnets::Task, remote: &RemoteRecord, record: &Record) -> Result<()> {
    let expected = librqbit::torrent_from_bytes(&record.metadata)
        .map_err(|_| DownloadError::InvalidMedia)?
        .info_hash
        .as_string();
    let mut wanted: Vec<_> = record.selected.iter().map(|f| f.id).collect();
    wanted.sort_unstable();
    let mut actual = task.file_ids.clone();
    actual.sort_unstable();
    if Some(&task.id) != remote.task_id.as_ref()
        || task.account_id != remote.account_id
        || expected != task.info_hash
        || wanted != actual
        || task.total_bytes != record.view.total_bytes.unwrap_or(0)
        || task.received_bytes > task.total_bytes
        || matches!(task.phase, DownloadPhase::Receiving | DownloadPhase::Muxing)
    {
        return Err(DownloadError::InvalidMedia);
    }
    Ok(())
}
async fn attach(
    manager: &MagnetManager,
    client: &Client,
    id: &str,
    record: &Record,
    cancel: &CancellationToken,
) -> Result<RemoteRecord> {
    let owner = account(client, cancel).await?;
    let mut remote = record.remote.clone().unwrap_or(RemoteRecord {
        account_id: owner.clone(),
        task_id: None,
        released: false,
    });
    if owner != remote.account_id {
        return Err(DownloadError::AccountMismatch);
    }
    if let Some(task) = &remote.task_id {
        match client
            .magnet_status(task, &RequestOptions::new(cancel))
            .await
        {
            Ok(task) => {
                valid_task(&task, &remote, record)?;
                if !task.released && task.phase != DownloadPhase::Cancelled {
                    return Ok(remote);
                }
                remote.task_id = None;
            }
            Err(ClientError::Http { status: 404, .. }) => {}
            Err(error) => return Err(failure(&error)),
        }
    }
    if remote.task_id.is_none() {
        remote.task_id = Some(random_id()?);
    }
    remote.released = false;
    save(manager, id, remote.clone())?;
    let input = magnets::Start {
        request_id: remote.task_id.clone().ok_or(DownloadError::Storage)?,
        info_hash: librqbit::torrent_from_bytes(&record.metadata)
            .map_err(|_| DownloadError::InvalidMedia)?
            .info_hash
            .as_string(),
        metainfo: STANDARD.encode(&record.metadata),
        file_ids: record.selected.iter().map(|f| f.id).collect(),
    };
    let task = client
        .magnet_start(&input, &RequestOptions::new(cancel))
        .await
        .map_err(|error| failure(&error))?;
    valid_task(&task, &remote, record)?;
    Ok(remote)
}
pub(super) async fn receive(
    manager: &MagnetManager,
    client: &Client,
    id: &str,
    record: &Record,
    data: &std::path::Path,
    cancel: &CancellationToken,
) -> Result<()> {
    let remote = attach(manager, client, id, record, cancel).await?;
    let task = remote.task_id.as_ref().ok_or(DownloadError::Storage)?;
    let result=async {
        client.magnet_control(task,&Control{action:Action::Resume},&RequestOptions::new(cancel)).await.map_err(|error|failure(&error))?;
        loop {
            let status=client.magnet_control(task,&Control{action:Action::Heartbeat},&RequestOptions::new(cancel)).await.map_err(|error|failure(&error))?;valid_task(&status,&remote,record)?;
            if status.released{return Err(DownloadError::Expired);}
if status.phase==DownloadPhase::Completed{break;}
            if status.phase==DownloadPhase::Paused{return Err(DownloadError::Cancelled);}
            if matches!(status.phase,DownloadPhase::Failed|DownloadPhase::Cancelled){return Err(DownloadError::SourceUnavailable);}
            manager.update(id,status.phase,status.received_bytes,status.bytes_per_second);
            tokio::select!{()=cancel.cancelled()=>return Err(DownloadError::Cancelled),()=tokio::time::sleep(Duration::from_secs(1))=>{}}
        }
        let manifest=client.magnet_manifest(task,&RequestOptions::new(cancel)).await.map_err(|error|failure(&error))?;validate_manifest(&manifest,record)?;
        pull(manager,client,id,task,data,&manifest,cancel).await
    }.await;
    // 暂停令牌已经取消，停止请求使用独立的短时令牌，退出时也等待它结束。
    let cleanup = CancellationToken::new();
    let _ = client
        .magnet_control(
            task,
            &Control {
                action: Action::Pause,
            },
            &RequestOptions {
                cancel: &cleanup,
                timeout: Some(Duration::from_secs(6)),
            },
        )
        .await;
    result
}
fn digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}
fn validate_manifest(manifest: &magnets::Manifest, record: &Record) -> Result<()> {
    let hash = librqbit::torrent_from_bytes(&record.metadata)
        .map_err(|_| DownloadError::InvalidMedia)?
        .info_hash
        .as_string();
    if hash != manifest.info_hash
        || manifest.chunk_bytes != magnets::CHUNK_BYTES
        || manifest.files.len() != record.selected.len()
    {
        return Err(DownloadError::InvalidMedia);
    }
    let mut seen = std::collections::HashSet::new();
    for file in &manifest.files {
        if !seen.insert(file.file.id)
            || !record.selected.contains(&file.file)
            || !digest(&file.sha256)
            || file.chunks.len() as u64 != file.file.size.div_ceil(magnets::CHUNK_BYTES)
            || file.chunks.iter().any(|v| !digest(v))
        {
            return Err(DownloadError::InvalidMedia);
        }
    }
    Ok(())
}
async fn open_cache(
    data: &std::path::Path,
    file: &assemble_protocol::downloads::DownloadFile,
) -> Result<tokio::fs::File> {
    let path = data.join(files::relative_path(&file.name)?);
    let parent = path.parent().ok_or(DownloadError::Storage)?;
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|_| DownloadError::Storage)?;
    files::reject_links(data).await?;
    let output = tokio::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .await
        .map_err(|_| DownloadError::Storage)?;
    output
        .set_len(file.size)
        .await
        .map_err(|_| DownloadError::Storage)?;
    Ok(output)
}
async fn pull(
    manager: &MagnetManager,
    client: &Client,
    id: &str,
    task: &str,
    data: &std::path::Path,
    manifest: &magnets::Manifest,
    cancel: &CancellationToken,
) -> Result<()> {
    files::reject_links(data).await?;
    let mut received = 0u64;
    let mut last = Instant::now();
    let mut last_bytes = 0;
    let mut transferred = 0u64;
    manager.update(id, DownloadPhase::Receiving, 0, 0);
    for file in &manifest.files {
        let mut output = open_cache(data, &file.file).await?;
        let mut whole = Sha256::new();
        for (index, hash) in file.chunks.iter().enumerate() {
            if cancel.is_cancelled() {
                return Err(DownloadError::Cancelled);
            }
            let offset = index as u64 * magnets::CHUNK_BYTES;
            let length = usize::try_from((file.file.size - offset).min(magnets::CHUNK_BYTES))
                .map_err(|_| DownloadError::Limit)?;
            let mut bytes = vec![0; length];
            output
                .seek(std::io::SeekFrom::Start(offset))
                .await
                .map_err(|_| DownloadError::Storage)?;
            let valid = output.read_exact(&mut bytes).await.is_ok()
                && assemble_protocol::primitives::to_hex(&Sha256::digest(&bytes)) == *hash;
            if !valid {
                bytes = client
                    .magnet_chunk(
                        task,
                        file.file.id,
                        index as u64,
                        &RequestOptions {
                            cancel,
                            timeout: Some(Duration::from_secs(60)),
                        },
                    )
                    .await
                    .map_err(|error| failure(&error))?;
                if bytes.len() != length
                    || assemble_protocol::primitives::to_hex(&Sha256::digest(&bytes)) != *hash
                {
                    return Err(DownloadError::InvalidMedia);
                }
                transferred += bytes.len() as u64;
                output
                    .seek(std::io::SeekFrom::Start(offset))
                    .await
                    .map_err(|_| DownloadError::Storage)?;
                output
                    .write_all(&bytes)
                    .await
                    .map_err(|e| crate::space::write_failure(&e))?;
                output
                    .sync_data()
                    .await
                    .map_err(|_| DownloadError::Storage)?;
            }
            whole.update(&bytes);
            received += bytes.len() as u64;
            let speed = u64::try_from(
                u128::from(transferred.saturating_sub(last_bytes)) * 1000
                    / last.elapsed().as_millis().max(1),
            )
            .unwrap_or(u64::MAX);
            last = Instant::now();
            last_bytes = transferred;
            manager.update(id, DownloadPhase::Receiving, received, speed);
            manager.persist(&manager.lock())?;
        }
        output
            .sync_all()
            .await
            .map_err(|_| DownloadError::Storage)?;
        if assemble_protocol::primitives::to_hex(&whole.finalize()) != file.sha256 {
            return Err(DownloadError::InvalidMedia);
        }
    }
    if received
        != manifest
            .files
            .iter()
            .map(|file| file.file.size)
            .sum::<u64>()
    {
        return Err(DownloadError::InvalidMedia);
    }
    Ok(())
}
pub(super) async fn cleanup(
    manager: &MagnetManager,
    client: &Client,
    id: &str,
    record: &Record,
) -> Result<()> {
    let Some(remote) = &record.remote else {
        return Ok(());
    };
    let Some(task) = &remote.task_id else {
        return Ok(());
    };
    if remote.released {
        return Ok(());
    }
    let cancel = CancellationToken::new();
    if account(client, &cancel).await? != remote.account_id {
        return Err(DownloadError::AccountMismatch);
    }
    let action = if record.view.phase == DownloadPhase::Cancelled {
        Action::Cancel
    } else {
        Action::Release
    };
    match client
        .magnet_control(
            task,
            &Control { action },
            &RequestOptions {
                cancel: &cancel,
                timeout: Some(Duration::from_secs(6)),
            },
        )
        .await
    {
        Ok(reply) => {
            if reply.id != *task
                || reply.account_id != remote.account_id
                || (!reply.released && reply.phase != DownloadPhase::Cancelled)
            {
                return Err(DownloadError::InvalidMedia);
            }
        }
        Err(ClientError::Http { status: 404, .. }) => {}
        Err(error) => return Err(failure(&error)),
    }
    let mut remote = remote.clone();
    remote.released = true;
    save(manager, id, remote)
}
