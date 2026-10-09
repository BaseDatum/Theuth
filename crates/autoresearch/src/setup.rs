//! 151 — the setup chat's engine side (design doc 151 §5–§6): the living
//! draft's field states (`DraftView`), the probe that is the only source of
//! "Verified", and the panel's edit / confirm / discard. Owned by ENG after the
//! contract; C committed the tool's registration and this stub.

use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tools::{Risk, Tool, ToolCx, ToolOutput};

use crate::tools::Seams;
use crate::types::ProbeKind;

/// What the skeleton answers until ENG builds the body.
pub const NOT_BUILT: &str = "151: not built yet";

/// `autoresearch_probe` arguments (151 D4).
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ProbeArgs {
    /// The draft id (the spec's slug).
    pub draft_id: String,
    /// `bench | check | holdout`.
    #[serde(default)]
    pub what: ProbeKind,
    /// Bench / holdout samples; default the spec's `bench.repeats`, clamped 3..=10.
    #[serde(default)]
    pub repeats: Option<u8>,
}

/// `autoresearch_probe` (151 D4): measure a draft's bench / check / holdout in
/// a scratch detached worktree of the repo at `base`; the result feeds the
/// panel's "Verified" badge. A root tool like `autoresearch_draft`; refused
/// inside a study session.
pub struct ProbeTool(pub Arc<Seams>);

#[async_trait::async_trait]
impl Tool for ProbeTool {
    const NAME: &'static str = "autoresearch_probe";
    const RISK: Risk = Risk::Mutating;
    const RETURNS: &'static str = "dict";
    type Args = ProbeArgs;

    fn description(&self) -> String {
        "Measure a drafted autoresearch study before it starts: `what: bench` runs the draft's benchmark `repeats` \
         times (3–10) in a scratch worktree of the repo at `base` and reports samples, median and noise; `check` runs \
         the correctness check once; `holdout` runs the holdout. The result is the only thing that marks a draft row \
         Verified in the Autoresearch tab — call it after writing or changing the bench, check or holdout, and never \
         state a measurement it did not produce. Never touches the user's checkout."
            .into()
    }

    async fn call(&self, a: ProbeArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        anyhow::bail!(
            "autoresearch_probe {} {:?}: {NOT_BUILT}",
            a.draft_id.trim(),
            a.what
        )
    }
}
