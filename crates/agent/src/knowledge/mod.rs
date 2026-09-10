//! Knowledge: an OKF v0.1 bundle the agent reads by id, searches, and
//! builds while the user works (design doc 10).
//!
//! - [`store`]: the bundle on disk + the FTS5 index (`Knowledge`).
//! - [`repo`]: local git history of the bundle via `gix` (no binary).
//! - [`harvest`]: the cache-hot continuation's instruction and reply parser,
//!   and the `<knowledge>` hint rendering.
//! - [`tools`]: the `knowledge_*` tools.

pub mod harvest;
pub mod repo;
pub mod store;
pub mod tools;
pub mod types;

pub use store::{Knowledge, terms};
pub use types::*;

/// Default mid-turn hint cadence in rounds (10 §4.2; `agent.knowledge.hint_rounds`).
pub const KNOWLEDGE_HINT_ROUNDS: u32 = 4;
/// Bundle directory under the data dir (10 §2.1).
pub const DIR: &str = "knowledge";
/// Derived FTS index file under the data dir (10 §3.1).
pub const INDEX_FILE: &str = "knowledge.sqlite";
/// Max body bytes accepted by `put` (10 §2.2).
pub const BODY_MAX: usize = 16 * 1024;
/// Description clip in hint lines (10 §4.1).
pub const HINT_DESCRIPTION_CHARS: usize = 120;
