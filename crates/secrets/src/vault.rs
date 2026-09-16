//! The vault (design doc 37 §3.5): `<data_dir>/secrets.vault` is one
//! XChaCha20-Poly1305 blob holding every secret's last [`KEEP`] versions.
//! Loaded once, reloaded whenever the file's mtime moves (a TUI, a daemon
//! and `wth --secret-set` on one data dir see each other's writes), written
//! atomically. Values never leave this module except through [`Vault::get`]
//! and [`Vault::expand`]; [`Vault::redact`] turns them back into placeholders.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::SystemTime;

use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};
use anyhow::Context as _;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use chacha20poly1305::XChaCha20Poly1305;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use regex::Regex;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

/// Versions kept per name. Not config: a sixth is the one nobody asked for.
pub const KEEP: usize = 5;

/// Values shorter than this (in chars) are not scrubbed from tool output:
/// too many false positives, and the receipt says so.
pub const MIN_REDACT: usize = 8;

/// Regex source for a placeholder: group 1 = name, group 2 = versions back (1..=4).
pub const PLACEHOLDER: &str = r"\{\{secret:([A-Za-z0-9_.\-]+)(?::-([1-4]))?\}\}";

const MAGIC: &[u8] = b"wth-secrets-1\n";
const FILE: &str = "secrets.vault";
const NONCE: usize = 24;

/// Where the vault key lives (`secrets.key_store`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyStore {
    /// The OS keychain, falling back to the file when none is reachable.
    #[default]
    Auto,
    /// The OS keychain only; opening fails without one.
    Keychain,
    /// `<data_dir>/secrets.key`, mode 0600.
    File,
}

/// One kept version. Wire-visible: never a value, never a length.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct VersionInfo {
    /// Per-name monotonic number; never reused.
    pub v: u32,
    /// RFC 3339 UTC.
    pub created: String,
    /// Who wrote it: `user`, `cli`, `shell`, `import`, `restore v3`, `provider add`, …
    pub source: String,
}

/// One secret as the model and the UIs see it.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SecretInfo {
    /// `[A-Za-z0-9_.-]{1,64}`.
    pub name: String,
    /// Per-name description, shown in lists.
    pub note: String,
    /// Backend; always `local` in v0.
    pub source: String,
    /// `v` of the current version.
    pub current: u32,
    /// Newest first, at most [`KEEP`].
    pub versions: Vec<VersionInfo>,
    /// `created` of the current version.
    pub updated: String,
}

/// `secret_list` / `list_secrets` result.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SecretsList {
    /// `keychain` or `file`.
    pub key_store: String,
    /// Sorted by name.
    pub secrets: Vec<SecretInfo>,
}

/// A local name: `[A-Za-z0-9_.-]{1,64}`. `/` is reserved for backends (37 §11).
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}

#[derive(Serialize, Deserialize)]
struct Version {
    v: u32,
    value: Zeroizing<String>,
    created: String,
    source: String,
}

#[derive(Serialize, Deserialize)]
struct Entry {
    note: String,
    /// Newest first, never empty.
    versions: Vec<Version>,
}

#[derive(Deserialize)]
struct Plain {
    secrets: BTreeMap<String, Entry>,
}

#[derive(Serialize)]
struct PlainRef<'a> {
    secrets: &'a BTreeMap<String, Entry>,
}

/// Every kept value ≥ [`MIN_REDACT`] chars and its base64, each paired with
/// the placeholder that names it.
struct Scrubber {
    ac: AhoCorasick,
    placeholders: Vec<String>,
}

struct Inner {
    entries: BTreeMap<String, Entry>,
    /// mtime of the file these entries came from (or that we last wrote).
    mtime: Option<SystemTime>,
    scrubber: Option<Scrubber>,
}

/// The encrypted, versioned store. `Send + Sync`; every method is sync.
pub struct Vault {
    path: PathBuf,
    key: Zeroizing<[u8; 32]>,
    key_store: &'static str,
    inner: RwLock<Inner>,
}

impl std::fmt::Debug for Vault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vault")
            .field("path", &self.path)
            .field("key_store", &self.key_store)
            .finish_non_exhaustive()
    }
}

