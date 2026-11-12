use super::*;

impl Model {

    /// `/knowledge [query | history [id] | show <commit> [id] | restore <id> [commit] | harvest]`.
    pub(super) fn knowledge_cmd(&mut self, arg: &str, tx: &UnboundedSender<Command>) {
        let (sub, rest) = arg
            .split_once(' ')
            .map_or((arg, ""), |(s, r)| (s, r.trim()));
        match (sub, rest) {
            ("", _) => send(tx, Command::KnowledgeList),
            ("history", id) => {
                self.open_history((!id.is_empty()).then(|| id.to_owned()), HISTORY_PAGE, tx);
            }
            ("show", "") => self
                .entries
                .push(Entry::Error("usage: /knowledge show <commit> [id]".into())),
            ("show", rest) => {
                let (commit, id) = rest
                    .split_once(' ')
                    .map_or((rest, None), |(c, i)| (c, Some(i.trim().to_owned())));
                send(
                    tx,
                    Command::KnowledgeRevision {
                        commit: commit.to_owned(),
                        id,
                        card: true,
                    },
                );
            }
            ("restore", "") => self.entries.push(Entry::Error(
                "usage: /knowledge restore <id> [commit]".into(),
            )),
            ("restore", rest) => {
                let (id, commit) = rest
                    .split_once(' ')
                    .map_or((rest, None), |(i, c)| (i, Some(c.trim().to_owned())));
                send(
                    tx,
                    Command::KnowledgeRestore {
                        id: id.to_owned(),
                        commit,
                    },
                );
            }
            ("harvest", _) => match self.session {
                Some(session) => send(tx, Command::Harvest(session)),
                None => self.entries.push(Entry::Info("no session".into())),
            },
            _ => send(tx, Command::KnowledgeSearch(arg.to_owned())),
        }
    }
}

impl Model {

    /// `/knowledge`: `⏎` / `h` opens the highlighted concept's history, `H`
    /// the whole bundle's (deleted concepts included).
    pub(super) fn knowledge_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        let Some(Menu::Knowledge { items, cursor }) = &mut self.menu else {
            return MenuKey::Consumed;
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
        MenuKey::Consumed
    }

    /// Ask for `limit` commits of `id`'s history and show the menu waiting
    /// for them; the same scope keeps its rows and highlight while the next
    /// page loads. Only the reply for this `id` / `limit` fills it.
    fn open_history(&mut self, id: Option<String>, limit: usize, tx: &UnboundedSender<Command>) {
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
        send(tx, Command::KnowledgeHistory { id, limit });
    }

    /// `/knowledge history`: `⏎` on a commit fetches its diff (`↑↓` / page
    /// keys scroll rows, `←→` columns; esc / `h` / `⌫` go back), `⏎` on the
    /// `older` row asks for the next page, `r` re-asks for the current one.
    /// Moving drops a pending fetch. Esc on the list closes.
    pub(super) fn knowledge_history_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
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
            return MenuKey::Consumed;
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
            return MenuKey::Consumed;
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
                        Command::KnowledgeRevision {
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
        MenuKey::Consumed
    }

    /// The `/knowledge history` menu: `(title, rows, cursor)`. The commit
    /// list (plus an `older` row when a page remains; `loading…`, the error
    /// or `no history` in its place), or the highlighted commit's diff with
    /// the cursor as its first shown row and `col` leading chars dropped.
    pub fn knowledge_history_menu(&self) -> (String, Vec<String>, usize) {
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
