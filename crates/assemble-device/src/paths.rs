//! 路径边界：只有明确授权的目录、且不经过链接的路径才能被访问。
//!
//! 两道门：
//!
//! - [`validate_root`]：共享目录本身必须是一个真实的绝对路径，不能是磁盘根目录，
//!   也不能经过任何链接；
//! - [`authorized_path`]：共享目录之下的相对路径逐段检查，任何一段是链接、
//!   或者实际位置跑出了共享目录，都直接拒绝。
//!
//! 凭据、依赖和构建产物永远不参与共享，见 [`excluded_path`]。

use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use tokio_util::sync::CancellationToken;

use crate::error::{DeviceError, io_failure};

/// 相对路径里有一段落在排除名单里：凭据、依赖与构建产物。
pub fn excluded_path(relative: &str) -> bool {
    relative.split(['/', '\\']).any(excluded_segment)
}

/// 完全匹配就排除的名字。
const EXCLUDED_NAMES: [&str; 15] = [
    ".git",
    "node_modules",
    "dist",
    "build",
    ".next",
    ".turbo",
    ".cache",
    "coverage",
    ".ssh",
    ".aws",
    ".npmrc",
    ".netrc",
    "id_rsa",
    "id_ed25519",
    "credentials",
];

/// 按扩展名排除的私钥、密钥库与传输中的暂存文件。
const EXCLUDED_EXTENSIONS: [&str; 5] = ["pem", "key", "p12", "pfx", "kdbx"];

fn excluded_segment(segment: &str) -> bool {
    let lower = segment.to_lowercase();
    if EXCLUDED_NAMES.contains(&lower.as_str()) || lower == "credentials.json" {
        return true;
    }
    // `.env` 以及它的各种变体。
    if lower.starts_with(".env") {
        return true;
    }
    lower.rsplit_once('.').is_some_and(|(_, extension)| {
        EXCLUDED_EXTENSIONS.contains(&extension) || extension == "part"
    })
}

/// 最后一段尚不存在时是否允许通过。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllowMissing {
    /// 路径必须已经存在。
    No,
    /// 只有最后一段可以不存在，用于即将写入的文件。
    LastSegment,
}

/// 检查一个共享目录，返回它的规范形式。
///
/// 拒绝相对路径、磁盘根目录、符号链接与 junction，以及任何规范路径
/// 与字面路径不一致的目录。
pub async fn validate_root(path: &Path) -> Result<PathBuf, DeviceError> {
    if !path.is_absolute() || path.to_string_lossy().contains('\0') {
        return Err(DeviceError::ROOT_NOT_ABSOLUTE);
    }
    let normalized = normalize_lexical(path);
    if normalized.parent().is_none() {
        return Err(DeviceError::ROOT_IS_DISK);
    }
    // 根目录上的链接或 junction 与目录内部的链接一样被拒绝。
    let info = match tokio::fs::symlink_metadata(&normalized).await {
        Ok(info) => info,
        Err(error) => return Err(io_failure("stat_root", &error)),
    };
    if !info.is_dir() || info.file_type().is_symlink() {
        return Err(DeviceError::ROOT_IS_LINK);
    }
    let canonical = match tokio::fs::canonicalize(&normalized).await {
        Ok(canonical) => canonical,
        Err(error) => return Err(io_failure("realpath_root", &error)),
    };
    if canonical != normalized {
        return Err(DeviceError::ROOT_IS_LINKED);
    }
    Ok(canonical)
}

/// 去掉路径里的 `.` 与多余的 `..`，但不触碰文件系统。
fn normalize_lexical(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            // 已经在根上时 `pop` 不会越过根。
            Component::ParentDir => {
                normalized.pop();
            }
            Component::RootDir | Component::Prefix(_) | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    if normalized.as_os_str().is_empty() {
        path.to_path_buf()
    } else {
        normalized
    }
}

/// 在共享目录之下解析一个相对路径，返回它在本机的绝对位置。
///
/// 只接受安全形式的相对路径；逐段确认没有链接、没有跑出共享目录。
pub async fn authorized_path(
    root: &Path,
    relative: &str,
    allow_missing: AllowMissing,
) -> Result<PathBuf, DeviceError> {
    let parts = safe_parts(relative)?;
    if excluded_path(relative) {
        return Err(DeviceError::PATH_EXCLUDED);
    }
    let canonical = validate_root(root).await?;
    let mut current = canonical.clone();

    for (index, part) in parts.iter().enumerate() {
        current.push(part);
        let is_last = index + 1 == parts.len();
        match inspect_step(&canonical, &current, is_last).await {
            Step::Present => {}
            // 只有最后一段允许"还不存在"。
            Step::Missing if is_last && allow_missing == AllowMissing::LastSegment => {
                return Ok(current);
            }
            Step::Missing => return Err(DeviceError::NOT_FOUND),
            Step::Rejected(error) => return Err(error),
        }
    }
    Ok(current)
}

