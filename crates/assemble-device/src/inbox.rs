//! 一次接收：把一段字节流存进接收目录。
//!
//! 收到的内容一律先暂存、核对（长度与摘要），通过后才出现在接收目录里。
//! 这一步的顺序是固定的：
//!
//! ```text
//! 定下接收目录 ─▶ 选一个没被占用的名字 ─▶ 写暂存文件并核对 ─▶ 再确认一次目录 ─▶ 发布
//! ```
//!
//! 传输可能持续很久，所以发布之前要再确认一次接收目录还是原来那个：
//! 下载途中目录被删、被换成链接，都不能让文件落到别的地方去。
//!
//! 一次只处理一个文件：调用方按顺序接收，同名的文件先后到达时自然得到不同序号。
//! 万一下载途中有别的程序抢先建了同名文件，发布会被拒绝（[`TransferError::FileExists`]），
//! 调用方从头再接收一次即可，第二次会选到别的名字。

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};

use assemble_transfer::TransferError;
use assemble_transfer::download::{SaveOptions, StagedDownload, stage};
use assemble_transfer::staged::is_staging_name;
use tokio::io::{AsyncRead, ReadBuf};

use crate::receive::{
    BoundaryError, choose_file_name, ensure_default_directory, safe_file_name, validate_directory,
};

/// 这次接收该用的目录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inbox {
    /// 规范路径（已经解析过链接）。
    pub directory: PathBuf,
    /// 用户设置的目录不能用，这次退回了默认目录。
    pub fell_back: bool,
}

/// 得到这次接收该用的目录：用户设置的那个能用就用它，否则退回"下载/Assemble"。
///
/// 设置的目录常常在移动硬盘或网络盘上，盘没插上时它就不存在了。
/// 这时与其让文件收不下来，不如先放进默认目录——退回这件事会如实告诉用户。
pub async fn resolve(preferred: Option<&Path>, downloads: &Path) -> Result<Inbox, BoundaryError> {
    if let Some(preferred) = preferred
        && let Ok(directory) = validate_directory(preferred).await
    {
        return Ok(Inbox {
            directory,
            fell_back: false,
        });
    }
    Ok(Inbox {
        directory: ensure_default_directory(downloads).await?,
        fell_back: preferred.is_some(),
    })
}

/// 读取来源时的失败：网络断了、对端不再送数据。它被放进 I/O 错误里带出来，
/// 好与本机磁盘的失败分开——前者该过一会儿重试，后者才要报告给发送方。
#[derive(Debug)]
struct SourceFailed(std::io::Error);

impl std::fmt::Display for SourceFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the source stopped yielding data: {}", self.0)
    }
}

impl std::error::Error for SourceFailed {}

/// 一个 [`accept`] 的失败是不是"读不到内容"（网络的事），而不是写不进本机。
pub fn is_source_failure(error: &TransferError) -> bool {
    match error {
        TransferError::Io(cause) => {
            matches!(cause.get_ref(), Some(inner) if inner.is::<SourceFailed>())
        }
        _ => false,
    }
}

/// 给来源的读取错误打上标记的读取器。
struct Tagged<R> {
    inner: R,
}

impl<R: AsyncRead + Unpin> AsyncRead for Tagged<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner)
            .poll_read(cx, buf)
            .map_err(|error| std::io::Error::new(error.kind(), SourceFailed(error)))
    }
}

/// 存好的文件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Received {
    pub path: PathBuf,
    /// 实际落盘的文件名：原名的收敛结果，撞名时带序号。
    pub name: String,
    pub size: u64,
}

