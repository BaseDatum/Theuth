// 144-W3 owns this file.
//! Notebook chat (design 144 §8): the `sessions.notebook` binding, the per-notebook chat list, and
//! "save answer as note" / "promote into this note". The RPC layer calls these four; the skeleton
//! leaves them unimplemented. Ids are strings here: the agent crate does not depend on `document`.

use kameo::actor::ActorRef;
use runtime::SessionId;
use serde::{Deserialize, Serialize};

use crate::SessionSummary;
use crate::supervisor::Agents;

/// `save_answer_as_note` reply: the new note and its `theuth://notes/<id>` URL.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SavedAnswer {
    /// The note's document id.
    pub doc: String,
    /// `theuth://notes/<id>`.
    pub url: String,
}

const NOT_IMPLEMENTED: &str = "144: not implemented yet";

/// Sessions bound to `notebook`, newest first (the "Chats · 3 ▾" menu). Owner only.
pub async fn notebook_chats(
    _agents: &ActorRef<Agents>,
    _notebook: String,
) -> anyhow::Result<Vec<SessionSummary>> {
    Err(anyhow::anyhow!(NOT_IMPLEMENTED))
}

/// Bind (or unbind, `None`) `session` to a notebook: `sessions.notebook`. Owner only; one notebook per session.
pub async fn session_set_notebook(
    _agents: &ActorRef<Agents>,
    _session: SessionId,
    _notebook: Option<String>,
) -> anyhow::Result<()> {
    Err(anyhow::anyhow!(NOT_IMPLEMENTED))
}

/// `doc_create(title, markdown)` from the assistant message at `turn` (1-based) with chips intact plus the
/// `*Answer from [chat](theuth://sessions/<id>#<turn>) · <model> · <date>*` line; the note is added to
/// `notebook`. Runs §6.6.
pub async fn save_answer_as_note(
    _agents: &ActorRef<Agents>,
    _session: SessionId,
    _turn: u32,
    _notebook: String,
) -> anyhow::Result<SavedAnswer> {
    Err(anyhow::anyhow!(NOT_IMPLEMENTED))
}

/// `doc_insert(at="end")` of the same markdown under `## From chat · <date>` into `note`, as
/// `Principal::Agent{session}` so "Undo last agent edit" reverts it.
pub async fn promote_answer(
    _agents: &ActorRef<Agents>,
    _session: SessionId,
    _turn: u32,
    _note: String,
) -> anyhow::Result<()> {
    Err(anyhow::anyhow!(NOT_IMPLEMENTED))
}
