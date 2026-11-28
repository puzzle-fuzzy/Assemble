//! 服务器复用下载核心；此入口不使用系统钥匙串，也不依赖窗口。
mod discovery;
mod relay;
pub use crate::magnet::hosted::{Account, Service, Torrent};
pub(crate) use discovery::Discovery;
pub(crate) use relay::PeerRelay;