/// 把一段字节流作为收到的文件存进接收目录。
///
/// `directory` 必须是 [`resolve`] 或 [`validate_directory`] 给出的规范路径。
/// `options` 里的摘要与长度在这一步就被核对，对不上的内容不会出现在目录里。
///
/// 读不到内容时（连接断了、对端不再送数据）返回的错误满足 [`is_source_failure`]：
/// 那不是这台电脑写不下来，调用方应当过一会儿重试，而不是报告失败。
pub async fn accept<R>(
    directory: &Path,
    raw_name: &str,
    source: &mut R,
    options: SaveOptions<'_>,
) -> Result<Received, TransferError>
where
    R: AsyncRead + Unpin,
{
    let name = choose_file_name(directory, &safe_file_name(raw_name)).await?;
    let destination = confirm_target(directory, &name).await?;
    let mut source = Tagged { inner: source };
    let staged = stage(&destination, &mut source, options).await?;
    // 下载可能持续了很久：发布之前再确认一次，目录还是不是原来那个。
    let destination = confirm_target(directory, &name).await?;
    publish(staged, destination).await
}

/// 目标文件的位置，附带一次接收目录的核对。
async fn confirm_target(directory: &Path, name: &str) -> Result<PathBuf, TransferError> {
    let current = validate_directory(directory)
        .await
        .map_err(|error| TransferError::Io(directory_unusable(error)))?;
    // `directory` 已经是规范路径；规范路径变了，说明它现在指向别的地方了。
    if current != directory {
        return Err(TransferError::Io(std::io::Error::other(
            "the receive directory changed while a file was being received",
        )));
    }
    Ok(current.join(name))
}

fn directory_unusable(error: BoundaryError) -> std::io::Error {
    match error {
        BoundaryError::Io(error) => error,
        BoundaryError::NotDirectory => {
            std::io::Error::other("the receive directory is not a directory")
        }
        BoundaryError::DiskRoot => std::io::Error::other("the receive directory is a whole disk"),
    }
}

async fn publish(staged: StagedDownload, destination: PathBuf) -> Result<Received, TransferError> {
    let name = destination
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let size = staged.size();
    let saved = staged.publish().await?;
    Ok(Received {
        path: saved.path,
        name,
        size,
    })
}

/// 清掉上次运行中断时留下的暂存文件，返回清掉了几个。
///
/// 只删名字正好是暂存文件样式的普通文件，别的一概不碰。目录不存在不算错。
/// 只在启动时、开始接收之前调用：那时不可能有正在写的暂存文件。
pub async fn sweep_staging(directory: &Path) -> std::io::Result<u32> {
    let mut entries = match tokio::fs::read_dir(directory).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    let mut removed = 0u32;
    while let Some(entry) = entries.next_entry().await? {
        // 只看链接本身：指向暂存文件的链接不删它指向的东西，也不该由这里删。
        let is_staging = entry.file_type().await?.is_file()
            && entry.file_name().to_str().is_some_and(is_staging_name);
        if !is_staging {
            continue;
        }
        match tokio::fs::remove_file(entry.path()).await {
            Ok(()) => removed += 1,
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(removed)
}

/// 接收失败的原因，与报告给发送方的一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiveFailure {
    /// 接收目录所在的磁盘写不下了。
    DiskFull,
    /// 写文件时出错：目录不能用、没有权限等。
    WriteFailed,
    /// 收到的内容与声明的长度或摘要对不上。
    VerificationFailed,
    /// 用户在接收途中取消了。
    Cancelled,
}

impl ReceiveFailure {
    /// 接口里的拼法。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DiskFull => "disk_full",
            Self::WriteFailed => "write_failed",
            Self::VerificationFailed => "verification_failed",
            Self::Cancelled => "cancelled",
        }
    }
}

