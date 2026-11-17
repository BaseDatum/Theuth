//! 183 Phase 3 (§7, §14.4): contacts — separate harnesses reached over A2A on
//! `theuth/a2a/1`. 191 (D2): the contact store is the mesh-wide record tables in
//! `sessions.sqlite` ([`records`]; `peers.yaml`, [`store`], keeps HTTP peers and is imported
//! once); this module holds what the contact RPCs return, the invite ticket, the
//! ceremony's state and frames ([`ceremony`]), the Agent Card theuth signs, the Add contact
//! window and its LAN discovery ([`discovery`]), the `theuth/a2a/1` reach ([`reach`]) and the
//! actor messages behind `contact_*` / `a2a_export*`.

use std::collections::BTreeMap;

use a2a_types::AgentCard;
use agent::communication::network::{ChatPaused, NOT_NATIVE, UNKNOWN_SESSION, ValidateExport};
use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use runtime::SessionId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::A2aActor;
use crate::config::{FirstContact, PeersFile};

pub mod ceremony;
pub mod discovery;
#[cfg(test)]
pub(crate) mod e2e_tests;
pub mod files;
pub mod forward;
pub mod members;
pub mod offers;
pub mod reach;
pub mod records;
pub mod rosters;
pub mod sessions;
pub mod store;

/// 183 §7.2 (W-21): the contact ALPN, one HTTP/1.1 request (or one ceremony) per bi stream.
pub const ALPN_A2A: &str = mesh::net::ALPN_A2A;
/// 183 §7.3: the card's custom `protocolBinding` for the iroh interface.
pub const BINDING_IROH: &str = "https://theuth.io/a2a/bindings/iroh/v1";
/// 183 §7.3: `supportedInterfaces[0].url` = this prefix + the endpoint id; also the
/// `peer_bindings.endpoint` of a contact.
pub const IROH_URL_PREFIX: &str = agent::communication::network::IROH_PREFIX;
/// 183 §7.4: invite tickets read `theuth-contact:1.<base32 json InviteTicket>`.
pub const TICKET_PREFIX: &str = "theuth-contact:1.";
/// 183 §7.4: the domain string hashed first into the 6-digit code.
pub const CODE_DOMAIN: &str = "theuth-contact-v1";
/// 183 §7.4: a ceremony waits this long for both Accepts.
pub const CEREMONY_SECS: u64 = 120;
/// 183 §7.4: failed ceremonies from one endpoint id that refuse it for the window.
pub const FAILED_MAX: u32 = 3;
/// 183 §7.4: failed ceremonies, whoever from, that close the window.
pub const FAILED_GLOBAL_MAX: u32 = 10;
/// 183 Z3-R2: longest card name or machine text kept or shown.
pub const CARD_TEXT_MAX: usize = 64;
/// 183 §8.2: Nearby agents listed at most.
pub const NEARBY_MAX: usize = 32;
/// 183 §7.1 (LAD §4.6): card fetches per window.
pub const CARD_FETCHES_MAX: u32 = 8;
/// 183 §7.1 (LAD §4.6): largest card body.
pub const CARD_BYTES_MAX: usize = 256 * 1024;
/// 183 §7.1 (LAD §4.6): one card fetch's deadline.
pub const CARD_FETCH_SECS: u64 = 30;
/// 183 §7.0 (W-22): the card's JWS protected-header `alg`.
pub const JWS_ALG: &str = "EdDSA";
/// 183 §7.0 (W-22): the card's JWS protected-header `typ` (A2A §8.4 SHOULD).
pub const JWS_TYP: &str = "JOSE";
/// 183 §7.3: the card extension whose `params` carry `machine` and `os_name`.
pub const EXT_MACHINE: &str = "https://theuth.io/a2a/ext/machine/v1";
/// 186 (D10): the card extension a build that collaborates in full advertises.
pub const EXT_FULL: &str = "https://theuth.io/a2a/ext/full/v1";
/// 199 (D1): files between theuth engines (`files::FileMeta` under this key on a reference
/// part; `Message.extensions` exactly `[EXT_FILES]`), advertised with `maxBytes`.
pub const EXT_FILES: &str = "https://theuth.io/a2a/ext/files/v1";
/// What the contact RPCs answer when this process runs no a2a plugin.
pub const OFF: &str = "contacts are unavailable: the a2a plugin is not running";

// ---------------------------------------------------------------------------
// What the RPCs return (§14.4, TS)
// ---------------------------------------------------------------------------

/// One contact as Settings › Network › Contacts shows it (`contact_list`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Contact {
    /// Owner alias: the `peer:<name>` address; 191: the record's alias, unique in the mesh.
    pub name: String,
    /// The contact's machine, from its card.
    pub machine: String,
    /// Pinned iroh endpoint id.
    pub endpoint: String,
    /// §7.3: the endpoint id in groups of 4 base32 characters, first 20 characters.
    pub fingerprint: String,
    /// How it is reached right now.
    pub path: mesh::PeerPath,
    /// Export names granted to it, mesh-wide (191: from the records, a chat on another
    /// machine too; a waiting or refused contact's as well).
    pub exports: Vec<String>,
    /// Export → first-contact answer; a missing export is `ask`.
    pub first_contact: BTreeMap<String, FirstContact>,
    /// Refused at the gate.
    pub blocked: bool,
    /// When the ceremony finished (ms since epoch).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub paired_ms: u64,
    /// 183 F1: what the contact shares with us (its exports granted to this engine), from
    /// the card it last served us.
    pub shared: Vec<agent::communication::network::ContactOffer>,
    /// 186 (D10): its card advertises [`EXT_FULL`]. Shown only: a share never depends on it.
    pub full_capable: bool,
    /// 191 (D2): its record id (the same on every machine of the mesh).
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<String>", optional))]
    pub id: String,
    /// 191 (D2): why this machine quarantined the record (it is refused, its endpoints
    /// denied), or D1's inflation alert on a live one; `None` for a sound one.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub problem: Option<String>,
    /// 191 (D8): what the row offers for its `problem`; `None` exactly when it has none.
    #[serde(default)]
    pub problem_kind: Option<ProblemKind>,
    /// 191 (D8): the machine of ours whose ceremony paired it, by its name (`contact_list`
    /// names the record's engine id: the first 8 characters once that machine left the
    /// mesh); `None` when no machine is recorded (an import).
    #[serde(default)]
    pub paired_on: Option<String>,
    /// 191 (D8): the contact's machines as THIS machine accepted them (its own roster state,
    /// never a replicated row): the current roster, then those its accepted history lists
    /// but the roster no longer does (`left`); the pins while no roster is based here.
    #[serde(default)]
    pub machines: Vec<ContactMachine>,
    /// 200 (D1): we gave it full access: any of its sessions may list and message ours.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub full_access: bool,
    /// 200 (D11): whether it gave us full access, from its last verified card; `None` when
    /// its card lacks the `full-access/v1` extension (an engine without 200).
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub full_access_from: Option<bool>,
}

/// 191 (D8): what a contact row with a `problem` offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum ProblemKind {
    /// Refused here and it will not mend itself: Remove is the action.
    Refused,
    /// Held while replication catches up: no action, it clears by itself.
    Waiting,
    /// D1's inflation alert: the contact still works; Remove, or unpair one of ours.
    Alert,
}

/// 191 (D8): one machine of a contact, as this machine knows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ContactMachine {
    /// Its endpoint id.
    pub eid: String,
    /// The `machine` of its last card this machine fetched (in memory), else the first 8
    /// characters of `eid`.
    pub name: String,
    /// In this machine's accepted history of the contact's roster, not in its current one.
    pub left: bool,
}

/// 183 F3, F11: one (contact, export) whose first message waits for the owner's answer
/// (`contact_first_waiting`), the same source as the `contact_first` Attention item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct FirstWaiting {
    /// The contact's alias.
    pub contact: String,
    /// The contact's machine, from its card (`""` when unknown).
    pub machine: String,
    /// The export it wants to reach.
    pub export: String,
    /// The session its message would run in (the exported chat, or the agent's inbox).
    pub session: SessionId,
    /// When the oldest waiting message arrived (ms since epoch).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub since_ms: u64,
    /// How many messages wait.
    pub waiting: u32,
    /// The first line of the oldest one.
    pub preview: String,
    /// 186 review 1: not a first-contact hold but a message admitted and waiting behind the
    /// chat's running turn (it starts when that turn ends); only `contact_first_waiting` lists
    /// these, never the Attention item.
    /// An older engine sends neither field (TS: optional).
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub queued: bool,
    /// 186 review 1: a queued message's place in the chat's queue (1 = next); `0` for a hold.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<u32>", optional))]
    pub position: u32,
    /// 191 (D4): another machine of the mesh holds it: only Always or Block answer it from
    /// here (Allow once is the holder's). An older engine sends none (its holds were its own;
    /// TS: optional).
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub elsewhere: bool,
}

/// `contact_update`: each `Some` field replaces the contact's value; `grant` and `revoke`
/// change only the exports they name.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(default)]
pub struct ContactPatch {
    /// Rename (the alias).
    pub name: Option<String>,
    /// 191: replaced by `grant` / `revoke`; a patch that sets it is refused
    /// ([`EXPORTS_REPLACED`]), never read as a whole list (it revoked every grant its
    /// machine could not see).
    pub exports: Option<Vec<String>>,
    /// Block or unblock.
    pub blocked: Option<bool>,
    /// Export names (or ids) to grant; every other grant stays.
    pub grant: Option<Vec<String>>,
    /// Export names (or ids) to take back; every other grant stays.
    pub revoke: Option<Vec<String>>,
    /// 200 (D1): give or take back full access (every record of the group).
    pub full_access: Option<bool>,
}

/// `contact_pair`: whom to start the ceremony with.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContactTarget {
    /// A theuth agent in the Nearby list, by its endpoint id.
    Nearby {
        /// Endpoint id (from its verified card, not the TXT hint).
        eid: String,
    },
    /// A pasted or scanned invite ticket (`theuth-contact:1.…`).
    Ticket {
        /// The ticket string.
        ticket: String,
    },
    /// A foreign agent's card URL: the HTTP A2A form (§7.1).
    CardUrl {
        /// Card URL or its base.
        url: String,
    },
}

/// `contact_window(true)`: the open Add contact window's invite ticket.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ContactWindow {
    /// `theuth-contact:1.…`, for the QR and the copy button.
    pub ticket: String,
    /// When the window and the ticket expire (ms since epoch).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub expires_ms: u64,
}

/// `contact_nearby`: one A2A agent the browse saw (a hint, never stored, §7.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct NearbyAgent {
    /// DNS-SD instance name (the card `name`).
    pub name: String,
    /// The service type it was seen under (`_a2a._tcp`, …).
    pub service: String,
    /// The address it was seen at.
    pub addr: String,
    /// TXT `eid` (a theuth agent), a 16-char hint.
    pub eid: Option<String>,
    /// Card URL (TXT `card` / `cardUrl` / `path` against `addr`).
    pub card: Option<String>,
    /// The fetched card's JWS verified against the endpoint id in its iroh interface.
    pub verified: bool,
}

/// 183 D9: what an export publishes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ExportTarget {
    /// One of this machine's agents: an inbox per (contact, export) that runs under the
    /// agent's own `tools:` ceiling (W-25).
    Agent {
        /// `<agent>@<engine>` desk root, or `theuth`.
        agent: String,
    },
    /// One chat of any machine in the mesh: the contact's messages arrive as a mesh peer's.
    Chat {
        /// The exported session.
        session: SessionId,
    },
}

/// One export (`a2a_exports`, `a2a_export_put`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Export {
    /// 191 (D2): its replicated id; `None` on a new export (`a2a_export_put` mints one).
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub id: Option<String>,
    /// Export name: the card skill and the `/agents/<name>` path, `[a-z0-9_-]`, unique in
    /// the mesh (191: a second one is renamed `<name>-2`).
    pub name: String,
    /// What it publishes.
    pub target: ExportTarget,
    /// Card description.
    pub description: String,
    /// W-41: why the export answers nothing now: `chat archived` | `agent messaging off`.
    #[serde(default)]
    pub paused: Option<String>,
}

/// 183 §10.4: contact bus events. Wire `type` names are prefixed: `MeshEvent` already owns
/// `pair` and `nearby`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type", content = "data")]
pub enum ContactEvent {
    /// A ceremony started, its code, or its outcome (`PairCeremony.peer` = endpoint id).
    #[serde(rename = "contact_pair")]
    Pair(mesh::PairCeremony),
    /// The Nearby agents list changed.
    #[serde(rename = "contact_nearby")]
    Nearby(Vec<NearbyAgent>),
}

impl wire::WireEvent for ContactEvent {
    const TOPIC: &'static str = "a2a.contacts";
    const VERSION: u32 = 1;
}

// ---------------------------------------------------------------------------
// Invite ticket and ceremony (§7.4; engine-internal, no TS)
// ---------------------------------------------------------------------------

/// The JSON inside `theuth-contact:1.<base32>`: how to reach the inviter. Single use,
/// expires with the window; it grants nothing by itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InviteTicket {
    /// The inviter's endpoint id.
    pub eid: String,
    /// The inviter's card name.
    pub name: String,
    /// Its home relay URL, when it has one.
    #[serde(default)]
    pub relay: Option<String>,
    /// Its direct addresses (`ip:port`).
    #[serde(default)]
    pub addrs: Vec<String>,
    /// The window's nonce: the invitee echoes it in `PairFrame::Request.ticket`.
    pub nonce: String,
    /// Expiry (ms since epoch).
    pub exp: u64,
}

/// The first line of a `theuth/a2a/1` stream that carries a ceremony instead of an HTTP
/// request: `{"pair": …}` (§7.4, §10.2), then one JSON line per frame.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PairLine {
    /// The frame.
    pub pair: PairFrame,
}

/// Ceremony frames. B (invitee, the dialer) commits to its nonce before it sees A's, as the
/// mesh's Nearby ceremony does (`mesh::net::Msg::PairRequest.commit`), so neither side can
/// steer the code.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairFrame {
    /// B → A: start. `commit` = base32 `SHA256(nonce_b)`.
    Request {
        /// B's card name.
        name: String,
        /// B's machine.
        machine: String,
        /// B's nonce, committed.
        commit: String,
        /// The invite ticket's `nonce`; `None` for a Nearby dial into an open window.
        #[serde(default)]
        ticket: Option<String>,
    },
    /// A → B: A's nonce and A's signed card (B verifies its JWS against A's id).
    Challenge {
        /// A's nonce.
        nonce: String,
        /// A's signed public card.
        card: Box<AgentCard>,
    },
    /// B → A: B opens its commitment and sends its signed card; both sides now show the code.
    Reveal {
        /// B's nonce.
        nonce: String,
        /// B's signed public card.
        card: Box<AgentCard>,
    },
    /// Either side, after its human pressed Accept: the exports it grants the other.
    Accept {
        /// Export names.
        #[serde(default)]
        exports: Vec<String>,
    },
    /// Either side: Reject, a mismatch or a timeout; the ceremony ends.
    Reject {
        /// Why, for the other side's log.
        #[serde(default)]
        reason: String,
    },
    /// Best effort after Remove or Block (§7.4 Revocation).
    Revoke {},
}

/// Which side of a ceremony this engine is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// A: opened the window (Nearby advert or invite ticket).
    Inviter,
    /// B: dialed in.
    Invitee,
}

