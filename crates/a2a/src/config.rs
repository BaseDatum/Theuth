//! `a2a:` config section and `<data_dir>/a2a/peers.yaml` (design doc 50 §2.1,
//! §6.1). Everything here is validation into an immutable [`Staged`] set;
//! nothing is installed until every check passed (atomic reload, §6.1).
//! Session existence is the agent's answer and happens in `lib.rs` after this
//! stage; secrets arrive through a resolver closure so the rules are testable
//! without a vault.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::net::{IpAddr, SocketAddr};

use agent::communication::ALIAS_MAX;
pub use agent::communication::ShareMode;
use runtime::SessionId;
use serde::{Deserialize, Serialize};
use url::{Host, Url};
use zeroize::Zeroizing;

/// Owner-managed peer/export file, relative to the data dir.
pub const PEERS_FILE: &str = "a2a/peers.yaml";
/// Default loopback listener.
pub const DEFAULT_LISTEN: &str = "127.0.0.1:7780";
/// Spec well-known suffix; a configured card URL may include it.
pub const CARD_SUFFIX: &str = "/.well-known/agent-card.json";

/// `a2a:` section (§2.1). Missing keys are false/loopback/none.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct A2aConfig {
    /// Permit the network client/server machinery at all.
    pub enabled: bool,
    /// SDK HTTP listener; must be a loopback socket address.
    pub listen: String,
    /// HTTPS base the reverse proxy publishes; required for production exports.
    pub public_url: Option<String>,
    /// 183 §9.3 (W-20): DNS-SD service types browsed while an Add contact window is open;
    /// the first is advertised. Read, not acted on yet.
    // 183 P3: E — advertise/browse these during the contact window.
    pub mdns_types: Vec<String>,
    /// 183 §9.3: how long an Add contact window (and its invite ticket) stays open.
    pub contact_window_secs: u64,
    /// 183 §9.3: the cap a new contact gets (`read_only` | `mutating`).
    pub contact_cap: tools::Risk,
}

/// 183 §9.3: `a2a.mdns_types` default: draft-zhao first, then the two rival types.
pub const MDNS_TYPES: [&str; 3] = ["_a2a._tcp", "_agent._tcp", "_lad-a2a._tcp"];

impl Default for A2aConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            listen: DEFAULT_LISTEN.to_owned(),
            public_url: None,
            mdns_types: MDNS_TYPES.map(str::to_owned).to_vec(),
            contact_window_secs: 600,
            contact_cap: tools::Risk::ReadOnly,
        }
    }
}

/// Validated listener settings (restart to change, §2.1).
#[derive(Clone, Debug)]
pub struct Listener {
    /// Loopback bind address.
    pub addr: SocketAddr,
    /// Advertised base: `public_url` (HTTPS) or `http://<addr>` for loopback development.
    pub base: Url,
    /// `public_url` was set: the deployment claims to be reachable through HTTPS.
    pub production: bool,
}

impl A2aConfig {
    /// Loopback-only listener and the advertised base URL (§6.1).
    pub fn listener(&self) -> Result<Listener, String> {
        let addr: SocketAddr = self
            .listen
            .parse()
            .map_err(|e| format!("a2a.listen {:?}: {e}", self.listen))?;
        if !addr.ip().is_loopback() {
            return Err(format!(
                "a2a.listen {addr} is not loopback; production exposure goes through an HTTPS reverse proxy and a2a.public_url"
            ));
        }
        match &self.public_url {
            None => Ok(Listener {
                base: Url::parse(&format!("http://{addr}")).map_err(|e| e.to_string())?,
                addr,
                production: false,
            }),
            Some(raw) => {
                let mut url =
                    Url::parse(raw).map_err(|e| format!("a2a.public_url {raw:?}: {e}"))?;
                if url.scheme() != "https" {
                    return Err(format!("a2a.public_url {raw:?} must be https"));
                }
                check_bare(&url).map_err(|e| format!("a2a.public_url {raw:?}: {e}"))?;
                trim_slash(&mut url);
                Ok(Listener {
                    addr,
                    base: url,
                    production: true,
                })
            }
        }
    }
}

/// `peers.yaml` as written by the owner (§6.1); 183 §7.8: also by the contact RPCs.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PeersFile {
    /// Remote agents by owner alias.
    pub peers: BTreeMap<String, PeerEntry>,
    /// Local sessions published by export name.
    pub exports: BTreeMap<String, ExportEntry>,
}

/// 183 §9.4: a contact's first-contact answer for one export. Absent = `ask`.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum FirstContact {
    /// Raise the Attention item; the task waits in `TASK_STATE_AUTH_REQUIRED`.
    #[default]
    Ask,
    /// Let every task through (Always).
    Allow,
    /// Refuse (Block).
    Block,
}

/// One configured remote agent. 183 §9.4: a contact is an entry with `endpoint` set; the
/// new fields are skipped when unset so a file of HTTP peers still loads on an older build.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PeerEntry {
    /// Agent Card URL or its base; HTTPS unless loopback.
    pub card_url: Option<String>,
    /// Vault name of the bearer we present.
    pub outbound_secret: Option<String>,
    /// Vault name of the bearer the peer presents.
    pub inbound_secret: Option<String>,
    /// Local roots whose enabled trees may send to this peer.
    pub sessions: Vec<SessionId>,
    /// Exports this peer may call.
    pub exports: Vec<String>,
    /// Owner opt-in for a loopback/private-network card origin (§9).
    pub allow_private: bool,
    /// 183 §7.3: the contact's iroh endpoint id, pinned by the ceremony.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// 183 §9.4: display name from the contact's card.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 183 (after C3): the contact's machine, from its card's `EXT_MACHINE` extension.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    /// 183 §7.6: the contact's risk cap; `None` = `a2a.contact_cap`. 190 (D9): read, never
    /// written; it binds only a W-40 turn.
    #[serde(default, skip_serializing)]
    pub cap: Option<tools::Risk>,
    /// 183 §7.6 (W-27): export → first-contact answer; a missing export is `ask`.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub first_contact: BTreeMap<String, FirstContact>,
    /// 186 (D1, D2): export → share mode. 190 (D9): read and dropped, never written (every
    /// share is full; a 186 build refuses unknown keys, so the field stays readable).
    #[serde(default, skip_serializing)]
    pub modes: BTreeMap<String, ShareMode>,
    /// 183 §9.4: refused at the `theuth/a2a/1` gate.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub blocked: bool,
    /// 183 §9.4: when the ceremony finished (ms since epoch).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paired_ms: Option<u64>,
    /// 183 §7.2: the contact's direct addresses from its last live connection (engine-local,
    /// rewritten when they change), dialled beside discovery and the relay.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub addrs: Vec<String>,
}

