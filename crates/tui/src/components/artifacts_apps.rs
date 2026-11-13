//! Artifact/app grouping, transcript cards and asynchronous source-file effects.
use std::{collections::HashMap, path::PathBuf};
use agent::{HistoryEntry, SessionSummary};
use rpc_compose::api::clients::{ArtifactArtifactsApiClient, AgentSessionsApiClient};
use crate::model::{Entry, Grouped, grouped_first, grouped_step, project_order, project_heading, short, markdown_rows, output_markdown, result_line};
use super::*;

/// Owner-local artifact card, including async expansion state.
pub struct Card {
    /// Authoritative artifact receipt joined to this call.
    pub artifact: artifact::Artifact,
    /// Current expanded file/source or failed raw tool result.
    pub expanded: Option<Box<markdown::Text>>,
    opening: bool,
    read_epoch: u64,
}
/// Grouped artifact/app picker.
pub struct Picker {
    /// Effective project, session and name/version rows.
    pub items: Vec<Grouped<artifact::Artifact>>,
    /// Highlighted row.
    pub cursor: usize,
    /// This picker lists only app sources.
    pub apps: bool,
}
/// Artifact capability and per-transcript projections.
#[derive(Default)]
pub(crate) struct ArtifactsApps {
    /// The boot-time app plugin capability; sources stay readable when false.
    pub enabled: bool,
    /// Joined cards keyed by stable call identity.
    pub cards: HashMap<String, Card>,
    /// Streamed argument summaries keyed by pending call identity.
    pub progress: HashMap<String, Progress>,
    /// Current grouped picker, separate from core modal state.
    pub menu: Option<Picker>,
    /// Fullscreen hydration/grouping read due on the next tick.
    pub refresh: bool,
    /// Replaced transcripts cannot accept old hydration/body replies.
    pub generation: u64,
}
/// Existing owner operation, retained as typed effect metadata for bindings.
#[derive(Clone)]
pub(crate) enum Request {
    /// List the selected tree; optionally open a filtered picker.
    ListArtifacts { session: SessionId, pick: bool, apps: bool },
    /// Export a blob for its authorized private/durable owner.
    ExportArtifact { id: String, name: String, session: Option<SessionId> },
    /// Open a harness-local path through the detached OS handler.
    OpenPath(PathBuf),
    /// Read inline source bytes off the UI thread.
    Read { artifact: artifact::Artifact, call: Option<String>, generation: u64, epoch: u64 },
}
/// Typed artifact completion or borrowed-core projection.
pub(crate) enum Msg {
    /// Live authoritative receipt.
    Shown(artifact::ArtifactShown),
    /// Owner list and grouping metadata; the transcript generation fences hydration.
    Listed { session: SessionId, generation: u64, pick: bool, apps: bool, result: Result<(Vec<artifact::Artifact>, Vec<SessionSummary>), String> },
    /// File/sidecar bytes, including inline read errors rather than generic command failure.
    Body { artifact: artifact::Artifact, call: Option<String>, generation: u64, epoch: u64, bytes: Result<Vec<u8>, String> },
    /// Open/export completion; only exports return a display line.
    Done(Result<Option<String>, String>),
    /// Latest arguments of a current pending core call.
    Progress(String, Progress),
    /// A core tool start/result replaces its pending-argument summary.
    Settled(String),
    /// Reconcile an open picker's project grouping.
    Metadata,
}
impl ArtifactsApps {
    /// Construct the owner from the boot-time capability, before registration.
    pub fn new(enabled: bool) -> Self { Self { enabled, ..Self::default() } }
    fn request(&self, request: Request, cx: &mut Cx<'_, '_, Msg>) {
        let generation = self.generation;
        cx.spawn_request(Lane::Concurrent, request, move |api, request| async move {
            match request {
                Request::ListArtifacts { session, pick, apps } => {
                    let result = async {
                        let mut items = api.list_artifacts(Some(session), Some(true)).await.map_err(crate::reason)?;
                        if apps { items.retain(|a| a.kind == artifact::Kind::App); }
                        let sessions = if pick && !items.is_empty() { api.list_sessions().await.map_err(crate::reason)? } else { Vec::new() };
                        Ok((items, sessions))
                    }.await;
                    Msg::Listed { session, generation, pick, apps, result }
                }
                Request::ExportArtifact { id, name, session } => Msg::Done(export_artifact(&api, id, name, session).await.map(Some)),
                Request::OpenPath(path) => Msg::Done(open_path(&path).map(|_| None)),
                Request::Read { artifact, call, generation, epoch } => {
                    let bytes = match artifact_read_path(&artifact) {
                        Some(path) => tokio::fs::read(path).await.map_err(|e| e.to_string()),
                        None => Ok(Vec::new()),
                    };
                    Msg::Body { artifact, call, generation, epoch, bytes }
                }
            }
        });
    }
    fn attach(&mut self, artifact: artifact::Artifact, core: &CoreView<'_>) {
        if !core.entries.iter().any(|entry| matches!(entry, Entry::Tool { call_id, .. } if *call_id == artifact.call_id)) { return; }
        match self.cards.get_mut(&artifact.call_id) {
            Some(card) => card.artifact = artifact,
            None => { self.cards.insert(artifact.call_id.clone(), Card { artifact, expanded: None, opening: false, read_epoch: 0 }); }
        }
    }
    /// The newest joined artifact in the current exchange, for the existing open shortcut.
    #[cfg(test)]
    pub fn artifact_path<'a>(&'a self, entries: &[Entry]) -> Option<&'a std::path::Path> {
        entries.iter().rev().take_while(|entry| !matches!(entry, Entry::User { .. }))
            .find_map(|entry| entry.key().and_then(|key| self.cards.get(key)).map(|card| card.artifact.path.as_path()))
    }
    fn owner(core: &CoreView<'_>, sessions: &[SessionSummary], id: SessionId) -> Option<Owner> {
        if let Some(session) = sessions.iter().find(|session| session.id == id) {
            return Some(Owner { project: session.project.clone(), label: format!("{}  {}", short(id), session.title.chars().take(40).collect::<String>()) });
        }
        if core.session == Some(id) {
            return Some(Owner { project: core.project.map(str::to_owned), label: format!("{}  {}", short(id), core.session_name.unwrap_or("this session")) });
        }
        core.members.member(id).map(|member| Owner { project: member.project.or(core.project).map(str::to_owned), label: format!("{}  {}", short(id), member.name) })
    }
    fn fence(cx: &mut Cx<'_, '_, Msg>) {
        if cx.core.reasoning { cx.plain(Plain::Text("\x1b[0m\n".into())); }
    }
}
impl Component for ArtifactsApps {
    type Msg = Msg;
    fn declaration(&self) -> Declaration {
        Declaration { id: "artifacts-apps", bindings: &[],
            slash: &[("/artifacts", "", "files the agent showed (picker: ⏎ opens, s shows inline, e exports to ~/Downloads)"), ("/apps", "", "interactive apps the agent built, run on desktop/web (picker: ⏎ opens, s shows source, e exports to ~/Downloads)")],
            interests: &[Interest { topic: "artifact.shown", version: 1, variants: &["ArtifactShown"] }, Interest { topic: "agent.events", version: 1, variants: &["ToolCallProgress", "ToolCallStarted", "ToolCallFinished", "SessionMetadataChanged"] }],
        }
    }
    fn slash_visible(&self, name: &str) -> bool { name != "/apps" || self.enabled }
    fn decode(&self, event: &TopicEvent, core: &CoreView<'_>) -> Option<Msg> {
        if ((event.topic() == Some("artifact.shown") && event.version() == Some(1)) || event.topic().is_none()) && event.event()["type"] == "ArtifactShown" {
            return serde::Deserialize::deserialize(event.event()).ok().map(Msg::Shown);
        }
        match core.agent? {
            agent::AgentEvent::SessionMetadataChanged { .. } => Some(Msg::Metadata),
            agent::AgentEvent::ToolCallProgress { session, call_id, args, .. } if core.session == Some(*session) && core.entries.iter().any(|entry| matches!(entry, Entry::Pending { call_id: current, .. } if current == call_id)) => Some(Msg::Progress(call_id.clone(), Progress { show: surface::partial_show(args), bytes: args.len() })),
            agent::AgentEvent::ToolCallStarted { session, call_id, .. } | agent::AgentEvent::ToolCallFinished { session, call_id, .. } if core.session == Some(*session) => Some(Msg::Settled(call_id.clone())),
            _ => None,
        }
    }
    fn update(&mut self, input: Input<'_, Msg>, cx: &mut Cx<'_, '_, Msg>) {
        match input {
            Input::Slash { name: "/apps", .. } if !self.enabled => cx.core(CoreAction::Notice("apps are off (app.enabled); /artifacts lists their sources".into())),
            Input::Slash { name, .. } => match cx.core.session {
                None => cx.core(CoreAction::Notice("no session".into())),
                Some(session) => self.request(Request::ListArtifacts { session, pick: true, apps: name == "/apps" }, cx),
            },
            Input::Message(Msg::Shown(event)) => {
                Self::fence(cx);
                let a = &event.artifact;
                cx.plain(Plain::Line(if a.kind == artifact::Kind::App { format!("[app: {} · {APP_WHERE} → source {}]", a.name, a.path.display()) } else { format!("[artifact: {} → {}]", a.name, a.path.display()) }));
                if cx.core.session == Some(event.session) { self.attach(event.artifact, &cx.core); }
            }
            Input::Message(Msg::Listed { session, generation, pick, apps, result }) => match result {
                Err(error) => cx.error(error),
                Ok((items, sessions)) => {
                    Self::fence(cx);
                    if items.is_empty() { cx.plain(Plain::Line(if apps { "no apps" } else { "no artifacts" }.into())); }
                    else { for item in &items { cx.plain(Plain::Line(artifact_row(item))); } }
                    if generation != self.generation || cx.core.session != Some(session) { return; }
                    if !pick { for item in items { self.attach(item, &cx.core); } }
                    else if items.is_empty() { cx.core(CoreAction::Notice(if apps { "no apps" } else { "no artifacts" }.into())); }
                    else {
                        let items = artifact_rows(items, |id| Self::owner(&cx.core, &sessions, id));
                        let cursor = self.menu.as_ref().map_or_else(|| grouped_first(&items), |menu| menu.cursor);
                        self.menu = Some(Picker { items, cursor, apps }); cx.focus(FocusClass::Modal);
                    }
                }
            },
            Input::Message(Msg::Body { artifact, call, generation, epoch, bytes }) => {
                if generation != self.generation { return; }
                if let Some(call) = call {
                    if let Some(card) = self.cards.get_mut(&call) && card.read_epoch == epoch && card.opening && card.artifact.id == artifact.id {
                        card.expanded = Some(Box::new(artifact_text(&artifact, bytes.as_deref().map_err(String::as_str), &cx.core.markdown_options()))); card.opening = false;
                    }
                } else {
                    let text = artifact_text(&artifact, bytes.as_deref().map_err(String::as_str), &cx.core.markdown_options());
                    cx.core(CoreAction::AppendCard(TranscriptCard { entry_key: None, group: "artifacts-inline", key: uuid::Uuid::new_v4(), glyph: "▣", color: crate::ui::TOOL, title: artifact.name, body: text, expanded: true, plain: false }));
                }
            }
            Input::Message(Msg::Done(result)) => match result {
                Ok(Some(text)) => { Self::fence(cx); cx.plain(Plain::Line(text.clone())); cx.core(CoreAction::Notice(text)); }
                Ok(None) => {}, Err(error) => cx.error(error),
            },
            Input::Message(Msg::Progress(call, progress)) => { self.progress.insert(call, progress); }
            Input::Message(Msg::Settled(call)) => { self.progress.remove(&call); }
            Input::Message(Msg::Metadata) => self.refresh |= self.menu.is_some(),
            Input::Lifecycle(Lifecycle::SessionReset | Lifecycle::TranscriptReplaced) => { self.generation = self.generation.wrapping_add(1); self.cards.clear(); self.progress.clear(); self.refresh = false; if matches!(input, Input::Lifecycle(Lifecycle::SessionReset)) { self.menu = None; } }
            Input::Lifecycle(Lifecycle::FocusLost) => self.menu = None,
            Input::Lifecycle(Lifecycle::History(loaded)) => self.refresh |= loaded.history.iter().any(|entry| matches!(entry, HistoryEntry::Tool { name, .. } if name == "show" || name == "app")),
            Input::Lifecycle(Lifecycle::Resync) => self.refresh = cx.core.session.is_some(),
            Input::Tick { .. } if std::mem::take(&mut self.refresh) => {
                if cx.core.interactive && let Some(session) = cx.core.session { self.request(Request::ListArtifacts { session, pick: self.menu.is_some(), apps: self.menu.as_ref().is_some_and(|menu| menu.apps) }, cx); }
            }
            Input::EntryAction { key, operation } if self.cards.contains_key(key) => {
                match operation {
                    EntryOperation::Open => self.request(Request::OpenPath(self.cards[key].artifact.path.clone()), cx),
                    EntryOperation::Toggle => {
                        let result = cx.core.entries.iter().find_map(|entry| match entry { Entry::Tool { call_id, result: Some(result), .. } if call_id == key => Some(result), _ => None });
                        let Some((output, error)) = result else { return };
                        let card = self.cards.get_mut(key).unwrap(); card.read_epoch = card.read_epoch.wrapping_add(1);
                        let request = if card.expanded.take().is_some() || card.opening { card.opening = false; None }
                        else if *error { card.expanded = Some(Box::new(markdown::Text::complete(&output_markdown(output)))); None }
                        else { card.opening = true; Some(Request::Read { artifact: card.artifact.clone(), call: Some(key.to_owned()), generation: self.generation, epoch: card.read_epoch }) };
                        if let Some(request) = request { self.request(request, cx); }
                    }
                }
                cx.consume_key();
            }
            Input::Key(key) if self.menu.is_some() => {
                let menu = self.menu.as_mut().unwrap();
                match key.code {
                    KeyCode::Up => menu.cursor = grouped_step(&menu.items, menu.cursor, -1),
                    KeyCode::Down => menu.cursor = grouped_step(&menu.items, menu.cursor, 1),
                    KeyCode::Esc => { self.menu = None; cx.close(); }
                    KeyCode::Enter | KeyCode::Char('e' | 's') => {
                        let picked = menu.items.get(menu.cursor).and_then(Grouped::item).cloned(); self.menu = None; cx.close();
                        if let Some(a) = picked { let request = match key.code {
                            KeyCode::Enter => Request::OpenPath(a.path),
                            KeyCode::Char('e') => Request::ExportArtifact { id: a.id, name: a.name, session: cx.core.private_member(a.session).then_some(a.session) },
                            _ => Request::Read { artifact: a, call: None, generation: self.generation, epoch: 0 },
                        }; self.request(request, cx); }
                    }
                    _ => {}
                }
                cx.consume_key();
            }
            _ => {}
        }
    }
    fn entry_openable(&self, key: &str) -> bool { self.cards.contains_key(key) }
    fn entry_height(&self, key: &str, width: usize) -> Option<usize> {
        let card = self.cards.get(key)?;
        Some(6 + usize::from(card.artifact.title != card.artifact.name) + card.expanded.as_ref().map_or(0, |text| markdown_rows(text, width.saturating_sub(4))))
    }
    fn wake(&self) -> Option<u64> { self.refresh.then_some(0) }
    fn view(&self, at: Surface<'_>, cx: &ViewCx<'_>) -> Option<View> {
        match at {
            Surface::EntrySummary { key } => {
                let progress = self.progress.get(key)?;
                let name = cx.core.entries.iter().find_map(|entry| match entry { Entry::Pending { call_id, name, .. } if call_id == key => Some(name.as_str()), _ => None })?;
                Some(View::Text(progress_line(name, progress)))
            }
            Surface::Modal => {
                let menu = self.menu.as_ref()?;
                let title = if menu.apps { format!("apps · {APP_WHERE}  ⏎ open  s show source  e export to ~/Downloads  esc close") } else { "artifacts  ⏎ open  s show inline  e export to ~/Downloads  esc close".into() };
                let rows: Vec<String> = menu.items.iter().map(|row| match row { Grouped::Heading(heading) => heading.clone(), Grouped::Item(a) => format!("    {}", artifact_row(a)) }).collect();
                Some(View::Element { rows: rows.len().min(10) + 3, replace: true, element: crate::ui::menu_view(title, rows, menu.cursor, cx.width as u16, cx.height as u16, false) })
            }
            Surface::Entry { key, .. } => {
                use iocraft::prelude::{View as BoxView, *};
                let card = self.cards.get(key)?; let a = &card.artifact; let expanded = &card.expanded;
                let result = cx.core.entries.iter().find_map(|entry| match entry { Entry::Tool { call_id, result, .. } if call_id == key => Some(result), _ => None })?;
                let mut header = format!("▣ {}", a.name);
                if a.version > 1 { header.push_str(&format!(" · v{}", a.version)); }
                if a.kind == artifact::Kind::Archive && let Some((output, false)) = result && let Some(files) = output.split(" · ").map(|part| part.trim_end_matches(')')).find(|part| part.ends_with(" files")) { header.push_str(&format!(" · {files}")); }
                header.push_str(&format!(" · {}", artifact_size(a)));
                if a.kind == artifact::Kind::App { header.push_str(&format!(" · {APP_WHERE}")); }
                let error = match result { Some((output, true)) => Some(format!("✗ {}", result_line(output))), _ => None };
                let hint = if expanded.is_some() || card.opening { "ctrl+o collapses" } else { "o open · ctrl+o" };
                let path = a.path.display().to_string();
                let link = vec![MixedTextContent::new(&path).color(crate::ui::SAND).decoration(TextDecoration::Underline).hyperlink(format!("file://{path}")), MixedTextContent::new(format!("   {hint}")).color(crate::ui::SAND)];
                Some(super::View::Element { rows: self.entry_height(key, cx.width).unwrap(), replace: true, element: element! {
                    BoxView(flex_direction: FlexDirection::Column, border_style: BorderStyle::Round, border_color: crate::ui::TOOL, padding_left: 1, padding_right: 1, margin_top: 1, margin_bottom: 1) {
                        Text(content: header, color: crate::ui::TOOL, weight: Weight::Bold)
                        #((a.title != a.name).then(|| element!(Text(content: a.title.clone(), wrap: TextWrap::Wrap))))
                        #(error.map(|error| element!(Text(content: error, color: crate::ui::CARNELIAN, wrap: TextWrap::Wrap))))
                        MixedText(contents: link, wrap: TextWrap::Wrap)
                        #(expanded.as_ref().map(|text| element!(BoxView(flex_direction: FlexDirection::Column, margin_top: 1) { #(text.view().lines().map(crate::ui::line)) })))
                    }
                }.into_any() })
            }
            _ => None,
        }
    }
}