/// The four inputs of the 6-digit code (§7.4): `u32(sha256(CODE_DOMAIN ‖ min(id_a,id_b) ‖
/// max(id_a,id_b) ‖ nonce_a ‖ nonce_b)[0..4]) mod 10^6`, shown `482 913`. `a` is always the
/// inviter, `b` the invitee, on both engines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeInputs {
    /// Inviter's endpoint id.
    pub id_a: String,
    /// Invitee's endpoint id.
    pub id_b: String,
    /// Inviter's nonce.
    pub nonce_a: String,
    /// Invitee's nonce.
    pub nonce_b: String,
}

/// One ceremony in progress on this engine (one at a time per window).
#[derive(Clone, Debug)]
pub struct Ceremony {
    /// `contact_answer(ceremony, …)`.
    pub id: Uuid,
    /// This engine's side.
    pub role: Role,
    /// The other engine's endpoint id (from QUIC, never from a frame).
    pub peer: String,
    /// Its card name.
    pub name: String,
    /// Its machine.
    pub machine: String,
    /// Our nonce.
    pub nonce: String,
    /// The invitee's commitment (inviter side).
    pub commit: Option<String>,
    /// Their nonce, once sent (`Challenge` or `Reveal`).
    pub their_nonce: Option<String>,
    /// Their signed card, once verified.
    pub card: Option<Box<AgentCard>>,
    /// Our human pressed Accept.
    pub accepted: bool,
    /// Their `Accept` arrived (with the exports they grant us).
    pub their_accept: Option<Vec<String>>,
    /// `CEREMONY_SECS` after the request (ms since epoch).
    pub expires_ms: u64,
}

// ---------------------------------------------------------------------------
// The Agent Card theuth signs (§7.0 W-22, §7.3)
// ---------------------------------------------------------------------------

/// What theuth puts in its public card: `name`, `description`, `version`,
/// `supportedInterfaces` = `[{url: "iroh:<endpoint>", protocolBinding: BINDING_IROH,
/// protocolVersion: "1.0"}]` plus the HTTPS one when `public_url` is set,
/// `capabilities{streaming: true, pushNotifications: false, extensions: [{uri:
/// EXT_MACHINE, params: {machine, os_name}}]}`, and one skill per export granted to the
/// card's reader (none in the public card).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardFacts {
    /// Card name: this engine's contact name.
    pub name: String,
    /// Card description.
    pub description: String,
    /// Mesh name of this machine.
    pub machine: String,
    /// `platform::process::machine().os_name`.
    pub os_name: String,
    /// This engine's endpoint id.
    pub endpoint: String,
    /// `a2a.public_url`, for the second interface.
    pub public_url: Option<String>,
    /// 183 F1: the exports granted to the card's reader, listed as skills (`id` = export
    /// name, `description` = its description; tags `text`, its kind and its state).
    pub skills: Vec<agent::communication::network::ContactOffer>,
}

/// The JWS protected header (RFC 7515) over the JCS-canonical card without `signatures`
/// (RFC 8785, A2A §8.4): `{"alg": "EdDSA", "kid": "<endpoint id>", "typ": "JOSE"}`. The
/// envelope is the SDK's `a2a_types::AgentCardSignature{protected, signature}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JwsProtected {
    /// [`JWS_ALG`].
    pub alg: String,
    /// The signer's endpoint id.
    pub kid: String,
    /// [`JWS_TYP`].
    pub typ: String,
}

/// RFC 8785 (JCS) for the JSON an Agent Card holds: object members sorted by their names'
/// UTF-16 code units, no insignificant whitespace, strings escaped as ECMAScript's
/// `JSON.stringify` does (serde_json's escaping is the same: `"`, `\`, the C0 controls,
/// short forms for `\b \f \n \r \t`, lowercase `\u00xx` otherwise). Integers print as is.
/// ponytail: a float is printed by serde_json's shortest form, which equals ECMAScript's
/// for every value a card carries; a generic JCS crate if cards ever carry exponents.
pub fn jcs(v: &serde_json::Value) -> String {
    use serde_json::Value;
    fn put(v: &Value, out: &mut String) {
        match v {
            Value::Object(m) => {
                let mut keys: Vec<&String> = m.keys().collect();
                keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
                out.push('{');
                for (i, k) in keys.into_iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push_str(&Value::String(k.clone()).to_string());
                    out.push(':');
                    put(&m[k], out);
                }
                out.push('}');
            }
            Value::Array(a) => {
                out.push('[');
                for (i, x) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    put(x, out);
                }
                out.push(']');
            }
            other => out.push_str(&other.to_string()),
        }
    }
    let mut out = String::new();
    put(v, &mut out);
    out
}

/// The JWS signing input of `card` (its `signatures` left out) under `protected`:
/// `BASE64URL(protected) . BASE64URL(JCS(card))`.
fn signing_input(card: &AgentCard, protected_b64: &str) -> Result<String, String> {
    let mut bare = card.clone();
    bare.signatures = None;
    let v = serde_json::to_value(&bare).map_err(|e| e.to_string())?;
    let payload = data_encoding::BASE64URL_NOPAD.encode(jcs(&v).as_bytes());
    Ok(format!("{protected_b64}.{payload}"))
}

