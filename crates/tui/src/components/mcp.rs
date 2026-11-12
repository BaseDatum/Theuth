//! MCP inventory, session visibility, risk policy and connection status.
use super::*;
use std::collections::BTreeSet;
use serde::Deserialize;
use tools::{Gate, McpAuth, McpProgress, McpServer, McpToolInfo, PolicyAction, RepoInfo};
use tools::git::SyncResult;
use rpc_compose::api::clients::*;

#[derive(Default)]
pub(crate) struct Mcp {
    pub(crate) menu: Option<Menu>,
    awaiting: bool,
    progress: Option<McpProgress>,
    refresh: bool,
}
#[derive(Debug)]
pub(crate) struct Menu {
    pub(crate) servers: Vec<McpServer>,
    pub(crate) cursor: usize,
    pub(crate) expanded: BTreeSet<String>,
    pub(crate) pending: Option<Target>,
}
#[derive(Clone, Debug)]
pub(crate) enum Request {
    ListMcp(Option<SessionId>), McpStatus,
    SetGate { server: String, tool: Option<String>, gate: Option<Gate>, session: Option<SessionId> },
    SetHidden { session: Option<SessionId>, tools: Vec<String>, hidden: bool },
    AddMcp { name: String, entry: serde_json::Value }, AddMcpRepo(String), SyncMcp,
    Login(String), Logout(String),
}
pub(crate) enum Msg {
    Mcp(Result<Vec<McpServer>, String>),
    Progress(Result<McpProgress, String>),
    Added(Result<McpServer, String>), Repo(Result<RepoInfo, String>),
    Synced(Result<Vec<SyncResult>, String>), Info(Result<String, String>),
}
fn send(cx: &mut Cx<'_, '_, Msg>, request: Request) {
    if matches!(request, Request::SetHidden { .. }) {
        cx.ensure_session_request(request, |api, adopted, request| async move {
            let Request::SetHidden { session, tools, hidden } = request else { unreachable!() };
            let session = session.unwrap_or(adopted);
            let result = async {
                api.set_mcp_hidden(session, tools, hidden).await.map_err(crate::reason)?;
                api.list_mcp(Some(session)).await.map_err(crate::reason)
            }.await;
            Msg::Mcp(result)
        }, |error| Msg::Mcp(Err(error)));
        return;
    }
    cx.spawn_request(Lane::Concurrent, request, |api, request| async move {
        match request {
            Request::ListMcp(session) => Msg::Mcp(api.list_mcp(session).await.map_err(crate::reason)),
            Request::McpStatus => Msg::Progress(api.mcp_status().await.map_err(crate::reason)),
            Request::SetGate { server, tool, gate, session } => Msg::Mcp(async {
                api.set_mcp_gate(server, tool, gate).await.map_err(crate::reason)?;
                api.list_mcp(session).await.map_err(crate::reason)
            }.await),
            Request::AddMcp { name, entry } => Msg::Added(api.add_mcp_server(name, entry).await.map_err(crate::reason)),
            Request::AddMcpRepo(url) => Msg::Repo(api.add_mcp_repo(url, None).await.map_err(crate::reason)),
            Request::SyncMcp => Msg::Synced(api.sync_mcp().await.map_err(crate::reason)),
            Request::Login(name) => Msg::Info(async {
                let url = if oauth::provider(&name).is_some() { api.login(name).await } else { api.mcp_login(name).await }.map_err(crate::reason)?;
                Ok(format!("login: open {url}"))
            }.await),
            Request::Logout(name) => Msg::Info(async {
                if oauth::provider(&name).is_some() { api.logout(name.clone()).await } else { api.mcp_logout(name.clone()).await }.map_err(crate::reason)?;
                Ok(format!("logged out of {name}"))
            }.await),
            Request::SetHidden { .. } => unreachable!(),
        }
    });
}
/// One line of the `/mcp` menu.
#[derive(Debug)]
pub enum Row<'a> {
    /// A server (its line applies to all its tools).
    Server(&'a McpServer),
    /// A tool of an expanded server.
    Tool(&'a McpServer, &'a McpToolInfo),
}

/// What a `/mcp` switch applies to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    /// Server name.
    pub server: String,
    /// Tool name; `None` = the whole server.
    pub tool: Option<String>,
}

/// Whether a `/mcp` row is on, and if not, which switch turned it off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Switch {
    /// Visible to the model.
    On,
    /// Hidden for this session (`set_mcp_hidden`); other sessions still see it.
    OffSession,
    /// `off` in `mcp/config.yaml`: every session, every restart.
    OffAlways,
}

