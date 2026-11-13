//! Node inventory, local aliases and shared node/mesh invite countdowns.
use super::*;
use serde::Deserialize;
use rpc_compose::api::clients::NetworkApiClient;
use crate::model::{ago, now_ms};
use super::link::{Countdown, platform_name};

/// Global inventory survives session resets; local input and invites do not.
#[derive(Default)]
pub struct Nodes {
    pub(crate) nodes: Option<Vec<::nodes::Node>>,
    pub(crate) awaiting: bool,
    pub(crate) refresh: bool,
    pub(crate) menu: Option<Menu>,
    pub(crate) prompt: Option<Prompt>,
    invite: Option<Countdown>,
}
#[derive(Debug)]
pub(crate) enum Menu { Nodes { cursor: usize } }
#[derive(Debug)]
pub(crate) enum Prompt {
    Key,
    Name { id: String, name: String },
    Remove { id: String, name: String },
}
/// Owner request carried into the RPC task.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Read inventory.
    ListNodes,
    /// Pair a key, then read inventory.
    AddNode(String, Option<String>),
    /// Resolve an exact name or unique id prefix, remove, then read inventory.
    RemoveNode(String),
    /// Resolve and rename, then read inventory.
    RenameNode(String, String),
    /// Resolve and create a phone invite.
    InviteNode(String),
}
/// Local replies and borrowed-decoded live traffic.
pub enum Msg {
    /// Mutation notice precedes either the authoritative list or its error.
    Listed {
        /// A mutation already succeeded, even if the following list failed.
        notice: Option<String>,
        /// The inventory re-read, or the unchanged generic error.
        result: Result<Vec<::nodes::Node>, String>,
    },
    /// Node display name and its pairing capability.
    Invited(Result<(String, ::link::Pairing), String>),
    /// Owner bus event.
    Event(::nodes::NodeEvent),
}
async fn resolve_node(api: &Engine, name: &str) -> Result<::nodes::Node, String> {
    let list = api.list_nodes().await.map_err(crate::reason)?;
    find_node(&list, name).cloned().ok_or_else(|| format!("no node named {name}"))
}
impl Nodes {
    fn request(&self, request: Request, cx: &mut Cx<'_, '_, Msg>) {
        cx.spawn_request(Lane::Concurrent, request, |api, request| async move {
            if let Request::InviteNode(name) = request {
                return Msg::Invited(async {
                    let node = resolve_node(&api, &name).await?;
                    let pairing = api.invite_node(node.id).await.map_err(crate::reason)?;
                    Ok((node.name, pairing))
                }.await);
            }
            let mutation = async {
                Ok(match request {
                    Request::ListNodes => None,
                    Request::AddNode(key, name) => {
                        let node = api.add_node(key, name).await.map_err(crate::reason)?;
                        Some(format!("paired node {}", node.name))
                    }
                    Request::RemoveNode(name) => {
                        let node = resolve_node(&api, &name).await?;
                        api.remove_node(node.id).await.map_err(crate::reason)?;
                        Some(format!("removed node {}", node.name))
                    }
                    Request::RenameNode(name, new_name) => {
                        let node = resolve_node(&api, &name).await?;
                        api.rename_node(node.id, new_name.clone()).await.map_err(crate::reason)?;
                        Some(format!("renamed node {} → {new_name}", node.name))
                    }
                    Request::InviteNode(_) => unreachable!(),
                })
            }.await;
            match mutation {
                Ok(notice) => Msg::Listed { notice, result: api.list_nodes().await.map_err(crate::reason) },
                Err(error) => Msg::Listed { notice: None, result: Err(error) },
            }
        });
    }
    fn slash(&mut self, args: &str, cx: &mut Cx<'_, '_, Msg>) {
        let request = match parse_nodes_arg(args) {
            Some(NodesArg::Menu) => { self.awaiting = true; Request::ListNodes }
            Some(NodesArg::Add(key)) => Request::AddNode(key, None),
            Some(NodesArg::Remove(name)) => Request::RemoveNode(name),
            Some(NodesArg::Rename(name, new_name)) => Request::RenameNode(name, new_name),
            Some(NodesArg::Invite(name)) => Request::InviteNode(name),
            None => { cx.core(CoreAction::Reject("usage: /nodes [add <key>|rm <n>|name <n> <name>|invite <n>]".into())); return; }
        };
        self.request(request, cx);
    }
    /// Existing menu title and keyboard hint.
    pub fn nodes_title(&self) -> String {
        format!("nodes · {} paired   a add (paste key)  i invite (QR for a phone)  r rename  x remove  esc", self.nodes.as_ref().map_or(0, Vec::len))
    }
    /// Existing inventory rows, including offline/revoked state.
    pub fn nodes_rows(&self) -> Vec<String> {
        let now = now_ms();
        let since = |ms: u64| ago(chrono::Duration::milliseconds(now.saturating_sub(ms) as i64));
        let rows: Vec<String> = self.nodes.iter().flatten().map(|n| {
            let tail = match (n.revoked, n.online, n.children, n.last_seen_ms) {
                (true, ..) => "revoked".to_owned(),
                (false, true, 0, _) => format!("paired {}", since(n.paired_at_ms)),
                (false, true, children, _) => format!("{children} running   paired {}", since(n.paired_at_ms)),
                (false, false, _, 0) => "never seen".to_owned(),
                (false, false, _, ms) => format!("seen {}", since(ms)),
            };
            format!("{} {:<20} {:<8} theuth {:<8} {tail}", if n.online { "●" } else { "○" },
                n.name.chars().take(20).collect::<String>(), platform_name(&n.platform), n.version)
        }).collect();
        if rows.is_empty() { return vec!["no nodes paired · a adds one from its key".into()] }
        rows
    }
    fn key(&mut self, code: KeyCode, cx: &mut Cx<'_, '_, Msg>) {
        if self.prompt.is_some() {
            if code == KeyCode::Esc {
                self.prompt = None; cx.close(); cx.consume_key(); cx.core(CoreAction::Notice("nodes: cancelled".into()));
            }
            return;
        }
        let Some(Menu::Nodes { cursor }) = &mut self.menu else { return };
        cx.consume_key();
        let list = self.nodes.as_deref().unwrap_or_default();
        let node = list.get(*cursor).map(|n| (n.id.clone(), n.name.clone()));
        match code {
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Down => *cursor = (*cursor + 1).min(list.len().saturating_sub(1)),
            KeyCode::Esc => { self.menu = None; cx.close(); }
            KeyCode::Char('a') => { self.menu = None; self.prompt = Some(Prompt::Key); }
            KeyCode::Char('i') => if let Some((id, _)) = node { self.menu = None; cx.close(); self.request(Request::InviteNode(id), cx); },
            KeyCode::Char('r') => if let Some((id, name)) = node { self.menu = None; self.prompt = Some(Prompt::Name { id, name }); },
            KeyCode::Char('x') => if let Some((id, name)) = node { self.menu = None; self.prompt = Some(Prompt::Remove { id, name }); },
            _ => {}
        }
    }
    fn submit(&mut self, line: &str, cx: &mut Cx<'_, '_, Msg>) {
        let Some(prompt) = self.prompt.take() else { return };
        cx.consume_key(); cx.close();
        let request = match prompt {
            Prompt::Key => Request::AddNode(line.to_owned(), None),
            Prompt::Name { id, .. } => Request::RenameNode(id, line.to_owned()),
            Prompt::Remove { id, name } => {
                if !line.eq_ignore_ascii_case("y") && !line.eq_ignore_ascii_case("yes") {
                    cx.core(CoreAction::Notice(format!("nodes: kept {name}"))); return;
                }
                Request::RemoveNode(id)
            }
        };
        self.awaiting = true;
        self.request(request, cx);
    }
}
impl Component for Nodes {
    type Msg = Msg;
    fn declaration(&self) -> Declaration {
        Declaration { id: "nodes", interests: &[Interest { topic: "nodes.events", version: 1, variants: &[] }],
            slash: &[("/nodes", "[add <key>|rm <n>|name <n> <name>|invite <n>]", "machines this harness can run sessions on")], bindings: &[] }
    }
    fn decode(&self, event: &TopicEvent, _: &CoreView<'_>) -> Option<Msg> {
        if event.topic().is_some() && (event.topic() != Some("nodes.events") || event.version() != Some(1)) { return None }
        ::nodes::NodeEvent::deserialize(event.event()).ok().map(Msg::Event)
    }
    fn update(&mut self, input: Input<'_, Msg>, cx: &mut Cx<'_, '_, Msg>) {
        match input {
            Input::Slash { args, .. } => self.slash(args, cx),
            Input::Key(key) => self.key(key.code, cx),
            Input::Submit(line) => self.submit(line, cx),
            Input::Message(Msg::Listed { notice, result }) => {
                if cx.core.reasoning { cx.plain(Plain::Text("\x1b[0m\n".into())); }
                if let Some(notice) = notice { cx.plain(Plain::Line(notice.clone())); cx.core(CoreAction::Notice(notice)); }
                match result {
                    Err(error) => cx.error(error),
                    Ok(nodes) => {
                        for n in &nodes {
                            cx.plain(Plain::Line(format!("{} {}  {}  theuth {}  {}", if n.online { "●" } else { "○" },
                                n.name, n.platform, n.version, if n.revoked { "revoked" } else if n.online { "online" } else { "offline" })));
                        }
                        if self.awaiting && self.menu.is_none() { self.menu = Some(Menu::Nodes { cursor: 0 }); cx.focus(FocusClass::Modal); }
                        self.awaiting = false; self.nodes = Some(nodes);
                    }
                }
            }
            Input::Message(Msg::Invited(result)) => match result {
                Err(error) => cx.error(error),
                Ok((name, pairing)) => {
                    if cx.core.reasoning { cx.plain(Plain::Text("\x1b[0m\n".into())); }
                    cx.plain(Plain::Line(format!("node invite · {name}\n{}\n{}", ::link::qr(&pairing.key), pairing.key)));
                    self.invite = Some(Countdown::append("node-invite", format!("node invite · {name}"), pairing, cx));
                }
            },
            Input::Message(Msg::Event(event)) => {
                if cx.core.reasoning { cx.plain(Plain::Text("\x1b[0m\n".into())); }
                if let ::nodes::NodeEvent::NodeAdded { node } = event { cx.plain(Plain::Line(format!("paired node {}", node.name))); }
                self.refresh = true;
            }
            Input::Tick { now_ms, .. } => {
                if std::mem::take(&mut self.refresh) && cx.core.interactive { self.request(Request::ListNodes, cx); }
                if let Some(card) = &mut self.invite && !card.tick(now_ms, cx) { self.invite = None; }
            }
            Input::Lifecycle(Lifecycle::CommandFailed(_)) => self.awaiting = false,
            Input::Lifecycle(Lifecycle::FocusLost) => { self.awaiting = false; self.menu = None; self.prompt = None; }
            Input::Lifecycle(Lifecycle::SessionReset) => { self.awaiting = false; self.menu = None; self.prompt = None; self.invite = None; }
            Input::Lifecycle(Lifecycle::TranscriptReplaced) => self.invite = None,
            Input::Lifecycle(Lifecycle::EngineChanged | Lifecycle::Resync) => {
                self.awaiting = false; self.refresh = self.nodes.is_some();
                if self.menu.is_some() || self.prompt.is_some() { cx.focus(FocusClass::Modal); }
            }
            _ => {}
        }
    }
    fn editor(&self) -> Option<Editor> {
        let hint = match self.prompt.as_ref()? {
            Prompt::Key => "nodes  add  paste the node's key  (esc cancel)".into(),
            Prompt::Name { name, .. } => format!("nodes  rename {name}  new name  (esc cancel)"),
            Prompt::Remove { name, .. } => format!("nodes  remove {name}?  y/n"),
        };
        Some(Editor { hint, masked: false, submit_empty: false })
    }
    fn view(&self, at: Surface<'_>, cx: &ViewCx<'_>) -> Option<View> {
        match at {
            Surface::Header(HeaderSlot::Nodes) => self.nodes.as_deref().filter(|nodes| !nodes.is_empty())
                .map(|nodes| View::Text(format!("  ·  nodes ●{}", nodes.iter().filter(|n| n.online).count()))),
            Surface::Modal => {
                let Some(Menu::Nodes { cursor }) = self.menu else { return None };
                Some(View::Element { element: crate::ui::menu_view(self.nodes_title(), self.nodes_rows(), cursor,
                    cx.width as u16, cx.height as u16, false), rows: 0, replace: false })
            }
            _ => None,
        }
    }
    fn wake(&self) -> Option<u64> {
        if self.refresh { Some(0) } else { self.invite.as_ref().map(Countdown::wake) }
    }
}

