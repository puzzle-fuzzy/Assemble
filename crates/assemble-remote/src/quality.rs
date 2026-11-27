//! 自动档需要连续样本才换挡，避免一次网络尖峰造成来回抖动。
use assemble_protocol::remote::RemoteQuality;
use std::time::Duration;

#[derive(Debug)]
pub struct QualityPolicy {
    requested: RemoteQuality,
    effective: RemoteQuality,
    good: u8,
    bad: u8,
}
impl Default for QualityPolicy {
    fn default() -> Self {
        Self {
            requested: RemoteQuality::Auto,
            effective: RemoteQuality::Balanced,
            good: 0,
            bad: 0,
        }
    }
}
impl QualityPolicy {
    pub fn select(&mut self, quality: RemoteQuality) {
        self.requested = quality;
        self.effective = if quality == RemoteQuality::Auto {
            RemoteQuality::Balanced
        } else {
            quality
        };
        self.good = 0;
        self.bad = 0;
    }
    pub fn effective(&self) -> RemoteQuality {
        self.effective
    }
    pub fn requested(&self) -> RemoteQuality {
        self.requested
    }
    pub fn profile(&self) -> (u32, u32, u8) {
        match self.effective {
            RemoteQuality::Smooth => (1000, 700, 45),
            RemoteQuality::Sharp => (1920, 1200, 85),
            RemoteQuality::Auto | RemoteQuality::Balanced => (1600, 1000, 65),
        }
    }
    pub fn observe(&mut self, elapsed: Duration) {
        if self.requested != RemoteQuality::Auto {
            return;
        }
        self.bad = if elapsed >= Duration::from_millis(350) {
            self.bad.saturating_add(1)
        } else {
            0
        };
        self.good = if elapsed <= Duration::from_millis(100) {
            self.good.saturating_add(1)
        } else {
            0
        };
        if self.bad >= 3 {
            self.effective = match self.effective {
                RemoteQuality::Sharp => RemoteQuality::Balanced,
                _ => RemoteQuality::Smooth,
            };
            self.good = 0;
            self.bad = 0;
        } else if self.good >= 20 {
            self.effective = match self.effective {
                RemoteQuality::Smooth => RemoteQuality::Balanced,
                _ => RemoteQuality::Sharp,
            };
            self.good = 0;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn automatic_quality_has_hysteresis_and_manual_quality_is_stable() {
        let mut policy = QualityPolicy::default();
        policy.observe(Duration::from_secs(1));
        assert_eq!(policy.effective(), RemoteQuality::Balanced);
        for _ in 0..3 {
            policy.observe(Duration::from_secs(1));
        }
        assert_eq!(policy.effective(), RemoteQuality::Smooth);
        for _ in 0..19 {
            policy.observe(Duration::from_millis(30));
        }
        assert_eq!(policy.effective(), RemoteQuality::Smooth);
        policy.observe(Duration::from_millis(30));
        assert_eq!(policy.effective(), RemoteQuality::Balanced);
        policy.select(RemoteQuality::Sharp);
        for _ in 0..50 {
            policy.observe(Duration::from_secs(2));
        }
        assert_eq!(policy.effective(), RemoteQuality::Sharp);
    }
}
