//! 通知去重账本：同一条通知在这台电脑上只弹一次。
//!
//! 服务端给的名单里带着全部还没读的通知：每次连上、每一处改动，都是整份再来一遍。
//! 账本记住这台电脑已经弹过哪些，同一份名单再来一次就不会再弹。它存在本机设置里，
//! 所以应用重启之后，弹过的也不会再来一遍。
//!
//! 账本是有界的：超过上限时淘汰最早的记录，设置文件不会随着时间无限增长。

use std::collections::{HashSet, VecDeque};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// 账本的默认容量：足以覆盖很长一段时间的通知，又不至于让状态文件变大。
pub const DEFAULT_CAPACITY: usize = 2_000;

impl Default for DeliveryLedger {
    /// 容量取默认值的空账本。
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }
}

/// 已显示通知的编号，按记录先后排列。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryLedger {
    order: VecDeque<String>,
    delivered: HashSet<String>,
    capacity: usize,
}

impl DeliveryLedger {
    /// 容量为 0 的账本什么都不记。
    pub fn new(capacity: usize) -> Self {
        Self {
            order: VecDeque::with_capacity(capacity),
            delivered: HashSet::with_capacity(capacity),
            capacity,
        }
    }

    pub fn contains(&self, notification_id: &str) -> bool {
        self.delivered.contains(notification_id)
    }

    /// 记下一次已经显示过的通知。返回它是不是新记录；重复记录不改变账本。
    pub fn record(&mut self, notification_id: &str) -> bool {
        if self.capacity == 0 || self.delivered.contains(notification_id) {
            return !self.delivered.contains(notification_id);
        }
        self.delivered.insert(notification_id.to_owned());
        self.order.push_back(notification_id.to_owned());
        while self.order.len() > self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.delivered.remove(&oldest);
            }
        }
        true
    }

    /// 换服务来源或换账户时清空：旧账户的通知编号对新账户没有意义。
    pub fn clear(&mut self) {
        self.order.clear();
        self.delivered.clear();
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// 按记录先后排列的通知编号。
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.order.iter().map(String::as_str)
    }
}

// 落盘就是一个按先后排列的编号数组。
impl Serialize for DeliveryLedger {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.order.iter())
    }
}

impl<'de> Deserialize<'de> for DeliveryLedger {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let ids = Vec::<String>::deserialize(deserializer)?;
        let mut ledger = Self::new(DEFAULT_CAPACITY);
        for id in ids {
            ledger.record(&id);
        }
        Ok(ledger)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_notification_is_remembered_after_its_first_delivery() {
        let mut ledger = DeliveryLedger::new(10);
        assert!(!ledger.contains("n-1"));
        assert!(ledger.record("n-1"));
        assert!(ledger.contains("n-1"));
        assert_eq!(ledger.len(), 1);

        // 同一份名单再来一遍：不再是新记录，账本也不变。
        assert!(!ledger.record("n-1"));
        assert_eq!(ledger.len(), 1);
        assert!(ledger.record("n-2"));
        assert_eq!(ledger.iter().collect::<Vec<_>>(), vec!["n-1", "n-2"]);
    }

    #[test]
    fn the_ledger_is_bounded_and_evicts_the_oldest_first() {
        let mut ledger = DeliveryLedger::new(3);
        for id in ["n-1", "n-2", "n-3", "n-1", "n-4"] {
            ledger.record(id);
        }
        assert_eq!(ledger.len(), 3);
        // 最早的 n-1 被淘汰；它再次到达时会重新算作新通知。
        assert!(!ledger.contains("n-1"));
        assert_eq!(ledger.iter().collect::<Vec<_>>(), vec!["n-2", "n-3", "n-4"]);
        assert!(ledger.record("n-1"));
        assert_eq!(ledger.iter().collect::<Vec<_>>(), vec!["n-3", "n-4", "n-1"]);

        // 默认容量能装下很长一段历史。
        let mut ledger = DeliveryLedger::new(DEFAULT_CAPACITY);
        for index in 0..(DEFAULT_CAPACITY + 25) {
            ledger.record(&format!("n-{index}"));
        }
        assert_eq!(ledger.len(), DEFAULT_CAPACITY);
        assert!(!ledger.contains("n-0"));
    }

    #[test]
    fn a_zero_capacity_ledger_remembers_nothing() {
        let mut ledger = DeliveryLedger::new(0);
        assert!(ledger.record("n-1"));
        assert!(ledger.is_empty());
        assert!(!ledger.contains("n-1"));
        // 之后再放大容量也不会凭空记得。
        assert!(ledger.record("n-1"));
        assert!(ledger.is_empty());
    }

    #[test]
    fn switching_scope_clears_everything() {
        let mut ledger = DeliveryLedger::new(10);
        ledger.record("n-1");
        ledger.record("n-2");
        ledger.clear();
        assert!(ledger.is_empty());
        assert!(ledger.record("n-1"));
    }

    #[test]
    fn the_stored_form_is_a_plain_array_like_the_original_state() {
        let mut ledger = DeliveryLedger::new(10);
        ledger.record("n-1");
        ledger.record("n-2");
        assert_eq!(serde_json::to_string(&ledger).unwrap(), r#"["n-1","n-2"]"#);
        assert_eq!(
            serde_json::to_value(DeliveryLedger::new(10)).unwrap(),
            json!([])
        );

        // 存下来的数组里有重复项时直接读回来，重复的并成一个。
        let restored: DeliveryLedger = serde_json::from_str(r#"["n-1","n-2","n-1"]"#).unwrap();
        assert_eq!(restored.len(), 2);
        assert!(restored.contains("n-2"));
        assert_eq!(
            serde_json::to_string(&restored).unwrap(),
            r#"["n-1","n-2"]"#
        );

        // 不是数组的文件视为损坏，而不是悄悄当成空账本。
        assert!(serde_json::from_str::<DeliveryLedger>(r#"{"n-1":true}"#).is_err());
        assert!(serde_json::from_str::<DeliveryLedger>("null").is_err());
    }
}
