// 144-W1 owns this file.
//! Source ingest (design 144 §4): the nine origin routes, the [`IngestActor`] (hub-only,
//! `notebooks.ingest_concurrency` jobs at a time), freshness and provenance. The four functions
//! below are what the `source_*` RPCs and tools call; the skeleton leaves them unimplemented.

use kameo::actor::ActorRef;
use serde::{Deserialize, Serialize};

use crate::actor::DocumentActor;
use crate::meta::{DocId, Principal};
use crate::notebook::Source;

/// The §13.2 `source_add` arguments: exactly one of `path | url | text | meeting | session | mcp`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SourceAddArgs {
    /// The notebook the source is filed in.
    pub notebook: DocId,
    /// A file on this machine (PDF, document, audio, video, …); becomes `File | Audio | Video` by extension.
    #[serde(default)]
    pub path: Option<String>,
    /// A web page or a YouTube URL.
    #[serde(default)]
    pub url: Option<String>,
    /// Pasted text.
    #[serde(default)]
    pub text: Option<String>,
    /// A meeting id (its transcript).
    #[serde(default)]
    pub meeting: Option<String>,
    /// A past session id (its transcript).
    #[serde(default)]
    pub session: Option<String>,
    /// An MCP tool call (Confluence page, Jira issue, …).
    #[serde(default)]
    pub mcp: Option<McpRef>,
    /// Title override; default from the origin (file name, page title, meeting title, first line).
    #[serde(default)]
    pub title: Option<String>,
}

/// One MCP call a source is fetched by.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct McpRef {
    /// MCP server name.
    pub server: String,
    /// Tool name.
    pub tool: String,
    /// The call's arguments.
    pub args: serde_json::Value,
}

/// The ingest job runner (§4.2): one per engine, hub-only. Placeholder until W1 lands.
pub struct IngestActor;

const NOT_IMPLEMENTED: &str = "144: not implemented yet";

/// Pre-flight the origin, write the `sources` row (`new`) and the `Source` document, file it in the
/// notebook, queue the job; answers at once with the card. Hub only (`NotHub` off the hub).
pub async fn source_add(
    _actor: &ActorRef<DocumentActor>,
    _args: SourceAddArgs,
    _by: Principal,
) -> anyhow::Result<Source> {
    Err(anyhow::anyhow!(NOT_IMPLEMENTED))
}

/// One source with its ingest state. Any engine.
pub async fn source_get(_actor: &ActorRef<DocumentActor>, _id: DocId) -> anyhow::Result<Source> {
    Err(anyhow::anyhow!(NOT_IMPLEMENTED))
}

/// Sources of `notebook`, or every source (unfiled included). Any engine.
pub async fn source_list(
    _actor: &ActorRef<DocumentActor>,
    _notebook: Option<DocId>,
) -> anyhow::Result<Vec<Source>> {
    Err(anyhow::anyhow!(NOT_IMPLEMENTED))
}

/// Re-run the job from the origin or the blob (§4.2); refused while `running | queued`. Hub only.
pub async fn source_refresh(
    _actor: &ActorRef<DocumentActor>,
    _id: DocId,
    _by: Principal,
) -> anyhow::Result<Source> {
    Err(anyhow::anyhow!(NOT_IMPLEMENTED))
}
