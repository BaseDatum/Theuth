//! Seams for user cells in notes (design doc 171): the key a cell is addressed
//! by, the actor message that reads a code block's text, the `doc_read`
//! run-annotation hook (§9.6) and the replicated latest-output rows (§13.2).
//! Types and stubs only; D2 builds the behaviour behind the `171:D2` marks.

use std::sync::{Arc, RwLock};

use kameo::message::{Context, Message};
use serde::{Deserialize, Serialize};

use crate::actor::DocumentActor;
use crate::meta::{DocId, Principal};
use crate::store::Store;

/// How a note cell is addressed (171 §15.1): by its `name:` header or by its
/// block id.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum NoteKey {
    /// The cell's `name:` header.
    Name(String),
    /// The BlockNote block id.
    Block(String),
}

/// Reply to [`CodeBlockText`]: a code block read from the live Y.Doc.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, kameo::Reply)]
pub struct CodeBlockTextReply {
    /// Block id.
    pub block: String,
    /// Fence language.
    pub language: String,
    /// Fence body.
    pub text: String,
    /// Who last wrote the block (`DocEvent::Authored`, 171 §9.6).
    pub last_author: Principal,
}

/// `ask` → `Result<CodeBlockTextReply, String>`: read one code block of a
/// note from the live Y.Doc (171 §15.2, internal — not RPC).
#[derive(Clone, Debug)]
pub struct CodeBlockText {
    /// Document.
    pub doc: DocId,
    /// The cell.
    pub key: NoteKey,
}

impl Message<CodeBlockText> for DocumentActor {
    type Reply = Result<CodeBlockTextReply, String>;

    async fn handle(
        &mut self,
        _: CodeBlockText,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        Err("not built (171:D2)".into()) // 171:D2
    }
}

/// What `doc_read` shows for a cell with a latest run (171 §9.6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellAnnotation {
    /// The outline prefix, e.g. `[cell:bash name · ran 14:38 · exit 0 · 33 s · expect ✓]`.
    pub outline: String,
    /// Text of the ```` ```output ```` fence, when the run produced one.
    pub output: Option<String>,
    /// The run line, e.g. `ran 14:38 · exit 0 · 33 s · expect ✓`.
    pub run_line: String,
}

/// Process-global hook the cells plugin installs so `doc_read` can annotate
/// a code block with its latest run; `cells` depends on `document`, not the
/// reverse (171 §9.6, the `tools::secrets` pattern).
pub trait RunAnnotations: Send + Sync {
    /// The annotation for one code block, `None` when it is not a cell or
    /// never ran.
    fn note_cell(
        &self,
        doc: &DocId,
        block: &str,
        language: &str,
        text: &str,
    ) -> Option<CellAnnotation>;
}

static RUN_ANNOTATIONS: RwLock<Option<Arc<dyn RunAnnotations>>> = RwLock::new(None);

/// Install the hook; a later call replaces the earlier one.
pub fn set_run_annotations(a: Arc<dyn RunAnnotations>) {
    *RUN_ANNOTATIONS.write().unwrap_or_else(|e| e.into_inner()) = Some(a);
}

/// The installed hook, if any.
pub fn run_annotations() -> Option<Arc<dyn RunAnnotations>> {
    RUN_ANNOTATIONS
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// One replicated latest-output row of a note cell (171 §13.2 `cell_latest`,
/// minus the HLC column).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct CellLatest {
    /// `"<doc>:<name or block id>"`.
    pub id: String,
    /// Document.
    pub doc: DocId,
    /// Cell name or block id.
    pub cell: String,
    /// Run id.
    pub run: String,
    /// Final `RunState`, snake_case.
    pub state: String,
    /// Exit code.
    pub exit: Option<i32>,
    /// Wall time.
    pub secs: Option<f64>,
    /// `expect` verdict, rendered.
    pub expect: Option<String>,
    /// Python/Monty one-line summary.
    pub summary: Option<String>,
    /// Digest of the code that ran.
    pub code_sha: String,
    /// Unix ms.
    pub ended_ms: i64,
    /// Cleaned, redacted head+tail; `""` for a private cell.
    pub text: String,
}

impl Store {
    /// Latest-output rows of a document (171 §13.2).
    pub async fn cell_latest(&self, _doc: &DocId) -> anyhow::Result<Vec<CellLatest>> {
        Ok(vec![]) // 171:D2
    }

    /// Upsert one latest-output row (171 §13.2).
    pub async fn put_cell_latest(&self, _row: &CellLatest) -> anyhow::Result<()> {
        Ok(()) // 171:D2
    }
}
