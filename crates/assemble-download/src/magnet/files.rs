//! BT 只写自己的缓存。发布使用已校验副本和原子硬链接，不覆盖已有文件。
use crate::{DownloadError, Result};
use assemble_protocol::downloads::DownloadFile;
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Publication {
    pub root: PathBuf,
    pub hashes: Vec<(u32, String)>,
}

pub(super) fn relative_path(value: &str) -> Result<PathBuf> {
    if value.len() > 2048
        || value.is_empty()
        || value.contains('\\')
        || value.contains(':')
        || value.chars().any(char::is_control)
    {
        return Err(DownloadError::InvalidMedia);
    }
    let path = Path::new(value);
    if path
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(DownloadError::InvalidMedia);
    }
    for component in value.split('/') {
        if component.is_empty() || assemble_device::safe_file_name(component) != component {
            return Err(DownloadError::InvalidMedia);
        }
    }
    Ok(path.to_path_buf())
}

pub(super) async fn directory(path: &Path) -> Result<PathBuf> {
    let value = tokio::fs::symlink_metadata(path)
        .await
        .map_err(|_| DownloadError::Storage)?;
    if !value.is_dir() || value.file_type().is_symlink() {
        return Err(DownloadError::Storage);
    }
    assemble_device::validate_directory(path)
        .await
        .map_err(|_| DownloadError::Storage)
}

pub(super) async fn cache(folder: &Path, id: &str, create: bool) -> Result<PathBuf> {
    if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(DownloadError::Storage);
    }
    let folder = directory(folder).await?;
    let path = folder.join(format!(".assemble-magnet-{id}"));
    if create {
        match tokio::fs::create_dir(&path).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(crate::space::write_failure(&e)),
        }
        directory(&path).await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
                .await
                .map_err(|_| DownloadError::Storage)?;
        }
    }
    directory(&path).await?;
    reject_links(&path).await?;
    Ok(path)
}

pub(super) async fn reject_links(root: &Path) -> Result<()> {
    let mut pending = vec![root.to_path_buf()];
    let mut count = 0;
    while let Some(path) = pending.pop() {
        let mut entries = tokio::fs::read_dir(path)
            .await
            .map_err(|_| DownloadError::Storage)?;
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|_| DownloadError::Storage)?
        {
            count += 1;
            if count > 30_000 {
                return Err(DownloadError::Limit);
            }
            let kind = entry
                .file_type()
                .await
                .map_err(|_| DownloadError::Storage)?;
            if kind.is_symlink() || (!kind.is_file() && !kind.is_dir()) {
                return Err(DownloadError::Storage);
            }
            if kind.is_dir() {
                pending.push(entry.path());
            }
        }
    }
    Ok(())
}

pub(super) async fn regular(root: &Path, file: &DownloadFile) -> Result<PathBuf> {
    let path = root.join(relative_path(&file.name)?);
    let meta = tokio::fs::symlink_metadata(&path)
        .await
        .map_err(|_| DownloadError::Storage)?;
    let canonical = tokio::fs::canonicalize(&path)
        .await
        .map_err(|_| DownloadError::Storage)?;
    if !meta.is_file()
        || meta.file_type().is_symlink()
        || meta.len() != file.size
        || !canonical.starts_with(
            tokio::fs::canonicalize(root)
                .await
                .map_err(|_| DownloadError::Storage)?,
        )
    {
        return Err(DownloadError::InvalidMedia);
    }
    Ok(path)
}

pub(super) async fn publication(
    folder: &Path,
    title: &str,
    data: &Path,
    files: &[DownloadFile],
    cancel: &CancellationToken,
) -> Result<Publication> {
    let mut hashes = Vec::new();
    for file in files {
        hashes.push((file.id, seal(&regular(data, file).await?, cancel).await?));
    }
    let base = assemble_device::safe_file_name(title);
    for _ in 0..100 {
        let name = assemble_device::choose_file_name(folder, &base)
            .await
            .map_err(|_| DownloadError::Storage)?;
        let path = folder.join(name);
        match tokio::fs::create_dir(&path).await {
            Ok(()) => return Ok(Publication { root: path, hashes }),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(crate::space::write_failure(&e)),
        }
    }
    Err(DownloadError::Storage)
}

pub(super) async fn publish(
    publication: &Publication,
    folder: &Path,
    cache: &Path,
    files: &[DownloadFile],
    cancel: &CancellationToken,
) -> Result<()> {
    if publication.root.parent() != Some(folder) {
        return Err(DownloadError::Storage);
    }
    directory(&publication.root).await?;
    reject_links(&publication.root).await?;
    for file in files {
        cancel
            .run_until_cancelled(async {
                let expected = publication
                    .hashes
                    .iter()
                    .find(|(id, _)| *id == file.id)
                    .map(|(_, hash)| hash)
                    .ok_or(DownloadError::Storage)?;
                let destination = publication.root.join(relative_path(&file.name)?);
                if tokio::fs::try_exists(&destination)
                    .await
                    .map_err(|_| DownloadError::Storage)?
                {
                    let path = regular(&publication.root, file).await?;
                    if &seal(&path, cancel).await? != expected {
                        return Err(DownloadError::Storage);
                    }
                    return Ok(());
                }
                let parent = destination.parent().ok_or(DownloadError::Storage)?;
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|e| crate::space::write_failure(&e))?;
                reject_links(&publication.root).await?;
                let staging =
                    tempfile::NamedTempFile::new_in(cache).map_err(|_| DownloadError::Storage)?;
                tokio::fs::copy(regular(&cache.join("data"), file).await?, staging.path())
                    .await
                    .map_err(|e| crate::space::write_failure(&e))?;
                if &seal(staging.path(), cancel).await? != expected {
                    return Err(DownloadError::InvalidMedia);
                }
                tokio::fs::hard_link(staging.path(), &destination)
                    .await
                    .map_err(|_| DownloadError::Storage)?;
                Ok(())
            })
            .await
            .ok_or(DownloadError::Cancelled)??;
    }
    Ok(())
}

async fn seal(path: &Path, cancel: &CancellationToken) -> Result<String> {
    super::super::manager::seal_with_limit(path, cancel, super::engine::MAX_SELECTED, true).await
}

pub(super) async fn remove_cache(folder: &Path, id: &str) -> Result<()> {
    let candidate = folder.join(format!(".assemble-magnet-{id}"));
    match tokio::fs::symlink_metadata(&candidate).await {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(DownloadError::Storage),
        Ok(_) => {}
    }
    let path = cache(folder, id, false).await?;
    tokio::fs::remove_dir_all(path)
        .await
        .map_err(|_| DownloadError::Storage)
}