impl PeerEntry {
    /// 183 §9.4: any contact field is set (the entry came from a ceremony).
    pub fn is_contact(&self) -> bool {
        self.endpoint.is_some()
            || self.name.is_some()
            || self.machine.is_some()
            || self.cap.is_some()
            || !self.first_contact.is_empty()
            || self.blocked
            || self.paired_ms.is_some()
    }
}

/// One published local session; 183 §9.4: or one local agent. Exactly one of `session`,
/// `agent` is set.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportEntry {
    /// Exact native session (not its tree): a chat export (D9).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionId>,
    /// 183 §7.6: an agent export (`<agent>@<engine>` desk root, or `theuth`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Card name.
    pub name: String,
    /// Card description; owner-written functional capability, no tool inventory.
    #[serde(default)]
    pub description: String,
    /// 183 Z3-R2: the export's allowlist for a contact's capped turn. 190 (D9): read and
    /// dropped, never written.
    #[serde(default, skip_serializing)]
    pub tools: Vec<String>,
}

/// 183 Z3-R2: what a contact's capped turn may use besides the reply: the public web only.
// W-40 only; removed by design 191 (contacts are mesh-wide)
pub fn default_contact_tools() -> Vec<String> {
    vec!["search_web".into(), "read_url".into()]
}

/// A resolved credential: zeroized on drop, never printed.
#[derive(Clone, PartialEq, Eq)]
pub struct Token(Zeroizing<String>);

impl Token {
    /// The bearer value, for the auth map and the client's header only.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Tests only: production tokens come from the vault through [`stage`].
    #[cfg(test)]
    pub(crate) fn for_test(value: &str) -> Self {
        Self(Zeroizing::new(value.to_owned()))
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(<redacted>)")
    }
}

/// A validated peer. Tokens are the resolved vault values, kept only here and
/// in the auth map / client; never in status, logs or messages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Peer {
    /// Owner alias (`peer:<name>`).
    pub name: String,
    /// Card base URL (without the well-known suffix) when outbound.
    pub card: Option<Url>,
    /// Bearer we present, when outbound.
    pub outbound: Option<Token>,
    /// Bearer the peer presents, when inbound.
    pub inbound: Option<Token>,
    /// Permitted local outbound roots.
    pub sessions: Vec<SessionId>,
    /// Granted exports.
    pub exports: Vec<String>,
    /// Private/loopback card origin permitted.
    pub allow_private: bool,
    /// 183 §7.3: a contact's pinned endpoint id (its identity: a re-pin is a new grant). 191:
    /// a record's first anchor (the one dialled); every anchor is in [`ContactFacts::eids`].
    pub eid: Option<String>,
    /// 191 (D2): a contact record's id and its replicated binding id.
    pub record: Option<(String, uuid::Uuid)>,
}

impl Peer {
    /// Stable identity string for the store binding: the card origin+path, or
    /// `inbound` for a peer that never gets called (§5.1); 183 §7.3: `iroh:<eid>` for a
    /// contact; 191: `contact:<record id>` for a contact record (constant for the record).
    pub fn endpoint(&self) -> String {
        if let Some((id, _)) = &self.record {
            return agent::communication::network::contact_record_endpoint(id);
        }
        if let Some(eid) = &self.eid {
            return agent::communication::network::contact_endpoint(eid);
        }
        self.card
            .as_ref()
            .map_or_else(|| "inbound".to_owned(), ToString::to_string)
    }
}

/// 183 §9.4: what a contact entry adds beyond its [`Peer`] grant. Kept apart from [`Peer`] so
/// a first-contact answer or a cap change is not a new grant epoch (open tasks survive it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContactFacts {
    /// 191 (D2): its record (a merge group's root); `""` for a `peers.yaml` entry (an import).
    pub id: String,
    /// The pinned endpoint id (191: the endpoint dialled first: a pin still admitted, else the
    /// first machine of its roster).
    pub eid: String,
    /// 191 (D1): every endpoint the group admits, `eid` first: its records' current rosters
    /// (pins while none is based).
    pub eids: Vec<String>,
    /// Display name from its card.
    pub display: String,
    /// Its machine, from its card.
    pub machine: String,
    /// `read_only` | `mutating`.
    pub cap: tools::Risk,
    /// Export → answer; absent = `ask`.
    pub first_contact: BTreeMap<String, FirstContact>,
    /// Refused at the gate; a blocked contact is in no route.
    pub blocked: bool,
    /// When the ceremony finished.
    pub paired_ms: u64,
    /// Last known direct addresses (unparsable ones dropped).
    pub addrs: Vec<std::net::SocketAddr>,
}

/// 191 (D2): a contact record the reconcile quarantined: listed with its problem, never
/// admitted, its endpoints denied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Quarantined {
    /// The record id.
    pub id: String,
    /// Its alias as stored (may be the malformed value).
    pub alias: String,
    /// Its first endpoint (`""` when none).
    pub eid: String,
    /// Its machine.
    pub machine: String,
    /// When it was paired.
    pub paired_ms: u64,
    /// Why it is quarantined.
    pub problem: String,
    /// A quarantine of replication lag or of a convergence in flight (a merge, a rename):
    /// the binding to keep live (unrouted, its inboxes capped) so it is not revoked for good.
    pub hold: Option<uuid::Uuid>,
}

