//! The secrets hook (37 §3.4): where `{{secret:NAME}}` / `{{secret:NAME:-N}}`
//! placeholders in tool arguments become values, and where values in tool
//! output become placeholders again.
//!
//! The vault itself lives in the `secrets` plugin, above this crate; it calls
//! [`install`] once at start. Until then, and when the plugin is off, every
//! function here is the identity. A process global rather than a message
//! because the readers — the registry, jobs, `mcp::expand` and the python
//! bridge — are four actors across two crates sharing one immutable handle set
//! once at start, and a registration message to each would be four edges and
//! a race with the first tool call; `tracing::subscriber::set_global_default`
//! is the shape.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::ops::Range;
use std::sync::{Arc, OnceLock};

use futures::future::BoxFuture;
use runtime::SessionId;
use zeroize::Zeroizing;

use crate::ToolOutput;

/// 221 §3.3: which way a call uses an external connection. Each is one always-ask entry per
/// connection, `secret:<backend>/<conn>:<dir>` (D6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Direction {
    /// A placeholder expands its value, or a tool reads metadata.
    Read,
    /// A value is created or replaced in the manager.
    Write,
    /// An entry is deleted (D13).
    Delete,
    /// A Bitwarden Send hands a secret out (§12).
    Send,
}

impl Direction {
    /// The directions a `backend` has: `send` is Bitwarden's only.
    pub fn of(backend: &str) -> &'static [Direction] {
        if backend == "bw" {
            &[Self::Read, Self::Write, Self::Delete, Self::Send]
        } else {
            &[Self::Read, Self::Write, Self::Delete]
        }
    }
}

impl fmt::Display for Direction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Delete => "delete",
            Self::Send => "send",
        })
    }
}

/// One external connection a call touches, and how (221 §3.3).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ExternalAccess {
    /// `aws` | `bw`.
    pub backend: &'static str,
    /// The connection name.
    pub conn: String,
    /// Read, write, delete or send.
    pub dir: Direction,
}

impl ExternalAccess {
    /// The 219 entry that makes this access always ask: `secret:<backend>/<conn>:<dir>`.
    pub fn always_ask_key(&self) -> String {
        always_ask_key(self.backend, &self.conn, self.dir)
    }
}

/// `secret:<backend>/<conn>:<dir>` (221 D6).
pub fn always_ask_key(backend: &str, conn: &str, dir: Direction) -> String {
    format!("secret:{backend}/{conn}:{dir}")
}

/// The always-ask entries a new connection is seeded with (D6, D20): one per direction.
pub fn ask_keys(backend: &str, conn: &str) -> Vec<String> {
    Direction::of(backend)
        .iter()
        .map(|d| always_ask_key(backend, conn, *d))
        .collect()
}

