//! Import of an existing `<root>/.omm/` (147 §4.6, §13.2): read at skeleton
//! time into `imported:<dir>` perspectives with `origin: Imported`; every
//! `diagram.mmd` through `surface::graph::from_mermaid` with any ids allowed,
//! node labels' second line matched against `files.path` (hit → `unit:`
//! anchor, miss → `exists: false`), fields copied, `meta.yaml`
//! `git_commit`/`updated` into provenance (`model: "imported"`). Read-only:
//! never regenerated, exported or marked stale.
//!
//! owner: W4 — design §4.6, §13.2

use std::path::Path;

use sqlx::SqlitePool;

use super::not_built;
use crate::index::RootIndex;

/// Read `<root>/.omm/` (when present) into `pool`'s `imported:<dir>`
/// perspectives, replacing the previous import. Returns the elements
/// imported; 0 when there is no `.omm/`. Called by `skeleton::recompute`.
pub async fn import(ri: &RootIndex, pool: &SqlitePool) -> anyhow::Result<u32> {
    let _ = (ri, pool);
    Err(not_built("import::import", 4))
}

/// Whether `omm_dir`'s `config.yaml` carries our `generator:` key (§10.2's
/// export refusal: a foreign `.omm/` is never overwritten in v0).
pub fn is_ours(omm_dir: &Path) -> anyhow::Result<bool> {
    let _ = omm_dir;
    Err(not_built("import::is_ours", 4))
}
