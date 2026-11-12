use super::*;

impl Model {

    /// Peer name of a subagent in the current session's subtree; `None`
    /// for the session itself or strangers.
    pub fn peer_name(&self, id: SessionId) -> Option<&str> {
        self.agents.get(&id).map(|a| a.info.name.as_str())
    }
}

impl Model {

    /// Install an authoritative state for the shown session; an older
    /// revision (a slow reply racing a change) is dropped. `true` when installed.
    pub(super) fn set_communication(&mut self, s: CommunicationState) -> bool {
        if self
            .communication
            .as_ref()
            .is_some_and(|c| c.session == s.session && c.revision > s.revision)
        {
            return false;
        }
        self.communication = Some(s);
        true
    }

    /// Remember a body `PeerDelivered` carded, bounded.
    pub(super) fn remember_peer(&mut self, text: &str) {
        if self.peer_shown.len() == 8 {
            self.peer_shown.pop_front();
        }
        self.peer_shown.push_back(text.to_owned());
    }

    /// `/communication [on|off] [always]` (50 §2.1): bare asks for the state
    /// and prints it; `always` moves the persisted global switch, otherwise
    /// this session's memory-only opt-out. Nothing is assumed locally: the
    /// reply says what is effective and why.
    pub(super) fn communication_cmd(&mut self, arg: &str, tx: &UnboundedSender<Command>) {
        let mut words = arg.split_whitespace();
        let enabled = match words.next() {
            None => {
                self.awaiting_communication = true;
                send(tx, Command::GetCommunication(self.session));
                return;
            }
            Some("on") => true,
            Some("off") => false,
            Some(w) => {
                self.entries.push(Entry::Error(format!(
                    "/communication: expected on|off, got {w:?}"
                )));
                return;
            }
        };
        let session = match words.next() {
            Some("always") => None,
            None => match self.session {
                Some(s) => Some(s),
                None => {
                    self.entries.push(Entry::Error(
                        "no session yet: send a message first, or use `/communication on always`"
                            .into(),
                    ));
                    return;
                }
            },
            Some(w) => {
                self.entries.push(Entry::Error(format!(
                    "/communication: expected `always`, got {w:?}"
                )));
                return;
            }
        };
        self.awaiting_communication = true;
        send(
            tx,
            Command::SetCommunication {
                session,
                enabled,
                view: self.session,
            },
        );
    }

    /// `/peers` opens the picker; `/peers scope owner|isolated [alias]` sets
    /// the shown root's scope (50 §2.3): owner-only, roots only, so a child
    /// is told where to go instead of guessing its root.
    pub(super) fn peers_cmd(&mut self, arg: &str, tx: &UnboundedSender<Command>) {
        let Some(session) = self.session else {
            self.entries.push(Entry::Info(
                "no session — `/communication` shows the global default".into(),
            ));
            return;
        };
        let mut words = arg.split_whitespace();
        match words.next() {
            None => {
                self.menu = Some(Menu::Peers {
                    session,
                    peers: None,
                    requests: None,
                    cursor: 0,
                });
                send(tx, Command::ListPeers(session));
                send(tx, Command::ListPeerRequests(session));
            }
            Some("scope") => {
                let scope = match words.next().map(str::parse::<PeerScope>) {
                    Some(Ok(s)) => s,
                    _ => {
                        self.entries.push(Entry::Error(
                            "usage: /peers scope owner|isolated [alias]".into(),
                        ));
                        return;
                    }
                };
                if self.session_parent.is_some() {
                    self.entries.push(Entry::Error(
                        "scope belongs to the root session: `/parent` up to it, then `/peers scope …`"
                            .into(),
                    ));
                    return;
                }
                let alias: String = words.collect::<Vec<_>>().join(" ");
                self.awaiting_communication = true;
                send(
                    tx,
                    Command::SetPeerScope {
                        root: session,
                        scope,
                        alias: (!alias.is_empty()).then_some(alias),
                    },
                );
            }
            Some(_) => self.entries.push(Entry::Error(
                "usage: /peers [scope owner|isolated [alias]]".into(),
            )),
        }
    }

