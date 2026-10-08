//! The `autoresearch.*` RPC methods (design doc 140 §12.5) as plain async fns
//! the `rpc` crate's `ApiServer` delegates to: each asks the [`Studies`]
//! actor; absent actor → `Err("autoresearch is not running")`. Owned by W3
//! after the skeleton (bodies here are complete delegation; W3 changes the
//! actor, not this file, unless a method's shape changes).

use kameo::actor::ActorRef;

use crate::config::AutoresearchConfig;
use crate::types::*;
use crate::{AUTORESEARCH_ACTOR, Studies};

/// The method names the `rpc` crate registers (140 §12.5).
pub const METHODS: [&str; 10] = [
    "autoresearch.list",
    "autoresearch.get",
    "autoresearch.experiments",
    "autoresearch.experiment",
    "autoresearch.control",
    "autoresearch.steer",
    "autoresearch.idea",
    "autoresearch.create",
    "autoresearch.settings.get",
    "autoresearch.settings.set",
];

fn actor() -> std::result::Result<ActorRef<Studies>, String> {
    ActorRef::<Studies>::lookup(AUTORESEARCH_ACTOR)
        .ok()
        .flatten()
        .ok_or_else(|| "autoresearch is not running".to_owned())
}

async fn ask<M>(
    m: M,
) -> std::result::Result<<<Studies as kameo::message::Message<M>>::Reply as kameo::Reply>::Ok, String>
where
    Studies: kameo::message::Message<M>,
    M: Send + 'static,
    <<Studies as kameo::message::Message<M>>::Reply as kameo::Reply>::Error: std::fmt::Display,
{
    actor()?.ask(m).await.map_err(|e| e.to_string())
}

/// `autoresearch.list` → every study, newest first.
pub async fn list() -> std::result::Result<Vec<StudyInfo>, String> {
    ask(Status { slug: None }).await
}

/// `autoresearch.get {slug}` → the Study view's data.
pub async fn get(slug: String) -> std::result::Result<StudyDetail, String> {
    ask(Detail { slug }).await
}

/// `autoresearch.experiments {slug, before?, limit}`.
pub async fn experiments(slug: String, before: Option<u32>, limit: u32) -> std::result::Result<Vec<ExperimentRow>, String> {
    ask(Experiments { slug, before, limit }).await
}

/// `autoresearch.experiment {slug, n}`.
pub async fn experiment(slug: String, n: u32) -> std::result::Result<ExperimentDetail, String> {
    ask(Experiment { slug, n }).await
}

/// `autoresearch.control {slug, op}` (the user's buttons).
pub async fn control(slug: String, op: ControlOp) -> std::result::Result<StudyInfo, String> {
    ask(Control { slug, op, by: By::User }).await
}

/// `autoresearch.steer {slug, text}`.
pub async fn steer(slug: String, text: String) -> std::result::Result<SteerId, String> {
    ask(SteerMsg { slug, text, by: By::User }).await
}

/// `autoresearch.idea {slug, op}`.
pub async fn idea(slug: String, op: IdeaChange) -> std::result::Result<Idea, String> {
    ask(IdeaOp { slug, op, by: By::User }).await
}

/// `autoresearch.create {draft_id, start}` — the draft card's press.
pub async fn create(draft_id: String, start: StartWhen) -> std::result::Result<StudyInfo, String> {
    ask(StartDraft { draft_id, start }).await
}

/// `autoresearch.settings.get`.
pub async fn settings() -> std::result::Result<AutoresearchConfig, String> {
    ask(Settings).await
}

/// `autoresearch.settings.set`.
pub async fn set_settings(cfg: AutoresearchConfig) -> std::result::Result<AutoresearchConfig, String> {
    ask(SetSettings(cfg)).await
}
