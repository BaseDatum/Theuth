use super::*;
use crate::components::{Wrapper, WrapperView};

/// One transcript row.
#[derive(Debug)]
pub enum Entry {
    /// What the user typed.
    User {
        /// The text.
        text: String,
        /// Seq of this row when it opens a turn (the `/edit`, `/regen`,
        /// `/fork` handle, 29 §6.1); `None` for steers and notes.
        turn: Option<i64>,
    },
    /// Streamed assistant text: rendered blocks + raw tail.
    Assistant(Box<markdown::Text>),
    /// Collapsible card: a peer message, a skill body, or the system prompt.
    /// One line until toggled with Ctrl+O.
    Card {
        /// Glyph and colour family.
        kind: CardKind,
        /// Peer name, skill name, or `prompt`.
        title: String,
        /// Body rendered as markdown.
        text: Box<markdown::Text>,
        /// Show the body.
        expanded: bool,
    },
    /// Streamed reasoning; collapsed once `done`.
    Reasoning {
        /// Text so far.
        text: String,
        /// No more deltas will come.
        done: bool,
    },
    /// The model is still emitting a tool call's arguments.
    Pending {
        /// Provider call id (matches `ToolCallStarted`).
        call_id: String,
        /// Tool name.
        name: String,
        /// `ToolCallProgress` so far: what a `show` call is doing and the
        /// argument bytes streamed (design doc 35 §6.1).
        progress: Option<Progress>,
    },
    /// A tool call and, once finished, its result.
    Tool {
        /// Provider call id (matches `ToolCallFinished`).
        call_id: String,
        /// Tool name.
        name: String,
        /// Arguments as [`describe`] lines.
        args: Vec<String>,
        /// `(output, is_error)` once finished.
        result: Option<(String, bool)>,
        /// Full output shown (Ctrl+O); built on first expand.
        expanded: Option<Box<markdown::Text>>,
        /// The file a `show` call produced (`ArtifactShown`, design doc 35);
        /// the card renders as an artifact card once set.
        artifact: Option<Box<artifact::Artifact>>,
    },
    /// A compaction boundary (design doc 07 §8.4): a divider, expandable with
    /// Ctrl+O to the summary when loaded from history (the live event has none).
    Compacted {
        /// Epoch the boundary opened.
        epoch: u32,
        /// How the head was built.
        style: CompactStyle,
        /// Summary the model works from; `None` for the live event.
        summary: Option<Box<markdown::Text>>,
        /// Show the summary.
        expanded: bool,
    },
    /// 109 §10.3: a plain divider in the `Compacted` shape (`── resumed after
    /// restart ──`); nothing to expand.
    Divider(String),
    /// 125 §7.4: one Advisory card — every note the advisor raised at one
    /// step boundary. Three notes until Ctrl+O (`ADVISORY_COLLAPSED`).
    Advisory {
        /// The typed metadata of the row.
        advisory: Advisory,
        /// Show every note.
        expanded: bool,
    },
    /// Informational line (session lists, notices).
    Info(String),
    /// Error line.
    Error(String),
}

/// What an [`Entry::Card`] holds; picks its glyph and colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardKind {
    /// A skill body (`/skills`) or a knowledge card (`/knowledge`).
    Skill,
    /// The assembled system prompt (`/prompt`).
    Prompt,
    /// A `<job_result>` the harness injected when a background job ended.
    Job,
    /// A `<schedule>` fire the harness injected (34 §1.5).
    Schedule,
    /// An artifact shown inline from `/artifacts` (`s`), or the `show` card header (35 §6.1).
    Artifact,
    /// A message the user removed from the model's context (48 §2.2): the
    /// original stays in the archive, shown only when expanded.
    Removed,
    /// A vision consultation note (52 §6): the main model's question and
    /// the fallback's answer, shown only when expanded.
    Vision,
    /// 152 D6: a `<resume_handoff>` after an engine restart (146 D10/D11) or a
    /// `<child_lost>` note in a parent.
    Restart,
    /// 152 D6: the view an endless parent handed a native child (137 R1-D1).
    Memory,
    /// 183 §5.3: this session moved to (or was taken by) another engine.
    Moved,
    /// 183 §11.2: the `<engine_restart/>` note a resumed tool result opens with.
    EngineRestart,
    /// Owner-formatted card with a stable identity and neutral presentation.
    Component {
        /// Newer cards freeze earlier cards in this named group.
        group: &'static str,
        /// Stable identity, independent of transcript position.
        key: Uuid,
        /// Header marker.
        glyph: &'static str,
        /// Header color.
        color: iocraft::Color,
        /// Literal untrusted text, without Markdown or terminal hyperlinks.
        plain: bool,
    },
}