/// 183 §7.6: a validated agent export (contacts only): each (contact, export) gets its own
/// inbox session running `agent`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentExport {
    /// Path segment (`/agents/<name>`).
    pub name: String,
    /// `<agent>@<engine>` desk root, or `theuth`.
    pub agent: String,
    /// Card name.
    pub title: String,
    /// Card description.
    pub description: String,
}

/// A validated export.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Export {
    /// Path segment (`/agents/<name>`).
    pub name: String,
    /// The one native session it publishes.
    pub session: SessionId,
    /// Card name.
    pub title: String,
    /// Card description.
    pub description: String,
    /// The single peer granted this export, if any (§6.3: one principal per exported session).
    pub peer: Option<String>,
}

/// Everything validation produced; installed atomically or not at all.
#[derive(Clone, Debug, Default)]
pub struct Staged {
    /// By alias. 183: contacts too, except blocked ones.
    pub peers: BTreeMap<String, Peer>,
    /// By export name: session (chat) exports.
    pub exports: BTreeMap<String, Export>,
    /// 183 §7.6: agent exports by name (names are unique across both maps).
    pub agents: BTreeMap<String, AgentExport>,
    /// 183 §9.4: every contact entry by alias, blocked ones included.
    pub contacts: BTreeMap<String, ContactFacts>,
    /// 191 (D2): endpoints the gate refuses first, whatever else matches them: those of
    /// quarantined and blocked records.
    pub deny: std::collections::BTreeSet<String>,
    /// 191 (D2): endpoints of removed records, never admitted as a contact until a newer
    /// ceremony pins them in a live record (they may pair again).
    pub removed: std::collections::BTreeSet<String>,
    /// 191 (D3): the last direct addresses this machine saw for each endpoint (its local
    /// `contact_addrs`): dial hints for every machine of every contact, verified or not yet.
    pub addrs: BTreeMap<String, Vec<std::net::SocketAddr>>,
    /// 191 (D2): records quarantined by the reconcile, listed with their problem.
    pub quarantined: Vec<Quarantined>,
    /// 191 (D2): export name → its replicated id (chat and agent exports).
    pub export_ids: BTreeMap<String, String>,
}

impl Staged {
    /// Whether `alias` is authorized identically in both sets: same
    /// credentials, roots and granted exports, and every granted export still
    /// publishes the same session. Anything else is a new authorization epoch
    /// (§6.1): what an already-admitted caller holds must not carry over.
    pub fn same_grant(&self, other: &Staged, alias: &str) -> bool {
        let (Some(a), Some(b)) = (self.peers.get(alias), other.peers.get(alias)) else {
            return false;
        };
        a == b
            && a.exports.iter().all(|e| {
                self.exports.get(e) == other.exports.get(e)
                    && self.agents.get(e) == other.agents.get(e)
            })
    }

    /// 183 F4: the exports `alias` holds in both sets with the same authority — the same
    /// session (chat) or agent — under the same contact cap: a first message held for one of
    /// them may wait on into the next epoch; any other was admitted under authority that is
    /// gone. A title or description edit changes no authority.
    pub fn unchanged_exports(&self, next: &Staged, alias: &str) -> Vec<String> {
        let (Some(a), Some(b)) = (self.peers.get(alias), next.peers.get(alias)) else {
            return Vec::new();
        };
        let cap = |s: &Staged| s.contacts.get(alias).map(|c| c.cap);
        if cap(self) != cap(next) {
            return Vec::new();
        }
        b.exports
            .iter()
            .filter(|e| {
                let chat = match (self.exports.get(*e), next.exports.get(*e)) {
                    (Some(x), Some(y)) => x.session == y.session,
                    (x, y) => x.is_none() && y.is_none(),
                };
                let agent = match (self.agents.get(*e), next.agents.get(*e)) {
                    (Some(x), Some(y)) => x.agent == y.agent,
                    (x, y) => x.is_none() && y.is_none(),
                };
                a.exports.contains(e) && chat && agent
            })
            .cloned()
            .collect()
    }

    /// 183 §7.2: the alias of the contact (blocked or not) pinned to `eid`; 191: any anchor
    /// of its group.
    pub fn contact_by_eid(&self, eid: &str) -> Option<&str> {
        self.contacts
            .iter()
            .find(|(_, c)| c.eid == eid || c.eids.iter().any(|e| e == eid))
            .map(|(alias, _)| alias.as_str())
    }
}

/// Resolves a vault name to its current value; `None` = unknown.
pub type Resolve<'a> = &'a dyn Fn(&str) -> Option<Zeroizing<String>>;

fn check_name(kind: &str, name: &str, errors: &mut Vec<String>) {
    let ok = !name.is_empty()
        && name.len() <= ALIAS_MAX
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
    if !ok {
        errors.push(format!(
            "{kind} name {name:?}: 1..={ALIAS_MAX} chars of [a-z0-9_-]"
        ));
    }
}

/// 183 §7.8: a peer or export name the contact RPCs are about to write.
pub(crate) fn check_alias(name: &str) -> Result<(), String> {
    let mut errors = Vec::new();
    check_name("name", name, &mut errors);
    errors.pop().map_or(Ok(()), Err)
}

/// 183 §7.4: a peer alias from a card name: lowercase `[a-z0-9_-]`, cut to fit, `contact`
/// when nothing is left.
pub(crate) fn alias_of(display: &str) -> String {
    let mut out = String::new();
    for c in display.trim().chars().flat_map(char::to_lowercase) {
        let c = if c.is_ascii_alphanumeric() || c == '_' {
            c
        } else {
            '-'
        };
        if !(c == '-' && (out.is_empty() || out.ends_with('-'))) {
            out.push(c);
        }
    }
    let mut out: String = out
        .trim_end_matches('-')
        .chars()
        .take(ALIAS_MAX - 3)
        .collect();
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        out.push_str("contact");
    }
    out
}

