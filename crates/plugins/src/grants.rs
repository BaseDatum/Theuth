//! `grants.yaml` read/write, HMAC-SHA256 under the vault row `plugins/grants-key` (design D13, §11).

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::Path;

use hmac::{Hmac, KeyInit as _, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::types::Grants;

type HmacSha256 = Hmac<Sha256>;

/// File name under the plugins dir.
pub(crate) const FILE: &str = "grants.yaml";

/// The whole `grants.yaml`: one entry per plugin, plus the unauthenticated UI order and hidden list.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct GrantsFile {
    /// Plugin name → its approval.
    #[serde(flatten)]
    pub entries: BTreeMap<String, GrantEntry>,
    /// Sidebar/status order (§9.1); not MAC'd.
    #[serde(default)]
    pub order: Vec<String>,
    /// Hidden placements `<plugin>/<placement>` (§9.1); not MAC'd.
    #[serde(default)]
    pub hidden: Vec<String>,
}

/// One plugin's approval; `mac` covers every other field and the plugin name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct GrantEntry {
    /// The card that was pressed; identifies, does not gate.
    pub manifest_sha256: String,
    /// Set only for plugins holding a secret or more than read-only power.
    pub code_sha256: Option<String>,
    /// RFC 3339 time of the press.
    pub approved_at: String,
    /// The client that pressed.
    pub approved_via: String,
    /// Owner switch.
    pub enabled: bool,
    /// What the owner approved.
    pub grants: Grants,
    /// Lowercase hex HMAC-SHA256 over the canonical JSON of the name and every field above.
    pub mac: String,
}

/// The MAC'd view of an entry: the name and every field but `mac`.
#[derive(Serialize)]
struct Signed<'a> {
    name: &'a str,
    manifest_sha256: &'a str,
    code_sha256: Option<&'a str>,
    approved_at: &'a str,
    approved_via: &'a str,
    enabled: bool,
    grants: &'a Grants,
}

/// Canonical JSON bytes of (name, entry without `mac`): through `serde_json::Value`, compact.
fn canonical(name: &str, e: &GrantEntry) -> Vec<u8> {
    let signed = Signed {
        name,
        manifest_sha256: &e.manifest_sha256,
        code_sha256: e.code_sha256.as_deref(),
        approved_at: &e.approved_at,
        approved_via: &e.approved_via,
        enabled: e.enabled,
        grants: &e.grants,
    };
    let value = serde_json::to_value(&signed).unwrap_or(serde_json::Value::Null);
    serde_json::to_vec(&value).unwrap_or_default()
}

fn mac_for(key: &[u8; 32], name: &str, e: &GrantEntry) -> HmacSha256 {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC takes any key length");
    mac.update(&canonical(name, e));
    mac
}

/// Set `e.mac` to the HMAC-SHA256 of its canonical JSON under `key`.
pub(crate) fn sign(key: &[u8; 32], name: &str, e: &mut GrantEntry) {
    e.mac = hex::encode(mac_for(key, name, e).finalize().into_bytes());
}

/// Constant-time check of `e.mac`; a malformed hex string fails.
pub(crate) fn verify(key: &[u8; 32], name: &str, e: &GrantEntry) -> bool {
    let Ok(want) = hex::decode(&e.mac) else {
        return false;
    };
    mac_for(key, name, e).verify_slice(&want).is_ok()
}

/// `requested` ⊆ `granted`: subscribe/emits/placements/secrets subsets, run/cwd/env equal, every tool
/// present with risk ≤ granted, capabilities: notify/input only if granted, call tools ⊆ and max_risk ≤.
pub(crate) fn covers(granted: &Grants, requested: &Grants) -> bool {
    let subset = |r: &[String], g: &[String]| r.iter().all(|x| g.contains(x));
    let tools = requested.tools.iter().all(|t| {
        granted
            .tools
            .iter()
            .any(|g| g.name == t.name && t.risk <= g.risk)
    });
    let gc = &granted.capabilities;
    let rc = &requested.capabilities;
    let call = match (&rc.call, &gc.call) {
        (None, _) => true,
        (Some(_), None) => false,
        (Some(r), Some(g)) => subset(&r.tools, &g.tools) && r.max_risk <= g.max_risk,
    };
    subset(&requested.subscribe, &granted.subscribe)
        && subset(&requested.emits, &granted.emits)
        && subset(&requested.placements, &granted.placements)
        && subset(&requested.secrets, &granted.secrets)
        && requested.run == granted.run
        && requested.cwd == granted.cwd
        && requested.env == granted.env
        && tools
        && (!rc.notify || gc.notify)
        && (!rc.input || gc.input)
        && call
}

