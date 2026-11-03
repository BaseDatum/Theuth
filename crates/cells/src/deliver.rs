//! Owner: E1 — `<cell_result>` (design 171 §9.1, §9.3): built from the committed row, handed
//! to the agent as `HarnessDeliver{kind: "cell <name>", wake}` per the wake rule.

use crate::config::CellsConfig;
use crate::error::{CellsError, Result};
use crate::types::{CellRun, SessionId};

/// The `<cell_result …>` text for `run` (`cleaned` = redacted output; head+tail capped).
pub fn cell_result(run: &CellRun, cleaned: &str, cfg: &CellsConfig) -> String {
    let _ = (run, cleaned, cfg);
    String::new()
}

/// Deliver `text` to `session` with the §9.3 wake rule (`wake = false` parks it).
pub async fn deliver(
    session: SessionId,
    run: &CellRun,
    text: String,
    wake: bool,
) -> Result<(), CellsError> {
    let _ = (session, run, text, wake);
    Err(CellsError::NotBuilt("E1"))
}