impl Switch {
    /// Column text.
    pub fn label(self) -> &'static str {
        match self {
            Switch::On => "on",
            Switch::OffSession => "off · this session",
            Switch::OffAlways => "off · always",
        }
    }
}

impl Row<'_> {
    /// The row's switch. A server is `OffSession` when every tool it has
    /// registered is hidden.
    pub fn switch(&self) -> Switch {
        match self {
            Row::Server(s) if s.gate == Some(Gate::Off) => Switch::OffAlways,
            Row::Server(s) => {
                let mut live = s.tools.iter().filter(|t| t.registered.is_some());
                if live.clone().next().is_some() && live.all(|t| t.hidden) {
                    Switch::OffSession
                } else {
                    Switch::On
                }
            }
            Row::Tool(_, t) if t.gate == Some(Gate::Off) => Switch::OffAlways,
            Row::Tool(_, t) if t.hidden => Switch::OffSession,
            _ => Switch::On,
        }
    }

    /// Registered names a session switch covers (a clashing tool registers as
    /// `<server>__<name>`, so this only ever touches this server's own slots).
    fn tool_names(&self) -> Vec<String> {
        match self {
            Row::Server(s) => s
                .tools
                .iter()
                .filter_map(|t| t.registered.clone())
                .collect(),
            Row::Tool(_, t) => t.registered.clone().into_iter().collect(),
        }
    }

    fn target(&self) -> Target {
        match self {
            Row::Server(s) => Target {
                server: s.name.clone(),
                tool: None,
            },
            Row::Tool(s, t) => Target {
                server: s.name.clone(),
                tool: Some(t.name.clone()),
            },
        }
    }

    /// The row's own risk line.
    fn gate(&self) -> Option<Gate> {
        match self {
            Row::Server(s) => s.gate,
            Row::Tool(_, t) => t.gate,
        }
    }
}

/// Lowercase yaml name of a policy action.
pub fn action_name(a: PolicyAction) -> &'static str {
    match a {
        PolicyAction::Allow => "allow",
        PolicyAction::Ask => "ask",
        PolicyAction::Deny => "deny",
    }
}

/// Lowercase yaml name of a `mcp/config.yaml` line.
pub fn gate_name(g: Gate) -> &'static str {
    match g {
        Gate::ReadOnly => "read_only",
        Gate::Mutating => "mutating",
        Gate::Destructive => "destructive",
        Gate::Off => "off",
    }
}

/// Space in `/mcp`: annotations → read_only → mutating → destructive → annotations (12 §5).
/// `off` is not a risk; ⏎ switches it.
fn next_gate(g: Option<Gate>) -> Option<Gate> {
    match g {
        None => Some(Gate::ReadOnly),
        Some(Gate::ReadOnly) => Some(Gate::Mutating),
        Some(Gate::Mutating) => Some(Gate::Destructive),
        Some(Gate::Destructive | Gate::Off) => None,
    }
}

impl Mcp {

    pub(crate) fn mcp_connecting(&self) -> bool {
        self.progress.as_ref().is_some_and(|p| !p.connecting.is_empty())
    }

    /// Bottom-right status: MCP servers connecting (with a spinner and the
    /// first names) or, once settled, `mcp 11/12`; empty with no servers.
    fn mcp_status(&self, spinner: &str) -> String {
        let Some(p) = &self.progress else {
            return String::new();
        };
        let total = p.connecting.len() + p.connected.len() + p.failed.len();
        if total == 0 {
            return String::new();
        }
        let up = format!("mcp {}/{total}", p.connected.len());
        if p.connecting.is_empty() {
            return match p.failed.len() {
                0 => up,
                n => format!("{up}  ·  {n} failed"),
            };
        }
        let names = p
            .connecting
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        let more = match p.connecting.len() {
            n if n > 3 => format!(" (+{})", n - 3),
            _ => String::new(),
        };
        format!("{} {up}  ·  connecting {names}{more}", spinner)
    }
}

impl Mcp {

    /// `/mcp add <url|json>` | `sync`.
    pub(crate) fn mcp_cmd(&mut self, arg: &str, tx: &mut Cx<'_, '_, Msg>) {
        let (sub, rest) = arg
            .split_once(' ')
            .map_or((arg, ""), |(s, r)| (s, r.trim()));
        match (sub, rest) {
            ("sync", _) => send(tx, Request::SyncMcp),
            ("add", src) if git_url(src) => send(tx, Request::AddMcpRepo(src.to_owned())),
            ("add", src) if !src.is_empty() => match mcp_entry(src) {
                Ok((name, entry)) => send(tx, Request::AddMcp { name, entry }),
                Err(e) => tx.core(CoreAction::Reject(format!("/mcp add: {e}"))),
            },
            _ => tx.core(CoreAction::Reject("usage: /mcp [add <url|json> | sync]".into())),
        }
    }
}

