use super::*;

/// One row of the `/login` picker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginRow {
    /// Provider namespace or MCP server name.
    pub name: String,
    /// An MCP server (else a provider).
    pub mcp: bool,
    /// `logged in (acct)` / `login required` / `not logged in`.
    pub status: String,
}

/// A pending `secret_request` (37 §5): the masked box in the approval box's place.
#[derive(Debug)]
pub struct SecretRequest {
    /// Answer with this id.
    pub id: secrets::RequestId,
    /// Name it will be stored under.
    pub name: String,
    /// What it is for.
    pub note: String,
    /// `Some(v)` when the name exists: the answer adds `v + 1`.
    pub current: Option<u32>,
}

impl Model {

    /// The `secret_request` currently shown, if any.
    pub fn secret_request(&self) -> Option<&SecretRequest> {
        self.secret_requests.front()
    }
}

impl Model {

    /// Fold a secrets bus event in: a change re-lists a held list, a request
    /// from this session's tree queues its masked box, a resolution (answered
    /// elsewhere, declined, timed out) drops it.
    pub(super) fn apply_secrets(&mut self, ev: secrets::SecretsEvent) {
        use secrets::SecretsEvent::*;
        match ev {
            Changed => self.refresh_secrets = self.secrets.is_some(),
            Requested {
                id,
                session,
                name,
                note,
                current,
                ..
            } => {
                if !self.secret_requests.iter().any(|pending| pending.id == id)
                    && (self.session == Some(session) || self.agents.contains_key(&session))
                {
                    self.secret_requests.push_back(SecretRequest {
                        id,
                        name,
                        note,
                        current,
                    });
                }
            }
            Resolved { id, timed_out, .. } => {
                if let Some(i) = self.secret_requests.iter().position(|r| r.id == id) {
                    let r = self.secret_requests.remove(i).expect("position");
                    if timed_out {
                        self.entries
                            .push(Entry::Info(format!("{}: request timed out", r.name)));
                    }
                }
            }
            // 183 §12.9: the rotation banner is the desktop's Network page; the TUI shows none.
            Rotation(_) => {}
        }
    }

    /// Answer the shown `secret_request` (`None` declines) and dismiss it
    /// locally; its `Resolved` is then a no-op. The value is never shown.
    pub fn answer_secret(&mut self, value: Option<String>, tx: &UnboundedSender<Command>) {
        let Some(r) = self.secret_requests.pop_front() else {
            return;
        };
        self.entries.push(Entry::Info(format!(
            "{}: {}",
            r.name,
            if value.is_some() {
                "stored"
            } else {
                "declined"
            }
        )));
        send(tx, Command::AnswerSecret { id: r.id, value });
    }
}

impl Model {

