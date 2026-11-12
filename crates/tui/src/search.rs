//! `/search` (design doc 49 §6–7): one live menu over the six search
//! categories, the request identities that keep late replies inert, the
//! read-only previews it opens, and the page-at-seq inspection of an old
//! session message. Pure state: `ui.rs` draws it, `lib.rs` turns its
//! [`Request`]s into `rpc::Api` calls and feeds [`Reply`]s back.
//!
//! Every request carries the view's `generation`; editing the query,
//! changing chips, closing or reopening bumps it, so whatever answers a
//! superseded request — results, errors, an opened destination — is dropped
//! on arrival. Groups remember the generation that filled them: a row is
//! activatable only while its group answers the current one.

use std::fmt::Write as _;

use ::search::{
    Coverage, MatchRange, PageState, SearchCategory, SearchNotice, SearchPage, validate_query,
};
use agent::search::{HitKind, HitRole, SearchHit};
use agent::{Loaded, SessionSummary};
use artifact::search::{ArtifactMatchContext, ArtifactSearchHit, ContentCoverage};
use document::search::NoteSearchHit;
use knowledge::search::{ConceptSearchHit, HistoricalConcept, HistorySearchHit, HistorySide};
use knowledge::{ChangeStatus, Concept, Revision};
use meeting::search::MeetingSearchHit;
use rpc::search::{SearchGroup, SearchResponse};
use runtime::SessionId;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::model::{revision_rows, short, when};

/// Text edits wait this long before a request (49 §2.2).
pub const DEBOUNCE_MS: u64 = 150;
/// Re-ask cadence for `indexing` groups, selected ones only (49 §4.3).
pub const POLL_MS: u64 = 500;
/// Per-group limits `show more` walks (49 §2.3).
pub const LIMITS: [u32; 3] = [10, 30, 50];
/// Recent sessions shown under a blank query with Sessions selected.
pub const RECENT: usize = 10;
/// Entries of the page an old message is inspected in (49 §7).
pub const PAGE: u32 = 60;
/// Chars of a document a preview shows; anything past is one notice line.
const PREVIEW_CHARS: usize = 400_000;
/// Category count: the chip and limit arrays' length.
pub const N: usize = SearchCategory::ALL.len();

/// Fixed chip labels, in [`SearchCategory::ALL`] order.
pub fn label(c: SearchCategory) -> &'static str {
    match c {
        SearchCategory::Sessions => "Sessions",
        SearchCategory::Artifacts => "Artifacts",
        SearchCategory::Knowledge => "Knowledge",
        SearchCategory::KnowledgeHistory => "Knowledge history",
        SearchCategory::Notes => "Notes",
        SearchCategory::Meetings => "Meetings",
        SearchCategory::Sources => "Sources",
        SearchCategory::Notebooks => "Notebooks",
    }
}

/// Wire name (`snake_case`), for the line printer.
pub fn wire(c: SearchCategory) -> &'static str {
    match c {
        SearchCategory::Sessions => "sessions",
        SearchCategory::Artifacts => "artifacts",
        SearchCategory::Knowledge => "knowledge",
        SearchCategory::KnowledgeHistory => "knowledge_history",
        SearchCategory::Notes => "notes",
        SearchCategory::Meetings => "meetings",
        SearchCategory::Sources => "sources",
        SearchCategory::Notebooks => "notebooks",
    }
}

/// Position in [`SearchCategory::ALL`].
fn idx(c: SearchCategory) -> usize {
    SearchCategory::ALL
        .iter()
        .position(|x| *x == c)
        .unwrap_or_default()
}

/// What the menu asks of `rpc::Api`; `generation` comes back on the reply.
#[derive(Debug)]
pub struct Request {
    /// View identity when asked.
    pub generation: u64,
    /// The call.
    pub kind: RequestKind,
}

/// The calls behind the menu.
#[derive(Debug)]
pub enum RequestKind {
    /// `search(query, categories, limit)`: the groups sharing one limit.
    Results {
        /// Trimmed query.
        query: String,
        /// Selected categories at this limit.
        categories: Vec<SearchCategory>,
        /// Per-group limit.
        limit: u32,
    },
    /// `list_sessions`, for the blank-query **Recent sessions** rows.
    Recent,
    /// `search_refresh(categories)`: reset the selected owners' failed state.
    Refresh(Vec<SearchCategory>),
    /// `get_artifact`, the file's bytes read off the UI thread, and
    /// `artifact_search_match` when the hit was a content/source match.
    Artifact {
        /// Exact record.
        call_id: String,
        /// Query for the matched-text panel; `None` for a metadata hit.
        query: Option<String>,
    },
    /// `knowledge_get`: the current concept reader.
    Concept(String),
    /// `knowledge_read_revision`: the historical reader.
    Historical {
        /// Concept id.
        id: String,
        /// Full commit whose tree holds the text.
        commit: String,
    },
    /// `knowledge_revision`: the change's diff.
    Revision {
        /// Full commit.
        commit: String,
        /// Concept id.
        id: String,
    },
    /// `load_session(session, seq + 1, PAGE)`: the page ending at the hit.
    Page {
        /// Session.
        session: SessionId,
        /// Target message seq.
        seq: i64,
    },
}

/// What came back, tagged in `UiEvent::Search(generation, reply)`.
#[derive(Debug)]
// One `Loaded` page per reply; a short-lived message, not worth a Box.
#[allow(clippy::large_enum_variant)]
pub enum Reply {
    /// Answer to [`RequestKind::Results`]; `Err` is the whole call failing.
    Results {
        /// Categories asked.
        categories: Vec<SearchCategory>,
        /// Limit asked.
        limit: u32,
        /// The groups, or why not.
        result: Result<SearchResponse, String>,
    },
    /// Answer to [`RequestKind::Recent`].
    Recent(Result<Vec<SessionSummary>, String>),
    /// Answer to [`RequestKind::Refresh`].
    Refreshed(Result<(), String>),
    /// Answer to [`RequestKind::Artifact`].
    Artifact(Result<ArtifactLoad, String>),
    /// Answer to [`RequestKind::Concept`].
    Concept(Result<Concept, String>),
    /// Answer to [`RequestKind::Historical`].
    Historical(Result<HistoricalConcept, String>),
    /// Answer to [`RequestKind::Revision`].
    Revision(Result<Revision, String>),
    /// Answer to [`RequestKind::Page`].
    Page {
        /// Session.
        session: SessionId,
        /// Target seq.
        seq: i64,
        /// The page.
        result: Result<Loaded, String>,
    },
}

/// An artifact fetched for its preview: the exact record, the file's bytes
/// for text-like kinds (read off the UI thread), and the matched text when asked.
#[derive(Debug)]
pub struct ArtifactLoad {
    /// The record.
    pub artifact: artifact::Artifact,
    /// File bytes for markdown/text/html/csv kinds; empty for others.
    pub bytes: Result<Vec<u8>, String>,
    /// `artifact_search_match` when the hit was a content/source match.
    pub matched: Option<Result<ArtifactMatchContext, String>>,
}

/// One hit of any category.
#[derive(Clone, Debug)]
pub enum Hit {
    /// A session's best title/message candidate.
    Session(SearchHit),
    /// One `call_id` occurrence.
    Artifact(ArtifactSearchHit),
    /// One current concept.
    Knowledge(ConceptSearchHit),
    /// One changed concept in one commit.
    History(HistorySearchHit),
    /// One free-standing note (66 §10).
    Note(NoteSearchHit),
    /// One meeting (66 §10).
    Meeting(MeetingSearchHit),
}

/// Stable identity of a selectable row: keeps the cursor on the same thing
/// across a same-query refresh (49 §2.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    /// A session hit.
    Session(SessionId),
    /// An artifact hit.
    Artifact(String),
    /// A concept hit.
    Knowledge(String),
    /// `(id, commit)` of a history hit.
    History(String, String),
    /// A note hit.
    Note(String),
    /// A meeting hit.
    Meeting(meeting::MeetingId),
    /// A group's `show more` row.
    More(SearchCategory),
    /// A recent-sessions row.
    Recent(SessionId),
}

impl Hit {
    /// The row's identity.
    pub fn key(&self) -> Key {
        match self {
            Hit::Session(h) => Key::Session(h.session),
            Hit::Artifact(h) => Key::Artifact(h.artifact.call_id.clone()),
            Hit::Knowledge(h) => Key::Knowledge(h.meta.id.clone()),
            Hit::History(h) => Key::History(h.id.clone(), h.commit.clone()),
            Hit::Note(h) => Key::Note(h.doc.id.to_string()),
            Hit::Meeting(h) => Key::Meeting(h.meeting.id),
        }
    }

    /// First row: what the hit is.
    pub fn title(&self) -> String {
        match self {
            Hit::Session(h) => h.title.clone(),
            Hit::Artifact(h)
                if h.artifact.title.is_empty() || h.artifact.title == h.artifact.name =>
            {
                h.artifact.name.clone()
            }
            Hit::Artifact(h) => format!("{} · {}", h.artifact.name, h.artifact.title),
            Hit::Knowledge(h) => format!("{} · {}", h.meta.title, h.meta.id),
            Hit::History(h) => format!("{} · {}", h.title, h.id),
            Hit::Note(h) if h.doc.title.trim().is_empty() => {
                format!("Untitled note · {}", h.doc.id)
            }
            Hit::Note(h) => h.doc.title.clone(),
            Hit::Meeting(h) => meeting::search::title_of(&h.meeting).to_owned(),
        }
    }