impl Mcp {

    /// `/mcp` menu rows: every server, then the tools of expanded ones.
    pub fn mcp_rows(&self) -> Vec<Row<'_>> {
        let Some(Menu {
            servers, expanded, ..
        }) = &self.menu
        else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        for s in servers {
            rows.push(Row::Server(s));
            if expanded.contains(&s.name) {
                rows.extend(s.tools.iter().map(|t| Row::Tool(s, t)));
            }
        }
        rows
    }
}

impl Mcp {

    pub(crate) fn mcp_key(&mut self, code: KeyCode, tx: &mut Cx<'_, '_, Msg>) {
        enum Act {
            Move(usize),
            Expand(String, bool),
            Send(Box<Request>),
            Pending(Option<Target>),
            Close,
            Nothing,
        }
        let Some(Menu {
            cursor, pending, ..
        }) = &self.menu
        else {
            return;
        };
        let session = tx.core.session;
        let rows = self.mcp_rows();
        let cursor = (*cursor).min(rows.len().saturating_sub(1));
        let row = rows.get(cursor);
        let set_gate = |row: &Row<'_>, gate: Option<Gate>| {
            let t = row.target();
            Act::Send(Box::new(Request::SetGate {
                server: t.server,
                tool: t.tool,
                gate,
                session,
            }))
        };
        let set_hidden = |row: &Row<'_>, hidden: bool| {
            Act::Send(Box::new(Request::SetHidden {
                session,
                tools: row.tool_names(),
                hidden,
            }))
        };
        // The scope prompt after ⏎ on an `on` row: the target is the row the
        // cursor was on, re-found by name (a refresh may have moved it).
        let act = if let Some(target) = pending {
            let row = rows.iter().find(|r| r.target() == *target);
            match (code, row) {
                (KeyCode::Esc, _) => Act::Pending(None),
                (KeyCode::Char('s') | KeyCode::Enter, Some(r)) => set_hidden(r, true),
                (KeyCode::Char('a'), Some(r)) => set_gate(r, Some(Gate::Off)),
                (_, None) => Act::Pending(None),
                _ => Act::Nothing,
            }
        } else {
            match (code, row) {
                (KeyCode::Up, _) => Act::Move(cursor.saturating_sub(1)),
                (KeyCode::Down, _) => Act::Move((cursor + 1).min(rows.len().saturating_sub(1))),
                (KeyCode::Esc, _) => Act::Close,
                (KeyCode::Char('s'), _) => Act::Send(Box::new(Request::SyncMcp)),
                (KeyCode::Right, Some(Row::Server(s))) => Act::Expand(s.name.clone(), true),
                (KeyCode::Left, Some(Row::Server(s) | Row::Tool(s, _))) => {
                    Act::Expand(s.name.clone(), false)
                }
                (KeyCode::Char('l'), Some(Row::Server(s))) => match s.auth {
                    McpAuth::Required => Act::Send(Box::new(Request::Login(s.name.clone()))),
                    McpAuth::LoggedIn => Act::Send(Box::new(Request::Logout(s.name.clone()))),
                    McpAuth::None => Act::Nothing,
                },
                // ⏎: turn on undoes whichever switch is off; turn off asks for the scope.
                (KeyCode::Enter, Some(r)) => match r.switch() {
                    Switch::OffAlways => set_gate(r, None),
                    Switch::OffSession => set_hidden(r, false),
                    Switch::On => Act::Pending(Some(r.target())),
                },
                (KeyCode::Char(' ') | KeyCode::Backspace, Some(r))
                    if r.switch() == Switch::OffAlways =>
                {
                    Act::Nothing
                }
                (KeyCode::Char(' '), Some(r)) => set_gate(r, next_gate(r.gate())),
                (KeyCode::Backspace, Some(r)) => set_gate(r, None),
                _ => Act::Nothing,
            }
        };
        let Some(Menu {
            servers,
            cursor: cur,
            expanded,
            pending,
        }) = &mut self.menu
        else {
            return;
        };
        *cur = cursor;
        match act {
            Act::Move(to) => *cur = to,
            Act::Expand(name, true) => {
                expanded.insert(name);
            }
            Act::Expand(name, false) => {
                expanded.remove(&name);
                // Land on the collapsed server's own row.
                *cur = servers.iter().position(|s| s.name == name).map_or(0, |i| {
                    servers[..i]
                        .iter()
                        .filter(|s| expanded.contains(&s.name))
                        .map(|s| s.tools.len())
                        .sum::<usize>()
                        + i
                });
            }
            Act::Send(cmd) => {
                *pending = None;
                send(tx, *cmd);
            }
            Act::Pending(p) => *pending = p,
            Act::Close => self.menu = None,
            Act::Nothing => {}
        }
    }
}

