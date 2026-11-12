use super::*;

/// A `/link pair` or `/nodes` invite card that is still counting down.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PairingCard {
    /// The pairing string (rendered as the QR and on one line).
    key: String,
    /// Unix ms.
    expires_at_ms: u64,
    /// Whole seconds the card last showed; redraw when it changes.
    left: u64,
}

impl PairingCard {
    /// A card over `p`, showing its whole seconds left as of now.
    pub(super) fn new(p: link::Pairing) -> Self {
        Self {
            left: p.expires_at_ms.saturating_sub(now_ms()) / 1000,
            key: p.key,
            expires_at_ms: p.expires_at_ms,
        }
    }

    /// Seconds until expiry, floored at zero.
    fn remaining(&self) -> u64 {
        self.expires_at_ms.saturating_sub(now_ms()) / 1000
    }

    /// Card body: the QR in a fence, the key, the countdown.
    pub(super) fn body(&self) -> String {
        let left = self.left;
        format!(
            "```\n{}\n```\n{}\nexpires in {}:{:02}",
            link::qr(&self.key),
            self.key,
            left / 60,
            left % 60
        )
    }
}

/// Redraw the live countdown card in `slot` when its countdown moved a
/// second; at zero it reads `pairing expired` and the slot empties.
pub(super) fn tick_card(entries: &mut [Entry], slot: &mut Option<PairingCard>, kind: CardKind) {
    let Some(p) = slot else { return };
    let left = p.remaining();
    if left == p.left {
        return;
    }
    p.left = left;
    let body = if left == 0 {
        "pairing expired".to_owned()
    } else {
        p.body()
    };
    if left == 0 {
        *slot = None;
    }
    set_card(entries, kind, &body);
}

/// Replace the body of the newest card of `kind`, if any.
fn set_card(entries: &mut [Entry], kind: CardKind, body: &str) {
    if let Some(Entry::Card { text, .. }) = entries
        .iter_mut()
        .rev()
        .find(|e| matches!(e, Entry::Card { kind: k, .. } if *k == kind))
    {
        **text = markdown::Text::complete(body);
    }
}

impl Model {

    /// Replace the body of the newest `/link pair` card, if any.
    pub(super) fn set_link_card(&mut self, body: &str) {
        set_card(&mut self.entries, CardKind::Link, body);
    }
}

impl Model {

    /// Fold a link bus event into the local status and the pairing card.
    /// What the copy cannot absorb (a device it has not seen) re-asks.
    pub(super) fn apply_link(&mut self, ev: link::LinkEvent) {
        use link::LinkEvent::*;
        match ev {
            LinkRelay { connected } => match &mut self.link {
                Some(s) => s.connected = connected,
                None => self.refresh_link = true,
            },
            LinkPairing { key: None, .. } => {
                if self.pairing.take().is_some() {
                    self.set_link_card("pairing expired");
                }
                if let Some(s) = &mut self.link {
                    s.pairing = None;
                }
            }
            LinkPairing {
                key: Some(key),
                expires_at_ms,
            } => {
                if let Some(s) = &mut self.link {
                    s.pairing = Some(link::Pairing {
                        key,
                        expires_at_ms: expires_at_ms.unwrap_or(0),
                    });
                }
            }
            LinkPaired { device } => {
                if self.pairing.take().is_some() {
                    self.set_link_card(&format!("paired: {}", device.name));
                }
                match &mut self.link {
                    Some(s) => {
                        s.pairing = None;
                        s.devices.retain(|d| d.id != device.id);
                        s.devices.push(device);
                    }
                    None => self.refresh_link = true,
                }
            }
            LinkDevice { id, online } => {
                match self
                    .link
                    .as_mut()
                    .and_then(|s| s.devices.iter_mut().find(|d| d.id == id))
                {
                    Some(d) => {
                        d.online = online;
                        if !online {
                            d.last_seen_ms = now_ms();
                        }
                    }
                    None => self.refresh_link = true,
                }
            }
            LinkRevoked { id } => {
                if let Some(s) = &mut self.link {
                    s.devices.retain(|d| d.id != id);
                }
            }
            // 69: the mesh identity's uplink and its replicated registry; the
            // status snapshot carries both, so re-ask.
            MeshRelay { .. } | DevicePaired { .. } | DevicesChanged => self.refresh_link = true,
        }
    }
}

