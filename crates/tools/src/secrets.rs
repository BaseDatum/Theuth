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
use std::ops::Range;
use std::sync::{Arc, OnceLock};

use crate::ToolOutput;

/// What the registry, jobs and the python bridge need from a vault. One
/// implementation (the `secrets` plugin); `tools` cannot depend on it.
pub trait Secrets: Send + Sync {
    /// Replace every placeholder in every string leaf of `args` (keys untouched)
    /// with its value; returns the count. `Err` = unknown name or version, and
    /// `args` may be partially expanded — the whole call must fail.
    fn expand(&self, args: &mut serde_json::Value) -> Result<usize, String>;
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

/// Expand placeholders in `args`; `Ok(0)` when nothing is installed.
pub fn expand(args: &mut serde_json::Value) -> Result<usize, String> {
    INSTALLED.get().map_or(Ok(0), |s| s.expand(args))
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
