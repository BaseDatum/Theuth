//! Owner: A1 — the three read-only runtime functions (design 171 §9.5): `cell_runs`,
//! `cell_output`, `interpreters` (`Risk::ReadOnly`), and the prompt block registration.

use runtime::Runtime;

use crate::Cells;

/// Register the runtime functions and (when non-empty) the prompt block. No-op until A1.
pub async fn register(rt: &Runtime, cells: Cells) {
    let _ = (rt, cells);
}
