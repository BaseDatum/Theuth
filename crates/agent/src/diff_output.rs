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

/// Complete display blocks, including only the newline introduced by the carrier.
fn carriers(text: &str) -> impl Iterator<Item = std::ops::Range<usize>> + '_ {
    let mut scan = 0;
    std::iter::from_fn(move || {
        loop {
            let (start, open, close, line_close) = [
                (
                    "<theuth-diff-result>",
                    "</theuth-diff-result>",
                    "\n</theuth-diff-result>",
                ),
                (
                    "<theuth-acp-diff>",
                    "</theuth-acp-diff>",
                    "\n</theuth-acp-diff>",
                ),
            ]
            .into_iter()
            .filter_map(|(open, close, line_close)| {
                text[scan..]
                    .match_indices(open)
                    .map(|(i, _)| scan + i)
                    .find(|&i| {
                        (i == 0 || text.as_bytes()[i - 1] == b'\n')
                            && text[i + open.len()..].starts_with('\n')
                    })
                    .map(|i| (i, open, close, line_close))
            })
            .min_by_key(|(i, _, _, _)| *i)?;
            let body = start + open.len();
            // Only framed carrier lines are metadata; marker literals in a receipt's code/diff stay data.
            let ending = text[body..].find(line_close).map(|i| body + i + 1);
            let Some(end) = ending else {
                scan = body;
                continue;
            };
            let from = start - usize::from(start > scan && text.as_bytes()[start - 1] == b'\n');
            scan = end + close.len();
            return Some(from..scan);
        }
    })
}

/// Remove every complete display carrier, leaving receipts byte-for-byte unchanged.
pub(crate) fn strip(text: &str) -> Cow<'_, str> {
    let mut blocks = carriers(text);
    let Some(first) = blocks.next() else {
        return Cow::Borrowed(text);
    };
    let mut clean = text[..first.start].to_owned();
    let mut kept = first.end;
    for block in blocks {
        clean.push_str(&text[kept..block.start]);
        kept = block.end;
    }
    clean.push_str(&text[kept..]);
    Cow::Owned(clean)
}

/// Reattach UI metadata after bounding the receipt, never through the model's truncation/spill path.
pub(crate) fn carry(mut receipt: String, original: &str) -> String {
    for block in carriers(original) {
        receipt.push_str(&original[block]);
    }
    receipt
}

/// Clean only model-facing copies; stored rows, UI events, images and call ids stay untouched.
pub(crate) fn strip_message(message: &mut genai::chat::ChatMessage) {
    use genai::chat::ContentPart;
    for part in message.content.iter_mut() {
        let text = match part {
            ContentPart::Text(text) => text,
            ContentPart::ToolResponse(result) => &mut result.content,
            _ => continue,
        };
        if let Cow::Owned(clean) = strip(text) {
            *text = clean;
        }
    }
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
pub(crate) fn carried(receipt: &str) -> String {
    let data = serde_json::json!({"diff": "metadata".repeat(2000)});
    format!(
        "{}{ACP_START}--- a/f\n+++ b/f\n@@ -1 +1 @@\n-old\n+new\n{ACP_END}",
        native(receipt, Some(&data))
    )
}
#[cfg(test)]
mod tests {
    #[test]
    fn diff_carrier_strip_preserves_receipts_and_ignores_quoted_closers() {
        for receipt in ["receipt", "é receipt\n", ""] {
            let input = carried(receipt);
            assert_eq!(strip(&input), receipt);
            assert_eq!(
                strip(&carry("bounded receipt".into(), &input)),
                "bounded receipt"
            );
        }
        assert!(matches!(strip("ordinary output"), Cow::Borrowed(_)));
        assert_eq!(
            strip(
                "before\n<theuth-diff-result>\n{}\n</theuth-diff-result>\nafter\n<theuth-acp-diff>\nx\n</theuth-acp-diff>\ntail"
            ),
            "before\nafter\ntail"
        );
        let incomplete = "keep\n<theuth-diff-result>\nunclosed";
        assert!(matches!(strip(incomplete), Cow::Borrowed(_)));
        let data = serde_json::json!({"diff": "quoted </theuth-diff-result> then data"});
        assert_eq!(strip(&native("receipt", Some(&data))), "receipt");
        let input = format!("{}\nbetween{}\nafter", carried("before"), carried(""));
        assert_eq!(strip(&input), "before\nbetween\nafter");
        let quoted = "+const OPEN: &str = \"<theuth-diff-result>\";\n+const CLOSE: &str = \"</theuth-diff-result>\";";
        assert!(matches!(strip(quoted), Cow::Borrowed(_)));
        assert_eq!(strip(&carried(quoted)), quoted);
    }

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