/// A `Pending` call's streamed arguments so far.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Progress {
    /// For `show`: the file and whether `content` is being written.
    pub show: Option<surface::PartialShow>,
    /// Cumulative argument bytes.
    pub bytes: usize,
}

/// The spinner row of a `Pending` call with progress: `writing report.pdf… 3.2 KB`
/// when `show` carries `content`, `showing report.pdf…` when it names a path,
/// `building <name>… 3.2 KB` for `app` (its `name` streams first),
/// `preparing <tool>… 3.2 KB` for every other tool.
pub fn progress_line(tool: &str, p: &Progress) -> String {
    let size = bytes_label(p.bytes as u64);
    match (tool, &p.show) {
        ("show", Some(s)) => match (&s.name, s.writing) {
            (Some(n), true) => format!("writing {n}… {size}"),
            (Some(n), false) => format!("showing {n}…"),
            (None, true) => format!("writing… {size}"),
            (None, false) => "preparing show…".to_owned(),
        },
        ("app", Some(s)) => match &s.name {
            Some(n) => format!("building {n}… {size}"),
            None => format!("building app… {size}"),
        },
        _ => format!("preparing {tool}… {size}"),
    }
}


/// What `/artifacts` knows about an owner: its effective project and a label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owner {
    /// Effective project of the owner's root.
    pub project: Option<String>,
    /// Heading text: `short-id  title`, a peer name, or `Unavailable session`.
    pub label: String,
}