/// No credentials, query or fragment in an origin/base URL.
fn check_bare(url: &Url) -> Result<(), String> {
    if !url.username().is_empty() || url.password().is_some() {
        return Err("credentials in URL are not allowed; use a vault secret".into());
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err("query/fragment not allowed".into());
    }
    if url.host().is_none() {
        return Err("missing host".into());
    }
    Ok(())
}

fn trim_slash(url: &mut Url) {
    let trimmed = url.path().trim_end_matches('/').to_owned();
    url.set_path(&trimmed);
}

/// Card URL → validated base (§9: HTTPS, or HTTP to a loopback host only).
pub fn card_base(raw: &str) -> Result<Url, String> {
    let mut url = Url::parse(raw).map_err(|e| e.to_string())?;
    check_bare(&url)?;
    let loopback = match url.host() {
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        Some(Host::Domain(d)) => d == "localhost",
        None => false,
    };
    match url.scheme() {
        "https" => {}
        "http" if loopback => {}
        s => {
            return Err(format!(
                "scheme {s:?}: https required (http only for loopback development)"
            ));
        }
    }
    if let Some(base) = url.path().strip_suffix(CARD_SUFFIX) {
        let base = base.to_owned();
        url.set_path(&base);
    }
    trim_slash(&mut url);
    Ok(url)
}

/// Whether `ip` is an acceptable egress destination (§9). Link-local (cloud
/// metadata), multicast, unspecified and broadcast never are; loopback and
/// private ranges only with the peer's explicit `allow_private`.
pub fn egress_allowed(ip: IpAddr, allow_private: bool) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            if v4.is_link_local()
                || v4.is_multicast()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
            {
                return false;
            }
            let o = v4.octets();
            // 0/8 and 100.64/10 (CGNAT) count as private for this purpose.
            let private = v4.is_private()
                || v4.is_loopback()
                || o[0] == 0
                || (o[0] == 100 && (64..128).contains(&o[1]));
            allow_private || !private
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return egress_allowed(IpAddr::V4(v4), allow_private);
            }
            if v6.is_multicast() || v6.is_unspecified() || v6.is_unicast_link_local() {
                return false;
            }
            let private = v6.is_loopback() || v6.is_unique_local();
            allow_private || !private
        }
    }
}

/// Grant chat export `export` to peer `name`: it must exist, and one principal per exported
/// session (§6.3), whether an HTTP peer or a contact holds it.
fn grant_chat(name: &str, export: &str, staged: &mut Staged, errors: &mut Vec<String>) {
    match staged.exports.get_mut(export) {
        None => errors.push(format!("peer {name}: unknown export {export:?}")),
        Some(x) => match &x.peer {
            Some(other) if other != name => errors.push(format!(
                "export {export} is granted to both {other} and {name}; one principal per exported session"
            )),
            _ => x.peer = Some(name.to_owned()),
        },
    }
}

/// 183 §9.4: one contact entry. A contact is its pinned endpoint id: no card URL, bearer or
/// local roots (every root may reach it, W-26). A blocked contact keeps its facts (the gate
/// refuses its id) and gets no route.
fn stage_contact<'a>(
    name: &'a str,
    p: &'a PeerEntry,
    contact_cap: tools::Risk,
    pins: &mut HashMap<&'a str, &'a str>,
    staged: &mut Staged,
    errors: &mut Vec<String>,
) {
    let Some(eid) = p.endpoint.as_deref().map(str::trim) else {
        errors.push(format!("peer {name}: a contact needs its endpoint"));
        return;
    };
    if eid.parse::<iroh::EndpointId>().is_err() {
        errors.push(format!(
            "peer {name}: endpoint {eid:?} is not an endpoint id"
        ));
    }
    if p.card_url.is_some()
        || p.outbound_secret.is_some()
        || p.inbound_secret.is_some()
        || !p.sessions.is_empty()
        || p.allow_private
    {
        errors.push(format!(
            "peer {name}: a contact takes no card_url, secrets, sessions or allow_private"
        ));
    }
    if let Some(other) = pins.insert(eid, name) {
        errors.push(format!(
            "peers {other} and {name} pin the same endpoint; one contact per endpoint"
        ));
    }
    let cap = p.cap.unwrap_or(contact_cap);
    if cap > tools::Risk::Mutating {
        errors.push(format!("peer {name}: cap {cap:?}: read_only or mutating"));
    }
    if !p.blocked {
        for export in &p.exports {
            if !staged.agents.contains_key(export) {
                grant_chat(name, export, staged, errors);
            }
        }
        staged.peers.insert(
            name.to_owned(),
            Peer {
                name: name.to_owned(),
                card: None,
                outbound: None,
                inbound: None,
                sessions: Vec::new(),
                exports: p.exports.clone(),
                allow_private: false,
                eid: Some(eid.to_owned()),
                record: None,
            },
        );
    }
    staged.contacts.insert(
        name.to_owned(),
        ContactFacts {
            id: String::new(),
            eid: eid.to_owned(),
            eids: vec![eid.to_owned()],
            display: p.name.clone().unwrap_or_else(|| name.to_owned()),
            machine: p.machine.clone().unwrap_or_default(),
            cap: cap.min(tools::Risk::Mutating),
            first_contact: p.first_contact.clone(),
            blocked: p.blocked,
            paired_ms: p.paired_ms.unwrap_or_default(),
            addrs: p.addrs.iter().filter_map(|a| a.parse().ok()).collect(),
        },
    );
}

/// Validate the whole file (§6.1). `console` is the RPC console token, which
/// no peer credential may equal; `contact_cap` is `a2a.contact_cap`, the cap of a contact
/// entry without its own. Returns every problem, not the first.
pub fn stage(
    file: &PeersFile,
    resolve: Resolve<'_>,
    console: Option<&str>,
    contact_cap: tools::Risk,
) -> Result<Staged, Vec<String>> {
    stage_on(Staged::default(), file, resolve, console, contact_cap)
}

