//! The `dreaming.*` RPC methods (design doc 136 §9.5, contract §2.7) as
//! plain async fns the `rpc` crate's `ApiServer` delegates to: each asks the
//! [`Night`] actor; absent actor → `Err("dreaming is not running")`.

use kameo::actor::ActorRef;
use runtime::SessionId;

use crate::night::{self, DREAMING_ACTOR, Night};
use crate::types::{
    DreamSettings, DreamStatus, Metrics, NightSummary, Proposal, RunRequest, UndoReport,
};

/// The method names the `rpc` crate registers (contract §2.7).
pub const METHODS: [&str; 9] = [
    "dreaming.status",
    "dreaming.nights",
    "dreaming.run_now",
    "dreaming.undo",
    "dreaming.settings",
    "dreaming.set_settings",
    "dreaming.proposals",
    "dreaming.proposal_act",
    "dreaming.metrics",
];

fn actor() -> Result<ActorRef<Night>, String> {
    ActorRef::<Night>::lookup(DREAMING_ACTOR)
        .ok()
        .flatten()
        .ok_or_else(|| "dreaming is not running".to_owned())
}

async fn ask<M>(
    m: M,
) -> Result<<<Night as kameo::message::Message<M>>::Reply as kameo::Reply>::Ok, String>
where
    Night: kameo::message::Message<M>,
    M: Send + 'static,
    <<Night as kameo::message::Message<M>>::Reply as kameo::Reply>::Error: std::fmt::Display,
{
    actor()?.ask(m).await.map_err(|e| e.to_string())
}

/// `dreaming.status`.
pub async fn status() -> Result<DreamStatus, String> {
    ask(night::Status).await
}

/// `dreaming.nights(limit)`.
pub async fn nights(limit: u32) -> Result<Vec<NightSummary>, String> {
    ask(night::Nights { limit }).await
}

/// `dreaming.run_now(RunRequest)` → the routine's session.
pub async fn run_now(r: RunRequest) -> Result<SessionId, String> {
    ask(night::RunNow(r)).await
}

/// `dreaming.undo(night, group?)`.
pub async fn undo(night: String, group: Option<String>) -> Result<UndoReport, String> {
    ask(night::Undo { night, group }).await
}

/// `dreaming.settings`.
pub async fn settings() -> Result<DreamSettings, String> {
    ask(night::Settings).await
}

/// `dreaming.set_settings(DreamSettings)`.
pub async fn set_settings(s: DreamSettings) -> Result<DreamSettings, String> {
    ask(night::SetSettings(s)).await
}

/// `dreaming.proposals`.
pub async fn proposals() -> Result<Vec<Proposal>, String> {
    ask(night::Proposals).await
}

/// `dreaming.proposal_act(id, choice)`.
pub async fn proposal_act(id: String, choice: String) -> Result<(), String> {
    ask(night::ProposalAct { id, choice }).await
}

/// `dreaming.metrics(days)`.
pub async fn metrics(days: u32) -> Result<Metrics, String> {
    ask(night::MetricsOver { days }).await
}
