//! The Claude Code client identity an Anthropic subscription (OAuth) request
//! claims: `User-Agent: claude-cli/<version> (external, cli)`, the
//! `anthropic-beta` list and the body's shape ([`wire`]). Anthropic refuses a version it deems too old (400
//! `claude_code_version_too_old`), so the version is live, not compiled in:
//!
//! - `<data_dir>/claude-code.json` (`version`, `betas`, `system`,
//!   `tool_prefix`, `system_replace`, `source`, `checked_at`, `pin`), re-read when its mtime changes (stat at most every
//!   [`RECHECK`]);
//! - [`Store::heal`]: the refusal's required version, persisted;
//! - [`start`]: npm's `latest` at startup and every 24 h, never a downgrade.
//!
//! The newest of the compiled default, the file and a learned value wins; a
//! file with `"pin": true` wins outright. `betas`, when set, replaces
//! [`DEFAULT_BETAS`]; `system`, `tool_prefix` and `system_replace`, when set,
//! replace [`wire::Wire`]'s defaults. See docs/design/cc-version-notes.md.

pub(crate) mod wire;

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, Instant, SystemTime};

use parking_lot::Mutex;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::supervisor::Auth;
use wire::Wire;

/// The npm release current when this was built (2026-11-18).
pub(crate) const DEFAULT_VERSION: Version = Version(2, 1, 327);
/// Betas a subscription token is accepted with; `oauth-2025-04-20` is the
/// one that turns a 403 into a 200.
pub(crate) const DEFAULT_BETAS: &str =
    "claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14";
const FILE: &str = "claude-code.json";
/// How stale the cached file may be before the next request stats it.
const RECHECK: Duration = Duration::from_secs(3);
const NPM_LATEST: &str = "https://registry.npmjs.org/@anthropic-ai/claude-code/latest";
const NPM_TIMEOUT: Duration = Duration::from_secs(5);
const REFRESH: Duration = Duration::from_secs(24 * 3600);
/// A heal raises to npm's `latest` too when it was fetched this recently.
const NPM_FRESH: Duration = Duration::from_secs(60);
/// The refusal's error code.
const TOO_OLD: &str = "claude_code_version_too_old";

/// `major.minor.patch`, ordered numerically.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Version(u32, u32, u32);

impl Version {
    pub(crate) fn parse(s: &str) -> Option<Self> {
        let mut it = s.trim().split('.').map(|p| p.parse().ok());
        let v = Version(it.next()??, it.next()??, it.next()??);
        it.next().is_none().then_some(v)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// `claude-code.json` as written and read.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Entry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    betas: Option<String>,
    /// The first system block; `""` sends none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    /// The tool-name prefix; `""` sends names as they are.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool_prefix: Option<String>,
    /// `[from, to]` pairs replaced in the system text, in order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    system_replace: Option<Vec<(String, String)>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    checked_at: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pin: bool,
}

/// The identity one request sends.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Identity {
    pub(crate) version: Version,
    pub(crate) betas: String,
    pub(crate) wire: Wire,
}

impl Identity {
    pub(crate) fn user_agent(&self) -> String {
        format!("claude-cli/{} (external, cli)", self.version)
    }
}

#[derive(Default)]
struct State {
    path: Option<PathBuf>,
    /// mtime of the file as last read; `None` = absent.
    stamp: Option<SystemTime>,
    /// Last stat; `None` = stat on the next request.
    checked: Option<Instant>,
    entry: Entry,
    /// Raised by a heal or npm in this process; survives a failed write.
    learned: Option<Version>,
    npm: Option<(Version, Instant)>,
}

