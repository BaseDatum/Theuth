//! Link inventory, pairing countdown and composer-backed rename/revoke prompts.
use super::*;
use serde::Deserialize;
use rpc_compose::api::clients::NetworkApiClient;
use crate::model::{ago, now_ms};

/// Global cache survives session resets; prompt and countdown state do not.
#[derive(Default)]
pub struct Link {
    pub(crate) link: Option<::link::LinkStatus>,
    pub(crate) awaiting: bool,
    pub(crate) menu: Option<Menu>,
    pub(crate) prompt: Option<Prompt>,
    pairing: Option<Countdown>,
}
#[derive(Debug)]
pub(crate) enum Menu { Link { cursor: usize } }
#[derive(Debug)]
pub(crate) enum Prompt {
    DeviceName { id: String, name: String },
    Name,
    Revoke { id: String, name: String },
}
/// Owner-local request carried into its effect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Read the authoritative global cache.
    Status,
    /// Enable or disable the uplink, then re-read.
    Enable(bool),
    /// Change relay, retaining the current enabled flag.
    Relay(String),
    /// Start pairing.
    Pair,
    /// Cancel pairing.
    CancelPair,
    /// Rename this engine.
    Rename(String),
    /// Rename a paired device.
    RenameDevice(String, String),
    /// Revoke a paired device.
    Revoke(String),
}
/// Typed messages never enter the central UI enum.
pub enum Msg {
    /// Local effect completion.
    Reply(Result<Reply, String>),
    /// Live owner event.
    Event(::link::LinkEvent),
}
/// Owner-local effect result.
pub enum Reply {
    /// Authoritative status.
    Status(::link::LinkStatus),
    /// New pairing capability.
    Pairing(::link::Pairing),
    /// Existing mutation confirmation.
    Notice(String),
}

