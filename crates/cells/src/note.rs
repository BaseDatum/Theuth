//! Owner: E2 — note scope (design 171 §5.1, §9.6, §13.2): kernel session ids, hub routing,
//! source resolution through `document::CodeBlockText`, origin delivery, notebook result lines,
//! `cell_latest`, the `doc_read` run annotations. E1's actor calls these at its `// 171:E2`
//! lines; E2 never edits `actor.rs`.
//!
//! The hooks are free functions: [`install`] keeps the plugin's [`crate::Cells`] handle, and
//! every document/agent read goes through [`Docs`] (the live actors; a fake in tests).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};

use parking_lot::{Mutex, RwLock};

use crate::error::{CellsError, Result};
use crate::types::{
    CellLatest, CellRef, CellRun, DocId, ExitStatus, NoteDefaults, NoteKey, Principal, RunState,
    Scope, SessionId,
};

/// Where a note cell runs (§5.1 W-d): the document's hub.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    /// This engine is the hub.
    Local,
    /// Forward to the hub engine (its link peer id).
    Hub(String),
}

/// The source the engine runs (§3.4): the live Y.Doc's block.
pub type NoteSource = document::CodeBlockTextReply;

/// §5.1 W-b: the deterministic session id that owns a note kernel's PTY job:
/// UUIDv5(UUIDv5(URL, "theuth:cells"), doc id). Never appears in a session list.
pub fn kernel_session(doc: &DocId) -> SessionId {
    let ns = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, b"theuth:cells");
    SessionId(uuid::Uuid::new_v5(&ns, doc.0.as_bytes()))
}

/// The document and agent reads note scope needs. [`Live`] asks the running actors.
#[async_trait::async_trait]
pub trait Docs: Send + Sync {
    /// `CodeBlockText`.
    async fn code_block(&self, doc: &DocId, key: &NoteKey) -> Result<NoteSource, String>;
    /// `None` = this engine is the hub; `Some(peer)` = the hub; `Err` = no hub holds the lease.
    async fn hub(&self) -> Result<Option<String>, String>;
    /// The note's markdown (for its ```` ```cells ```` defaults block).
    async fn markdown(&self, doc: &DocId) -> Option<String>;
    /// The note's title.
    async fn title(&self, doc: &DocId) -> Option<String>;
    /// Root sessions bound to a notebook containing the note (144 §8).
    async fn bound_sessions(&self, doc: &DocId) -> Vec<SessionId>;
    /// Upsert one `cell_latest` row (hub).
    async fn put_latest(&self, row: CellLatest) -> Result<(), String>;
    /// The replicated `cell_latest` rows of a note.
    async fn latest(&self, doc: &DocId) -> Result<Vec<CellLatest>, String>;
}

/// The live `DocumentActor` / `Agents`.
pub struct Live;

fn docs_actor() -> Result<document::Handle, String> {
    document::Handle::lookup().ok_or_else(|| "documents are not running".to_string())
}

fn ask_err<M, E: std::fmt::Display>(e: kameo::error::SendError<M, E>) -> String {
    match e {
        kameo::error::SendError::HandlerError(e) => e.to_string(),
        other => format!("documents: {other}"),
    }
}

const OWNER: Principal = Principal::Owner { device: None };

#[async_trait::async_trait]
impl Docs for Live {
    async fn code_block(&self, doc: &DocId, key: &NoteKey) -> Result<NoteSource, String> {
        let msg = document::CodeBlockText {
            doc: doc.clone(),
            key: key.clone(),
        };
        docs_actor()?.actor().ask(msg).await.map_err(ask_err)
    }

    async fn hub(&self) -> Result<Option<String>, String> {
        match docs_actor()?.actor().ask(document::ingest::HubGate).await {
            Ok(()) => Ok(None),
            Err(kameo::error::SendError::HandlerError(s)) => {
                match document::DocError::parse(&s).not_hub() {
                    Some(Some(hub)) => Ok(Some(hub.0.clone())),
                    Some(None) => Err("no engine holds the documents hub".into()),
                    None => Err(s),
                }
            }
            Err(e) => Err(ask_err(e)),
        }
    }

    async fn markdown(&self, doc: &DocId) -> Option<String> {
        let msg = document::Markdown {
            doc: doc.clone(),
            blocks: None,
            annotate: false,
        };
        docs_actor().ok()?.actor().ask(msg).await.ok()
    }

