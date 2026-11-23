//! 同目录暂存与原子发布。
//!
//! 暂存文件 `<目标>.<随机>.part` 与目标同目录、独占创建、仅本人可读写。
//! 发布用硬链接：目标在这一步要么不存在、要么内容完全一致，否则失败；
//! 目标文件在任何一个瞬间都只会是"完整的旧内容"或"完整的新内容"。
//!
//! 暂存文件以 `.part` 结尾，不会与任何最终文件重名；传输中断留下的暂存文件
//! 由各自的清理逻辑回收（服务端的定时清理、桌面端启动时的清扫）。

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use tokio::io::AsyncWriteExt;

use crate::TransferError;

/// 一次发布的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishOutcome {
    /// 目标由本次传输建立。
    Published,
    /// 目标早已存在，且与本次内容逐字节一致；什么都没改。
    AlreadyPresent,
}

fn staging_path(destination: &Path) -> Result<PathBuf, TransferError> {
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|_| TransferError::RandomUnavailable)?;
    let suffix = assemble_protocol::primitives::to_hex(&random);
    let mut name = destination
        .file_name()
        .map(std::ffi::OsStr::to_os_string)
        .unwrap_or_default();
    name.push(format!(".{suffix}.part"));
    Ok(destination.with_file_name(name))
}

/// 已写入、已落盘、等待发布的暂存文件。没被发布就会被清除。
#[derive(Debug)]
pub struct SealedStaging {
    destination: PathBuf,
    path: PathBuf,
    size: u64,
    sha256: String,
    cleanup: bool,
}

impl SealedStaging {
    /// 最终要出现的目标位置。
    pub fn destination(&self) -> &Path {
        &self.destination
    }

    /// 暂存内容的大小。
    pub fn size(&self) -> u64 {
        self.size
    }

    /// 暂存内容的 SHA-256 小写十六进制。
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    /// 发布到目标位置。目标已存在时，只有它是一个逐字节相同的普通文件才算成功。
    pub async fn publish(mut self) -> Result<PublishOutcome, TransferError> {
        let outcome = match fs::hard_link(&self.path, &self.destination) {
            Ok(()) => Ok(PublishOutcome::Published),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                if existing_file_matches(&self.destination, self.size, &self.sha256).await? {
                    Ok(PublishOutcome::AlreadyPresent)
                } else {
                    Err(TransferError::FileExists)
                }
            }
            // 个别文件系统不支持硬链接；那里只能退回到"先查后改名"。
            Err(error) if is_unsupported_link(&error) => self.rename_into_place().await,
            Err(error) => Err(error.into()),
        };
        if outcome.is_ok() {
            // 硬链接之后暂存名仍指向同一份内容，必须撤掉；改名路径上它已不存在。
            match fs::remove_file(&self.path) {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => tracing::warn!(
                    kind = ?error.kind(),
                    "staging name left behind after publish"
                ),
            }
            self.cleanup = false;
        }
        outcome
    }

    async fn rename_into_place(&self) -> Result<PublishOutcome, TransferError> {
        match fs::symlink_metadata(&self.destination) {
            Ok(_) => {
                if existing_file_matches(&self.destination, self.size, &self.sha256).await? {
                    Ok(PublishOutcome::AlreadyPresent)
                } else {
                    Err(TransferError::FileExists)
                }
            }
            // 检查与改名之间存在一个很短的窗口。它只出现在不支持硬链接的
            // 文件系统上（如 exFAT、部分网络盘），且目标目录本来就是本机独占使用。
            Err(error) if error.kind() == ErrorKind::NotFound => {
                fs::rename(&self.path, &self.destination)?;
                Ok(PublishOutcome::Published)
            }
            Err(error) => Err(error.into()),
        }
    }
}