    /// Muted provenance beside the title (49 §2.3).
    pub fn provenance(&self) -> String {
        match self {
            Hit::Session(h) => {
                let who = match (h.kind, h.role) {
                    (HitKind::Title, _) => "title",
                    (_, Some(HitRole::User)) => "user",
                    (_, Some(HitRole::Assistant)) => "assistant",
                    (_, None) => "message",
                };
                let mut s = format!("{who} · {}", when(&h.updated_at));
                if h.matches > 1 {
                    let _ = write!(s, " · {} matches", h.matches);
                }
                if h.context_removed {
                    s.push_str(" · ⊘ removed from context");
                }
                s
            }
            Hit::Artifact(h) => {
                let a = &h.artifact;
                let created = chrono::DateTime::from_timestamp(a.created_at, 0).map_or_else(
                    String::new,
                    |t| {
                        t.with_timezone(&chrono::Local)
                            .format("%Y-%m-%d %H:%M")
                            .to_string()
                    },
                );
                let mut s = format!(
                    "v{} · {} · {} · {created}",
                    a.version,
                    a.kind.as_str(),
                    short(a.session)
                );
                if h.content_coverage == ContentCoverage::MetadataOnly {
                    s.push_str(" · metadata only");
                }
                s
            }
            Hit::Knowledge(h) => {
                let mut s = format!("[{}]", h.meta.type_);
                for t in &h.meta.tags {
                    let _ = write!(s, " #{t}");
                }
                s
            }
            Hit::History(h) => format!(
                "{} · {} · {} · {}",
                status_word(h.status),
                h.commit.get(..7).unwrap_or(&h.commit),
                when(&h.time),
                h.message
            ),
            Hit::Note(h) => format!("note · updated {}", when(&ms_rfc3339(h.doc.updated_ms))),
            Hit::Meeting(h) => {
                let m = &h.meeting;
                let mut s = format!("meeting · {}", when(&m.started_at));
                if let Some(d) = m.duration_ms {
                    let _ = write!(s, " · {} min", d / 60_000);
                }
                if m.state != meeting::MeetingState::Done {
                    let _ = write!(s, " · {:?}", m.state);
                }
                s
            }
        }
    }

    /// Second row: `(candidate label, snippet, ranges)`.
    pub fn snippet(&self) -> (&'static str, &str, &[MatchRange]) {
        match self {
            Hit::Session(h) => ("", h.snippet.as_str(), h.ranges.as_slice()),
            Hit::Artifact(h) => (
                field_label(&h.r#match.field),
                h.r#match.snippet.as_str(),
                h.r#match.ranges.as_slice(),
            ),
            Hit::Knowledge(h) => (
                field_label(&h.r#match.field),
                h.r#match.snippet.as_str(),
                h.r#match.ranges.as_slice(),
            ),
            Hit::History(h) => (
                match h.side {
                    HistorySide::Before => "Before change: ",
                    HistorySide::After => "After change: ",
                    HistorySide::Metadata => "metadata: ",
                },
                h.r#match.snippet.as_str(),
                h.r#match.ranges.as_slice(),
            ),
            Hit::Note(h) => (
                field_label(&h.r#match.field),
                h.r#match.snippet.as_str(),
                h.r#match.ranges.as_slice(),
            ),
            Hit::Meeting(h) => (
                field_label(&h.r#match.field),
                h.r#match.snippet.as_str(),
                h.r#match.ranges.as_slice(),
            ),
        }
    }

    /// Exact identity for the line printer.
    pub fn identity(&self) -> String {
        match self {
            Hit::Session(h) => match h.seq {
                Some(seq) => format!("session {} seq {seq}", h.session),
                None => format!("session {}", h.session),
            },
            Hit::Artifact(h) => format!("artifact {}", h.artifact.call_id),
            Hit::Knowledge(h) => format!("concept {}", h.meta.id),
            Hit::History(h) => match &h.snapshot_commit {
                Some(s) => format!("{} @ {} snapshot {s}", h.id, h.commit),
                None => format!("{} @ {}", h.id, h.commit),
            },
            Hit::Note(h) => format!("note {}", h.doc.id),
            Hit::Meeting(h) => format!("meeting {}", h.meeting.id),
        }
    }
}

/// Unix milliseconds as RFC 3339 UTC, for [`when`].
pub(crate) fn ms_rfc3339(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default()
}

/// `content: ` / `source: ` / `metadata: ` / `title: ` …; empty for a body.
fn field_label(field: &str) -> &'static str {
    match field {
        "body" => "",
        "content" => "content: ",
        "source" => "source: ",
        "metadata" => "metadata: ",
        "name" => "name: ",
        "title" => "title: ",
        "kind" => "kind: ",
        "transcript" => "transcript: ",
        _ => "",
    }
}

fn status_word(s: ChangeStatus) -> &'static str {
    match s {
        ChangeStatus::Added => "added",
        ChangeStatus::Modified => "modified",
        ChangeStatus::Deleted => "deleted",
    }
}

/// One category's answer as shown, stamped with the generation it answers.
#[derive(Debug)]
pub struct Group {
    /// Category.
    pub category: SearchCategory,
    /// Generation of the request it answered; stale when behind the view's.
    pub generation: u64,
    /// Limit it answered.
    pub limit: u32,
    /// Page state.
    pub state: PageState,
    /// Ranked hits.
    pub hits: Vec<Hit>,
    /// A further hit exists.
    pub has_more: bool,
    /// Coverage.
    pub coverage: Coverage,
    /// Reasons.
    pub notices: Vec<SearchNotice>,
}

impl Group {
    fn from(g: SearchGroup, generation: u64, limit: u32) -> Self {
        fn page<T>(
            category: SearchCategory,
            p: SearchPage<T>,
            wrap: fn(T) -> Hit,
            generation: u64,
            limit: u32,
        ) -> Group {
            Group {
                category,
                generation,
                limit,
                state: p.state,
                hits: p.hits.into_iter().map(wrap).collect(),
                has_more: p.has_more,
                coverage: p.coverage,
                notices: p.notices,
            }
        }
        match g {
            SearchGroup::Sessions { page: p } => {
                page(SearchCategory::Sessions, p, Hit::Session, generation, limit)
            }
            SearchGroup::Artifacts { page: p } => page(
                SearchCategory::Artifacts,
                p,
                Hit::Artifact,
                generation,
                limit,
            ),
            SearchGroup::Knowledge { page: p } => page(
                SearchCategory::Knowledge,
                p,
                Hit::Knowledge,
                generation,
                limit,
            ),
            SearchGroup::KnowledgeHistory { page: p } => page(
                SearchCategory::KnowledgeHistory,
                p,
                Hit::History,
                generation,
                limit,
            ),
            SearchGroup::Notes { page: p } => {
                page(SearchCategory::Notes, p, Hit::Note, generation, limit)
            }
            SearchGroup::Meetings { page: p } => {
                page(SearchCategory::Meetings, p, Hit::Meeting, generation, limit)
            }
            // 144 §7.4: block-level groups; the TUI rows for them land with the index (W2/W4).
            SearchGroup::Sources { page: p } => Group {
                category: SearchCategory::Sources,
                generation,
                limit,
                state: p.state,
                hits: Vec::new(),
                has_more: p.has_more,
                coverage: p.coverage,
                notices: p.notices,
            },
            SearchGroup::Notebooks { page: p } => Group {
                category: SearchCategory::Notebooks,
                generation,
                limit,
                state: p.state,
                hits: Vec::new(),
                has_more: p.has_more,
                coverage: p.coverage,
                notices: p.notices,
            },
        }
    }

    fn error(category: SearchCategory, message: String, generation: u64, limit: u32) -> Self {
        Group {
            category,
            generation,
            limit,
            state: PageState::Error,
            hits: Vec::new(),
            has_more: false,
            coverage: Coverage::Partial,
            notices: vec![SearchNotice {
                code: "error".into(),
                message,
                affected: None,
            }],
        }
    }

    /// Notices joined for one line.
    fn reasons(&self) -> String {
        let mut s = String::new();
        for n in &self.notices {
            if !s.is_empty() {
                s.push_str("; ");
            }
            s.push_str(&n.message);
            if let Some(k) = n.affected {
                let _ = write!(s, " ({k})");
            }
        }
        s
    }
}

/// One drawn line of the result list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    /// Group heading; never selectable.
    Header(String),
    /// `groups[g].hits[i]`: title row plus snippet row.
    Hit(usize, usize),
    /// `groups[g]`'s `show more` action.
    More(usize),
    /// A status, notice or guidance line; never selectable.
    Note(String),
    /// `recent[i]`.
    Recent(usize),
}

impl Row {
    /// The cursor can rest here.
    pub fn selectable(&self) -> bool {
        matches!(self, Row::Hit(..) | Row::More(_) | Row::Recent(_))
    }

    /// Terminal rows the row takes; hits lose their snippet in `compact`.
    pub fn height(&self, compact: bool) -> usize {
        match self {
            Row::Hit(..) if !compact => 2,
            _ => 1,
        }
    }
}

/// Which part of the menu has the keys (49 §6.1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Focus {
    /// The query editor.
    #[default]
    Query,
    /// `All` (0) or chip `1..=N` (the category count).
    Filters(usize),
    /// The result rows.
    Results,
    /// Footer action: 0 refresh, 1 help.
    Footer(usize),
}

/// Footer actions, in order.
pub const FOOTER: [&str; 2] = ["↻ Refresh", "? Help"];

