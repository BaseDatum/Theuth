use super::*;

/// Rows of live output a `shell` card shows for its background job.
pub const JOB_LINES: usize = 10;

/// The tail of a background job's output as it arrives (`JobEvent::Output`,
/// coalesced upstream to one chunk per half second per job): the last
/// [`JOB_LINES`] lines, frozen with how it ended. A PTY job (22 §5) instead
/// shows the last non-blank rows of its latest `JobEvent::Screen`. Bounded,
/// so any number of jobs costs the same; the log file is the whole story.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JobTail {
    /// Job id (`j3`).
    pub id: String,
    /// Complete lines, oldest first, at most [`JOB_LINES`].
    lines: VecDeque<String>,
    /// Bytes after the last newline (a progress bar's current state).
    partial: String,
    /// `exited 0 · 47s` and the like once finished.
    pub exit: Option<String>,
}

impl JobTail {
    /// An empty tail for job `id`.
    pub fn new(id: String) -> Self {
        Self {
            id,
            ..Self::default()
        }
    }

    /// Append raw output. Escape sequences are dropped, `\r` rewinds the
    /// current line (progress bars), tabs become spaces.
    pub fn push(&mut self, chunk: &str) {
        let mut chars = chunk.chars();
        while let Some(c) = chars.next() {
            match c {
                '\n' => {
                    if self.lines.len() == JOB_LINES {
                        self.lines.pop_front();
                    }
                    self.lines.push_back(std::mem::take(&mut self.partial));
                }
                '\r' => self.partial.clear(),
                '\t' => self.partial.push_str("    "),
                '\x1b' => match chars.next() {
                    // CSI: parameters, then one final byte in `@`..`~`.
                    Some('[') => {
                        for c in chars.by_ref() {
                            if ('@'..='~').contains(&c) {
                                break;
                            }
                        }
                    }
                    // OSC: to BEL or ST.
                    Some(']') => {
                        let mut prev = '\0';
                        for c in chars.by_ref() {
                            if c == '\x07' || (prev == '\x1b' && c == '\\') {
                                break;
                            }
                            prev = c;
                        }
                    }
                    _ => {}
                },
                c if c.is_control() => {}
                c => self.partial.push(c),
            }
        }
    }

    /// Replace the tail with the last [`JOB_LINES`] non-blank rows of a PTY
    /// job's screen (22 §5). The screen is a whole, not a stream: nothing
    /// carried over from before.
    pub fn set_screen(&mut self, rows: &[String]) {
        self.partial.clear();
        self.lines.clear();
        for r in rows
            .iter()
            .rev()
            .filter(|r| !r.trim().is_empty())
            .take(JOB_LINES)
        {
            self.lines.push_front(r.clone());
        }
    }

    /// The rows to draw, oldest first: complete lines and the partial one.
    pub fn rows(&self) -> impl Iterator<Item = &str> {
        let extra = usize::from(!self.partial.is_empty());
        self.lines
            .iter()
            .map(String::as_str)
            .skip((self.lines.len() + extra).saturating_sub(JOB_LINES))
            .chain((!self.partial.is_empty()).then_some(self.partial.as_str()))
    }

    /// Header row: `j3 · running` until it ends.
    pub fn header(&self) -> String {
        match &self.exit {
            Some(e) => format!("{} · {e}", self.id),
            None => format!("{} · running", self.id),
        }
    }
}

impl Model {

    /// The live block of job `id`, if a card on screen or off has one.
    pub(super) fn job_mut(&mut self, id: &str) -> Option<&mut JobTail> {
        self.entries.iter_mut().rev().find_map(|e| match e {
            Entry::Tool { job: Some(j), .. } if j.id == id => Some(j),
            _ => None,
        })
    }
}