impl Model {

    /// `/link` menu | `on` | `off` | `pair` | `cancel` | `rm <id>` |
    /// `name <id> <name…>` | `relay <url>` (31 §6.5).
    pub(super) fn link_cmd(&mut self, arg: &str, tx: &UnboundedSender<Command>) {
        let (sub, rest) = arg
            .split_once(char::is_whitespace)
            .map_or((arg, ""), |(s, r)| (s, r.trim()));
        match (sub, rest) {
            ("", _) => {
                self.awaiting_link = true;
                send(tx, Command::LinkStatus);
            }
            ("on", _) => {
                self.awaiting_link = true;
                send(tx, Command::LinkEnable(true));
            }
            ("off", _) => send(tx, Command::LinkEnable(false)),
            ("pair", _) => send(tx, Command::LinkPair),
            ("cancel", _) => send(tx, Command::LinkCancelPair),
            ("rm", id) if !id.is_empty() => send(tx, Command::LinkRevoke(id.to_owned())),
            ("name", rest) if rest.contains(char::is_whitespace) => {
                let (id, name) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
                send(
                    tx,
                    Command::LinkRenameDevice(id.to_owned(), name.trim().to_owned()),
                );
            }
            ("relay", url) if !url.is_empty() => send(tx, Command::LinkRelay(url.to_owned())),
            _ => self.entries.push(Entry::Error(
                "usage: /link [on|off|pair|cancel|rm <id>|name <id> <name>|relay <url>]".into(),
            )),
        }
    }


    /// `/mesh devices [list|add [name]|rm <id>|name <id> <name…>]` (69 §7), the CLI's `t mesh devices`.
    pub(super) fn mesh_cmd(&mut self, arg: &str, tx: &UnboundedSender<Command>) {
        const USAGE: &str = "usage: /mesh devices [list|add [name]|rm <id>|name <id> <name>]";
        let (sub, rest) = arg
            .split_once(char::is_whitespace)
            .map_or((arg, ""), |(s, r)| (s, r.trim()));
        if sub != "devices" {
            self.entries.push(Entry::Error(USAGE.into()));
            return;
        }
        let (verb, rest) = rest
            .split_once(char::is_whitespace)
            .map_or((rest, ""), |(s, r)| (s, r.trim()));
        match (verb, rest) {
            ("" | "list", _) => send(tx, Command::MeshDevicesList),
            ("add", name) => send(
                tx,
                Command::MeshDevicesAdd((!name.is_empty()).then(|| name.to_owned())),
            ),
            ("rm", id) if !id.is_empty() => send(tx, Command::MeshDevicesRevoke(id.to_owned())),
            ("name", rest) if rest.contains(char::is_whitespace) => {
                let (id, name) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
                send(
                    tx,
                    Command::MeshDevicesRename(id.to_owned(), name.trim().to_owned()),
                );
            }
            _ => self.entries.push(Entry::Error(USAGE.into())),
        }
    }

    /// `/link` menu title: the status line, then the keys.
    pub fn link_title(&self) -> String {
        let Some(s) = &self.link else {
            return "link".into();
        };
        let state = match (s.enabled, s.connected) {
            (false, _) => "off",
            (true, true) => "connected",
            (true, false) => "disconnected",
        };
        let id: String = s
            .harness_id
            .as_deref()
            .map_or_else(|| "no identity".into(), |id| id.chars().take(8).collect());
        format!(
            "link · {} · {state} · {} ({id}…)   p pair  r rename  x revoke  n node name  e {}  esc",
            relay_host(&s.relay),
            s.name,
            if s.enabled { "disable" } else { "enable" }
        )
    }

    /// `/link` menu rows: `● name  platform  paired <ago>  online|seen <ago>`
    /// per device, or one hint row when none is paired.
    pub fn link_rows(&self) -> Vec<String> {
        let now = now_ms();
        let since = |ms: u64| ago(chrono::Duration::milliseconds(now.saturating_sub(ms) as i64));
        let rows: Vec<String> = self
            .link
            .iter()
            .flat_map(|s| &s.devices)
            .map(|d| {
                let seen = match (d.online, d.last_seen_ms) {
                    (true, _) => "online".to_owned(),
                    (false, 0) => "never seen".to_owned(),
                    (false, ms) => format!("seen {}", since(ms)),
                };
                format!(
                    "{} {:<20} {:<8} paired {:<10} {seen}",
                    if d.online { "●" } else { "○" },
                    d.name.chars().take(20).collect::<String>(),
                    platform_name(&d.platform),
                    since(d.paired_at_ms),
                )
            })
            .collect();
        if rows.is_empty() {
            return vec!["no devices paired · p pairs one".into()];
        }
        rows
    }

