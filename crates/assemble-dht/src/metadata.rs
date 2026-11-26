//! 先核对原始 info 字节的摘要，再解释名称与文件清单；不发布未经验证的文字。
use crate::{
    InfoHash, Invalid,
    bencode::{self, Value},
};
use assemble_protocol::dht::DhtFile;
use sha1::{Digest, Sha1};

pub const MAX_METADATA_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_FILES: usize = 50_000;
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Debug, Clone)]
pub struct Metadata {
    pub name: String,
    pub files: Vec<DhtFile>,
    pub total_size: u64,
    pub piece_length: u64,
    pub private: bool,
    pub comment: Option<String>,
    pub source: Option<String>,
}

fn text(value: &Value, max: usize) -> Result<String, Invalid> {
    let bytes = value.bytes()?;
    if bytes.is_empty() || bytes.len() > max {
        return Err(Invalid);
    }
    let text = String::from_utf8_lossy(bytes).into_owned();
    if text.trim().is_empty() || text.chars().any(char::is_control) {
        return Err(Invalid);
    }
    Ok(text)
}
fn component(value: &Value) -> Result<String, Invalid> {
    let text = text(value, 1024)?;
    if text == "." || text == ".." || text.contains(['/', '\\', ':']) {
        return Err(Invalid);
    }
    Ok(text)
}
fn number(value: &Value) -> Result<u64, Invalid> {
    u64::try_from(value.int()?)
        .ok()
        .filter(|number| *number <= MAX_SAFE_INTEGER)
        .ok_or(Invalid)
}
fn optional_text(info: &Value, key: &str) -> Result<Option<String>, Invalid> {
    info.get(key)
        .ok()
        .map(|value| text(value, 4096))
        .transpose()
}

pub fn parse(raw: &[u8], expected: &InfoHash) -> Result<Metadata, Invalid> {
    if raw.is_empty() || raw.len() > MAX_METADATA_BYTES || Sha1::digest(raw).as_slice() != expected
    {
        return Err(Invalid);
    }
    let info = bencode::decode(raw)?;
    let name_value = info.get("name.utf-8").or_else(|_| info.get("name"))?;
    let name = text(name_value, 4096)?;
    let files = if let Ok(files) = info.get("files") {
        if info.get("length").is_ok() {
            return Err(Invalid);
        }
        let files = files.list()?;
        if files.is_empty() || files.len() > MAX_FILES {
            return Err(Invalid);
        }
        files
            .iter()
            .map(|file| {
                let path = file
                    .get("path.utf-8")
                    .or_else(|_| file.get("path"))?
                    .list()?;
                if path.is_empty() || path.len() > 64 {
                    return Err(Invalid);
                }
                Ok(DhtFile {
                    path: path.iter().map(component).collect::<Result<_, _>>()?,
                    length: number(file.get("length")?)?,
                })
            })
            .collect::<Result<Vec<_>, Invalid>>()?
    } else {
        vec![DhtFile {
            path: vec![component(name_value)?],
            length: number(info.get("length")?)?,
        }]
    };
    let total_size = files.iter().try_fold(0_u64, |sum, file| {
        sum.checked_add(file.length)
            .filter(|sum| *sum <= MAX_SAFE_INTEGER)
            .ok_or(Invalid)
    })?;
    let piece_length = number(info.get("piece length")?)?;
    if piece_length == 0 {
        return Err(Invalid);
    }
    let pieces = info.get("pieces")?.bytes()?;
    if u64::try_from(pieces.len()).map_err(|_| Invalid)?
        != total_size
            .div_ceil(piece_length)
            .checked_mul(20)
            .ok_or(Invalid)?
    {
        return Err(Invalid);
    }
    let private = match info.get("private") {
        Ok(value) => match value.int()? {
            0 => false,
            1 => true,
            _ => return Err(Invalid),
        },
        Err(_) => false,
    };
    Ok(Metadata {
        name,
        files,
        total_size,
        piece_length,
        private,
        comment: optional_text(&info, "comment")?,
        source: optional_text(&info, "source")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bencode::{bytes, dict, encode};
    fn raw(name: &str) -> Vec<u8> {
        encode(&dict([
            ("name", bytes(name)),
            ("length", Value::Int(3)),
            ("piece length", Value::Int(16384)),
            ("pieces", bytes([0; 20])),
        ]))
    }
    #[test]
    fn verifies_hash_sizes_and_paths_before_returning_metadata() {
        let bytes = raw("文件.txt");
        let hash: InfoHash = Sha1::digest(&bytes).into();
        let metadata = parse(&bytes, &hash).unwrap();
        assert_eq!(metadata.total_size, 3);
        assert_eq!(metadata.files[0].path, ["文件.txt"]);
        assert!(parse(&bytes, &[0; 20]).is_err());
        for name in ["../x", "..", "C:\\x", "a\0b"] {
            let bytes = raw(name);
            assert!(parse(&bytes, &Sha1::digest(&bytes).into()).is_err());
        }
        let mut trailing = bytes.clone();
        trailing.push(b'e');
        assert!(parse(&trailing, &Sha1::digest(&trailing).into()).is_err());
    }
}
