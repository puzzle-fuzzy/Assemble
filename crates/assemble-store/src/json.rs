//! 状态文件的原子私有读写。

use std::fs;
use std::io::{BufWriter, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::Duration;

use serde::{Serialize, de::DeserializeOwned};
use tempfile::{Builder, NamedTempFile};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("local data is unavailable: {0}")]
    Io(#[from] std::io::Error),
    #[error("local data could not be encoded: {0}")]
    Encode(#[from] serde_json::Error),
    /// 文件存在但不是合法 JSON。宁可失败也不回退到默认值，避免把损坏当成空数据。
    #[error("local data file is malformed: {}", path.display())]
    Corrupt {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

/// Windows 上的杀毒与索引可能短暂占用目标文件。等待后重试，绝不先删旧文件。
const RENAME_RETRY_DELAYS: [Duration; 6] = [
    Duration::from_millis(10),
    Duration::from_millis(20),
    Duration::from_millis(40),
    Duration::from_millis(80),
    Duration::from_millis(160),
    Duration::from_millis(250),
];

/// Windows 上值得重试的替换失败：拒绝访问、共享冲突、锁定冲突。
const WINDOWS_RETRYABLE_ERRORS: [i32; 3] = [5, 32, 33];

/// 同一个进程里写状态文件的排成一队。
///
/// 临时文件的名字各不相同，彼此碰不到；大家替换的是同一个目标。类 Unix 上替换是原子的，
/// Windows 上不是：两个线程同时往同一个目标上改名，可能一个失败，目标还可能在那一下消失
/// （2026-11-25 持续集成的 Windows 那一栏上，"并发写入"那条测试就这样失败过一次）。
/// 写状态文件本来就不频繁，排队的代价可以忽略。
static WRITERS: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 读取一份状态文件；不存在时返回 `None`，内容损坏时返回错误。
pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, StoreError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|source| StoreError::Corrupt {
            path: path.to_owned(),
            source,
        })
}

/// 原子地写入一份状态文件，权限为仅本人可读写。
///
/// 先在同目录写入唯一的临时文件并落盘，再替换目标；任何时刻读到的
/// 要么是完整的新内容，要么是完整的旧内容。同一时刻只有一个写入者（见 [`WRITERS`]）。
pub fn write_private_json<T: Serialize + ?Sized>(path: &Path, value: &T) -> Result<(), StoreError> {
    // 持有到替换完成为止。锁被毒掉只说明上一个写入者在半路上慌了退出了，文件本身没事。
    let _turn = WRITERS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    create_private_dir(parent)?;

    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("state");
    // 独占创建的唯一文件名把并发的写入者彼此隔开；临时文件默认仅本人可读写。
    let mut staging = Builder::new()
        .prefix(&format!("{name}."))
        .suffix(".tmp")
        .tempfile_in(parent)?;
    {
        let mut writer = BufWriter::new(staging.as_file_mut());
        serde_json::to_writer_pretty(&mut writer, value)?;
        writer.write_all(b"\n")?;
        writer.flush()?;
    }
    staging.as_file().sync_all()?;
    replace_with_retry(staging, path)?;
    // "仅本人可读写"是类 Unix 上的文件权限位；Windows 上没有对应的东西可设。
    #[cfg(unix)]
    set_private_file(path)?;
    Ok(())
}

#[cfg(unix)]
pub(crate) fn create_private_dir(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;

    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
}

#[cfg(not(unix))]
pub(crate) fn create_private_dir(path: &Path) -> std::io::Result<()> {
    fs::create_dir_all(path)
}

#[cfg(unix)]
pub(crate) fn set_private_file(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

fn replace_with_retry(mut staging: NamedTempFile, path: &Path) -> Result<(), StoreError> {
    let mut attempt = 0;
    loop {
        match staging.persist(path) {
            Ok(_persisted) => return Ok(()),
            Err(failure) => {
                let delay = RENAME_RETRY_DELAYS
                    .get(attempt)
                    .copied()
                    .filter(|_| is_retryable_rename(&failure.error));
                // 不值得重试，或等待预算用尽：如实上报。
                let Some(delay) = delay else {
                    return Err(failure.error.into());
                };
                // 失败后临时文件仍然归本次调用所有，保留下来再试。
                staging = failure.file;
                sleep(delay);
                attempt += 1;
            }
        }
    }
}

/// 只有 Windows 上的占用类错误才值得重试；其他平台和其他错误直接上报。
fn is_retryable_rename(error: &std::io::Error) -> bool {
    cfg!(windows)
        && error
            .raw_os_error()
            .is_some_and(|code| WINDOWS_RETRYABLE_ERRORS.contains(&code))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::Arc;

    use serde_json::{Value, json};

    use super::*;

    fn assert_private(path: &Path, mode: u32) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let actual = fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(actual, mode, "{}", path.display());
        }
        #[cfg(not(unix))]
        let _ = (path, mode);
    }

    #[test]
    fn writes_pretty_json_that_only_the_owner_can_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/deep/state.json");
        write_private_json(&path, &json!({ "b": 1, "a": [true, null] })).unwrap();

        let text = fs::read_to_string(&path).unwrap();
        assert!(text.ends_with("}\n"));
        assert!(text.contains("\n  \"a\":"));
        assert_eq!(
            read_json::<Value>(&path).unwrap(),
            Some(json!({ "b": 1, "a": [true, null] }))
        );
        assert_private(&path, 0o600);
        assert_private(&dir.path().join("nested"), 0o700);
        assert_private(&dir.path().join("nested/deep"), 0o700);
    }

    #[test]
    fn an_existing_file_is_replaced_whole_without_leftovers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        write_private_json(&path, &json!({ "generation": 1 })).unwrap();
        write_private_json(&path, &json!({ "generation": 2 })).unwrap();

        assert_eq!(read_json::<Value>(&path).unwrap().unwrap()["generation"], 2);
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name != "state.json")
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn concurrent_writers_never_expose_a_partial_document() {
        let dir = tempfile::tempdir().unwrap();
        let path = Arc::new(dir.path().join("state.json"));
        let workers: Vec<_> = (0..8)
            .map(|worker| {
                let path = Arc::clone(&path);
                std::thread::spawn(move || {
                    for round in 0..20 {
                        let document =
                            json!({ "worker": worker, "round": round, "pad": "x".repeat(2_000) });
                        write_private_json(&path, &document).unwrap();
                        let seen: Value = read_json(&path).unwrap().unwrap();
                        // 任何一次读取都必须是某次写入的完整文档；
                        // 撕裂的内容会在解析时失败，而不是被当成合法状态。
                        assert_eq!(seen["pad"].as_str().map(str::len), Some(2_000));
                    }
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        let final_document: Value = read_json(&path).unwrap().unwrap();
        assert_eq!(final_document["pad"].as_str().map(str::len), Some(2_000));
    }

    #[test]
    fn a_missing_file_is_absent_but_a_broken_one_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            read_json::<Value>(&dir.path().join("absent.json")).unwrap(),
            None
        );

        let broken = dir.path().join("broken.json");
        fs::write(&broken, b"{ not json").unwrap();
        let error = read_json::<Value>(&broken).unwrap_err();
        assert!(
            matches!(error, StoreError::Corrupt { ref path, .. } if path == &broken),
            "{error}"
        );

        fs::write(&broken, b"[1, 2").unwrap();
        assert!(matches!(
            read_json::<BTreeSet<u8>>(&broken).unwrap_err(),
            StoreError::Corrupt { .. }
        ));
    }

    #[test]
    fn only_windows_sharing_and_lock_failures_are_retried() {
        let denied = std::io::Error::from_raw_os_error(5);
        let sharing = std::io::Error::from_raw_os_error(32);
        let missing = std::io::Error::from_raw_os_error(2);
        if cfg!(windows) {
            assert!(is_retryable_rename(&denied));
            assert!(is_retryable_rename(&sharing));
        } else {
            assert!(!is_retryable_rename(&denied));
            assert!(!is_retryable_rename(&sharing));
        }
        assert!(!is_retryable_rename(&missing));
        assert!(!is_retryable_rename(&std::io::Error::other("no os code")));
        assert_eq!(
            RENAME_RETRY_DELAYS.last(),
            Some(&Duration::from_millis(250))
        );
    }

    #[test]
    fn writing_into_a_missing_leaf_directory_fails_without_creating_a_file() {
        let dir = tempfile::tempdir().unwrap();
        // 父路径上是一个普通文件，目录无法创建。
        let blocker = dir.path().join("blocker");
        fs::write(&blocker, b"x").unwrap();
        let path = blocker.join("state.json");
        assert!(write_private_json(&path, &json!(1)).is_err());
        let names: BTreeSet<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, BTreeSet::from(["blocker".to_owned()]));
    }
}
