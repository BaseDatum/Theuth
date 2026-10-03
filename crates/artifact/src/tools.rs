//! The `show` tool (35 §3.1), `artifact_read` (66 §6.2) and the prompt block (35 §5).

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tools::fs::ReadFileArgs;
use tools::{Risk, Tool, ToolCx, ToolOutput};

use crate::pdf::Source;
use crate::store::Stores;
use crate::{Artifact, ExportDoc, ExportFormat, Kind, kind};

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
`show` puts a file in front of the user as a card in this chat — rendered, openable, savable. Use it whenever the deliverable is a file: a report, a summary longer than a screen, a table over ~8 rows, a document you were asked to write, an image or screenshot they should look at, a PDF. `show{path}` shows a file that exists (never read a file and then repeat its content in your reply — show it). `show{content, name}` writes and shows a new one; a `.pdf` name turns markdown into a real PDF (front matter `title`, `author`, `toc: true` for a title block and contents) and the result includes a picture of page 1 — look at it; `.zip`/`.tar.gz` shows the archive with a browsable listing (build it with `tar`/`zip` in `shell()` first). A diagram the user should keep or open on its own: `show{content, name: 'x.mmd'}`; a diagram that explains this reply: a ```mermaid fence in your text. Same name again = a new version. One sentence of takeaway in your reply, then the card carries the content. `message_user` is still how you reach the user *outside* this chat.";

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

/// Arguments of `artifact_read`: the `call_id` and `read_file`'s selection.
#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ReadArgs {
    /// The artifact's `call_id`, from an `[attached artifact …]` line, a `show` result or `search`.
    pub artifact: String,
    /// 1-based first line to return.
    #[serde(default)]
    pub offset: Option<usize>,
    /// Maximum number of lines to return (default 2000).
    #[serde(default)]
    pub limit: Option<usize>,
    /// Return the last N lines instead; not combinable with `offset`/`limit`.
    #[serde(default)]
    pub tail: Option<usize>,
    /// 1-based inclusive `[start, end]` line windows, as `read_file`'s. Not combinable with `offset`/`tail`.
    #[serde(default)]
    pub ranges: Option<Vec<[usize; 2]>>,
}

/// `artifact_read`: one artifact version by `call_id` — text and PDFs paged
/// like `read_file`, images returned as pictures, archives as their listing,
/// apps as their source.
pub struct ReadTool {
    /// The stores; the id is looked up in every one (a call_id names one row).
    pub stores: Arc<Stores>,
}

#[async_trait::async_trait]
impl Tool for ReadTool {
    const NAME: &'static str = "artifact_read";
    const RISK: Risk = Risk::ReadOnly;
    type Args = ReadArgs;

    fn description(&self) -> String {
        "Read a saved artifact by its call_id. Line 1 is a header `[artifact_read \"<title>\" (<name> v<N> of <M>, \
         <kind>, <size>, shown <date> in session \"…\"): …]` — a newer version is named there. Text kinds and \
         PDFs/documents (rendered as Markdown) page like `read_file`: first 2000 lines, then `offset`/`limit`, \
         `tail` or `ranges`. Images come back as pictures, archives as their listing, apps as their source; a \
         markdown artifact bound to a document names its document id for `doc_read`/`doc_replace`."
            .into()
    }

    async fn call(&self, a: ReadArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let call_id = a.artifact.trim();
        let mut found = None;
        for state in self.stores.all() {
            if let Ok(r) = state.record(call_id).await {
                found = Some((state, r));
                break;
            }
        }
        let Some((state, art)) = found else {
            return Ok(ToolOutput::err(format!(
                "artifact {call_id} not found (deleted or from another data dir)"
            )));
        };
        let siblings = state
            .list(Some(art.session), false)
            .await
            .unwrap_or_default();
        let same: Vec<&Artifact> = siblings.iter().filter(|s| s.name == art.name).collect();
        let versions = same.len().max(art.version as usize);
        let latest = same
            .iter()
            .max_by_key(|s| s.version)
            .filter(|s| s.version > art.version);
        let doc = if art.kind == Kind::Markdown {
            bound_document(&art).await
        } else {
            None
        };
        let shown = header(
            &art,
            versions,
            latest.map(|l| l.call_id.as_str()),
            session_title(art.session).await.as_deref(),
            doc.as_deref(),
        );

        let bytes = match tokio::fs::read(&art.path).await {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ToolOutput::err(format!(
                    "[artifact_read {shown}]: blob missing"
                )));
            }
            Err(e) => anyhow::bail!("{}: {e}", art.path.display()),
        };
        match art.kind {
            Kind::Image => Ok(ToolOutput {
                files: vec![art.path.clone()],
                ..ToolOutput::ok(format!(
                    "[artifact_read {shown}: image, {}]",
                    kind::human(art.bytes)
                ))
            }),
            Kind::Archive => {
                let listing = match crate::archive::read_listing(&art.path).await {
                    Ok(l) => l,
                    Err(e) => return Ok(ToolOutput::err(format!("[artifact_read {shown}]: {e}"))),
                };
                let mut s = format!(
                    "[artifact_read {shown}: archive, {} entries, {} unpacked{}]\n",
                    listing.total_entries,
                    kind::human(listing.total_size),
                    if listing.truncated {
                        ", listing truncated"
                    } else {
                        ""
                    }
                );
                for e in &listing.entries {
                    s.push_str(&e.path);
                    if e.is_dir {
                        if !e.path.ends_with('/') {
                            s.push('/');
                        }
                    } else {
                        s.push_str(&format!("\t{}", kind::human(e.size)));
                    }
                    s.push('\n');
                }
                Ok(ToolOutput::ok(s))
            }
            _ => {
                let (note, text) = match tools::fs::text_of(bytes, &shown).await {
                    Ok(t) => t,
                    Err(e) => return Ok(ToolOutput::err(e.to_string())),
                };
                if note.is_none() && text.contains('\u{FFFD}') && art.kind == Kind::File {
                    return Ok(ToolOutput::ok(format!(
                        "[artifact_read {shown}: binary, {}; not readable as text]",
                        kind::human(art.bytes)
                    )));
                }
                let args = ReadFileArgs {
                    path: art.path.clone(),
                    offset: a.offset,
                    limit: a.limit,
                    tail: a.tail,
                    ranges: a.ranges,
                    numbers: true,
                    outline: false,
                };
                Ok(ToolOutput::ok(tools::fs::paginate(
                    "artifact_read",
                    &shown,
                    note.as_deref(),
                    &text,
                    &args,
                    Vec::new(),
                )?))
            }
        }
    }
}