/// A search-owned read-only view over the results (49 §6.2, §7). Esc pops
/// back to the results with query, chips, cursor and scroll untouched.
#[derive(Debug)]
pub enum Preview {
    /// Exact artifact record: body by kind, `o` opens, `e` exports, `m`
    /// swaps in the matched text.
    Artifact {
        /// The record.
        artifact: Box<artifact::Artifact>,
        /// Rendered body (35 §3.2).
        body: Box<markdown::Text>,
        /// `artifact_search_match` for a content/source hit.
        matched: Option<Result<ArtifactMatchContext, String>>,
        /// The matched-text panel replaces the body.
        panel: bool,
        /// First shown body line.
        scroll: usize,
    },
    /// Current concept, verbatim, read-only.
    Concept {
        /// The concept.
        concept: Box<Concept>,
        /// First shown wrapped row.
        scroll: usize,
    },
    /// A concept's complete text at one commit, read-only.
    Historical {
        /// The hit that opened it (side, commit, subject).
        hit: Box<HistorySearchHit>,
        /// The text.
        text: HistoricalConcept,
        /// First shown wrapped row.
        scroll: usize,
    },
    /// One change's diff (the existing history renderer).
    Changes {
        /// The revision.
        revision: Box<Revision>,
        /// First shown row.
        row: usize,
        /// Leading columns dropped.
        col: usize,
    },
    /// Query syntax help (49 §2.2).
    Help,
}

/// Query syntax help lines.
pub const HELP: &[&str] = &[
    "fzf syntax, per candidate text: smart case, fuzzy words.",
    "  'exact    substring    ^prefix    suffix$    !not",
    "A query matches ONE candidate at a time: a title, one message, one",
    "artifact's content, one concept body, one before/after revision text.",
    "`!old` excludes candidates containing old — not every object mentioning it.",
    "Metadata candidates join id/name, title, description, tags and type.",
    "Limits: 10 per category, show more → 30 → 50; the count never claims a total.",
    "Enter opens the selected hit; a second Enter is never automatic.",
];

impl Preview {
    /// Title row.
    pub fn title(&self) -> String {
        match self {
            Preview::Artifact {
                artifact, panel, ..
            } => format!(
                "{} · v{} · {} · {}{}   o open  e export{}  esc back",
                artifact.name,
                artifact.version,
                artifact.kind.as_str(),
                crate::components::artifacts_apps::bytes_label(artifact.bytes),
                if *panel { " · matched text" } else { "" },
                if *panel {
                    "  m content"
                } else {
                    "  m matched text"
                },
            ),
            Preview::Concept { concept, .. } => format!(
                "{} · {} · [{}] · read-only   ↑↓ scroll  esc back",
                concept.meta.title, concept.meta.id, concept.meta.type_
            ),
            Preview::Historical { hit, text, .. } => format!(
                "Historical version · {} · {} · {} · {} · {}   c changes  ↑↓ scroll  esc back",
                text.id,
                match hit.side {
                    HistorySide::Before => "Before change",
                    HistorySide::After => "After change",
                    HistorySide::Metadata => "metadata",
                },
                when(&hit.time),
                text.commit.get(..7).unwrap_or(&text.commit),
                hit.message
            ),
            Preview::Changes { revision, .. } => format!(
                "changes · {} · {} · {} · {} file{}   ↑↓←→ scroll  esc back",
                revision.change.commit,
                revision.change.time,
                revision.change.message,
                revision.files.len(),
                if revision.files.len() == 1 { "" } else { "s" }
            ),
            Preview::Help => "search help   esc back".into(),
        }
    }

    /// Plain rows for the text previews, wrapped at `width` cells; `None`
    /// for the artifact body (markdown lines, drawn by the renderer).
    pub fn rows(&self, width: usize) -> Option<Vec<String>> {
        match self {
            Preview::Artifact {
                matched,
                panel: true,
                ..
            } => Some(match matched {
                None => vec![
                    "no matched text: this hit matched the record's name, title or kind".into(),
                ],
                Some(Err(e)) => vec![format!("matched text unavailable: {e}")],
                Some(Ok(m)) => {
                    let mut rows = vec![format!(
                        "{}{}",
                        field_label(&m.r#match.field).trim_end(),
                        if m.content_coverage == ContentCoverage::MetadataOnly {
                            " · metadata only"
                        } else {
                            ""
                        }
                    )];
                    rows.extend(wrap_cells(&m.r#match.snippet, width));
                    rows
                }
            }),
            Preview::Artifact { .. } => None,
            Preview::Concept { concept, .. } => Some(text_rows(&concept.text, width)),
            Preview::Historical { text, .. } => Some(text_rows(&text.text, width)),
            Preview::Changes { revision, col, .. } => Some(
                revision_rows(revision)
                    .into_iter()
                    .map(|r| if *col == 0 { r } else { skip_cells(&r, *col) })
                    .collect(),
            ),
            Preview::Help => Some(HELP.iter().map(|s| (*s).to_owned()).collect()),
        }
    }

    fn scroll_mut(&mut self) -> Option<&mut usize> {
        match self {
            Preview::Artifact { scroll, .. }
            | Preview::Concept { scroll, .. }
            | Preview::Historical { scroll, .. } => Some(scroll),
            Preview::Changes { row, .. } => Some(row),
            Preview::Help => None,
        }
    }

    /// First shown row.
    pub fn scroll(&self) -> usize {
        match self {
            Preview::Artifact { scroll, .. }
            | Preview::Concept { scroll, .. }
            | Preview::Historical { scroll, .. } => *scroll,
            Preview::Changes { row, .. } => *row,
            Preview::Help => 0,
        }
    }
}

/// `text` as rows: each line wrapped at `width` cells, capped at
/// [`PREVIEW_CHARS`] with a closing notice.
fn text_rows(text: &str, width: usize) -> Vec<String> {
    let (shown, cut) = match text.char_indices().nth(PREVIEW_CHARS) {
        Some((i, _)) => (&text[..i], true),
        None => (text, false),
    };
    let mut rows: Vec<String> = shown.lines().flat_map(|l| wrap_cells(l, width)).collect();
    if cut {
        rows.push(format!("… (cut at {PREVIEW_CHARS} chars)"));
    }
    if rows.is_empty() {
        rows.push(String::new());
    }
    rows
}

/// Line index of `snippet`'s core in `text`, to open a reader at the match
/// (49 §7: coordinates survive in raw text). `None` when not found verbatim.
pub fn locate(text: &str, snippet: &str) -> Option<usize> {
    let core = snippet.trim_matches('…').trim();
    if core.is_empty() {
        return None;
    }
    let at = text.find(core)?;
    Some(text[..at].matches('\n').count())
}

/// The **Earlier messages** inspection (49 §7): a page ending at an old
/// match, shown in place of the live transcript and isolated from its events.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inspect {
    /// Session.
    pub session: SessionId,
    /// Target message seq.
    pub seq: i64,
    /// The page held the target (else it was removed/hidden: a notice says so).
    pub found: bool,
}

/// What a key on the results asks the model to do.
#[derive(Debug)]
pub enum Act {
    /// Nothing beyond state.
    None,
    /// Send these.
    Send(Vec<Request>),
    /// Switch to this session (title hit, recent row).
    Resume(SessionId),
}

/// What a reply asks the model to do beyond state.
#[derive(Debug)]
// One `Loaded` page per reply; a short-lived message, not worth a Box.
#[allow(clippy::large_enum_variant)]
pub enum Effect {
    /// Nothing beyond state.
    None,
    /// Send these.
    Send(Vec<Request>),
    /// Build the artifact preview (needs the renderer's markdown options).
    Artifact(ArtifactLoad),
    /// Show the page as the inspection view.
    Inspect {
        /// Session.
        session: SessionId,
        /// Target seq.
        seq: i64,
        /// The page.
        loaded: Loaded,
    },
}

/// The search menu's state (49 §6.1). Lives on the model across session
/// switches: chips survive, and **Back to results** restores the last view.
#[derive(Debug)]
pub struct State {
    /// Editable query, verbatim (the request trims it).
    pub query: String,
    /// Chip bits in [`SearchCategory::ALL`] order; any subset, empty allowed.
    pub selected: [bool; N],
    /// Which part has the keys.
    pub focus: Focus,
    /// Current view identity; every request carries it.
    pub generation: u64,
    /// Trimmed query the current generation asked for.
    requested: String,
    /// Debounce deadline (unix ms).
    deadline: Option<u64>,
    /// Next `indexing` re-ask (unix ms).
    poll_at: Option<u64>,
    /// Result requests of `generation` not yet answered.
    in_flight: usize,
    /// Per-group limit, [`SearchCategory::ALL`] order.
    pub limits: [u32; N],
    /// Groups shown, canonical order; each stamped with its generation.
    pub groups: Vec<Group>,
    /// Local validation failure of the current query.
    failed: Option<String>,
    /// Recent sessions for the blank query; `None` until fetched.
    pub recent: Option<Vec<SessionSummary>>,
    /// A recent-sessions request is out.
    recent_loading: bool,
    /// Drawn rows, rebuilt on every change.
    pub rows: Vec<Row>,
    /// Highlighted row (index into `rows`, always selectable when any is).
    pub cursor: usize,
    /// Identity of the highlighted row, taken when the cursor lands there;
    /// `rebuild` follows it after the groups underneath have changed.
    anchor: Option<Key>,
    /// First shown row; the renderer clamps it to keep the cursor in view.
    pub scroll: usize,
    /// `search_refresh` is out.
    pub refreshing: bool,
    /// Transient line: a failed refresh or open.
    pub notice: Option<String>,
    /// A destination fetch is out: what it is.
    pub opening: Option<String>,
    /// Open previews, innermost last.
    pub previews: Vec<Preview>,
    /// A source changed while closed: re-ask on restore.
    dirty: bool,
    /// The history hit whose text is being fetched, for the reader's title
    /// and its **Changes** action.
    pending_hit: Option<HistorySearchHit>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            query: String::new(),
            selected: [true; N],
            focus: Focus::Query,
            generation: 0,
            requested: String::new(),
            deadline: None,
            poll_at: None,
            in_flight: 0,
            limits: [LIMITS[0]; N],
            groups: Vec::new(),
            failed: None,
            recent: None,
            recent_loading: false,
            rows: Vec::new(),
            cursor: 0,
            anchor: None,
            scroll: 0,
            refreshing: false,
            notice: None,
            opening: None,
            previews: Vec::new(),
            dirty: false,
            pending_hit: None,
        }
    }
}