/// A parsed `/nodes` argument.
#[derive(Debug, PartialEq, Eq)]
pub enum NodesArg {
    /// Bare `/nodes`: open the menu.
    Menu,
    /// `add <key>`.
    Add(String),
    /// `rm <n>`.
    Remove(String),
    /// `name <n> <name…>`.
    Rename(String, String),
    /// `invite <n>`.
    Invite(String),
}
/// `/nodes` verbs; `None` for anything else (the caller prints usage).
pub fn parse_nodes_arg(arg: &str) -> Option<NodesArg> {
    let (sub, rest) = arg.split_once(char::is_whitespace).map_or((arg, ""), |(s, r)| (s, r.trim()));
    Some(match (sub, rest) {
        ("", _) => NodesArg::Menu,
        ("add", key) if !key.is_empty() => NodesArg::Add(key.to_owned()),
        ("rm", n) if !n.is_empty() => NodesArg::Remove(n.to_owned()),
        ("invite", n) if !n.is_empty() => NodesArg::Invite(n.to_owned()),
        ("name", rest) => {
            let (n, name) = rest.split_once(char::is_whitespace)?;
            NodesArg::Rename(n.to_owned(), name.trim().to_owned())
        }
        _ => return None,
    })
}
/// An exact name wins over a unique id prefix; ambiguous prefixes do not resolve.
pub fn find_node<'a>(list: &'a [::nodes::Node], n: &str) -> Option<&'a ::nodes::Node> {
    list.iter().find(|x| x.name == n).or_else(|| {
        let mut hits = list.iter().filter(|x| x.id.starts_with(n));
        match (hits.next(), hits.next()) { (Some(one), None) => Some(one), _ => None }
    })
}