/// `"<title>" (<name> v<N> of <M>, <kind>, <bytes>, shown <date> in session "<title>")`,
/// plus `; latest is <call_id>` and `; editable as document <id> (doc_read)` when they apply.
fn header(
    a: &Artifact,
    versions: usize,
    latest: Option<&str>,
    session: Option<&str>,
    doc: Option<&str>,
) -> String {
    let date = chrono::DateTime::from_timestamp(a.created_at, 0)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%d")
                .to_string()
        })
        .unwrap_or_default();
    let mut s = format!(
        "{:?} ({} v{} of {versions}, {}, {}",
        a.title,
        a.name,
        a.version,
        a.kind.as_str(),
        kind::human(a.bytes)
    );
    if let Some(p) = a.pages {
        s.push_str(&format!(", {p} pages"));
    }
    s.push_str(&format!(", shown {date}"));
    if let Some(t) = session {
        s.push_str(&format!(" in session {t:?}"));
    }
    if let Some(l) = latest {
        s.push_str(&format!("; latest is {l}"));
    }
    if let Some(d) = doc {
        s.push_str(&format!("; editable as document {d} (doc_read)"));
    }
    s.push(')');
    s
}

/// The document a markdown artifact is bound to (59 §0.7), if the document plugin runs.
async fn bound_document(a: &Artifact) -> Option<String> {
    let docs = document::Handle::lookup()?;
    docs.actor()
        .ask(document::Lookup {
            binding: document::Binding::Artifact {
                session: a.session.to_string(),
                name: a.name.clone(),
            },
        })
        .await
        .ok()
        .flatten()
        .map(|id| id.to_string())
}