impl State {
    /// Selected categories in canonical order.
    pub fn categories(&self) -> Vec<SearchCategory> {
        SearchCategory::ALL
            .into_iter()
            .filter(|c| self.selected[idx(*c)])
            .collect()
    }

    /// Every chip is on: the `All` action's selected look (49 §2.1).
    pub fn all(&self) -> bool {
        self.selected.iter().all(|b| *b)
    }

    /// A fresh view: empty (or prefilled) query, remembered chips. Late
    /// replies to the old view are inert from here. A prefilled query runs
    /// at once (49 §6); a blank one asks for the recent sessions.
    pub fn open(&mut self, query: &str) -> Vec<Request> {
        self.generation += 1;
        self.query = query.to_owned();
        self.requested.clear();
        self.focus = Focus::Query;
        self.deadline = None;
        self.poll_at = None;
        self.in_flight = 0;
        self.limits = [LIMITS[0]; N];
        self.groups.clear();
        self.failed = None;
        self.recent = None;
        self.recent_loading = false;
        self.cursor = 0;
        self.anchor = None;
        self.scroll = 0;
        self.refreshing = false;
        self.notice = None;
        self.opening = None;
        self.previews.clear();
        self.dirty = false;
        self.pending_hit = None;
        self.fire()
    }

    /// The menu closed: nothing in flight may land, the view stays for
    /// **Back to results**. A settled view stays activatable there; an
    /// unanswered one is re-asked on restore.
    pub fn close(&mut self) {
        self.dirty |= self.pending();
        self.generation += 1;
        if !self.dirty {
            for g in &mut self.groups {
                g.generation = self.generation;
            }
        }
        self.deadline = None;
        self.poll_at = None;
        self.in_flight = 0;
        self.refreshing = false;
        self.opening = None;
        self.previews.clear();
        self.rebuild();
    }

    /// **Back to results**: the view as left; re-asked when a source changed
    /// meanwhile or an answer was still out (rows stay until the answer).
    pub fn restore(&mut self) -> Vec<Request> {
        self.notice = None;
        if std::mem::take(&mut self.dirty) {
            self.generation += 1;
            return self.fire();
        }
        Vec::new()
    }

    /// The editor changed: a new trimmed text starts a new generation and
    /// the debounce; a blank one clears the groups now and asks for recent
    /// sessions on the next frame.
    pub fn edit(&mut self, text: String, now: u64) {
        self.query = text;
        let trimmed = self.query.trim();
        if trimmed == self.requested && self.failed.is_none() {
            return;
        }
        self.generation += 1;
        self.in_flight = 0;
        self.poll_at = None;
        self.limits = [LIMITS[0]; N];
        self.failed = None;
        self.notice = None;
        self.deadline = Some(if trimmed.is_empty() {
            now
        } else {
            now + DEBOUNCE_MS
        });
        self.rebuild();
    }

    /// `All` (chip 0) selects every category (a no-op when all are on);
    /// chips `1..=N` toggle. Query and focus stay; the request starts now.
    pub fn toggle(&mut self, chip: usize) -> Vec<Request> {
        match chip {
            0 => {
                if self.all() {
                    return Vec::new();
                }
                self.selected = [true; N];
            }
            1..=N => self.selected[chip - 1] = !self.selected[chip - 1],
            _ => return Vec::new(),
        }
        self.generation += 1;
        self.limits = [LIMITS[0]; N];
        self.groups.retain(|g| self.selected[idx(g.category)]);
        self.deadline = None;
        self.poll_at = None;
        self.fire()
    }

