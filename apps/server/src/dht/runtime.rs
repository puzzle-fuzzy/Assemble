use super::{WORKERS, store};
use crate::http::Shared;
use assemble_dht::{
    InfoHash, hash_text,
    metadata::Metadata,
    network::{Engine, Handle, Observation},
    wire::{self, Failure},
};
use assemble_protocol::dht::{DhtCounters, DhtMode};
use sqlx::SqlitePool;
use std::{
    collections::{HashSet, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    sync::{Mutex, mpsc},
    task::{JoinHandle, JoinSet},
};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Default)]
pub struct Runtime {
    task: Mutex<Option<JoinHandle<()>>>,
    stop: CancellationToken,
    pub running: AtomicBool,
    pub counters: std::sync::Mutex<DhtCounters>,
    pub last_error: std::sync::Mutex<Option<String>>,
}
impl Runtime {
    pub async fn start(self: &Arc<Self>, state: &Shared) -> Result<(), sqlx::Error> {
        let mut task = self.task.lock().await;
        if task.is_some() {
            return Ok(());
        }
        recover(&state.pool).await?;
        let runtime = Arc::clone(self);
        let state = Arc::clone(state);
        *task = Some(tokio::spawn(async move {
            run(&state, &runtime).await;
        }));
        Ok(())
    }
    pub async fn shutdown(&self) {
        self.stop.cancel();
        if let Some(task) = self.task.lock().await.take() {
            let _ = task.await;
        }
    }
    fn error(&self, code: &str) {
        *self.last_error.lock().expect("DHT error lock") = Some(code.to_owned());
    }
    fn persistence_error(&self) {
        self.counters
            .lock()
            .expect("DHT counters lock")
            .persistence_errors += 1;
        self.error("dht_persistence_failed");
    }
}

async fn recover(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE dht_observations SET status = CASE WHEN attempts < 3 THEN 'pending' ELSE 'failed' END, last_error = 'metadata_interrupted' WHERE status = 'fetching'").execute(pool).await?;
    Ok(())
}

async fn close(
    engine: &mut Option<Engine>,
    tasks: &mut JoinSet<()>,
    state: &Shared,
    runtime: &Runtime,
) {
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    if let Some(engine) = engine.take() {
        engine.shutdown().await;
    }
    runtime.running.store(false, Ordering::Relaxed);
    if recover(&state.pool).await.is_err() {
        runtime.persistence_error();
    }
}

async fn run(state: &Shared, runtime: &Runtime) {
    let mut engine = None;
    let mut tasks = JoinSet::new();
    let mut observed = VecDeque::new();
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut next_start = Instant::now();
    let mut last_mode = DhtMode::Stopped;
    loop {
        tokio::select! {
            () = runtime.stop.cancelled() => break,
            _ = tick.tick() => {},
            Some(result) = tasks.join_next(), if !tasks.is_empty() => { if result.is_err() { runtime.error("metadata_worker_failed"); } continue; }
        }
        let Ok(mode) = store::mode(&state.pool).await else {
            runtime.persistence_error();
            continue;
        };
        if !state.config.dht.enabled || mode == DhtMode::Stopped {
            observed.clear();
            if engine.is_some() {
                close(&mut engine, &mut tasks, state, runtime).await;
            }
            last_mode = mode;
            continue;
        }
        if engine.is_none() && Instant::now() >= next_start {
            if let Ok(started) = Engine::start(
                state.config.dht.listen,
                mode == DhtMode::Collecting,
                state.config.dht.bootstrap_hosts.clone(),
            )
            .await
            {
                engine = Some(started);
                runtime.running.store(true, Ordering::Relaxed);
                *runtime.last_error.lock().expect("DHT error lock") = None;
            } else {
                runtime.error("dht_bind_failed");
                next_start = Instant::now() + Duration::from_secs(30);
            }
        }
        let Some(active) = engine.as_mut() else {
            continue;
        };
        if last_mode != mode {
            active.handle.collect(mode == DhtMode::Collecting).await;
            observed.clear();
            last_mode = mode;
        }
        persist_observations(active, &mut observed, mode, state, runtime).await;
        if store::promote(&state.pool).await.is_err() {
            runtime.persistence_error();
        }
        while tasks.len() < WORKERS {
            let hash = match store::claim(&state.pool).await {
                Ok(Some(hash)) => hash,
                Ok(None) => break,
                Err(_) => {
                    runtime.persistence_error();
                    break;
                }
            };
            let state = Arc::clone(state);
            let handle = active.handle.clone();
            tasks.spawn(async move {
                let result = resolve(hash, handle, &state).await;
                match store::finish(&state.pool, &hash, result).await {
                    Ok(true) => {
                        state
                            .dht
                            .counters
                            .lock()
                            .expect("DHT counters lock")
                            .verified += 1;
                    }
                    Ok(false) => {}
                    Err(_) => state.dht.persistence_error(),
                }
            });
        }
    }
    close(&mut engine, &mut tasks, state, runtime).await;
}