impl GrantsFile {
    /// Read `path`: missing → empty; unparsable → empty with a warning.
    pub fn load(path: &Path) -> Self {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "plugins: cannot read grants");
                return Self::default();
            }
        };
        if text.trim().is_empty() {
            return Self::default();
        }
        serde_yaml::from_str(&text).unwrap_or_else(|e| {
            tracing::warn!(path = %path.display(), error = %e, "plugins: unparsable grants, ignored");
            Self::default()
        })
    }

    /// Write atomically (temp file + rename), mode 0600.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let text = serde_yaml::to_string(self).map_err(std::io::Error::other)?;
        let dir = path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(format!(".{FILE}.{}.tmp", std::process::id()));
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            opts.mode(0o600);
        }
        let written = (|| {
            let mut f = opts.open(&tmp)?;
            f.write_all(text.as_bytes())?;
            f.sync_all()?;
            std::fs::rename(&tmp, path)
        })();
        if written.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        written
    }

    /// Whether `<plugin>/<placement>` is hidden.
    pub fn is_hidden(&self, plugin: &str, placement: &str) -> bool {
        let key = format!("{plugin}/{placement}");
        self.hidden.contains(&key)
    }

    /// Hide or show `<plugin>/<placement>`.
    pub fn set_hidden(&mut self, plugin: &str, placement: &str, hidden: bool) {
        let key = format!("{plugin}/{placement}");
        let present = self.hidden.contains(&key);
        if hidden && !present {
            self.hidden.push(key);
        } else if !hidden && present {
            self.hidden.retain(|h| *h != key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CallGrant, Capabilities, ToolGrant};
    use tools::Risk;

    const KEY: [u8; 32] = [7; 32];

    fn grants() -> Grants {
        Grants {
            subscribe: vec!["agent.events#TurnFinished".into()],
            emits: vec!["limit_reached".into()],
            placements: vec!["turns".into()],
            tools: vec![ToolGrant {
                name: "turns_today".into(),
                risk: Risk::ReadOnly,
            }],
            capabilities: Capabilities::default(),
            secrets: vec![],
            run: vec!["python3".into(), "main.py".into()],
            cwd: ".".into(),
            env: BTreeMap::from([("TURNS_LIMIT".into(), "50".into())]),
        }
    }

    fn entry() -> GrantEntry {
        let mut e = GrantEntry {
            manifest_sha256: "ab".repeat(32),
            code_sha256: Some("cd".repeat(32)),
            approved_at: "2026-11-24T16:40:02Z".into(),
            approved_via: "desktop".into(),
            enabled: true,
            grants: grants(),
            mac: String::new(),
        };
        sign(&KEY, "turns", &mut e);
        e
    }

    #[test]
    fn roundtrip_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        let mut file = GrantsFile::default();
        file.entries.insert("turns".into(), entry());
        file.order = vec!["turns".into()];
        file.set_hidden("turns", "turns", true);
        file.save(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let back = GrantsFile::load(&path);
        assert_eq!(back, file);
        let e = &back.entries["turns"];
        assert_eq!(e.mac.len(), 64);
        assert_eq!(e.mac, e.mac.to_lowercase());
        assert!(verify(&KEY, "turns", e));
        assert!(!verify(&KEY, "other", e), "the name is MAC'd");
        assert!(!verify(&[8; 32], "turns", e), "another key fails");
        assert!(back.is_hidden("turns", "turns"));
        assert_eq!(
            GrantsFile::load(&dir.path().join("missing.yaml")),
            GrantsFile::default()
        );
    }

    #[test]
    fn any_byte_change_fails_verification() {
        let base = entry();
        assert!(verify(&KEY, "turns", &base));
        let mutations: Vec<(&str, Box<dyn Fn(&mut GrantEntry)>)> = vec![
            ("run", Box::new(|e| e.grants.run[1] = "main2.py".into())),
            (
                "env value",
                Box::new(|e| {
                    e.grants.env.insert("TURNS_LIMIT".into(), "51".into());
                }),
            ),
            (
                "approved_at",
                Box::new(|e| e.approved_at = "2026-11-24T16:40:03Z".into()),
            ),
            (
                "code hash",
                Box::new(|e| e.code_sha256 = Some("ce".repeat(32))),
            ),
            ("code hash removed", Box::new(|e| e.code_sha256 = None)),
            (
                "subscribe entry",
                Box::new(|e| e.grants.subscribe[0] = "agent.events#TurnStarted".into()),
            ),
            (
                "mac byte",
                Box::new(|e| {
                    e.mac
                        .replace_range(0..1, if e.mac.starts_with('0') { "1" } else { "0" })
                }),
            ),
        ];
        for (what, mutate) in mutations {
            let mut e = base.clone();
            mutate(&mut e);
            assert!(
                !verify(&KEY, "turns", &e),
                "{what} changed but still verifies"
            );
        }
    }

    #[test]
    fn enabled_is_mac_covered_order_hidden_are_not() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        let mut file = GrantsFile::default();
        file.entries.insert("turns".into(), entry());
        file.save(&path).unwrap();

        let mut f = GrantsFile::load(&path);
        f.order = vec!["ping".into(), "turns".into()];
        f.set_hidden("turns", "turns", true);
        f.save(&path).unwrap();
        let f = GrantsFile::load(&path);
        assert!(verify(&KEY, "turns", &f.entries["turns"]));

        let mut f = f;
        if let Some(e) = f.entries.get_mut("turns") {
            e.enabled = false;
        }
        f.save(&path).unwrap();
        let f = GrantsFile::load(&path);
        assert!(!verify(&KEY, "turns", &f.entries["turns"]));
    }

    #[test]
    fn handwritten_grant_does_not_verify() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        let mac = "9c0e".repeat(16);
        let text = format!(
            "turns:\n  manifest_sha256: abc\n  code_sha256: null\n  approved_at: 2026-11-24T16:40:02Z\n  approved_via: desktop\n  enabled: true\n  grants:\n    subscribe: [\"agent.events#TurnFinished\"]\n    emits: [limit_reached]\n    placements: [turns]\n    tools: [{{name: turns_today, risk: read_only}}]\n    capabilities: {{notify: false, input: false, call: null}}\n    secrets: []\n    run: [python3, main.py]\n    cwd: .\n    env: {{TURNS_LIMIT: \"50\"}}\n  mac: {mac}\norder: [turns]\nhidden: []\n"
        );
        std::fs::write(&path, text).unwrap();
        let f = GrantsFile::load(&path);
        let e = &f.entries["turns"];
        assert_eq!(e.grants.tools[0].risk, Risk::ReadOnly);
        assert_eq!(f.order, vec!["turns".to_string()]);
        assert!(!verify(&KEY, "turns", e));
        let mut garbage = e.clone();
        garbage.mac = "not hex".into();
        assert!(!verify(&KEY, "turns", &garbage));
    }

    #[test]
    fn covers_checks_every_dimension() {
        let mut granted = grants();
        granted.capabilities.call = Some(CallGrant {
            tools: vec!["read_file".into(), "write_file".into()],
            max_risk: Risk::Mutating,
        });
        granted.capabilities.notify = true;
        let mut req = grants();
        assert!(covers(&granted, &req));
        req.subscribe.clear();
        req.capabilities.notify = true;
        req.capabilities.call = Some(CallGrant {
            tools: vec!["read_file".into()],
            max_risk: Risk::ReadOnly,
        });
        assert!(covers(&granted, &req), "narrower is covered");

        let wider: Vec<Box<dyn Fn(&mut Grants)>> = vec![
            Box::new(|g| g.subscribe.push("agent.events".into())),
            Box::new(|g| g.emits.push("other".into())),
            Box::new(|g| g.placements.push("panel".into())),
            Box::new(|g| g.secrets.push("TOKEN".into())),
            Box::new(|g| g.run.push("--flag".into())),
            Box::new(|g| g.cwd = "src".into()),
            Box::new(|g| {
                g.env.insert("X".into(), "1".into());
            }),
            Box::new(|g| g.tools[0].risk = Risk::Mutating),
            Box::new(|g| {
                g.tools.push(ToolGrant {
                    name: "other".into(),
                    risk: Risk::ReadOnly,
                })
            }),
            Box::new(|g| g.capabilities.input = true),
            Box::new(|g| {
                if let Some(c) = g.capabilities.call.as_mut() {
                    c.max_risk = Risk::Destructive;
                }
            }),
            Box::new(|g| {
                if let Some(c) = g.capabilities.call.as_mut() {
                    c.tools.push("shell".into());
                }
            }),
        ];
        for (i, widen) in wider.iter().enumerate() {
            let mut r = req.clone();
            widen(&mut r);
            assert!(!covers(&granted, &r), "widening #{i} was covered");
        }
        let mut no_call = granted.clone();
        no_call.capabilities.call = None;
        assert!(!covers(&no_call, &req));
    }
}
