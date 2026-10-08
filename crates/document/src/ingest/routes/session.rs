// 144-W1 owns this file.
//! The past-session route (§4.1 row 5): `session_read` (outline, then the turns in 64 KiB pages)
//! through the tool registry → `## Turn N — user | theuth` blocks, tool calls one line each. The
//! `agent` crate reaches `document` transitively, so the tool's text is the contract
//! (`agent/src/session_read.rs` `render`).

use std::time::Duration;

use crate::ingest::{Ctx, Extracted};

const DEADLINE: Duration = Duration::from_secs(60);

pub async fn extract(_ctx: &Ctx, session: &str) -> anyhow::Result<Extracted> {
    let reg = super::registry()
        .ok_or_else(|| anyhow::anyhow!("no tool registry: session_read is unavailable"))?;
    let outline = super::invoke(
        &reg,
        "session_read",
        serde_json::json!({ "session": session, "outline": true }),
        DEADLINE,
    )
    .await?
    .content;
    let (title, turns) = parse_header(&outline).ok_or_else(|| {
        anyhow::anyhow!(
            "session_read: unexpected header: {}",
            outline.lines().next().unwrap_or("")
        )
    })?;
    let mut pages = Vec::new();
    let mut from = 1;
    while from <= turns {
        let page = super::invoke(
            &reg,
            "session_read",
            serde_json::json!({ "session": session, "turns": [from, turns] }),
            DEADLINE,
        )
        .await?
        .content;
        let (body, cut_at) = split_cut(&page);
        let mut body = body.to_owned();
        let next = match cut_at {
            // The cut turn is incomplete: drop it and page from there — unless it is alone and
            // simply too big; then its head stays with the notice.
            Some(k) if k > from => {
                body = drop_turn_tail(&body, k);
                k
            }
            Some(k) => {
                body.push_str(&format!("\n[turn #{k} was cut at 64 KiB]\n"));
                k + 1
            }
            None => turns + 1,
        };
        pages.push(body);
        from = next;
    }
    let markdown = render(&pages.join("\n"));
    anyhow::ensure!(!markdown.trim().is_empty(), "the session has no turns");
    Ok(Extracted {
        bytes: Some(markdown.len() as u64),
        markdown,
        title: Some(title),
        meta: serde_json::json!({ "session": session, "turns": turns }),
        pages: None,
        duration_ms: None,
        blob: None,
        origin: None,
    })
}

/// `[session_read "title" <id>: N turns, …]` → (title, N).
pub fn parse_header(text: &str) -> Option<(String, usize)> {
    let line = text.lines().next()?.strip_prefix("[session_read ")?;
    let (title, rest) = if let Some(r) = line.strip_prefix('"') {
        let end = r.find("\" ")?;
        (r[..end].to_owned(), &r[end + 2..])
    } else {
        let (t, r) = line.split_once(' ')?;
        (t.to_owned(), r)
    };
    let n = rest
        .split_once(": ")?
        .1
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    Some((title, n))
}

/// Body without the `[session_read …]` header, and the turn a 64 KiB cut notice names.
fn split_cut(page: &str) -> (&str, Option<usize>) {
    let body = page.split_once('\n').map(|(_, b)| b).unwrap_or("");
    let marker = "\n[output cut at 64 KiB in turn #";
    match body.rfind(marker) {
        Some(i) => {
            let k = body[i + marker.len()..]
                .split(|c: char| !c.is_ascii_digit())
                .next()
                .and_then(|n| n.parse().ok());
            (&body[..i], k)
        }
        None => (body, None),
    }
}

fn drop_turn_tail(body: &str, turn: usize) -> String {
    let marker = format!("\n#{turn} user:\n");
    match body.find(&marker) {
        Some(i) => body[..i].to_owned(),
        None if body.starts_with(&marker[1..]) => String::new(),
        None => body.to_owned(),
    }
}

/// `#n user:` / `#n assistant:` / `#n tool x: …` lines → `## Turn n — user|theuth` blocks.
pub fn render(text: &str) -> String {
    let mut out = String::new();
    let mut open = false;
    for line in text.lines() {
        if let Some((n, rest)) = turn_marker(line) {
            if rest == "user:" {
                out.push_str(&format!("\n## Turn {n} — user\n\n"));
                open = true;
                continue;
            }
            if rest == "assistant:" {
                out.push_str(&format!("\n## Turn {n} — theuth\n\n"));
                open = true;
                continue;
            }
            if let Some(t) = rest.strip_prefix("tool ") {
                out.push_str(&format!("*tool {t}*\n\n"));
                continue;
            }
        }
        if !open && line.trim().is_empty() {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim_start().to_owned()
}

fn turn_marker(line: &str) -> Option<(usize, &str)> {
    let r = line.strip_prefix('#')?;
    let end = r.find(' ')?;
    let n: usize = r[..end].parse().ok()?;
    Some((n, &r[end + 1..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_and_turns_render_as_blocks() {
        let page = "[session_read \"Plan: x\" 0199-abc: 2 turns, proj, 2026-01-01 – 2026-01-02 10:00; showing turns 1–2]\n\
                    #1 user:\nhello\n\n#1 assistant:\nhi there\n#1 tool shell: ls\n#2 user:\nbye\n\n#2 assistant:\nbye!\n";
        assert_eq!(parse_header(page), Some(("Plan: x".into(), 2)));
        let (body, cut) = split_cut(page);
        assert_eq!(cut, None);
        let md = render(body);
        assert!(md.starts_with("## Turn 1 — user\n\nhello\n"), "{md}");
        assert!(
            md.contains("## Turn 1 — theuth\n\nhi there\n*tool shell: ls*\n"),
            "{md}"
        );
        assert!(md.contains("## Turn 2 — theuth\n\nbye!\n"), "{md}");
    }

    #[test]
    fn a_cut_page_names_the_turn_and_drops_its_tail() {
        let page = "[session_read t 1: 3 turns, p, a – b]\n#1 user:\na\n#2 user:\nbb\n[output cut at 64 KiB in turn #2; narrow the range]";
        let (body, cut) = split_cut(page);
        assert_eq!(cut, Some(2));
        assert_eq!(drop_turn_tail(body, 2), "#1 user:\na");
    }
}