/// 把相对路径拆成若干段，并确认每一段都安全。
fn safe_parts(relative: &str) -> Result<Vec<&str>, DeviceError> {
    if relative.is_empty()
        || relative.contains('\0')
        || Path::new(relative).is_absolute()
        || has_drive_prefix(relative)
    {
        return Err(DeviceError::PATH_NOT_RELATIVE);
    }
    let parts: Vec<&str> = relative.split(['/', '\\']).collect();
    if parts.iter().any(|part| !is_safe_segment(part)) {
        return Err(DeviceError::PATH_NOT_RELATIVE);
    }
    Ok(parts)
}

fn has_drive_prefix(relative: &str) -> bool {
    relative.get(..2).is_some_and(|prefix| {
        prefix.ends_with(':') && prefix.starts_with(|first: char| first.is_ascii_alphabetic())
    })
}

fn is_safe_segment(segment: &str) -> bool {
    if segment.is_empty() || matches!(segment, "." | "..") {
        return false;
    }
    if segment.contains([':', '<', '>', '"', '|', '?', '*']) {
        return false;
    }
    if segment.ends_with(['.', ' ']) {
        return false;
    }
    !is_windows_reserved(segment)
}

/// Windows 保留的设备名，带不带扩展名都算。
fn is_windows_reserved(segment: &str) -> bool {
    let stem = segment.split('.').next().unwrap_or_default();
    let lower = stem.to_lowercase();
    let Some(rest) = lower
        .strip_prefix("com")
        .or_else(|| lower.strip_prefix("lpt"))
    else {
        return matches!(lower.as_str(), "con" | "prn" | "aux" | "nul");
    };
    matches!(rest, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
}

/// 逐段检查的结论。
enum Step {
    Present,
    Missing,
    Rejected(DeviceError),
}

/// 检查已经拼出来的一段路径：不能是链接，中间段必须是目录，且仍在共享目录内。
async fn inspect_step(canonical_root: &Path, current: &Path, is_last: bool) -> Step {
    let info = match tokio::fs::symlink_metadata(current).await {
        Ok(info) => info,
        Err(error) if error.kind() == ErrorKind::NotFound => return Step::Missing,
        Err(error) => return Step::Rejected(io_failure("stat_step", &error)),
    };
    if info.file_type().is_symlink() {
        return Step::Rejected(DeviceError::LINK_IS_SYMLINK);
    }
    if !is_last && !info.is_dir() {
        return Step::Rejected(DeviceError::PATH_PARENT_NOT_DIRECTORY);
    }
    let actual = match tokio::fs::canonicalize(current).await {
        Ok(actual) => actual,
        Err(error) => return Step::Rejected(io_failure("realpath_step", &error)),
    };
    if actual.strip_prefix(canonical_root).is_err() {
        return Step::Rejected(DeviceError::LINK_LEAVES_ROOT);
    }
    Step::Present
}

/// 列出一个共享目录里的全部文件，返回以 `/` 分隔的相对路径。
///
/// 跳过排除名单里的路径和所有链接；每个子目录进入前都重新校验边界。
pub async fn walk_files(
    root: &Path,
    cancel: &CancellationToken,
) -> Result<Vec<String>, DeviceError> {
    let mut files = Vec::new();
    // 空串表示共享目录本身。
    let mut pending = vec![String::new()];

    while let Some(prefix) = pending.pop() {
        if cancel.is_cancelled() {
            return Err(DeviceError::CANCELLED);
        }
        let directory = if prefix.is_empty() {
            validate_root(root).await?
        } else {
            authorized_path(root, &prefix, AllowMissing::No).await?
        };
        let mut entries = match tokio::fs::read_dir(&directory).await {
            Ok(entries) => entries,
            Err(error) => return Err(io_failure("read_dir", &error)),
        };
        loop {
            let entry = match entries.next_entry().await {
                Ok(Some(entry)) => entry,
                Ok(None) => break,
                Err(error) => return Err(io_failure("read_dir_entry", &error)),
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            let relative = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            if excluded_path(&relative) {
                continue;
            }
            let kind = match entry.file_type().await {
                Ok(kind) => kind,
                Err(error) => return Err(io_failure("file_type", &error)),
            };
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                pending.push(relative);
            } else if kind.is_file() {
                files.push(relative);
            }
        }
    }
    files.sort_unstable();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;

    use super::*;

    /// macOS 的临时目录本身经过链接，先取规范路径再当共享目录用。
    fn root() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let canonical = fs::canonicalize(dir.path()).unwrap();
        (dir, canonical)
    }

    #[test]
    fn credentials_dependencies_and_generated_trees_are_excluded() {
        for excluded in [
            ".git/config",
            "node_modules/pkg/index.js",
            "dist/app.js",
            "build/out.js",
            ".next/static",
            ".turbo/cache",
            ".cache/x",
            "coverage/lcov.info",
            ".env",
            ".env.local",
            ".env.production",
            ".ssh/id_rsa",
            ".aws/credentials",
            ".npmrc",
            ".netrc",
            "id_rsa",
            "id_ed25519",
            "credentials",
            "credentials.json",
            "server.pem",
            "certs/client.key",
            "store.p12",
            "bundle.pfx",
            "vault.kdbx",
            "downloads/a.zip.9f.part",
            "a\\b\\node_modules\\c",
            ".GIT\\hooks",
            "NODE_MODULES/x",
            ".Env",
            ".environment",
        ] {
            assert!(excluded_path(excluded), "{excluded} should be excluded");
        }
        for kept in [
            "notes.md",
            "distillation/process.md",
            "buildings/plan.txt",
            "environment.md",
            "credentials.txt",
            "id_rsa_backup.txt",
            "certificate.pem.bak",
            "party.md",
            "keys.md",
        ] {
            assert!(!excluded_path(kept), "{kept} should be kept");
        }
    }

    #[tokio::test]
    async fn a_root_must_be_an_explicit_absolute_non_volume_path() {
        let (_dir, root) = root();

        assert_eq!(
            validate_root(Path::new("relative/dir")).await,
            Err(DeviceError::ROOT_NOT_ABSOLUTE)
        );
        assert_eq!(
            validate_root(Path::new("a\0b")).await,
            Err(DeviceError::ROOT_NOT_ABSOLUTE)
        );
        assert_eq!(
            validate_root(Path::new("/")).await,
            Err(DeviceError::ROOT_IS_DISK)
        );
        assert_eq!(
            validate_root(&root.join("missing")).await,
            Err(DeviceError::FAILED)
        );

        assert_eq!(validate_root(&root).await.unwrap(), root);
        // 字面路径里的 `..` 先被规整，再逐级确认。
        assert_eq!(validate_root(&root.join("child/..")).await.unwrap(), root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn linked_roots_and_roots_behind_links_are_rejected() {
        let (dir, root) = root();
        let real = root.join("real");
        fs::create_dir(&real).unwrap();

        symlink(&real, dir.path().join("alias")).unwrap();
        // `alias` 的字面路径能通过 `..` 回到它自己，但它的规范路径是 `real`。
        assert_eq!(
            validate_root(&root.join("alias")).await,
            Err(DeviceError::ROOT_IS_LINK)
        );

        // 一个普通文件不能当共享目录。
        fs::write(root.join("file.txt"), b"x").unwrap();
        assert_eq!(
            validate_root(&root.join("file.txt")).await,
            Err(DeviceError::ROOT_IS_LINK)
        );
    }

    #[tokio::test]
    async fn traversal_absolute_and_escaping_paths_are_rejected() {
        let (_dir, root) = root();
        fs::create_dir(root.join("docs")).unwrap();
        fs::write(root.join("docs/a.txt"), b"a").unwrap();

        for rejected in [
            "",
            "../outside",
            "docs/../..",
            "docs/./a.txt",
            "/etc/passwd",
            "C:/Windows",
            "c:boot.ini",
            "\\\\server\\share",
            "docs//a.txt",
            "docs/a.txt/",
            "a\0b",
            "docs:name",
            "docs/a?.txt",
            "docs/a*.txt",
            "docs/trailing.",
            "docs/trailing ",
            "docs/NUL",
            "docs/com1.txt",
            "docs/lpt9",
        ] {
            let error = authorized_path(&root, rejected, AllowMissing::LastSegment)
                .await
                .unwrap_err();
            assert!(
                matches!(error.code, "INVALID_PATH" | "EXCLUDED_PATH"),
                "{rejected:?} -> {error}"
            );
        }
        assert_eq!(
            authorized_path(&root, "docs/a.txt", AllowMissing::No)
                .await
                .unwrap(),
            root.join("docs/a.txt")
        );
        // 两种分隔符都认。
        assert_eq!(
            authorized_path(&root, "docs\\a.txt", AllowMissing::No)
                .await
                .unwrap(),
            root.join("docs/a.txt")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn links_inside_a_shared_directory_never_grant_access_outside() {
        let (dir, root) = root();
        fs::create_dir(root.join("inside")).unwrap();
        fs::write(root.join("inside/a.txt"), b"a").unwrap();
        let outside = dir.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("secret.txt"), b"secret").unwrap();

        symlink(&outside, root.join("docs")).unwrap();
        symlink(outside.join("secret.txt"), root.join("link.txt")).unwrap();
        symlink(root.join("inside/a.txt"), root.join("inside-link.txt")).unwrap();

        // 指向目录外的链接目录。
        assert_eq!(
            authorized_path(&root, "docs/secret.txt", AllowMissing::No).await,
            Err(DeviceError::LINK_IS_SYMLINK)
        );
        // 指向目录外的链接文件，即使目标内容无害也拒绝。
        assert_eq!(
            authorized_path(&root, "link.txt", AllowMissing::No).await,
            Err(DeviceError::LINK_IS_SYMLINK)
        );
        // 指回共享目录内部的链接同样拒绝：链接一律不共享。
        assert_eq!(
            authorized_path(&root, "inside-link.txt", AllowMissing::No).await,
            Err(DeviceError::LINK_IS_SYMLINK)
        );
    }

    #[tokio::test]
    async fn files_are_created_only_inside_existing_authorized_parents() {
        let (_dir, root) = root();
        fs::create_dir(root.join("docs")).unwrap();
        fs::write(root.join("docs/a.txt"), b"a").unwrap();

        // 最后一段不存在：允许用于即将写入的文件。
        assert_eq!(
            authorized_path(&root, "docs/new.txt", AllowMissing::LastSegment)
                .await
                .unwrap(),
            root.join("docs/new.txt")
        );
        // 同一条路径在要求存在时失败。
        assert_eq!(
            authorized_path(&root, "docs/new.txt", AllowMissing::No).await,
            Err(DeviceError::NOT_FOUND)
        );
        // 父目录不存在：即使允许最后一段缺失也不行。
        assert_eq!(
            authorized_path(&root, "missing/new.txt", AllowMissing::LastSegment).await,
            Err(DeviceError::NOT_FOUND)
        );
        // 父路径是个文件。
        assert_eq!(
            authorized_path(&root, "docs/a.txt/child", AllowMissing::LastSegment).await,
            Err(DeviceError::PATH_PARENT_NOT_DIRECTORY)
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn walking_skips_excluded_paths_and_links_and_visits_nested_files() {
        let (_dir, root) = root();
        for directory in [
            "docs/2026",
            "node_modules/pkg",
            "project/.git",
            "project/src",
        ] {
            fs::create_dir_all(root.join(directory)).unwrap();
        }
        for file in [
            "docs/2026/report.md",
            "docs/notes.md",
            "project/src/main.rs",
            "project/.env",
            "node_modules/pkg/index.js",
            "project/.git/config",
            "project/secret.pem",
        ] {
            fs::write(root.join(file), b"x").unwrap();
        }
        symlink(root.join("docs"), root.join("linked-docs")).unwrap();
        symlink(root.join("docs/notes.md"), root.join("linked-note.md")).unwrap();

        let files = walk_files(&root, &CancellationToken::new()).await.unwrap();
        assert_eq!(
            files,
            vec![
                "docs/2026/report.md".to_owned(),
                "docs/notes.md".to_owned(),
                "project/src/main.rs".to_owned()
            ]
        );
    }

    #[tokio::test]
    async fn walking_stops_when_cancelled() {
        let (_dir, root) = root();
        fs::write(root.join("a.txt"), b"a").unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert_eq!(
            walk_files(&root, &cancel).await,
            Err(DeviceError::CANCELLED)
        );
    }

    #[test]
    fn lexical_normalisation_stays_at_the_root() {
        assert_eq!(normalize_lexical(Path::new("/a/b/../c")), Path::new("/a/c"));
        assert_eq!(normalize_lexical(Path::new("/a/./b")), Path::new("/a/b"));
        assert_eq!(normalize_lexical(Path::new("/../../a")), Path::new("/a"));
        assert_eq!(normalize_lexical(Path::new("/")), Path::new("/"));
    }
}
