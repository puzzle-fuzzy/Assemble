//! 从本机读取文件：确认身份并流式计算摘要。
//!
//! 上传和"文件还在不在本机"的判断都依赖这里。整个文件分块读取，
//! 读之前与读之后各看一次大小和修改时间，中途被改写的文件不会被当成有效来源。

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

use crate::hash::Sha256Sink;
use crate::{CHUNK_BYTES, TransferError};

/// 一个本机文件的身份：位置、大小、修改时间与内容摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalFile {
    pub path: PathBuf,
    pub size: u64,
    pub modified_at: Option<SystemTime>,
    pub sha256: String,
}

/// 读取一个普通文件的大小、修改时间与摘要。
pub async fn inspect(path: &Path, cancel: &CancellationToken) -> Result<LocalFile, TransferError> {
    let before = tokio::fs::metadata(path).await?;
    if !before.is_file() {
        return Err(TransferError::NotRegularFile);
    }
    let sha256 = hash_with(path, cancel).await?;
    let after = tokio::fs::metadata(path).await?;
    ensure_unchanged(&before, &after)?;
    Ok(LocalFile {
        path: path.to_owned(),
        size: after.len(),
        modified_at: after.modified().ok(),
        sha256,
    })
}

// 读之前与读之后的身份必须一致，否则算出来的摘要不代表磁盘上的内容。
fn ensure_unchanged(
    before: &std::fs::Metadata,
    after: &std::fs::Metadata,
) -> Result<(), TransferError> {
    if !after.is_file()
        || before.len() != after.len()
        || before.modified().ok() != after.modified().ok()
    {
        return Err(TransferError::SourceChanged);
    }
    Ok(())
}

/// 流式计算一个文件的 SHA-256 小写十六进制。
pub async fn hash_file(path: &Path, cancel: &CancellationToken) -> Result<String, TransferError> {
    hash_with(path, cancel).await
}

async fn hash_with(path: &Path, cancel: &CancellationToken) -> Result<String, TransferError> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut sink = Sha256Sink::new();
    let mut buffer = vec![0u8; CHUNK_BYTES];
    loop {
        let read = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(TransferError::Cancelled),
            read = file.read(&mut buffer) => read,
        }?;
        if read == 0 {
            break;
        }
        sink.update(&buffer[..read]);
    }
    Ok(sink.finish())
}

#[cfg(test)]
mod tests {
    use tokio::io::AsyncWriteExt;

    use super::*;

    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    async fn write(path: &Path, bytes: &[u8]) {
        let mut file = tokio::fs::File::create(path).await.unwrap();
        file.write_all(bytes).await.unwrap();
        // tokio 的文件写入先经过缓冲，落盘之后才能读到完整内容。
        file.sync_all().await.unwrap();
    }

    #[tokio::test]
    async fn inspect_reports_size_time_and_digest_of_a_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        write(&path, b"abc").await;

        let file = inspect(&path, &CancellationToken::new()).await.unwrap();
        assert_eq!(file.path, path);
        assert_eq!(file.size, 3);
        assert_eq!(file.sha256, ABC);
        assert!(file.modified_at.is_some());
    }

    #[tokio::test]
    async fn large_files_are_hashed_in_chunks_without_losing_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.bin");
        let payload: Vec<u8> = (0..(CHUNK_BYTES as u32 * 5 + 123))
            .map(|i| i as u8)
            .collect();
        write(&path, &payload).await;

        let file = inspect(&path, &CancellationToken::new()).await.unwrap();
        assert_eq!(file.size, payload.len() as u64);
        assert_eq!(file.sha256, crate::hash::digest(&payload));
    }

    #[tokio::test]
    async fn a_directory_is_not_a_readable_source() {
        let dir = tempfile::tempdir().unwrap();
        let error = inspect(dir.path(), &CancellationToken::new())
            .await
            .unwrap_err();
        assert!(matches!(error, TransferError::NotRegularFile), "{error}");
        assert_eq!(error.code(), "FILE_TOO_LARGE");
    }

    #[tokio::test]
    async fn a_missing_file_is_an_io_error_not_an_empty_one() {
        let dir = tempfile::tempdir().unwrap();
        let error = inspect(&dir.path().join("absent"), &CancellationToken::new())
            .await
            .unwrap_err();
        assert!(matches!(error, TransferError::Io(_)), "{error}");
    }

    #[tokio::test]
    async fn a_file_that_changes_between_two_reads_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("changing.txt");
        write(&path, b"aaa").await;
        let before = tokio::fs::metadata(&path).await.unwrap();
        ensure_unchanged(&before, &before).unwrap();

        // 大小变了。
        write(&path, b"aaaa").await;
        let grown = tokio::fs::metadata(&path).await.unwrap();
        assert!(matches!(
            ensure_unchanged(&before, &grown),
            Err(TransferError::SourceChanged)
        ));

        // 大小不变但修改时间变了。
        write(&path, b"bbb").await;
        let rewritten = tokio::fs::metadata(&path).await.unwrap();
        assert_eq!(rewritten.len(), before.len());
        assert!(matches!(
            ensure_unchanged(&before, &rewritten),
            Err(TransferError::SourceChanged)
        ));

        // 路径被换成了目录。
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        let replaced = tokio::fs::metadata(&path).await.unwrap();
        assert!(matches!(
            ensure_unchanged(&before, &replaced),
            Err(TransferError::SourceChanged)
        ));
        assert_eq!(TransferError::SourceChanged.code(), "SOURCE_CHANGED");
    }

    #[tokio::test]
    async fn hashing_stops_when_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        write(&path, b"abc").await;

        let cancel = CancellationToken::new();
        cancel.cancel();
        let error = hash_file(&path, &cancel).await.unwrap_err();
        assert!(matches!(error, TransferError::Cancelled), "{error}");
    }
}