impl Link {
    fn request(&self, request: Request, cx: &mut Cx<'_, '_, Msg>) {
        cx.spawn_request(Lane::Concurrent, request, |api, request| async move {
            let result = async {
                match request {
                    Request::Status => {}
                    Request::Enable(enabled) => { api.link_enable(enabled, None).await.map_err(crate::reason)?; }
                    Request::Relay(url) => {
                        let enabled = api.link_status().await.map_err(crate::reason)?.enabled;
                        api.link_enable(enabled, Some(url)).await.map_err(crate::reason)?;
                    }
                    Request::Pair => return api.link_pair().await.map(Reply::Pairing).map_err(crate::reason),
                    Request::CancelPair => {
                        api.link_cancel_pair().await.map_err(crate::reason)?;
                        return Ok(Reply::Notice("pairing cancelled".into()));
                    }
                    Request::Rename(name) => { api.link_rename(name).await.map_err(crate::reason)?; }
                    Request::RenameDevice(id, name) => { api.link_rename_device(id, name).await.map_err(crate::reason)?; }
                    Request::Revoke(id) => { api.link_revoke_device(id).await.map_err(crate::reason)?; }
                }
                api.link_status().await.map(Reply::Status).map_err(crate::reason)
            }.await;
            Msg::Reply(result)
        });
    }
    fn slash(&mut self, arg: &str, cx: &mut Cx<'_, '_, Msg>) {
        let (sub, rest) = arg.split_once(char::is_whitespace).map_or((arg, ""), |(s, r)| (s, r.trim()));
        let request = match (sub, rest) {
            ("", _) => { self.awaiting = true; Request::Status }
            ("on", _) => { self.awaiting = true; Request::Enable(true) }
            ("off", _) => Request::Enable(false),
            ("pair", _) => Request::Pair,
            ("cancel", _) => Request::CancelPair,
            ("rm", id) if !id.is_empty() => Request::Revoke(id.to_owned()),
            ("name", rest) if rest.contains(char::is_whitespace) => {
                let (id, name) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
                Request::RenameDevice(id.to_owned(), name.trim().to_owned())
            }
            ("relay", url) if !url.is_empty() => Request::Relay(url.to_owned()),
            _ => { cx.core(CoreAction::Reject("usage: /link [on|off|pair|cancel|rm <id>|name <id> <name>|relay <url>]".into())); return; }
        };
        self.request(request, cx);
    }
    pub(crate) fn link_title(&self) -> String {
        let Some(s) = &self.link else { return "link".into() };
        let state = match (s.enabled, s.connected) {
            (false, _) => "off", (true, true) => "connected", (true, false) => "disconnected",
        };
        let id: String = s.harness_id.as_deref().map_or_else(|| "no identity".into(), |id| id.chars().take(8).collect());
        format!("link · {} · {state} · {} ({id}…)   p pair  r rename  x revoke  n node name  e {}  esc",
            relay_host(&s.relay), s.name, if s.enabled { "disable" } else { "enable" })
    }
    pub(crate) fn link_rows(&self) -> Vec<String> {
        let now = now_ms();
        let since = |ms: u64| ago(chrono::Duration::milliseconds(now.saturating_sub(ms) as i64));
        let rows: Vec<String> = self.link.iter().flat_map(|s| &s.devices).map(|d| {
            let seen = match (d.online, d.last_seen_ms) {
                (true, _) => "online".to_owned(), (false, 0) => "never seen".to_owned(),
                (false, ms) => format!("seen {}", since(ms)),
            };
            format!("{} {:<20} {:<8} paired {:<10} {seen}", if d.online { "●" } else { "○" },
                d.name.chars().take(20).collect::<String>(), platform_name(&d.platform), since(d.paired_at_ms))
        }).collect();
        if rows.is_empty() { return vec!["no devices paired · p pairs one".into()] }
        rows
    }
    fn key(&mut self, code: KeyCode, cx: &mut Cx<'_, '_, Msg>) {
        if self.prompt.is_some() {
            if code == KeyCode::Esc {
                self.prompt = None; cx.close(); cx.core(CoreAction::Notice("link: cancelled".into())); cx.consume_key();
            }
            return;
        }
        let Some(Menu::Link { cursor }) = &mut self.menu else { return };
        cx.consume_key();
        let Some(s) = &self.link else { self.menu = None; cx.close(); return };
        let device = s.devices.get(*cursor).map(|d| (d.id.clone(), d.name.clone()));
        let enabled = s.enabled;
        match code {
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Down => *cursor = (*cursor + 1).min(s.devices.len().saturating_sub(1)),
            KeyCode::Esc => { self.menu = None; cx.close(); }
            KeyCode::Char('p') => { self.menu = None; cx.close(); self.request(Request::Pair, cx); }
            KeyCode::Char('e') => self.request(Request::Enable(!enabled), cx),
            KeyCode::Char('n') => { self.menu = None; self.prompt = Some(Prompt::Name); }
            KeyCode::Char('r') => if let Some((id, name)) = device { self.menu = None; self.prompt = Some(Prompt::DeviceName { id, name }); },
            KeyCode::Char('x') => if let Some((id, name)) = device { self.menu = None; self.prompt = Some(Prompt::Revoke { id, name }); },
            _ => {}
        }
    }
    fn submit(&mut self, line: &str, cx: &mut Cx<'_, '_, Msg>) {
        let Some(prompt) = self.prompt.take() else { return };
        cx.consume_key(); cx.close();
        let request = match prompt {
            Prompt::DeviceName { id, .. } => Request::RenameDevice(id, line.to_owned()),
            Prompt::Name => Request::Rename(line.to_owned()),
            Prompt::Revoke { id, name } => {
                if !line.eq_ignore_ascii_case("y") && !line.eq_ignore_ascii_case("yes") {
                    cx.core(CoreAction::Notice(format!("link: kept {name}"))); return;
                }
                Request::Revoke(id)
            }
        };
        self.awaiting = true;
        self.request(request, cx);
    }
    fn event(&mut self, event: ::link::LinkEvent, cx: &mut Cx<'_, '_, Msg>) {
        use ::link::LinkEvent::*;
        let mut refresh = false;
        match event {
            LinkRelay { connected } => match &mut self.link { Some(s) => s.connected = connected, None => refresh = true },
            LinkPairing { key: None, .. } => {
                if let Some(card) = self.pairing.take() { card.replace("pairing expired".into(), cx); }
                if let Some(s) = &mut self.link { s.pairing = None; }
            }
            LinkPairing { key: Some(key), expires_at_ms } => {
                if let Some(s) = &mut self.link { s.pairing = Some(::link::Pairing { key, expires_at_ms: expires_at_ms.unwrap_or(0) }); }
            }
            LinkPaired { device } => {
                cx.plain(Plain::Line(format!("paired: {}", device.name)));
                if let Some(card) = self.pairing.take() { card.replace(format!("paired: {}", device.name), cx); }
                match &mut self.link {
                    Some(s) => { s.pairing = None; s.devices.retain(|d| d.id != device.id); s.devices.push(device); }
                    None => refresh = true,
                }
            }
            LinkDevice { id, online } => match self.link.as_mut().and_then(|s| s.devices.iter_mut().find(|d| d.id == id)) {
                Some(d) => { d.online = online; if !online { d.last_seen_ms = now_ms(); } }
                None => refresh = true,
            },
            LinkRevoked { id } => if let Some(s) = &mut self.link { s.devices.retain(|d| d.id != id); },
            MeshRelay { .. } | DevicePaired { .. } | DevicesChanged => refresh = true,
        }
        if refresh && cx.core.interactive { self.request(Request::Status, cx); }
    }
}
impl Component for Link {
    type Msg = Msg;
    fn declaration(&self) -> Declaration {
        Declaration { id: "link", interests: &[Interest { topic: "link.events", version: 1, variants: &[] }],
            slash: &[("/link", "[on|off|pair|rm <n>|name <n> <name>|relay <url>]", "pair and manage phones")], bindings: &[] }
    }
    fn decode(&self, event: &TopicEvent, _: &CoreView<'_>) -> Option<Msg> {
        if event.topic().is_some() && (event.topic() != Some("link.events") || event.version() != Some(1)) { return None }
        ::link::LinkEvent::deserialize(event.event()).ok().map(Msg::Event)
    }
    fn update(&mut self, input: Input<'_, Msg>, cx: &mut Cx<'_, '_, Msg>) {
        match input {
            Input::Slash { args, .. } => self.slash(args, cx),
            Input::Key(key) => self.key(key.code, cx),
            Input::Submit(line) => self.submit(line, cx),
            Input::Message(Msg::Reply(Err(error))) => cx.error(error),
            Input::Message(Msg::Reply(Ok(reply))) => {
                if cx.core.reasoning { cx.plain(Plain::Text("\x1b[0m\n".into())); }
                match reply {
                    Reply::Status(status) => {
                        let text = link_line(&status);
                        cx.plain(Plain::Line(text.clone()));
                        if self.menu.is_none() && !self.awaiting { cx.core(CoreAction::Notice(text)); }
                        if self.awaiting && self.menu.is_none() { self.menu = Some(Menu::Link { cursor: 0 }); cx.focus(FocusClass::Modal); }
                        self.awaiting = false; self.link = Some(status);
                    }
                    Reply::Pairing(pairing) => {
                        cx.plain(Plain::Line(format!("{}\n{}", ::link::qr(&pairing.key), pairing.key)));
                        if let Some(card) = self.pairing.take() { card.replace("pairing expired".into(), cx); }
                        self.pairing = Some(Countdown::append("link", "link pairing".into(), pairing, cx));
                    }
                    Reply::Notice(text) => { cx.plain(Plain::Line(text.clone())); cx.core(CoreAction::Notice(text)); }
                }
            }
            Input::Message(Msg::Event(event)) => {
                if cx.core.reasoning { cx.plain(Plain::Text("\x1b[0m\n".into())); }
                self.event(event, cx);
            }
            Input::Tick { now_ms, .. } => if let Some(card) = &mut self.pairing && !card.tick(now_ms, cx) { self.pairing = None; },
            Input::Lifecycle(Lifecycle::CommandFailed(_)) => self.awaiting = false,
            Input::Lifecycle(Lifecycle::FocusLost) => { self.awaiting = false; self.menu = None; self.prompt = None; }
            Input::Lifecycle(Lifecycle::SessionReset) => { self.awaiting = false; self.menu = None; self.prompt = None; self.pairing = None; }
            Input::Lifecycle(Lifecycle::TranscriptReplaced) => self.pairing = None,
            Input::Lifecycle(Lifecycle::EngineChanged | Lifecycle::Resync) => {
                self.awaiting = false;
                if self.link.is_some() && cx.core.interactive { self.request(Request::Status, cx); }
                if self.menu.is_some() || self.prompt.is_some() { cx.focus(FocusClass::Modal); }
            }
            _ => {}
        }
    }
    fn editor(&self) -> Option<Editor> {
        let hint = match self.prompt.as_ref()? {
            Prompt::DeviceName { name, .. } => format!("link  rename {name}  new name  (esc cancel)"),
            Prompt::Name => "link  node name  (esc cancel)".into(),
            Prompt::Revoke { name, .. } => format!("link  revoke {name}?  y/n"),
        };
        Some(Editor { hint, masked: false, submit_empty: false })
    }
    fn view(&self, at: Surface<'_>, cx: &ViewCx<'_>) -> Option<View> {
        match at {
            Surface::Header(HeaderSlot::Link) => self.link.as_ref().filter(|s| s.connected && !s.devices.is_empty()).map(|s|
                View::Text(format!("  ·  link ●{}", s.devices.iter().filter(|d| d.online).count()))),
            Surface::Modal => {
                let Some(Menu::Link { cursor }) = self.menu else { return None };
                Some(View::Element { element: crate::ui::menu_view(self.link_title(), self.link_rows(), cursor,
                    cx.width as u16, cx.height as u16, false), rows: 0, replace: false })
            }
            _ => None,
        }
    }
    fn wake(&self) -> Option<u64> { self.pairing.as_ref().map(Countdown::wake) }
}