impl CardKind {
    /// Whether this owner card requests literal text presentation.
    pub fn plain(self) -> bool { matches!(self, Self::Component { plain: true, .. }) }
    /// Header glyph.
    pub fn glyph(self) -> &'static str {
        match self {
            CardKind::Skill => "◇",
            CardKind::Prompt => "§",
            CardKind::Job => "▸",
            CardKind::Schedule => "◷",
            CardKind::Artifact => "▣",
            CardKind::Removed => "⊘",
            CardKind::Vision => "◉",
            CardKind::Restart => "↻",
            CardKind::Memory => "∞",
            CardKind::Moved => "⇄",
            CardKind::EngineRestart => "↻",
            CardKind::Component { glyph, .. } => glyph,
        }
    }
}

/// 183 §5.3: the one-line title of a Moved card.
pub fn moved_title(m: &agent::SessionMoved) -> String {
    let from = if m.from.is_empty() {
        String::new()
    } else {
        format!(" · from {}", m.from)
    };
    format!(
        "Session moved to {} · {} · {}{from}",
        m.to, m.os_name, m.cwd
    )
}

/// 183 §11.2: a card for a leading `<engine_restart …/>` line, split off a
/// resumed tool result; `rest` is the output without it.
pub(super) fn split_engine_restart(output: &str) -> Option<(Entry, &str)> {
    let (note, rest) = output.split_once('\n').unwrap_or((output, ""));
    let note = note.trim_end();
    if !note.starts_with("<engine_restart ") || !note.ends_with("/>") {
        return None;
    }
    let get = |k: &str| attr(note, k).map(unescape).unwrap_or_default();
    let after = get(" after=\"");
    let title = match attr(note, " to=\"") {
        Some(to) => {
            let from = attr(note, " from=\"")
                .map_or_else(String::new, |f| format!(" · moved from {}", unescape(f)));
            format!("Resumed on {} after {after}{from}", unescape(to))
        }
        None => format!("Resumed after {after} · {}", get(" kind=\"")),
    };
    Some((card(CardKind::EngineRestart, title, note), rest))
}

/// A collapsed [`Entry::Card`] over `text`.
pub(super) fn card(kind: CardKind, title: String, text: &str) -> Entry {
    Entry::Card {
        kind,
        title,
        text: Box::new(markdown::Text::complete(text)),
        expanded: false,
    }
}

/// Rows a string of `chars` occupies at `width` columns (at least one).
fn rows(chars: usize, width: usize) -> usize {
    chars.div_ceil(width.max(1)).max(1)
}

/// Rows a rendered markdown line takes once the terminal wraps it.
fn line_rows(line: &markdown::Line, width: usize) -> usize {
    rows(line.iter().map(|s| s.text.chars().count()).sum(), width)
}

/// Rows a plain text takes once wrapped: one per line, more for long ones.
fn text_rows(text: &str, width: usize) -> usize {
    text.lines().map(|l| rows(l.chars().count(), width)).sum()
}

