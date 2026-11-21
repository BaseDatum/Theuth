//! Vault metadata, masked administration wizard and intrinsic live-request overlay.
use super::*;
use std::collections::{HashMap, VecDeque};
use serde::Deserialize;
use rpc_compose::api::clients::*;
use iocraft::prelude::{element, View as BoxView, Text, BorderStyle, FlexDirection, Weight, TextWrap};
use crate::model::when;
use crate::ui::{FAIENCE, BONE_DIM};

/// Global metadata is retained across session resets; pending input is not.
#[derive(Default)]
pub struct Secrets {
    pub(crate) secrets: Option<::secrets::SecretsList>,
    awaiting: bool,
    pub(crate) refresh: bool,
    pub(crate) menu: Option<Menu>,
    pub(crate) prompt: Option<Prompt>,
    pub(crate) requests: VecDeque<SecretRequest>,
}
// No Debug: `values` holds what the user typed until its one RPC submission.
pub(crate) struct SecretRequest {
    pub(crate) id: ::secrets::RequestId,
    /// What it is for (a card's title, else the one field's label).
    note: String,
    /// In card order; never empty.
    pub(crate) fields: Vec<::secrets::SecretRequestField>,
    /// The agent's suggestion (`2h`, an RFC 3339 instant); the expiry step shows it.
    expires: Option<String>,
    /// Typed so far, one per field in order; the expiry step follows the last.
    values: Vec<String>,
}
impl SecretRequest {
    /// The field being typed; `None` = the expiry step.
    fn field(&self) -> Option<&::secrets::SecretRequestField> { self.fields.get(self.values.len()) }
    fn names(&self) -> String { self.fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>().join(", ") }
}
pub(crate) enum Menu { Secrets { cursor: usize, history: Option<usize> } }
// No Debug: the note step owns the value until its one RPC submission.
pub(crate) enum Prompt {
    Name,
    Value { name: String, current: Option<u32> },
    Note { name: String, value: String },
    Delete { name: String },
}
impl Prompt {
    pub(crate) fn hint(&self) -> String {
        match self {
            Self::Name => "secrets add  name  (esc cancel)".into(),
            Self::Value { name, current: None } => format!("secrets add  {name}  value  (esc cancel)"),
            Self::Value { name, current: Some(v) } => format!("secrets add  {name}  value  (replaces v{v} → v{}; esc cancel)", v + 1),
            Self::Note { name, .. } => format!("secrets add  {name}  note  (⏎ none; esc cancel)"),
            Self::Delete { name } => format!("secrets  delete {name}?  type the name to confirm  (esc cancel)"),
        }
    }
    pub(crate) fn masked(&self) -> bool { matches!(self, Self::Value { .. }) }
}
/// RPC input; deliberately opaque to Debug because set/answer carry raw values.
#[derive(Clone)]
pub enum Request {
    /// Read metadata only.
    ListSecrets,
    /// Store one submitted value.
    SetSecret {
        /// Vault key.
        name: String,
        /// Value, never printed.
        value: String,
        /// Optional note.
        note: Option<String>,
    },
    /// Restore an older version as the current version.
    RestoreSecret {
        /// Vault key.
        name: String,
        /// Number of versions back.
        back: u8,
    },
    /// Delete a key and its versions.
    DeleteSecret(String),
    /// Answer or decline one live request.
    AnswerSecret {
        /// Request identity.
        id: ::secrets::RequestId,
        /// Field name → value, or `None` to decline.
        values: Option<HashMap<String, String>>,
        /// When every stored value expires (Unix ms); `None` = never.
        expires_ms: Option<u64>,
    },
}
/// Value-free owner replies and live bus events.
pub enum Msg {
    /// Metadata list.
    Listed(Result<::secrets::SecretsList, String>),
    /// Newly current metadata.
    Set(Result<::secrets::SecretInfo, String>),
    /// Delete/answer completion.
    Done(Result<Option<String>, String>),
    /// Live request/change/resolution.
    Event(::secrets::SecretsEvent),
}
impl Secrets {
    fn request(&self, request: Request, cx: &mut Cx<'_, '_, Msg>) {
        cx.spawn_request(Lane::Concurrent, request, |api, request| async move {
            match request {
                Request::ListSecrets => Msg::Listed(api.list_secrets().await.map_err(crate::reason)),
                Request::SetSecret { name, value, note } => Msg::Set(api.set_secret(name, value, note).await.map_err(crate::reason)),
                Request::RestoreSecret { name, back } => Msg::Set(api.restore_secret(name, back).await.map_err(crate::reason)),
                Request::DeleteSecret(name) => Msg::Done(api.delete_secret(name.clone()).await.map(|_| Some(format!("{name}: deleted"))).map_err(crate::reason)),
                Request::AnswerSecret { id, values, expires_ms } => Msg::Done(api.answer_secret(id, None, values, expires_ms).await.map(|_| None).map_err(crate::reason)),
            }
        });
    }
    fn slash(&mut self, arg: &str, cx: &mut Cx<'_, '_, Msg>) {
        let (sub, rest) = arg.split_once(char::is_whitespace).map_or((arg, ""), |(s, r)| (s, r.trim()));
        let request = match (sub, rest) {
            ("", _) => { self.awaiting = true; Some(Request::ListSecrets) }
            ("add", "") => { self.prompt = Some(Prompt::Name); None }
            ("add", name) if ::secrets::vault::valid_name(name) => {
                self.prompt = Some(Prompt::Value { name: name.to_owned(), current: self.secret_current(name) }); None
            }
            ("add", name) => { cx.core(CoreAction::Reject(bad_secret_name(name))); None }
            ("rm", name) if !name.is_empty() => Some(Request::DeleteSecret(name.to_owned())),
            ("restore", rest) if !rest.is_empty() => {
                let (name, back) = rest.split_once(char::is_whitespace).map_or((rest, "1"), |(n, b)| (n, b.trim()));
                match back.parse::<u8>() {
                    Ok(back) if (1..::secrets::KEEP as u8).contains(&back) => Some(Request::RestoreSecret { name: name.to_owned(), back }),
                    _ => { cx.core(CoreAction::Reject(format!("/secrets restore: n is 1..={}", ::secrets::KEEP - 1))); None }
                }
            }
            _ => { cx.core(CoreAction::Reject("usage: /secrets [add [name] | rm <name> | restore <name> [n]]".into())); None }
        };
        if let Some(request) = request { self.request(request, cx); }
        if self.prompt.is_some() { cx.focus(FocusClass::Modal); }
    }
    fn secret_current(&self, name: &str) -> Option<u32> {
        self.secrets.as_ref()?.secrets.iter().find(|s| s.name == name).map(|s| s.current)
    }
    /// Metadata-only menu title and keys.
    pub fn secrets_title(&self) -> String {
        let key = self.secrets.as_ref().map_or("?", |l| l.key_store.as_str());
        match &self.menu {
            Some(Menu::Secrets { cursor, history: Some(_) }) => {
                let name = self.secrets.as_ref().and_then(|l| l.secrets.get(*cursor)).map_or("", |s| s.name.as_str());
                format!("secrets · {name} · history   ⏎ restore  h/esc back")
            }
            _ => format!("secrets · key: {key}   a add  r replace  h history  d delete  esc"),
        }
    }
    /// Metadata/history rows; values never enter presentation state.
    pub fn secrets_rows(&self) -> Vec<String> {
        let list = self.secrets.as_ref().map(|l| l.secrets.as_slice()).unwrap_or_default();
        if let Some(Menu::Secrets { cursor, history: Some(_) }) = &self.menu
            && let Some(s) = list.get(*cursor)
        {
            return s.versions.iter().enumerate().map(|(i, v)| format!("v{} · {} · {}{}", v.v, when(&v.created), v.source, if i == 0 { " · current" } else { "" })).collect();
        }
        if list.is_empty() { return vec!["no secrets · a adds one".into()] }
        let now = now_ms();
        list.iter().map(|s| {
            let expires = s.expires.as_deref().and_then(::secrets::expiry::from_rfc3339).map_or_else(String::new, |t| format!(" · {}", ::secrets::expiry::describe(t, now)));
            format!("{:<20} · v{} ({} kept) · {} · {}{expires}", s.name.chars().take(20).collect::<String>(), s.current, s.versions.len(), s.note, when(&s.updated))
        }).collect()
    }
    fn key(&mut self, code: KeyCode, cx: &mut Cx<'_, '_, Msg>) {
        if !self.requests.is_empty() {
            if code == KeyCode::Esc { self.decline(cx); cx.consume_key(); }
            return;
        }
        if self.prompt.is_some() {
            if code == KeyCode::Esc { self.prompt = None; cx.close(); cx.consume_key(); cx.core(CoreAction::Notice("secrets: cancelled".into())); }
            return;
        }
        let Some(Menu::Secrets { cursor, history }) = &mut self.menu else { return };
        cx.consume_key();
        let list = self.secrets.as_ref().map(|l| l.secrets.as_slice()).unwrap_or_default();
        let info = list.get(*cursor);
        let name = info.map(|s| s.name.clone());
        let current = info.map(|s| s.current);
        let versions = info.map_or(0, |s| s.versions.len());
        match (code, *history) {
            (KeyCode::Up, Some(h)) => *history = Some(h.saturating_sub(1)),
            (KeyCode::Down, Some(h)) => *history = Some((h + 1).min(versions.saturating_sub(1))),
            (KeyCode::Esc | KeyCode::Char('h'), Some(_)) => *history = None,
            (KeyCode::Enter, Some(h)) if h > 0 => {
                if let (Some(name), Ok(back)) = (name, u8::try_from(h)) { self.request(Request::RestoreSecret { name, back }, cx); }
            }
            (KeyCode::Up, None) => *cursor = cursor.saturating_sub(1),
            (KeyCode::Down, None) => *cursor = (*cursor + 1).min(list.len().saturating_sub(1)),
            (KeyCode::Esc, None) => { self.menu = None; cx.close(); }
            (KeyCode::Char('h'), None) if info.is_some() => *history = Some(0),
            (KeyCode::Char('a'), _) => { self.menu = None; self.prompt = Some(Prompt::Name); }
            (KeyCode::Char('r'), _) => if let Some(name) = name { self.menu = None; self.prompt = Some(Prompt::Value { name, current }); },
            (KeyCode::Char('d'), _) => if let Some(name) = name { self.menu = None; self.prompt = Some(Prompt::Delete { name }); },
            _ => {}
        }
    }
    fn submit(&mut self, line: &str, cx: &mut Cx<'_, '_, Msg>) {
        if !self.requests.is_empty() { cx.consume_key(); self.live_submit(line, cx); return; }
        let Some(prompt) = self.prompt.take() else { return };
        cx.consume_key();
        let request = match prompt {
            Prompt::Name => {
                if ::secrets::vault::valid_name(line) { self.prompt = Some(Prompt::Value { name: line.to_owned(), current: self.secret_current(line) }); }
                else { self.prompt = Some(Prompt::Name); cx.core(CoreAction::Reject(bad_secret_name(line))); }
                None
            }
            Prompt::Value { name, .. } => { self.prompt = Some(Prompt::Note { name, value: line.to_owned() }); None }
            Prompt::Note { name, value } => Some(Request::SetSecret { name, value, note: (!line.is_empty()).then(|| line.to_owned()) }),
            Prompt::Delete { name } => {
                if line == name { Some(Request::DeleteSecret(name)) }
                else { cx.core(CoreAction::Notice(format!("secrets: kept {name}"))); None }
            }
        };
        if self.prompt.is_none() { cx.close(); }
        if let Some(request) = request { self.request(request, cx); }
    }
    /// One step of the front request: the next field's value, then the expiry
    /// (`30m` / `2h` / `3d` / `YYYY-MM-DD HH:MM` local; empty = never), which sends the answer.
    fn live_submit(&mut self, line: &str, cx: &mut Cx<'_, '_, Msg>) {
        let Some(r) = self.requests.front_mut() else { return };
        if r.field().is_some() {
            r.values.push(line.to_owned());
            return;
        }
        let now = now_ms();
        let expires_ms = if line.trim().is_empty() {
            None
        } else {
            match ::secrets::expiry::parse(line, now) {
                Ok(e) => Some(e.at(now)),
                Err(e) => { cx.core(CoreAction::Reject(e)); return; }
            }
        };
        let r = self.requests.pop_front().expect("front");
        let when = expires_ms.map_or_else(String::new, |t| format!("; expires {}", ::secrets::expiry::local_time(t)));
        cx.core(CoreAction::Notice(format!("{}: stored{when}", r.names())));
        let values = r.fields.iter().map(|f| f.name.clone()).zip(r.values).collect();
        self.request(Request::AnswerSecret { id: r.id, values: Some(values), expires_ms }, cx);
    }
    fn decline(&mut self, cx: &mut Cx<'_, '_, Msg>) {
        let Some(r) = self.requests.pop_front() else { return };
        cx.core(CoreAction::Notice(format!("{}: declined", r.names())));
        self.request(Request::AnswerSecret { id: r.id, values: None, expires_ms: None }, cx);
    }
    fn event(&mut self, event: ::secrets::SecretsEvent, cx: &mut Cx<'_, '_, Msg>) {
        use ::secrets::SecretsEvent::*;
        if cx.core.reasoning { cx.plain(Plain::Text("\x1b[0m\n".into())); }
        match event {
            Changed => self.refresh = self.secrets.is_some(),
            Requested { id, session, name, note, current, fields, expires, .. } => {
                cx.plain(Plain::Line(format!("secret requested: {name} — {note} (needs a TTY or another client)")));
                if !self.requests.iter().any(|r| r.id == id) && cx.core.owns_session(session) {
                    // A pre-79 engine sends the one field as `name` / `note` / `current` only.
                    let fields = if fields.is_empty() {
                        vec![::secrets::SecretRequestField { name, label: note.clone(), masked: true, current, note: None }]
                    } else { fields };
                    self.requests.push_back(SecretRequest { id, note, fields, expires, values: Vec::new() });
                }
            }
            Resolved { id, timed_out, .. } => if let Some(i) = self.requests.iter().position(|r| r.id == id) {
                let r = self.requests.remove(i).expect("position");
                if timed_out { cx.core(CoreAction::Notice(format!("{}: request timed out", r.names()))); }
            },
            Rotation(_) => {}
        }
    }
}
impl Component for Secrets {
    type Msg = Msg;
    fn declaration(&self) -> Declaration {
        Declaration { id: "secrets", interests: &[Interest { topic: "secrets.events", version: 1, variants: &[] }],
            slash: &[("/secrets", "[add [name]|rm <name>|restore <name> [n]]", "encrypted secrets the agent uses by name")], bindings: &[] }
    }
    fn decode(&self, event: &TopicEvent, _: &CoreView<'_>) -> Option<Msg> {
        if event.topic().is_some() && (event.topic() != Some("secrets.events") || event.version() != Some(1)) { return None }
        ::secrets::SecretsEvent::deserialize(event.event()).ok().map(Msg::Event)
    }
    fn update(&mut self, input: Input<'_, Msg>, cx: &mut Cx<'_, '_, Msg>) {
        match input {
            Input::Slash { args, .. } => self.slash(args, cx),
            Input::Key(key) => self.key(key.code, cx),
            Input::Submit(line) => self.submit(line, cx),
            Input::Message(Msg::Listed(result)) => match result {
                Err(error) => cx.error(error),
                Ok(list) => {
                    if cx.core.reasoning { cx.plain(Plain::Text("\x1b[0m\n".into())); }
                    cx.plain(Plain::Line(format!("key: {}", list.key_store)));
                    let now = now_ms();
                    for s in &list.secrets {
                        let expires = s.expires.as_deref().and_then(::secrets::expiry::from_rfc3339).map_or_else(String::new, |t| format!(" · {}", ::secrets::expiry::describe(t, now)));
                        cx.plain(Plain::Line(format!("{} · v{} ({} kept) · {} · {}{expires}", s.name, s.current, s.versions.len(), s.note, s.updated)));
                    }
                    if self.awaiting && self.menu.is_none() { self.menu = Some(Menu::Secrets { cursor: 0, history: None }); cx.focus(FocusClass::Modal); }
                    self.awaiting = false; self.secrets = Some(list);
                }
            },
            Input::Message(Msg::Set(result)) => match result {
                Err(error) => cx.error(error),
                Ok(info) => {
                    if cx.core.reasoning { cx.plain(Plain::Text("\x1b[0m\n".into())); }
                    let text = format!("{}: v{} set", info.name, info.current);
                    cx.plain(Plain::Line(text.clone())); cx.core(CoreAction::Notice(text));
                }
            },
            Input::Message(Msg::Done(result)) => match result {
                Err(error) => cx.error(error),
                Ok(Some(text)) => {
                    if cx.core.reasoning { cx.plain(Plain::Text("\x1b[0m\n".into())); }
                    cx.plain(Plain::Line(text.clone())); cx.core(CoreAction::Notice(text));
                }
                Ok(None) => {}
            },
            Input::Message(Msg::Event(event)) => self.event(event, cx),
            Input::Tick { .. } => if std::mem::take(&mut self.refresh) && cx.core.interactive { self.request(Request::ListSecrets, cx); },
            Input::Lifecycle(Lifecycle::Prompts(value)) => if let Some(rows) = value["secrets"].as_array() {
                self.requests.clear();
                for row in rows { if let Ok(event) = ::secrets::SecretsEvent::deserialize(row) { self.event(event, cx); } }
            },
            Input::Lifecycle(Lifecycle::CommandFailed(_)) => self.awaiting = false,
            Input::Lifecycle(Lifecycle::FocusLost) => { self.awaiting = false; self.menu = None; self.prompt = None; }
            Input::Lifecycle(Lifecycle::SessionReset) => { self.awaiting = false; self.menu = None; self.prompt = None; self.requests.clear(); }
            Input::Lifecycle(Lifecycle::EngineChanged | Lifecycle::Resync) => {
                self.awaiting = false; self.refresh = self.secrets.is_some();
                if self.menu.is_some() || self.prompt.is_some() { cx.focus(FocusClass::Modal); }
            }
            _ => {}
        }
    }
    fn captures(&self, class: FocusClass) -> bool { class == FocusClass::Secret && !self.requests.is_empty() }
    fn editor(&self) -> Option<Editor> {
        if let Some(r) = self.requests.front() {
            return Some(match r.field() {
                Some(f) => Editor { hint: format!("{}  {}  ({}/{}; esc declines)", f.label, f.name, r.values.len() + 1, r.fields.len()), masked: f.masked, submit_empty: false },
                None => Editor { hint: "expires  30m · 2h · 3d · YYYY-MM-DD HH:MM  (⏎ never; esc declines)".into(), masked: false, submit_empty: true },
            });
        }
        let prompt = self.prompt.as_ref()?;
        Some(Editor { hint: prompt.hint(), masked: prompt.masked(), submit_empty: matches!(prompt, Prompt::Note { .. }) })
    }
    fn view(&self, at: Surface<'_>, vc: &ViewCx<'_>) -> Option<View> {
        match at {
            Surface::Prompt(FocusClass::Secret) => {
                let r = self.requests.front()?;
                let head = r.note.clone();
                let step = match r.field() {
                    Some(f) => {
                        let version = f.current.map_or_else(|| "new secret".to_owned(), |v| format!("adds v{}; v{v} stays as {{{{secret:{}:-1}}}}", v + 1, f.name));
                        format!("{} ({}) · {version}", f.label, f.name)
                    }
                    None => match r.expires.as_deref() {
                        Some(s) => match ::secrets::expiry::from_rfc3339(s) {
                            Some(t) => {
                                let typed = chrono::DateTime::from_timestamp_millis(t as i64).map(|d| d.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default();
                                format!("expiry · the agent suggests {} — type {typed} to accept, ⏎ for never", ::secrets::expiry::local_time(t))
                            }
                            None => {
                                let at = ::secrets::expiry::parse(s, now_ms()).map(|e| ::secrets::expiry::local_time(e.at(now_ms()))).unwrap_or_default();
                                format!("expiry · the agent suggests {s} (until {at}) — type {s} to accept, ⏎ for never")
                            }
                        },
                        None => "expiry · ⏎ keeps it until you delete it".to_owned(),
                    },
                };
                let keys = if r.field().is_some() { "type it below · [⏎] next  [esc] decline" } else { "30m · 2h · 3d · YYYY-MM-DD HH:MM · [⏎] store  [esc] decline" };
                Some(View::IntrinsicElement { element: element! {
                    BoxView(border_style: BorderStyle::Double, border_color: FAIENCE, flex_direction: FlexDirection::Column, padding_left: 1, padding_right: 1) {
                        Text(content: "secret requested", weight: Weight::Bold)
                        Text(content: head, color: FAIENCE, wrap: TextWrap::Wrap)
                        Text(content: step, color: BONE_DIM, wrap: TextWrap::Wrap)
                        Text(content: keys, color: FAIENCE)
                    }
                }.into_any(), rows: 0, replace: false })
            }
            Surface::Modal => {
                let Some(Menu::Secrets { cursor, history }) = self.menu else { return None };
                Some(View::IntrinsicElement { element: crate::ui::menu_view(self.secrets_title(), self.secrets_rows(), history.unwrap_or(cursor), vc.width as u16, vc.height as u16, false), rows: 0, replace: false })
            }
            _ => None,
        }
    }
    fn wake(&self) -> Option<u64> { self.refresh.then_some(0) }
}
fn bad_secret_name(name: &str) -> String { format!("secrets: {name:?} is not a name; use [A-Za-z0-9_.-], at most 64") }
fn now_ms() -> u64 { u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0) }

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Command, Model, Output};
    use serde_json::json;

    #[test]
    fn empty_secret_snapshot_clears_stale_request_without_losing_approval_or_ask_or_echoing_value() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let session = SessionId::new();
        let mut model = Model::new("model".into(), true);
        model.components = crate::components::Registry::empty();
        model.components.register(Secrets::default()).unwrap();
        model.session = Some(session);
        model.mount_components(&tx);
        let approval = tools::ApprovalRequested { id: tools::ApprovalId(uuid::Uuid::now_v7()), session, tool: "shell".into(), args: json!({"command":"echo hi"}), risk: tools::Risk::ReadOnly };
        let ask = tools::AskRequested { id: tools::AskId(uuid::Uuid::now_v7()), session, questions: vec![tools::Question { id: "q".into(), question: "Which?".into(), options: vec![], recommended: None }] };
        let secret = json!({"type":"SecretRequested","id":uuid::Uuid::now_v7(),"session":session,"name":"token","note":"API","current":null});
        let mut plain = String::new();
        model.receive(UiEvent::Wire(TopicEvent::control(json!({
            "type":"PendingPrompts", "approvals":[approval], "asks":[ask], "secrets":[secret]
        }))), &mut |output| {
            if let Output::Plain(Plain::Text(text) | Plain::Line(text)) = output { plain.push_str(text); }
        });
        let value = "never-echo-this-request-value";
        model.component_submit(value);
        assert!(rx.try_recv().is_err(), "the expiry step comes before the answer");
        model.component_submit("");
        let command = rx.try_recv().unwrap();
        assert!(!format!("{command:?}").contains(value));
        let Command::Component(effect) = command else { panic!("expected request effect") };
        assert!(matches!(effect.request::<Request>(), Some(Request::AnswerSecret { values: Some(v), expires_ms: None, .. }) if v.get("token").map(String::as_str) == Some(value)));
        model.apply(UiEvent::Wire(crate::components::tests::topic("secrets.events", 1, secret)));
        assert_eq!(model.components.get::<Secrets>().unwrap().requests.len(), 1);
        model.receive(UiEvent::Wire(TopicEvent::control(json!({"type":"PendingPrompts","secrets":[]}))), &mut |output| {
            if let Output::Plain(Plain::Text(text) | Plain::Line(text)) = output { plain.push_str(text); }
        });
        assert!(model.components.get::<Secrets>().unwrap().requests.is_empty());
        assert_eq!(model.approvals.iter().map(|r| r.id).collect::<Vec<_>>(), [approval.id]);
        assert_eq!(model.asks.iter().map(|r| r.req.id).collect::<Vec<_>>(), [ask.id]);
        assert!(!plain.contains(value));
        assert!(!format!("{model:?}").contains(value));
    }

    #[test]
    fn a_form_takes_every_field_then_an_expiry_and_sends_them_together() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let session = SessionId::new();
        let mut model = Model::new("model".into(), true);
        model.components = crate::components::Registry::empty();
        model.components.register(Secrets::default()).unwrap();
        model.session = Some(session);
        model.mount_components(&tx);
        let secret = json!({"type":"SecretRequested","id":uuid::Uuid::now_v7(),"session":session,"name":"gh.user","note":"GitHub","current":null,
            "title":"GitHub","expires":"2h","fields":[
                {"name":"gh.user","label":"Username","masked":false,"current":null},
                {"name":"gh.pass","label":"Password","masked":true,"current":3}]});
        model.apply(UiEvent::Wire(crate::components::tests::topic("secrets.events", 1, secret)));
        let editor = |m: &Model| m.components.get::<Secrets>().unwrap().editor().unwrap();
        assert!(!editor(&model).masked, "a username is typed in the clear");
        model.component_submit("alice");
        assert!(editor(&model).masked);
        model.component_submit("hunter22-password");
        assert!(editor(&model).submit_empty && editor(&model).hint.starts_with("expires"));
        model.component_submit("0m");
        assert!(rx.try_recv().is_err(), "a zero expiry is rejected inline");
        model.component_submit("2030-01-01 00:00");
        let before = now_ms();
        let Command::Component(effect) = rx.try_recv().unwrap() else { panic!("expected request effect") };
        let Some(Request::AnswerSecret { values: Some(values), expires_ms: Some(at), .. }) = effect.request::<Request>() else { panic!("expected an answer") };
        assert_eq!(values.get("gh.user").map(String::as_str), Some("alice"));
        assert_eq!(values.get("gh.pass").map(String::as_str), Some("hunter22-password"));
        assert_eq!(*at, ::secrets::expiry::parse("2030-01-01 00:00", before).unwrap().at(before));
        assert!(model.components.get::<Secrets>().unwrap().requests.is_empty());
    }
}
