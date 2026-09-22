//! Fixture: definitions, nesting, locals, and a `mod` tree with a re-export.

pub mod store;
pub use store::open_db;

/// A registry.
pub struct Registry {
    items: Vec<String>,
}

impl Registry {
    pub fn new() -> Self {
        Self { items: Vec::new() }
    }

    fn push(&mut self, s: String) {
        self.items.push(s);
    }
}

pub trait Store {
    fn get(&self, k: &str) -> Option<String>;
}

impl Store for Registry {
    fn get(&self, k: &str) -> Option<String> {
        self.items.iter().find(|i| i.as_str() == k).cloned()
    }
}

pub enum Kind {
    A,
    B,
}

macro_rules! twice {
    ($e:expr) => {
        $e + $e
    };
}

pub mod inner {
    pub fn helper() -> u32 {
        twice!(1)
    }
}

fn apply(f: fn(u32) -> u32, x: u32) -> u32 {
    // `f` is a parameter: not a reference to any `f` in the repo.
    let g = |y: u32| y + 1;
    f(g(x)) + inner::helper()
}

pub fn run() -> u32 {
    let mut r = Registry::new();
    r.push("a".into());
    apply(|v| v * 2, 3)
}
