//! Communication switches, peer directory/answers and authenticated transcript cards.
use super::*;
use std::collections::VecDeque;
use uuid::Uuid;
use serde::Deserialize;
use rpc_compose::api::clients::{AgentMessagingApiClient, A2aMessagingApiClient};
use agent::{AgentEvent, CommunicationChanged, CommunicationState, PeerDelivered, PeerInfo,
    PeerMessageChanged, PeerMessagePage, PeerPage, PeerReceipt, PeerRecord, PeerScope};
use crate::model::{CardKind, clean, short, on_off, attr, unescape, injected};

/// Session-scoped communication presentation and bounded legacy echo identities.
#[derive(Default)]
pub(crate) struct Peers {
    pub(crate) communication: Option<CommunicationState>,
    awaiting_communication: bool,
    refresh_communication: bool,
    pub(crate) refresh_peers: bool,
    pub(crate) menu: Option<Menu>,
    pub(crate) prompt: Option<Prompt>,
    peer_shown: VecDeque<String>,
    legacy_echo: bool,
}
#[derive(Debug)]
pub(crate) enum Menu {
    Peers { session: SessionId, peers: Option<Result<PeerPage, String>>, requests: Option<PeerMessagePage>, cursor: usize },
}
#[derive(Debug)]
pub(crate) enum Prompt {
    PeerAnswer { session: SessionId, reply_to: Uuid, from: String, resolve: bool },
}
/// Exact owner request carried into the effect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Read the selected session or global default.
    GetCommunication(Option<SessionId>),
    /// Change a switch, then read the original view.
    SetCommunication {
        /// None selects the persisted global switch.
        session: Option<SessionId>,
        /// Requested switch value.
        enabled: bool,
        /// View through which a global change is read back.
        view: Option<SessionId>,
    },
    /// Change a root's routing scope and optional alias.
    SetPeerScope {
        /// Selected root.
        root: SessionId,
        /// Exact requested scope.
        scope: PeerScope,
        /// None keeps the existing alias.
        alias: Option<String>,
    },
    /// Read the permitted directory, retaining a disabled reason as a page error.
    ListPeers(SessionId),
    /// Read exact pending message ids.
    ListPeerRequests(SessionId),
    /// Answer or resolve one exact request.
    AnswerPeerRequest {
        /// Session on whose behalf the human acts.
        session: SessionId,
        /// Durable request id, never parsed from a short label.
        reply_to: Uuid,
        /// Human answer.
        text: String,
    },
    /// Inspect network listener, peers and exports.
    A2aStatus,
    /// Validate and atomically reload network configuration.
    ReloadA2a,
}
pub(crate) enum Msg {
    Communication(CommunicationState),
    Peers(SessionId, Result<PeerPage, String>),
    PeerRequests(SessionId, PeerMessagePage),
    PeerAnswered(PeerReceipt),
    A2a(a2a::A2aStatus),
    CommunicationChanged,
    PeerMessageChanged(PeerMessageChanged),
    PeerDelivered(PeerDelivered),
    A2aChanged,
    Agent,
    Failed(String),
}
impl Peers {
    fn request(&self, request: Request, cx: &mut Cx<'_, '_, Msg>) {
        cx.spawn_request(Lane::Concurrent, request, |api, request| async move {
            let result = async {
                Ok(match request {
                    Request::GetCommunication(session) => Msg::Communication(api.get_communication(session).await.map_err(crate::reason)?),
                    Request::SetCommunication { session, enabled, view } => {
                        let state = api.set_communication(session, enabled).await.map_err(crate::reason)?;
                        Msg::Communication(if session == view { state } else { api.get_communication(view).await.map_err(crate::reason)? })
                    }
                    Request::SetPeerScope { root, scope, alias } => Msg::Communication(api.set_peer_scope(root, scope, alias).await.map_err(crate::reason)?),
                    Request::ListPeers(session) => Msg::Peers(session, api.list_peers(session, None, None, None).await.map_err(crate::reason)),
                    Request::ListPeerRequests(session) => Msg::PeerRequests(session, api.list_peer_messages(session, None, None, true).await.map_err(crate::reason)?),
                    Request::AnswerPeerRequest { session, reply_to, text } => Msg::PeerAnswered(api.answer_peer_request(session, reply_to, text).await.map_err(crate::reason)?),
                    Request::A2aStatus => Msg::A2a(api.a2a_status().await.map_err(crate::reason)?),
                    Request::ReloadA2a => Msg::A2a(api.reload_a2a().await.map_err(crate::reason)?),
                })
            }.await;
            result.unwrap_or_else(Msg::Failed)
        });
    }
    fn set_communication(&mut self, state: CommunicationState) -> bool {
        if self.communication.as_ref().is_some_and(|old| old.session == state.session && old.revision > state.revision) { return false }
        self.communication = Some(state);
        true
    }
    fn communication_cmd(&mut self, arg: &str, cx: &mut Cx<'_, '_, Msg>) {
        let mut words = arg.split_whitespace();
        let enabled = match words.next() {
            None => { self.awaiting_communication = true; self.request(Request::GetCommunication(cx.core.session), cx); return; }
            Some("on") => true,
            Some("off") => false,
            Some(word) => { cx.core(CoreAction::Reject(format!("/communication: expected on|off, got {word:?}"))); return; }
        };
        let session = match words.next() {
            Some("always") => None,
            None => match cx.core.session {
                Some(session) => Some(session),
                None => { cx.core(CoreAction::Reject("no session yet: send a message first, or use `/communication on always`".into())); return; }
            },
            Some(word) => { cx.core(CoreAction::Reject(format!("/communication: expected `always`, got {word:?}"))); return; }
        };
        self.awaiting_communication = true;
        self.request(Request::SetCommunication { session, enabled, view: cx.core.session }, cx);
    }
    fn peers_cmd(&mut self, arg: &str, cx: &mut Cx<'_, '_, Msg>) {
        let Some(session) = cx.core.session else {
            cx.core(CoreAction::Notice("no session — `/communication` shows the global default".into())); return;
        };
        let mut words = arg.split_whitespace();
        match words.next() {
            None => {
                self.menu = Some(Menu::Peers { session, peers: None, requests: None, cursor: 0 });
                self.prompt = None;
                cx.focus(FocusClass::Modal);
                self.request(Request::ListPeers(session), cx);
                self.request(Request::ListPeerRequests(session), cx);
            }
            Some("scope") => {
                let scope = match words.next().map(str::parse::<PeerScope>) {
                    Some(Ok(scope)) => scope,
                    _ => { cx.core(CoreAction::Reject("usage: /peers scope owner|isolated [alias]".into())); return; }
                };
                if cx.core.parent.is_some() {
                    cx.core(CoreAction::Reject("scope belongs to the root session: `/parent` up to it, then `/peers scope …`".into())); return;
                }
                let alias = words.collect::<Vec<_>>().join(" ");
                self.awaiting_communication = true;
                self.request(Request::SetPeerScope { root: session, scope, alias: (!alias.is_empty()).then_some(alias) }, cx);
            }
            Some(_) => cx.core(CoreAction::Reject("usage: /peers [scope owner|isolated [alias]]".into())),
        }
    }
    /// Effective state and blocker, followed by unchanged menu keys.
    pub fn peers_title(&self) -> String {
        let state = match &self.communication { None => "communication: …".to_owned(), Some(state) => communication_lines(state).remove(0) };
        format!("peers · {state}   ⏎ open  i inspect  a answer  r refresh  esc")
    }
    /// Directory, separator, then exact pending requests in durable page order.
    pub fn peers_rows(&self) -> Vec<PeerRow<'_>> {
        let Some(Menu::Peers { peers, requests, .. }) = &self.menu else { return Vec::new() };
        let mut rows = match peers {
            None => vec![PeerRow::Note("loading peers…".into())],
            Some(Err(error)) => vec![PeerRow::Note(format!("no peers · {error}"))],
            Some(Ok(page)) if page.peers.is_empty() => vec![PeerRow::Note("no peers this session may address".into())],
            Some(Ok(page)) => page.peers.iter().map(PeerRow::Peer).collect(),
        };
        rows.push(PeerRow::Sep);
        match requests {
            None => rows.push(PeerRow::Note("loading pending requests…".into())),
            Some(page) => {
                let pending: Vec<_> = page.messages.iter().filter_map(|record| {
                    let id = record.receipt.id;
                    if page.awaiting_answer.contains(&id) { Some(PeerRow::Request(record, false)) }
                    else if page.awaiting_reply.contains(&id) { Some(PeerRow::Request(record, true)) } else { None }
                }).collect();
                if pending.is_empty() { rows.push(PeerRow::Note("no requests await a human (a restart clears live waits)".into())); }
                else { rows.extend(pending); }
            }
        }
        rows
    }
    fn key(&mut self, code: KeyCode, cx: &mut Cx<'_, '_, Msg>) {
        if self.prompt.is_some() {
            if code == KeyCode::Esc {
                self.prompt = None; cx.close(); cx.consume_key();
                cx.core(CoreAction::Notice("peers: answer cancelled".into()));
            }
            return;
        }
        #[expect(clippy::large_enum_variant, reason = "This synchronous local action moves the card without allocating.")]
        enum Act { Move(usize), Open(SessionId), Inspect(TranscriptCard), Answer(Prompt), Refresh, Note(&'static str), Close, Nothing }
        let Some(Menu::Peers { session, cursor, .. }) = &self.menu else { return };
        let session = *session;
        let rows = self.peers_rows();
        let last = rows.len().saturating_sub(1);
        let cursor = (*cursor).min(last);
        let step = |to: usize, dir: isize| match rows.get(to) { Some(PeerRow::Sep) => to.saturating_add_signed(dir).min(last), _ => to };
        let act = match (code, rows.get(cursor)) {
            (KeyCode::Up, _) => Act::Move(step(cursor.saturating_sub(1), -1)),
            (KeyCode::Down, _) => Act::Move(step((cursor + 1).min(last), 1)),
            (KeyCode::Esc, _) => Act::Close,
            (KeyCode::Char('r'), _) => Act::Refresh,
            (KeyCode::Enter, Some(PeerRow::Peer(peer))) => match agent::communication::PeerAddress::local(&peer.address) {
                Some(id) => Act::Open(id), None => Act::Note("remote peer: no transcript here (/a2a shows its status)"),
            },
            (KeyCode::Enter | KeyCode::Char('i'), Some(PeerRow::Request(record, resolve))) => Act::Inspect(request_card(record, *resolve)),
            (KeyCode::Char('a'), Some(PeerRow::Request(record, resolve))) => Act::Answer(Prompt::PeerAnswer {
                session, reply_to: record.receipt.id, from: if *resolve { record.receipt.to.clone() } else { record.from.clone() }, resolve: *resolve,
            }),
            _ => Act::Nothing,
        };
        cx.consume_key();
        let Some(Menu::Peers { cursor: current, .. }) = &mut self.menu else { return };
        *current = cursor;
        match act {
            Act::Move(to) => *current = to,
            Act::Open(id) => { self.menu = None; cx.close(); cx.core(CoreAction::ResumeSession(id)); }
            Act::Inspect(card) => { self.menu = None; cx.close(); cx.core(CoreAction::AppendCard(card)); }
            Act::Answer(prompt) => { self.menu = None; self.prompt = Some(prompt); }
            Act::Refresh => { self.request(Request::ListPeers(session), cx); self.request(Request::ListPeerRequests(session), cx); }
            Act::Note(note) => cx.core(CoreAction::Notice(note.into())),
            Act::Close => { self.menu = None; cx.close(); }
            Act::Nothing => {}
        }
    }
    fn message(&mut self, message: Msg, cx: &mut Cx<'_, '_, Msg>) {
        if !matches!(&message, Msg::Agent | Msg::Failed(_)) && cx.core.reasoning { cx.plain(Plain::Text("\x1b[0m\n".into())); }
        match message {
            Msg::Failed(error) => cx.error(error),
            Msg::Communication(state) => {
                for line in communication_lines(&state) { cx.plain(Plain::Line(line)); }
                if state.session != cx.core.session { return }
                let print = std::mem::take(&mut self.awaiting_communication);
                if self.set_communication(state) && print {
                    for line in communication_lines(self.communication.as_ref().expect("just set")) { cx.core(CoreAction::Notice(line)); }
                }
            }
            Msg::Peers(session, page) => {
                match &page {
                    Err(error) => cx.plain(Plain::Line(format!("no peers · {error}"))),
                    Ok(page) if page.peers.is_empty() => cx.plain(Plain::Line("no peers this session may address".into())),
                    Ok(page) => for peer in &page.peers { cx.plain(Plain::Line(PeerRow::Peer(peer).line())); },
                }
                if let Some(Menu::Peers { session: current, peers, .. }) = &mut self.menu && *current == session { *peers = Some(page); }
            }
            Msg::PeerRequests(session, page) => {
                let mut any = false;
                for record in &page.messages {
                    let id = record.receipt.id;
                    let side = if page.awaiting_answer.contains(&id) { format!("from {} · awaiting your answer", record.from) }
                        else if page.awaiting_reply.contains(&id) { format!("to {} · awaiting their reply", record.receipt.to) } else { continue };
                    any = true;
                    cx.plain(Plain::Line(format!("{id}  {side}  {}", record.text.lines().next().unwrap_or_default())));
                }
                if !any { cx.plain(Plain::Line("no requests await a human (a restart clears live waits)".into())); }
                if let Some(Menu::Peers { session: current, requests, .. }) = &mut self.menu && *current == session { *requests = Some(page); }
            }
            Msg::PeerAnswered(receipt) => {
                let line = |id| format!("peers: {id} answered → {}{}{}", receipt.state.as_str(),
                    receipt.answered_by.as_deref().map_or_else(String::new, |by| format!(" · by {by}")),
                    receipt.error.as_deref().map_or_else(String::new, |error| format!(" · {error}")));
                let id = receipt.reply_to.unwrap_or(receipt.id);
                cx.plain(Plain::Line(line(id.to_string())));
                cx.core(CoreAction::Notice(line(short_uuid(id))));
            }
            Msg::A2a(status) => for line in a2a_lines(&status) { cx.plain(Plain::Line(line.clone())); cx.core(CoreAction::Notice(line)); },
            Msg::CommunicationChanged | Msg::A2aChanged => { self.refresh_communication = true; self.refresh_peers |= self.menu.is_some(); }
            Msg::PeerMessageChanged(event) => { self.refresh_peers |= matches!(&self.menu, Some(Menu::Peers { session, .. }) if *session == event.session); }
            Msg::PeerDelivered(event) => {
                cx.plain(Plain::Line(format!("⟵ {} · {}{}: {}…", event.origin.from, event.origin.id,
                    event.origin.reply_to.map_or_else(String::new, |id| format!(" · reply to {id}")), event.text.lines().next().unwrap_or_default())));
                if cx.core.session == Some(event.session) {
                    if self.peer_shown.len() == 8 { self.peer_shown.pop_front(); }
                    self.peer_shown.push_back(event.text.clone());
                    cx.core(CoreAction::InterjectCard(peer_card(WrapperOrigin { id: event.origin.id, from: &event.origin.from, reply_to: event.origin.reply_to }, event.text, "")));
                }
            }
            Msg::Agent => {
                self.legacy_echo = false;
                if let Some(AgentEvent::MessageDelivered { session, from, text }) = cx.core.agent
                    && cx.core.session == Some(*session) && !from.starts_with("job ")
                    && let Some(at) = self.peer_shown.iter().position(|shown| shown == text) {
                    self.peer_shown.remove(at); self.legacy_echo = true;
                }
            }
        }
    }
}
impl Component for Peers {
    type Msg = Msg;
    fn declaration(&self) -> Declaration {
        Declaration { id: "peers", interests: &[
            Interest { topic: "agent.communication", version: 1, variants: &[] },
            Interest { topic: "agent.peer-message", version: 1, variants: &[] },
            Interest { topic: "agent.peer-delivered", version: 1, variants: &[] },
            Interest { topic: "a2a.changed", version: 1, variants: &[] },
            Interest { topic: "agent.events", version: 1, variants: &["MessageDelivered"] },
        ], slash: &[
            ("/communication", "[on|off] [always]", "agent-to-agent communication: status with its exact blocker; `always` persists the global switch"),
            ("/peers", "[scope owner|isolated [alias]]", "peers this session may address and requests awaiting a human (picker: ⏎ opens, i inspects, a answers)"),
            ("/a2a", "[reload]", "network (A2A) status: listener, configured peers and exports; reload swaps peers.yaml"),
        ], bindings: &[] }
    }
    fn decode(&self, event: &TopicEvent, core: &CoreView<'_>) -> Option<Msg> {
        if event.topic().is_some() && event.version() != Some(1) { return None }
        if matches!(event.topic(), Some("agent.events") | None) && matches!(core.agent, Some(AgentEvent::MessageDelivered { .. })) { return Some(Msg::Agent) }
        match (event.topic(), event.event()["type"].as_str()) {
            (Some("agent.communication"), _) | (None, Some("CommunicationChanged")) => CommunicationChanged::deserialize(event.event()).ok().map(|_| Msg::CommunicationChanged),
            (Some("agent.peer-message"), _) | (None, Some("PeerMessageChanged")) => PeerMessageChanged::deserialize(event.event()).ok().map(Msg::PeerMessageChanged),
            (Some("agent.peer-delivered"), _) | (None, Some("PeerDelivered")) => PeerDelivered::deserialize(event.event()).ok().map(Msg::PeerDelivered),
            (Some("a2a.changed"), _) | (None, Some("A2aChanged")) => a2a::A2aChanged::deserialize(event.event()).ok().map(|_| Msg::A2aChanged),
            _ => None,
        }
    }
    fn update(&mut self, input: Input<'_, Msg>, cx: &mut Cx<'_, '_, Msg>) {
        match input {
            Input::Slash { name: "/communication", args } => self.communication_cmd(args, cx),
            Input::Slash { name: "/peers", args } => self.peers_cmd(args, cx),
            Input::Slash { name: "/a2a", args } => match args {
                "" => self.request(Request::A2aStatus, cx), "reload" => self.request(Request::ReloadA2a, cx),
                _ => cx.core(CoreAction::Reject("usage: /a2a [reload]".into())),
            },
            Input::Key(key) => self.key(key.code, cx),
            Input::Submit(line) => if let Some(Prompt::PeerAnswer { session, reply_to, .. }) = self.prompt.take() {
                cx.consume_key(); cx.close(); self.request(Request::AnswerPeerRequest { session, reply_to, text: line.to_owned() }, cx);
            },
            Input::Message(message) => self.message(message, cx),
            Input::Tick { .. } => {
                if std::mem::take(&mut self.refresh_communication) && cx.core.interactive { self.request(Request::GetCommunication(cx.core.session), cx); }
                if std::mem::take(&mut self.refresh_peers) && cx.core.interactive && let Some(Menu::Peers { session, .. }) = &self.menu {
                    let session = *session; self.request(Request::ListPeers(session), cx); self.request(Request::ListPeerRequests(session), cx);
                }
            }
            Input::Lifecycle(Lifecycle::Mount) if cx.core.interactive => self.request(Request::GetCommunication(None), cx),
            Input::Lifecycle(Lifecycle::SessionReset) => { *self = Self::default(); self.refresh_communication = true; }
            Input::Lifecycle(Lifecycle::SessionChanged | Lifecycle::EngineChanged | Lifecycle::Resync) => self.refresh_communication = true,
            Input::Lifecycle(Lifecycle::History(loaded)) => if let Some(state) = &loaded.communication { self.set_communication(state.clone()); },
            Input::Lifecycle(Lifecycle::FocusLost) => { self.menu = None; self.prompt = None; }
            _ => {}
        }
    }
    fn wrapper_tags(&self) -> &'static [&'static str] { &["message", "subagent_result", "user"] }
    fn wrapper(&self, row: Wrapper<'_>) -> WrapperView {
        if row.tag == "user" {
            if let Some(origin) = row.origin {
                let (facts, words) = contact_row(row.body).unwrap_or_else(|| {
                    let words = injected(row.body, |row| Some(project_xml(row))).and_then(|cards| cards.into_iter().next()).map_or_else(|| row.body.to_owned(), |(_, _, body)| body);
                    (String::new(), words)
                });
                let title = peer_title(origin, &facts);
                return WrapperView::Card { kind: kind(&title), title, body: words };
            }
            return if row.live && contact_row(row.body).is_some() { WrapperView::Hidden } else { WrapperView::Fallback };
        }
        if let Some(sender) = row.sender {
            if sender.starts_with("job ") { return WrapperView::Fallback }
            return if self.legacy_echo { WrapperView::Hidden } else { WrapperView::Card { kind: kind(sender), title: sender.to_owned(), body: row.body.to_owned() } };
        }
        project_xml(row)
    }
    fn editor(&self) -> Option<Editor> {
        let Prompt::PeerAnswer { reply_to, from, resolve, .. } = self.prompt.as_ref()?;
        Some(Editor { hint: format!("peers  {} {} · {}  answer  (esc cancel)",
            if *resolve { "resolve own wait on" } else { "answer for this session to" }, short_uuid(*reply_to), from), masked: false, submit_empty: false })
    }
    fn view(&self, at: Surface<'_>, cx: &ViewCx<'_>) -> Option<View> {
        match at {
            Surface::Header(HeaderSlot::Communication) => self.communication.as_ref().map(|state| View::Text(if state.effective { "  ·  communication on" } else { "  ·  communication off" }.into())),
            Surface::Modal => {
                let Some(Menu::Peers { cursor, .. }) = self.menu else { return None };
                Some(View::Element { element: crate::ui::menu_view(self.peers_title(), self.peers_rows().iter().map(PeerRow::line).collect(), cursor,
                    cx.width as u16, cx.height as u16, false), rows: 0, replace: false })
            }
            _ => None,
        }
    }
    fn wake(&self) -> Option<u64> { (self.refresh_communication || self.refresh_peers).then_some(0) }
}
fn project_xml(row: Wrapper<'_>) -> WrapperView {
    let key = match row.tag { "message" => "from=\"", "subagent_result" => "name=\"", _ => return WrapperView::Fallback };
    let Some(from) = attr(row.open, key) else { return WrapperView::Fallback };
    WrapperView::Card { kind: kind(from), title: from.to_owned(), body: row.body.to_owned() }
}
/// Neutral peer-card metadata; outside senders request literal text at both layout and paint.
pub(crate) fn kind(title: &str) -> CardKind {
    let mut kind = CardKind::Component { group: "peer", key: Uuid::nil(), glyph: "⟵", color: crate::ui::FAIENCE, plain: false };
    let literal = outside_card(kind, title);
    if let CardKind::Component { plain, .. } = &mut kind { *plain = literal; }
    kind
}
/// First eight hex digits of a message id: enough to tell rows apart in a
/// menu; the exact id travels in the command, never re-parsed from this.
pub(super) fn short_uuid(id: Uuid) -> String {
    id.simple().to_string()[..8].to_owned()
}