    /// `/provider` list | `add` [name [url [key]]] | `refresh <name>` | `rm <name>`
    /// | `limits <provider> <model> <context> [output]` | `find` (177 E10).
    pub(super) fn provider_cmd(&mut self, arg: &str, tx: &UnboundedSender<Command>) {
        const USAGE: &str = "usage: /provider [add [name [url [key]]] | refresh <name> | rm <name> | limits <provider> <model> <context> [output] | find]";
        let (sub, rest) = arg
            .split_once(' ')
            .map_or((arg, ""), |(s, r)| (s, r.trim()));
        match (sub, rest) {
            ("", _) => send(tx, Command::ListProviders),
            ("add", "") => {
                self.prompt = Some(Prompt::ProviderName);
                self.entries.push(Entry::Info("provider add: name".into()));
            }
            ("add", rest) => {
                let mut parts = rest.splitn(3, char::is_whitespace);
                let name = parts.next().unwrap_or_default();
                let url = parts.next().unwrap_or("").trim();
                let key = parts.next().unwrap_or("").trim();
                if name.is_empty() {
                    self.entries.push(Entry::Error(
                        "usage: /provider add [name [url [key]]]".into(),
                    ));
                    return;
                }
                if url.is_empty() {
                    self.prompt = Some(Prompt::ProviderUrl {
                        name: name.to_owned(),
                    });
                    self.entries
                        .push(Entry::Info(format!("provider add {name}: base url")));
                    return;
                }
                // 177 E10: a local server needs no key; a cloud one is asked for.
                if key.is_empty() && !agent::local::private_url(url) {
                    self.prompt = Some(Prompt::ProviderKey {
                        name: name.to_owned(),
                        url: url.to_owned(),
                    });
                    self.entries
                        .push(Entry::Info(format!("provider add {name}: api key")));
                    return;
                }
                send(
                    tx,
                    Command::AddProvider {
                        name: name.to_owned(),
                        base_url: url.to_owned(),
                        key: key.to_owned(),
                    },
                );
            }
            ("refresh", name) if !name.is_empty() => {
                send(tx, Command::RefreshProvider(name.to_owned()))
            }
            ("rm", name) if !name.is_empty() => send(tx, Command::RemoveProvider(name.to_owned())),
            ("find", "") => send(tx, Command::FindLocalServers),
            ("limits", rest) => {
                let p: Vec<&str> = rest.split_whitespace().collect();
                let n = |i: usize| p.get(i).map(|s| s.parse::<u32>());
                match (p.len(), n(2), n(3)) {
                    (3, Some(Ok(context)), None) | (4, Some(Ok(context)), Some(Ok(_))) => {
                        send(
                            tx,
                            Command::SetModelLimits {
                                provider: p[0].to_owned(),
                                model: p[1].to_owned(),
                                // 0 clears back to unknown.
                                context: (context > 0).then_some(context),
                                output: n(3).and_then(Result::ok),
                            },
                        );
                    }
                    _ => self.entries.push(Entry::Error(
                        "usage: /provider limits <provider> <model> <context> [output] (0 = unknown)"
                            .into(),
                    )),
                }
            }
            _ => self.entries.push(Entry::Error(USAGE.into())),
        }
    }

    /// `/secrets` menu | `add [name]` | `rm <name>` | `restore <name> [n]` (37 §5).
    pub(super) fn secrets_cmd(&mut self, arg: &str, tx: &UnboundedSender<Command>) {
        let (sub, rest) = arg
            .split_once(char::is_whitespace)
            .map_or((arg, ""), |(s, r)| (s, r.trim()));
        match (sub, rest) {
            ("", _) => {
                self.awaiting_secrets = true;
                send(tx, Command::ListSecrets);
            }
            ("add", "") => self.prompt = Some(Prompt::SecretName),
            ("add", name) if secrets::vault::valid_name(name) => {
                self.prompt = Some(Prompt::SecretValue {
                    name: name.to_owned(),
                    current: self.secret_current(name),
                });
            }
            ("add", name) => self.entries.push(Entry::Error(bad_secret_name(name))),
            ("rm", name) if !name.is_empty() => send(tx, Command::DeleteSecret(name.to_owned())),
            ("restore", rest) if !rest.is_empty() => {
                let (name, back) = rest
                    .split_once(char::is_whitespace)
                    .map_or((rest, "1"), |(n, b)| (n, b.trim()));
                match back.parse::<u8>() {
                    Ok(back) if (1..secrets::KEEP as u8).contains(&back) => send(
                        tx,
                        Command::RestoreSecret {
                            name: name.to_owned(),
                            back,
                        },
                    ),
                    _ => self.entries.push(Entry::Error(format!(
                        "/secrets restore: n is 1..={}",
                        secrets::KEEP - 1
                    ))),
                }
            }
            _ => self.entries.push(Entry::Error(
                "usage: /secrets [add [name] | rm <name> | restore <name> [n]]".into(),
            )),
        }
    }

