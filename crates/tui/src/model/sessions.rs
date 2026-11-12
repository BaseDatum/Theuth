use super::*;

/// The private root this view owns (51 §4.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Private {
    /// The root.
    pub root: SessionId,
    /// Lease secret presented on leave.
    pub secret: String,
    /// Tree members seen so far (the root, its subtree as listed or spawned):
    /// navigating among them is not a leave.
    pub members: std::collections::HashSet<SessionId>,
}

impl Private {
    /// `id` is in the owned tree as far as this view has seen.
    pub fn member(&self, id: SessionId) -> bool {
        id == self.root || self.members.contains(&id)
    }
}

/// Heading for a project label; labels are display data, control characters dropped.
pub(crate) fn project_heading(project: Option<&str>) -> String {
    match project {
        Some(p) => p.chars().filter(|c| !c.is_control()).collect(),
        None => "Ungrouped".to_owned(),
    }
}

/// Projects in first-seen order over `roots` (the pinned/newest list order), `None` last (51 §3).
pub(crate) fn project_order<'a>(roots: impl Iterator<Item = Option<&'a str>>) -> Vec<Option<&'a str>> {
    let mut order: Vec<Option<&str>> = Vec::new();
    let mut ungrouped = false;
    for p in roots {
        match p {
            None => ungrouped = true,
            Some(p) if !order.contains(&Some(p)) => order.push(Some(p)),
            Some(_) => {}
        }
    }
    if ungrouped {
        order.push(None);
    }
    order
}

/// `/sessions` rows: a heading per project in first-seen order of the roots
/// (Ungrouped last), each root followed by its descendants, depth-first, in
/// list order (51 §3). A roster desk (`desk` set) is not a chat: it and its
/// tree (reached only through it) are left out.
pub fn session_rows(mut list: Vec<SessionSummary>) -> Vec<Grouped<(SessionSummary, usize)>> {
    list.retain(|s| s.desk.is_none());
    let mut children: HashMap<SessionId, Vec<usize>> = HashMap::new();
    for (i, s) in list.iter().enumerate() {
        if let Some(p) = s.parent {
            children.entry(p).or_default().push(i);
        }
    }
    let roots: Vec<usize> = (0..list.len())
        .filter(|&i| list[i].parent.is_none())
        .collect();
    let order = project_order(roots.iter().map(|&i| list[i].project.as_deref()));
    fn walk(
        list: &[SessionSummary],
        children: &HashMap<SessionId, Vec<usize>>,
        i: usize,
        depth: usize,
        out: &mut Vec<Grouped<(SessionSummary, usize)>>,
    ) {
        out.push(Grouped::Item((list[i].clone(), depth)));
        for &c in children.get(&list[i].id).map_or(&[][..], Vec::as_slice) {
            walk(list, children, c, depth + 1, out);
        }
    }
    let mut rows = Vec::with_capacity(list.len() + order.len());
    for project in order {
        rows.push(Grouped::Heading(project_heading(project)));
        for &r in roots
            .iter()
            .filter(|&&r| list[r].project.as_deref() == project)
        {
            walk(&list, &children, r, 0, &mut rows);
        }
    }
    rows
}

/// What a [`Menu::Turns`] pick does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TurnAction {
    /// Put the opener's text in the editor; ⏎ replaces the turn with it.
    Edit,
    /// Regenerate the turn's reply.
    Regen,
    /// New session with the conversation through the turn.
    Fork,
}

/// Three Escs inside this window stop the running subtree as well as the root (133 §5).
pub const ESC_TAPS_WINDOW: std::time::Duration = std::time::Duration::from_secs(1);

impl Model {

