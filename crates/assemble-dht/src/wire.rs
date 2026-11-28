//! BEP-9/10 的 info-only 请求；参考旧 Observer/Spider 的帧与分片规则。
use crate::{
    InfoHash,
    bencode::{self, Value, dict},
    metadata::{self, MAX_METADATA_BYTES, Metadata},
    public_address,
};
use std::{net::SocketAddr, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::mpsc,
};

const BLOCK: usize = 16_384;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Failure {
    #[error("metadata_timeout")]
    Timeout,
    #[error("metadata_connection")]
    Connection,
    #[error("metadata_protocol")]
    Protocol,
    #[error("metadata_invalid")]
    Validation,
    #[error("metadata_unsupported")]
    Unsupported,
    #[error("metadata_rejected")]
    Rejected,
}
impl From<std::io::Error> for Failure {
    fn from(_: std::io::Error) -> Self {
        Self::Connection
    }
}
impl From<crate::Invalid> for Failure {
    fn from(_: crate::Invalid) -> Self {
        Self::Protocol
    }
}

pub async fn fetch(
    hash: InfoHash,
    peer: SocketAddr,
    pex: bool,
    hints: mpsc::Sender<SocketAddr>,
) -> Result<Metadata, Failure> {
    fetch_raw(hash, peer, pex, hints)
        .await
        .map(|(metadata, _)| metadata)
}

/// 托管下载复用原始、已校验的 info；观测接口仍只返回安全投影。
pub async fn fetch_raw(
    hash: InfoHash,
    peer: SocketAddr,
    pex: bool,
    hints: mpsc::Sender<SocketAddr>,
) -> Result<(Metadata, Vec<u8>), Failure> {
    if !public_address(peer) {
        return Err(Failure::Connection);
    }
    tokio::time::timeout(Duration::from_secs(15), async {
        let mut stream = TcpStream::connect(peer).await?;
        exchange_raw(&mut stream, hash, pex, &hints).await
    })
    .await
    .map_err(|_| Failure::Timeout)?
}

async fn send(stream: &mut TcpStream, extension: u8, value: &Value) -> Result<(), Failure> {
    let mut frame = vec![20, extension];
    frame.extend_from_slice(&bencode::encode(value));
    stream
        .write_all(
            &u32::try_from(frame.len())
                .map_err(|_| Failure::Protocol)?
                .to_be_bytes(),
        )
        .await?;
    stream.write_all(&frame).await?;
    Ok(())
}
async fn request(stream: &mut TcpStream, extension: u8, piece: usize) -> Result<(), Failure> {
    send(
        stream,
        extension,
        &dict([
            ("msg_type", Value::Int(0)),
            (
                "piece",
                Value::Int(i64::try_from(piece).map_err(|_| Failure::Protocol)?),
            ),
        ]),
    )
    .await
}

#[cfg(test)]
async fn exchange(
    stream: &mut TcpStream,
    hash: InfoHash,
    pex: bool,
    hints: &mpsc::Sender<SocketAddr>,
) -> Result<Metadata, Failure> {
    exchange_raw(stream, hash, pex, hints)
        .await
        .map(|(metadata, _)| metadata)
}

