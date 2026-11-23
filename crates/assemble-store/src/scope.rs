//! 分段隔离用的文件名片段。
//!
//! 本机数据按服务来源与账户分开保存。文件名里用摘要而不是原文，
//! 既不泄露来源和账户，也避免路径里的特殊字符。

use sha2::{Digest, Sha256};

/// 若干分段值（例如服务来源与账户，未登录时账户为空）对应的文件名片段：
/// 把它们当作 JSON 数组做 SHA-256，取小写十六进制。
pub fn scope_digest(parts: &[&str]) -> String {
    // `&[&str]` 的 JSON 序列化不会失败；万一失败也只会得到可区分的空串摘要。
    let encoded = serde_json::to_string(parts).unwrap_or_default();
    digest(encoded.as_bytes())
}

/// 单个文本的 SHA-256 小写十六进制。
pub fn digest_hex(value: &str) -> String {
    digest(value.as_bytes())
}

/// 字节的 SHA-256 小写十六进制。
pub fn digest(bytes: &[u8]) -> String {
    assemble_protocol::primitives::to_hex(&Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGIN: &str = "https://jarvis.example";

    #[test]
    fn scope_digest_hashes_the_parts_as_a_json_array() {
        // 与 `sha256(JSON.stringify([origin, accountId]))` 等价。
        assert_eq!(
            scope_digest(&[ORIGIN, "account-1"]),
            digest_hex(r#"["https://jarvis.example","account-1"]"#)
        );
        assert_eq!(
            scope_digest(&[ORIGIN, ""]),
            digest_hex(r#"["https://jarvis.example",""]"#)
        );
        assert_eq!(scope_digest(&[]), digest_hex("[]"));
        // 非 ASCII 原样进入摘要，与 JSON.stringify 一致。
        assert_eq!(scope_digest(&["来源"]), digest_hex(r#"["来源"]"#));
        // 与 `shasum -a 256` 对同一 JSON 数组的结果逐位一致。
        assert_eq!(
            scope_digest(&[ORIGIN, "account-1"]),
            "549fe7d72a511a5f235a7bcce3c79b33bc9dcfa18e3dac2a63b5ea34fda09211"
        );
        assert_eq!(
            scope_digest(&[ORIGIN, ""]),
            "5b12ef092c020bd6afc3b8b4b7ef20dffc8cd2396fcdade20dcd2de1423141af"
        );
    }

    #[test]
    fn digests_are_stable_lowercase_hex() {
        assert_eq!(digest_hex("abc"), digest("abc".as_bytes()));
        assert_eq!(
            digest_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            digest(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let value = digest_hex("中文");
        assert_eq!(value.len(), 64);
        assert!(
            value
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        );
        assert_ne!(digest_hex("abd"), digest_hex("abc"));
    }

    #[test]
    fn every_scope_boundary_produces_a_different_name() {
        let base = scope_digest(&[ORIGIN, "account-1"]);
        assert_ne!(base, scope_digest(&[ORIGIN, "account-2"]));
        assert_ne!(base, scope_digest(&[ORIGIN, ""]));
        assert_ne!(base, scope_digest(&["http://127.0.0.1:3300", "account-1"]));
        // 分段的边界本身有意义：`["a", "bc"]` 与 `["ab", "c"]` 不能落到同一个文件。
        assert_ne!(scope_digest(&["a", "bc"]), scope_digest(&["ab", "c"]));
        assert_ne!(scope_digest(&[ORIGIN]), scope_digest(&[ORIGIN, ""]));
    }
}
