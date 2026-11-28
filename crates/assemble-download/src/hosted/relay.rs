//! 进程内 UDP Tracker 只把本任务的安全 DHT 结果持续交给 BT 引擎，不对外发布端口。
use super::Discovery;
use crate::{DownloadError, Result};
use assemble_dht::InfoHash;
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::{net::UdpSocket, task::JoinHandle};
use tokio_util::sync::CancellationToken;

pub(crate) struct PeerRelay {
    pub url: String,
    stop: CancellationToken,
    worker: JoinHandle<()>,
}
struct Context {
    hash: InfoHash,
    connection: [u8; 8],
    peers: Vec<SocketAddr>,
    local_test: bool,
}
impl Context {
    fn remember(&mut self, peer: assemble_dht::network::Peer) -> bool {
        if peer.hash != self.hash
            || (!self.local_test && !assemble_dht::public_address(peer.address))
            || self.peers.contains(&peer.address)
        {
            return false;
        }
        if self.peers.len() >= 64 {
            self.peers.remove(0);
        }
        self.peers.push(peer.address);
        true
    }
    fn reply(&self, request: &[u8]) -> Option<Vec<u8>> {
        if request.len() < 16 {
            return None;
        }
        let transaction = &request[12..16];
        match &request[8..12] {
            [0, 0, 0, 0]
                if request.len() == 16 && request[..8] == 0x0417_2710_1980_u64.to_be_bytes() =>
            {
                let mut reply = vec![0; 4];
                reply.extend_from_slice(transaction);
                reply.extend_from_slice(&self.connection);
                Some(reply)
            }
            [0, 0, 0, 1]
                if request.len() >= 98
                    && request[..8] == self.connection
                    && request[16..36] == self.hash =>
            {
                let mut reply = 1_u32.to_be_bytes().to_vec();
                reply.extend_from_slice(transaction);
                reply.extend_from_slice(&10_u32.to_be_bytes());
                reply.extend_from_slice(&[0; 8]);
                for peer in &self.peers {
                    if let SocketAddr::V4(peer) = peer {
                        reply.extend_from_slice(&peer.ip().octets());
                        reply.extend_from_slice(&peer.port().to_be_bytes());
                    }
                }
                Some(reply)
            }
            _ => {
                let mut reply = 3_u32.to_be_bytes().to_vec();
                reply.extend_from_slice(transaction);
                reply.extend_from_slice(b"invalid request");
                Some(reply)
            }
        }
    }
}
impl PeerRelay {
    pub async fn open(
        discovery: Arc<Discovery>,
        hash: InfoHash,
        local_test: bool,
        download: CancellationToken,
    ) -> Result<Self> {
        let socket = UdpSocket::bind("127.0.0.1:0")
            .await
            .map_err(|_| DownloadError::Network)?;
        let address = socket.local_addr().map_err(|_| DownloadError::Network)?;
        let mut connection = [0; 8];
        getrandom::fill(&mut connection).map_err(|_| DownloadError::Network)?;
        let context = Context {
            hash,
            connection,
            peers: discovery.known(hash),
            local_test,
        };
        let stop = CancellationToken::new();
        let worker = tokio::spawn(run(socket, context, discovery, stop.clone(), download));
        Ok(Self {
            url: format!("udp://{address}/announce"),
            stop,
            worker,
        })
    }
    pub async fn shutdown(self) {
        self.stop.cancel();
        let _ = self.worker.await;
    }
}
async fn run(
    socket: UdpSocket,
    mut context: Context,
    discovery: Arc<Discovery>,
    stop: CancellationToken,
    download: CancellationToken,
) {
    let mut incoming = discovery.subscribe();
    let mut query = tokio::time::interval(Duration::from_secs(15));
    query.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut packet = [0; 2048];
    loop {
        tokio::select! {
            ()=stop.cancelled()=>break,
            _=query.tick(),if !download.is_cancelled()=>{tokio::select!{()=stop.cancelled()=>break,()=download.cancelled()=>{},()=discovery.lookup(context.hash)=>{}}},
            peer=incoming.recv()=>match peer {
                Ok(peer) if !download.is_cancelled()=>{if context.remember(peer){discovery.remember(context.hash,context.peers.clone());}},
                Err(tokio::sync::broadcast::error::RecvError::Closed)=>break,
                _=>{}
            },
            request=socket.recv_from(&mut packet)=>{
                let Ok((length,sender))=request else {break;};
                if sender.ip().is_loopback() && let Some(reply)=context.reply(&packet[..length]) {let _=socket.send_to(&reply,sender).await;}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn receive(socket: &UdpSocket, buffer: &mut [u8]) -> usize {
        tokio::time::timeout(Duration::from_secs(5), socket.recv(buffer))
            .await
            .unwrap()
            .unwrap()
    }
    #[tokio::test]
    async fn loopback_tracker_binds_hash_and_nonce_and_stops_explicitly() {
        let dir = tempfile::tempdir().unwrap();
        let discovery = Discovery::for_test(dir.path(), Vec::new()).await.unwrap();
        let hash = [7; 20];
        let relay = PeerRelay::open(Arc::clone(&discovery), hash, true, CancellationToken::new())
            .await
            .unwrap();
        let port = url::Url::parse(&relay.url).unwrap().port().unwrap();
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        socket
            .connect((std::net::Ipv4Addr::LOCALHOST, port))
            .await
            .unwrap();
        let mut connect = 0x0417_2710_1980_u64.to_be_bytes().to_vec();
        connect.extend_from_slice(&[0; 4]);
        connect.extend_from_slice(&42_u32.to_be_bytes());
        socket.send(&connect).await.unwrap();
        let mut buffer = [0; 2048];
        let length = receive(&socket, &mut buffer).await;
        assert_eq!(length, 16);
        assert_eq!(buffer[4..8], 42_u32.to_be_bytes());
        let mut announce = vec![0; 98];
        announce[..8].copy_from_slice(&buffer[8..16]);
        announce[8..12].copy_from_slice(&1_u32.to_be_bytes());
        announce[12..16].copy_from_slice(&43_u32.to_be_bytes());
        announce[16..36].copy_from_slice(&hash);
        socket.send(&announce).await.unwrap();
        let length = receive(&socket, &mut buffer).await;
        assert_eq!(length, 20);
        assert_eq!(buffer[..4], 1_u32.to_be_bytes());
        announce[16] = 8;
        socket.send(&announce).await.unwrap();
        receive(&socket, &mut buffer).await;
        assert_eq!(buffer[..4], 3_u32.to_be_bytes());
        announce[16] = 7;
        announce[0] ^= 1;
        socket.send(&announce).await.unwrap();
        receive(&socket, &mut buffer).await;
        assert_eq!(buffer[..4], 3_u32.to_be_bytes());
        relay.shutdown().await;
        assert!(
            UdpSocket::bind((std::net::Ipv4Addr::LOCALHOST, port))
                .await
                .is_ok()
        );
        discovery.shutdown().await;
    }
}
