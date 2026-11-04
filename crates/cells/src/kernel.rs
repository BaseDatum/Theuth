//! The kernel contract (owner: C, FROZEN; design 171 §5, §7, §8). E1 implements it for shells
//! (a `cell` PTY job + OSC marks, `shell.rs`), E2 for python (the NDJSON shim, `python.rs`) and
//! Monty (a pool checkout, `monty.rs`). The actor (E1) owns queueing (FIFO per kernel, §5.10),
//! the run store and the CWD rule's *inputs*; a kernel only executes what it is handed and
//! reports through the run's [`KernelSink`]. Changing anything here is a contract change.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use kameo::actor::ActorRef;
use tokio::sync::mpsc;

use crate::config::CellsConfig;
use crate::error::{CellsError, Result};
use crate::types::{
    CellOutput, CellRef, ExitStatus, KernelId, KernelPhase, KernelSpec, Mark, RunId, RunMode,
    SessionId,
};

/// One run handed to a kernel (§5.6 step 2). Everything is resolved by the actor: the kernel
/// writes no policy of its own.
#[derive(Clone, Debug)]
pub struct ExecuteRequest {
    /// The run.
    pub run: RunId,
    /// Exactly the text to run: the whole code, the line or the selection (header excluded;
    /// the user's edit when edited). Placeholders unexpanded — they only ever appear in `env`.
    pub code: String,
    /// What part of the cell this is.
    pub mode: RunMode,
    /// Present when [`needs_prologue`] says so: `cd -- <cwd>` (shell prologue / `os.chdir` /
    /// Monty's `/cwd` mount).
    pub cwd: Option<PathBuf>,
    /// `env` pairs with `{{secret:NAME}}` already expanded IN MEMORY (invariant 9: shells write
    /// them only into the 0600 run file's prologue, deleted at mark D). Empty when no prologue.
    pub env: Vec<(String, String)>,
    /// `timeout:` — the actor sends Stop when it elapses and Kill `kill_after_secs` later; a
    /// kernel may also enforce its own hard limit (Monty's 60 s).
    pub timeout: Option<Duration>,
}

/// What a kernel reports for the run in flight, in order. Shells send `Bytes` + `Mark`s;
/// python/Monty send `Output`s and `InputRequest`s. Every run ends with exactly one `Finished`,
/// sent only after its last output (the record-then-read order, §8.7).
#[derive(Clone, Debug, PartialEq)]
pub enum KernelEvent {
    /// The run began executing (shell: mark C; python: `status busy` with the id).
    Started {
        /// Shell: byte offset of mark C in the job log (`[C, D)` is the output); else `None`.
        log_from: Option<u64>,
    },
    /// Raw terminal bytes after mark C (shells; live view only — the record is rendered from
    /// the log range at D, §8.3).
    Bytes(Vec<u8>),
    /// An OSC mark as scanned by `tools::pty` (shells): 133;A/C/D and OSC 7.
    Mark(Mark),
    /// One structured nbformat output (python/Monty).
    Output(CellOutput),
    /// `input()` / `getpass()` (python, §7.4); the answer arrives through [`Kernel::input`].
    InputRequest {
        /// Prompt text.
        prompt: String,
        /// Mask it; never store the answer.
        password: bool,
    },
    /// The run ended.
    Finished {
        /// Exit code / signal; `None` when the kernel cannot tell (Monty ok = `Code(0)`).
        exit: Option<ExitStatus>,
        /// Shell: byte offset of mark D; else `None`.
        log_to: Option<u64>,
        /// The kernel process itself exited during the run (§5.3 → `dead`).
        kernel_died: bool,
    },
}

/// Per-run event channel from a kernel to the actor.
pub type KernelSink = mpsc::Sender<KernelEvent>;

/// `~/.theuth/cells/` and everything under it (§15.5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellsPaths {
    /// `<data_dir>/cells`.
    pub root: PathBuf,
}

impl CellsPaths {
    /// `<data_dir>/cells`.
    pub fn new(data_dir: &Path) -> Self {
        Self {
            root: data_dir.join(crate::DIR),
        }
    }
    /// `cells.sqlite`.
    pub fn db(&self) -> PathBuf {
        self.root.join("cells.sqlite")
    }
    /// `shim.py` (written at start, binary-owned).
    pub fn shim(&self) -> PathBuf {
        self.root.join("shim.py")
    }
    /// `rc/` (bash.rc, zsh/.zshenv, zsh/.zshrc, sh.rc, rc.ps1).
    pub fn rc_dir(&self) -> PathBuf {
        self.root.join("rc")
    }
    /// `kernels/<kernel>/` (per-run `run.sh` scratch, deleted at mark D).
    pub fn kernel_dir(&self, kernel: &str) -> PathBuf {
        self.root.join("kernels").join(kernel)
    }
    /// `runs/<run>/` (code, term.log, out.ansi, out.txt, outputs.json, img-<n>.png).
    pub fn run_dir(&self, run: &RunId) -> PathBuf {
        self.root.join("runs").join(run.as_str())
    }
}

