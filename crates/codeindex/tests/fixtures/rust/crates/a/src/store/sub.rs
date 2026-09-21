//! Fixture: `store::sub`, reached through `store/sub.rs`.

use super::Store;

pub fn check(path: &str) -> bool {
    Store::new(path).path.is_empty()
}

impl Store {
    /// The one `summarize` in the tree.
    pub fn summarize(&self) -> usize {
        self.path.len()
    }
}