/// `/mcp add` argument that means "clone a repo": a `.git` URL or a known git host.
pub fn git_url(s: &str) -> bool {
    s.ends_with(".git")
        || ["github.com/", "gitlab.com/", "codeberg.org/"]
            .iter()
            .any(|h| s.contains(h))
}

/// Parse one MCP server from `/mcp add <json>`: a server object with a
/// `name` key (stripped), `{ "<name>": {...} }`, or `{ "mcpServers": { "<name>": {...} } }`.
pub fn mcp_entry(text: &str) -> Result<(String, serde_json::Value), String> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let serde_json::Value::Object(mut obj) = v else {
        return Err("expected a JSON object".into());
    };
    if let Some(serde_json::Value::Object(servers)) = obj.remove("mcpServers") {
        obj = servers;
    }
    if obj.contains_key("command") || obj.contains_key("url") {
        return match obj.remove("name") {
            Some(serde_json::Value::String(name)) => Ok((name, serde_json::Value::Object(obj))),
            _ => Err("server object needs a \"name\" field".into()),
        };
    }
    match (obj.len(), obj.into_iter().next()) {
        (1, Some((name, entry))) if entry.is_object() => Ok((name, entry)),
        _ => Err("expected exactly one server: {\"<name>\": {\"command\": ...}}".into()),
    }
}

