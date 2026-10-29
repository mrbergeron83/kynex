//! A cache bounded by size, dropping what was used longest ago: decoded
//! screenshots, which the transcript reads back from the log as they're shown.

use std::collections::HashMap;
use std::hash::Hash;

pub struct Lru<K, V> {
    /// Total size it may hold; the newest entry stays even if it alone is bigger.
    cap: usize,
    used: usize,
    /// Each entry with its size and when it was last used.
    map: HashMap<K, (V, usize, u64)>,
    clock: u64,
}

impl<K: Hash + Eq + Clone, V> Lru<K, V> {
    pub fn new(cap: usize) -> Self {
        Self { cap, used: 0, map: HashMap::new(), clock: 0 }
    }

    /// The entry for `key`, which counts as a use.
    pub fn get(&mut self, key: &K) -> Option<&V> {
        self.clock += 1;
        let entry = self.map.get_mut(key)?;
        entry.2 = self.clock;
        Some(&entry.0)
    }

    /// Adds `value`, `size` big, and returns what had to go to make room.
    pub fn insert(&mut self, key: K, value: V, size: usize) -> Vec<V> {
        self.clock += 1;
        let mut gone: Vec<V> = Vec::new();
        if let Some((old, s, _)) = self.map.insert(key.clone(), (value, size, self.clock)) {
            self.used -= s;
            gone.push(old);
        }
        self.used += size;
        while self.used > self.cap && self.map.len() > 1 {
            let oldest = self.map.iter().filter(|(k, _)| **k != key).min_by_key(|(_, e)| e.2).map(|(k, _)| k.clone());
            let Some((v, s, _)) = oldest.and_then(|k| self.map.remove(&k)) else { break };
            self.used -= s;
            gone.push(v);
        }
        gone
    }

    pub fn used(&self) -> usize {
        self.used
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_the_least_recently_used_to_stay_in_size() {
        let mut lru = Lru::new(10);
        assert!(lru.insert("a", 1, 4).is_empty());
        assert!(lru.insert("b", 2, 4).is_empty());
        assert_eq!(lru.get(&"a"), Some(&1));
        // "b" was used longest ago.
        assert_eq!(lru.insert("c", 3, 4), [2]);
        assert_eq!(lru.used(), 8);
        assert_eq!(lru.get(&"b"), None);
        // Too big alone: it stays, everything else goes.
        assert_eq!(lru.insert("d", 4, 20).len(), 2);
        assert_eq!(lru.used(), 20);
        assert_eq!(lru.get(&"d"), Some(&4));
        // Replacing an entry hands back the old value.
        assert_eq!(lru.insert("d", 5, 2), [4]);
        assert_eq!(lru.used(), 2);
    }
}
