//! Owner: E1 — terminal bytes → static view and agent text (design 171 §8.3): a fresh
//! render-only `vt100::Parser` over the log range `[C, D)`.

use std::num::NonZeroU16;

/// The two renders of a run's terminal bytes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rendered {
    /// `out.ansi`: rows with SGR attributes, for the user's static view.
    pub ansi: String,
    /// `out.txt`: plain text, `\r` resolved, spinners collapsed, escapes gone, trailing blanks
    /// trimmed — the record's text, expect input, the agent's and the replicated copy.
    pub text: String,
    /// Lines beyond the scrollback ("first N lines are in the log").
    pub dropped_lines: u64,
}

/// Counts the normal-screen rows the emulator discarded off the top of its scrollback
/// (`Callbacks::on_scroll`): exactly the lines that are in the log but not in the render.
struct Dropped(u64);

impl vt100::Callbacks for Dropped {
    fn on_scroll(&mut self, _: vt100::capture::RowContents<'_>, alternate_screen: bool) {
        if !alternate_screen {
            self.0 += 1;
        }
    }
}

/// Render `bytes` on a `cols` × `rows` screen with `scrollback` rows.
pub fn render(bytes: &[u8], cols: u16, rows: u16, scrollback: usize) -> Rendered {
    let nz = |n: u16| NonZeroU16::new(n).unwrap_or(NonZeroU16::MIN);
    let mut parser = vt100::Parser::new_with_callbacks(nz(rows), nz(cols), scrollback, Dropped(0));
    parser.process(bytes);
    let dropped_lines = parser.callbacks().0;
    let screen = parser.screen_mut();
    // On the alternate screen `set_scrollback` is a no-op and `scrollback()` is 0, so only
    // what is visible is rendered — what the user saw when the program left it on.
    screen.set_scrollback(usize::MAX);
    let retained = screen.scrollback();
    let mut ansi = String::new();
    let mut lines: Vec<String> = Vec::with_capacity(retained + usize::from(rows));
    let mut wrapping = false;
    // Each ansi row ends with a reset so attributes never leak across rows; a wrapped row
    // continues the previous text line so a wide line stays one line.
    let mut take = |formatted: String, plain: String, wrapped: bool| {
        ansi.push_str(&formatted);
        ansi.push_str("\x1b[m\r\n");
        match lines.last_mut() {
            Some(prev) if wrapping => prev.push_str(&plain),
            _ => lines.push(plain),
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
    // Rows end with `ESC[m CR LF`; drop the last CRLF so the render ends with the reset.
    ansi.truncate(ansi.len().saturating_sub(2));
    for line in &mut lines {
        let end = line.trim_end().len();
        line.truncate(end);
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    let mut text = lines.join("\n");
    if dropped_lines > 0 {
        let note = format!("first {dropped_lines} lines are in the log\n");
        ansi.insert_str(0, &note);
        text.insert_str(0, &note);
    }
    Rendered {
        ansi,
        text,
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

    #[test]
    fn overflow_says_first_lines_are_in_the_log() {
        let mut bytes = String::new();
        for i in 1..=10_100 {
            bytes.push_str(&format!("line {i}\r\n"));
        }
        let out = render(bytes.as_bytes(), 100, 30, 10_000);
        assert!(out.dropped_lines > 0);
        let note = format!("first {} lines are in the log\n", out.dropped_lines);
        assert!(out.text.starts_with(&note));
        assert!(out.ansi.starts_with(&note));
        assert!(out.text.ends_with("line 10100"));
    }

    #[test]
    fn ansi_keeps_colour() {
        let out = render(b"\x1b[31mred\x1b[0m\r\n", 80, 24, 100);
        assert!(out.ansi.contains("\x1b[31m"));
        assert_eq!(out.text, "red");
    }
}
