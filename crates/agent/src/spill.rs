//! Tool-output cap with spill-to-disk (design doc 07 §6) and compaction
//! epoch files (36 §6.4).
//!
//! Output over the cap is written whole to `<dir>/<session>/<call_id>.txt`
//! and replaced by head (70 %) + marker naming the file + tail (30 %), so the
//! model's `read_file` is the retrieval tool. Applied once, at append time.
//!
//! Each compaction `n` leaves the folded turns verbatim in
//! `<dir>/<session>/epoch_<n>.txt` and the summary that replaced them in
//! `epoch_<n>.summary.md`; same directory, same 7-day sweep.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use genai::chat::{ChatMessage, ChatRole, ContentPart};
use runtime::SessionId;

/// Bound `content` to `max_chars` chars, spilling the full text under `dir`.
/// At or under the cap the text is returned unchanged. A failed spill logs
/// and falls back to head-only truncation with a marker that names no file.
pub fn cap(
    session: SessionId,
    call_id: &str,
    content: &str,
    max_chars: usize,
    dir: &Path,
) -> String {
    let total = content.chars().count();
    if total <= max_chars {
        return content.to_string();
    }
    let file = dir
        .join(session.to_string())
        .join(format!("{}.txt", sanitize(call_id)));
    let saved = std::fs::create_dir_all(file.parent().unwrap_or(dir))
        .and_then(|()| std::fs::write(&file, content))
        .and_then(|()| std::path::absolute(&file));
    let path = match saved {
        Ok(p) => Some(p.display().to_string()),
        Err(e) => {
            tracing::warn!(%session, call_id, "spill failed ({e}); truncating without a file");
            None
        }
    };
    let marker = |omitted: usize| match &path {
        Some(p) => format!(
            "\n[… {omitted} chars omitted; full output saved to {p} — read_file with offset/limit …]\n"
        ),
        None => format!("\n[… {omitted} chars omitted …]\n"),
    };
    // The marker's length depends on the omitted count, which depends on the
    // marker's length; iterate to the fixed point (digit count is monotone).
    let mut keep = max_chars;
    let marker = loop {
        let m = marker(total - keep);
        let k = max_chars.saturating_sub(m.chars().count());
        if k == keep {
            break m;
        }
        keep = k;
    };
    let head_chars = if path.is_some() { keep * 7 / 10 } else { keep };
    let tail_chars = keep - head_chars;
    let head_end = content
        .char_indices()
        .nth(head_chars)
        .map_or(content.len(), |(i, _)| i);
    let tail_start = match tail_chars {
        0 => content.len(),
        n => content.char_indices().nth_back(n - 1).map_or(0, |(i, _)| i),
    };
    let mut out = String::with_capacity(head_end + marker.len() + (content.len() - tail_start));
    out.push_str(&content[..head_end]);
    out.push_str(&marker);
    out.push_str(&content[tail_start..]);
    out
}

/// Write compaction `n`'s folded span as a transcript to
/// `<dir>/<session>/epoch_<n>.txt` and `head_body` to `epoch_<n>.summary.md`
/// (36 §6.4). Returns the absolute transcript path; `None` (logged) when a
/// write fails, so the head line simply omits the reference.
pub fn epoch(
    session: SessionId,
    n: u32,
    folded: &[ChatMessage],
    head_body: &str,
    dir: &Path,
) -> Option<PathBuf> {
    let sub = dir.join(session.to_string());
    let txt = sub.join(format!("epoch_{n}.txt"));
    let saved = std::fs::create_dir_all(&sub)
        .and_then(|()| std::fs::write(&txt, transcript(folded)))
        .and_then(|()| std::fs::write(sub.join(format!("epoch_{n}.summary.md")), head_body))
        .and_then(|()| std::path::absolute(&txt));
    match saved {
        Ok(p) => Some(p),
        Err(e) => {
            tracing::warn!(session = %session, "epoch {n} spill {}: {e}", txt.display());
            None
        }
    }
}

/// One block per message: `--- <role> [<i>] ---` then its parts, one per
/// line. Tool results as stored (already capped); binaries named, not embedded.
fn transcript(msgs: &[ChatMessage]) -> String {
    let mut out = String::new();
    for (i, m) in msgs.iter().enumerate() {
        let role = match m.role {
            ChatRole::System => "system",
            ChatRole::User => "user",
            ChatRole::Assistant => "assistant",
            ChatRole::Tool => "tool",
        };
        let _ = writeln!(out, "--- {role} [{i}] ---");
        for part in m.content.iter() {
            match part {
                ContentPart::Text(t) => out.push_str(&crate::diff_output::strip(t)),
                ContentPart::ToolCall(tc) => {
                    let _ = write!(out, "call {}({})", tc.fn_name, tc.fn_arguments);
                }
                ContentPart::ToolResponse(r) => {
                    let _ = write!(
                        out,
                        "result {}:\n{}",
                        r.call_id,
                        crate::diff_output::strip(&r.content)
                    );
                    if !r.images.is_empty() {
                        let _ = write!(out, "\n[image ×{}]", r.images.len());
                    }
                }
                ContentPart::Binary(b) => {
                    let _ = write!(
                        out,
                        "[binary {} {}]",
                        b.content_type,
                        b.name.as_deref().unwrap_or("")
                    );
                }
                other => out.push_str(&serde_json::to_string(other).unwrap_or_default()),
            }
            out.push('\n');
        }
    }
    out
}

