//! 边读边算的 SHA-256。

use assemble_protocol::primitives::to_hex;
use sha2::{Digest, Sha256};

/// 一次性计算一段字节的 SHA-256，返回小写十六进制。
pub fn digest(bytes: &[u8]) -> String {
    to_hex(&Sha256::digest(bytes))
}

/// 增量摘要：按到达的顺序喂入字节，最后取结果。
#[derive(Debug, Default)]
pub struct Sha256Sink {
    state: Sha256,
}

impl Sha256Sink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update(&mut self, bytes: &[u8]) {
        self.state.update(bytes);
    }

    /// 取出结果；之后的调用将从一个全新的摘要开始。
    pub fn finish(&mut self) -> String {
        let state = std::mem::take(&mut self.state);
        to_hex(&state.finalize())
    }
}

/// 大小写无关地比较两个十六进制摘要；期望值本身必须是合法摘要。
pub fn digests_match(expected: &str, actual: &str) -> bool {
    expected.len() == 64
        && actual.len() == 64
        && expected
            .bytes()
            .zip(actual.bytes())
            .all(|(left, right)| left.eq_ignore_ascii_case(&right))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CHUNK_BYTES;

    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn known_vectors_match_the_reference_digests() {
        assert_eq!(digest(b"abc"), ABC);
        assert_eq!(
            digest(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            digest(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn incremental_hashing_equals_a_single_pass() {
        let payload: Vec<u8> = (0..200_000u32).map(|value| value as u8).collect();
        let mut sink = Sha256Sink::new();
        for chunk in payload.chunks(CHUNK_BYTES / 3) {
            sink.update(chunk);
        }
        assert_eq!(sink.finish(), digest(&payload));
        // 取过结果后从零开始，不会把上一份内容带进来。
        sink.update(b"abc");
        assert_eq!(sink.finish(), ABC);
    }

    #[test]
    fn digest_comparison_ignores_case_but_nothing_else() {
        assert!(digests_match(ABC, ABC));
        assert!(digests_match(&ABC.to_uppercase(), ABC));
        assert!(!digests_match(&ABC[..63], &ABC[..63]));
        assert!(!digests_match(ABC, &ABC.replace('a', "b")));
        assert!(!digests_match("", ""));
    }
}
