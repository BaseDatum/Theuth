//! The generator — what the model decides (147 §6): an engine-owned pipeline
//! of no-tools side calls (`runtime::SideCall { kind: CallKind::CodeMap }`
//! asked of the agent actor through the registry, C8's shape), one job per
//! root, stages A → B → D → C, every reply JSON-validated by [`super::validate`]
//! with one repair round, then the structure fallback. Cost is charged to the
//! opening session (S1); progress is `CodeindexEvent::CodeMapProgress`.
//!
//! The one-job-per-root registry (§6.1 `Mutex<HashMap<PathBuf, JobHandle>>`)
//! lives in this file as a `static` — one `Codeindex` per process, and it keeps
//! `index.rs` untouched by the generator.
//!
//! owner: W2 — design §6, §10.1 (CodeMapProgress), §10.3 (Generate/Cancel)

use super::{Cancel, Generate, MapJob, not_built};
use crate::index::Codeindex;

/// Stage-B calls in flight at once (§6.7).
pub const TOP_CONCURRENCY: usize = 3;
/// Elements described per stage-C call (§6.2).
pub const DESCRIBE_BATCH: usize = 10;
/// Entry points per flow perspective (§6.2 stage D).
pub const FLOW_ENTRY_POINTS: usize = 4;
/// `CodeMapProgress` gate per root, as `CodeReconciled` (§10.1).
pub const PROGRESS_GATE: std::time::Duration = std::time::Duration::from_millis(250);

/// `codemap_generate`: start the root's job or return the running one;
/// `element` set = regenerate that subtree only. `ASK_FAILED` with the
/// supervisor's text when `session` is not a live native session (C2).
pub async fn generate(ix: &Codeindex, m: Generate) -> anyhow::Result<MapJob> {
    let _ = (ix, m);
    Err(not_built("generator::generate", 2))
}

/// `codemap_cancel`: abort the root's running job (`stage: "cancelled"`);
/// stored elements stay. Errors when no job runs.
pub async fn cancel(ix: &Codeindex, m: Cancel) -> anyhow::Result<MapJob> {
    let _ = (ix, m);
    Err(not_built("generator::cancel", 2))
}