/// The theuth public card for `facts` (unsigned).
pub fn card(facts: &CardFacts) -> AgentCard {
    use a2a_types::{AgentCapabilities, AgentExtension, AgentInterface, AgentSkill};
    let mut interfaces = vec![AgentInterface::new(
        agent::communication::network::contact_endpoint(&facts.endpoint),
        BINDING_IROH,
    )];
    if let Some(url) = &facts.public_url {
        interfaces.push(AgentInterface::new(
            url.clone(),
            a2a_types::TRANSPORT_PROTOCOL_JSONRPC,
        ));
    }
    let params = std::collections::HashMap::from([
        ("machine".to_owned(), facts.machine.clone().into()),
        ("os_name".to_owned(), facts.os_name.clone().into()),
    ]);
    AgentCard {
        name: facts.name.clone(),
        description: facts.description.clone(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        supported_interfaces: interfaces,
        capabilities: AgentCapabilities {
            streaming: Some(true),
            push_notifications: Some(false),
            // 186 (D10): this build collaborates in full.
            extensions: Some(vec![
                AgentExtension {
                    uri: EXT_MACHINE.to_owned(),
                    description: None,
                    required: Some(false),
                    params: Some(params),
                },
                AgentExtension {
                    uri: EXT_FULL.to_owned(),
                    description: None,
                    required: Some(false),
                    params: None,
                },
                // 199 (D1): reference parts, each file at most `FILE_MAX`.
                AgentExtension {
                    uri: EXT_FILES.to_owned(),
                    description: None,
                    required: Some(false),
                    params: Some(std::collections::HashMap::from([(
                        "maxBytes".to_owned(),
                        agent::communication::files::FILE_MAX.into(),
                    )])),
                },
            ]),
            extended_agent_card: Some(false),
        },
        default_input_modes: vec!["text/plain".into()],
        default_output_modes: vec!["text/plain".into()],
        skills: facts
            .skills
            .iter()
            .map(|o| AgentSkill {
                id: o.name.clone(),
                name: o.name.clone(),
                description: o.title.clone(),
                tags: vec![
                    "text".into(),
                    o.kind.as_str().into(),
                    o.state.as_str().into(),
                    o.mode.as_str().into(),
                ],
                examples: None,
                input_modes: None,
                output_modes: None,
                security_requirements: None,
            })
            .collect(),
        provider: None,
        documentation_url: None,
        icon_url: None,
        security_schemes: None,
        security_requirements: None,
        signatures: None,
    }
}

/// Build the card for `facts` and sign it with the engine key (`sign` = Ed25519 over the
/// JWS signing input).
pub fn signed_card(
    facts: &CardFacts,
    sign: &dyn Fn(&[u8]) -> Vec<u8>,
) -> Result<AgentCard, String> {
    sign_card(card(facts), &facts.endpoint, sign)
}

/// Sign `card` as `kid` (A2A §8.4): one JWS in `signatures`, replacing any.
pub fn sign_card(
    mut card: AgentCard,
    kid: &str,
    sign: &dyn Fn(&[u8]) -> Vec<u8>,
) -> Result<AgentCard, String> {
    let protected = serde_json::to_string(&JwsProtected {
        alg: JWS_ALG.to_owned(),
        kid: kid.to_owned(),
        typ: JWS_TYP.to_owned(),
    })
    .map_err(|e| e.to_string())?;
    let protected = data_encoding::BASE64URL_NOPAD.encode(protected.as_bytes());
    let input = signing_input(&card, &protected)?;
    let signature = data_encoding::BASE64URL_NOPAD.encode(&sign(input.as_bytes()));
    card.signatures = Some(vec![a2a_types::AgentCardSignature {
        protected,
        signature,
        header: None,
    }]);
    Ok(card)
}

/// Verify `card.signatures[0]` against `eid` (the pinned or dialed endpoint id) and that
/// its iroh interface names the same id.
pub fn verify_card(card: &AgentCard, eid: &str) -> Result<(), String> {
    let key: iroh::EndpointId = eid
        .parse()
        .map_err(|_| format!("{eid:?} is not an endpoint id"))?;
    let iface = agent::communication::network::contact_endpoint(eid);
    if !card
        .supported_interfaces
        .iter()
        .any(|i| i.protocol_binding == BINDING_IROH && i.url == iface)
    {
        return Err("the card's iroh interface names another endpoint".into());
    }
    let sig = card
        .signatures
        .as_ref()
        .and_then(|s| s.first())
        .ok_or("the card is not signed")?;
    let b64 = |s: &str| {
        data_encoding::BASE64URL_NOPAD
            .decode(s.as_bytes())
            .map_err(|_| "the card signature is not base64url".to_owned())
    };
    let protected: JwsProtected = serde_json::from_slice(&b64(&sig.protected)?)
        .map_err(|_| "the card signature header is not JSON".to_owned())?;
    if protected.alg != JWS_ALG || protected.kid != eid {
        // Remote text: cut, so a forged header cannot flood a log or a status line.
        return Err(format!(
            "the card is signed {} by {}, not {JWS_ALG} by its endpoint",
            clip(&protected.alg),
            clip(&protected.kid)
        ));
    }
    let bytes: [u8; 64] = b64(&sig.signature)?
        .try_into()
        .map_err(|_| "the card signature is not 64 bytes".to_owned())?;
    let input = signing_input(card, &sig.protected)?;
    key.verify(input.as_bytes(), &iroh::Signature::from_bytes(&bytes))
        .map_err(|_| "the card signature does not verify against its endpoint".to_owned())
}

/// The card's display name, cut to [`CARD_TEXT_MAX`] characters.
pub fn card_name(card: &AgentCard) -> String {
    clip(&card.name)
}

/// 186 (D10): the card advertises [`EXT_FULL`]: its engine collaborates in full.
pub fn card_full(card: &AgentCard) -> bool {
    card.capabilities
        .extensions
        .iter()
        .flatten()
        .any(|e| e.uri == EXT_FULL)
}

/// 199 (D1): the card advertises [`EXT_FILES`]: its engine takes file references.
pub fn card_files(card: &AgentCard) -> bool {
    card.capabilities
        .extensions
        .iter()
        .flatten()
        .any(|e| e.uri == EXT_FILES)
}

/// The `machine` and `os_name` of a card's [`EXT_MACHINE`] extension (empty when absent),
/// each cut to [`CARD_TEXT_MAX`] characters.
pub fn card_machine(card: &AgentCard) -> (String, String) {
    let params = card
        .capabilities
        .extensions
        .iter()
        .flatten()
        .find(|e| e.uri == EXT_MACHINE)
        .and_then(|e| e.params.as_ref());
    let get = |k: &str| {
        clip(
            params
                .and_then(|p| p.get(k))
                .and_then(|v| v.as_str())
                .unwrap_or_default(),
        )
    };
    (get("machine"), get("os_name"))
}

// ---------------------------------------------------------------------------
// Actor messages behind the RPCs (§10.1)
// ---------------------------------------------------------------------------

/// `ask` → `Result<Option<ContactWindow>, String>`: `contact_window(open)`.
#[derive(Clone, Copy, Debug)]
pub struct ContactWindowOpen(pub bool);

/// `ask` → `Vec<NearbyAgent>`: `contact_nearby`.
#[derive(Clone, Copy, Debug)]
pub struct ContactsNearby;

/// `ask` → `Result<mesh::PairCeremony, String>`: `contact_pair(target)`.
#[derive(Clone, Debug)]
pub struct ContactPair(pub ContactTarget);

/// `ask` → `Result<(), String>`: `contact_answer(ceremony, accept)`.
#[derive(Clone, Copy, Debug)]
pub struct ContactAnswer {
    /// The ceremony.
    pub ceremony: Uuid,
    /// Accept (`true`) or Reject.
    pub accept: bool,
}

/// `ask` → `Vec<Contact>`: `contact_list`.
#[derive(Clone, Copy, Debug)]
pub struct ContactsList;

/// `ask` → `Result<Contact, String>`: `contact_update(name, patch)`.
#[derive(Clone, Debug)]
pub struct ContactUpdate {
    /// The contact.
    pub name: String,
    /// What changes.
    pub patch: ContactPatch,
}

/// `ask` → `Result<(), String>`: `contact_remove(name, block)`.
#[derive(Clone, Debug)]
pub struct ContactRemove {
    /// The contact.
    pub name: String,
    /// Keep the entry as `blocked: true` instead of deleting it.
    pub block: bool,
}

/// `ask` → `Vec<Export>`: `a2a_exports`.
#[derive(Clone, Copy, Debug)]
pub struct ExportsList;

/// `ask` → `Result<(), String>`: `a2a_export_put(export)` (create or replace by name).
#[derive(Clone, Debug)]
pub struct ExportPut(pub Export);

/// `ask` → `Result<(), String>`: `a2a_export_remove(name)` (also Stop sharing, §12.10).
#[derive(Clone, Debug)]
pub struct ExportRemove {
    /// Export name.
    pub name: String,
}

/// `ask` → `bool` (183 W-41, §12.10): chat `session` was archived or unarchived. When it
/// is exported here its export row's pause changed, so `A2aChanged` fires (answers `true`).
#[derive(Clone, Copy, Debug)]
pub struct ChatArchived {
    /// The chat.
    pub session: SessionId,
}

/// `ask` → `Result<(), String>`: `contact_first_answer(contact, export, answer)` from the
/// `contact_first` Attention item (id `contact_first:<contact>:<export>`).
#[derive(Clone, Debug)]
pub struct ContactFirstAnswer {
    /// The contact.
    pub contact: String,
    /// The export it asked for.
    pub export: String,
    /// Allow once (`ask` stays) · Always (`allow`) · Block (`block`).
    pub answer: FirstContact,
}

// ---------------------------------------------------------------------------
// The Add contact window and the ceremonies (§7.1, §7.4)
// ---------------------------------------------------------------------------

/// The A2A actor's contact state: at most one window, the ceremonies running.
#[derive(Default)]
pub(crate) struct Contacts {
    window: Option<Window>,
    ceremonies: std::collections::HashMap<Uuid, Running>,
    /// Bumped per window, so a stale expiry timer closes nothing.
    seq: u64,
    /// Tests shorten [`CEREMONY_SECS`].
    pub(crate) ceremony_secs: Option<u64>,
    /// The one inviter ceremony slot, taken at `admit_request` (before any ceremony shows
    /// in `ceremonies`) and freed when that ceremony fails or ends.
    busy: bool,
    /// Private, link-local and loopback targets are dialed and fetched without a LAN
    /// sighting (the dev rig and the tests, whose "LAN" is loopback).
    pub(crate) private_ok: bool,
}

impl Contacts {
    pub(crate) fn new(private_ok: bool) -> Self {
        Self {
            private_ok,
            ..Self::default()
        }
    }
}

struct Window {
    seq: u64,
    nonce: String,
    ticket: String,
    expires_ms: u64,
    /// The ticket was used (single use).
    used: bool,
    /// Failed ceremonies this window (§7.4 Attempts).
    failed: u32,
    /// Failed ceremonies per remote endpoint id: [`FAILED_MAX`] refuse that id.
    failed_by: std::collections::HashMap<String, u32>,
    /// The advert + browse; dropping it stops both.
    _mdns: Option<Box<dyn Send>>,
    /// Sightings, newest first (§8.2: at most [`NEARBY_MAX`]).
    nearby: Vec<discovery::Sighting>,
    /// Endpoint ids whose fetched card verified (`NearbyAgent.verified`).
    verified: std::collections::HashSet<String>,
    /// Card fetches this window (LAD §4.6).
    fetches: u32,
}

pub(crate) struct Running {
    view: mesh::PairCeremony,
    role: Role,
    answers: tokio::sync::mpsc::UnboundedSender<bool>,
}

impl crate::Ctx {
    /// This engine's mesh endpoint, once bound.
    pub(crate) fn net(&self) -> Option<std::sync::Arc<mesh::net::Net>> {
        self.net.get().cloned()
    }

    /// This engine's endpoint id.
    pub(crate) fn me(&self) -> Result<String, String> {
        self.net()
            .map(|n| n.id.0.clone())
            .ok_or_else(|| "this engine has no mesh endpoint".to_owned())
    }

    /// (card name, machine) of this engine: the user's login name, and the mesh name.
    pub(crate) fn my_names(&self) -> (String, String) {
        let machine = mesh::self_name();
        let name = std::env::var("USER")
            .ok()
            .filter(|u| !u.trim().is_empty())
            .unwrap_or_else(|| machine.clone());
        (name, machine)
    }

    /// This engine's public card facts (§7.3): no skills.
    pub(crate) fn card_facts(&self, net: &mesh::net::Net) -> CardFacts {
        let (name, machine) = self.my_names();
        CardFacts {
            description: format!("theuth on {machine}"),
            name,
            machine,
            os_name: platform::process::machine().os_name.clone(),
            endpoint: net.id.0.clone(),
            public_url: self.public_url.clone(),
            skills: Vec::new(),
        }
    }

    /// This engine's signed public card (§7.3).
    pub(crate) fn my_card(&self) -> Result<AgentCard, String> {
        let net = self.net().ok_or("this engine has no mesh endpoint")?;
        signed_card(&self.card_facts(&net), &|m: &[u8]| net.sign(m))
    }

    /// A fresh nonce and this engine's signed card, for a ceremony.
    pub(crate) fn my_nonce_and_card(&self) -> Result<(String, AgentCard), String> {
        Ok((ceremony::nonce(), self.my_card()?))
    }

    /// How long a ceremony waits for both Accepts ([`CEREMONY_SECS`]; tests shorten it).
    pub(crate) fn ceremony_secs(&self) -> u64 {
        self.contacts.lock().ceremony_secs.unwrap_or(CEREMONY_SECS)
    }

    /// An Add contact window is open now.
    pub(crate) fn window_open(&self) -> bool {
        let now = mesh::identity::now_ms();
        self.contacts
            .lock()
            .window
            .as_ref()
            .is_some_and(|w| w.expires_ms > now)
    }

    /// §7.2: what `theuth/a2a/1` lets `remote` do.
    pub(crate) fn gate(&self, remote: &str) -> crate::server::IrohGate {
        let open = self.window_open();
        let ready = self.ready.load(std::sync::atomic::Ordering::SeqCst);
        crate::server::iroh_gate(&self.shared.read(), remote, open, ready)
    }

    /// §7.4: may `remote` start a ceremony now? One at a time (the slot is taken here, and
    /// freed when the ceremony fails or ends), inside an open window, not an endpoint that
    /// failed [`FAILED_MAX`] times, a ticket's nonce only once, and without a ticket only
    /// from an endpoint a LAN sighting of this window shows (its `eid` hint).
    pub(crate) fn admit_request(&self, remote: &str, ticket: Option<&str>) -> Result<(), String> {
        let now = mesh::identity::now_ms();
        let mut guard = self.contacts.lock();
        let s = &mut *guard;
        if s.busy || !s.ceremonies.is_empty() {
            return Err("another contact ceremony is running".into());
        }
        let w = s
            .window
            .as_mut()
            .filter(|w| w.expires_ms > now)
            .ok_or("no Add contact window is open")?;
        if w.failed_by.get(remote).is_some_and(|n| *n >= FAILED_MAX) {
            return Err("too many failed ceremonies from this endpoint".into());
        }
        match ticket {
            Some(t) => {
                if w.used || t != w.nonce {
                    return Err("the invite ticket was used or is not this window's".into());
                }
                w.used = true;
            }
            None => {
                let seen = w.nearby.iter().any(|x| {
                    x.txt
                        .get("eid")
                        .is_some_and(|h| !h.is_empty() && remote.starts_with(h.as_str()))
                });
                if !seen {
                    return Err("a request without a ticket must come from a Nearby agent".into());
                }
            }
        }
        s.busy = true;
        tracing::info!(%remote, ticket = ticket.is_some(), "a2a: contact ceremony requested");
        Ok(())
    }

    /// A ceremony reached its code: register it, publish it, and hand back its answers.
    pub(crate) fn ceremony_started(
        self: &std::sync::Arc<Self>,
        role: Role,
        peer: &str,
        name: &str,
        machine: &str,
        card: &AgentCard,
        code: String,
    ) -> (Uuid, tokio::sync::mpsc::UnboundedReceiver<bool>) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let id = Uuid::now_v7();
        let shown = card_name(card);
        let shown = if shown.is_empty() { clip(name) } else { shown };
        // 183 Z3-R2: the same name under another key is shown as such, before the owner
        // compares codes.
        let lookalike = self
            .shared
            .read()
            .staged
            .contacts
            .values()
            // 191: a machine the contact's roster already lists is that contact, not a
            // lookalike.
            .any(|c| c.display == shown && c.eid != peer && !c.eids.iter().any(|e| e == peer));
        if lookalike {
            tracing::warn!(%shown, %peer, "a2a: ceremony peer has the name of an existing contact but another key");
        }
        let view = mesh::PairCeremony {
            id,
            peer: mesh::EngineId(peer.to_owned()),
            name: if lookalike {
                format!("{shown} (new key — not your existing {shown})")
            } else {
                shown
            },
            os_name: card_machine(card).1,
            // 183 §12.7: the ceremony shows the other side's machine. The pin is the endpoint
            // id; where it answered from is not tracked, so `addr` carries the machine.
            addr: Some(card_machine(card).0)
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| clip(machine)),
            code,
            state: "waiting".into(),
            expires_ms: mesh::identity::now_ms() + self.ceremony_secs() * 1000,
        };
        tracing::info!(ceremony = %id, %peer, %machine, ?role, "a2a: contact ceremony shows its code");
        self.contacts.lock().ceremonies.insert(
            id,
            Running {
                view: view.clone(),
                role,
                answers: tx,
            },
        );
        let c = self.clone();
        tokio::spawn(async move { ceremony::publish(&c, view).await });
        (id, rx)
    }

    /// The ceremony as `contact_pair` answers it.
    pub(crate) fn ceremony_view(&self, id: Uuid) -> Option<mesh::PairCeremony> {
        self.contacts
            .lock()
            .ceremonies
            .get(&id)
            .map(|r| r.view.clone())
    }

    /// A ceremony's state changed (this side accepted).
    pub(crate) async fn ceremony_state(&self, id: Uuid, state: &str) {
        let view = {
            let mut s = self.contacts.lock();
            let Some(r) = s.ceremonies.get_mut(&id) else {
                return;
            };
            r.view.state = state.to_owned();
            r.view.clone()
        };
        ceremony::publish(self, view).await;
    }

    /// §7.4 Attempts: a failed ceremony counts against `remote` ([`FAILED_MAX`] refuse that
    /// endpoint for the window) and the window ([`FAILED_GLOBAL_MAX`] close it). A failed
    /// inviter ceremony frees the one ceremony slot.
    pub(crate) fn ceremony_failed(&self, remote: &str, role: Role) {
        let close = {
            let mut s = self.contacts.lock();
            if role == Role::Inviter {
                s.busy = false;
            }
            match s.window.as_mut() {
                Some(w) => {
                    w.failed += 1;
                    *w.failed_by.entry(remote.to_owned()).or_default() += 1;
                    w.failed >= FAILED_GLOBAL_MAX
                }
                None => false,
            }
        };
        if close {
            tracing::info!("a2a: {FAILED_GLOBAL_MAX} failed contact ceremonies close the window");
            self.close_window();
        }
    }

    /// The ceremony ended (`paired`, `rejected`, `expired`).
    pub(crate) async fn ceremony_ended(&self, id: Uuid, state: &str) {
        let mut r = {
            let mut s = self.contacts.lock();
            let Some(r) = s.ceremonies.remove(&id) else {
                return;
            };
            if r.role == Role::Inviter {
                s.busy = false;
            }
            r
        };
        r.view.state = state.to_owned();
        tracing::info!(ceremony = %id, %state, "a2a: contact ceremony ended");
        ceremony::publish(self, r.view).await;
    }

    /// Both accepted: pin the other side's endpoint id (TOFU) as an anchor of a contact
    /// record (191 D2, replicated to every machine of the mesh); the inviter's window closes.
    /// A live record with that anchor keeps its alias and settings (a blocked one is unblocked
    /// under a fresh binding); otherwise a new record is made. 183 F1: then fetch what the
    /// new contact shares with us.
    pub(crate) async fn pin(
        self: &std::sync::Arc<Self>,
        id: Uuid,
        card: &AgentCard,
    ) -> Result<(), String> {
        use agent::store::contacts::{Cell, RecordTable, RecordWrite};
        let (eid, role) = {
            let s = self.contacts.lock();
            let r = s.ceremonies.get(&id).ok_or("the ceremony ended")?;
            (r.view.peer.0.clone(), r.role)
        };
        let machine = card_machine(card).0;
        let display = card_name(card);
        let now = mesh::identity::now_ms() as i64;
        let r = self.records().await?;
        let a = records::analyze(&r);
        // A live record whose anchor is `eid`, else (191 D1: "an endpoint that already belongs
        // to a live record") the contact whose current roster admits it: Pair again adds a
        // pin to it, never a second record.
        let listed = {
            let live = self.shared.read();
            live.staged
                .contact_by_eid(&eid)
                .map(|alias| live.staged.contacts[alias].id.clone())
        };
        let known = records::record_with_eid(&r, &eid, false)
            .or_else(|| listed.and_then(|id| r.contacts.iter().find(|c| c.id == id)))
            .map(|c| records::group_of(&a, &c.id).map_or(vec![c], |g| g.members.clone()));
        let mut w = Vec::new();
        let record = match known {
            Some(members) => {
                let root = members[0];
                let blocked = members.iter().any(|m| m.blocked.is_some_and(|b| b != 0));
                for m in members.iter().filter(|m| m.blocked.is_some_and(|b| b != 0)) {
                    w.push(RecordWrite::new(
                        RecordTable::Contacts,
                        m.id.clone(),
                        vec![("blocked", Cell::Int(0))],
                    ));
                }
                let mut set = vec![("name", Cell::Text(display.clone()))];
                if !machine.is_empty() {
                    set.push(("machine", Cell::Text(machine.clone())));
                }
                // Unblock writes a fresh binding: the blocked one stays revoked everywhere;
                // so does a re-pair into a record whose binding this machine revoked (a
                // removed duplicate's), which a replica never un-revokes.
                let revoked = root
                    .binding
                    .as_deref()
                    .and_then(records::binding_id)
                    .is_none_or(|b| r.revoked.contains(&b.to_string()));
                if blocked || revoked {
                    set.push(("binding", Cell::Text(records::fresh_binding())));
                }
                w.push(RecordWrite::new(
                    RecordTable::Contacts,
                    root.id.clone(),
                    set,
                ));
                root.id.clone()
            }
            None => {
                // 183 Z3-R2: a card named like an existing contact but with another key is
                // not silently "bob-2": its alias says so, as does the ceremony it came from.
                let lookalike = r
                    .contacts
                    .iter()
                    .any(|c| c.removed_ms.is_none() && c.name.as_deref() == Some(display.as_str()));
                let mut base = crate::config::alias_of(&display);
                if lookalike {
                    let shown = display.as_str();
                    tracing::warn!(name = %shown, %eid, "a2a: new contact has the name of an existing one but another key");
                    base = format!("{base}-new-key");
                }
                let live = self.shared.read().clone();
                let taken = |alias: &str| {
                    r.contacts
                        .iter()
                        .any(|c| c.removed_ms.is_none() && c.alias.as_deref() == Some(alias))
                        || live.staged.peers.contains_key(alias)
                };
                let mut alias = base.clone();
                let mut n = 2;
                while taken(&alias) {
                    alias = format!("{base}-{n}");
                    n += 1;
                }
                let record = Uuid::now_v7().to_string();
                w.push(RecordWrite::new(
                    RecordTable::Contacts,
                    record.clone(),
                    vec![
                        ("alias", Cell::Text(alias)),
                        ("name", Cell::Text(display.clone())),
                        ("machine", Cell::Text(machine.clone())),
                        ("binding", Cell::Text(records::fresh_binding())),
                        ("paired_ms", Cell::Int(now)),
                        ("paired_on", Cell::Text(r.me.clone())),
                        ("blocked", Cell::Int(0)),
                    ],
                ));
                record
            }
        };
        let pin_id = format!("{record}|{eid}");
        // 191 (D1): this machine joins the pins the contact holds of us; a machine that pinned
        // this endpoint before stays one (a set: a third ceremony forgets nobody).
        let pinned_by = records::pinned_by_with(&r, &pin_id, &r.me);
        w.push(RecordWrite::new(
            RecordTable::Endpoints,
            pin_id,
            vec![
                ("contact", Cell::Text(record.clone())),
                ("eid", Cell::Text(eid.clone())),
                ("paired_ms", Cell::Int(now)),
                ("barred_ms", Cell::Null),
                ("pinned_by", pinned_by),
            ],
        ));
        self.commit_records(w).await?;
        if role == Role::Inviter {
            self.close_window();
        }
        if let Some(net) = self.net() {
            net.join_network().await;
        }
        self.refresh_offers_soon(&eid);
        Ok(())
    }

    /// `contact_window(true)`: open (or return the open) Add contact window: its ticket,
    /// the advert and the browse; it closes itself after `a2a.contact_window_secs`.
    pub(crate) async fn open_window(self: &std::sync::Arc<Self>) -> Result<ContactWindow, String> {
        // 191 (D1): version 1 never comes from a stale view: no ceremony before the first
        // reconcile after the mesh latch.
        if !self.ready.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(agent::communication::network::CONTACTS_WAITING.into());
        }
        let now = mesh::identity::now_ms();
        if let Some(w) = self
            .contacts
            .lock()
            .window
            .as_ref()
            .filter(|w| w.expires_ms > now)
        {
            return Ok(ContactWindow {
                ticket: w.ticket.clone(),
                expires_ms: w.expires_ms,
            });
        }
        let net = self.net().ok_or("this engine has no mesh endpoint")?;
        net.join_network().await;
        let (addrs, relay) = net.a2a_reach();
        let (name, machine) = self.my_names();
        let card = serde_json::to_string(&self.my_card()?).map_err(|e| e.to_string())?;
        let nonce = ceremony::nonce();
        let expires_ms = now + self.cfg.contact_window_secs.max(1) * 1000;
        let ticket = ceremony::encode_ticket(&InviteTicket {
            eid: net.id.0.clone(),
            name: name.clone(),
            relay,
            addrs: addrs.clone(),
            nonce: nonce.clone(),
            exp: expires_ms,
        });
        let seq = {
            let mut s = self.contacts.lock();
            s.seq += 1;
            s.seq
        };
        self.contacts.lock().window = Some(Window {
            seq,
            nonce,
            ticket: ticket.clone(),
            expires_ms,
            used: false,
            failed: 0,
            failed_by: Default::default(),
            _mdns: None,
            nearby: Vec::new(),
            verified: Default::default(),
            fetches: 0,
        });
        // §7.1: advertise the first type, browse them all, only while this window lives
        // (installed first: the browse may report at once).
        if let Some(service) = self.cfg.mdns_types.first() {
            let weak = std::sync::Arc::downgrade(self);
            let seen: discovery::Seen = Box::new(move |s| {
                if let Some(c) = weak.upgrade() {
                    c.sighted(seq, s);
                }
            });
            let advert = discovery::advert(service, &machine, &net.id.0, &addrs, card);
            match self.mdns.start(advert, self.cfg.mdns_types.clone(), seen) {
                Ok(guard) => {
                    if let Some(w) = self
                        .contacts
                        .lock()
                        .window
                        .as_mut()
                        .filter(|w| w.seq == seq)
                    {
                        w._mdns = Some(guard);
                    }
                }
                Err(e) => tracing::warn!("a2a: contact advert: {e}"),
            }
        }
        let weak = std::sync::Arc::downgrade(self);
        let after = std::time::Duration::from_millis(expires_ms.saturating_sub(now));
        tokio::spawn(async move {
            tokio::time::sleep(after).await;
            let Some(c) = weak.upgrade() else { return };
            let current = c
                .contacts
                .lock()
                .window
                .as_ref()
                .is_some_and(|w| w.seq == seq);
            if current {
                c.close_window();
            }
        });
        tracing::info!(expires_ms, "a2a: Add contact window open");
        Ok(ContactWindow { ticket, expires_ms })
    }

    /// Close the window (Cancel, success, expiry, too many failures): the advert and the
    /// browse stop, the sightings go, an inviter ceremony still waiting is rejected.
    pub(crate) fn close_window(&self) {
        let (window, waiting) = {
            let mut s = self.contacts.lock();
            let waiting: Vec<_> = s
                .ceremonies
                .values()
                .filter(|r| r.role == Role::Inviter)
                .map(|r| r.answers.clone())
                .collect();
            (s.window.take(), waiting)
        };
        for tx in waiting {
            let _ = tx.send(false);
        }
        if window.is_some() {
            tracing::info!("a2a: Add contact window closed");
            let rt = self.rt.clone();
            tokio::spawn(async move { rt.publish(ContactEvent::Nearby(Vec::new())).await });
        }
    }

    /// A browse result of window `seq`: kept newest first, at most [`NEARBY_MAX`].
    fn sighted(self: &std::sync::Arc<Self>, seq: u64, s: discovery::Sighting) {
        let list = {
            let mut st = self.contacts.lock();
            let Some(w) = st.window.as_mut().filter(|w| w.seq == seq) else {
                return;
            };
            w.nearby
                .retain(|x| !(x.service == s.service && x.instance == s.instance));
            if !s.gone {
                w.nearby.insert(0, s);
                w.nearby.truncate(NEARBY_MAX);
            }
            nearby_list(w)
        };
        let rt = self.rt.clone();
        tokio::spawn(async move { rt.publish(ContactEvent::Nearby(list)).await });
    }

    /// `contact_nearby`.
    pub(crate) fn nearby(&self) -> Vec<NearbyAgent> {
        let now = mesh::identity::now_ms();
        self.contacts
            .lock()
            .window
            .as_ref()
            .filter(|w| w.expires_ms > now)
            .map(nearby_list)
            .unwrap_or_default()
    }

    /// `contact_pair(target)`.
    pub(crate) async fn pair(
        self: &std::sync::Arc<Self>,
        target: ContactTarget,
    ) -> Result<mesh::PairCeremony, String> {
        if !self.ready.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(agent::communication::network::CONTACTS_WAITING.into());
        }
        let me = self.me()?;
        let (lan, private_ok) = self.dial_policy();
        let (eid, hints, ticket) = match target {
            ContactTarget::Ticket { ticket } => {
                let t = ceremony::decode_ticket(&ticket)?;
                let hints = ceremony::ticket_target(&t, &lan, private_ok)?;
                (t.eid, hints, Some(t.nonce))
            }
            ContactTarget::Nearby { eid } => {
                let (full, hints) = self.nearby_target(&eid).await?;
                (full, hints, None)
            }
            ContactTarget::CardUrl { url } => {
                let card = fetch_card(&url, &lan, private_ok).await?;
                let eid = card
                    .supported_interfaces
                    .iter()
                    .find(|i| i.protocol_binding == BINDING_IROH)
                    .and_then(|i| i.url.strip_prefix(IROH_URL_PREFIX))
                    .map(str::to_owned)
                    .ok_or("not a theuth agent: add it as an HTTP A2A peer")?;
                verify_card(&card, &eid).map_err(|e| format!("unverified card: {e}"))?;
                (eid, reach::Hints::default(), None)
            }
        };
        if eid == me {
            return Err("that is this engine".into());
        }
        if self.contacts.lock().ceremonies.values().next().is_some() {
            return Err("another contact ceremony is running".into());
        }
        ceremony::start(self.clone(), eid, hints, ticket).await
    }

    /// 183 Z3-R2 (SSRF): the IPs of this window's LAN sightings, and whether private
    /// targets are allowed regardless (the dev rig and tests).
    fn dial_policy(&self) -> (Vec<std::net::IpAddr>, bool) {
        let s = self.contacts.lock();
        let lan = s
            .window
            .as_ref()
            .map(|w| {
                w.nearby
                    .iter()
                    .flat_map(|x| x.addrs.iter().map(|a| a.ip()))
                    .collect()
            })
            .unwrap_or_default();
        (lan, s.private_ok)
    }

    /// A Nearby sighting by its full endpoint id, or by its TXT hint after fetching and
    /// verifying its card (§7.1: on click, at most [`CARD_FETCHES_MAX`] per window).
    async fn nearby_target(&self, eid: &str) -> Result<(String, reach::Hints), String> {
        let sighting = {
            let s = self.contacts.lock();
            let w = s.window.as_ref().ok_or("open Add contact first")?;
            w.nearby
                .iter()
                .find(|x| {
                    x.txt
                        .get("eid")
                        .is_some_and(|h| !h.is_empty() && eid.starts_with(h.as_str()))
                })
                .cloned()
        };
        let hints = |s: &Option<discovery::Sighting>| reach::Hints {
            addrs: s.as_ref().map(discovery::iroh_addrs).unwrap_or_default(),
            relay: None,
        };
        if eid.parse::<iroh::EndpointId>().is_ok() {
            return Ok((eid.to_owned(), hints(&sighting)));
        }
        let s = sighting.ok_or("that agent is no longer nearby")?;
        let url = discovery::nearby(&s)
            .card
            .ok_or("that agent advertises no card")?;
        {
            let mut st = self.contacts.lock();
            let w = st.window.as_mut().ok_or("open Add contact first")?;
            if w.fetches >= CARD_FETCHES_MAX {
                return Err("too many cards fetched in this window".into());
            }
            w.fetches += 1;
        }
        let (lan, private_ok) = self.dial_policy();
        let card = fetch_card(&url, &lan, private_ok).await?;
        let full = card
            .supported_interfaces
            .iter()
            .find(|i| i.protocol_binding == BINDING_IROH)
            .and_then(|i| i.url.strip_prefix(IROH_URL_PREFIX))
            .filter(|full| full.starts_with(eid))
            .map(str::to_owned)
            .ok_or("the card does not match the advert")?;
        verify_card(&card, &full).map_err(|e| format!("unverified card: {e}"))?;
        if let Some(w) = self.contacts.lock().window.as_mut() {
            w.verified.insert(full.clone());
        }
        Ok((full, hints(&Some(s))))
    }

    /// `contact_answer(ceremony, accept)`.
    pub(crate) fn answer(&self, ceremony: Uuid, accept: bool) -> Result<(), String> {
        let s = self.contacts.lock();
        let r = s
            .ceremonies
            .get(&ceremony)
            .ok_or("no such contact ceremony (it may have ended)")?;
        r.answers
            .send(accept)
            .map_err(|_| "the ceremony already ended".to_owned())
    }
}