impl State {
    /// Re-read the file if its mtime moved since the last read.
    fn sync(&mut self) {
        let Some(path) = &self.path else { return };
        if self.checked.is_some_and(|t| t.elapsed() < RECHECK) {
            return;
        }
        self.checked = Some(Instant::now());
        let stamp = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        if stamp == self.stamp {
            return;
        }
        self.stamp = stamp;
        self.entry = match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                tracing::warn!("{}: unreadable ({e}); using defaults", path.display());
                Entry::default()
            }),
            Err(_) => Entry::default(),
        };
    }

    fn file_version(&self) -> Option<Version> {
        self.entry.version.as_deref().and_then(Version::parse)
    }

    fn pinned(&self) -> Option<Version> {
        self.file_version().filter(|_| self.entry.pin)
    }

    fn version(&self) -> Version {
        self.pinned().unwrap_or_else(|| {
            [Some(DEFAULT_VERSION), self.file_version(), self.learned]
                .into_iter()
                .flatten()
                .max()
                .unwrap_or(DEFAULT_VERSION)
        })
    }

    fn wire(&self) -> Wire {
        let d = Wire::default();
        let e = &self.entry;
        Wire {
            system: e.system.clone().unwrap_or(d.system),
            tool_prefix: e.tool_prefix.clone().unwrap_or(d.tool_prefix),
            replace: e.system_replace.clone().unwrap_or(d.replace),
        }
    }

    /// Adopt `v` and persist it (keeping the file's other fields).
    fn raise(&mut self, v: Version, source: String) {
        self.learned = Some(v);
        let Some(path) = self.path.clone() else {
            return;
        };
        let entry = Entry {
            version: Some(v.to_string()),
            source: Some(source),
            checked_at: Some(chrono::Utc::now().to_rfc3339()),
            pin: false,
            ..self.entry.clone()
        };
        let tmp = path.with_extension("json.tmp");
        let written = serde_json::to_vec_pretty(&entry)
            .map_err(std::io::Error::other)
            .and_then(|bytes| std::fs::write(&tmp, bytes))
            .and_then(|()| std::fs::rename(&tmp, &path));
        match written {
            Ok(()) => {
                self.stamp = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
                self.entry = entry;
            }
            Err(e) => tracing::warn!("{}: not written ({e})", path.display()),
        }
    }
}

/// The live identity: one per process ([`global`]), a private one per test.
#[derive(Default)]
pub(crate) struct Store(Mutex<State>);

impl Store {
    #[cfg(test)]
    fn at(path: PathBuf) -> Self {
        Self(Mutex::new(State {
            path: Some(path),
            ..State::default()
        }))
    }

    /// The identity for the request about to be sent.
    pub(crate) fn current(&self) -> Identity {
        let mut st = self.0.lock();
        st.sync();
        Identity {
            version: st.version(),
            betas: st
                .entry
                .betas
                .clone()
                .filter(|b| !b.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_BETAS.to_owned()),
            wire: st.wire(),
        }
    }

    /// A provider error that is the too-old refusal raises the version to
    /// the one it requires (or npm's `latest` if fetched within
    /// [`NPM_FRESH`] and newer) and persists it, sourced `auto: <request_id>`.
    /// The raised version, or `None`: another error, a pin, or not newer.
    pub(crate) fn heal(&self, error: &str) -> Option<Version> {
        if !error.contains(TOO_OLD) {
            return None;
        }
        let required = capture(&REQUIRED, error)?;
        let mut st = self.0.lock();
        st.sync();
        if let Some(pin) = st.pinned() {
            if required > pin {
                tracing::warn!(
                    "Anthropic requires Claude Code {required}; claude-code.json pins {pin}: edit or unpin it"
                );
            }
            return None;
        }
        let current = st.version();
        if required <= current {
            return None;
        }
        let npm = st
            .npm
            .filter(|(_, at)| at.elapsed() < NPM_FRESH)
            .map(|(v, _)| v);
        let target = npm.map_or(required, |n| n.max(required));
        let id = REQUEST_ID
            .captures(error)
            .map_or("?", |c| c.get(1).map_or("?", |m| m.as_str()));
        tracing::info!(
            "Claude Code client {current} refused; now claiming {target} (request {id})"
        );
        st.raise(target, format!("auto: {id}"));
        Some(target)
    }

    /// Whether `error` refused a version older than the current one, so the
    /// same request is worth resending once.
    pub(crate) fn outdated(&self, error: &str) -> bool {
        error.contains(TOO_OLD)
            && capture(&SENT, error).is_some_and(|sent| sent < self.current().version)
    }

    /// npm's `latest`: remembered for a heal, adopted when newer and unpinned.
    fn offer_npm(&self, v: Version) {
        let mut st = self.0.lock();
        st.npm = Some((v, Instant::now()));
        st.sync();
        if st.pinned().is_none() && v > st.version() {
            tracing::info!("Claude Code client: npm latest {v}");
            st.raise(v, "npm".to_owned());
        }
    }
}