/// 191 (D2): [`stage`] onto `base` (what the contact records staged: their exports, which
/// the file's HTTP peers may be granted, and their contacts, whose aliases the file's peers
/// may not take).
pub fn stage_on(
    base: Staged,
    file: &PeersFile,
    resolve: Resolve<'_>,
    console: Option<&str>,
    contact_cap: tools::Risk,
) -> Result<Staged, Vec<String>> {
    let mut errors = Vec::new();
    let mut staged = base;

    let mut export_sessions: HashMap<SessionId, &str> = HashMap::new();
    for (name, e) in &file.exports {
        check_name("export", name, &mut errors);
        if staged.exports.contains_key(name) || staged.agents.contains_key(name) {
            errors.push(format!("export {name}: the name of an export of your mesh"));
        }
        if e.name.trim().is_empty() {
            errors.push(format!("export {name}: empty name"));
        }
        let session = match (e.session, &e.agent) {
            (Some(s), None) => s,
            (None, Some(agent)) => {
                if agent.trim().is_empty() {
                    errors.push(format!("export {name}: empty agent"));
                }
                staged.agents.insert(
                    name.clone(),
                    AgentExport {
                        name: name.clone(),
                        agent: agent.trim().to_owned(),
                        title: e.name.trim().to_owned(),
                        description: e.description.trim().to_owned(),
                    },
                );
                continue;
            }
            _ => {
                errors.push(format!(
                    "export {name}: set exactly one of session and agent"
                ));
                continue;
            }
        };
        if let Some(other) = export_sessions.insert(session, name) {
            errors.push(format!(
                "exports {other} and {name} publish the same session {session}; one export per session"
            ));
        }
        staged.exports.insert(
            name.clone(),
            Export {
                name: name.clone(),
                session,
                title: e.name.trim().to_owned(),
                description: e.description.trim().to_owned(),
                peer: None,
            },
        );
    }

    let secret = |peer: &str, role: &str, name: &str, errors: &mut Vec<String>| -> Option<Token> {
        match resolve(name) {
            None => {
                errors.push(format!(
                    "peer {peer}: {role}_secret {name:?} is not in the vault"
                ));
                None
            }
            Some(v) if v.trim().is_empty() => {
                errors.push(format!("peer {peer}: {role}_secret {name:?} is empty"));
                None
            }
            Some(v) if console.is_some_and(|c| c == v.as_str()) => {
                errors.push(format!(
                    "peer {peer}: {role}_secret {name:?} is the console token; peers get their own random credential"
                ));
                None
            }
            Some(v) => Some(Token(v)),
        }
    };

    // Every resolved credential value, for the cross-peer collision check:
    // a peer whose outbound bearer equals any inbound bearer would be
    // accepted as that other principal (§6.1), whatever the vault names.
    let mut credentials: Vec<(String, Token)> = Vec::new();
    let mut pins: HashMap<&str, &str> = HashMap::new();
    for (name, p) in &file.peers {
        check_name("peer", name, &mut errors);
        if staged.contacts.contains_key(name) {
            errors.push(format!("peer {name}: the name of a contact of your mesh"));
            continue;
        }
        if p.is_contact() {
            stage_contact(name, p, contact_cap, &mut pins, &mut staged, &mut errors);
            continue;
        }
        let outbound_side =
            p.card_url.is_some() || p.outbound_secret.is_some() || !p.sessions.is_empty();
        let inbound_side = p.inbound_secret.is_some() || !p.exports.is_empty();
        if !outbound_side && !inbound_side {
            errors.push(format!(
                "peer {name}: neither outbound (card_url) nor inbound (inbound_secret) configured"
            ));
        }

        let mut card = None;
        let mut outbound = None;
        if outbound_side {
            // Each field is checked on its own so one problem does not hide
            // another (an insecure URL is reported even without a secret).
            match &p.card_url {
                Some(raw) => match card_base(raw) {
                    Ok(u) => card = Some(u),
                    Err(e) => errors.push(format!("peer {name}: card_url {raw:?}: {e}")),
                },
                None => errors.push(format!(
                    "peer {name}: sessions/outbound_secret need a card_url"
                )),
            }
            match &p.outbound_secret {
                Some(s) => {
                    outbound = secret(name, "outbound", s, &mut errors);
                    if let Some(v) = &outbound {
                        credentials
                            .push((format!("peer {name}: outbound_secret {s:?}"), v.clone()));
                    }
                }
                None => errors.push(format!("peer {name}: card_url needs an outbound_secret")),
            }
            if p.sessions.is_empty() {
                errors.push(format!(
                    "peer {name}: outbound peer grants no local sessions"
                ));
            }
        }

        let mut inbound = None;
        if inbound_side {
            match &p.inbound_secret {
                Some(s) => {
                    inbound = secret(name, "inbound", s, &mut errors);
                    if let Some(v) = &inbound {
                        credentials.push((format!("peer {name}: inbound_secret {s:?}"), v.clone()));
                    }
                }
                None => errors.push(format!("peer {name}: exports need an inbound_secret")),
            }
            if p.exports.is_empty() {
                errors.push(format!("peer {name}: inbound peer is granted no exports"));
            }
            for export in &p.exports {
                if staged.agents.contains_key(export) {
                    errors.push(format!(
                        "peer {name}: export {export} is an agent export; only contacts reach agents"
                    ));
                    continue;
                }
                grant_chat(name, export, &mut staged, &mut errors);
            }
        }

        staged.peers.insert(
            name.clone(),
            Peer {
                name: name.clone(),
                card,
                outbound,
                inbound,
                sessions: p.sessions.clone(),
                exports: p.exports.clone(),
                allow_private: p.allow_private,
                eid: None,
                record: None,
            },
        );
    }

    for (i, (a, ta)) in credentials.iter().enumerate() {
        for (b, tb) in &credentials[i + 1..] {
            if ta == tb {
                errors.push(format!(
                    "{a} and {b} resolve to the same value; every credential must be distinct"
                ));
            }
        }
    }

    if errors.is_empty() {
        Ok(staged)
    } else {
        Err(errors)
    }
}