fn nearby_list(w: &Window) -> Vec<NearbyAgent> {
    w.nearby
        .iter()
        .map(|s| {
            let mut n = discovery::nearby(s);
            n.verified = n
                .eid
                .as_deref()
                .is_some_and(|h| w.verified.iter().any(|v| v.starts_with(h)));
            n
        })
        .collect()
}

/// 183 Z3-R2 (SSRF): an address that is not on the public internet.
pub(crate) fn is_local(ip: std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    match ip {
        IpAddr::V4(a) => {
            a.is_loopback()
                || a.is_private()
                || a.is_link_local()
                || a.is_unspecified()
                || a.is_broadcast()
                // 100.64.0.0/10 (carrier-grade NAT)
                || (a.octets()[0] == 100 && a.octets()[1] & 0xc0 == 64)
        }
        IpAddr::V6(a) => {
            a.is_loopback()
                || a.is_unspecified()
                // fc00::/7 unique local, fe80::/10 link-local
                || a.segments()[0] & 0xfe00 == 0xfc00
                || a.segments()[0] & 0xffc0 == 0xfe80
                || a.to_ipv4_mapped().is_some_and(|v| is_local(IpAddr::V4(v)))
        }
    }
}

/// 183 Z3-R2: may this engine fetch from or dial `ip`? Public addresses always; private,
/// link-local and loopback ones only when a LAN sighting of this window carried them
/// (`lan`), or in the dev rig / tests (`private_ok`).
pub(crate) fn dial_ok(ip: std::net::IpAddr, lan: &[std::net::IpAddr], private_ok: bool) -> bool {
    private_ok || !is_local(ip) || lan.contains(&ip)
}

/// Cut a card's text to [`CARD_TEXT_MAX`] characters (at a character boundary).
pub(crate) fn clip(s: &str) -> String {
    s.chars().take(CARD_TEXT_MAX).collect()
}

/// Fetch a card (§7.1, LAD §4.6): at most [`CARD_BYTES_MAX`], [`CARD_FETCH_SECS`], no
/// redirects, and only from an address [`dial_ok`] allows (the host is resolved once here
/// and the connection pinned to what was checked).
async fn fetch_card(
    url: &str,
    lan: &[std::net::IpAddr],
    private_ok: bool,
) -> Result<AgentCard, String> {
    let url = if url.ends_with(crate::config::CARD_SUFFIX) {
        url.to_owned()
    } else {
        format!(
            "{}{}",
            url.trim_end_matches('/'),
            crate::config::CARD_SUFFIX
        )
    };
    let parsed = url::Url::parse(&url).map_err(|e| format!("card URL: {e}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("a card URL is http or https".into());
    }
    let host = match parsed.host().ok_or("the card URL has no host")? {
        url::Host::Domain(d) => d.to_owned(),
        url::Host::Ipv4(i) => i.to_string(),
        url::Host::Ipv6(i) => i.to_string(),
    };
    let port = parsed.port_or_known_default().unwrap_or(80);
    let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|e| format!("card host: {e}"))?
        .collect();
    if addrs.is_empty() {
        return Err("the card host did not resolve".into());
    }
    if let Some(a) = addrs.iter().find(|a| !dial_ok(a.ip(), lan, private_ok)) {
        return Err(format!(
            "the card URL points at a private address ({}) that no LAN sighting carried",
            a.ip()
        ));
    }
    let http = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .resolve_to_addrs(&host, &addrs)
        .timeout(std::time::Duration::from_secs(CARD_FETCH_SECS))
        .build()
        .map_err(|e| e.to_string())?;
    let mut resp = http
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("card fetch: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("card fetch: HTTP {}", resp.status()));
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| format!("card body: {e}"))? {
        if body.len() + chunk.len() > CARD_BYTES_MAX {
            return Err("card larger than 256 KiB".into());
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|e| format!("not an Agent Card: {e}"))
}

impl Message<ContactWindowOpen> for A2aActor {
    type Reply = Result<Option<ContactWindow>, String>;

    /// In the mailbox, so opens and closes apply in call order: a close that overtook a
    /// still-opening window (a remounted dialog) left no window and a dropped advert.
    async fn handle(
        &mut self,
        m: ContactWindowOpen,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        if m.0 {
            self.ctx.open_window().await.map(Some)
        } else {
            self.ctx.close_window();
            Ok(None)
        }
    }
}

impl Message<ContactsNearby> for A2aActor {
    type Reply = Vec<NearbyAgent>;

    async fn handle(
        &mut self,
        _: ContactsNearby,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.ctx.nearby()
    }
}

impl Message<ContactPair> for A2aActor {
    type Reply = DelegatedReply<Result<mesh::PairCeremony, String>>;

    async fn handle(
        &mut self,
        m: ContactPair,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let c = self.ctx.clone();
        off_mailbox(ctx, async move { c.pair(m.0).await })
    }
}

impl Message<ContactAnswer> for A2aActor {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: ContactAnswer,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.ctx.answer(m.ceremony, m.accept)
    }
}

/// Run `fut` off the mailbox (it rewrites `peers.yaml` and reloads, which asks the agent)
/// and answer the caller with its result.
fn off_mailbox<R, F>(ctx: &mut Context<A2aActor, DelegatedReply<R>>, fut: F) -> DelegatedReply<R>
where
    R: kameo::Reply<Value = R> + Send + 'static,
    F: Future<Output = R> + Send + 'static,
{
    let (reply, tx) = ctx.reply_sender();
    if let Some(tx) = tx {
        tokio::spawn(async move { tx.send(fut.await) });
    }
    reply
}

/// §7.3: the endpoint id in base32, first 20 characters, in groups of 4.
pub fn fingerprint(eid: &str) -> String {
    let Ok(id) = eid.parse::<iroh::EndpointId>() else {
        return String::new();
    };
    let b32 = data_encoding::BASE32_NOPAD.encode(id.as_bytes());
    b32.as_bytes()[..20]
        .chunks(4)
        .map(|c| std::str::from_utf8(c).unwrap_or_default())
        .collect::<Vec<_>>()
        .join(" ")
}

/// 191 (D8): the machines a contact row lists (`fallback`, `eid` first, while the reconcile
/// recorded none: a `peers.yaml` entry), each named by `name_of`.
fn machines(
    shown: Option<&crate::config::Shown>,
    fallback: &[String],
    name_of: &dyn Fn(&str) -> String,
) -> Vec<ContactMachine> {
    let view = |eid: &str, left: bool| ContactMachine {
        eid: eid.to_owned(),
        name: name_of(eid),
        left,
    };
    match shown {
        Some(s) => s.machines.iter().map(|(e, left)| view(e, *left)).collect(),
        None => fallback.iter().map(|e| view(e, false)).collect(),
    }
}

