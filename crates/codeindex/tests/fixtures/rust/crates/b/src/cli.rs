//! Fixture: `crate::` and `super::` paths from a child module of a bin.

use crate::Args;
use fixture_a::store::Store;
use fixture_a::Registry;

pub fn run(_r: Registry, _s: Store, _n: u32, _m: std::collections::HashMap<String, u32>) -> Option<Args> {
    None
}
