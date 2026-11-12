//! Provider catalog/configuration, login picker and OAuth completion display.
use super::*;
use agent::ProviderInfo;
use rpc_compose::api::clients::*;
use serde::Deserialize;
use iocraft::prelude::{element, Text, View as Layout};
use tools::McpAuth;

#[derive(Default)]
pub(crate) struct ProvidersLogin {
    pub(crate) prompt: Option<Prompt>,
    pub(crate) menu: Option<(Vec<LoginRow>, usize)>,
}
#[derive(Clone)]
pub(crate) enum Request {
    ListLogins, Login(String), Logout(String), ListProviders,
    AddProvider { name: String, base_url: String, key: String },
    RemoveProvider(String), RefreshProvider(String), FindLocalServers,
    SetModelLimits { provider: String, model: String, context: Option<u32>, output: Option<u32> },
}
pub(crate) enum Reply { Logins(Vec<LoginRow>), Providers(Vec<ProviderInfo>), ProviderAdded(ProviderInfo), Info(String) }
pub(crate) enum Msg { Reply(Result<Reply, String>), Finished(agent::LoginFinished) }
/// One row of the `/login` picker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LoginRow {
    /// Provider namespace or MCP server name.
    pub name: String,
    /// An MCP server (else a provider).
    pub mcp: bool,
    /// `logged in (acct)` / `login required` / `not logged in`.
    pub status: String,
}


/// Provider wizard fields, backed by the mounted composer.
#[derive(Debug)]
pub(crate) enum Prompt {
    /// Asking for the gateway name.
    Name,
    /// Asking for the base URL.
    Url {
        /// Name already collected.
        name: String,
    },
    /// Asking for the API key.
    Key {
        /// Name already collected.
        name: String,
        /// Normalized URL already collected.
        url: String,
    },
}

impl Prompt {
    /// Line shown above the input.
    pub fn hint(&self) -> String {
        match self {
            Prompt::Name => "provider add  name  (esc cancel)".into(),
            Prompt::Url { name } => {
                format!("provider add  {name}  base url  (esc cancel)")
            }
            Prompt::Key { name, .. } => {
                format!("provider add  {name}  api key  (esc cancel)")
            }
        }
    }

    /// Notice when Esc drops the flow.
    fn cancelled(&self) -> &'static str {
        match self {
            Prompt::Name | Prompt::Url { .. } | Prompt::Key { .. } => {
                "provider add cancelled"
            }
        }
    }

    /// The input box hides the provider's key.
    pub fn masked(&self) -> bool {
        matches!(
            self,
            Prompt::Key { .. }
        )
    }
}
impl ProvidersLogin {

    /// `/provider` list | `add` [name [url [key]]] | `refresh <name>` | `rm <name>`
    /// | `limits <provider> <model> <context> [output]` | `find` (177 E10).
    fn provider_cmd(&mut self, arg: &str, cx: &mut Cx<'_, '_, Msg>) {
        const USAGE: &str = "usage: /provider [add [name [url [key]]] | refresh <name> | rm <name> | limits <provider> <model> <context> [output] | find]";
        let (sub, rest) = arg
            .split_once(' ')
            .map_or((arg, ""), |(s, r)| (s, r.trim()));
        match (sub, rest) {
            ("", _) => self.request(Request::ListProviders, cx),
            ("add", "") => {
                self.prompt = Some(Prompt::Name);
                cx.core(CoreAction::Notice("provider add: name".into()));
            }
            ("add", rest) => {
                let mut parts = rest.splitn(3, char::is_whitespace);
                let name = parts.next().unwrap_or_default();
                let url = parts.next().unwrap_or("").trim();
                let key = parts.next().unwrap_or("").trim();
                if name.is_empty() {
                    cx.core(CoreAction::Reject(
                        "usage: /provider add [name [url [key]]]".into(),
                    ));
                    return;
                }
                if url.is_empty() {
                    self.prompt = Some(Prompt::Url {
                        name: name.to_owned(),
                    });
                    cx.core(CoreAction::Notice(format!("provider add {name}: base url")));
                    return;
                }
                // 177 E10: a local server needs no key; a cloud one is asked for.
                if key.is_empty() && !agent::local::private_url(url) {
                    self.prompt = Some(Prompt::Key {
                        name: name.to_owned(),
                        url: url.to_owned(),
                    });
                    cx.core(CoreAction::Notice(format!("provider add {name}: api key")));
                    return;
                }
                self.request(Request::AddProvider {
                        name: name.to_owned(),
                        base_url: url.to_owned(),
                        key: key.to_owned(),
                    }, cx);
            }
            ("refresh", name) if !name.is_empty() => {
                self.request(Request::RefreshProvider(name.to_owned()), cx)
            }
            ("rm", name) if !name.is_empty() => self.request(Request::RemoveProvider(name.to_owned()), cx),
            ("find", "") => self.request(Request::FindLocalServers, cx),
            ("limits", rest) => {
                let p: Vec<&str> = rest.split_whitespace().collect();
                let n = |i: usize| p.get(i).map(|s| s.parse::<u32>());
                match (p.len(), n(2), n(3)) {
                    (3, Some(Ok(context)), None) | (4, Some(Ok(context)), Some(Ok(_))) => {
                        self.request(Request::SetModelLimits {
                                provider: p[0].to_owned(),
                                model: p[1].to_owned(),
                                // 0 clears back to unknown.
                                context: (context > 0).then_some(context),
                                output: n(3).and_then(Result::ok),
                            }, cx);
                    }
                    _ => cx.core(CoreAction::Reject(
                        "usage: /provider limits <provider> <model> <context> [output] (0 = unknown)"
                            .into(),
                    )),
                }
            }
            _ => cx.core(CoreAction::Reject(USAGE.into())),
        }
    }

}
impl ProvidersLogin {

