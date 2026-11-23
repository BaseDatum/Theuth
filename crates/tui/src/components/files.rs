//! 199 CONTACT-FILES: the file lines of `<message_file …/>` placeholders, notices and
//! ended-share rows, their live state (`ContactFileChanged`), and the actions on a file.
//! Lines are keyed cards (`file:<holder>:<id>`); every value is engine-written but may
//! carry a contact's text, so it is shown without controls, bidi controls or Markdown.
use std::path::Path;
use agent::communication::files::{ContactFile, FileDirection, FileState, InState, OutState, notice_id};
use rpc_compose::api::clients::{AgentMessagingApiClient, NetworkApiClient};
use crate::model::{CardKind, Entry, attr, unescape};
use super::*;

/// D17: the ended-share row's label.
const ENDED: &str = " · not given to the agent";
/// An element that does not parse: never the raw markup.
pub(crate) const UNREADABLE: &str = "File notice (unreadable)";
/// The end of the message row's receiving hint (`files::placeholders`).
const RECEIVING_HINT: &str = " are arriving; a notice follows when each one is saved or fails.";
/// The start of every notice row's hint line (`files::notice`).
const NOTICE_HINT: &str = "File notice from theuth";
const PICKER: &str = "files  ⏎ open  r reveal  y copy path  s save to ~/Downloads  f fetch here  esc close";

/// One file as its line shows it: from an engine-written element or a typed `ContactFile`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Line {
    pub holder: String,
    pub id: String,
    pub from: String,
    pub name: String,
    pub media_type: String,
    pub bytes: Option<u64>,
    pub done: u64,
    pub state: FileState,
    pub saved: Option<String>,
    pub reason: Option<String>,
    pub machine: Option<String>,
}