/// Add [`ask_keys`] to the harness-wide always-ask set (`tools.always_ask`), before the
/// connection is registered, so it is never un-gated (221 §6.3). Entries already there stay.
pub async fn seed_ask(
    registry: &kameo::actor::ActorRef<crate::ToolRegistry>,
    backend: &str,
    conn: &str,
) -> Result<(), String> {
    for tool in ask_keys(backend, conn) {
        registry
            .ask(crate::registry::SetAlwaysAsk {
                session: None,
                tool,
                on: true,
            })
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Who resolves: the call's session and id (for the unlock card and the audit line).
#[derive(Clone, Debug)]
pub struct ResolveCx {
    /// The calling session.
    pub session: SessionId,
    /// The tool call id.
    pub call_id: String,
}

/// `(placeholder text) → value` for one call's external references, filled by
/// [`resolve_external`] and read by [`expand_with`]. Zeroized on drop; never Debug-printed.
#[derive(Default)]
pub struct Fetched(HashMap<String, Zeroizing<String>>);

impl Fetched {
    /// Keep `value` for `placeholder` (the whole `{{secret:…}}` text).
    pub fn insert(&mut self, placeholder: String, value: Zeroizing<String>) {
        self.0.insert(placeholder, value);
    }

    /// The value fetched for `placeholder`.
    pub fn get(&self, placeholder: &str) -> Option<&str> {
        self.0.get(placeholder).map(|v| v.as_str())
    }

    /// Whether `placeholder` was fetched.
    pub fn contains(&self, placeholder: &str) -> bool {
        self.0.contains_key(placeholder)
    }

    /// How many references were fetched.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Nothing fetched.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Fetched {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Fetched({} value(s))", self.0.len())
    }
}

/// What the registry, jobs and the python bridge need from a vault. One
/// implementation (the `secrets` plugin); `tools` cannot depend on it.
pub trait Secrets: Send + Sync {
    /// Replace every placeholder in every string leaf of `args` (keys untouched)
    /// with its value; returns the count. `Err` = unknown name or version, and
    /// `args` may be partially expanded — the whole call must fail.
    fn expand(&self, args: &mut serde_json::Value) -> Result<usize, String>;
    /// [`Secrets::expand`] that also replaces the external placeholders [`Secrets::resolve_external`]
    /// fetched; an external placeholder missing from `fetched` is an error. Without backends:
    /// `expand`, and any fetched value is an error.
    fn expand_with(
        &self,
        args: &mut serde_json::Value,
        fetched: &Fetched,
    ) -> Result<usize, String> {
        if fetched.is_empty() {
            self.expand(args)
        } else {
            Err("external secrets are not supported here".to_owned())
        }
    }
    /// Which external references `args` names, and in which direction this call uses them.
    /// Sync, no I/O: a parse. The registry ORs their always-ask entries into `always()` (221
    /// D10). `tool` lets the `secret_external_*` tools declare a write (their args name a
    /// destination, not a placeholder).
    fn external_access(&self, _tool: &str, _args: &serde_json::Value) -> Vec<ExternalAccess> {
        Vec::new()
    }
    /// Fetch every external reference in `args` (after the approval, before `expand_with`).
    /// Each value joins the scrub set before this returns. `Err` = unknown backend or
    /// connection, locked, not found, denied by the provider, timeout: the call fails whole.
    fn resolve_external<'a>(
        &'a self,
        _args: &'a serde_json::Value,
        _cx: ResolveCx,
    ) -> BoxFuture<'a, Result<Fetched, String>> {
        Box::pin(async { Ok(Fetched::default()) })
    }
    /// Every kept version of every value (and its base64) → its placeholder.
    /// `Cow::Borrowed` when nothing matched.
    fn redact<'a>(&self, text: &'a str) -> Cow<'a, str>;
    /// [`Secrets::redact`] over every string in a JSON value.
    fn redact_value(&self, v: &mut serde_json::Value);
    /// Byte ranges [`Secrets::redact`] would replace, ascending and
    /// non-overlapping. Nothing by default (51 §7.2).
    fn redact_spans(&self, _text: &str) -> Vec<Range<usize>> {
        Vec::new()
    }
    /// Longest literal [`Secrets::redact_spans`] can match, in bytes; `0` = none.
    fn max_literal_len(&self) -> usize {
        0
    }
    /// 225: `Some(text)` while the vault is sealed ("sky-cloud is sealed — unlock it …").
    /// Schedules defer their fires on it; the expansion itself fails with the same text.
    fn sealed(&self) -> Option<String> {
        None
    }
}

static INSTALLED: OnceLock<Arc<dyn Secrets>> = OnceLock::new();

/// Install the vault. Once per process; a second call is logged and ignored.
pub fn install(s: Arc<dyn Secrets>) {
    if INSTALLED.set(s).is_err() {
        tracing::warn!("secrets hook already installed; second install ignored");
    }
}

