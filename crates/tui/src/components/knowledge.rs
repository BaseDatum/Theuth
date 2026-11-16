//! Knowledge browsing, revision history and detached session harvesting.
use super::*;
use ::knowledge::{Change, ChangeStatus, HarvestResult, History, Meta, Revision};
use iocraft::KeyCode;
use rpc_compose::api::clients::{AgentConceptsApiClient, KnowledgeConceptsApiClient};
use runtime::SessionId;

#[derive(Default)]
pub(crate) struct Knowledge {
    pub(crate) menu: Option<Menu>,
    pub(crate) harvesting: bool,
}
#[expect(
    clippy::large_enum_variant,
    reason = "The single held menu already lives in its boxed owner."
)]
pub(crate) enum Menu {
    /// `/knowledge [query]` picker (10 §6).
    Knowledge {
        /// The list or search reply.
        items: Vec<Meta>,
        /// Highlighted row.
        cursor: usize,
    },
    /// `/knowledge history [id]`: commits newest first, then one commit's
    /// diff against its parent. Opened when asked, filled by the reply that
    /// matches `id` and `limit`; later or stale replies are dropped.
    KnowledgeHistory {
        /// Concept id, `None` for the whole bundle.
        id: Option<String>,
        /// Shown commits, at most `limit`.
        changes: Vec<Change>,
        /// Page size asked for; `⏎` on the `older` row asks for 20 more.
        limit: usize,
        /// Commits exist past `changes`: the `older` row shows.
        more: bool,
        /// Highlighted commit (or the `older` row at `changes.len()`).
        cursor: usize,
        /// A page request is in flight.
        loading: bool,
        /// Commit whose revision is being fetched; cleared by moving.
        pending: Option<String>,
        /// The last failed request's message, shown in place of the rows.
        error: Option<String>,
        /// Open diff of the highlighted commit: first shown row, first shown column.
        revision: Option<(Revision, usize, usize)>,
    },
}
#[derive(Clone, Debug)]
pub(crate) enum Request {
    KnowledgeList,
    KnowledgeSearch(String),
    KnowledgeHistory {
        id: Option<String>,
        limit: usize,
    },
    KnowledgeRevision {
        commit: String,
        id: Option<String>,
        card: bool,
    },
    KnowledgeRestore {
        id: String,
        commit: Option<String>,
    },
    Harvest(SessionId),
}
pub(crate) enum Msg {
    Knowledge(Result<Vec<Meta>, String>),
    KnowledgeHistory {
        id: Option<String>,
        limit: usize,
        history: Result<History, String>,
    },
    KnowledgeRevision {
        id: Option<String>,
        card: bool,
        revision: Result<Revision, String>,
    },
    KnowledgeRestored(Result<Change, String>),
    Harvested(Result<HarvestResult, String>),
    HarvestingSession(SessionId),
    HarvestedEvent {
        session: SessionId,
        added: u32,
        updated: u32,
    },
}
fn send(cx: &mut Cx<'_, '_, Msg>, request: Request) {
    cx.spawn_request(Lane::Concurrent, request, |api, request| async move {
        match request {
            Request::KnowledgeList => Msg::Knowledge(
                api.knowledge_list(None, None, None, Some(50), None)
                    .await
                    .map_err(crate::reason),
            ),
            Request::KnowledgeSearch(query) => Msg::Knowledge(
                api.knowledge_search(query, None, None)
                    .await
                    .map(|hits| hits.into_iter().map(|hit| hit.meta).collect())
                    .map_err(crate::reason),
            ),
            Request::KnowledgeHistory { id, limit } => {
                let fetch = u32::try_from(limit + 1).unwrap_or(u32::MAX);
                let history = api
                    .knowledge_history(id.clone(), Some(fetch))
                    .await
                    .map_err(crate::reason);
                Msg::KnowledgeHistory { id, limit, history }
            }
            Request::KnowledgeRevision { commit, id, card } => {
                let revision = api
                    .knowledge_revision(commit, id.clone())
                    .await
                    .map_err(crate::reason);
                Msg::KnowledgeRevision { id, card, revision }
            }
            Request::KnowledgeRestore { id, commit } => Msg::KnowledgeRestored(
                api.knowledge_restore(id, commit)
                    .await
                    .map_err(crate::reason),
            ),
            Request::Harvest(session) => {
                Msg::Harvested(api.knowledge_harvest(session).await.map_err(crate::reason))
            }
        }
    });
}

