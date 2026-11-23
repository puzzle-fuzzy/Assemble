//! 账户：公开信息，不含密码散列等内部字段。

use serde::{Deserialize, Serialize};

use crate::primitives::Timestamp;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    pub username: String,
    pub display_name: String,
    pub created_at: Timestamp,
}
