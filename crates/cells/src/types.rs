//! The wire types of design doc 171 §15.1–15.3 (owner: C, FROZEN after the skeleton). Every
//! type derives Serialize / Deserialize / JsonSchema and `ts_rs::TS` behind `feature = "ts"`.
//! Grammar types live in [`surface::cell`], the note key and the replicated latest-run row in
//! [`document`], the PTY mark in [`tools::pty`]; all are re-exported here. Five TS names are
//! prefixed because the bare name is already a binding (`Scope` → `CellScope`, `RunId` →
//! `CellRunId`, `RunMode` → `CellRunMode`, `RunState` → `CellRunState`; `surface::cell::Cell`
//! → `CellSpec`). A new field or variant is a contract change (`// 171:<TAG>-ADD`, see the
//! ownership table).

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub use document::{CellLatest, DocId, NoteKey, Principal};
pub use runtime::SessionId;
pub use surface::cell::{
    Cell, CellOptions, Chip, ChipKind, Expect, HeaderLine, Lang, NoteDefaults, Problem,
    PythonChoice, Severity, ValidateCtx,
};
pub use tools::pty::{Mark, MarkKind, PtyChunk};

/// `#[derive]` set every wire type carries.
macro_rules! wire {
    ($(#[$m:meta])* pub struct $name:ident $(as $ts:literal)? { $($body:tt)* }) => {
        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
        #[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export $(, rename = $ts)?))]
        $(#[$m])*
        pub struct $name { $($body)* }
    };
    ($(#[$m:meta])* pub enum $name:ident $(as $ts:literal)? { $($body:tt)* }) => {
        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
        #[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export $(, rename = $ts)?))]
        $(#[$m])*
        pub enum $name { $($body)* }
    };
}

wire! {
    /// Which cell (§3.4). Chat: the assistant message `entry` and the 1-based position among
    /// its *cells*; note: the doc and the cell's `name`, else its BlockNote block id.
    #[serde(tag = "type", rename_all = "snake_case")]
    pub enum CellRef {
        /// A cell in an assistant reply.
        Chat {
            /// The session.
            session: SessionId,
            /// The assistant message (transcript entry id).
            entry: String,
            /// 1-based position among the message's cells.
            ordinal: u32,
        },
        /// A cell in a note.
        Note {
            /// The note.
            doc: DocId,
            /// Name or block id.
            key: NoteKey,
        },
    }
}

wire! {
    /// The kernel scope (§5.1): a chat session or a note.
    #[serde(tag = "type", content = "id", rename_all = "snake_case")]
    pub enum Scope as "CellScope" {
        /// Chat cells of a session.
        Session(SessionId),
        /// Cells of a note.
        Doc(DocId),
    }
}

wire! {
    /// Which shell runs a shell cell (§5.2).
    #[derive(Copy, Eq, Hash)]
    #[serde(rename_all = "snake_case")]
    pub enum ShellKind {
        /// `bash`.
        Bash,
        /// `zsh`.
        Zsh,
        /// `sh`.
        Sh,
        /// `pwsh` / `powershell`.
        Pwsh,
    }
}

wire! {
    /// The resolved program of a kernel (§5.1–5.2); each distinct path is its own kernel.
    #[serde(tag = "type", rename_all = "snake_case")]
    pub enum KernelSpec {
        /// A shell on a PTY.
        Shell {
            /// Which shell.
            kind: ShellKind,
            /// The binary.
            path: PathBuf,
        },
        /// A python interpreter running the shim.
        Python {
            /// The interpreter.
            path: PathBuf,
        },
        /// A Monty checkout.
        Monty,
    }
}

wire! {
    /// Kernel key (§5.1): `(Scope, Spec)`.
    pub struct KernelId {
        /// Session or doc.
        pub scope: Scope,
        /// The program.
        pub spec: KernelSpec,
    }
}

wire! {
    /// Kernel lifecycle (§5.3).
    #[derive(Copy, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum KernelPhase {
        /// First run arrived; waiting for the ready mark / `status ready`.
        Starting,
        /// Idle.
        Ready,
        /// A run is executing.
        Busy,
        /// The process exited or failed to spawn.
        Dead,
        /// Closed (idle timeout, Close, session close, doc delete).
        Closed,
    }
}

wire! {
    /// One live kernel as the strip shows it (§11.4).
    pub struct KernelState {
        /// Key.
        pub id: KernelId,
        /// Phase.
        pub phase: KernelPhase,
        /// Chip text, e.g. `bash 5.2 (Homebrew)`.
        pub label: String,
        /// Live cwd from OSC 7 (shells) or the shim.
        pub cwd: Option<String>,
        /// §5.4: "your shell's environment could not be read — Refresh" when the login
        /// environment capture failed.
        #[serde(default)]
        pub env_warning: Option<String>, // 171:E1-ADD (L11)
    }
}

wire! {
    /// What part of a cell a press runs (§7.1); offsets count code lines from 1, header excluded.
    #[serde(tag = "type", rename_all = "snake_case")]
    pub enum RunMode as "CellRunMode" {
        /// The whole code.
        Cell,
        /// One code line.
        Line {
            /// 1-based code line.
            line: u32,
        },
        /// A selection.
        Selection {
            /// Start offset (code text, header excluded).
            from: u32,
            /// End offset, exclusive.
            to: u32,
        },
    }
}

/// Run id: `r_` + ULID, minted by the pressing client (§5.10); a retried press is idempotent.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, rename = "CellRunId"))]
#[serde(transparent)]
pub struct RunId(pub String);

wire! {
    /// Run lifecycle (§8.4 `state`).
    #[derive(Copy, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum RunState as "CellRunState" {
        /// Waiting for the kernel.
        Queued,
        /// Between marks C and D (or the python/Monty equivalent).
        Running,
        /// Exit 0 and every expect met.
        Done,
        /// Non-zero exit or a missed expect.
        Failed,
        /// Stop.
        Stopped,
        /// `timeout:` elapsed.
        Timeout,
        /// Kill.
        Killed,
        /// The engine stopped while it ran (§5.3).
        Lost,
        /// Removed from the queue before it ran.
        Cancelled,
    }
}

wire! {
    /// How a run ended.
    #[serde(tag = "type", content = "value", rename_all = "snake_case")]
    pub enum ExitStatus {
        /// Exit code.
        Code(i32),
        /// Signal name.
        Signal(String),
    }
}

wire! {
    /// One `expect` item, met or missed (§7.5).
    pub struct ExpectResult {
        /// The item.
        pub item: Expect,
        /// Found in the cleaned output.
        pub met: bool,
    }
}

wire! {
    /// The run record (§8.4) as clients see it.
    pub struct CellRun {
        /// Run id.
        pub id: RunId,
        /// Which cell.
        pub cell: CellRef,
        /// What part ran.
        pub mode: RunMode,
        /// Run-all pass id.
        pub pass: Option<String>,
        /// The kernel it ran on.
        pub kernel: KernelId,
        /// `bash 5.2 (Homebrew) /opt/homebrew/bin/bash`.
        pub interpreter: String,
        /// Resolved cwd.
        pub cwd: String,
        /// `env` names, never values.
        pub env_names: Vec<String>,
        /// Digest the user saw.
        pub presented_sha: String,
        /// Digest of what ran.
        pub code_sha: String,
        /// The user edited the code before the press (F4).
        pub edited: bool,
        /// Device id + `desktop|web|phone`.
        pub pressed_by: String,
        /// Lifecycle.
        pub state: RunState,
        /// How it ended.
        pub exit: Option<ExitStatus>,
        /// Duration.
        pub secs: Option<f64>,
        /// Per-item expect results.
        pub expect: Vec<ExpectResult>,
        /// The summary (§7.5).
        pub summary: Option<String>,
        /// Size of the cleaned output.
        #[cfg_attr(feature = "ts", ts(type = "number | null"))]
        pub output_bytes: Option<u64>,
        /// Enqueued, ms since the epoch.
        #[cfg_attr(feature = "ts", ts(type = "number"))]
        pub queued_ms: i64,
        /// Started.
        #[cfg_attr(feature = "ts", ts(type = "number | null"))]
        pub started_ms: Option<i64>,
        /// Ended.
        #[cfg_attr(feature = "ts", ts(type = "number | null"))]
        pub ended_ms: Option<i64>,
        /// How many values the agent's copy masked (§9.4: vault values + credential patterns).
        #[serde(default)]
        pub masked: u32, // 171:E1-ADD (L11)
        /// The cell's `#| private` at press (§9.1).
        #[serde(default)]
        pub private: bool, // 171:E1-ADD (L11)
        /// The cell's `#| name` at press (chat cells too).
        #[serde(default)]
        pub name: Option<String>, // 171:E1-ADD (L11)
        /// Set while the run's kernel waits on `input()`; cleared once `cell_input` answers.
        #[serde(default)]
        pub input: Option<InputPrompt>, // 171:E1-ADD (L11)
    }
}

wire! {
    /// A run waiting on `input()` / `getpass()` (§7.4); answered through `cell_input`.
    pub struct InputPrompt { // 171:E1-ADD (L11)
        /// Prompt text.
        pub prompt: String,
        /// Mask the answer; it is never stored.
        pub password: bool,
    }
}

wire! {
    /// One nbformat v4 output (§8.1); `output_type` is the tag, as in nbformat.
    #[serde(tag = "output_type", rename_all = "snake_case")]
    pub enum CellOutput {
        /// stdout / stderr text (shells: one, the static render, ANSI kept).
        Stream {
            /// `stdout` | `stderr`.
            name: String,
            /// Text.
            text: String,
        },
        /// `display()` / a matplotlib figure.
        DisplayData {
            /// MIME bundle.
            data: BTreeMap<String, serde_json::Value>,
            /// Metadata.
            metadata: serde_json::Value,
        },
        /// The last expression.
        ExecuteResult {
            /// MIME bundle.
            data: BTreeMap<String, serde_json::Value>,
            /// Metadata.
            metadata: serde_json::Value,
            /// Execution count (nbformat's `execution_count`).
            #[serde(rename = "execution_count")]
            count: u32,
        },
        /// An uncaught exception.
        Error {
            /// Exception name.
            ename: String,
            /// Exception value.
            evalue: String,
            /// Trimmed traceback.
            traceback: Vec<String>,
        },
    }
}

wire! {
    /// An armed `after` gate (§7.6).
    pub struct Armed {
        /// The gating run.
        pub by: RunId,
        /// When it ended.
        #[cfg_attr(feature = "ts", ts(type = "number"))]
        pub at_ms: i64,
        /// Its summary, shown on the armed line.
        pub summary: String,
    }
}

wire! {
    /// What the engine knows about one presented cell (§15.2 `cell_state`). Problems and chips
    /// come from [`surface::cell`]; `armed`/`superseded_by`/`stale`/`member_edit` from the engine.
    pub struct CellState {
        /// Which cell.
        pub cell: CellRef,
        /// §3.4 digest of the source the engine read.
        pub digest: String,
        /// Validation (§3.6).
        pub problems: Vec<Problem>,
        /// Lint (§3.7).
        pub chips: Vec<Chip>,
        /// `after` satisfied.
        pub armed: Option<Armed>,
        /// A later reply re-presented the name (§3.5).
        pub superseded_by: Option<CellRef>,
        /// Latest run.
        pub latest: Option<CellRun>,
        /// The note text changed since the latest run.
        pub stale: bool,
        /// Member pub8 when a member wrote the block since the owner's last run (§13.1).
        pub member_edit: Option<String>,
        /// What the next press would use.
        pub interpreter: Option<Interpreter>,
        /// §14.2: the engine that runs this cell when it is not this one (the host chip);
        /// `None` = this engine. Filled by the RPC layer.
        #[serde(default)]
        pub host: Option<String>, // 171:E1-ADD (L11)
    }
}

wire! {
    /// Interpreter family (§6.4).
    #[derive(Copy, Eq, Hash)]
    #[serde(rename_all = "snake_case")]
    pub enum InterpreterKind {
        /// CPython.
        Python,
        /// The built-in Monty.
        Monty,
        /// bash.
        Bash,
        /// zsh.
        Zsh,
        /// sh.
        Sh,
        /// PowerShell.
        Pwsh,
        /// Node (wave 3).
        Node,
        /// Deno (wave 3).
        Deno,
        /// Bun (wave 3).
        Bun,
    }
}

wire! {
    /// One candidate of `interpreters(cwd)` (§6.4).
    pub struct Interpreter {
        /// Family.
        pub kind: InterpreterKind,
        /// `.venv` | `uv:costs` | `pyenv:3.12.4` | `conda:base` | `system` | `Monty` | `homebrew` | `macOS`.
        pub source: String,
        /// Binary; `None` for Monty.
        pub path: Option<PathBuf>,
        /// `3.12.4`, `5.2.37`.
        pub version: Option<String>,
        /// Why it is listed / picked.
        pub why: String,
        /// What `auto` (python) or the default (shells) picks for this cwd.
        pub auto: bool,
    }
}

wire! {
    /// `cell_state` reply.
    pub struct StateReply {
        /// One per requested cell.
        pub cells: Vec<CellState>,
        /// The scope's live kernels.
        pub kernels: Vec<KernelState>,
    }
}

wire! {
    /// `cell_run_get` reply (text capped).
    pub struct RunGet {
        /// The record.
        pub run: CellRun,
        /// nbformat outputs.
        pub outputs: Vec<CellOutput>,
    }
}

wire! {
    /// `cell_output` reply: a page of the cleaned (or ANSI) text.
    pub struct OutputPage {
        /// The page.
        pub text: String,
        /// Byte offset of `text`.
        #[cfg_attr(feature = "ts", ts(type = "number"))]
        pub offset: u64,
        /// Total bytes.
        #[cfg_attr(feature = "ts", ts(type = "number"))]
        pub total: u64,
        /// Offset of the next page; `None` at the end.
        #[cfg_attr(feature = "ts", ts(type = "number | null"))]
        pub next: Option<u64>,
    }
}

wire! {
    /// Bus event (§15.3), fanned out by the RPC bridge. Hints only; clients re-read (§8.7).
    #[serde(tag = "type", rename_all = "snake_case")]
    pub enum CellEvent {
        /// A run was enqueued.
        RunQueued {
            /// Which.
            run: RunId,
            /// Its scope (event filtering).
            scope: Scope,
        },
        /// A run started executing.
        RunStarted {
            /// Which.
            run: RunId,
            /// Its scope.
            scope: Scope,
        },
        /// A run's record is final.
        RunFinished {
            /// Which.
            run: RunId,
            /// Its scope.
            scope: Scope,
        },
        /// A kernel changed phase or cwd.
        KernelChanged {
            /// Which.
            kernel: KernelId,
        },
        /// Validation, armed or supersede state recomputed for a scope.
        CellsChanged {
            /// Which scope.
            scope: Scope,
        },
    }
}

/// `cell_run` parameters plus what the RPC layer knows about the press (internal: RPC → actor).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunRequest {
    /// Which cell.
    pub cell: CellRef,
    /// Digest the client presented (invariant 5).
    pub seen: String,
    /// What part.
    pub mode: RunMode,
    /// Client-minted id.
    pub run: RunId,
    /// The user's edited code (F4).
    pub edited: Option<String>,
    /// Member pub8 acknowledged (invariant 6).
    pub member_ack: Option<String>,
    /// Device id + client kind, filled by the RPC layer.
    pub pressed_by: String,
}

/// `cell_run_all` parameters plus the pressing device (internal: RPC → actor).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunAllRequest {
    /// Session or doc.
    pub scope: Scope,
    /// Cells in order with the digest the client saw.
    pub cells: Vec<(CellRef, String)>,
    /// Client-minted pass id.
    pub pass: String,
    /// Device id + client kind, filled by the RPC layer.
    pub pressed_by: String,
}

