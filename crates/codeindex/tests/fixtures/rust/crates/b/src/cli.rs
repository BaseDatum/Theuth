//! Fixture: `crate::` and `super::` paths from a child module of a bin.

use crate::Args;
use fixture_a::store::Store;
use fixture_a::Registry;

pub fn run(_r: Registry, _s: Store, _n: u32, _m: std::collections::HashMap<String, u32>) -> Option<Args> {
    None
}

/// Receiver-call targets (60 §21): `clone_from` is stoplisted, `describe` is not.
pub struct Opts;

impl Opts {
    pub fn clone_from(&self) -> Self {
        Opts
    }

    pub fn describe(&self) -> String {
        String::new()
    }
}

/// Generic impl header: methods still hang off `Wrapper`.
pub struct Wrapper<T>(pub T);

impl<T> Wrapper<T> {
    pub fn unwrap_inner(self) -> T {
        self.0
    }
}
