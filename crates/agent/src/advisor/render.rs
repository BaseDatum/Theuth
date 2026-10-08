//! 125 §4.4–§4.5: the delivered-prefix fingerprint and the update renderer —
//! the compact markdown the advisor reads for one or more agent-core steps.

use std::hash::{DefaultHasher, Hash, Hasher};

use crate::HistoryEntry;

/// Tool IO cap per result (omp's constants).
const RESULT_BYTES: usize = 8 * 1024;
const RESULT_LINES: usize = 80;
/// Reasoning preview length.
const THINKING_CHARS: usize = 400;
/// Primary tool argument cap.
const ARG_CHARS: usize = 120;
/// Whole-update cap.
pub const UPDATE_BYTES: usize = 48 * 1024;
const ELIDED: &str = "[…content elided to fit advisor context…]";

/// Harness wrappers a user row may consist of (the TUI's `injected()` set).
const HARNESS_TAGS: &[&str] = &[
    "reminder",
    "job_result",
    "job_idle",
    "message",
    "subagent_result",
    "schedule",
    "context_summary",
    "todo",
    "context_pressure",
    "runtime",
    "knowledge",
    "nudge",
    "journal",
];

/// How one update is framed.
#[derive(Clone, Copy, Debug, Default)]
pub struct UpdateOpts<'a> {
    /// Agent-core steps this update covers (`> 1` after coalescing; `0` for
    /// a consultation with nothing pending or a one-shot's whole transcript).
    pub steps: u32,
    /// More tool steps follow in the advised turn.
    pub will_continue: bool,
    /// One italic line under the heading (restart, replay, re-prime).
    pub preface: Option<&'a str>,
    /// Render `Reasoning` rows; off after the advisor model refused on them.
    pub reasoning: bool,
    /// R1.5: never elide the first row (a subagent's brief).
    pub pin_first: bool,
    /// R1.4: the `### Consultation` block, appended after a `---` line and
    /// before the `[in progress …]` trailer; counted against the byte cap.
    pub consult: Option<&'a str>,
}

/// Rows the cursor counts and fingerprints: everything but Advisory and
/// UI-only rows.
pub fn counted(e: &HistoryEntry) -> bool {
    !matches!(
        e,
        HistoryEntry::Advisory { .. } | HistoryEntry::Error { .. }
    )
}

/// `DefaultHasher` over the variant tag and the content fields a rewrite
/// would change — never `context`, `thinking_ms`, usage or seq.
pub fn fingerprint(e: &HistoryEntry) -> u64 {
    let mut h = DefaultHasher::new();
    match e {
        HistoryEntry::User { text, .. } => {
            0u8.hash(&mut h);
            text.hash(&mut h);
        }
        HistoryEntry::Assistant { text, .. } => {
            1u8.hash(&mut h);
            text.hash(&mut h);
        }
        HistoryEntry::Reasoning { text, .. } => {
            2u8.hash(&mut h);
            text.hash(&mut h);
        }
        HistoryEntry::Tool {
            name, args, result, ..
        } => {
            3u8.hash(&mut h);
            name.hash(&mut h);
            args.to_string().hash(&mut h);
            result.hash(&mut h);
        }
        HistoryEntry::Compacted { epoch, summary, .. } => {
            4u8.hash(&mut h);
            epoch.hash(&mut h);
            summary.hash(&mut h);
        }
        HistoryEntry::Interrupted { text, .. } => {
            5u8.hash(&mut h);
            text.hash(&mut h);
        }
        HistoryEntry::Error { text, .. } => {
            6u8.hash(&mut h);
            text.hash(&mut h);
        }
        HistoryEntry::Advisory { .. } => 7u8.hash(&mut h),
    }
    h.finish()
}

/// Char-boundary-safe prefix of `max` chars with `…` when cut.
fn preview(s: &str, max: usize) -> String {
    let s = s.trim();
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_owned(),
    }
}

/// `text` capped at [`RESULT_BYTES`] / [`RESULT_LINES`] with the middle elided.
fn cap_result(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = if lines.len() > RESULT_LINES {
        let head = RESULT_LINES / 2;
        let tail = RESULT_LINES - head - 1;
        let mut v: Vec<&str> = lines[..head].to_vec();
        v.push(ELIDED);
        v.extend_from_slice(&lines[lines.len() - tail..]);
        v.join("\n")
    } else {
        text.to_owned()
    };
    if out.len() > RESULT_BYTES {
        let half = RESULT_BYTES / 2;
        let a = floor_char(&out, half);
        let b = ceil_char(&out, out.len() - half);
        out = format!("{}\n{ELIDED}\n{}", &out[..a], &out[b..]);
    }
    out
}

