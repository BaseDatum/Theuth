//! Wire and domain types of the knowledge bundle (design doc 10 §10).
//! Shared by the store, the tools, the actor messages, RPC and the TUI.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Who wrote a concept (frontmatter `source`; absent for hand-written files).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// Written by the harness from a harvest reply (10 §5).
    Harvest,
    /// Written by the model through `knowledge_put`.
    Model,
    /// Written by a human through RPC/TUI or an editor.
    User,
    /// Found changed on disk by `sync`.
    Sync,
}

impl Source {
    /// Frontmatter value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Harvest => "harvest",
            Self::Model => "model",
            Self::User => "user",
            Self::Sync => "sync",
        }
    }

    /// Inverse of [`Self::as_str`].
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "harvest" => Self::Harvest,
            "model" => Self::Model,
            "user" => Self::User,
            "sync" => Self::Sync,
            _ => return None,
        })
    }
}

/// `KnowledgeChanged.op`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeOp {
    /// Created or updated.
    Put,
    /// Removed.
    Delete,
}

/// A concept's frontmatter as the harness reads it (10 §2.2).
#[derive(
    Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Meta {
    /// OKF concept id: the path without `.md` (`facts/wth-toolchain`).
    pub id: String,
    /// Frontmatter `type` (the one conformance requirement).
    #[serde(rename = "type")]
    pub type_: String,
    /// Frontmatter `title`; the id when absent.
    pub title: String,
    /// Frontmatter `description`; `""` when absent.
    pub description: String,
    /// Frontmatter `tags`.
    pub tags: Vec<String>,
    /// Absolute directory this concept is specific to (10 §2.2).
    pub scope: Option<PathBuf>,
    /// Frontmatter `source`.
    pub source: Option<Source>,
    /// Frontmatter `timestamp` (RFC 3339).
    pub timestamp: Option<String>,
}

/// A full concept as `knowledge_get` returns it.
#[derive(
    Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Concept {
    /// Frontmatter.
    pub meta: Meta,
    /// The file verbatim (frontmatter + body).
    pub text: String,
    /// Outbound links: `(target id, exists)`.
    pub links: Vec<(String, bool)>,
    /// Concepts linking here.
    pub cited_by: Vec<String>,
}

/// One search hit (10 §3.3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Hit {
    /// Frontmatter.
    pub meta: Meta,
    /// Higher is better (scope-adjusted `-bm25`).
    pub score: f32,
    /// FTS5 snippet from the body; `""` when none.
    pub snippet: String,
}

/// What a writer supplies to `put` (10 §2.3). `timestamp` and `source` are
/// set by the harness.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Put {
    /// Concept id (`dir/slug`, lowercase; ≤ 3 segments).
    pub id: String,
    /// `fact | entity | decision | procedure | project` (open).
    #[serde(rename = "type")]
    pub type_: String,
    /// One line.
    pub title: String,
    /// One line; the title when absent.
    #[serde(default)]
    pub description: Option<String>,
    /// Tags.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Absolute directory the concept is specific to.
    #[serde(default)]
    pub scope: Option<PathBuf>,
    /// Markdown body (≤ `BODY_MAX`).
    pub body: String,
}

/// `sync` result (10 §3.2).
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
    kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Sync {
    /// New rows.
    pub added: u32,
    /// Rows refreshed from a changed file.
    pub updated: u32,
    /// Rows whose file vanished.
    pub removed: u32,
}

impl Sync {
    /// Anything changed.
    pub fn is_empty(self) -> bool {
        self.added == 0 && self.updated == 0 && self.removed == 0
    }
}

/// One harvest's outcome (10 §5.4).
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
    kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct HarvestResult {
    /// Concepts created.
    pub added: u32,
    /// Concepts updated.
    pub updated: u32,
    /// Documents rejected by validation.
    pub skipped: u32,
}

/// One commit in the bundle's history (10 §2.4).
#[derive(
    Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Change {
    /// Abbreviated commit id (7 hex chars).
    pub commit: String,
    /// Commit time, `YYYY-MM-DD HH:MM` local.
    pub time: String,
    /// Commit message.
    pub message: String,
}

/// `knowledge_history` result.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct History {
    /// Newest first.
    pub changes: Vec<Change>,
    /// Unified diff of the latest change to the requested id; `None` for the
    /// whole-bundle history or a concept with a single commit.
    pub diff: Option<String>,
}

/// How a commit touched a concept file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum ChangeStatus {
    /// Absent in the parent, present in the commit.
    Added,
    /// Present in both trees with different content or mode.
    Modified,
    /// Present in the parent, absent in the commit.
    Deleted,
}

/// One concept file changed by a commit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ChangedFile {
    /// Concept id.
    pub id: String,
    /// Whether the file was added, modified or deleted.
    pub status: ChangeStatus,
    /// Unified diff against the first parent; `/dev/null` names an absent side.
    pub diff: String,
}

/// `knowledge_revision` result: one commit against its first parent (the
/// empty tree for a root commit), concept files only.
#[derive(
    Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Revision {
    /// The selected commit's identity, time and message.
    pub change: Change,
    /// Full id of the first parent; `None` for a root commit.
    pub parent: Option<String>,
    /// Tree order (sorted by path).
    pub files: Vec<ChangedFile>,
}