impl From<&TransferError> for ReceiveFailure {
    fn from(error: &TransferError) -> Self {
        match error {
            // 内容本身不对，与接收方的磁盘无关：单独报出来，发送方才知道该重发。
            TransferError::HashMismatch
            | TransferError::IncompleteDownload
            | TransferError::EmptyResponse
            | TransferError::TooLarge => Self::VerificationFailed,
            TransferError::Cancelled => Self::Cancelled,
            TransferError::Io(cause) if cause.kind() == ErrorKind::StorageFull => Self::DiskFull,
            TransferError::Io(_) | TransferError::FileExists | TransferError::RandomUnavailable => {
                Self::WriteFailed
            }
            // 读取本机文件才会遇到的两种失败，接收时不会出现；真出现了也只会是写不下去。
            TransferError::NotRegularFile | TransferError::SourceChanged => Self::WriteFailed,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Cursor;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use assemble_protocol::sends::FailReason;
    use assemble_transfer::hash::digest;
    use tokio::io::ReadBuf;
    use tokio_util::sync::CancellationToken;

    use super::*;

    /// macOS 的临时目录本身经过链接，先取规范路径再用。
    fn directory() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let canonical = fs::canonicalize(dir.path()).unwrap();
        (dir, canonical)
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn options<'a>(
        cancel: &'a CancellationToken,
        expected: &'a str,
        len: usize,
    ) -> SaveOptions<'a> {
        SaveOptions {
            expected_sha256: Some(expected),
            content_length: Some(len as u64),
            max_bytes: Some(100_000_000),
            ..SaveOptions::new(cancel)
        }
    }

