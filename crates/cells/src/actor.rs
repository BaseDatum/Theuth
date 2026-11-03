//! Owner: E1 (public method set FROZEN by C) — the cells actor and its cheap `Cells` handle
//! (design 171 §5, §7, §15.2): one async method per RPC. Skeleton bodies are neutral: reads
//! answer empty, mutations refuse `NotBuilt("E1")`, nothing is registered with the agent,
//! nothing is written under `~/.theuth`, no event is emitted. The note-scope arms call
//! `crate::note` at the lines marked `// 171:E2` so E2 never edits this file.

use std::sync::Arc;

use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use runtime::Runtime;
use tokio::sync::mpsc;

use crate::config::CellsConfig;
use crate::error::{CellsError, Result};
use crate::kernel::CellsPaths;
use crate::note;
use crate::types::{
    CellEvent, CellLatest, CellRef, CellRun, DocId, Interpreter, KernelId, OutputPage, PtyChunk,
    RunAllRequest, RunGet, RunId, RunRequest, Scope, StateReply,
};

/// Registry name of the [`CellsActor`].
pub const CELLS_ACTOR: &str = "cells";

/// The cells actor: one per engine, registered as [`CELLS_ACTOR`]. Idle in the skeleton.
#[derive(kameo::Actor)]
pub struct CellsActor {
    cells: Cells,
}

/// Ask the actor for its handle (how the RPC layer finds it).
pub struct Handle;

impl Message<Handle> for CellsActor {
    type Reply = Cells;

    async fn handle(&mut self, _: Handle, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.cells.clone()
    }
}

struct Inner {
    rt: Runtime,
    config: Arc<CellsConfig>,
    paths: CellsPaths,
}

/// The cheap, cloneable handle every caller uses (RPC, runtime functions, the plugin).
#[derive(Clone, kameo::Reply)]
pub struct Cells {
    inner: Arc<Inner>,
}

/// Spawn and register the idle actor; returns its handle.
pub async fn start(rt: &Runtime, config: CellsConfig) -> Cells {
    let cells = Cells {
        inner: Arc::new(Inner {
            rt: rt.clone(),
            paths: CellsPaths::new(&rt.data_dir),
            config: Arc::new(config),
        }),
    };
    let actor = CellsActor::spawn(CellsActor {
        cells: cells.clone(),
    });
    if let Err(e) = actor.register(CELLS_ACTOR) {
        tracing::warn!(error = %e, "cells actor already registered");
    }
    cells
}

impl Cells {
    /// The running engine's handle; `None` when the plugin is disabled or not started.
    pub async fn lookup() -> Option<Cells> {
        let actor = ActorRef::<CellsActor>::lookup(CELLS_ACTOR).ok().flatten()?;
        actor.ask(Handle).await.ok()
    }

    /// The section as read at start.
    pub fn config(&self) -> &CellsConfig {
        &self.inner.config
    }

    /// `~/.theuth/cells/`.
    pub fn paths(&self) -> &CellsPaths {
        &self.inner.paths
    }

    /// Publish a [`CellEvent`] on the bus (the RPC bridge fans it out to `subscribe_events`).
    pub async fn publish(&self, event: CellEvent) {
        self.inner.rt.publish(event).await;
    }

    /// `cell_run`: guard, read the source, compare `seen`, enqueue; idempotent per run id.
    pub async fn run(&self, req: RunRequest) -> Result<CellRun, CellsError> {
        if let CellRef::Note { doc, key } = &req.cell {
            let _source = note::source(doc, key).await?; // 171:E2
        }
        Err(CellsError::NotBuilt("E1"))
    }

    /// `cell_run_all`: the engine pass (§7.2); returns the pass id.
    pub async fn run_all(&self, req: RunAllRequest) -> Result<String, CellsError> {
        let _ = req;
        Err(CellsError::NotBuilt("E1"))
    }

    /// `cell_stop`: Stop (or Kill) a run; a queued run is cancelled.
    pub async fn stop(&self, run: RunId, kill: bool) -> Result<(), CellsError> {
        let _ = (run, kill);
        Err(CellsError::NotBuilt("E1"))
    }