impl Drop for SealedStaging {
    fn drop(&mut self) {
        if self.cleanup {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// 正在写入的暂存文件。
#[derive(Debug)]
pub struct StagedFile {
    destination: PathBuf,
    path: PathBuf,
    file: Option<tokio::fs::File>,
    cleanup: bool,
}

impl StagedFile {
    /// 在目标的同一目录独占创建暂存文件。目标所在的目录必须已经存在。
    pub async fn create(destination: &Path) -> Result<Self, TransferError> {
        let path = staging_path(destination)?;
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let file = options.open(&path).await?;
        Ok(Self {
            destination: destination.to_owned(),
            path,
            file: Some(file),
            cleanup: true,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub async fn write_all(&mut self, bytes: &[u8]) -> Result<(), TransferError> {
        match self.file.as_mut() {
            Some(file) => file.write_all(bytes).await.map_err(Into::into),
            None => Err(TransferError::Io(ErrorKind::BrokenPipe.into())),
        }
    }

    /// 把内容落盘，连同它的大小与摘要一起交出一个可发布的暂存文件。
    ///
    /// 这两个值由写入方在写入时统计，发布时据此判断"已存在的同名文件
    /// 是不是同一份内容"。
    pub async fn seal(mut self, size: u64, sha256: String) -> Result<SealedStaging, TransferError> {
        let synced = match self.file.as_mut() {
            Some(file) => file.sync_all().await,
            None => Ok(()),
        };
        match synced {
            Ok(()) => {
                drop(self.file.take());
                self.cleanup = false;
                Ok(SealedStaging {
                    destination: self.destination.clone(),
                    path: self.path.clone(),
                    size,
                    sha256,
                    cleanup: true,
                })
            }
            Err(error) => Err(error.into()),
        }
    }
}

impl Drop for StagedFile {
    fn drop(&mut self) {
        if self.cleanup {
            // 先关句柄再删除，Windows 上才删得掉。
            drop(self.file.take());
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// 目标位置是不是一个与给定大小、摘要完全一致的普通文件。
async fn existing_file_matches(
    path: &Path,
    size: u64,
    sha256: &str,
) -> Result<bool, TransferError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.len() != size {
        return Ok(false);
    }
    // 核对已有文件内容时没有可取消的外层操作，用一个永不触发的令牌。
    let never = tokio_util::sync::CancellationToken::new();
    Ok(crate::source::hash_file(path, &never).await? == sha256)
}

/// 文件系统不支持硬链接时 `link` 给出的几种错误。
fn is_unsupported_link(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::PermissionDenied | ErrorKind::Unsupported | ErrorKind::CrossesDevices
    )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;

        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// 写入一段内容并落盘，得到可发布的暂存文件。
    async fn sealed(destination: &Path, bytes: &[u8]) -> SealedStaging {
        let mut staged = StagedFile::create(destination).await.unwrap();
        staged.write_all(bytes).await.unwrap();
        staged
            .seal(bytes.len() as u64, crate::hash::digest(bytes))
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_staging_file_sits_beside_its_target_and_is_private() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("共享/报告.pdf");
        fs::create_dir(destination.parent().unwrap()).unwrap();

        let staged = StagedFile::create(&destination).await.unwrap();
        let path = staged.path().to_path_buf();
        assert_eq!(path.parent(), destination.parent());
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("报告.pdf."), "{name}");
        assert!(name.ends_with(".part"), "{name}");
        #[cfg(unix)]
        assert_eq!(mode(&path), 0o600);

        // 两次创建得到不同的暂存文件，彼此不会覆盖。
        let other = StagedFile::create(&destination).await.unwrap();
        assert_ne!(other.path(), staged.path());
    }

    #[tokio::test]
    async fn an_unpublished_staging_file_is_cleaned_up_on_every_failure_path() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("a.txt");

        // 仍在写入时被丢弃。
        {
            let staged = StagedFile::create(&destination).await.unwrap();
            assert!(staged.path().exists());
        }
        assert_eq!(entries(dir.path()), Vec::<String>::new());

        // 已落盘但未发布时被丢弃。
        {
            let _sealed = sealed(&destination, b"abc").await;
        }
        assert_eq!(entries(dir.path()), Vec::<String>::new());

        // 发布失败时同样清除。
        fs::write(&destination, b"something else").unwrap();
        let error = sealed(&destination, b"abc")
            .await
            .publish()
            .await
            .unwrap_err();
        assert!(matches!(error, TransferError::FileExists), "{error}");
        assert_eq!(entries(dir.path()), vec!["a.txt"]);
        assert_eq!(fs::read(&destination).unwrap(), b"something else");
    }

    #[tokio::test]
    async fn publish_uses_a_link_and_leaves_only_the_target_behind() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("a.txt");

        let outcome = sealed(&destination, b"hello")
            .await
            .publish()
            .await
            .unwrap();
        assert_eq!(outcome, PublishOutcome::Published);
        assert_eq!(fs::read(&destination).unwrap(), b"hello");
        assert_eq!(entries(dir.path()), vec!["a.txt"]);
        #[cfg(unix)]
        assert_eq!(mode(&destination), 0o600);
    }

    #[tokio::test]
    async fn an_identical_existing_file_counts_as_already_present() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("a.txt");
        fs::write(&destination, b"hello").unwrap();

        let outcome = sealed(&destination, b"hello")
            .await
            .publish()
            .await
            .unwrap();
        assert_eq!(outcome, PublishOutcome::AlreadyPresent);
        assert_eq!(fs::read(&destination).unwrap(), b"hello");
        assert_eq!(entries(dir.path()), vec!["a.txt"]);
    }

    #[tokio::test]
    async fn a_different_existing_file_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("a.txt");
        fs::write(&destination, b"world").unwrap();

        let error = sealed(&destination, b"hello")
            .await
            .publish()
            .await
            .unwrap_err();
        assert!(matches!(error, TransferError::FileExists), "{error}");
        assert_eq!(fs::read(&destination).unwrap(), b"world");

        // 大小相同但内容不同，同样拒绝。
        fs::write(&destination, b"hella").unwrap();
        let error = sealed(&destination, b"hello")
            .await
            .publish()
            .await
            .unwrap_err();
        assert!(matches!(error, TransferError::FileExists), "{error}");
        assert_eq!(fs::read(&destination).unwrap(), b"hella");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_directory_in_the_way_is_not_treated_as_a_match() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("a.txt");
        fs::create_dir(&destination).unwrap();

        let error = sealed(&destination, b"hello")
            .await
            .publish()
            .await
            .unwrap_err();
        assert!(matches!(error, TransferError::FileExists), "{error}");
        assert!(destination.is_dir());
    }

    #[tokio::test]
    async fn publishing_requires_an_existing_parent_directory() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("missing/a.txt");
        let error = StagedFile::create(&destination).await.unwrap_err();
        assert!(
            matches!(error, TransferError::Io(ref cause) if cause.kind() == ErrorKind::NotFound),
            "{error}"
        );
        assert_eq!(entries(dir.path()), Vec::<String>::new());
    }

    #[test]
    fn only_filesystem_limitations_trigger_the_rename_fallback() {
        use std::io::Error;

        assert!(is_unsupported_link(&Error::new(
            ErrorKind::Unsupported,
            "exfat"
        )));
        assert!(is_unsupported_link(&Error::new(
            ErrorKind::CrossesDevices,
            "xdev"
        )));
        assert!(is_unsupported_link(&Error::new(
            ErrorKind::PermissionDenied,
            "eperm"
        )));
        assert!(!is_unsupported_link(&Error::new(
            ErrorKind::NotFound,
            "gone"
        )));
        assert!(!is_unsupported_link(&Error::new(
            ErrorKind::AlreadyExists,
            "exists"
        )));
        assert!(!is_unsupported_link(&Error::other("no kind")));
    }
}
