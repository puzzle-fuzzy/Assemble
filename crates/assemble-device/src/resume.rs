//! 私有恢复检查点只引用受控目录内的有界分片；从不接受界面传来的路径。
use crate::{Received, choose_file_name, safe_file_name, validate_directory};
use assemble_transfer::{
    TransferError,
    hash::{Sha256Sink, digest},
    staged::StagedFile,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const CHUNK_BYTES: u64 = 1024 * 1024;
const MAX_BYTES: u64 = 100_000_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Checkpoint {
    pub directory: PathBuf,
    pub staging: String,
    pub name: String,
    pub size: u64,
    pub sha256: String,
    pub parts: Vec<String>,
}

fn invalid() -> TransferError {
    TransferError::HashMismatch
}
#[cfg(windows)]
fn plain(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    // 重解析点还包括目录联接，不能只检查文件符号链接。
    !metadata.file_type().is_symlink() && metadata.file_attributes() & 0x400 == 0
}

#[cfg(not(windows))]
fn plain(metadata: &std::fs::Metadata) -> bool {
    !metadata.file_type().is_symlink()
}

impl Checkpoint {
    pub fn offset(&self) -> u64 {
        (self.parts.len() as u64 * CHUNK_BYTES).min(self.size)
    }
    fn valid(&self) -> bool {
        self.size <= MAX_BYTES
            && self.name == safe_file_name(&self.name)
            && self.sha256.len() == 64
            && self.sha256.bytes().all(|b| b.is_ascii_hexdigit())
            && self
                .staging
                .strip_prefix(".assemble-resume-")
                .is_some_and(|v| v.len() == 32 && v.bytes().all(|b| b.is_ascii_hexdigit()))
            && self.parts.len() as u64 <= self.size.div_ceil(CHUNK_BYTES)
            && self
                .parts
                .iter()
                .all(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
    }
    async fn root(&self) -> Result<PathBuf, TransferError> {
        if !self.valid() {
            return Err(invalid());
        }
        let current = validate_directory(&self.directory)
            .await
            .map_err(|_| invalid())?;
        if current != self.directory {
            return Err(invalid());
        }
        let path = current.join(&self.staging);
        let metadata = tokio::fs::symlink_metadata(&path).await?;
        if !plain(&metadata) || !metadata.is_dir() {
            return Err(invalid());
        }
        Ok(path)
    }
    pub async fn create(
        directory: &Path,
        name: &str,
        size: u64,
        sha256: &str,
    ) -> Result<Self, TransferError> {
        let directory = validate_directory(directory).await.map_err(|_| invalid())?;
        let mut random = [0_u8; 16];
        getrandom::fill(&mut random).map_err(|_| TransferError::RandomUnavailable)?;
        let suffix = random.iter().fold(String::new(), |mut s, b| {
            use std::fmt::Write;
            let _ = write!(s, "{b:02x}");
            s
        });
        let staging = format!(".assemble-resume-{suffix}");
        let checkpoint = Self {
            name: choose_file_name(&directory, &safe_file_name(name)).await?,
            directory,
            staging,
            size,
            sha256: sha256.to_owned(),
            parts: vec![],
        };
        if !checkpoint.valid() {
            return Err(invalid());
        }
        let builder = tokio::fs::DirBuilder::new();
        #[cfg(unix)]
        let builder = {
            let mut builder = builder;
            builder.mode(0o700);
            builder
        };
        builder
            .create(checkpoint.directory.join(&checkpoint.staging))
            .await?;
        Ok(checkpoint)
    }
    async fn part(&self, root: &Path, index: usize) -> Result<Vec<u8>, TransferError> {
        let path = root.join(format!("{index}.chunk"));
        let metadata = tokio::fs::symlink_metadata(&path).await?;
        let expected = (self.size - index as u64 * CHUNK_BYTES).min(CHUNK_BYTES);
        if !plain(&metadata) || !metadata.is_file() || metadata.len() != expected {
            return Err(invalid());
        }
        let mut file = tokio::fs::File::open(path).await?;
        let mut bytes = Vec::new();
        (&mut file)
            .take(CHUNK_BYTES + 1)
            .read_to_end(&mut bytes)
            .await?;
        if bytes.len() as u64 != expected || digest(&bytes) != self.parts[index] {
            return Err(invalid());
        }
        Ok(bytes)
    }
    /// 恢复前重新读回所有已确认片，不能信任日志中的旧摘要。
    pub async fn verify(&self) -> Result<(), TransferError> {
        let root = self.root().await?;
        for index in 0..self.parts.len() {
            self.part(&root, index).await?;
        }
        Ok(())
    }
    /// 片文件先同步，再由调用方把新的检查点持久化；响应丢失只重写未确认的片。
    pub async fn append(&mut self, bytes: &[u8]) -> Result<(), TransferError> {
        let root = self.root().await?;
        let expected = (self.size - self.offset()).min(CHUNK_BYTES);
        if expected == 0 || bytes.len() as u64 != expected {
            return Err(invalid());
        }
        let path = root.join(format!("{}.chunk", self.parts.len()));
        if let Ok(metadata) = tokio::fs::symlink_metadata(&path).await {
            if !plain(&metadata) || !metadata.is_file() {
                return Err(invalid());
            }
            tokio::fs::remove_file(&path).await?;
        }
        let mut options = tokio::fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(path).await?;
        file.write_all(bytes).await?;
        file.sync_all().await?;
        self.parts.push(digest(bytes));
        Ok(())
    }
    /// 只发布完整核验的内容。重启发生在发布后、日志提交前时，同名同内容幂等成功。
    pub async fn publish(&self) -> Result<Received, TransferError> {
        self.publish_with_cancel(&tokio_util::sync::CancellationToken::new())
            .await
    }
    pub async fn publish_with_cancel(
        &self,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<Received, TransferError> {
        if self.offset() != self.size {
            return Err(TransferError::IncompleteDownload);
        }
        let root = self.root().await?;
        let destination = self.directory.join(&self.name);
        let mut staged = StagedFile::create(&destination).await?;
        let mut sink = Sha256Sink::new();
        for index in 0..self.parts.len() {
            if cancel.is_cancelled() {
                return Err(TransferError::Cancelled);
            }
            let bytes = self.part(&root, index).await?;
            sink.update(&bytes);
            staged.write_all(&bytes).await?;
        }
        if sink.finish() != self.sha256 {
            return Err(invalid());
        }
        let staged = staged.seal(self.size, self.sha256.clone()).await?;
        self.root().await?;
        if cancel.is_cancelled() {
            return Err(TransferError::Cancelled);
        }
        staged.publish_strict().await?;
        Ok(Received {
            path: destination,
            name: self.name.clone(),
            size: self.size,
        })
    }
    /// 日志中的取消与完成只有在受控暂存清理确认后才能移除。
    pub async fn cleanup(&self) -> Result<(), TransferError> {
        let root = match self.root().await {
            Err(TransferError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            result => result?,
        };
        let mut entries = tokio::fs::read_dir(&root).await?;
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name();
            let Some(index) = name
                .to_str()
                .and_then(|s| s.strip_suffix(".chunk"))
                .and_then(|s| s.parse::<usize>().ok())
            else {
                return Err(invalid());
            };
            let metadata = tokio::fs::symlink_metadata(entry.path()).await?;
            if index > 100 || !plain(&metadata) || !metadata.is_file() {
                return Err(invalid());
            }
            tokio::fs::remove_file(entry.path()).await?;
        }
        tokio::fs::remove_dir(root).await?;
        Ok(())
    }
}

/// 启动时只回收本命名空间中超过一天且未被任何账户日志引用的目录。
pub async fn sweep(
    directory: &Path,
    protected: &[(PathBuf, String)],
) -> Result<u32, TransferError> {
    let directory = validate_directory(directory).await.map_err(|_| invalid())?;
    let mut entries = tokio::fs::read_dir(&directory).await?;
    let mut removed = 0;
    while let Some(entry) = entries.next_entry().await? {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if protected
            .iter()
            .any(|(root, staging)| root == &directory && staging == &name)
        {
            continue;
        }
        let meta = tokio::fs::symlink_metadata(entry.path()).await?;
        if !plain(&meta)
            || !meta.is_dir()
            || meta
                .modified()
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_none_or(|age| age < std::time::Duration::from_hours(24))
        {
            continue;
        }
        let candidate = Checkpoint {
            directory: directory.clone(),
            staging: name,
            name: "orphan".into(),
            size: 0,
            sha256: digest(b""),
            parts: vec![],
        };
        if candidate.valid() && candidate.cleanup().await.is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn empty_files_cancellation_and_bad_whole_digest_never_publish_unverified_content() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let empty = Checkpoint::create(&root, "empty", 0, &digest(b""))
            .await
            .unwrap();
        let cancel = tokio_util::sync::CancellationToken::new();
        cancel.cancel();
        assert!(matches!(
            empty.publish_with_cancel(&cancel).await,
            Err(TransferError::Cancelled)
        ));
        assert!(!root.join("empty").exists());
        assert_eq!(empty.publish().await.unwrap().size, 0);
        empty.cleanup().await.unwrap();
        let mut bad = Checkpoint::create(&root, "bad", 3, &digest(b"xyz"))
            .await
            .unwrap();
        bad.append(b"abc").await.unwrap();
        assert!(matches!(
            bad.publish().await,
            Err(TransferError::HashMismatch)
        ));
        assert!(!root.join("bad").exists());
        bad.cleanup().await.unwrap();
        assert!(
            Checkpoint::create(&root, "too-big", MAX_BYTES + 1, &digest(b""))
                .await
                .is_err()
        );
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn symlinked_parts_and_changed_receive_directory_are_never_followed() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let chosen = root.join("chosen");
        std::fs::create_dir(&chosen).unwrap();
        let mut cp = Checkpoint::create(&chosen, "received", 3, &digest(b"abc"))
            .await
            .unwrap();
        cp.append(b"abc").await.unwrap();
        let outside = root.join("outside");
        std::fs::write(&outside, b"abc").unwrap();
        let part = chosen.join(&cp.staging).join("0.chunk");
        std::fs::remove_file(&part).unwrap();
        std::os::unix::fs::symlink(&outside, &part).unwrap();
        assert!(cp.verify().await.is_err());
        assert!(cp.publish().await.is_err());
        assert!(cp.cleanup().await.is_err());
        assert_eq!(std::fs::read(&outside).unwrap(), b"abc");
        std::fs::rename(&chosen, root.join("moved")).unwrap();
        std::fs::create_dir(&chosen).unwrap();
        assert!(cp.verify().await.is_err());
        assert!(cp.publish().await.is_err());
        assert!(!chosen.join("received").exists());
    }
    #[tokio::test]
    async fn resumes_verified_parts_and_publishes_idempotently_without_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let bytes = vec![42; CHUNK_BYTES as usize + 3];
        let mut checkpoint =
            Checkpoint::create(&root, "文件.bin", bytes.len() as u64, &digest(&bytes))
                .await
                .unwrap();
        checkpoint
            .append(&bytes[..CHUNK_BYTES as usize])
            .await
            .unwrap();
        let saved = serde_json::to_vec(&checkpoint).unwrap();
        let mut restored: Checkpoint = serde_json::from_slice(&saved).unwrap();
        restored.verify().await.unwrap();
        assert_eq!(restored.offset(), CHUNK_BYTES);
        restored
            .append(&bytes[CHUNK_BYTES as usize..])
            .await
            .unwrap();
        let received = restored.publish().await.unwrap();
        assert_eq!(tokio::fs::read(&received.path).await.unwrap(), bytes);
        restored.publish().await.unwrap();
        tokio::fs::write(&received.path, b"other").await.unwrap();
        assert!(matches!(
            restored.publish().await,
            Err(TransferError::FileExists)
        ));
        restored.cleanup().await.unwrap();
        assert_eq!(tokio::fs::read(&received.path).await.unwrap(), b"other");
    }
    #[tokio::test]
    async fn damaged_checkpoint_and_directory_replacement_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let mut cp = Checkpoint::create(&root, "a", 3, &digest(b"abc"))
            .await
            .unwrap();
        cp.append(b"abc").await.unwrap();
        tokio::fs::write(root.join(&cp.staging).join("0.chunk"), b"xyz")
            .await
            .unwrap();
        assert!(cp.verify().await.is_err());
        assert!(cp.publish().await.is_err());
        cp.staging = "../other".into();
        assert!(cp.cleanup().await.is_err());
        assert!(!root.join("a").exists());
    }
}