    /// Switch to `id`: ask for its history and its subtree (design doc 19
    /// §3.5). `/resume`, `/sessions`, `/agents` and `/parent` all land here.
    /// Pending approvals ride along (an approval `o`pened from stays up); the
    /// subtree reply prunes the ones that do not belong to the new view.
    pub fn resume(&mut self, id: SessionId, tx: &UnboundedSender<Command>) {
        let approvals = std::mem::take(&mut self.approvals);
        let asks = std::mem::take(&mut self.asks);
        // Root→child→parent moves stay inside the owned tree; anything else releases it (51 §4.3).
        if self.private.as_ref().is_some_and(|p| !p.member(id)) {
            self.leave_private(tx);
        }
        self.reset();
        self.approvals = approvals;
        self.asks = asks;
        self.session = Some(id);
        self.component_notify(crate::components::Lifecycle::SessionChanged);
        send(
            tx,
            Command::Load {
                session: id,
                settled_only: false,
            },
        );
        send(tx, Command::ListAgents(id));
    }

    /// Esc: cancel the current session's in-flight turn or `/compact`, if any
    /// (the root only); the third Esc within [`ESC_TAPS_WINDOW`] stops its
    /// running subagents too (133 §5).
    pub fn cancel(&mut self, tx: &UnboundedSender<Command>) {
        self.cancel_at(std::time::Instant::now(), tx);
    }

    /// [`Self::cancel`] with the clock injected.
    pub fn cancel_at(&mut self, now: std::time::Instant, tx: &UnboundedSender<Command>) {
        // 141 §10.1: a watch-only child is the harness's; Esc reaches it only
        // when the harness accepts a per-child cancel (RFD `capabilities.cancel`).
        if let Some(can_cancel) = self.observed.as_ref().map(|o| o.can_cancel) {
            match self.session {
                Some(s) if can_cancel && self.streaming => send(tx, Command::Cancel(s)),
                Some(_) if can_cancel => {}
                _ => self.watch_swallow(),
            }
            return;
        }
        let Some(session) = self.session.filter(|_| self.streaming || self.compacting) else {
            return;
        };
        self.esc_taps
            .retain(|t| now.duration_since(*t) < ESC_TAPS_WINDOW);
        self.esc_taps.push(now);
        if self.esc_taps.len() < 3 {
            send(tx, Command::Cancel(session));
            return;
        }
        self.esc_taps.clear();
        let n = self
            .agents
            .values()
            .filter(|r| r.info.id != session && r.live())
            .count();
        self.entries.push(Entry::Info(format!(
            "stopped {n} agent{}",
            if n == 1 { "" } else { "s" }
        )));
        send(tx, Command::CancelTree(session));
    }

    /// Alt+Up: take the newest queued steer back into the editor. Only into
    /// an empty editor, so nothing typed is overwritten; the text arrives
    /// with `UiEvent::Withdrawn` once the agent confirms it was still queued.
    pub fn withdraw(&mut self, editor_empty: bool, tx: &UnboundedSender<Command>) {
        let Some(q) = self.queued.last() else { return };
        if !editor_empty {
            self.entries.push(Entry::Info("editor not empty".into()));
            return;
        }
        if let Some(session) = self.session {
            send(tx, Command::Withdraw { session, id: q.id });
        }
    }