    async fn title(&self, doc: &DocId) -> Option<String> {
        let msg = document::MetaOf {
            doc: doc.clone(),
            by: OWNER,
        };
        docs_actor()
            .ok()?
            .actor()
            .ask(msg)
            .await
            .ok()
            .map(|m| m.title)
    }

    async fn bound_sessions(&self, doc: &DocId) -> Vec<SessionId> {
        let Ok(docs) = docs_actor() else {
            return Vec::new();
        };
        let Ok(books) = docs
            .actor()
            .ask(document::NotebookList { archived: false })
            .await
        else {
            return Vec::new();
        };
        let agents = kameo::actor::ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
            .ok()
            .flatten();
        let mut out = Vec::new();
        for book in books {
            let items = document::NotebookItemsList {
                notebook: book.doc.id.clone(),
                by: OWNER,
            };
            let Ok(items) = docs.actor().ask(items).await else {
                continue;
            };
            let holds = items
                .iter()
                .any(|i| i.kind == document::ItemKind::Note && i.item == doc.0);
            if let (true, Some(agents)) = (holds, &agents)
                && let Ok(chats) =
                    agent::notebooks::notebook_chats(agents, book.doc.id.0.clone()).await
            {
                out.extend(
                    chats
                        .into_iter()
                        .map(|s| s.id)
                        .filter(|s| !out.contains(s))
                        .collect::<Vec<_>>(),
                );
            }
        }
        out
    }

    async fn put_latest(&self, row: CellLatest) -> Result<(), String> {
        docs_actor()?
            .actor()
            .ask(document::cells::PutCellLatest(row))
            .await
            .map_err(ask_err)
    }

    async fn latest(&self, doc: &DocId) -> Result<Vec<CellLatest>, String> {
        docs_actor()?
            .actor()
            .ask(document::cells::CellLatestOf(doc.clone()))
            .await
            .map_err(ask_err)
    }
}

static DOCS: LazyLock<RwLock<Arc<dyn Docs>>> = LazyLock::new(|| RwLock::new(Arc::new(Live)));
static CELLS: RwLock<Option<crate::Cells>> = RwLock::new(None);

fn docs() -> Arc<dyn Docs> {
    DOCS.read().clone()
}

/// Replace the document/agent reads (tests).
pub fn set_docs(d: Arc<dyn Docs>) {
    *DOCS.write() = d;
}

/// Tests that [`set_docs`] hold this: the fake is process-global.
#[cfg(test)]
pub(crate) static DOCS_TEST: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn config() -> Arc<crate::CellsConfig> {
    match &*CELLS.read() {
        Some(c) => Arc::new(c.config().clone()),
        None => Arc::default(),
    }
}

/// Plugin start: keep the handle and install the `document::RunAnnotations` impl so `doc_read`
/// prints each cell's latest run (§4.4, §9.6).
pub fn install(cells: &crate::Cells) {
    *CELLS.write() = Some(cells.clone());
    document::set_run_annotations(Arc::new(Annotations));
}

/// Which engine runs `doc`'s cells.
pub async fn route(doc: &DocId) -> Result<Route, CellsError> {
    let _ = doc; // one documents hub per mesh (69): the lease is engine-wide
    match docs().hub().await {
        Ok(None) => Ok(Route::Local),
        Ok(Some(hub)) => Ok(Route::Hub(hub)),
        Err(e) => Err(CellsError::Failed(e)),
    }
}

/// Read a note cell's current text, language and last author (`CodeBlockText`).
pub async fn source(doc: &DocId, key: &NoteKey) -> Result<NoteSource, CellsError> {
    docs()
        .code_block(doc, key)
        .await
        .map_err(CellsError::NotFound)
}

/// §9.6: the session whose agent last wrote the block; `None` = the owner (or a member) wrote it.
pub async fn origin(doc: &DocId, block: &str) -> Option<SessionId> {
    let src = source(doc, &NoteKey::Block(block.to_owned())).await.ok()?;
    author_session(&src.last_author)
}

fn author_session(p: &Principal) -> Option<SessionId> {
    match p {
        Principal::Agent { session } => Some(SessionId(*session)),
        _ => None,
    }
}

/// The member's pub8 (`doc-member:<pub8>`).
fn pub8(p: &Principal) -> Option<String> {
    p.member().map(|k| k.chars().take(8).collect())
}

/// §13.1 / invariant 6: the member pub8 when a member wrote the block and the owner has not
/// run that text since (`latest` = the cell's latest run). Feeds `CellState.member_edit`.
pub fn member_edit(src: &NoteSource, latest: Option<&CellRun>) -> Option<String> {
    let member = pub8(&src.last_author)?;
    let current = surface::cell::digest(&src.text);
    match latest {
        Some(run) if run.code_sha == current => None,
        _ => Some(member),
    }
}