    /// Advance a prompt flow one field. An API key is never written to the transcript.
    fn prompt_submit(&mut self, line: &str, cx: &mut Cx<'_, '_, Msg>) {
        match self.prompt.take() {
            Some(Prompt::Name) => {
                self.prompt = Some(Prompt::Url {
                    name: line.to_owned(),
                });
                cx.core(CoreAction::Notice(format!("name: {line}")));
            }
            Some(Prompt::Url { name }) => {
                cx.core(CoreAction::Notice(format!("url: {line}")));
                // 177 E10: a local server needs no key.
                if agent::local::private_url(line) {
                    self.request(Request::AddProvider {
                            name,
                            base_url: line.to_owned(),
                            key: String::new(),
                        }, cx);
                } else {
                    self.prompt = Some(Prompt::Key {
                        name: name.clone(),
                        url: line.to_owned(),
                    });
                }
            }
            Some(Prompt::Key { name, url }) => {
                cx.core(CoreAction::Notice("api key: set".into()));
                self.request(Request::AddProvider {
                        name,
                        base_url: url,
                        key: line.to_owned(),
                    }, cx);
            }
            None => {}
        }
    }

    /// Drop an in-progress prompt flow.
    fn cancel_prompt(&mut self, cx: &mut Cx<'_, '_, Msg>) {
        if let Some(p) = self.prompt.take() {
            cx.core(CoreAction::Notice(p.cancelled().into()));
        }
    }
}

