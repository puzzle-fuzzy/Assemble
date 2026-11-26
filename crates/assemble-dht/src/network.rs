//! 单个 UDP 节点，固定 RPC、路由、采样和事件预算；地址只在运行内存中存在。
use crate::{
    InfoHash, Invalid,
    bencode::{self, Value, bytes, dict},
    public_address,
};
use sha1::{Digest, Sha1};
use std::{
    collections::{HashMap, HashSet},
    net::{Ipv4Addr, SocketAddr},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    net::UdpSocket,
    sync::{broadcast, mpsc},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

const RPC_LIMIT: usize = 16;
const NODE_LIMIT: usize = 5000;

#[derive(Debug)]
pub enum Observation {
    Announce(InfoHash),
    Samples(Vec<InfoHash>),
}
#[derive(Debug, Clone, Copy)]
pub struct Peer {
    pub hash: InfoHash,
    pub address: SocketAddr,
}
enum Command {
    Lookup(InfoHash),
    Collect(bool),
}

#[derive(Clone)]
pub struct Handle {
    commands: mpsc::Sender<Command>,
    peers: broadcast::Sender<Peer>,
}
impl Handle {
    pub async fn lookup(&self, hash: InfoHash) -> Result<(), Invalid> {
        self.commands
            .send(Command::Lookup(hash))
            .await
            .map_err(|_| Invalid)
    }
    pub fn subscribe(&self) -> broadcast::Receiver<Peer> {
        self.peers.subscribe()
    }
    pub async fn collect(&self, enabled: bool) {
        let _ = self.commands.send(Command::Collect(enabled)).await;
    }
}

pub struct Engine {
    pub handle: Handle,
    pub observations: mpsc::Receiver<Observation>,
    pub dropped: Arc<AtomicU64>,
    stop: CancellationToken,
    task: JoinHandle<()>,
}
impl Engine {
    pub async fn start(
        listen: SocketAddr,
        collecting: bool,
        bootstrap_hosts: Vec<String>,
    ) -> Result<Self, std::io::Error> {
        let socket = UdpSocket::bind(listen).await?;
        let (commands, receiver) = mpsc::channel(64);
        let (observed, observations) = mpsc::channel(1024);
        let (peers, _) = broadcast::channel(256);
        let stop = CancellationToken::new();
        let dropped = Arc::new(AtomicU64::new(0));
        let mut id = [0; 20];
        let mut secret = [0; 20];
        getrandom::fill(&mut id).map_err(std::io::Error::other)?;
        getrandom::fill(&mut secret).map_err(std::io::Error::other)?;
        let actor = Actor {
            socket,
            id,
            secrets: [secret, secret],
            rotated: Instant::now(),
            nodes: HashMap::new(),
            pending: HashMap::new(),
            lookups: HashMap::new(),
            candidates: HashMap::new(),
            sample_backoff: HashMap::new(),
            observed,
            peers: peers.clone(),
            dropped: Arc::clone(&dropped),
            collecting,
            serial: u32::from_be_bytes(secret[..4].try_into().expect("four random bytes")),
            next_find: Instant::now(),
            next_sample: Instant::now(),
        };
        let task = tokio::spawn(actor.run(receiver, stop.clone(), bootstrap_hosts));
        Ok(Self {
            handle: Handle { commands, peers },
            observations,
            stop,
            task,
            dropped,
        })
    }
    pub async fn shutdown(self) {
        self.stop.cancel();
        let _ = self.task.await;
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Find,
    Sample,
    Peers(InfoHash),
}
struct Pending {
    address: SocketAddr,
    kind: Kind,
    expires: Instant,
}
struct Node {
    id: InfoHash,
    seen: Instant,
    sample_after: Instant,
}
struct Lookup {
    visited: HashSet<SocketAddr>,
    expires: Instant,
}
struct Actor {
    socket: UdpSocket,
    id: InfoHash,
    secrets: [InfoHash; 2],
    rotated: Instant,
    nodes: HashMap<SocketAddr, Node>,
    pending: HashMap<u32, Pending>,
    lookups: HashMap<InfoHash, Lookup>,
    candidates: HashMap<InfoHash, (Instant, Vec<SocketAddr>)>,
    // 路由节点可提前淘汰，远端要求的采样间隔不能随它一起遗忘。
    sample_backoff: HashMap<SocketAddr, Instant>,
    observed: mpsc::Sender<Observation>,
    peers: broadcast::Sender<Peer>,
    dropped: Arc<AtomicU64>,
    collecting: bool,
    serial: u32,
    next_find: Instant,
    next_sample: Instant,
}

impl Actor {
    async fn run(
        mut self,
        mut commands: mpsc::Receiver<Command>,
        stop: CancellationToken,
        bootstrap_hosts: Vec<String>,
    ) {
        let mut bootstrap = tokio::task::JoinSet::new();
        seed(&mut bootstrap, &bootstrap_hosts);
        let mut next_bootstrap = Instant::now() + Duration::from_mins(1);
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut buffer = [0; 8192];
        loop {
            tokio::select! {
                () = stop.cancelled() => break,
                Some(result) = bootstrap.join_next(), if !bootstrap.is_empty() => {
                    if let Ok(addresses) = result { for address in addresses { self.query(address, Kind::Find).await; } }
                }
                command = commands.recv() => match command {
                    Some(Command::Collect(enabled)) => self.collecting = enabled,
                    Some(Command::Lookup(hash)) => {
                        if let Some((_, addresses)) = self.candidates.get(&hash) { for address in addresses { let _ = self.peers.send(Peer { hash, address: *address }); } }
                        if self.lookups.len() < 32 && !self.lookups.contains_key(&hash) {
                            self.lookups.insert(hash, Lookup { visited: HashSet::new(), expires: Instant::now() + Duration::from_secs(25) });
                            let nodes = self.nearest(&hash, 8);
                            for address in nodes { self.query(address, Kind::Peers(hash)).await; }
                        }
                    }
                    None => break,
                },
                _ = tick.tick() => {
                    if self.nodes.is_empty() && bootstrap.is_empty() && Instant::now() >= next_bootstrap { seed(&mut bootstrap, &bootstrap_hosts); next_bootstrap = Instant::now() + Duration::from_mins(1); }
                    self.tick().await;
                },
                result = self.socket.recv_from(&mut buffer) => {
                    if let Ok((length, address)) = result
                        && public_address(address) { let _ = self.packet(&buffer[..length], address).await; }
                }
            }
        }
        bootstrap.abort_all();
        while bootstrap.join_next().await.is_some() {}
    }

    fn nearest(&self, target: &InfoHash, count: usize) -> Vec<SocketAddr> {
        let mut nodes: Vec<_> = self.nodes.iter().collect();
        nodes.sort_unstable_by_key(|(_, node)| {
            let mut distance = [0; 20];
            for index in 0..20 {
                distance[index] = node.id[index] ^ target[index];
            }
            distance
        });
        nodes
            .into_iter()
            .take(count)
            .map(|(address, _)| *address)
            .collect()
    }

    async fn query(&mut self, address: SocketAddr, kind: Kind) {
        if self.pending.len() >= RPC_LIMIT || !public_address(address) {
            return;
        }
        if self.pending.values().any(|pending| {
            pending.address == address
                && matches!(
                    (pending.kind, kind),
                    (Kind::Find, Kind::Find) | (Kind::Sample, Kind::Sample)
                )
        }) {
            return;
        }
        if let Kind::Peers(hash) = kind {
            let Some(lookup) = self.lookups.get_mut(&hash) else {
                return;
            };
            if lookup.visited.len() >= 24 || !lookup.visited.insert(address) {
                return;
            }
        }
        self.serial = self.serial.wrapping_add(1);
        let (method, target, hash) = match kind {
            Kind::Find => {
                let mut target = self.id;
                if getrandom::fill(&mut target).is_err() {
                    return;
                }
                ("find_node", target, false)
            }
            Kind::Sample => ("sample_infohashes", self.id, false),
            Kind::Peers(hash) => ("get_peers", hash, true),
        };
        let arguments = dict([
            ("id", bytes(self.id)),
            (if hash { "info_hash" } else { "target" }, bytes(target)),
        ]);
        let packet = bencode::encode(&dict([
            ("t", bytes(self.serial.to_be_bytes())),
            ("y", bytes("q")),
            ("q", bytes(method)),
            ("a", arguments),
        ]));
        if self.socket.send_to(&packet, address).await.is_ok() {
            self.pending.insert(
                self.serial,
                Pending {
                    address,
                    kind,
                    expires: Instant::now() + Duration::from_secs(10),
                },
            );
        }
    }

    async fn tick(&mut self) {
        let now = Instant::now();
        self.pending.retain(|_, pending| pending.expires > now);
        self.lookups.retain(|_, lookup| lookup.expires > now);
        self.sample_backoff.retain(|_, until| *until > now);
        self.candidates
            .retain(|_, (seen, _)| now.duration_since(*seen) < Duration::from_mins(1));
        self.nodes
            .retain(|_, node| now.duration_since(node.seen) < Duration::from_mins(15));
        if now.duration_since(self.rotated) >= Duration::from_secs(300) {
            let mut secret = [0; 20];
            if getrandom::fill(&mut secret).is_ok() {
                self.secrets = [secret, self.secrets[0]];
                self.rotated = now;
            }
        }
        if now >= self.next_find {
            self.next_find = now + Duration::from_secs(1);
            if let Some(address) = self
                .nodes
                .keys()
                .copied()
                .nth(usize::try_from(self.serial).unwrap_or(0) % self.nodes.len().max(1))
            {
                self.query(address, Kind::Find).await;
            }
            // 等待 RPC 预算的查找继续推进，不能因第一次队列满而永久饿死。
            let hashes: Vec<_> = self.lookups.keys().copied().collect();
            for hash in hashes {
                for address in self.nearest(&hash, 8) {
                    self.query(address, Kind::Peers(hash)).await;
                }
            }
        }
        if self.collecting && now >= self.next_sample && self.sample_backoff.len() < NODE_LIMIT {
            self.next_sample = now + Duration::from_secs(5);
            if let Some(address) = self
                .nodes
                .iter()
                .filter(|(address, node)| {
                    node.sample_after <= now && !self.sample_backoff.contains_key(address)
                })
                .min_by_key(|(_, node)| node.sample_after)
                .map(|(address, _)| *address)
            {
                self.query(address, Kind::Sample).await;
                if self.pending.values().any(|pending| {
                    pending.address == address && matches!(pending.kind, Kind::Sample)
                }) {
                    let until = now + Duration::from_hours(6);
                    self.sample_backoff.insert(address, until);
                    if let Some(node) = self.nodes.get_mut(&address) {
                        node.sample_after = until;
                    }
                }
            }
        }
    }

    fn token(address: SocketAddr, secret: &InfoHash) -> Vec<u8> {
        let mut hasher = Sha1::new();
        hasher.update(secret);
        match address.ip() {
            std::net::IpAddr::V4(ip) => hasher.update(ip.octets()),
            std::net::IpAddr::V6(ip) => hasher.update(ip.octets()),
        }
        hasher.finalize().to_vec()
    }
    fn emit(&self, event: Observation) {
        if self.observed.try_send(event).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn remember_peer(&mut self, hash: InfoHash, address: SocketAddr) {
        if !public_address(address) {
            return;
        }
        if self.candidates.len() >= 2000
            && !self.candidates.contains_key(&hash)
            && let Some(oldest) = self
                .candidates
                .iter()
                .min_by_key(|(_, (seen, _))| *seen)
                .map(|(hash, _)| *hash)
        {
            self.candidates.remove(&oldest);
        }
        let (seen, peers) = self
            .candidates
            .entry(hash)
            .or_insert_with(|| (Instant::now(), Vec::new()));
        *seen = Instant::now();
        if peers.len() < 3 && !peers.contains(&address) {
            peers.push(address);
        }
        let _ = self.peers.send(Peer { hash, address });
    }

    async fn packet(&mut self, packet: &[u8], address: SocketAddr) -> Result<(), Invalid> {
        let value = bencode::decode(packet)?;
        let transaction = value.get("t")?.bytes()?;
        match value.get("y")?.bytes()? {
            b"q" => self.request(&value, transaction, address).await,
            b"r" | b"e" => self.response(&value, transaction, address).await,
            _ => Err(Invalid),
        }
    }
    async fn request(
        &mut self,
        value: &Value,
        transaction: &[u8],
        address: SocketAddr,
    ) -> Result<(), Invalid> {
        if transaction.len() > 8 {
            return Err(Invalid);
        }
        let arguments = value.get("a")?;
        let _: InfoHash = arguments
            .get("id")?
            .bytes()?
            .try_into()
            .map_err(|_| Invalid)?;
        let response = match value.get("q")?.bytes()? {
            b"ping" => dict([("id", bytes(self.id))]),
            b"find_node" => {
                let _: InfoHash = arguments
                    .get("target")?
                    .bytes()?
                    .try_into()
                    .map_err(|_| Invalid)?;
                dict([("id", bytes(self.id)), ("nodes", bytes([]))])
            }
            b"get_peers" => {
                let _: InfoHash = arguments
                    .get("info_hash")?
                    .bytes()?
                    .try_into()
                    .map_err(|_| Invalid)?;
                dict([
                    ("id", bytes(self.id)),
                    ("token", bytes(Self::token(address, &self.secrets[0]))),
                    ("nodes", bytes([])),
                ])
            }
            b"announce_peer" => {
                let hash: InfoHash = arguments
                    .get("info_hash")?
                    .bytes()?
                    .try_into()
                    .map_err(|_| Invalid)?;
                let token = arguments.get("token")?.bytes()?;
                if !self
                    .secrets
                    .iter()
                    .any(|secret| Self::token(address, secret) == token)
                {
                    return Err(Invalid);
                }
                let port = if arguments
                    .get("implied_port")
                    .and_then(Value::int)
                    .unwrap_or(0)
                    != 0
                {
                    address.port()
                } else {
                    u16::try_from(arguments.get("port")?.int()?).map_err(|_| Invalid)?
                };
                if port == 0 {
                    return Err(Invalid);
                }
                if self.collecting {
                    self.emit(Observation::Announce(hash));
                }
                self.remember_peer(hash, SocketAddr::new(address.ip(), port));
                dict([("id", bytes(self.id))])
            }
            _ => return Err(Invalid),
        };
        let packet = bencode::encode(&dict([
            ("t", bytes(transaction)),
            ("y", bytes("r")),
            ("r", response),
        ]));
        let _ = self.socket.send_to(&packet, address).await;

        Ok(())
    }
    async fn response(
        &mut self,
        value: &Value,
        transaction: &[u8],
        address: SocketAddr,
    ) -> Result<(), Invalid> {
        let tid = u32::from_be_bytes(transaction.try_into().map_err(|_| Invalid)?);
        let pending = self.pending.get(&tid).ok_or(Invalid)?;
        if pending.address != address || pending.expires < Instant::now() {
            return Err(Invalid);
        }
        let pending = self.pending.remove(&tid).ok_or(Invalid)?;
        if value.get("y")?.bytes()? == b"e" {
            return Ok(());
        }
        let response = value.get("r")?;
        let id: InfoHash = response
            .get("id")?
            .bytes()?
            .try_into()
            .map_err(|_| Invalid)?;
        if self.nodes.len() >= NODE_LIMIT
            && !self.nodes.contains_key(&address)
            && let Some(oldest) = self
                .nodes
                .iter()
                .min_by_key(|(_, node)| node.seen)
                .map(|(address, _)| *address)
        {
            self.nodes.remove(&oldest);
        }
        let node = self.nodes.entry(address).or_insert(Node {
            id,
            seen: Instant::now(),
            sample_after: self
                .sample_backoff
                .get(&address)
                .copied()
                .unwrap_or_else(Instant::now),
        });
        node.id = id;
        node.seen = Instant::now();
        if matches!(pending.kind, Kind::Sample)
            && let Ok(samples) = response.get("samples").and_then(Value::bytes)
        {
            if !samples.len().is_multiple_of(20) || samples.len() > 5120 {
                return Err(Invalid);
            }
            let interval = response.get("interval")?.int()?;
            let interval = u64::try_from(interval)
                .ok()
                .filter(|value| *value <= 21_600)
                .ok_or(Invalid)?;
            node.sample_after = Instant::now() + Duration::from_secs(interval.max(300));
            self.sample_backoff.insert(address, node.sample_after);
            if self.collecting {
                self.emit(Observation::Samples(samples.as_chunks::<20>().0.to_vec()));
            }
        }
        if let Kind::Peers(hash) = pending.kind
            && let Ok(values) = response.get("values").and_then(Value::list)
        {
            for value in values.iter().take(32) {
                if let Ok(bytes) = value.bytes() {
                    for peer in compact_peers(bytes) {
                        self.remember_peer(hash, peer);
                    }
                }
            }
        }

        self.follow_nodes(response, pending.kind).await;

        Ok(())
    }
    async fn follow_nodes(&mut self, response: &Value, kind: Kind) {
        if let Ok(nodes) = response.get("nodes").and_then(Value::bytes) {
            for node in nodes.as_chunks::<26>().0.iter().take(8) {
                let addr = SocketAddr::new(
                    Ipv4Addr::new(node[20], node[21], node[22], node[23]).into(),
                    u16::from_be_bytes([node[24], node[25]]),
                );
                if !self.nodes.contains_key(&addr) || matches!(kind, Kind::Peers(_)) {
                    self.query(
                        addr,
                        if let Kind::Peers(hash) = kind {
                            Kind::Peers(hash)
                        } else {
                            Kind::Find
                        },
                    )
                    .await;
                }
            }
        }
    }
}

fn seed(bootstrap: &mut tokio::task::JoinSet<Vec<SocketAddr>>, bootstrap_hosts: &[String]) {
    for host in bootstrap_hosts {
        let host = host.clone();
        bootstrap.spawn(async move {
            tokio::time::timeout(
                Duration::from_secs(5),
                tokio::net::lookup_host(host.as_str()),
            )
            .await
            .ok()
            .and_then(Result::ok)
            .map_or_default(|addresses| {
                addresses
                    .filter(|address| address.is_ipv4() && public_address(*address))
                    .take(2)
                    .collect::<Vec<_>>()
            })
        });
    }
}

pub fn compact_peers(bytes: &[u8]) -> Vec<SocketAddr> {
    if !bytes.len().is_multiple_of(6) {
        return Vec::new();
    }
    bytes
        .as_chunks::<6>()
        .0
        .iter()
        .take(32)
        .map(|peer| {
            SocketAddr::new(
                Ipv4Addr::new(peer[0], peer[1], peer[2], peer[3]).into(),
                u16::from_be_bytes([peer[4], peer[5]]),
            )
        })
        .filter(|address| public_address(*address))
        .collect()
}

#[cfg(test)]
#[path = "network_tests.rs"]
mod tests;