/// Invariant 6 at press: a member's unrun text needs `member_ack = <pub8>`.
pub fn member_guard(
    src: &NoteSource,
    latest: Option<&CellRun>,
    member_ack: Option<&str>,
) -> Result<(), CellsError> {
    match member_edit(src, latest) {
        Some(p) if member_ack != Some(p.as_str()) => Err(CellsError::MemberAck(p)),
        _ => Ok(()),
    }
}

/// §3.3: the note's defaults from its first ```` ```cells ```` block; `(defaults, second
/// block present)`.
pub fn defaults_of(markdown: &str) -> (NoteDefaults, bool) {
    let mut found: Vec<String> = Vec::new();
    let mut body: Option<String> = None;
    for line in markdown.lines() {
        let t = line.trim_start();
        match &mut body {
            None if t.starts_with("```") && t.trim_start_matches('`').trim() == "cells" => {
                body = Some(String::new());
            }
            Some(b) if t.starts_with("```") => {
                found.push(std::mem::take(b));
                body = None;
            }
            Some(b) => {
                b.push_str(line);
                b.push('\n');
            }
            None => {}
        }
    }
    let second = found.len() > 1;
    let d = found
        .first()
        .map(|b| surface::cell::parse_defaults(b))
        .unwrap_or_default();
    (d, second)
}

/// The note's markdown (its ```` ```cells ```` defaults block, its other cells); `None` when it
/// cannot be read.
pub async fn markdown(doc: &DocId) -> Option<String> {
    docs().markdown(doc).await
}

/// §3.3: a note cell's cwd — absolute (`/…`, `~/…`) as is, relative against the default `cwd`;
/// `None` when there is nothing to resolve against (`cwd_missing` / `cwd_relative`).
pub fn cell_cwd(
    cell_cwd: Option<&str>,
    defaults: &NoteDefaults,
    home: Option<&std::path::Path>,
) -> Option<PathBuf> {
    let abs = |p: &str| -> Option<PathBuf> {
        if p == "~" {
            return home.map(PathBuf::from);
        }
        if let Some(rest) = p.strip_prefix("~/") {
            return home.map(|h| h.join(rest));
        }
        p.starts_with('/').then(|| PathBuf::from(p))
    };
    let base = defaults.cwd.as_deref().and_then(abs);
    let cwd = match cell_cwd {
        Some(c) => abs(c).or_else(|| base.map(|b| b.join(c))),
        None => base,
    };
    cwd.map(|p| lexical(&p))
}

/// 171F4 N6: `.` and `..` resolved by the text alone (no symlink is followed, as before):
/// `/tmp/t/.` → `/tmp/t`, `/tmp/t/a/..` → `/tmp/t`; `..` above the root stays at the root.
pub(crate) fn lexical(p: &std::path::Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push(c);
                }
            }
            c => out.push(c),
        }
    }
    out
}

/// §9.6: deliver a finished note run: the full `<cell_result in="note …">` to the origin
/// session and the body-less quiet line to every other bound notebook chat; then upsert
/// `cell_latest` (§13.2). `pass_end`: the last result of a Run-all pass, which carries
/// `pass="…" pass_end="true"` as in chat (171F4 N6). Returns the full result's text.
pub async fn deliver(run: &CellRun, cleaned: &str, pass_end: bool) -> Result<String, CellsError> {
    let CellRef::Note { doc, key } = &run.cell else {
        return Ok(String::new());
    };
    let cfg = config();
    let d = docs();
    let src = d.code_block(doc, key).await.ok();
    let cell = src
        .as_ref()
        .and_then(|s| surface::cell::parse(&s.language, &s.text));
    // Fail closed: a cell we cannot read again is treated as private.
    let private = cell.as_ref().is_none_or(|c| c.options.private);
    let review = cell.as_ref().is_some_and(|c| c.options.review);
    let name = cell_name(key);

    remember(
        doc,
        &name,
        run,
        if private { "" } else { cleaned },
        private,
        &cfg,
    );
    let row = latest_row(run, doc, &name, cleaned, private, cfg.replicate_bytes);
    if let Err(e) = d.put_latest(row).await {
        tracing::warn!(doc = %doc.0, "cell_latest upsert failed: {e}");
    }

    let title = d.title(doc).await.unwrap_or_default();
    let in_attr = format!("note {} \\\"{}\\\"", doc.0, title.replace('"', "\\\""));
    let origin = src.as_ref().and_then(|s| author_session(&s.last_author));
    let x = crate::deliver::Extra {
        name: run.name.as_deref(),
        private: run.private,
        pass_end,
        ..crate::deliver::Extra::default()
    };
    let full = with_in(&crate::deliver::build(run, cleaned, &cfg, &x), &in_attr);
    if let Some(session) = origin
        && let Err(e) = crate::deliver::deliver(session, run, full.clone(), review).await
    {
        tracing::warn!(%session, "note cell delivery failed: {e}");
    }
    let quiet = quiet_line(run, key, &in_attr);
    for session in d.bound_sessions(doc).await {
        if Some(session) == origin {
            continue;
        }
        if let Err(e) = crate::deliver::deliver(session, run, quiet.clone(), false).await {
            tracing::debug!(%session, "quiet note cell line failed: {e}");
        }
    }
    Ok(full)
}

