//! Owner: E1 — terminal bytes → static view and agent text (design 171 §8.3): a fresh
//! render-only `vt100::Parser` over the log range `[C, D)`.

use std::borrow::Cow;
use std::num::NonZeroU16;
use std::path::Path;
use std::sync::LazyLock;

use vt100::capture::{BasicFormattedCaptureState, RowContents, basic_formatted_to_plain};

use crate::types::ShellKind;

/// Bytes of rows scrolled past the parser's scrollback that the render keeps in front
/// (171F B17); past it they are only counted ("N more lines are in term.log").
const HEAD_CAP: usize = 8 * 1024 * 1024;

/// The two renders of a run's terminal bytes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rendered {
    /// `out.ansi`: rows with SGR attributes, for the user's static view.
    pub ansi: String,
    /// `out.txt`: plain text, `\r` resolved, spinners collapsed, escapes gone, trailing blanks
    /// trimmed — the record's text, expect input, the agent's and the replicated copy.
    pub text: String,
    /// Rows past the 8 MiB head that are only in `term.log` ("N more lines are in term.log").
    pub dropped_lines: u64,
}

/// The normal-screen rows the emulator scrolls off the top of its scrollback
/// (`Callbacks::on_scroll`), streamed into one basic-formatted capture (one state, so wrapped
/// rows join and attributes carry); past `cap` bytes only counted.
struct Head {
    buf: String,
    state: BasicFormattedCaptureState,
    cap: usize,
    more: u64,
}

impl vt100::Callbacks for Head {
    fn on_scroll(&mut self, row: RowContents<'_>, alternate_screen: bool) {
        if alternate_screen {
            return;
        }
        if self.buf.len() >= self.cap {
            self.more += 1;
            return;
        }
        let _ = row.write_formatted_basic(&mut self.buf, &mut self.state);
    }
}

/// theuth's own text the PTY echoes inside `[C, D)` after a Stop (171F B14): the probe line
/// (any exit) and the `D`-mark tail of the run line — built from `shell`'s own lines so they
/// cannot drift.
static ECHO: LazyLock<regex::Regex> = LazyLock::new(|| {
    const EXIT: i32 = 987_654_321;
    const FILE: &str = "/theuth-render-echo";
    let mut alts = Vec::new();
    for kind in [ShellKind::Bash, ShellKind::Pwsh] {
        let probe = crate::shell::probe_line(kind, EXIT);
        if let Some((pre, post)) = probe.split_once(&EXIT.to_string()) {
            alts.push(format!(
                "{}-?[0-9]+{}",
                regex::escape(pre),
                regex::escape(post)
            ));
        }
        let run = crate::shell::run_line(kind, Path::new(FILE));
        if let Some((_, tail)) = run
            .split_once(FILE)
            .and_then(|(_, rest)| rest.split_once("; "))
        {
            alts.push(regex::escape(tail));
        }
    }
    regex::Regex::new(&alts.join("|")).expect("echo pattern")
});

/// Cut theuth's echo out of a text line. A line that had one keeps only its plain text: the
/// echo can straddle a wrap, and its attributes are theuth's own.
fn unecho(formatted: Cow<'_, str>, plain: String) -> (Cow<'_, str>, String) {
    if !ECHO.is_match(&plain) {
        return (formatted, plain);
    }
    let plain = ECHO.replace_all(&plain, "").into_owned();
    (Cow::Owned(plain.clone()), plain)
}

/// Render `bytes` on a `cols` × `rows` screen with `scrollback` rows.
pub fn render(bytes: &[u8], cols: u16, rows: u16, scrollback: usize) -> Rendered {
    render_capped(bytes, cols, rows, scrollback, HEAD_CAP)
}

