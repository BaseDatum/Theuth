//! Owner: E1 — `<cell_result>` (design 171 §9.1, §9.3; lead pins L2, L3): built from the
//! committed row, handed to the agent as `HarnessDeliver{kind: "cell <name>", wake}` per the
//! wake rule. `run="…"` is always the first attribute (the desktop card keys on it).

use std::fmt::Write as _;

use kameo::actor::ActorRef;

use crate::config::CellsConfig;
use crate::error::{CellsError, Result};
use crate::types::{
    CellOutput, CellRef, CellRun, ExitStatus, Expect, RunMode, RunState, SessionId,
};

/// What the row does not hold but the result shows.
#[derive(Clone, Debug, Default)]
pub(crate) struct Extra<'a> {
    /// The cell's `name`.
    pub name: Option<&'a str>,
    /// The cell's `label` (171F B20: the card's "theuth read" line names a run by it).
    pub label: Option<&'a str>,
    /// `private: true`: attributes and expect lines only.
    pub private: bool,
    /// The code that ran when the user edited it (F4).
    pub edited_code: Option<&'a str>,
    /// Python / Monty outputs (shell runs: empty, the text is `cleaned`).
    pub outputs: &'a [CellOutput],
    /// The last result of a Run-all pass.
    pub pass_end: bool,
    /// `$HOME`, to show the cwd as `~/…`.
    pub home: Option<&'a str>,
}

/// The `<cell_result …>` text for `run` (`cleaned` = redacted output; head+tail capped); the
/// record's `name` and `private` (L11) apply — a private run's output never reaches theuth.
pub fn cell_result(run: &CellRun, cleaned: &str, cfg: &CellsConfig) -> String {
    let x = Extra {
        name: run.name.as_deref(),
        private: run.private,
        ..Extra::default()
    };
    build(run, cleaned, cfg, &x)
}

/// [`cell_result`] with everything the row does not carry.
pub(crate) fn build(run: &CellRun, cleaned: &str, cfg: &CellsConfig, x: &Extra<'_>) -> String {
    let (attrs, body) = parts(run, cleaned, cfg, x);
    format!("<cell_result {attrs}>\n{body}</cell_result>")
}

