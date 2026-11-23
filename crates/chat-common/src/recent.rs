//! Bounded FIFO dedup set (21 §3.1, 98 §11).

use std::borrow::Borrow;
use std::collections::{HashSet, VecDeque};
use std::hash::Hash;

/// Dedup set size.
pub const RECENT_KEYS: usize = 1024;

/// Bounded FIFO set of recently seen keys: envelope keys (Slack) or
/// `update_id`s (Telegram).
#[derive(Debug)]
pub struct RecentKeys<K> {
    order: VecDeque<K>,
    set: HashSet<K>,
}

impl<K> Default for RecentKeys<K> {
    fn default() -> Self {
        Self {
            order: VecDeque::new(),
            set: HashSet::new(),
        }
    }
}

impl<K: Clone + Eq + Hash> RecentKeys<K> {
    /// Records `key`, evicting the oldest past [`RECENT_KEYS`]; `true` when it is new.
    pub fn insert<Q>(&mut self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ToOwned<Owned = K> + ?Sized,
    {
        if self.set.contains(key) {
            return false;
        }
        if self.order.len() >= RECENT_KEYS
            && let Some(old) = self.order.pop_front()
        {
            self.set.remove::<K>(&old);
        }
        self.order.push_back(key.to_owned());
        self.set.insert(key.to_owned());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_the_oldest_past_the_cap() {
        let mut r = RecentKeys::<String>::default();
        assert!(r.insert("k0"));
        assert!(!r.insert("k0"));
        for i in 1..RECENT_KEYS {
            assert!(r.insert(&format!("k{i}")));
        }
        assert!(!r.insert("k0"), "still present at exactly the cap");
        assert!(r.insert("k1024"));
        assert!(!r.insert("k1"), "second-oldest survives the 1025th key");
        assert!(
            r.insert("k0"),
            "evicted by the 1025th key (and re-inserted, evicting k1)"
        );
    }

    #[test]
    fn integer_keys() {
        let mut r = RecentKeys::<i64>::default();
        assert!(r.insert(&1_i64));
        assert!(!r.insert(&1_i64));
        for i in 2..=(RECENT_KEYS as i64 + 1) {
            assert!(r.insert(&i));
        }
        // 1 was evicted: it is new again.
        assert!(r.insert(&1_i64));
    }
}
