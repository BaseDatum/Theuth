//! Owner: E2 — interpreter discovery (design 171 §6): `auto` per scope, the chip menu,
//! `interpreters(cwd)`; shells through E1's [`crate::shell::resolve`].

use std::collections::BTreeMap;
use std::path::Path;

use crate::error::{CellsError, Result};
use crate::types::{Interpreter, KernelSpec, PythonChoice};

/// Every candidate for `cwd` with its `why`, `auto` marking what `auto`/the default picks
/// (§6.4); cached 60 s per (cwd, login-env generation).
pub async fn interpreters(cwd: &Path, login_env: &BTreeMap<String, String>) -> Vec<Interpreter> {
    let _ = (cwd, login_env);
    Vec::new()
}

/// Resolve a python cell's interpreter (§6.1): `choice` from the header, else the note default,
/// else `auto`. `monty_ok` = every `auto` python cell of the scope is Monty-sufficient.
pub async fn resolve(
    choice: Option<&PythonChoice>,
    cwd: &Path,
    login_env: &BTreeMap<String, String>,
    monty_ok: bool,
) -> Result<(KernelSpec, Interpreter), CellsError> {
    let _ = (choice, cwd, login_env, monty_ok);
    Err(CellsError::NotBuilt("E2"))
}
