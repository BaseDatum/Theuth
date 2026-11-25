//! `grants.yaml` read/write, HMAC-SHA256 under the vault row `plugins/grants-key` (design D13, §11).
//!
//! Each MAC also covers the plugin's grant *generation*, a counter kept in the vault's local-only
//! `meta` table beside the key (review R3). Every revoke (uninstall, disable) and re-approve bumps
//! it, so an old copy of an entry written back into `grants.yaml` no longer verifies.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::Path;
use std::sync::Arc;

use hmac::{Hmac, KeyInit as _, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::types::Grants;

type HmacSha256 = Hmac<Sha256>;

/// File name under the plugins dir.
pub(crate) const FILE: &str = "grants.yaml";

/// The vault `meta` key holding the per-plugin generations (JSON `{name: n}`), beside
/// [`secrets::PLUGINS_GRANTS_KEY`]; local-only like the key, never replicated.
pub(crate) const GENERATIONS_KEY: &str = "plugins/grants-generations";

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
    /// Per-plugin generations; never written to the file.
    #[serde(skip)]
    gens: Generations,
}

/// The generations and where they persist. Without an attached vault they live in memory only
/// (tests; an engine without a vault has no key either, so nothing verifies there anyway).
#[derive(Clone, Default)]
struct Generations {
    map: BTreeMap<String, u64>,
    vault: Option<Arc<secrets::Vault>>,
    /// [`GrantsFile::attach`] failed: the counters are unknown, so nothing verifies.
    unknown: bool,
}

impl std::fmt::Debug for Generations {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Generations")
            .field("map", &self.map)
            .field("vault", &self.vault.is_some())
            .field("unknown", &self.unknown)
            .finish()
    }
}

impl PartialEq for Generations {
    fn eq(&self, other: &Self) -> bool {
        self.map == other.map && self.unknown == other.unknown
    }
}

/// One plugin's approval; `mac` covers every other field, the plugin name and its generation.
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
    /// Lowercase hex HMAC-SHA256 over the canonical JSON of the name, the generation and every
    /// field above.
    pub mac: String,
}

/// The MAC'd view of an entry: the name, the generation and every field but `mac`.
#[derive(Serialize)]
struct Signed<'a> {
    name: &'a str,
    generation: u64,
    manifest_sha256: &'a str,
    code_sha256: Option<&'a str>,
    approved_at: &'a str,
    approved_via: &'a str,
    enabled: bool,
    grants: &'a Grants,
}

/// Canonical JSON bytes of (name, generation, entry without `mac`): through
/// `serde_json::Value`, compact. An entry that does not serialise is an error, never a
/// stand-in value (review R29).
fn canonical(name: &str, generation: u64, e: &GrantEntry) -> anyhow::Result<Vec<u8>> {
    let signed = Signed {
        name,
        generation,
        manifest_sha256: &e.manifest_sha256,
        code_sha256: e.code_sha256.as_deref(),
        approved_at: &e.approved_at,
        approved_via: &e.approved_via,
        enabled: e.enabled,
        grants: &e.grants,
    };
    let value = serde_json::to_value(&signed)?;
    Ok(serde_json::to_vec(&value)?)
}

fn mac_for(
    key: &[u8; 32],
    name: &str,
    generation: u64,
    e: &GrantEntry,
) -> anyhow::Result<HmacSha256> {
    let mut mac = HmacSha256::new_from_slice(key)?;
    mac.update(&canonical(name, generation, e)?);
    Ok(mac)
}

/// Set `e.mac` to the HMAC-SHA256 of its canonical JSON at `generation` under `key`.
fn sign(key: &[u8; 32], name: &str, generation: u64, e: &mut GrantEntry) -> anyhow::Result<()> {
    e.mac = hex::encode(mac_for(key, name, generation, e)?.finalize().into_bytes());
    Ok(())
}

