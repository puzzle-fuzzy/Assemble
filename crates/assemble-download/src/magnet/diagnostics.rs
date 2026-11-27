//! 手工公网验收只收集固定分类的计数，不输出节点、来源或协议载荷。
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tracing::{
    Event, Subscriber,
    field::{Field, Visit},
    span::Attributes,
};
use tracing_subscriber::{Layer, layer::Context, prelude::*};

#[derive(Clone, Default)]
pub(super) struct Counters {
    counts: Arc<Mutex<BTreeMap<&'static str, u64>>>,
    active: Arc<AtomicBool>,
}
impl Counters {
    pub fn start() -> Option<Self> {
        if std::env::var("ASSEMBLE_BT_DIAGNOSTICS").as_deref() != Ok("1") {
            return None;
        }
        let this = Self::default();
        this.active.store(true, Ordering::Relaxed);
        tracing_subscriber::registry()
            .with(
                this.clone()
                    .with_filter(tracing_subscriber::filter::filter_fn(|meta| {
                        matches!(
                            meta.target(),
                            "librqbit::dht_utils"
                                | "librqbit::peer_connection"
                                | "librqbit::stream_connect"
                        )
                    })),
            )
            .try_init()
            .unwrap();
        Some(this)
    }
    pub fn finish(&self) -> BTreeMap<&'static str, u64> {
        self.active.store(false, Ordering::Relaxed);
        self.counts.lock().unwrap().clone()
    }
    fn count(&self, key: &'static str) {
        *self.counts.lock().unwrap().entry(key).or_default() += 1;
    }
}
struct Message {
    value: String,
}
impl Visit for Message {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.value = format!("{value:?}");
        }
    }
}
impl<S: Subscriber> Layer<S> for Counters {
    fn on_new_span(&self, attrs: &Attributes<'_>, _: &tracing::Id, _: Context<'_, S>) {
        if self.active.load(Ordering::Relaxed)
            && attrs.metadata().name() == "read_metainfo_from_peer"
        {
            self.count("metadataPeerAttempts");
        }
    }
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        if !self.active.load(Ordering::Relaxed) {
            return;
        }
        let mut msg = Message {
            value: String::new(),
        };
        event.record(&mut msg);
        let text = msg.value;
        for (pattern, key) in [
            ("connected over TCP", "tcpConnected"),
            ("connected over uTP", "utpConnected"),
            ("sending extended handshake", "extendedHandshakeSent"),
            ("received: Extended(Handshake", "extendedHandshakeReceived"),
            ("UtMetadata(Request", "metadataRequests"),
            ("UtMetadata(Data", "metadataData"),
            ("error reading metainfo", "metadataPeerErrors"),
            ("timeout", "timeouts"),
            ("Connection refused", "refused"),
            ("unexpected end of file", "remoteEof"),
            ("metadata_size", "metadataSizeMentioned"),
            ("does not support", "unsupported"),
            ("reader finished with error", "readerErrors"),
            ("writer finished with error", "writerErrors"),
            ("peer disconnected", "remoteDisconnected"),
            ("error reading handshake", "handshakeReadError"),
            ("error writing handshake", "handshakeWriteError"),
            ("wrong info hash", "wrongHash"),
            ("connecting to ourselves", "selfConnection"),
            ("deserializing handshake", "handshakeDecodeError"),
            ("error reading:", "streamReadError"),
            ("error writing:", "streamWriteError"),
            ("Connection reset", "reset"),
            ("Broken pipe", "brokenPipe"),
            ("network is unreachable", "networkUnreachable"),
            ("Operation not supported", "operationUnsupported"),
        ] {
            if text.contains(pattern) {
                self.count(key);
            }
        }
    }
}
