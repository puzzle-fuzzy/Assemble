//! DHT 协议与元数据边界，不依赖窗口、HTTP 或数据库。
#![cfg_attr(test, allow(clippy::pedantic))]

pub mod bencode;
pub mod metadata;
pub mod network;
pub mod wire;

use std::net::{IpAddr, SocketAddr};

pub type InfoHash = [u8; 20];

#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("invalid DHT input")]
pub struct Invalid;

pub fn parse_hash(value: &str) -> Result<InfoHash, Invalid> {
    let value = value.trim();
    if value.len() > 4096 {
        return Err(Invalid);
    }
    if value.starts_with("magnet:") {
        let uri = url::Url::parse(value).map_err(|_| Invalid)?;
        let mut hashes = uri.query_pairs().filter_map(|(key, value)| {
            (key == "xt")
                .then(|| value.strip_prefix("urn:btih:").map(str::to_owned))
                .flatten()
        });
        let hash = parse_hash(&hashes.next().ok_or(Invalid)?)?;
        if hashes.any(|other| parse_hash(&other).ok() != Some(hash)) {
            return Err(Invalid);
        }
        return Ok(hash);
    }
    let mut hash = [0; 20];
    if value.len() == 40 && value.is_ascii() {
        for (index, pair) in value.as_bytes().as_chunks::<2>().0.iter().enumerate() {
            let digit = |b: u8| char::from(b).to_digit(16).ok_or(Invalid);
            hash[index] =
                u8::try_from(digit(pair[0])? * 16 + digit(pair[1])?).map_err(|_| Invalid)?;
        }
    } else if value.len() == 32 && value.is_ascii() {
        let mut buffer = 0_u32;
        let mut bits = 0;
        let mut index = 0;
        for byte in value.bytes() {
            let digit = match byte.to_ascii_uppercase() {
                b'A'..=b'Z' => byte.to_ascii_uppercase() - b'A',
                b'2'..=b'7' => byte - b'2' + 26,
                _ => return Err(Invalid),
            };
            buffer = (buffer << 5) | u32::from(digit);
            bits += 5;
            if bits >= 8 {
                bits -= 8;
                hash[index] = u8::try_from((buffer >> bits) & 255).map_err(|_| Invalid)?;
                index += 1;
            }
        }
    } else {
        return Err(Invalid);
    }
    Ok(hash)
}

pub fn hash_text(hash: &InfoHash) -> String {
    use std::fmt::Write;
    hash.iter()
        .fold(String::with_capacity(40), |mut text, byte| {
            write!(&mut text, "{byte:02x}").expect("writing a string cannot fail");
            text
        })
}

pub fn magnet(hash: &str, name: Option<&str>) -> String {
    let mut uri = format!("magnet:?xt=urn:btih:{hash}");
    if let Some(name) = name {
        uri.push('&');
        uri.push_str(
            &url::form_urlencoded::Serializer::new(String::new())
                .append_pair("dn", name)
                .finish(),
        );
    }
    uri
}

/// 不解析网络提供的主机名；映射的 IPv4 仍走同一份公网检查。
pub fn public_address(addr: SocketAddr) -> bool {
    if addr.port() == 0 {
        return false;
    }
    match addr.ip() {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            a != 0
                && a != 10
                && a != 127
                && a < 224
                && !(a == 100 && (64..=127).contains(&b))
                && !(a == 169 && b == 254)
                && !(a == 172 && (16..=31).contains(&b))
                && !(a == 192 && (b == 0 || b == 168 || (b == 88 && c == 99)))
                && !(a == 198 && ((18..=19).contains(&b) || (b == 51 && c == 100)))
                && !(a == 203 && b == 0 && c == 113)
        }
        IpAddr::V6(ip) => {
            if let Some(ip) = ip.to_ipv4_mapped() {
                return public_address(SocketAddr::new(ip.into(), addr.port()));
            }
            let words = ip.segments();
            words[0] & 0xe000 == 0x2000
                && !(words[0] == 0x2001 && (words[1] < 0x0200 || words[1] == 0x0db8))
                && words[0] != 0x2002
                && !(words[0] == 0x3fff && words[1] & 0xf000 == 0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hashes_magnets_and_public_boundary() {
        assert_eq!(
            parse_hash("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").unwrap(),
            [0; 20]
        );
        let hash = "0123456789012345678901234567890123456789";
        assert_eq!(
            hash_text(&parse_hash(&magnet(hash, Some("测试"))).unwrap()),
            hash
        );
        assert!(parse_hash("a").is_err());
        for address in [
            "127.0.0.1:1",
            "10.0.0.1:1",
            "[::ffff:192.168.1.1]:1",
            "[::1]:1",
            "198.51.100.1:1",
        ] {
            assert!(!public_address(address.parse().unwrap()));
        }
        assert!(public_address("8.8.8.8:6881".parse().unwrap()));
    }
}
