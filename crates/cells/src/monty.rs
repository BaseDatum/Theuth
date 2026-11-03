//! Owner: E2 — the Monty kernel (design 171 §5.9): a checkout from the cells-owned pool, no
//! tool table, `/cwd` mounted read-only, 60 s per run; plus the presentation-time sufficiency
//! check (§6.2).

use serde::{Deserialize, Serialize};

use crate::error::{CellsError, Result};
use crate::kernel::{Kernel, KernelCtx};
use crate::types::KernelSpec;

/// E2's Monty keys of the `cells:` section (flattened into [`crate::CellsConfig`]).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MontyConfig {}

/// Start a Monty kernel (`KernelSpec::Monty`).
pub async fn start(spec: &KernelSpec, ctx: KernelCtx) -> Result<Box<dyn Kernel>, CellsError> {
    let _ = (spec, ctx);
    Err(CellsError::NotBuilt("E2"))
}

/// §6.2: `Ok(())` when Monty can run `code` (`surface::cell::monty_scan` + the pool's type
/// check), else the reason shown on the cell. Cached by digest by the caller. `None` = not
/// checked (the pool is not built): validation then skips `monty_insufficient`.
pub async fn sufficient(code: &str) -> Option<Result<(), String>> {
    let _ = code;
    None
}
