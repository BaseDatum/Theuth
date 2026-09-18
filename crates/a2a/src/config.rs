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
}

impl Default for A2aConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            listen: DEFAULT_LISTEN.to_owned(),
            public_url: None,
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

/// `peers.yaml` as written by the owner (§6.1).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PeersFile {
    /// Remote agents by owner alias.
    pub peers: BTreeMap<String, PeerEntry>,
    /// Local sessions published by export name.
    pub exports: BTreeMap<String, ExportEntry>,
}

/// One configured remote agent.
#[derive(Clone, Debug, Default, Deserialize)]
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
}

/// One published local session.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportEntry {
    /// Exact native session (not its tree).
    pub session: SessionId,
    /// Card name.
    pub name: String,
    /// Card description; owner-written functional capability, no tool inventory.
    #[serde(default)]
    pub description: String,
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
}

impl Peer {
    /// Stable identity string for the store binding: the card origin+path, or
    /// `inbound` for a peer that never gets called (§5.1).
    pub fn endpoint(&self) -> String {
        self.card
            .as_ref()
            .map_or_else(|| "inbound".to_owned(), ToString::to_string)
    }
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
    /// By alias.
    pub peers: BTreeMap<String, Peer>,
    /// By export name.
    pub exports: BTreeMap<String, Export>,
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
            && a.exports
                .iter()
                .all(|e| self.exports.get(e) == other.exports.get(e))
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

/// Validate the whole file (§6.1). `console` is the RPC console token, which
/// no peer credential may equal. Returns every problem, not the first.
pub fn stage(
    file: &PeersFile,
    resolve: Resolve<'_>,
    console: Option<&str>,
) -> Result<Staged, Vec<String>> {
    let mut errors = Vec::new();
    let mut staged = Staged::default();

    let mut export_sessions: HashMap<SessionId, &str> = HashMap::new();
    for (name, e) in &file.exports {
        check_name("export", name, &mut errors);
        if e.name.trim().is_empty() {
            errors.push(format!("export {name}: empty name"));
        }
        if let Some(other) = export_sessions.insert(e.session, name) {
            errors.push(format!(
                "exports {other} and {name} publish the same session {}; one export per session",
                e.session
            ));
        }
        staged.exports.insert(
            name.clone(),
            Export {
                name: name.clone(),
                session: e.session,
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
    for (name, p) in &file.peers {
        check_name("peer", name, &mut errors);
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
                match staged.exports.get_mut(export) {
                    None => errors.push(format!("peer {name}: unknown export {export:?}")),
                    Some(x) => match &x.peer {
                        Some(other) if other != name => errors.push(format!(
                            "export {export} is granted to both {other} and {name}; one principal per exported session"
                        )),
                        _ => x.peer = Some(name.clone()),
                    },
                }
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