/// §13.2: the replicated latest runs of `doc` (any engine; membership is checked by the RPC).
pub async fn latest(doc: &DocId) -> Result<Vec<CellLatest>, CellsError> {
    docs().latest(doc).await.map_err(CellsError::Failed)
}

fn cell_name(key: &NoteKey) -> String {
    match key {
        NoteKey::Name(n) | NoteKey::Block(n) => n.clone(),
    }
}

/// §9.1 exit text: the code, the signal name, or `timeout|killed|lost|cancelled`.
fn exit_text(run: &CellRun) -> String {
    match run.state {
        RunState::Timeout => "timeout".into(),
        RunState::Killed => "killed".into(),
        RunState::Lost => "lost".into(),
        RunState::Cancelled => "cancelled".into(),
        _ => match &run.exit {
            Some(ExitStatus::Code(c)) => c.to_string(),
            Some(ExitStatus::Signal(s)) => s.clone(),
            None => "?".into(),
        },
    }
}

fn expect_text(run: &CellRun) -> Option<String> {
    if run.expect.is_empty() {
        return None;
    }
    let missed = run.expect.iter().filter(|e| !e.met).count();
    let n = run.expect.len();
    Some(if missed == 0 {
        format!("met {n}/{n}")
    } else {
        format!("missed {missed}/{n}")
    })
}

/// Put the titled `in="note <doc> \"<title>\""` into the opening tag of a `<cell_result …>`:
/// in place of the builder's bare `in="note <doc>"`, else after `at`, before `pass` (L2).
fn with_in(full: &str, in_attr: &str) -> String {
    let Some(end) = full.find('>') else {
        return full.to_owned();
    };
    let open = &full[..end];
    let attr = format!(" in=\"{in_attr}\"");
    if let Some(start) = open.find(" in=\"note ") {
        let value = start + " in=\"".len();
        let close = open[value..].find('"').map_or(end, |i| value + i + 1);
        return format!("{}{attr}{}", &full[..start], &full[close..]);
    }
    let at = open.find(" pass=\"").unwrap_or(end);
    format!("{}{attr}{}", &full[..at], &full[at..])
}

/// §9.6 / 157: the body-less quiet line for notebook chats — ids and exit only.
fn quiet_line(run: &CellRun, key: &NoteKey, in_attr: &str) -> String {
    let who = match key {
        NoteKey::Name(n) => format!("name=\"{n}\""),
        NoteKey::Block(b) => format!("cell=\"{b}\""),
    };
    format!(
        "<cell_result run=\"{}\" {who} exit=\"{}\" in=\"{in_attr}\"></cell_result>",
        run.id,
        exit_text(run)
    )
}

/// §13.2: the row the hub upserts (`text` = cleaned, head+tail capped; `""` when private).
fn latest_row(
    run: &CellRun,
    doc: &DocId,
    name: &str,
    cleaned: &str,
    private: bool,
    cap: usize,
) -> CellLatest {
    let state = serde_json::to_value(run.state)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();
    let text = if private {
        String::new()
    } else {
        let (masked, _) = crate::redact::mask(&tools::secrets::redact(cleaned), &[]);
        head_tail(&masked, cap)
    };
    CellLatest {
        id: format!("{}:{name}", doc.0),
        doc: doc.clone(),
        cell: name.to_owned(),
        run: run.id.0.clone(),
        state,
        exit: match run.exit {
            Some(ExitStatus::Code(c)) => Some(c),
            _ => None,
        },
        secs: run.secs,
        expect: expect_text(run),
        summary: run.summary.clone(),
        code_sha: run.code_sha.clone(),
        ended_ms: run
            .ended_ms
            .unwrap_or_else(|| chrono::Utc::now().timestamp_millis()),
        text,
    }
}