/// 190 (D10): beside `peers.yaml`, the marker that the one-time "every share is full" notice
/// ran (never a `peers.yaml` key: a 186 build refuses unknown keys).
pub const FULL_NOTICE: &str = "a2a/.190-full-notice";

/// 190 (D10): the notice's lines, one per grant (contact × export) of `file`, blocked
/// contacts left out: every share is full now, whatever `modes` said.
pub fn full_notice(file: &PeersFile) -> Vec<String> {
    file.peers
        .iter()
        .filter(|(_, p)| p.is_contact() && !p.blocked)
        .flat_map(|(alias, p)| {
            p.exports
                .iter()
                .map(move |e| format!("a2a: share {e} to {alias} is full (cautious mode removed)"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolver(
        values: &'static [(&'static str, &'static str)],
    ) -> impl Fn(&str) -> Option<Zeroizing<String>> {
        move |n| {
            values
                .iter()
                .find(|(k, _)| *k == n)
                .map(|(_, v)| Zeroizing::new((*v).to_owned()))
        }
    }

    fn file(yaml: &str) -> PeersFile {
        serde_yaml::from_str(yaml).unwrap()
    }

    /// [`super::stage`] with `a2a.contact_cap` at its default.
    fn stage(
        file: &PeersFile,
        resolve: Resolve<'_>,
        console: Option<&str>,
    ) -> Result<Staged, Vec<String>> {
        super::stage(file, resolve, console, tools::Risk::ReadOnly)
    }

    const S: &[(&str, &str)] = &[
        ("out", "o-token"),
        ("in", "i-token"),
        ("in2", "i-token"),
        ("console", "c"),
    ];

    #[test]
    fn listener_rules() {
        let mut c = A2aConfig::default();
        assert!(!c.enabled);
        let l = c.listener().unwrap();
        assert!(!l.production);
        assert_eq!(l.base.as_str(), "http://127.0.0.1:7780/");
        c.listen = "0.0.0.0:7780".into();
        assert!(c.listener().unwrap_err().contains("loopback"));
        c.listen = DEFAULT_LISTEN.into();
        c.public_url = Some("http://a2a.example".into());
        assert!(c.listener().unwrap_err().contains("https"));
        c.public_url = Some("https://user:pw@a2a.example/".into());
        assert!(c.listener().unwrap_err().contains("credentials"));
        c.public_url = Some("https://a2a.example/base/".into());
        let l = c.listener().unwrap();
        assert!(l.production);
        assert_eq!(l.base.as_str(), "https://a2a.example/base");
    }

    #[test]
    fn card_base_rules() {
        assert_eq!(
            card_base("https://lab.example/agents/r/.well-known/agent-card.json")
                .unwrap()
                .as_str(),
            "https://lab.example/agents/r"
        );
        assert!(card_base("http://lab.example/agents/r").is_err());
        assert!(card_base("http://127.0.0.1:7781/agents/probe").is_ok());
        assert!(card_base("https://lab.example/x?token=1").is_err());
        assert!(card_base("https://u:p@lab.example/x").is_err());
    }

    #[test]
    fn egress_policy() {
        assert!(egress_allowed("93.184.216.34".parse().unwrap(), false));
        assert!(!egress_allowed("169.254.169.254".parse().unwrap(), true));
        assert!(!egress_allowed("10.1.2.3".parse().unwrap(), false));
        assert!(egress_allowed("10.1.2.3".parse().unwrap(), true));
        assert!(!egress_allowed("127.0.0.1".parse().unwrap(), false));
        assert!(egress_allowed("::1".parse().unwrap(), true));
        assert!(!egress_allowed("fe80::1".parse().unwrap(), true));
        assert!(!egress_allowed("::ffff:169.254.1.1".parse().unwrap(), true));
    }

    #[test]
    fn full_valid_file() {
        let f = file(
            r#"
peers:
  lab:
    card_url: https://lab.example/agents/research/.well-known/agent-card.json
    outbound_secret: out
    inbound_secret: in
    sessions: ["00000000-0000-0000-0000-000000000001"]
    exports: [research]
exports:
  research:
    session: "00000000-0000-0000-0000-000000000002"
    name: Research
    description: Answer explicitly submitted research requests.
"#,
        );
        let s = stage(&f, &resolver(S), Some("c")).unwrap();
        let lab = &s.peers["lab"];
        assert_eq!(
            lab.card.as_ref().unwrap().as_str(),
            "https://lab.example/agents/research"
        );
        assert_eq!(lab.outbound.as_ref().unwrap().as_str(), "o-token");
        assert_eq!(lab.endpoint(), "https://lab.example/agents/research");
        assert_eq!(s.exports["research"].peer.as_deref(), Some("lab"));
        assert!(!format!("{lab:?}").contains("o-token"));
    }

    /// 183: today's file round-trips with no Phase 3 key written; contact entries and agent
    /// exports stage; a broken contact is refused with every reason.
    #[test]
    fn contacts_and_agent_exports_stage() {
        let yaml = r#"
peers:
  lab:
    inbound_secret: in
    exports: [research]
exports:
  research:
    session: "00000000-0000-0000-0000-000000000002"
    name: Research
"#;
        let f = file(yaml);
        assert!(!f.peers["lab"].is_contact());
        let written = serde_yaml::to_string(&f).unwrap();
        for key in [
            "endpoint",
            "machine",
            "cap",
            "first_contact",
            "blocked",
            "paired_ms",
            "agent",
        ] {
            assert!(
                !written.contains(key),
                "{key} written for an HTTP peer: {written}"
            );
        }
        assert!(stage(&file(&written), &resolver(S), None).is_ok());

        let bob = iroh::SecretKey::from_bytes(&[7; 32]).public().to_string();
        let eve = iroh::SecretKey::from_bytes(&[8; 32]).public().to_string();
        let f = file(&format!(
            r#"
peers:
  bob:
    endpoint: {bob}
    name: Bob
    machine: Bob's MacBook
    cap: mutating
    exports: [research, ops-chat]
    first_contact: {{ research: allow, ops: block }}
    paired_ms: 1
  eve:
    endpoint: {eve}
    exports: [research]
    blocked: true
exports:
  research: {{ agent: research@xeus, name: Research }}
  ops-chat: {{ session: "00000000-0000-0000-0000-000000000003", name: Ops }}
"#
        ));
        let s = super::stage(&f, &resolver(S), None, tools::Risk::ReadOnly).unwrap();
        assert_eq!(s.peers["bob"].endpoint(), format!("iroh:{bob}"));
        assert!(
            !s.peers.contains_key("eve"),
            "a blocked contact has no route"
        );
        assert!(s.contacts["eve"].blocked);
        assert_eq!(s.contacts["eve"].cap, tools::Risk::ReadOnly);
        assert_eq!(s.contacts["bob"].cap, tools::Risk::Mutating);
        assert_eq!(s.contacts["bob"].machine, "Bob's MacBook");
        assert_eq!(s.contact_by_eid(&eve), Some("eve"));
        assert_eq!(s.agents["research"].agent, "research@xeus");
        assert_eq!(s.exports["ops-chat"].peer.as_deref(), Some("bob"));

        let f = file(&format!(
            r#"
peers:
  bob:
    endpoint: {bob}
    card_url: https://lab.example/x
    cap: destructive
  bob2: {{ endpoint: {bob} }}
  nopin: {{ name: X }}
  lab:
    inbound_secret: in
    exports: [research]
exports:
  research: {{ agent: research@xeus, name: Research }}
  both: {{ agent: x, session: "00000000-0000-0000-0000-000000000003", name: Both }}
"#
        ));
        let errors = stage(&f, &resolver(S), None).unwrap_err();
        for e in [
            "peer bob: a contact takes no card_url, secrets, sessions or allow_private",
            "peer bob: cap Destructive: read_only or mutating",
            "peers bob and bob2 pin the same endpoint; one contact per endpoint",
            "peer nopin: a contact needs its endpoint",
            "peer lab: export research is an agent export; only contacts reach agents",
            "export both: set exactly one of session and agent",
        ] {
            assert!(errors.contains(&e.to_owned()), "{e}: {errors:?}");
        }
    }

    /// 190 (D9, D10): a 186-written grant loads full whatever its `modes` said (none,
    /// `full`, `cautious`) and each is in the one-time notice; a file 190 writes has no
    /// `modes`, `cap` or `tools` key and no key a 186 build would refuse.
    #[test]
    fn a_186_grant_loads_full_in_every_variant_and_190_writes_no_new_key() {
        let bob = iroh::SecretKey::from_bytes(&[7; 32]).public().to_string();
        let yaml = |modes: &str| {
            format!(
                "peers:\n  bob:\n    endpoint: {bob}\n    cap: mutating\n    exports: [research, ops]\n{modes}exports:\n  research: {{ agent: research@xeus, name: Research, tools: [search_web] }}\n  ops: {{ session: \"00000000-0000-0000-0000-000000000003\", name: Ops }}\n"
            )
        };
        let mut grants = Vec::new();
        for modes in [
            "",
            "    modes: { research: full }\n",
            "    modes: { research: cautious, ops: cautious }\n",
        ] {
            let f = file(&yaml(modes));
            let s = stage(&f, &resolver(S), None).unwrap();
            assert_eq!(s.peers["bob"].exports, ["research", "ops"], "{modes}");
            assert_eq!(
                full_notice(&f),
                [
                    "a2a: share research to bob is full (cautious mode removed)",
                    "a2a: share ops to bob is full (cautious mode removed)",
                ],
                "{modes}"
            );
            grants.push(s);
        }
        assert!(
            grants[0].same_grant(&grants[1], "bob") && grants[1].same_grant(&grants[2], "bob"),
            "a mode is no grant term any more"
        );

        let f = file(&yaml("    modes: { research: cautious }\n"));
        let written = serde_yaml::to_string(&f).unwrap();
        for key in ["modes", "cap", "tools"] {
            assert!(!written.contains(key), "{key} written: {written}");
        }
        let v: serde_yaml::Value = serde_yaml::from_str(&written).unwrap();
        let known = [
            "card_url",
            "outbound_secret",
            "inbound_secret",
            "sessions",
            "exports",
            "allow_private",
            "endpoint",
            "name",
            "machine",
            "first_contact",
            "blocked",
            "paired_ms",
            "addrs",
        ];
        for k in v["peers"]["bob"].as_mapping().unwrap().keys() {
            assert!(known.contains(&k.as_str().unwrap()), "a new key: {k:?}");
        }
        assert!(stage(&file(&written), &resolver(S), None).is_ok());
    }

    /// 183 F4 (review 1, 2): a held first message may outlive a grant change only for an
    /// export whose authority is unchanged: a chat re-pointed to another session, a removed
    /// grant or a changed contact cap each drop it; a new title or description keeps it, and
    /// so does a hand-edited `tools` list (190 D9: read and dropped).
    #[test]
    fn only_unchanged_exports_keep_held_messages_across_a_grant_change() {
        let bob = iroh::SecretKey::from_bytes(&[7; 32]).public().to_string();
        let staged = |cap: &str, chat: u8, tools: &str, grants: &str, words: &str| {
            super::stage(
                &file(&format!(
                    r#"
peers:
  bob:
    endpoint: {bob}
    cap: {cap}
    exports: [{grants}]
exports:
  chat: {{ session: "00000000-0000-0000-0000-00000000000{chat}", name: Chat{words}, description: "About{words}" }}
  agent: {{ agent: theuth, name: Agent{words}, description: "Does{words}", tools: [{tools}] }}
  other: {{ session: "00000000-0000-0000-0000-000000000009", name: Other }}
"#
                )),
                &resolver(S),
                None,
                tools::Risk::ReadOnly,
            )
            .unwrap()
        };
        let base = staged("read_only", 1, "search_web", "chat, agent, other", "");
        let same = |n: &Staged| base.unchanged_exports(n, "bob");
        assert_eq!(
            same(&staged(
                "read_only",
                1,
                "search_web",
                "chat, agent, other",
                " v2"
            )),
            ["chat", "agent", "other"],
            "a title or description edit keeps every hold"
        );
        assert_eq!(
            same(&staged("read_only", 1, "search_web", "chat, agent", "")),
            ["chat", "agent"],
            "a removed grant drops only its own holds"
        );
        assert_eq!(
            same(&staged(
                "read_only",
                2,
                "search_web",
                "chat, agent, other",
                ""
            )),
            ["agent", "other"],
            "the chat now names another session"
        );
        assert_eq!(
            same(&staged(
                "read_only",
                1,
                "read_url",
                "chat, agent, other",
                ""
            )),
            ["chat", "agent", "other"],
            "an export's tools are no authority any more"
        );
        assert!(
            same(&staged(
                "mutating",
                1,
                "search_web",
                "chat, agent, other",
                ""
            ))
            .is_empty(),
            "a changed cap drops every hold"
        );
    }

    #[test]
    fn refusals_are_complete_and_atomic() {
        let f = file(
            r#"
peers:
  Bad Name:
    card_url: http://lab.example/x
    sessions: ["00000000-0000-0000-0000-000000000001"]
  a:
    inbound_secret: console
    exports: [research, missing]
  b:
    inbound_secret: in
    exports: [research]
  c:
    inbound_secret: in2
    exports: [research]
  d:
    card_url: https://ok.example/agents/x
    outbound_secret: out
    inbound_secret: out
    sessions: ["00000000-0000-0000-0000-000000000001"]
    exports: [other]
exports:
  research:
    session: "00000000-0000-0000-0000-000000000002"
    name: Research
  other:
    session: "00000000-0000-0000-0000-000000000002"
    name: Other
"#,
        );
        let errs = stage(&f, &resolver(S), Some("c")).unwrap_err();
        let has = |needle: &str| errs.iter().any(|e| e.contains(needle));
        assert!(has("peer name \"Bad Name\""));
        assert!(has("https required"));
        assert!(has("card_url needs an outbound_secret"));
        assert!(has("console token"));
        assert!(has("unknown export \"missing\""));
        assert!(has("granted to both"));
        assert!(has("resolve to the same value"));
        assert!(has("publish the same session"));
    }

    /// An outbound bearer equal to any inbound bearer would authenticate as
    /// that principal: refused on the resolved value, whether the vault names
    /// agree or not.
    #[test]
    fn credential_values_are_distinct_across_peers_and_roles() {
        const V: &[(&str, &str)] = &[
            ("a-out", "same"),
            ("b-in", "same"),
            ("shared", "same"),
            ("c-in", "other"),
        ];
        let yaml = |a_out: &str, b_in: &str| {
            file(&format!(
                r#"
peers:
  a:
    card_url: https://a.example/agents/x
    outbound_secret: {a_out}
    sessions: ["00000000-0000-0000-0000-000000000001"]
  b:
    inbound_secret: {b_in}
    exports: [research]
exports:
  research:
    session: "00000000-0000-0000-0000-000000000002"
    name: Research
"#
            ))
        };
        for (a_out, b_in) in [("a-out", "b-in"), ("shared", "shared")] {
            let errs = stage(&yaml(a_out, b_in), &resolver(V), None).unwrap_err();
            assert!(
                errs.iter().any(|e| e.contains("peer a: outbound_secret")
                    && e.contains("peer b: inbound_secret")
                    && e.contains("same value")),
                "{a_out}/{b_in}: {errs:?}"
            );
        }
        assert!(stage(&yaml("a-out", "c-in"), &resolver(V), None).is_ok());
    }

    #[test]
    fn same_grant_tracks_credentials_exports_and_roots() {
        let yaml = |secret: &str, session: &str, root: &str| {
            file(&format!(
                r#"
peers:
  lab:
    card_url: https://lab.example/agents/research
    outbound_secret: out
    inbound_secret: {secret}
    sessions: ["00000000-0000-0000-0000-00000000000{root}"]
    exports: [research]
  reader:
    inbound_secret: in2
    exports: [other]
exports:
  research:
    session: "00000000-0000-0000-0000-00000000000{session}"
    name: Research
  other:
    session: "00000000-0000-0000-0000-000000000009"
    name: Other
"#
            ))
        };
        const V: &[(&str, &str)] = &[("out", "o"), ("in", "i"), ("rotated", "r"), ("in2", "i2")];
        let base = stage(&yaml("in", "2", "1"), &resolver(V), None).unwrap();
        let same = stage(&yaml("in", "2", "1"), &resolver(V), None).unwrap();
        assert!(base.same_grant(&same, "lab") && base.same_grant(&same, "reader"));
        // Rotation, export remap and root change each retire only that peer.
        for changed in [
            yaml("rotated", "2", "1"),
            yaml("in", "3", "1"),
            yaml("in", "2", "4"),
        ] {
            let next = stage(&changed, &resolver(V), None).unwrap();
            assert!(!base.same_grant(&next, "lab"));
            assert!(base.same_grant(&next, "reader"));
        }
        assert!(!base.same_grant(&Staged::default(), "lab"));
    }

    #[test]
    fn inbound_only_and_outbound_only() {
        let f = file(
            r#"
peers:
  reader:
    inbound_secret: in
    exports: [research]
  target:
    card_url: https://t.example/agents/a
    outbound_secret: out
    sessions: ["00000000-0000-0000-0000-000000000001"]
exports:
  research:
    session: "00000000-0000-0000-0000-000000000002"
    name: Research
"#,
        );
        let s = stage(&f, &resolver(S), None).unwrap();
        assert_eq!(s.peers["reader"].endpoint(), "inbound");
        assert!(s.peers["target"].inbound.is_none());
        assert!(s.peers["target"].exports.is_empty());
    }

    #[test]
    fn empty_file_is_valid() {
        let s = stage(&PeersFile::default(), &resolver(S), None).unwrap();
        assert!(s.peers.is_empty() && s.exports.is_empty());
    }
}
