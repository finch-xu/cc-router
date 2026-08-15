//! In-memory session → subscription affinity for `RoutingMode::Sticky`.
//! Not persisted (same trade-off as `VirtualModelConfig::last_used_index`): a restart
//! costs each live session one cold cache, which is acceptable.
//!
//! Idle TTL = 1h, aligned with the longest Anthropic/OpenAI prompt-cache lifetime;
//! after that the cache is cold anyway and staying pinned would only block rebalancing.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::virtual_model::model::VirtualModelName;

pub const IDLE_TTL: Duration = Duration::from_secs(60 * 60);
pub const MAX_ENTRIES: usize = 10_000;
const SWEEP_EVERY: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, Copy)]
struct Pin {
    sub_id: Uuid,
    last_seen: Instant,
}

#[derive(Debug, Default)]
pub struct AffinityTable {
    map: HashMap<(VirtualModelName, String), Pin>,
    last_sweep: Option<Instant>,
}

impl AffinityTable {
    pub fn get(&mut self, vm: VirtualModelName, key: &str, now: Instant) -> Option<Uuid> {
        let k = (vm, key.to_string());
        let pin = self.map.get_mut(&k)?;
        if now.saturating_duration_since(pin.last_seen) > IDLE_TTL {
            self.map.remove(&k);
            return None;
        }
        pin.last_seen = now;
        Some(pin.sub_id)
    }

    pub fn pin(&mut self, vm: VirtualModelName, key: &str, sub_id: Uuid, now: Instant) {
        if self.last_sweep.map_or(true, |t| now.saturating_duration_since(t) >= SWEEP_EVERY) {
            self.sweep(now);
        }
        let k = (vm, key.to_string());
        if !self.map.contains_key(&k) && self.map.len() >= MAX_ENTRIES {
            if let Some(oldest) = self.map.iter().min_by_key(|(_, p)| p.last_seen).map(|(k, _)| k.clone()) {
                self.map.remove(&oldest);
            }
        }
        self.map.insert(k, Pin { sub_id, last_seen: now });
    }

    pub fn sweep(&mut self, now: Instant) {
        self.map.retain(|_, p| now.saturating_duration_since(p.last_seen) <= IDLE_TTL);
        self.last_sweep = Some(now);
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn pin_get_and_idle_expiry() {
        let mut t = AffinityTable::default();
        let t0 = Instant::now();
        let s = Uuid::new_v4();
        assert_eq!(t.get(VirtualModelName::Sonnet, "k", t0), None);
        t.pin(VirtualModelName::Sonnet, "k", s, t0);
        assert_eq!(t.get(VirtualModelName::Sonnet, "k", t0 + Duration::from_secs(1)), Some(s));
        // 不同 vm 隔离
        assert_eq!(t.get(VirtualModelName::Opus, "k", t0), None);
        // 命中刷新: 59min 时命中, 再过 59min 仍在
        assert!(t.get(VirtualModelName::Sonnet, "k", t0 + Duration::from_secs(59 * 60)).is_some());
        assert!(t.get(VirtualModelName::Sonnet, "k", t0 + Duration::from_secs(118 * 60)).is_some());
        // 空闲超 1h → 过期
        assert_eq!(t.get(VirtualModelName::Sonnet, "k", t0 + Duration::from_secs(118 * 60 + 3601)), None);
        assert_eq!(t.len(), 0);
    }

    #[test]
    fn repin_overwrites() {
        let mut t = AffinityTable::default();
        let t0 = Instant::now();
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        t.pin(VirtualModelName::Haiku, "k", a, t0);
        t.pin(VirtualModelName::Haiku, "k", b, t0);
        assert_eq!(t.get(VirtualModelName::Haiku, "k", t0), Some(b));
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn evicts_oldest_when_full() {
        let mut t = AffinityTable::default();
        let t0 = Instant::now();
        for i in 0..MAX_ENTRIES {
            t.pin(VirtualModelName::Sonnet, &format!("k{i}"), Uuid::new_v4(), t0 + Duration::from_millis(i as u64));
        }
        assert_eq!(t.len(), MAX_ENTRIES);
        t.pin(VirtualModelName::Sonnet, "new", Uuid::new_v4(), t0 + Duration::from_secs(60));
        assert_eq!(t.len(), MAX_ENTRIES);
        assert_eq!(t.get(VirtualModelName::Sonnet, "k0", t0 + Duration::from_secs(61)), None, "最旧的 k0 被淘汰");
        assert!(t.get(VirtualModelName::Sonnet, "new", t0 + Duration::from_secs(61)).is_some());
    }

    #[test]
    fn sweep_removes_only_expired() {
        let mut t = AffinityTable::default();
        let t0 = Instant::now();
        t.pin(VirtualModelName::Sonnet, "old", Uuid::new_v4(), t0);
        t.pin(VirtualModelName::Sonnet, "fresh", Uuid::new_v4(), t0 + IDLE_TTL);
        t.sweep(t0 + IDLE_TTL + Duration::from_secs(1));
        assert_eq!(t.len(), 1);
        assert!(t.get(VirtualModelName::Sonnet, "fresh", t0 + IDLE_TTL + Duration::from_secs(1)).is_some());
    }
}
