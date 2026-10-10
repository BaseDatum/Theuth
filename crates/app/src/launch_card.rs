//! 159 W6 — the launch card (159 §11 "Launch card and attention row"): an app card like the
//! study draft, posted to the study's origin chat when a world experiment asks for its press:
//! the diff summary, the arms, the exposure, the window, *Approve* and *Refuse*. Both presses
//! run `autoresearch.launch {slug, n, token, approve}` with the press token the `launches` row
//! hashes and the model never sees — "the press, not the model, starts the study" (140 §12.4)
//! holds for every launch too (159 §15 "Must not regress").
//!
//! Owner: W6. The skeleton (C) registers the module (`pub mod launch_card` and the
//! `RegisterTool` line in lib.rs, both `// 159:W6`) and freezes the two entry points below;
//! their bodies are W6's. Nothing here runs for a 140 study.

use std::sync::Arc;

use autoresearch::StudySpec;

use crate::actor::Core;

/// The OpenUI app name the card publishes under.
pub const APP_NAME: &str = "study-launches";

/// What the card shows (W6 fills it from the `launches` row and the experiment).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Launch {
    /// Study.
    pub slug: String,
    /// Experiment.
    pub n: u32,
    /// The diff summary the session logged.
    pub summary: String,
    /// Share of traffic both arms take.
    pub exposure: f64,
    /// The window, humantime.
    pub window: String,
    /// The press token (never shown to the model).
    pub token: String,
}

/// The card's plain lines: "Launch #<n> of `<slug>`: <summary>", the arms and the exposure, the
/// window, the rollback command.
pub fn lines(spec: &StudySpec, launch: &Launch) -> [String; 4] {
    [
        format!(
            "Launch #{} of `{}`: {}",
            launch.n,
            spec.slug,
            launch.summary.trim()
        ),
        format!(
            "two arms, {:.0} % of traffic together ({:.0} % each)",
            launch.exposure * 100.0,
            launch.exposure * 50.0
        ),
        format!("windows of {}", launch.window),
        "Approve deploys both arms; Refuse closes the experiment".to_owned(),
    ]
}

/// The OpenUI source of the card with its Approve and Refuse presses wired through the token.
pub fn source(spec: &StudySpec, launch: &Launch) -> String {
    let _ = (spec, launch);
    String::new()
}

/// Post the card to the study's origin chat (called by `world::launch` plugin-side).
pub async fn post(core: Arc<Core>, spec: &StudySpec, launch: &Launch) -> Result<(), String> {
    let _ = (core, spec, launch);
    Err("not built (W6)".into())
}