    /// When the event loop must wake for a due request, unix ms.
    pub fn wake(&self) -> Option<u64> {
        match (self.deadline, self.poll_at) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Send whatever is due at `now`.
    pub fn tick(&mut self, now: u64) -> Vec<Request> {
        if self.deadline.is_some_and(|d| d <= now) {
            return self.fire();
        }
        if self.poll_at.is_some_and(|d| d <= now) {
            return self.poll();
        }
        Vec::new()
    }

    /// Ask for the current query at the current chips and limits: one
    /// request per distinct limit. A blank query asks nothing (recent
    /// sessions at most); an over-long one fails locally.
    fn fire(&mut self) -> Vec<Request> {
        self.deadline = None;
        self.poll_at = None;
        self.requested = self.query.trim().to_owned();
        let generation = self.generation;
        let mut out = Vec::new();
        if self.requested.is_empty() {
            self.groups.clear();
            self.in_flight = 0;
            self.failed = None;
            if self.selected[0] && self.recent.is_none() && !self.recent_loading {
                self.recent_loading = true;
                out.push(Request {
                    generation,
                    kind: RequestKind::Recent,
                });
            }
            self.rebuild();
            return out;
        }
        if let Err(e) = validate_query(&self.requested) {
            self.failed = Some(e);
            self.groups.clear();
            self.in_flight = 0;
            self.rebuild();
            return out;
        }
        self.failed = None;
        let cats = self.categories();
        for limit in LIMITS {
            let categories: Vec<SearchCategory> = cats
                .iter()
                .copied()
                .filter(|c| self.limits[idx(*c)] == limit)
                .collect();
            if !categories.is_empty() {
                out.push(Request {
                    generation,
                    kind: RequestKind::Results {
                        query: self.requested.clone(),
                        categories,
                        limit,
                    },
                });
            }
        }
        self.in_flight = out.len();
        self.rebuild();
        out
    }

    /// Re-ask only the selected groups still `indexing`, same generation.
    fn poll(&mut self) -> Vec<Request> {
        self.poll_at = None;
        let generation = self.generation;
        let mut out = Vec::new();
        for limit in LIMITS {
            let categories: Vec<SearchCategory> = self
                .groups
                .iter()
                .filter(|g| {
                    g.state == PageState::Indexing
                        && self.selected[idx(g.category)]
                        && self.limits[idx(g.category)] == limit
                })
                .map(|g| g.category)
                .collect();
            if !categories.is_empty() {
                out.push(Request {
                    generation,
                    kind: RequestKind::Results {
                        query: self.requested.clone(),
                        categories,
                        limit,
                    },
                });
            }
        }
        self.in_flight += out.len();
        out
    }

    /// A source changed (bus event): a shown non-blank query re-asks after
    /// the debounce while open; a closed view re-asks on restore. Blank
    /// queries drop the recent list so it is fetched again.
    pub fn invalidate(&mut self, category: SearchCategory, open: bool, now: u64) {
        if !self.selected[idx(category)] {
            return;
        }
        if category == SearchCategory::Sessions {
            self.recent = None;
            self.recent_loading = false;
        }
        if !open {
            self.dirty = true;
            return;
        }
        if self.query.trim().is_empty() {
            if category == SearchCategory::Sessions {
                self.deadline = Some(self.deadline.map_or(now, |d| d.min(now)));
            }
            return;
        }
        // Same identity: the answer replaces the group under the same cursor key.
        if self.deadline.is_none() {
            self.deadline = Some(now + DEBOUNCE_MS);
        }
    }

    /// The editor's text is newer than what the rows answer, or an answer
    /// is still out: Enter flushes/waits instead of opening a row.
    pub fn pending(&self) -> bool {
        self.deadline.is_some()
            || self.in_flight > 0
            || self.groups.iter().any(|g| g.generation != self.generation)
    }

    /// `show more` on `groups[g]`: the next limit, that group re-asked.
    fn more(&mut self, g: usize) -> Vec<Request> {
        let Some(group) = self.groups.get(g) else {
            return Vec::new();
        };
        let cat = group.category;
        let cur = self.limits[idx(cat)];
        let Some(next) = LIMITS.iter().copied().find(|l| *l > cur) else {
            return Vec::new();
        };
        self.limits[idx(cat)] = next;
        self.in_flight += 1;
        self.rebuild();
        vec![Request {
            generation: self.generation,
            kind: RequestKind::Results {
                query: self.requested.clone(),
                categories: vec![cat],
                limit: next,
            },
        }]
    }

    /// Footer **Refresh** / a group's **Retry**: reset the selected owners,
    /// then (on the reply) a new generation of the same query.
    pub fn refresh(&mut self) -> Vec<Request> {
        self.refreshing = true;
        self.notice = None;
        vec![Request {
            generation: self.generation,
            kind: RequestKind::Refresh(self.categories()),
        }]
    }

    /// `/attach` Enter (66 §10): the highlighted hit or recent session as a
    /// reference; `None` on a pending edit, a `show more` row, a stale group
    /// or nothing selectable — the caller falls back to [`State::enter`].
    pub fn pick(&self) -> Option<agent::attach::AttachRef> {
        if self.deadline.is_some() {
            return None;
        }
        match self.rows.get(self.cursor)? {
            Row::Recent(i) => self
                .recent
                .as_ref()?
                .get(*i)
                .map(crate::attach::from_session),
            Row::Hit(g, i) => {
                let group = self.groups.get(*g)?;
                if group.generation != self.generation {
                    return None;
                }
                group.hits.get(*i).map(crate::attach::from_hit)
            }
            Row::More(_) | Row::Header(_) | Row::Note(_) => None,
        }
    }

    /// Enter: flush a pending edit, open the highlighted row, or grow a group.
    pub fn enter(&mut self) -> Act {
        if self.deadline.is_some() {
            return Act::Send(self.fire());
        }
        let Some(row) = self.rows.get(self.cursor).cloned() else {
            return Act::None;
        };
        match row {
            Row::Recent(i) => match self.recent.as_ref().and_then(|r| r.get(i)) {
                Some(s) => Act::Resume(s.id),
                None => Act::None,
            },
            Row::More(g) => {
                if self
                    .groups
                    .get(g)
                    .is_some_and(|g| g.generation != self.generation)
                {
                    return Act::None;
                }
                Act::Send(self.more(g))
            }
            Row::Hit(g, i) => {
                let Some(group) = self.groups.get(g) else {
                    return Act::None;
                };
                if group.generation != self.generation {
                    return Act::None;
                }
                let Some(hit) = group.hits.get(i) else {
                    return Act::None;
                };
                self.activate(hit.clone())
            }
            Row::Header(_) | Row::Note(_) => Act::None,
        }
    }

    /// The typed destination of a hit (49 §7).
    fn activate(&mut self, hit: Hit) -> Act {
        match hit {
            Hit::Session(h) => match h.seq {
                None => Act::Resume(h.session),
                Some(seq) => self.ask(
                    format!("loading {} at message {seq}…", short(h.session)),
                    RequestKind::Page {
                        session: h.session,
                        seq,
                    },
                ),
            },
            Hit::Artifact(h) => {
                let content = matches!(h.r#match.field.as_str(), "content" | "source");
                let query = content.then(|| self.requested.clone());
                self.ask(
                    format!("loading {}…", h.artifact.name),
                    RequestKind::Artifact {
                        call_id: h.artifact.call_id,
                        query,
                    },
                )
            }
            Hit::Knowledge(h) => self.ask(
                format!("loading {}…", h.meta.id),
                RequestKind::Concept(h.meta.id),
            ),
            Hit::History(h) => match &h.snapshot_commit {
                Some(snapshot) => {
                    let (id, commit) = (h.id.clone(), snapshot.clone());
                    let act = self.ask(
                        format!("loading {id} at {}…", commit.get(..7).unwrap_or(&commit)),
                        RequestKind::Historical { id, commit },
                    );
                    self.pending_hit = Some(h);
                    act
                }
                None => self.ask(
                    format!(
                        "loading changes {}…",
                        h.commit.get(..7).unwrap_or(&h.commit)
                    ),
                    RequestKind::Revision {
                        commit: h.commit,
                        id: h.id,
                    },
                ),
            },
            // No TUI reader for these yet: name the id and the way to hand it to the agent.
            Hit::Note(h) => {
                self.notice = Some(format!("note {} — attach it with /attach", h.doc.id));
                Act::None
            }
            Hit::Meeting(h) => {
                self.notice = Some(format!("meeting {} — attach it with /attach", h.meeting.id));
                Act::None
            }
        }
    }

    /// A destination fetch under the current generation; the status says what.
    fn ask(&mut self, what: String, kind: RequestKind) -> Act {
        self.opening = Some(what);
        self.notice = None;
        Act::Send(vec![Request {
            generation: self.generation,
            kind,
        }])
    }

    /// Fold a tagged reply in; anything not answering `self.generation` is inert.
    pub fn reply(&mut self, generation: u64, reply: Reply, now: u64) -> Effect {
        if generation != self.generation {
            return Effect::None;
        }
        match reply {
            Reply::Results {
                categories,
                limit,
                result,
            } => {
                self.in_flight = self.in_flight.saturating_sub(1);
                match result {
                    Ok(resp) => {
                        for g in resp.groups {
                            let cat = g.category();
                            if self.limits[idx(cat)] != limit || !self.selected[idx(cat)] {
                                continue;
                            }
                            self.set_group(Group::from(g, generation, limit));
                        }
                    }
                    Err(e) => {
                        for cat in categories {
                            if self.limits[idx(cat)] == limit && self.selected[idx(cat)] {
                                self.set_group(Group::error(cat, e.clone(), generation, limit));
                            }
                        }
                    }
                }
                if self
                    .groups
                    .iter()
                    .any(|g| g.state == PageState::Indexing && self.selected[idx(g.category)])
                {
                    self.poll_at = Some(now + POLL_MS);
                }
                self.rebuild();
                Effect::None
            }
            Reply::Recent(result) => {
                self.recent_loading = false;
                match result {
                    Ok(list) => {
                        let mut list: Vec<SessionSummary> = list
                            .into_iter()
                            .filter(|s| s.parent.is_none() && s.desk.is_none())
                            .collect();
                        list.truncate(RECENT);
                        self.recent = Some(list);
                    }
                    Err(e) => self.notice = Some(format!("recent sessions: {e}")),
                }
                self.rebuild();
                Effect::None
            }
            Reply::Refreshed(result) => {
                self.refreshing = false;
                match result {
                    Ok(()) => {
                        self.generation += 1;
                        Effect::Send(self.fire())
                    }
                    Err(e) => {
                        self.notice = Some(format!("refresh failed: {e}"));
                        self.rebuild();
                        Effect::None
                    }
                }
            }
            Reply::Artifact(result) => {
                self.opening = None;
                match result {
                    Ok(load) => Effect::Artifact(load),
                    Err(e) => {
                        self.notice = Some(format!("cannot open artifact: {e}"));
                        Effect::None
                    }
                }
            }
            Reply::Concept(result) => {
                self.opening = None;
                match result {
                    Ok(concept) => {
                        let scroll = self
                            .selected_snippet()
                            .and_then(|s| locate(&concept.text, &s))
                            .unwrap_or(0);
                        self.previews.push(Preview::Concept {
                            concept: Box::new(concept),
                            scroll,
                        });
                    }
                    Err(e) => self.notice = Some(format!("cannot open concept: {e}")),
                }
                Effect::None
            }
            Reply::Historical(result) => {
                self.opening = None;
                let hit = self.pending_hit.take();
                match (result, hit) {
                    (Ok(text), Some(hit)) => {
                        let scroll = locate(&text.text, &hit.r#match.snippet).unwrap_or(0);
                        self.previews.push(Preview::Historical {
                            hit: Box::new(hit),
                            text,
                            scroll,
                        });
                    }
                    (Ok(_), None) => {}
                    (Err(e), _) => self.notice = Some(format!("cannot read revision: {e}")),
                }
                Effect::None
            }
            Reply::Revision(result) => {
                self.opening = None;
                match result {
                    Ok(revision) => self.previews.push(Preview::Changes {
                        revision: Box::new(revision),
                        row: 0,
                        col: 0,
                    }),
                    Err(e) => self.notice = Some(format!("cannot load changes: {e}")),
                }
                Effect::None
            }
            Reply::Page {
                session,
                seq,
                result,
            } => {
                self.opening = None;
                match result {
                    Ok(loaded) => Effect::Inspect {
                        session,
                        seq,
                        loaded,
                    },
                    Err(e) => {
                        self.notice = Some(format!("cannot load session page: {e}"));
                        Effect::None
                    }
                }
            }
        }
    }

    /// Snippet of the highlighted hit, to open its reader at the match.
    fn selected_snippet(&self) -> Option<String> {
        match self.rows.get(self.cursor)? {
            Row::Hit(g, i) => Some(self.groups.get(*g)?.hits.get(*i)?.snippet().1.to_owned()),
            _ => None,
        }
    }

    /// Replace or insert a group, canonical order.
    fn set_group(&mut self, group: Group) {
        match self
            .groups
            .iter()
            .position(|g| g.category == group.category)
        {
            Some(i) => self.groups[i] = group,
            None => {
                let at = self
                    .groups
                    .iter()
                    .position(|g| idx(g.category) > idx(group.category))
                    .unwrap_or(self.groups.len());
                self.groups.insert(at, group);
            }
        }
    }

    /// Identity of `rows[row]`, when selectable.
    pub fn key_at(&self, row: usize) -> Option<Key> {
        match self.rows.get(row)? {
            Row::Hit(g, i) => Some(self.groups.get(*g)?.hits.get(*i)?.key()),
            Row::More(g) => Some(Key::More(self.groups.get(*g)?.category)),
            Row::Recent(i) => Some(Key::Recent(self.recent.as_ref()?.get(*i)?.id)),
            Row::Header(_) | Row::Note(_) => None,
        }
    }

    /// Rows from the current groups (49 §2.3); the cursor keeps its identity
    /// when that row is still there, else the first selectable row.
    fn rebuild(&mut self) {
        let keep = self.anchor.take();
        let mut rows = Vec::new();
        let cats = self.categories();
        if let Some(e) = &self.failed {
            rows.push(Row::Note(format!("query rejected: {e}")));
        } else if self.query.trim().is_empty() {
            if cats.is_empty() {
                rows.push(Row::Note("Select at least one category.".into()));
            } else {
                rows.push(Row::Note(
                    "Type to search. fzf syntax: 'exact  ^prefix  suffix$  !not  (? help)".into(),
                ));
            }
            if self.selected[0] {
                rows.push(Row::Header("Recent sessions · not matches".into()));
                match &self.recent {
                    Some(list) if list.is_empty() => rows.push(Row::Note("no sessions".into())),
                    Some(list) => rows.extend((0..list.len()).map(Row::Recent)),
                    None => rows.push(Row::Note("loading…".into())),
                }
            }
        } else if cats.is_empty() {
            rows.push(Row::Note(
                "Select at least one category — nothing is searched.".into(),
            ));
        } else {
            for cat in cats {
                let name = label(cat);
                let Some(g) = self.groups.iter().position(|g| g.category == cat) else {
                    rows.push(Row::Header(format!("{name} · searching…")));
                    continue;
                };
                let group = &self.groups[g];
                let stale = group.generation != self.generation;
                let count = match (group.state, group.has_more) {
                    (PageState::Ready, true) => format!("{}+ matches", group.hits.len()),
                    (PageState::Ready, false) => format!("{} shown", group.hits.len()),
                    (PageState::Indexing, _) => "indexing…".to_owned(),
                    (PageState::Unavailable, _) => "unavailable".to_owned(),
                    (PageState::Error, _) => "error".to_owned(),
                };
                rows.push(Row::Header(format!(
                    "{name} · {count}{}",
                    if stale { " · updating…" } else { "" }
                )));
                match group.state {
                    PageState::Ready => {
                        if group.hits.is_empty() {
                            rows.push(Row::Note("no matches".into()));
                        }
                        rows.extend((0..group.hits.len()).map(|i| Row::Hit(g, i)));
                        if group.coverage == Coverage::Partial {
                            rows.push(Row::Note(format!("partial: {}", group.reasons())));
                        }
                        if group.has_more {
                            if group.limit >= LIMITS[LIMITS.len() - 1] {
                                rows.push(Row::Note(
                                    "More matches; refine your query or categories.".into(),
                                ));
                            } else {
                                rows.push(Row::More(g));
                            }
                        }
                    }
                    PageState::Indexing => {
                        rows.push(Row::Note(
                            "building the index… asked again every 0.5 s".into(),
                        ));
                    }
                    PageState::Unavailable => {
                        rows.push(Row::Note(format!("unavailable: {}", group.reasons())));
                    }
                    PageState::Error => {
                        rows.push(Row::Note(format!(
                            "error: {} · Refresh retries",
                            group.reasons()
                        )));
                    }
                }
            }
        }
        self.rows = rows;
        let same =
            keep.and_then(|k| (0..self.rows.len()).find(|r| self.key_at(*r).as_ref() == Some(&k)));
        self.cursor = same
            .or_else(|| self.rows.iter().position(Row::selectable))
            .unwrap_or(0);
        self.anchor = self.key_at(self.cursor);
    }

    /// The highlighted row's `show more` label, when it is one.
    pub fn more_label(&self, g: usize) -> String {
        let cur = self.groups.get(g).map_or(LIMITS[0], |g| g.limit);
        let next = LIMITS.iter().copied().find(|l| *l > cur).unwrap_or(cur);
        format!("show more ({next})")
    }

    /// Move the cursor `delta` selectable rows.
    pub fn move_cursor(&mut self, delta: isize) {
        let sel: Vec<usize> = (0..self.rows.len())
            .filter(|r| self.rows[*r].selectable())
            .collect();
        if sel.is_empty() {
            self.cursor = 0;
            self.anchor = None;
            return;
        }
        let at = sel
            .iter()
            .position(|r| *r >= self.cursor)
            .unwrap_or(sel.len() - 1);
        let next = at.saturating_add_signed(delta).min(sel.len() - 1);
        self.cursor = sel[next];
        self.anchor = self.key_at(self.cursor);
    }

    /// Keep the cursor inside a window `area` rows tall, starting at `scroll`.
    pub fn ensure_visible(&mut self, area: usize, compact: bool) {
        self.scroll = first_row(&self.rows, self.scroll, self.cursor, area, compact);
    }

    /// Status line under the chips.
    pub fn status(&self) -> String {
        if let Some(n) = &self.notice {
            return n.clone();
        }
        if let Some(o) = &self.opening {
            return o.clone();
        }
        if self.refreshing {
            return "refreshing…".into();
        }
        if self.deadline.is_some() || self.in_flight > 0 {
            return "searching…".into();
        }
        if self.query.trim().is_empty() || self.categories().is_empty() {
            return String::new();
        }
        let shown: usize = self.groups.iter().map(|g| g.hits.len()).sum();
        let bad = self
            .groups
            .iter()
            .filter(|g| matches!(g.state, PageState::Error | PageState::Unavailable))
            .count();
        let mut s = format!("{shown} shown");
        if bad > 0 {
            let _ = write!(
                s,
                " · {bad} group{} not searched",
                if bad == 1 { "" } else { "s" }
            );
        }
        s
    }

    /// The innermost preview's key: scroll, back, artifact actions,
    /// `c` for a historical text's changes. `None` when no preview is up.
    pub fn preview_key(
        &mut self,
        code: iocraft::KeyCode,
        width: usize,
        area: usize,
    ) -> Option<Act> {
        use iocraft::KeyCode as K;
        let generation = self.generation;
        let p = self.previews.last_mut()?;
        let total = match p.rows(width) {
            Some(rows) => rows.len(),
            None => match &*p {
                Preview::Artifact { body, .. } => body.view().lines().count(),
                _ => 0,
            },
        };
        let max = total.saturating_sub(area.max(1));
        let page = area.max(1);
        match code {
            K::Esc | K::Backspace => {
                self.previews.pop();
                return Some(Act::None);
            }
            K::Up => {
                if let Some(s) = p.scroll_mut() {
                    *s = s.saturating_sub(1);
                }
            }
            K::Down => {
                if let Some(s) = p.scroll_mut() {
                    *s = (*s + 1).min(max);
                }
            }
            K::PageUp => {
                if let Some(s) = p.scroll_mut() {
                    *s = s.saturating_sub(page);
                }
            }
            K::PageDown => {
                if let Some(s) = p.scroll_mut() {
                    *s = (*s + page).min(max);
                }
            }
            K::Home => {
                if let Some(s) = p.scroll_mut() {
                    *s = 0;
                }
            }
            K::End => {
                if let Some(s) = p.scroll_mut() {
                    *s = max;
                }
            }
            K::Left => {
                if let Preview::Changes { col, .. } = p {
                    *col = col.saturating_sub(8);
                }
            }
            K::Right => {
                if let Preview::Changes { col, .. } = p {
                    *col += 8;
                }
            }
            K::Char('m') => {
                if let Preview::Artifact { panel, .. } = p {
                    *panel = !*panel;
                }
            }
            K::Char('c') => {
                if let Preview::Historical { hit, .. } = p {
                    let (commit, id) = (hit.commit.clone(), hit.id.clone());
                    self.opening = Some(format!(
                        "loading changes {}…",
                        commit.get(..7).unwrap_or(&commit)
                    ));
                    return Some(Act::Send(vec![Request {
                        generation,
                        kind: RequestKind::Revision { commit, id },
                    }]));
                }
            }
            _ => {}
        }
        Some(Act::None)
    }

    /// The artifact the innermost preview shows, for `o` / `e`.
    pub fn preview_artifact(&self) -> Option<&artifact::Artifact> {
        match self.previews.last()? {
            Preview::Artifact { artifact, .. } => Some(artifact),
            _ => None,
        }
    }
}

/// First row to draw so `cursor` (and, uncompacted, its snippet row) fits in
/// `area` rows starting no later than `scroll`.
pub fn first_row(rows: &[Row], scroll: usize, cursor: usize, area: usize, compact: bool) -> usize {
    if rows.is_empty() {
        return 0;
    }
    let cursor = cursor.min(rows.len() - 1);
    let mut first = scroll.min(cursor);
    let area = area.max(1);
    while first < cursor {
        let h: usize = rows[first..=cursor].iter().map(|r| r.height(compact)).sum();
        if h <= area {
            break;
        }
        first += 1;
    }
    first
}

/// Terminal cells `s` takes.
pub fn cells(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// `s` cut to at most `max` cells, never splitting a wide character; a
/// cut text ends in `…`.
pub fn fit(s: &str, max: usize) -> String {
    if cells(s) <= max {
        return s.to_owned();
    }
    let keep = max.saturating_sub(1);
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let w = UnicodeWidthChar::width(c).unwrap_or(0);
        if used + w > keep {
            break;
        }
        used += w;
        out.push(c);
    }
    if max > 0 {
        out.push('…');
    }
    out
}

/// `s` without its first `n` cells (a wide character straddling the cut is dropped).
fn skip_cells(s: &str, n: usize) -> String {
    let mut used = 0;
    s.chars()
        .skip_while(|c| {
            if used >= n {
                return false;
            }
            used += UnicodeWidthChar::width(*c).unwrap_or(0);
            true
        })
        .collect()
}

/// Greedy word wrap at `width` cells; a word wider than the row is split by
/// cells. Never empty: an empty text is one empty row.
pub fn wrap_cells(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut rows = Vec::new();
    let mut cur = String::new();
    let mut used = 0;
    for word in text.split_whitespace() {
        let w = cells(word);
        if used > 0 && used + 1 + w > width {
            rows.push(std::mem::take(&mut cur));
            used = 0;
        } else if used > 0 {
            cur.push(' ');
            used += 1;
        }
        if w <= width - used {
            cur.push_str(word);
            used += w;
            continue;
        }
        for c in word.chars() {
            let cw = UnicodeWidthChar::width(c).unwrap_or(0);
            if used + cw > width && used > 0 {
                rows.push(std::mem::take(&mut cur));
                used = 0;
            }
            cur.push(c);
            used += cw;
        }
    }
    rows.push(cur);
    rows
}

/// `snippet` as `(text, highlighted)` runs from scalar `ranges`, control
/// characters blanked (the display boundary), cut to `max` cells.
pub fn runs(snippet: &str, ranges: &[MatchRange], max: usize) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = Vec::new();
    let mut used = 0;
    let mut range = ranges.iter().peekable();
    for (i, c) in snippet.chars().enumerate() {
        let i = u32::try_from(i).unwrap_or(u32::MAX);
        while range.peek().is_some_and(|r| r.end <= i) {
            range.next();
        }
        let hit = range.peek().is_some_and(|r| r.start <= i && i < r.end);
        let c = if c.is_control() || c.is_whitespace() {
            ' '
        } else {
            c
        };
        let w = UnicodeWidthChar::width(c).unwrap_or(0);
        if used + w > max {
            if max > 0 {
                out.push(("…".into(), false));
            }
            break;
        }
        used += w;
        match out.last_mut() {
            Some((s, h)) if *h == hit => s.push(c),
            _ => out.push((c.to_string(), hit)),
        }
    }
    out
}

/// One wrapped row of chips: `(chip index, text)` per chip.
pub type ChipRow = Vec<(usize, String)>;

/// Chip texts wrapped into rows of `width` cells, whitespace between chips
/// (49 §6): `(chip index, text)` per chip, full labels always.
pub fn chip_rows(state: &State, width: usize) -> Vec<ChipRow> {
    let mut rows: Vec<ChipRow> = vec![Vec::new()];
    let mut used = 0;
    let mark = |on: bool| if on { "✓" } else { " " };
    let chips = std::iter::once(format!("[{} All]", mark(state.all()))).chain(
        SearchCategory::ALL
            .iter()
            .map(|c| format!("[{} {}]", mark(state.selected[idx(*c)]), label(*c))),
    );
    for (i, text) in chips.enumerate() {
        let w = cells(&text);
        if used > 0 && used + 1 + w > width {
            rows.push(Vec::new());
            used = 0;
        } else if used > 0 {
            used += 1;
        }
        used += w;
        rows.last_mut().expect("one row").push((i, text));
    }
    rows
}

/// The line printer's rows for one results reply (49 §6.2): per group its
/// state, count, coverage and notices, then one line per hit with exact
/// identity, provenance and snippet.
pub fn print_lines(reply: &Reply) -> Vec<String> {
    let mut out = Vec::new();
    match reply {
        Reply::Results { result: Err(e), .. } => out.push(format!("search error: {e}")),
        Reply::Results {
            result: Ok(resp),
            limit,
            ..
        } => {
            for g in &resp.groups {
                let g = Group::from(g.clone(), 0, *limit);
                let state = match g.state {
                    PageState::Ready => "ready",
                    PageState::Indexing => "indexing",
                    PageState::Unavailable => "unavailable",
                    PageState::Error => "error",
                };
                let mut head = format!("## {} — {state}, {} hits", wire(g.category), g.hits.len());
                if g.has_more {
                    head.push_str(", more available");
                }
                head.push_str(match g.coverage {
                    Coverage::Complete => ", complete",
                    Coverage::Partial => ", partial",
                });
                let reasons = g.reasons();
                if !reasons.is_empty() {
                    let _ = write!(head, ": {reasons}");
                }
                out.push(head);
                for h in &g.hits {
                    let (label, snippet, _) = h.snippet();
                    let snippet: String = snippet
                        .chars()
                        .map(|c| if c.is_control() { ' ' } else { c })
                        .collect();
                    out.push(format!(
                        "{}  {}  {}  {label}{snippet}",
                        h.identity(),
                        h.title(),
                        h.provenance()
                    ));
                }
            }
        }
        Reply::Recent(Ok(list)) => {
            out.push("## recent sessions (not matches)".into());
            out.extend(
                list.iter()
                    .filter(|s| s.parent.is_none())
                    .take(RECENT)
                    .map(|s| format!("{}  {}  {}", s.id, s.model, s.title)),
            );
        }
        Reply::Recent(Err(e)) => out.push(format!("recent sessions: {e}")),
        Reply::Refreshed(Ok(())) => out.push("search caches refreshed".into()),
        Reply::Refreshed(Err(e)) => out.push(format!("refresh failed: {e}")),
        Reply::Artifact(_)
        | Reply::Concept(_)
        | Reply::Historical(_)
        | Reply::Revision(_)
        | Reply::Page { .. } => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::search::TextMatch;

    fn session_hit(title: &str, seq: Option<i64>) -> SearchHit {
        SearchHit {
            session: SessionId::new(),
            title: title.into(),
            kind: if seq.is_some() {
                HitKind::Message
            } else {
                HitKind::Title
            },
            seq,
            role: seq.map(|_| HitRole::User),
            snippet: format!("…{title} plan…"),
            ranges: vec![MatchRange { start: 1, end: 3 }],
            score: 10,
            matches: 1,
            created_at: "2026-09-18T09:00:00Z".into(),
            updated_at: "2026-09-18T10:00:00Z".into(),
            context_removed: false,
            archived: false,
        }
    }

    fn ready(hits: Vec<SearchHit>, has_more: bool) -> SearchResponse {
        let mut page = SearchPage::ready(hits, 50);
        page.has_more = has_more;
        SearchResponse {
            groups: vec![SearchGroup::Sessions { page }],
        }
    }

    fn results(req: &Request, result: Result<SearchResponse, String>) -> (u64, Reply) {
        let RequestKind::Results {
            categories, limit, ..
        } = &req.kind
        else {
            panic!("not a results request");
        };
        (
            req.generation,
            Reply::Results {
                categories: categories.clone(),
                limit: *limit,
                result,
            },
        )
    }

    fn typed(s: &mut State, text: &str, now: u64) -> Vec<Request> {
        s.edit(text.into(), now);
        assert!(s.tick(now + DEBOUNCE_MS - 1).is_empty(), "not due yet");
        s.tick(now + DEBOUNCE_MS)
    }

    #[test]
    fn edits_debounce_and_a_reply_to_the_old_generation_is_inert() {
        let mut s = State::default();
        s.open("");
        let old = typed(&mut s, "dep", 1_000);
        assert_eq!(old.len(), 1, "all four chips share limit 10: one request");
        // A quicker edit supersedes it before it answers.
        let new = typed(&mut s, "deploy", 1_200);
        assert!(s.pending());
        let (g, reply) = results(&old[0], Ok(ready(vec![session_hit("stale", None)], false)));
        assert!(matches!(s.reply(g, reply, 1_400), Effect::None));
        assert!(s.groups.is_empty(), "old generation dropped");
        let (g, reply) = results(&new[0], Ok(ready(vec![session_hit("fresh", None)], false)));
        s.reply(g, reply, 1_400);
        assert_eq!(s.groups.len(), 1);
        assert!(!s.pending());
        assert!(matches!(s.rows.get(s.cursor), Some(Row::Hit(0, 0))));
    }

    #[test]
    fn enter_during_debounce_flushes_and_never_opens_the_previous_row() {
        let mut s = State::default();
        s.open("");
        let first = typed(&mut s, "alpha", 0);
        let (g, reply) = results(
            &first[0],
            Ok(ready(vec![session_hit("alpha", None)], false)),
        );
        s.reply(g, reply, 10);
        s.edit("beta".into(), 20);
        match s.enter() {
            Act::Send(reqs) => assert!(
                matches!(&reqs[0].kind, RequestKind::Results { query, .. } if query == "beta")
            ),
            other => panic!("enter during debounce must flush, got {other:?}"),
        }
        // Still in flight: the stale row does not open.
        assert!(matches!(s.enter(), Act::None));
    }

    #[test]
    fn escape_makes_an_in_flight_answer_and_a_pending_open_inert() {
        let mut s = State::default();
        let reqs = s.open("q");
        assert_eq!(reqs.len(), 1, "a prefilled query runs at once");
        assert!(s.tick(0).is_empty(), "nothing else is due");
        let (g, reply) = results(&reqs[0], Ok(ready(vec![session_hit("a", Some(4))], false)));
        s.reply(g, reply, 200);
        let Act::Send(open) = s.enter() else {
            panic!("message hit asks for its page");
        };
        assert!(matches!(open[0].kind, RequestKind::Page { seq: 4, .. }));
        s.close();
        let eff = s.reply(
            open[0].generation,
            Reply::Page {
                session: SessionId::new(),
                seq: 4,
                result: Ok(Loaded::default()),
            },
            400,
        );
        assert!(
            matches!(eff, Effect::None),
            "a page answering a closed view never opens"
        );
        let (g, reply) = results(&reqs[0], Err("late".into()));
        s.reply(g, reply, 500);
        assert!(
            s.groups[0].state == PageState::Ready,
            "late error dropped too"
        );
    }

    #[test]
    fn all_is_idempotent_and_empty_selection_is_an_explicit_idle_state() {
        let mut s = State::default();
        s.open("");
        typed(&mut s, "x", 0);
        assert!(s.all());
        assert!(s.toggle(0).is_empty(), "All while all on is a no-op");
        for chip in 1..=N {
            s.toggle(chip);
        }
        assert_eq!(s.categories(), Vec::<SearchCategory>::new());
        assert!(
            s.rows
                .iter()
                .any(|r| matches!(r, Row::Note(t) if t.starts_with("Select at least one")))
        );
        let reqs = s.toggle(0);
        assert!(s.all());
        assert_eq!(
            reqs.len(),
            1,
            "All restores a usable selection and asks at once"
        );
        s.toggle(2);
        assert_eq!(
            s.categories(),
            vec![
                SearchCategory::Sessions,
                SearchCategory::Knowledge,
                SearchCategory::KnowledgeHistory,
                SearchCategory::Notes,
                SearchCategory::Meetings,
                SearchCategory::Sources,
                SearchCategory::Notebooks,
            ]
        );
    }

    #[test]
    fn show_more_grows_one_group_and_a_stale_limit_reply_is_dropped() {
        let mut s = State::default();
        s.open("");
        let reqs = typed(&mut s, "x", 0);
        let ten: Vec<SearchHit> = (0..10)
            .map(|i| session_hit(&format!("t{i}"), None))
            .collect();
        let (g, reply) = results(&reqs[0], Ok(ready(ten.clone(), true)));
        s.reply(g, reply, 200);
        let more = s
            .rows
            .iter()
            .position(|r| matches!(r, Row::More(0)))
            .unwrap();
        s.cursor = more;
        let Act::Send(grow) = s.enter() else {
            panic!("more asks")
        };
        assert!(matches!(
            &grow[0].kind,
            RequestKind::Results { categories, limit: 30, .. } if categories == &[SearchCategory::Sessions]
        ));
        // The limit-10 answer arriving late must not shrink the group back.
        let (g, reply) = results(&reqs[0], Ok(ready(ten.clone(), true)));
        s.reply(g, reply, 310);
        let mut thirty = ten.clone();
        thirty.extend((10..30).map(|i| session_hit(&format!("t{i}"), None)));
        let (g, reply) = results(&grow[0], Ok(ready(thirty, true)));
        s.reply(g, reply, 320);
        assert_eq!(s.groups[0].hits.len(), 30);
        assert_eq!(s.groups[0].limit, 30);
    }

    #[test]
    fn cursor_keeps_its_identity_across_a_same_query_refresh() {
        let mut s = State::default();
        s.open("");
        let reqs = typed(&mut s, "x", 0);
        let (a, b) = (session_hit("a", None), session_hit("b", None));
        let (g, reply) = results(&reqs[0], Ok(ready(vec![a.clone(), b.clone()], false)));
        s.reply(g, reply, 200);
        s.move_cursor(1);
        assert_eq!(s.key_at(s.cursor), Some(Key::Session(b.session)));
        s.invalidate(SearchCategory::Sessions, true, 300);
        let again = s.tick(300 + DEBOUNCE_MS);
        assert_eq!(again.len(), 1);
        let (g, reply) = results(&again[0], Ok(ready(vec![b.clone(), a.clone()], false)));
        s.reply(g, reply, 500);
        assert_eq!(
            s.key_at(s.cursor),
            Some(Key::Session(b.session)),
            "same row, new position"
        );
        assert!(matches!(s.rows[s.cursor], Row::Hit(0, 0)));
    }

    #[test]
    fn back_to_results_keeps_a_settled_view_activatable_and_reasks_an_unanswered_one() {
        let mut s = State::default();
        let reqs = s.open("q");
        let hit = session_hit("a", None);
        let (g, reply) = results(&reqs[0], Ok(ready(vec![hit.clone()], false)));
        s.reply(g, reply, 200);
        s.close();
        assert!(s.restore().is_empty(), "nothing changed: nothing re-asked");
        assert!(!s.pending());
        assert!(matches!(s.enter(), Act::Resume(id) if id == hit.session));
        // Closed while an answer was still out: restore re-asks.
        s.invalidate(SearchCategory::Sessions, true, 300);
        s.close();
        let again = s.restore();
        assert_eq!(again.len(), 1);
        assert!(matches!(s.enter(), Act::None), "stale until the answer");
        let (g, reply) = results(&again[0], Ok(ready(vec![hit.clone()], false)));
        s.reply(g, reply, 900);
        assert!(matches!(s.enter(), Act::Resume(_)));
    }

    #[test]
    fn indexing_groups_poll_only_themselves_and_errors_carry_identity() {
        let mut s = State::default();
        s.open("");
        let reqs = typed(&mut s, "x", 0);
        let resp = SearchResponse {
            groups: vec![
                SearchGroup::Sessions {
                    page: SearchPage::ready(vec![], 10),
                },
                SearchGroup::Artifacts {
                    page: SearchPage::indexing(),
                },
                SearchGroup::Knowledge {
                    page: SearchPage::unavailable("plugin off"),
                },
                SearchGroup::KnowledgeHistory {
                    page: SearchPage::error("git broke"),
                },
            ],
        };
        let (g, reply) = results(&reqs[0], Ok(resp));
        s.reply(g, reply, 200);
        assert_eq!(s.wake(), Some(200 + POLL_MS));
        assert!(s.tick(200 + POLL_MS - 1).is_empty());
        let poll = s.tick(200 + POLL_MS);
        assert!(matches!(
            &poll[0].kind,
            RequestKind::Results { categories, .. } if categories == &[SearchCategory::Artifacts]
        ));
        assert!(
            s.rows
                .iter()
                .any(|r| matches!(r, Row::Note(t) if t.contains("git broke")))
        );
        assert!(
            s.rows
                .iter()
                .any(|r| matches!(r, Row::Note(t) if t.contains("plugin off")))
        );
        // A failed poll marks only its group.
        let (g, reply) = results(&poll[0], Err("owner died".into()));
        s.reply(g, reply, 800);
        let art = s
            .groups
            .iter()
            .find(|g| g.category == SearchCategory::Artifacts)
            .unwrap();
        assert_eq!(art.state, PageState::Error);
        assert_eq!(s.groups[0].state, PageState::Ready);
        assert_eq!(s.wake(), None, "nothing left indexing");
    }

    #[test]
    fn destinations_are_typed_per_hit() {
        let mut s = State::default();
        s.open("");
        s.requested = "q".into();
        // Title hit resumes; message hit pages at seq + 1's page.
        let title = session_hit("t", None);
        assert!(
            matches!(s.activate(Hit::Session(title.clone())), Act::Resume(id) if id == title.session)
        );
        let Act::Send(r) = s.activate(Hit::Session(session_hit("m", Some(7)))) else {
            panic!()
        };
        assert!(matches!(r[0].kind, RequestKind::Page { seq: 7, .. }));
        // History body hit reads the snapshot; metadata hit opens the diff.
        let hit = HistorySearchHit {
            id: "facts/x".into(),
            title: "X".into(),
            commit: "c".repeat(40),
            parent: Some("p".repeat(40)),
            time: "2026-09-18T10:00:00Z".into(),
            message: "edit".into(),
            status: ChangeStatus::Modified,
            side: HistorySide::Before,
            snapshot_commit: Some("p".repeat(40)),
            r#match: TextMatch {
                field: "body".into(),
                snippet: "old text".into(),
                ranges: vec![],
            },
        };
        let Act::Send(r) = s.activate(Hit::History(hit.clone())) else {
            panic!()
        };
        assert!(
            matches!(&r[0].kind, RequestKind::Historical { commit, .. } if commit == &"p".repeat(40))
        );
        let meta = HistorySearchHit {
            side: HistorySide::Metadata,
            snapshot_commit: None,
            ..hit
        };
        let Act::Send(r) = s.activate(Hit::History(meta)) else {
            panic!()
        };
        assert!(
            matches!(&r[0].kind, RequestKind::Revision { commit, id } if commit == &"c".repeat(40) && id == "facts/x")
        );
        // Artifact content hit asks for the matched text; a name hit does not.
        let art = artifact::Artifact {
            id: "a".into(),
            session: SessionId::new(),
            call_id: "call-1".into(),
            name: "plan.md".into(),
            title: "Plan".into(),
            kind: artifact::Kind::Markdown,
            mime: "text/markdown".into(),
            bytes: 3,
            version: 2,
            path: "/tmp/plan.md".into(),
            source: None,
            preview: None,
            pages: None,
            origin: None,
            created_at: 0,
        };
        let content = ArtifactSearchHit {
            artifact: art.clone(),
            r#match: TextMatch {
                field: "content".into(),
                snippet: String::new(),
                ranges: vec![],
            },
            content_coverage: ContentCoverage::Complete,
        };
        let Act::Send(r) = s.activate(Hit::Artifact(content)) else {
            panic!()
        };
        assert!(
            matches!(&r[0].kind, RequestKind::Artifact { call_id, query: Some(q) } if call_id == "call-1" && q == "q")
        );
        let name = ArtifactSearchHit {
            artifact: art,
            r#match: TextMatch {
                field: "name".into(),
                snippet: String::new(),
                ranges: vec![],
            },
            content_coverage: ContentCoverage::MetadataOnly,
        };
        let Act::Send(r) = s.activate(Hit::Artifact(name)) else {
            panic!()
        };
        assert!(matches!(
            &r[0].kind,
            RequestKind::Artifact { query: None, .. }
        ));
    }