impl Vault {
    /// Open or create `<data_dir>/secrets.vault`. Imports and unlinks
    /// `<data_dir>/credentials.yaml` (flat `name: value` YAML) if present,
    /// source `import`. Err only when the key store fails or the file is
    /// corrupt / undecryptable.
    pub fn open(data_dir: &Path, key_store: KeyStore) -> anyhow::Result<Vault> {
        std::fs::create_dir_all(data_dir).with_context(|| data_dir.display().to_string())?;
        let (key, key_store) = crate::key::load_or_create(data_dir, key_store)?;
        let key = Zeroizing::new(key);
        let path = data_dir.join(FILE);
        let entries = load(&path, &key)?.unwrap_or_default();
        let vault = Vault {
            inner: RwLock::new(Inner {
                scrubber: scrubber(&entries),
                entries,
                mtime: mtime(&path),
            }),
            path,
            key,
            key_store,
        };
        vault.import(&data_dir.join("credentials.yaml"));
        Ok(vault)
    }

    /// `"keychain"` or `"file"`.
    pub fn key_store(&self) -> &'static str {
        self.key_store
    }

    /// `back` = 0 current, 1 previous, …; `None` when the name or version
    /// does not exist.
    pub fn get(&self, name: &str, back: u8) -> Option<Zeroizing<String>> {
        self.synced()
            .entries
            .get(name)?
            .versions
            .get(usize::from(back))
            .map(|v| v.value.clone())
    }

    /// Every secret, sorted by name. Never a value.
    pub fn list(&self) -> Vec<SecretInfo> {
        self.synced()
            .entries
            .iter()
            .map(|(name, e)| info(name, e))
            .collect()
    }

    /// Push a new version (`v` monotonic per name, ≤ [`KEEP`] kept). The same
    /// value as the current version returns the current info and writes
    /// nothing. `note: None` keeps the old note (`""` for a new name).
    pub fn set(
        &self,
        name: &str,
        value: &str,
        note: Option<&str>,
        source: &str,
    ) -> Result<SecretInfo, String> {
        if !valid_name(name) {
            return Err(format!(
                "invalid secret name {name:?}; names are [A-Za-z0-9_.-]{{1,64}}"
            ));
        }
        let mut g = self.synced_mut();
        let entry = g.entries.entry(name.to_owned()).or_insert_with(|| Entry {
            note: String::new(),
            versions: Vec::new(),
        });
        if entry
            .versions
            .first()
            .is_some_and(|cur| cur.value.as_str() == value)
        {
            return Ok(info(name, entry));
        }
        if let Some(note) = note {
            entry.note = note.to_owned();
        }
        push(entry, Zeroizing::new(value.to_owned()), source.to_owned());
        let out = info(name, entry);
        self.save(&mut g)?;
        Ok(out)
    }

    /// Copy version `back` (1..=4) to the top as a new version, source `restore vN`.
    pub fn restore(&self, name: &str, back: u8) -> Result<SecretInfo, String> {
        if !(1..=4).contains(&back) {
            return Err(format!("back: {back} is not in 1..=4"));
        }
        let mut g = self.synced_mut();
        let entry = g.entries.get_mut(name).ok_or_else(|| unknown(name))?;
        let old = entry.versions.get(usize::from(back)).ok_or_else(|| {
            format!(
                "{name} has {} versions; back: {back} does not exist",
                entry.versions.len()
            )
        })?;
        let (value, source) = (old.value.clone(), format!("restore v{}", old.v));
        push(entry, value, source);
        let out = info(name, entry);
        self.save(&mut g)?;
        Ok(out)
    }

    /// Remove the name and every version.
    pub fn delete(&self, name: &str) -> Result<(), String> {
        let mut g = self.synced_mut();
        if g.entries.remove(name).is_none() {
            return Err(unknown(name));
        }
        self.save(&mut g)
    }

    /// Replace every `{{secret:NAME}}` / `{{secret:NAME:-N}}` in every string
    /// leaf of `args` (keys untouched); returns the count. Err on the first
    /// unknown name or version — `args` may then be partially expanded, so
    /// the caller must not dispatch it.
    pub fn expand(&self, args: &mut serde_json::Value) -> Result<usize, String> {
        static RE: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(PLACEHOLDER).expect("PLACEHOLDER is a valid regex"));
        let g = self.synced();
        let mut n = 0;
        strings(args, &mut |s| {
            if !s.contains("{{secret:") {
                return Ok(());
            }
            let mut err = None;
            let out = RE.replace_all(s, |c: &regex::Captures| -> String {
                let back = c.get(2).map_or(0, |m| m.as_str().parse().unwrap_or(0));
                match lookup(&g.entries, &c[1], back) {
                    Ok(value) => {
                        n += 1;
                        value.to_owned()
                    }
                    Err(e) => {
                        err.get_or_insert(e);
                        String::new()
                    }
                }
            });
            if let Some(e) = err {
                return Err(e);
            }
            if let Cow::Owned(o) = out {
                *s = o;
            }
            Ok(())
        })?;
        Ok(n)
    }

    /// Every kept version of every value ≥ [`MIN_REDACT`] chars, and its
    /// standard base64, → `{{secret:NAME}}` (current) or `{{secret:NAME:-N}}`.
    /// One leftmost-longest pass; borrowed when nothing matches.
    pub fn redact<'a>(&self, text: &'a str) -> Cow<'a, str> {
        scrub(self.synced().scrubber.as_ref(), text)
    }

    /// [`Vault::redact`] over every string in a JSON value.
    pub fn redact_value(&self, v: &mut serde_json::Value) {
        let g = self.synced();
        let Some(sc) = g.scrubber.as_ref() else {
            return;
        };
        let _ = strings(v, &mut |s| {
            if let Cow::Owned(o) = scrub(Some(sc), s) {
                *s = o;
            }
            Ok(())
        });
    }

    /// Read view, reloaded first if the file's mtime moved.
    fn synced(&self) -> RwLockReadGuard<'_, Inner> {
        let disk = mtime(&self.path);
        {
            let g = self.inner.read().unwrap_or_else(PoisonError::into_inner);
            if g.mtime == disk {
                return g;
            }
        }
        drop(self.synced_mut());
        self.inner.read().unwrap_or_else(PoisonError::into_inner)
    }

    /// Write view, reloaded first if the file's mtime moved.
    fn synced_mut(&self) -> RwLockWriteGuard<'_, Inner> {
        let mut g = self.inner.write().unwrap_or_else(PoisonError::into_inner);
        let disk = mtime(&self.path);
        if g.mtime != disk {
            self.reload(&mut g, disk);
        }
        g
    }

    /// Replace memory with the file's contents. A file that no longer
    /// decrypts keeps the old entries (and stops retrying until it changes again).
    fn reload(&self, g: &mut Inner, disk: Option<SystemTime>) {
        match load(&self.path, &self.key) {
            Ok(entries) => {
                g.entries = entries.unwrap_or_default();
                g.scrubber = scrubber(&g.entries);
            }
            Err(e) => tracing::warn!("{e:#}; keeping the secrets loaded before"),
        }
        g.mtime = disk;
    }

    /// Encrypt + atomically write `g.entries`; on failure memory reverts to
    /// the file so the two never disagree.
    fn save(&self, g: &mut Inner) -> Result<(), String> {
        let plain = Zeroizing::new(
            serde_json::to_vec(&PlainRef {
                secrets: &g.entries,
            })
            .map_err(|e| e.to_string())?,
        );
        let nonce: [u8; NONCE] = rand::random();
        let sealed = XChaCha20Poly1305::new(&(*self.key).into())
            .encrypt(
                &nonce.into(),
                Payload {
                    msg: &plain,
                    aad: MAGIC,
                },
            )
            .map_err(|_| "encrypting the vault failed".to_owned())?;
        let mut bytes = Vec::with_capacity(MAGIC.len() + NONCE + sealed.len());
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&nonce);
        bytes.extend_from_slice(&sealed);
        match atomic_write(&self.path, &bytes) {
            Ok(()) => {
                g.mtime = mtime(&self.path);
                g.scrubber = scrubber(&g.entries);
                Ok(())
            }
            Err(e) => {
                self.reload(g, mtime(&self.path));
                Err(format!("writing {}: {e}", self.path.display()))
            }
        }
    }

    /// 37 §3.6: move `credentials.yaml` into the vault and unlink it. A parse
    /// or write failure leaves the file where it is.
    fn import(&self, path: &Path) {
        let Ok(text) = std::fs::read_to_string(path).map(Zeroizing::new) else {
            return;
        };
        let rows = match serde_yaml::from_str::<Option<HashMap<String, String>>>(&text) {
            Ok(rows) => rows.unwrap_or_default(),
            Err(e) => {
                tracing::warn!("{}: {e}; not imported", path.display());
                return;
            }
        };
        let (mut n, mut failed) = (0, false);
        for (name, value) in rows.iter().filter(|(_, v)| !v.trim().is_empty()) {
            match self.set(
                name,
                value.trim(),
                Some("imported from credentials.yaml"),
                "import",
            ) {
                Ok(_) => n += 1,
                Err(e) => {
                    failed = true;
                    tracing::warn!("credentials.yaml: {name}: {e}");
                }
            }
        }
        if failed {
            tracing::warn!(
                "{}: some keys were not imported; the file was kept",
                path.display()
            );
            return;
        }
        match std::fs::remove_file(path) {
            Ok(()) => tracing::info!(
                "imported {n} keys from credentials.yaml into the vault; the file was removed"
            ),
            Err(e) => tracing::warn!(
                "imported {n} keys from credentials.yaml but could not remove it: {e}"
            ),
        }
    }
}