    /// `cell_input`: bytes typed into the run's live terminal / its `input()` answer.
    pub async fn input(&self, run: RunId, data: Vec<u8>) -> Result<(), CellsError> {
        let _ = (run, data);
        Err(CellsError::NotBuilt("E1"))
    }

    /// `cell_ask`: wake the session with that run's result (un-park it).
    pub async fn ask(&self, run: RunId) -> Result<(), CellsError> {
        let _ = run;
        Err(CellsError::NotBuilt("E1"))
    }

    /// `cell_state`: problems, chips, armed/superseded/stale per cell, and the scope's kernels.
    pub async fn state(&self, scope: Scope, cells: Vec<CellRef>) -> Result<StateReply, CellsError> {
        if let Scope::Doc(_doc) = &scope {
            // 171:E2 note scope: resolve note sources with note::source here.
        }
        let _ = cells;
        Ok(StateReply {
            cells: Vec::new(),
            kernels: Vec::new(),
        })
    }

    /// `cell_run_get`: the record and its outputs (text capped).
    pub async fn run_get(&self, run: RunId) -> Result<RunGet, CellsError> {
        // 171:E2 note runs read the same store; nothing note-specific expected here.
        Err(CellsError::NotFound(run.0))
    }

    /// `cell_output`: a page of the cleaned (or ANSI) text.
    pub async fn output(
        &self,
        run: RunId,
        offset: u64,
        limit: u32,
        ansi: bool,
    ) -> Result<OutputPage, CellsError> {
        let _ = (offset, limit, ansi);
        Err(CellsError::NotFound(run.0))
    }

    /// `cell_output_file`: one file of `runs/<run>/` (images).
    pub async fn output_file(&self, run: RunId, name: String) -> Result<Vec<u8>, CellsError> {
        let _ = name;
        Err(CellsError::NotFound(run.0))
    }

    /// `cell_runs`: the scope's runs, newest first.
    pub async fn runs(&self, scope: Scope, limit: u32) -> Result<Vec<CellRun>, CellsError> {
        let _ = (scope, limit);
        Ok(Vec::new())
    }

    /// `cell_interpreters`: `interpreters(cwd)` on this engine (the RPC routes `engine`).
    pub async fn interpreters(&self, cwd: String) -> Result<Vec<Interpreter>, CellsError> {
        let env = std::collections::BTreeMap::new();
        Ok(crate::interp::interpreters(std::path::Path::new(&cwd), &env).await) // 171:E2
    }

    /// `cell_kernel_restart`: Close now; the next run starts a fresh kernel.
    pub async fn kernel_restart(&self, kernel: KernelId) -> Result<(), CellsError> {
        let _ = kernel;
        Err(CellsError::NotBuilt("E1"))
    }

    /// `cell_kernel_close`: Stop any run, kill the process group, release the PTY.
    pub async fn kernel_close(&self, kernel: KernelId) -> Result<(), CellsError> {
        let _ = kernel;
        Err(CellsError::NotBuilt("E1"))
    }

    /// `cell_env_refresh`: re-capture the login environment.
    pub async fn env_refresh(&self) -> Result<(), CellsError> {
        Err(CellsError::NotBuilt("E1"))
    }

    /// `cell_live`: runs in flight on this engine (quit / update sheet).
    pub async fn live(&self) -> Result<Vec<CellRun>, CellsError> {
        Ok(Vec::new())
    }

    /// `cell_latest`: the replicated latest runs of a note (§13.2).
    pub async fn latest(&self, doc: DocId) -> Result<Vec<CellLatest>, CellsError> {
        note::latest(&doc).await // 171:E2
    }

    /// `cell_attach`: the run's live terminal from mark C: a `reset` chunk, then `data`, then
    /// `exit`.
    pub async fn attach(&self, run: RunId) -> Result<mpsc::Receiver<PtyChunk>, CellsError> {
        let _ = run;
        Err(CellsError::NotBuilt("E1"))
    }

    /// `cell_detach`: drop the attachment (also implied when the receiver is dropped).
    pub async fn detach(&self, run: RunId) -> Result<(), CellsError> {
        let _ = run;
        Ok(())
    }
}