    /// The transcript's turns, oldest first: `(entry index, turn seq, first
    /// line of the opener)`. The 1-based position is the `n` of `/edit n`.
    fn turns(&self) -> Vec<(usize, i64, String)> {
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| match e {
                Entry::User {
                    text,
                    turn: Some(t),
                } => Some((
                    i,
                    *t,
                    crate::attach::spoken(text)
                        .map_or(text.as_str(), |(_, _, body)| body)
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .chars()
                        .take(60)
                        .collect(),
                )),
                _ => None,
            })
            .collect()
    }

    /// `[n]` of `/edit`, `/regen`, `/fork`: `None` when absent (open the
    /// picker), the turn's `(entry index, seq)` when it names one.
    fn turn_arg(&self, cmd: &str, arg: &str) -> Result<Option<(usize, i64)>, String> {
        if arg.is_empty() {
            return Ok(None);
        }
        let n: usize = arg.parse().map_err(|_| format!("usage: {cmd} [n]"))?;
        self.turns()
            .get(n.wrapping_sub(1))
            .map(|&(idx, seq, _)| Some((idx, seq)))
            .ok_or_else(|| format!("no turn #{n}"))
    }

    /// `/edit`, `/regen`, `/fork`: act on turn `arg`, or open the picker on the newest.
    pub(super) fn turn_cmd(
        &mut self,
        action: TurnAction,
        cmd: &str,
        arg: &str,
        tx: &UnboundedSender<Command>,
    ) {
        if self.session.is_none() {
            self.entries.push(Entry::Info("no session".into()));
            return;
        }
        if self.turn_blocked(action) {
            return;
        }
        match self.turn_arg(cmd, arg) {
            Err(e) => self.entries.push(Entry::Error(e)),
            Ok(Some((idx, turn))) => self.turn_action(action, idx, turn, tx),
            Ok(None) => {
                let items = self.turns();
                if items.is_empty() {
                    self.entries.push(Entry::Error("no turns yet".into()));
                } else {
                    self.menu = Some(Menu::Turns {
                        cursor: items.len() - 1,
                        items,
                        action,
                    });
                }
            }
        }
    }

    /// Edit and regenerate are refused while a turn runs (the server's
    /// `"turn in flight"`); fork copies stored rows and is not. Pushes the
    /// error and says whether `action` must wait.
    fn turn_blocked(&mut self, action: TurnAction) -> bool {
        let blocked = self.streaming && action != TurnAction::Fork;
        if blocked {
            self.entries
                .push(Entry::Error("turn in flight — /cancel first".into()));
        }
        blocked
    }

    /// Apply `action` to the turn at `entries[idx]` (29 §6.2). Regenerate
    /// drops the turn from the transcript now; the new one streams back in.
    /// Edit only puts the opener in the editor: `submit` does the rest.
    fn turn_action(
        &mut self,
        action: TurnAction,
        idx: usize,
        turn: i64,
        tx: &UnboundedSender<Command>,
    ) {
        let Some(session) = self.session else { return };
        // Checked again: a turn may have started while the picker was open.
        if self.turn_blocked(action) {
            return;
        }
        match action {
            TurnAction::Regen => {
                self.entries.truncate(idx);
                send(tx, Command::Regenerate { session, turn });
            }
            TurnAction::Edit => {
                // The line that ran `/edit` is gone; held pastes are what is left.
                if !self.pastes.is_empty() {
                    self.entries.push(Entry::Info("editor not empty".into()));
                    return;
                }
                let Some(Entry::User { text, .. }) = self.entries.get(idx) else {
                    return;
                };
                self.recall = Some(text.clone());
                self.editing = Some((idx, turn));
            }
            TurnAction::Fork => send(
                tx,
                Command::Fork {
                    session,
                    through: Some(turn),
                },
            ),
        }
    }

    /// Esc while `/edit` is pending: forget the edit and empty the editor.
    /// Returns whether one was pending.
    pub fn cancel_edit(&mut self) -> bool {
        if self.editing.take().is_none() {
            return false;
        }
        self.recall = Some(String::new());
        true
    }
}

impl Model {

    /// `/compact [summary|cut|snap|style …]`.
    pub(super) fn compact_cmd(&mut self, arg: &str, tx: &UnboundedSender<Command>) {
        let (sub, rest) = arg
            .split_once(char::is_whitespace)
            .map_or((arg, ""), |(s, r)| (s, r.trim()));
        match (sub, rest) {
            ("", _) => match self.session {
                Some(session) => send(
                    tx,
                    Command::Compact {
                        session,
                        style: None,
                    },
                ),
                None => self.entries.push(Entry::Info("no session".into())),
            },
            (token, _) if token.parse::<CompactStyle>().is_ok() => {
                let style = token.parse().expect("guard");
                match self.session {
                    Some(session) => send(
                        tx,
                        Command::Compact {
                            session,
                            style: Some(style),
                        },
                    ),
                    None => self.entries.push(Entry::Info("no session".into())),
                }
            }
            ("style", "") => send(tx, Command::GetCompactStyle),
            ("style", rest) if rest.parse::<CompactStyle>().is_ok() => {
                let style = rest.parse().expect("guard");
                send(tx, Command::SetCompactStyle(style));
            }
            _ => self.entries.push(Entry::Error(
                "usage: /compact [summary|cut|snap|style …]".into(),
            )),
        }
    }

