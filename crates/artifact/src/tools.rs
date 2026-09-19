//! The `show` tool (35 §3.1) and the prompt block (35 §5).

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tools::{Risk, Tool, ToolCx, ToolOutput};

use crate::pdf::Source;
use crate::store::Stores;
use crate::{Kind, kind};

/// Arguments of `show`. Exactly one of `path` / `content`; `content` requires `name`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ShowArgs {
    /// Absolute path of an existing file to show (a file you wrote, a screenshot, a document).
    pub path: Option<PathBuf>,
    /// Text to write and show instead of `path`. Requires `name`. For `.pdf` names this is the
    /// document source (markdown by default; `source: typst` for Typst).
    pub content: Option<String>,
    /// File name with extension; the extension picks the rendering (`report.md`, `chart.svg`,
    /// `summary.pdf`, `data.csv`, `notes.txt`, `page.html`). Showing the same name again in this
    /// session makes a new version.
    pub name: Option<String>,
    /// Short human title for the card. Default: `name`, or the first heading of a markdown file.
    pub title: Option<String>,
    /// Source language of `content` when `name` ends in `.pdf`: `markdown` (default) or `typst`.
    pub source: Option<Source>,
}

/// Prompt block registered as `RegisterPrompt { plugin: "artifact" }` (35 §5).
pub const PROMPT: &str = "## Hand over files, don't paste them\n\
`show` puts a file in front of the user as a card in this chat — rendered, openable, savable. Use it whenever the deliverable is a file: a report, a summary longer than a screen, a table over ~8 rows, a document you were asked to write, an image or screenshot they should look at, a PDF. `show{path}` shows a file that exists (never read a file and then repeat its content in your reply — show it). `show{content, name}` writes and shows a new one; a `.pdf` name turns markdown into a real PDF (front matter `title`, `author`, `toc: true` for a title block and contents) and the result includes a picture of page 1 — look at it. Same name again = a new version. One sentence of takeaway in your reply, then the card carries the content. `message_user` is still how you reach the user *outside* this chat.";

/// `show`: snapshot a file (or write `content`) under the harness and show it
/// as a card. Calls [`crate::store::State::show`] directly — the tool already
/// runs off the registry's mailbox, and the receipt needs numbers only the
/// store has.
pub struct ShowTool {
    /// The stores, routed by the calling session.
    pub stores: Arc<Stores>,
}

#[async_trait::async_trait]
impl Tool for ShowTool {
    const NAME: &'static str = "show";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "dict";
    const DIRECT: bool = true;
    type Args = ShowArgs;

    fn description(&self) -> String {
        "Show a file to the user in the chat, as a card they can read, open and save: markdown, \
         text and code render inline; images and SVGs show as pictures; CSV as a table; `.pdf` \
         names compile the markdown (or Typst) in `content` into a real PDF. Pass `path` for a \
         file that exists, or `content` + `name` to write one under the harness first. The result \
         tells you the version and, for PDFs and SVGs, includes a picture of the result. Use this \
         instead of pasting a file's content into your reply."
            .into()
    }

    async fn call(&self, args: ShowArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        // A private caller whose tree the actor has not mapped yet must not
        // fall through to the durable store.
        let state = if cx.resources.is_private() {
            self.stores.private_for(cx.session)
        } else {
            self.stores.state_for(cx.session).ok()
        };
        let Some(state) = state else {
            return Ok(ToolOutput::err("show: private session is not open"));
        };
        let shown = match state.show(cx.session, cx.call_id, args).await {
            Ok(s) => s,
            Err(msg) => return Ok(ToolOutput::err(format!("show: {msg}"))),
        };
        let mut content = kind::receipt(&shown.artifact, &shown.receipt);
        if shown.artifact.kind == Kind::File {
            content.push_str("\nnote: unknown kind; shown as a downloadable file");
        }
        Ok(ToolOutput {
            structured: serde_json::to_value(&shown.artifact).ok(),
            files: shown.artifact.preview.into_iter().collect(),
            ..ToolOutput::ok(content)
        })
    }
}
