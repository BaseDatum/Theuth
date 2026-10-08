//! `dreaming_ledger` (design doc 136 §4 phase 1): asks `agent::LedgerSince`
//! (since `state.last_dream_at` or 24 h, excluding the dreaming session and
//! its children) and knowledge `Stats` for the shown ids, folds
//! `ledger/pending/*.json` and copies the desk journals into
//! `ledger/<night>/journals/`, writes `ledger/<night>.json` (≤ 2 MB) and
//! returns it plus `run_request`. 136 W3.

use std::path::Path;

use crate::types::RunRequest;

/// What `dreaming_ledger` returns: the agent's ledger plus dreaming's folds.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct DayLedger {
    /// The agent's part.
    pub ledger: agent::Ledger,
    /// Rewind snapshots from `ledger/pending/`.
    pub pending_rewinds: Vec<agent::LedgerRewind>,
    /// Desk journal file names copied this night.
    pub desk_journals: Vec<String>,
    /// A `run_now` waiting for this run.
    pub run_request: Option<RunRequest>,
}

/// Build tonight's ledger (136 W3).
pub async fn build(data_dir: &Path, since: Option<&str>) -> anyhow::Result<DayLedger> {
    // 136 W3
    let _ = (data_dir, since);
    anyhow::bail!("not built: W3 (dreaming_ledger)")
}