    /// `/link`: `p` pairs (card in the transcript), `r`/`n`/`x` open a
    /// prompt, `e` flips `link.enabled` in place.
    pub(super) fn link_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        let Some(Menu::Link { cursor }) = &mut self.menu else {
            return MenuKey::Consumed;
        };
        let Some(s) = &self.link else {
            self.menu = None;
            return MenuKey::Consumed;
        };
        let device = s
            .devices
            .get(*cursor)
            .map(|d| (d.id.clone(), d.name.clone()));
        let enabled = s.enabled;
        match code {
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Down => *cursor = (*cursor + 1).min(s.devices.len().saturating_sub(1)),
            KeyCode::Esc => self.menu = None,
            KeyCode::Char('p') => {
                self.menu = None;
                send(tx, Command::LinkPair);
            }
            KeyCode::Char('e') => send(tx, Command::LinkEnable(!enabled)),
            KeyCode::Char('n') => {
                self.menu = None;
                self.prompt = Some(Prompt::LinkName);
            }
            KeyCode::Char('r') => {
                if let Some((id, name)) = device {
                    self.menu = None;
                    self.prompt = Some(Prompt::LinkDeviceName { id, name });
                }
            }
            KeyCode::Char('x') => {
                if let Some((id, name)) = device {
                    self.menu = None;
                    self.prompt = Some(Prompt::LinkRevoke { id, name });
                }
            }
            _ => {}
        }
        MenuKey::Consumed
    }

    /// `/nodes` menu | `add <key>` | `rm <n>` | `name <n> <name…>` |
    /// `invite <n>` (32 §6.7). `<n>` is a node name or id prefix, resolved
    /// by the command task against `list_nodes` ([`find_node`]).
    pub(super) fn nodes_cmd(&mut self, arg: &str, tx: &UnboundedSender<Command>) {
        match parse_nodes_arg(arg) {
            Some(NodesArg::Menu) => {
                self.awaiting_nodes = true;
                send(tx, Command::ListNodes);
            }
            Some(NodesArg::Add(key)) => send(tx, Command::AddNode(key, None)),
            Some(NodesArg::Remove(n)) => send(tx, Command::RemoveNode(n)),
            Some(NodesArg::Rename(n, name)) => send(tx, Command::RenameNode(n, name)),
            Some(NodesArg::Invite(n)) => send(tx, Command::InviteNode(n)),
            None => self.entries.push(Entry::Error(
                "usage: /nodes [add <key>|rm <n>|name <n> <name>|invite <n>]".into(),
            )),
        }
    }

    /// `/nodes` menu title: the count, then the keys.
    pub fn nodes_title(&self) -> String {
        format!(
            "nodes · {} paired   a add (paste key)  i invite (QR for a phone)  r rename  x remove  esc",
            self.nodes.as_ref().map_or(0, Vec::len)
        )
    }

    /// `/nodes` menu rows: `● name  platform  theuth <version>  [<n> running]  paired <ago>`
    /// online, `○ name  platform  theuth <version>  seen <ago>|revoked` otherwise,
    /// or one hint row when none is paired.
    pub fn nodes_rows(&self) -> Vec<String> {
        let now = now_ms();
        let since = |ms: u64| ago(chrono::Duration::milliseconds(now.saturating_sub(ms) as i64));
        let rows: Vec<String> = self
            .nodes
            .iter()
            .flatten()
            .map(|n| {
                let tail = match (n.revoked, n.online, n.children, n.last_seen_ms) {
                    (true, ..) => "revoked".to_owned(),
                    (false, true, 0, _) => format!("paired {}", since(n.paired_at_ms)),
                    (false, true, c, _) => {
                        format!("{c} running   paired {}", since(n.paired_at_ms))
                    }
                    (false, false, _, 0) => "never seen".to_owned(),
                    (false, false, _, ms) => format!("seen {}", since(ms)),
                };
                format!(
                    "{} {:<20} {:<8} theuth {:<8} {tail}",
                    if n.online { "●" } else { "○" },
                    n.name.chars().take(20).collect::<String>(),
                    platform_name(&n.platform),
                    n.version,
                )
            })
            .collect();
        if rows.is_empty() {
            return vec!["no nodes paired · a adds one from its key".into()];
        }
        rows
    }

    /// `/nodes`: `a` prompts for a key, `i` invites (card in the transcript),
    /// `r`/`x` open a prompt over the highlighted node.
    pub(super) fn nodes_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        let Some(Menu::Nodes { cursor }) = &mut self.menu else {
            return MenuKey::Consumed;
        };
        let list = self.nodes.as_deref().unwrap_or_default();
        let node = list.get(*cursor).map(|n| (n.id.clone(), n.name.clone()));
        match code {
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Down => *cursor = (*cursor + 1).min(list.len().saturating_sub(1)),
            KeyCode::Esc => self.menu = None,
            KeyCode::Char('a') => {
                self.menu = None;
                self.prompt = Some(Prompt::NodeKey);
            }
            KeyCode::Char('i') => {
                if let Some((id, _)) = node {
                    self.menu = None;
                    send(tx, Command::InviteNode(id));
                }
            }
            KeyCode::Char('r') => {
                if let Some((id, name)) = node {
                    self.menu = None;
                    self.prompt = Some(Prompt::NodeName { id, name });
                }
            }
            KeyCode::Char('x') => {
                if let Some((id, name)) = node {
                    self.menu = None;
                    self.prompt = Some(Prompt::NodeRemove { id, name });
                }
            }
            _ => {}
        }
        MenuKey::Consumed
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
    let (sub, rest) = arg
        .split_once(char::is_whitespace)
        .map_or((arg, ""), |(s, r)| (s, r.trim()));
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

