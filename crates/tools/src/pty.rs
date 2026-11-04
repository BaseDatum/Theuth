//! PTY jobs (design doc 22): `start` / `send` / `screen` / `wait` run a command
//! on a pseudo-terminal as a job on the [`Jobs`] actor (14) and show the model a
//! rendered 120×40 screen instead of the raw bytes. Same `j{n}` id, log file,
//! `jobs` / `cancel_job` and `<job_result>`; stdin, a screen and no deadline
//! are what differ (22 §1).

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::io::{Read, Seek, SeekFrom, Write};
use std::num::NonZeroU16;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use brush_core::Shell;
use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use platform::ProcessTree;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use regex::Regex;
use runtime::{Runtime, SessionId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::{Notify, broadcast, mpsc, oneshot};

use crate::activity::{self, Activity, Kind, Tracked};
use crate::jobs::{
    self, Cancel, CancelOrigin, Finished, Job, JobEvent, JobExit, JobId, Jobs, Launch, Log, Lost,
    admit_job, lock, sep,
};
use crate::journal::{JobKind, JobRow, JobState, Journal, PtyScreenRow, WaitKind, WaitRow};
use crate::proctree::{PtyThresholds, TreeQuiet};
use crate::{Risk, Tool, ToolCx, ToolOutput};

/// Screen height at `start` (22 §1.9); a viewer may resize it (`pty_resize`).
const ROWS: u16 = 40;
/// Screen width at `start` (22 §1.9).
const COLS: u16 = 120;
/// `pty_resize` bounds: a phone is ~40 columns, a wide monitor ~300.
const ROWS_MAX: u16 = 200;
const COLS_MAX: u16 = 400;
/// Output chunks a slow `subscribe_pty` client may fall behind before it is
/// resynced from a fresh snapshot.
const STREAM_CHUNKS: usize = 256;
/// `send` replies once no bytes arrived for this long (22 §4.5)…
const SETTLE_QUIET: Duration = Duration::from_millis(300);
/// …or after this long regardless.
const SETTLE_MAX: Duration = Duration::from_secs(2);
/// A tool reply carries `user attached` this long after a human typed (55 §0.5).
const USER_ATTACHED: Duration = Duration::from_secs(30);
/// `wait` default / maximum seconds (22 §4.6).
const WAIT_DEFAULT: u64 = 30;
const WAIT_MAX: u64 = 120;
/// `ready.timeout` maximum seconds (22 §4.7).
const READY_MAX: u64 = 300;
/// `start.idle_secs` below this is raised to it (61 §1.6); `0` stays "never".
const IDLE_MIN: u64 = 10;
/// Longest backoff between consecutive unanswered `<job_idle>` nudges (61 §1.4).
const IDLE_BACKOFF_MAX: Duration = Duration::from_secs(3600);
/// TCP connect poll spacing for `ready.port`.
const PORT_POLL: Duration = Duration::from_millis(250);
/// Longest terminal query the responder carries across chunks (22 §4.4).
const CARRY: usize = 16;
/// Reader thread buffer (22 §4.2).
const READ_BUF: usize = 64 * 1024;
/// Normal-screen scrollback rows retained per PTY (51 §7.1).
const HISTORY: usize = 512;

/// vt100 sizes are `NonZeroU16`; every size passed here is a non-zero constant or validated.
fn nz(n: u16) -> NonZeroU16 {
    NonZeroU16::new(n).expect("terminal size is non-zero")
}

/// `screen.size()` as plain `(rows, cols)`.
fn dims(screen: &vt100::Screen) -> (u16, u16) {
    let (rows, cols) = screen.size();
    (rows.get(), cols.get())
}

/// Longest `History` page (51 §7.3).
const PAGE_MAX: u16 = 80;
/// A serialized [`PtyPage`] is cut (top rows first) past this.
const PAGE_BYTES: usize = 256 * 1024;
/// Longest cell text on the wire (vt100 stores less; enforced anyway).
const CELL_BYTES: usize = 64;
/// Snapshot renders in flight at once, across every PTY (51 §7.4).
pub(crate) const SNAPSHOTS: usize = 4;
/// What one PTY is charged against [`jobs::PTY_BUDGET`] at `start`: every
/// retained row at a generous per-cell estimate plus parser overhead. Charged
/// once per handle; snapshot clones are transient and not charged.
pub(crate) const RESERVE: usize = (ROWS as usize + HISTORY) * COLS as usize * 96 + 4 * 1024;

/// One PTY job as `list_terminals` reports it (51 §7.3).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct TerminalSummary {
    /// Owning session.
    pub session: SessionId,
    /// Job.
    pub id: JobId,
    /// `start`'s label.
    pub name: Option<String>,
    /// Command line, scrubbed.
    pub command: String,
    /// [`JobView::state`](crate::JobView::state) wording.
    pub state: String,
    /// Screen height.
    pub rows: u16,
    /// Screen width.
    pub cols: u16,
    /// Bumps on every rendered change and once at exit; see [`PtyScreen::if_revision`].
    pub revision: u64,
    /// Daemon epoch; `None` until the harness has one.
    pub epoch: Option<String>,
}

/// Which rows of the retained terminal a [`PtyScreen`] asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "anchor", rename_all = "snake_case")]
pub enum PtyViewport {
    /// The 40 live rows.
    Live,
    /// `height` (≤ 80) scrollback rows whose bottom is `offset` rows above
    /// the live screen's top; clamped to what is retained.
    History {
        /// Rows between the page's bottom and the live screen.
        offset: u32,
        /// Rows in the page.
        height: u16,
    },
}

/// Reply to [`PtyScreen`].
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PtyScreenReply {
    /// A fresh page.
    Updated(PtyPage),
    /// `if_revision` still matches: nothing to send.
    Unchanged {
        /// As requested.
        viewport: PtyViewport,
        /// The current revision.
        revision: u64,
    },
    /// No such job, or its terminal was evicted.
    Unavailable,
}

/// One rendered page of a PTY job's retained terminal (51 §7.3). Styled
/// runs, never ANSI; secrets masked with `•`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PtyPage {
    /// Owning session.
    pub session: SessionId,
    /// Job.
    pub id: JobId,
    /// Revision rendered.
    pub revision: u64,
    /// Daemon epoch; `None` until the harness has one.
    pub epoch: Option<String>,
    /// As requested (height clamped).
    pub viewport: PtyViewport,
    /// Index of `lines[0]` in the retained buffer: `0` is the oldest
    /// scrollback row, `retained` the live screen's top.
    pub origin: u32,
    /// Scrollback rows retained right now (≤ 512; `0` on the alternate screen).
    pub retained: u32,
    /// Screen height.
    pub rows: u16,
    /// Screen width.
    pub cols: u16,
    /// The alternate screen is showing (no scrollback accrues).
    pub alternate: bool,
    /// `(row, col)` within `lines`, zero-based; `None` when hidden or the
    /// page is history.
    pub cursor: Option<(u16, u16)>,
    /// Per row of `lines`: it soft-wraps into the next.
    pub wrapped: Vec<bool>,
    /// Per row: adjacent cells sharing a style.
    pub lines: Vec<Vec<StyledRun>>,
    /// Top rows were dropped to fit the 256 KiB reply cap; `origin` moved down.
    pub truncated: bool,
    /// `false` when a secret could straddle the evicted-history boundary
    /// into this page (51 §7.2).
    pub redaction_complete: bool,
    /// Set once the job ended.
    pub exit: Option<JobExit>,
}

/// Adjacent cells with one [`Style`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct StyledRun {
    /// Shared style.
    pub style: Style,
    /// Cells in column order.
    pub cells: Vec<Cell>,
}

/// Cell attributes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[allow(missing_docs)]
pub struct Style {
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
}

/// A terminal colour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "kind", rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum Color {
    #[default]
    Default,
    Idx {
        idx: u8,
    },
    Rgb {
        r: u8,
        g: u8,
        b: u8,
    },
}

impl From<vt100::Color> for Color {
    fn from(c: vt100::Color) -> Self {
        match c {
            vt100::Color::Default => Color::Default,
            vt100::Color::Idx(idx) => Color::Idx { idx },
            vt100::Color::Rgb(r, g, b) => Color::Rgb { r, g, b },
        }
    }
}

/// One cell. `wide`/`continuation` are omitted on the wire when false.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Cell {
    /// Grapheme(s), `" "` when blank, `""` for a wide continuation, `"•"` when masked.
    pub text: String,
    /// Occupies this column and the next.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub wide: bool,
    /// The second column of a wide cell.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub continuation: bool,
}

/// Arguments for [`StartTool`] (22 §2.1).
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct StartArgs {
    /// Command line, run by `$SHELL -lc` on a 120×40 pseudo-terminal.
    pub command: String,
    /// Label; `send`/`screen`/`wait` accept it as `id` in place of `j{n}`.
    /// Unique among the session's running jobs.
    pub name: Option<String>,
    /// Working directory; default the session shell's.
    pub cwd: Option<PathBuf>,
    /// Extra environment variables.
    pub env: Option<BTreeMap<String, String>>,
    /// Block until the job is ready before replying (22 §1.7).
    pub ready: Option<Ready>,
    /// Seconds of unchanged screen before `<job_idle>` wakes the model
    /// (61 §2.1); default `tools.pty_idle_secs` (60), values 1–9 become 10,
    /// `0` = never.
    pub idle_secs: Option<u64>,
}

/// `start`'s readiness condition: every field given must pass.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct Ready {
    /// Rust regex (no look-around) matched against the output so far.
    pub log: Option<String>,
    /// TCP port that must accept a connection.
    pub port: Option<u16>,
    /// Host for `port`; default `127.0.0.1`.
    pub host: Option<String>,
    /// Seconds to wait; default `tools.pty_ready_timeout_secs` (30), max 300.
    pub timeout: Option<u64>,
}

/// Arguments for [`SendTool`] (22 §2.1).
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct SendArgs {
    /// `j4` or the job's `name`.
    pub id: String,
    /// Typed first.
    pub text: Option<String>,
    /// Append Enter after `text`; default true.
    pub enter: Option<bool>,
    /// Keys sent after `text`.
    pub keys: Option<Vec<Key>>,
    /// Signal delivered to the process group after the write.
    pub signal: Option<Signal>,
}

/// Arguments for [`ScreenTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ScreenArgs {
    /// `j4` or the job's `name`.
    pub id: String,
}

/// Arguments for [`WaitTool`] (22 §2.1).
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct WaitArgs {
    /// `j4` or the job's `name`.
    pub id: String,
    /// Rust regex (no look-around) over output since your last `send` (or
    /// the job's start) — a prompt already on screen matches at once;
    /// absent → wait for exit.
    pub pattern: Option<String>,
    /// Seconds; default 30, max 120.
    pub timeout: Option<u64>,
}

/// A key `send` (and a human over `pty_input`, 55 §2.2) types. Encoded by
/// [`encode`], which consults the screen's modes: arrows follow
/// application-cursor mode, `paste` is bracketed only when the program asked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum Key {
    Enter,
    Tab,
    ShiftTab,
    Escape,
    CtrlC,
    CtrlD,
    CtrlZ,
    CtrlL,
    /// Ctrl + a letter or one of `@ [ \ ] ^ _`.
    Ctrl(char),
    /// Alt (Meta) + a character.
    Alt(char),
    /// F1–F12.
    F(u8),
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Backspace,
    Delete,
    /// Text pasted in one piece.
    Paste(String),
}

impl Key {
    fn encode(&self, out: &mut Vec<u8>, screen: &vt100::Screen) -> Result<(), String> {
        let cursor = if screen.application_cursor() {
            b'O'
        } else {
            b'['
        };
        let bytes: &[u8] = match self {
            Key::Enter => b"\r",
            Key::Tab => b"\t",
            Key::ShiftTab => b"\x1b[Z",
            Key::Escape => b"\x1b",
            Key::CtrlC => b"\x03",
            Key::CtrlD => b"\x04",
            Key::CtrlZ => b"\x1a",
            Key::CtrlL => b"\x0c",
            Key::Ctrl(c) => {
                let c = c.to_ascii_uppercase();
                if !(c.is_ascii_uppercase() || matches!(c, '@' | '[' | '\\' | ']' | '^' | '_')) {
                    return Err(format!("ctrl+{c:?} is not a control character"));
                }
                out.push(c as u8 & 0x1f);
                return Ok(());
            }
            Key::Alt(c) => {
                out.push(0x1b);
                let mut b = [0; 4];
                out.extend_from_slice(c.encode_utf8(&mut b).as_bytes());
                return Ok(());
            }
            Key::F(n) => match n {
                1 => b"\x1bOP",
                2 => b"\x1bOQ",
                3 => b"\x1bOR",
                4 => b"\x1bOS",
                5 => b"\x1b[15~",
                6 => b"\x1b[17~",
                7 => b"\x1b[18~",
                8 => b"\x1b[19~",
                9 => b"\x1b[20~",
                10 => b"\x1b[21~",
                11 => b"\x1b[23~",
                12 => b"\x1b[24~",
                _ => return Err(format!("f{n}: only F1–F12 exist")),
            },
            Key::Up | Key::Down | Key::Right | Key::Left => {
                let c = match self {
                    Key::Up => b'A',
                    Key::Down => b'B',
                    Key::Right => b'C',
                    _ => b'D',
                };
                out.extend_from_slice(&[0x1b, cursor, c]);
                return Ok(());
            }
            Key::Home => b"\x1b[H",
            Key::End => b"\x1b[F",
            Key::PageUp => b"\x1b[5~",
            Key::PageDown => b"\x1b[6~",
            Key::Insert => b"\x1b[2~",
            Key::Backspace => b"\x7f",
            Key::Delete => b"\x1b[3~",
            Key::Paste(text) => {
                if screen.bracketed_paste() {
                    out.extend_from_slice(b"\x1b[200~");
                    out.extend_from_slice(text.as_bytes());
                    out.extend_from_slice(b"\x1b[201~");
                } else {
                    out.extend_from_slice(text.as_bytes());
                }
                return Ok(());
            }
        };
        out.extend_from_slice(bytes);
        Ok(())
    }
}

/// `text` then `keys` as the program expects them right now (55 §2.2): the
/// one key table, read under the parser lock so the modes are current.
fn encode(pty: &PtyHandle, text: Option<&str>, keys: &[Key]) -> Result<Vec<u8>, String> {
    let mut out = text.map(|t| t.as_bytes().to_vec()).unwrap_or_default();
    let p = lock(&pty.parser);
    for k in keys {
        k.encode(&mut out, p.screen())?;
    }
    Ok(out)
}

/// A signal `send` delivers to the job's process group.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum Signal {
    Int,
    Term,
    Hup,
    Quit,
    Kill,
}

impl From<Signal> for platform::Signal {
    fn from(s: Signal) -> Self {
        match s {
            Signal::Int => Self::Int,
            Signal::Term => Self::Term,
            Signal::Hup => Self::Hup,
            Signal::Quit => Self::Quit,
            Signal::Kill => Self::Kill,
        }
    }
}

/// What a running PTY job shares with the tools (22 §4.8): the master's
/// writer, the rendered screen, an output wake-up and the process group.
/// `writer` and `master` are the handle's two descriptors (both dups of the
/// master); [`PtyHandle::release`] drops them at exit, and the retained
/// row answers from the parser's final screen alone (D2).
pub(crate) struct PtyHandle {
    /// `None` once released: a late `send` sees "job finished".
    writer: Mutex<Option<Box<dyn Write + Send>>>,
    parser: Mutex<vt100::Parser>,
    /// Bumped under `parser`'s lock after each `process` and once at exit
    /// (51 §7.3): equal revisions mean an identical retained buffer.
    revision: AtomicU64,
    output: Notify,
    /// Raw output as it arrived, for `subscribe_pty` viewers running their
    /// own emulator; sent under `parser`'s lock so a subscriber that took its
    /// snapshot under the same lock sees every byte after it exactly once.
    stream: broadcast::Sender<PtyOut>,
    /// The master side, kept for `resize`; released with `writer` at exit.
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    /// `== pgid`: the child is a session leader.
    pid: u32,
    /// The child's process tree (58 §0.4): killed on cancel, signalled by `send`.
    tree: ProcessTree,
    /// Set by the job task just before it finishes; `send`/`wait` loops read it.
    exit: Mutex<Option<JobExit>>,
    /// When a human last typed into this terminal (`pty_input`, 55 §2.3);
    /// tool replies say so for [`USER_ATTACHED`] after.
    user_input: Mutex<Option<Instant>>,
    /// Last `send` / `pty_input` / `screen` reply / `wait` return: restarts
    /// the idle timer (61 §1.3). `idle_wake` tells the job task to re-read it.
    touched: Mutex<Instant>,
    idle_wake: Notify,
    /// Outstanding `wait`s: while any, a quiet screen does not fire (61 §1.5).
    waiting: AtomicUsize,
    /// The model touched the job (`send`/`screen`/`wait`) since the last
    /// nudge fired: the next stretch runs at the base interval (61 §1.4).
    interacted: AtomicBool,
    /// `wait`'s read cursor into the log (55 §12): where the model last
    /// acted — reset to the log length by `send`, moved past each `wait`
    /// match — so a prompt already on screen matches at once.
    cursor: AtomicU64,
    /// The revision last written to `pty_screens` (109 §5.3); `u64::MAX`
    /// before the first write. Equal revisions skip the write.
    snapshotted: AtomicU64,
    /// 155 §6.4: when the model last `send`-wrote, until the screen or the
    /// process tree moved (then `None`); the reaction check's clock.
    sent_at: Mutex<Option<Instant>>,
}

/// One item of [`PtyHandle::stream`].
#[derive(Clone, Debug)]
pub enum PtyOut {
    /// Bytes the program wrote, verbatim.
    Data(Arc<[u8]>),
    /// The job ended; nothing follows.
    Exit(JobExit),
}

/// The screen cloned under the lock, with what else a page reports.
pub(crate) struct Capture {
    screen: vt100::Screen,
    revision: u64,
    exit: Option<JobExit>,
}

/// [`render`]'s result: a [`PtyPage`] short of its job identity, and the
/// text rows the tools show.
pub(crate) struct PtySnapshot {
    revision: u64,
    viewport: PtyViewport,
    origin: u32,
    retained: u32,
    rows: u16,
    cols: u16,
    alternate: bool,
    cursor_pos: (u16, u16),
    cursor_hidden: bool,
    wrapped: Vec<bool>,
    lines: Vec<Vec<StyledRun>>,
    redaction_complete: bool,
    exit: Option<JobExit>,
}

impl PtyHandle {
    pub(crate) fn revision(&self) -> u64 {
        self.revision.load(SeqCst)
    }

    /// Clone the screen under the lock (≈ 2 MiB at most; the render happens
    /// on the clone, never on the live parser).
    pub(crate) fn capture(&self) -> Capture {
        let (screen, revision) = {
            let p = lock(&self.parser);
            (p.screen().clone(), self.revision.load(SeqCst))
        };
        Capture {
            screen,
            revision,
            exit: self.exit(),
        }
    }

    /// One page, rendered outside the lock with the installed scrubber.
    pub(crate) fn snapshot(&self, viewport: PtyViewport) -> PtySnapshot {
        render(
            self.capture(),
            viewport,
            &crate::secrets::redact_spans,
            crate::secrets::max_literal_len(),
        )
    }

    /// Current screen per 22 §4.3, scrubbed of vault values (37 §3.4) by
    /// the same masking pass as [`PtyPage`]: this is the one source for the
    /// `screen` tool, `JobView.tail`, `JobEvent::Screen` and the final
    /// `<job_result>` of a pty job.
    pub(crate) fn screen(&self) -> (Vec<String>, (u16, u16)) {
        let s = self.snapshot(PtyViewport::Live);
        (s.text(), s.cursor_pos)
    }

    /// Drop the master and its writer (D2): the slave side gets its SIGHUP
    /// once the reader's dup goes too, a late `resize`/`send` sees "job
    /// finished", and the row keeps only the parser. Idempotent.
    pub(crate) fn release(&self) {
        drop(lock(&self.master).take());
        drop(lock(&self.writer).take());
    }

    /// D2 probe: does the handle still own a pty descriptor?
    #[cfg(test)]
    pub(crate) fn holds_fds(&self) -> bool {
        lock(&self.master).is_some() || lock(&self.writer).is_some()
    }

    fn exit(&self) -> Option<JobExit> {
        lock(&self.exit).clone()
    }

    /// Current `(rows, cols)`.
    pub(crate) fn size(&self) -> (u16, u16) {
        dims(lock(&self.parser).screen())
    }

    /// Resize the grid and tell the program (SIGWINCH). The parser learns
    /// the size first so the redraw it triggers is parsed at the new width.
    pub(crate) fn resize(&self, rows: u16, cols: u16) -> Result<(), String> {
        if !(2..=ROWS_MAX).contains(&rows) || !(10..=COLS_MAX).contains(&cols) {
            return Err(format!(
                "size {cols}x{rows} out of range (10..={COLS_MAX} x 2..={ROWS_MAX})"
            ));
        }
        let master = lock(&self.master);
        let Some(master) = master.as_ref() else {
            return Err("job finished".into());
        };
        {
            let mut p = lock(&self.parser);
            if dims(p.screen()) == (rows, cols) {
                return Ok(());
            }
            p.screen_mut().set_size(nz(rows), nz(cols));
            self.revision.fetch_add(1, SeqCst);
        }
        master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("resize pty: {e:#}"))
    }

    /// Subscribe and snapshot under one lock: the replay plus every byte
    /// after it is the whole terminal. The replay is formatted off the lock.
    pub(crate) fn attach(
        &self,
    ) -> (
        Vec<u8>,
        (u16, u16),
        broadcast::Receiver<PtyOut>,
        Option<JobExit>,
    ) {
        let (screen, rx) = {
            let p = lock(&self.parser);
            (p.screen().clone(), self.stream.subscribe())
        };
        let size = dims(&screen);
        (replay(screen), size, rx, self.exit())
    }

    /// 109 §5.3: write the live screen as the job's `pty_screens` row when
    /// the revision moved since the last write. Called as `screen` / `send`
    /// / `wait` return, on the snapshot timer, and at exit.
    pub(crate) async fn snapshot_to_store(
        &self,
        journal: &dyn Journal,
        session: SessionId,
        id: &JobId,
    ) {
        let snap = self.snapshot(PtyViewport::Live);
        if self.snapshotted.load(SeqCst) == snap.revision {
            return;
        }
        let (rows, cols) = (snap.rows, snap.cols);
        let (cursor_row, cursor_col) = snap.cursor_pos;
        let row = PtyScreenRow {
            session,
            job_id: id.clone(),
            revision: snap.revision,
            rows,
            cols,
            cursor_row,
            cursor_col,
            text: snap.text().join("\n"),
            at_ms: chrono::Utc::now().timestamp_millis(),
        };
        self.snapshotted.store(snap.revision, SeqCst);
        journal.pty_screen(row).await;
    }

    /// Someone touched the job (61 §1.3): the idle timer restarts. `model`
    /// (a `send`/`screen`/`wait`, not a human's `pty_input`) also resets the
    /// backoff (61 §1.4).
    fn touch(&self, model: bool) {
        *lock(&self.touched) = Instant::now();
        if model {
            self.interacted.store(true, SeqCst);
        }
        self.idle_wake.notify_one();
    }
}