/// One staged contact as `contact_list` shows it.
fn contact_view(
    staged: &crate::config::Staged,
    alias: &str,
    net: Option<&mesh::net::Net>,
    shared: impl FnOnce(&str) -> (Vec<agent::communication::network::ContactOffer>, bool),
    name_of: &dyn Fn(&str) -> String,
) -> Option<Contact> {
    let c = staged.contacts.get(alias)?;
    let shown = staged.shown.get(&c.id);
    let (shared, full_capable) = shared(&c.eid);
    Some(Contact {
        name: alias.to_owned(),
        machine: clip(&c.machine),
        endpoint: c.eid.clone(),
        fingerprint: fingerprint(&c.eid),
        path: reach::path(net, &c.eid),
        exports: shown.map(|s| s.exports.clone()).unwrap_or_default(),
        first_contact: c.first_contact.clone(),
        blocked: c.blocked,
        paired_ms: c.paired_ms,
        shared,
        full_capable,
        id: c.id.clone(),
        problem: None,
        problem_kind: None,
        paired_on: shown.and_then(|s| s.paired_on.clone()),
        machines: machines(shown, &c.eids, name_of),
        full_access: staged.peers.get(alias).is_some_and(|p| p.full_access),
        // 200: `contact_list` fills it from the cached card.
        full_access_from: None,
    })
}

/// 191 (D2): a quarantined record as `contact_list` shows it: refused or waiting, its
/// problem said.
fn quarantined_view(
    staged: &crate::config::Staged,
    q: &crate::config::Quarantined,
    name_of: &dyn Fn(&str) -> String,
) -> Contact {
    let shown = staged.shown.get(&q.id);
    let first: Vec<String> = (!q.eid.is_empty())
        .then(|| q.eid.clone())
        .into_iter()
        .collect();
    Contact {
        name: q.alias.clone(),
        machine: clip(&q.machine),
        endpoint: q.eid.clone(),
        fingerprint: fingerprint(&q.eid),
        path: mesh::PeerPath::Offline,
        exports: shown.map(|s| s.exports.clone()).unwrap_or_default(),
        first_contact: BTreeMap::new(),
        blocked: q.blocked,
        paired_ms: q.paired_ms,
        shared: Vec::new(),
        full_capable: false,
        id: q.id.clone(),
        problem: Some(q.problem.clone()),
        problem_kind: Some(q.kind),
        paired_on: shown.and_then(|s| s.paired_on.clone()),
        machines: machines(shown, &first, name_of),
        full_access: false,
        full_access_from: None,
    }
}

/// Drop export `name` and every grant and first-contact answer naming it.
pub(crate) fn drop_export(file: &mut PeersFile, name: &str) {
    file.exports.remove(name);
    for p in file.peers.values_mut() {
        p.exports.retain(|e| e != name);
        p.first_contact.remove(name);
    }
}

impl Message<ContactsList> for A2aActor {
    type Reply = Vec<Contact>;

    async fn handle(&mut self, _: ContactsList, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.ctx.contact_list()
    }
}

impl crate::Ctx {
    /// [`ContactsList`]: every contact of the mesh (the replicated records), quarantined
    /// ones with their problem. 191 (D1 note 2): an inflation alert on a record is its
    /// problem too (alert only: the contact stays admitted).
    pub(crate) fn contact_list(&self) -> Vec<Contact> {
        let live = self.shared.read().clone();
        let net = self.net();
        let alerts = self.rosters.lock().alerts.clone();
        let name_of = |eid: &str| self.machine_name(eid);
        let mut out: Vec<Contact> = live
            .staged
            .contacts
            .keys()
            .filter_map(|alias| {
                let mut c = contact_view(
                    &live.staged,
                    alias,
                    net.as_deref(),
                    |eid| {
                        let eid = self.offers_eid(eid);
                        (self.offers_of(&eid), self.full_of(&eid))
                    },
                    &name_of,
                )?;
                // 200 (D11): what the card that counts says of full access for us.
                let eid = self.offers_eid(&c.endpoint);
                c.full_access_from = self.offers.lock().get(&eid).and_then(|k| k.full_access);
                c.problem = alerts.get(&c.id).map(|(_, text, _)| text.clone());
                c.problem_kind = c.problem.as_ref().map(|_| ProblemKind::Alert);
                Some(c)
            })
            .collect();
        out.extend(
            live.staged
                .quarantined
                .iter()
                .map(|q| quarantined_view(&live.staged, q, &name_of)),
        );
        out
    }

    /// 191 (D2): the replicated contact records, as the agent's store reads them now. `me`
    /// is this engine's mesh id (the store's own, unless a test's store has none).
    pub(crate) async fn records(&self) -> Result<agent::store::contacts::Records, String> {
        let agents = self.agents()?;
        let mut r = self
            .ask(&agents, agent::communication::network::ContactRecords)
            .await?;
        if let Some(net) = self.net() {
            r.me.clone_from(&net.id.0);
        }
        Ok(r)
    }

    /// 191 (D2): write record columns, then run the reconcile at once (the RPC's answer
    /// reflects it) and tell the UI.
    pub(crate) async fn commit_records(
        &self,
        writes: Vec<agent::store::contacts::RecordWrite>,
    ) -> Result<(), String> {
        if !writes.is_empty() {
            let agents = self.agents()?;
            self.ask(
                &agents,
                agent::communication::network::WriteContactRecords(writes),
            )
            .await?;
        }
        self.reconcile_now().await
    }

    /// The group a contact RPC names (its alias, or a record id: a held row's, whose alias a
    /// live contact may share while a rename lands), or its records whose merge chain is
    /// broken (alias or record id): every record it writes. Root first.
    fn targets<'r>(
        a: &'r records::Analysis<'r>,
        r: &'r agent::store::contacts::Records,
        name: &str,
        quarantined_too: bool,
    ) -> Result<Vec<&'r agent::store::contacts::ContactRow>, String> {
        if let Some(g) = records::root_by_alias(a, name).or_else(|| records::group_of(a, name)) {
            return Ok(g.members.clone());
        }
        if quarantined_too {
            let rows: Vec<_> = r
                .contacts
                .iter()
                .filter(|c| c.removed_ms.is_none())
                .filter(|c| c.id == name || c.alias.as_deref() == Some(name))
                .collect();
            if !rows.is_empty() {
                return Ok(rows);
            }
        }
        Err(format!("no contact named {name}"))
    }
}

impl Message<ContactUpdate> for A2aActor {
    type Reply = DelegatedReply<Result<Contact, String>>;

    async fn handle(
        &mut self,
        m: ContactUpdate,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let c = self.ctx.clone();
        off_mailbox(ctx, async move { c.update_contact(m).await })
    }
}

/// 191 (D2): the writes that set export `xid`'s grant (and, with `answer`, its first-contact
/// answer) for a group: every member that has a row for it is written, so the strictest read
/// never pins an old value; with no row anywhere, the root gets one.
fn grant_writes(
    r: &agent::store::contacts::Records,
    members: &[&agent::store::contacts::ContactRow],
    xid: &str,
    granted: Option<bool>,
    answer: impl Fn(Option<&str>) -> Option<FirstContact>,
) -> Vec<agent::store::contacts::RecordWrite> {
    use agent::store::contacts::{Cell, RecordTable, RecordWrite};
    let rows: Vec<_> = r
        .grants
        .iter()
        .filter(|g| g.export.as_deref() == Some(xid))
        .filter(|g| {
            members
                .iter()
                .any(|m| g.contact.as_deref() == Some(m.id.as_str()))
        })
        .collect();
    let write = |id: String, contact: &str, old: Option<&str>| {
        let mut set = vec![
            ("contact", Cell::Text(contact.to_owned())),
            ("export", Cell::Text(xid.to_owned())),
        ];
        if let Some(g) = granted {
            set.push(("granted", Cell::Int(i64::from(g))));
        }
        if let Some(a) = answer(old) {
            set.push((
                "first_contact",
                Cell::Text(records::answer_text(a).to_owned()),
            ));
        }
        RecordWrite::new(RecordTable::Grants, id, set)
    };
    if rows.is_empty() {
        let root = &members[0].id;
        return vec![write(format!("{root}|{xid}"), root, None)];
    }
    rows.iter()
        .map(|g| {
            write(
                g.id.clone(),
                g.contact.as_deref().unwrap_or_default(),
                g.first_contact.as_deref(),
            )
        })
        .collect()
}

/// 191: what `contact_update` answers a patch that still names `exports` (the whole list an
/// earlier build sent, which revoked every grant its machine could not see).
pub const EXPORTS_REPLACED: &str = "contact_update: `exports` was replaced by `grant` / `revoke`";

/// 191 (D2): the grant writes of `patch` for group `members` (root first), read against
/// `staged`, this machine's view. `grant` and `revoke` touch only the exports they name (by
/// name or id, keyed by id: a revoke wins over a grant of the same export). No export is
/// ever removed here: one no grant holds has no route and is reused by the next share of
/// its chat. `exports` is refused.
pub(crate) fn grant_patch(
    r: &agent::store::contacts::Records,
    staged: &crate::config::Staged,
    members: &[&agent::store::contacts::ContactRow],
    patch: &ContactPatch,
) -> Result<Vec<agent::store::contacts::RecordWrite>, String> {
    if patch.exports.is_some() {
        return Err(EXPORTS_REPLACED.to_owned());
    }
    let root = members[0];
    let alias = root.alias.as_deref().unwrap_or_default();
    let xid_of = |x: &str| {
        staged
            .export_ids
            .get(x)
            .map(String::as_str)
            .or_else(|| {
                r.exports
                    .iter()
                    .find(|e| e.removed_ms.is_none() && e.id == x)
                    .map(|e| e.id.as_str())
            })
            .ok_or_else(|| format!("peer {alias}: unknown export {x:?}"))
    };
    let mut want: BTreeMap<&str, bool> = BTreeMap::new();
    for x in patch.grant.iter().flatten() {
        want.insert(xid_of(x)?, true);
    }
    for x in patch.revoke.iter().flatten() {
        want.insert(xid_of(x)?, false);
    }
    let a = records::analyze(r);
    let grants = records::grants_of(r, members);
    let mut w = Vec::new();
    for (xid, on) in want {
        // §6.3: one principal per exported chat. Another unblocked contact's effective grant,
        // from the records (a chat this machine does not publish too, which staging skips
        // silently), or an HTTP peer of this machine's file, holds it.
        let name = staged
            .export_ids
            .iter()
            .find(|(_, id)| id.as_str() == xid)
            .map_or(xid, |(n, _)| n.as_str());
        let holder = a
            .groups
            .iter()
            .filter(|g| !members.iter().any(|m| m.id == g.root.id))
            .filter(|g| !g.members.iter().any(|m| m.blocked.is_some_and(|b| b != 0)))
            .find(|g| {
                records::grants_of(r, &g.members)
                    .get(xid)
                    .is_some_and(|rows| records::effective(rows).0 == Some(true))
            })
            .map(|g| g.root.alias.clone().unwrap_or_else(|| g.root.id.clone()))
            .or_else(|| {
                let peer = staged.exports.get(name)?.peer.as_deref()?;
                (!staged.contacts.contains_key(peer)).then(|| peer.to_owned())
            });
        if on && let Some(other) = holder {
            return Err(format!(
                "export {name} is granted to both {other} and {alias}; one principal per exported session"
            ));
        }
        // 183 F2: sharing counts as allowing: an export the group does not hold now starts
        // on "Always allow"; a stored answer (Ask first included) is kept.
        let fresh = on
            && grants
                .get(xid)
                .is_none_or(|rows| records::effective(rows).0 != Some(true));
        w.extend(grant_writes(r, members, xid, Some(on), |old| {
            (fresh && old.is_none()).then_some(FirstContact::Allow)
        }));
    }
    Ok(w)
}

impl crate::Ctx {
    /// [`ContactUpdate`]: 191 (D2): writes the contact's records (every record of a merged
    /// group), so every machine of the mesh sees it. A held contact (waiting or refused here)
    /// is named by its alias or record id: its grants are records like any other, and apply
    /// wherever it is admitted.
    pub(crate) async fn update_contact(&self, m: ContactUpdate) -> Result<Contact, String> {
        use agent::store::contacts::{Cell, RecordTable, RecordWrite};
        let ContactUpdate { name, patch } = m;
        let r = self.records().await?;
        let a = records::analyze(&r);
        let members = Self::targets(&a, &r, &name, true)?;
        let root = members[0];
        let live = self.shared.read().clone();
        let mut w = grant_patch(&r, &live.staged, &members, &patch)?;
        // 200 (D1): one single-column write on every record of the group (on only when all of
        // them read 1); the staged `Peer` changes, so the grant epoch rotates.
        if let Some(on) = patch.full_access {
            for m in &members {
                w.push(RecordWrite::new(
                    RecordTable::Contacts,
                    m.id.clone(),
                    vec![("full_access", Cell::Int(i64::from(on)))],
                ));
            }
        }
        if let Some(blocked) = patch.blocked {
            let was = members.iter().any(|m| m.blocked.is_some_and(|b| b != 0));
            for m in &members {
                w.push(RecordWrite::new(
                    RecordTable::Contacts,
                    m.id.clone(),
                    vec![("blocked", Cell::Int(i64::from(blocked)))],
                ));
            }
            // Unblock writes a fresh binding: the blocked one stays revoked everywhere.
            if was && !blocked {
                w.push(RecordWrite::new(
                    RecordTable::Contacts,
                    root.id.clone(),
                    vec![("binding", Cell::Text(records::fresh_binding()))],
                ));
            }
        }
        // A contact named by its record id keeps its alias.
        let current = root.alias.clone().unwrap_or_else(|| name.clone());
        let alias = match patch.name {
            Some(new) if new != current => {
                crate::config::check_alias(&new)?;
                let taken =
                    r.contacts.iter().any(|c| {
                        c.removed_ms.is_none() && c.alias.as_deref() == Some(new.as_str())
                    }) || live.staged.peers.contains_key(&new);
                if taken {
                    return Err(format!("a peer named {new} already exists"));
                }
                w.push(RecordWrite::new(
                    RecordTable::Contacts,
                    root.id.clone(),
                    vec![("alias", Cell::Text(new.clone()))],
                ));
                new
            }
            _ => current,
        };
        self.commit_records(w).await?;
        let live = self.shared.read().clone();
        if let Some(f) = live
            .staged
            .contacts
            .get(&alias)
            .filter(|f| f.blocked && f.id == root.id)
        {
            for eid in f.eids.clone() {
                self.contact_gone(&alias, &eid, false).await;
            }
        }
        self.contact_list()
            .into_iter()
            .find(|c| c.id == root.id)
            .ok_or_else(|| format!("contact {alias} vanished"))
    }
}

impl Message<ContactRemove> for A2aActor {
    type Reply = DelegatedReply<Result<(), String>>;

    async fn handle(
        &mut self,
        m: ContactRemove,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let c = self.ctx.clone();
        off_mailbox(ctx, async move { c.remove_contact(m).await })
    }
}

