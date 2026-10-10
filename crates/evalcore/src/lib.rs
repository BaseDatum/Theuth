//! 159 §13.1 — the shared eval core. A prompt study (W2) and dreaming's rehearse step judge
//! the same thing, a text replayed over cases; this crate holds what they share and nothing
//! else: no actor, no store, no runtime. Depends on `serde`, `serde_json`, `sha2`, `regex`
//! and `async-trait` only.
//!
//! Ownership (159 §18): C froze this file and every signature; W2 implements `case`, `judge`,
//! `gate`, `rows` and `run`; W3 implements `pair`; W7 consumes `gate::{paired, decide}`.
//! `split` and `candidate_hash` are implemented by C (they are the two pieces every consumer
//! needs on day one).

pub mod case;
pub mod gate;
pub mod judge;
pub mod pair;
pub mod rows;
pub mod run;

pub use case::{Case, Check, Shares, Split, candidate_hash, split};