impl Entry {
    /// Stable raw-call identity for neutral owner decorations.
    pub(crate) fn key(&self) -> Option<&str> {
        match self { Self::Pending { call_id, .. } | Self::Tool { call_id, .. } => Some(call_id), _ => None }
    }
    /// Rows this entry draws at `width` columns (borders, headers and
    /// wrapping included; an estimate — the terminal's wrap is the truth).
    pub(super) fn height(&self, width: usize) -> usize {
        // Bordered boxes have two columns of frame and padding each side.
        let inner = width.saturating_sub(4);
        let md = crate::model::markdown_rows;
        match self {
            Entry::User { text, .. } => match crate::attach::spoken(text) {
                Some((_, _, body)) => text_rows(&crate::attach::display(body), inner) + 5,
                None => text_rows(&crate::attach::display(text), inner) + 4,
            },
            // Rendered fences (diagrams, surfaces) never exceed the construction
            // width, so they count one row per line like everything else.
            Entry::Assistant(t) => md(t, width),
            Entry::Card {
                kind,
                title: _,
                text,
                expanded,
            } => {
                4 + match (*expanded, kind.plain()) {
                    (false, _) => 0,
                    (true, true) => text_rows(&clean(text.raw()), inner),
                    (true, false) => md(text, inner),
                }
            }
            // 125 §7.4: border + title + margin, one row per note shown, the `… +N more` row.
            Entry::Advisory { advisory, expanded } => {
                let n = advisory_row_count(advisory);
                if *expanded {
                    4 + n
                } else {
                    4 + n.min(ADVISORY_COLLAPSED) + usize::from(n > ADVISORY_COLLAPSED)
                }
            }
            Entry::Reasoning { done: true, .. } => 1,
            Entry::Reasoning { text, .. } | Entry::Info(text) | Entry::Error(text) => {
                text_rows(text, width)
            }
            Entry::Pending { .. } => 2,
            // Artifact card: header, title (when it differs), path row; the
            // arguments are not drawn (35 §6.1).
            Entry::Tool {
                artifact: Some(a),
                expanded,
                ..
            } => 6 + usize::from(a.title != a.name) + expanded.as_ref().map_or(0, |t| md(t, inner)),
            Entry::Tool {
                args,
                expanded,
                ..
            } => {
                5 + args.len()
                    + expanded.as_ref().map_or(0, |t| md(t, inner))
            }
            Entry::Compacted {
                summary, expanded, ..
            } => {
                4 + match summary {
                    Some(t) if *expanded => md(t, inner),
                    _ => 0,
                }
            }
            Entry::Divider(_) => 3,
        }
    }
}