/// `/artifacts` rows grouped effective project → session → name/version
/// (51 §3). `owner` resolves an artifact's session; `None` is a missing
/// durable owner, shown as `Unavailable session` under Ungrouped.
pub fn artifact_rows(
    items: Vec<artifact::Artifact>,
    owner: impl Fn(SessionId) -> Option<Owner>,
) -> Vec<Grouped<artifact::Artifact>> {
    // Sessions in first-seen order (the list is newest first), each with its rows.
    let mut sessions: Vec<(SessionId, Owner, Vec<artifact::Artifact>)> = Vec::new();
    for a in items {
        match sessions.iter_mut().find(|(s, ..)| *s == a.session) {
            Some((_, _, rows)) => rows.push(a),
            None => {
                let o = owner(a.session).unwrap_or(Owner {
                    project: None,
                    label: "Unavailable session".to_owned(),
                });
                sessions.push((a.session, o, vec![a]));
            }
        }
    }
    for (_, _, rows) in &mut sessions {
        rows.sort_by(|a, b| a.name.cmp(&b.name).then(a.version.cmp(&b.version)));
    }
    let order = project_order(sessions.iter().map(|(_, o, _)| o.project.as_deref()));
    let mut out = Vec::new();
    for project in order {
        out.push(Grouped::Heading(project_heading(project)));
        for (_, o, rows) in sessions
            .iter()
            .filter(|(_, o, _)| o.project.as_deref() == project)
        {
            out.push(Grouped::Heading(format!("  {}", o.label)));
            out.extend(rows.iter().cloned().map(Grouped::Item));
        }
    }
    out
}