    /// Current version of `name` per the last `list_secrets`; `None` when
    /// unknown (or never listed).
    pub(super) fn secret_current(&self, name: &str) -> Option<u32> {
        self.secrets
            .as_ref()?
            .secrets
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.current)
    }

    /// `/secrets` menu title: the key store, then the keys.
    pub fn secrets_title(&self) -> String {
        let key = self.secrets.as_ref().map_or("?", |l| l.key_store.as_str());
        match &self.menu {
            Some(Menu::Secrets {
                cursor,
                history: Some(_),
            }) => {
                let name = self
                    .secrets
                    .as_ref()
                    .and_then(|l| l.secrets.get(*cursor))
                    .map_or("", |s| s.name.as_str());
                format!("secrets · {name} · history   ⏎ restore  h/esc back")
            }
            _ => format!("secrets · key: {key}   a add  r replace  h history  d delete  esc"),
        }
    }

    /// `/secrets` menu rows: `NAME · vN (k kept) · note · updated`, or the
    /// highlighted secret's versions `vN · created · source [· current]`
    /// while its history is open. Never a value.
    pub fn secrets_rows(&self) -> Vec<String> {
        let list = self
            .secrets
            .as_ref()
            .map(|l| l.secrets.as_slice())
            .unwrap_or_default();
        if let Some(Menu::Secrets {
            cursor,
            history: Some(_),
        }) = &self.menu
            && let Some(s) = list.get(*cursor)
        {
            return s
                .versions
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    format!(
                        "v{} · {} · {}{}",
                        v.v,
                        when(&v.created),
                        v.source,
                        if i == 0 { " · current" } else { "" }
                    )
                })
                .collect();
        }
        if list.is_empty() {
            return vec!["no secrets · a adds one".into()];
        }
        list.iter()
            .map(|s| {
                format!(
                    "{:<20} · v{} ({} kept) · {} · {}",
                    s.name.chars().take(20).collect::<String>(),
                    s.current,
                    s.versions.len(),
                    s.note,
                    when(&s.updated)
                )
            })
            .collect()
    }

    /// `/secrets`: `a` adds, `r` replaces the highlighted value, `h` opens its
    /// history (⏎ on an older version restores it), `d` deletes after a
    /// typed confirm. Esc leaves the history, then the menu.
    pub(super) fn secrets_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        let Some(Menu::Secrets { cursor, history }) = &mut self.menu else {
            return MenuKey::Consumed;
        };
        // Field access, not a method: `cursor`/`history` hold `self.menu`.
        let list = self
            .secrets
            .as_ref()
            .map(|l| l.secrets.as_slice())
            .unwrap_or_default();
        let info = list.get(*cursor);
        let name = info.map(|s| s.name.clone());
        let current = info.map(|s| s.current);
        let versions = info.map_or(0, |s| s.versions.len());
        match (code, *history) {
            (KeyCode::Up, Some(h)) => *history = Some(h.saturating_sub(1)),
            (KeyCode::Down, Some(h)) => *history = Some((h + 1).min(versions.saturating_sub(1))),
            (KeyCode::Esc | KeyCode::Char('h'), Some(_)) => *history = None,
            (KeyCode::Enter, Some(h)) if h > 0 => {
                if let (Some(name), Ok(back)) = (name, u8::try_from(h)) {
                    send(tx, Command::RestoreSecret { name, back });
                }
            }
            (KeyCode::Up, None) => *cursor = cursor.saturating_sub(1),
            (KeyCode::Down, None) => *cursor = (*cursor + 1).min(list.len().saturating_sub(1)),
            (KeyCode::Esc, None) => self.menu = None,
            (KeyCode::Char('h'), None) if info.is_some() => *history = Some(0),
            (KeyCode::Char('a'), _) => {
                self.menu = None;
                self.prompt = Some(Prompt::SecretName);
            }
            (KeyCode::Char('r'), _) => {
                if let Some(name) = name {
                    self.menu = None;
                    self.prompt = Some(Prompt::SecretValue { name, current });
                }
            }
            (KeyCode::Char('d'), _) => {
                if let Some(name) = name {
                    self.menu = None;
                    self.prompt = Some(Prompt::SecretDelete { name });
                }
            }
            _ => {}
        }
        MenuKey::Consumed
    }
}

impl Model {

    /// `/login`: Enter logs the highlighted row in.
    pub(super) fn logins_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        let Some(Menu::Logins { items, cursor }) = &mut self.menu else {
            return MenuKey::Consumed;
        };
        match code {
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Down => *cursor = (*cursor + 1).min(items.len().saturating_sub(1)),
            KeyCode::Esc => self.menu = None,
            KeyCode::Enter => {
                if let Some(r) = items.get(*cursor) {
                    send(tx, Command::Login(r.name.clone()));
                }
                self.menu = None;
            }
            _ => {}
        }
        MenuKey::Consumed
    }
}

/// Error line for a name the vault would refuse (37 §3.1).
pub(super) fn bad_secret_name(name: &str) -> String {
    format!("secrets: {name:?} is not a name; use [A-Za-z0-9_.-], at most 64")
}
