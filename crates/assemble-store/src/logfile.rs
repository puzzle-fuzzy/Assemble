//! 限大小的本机日志文件。
//!
//! 桌面应用挂在托盘里一跑就是几天，日志不能只写标准错误：从启动台或托盘启动的进程
//! 没有可见的输出，出了事无从查起。所以把同样的内容另写一份到文件。文件要有上限，
//! 不然一直开着的应用会把它写个没完：写到一定大小，就把当前这份转成"上一份"，
//! 只留一份旧的。
//!
//! 日志里本来就不记凭据（见各处的调用），这是写日志的人要守的规矩，这里不管；
//! 这个模块管的只是文件本身仅本人可读写，以及它占的地方有上限。

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::json::create_private_dir;
#[cfg(unix)]
use crate::json::set_private_file;

/// 一份最多写这么多字节，超过了就转成上一份。一兆够记下好几天的正常运行；
/// 真出了在循环里刷日志的事，两份加起来也不会把磁盘写满。
pub const MAX_BYTES: u64 = 1024 * 1024;

/// 一份限大小的日志：当前文件，加上至多一份上一份。
#[derive(Debug)]
pub struct LogFile {
    path: PathBuf,
    file: File,
    written: u64,
    limit: u64,
}

impl LogFile {
    /// 在 `directory` 里接着写名叫 `name` 的文件。目录不存在就建。
    pub fn open(directory: &Path, name: &str) -> io::Result<Self> {
        Self::within(directory, name, MAX_BYTES)
    }

    fn within(directory: &Path, name: &str, limit: u64) -> io::Result<Self> {
        create_private_dir(directory)?;
        let path = directory.join(name);
        let mut file = open_private(&path)?;
        let mut written = file.metadata()?.len();
        // 上次运行留下的这份已经写满：接着写就会超限，先转走。
        if written >= limit {
            drop(file);
            file = set_aside(&path)?;
            written = 0;
        }
        Ok(Self {
            path,
            file,
            written,
            limit,
        })
    }

    /// 写一段。会让当前这份超过上限时，先把它转成上一份，再从头写。
    pub fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        let length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if self.written + length > self.limit {
            // 开着的文件照样可以改名：标准库打开文件时允许它被改名或删除，Windows 上也是。
            // 旧句柄下面被换掉时才关闭，之后不会再有东西写进上一份。
            self.file.flush()?;
            fs::rename(&self.path, previous_of(&self.path))?;
            self.file = open_private(&self.path)?;
            self.written = 0;
        }
        self.file.write_all(bytes)?;
        self.written += length;
        Ok(())
    }

    /// 这份日志现在占着哪些文件、各有多大，旧的在前。给自检和文档用。
    pub fn files(&self) -> Vec<(PathBuf, u64)> {
        let mut listed = Vec::new();
        if let Ok(previous) = previous_of(&self.path).metadata() {
            listed.push((previous_of(&self.path), previous.len()));
        }
        listed.push((self.path.clone(), self.written));
        listed
    }
}

/// 当前这份改名为上一份，顶替原来那一份；再开一份新的空文件。
fn set_aside(path: &Path) -> io::Result<File> {
    fs::rename(path, previous_of(path))?;
    open_private(path)
}

/// 以追加方式打开，没有就建；在类 Unix 上还把它改成本人可读写。
fn open_private(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new().append(true).create(true).open(path)?;
    // "仅本人可读写"是类 Unix 上的文件权限位；Windows 没有对应的东西可设。
    // 这个调用只在类 Unix 上存在：给 Windows 造一个永远成功的空壳，静态检查会拦。
    #[cfg(unix)]
    set_private_file(path)?;
    Ok(file)
}

/// 上一份的名字：在原名后面加 `.1`。
fn previous_of(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map_or_else(|| std::ffi::OsString::from("log"), ToOwned::to_owned);
    name.push(".1");
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_are_appended_and_survive_reopening() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = LogFile::open(dir.path(), "app.log").unwrap();
        log.write(b"first line\n").unwrap();
        drop(log);
        let mut log = LogFile::open(dir.path(), "app.log").unwrap();
        log.write(b"second line\n").unwrap();
        drop(log);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("app.log")).unwrap(),
            "first line\nsecond line\n"
        );
        // 还没转过，就没有上一份。
        assert!(!dir.path().join("app.log.1").exists());
    }

    #[test]
    fn a_full_file_is_set_aside_and_nothing_is_lost() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = LogFile::within(dir.path(), "app.log", 30).unwrap();
        let mut everything = String::new();
        for index in 0..20 {
            let line = format!("line {index:02}\n");
            log.write(line.as_bytes()).unwrap();
            everything.push_str(&line);
            // 任何时刻：当前这份不超过上限，两份加起来不丢字。
            assert!(
                std::fs::metadata(dir.path().join("app.log")).unwrap().len() <= 30,
                "after line {index}"
            );
            let kept = std::fs::read_to_string(dir.path().join("app.log.1")).unwrap_or_default()
                + &std::fs::read_to_string(dir.path().join("app.log")).unwrap();
            assert!(everything.ends_with(&kept), "after line {index}");
        }
        // 只留一份旧的：最早的那几行被转出去了。
        let kept = std::fs::read_to_string(dir.path().join("app.log.1")).unwrap()
            + &std::fs::read_to_string(dir.path().join("app.log")).unwrap();
        assert!(kept.len() < everything.len());
        assert_eq!(kept, everything[everything.len() - kept.len()..]);
        assert_eq!(log.files().len(), 2);
    }

    #[test]
    fn a_file_left_full_by_the_previous_run_is_set_aside_on_open() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("app.log"), "x".repeat(40)).unwrap();
        let mut log = LogFile::within(dir.path(), "app.log", 30).unwrap();
        log.write(b"fresh\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("app.log")).unwrap(),
            "fresh\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("app.log.1")).unwrap(),
            "x".repeat(40)
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_log_is_readable_by_its_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let directory = dir.path().join("logs");
        let mut log = LogFile::open(&directory, "app.log").unwrap();
        log.write(b"hello\n").unwrap();
        drop(log);
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&directory), 0o700);
        assert_eq!(mode(&directory.join("app.log")), 0o600);
    }

    #[test]
    fn the_previous_copy_replaces_the_one_before_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = LogFile::within(dir.path(), "app.log", 10).unwrap();
        log.write(b"0123456789\n").unwrap();
        log.write(b"abcdefghij\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("app.log.1")).unwrap(),
            "0123456789\n"
        );
        log.write(b"klmnopqrst\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("app.log.1")).unwrap(),
            "abcdefghij\n"
        );
    }
}