/// Where an app runs; the TUI only reads its source (46 §8.2).
pub const APP_WHERE: &str = "interactive on desktop/web";
/// Rows an inline artifact body shows at most (35 §6.1).
const INLINE_LINES: usize = 200;
/// Rows of a CSV artifact shown inline.
const CSV_ROWS: usize = 20;

/// Bytes as `512 B` / `3.2 KB` / `1.4 MB`.
pub fn bytes_label(bytes: u64) -> String {
    match bytes {
        b if b < 1024 => format!("{b} B"),
        b if b < 1024 * 1024 => format!("{:.1} KB", b as f64 / 1024.0),
        b => format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)),
    }
}


/// The kind-specific size of an artifact card header (35 §6.1): pages for a
/// compiled PDF, bytes otherwise. Drawn every frame, so nothing here touches
/// the file; the line count shows in the expanded body.
pub fn artifact_size(a: &artifact::Artifact) -> String {
    match (a.kind, a.pages) {
        (artifact::Kind::Pdf, Some(p)) => format!("{p} pages · {}", bytes_label(a.bytes)),
        _ => bytes_label(a.bytes),
    }
}

/// One `/artifacts` row: `▣ name · kind · size · vN · 2m ago`.
pub fn artifact_row(a: &artifact::Artifact) -> String {
    let age = chrono::DateTime::from_timestamp(a.created_at, 0)
        .map(|t| chrono::Utc::now().signed_duration_since(t))
        .map_or_else(String::new, |d| {
            let secs = d.num_seconds().max(0);
            match secs {
                s if s < 60 => format!("{s}s ago"),
                s if s < 3600 => format!("{}m ago", s / 60),
                s if s < 86400 => format!("{}h ago", s / 3600),
                s => format!("{}d ago", s / 86400),
            }
        });
    format!(
        "▣ {} · {} · {} · v{} · {age}",
        a.name,
        a.kind.as_str(),
        bytes_label(a.bytes),
        a.version
    )
}