impl crate::Ctx {
    /// [`ContactRemove`]: 191 (D2): a block writes `blocked` on every record of the group; a
    /// removal tombstones them (`removed_ms`, never cleared). A quarantined record can be
    /// removed by its alias or id.
    pub(crate) async fn remove_contact(&self, m: ContactRemove) -> Result<(), String> {
        use agent::store::contacts::{Cell, RecordTable, RecordWrite};
        let r = self.records().await?;
        let a = records::analyze(&r);
        let members = Self::targets(&a, &r, &m.name, !m.block)?;
        let ids: Vec<String> = members.iter().map(|c| c.id.clone()).collect();
        let eids: Vec<String> = r
            .endpoints
            .iter()
            .filter(|e| e.contact.as_ref().is_some_and(|c| ids.contains(c)))
            .filter_map(|e| e.eid.clone())
            .collect();
        let now = mesh::identity::now_ms() as i64;
        let w = ids
            .iter()
            .map(|id| {
                let set = if m.block {
                    vec![("blocked", Cell::Int(1))]
                } else {
                    vec![("removed_ms", Cell::Int(now))]
                };
                RecordWrite::new(RecordTable::Contacts, id.clone(), set)
            })
            .collect();
        self.commit_records(w).await?;
        // The reconcile revoked the binding and retired its grant epoch (its tasks end); the
        // other side hears a removal best effort.
        for eid in eids {
            self.contact_gone(&m.name, &eid, !m.block).await;
        }
        Ok(())
    }
}

impl Message<ExportsList> for A2aActor {
    type Reply = DelegatedReply<Vec<Export>>;

    async fn handle(
        &mut self,
        _: ExportsList,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let c = self.ctx.clone();
        off_mailbox(ctx, async move { c.exports().await })
    }
}

/// 191: [`crate::Ctx::exports`]'s pause for a chat export whose session this machine does
/// not hold (yet: it may still be replicating).
pub const PAUSED_ELSEWHERE: &str = "chat not on this machine";
/// 191 (D4): an agent export whose home machine left the mesh: it opens no new inbox.
pub const PAUSED_HOME_LEFT: &str = "its machine left the mesh";
/// 191: a chat export whose chat runs an external harness: no machine publishes it.
pub const PAUSED_NOT_NATIVE: &str = "its chat runs an external harness";

/// 191 (W-41): a chat deleted here, as [`ChatDeletions`] forwards it to [`A2aActor`].
#[derive(Clone, Copy, Debug)]
pub struct ChatDeleted(pub SessionId);

impl Message<ChatDeleted> for A2aActor {
    type Reply = ();

    async fn handle(&mut self, m: ChatDeleted, _: &mut Context<Self, Self::Reply>) {
        let c = self.ctx.clone();
        tokio::spawn(async move {
            if let Err(e) = c.session_deleted(m.0).await {
                tracing::warn!(session = %m.0, "a2a: exports of a deleted chat not dropped: {e}");
            }
        });
    }
}

/// The subscriber of every [`agent::AgentEvent`] (token deltas included) that hands only
/// `SessionDeleted` on to [`A2aActor`], so its mailbox never carries the stream.
#[derive(kameo::Actor)]
pub struct ChatDeletions(kameo::actor::ActorRef<A2aActor>);

impl Message<agent::AgentEvent> for ChatDeletions {
    type Reply = ();

    async fn handle(&mut self, ev: agent::AgentEvent, _: &mut Context<Self, Self::Reply>) {
        if let agent::AgentEvent::SessionDeleted { session } = ev {
            let _ = self.0.tell(ChatDeleted(session)).await;
        }
    }
}

/// Spawn [`ChatDeletions`] for `a2a` and subscribe it to `rt`'s agent events.
pub(crate) async fn subscribe_deletions(
    rt: &runtime::Runtime,
    a2a: &kameo::actor::ActorRef<A2aActor>,
) {
    use kameo::actor::Spawn as _;
    let fwd = ChatDeletions::spawn_with_mailbox(
        ChatDeletions(a2a.clone()),
        runtime::subscriber_mailbox(),
    );
    rt.subscribe::<ChatDeletions, agent::AgentEvent>(&fwd).await;
}

impl crate::Ctx {
    /// 191 (W-41): chat `session` was deleted here (`SessionDeleted`, published by the engine
    /// that ran the delete, by `DeleteSession` or a private tree's close, never for a
    /// replicated one): every live export of it is tombstoned, whatever its home (one another
    /// machine made for this chat too), and the grants naming it stop counting. A second
    /// machine's tombstone of the same export would be the same write. A missing session is
    /// never read as a deletion (it may be on another machine, or not replicated yet).
    pub(crate) async fn session_deleted(&self, session: SessionId) -> Result<(), String> {
        use agent::store::contacts::{Cell, RecordTable, RecordWrite};
        let r = self.records().await?;
        let s = session.to_string();
        let now = mesh::identity::now_ms() as i64;
        let w: Vec<_> = r
            .exports
            .iter()
            .filter(|x| x.removed_ms.is_none() && x.session.as_deref() == Some(s.as_str()))
            .map(|x| {
                RecordWrite::new(
                    RecordTable::Exports,
                    x.id.clone(),
                    vec![("removed_ms", Cell::Int(now))],
                )
            })
            .collect();
        if w.is_empty() {
            return Ok(());
        }
        tracing::info!(%session, exports = w.len(), "a2a: chat deleted; its exports removed");
        self.commit_records(w).await
    }

    /// `a2a_exports`: every export of the mesh; a chat's pause is read now (W-41). One whose
    /// chat this machine lacks is listed paused ([`PAUSED_ELSEWHERE`]; its session may be
    /// on another machine, or on its way), never tombstoned: only a deletion is
    /// ([`Self::session_deleted`]). 191: so is one this machine's routes leave out (its chat
    /// not here, or [`PAUSED_NOT_NATIVE`]), so a contact's share of it reads by name and
    /// state wherever it is granted.
    pub(crate) async fn exports(&self) -> Vec<Export> {
        let live = self.shared.read().clone();
        let agents = self.agents().ok();
        let r = self.records().await.unwrap_or_default();
        let left_out: Vec<crate::config::Export> = live
            .staged
            .export_ids
            .iter()
            .filter(|(n, _)| {
                !live.staged.exports.contains_key(*n) && !live.staged.agents.contains_key(*n)
            })
            .filter_map(|(n, id)| {
                let row = r.exports.iter().find(|x| x.id == *id)?;
                Some(crate::config::Export {
                    name: n.clone(),
                    session: row.session.as_deref()?.parse().ok()?,
                    title: n.clone(),
                    description: row.description.clone().unwrap_or_default(),
                    peer: None,
                })
            })
            .collect();
        let mut out = Vec::new();
        for e in live.staged.exports.values().chain(&left_out) {
            let id = live.staged.export_ids.get(&e.name).cloned();
            let left = left_out.iter().any(|x| x.name == e.name);
            // Why the routes leave it out, when its chat is here: an external harness runs
            // it, or the check failed (its error is the pause; logged).
            let unpublished = match &agents {
                Some(a) if left => match self.ask(a, ValidateExport { session: e.session }).await {
                    Err(err) if err.contains(NOT_NATIVE) => {
                        Some(PAUSED_NOT_NATIVE.to_owned())
                    }
                    Err(err) if !err.starts_with(UNKNOWN_SESSION) => {
                        tracing::warn!(export = %e.name, "a2a: export not published here: {err}");
                        Some(err)
                    }
                    _ => None,
                },
                _ => None,
            };
            let paused = match &agents {
                _ if unpublished.is_some() => unpublished,
                Some(a) => match self.ask(a, ChatPaused { session: e.session }).await {
                    Ok(p) => p,
                    Err(err) if err.starts_with(UNKNOWN_SESSION) => {
                        Some(PAUSED_ELSEWHERE.to_owned())
                    }
                    Err(_) => None,
                },
                None => None,
            };
            out.push(Export {
                id,
                name: e.name.clone(),
                target: ExportTarget::Chat { session: e.session },
                description: e.description.clone(),
                paused,
            });
        }
        out.extend(live.staged.agents.values().map(|a| {
            Export {
                id: live.staged.export_ids.get(&a.name).cloned(),
                name: a.name.clone(),
                target: ExportTarget::Agent {
                    agent: a.agent.clone(),
                },
                description: a.description.clone(),
                // 191 (D4): its home left the mesh: no new inbox (the UI offers to rehome it).
                paused: (a.home == agent::communication::network::InboxHome::Left)
                    .then(|| PAUSED_HOME_LEFT.to_owned()),
            }
        }));
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }
}

impl Message<ExportPut> for A2aActor {
    type Reply = DelegatedReply<Result<(), String>>;

    async fn handle(&mut self, m: ExportPut, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let c = self.ctx.clone();
        off_mailbox(ctx, async move { c.put_export(m.0).await })
    }
}

impl crate::Ctx {
    /// [`ExportPut`]: 191 (D2): by its id, else by its name (create or replace); a new one
    /// gets a fresh id and this machine as its home. A name another live export holds is
    /// refused. 191 (D4): re-sharing an agent export whose home left the mesh (a home that is
    /// neither this machine nor one of the mesh, or none) moves its home here; a home that is
    /// a machine of the mesh stays, and without a mesh endpoint here nothing is rehomed.
    pub(crate) async fn put_export(&self, exp: Export) -> Result<(), String> {
        use agent::store::contacts::{Cell, RecordTable, RecordWrite};
        let Export {
            id,
            name,
            target,
            description,
            ..
        } = exp;
        crate::config::check_alias(&name)?;
        let r = self.records().await?;
        let live: Vec<_> = r
            .exports
            .iter()
            .filter(|x| x.removed_ms.is_none())
            .collect();
        let existing = match &id {
            Some(id) => Some(
                *live
                    .iter()
                    .find(|x| x.id == *id)
                    .ok_or_else(|| format!("no export {id}"))?,
            ),
            None => live
                .iter()
                .find(|x| x.name.as_deref() == Some(name.as_str()))
                .copied(),
        };
        if live.iter().any(|x| {
            x.name.as_deref() == Some(name.as_str()) && Some(&x.id) != existing.map(|e| &e.id)
        }) {
            return Err(format!("an export named {name} already exists"));
        }
        let agent_target = matches!(target, ExportTarget::Agent { .. });
        let (session, agent) = match target {
            ExportTarget::Chat { session } => (Cell::Text(session.to_string()), Cell::Null),
            ExportTarget::Agent { agent } => (Cell::Null, Cell::Text(agent)),
        };
        // 190 (D8): a re-pointed export (another chat or agent) stays shared, in full; its
        // grant epoch rotates (`Staged::same_grant`), so a held first message fails.
        let mut set = vec![
            ("name", Cell::Text(name.clone())),
            ("session", session),
            ("agent", agent),
            ("title", Cell::Text(name.clone())),
            ("description", Cell::Text(description)),
        ];
        let id = match existing {
            Some(x) => {
                if agent_target && self.home_left(x.home.as_deref(), &r.me) {
                    tracing::info!(export = %x.id, from = ?x.home, "191: agent export rehomed here (its home left the mesh)");
                    set.push(("home", Cell::Text(r.me.clone())));
                }
                x.id.clone()
            }
            None => {
                set.push(("home", Cell::Text(r.me.clone())));
                Uuid::now_v7().to_string()
            }
        };
        self.commit_records(vec![RecordWrite::new(RecordTable::Exports, id, set)])
            .await
    }

    /// 191 (D4): `home` left the mesh, read as [`records::stage_records`] stages
    /// `InboxHome::Left` (neither `me` nor a machine of the mesh, or none). Without a mesh
    /// endpoint here the mesh cannot be read: not left (fail closed: nothing is rehomed).
    fn home_left(&self, home: Option<&str>, me: &str) -> bool {
        let Some(net) = self.net() else {
            return false;
        };
        let family = members::family(Some(net.as_ref()));
        home.is_none_or(|h| h != me && !family.contains(h))
    }
}

impl Message<ExportRemove> for A2aActor {
    type Reply = DelegatedReply<Result<(), String>>;

    async fn handle(
        &mut self,
        m: ExportRemove,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let c = self.ctx.clone();
        off_mailbox(ctx, async move { c.remove_export(&m.name).await })
    }
}

impl crate::Ctx {
    /// [`ExportRemove`] (by id or name): 191: tombstoned (`removed_ms`); grants naming it
    /// stop counting.
    pub(crate) async fn remove_export(&self, name: &str) -> Result<(), String> {
        use agent::store::contacts::{Cell, RecordTable, RecordWrite};
        let r = self.records().await?;
        let x = r
            .exports
            .iter()
            .filter(|x| x.removed_ms.is_none())
            .find(|x| x.id == name || x.name.as_deref() == Some(name))
            .ok_or_else(|| format!("no export named {name}"))?;
        let now = mesh::identity::now_ms() as i64;
        self.commit_records(vec![RecordWrite::new(
            RecordTable::Exports,
            x.id.clone(),
            vec![("removed_ms", Cell::Int(now))],
        )])
        .await
    }
}

impl Message<ChatArchived> for A2aActor {
    type Reply = bool;

    async fn handle(&mut self, m: ChatArchived, _: &mut Context<Self, Self::Reply>) -> bool {
        self.ctx.chat_archived(m.session).await
    }
}

impl crate::Ctx {
    /// [`ChatArchived`].
    pub(crate) async fn chat_archived(&self, session: SessionId) -> bool {
        let live = self.shared.read().clone();
        let names: Vec<&String> = live
            .staged
            .exports
            .values()
            .filter(|e| e.session == session)
            .map(|e| &e.name)
            .collect();
        if names.is_empty() {
            return false;
        }
        self.rt.publish(crate::A2aChanged {}).await;
        // 183 F1: the contacts it is shared with see it `unavailable` (or back) at once.
        let eids: Vec<String> = live
            .staged
            .contacts
            .iter()
            .filter(|(alias, f)| {
                !f.blocked
                    && live
                        .staged
                        .peers
                        .get(*alias)
                        .is_some_and(|p| p.exports.iter().any(|x| names.contains(&x)))
            })
            .map(|(_, f)| f.eid.clone())
            .collect();
        self.notify_offers(eids);
        true
    }
}

impl Message<ContactFirstAnswer> for A2aActor {
    type Reply = DelegatedReply<Result<(), String>>;

    async fn handle(
        &mut self,
        m: ContactFirstAnswer,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let c = self.ctx.clone();
        off_mailbox(ctx, async move { c.first_answer(m).await })
    }
}

impl crate::Ctx {
    /// 191 (D2): write `answer` as the group's first-contact answer for `export`.
    async fn write_answer(
        &self,
        contact: &str,
        export: &str,
        answer: FirstContact,
    ) -> Result<(), String> {
        let r = self.records().await?;
        let a = records::analyze(&r);
        let members = Self::targets(&a, &r, contact, false)?;
        let xid = self
            .shared
            .read()
            .staged
            .export_ids
            .get(export)
            .cloned()
            .ok_or_else(|| format!("no export named {export}"))?;
        let w = grant_writes(&r, &members, &xid, None, |_| Some(answer));
        self.commit_records(w).await
    }

