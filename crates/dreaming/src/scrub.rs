//! Secrets never reach a case, the ledger or the journal (design doc 136 §10):
//! every write runs the installed vault redactor (`tools::secrets::redact`,
//! the spill path's) and then a short list of token shapes the vault cannot
//! know — API keys (`sk-…`, `ghp_…`, `xox…`, `AKIA…`), bearer tokens, PEM
//! blocks — and neutralises the design-37 placeholder `{{secret:NAME}}` so a
//! stored excerpt can never be expanded by a later tool call.

use std::sync::LazyLock;

use regex::Regex;

static PATTERNS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    [
        (r"\{\{\s*secret:[^}]*\}\}", "[secret]"),
        (r"\bsk-[A-Za-z0-9_\-]{8,}", "[redacted]"),
        (r"\bgh[pousr]_[A-Za-z0-9]{20,}", "[redacted]"),
        (r"\bxox[abprs]-[A-Za-z0-9\-]{10,}", "[redacted]"),
        (r"\bAKIA[0-9A-Z]{16}\b", "[redacted]"),
        (r"(?i)\bbearer\s+[A-Za-z0-9._\-]{16,}", "bearer [redacted]"),
        (
            r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----",
            "[redacted private key]",
        ),
    ]
    .into_iter()
    .map(|(p, r)| (Regex::new(p).expect("static pattern"), r))
    .collect()
});

/// The scrubbed text.
pub fn scrub(text: &str) -> String {
    let mut out = tools::secrets::redact(text).into_owned();
    for (re, rep) in PATTERNS.iter() {
        if re.is_match(&out) {
            out = re.replace_all(&out, *rep).into_owned();
        }
    }
    out
}

/// Scrub every string inside a JSON value, in place.
pub fn scrub_value(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::String(s) => {
            let t = scrub(s);
            if t != *s {
                *s = t;
            }
        }
        serde_json::Value::Array(a) => a.iter_mut().for_each(scrub_value),
        serde_json::Value::Object(o) => o.values_mut().for_each(scrub_value),
        _ => {}
    }
}

/// The first `max` chars of `text` (char boundary safe), with `…` when cut.
pub fn clip(text: &str, max: usize) -> String {
    platform::text::ellipsize(text, max).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_and_tokens_go_and_plain_text_stays() {
        let t = "use {{secret:OPENAI_KEY}} or sk-abc123DEF456ghi789 and ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ12 \
                 with Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.abc.def; AKIAABCDEFGHIJKLMNOP; the sk- prefix alone stays";
        let s = scrub(t);
        assert!(!s.contains("OPENAI_KEY") && s.contains("[secret]"), "{s}");
        assert!(!s.contains("sk-abc123") && !s.contains("ghp_ABC"), "{s}");
        assert!(
            !s.contains("eyJhbGci") && !s.contains("AKIAABCDEFGHIJKLMNOP"),
            "{s}"
        );
        assert!(s.contains("the sk- prefix alone stays"), "{s}");
        assert_eq!(scrub("nothing here"), "nothing here");
        let mut v = serde_json::json!({"a": ["sk-abcdefghijkl"], "b": {"c": "{{secret:X}}"}});
        scrub_value(&mut v);
        assert_eq!(v["a"][0], "[redacted]");
        assert_eq!(v["b"]["c"], "[secret]");
    }
}