/// What every kernel gets at start.
#[derive(Clone)]
pub struct KernelCtx {
    /// Its key.
    pub id: KernelId,
    /// The session owning its PTY job: the chat session, or a note's UUIDv5 kernel session
    /// (`note::kernel_session`, §5.1 W-b).
    pub owner: SessionId,
    /// `platform::env::login_env` after the §5.4 drop/set rules.
    pub login_env: Arc<BTreeMap<String, String>>,
    /// `~/.theuth/cells/`.
    pub paths: CellsPaths,
    /// The section.
    pub config: Arc<CellsConfig>,
    /// The jobs actor (shell kernels start their PTY with `StartPty{program, cell}`); `None`
    /// in tests and when the tools plugin is absent.
    pub jobs: Option<ActorRef<tools::jobs::Jobs>>,
    /// The session's resource root for job logs (`SessionResources::root`).
    pub root: PathBuf,
}

/// A live kernel (§5.2–5.3). All methods take `&self`: Stop/Kill travel on their own lane and
/// must never wait behind output work (§7.3).
#[async_trait::async_trait]
pub trait Kernel: Send + Sync {
    /// Its spec.
    fn spec(&self) -> &KernelSpec;
    /// Current phase.
    fn phase(&self) -> KernelPhase;
    /// Start `req` (the kernel is ready; the actor queues). Returns once the run is handed to
    /// the process; progress and the end arrive on `sink`.
    async fn execute(&self, req: ExecuteRequest, sink: KernelSink) -> Result<()>;
    /// Bytes typed into the live terminal (shell) or the `input_reply` value (python).
    async fn input(&self, run: &RunId, bytes: Vec<u8>) -> Result<()>;
    /// Stop: Ctrl-C into the PTY / SIGINT to the python group / cancel the Monty checkout.
    async fn interrupt(&self) -> Result<()>;
    /// Kill: close the shell kernel (171F B16: the next run starts a fresh shell) / kill the
    /// python or Monty process (→ `dead`).
    async fn kill(&self) -> Result<()>;
    /// Close: stop any run, kill the process group, release the PTY.
    async fn shutdown(&self) -> Result<()>;
    /// Live cwd (OSC 7 / the shim), for the strip.
    fn live_cwd(&self) -> Option<String>;
}

/// Start the kernel for `spec` — one marked arm per implementer.
pub async fn start_kernel(
    spec: &KernelSpec,
    ctx: KernelCtx,
) -> Result<Box<dyn Kernel>, CellsError> {
    match spec {
        KernelSpec::Shell { .. } => crate::shell::start(spec, ctx).await, // 171:E1
        KernelSpec::Python { .. } => crate::python::start(spec, ctx).await, // 171:E2
        KernelSpec::Monty => crate::monty::start(spec, ctx).await,        // 171:E2
    }
}

/// The CWD rule (§5.6 step 4), shared by every kernel: the prologue (`cd` + exports, or
/// python's `cwd`/`env` fields) is written for a whole-cell run, or for a line/selection run
/// whose kernel's previous run came from another cell (`prev`), or which is the kernel's first
/// run (`prev == None`).
pub fn needs_prologue(mode: &RunMode, this: &CellRef, prev: Option<&CellRef>) -> bool {
    match mode {
        RunMode::Cell => true,
        RunMode::Line { .. } | RunMode::Selection { .. } => prev != Some(this),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{DocId, NoteKey};

    fn chat(o: u32) -> CellRef {
        CellRef::Chat {
            session: SessionId(uuid::Uuid::nil()),
            entry: "m1".into(),
            ordinal: o,
        }
    }

    /// §5.6 worked example, row by row (A = ordinal 1, B = ordinal 2).
    #[test]
    fn cwd_rule_table() {
        let (a, b) = (chat(1), chat(2));
        let line2 = RunMode::Line { line: 2 };
        let sel = RunMode::Selection { from: 0, to: 9 };
        // Run A: whole cell → prologue.
        assert!(needs_prologue(&RunMode::Cell, &a, Some(&b)));
        assert!(needs_prologue(&RunMode::Cell, &a, Some(&a)));
        // then line 2 of A: previous run was A → no prologue.
        assert!(!needs_prologue(&line2, &a, Some(&a)));
        // then Run B, then line 2 of A → prologue.
        assert!(needs_prologue(&line2, &a, Some(&b)));
        // fresh kernel, selection of A → prologue.
        assert!(needs_prologue(&sel, &a, None));
        assert!(!needs_prologue(&sel, &a, Some(&a)));
        // a note cell is "another cell" for a chat cell.
        let n = CellRef::Note {
            doc: DocId("d".into()),
            key: NoteKey::Name("x".into()),
        };
        assert!(needs_prologue(&line2, &a, Some(&n)));
    }
}
