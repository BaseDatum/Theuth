//! Tool-output cap with spill-to-disk (design doc 07 §6).
//!
//! Output over the cap is written whole to `<dir>/<session>/<call_id>.txt`
//! and replaced by head (70 %) + marker naming the file + tail (30 %), so the
//! model's `read_file` is the retrieval tool. Applied once, at append time.

use std::path::Path;
use std::time::Duration;

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
fn sanitize(call_id: &str) -> String {
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
    use super::*;

    #[test]
    fn cap_spills_head_marker_tail() {
        let dir = std::env::temp_dir().join(format!("wth-spill-{}", uuid::Uuid::new_v4()));
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

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