    /// W-27: the owner answered a `contact_first` item. Allow once lets the waiting tasks
    /// through and keeps asking; Always writes `first_contact: allow` first; Block writes
    /// `blocked: true` (the gate refuses the contact from now on, its waiting tasks fail).
    /// With no task waiting for (contact, export) — the select in Settings — the answer is
    /// only that export's standing default (§9.4): `ask`, `allow`, `block`, each stored, so an
    /// owner's explicit Ask first survives taking the share back and giving it again (F2,
    /// review 1; an absent entry also reads Ask first, and a new grant writes Always allow).
    /// 191: the answer is a record write, seen by every machine. 191 (D4): a hold on another
    /// machine of the mesh is released only by its owner, on a grant change: Always and Block
    /// are such writes (the owner's reconcile releases or fails it); Allow once writes
    /// nothing, so it is refused here.
    async fn first_answer(&self, m: ContactFirstAnswer) -> Result<(), String> {
        let ContactFirstAnswer {
            contact,
            export,
            answer,
        } = m;
        let waiting = match self.agents() {
            Ok(agents) => agents
                .ask(agent::communication::network::ContactsHeld)
                .await
                .map_err(|e| format!("agent unavailable: {e}"))?
                .iter()
                .any(|h| h.contact == contact && h.export == export),
            Err(_) => false,
        };
        if !waiting {
            // P3 review M1: a holds read that fails is the answer's error, never "nothing
            // waits" (which would write the standing default and return Ok).
            let elsewhere = self
                .waiting_list(true)
                .await?
                .iter()
                .any(|w| w.elsewhere && w.contact == contact && w.export == export);
            match answer {
                FirstContact::Ask if elsewhere => {
                    return Err(format!(
                        "the message from {contact} waits on the machine that holds the chat: \
                         answer Allow once there, or Always here"
                    ));
                }
                FirstContact::Block if elsewhere => {
                    return self
                        .remove_contact(ContactRemove {
                            name: contact,
                            block: true,
                        })
                        .await;
                }
                _ => return self.write_answer(&contact, &export, answer).await,
            }
        }
        match answer {
            FirstContact::Ask => {}
            FirstContact::Allow => {
                self.write_answer(&contact, &export, FirstContact::Allow)
                    .await?;
            }
            FirstContact::Block => {
                return self
                    .remove_contact(ContactRemove {
                        name: contact,
                        block: true,
                    })
                    .await;
            }
        }
        let agents = self.agents()?;
        self.ask(
            &agents,
            agent::communication::network::ContactRelease {
                contact,
                export: Some(export),
                allow: true,
            },
        )
        .await
        .map(|_| ())
    }

    /// 183 §7.2: keep each contact's direct addresses from its live `theuth/a2a/1`
    /// connection (191: in this machine's local `contact_addrs`) when they differ from the
    /// stored ones, so a LAN-only pair dials them after a restart. Nothing is written without
    /// a live connection.
    pub(crate) async fn remember_addrs(&self) {
        /// 183 Z3-R2: at most one write a minute per contact (path churn must not turn
        /// into disk churn).
        const EVERY: std::time::Duration = std::time::Duration::from_secs(60);
        let Some(net) = self.net() else { return };
        let live = self.shared.read().clone();
        let now_at = std::time::Instant::now();
        // 191 (D3): every machine of a contact, each with its own addresses: (alias, eid,
        // addresses).
        let changed: Vec<(String, String, Vec<String>)> = {
            let s = self.state.lock();
            live.staged
                .contacts
                .iter()
                .filter(|(_, f)| !f.blocked)
                .filter(|(alias, _)| {
                    s.addr_written
                        .get(*alias)
                        .is_none_or(|t| now_at.duration_since(*t) >= EVERY)
                })
                .flat_map(|(alias, f)| f.eids.iter().map(move |eid| (alias, eid)))
                .filter_map(|(alias, eid)| {
                    let now = net.a2a_direct_addrs(eid);
                    (!now.is_empty() && live.staged.addrs.get(eid) != Some(&now)).then(|| {
                        let addrs = now.iter().map(ToString::to_string).collect();
                        (alias.clone(), eid.clone(), addrs)
                    })
                })
                .collect()
        };
        if changed.is_empty() {
            return;
        }
        let Ok(agents) = self.agents() else { return };
        let mut result = Ok(());
        for (alias, eid, addrs) in changed {
            self.state.lock().addr_written.insert(alias, now_at);
            if let Err(e) = self
                .ask(
                    &agents,
                    agent::communication::network::SetContactAddrs { eid, addrs },
                )
                .await
            {
                result = Err(e);
            }
        }
        let _ = self.reconcile_now().await;
        let mut s = self.state.lock();
        match result {
            Ok(()) => s.addr_error = None,
            Err(e) if s.addr_error.as_ref() != Some(&e) => {
                tracing::warn!("a2a: remember contact addresses: {e}");
                s.addr_error = Some(e);
            }
            Err(_) => {}
        }
    }

    /// A contact was removed or blocked: its held first-contact tasks fail, its
    /// connections close, and (for a removal) the other side hears a best-effort `Revoke`.
    pub(crate) async fn contact_gone(&self, alias: &str, eid: &str, revoke: bool) {
        if let Ok(agents) = self.agents()
            && let Err(e) = self
                .ask(
                    &agents,
                    agent::communication::network::ContactRelease {
                        contact: alias.to_owned(),
                        export: None,
                        allow: false,
                    },
                )
                .await
        {
            tracing::warn!(%alias, "a2a: release held tasks: {e}");
        }
        if let Some(net) = self.net() {
            if revoke {
                tokio::spawn(ceremony::revoke(net, eid.to_owned()));
            } else {
                net.close_a2a(eid);
            }
        }
    }

    /// 191 (D2 step 5, D4): release the holds whose answer is now a grant, wherever it was
    /// written. Only this machine's own holds (the agent's memory: it holds the tasks), and
    /// only on their grant: `first_contact: allow` on an export still granted starts them
    /// (`start_held` checks the owner first), `block` fails them. Anything else waits.
    pub(crate) async fn release_answered(&self, agents: &kameo::actor::ActorRef<agent::Agents>) {
        let held = match agents
            .ask(agent::communication::network::ContactsHeld)
            .await
        {
            Ok(held) => held,
            Err(e) => {
                tracing::warn!("a2a: first-contact holds: {e}");
                return;
            }
        };
        let live = self.shared.read().clone();
        for h in held {
            let Some(allow) = release_of(&live.staged, &h.contact, &h.export) else {
                continue;
            };
            tracing::info!(contact = %h.contact, export = %h.export, allow, "191: a held first message's answer arrived as a grant");
            if let Err(e) = self
                .ask(
                    agents,
                    agent::communication::network::ContactRelease {
                        contact: h.contact.clone(),
                        export: Some(h.export.clone()),
                        allow,
                    },
                )
                .await
            {
                tracing::warn!(contact = %h.contact, "a2a: release held tasks: {e}");
            }
        }
    }
}

/// 191 (D2 step 5, D4): what [`crate::Ctx::release_answered`] does with a hold of (`contact`,
/// `export`): `Some(true)` starts it (`allow` on an export the contact is still granted),
/// `Some(false)` fails it (`block`, which an unknown stored answer reads as), `None` leaves
/// it waiting: a contact not admitted here (blocked, quarantined, unknown), an `ask`, or an
/// `allow` on an export no longer granted.
fn release_of(staged: &crate::config::Staged, contact: &str, export: &str) -> Option<bool> {
    let f = staged.contacts.get(contact).filter(|f| !f.blocked)?;
    let granted = staged
        .peers
        .get(contact)
        .is_some_and(|p| p.exports.iter().any(|e| e == export));
    match f.first_contact.get(export) {
        Some(FirstContact::Allow) if granted => Some(true),
        Some(FirstContact::Block) => Some(false),
        _ => None,
    }
}

impl crate::Ctx {
    /// 191 (D4): `contact_holds` changed (a row of any machine arrived, changed or went; the
    /// record replica publishes nothing for it here): every surface refetches (`A2aChanged`)
    /// and Attention derives again. Returns whether it changed.
    pub(crate) async fn holds_tick(&self) -> bool {
        use std::hash::{Hash, Hasher};
        let Ok(agents) = self.agents() else {
            return false;
        };
        let rows = match self
            .ask(&agents, agent::communication::network::ContactHolds)
            .await
        {
            Ok(rows) => rows,
            Err(e) => {
                tracing::debug!("a2a: contact_holds: {e}");
                return false;
            }
        };
        // P3 review N2: only the live rows, so a row that passes its end (an offline owner's)
        // changes the hash and leaves every surface at the next tick.
        let now = mesh::identity::now_ms() as i64;
        let mut h = std::hash::DefaultHasher::new();
        for r in rows.iter().filter(|r| hold_live(r.until_ms, now)) {
            r.hash(&mut h);
        }
        let hash = h.finish();
        if self.state.lock().holds_hash.replace(hash) == Some(hash) {
            return false;
        }
        nudge_attention();
        self.rt.publish(crate::A2aChanged {}).await;
        true
    }
}

/// 183 W-27: a first-contact hold began or ended: the inbox derives again now (its alert
/// recheck), so the `contact_first` item shows or goes without waiting for another event.
pub(crate) fn nudge_attention() {
    tokio::spawn(async {
        if let Ok(Some(inbox)) = kameo::actor::ActorRef::<roster::attention::Attention>::lookup(
            roster::attention::ATTENTION_ACTOR,
        ) {
            let _ = inbox.tell(roster::attention::AlertDue).await;
        }
    });
}

impl crate::Ctx {
    /// 183 W-27, F3: the (contact, export) pairs whose first message waits for the owner,
    /// from the agent's holds and the contacts' facts. The one source of the Attention item,
    /// `contact_first_waiting`, the Communication window, the Contacts row and the strip.
    /// 190: holds only (no queued rows). 191 (D4): every machine's ([`Self::waiting_list`]);
    /// a read that fails is logged: the other machines' rows unread, this machine's own
    /// holds still list.
    pub(crate) async fn first_waiting(&self) -> Vec<FirstWaiting> {
        self.waiting_list(false).await.unwrap_or_else(|e| {
            tracing::warn!("a2a: first-contact holds: {e}");
            Vec::new()
        })
    }

    /// [`Self::first_waiting`]: this machine's own holds from the agent, then the other
    /// machines' `contact_holds` rows (`elsewhere`), cleaned, cut and checked
    /// ([`held_row`]). P3 review S2: one entry per (contact, export), its own hold first,
    /// else the oldest row. `Ok(empty)` with no agent; `strict`: a rows read that fails is
    /// the error.
    pub(crate) async fn waiting_list(&self, strict: bool) -> Result<Vec<FirstWaiting>, String> {
        let Ok(agents) = self.agents() else {
            return Ok(Vec::new());
        };
        let held = agents
            .ask(agent::communication::network::ContactsHeld)
            .await
            .map_err(|e| format!("agent unavailable: {e}"))?;
        let mut rows = match self
            .ask(&agents, agent::communication::network::ContactHolds)
            .await
        {
            Ok(rows) => rows,
            Err(e) if !strict => {
                tracing::warn!("a2a: contact_holds: {e}");
                Vec::new()
            }
            Err(e) => return Err(e),
        };
        let live = self.shared.read().clone();
        let mut out: Vec<FirstWaiting> = held
            .into_iter()
            .map(|h| FirstWaiting {
                machine: h.machine,
                contact: h.contact,
                export: h.export,
                session: h.session,
                since_ms: h.since_ms,
                waiting: h.waiting as u32,
                preview: clean_preview(&h.preview),
                queued: false,
                position: 0,
                elsewhere: false,
            })
            .collect();
        let now = mesh::identity::now_ms() as i64;
        rows.sort_by_key(|r| r.since_ms);
        for w in rows.into_iter().filter_map(|r| {
            held_row(r, now, |c, e, session, inbox| {
                hold_target(&live.staged, e, session, inbox)
                    && live.staged.contacts.get(c).is_some_and(|f| !f.blocked)
            })
        }) {
            if !out
                .iter()
                .any(|o| o.contact == w.contact && o.export == w.export)
            {
                out.push(w);
            }
        }
        out.sort_by(|a, b| {
            (&a.contact, &a.export, a.since_ms).cmp(&(&b.contact, &b.export, b.since_ms))
        });
        Ok(out)
    }
}

/// 191 (D4, P3 review M1, N2): a hold row lives until its `until_ms`, which no real hold
/// sets past now + [`HOLD_TTL`](agent::communication::network::HOLD_TTL) (its newest task
/// arrived by now), give or take ten minutes of clock skew between machines: a row past
/// that (`i64::MAX`, say) is malformed and never shown. Known limit (P3 final review N4):
/// both bounds read the owner's wall clock against ours, so every hold of a machine whose
/// clock runs more than ten minutes ahead is hidden here (and one running behind shows its
/// holds that much shorter); the owner still lists and releases its own.
fn hold_live(until_ms: Option<i64>, now_ms: i64) -> bool {
    const SKEW_MS: i64 = 10 * 60 * 1000;
    let ttl = agent::communication::network::HOLD_TTL.as_millis() as i64;
    until_ms.is_some_and(|u| u > now_ms && u <= now_ms.saturating_add(ttl + SKEW_MS))
}

/// 191 (P3 review N3): `session` is where a message to `export` runs: a chat export's chat
/// itself, or a contact's inbox of that agent export (`inbox`, its `contact_inbox` marker
/// here, names the export's id; any other chat or inbox here is not). A session not here
/// yet names no chat of ours. An unknown export is no target.
fn hold_target(
    staged: &crate::config::Staged,
    export: &str,
    session: SessionId,
    inbox: Option<&str>,
) -> bool {
    if let Some(x) = staged.exports.get(export) {
        return x.session == session;
    }
    staged.agents.contains_key(export)
        && inbox.is_none_or(|m| {
            m.rsplit_once('|')
                .is_some_and(|(_, id)| staged.export_ids.get(export).is_some_and(|x| x == id))
        })
}

/// 191 (D4): a hold row's cut (as the agent's `preview_of`): control characters dropped,
/// at most 200 bytes, on a character boundary.
fn clean_preview(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars().filter(|c| !c.is_control()) {
        if out.len() + c.len_utf8() > 200 {
            break;
        }
        out.push(c);
    }
    out
}

/// 191 (D4): another machine's `contact_holds` row is data. Cleaned and cut on read: control
/// characters dropped, `machine` to 64 characters (as `held_line`), `preview` to 200 bytes.
/// Dropped unless [`hold_live`] (an offline owner's row lives until its end), when a field
/// is missing or malformed, or unless `admits(contact, export, session, inbox)`: a contact
/// this machine admits (not blocked, quarantined or unknown), an export it knows, and the
/// session that export's message runs in (P3 review N3: a row never plants a strip in
/// another chat).
fn held_row(
    r: agent::store::contacts::HoldRow,
    now_ms: i64,
    admits: impl Fn(&str, &str, SessionId, Option<&str>) -> bool,
) -> Option<FirstWaiting> {
    if !hold_live(r.until_ms, now_ms) {
        return None;
    }
    let (contact, export) = (r.contact?, r.export?);
    let session: SessionId = r.session?.parse().ok()?;
    if !admits(&contact, &export, session, r.inbox.as_deref()) {
        return None;
    }
    Some(FirstWaiting {
        contact,
        machine: r
            .machine
            .unwrap_or_default()
            .chars()
            .filter(|c| !c.is_control())
            .take(64)
            .collect(),
        export,
        session,
        since_ms: u64::try_from(r.since_ms?).ok()?,
        waiting: u32::try_from(r.waiting?).ok().filter(|n| *n > 0)?,
        preview: clean_preview(r.preview.as_deref().unwrap_or_default()),
        queued: false,
        position: 0,
        elsewhere: true,
    })
}

/// `ask` → `Vec<FirstWaiting>`: `contact_first_waiting`.
#[derive(Clone, Copy, Debug)]
pub struct ContactsWaiting;

impl Message<ContactsWaiting> for A2aActor {
    type Reply = DelegatedReply<Vec<FirstWaiting>>;

    async fn handle(
        &mut self,
        _: ContactsWaiting,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let c = self.ctx.clone();
        off_mailbox(ctx, async move { c.first_waiting().await })
    }
}

/// 183 W-27: the `contact_first` Attention items, one per (contact, export) with a task
/// waiting ([`crate::Ctx::first_waiting`]). 191 (D1 note 2): and one per raised inflation
/// alert (`contact_roster:<record>`; an alert, nothing is denied).
pub(crate) struct FirstContactItems(pub std::sync::Weak<crate::Ctx>);