fn unknown(name: &str) -> String {
    format!("unknown secret {name}; secret_list shows the names")
}

/// The value `back` versions behind the current, with the placeholder-style error texts.
fn lookup<'a>(
    entries: &'a BTreeMap<String, Entry>,
    name: &str,
    back: usize,
) -> Result<&'a str, String> {
    let e = entries.get(name).ok_or_else(|| unknown(name))?;
    e.versions
        .get(back)
        .map(|v| v.value.as_str())
        .ok_or_else(|| {
            format!(
                "{name} has {} versions; :-{back} does not exist",
                e.versions.len()
            )
        })
}

fn push(entry: &mut Entry, value: Zeroizing<String>, source: String) {
    let v = entry.versions.first().map_or(1, |cur| cur.v + 1);
    let created = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    entry.versions.insert(
        0,
        Version {
            v,
            value,
            created,
            source,
        },
    );
    entry.versions.truncate(KEEP);
}

fn info(name: &str, e: &Entry) -> SecretInfo {
    let cur = e.versions.first();
    SecretInfo {
        name: name.to_owned(),
        note: e.note.clone(),
        source: "local".to_owned(),
        current: cur.map_or(0, |c| c.v),
        versions: e
            .versions
            .iter()
            .map(|v| VersionInfo {
                v: v.v,
                created: v.created.clone(),
                source: v.source.clone(),
            })
            .collect(),
        updated: cur.map(|c| c.created.clone()).unwrap_or_default(),
    }
}

