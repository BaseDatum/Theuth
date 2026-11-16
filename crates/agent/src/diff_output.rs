//! Display-only diff payloads carried by the existing output string: no wire/schema migration.
use std::borrow::Cow;

const START: &str = "\n<theuth-diff-result>\n";
const END: &str = "\n</theuth-diff-result>";
pub(crate) const ACP_START: &str = "\n<theuth-acp-diff>\n";
pub(crate) const ACP_END: &str = "\n</theuth-acp-diff>";

/// Retain native structured diffs in the journal and Python child events, not just their receipts.
pub(crate) fn native<'a>(text: &'a str, structured: Option<&serde_json::Value>) -> Cow<'a, str> {
    let Some(data) = structured.filter(|v| {
        v.get("diff").is_some_and(serde_json::Value::is_string)
            || v.get("files")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|files| {
                    files
                        .iter()
                        .any(|f| f.get("diff").is_some_and(serde_json::Value::is_string))
                })
    }) else {
        return Cow::Borrowed(text);
    };
    Cow::Owned(format!("{text}{START}{data}{END}"))
}

/// Do not cut a diff carrier in half; native patches are tool-bounded and the UI caps ACP rows.
pub(crate) fn display(text: &str, max: usize) -> String {
    if let Some(i) = text.find(START).or_else(|| text.find(ACP_START)) {
        format!("{}{}", crate::frame::truncate(&text[..i], max), &text[i..])
    } else {
        crate::frame::truncate(text, max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn acp_native_diff_carrier_survives_display_cap() {
        let data = json!({"files": [{"path": "a.rs", "snapshot": "abcd1234", "diff": format!("--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-{}\n+new\n", "old".repeat(1000))}]});
        let full = native("moved lines", Some(&data));
        let shown = display(&full, 20);
        let payload = shown
            .split_once(START)
            .unwrap()
            .1
            .strip_suffix(END)
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(payload).unwrap(),
            data
        );
        assert!(shown.starts_with("moved lines"));
        assert_eq!(native("ordinary output", None), "ordinary output");
    }
}
