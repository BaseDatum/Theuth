//! Owner: E1 — redaction for the agent (design 171 §9.4): vault values exactly, then the
//! credential patterns. Applied to the agent's copy, `cell_output` and replicated text.

use std::borrow::Cow;
use std::sync::LazyLock;

use regex::Regex;

/// §9.4 credential table: `(pattern, replacement)`; `$1` keeps a key name where the table
/// says the value alone is masked.
static PATTERNS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    [
        (r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b", "[masked aws-key-id]"),
        (
            r#"((?i:aws_secret_access_key)|SecretAccessKey)(["' ]*[=:]["' ]*)[A-Za-z0-9/+=]{40}"#,
            "$1$2[masked aws-secret]",
        ),
        (
            r#"((?i:aws_session_token)|SessionToken)(["' ]*[=:]["' ]*)[A-Za-z0-9/+=]{100,}"#,
            "$1$2[masked aws-session-token]",
        ),
        (r"\beyJ[\w-]{10,}\.[\w-]{10,}\.[\w-]{10,}", "[masked jwt]"),
        (r"\bgh[pousr]_[A-Za-z0-9]{36,}", "[masked token]"),
        (r"\bxox[abposr]-[\w-]{10,}", "[masked token]"),
        (
            r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?-----END [A-Z ]*PRIVATE KEY-----",
            "[masked private key]",
        ),
    ]
    .into_iter()
    .map(|(src, mask)| {
        (
            Regex::new(src).unwrap_or_else(|e| unreachable!("§9.4 pattern {src}: {e}")),
            mask,
        )
    })
    .collect()
});

/// Mask vault values and credential patterns in `text`; returns the masked text and how many
/// values were masked (the footer's "2 values masked for theuth").
pub fn mask(text: &str, vault_values: &[String]) -> (String, u32) {
    let mut count = 0u32;
    let mut out = text.to_owned();
    let mut values: Vec<&str> = vault_values
        .iter()
        .map(String::as_str)
        .filter(|v| !v.is_empty())
        .collect();
    values.sort_by_key(|v| std::cmp::Reverse(v.len()));
    for v in values {
        let n = out.matches(v).count();
        if n > 0 {
            count += n as u32;
            out = out.replace(v, "[masked]");
        }
    }
    count += tools::secrets::redact_spans(&out).len() as u32;
    if let Cow::Owned(s) = tools::secrets::redact(&out) {
        out = s;
    }
    let (out, n) = patterns_only(&out);
    (out, count + n)
}

/// The §9.4 credential pattern table alone (no vault values); returns the masked text and the
/// number of occurrences masked.
pub fn patterns_only(text: &str) -> (String, u32) {
    let mut count = 0u32;
    let mut out = text.to_owned();
    for (re, mask) in PATTERNS.iter() {
        let n = re.find_iter(&out).count();
        if n > 0 {
            count += n as u32;
            out = re.replace_all(&out, *mask).into_owned();
        }
    }
    (out, count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vault_value_masked_and_counted() {
        let (out, n) = mask("token=hunter2 again hunter2", &["hunter2".to_owned()]);
        assert_eq!(out, "token=[masked] again [masked]");
        assert_eq!(n, 2);
    }

    #[test]
    fn aws_key_id() {
        let (out, n) = patterns_only("key AKIAABCDEFGHIJKLMNOP here");
        assert_eq!(out, "key [masked aws-key-id] here");
        assert_eq!(n, 1);
    }

    #[test]
    fn jwt() {
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
        let (out, n) = patterns_only(&format!("Authorization: Bearer {jwt}\n"));
        assert_eq!(out, "Authorization: Bearer [masked jwt]\n");
        assert_eq!(n, 1);
    }

    #[test]
    fn aws_secret_keeps_key_name() {
        let (out, n) =
            patterns_only("aws_secret_access_key = wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY");
        assert_eq!(out, "aws_secret_access_key = [masked aws-secret]");
        assert_eq!(n, 1);
        let (out, _) =
            patterns_only(r#""SecretAccessKey": "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY""#);
        assert_eq!(out, r#""SecretAccessKey": "[masked aws-secret]""#);
    }

    #[test]
    fn aws_session_token() {
        let tok = "A".repeat(120);
        let (out, n) = patterns_only(&format!("aws_session_token={tok}"));
        assert_eq!(out, "aws_session_token=[masked aws-session-token]");
        assert_eq!(n, 1);
    }

    #[test]
    fn github_and_slack_tokens() {
        let gh = format!("ghp_{}", "a".repeat(36));
        let (out, n) = patterns_only(&format!("{gh} and xoxb-1234567890-abc"));
        assert_eq!(out, "[masked token] and [masked token]");
        assert_eq!(n, 2);
    }

    #[test]
    fn private_key_block() {
        let text = "before\n-----BEGIN RSA PRIVATE KEY-----\nMIIE\nabc\n-----END RSA PRIVATE KEY-----\nafter";
        let (out, n) = patterns_only(text);
        assert_eq!(out, "before\n[masked private key]\nafter");
        assert_eq!(n, 1);
    }

    #[test]
    fn counts_every_occurrence_across_kinds() {
        let text = format!(
            "AKIAABCDEFGHIJKLMNOP ASIAABCDEFGHIJKLMNOP ghp_{} secret",
            "b".repeat(40)
        );
        let (out, n) = mask(&text, &["secret".to_owned()]);
        assert_eq!(n, 4);
        assert_eq!(
            out,
            "[masked aws-key-id] [masked aws-key-id] [masked token] [masked]"
        );
    }

    #[test]
    fn ordinary_text_untouched() {
        let text = "Previewing update (ir-prod1):\n    ~ 3 to update\n    4 unchanged\n";
        assert_eq!(mask(text, &[]), (text.to_owned(), 0));
        assert_eq!(patterns_only(text), (text.to_owned(), 0));
    }
}