/// `(attributes, body)`; the body ends with a newline when non-empty.
pub(crate) fn parts(
    run: &CellRun,
    cleaned: &str,
    cfg: &CellsConfig,
    x: &Extra<'_>,
) -> (String, String) {
    let mut a = format!("run=\"{}\"", esc(run.id.as_str()));
    match (x.name, &run.cell) {
        (Some(n), _) => {
            let _ = write!(a, " name=\"{}\"", esc(n));
        }
        (None, CellRef::Chat { entry, ordinal, .. }) => {
            let _ = write!(a, " cell=\"{ordinal}\" message=\"{}\"", esc(entry));
        }
        (None, CellRef::Note { key, .. }) => {
            let k = match key {
                crate::types::NoteKey::Name(n) | crate::types::NoteKey::Block(n) => n,
            };
            let _ = write!(a, " cell=\"{}\"", esc(k));
        }
    }
    if let Some(l) = x.label.filter(|l| !l.is_empty()) {
        let l: String = l.chars().take(40).collect();
        let _ = write!(a, " label=\"{}\"", esc(&l));
    }
    let _ = write!(a, " exit=\"{}\"", exit_word(run));
    if let Some(s) = run.secs {
        let _ = write!(a, " secs=\"{}\"", s.round() as i64);
    }
    // 171F4 T2-2: a fragment run says which it was (its `expect` was not judged).
    match run.mode {
        RunMode::Cell => {}
        RunMode::Line { .. } => a.push_str(" mode=\"line\""),
        RunMode::Selection { .. } => a.push_str(" mode=\"selection\""),
    }
    let _ = write!(a, " cwd=\"{}\"", esc(&tilde(&run.cwd, x.home)));
    let _ = write!(a, " interpreter=\"{}\"", esc(chip_of(&run.interpreter)));
    if !run.expect.is_empty() {
        let met = run.expect.iter().filter(|e| e.met).count();
        let total = run.expect.len();
        if met == total {
            let _ = write!(a, " expect=\"met {met}/{total}\"");
        } else {
            let _ = write!(a, " expect=\"missed {}/{total}\"", total - met);
        }
    }
    let _ = write!(a, " edited=\"{}\"", run.edited);
    let _ = write!(a, " pressed=\"{}\"", esc(&run.pressed_by));
    if let Some(at) = run
        .ended_ms
        .and_then(chrono::DateTime::from_timestamp_millis)
    {
        let local = at.with_timezone(&chrono::Local);
        let _ = write!(a, " at=\"{}\"", local.format("%H:%M"));
    }
    if let CellRef::Note { doc, .. } = &run.cell {
        let _ = write!(a, " in=\"note {}\"", esc(&doc.0));
    }
    if x.pass_end
        && let Some(p) = &run.pass
    {
        let _ = write!(a, " pass=\"{}\" pass_end=\"true\"", esc(p));
    }

    let mut b = String::new();
    // §7.6: Run anyway past an unarmed `after` gate, said once, private runs included.
    if let Some(p) = &run.ran_past {
        let _ = writeln!(b, "{p}");
    }
    for e in &run.expect {
        let item = match &e.item {
            Expect::Text(t) => t.clone(),
            Expect::Regex(r) => format!("/{r}/"),
        };
        let _ = writeln!(b, "expect {} {item}", if e.met { "✓" } else { "✗" });
    }
    if x.private {
        b.push_str("output kept from theuth (private)\n");
        return (a, b);
    }
    if let Some(s) = &run.summary {
        let _ = writeln!(b, "summary {s}");
    }
    let rich: Vec<String> = x.outputs.iter().filter_map(output_text).collect();
    let text = if rich.is_empty() {
        cleaned.to_owned()
    } else {
        rich.join("\n")
    };
    if run.state == RunState::Cancelled {
        b.push_str("cancelled before it ran\n");
    } else if text.trim().is_empty() {
        b.push_str("no output\n");
    } else {
        let (shown, complete) = head_tail(&text, cfg.agent_bytes, run.id.as_str());
        let _ = writeln!(
            b,
            "output {}, {}:",
            size(text.len()),
            if complete {
                "complete"
            } else {
                "head and tail"
            }
        );
        b.push_str(&shown);
        if !shown.ends_with('\n') {
            b.push('\n');
        }
    }
    if let Some(code) = x.edited_code {
        b.push_str("code that ran (edited):\n");
        let (c, _) = cut(code, 2048);
        b.push_str(c);
        if !c.ends_with('\n') {
            b.push('\n');
        }
    }
    (a, b)
}

/// One output's agent text: `text/plain` of a bundle, a stream's text, `ename: evalue` plus
/// the last traceback frame.
fn output_text(o: &CellOutput) -> Option<String> {
    match o {
        CellOutput::Stream { text, .. } => Some(text.clone()),
        CellOutput::DisplayData { data, .. } | CellOutput::ExecuteResult { data, .. } => {
            data.get("text/plain").map(|v| match v {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Array(a) => a
                    .iter()
                    .filter_map(|x| x.as_str())
                    .collect::<Vec<_>>()
                    .join(""),
                other => other.to_string(),
            })
        }
        CellOutput::Error {
            ename,
            evalue,
            traceback,
        } => {
            let mut s = format!("{ename}: {evalue}");
            if let Some(last) = traceback.last() {
                s.push('\n');
                s.push_str(last);
            }
            Some(s)
        }
    }
}

/// `exit`: code, signal name, or `timeout` / `killed` / `lost` / `cancelled` / `stopped`.
fn exit_word(run: &CellRun) -> String {
    match run.state {
        RunState::Timeout => return "timeout".into(),
        RunState::Killed => return "killed".into(),
        RunState::Lost => return "lost".into(),
        RunState::Cancelled => return "cancelled".into(),
        _ => {}
    }
    match &run.exit {
        Some(ExitStatus::Code(c)) => c.to_string(),
        Some(ExitStatus::Signal(s)) => s.clone(),
        None if run.state == RunState::Stopped => "stopped".into(),
        None => "?".into(),
    }
}