#[async_trait::async_trait]
impl roster::attention::Source for FirstContactItems {
    async fn items(&self) -> Vec<roster::attention::AttentionItem> {
        let Some(c) = self.0.upgrade() else {
            return Vec::new();
        };
        let alerts = c.rosters.lock().alerts.clone();
        let mut items: Vec<roster::attention::AttentionItem> = alerts
            .into_iter()
            .map(|(record, (alias, text, at))| roster::attention::AttentionItem {
                id: format!("contact_roster:{record}"),
                kind: roster::attention::AttentionKind::Node,
                title: text,
                detail: Some(format!(
                    "remove that machine from your mesh, then ask {alias} to remove you and pair again"
                )),
                at_ms: at,
                task: None,
                agent: None,
                session: None,
                approval: None,
                decision: None,
                packet: None,
                ask: None,
                turn: None,
                waiting_since_ms: None,
                summary: None,
                next_steps: Vec::new(),
                context: None,
                priority: None,
                rank: 4,
                alert: None,
            })
            .collect();
        items.extend(c.first_waiting().await.into_iter().map(|w| {
            let who = if w.machine.is_empty() {
                w.contact.clone()
            } else {
                format!("{} ({})", w.contact, w.machine)
            };
            roster::attention::AttentionItem {
                id: format!("contact_first:{}:{}", w.contact, w.export),
                kind: roster::attention::AttentionKind::ContactFirst,
                title: format!(
                    "{who} wants to reach {} — {}",
                    w.export,
                    if w.elsewhere {
                        "Always · Block"
                    } else {
                        "Allow once · Always · Block"
                    }
                ),
                detail: Some(format!(
                    "{} waiting · a contact outside your mesh",
                    w.waiting
                )),
                at_ms: w.since_ms,
                task: None,
                agent: None,
                session: Some(w.session.to_string()),
                approval: None,
                decision: None,
                packet: None,
                ask: None,
                turn: None,
                waiting_since_ms: Some(w.since_ms),
                summary: None,
                next_steps: Vec::new(),
                context: None,
                priority: None,
                rank: 5,
                alert: None,
            }
        }));
        items
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire shapes the UI and an older peers.yaml depend on.
    #[test]
    fn contact_shapes_keep_their_wire_names() {
        let chat = Export {
            id: None,
            name: "ops-chat".into(),
            target: ExportTarget::Chat {
                session: SessionId::new(),
            },
            description: String::new(),
            paused: Some("chat archived".into()),
        };
        let v = serde_json::to_value(&chat).unwrap();
        assert_eq!(v["target"]["kind"], "chat");
        assert_eq!(serde_json::from_value::<Export>(v).unwrap(), chat);
        let old: Export = serde_json::from_value(serde_json::json!({
            "name": "research", "target": {"kind": "agent", "agent": "research@xeus"}, "description": ""
        }))
        .unwrap();
        assert_eq!(old.paused, None);

        let t = serde_json::to_value(ContactTarget::CardUrl {
            url: "https://x".into(),
        })
        .unwrap();
        assert_eq!(t["kind"], "card_url");
        let ev = serde_json::to_value(ContactEvent::Nearby(Vec::new())).unwrap();
        assert_eq!(ev["type"], "contact_nearby");

        let line = serde_json::to_string(&PairLine {
            pair: PairFrame::Accept {
                exports: vec!["research".into()],
            },
        })
        .unwrap();
        assert_eq!(line, r#"{"pair":{"accept":{"exports":["research"]}}}"#);
        assert!(matches!(
            serde_json::from_str::<PairLine>(r#"{"pair":{"revoke":{}}}"#)
                .unwrap()
                .pair,
            PairFrame::Revoke {}
        ));
    }

    /// RFC 8785 §3.2.3's sorting example: names ordered by UTF-16 code units (the emoji's
    /// surrogate pair sorts before U+FB33), control characters escaped, the rest raw.
    #[test]
    fn jcs_sorts_by_utf16_and_escapes_as_ecmascript() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"\u20ac":"Euro Sign","\r":"Carriage Return","\ufb33":"Hebrew Letter Dalet With Dagesh","1":"One","\ud83d\ude00":"Emoji: Grinning Face","\u0080":"Control","\u00f6":"Latin Small Letter O With Diaeresis","n":[1,true,null,{"b":2,"a":"x\ty"}]}"#,
        )
        .unwrap();
        assert_eq!(
            jcs(&v),
            "{\"\\r\":\"Carriage Return\",\"1\":\"One\",\"n\":[1,true,null,{\"a\":\"x\\ty\",\"b\":2}],\"\u{80}\":\"Control\",\"\u{f6}\":\"Latin Small Letter O With Diaeresis\",\"\u{20ac}\":\"Euro Sign\",\"\u{1f600}\":\"Emoji: Grinning Face\",\"\u{fb33}\":\"Hebrew Letter Dalet With Dagesh\"}"
        );
    }

    /// RFC 8032 test key 1 signs a fixed card: the JWS is a vector (Ed25519 is
    /// deterministic), it verifies against that key's endpoint id only, and any edit to the
    /// signed card, its interface or its header is refused.
    #[test]
    fn card_jws_vector_verifies_and_refuses_tampering() {
        let secret = iroh::SecretKey::from_bytes(&[
            0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec,
            0x2c, 0xc4, 0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03,
            0x1c, 0xae, 0x7f, 0x60,
        ]);
        let eid = secret.public().to_string();
        let facts = CardFacts {
            name: "Sky".into(),
            description: "Sky's harness".into(),
            machine: "xeus".into(),
            os_name: "Arch Linux".into(),
            endpoint: eid.clone(),
            public_url: None,
            skills: vec![],
        };
        let mut unsigned = card(&facts);
        unsigned.version = "1.0.0".into();
        let sign = |m: &[u8]| secret.sign(m).to_bytes().to_vec();
        let signed = sign_card(unsigned, &eid, &sign).unwrap();
        let sig = &signed.signatures.as_ref().unwrap()[0];
        assert_eq!(
            String::from_utf8(
                data_encoding::BASE64URL_NOPAD
                    .decode(sig.protected.as_bytes())
                    .unwrap()
            )
            .unwrap(),
            format!(r#"{{"alg":"EdDSA","kid":"{eid}","typ":"JOSE"}}"#)
        );
        // 186, 199: the card carries the full-collaboration and the files extensions too.
        assert_eq!(
            sig.signature,
            "282xD1T3JqBI8nAySSmF3Ix1umLrj2JirtmqKNmGPVbrDIox8uiAvkJsM1Yj2CwQtdZ0lOyCdssV1DQoXeZaBA"
        );
        verify_card(&signed, &eid).unwrap();
        assert_eq!(card_machine(&signed), ("xeus".into(), "Arch Linux".into()));
        // 199 (D1): it takes files, each up to 5 GB.
        assert!(card_files(&signed));
        let files = signed.capabilities.extensions.iter().flatten();
        let files = files.filter(|e| e.uri == EXT_FILES).collect::<Vec<_>>();
        assert_eq!(files[0].required, Some(false));
        assert_eq!(
            files[0].params.as_ref().unwrap()["maxBytes"],
            serde_json::json!(5_000_000_000u64)
        );

        // Over the wire and back: the same bytes verify.
        let back: AgentCard =
            serde_json::from_str(&serde_json::to_string(&signed).unwrap()).unwrap();
        verify_card(&back, &eid).unwrap();

        let other = iroh::SecretKey::from_bytes(&[3; 32]).public().to_string();
        assert!(
            verify_card(&signed, &other)
                .unwrap_err()
                .contains("another endpoint")
        );
        let mut renamed = signed.clone();
        renamed.name = "Mallory".into();
        assert!(
            verify_card(&renamed, &eid)
                .unwrap_err()
                .contains("does not verify")
        );
        let mut moved = signed.clone();
        moved.supported_interfaces[0].url = format!("iroh:{other}");
        assert!(
            verify_card(&moved, &other)
                .unwrap_err()
                .contains("by its endpoint")
        );
        let mut unsigned = signed.clone();
        unsigned.signatures = None;
        assert!(
            verify_card(&unsigned, &eid)
                .unwrap_err()
                .contains("not signed")
        );
        // A card another key signed for this endpoint is refused too.
        let forged = sign_card(signed, &eid, &|m: &[u8]| {
            iroh::SecretKey::from_bytes(&[3; 32])
                .sign(m)
                .to_bytes()
                .to_vec()
        })
        .unwrap();
        assert!(
            verify_card(&forged, &eid)
                .unwrap_err()
                .contains("does not verify")
        );
    }

    /// 191 (D4): another machine's hold row is cleaned and cut on read (no control
    /// characters; `machine` to 64 characters, `preview` to 200 bytes on a character
    /// boundary); a row past `until_ms` goes, an offline owner's row before it stays, one
    /// that ends further out than any real hold does (P3 review M1) goes, and a row for a
    /// contact or export this machine does not admit, or with a malformed field, is dropped.
    #[test]
    fn another_machines_hold_rows_are_cleaned_cut_and_kept_until_their_end() {
        let session = SessionId::new();
        let row = agent::store::contacts::HoldRow {
            id: "task-1".into(),
            contact: Some("bob".into()),
            machine: Some(format!("33\n3\u{1b}[31m{}", "x".repeat(4096))),
            export: Some("research".into()),
            session: Some(session.to_string()),
            owner: Some("n2-engine".into()),
            since_ms: Some(1_000),
            waiting: Some(2),
            preview: Some(format!("a\u{7}b{}", "é".repeat(300))),
            until_ms: Some(5_000),
            inbox: None,
        };
        let admits = |c: &str, e: &str, s: SessionId, _: Option<&str>| {
            c == "bob" && e == "research" && s == session
        };
        let w = held_row(row.clone(), 4_999, admits).expect("before its end");
        assert!(!w.machine.chars().any(char::is_control), "{:?}", w.machine);
        assert_eq!(w.machine.chars().count(), 64);
        assert!(w.machine.starts_with("333[31mxxx"), "{}", w.machine);
        assert!(!w.preview.chars().any(char::is_control));
        assert!(w.preview.len() <= 200 && w.preview.starts_with("abé"));
        assert_eq!(w.preview.len(), 2 + 99 * 2, "cut on a character boundary");
        assert_eq!((w.session, w.since_ms, w.waiting), (session, 1_000, 2));
        assert_eq!((w.queued, w.position, w.elsewhere), (false, 0, true));

        assert!(
            held_row(row.clone(), 5_000, admits).is_none(),
            "past until_ms"
        );
        let ttl = agent::communication::network::HOLD_TTL.as_millis() as i64;
        let far = |until_ms| agent::store::contacts::HoldRow {
            until_ms: Some(until_ms),
            ..row.clone()
        };
        assert!(
            held_row(far(ttl + 5_000), 5_000, admits).is_some(),
            "a day out"
        );
        assert!(
            held_row(far(i64::MAX), 5_000, admits).is_none(),
            "no hold lasts forever"
        );
        assert!(held_row(far(2 * ttl), 5_000, admits).is_none());
        let no_end = agent::store::contacts::HoldRow {
            until_ms: None,
            ..row.clone()
        };
        assert!(held_row(no_end, 0, admits).is_none(), "no end: dropped");
        assert!(
            held_row(row.clone(), 0, |_, _, _, _| false).is_none(),
            "a contact, export or session not admitted here"
        );
        let bad = agent::store::contacts::HoldRow {
            session: Some("not a session".into()),
            ..row.clone()
        };
        assert!(held_row(bad, 0, admits).is_none());
        let none_waiting = agent::store::contacts::HoldRow {
            waiting: Some(0),
            ..row
        };
        assert!(held_row(none_waiting, 0, admits).is_none());
    }

    /// 191 (P3 review N3): a hold row names where its export's message runs, or it plants
    /// nothing: a chat export's own chat only; an agent export's inbox here (its marker names
    /// that export's id), or a session not here yet; never another chat, another export's
    /// inbox, or an export this machine does not know.
    #[test]
    fn a_hold_row_only_points_at_its_exports_session() {
        let (chat, other) = (SessionId::new(), SessionId::new());
        let mut staged = crate::config::Staged::default();
        staged.exports.insert(
            "notes".into(),
            crate::config::Export {
                name: "notes".into(),
                session: chat,
                title: String::new(),
                description: String::new(),
                peer: None,
            },
        );
        staged.agents.insert(
            "research".into(),
            crate::config::AgentExport {
                name: "research".into(),
                agent: "theuth".into(),
                title: String::new(),
                description: String::new(),
                home: agent::communication::network::InboxHome::Here,
            },
        );
        staged.export_ids.insert("notes".into(), "x-notes".into());
        staged
            .export_ids
            .insert("research".into(), "x-research".into());
        assert!(hold_target(&staged, "notes", chat, Some("")));
        assert!(
            !hold_target(&staged, "notes", other, Some("")),
            "another chat"
        );
        assert!(!hold_target(&staged, "notes", other, None));
        assert!(hold_target(
            &staged,
            "research",
            other,
            Some("peer:b|x-research")
        ));
        assert!(
            hold_target(&staged, "research", other, None),
            "not here yet"
        );
        assert!(
            !hold_target(&staged, "research", chat, Some("")),
            "a plain chat"
        );
        assert!(
            !hold_target(&staged, "research", other, Some("peer:b|x-notes")),
            "another export's inbox"
        );
        assert!(!hold_target(&staged, "gone", chat, None), "unknown export");
    }

    /// 191 (P3 review M2): a hold is released only on `allow` for an export the contact is
    /// still granted, failed on `block`, and otherwise left waiting: `allow` on an export no
    /// longer granted, `ask`, a blocked contact, a quarantined or unknown one (not in the
    /// staged contacts).
    #[test]
    fn a_hold_is_released_only_on_a_grant_it_still_holds() {
        let mut staged = crate::config::Staged::default();
        staged.peers.insert(
            "bob".into(),
            crate::config::Peer {
                name: "bob".into(),
                card: None,
                outbound: None,
                inbound: None,
                sessions: Vec::new(),
                exports: vec!["research".into()],
                allow_private: false,
                eid: None,
                record: None,
                full_access: false,
            },
        );
        staged.contacts.insert(
            "bob".into(),
            crate::config::ContactFacts {
                id: "r1".into(),
                eid: "e1".into(),
                eids: vec!["e1".into()],
                display: String::new(),
                machine: String::new(),
                first_contact: BTreeMap::from([
                    ("research".to_owned(), FirstContact::Allow),
                    ("notes".to_owned(), FirstContact::Allow),
                    ("ops".to_owned(), FirstContact::Block),
                    ("ask".to_owned(), FirstContact::Ask),
                ]),
                blocked: false,
                paired_ms: 0,
                addrs: Vec::new(),
            },
        );
        assert_eq!(release_of(&staged, "bob", "research"), Some(true));
        assert_eq!(
            release_of(&staged, "bob", "notes"),
            None,
            "allow on an export not granted"
        );
        assert_eq!(release_of(&staged, "bob", "ops"), Some(false), "block");
        assert_eq!(release_of(&staged, "bob", "ask"), None);
        assert_eq!(release_of(&staged, "bob", "none"), None, "no answer: ask");
        assert_eq!(
            release_of(&staged, "kim", "research"),
            None,
            "quarantined or unknown"
        );
        staged.contacts.get_mut("bob").unwrap().blocked = true;
        assert_eq!(release_of(&staged, "bob", "research"), None, "blocked");
        assert_eq!(release_of(&staged, "bob", "ops"), None, "blocked");
    }
}
