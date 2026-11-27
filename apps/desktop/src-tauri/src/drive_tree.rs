//! 两个目录读取现场（主列表与移动选择器）分别取消，旧结果不覆盖新导航。
use std::collections::VecDeque;
use std::sync::{Mutex, PoisonError};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
pub struct Queries(Mutex<Inner>);
#[derive(Default)]
struct Inner {
    slots: [Option<(String, CancellationToken)>; 2],
    closed: VecDeque<String>,
}
impl Queries {
    pub fn begin(&self, id: &str, remember: bool, parent: &CancellationToken) -> CancellationToken {
        let mut inner = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let cancel = parent.child_token();
        if inner.closed.iter().any(|closed| closed == id) {
            cancel.cancel();
            return cancel;
        }
        if let Some((_, old)) =
            inner.slots[usize::from(remember)].replace((id.to_owned(), cancel.clone()))
        {
            old.cancel();
        }
        cancel
    }
    pub fn close(&self, id: &str) {
        let mut inner = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        for slot in &mut inner.slots {
            if slot.as_ref().is_some_and(|(current, _)| current == id)
                && let Some((_, cancel)) = slot.take()
            {
                cancel.cancel();
            }
        }
        if !inner.closed.iter().any(|closed| closed == id) {
            inner.closed.push_back(id.into());
        }
        while inner.closed.len() > 32 {
            inner.closed.pop_front();
        }
    }
    pub fn clear(&self) {
        let mut inner = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        for slot in &mut inner.slots {
            if let Some((_, cancel)) = slot.take() {
                cancel.cancel();
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_is_scoped_and_cannot_cancel_a_newer_request() {
        let queries = Queries::default();
        let session = CancellationToken::new();
        let a = queries.begin("a", true, &session);
        let picker = queries.begin("picker", false, &session);
        let b = queries.begin("b", true, &session);
        assert!(a.is_cancelled());
        assert!(!picker.is_cancelled());
        queries.close("a");
        assert!(!b.is_cancelled());
        queries.close("early");
        assert!(queries.begin("early", true, &session).is_cancelled());
        queries.clear();
        assert!(b.is_cancelled() && picker.is_cancelled());
    }
}