/// Constant-time check of `e.mac` at `generation`; a malformed hex string or an entry that does
/// not serialise fails.
fn verify(key: &[u8; 32], name: &str, generation: u64, e: &GrantEntry) -> bool {
    let Ok(want) = hex::decode(&e.mac) else {
        return false;
    };
    mac_for(key, name, generation, e).is_ok_and(|m| m.verify_slice(&want).is_ok())
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
    /// Read `path`: missing → empty. Every top-level key parses on its own (review R27): a
    /// malformed entry is dropped with a warning and the others stay; an unparsable file is empty.
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
        let map: BTreeMap<String, serde_yaml::Value> = match serde_yaml::from_str(&text) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "plugins: unparsable grants, ignored");
                return Self::default();
            }
        };
        let mut out = Self::default();
        for (k, v) in map {
            let parsed = match k.as_str() {
                "order" => serde_yaml::from_value(v).map(|o| out.order = o),
                "hidden" => serde_yaml::from_value(v).map(|h| out.hidden = h),
                _ => serde_yaml::from_value::<GrantEntry>(v).map(|e| {
                    out.entries.insert(k.clone(), e);
                }),
            };
            if let Err(e) = parsed {
                tracing::warn!(path = %path.display(), key = %k, error = %e, "plugins: malformed grants entry, ignored");
            }
        }
        out
    }

    /// Read the generations from the vault's `meta` table; every later bump persists there. On
    /// `Err` the counters are unknown: [`GrantsFile::verified`] answers `None` for every plugin
    /// and nothing can be signed, bumped or revoked (fail closed).
    pub async fn attach(&mut self, vault: Arc<secrets::Vault>) -> anyhow::Result<()> {
        let read = match secrets::store::meta_get(vault.pool(), GENERATIONS_KEY).await {
            Ok(Some(bytes)) => serde_json::from_slice(&bytes).map_err(anyhow::Error::from),
            Ok(None) => Ok(BTreeMap::new()),
            Err(e) => Err(e),
        };
        let (map, unknown, out) = match read {
            Ok(map) => (map, false, Ok(())),
            Err(e) => (
                BTreeMap::new(),
                true,
                Err(e.context("plugins: grant generations unreadable")),
            ),
        };
        self.gens = Generations {
            map,
            vault: Some(vault),
            unknown,
        };
        out
    }

    /// The current generation of `name` (0 before its first bump).
    pub fn generation(&self, name: &str) -> u64 {
        self.gens.map.get(name).copied().unwrap_or(0)
    }

    /// `name`'s entry when its MAC verifies under `key` at the current generation.
    pub fn verified(&self, key: &[u8; 32], name: &str) -> Option<&GrantEntry> {
        if self.gens.unknown {
            return None;
        }
        let e = self.entries.get(name)?;
        verify(key, name, self.generation(name), e).then_some(e)
    }

    /// Sign `entry` under `key` at `name`'s current generation and store it (appended to `order`
    /// if new). Call [`GrantsFile::bump`] first when it replaces an earlier approval, so the
    /// earlier one cannot be written back. Not saved: call [`GrantsFile::save`].
    pub fn insert_signed(
        &mut self,
        key: &[u8; 32],
        name: &str,
        mut entry: GrantEntry,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(!self.gens.unknown, "plugins: grant generations unreadable");
        sign(key, name, self.generation(name), &mut entry)?;
        self.entries.insert(name.to_owned(), entry);
        if !self.order.iter().any(|n| n == name) {
            self.order.push(name.to_owned());
        }
        Ok(())
    }

    /// Advance `name`'s generation and persist it before returning: every entry signed earlier,
    /// the one in memory included, stops verifying. Used on re-approve, before
    /// [`GrantsFile::insert_signed`].
    pub async fn bump(&mut self, name: &str) -> anyhow::Result<()> {
        anyhow::ensure!(!self.gens.unknown, "plugins: grant generations unreadable");
        let mut map = self.gens.map.clone();
        let next = map.get(name).copied().unwrap_or(0).saturating_add(1);
        map.insert(name.to_owned(), next);
        if let Some(v) = &self.gens.vault {
            secrets::store::meta_put(v.pool(), GENERATIONS_KEY, &serde_json::to_vec(&map)?).await?;
        }
        self.gens.map = map;
        Ok(())
    }

    /// Revoke `name`'s approval (uninstall, disable): bump its generation, then remove its entry.
    /// `order` and `hidden` are left to the caller. Not saved: call [`GrantsFile::save`].
    pub async fn revoke(&mut self, name: &str) -> anyhow::Result<()> {
        self.bump(name).await?;
        self.entries.remove(name);
        Ok(())
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

    /// A named change to an entry, and a change to a grant set.
    type Mutation<'a> = (&'a str, Box<dyn Fn(&mut GrantEntry)>);
    type Widen = Box<dyn Fn(&mut Grants)>;

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

    fn unsigned() -> GrantEntry {
        GrantEntry {
            manifest_sha256: "ab".repeat(32),
            code_sha256: Some("cd".repeat(32)),
            approved_at: "2026-11-24T16:40:02Z".into(),
            approved_via: "desktop".into(),
            enabled: true,
            grants: grants(),
            mac: String::new(),
        }
    }

    fn entry() -> GrantEntry {
        let mut e = unsigned();
        sign(&KEY, "turns", 0, &mut e).unwrap();
        e
    }

    #[test]
    fn roundtrip_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        let mut file = GrantsFile::default();
        file.insert_signed(&KEY, "turns", unsigned()).unwrap();
        assert_eq!(file.order, vec!["turns".to_string()]);
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
        let e = back.verified(&KEY, "turns").expect("verifies");
        assert_eq!(e.mac.len(), 64);
        assert_eq!(e.mac, e.mac.to_lowercase());
        assert!(!verify(&KEY, "other", 0, e), "the name is MAC'd");
        assert!(!verify(&[8; 32], "turns", 0, e), "another key fails");
        assert!(back.is_hidden("turns", "turns"));
        assert_eq!(
            GrantsFile::load(&dir.path().join("missing.yaml")),
            GrantsFile::default()
        );
    }

    #[test]
    fn any_byte_change_fails_verification() {
        let base = entry();
        assert!(verify(&KEY, "turns", 0, &base));
        assert!(!verify(&KEY, "turns", 1, &base), "the generation is MAC'd");
        let mutations: Vec<Mutation> = vec![
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
                !verify(&KEY, "turns", 0, &e),
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
        assert!(f.verified(&KEY, "turns").is_some());

        let mut f = f;
        if let Some(e) = f.entries.get_mut("turns") {
            e.enabled = false;
        }
        f.save(&path).unwrap();
        let f = GrantsFile::load(&path);
        assert!(f.verified(&KEY, "turns").is_none());
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
        assert!(f.verified(&KEY, "turns").is_none());
        let mut garbage = e.clone();
        garbage.mac = "not hex".into();
        assert!(!verify(&KEY, "turns", 0, &garbage));
    }

    /// R27: one malformed entry costs only itself, not every other approval.
    #[test]
    fn malformed_entry_keeps_the_others() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        let mut good = GrantsFile::default();
        good.insert_signed(&KEY, "turns", unsigned()).unwrap();
        good.save(&path).unwrap();
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("broken:\n  manifest_sha256: [not, a, string]\n");
        std::fs::write(&path, &text).unwrap();
        let f = GrantsFile::load(&path);
        assert!(f.verified(&KEY, "turns").is_some());
        assert!(!f.entries.contains_key("broken"));
        assert_eq!(f.order, vec!["turns".to_string()]);
    }

    /// R3: a copy of an entry taken before revoke or re-approve does not verify afterwards.
    #[tokio::test]
    async fn revoke_and_bump_stop_old_copies_verifying() {
        let mut f = GrantsFile::default();
        f.insert_signed(&KEY, "turns", unsigned()).unwrap();
        let old = f.entries["turns"].clone();
        f.revoke("turns").await.unwrap();
        assert!(!f.entries.contains_key("turns"));
        f.entries.insert("turns".into(), old);
        assert!(f.verified(&KEY, "turns").is_none(), "replayed after revoke");

        f.bump("turns").await.unwrap();
        f.insert_signed(&KEY, "turns", unsigned()).unwrap();
        assert!(f.verified(&KEY, "turns").is_some());
        let approved = f.entries["turns"].clone();
        f.bump("turns").await.unwrap();
        f.entries.insert("turns".into(), approved);
        assert!(f.verified(&KEY, "turns").is_none(), "replayed after bump");
        assert_eq!(f.generation("turns"), 3);
        assert_eq!(f.generation("other"), 0);
    }

    /// R3: the generation survives a restart through the vault's meta table.
    #[tokio::test]
    async fn generations_persist_in_the_vault() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        let vault = Arc::new(
            secrets::Vault::open(dir.path(), secrets::KeyStore::File)
                .await
                .unwrap(),
        );
        let mut f = GrantsFile::load(&path);
        f.attach(vault.clone()).await.unwrap();
        f.insert_signed(&KEY, "turns", unsigned()).unwrap();
        let old = f.entries["turns"].clone();
        f.revoke("turns").await.unwrap();
        f.save(&path).unwrap();

        // The model writes the old entry back; the next boot reads it.
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str(&serde_yaml::to_string(&BTreeMap::from([("turns", &old)])).unwrap());
        std::fs::write(&path, text).unwrap();
        let mut boot = GrantsFile::load(&path);
        assert!(boot.entries.contains_key("turns"));
        boot.attach(vault).await.unwrap();
        assert_eq!(boot.generation("turns"), 1);
        assert!(boot.verified(&KEY, "turns").is_none());
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

        let wider: Vec<Widen> = vec![
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