impl Knowledge {
    /// `/knowledge [query | history [id] | show <commit> [id] | restore <id> [commit] | harvest]`.
    fn knowledge_cmd(&mut self, arg: &str, tx: &mut Cx<'_, '_, Msg>) {
        let (sub, rest) = arg
            .split_once(' ')
            .map_or((arg, ""), |(s, r)| (s, r.trim()));
        match (sub, rest) {
            ("", _) => send(tx, Request::KnowledgeList),
            ("history", id) => {
                self.open_history((!id.is_empty()).then(|| id.to_owned()), HISTORY_PAGE, tx);
            }
            ("show", "") => tx.core(CoreAction::Reject(
                "usage: /knowledge show <commit> [id]".into(),
            )),
            ("show", rest) => {
                let (commit, id) = rest
                    .split_once(' ')
                    .map_or((rest, None), |(c, i)| (c, Some(i.trim().to_owned())));
                send(
                    tx,
                    Request::KnowledgeRevision {
                        commit: commit.to_owned(),
                        id,
                        card: true,
                    },
                );
            }
            ("restore", "") => tx.core(CoreAction::Reject(
                "usage: /knowledge restore <id> [commit]".into(),
            )),
            ("restore", rest) => {
                let (id, commit) = rest
                    .split_once(' ')
                    .map_or((rest, None), |(i, c)| (i, Some(c.trim().to_owned())));
                send(
                    tx,
                    Request::KnowledgeRestore {
                        id: id.to_owned(),
                        commit,
                    },
                );
            }
            ("harvest", _) => match tx.core.session {
                Some(session) => send(tx, Request::Harvest(session)),
                None => tx.core(CoreAction::Notice("no session".into())),
            },
            _ => send(tx, Request::KnowledgeSearch(arg.to_owned())),
        }
    }
}

impl Knowledge {
    /// `/knowledge`: `⏎` / `h` opens the highlighted concept's history, `H`
    /// the whole bundle's (deleted concepts included).
    fn knowledge_key(&mut self, code: KeyCode, tx: &mut Cx<'_, '_, Msg>) {
        let Some(Menu::Knowledge { items, cursor }) = &mut self.menu else {
            return;
        };
        match code {
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Down => *cursor = (*cursor + 1).min(items.len().saturating_sub(1)),
            KeyCode::Esc => self.menu = None,
            KeyCode::Enter | KeyCode::Char('h') => {
                if let Some(id) = items.get(*cursor).map(|m| m.id.clone()) {
                    self.open_history(Some(id), HISTORY_PAGE, tx);
                }
            }
            KeyCode::Char('H') => self.open_history(None, HISTORY_PAGE, tx),
            _ => {}
        }
    }

    /// Ask for `limit` commits of `id`'s history and show the menu waiting
    /// for them; the same scope keeps its rows and highlight while the next
    /// page loads. Only the reply for this `id` / `limit` fills it.
    fn open_history(&mut self, id: Option<String>, limit: usize, tx: &mut Cx<'_, '_, Msg>) {
        match &mut self.menu {
            Some(Menu::KnowledgeHistory {
                id: open,
                limit: page,
                loading,
                pending,
                error,
                revision,
                ..
            }) if *open == id => {
                *page = limit;
                *loading = true;
                *pending = None;
                *error = None;
                *revision = None;
            }
            _ => {
                self.menu = Some(Menu::KnowledgeHistory {
                    id: id.clone(),
                    changes: Vec::new(),
                    limit,
                    more: false,
                    cursor: 0,
                    loading: true,
                    pending: None,
                    error: None,
                    revision: None,
                });
            }
        }
        send(tx, Request::KnowledgeHistory { id, limit });
    }