/// Delete `dir/*/*` files older than `max_age`. Errors are ignored: a sweep
/// that misses a file today catches it tomorrow.
pub fn sweep(dir: &Path, max_age: Duration) {
    let Ok(sessions) = std::fs::read_dir(dir) else {
        return;
    };
    for file in sessions
        .flatten()
        .filter_map(|s| std::fs::read_dir(s.path()).ok())
        .flatten()
        .flatten()
    {
        let stale = file
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > max_age);
        if stale {
            let _ = std::fs::remove_file(file.path());
        }
    }
}

/// `call_id` as a safe file stem: `[A-Za-z0-9._-]`, anything else → `_`.
pub fn sanitize(call_id: &str) -> String {
    call_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn diff_carrier_readable_epoch_archive_contains_receipts_only() {
        use genai::chat::ToolResponse;
        let carried = crate::diff_output::carried("receipt");
        let messages = [
            ChatMessage::from(vec![ToolResponse::new("c", carried.clone())]),
            ChatMessage::user(carried),
        ];
        let output = transcript(&messages);
        assert!(output.contains("result c:\nreceipt\n"));
        assert!(!output.contains("theuth-"));
        assert!(!output.contains("metadata"));
    }

    use super::*;

    #[test]
    fn cap_spills_head_marker_tail() {
        let scratch = crate::TestDir::new("spill");
        let dir = scratch.0.join("spill");
        let session = SessionId::new();
        // Multibyte chars so char/byte boundaries differ.
        let content: String = (0..3000)
            .map(|i| if i % 7 == 0 { 'é' } else { 'a' })
            .collect();

        let same = cap(session, "call/1", &content, 3000, &dir);
        assert_eq!(same, content, "at the cap: unchanged");
        assert!(!dir.exists(), "no spill at the cap");

        let out = cap(session, "call/1", &content, 1000, &dir);
        let file = dir.join(session.to_string()).join("call_1.txt");
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            content,
            "full content on disk"
        );
        assert!(out.chars().count() <= 1000, "{} chars", out.chars().count());
        let start = out.find("\n[… ").unwrap();
        let end = out.find(" …]\n").unwrap() + " …]\n".len();
        assert!(
            out[start..end].contains(&std::path::absolute(&file).unwrap().display().to_string())
        );
        let head = &out[..start];
        let tail = &out[end..];
        assert!(content.starts_with(head) && content.ends_with(tail));
        assert!(
            head.chars().count() > tail.chars().count() * 2,
            "head ~70 %, tail ~30 %"
        );
        assert!(!tail.is_empty());
        let omitted: usize = out[start..end].split(' ').nth(1).unwrap().parse().unwrap();
        assert_eq!(omitted, 3000 - head.chars().count() - tail.chars().count());
    }

    #[test]
    fn epoch_writes_transcript_and_summary() {
        let scratch = crate::TestDir::new("spill");
        let dir = scratch.0.clone();
        let session = SessionId::new();
        let folded = [
            ChatMessage::user("what time is it"),
            ChatMessage::assistant(genai::chat::MessageContent::from_parts(vec![
                ContentPart::ToolCall(genai::chat::ToolCall {
                    call_id: "c1".into(),
                    fn_name: "clock".into(),
                    fn_arguments: serde_json::json!({"tz": "UTC"}),
                    thought_signatures: None,
                }),
            ])),
        ];
        let txt = epoch(session, 3, &folded, "## Summary\nasked the time", &dir).unwrap();
        assert!(txt.is_absolute() && txt.ends_with("epoch_3.txt"));
        let t = std::fs::read_to_string(&txt).unwrap();
        assert!(t.contains("--- user [0] ---\nwhat time is it\n"), "{t}");
        assert!(
            t.contains("--- assistant [1] ---\ncall clock({\"tz\":\"UTC\"})\n"),
            "{t}"
        );
        let summary = std::fs::read_to_string(txt.with_file_name("epoch_3.summary.md")).unwrap();
        assert_eq!(summary, "## Summary\nasked the time");
    }
}