async fn persist_observations(
    active: &mut Engine,
    observed: &mut VecDeque<(InfoHash, bool)>,
    mode: DhtMode,
    state: &Shared,
    runtime: &Runtime,
) {
    for _ in 0..16 {
        let Ok(event) = active.observations.try_recv() else {
            break;
        };
        if mode != DhtMode::Collecting {
            continue;
        }
        let entries = match event {
            Observation::Announce(hash) => vec![(hash, true)],
            Observation::Samples(hashes) => hashes.into_iter().map(|hash| (hash, false)).collect(),
        };
        for entry in entries {
            if observed.len() < 2048 {
                observed.push_back(entry);
            } else {
                runtime
                    .counters
                    .lock()
                    .expect("DHT counters lock")
                    .dropped_events += 1;
            }
        }
    }
    runtime
        .counters
        .lock()
        .expect("DHT counters lock")
        .dropped_events += active.dropped.swap(0, Ordering::Relaxed);
    for _ in 0..32 {
        let Some((hash, announce)) = observed.pop_front() else {
            break;
        };
        match store::observe(
            &state.pool,
            &hash_text(&hash),
            announce,
            state.config.dht.max_records,
        )
        .await
        {
            Ok(true) => {}
            Ok(false) => {
                runtime
                    .counters
                    .lock()
                    .expect("DHT counters lock")
                    .dropped_events += 1;
            }
            Err(_) => runtime.persistence_error(),
        }
    }
}

async fn resolve(
    hash: InfoHash,
    handle: Handle,
    state: &Shared,
) -> Result<Option<Metadata>, Failure> {
    let mut peers = handle.subscribe();
    handle.lookup(hash).await.map_err(|_| Failure::Connection)?;
    state
        .dht
        .counters
        .lock()
        .expect("DHT counters lock")
        .lookups += 1;
    let (hints, mut hinted) = mpsc::channel(16);
    let mut tried = HashSet::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    let mut last = None;
    loop {
        let candidate = tokio::select! {
            () = tokio::time::sleep_until(deadline) => break,
            Some(address) = hinted.recv() => Some(address),
            event = peers.recv() => match event { Ok(peer) if peer.hash == hash => Some(peer.address), Err(tokio::sync::broadcast::error::RecvError::Closed) => break, _ => None }
        };
        let Some(candidate) = candidate else {
            continue;
        };
        if !tried.insert(candidate) {
            continue;
        }
        {
            let mut counters = state.dht.counters.lock().expect("DHT counters lock");
            if tried.len() == 1 {
                counters.lookups_with_peers += 1;
            }
            counters.fetch_attempts += 1;
        }
        let fetched = tokio::time::timeout_at(
            deadline,
            wire::fetch(
                hash,
                candidate,
                state.config.dht.peer_exchange,
                hints.clone(),
            ),
        )
        .await
        .unwrap_or(Err(Failure::Timeout));
        match fetched {
            Ok(metadata) => return Ok(Some(metadata)),
            Err(error) => {
                let mut counters = state.dht.counters.lock().expect("DHT counters lock");
                match error {
                    Failure::Timeout => counters.timeouts += 1,
                    Failure::Connection => counters.connection_errors += 1,
                    Failure::Validation => counters.validation_errors += 1,
                    _ => counters.protocol_errors += 1,
                }
                last = Some(error);
            }
        }
        if tried.len() >= 3 {
            break;
        }
    }
    last.map_or(Ok(None), Err)
}