    /// `/knowledge history`: `⏎` on a commit fetches its diff (`↑↓` / page
    /// keys scroll rows, `←→` columns; esc / `h` / `⌫` go back), `⏎` on the
    /// `older` row asks for the next page, `r` re-asks for the current one.
    /// Moving drops a pending fetch. Esc on the list closes.
    fn knowledge_history_key(&mut self, code: KeyCode, tx: &mut Cx<'_, '_, Msg>) {
        let Some(Menu::KnowledgeHistory {
            id,
            changes,
            limit,
            more,
            cursor,
            pending,
            revision,
            ..
        }) = &mut self.menu
        else {
            return;
        };
        if let Some((rev, row, col)) = revision {
            // One header row per file plus its diff lines; `revision_rows` shape.
            let last = rev
                .files
                .iter()
                .map(|f| 1 + f.diff.lines().count())
                .sum::<usize>()
                .saturating_sub(1);
            match code {
                KeyCode::Up => *row = row.saturating_sub(1),
                KeyCode::Down => *row = (*row + 1).min(last),
                KeyCode::PageUp => *row = row.saturating_sub(HISTORY_PAGE),
                KeyCode::PageDown => *row = (*row + HISTORY_PAGE).min(last),
                KeyCode::Left => *col = col.saturating_sub(DIFF_STEP),
                KeyCode::Right => *col += DIFF_STEP,
                KeyCode::Esc | KeyCode::Char('h') | KeyCode::Backspace => *revision = None,
                _ => {}
            }
            return;
        }
        let rows = changes.len() + usize::from(*more);
        match code {
            KeyCode::Up => {
                *cursor = cursor.saturating_sub(1);
                *pending = None;
            }
            KeyCode::Down => {
                *cursor = (*cursor + 1).min(rows.saturating_sub(1));
                *pending = None;
            }
            KeyCode::Esc => self.menu = None,
            KeyCode::Char('r') => {
                let (id, limit) = (id.clone(), *limit);
                self.open_history(id, limit, tx);
            }
            KeyCode::Enter => match changes.get(*cursor) {
                Some(c) => {
                    *pending = Some(c.commit.clone());
                    send(
                        tx,
                        Request::KnowledgeRevision {
                            commit: c.commit.clone(),
                            id: id.clone(),
                            card: false,
                        },
                    );
                }
                None if *more => {
                    let (id, limit) = (id.clone(), *limit + HISTORY_PAGE);
                    self.open_history(id, limit, tx);
                }
                None => {}
            },
            _ => {}
        }
    }

    /// The `/knowledge history` menu: `(title, rows, cursor)`. The commit
    /// list (plus an `older` row when a page remains; `loading…`, the error
    /// or `no history` in its place), or the highlighted commit's diff with
    /// the cursor as its first shown row and `col` leading chars dropped.
    pub(crate) fn knowledge_history_menu(&self) -> (String, Vec<String>, usize) {
        let Some(Menu::KnowledgeHistory {
            id,
            changes,
            more,
            cursor,
            loading,
            pending,
            error,
            revision,
            ..
        }) = &self.menu
        else {
            return (String::new(), Vec::new(), 0);
        };
        let scope = id.as_deref().unwrap_or("all concepts");
        if let Some((rev, row, col)) = revision {
            let parent = rev
                .parent
                .as_deref()
                .map_or("none (root)", |p| p.get(..7).unwrap_or(p));
            let title = format!(
                "knowledge · {scope} · {} · {} · {} · parent {parent} · {} file{}   ↑↓←→ scroll  esc back",
                rev.change.commit,
                rev.change.time,
                rev.change.message,
                rev.files.len(),
                if rev.files.len() == 1 { "" } else { "s" },
            );
            let rows = revision_rows(rev)
                .into_iter()
                .map(|r| {
                    if *col == 0 {
                        r
                    } else {
                        r.chars().skip(*col).collect()
                    }
                })
                .collect();
            return (title, rows, *row);
        }
        let state = match (*loading, pending.as_deref(), error.as_deref()) {
            (_, _, Some(e)) => format!("   error: {e}"),
            (true, _, None) => "   loading…".to_owned(),
            (false, Some(c), None) => format!("   loading {c}…"),
            (false, None, None) => String::new(),
        };
        let title = format!("knowledge history · {scope}   ⏎ diff  r refresh  esc close{state}");
        let mut rows: Vec<String> = changes
            .iter()
            .map(|c| format!("{}  {}  {}", c.commit, c.time, c.message))
            .collect();
        if *more {
            rows.push(format!("… older  (⏎ loads {HISTORY_PAGE} more)"));
        }
        if rows.is_empty() {
            rows.push(match (*loading, error) {
                (_, Some(_)) => "request failed · r retries".into(),
                (true, None) => "loading…".into(),
                (false, None) => "no history · r refreshes".into(),
            });
        }
        (title, rows, *cursor)
    }
}

/// One `/knowledge` row: `` `id` — title [type]: description `` (10 §6).
pub fn meta_line(m: &Meta) -> String {
    format!("`{}` — {} [{}]: {}", m.id, m.title, m.type_, m.description)
}

/// `/knowledge history` rows per page; `older` asks for this many more.
pub const HISTORY_PAGE: usize = 20;
/// Chars a diff scrolls sideways per `←` / `→`.
pub(super) const DIFF_STEP: usize = 8;

