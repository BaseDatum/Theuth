//! Owner: E1 — shell kernels (design 171 §5.2, §5.4–5.7): a `cell` PTY job started with an
//! explicit `program`, the binary-owned rc files, the run file + run line, marks → events.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::{CellsError, Result};
use crate::kernel::{Kernel, KernelCtx};
use crate::types::{Interpreter, KernelSpec, ShellKind};

/// E1's keys of the `cells:` section (flattened into [`crate::CellsConfig`]).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShellConfig {}

/// Start a shell kernel for `spec` (`KernelSpec::Shell`).
pub async fn start(spec: &KernelSpec, ctx: KernelCtx) -> Result<Box<dyn Kernel>, CellsError> {
    let _ = (spec, ctx);
    Err(CellsError::NotBuilt("E1"))
}

/// §6.5: which binary runs a `kind` cell — the note default `shell:`, else the first `kind` on
/// the login PATH, else the system one — and its chip. E2's `interp::interpreters` lists the
/// shells through this.
pub async fn resolve(
    kind: ShellKind,
    note_shell: Option<&str>,
    login_env: &BTreeMap<String, String>,
) -> Result<(PathBuf, Interpreter), CellsError> {
    let _ = (kind, note_shell, login_env);
    Err(CellsError::NotBuilt("E1"))
}