/// Visit every string leaf (object keys excluded); stops at the first Err.
fn strings(
    v: &mut serde_json::Value,
    f: &mut impl FnMut(&mut String) -> Result<(), String>,
) -> Result<(), String> {
    match v {
        serde_json::Value::String(s) => f(s),
        serde_json::Value::Array(a) => a.iter_mut().try_for_each(|v| strings(v, f)),
        serde_json::Value::Object(o) => o.values_mut().try_for_each(|v| strings(v, f)),
        _ => Ok(()),
    }
}

fn scrub<'a>(sc: Option<&Scrubber>, text: &'a str) -> Cow<'a, str> {
    match sc {
        Some(sc) if sc.ac.find(text).is_some() => {
            Cow::Owned(sc.ac.replace_all(text, &sc.placeholders))
        }
        _ => Cow::Borrowed(text),
    }
}

/// Newest version first per name and each text once, so a value two
/// versions share (a restore) reports the current placeholder.
fn scrubber(entries: &BTreeMap<String, Entry>) -> Option<Scrubber> {
    let mut patterns: Vec<Zeroizing<String>> = Vec::new();
    let mut placeholders = Vec::new();
    for (name, e) in entries {
        for (i, ver) in e.versions.iter().enumerate() {
            if ver.value.chars().count() < MIN_REDACT {
                continue;
            }
            let ph = if i == 0 {
                format!("{{{{secret:{name}}}}}")
            } else {
                format!("{{{{secret:{name}:-{i}}}}}")
            };
            for text in [
                ver.value.clone(),
                Zeroizing::new(B64.encode(ver.value.as_bytes())),
            ] {
                if !patterns.contains(&text) {
                    patterns.push(text);
                    placeholders.push(ph.clone());
                }
            }
        }
    }
    if patterns.is_empty() {
        return None;
    }
    let ac = AhoCorasickBuilder::new()
        .match_kind(MatchKind::LeftmostLongest)
        .build(&patterns);
    match ac {
        Ok(ac) => Some(Scrubber { ac, placeholders }),
        Err(e) => {
            tracing::warn!("secrets: building the redaction automaton failed: {e}");
            None
        }
    }
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// `None` = no file yet.
fn load(path: &Path, key: &[u8; 32]) -> anyhow::Result<Option<BTreeMap<String, Entry>>> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let Some(rest) = bytes.strip_prefix(MAGIC).filter(|r| r.len() > NONCE) else {
        anyhow::bail!("{} is not a wth secrets vault", path.display());
    };
    let (nonce, sealed) = rest.split_at(NONCE);
    let nonce: [u8; NONCE] = nonce.try_into().expect("split_at(NONCE)");
    let plain = XChaCha20Poly1305::new(&(*key).into())
        .decrypt(
            &nonce.into(),
            Payload {
                msg: sealed,
                aad: MAGIC,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| {
            anyhow::anyhow!(
                "cannot decrypt {}: wrong key or corrupt file",
                path.display()
            )
        })?;
    let plain: Plain = serde_json::from_slice(&plain)
        .with_context(|| format!("{}: bad vault contents", path.display()))?;
    Ok(Some(plain.secrets))
}

/// `<path>.tmp` at mode 0600, then rename over `path`.
fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    let mut open = std::fs::OpenOptions::new();
    open.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        open.mode(0o600);
    }
    open.open(&tmp).and_then(|mut f| f.write_all(bytes))?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(dir: &Path) -> Vault {
        Vault::open(dir, KeyStore::File).expect("open")
    }

    #[test]
    fn round_trip_and_tamper() {
        let dir = tempfile::tempdir().unwrap();
        let v = open(dir.path());
        v.set("gh", "ghp_0123456789", Some("GitHub PAT"), "user")
            .unwrap();
        drop(v);
        let v = open(dir.path());
        assert_eq!(v.get("gh", 0).unwrap().as_str(), "ghp_0123456789");
        assert_eq!(v.list()[0].note, "GitHub PAT");
        assert_eq!(v.key_store(), "file");
        drop(v);

        let path = dir.path().join(FILE);
        let mut bytes = std::fs::read(&path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0x55;
        std::fs::write(&path, bytes).unwrap();
        assert!(Vault::open(dir.path(), KeyStore::File).is_err());
    }

    #[test]
    fn versions_keep_five_and_v_is_monotonic() {
        let dir = tempfile::tempdir().unwrap();
        let v = open(dir.path());
        for i in 1..=10 {
            let info = v
                .set("k", &format!("value-number-{i}"), None, "shell")
                .unwrap();
            assert_eq!(info.current, i);
        }
        let info = &v.list()[0];
        assert_eq!(info.versions.len(), KEEP);
        assert_eq!(
            info.versions.iter().map(|x| x.v).collect::<Vec<_>>(),
            [10, 9, 8, 7, 6]
        );
        assert_eq!(v.get("k", 4).unwrap().as_str(), "value-number-6");
        assert!(v.get("k", 5).is_none());
        assert!(v.get("nope", 0).is_none());

        // Same value: no new version.
        let same = v.set("k", "value-number-10", None, "shell").unwrap();
        assert_eq!(same.current, 10);
        assert_eq!(v.list()[0].versions.len(), KEEP);

        // Restore copies, keeps history, never reuses a number.
        let r = v.restore("k", 2).unwrap();
        assert_eq!(r.current, 11);
        assert_eq!(r.versions[0].source, "restore v8");
        assert_eq!(v.get("k", 0).unwrap().as_str(), "value-number-8");
        assert_eq!(v.get("k", 1).unwrap().as_str(), "value-number-10");
        assert_eq!(v.restore("k", 5).unwrap_err(), "back: 5 is not in 1..=4");
        v.delete("k").unwrap();
        assert_eq!(
            v.restore("k", 1).unwrap_err(),
            "unknown secret k; secret_list shows the names"
        );
        v.set("two", "a", None, "u").unwrap();
        v.set("two", "b", None, "u").unwrap();
        assert_eq!(
            v.restore("two", 3).unwrap_err(),
            "two has 2 versions; back: 3 does not exist"
        );
        assert!(v.delete("two").is_ok());
        assert!(v.delete("two").is_err());
        assert!(v.list().is_empty());
    }

    #[test]
    fn expand_placeholders() {
        let dir = tempfile::tempdir().unwrap();
        let v = open(dir.path());
        v.set("tok", "old-token-value", None, "user").unwrap();
        v.set("tok", "new-token-value", None, "user").unwrap();
        let mut args = serde_json::json!({
            "command": "curl -H 'Authorization: Bearer {{secret:tok}}' -H 'X-Old: {{secret:tok:-1}}'",
            "nested": {"list": ["{{secret:tok}}", 7, null], "{{secret:tok}}": "key untouched"},
            "plain": "nothing here"
        });
        assert_eq!(v.expand(&mut args).unwrap(), 3);
        assert_eq!(
            args["command"],
            "curl -H 'Authorization: Bearer new-token-value' -H 'X-Old: old-token-value'"
        );
        assert_eq!(args["nested"]["list"][0], "new-token-value");
        assert!(args["nested"].get("{{secret:tok}}").is_some());

        let mut bad = serde_json::json!({"a": "{{secret:nope}}"});
        assert_eq!(
            v.expand(&mut bad).unwrap_err(),
            "unknown secret nope; secret_list shows the names"
        );
        let mut bad = serde_json::json!({"a": "{{secret:tok:-3}}"});
        assert_eq!(
            v.expand(&mut bad).unwrap_err(),
            "tok has 2 versions; :-3 does not exist"
        );
        let mut none = serde_json::json!({"a": "{{secret:}}", "b": 1});
        assert_eq!(v.expand(&mut none).unwrap(), 0);
    }

    #[test]
    fn redact_all_versions_and_base64() {
        let dir = tempfile::tempdir().unwrap();
        let v = open(dir.path());
        v.set("tok", "first-secret-value", None, "user").unwrap();
        v.set("tok", "second-secret-value", None, "user").unwrap();
        v.set("pin", "1234", None, "user").unwrap();
        let text = format!(
            "cur=second-secret-value old=first-secret-value b64={} pin=1234",
            B64.encode("second-secret-value")
        );
        assert_eq!(
            v.redact(&text),
            "cur={{secret:tok}} old={{secret:tok:-1}} b64={{secret:tok}} pin=1234"
        );
        assert!(matches!(v.redact("nothing to see"), Cow::Borrowed(_)));

        let mut val = serde_json::json!({"out": ["second-secret-value"], "n": 1});
        v.redact_value(&mut val);
        assert_eq!(val["out"][0], "{{secret:tok}}");

        // A restored value lives in two versions; the current placeholder wins.
        v.restore("tok", 1).unwrap();
        assert_eq!(v.redact("first-secret-value"), "{{secret:tok}}");
    }

    #[test]
    fn imports_credentials_yaml_once() {
        let dir = tempfile::tempdir().unwrap();
        let yaml = dir.path().join("credentials.yaml");
        std::fs::write(
            &yaml,
            "# keys\nanthropic: sk-ant-abc\nopenai: \"\"\nbrave: brv-1\n",
        )
        .unwrap();
        let v = open(dir.path());
        assert!(!yaml.exists());
        let list = v.list();
        assert_eq!(
            list.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            ["anthropic", "brave"]
        );
        assert_eq!(list[0].versions[0].source, "import");
        assert_eq!(list[0].note, "imported from credentials.yaml");
        assert_eq!(v.get("anthropic", 0).unwrap().as_str(), "sk-ant-abc");
        drop(v);
        let v = open(dir.path());
        assert_eq!(v.list()[0].versions.len(), 1);

        // Unparsable: left alone, nothing imported.
        std::fs::write(&yaml, "not: [valid\n").unwrap();
        let v2 = open(dir.path());
        assert!(yaml.exists());
        assert_eq!(v2.list().len(), 2);
    }

    #[test]
    fn names() {
        assert!(valid_name("GITHUB_TOKEN"));
        assert!(valid_name("a.b-c"));
        assert!(!valid_name(""));
        assert!(!valid_name("bitwarden/github"));
        assert!(!valid_name(&"x".repeat(65)));
        assert!(valid_name(&"x".repeat(64)));
        let dir = tempfile::tempdir().unwrap();
        assert!(open(dir.path()).set("a/b", "v", None, "u").is_err());
    }

    #[test]
    fn second_instance_sees_writes() {
        let dir = tempfile::tempdir().unwrap();
        let a = open(dir.path());
        a.set("seed", "seed-value", None, "u").unwrap();
        let b = open(dir.path());
        assert!(b.get("fresh", 0).is_none());
        a.set("fresh", "fresh-value-123", None, "u").unwrap();
        // Make the mtime move on filesystems with coarse timestamps.
        let f = std::fs::File::options()
            .write(true)
            .open(dir.path().join(FILE))
            .unwrap();
        f.set_modified(SystemTime::now() + std::time::Duration::from_secs(2))
            .unwrap();
        assert_eq!(b.get("fresh", 0).unwrap().as_str(), "fresh-value-123");
        assert_eq!(b.redact("x fresh-value-123 y"), "x {{secret:fresh}} y");
    }
}
