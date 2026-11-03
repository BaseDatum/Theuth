//! Owner: E1 — terminal bytes → static view and agent text (design 171 §8.3): a fresh
//! render-only `vt100::Parser` over the log range `[C, D)`.

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

/// Render `bytes` on a `cols` × `rows` screen with `scrollback` rows.
pub fn render(bytes: &[u8], cols: u16, rows: u16, scrollback: usize) -> Rendered {
    let _ = (bytes, cols, rows, scrollback);
    Rendered::default()
}