fn floor_char(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_char(s: &str, mut i: usize) -> usize {
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// `(tag, from)` when `text` is exactly one harness wrapper element.
fn harness_wrapper(text: &str) -> Option<(&str, Option<&str>)> {
    let t = text.trim();
    let rest = t.strip_prefix('<')?;
    let end = rest.find(|c: char| c == '>' || c.is_whitespace())?;
    let tag = &rest[..end];
    if !HARNESS_TAGS.contains(&tag) {
        return None;
    }
    let close = format!("</{tag}>");
    if !t.ends_with(&close) && !t.ends_with("/>") {
        return None;
    }
    let head = &rest[end..rest.find('>')?];
    let from = head
        .find("from=\"")
        .map(|i| &head[i + 6..])
        .and_then(|s| s.split('"').next());
    Some((tag, from))
}

/// Body lines of a wrapper element (between the opening `>` and `</tag>`).
fn wrapper_body(text: &str) -> &str {
    let t = text.trim();
    let start = t.find('>').map_or(0, |i| i + 1);
    let end = t.rfind("</").unwrap_or(t.len()).max(start);
    t[start..end].trim()
}

/// The primary argument of a tool call: the first string argument, else the
/// first argument's JSON, capped at [`ARG_CHARS`].
fn primary_arg(args: &serde_json::Value) -> String {
    let raw = match args {
        serde_json::Value::Object(m) => m
            .values()
            .find_map(|v| v.as_str().map(str::to_owned))
            .or_else(|| m.values().next().map(ToString::to_string))
            .unwrap_or_default(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    };
    preview(raw.replace('\n', " ").as_str(), ARG_CHARS)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    None,
    User,
    Agent,
}

/// One rendered row and, for a tool with output, its separable result block.
struct Piece {
    text: String,
    result: Option<String>,
}

/// Render `rows` as one advisor update (125 §4.5). Advisory and UI-only rows
/// are skipped; the heading carries the step count after coalescing; the
/// trailer marks a mid-turn update. Beyond [`UPDATE_BYTES`] the oldest tool
/// results are elided first, then the oldest rows.
pub fn render_update(rows: &[(i64, HistoryEntry)], opts: UpdateOpts<'_>) -> String {
    let mut pieces: Vec<Piece> = Vec::new();
    let mut role = Role::None;
    let marker = |want: Role, role: &mut Role, out: &mut String| {
        if *role != want {
            *role = want;
            out.push_str(match want {
                Role::User => "**user**:\n",
                Role::Agent => "**agent**:\n",
                Role::None => "",
            });
        }
    };
    for (_, e) in rows {
        let mut text = String::new();
        let mut result = None;
        match e {
            HistoryEntry::User { text: t, .. } => {
                marker(Role::User, &mut role, &mut text);
                match harness_wrapper(t) {
                    Some(("message", from)) => {
                        let first = wrapper_body(t).lines().next().unwrap_or("").trim();
                        text.push_str(&format!(
                            "_(message from {}: {})_",
                            from.unwrap_or("?"),
                            preview(first, ARG_CHARS)
                        ));
                    }
                    Some((tag, from)) => {
                        let n = wrapper_body(t).lines().count();
                        text.push_str(&format!(
                            "_(harness input: {tag} from {}, {n} lines)_",
                            from.unwrap_or("harness")
                        ));
                    }
                    None => text.push_str(t.trim()),
                }
            }
            HistoryEntry::Assistant { text: t, .. } => {
                marker(Role::Agent, &mut role, &mut text);
                text.push_str(t.trim());
            }
            HistoryEntry::Reasoning { text: t, .. } => {
                if !opts.reasoning {
                    continue;
                }
                marker(Role::Agent, &mut role, &mut text);
                text.push_str(&format!("_thinking:_ {}", preview(t, THINKING_CHARS)));
            }
            HistoryEntry::Tool {
                name,
                args,
                result: r,
                ..
            } => {
                marker(Role::Agent, &mut role, &mut text);
                text.push_str(&format!("→ {name}({})", primary_arg(args)));
                match r {
                    None => text.push_str(" ⇒ pending"),
                    Some((out, true)) => {
                        let first = out.lines().next().unwrap_or("").trim();
                        text.push_str(&format!(
                            " ⇒ error · {} lines — {}",
                            out.lines().count(),
                            preview(first, ARG_CHARS)
                        ));
                    }
                    Some((out, false)) => {
                        text.push_str(&format!(" ⇒ ok · {} lines", out.lines().count()));
                        if !out.trim().is_empty() {
                            result =
                                Some(format!("Tool result:\n```text\n{}\n```", cap_result(out)));
                        }
                    }
                }
            }
            HistoryEntry::Compacted { epoch, .. } => {
                role = Role::None;
                text.push_str(&format!("_(context compacted — epoch {epoch})_"));
            }
            HistoryEntry::Interrupted { text: t, .. } => {
                marker(Role::Agent, &mut role, &mut text);
                text.push_str(&format!("{}\n_(interrupted)_", t.trim()));
            }
            HistoryEntry::Error { .. } | HistoryEntry::Advisory { .. } => continue,
        }
        pieces.push(Piece { text, result });
    }

    let heading = if opts.steps == 1 {
        "### Session update".to_owned()
    } else {
        format!("### Session update ({} steps)", opts.steps)
    };
    let mut head = heading;
    if let Some(p) = opts.preface {
        head.push('\n');
        head.push_str(p);
    }
    let mut trailer = String::new();
    if let Some(block) = opts.consult {
        trailer.push_str("\n\n---\n\n");
        trailer.push_str(block);
    }
    if opts.will_continue {
        trailer.push_str("\n\n---\n\n[in progress — more steps follow]");
    }
    let fixed = head.len() + trailer.len() + 2;
    let size = |pieces: &[Piece]| {
        pieces
            .iter()
            .map(|p| p.text.len() + p.result.as_ref().map_or(0, |r| r.len() + 1) + 1)
            .sum::<usize>()
    };
    // Oldest tool results first.
    let mut i = 0;
    while fixed + size(&pieces) > UPDATE_BYTES && i < pieces.len() {
        if pieces[i].result.take().is_some() {
            pieces[i].text.push_str(&format!("\n{ELIDED}"));
        }
        i += 1;
    }
    // Then the oldest rows — after the pinned first one when asked.
    let keep = if opts.pin_first { 1 } else { 0 };
    let mut dropped = 0usize;
    while fixed + size(&pieces) > UPDATE_BYTES && pieces.len() > keep + 1 {
        pieces.remove(keep);
        dropped += 1;
    }
    let mut body = String::new();
    for (n, p) in pieces.iter().enumerate() {
        if n == keep && dropped > 0 {
            body.push_str(&format!("_({dropped} earlier rows elided)_\n"));
        }
        body.push_str(&p.text);
        body.push('\n');
        if let Some(r) = &p.result {
            body.push_str(r);
            body.push('\n');
        }
    }
    format!("{head}\n\n{}{trailer}", body.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(t: &str) -> HistoryEntry {
        HistoryEntry::User {
            text: t.into(),
            turn: None,
            context: None,
            peer: None,
            vision: None,
        }
    }

    fn tool(name: &str, args: serde_json::Value, result: Option<(&str, bool)>) -> HistoryEntry {
        HistoryEntry::Tool {
            call_id: "c".into(),
            name: name.into(),
            args,
            result: result.map(|(o, e)| (o.to_owned(), e)),
            files: Vec::new(),
            context: None,
        }
    }

    #[test]
    fn renders_markers_tools_wrappers_and_trailer() {
        let rows = vec![
            (1, user("fix the build")),
            (
                2,
                HistoryEntry::Reasoning {
                    text: "hmm".into(),
                    turn: None,
                    context: None,
                    thinking_ms: None,
                },
            ),
            (
                3,
                HistoryEntry::Assistant {
                    text: "reading".into(),
                    turn: None,
                    context: None,
                },
            ),
            (
                4,
                tool(
                    "read_file",
                    serde_json::json!({"path": "a.rs"}),
                    Some(("line1\nline2", false)),
                ),
            ),
            (
                5,
                tool(
                    "shell",
                    serde_json::json!({"cmd": "cargo test"}),
                    Some(("ERROR: could not compile\nmore", true)),
                ),
            ),
            (
                6,
                user("<reminder id=\"r1\" from=\"jobs\">\nx\ny\n</reminder>"),
            ),
            (
                6,
                user("<message from=\"peer\" id=\"1\">\nhello there\nmore\n</message>"),
            ),
        ];
        let s = render_update(
            &rows,
            UpdateOpts {
                steps: 3,
                will_continue: true,
                preface: Some("_(p)_"),
                reasoning: true,
                ..Default::default()
            },
        );
        assert!(s.starts_with("### Session update (3 steps)\n_(p)_\n\n**user**:\nfix the build\n"));
        assert!(s.contains("**agent**:\n_thinking:_ hmm\nreading\n→ read_file(a.rs) ⇒ ok · 2 lines\nTool result:\n```text\nline1\nline2\n```\n"));
        assert!(s.contains("→ shell(cargo test) ⇒ error · 2 lines — ERROR: could not compile\n"));
        assert!(s.contains("**user**:\n_(harness input: reminder from jobs, 2 lines)_\n_(message from peer: hello there)_"));
        assert!(s.ends_with("\n\n---\n\n[in progress — more steps follow]"));
        let one = UpdateOpts {
            steps: 1,
            ..Default::default()
        };
        let idle = render_update(&rows[..1], one);
        assert_eq!(idle, "### Session update\n\n**user**:\nfix the build");
        // Reasoning off drops the thinking line.
        let quiet = render_update(&rows[..3], one);
        assert!(!quiet.contains("_thinking:_"));
    }

    /// R1.4 step 4 / R1.5: the block sits after `---` and before the
    /// trailer; a zero-step heading says so; the pinned first row survives
    /// the cap while the rows after it are elided.
    #[test]
    fn consult_block_and_pinned_first_row() {
        let rows = vec![(1, user("fix the build"))];
        let s = render_update(
            &rows,
            UpdateOpts {
                steps: 0,
                will_continue: true,
                consult: Some("### Consultation\nQ"),
                ..Default::default()
            },
        );
        assert_eq!(
            s,
            "### Session update (0 steps)\n\n**user**:\nfix the build\n\n---\n\n### Consultation\nQ\n\n---\n\n[in progress — more steps follow]"
        );
        let big = "x".repeat(7 * 1024);
        let mut rows = vec![(0, user("the brief"))];
        rows.extend((1..13).map(|i| (i, user(&format!("row{i} {big}")))));
        let s = render_update(
            &rows,
            UpdateOpts {
                pin_first: true,
                consult: Some("### Consultation\nQ"),
                ..Default::default()
            },
        );
        assert!(s.len() <= UPDATE_BYTES, "{}", s.len());
        assert!(
            s.starts_with("### Session update (0 steps)\n\n**user**:\nthe brief\n_("),
            "{}",
            &s[..200]
        );
        assert!(s.contains("earlier rows elided)_\nrow"));
        assert!(
            !s.contains("row1 "),
            "the oldest rows after the brief go first"
        );
        assert!(s.contains("row12 "));
    }

    #[test]
    fn caps_results_and_whole_update() {
        let long = (0..200)
            .map(|i| format!("l{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let capped = cap_result(&long);
        assert!(capped.lines().count() <= RESULT_LINES + 1);
        assert!(capped.contains(ELIDED));
        let big = "x".repeat(7 * 1024);
        let rows: Vec<(i64, HistoryEntry)> = (0..12)
            .map(|i| {
                (
                    i,
                    tool(
                        "read_file",
                        serde_json::json!({"p": "f"}),
                        Some((&big, false)),
                    ),
                )
            })
            .collect();
        let s = render_update(&rows, UpdateOpts::default());
        assert!(s.len() <= UPDATE_BYTES, "{}", s.len());
        assert!(s.contains(ELIDED));
    }

    #[test]
    fn fingerprint_ignores_context_fields() {
        let a = user("hi");
        let b = HistoryEntry::User {
            text: "hi".into(),
            turn: Some(3),
            context: None,
            peer: None,
            vision: None,
        };
        assert_eq!(fingerprint(&a), fingerprint(&b));
        assert_ne!(fingerprint(&a), fingerprint(&user("ho")));
    }
}
