//! Fixture: `store::sub`, reached through `store/sub.rs`.

use super::Store;

pub fn check(path: &str) -> bool {
    Store::new(path).path.is_empty()
}