/// Split a harness-injected input into `(kind, title, body)` per wrapper.
/// Inputs queued during a turn arrive as one text: `<subagent_result
/// name="…">` / `<message from="…">` / `<job_result id="…" command="…"
/// exit="…" secs="…">` / `<job_idle id="…" quiet="…">` (61) blocks joined by
/// newlines. Harness notes (`<context_summary>`, `<todo>`,
/// `<context_pressure>`, `<runtime>`, `<knowledge>`; design docs 07 §8.7,
/// 10 §4) are wrappers too but yield no card. `None` for human text
/// (or anything not entirely made of wrappers).
pub fn injected<'a>(text: &'a str, mut project: impl FnMut(Wrapper<'a>) -> Option<WrapperView>) -> Option<Vec<(CardKind, String, String)>> {
    let mut out = Vec::new();
    let mut rest = text.trim_start();
    while !rest.is_empty() {
        let (open, after) = rest.strip_prefix('<')?.split_once('>')?;
        let name = open.split(' ').next()?;
        let (key, close) = match name {
            "subagent_result" => (Some("name=\""), "</subagent_result>"),
            "message" => (Some("from=\""), "</message>"),
            "job_result" => (Some("id=\""), "</job_result>"),
            "job_idle" => (Some("id=\""), "</job_idle>"),
            "reminder" => (Some("id=\""), "</reminder>"),
            "schedule" => (Some("name=\""), "</schedule>"),
            "context_summary" => (None, "</context_summary>"),
            "todo" => (None, "</todo>"),
            "context_pressure" => (None, "</context_pressure>"),
            "runtime" => (None, "</runtime>"),
            // 137G: a bare `<memory>` endless note (no attributes).
            "memory" if open == "memory" => (None, "</memory>"),
            "knowledge" => (None, "</knowledge>"),
            // 144 §5.3: the bound chat's volatile `<notebook>` block (a harness note like `<knowledge>`).
            "notebook" => (None, "</notebook>"),
            "nudge" => (None, "</nudge>"),
            "journal" => (None, "</journal>"),
            // 125 §7.4: an advisory steer's row is carded by `AdvisoryAdded`, never echoed.
            "advisory" => (None, "</advisory>"),
            // 152 D5: `<budget spent cap currency>` is an in-turn notice like `<todo>`.
            "budget" => (None, "</budget>"),
            // 152 D6: no sender attribute; the title is parsed from the body below.
            "resume_handoff" => (None, "</resume_handoff>"),
            "child_lost" => (Some("name=\""), "</child_lost>"),
            // 151 R1: a study's start / result / failure card delivered into its setup chat.
            "autoresearch" => (Some("slug=\""), "</autoresearch>"),
            _ => return None,
        };
        let (body, tail) = after.split_once(close)?;
        let row = Wrapper { tag: name, open, body: body.strip_prefix('\n').unwrap_or(body).trim_end(),
            sender: None, origin: None, live: false };
        match project(row) {
            Some(WrapperView::Hidden) => { rest = tail.trim_start(); continue; }
            Some(WrapperView::Card { kind, title, body }) => {
                out.push((kind, title, body)); rest = tail.trim_start(); continue;
            }
            Some(WrapperView::Fallback) | None => {}
        }
        if name == "resume_handoff" {
            let body = body.trim();
            let at = body
                .strip_prefix("The theuth engine restarted at ")
                .map_or("", |s| s.split(' ').next().unwrap_or(""))
                .trim_end_matches('.');
            let what = if body.contains("has been restored") {
                "resumed after restart"
            } else {
                "restarted fresh"
            };
            let title = if at.is_empty() {
                what.to_owned()
            } else {
                format!("{what} · {at}")
            };
            out.push((CardKind::Restart, title, body.to_owned()));
            rest = tail.trim_start();
            continue;
        }
        if let Some(key) = key {
            let from = attr(open, key)?;
            let (kind, title) = if name == "job_result" {
                let command = unescape(attr(open, "command=\"").unwrap_or_default());
                let exit = attr(open, "exit=\"").unwrap_or_default();
                let secs = attr(open, "secs=\"").unwrap_or_default();
                // 22 §5: a PTY job's card says so.
                let pty = if attr(open, "pty=\"") == Some("true") {
                    " · pty"
                } else {
                    ""
                };
                (
                    CardKind::Job,
                    format!("{from} · {command} · exit {exit} · {secs}s{pty}"),
                )
            } else if name == "job_idle" {
                // 61 §2.3: `j4 idle 63s · omp1` (name only when the job has one).
                let quiet = attr(open, "quiet=\"").unwrap_or_default();
                let name = attr(open, "name=\"")
                    .map(|n| format!(" · {}", unescape(n)))
                    .unwrap_or_default();
                (CardKind::Job, format!("{from} idle {quiet}s{name}"))
            } else if name == "reminder" {
                // 14 as-built: `r3 reminder · after 10m` (` · late` when re-armed past due).
                let after = attr(open, "after=\"")
                    .and_then(|s| s.parse::<u64>().ok())
                    .map_or_else(String::new, |s| format!(" · after {}", human_secs(s)));
                let late = if attr(open, "late=\"") == Some("true") {
                    " · late"
                } else {
                    ""
                };
                (CardKind::Job, format!("{from} reminder{after}{late}"))
            } else if name == "schedule" {
                let at = attr(open, "at=\"").unwrap_or_default();
                let at = chrono::DateTime::parse_from_rfc3339(at).map_or_else(
                    |_| at.to_owned(),
                    |t| t.with_timezone(&chrono::Local).format("%H:%M").to_string(),
                );
                (CardKind::Schedule, format!("{from} · {at}"))
            } else if name == "child_lost" {
                let at = attr(open, "at=\"").unwrap_or_default();
                let why = unescape(attr(open, "why=\"").unwrap_or_default());
                (
                    CardKind::Restart,
                    format!("lost · {} · {at} · {why}", unescape(from)),
                )
            } else if name == "autoresearch" {
                let event = attr(open, "event=\"").unwrap_or_default();
                (
                    CardKind::Schedule,
                    format!("autoresearch · {} · {event}", unescape(from)),
                )
            } else {
                (wrapper_kind(from), from.to_owned())
            };
            let body = body.strip_prefix('\n').unwrap_or(body).trim_end();
            out.push((kind, title, body.to_owned()));
        }
        rest = tail.trim_start();
    }
    Some(out)
}