/// Whether any string leaf of `args` contains a placeholder. A cheap scan
/// that needs no vault: the gate uses it to raise the action to `Ask`.
pub fn mentions(args: &serde_json::Value) -> bool {
    match args {
        serde_json::Value::String(s) => s.contains("{{secret:"),
        serde_json::Value::Array(a) => a.iter().any(mentions),
        serde_json::Value::Object(o) => o.values().any(mentions),
        _ => false,
    }
}

/// 225: `Some(text)` while this engine's vault is sealed; `None` unsealed or without a vault.
pub fn sealed() -> Option<String> {
    INSTALLED.get().and_then(|s| s.sealed())
}

/// Expand placeholders in `args`; `Ok(0)` when nothing is installed.
pub fn expand(args: &mut serde_json::Value) -> Result<usize, String> {
    INSTALLED.get().map_or(Ok(0), |s| s.expand(args))
}

/// [`Secrets::expand_with`]; without a hook, `Ok(0)` when nothing was fetched.
pub fn expand_with(args: &mut serde_json::Value, fetched: &Fetched) -> Result<usize, String> {
    match INSTALLED.get() {
        Some(s) => s.expand_with(args, fetched),
        None if fetched.is_empty() => Ok(0),
        None => Err("external secrets are not supported here".to_owned()),
    }
}

/// [`Secrets::external_access`]; empty when nothing is installed.
pub fn external_access(tool: &str, args: &serde_json::Value) -> Vec<ExternalAccess> {
    INSTALLED
        .get()
        .map_or_else(Vec::new, |s| s.external_access(tool, args))
}

/// [`Secrets::resolve_external`]; nothing fetched when nothing is installed.
pub async fn resolve_external(args: &serde_json::Value, cx: ResolveCx) -> Result<Fetched, String> {
    match INSTALLED.get() {
        Some(s) => s.resolve_external(args, cx).await,
        None => Ok(Fetched::default()),
    }
}

/// Replace known values in `text` with their placeholders; identity when
/// nothing is installed.
pub fn redact(text: &str) -> Cow<'_, str> {
    INSTALLED
        .get()
        .map_or(Cow::Borrowed(text), |s| s.redact(text))
}

/// [`redact`] over every string in a JSON value; identity when nothing is installed.
pub fn redact_value(v: &mut serde_json::Value) {
    if let Some(s) = INSTALLED.get() {
        s.redact_value(v);
    }
}

/// Byte ranges [`redact`] would replace; empty when nothing is installed.
pub fn redact_spans(text: &str) -> Vec<Range<usize>> {
    INSTALLED
        .get()
        .map_or_else(Vec::new, |s| s.redact_spans(text))
}

/// Longest literal [`redact_spans`] can match; `0` when nothing is installed.
pub fn max_literal_len() -> usize {
    INSTALLED.get().map_or(0, |s| s.max_literal_len())
}

/// [`redact`] over `content` and every string in `structured`.
pub fn redact_output(out: &mut ToolOutput) {
    let Some(s) = INSTALLED.get() else { return };
    if let Cow::Owned(clean) = s.redact(&out.content) {
        out.content = clean;
    }
    if let Some(v) = out.structured.as_mut() {
        s.redact_value(v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mentions_scans_nested_string_leaves_only() {
        let args = serde_json::json!({
            "{{secret:key}}": 1,
            "list": [{"deep": "x {{secret:GITHUB}} y"}]
        });
        assert!(mentions(&args));
        assert!(!mentions(&serde_json::json!({"{{secret:key}}": ["plain"]})));
    }

    #[test]
    fn identity_when_nothing_installed() {
        let mut args = serde_json::json!({"a": "{{secret:X}}"});
        assert_eq!(expand(&mut args), Ok(0));
        assert!(matches!(redact("{{secret:X}} hunter22"), Cow::Borrowed(_)));
        let mut out = ToolOutput::ok("hunter22");
        redact_output(&mut out);
        assert_eq!(out.content, "hunter22");
    }
}
