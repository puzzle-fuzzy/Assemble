//! 把一个字节流落到目标位置：全部校验通过之后才发布。
//!
//! ```text
//! 调用方校验目标路径 ──▶ stage ──▶ 调用方再校验一次 ──▶ publish
//!                        │                              │
//!                   写 .part 并计算摘要            硬链接到目标，不覆盖
//! ```
//!
//! 中间那次"再校验"留给调用方：传输可能持续很久，发布前必须重新确认
//! 目标仍在授权目录内。

use std::path::{Path, PathBuf};

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio_util::sync::CancellationToken;

use crate::hash::{Sha256Sink, digests_match};
use crate::staged::{PublishOutcome, SealedStaging, StagedFile};
use crate::{CHUNK_BYTES, TransferError};

/// 传输进度。`progress` 是 0 到 100 的百分比，完成前最高到 99。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransferProgress {
    pub received: u64,
    pub total: Option<u64>,
    pub progress: f64,
}

impl TransferProgress {
    /// 完成时的进度：只在内容真正发布之后才上报。
    pub fn complete(received: u64, total: Option<u64>) -> Self {
        Self {
            received,
            total,
            progress: 100.0,
        }
    }
}

pub type ProgressCallback<'a> = &'a mut dyn FnMut(TransferProgress);

/// 一次暂存的参数。带有进度回调，所以按值传入、只使用一次。
pub struct SaveOptions<'a> {
    /// 内容必须匹配的摘要；不匹配就不发布。
    pub expected_sha256: Option<&'a str>,
    /// 响应声明的总长度；与之不符就不发布。
    pub content_length: Option<u64>,
    /// 允许写入的最大字节数；超过即中止。
    pub max_bytes: Option<u64>,
    pub cancel: &'a CancellationToken,
    pub progress: Option<ProgressCallback<'a>>,
}

impl<'a> SaveOptions<'a> {
    pub fn new(cancel: &'a CancellationToken) -> Self {
        Self {
            expected_sha256: None,
            content_length: None,
            max_bytes: None,
            cancel,
            progress: None,
        }
    }
}

/// 已校验、已落盘、尚未发布的下载。
#[derive(Debug)]
pub struct StagedDownload {
    sealed: SealedStaging,
    received: u64,
    total: Option<u64>,
}

impl StagedDownload {
    /// 发布之后内容所在的位置。
    pub fn destination(&self) -> &Path {
        self.sealed.destination()
    }

    pub fn size(&self) -> u64 {
        self.sealed.size()
    }

    pub fn sha256(&self) -> &str {
        self.sealed.sha256()
    }

    /// 校验通过后对外的完整进度事件。
    pub fn completion(&self) -> TransferProgress {
        TransferProgress::complete(self.received, self.total)
    }

    pub async fn publish(self) -> Result<SavedFile, TransferError> {
        // 发布会消费暂存文件，先把结果需要的值取出来。
        let path = self.destination().to_owned();
        let size = self.received;
        let sha256 = self.sha256().to_owned();
        let outcome = self.sealed.publish().await?;
        Ok(SavedFile {
            path,
            size,
            sha256,
            outcome,
        })
    }
}

/// 落到本机的一个文件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedFile {
    pub path: PathBuf,
    pub size: u64,
    pub sha256: String,
    pub outcome: PublishOutcome,
}

