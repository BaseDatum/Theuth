use super::*;

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

impl Model {

    pub(super) fn mcp_connecting(&self) -> bool {
        self.mcp.as_ref().is_some_and(|p| !p.connecting.is_empty())
    }

    /// Bottom-right status: MCP servers connecting (with a spinner and the
    /// first names) or, once settled, `mcp 11/12`; empty with no servers.
    pub fn mcp_status(&self) -> String {
        let Some(p) = &self.mcp else {
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
        format!("{} {up}  ·  connecting {names}{more}", self.spinner())
    }
}

impl Model {

    /// `/mcp add <url|json>` | `sync`.
    pub(super) fn mcp_cmd(&mut self, arg: &str, tx: &UnboundedSender<Command>) {
        let (sub, rest) = arg
            .split_once(' ')
            .map_or((arg, ""), |(s, r)| (s, r.trim()));
        match (sub, rest) {
            ("sync", _) => send(tx, Command::SyncMcp),
            ("add", src) if git_url(src) => send(tx, Command::AddMcpRepo(src.to_owned())),
            ("add", src) if !src.is_empty() => match mcp_entry(src) {
                Ok((name, entry)) => send(tx, Command::AddMcp { name, entry }),
                Err(e) => self.entries.push(Entry::Error(format!("/mcp add: {e}"))),
            },
            _ => self
                .entries
                .push(Entry::Error("usage: /mcp [add <url|json> | sync]".into())),
        }
    }
}

impl Model {

    /// `/mcp` menu rows: every server, then the tools of expanded ones.
    pub fn mcp_rows(&self) -> Vec<Row<'_>> {
        let Some(Menu::Mcp {
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

impl Model {

    pub(super) fn mcp_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        enum Act {
            Move(usize),
            Expand(String, bool),
            Send(Box<Command>),
            Pending(Option<Target>),
            Close,
            Nothing,
        }
        let Some(Menu::Mcp {
            cursor, pending, ..
        }) = &self.menu
        else {
            return MenuKey::Consumed;
        };
        let session = self.session;
        let rows = self.mcp_rows();
        let cursor = (*cursor).min(rows.len().saturating_sub(1));
        let row = rows.get(cursor);
        let set_gate = |row: &Row<'_>, gate: Option<Gate>| {
            let t = row.target();
            Act::Send(Box::new(Command::SetGate {
                server: t.server,
                tool: t.tool,
                gate,
                session,
            }))
        };
        let draft = self.draft_options();
        let set_hidden = |row: &Row<'_>, hidden: bool| {
            Act::Send(Box::new(Command::SetHidden {
                session,
                tools: row.tool_names(),
                hidden,
                draft: draft.clone(),
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
                (KeyCode::Char('s'), _) => Act::Send(Box::new(Command::SyncMcp)),
                (KeyCode::Right, Some(Row::Server(s))) => Act::Expand(s.name.clone(), true),
                (KeyCode::Left, Some(Row::Server(s) | Row::Tool(s, _))) => {
                    Act::Expand(s.name.clone(), false)
                }
                (KeyCode::Char('l'), Some(Row::Server(s))) => match s.auth {
                    McpAuth::Required => Act::Send(Box::new(Command::Login(s.name.clone()))),
                    McpAuth::LoggedIn => Act::Send(Box::new(Command::Logout(s.name.clone()))),
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
        let Some(Menu::Mcp {
            servers,
            cursor: cur,
            expanded,
            pending,
        }) = &mut self.menu
        else {
            return MenuKey::Consumed;
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
        MenuKey::Consumed
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