impl Component for Mcp {
    type Msg = Msg;
    fn declaration(&self) -> Declaration {
        Declaration {
            id: "mcp",
            interests: &[Interest { topic: "tools.mcp-progress", version: 1, variants: &["McpProgress"] }],
            slash: &[
                ("/mcp", "[add <url|json>|sync]", "MCP servers: on/off (this session or always), risk lines"),
                ("/tools", "[add <url|json>|sync]", "same as /mcp"),
            ],
            bindings: &[],
        }
    }
    fn decode(&self, event: &TopicEvent, _: &CoreView<'_>) -> Option<Msg> {
        if event.topic().is_some() && (event.topic() != Some("tools.mcp-progress") || event.version() != Some(1)) {
            return None;
        }
        if event.event()["type"] != "McpProgress" { return None; }
        McpProgress::deserialize(event.event()).ok().map(|progress| Msg::Progress(Ok(progress)))
    }
    fn update(&mut self, input: Input<'_, Msg>, cx: &mut Cx<'_, '_, Msg>) {
        if cx.core.reasoning && matches!(&input, Input::Message(Msg::Mcp(Ok(_)) | Msg::Progress(Ok(_)) | Msg::Added(Ok(_)) | Msg::Repo(Ok(_)) | Msg::Synced(Ok(_)) | Msg::Info(Ok(_)))) {
            cx.plain(Plain::Text("\x1b[0m\n".into()));
        }
        match input {
            Input::Lifecycle(Lifecycle::Mount | Lifecycle::EngineChanged | Lifecycle::Resync) => send(cx, Request::McpStatus),
            Input::Slash { args: "", .. } => { self.awaiting = true; send(cx, Request::ListMcp(cx.core.session)); }
            Input::Slash { args, .. } => self.mcp_cmd(args, cx),
            Input::Key(key) if self.menu.is_some() => {
                self.mcp_key(key.code, cx);
                cx.consume_key();
                if self.menu.is_none() { cx.close(); }
            }
            Input::Message(Msg::Mcp(Ok(list))) => {
                cx.plain(Plain::TtyRequired("menus need a TTY".into()));
                match &mut self.menu {
                    Some(Menu { servers, .. }) => *servers = list,
                    _ if self.awaiting => {
                        self.awaiting = false;
                        self.menu = Some(Menu { servers: list, cursor: 0, expanded: BTreeSet::new(), pending: None });
                        cx.focus(FocusClass::Modal);
                    }
                    _ => {}
                }
            }
            Input::Message(Msg::Progress(Ok(progress))) => {
                if progress.connecting.is_empty() {
                    cx.plain(Plain::Line(format!("mcp: {} connected{}", progress.connected.len(), match progress.failed.as_slice() {
                        [] => String::new(), failed => format!(", failed: {}", failed.join(", ")),
                    })));
                }
                if self.progress.as_ref() != Some(&progress) && self.menu.is_some() { self.refresh = true; }
                self.progress = Some(progress);
            }
            Input::Message(Msg::Added(Ok(server))) => {
                cx.plain(Plain::Line(format!("{}  {} tools", server.name, server.tools.len())));
                cx.core(CoreAction::Notice(format!(
                    "{}  {} tools  {}", server.name, server.tools.len(), if server.connected { "connected" } else { "not connected" }
                )));
            }
            Input::Message(Msg::Repo(Ok(repo))) => {
                cx.plain(Plain::Line(format!("cloned {} → {}\n{}", repo.name, repo.path, repo.readme)));
                cx.core(CoreAction::Notice(format!(
                    "cloned {} → {}  (build it, then `/mcp add {{...\"cwd\": ...}}`)", repo.name, repo.path
                )));
            }
            Input::Message(Msg::Synced(Ok(results))) => {
                if results.is_empty() { cx.core(CoreAction::Notice("no repos".into())); }
                for result in results {
                    let line = format!("{}  {}", result.name, result.status);
                    cx.plain(Plain::Line(line.clone()));
                    cx.core(CoreAction::Notice(line));
                }
            }
            Input::Message(Msg::Info(Ok(text))) => {
                cx.plain(Plain::Line(text.clone())); cx.core(CoreAction::Notice(text));
            }
            Input::Message(Msg::Mcp(Err(error)) | Msg::Progress(Err(error)) | Msg::Added(Err(error)) | Msg::Repo(Err(error)) | Msg::Synced(Err(error)) | Msg::Info(Err(error))) => cx.error(error),
            Input::Tick { .. } if std::mem::take(&mut self.refresh) => send(cx, Request::ListMcp(cx.core.session)),
            Input::Lifecycle(Lifecycle::FocusLost) => self.menu = None,
            Input::Lifecycle(Lifecycle::SessionReset) => { *self = Self::default(); cx.close(); }
            _ => {}
        }
    }
    fn view(&self, at: Surface<'_>, cx: &ViewCx<'_>) -> Option<View> {
        match at {
            Surface::Status(StatusSlot::Right) => Some(View::Text(self.mcp_status(cx.spinner()))),
            Surface::Modal => {
                let Menu { cursor, expanded, pending, .. } = self.menu.as_ref()?;
                let title = match pending {
                    Some(t) => format!(
                        "turn off {}{}?   press  s = this session only  ·  a = always (all sessions, saved to mcp/config.yaml)  ·  esc = cancel",
                        t.server, t.tool.as_deref().map_or(String::new(), |n| format!(" › {n}")),
                    ),
                    None => "mcp  ⏎ turn on/off (asks: this session or always)  ␣ cycle risk  ⌫ reset risk  →/← tools  l login/logout  s sync  esc close".to_owned(),
                };
                let rows: Vec<_> = self.mcp_rows().iter().map(|r| match r {
                    Row::Server(s) => format!(
                        "{} {:<16} {:>2} tools   {:<20} {:<18} {}{}",
                        if expanded.contains(&s.name) { "▾" } else { "▸" }, s.name,
                        if s.connected { s.tools.len().to_string() } else { "–".to_owned() },
                        r.switch().label(),
                        match s.gate {
                            None | Some(Gate::Off) => "risk: annotations".to_owned(),
                            Some(g) => format!("risk: {}", gate_name(g)),
                        },
                        match (r.switch(), s.connected, s.auth) {
                            (Switch::OffAlways, ..) => "○ off", (_, true, _) => "● connected",
                            (_, false, McpAuth::Required) => "○ login required", (_, false, _) => "○ not connected",
                        },
                        s.repo.as_deref().map_or_else(String::new, |r| format!("   ⟳ {r}")),
                    ),
                    Row::Tool(_, t) => format!(
                        "    {:<24} {:<20} {:<22}{}", t.name, r.switch().label(),
                        match t.gate {
                            Some(Gate::Off) => String::new(),
                            Some(_) => format!("{}* ({})", crate::ui::risk_name(t.risk), action_name(t.action)),
                            None => format!("{} ({})", crate::ui::risk_name(t.risk), action_name(t.action)),
                        },
                        t.registered.as_deref().filter(|n| *n != t.name).map_or_else(String::new, |n| format!("  as {n}")),
                    ),
                }).collect();
                Some(View::Element { rows: rows.len().min(10) + 3, replace: false,
                    element: crate::ui::menu_view(title, rows, *cursor, cx.width as u16, cx.height as u16, false) })
            }
            _ => None,
        }
    }
    fn animating(&self) -> bool { self.mcp_connecting() }
}