    /// `/endless [on|off|fold|pause|resume]` (137 §8.6, 137G): switch the
    /// session's memory mode, fold the backlog, pause or resume settling, or show it.
    pub(super) fn endless_cmd(&mut self, arg: &str, tx: &UnboundedSender<Command>) {
        let Some(session) = self.session else {
            self.entries.push(Entry::Info("no session".into()));
            return;
        };
        let command = match arg.trim() {
            "" | "status" => Command::Memory {
                session,
                mode: None,
            },
            "on" => Command::Memory {
                session,
                mode: Some(MemoryMode::Endless),
            },
            "off" => Command::Memory {
                session,
                mode: Some(MemoryMode::Epochs),
            },
            "fold" => Command::EndlessFold { session },
            "pause" => Command::EndlessPause {
                session,
                paused: true,
            },
            "resume" => Command::EndlessPause {
                session,
                paused: false,
            },
            "model clear" => Command::EndlessModel {
                session,
                model: None,
            },
            a if a.starts_with("model ") => Command::EndlessModel {
                session,
                model: Some(a["model ".len()..].trim().to_owned()),
            },
            _ => {
                self.entries.push(Entry::Error(
                    "usage: /endless [on|off|fold|pause|resume|model <id>|model clear]".into(),
                ));
                return;
            }
        };
        send(tx, command);
    }
}

impl Model {

    /// `/sessions`: Enter switches to the highlighted session. The one left
    /// behind keeps running in the agent; only what is shown changes.
    pub(super) fn sessions_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        let Some(Menu::Sessions { items, cursor }) = &mut self.menu else {
            return MenuKey::Consumed;
        };
        match code {
            KeyCode::Up => *cursor = grouped_step(items, *cursor, -1),
            KeyCode::Down => *cursor = grouped_step(items, *cursor, 1),
            KeyCode::Esc => self.menu = None,
            KeyCode::Enter => {
                let picked = items.get(*cursor).and_then(|r| r.item()).map(|(s, _)| s.id);
                self.menu = None;
                match picked {
                    Some(id) if Some(id) == self.session => {}
                    Some(id) => self.resume(id, tx),
                    None => {}
                }
            }
            _ => {}
        }
        MenuKey::Consumed
    }

    /// `/edit`, `/regen`, `/fork` picker: Enter applies the action to the
    /// highlighted turn.
    pub(super) fn turns_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        let Some(Menu::Turns {
            items,
            cursor,
            action,
        }) = &mut self.menu
        else {
            return MenuKey::Consumed;
        };
        match code {
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Down => *cursor = (*cursor + 1).min(items.len().saturating_sub(1)),
            KeyCode::Esc => self.menu = None,
            KeyCode::Enter => {
                let action = *action;
                let picked = items.get(*cursor).map(|&(idx, turn, _)| (idx, turn));
                self.menu = None;
                if let Some((idx, turn)) = picked {
                    self.turn_action(action, idx, turn, tx);
                }
            }
            _ => {}
        }
        MenuKey::Consumed
    }
}

/// Owner chip for a mesh `EngineId` (a 64-hex key): its first 8 chars; `?` when empty.
pub fn engine_chip(owner: &str) -> String {
    if owner.is_empty() {
        return "?".into();
    }
    owner.chars().take(8).collect()
}

/// `/sessions` row: mark · id · stamp · model · owner chip · title (68 §10).
/// The chip is the engine's short id on a session owned elsewhere, blank here.
pub fn session_row(s: &SessionSummary, depth: usize, width: u16, current: bool) -> String {
    // Row prefix (mark, id, stamp, model, chip, gaps, box) is ~62 columns; the title gets the rest.
    let room = usize::from(width).saturating_sub(62 + 2 * depth).max(16);
    let title: String = s.title.chars().take(room).collect();
    let chip = if s.here {
        String::new()
    } else {
        engine_chip(&s.owner)
    };
    format!(
        "{} {}{}  {}  {:<20} {chip:<8}  {title}",
        if current { "●" } else { " " },
        "  ".repeat(depth),
        short(s.id),
        s.updated_at
            .with_timezone(&chrono::Local)
            .format("%m-%d %H:%M"),
        s.model.rsplit('/').next().unwrap_or(&s.model),
    )
}