    /// 第一次读取时先做一件事，再照常给出内容：用来模拟"下载途中目录变了"。
    struct Tripwire<'a> {
        inner: Cursor<Vec<u8>>,
        on_first_read: Option<Box<dyn FnOnce() + Send + 'a>>,
    }

    impl AsyncRead for Tripwire<'_> {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            if let Some(action) = self.on_first_read.take() {
                action();
            }
            Pin::new(&mut self.inner).poll_read(cx, buf)
        }
    }

    fn tripwire(payload: &[u8], action: impl FnOnce() + Send + 'static) -> Tripwire<'static> {
        Tripwire {
            inner: Cursor::new(payload.to_vec()),
            on_first_read: Some(Box::new(action)),
        }
    }

    #[tokio::test]
    async fn a_configured_directory_is_used_as_long_as_it_is_usable() {
        let (_dir, root) = directory();
        let downloads = root.join("Downloads");
        let chosen = root.join("chosen");
        fs::create_dir(&chosen).unwrap();

        let inbox = resolve(Some(&chosen), &downloads).await.unwrap();
        assert_eq!(
            inbox,
            Inbox {
                directory: fs::canonicalize(&chosen).unwrap(),
                fell_back: false,
            }
        );
        // 能用时不去动默认目录。
        assert!(!downloads.exists());
    }

    #[tokio::test]
    async fn an_unusable_configured_directory_falls_back_to_the_default_one() {
        let (dir, root) = directory();
        let downloads = root.join("Downloads");

        // 没有设置：就是默认目录，不算退回。
        let plain = resolve(None, &downloads).await.unwrap();
        assert!(!plain.fell_back);
        assert_eq!(
            plain.directory,
            fs::canonicalize(downloads.join("Assemble")).unwrap()
        );

        // 设置的目录不在了（盘没插上）：退回默认目录，并如实标出这是一次退回。
        let missing = root.join("missing");
        let fallback = resolve(Some(&missing), &downloads).await.unwrap();
        assert!(fallback.fell_back);
        assert_eq!(fallback.directory, plain.directory);

        // 设置的目录那个位置上现在是一个文件：同样退回。
        let blocked = root.join("blocked");
        fs::write(&blocked, b"x").unwrap();
        let fallback = resolve(Some(&blocked), &downloads).await.unwrap();
        assert!(fallback.fell_back);
        assert_eq!(fallback.directory, plain.directory);
        drop(dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_whole_disk_is_never_used_as_the_receive_directory() {
        let (_dir, root) = directory();
        let downloads = root.join("Downloads");
        let fallback = resolve(Some(Path::new("/")), &downloads).await.unwrap();
        assert!(fallback.fell_back);
        assert_eq!(
            fallback.directory,
            fs::canonicalize(downloads.join("Assemble")).unwrap()
        );
    }

    #[tokio::test]
    async fn a_received_file_is_verified_then_saved_under_a_safe_name() {
        let (_dir, root) = directory();
        let cancel = CancellationToken::new();
        let payload = b"hello from another device".to_vec();

        // 名字里带着路径与 Windows 不认的字符：只留最后一段，坏字符换掉。
        let mut source = Cursor::new(payload.clone());
        let received = accept(
            &root,
            "C:\\Users\\someone\\合同:终稿.pdf",
            &mut source,
            options(&cancel, &digest(&payload), payload.len()),
        )
        .await
        .unwrap();

        assert_eq!(received.name, "合同_终稿.pdf");
        assert_eq!(received.size, payload.len() as u64);
        assert_eq!(received.path, root.join("合同_终稿.pdf"));
        assert_eq!(fs::read(&received.path).unwrap(), payload);
        // 目录里只有它自己：暂存文件没有留下来。
        assert_eq!(entries(&root), ["合同_终稿.pdf"]);
    }

    #[tokio::test]
    async fn received_files_never_replace_what_is_already_there() {
        let (_dir, root) = directory();
        let cancel = CancellationToken::new();
        fs::write(root.join("报告.pdf"), b"earlier").unwrap();

        let payload = b"new".to_vec();
        let mut source = Cursor::new(payload.clone());
        let received = accept(
            &root,
            "报告.pdf",
            &mut source,
            options(&cancel, &digest(&payload), payload.len()),
        )
        .await
        .unwrap();

        assert_eq!(received.name, "报告 (2).pdf");
        assert_eq!(fs::read(root.join("报告.pdf")).unwrap(), b"earlier");
        assert_eq!(entries(&root), ["报告 (2).pdf", "报告.pdf"]);
    }

    #[tokio::test]
    async fn content_that_fails_verification_is_not_saved() {
        let (_dir, root) = directory();
        let cancel = CancellationToken::new();
        let payload = vec![7u8; 5_000];

        // 摘要对不上。
        let wrong = "0".repeat(64);
        let mut source = Cursor::new(payload.clone());
        let error = accept(
            &root,
            "a.bin",
            &mut source,
            options(&cancel, &wrong, payload.len()),
        )
        .await
        .unwrap_err();
        assert_eq!(
            ReceiveFailure::from(&error),
            ReceiveFailure::VerificationFailed
        );
        assert_eq!(error.code(), "HASH_MISMATCH");

        // 声明的长度与实际不符：多报与少报都算。
        for declared in [4_000usize, 6_000] {
            let mut source = Cursor::new(payload.clone());
            let error = accept(
                &root,
                "a.bin",
                &mut source,
                options(&cancel, &digest(&payload), declared),
            )
            .await
            .unwrap_err();
            assert_eq!(
                ReceiveFailure::from(&error),
                ReceiveFailure::VerificationFailed
            );
            assert_eq!(error.code(), "INCOMPLETE_DOWNLOAD");
        }

        assert_eq!(entries(&root), Vec::<String>::new());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_directory_that_disappears_during_the_transfer_takes_nothing_with_it() {
        let (dir, root) = directory();
        let cancel = CancellationToken::new();
        let payload = b"content".to_vec();
        let gone = root.clone();

        let mut source = tripwire(&payload, move || fs::remove_dir_all(&gone).unwrap());
        let error = accept(
            &root,
            "a.txt",
            &mut source,
            options(&cancel, &digest(&payload), payload.len()),
        )
        .await
        .unwrap_err();

        assert_eq!(ReceiveFailure::from(&error), ReceiveFailure::WriteFailed);
        // 目录已经不在了，任何地方都没有出现这个文件。
        assert!(!root.exists());
        drop(dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_directory_swapped_for_a_link_during_the_transfer_does_not_receive_the_file() {
        let (dir, root) = directory();
        let cancel = CancellationToken::new();
        let payload = b"content".to_vec();
        // 链接指向另一个临时目录：原目录整个删掉之后，它仍然是一个真实的目录。
        let (other, elsewhere) = directory();
        let original = root.clone();
        let link_target = elsewhere.clone();

        let mut source = tripwire(&payload, move || {
            fs::remove_dir_all(&original).unwrap();
            std::os::unix::fs::symlink(&link_target, &original).unwrap();
        });
        let error = accept(
            &root,
            "a.txt",
            &mut source,
            options(&cancel, &digest(&payload), payload.len()),
        )
        .await
        .unwrap_err();

        assert_eq!(ReceiveFailure::from(&error), ReceiveFailure::WriteFailed);
        assert_eq!(entries(&elsewhere), Vec::<String>::new());
        assert_eq!(entries(&root), Vec::<String>::new());
        drop((dir, other));
    }

    /// 读到一段之后就断掉的来源。
    struct CutOff {
        first: Option<Vec<u8>>,
    }

    impl AsyncRead for CutOff {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            match self.first.take() {
                Some(bytes) => {
                    buf.put_slice(&bytes);
                    Poll::Ready(Ok(()))
                }
                // 连接被重置时，网络上来的错误就是这样：类型不明，只带一句话。
                None => Poll::Ready(Err(std::io::Error::other("connection reset"))),
            }
        }
    }

    #[tokio::test]
    async fn a_source_that_stops_midway_is_told_apart_from_a_disk_failure() {
        let (_dir, root) = directory();
        let cancel = CancellationToken::new();
        let payload = vec![3u8; 9_000];
        let mut cut_off = CutOff {
            first: Some(payload[..4_000].to_vec()),
        };

        let error = accept(
            &root,
            "a.bin",
            &mut cut_off,
            options(&cancel, &digest(&payload), payload.len()),
        )
        .await
        .unwrap_err();

        // 是"读不到"，不是"写不下"；原来的错误信息留着，诊断时用得上。
        assert!(is_source_failure(&error), "{error}");
        assert!(error.to_string().contains("connection reset"), "{error}");
        assert_eq!(error.code(), "DEVICE_ERROR");
        // 写了一半的暂存文件没有留下来。
        assert_eq!(entries(&root), Vec::<String>::new());

        // 校验没通过与名字被占，都不是来源读不出来。
        let mut source = Cursor::new(b"x".to_vec());
        let mismatch = accept(
            &root,
            "a.txt",
            &mut source,
            options(&cancel, &"0".repeat(64), 1),
        )
        .await
        .unwrap_err();
        assert!(!is_source_failure(&mismatch));
        assert!(!is_source_failure(&TransferError::FileExists));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_disk_that_refuses_writes_is_not_blamed_on_the_source() {
        use std::os::unix::fs::PermissionsExt;

        let (dir, root) = directory();
        let cancel = CancellationToken::new();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o555)).unwrap();

        let mut source = Cursor::new(b"content".to_vec());
        let error = accept(&root, "a.txt", &mut source, SaveOptions::new(&cancel))
            .await
            .unwrap_err();

        // 收尾前把权限改回来，临时目录才删得掉。
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        drop(dir);
        assert!(!is_source_failure(&error), "{error}");
        assert_eq!(ReceiveFailure::from(&error), ReceiveFailure::WriteFailed);
    }

    #[tokio::test]
    async fn a_name_taken_during_the_transfer_is_reported_and_left_alone() {
        let (_dir, root) = directory();
        let cancel = CancellationToken::new();
        let payload = b"content".to_vec();
        let target = root.join("a.txt");

        let mut source = tripwire(&payload, move || fs::write(&target, b"rival").unwrap());
        let error = accept(
            &root,
            "a.txt",
            &mut source,
            options(&cancel, &digest(&payload), payload.len()),
        )
        .await
        .unwrap_err();

        // 调用方据此重新接收一次；抢先建出来的文件没有被覆盖。
        assert!(matches!(error, TransferError::FileExists), "{error}");
        assert_eq!(fs::read(root.join("a.txt")).unwrap(), b"rival");
        assert_eq!(entries(&root), ["a.txt"]);
    }

    #[tokio::test]
    async fn a_file_with_nothing_left_in_its_name_still_gets_a_fallback_name() {
        let (_dir, root) = directory();
        let cancel = CancellationToken::new();
        let mut source = Cursor::new(b"x".to_vec());
        let received = accept(
            &root,
            "../../",
            &mut source,
            SaveOptions {
                content_length: Some(1),
                ..SaveOptions::new(&cancel)
            },
        )
        .await
        .unwrap();
        assert_eq!(received.name, "file");
        assert_eq!(received.path, root.join("file"));
    }

    #[tokio::test]
    async fn sweeping_removes_only_files_that_look_like_interrupted_staging() {
        let (_dir, root) = directory();
        // 链接指向的文件放在另一个临时目录里，不混进被清理的目录。
        let (other, outside_dir) = directory();
        let staging = |name: &str| format!("{name}.0123456789abcdef0123456789abcdef.part");
        fs::write(root.join(staging("a.txt")), b"half").unwrap();
        fs::write(root.join(staging("b.bin")), b"half").unwrap();

        // 这些都不是暂存文件，一个都不能动。
        let kept = [
            "a.txt",
            "notes.part",
            "报告.pdf.part",
            "x.0123456789abcdef0123456789abcde.part",
            "x.0123456789ABCDEF0123456789ABCDEF.part",
            "y.0123456789abcdef0123456789abcdef.partial",
        ];
        for name in kept {
            fs::write(root.join(name), b"keep").unwrap();
        }
        fs::create_dir(root.join(staging("dir"))).unwrap();
        #[cfg(unix)]
        {
            let outside = outside_dir.join("outside.txt");
            fs::write(&outside, b"keep").unwrap();
            std::os::unix::fs::symlink(&outside, root.join(staging("link"))).unwrap();
            std::os::unix::fs::symlink(root.join("elsewhere"), root.join(staging("dangling")))
                .unwrap();
        }

        assert_eq!(sweep_staging(&root).await.unwrap(), 2);
        assert_eq!(sweep_staging(&root).await.unwrap(), 0);

        let mut remaining = entries(&root);
        for name in kept {
            assert!(remaining.contains(&name.to_owned()), "{name}");
            remaining.retain(|entry| entry != name);
        }
        // 剩下的只有那个目录与两个链接。
        assert_eq!(
            remaining.len(),
            if cfg!(unix) { 3 } else { 1 },
            "{remaining:?}"
        );
        #[cfg(unix)]
        assert_eq!(fs::read(outside_dir.join("outside.txt")).unwrap(), b"keep");

        // 目录不存在不算错。
        assert_eq!(sweep_staging(&root.join("missing")).await.unwrap(), 0);
        drop(other);
    }

    #[test]
    fn failures_are_reported_with_the_reasons_the_protocol_knows() {
        let full = TransferError::Io(std::io::Error::from_raw_os_error(28));
        assert_eq!(ReceiveFailure::from(&full), ReceiveFailure::DiskFull);

        let denied = TransferError::Io(std::io::Error::from_raw_os_error(13));
        assert_eq!(ReceiveFailure::from(&denied), ReceiveFailure::WriteFailed);
        assert_eq!(
            ReceiveFailure::from(&TransferError::FileExists),
            ReceiveFailure::WriteFailed
        );
        assert_eq!(
            ReceiveFailure::from(&TransferError::Cancelled),
            ReceiveFailure::Cancelled
        );

        // 每一种原因的拼法，就是协议里 `FailReason` 的拼法。
        for (reason, spelled) in [
            (ReceiveFailure::DiskFull, FailReason::DiskFull),
            (ReceiveFailure::WriteFailed, FailReason::WriteFailed),
            (
                ReceiveFailure::VerificationFailed,
                FailReason::VerificationFailed,
            ),
            (ReceiveFailure::Cancelled, FailReason::Cancelled),
        ] {
            assert_eq!(
                serde_json::to_value(spelled).unwrap(),
                reason.as_str(),
                "{reason:?}"
            );
        }
    }
}
