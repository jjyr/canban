//! Bounded in-memory LRU cache for UI-derived Markdown documents, report bodies, and previews.
use std::{borrow::Borrow, collections::BTreeMap};

/// Deterministic capacity-bounded Least-Recently-Used cache (`O(log N)` lookup, touch, and eviction).
#[derive(Clone, Debug)]
pub struct LruCache<K, V> {
    capacity: usize,
    tick: u64,
    entries: BTreeMap<K, (u64, V)>,
    order: BTreeMap<u64, K>,
}

impl<K: Ord + Clone, V> LruCache<K, V> {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            tick: 0,
            entries: BTreeMap::new(),
            order: BTreeMap::new(),
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.entries.contains_key(key)
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }

    /// Look up `key` without updating recency order.
    pub fn peek<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.entries.get(key).map(|(_, v)| v)
    }

    /// Look up `key` and promote it to most-recently-used.
    pub fn get<Q>(&mut self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        let next_tick = self.tick.saturating_add(1);
        let (stored_key, entry) = self.entries.get_key_value(key)?;
        let stored_key = stored_key.clone();
        let prev_tick = entry.0;
        self.tick = next_tick;
        self.order.remove(&prev_tick);
        self.order.insert(next_tick, stored_key);
        let entry = self.entries.get_mut(key)?;
        entry.0 = next_tick;
        Some(&entry.1)
    }

    /// Insert or replace `key`, promoting it to most-recently-used and evicting the
    /// least-recently-used entry when the cache is at `capacity`.
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        if self.capacity == 0 {
            return None;
        }
        if let Some((prev_tick, prev_val)) = self.entries.remove(&key) {
            self.order.remove(&prev_tick);
            self.tick = self.tick.saturating_add(1);
            let next_tick = self.tick;
            self.order.insert(next_tick, key.clone());
            self.entries.insert(key, (next_tick, value));
            return Some(prev_val);
        }
        while self.entries.len() >= self.capacity {
            let Some((_, oldest_key)) = self.order.pop_first() else {
                break;
            };
            self.entries.remove(&oldest_key);
        }
        self.tick = self.tick.saturating_add(1);
        let next_tick = self.tick;
        self.order.insert(next_tick, key.clone());
        self.entries.insert(key, (next_tick, value));
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_least_recently_used_and_promotes_accessed_entries() {
        let mut cache = LruCache::<String, i32>::new(3);
        cache.insert("a".into(), 1);
        cache.insert("b".into(), 2);
        cache.insert("c".into(), 3);
        assert_eq!(cache.len(), 3);

        // Touch `"a"` by `&str` without allocating so `"b"` becomes the oldest entry.
        assert_eq!(cache.get("a"), Some(&1));

        cache.insert("d".into(), 4);
        assert_eq!(cache.len(), 3);
        assert_eq!(
            cache.peek("b"),
            None,
            "oldest untouched entry must be evicted"
        );
        assert_eq!(cache.peek("a"), Some(&1));
        assert_eq!(cache.peek("c"), Some(&3));
        assert_eq!(cache.peek("d"), Some(&4));

        // Updating `"c"` refreshes its recency so `"a"` becomes oldest.
        assert_eq!(cache.insert("c".into(), 30), Some(3));
        cache.insert("e".into(), 5);
        assert_eq!(cache.peek("a"), None);
        assert_eq!(cache.peek("c"), Some(&30));
        assert_eq!(cache.peek("d"), Some(&4));
        assert_eq!(cache.peek("e"), Some(&5));
    }

    #[test]
    fn zero_capacity_discards_inserts() {
        let mut cache = LruCache::<String, i32>::new(0);
        assert_eq!(cache.insert("a".into(), 1), None);
        assert!(cache.is_empty());
        assert_eq!(cache.get("a"), None);
    }
}