/// `45s`, `10m`, `1h30m`.
fn human_secs(s: u64) -> String {
    match (s / 3600, (s % 3600) / 60, s % 60) {
        (0, 0, s) => format!("{s}s"),
        (0, m, _) => format!("{m}m"),
        (h, 0, _) => format!("{h}h"),
        (h, m, _) => format!("{h}h{m}m"),
    }
}

/// Undo the wrappers' XML attribute escape.
pub(crate) fn unescape(s: &str) -> String {
    s.replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// Value of `key` (`name="`) in an open tag, up to the closing quote.
pub(crate) fn attr<'a>(open: &'a str, key: &str) -> Option<&'a str> {
    open.split_once(key).and_then(|(_, v)| v.split('"').next())
}

/// Tool arguments as display lines. `shell` shows the command itself
/// (`$ ` on the first line); anything else is `key: value` per argument,
/// with long or multi-line strings collapsed to their size.
pub fn describe(name: &str, args: &serde_json::Value) -> Vec<String> {
    if name == "shell"
        && let Some(cmd) = args.get("command").and_then(|c| c.as_str())
    {
        return cmd
            .trim()
            .lines()
            .enumerate()
            .map(|(i, l)| format!("{}{l}", if i == 0 { "$ " } else { "  " }))
            .collect();
    }
    let Some(obj) = args.as_object() else {
        return if args.is_null() {
            Vec::new()
        } else {
            vec![args.to_string()]
        };
    };
    obj.iter()
        .map(|(k, v)| match v {
            serde_json::Value::String(s) if !s.contains('\n') && s.chars().count() <= 120 => {
                format!("{k}: {s}")
            }
            serde_json::Value::String(s) => {
                format!("{k}: ({} lines, {} bytes)", s.lines().count(), s.len())
            }
            other => {
                let s = other.to_string();
                match s.char_indices().nth(120) {
                    Some((cut, _)) => format!("{k}: {}…", &s[..cut]),
                    None => format!("{k}: {s}"),
                }
            }
        })
        .collect()
}

/// Full tool output for an expanded card in a code fence so it renders
/// verbatim; JSON is pretty-printed, also when a trailer line follows it
/// (the shell tool appends `exit code: n`).
pub fn output_markdown(output: &str) -> String {
    let output = output.trim_end();
    let pretty = |s: &str| {
        serde_json::from_str::<serde_json::Value>(s.trim())
            .ok()
            .and_then(|v| serde_json::to_string_pretty(&v).ok())
    };
    if let Some(json) = pretty(output) {
        return format!("```json\n{json}\n```");
    }
    if let Some((head, tail)) = output.rsplit_once('\n')
        && let Some(json) = pretty(head)
    {
        return format!("```json\n{json}\n```\n{tail}");
    }
    format!("```\n{output}\n```")
}

/// First output line plus a `(+n lines)` tail for the rest.
pub fn result_line(output: &str) -> String {
    let mut lines = output.lines();
    let first = lines.next().unwrap_or_default();
    match lines.count() {
        0 => first.to_owned(),
        n => format!("{first}  (+{n} lines)"),
    }
}

