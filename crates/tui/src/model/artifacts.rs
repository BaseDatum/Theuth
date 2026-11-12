use super::*;

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
        let Some(e) = self.entries.iter_mut().rev().find(|e| {
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

/// [`artifact_body`] over bytes already read (the search preview reads them
/// off the UI thread); `Err` is why the file could not be read.
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