impl ProvidersLogin {
    fn request(&self, request: Request, cx: &mut Cx<'_, '_, Msg>) {
        cx.spawn_request(Lane::Concurrent, request, |api, request| async move { Msg::Reply(run(&api, request).await) });
    }
    fn key(&mut self, code: KeyCode, cx: &mut Cx<'_, '_, Msg>) {
        let Some((items, cursor)) = &mut self.menu else {
            if self.prompt.is_some() && code == KeyCode::Esc { self.cancel_prompt(cx); cx.close(); cx.consume_key(); }
            return;
        };
        match code {
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Down => *cursor = (*cursor + 1).min(items.len().saturating_sub(1)),
            KeyCode::Esc => { self.menu = None; if self.prompt.is_none() { cx.close(); } }
            KeyCode::Enter => {
                let name = items.get(*cursor).map(|r| r.name.clone());
                self.menu = None; if self.prompt.is_none() { cx.close(); }
                if let Some(name) = name { self.request(Request::Login(name), cx); }
            }
            _ => {}
        }
        cx.consume_key();
    }
}
async fn run(api: &Engine, request: Request) -> Result<Reply, String> {
    Ok(match request {
        Request::ListLogins => {
            let mut rows: Vec<LoginRow> = api.list_logins().await.map_err(crate::reason)?
                .into_iter()
                .map(|l| LoginRow {
                    name: l.name,
                    mcp: false,
                    status: match (l.logged_in, l.account) {
                        (false, _) => "not logged in".to_owned(),
                        (true, Some(a)) => format!("logged in ({a})"),
                        (true, None) => "logged in".to_owned(),
                    },
                })
                .collect();
            rows.extend(
                api.list_mcp(None).await.map_err(crate::reason)?
                    .into_iter()
                    .filter(|s| s.auth != McpAuth::None)
                    .map(|s| LoginRow {
                        name: s.name,
                        mcp: true,
                        status: match s.auth {
                            McpAuth::LoggedIn => "logged in".to_owned(),
                            _ => "login required".to_owned(),
                        },
                    }),
            );
            Reply::Logins(rows)
        }
        // A provider name first, else an MCP server.
        Request::Login(name) => {
            let url = if oauth::provider(&name).is_some() {
                api.login(name).await.map_err(crate::reason)?
            } else {
                api.mcp_login(name).await.map_err(crate::reason)?
            };
            Reply::Info(format!("login: open {url}"))
        }
        Request::Logout(name) => {
            if oauth::provider(&name).is_some() {
                api.logout(name.clone()).await.map_err(crate::reason)?;
            } else {
                api.mcp_logout(name.clone()).await.map_err(crate::reason)?;
            }
            Reply::Info(format!("logged out of {name}"))
        }
        Request::ListProviders => Reply::Providers(api.list_providers().await.map_err(crate::reason)?),
        Request::AddProvider {
            name,
            base_url,
            key,
        } => Reply::ProviderAdded(api
            .add_provider(name, base_url, (!key.is_empty()).then_some(key), None)
            .await.map_err(crate::reason)?),
        Request::RemoveProvider(name) => {
            api.remove_provider(name.clone()).await.map_err(crate::reason)?;
            Reply::Info(format!("removed provider {name}"))
        }
        Request::RefreshProvider(name) => {
            Reply::Info(api.refresh_provider_models(name).await.map_err(crate::reason)?.line())
        }
        Request::FindLocalServers => Reply::Info(
            api.local_servers_probe().await.map_err(crate::reason)?
                .iter()
                .map(agent::LocalServer::line)
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        Request::SetModelLimits {
            provider,
            model,
            context,
            output,
        } => {
            // R2-8: no output given keeps the stored one (as the desktop does); 0 clears.
            let output = match output {
                Some(o) => (o > 0).then_some(o),
                None => api.provider_models(provider.clone()).await.map_err(crate::reason)?
                    .into_iter()
                    .find(|m| m.id == model)
                    .map(|m| m.output)
                    .filter(|&o| o > 0),
            };
            let m = api
                .set_model_limits(provider.clone(), model, context, output)
                .await.map_err(crate::reason)?;
            Reply::Info(format!("{provider}/{}", m.line()))
        }
    })
}
impl Component for ProvidersLogin {
    type Msg = Msg;
    fn declaration(&self) -> Declaration {
        Declaration { id: "providers-login", interests: &[Interest { topic: "agent.login", version: 1, variants: &["LoginFinished"] }], slash: &[
            ("/login", "[name]", "log in to a provider or MCP server (picker without name)"),
            ("/logout", "<name>", "forget a login"),
            ("/provider", "[add [name [url [key]]] | refresh <name> | rm <name> | limits <provider> <model> <context> [output] | find]", "providers: built-in opencode (Zen) / openrouter keyed via /secrets add; add prompts for a custom provider's name, url, key (no key for a local server); refresh lists a provider's models again; limits sets a model's context/output (0 = unknown); find probes local model servers"),
        ], bindings: &[] }
    }
    fn decode(&self, event: &TopicEvent, _: &CoreView<'_>) -> Option<Msg> {
        if event.topic().is_some() && (event.topic() != Some("agent.login") || event.version() != Some(1)) { return None; }
        if event.event()["type"] != "LoginFinished" { return None; }
        agent::LoginFinished::deserialize(event.event()).ok().map(Msg::Finished)
    }
    fn update(&mut self, input: Input<'_, Msg>, cx: &mut Cx<'_, '_, Msg>) {
        if cx.core.reasoning && matches!(&input, Input::Message(Msg::Reply(Ok(_)) | Msg::Finished(_))) { cx.plain(Plain::Text("\x1b[0m\n".into())); }
        match input {
            Input::Slash { name: "/provider", args } => { self.provider_cmd(args, cx); if self.prompt.is_some() { cx.focus(FocusClass::Modal); } }
            Input::Slash { name: "/login", args } => self.request(if args.is_empty() { Request::ListLogins } else { Request::Login(args.to_owned()) }, cx),
            Input::Slash { name: "/logout", args: "" } => cx.core(CoreAction::Reject("usage: /logout <name>".into())),
            Input::Slash { name: "/logout", args } => self.request(Request::Logout(args.to_owned()), cx),
            Input::Submit(line) if self.prompt.is_some() => {
                self.prompt_submit(line, cx); cx.consume_key(); if self.prompt.is_none() { cx.close(); }
            }
            Input::Key(key) => self.key(key.code, cx),
            Input::Message(Msg::Reply(Err(error))) => cx.error(error),
            Input::Message(Msg::Reply(Ok(reply))) => match reply {
                Reply::Logins(items) => {
                    for r in &items { cx.plain(Plain::Line(format!("{}  {}", r.name, r.status))); }
                    if items.is_empty() { cx.core(CoreAction::Notice("nothing to log in to".into())); }
                    else { self.menu = Some((items, 0)); cx.focus(FocusClass::Modal); }
                }
                Reply::Info(text) => { cx.plain(Plain::Line(text.clone())); cx.core(CoreAction::Notice(text)); }
                Reply::ProviderAdded(p) => {
                    let line = format!("provider {}  {}  {} models — /model {}/<id>", p.name, p.base_url, p.models, p.name);
                    cx.plain(Plain::Line(line.clone())); cx.core(CoreAction::Notice(line));
                }
                Reply::Providers(list) => {
                    for p in &list {
                        cx.plain(Plain::Line(format!("{}  {}  {} models", p.name, p.base_url, p.models)));
                        cx.core(CoreAction::Notice(if p.builtin { format!("{}  built-in  {} models  {}", p.name, p.models, if p.configured { "key set".to_owned() } else { format!("no key — /secrets add {}", p.name) }) }
                            else { format!("{}  {}  {} models", p.name, p.base_url, p.models) }));
                    }
                    if list.iter().all(|p| p.builtin) { cx.core(CoreAction::Notice("no custom providers; /provider add to create one".into())); }
                }
            },
            Input::Message(Msg::Finished(f)) => {
                let line = match &f.error { None => format!("login: {} ok", f.provider), Some(e) => format!("login: {} failed: {e}", f.provider) };
                cx.plain(Plain::Line(line.clone()));
                cx.core(if f.error.is_some() { CoreAction::Reject(line) } else { CoreAction::Notice(line) });
            }
            Input::Lifecycle(Lifecycle::FocusLost | Lifecycle::SessionReset) => { self.prompt = None; self.menu = None; }
            _ => {}
        }
    }
    fn captures(&self, class: FocusClass) -> bool { class == FocusClass::Modal && self.menu.is_some() }
    fn editor(&self) -> Option<Editor> {
        self.prompt.as_ref().map(|p| Editor { hint: String::new(), masked: p.masked(), submit_empty: false })
    }
    fn view(&self, at: Surface<'_>, cx: &ViewCx<'_>) -> Option<View> {
        if matches!(at, Surface::PromptBadge) {
            let prompt = self.prompt.as_ref()?;
            return Some(View::Element {
                element: element!(Layout(height: 1, padding_left: 1) {
                    Text(content: prompt.hint(), color: crate::ui::OCHRE)
                }).into_any(),
                rows: 1, replace: false,
            });
        }
        if !matches!(at, Surface::Modal) { return None; }
        let (items, cursor) = self.menu.as_ref()?;
        let rows = items.iter().map(|r| format!("{:<8} {:<20} {}", if r.mcp { "mcp" } else { "provider" }, r.name, r.status)).collect();
        Some(View::Element { element: crate::ui::menu_view("login  ⏎ log in  esc close".into(), rows, *cursor, cx.width as u16, cx.height as u16, false), rows: 0, replace: false })
    }
}