/// 把来源的全部内容写入目标旁的暂存文件，边写边算摘要。
///
/// 这一步不做任何授权判断，也不会让目标文件出现。
pub async fn stage<R>(
    destination: &Path,
    source: &mut R,
    options: SaveOptions<'_>,
) -> Result<StagedDownload, TransferError>
where
    R: AsyncRead + Unpin,
{
    let SaveOptions {
        expected_sha256,
        content_length,
        max_bytes,
        cancel,
        mut progress,
    } = options;

    check_cancelled(cancel)?;
    let mut sink = Sha256Sink::new();
    let mut staged = StagedFile::create(destination).await?;
    let mut buffer = vec![0u8; CHUNK_BYTES];
    let mut received: u64 = 0;

    loop {
        let read = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(TransferError::Cancelled),
            read = source.read(&mut buffer) => read,
        }?;
        if read == 0 {
            break;
        }
        let chunk = &buffer[..read];
        received = received.saturating_add(read as u64);
        if max_bytes.is_some_and(|limit| received > limit) {
            return Err(TransferError::TooLarge);
        }
        sink.update(chunk);
        staged.write_all(chunk).await?;
        if let Some(report) = progress.as_deref_mut() {
            report(TransferProgress {
                received,
                total: content_length,
                progress: percent(received, content_length),
            });
        }
    }

    if content_length.is_some_and(|total| total != received) {
        return Err(TransferError::IncompleteDownload);
    }
    let sha256 = sink.finish();
    if expected_sha256.is_some_and(|expected| !digests_match(expected, &sha256)) {
        return Err(TransferError::HashMismatch);
    }

    let sealed = staged.seal(received, sha256).await?;
    Ok(StagedDownload {
        sealed,
        received,
        total: content_length,
    })
}

/// 已完成百分比；总量未知时为 0，完成前封顶 99。
fn percent(received: u64, total: Option<u64>) -> f64 {
    match total {
        Some(total) if total > 0 => f64::from((received.saturating_mul(100) / total).min(99) as u8),
        _ => 0.0,
    }
}