/// A revision as diff rows: `+ id` / `~ id` / `- id` per concept file, each
/// followed by its unified diff. Empty commits (only generated files
/// changed) say so.
pub fn revision_rows(r: &Revision) -> Vec<String> {
    if r.files.is_empty() {
        return vec!["no concept files changed".into()];
    }
    let mut rows = Vec::new();
    for f in &r.files {
        let glyph = match f.status {
            ChangeStatus::Added => "+",
            ChangeStatus::Modified => "~",
            ChangeStatus::Deleted => "-",
        };
        rows.push(format!("{glyph} {}  ({})", f.id, status_name(f.status)));
        rows.extend(f.diff.lines().map(str::to_owned));
    }
    rows
}

/// `added` / `modified` / `deleted`.
fn status_name(s: ChangeStatus) -> &'static str {
    match s {
        ChangeStatus::Added => "added",
        ChangeStatus::Modified => "modified",
        ChangeStatus::Deleted => "deleted",
    }
}

impl Component for Knowledge {
    type Msg = Msg;
    fn declaration(&self) -> Declaration {
        Declaration {
            id: "knowledge",
            interests: &[Interest {
                topic: "agent.events",
                version: 1,
                variants: &["Harvesting", "Harvested"],
            }],
            slash: &[(
                "/knowledge",
                "[query | history [id] | show <commit> [id] | restore <id> [commit] | harvest]",
                "knowledge base: list, search, history, show a commit, restore, harvest",
            )],
            bindings: &[],
        }
    }
    fn decode(&self, event: &TopicEvent, core: &CoreView<'_>) -> Option<Msg> {
        if event.topic().is_some()
            && (event.topic() != Some("agent.events") || event.version() != Some(1))
        {
            return None;
        }
        match core.agent? {
            agent::AgentEvent::Harvesting { session } => Some(Msg::HarvestingSession(*session)),
            agent::AgentEvent::Harvested {
                session,
                added,
                updated,
            } => Some(Msg::HarvestedEvent {
                session: *session,
                added: *added,
                updated: *updated,
            }),
            _ => None,
        }
    }
    fn update(&mut self, input: Input<'_, Msg>, cx: &mut Cx<'_, '_, Msg>) {
        if cx.core.reasoning
            && matches!(
                &input,
                Input::Message(
                    Msg::Knowledge(Ok(_))
                        | Msg::KnowledgeHistory { history: Ok(_), .. }
                        | Msg::KnowledgeRevision {
                            revision: Ok(_),
                            ..
                        }
                        | Msg::KnowledgeRestored(Ok(_))
                        | Msg::Harvested(Ok(_))
                        | Msg::HarvestingSession(_)
                        | Msg::HarvestedEvent { .. }
                )
            )
        {
            cx.plain(Plain::Text("\x1b[0m\n".into()));
        }
        match input {
            Input::Slash { args, .. } => {
                self.knowledge_cmd(args, cx);
                if self.menu.is_some() {
                    cx.focus(FocusClass::Modal);
                }
            }
            Input::Key(key) if self.menu.is_some() => {
                cx.consume_key();
                match self.menu {
                    Some(Menu::Knowledge { .. }) => self.knowledge_key(key.code, cx),
                    _ => self.knowledge_history_key(key.code, cx),
                }
                if self.menu.is_none() {
                    cx.close();
                }
            }
            Input::Message(Msg::Knowledge(Ok(items))) => {
                for item in &items {
                    cx.plain(Plain::Line(meta_line(item)));
                }
                self.menu = Some(Menu::Knowledge { items, cursor: 0 });
                cx.focus(FocusClass::Modal);
            }
            Input::Message(Msg::KnowledgeHistory {
                id,
                limit,
                history: Ok(history),
            }) => {
                for change in &history.changes {
                    cx.plain(Plain::Line(format!(
                        "{}  {}  {}",
                        change.commit, change.time, change.message
                    )));
                }
                if let Some(diff) = &history.diff {
                    cx.plain(Plain::Line(diff.clone()));
                }
                if let Some(Menu::KnowledgeHistory {
                    id: open,
                    changes: shown,
                    limit: page,
                    more,
                    loading,
                    error,
                    ..
                }) = &mut self.menu
                    && *open == id
                    && *page == limit
                {
                    let mut changes = history.changes;
                    *more = changes.len() > limit;
                    changes.truncate(limit);
                    *shown = changes;
                    *loading = false;
                    *error = None;
                }
            }
            Input::Message(Msg::KnowledgeRevision {
                id,
                card,
                revision: Ok(revision),
            }) => {
                cx.plain(Plain::Line(format!(
                    "{}  {}  {}  parent {}\n{}",
                    revision.change.commit,
                    revision.change.time,
                    revision.change.message,
                    revision.parent.as_deref().unwrap_or("none"),
                    revision_rows(&revision).join("\n")
                )));
                if card {
                    let title = format!(
                        "knowledge {}  {}{}",
                        revision.change.commit,
                        revision.change.message,
                        id.map_or(String::new(), |id| format!("  ({id})"))
                    );
                    cx.core(CoreAction::AppendCard(TranscriptCard {
                        entry_key: None,
                        group: "knowledge",
                        key: uuid::Uuid::new_v4(),
                        glyph: "◇",
                        color: crate::ui::OCHRE,
                        title,
                        body: markdown::Text::complete(&format!(
                            "```diff\n{}\n```",
                            revision_rows(&revision).join("\n")
                        )),
                        expanded: false,
                        plain: false,
                    }));
                } else if let Some(Menu::KnowledgeHistory {
                    id: open,
                    pending,
                    revision: shown,
                    ..
                }) = &mut self.menu
                    && *open == id
                    && pending.as_deref() == Some(revision.change.commit.as_str())
                {
                    *pending = None;
                    *shown = Some((revision, 0, 0));
                }
            }
            Input::Message(Msg::KnowledgeRestored(Ok(change))) => {
                let text = format!("{}  {}", change.commit, change.message);
                cx.plain(Plain::Line(text.clone()));
                cx.core(CoreAction::Notice(text));
            }
            Input::Message(Msg::Harvested(Ok(result))) => {
                let text = format!("knowledge +{} ~{}", result.added, result.updated);
                cx.plain(Plain::Line(text.clone()));
                cx.core(CoreAction::Notice(text));
            }
            Input::Message(
                Msg::Knowledge(Err(error))
                | Msg::KnowledgeHistory {
                    history: Err(error),
                    ..
                }
                | Msg::KnowledgeRevision {
                    revision: Err(error),
                    ..
                }
                | Msg::KnowledgeRestored(Err(error))
                | Msg::Harvested(Err(error)),
            ) => cx.error(error),
            Input::Message(Msg::HarvestingSession(session)) => {
                cx.claim_plain();
                if cx.core.session == Some(session) && !cx.core.inspecting {
                    self.harvesting = true;
                }
            }
            Input::Message(Msg::HarvestedEvent {
                session,
                added,
                updated,
            }) => {
                cx.claim_plain();
                let text = format!("knowledge +{added} ~{updated}");
                cx.plain(Plain::Line(text.clone()));
                if cx.core.session == Some(session) && !cx.core.inspecting {
                    self.harvesting = false;
                    cx.interject(text);
                }
            }
            Input::Lifecycle(Lifecycle::CommandFailed(message)) => {
                if let Some(Menu::KnowledgeHistory {
                    loading,
                    pending,
                    error,
                    ..
                }) = &mut self.menu
                    && (*loading || pending.is_some())
                {
                    *loading = false;
                    *pending = None;
                    *error = Some(message.into());
                    cx.claim_error();
                }
            }
            Input::Lifecycle(Lifecycle::FocusLost) => self.menu = None,
            Input::Lifecycle(Lifecycle::SessionReset | Lifecycle::EngineChanged) => {
                self.menu = None;
                self.harvesting = false;
                cx.close();
            }
            _ => {}
        }
    }
    fn view(&self, at: Surface<'_>, cx: &ViewCx<'_>) -> Option<View> {
        match at {
            Surface::Activity if self.harvesting => Some(View::Text("harvesting…".into())),
            Surface::Modal => {
                let (title, rows, cursor) = match self.menu.as_ref()? {
                    Menu::Knowledge { items, cursor } => (
                        "knowledge  ⏎/h history of concept  H history of all  esc close".into(),
                        if items.is_empty() {
                            vec![
                                "no concepts · H shows the history of all (deleted ones too)"
                                    .into(),
                            ]
                        } else {
                            items.iter().map(meta_line).collect()
                        },
                        *cursor,
                    ),
                    Menu::KnowledgeHistory { .. } => self.knowledge_history_menu(),
                };
                let tall = matches!(
                    self.menu,
                    Some(Menu::KnowledgeHistory {
                        revision: Some(_),
                        ..
                    })
                );
                let height = cx.height as u16;
                let count = rows.len().min(if tall {
                    usize::from(height.saturating_sub(10)).max(10)
                } else {
                    10
                }) + 3;
                Some(View::IntrinsicElement {
                    rows: count,
                    replace: false,
                    element: crate::ui::menu_view(
                        title,
                        rows,
                        cursor,
                        cx.width as u16,
                        height,
                        tall,
                    ),
                })
            }
            _ => None,
        }
    }
    fn animating(&self) -> bool {
        self.harvesting
    }
}