/// The first [`INLINE_LINES`] lines of `s` and how many were cut.
fn cap_lines(s: &str) -> (String, usize) {
    let total = s.lines().count();
    if total <= INLINE_LINES {
        return (s.trim_end().to_owned(), 0);
    }
    let kept: Vec<&str> = s.lines().take(INLINE_LINES).collect();
    (kept.join("\n"), total - INLINE_LINES)
}

/// `body` in a code fence tagged `lang`; the fence grows past any backtick run inside.
fn fenced(lang: &str, body: &str) -> String {
    let longest = body
        .lines()
        .map(|l| l.chars().take_while(|c| *c == '`').count())
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    format!("{fence}{lang}\n{body}\n{fence}")
}

/// A CSV cell as a markdown table cell: pipes escaped, newlines flattened.
fn md_cell(s: &str) -> String {
    s.replace('|', "\\|").replace(['\n', '\r'], " ")
}


/// The file an expanded card reads (35 §6.1): the blob for text-like kinds,
/// the listing sidecar for an archive, nothing for pictures and downloads.
pub(crate) fn artifact_read_path(a: &artifact::Artifact) -> Option<std::path::PathBuf> {
    use artifact::Kind;
    match a.kind {
        Kind::Markdown | Kind::Text | Kind::Html | Kind::Csv | Kind::App | Kind::Mermaid => {
            Some(a.path.clone())
        }
        Kind::Archive => Some(artifact::archive::sidecar(&a.path)),
        Kind::Image | Kind::Svg | Kind::Pdf | Kind::File => None,
    }
}