/// Escape sequences that rebuild a terminal from the retained buffer: every
/// scrollback row on its own line, pushed off the top so the viewer's own
/// scrollback holds it, then the live screen with its cursor and input modes.
fn replay(mut screen: vt100::Screen) -> Vec<u8> {
    let (rows, cols) = dims(&screen);
    let mut out = Vec::new();
    if !screen.alternate_screen() {
        screen.set_scrollback(usize::MAX);
        let retained = screen.scrollback();
        for off in (1..=retained).rev() {
            screen.set_scrollback(off);
            if let Some(row) = screen.rows_formatted(0, cols).next() {
                out.extend_from_slice(row.as_bytes());
            }
            out.extend_from_slice(b"\x1b[m\r\n");
        }
        screen.set_scrollback(0);
        for _ in 0..rows {
            out.extend_from_slice(b"\r\n");
        }
    }
    out.extend_from_slice(screen.state_formatted().as_bytes());
    out
}

impl PtySnapshot {
    /// Rows right-trimmed, trailing blank rows dropped (22 §4.3).
    fn text(&self) -> Vec<String> {
        let mut rows: Vec<String> = self
            .lines
            .iter()
            .map(|runs| {
                let mut r: String = runs
                    .iter()
                    .flat_map(|run| run.cells.iter().map(|c| c.text.as_str()))
                    .collect();
                r.truncate(r.trim_end().len());
                r
            })
            .collect();
        let last = rows
            .iter()
            .rposition(|r| !r.is_empty())
            .map_or(0, |i| i + 1);
        rows.truncate(last);
        rows
    }

    /// The wire page, top rows dropped by halves until it serializes under
    /// [`PAGE_BYTES`] (never below one row).
    pub(crate) fn into_page(self, session: SessionId, id: JobId) -> PtyPage {
        let cursor =
            (!self.cursor_hidden && self.viewport == PtyViewport::Live).then_some(self.cursor_pos);
        let mut page = PtyPage {
            session,
            id,
            revision: self.revision,
            epoch: None,
            viewport: self.viewport,
            origin: self.origin,
            retained: self.retained,
            rows: self.rows,
            cols: self.cols,
            alternate: self.alternate,
            cursor,
            wrapped: self.wrapped,
            lines: self.lines,
            truncated: false,
            redaction_complete: self.redaction_complete,
            exit: self.exit,
        };
        while page.lines.len() > 1 && serde_json::to_vec(&page).is_ok_and(|v| v.len() > PAGE_BYTES)
        {
            let drop = page.lines.len() / 2;
            page.lines.drain(..drop);
            page.wrapped.drain(..drop);
            page.origin += drop as u32;
            page.truncated = true;
            page.cursor = page
                .cursor
                .and_then(|(r, c)| r.checked_sub(drop as u16).map(|r| (r, c)));
        }
        page
    }
}

/// A page cell before styling into runs, with its span in the logical text.
struct Raw {
    text: String,
    style: Style,
    wide: bool,
    continuation: bool,
    at: usize,
    len: usize,
}

fn style_of(c: &vt100::Cell) -> Style {
    Style {
        fg: c.fgcolor().into(),
        bg: c.bgcolor().into(),
        bold: c.bold(),
        dim: c.dim(),
        italic: c.italic(),
        underline: c.underline(),
        inverse: c.inverse(),
    }
}

/// First ≤ [`CELL_BYTES`] bytes on a char boundary.
fn clip(t: &str) -> &str {
    let mut n = t.len().min(CELL_BYTES);
    while !t.is_char_boundary(n) {
        n -= 1;
    }
    &t[..n]
}

/// 51 §7.2–7.3: page the retained buffer (`0` = oldest scrollback row,
/// `retained` = live top), build the logical text of the whole buffer —
/// soft-wrapped rows joined, others `\n`-separated, wide continuations
/// skipped — run `spans` over it, mask every page cell a span touches with
/// `•` (style dropped, width kept), then merge same-style cells into runs.
/// `spans` must return ascending, non-overlapping byte ranges; `max_len` is
/// the longest literal it can match (`0`: nothing installed, page rows only).
fn render(
    cap: Capture,
    viewport: PtyViewport,
    spans: &dyn Fn(&str) -> Vec<Range<usize>>,
    max_len: usize,
) -> PtySnapshot {
    let Capture {
        mut screen,
        revision,
        exit,
    } = cap;
    screen.set_scrollback(usize::MAX);
    let retained = screen.scrollback();
    let (rows, cols) = dims(&screen);
    let live = usize::from(rows);
    let (start, end, viewport) = match viewport {
        PtyViewport::Live => (retained, retained + live, viewport),
        PtyViewport::History { offset, height } => {
            let height = height.clamp(1, PAGE_MAX);
            let end = retained - (offset as usize).min(retained);
            (
                end.saturating_sub(usize::from(height)),
                end,
                PtyViewport::History { offset, height },
            )
        }
    };
    let scan = if max_len > 0 {
        0..retained + live
    } else {
        start..end
    };
    let mut text = String::new();
    let mut page: Vec<Vec<Raw>> = Vec::with_capacity(end - start);
    let mut wrapped = Vec::with_capacity(end - start);
    let mut page_at = 0;
    for i in scan {
        // Row `i` is visible row 0 at offset `retained - i` while in
        // scrollback, else live row `i - retained`. `set_scrollback` is a
        // field write; `cell` is O(1) over the visible rows.
        screen.set_scrollback(retained.saturating_sub(i));
        let r = i.saturating_sub(retained) as u16;
        let in_page = (start..end).contains(&i);
        if in_page && i == start {
            page_at = text.len();
        }
        let mut row = in_page.then(|| Vec::with_capacity(usize::from(cols)));
        for c in 0..cols {
            let at = text.len();
            let cell = screen.cell(r, c);
            let continuation = cell.is_some_and(vt100::Cell::is_wide_continuation);
            if !continuation {
                match cell.map(vt100::Cell::contents).filter(|t| !t.is_empty()) {
                    Some(t) => text.push_str(clip(t)),
                    None => text.push(' '),
                }
            }
            if let Some(row) = &mut row {
                row.push(Raw {
                    text: text[at..].to_owned(),
                    style: cell.map(style_of).unwrap_or_default(),
                    wide: cell.is_some_and(vt100::Cell::is_wide),
                    continuation,
                    at,
                    len: text.len() - at,
                });
            }
        }
        let soft = screen.row_wrapped(r);
        if let Some(row) = row {
            page.push(row);
            wrapped.push(soft);
        }
        if !soft {
            text.push('\n');
        }
    }
    let mut ranges = if max_len > 0 {
        spans(&text)
    } else {
        Vec::new()
    }
    .into_iter()
    .peekable();
    let mut prev_masked = false;
    for cell in page.iter_mut().flatten() {
        while ranges.peek().is_some_and(|s| s.end <= cell.at) {
            ranges.next();
        }
        let hit = if cell.continuation {
            prev_masked
        } else {
            ranges.peek().is_some_and(|s| s.start < cell.at + cell.len)
        };
        if hit {
            cell.text = if cell.continuation {
                String::new()
            } else {
                "•".to_owned()
            };
            cell.style = Style::default();
        }
        prev_masked = hit;
    }
    let lines = page
        .into_iter()
        .map(|row| {
            let mut runs: Vec<StyledRun> = Vec::new();
            for c in row {
                let cell = Cell {
                    text: c.text,
                    wide: c.wide,
                    continuation: c.continuation,
                };
                match runs.last_mut() {
                    Some(run) if run.style == c.style => run.cells.push(cell),
                    _ => runs.push(StyledRun {
                        style: c.style,
                        cells: vec![cell],
                    }),
                }
            }
            runs
        })
        .collect();
    // A literal that began in evicted rows can only reach this page if the
    // page starts within `max_len - 1` bytes of the retained boundary.
    let redaction_complete = !(max_len > 0 && retained >= HISTORY && page_at < max_len - 1);
    PtySnapshot {
        revision,
        viewport,
        origin: start as u32,
        retained: retained as u32,
        rows,
        cols,
        alternate: screen.alternate_screen(),
        cursor_pos: screen.cursor_position(),
        cursor_hidden: screen.hide_cursor(),
        wrapped,
        lines,
        redaction_complete,
        exit,
    }
}

/// `ask` → [`ToolOutput`]: spawn `args.command` on a pty as a job, wait for
/// grace or `ready`, reply per 22 §2.2.
pub struct StartPty {
    /// Owning session.
    pub session: SessionId,
    /// Provider call id (carried on `JobEvent::Started`).
    pub call_id: String,
    /// The tool arguments.
    pub args: StartArgs,
    /// Resolved working directory.
    pub cwd: PathBuf,
    /// `ready` deadline when `args.ready.timeout` is absent.
    pub ready_timeout: Duration,
    /// Screen-quiet interval before `<job_idle>` (61); `None` = never.
    pub idle_after: Option<Duration>,
    /// The session's [`crate::SessionResources::root`] (51 §4.5): the log
    /// is `<root>/jobs/<session>/<id>.log`.
    pub root: PathBuf,
    /// [`activity::current`] at the caller; see [`jobs::Start::activity`].
    /// The readiness wait runs under it too.
    pub activity: Option<Activity>,
    /// `tools.pty_shell`: the program instead of the platform default (58 §0.3).
    pub pty_shell: Option<PathBuf>,
    /// 171: explicit program instead of the user shell; E1 honours it.
    pub program: Option<(PathBuf, Vec<String>)>,
    /// 171: a user-cell kernel; marks are sent here; E1.
    pub cell: Option<mpsc::Sender<Mark>>,
}

/// 171 §5.5: a shell-integration mark scanned from a `cell` PTY job's log
/// (E1 implements the scan).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Mark {
    /// Which sequence was seen.
    pub kind: MarkKind,
    /// The exit code carried by `OSC 133;D;n`.
    pub exit: Option<i32>,
    /// The directory carried by `OSC 7`.
    pub cwd: Option<String>,
    /// Byte offset of the sequence in the log file as written.
    pub offset: u64,
}

/// The shell-integration sequences a [`Mark`] reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum MarkKind {
    /// `OSC 133;A`: the kernel's rc finished.
    Ready,
    /// `OSC 133;C`: a run's output starts.
    Start,
    /// `OSC 133;D[;n]`: a run ended with exit `n`.
    End,
    /// `OSC 7 ;file://…`: the live working directory.
    Cwd,
}

/// `ask` → `Result<(), String>`: the user's Stop / Kill on a kernel (171
/// §7.3). `close: false` SIGKILLs the pty's foreground process group when it
/// is not the shell's own (the kernel survives; nothing to kill is `Ok`);
/// `close: true` ends the job as a shutdown would. Err `no such job` |
/// `not a cell job` | `j3 already finished`.
pub struct CellKill {
    /// Owning session.
    pub session: SessionId,
    /// The kernel job.
    pub id: JobId,
    /// Kill the whole kernel, not just what it is running.
    pub close: bool,
}

/// `ask` → `Result<bool, String>`: is the kernel shell itself the pty's
/// foreground process group (at its prompt or running builtins)? `false`
/// while an external command runs. Err as [`CellKill`]. Always `true` on an
/// OS without the notion.
pub struct CellForeground {
    /// Owning session.
    pub session: SessionId,
    /// The kernel job.
    pub id: JobId,
}

/// `ask` → `Result<ToolOutput, String>`: type into a PTY job, reply with the
/// settled screen (22 §4.5).
pub struct SendInput {
    /// Owning session.
    pub session: SessionId,
    /// The tool arguments (`id` inside).
    pub args: SendArgs,
    /// [`activity::current`] at the caller: the settle wait runs under it,
    /// so it counts until the screen actually settled even if the asker is gone.
    pub activity: Option<Activity>,
}

/// `ask` → `Result<ToolOutput, String>`: the current screen.
pub struct Screen {
    /// Owning session.
    pub session: SessionId,
    /// `j4` or the job's `name`.
    pub id: String,
}

/// `ask` → `Result<ToolOutput, String>`: block for a pattern or exit (22 §4.6).
pub struct Wait {
    /// Owning session.
    pub session: SessionId,
    /// Provider call id: the `pty_wait` row's key (109 §4.4); empty = no row.
    pub call_id: String,
    /// The tool arguments (`id` inside).
    pub args: WaitArgs,
    /// [`activity::current`] at the caller: the wait runs under it.
    pub activity: Option<Activity>,
}

/// `ask` → `Vec<TerminalSummary>`: every PTY job of these sessions whose
/// terminal is still retained (51 §7.3). The RPC resolves the subtree.
pub struct ListTerminals {
    /// Sessions to list.
    pub sessions: Vec<SessionId>,
}

/// `ask` → `Result<PtyScreenReply, String>`: one page of a PTY job's
/// retained terminal (51 §7.3). `Err` = every render slot is busy; retry.
pub struct PtyScreen {
    /// Owning session.
    pub session: SessionId,
    /// Job.
    pub id: JobId,
    /// Which rows.
    pub viewport: PtyViewport,
    /// The revision the caller already has: equal → `Unchanged`.
    pub if_revision: Option<u64>,
}

/// `ask` → `Result<(), String>`: a human types into a running PTY job
/// (55 §2.1). Bytes are written and the call returns; no settle, no screen —
/// the viewer is already polling `pty_screen`. `Err` names the job's exit or
/// the write failure.
pub struct PtyInput {
    /// Owning session.
    pub session: SessionId,
    /// Job (exact id; names are a tool convenience).
    pub id: JobId,
    /// Typed first, verbatim.
    pub text: Option<String>,
    /// Keys after `text`.
    pub keys: Vec<Key>,
}

/// `ask` → `Result<PtyAttached, String>`: a viewer with its own terminal
/// emulator attaches to a PTY job: the retained buffer as escape sequences,
/// then every byte the program writes, on a broadcast receiver. `size`
/// resizes the pty first (a running job only). `Err` = no such job / not a
/// PTY job / a bad size.
pub struct PtyAttach {
    /// Owning session.
    pub session: SessionId,
    /// Job (exact id).
    pub id: JobId,
    /// `(rows, cols)` the viewer shows.
    pub size: Option<(u16, u16)>,
}

/// Reply to [`PtyAttach`].
#[derive(kameo::Reply)]
#[allow(missing_docs)]
pub struct PtyAttached {
    /// Rebuilds the retained buffer and live screen in a fresh terminal.
    pub replay: Vec<u8>,
    /// Grid size the replay assumes.
    pub rows: u16,
    pub cols: u16,
    /// Everything after `replay`, in order; `Lagged` means re-attach.
    pub rx: broadcast::Receiver<PtyOut>,
    /// Set when the job already ended: `rx` yields nothing more.
    pub exit: Option<JobExit>,
}

/// `ask` → `Result<(), String>`: resize a running PTY job's grid; the
/// program gets SIGWINCH. `Err` names the job's exit, a non-PTY job, or a
/// size out of range.
#[allow(missing_docs)]
pub struct PtyResize {
    /// Owning session.
    pub session: SessionId,
    /// Job (exact id).
    pub id: JobId,
    pub rows: u16,
    pub cols: u16,
}

/// One `subscribe_pty` item.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum PtyChunk {
    /// Start over: reset the terminal to `rows`×`cols` and write `data`
    /// (base64). First item, and again after the client fell behind.
    Reset { data: String, rows: u16, cols: u16 },
    /// Program output (base64), in order.
    Data { data: String },
    /// The job ended; the subscription closes.
    Exit { exit: JobExit },
}

/// A resolved PTY job as the tool paths see it, detached from the actor.
struct Live {
    session: SessionId,
    id: JobId,
    name: Option<String>,
    log_path: PathBuf,
    started: Instant,
    pty: Arc<PtyHandle>,
    /// Where the screen snapshot goes as a reply returns (109 §5.3).
    journal: Arc<dyn Journal>,
}

impl Live {
    /// 109 §5.3: the model just saw this screen.
    async fn snapshot(&self) {
        self.pty
            .snapshot_to_store(&*self.journal, self.session, &self.id)
            .await;
    }
}

impl Jobs {
    /// 171 §9.7, 171F B23: `id` (a job id or name, live, finished or lost)
    /// is a user-cell kernel — the model neither types into nor reads one.
    fn kernel(&self, session: SessionId, id: &str) -> bool {
        jobs::is_cell(Some(id))
            || self.sessions.get(&session).is_some_and(|s| {
                s.jobs
                    .iter()
                    .any(|j| j.id.0 == id && jobs::is_cell(j.name.as_deref()))
            })
    }

    /// `id` resolution (22 §4.8): exact `j{n}`, else a running job named `id`,
    /// else a finished one for the error text.
    fn pty(&self, session: SessionId, id: &str) -> Result<Live, String> {
        let jobs = self
            .sessions
            .get(&session)
            .map_or(&[][..], |s| s.jobs.as_slice());
        let named = |j: &&Job| j.name.as_deref() == Some(id);
        let job = jobs
            .iter()
            .find(|j| j.id.0 == id)
            .or_else(|| jobs.iter().rev().filter(|j| j.exit.is_none()).find(named))
            .or_else(|| jobs.iter().rev().find(named))
            .ok_or_else(|| "no such job".to_owned())?;
        if let Some(exit) = &job.exit {
            return Err(format!(
                "{} {exit} — start it again",
                label(&job.id, job.name.as_deref())
            ));
        }
        let Some(pty) = &job.pty else {
            return Err(format!(
                "{} is a background `shell` job, not a PTY: `wait`/`send`/`screen` do not apply. \
                 Its result arrives as <job_result id=\"{}\"> when it exits — keep working or end the turn.",
                job.id, job.id
            ));
        };
        Ok(Live {
            session,
            id: job.id.clone(),
            name: job.name.clone(),
            log_path: job.log_path.clone(),
            started: job.started,
            pty: Arc::clone(pty),
            journal: Arc::clone(&self.journal),
        })
    }

    /// 109 §5.3: the `screen` / `send` reply for a PTY job a previous engine
    /// lost — the stored page under the "process gone" banner; `None` when
    /// `id` is not such a job.
    fn lost_page(&self, session: SessionId, id: &str, note: Option<&str>) -> Option<ToolOutput> {
        let job = self.lost(session, id)?;
        let lost = job.lost.as_ref()?;
        let screen = lost.screen.as_ref()?;
        let rows: Vec<String> = if screen.text.is_empty() {
            Vec::new()
        } else {
            screen.text.split('\n').map(str::to_owned).collect()
        };
        let cursor = (screen.cursor_row, screen.cursor_col);
        let captured = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(screen.at_ms)
            .map_or_else(|| "?".to_owned(), |t| hhmmss(&t));
        let mut content = format!(
            "job {} · lost · {}s\n",
            label(&job.id, job.name.as_deref()),
            job.secs
        );
        if let Some(n) = note {
            content.push_str(n);
            content.push('\n');
        }
        let (gone, on) = match &lost.reason {
            crate::jobs::LostReason::Restart => (
                format!("the engine restarted at {}", hhmmss(&lost.restarted_at)),
                String::new(),
            ),
            // 139 §7.2: the page is the previous owner's, replicated.
            crate::jobs::LostReason::OwnerChanged { engine } => (
                format!(
                    "the session moved from {engine} at {}",
                    hhmmss(&lost.restarted_at)
                ),
                format!(" on {engine}"),
            ),
        };
        let _ = writeln!(
            content,
            "[process gone — {gone}; this is the last screen captured{on} at {captured} (revision {})]",
            screen.revision
        );
        content.push_str(&screen_block(&rows, cursor));
        Some(ToolOutput {
            content,
            is_error: false,
            structured: Some(serde_json::json!({
                "job": job.id, "rows": rows, "cursor": cursor, "lost": true,
                "restarted_at": lost.restarted_at.to_rfc3339(),
            })),
            files: Vec::new(),
        })
    }
}

/// What the `start` asker waits on after [`Launch`].
pub(crate) struct Running {
    done: oneshot::Receiver<ToolOutput>,
    claimable: Arc<AtomicBool>,
    pty: Arc<PtyHandle>,
    log_path: PathBuf,
}

impl Message<StartPty> for Jobs {
    type Reply = DelegatedReply<ToolOutput>;

