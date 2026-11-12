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