    #[test]
    fn wide_characters_count_cells_and_chips_wrap_whole() {
        assert_eq!(cells("日本語"), 6);
        assert_eq!(fit("日本語テキスト", 7), "日本語…");
        assert_eq!(fit("abc", 3), "abc");
        assert_eq!(wrap_cells("日本語 テキスト", 8), vec!["日本語", "テキスト"]);
        assert_eq!(
            wrap_cells("日本語 テキスト", 6),
            vec!["日本語", "テキス", "ト"]
        );
        let r = runs("a日b", &[MatchRange { start: 1, end: 2 }], 10);
        assert_eq!(
            r,
            vec![
                ("a".to_owned(), false),
                ("日".to_owned(), true),
                ("b".to_owned(), false)
            ]
        );
        assert_eq!(
            runs("a日b", &[], 2),
            vec![("a".to_owned(), false), ("…".to_owned(), false)]
        );
        let s = State::default();
        let rows = chip_rows(&s, 30);
        assert!(rows.len() >= 2, "five chips do not fit 30 cells on one row");
        assert!(
            rows.iter()
                .flatten()
                .any(|(_, t)| t.contains("Knowledge history")),
            "full label kept"
        );
        for row in &rows {
            let w: usize = row.iter().map(|(_, t)| cells(t)).sum::<usize>() + row.len() - 1;
            assert!(w <= 30, "row {row:?} is {w} cells");
        }
        assert_eq!(chip_rows(&s, 200).len(), 1);
    }

    #[test]
    fn window_keeps_the_cursor_and_its_snippet_row_in_view() {
        let rows = vec![
            Row::Header("h".into()),
            Row::Hit(0, 0),
            Row::Hit(0, 1),
            Row::Hit(0, 2),
            Row::More(0),
        ];
        assert_eq!(first_row(&rows, 0, 1, 3, false), 0);
        // Cursor on the third hit: header + 3 hits = 7 rows; 4 fit.
        assert_eq!(first_row(&rows, 0, 3, 4, false), 2);
        assert_eq!(first_row(&rows, 0, 3, 4, true), 0);
        assert_eq!(
            first_row(&rows, 4, 1, 4, false),
            1,
            "scroll never hides the cursor above"
        );
    }

    #[test]
    fn locate_finds_the_snippet_line() {
        let text = "# title\n\nfirst line\nthe deploy plan here\nlast";
        assert_eq!(locate(text, "…deploy plan…"), Some(3));
        assert_eq!(locate(text, "…nowhere…"), None);
        assert_eq!(locate(text, "…"), None);
    }
}