async fn exchange_raw(
    stream: &mut TcpStream,
    hash: InfoHash,
    pex: bool,
    hints: &mpsc::Sender<SocketAddr>,
) -> Result<(Metadata, Vec<u8>), Failure> {
    handshake(stream, hash, pex).await?;
    let mut extension = None;
    let mut size = None;
    let mut pipelined = false;
    let mut pieces = vec![Vec::new(); MAX_METADATA_BYTES / BLOCK];
    let mut hinted = 0;
    let mut frames = 0;
    loop {
        frames += 1;
        if frames > 4096 {
            return Err(Failure::Protocol);
        }
        let length = stream.read_u32().await?;
        if length == 0 {
            continue;
        }
        if length > 1_048_576 {
            return Err(Failure::Protocol);
        }
        let mut frame = vec![0; usize::try_from(length).map_err(|_| Failure::Protocol)?];
        stream.read_exact(&mut frame).await?;
        if frame[0] != 20 {
            continue;
        }
        if frame.len() < 2 {
            return Err(Failure::Protocol);
        }
        let rest = &frame[2..];
        if frame[1] == 0 {
            start_metadata(stream, rest, &mut extension, &mut size).await?;
            pipelined = size.is_some();
        } else if pex && frame[1] == 2 {
            accept_hints(rest, hints, &mut hinted)?;
        } else if frame[1] == 1 {
            let id = extension.ok_or(Failure::Protocol)?;
            let (header, offset) = bencode::prefix(rest)?;
            if !accept_message(stream, id, &header).await? {
                continue;
            }
            let piece =
                usize::try_from(header.get("piece")?.int()?).map_err(|_| Failure::Protocol)?;
            if piece >= pieces.len() {
                return Err(Failure::Protocol);
            }
            if let Ok(total) = header.get("total_size") {
                let declared = metadata_size(total.int()?)?;
                if size.is_some_and(|size| size != declared) {
                    return Err(Failure::Protocol);
                }
                size = Some(declared);
            }
            let data = &rest[offset..];
            let expected = if let Some(size) = size {
                if piece >= size.div_ceil(BLOCK) {
                    return Err(Failure::Protocol);
                }
                BLOCK.min(size - piece * BLOCK)
            } else {
                BLOCK
            };
            if data.is_empty()
                || data.len() > expected
                || (size.is_some() && data.len() != expected)
            {
                return Err(Failure::Protocol);
            }
            if !pieces[piece].is_empty() && pieces[piece] != data {
                return Err(Failure::Protocol);
            }
            pieces[piece] = data.to_vec();
            let contiguous = pieces.iter().take_while(|piece| !piece.is_empty()).count();
            let complete = size.is_some_and(|size| contiguous == size.div_ceil(BLOCK))
                || (size.is_none() && contiguous == piece + 1 && data.len() < BLOCK);
            if complete {
                let raw = pieces[..contiguous].concat();
                return metadata::parse(&raw, &hash)
                    .map(|metadata| (metadata, raw))
                    .map_err(|_| Failure::Validation);
            }
            // 省略 size 的 peer 也只能在固定分片与总字节预算内串行请求。
            if contiguous == piece + 1 {
                if size.is_none() {
                    let raw = pieces[..contiguous].concat();
                    if bencode::decode(&raw).is_ok() {
                        return metadata::parse(&raw, &hash)
                            .map(|metadata| (metadata, raw))
                            .map_err(|_| Failure::Validation);
                    }
                }
                if contiguous >= pieces.len() {
                    return Err(Failure::Protocol);
                }
                if !pipelined {
                    request(stream, id, contiguous).await?;
                }
            }
        }
    }
}
async fn accept_message(
    stream: &mut TcpStream,
    extension: u8,
    header: &Value,
) -> Result<bool, Failure> {
    match header.get("msg_type")?.int()? {
        1 => Ok(true),
        2 => Err(Failure::Rejected),
        0 => {
            let piece = header.get("piece")?.int()?;
            if !(0..512).contains(&piece) {
                return Err(Failure::Protocol);
            }
            // 尚未取得并验证完整 info，不能向远端提供任何元数据分片。
            send(
                stream,
                extension,
                &dict([("msg_type", Value::Int(2)), ("piece", Value::Int(piece))]),
            )
            .await?;
            Ok(false)
        }
        _ => Ok(false),
    }
}