/// The peer card of a consumed exchange (live `PeerDelivered` or replayed
/// `HistoryEntry::User { peer }`): titled by its authenticated origin, then
/// `facts` (a contact row's, [`contact_row`]).
fn peer_title(origin: WrapperOrigin<'_>, facts: &str) -> String {
    let mut title = format!("{} · {}", origin.from, short_uuid(origin.id));
    if let Some(r) = origin.reply_to {
        title.push_str(&format!(" · reply to {}", short_uuid(r)));
    }
    title.push_str(facts);
    // The sender address and a contact's facts are remote text.
    clean(&title).replace(['\n', '\t'], " ")
}

fn peer_card(origin: WrapperOrigin<'_>, body: String, facts: &str) -> TranscriptCard {
    let title = peer_title(origin, facts);
    TranscriptCard { entry_key: None, group: "peer", key: origin.id, glyph: "⟵", color: crate::ui::FAIENCE,
        plain: outside_card(kind(""), &title), title, body: markdown::Text::complete(&body), expanded: false }
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
        let mut cards = injected(rest, |row| Some(project_xml(row)))?;
        match cards.pop() {
            Some((CardKind::Component { group: "peer", .. }, _, words)) if cards.is_empty() => (open, words),
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
/// 183: an outside agent's card (a `contact:` / `peer:` sender, the web's
/// `isOutside`): its words draw as plain text, never Markdown, no live links.
/// A peer card's title opens with the sender's address.
pub(crate) fn outside_card(kind: CardKind, title: &str) -> bool {
    matches!(kind, CardKind::Component { group: "peer", .. }) && (title.starts_with("contact:") || title.starts_with("peer:"))
}

/// The inspector card of one pending request (`/peers` `i`): every durable
/// field above the body, expanded so it reads at once.
fn request_card(r: &PeerRecord, resolve: bool) -> TranscriptCard {
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
    TranscriptCard { entry_key: None, group: "peer", key: rc.id, glyph: "⟵", color: crate::ui::FAIENCE, plain: false,
    title: format!("request {} · {}", short_uuid(rc.id), r.from),
    body: markdown::Text::complete(&format!("{}\n\n{}", meta.join("  \n"), r.text)),
    expanded: true, }
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

#[cfg(test)]
#[test]
fn typed_peer_delivery_and_legacy_echo_share_one_card_above_streaming_assistant() {
    use crate::model::{Model, Entry, UiEvent};
    let session = SessionId::new();
    let turn = runtime::TurnId::new();
    let mut model = Model::new("m".into(), true);
    model.session = Some(session);
    let event = |event| UiEvent::Wire(super::tests::topic("agent.events", 1, serde_json::to_value(event).unwrap()));
    model.apply(event(AgentEvent::TextDelta { session, turn, text: "before".into() }));
    model.apply(UiEvent::Wire(super::tests::topic("agent.peer-delivered", 1, serde_json::to_value(PeerDelivered {
        session, origin: agent::PeerOrigin { id: Uuid::nil(), from: "session:source".into(), reply_to: None, scope: None, contact_mode: None },
        text: "PONG".into(), seq: 1, turn,
    }).unwrap())));
    model.apply(event(AgentEvent::MessageDelivered { session, from: "session:source".into(), text: "PONG".into() }));
    model.apply(event(AgentEvent::TextDelta { session, turn, text: " after".into() }));
    assert!(matches!(model.entries.as_slice(),
        [Entry::Card { title, text, .. }, Entry::Assistant(tail)]
        if title == "session:source · 00000000" && text.raw() == "PONG" && tail.raw() == "before after" && !tail.is_done()));
}
