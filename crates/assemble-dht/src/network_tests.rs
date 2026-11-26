use super::*;

async fn actor() -> (Actor, mpsc::Receiver<Observation>) {
    let (observed, receiver) = mpsc::channel(16);
    let (peers, _) = broadcast::channel(16);
    (
        Actor {
            socket: UdpSocket::bind("127.0.0.1:0").await.unwrap(),
            id: [1; 20],
            secrets: [[2; 20]; 2],
            rotated: Instant::now(),
            nodes: HashMap::new(),
            pending: HashMap::new(),
            lookups: HashMap::new(),
            candidates: HashMap::new(),
            sample_backoff: HashMap::new(),
            observed,
            peers,
            dropped: Arc::new(AtomicU64::new(0)),
            collecting: true,
            serial: 0,
            next_find: Instant::now(),
            next_sample: Instant::now(),
        },
        receiver,
    )
}

#[test]
fn peer_hints_never_admit_private_or_zero_ports() {
    assert!(compact_peers(&[127, 0, 0, 1, 0, 80]).is_empty());
    assert!(compact_peers(&[8, 8, 8, 8, 0, 0]).is_empty());
    assert_eq!(compact_peers(&[8, 8, 8, 8, 0, 80]).len(), 1);
}

#[tokio::test]
async fn only_matching_sample_transactions_emit_samples_and_respect_interval() {
    let (mut actor, mut observations) = actor().await;
    let source: SocketAddr = "127.0.0.1:12000".parse().unwrap();
    actor.pending.insert(
        9,
        Pending {
            address: source,
            kind: Kind::Sample,
            expires: Instant::now() + Duration::from_secs(10),
        },
    );
    let packet = bencode::encode(&dict([
        ("y", bytes("r")),
        ("t", bytes(9_u32.to_be_bytes())),
        (
            "r",
            dict([
                ("id", bytes([4; 20])),
                ("samples", bytes([3; 20])),
                ("interval", Value::Int(600)),
            ]),
        ),
    ]));
    assert!(
        actor
            .packet(&packet, "127.0.0.2:12000".parse().unwrap())
            .await
            .is_err()
    );
    assert!(observations.try_recv().is_err());
    actor.packet(&packet, source).await.unwrap();
    let Observation::Samples(samples) = observations.try_recv().unwrap() else {
        panic!("sample evidence expected")
    };
    assert_eq!(samples, vec![[3; 20]]);
    assert!(actor.nodes[&source].sample_after > Instant::now() + Duration::from_secs(599));
    let until = actor.sample_backoff[&source];
    actor.nodes.clear();
    actor.pending.insert(
        10,
        Pending {
            address: source,
            kind: Kind::Find,
            expires: Instant::now() + Duration::from_secs(10),
        },
    );
    let response = dict([("y", bytes("r")), ("r", dict([("id", bytes([4; 20]))]))]);
    actor
        .response(&response, &10_u32.to_be_bytes(), source)
        .await
        .unwrap();
    assert_eq!(actor.nodes[&source].sample_after, until);
    assert!(actor.packet(&packet, source).await.is_err());
}

#[tokio::test]
async fn get_peers_is_not_announce_and_bad_tokens_or_ports_do_not_create_evidence() {
    let (mut actor, mut observations) = actor().await;
    let remote = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let source = remote.local_addr().unwrap();
    let get = bencode::encode(&dict([
        ("y", bytes("q")),
        ("t", bytes("1")),
        ("q", bytes("get_peers")),
        (
            "a",
            dict([("id", bytes([4; 20])), ("info_hash", bytes([3; 20]))]),
        ),
    ]));
    actor.packet(&get, source).await.unwrap();
    assert!(observations.try_recv().is_err());
    for (token, port, implied, expected) in [
        (vec![0; 20], 1, 0, false),
        (Actor::token(source, &[2; 20]), 0, 0, false),
        (Actor::token(source, &[2; 20]), 1234, 0, true),
        (Actor::token(source, &[2; 20]), 0, 2, true),
    ] {
        let announce = bencode::encode(&dict([
            ("y", bytes("q")),
            ("t", bytes("2")),
            ("q", bytes("announce_peer")),
            (
                "a",
                dict([
                    ("id", bytes([4; 20])),
                    ("info_hash", bytes([3; 20])),
                    ("port", Value::Int(port)),
                    ("implied_port", Value::Int(implied)),
                    ("token", bytes(token)),
                ]),
            ),
        ]));
        assert_eq!(actor.packet(&announce, source).await.is_ok(), expected);
        assert_eq!(observations.try_recv().is_ok(), expected);
    }
}

#[tokio::test]
async fn direct_announce_candidates_survive_until_the_persisted_job_subscribes() {
    let (mut actor, _) = actor().await;
    for port in 1..10 {
        actor.remember_peer([3; 20], SocketAddr::new("8.8.8.8".parse().unwrap(), port));
    }
    assert_eq!(actor.candidates[&[3; 20]].1.len(), 3);
    actor.remember_peer([4; 20], "127.0.0.1:1".parse().unwrap());
    assert!(!actor.candidates.contains_key(&[4; 20]));
}

#[tokio::test]
async fn explicit_shutdown_releases_udp_socket() {
    let reserve = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let address = reserve.local_addr().unwrap();
    drop(reserve);
    let engine = Engine::start(address, false, Vec::new()).await.unwrap();
    assert!(UdpSocket::bind(address).await.is_err());
    engine.shutdown().await;
    assert!(UdpSocket::bind(address).await.is_ok());
}