/// The owner-offline sentence (68 §5), `/take` as the action. `last_seen` is
/// already text (`3m ago`, `never`).
pub fn owner_offline_line(owner: &str, last_seen: &str) -> String {
    let chip = engine_chip(owner);
    format!(
        "owned by {chip}, last seen {last_seen}. continue here now? \
         (turns not yet shipped from {chip} stay there) — /take"
    )
}

/// 139 §9.3: appended to [`owner_offline_line`] when the loaded session has a
/// turn open on its owner (`Loaded.resume` with `here = false`): the `/take`
/// is a user takeover and resumes it here (D5).
pub fn owner_turn_suffix(owner: &str) -> String {
    format!(
        " — a turn was running on {}; /take resumes it here",
        engine_chip(owner)
    )
}

/// `OwnerUnreachable{owner, last_seen}` out of a failed call's `data`: the
/// serde external form (`{"OwnerUnreachable": {…}}`) or a flat object tagged
/// `kind`/`type`. Returns the owner and `last_seen` as `ago` text.
pub fn owner_unreachable(data: Option<&str>) -> Option<(String, String)> {
    let v: serde_json::Value = serde_json::from_str(data?).ok()?;
    let tagged = ["kind", "type"]
        .iter()
        .any(|k| v.get(k).and_then(|t| t.as_str()) == Some("OwnerUnreachable"));
    let o = v.get("OwnerUnreachable").or(tagged.then_some(&v))?;
    let owner = o.get("owner")?.as_str()?.to_owned();
    let last_seen = match o.get("last_seen") {
        Some(serde_json::Value::String(t)) => chrono::DateTime::parse_from_rfc3339(t).map_or_else(
            |_| t.clone(),
            |t| ago(chrono::Utc::now() - t.with_timezone(&chrono::Utc)),
        ),
        Some(serde_json::Value::Number(ms)) => ms
            .as_i64()
            .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
            .map_or_else(|| ms.to_string(), |t| ago(chrono::Utc::now() - t)),
        _ => "never".to_owned(),
    };
    Some((owner, last_seen))
}

/// Last 8 chars of a session id, for display (the first 8 of a UUIDv7 are
/// the timestamp and collide across sessions created close together).
pub fn short(id: SessionId) -> String {
    let s = id.to_string();
    s[s.len() - 8..].to_owned()
}

/// Shown once per run before the first private session (51 §1).
pub const PRIVATE_WARNING: &str = "private session: its records live in memory only and are gone when you leave or the harness restarts; \
files its tools write are real files under a temporary root, deleted on close; \
provider retention, swap, crash dumps and effects outside the tree are not undone";

impl Model {
    /// This view's stable id for the private lease (51 §4.1): one per process.
    fn view_id() -> String {
        format!("tui-{}", std::process::id())
    }

    /// `/new private`: release any owned tree first, then ask for a fresh
    /// private root labelled with the shown project (51 §4.1).
    pub(super) fn new_private(&mut self, tx: &UnboundedSender<Command>) {
        self.leave_private(tx);
        let project = self.project.clone();
        self.reset();
        self.entries
            .push(Entry::Info("opening a private session…".into()));
        send(
            tx,
            Command::CreateSession(CreateSessionOptions {
                retention: Retention::Private,
                project,
                view: Some(Self::view_id()),
                cwd: std::env::current_dir()
                    .ok()
                    .map(|p| p.display().to_string()),
                ..Default::default()
            }),
        );
    }

    /// Release the owned private tree, if any (51 §4.3). The caller resets
    /// what was shown; the reply says how far cleanup got.
    pub(super) fn leave_private(&mut self, tx: &UnboundedSender<Command>) {
        if let Some(p) = self.private.take() {
            send(
                tx,
                Command::LeaveSession {
                    session: p.root,
                    secret: p.secret,
                },
            );
        }
    }

}