/// What `Model::window` hands the renderer; see there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    /// First entry built.
    pub from: usize,
    /// One past the last entry built.
    pub to: usize,
    /// Pinned: rendered lines of `from` to drop.
    pub skip: usize,
    /// Anchored: rows of `from` above the fold.
    pub top: usize,
}

impl Model {
    /// Core measurement plus registered owners' exact decoration rows.
    pub(crate) fn entry_height(&self, entry: &Entry, width: usize) -> usize {
        let Some(key) = entry.key() else { return entry.height(width) };
        let (replacement, appended) = self.components.entry_layout(key, width);
        replacement.unwrap_or_else(|| entry.height(width)) + appended
    }


    /// The slice of the transcript worth building this frame. Every frame
    /// lays out and paints every child it is given, so building the whole
    /// history made each frame O(session); the screen shows `view` rows.
    ///
    /// Pinned (`anchor == None`): `[from, len)` drawn bottom-aligned, with
    /// `skip` leading rendered lines of `from` dropped when that one entry
    /// (a long reply) is taller than the window. Anchored: `[from, to)` drawn
    /// from the top with the first `top` rows of `from` above the fold.
    pub fn window(&self, view: usize, width: usize) -> Window {
        let budget = view * 2;
        if let Some((i, r)) = self.anchor.filter(|(i, _)| *i < self.entries.len()) {
            let mut total = self.entry_height(&self.entries[i], width).saturating_sub(r);
            let mut to = i + 1;
            while total < budget && to < self.entries.len() {
                total += self.entry_height(&self.entries[to], width);
                to += 1;
            }
            return Window {
                from: i,
                to,
                skip: 0,
                top: r,
            };
        }
        let (from, skip) = self.tail(budget, width);
        Window {
            from,
            to: self.entries.len(),
            skip,
            top: 0,
        }
    }

    /// The pinned window's start: the first entry whose tail reaches `lines`
    /// rows at `width`, and how many rendered markdown lines of it to skip
    /// when that entry alone is taller than the window.
    pub(super) fn tail(&self, lines: usize, width: usize) -> (usize, usize) {
        let mut total = 0;
        for (i, e) in self.entries.iter().enumerate().rev() {
            let h = self.entry_height(e, width);
            if total + h < lines {
                total += h;
                continue;
            }
            // Only a rendered reply is skippable line by line; leave a full
            // budget of its own rows, the plain tail always shows.
            let Entry::Assistant(t) = e else {
                return (i, 0);
            };
            let v = t.view();
            let mut budget = (lines - total).saturating_sub(text_rows(v.plain, width));
            let rendered: Vec<&markdown::Line> = v.lines().collect();
            let mut skip = rendered.len();
            for l in rendered.iter().rev() {
                if budget == 0 {
                    break;
                }
                budget = budget.saturating_sub(line_rows(l, width));
                skip -= 1;
            }
            return (i, skip);
        }
        (0, 0)
    }

    /// Move the transcript by `delta` rows (negative = up) in a `view`-row
    /// window at `width`. Heights are the same estimates `window` draws by.
    /// Reaching the bottom re-pins; the top stops at the first entry.
    pub fn scroll(&mut self, delta: isize, view: usize, width: usize) {
        let h = |i: usize| self.entry_height(&self.entries[i], width);
        let len = self.entries.len();
        let (mut i, mut r) = match self.anchor.filter(|(i, _)| *i < len) {
            Some(a) => a,
            None => {
                // The pinned window's first row: `view` rows up from the end.
                let mut left = view;
                let mut i = len;
                loop {
                    if i == 0 {
                        return; // shorter than the window: nothing to scroll
                    }
                    i -= 1;
                    let hi = h(i);
                    if hi > left {
                        break (i, hi - left);
                    }
                    left -= hi;
                }
            }
        };
        if delta < 0 {
            let mut d = delta.unsigned_abs();
            loop {
                if r >= d {
                    r -= d;
                    break;
                }
                d -= r;
                if i == 0 {
                    r = 0;
                    break;
                }
                i -= 1;
                r = h(i);
            }
        } else {
            r += delta.unsigned_abs();
            while r >= h(i) {
                r -= h(i);
                i += 1;
                if i == len {
                    self.anchor = None;
                    return;
                }
            }
        }
        // Less than a window left below the anchor: follow the tail again.
        let mut below = h(i) - r;
        let mut j = i + 1;
        while below <= view && j < len {
            below += h(j);
            j += 1;
        }
        self.anchor = (below > view).then_some((i, r));
    }
}