/// A stable, owner-local countdown for one named transcript replacement group.
pub(crate) struct Countdown {
    group: &'static str,
    key: uuid::Uuid,
    pairing: ::link::Pairing,
    left: u64,
    wake: u64,
}
impl Countdown {
    pub(crate) fn append<M: Send + 'static>(group: &'static str, title: String, pairing: ::link::Pairing, cx: &mut Cx<'_, '_, M>) -> Self {
        let now = now_ms();
        let remaining = pairing.expires_at_ms.saturating_sub(now);
        let card = Self { group, key: uuid::Uuid::new_v4(), left: remaining / 1000, pairing, wake: now + remaining % 1000 + 1 };
        cx.core(CoreAction::AppendCard(TranscriptCard { entry_key: None, group, key: card.key, glyph: "⌁", color: crate::ui::FAIENCE,
            title, body: markdown::Text::complete(&card.body()), expanded: true, plain: false }));
        card
    }
    fn body(&self) -> String {
        format!("```\n{}\n```\n{}\nexpires in {}:{:02}", ::link::qr(&self.pairing.key), self.pairing.key, self.left / 60, self.left % 60)
    }
    pub(crate) fn replace<M: Send + 'static>(&self, body: String, cx: &mut Cx<'_, '_, M>) {
        cx.core(CoreAction::ReplaceCard { group: self.group, key: self.key, body });
    }
    pub(crate) fn tick<M: Send + 'static>(&mut self, now: u64, cx: &mut Cx<'_, '_, M>) -> bool {
        if !cx.core.card_current(self.group, self.key) { return false }
        self.wake = now + self.pairing.expires_at_ms.saturating_sub(now) % 1000 + 1;
        let left = self.pairing.expires_at_ms.saturating_sub(now) / 1000;
        if left != self.left {
            self.left = left;
            self.replace(if left == 0 { "pairing expired".into() } else { self.body() }, cx);
        }
        left != 0
    }
    pub(crate) fn wake(&self) -> u64 { self.wake }
}
pub(crate) fn relay_host(relay: &str) -> &str {
    let rest = relay.split_once("://").map_or(relay, |(_, r)| r);
    rest.split('/').next().unwrap_or(rest)
}
pub(crate) fn platform_name(p: &str) -> &str {
    match p { "ios" => "iOS", "macos" => "macOS", other => other }
}
/// Existing one-line status formatting.
pub fn link_line(s: &::link::LinkStatus) -> String {
    if !s.enabled { return "link off".into() }
    format!("link on · {} · {} · {} device{} ({} online)", relay_host(&s.relay),
        if s.connected { "connected" } else { "disconnected" }, s.devices.len(),
        if s.devices.len() == 1 { "" } else { "s" }, s.devices.iter().filter(|d| d.online).count())
}