#[cfg(test)]
#[tokio::test]
async fn close_reopen_pending_hidden_mutation_relists_original_target_without_second_draft() {
    use crate::model::{Command, Model, UiEvent};
    type Calls = Arc<parking_lot::Mutex<Vec<(String, SessionId)>>>;
    fn inventory(hidden: bool) -> McpServer {
        McpServer {
            name: "fs".into(), connected: true, repo: None, auth: McpAuth::None,
            home: None, home_name: None, home_here: true, everywhere: false,
            state: tools::McpState::Running, reason: None, gate: None,
            tools: vec![McpToolInfo {
                name: "read".into(), description: String::new(), gate: None,
                risk: tools::Risk::Mutating, action: PolicyAction::Ask,
                registered: Some("read".into()), hidden,
            }],
        }
    }
    fn module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
        let mut module = jsonrpsee::RpcModule::new(server);
        module.register_method("set_mcp_hidden", |params, server, _| {
            let (session, tools, hidden): (SessionId, Vec<String>, bool) = params.parse()?;
            assert_eq!(tools, ["read"]);
            assert!(hidden);
            server.rt.extension::<Calls>(Default::default).lock().push(("hide".into(), session));
            Ok::<_, jsonrpsee::types::ErrorObjectOwned>(())
        }).unwrap();
        module.register_method("list_mcp", |params, server, _| {
            let (session,): (Option<SessionId>,) = params.parse()?;
            server.rt.extension::<Calls>(Default::default).lock().push(("list".into(), session.unwrap()));
            Ok::<_, jsonrpsee::types::ErrorObjectOwned>(vec![inventory(true)])
        }).unwrap();
        module
    }
    fn inject(model: &mut Model, message: Msg) {
        model.apply(UiEvent::Component(LocalReply {
            to: model.components.token("mcp"), engine_epoch: 0, body: Box::new(message),
        }));
    }
    fn key(model: &mut Model, code: KeyCode) {
        model.component_key(FocusClass::Modal, &KeyEvent::new(iocraft::KeyEventKind::Press, code));
    }
    let (tx, mut commands) = tokio::sync::mpsc::unbounded_channel();
    let mut model = Model::new(String::new(), true);
    model.submit("/mcp", &tx);
    assert!(matches!(commands.try_recv(), Ok(Command::Component(effect)) if matches!(effect.request::<Request>(), Some(Request::ListMcp(None)))));
    inject(&mut model, Msg::Mcp(Ok(vec![inventory(false)])));
    key(&mut model, KeyCode::Enter);
    key(&mut model, KeyCode::Char('s'));
    assert!(matches!(commands.try_recv(), Ok(Command::CreateDraft(_))));
    key(&mut model, KeyCode::Esc);
    assert!(model.components.get::<Mcp>().unwrap().menu.is_none());
    model.submit("/mcp", &tx);
    assert!(matches!(commands.try_recv(), Ok(Command::Component(effect)) if matches!(effect.request::<Request>(), Some(Request::ListMcp(None)))));
    assert!(commands.try_recv().is_err());
    let target = SessionId::new();
    model.apply(UiEvent::SessionCreated(target));
    let Command::Component(effect) = commands.try_recv().unwrap() else { panic!("hidden mutation") };
    assert!(matches!(effect.request::<Request>(), Some(Request::SetHidden { session: None, hidden: true, .. })));
    let rt = runtime::Runtime::new(Default::default(), std::env::temp_dir());
    let calls = rt.extension::<Calls>(Default::default);
    let engine = Arc::new(Engine::Local(rpc::client::Local::new(rt.clone(), module)));
    model.session = Some(SessionId::new());
    model.apply(super::execute(effect, engine).await);
    assert_eq!(*calls.lock(), [("hide".into(), target), ("list".into(), target)]);
    assert_eq!(model.components.get::<Mcp>().unwrap().mcp_rows()[0].switch(), Switch::OffSession);
    assert!(commands.try_recv().is_err());
    rt.shutdown.cancel();
}