impl RunId {
    /// The id text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RunId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl Scope {
    /// The `cell_runs.scope` column text: `session:<uuid>` | `doc:<id>` (§8.4).
    pub fn key(&self) -> String {
        match self {
            Scope::Session(s) => format!("session:{}", s.0),
            Scope::Doc(d) => format!("doc:{}", d.0),
        }
    }
}

impl CellRef {
    /// The scope this cell's kernels live in (§5.1).
    pub fn scope(&self) -> Scope {
        match self {
            CellRef::Chat { session, .. } => Scope::Session(*session),
            CellRef::Note { doc, .. } => Scope::Doc(doc.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_shapes_are_stable() {
        let s = SessionId(uuid::Uuid::nil());
        let r = CellRef::Chat {
            session: s,
            entry: "e1".into(),
            ordinal: 2,
        };
        let j = serde_json::to_value(&r).unwrap();
        assert_eq!(j["type"], "chat");
        assert_eq!(serde_json::from_value::<CellRef>(j).unwrap(), r);
        assert_eq!(
            serde_json::to_value(RunId("r_1".into())).unwrap(),
            serde_json::json!("r_1")
        );
        assert_eq!(
            serde_json::to_value(Scope::Session(s)).unwrap(),
            serde_json::json!({"type": "session", "id": s.0.to_string()})
        );
        assert_eq!(Scope::Doc(DocId("d".into())).key(), "doc:d");
        let o = CellOutput::ExecuteResult {
            data: BTreeMap::new(),
            metadata: serde_json::Value::Null,
            count: 3,
        };
        let j = serde_json::to_value(&o).unwrap();
        assert_eq!(j["output_type"], "execute_result");
        assert_eq!(j["execution_count"], 3);
    }
}