/// The owning session's title, when the agent supervisor runs and lists it.
async fn session_title(session: runtime::SessionId) -> Option<String> {
    let agents = kameo::actor::ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
        .ok()
        .flatten()?;
    let rows: Vec<agent::SessionSummary> = agents.ask(agent::ListSessions).await.ok()?;
    rows.into_iter()
        .find(|r| r.id == session)
        .map(|r| r.title)
        .filter(|t| !t.trim().is_empty())
}

/// Arguments of `doc_export` (72 §3).
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DocExportArgs {
    /// Document id (from `doc_create` / `doc_read`).
    pub doc: String,
    /// `markdown` or `pdf` (default).
    #[serde(default)]
    pub format: Option<ExportFormat>,
    /// Absolute path under the user's home to write; omitted = an artifact card in this chat.
    #[serde(default)]
    pub dest: Option<PathBuf>,
}

/// `doc_export`: a note or board as a `.md` or `.pdf` file — a card in the chat, or a file at `dest`.
/// Lives here rather than with the other `doc_*` tools because `document` cannot depend on this crate.
pub struct DocExportTool {
    /// The durable store (exports are copies of persistent documents).
    pub state: Arc<crate::store::State>,
}

#[async_trait::async_trait]
impl Tool for DocExportTool {
    const NAME: &'static str = "doc_export";
    const RISK: Risk = Risk::Mutating;
    const RETURNS: &'static str = "dict";
    type Args = DocExportArgs;

    fn description(&self) -> String {
        "Export a note or board (`doc` from doc_create / doc_read) as a file: `pdf` (default; A4, \
         diagrams as text, excerpts as numbered code) or `markdown`. Without `dest` the file becomes an \
         artifact card in this chat; with `dest` (absolute, under the user's home) it is written there. \
         Returns {path | artifact_id, call_id, pages, warnings}."
            .into()
    }

    async fn call(&self, a: DocExportArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let Some(doc) = document::DocId::parse(&a.doc) else {
            return Ok(ToolOutput::err(format!(
                "doc_export: `{}` is not a document id",
                a.doc
            )));
        };
        let Some(docs) = document::Handle::lookup() else {
            return Ok(ToolOutput::err("doc_export: documents are disabled"));
        };
        let format = a.format.unwrap_or(ExportFormat::Pdf);
        if let Some(dest) = &a.dest {
            let home = dirs::home_dir().unwrap_or_default();
            if !dest.is_absolute()
                || !dest.starts_with(&home)
                || dest
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                return Ok(ToolOutput::err(format!(
                    "doc_export: dest must be an absolute path under {}",
                    home.display()
                )));
            }
        }
        let by = document::Principal::Agent {
            session: cx.session.0,
        };
        let meta = docs
            .actor()
            .ask(document::MetaOf {
                doc: doc.clone(),
                by,
            })
            .await
            .map_err(anyhow::Error::msg)?;
        let markdown = docs
            .actor()
            .ask(document::Markdown {
                doc,
                blocks: None,
                annotate: format == ExportFormat::Pdf,
            })
            .await
            .map_err(anyhow::Error::msg)?;
        let r = self
            .state
            .export_doc(ExportDoc {
                title: meta.title,
                format,
                markdown,
                dest: a.dest,
                assets: Vec::new(),
                record: Some((cx.session, cx.call_id)),
            })
            .await;
        Ok(match r {
            Ok(r) => {
                let mut text = match (&r.path, &r.call_id) {
                    (Some(p), _) => format!("wrote {}", p.display()),
                    (None, Some(c)) => format!("exported as artifact {c}"),
                    _ => "exported".to_owned(),
                };
                if let Some(p) = r.pages {
                    text.push_str(&format!(" ({p} pages)"));
                }
                for w in &r.warnings {
                    text.push_str(&format!("\nwarning: {w}"));
                }
                ToolOutput {
                    structured: serde_json::to_value(&r).ok(),
                    ..ToolOutput::ok(text)
                }
            }
            Err(e) => ToolOutput::err(format!("doc_export: {e}")),
        })
    }
}