    /// `/peers` menu title: the effective state and its blocker, then the keys.
    pub fn peers_title(&self) -> String {
        let state = match &self.communication {
            None => "communication: …".to_owned(),
            Some(c) => communication_lines(c).remove(0),
        };
        format!("peers · {state}   ⏎ open  i inspect  a answer  r refresh  esc")
    }

    /// `/peers` rows in cursor order: the directory (or its one-line
    /// blocker), a separator, then pending requests by exact id.
    pub fn peers_rows(&self) -> Vec<PeerRow<'_>> {
        let Some(Menu::Peers {
            peers, requests, ..
        }) = &self.menu
        else {
            return Vec::new();
        };
        let mut rows: Vec<PeerRow<'_>> = match peers {
            None => vec![PeerRow::Note("loading peers…".into())],
            Some(Err(e)) => vec![PeerRow::Note(format!("no peers · {e}"))],
            Some(Ok(p)) if p.peers.is_empty() => {
                vec![PeerRow::Note("no peers this session may address".into())]
            }
            Some(Ok(p)) => p.peers.iter().map(PeerRow::Peer).collect(),
        };
        rows.push(PeerRow::Sep);
        match requests {
            None => rows.push(PeerRow::Note("loading pending requests…".into())),
            Some(page) => {
                let pending: Vec<PeerRow<'_>> = page
                    .messages
                    .iter()
                    .filter_map(|r| {
                        let id = r.receipt.id;
                        if page.awaiting_answer.contains(&id) {
                            Some(PeerRow::Request(r, false))
                        } else if page.awaiting_reply.contains(&id) {
                            Some(PeerRow::Request(r, true))
                        } else {
                            None
                        }
                    })
                    .collect();
                if pending.is_empty() {
                    rows.push(PeerRow::Note(
                        "no requests await a human (a restart clears live waits)".into(),
                    ));
                } else {
                    rows.extend(pending);
                }
            }
        }
        rows
    }

    /// `/peers` keys: ⏎ opens a native peer's session, `i`/⏎ on a request
    /// cards its full record, `a` answers it (exact id), `r` refetches.
    pub(super) fn peers_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        enum Act {
            Move(usize),
            Open(SessionId),
            Inspect(Entry),
            Answer(Prompt),
            Refresh,
            Note(&'static str),
            Close,
            Nothing,
        }
        let Some(Menu::Peers {
            session, cursor, ..
        }) = &self.menu
        else {
            return MenuKey::Consumed;
        };
        let session = *session;
        let rows = self.peers_rows();
        let last = rows.len().saturating_sub(1);
        let cursor = (*cursor).min(last);
        // Up/Down step over the separator row.
        let step = |to: usize, dir: isize| match rows.get(to) {
            Some(PeerRow::Sep) => to.saturating_add_signed(dir).min(last),
            _ => to,
        };
        let act = match (code, rows.get(cursor)) {
            (KeyCode::Up, _) => Act::Move(step(cursor.saturating_sub(1), -1)),
            (KeyCode::Down, _) => Act::Move(step((cursor + 1).min(last), 1)),
            (KeyCode::Esc, _) => Act::Close,
            (KeyCode::Char('r'), _) => Act::Refresh,
            (KeyCode::Enter, Some(PeerRow::Peer(p))) => {
                match agent::communication::PeerAddress::local(&p.address) {
                    Some(id) => Act::Open(id),
                    None => Act::Note("remote peer: no transcript here (/a2a shows its status)"),
                }
            }
            (KeyCode::Enter | KeyCode::Char('i'), Some(PeerRow::Request(r, resolve))) => {
                Act::Inspect(request_card(r, *resolve))
            }
            (KeyCode::Char('a'), Some(PeerRow::Request(r, resolve))) => {
                Act::Answer(Prompt::PeerAnswer {
                    session,
                    reply_to: r.receipt.id,
                    from: if *resolve {
                        r.receipt.to.clone()
                    } else {
                        r.from.clone()
                    },
                    resolve: *resolve,
                })
            }
            _ => Act::Nothing,
        };
        let Some(Menu::Peers { cursor: cur, .. }) = &mut self.menu else {
            return MenuKey::Consumed;
        };
        *cur = cursor;
        match act {
            Act::Move(to) => *cur = to,
            Act::Open(id) => {
                self.menu = None;
                self.resume(id, tx);
            }
            Act::Inspect(card) => {
                self.menu = None;
                self.entries.push(card);
            }
            Act::Answer(p) => {
                self.menu = None;
                self.prompt = Some(p);
            }
            Act::Refresh => {
                send(tx, Command::ListPeers(session));
                send(tx, Command::ListPeerRequests(session));
            }
            Act::Note(n) => self.entries.push(Entry::Info(n.into())),
            Act::Close => self.menu = None,
            Act::Nothing => {}
        }
        MenuKey::Consumed
    }
}

