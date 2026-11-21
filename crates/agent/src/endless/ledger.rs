//! 201 §9 (D10, P6a): the state ledger — a small structured record of what
//! is true now in a chat session: Goals, Decisions, Constraints (written by
//! the session model through the `ledger` tool), Open tasks (mirrored from
//! the `todo` list) and Artefacts (files named by tool-call arguments).
//! Pure: the pump loads it from the store, updates it at a trim and renders
//! the snapshot that is frozen with the generation; tool ops between trims
//! queue `<ledger>` change notes for the next turn's input.

use serde::{Deserialize, Serialize};

/// The five sections, in snapshot order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Section {
    Goals,
    Decisions,
    Constraints,
    OpenTasks,
    Artefacts,
}

impl Section {
    pub const ALL: [Section; 5] = [
        Section::Goals,
        Section::Decisions,
        Section::Constraints,
        Section::OpenTasks,
        Section::Artefacts,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Section::Goals => "Goals",
            Section::Decisions => "Decisions",
            Section::Constraints => "Constraints",
            Section::OpenTasks => "Open tasks",
            Section::Artefacts => "Artefacts",
        }
    }

    /// Lenient parse of the tool's `section` argument.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_ascii_lowercase().replace(['_', '-'], " ");
        Some(match s.as_str() {
            "goal" | "goals" => Section::Goals,
            "decision" | "decisions" => Section::Decisions,
            "constraint" | "constraints" => Section::Constraints,
            "open tasks" | "open task" | "tasks" | "task" => Section::OpenTasks,
            "artefacts" | "artefact" | "artifacts" | "artifact" | "files" => Section::Artefacts,
            _ => return None,
        })
    }
}

/// An entry's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Open,
    Closed,
    Superseded,
}

/// One entry of Goals, Decisions, Constraints or Open tasks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// `L<n>`, never reused.
    pub id: String,
    pub section: Section,
    pub text: String,
    pub state: State,
    /// The unit (message line) it came from, when known (an accepted suggestion).
    #[serde(default)]
    pub unit: Option<u64>,
    /// Generation in force when it was made.
    pub created: u32,
    /// Generation in force when it was closed or superseded.
    #[serde(default)]
    pub closed: Option<u32>,
    /// Mirrored from the `todo` list (rewritten at each trim).
    #[serde(default)]
    pub todo: bool,
}

/// A `user` line flagged as a decision or a constraint, not yet confirmed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Suggestion {
    /// The line's address, `<unit>+1`.
    pub id: String,
    pub unit: u64,
    pub section: Section,
    /// The user's words (capped).
    pub text: String,
    /// Generation whose trim flagged it.
    pub created: u32,
}

/// A file named by a tool call's arguments.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artefact {
    pub path: String,
    /// The tool that last named it.
    pub action: String,
    /// Generation whose trim last saw it.
    pub generation: u32,
}

/// At most this many artefacts are kept (newest first).
pub const MAX_ARTEFACTS: usize = 40;
/// A suggestion neither accepted nor dropped lapses this many trims after it appeared.
pub const LAPSE_TRIMS: u32 = 2;
const SUGGESTION_CHARS: usize = 240;

/// The whole ledger, persisted as JSON (`endless_ledger.body`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ledger {
    #[serde(default)]
    pub next: u32,
    #[serde(default)]
    pub entries: Vec<Entry>,
    #[serde(default)]
    pub suggested: Vec<Suggestion>,
    #[serde(default)]
    pub artefacts: Vec<Artefact>,
    /// Change notes not yet sent (cleared by a trim: the snapshot has them).
    #[serde(default)]
    pub notes: Vec<String>,
    /// First unit not yet scanned for artefacts and suggestions.
    #[serde(default)]
    pub scanned: u64,
}

/// One `ledger` tool call.
#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
pub struct Op {
    /// goals | decisions | constraints | open_tasks | artefacts
    pub section: String,
    /// add | close | replace | accept | drop
    pub op: String,
    /// The entry's text, in the user's own words (add, replace).
    #[serde(default)]
    pub text: Option<String>,
    /// The entry (`L3`) or suggestion (`12+1`) id (close, replace, accept, drop).
    #[serde(default)]
    pub id: Option<String>,
}