fn check_cancelled(cancel: &CancellationToken) -> Result<(), TransferError> {
    if cancel.is_cancelled() {
        Err(TransferError::Cancelled)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn content(length: usize) -> Vec<u8> {
        (0..length).map(|index| (index % 251) as u8).collect()
    }

    #[tokio::test]
    async fn a_staged_download_is_verified_before_it_can_be_published() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("a.bin");
        let payload = content(CHUNK_BYTES * 2 + 500);
        let expected = crate::hash::digest(&payload);
        let cancel = CancellationToken::new();

        let mut source = Cursor::new(payload.clone());
        let staged = stage(
            &destination,
            &mut source,
            SaveOptions {
                expected_sha256: Some(&expected),
                content_length: Some(payload.len() as u64),
                ..SaveOptions::new(&cancel)
            },
        )
        .await
        .unwrap();

        // 发布之前目标不存在，摘要已经算好。
        assert!(!destination.exists());
        assert_eq!(staged.size(), payload.len() as u64);
        assert_eq!(staged.sha256(), expected);
        assert_eq!(staged.completion().progress, 100.0);

        let saved = staged.publish().await.unwrap();
        assert_eq!(saved.path, destination);
        assert_eq!(saved.outcome, PublishOutcome::Published);
        assert_eq!(std::fs::read(&destination).unwrap(), payload);
        assert_eq!(entries(dir.path()), vec!["a.bin"]);
    }

    #[tokio::test]
    async fn a_hash_mismatch_never_reaches_the_target() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("a.bin");
        let cancel = CancellationToken::new();
        let wrong = "0".repeat(64);

        let mut source = Cursor::new(content(1_000));
        let error = stage(
            &destination,
            &mut source,
            SaveOptions {
                expected_sha256: Some(&wrong),
                ..SaveOptions::new(&cancel)
            },
        )
        .await
        .unwrap_err();

        assert!(matches!(error, TransferError::HashMismatch), "{error}");
        assert_eq!(error.code(), "HASH_MISMATCH");
        assert!(!destination.exists());
        assert_eq!(entries(dir.path()), Vec::<String>::new());
    }

    #[tokio::test]
    async fn the_size_must_match_the_declared_total_in_both_directions() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();

        for (body, declared) in [(500usize, 900u64), (900, 500)] {
            let destination = dir.path().join(format!("{}.bin", declared));
            let mut source = Cursor::new(content(body));
            let error = stage(
                &destination,
                &mut source,
                SaveOptions {
                    content_length: Some(declared),
                    ..SaveOptions::new(&cancel)
                },
            )
            .await
            .unwrap_err();
            assert!(
                matches!(error, TransferError::IncompleteDownload),
                "{error}"
            );
            assert!(!destination.exists());
        }
        assert_eq!(entries(dir.path()), Vec::<String>::new());
    }

    #[tokio::test]
    async fn an_undeclared_total_is_not_checked() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("a.bin");
        let cancel = CancellationToken::new();

        let mut source = Cursor::new(content(10));
        let saved = stage(&destination, &mut source, SaveOptions::new(&cancel))
            .await
            .unwrap()
            .publish()
            .await
            .unwrap();
        assert_eq!(saved.size, 10);
    }

    #[tokio::test]
    async fn exceeding_the_size_limit_stops_the_transfer() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("a.bin");
        let cancel = CancellationToken::new();

        let mut source = Cursor::new(content(CHUNK_BYTES * 3));
        let error = stage(
            &destination,
            &mut source,
            SaveOptions {
                max_bytes: Some(CHUNK_BYTES as u64),
                ..SaveOptions::new(&cancel)
            },
        )
        .await
        .unwrap_err();

        assert!(matches!(error, TransferError::TooLarge), "{error}");
        assert_eq!(error.code(), "FILE_TOO_LARGE");
        assert!(!destination.exists());
        assert_eq!(entries(dir.path()), Vec::<String>::new());
    }

    #[tokio::test]
    async fn progress_is_monotonic_and_capped_below_completion() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("a.bin");
        let cancel = CancellationToken::new();
        let total = CHUNK_BYTES as u64 * 4;
        let mut seen: Vec<f64> = Vec::new();
        let mut received_at_each_step = Vec::new();
        let mut source = Cursor::new(content(total as usize));

        let staged = {
            let mut report = |progress: TransferProgress| {
                seen.push(progress.progress);
                received_at_each_step.push(progress.received);
            };
            stage(
                &destination,
                &mut source,
                SaveOptions {
                    content_length: Some(total),
                    progress: Some(&mut report),
                    ..SaveOptions::new(&cancel)
                },
            )
            .await
            .unwrap()
        };

        assert_eq!(seen, vec![25.0, 50.0, 75.0, 99.0]);
        assert_eq!(
            received_at_each_step,
            (1..=4)
                .map(|step| step * CHUNK_BYTES as u64)
                .collect::<Vec<_>>()
        );
        // 100 只属于发布之后的完成事件。
        assert_eq!(staged.completion().progress, 100.0);
    }

    #[test]
    fn percent_handles_unknown_and_degenerate_totals() {
        assert_eq!(percent(500, None), 0.0);
        assert_eq!(percent(500, Some(0)), 0.0);
        assert_eq!(percent(0, Some(100)), 0.0);
        assert_eq!(percent(99, Some(100)), 99.0);
        assert_eq!(percent(100, Some(100)), 99.0);
        assert_eq!(percent(u64::MAX, Some(1)), 99.0);
    }

    #[tokio::test]
    async fn cancelling_before_or_during_the_transfer_leaves_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();

        // 开始之前就已取消。
        let early = dir.path().join("early.bin");
        let mut source = Cursor::new(content(10));
        let error = stage(&early, &mut source, SaveOptions::new(&cancel))
            .await
            .unwrap_err();
        assert!(matches!(error, TransferError::Cancelled), "{error}");
        assert!(!early.exists());

        // 传输途中取消：在第一次进度回调里触发。
        let mid = dir.path().join("mid.bin");
        let total = CHUNK_BYTES as u64 * 4;
        let mut source = Cursor::new(content(total as usize));
        let cancel = CancellationToken::new();
        let trigger = cancel.clone();
        let error = {
            let mut report = move |progress: TransferProgress| {
                if progress.received >= CHUNK_BYTES as u64 * 2 {
                    trigger.cancel();
                }
            };
            stage(
                &mid,
                &mut source,
                SaveOptions {
                    progress: Some(&mut report),
                    ..SaveOptions::new(&cancel)
                },
            )
            .await
            .unwrap_err()
        };
        assert!(matches!(error, TransferError::Cancelled), "{error}");
        assert!(!mid.exists());
        assert_eq!(entries(dir.path()), Vec::<String>::new());
    }
}