pub(super) fn on_off(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

/// First eight hex digits of a message id: enough to tell rows apart in a
/// menu; the exact id travels in the command, never re-parsed from this.
pub(super) fn short_uuid(id: Uuid) -> String {
    id.simple().to_string()[..8].to_owned()
}

/// The peer card of a consumed exchange (live `PeerDelivered` or replayed
/// `HistoryEntry::User { peer }`): titled by its authenticated origin, then
/// `facts` (a contact row's, [`contact_row`]).
pub(super) fn peer_card(origin: &PeerOrigin, body: String, facts: &str) -> Entry {
    let mut title = format!("{} · {}", origin.from, short_uuid(origin.id));
    if let Some(r) = origin.reply_to {
        title.push_str(&format!(" · reply to {}", short_uuid(r)));
    }
    title.push_str(facts);
    // The sender address and a contact's facts are remote text.
    card(
        CardKind::Peer,
        clean(&title).replace(['\n', '\t'], " "),
        &body,
    )
}

/// 183 §11.5/§11.6: a peer row a contact wrapper opened, as `(facts, words)`:
/// the lone `<contact_message>`'s words (the LAST close ends it, so a close
/// inside the words stays theirs; the harness note is the model's), or the
/// `<message>` after an inbox's `<contact_inbox/>` notice. `facts` is the
/// header tail: ` · bob on <machine> via <export> · read-only`.
pub(super) fn contact_row(text: &str) -> Option<(String, String)> {
    let text = text.trim();
    let (open, words) = if let Some(rest) = text.strip_prefix("<contact_message ") {
        let (open, body) = rest.strip_suffix("</contact_message>")?.split_once('>')?;
        let body = body.strip_prefix('\n').unwrap_or(body);
        let body = match body.split_once('\n') {
            Some((note, words)) if note.starts_with("This message comes from ") => words,
            _ => body,
        };
        (open, body.trim_end().to_owned())
    } else {
        let (open, rest) = text.strip_prefix("<contact_inbox ")?.split_once("/>")?;
        let rest = rest.trim_start();
        let rest = match rest.split_once('\n') {
            Some((note, rest)) if note.starts_with("Messages in this session come from ") => rest,
            _ => rest,
        };
        let mut cards = injected(rest)?;
        match cards.pop() {
            Some((CardKind::Peer, _, words)) if cards.is_empty() => (open, words),
            _ => return None,
        }
    };
    let cap = match attr(open, "cap=\"").unwrap_or_default() {
        "read_only" => "read-only".to_owned(),
        "mutating" => "can make changes".to_owned(),
        other => unescape(other),
    };
    // Each part only when the wrapper names it, as the web header does.
    let mut facts = String::new();
    for (lead, key) in [
        ("", "contact=\""),
        ("on ", "machine=\""),
        ("via ", "export=\""),
    ] {
        let v = unescape(attr(open, key).unwrap_or_default());
        if !v.is_empty() {
            facts.push_str(if facts.is_empty() { " · " } else { " " });
            facts.push_str(lead);
            facts.push_str(&v);
        }
    }
    if !cap.is_empty() {
        facts.push_str(" · ");
        facts.push_str(&cap);
    }
    Some((facts, words))
}

/// `s` without control characters (an ESC would reach the terminal: iocraft
/// draws text verbatim), keeping line breaks and tabs. For remote text.
pub(crate) fn clean(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect()
}

/// 183: an outside agent's card (a `contact:` / `peer:` sender, the web's
/// `isOutside`): its words draw as plain text, never Markdown, no live links.
/// A peer card's title opens with the sender's address.
pub(crate) fn outside_card(kind: CardKind, title: &str) -> bool {
    kind == CardKind::Peer && (title.starts_with("contact:") || title.starts_with("peer:"))
}

/// The inspector card of one pending request (`/peers` `i`): every durable
/// field above the body, expanded so it reads at once.
fn request_card(r: &PeerRecord, resolve: bool) -> Entry {
    let rc = &r.receipt;
    let mut meta = vec![
        format!("id: {}", rc.id),
        format!("from: {}", r.from),
        format!("to: {}", rc.to),
        format!("state: {}", rc.state.as_str()),
        format!(
            "created: {}",
            r.created_at
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S")
        ),
        if resolve {
            "pending: this session sent it and is blocked on the reply — `a` resolves its wait with your text".to_owned()
        } else {
            "pending: the sender awaits this session's answer — `a` answers on its behalf"
                .to_owned()
        },
    ];
    if let Some(rt) = rc.reply_to {
        meta.push(format!("reply_to: {rt}"));
    }
    if let Some(s) = r.request_scope {
        meta.push(format!("request scope: {s}"));
    }
    if let Some(e) = &rc.error {
        meta.push(format!("error: {e}"));
    }
    Entry::Card {
        kind: CardKind::Peer,
        title: format!("request {} · {}", short_uuid(rc.id), r.from),
        text: Box::new(markdown::Text::complete(&format!(
            "{}\n\n{}",
            meta.join("  \n"),
            r.text
        ))),
        expanded: true,
    }
}

/// `/communication` lines (50 §2.1): the effective state with its exact
/// blocker first, then the switches it derives from, then the network. The
/// global switch is a ceiling: a session `on` under global `off` says so.
pub fn communication_lines(s: &CommunicationState) -> Vec<String> {
    let first = match (s.session, s.effective) {
        (None, _) => format!(
            "communication: {} (global default · source {}){}",
            on_off(s.global),
            s.source,
            if s.global {
                ""
            } else {
                " — `/communication on always` enables the harness"
            }
        ),
        (Some(_), true) => "communication: on (this session) — routes still need scope or a grant".to_owned(),
        (Some(_), false) if !s.global => format!(
            "communication: off — disabled globally (source {}); a session `on` cannot override it, `/communication on always` enables the harness",
            s.source
        ),
        (Some(_), false) if s.opted_out => {
            "communication: off — this session opted out (memory-only: cleared on daemon restart); `/communication on` re-enables it".to_owned()
        }
        (Some(_), false) => format!(
            "communication: off — inherited: ancestor {} opted out (memory-only: cleared on daemon restart); `/communication on` there re-enables the tree",
            s.inherited_from.map_or_else(|| "?".to_owned(), short)
        ),
    };
    let mut out = vec![first];
    if s.session.is_some() {
        out.push(format!(
            "  global {} ({})  ·  this session {}  ·  root scope {}{}",
            on_off(s.global),
            s.source,
            if s.opted_out { "off" } else { "on" },
            s.scope.as_str(),
            s.alias
                .as_deref()
                .map_or_else(String::new, |a| format!(" · alias {a}"))
        ));
    }
    out.push(match (s.network_enabled, s.listening) {
        (false, _) => "  network (a2a): off — `a2a.enabled: false`".to_owned(),
        (true, true) => "  network (a2a): on · listening".to_owned(),
        (true, false) if !s.global => {
            "  network (a2a): enabled · listener stopped while communication is off globally"
                .to_owned()
        }
        (true, false) => "  network (a2a): enabled · not listening — `/a2a` shows why".to_owned(),
    });
    out
}

/// `/a2a` lines: the listener, then one row per configured peer and export,
/// errors where they belong.
pub fn a2a_lines(s: &a2a::A2aStatus) -> Vec<String> {
    let mut out = vec![match (s.enabled, s.listening) {
        (false, _) => "a2a: off — `a2a.enabled: false`; nothing listens, nothing dials".to_owned(),
        (true, true) => format!(
            "a2a: on · listening {}{}",
            s.address.as_deref().unwrap_or("?"),
            s.public_url
                .as_deref()
                .map_or_else(String::new, |u| format!(" · public {u}"))
        ),
        (true, false) => {
            "a2a: enabled · not listening (communication off globally, or see error)".to_owned()
        }
    }];
    if let Some(e) = &s.error {
        out.push(format!("  error: {e}"));
    }
    for p in &s.peers {
        out.push(format!(
            "  peer {:<16} {}{}  sessions {}  exports {}{}{}",
            p.name,
            if p.outbound { "→out " } else { "" },
            if p.inbound { "←in" } else { "" },
            p.sessions.len(),
            if p.exports.is_empty() {
                "–".to_owned()
            } else {
                p.exports.join(",")
            },
            p.card
                .as_deref()
                .map_or_else(String::new, |c| format!("  card {c}")),
            p.error
                .as_deref()
                .map_or_else(String::new, |e| format!("  error: {e}")),
        ));
    }
    for e in &s.exports {
        out.push(format!(
            "  export {:<14} session {}  {}{}",
            e.name,
            short(e.session),
            e.url,
            e.peer
                .as_deref()
                .map_or_else(String::new, |p| format!("  peer {p}"))
        ));
    }
    if s.enabled && s.peers.is_empty() && s.exports.is_empty() {
        out.push(
            "  no peers or exports configured (peers.yaml; `/a2a reload` applies edits)".to_owned(),
        );
    }
    out
}

/// One row of the `/peers` menu.
#[derive(Debug)]
pub enum PeerRow<'a> {
    /// A permitted peer; ⏎ opens a native one.
    Peer(&'a PeerInfo),
    /// Blank line between the directory and the requests; the cursor skips it.
    Sep,
    /// A request a human may resolve now: `true` when the shown session
    /// sent it and awaits the reply, `false` when it must answer.
    Request(&'a PeerRecord, bool),
    /// A hint or blocker in place of rows.
    Note(String),
}

impl PeerRow<'_> {
    /// The row's text.
    pub fn line(&self) -> String {
        match self {
            PeerRow::Peer(p) => format!(
                "{} {:<20} {:<12} {:<10} {:<8} {}{}{}",
                match p.state {
                    agent::communication::PeerState::Running
                    | agent::communication::PeerState::Awaiting => "●",
                    agent::communication::PeerState::Idle => "○",
                    _ => " ",
                },
                p.name.chars().take(20).collect::<String>(),
                format!("{:?}", p.relationship).to_lowercase(),
                format!("{:?}", p.state).to_lowercase(),
                format!("{:?}", p.kind).to_lowercase(),
                if p.send { "send " } else { "" },
                if p.reply { "reply " } else { "" },
                p.address
            ),
            PeerRow::Sep => String::new(),
            PeerRow::Request(r, resolve) => format!(
                "{} {}  {:<44}  {}",
                if *resolve { "↑" } else { "?" },
                short_uuid(r.receipt.id),
                if *resolve {
                    format!("to {} · awaiting their reply", r.receipt.to)
                } else {
                    format!("from {} · awaiting your answer", r.from)
                }
                .chars()
                .take(44)
                .collect::<String>(),
                r.text
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .chars()
                    .take(50)
                    .collect::<String>()
            ),
            PeerRow::Note(n) => n.clone(),
        }
    }
}