/// `CellRun.interpreter` is `<chip> <path>`; the attribute shows the chip.
fn chip_of(interpreter: &str) -> &str {
    interpreter
        .rfind(" /")
        .map_or(interpreter, |i| &interpreter[..i])
}

fn tilde(cwd: &str, home: Option<&str>) -> String {
    match home.filter(|h| !h.is_empty()) {
        Some(h) if cwd == h => "~".into(),
        Some(h) => cwd
            .strip_prefix(h)
            .filter(|rest| rest.starts_with('/'))
            .map_or_else(|| cwd.to_owned(), |rest| format!("~{rest}")),
        None => cwd.to_owned(),
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// `1.9 KiB`, `312 B`.
fn size(n: usize) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.1} KiB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MiB", n as f64 / (1024.0 * 1024.0))
    }
}

/// `14 212`.
fn grouped(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

/// The longest prefix of at most `max` bytes, cut on a char boundary.
fn cut(s: &str, max: usize) -> (&str, bool) {
    if s.len() <= max {
        return (s, true);
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    (&s[..end], false)
}

/// Line-aligned cuts for a head of at most `head_max` bytes and a tail from byte `tail_from`
/// on (171F B17): the head ends after its last `\n`, the tail starts at the next line start; a
/// window without a newline is cut at the byte (on a char boundary). `(head_end, tail_start)`.
pub(crate) fn line_cuts(text: &str, head_max: usize, tail_from: usize) -> (usize, usize) {
    let (head, _) = cut(text, head_max);
    let head_end = head.rfind('\n').map_or(head.len(), |i| i + 1);
    let mut tail = tail_from.min(text.len());
    while !text.is_char_boundary(tail) {
        tail += 1;
    }
    let at_line_start = tail == 0 || text.as_bytes()[tail - 1] == b'\n';
    let tail_start = text[tail..]
        .find('\n')
        .map(|i| tail + i + 1)
        .filter(|&t| !at_line_start && t < text.len())
        .unwrap_or(tail);
    (head_end, tail_start.max(head_end))
}

/// §9.1: head + tail at `cap` bytes (¾ head, ¼ tail) with the exact cut line.
pub(crate) fn head_tail(text: &str, cap: usize, run: &str) -> (String, bool) {
    if text.len() <= cap {
        return (text.to_owned(), true);
    }
    let head_max = cap * 3 / 4;
    let (head_end, tail_start) = line_cuts(text, head_max, text.len() - (cap - head_max));
    let head = &text[..head_end];
    let cut_bytes = tail_start - head.len();
    let mut out = head.to_owned();
    if !out.ends_with('\n') {
        out.push('\n');
    }
    let _ = writeln!(
        out,
        "… {} bytes cut — cell_output(\"{run}\", offset={}) reads them …",
        grouped(cut_bytes),
        head.len()
    );
    out.push_str(&text[tail_start..]);
    (out, false)
}

/// Deliver `text` to `session` with the §9.3 wake rule (`wake = false` parks it).
pub async fn deliver(
    session: SessionId,
    run: &CellRun,
    text: String,
    wake: bool,
) -> Result<(), CellsError> {
    let name = text
        .split_once(" name=\"")
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(n, _)| n.to_owned());
    let label = match (&name, &run.cell) {
        (Some(n), _) => n.clone(),
        (None, CellRef::Chat { ordinal, .. }) => ordinal.to_string(),
        (None, CellRef::Note { .. }) => "note".to_owned(),
    };
    let body = text
        .split_once('\n')
        .map(|(_, rest)| rest.strip_suffix("</cell_result>").unwrap_or(rest))
        .unwrap_or_default()
        .to_owned();
    send(session, format!("cell {label}"), body, text, wake).await
}

/// L3: `HarnessDeliver { session, kind, text: bare body, wrapped, wake }`.
pub(crate) async fn send(
    session: SessionId,
    kind: String,
    text: String,
    wrapped: String,
    wake: bool,
) -> Result<()> {
    let agents = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
        .ok()
        .flatten()
        .ok_or_else(|| CellsError::Failed("the agent is not running".into()))?;
    agents
        .tell(agent::supervisor::HarnessDeliver {
            session,
            kind,
            text,
            wrapped,
            wake,
        })
        .await
        .map_err(|e| CellsError::Failed(format!("deliver: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ExpectResult, KernelId, KernelSpec, RunId, RunMode, Scope};

    fn run() -> CellRun {
        let session = SessionId(uuid::Uuid::nil());
        CellRun {
            id: RunId("r_01JC7Q2M8V".into()),
            cell: CellRef::Chat {
                session,
                entry: "12".into(),
                ordinal: 2,
            },
            mode: RunMode::Cell,
            pass: Some("p1".into()),
            kernel: KernelId {
                scope: Scope::Session(session),
                spec: KernelSpec::Monty,
                instance: 0,
            },
            interpreter: "bash 5.2 (Homebrew) /opt/homebrew/bin/bash".into(),
            cwd: "/Users/sky/dr".into(),
            env_names: Vec::new(),
            presented_sha: "a".into(),
            code_sha: "a".into(),
            edited: false,
            pressed_by: "desktop · Sky's MacBook".into(),
            state: RunState::Done,
            exit: Some(ExitStatus::Code(0)),
            secs: Some(41.2),
            expect: vec![ExpectResult {
                item: Expect::Text("3 to update".into()),
                met: true,
            }],
            summary: Some("~3 update · 0 replace · 0 delete".into()),
            output_bytes: Some(10),
            queued_ms: 0,
            started_ms: Some(0),
            ended_ms: Some(1_700_000_000_000),
            masked: 0,
            private: false,
            name: Some("preview-ir-prod1".into()),
            input: None,
            ran_past: None,
        }
    }

    /// L2: `run` first, then the design's order; the body per §9.1.
    #[test]
    fn attributes_in_pin_order_and_body() {
        let cfg = CellsConfig::default();
        let x = Extra {
            name: Some("preview-ir-prod1"),
            home: Some("/Users/sky"),
            pass_end: true,
            ..Extra::default()
        };
        let t = build(&run(), "Resources:\n    ~ 3 to update\n", &cfg, &x);
        let head = t.lines().next().unwrap();
        assert!(head.starts_with("<cell_result run=\"r_01JC7Q2M8V\" name=\"preview-ir-prod1\" exit=\"0\" secs=\"41\" cwd=\"~/dr\" interpreter=\"bash 5.2 (Homebrew)\" expect=\"met 1/1\" edited=\"false\" pressed=\"desktop · Sky's MacBook\" at=\""), "{head}");
        assert!(head.ends_with("pass=\"p1\" pass_end=\"true\">"), "{head}");
        assert!(t.contains("\nexpect ✓ 3 to update\nsummary ~3 update · 0 replace · 0 delete\noutput 29 B, complete:\nResources:\n"), "{t}");
        assert!(t.ends_with("</cell_result>"));
        // Unnamed: cell + message.
        let t = build(&run(), "x", &cfg, &Extra::default());
        assert!(
            t.starts_with("<cell_result run=\"r_01JC7Q2M8V\" cell=\"2\" message=\"12\" exit=\"0\"")
        );
    }

    #[test]
    fn private_sends_attributes_and_expect_only() {
        let cfg = CellsConfig::default();
        let x = Extra {
            private: true,
            ..Extra::default()
        };
        let t = build(&run(), "SECRET OUTPUT", &cfg, &x);
        assert!(!t.contains("SECRET"));
        assert!(t.contains("expect ✓ 3 to update\noutput kept from theuth (private)\n"));
        assert!(!t.contains("summary"));
    }

    /// The note path (`cell_result`, no `Extra`): the record's name and `private` apply.
    #[test]
    fn cell_result_takes_name_and_private_from_the_record() {
        let cfg = CellsConfig::default();
        let mut r = run();
        r.private = true;
        let t = cell_result(&r, "SECRET OUTPUT", &cfg);
        assert!(
            t.starts_with("<cell_result run=\"r_01JC7Q2M8V\" name=\"preview-ir-prod1\" "),
            "{t}"
        );
        assert!(!t.contains("SECRET"), "{t}");
    }

    #[test]
    fn long_output_is_head_and_tail_with_the_cut_line() {
        let text = "a".repeat(20_000);
        let (shown, complete) = head_tail(&text, 8192, "r_x");
        assert!(!complete);
        assert!(
            shown.contains("… 11 808 bytes cut — cell_output(\"r_x\", offset=6144) reads them …")
        );
        let kept: usize = shown
            .lines()
            .filter(|l| l.bytes().all(|b| b == b'a'))
            .map(str::len)
            .sum();
        assert_eq!(kept, 8192);
    }

    /// 171F B20: `label` rides on the tag when the cell has one (escaped, ≤ 40 chars).
    #[test]
    fn label_attribute_only_when_labelled() {
        let cfg = CellsConfig::default();
        let t = build(&run(), "x", &cfg, &Extra::default());
        assert!(!t.lines().next().unwrap().contains(" label="), "{t}");
        let x = Extra {
            label: Some("Preview \"prod\""),
            ..Extra::default()
        };
        let t = build(&run(), "x", &cfg, &x);
        assert!(
            t.starts_with("<cell_result run=\"r_01JC7Q2M8V\" cell=\"2\" message=\"12\" label=\"Preview &quot;prod&quot;\" exit=\"0\""),
            "{t}"
        );
        let long = "L".repeat(60);
        let x = Extra {
            label: Some(&long),
            ..Extra::default()
        };
        let t = build(&run(), "x", &cfg, &x);
        assert!(
            t.contains(&format!(" label=\"{}\" ", "L".repeat(40))),
            "{t}"
        );
    }

    /// 171F B17: with newlines the head ends at a line end, the tail starts at a line start,
    /// and `offset` is the head's length.
    #[test]
    fn head_and_tail_cut_on_lines() {
        let text: String = (0..3000).map(|i| format!("row {i:04}\n")).collect();
        let (shown, complete) = head_tail(&text, 8192, "r_x");
        assert!(!complete);
        let (head, rest) = shown.split_once("… ").unwrap();
        assert!(head.ends_with('\n'));
        let (cut_line, tail) = rest.split_once('\n').unwrap();
        assert!(
            cut_line.contains(&format!("offset={})", head.len())),
            "{cut_line}"
        );
        assert!(text.starts_with(head));
        assert!(text.ends_with(tail));
        assert!(tail.starts_with("row "));
        let tail_start = text.len() - tail.len();
        assert_eq!(&text.as_bytes()[tail_start - 1..tail_start], b"\n");
        assert!(head.len() <= 6144 && tail.len() <= 2048);
    }

    #[test]
    fn edited_and_python_forms() {
        let cfg = CellsConfig::default();
        let outs = vec![
            CellOutput::ExecuteResult {
                data: [("text/plain".to_owned(), serde_json::json!("42"))].into(),
                metadata: serde_json::Value::Null,
                count: 1,
            },
            CellOutput::Error {
                ename: "KeyError".into(),
                evalue: "'x'".into(),
                traceback: vec!["frame 1".into(), "File cell, line 3".into()],
            },
        ];
        let mut r = run();
        r.edited = true;
        let x = Extra {
            outputs: &outs,
            edited_code: Some("ls -la"),
            ..Extra::default()
        };
        let t = build(&r, "", &cfg, &x);
        assert!(t.contains("42\nKeyError: 'x'\nFile cell, line 3\n"));
        assert!(t.contains("edited=\"true\""));
        assert!(t.contains("code that ran (edited):\nls -la\n"));
    }

    #[test]
    fn exit_words() {
        let mut r = run();
        r.state = RunState::Timeout;
        assert_eq!(exit_word(&r), "timeout");
        r.state = RunState::Lost;
        assert_eq!(exit_word(&r), "lost");
        r.state = RunState::Failed;
        r.exit = Some(ExitStatus::Signal("KILL".into()));
        assert_eq!(exit_word(&r), "KILL");
    }
}