fn render_capped(bytes: &[u8], cols: u16, rows: u16, scrollback: usize, cap: usize) -> Rendered {
    let nz = |n: u16| NonZeroU16::new(n).unwrap_or(NonZeroU16::MIN);
    let head = Head {
        buf: String::new(),
        state: BasicFormattedCaptureState::new(),
        cap,
        more: 0,
    };
    let mut parser = vt100::Parser::new_with_callbacks(nz(rows), nz(cols), scrollback, head);
    parser.process(bytes);
    let dropped_lines = parser.callbacks().more;
    let head = std::mem::take(&mut parser.callbacks_mut().buf);
    let screen = parser.screen_mut();
    let mut ansi = String::new();
    let mut lines: Vec<String> = Vec::new();
    // The scrolled-out head, one capture line per text line. Attributes carry across its
    // lines, so only the head's end carries the reset.
    // ponytail: a wide line split exactly at the head/scrollback boundary shows as two lines.
    if !head.is_empty() {
        for line in head.split('\n') {
            let (formatted, plain) = unecho(
                Cow::Borrowed(line),
                basic_formatted_to_plain(line).collect(),
            );
            ansi.push_str(&formatted);
            ansi.push_str("\r\n");
            lines.push(plain);
        }
        ansi.truncate(ansi.len() - 2);
        ansi.push_str("\x1b[m\r\n");
        if dropped_lines > 0 {
            let note = format!("… {dropped_lines} more lines are in term.log");
            ansi.push_str(&note);
            ansi.push_str("\r\n");
            lines.push(note);
        }
    }
    // On the alternate screen `set_scrollback` is a no-op and `scrollback()` is 0, so only
    // what is visible is rendered — what the user saw when the program left it on.
    screen.set_scrollback(usize::MAX);
    let retained = screen.scrollback();
    // (formatted, plain) per text line: a wrapped row continues the previous line so a wide
    // line stays one line; its rows join at a reset (each row's SGR starts from default).
    let mut kept: Vec<(String, String)> = Vec::with_capacity(retained + usize::from(rows));
    let mut wrapping = false;
    let mut take = |formatted: String, plain: String, wrapped: bool| {
        match kept.last_mut() {
            Some((f, p)) if wrapping => {
                f.push_str("\x1b[m");
                f.push_str(&formatted);
                p.push_str(&plain);
            }
            _ => kept.push((formatted, plain)),
        }
        wrapping = wrapped;
    };
    for off in (1..=retained).rev() {
        screen.set_scrollback(off);
        let formatted = screen.rows_formatted(0, cols).next().unwrap_or_default();
        let plain = screen.rows(0, cols).next().unwrap_or_default();
        take(formatted, plain, screen.row_wrapped(0));
    }
    screen.set_scrollback(0);
    let visible: Vec<(String, String)> = screen
        .rows_formatted(0, cols)
        .zip(screen.rows(0, cols))
        .collect();
    for (r, (formatted, plain)) in (0u16..).zip(visible) {
        take(formatted, plain, screen.row_wrapped(r));
    }
    let mut kept: Vec<(Cow<'_, str>, String)> = kept
        .into_iter()
        .map(|(f, p)| unecho(Cow::Owned(f), p))
        .collect();
    // The screen's unused rows are not output (171F B3).
    while kept.last().is_some_and(|(_, p)| p.trim_end().is_empty()) {
        kept.pop();
    }
    // Each ansi line ends with a reset so attributes never leak across lines.
    for (formatted, plain) in kept {
        ansi.push_str(&formatted);
        ansi.push_str("\x1b[m\r\n");
        lines.push(plain);
    }
    // Lines end with `ESC[m CR LF`; drop the last CRLF so the render ends with the reset.
    ansi.truncate(ansi.len().saturating_sub(2));
    for line in &mut lines {
        let end = line.trim_end().len();
        line.truncate(end);
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    Rendered {
        ansi,
        text: lines.join("\n"),
        dropped_lines,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(bytes: &[u8]) -> String {
        render(bytes, 100, 30, 10_000).text
    }

    #[test]
    fn carriage_return_progress_collapses() {
        // A PTY writes `\r\n` for a newline (onlcr).
        assert_eq!(text(b"10%\r55%\r100%\r\ndone\r\n"), "100%\ndone");
    }

    #[test]
    fn spinner_frames_collapse_to_final_transcript() {
        let mut bytes = Vec::new();
        for frame in ["⠋", "⠙", "⠹", "⠸", "⠼"] {
            bytes.extend_from_slice(
                format!("\r\x1b[K\x1b[36m{frame}\x1b[0m Previewing update…").as_bytes(),
            );
        }
        let table = concat!(
            "\r\x1b[KPreviewing update (ir-prod1):\r\n",
            "     Type                              Name                          Plan       Info\r\n",
            "     pulumi:pulumi:Stack               sa-frp-integration-ir-prod1\r\n",
            "\x1b[33m ~ \x1b[0m  ├─ aws:secretsmanager:Secret      frp-api-credentials           \x1b[33mupdate\x1b[0m     [diff: ~tags]\r\n",
            "\x1b[33m ~ \x1b[0m  └─ aws:iam:Role                   frp-integration-role          \x1b[33mupdate\x1b[0m     [diff: ~managedPolicyArns]\r\n",
            "Resources:\r\n",
            "    \x1b[33m~ 3 to update\x1b[0m\r\n",
            "    4 unchanged\r\n",
        );
        bytes.extend_from_slice(table.as_bytes());
        let want = concat!(
            "Previewing update (ir-prod1):\n",
            "     Type                              Name                          Plan       Info\n",
            "     pulumi:pulumi:Stack               sa-frp-integration-ir-prod1\n",
            " ~   ├─ aws:secretsmanager:Secret      frp-api-credentials           update     [diff: ~tags]\n",
            " ~   └─ aws:iam:Role                   frp-integration-role          update     [diff: ~managedPolicyArns]\n",
            "Resources:\n",
            "    ~ 3 to update\n",
            "    4 unchanged",
        );
        let out = render(&bytes, 120, 30, 10_000);
        assert_eq!(out.text, want);
        assert!(!out.text.contains('⠋'));
        assert!(!out.text.contains('\x1b'));
        assert!(
            out.ansi.contains("\x1b[33m"),
            "ansi keeps SGR: {:?}",
            out.ansi
        );
        assert!(out.ansi.ends_with("\x1b[m"));
        assert_eq!(out.dropped_lines, 0);
    }

    #[test]
    fn wide_line_stays_one_line() {
        let line = "x".repeat(250);
        let out = render(format!("{line}\r\nnext\r\n").as_bytes(), 100, 30, 10_000);
        assert_eq!(out.text, format!("{line}\nnext"));
    }

    /// 171F B17: rows past the scrollback stay in the render, in order.
    #[test]
    fn long_output_keeps_every_line() {
        let mut bytes = String::new();
        for i in 1..=20_000 {
            bytes.push_str(&format!("{i}\r\n"));
        }
        let out = render(bytes.as_bytes(), 100, 30, 10_000);
        let want: Vec<String> = (1..=20_000).map(|i| i.to_string()).collect();
        assert_eq!(out.text, want.join("\n"));
        assert!(out.ansi.starts_with("1\r\n2\r\n"), "{:?}", &out.ansi[..20]);
        assert!(out.ansi.ends_with("20000\x1b[m"));
        assert_eq!(out.dropped_lines, 0);
    }

    /// Over the head cap: the head, the cut note, then the scrollback — no line lost unsaid.
    #[test]
    fn head_over_the_cap_says_more_lines_are_in_term_log() {
        let mut bytes = String::new();
        for i in 1..=500 {
            bytes.push_str(&format!("{i}\r\n"));
        }
        let out = render_capped(bytes.as_bytes(), 100, 30, 100, 50);
        assert!(out.dropped_lines > 0);
        let note = format!("… {} more lines are in term.log", out.dropped_lines);
        let (head, rest) = out.text.split_once(&format!("\n{note}\n")).expect("note");
        assert!(out.ansi.contains(&note));
        let last_head: u64 = head.lines().last().unwrap().parse().unwrap();
        assert_eq!(
            head,
            (1..=last_head)
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join("\n")
        );
        let first_rest: u64 = rest.lines().next().unwrap().parse().unwrap();
        assert_eq!(first_rest, last_head + out.dropped_lines + 1);
        assert!(out.text.ends_with("\n500"));
    }

    /// 171F B3: the screen's unused rows are not output rows.
    #[test]
    fn blank_screen_rows_are_dropped() {
        let out = render(b"one\r\ntwo\r\n", 100, 30, 10_000);
        assert_eq!(out.ansi.split("\r\n").count(), 2, "{:?}", out.ansi);
        assert!(out.ansi.ends_with("two\x1b[m"));
        assert_eq!(render(b"", 100, 30, 10_000).ansi, "");
    }

    /// 171F B14: after a Stop the PTY echoes theuth's probe line (and the run line's `D`
    /// tail) inside `[C, D)`; it is cut, the tty's `^C` on that row stays.
    #[test]
    fn stop_echo_is_cut_and_ctrl_c_stays() {
        use crate::shell::{probe_line, run_line};
        for kind in [ShellKind::Bash, ShellKind::Zsh, ShellKind::Pwsh] {
            for exit in [130, 137] {
                let probe = probe_line(kind, exit);
                let out = render(format!("^C{probe}\r\n").as_bytes(), 100, 30, 10_000);
                assert_eq!(out.text, "^C", "{kind:?} {exit}");
                assert!(!out.ansi.contains("133;D"), "{:?}", out.ansi);
                assert_eq!(crate::run::summary(&out.text, &[]).as_deref(), Some("^C"));
            }
            let line = run_line(kind, Path::new("/k/run.sh"));
            let tail = &line[line.find("/k/run.sh").unwrap()..];
            let tail = &tail[tail.find("; ").unwrap() + 2..];
            let bytes = format!("partial\r\n^C{tail}\r\n{}\r\n", probe_line(kind, 130));
            let out = render(bytes.as_bytes(), 100, 30, 10_000);
            assert_eq!(out.text, "partial\n^C", "{kind:?}");
            assert!(!out.ansi.contains("printf") && !out.ansi.contains("Console"));
        }
        // Text that merely mentions printf is output.
        assert_eq!(text(b"printf '%s' x\r\n"), "printf '%s' x");
    }

    #[test]
    fn ansi_keeps_colour() {
        let out = render(b"\x1b[31mred\x1b[0m\r\n", 80, 24, 100);
        assert!(out.ansi.contains("\x1b[31m"));
        assert_eq!(out.text, "red");
    }
}