fn accept_hints(
    rest: &[u8],
    hints: &mpsc::Sender<SocketAddr>,
    hinted: &mut usize,
) -> Result<(), Failure> {
    let value = bencode::decode(rest)?;
    if let Ok(added) = value.get("added").and_then(Value::bytes) {
        for peer in crate::network::compact_peers(added).into_iter().take(16) {
            if *hinted >= 32 {
                break;
            }
            *hinted += 1;
            let _ = hints.try_send(peer);
        }
    }
    Ok(())
}
async fn handshake(stream: &mut TcpStream, hash: InfoHash, pex: bool) -> Result<(), Failure> {
    let mut handshake = [0; 68];
    handshake[0] = 19;
    handshake[1..20].copy_from_slice(b"BitTorrent protocol");
    handshake[25] = 16;
    handshake[28..48].copy_from_slice(&hash);
    getrandom::fill(&mut handshake[48..]).map_err(|_| Failure::Connection)?;
    stream.write_all(&handshake).await?;
    let mut remote = [0; 68];
    stream.read_exact(&mut remote).await?;
    if remote[0..20] != handshake[0..20] || remote[28..48] != hash {
        return Err(Failure::Protocol);
    }
    if remote[25] & 16 == 0 {
        return Err(Failure::Unsupported);
    }
    let mut extensions = vec![("ut_metadata", Value::Int(1))];
    if pex {
        extensions.push(("ut_pex", Value::Int(2)));
    }
    send(stream, 0, &dict([("m", dict(extensions))])).await?;
    Ok(())
}
async fn start_metadata(
    stream: &mut TcpStream,
    rest: &[u8],
    extension: &mut Option<u8>,
    size: &mut Option<usize>,
) -> Result<(), Failure> {
    if extension.is_some() {
        return Err(Failure::Protocol);
    }
    let header = bencode::decode(rest)?;
    let id = header
        .get("m")
        .and_then(|map| map.get("ut_metadata"))
        .and_then(Value::int)
        .unwrap_or(0);
    if id == 0 {
        return Err(Failure::Unsupported);
    }
    let id = u8::try_from(id).map_err(|_| Failure::Protocol)?;
    *extension = Some(id);
    if let Ok(value) = header.get("metadata_size") {
        *size = Some(metadata_size(value.int()?)?);
    }
    let count = size.map_or(1, |size| size.div_ceil(BLOCK));
    for piece in 0..count {
        request(stream, id, piece).await?;
    }
    Ok(())
}
fn metadata_size(value: i64) -> Result<usize, Failure> {
    usize::try_from(value)
        .ok()
        .filter(|size| (1..=MAX_METADATA_BYTES).contains(size))
        .ok_or(Failure::Protocol)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bencode::bytes;
    use sha1::{Digest, Sha1};
    #[tokio::test]
    async fn real_tcp_exchange_verifies_metadata_and_rejects_wrong_hash() {
        for corrupt in [false, true] {
            let raw = bencode::encode(&dict([
                ("name", bytes("sample.txt")),
                ("length", Value::Int(1)),
                ("piece length", Value::Int(16384)),
                ("pieces", bytes([0; 20])),
            ]));
            let hash: InfoHash = Sha1::digest(&raw).into();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut handshake = [0; 68];
                stream.read_exact(&mut handshake).await.unwrap();
                stream.write_all(&handshake).await.unwrap();
                let length = stream.read_u32().await.unwrap();
                let mut frame = vec![0; length as usize];
                stream.read_exact(&mut frame).await.unwrap();
                send(
                    &mut stream,
                    0,
                    &dict([
                        ("m", dict([("ut_metadata", Value::Int(7))])),
                        ("metadata_size", Value::Int(raw.len() as i64)),
                    ]),
                )
                .await
                .unwrap();
                let length = stream.read_u32().await.unwrap();
                let mut frame = vec![0; length as usize];
                stream.read_exact(&mut frame).await.unwrap();
                assert_eq!(frame[1], 7);
                send(
                    &mut stream,
                    1,
                    &dict([("msg_type", Value::Int(0)), ("piece", Value::Int(0))]),
                )
                .await
                .unwrap();
                let length = stream.read_u32().await.unwrap();
                let mut rejected = vec![0; length as usize];
                stream.read_exact(&mut rejected).await.unwrap();
                assert_eq!(rejected[1], 7);
                assert_eq!(
                    bencode::decode(&rejected[2..])
                        .unwrap()
                        .get("msg_type")
                        .unwrap()
                        .int()
                        .unwrap(),
                    2
                );
                let mut frame = vec![20, 1];
                frame.extend(bencode::encode(&dict([
                    ("msg_type", Value::Int(1)),
                    ("piece", Value::Int(0)),
                    ("total_size", Value::Int(raw.len() as i64)),
                ])));
                let mut raw = raw;
                if corrupt {
                    raw[10] ^= 1;
                }
                frame.extend(raw);
                stream
                    .write_all(&(frame.len() as u32).to_be_bytes())
                    .await
                    .unwrap();
                stream.write_all(&frame).await.unwrap();
            });
            let (tx, _rx) = mpsc::channel(16);
            let mut stream = TcpStream::connect(address).await.unwrap();
            let result = tokio::time::timeout(
                Duration::from_secs(5),
                exchange(&mut stream, hash, false, &tx),
            )
            .await
            .unwrap();
            assert_eq!(result.is_ok(), !corrupt);
            server.await.unwrap();
        }
    }
}