/// The node `/nodes <verb> <n>` names: an exact name, else the one node
/// whose id starts with `n`.
pub fn find_node<'a>(list: &'a [nodes::Node], n: &str) -> Option<&'a nodes::Node> {
    list.iter().find(|x| x.name == n).or_else(|| {
        let mut hits = list.iter().filter(|x| x.id.starts_with(n));
        match (hits.next(), hits.next()) {
            (Some(one), None) => Some(one),
            _ => None,
        }
    })
}

/// Host part of a relay origin: `wss://relay.theuth.io/x` → `relay.theuth.io`.
fn relay_host(relay: &str) -> &str {
    let rest = relay.split_once("://").map_or(relay, |(_, r)| r);
    rest.split('/').next().unwrap_or(rest)
}

/// `ios` → `iOS`, `macos` → `macOS`; anything else as sent.
fn platform_name(p: &str) -> &str {
    match p {
        "ios" => "iOS",
        "macos" => "macOS",
        other => other,
    }
}

/// One-line `link_status`: `link off` or `link on · <host> · connected · 2 devices (1 online)`.
pub fn link_line(s: &link::LinkStatus) -> String {
    if !s.enabled {
        return "link off".into();
    }
    format!(
        "link on · {} · {} · {} device{} ({} online)",
        relay_host(&s.relay),
        if s.connected {
            "connected"
        } else {
            "disconnected"
        },
        s.devices.len(),
        if s.devices.len() == 1 { "" } else { "s" },
        s.devices.iter().filter(|d| d.online).count()
    )
}


/// One `/mesh devices` row: `<id>  <name>  <platform>  paired 2d ago · seen 3h ago | never seen | revoked`.
pub fn mesh_device_line(d: &link::DeviceInfo, now: u64) -> String {
    let since = |ms: u64| ago(chrono::Duration::milliseconds(now.saturating_sub(ms) as i64));
    let state = if d.revoked {
        "revoked".to_owned()
    } else if d.last_seen_ms == 0 {
        "never seen".to_owned()
    } else {
        format!("seen {}", since(d.last_seen_ms))
    };
    format!(
        "{}  {}  {}  paired {} · {state}",
        d.id,
        d.name,
        d.platform,
        since(d.paired_ms)
    )
}