static REQUIRED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"version (\d+\.\d+\.\d+) or newer").expect("static regex"));
static SENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"Claude Code (\d+\.\d+\.\d+) does not").expect("static regex"));
static REQUEST_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""request_id"\s*:\s*"([^"]+)""#).expect("static regex"));

fn capture(re: &Regex, s: &str) -> Option<Version> {
    Version::parse(re.captures(s)?.get(1)?.as_str())
}

static GLOBAL: LazyLock<Store> = LazyLock::new(Store::default);

/// The process's identity; file-less until [`start`].
pub(crate) fn global() -> &'static Store {
    &GLOBAL
}

/// Bind the process identity to `<data_dir>/claude-code.json` and refresh it
/// from npm at startup and every 24 h while an Anthropic login exists
/// (failures ignored). The first call wins; later engines in the same
/// process (tests) share it.
pub(crate) fn start(data_dir: &Path, auth: Auth, shutdown: tokio_util::sync::CancellationToken) {
    {
        let mut st = GLOBAL.0.lock();
        if st.path.is_some() {
            return;
        }
        st.path = Some(data_dir.join(FILE));
        st.checked = None;
    }
    let http = match reqwest::Client::builder().timeout(NPM_TIMEOUT).build() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("claude-code: no HTTP client ({e}); npm refresh off");
            return;
        }
    };
    tokio::spawn(async move {
        loop {
            if auth.login("anthropic")
                && let Some(v) = npm_latest(&http).await
            {
                GLOBAL.offer_npm(v);
            }
            tokio::select! {
                _ = shutdown.cancelled() => return,
                _ = tokio::time::sleep(REFRESH) => {}
            }
        }
    });
}

