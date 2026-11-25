//! Vault metadata, external connections (221 §9.3), Bitwarden Sends (§12.5), masked
//! administration wizard and the intrinsic live-request / unlock overlay.
use super::*;
use std::collections::{HashMap, VecDeque};
use serde::Deserialize;
use zeroize::Zeroizing;
use rpc_compose::api::clients::*;
use iocraft::prelude::{element, View as BoxView, Text, BorderStyle, FlexDirection, Weight, TextWrap};
use ::secrets::connections::{AskInfo, BwLogin, BwServer, ConnConfig, ConnInfo, DestinationView};
use ::secrets::external::{ConnStatus, DeleteMode, ExtEntry, ListPage, ListQuery, LoginForm, LoginStep, SsoBegin, UnlockRequestView};
use ::secrets::tools_send::SendRecord;
use crate::model::{now_ms, when};
use crate::ui::{FAIENCE, BONE_DIM};

/// Global metadata is retained across session resets; pending input is not.
#[derive(Default)]
pub struct Secrets {
    pub(crate) secrets: Option<::secrets::SecretsList>,
    /// 221 §9.3: this engine's external connections, listed with the local vault.
    pub(crate) connections: Vec<ConnInfo>,
    /// Why the connections could not be listed (an engine without them).
    conn_error: Option<String>,
    awaiting: bool,
    pub(crate) refresh: bool,
    pub(crate) menu: Option<Menu>,
    pub(crate) prompt: Option<Prompt>,
    pub(crate) requests: VecDeque<SecretRequest>,
    /// 221 §9.4: unlock cards; engine-wide, shown before any request.
    pub(crate) unlocks: VecDeque<UnlockCard>,
    /// A password login in flight: the form is re-sent with the second factor.
    login: Option<PendingLogin>,
    /// A Bitwarden connection being added (name, login method, email): its login prompt opens
    /// only once the add succeeded.
    adding: Option<(String, BwLogin, String)>,
    /// Unlock cards answered here with a password, until the engine says how it went.
    answered: Vec<(::secrets::RequestId, String)>,
    /// `create_send` calls in flight: an `ask` password card they raise has no session.
    sending: usize,
    /// The unlock cards held when `list_unlock_requests` was sent; only these can be stale.
    reseed: Vec<::secrets::RequestId>,
    /// §9.6: an SSO login waiting on its second factor (the pasted code is sent again).
    sso: Option<PendingSso>,
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
    /// 221 §7.3: where the answer goes; the TUI shows it and cannot change it.
    destination: Option<DestinationView>,
    /// Typed so far, one per field in order; the expiry step follows the last.
    values: Vec<Zeroizing<String>>,
}
impl SecretRequest {
    /// The field being typed; `None` = the expiry step.
    fn field(&self) -> Option<&::secrets::SecretRequestField> { self.fields.get(self.values.len()) }
    fn names(&self) -> String { self.fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>().join(", ") }
    /// 221 §9.2: expiry applies only to local copies.
    fn expiry_step(&self) -> bool { self.destination.as_ref().is_none_or(|d| d.destination.keep_local) }
}
/// 221 §9.4: `ExternalUnlockRequested`, answered with `answer_unlock`.
pub(crate) struct UnlockCard {
    pub(crate) id: ::secrets::RequestId,
    conn: String,
    backend: String,
    account: String,
    reason: String,
    /// Takes the editor now. A card that arrives while a prompt or request is mid-line waits
    /// for that line, so text typed for one is never sent as the other.
    armed: bool,
}
impl UnlockCard {
    fn target(&self) -> String { format!("{}/{}", self.backend, self.conn) }
}
// No Debug: the master password and the second factor wait here.
struct PendingLogin {
    email: String,
    password: Zeroizing<String>,
    two_factor: Option<(String, Zeroizing<String>)>,
}
impl PendingLogin {
    fn form(&self, new_device_otp: Option<String>) -> LoginForm {
        LoginForm::Password {
            email: self.email.clone(),
            password: self.password.to_string(),
            two_factor: self.two_factor.as_ref().map(|(p, c)| (p.clone(), c.to_string())),
            new_device_otp,
        }
    }
}
// No Debug: the pasted redirect address carries a one-time code.
struct PendingSso {
    state: String,
    code: Zeroizing<String>,
}
pub(crate) enum Menu {
    /// Connection rows first, then the local vault; `cursor` spans both.
    Secrets { cursor: usize, history: Option<usize> },
    /// One connection's entries (metadata only); `deleted`: its trash / pending deletions.
    Browse { conn: String, back: usize, cursor: usize, entries: Vec<ExtEntry>, next: Option<String>, deleted: bool },
    /// A Bitwarden connection's Sends (never a link).
    Sends { conn: String, back: usize, cursor: usize, sends: Vec<SendRecord> },
}
/// What a finished action reopens (the list it changed).
#[derive(Clone)]
pub enum Reopen {
    /// A connection's entries.
    Browse {
        /// Connection name.
        conn: String,
        /// The deleted view.
        deleted: bool,
    },
    /// A connection's Sends.
    Sends(String),
}
/// §12.5: what a Send shares.
pub(crate) enum SendWhat {
    /// Text: a local vault name or an external reference.
    Text(String),
    /// A file: an absolute path on the engine.
    File(String),
}
/// The §12.3 options, one prompt step each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SendStep { Conn, FileName, Name, Expires, Delete, MaxOpens, Password, HideEmail, Notes }
const SEND_STEPS: [SendStep; 9] = [
    SendStep::Conn, SendStep::FileName, SendStep::Name, SendStep::Expires, SendStep::Delete,
    SendStep::MaxOpens, SendStep::Password, SendStep::HideEmail, SendStep::Notes,
];
/// A Send being described; nothing in it is secret (a password is asked on a masked card).
pub(crate) struct SendDraft {
    /// Empty until the connection step.
    conn: String,
    /// The Bitwarden connections offered at that step.
    choices: Vec<String>,
    what: SendWhat,
    file_name: Option<String>,
    name: Option<String>,
    expires: Option<String>,
    delete: Option<String>,
    max_opens: Option<u32>,
    /// `{"ask": true}` | `{"from": NAME}`.
    password: Option<serde_json::Value>,
    hide_email: bool,
    notes: Option<String>,
}
impl SendDraft {
    fn new(conn: String, choices: Vec<String>, what: SendWhat) -> Self {
        Self { conn, choices, what, file_name: None, name: None, expires: None, delete: None, max_opens: None, password: None, hide_email: true, notes: None }
    }
    fn applies(&self, step: SendStep) -> bool {
        match step {
            SendStep::Conn => self.conn.is_empty(),
            SendStep::FileName => matches!(self.what, SendWhat::File(_)),
            _ => true,
        }
    }
    fn first(&self) -> Option<SendStep> { SEND_STEPS.into_iter().find(|s| self.applies(*s)) }
    fn next(&self, step: SendStep) -> Option<SendStep> {
        SEND_STEPS.into_iter().skip_while(|s| *s != step).skip(1).find(|s| self.applies(*s))
    }
    fn label(&self) -> &str { match &self.what { SendWhat::Text(s) | SendWhat::File(s) => s } }
    fn basename(&self) -> String {
        std::path::Path::new(self.label()).file_name().map_or_else(|| self.label().to_owned(), |n| n.to_string_lossy().into_owned())
    }
    fn hint(&self, step: SendStep) -> String {
        let what = match &self.what { SendWhat::Text(s) => format!("send {s}"), SendWhat::File(p) => format!("send file {p}") };
        match step {
            SendStep::Conn if self.choices.is_empty() => format!("{what}  Bitwarden connection  (esc cancel)"),
            SendStep::Conn => format!("{what}  on  {}  (esc cancel)", self.choices.join(" · ")),
            SendStep::FileName => format!("{what}  file name the recipient sees  (⏎ {}; esc cancel)", self.basename()),
            SendStep::Name => format!("{what}  name shown to the recipient, never the secret  (⏎ Shared from theuth; esc cancel)"),
            SendStep::Expires => format!("{what}  link expires  30m · 2h · 3d · YYYY-MM-DD HH:MM  (⏎ 24h; esc cancel)"),
            SendStep::Delete => format!("{what}  Bitwarden deletes it  within 31 days  (⏎ when it expires; esc cancel)"),
            SendStep::MaxOpens => format!("{what}  how many opens  (⏎ 1; esc cancel)"),
            SendStep::Password => format!("{what}  access password  ask (typed on a masked card) · a vault name  (⏎ none; esc cancel)"),
            SendStep::HideEmail => format!("{what}  hide your email from the recipient  yes · no  (⏎ yes; esc cancel)"),
            SendStep::Notes => format!("{what}  private notes  (⏎ none; esc cancel)"),
        }
    }
    /// One step's answer; `Err` re-asks it.
    fn take(&mut self, step: SendStep, line: &str) -> Result<(), String> {
        let when = |line: &str| -> Result<Option<String>, String> {
            match opt(line) {
                None => Ok(None),
                Some(t) => ::secrets::expiry::normalize(&t, now_ms()).map(Some),
            }
        };
        match step {
            SendStep::Conn => match line.trim() {
                c if self.choices.iter().any(|x| x == c) || (self.choices.is_empty() && ::secrets::external::valid_conn(c)) => self.conn = c.to_owned(),
                _ if self.choices.is_empty() => return Err("secrets: a connection name".into()),
                _ => return Err(format!("secrets: one of {}", self.choices.join(", "))),
            },
            SendStep::FileName => self.file_name = opt(line),
            SendStep::Name => self.name = opt(line),
            SendStep::Expires => self.expires = when(line)?,
            SendStep::Delete => self.delete = when(line)?,
            SendStep::MaxOpens => self.max_opens = match opt(line) {
                None => None,
                Some(n) => match n.parse::<u32>() {
                    Ok(n) if n > 0 => Some(n),
                    _ => return Err("secrets: a whole number of opens, 1 or more".into()),
                },
            },
            SendStep::Password => self.password = match line.trim() {
                "" | "none" => None,
                "ask" => Some(serde_json::json!({ "ask": true })),
                name if ::secrets::vault::valid_name(name) => Some(serde_json::json!({ "from": name })),
                name => return Err(bad_secret_name(name)),
            },
            SendStep::HideEmail => self.hide_email = match line.trim() {
                "" | "yes" | "y" => true,
                "no" | "n" => false,
                _ => return Err("secrets: yes or no".into()),
            },
            SendStep::Notes => self.notes = opt(line),
        }
        Ok(())
    }
    /// `create_send`'s args (`SendCreateArgs` without `connection`).
    fn args(&self) -> serde_json::Value {
        let mut a = serde_json::Map::new();
        match &self.what {
            SendWhat::Text(s) => a.insert("source".into(), s.clone().into()),
            SendWhat::File(p) => a.insert("file".into(), p.clone().into()),
        };
        let mut put = |k: &str, v: Option<serde_json::Value>| { if let Some(v) = v { a.insert(k.into(), v); } };
        put("file_name", self.file_name.clone().map(Into::into));
        put("name", self.name.clone().map(Into::into));
        put("expires", self.expires.clone().map(Into::into));
        put("delete", self.delete.clone().map(Into::into));
        put("max_access_count", self.max_opens.map(Into::into));
        put("password", self.password.clone());
        put("hide_email", Some(self.hide_email.into()));
        put("notes", self.notes.clone().map(Into::into));
        serde_json::Value::Object(a)
    }
}
// No Debug: the note step owns the value until its one RPC submission.
pub(crate) enum Prompt {
    Name,
    Value { name: String, current: Option<u32> },
    Note { name: String, value: Zeroizing<String> },
    Delete { name: String },
    /// `/secrets connect aws|bw`: the connection's name.
    ConnName { backend: &'static str },
    AwsProfile { conn: String, profiles: Vec<String> },
    AwsRegion { conn: String, profile: Option<String> },
    AwsKms { conn: String, profile: Option<String>, region: String },
    AwsPrefix { conn: String, profile: Option<String>, region: String, kms_key_id: Option<String> },
    /// `us` · `eu` · a self-hosted base URL.
    BwServer { conn: String },
    BwEmail { conn: String, server: BwServer, base_url: Option<String> },
    BwMethod { conn: String, server: Option<(BwServer, Option<String>)>, email: String },
    ClientId { conn: String },
    ClientSecret { conn: String, client_id: String },
    /// Password login: the master password.
    MasterLogin { conn: String, email: String },
    TwoFactorProvider { conn: String, providers: Vec<String> },
    TwoFactorCode { conn: String, provider: String },
    NewDeviceOtp { conn: String },
    /// §9.6: the organisation's SSO identifier (`server` is set while adding).
    SsoOrg { conn: String, server: Option<(BwServer, Option<String>)>, email: String },
    /// §9.6: the address the browser landed on (it carries the one-time code).
    SsoPaste { conn: String, state: String },
    /// A locked connection's master password.
    Unlock { conn: String, account: String },
    /// The local name an external value is imported under.
    Import { conn: String, r#ref: String },
    ExtDelete { conn: String, r#ref: String, name: String, window: String },
    /// Which field of an entry `y` copies a reference to.
    RefField { conn: String, r#ref: String, keys: Vec<String> },
    /// §12.5: one option of a Send.
    Send(Box<SendDraft>, SendStep),
}
impl Prompt {
    pub(crate) fn hint(&self) -> String {
        match self {
            Self::Name => "secrets add  name  (esc cancel)".into(),
            Self::Value { name, current: None } => format!("secrets add  {name}  value  (esc cancel)"),
            Self::Value { name, current: Some(v) } => format!("secrets add  {name}  value  (replaces v{v} → v{}; esc cancel)", v + 1),
            Self::Note { name, .. } => format!("secrets add  {name}  note  (⏎ none; esc cancel)"),
            Self::Delete { name } => format!("secrets  delete {name}?  type the name to confirm  (esc cancel)"),
            Self::ConnName { backend } => format!("secrets connect {backend}  name  (esc cancel)"),
            Self::AwsProfile { conn, profiles } => {
                let some = profiles.iter().take(6).map(String::as_str).collect::<Vec<_>>().join(" · ");
                let more = if profiles.len() > 6 { " · …" } else { "" };
                format!("connect aws/{conn}  profile  ({some}{more}; ⏎ default chain; esc cancel)")
            }
            Self::AwsRegion { conn, .. } => format!("connect aws/{conn}  region  (eu-west-1 …; esc cancel)"),
            Self::AwsKms { conn, .. } => format!("connect aws/{conn}  KMS key for new secrets  (⏎ aws/secretsmanager; esc cancel)"),
            Self::AwsPrefix { conn, .. } => format!("connect aws/{conn}  name prefix to browse  (⏎ all; esc cancel)"),
            Self::BwServer { conn } => format!("connect bw/{conn}  server  us · eu · https://vault.example.org  (http:// only for localhost; esc cancel)"),
            Self::BwEmail { conn, .. } => format!("connect bw/{conn}  account email  (esc cancel)"),
            Self::BwMethod { conn, .. } => format!("login bw/{conn}  api_key (recommended) · password · sso  (⏎ api_key; Organisation requires SSO? choose sso or api key; esc cancel)"),
            Self::SsoOrg { conn, server: Some(_), .. } => format!("login bw/{conn}  SSO organisation identifier  (⏎ none, as on Vaultwarden; esc cancel)"),
            Self::SsoOrg { conn, server: None, .. } => format!("login bw/{conn}  SSO organisation identifier  (⏎ the saved one; esc cancel)"),
            Self::SsoPaste { conn, .. } => format!("login bw/{conn}  {SSO_HOWTO}  (esc cancel)"),
            Self::ClientId { conn } => format!("login bw/{conn}  client_id  (Account settings › Security › Keys; esc cancel)"),
            Self::ClientSecret { conn, .. } => format!("login bw/{conn}  client_secret  (esc cancel)"),
            Self::MasterLogin { conn, email } => format!("login bw/{conn} ({email})  master password  (esc cancel)"),
            Self::TwoFactorProvider { conn, providers } => format!("login bw/{conn}  two-step login  {}  (esc cancel)", providers.join(" · ")),
            Self::TwoFactorCode { conn, provider } => format!("login bw/{conn}  {provider} code  (esc cancel)"),
            Self::NewDeviceOtp { conn } => format!("login bw/{conn}  new-device code from your email  (esc cancel)"),
            Self::Unlock { conn, account } => format!("unlock bw/{conn} ({account})  master password  (⏎ unlock; esc cancel)"),
            Self::Import { r#ref, .. } => format!("import {ref}  local name  (esc cancel)"),
            Self::ExtDelete { name, window, .. } => format!("delete {name}? {window}  type the name to confirm  (esc cancel)"),
            Self::RefField { r#ref, keys, .. } => format!(
                "copy {ref}  field  {}  (⏎ the default: a login's password, a note's notes, an AWS secret's whole value; esc cancel)",
                keys.join(" · ")
            ),
            Self::Send(d, step) => d.hint(*step),
        }
    }
    pub(crate) fn masked(&self) -> bool {
        matches!(self, Self::Value { .. } | Self::ClientSecret { .. } | Self::MasterLogin { .. } | Self::TwoFactorCode { .. } | Self::NewDeviceOtp { .. } | Self::SsoPaste { .. } | Self::Unlock { .. })
    }
    fn submit_empty(&self) -> bool {
        match self {
            Self::Send(_, step) => *step != SendStep::Conn,
            _ => matches!(self, Self::Note { .. } | Self::AwsProfile { .. } | Self::AwsKms { .. } | Self::AwsPrefix { .. } | Self::BwMethod { .. } | Self::SsoOrg { .. } | Self::RefField { .. }),
        }
    }
}
/// §9.6: what the TUI user does with the authorize URL.
const SSO_HOWTO: &str = "Open this in your browser and sign in. The last page won't load; copy its address and paste it here.";
/// RPC input; deliberately opaque to Debug because set/answer/login/unlock carry raw values.
#[derive(Clone)]
pub enum Request {
    /// Read metadata only (the vault, then the connections).
    ListSecrets,
    /// Store one submitted value.
    SetSecret {
        /// Vault key.
        name: String,
        /// Value, never printed.
        value: Zeroizing<String>,
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
    /// Answer or decline one unlock card (221 §9.4).
    AnswerUnlock {
        /// Card identity.
        id: ::secrets::RequestId,
        /// `bw/personal`, for the outcome line.
        target: String,
        /// The master password, or `None` to decline.
        password: Option<Zeroizing<String>>,
    },
    /// The engine's AWS profiles for `/secrets connect aws`.
    AwsProfiles {
        /// The connection being added.
        conn: String,
    },
    /// Probe, then add a connection; a Bitwarden one continues with its login.
    AddConnection {
        /// Connection name.
        conn: String,
        /// Its config.
        cfg: ConnConfig,
    },
    /// One login step.
    Login {
        /// Connection name.
        conn: String,
        /// The masked card's values.
        form: LoginForm,
    },
    /// Unlock with the master password.
    Unlock {
        /// Connection name.
        conn: String,
        /// Master password, never printed.
        password: Zeroizing<String>,
    },
    /// Lock now.
    Lock(String),
    /// One page of a connection's entries.
    Browse {
        /// Connection name.
        conn: String,
        /// The previous page's `next`.
        cursor: Option<String>,
        /// The deleted entries instead.
        deleted: bool,
    },
    /// Copy a `{{secret:…}}` reference (never a value).
    CopyRef {
        /// The placeholder.
        reference: String,
        /// The list to reopen afterwards.
        reopen: Option<Reopen>,
    },
    /// Copy an external value into the local vault under `name`.
    Import {
        /// Connection name.
        conn: String,
        /// The entry's reference.
        r#ref: String,
        /// Local vault name.
        name: String,
    },
    /// Recoverable delete of an external entry.
    DeleteExternal {
        /// Connection name.
        conn: String,
        /// The entry's reference.
        r#ref: String,
        /// Its display name.
        name: String,
    },
    /// Restore a deleted external entry.
    RestoreExternal {
        /// Connection name.
        conn: String,
        /// The entry's reference.
        r#ref: String,
        /// Its display name.
        name: String,
    },
    /// A Bitwarden connection's Sends.
    ListSends(String),
    /// Revoke one Send.
    RevokeSend {
        /// Its connection (the list reopens).
        conn: String,
        /// Row id.
        id: String,
        /// Its link name, for the receipt.
        label: String,
    },
    /// Copy one Send's link to the terminal's clipboard (OSC 52); it never reaches the screen.
    CopyLink {
        /// Row id.
        id: String,
        /// Its link name, for the receipt.
        label: String,
    },
    /// The unlock cards waiting on the engine (§9.4), to reseed them on (re)connect.
    ListUnlocks,
    /// §9.6: start an SSO login; the authorize URL is shown and copied (OSC 52).
    SsoBegin {
        /// Connection name.
        conn: String,
        /// `None` = the saved identifier.
        org_identifier: Option<String>,
    },
    /// §9.6: finish it with the pasted redirect address (or bare code).
    SsoComplete {
        /// Connection name.
        conn: String,
        /// From `SsoBegin`.
        state: String,
        /// The pasted line, never printed.
        code: Zeroizing<String>,
        /// `(provider, code)` on the second round.
        two_factor: Option<(String, Zeroizing<String>)>,
    },
    /// §12.5: create a Send from the owner's prompts.
    CreateSend {
        /// Bitwarden connection.
        conn: String,
        /// `SendCreateArgs` without `connection`.
        args: serde_json::Value,
    },
}
/// Value-free owner replies and live bus events.
pub enum Msg {
    /// Metadata list and the connections.
    Listed(Result<::secrets::SecretsList, String>, Result<Vec<ConnInfo>, String>),
    /// Newly current metadata.
    Set(Result<::secrets::SecretInfo, String>),
    /// Delete/answer completion.
    Done(Result<Option<String>, String>),
    /// Live request/change/resolution.
    Event(::secrets::SecretsEvent),
    /// AWS profile names for the add flow.
    Profiles(String, Result<Vec<String>, String>),
    /// A connection was added (or refused at the probe).
    Added(Result<ConnInfo, String>),
    /// A login step's answer, with the connections re-listed after `Done`.
    LoggedIn {
        /// Connection name.
        conn: String,
        /// What the server asks next.
        step: Result<LoginStep, String>,
        /// The connections after a finished login.
        connections: Option<Vec<ConnInfo>>,
    },
    /// A connection after lock/unlock.
    Conn(Result<ConnInfo, String>),
    /// One page of entries: connection, deleted view, the cursor it followed, the page.
    Browsed(String, bool, Option<String>, Result<ListPage, String>),
    /// `answer_unlock` with a password: `false` = the card was no longer waiting.
    Unlocked {
        /// Card identity.
        id: ::secrets::RequestId,
        /// `bw/personal`.
        target: String,
        /// The RPC's answer.
        result: Result<bool, String>,
    },
    /// An action's reply, then the list it changed reopens.
    Then {
        /// The reply.
        result: Box<Msg>,
        /// What to reopen.
        reopen: Reopen,
    },
    /// A connection's Sends.
    Sends(String, Result<Vec<SendRecord>, String>),
    /// A created Send (never its link).
    SendCreated(Result<SendRecord, String>),
    /// The engine's waiting unlock cards.
    Unlocks(Result<Vec<UnlockRequestView>, String>),
    /// §9.6: an SSO login started; `copied` = the URL went to the clipboard.
    SsoBegun {
        /// Connection name.
        conn: String,
        /// The engine's answer.
        begin: Result<SsoBegin, String>,
        /// OSC 52 written.
        copied: bool,
    },
}
/// `prod · aws · ready` status words (221 §9.1).
fn status_word(s: &ConnStatus) -> String {
    match s {
        ConnStatus::Ready => "ready".into(),
        ConnStatus::Locked => "locked".into(),
        ConnStatus::LoggedOut => "logged out".into(),
        ConnStatus::Expired { hint } => format!("expired ({hint})"),
        ConnStatus::Error(e) => format!("error: {e}"),
        ConnStatus::Unsupported(e) => format!("unsupported: {e}"),
    }
}
/// §9.3: `→ bw/personal › Personal/theuth · login "GitHub — CI"`.
fn destination_line(v: &DestinationView) -> String {
    let d = &v.destination;
    let mut line = format!("→ {}/{}", v.backend, d.connection);
    if v.backend == "bw" {
        let place = match (&d.org, &d.collection) {
            (Some(org), Some(c)) => format!("{org}/{c}"),
            (Some(org), None) => org.clone(),
            _ => format!("Personal/{}", d.folder.as_deref().unwrap_or("")).trim_end_matches('/').to_owned(),
        };
        let item = d.item.and_then(|k| serde_json::to_value(k).ok()).and_then(|k| k.as_str().map(str::to_owned)).unwrap_or_else(|| "login".into());
        line.push_str(&format!(" › {place} · {item} \"{}\"", d.name));
    } else {
        let format = d.format.and_then(|f| serde_json::to_value(f).ok()).and_then(|f| f.as_str().map(str::to_owned)).unwrap_or_else(|| "string".into());
        line.push_str(&format!(" › {} · {format}", d.name));
        if let Some(kms) = &d.kms_key_id { line.push_str(&format!(" · KMS {kms}")); }
    }
    if d.replace.is_some() { line.push_str(" · replaces"); }
    if d.keep_local { line.push_str(" · + local copy"); }
    if v.status == ConnStatus::Locked { line.push_str(" · locked: unlock first"); }
    line
}
/// The prompt's `us` · `eu` · base URL: `https://`, or `http://` to this machine only (a
/// master password never crosses a network in the clear).
fn parse_server(line: &str) -> Option<(BwServer, Option<String>)> {
    let line = line.trim();
    let loopback = |rest: &str| {
        let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
        let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
        let host = if host.starts_with('[') { host.split_inclusive(']').next().unwrap_or_default() } else { host.split(':').next().unwrap_or_default() };
        matches!(host, "localhost" | "[::1]") || host.starts_with("127.")
    };
    let self_hosted = |url: &str| Some((BwServer::SelfHosted, Some(url.trim_end_matches('/').to_owned())));
    match line {
        "us" => Some((BwServer::Us, None)),
        "eu" => Some((BwServer::Eu, None)),
        url if url.len() > "https://".len() && url.starts_with("https://") => self_hosted(url),
        url if url.strip_prefix("http://").is_some_and(loopback) => self_hosted(url),
        _ => None,
    }
}
fn opt(line: &str) -> Option<String> { let t = line.trim(); (!t.is_empty()).then(|| t.to_owned()) }
/// `send-personal-3fa9c2d1 · text · GITHUB_TOKEN · expires … · opened 0/1 · password no · active`.
fn send_row(s: &SendRecord) -> String {
    let kind = match (&s.file_name, s.file_bytes) {
        (Some(n), Some(b)) => format!("{n} · {} KB", (b.max(0) as u64).div_ceil(1024)),
        (Some(n), None) => n.clone(),
        _ => s.kind.clone(),
    };
    let expires = s.expiration_ms.map_or_else(String::new, |t| format!(" · expires {}", ::secrets::expiry::local_time(t.max(0) as u64)));
    let max = s.max_access.map_or_else(|| "∞".to_owned(), |m| m.to_string());
    let status = serde_json::to_value(s.status).ok().and_then(|v| v.as_str().map(|s| s.replace('_', " "))).unwrap_or_default();
    format!("{} · {kind} · {}{expires} · opened {}/{max} · password {} · {status}",
        send_label(s), s.source_ref, s.access_count, if s.has_password { "yes" } else { "no" })
}
fn send_label(s: &SendRecord) -> String { s.link_secret.clone().unwrap_or_else(|| s.id.chars().take(8).collect()) }
/// OSC 52 "set clipboard": the terminal takes it and draws nothing. Inside tmux it is wrapped
/// in a passthrough.
fn osc52_sequence(text: &str, tmux: bool) -> Zeroizing<String> {
    use base64::Engine as _;
    let b64 = Zeroizing::new(base64::engine::general_purpose::STANDARD.encode(text.as_bytes()));
    let seq = Zeroizing::new(format!("\x1b]52;c;{}\x07", b64.as_str()));
    if tmux { Zeroizing::new(format!("\x1bPtmux;{}\x1b\\", seq.replace('\x1b', "\x1b\x1b"))) } else { seq }
}
/// Write [`osc52_sequence`] straight to the terminal: never through the transcript or the frame.
fn osc52(text: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    let seq = osc52_sequence(text, std::env::var_os("TMUX").is_some());
    #[cfg(unix)]
    let mut out = std::fs::OpenOptions::new().write(true).open("/dev/tty")?;
    #[cfg(not(unix))]
    let mut out = std::io::stdout();
    out.write_all(seq.as_bytes())?;
    out.flush()
}
impl Secrets {
    fn request(&mut self, request: Request, cx: &mut Cx<'_, '_, Msg>) {
        if matches!(request, Request::CreateSend { .. }) { self.sending += 1; }
        cx.spawn_request(Lane::Concurrent, request, |api, request| async move {
            match request {
                Request::ListSecrets => {
                    let list = api.list_secrets().await.map_err(crate::reason);
                    Msg::Listed(list, api.list_secret_connections().await.map_err(crate::reason))
                }
                Request::SetSecret { name, value, note } => Msg::Set(api.set_secret(name, value.to_string(), note).await.map_err(crate::reason)),
                Request::RestoreSecret { name, back } => Msg::Set(api.restore_secret(name, back).await.map_err(crate::reason)),
                Request::DeleteSecret(name) => Msg::Done(api.delete_secret(name.clone()).await.map(|_| Some(format!("{name}: deleted"))).map_err(crate::reason)),
                Request::AnswerSecret { id, values, expires_ms } => Msg::Done(api.answer_secret(id, None, values, expires_ms, None).await.map(|_| None).map_err(crate::reason)),
                Request::AnswerUnlock { id, target, password: Some(password) } => Msg::Unlocked {
                    id, target, result: api.answer_unlock(id, Some(password.to_string())).await.map_err(crate::reason),
                },
                Request::AnswerUnlock { id, password: None, .. } => Msg::Done(api.answer_unlock(id, None).await.map(|_| None).map_err(crate::reason)),
                Request::AwsProfiles { conn } => Msg::Profiles(conn, api.list_aws_profiles().await.map(|p| p.profiles).map_err(crate::reason)),
                Request::AddConnection { conn, cfg } => Msg::Added(async {
                    match api.probe_secret_connection(conn.clone(), cfg.clone()).await.map_err(crate::reason)? {
                        ConnStatus::Error(e) | ConnStatus::Unsupported(e) => Err(format!("{}/{conn}: {e}; not added", cfg.backend())),
                        _ => api.add_secret_connection(conn, cfg).await.map_err(crate::reason),
                    }
                }.await),
                Request::Login { conn, form } => {
                    let step = api.connection_login(conn.clone(), form).await.map_err(crate::reason);
                    let connections = match step {
                        Ok(LoginStep::Done) => api.list_secret_connections().await.ok(),
                        _ => None,
                    };
                    Msg::LoggedIn { conn, step, connections }
                }
                Request::Unlock { conn, password } => Msg::Conn(api.connection_unlock(conn, password.to_string()).await.map_err(crate::reason)),
                Request::Lock(conn) => Msg::Conn(api.connection_lock(conn).await.map_err(crate::reason)),
                Request::Browse { conn, cursor, deleted } => {
                    let append = cursor.clone();
                    let page = api.list_external_secrets(conn.clone(), ListQuery { cursor, deleted, ..ListQuery::default() }).await.map_err(crate::reason);
                    Msg::Browsed(conn, deleted, append, page)
                }
                Request::CopyRef { reference, reopen } => {
                    let done = Msg::Done(
                        arboard::Clipboard::new().and_then(|mut clip| clip.set_text(reference.clone()))
                            .map(|()| Some(format!("copied {reference}"))).map_err(|e| format!("could not copy the reference: {e}")),
                    );
                    match reopen { Some(reopen) => Msg::Then { result: Box::new(done), reopen }, None => done }
                }
                Request::Import { conn, r#ref, name } => Msg::Then {
                    result: Box::new(Msg::Set(api.import_external_secret(conn.clone(), r#ref, name, None).await.map_err(crate::reason))),
                    reopen: Reopen::Browse { conn, deleted: false },
                },
                Request::DeleteExternal { conn, r#ref, name } => Msg::Then {
                    result: Box::new(Msg::Done(
                        api.delete_external_secret(conn.clone(), r#ref, DeleteMode::Recoverable { days: None }).await.map(|o| Some(match o.recoverable_until {
                            Some(until) => format!("{name}: deleted; recoverable until {}", when(&until)),
                            None => format!("{name}: deleted"),
                        })).map_err(crate::reason),
                    )),
                    reopen: Reopen::Browse { conn, deleted: false },
                },
                Request::RestoreExternal { conn, r#ref, name } => Msg::Then {
                    result: Box::new(Msg::Done(api.restore_external_secret(conn.clone(), r#ref).await.map(|_| Some(format!("{name}: restored"))).map_err(crate::reason))),
                    reopen: Reopen::Browse { conn, deleted: true },
                },
                Request::ListSends(conn) => Msg::Sends(conn.clone(), api.list_sends(conn).await.map_err(crate::reason)),
                Request::RevokeSend { conn, id, label } => Msg::Then {
                    result: Box::new(Msg::Done(api.revoke_send(id).await.map(|()| Some(format!("revoked {label}"))).map_err(crate::reason))),
                    reopen: Reopen::Sends(conn),
                },
                Request::CopyLink { id, label } => Msg::Done(async {
                    let link = Zeroizing::new(api.copy_send_link(id).await.map_err(crate::reason)?);
                    osc52(&link).map_err(|e| format!("could not copy the link: {e}"))?;
                    Ok(Some(format!("{label}: link copied to the clipboard (OSC 52); it is never shown here")))
                }.await),
                Request::CreateSend { conn, args } => Msg::SendCreated(api.create_send(conn, args).await.map_err(crate::reason)),
                Request::ListUnlocks => Msg::Unlocks(api.list_unlock_requests().await.map_err(crate::reason)),
                Request::SsoBegin { conn, org_identifier } => {
                    let begin = api.connection_sso_begin(conn.clone(), org_identifier, None).await.map_err(crate::reason);
                    let copied = begin.as_ref().is_ok_and(|b| osc52(&b.authorize_url).is_ok());
                    Msg::SsoBegun { conn, begin, copied }
                }
                Request::SsoComplete { conn, state, code, two_factor } => {
                    let two_factor = two_factor.map(|(p, c)| (p, c.to_string()));
                    let step = api.connection_sso_complete(conn.clone(), state, code.to_string(), two_factor).await.map_err(crate::reason);
                    let connections = match step {
                        Ok(LoginStep::Done) => api.list_secret_connections().await.ok(),
                        _ => None,
                    };
                    Msg::LoggedIn { conn, step, connections }
                }
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
            ("connect", "aws") => { self.prompt = Some(Prompt::ConnName { backend: "aws" }); None }
            ("connect", "bw" | "bitwarden") => { self.prompt = Some(Prompt::ConnName { backend: "bw" }); None }
            ("send-file", path) if std::path::Path::new(path).is_absolute() => { self.start_send(SendWhat::File(path.to_owned()), None); None }
            ("send-file", _) => { cx.core(CoreAction::Reject("/secrets send-file <absolute path on this engine>".into())); None }
            _ => { cx.core(CoreAction::Reject("usage: /secrets [add [name] | rm <name> | restore <name> [n] | connect aws|bw | send-file <abs path>]".into())); None }
        };
        if let Some(request) = request { self.request(request, cx); }
        if self.prompt.is_some() { cx.focus(FocusClass::Modal); }
    }
    fn secret_current(&self, name: &str) -> Option<u32> {
        self.secrets.as_ref()?.secrets.iter().find(|s| s.name == name).map(|s| s.current)
    }
    fn local(&self) -> &[::secrets::SecretInfo] { self.secrets.as_ref().map(|l| l.secrets.as_slice()).unwrap_or_default() }
    fn conn(&self, conn: &str) -> Option<&ConnInfo> { self.connections.iter().find(|c| c.conn == conn) }
    /// The TUI is not the owner's press: say when the engine will put a card in front of it.
    fn card_notice(&self, conn: &str, asks: fn(&AskInfo) -> bool, what: &str, cx: &mut Cx<'_, '_, Msg>) {
        if let Some(c) = self.conn(conn).filter(|c| asks(&c.ask)) {
            cx.core(CoreAction::Notice(format!("{what}: {}/{conn} always asks, so this waits for approval on a card", c.backend)));
        }
    }
    /// §12.5: the Send prompts, on `prefer` when it is a Bitwarden connection.
    fn start_send(&mut self, what: SendWhat, prefer: Option<&str>) {
        let choices: Vec<String> = self.connections.iter().filter(|c| c.backend == "bw").map(|c| c.conn.clone()).collect();
        let conn = match prefer.filter(|p| choices.iter().any(|c| c == p)) {
            Some(p) => p.to_owned(),
            None if choices.len() == 1 => choices[0].clone(),
            None => String::new(),
        };
        let draft = SendDraft::new(conn, choices, what);
        let Some(step) = draft.first() else { return };
        self.menu = None;
        self.prompt = Some(Prompt::Send(Box::new(draft), step));
    }
    fn reopen(&mut self, reopen: Reopen, cx: &mut Cx<'_, '_, Msg>) {
        if self.menu.is_some() || self.prompt.is_some() { return; }
        match reopen {
            Reopen::Browse { conn, deleted } => self.request(Request::Browse { conn, cursor: None, deleted }, cx),
            Reopen::Sends(conn) => self.request(Request::ListSends(conn), cx),
        }
    }
    /// The front unlock card has the editor (see [`UnlockCard::armed`]).
    fn unlock_active(&self) -> bool {
        self.unlocks.front().is_some_and(|u| u.armed || (self.prompt.is_none() && self.requests.is_empty()))
    }
    /// Metadata-only menu title and keys.
    pub fn secrets_title(&self) -> String {
        let key = self.secrets.as_ref().map_or("?", |l| l.key_store.as_str());
        let n = self.connections.len();
        match &self.menu {
            Some(Menu::Secrets { cursor, history: Some(_) }) => {
                let name = cursor.checked_sub(n).and_then(|i| self.local().get(i)).map_or("", |s| s.name.as_str());
                format!("secrets · {name} · history   ⏎ restore  h/esc back")
            }
            Some(Menu::Secrets { cursor, history: None }) if *cursor < n => {
                let c = &self.connections[*cursor];
                let sends = if c.backend == "bw" { "  s sends" } else { "" };
                format!("secrets · {}/{} · {}   u unlock  l lock  ⏎ browse  t deleted{sends}  a add  esc", c.backend, c.conn, status_word(&c.status))
            }
            Some(Menu::Browse { conn, deleted: false, .. }) => {
                let backend = self.conn(conn).map_or("?", |c| c.backend.as_str());
                format!("secrets · {backend}/{conn}   y copy reference  i import  S send  d delete  t deleted  esc back")
            }
            Some(Menu::Browse { conn, deleted: true, .. }) => {
                let backend = self.conn(conn).map_or("?", |c| c.backend.as_str());
                format!("secrets · {backend}/{conn} · deleted   r restore  t entries  esc back")
            }
            Some(Menu::Sends { conn, .. }) => format!("secrets · bw/{conn} · Sends   y copy link  r revoke  esc back"),
            _ => format!("secrets · key: {key}   a add  r replace  h history  d delete  S send  esc"),
        }
    }
    /// Metadata/history rows; values and links never enter presentation state.
    pub fn secrets_rows(&self) -> Vec<String> {
        let n = self.connections.len();
        let list = self.local();
        match &self.menu {
            Some(Menu::Secrets { cursor, history: Some(_) }) => if let Some(s) = cursor.checked_sub(n).and_then(|i| list.get(i)) {
                return s.versions.iter().enumerate().map(|(i, v)| format!("v{} · {} · {}{}", v.v, when(&v.created), v.source, if i == 0 { " · current" } else { "" })).collect();
            },
            // §9.3: `ref · kind · place · rev`; no Reveal here (D25).
            Some(Menu::Browse { entries, next, deleted, .. }) => {
                let mut rows: Vec<String> = entries.iter().map(|e| {
                    let rev = e.versions.first().map_or_else(String::new, |v| v.changed.as_deref().map_or_else(|| v.id.chars().take(8).collect(), when));
                    let tail = match (&e.recoverable_until, deleted) {
                        (Some(until), _) => format!("recoverable until {}", when(until)),
                        (None, true) => "deleted".to_owned(),
                        (None, false) => "reveal in the desktop app".to_owned(),
                    };
                    format!("{} · {} · {} · {rev} · {tail}", e.r#ref, e.kind, e.location)
                }).collect();
                if rows.is_empty() { rows.push(if *deleted { "nothing deleted".into() } else { "no entries".into() }); }
                if next.is_some() { rows.push("more · ⏎ loads the next page".into()); }
                return rows;
            }
            Some(Menu::Sends { sends, .. }) => {
                let mut rows: Vec<String> = sends.iter().map(send_row).collect();
                if rows.is_empty() { rows.push("no Sends · S on a secret creates one".into()); }
                return rows;
            }
            _ => {}
        }
        let mut rows: Vec<String> = self.connections.iter().map(|c| format!("{} · {} · {} · {}", c.conn, c.backend, status_word(&c.status), c.summary)).collect();
        if let Some(e) = &self.conn_error { rows.push(format!("connections unavailable: {e}")); }
        if list.is_empty() { rows.push("no secrets · a adds one".into()); return rows; }
        let now = now_ms();
        rows.extend(list.iter().map(|s| {
            let expires = s.expires.as_deref().and_then(::secrets::expiry::from_rfc3339).map_or_else(String::new, |t| format!(" · {}", ::secrets::expiry::describe(t, now)));
            format!("{:<20} · v{} ({} kept) · {} · {}{expires}", s.name.chars().take(20).collect::<String>(), s.current, s.versions.len(), s.note, when(&s.updated))
        }));
        rows
    }
    fn key(&mut self, code: KeyCode, cx: &mut Cx<'_, '_, Msg>) {
        if self.unlock_active() {
            if code == KeyCode::Esc { self.decline_unlock(cx); cx.consume_key(); }
            return;
        }
        if code == KeyCode::Esc { for u in &mut self.unlocks { u.armed = true; } }
        if !self.requests.is_empty() {
            if code == KeyCode::Esc { self.decline(cx); cx.consume_key(); }
            return;
        }
        if self.prompt.is_some() {
            if code == KeyCode::Esc { self.prompt = None; self.login = None; self.sso = None; cx.close(); cx.consume_key(); cx.core(CoreAction::Notice("secrets: cancelled".into())); }
            return;
        }
        match self.menu {
            Some(Menu::Browse { .. }) => { cx.consume_key(); self.browse_key(code, cx); return }
            Some(Menu::Sends { .. }) => { cx.consume_key(); self.sends_key(code, cx); return }
            Some(Menu::Secrets { cursor, history: None }) if cursor < self.connections.len() => { cx.consume_key(); self.conn_key(code, cursor, cx); return }
            Some(Menu::Secrets { .. }) => {}
            None => return,
        }
        let n = self.connections.len();
        let total = n + self.local().len();
        let Some(Menu::Secrets { cursor, history }) = &mut self.menu else { return };
        cx.consume_key();
        let list = self.secrets.as_ref().map(|l| l.secrets.as_slice()).unwrap_or_default();
        let info = cursor.checked_sub(n).and_then(|i| list.get(i));
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
            (KeyCode::Down, None) => *cursor = (*cursor + 1).min(total.saturating_sub(1)),
            (KeyCode::Esc, None) => { self.menu = None; cx.close(); }
            (KeyCode::Char('h'), None) if info.is_some() => *history = Some(0),
            (KeyCode::Char('a'), _) => { self.menu = None; self.prompt = Some(Prompt::Name); }
            (KeyCode::Char('r'), _) => if let Some(name) = name { self.menu = None; self.prompt = Some(Prompt::Value { name, current }); },
            (KeyCode::Char('d'), _) => if let Some(name) = name { self.menu = None; self.prompt = Some(Prompt::Delete { name }); },
            (KeyCode::Char('S'), None) => if let Some(name) = name { self.start_send(SendWhat::Text(name), None); },
            _ => {}
        }
    }
    /// §9.3 keys on a connection row.
    fn conn_key(&mut self, code: KeyCode, cursor: usize, cx: &mut Cx<'_, '_, Msg>) {
        let total = self.connections.len() + self.local().len();
        let c = &self.connections[cursor];
        let (conn, target) = (c.conn.clone(), format!("{}/{}", c.backend, c.conn));
        match code {
            KeyCode::Up => self.menu = Some(Menu::Secrets { cursor: cursor.saturating_sub(1), history: None }),
            KeyCode::Down => self.menu = Some(Menu::Secrets { cursor: (cursor + 1).min(total.saturating_sub(1)), history: None }),
            KeyCode::Esc => { self.menu = None; cx.close(); }
            KeyCode::Char('a') => { self.menu = None; self.prompt = Some(Prompt::Name); }
            KeyCode::Char('l') => self.request(Request::Lock(conn), cx),
            KeyCode::Enter | KeyCode::Char('t') => match &c.status {
                ConnStatus::Ready => self.request(Request::Browse { conn, cursor: None, deleted: code != KeyCode::Enter }, cx),
                s => cx.core(CoreAction::Notice(format!("{target}: {}; u unlocks", status_word(s)))),
            },
            KeyCode::Char('s') => match (&c.status, c.backend.as_str()) {
                (_, b) if b != "bw" => cx.core(CoreAction::Notice(format!("{target}: Sends are Bitwarden's"))),
                (ConnStatus::Ready, _) => self.request(Request::ListSends(conn), cx),
                (s, _) => cx.core(CoreAction::Notice(format!("{target}: {}; u unlocks", status_word(s)))),
            },
            KeyCode::Char('u') => match (&c.status, c.backend.as_str()) {
                (ConnStatus::Ready, _) => cx.core(CoreAction::Notice(format!("{target}: already unlocked"))),
                (ConnStatus::Locked, _) => {
                    let account = c.summary.split(" · ").next().unwrap_or_default().to_owned();
                    self.menu = None; self.prompt = Some(Prompt::Unlock { conn, account });
                }
                // §9.6: an SSO connection logs in again with SSO (its stored identifier).
                (ConnStatus::LoggedOut, "bw") if c.login == Some(BwLogin::Sso) => { self.menu = None; self.request(Request::SsoBegin { conn, org_identifier: None }, cx); }
                (ConnStatus::LoggedOut, "bw") => {
                    let email = c.summary.split(" · ").next().unwrap_or_default().to_owned();
                    self.menu = None; self.prompt = Some(Prompt::BwMethod { conn, server: None, email });
                }
                (s, _) => cx.core(CoreAction::Notice(format!("{target}: {}", status_word(s)))),
            },
            _ => {}
        }
    }
    /// Probe and add a Bitwarden connection; its login opens on `Added(Ok)`.
    #[allow(clippy::too_many_arguments)]
    fn add_bw(&mut self, conn: String, server: BwServer, base_url: Option<String>, email: String, login: BwLogin, org_identifier: Option<String>, cx: &mut Cx<'_, '_, Msg>) -> Option<Request> {
        let cfg = ConnConfig::Bw { server, base_url, api_url: None, identity_url: None, web_vault_url: None, email: email.clone(), login, org_identifier };
        cx.core(CoreAction::Notice(format!("bw/{conn}: checking the server; the login follows")));
        self.adding = Some((conn.clone(), login, email));
        Some(Request::AddConnection { conn, cfg })
    }
    fn browse_key(&mut self, code: KeyCode, cx: &mut Cx<'_, '_, Msg>) {
        let Some(Menu::Browse { conn, back, cursor, entries, next, deleted }) = &mut self.menu else { return };
        let rows = entries.len().max(1) + usize::from(next.is_some());
        let entry = entries.get(*cursor).cloned();
        let (conn, deleted) = (conn.clone(), *deleted);
        match (code, deleted) {
            (KeyCode::Up, _) => *cursor = cursor.saturating_sub(1),
            (KeyCode::Down, _) => *cursor = (*cursor + 1).min(rows - 1),
            (KeyCode::Esc, _) => { let back = *back; self.menu = Some(Menu::Secrets { cursor: back, history: None }); }
            (KeyCode::Enter, _) if entry.is_none() && next.is_some() => { let cursor = next.clone(); self.request(Request::Browse { conn, cursor, deleted }, cx); }
            (KeyCode::Enter, false) if entry.is_some() => cx.core(CoreAction::Notice("values: reveal in the desktop app".into())),
            (KeyCode::Enter, true) if entry.is_some() => cx.core(CoreAction::Notice("r restores it".into())),
            (KeyCode::Char('t'), _) => self.request(Request::Browse { conn, cursor: None, deleted: !deleted }, cx),
            (KeyCode::Char('r'), true) => if let Some(e) = entry {
                self.card_notice(&conn, |a| a.write, &format!("restore {}", e.name), cx);
                self.request(Request::RestoreExternal { conn, r#ref: e.r#ref, name: e.name }, cx);
            },
            (KeyCode::Char('y'), false) => if let Some(e) = entry {
                if e.keys.is_empty() {
                    self.request(Request::CopyRef { reference: format!("{{{{secret:{}}}}}", e.r#ref), reopen: None }, cx);
                } else {
                    self.menu = None; self.prompt = Some(Prompt::RefField { conn, r#ref: e.r#ref, keys: e.keys });
                }
            },
            (KeyCode::Char('i'), false) => if let Some(e) = entry { self.menu = None; self.prompt = Some(Prompt::Import { conn, r#ref: e.r#ref }); },
            (KeyCode::Char('S'), false) => if let Some(e) = entry { self.start_send(SendWhat::Text(e.r#ref), Some(&conn)); },
            (KeyCode::Char('d'), false) => if let Some(e) = entry {
                let window = match self.conn(&conn) {
                    Some(c) if c.backend == "aws" => format!("recoverable for {} days", c.settings.delete_recovery_days),
                    _ => "it moves to the Bitwarden trash".to_owned(),
                };
                self.menu = None; self.prompt = Some(Prompt::ExtDelete { conn, r#ref: e.r#ref, name: e.name, window });
            },
            _ => {}
        }
    }
    /// §12.5 keys on a connection's Sends.
    fn sends_key(&mut self, code: KeyCode, cx: &mut Cx<'_, '_, Msg>) {
        let Some(Menu::Sends { conn, back, cursor, sends }) = &mut self.menu else { return };
        let send = sends.get(*cursor).map(|s| (s.id.clone(), send_label(s)));
        let conn = conn.clone();
        match code {
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Down => *cursor = (*cursor + 1).min(sends.len().saturating_sub(1)),
            KeyCode::Esc => { let back = *back; self.menu = Some(Menu::Secrets { cursor: back, history: None }); }
            KeyCode::Char('r') => if let Some((id, label)) = send {
                self.card_notice(&conn, |a| a.send == Some(true), &format!("revoke {label}"), cx);
                self.request(Request::RevokeSend { conn, id, label }, cx);
            },
            KeyCode::Char('y') => if let Some((id, label)) = send { self.request(Request::CopyLink { id, label }, cx); },
            _ => {}
        }
    }
    fn submit(&mut self, line: &str, cx: &mut Cx<'_, '_, Msg>) {
        if self.unlock_active() { cx.consume_key(); self.unlock_submit(line, cx); return; }
        // A card that arrived while this line was typed takes the next line, not this one.
        for u in &mut self.unlocks { u.armed = true; }
        if !self.requests.is_empty() { cx.consume_key(); self.live_submit(line, cx); return; }
        let Some(prompt) = self.prompt.take() else { return };
        cx.consume_key();
        let reject = |prompt: Prompt, why: String, me: &mut Self, cx: &mut Cx<'_, '_, Msg>| { me.prompt = Some(prompt); cx.core(CoreAction::Reject(why)); };
        let request = match prompt {
            Prompt::Name => {
                if ::secrets::vault::valid_name(line) { self.prompt = Some(Prompt::Value { name: line.to_owned(), current: self.secret_current(line) }); }
                else { reject(Prompt::Name, bad_secret_name(line), self, cx); }
                None
            }
            Prompt::Value { name, .. } => { self.prompt = Some(Prompt::Note { name, value: Zeroizing::new(line.to_owned()) }); None }
            Prompt::Note { name, value } => Some(Request::SetSecret { name, value, note: (!line.is_empty()).then(|| line.to_owned()) }),
            Prompt::Delete { name } => {
                if line == name { Some(Request::DeleteSecret(name)) }
                else { cx.core(CoreAction::Notice(format!("secrets: kept {name}"))); None }
            }
            Prompt::ConnName { backend } => {
                let conn = line.trim().to_owned();
                if !::secrets::external::valid_conn(&conn) { reject(Prompt::ConnName { backend }, format!("secrets: {conn:?} is not a connection name"), self, cx); None }
                else if self.connections.iter().any(|c| c.conn == conn) { reject(Prompt::ConnName { backend }, format!("secrets: {conn} exists"), self, cx); None }
                else if backend == "aws" { Some(Request::AwsProfiles { conn }) }
                else { self.prompt = Some(Prompt::BwServer { conn }); None }
            }
            Prompt::AwsProfile { conn, profiles } => match opt(line) {
                None => { self.prompt = Some(Prompt::AwsRegion { conn, profile: None }); None }
                Some(p) if profiles.contains(&p) => { self.prompt = Some(Prompt::AwsRegion { conn, profile: Some(p) }); None }
                Some(p) => { reject(Prompt::AwsProfile { conn, profiles }, format!("secrets: no AWS profile {p:?}"), self, cx); None }
            },
            Prompt::AwsRegion { conn, profile } => match opt(line) {
                Some(region) if region.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') => { self.prompt = Some(Prompt::AwsKms { conn, profile, region }); None }
                _ => { reject(Prompt::AwsRegion { conn, profile }, "secrets: a region is like eu-west-1".into(), self, cx); None }
            },
            Prompt::AwsKms { conn, profile, region } => { self.prompt = Some(Prompt::AwsPrefix { conn, profile, region, kms_key_id: opt(line) }); None }
            Prompt::AwsPrefix { conn, profile, region, kms_key_id } => Some(Request::AddConnection { conn, cfg: ConnConfig::Aws { profile, region, kms_key_id, prefix: opt(line) } }),
            Prompt::BwServer { conn } => match parse_server(line) {
                Some((server, base_url)) => { self.prompt = Some(Prompt::BwEmail { conn, server, base_url }); None }
                None => { reject(Prompt::BwServer { conn }, "secrets: us, eu or the server's https:// URL (http:// only for localhost)".into(), self, cx); None }
            },
            Prompt::BwEmail { conn, server, base_url } => match opt(line) {
                Some(email) if email.contains('@') => { self.prompt = Some(Prompt::BwMethod { conn, server: Some((server, base_url)), email }); None }
                _ => { reject(Prompt::BwEmail { conn, server, base_url }, "secrets: an email address".into(), self, cx); None }
            },
            Prompt::BwMethod { conn, server, email } => {
                // `Some(None)` = sso.
                let method = match line.trim() { "" | "api_key" | "api key" => Some(Some(BwLogin::ApiKey)), "password" => Some(Some(BwLogin::Password)), "sso" => Some(None), _ => None };
                match (method, server) {
                    (None, server) => { reject(Prompt::BwMethod { conn, server, email }, "secrets: api_key, password or sso".into(), self, cx); None }
                    // §9.6: the organisation identifier first; it belongs in the config.
                    (Some(None), server) => { self.prompt = Some(Prompt::SsoOrg { conn, server, email }); None }
                    // Adding: the connection must exist before its login (the login RPC needs it),
                    // so the login prompt opens on `Added(Ok)`; a refused add asks for nothing.
                    (Some(Some(login)), Some((server, base_url))) => self.add_bw(conn, server, base_url, email, login, None, cx),
                    (Some(Some(BwLogin::Password)), None) => { self.prompt = Some(Prompt::MasterLogin { conn, email }); None }
                    (Some(Some(_)), None) => { self.prompt = Some(Prompt::ClientId { conn }); None }
                }
            }
            Prompt::SsoOrg { conn, server, email } => match server {
                Some((server, base_url)) => self.add_bw(conn, server, base_url, email, BwLogin::Sso, opt(line), cx),
                None => Some(Request::SsoBegin { conn, org_identifier: opt(line) }),
            },
            Prompt::SsoPaste { conn, state } => match opt(line) {
                Some(pasted) => {
                    let code = Zeroizing::new(pasted);
                    self.sso = Some(PendingSso { state: state.clone(), code: code.clone() });
                    Some(Request::SsoComplete { conn, state, code, two_factor: None })
                }
                None => { reject(Prompt::SsoPaste { conn, state }, "secrets: paste the address the browser landed on".into(), self, cx); None }
            },
            Prompt::ClientId { conn } => match opt(line) {
                Some(client_id) => { self.prompt = Some(Prompt::ClientSecret { conn, client_id }); None }
                None => { reject(Prompt::ClientId { conn }, "secrets: the client_id".into(), self, cx); None }
            },
            Prompt::ClientSecret { conn, client_id } => Some(Request::Login { conn, form: LoginForm::ApiKey { client_id, client_secret: line.to_owned() } }),
            Prompt::MasterLogin { conn, email } => {
                let login = PendingLogin { email, password: Zeroizing::new(line.to_owned()), two_factor: None };
                let form = login.form(None);
                self.login = Some(login);
                Some(Request::Login { conn, form })
            }
            Prompt::TwoFactorProvider { conn, providers } => match providers.iter().find(|p| p.as_str() == line.trim()) {
                Some(provider) => { self.prompt = Some(Prompt::TwoFactorCode { conn, provider: provider.clone() }); None }
                None => { let why = format!("secrets: one of {}", providers.join(", ")); reject(Prompt::TwoFactorProvider { conn, providers }, why, self, cx); None }
            },
            Prompt::TwoFactorCode { conn, provider } => match (self.login.as_mut(), self.sso.as_ref()) {
                (Some(login), _) => { login.two_factor = Some((provider, Zeroizing::new(line.trim().to_owned()))); Some(Request::Login { conn, form: login.form(None) }) }
                // §9.6: the same code again, now with the second factor.
                (None, Some(sso)) => Some(Request::SsoComplete {
                    conn, state: sso.state.clone(), code: sso.code.clone(), two_factor: Some((provider, Zeroizing::new(line.trim().to_owned()))),
                }),
                (None, None) => None,
            },
            Prompt::NewDeviceOtp { conn } => self.login.as_ref().map(|login| Request::Login { conn, form: login.form(Some(line.trim().to_owned())) }),
            Prompt::Unlock { conn, .. } => Some(Request::Unlock { conn, password: Zeroizing::new(line.to_owned()) }),
            Prompt::Import { conn, r#ref } => {
                if ::secrets::vault::valid_name(line) {
                    self.card_notice(&conn, |a| a.read, &format!("import {ref} as {line}"), cx);
                    Some(Request::Import { conn, r#ref, name: line.to_owned() })
                }
                else { reject(Prompt::Import { conn, r#ref }, bad_secret_name(line), self, cx); None }
            }
            Prompt::ExtDelete { conn, r#ref, name, .. } => {
                if line == name {
                    self.card_notice(&conn, |a| a.delete, &format!("delete {name}"), cx);
                    Some(Request::DeleteExternal { conn, r#ref, name })
                }
                else { cx.core(CoreAction::Notice(format!("secrets: kept {name}"))); None }
            }
            Prompt::RefField { conn, r#ref, keys } => match opt(line) {
                None => Some(Request::CopyRef { reference: format!("{{{{secret:{ref}}}}}"), reopen: Some(Reopen::Browse { conn, deleted: false }) }),
                Some(k) if keys.contains(&k) => Some(Request::CopyRef { reference: format!("{{{{secret:{ref}#{k}}}}}"), reopen: Some(Reopen::Browse { conn, deleted: false }) }),
                Some(k) => { let why = format!("secrets: no field {k:?}; one of {}", keys.join(", ")); reject(Prompt::RefField { conn, r#ref, keys }, why, self, cx); None }
            },
            Prompt::Send(mut draft, step) => match draft.take(step, line) {
                Err(why) => { reject(Prompt::Send(draft, step), why, self, cx); None }
                Ok(()) => match draft.next(step) {
                    Some(next) => { self.prompt = Some(Prompt::Send(draft, next)); None }
                    None => {
                        let target = format!("bw/{}", draft.conn);
                        let password = if draft.password.as_ref().is_some_and(|p| p.get("ask").is_some()) { "; type its password on the masked card that follows" } else { "" };
                        cx.core(CoreAction::Notice(format!("send {}: creating on {target}{password}", draft.label())));
                        self.card_notice(&draft.conn, |a| a.send == Some(true), "the Send", cx);
                        Some(Request::CreateSend { conn: draft.conn.clone(), args: draft.args() })
                    }
                },
            },
        };
        if self.prompt.is_none() { cx.close(); }
        if let Some(request) = request { self.request(request, cx); }
    }
    /// One step of the front request: the next field's value, then the expiry
    /// (`30m` / `2h` / `3d` / `YYYY-MM-DD HH:MM` local; empty = never), which sends the answer.
    /// A request stored only in an external manager has no expiry step (221 §9.2).
    fn live_submit(&mut self, line: &str, cx: &mut Cx<'_, '_, Msg>) {
        let Some(r) = self.requests.front_mut() else { return };
        if r.field().is_some() {
            r.values.push(Zeroizing::new(line.to_owned()));
            if r.field().is_some() || r.expiry_step() { return; }
        }
        let now = now_ms();
        let expires_ms = if line.trim().is_empty() || !r.expiry_step() {
            None
        } else {
            match ::secrets::expiry::parse(line, now) {
                Ok(e) => Some(e.at(now)),
                Err(e) => { cx.core(CoreAction::Reject(e)); return; }
            }
        };
        let r = self.requests.pop_front().expect("front");
        let when = expires_ms.map_or_else(String::new, |t| format!("; expires {}", ::secrets::expiry::local_time(t)));
        let to = r.destination.as_ref().map_or_else(String::new, |d| format!(" → {}/{}", d.backend, d.destination.connection));
        cx.core(CoreAction::Notice(format!("{}: stored{to}{when}", r.names())));
        let values = r.fields.iter().map(|f| f.name.clone()).zip(r.values.iter().map(|v| v.to_string())).collect();
        self.request(Request::AnswerSecret { id: r.id, values: Some(values), expires_ms }, cx);
    }
    fn decline(&mut self, cx: &mut Cx<'_, '_, Msg>) {
        let Some(r) = self.requests.pop_front() else { return };
        cx.core(CoreAction::Notice(format!("{}: declined", r.names())));
        self.request(Request::AnswerSecret { id: r.id, values: None, expires_ms: None }, cx);
    }
    fn unlock_submit(&mut self, line: &str, cx: &mut Cx<'_, '_, Msg>) {
        let Some(u) = self.unlocks.pop_front() else { return };
        let target = u.target();
        cx.core(CoreAction::Notice(format!("{target}: unlocking")));
        self.answered.push((u.id, target.clone()));
        self.request(Request::AnswerUnlock { id: u.id, target, password: Some(Zeroizing::new(line.to_owned())) }, cx);
    }
    fn decline_unlock(&mut self, cx: &mut Cx<'_, '_, Msg>) {
        let Some(u) = self.unlocks.pop_front() else { return };
        let target = u.target();
        cx.core(CoreAction::Notice(format!("{target}: unlock declined")));
        self.request(Request::AnswerUnlock { id: u.id, target, password: None }, cx);
    }
    fn unlock_failed(target: &str, cx: &mut Cx<'_, '_, Msg>) {
        cx.core(CoreAction::Notice(format!("{target}: wrong password or already answered; u to retry")));
    }
    fn event(&mut self, event: ::secrets::SecretsEvent, cx: &mut Cx<'_, '_, Msg>) {
        use ::secrets::SecretsEvent::*;
        if cx.core.reasoning { cx.plain(Plain::Text("\x1b[0m\n".into())); }
        match event {
            Changed => self.refresh = self.secrets.is_some(),
            Requested { id, session, name, note, current, fields, expires, destination, .. } => {
                cx.plain(Plain::Line(format!("secret requested: {name} — {note} (needs a TTY or another client)")));
                // A Send created here with `password: ask` raises its card with no session.
                let ours = cx.core.owns_session(session) || (self.sending > 0 && session.0.is_nil());
                if !self.requests.iter().any(|r| r.id == id) && ours {
                    // A pre-79 engine sends the one field as `name` / `note` / `current` only.
                    let fields = if fields.is_empty() {
                        vec![::secrets::SecretRequestField { name, label: note.clone(), masked: true, current, note: None }]
                    } else { fields };
                    self.requests.push_back(SecretRequest { id, note, fields, expires, destination: destination.map(|d| *d), values: Vec::new() });
                }
            }
            Resolved { id, timed_out, .. } => if let Some(i) = self.requests.iter().position(|r| r.id == id) {
                let r = self.requests.remove(i).expect("position");
                if timed_out { cx.core(CoreAction::Notice(format!("{}: request timed out", r.names()))); }
            },
            // Engine-wide (not a session's): whoever is at a TTY may answer it.
            UnlockRequested { id, conn, backend, account, reason } => {
                cx.plain(Plain::Line(format!("unlock requested: {backend}/{conn} — {reason}")));
                if !self.unlocks.iter().any(|u| u.id == id) {
                    let armed = self.prompt.is_none() && self.requests.is_empty();
                    self.unlocks.push_back(UnlockCard { id, conn, backend, account, reason, armed });
                }
            }
            UnlockResolved { id, unlocked } => {
                if let Some(i) = self.unlocks.iter().position(|u| u.id == id) {
                    let u = self.unlocks.remove(i).expect("position");
                    cx.core(CoreAction::Notice(format!("{}: {}", u.target(), if unlocked { "unlocked" } else { "unlock card closed" })));
                } else if let Some(i) = self.answered.iter().position(|(a, _)| *a == id) {
                    let (_, target) = self.answered.remove(i);
                    if unlocked { cx.core(CoreAction::Notice(format!("{target}: unlocked"))); } else { Self::unlock_failed(&target, cx); }
                }
                self.refresh = self.secrets.is_some();
            }
            Rotation(_) => {}
        }
    }
    fn update_conn(&mut self, info: ConnInfo) {
        match self.connections.iter_mut().find(|c| c.conn == info.conn) {
            Some(c) => *c = info,
            None => self.connections.push(info),
        }
    }
}
impl Component for Secrets {
    type Msg = Msg;
    fn declaration(&self) -> Declaration {
        Declaration { id: "secrets", interests: &[Interest { topic: "secrets.events", version: 1, variants: &[] }],
            slash: &[("/secrets", "[add [name]|rm <name>|restore <name> [n]|connect aws|bw|send-file <abs path>]", "encrypted secrets the agent uses by name, AWS/Bitwarden connections and Bitwarden Sends")], bindings: &[] }
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
            Input::Message(Msg::Listed(result, connections)) => {
                match connections {
                    Ok(c) => { self.connections = c; self.conn_error = None; }
                    Err(e) => { self.connections.clear(); self.conn_error = Some(e); }
                }
                match result {
                    Err(error) => cx.error(error),
                    Ok(list) => {
                        if cx.core.reasoning { cx.plain(Plain::Text("\x1b[0m\n".into())); }
                        cx.plain(Plain::Line(format!("key: {}", list.key_store)));
                        for c in &self.connections {
                            cx.plain(Plain::Line(format!("{} · {} · {} · {}", c.conn, c.backend, status_word(&c.status), c.summary)));
                        }
                        let now = now_ms();
                        for s in &list.secrets {
                            let expires = s.expires.as_deref().and_then(::secrets::expiry::from_rfc3339).map_or_else(String::new, |t| format!(" · {}", ::secrets::expiry::describe(t, now)));
                            cx.plain(Plain::Line(format!("{} · v{} ({} kept) · {} · {}{expires}", s.name, s.current, s.versions.len(), s.note, s.updated)));
                        }
                        if self.awaiting && self.menu.is_none() { self.menu = Some(Menu::Secrets { cursor: 0, history: None }); cx.focus(FocusClass::Modal); }
                        self.awaiting = false; self.secrets = Some(list);
                    }
                }
            }
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
            Input::Message(Msg::Then { result, reopen }) => {
                self.update(Input::Message(*result), cx);
                self.reopen(reopen, cx);
            }
            Input::Message(Msg::Event(event)) => self.event(event, cx),
            Input::Message(Msg::Profiles(conn, result)) => match result {
                Err(error) => cx.error(error),
                Ok(profiles) => { self.prompt = Some(Prompt::AwsProfile { conn, profiles }); cx.focus(FocusClass::Modal); }
            },
            Input::Message(Msg::Added(result)) => match result {
                Err(error) => { self.adding = None; cx.error(error); }
                Ok(info) => {
                    cx.core(CoreAction::Notice(format!("{}/{}: added · {}", info.backend, info.conn, status_word(&info.status))));
                    if self.adding.as_ref().is_some_and(|(c, ..)| *c == info.conn) {
                        let (conn, login, email) = self.adding.take().expect("adding");
                        if self.prompt.is_none() && self.unlocks.is_empty() && self.requests.is_empty() {
                            match login {
                                BwLogin::ApiKey => self.prompt = Some(Prompt::ClientId { conn }),
                                BwLogin::Password => self.prompt = Some(Prompt::MasterLogin { conn, email }),
                                // The identifier was saved with the connection.
                                BwLogin::Sso => self.request(Request::SsoBegin { conn, org_identifier: None }, cx),
                            }
                            if self.prompt.is_some() { cx.focus(FocusClass::Modal); }
                        } else {
                            cx.core(CoreAction::Notice(format!("bw/{conn}: u on its row logs in")));
                        }
                    }
                    self.update_conn(info);
                }
            },
            Input::Message(Msg::LoggedIn { conn, step, connections }) => match step {
                Err(error) => {
                    self.login = None; self.sso = None;
                    // §9.6: a password login an organisation refuses.
                    if error.contains("sso_required") { cx.error(format!("bw/{conn}: organisation requires SSO? choose sso or api key")); } else { cx.error(error); }
                }
                Ok(LoginStep::Done) => {
                    self.login = None; self.sso = None;
                    if let Some(c) = connections { self.connections = c; }
                    let info = self.connections.iter().find(|c| c.conn == conn);
                    match info.map(|c| (&c.status, c.summary.split(" · ").next().unwrap_or_default().to_owned())) {
                        Some((ConnStatus::Locked, account)) => {
                            cx.core(CoreAction::Notice(format!("bw/{conn}: logged in; unlock it")));
                            self.prompt = Some(Prompt::Unlock { conn, account }); cx.focus(FocusClass::Modal);
                        }
                        Some((s, _)) => cx.core(CoreAction::Notice(format!("bw/{conn}: logged in · {}", status_word(s)))),
                        None => cx.core(CoreAction::Notice(format!("bw/{conn}: logged in"))),
                    }
                }
                Ok(LoginStep::NeedTwoFactor { providers }) if self.login.is_some() || self.sso.is_some() => {
                    self.prompt = Some(match providers.as_slice() {
                        [one] => Prompt::TwoFactorCode { conn, provider: one.clone() },
                        _ => Prompt::TwoFactorProvider { conn, providers },
                    });
                    cx.focus(FocusClass::Modal);
                }
                Ok(LoginStep::NeedNewDeviceOtp) if self.login.is_some() => { self.prompt = Some(Prompt::NewDeviceOtp { conn }); cx.focus(FocusClass::Modal); }
                Ok(_) => { self.login = None; self.sso = None; cx.error(format!("bw/{conn}: the server asked for another step; log in again")) }
            },
            Input::Message(Msg::SsoBegun { conn, begin, copied }) => match begin {
                Err(error) => cx.error(error),
                Ok(begin) => {
                    if cx.core.reasoning { cx.plain(Plain::Text("\x1b[0m\n".into())); }
                    let copied = if copied { " (copied to the clipboard)" } else { "" };
                    cx.plain(Plain::Line(format!("bw/{conn} SSO: open {}{copied}", begin.authorize_url)));
                    cx.core(CoreAction::Notice(format!("bw/{conn}: {SSO_HOWTO}")));
                    if self.prompt.is_none() {
                        self.prompt = Some(Prompt::SsoPaste { conn, state: begin.state });
                        cx.focus(FocusClass::Modal);
                    }
                }
            },
            Input::Message(Msg::Conn(result)) => match result {
                Err(error) => cx.error(error),
                Ok(info) => {
                    cx.core(CoreAction::Notice(format!("{}/{}: {}", info.backend, info.conn, status_word(&info.status))));
                    self.update_conn(info);
                }
            },
            Input::Message(Msg::Unlocked { id, target, result }) => match result {
                // The outcome follows as `UnlockResolved`.
                Ok(true) => {}
                Ok(false) => { self.answered.retain(|(a, _)| *a != id); Self::unlock_failed(&target, cx); }
                Err(error) => { self.answered.retain(|(a, _)| *a != id); cx.error(error); }
            },
            Input::Message(Msg::Browsed(conn, deleted, after, result)) => match result {
                Err(error) => cx.error(error),
                Ok(page) => match &mut self.menu {
                    Some(Menu::Browse { conn: open, entries, next, deleted: d, .. }) if after.is_some() && *open == conn && *d == deleted => {
                        entries.extend(page.entries); *next = page.next;
                    }
                    _ => {
                        let back = self.connections.iter().position(|c| c.conn == conn).unwrap_or(0);
                        self.menu = Some(Menu::Browse { conn, back, cursor: 0, entries: page.entries, next: page.next, deleted });
                        cx.focus(FocusClass::Modal);
                    }
                },
            },
            Input::Message(Msg::Sends(conn, result)) => match result {
                Err(error) => cx.error(error),
                Ok(sends) => {
                    let back = self.connections.iter().position(|c| c.conn == conn).unwrap_or(0);
                    self.menu = Some(Menu::Sends { conn, back, cursor: 0, sends });
                    cx.focus(FocusClass::Modal);
                }
            },
            // The engine's list is the truth: cards answered elsewhere go, missed ones come.
            Input::Message(Msg::Unlocks(result)) => match result {
                Err(error) => tracing::debug!(%error, "unlock cards not reseeded"),
                Ok(open) => {
                    // Only cards older than the request can be stale; a newer one stays.
                    let stale = std::mem::take(&mut self.reseed);
                    self.unlocks.retain(|u| open.iter().any(|v| v.id == u.id) || !stale.contains(&u.id));
                    for v in open {
                        if !self.unlocks.iter().any(|u| u.id == v.id) {
                            let armed = self.prompt.is_none() && self.requests.is_empty();
                            self.unlocks.push_back(UnlockCard { id: v.id, conn: v.conn, backend: v.backend, account: v.account, reason: v.reason, armed });
                        }
                    }
                }
            },
            Input::Message(Msg::SendCreated(result)) => {
                self.sending = self.sending.saturating_sub(1);
                match result {
                    Err(error) => cx.error(error),
                    Ok(s) => {
                        let expires = s.expiration_ms.map_or_else(String::new, |t| format!("; expires {}", ::secrets::expiry::local_time(t.max(0) as u64)));
                        let text = format!("created {} Send {} on bw/{}{expires}; /secrets › {} › s › y copies its link", s.kind, send_label(&s), s.conn, s.conn);
                        cx.plain(Plain::Line(text.clone())); cx.core(CoreAction::Notice(text));
                    }
                }
            }
            Input::Tick { .. } => if std::mem::take(&mut self.refresh) && cx.core.interactive { self.request(Request::ListSecrets, cx); },
            Input::Lifecycle(Lifecycle::Prompts(value)) => if let Some(rows) = value["secrets"].as_array() {
                self.requests.clear();
                for row in rows { if let Ok(event) = ::secrets::SecretsEvent::deserialize(row) { self.event(event, cx); } }
            },
            Input::Lifecycle(Lifecycle::CommandFailed(_)) => self.awaiting = false,
            Input::Lifecycle(Lifecycle::FocusLost) => { self.awaiting = false; self.menu = None; self.prompt = None; self.login = None; self.sso = None; self.adding = None; }
            Input::Lifecycle(Lifecycle::SessionReset) => { self.awaiting = false; self.menu = None; self.prompt = None; self.login = None; self.sso = None; self.adding = None; self.requests.clear(); }
            Input::Lifecycle(Lifecycle::EngineChanged | Lifecycle::Resync) => {
                self.awaiting = false; self.refresh = self.secrets.is_some();
                if matches!(input, Input::Lifecycle(Lifecycle::EngineChanged)) {
                    self.unlocks.clear(); self.login = None; self.sso = None; self.adding = None; self.answered.clear(); self.sending = 0;
                }
                if self.menu.is_some() || self.prompt.is_some() { cx.focus(FocusClass::Modal); }
                if cx.core.interactive {
                    self.reseed = self.unlocks.iter().map(|u| u.id).collect();
                    self.request(Request::ListUnlocks, cx);
                }
            }
            _ => {}
        }
    }
    fn captures(&self, class: FocusClass) -> bool { class == FocusClass::Secret && !(self.requests.is_empty() && self.unlocks.is_empty()) }
    fn editor(&self) -> Option<Editor> {
        if let Some(u) = self.unlocks.front().filter(|_| self.unlock_active()) {
            return Some(Editor { hint: format!("unlock {} ({})  master password  (esc cancels)", u.target(), u.account), masked: true, submit_empty: false });
        }
        if let Some(r) = self.requests.front() {
            return Some(match r.field() {
                Some(f) => Editor { hint: format!("{}  {}  ({}/{}; esc declines)", f.label, f.name, r.values.len() + 1, r.fields.len()), masked: f.masked, submit_empty: false },
                None => Editor { hint: "expires  30m · 2h · 3d · YYYY-MM-DD HH:MM  (⏎ never; esc declines)".into(), masked: false, submit_empty: true },
            });
        }
        let prompt = self.prompt.as_ref()?;
        Some(Editor { hint: prompt.hint(), masked: prompt.masked(), submit_empty: prompt.submit_empty() })
    }
    fn view(&self, at: Surface<'_>, vc: &ViewCx<'_>) -> Option<View> {
        match at {
            // §9.3: the unlock modal reuses the request modal's frame.
            Surface::Prompt(FocusClass::Secret) if self.unlock_active() => {
                let u = self.unlocks.front()?;
                let head = format!("unlock {} ({})", u.target(), u.account);
                let reason = u.reason.clone();
                Some(View::IntrinsicElement { element: element! {
                    BoxView(border_style: BorderStyle::Double, border_color: FAIENCE, flex_direction: FlexDirection::Column, padding_left: 1, padding_right: 1) {
                        Text(content: head, weight: Weight::Bold)
                        Text(content: reason, color: BONE_DIM, wrap: TextWrap::Wrap)
                        Text(content: "master password · ••••• below · [⏎] unlock  [esc] cancel", color: FAIENCE)
                    }
                }.into_any(), rows: 0, replace: false })
            }
            Surface::Prompt(FocusClass::Secret) => {
                let r = self.requests.front()?;
                let head = r.note.clone();
                let to = r.destination.as_ref().map(destination_line).unwrap_or_default();
                let step = match r.field() {
                    Some(f) if r.destination.as_ref().is_some_and(|d| !d.destination.keep_local) => format!("{} ({})", f.label, f.name),
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
                let keys = match (r.field().is_some(), r.expiry_step() || r.values.len() + 1 < r.fields.len()) {
                    (true, true) => "type it below · [⏎] next  [esc] decline",
                    (true, false) => "type it below · [⏎] store  [esc] decline",
                    (false, _) => "30m · 2h · 3d · YYYY-MM-DD HH:MM · [⏎] store  [esc] decline",
                };
                Some(View::IntrinsicElement { element: element! {
                    BoxView(border_style: BorderStyle::Double, border_color: FAIENCE, flex_direction: FlexDirection::Column, padding_left: 1, padding_right: 1) {
                        Text(content: "secret requested", weight: Weight::Bold)
                        Text(content: head, color: FAIENCE, wrap: TextWrap::Wrap)
                        #((!to.is_empty()).then(|| element!(Text(content: to.clone(), color: FAIENCE, wrap: TextWrap::Wrap))))
                        Text(content: step, color: BONE_DIM, wrap: TextWrap::Wrap)
                        Text(content: keys, color: FAIENCE)
                    }
                }.into_any(), rows: 0, replace: false })
            }
            Surface::Modal => {
                let highlighted = match &self.menu {
                    Some(Menu::Secrets { cursor, history }) => history.unwrap_or(*cursor),
                    Some(Menu::Browse { cursor, .. } | Menu::Sends { cursor, .. }) => *cursor,
                    None => return None,
                };
                Some(View::IntrinsicElement { element: crate::ui::menu_view(self.secrets_title(), self.secrets_rows(), highlighted, vc.width as u16, vc.height as u16, false), rows: 0, replace: false })
            }
            _ => None,
        }
    }
    fn wake(&self) -> Option<u64> { self.refresh.then_some(0) }
}
fn bad_secret_name(name: &str) -> String { format!("secrets: {name:?} is not a name; use [A-Za-z0-9_.-], at most 64") }

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
        let approval = tools::ApprovalRequested { id: tools::ApprovalId(uuid::Uuid::now_v7()), session, tool: "shell".into(), args: json!({"command":"echo hi"}), risk: tools::Risk::ReadOnly, human_only: false, always_ask: false };
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

    #[test]
    fn a_request_into_bitwarden_shows_its_destination_and_skips_the_expiry() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let session = SessionId::new();
        let mut model = Model::new("model".into(), true);
        model.components = crate::components::Registry::empty();
        model.components.register(Secrets::default()).unwrap();
        model.session = Some(session);
        model.mount_components(&tx);
        let secret = json!({"type":"SecretRequested","id":uuid::Uuid::now_v7(),"session":session,"name":"gh.token","note":"GitHub","current":null,
            "destination":{"connection":"personal","name":"GitHub — CI","folder":"theuth","item":"login",
                "backend":"bw","summary":"me@example.org · vault.bitwarden.com","status":{"state":"ready"}}});
        model.apply(UiEvent::Wire(crate::components::tests::topic("secrets.events", 1, secret)));
        let s = model.components.get::<Secrets>().unwrap();
        let line = destination_line(s.requests.front().unwrap().destination.as_ref().unwrap());
        assert_eq!(line, "→ bw/personal › Personal/theuth · login \"GitHub — CI\"");
        model.component_submit("ghp_into_bitwarden");
        let Command::Component(effect) = rx.try_recv().unwrap() else { panic!("answered without an expiry step") };
        assert!(matches!(effect.request::<Request>(), Some(Request::AnswerSecret { expires_ms: None, values: Some(_), .. })));
    }

    fn conn(name: &str, backend: &str, status: ConnStatus) -> ConnInfo {
        ConnInfo { conn: name.into(), backend: backend.into(), status, summary: "me@example.org · vault.bitwarden.com".into(),
            ask: Default::default(), settings: Default::default(), login: (backend == "bw").then_some(BwLogin::Password) }
    }

    #[test]
    fn connection_rows_lock_browse_and_copy_a_reference() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut model = Model::new("model".into(), true);
        model.components = crate::components::Registry::empty();
        model.components.register(Secrets::default()).unwrap();
        model.mount_components(&tx);
        let s = model.components.get_mut::<Secrets>().unwrap();
        s.connections = vec![conn("prod", "aws", ConnStatus::Ready), conn("personal", "bw", ConnStatus::Locked)];
        s.menu = Some(Menu::Secrets { cursor: 0, history: None });
        let rows = s.secrets_rows();
        assert!(rows[0].starts_with("prod · aws · ready") && rows[1].starts_with("personal · bw · locked"), "{rows:?}");
        let key = |m: &mut Model, k| crate::model::tests::component_menu_key(m, "secrets", k);
        key(&mut model, KeyCode::Char('l'));
        let Command::Component(effect) = rx.try_recv().unwrap() else { panic!() };
        assert!(matches!(effect.request::<Request>(), Some(Request::Lock(c)) if c == "prod"));
        key(&mut model, KeyCode::Enter);
        let Command::Component(effect) = rx.try_recv().unwrap() else { panic!() };
        assert!(matches!(effect.request::<Request>(), Some(Request::Browse { conn, cursor: None, deleted: false }) if conn == "prod"));
        let entry = ExtEntry { r#ref: "aws/prod/app/token".into(), name: "app/token".into(), kind: "string".into(), keys: vec![],
            location: "eu-west-1".into(), versions: vec![], recoverable_until: None };
        model.components.get_mut::<Secrets>().unwrap().menu = Some(Menu::Browse { conn: "prod".into(), back: 0, cursor: 0, entries: vec![entry], next: None, deleted: false });
        assert!(model.components.get::<Secrets>().unwrap().secrets_rows()[0].ends_with("reveal in the desktop app"));
        key(&mut model, KeyCode::Char('y'));
        let Command::Component(effect) = rx.try_recv().unwrap() else { panic!() };
        assert!(matches!(effect.request::<Request>(), Some(Request::CopyRef { reference, reopen: None }) if reference == "{{secret:aws/prod/app/token}}"));
        key(&mut model, KeyCode::Char('i'));
        model.component_submit("app_token");
        let Command::Component(effect) = rx.try_recv().unwrap() else { panic!() };
        assert!(matches!(effect.request::<Request>(), Some(Request::Import { conn, r#ref, name }) if conn == "prod" && r#ref == "aws/prod/app/token" && name == "app_token"));
        // `u` on the locked Bitwarden row: a masked prompt, then `connection_unlock`.
        model.components.get_mut::<Secrets>().unwrap().menu = Some(Menu::Secrets { cursor: 1, history: None });
        key(&mut model, KeyCode::Char('u'));
        assert!(model.components.get::<Secrets>().unwrap().editor().unwrap().masked);
        model.component_submit("correct horse");
        let Command::Component(effect) = rx.try_recv().unwrap() else { panic!() };
        assert!(matches!(effect.request::<Request>(), Some(Request::Unlock { conn, password }) if conn == "personal" && password.as_str() == "correct horse"));
    }

    #[test]
    fn bitwarden_connect_walks_password_login_with_a_second_factor() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut model = Model::new("model".into(), true);
        model.components = crate::components::Registry::empty();
        model.components.register(Secrets::default()).unwrap();
        model.mount_components(&tx);
        let mut sent = || { let Command::Component(effect) = rx.try_recv().unwrap() else { panic!() }; effect };
        model.submit("/secrets connect bw", &tx);
        for line in ["personal", "eu", "me@example.org", "password"] { model.component_submit(line); }
        let effect = sent();
        assert!(matches!(effect.request::<Request>(), Some(Request::AddConnection { conn, cfg: ConnConfig::Bw { server: BwServer::Eu, login: BwLogin::Password, .. } }) if conn == "personal"));
        // The login waits for the add: a refused add asks for no password.
        assert!(model.components.get::<Secrets>().unwrap().prompt.is_none());
        crate::model::tests::component_reply(&mut model, "secrets", Msg::Added(Ok(conn("personal", "bw", ConnStatus::LoggedOut))));
        assert!(model.components.get::<Secrets>().unwrap().editor().unwrap().masked, "the master password is masked");
        model.component_submit("master-pw");
        let effect = sent();
        assert!(matches!(effect.request::<Request>(), Some(Request::Login { form: LoginForm::Password { two_factor: None, .. }, .. })));
        crate::model::tests::component_reply(&mut model, "secrets", Msg::LoggedIn { conn: "personal".into(), step: Ok(LoginStep::NeedTwoFactor { providers: vec!["authenticator".into(), "email".into()] }), connections: None });
        assert!(!model.components.get::<Secrets>().unwrap().editor().unwrap().masked, "the provider is a choice");
        model.component_submit("email");
        assert!(model.components.get::<Secrets>().unwrap().editor().unwrap().masked, "the code is masked");
        model.component_submit("123456");
        let effect = sent();
        assert!(matches!(effect.request::<Request>(), Some(Request::Login { form: LoginForm::Password { password, two_factor: Some((p, c)), .. }, .. })
            if password == "master-pw" && p == "email" && c == "123456"));
        crate::model::tests::component_reply(&mut model, "secrets", Msg::LoggedIn { conn: "personal".into(), step: Ok(LoginStep::NeedNewDeviceOtp), connections: None });
        assert!(model.components.get::<Secrets>().unwrap().editor().unwrap().masked, "the new-device code is masked");
        model.component_submit("998877");
        let effect = sent();
        assert!(matches!(effect.request::<Request>(), Some(Request::Login { form: LoginForm::Password { new_device_otp: Some(o), .. }, .. }) if o == "998877"));
        crate::model::tests::component_reply(&mut model, "secrets", Msg::LoggedIn { conn: "personal".into(), step: Ok(LoginStep::Done),
            connections: Some(vec![conn("personal", "bw", ConnStatus::Locked)]) });
        let s = model.components.get::<Secrets>().unwrap();
        assert!(s.login.is_none() && matches!(s.prompt, Some(Prompt::Unlock { .. })));
    }

    #[test]
    fn a_refused_bitwarden_add_opens_no_login_prompt() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut model = Model::new("model".into(), true);
        model.components = crate::components::Registry::empty();
        model.components.register(Secrets::default()).unwrap();
        model.mount_components(&tx);
        model.submit("/secrets connect bw", &tx);
        for line in ["personal", "us", "me@example.org", ""] { model.component_submit(line); }
        assert!(matches!(rx.try_recv(), Ok(Command::Component(_))));
        crate::model::tests::component_reply(&mut model, "secrets", Msg::Added(Err("bw/personal: unreachable; not added".into())));
        let s = model.components.get::<Secrets>().unwrap();
        assert!(s.prompt.is_none() && s.adding.is_none());
    }

    #[test]
    fn self_hosted_servers_need_https_except_on_this_machine() {
        assert_eq!(parse_server("https://vault.example.org/").map(|s| s.1), Some(Some("https://vault.example.org".into())));
        assert!(parse_server("http://vault.example.org").is_none());
        assert!(parse_server("http://localhost.evil.example").is_none());
        assert!(parse_server("http://user@vault.example.org").is_none());
        assert!(parse_server("https://").is_none());
        for ok in ["http://localhost:8080", "http://127.0.0.1", "http://[::1]:8000/"] {
            assert!(matches!(parse_server(ok), Some((BwServer::SelfHosted, Some(_)))), "{ok}");
        }
    }

    /// T5: `answer_unlock` → `false`, or the card resolving unlocked: false, says how to retry.
    #[test]
    fn a_failed_unlock_says_how_to_retry() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut model = Model::new("model".into(), true);
        model.components = crate::components::Registry::empty();
        model.components.register(Secrets::default()).unwrap();
        model.mount_components(&tx);
        let card = |n: u8| ::secrets::SecretsEvent::UnlockRequested {
            id: ::secrets::RequestId(uuid::Uuid::from_u128(u128::from(n))), conn: "personal".into(), backend: "bw".into(),
            account: "me@example.org".into(), reason: "a tool call".into(),
        };
        let notices = |m: &Model| m.entries.iter().filter_map(|e| match e { crate::model::Entry::Info(t) => Some(t.as_str()), _ => None }).collect::<Vec<_>>().join("\n");
        model.apply(UiEvent::Wire(crate::components::tests::topic("secrets.events", 1, serde_json::to_value(card(1)).unwrap())));
        model.component_submit("wrong");
        let Command::Component(effect) = rx.try_recv().unwrap() else { panic!() };
        assert!(matches!(effect.request::<Request>(), Some(Request::AnswerUnlock { password: Some(_), .. })));
        crate::model::tests::component_reply(&mut model, "secrets", Msg::Unlocked { id: ::secrets::RequestId(uuid::Uuid::from_u128(1)), target: "bw/personal".into(), result: Ok(false) });
        assert!(notices(&model).contains("wrong password or already answered; u to retry"), "{}", notices(&model));
        model.apply(UiEvent::Wire(crate::components::tests::topic("secrets.events", 1, serde_json::to_value(card(2)).unwrap())));
        model.component_submit("also-wrong");
        crate::model::tests::component_reply(&mut model, "secrets", Msg::Unlocked { id: ::secrets::RequestId(uuid::Uuid::from_u128(2)), target: "bw/personal".into(), result: Ok(true) });
        model.apply(UiEvent::Wire(crate::components::tests::topic("secrets.events", 1,
            serde_json::to_value(::secrets::SecretsEvent::UnlockResolved { id: ::secrets::RequestId(uuid::Uuid::from_u128(2)), unlocked: false }).unwrap())));
        assert!(notices(&model).contains("u to retry"), "{}", notices(&model));
        assert!(model.components.get::<Secrets>().unwrap().answered.is_empty());
    }

    /// T11: an unlock card arriving while a prompt is mid-line does not take that line.
    #[test]
    fn an_unlock_card_waits_for_the_line_being_typed() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut model = Model::new("model".into(), true);
        model.components = crate::components::Registry::empty();
        model.components.register(Secrets::default()).unwrap();
        model.mount_components(&tx);
        model.submit("/secrets add github", &tx);
        let hint = model.components.get::<Secrets>().unwrap().editor().unwrap().hint;
        assert!(hint.starts_with("secrets add  github  value"));
        let card = ::secrets::SecretsEvent::UnlockRequested {
            id: ::secrets::RequestId(uuid::Uuid::from_u128(7)), conn: "personal".into(), backend: "bw".into(),
            account: "me@example.org".into(), reason: "a tool call".into(),
        };
        model.apply(UiEvent::Wire(crate::components::tests::topic("secrets.events", 1, serde_json::to_value(card).unwrap())));
        assert_eq!(model.components.get::<Secrets>().unwrap().editor().unwrap().hint, hint, "the editor did not swap under the user");
        model.component_submit("ghp_value_for_github");
        assert!(rx.try_recv().is_err(), "the line went to the prompt, not answer_unlock");
        assert!(matches!(&model.components.get::<Secrets>().unwrap().prompt, Some(Prompt::Note { value, .. }) if value.as_str() == "ghp_value_for_github"));
        let editor = model.components.get::<Secrets>().unwrap().editor().unwrap();
        assert!(editor.hint.starts_with("unlock bw/personal") && editor.masked, "the card takes the next line");
    }

    fn record(id: &str) -> SendRecord {
        serde_json::from_value(serde_json::json!({
            "id": id, "conn": "personal", "kind": "text", "name": "Shared from theuth", "file_name": null, "file_bytes": null,
            "source_ref": "GITHUB_TOKEN", "link_secret": "send-personal-3fa9c2d1", "created_ms": 0, "expiration_ms": null,
            "deletion_ms": 0, "access_count": 0, "max_access": 1, "has_password": false, "status": "active",
        })).unwrap()
    }

    /// T12: `S` on a local row walks the §12.3 options; `s` lists Sends; `r` revokes; `y` copies
    /// the link (by id only); `t` shows the deleted entries, `r` restores one.
    #[test]
    fn sends_and_the_deleted_view() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut model = Model::new("model".into(), true);
        model.components = crate::components::Registry::empty();
        model.components.register(Secrets::default()).unwrap();
        model.mount_components(&tx);
        let key = |m: &mut Model, k| crate::model::tests::component_menu_key(m, "secrets", k);
        let s = model.components.get_mut::<Secrets>().unwrap();
        s.connections = vec![conn("personal", "bw", ConnStatus::Ready)];
        s.secrets = Some(::secrets::SecretsList { key_store: "keychain".into(), secrets: vec![::secrets::SecretInfo {
            name: "GITHUB_TOKEN".into(), note: String::new(), source: "user".into(), current: 1, updated: "2026-11-25T00:00:00Z".into(), expires: None, versions: vec![],
        }] });
        s.menu = Some(Menu::Secrets { cursor: 1, history: None });
        key(&mut model, KeyCode::Char('S'));
        // name, expires, delete, max opens, password, hide email, notes (one Bitwarden connection: no choice).
        for (line, bad) in [("", None), ("2h", Some("0h")), ("", None), ("", Some("zero")), ("ask", Some("not a name!")), ("no", Some("maybe")), ("", None)] {
            let editor = model.components.get::<Secrets>().unwrap().editor().unwrap();
            assert!(!editor.masked && editor.submit_empty, "{}", editor.hint);
            if let Some(bad) = bad {
                model.component_submit(bad);
                assert!(rx.try_recv().is_err(), "{bad} is re-asked");
            }
            model.component_submit(line);
        }
        let mut sent = || { let Command::Component(effect) = rx.try_recv().unwrap() else { panic!() }; effect };
        let effect = sent();
        let Some(Request::CreateSend { conn, args }) = effect.request::<Request>() else { panic!("expected create_send") };
        assert_eq!(conn, "personal");
        assert_eq!(args, &serde_json::json!({ "source": "GITHUB_TOKEN", "expires": "2h", "password": { "ask": true }, "hide_email": false }));
        // The `ask` card the RPC raises has no session; it is ours while the Send is in flight.
        model.apply(UiEvent::Wire(crate::components::tests::topic("secrets.events", 1, serde_json::json!({
            "type": "SecretRequested", "id": uuid::Uuid::now_v7(), "session": uuid::Uuid::nil(), "name": "send-personal-3fa9c2d1-password",
            "note": "Send password", "current": null,
            "fields": [{ "name": "send-personal-3fa9c2d1-password", "label": "Send password for \"Shared from theuth\"", "masked": true, "current": null }],
        }))));
        assert!(model.components.get::<Secrets>().unwrap().editor().unwrap().masked, "the Send password is typed masked");
        crate::model::tests::component_reply(&mut model, "secrets", Msg::SendCreated(Ok(record("r1"))));
        assert_eq!(model.components.get::<Secrets>().unwrap().sending, 0);
        // `s` on the connection row.
        model.components.get_mut::<Secrets>().unwrap().requests.clear();
        model.components.get_mut::<Secrets>().unwrap().menu = Some(Menu::Secrets { cursor: 0, history: None });
        key(&mut model, KeyCode::Char('s'));
        assert!(matches!(sent().request::<Request>(), Some(Request::ListSends(c)) if c == "personal"));
        crate::model::tests::component_reply(&mut model, "secrets", Msg::Sends("personal".into(), Ok(vec![record("r1")])));
        let rows = model.components.get::<Secrets>().unwrap().secrets_rows();
        assert!(rows[0].starts_with("send-personal-3fa9c2d1 · text · GITHUB_TOKEN") && rows[0].ends_with("opened 0/1 · password no · active"), "{rows:?}");
        key(&mut model, KeyCode::Char('y'));
        assert!(matches!(sent().request::<Request>(), Some(Request::CopyLink { id, .. }) if id == "r1"));
        key(&mut model, KeyCode::Char('r'));
        assert!(matches!(sent().request::<Request>(), Some(Request::RevokeSend { conn, id, .. }) if conn == "personal" && id == "r1"));
        // `t`: the deleted view; `r` restores.
        key(&mut model, KeyCode::Esc);
        key(&mut model, KeyCode::Char('t'));
        assert!(matches!(sent().request::<Request>(), Some(Request::Browse { deleted: true, .. })));
        let gone = ExtEntry { r#ref: "bw/personal/2f1c".into(), name: "GitHub".into(), kind: "login".into(), keys: vec![],
            location: "Personal".into(), versions: vec![], recoverable_until: Some("2026-12-25T00:00:00Z".into()) };
        crate::model::tests::component_reply(&mut model, "secrets", Msg::Browsed("personal".into(), true, None, Ok(ListPage { entries: vec![gone], next: None })));
        assert!(model.components.get::<Secrets>().unwrap().secrets_title().contains("deleted   r restore"));
        key(&mut model, KeyCode::Char('r'));
        assert!(matches!(sent().request::<Request>(), Some(Request::RestoreExternal { conn, r#ref, .. }) if conn == "personal" && r#ref == "bw/personal/2f1c"));
    }

    /// T10: `y` on an entry with fields asks which; ⏎ takes the default.
    #[test]
    fn copying_a_reference_offers_its_fields() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut model = Model::new("model".into(), true);
        model.components = crate::components::Registry::empty();
        model.components.register(Secrets::default()).unwrap();
        model.mount_components(&tx);
        let entry = ExtEntry { r#ref: "bw/personal/2f1c".into(), name: "GitHub".into(), kind: "login".into(), keys: vec!["password".into(), "username".into()],
            location: "Personal".into(), versions: vec![], recoverable_until: None };
        let s = model.components.get_mut::<Secrets>().unwrap();
        s.connections = vec![conn("personal", "bw", ConnStatus::Ready)];
        s.menu = Some(Menu::Browse { conn: "personal".into(), back: 0, cursor: 0, entries: vec![entry], next: None, deleted: false });
        crate::model::tests::component_menu_key(&mut model, "secrets", KeyCode::Char('y'));
        assert!(model.components.get::<Secrets>().unwrap().editor().unwrap().hint.contains("password · username"));
        model.component_submit("username");
        let Command::Component(effect) = rx.try_recv().unwrap() else { panic!() };
        assert!(matches!(effect.request::<Request>(), Some(Request::CopyRef { reference, reopen: Some(Reopen::Browse { .. }) }) if reference == "{{secret:bw/personal/2f1c#username}}"));
    }

    #[test]
    fn osc52_carries_the_text_only_as_base64() {
        let link = "https://vault.bitwarden.com/#/send/abc/key";
        let seq = osc52_sequence(link, false);
        assert!(seq.starts_with("\x1b]52;c;") && seq.ends_with('\x07') && !seq.contains(link));
        let tmux = osc52_sequence(link, true);
        assert!(tmux.starts_with("\x1bPtmux;\x1b\x1b]52;c;") && tmux.ends_with("\x1b\\"));
    }

    /// T7: `list_unlock_requests` brings back the cards missed while away and drops the ones
    /// answered elsewhere, but not one raised after the request.
    #[test]
    fn unlock_cards_reseed_from_the_engine() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut model = Model::new("model".into(), true);
        model.components = crate::components::Registry::empty();
        model.components.register(Secrets::default()).unwrap();
        model.mount_components(&tx);
        let id = |n: u128| ::secrets::RequestId(uuid::Uuid::from_u128(n));
        let view = |n| UnlockRequestView { id: id(n), conn: "personal".into(), backend: "bw".into(), account: "me@example.org".into(), reason: "a tool call".into() };
        crate::model::tests::component_reply(&mut model, "secrets", Msg::Unlocks(Ok(vec![view(1)])));
        assert!(model.components.get::<Secrets>().unwrap().editor().unwrap().hint.starts_with("unlock bw/personal"));
        let s = model.components.get_mut::<Secrets>().unwrap();
        s.reseed = vec![id(1)];
        s.unlocks.push_back(UnlockCard { id: id(2), conn: "work".into(), backend: "bw".into(), account: String::new(), reason: String::new(), armed: true });
        crate::model::tests::component_reply(&mut model, "secrets", Msg::Unlocks(Ok(vec![])));
        assert_eq!(model.components.get::<Secrets>().unwrap().unlocks.iter().map(|u| u.id).collect::<Vec<_>>(), [id(2)]);
    }
}
