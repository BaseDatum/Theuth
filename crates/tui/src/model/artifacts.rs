use super::*;
use crate::components::artifacts_apps::{artifact_text, artifact_read_path, bytes_label};

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

impl Model {

    /// Add an entry that did not come from the model's stream (a peer message,
    /// a spawn notice). It goes above a still-streaming tail so the text keeps
    /// accumulating in one `markdown::Text`: pushing it last would split the
    /// response mid-block and render both halves as broken markdown.
    pub(super) fn interject(&mut self, e: Entry) {
        let streaming = match self.entries.last() {
            Some(Entry::Assistant(t)) => !t.is_done(),
            Some(Entry::Reasoning { done, .. }) => !done,
            _ => false,
        };
        let at = self.entries.len() - usize::from(streaming);
        self.entries.insert(at, e);
    }

    /// Expand or collapse the most recent card or finished tool call (Ctrl+O).
    /// An artifact card expands to its file rendered by kind (35 §6.1).
    pub fn toggle_card(&mut self) {
        let opts = self.assistant_options();
        let Some(index) = self.entries.iter().rposition(|e| {
            matches!(
                e,
                Entry::Card { .. }
                    | Entry::Tool {
                        result: Some(_),
                        ..
                    }
                    | Entry::Compacted {
                        summary: Some(_),
                        ..
                    }
            ) || matches!(e, Entry::Advisory { advisory, .. } if advisory_row_count(advisory) > ADVISORY_COLLAPSED)
        }) else {
            return;
        };
        if self.component_entry_action(index, crate::components::EntryOperation::Toggle) { return; }
        let e = &mut self.entries[index];
        match e {
            Entry::Card { expanded, .. }
            | Entry::Compacted { expanded, .. }
            | Entry::Advisory { expanded, .. } => {
                *expanded = !*expanded;
            }
            Entry::Tool {
                result: Some((_, false)),
                artifact: Some(a),
                expanded,
                ..
            } => {
                *expanded = match expanded.take() {
                    Some(_) => None,
                    None => Some(Box::new(artifact_body(a, &opts))),
                }
            }
            Entry::Tool {
                result: Some((output, _)),
                expanded,
                ..
            } => {
                *expanded = match expanded.take() {
                    Some(_) => None,
                    None => Some(Box::new(markdown::Text::complete(&output_markdown(output)))),
                }
            }
            _ => {}
        }
    }

    /// The path `o` opens: the newest artifact card of the current exchange
    /// (nothing later than the user's last message), if any.
    pub fn artifact_path(&self) -> Option<&std::path::Path> {
        self.entries
            .iter()
            .rev()
            .take_while(|e| !matches!(e, Entry::User { .. }))
            .find_map(|e| match e {
                Entry::Tool {
                    artifact: Some(a), ..
                } => Some(a.path.as_path()),
                _ => None,
            })
    }

    /// Join an artifact to its `show` card by `call_id` (35 §0.3); a card
    /// not on this transcript (another session's, or scrolled out of
    /// history) is left alone.
    pub(super) fn attach_artifact(&mut self, a: artifact::Artifact) {
        if let Some(Entry::Tool { artifact, .. }) = self
            .entries
            .iter_mut()
            .rev()
            .find(|e| matches!(e, Entry::Tool { call_id, .. } if *call_id == a.call_id))
        {
            *artifact = Some(Box::new(a));
        }
    }
}

impl Model {

    /// `/artifacts` and `/apps`: Enter opens the highlighted file with the OS,
    /// `s` shows it inline as a card (an app's read-only source), `e` exports
    /// it to `~/Downloads/<name>` (35 §6.2, 46 §8.2).
    pub(super) fn artifacts_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        let Some(Menu::Artifacts { items, cursor, .. }) = &mut self.menu else {
            return MenuKey::Consumed;
        };
        match code {
            KeyCode::Up => *cursor = grouped_step(items, *cursor, -1),
            KeyCode::Down => *cursor = grouped_step(items, *cursor, 1),
            KeyCode::Esc => self.menu = None,
            KeyCode::Enter => {
                if let Some(a) = items.get(*cursor).and_then(Grouped::item) {
                    send(tx, Command::OpenPath(a.path.clone()));
                }
                self.menu = None;
            }
            KeyCode::Char('e') => {
                if let Some(a) = items.get(*cursor).and_then(Grouped::item) {
                    send(
                        tx,
                        Command::ExportArtifact {
                            id: a.id.clone(),
                            name: a.name.clone(),
                            // 51 §4.2: a private blob is exported for its tree's owner only.
                            session: self
                                .private
                                .as_ref()
                                .filter(|p| p.member(a.session))
                                .map(|_| a.session),
                        },
                    );
                }
                self.menu = None;
            }
            KeyCode::Char('s') => {
                let picked = items.get(*cursor).and_then(Grouped::item).cloned();
                self.menu = None;
                if let Some(a) = picked {
                    let text = artifact_body(&a, &self.assistant_options());
                    self.entries.push(Entry::Card {
                        kind: CardKind::Artifact,
                        title: a.name,
                        text: Box::new(text),
                        expanded: true,
                    });
                }
            }
            _ => {}
        }
        MenuKey::Consumed
    }
}

/// The tool whose card is an artifact card (35 §0.3).
pub(super) const SHOW: &str = "show";
/// The tool whose card is an app card (46 §3.1): same join, source is the file.
pub(super) const APP: &str = "app";
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
        (APP, Some(s)) => match &s.name {
            Some(n) => format!("building {n}… {size}"),
            None => format!("building app… {size}"),
        },
        _ => format!("preparing {tool}… {size}"),
    }
}

/// An artifact's inline body by kind (35 §3.2): markdown rendered with the
/// assistant's options so its own fences draw, text in a fence tagged with the
/// extension, CSV as a table of the first [`CSV_ROWS`] rows, an app's
/// `.openui` source read-only in a fence under where it runs (46 §8.2),
/// anything else one line. Text bodies cap at [`INLINE_LINES`].
fn artifact_body(a: &artifact::Artifact, opts: &markdown::Options) -> markdown::Text {
    let bytes = artifact_read_path(a).map(|p| std::fs::read(p).map_err(|e| e.to_string()));
    artifact_text(
        a,
        bytes
            .as_ref()
            .map_or(Ok(&[][..]), |b| b.as_deref().map_err(String::as_str)),
        opts,
    )
}