    async fn handle(
        &mut self,
        mut m: StartPty,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let re = match m
            .args
            .ready
            .as_ref()
            .and_then(|r| r.log.as_deref())
            .map(Regex::new)
        {
            Some(Err(e)) => {
                tx.send(ToolOutput::err(format!("ready.log: {e}")));
                return reply;
            }
            Some(Ok(re)) => Some(re),
            None => None,
        };
        // 171 §12.1 inv. 3: the `cell:` namespace belongs to kernel jobs, and
        // a kernel job is always in it.
        let cell = m.cell.is_some();
        if jobs::is_cell(m.args.name.as_deref()) != cell {
            tx.send(ToolOutput::err(if cell {
                "a user-cell job must be named `cell:<label>`".to_owned()
            } else {
                format!(
                    "names starting with \"{}\" are reserved for user cells",
                    jobs::CELL_PREFIX
                )
            }));
            return reply;
        }
        let dir = m.root.join("jobs").join(m.session.to_string());
        let id = self.sessions.entry(m.session).or_default().alloc(&dir);
        let sink = self.activity.clone();
        let me = ctx.actor_ref().clone();
        let grace = self.grace;
        let journal = Arc::clone(&self.journal);
        let session = m.session;
        // Admission leaves the mailbox (47 §4.1); `Launch` spawns under the
        // actor's state; the readiness wait below runs under the caller's
        // invocation so it counts until it actually ends (47 §4.2 rule 3).
        let under = m.activity.take();
        let parent = under.clone();
        let waiter = async move {
            let fail = |why: String| ToolOutput::err(format!("cannot start job: {why}"));
            let activity =
                match admit_job(parent, sink.as_ref(), m.session, Kind::Pty(id.clone())).await {
                    Ok(a) => a,
                    Err(why) => return tx.send(fail(why.to_string())),
                };
            let ready = m.args.ready.take();
            let name = m.args.name.clone();
            let ready_timeout = m.ready_timeout;
            let Running {
                mut done,
                claimable,
                pty,
                log_path,
            } = match me
                .ask(Launch {
                    m,
                    id: id.clone(),
                    activity,
                })
                .await
            {
                Ok(r) => r,
                Err(e) => return tx.send(fail(e.to_string())),
            };
            // `Some`: the task finished; `None`: grace / readiness ended first.
            // A kernel (171 §5.3) answers at once: the cells actor waits on
            // the ready mark, not on the screen.
            let (note, within) = match ready {
                None if cell || grace.is_zero() => (String::new(), None),
                None => (
                    String::new(),
                    tokio::time::timeout(grace, &mut done).await.ok(),
                ),
                Some(r) => {
                    let secs = r.timeout.unwrap_or(ready_timeout.as_secs()).min(READY_MAX);
                    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
                    let host = r.host.as_deref().unwrap_or("127.0.0.1");
                    let port = r.port.map(|p| (host, p));
                    tokio::select! {
                        r = &mut done => (String::new(), Some(r)),
                        o = await_output(&pty, &log_path, 0, re.as_ref(), port, deadline) => match o {
                            Outcome::Exited => (String::new(), Some((&mut done).await)),
                            Outcome::Ready(matched) => {
                                let mut parts = Vec::new();
                                if let Some((m, _)) = matched {
                                    parts.push(format!("log matched `{m}`"));
                                }
                                if let Some((_, p)) = port {
                                    parts.push(format!("port {p}"));
                                }
                                (format!(" · ready: {}", parts.join(", ")), None)
                            }
                            Outcome::TimedOut(pending) => (
                                format!(" · not ready after {secs}s (waiting on: {})", pending.join(", ")),
                                None,
                            ),
                        },
                    }
                }
            };
            let done = match within {
                Some(r) => r.ok(),
                None if claimable.swap(false, SeqCst) => None,
                None => done.await.ok(),
            };
            let out = match done {
                Some(out) => out,
                None => {
                    let (rows, cursor) = pty.screen();
                    let mut content = format!(
                        "job {} · running · pid {}{note}\nlog: {}\n",
                        label(&id, name.as_deref()),
                        pty.pid,
                        log_path.display()
                    );
                    content.push_str(&screen_block(&rows, cursor));
                    // 109 §5.3: the model saw this screen.
                    pty.snapshot_to_store(&*journal, session, &id).await;
                    ToolOutput {
                        content,
                        is_error: false,
                        structured: Some(serde_json::json!({
                            "job": id, "log": log_path, "pid": pty.pid, "rows": rows, "cursor": cursor,
                        })),
                        files: Vec::new(),
                    }
                }
            };
            tx.send(out);
        };
        tokio::spawn(
            Tracked {
                message: (),
                activity: under,
            }
            .scope(|()| waiter),
        );
        reply
    }
}

impl Message<Launch<StartPty>> for Jobs {
    type Reply = Result<Running, String>;

    async fn handle(
        &mut self,
        l: Launch<StartPty>,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let Launch { m, id, activity } = l;
        // Checked here, serialized with the push, so two concurrent `start`s
        // cannot both take a name. On `Err` the admitted guard drops: settled
        // `Finished`, no blocker.
        if let Some(name) = &m.args.name
            && let Some(j) = self.sessions.get(&m.session).and_then(|s| {
                s.jobs
                    .iter()
                    .find(|j| j.exit.is_none() && j.name.as_deref() == Some(name))
            })
        {
            return Err(format!(
                "a running job is already named `{name}` ({})",
                j.id
            ));
        }
        let dir = m.root.join("jobs").join(m.session.to_string());
        let log_path = if m.cell.is_some() {
            dir.join(format!("{id}{}", jobs::CELL_LOG))
        } else {
            dir.join(format!("{id}.log"))
        };
        // 51 §7.4: the retained-terminal budget admits before anything spawns;
        // exited terminals go first, oldest across every session.
        self.make_room(RESERVE);
        if self.pty_bytes + RESERVE > jobs::PTY_BUDGET {
            return Err(format!(
                "terminal capacity exhausted ({} MiB retained-terminal budget)",
                jobs::PTY_BUDGET >> 20
            ));
        }
        let cell = m.cell.is_some();
        let s = spawn(
            &dir,
            &log_path,
            &m.args,
            &m.cwd,
            m.pty_shell.as_deref(),
            m.program.as_ref(),
            cell,
        )
        .map_err(|e| format!("{e:#}"))?;
        self.pty_bytes += RESERVE;
        let pty = Arc::new(PtyHandle {
            writer: Mutex::new(Some(s.writer)),
            parser: Mutex::new(vt100::Parser::new(nz(ROWS), nz(COLS), HISTORY)),
            revision: AtomicU64::new(0),
            output: Notify::new(),
            stream: broadcast::channel(STREAM_CHUNKS).0,
            master: Mutex::new(Some(s.master)),
            pid: s.pid,
            tree: s.tree,
            exit: Mutex::new(None),
            user_input: Mutex::new(None),
            touched: Mutex::new(Instant::now()),
            idle_wake: Notify::new(),
            waiting: AtomicUsize::new(0),
            interacted: AtomicBool::new(false),
            cursor: AtomicU64::new(0),
            snapshotted: AtomicU64::new(u64::MAX),
            sent_at: Mutex::new(None),
        });
        let mut log = Log::new(Some(s.file));
        if let Some(tx) = m.cell {
            log.scan_marks_to(tx);
        }
        let log = Arc::new(Mutex::new(log));
        let cancel = Arc::new(Cancel::default());
        let (done_tx, done) = oneshot::channel();
        // Same claim protocol as the pipes path (14): first swap owns the result.
        let claimable = Arc::new(AtomicBool::new(true));
        let started = Instant::now();
        let started_ms = chrono::Utc::now().timestamp_millis();
        // 37 §3.4: `spawn` above ran the real command; every stored/published
        // copy is scrubbed.
        let shown = crate::secrets::redact(&m.args.command).into_owned();
        let (chunk_tx, rx) = mpsc::channel(64);
        std::thread::spawn(move || read_loop(s.reader, &chunk_tx));
        self.track(&activity);
        let op = activity.id();
        let call_id = m.call_id;
        let task = tokio::spawn(run(Run {
            rt: self.rt.clone(),
            me: ctx.actor_ref().clone(),
            session: m.session,
            id: id.clone(),
            command: shown.clone(),
            name: m.args.name.clone(),
            call_id: call_id.clone(),
            log_path: log_path.clone(),
            log: log.clone(),
            pty: Arc::clone(&pty),
            child: Some(s.child),
            rx,
            cancel: cancel.clone(),
            claimable: claimable.clone(),
            started,
            started_ms,
            // 171 §12.1 inv. 3: never `<job_idle>` for a kernel.
            idle_after: m.idle_after.filter(|_| !cell),
            cell,
            done: done_tx,
            activity,
            journal: Arc::clone(&self.journal),
            snapshot_every: self.pty_snapshot,
            thresholds: self.pty_thresholds,
        }))
        .abort_handle();
        self.push(
            m.session,
            Job {
                id,
                op,
                command: shown,
                call_id,
                started,
                started_ms,
                exit: None,
                secs: 0,
                log_path: log_path.clone(),
                log,
                pgids: Arc::new(Mutex::new(vec![pty.tree.clone()])),
                cancel,
                task,
                name: m.args.name,
                on_pty: true,
                pty: Some(Arc::clone(&pty)),
                lost: None,
                kind: JobKind::Pty,
            },
        );
        Ok(Running {
            done,
            claimable,
            pty,
            log_path,
        })
    }
}

impl Message<SendInput> for Jobs {
    type Reply = DelegatedReply<Result<ToolOutput, String>>;

    async fn handle(&mut self, m: SendInput, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        // 171 §9.7: the model never types into a kernel (nor reads a lost
        // one's last screen).
        if self.kernel(m.session, &m.args.id) {
            tx.send(Err(jobs::CELL_REFUSED.to_owned()));
            return reply;
        }
        // 109 §5.3: typing into a terminal a previous engine lost.
        if let Some(out) = self.lost(m.session, &m.args.id).and_then(|j| {
            let lost = j.lost.as_ref()?;
            let at = hhmmss(&lost.restarted_at);
            let why = match &lost.reason {
                crate::jobs::LostReason::Restart => format!("engine restarted at {at}"),
                crate::jobs::LostReason::OwnerChanged { engine } => {
                    format!("session moved from {engine} at {at}")
                }
            };
            self.lost_page(
                m.session,
                &m.args.id,
                Some(&format!(
                    "the terminal is gone ({why}); last screen follows"
                )),
            )
        }) {
            tx.send(Ok(out));
            return reply;
        }
        match self.pty(m.session, &m.args.id) {
            Ok(live) => {
                // Under the caller's guard: the settle wait is its work until
                // it ends, whether or not the asker is still there (47 §3.2).
                let tracked = Tracked {
                    message: m.args,
                    activity: m.activity,
                };
                tokio::spawn(tracked.scope(|args| async move {
                    let out = send(&live, args).await;
                    live.snapshot().await;
                    tx.send(out);
                }));
            }
            Err(e) => tx.send(Err(e)),
        }
        reply
    }
}

impl Message<Screen> for Jobs {
    type Reply = Result<ToolOutput, String>;

    async fn handle(&mut self, m: Screen, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if self.kernel(m.session, &m.id) {
            return Err(jobs::CELL_REFUSED.to_owned());
        }
        if let Some(out) = self.lost_page(m.session, &m.id, None) {
            return Ok(out);
        }
        let live = self.pty(m.session, &m.id)?;
        let out = reply(&live, None, None);
        // Off the mailbox: the row is a store write.
        tokio::spawn(async move { live.snapshot().await });
        Ok(out)
    }
}

impl Message<Wait> for Jobs {
    type Reply = DelegatedReply<Result<ToolOutput, String>>;

    async fn handle(&mut self, m: Wait, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        if self.kernel(m.session, &m.args.id) {
            tx.send(Err(jobs::CELL_REFUSED.to_owned()));
            return reply;
        }
        match self.pty(m.session, &m.args.id) {
            Ok(live) => {
                let tracked = Tracked {
                    message: m.args,
                    activity: m.activity,
                };
                let call_id = m.call_id;
                tokio::spawn(tracked.scope(|args| async move {
                    // 109 §4.4: a `pty_wait` row while the call blocks.
                    let waits = !call_id.is_empty();
                    if waits {
                        let secs = args.timeout.unwrap_or(WAIT_DEFAULT).min(WAIT_MAX);
                        live.journal
                            .wait(
                                Some(WaitRow {
                                    session: live.session,
                                    tool_call_id: call_id.clone(),
                                    kind: WaitKind::PtyWait,
                                    target: live.id.0.clone(),
                                    deadline: Some(chrono::Utc::now() + Duration::from_secs(secs)),
                                    payload: serde_json::json!({
                                        "pattern": args.pattern,
                                        "cursor": live.pty.cursor.load(SeqCst),
                                    }),
                                }),
                                live.session,
                                &call_id,
                            )
                            .await;
                    }
                    let out = wait(&live, args).await;
                    if waits {
                        live.journal.wait(None, live.session, &call_id).await;
                    }
                    live.snapshot().await;
                    tx.send(out);
                }));
            }
            Err(e) => tx.send(Err(e)),
        }
        reply
    }
}

impl Message<ListTerminals> for Jobs {
    type Reply = Vec<TerminalSummary>;

    async fn handle(
        &mut self,
        m: ListTerminals,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        // 171: kernels are not inline terminals (a viewer must not resize one).
        m.sessions
            .iter()
            .filter_map(|s| self.sessions.get(s).map(|sj| (*s, sj)))
            .flat_map(|(session, sj)| {
                sj.jobs.iter().filter_map(move |j| {
                    if jobs::is_cell(j.name.as_deref()) {
                        return None;
                    }
                    j.pty.as_ref().map(|p| {
                        let (rows, cols) = p.size();
                        TerminalSummary {
                            session,
                            id: j.id.clone(),
                            name: j.name.clone(),
                            command: j.command.clone(),
                            state: j.state(),
                            rows,
                            cols,
                            revision: p.revision(),
                            epoch: None,
                        }
                    })
                })
            })
            .collect()
    }
}

impl Message<PtyScreen> for Jobs {
    type Reply = DelegatedReply<Result<PtyScreenReply, String>>;

    async fn handle(&mut self, m: PtyScreen, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let job = self
            .sessions
            .get(&m.session)
            .and_then(|s| s.jobs.iter().find(|j| j.id == m.id));
        // 109 §5.3 / §8.4: a job a previous engine lost serves its stored
        // page (`exit = lost`, `epoch: None`); the viewer renders it under
        // the banner.
        if let Some(Job {
            pty: None,
            lost:
                Some(Lost {
                    screen: Some(screen),
                    ..
                }),
            ..
        }) = job
        {
            tx.send(Ok(if m.if_revision == Some(screen.revision) {
                PtyScreenReply::Unchanged {
                    viewport: m.viewport,
                    revision: screen.revision,
                }
            } else {
                PtyScreenReply::Updated(stored_page(screen, m.viewport))
            }));
            return reply;
        }
        let Some(pty) = job.and_then(|j| j.pty.as_ref()) else {
            tx.send(Ok(PtyScreenReply::Unavailable));
            return reply;
        };
        let revision = pty.revision();
        if m.if_revision == Some(revision) {
            tx.send(Ok(PtyScreenReply::Unchanged {
                viewport: m.viewport,
                revision,
            }));
            return reply;
        }
        // Admission (51 §7.4): the clone is the mailbox's only work; the
        // render and its serialization run off it under one of the permits.
        let Ok(permit) = Arc::clone(&self.snapshots).try_acquire_owned() else {
            tx.send(Err("terminal snapshot busy; retry".into()));
            return reply;
        };
        let cap = pty.capture();
        tokio::spawn(async move {
            let _permit = permit;
            let snap = render(
                cap,
                m.viewport,
                &crate::secrets::redact_spans,
                crate::secrets::max_literal_len(),
            );
            tx.send(Ok(PtyScreenReply::Updated(snap.into_page(m.session, m.id))));
        });
        reply
    }
}

impl Jobs {
    /// The handle of a PTY job by exact id; `running` also refuses a finished one.
    fn pty_handle(
        &self,
        session: SessionId,
        id: &JobId,
        running: bool,
    ) -> Result<Arc<PtyHandle>, String> {
        let job = self
            .sessions
            .get(&session)
            .and_then(|s| s.jobs.iter().find(|j| j.id == *id));
        match job {
            None => Err("no such job".to_owned()),
            Some(Job { exit: Some(e), .. }) if running => Err(format!("{id} {e}")),
            Some(Job { pty: None, .. }) => Err(format!("{id} is not a PTY job")),
            Some(Job { pty: Some(p), .. }) => Ok(Arc::clone(p)),
        }
    }
}

impl Message<PtyInput> for Jobs {
    type Reply = DelegatedReply<Result<(), String>>;

    async fn handle(&mut self, m: PtyInput, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        match self.pty_handle(m.session, &m.id, true) {
            Err(e) => tx.send(Err(e)),
            Ok(pty) => {
                tokio::spawn(async move {
                    tx.send(write(&pty, m.text.as_deref(), &m.keys, true).await)
                });
            }
        }
        reply
    }
}

impl Message<PtyAttach> for Jobs {
    type Reply = DelegatedReply<Result<PtyAttached, String>>;

    async fn handle(&mut self, m: PtyAttach, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let pty = match self.pty_handle(m.session, &m.id, false) {
            Err(e) => {
                tx.send(Err(e));
                return reply;
            }
            Ok(p) => p,
        };
        // A finished job keeps its last size; the viewer adopts it.
        if let Some((rows, cols)) = m.size
            && pty.exit().is_none()
            && let Err(e) = pty.resize(rows, cols)
        {
            tx.send(Err(e));
            return reply;
        }
        // The replay of up to 552 rows is formatted off the mailbox.
        tokio::task::spawn_blocking(move || {
            let (replay, (rows, cols), rx, exit) = pty.attach();
            tx.send(Ok(PtyAttached {
                replay,
                rows,
                cols,
                rx,
                exit,
            }));
        });
        reply
    }
}

impl Message<PtyResize> for Jobs {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: PtyResize, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.pty_handle(m.session, &m.id, true)?
            .resize(m.rows, m.cols)
    }
}