/// Head (¾) + tail (¼) of `text` within `cap` bytes, the middle replaced by a cut line; the
/// cuts fall on line starts when the windows have newlines.
pub fn head_tail(text: &str, cap: usize) -> String {
    if text.len() <= cap {
        return text.to_owned();
    }
    let marker = |n: usize| format!("… {n} bytes cut …\n");
    let budget = cap.saturating_sub(marker(text.len()).len() + 1);
    let (head, tail) =
        crate::deliver::line_cuts(text, budget * 3 / 4, text.len() - (budget - budget * 3 / 4));
    let nl = if text[..head].ends_with('\n') {
        ""
    } else {
        "\n"
    };
    format!(
        "{}{nl}{}{}",
        &text[..head],
        marker(tail - head),
        &text[tail..]
    )
}

// ---- doc_read run annotations (§4.4, §9.6) ------------------------------------------------------

/// The latest run of each note cell this engine knows: filled by [`deliver`] and, per note on
/// its first `doc_read`, from the run store (in the background: the hook is synchronous).
struct Known {
    run: CellRun,
    /// Cleaned text capped at `export_output_bytes`; `""` for private cells.
    text: String,
    private: bool,
}

type DocRuns = HashMap<String, Known>;
static KNOWN: LazyLock<Mutex<HashMap<DocId, DocRuns>>> = LazyLock::new(Default::default);

fn remember(
    doc: &DocId,
    name: &str,
    run: &CellRun,
    cleaned: &str,
    private: bool,
    cfg: &crate::CellsConfig,
) {
    let known = Known {
        run: run.clone(),
        text: head_tail(cleaned, cfg.export_output_bytes),
        private,
    };
    KNOWN
        .lock()
        .entry(doc.clone())
        .or_default()
        .insert(name.to_owned(), known);
}

/// Fill `doc`'s runs from the store once (newest first; the first run per cell wins).
fn fill(doc: &DocId) {
    if KNOWN.lock().contains_key(doc) {
        return;
    }
    KNOWN.lock().insert(doc.clone(), HashMap::new());
    let (Some(cells), Ok(rt)) = (CELLS.read().clone(), tokio::runtime::Handle::try_current())
    else {
        return;
    };
    let doc = doc.clone();
    rt.spawn(async move {
        let Ok(runs) = cells.runs(Scope::Doc(doc.clone()), 200).await else {
            return;
        };
        let cfg = cells.config().clone();
        for run in runs {
            let CellRef::Note { key, .. } = &run.cell else {
                continue;
            };
            let name = cell_name(key);
            if KNOWN
                .lock()
                .get(&doc)
                .is_some_and(|m| m.contains_key(&name))
            {
                continue;
            }
            let text = cells
                .output(run.id.clone(), 0, cfg.export_output_bytes as u32 * 4, false)
                .await
                .map(|p| p.text)
                .unwrap_or_default();
            // Private runs are refused by `output` (empty text); the flag is re-read at annotate.
            remember(&doc, &name, &run, &text, false, &cfg);
        }
    });
}

struct Annotations;

impl document::RunAnnotations for Annotations {
    fn note_cell(
        &self,
        doc: &DocId,
        block: &str,
        language: &str,
        text: &str,
    ) -> Option<document::cells::CellAnnotation> {
        let cell = surface::cell::parse(language, text)?;
        fill(doc);
        let name = cell
            .options
            .name
            .clone()
            .unwrap_or_else(|| block.to_owned());
        let known = KNOWN.lock();
        let k = known.get(doc)?.get(&name)?;
        let run_line = run_line(&k.run);
        let stale = k.run.code_sha != surface::cell::digest(text);
        let label = match &cell.options.name {
            Some(n) => format!("{} {n}", lang_name(cell.lang)),
            None => lang_name(cell.lang),
        };
        let outline = format!(
            "[cell:{label} · {run_line}{}]",
            if stale {
                " · ran an earlier version"
            } else {
                ""
            }
        );
        let private = k.private || cell.options.private;
        let output = (!private && !k.text.is_empty()).then(|| k.text.clone());
        Some(document::cells::CellAnnotation {
            outline,
            output,
            run_line,
        })
    }
}

/// `bash`, `python`, … (the fence word).
fn lang_name(lang: surface::cell::Lang) -> String {
    format!("{lang:?}").to_lowercase()
}

