//! A small in-memory cache with an expiry and a size bound, for what is cheap to recompute but
//! asked for often (folder layouts, region listings, parsed player and entity files).
use std::{
    collections::HashMap,
    hash::Hash,
    sync::Mutex,
    time::{Duration, Instant},
};

pub struct Memo<K, V> {
    entries: Mutex<HashMap<K, (Instant, V)>>,
    ttl: Duration,
    capacity: usize,
}

impl<K: Eq + Hash, V: Clone> Memo<K, V> {
    pub fn new(ttl: Duration, capacity: usize) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            ttl,
            capacity,
        }
    }

    pub fn get(&self, key: &K) -> Option<V> {
        let entries = self.entries.lock().unwrap_or_else(|err| err.into_inner());
        entries
            .get(key)
            .filter(|(stored, _)| stored.elapsed() < self.ttl)
            .map(|(_, value)| value.clone())
    }

    /// Stores `value`; when full, expired entries go first, then everything.
    pub fn insert(&self, key: K, value: V) {
        let mut entries = self.entries.lock().unwrap_or_else(|err| err.into_inner());
        if entries.len() >= self.capacity {
            let ttl = self.ttl;
            entries.retain(|_, (stored, _)| stored.elapsed() < ttl);
            if entries.len() >= self.capacity {
                entries.clear();
            }
        }
        entries.insert(key, (Instant::now(), value));
    }

    pub fn remove(&self, key: &K) {
        self.entries
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expires_and_stays_bounded() {
        let memo = Memo::new(Duration::from_secs(60), 2);
        memo.insert(1, "a");
        memo.insert(2, "b");
        assert_eq!(memo.get(&1), Some("a"));
        memo.insert(3, "c");
        assert_eq!(memo.get(&3), Some("c"));
        assert_eq!(memo.get(&1), None);
        memo.remove(&3);
        assert_eq!(memo.get(&3), None);

        let expired = Memo::new(Duration::ZERO, 10);
        expired.insert(1, "a");
        assert_eq!(expired.get(&1), None);
    }
}