async fn npm_latest(http: &reqwest::Client) -> Option<Version> {
    #[derive(Deserialize)]
    struct Latest {
        version: String,
    }
    let r = http
        .get(NPM_LATEST)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?;
    Version::parse(&r.json::<Latest>().await.ok()?.version)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sky's error, verbatim (2026-11-18), as `provider_error` renders it.
    const REFUSAL: &str = r#"Web stream error for model 'anthropic::claude-opus-5-5 (adapter: Anthropic)'. Status: 400
{"type":"error","error":{"type":"invalid_request_error","message":"Claude Code 2.1.257 does not support this model; version 2.1.280 or newer is required. Run 'claude update', or update the Claude desktop app, then try again.","details":{"error_code":"claude_code_version_too_old"}},"request_id":"req_011ChAVxq7rgFvFQmnk27n2u"}"#;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    fn store(dir: &tempfile::TempDir) -> Store {
        Store::at(dir.path().join(FILE))
    }

    fn write(dir: &tempfile::TempDir, json: serde_json::Value) {
        std::fs::write(dir.path().join(FILE), json.to_string()).unwrap();
    }

    fn read(dir: &tempfile::TempDir) -> Entry {
        serde_json::from_slice(&std::fs::read(dir.path().join(FILE)).unwrap()).unwrap()
    }

    #[test]
    fn parses_the_refusal() {
        assert_eq!(capture(&REQUIRED, REFUSAL), Some(v("2.1.280")));
        assert_eq!(capture(&SENT, REFUSAL), Some(v("2.1.257")));
        assert!(v("2.1.280") > v("2.1.257") && v("2.10.0") > v("2.9.99"));
        assert_eq!(Version::parse("2.1"), None);
        assert_eq!(Version::parse("2.1.3.4"), None);
    }

    #[test]
    fn user_agent_has_the_claude_cli_shape() {
        let dir = tempfile::tempdir().unwrap();
        let id = store(&dir).current();
        assert_eq!(id.user_agent(), "claude-cli/2.1.327 (external, cli)");
        assert_eq!(id.betas, DEFAULT_BETAS);
    }

    #[test]
    fn newest_wins_unless_pinned() {
        let dir = tempfile::tempdir().unwrap();
        // An older file never lowers the default.
        write(&dir, serde_json::json!({"version": "2.1.100"}));
        let s = store(&dir);
        assert_eq!(s.current().version, DEFAULT_VERSION);
        // npm older than current: no downgrade, file untouched.
        s.offer_npm(v("2.1.300"));
        assert_eq!(s.current().version, DEFAULT_VERSION);
        assert_eq!(read(&dir).version.as_deref(), Some("2.1.100"));
        // npm newer: adopted and persisted.
        s.offer_npm(v("2.2.0"));
        assert_eq!(s.current().version, v("2.2.0"));
        assert_eq!(read(&dir).source.as_deref(), Some("npm"));
        // A pin wins outright, even below the default; npm leaves it alone.
        let dir = tempfile::tempdir().unwrap();
        write(&dir, serde_json::json!({"version": "2.1.200", "pin": true}));
        let s = store(&dir);
        s.offer_npm(v("9.0.0"));
        assert_eq!(s.current().version, v("2.1.200"));
        assert!(read(&dir).pin);
    }

    #[test]
    fn file_edits_are_live() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(&dir);
        assert_eq!(s.current().version, DEFAULT_VERSION);
        write(
            &dir,
            serde_json::json!({"version": "3.0.1", "betas": "oauth-2025-04-20,x-1"}),
        );
        s.0.lock().checked = None; // skip RECHECK's wait
        let id = s.current();
        assert_eq!(id.user_agent(), "claude-cli/3.0.1 (external, cli)");
        assert_eq!(id.betas, "oauth-2025-04-20,x-1");
        assert_eq!(id.wire, Wire::default());
        write(
            &dir,
            serde_json::json!({"system": "", "tool_prefix": "x_", "system_replace": [["a", "b"]]}),
        );
        s.0.lock().checked = None;
        let w = s.current().wire;
        assert_eq!((w.system.as_str(), w.tool_prefix.as_str()), ("", "x_"));
        assert_eq!(w.replace, [("a".to_owned(), "b".to_owned())]);
    }

    /// The loop resends once when `outdated` after the heal; a second
    /// refusal of the same version is not newer and is not resent.
    #[test]
    fn heal_persists_and_allows_one_resend() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir,
            serde_json::json!({"betas": "oauth-2025-04-20", "tool_prefix": ""}),
        );
        let s = store(&dir);
        // Refused at the default (2.1.327) for 2.1.400.
        let refusal = REFUSAL
            .replace("2.1.257", "2.1.327")
            .replace("2.1.280", "2.1.400");
        assert_eq!(s.heal(&refusal), Some(v("2.1.400")));
        let e = read(&dir);
        assert_eq!(e.version.as_deref(), Some("2.1.400"));
        assert_eq!(
            e.source.as_deref(),
            Some("auto: req_011ChAVxq7rgFvFQmnk27n2u")
        );
        assert!(e.checked_at.is_some());
        assert_eq!(
            e.betas.as_deref(),
            Some("oauth-2025-04-20"),
            "a heal keeps betas"
        );
        assert_eq!(e.tool_prefix.as_deref(), Some(""), "and the wire fields");
        assert!(s.outdated(&refusal), "sent 2.1.327 < 2.1.400: resend");
        assert_eq!(
            s.current().user_agent(),
            "claude-cli/2.1.400 (external, cli)"
        );
        // The resend is refused again for 2.1.400: nothing newer, no resend.
        let again = REFUSAL
            .replace("2.1.257", "2.1.400")
            .replace("2.1.280", "2.1.400");
        assert_eq!(s.heal(&again), None);
        assert!(!s.outdated(&again));
        // Sky's error against the new default: already satisfied, but the
        // request sent the old version, so a resend is still worth it.
        assert_eq!(s.heal(REFUSAL), None);
        assert!(s.outdated(REFUSAL));
        assert_eq!(s.heal("Status: 400 overloaded"), None);
    }

    #[test]
    fn heal_takes_a_fresh_npm_latest_and_respects_a_pin() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(&dir);
        s.0.lock().npm = Some((v("2.1.500"), Instant::now()));
        let refusal = REFUSAL.replace("2.1.280", "2.1.400");
        assert_eq!(s.heal(&refusal), Some(v("2.1.500")));
        let dir = tempfile::tempdir().unwrap();
        write(&dir, serde_json::json!({"version": "2.1.257", "pin": true}));
        let s = store(&dir);
        assert_eq!(s.heal(REFUSAL), None);
        assert!(!s.outdated(REFUSAL));
        assert_eq!(s.current().version, v("2.1.257"));
    }
}