/// One `VisionProgress` step as a line (52 §6): the status text while the
/// question or inspection is in flight, the card title once it settled
/// (every source answered on `Completed`, none on `Failed`). `pub` for
/// print mode.
pub fn vision_line(stage: VisionStage, model: &str, n: usize) -> String {
    match stage {
        VisionStage::Questioning => format!("asking {model} what to look for…"),
        VisionStage::Inspecting => format!("{model} inspecting {n} image(s)…"),
        VisionStage::Completed => vision_title(model, n, n),
        VisionStage::Failed => vision_title(model, 0, n),
    }
}

/// Header of a vision card: `vision: <fallback> inspected 2 image(s)`,
/// `… inspected 1 of 3 image(s)`, or `… failed on 3 image(s)` when nothing
/// was answered. Shared by [`vision_line`] and the reloaded note
/// ([`note_title`]).
fn vision_title(model: &str, answered: usize, total: usize) -> String {
    match answered {
        0 => format!("vision: {model} failed on {total} image(s)"),
        a if a == total => format!("vision: {model} inspected {total} image(s)"),
        a => format!("vision: {model} inspected {a} of {total} image(s)"),
    }
}

/// [`vision_title`] of a stored note, counting its answered observations.
pub fn note_title(note: &VisionNote) -> String {
    let answered = note
        .observations
        .iter()
        .filter(|o| o.status == VisionStatus::Answered)
        .count();
    vision_title(&note.vision_model, answered, note.observations.len())
}

/// One line per attachment occurrence of a history row: `⎘ shot.png
/// (image/png)`, `⊘ report.pdf (application/pdf) · removed from context`.
pub(super) fn attachment_labels(c: &Option<ContextEntry>) -> Vec<String> {
    c.as_ref().map_or_else(Vec::new, |c| {
        c.attachments.iter().map(attachment_label).collect()
    })
}

/// See [`attachment_labels`].
pub fn attachment_label(a: &ContextAttachment) -> String {
    if a.removed {
        format!("⊘ {} ({}) · removed from context", a.name, a.mime)
    } else {
        format!("⎘ {} ({})", a.name, a.mime)
    }
}

/// The first string argument of a tool call, cut to 40 chars: the `running
/// read_file crates/…` detail of a row.
pub(super) fn first_arg(args: &serde_json::Value) -> Option<String> {
    args.as_object()?
        .values()
        .find_map(|v| v.as_str())
        .map(|s| {
            s.lines()
                .next()
                .unwrap_or_default()
                .chars()
                .take(40)
                .collect()
        })
}

/// Rows occupied by the existing markdown view at a terminal width.
pub(crate) fn markdown_rows(text: &markdown::Text, width: usize) -> usize {
    let view = text.view();
    view.lines().map(|line| line_rows(line, width)).sum::<usize>() + text_rows(view.plain, width)
}
/// `s` without control characters (an ESC would reach the terminal: iocraft
/// draws text verbatim), keeping line breaks and tabs. For remote text.
pub(crate) fn clean(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect()
}

pub(super) fn wrapper_kind(from: &str) -> CardKind {
    CardKind::Component { group: "wrapper", key: Uuid::nil(), glyph: "⟵", color: crate::ui::FAIENCE,
        plain: from.starts_with("contact:") || from.starts_with("peer:") }
}