/// Remote text on one line: no control characters (line breaks included) and no bidi controls.
pub(crate) fn text(s: &str) -> String {
    s.chars().filter(|c| !c.is_control() && !matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')).collect()
}

/// The card group of a file line the engine's notice row (or ended-share row) drew; the
/// message's own placeholder lines are `"file"` (UI-1: those are the live lines).
const NOTICE_GROUP: &str = "file-notice";

impl Line {
    /// One `<message_file …/>` element; `None` when it is malformed.
    pub(crate) fn parse(element: &str) -> Option<Self> {
        let open = format!(" {}", element.strip_prefix("<message_file ")?.strip_suffix("/>")?);
        let get = |key: &str| attr(&open, &format!(" {key}=\"")).map(|v| text(&unescape(v))).filter(|v| !v.is_empty());
        let hex = |key: &str| get(key).filter(|v| v.bytes().all(|b| b.is_ascii_hexdigit()));
        let state = match get("state")?.as_str() {
            "receiving" => FileState::In(InState::Receiving),
            "received" => FileState::In(InState::Received),
            "failed" => FileState::In(InState::Failed),
            _ => return None,
        };
        Some(Self {
            holder: hex("holder")?, id: hex("file")?, from: get("from").unwrap_or_default(), name: get("name")?,
            media_type: get("type").unwrap_or_default(), bytes: get("bytes").and_then(|b| b.parse().ok()), done: 0, state,
            saved: get("saved"), reason: get("reason"), machine: get("machine"),
        })
    }
    /// The stable entry key every line of this file shares.
    pub(crate) fn key(&self) -> String { format!("file:{}:{}", self.holder, self.id) }
    fn status(&self) -> String {
        let pct = || self.bytes.filter(|n| *n > 0 && self.done > 0).map_or_else(String::new, |n| format!(" {}%", self.done.saturating_mul(100) / n));
        let failed = || self.reason.as_deref().map_or_else(|| "failed".to_owned(), |r| format!("failed · {r}"));
        match self.state {
            FileState::In(InState::Queued) => "queued".into(),
            FileState::In(InState::Receiving) => format!("receiving{}", pct()),
            FileState::In(InState::Publishing) => "saving".into(),
            FileState::In(InState::Saved | InState::Received) => "saved".into(),
            FileState::Out(OutState::Queued) => "offered".into(),
            FileState::Out(OutState::Sending) if self.done > 0 => format!("sending{} · {}", pct(), platform::fmt::bytes(self.done)),
            FileState::Out(OutState::Sending) => "sending".into(),
            FileState::Out(OutState::Sent) => "sent".into(),
            FileState::In(InState::Failed) | FileState::Out(OutState::Failed) => failed(),
        }
    }
    /// `name · size · type · state`, and the ended-share label.
    pub(crate) fn title(&self, ended: bool) -> String {
        let mut title = self.name.clone();
        if let Some(n) = self.bytes { title.push_str(&format!(" · {}", platform::fmt::bytes(n))); }
        if !self.media_type.is_empty() { title.push_str(&format!(" · {}", self.media_type)); }
        title.push_str(&format!(" · {}", self.status()));
        if ended { title.push_str(ENDED); }
        title
    }
    /// The expanded card: where the file is and who holds it.
    fn body(&self) -> String {
        let mut lines = Vec::new();
        if !self.from.is_empty() { lines.push(format!("from {}", self.from)); }
        if let Some(saved) = &self.saved { lines.push(format!("saved {saved}")); }
        if let Some(machine) = &self.machine { lines.push(format!("on {machine}")); }
        lines.push(format!("holder {} · file {}", self.holder, self.id));
        lines.join("\n")
    }
    fn kind(&self, group: &'static str) -> CardKind {
        let (glyph, color) = match self.state {
            FileState::In(InState::Failed) | FileState::Out(OutState::Failed) => ("✕", crate::ui::CARNELIAN),
            FileState::In(InState::Saved | InState::Received) | FileState::Out(OutState::Sent) => ("▤", crate::ui::MALACHITE),
            _ => ("▤", crate::ui::FAIENCE),
        };
        CardKind::Component { group, key: notice_id(&self.holder, &self.id), glyph, color, plain: true }
    }
    /// The keyed transcript card of this line.
    pub(crate) fn card(&self, ended: bool, group: &'static str) -> TranscriptCard {
        let CardKind::Component { group, key, glyph, color, plain } = self.kind(group) else { unreachable!("a file line is a component card") };
        TranscriptCard { entry_key: Some(self.key()), group, key, glyph, color, plain, title: self.title(ended),
            body: markdown::Text::complete(&self.body()), expanded: false }
    }
}

impl From<&ContactFile> for Line {
    fn from(f: &ContactFile) -> Self {
        Self {
            holder: text(&f.holder), id: text(&f.id), from: text(&f.contact), name: text(&f.name), media_type: text(&f.media_type),
            bytes: f.size, done: f.done, state: f.state, saved: f.path.as_ref().map(|p| text(&p.display().to_string())),
            reason: f.error.as_deref().map(text), machine: Some(text(&f.machine)).filter(|m| !m.is_empty()),
        }
    }
}

fn entry(line: Option<Line>, ended: bool, group: &'static str) -> Entry {
    line.map_or_else(|| Entry::Error(UNREADABLE.into()), |line| line.card(ended, group).into())
}

/// UI-1: the message's placeholder line is the live line of its file. A notice line (or an
/// ended-share line) of a file whose message line is above it in `entries` moves that line to
/// the notice's state (in history the placeholder still says receiving) and becomes one
/// compact note (its title and whether the agent was given it), never a second card; with no
/// message line loaded (paged out, or only the notice here) it stays the full line.
pub(crate) fn fold_notices(entries: &mut [Entry]) {
    let mut lines = std::collections::HashMap::new();
    for j in 0..entries.len() {
        let (key, group) = match &entries[j] {
            Entry::Card { entry_key: Some(k), kind: CardKind::Component { group, .. }, .. } => (k.clone(), *group),
            _ => continue,
        };
        if group == "file" {
            lines.insert(key, j);
            continue;
        }
        let (true, Some(&i)) = (group == NOTICE_GROUP, lines.get(&key)) else { continue };
        let Entry::Card { kind: notice_kind, title, text, .. } = std::mem::replace(&mut entries[j], Entry::Info(String::new())) else { continue };
        if let Entry::Card { kind, title: live, text: body, .. } = &mut entries[i] {
            *live = title.strip_suffix(ENDED).unwrap_or(&title).to_owned();
            *body = text;
            if let (CardKind::Component { glyph, color, .. }, CardKind::Component { glyph: g, color: c, .. }) = (notice_kind, kind) {
                (*g, *c) = (glyph, color);
            }
        }
        let given = if title.ends_with(ENDED) { "" } else { " · given to the agent" };
        entries[j] = Entry::Info(format!("↳ file notice · {title}{given}"));
    }
}

/// The `<message_file …/>` lines and the receiving hint after a `</message>` close (D5), and the rest.
fn tail(text: &str) -> (Vec<Option<Line>>, &str) {
    let mut files = Vec::new();
    let mut rest = text;
    loop {
        let t = rest.trim_start();
        let Some(end) = t.strip_prefix("<message_file ").and(t.find("/>")) else { break };
        files.push(Line::parse(&t[..end + 2]));
        rest = &t[end + 2..];
    }
    if !files.is_empty() {
        let t = rest.trim_start();
        let (line, after) = t.split_once('\n').unwrap_or((t, ""));
        if line.starts_with("Files from ") && line.ends_with(RECEIVING_HINT) { rest = after; }
    }
    (files, rest)
}

/// `injected`: a message wrapper's placeholder tail belongs to it, never a row of its own.
pub(crate) fn skip(text: &str) -> &str { tail(text).1 }

/// The file lines of every message wrapper's placeholder tail in a stored row.
pub(crate) fn placeholders(text: &str) -> Vec<Entry> {
    text.match_indices("</message>").flat_map(|(at, close)| tail(&text[at + close.len()..]).0).map(|line| entry(line, false, "file")).collect()
}

/// A notice row (one element, then its fixed hint line) as its file line; `None` for other text.
pub(crate) fn notice(text: &str) -> Option<Entry> {
    let text = text.trim();
    if !text.starts_with("<message_file ") { return None }
    let Some(end) = text.find("/>") else { return text.contains(NOTICE_HINT).then(|| Entry::Error(UNREADABLE.into())) };
    text[end + 2..].trim_start().starts_with(NOTICE_HINT).then(|| entry(Line::parse(&text[..end + 2]), false, NOTICE_GROUP))
}

/// An `errors` row (history, or a live `AgentEvent::Error`) that opens with `<message_file`: the
/// D17 ended-share notice as its labelled line, or [`UNREADABLE`]. `None` for any other error.
pub(crate) fn ended(text: &str) -> Option<Entry> {
    let text = text.trim_start();
    text.starts_with("<message_file").then(|| entry(text.find("/>").and_then(|end| Line::parse(&text[..end + 2])), true, NOTICE_GROUP))
}

/// A file entry as one plain line: its glyph and title, or [`UNREADABLE`].
fn plain_line(e: Entry) -> String {
    match e {
        Entry::Card { kind, title, .. } => format!("{} {title}", kind.glyph()),
        _ => UNREADABLE.into(),
    }
}

/// A notice row as its plain line (the composer's queue, the non-TTY printer); `None` for
/// other text.
pub(crate) fn notice_line(text: &str) -> Option<String> {
    notice(text).map(plain_line)
}

/// An ended-share `errors` row as its labelled plain line; `None` for any other error.
pub(crate) fn ended_line(text: &str) -> Option<String> {
    ended(text).map(plain_line)
}

/// The non-TTY printer's text of a row: every message wrapper's placeholder tail (the
/// elements and the receiving hint) as one plain line per file, never the markup.
pub(crate) fn plain(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains("<message_file ") {
        return text.into();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("</message>") {
        let end = at + "</message>".len();
        out.push_str(&rest[..end]);
        let (files, after) = tail(&rest[end..]);
        let any = !files.is_empty();
        for f in files {
            out.push('\n');
            out.push_str(&plain_line(entry(f, false, "file")));
        }
        if any && !after.is_empty() {
            out.push('\n');
        }
        rest = after;
    }
    out.push_str(rest);
    out.into()
}

/// `ContactFileChanged`: every line of the file takes its new state. An outgoing file's first
/// event adds its line under the `send_message` call whose receipt names the exchange
/// (`<to> <state> id=<exchange>`), after that call's earlier lines.
pub(crate) fn changed(entries: &mut Vec<Entry>, file: &ContactFile) {
    let line = Line::from(file);
    let key = line.key();
    let mut found = false;
    for entry in entries.iter_mut() {
        if let Entry::Card { entry_key: Some(k), kind, title, .. } = entry && *k == key {
            *title = line.title(title.ends_with(ENDED));
            let group = match kind { CardKind::Component { group, .. } => *group, _ => "file" };
            *kind = line.kind(group);
            found = true;
        }
    }
    if found || file.state.direction() != FileDirection::Out { return }
    let receipt = format!("id={}", file.exchange);
    let Some(at) = entries.iter().rposition(|e| matches!(e, Entry::Tool { name, result: Some((output, _)), .. }
        if name.trim_start_matches("↳ ") == "send_message" && output.split_whitespace().any(|w| w == receipt))) else { return };
    let after = entries[at + 1..].iter().take_while(|e| e.key().is_some_and(|k| k.starts_with("file:"))).count();
    entries.insert(at + 1 + after, line.card(false, "file").into());
}

fn parse_key(key: &str) -> Option<(&str, &str)> { key.strip_prefix("file:")?.split_once(':') }

/// Every file line of the transcript, the newest line of each file, in transcript order.
fn rows(entries: &[Entry]) -> Vec<(&str, &str)> {
    let mut seen = std::collections::HashSet::new();
    let mut rows: Vec<_> = entries.iter().rev().filter_map(|e| match e {
        Entry::Card { entry_key: Some(k), title, .. } if k.starts_with("file:") && seen.insert(k.as_str()) => Some((k.as_str(), title.as_str())),
        _ => None,
    }).collect();
    rows.reverse();
    rows
}

/// An action on one file line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Act { Open, Reveal, Copy, Save, Fetch }

/// One action, resolved through the holder (`contact_file`). The bytes stay where they are:
/// open, reveal and save need them on this machine; another machine fetches them first (P2).
/// `open` launches the registered app: only `Open` and `Reveal` reach it (SEC-A2: a fetch
/// only saves and reports the path; opening a contact's file is always its own action).
async fn act(api: &Engine, session: Option<SessionId>, holder: String, id: String, act: Act,
    open: &(dyn Fn(&Path) -> Result<(), String> + Send + Sync)) -> Result<String, String> {
    if act == Act::Fetch {
        let session = session.ok_or("no session")?;
        let path = api.contact_file_fetch(session, holder, id).await.map_err(crate::reason)?;
        return Ok(format!("fetched → {} · ⏎ opens it", path.display()));
    }
    let file = api.contact_file(holder.clone(), id.clone()).await.map_err(crate::reason)?;
    let name = text(&file.name);
    let here = api.mesh_status().await.map_or(true, |mesh| mesh.self_id.0 == file.holder);
    let path = match (here, act) {
        (true, _) | (false, Act::Copy) => file.path.clone().ok_or_else(|| format!("{name}: no saved file"))?,
        // Asked to open on another machine: its fetched copy (a verified cache hit once fetched).
        (false, Act::Open) => api.contact_file_fetch(session.ok_or("no session")?, holder.clone(), id.clone()).await.map_err(crate::reason)?,
        (false, _) => return Err(format!("{name} is stored on {} · f fetches it here", text(&file.machine))),
    };
    match act {
        Act::Open => open(&path).map(|()| format!("opened {}", path.display())),
        Act::Reveal => open(path.parent().unwrap_or(&path)).map(|()| format!("revealed {}", path.display())),
        Act::Copy => arboard::Clipboard::new().and_then(|mut clip| clip.set_text(path.display().to_string()))
            .map(|()| format!("copied {}", path.display())).map_err(|e| format!("could not copy the path: {e}")),
        Act::Save => {
            let base = Path::new(&name).file_name().ok_or_else(|| format!("{name}: no file name"))?;
            let dest = std::env::home_dir().ok_or("no home directory")?.join("Downloads").join(base);
            let out = api.contact_file_export(holder, id, dest).await.map_err(crate::reason)?;
            Ok(format!("saved → {}", out.display()))
        }
        Act::Fetch => unreachable!("handled above"),
    }
}

/// The `/files` picker and the actions on file lines (`o` opens the newest one).
#[derive(Default)]
pub(crate) struct Files {
    /// The open picker's cursor.
    cursor: Option<usize>,
}
pub(crate) enum Msg { Done(Result<String, String>) }
impl Files {
    fn run(key: &str, which: Act, cx: &mut Cx<'_, '_, Msg>) {
        let Some((holder, id)) = parse_key(key) else { return };
        let (holder, id, session) = (holder.to_owned(), id.to_owned(), cx.core.session);
        cx.spawn(Lane::Concurrent, move |api| async move { Msg::Done(act(&api, session, holder, id, which, &artifacts_apps::open_path).await) });
    }
}
impl Component for Files {
    type Msg = Msg;
    fn declaration(&self) -> Declaration {
        Declaration { id: "files", interests: &[], bindings: &[],
            slash: &[("/files", "", "files sent and received in this chat (picker: ⏎ opens, r reveals, y copies the path, s saves to ~/Downloads, f fetches here)")] }
    }
    fn decode(&self, _: &TopicEvent, _: &CoreView<'_>) -> Option<Msg> { None }
    fn update(&mut self, input: Input<'_, Msg>, cx: &mut Cx<'_, '_, Msg>) {
        let entries = cx.core.entries;
        match input {
            Input::Slash { .. } if rows(entries).is_empty() => cx.core(CoreAction::Notice("no files in this chat".into())),
            Input::Slash { .. } => { self.cursor = Some(rows(entries).len() - 1); cx.focus(FocusClass::Modal); }
            Input::Key(key) if self.cursor.is_some() => {
                let rows = rows(entries);
                let cursor = self.cursor.unwrap_or_default().min(rows.len().saturating_sub(1));
                let which = match key.code {
                    KeyCode::Up => { self.cursor = Some(cursor.saturating_sub(1)); None }
                    KeyCode::Down => { self.cursor = Some((cursor + 1).min(rows.len().saturating_sub(1))); None }
                    KeyCode::Esc => { self.cursor = None; cx.close(); None }
                    KeyCode::Enter => Some(Act::Open),
                    KeyCode::Char('r') => Some(Act::Reveal),
                    KeyCode::Char('y') => Some(Act::Copy),
                    KeyCode::Char('s') => Some(Act::Save),
                    KeyCode::Char('f') => Some(Act::Fetch),
                    _ => None,
                };
                if let Some(which) = which && let Some((key, _)) = rows.get(cursor) {
                    self.cursor = None;
                    cx.close();
                    Self::run(key, which, cx);
                }
                cx.consume_key();
            }
            Input::EntryAction { key, operation: EntryOperation::Open } if key.starts_with("file:") => { Self::run(key, Act::Open, cx); cx.consume_key(); }
            Input::Message(Msg::Done(Ok(line))) => cx.core(CoreAction::Notice(line)),
            Input::Message(Msg::Done(Err(error))) => cx.error(error),
            Input::Lifecycle(Lifecycle::FocusLost | Lifecycle::SessionReset | Lifecycle::TranscriptReplaced) => self.cursor = None,
            _ => {}
        }
    }
    fn view(&self, at: Surface<'_>, cx: &ViewCx<'_>) -> Option<View> {
        let (Surface::Modal, Some(cursor)) = (at, self.cursor) else { return None };
        let rows: Vec<String> = rows(cx.core.entries).into_iter().map(|(_, title)| format!("    {title}")).collect();
        Some(View::IntrinsicElement { rows: rows.len().min(10) + 3, replace: true,
            element: crate::ui::menu_view(PICKER.into(), rows, cursor, cx.width as u16, cx.height as u16, false) })
    }
    fn entry_openable(&self, key: &str) -> bool { key.starts_with("file:") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent::communication::files as engine;
    use agent::{AgentEvent, HistoryEntry, PeerOrigin};
    use crate::model::{Model, UiEvent};

    const HOLDER: &str = "5f3a5f3a5f3a5f3a5f3a5f3a5f3a5f3a5f3a5f3a5f3a5f3a5f3a5f3a5f3a5f3a";
    const EXCHANGE: uuid::Uuid = uuid::Uuid::from_u128(0x0193_f0a2_6b1c_7d3e_9f40_1a2b_3c4d_5e6f);

    fn file(id: &str, name: &str, media_type: &str, size: u64, state: FileState) -> ContactFile {
        ContactFile {
            id: id.into(), holder: HOLDER.into(), state, exchange: EXCHANGE, contact_id: "rec".into(), contact: "maya".into(),
            name: name.into(), media_type: media_type.into(), size: Some(size), blake3: Some("7c1e".repeat(16)), done: 0,
            error: None, path: None, machine: "Sky's MacBook Pro".into(), deadline: chrono::Utc::now(),
        }
    }
    fn zip(state: FileState) -> ContactFile { file("9d0c4b7e21a35f6880e1c2d3b4a59687", "site-survey.zip", "application/zip", 1_200_000_000, state) }
    fn pdf(state: FileState) -> ContactFile { file("41b8d2f0c7e65a1394b2a8d0e3f1c5a7", "drawings.pdf", "application/pdf", 24_100_000, state) }
    fn history(row: HistoryEntry) -> Vec<Entry> { crate::model::from_history(row, &markdown::Options::default(), &mut crate::components::builtin()) }
    fn user(text: String, peer: Option<PeerOrigin>) -> HistoryEntry {
        HistoryEntry::User { text, turn: Some(1), context: None, peer, vision: None, channel: None }
    }
    /// `(key, glyph, title, body)` of a file line.
    fn line(entry: &Entry) -> (&str, &str, &str, &str) {
        match entry {
            Entry::Card { entry_key: Some(key), kind: kind @ CardKind::Component { group: "file" | NOTICE_GROUP, plain: true, .. }, title, text, .. } => (key, kind.glyph(), title, text.raw()),
            other => panic!("not a file line: {other:?}"),
        }
    }
    fn no_markup(entries: &[Entry]) {
        for entry in entries {
            let shown = match entry { Entry::Card { title, text, .. } => format!("{title}\n{}", text.raw()), other => format!("{other:?}") };
            assert!(!shown.contains("<message_file") && !shown.contains("File notice from"), "{shown}");
        }
    }

    /// D5: the message row's placeholders follow the peer card as receiving lines; the hint
    /// and the markup never show, and the live echo of that row is still one wrapper.
    #[test]
    fn a_message_rows_placeholders_are_receiving_lines_under_its_card() {
        let text = format!("<message from=\"peer:maya\" id=\"{EXCHANGE}\" at=\"14:31\">\nSite survey and the old drawings, as promised.\n</message>\n{}",
            engine::placeholders("peer:maya", &[zip(FileState::In(InState::Receiving)), pdf(FileState::In(InState::Receiving))]));
        let origin = PeerOrigin { id: EXCHANGE, from: "peer:maya".into(), reply_to: None, scope: None, contact_mode: Some(agent::ShareMode::Full) };
        let out = history(user(text.clone(), Some(origin)));
        let [Entry::Card { kind: CardKind::Component { group: "peer", .. }, text: words, .. }, zip_line, pdf_line] = out.as_slice() else { panic!("{out:?}") };
        assert_eq!(words.raw(), "Site survey and the old drawings, as promised.");
        assert_eq!(line(zip_line), (&*format!("file:{HOLDER}:9d0c4b7e21a35f6880e1c2d3b4a59687"), "▤",
            "site-survey.zip · 1.2 GB · application/zip · receiving", &*format!("from peer:maya\nholder {HOLDER} · file 9d0c4b7e21a35f6880e1c2d3b4a59687")));
        assert_eq!(line(pdf_line).2, "drawings.pdf · 24.1 MB · application/pdf · receiving");
        no_markup(&out);
        // The live turn that consumed it is one `<message>` wrapper (`TurnStarted` hides it).
        assert_eq!(crate::model::injected(&text, |_| None).map(|cards| cards.len()), Some(1));
    }

    /// D5: a notice row is its file line, never a `you` row: received with its path, failed with its reason.
    #[test]
    fn notice_rows_are_received_and_failed_lines() {
        let mut received = zip(FileState::In(InState::Received));
        received.path = Some("/Users/sky/.theuth/files/received/maya/2026-11-17/site-survey.zip".into());
        let out = history(user(engine::notice("peer:maya", &received).unwrap(), None));
        let [saved] = out.as_slice() else { panic!("{out:?}") };
        let (_, glyph, title, body) = line(saved);
        assert_eq!((glyph, title), ("▤", "site-survey.zip · 1.2 GB · application/zip · saved"));
        assert!(body.contains("saved /Users/sky/.theuth/files/received/maya/2026-11-17/site-survey.zip\non Sky's MacBook Pro"), "{body}");
        let mut failed = pdf(FileState::In(InState::Failed));
        failed.error = Some("the sender's file changed".into());
        let out = history(user(engine::notice("peer:maya", &failed).unwrap(), None));
        let [failed] = out.as_slice() else { panic!("{out:?}") };
        let (_, glyph, title, _) = line(failed);
        assert_eq!((glyph, title), ("✕", "drawings.pdf · application/pdf · failed · the sender's file changed"));
        // A human typing the tag keeps a `you` row: only the engine's hint makes a notice.
        assert!(matches!(history(user("<message_file name=\"x\"/> hi".into(), None)).as_slice(), [Entry::User { .. }]));
    }

    /// D17: an ended share's `errors` row is the same line, labelled, in history and live;
    /// a malformed element is "File notice (unreadable)"; other errors are untouched.
    #[test]
    fn ended_share_rows_are_labelled_lines_and_malformed_ones_unreadable() {
        let mut received = zip(FileState::In(InState::Received));
        received.path = Some("/tmp/site-survey.zip".into());
        let ended = engine::ended_notice("peer:maya", &received).unwrap();
        let out = history(HistoryEntry::Error { text: ended.clone(), context: None });
        no_markup(&out);
        assert_eq!(line(&out[0]).2, "site-survey.zip · 1.2 GB · application/zip · saved · not given to the agent");
        let malformed = "<message_file from=\"peer:maya\" name=\"a.zip\" state=\"bogus\" holder=\"5f\" file=\"9d\"/>";
        assert!(matches!(history(HistoryEntry::Error { text: malformed.into(), context: None }).as_slice(), [Entry::Error(e)] if e == UNREADABLE));
        assert!(matches!(history(user(format!("{malformed}\n{NOTICE_HINT}: no reply to the contact is needed."), None)).as_slice(), [Entry::Error(e)] if e == UNREADABLE));
        assert!(matches!(history(HistoryEntry::Error { text: "rate limited".into(), context: None }).as_slice(), [Entry::Error(e)] if e == "rate limited"));

        let session = runtime::SessionId::new();
        let mut model = Model::new("m".into(), true);
        model.session = Some(session);
        model.apply(UiEvent::Wire(super::super::tests::topic("agent.events", 1, serde_json::to_value(AgentEvent::Error { session, turn: None, message: ended }).unwrap())));
        assert_eq!(line(&model.entries[0]).2, "site-survey.zip · 1.2 GB · application/zip · saved · not given to the agent");
    }

    /// Names and reasons are remote: no controls, no bidi controls, no line breaks, and the
    /// escaped attribute values read as written.
    #[test]
    fn remote_values_lose_controls_and_bidi() {
        let mut failed = pdf(FileState::In(InState::Failed));
        failed.name = "in\u{202E}voice\u{1b}[2J.pdf".into();
        failed.error = Some("disk \"full\" <on>\nb & c".into());
        let notice = engine::notice("peer:maya", &failed).unwrap();
        let line = Line::parse(&notice[..notice.find("/>").unwrap() + 2]).unwrap();
        assert_eq!(line.title(false), "invoice[2J.pdf · application/pdf · failed · disk \"full\" <on>b & c");
    }

    /// Live: `PeerDelivered.files` follow the peer card as their own lines.
    #[test]
    fn a_live_delivery_shows_its_files_under_the_card() {
        let session = runtime::SessionId::new();
        let mut model = Model::new("m".into(), true);
        model.session = Some(session);
        model.apply(UiEvent::Wire(super::super::tests::topic("agent.peer-delivered", 1, serde_json::to_value(agent::PeerDelivered {
            session, origin: PeerOrigin { id: EXCHANGE, from: "peer:maya".into(), reply_to: None, scope: None, contact_mode: Some(agent::ShareMode::Full) },
            text: "Site survey".into(), seq: 1, turn: runtime::TurnId::new(), files: vec![zip(FileState::In(InState::Queued))],
        }).unwrap())));
        assert!(matches!(model.entries.as_slice(), [Entry::Card { kind: CardKind::Component { group: "peer", .. }, .. }, file]
            if line(file).2 == "site-survey.zip · 1.2 GB · application/zip · queued"), "{:?}", model.entries);
    }

    /// `ContactFileChanged`: a receiving line shows progress and its terminal state; an
    /// outgoing file's lines appear under the `send_message` call that offered them and
    /// move through offered, sending, sent and failed.
    #[test]
    fn contact_file_changed_moves_receiving_and_outgoing_lines() {
        let session = runtime::SessionId::new();
        let mut model = Model::new("m".into(), true);
        model.session = Some(session);
        let changed = |model: &mut Model, file: ContactFile| model.apply(UiEvent::Wire(super::super::tests::topic("agent.events", 1,
            serde_json::to_value(AgentEvent::ContactFileChanged { session, file }).unwrap())));
        model.entries.extend(placeholders(&format!("</message>\n{}", engine::placeholders("peer:maya", &[zip(FileState::In(InState::Receiving))]))));
        let mut receiving = zip(FileState::In(InState::Receiving));
        receiving.done = 456_000_000;
        changed(&mut model, receiving);
        assert_eq!(line(&model.entries[0]).2, "site-survey.zip · 1.2 GB · application/zip · receiving 38%");

        let call = Entry::Tool { call_id: "c1".into(), name: "send_message".into(), args: Vec::new(), expanded: None,
            result: Some((format!("contact:maya accepted id={EXCHANGE}"), false)) };
        model.entries.push(call);
        model.entries.push(Entry::Info("after".into()));
        let titles = |model: &Model| model.entries.iter().filter_map(|e| match e { Entry::Card { title, .. } => Some(title.clone()), _ => None }).collect::<Vec<_>>();
        changed(&mut model, pdf(FileState::Out(OutState::Queued)));
        let mut notes = file(&"ab".repeat(16), "notes.md", "text/markdown", 1_200_000, FileState::Out(OutState::Sending));
        notes.done = 456_000;
        changed(&mut model, notes.clone());
        assert!(matches!(&model.entries[2..], [Entry::Card { .. }, Entry::Card { .. }, Entry::Info(_)]), "{:?}", model.entries);
        assert_eq!(titles(&model)[1..], ["drawings.pdf · 24.1 MB · application/pdf · offered", "notes.md · 1.2 MB · text/markdown · sending 38% · 456.0 kB"]);
        notes.state = FileState::Out(OutState::Sent);
        changed(&mut model, notes);
        let mut lost = pdf(FileState::Out(OutState::Failed));
        lost.error = Some("the file changed before it was fetched".into());
        changed(&mut model, lost);
        assert_eq!(titles(&model)[1..], ["drawings.pdf · 24.1 MB · application/pdf · failed · the file changed before it was fetched", "notes.md · 1.2 MB · text/markdown · sent"]);
        assert_eq!(line(&model.entries[2]).1, "✕");
        // Another exchange's file has no call here and adds nothing.
        let mut other = pdf(FileState::Out(OutState::Queued));
        (other.id, other.exchange) = ("00".repeat(16), uuid::Uuid::nil());
        changed(&mut model, other);
        assert_eq!(model.entries.len(), 5);
    }

    /// The non-TTY printer (`lib.rs`): a message row's placeholders, a notice row and an
    /// ended-share row print as file lines, the words around them kept, never the markup.
    #[test]
    fn plain_rows_print_file_lines_never_markup() {
        let placeholders = engine::placeholders("peer:maya", &[zip(FileState::In(InState::Receiving)), pdf(FileState::In(InState::Receiving))]);
        let text = format!("<message from=\"peer:maya\" id=\"{EXCHANGE}\" at=\"14:31\">\nAs promised.\n</message>\n{placeholders}\nafter");
        let shown = plain(&text);
        assert_eq!(shown, format!("<message from=\"peer:maya\" id=\"{EXCHANGE}\" at=\"14:31\">\nAs promised.\n</message>\n\
            ▤ site-survey.zip · 1.2 GB · application/zip · receiving\n▤ drawings.pdf · 24.1 MB · application/pdf · receiving\nafter"));
        assert_eq!(plain("no files here"), "no files here");
        let received = zip(FileState::In(InState::Received));
        assert_eq!(notice_line(&engine::notice("peer:maya", &received).unwrap()).as_deref(), Some("▤ site-survey.zip · 1.2 GB · application/zip · saved"));
        assert_eq!(ended_line(&engine::ended_notice("peer:maya", &received).unwrap()).as_deref(),
            Some("▤ site-survey.zip · 1.2 GB · application/zip · saved · not given to the agent"));
        assert_eq!(ended_line("<message_file broken"), Some(UNREADABLE.into()));
        assert_eq!(ended_line("some other error"), None);
        assert_eq!(notice_line("plain words"), None);
    }

    /// SEC-A2: `f` fetches and reports the path; it never launches an app on the contact's
    /// file (only `Open`/`Reveal` call the opener).
    #[tokio::test]
    async fn fetching_here_never_opens_the_file() {
        fn engine(s: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
            let mut m = jsonrpsee::RpcModule::new(s);
            m.register_method("contact_file_fetch", |_, _, _| -> jsonrpsee::core::RpcResult<std::path::PathBuf> {
                Ok("/cache/h/f/site-survey.zip".into())
            }).unwrap();
            m
        }
        let rt = runtime::Runtime::new(Default::default(), std::env::temp_dir());
        let api = rpc::client::Engine::Local(rpc::client::Local::new(rt, engine));
        let opened = std::sync::Mutex::new(Vec::new());
        let open = |p: &Path| { opened.lock().unwrap().push(p.to_owned()); Ok(()) };
        let got = act(&api, Some(SessionId::new()), HOLDER.into(), "9d0c".into(), Act::Fetch, &open).await;
        assert_eq!(got.as_deref(), Ok("fetched → /cache/h/f/site-survey.zip · ⏎ opens it"));
        assert!(opened.lock().unwrap().is_empty(), "a fetch opened {:?}", opened.lock().unwrap());
    }

    /// UI-1: the message's placeholder line is the file's live line; with it loaded, the
    /// notice row and the ended-share row are one compact note each (given / not given to
    /// the agent), never a second card. Without the message row, the notice stays a card.
    #[test]
    fn a_notice_beside_its_message_is_a_compact_note() {
        let text = format!("<message from=\"peer:maya\" id=\"{EXCHANGE}\" at=\"14:31\">\nTwo files.\n</message>\n{}",
            engine::placeholders("peer:maya", &[zip(FileState::In(InState::Receiving)), pdf(FileState::In(InState::Receiving))]));
        let origin = PeerOrigin { id: EXCHANGE, from: "peer:maya".into(), reply_to: None, scope: None, contact_mode: Some(agent::ShareMode::Full) };
        let mut received = zip(FileState::In(InState::Received));
        received.path = Some("/r/site-survey.zip".into());
        let ended_row = HistoryEntry::Error { text: engine::ended_notice("peer:maya", &pdf(FileState::In(InState::Received))).unwrap(), context: None };
        let mut entries = history(user(text, Some(origin)));
        entries.extend(history(user(engine::notice("peer:maya", &received).unwrap(), None)));
        entries.extend(history(ended_row));
        fold_notices(&mut entries);
        let notes: Vec<&str> = entries.iter().filter_map(|e| match e { Entry::Info(t) => Some(t.as_str()), _ => None }).collect();
        assert_eq!(notes, [
            "↳ file notice · site-survey.zip · 1.2 GB · application/zip · saved · given to the agent",
            "↳ file notice · drawings.pdf · 24.1 MB · application/pdf · saved · not given to the agent",
        ]);
        let live: Vec<&str> = entries.iter().filter(|e| e.key().is_some_and(|k| k.starts_with("file:"))).map(|e| line(e).2).collect();
        assert_eq!(live, [
            "site-survey.zip · 1.2 GB · application/zip · saved",
            "drawings.pdf · 24.1 MB · application/pdf · saved",
        ], "one live line per file, the message's, at the notice's state");

        let mut alone = history(user(engine::notice("peer:maya", &received).unwrap(), None));
        fold_notices(&mut alone);
        assert_eq!(line(&alone[0]).2, "site-survey.zip · 1.2 GB · application/zip · saved", "no message row: the full line");
    }
}