#[cfg(test)]
#[test]
fn stale_list_failure_cannot_replace_newer_invite() {
    use crate::model::{CardKind, Command, Entry, Model, UiEvent};
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut model = Model::default();
    model.components = crate::components::Registry::empty();
    model.components.register(Nodes::default()).unwrap();
    model.mount_components(&tx);
    model.submit("/nodes", &tx);
    let Command::Component(effect) = rx.try_recv().unwrap() else { panic!("node effect") };
    assert_eq!(effect.request::<Request>(), Some(&Request::ListNodes));
    let token = model.components.token("nodes");
    model.apply(UiEvent::Component(LocalReply { to: token, engine_epoch: 0,
        body: Box::new(Msg::Invited(Ok(("build-box".into(), ::link::Pairing {
            key: "https://relay.example/link#1.BBBB.QQ".into(), expires_at_ms: now_ms() + 300_000,
        })))) }));
    let body = |model: &Model| match &model.entries[0] {
        Entry::Card { kind: CardKind::Component { group: "node-invite", key, .. }, title, text, .. } =>
            (*key, title.clone(), text.view().plain.to_owned() + &text.view().lines()
                .flat_map(|line| line.iter().map(|span| span.text.as_str())).collect::<String>()),
        _ => panic!("node invite card"),
    };
    let before = body(&model);
    model.apply(UiEvent::Component(LocalReply { to: token, engine_epoch: 0,
        body: Box::new(Msg::Listed { notice: None, result: Err("list failed".into()) }) }));
    assert_eq!(body(&model), before);
    assert!(model.components.get::<Nodes>().unwrap().wake().is_some());
    assert!(matches!(model.entries.last(), Some(Entry::Error(error)) if error == "list failed"));
}