/// `ran 14:38 · exit 0 · 33 s · expect ✓`.
fn run_line(run: &CellRun) -> String {
    let at = run
        .ended_ms
        .or(run.started_ms)
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(|t| t.with_timezone(&chrono::Local).format("%H:%M").to_string())
        .unwrap_or_else(|| "—".into());
    let mut out = format!("ran {at} · exit {}", exit_text(run));
    if let Some(s) = run.secs {
        if s < 10.0 {
            out.push_str(&format!(" · {s:.1} s"));
        } else {
            out.push_str(&format!(" · {s:.0} s"));
        }
    }
    if !run.expect.is_empty() {
        out.push_str(if run.expect.iter().all(|e| e.met) {
            " · expect ✓"
        } else {
            " · expect ✗"
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Expect, ExpectResult, KernelId, KernelSpec, RunId, RunMode};
    use document::RunAnnotations as _;

    #[test]
    fn kernel_session_is_deterministic_per_doc() {
        let a = kernel_session(&DocId("doc-a".into()));
        assert_eq!(a, kernel_session(&DocId("doc-a".into())));
        assert_ne!(a, kernel_session(&DocId("doc-b".into())));
        assert_eq!(a.0.get_version_num(), 5);
    }

    fn src(text: &str, by: Principal) -> NoteSource {
        NoteSource {
            block: "b1".into(),
            language: "bash".into(),
            text: text.into(),
            last_author: by,
        }
    }

    fn run(doc: &str, key: NoteKey, code: &str) -> CellRun {
        CellRun {
            id: RunId("r_1".into()),
            cell: CellRef::Note {
                doc: DocId(doc.into()),
                key,
            },
            mode: RunMode::Cell,
            pass: None,
            kernel: KernelId {
                scope: Scope::Doc(DocId(doc.into())),
                spec: KernelSpec::Monty,
            },
            interpreter: "Monty · sandboxed".into(),
            cwd: "/tmp".into(),
            env_names: Vec::new(),
            presented_sha: surface::cell::digest(code),
            code_sha: surface::cell::digest(code),
            edited: false,
            pressed_by: "desktop".into(),
            state: RunState::Done,
            exit: Some(ExitStatus::Code(0)),
            secs: Some(33.2),
            expect: vec![ExpectResult {
                item: Expect::Text("ok".into()),
                met: true,
            }],
            summary: Some("ok".into()),
            output_bytes: Some(2),
            queued_ms: 0,
            started_ms: Some(1),
            ended_ms: Some(1_700_000_000_000),
            masked: 0,
            private: false,
            name: None,
            input: None,
        }
    }

    const ALICE: &str = "QUxJQ0VLRVlBTElDRUtFWQ";

    #[test]
    fn member_text_needs_the_ack_until_the_owner_ran_it() {
        let text = "#| name: x\necho hi";
        let member = src(
            text,
            Principal::Member {
                pub_: ALICE.into(),
                device: None,
            },
        );
        assert_eq!(member_edit(&member, None), Some("QUxJQ0VL".into()));
        assert_eq!(
            member_guard(&member, None, None),
            Err(CellsError::MemberAck("QUxJQ0VL".into()))
        );
        assert_eq!(member_guard(&member, None, Some("QUxJQ0VL")), Ok(()));
        assert!(member_guard(&member, None, Some("someone")).is_err());
        // The owner already ran exactly this text: no ack needed.
        let ran = run("d", NoteKey::Name("x".into()), text);
        assert_eq!(member_guard(&member, Some(&ran), None), Ok(()));
        // The member edited after the owner's run: ack again.
        let edited = src(
            "#| name: x\necho bye",
            Principal::Member {
                pub_: ALICE.into(),
                device: None,
            },
        );
        assert!(member_guard(&edited, Some(&ran), None).is_err());
        // Owner or agent text needs nothing.
        assert_eq!(member_guard(&src(text, OWNER), None, None), Ok(()));
        let agent = src(
            text,
            Principal::Agent {
                session: uuid::Uuid::nil(),
            },
        );
        assert_eq!(member_guard(&agent, None, None), Ok(()));
        assert_eq!(
            author_session(&agent.last_author),
            Some(SessionId(uuid::Uuid::nil()))
        );
        assert_eq!(author_session(&OWNER), None);
    }

    #[test]
    fn defaults_block_and_cwd_resolution() {
        let md = "# Runbook\n\n```cells\n#| cwd: ~/dr\n#| shell: /opt/homebrew/bin/bash\n#| python: .venv\n```\n\ntext\n\n```cells\n#| cwd: /other\n```\n";
        let (d, second) = defaults_of(md);
        assert_eq!(d.cwd.as_deref(), Some("~/dr"));
        assert_eq!(d.shell.as_deref(), Some("/opt/homebrew/bin/bash"));
        assert_eq!(d.python, Some(crate::types::PythonChoice::Venv));
        assert!(second);
        let home = std::path::Path::new("/Users/sky");
        assert_eq!(
            cell_cwd(Some("stacks/a"), &d, Some(home)),
            Some(PathBuf::from("/Users/sky/dr/stacks/a"))
        );
        assert_eq!(
            cell_cwd(None, &d, Some(home)),
            Some(PathBuf::from("/Users/sky/dr"))
        );
        assert_eq!(
            cell_cwd(Some("/abs"), &d, Some(home)),
            Some(PathBuf::from("/abs"))
        );
        let none = NoteDefaults::default();
        assert_eq!(cell_cwd(Some("rel"), &none, Some(home)), None);
        assert_eq!(cell_cwd(None, &none, Some(home)), None);
        // 171F4 N6: `.` / `..` under the default resolve lexically.
        let tmp = NoteDefaults {
            cwd: Some("/tmp/cells-test".into()),
            ..NoteDefaults::default()
        };
        assert_eq!(
            cell_cwd(Some("."), &tmp, Some(home)),
            Some(PathBuf::from("/tmp/cells-test"))
        );
        assert_eq!(
            cell_cwd(Some("./a/../b/."), &tmp, Some(home)),
            Some(PathBuf::from("/tmp/cells-test/b"))
        );
        assert_eq!(
            cell_cwd(Some(".."), &tmp, Some(home)),
            Some(PathBuf::from("/tmp"))
        );
        assert_eq!(
            cell_cwd(Some("~/x/.."), &none, Some(home)),
            Some(PathBuf::from("/Users/sky"))
        );
        assert_eq!(defaults_of("no block").0, NoteDefaults::default());
    }

    #[test]
    fn head_tail_stays_within_the_cap_on_char_boundaries() {
        assert_eq!(head_tail("short", 100), "short");
        let long = "é".repeat(5000) + "END";
        let out = head_tail(&long, 1000);
        assert!(out.len() <= 1000, "{}", out.len());
        assert!(out.ends_with("END"));
        assert!(out.contains("bytes cut"));
    }

    /// 171F B17: cuts fall on line starts.
    #[test]
    fn head_tail_cuts_on_lines() {
        let long: String = (0..2000).map(|i| format!("line {i}\n")).collect();
        let out = head_tail(&long, 1000);
        assert!(out.len() <= 1000, "{}", out.len());
        assert!(out.starts_with("line 0\n"));
        assert!(out.ends_with("line 1999\n"));
        for l in out.lines() {
            assert!(l.starts_with("line ") || l.contains("bytes cut"), "{l:?}");
        }
    }

    #[test]
    fn result_attributes_and_run_line() {
        let r = run("d1", NoteKey::Name("preview".into()), "x");
        let in_attr = "note d1 \\\"Runbook\\\"";
        assert_eq!(
            quiet_line(&r, &NoteKey::Name("preview".into()), in_attr),
            "<cell_result run=\"r_1\" name=\"preview\" exit=\"0\" in=\"note d1 \\\"Runbook\\\"\"></cell_result>"
        );
        let full = "<cell_result run=\"r_1\" name=\"preview\" exit=\"0\" at=\"14:31\" pass=\"p1\" pass_end=\"true\">\nbody\n</cell_result>";
        assert_eq!(
            with_in(full, in_attr),
            "<cell_result run=\"r_1\" name=\"preview\" exit=\"0\" at=\"14:31\" in=\"note d1 \\\"Runbook\\\"\" pass=\"p1\" pass_end=\"true\">\nbody\n</cell_result>"
        );
        assert!(
            with_in("<cell_result run=\"r\">x</cell_result>", in_attr)
                .starts_with("<cell_result run=\"r\" in=\"note d1")
        );
        // The builder's bare `in="note d1"` gives way to the titled one.
        assert_eq!(
            with_in(
                "<cell_result run=\"r\" in=\"note d1\" pass=\"p\">x</cell_result>",
                in_attr
            ),
            "<cell_result run=\"r\" in=\"note d1 \\\"Runbook\\\"\" pass=\"p\">x</cell_result>"
        );
        let line = run_line(&r);
        assert!(
            line.starts_with("ran ") && line.ends_with(" · exit 0 · 33 s · expect ✓"),
            "{line}"
        );
        let mut t = r.clone();
        t.state = RunState::Timeout;
        t.expect[0].met = false;
        t.secs = Some(1.25);
        assert!(
            run_line(&t).ends_with(" · exit timeout · 1.2 s · expect ✗"),
            "{}",
            run_line(&t)
        );
    }

    /// A fake document side: one block, recorded upserts, one bound chat.
    struct Fake {
        block: NoteSource,
        puts: Mutex<Vec<CellLatest>>,
    }

    #[async_trait::async_trait]
    impl Docs for Fake {
        async fn code_block(&self, _: &DocId, _: &NoteKey) -> Result<NoteSource, String> {
            Ok(self.block.clone())
        }
        async fn hub(&self) -> Result<Option<String>, String> {
            Ok(Some("engine-hub".into()))
        }
        async fn markdown(&self, _: &DocId) -> Option<String> {
            Some("```cells\n#| cwd: /srv\n```\n".into())
        }
        async fn title(&self, _: &DocId) -> Option<String> {
            Some("Runbook".into())
        }
        async fn bound_sessions(&self, _: &DocId) -> Vec<SessionId> {
            Vec::new()
        }
        async fn put_latest(&self, row: CellLatest) -> Result<(), String> {
            self.puts.lock().push(row);
            Ok(())
        }
        async fn latest(&self, _: &DocId) -> Result<Vec<CellLatest>, String> {
            Ok(self.puts.lock().clone())
        }
    }

    /// One test owns the process-global fake (the hooks are free functions).
    #[tokio::test]
    async fn hooks_against_a_fake_document_side() {
        let _one = DOCS_TEST.lock().await;
        let agent = uuid::Uuid::from_u128(7);
        let text = "#| name: preview\n#| private\necho AKIA";
        let fake = Arc::new(Fake {
            block: src(text, Principal::Agent { session: agent }),
            puts: Mutex::default(),
        });
        set_docs(fake.clone());
        let doc = DocId("d-fake".into());
        assert_eq!(route(&doc).await, Ok(Route::Hub("engine-hub".into())));
        assert_eq!(
            source(&doc, &NoteKey::Name("preview".into()))
                .await
                .unwrap()
                .text,
            text
        );
        assert_eq!(origin(&doc, "b1").await, Some(SessionId(agent)));
        let md = markdown(&doc).await.unwrap();
        assert_eq!(defaults_of(&md).0.cwd.as_deref(), Some("/srv"));

        // A private cell replicates no text; the row id is "<doc>:<name>".
        let r = run("d-fake", NoteKey::Name("preview".into()), text);
        deliver(&r, "secret output", false).await.unwrap();
        let rows = latest(&doc).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            (rows[0].id.as_str(), rows[0].text.as_str()),
            ("d-fake:preview", "")
        );
        assert_eq!((rows[0].state.as_str(), rows[0].exit), ("done", Some(0)));
        assert_eq!(rows[0].expect.as_deref(), Some("met 1/1"));
        // ... and doc_read shows the run line but no output fence.
        let ann = Annotations.note_cell(&doc, "b1", "bash", text).unwrap();
        assert!(
            ann.outline.starts_with("[cell:bash preview · ran "),
            "{}",
            ann.outline
        );
        assert_eq!(ann.output, None);

        // A public cell replicates capped text and exports it.
        let public = "#| name: preview\necho hi";
        fake.puts.lock().clear();
        let r = run("d-fake", NoteKey::Name("preview".into()), public);
        set_docs(Arc::new(Fake {
            block: src(public, OWNER),
            puts: Mutex::default(),
        }));
        deliver(&r, &"x".repeat(40_000), false).await.unwrap();
        let rows = latest(&doc).await.unwrap();
        assert!(rows[0].text.len() <= crate::CellsConfig::default().replicate_bytes);
        let ann = Annotations.note_cell(&doc, "b1", "bash", public).unwrap();
        assert!(ann.output.unwrap().len() <= crate::CellsConfig::default().export_output_bytes);
        assert!(!ann.outline.contains("earlier version"));
        let stale = Annotations
            .note_cell(&doc, "b1", "bash", "#| name: preview\necho changed")
            .unwrap();
        assert!(stale.outline.contains("ran an earlier version"));
        // Not a cell → no annotation.
        assert!(
            Annotations
                .note_cell(&doc, "b1", "bash", "echo plain")
                .is_none()
        );
        set_docs(Arc::new(Live));
    }
}