/// Encode and write to the master off the runtime (a full kernel buffer
/// blocks). `human` stamps [`PtyHandle::user_input`] and restarts the idle
/// timer without counting as the model's interaction (61 §1.3).
async fn write(
    pty: &Arc<PtyHandle>,
    text: Option<&str>,
    keys: &[Key],
    human: bool,
) -> Result<(), String> {
    let data = encode(pty, text, keys)?;
    if data.is_empty() {
        return Ok(());
    }
    if human {
        *lock(&pty.user_input) = Some(Instant::now());
        pty.touch(false);
    }
    let pty = Arc::clone(pty);
    tokio::task::spawn_blocking(move || {
        let mut w = lock(&pty.writer);
        let Some(w) = w.as_mut() else {
            return Err(std::io::Error::other("job finished"));
        };
        w.write_all(&data).and_then(|()| w.flush())
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| format!("write to pty: {e}"))
}

/// What the spawn hands the job task and the handle.
struct Spawned {
    file: std::fs::File,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    reader: Box<dyn Read + Send>,
    writer: Box<dyn Write + Send>,
    pid: u32,
    tree: ProcessTree,
}

/// 22 §4.1: [`platform::pty_command`] (`$SHELL -lc <command>` on Unix) — or
/// `program` verbatim (171 §5.2) — on a fresh 120×40 pty (`setsid` +
/// `TIOCSCTTY` in the child; ConPTY on Windows), log file created, the
/// child's tree adopted. A `cell` job inherits nothing: `args.env` is the
/// whole kernel environment (171 §5.4).
fn spawn(
    dir: &Path,
    log_path: &Path,
    args: &StartArgs,
    cwd: &Path,
    pty_shell: Option<&Path>,
    program: Option<&(PathBuf, Vec<String>)>,
    cell: bool,
) -> anyhow::Result<Spawned> {
    std::fs::create_dir_all(dir)?;
    // 171F3 R2: never truncate an existing log (the id skips them).
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(log_path)?;
    let pair = native_pty_system().openpty(PtySize {
        rows: ROWS,
        cols: COLS,
        pixel_width: 0,
        pixel_height: 0,
    })?;
    let mut cmd = match program {
        Some((path, argv)) => {
            let mut cmd = CommandBuilder::new(path);
            cmd.args(argv);
            cmd
        }
        None => {
            let (program, argv) = platform::pty_command(&args.command, pty_shell);
            let mut cmd = CommandBuilder::new(program);
            cmd.args(argv);
            cmd
        }
    };
    cmd.cwd(cwd);
    if cell {
        cmd.env_clear();
    }
    cmd.env("TERM", "xterm-256color");
    for (k, v) in args.env.iter().flatten() {
        cmd.env(k, v);
    }
    let child = pair.slave.spawn_command(cmd)?;
    drop(pair.slave);
    let pid = child
        .process_id()
        .ok_or_else(|| anyhow::anyhow!("pty child has no pid"))?;
    let tree = ProcessTree::adopt(pid)?;
    let reader = pair.master.try_clone_reader()?;
    let writer = pair.master.take_writer()?;
    Ok(Spawned {
        file,
        master: pair.master,
        child,
        reader,
        writer,
        pid,
        tree,
    })
}

/// Reader thread (22 §4.2): master → bounded channel; back-pressure lands in
/// the kernel's pty buffer. Ends on EOF/EIO (every slave closed) or when the
/// job task is gone.
// ponytail: the dup'd master fd here keeps the pty alive until every slave
// holder exits, so a `setsid` escapee delays the drop-master SIGHUP (22 §2.4)
// and parks this thread; poll(2) with a stop flag if that ever matters.
fn read_loop(mut r: Box<dyn Read + Send>, tx: &mpsc::Sender<Vec<u8>>) {
    let mut buf = vec![0u8; READ_BUF];
    loop {
        match r.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if tx.blocking_send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
}

/// Everything one PTY job task owns.
struct Run {
    rt: Runtime,
    me: ActorRef<Jobs>,
    session: SessionId,
    id: JobId,
    command: String,
    name: Option<String>,
    call_id: String,
    log_path: PathBuf,
    log: Arc<Mutex<Log>>,
    pty: Arc<PtyHandle>,
    /// The leader; the reaper (`run`) takes it at start.
    child: Option<Box<dyn Child + Send + Sync>>,
    rx: mpsc::Receiver<Vec<u8>>,
    cancel: Arc<Cancel>,
    claimable: Arc<AtomicBool>,
    started: Instant,
    /// Spawn time, unix ms (the row's `started_ms`).
    started_ms: i64,
    /// Screen-quiet interval before `<job_idle>` (61 §3.4); `None` = off.
    idle_after: Option<Duration>,
    /// 171 §12.1 inv. 3: a user-cell kernel; its `Finished` is never a
    /// `<job_result>`.
    cell: bool,
    done: oneshot::Sender<ToolOutput>,
    /// The job's admitted lifetime (`Kind::Pty`); the actor holds the other
    /// clone in [`Jobs::live`]. Dropped when this task returns.
    activity: Activity,
    /// 109 §5.1 / §5.3: the `jobs` row and the screen snapshots.
    journal: Arc<dyn Journal>,
    /// `durability.pty_snapshot_secs`.
    snapshot_every: Duration,
    /// 155 §6.6: `tools.pty_reaction_secs` / `pty_tree_cpu_pct`.
    thresholds: PtyThresholds,
}

/// The reader still draining a lingering slave, and the log its bytes reach.
type Drain = (mpsc::Receiver<Vec<u8>>, Arc<Mutex<Log>>);

/// Something outlived the exit: a `&` child that ignored SIGHUP stays in
/// the leader's process group (job control is off under `$SHELL -c`) and,
/// by platform, keeps the slave open or loses it when the tty is revoked
/// with its leader; a `setsid` escapee holding the slave sits outside the
/// group. Either is this session's work until it actually ends (47 §4.2
/// rule 4): a child operation of the job, settled by the group going empty
/// and the reader hitting EOF, never inferred from the screen or the
/// command line. Its bytes still reach the log. A killed group that
/// survived `KILL_WAIT` is a doubt, reported on the child alone so the
/// job's own result is not overridden.
async fn linger(
    job: &Activity,
    id: &JobId,
    group: Option<ProcessTree>,
    rx: Option<Drain>,
    doubt: Option<String>,
) {
    match job.admit(Kind::Command("pty background process")).await {
        Ok(a) => {
            let id = id.clone();
            let guard = a.clone();
            tokio::spawn(a.scope(async move {
                if let Some(why) = doubt {
                    guard.unknown(format!("{id}: {why}"));
                }
                if let Some((mut rx, log)) = rx {
                    while let Some(c) = rx.recv().await {
                        lock(&log).push(&c);
                    }
                    lock(&log).close();
                }
                if let Some(tree) = group {
                    jobs::group_gone(&tree, None).await;
                }
            }));
        }
        // The job itself completed and its result is owed regardless; the
        // supervisor that refused (or is gone) certifies nothing meanwhile.
        // The reader goes with `rx`; nothing feeds the log after this.
        Err(e) => {
            if let Some((_, log)) = rx {
                lock(&log).close();
            }
            tracing::warn!("{id}: pty background work left untracked: {e}");
        }
    }
}

fn exit_of(
    r: Result<std::io::Result<portable_pty::ExitStatus>, tokio::task::JoinError>,
) -> JobExit {
    match r {
        Ok(Ok(st)) => JobExit::Exited(u8::try_from(st.exit_code()).unwrap_or(1)),
        _ => JobExit::Exited(1),
    }
}

/// The idle detector (61 §3.4): a timer restarted by any screen change or
/// touch, disarmed by firing, whose interval doubles while nobody acts.
struct Idle {
    base: Duration,
    last_change: Instant,
    last_rows: Vec<String>,
    last_fire: Option<Instant>,
    nudges: u32,
    backoff: Duration,
    /// 155 §6.3: the last busy-tree deadline; the next one counts from here.
    deferred: Option<Instant>,
}

impl Idle {
    fn new(base: Duration, started: Instant) -> Self {
        Self {
            base,
            last_change: started,
            last_rows: Vec::new(),
            last_fire: None,
            nudges: 0,
            backoff: base,
            deferred: None,
        }
    }

    /// A rendered screen: the timer restarts when the rows differ (61 §1.2;
    /// the cursor is not compared).
    fn saw(&mut self, rows: &[String]) {
        if *rows != self.last_rows {
            self.last_rows = rows.to_vec();
            self.last_change = Instant::now();
        }
    }

    /// When the next nudge is due; `None` while disarmed (fired, and neither
    /// the screen nor anyone has touched the job since, 61 §1.3).
    fn deadline(&self, pty: &PtyHandle) -> Option<tokio::time::Instant> {
        let touched = *lock(&pty.touched);
        let armed = self
            .last_fire
            .is_none_or(|f| self.last_change > f || touched > f);
        armed.then(|| {
            let interval = if pty.interacted.load(SeqCst) {
                self.base
            } else {
                self.backoff
            };
            let from = self
                .last_change
                .max(touched)
                .max(self.deferred.unwrap_or(touched));
            tokio::time::Instant::from_std(from + interval)
        })
    }

    /// The deadline passed: `(nudge, quiet_secs)` to send, or `None` while a
    /// `wait` is watching (61 §1.5; its return re-arms). Either way the
    /// detector is disarmed until the next change or touch.
    fn fire(&mut self, pty: &PtyHandle, now: Instant) -> Option<(u32, u64)> {
        self.last_fire = Some(now);
        if pty.waiting.load(SeqCst) > 0 {
            return None;
        }
        // 61 §1.4: a run of unanswered nudges doubles; any model touch resets.
        if pty.interacted.swap(false, SeqCst) {
            self.nudges = 0;
            self.backoff = self.base;
        }
        self.nudges += 1;
        self.backoff = (self.backoff * 2).min(IDLE_BACKOFF_MAX);
        Some((self.nudges, now.duration_since(self.last_change).as_secs()))
    }

    /// 155 §6.3: the deadline passed but the process tree was busy — push
    /// the next check out by one interval without firing, resetting `nudges`
    /// or pretending the screen changed.
    fn defer(&mut self, now: Instant) {
        self.deferred = Some(now);
    }
}

/// 155 §6.3: at the idle deadline. A busy tree defers; a quiet one fires as
/// in 61 (still `None` while a `wait` watches) with the quiet reason.
fn idle_due(
    idle: &mut Idle,
    tree: &TreeQuiet,
    pty: &PtyHandle,
    now: Instant,
) -> Option<(u32, u64, String)> {
    if !tree.quiet() {
        idle.defer(now);
        return None;
    }
    let (nudge, quiet_secs) = idle.fire(pty, now)?;
    Some((
        nudge,
        quiet_secs,
        format!("screen unchanged for {quiet_secs} s, process tree quiet"),
    ))
}

/// 155 §6.4: the reaction check. `Some((nudge, quiet_secs))` once when a
/// `send` saw neither a screen change nor tree movement for `reaction`.
/// `sent_at` clears on movement, on firing and when a `wait` suppresses the
/// fire (its timeout is the answer then), so nothing fires twice.
fn reaction_due(
    idle: &Idle,
    tree: &TreeQuiet,
    pty: &PtyHandle,
    reaction: Duration,
    now: Instant,
) -> Option<(u32, u64)> {
    let mut sent_at = lock(&pty.sent_at);
    let sent = (*sent_at)?;
    if idle.last_change > sent || tree.moved_since(sent) {
        *sent_at = None;
        return None;
    }
    if reaction.is_zero() || now.duration_since(sent) < reaction {
        return None;
    }
    *sent_at = None;
    if pty.waiting.load(SeqCst) > 0 {
        return None;
    }
    Some((
        idle.nudges + 1,
        now.duration_since(idle.last_change).as_secs(),
    ))
}

/// The pty's foreground process group; `None` when it cannot be read (no
/// master, no fd, `tcgetpgrp` failed).
#[cfg(unix)]
fn foreground_pgrp(pty: &PtyHandle) -> Option<libc::pid_t> {
    let fd = lock(&pty.master).as_ref().and_then(|m| m.as_raw_fd())?;
    // SAFETY: `tcgetpgrp` reads one int from the kernel for a live fd.
    let fg = unsafe { libc::tcgetpgrp(fd) };
    (fg > 0).then_some(fg)
}

/// 155 §6.2: is the program theuth started still the terminal's foreground
/// process group? `true` when that cannot be read (no master, no fd, or an
/// OS without the notion), so the 61 behaviour stands.
fn foreground_is_leader(pty: &PtyHandle) -> bool {
    #[cfg(unix)]
    {
        foreground_pgrp(pty).is_none_or(|fg| fg as u32 == pty.pid)
    }
    #[cfg(not(unix))]
    {
        let _ = pty;
        true
    }
}

impl Jobs {
    /// The running kernel job `id` (171): Err `no such job` | `not a cell
    /// job` | `j3 already finished`.
    fn cell_job(&self, session: SessionId, id: &JobId) -> Result<&Job, String> {
        let job = self
            .sessions
            .get(&session)
            .and_then(|s| s.jobs.iter().find(|j| j.id == *id))
            .ok_or_else(|| "no such job".to_owned())?;
        if !jobs::is_cell(job.name.as_deref()) {
            return Err("not a cell job".into());
        }
        if job.exit.is_some() {
            return Err(format!("{id} already finished"));
        }
        Ok(job)
    }
}

impl Message<CellKill> for Jobs {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: CellKill, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let job = self.cell_job(m.session, &m.id)?;
        if m.close {
            // As `CloseSessions`: the task kills the tree and settles `Killed`.
            job.cancel.fire(CancelOrigin::Shutdown);
            return Ok(());
        }
        let Some(pty) = &job.pty else {
            return Err(format!("{} has no terminal", m.id));
        };
        #[cfg(unix)]
        {
            // 171 §7.3: Kill reaches what the kernel is running, never the
            // kernel shell itself (its own group is the prompt / a builtin).
            if let Some(fg) = foreground_pgrp(pty)
                && fg as u32 != pty.pid
            {
                // SAFETY: `killpg` on a positive group id read from our own
                // pty; a stale id at worst fails with ESRCH.
                unsafe { libc::killpg(fg, libc::SIGKILL) };
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            // No foreground group to read: the whole kernel goes.
            let _ = pty;
            job.cancel.fire(CancelOrigin::Shutdown);
            Ok(())
        }
    }
}

impl Message<CellForeground> for Jobs {
    type Reply = Result<bool, String>;

    async fn handle(
        &mut self,
        m: CellForeground,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let job = self.cell_job(m.session, &m.id)?;
        let Some(pty) = &job.pty else {
            return Err(format!("{} has no terminal", m.id));
        };
        #[cfg(unix)]
        {
            Ok(foreground_pgrp(pty).is_some_and(|fg| fg as u32 == pty.pid))
        }
        #[cfg(not(unix))]
        {
            let _ = pty;
            Ok(true)
        }
    }
}

/// One `<job_idle>` (61 §1.8 / 155 §6.5): the wake rides the guard, the bus
/// copy is for UIs.
async fn nudge_model(j: &Run, nudge: u32, quiet_secs: u64, why: String) {
    let (rows, cursor) = j.pty.screen();
    let event = JobEvent::Idle {
        session: j.session,
        id: j.id.clone(),
        command: j.command.clone(),
        name: j.name.clone(),
        quiet_secs,
        secs: j.started.elapsed().as_secs(),
        nudge,
        user_typed_secs: attached_for(*lock(&j.pty.user_input)).map(|d| d.as_secs()),
        output: screen_body(&rows, cursor),
        why,
    };
    j.activity.notify(event.clone());
    j.rt.publish(event).await;
}

/// The job task (22 §4.2): ingest chunks, coalesce `Screen` events, wake the
/// model when the screen goes quiet (61), detect exit (reaper or EOF,
/// whichever first), then the 14 finish path with the final screen as `output`.
async fn run(mut j: Run) {
    // 109 §5.1: the row before anything can refer to the job; the screen
    // rows (§5.3) key on it.
    let mut row = JobRow {
        session: j.session,
        id: j.id.clone(),
        kind: JobKind::Pty,
        call_id: j.call_id.clone(),
        command: j.command.clone(),
        name: j.name.clone(),
        pgid: Some(i64::from(j.pty.tree.id())),
        log: Some(j.log_path.clone()),
        started_ms: j.started_ms,
        state: JobState::Running,
        exit: None,
        settled_ms: None,
        engine: String::new(),
    };
    j.journal.job(row.clone()).await;
    j.rt.publish(JobEvent::Started {
        session: j.session,
        id: j.id.clone(),
        command: j.command.clone(),
        call_id: j.call_id.clone(),
        log: j.log_path.clone(),
        name: j.name.clone(),
        pty: true,
        kind: JobKind::Pty,
    })
    .await;

    let mut child = j.child.take().expect("the reaper takes the child once");
    let mut waited = tokio::task::spawn_blocking(move || child.wait());
    let tree = j.pty.tree.clone();
    let mut carry = Vec::new();
    let mut dirty = false;
    let mut tick = tokio::time::interval(jobs::OUTPUT_EVERY);
    let mut snap = tokio::time::interval(j.snapshot_every);
    snap.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut idle = j.idle_after.map(|base| Idle::new(base, j.started));
    // 155 §6.2: the process tree, sampled once a second while the screen is
    // unchanged; ticks missed while output flows are dropped, not replayed.
    let mut quiet = TreeQuiet::new(j.thresholds);
    let mut tree_tick = tokio::time::interval(Duration::from_secs(1));
    tree_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let (exit, killed, origin) = loop {
        // 155 §6.4: every wake (ticks, samples, chunks) re-checks the reaction
        // clock; the tree tick bounds the lateness to a second.
        if let Some(i) = &idle
            && let Some((nudge, quiet_secs)) =
                reaction_due(i, &quiet, &j.pty, j.thresholds.reaction, Instant::now())
        {
            nudge_model(&j, nudge, quiet_secs, "no reaction to input".to_owned()).await;
        }
        let due = idle.as_ref().and_then(|i| i.deadline(&j.pty));
        tokio::select! {
            biased;
            origin = j.cancel.notified() => {
                tree.kill();
                // Cancellation is not cessation: the exit is the reaper's.
                let _ = (&mut waited).await;
                break (JobExit::Killed, true, Some(origin));
            }
            r = &mut waited => break (exit_of(r), false, None),
            chunk = j.rx.recv() => match chunk {
                Some(c) => {
                    ingest(&j.pty, &j.log, &mut carry, &c);
                    dirty = true;
                }
                // EOF: every slave closed (or the tty revoked, on platforms
                // that do so once its leader is gone). A leader that closed
                // its tty and lingers gets a second, then SIGKILL.
                None => break match tokio::time::timeout(Duration::from_secs(1), &mut waited).await {
                    Ok(r) => (exit_of(r), false, None),
                    Err(_) => {
                        tree.kill();
                        (exit_of(waited.await), true, None)
                    }
                },
            },
            // A touch moved the idle deadline: loop to recompute `due`.
            () = j.pty.idle_wake.notified(), if idle.is_some() => {}
            _ = tick.tick(), if dirty => {
                let (rows, cursor) = j.pty.screen();
                if let Some(i) = &mut idle {
                    i.saw(&rows);
                }
                j.rt.publish(JobEvent::Screen { session: j.session, id: j.id.clone(), rows, cursor }).await;
                dirty = false;
            }
            // 109 §5.3: bounds the loss for a busy screen nobody reads;
            // an unchanged revision writes nothing.
            _ = snap.tick() => j.pty.snapshot_to_store(&*j.journal, j.session, &j.id).await,
            _ = tree_tick.tick(), if idle.is_some() && !dirty => {
                quiet.saw(Instant::now(), tree.sample(), foreground_is_leader(&j.pty));
            }
            () = tokio::time::sleep_until(due.unwrap_or_else(tokio::time::Instant::now)), if due.is_some() => {
                // A fresh reading: the verdict should not be a second stale.
                let now = Instant::now();
                quiet.saw(now, tree.sample(), foreground_is_leader(&j.pty));
                if let Some(i) = &mut idle
                    && let Some((nudge, quiet_secs, why)) = idle_due(i, &quiet, &j.pty, now)
                {
                    nudge_model(&j, nudge, quiet_secs, why).await;
                }
            }
        }
    };
    // Trailing bytes still in flight from the reader. `None` is EOF/EIO;
    // idling out instead means a holder still has the slave.
    let closed = loop {
        match tokio::time::timeout(Duration::from_millis(200), j.rx.recv()).await {
            Ok(Some(c)) => ingest(&j.pty, &j.log, &mut carry, &c),
            Ok(None) => break true,
            Err(_) => break false,
        }
    };
    // Closes the slave: anything that escaped the group gets SIGHUP (once the
    // reader's dup is gone too). Taken from the handle so a late `resize` /
    // `send` sees "job finished" (D2).
    j.pty.release();
    // Drained to EOF: the log file goes now (D1); otherwise with the reader
    // in `linger`.
    if closed {
        lock(&j.log).close();
    }
    // Ownership is the process group, not the tty: the leader is reaped, but
    // a `&` child of it lives on in the same group whether or not it still
    // holds the slave. The result waits `SETTLE` (`KILL_WAIT` after a kill)
    // for the group to empty; past that the tail is its own operation.
    let bound = if killed {
        jobs::KILL_WAIT
    } else {
        jobs::SETTLE
    };
    let gone = jobs::group_gone(&tree, Some(tokio::time::Instant::now() + bound)).await;
    let rx = (!closed).then(|| (j.rx, j.log.clone()));
    match (gone, killed) {
        (true, _) if closed => {}
        (true, _) => linger(&j.activity, &j.id, None, rx, None).await,
        (false, false) => linger(&j.activity, &j.id, Some(tree), rx, None).await,
        (false, true) => {
            let doubt = format!("process group {} survived SIGKILL", tree.id());
            linger(&j.activity, &j.id, None, rx, Some(doubt)).await;
        }
    }

    let (rows, cursor) = j.pty.screen();
    let screen = rows.join("\n");
    let secs = j.started.elapsed().as_secs();
    let out = match &exit {
        JobExit::Exited(code) => ToolOutput {
            content: format!("{screen}{}exit code: {code}", sep(&screen)),
            is_error: *code != 0,
            structured: Some(serde_json::Value::String(screen.clone())),
            files: Vec::new(),
        },
        JobExit::Timeout | JobExit::Killed | JobExit::Lost => {
            ToolOutput::err(format!("{screen}{}killed", sep(&screen)))
        }
    };
    *lock(&j.pty.exit) = Some(exit.clone());
    {
        let _p = lock(&j.pty.parser);
        j.pty.revision.fetch_add(1, SeqCst);
        let _ = j.pty.stream.send(PtyOut::Exit(exit.clone()));
    }
    j.pty.output.notify_waiters();
    // 109 §5.3 / §5.1: the final screen, then the settled row.
    j.pty.snapshot_to_store(&*j.journal, j.session, &j.id).await;
    row.state = JobState::Settled;
    row.exit = Some(exit.clone());
    row.settled_ms = Some(chrono::Utc::now().timestamp_millis());
    j.journal.job(row).await;
    j.rt.publish(JobEvent::Screen {
        session: j.session,
        id: j.id.clone(),
        rows,
        cursor,
    })
    .await;
    // The model that called `cancel_job` read this screen before it killed
    // the job (61) and got the tool reply: its kill is inline, and a kill
    // from anywhere else carries the one-line notice, never the screen
    // (14 §1.2 as-built). The `Screen` event above still has the last frame.
    // A kernel's exit is the cells actor's business (171 §12.1 inv. 3).
    let inline = j.claimable.swap(false, SeqCst) || origin == Some(CancelOrigin::Tool) || j.cell;
    let event = JobEvent::Finished {
        session: j.session,
        id: j.id.clone(),
        command: j.command,
        exit: exit.clone(),
        secs,
        output: origin.map_or(screen, |o| o.notice().to_owned()),
        inline,
        pty: true,
        kind: JobKind::Pty,
    };
    // The result rides the guard unless the tool call already returned it
    // (14 / 47 §4.1); the bus event stays observation for UIs.
    let _ =
        j.me.tell(Finished {
            op: j.activity.id(),
            row: Some((j.session, j.id, exit, secs)),
            result: (!inline).then(|| event.clone()),
        })
        .await;
    j.rt.publish(event).await;
    let _ = j.done.send(out);
}

/// One chunk: log, render, answer terminal queries, wake waiters.
fn ingest(pty: &PtyHandle, log: &Mutex<Log>, carry: &mut Vec<u8>, chunk: &[u8]) {
    lock(log).push(chunk);
    let cursor = {
        let mut p = lock(&pty.parser);
        p.process(chunk);
        pty.revision.fetch_add(1, SeqCst);
        let _ = pty.stream.send(PtyOut::Data(Arc::from(chunk)));
        p.screen().cursor_position()
    };
    respond(pty, carry, chunk, cursor);
    pty.output.notify_waiters();
}

/// Fixed-reply terminal queries (22 §4.4); `ESC[6n` is answered from the cursor.
const QUERIES: &[(&[u8], &[u8])] = &[
    (b"\x1b[5n", b"\x1b[0n"),
    (b"\x1b[c", b"\x1b[?62;22c"),
    (b"\x1b[0c", b"\x1b[?62;22c"),
    (b"\x1b[>c", b"\x1b[>41;0;0c"),
    (b"\x1b[>0c", b"\x1b[>41;0;0c"),
    (b"\x1b]10;?\x07", b"\x1b]10;rgb:ffff/ffff/ffff\x07"),
    (b"\x1b]10;?\x1b\\", b"\x1b]10;rgb:ffff/ffff/ffff\x1b\\"),
    (b"\x1b]11;?\x07", b"\x1b]11;rgb:0000/0000/0000\x07"),
    (b"\x1b]11;?\x1b\\", b"\x1b]11;rgb:0000/0000/0000\x1b\\"),
];
const CPR: &[u8] = b"\x1b[6n";

enum Scan {
    /// A query of this many bytes; write the reply.
    Reply(usize, Cow<'static, [u8]>),
    /// The rest of the chunk is a strict prefix of a query: carry it.
    Partial,
    None,
}

/// Classify the bytes at an ESC.
fn query(s: &[u8], cursor: (u16, u16)) -> Scan {
    if s.starts_with(CPR) {
        let reply = format!("\x1b[{};{}R", cursor.0 + 1, cursor.1 + 1);
        return Scan::Reply(CPR.len(), Cow::Owned(reply.into_bytes()));
    }
    for (q, r) in QUERIES {
        if s.starts_with(q) {
            return Scan::Reply(q.len(), Cow::Borrowed(r));
        }
    }
    let prefix = std::iter::once(CPR)
        .chain(QUERIES.iter().map(|(q, _)| *q))
        .any(|q| q.len() > s.len() && q.starts_with(s));
    if prefix { Scan::Partial } else { Scan::None }
}

/// The query responder (22 §4.4): replies go to the master, never the log.
fn respond(pty: &PtyHandle, carry: &mut Vec<u8>, chunk: &[u8], cursor: (u16, u16)) {
    let data: Cow<[u8]> = if carry.is_empty() {
        Cow::Borrowed(chunk)
    } else {
        carry.extend_from_slice(chunk);
        Cow::Owned(std::mem::take(carry))
    };
    let mut i = 0;
    while let Some(off) = data[i..].iter().position(|&b| b == 0x1b) {
        let at = i + off;
        match query(&data[at..], cursor) {
            Scan::Reply(len, reply) => {
                if let Some(w) = lock(&pty.writer).as_mut() {
                    let _ = w.write_all(&reply).and_then(|()| w.flush());
                }
                i = at + len;
            }
            Scan::Partial => {
                if data.len() - at <= CARRY {
                    carry.extend_from_slice(&data[at..]);
                }
                break;
            }
            Scan::None => i = at + 1,
        }
    }
}

/// Drop ESC sequences (CSI, OSC, two-byte) so a pattern matches through colour
/// codes (22 §4.6); lossy UTF-8 of the rest.
fn strip(bytes: &[u8]) -> String {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b {
            i = skip_esc(bytes, i);
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Index just past the ESC sequence starting at `bytes[i]`.
fn skip_esc(bytes: &[u8], mut i: usize) -> usize {
    i += 1;
    match bytes.get(i) {
        Some(b'[') => {
            i += 1;
            while i < bytes.len() && !(0x40..=0x7e).contains(&bytes[i]) {
                i += 1;
            }
            i + 1
        }
        Some(b']') => {
            while i < bytes.len()
                && bytes[i] != 0x07
                && !(bytes[i] == 0x1b && bytes.get(i + 1) == Some(&b'\\'))
            {
                i += 1;
            }
            i + if bytes.get(i) == Some(&0x1b) { 2 } else { 1 }
        }
        // `ESC ( B` and friends: intermediates 0x20..=0x2f, then one final byte.
        Some(b) if (0x20..=0x2f).contains(b) => {
            while i < bytes.len() && (0x20..=0x2f).contains(&bytes[i]) {
                i += 1;
            }
            i + 1
        }
        Some(_) => i + 1,
        None => i,
    }
}

/// Raw index just past the first `kept` bytes [`strip`] keeps: maps a match
/// end in the stripped text back into `bytes`.
// ponytail: `kept` indexes the lossy String, which equals the kept-bytes
// index unless invalid UTF-8 precedes the match; then the cursor lands a
// few bytes off. Match on `regex::bytes` over the kept bytes if it matters.
fn raw_end(bytes: &[u8], kept: usize) -> usize {
    let (mut i, mut n) = (0, 0);
    while i < bytes.len() && n < kept {
        if bytes[i] == 0x1b {
            i = skip_esc(bytes, i);
        } else {
            i += 1;
            n += 1;
        }
    }
    i
}

fn log_len(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |m| m.len())
}

/// First `re` match in the log bytes appended since offset `from`, with the
/// log offset just past it.
fn find_since(path: &Path, from: u64, re: &Regex) -> Option<(String, u64)> {
    let mut f = std::fs::File::open(path).ok()?;
    f.seek(SeekFrom::Start(from)).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    let text = strip(&buf);
    let m = re.find(&text)?;
    Some((m.as_str().to_owned(), from + raw_end(&buf, m.end()) as u64))
}

async fn connects(host: &str, port: u16) -> bool {
    tokio::time::timeout(PORT_POLL, tokio::net::TcpStream::connect((host, port)))
        .await
        .is_ok_and(|r| r.is_ok())
}

enum Outcome {
    /// Every condition passed; the log match and the offset past it, if there
    /// was a pattern.
    Ready(Option<(String, u64)>),
    Exited,
    /// What was still pending: `log \`re\``, `port N`.
    TimedOut(Vec<String>),
}

/// The shared loop of `wait` (22 §4.6) and `ready` (22 §4.7): wake on output
/// (or every [`PORT_POLL`] for a port), re-check, until every condition
/// passes, the job exits, or `deadline`. No pattern and no port → exit only.
async fn await_output(
    pty: &PtyHandle,
    log: &Path,
    from: u64,
    re: Option<&Regex>,
    port: Option<(&str, u16)>,
    deadline: tokio::time::Instant,
) -> Outcome {
    let exit_only = re.is_none() && port.is_none();
    let mut need_log = re.is_some();
    let mut need_port = port.is_some();
    let mut matched = None;
    loop {
        // Created before the checks so a wake between check and await counts.
        let notified = pty.output.notified();
        if pty.exit().is_some() {
            return Outcome::Exited;
        }
        if need_log && let Some(m) = re.and_then(|re| find_since(log, from, re)) {
            matched = Some(m);
            need_log = false;
        }
        if need_port && let Some((h, p)) = port {
            need_port = !connects(h, p).await;
        }
        if !exit_only && !need_log && !need_port {
            return Outcome::Ready(matched);
        }
        tokio::select! {
            () = notified, if need_log || exit_only => {}
            () = tokio::time::sleep(PORT_POLL), if need_port => {}
            () = tokio::time::sleep_until(deadline) => {
                let mut pending = Vec::new();
                if let (true, Some(re)) = (need_log, re) {
                    pending.push(format!("log `{}`", re.as_str()));
                }
                if let (true, Some((_, p))) = (need_port, port) {
                    pending.push(format!("port {p}"));
                }
                return Outcome::TimedOut(pending);
            }
        }
    }
}

/// 22 §4.5: write, signal, settle, screen. The write moves `wait`'s read
/// cursor to the end of the log first (55 §12): what follows the keystroke
/// is what the model waits for next.
async fn send(live: &Live, args: SendArgs) -> Result<ToolOutput, String> {
    live.pty.cursor.fetch_max(log_len(&live.log_path), SeqCst);
    let mut text = args.text.clone().unwrap_or_default();
    if args.text.is_some() && args.enter.unwrap_or(true) {
        text.push('\r');
    }
    write(
        &live.pty,
        Some(&text),
        args.keys.as_deref().unwrap_or_default(),
        false,
    )
    .await?;
    // 155 §6.4: the reaction clock starts at the write (`reaction_due` in `run`).
    *lock(&live.pty.sent_at) = Some(Instant::now());
    if let Some(sig) = args.signal {
        live.pty
            .tree
            .signal(sig.into())
            .map_err(|e| format!("signal: {e}"))?;
    }
    settle(&live.pty).await;
    Ok(reply(live, None, None))
}

/// Quiet for [`SETTLE_QUIET`], or [`SETTLE_MAX`] total, or exit.
async fn settle(pty: &PtyHandle) {
    let end = tokio::time::Instant::now() + SETTLE_MAX;
    loop {
        let notified = pty.output.notified();
        if pty.exit().is_some() {
            return;
        }
        tokio::select! {
            () = notified => {}
            () = tokio::time::sleep(SETTLE_QUIET) => return,
            () = tokio::time::sleep_until(end) => return,
        }
    }
}

/// 22 §4.6; the scan starts at the job's read cursor (55 §12), so output
/// already on screen since the model's last `send` matches at once.
async fn wait(live: &Live, args: WaitArgs) -> Result<ToolOutput, String> {
    let re = args
        .pattern
        .as_deref()
        .map(Regex::new)
        .transpose()
        .map_err(|e| format!("pattern: {e}"))?;
    let secs = args.timeout.unwrap_or(WAIT_DEFAULT).min(WAIT_MAX);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    let from = live.pty.cursor.load(SeqCst);
    let on_screen = re
        .as_ref()
        .and_then(|re| find_since(&live.log_path, from, re));
    let already = on_screen.is_some();
    let outcome = match on_screen {
        Some(m) => Outcome::Ready(Some(m)),
        None => {
            // 61 §1.5: the model is watching; the reply, not a nudge, is the wake.
            live.pty.waiting.fetch_add(1, SeqCst);
            let o =
                await_output(&live.pty, &live.log_path, from, re.as_ref(), None, deadline).await;
            live.pty.waiting.fetch_sub(1, SeqCst);
            o
        }
    };
    let (note, matched) = match outcome {
        Outcome::Ready(Some((m, end))) => {
            live.pty.cursor.fetch_max(end, SeqCst);
            let how = if already { " (already on screen)" } else { "" };
            (Some(format!("matched{how}: {m:?}")), Some(m))
        }
        Outcome::TimedOut(_) => (Some(format!("timed out after {secs}s")), None),
        Outcome::Ready(None) | Outcome::Exited => (None, None),
    };
    Ok(reply(live, note.as_deref(), matched.as_deref()))
}

fn label(id: &JobId, name: Option<&str>) -> String {
    match name {
        Some(n) => format!("{id} ({n})"),
        None => id.0.clone(),
    }
}

/// Local `HH:MM:SS`, as the 109 §5.3 banner reads.
fn hhmmss(t: &chrono::DateTime<chrono::Utc>) -> String {
    t.with_timezone(&chrono::Local)
        .format("%H:%M:%S")
        .to_string()
}

/// 109 §5.3: a [`PtyPage`] from a stored `pty_screens` row — plain cells,
/// no scrollback, `exit = lost`. The `Live` viewport only; a history page
/// of a terminal that no longer exists is the same rows.
fn stored_page(s: &PtyScreenRow, viewport: PtyViewport) -> PtyPage {
    let lines: Vec<Vec<StyledRun>> = s
        .text
        .split('\n')
        .map(|row| {
            vec![StyledRun {
                style: Style::default(),
                cells: row
                    .chars()
                    .map(|c| Cell {
                        text: c.to_string(),
                        wide: false,
                        continuation: false,
                    })
                    .collect(),
            }]
        })
        .collect();
    PtyPage {
        session: s.session,
        id: s.job_id.clone(),
        revision: s.revision,
        epoch: None,
        viewport,
        origin: 0,
        retained: 0,
        rows: s.rows,
        cols: s.cols,
        alternate: false,
        cursor: Some((s.cursor_row, s.cursor_col)),
        wrapped: vec![false; lines.len()],
        lines,
        truncated: false,
        redaction_complete: true,
        exit: Some(JobExit::Lost),
    }
}

/// Rows then the `cursor:` line (22 §2.2): the `<job_idle>` body (61 §2.2).
fn screen_body(rows: &[String], cursor: (u16, u16)) -> String {
    let mut s = rows.join("\n");
    if !rows.is_empty() {
        s.push('\n');
    }
    let _ = write!(s, "cursor: row {} col {}", cursor.0 + 1, cursor.1 + 1);
    s
}

fn screen_block(rows: &[String], cursor: (u16, u16)) -> String {
    format!("screen:\n{}", screen_body(rows, cursor))
}

/// The `send` / `screen` / `wait` reply (22 §2.2): state line, optional note,
/// screen; `structured` mirrors it. The reply is the model's touch (61 §1.3).
fn reply(live: &Live, note: Option<&str>, matched: Option<&str>) -> ToolOutput {
    live.pty.touch(true);
    let (rows, cursor) = live.pty.screen();
    let state = live
        .pty
        .exit()
        .map_or_else(|| "running".to_owned(), |e| e.to_string());
    let mut content = format!(
        "job {} · {state} · {}s\n",
        label(&live.id, live.name.as_deref()),
        live.started.elapsed().as_secs()
    );
    if let Some(n) = note {
        content.push_str(n);
        content.push('\n');
    }
    content.push_str(&screen_block(&rows, cursor));
    // 55 §2.3: a hand is on the keyboard — read before typing more.
    let user_attached = attached_for(*lock(&live.pty.user_input));
    if let Some(ago) = user_attached {
        let _ = write!(content, "\nuser attached · typed {}s ago", ago.as_secs());
    }
    ToolOutput {
        content,
        is_error: false,
        structured: Some(serde_json::json!({
            "job": live.id, "rows": rows, "cursor": cursor, "matched": matched,
            "user_attached_ms": user_attached.map(|d| d.as_millis() as u64),
        })),
        files: Vec::new(),
    }
}

/// How long ago a human last typed, while that is still worth saying.
fn attached_for(last: Option<Instant>) -> Option<Duration> {
    last.map(|t| t.elapsed()).filter(|ago| *ago < USER_ATTACHED)
}

/// kameo lifts a `Result` reply: the handler's `Err(String)` arrives as
/// `SendError::HandlerError`; that is the model's error text.
fn lift<M>(
    r: Result<ToolOutput, kameo::error::SendError<M, String>>,
) -> anyhow::Result<ToolOutput> {
    match r {
        Ok(out) => Ok(out),
        Err(kameo::error::SendError::HandlerError(e)) => Ok(ToolOutput::err(e)),
        Err(e) => anyhow::bail!("jobs actor: {e}"),
    }
}

/// `start`: a command on a pty as a job with stdin and a screen.
pub struct StartTool {
    jobs: ActorRef<Jobs>,
    shells: Arc<Mutex<HashMap<SessionId, Shell>>>,
    ready_timeout: Duration,
    /// `tools.pty_idle_secs`, shared with the registry's `SetPtyIdle`.
    idle_default: Arc<AtomicU64>,
    /// `tools.pty_shell`: the PTY program instead of the platform default.
    pty_shell: Option<PathBuf>,
}

impl StartTool {
    /// `shells` is [`crate::shell::ShellTool::shells`]: the default cwd is the
    /// session shell's (22 §1.5); `ready_timeout` is `tools.pty_ready_timeout_secs`;
    /// `idle_default` is the live `tools.pty_idle_secs` (61 §1.6), zero for never;
    /// `pty_shell` is `tools.pty_shell` (58 §0.3). The 155 §6.6 thresholds
    /// are the `Jobs` actor's (`jobs::start`), since `StartPty` is also
    /// built outside this crate.
    pub fn new(
        jobs: ActorRef<Jobs>,
        shells: Arc<Mutex<HashMap<SessionId, Shell>>>,
        ready_timeout: Duration,
        idle_default: Arc<AtomicU64>,
        pty_shell: Option<PathBuf>,
    ) -> Self {
        Self {
            jobs,
            shells,
            ready_timeout,
            idle_default,
            pty_shell,
        }
    }

    /// 61 §1.6: `0` is never, 1–9 become [`IDLE_MIN`], absent is the config.
    fn idle_after(&self, secs: Option<u64>) -> Option<Duration> {
        match secs {
            Some(0) => None,
            Some(s) => Some(Duration::from_secs(s.max(IDLE_MIN))),
            None => {
                Some(Duration::from_secs(self.idle_default.load(SeqCst))).filter(|d| !d.is_zero())
            }
        }
    }
}

#[async_trait::async_trait]
impl Tool for StartTool {
    const NAME: &'static str = "start";
    const RISK: Risk = Risk::Destructive;
    type Args = StartArgs;

    /// Job-shaped already (145 §0.4).
    fn promotable(&self) -> bool {
        false
    }

    fn description(&self) -> String {
        "Run a command on a pseudo-terminal as a named job you can type into (REPLs, servers, TUIs, prompts). Returns the job id and the screen; with `ready`, waits for a log regex and/or a TCP port first. cwd defaults to the shell's; variables and functions do not carry over. Use `shell` for anything that just runs and exits. A job whose screen stops changing for `idle_secs` (default 60) wakes you with `<job_idle>` and the screen; `0` for a server you only need running.".into()
    }

    async fn call(&self, args: StartArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let cwd = match &args.cwd {
            Some(c) => crate::seat::resolve(cx.session, c),
            None => match lock(&self.shells).get(&cx.session) {
                Some(s) => s.working_dir().to_path_buf(),
                None => crate::seat::cwd(cx.session)?,
            },
        };
        let idle_after = self.idle_after(args.idle_secs);
        self.jobs
            .ask(StartPty {
                session: cx.session,
                call_id: cx.call_id,
                args,
                cwd,
                ready_timeout: self.ready_timeout,
                idle_after,
                root: cx.resources.root.clone(),
                activity: activity::current(),
                pty_shell: self.pty_shell.clone(),
                program: None,
                cell: None,
            })
            .await
            .map_err(|e| anyhow::anyhow!("jobs actor: {e}"))
    }
}

/// `send`: type into a PTY job.
pub struct SendTool(pub ActorRef<Jobs>);

#[async_trait::async_trait]
impl Tool for SendTool {
    const NAME: &'static str = "send";
    const RISK: Risk = Risk::Mutating;
    type Args = SendArgs;

    /// Job-shaped already (145 §0.4).
    fn promotable(&self) -> bool {
        false
    }

    fn description(&self) -> String {
        "Type into a PTY job: `text` (+ Enter unless `enter: false`), then `keys`, or deliver a `signal`. Returns the screen once output settles. After a `<job_idle>`, this is how you answer what the program is waiting for.".into()
    }

    async fn call(&self, args: SendArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        lift(
            self.0
                .ask(SendInput {
                    session: cx.session,
                    args,
                    activity: activity::current(),
                })
                .await,
        )
    }
}

/// `screen`: the current screen of a PTY job.
pub struct ScreenTool(pub ActorRef<Jobs>);

#[async_trait::async_trait]
impl Tool for ScreenTool {
    const NAME: &'static str = "screen";
    const RISK: Risk = Risk::ReadOnly;
    type Args = ScreenArgs;

    /// Job-shaped already (145 §0.4).
    fn promotable(&self) -> bool {
        false
    }

    fn description(&self) -> String {
        "The current screen of a PTY job (120×40, blank rows dropped, cursor on the last line). For line output the log file under `read_file` is the full history.".into()
    }

    async fn call(&self, args: ScreenArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        lift(
            self.0
                .ask(Screen {
                    session: cx.session,
                    id: args.id,
                })
                .await,
        )
    }
}

/// `wait`: block for a pattern or exit.
pub struct WaitTool(pub ActorRef<Jobs>);

#[async_trait::async_trait]
impl Tool for WaitTool {
    const NAME: &'static str = "wait";
    const RISK: Risk = Risk::ReadOnly;
    type Args = WaitArgs;

    /// An explicit wait is the caller's (145 §0.4).
    fn promotable(&self) -> bool {
        false
    }

    fn description(&self) -> String {
        "Block until a PTY job (started with `start`) prints something matching `pattern`, or exits; at most `timeout` seconds (default 30). `pattern` matches output since your last `send` (or since the job started), so a prompt already on screen returns at once. Returns the match and the screen. Use it for prompts and readiness, not to babysit a build — end the turn and the `<job_result>` wakes you, or `<job_idle>` does once the screen stops changing. Not for `shell` jobs: those cannot be waited on and report as `<job_result>` on their own.".into()
    }

    async fn call(&self, args: WaitArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        lift(
            self.0
                .ask(Wait {
                    session: cx.session,
                    call_id: cx.call_id.clone(),
                    args,
                    activity: activity::current(),
                })
                .await,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::CancelJob;
    use crate::jobs::tests::{Fx, finished, fixture, list, settled};

    fn cx(fx: &Fx) -> ToolCx {
        ToolCx {
            session: fx.session,
            call_id: String::new(),
            quiet: false,
            resources: crate::SessionResources::durable_arc(&fx.dir),
            cancel: tokio_util::sync::CancellationToken::new(),
            promote_after: None,
        }
    }

    fn capture(bytes: &[u8]) -> Capture {
        let mut p = vt100::Parser::new(nz(ROWS), nz(COLS), HISTORY);
        p.process(bytes);
        Capture {
            screen: p.screen().clone(),
            revision: 1,
            exit: None,
        }
    }

    fn no_spans(_: &str) -> Vec<Range<usize>> {
        Vec::new()
    }

    fn cells(row: &[StyledRun]) -> Vec<&Cell> {
        row.iter().flat_map(|r| r.cells.iter()).collect()
    }

    fn text(row: &[StyledRun]) -> String {
        cells(row)
            .iter()
            .map(|c| c.text.as_str())
            .collect::<String>()
            .trim_end()
            .to_owned()
    }

    #[test]
    fn render_merges_same_style_cells_into_runs() {
        let s = render(
            capture(b"\x1b[31mred\x1b[0m plain"),
            PtyViewport::Live,
            &no_spans,
            0,
        );
        assert_eq!(s.origin, 0);
        assert_eq!(s.lines.len(), 40);
        let row = &s.lines[0];
        assert_eq!(row.len(), 2, "{row:?}");
        assert_eq!(row[0].style.fg, Color::Idx { idx: 1 });
        assert_eq!(text(&row[..1]), "red");
        assert_eq!(row[1].style, Style::default());
        assert_eq!(row[1].cells.len(), 117);
        assert_eq!(text(row), "red plain");
        assert_eq!(s.cursor_pos, (0, 9));
        assert!(!s.cursor_hidden);
    }

    #[test]
    fn encode_follows_screen_modes() {
        let enc = |feed: &[u8], keys: &[Key]| {
            let mut p = vt100::Parser::new(nz(ROWS), nz(COLS), HISTORY);
            p.process(feed);
            let mut out = Vec::new();
            for k in keys {
                k.encode(&mut out, p.screen())?;
            }
            Ok::<_, String>(out)
        };
        assert_eq!(enc(b"", &[Key::Up]).unwrap(), b"\x1b[A");
        // DECCKM on: application cursor keys.
        assert_eq!(enc(b"\x1b[?1h", &[Key::Up]).unwrap(), b"\x1bOA");
        let paste = [Key::Paste("hi".into())];
        assert_eq!(enc(b"", &paste).unwrap(), b"hi");
        assert_eq!(
            enc(b"\x1b[?2004h", &paste).unwrap(),
            b"\x1b[200~hi\x1b[201~"
        );
        assert_eq!(enc(b"", &[Key::Ctrl('c')]).unwrap(), b"\x03");
        assert_eq!(
            enc(b"", &[Key::Ctrl('c')]).unwrap(),
            enc(b"", &[Key::CtrlC]).unwrap()
        );
        assert_eq!(enc(b"", &[Key::Ctrl(']')]).unwrap(), b"\x1d");
        assert_eq!(
            enc(b"", &[Key::Alt('x'), Key::F(5), Key::ShiftTab]).unwrap(),
            b"\x1bx\x1b[15~\x1b[Z"
        );
        assert!(enc(b"", &[Key::Ctrl('1')]).is_err());
        assert!(enc(b"", &[Key::F(13)]).is_err());
        assert_eq!(
            serde_json::to_value([Key::Ctrl('a'), Key::F(2), Key::Paste("p".into()), Key::Up])
                .unwrap(),
            serde_json::json!([{"ctrl": "a"}, {"f": 2}, {"paste": "p"}, "up"])
        );
    }

    #[test]
    fn render_pages_retained_history() {
        let mut feed = Vec::new();
        for i in 0..600 {
            feed.extend_from_slice(format!("line {i}\r\n").as_bytes());
        }
        // 601 logical rows; the buffer keeps the last 512 + 40, so absolute
        // row `k` is `line {k + 49}`.
        let live = render(capture(&feed), PtyViewport::Live, &no_spans, 0);
        assert_eq!(live.retained, 512);
        assert_eq!(live.origin, 512);
        assert_eq!(live.lines.len(), 40);
        assert_eq!(text(&live.lines[0]), "line 561");
        assert_eq!(text(&live.lines[38]), "line 599");
        assert_eq!(text(&live.lines[39]), "");

        let hist = render(
            capture(&feed),
            PtyViewport::History {
                offset: 10,
                height: 80,
            },
            &no_spans,
            0,
        );
        assert_eq!(hist.origin, 512 - 10 - 80);
        assert_eq!(hist.lines.len(), 80);
        assert_eq!(hist.wrapped.len(), 80);
        assert_eq!(text(&hist.lines[0]), "line 471");
        assert_eq!(text(&hist.lines[79]), "line 550");
        let page = hist.into_page(SessionId::new(), JobId("j1".into()));
        assert!(page.cursor.is_none() && !page.truncated && page.redaction_complete);

        // Past the top: clamped to what is retained.
        let top = render(
            capture(&feed),
            PtyViewport::History {
                offset: 600,
                height: 200,
            },
            &no_spans,
            0,
        );
        assert_eq!((top.origin, top.lines.len()), (0, 0));
    }

    const LITERAL: &str = "SECRET-LITERAL-1234567890";

    fn literal_spans(text: &str) -> Vec<Range<usize>> {
        text.match_indices(LITERAL)
            .map(|(i, m)| i..i + m.len())
            .collect()
    }

    /// 51 §7.2: a value split by a soft wrap and by a style change is masked
    /// as a whole; a page starting at a full buffer's edge says so.
    #[test]
    fn render_masks_secrets_across_wrap_and_style() {
        let mut feed = vec![b'x'; 110];
        feed.extend_from_slice(b"\x1b[31m");
        feed.extend_from_slice(&LITERAL.as_bytes()[..10]);
        feed.extend_from_slice(b"\x1b[0m");
        feed.extend_from_slice(&LITERAL.as_bytes()[10..]);
        feed.extend_from_slice(b" tail");
        let s = render(
            capture(&feed),
            PtyViewport::Live,
            &literal_spans,
            LITERAL.len(),
        );
        assert!(s.wrapped[0] && !s.wrapped[1], "{:?}", s.wrapped);
        let (r0, r1) = (cells(&s.lines[0]), cells(&s.lines[1]));
        assert!(r0[..110].iter().all(|c| c.text == "x"));
        assert!(
            r0[110..].iter().all(|c| c.text == "•"),
            "{:?}",
            text(&s.lines[0])
        );
        assert!(
            r1[..15].iter().all(|c| c.text == "•"),
            "{:?}",
            text(&s.lines[1])
        );
        assert_eq!(
            text(&s.lines[1]),
            "•••••••••••••••  tail".replace("  ", " ")
        );
        assert!(s.lines[0].iter().all(|run| run.style == Style::default()));
        let shown: String = s
            .lines
            .iter()
            .map(|l| text(l))
            .collect::<Vec<_>>()
            .join("\n");
        for i in 0..=LITERAL.len() - 4 {
            assert!(!shown.contains(&LITERAL[i..i + 4]), "{shown}");
        }
        assert!(s.redaction_complete);

        let mut feed = Vec::new();
        for i in 0..600 {
            feed.extend_from_slice(format!("line {i}\r\n").as_bytes());
        }
        let edge = |offset| {
            render(
                capture(&feed),
                PtyViewport::History { offset, height: 80 },
                &literal_spans,
                LITERAL.len(),
            )
        };
        assert!(!edge(512 - 80).redaction_complete);
        assert!(edge(10).redaction_complete);
    }

    #[tokio::test]
    async fn pty_screen_unchanged_and_unavailable() {
        let fx = fixture(Duration::from_millis(100), Duration::from_secs(60)).await;
        let (start, ..) = tools(&fx);
        let out = Tool::call(&start, start_args("sleep 30"), cx(&fx))
            .await
            .unwrap();
        assert!(
            out.content.starts_with("job j1 · running"),
            "{}",
            out.content
        );
        let ask = |id: &str, if_revision| {
            fx.jobs.ask(PtyScreen {
                session: fx.session,
                id: JobId(id.into()),
                viewport: PtyViewport::Live,
                if_revision,
            })
        };
        let PtyScreenReply::Updated(page) = ask("j1", None).await.unwrap() else {
            panic!("expected a page");
        };
        assert_eq!((page.lines.len(), page.origin, page.rows), (40, 0, ROWS));
        assert!(page.cursor.is_some() && page.exit.is_none());
        let terms = fx
            .jobs
            .ask(ListTerminals {
                sessions: vec![fx.session],
            })
            .await
            .unwrap();
        assert_eq!(terms.len(), 1);
        assert_eq!(
            (terms[0].revision, terms[0].state.as_str()),
            (page.revision, "running")
        );
        assert!(matches!(
            ask("j1", Some(page.revision)).await.unwrap(),
            PtyScreenReply::Unchanged { viewport: PtyViewport::Live, revision } if revision == page.revision
        ));
        assert!(matches!(
            ask("j9", None).await.unwrap(),
            PtyScreenReply::Unavailable
        ));
        cancel(&fx, "j1").await;
    }

    #[tokio::test]
    async fn pty_input_types_and_marks_user_attached() {
        let fx = fixture(Duration::from_millis(100), Duration::from_secs(60)).await;
        let (start, _, screen, _) = tools(&fx);
        let out = Tool::call(&start, start_args("cat"), cx(&fx))
            .await
            .unwrap();
        assert!(
            out.content.starts_with("job j1 · running"),
            "{}",
            out.content
        );
        let out = Tool::call(&screen, ScreenArgs { id: "j1".into() }, cx(&fx))
            .await
            .unwrap();
        assert!(!out.content.contains("user attached"), "{}", out.content);

        // A human types: bytes land, the call returns without a screen.
        fx.jobs
            .ask(PtyInput {
                session: fx.session,
                id: JobId("j1".into()),
                text: Some("hello".into()),
                keys: vec![Key::Enter],
            })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        let out = Tool::call(&screen, ScreenArgs { id: "j1".into() }, cx(&fx))
            .await
            .unwrap();
        // Typed, echoed by the tty, echoed back by cat.
        assert_eq!(out.content.matches("hello").count(), 2, "{}", out.content);
        assert!(
            out.content.ends_with("user attached · typed 0s ago"),
            "{}",
            out.content
        );
        assert!(out.structured.unwrap()["user_attached_ms"].is_u64());

        // The trailer expires.
        assert!(attached_for(Some(Instant::now())).is_some());
        assert!(attached_for(Some(Instant::now() - USER_ATTACHED)).is_none());
        assert!(attached_for(None).is_none());

        // Unknown or exited jobs refuse; no such thing as typing into a name.
        let err = fx
            .jobs
            .ask(PtyInput {
                session: fx.session,
                id: JobId("j9".into()),
                text: None,
                keys: vec![],
            })
            .await
            .unwrap_err();
        assert!(
            matches!(&err, kameo::error::SendError::HandlerError(e) if e == "no such job"),
            "{err}"
        );
        cancel(&fx, "j1").await;
        finished(&fx).await;
        let err = fx
            .jobs
            .ask(PtyInput {
                session: fx.session,
                id: JobId("j1".into()),
                text: Some("x".into()),
                keys: vec![],
            })
            .await
            .unwrap_err();
        assert!(
            matches!(&err, kameo::error::SendError::HandlerError(e) if e.starts_with("j1 ")),
            "{err}"
        );
    }

    /// A viewer's own emulator fed `replay` then the stream shows what the
    /// daemon's does; `resize` changes the grid every reader reports; the
    /// stream ends with the exit; a finished job attaches but never resizes.
    #[tokio::test]
    async fn attach_replays_streams_and_resizes() {
        let fx = fixture(Duration::from_millis(100), Duration::from_secs(60)).await;
        let (start, ..) = tools(&fx);
        let out = Tool::call(&start, start_args("cat"), cx(&fx))
            .await
            .unwrap();
        assert!(
            out.content.starts_with("job j1 · running"),
            "{}",
            out.content
        );
        let type_ = |text: &str| {
            fx.jobs.ask(PtyInput {
                session: fx.session,
                id: JobId("j1".into()),
                text: Some(text.into()),
                keys: vec![Key::Enter],
            })
        };
        type_("before").await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;

        let attached = fx
            .jobs
            .ask(PtyAttach {
                session: fx.session,
                id: JobId("j1".into()),
                size: Some((24, 80)),
            })
            .await
            .unwrap();
        assert_eq!((attached.rows, attached.cols), (24, 80));
        assert!(attached.exit.is_none());
        let mut viewer = vt100::Parser::new(nz(24), nz(80), 1000);
        viewer.process(&attached.replay);
        assert!(
            viewer.screen().contents().contains("before"),
            "{}",
            viewer.screen().contents()
        );
        let terms = fx
            .jobs
            .ask(ListTerminals {
                sessions: vec![fx.session],
            })
            .await
            .unwrap();
        assert_eq!((terms[0].rows, terms[0].cols), (24, 80));

        // Bytes after the snapshot arrive in order; the program saw SIGWINCH.
        let mut rx = attached.rx;
        fx.jobs
            .ask(PtyResize {
                session: fx.session,
                id: JobId("j1".into()),
                rows: 30,
                cols: 100,
            })
            .await
            .unwrap();
        type_("after").await.unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let out = tokio::time::timeout_at(deadline, rx.recv())
                .await
                .unwrap()
                .unwrap();
            let PtyOut::Data(bytes) = out else {
                panic!("exit before echo")
            };
            viewer.process(&bytes);
            if viewer.screen().contents().matches("after").count() == 2 {
                break;
            }
        }
        let daemon = fx
            .jobs
            .ask(PtyScreen {
                session: fx.session,
                id: JobId("j1".into()),
                viewport: PtyViewport::Live,
                if_revision: None,
            })
            .await
            .unwrap();
        let PtyScreenReply::Updated(page) = daemon else {
            panic!("expected a page")
        };
        assert_eq!((page.rows, page.cols, page.lines.len()), (30, 100, 30));
        assert!(matches!(
            fx.jobs
                .ask(PtyResize { session: fx.session, id: JobId("j1".into()), rows: 1, cols: 5 })
                .await
                .unwrap_err(),
            kameo::error::SendError::HandlerError(e) if e.contains("out of range")
        ));

        // The model's own kill (14 §1.2 as-built): inline, one line, no screen
        // in the event — the attach below still has the last frame.
        cancel_from(&fx, "j1", CancelOrigin::Tool).await;
        let exit = loop {
            match tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .unwrap()
                .unwrap()
            {
                PtyOut::Data(_) => {}
                PtyOut::Exit(e) => break e,
            }
        };
        assert_eq!(exit, JobExit::Killed);
        assert_eq!(
            finished(&fx).await,
            (JobExit::Killed, "killed by cancel_job".to_owned(), true)
        );
        // A finished job still attaches (last screen, exit set); a resize does not.
        let again = fx
            .jobs
            .ask(PtyAttach {
                session: fx.session,
                id: JobId("j1".into()),
                size: Some((24, 80)),
            })
            .await
            .unwrap();
        assert_eq!(
            (again.rows, again.cols, again.exit),
            (30, 100, Some(JobExit::Killed))
        );
        assert!(
            fx.jobs
                .ask(PtyResize {
                    session: fx.session,
                    id: JobId("j1".into()),
                    rows: 24,
                    cols: 80
                })
                .await
                .is_err()
        );
    }

    fn tools(fx: &Fx) -> (StartTool, SendTool, ScreenTool, WaitTool) {
        (
            start_idle(fx, Duration::ZERO),
            SendTool(fx.jobs.clone()),
            ScreenTool(fx.jobs.clone()),
            WaitTool(fx.jobs.clone()),
        )
    }

    /// A `start` whose default `idle_secs` is `idle` (61; the tool clamps
    /// only the argument, so tests run at any interval).
    fn start_idle(fx: &Fx, idle: Duration) -> StartTool {
        StartTool::new(
            fx.jobs.clone(),
            Arc::default(),
            Duration::from_secs(30),
            Arc::new(AtomicU64::new(idle.as_secs())),
            None,
        )
    }

    fn start_args(command: &str) -> StartArgs {
        StartArgs {
            command: command.into(),
            name: None,
            cwd: None,
            env: None,
            ready: None,
            idle_secs: None,
        }
    }

    /// `(nudge, quiet_secs, user_typed_secs, output)` of every `Idle` on the bus.
    fn idles(fx: &Fx) -> Vec<(u32, u64, Option<u64>, String)> {
        lock(&fx.events)
            .iter()
            .filter_map(|e| match e {
                JobEvent::Idle {
                    nudge,
                    quiet_secs,
                    user_typed_secs,
                    output,
                    ..
                } => Some((*nudge, *quiet_secs, *user_typed_secs, output.clone())),
                _ => None,
            })
            .collect()
    }

    /// The `why` of every `Idle` on the bus (155 §6.5).
    fn whys(fx: &Fx) -> Vec<String> {
        lock(&fx.events)
            .iter()
            .filter_map(|e| match e {
                JobEvent::Idle { why, .. } => Some(why.clone()),
                _ => None,
            })
            .collect()
    }

    /// 155 rule tests: a handle with no terminal behind it (unix: adopting
    /// our own pid is free; on Windows it would put the test in a job).
    #[cfg(unix)]
    fn bare_handle(touched: Instant) -> PtyHandle {
        let pid = std::process::id();
        PtyHandle {
            writer: Mutex::new(None),
            parser: Mutex::new(vt100::Parser::new(nz(ROWS), nz(COLS), HISTORY)),
            revision: AtomicU64::new(0),
            output: Notify::new(),
            stream: broadcast::channel(1).0,
            master: Mutex::new(None),
            pid,
            tree: ProcessTree::adopt(pid).unwrap(),
            exit: Mutex::new(None),
            user_input: Mutex::new(None),
            touched: Mutex::new(touched),
            idle_wake: Notify::new(),
            waiting: AtomicUsize::new(0),
            interacted: AtomicBool::new(false),
            cursor: AtomicU64::new(0),
            snapshotted: AtomicU64::new(u64::MAX),
            sent_at: Mutex::new(None),
        }
    }

    #[cfg(unix)]
    const THRESHOLDS: PtyThresholds = PtyThresholds {
        reaction: Duration::from_secs(10),
        tree_cpu_pct: 2,
    };

    #[cfg(unix)]
    fn tree_sample(cpu_ms: u64) -> Option<platform::TreeSample> {
        Some(platform::TreeSample {
            processes: 1,
            cpu: Duration::from_millis(cpu_ms),
        })
    }

    /// 155 §6.3 / §11: screen unchanged + tree busy → no fire, deferred one
    /// interval with `nudges` untouched; tree quiet → the fire with its `why`.
    #[cfg(unix)]
    #[test]
    fn idle_rule_defers_on_a_busy_tree_and_fires_with_why_when_quiet() {
        let t0 = Instant::now();
        let s = |secs: u64| t0 + Duration::from_secs(secs);
        let at = |secs| Some(tokio::time::Instant::from_std(s(secs)));
        let pty = bare_handle(t0);
        let mut idle = Idle::new(Duration::from_secs(10), t0);
        let mut tree = TreeQuiet::new(THRESHOLDS);
        // 500 ms of CPU over one second: busy.
        tree.saw(s(8), tree_sample(0), true);
        tree.saw(s(9), tree_sample(500), true);
        assert_eq!(idle.deadline(&pty), at(10));
        assert_eq!(idle_due(&mut idle, &tree, &pty, s(10)), None);
        assert_eq!(idle.nudges, 0);
        assert_eq!(idle.deadline(&pty), at(20), "deferred by one interval");
        // Quiet again: fires at the moved deadline with the quiet reason.
        tree.saw(s(19), tree_sample(505), true);
        assert_eq!(
            idle_due(&mut idle, &tree, &pty, s(20)),
            Some((
                1,
                20,
                "screen unchanged for 20 s, process tree quiet".into()
            ))
        );
        assert_eq!(
            idle.deadline(&pty),
            None,
            "disarmed until a change or touch"
        );
        // A `wait` in flight still suppresses the quiet fire.
        *lock(&pty.touched) = s(25);
        pty.waiting.store(1, SeqCst);
        assert_eq!(idle_due(&mut idle, &tree, &pty, s(45)), None);
    }

    /// 155 §6.4 / §11: a `send` with no screen or tree movement for `reaction`
    /// fires once; suppressed while `waiting > 0`; movement clears it; `0` is off.
    #[cfg(unix)]
    #[test]
    fn reaction_check_fires_once_when_nothing_answers_a_send() {
        let t0 = Instant::now();
        let s = |secs: u64| t0 + Duration::from_secs(secs);
        let r = THRESHOLDS.reaction;
        let pty = bare_handle(t0);
        let idle = Idle::new(Duration::from_secs(60), t0);
        let tree = TreeQuiet::new(THRESHOLDS);
        assert_eq!(
            reaction_due(&idle, &tree, &pty, r, s(1)),
            None,
            "nothing sent"
        );

        *lock(&pty.sent_at) = Some(s(1));
        assert_eq!(reaction_due(&idle, &tree, &pty, r, s(5)), None, "too early");
        assert!(lock(&pty.sent_at).is_some());
        assert_eq!(reaction_due(&idle, &tree, &pty, r, s(11)), Some((1, 11)));
        assert_eq!(reaction_due(&idle, &tree, &pty, r, s(12)), None, "once");

        // Suppressed by a `wait`: its timeout is the answer, no late fire.
        *lock(&pty.sent_at) = Some(s(20));
        pty.waiting.store(1, SeqCst);
        assert_eq!(reaction_due(&idle, &tree, &pty, r, s(31)), None);
        assert!(lock(&pty.sent_at).is_none());
        pty.waiting.store(0, SeqCst);

        // A screen change after the send clears the clock.
        *lock(&pty.sent_at) = Some(s(40));
        let mut changed = Idle::new(Duration::from_secs(60), t0);
        changed.last_change = s(41);
        assert_eq!(reaction_due(&changed, &tree, &pty, r, s(55)), None);
        assert!(lock(&pty.sent_at).is_none());

        // Tree movement after the send clears it too.
        *lock(&pty.sent_at) = Some(s(60));
        let mut busy = TreeQuiet::new(THRESHOLDS);
        busy.saw(s(60), tree_sample(0), true);
        busy.saw(s(61), tree_sample(500), true);
        assert_eq!(reaction_due(&idle, &busy, &pty, r, s(75)), None);
        assert!(lock(&pty.sent_at).is_none());

        // `pty_reaction_secs: 0` is off.
        *lock(&pty.sent_at) = Some(s(80));
        assert_eq!(
            reaction_due(&idle, &tree, &pty, Duration::ZERO, s(100)),
            None
        );
    }

    /// Poll until `n` idle events are on the bus, or fail at `within`.
    async fn idles_within(
        fx: &Fx,
        n: usize,
        within: Duration,
    ) -> Vec<(u32, u64, Option<u64>, String)> {
        let deadline = Instant::now() + within;
        loop {
            let v = idles(fx);
            if v.len() >= n {
                return v;
            }
            assert!(
                Instant::now() < deadline,
                "only {} idle event(s): {v:?}",
                v.len()
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    fn send_args(id: &str) -> SendArgs {
        SendArgs {
            id: id.into(),
            text: None,
            enter: None,
            keys: None,
            signal: None,
        }
    }

    fn rows(out: &ToolOutput) -> Vec<String> {
        serde_json::from_value(out.structured.as_ref().unwrap()["rows"].clone()).unwrap()
    }

    fn free_port() -> u16 {
        std::net::TcpListener::bind(("127.0.0.1", 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    /// A human's kill: the model gets the one-line `<job_result>`.
    async fn cancel(fx: &Fx, id: &str) {
        cancel_from(fx, id, CancelOrigin::Ui).await;
    }

    async fn cancel_from(fx: &Fx, id: &str, origin: CancelOrigin) {
        fx.jobs
            .ask(CancelJob {
                session: fx.session,
                id: JobId(id.into()),
                origin,
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn repl_round_trip() {
        let fx = fixture(Duration::from_secs(3), Duration::from_secs(60)).await;
        let (start, send, ..) = tools(&fx);
        let mut args = start_args("python3 -q");
        args.ready = Some(Ready {
            log: Some(">>> ".into()),
            port: None,
            host: None,
            timeout: None,
        });
        let out = Tool::call(&start, args, cx(&fx)).await.unwrap();
        assert!(!out.is_error, "{}", out.content);
        assert!(
            out.content.starts_with("job j1 · running · pid ")
                && out.content.contains("· ready: log matched `>>> `"),
            "{}",
            out.content
        );
        assert_eq!(
            rows(&out).last().map(String::as_str),
            Some(">>>"),
            "{}",
            out.content
        );

        let mut args = send_args("j1");
        args.text = Some("1+1".into());
        let out = Tool::call(&send, args, cx(&fx)).await.unwrap();
        let rows = rows(&out);
        assert!(rows.iter().any(|r| r == "2"), "{}", out.content);
        assert!(
            rows.iter().filter(|r| r.starts_with(">>>")).count() >= 2,
            "{}",
            out.content
        );
        assert!(
            out.content.starts_with("job j1 · running · "),
            "{}",
            out.content
        );

        let mut args = send_args("j1");
        args.keys = Some(vec![Key::CtrlD]);
        let out = Tool::call(&send, args, cx(&fx)).await.unwrap();
        assert!(
            out.content.starts_with("job j1 · exited 0 · "),
            "{}",
            out.content
        );
        assert_eq!(finished(&fx).await.0, JobExit::Exited(0));
        assert!(matches!(
            lock(&fx.events).iter().last(),
            Some(JobEvent::Finished {
                pty: true,
                inline: false,
                ..
            })
        ));
        let v = settled(&fx, Duration::from_secs(1)).await;
        assert!(v.pty && v.state == "exited 0", "{v:?}");
    }

    #[tokio::test]
    async fn screen_is_rendered_not_bytes() {
        let fx = fixture(Duration::from_secs(3), Duration::from_secs(60)).await;
        let (start, ..) = tools(&fx);
        let out = Tool::call(&start, start_args(r"printf 'abc\rX\n'"), cx(&fx))
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.ends_with("exit code: 0"), "{}", out.content);
        let (exit, screen, inline) = finished(&fx).await;
        assert_eq!(exit, JobExit::Exited(0));
        assert!(inline);
        assert!(screen.lines().any(|l| l == "Xbc"), "{screen:?}");
        let log = std::fs::read(&list(&fx).await[0].log).unwrap();
        assert!(log.windows(5).any(|w| w == b"abc\rX"), "{log:?}");
    }

    /// The `\[` in the pattern: zsh reads an unescaped `*[` as a broken glob (`bad pattern`),
    /// so the test runs under whatever `$SHELL` is.
    #[tokio::test]
    async fn query_responder() {
        let fx = fixture(Duration::from_secs(5), Duration::from_secs(60)).await;
        let (start, ..) = tools(&fx);
        let out = Tool::call(
            &start,
            start_args(
                r#"stty -echo; printf '\033[H\033[2J\033[6n'; read -t 2 -rs -d R r; echo "${r#*\[}""#,
            ),
            cx(&fx),
        )
        .await
        .unwrap();
        let (_, screen, _) = finished(&fx).await;
        assert!(screen.lines().any(|l| l == "1;1"), "{}", out.content);
        let log = std::fs::read_to_string(&list(&fx).await[0].log).unwrap();
        assert!(!log.contains("1;1R"), "{log:?}");
    }

    #[tokio::test]
    async fn wait_pattern_and_timeout() {
        let fx = fixture(Duration::from_millis(100), Duration::from_secs(60)).await;
        let (start, _, _, wait) = tools(&fx);
        let out = Tool::call(&start, start_args("sleep 1; echo READY; sleep 30"), cx(&fx))
            .await
            .unwrap();
        assert!(
            out.content.starts_with("job j1 · running"),
            "{}",
            out.content
        );
        let t = Instant::now();
        let out = Tool::call(
            &wait,
            WaitArgs {
                id: "j1".into(),
                pattern: Some("REA+DY".into()),
                timeout: None,
            },
            cx(&fx),
        )
        .await
        .unwrap();
        assert!(t.elapsed() < Duration::from_secs(5));
        assert!(
            out.content.contains("matched: \"READY\""),
            "{}",
            out.content
        );
        assert_eq!(out.structured.as_ref().unwrap()["matched"], "READY");
        let out = Tool::call(
            &wait,
            WaitArgs {
                id: "j1".into(),
                pattern: Some("NEVER".into()),
                timeout: Some(1),
            },
            cx(&fx),
        )
        .await
        .unwrap();
        assert!(!out.is_error);
        assert!(
            out.content.contains("timed out after 1s"),
            "{}",
            out.content
        );
        cancel(&fx, "j1").await;
    }

    /// `start` returns before the program prints; block until `needle` is
    /// in the log so the following `wait` sees it already on screen.
    async fn logged(fx: &Fx, needle: &str) {
        let log = list(fx).await[0].log.clone();
        let end = Instant::now() + Duration::from_secs(5);
        while !std::fs::read_to_string(&log).is_ok_and(|s| s.contains(needle)) {
            assert!(Instant::now() < end, "{needle} never logged");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    fn wait_args(pattern: &str, timeout: u64) -> WaitArgs {
        WaitArgs {
            id: "j1".into(),
            pattern: Some(pattern.into()),
            timeout: Some(timeout),
        }
    }

    #[tokio::test]
    async fn wait_matches_output_already_present() {
        let fx = fixture(Duration::from_millis(100), Duration::from_secs(60)).await;
        let (start, _, _, wait) = tools(&fx);
        Tool::call(&start, start_args("echo READY; sleep 30"), cx(&fx))
            .await
            .unwrap();
        logged(&fx, "READY").await;
        let t = Instant::now();
        let out = Tool::call(&wait, wait_args("READY", 10), cx(&fx))
            .await
            .unwrap();
        assert!(t.elapsed() < Duration::from_secs(2));
        assert!(
            out.content
                .contains("matched (already on screen): \"READY\""),
            "{}",
            out.content
        );
        assert_eq!(out.structured.as_ref().unwrap()["matched"], "READY");
        cancel(&fx, "j1").await;
    }

    #[tokio::test]
    async fn send_advances_cursor_so_wait_ignores_earlier_output() {
        let fx = fixture(Duration::from_millis(100), Duration::from_secs(60)).await;
        let (start, send, _, wait) = tools(&fx);
        Tool::call(&start, start_args("echo READY; sleep 30"), cx(&fx))
            .await
            .unwrap();
        logged(&fx, "READY").await;
        Tool::call(&send, send_args("j1"), cx(&fx)).await.unwrap();
        let out = Tool::call(&wait, wait_args("READY", 1), cx(&fx))
            .await
            .unwrap();
        assert!(
            out.content.contains("timed out after 1s"),
            "{}",
            out.content
        );
        cancel(&fx, "j1").await;
    }

    #[tokio::test]
    async fn wait_match_advances_cursor() {
        let fx = fixture(Duration::from_millis(100), Duration::from_secs(60)).await;
        let (start, _, _, wait) = tools(&fx);
        Tool::call(&start, start_args("echo READY; sleep 30"), cx(&fx))
            .await
            .unwrap();
        logged(&fx, "READY").await;
        let out = Tool::call(&wait, wait_args("READY", 10), cx(&fx))
            .await
            .unwrap();
        assert!(out.content.contains("matched"), "{}", out.content);
        let out = Tool::call(&wait, wait_args("READY", 1), cx(&fx))
            .await
            .unwrap();
        assert!(
            out.content.contains("timed out after 1s"),
            "{}",
            out.content
        );
        cancel(&fx, "j1").await;
    }

    #[tokio::test]
    async fn ready_port() {
        let fx = fixture(Duration::from_millis(100), Duration::from_secs(60)).await;
        let (start, ..) = tools(&fx);
        let port = free_port();
        let mut args = start_args(&format!("python3 -m http.server {port}"));
        args.ready = Some(Ready {
            log: None,
            port: Some(port),
            host: None,
            timeout: None,
        });
        let out = Tool::call(&start, args, cx(&fx)).await.unwrap();
        assert!(
            out.content.contains(&format!("· ready: port {port}")),
            "{}",
            out.content
        );

        let wrong = free_port();
        let mut args = start_args("sleep 30");
        args.ready = Some(Ready {
            log: None,
            port: Some(wrong),
            host: None,
            timeout: Some(1),
        });
        let out = Tool::call(&start, args, cx(&fx)).await.unwrap();
        assert!(!out.is_error);
        assert!(
            out.content
                .contains(&format!("not ready after 1s (waiting on: port {wrong})")),
            "{}",
            out.content
        );
        cancel(&fx, "j1").await;
        cancel(&fx, "j2").await;
    }

    #[tokio::test]
    async fn name_resolves_and_cancel_kills() {
        let fx = fixture(Duration::from_millis(100), Duration::from_secs(60)).await;
        let (start, _, screen, _) = tools(&fx);
        let mut args = start_args("sleep 30");
        args.name = Some("srv".into());
        let out = Tool::call(&start, args, cx(&fx)).await.unwrap();
        assert!(
            out.content.starts_with("job j1 (srv) · running"),
            "{}",
            out.content
        );
        let pid = out.structured.as_ref().unwrap()["pid"].as_u64().unwrap() as u32;

        let out = Tool::call(&screen, ScreenArgs { id: "srv".into() }, cx(&fx))
            .await
            .unwrap();
        assert!(
            out.content.starts_with("job j1 (srv) · running"),
            "{}",
            out.content
        );
        let table = jobs::table(&list(&fx).await);
        assert!(table.contains("pty srv: sleep 30"), "{table}");

        // A second running job may not reuse the name.
        let mut args = start_args("sleep 30");
        args.name = Some("srv".into());
        let out = Tool::call(&start, args, cx(&fx)).await.unwrap();
        assert!(
            out.is_error && out.content.contains("already named `srv` (j1)"),
            "{}",
            out.content
        );

        cancel(&fx, "j1").await;
        assert_eq!(settled(&fx, Duration::from_secs(2)).await.state, "killed");
        let gone = Instant::now() + Duration::from_secs(1);
        while platform::process::pid_alive(pid) {
            assert!(Instant::now() < gone, "pid {pid} still alive");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let out = Tool::call(&screen, ScreenArgs { id: "srv".into() }, cx(&fx))
            .await
            .unwrap();
        assert!(out.is_error);
        assert_eq!(out.content, "j1 (srv) killed — start it again");
    }

    /// 47 §3.2: a ready PTY is an open `Kind::Pty` operation, nothing settles
    /// while it serves; `cancel_job` settles it only once the reaper has the
    /// exit, with the result the `start` call did not return.
    #[tokio::test]
    async fn ready_pty_stays_open_until_reaped() {
        use crate::activity::test_sink::Seen;
        use crate::jobs::tests::{certified, ledger_until};
        let (fx, ledger, seen) =
            certified(Duration::from_millis(100), Duration::from_secs(60)).await;
        let (start, ..) = tools(&fx);
        let mut args = start_args("echo READY; sleep 30");
        args.ready = Some(Ready {
            log: Some("READY".into()),
            port: None,
            host: None,
            timeout: None,
        });
        let out = Tool::call(&start, args, cx(&fx)).await.unwrap();
        assert!(
            out.content.contains("· ready: log matched"),
            "{}",
            out.content
        );
        let pid = out.structured.as_ref().unwrap()["pid"].as_u64().unwrap() as u32;
        let v = crate::activity::test_sink::drained(&ledger, &seen).await;
        let [Seen::Admitted(op)] = v.as_slice() else {
            panic!("expected only the admission, got {v:?}");
        };
        assert_eq!(op.kind, Kind::Pty(JobId("j1".into())));
        let op = op.id;

        cancel(&fx, "j1").await;
        assert_eq!(settled(&fx, Duration::from_secs(2)).await.state, "killed");
        // The row closed after the reaper returned: the pid is already gone.
        assert!(!platform::process::pid_alive(pid));
        let v = ledger_until(&ledger, &seen, Duration::from_secs(2), |v| v.len() >= 2).await;
        assert_eq!(v[1..], [Seen::Result(op)]);
        // `/jobs kill`: the result rides the guard as one line, not the screen.
        assert!(matches!(
            lock(&fx.events).iter().last(),
            Some(JobEvent::Finished {
                pty: true,
                inline: false,
                exit: JobExit::Killed,
                output,
                ..
            }) if output == "killed by the user"
        ));
    }

    /// 47 §4.2 rule 4: a `&` child that survives the shell's exit (SIGHUP
    /// ignored) lives on in its process group, holding the slave or not (the
    /// tty is revoked with its leader on some platforms). The job settles on
    /// the reaper's exit with its result; the survivor is its own operation
    /// until it actually exits.
    /// Pinned to `/bin/sh`: whether a `&` child of a `-c` shell outlives the exit holding the
    /// tty is the shell's business (zsh's differs), and this test is about the ledger, not the shell.
    #[tokio::test]
    async fn tty_holder_outliving_exit_keeps_an_operation_open() {
        use crate::activity::test_sink::Seen;
        use crate::jobs::tests::{certified, ledger_until};
        let (fx, ledger, seen) = certified(Duration::from_secs(3), Duration::from_secs(60)).await;
        let start = StartTool::new(
            fx.jobs.clone(),
            Arc::default(),
            Duration::from_secs(30),
            Arc::new(AtomicU64::new(0)),
            Some(PathBuf::from("/bin/sh")),
        );
        let out = Tool::call(
            &start,
            start_args("trap '' HUP; sleep 2 & echo up"),
            cx(&fx),
        )
        .await
        .unwrap();
        assert!(out.content.ends_with("exit code: 0"), "{}", out.content);
        let v = ledger_until(&ledger, &seen, Duration::from_secs(2), |v| {
            v.iter().any(|s| matches!(s, Seen::Finished(_)))
        })
        .await;
        let Seen::Admitted(job) = &v[0] else {
            panic!("{v:?}")
        };
        assert_eq!(job.kind, Kind::Pty(JobId("j1".into())));
        let holder = v
            .iter()
            .find_map(|s| match s {
                Seen::Admitted(op) if op.kind == Kind::Command("pty background process") => {
                    Some(op.id)
                }
                _ => None,
            })
            .expect("holder operation admitted");
        assert!(v.contains(&Seen::Finished(job.id)), "{v:?}");
        assert!(!v.contains(&Seen::Finished(holder)), "{v:?}");
        let v = ledger_until(&ledger, &seen, Duration::from_secs(4), |v| {
            v.contains(&Seen::Finished(holder))
        })
        .await;
        assert!(!v.iter().any(|s| matches!(s, Seen::Unknown(..))), "{v:?}");
    }

    #[test]
    fn strip_and_queries() {
        assert_eq!(
            strip(b"\x1b[32m>>> \x1b[0m\x1b]0;title\x07x\x1b(B"),
            ">>> x"
        );
        // `>>> ` is 4 kept bytes; the raw index past them skips the SGR prefix.
        assert_eq!(raw_end(b"\x1b[32m>>> \x1b[0mx", 4), 9);
        assert_eq!(raw_end(b"ab", 5), 2);
        assert!(matches!(query(b"\x1b[6n", (1, 4)), Scan::Reply(4, r) if &*r == b"\x1b[2;5R"));
        assert!(matches!(query(b"\x1b]11;", (0, 0)), Scan::Partial));
        assert!(matches!(query(b"\x1b[31m", (0, 0)), Scan::None));
    }

    const IDLE: Duration = Duration::from_secs(1);

    async fn start_quiet(fx: &Fx, command: &str) -> ToolOutput {
        let out = Tool::call(&start_idle(fx, IDLE), start_args(command), cx(fx))
            .await
            .unwrap();
        assert!(out.content.contains("· running ·"), "{}", out.content);
        out
    }

    /// 61 §5.1: one nudge per quiet stretch, none after; a human's typing is
    /// reported on it and is not the model's interaction.
    #[tokio::test]
    async fn pty_idle_fires_once_after_quiet() {
        let fx = fixture(Duration::from_millis(100), Duration::from_secs(60)).await;
        start_quiet(&fx, r"printf 'hi\n'; sleep 30").await;
        fx.jobs
            .ask(PtyInput {
                session: fx.session,
                id: JobId("j1".into()),
                text: Some("x".into()),
                keys: Vec::new(),
            })
            .await
            .unwrap();
        let v = idles_within(&fx, 1, Duration::from_secs(3)).await;
        let (nudge, quiet, typed, output) = &v[0];
        assert_eq!(*nudge, 1);
        assert!(*quiet <= 2, "{quiet}");
        assert!(typed.is_some(), "{v:?}");
        assert!(
            output.contains("hi") && output.contains("\ncursor: row "),
            "{output:?}"
        );
        tokio::time::sleep(2 * IDLE + Duration::from_millis(500)).await;
        assert_eq!(idles(&fx).len(), 1, "{:?}", idles(&fx));
        cancel(&fx, "j1").await;
    }

    /// 61 §5.2: a screen change re-arms; the second nudge's `quiet` counts
    /// from that change.
    #[tokio::test]
    async fn pty_idle_rearms_on_screen_change() {
        let fx = fixture(Duration::from_millis(100), Duration::from_secs(60)).await;
        start_quiet(&fx, "echo a; sleep 2; echo b; sleep 30").await;
        let v = idles_within(&fx, 2, Duration::from_secs(8)).await;
        assert_eq!((v[0].0, v[1].0), (1, 2), "{v:?}");
        assert!(!v[0].3.contains('b') && v[1].3.contains('b'), "{v:?}");
        assert!((2..=3).contains(&v[1].1), "{v:?}");
        cancel(&fx, "j1").await;
    }

    /// 61 §5.3: a `send` after a nudge restarts the timer at the base
    /// interval and the next nudge is again the first of its run.
    #[tokio::test]
    async fn pty_idle_send_resets_and_resets_backoff() {
        let fx = fixture(Duration::from_millis(100), Duration::from_secs(60)).await;
        let (_, send, ..) = tools(&fx);
        start_quiet(&fx, "cat").await;
        idles_within(&fx, 1, Duration::from_secs(3)).await;
        let mut args = send_args("j1");
        args.text = Some("x".into());
        Tool::call(&send, args, cx(&fx)).await.unwrap();
        let v = idles_within(&fx, 2, Duration::from_secs(4)).await;
        assert_eq!(v.len(), 2, "{v:?}");
        // Base interval, not the doubled one: quiet is ≈ 1, and nudge restarts.
        assert_eq!(v[1].0, 1, "{v:?}");
        assert!(v[1].1 <= 1, "{v:?}");
        cancel(&fx, "j1").await;
    }

    /// 61 §5.4 (negative control): a screen that keeps changing faster than
    /// the interval is never idle.
    #[tokio::test]
    async fn pty_idle_changing_screen_never_fires() {
        let fx = fixture(Duration::from_millis(100), Duration::from_secs(60)).await;
        start_quiet(&fx, "i=0; while :; do i=$((i+1)); echo $i; sleep 0.4; done").await;
        tokio::time::sleep(3 * IDLE).await;
        assert!(idles(&fx).is_empty(), "{:?}", idles(&fx));
        cancel(&fx, "j1").await;
    }

    /// 61 §5.4: consecutive unanswered nudges double the quiet interval.
    #[tokio::test]
    async fn pty_idle_backoff_doubles_without_interaction() {
        let fx = fixture(Duration::from_millis(100), Duration::from_secs(60)).await;
        start_quiet(&fx, "for i in 1 2 3; do echo $i; sleep 3; done; sleep 60").await;
        let v = idles_within(&fx, 3, Duration::from_secs(14)).await;
        let nudges: Vec<u32> = v.iter().map(|i| i.0).collect();
        let quiet: Vec<u64> = v.iter().map(|i| i.1).collect();
        assert_eq!(nudges, [1, 2, 3], "{v:?}");
        assert!(
            (1..=2).contains(&quiet[0])
                && (2..=3).contains(&quiet[1])
                && (4..=5).contains(&quiet[2]),
            "{quiet:?}"
        );
        cancel(&fx, "j1").await;
    }

    /// 61 §5.5: an outstanding `wait` suppresses the nudge; its return re-arms.
    #[tokio::test]
    async fn pty_idle_suppressed_while_waiting() {
        let fx = fixture(Duration::from_millis(100), Duration::from_secs(60)).await;
        start_quiet(&fx, "sleep 30").await;
        let waiting = tokio::spawn({
            let jobs = fx.jobs.clone();
            let session = fx.session;
            async move {
                jobs.ask(Wait {
                    session,
                    call_id: String::new(),
                    args: WaitArgs {
                        id: "j1".into(),
                        pattern: Some("never".into()),
                        timeout: Some(3),
                    },
                    activity: None,
                })
                .await
            }
        });
        tokio::time::sleep(Duration::from_millis(2500)).await;
        assert!(idles(&fx).is_empty(), "{:?}", idles(&fx));
        let out = waiting.await.unwrap().unwrap();
        assert!(
            out.content.contains("timed out after 3s"),
            "{}",
            out.content
        );
        let v = idles_within(&fx, 1, Duration::from_secs(3)).await;
        assert_eq!(v[0].0, 1, "{v:?}");
        cancel(&fx, "j1").await;
    }

    /// 155 §6.3 (live proof 3 in miniature): a `sleep` in the foreground of
    /// an interactive shell is a busy tree, so the unchanged prompt line does
    /// not fire; once it ends the quiet fire carries the tree reason.
    #[cfg(unix)]
    #[tokio::test]
    async fn pty_idle_waits_for_a_busy_tree() {
        let fx = fixture(Duration::from_millis(100), Duration::from_secs(60)).await;
        let (_, send, ..) = tools(&fx);
        start_quiet(&fx, "bash --norc -i").await;
        let mut args = send_args("j1");
        args.text = Some("sleep 4".into());
        Tool::call(&send, args, cx(&fx)).await.unwrap();
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert!(idles(&fx).is_empty(), "{:?}", idles(&fx));
        let v = idles_within(&fx, 1, Duration::from_secs(4)).await;
        assert_eq!(v[0].0, 1, "{v:?}");
        assert!(
            whys(&fx)[0].ends_with("process tree quiet"),
            "{:?}",
            whys(&fx)
        );
        cancel(&fx, "j1").await;
    }

    /// 61 §5.6: `idle_secs: 0` is never; and the tool's clamp (61 §1.6).
    #[tokio::test]
    async fn pty_idle_off() {
        let fx = fixture(Duration::from_millis(100), Duration::from_secs(60)).await;
        let start = start_idle(&fx, IDLE);
        assert_eq!(start.idle_after(Some(0)), None);
        assert_eq!(start.idle_after(Some(3)), Some(Duration::from_secs(10)));
        assert_eq!(start.idle_after(Some(45)), Some(Duration::from_secs(45)));
        assert_eq!(start.idle_after(None), Some(IDLE));
        assert_eq!(start_idle(&fx, Duration::ZERO).idle_after(None), None);

        let mut args = start_args("echo hi; sleep 30");
        args.idle_secs = Some(0);
        Tool::call(&start, args, cx(&fx)).await.unwrap();
        tokio::time::sleep(3 * IDLE + Duration::from_millis(500)).await;
        assert!(idles(&fx).is_empty(), "{:?}", idles(&fx));
        cancel(&fx, "j1").await;
    }

    /// 61 §5.7: the same rows redrawn with a moving cursor are still quiet.
    #[tokio::test]
    async fn pty_idle_ignores_cursor_only_redraws() {
        let fx = fixture(Duration::from_millis(100), Duration::from_secs(60)).await;
        start_quiet(
            &fx,
            r"while :; do printf '\rsame'; sleep 0.1; printf '\r'; sleep 0.1; done",
        )
        .await;
        let v = idles_within(&fx, 1, Duration::from_secs(3)).await;
        assert!(v[0].3.starts_with("same\n"), "{v:?}");
        cancel(&fx, "j1").await;
    }

    /// 109 §5.1 / §5.3: a PTY job writes its row at spawn, a screen row as
    /// `screen`/`send` return (revision-gated: an unchanged screen is not
    /// rewritten), the final screen and the settled row at exit.
    #[tokio::test]
    async fn screen_snapshot_on_screen_and_timer() {
        use crate::jobs::tests::{journaled, rows_until};
        let (fx, journal) = journaled(Duration::from_millis(100), Duration::from_secs(60)).await;
        let (start, send, screen, ..) = tools(&fx);
        let out = Tool::call(&start, start_args("cat"), cx(&fx))
            .await
            .unwrap();
        assert!(
            out.content.starts_with("job j1 · running"),
            "{}",
            out.content
        );
        let rows = lock(&journal.jobs).clone();
        assert_eq!(rows[0].0.kind, JobKind::Pty);
        assert_eq!(rows[0].0.state, JobState::Running);
        assert!(rows[0].0.pgid.is_some());
        // `send` typed a line: the echoed screen is new → one row with it.
        let mut args = send_args("j1");
        args.text = Some("hello snapshot".into());
        Tool::call(&send, args, cx(&fx)).await.unwrap();
        let n = lock(&journal.screens).len();
        let last = lock(&journal.screens).last().cloned().unwrap();
        assert!(last.text.contains("hello snapshot"), "{:?}", last.text);
        assert_eq!((last.rows, last.cols), (ROWS, COLS));
        assert_eq!(last.job_id.0, "j1");
        // `screen` with nothing new: the revision gate skips the write.
        Tool::call(&screen, ScreenArgs { id: "j1".into() }, cx(&fx))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(lock(&journal.screens).len(), n);
        // Exit: the final screen and the settled row.
        let mut args = send_args("j1");
        args.keys = Some(vec![Key::Ctrl('d')]);
        Tool::call(&send, args, cx(&fx)).await.unwrap();
        let rows = rows_until(&journal, Duration::from_secs(5), |r| {
            r.iter().any(|(j, _)| j.state == JobState::Settled)
        })
        .await;
        let settled = &rows.last().unwrap().0;
        assert_eq!(settled.exit, Some(JobExit::Exited(0)));
        let screens = lock(&journal.screens).clone();
        assert!(screens.len() > n, "no exit snapshot");
        assert!(screens.last().unwrap().revision > last.revision);
    }

    /// 109 §5.3: after a restart `screen`/`send` on a lost PTY serve the
    /// stored page under the banner; `pty_screen` serves it as a page with
    /// `exit = lost` and no epoch; `wait` says the job is lost.
    #[tokio::test]
    async fn screen_after_restart_serves_banner() {
        use crate::jobs::{LostJob, Reaped};
        let fx = fixture(Duration::ZERO, Duration::from_secs(60)).await;
        let (_, send, screen, wait) = tools(&fx);
        let restarted_at = chrono::Utc::now();
        let row = JobRow {
            session: fx.session,
            id: JobId("j2".into()),
            kind: JobKind::Pty,
            call_id: "c2".into(),
            command: "python3 -q".into(),
            name: Some("py".into()),
            pgid: None,
            log: Some(fx.dir.join("j2.log")),
            started_ms: 0,
            state: JobState::Lost,
            exit: Some(JobExit::Lost),
            settled_ms: Some(41_000),
            engine: String::new(),
        };
        let stored = PtyScreenRow {
            session: fx.session,
            job_id: JobId("j2".into()),
            revision: 418,
            rows: 40,
            cols: 120,
            cursor_row: 1,
            cursor_col: 4,
            text: ">>> 1+1\n2\n>>>".into(),
            at_ms: 1_700_000_000_000,
        };
        fx.jobs
            .tell(Reaped {
                restarted_at,
                jobs: vec![LostJob {
                    row,
                    screen: Some(stored.clone()),
                    tail: String::new(),
                    deliver: Some("gone".into()),
                    reason: crate::jobs::LostReason::Restart,
                }],
            })
            .await
            .unwrap();
        let out = Tool::call(&screen, ScreenArgs { id: "py".into() }, cx(&fx))
            .await
            .unwrap();
        let at = hhmmss(&restarted_at);
        let captured = hhmmss(&chrono::DateTime::from_timestamp_millis(stored.at_ms).unwrap());
        assert_eq!(
            out.content,
            format!(
                "job j2 (py) · lost · 41s\n[process gone — the engine restarted at {at}; this is the last screen captured at {captured} (revision 418)]\nscreen:\n>>> 1+1\n2\n>>>\ncursor: row 2 col 5"
            )
        );
        assert!(!out.is_error);
        assert_eq!(out.structured.as_ref().unwrap()["lost"], true);
        let out = Tool::call(&send, send_args("j2"), cx(&fx)).await.unwrap();
        assert!(
            out.content.contains(&format!(
                "the terminal is gone (engine restarted at {at}); last screen follows\n[process gone"
            )),
            "{}",
            out.content
        );
        let out = Tool::call(
            &wait,
            WaitArgs {
                id: "j2".into(),
                pattern: None,
                timeout: Some(1),
            },
            cx(&fx),
        )
        .await
        .unwrap();
        assert!(out.is_error);
        assert_eq!(out.content, "j2 (py) lost — start it again");
        // The viewer's page.
        let page = fx
            .jobs
            .ask(PtyScreen {
                session: fx.session,
                id: JobId("j2".into()),
                viewport: PtyViewport::Live,
                if_revision: None,
            })
            .await
            .unwrap();
        let PtyScreenReply::Updated(page) = page else {
            panic!("{page:?}");
        };
        assert_eq!(
            (page.revision, page.epoch, page.exit),
            (418, None, Some(JobExit::Lost))
        );
        assert_eq!(page.cursor, Some((1, 4)));
        assert_eq!(page.lines.len(), 3);
        assert_eq!(text(&page.lines[0]), ">>> 1+1");
        let same = fx
            .jobs
            .ask(PtyScreen {
                session: fx.session,
                id: JobId("j2".into()),
                viewport: PtyViewport::Live,
                if_revision: Some(418),
            })
            .await
            .unwrap();
        assert!(matches!(
            same,
            PtyScreenReply::Unchanged { revision: 418, .. }
        ));
        let views = list(&fx).await;
        assert_eq!(views[0].state, "lost");
        assert_eq!(views[0].tail, ">>> 1+1\n2\n>>>");
        assert!(views[0].pty);
    }

    /// 139 §7.2: a PTY lost to a transfer serves the replicated page under
    /// the moved-from banner, in `screen` and in `send`'s refusal.
    #[tokio::test]
    async fn screen_after_transfer_names_the_previous_owner() {
        use crate::jobs::{LostJob, LostReason, Reaped};
        let fx = fixture(Duration::ZERO, Duration::from_secs(60)).await;
        let (_, send, screen, _) = tools(&fx);
        let restarted_at = chrono::Utc::now();
        let row = JobRow {
            session: fx.session,
            id: JobId("j3".into()),
            kind: JobKind::Pty,
            call_id: "c3".into(),
            command: "bash -l".into(),
            name: None,
            pgid: None,
            log: None,
            started_ms: 0,
            state: JobState::Lost,
            exit: Some(JobExit::Lost),
            settled_ms: Some(7_000),
            engine: "a".into(),
        };
        let stored = PtyScreenRow {
            session: fx.session,
            job_id: JobId("j3".into()),
            revision: 9,
            rows: 40,
            cols: 120,
            cursor_row: 0,
            cursor_col: 2,
            text: "$ ".into(),
            at_ms: 1_700_000_000_000,
        };
        fx.jobs
            .tell(Reaped {
                restarted_at,
                jobs: vec![LostJob {
                    row,
                    screen: Some(stored.clone()),
                    tail: String::new(),
                    deliver: None,
                    reason: LostReason::OwnerChanged { engine: "a".into() },
                }],
            })
            .await
            .unwrap();
        let out = Tool::call(&screen, ScreenArgs { id: "j3".into() }, cx(&fx))
            .await
            .unwrap();
        let at = hhmmss(&restarted_at);
        let captured = hhmmss(&chrono::DateTime::from_timestamp_millis(stored.at_ms).unwrap());
        assert_eq!(
            out.content,
            format!(
                "job j3 · lost · 7s\n[process gone — the session moved from a at {at}; this is the last screen captured on a at {captured} (revision 9)]\nscreen:\n$ \ncursor: row 1 col 3"
            )
        );
        let out = Tool::call(&send, send_args("j3"), cx(&fx)).await.unwrap();
        assert!(
            out.content.contains(&format!(
                "the terminal is gone (session moved from a at {at}); last screen follows\n[process gone"
            )),
            "{}",
            out.content
        );
    }

    /// A pipe job through the real actor, as `shell` sends it.
    async fn pipe_job(fx: &Fx, command: &str, timeout: Option<Duration>) {
        fx.jobs
            .ask(crate::jobs::Start {
                session: fx.session,
                command: command.into(),
                call_id: String::new(),
                quiet: false,
                shell: crate::shell::ShellTool::build_shell(false).await.unwrap(),
                put_back: Arc::new(|_| {}),
                root: fx.dir.clone(),
                activity: None,
                timeout,
            })
            .await
            .unwrap();
    }

    /// Polls until every row of the session has left `running`.
    async fn all_settled(fx: &Fx, within: Duration) {
        let deadline = Instant::now() + within;
        loop {
            let rows = list(fx).await;
            if rows.iter().all(|r| r.state != "running") {
                return;
            }
            assert!(Instant::now() < deadline, "still running: {rows:?}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Polls the probe until no finished row holds a descriptor; fails with
    /// the holders at `within`.
    async fn nothing_held(fx: &Fx, within: Duration) {
        let deadline = Instant::now() + within;
        loop {
            let held = fx
                .jobs
                .ask(crate::jobs::OpenHandles(fx.session))
                .await
                .unwrap();
            if held.is_empty() {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "finished jobs still hold descriptors: {held:?}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// D1–D3: once a job has exited (normally, by timeout, by cancel) its
    /// retained row keeps the tail but no log file, master, reader or writer.
    #[tokio::test]
    async fn finished_jobs_hold_no_descriptors() {
        let fx = fixture(Duration::from_millis(100), Duration::from_secs(60)).await;
        let (start, ..) = tools(&fx);
        for i in 0..4 {
            pipe_job(&fx, &format!("echo pipe {i}"), None).await;
        }
        pipe_job(&fx, "sleep 30", Some(Duration::from_secs(1))).await; // j5: timeout
        pipe_job(&fx, "sleep 30", None).await; // j6: cancelled
        cancel(&fx, "j6").await;
        for i in 0..3 {
            Tool::call(&start, start_args(&format!("echo pty {i}")), cx(&fx))
                .await
                .unwrap();
        }
        Tool::call(&start, start_args("sleep 30"), cx(&fx))
            .await
            .unwrap(); // j10: cancelled
        cancel(&fx, "j10").await;
        all_settled(&fx, Duration::from_secs(10)).await;
        let rows = list(&fx).await;
        assert_eq!(rows.len(), 10, "{rows:?}");
        assert_eq!(rows[4].state, "timeout");
        assert_eq!(rows[5].state, "killed");
        assert_eq!(rows[9].state, "killed");
        assert_eq!(rows[0].tail, "pipe 0\n");
        assert!(rows[6].tail.starts_with("pty 0"), "{:?}", rows[6].tail);
        nothing_held(&fx, Duration::from_secs(5)).await;
        // D4: the logs on disk stay.
        assert_eq!(std::fs::read_to_string(&rows[3].log).unwrap(), "pipe 3\n");
        assert!(
            std::fs::read_to_string(&rows[8].log)
                .unwrap()
                .contains("pty 2")
        );
    }

    /// D3: rows a drain leaves `lost` (`KillSession`) hold nothing either.
    #[tokio::test]
    async fn lost_jobs_hold_no_descriptors() {
        let fx = fixture(Duration::from_millis(100), Duration::from_secs(60)).await;
        let (start, ..) = tools(&fx);
        pipe_job(&fx, "sleep 30", None).await;
        Tool::call(&start, start_args("sleep 30"), cx(&fx))
            .await
            .unwrap();
        fx.jobs
            .ask(crate::jobs::KillSession {
                session: fx.session,
            })
            .await
            .unwrap();
        let rows = list(&fx).await;
        assert_eq!(
            rows.iter().map(|r| r.state.as_str()).collect::<Vec<_>>(),
            ["lost", "lost"]
        );
        nothing_held(&fx, Duration::from_secs(5)).await;
    }

    // ---- 171 user cells ----

    /// A kernel-shaped `StartPty` (171 §5.2): `/bin/sh argv…` named `cell:test`.
    fn cell_start(
        fx: &Fx,
        argv: &[&str],
        env: Option<BTreeMap<String, String>>,
        tx: mpsc::Sender<Mark>,
    ) -> StartPty {
        let mut args = start_args("");
        args.name = Some("cell:test".into());
        args.env = env;
        StartPty {
            session: fx.session,
            call_id: String::new(),
            args,
            cwd: fx.dir.clone(),
            ready_timeout: Duration::from_secs(30),
            idle_after: Some(Duration::from_secs(10)),
            root: fx.dir.clone(),
            activity: None,
            pty_shell: None,
            program: Some((
                "/bin/sh".into(),
                argv.iter().map(|s| (*s).to_owned()).collect(),
            )),
            cell: Some(tx),
        }
    }

    /// The handler's `Err` text of an `ask` whose reply is a `Result<_, String>`.
    fn refused<T: std::fmt::Debug, M>(r: Result<T, kameo::error::SendError<M, String>>) -> String {
        match r {
            Err(kameo::error::SendError::HandlerError(e)) => e,
            Ok(v) => panic!("expected a refusal, got {v:?}"),
            Err(e) => panic!("jobs actor: {e}"),
        }
    }

    async fn cell_close(fx: &Fx) {
        fx.jobs
            .ask(CellKill {
                session: fx.session,
                id: JobId("j1".into()),
                close: true,
            })
            .await
            .unwrap();
        let (exit, _, inline) = finished(fx).await;
        assert_eq!(exit, JobExit::Killed);
        assert!(inline, "a kernel's Finished never rides the guard");
    }

    #[tokio::test]
    async fn cell_start_rejects_reserved_name() {
        let fx = fixture(Duration::from_secs(3), Duration::from_secs(60)).await;
        let (start, ..) = tools(&fx);
        let mut args = start_args("sleep 30");
        args.name = Some("cell:x".into());
        let out = Tool::call(&start, args, cx(&fx)).await.unwrap();
        assert!(out.is_error);
        assert_eq!(
            out.content,
            "names starting with \"cell:\" are reserved for user cells"
        );
        // The converse: a kernel outside the namespace is a programming error.
        let (tx, _rx) = mpsc::channel(4);
        let mut m = cell_start(&fx, &["-c", "sleep 30"], None, tx);
        m.args.name = Some("plain".into());
        let out = fx.jobs.ask(m).await.unwrap();
        assert!(out.is_error, "{}", out.content);
        assert!(list(&fx).await.is_empty());
    }

    #[tokio::test]
    async fn cell_jobs_omitted_from_jobs_list() {
        let fx = fixture(Duration::from_secs(3), Duration::from_secs(60)).await;
        let (tx, _rx) = mpsc::channel(4);
        let out = fx
            .jobs
            .ask(cell_start(&fx, &["-c", "sleep 30"], None, tx))
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.content);
        let s = out.structured.as_ref().unwrap();
        assert_eq!(s["job"], "j1");
        assert!(s["pid"].is_number() && s["log"].is_string(), "{s}");
        assert!(list(&fx).await.is_empty(), "jobs() must omit kernels");
        let terminals = fx
            .jobs
            .ask(ListTerminals {
                sessions: vec![fx.session],
            })
            .await
            .unwrap();
        assert!(terminals.is_empty(), "{terminals:?}");
        // A non-cell job still lists.
        let (start, ..) = tools(&fx);
        let out = Tool::call(&start, start_args("sleep 30"), cx(&fx))
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.content);
        let ids: Vec<String> = list(&fx).await.into_iter().map(|v| v.id.0).collect();
        assert_eq!(ids, ["j2"]);
        // The kernel first: `finished` reads the first `Finished` on the bus.
        cell_close(&fx).await;
        cancel_from(&fx, "j2", CancelOrigin::Shutdown).await;
    }

    #[tokio::test]
    async fn cell_send_and_cancel_refused() {
        let fx = fixture(Duration::from_secs(3), Duration::from_secs(60)).await;
        let (tx, _rx) = mpsc::channel(4);
        let out = fx
            .jobs
            .ask(cell_start(&fx, &["-c", "sleep 30"], None, tx))
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.content);
        const TEXT: &str = "this terminal belongs to a user cell — ask the user to press Run";
        for id in ["j1", "cell:test"] {
            let mut args = send_args(id);
            args.text = Some("echo hi".into());
            let r = fx
                .jobs
                .ask(SendInput {
                    session: fx.session,
                    args,
                    activity: None,
                })
                .await;
            assert_eq!(refused(r), TEXT);
        }
        for origin in [CancelOrigin::Tool, CancelOrigin::Ui] {
            let r = fx
                .jobs
                .ask(CancelJob {
                    session: fx.session,
                    id: JobId("j1".into()),
                    origin,
                })
                .await;
            assert_eq!(refused(r), TEXT);
        }
        // The `cancel_job` tool answers the same text.
        let out = Tool::call(
            &crate::jobs::CancelJobTool(fx.jobs.clone()),
            crate::jobs::CancelJobArgs { id: "j1".into() },
            cx(&fx),
        )
        .await
        .unwrap();
        assert!(out.is_error);
        assert_eq!(out.content, TEXT);
        assert!(
            lock(&fx.events)
                .iter()
                .all(|e| !matches!(e, JobEvent::Finished { .. }))
        );
        // Shutdown still kills.
        cancel_from(&fx, "j1", CancelOrigin::Shutdown).await;
        let (exit, _, inline) = finished(&fx).await;
        assert_eq!(exit, JobExit::Killed);
        assert!(inline);
    }

    /// 171F B23: `screen` and `wait` refuse a kernel (by id or name) as `send`
    /// does, while an agent PTY still answers; B15: the counts behind the
    /// session's terminal glyph skip kernels; B24: a kernel's log is
    /// `<id>.cell.log`.
    #[tokio::test]
    async fn cell_kernel_unreadable_and_uncounted() {
        let fx = fixture(Duration::from_secs(3), Duration::from_secs(60)).await;
        let (tx, _rx) = mpsc::channel(4);
        let out = fx
            .jobs
            .ask(cell_start(&fx, &["-c", "sleep 30"], None, tx))
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.content);
        let log = out.structured.as_ref().unwrap()["log"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(log.ends_with("/j1.cell.log"), "{log}");
        let (start, _, screen, wait) = tools(&fx);
        let waits = |id: &str| WaitArgs {
            id: id.into(),
            pattern: Some(String::new()),
            timeout: Some(1),
        };
        for id in ["j1", "cell:test"] {
            let out = Tool::call(&screen, ScreenArgs { id: id.into() }, cx(&fx))
                .await
                .unwrap();
            assert!(
                out.is_error && out.content == jobs::CELL_REFUSED,
                "{}",
                out.content
            );
            let out = Tool::call(&wait, waits(id), cx(&fx)).await.unwrap();
            assert!(
                out.is_error && out.content == jobs::CELL_REFUSED,
                "{}",
                out.content
            );
        }
        let counts = crate::jobs::JobCounts(vec![fx.session]);
        assert!(fx.jobs.ask(counts.clone()).await.unwrap().is_empty());
        assert!(
            fx.jobs
                .ask(crate::jobs::LiveCounts)
                .await
                .unwrap()
                .is_empty()
        );
        // An agent PTY reads and counts as before.
        let out = Tool::call(&start, start_args("sleep 30"), cx(&fx))
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.content);
        let out = Tool::call(&screen, ScreenArgs { id: "j2".into() }, cx(&fx))
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.content);
        let out = Tool::call(&wait, waits("j2"), cx(&fx)).await.unwrap();
        assert!(!out.is_error, "{}", out.content);
        let n = fx.jobs.ask(counts).await.unwrap();
        assert_eq!(n[&fx.session], (0, 1));
        let rows = fx.jobs.ask(crate::jobs::LiveCounts).await.unwrap();
        assert_eq!(rows[0].ptys, [JobId("j2".into())]);
        cell_close(&fx).await;
        cancel_from(&fx, "j2", CancelOrigin::Shutdown).await;
    }

    #[tokio::test]
    async fn cell_kill_spares_shell_group() {
        let fx = fixture(Duration::from_secs(3), Duration::from_secs(60)).await;
        let (tx, _rx) = mpsc::channel(4);
        fx.jobs
            .ask(cell_start(&fx, &["-c", "sleep 30"], None, tx))
            .await
            .unwrap();
        let id = JobId("j1".into());
        // `sh -c sleep` is one process group: the leader's own.
        let fg = fx
            .jobs
            .ask(CellForeground {
                session: fx.session,
                id: id.clone(),
            })
            .await
            .unwrap();
        assert!(fg);
        fx.jobs
            .ask(CellKill {
                session: fx.session,
                id: id.clone(),
                close: false,
            })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            lock(&fx.events)
                .iter()
                .all(|e| !matches!(e, JobEvent::Finished { .. })),
            "the kernel shell must survive Kill"
        );
        // A plain PTY job is not a kernel.
        let (start, ..) = tools(&fx);
        Tool::call(&start, start_args("sleep 30"), cx(&fx))
            .await
            .unwrap();
        let r = fx
            .jobs
            .ask(CellKill {
                session: fx.session,
                id: JobId("j2".into()),
                close: false,
            })
            .await;
        assert_eq!(refused(r), "not a cell job");
        cell_close(&fx).await;
        cancel_from(&fx, "j2", CancelOrigin::Shutdown).await;
    }

    #[tokio::test]
    async fn cell_program_honoured_and_env_cleared() {
        let fx = fixture(Duration::from_secs(3), Duration::from_secs(60)).await;
        let (tx, _rx) = mpsc::channel(4);
        let env: BTreeMap<String, String> = [("FOO".to_owned(), "bar".to_owned())].into();
        let out = fx
            .jobs
            .ask(cell_start(
                &fx,
                &["-c", r#"printf "%s" "$FOO-$HOME""#],
                Some(env),
                tx,
            ))
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.content);
        let (exit, _, inline) = finished(&fx).await;
        assert_eq!(exit, JobExit::Exited(0));
        assert!(inline);
        let log = fx
            .dir
            .join("jobs")
            .join(fx.session.to_string())
            .join("j1.cell.log");
        let log = std::fs::read_to_string(&log).unwrap();
        assert!(log.contains("bar-"), "{log:?}");
        assert!(
            !log.contains("bar-/"),
            "HOME must not be inherited: {log:?}"
        );
    }

    #[tokio::test]
    async fn cell_marks_reach_channel() {
        let fx = fixture(Duration::from_secs(3), Duration::from_secs(60)).await;
        let (tx, mut rx) = mpsc::channel(8);
        fx.jobs
            .ask(cell_start(
                &fx,
                &["-c", r#"printf '\033]133;C\007hi\033]133;D;3\007'"#],
                None,
                tx,
            ))
            .await
            .unwrap();
        let (exit, ..) = finished(&fx).await;
        assert_eq!(exit, JobExit::Exited(0));
        let mut marks = Vec::new();
        while let Ok(m) = rx.try_recv() {
            marks.push(m);
        }
        assert_eq!(
            marks,
            [
                Mark {
                    kind: MarkKind::Start,
                    exit: None,
                    cwd: None,
                    offset: 0
                },
                Mark {
                    kind: MarkKind::End,
                    exit: Some(3),
                    cwd: None,
                    offset: 10
                },
            ]
        );
    }

    /// Waits until the ledger saw the job's settle, then checks the supervisor got neither a
    /// result to inject nor an idle nudge (171 HANDOFF-Z-2/Z-3: a note kernel's session id
    /// has no session row, so either would open a fresh root chat and start a turn).
    async fn assert_settled_bare(
        ledger: &ActorRef<crate::activity::test_sink::Ledger>,
        seen: &Mutex<Vec<crate::activity::test_sink::Seen>>,
    ) {
        use crate::activity::test_sink::Seen;
        let v = crate::jobs::tests::ledger_until(ledger, seen, Duration::from_secs(8), |v| {
            v.iter()
                .any(|s| matches!(s, Seen::Finished(_) | Seen::Result(_)))
        })
        .await;
        assert!(
            v.iter().any(|s| matches!(s, Seen::Finished(_))),
            "kernel settles bare: {v:?}"
        );
        assert!(
            !v.iter()
                .any(|s| matches!(s, Seen::Result(_) | Seen::Idle(_))),
            "no FinishedWithResult / JobIdle for a kernel: {v:?}"
        );
    }

    /// A kernel whose process exits by itself (crash, `exit` in a cell) after sitting quiet
    /// past a 1 s idle interval: no `<job_idle>` while it ran, no `<job_result>` after.
    #[tokio::test]
    async fn cell_kernel_exit_never_reaches_the_model() {
        let (fx, ledger, seen) =
            crate::jobs::tests::certified(Duration::from_secs(3), Duration::from_secs(60)).await;
        let (tx, _rx) = mpsc::channel(8);
        let mut m = cell_start(&fx, &["-c", "printf hi; sleep 3; exit 3"], None, tx);
        m.idle_after = Some(IDLE);
        fx.jobs.ask(m).await.unwrap();
        assert_settled_bare(&ledger, &seen).await;
        let (exit, _, inline) = finished(&fx).await;
        assert_eq!(exit, JobExit::Exited(3));
        assert!(inline);
        assert!(idles(&fx).is_empty(), "{:?}", idles(&fx));
    }

    /// A kernel killed from outside (session close / `CellKill{close}`): `cancel_job`'s and a
    /// UI's `Ui` origin are refused for kernels, so `Shutdown` is the kill that remains.
    #[tokio::test]
    async fn cell_kernel_kill_never_reaches_the_model() {
        let (fx, ledger, seen) =
            crate::jobs::tests::certified(Duration::from_secs(3), Duration::from_secs(60)).await;
        let (tx, _rx) = mpsc::channel(8);
        fx.jobs
            .ask(cell_start(&fx, &["-c", "sleep 30"], None, tx))
            .await
            .unwrap();
        let ui = fx
            .jobs
            .ask(CancelJob {
                session: fx.session,
                id: JobId("j1".into()),
                origin: CancelOrigin::Ui,
            })
            .await;
        assert_eq!(refused(ui), jobs::CELL_REFUSED);
        fx.jobs
            .ask(CancelJob {
                session: fx.session,
                id: JobId("j1".into()),
                origin: CancelOrigin::Shutdown,
            })
            .await
            .unwrap();
        assert_settled_bare(&ledger, &seen).await;
        let (exit, _, inline) = finished(&fx).await;
        assert_eq!(exit, JobExit::Killed);
        assert!(inline);
    }
}