/// Rows of an expanded archive card.
const ARCHIVE_ROWS: usize = 50;

/// Render bytes read by the owner effect (also used by search previews).
/// `Err` is why the file could not be read.
pub(crate) fn artifact_text(
    a: &artifact::Artifact,
    bytes: Result<&[u8], &str>,
    opts: &markdown::Options,
) -> markdown::Text {
    use artifact::Kind;
    let ext = a.path.extension().map_or("", |e| e.to_str().unwrap_or(""));
    let path = a.path.display().to_string();
    let unreadable = |e: &dyn std::fmt::Display| format!("cannot read {path}: {e}");
    let md = match a.kind {
        Kind::Markdown | Kind::Text | Kind::Html | Kind::App => match bytes {
            Err(e) => unreadable(&e),
            Ok(b) => {
                let s = String::from_utf8_lossy(b);
                let (body, cut) = cap_lines(&s);
                let text = match a.kind {
                    Kind::Markdown => body,
                    Kind::App => format!("[app] {APP_WHERE} · source:\n\n{}", fenced(ext, &body)),
                    _ => fenced(ext, &body),
                };
                match cut {
                    0 => text,
                    n => format!("{text}\n\n… (+{n} lines, o opens)"),
                }
            }
        },
        Kind::Csv => match bytes
            .map_err(str::to_owned)
            .and_then(|b| artifact::kind::csv_table(b, ext == "tsv", CSV_ROWS))
        {
            Err(e) => unreadable(&e),
            Ok((header, rows, total)) => {
                let cols = header.len().max(1);
                let row = |cells: &[String]| {
                    let mut line = String::from("|");
                    for i in 0..cols {
                        line.push(' ');
                        line.push_str(&cells.get(i).map_or_else(String::new, |c| md_cell(c)));
                        line.push_str(" |");
                    }
                    line
                };
                let mut out = row(&header);
                out.push('\n');
                out.push_str(&format!("|{}", " --- |".repeat(cols)));
                for r in &rows {
                    out.push('\n');
                    out.push_str(&row(r));
                }
                if total > rows.len() {
                    out.push_str(&format!("\n\n… (+{} rows)", total - rows.len()));
                }
                out
            }
        },
        // A `mermaid` fence: the assistant's fence hook draws it through
        // `surface::mermaid_lines`, the plain code block when that declines.
        Kind::Mermaid => match bytes {
            Err(e) => unreadable(&e),
            Ok(b) => fenced("mermaid", String::from_utf8_lossy(b).trim_end()),
        },
        Kind::Archive => match bytes.map_err(str::to_owned).and_then(|b| {
            serde_json::from_slice::<artifact::archive::ArchiveListing>(b)
                .map_err(|e| e.to_string())
        }) {
            Err(_) => format!(
                "{} · no listing (archive unreadable) · {path}",
                bytes_label(a.bytes)
            ),
            Ok(l) => {
                let mut out = format!(
                    "{} files · {} unpacked · {} · {path}\n",
                    l.total_entries,
                    bytes_label(l.total_size),
                    bytes_label(a.bytes)
                );
                for e in l.entries.iter().take(ARCHIVE_ROWS) {
                    out.push_str(&format!(
                        "\n    {:>9}  {}",
                        if e.is_dir {
                            String::new()
                        } else {
                            bytes_label(e.size)
                        },
                        e.path
                    ));
                }
                if l.total_entries > ARCHIVE_ROWS as u64 {
                    out.push_str(&format!(
                        "\n\n… (+{} entries, o opens)",
                        l.total_entries - ARCHIVE_ROWS as u64
                    ));
                }
                out
            }
        },
        Kind::Image => format!("[image] {path}"),
        Kind::Svg => format!("[svg] {path}"),
        Kind::Pdf => format!("{} · {path}", artifact_size(a)),
        Kind::File => path.clone(),
    };
    let mut t = markdown::Text::with(opts.clone());
    t.push(&md);
    t.finish();
    t.reveal_all();
    t
}


/// Detached harness-local open operation, shared by artifact and search effects.
pub(crate) fn open_path(path: &std::path::Path) -> Result<(), String> {
    platform::open::open_path(path).map_err(|error| format!("could not open {}: {error}", path.display()))
}
/// Existing export destination and display line, shared by artifact and search effects.
pub(crate) async fn export_artifact(api: &rpc::client::Engine, id: String, name: String, session: Option<runtime::SessionId>) -> Result<String, String> {
    use rpc_compose::api::clients::ArtifactArtifactsApiClient;
    let dest = std::env::home_dir().ok_or_else(|| "no home directory".to_owned())?.join("Downloads").join(name);
    let path = api.export_artifact(id, dest, session).await.map_err(crate::reason)?;
    Ok(format!("exported → {}", path.display()))
}