/// A unit the trim scans: its index, kind and text.
pub struct Scan<'a> {
    pub i: u64,
    pub kind: super::tree::Kind,
    pub text: &'a str,
}

impl Ledger {
    pub fn from_json(s: &str) -> Self {
        serde_json::from_str(s).unwrap_or_default()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    fn entry_mut(&mut self, id: &str) -> Option<&mut Entry> {
        self.entries
            .iter_mut()
            .find(|e| e.id.eq_ignore_ascii_case(id))
    }

    fn add(
        &mut self,
        section: Section,
        text: String,
        unit: Option<u64>,
        gen_: u32,
        todo: bool,
    ) -> String {
        self.next += 1;
        let id = format!("L{}", self.next);
        self.entries.push(Entry {
            id: id.clone(),
            section,
            text,
            state: State::Open,
            unit,
            created: gen_,
            closed: None,
            todo,
        });
        id
    }

    fn suggestion(&self, id: &str) -> Option<usize> {
        let id = id.trim();
        self.suggested
            .iter()
            .position(|s| s.id == id || s.unit.to_string() == id)
    }

    /// Apply one tool op at generation `gen_`; the result text for the tool
    /// (also queued as a `<ledger>` change note), or the refusal.
    pub fn apply(&mut self, op: &Op, gen_: u32) -> Result<String, String> {
        let section = Section::parse(&op.section);
        let text = op.text.as_deref().map(str::trim).filter(|t| !t.is_empty());
        let id = op.id.as_deref().map(str::trim).filter(|t| !t.is_empty());
        let need_id = || id.ok_or_else(|| format!("ledger {}: `id` is required", op.op));
        let note = match op.op.trim() {
            "add" => {
                let section =
                    section.ok_or_else(|| format!("ledger: unknown section {:?}", op.section))?;
                let text = text.ok_or("ledger add: `text` is required")?;
                let id = self.add(section, text.to_owned(), None, gen_, false);
                format!("{id} added to {}: {text}", section.title())
            }
            "close" => {
                let id = need_id()?;
                let e = self
                    .entry_mut(id)
                    .filter(|e| e.state == State::Open)
                    .ok_or_else(|| format!("ledger close: no open entry {id}"))?;
                e.state = State::Closed;
                e.closed = Some(gen_);
                format!("{} closed in {}: {}", e.id, e.section.title(), e.text)
            }
            "replace" => {
                let id = need_id()?;
                let text = text.ok_or("ledger replace: `text` is required")?;
                let e = self
                    .entry_mut(id)
                    .filter(|e| e.state == State::Open)
                    .ok_or_else(|| format!("ledger replace: no open entry {id}"))?;
                e.state = State::Superseded;
                e.closed = Some(gen_);
                let (old, section) = (e.id.clone(), e.section);
                let new = self.add(section, text.to_owned(), None, gen_, false);
                format!("{new} replaces {old} in {}: {text}", section.title())
            }
            "accept" => {
                let id = need_id()?;
                let k = self
                    .suggestion(id)
                    .ok_or_else(|| format!("ledger accept: no suggestion {id}"))?;
                let s = self.suggested.remove(k);
                let section = section
                    .filter(|s| {
                        matches!(
                            s,
                            Section::Goals | Section::Decisions | Section::Constraints
                        )
                    })
                    .unwrap_or(s.section);
                let text = text.map_or(s.text.clone(), str::to_owned);
                let new = self.add(section, text.clone(), Some(s.unit), gen_, false);
                format!(
                    "{new} accepted from {} into {}: {text}",
                    s.id,
                    section.title()
                )
            }
            "drop" => {
                let id = need_id()?;
                let k = self.suggestion(id).ok_or_else(|| {
                    format!("ledger drop: no suggestion {id} (close or replace an entry instead)")
                })?;
                let s = self.suggested.remove(k);
                format!("suggestion {} dropped", s.id)
            }
            other => {
                return Err(format!(
                    "ledger: unknown op {other:?} (add, close, replace, accept, drop)"
                ));
            }
        };
        self.notes.push(note.clone());
        Ok(note)
    }

    /// The queued change notes as one `<ledger>` note body, cleared.
    pub fn take_notes(&mut self) -> Option<String> {
        if self.notes.is_empty() {
            return None;
        }
        Some(std::mem::take(&mut self.notes).join("\n"))
    }

    /// Open tasks mirror the `todo` list: `(text, done)` per item. Items no
    /// longer listed or done are closed; new ones are added.
    pub fn mirror_todos(&mut self, items: &[(String, bool)], gen_: u32) {
        for e in self
            .entries
            .iter_mut()
            .filter(|e| e.todo && e.state == State::Open)
        {
            let listed_open = items.iter().any(|(t, done)| !done && *t == e.text);
            if !listed_open {
                e.state = State::Closed;
                e.closed = Some(gen_);
            }
        }
        for (text, done) in items {
            if *done {
                continue;
            }
            let open = self
                .entries
                .iter()
                .any(|e| e.todo && e.state == State::Open && e.text == *text);
            if !open {
                self.add(Section::OpenTasks, text.clone(), None, gen_, true);
            }
        }
    }

    /// A trim into generation `gen_`: scan the units not yet scanned
    /// (artefacts from tool calls; suggestions from user lines when
    /// `suggestions`), lapse old suggestions, drop the notes (the snapshot
    /// about to be rendered holds every change).
    pub fn trim(&mut self, units: &[Scan<'_>], gen_: u32, suggestions: bool) {
        for u in units {
            match u.kind {
                super::tree::Kind::Tool => {
                    if let Some((action, path)) = artefact_of(u.text) {
                        self.touch(path, action, gen_);
                    }
                }
                super::tree::Kind::User if suggestions => {
                    let id = format!("{}+1", u.i);
                    if let Some(section) = cue(u.text)
                        && !self.suggested.iter().any(|s| s.id == id)
                        && !self.entries.iter().any(|e| e.unit == Some(u.i))
                    {
                        self.suggested.push(Suggestion {
                            id,
                            unit: u.i,
                            section,
                            text: cap(crate::clock::strip(u.text).trim(), SUGGESTION_CHARS),
                            created: gen_,
                        });
                    }
                }
                _ => {}
            }
            self.scanned = self.scanned.max(u.i + 1);
        }
        self.suggested
            .retain(|s| gen_ < s.created.saturating_add(LAPSE_TRIMS));
        self.notes.clear();
    }

    fn touch(&mut self, path: String, action: String, gen_: u32) {
        self.artefacts.retain(|a| a.path != path);
        self.artefacts.push(Artefact {
            path,
            action,
            generation: gen_,
        });
        if self.artefacts.len() > MAX_ARTEFACTS {
            let over = self.artefacts.len() - MAX_ARTEFACTS;
            self.artefacts.drain(..over);
        }
    }

    /// Entry counts per section (open entries; artefacts kept) and suggestions.
    pub fn counts(&self) -> [u32; 6] {
        let mut c = [0u32; 6];
        for e in self.entries.iter().filter(|e| e.state == State::Open) {
            let k = Section::ALL
                .iter()
                .position(|s| *s == e.section)
                .unwrap_or(0);
            c[k] += 1;
        }
        c[4] = self.artefacts.len() as u32;
        c[5] = self.suggested.len() as u32;
        c
    }

    /// The snapshot block, fitted to `budget` bytes: over it, the oldest
    /// closed tasks, then the oldest superseded decisions, then other
    /// closed entries, then the oldest artefacts leave the snapshot (they
    /// stay stored). Empty ledger → a one-line block naming the tool.
    pub fn render(&self, budget: usize) -> String {
        let mut hidden_entries: Vec<&str> = Vec::new();
        let mut hidden_artefacts = 0usize;
        let mut order: Vec<&Entry> = self
            .entries
            .iter()
            .filter(|e| e.state != State::Open)
            .collect();
        let rank = |e: &Entry| match (e.section, e.state) {
            (Section::OpenTasks, State::Closed) => 0,
            (Section::Decisions, State::Superseded) => 1,
            _ => 2,
        };
        order.sort_by_key(|e| (rank(e), e.closed.unwrap_or(0), e.created));
        let mut next = order.into_iter();
        loop {
            let text = self.render_with(&hidden_entries, hidden_artefacts);
            if text.len() <= budget {
                return text;
            }
            if let Some(e) = next.next() {
                hidden_entries.push(&e.id);
            } else if hidden_artefacts < self.artefacts.len() {
                hidden_artefacts += 1;
            } else {
                return text;
            }
        }
    }

    fn render_with(&self, hidden: &[&str], hidden_artefacts: usize) -> String {
        let mut out = String::from(
            "<ledger_snapshot>\nThe state ledger: what is true now. Record goals, decisions and constraints with the `ledger` tool when they are stated or change; accept or drop suggestions.\n",
        );
        for section in [
            Section::Goals,
            Section::Decisions,
            Section::Constraints,
            Section::OpenTasks,
        ] {
            let rows: Vec<&Entry> = self
                .entries
                .iter()
                .filter(|e| e.section == section && !hidden.contains(&e.id.as_str()))
                .collect();
            if rows.is_empty() {
                continue;
            }
            out.push_str(section.title());
            out.push('\n');
            for e in rows {
                let state = match e.state {
                    State::Open => "",
                    State::Closed => " (closed)",
                    State::Superseded => " (superseded)",
                };
                out.push_str(&format!("- [{}]{state} {}\n", e.id, e.text));
            }
        }
        let shown = &self.artefacts[hidden_artefacts.min(self.artefacts.len())..];
        if !shown.is_empty() {
            out.push_str("Artefacts\n");
            for a in shown.iter().rev() {
                out.push_str(&format!(
                    "- {} ({}, generation {})\n",
                    a.path, a.action, a.generation
                ));
            }
        }
        if !self.suggested.is_empty() {
            out.push_str("Suggested (unconfirmed)\n");
            for s in &self.suggested {
                out.push_str(&format!(
                    "- [{}] {}? \"{}\"\n",
                    s.id,
                    s.section.title().trim_end_matches('s').to_ascii_lowercase(),
                    s.text
                ));
            }
        }
        out.push_str("</ledger_snapshot>");
        out
    }
}

/// The file a tool call names: `(tool, path)` from a `Kind::Tool` unit's
/// text (`name {json args}`; a capped text falls back to a plain scan).
pub fn artefact_of(text: &str) -> Option<(String, String)> {
    const KEYS: [&str; 7] = [
        "path",
        "file_path",
        "file",
        "filename",
        "filepath",
        "target_file",
        "destination",
    ];
    let (name, args) = text.split_once(' ')?;
    if let Ok(serde_json::Value::Object(m)) = serde_json::from_str::<serde_json::Value>(args) {
        let path = KEYS
            .iter()
            .find_map(|k| m.get(*k).and_then(|v| v.as_str()))
            .filter(|p| !p.trim().is_empty())?;
        return Some((name.to_owned(), path.trim().to_owned()));
    }
    KEYS.iter().find_map(|k| {
        let at = args.find(&format!("\"{k}\":\""))? + k.len() + 4;
        let rest = &args[at..];
        let end = rest.find('"')?;
        let p = rest[..end].trim();
        (!p.is_empty()).then(|| (name.to_owned(), p.to_owned()))
    })
}

const CONSTRAINT_CUES: [&str; 10] = [
    "must not",
    "mustn't",
    "never ",
    "do not ",
    "don't ",
    "always ",
    "make sure",
    "only use",
    "not allowed",
    "you must",
];

const DECISION_CUES: [&str; 9] = [
    "let's go with",
    "lets go with",
    "go with ",
    "we'll use",
    "we will use",
    "i've decided",
    "i decided",
    "we decided",
    "switch to ",
];

/// 201 §9 (both profiles, P6a): the deterministic cue list on a user's
/// words — `Constraints`, `Decisions` or nothing. The `standard` profile's
/// "leading marker from the compactor" variant is a seam: `user` lines are
/// final at ingest (P3, D8) and never reach a model, so there is no model
/// line to mark; [`marker_of`] strips such a marker if a later phase adds it.
pub fn cue(text: &str) -> Option<Section> {
    let t = format!("{} ", text.to_ascii_lowercase());
    if CONSTRAINT_CUES.iter().any(|c| t.contains(c)) {
        Some(Section::Constraints)
    } else if DECISION_CUES.iter().any(|c| t.contains(c)) {
        Some(Section::Decisions)
    } else {
        None
    }
}

/// Seam (a later phase that sends `user` units to the `standard` compactor):
/// a leading `[decision]` / `[constraint]` marker on a model line — the
/// flagged section and the line without it (the guards see the stripped line).
pub fn marker_of(line: &str) -> (Option<Section>, &str) {
    for (m, s) in [
        ("[decision]", Section::Decisions),
        ("[constraint]", Section::Constraints),
    ] {
        if let Some(rest) = line.strip_prefix(m) {
            return (Some(s), rest.trim_start());
        }
    }
    (None, line)
}

fn cap(s: &str, n: usize) -> String {
    match s.char_indices().nth(n) {
        Some((at, _)) => format!("{}…", &s[..at]),
        None => s.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::tree::Kind;
    use super::*;

    fn op(section: &str, op: &str, text: Option<&str>, id: Option<&str>) -> Op {
        Op {
            section: section.into(),
            op: op.into(),
            text: text.map(Into::into),
            id: id.map(Into::into),
        }
    }

    #[test]
    fn the_five_ops() {
        let mut l = Ledger::default();
        let n = l
            .apply(&op("decisions", "add", Some("use sqlite"), None), 1)
            .unwrap();
        assert!(n.starts_with("L1 added to Decisions"), "{n}");
        l.apply(&op("goals", "add", Some("ship 201"), None), 1)
            .unwrap();
        // replace supersedes and adds.
        let n = l
            .apply(
                &op("decisions", "replace", Some("use postgres"), Some("L1")),
                2,
            )
            .unwrap();
        assert!(n.starts_with("L3 replaces L1"), "{n}");
        assert_eq!(l.entries[0].state, State::Superseded);
        assert_eq!(l.entries[0].closed, Some(2));
        l.apply(&op("goals", "close", None, Some("L2")), 2).unwrap();
        assert_eq!(l.entries[1].state, State::Closed);
        assert!(l.apply(&op("goals", "close", None, Some("L2")), 2).is_err());
        // accept and drop act on suggestions.
        l.trim(
            &[
                Scan {
                    i: 7,
                    kind: Kind::User,
                    text: "never push to main",
                },
                Scan {
                    i: 9,
                    kind: Kind::User,
                    text: "let's go with rust",
                },
            ],
            2,
            true,
        );
        assert_eq!(l.suggested.len(), 2);
        let n = l
            .apply(&op("constraints", "accept", None, Some("7+1")), 2)
            .unwrap();
        assert!(n.contains("into Constraints: never push to main"), "{n}");
        assert_eq!(l.entries.last().unwrap().unit, Some(7));
        l.apply(&op("decisions", "drop", None, Some("9+1")), 2)
            .unwrap();
        assert!(l.suggested.is_empty());
        assert!(l.apply(&op("goals", "frob", None, None), 2).is_err());
        assert!(l.apply(&op("nowhere", "add", Some("x"), None), 2).is_err());
        // The trim above folded the first four; accept and drop are queued.
        assert_eq!(l.take_notes().unwrap().lines().count(), 2);
        assert_eq!(l.take_notes(), None);
    }

    #[test]
    fn open_tasks_mirror_the_todo_list() {
        let mut l = Ledger::default();
        l.mirror_todos(&[("a".into(), false), ("b".into(), false)], 1);
        assert_eq!(l.counts()[3], 2);
        l.mirror_todos(
            &[("a".into(), true), ("b".into(), false), ("c".into(), false)],
            2,
        );
        let open: Vec<&str> = l
            .entries
            .iter()
            .filter(|e| e.state == State::Open)
            .map(|e| e.text.as_str())
            .collect();
        assert_eq!(open, ["b", "c"]);
        assert_eq!(l.entries[0].closed, Some(2));
        // A model-added task is not touched by the mirror.
        l.apply(&op("open_tasks", "add", Some("mine"), None), 2)
            .unwrap();
        l.mirror_todos(&[], 3);
        let open: Vec<&str> = l
            .entries
            .iter()
            .filter(|e| e.state == State::Open)
            .map(|e| e.text.as_str())
            .collect();
        assert_eq!(open, ["mine"]);
    }

    #[test]
    fn artefacts_come_from_tool_arguments_bounded() {
        assert_eq!(
            artefact_of(r#"write_file {"path":"/srv/a.rs","content":"x"}"#),
            Some(("write_file".into(), "/srv/a.rs".into()))
        );
        // A capped (invalid) JSON still yields the path.
        assert_eq!(
            artefact_of(r#"edit {"file_path":"b.md","old":"…"#),
            Some(("edit".into(), "b.md".into()))
        );
        assert_eq!(artefact_of(r#"shell {"cmd":"ls"}"#), None);
        let mut l = Ledger::default();
        let texts: Vec<String> = (0..50)
            .map(|k| format!(r#"read_file {{"path":"f{k}.rs"}}"#))
            .collect();
        let mut scans: Vec<Scan> = texts
            .iter()
            .enumerate()
            .map(|(i, t)| Scan {
                i: i as u64,
                kind: Kind::Tool,
                text: t,
            })
            .collect();
        let again = r#"edit {"path":"f0.rs"}"#;
        scans.push(Scan {
            i: 50,
            kind: Kind::Tool,
            text: again,
        });
        l.trim(&scans, 3, true);
        assert_eq!(l.artefacts.len(), MAX_ARTEFACTS);
        let last = l.artefacts.last().unwrap();
        assert_eq!(
            (last.path.as_str(), last.action.as_str(), last.generation),
            ("f0.rs", "edit", 3)
        );
        assert_eq!(l.scanned, 51);
    }

    #[test]
    fn budget_evicts_closed_tasks_then_superseded_decisions() {
        let mut l = Ledger::default();
        let pad = "x".repeat(60);
        l.apply(
            &op(
                "decisions",
                "add",
                Some(&format!("old decision {pad}")),
                None,
            ),
            1,
        )
        .unwrap();
        l.apply(
            &op(
                "decisions",
                "replace",
                Some(&format!("new decision {pad}")),
                Some("L1"),
            ),
            2,
        )
        .unwrap();
        l.mirror_todos(
            &[
                (format!("task one {pad}"), false),
                (format!("task two {pad}"), false),
            ],
            1,
        );
        l.mirror_todos(
            &[
                (format!("task one {pad}"), true),
                (format!("task two {pad}"), true),
            ],
            2,
        );
        l.apply(&op("goals", "add", Some("the goal"), None), 2)
            .unwrap();
        let full = l.render(usize::MAX);
        assert!(full.contains("old decision") && full.contains("task one"));
        // Just under the full size: the first closed task leaves first.
        let r = l.render(full.len() - 1);
        assert!(
            !r.contains("task one") && r.contains("task two") && r.contains("old decision"),
            "{r}"
        );
        let r = l.render(full.len() - 100);
        assert!(!r.contains("task two") && r.contains("old decision"), "{r}");
        let r = l.render(full.len() - 200);
        assert!(
            !r.contains("old decision") && r.contains("new decision") && r.contains("the goal"),
            "{r}"
        );
        // Stored all the same.
        assert_eq!(l.entries.len(), 5);
    }

    #[test]
    fn suggestions_lapse_after_two_trims() {
        let mut l = Ledger::default();
        l.trim(
            &[Scan {
                i: 3,
                kind: Kind::User,
                text: "you must keep the API stable",
            }],
            4,
            true,
        );
        assert!(l.render(usize::MAX).contains(
            "Suggested (unconfirmed)\n- [3+1] constraint? \"you must keep the API stable\""
        ));
        l.trim(&[], 5, true);
        assert_eq!(l.suggested.len(), 1);
        l.trim(&[], 6, true);
        assert!(l.suggested.is_empty());
        // Off: nothing flagged; a unit already scanned is not flagged again.
        let mut l = Ledger::default();
        l.trim(
            &[Scan {
                i: 3,
                kind: Kind::User,
                text: "never do that",
            }],
            1,
            false,
        );
        assert!(l.suggested.is_empty());
        assert_eq!(
            marker_of("[decision] use x"),
            (Some(Section::Decisions), "use x")
        );
    }
}
