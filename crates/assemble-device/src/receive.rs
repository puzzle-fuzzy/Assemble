//! 收到的文件落在接收目录里的规则。
//!
//! 接收目录由用户在设置里选，默认是"下载/Assemble"。目录是用户亲自指定的，
//! 所以它上层的链接不算越界；真正要防的是**文件名**：它来自网络，
//! 可能带着路径、Windows 不认的字符、设备保留名，或者长得放不进文件系统。

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// 文件名字节的预算。给序号后缀 ` (999)` 留出余量之后，仍低于常见文件系统
/// 每段 255 的上限；按 UTF-8 字节算，三种桌面平台都不会超。
const NAME_BUDGET_BYTES: usize = 240;

/// 收敛不出可用的名字时用它。
const FALLBACK_NAME: &str = "file";

/// 名字里一律替换掉的字：Windows 不认的那几个，加上 macOS 访达显示成斜杠的冒号。
fn is_forbidden(character: char) -> bool {
    character.is_control() || matches!(character, '<' | '>' | '"' | '|' | '?' | '*' | ':')
}

/// Windows 保留的设备名，带不带扩展名都算。
fn is_windows_reserved(stem: &str) -> bool {
    let lower = stem.to_lowercase();
    let Some(rest) = lower
        .strip_prefix("com")
        .or_else(|| lower.strip_prefix("lpt"))
    else {
        return matches!(lower.as_str(), "con" | "prn" | "aux" | "nul");
    };
    matches!(rest, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
}

/// 扩展名超过这个长度就不当扩展名看了：正常文件的扩展名没有这么长的。
const MAX_EXTENSION_BYTES: usize = 64;

/// 把扩展名与主干分开。开头的点不算扩展名（`.profile` 整个是主干）；
/// `.tar.gz` 这类压缩包的双扩展名算一个整体，加序号时不会把它拆开。
fn split_extension(name: &str) -> (&str, &str) {
    let Some(index) = name.rfind('.') else {
        return (name, "");
    };
    if index == 0 || name.len() - index > MAX_EXTENSION_BYTES {
        return (name, "");
    }
    // `then_some` 会先算出参数，这里必须用 `then`：`index` 小于 4 时不能做减法。
    let is_tar_second_part = Path::new(&name[..index])
        .extension()
        .is_some_and(|part| part.eq_ignore_ascii_case("tar"));
    let start = if is_tar_second_part { index - 4 } else { index };
    (&name[..start], &name[start..])
}

/// 按字节预算截断主干，不切开字符；截断之后结尾不留点和空格。
fn truncate_stem(stem: &str, budget: usize) -> &str {
    if stem.len() <= budget {
        return stem;
    }
    let mut end = budget;
    while !stem.is_char_boundary(end) {
        end -= 1;
    }
    stem[..end].trim_end_matches(['.', ' '])
}

/// 把网络上来的一个名字收敛成单个、三种桌面平台都能用的文件名。
///
/// 永远返回一个非空的名字：收敛不出东西时给一个兜底的名字，而不是拒绝接收——
/// 名字不好看，总比文件丢了好。
pub fn safe_file_name(raw: &str) -> String {
    // 只认最后一段：名字里万一带着路径，也不能落到接收目录之外。
    let last = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let replaced: String = last
        .chars()
        .map(|character| {
            if is_forbidden(character) {
                '_'
            } else {
                character
            }
        })
        .collect();
    // Windows 会悄悄去掉结尾的点和空格，"a." 与 "a" 成了同一个文件；这里先去掉。
    let trimmed = replaced
        .trim_end_matches(['.', ' '])
        .trim_start_matches(' ');
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        return FALLBACK_NAME.to_owned();
    }

    let (stem, extension) = split_extension(trimmed);
    let stem = truncate_stem(stem, NAME_BUDGET_BYTES.saturating_sub(extension.len()));
    let mut safe = String::with_capacity(stem.len() + extension.len() + 1);
    if is_windows_reserved(stem) {
        safe.push('_');
    }
    safe.push_str(stem);
    safe.push_str(extension);
    safe
}

/// 同名文件已存在时用的名字：`报告 (2).pdf`。
///
/// `base` 必须已经是 [`safe_file_name`] 的结果。
pub fn numbered_name(base: &str, number: u32) -> String {
    let (stem, extension) = split_extension(base);
    format!("{stem} ({number}){extension}")
}

/// 在接收目录里给一个文件找一个还没被占用的名字。
///
/// 先用原名；被占用就依次试 ` (2)`、` (3)`……这只是一次预选：两个文件同时
/// 到达时仍可能撞名，真正的不覆盖由发布那一步（硬链接）保证。
pub async fn choose_file_name(directory: &Path, base: &str) -> std::io::Result<String> {
    let mut candidate = base.to_owned();
    let mut number = 1u32;
    loop {
        match tokio::fs::symlink_metadata(directory.join(&candidate)).await {
            // 这个名字没有任何东西占着。
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(candidate),
            Err(error) => return Err(error),
            Ok(_) => {}
        }
        number = number
            .checked_add(1)
            .ok_or_else(|| std::io::Error::other("too many files share this name"))?;
        candidate = numbered_name(base, number);
    }
}

