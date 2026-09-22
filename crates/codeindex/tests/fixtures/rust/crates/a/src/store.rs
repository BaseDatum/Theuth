//! Fixture: a module reached by `mod store;`, with a child by `mod sub;`.

use crate::Registry;

pub mod sub;

/// Opens the fixture store.
pub fn open_db(path: &str) -> Registry {
    let _ = sub::check(path);
    Registry::new()
}

pub struct Store {
    pub path: String,
}

impl Store {
    pub fn new(path: &str) -> Self {
        Self { path: path.to_owned() }
    }
}
