//! 云盘分片在受控暂存区保存，最终仍复用内容库的校验与租约。
pub(crate) mod capacity;
mod routes;
mod storage;
pub use routes::routes;
pub use storage::Store;
pub(crate) use storage::sweep;