/// 接收目录在接口里的失败原因。
#[derive(Debug, thiserror::Error)]
pub enum BoundaryError {
    #[error("the receive directory is not a directory")]
    NotDirectory,
    #[error("a whole disk cannot be the receive directory")]
    DiskRoot,
    #[error("receive directory could not be used: {0}")]
    Io(#[from] std::io::Error),
}

/// 确认一个路径能当接收目录，并给出它的规范形式。
///
/// 必须是已经存在的目录，且不是磁盘根目录。用户选的可以是一个链接，
/// 这里返回它实际指向的位置；之后所有落盘都用返回的这条路径。
pub async fn validate_directory(path: &Path) -> Result<PathBuf, BoundaryError> {
    let info = tokio::fs::metadata(path).await?;
    if !info.is_dir() {
        return Err(BoundaryError::NotDirectory);
    }
    let canonical = tokio::fs::canonicalize(path).await?;
    if canonical.parent().is_none() {
        return Err(BoundaryError::DiskRoot);
    }
    Ok(canonical)
}

/// 默认接收目录在"下载"之下的名字。
pub const DEFAULT_FOLDER: &str = "Assemble";

/// 默认接收目录的位置："下载/Assemble"。不保证它已经存在。
pub fn default_directory(downloads: &Path) -> PathBuf {
    downloads.join(DEFAULT_FOLDER)
}

/// 准备默认的接收目录："下载/Assemble"。不存在就建出来。
pub async fn ensure_default_directory(downloads: &Path) -> Result<PathBuf, BoundaryError> {
    let directory = default_directory(downloads);
    match tokio::fs::create_dir_all(&directory).await {
        Ok(()) => {}
        // 两个进程同时启动时另一个可能已经建好；是不是目录由下一步确认。
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    validate_directory(&directory).await
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    /// macOS 的临时目录本身经过链接，先取规范路径再用。
    fn directory() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let canonical = fs::canonicalize(dir.path()).unwrap();
        (dir, canonical)
    }

    #[test]
    fn ordinary_names_are_kept_as_they_are() {
        assert_eq!(safe_file_name("合同.pdf"), "合同.pdf");
        assert_eq!(safe_file_name("  周报.docx  "), "周报.docx");
        assert_eq!(safe_file_name(".gitignore"), ".gitignore");
        assert_eq!(safe_file_name("a.b.c"), "a.b.c");
        assert_eq!(safe_file_name("报告 (2).pdf"), "报告 (2).pdf");
        assert_eq!(safe_file_name("console.txt"), "console.txt");
        assert_eq!(safe_file_name("COM10.txt"), "COM10.txt");
    }

    #[test]
    fn path_parts_never_survive_a_file_name() {
        assert_eq!(safe_file_name("a/b/c.txt"), "c.txt");
        assert_eq!(safe_file_name("C:\\Users\\me\\file.txt"), "file.txt");
        assert_eq!(safe_file_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_file_name("..\\..\\boot.ini"), "boot.ini");
        assert_eq!(safe_file_name("dir/"), FALLBACK_NAME);
        assert_eq!(safe_file_name("/"), FALLBACK_NAME);
    }

    #[test]
    fn characters_windows_refuses_are_replaced() {
        assert_eq!(safe_file_name("a<b>c:d|e?f*h.txt"), "a_b_c_d_e_f_h.txt");
        assert_eq!(safe_file_name("line\nbreak\ttab.txt"), "line_break_tab.txt");
        assert_eq!(safe_file_name("12:30 录音.m4a"), "12_30 录音.m4a");
        assert_eq!(safe_file_name("a\0b"), "a_b");
        // 结尾的点和空格去掉；开头的点保留。
        assert_eq!(safe_file_name("trailing... "), "trailing");
        assert_eq!(safe_file_name(" .hidden"), ".hidden");
    }

    #[test]
    fn names_with_nothing_left_get_a_fallback() {
        for empty in ["", "   ", ".", "..", "...", "...."] {
            assert_eq!(safe_file_name(empty), FALLBACK_NAME, "{empty:?}");
        }
        assert_eq!(FALLBACK_NAME, "file");
    }

    #[test]
    fn windows_device_names_are_prefixed() {
        assert_eq!(safe_file_name("CON"), "_CON");
        assert_eq!(safe_file_name("con.txt"), "_con.txt");
        assert_eq!(safe_file_name("LPT1.log"), "_LPT1.log");
        assert_eq!(safe_file_name("nul"), "_nul");
        assert_eq!(safe_file_name("PrN.pdf"), "_PrN.pdf");
    }

    #[test]
    fn long_names_are_cut_to_the_budget_without_losing_the_extension() {
        let long = format!("{}.tar.gz", "字".repeat(300));
        let cut = safe_file_name(&long);
        assert!(cut.ends_with(".tar.gz"), "{cut}");
        assert!(cut.len() <= NAME_BUDGET_BYTES, "{}", cut.len());
        // 加上序号之后仍然放得进每段 255 的文件系统。
        let numbered = numbered_name(&cut, 9999);
        assert!(numbered.len() <= 255, "{}", numbered.len());
        assert!(numbered.ends_with(".tar.gz"));

        // 没有扩展名时只截主干。
        let plain = safe_file_name(&"a".repeat(300));
        assert_eq!(plain.len(), NAME_BUDGET_BYTES);
        let ascii = safe_file_name(&format!("{}.txt", "x".repeat(300)));
        assert_eq!(ascii.len(), NAME_BUDGET_BYTES);
        assert!(ascii.ends_with(".txt"));
    }

    #[test]
    fn numbered_names_keep_the_extension_and_hidden_stems() {
        assert_eq!(numbered_name("a.txt", 2), "a (2).txt");
        assert_eq!(numbered_name("a", 2), "a (2)");
        assert_eq!(numbered_name(".profile", 2), ".profile (2)");
        assert_eq!(numbered_name("报告.tar.gz", 3), "报告 (3).tar.gz");
        assert_eq!(numbered_name("报告.TAR.GZ", 3), "报告 (3).TAR.GZ");
        // 以点开头的 `.tar.gz` 是一个隐藏文件，`.tar` 是它的主干。
        assert_eq!(numbered_name(".tar.gz", 3), ".tar (3).gz");
        // 扩展名长得不像扩展名时，整个名字当主干，序号加在最后。
        let odd = format!("a.{}", "x".repeat(MAX_EXTENSION_BYTES + 1));
        assert_eq!(numbered_name(&odd, 2), format!("{odd} (2)"));
        // 截断不会把结尾留成点或空格。
        let padded = format!("{}{} .txt", "a".repeat(100), "b".repeat(300));
        let cut = safe_file_name(&padded);
        assert!(cut.ends_with("a.txt") || cut.ends_with("b.txt"), "{cut}");
        assert!(!cut.starts_with(' '));
    }

    #[tokio::test]
    async fn a_free_name_is_used_and_taken_names_are_numbered() {
        let (_dir, root) = directory();
        assert_eq!(choose_file_name(&root, "a.txt").await.unwrap(), "a.txt");

        fs::write(root.join("a.txt"), b"first").unwrap();
        assert_eq!(choose_file_name(&root, "a.txt").await.unwrap(), "a (2).txt");

        fs::write(root.join("a (2).txt"), b"second").unwrap();
        assert_eq!(choose_file_name(&root, "a.txt").await.unwrap(), "a (3).txt");

        // 目录占着这个名字也算占用。
        fs::create_dir(root.join("a (3).txt")).unwrap();
        assert_eq!(choose_file_name(&root, "a.txt").await.unwrap(), "a (4).txt");
        // 其它名字不受影响。
        assert_eq!(choose_file_name(&root, "b.txt").await.unwrap(), "b.txt");
    }

    #[tokio::test]
    async fn the_receive_directory_must_be_a_real_directory() {
        let (_dir, root) = directory();
        assert_eq!(validate_directory(&root).await.unwrap(), root);

        fs::write(root.join("file.txt"), b"x").unwrap();
        assert!(matches!(
            validate_directory(&root.join("file.txt")).await,
            Err(BoundaryError::NotDirectory)
        ));
        assert!(matches!(
            validate_directory(&root.join("missing")).await,
            Err(BoundaryError::Io(_))
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_whole_disk_is_not_a_receive_directory() {
        assert!(matches!(
            validate_directory(Path::new("/")).await,
            Err(BoundaryError::DiskRoot)
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_linked_receive_directory_resolves_to_its_real_location() {
        let (dir, root) = directory();
        // 目标建在临时目录之外，规范路径才与字面路径不同。
        let target = dir.path().join("elsewhere");
        fs::create_dir(&target).unwrap();
        std::os::unix::fs::symlink(&target, root.join("alias")).unwrap();

        let resolved = validate_directory(&root.join("alias")).await.unwrap();
        assert_eq!(resolved, fs::canonicalize(&target).unwrap());
        assert_ne!(resolved, root.join("alias"));
    }

    #[tokio::test]
    async fn the_default_directory_is_created_under_downloads() {
        let (dir, root) = directory();
        let downloads = root.join("Downloads");
        let created = ensure_default_directory(&downloads).await.unwrap();
        assert_eq!(
            created,
            fs::canonicalize(downloads.join("Assemble")).unwrap()
        );
        // 再来一次：已经存在也没关系。
        assert_eq!(ensure_default_directory(&downloads).await.unwrap(), created);
        // 下载目录的位置被一个文件占着，就建不出来。
        let blocked = root.join("blocked");
        fs::write(&blocked, b"x").unwrap();
        assert!(ensure_default_directory(&blocked).await.is_err());
        // `Assemble` 这个名字被一个文件占着，同样不能用。
        let occupied = root.join("occupied");
        fs::create_dir(&occupied).unwrap();
        fs::write(occupied.join("Assemble"), b"x").unwrap();
        assert!(matches!(
            ensure_default_directory(&occupied).await,
            Err(BoundaryError::NotDirectory)
        ));
        drop(dir);
    }
}
