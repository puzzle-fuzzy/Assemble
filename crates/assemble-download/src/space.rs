//! 下载目录所在的磁盘还剩多少地方。
//!
//! 下载一路写到把盘写满才失败，既白费了流量，也会在别人正用着的盘上把最后一口气用光。
//! 所以开始之前先看一眼余量，不够就不开始，并说清楚是空间不够。点播清单的总大小
//! 事先不知道，所以开始之后仍然可能写满：那时候把"磁盘已满"和别的存储故障分开说。

use std::io;
use std::path::Path;

use crate::{DownloadError, MIN_FREE_BYTES, Result};

/// 这块磁盘放得下 `wanted` 字节吗：写完之后还得给系统留着一口气。
pub(crate) fn enough(free: u64, wanted: u64) -> bool {
    free.saturating_sub(wanted) >= MIN_FREE_BYTES
}

/// 开始之前问一句：`path` 所在的磁盘放得下 `wanted` 字节吗。
/// 要写多少还不知道时给 0，只看那一口气还在不在。
pub(crate) fn ensure_room(path: &Path, wanted: u64) -> Result<()> {
    // 文件可能还没建：问它所在的目录。
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(path);
    let free = fs4::available_space(directory).map_err(|_| DownloadError::Storage)?;
    if enough(free, wanted) {
        Ok(())
    } else {
        Err(DownloadError::NoSpace)
    }
}

/// 写文件失败时的说法：磁盘写满是它自己的一个错，不和别的存储故障混在一起。
pub(crate) fn write_failure(error: &io::Error) -> DownloadError {
    if error.kind() == io::ErrorKind::StorageFull {
        DownloadError::NoSpace
    } else {
        DownloadError::Storage
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_means_the_bytes_plus_breathing_room() {
        assert!(enough(MIN_FREE_BYTES, 0));
        assert!(enough(MIN_FREE_BYTES + 4096, 4096));
        assert!(!enough(MIN_FREE_BYTES, 1));
        assert!(!enough(MIN_FREE_BYTES - 1, 0));
        assert!(!enough(u64::MAX, u64::MAX));
    }

    #[test]
    fn a_full_disk_is_told_apart_from_other_storage_failures() {
        assert_eq!(
            write_failure(&io::Error::new(io::ErrorKind::StorageFull, "no space")),
            DownloadError::NoSpace
        );
        for other in [
            io::ErrorKind::PermissionDenied,
            io::ErrorKind::NotFound,
            io::Error::other("broken pipe").kind(),
        ] {
            assert_eq!(
                write_failure(&io::Error::new(other, "something else")),
                DownloadError::Storage
            );
        }
    }

    #[test]
    fn asking_a_real_disk_works_even_before_the_file_exists() {
        let dir = tempfile::tempdir().unwrap();
        ensure_room(&dir.path().join("not-yet.mp4"), 0).unwrap();
        // 谁也拿不出这么大一块盘。
        assert_eq!(
            ensure_room(&dir.path().join("not-yet.mp4"), u64::MAX).unwrap_err(),
            DownloadError::NoSpace
        );
        // 目录本身不存在：如实说问不出来，不当作空间不够。
        assert_eq!(
            ensure_room(&dir.path().join("missing").join("x.mp4"), 0).unwrap_err(),
            DownloadError::Storage
        );
    }
}
