//! Owner: E2 — python kernels (design 171 §5.8): the resolved interpreter running
//! `~/.theuth/cells/shim.py` on pipes in its own process group, NDJSON frames.

use serde::{Deserialize, Serialize};

use crate::error::{CellsError, Result};
use crate::kernel::{Kernel, KernelCtx};
use crate::types::KernelSpec;

/// The shim, embedded and written to `~/.theuth/cells/shim.py` at start (binary-owned).
pub const SHIM: &str = include_str!("../shim.py");

/// E2's python keys of the `cells:` section (flattened into [`crate::CellsConfig`]).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PythonConfig {}

/// Start a python kernel for `spec` (`KernelSpec::Python`).
pub async fn start(spec: &KernelSpec, ctx: KernelCtx) -> Result<Box<dyn Kernel>, CellsError> {
    let _ = (spec, ctx);
    Err(CellsError::NotBuilt("E2"))
}
