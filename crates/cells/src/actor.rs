//! Owner: E1 (public method set FROZEN by C) — the cells actor and its cheap `Cells` handle
//! (design 171 §5, §7, §8, §9.3, §12.1, §15.2): one async method per RPC.
//!
//! Shape: the kameo actor only hands out the handle and listens to `AgentEvent` (parked
//! results consumed at a turn start, §9.3). The work lives behind the handle: a sync-locked
//! [`State`] (kernels, in-flight runs) that no `await` ever holds, one FIFO worker task per
//! kernel (§5.10), and Stop/Kill that reach the kernel straight from the handle — never
//! behind output work (§7.3). The record is written before anything is announced (§8.7):
//! files + finished row → `CellEvent::RunFinished` → `<cell_result>`.
//!
//! Chat scope is built here; note scope goes through `crate::note` at the `// 171:E2` lines
//! (lead pin L6).

mod chat;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use base64::Engine as _;
use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use parking_lot::Mutex;
use runtime::Runtime;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, oneshot};

use self::chat::{Presented, Transcript};
use crate::config::CellsConfig;
use crate::error::{CellsError, Result};
use crate::kernel::{CellsPaths, ExecuteRequest, Kernel, KernelCtx, KernelEvent};
use crate::note;
use crate::store::Store;
use crate::types::{
    Armed, Cell, CellEvent, CellLatest, CellOutput, CellRef, CellRun, CellState, Chip, DocId,
    ExitStatus, Interpreter, KernelId, KernelPhase, KernelSpec, KernelState, Lang, NoteDefaults,
    NoteKey, OutputPage, PassEnd, PassStop, Problem, PtyChunk, PythonChoice, RunAllRequest,
    RunGet, RunId, RunMode, RunRequest, RunState, Scope, SessionId, Severity, ShellKind,
    StateReply, ValidateCtx,
};

/// Registry name of the [`CellsActor`].
pub const CELLS_ACTOR: &str = "cells";

/// The cells actor: one per engine, registered as [`CELLS_ACTOR`].
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

/// §9.3: a turn opened — the parked results it carries (`run="<id>"` in its text) joined it
/// (`consumed_ms`). Scoped and peer-opened turns do not take parked results (A1), so a
/// `TurnStarted` that names none consumes none.
impl Message<agent::AgentEvent> for CellsActor {
    type Reply = ();

    async fn handle(&mut self, e: agent::AgentEvent, _: &mut Context<Self, Self::Reply>) {
        if let agent::AgentEvent::TurnStarted { session, text, .. } = e
            && let Ok(store) = self.cells.store()
        {
            let ids = runs_named(&text);
            if ids.is_empty() {
                return;
            }
            let store = store.clone();
            tokio::spawn(async move {
                let _ = store
                    .consumed(&Scope::Session(session).key(), &ids, now_ms())
                    .await;
            });
        }
    }
}

/// Every `run="<id>"` in a turn's text (the `<cell_result run=…>` tags it carries, L2).
fn runs_named(text: &str) -> Vec<RunId> {
    let mut out: Vec<RunId> = Vec::new();
    for part in text.split("run=\"").skip(1) {
        if let Some((id, _)) = part.split_once('"')
            && !id.is_empty()
            && !out.iter().any(|r| r.0 == id)
        {
            out.push(RunId(id.to_owned()));
        }
    }
    out
}

/// The captured login environment (§5.4) and why it fell back, if it did.
#[derive(Clone)]
struct Env {
    vars: Arc<BTreeMap<String, String>>,
    warning: Option<String>,
}

/// What `runs/<run>/meta.json` keeps beside the row: what a re-built `<cell_result>` needs.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Meta {
    name: Option<String>,
    label: Option<String>,
    private: bool,
    review: bool,
    edited_code: Option<String>,
    /// The cell's `expect` items, checked at the end on the cleaned text (§7.5).
    expect: Vec<crate::types::Expect>,
}

/// How a run was stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stop {
    Stopped,
    Killed,
    Timeout,
}

/// The stop that sticks: the first reason wins over a later timeout (a user Stop stays
/// "stopped"), a Kill overrides a Stop, nothing overrides a timeout.
fn stop_after(prev: Option<Stop>, how: Stop) -> Option<Stop> {
    match (prev, how) {
        (Some(Stop::Timeout), _) => Some(Stop::Timeout),
        (Some(s), Stop::Timeout) => Some(s),
        (_, h) => Some(h),
    }
}

/// A queued or running run.
struct Live {
    record: CellRun,
    key: String,
    code: String,
    cwd: PathBuf,
    env: Vec<(String, String)>,
    timeout: Option<Duration>,
    meta: Meta,
    executing: bool,
    stop: Option<Stop>,
    /// Live bytes since mark C (capped tail) for a late `cell_attach`.
    buf: Vec<u8>,
    tx: broadcast::Sender<Arc<[u8]>>,
    done: Vec<oneshot::Sender<CellRun>>,
}

/// One kernel slot (§5.1 key `(Scope, Spec)`).
struct Slot {
    id: KernelId,
    label: String,
    kernel: Option<Arc<dyn Kernel>>,
    log: Option<PathBuf>,
    tx: mpsc::UnboundedSender<RunId>,
    prev_cell: Option<CellRef>,
    last_used: Instant,
    busy: bool,
    /// §5.4 capture failure, shown on the strip.
    env_warning: Option<String>,
}

#[derive(Default)]
struct State {
    kernels: HashMap<String, Slot>,
    runs: HashMap<RunId, Live>,
    /// The queued/running run of each cell (a second press attaches, §5.10).
    by_cell: HashMap<String, RunId>,
    /// Run-all passes the user stopped.
    stopped_passes: std::collections::HashSet<String>,
    /// The last finished pass of each scope (`json(scope)`), cleared when one starts.
    last_pass: HashMap<String, PassEnd>,
}

struct Inner {
    rt: Runtime,
    config: Arc<CellsConfig>,
    paths: CellsPaths,
    store: std::result::Result<Store, String>,
    state: Mutex<State>,
    env: tokio::sync::Mutex<Option<Env>>,
    shutting_down: AtomicBool,
}

/// The cheap, cloneable handle every caller uses (RPC, runtime functions, the plugin).
#[derive(Clone, kameo::Reply)]
pub struct Cells {
    inner: Arc<Inner>,
}

/// Live-byte tail kept per run for late attachers.
const ATTACH_BUF: usize = 512 * 1024;

/// Spawn and register the actor; open the store; settle what a previous engine left (§5.3)
/// and re-park unconsumed results (§9.3); returns its handle.
pub async fn start(rt: &Runtime, config: CellsConfig) -> Cells {
    let paths = CellsPaths::new(&rt.data_dir);
    let store = Store::open(&paths.db()).await.map_err(|e| {
        tracing::error!(error = %e, "cells store unavailable; runs are refused");
        e.to_string()
    });
    let cells = Cells {
        inner: Arc::new(Inner {
            rt: rt.clone(),
            paths,
            config: Arc::new(config),
            store,
            state: Mutex::new(State::default()),
            env: tokio::sync::Mutex::new(None),
            shutting_down: AtomicBool::new(false),
        }),
    };
    let actor = CellsActor::spawn_with_mailbox(
        CellsActor {
            cells: cells.clone(),
        },
        runtime::subscriber_mailbox(),
    );
    if let Err(e) = actor.register(CELLS_ACTOR) {
        tracing::warn!(error = %e, "cells actor already registered");
    }
    rt.subscribe::<CellsActor, agent::AgentEvent>(&actor).await;
    let c = cells.clone();
    tokio::spawn(async move { c.boot().await });
    let c = cells.clone();
    tokio::spawn(async move {
        c.inner.rt.shutdown.cancelled().await;
        // 109 R1: what is running now ends `lost` at the next start, never here.
        c.inner.shutting_down.store(true, Ordering::SeqCst);
    });
    cells
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn json<T: Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

fn sha(text: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// §5.4: dropped before use / set for every kernel.
const DROP: &[&str] = &["PWD", "OLDPWD", "SHLVL", "_", "TERM", "COLUMNS", "LINES"];

fn apply_env_rules(vars: &mut BTreeMap<String, String>) {
    vars.retain(|k, _| !DROP.contains(&k.as_str()) && !k.starts_with("THEUTH_"));
    vars.insert("TERM".into(), "xterm-256color".into());
    vars.insert("COLORTERM".into(), "truecolor".into());
    vars.insert("THEUTH_CELL".into(), "1".into());
}

/// `~` / `~/x` against `home`.
fn expand_home(p: &str, home: Option<&str>) -> PathBuf {
    match (p, home) {
        ("~", Some(h)) => PathBuf::from(h),
        (_, Some(h)) if p.starts_with("~/") => Path::new(h).join(&p[2..]),
        _ => PathBuf::from(p),
    }
}

/// `[from, to)` in UTF-8 bytes of the code (U1's selection offsets, header excluded); `None`
/// out of range or off a char boundary.
fn byte_slice(s: &str, from: u32, to: u32) -> Option<&str> {
    s.get(from as usize..to as usize)
}

/// `{{secret:NAME}}` → value, in memory only (invariant 9: shells write it only into the
/// 0600 run file's prologue).
fn expand_secret(v: &str) -> Result<String> {
    if !v.contains("{{secret:") {
        return Ok(v.to_owned());
    }
    let mut j = serde_json::Value::String(v.to_owned());
    tools::secrets::expand(&mut j).map_err(|e| CellsError::Failed(format!("secret: {e}")))?;
    Ok(j.as_str().unwrap_or_default().to_owned())
}

fn shell_kind(lang: Lang) -> Option<ShellKind> {
    match lang {
        Lang::Bash => Some(ShellKind::Bash),
        Lang::Zsh => Some(ShellKind::Zsh),
        Lang::Sh => Some(ShellKind::Sh),
        Lang::Pwsh => Some(ShellKind::Pwsh),
        Lang::Python | Lang::Javascript | Lang::Typescript => None,
    }
}

/// The source the engine read for one cell, checked against the rest of its scope.
struct Inspected {
    presented: Presented,
    problems: Vec<Problem>,
    chips: Vec<Chip>,
    superseded_by: Option<CellRef>,
    /// `after:` and the gate's state (`None` = no `after`).
    after: Option<Gate>,
    cwd: Option<PathBuf>,
    cwd_exists: bool,
    lineage: Option<String>,
    /// A note's ```` ```cells ```` defaults (`shell:`, `python:`); `None` in chat.
    defaults: Option<NoteDefaults>,
}

/// An `after` gate (§7.6).
enum Gate {
    /// Not looked up yet: the gate's name and its newest version's current digest.
    Pending {
        name: String,
        current: String,
    },
    Armed(Armed),
    Disarmed(String),
}

/// The pure refusals of a press, in order (§12.1 inv. 5, 7, 8; §5.6): the source changed or
/// was superseded, the cell is invalid, a `danger` cell pressed other than whole, the cwd
/// is missing, the `after` gate is disarmed. Lint never refuses (inv. 12). The cwd on `Ok`.
fn refuse(i: &Inspected, seen: &str, mode: &RunMode) -> Result<PathBuf> {
    if i.presented.digest != seen || i.superseded_by.is_some() {
        return Err(CellsError::Changed);
    }
    if let Some(p) = i.problems.iter().find(|p| p.severity == Severity::Error) {
        return Err(CellsError::Invalid(p.message.clone()));
    }
    let o = &i.presented.cell.options;
    if o.danger && *mode != RunMode::Cell {
        let label = o
            .label
            .clone()
            .or_else(|| o.name.clone())
            .unwrap_or_else(|| "its button".into());
        return Err(CellsError::Danger(label));
    }
    let Some(cwd) = i.cwd.clone().filter(|_| i.cwd_exists) else {
        return Err(CellsError::FolderNotFound(
            o.cwd.clone().unwrap_or_default(),
        ));
    };
    if let Some(Gate::Disarmed(why)) = &i.after {
        return Err(CellsError::Disarmed(why.clone()));
    }
    Ok(cwd)
}

/// Run all stops BEFORE a `danger` cell or one with a blocking lint chip (§7.2, inv. 7, 12):
/// the reason it shows, `None` when the pass may run the cell.
fn pass_boundary(cell: &Cell, chips: &[Chip]) -> Option<String> {
    if cell.options.danger {
        return Some("danger — runs from its button only".into());
    }
    chips
        .iter()
        .find(|c| c.stops_run_all)
        .map(|c| c.text.clone())
}

/// Why a pass stopped AT a run that did not succeed: its end, or the missed `expect`.
fn ran_why(r: &CellRun) -> String {
    let missed = r.expect.iter().filter(|e| !e.met).count();
    match (r.state, &r.exit) {
        (RunState::Timeout, _) => "timed out".into(),
        (RunState::Killed, _) => "killed".into(),
        (RunState::Stopped, _) => "stopped".into(),
        (RunState::Cancelled, _) => "cancelled".into(),
        (RunState::Lost, _) => "lost".into(),
        (_, Some(ExitStatus::Code(0))) if missed > 0 => {
            format!("expect missed {missed}/{}", r.expect.len())
        }
        (_, Some(ExitStatus::Code(c))) => format!("exit {c}"),
        (_, Some(ExitStatus::Signal(s))) => format!("signal {s}"),
        (_, None) => "failed".into(),
    }
}

/// 128 bits as 26 Crockford base32 chars, most significant first: a UUIDv7 keeps its time
/// order and gets the shape of the clients' `r_` + ULID ids (B18).
fn crockford(n: u128) -> String {
    const B32: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    (0..26)
        .rev()
        .map(|i| char::from(B32[((n >> (i * 5)) & 31) as usize]))
        .collect()
}

/// §9.3: `(wake, delivery word)` from whether a turn is running, the cell's (or pass's)
/// `review`, and a forced wake (Ask theuth `Some(true)`, a pass end `Some(review)`).
fn wake_rule(running: bool, review: bool, force: Option<bool>) -> (bool, &'static str) {
    let wake = running || force.unwrap_or(review);
    let how = if running {
        "joined"
    } else if wake {
        "woke"
    } else {
        "parked"
    };
    (wake, how)
}

/// Everything a press needs, resolved.
struct Prepared {
    inspected: Inspected,
    spec: KernelSpec,
    interpreter: Interpreter,
    code: String,
    code_sha: String,
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

    fn store(&self) -> Result<&Store> {
        self.inner
            .store
            .as_ref()
            .map_err(|e| CellsError::Failed(e.clone()))
    }

    // ---- environment (§5.4) ----

    async fn login_env(&self) -> Env {
        let mut g = self.inner.env.lock().await;
        if let Some(e) = &*g {
            return e.clone();
        }
        let e = capture_env(self.inner.config.login_env_timeout_secs).await;
        *g = Some(e.clone());
        e
    }

    // ---- reading the record (§3.4, §3.5, §12.1 inv. 5) ----

    /// Inspect one chat cell against the session's record; `None` = no such cell.
    fn inspect(t: &Transcript, cell: &CellRef, home: Option<&str>) -> Option<Inspected> {
        let CellRef::Chat {
            session,
            entry,
            ordinal,
        } = cell
        else {
            return None;
        };
        let seq: i64 = entry.parse().ok()?;
        let all = t.all();
        let mine = all.iter().find(|(s, _)| *s == seq)?;
        let presented = mine.1.get((*ordinal as usize).checked_sub(1)?)?.clone();
        let scope = Scope::Session(*session);
        let opts = &presented.cell.options;
        // Validation scope (§9.2): every name of this reply (each occurrence) plus the
        // names of the session's other replies once.
        let mut names: Vec<String> = mine
            .1
            .iter()
            .filter_map(|p| p.cell.options.name.clone())
            .collect();
        let mut edges: Vec<(String, String)> = Vec::new();
        for (s, cells) in &all {
            for p in cells {
                let Some(n) = &p.cell.options.name else {
                    continue;
                };
                if *s != seq && !names.contains(n) {
                    names.push(n.clone());
                }
                if let Some(a) = &p.cell.options.after
                    && !(*s == seq && p.digest == presented.digest)
                {
                    edges.push((n.clone(), a.clone()));
                }
            }
        }
        let cwd = opts.cwd.as_deref().map(|c| expand_home(c, home));
        let cwd_exists = cwd.as_ref().is_some_and(|c| c.is_absolute() && c.is_dir());
        let ctx = ValidateCtx {
            chat: true,
            names,
            after_edges: edges,
            secrets: None,
            defaults: None,
            monty_ok: None,
            monty_why: None,
            cwd_exists: cwd.as_ref().map(|_| cwd_exists),
            second_defaults: false,
        };
        let problems = surface::cell::validate(&presented.cell, &ctx);
        let mut chips = surface::cell::lint(&presented.cell);
        chips.extend(surface::cell::lint_scripts(&presented.cell, &t.edited));
        // §3.5: a later reply presenting the same name supersedes this one.
        let superseded_by = opts.name.as_ref().and_then(|n| {
            all.iter()
                .rev()
                .filter(|(s, _)| *s > seq)
                .find_map(|(s, cells)| {
                    let i = cells
                        .iter()
                        .position(|p| p.cell.options.name.as_ref() == Some(n))?;
                    Some(CellRef::Chat {
                        session: *session,
                        entry: s.to_string(),
                        ordinal: i as u32 + 1,
                    })
                })
        });
        let lineage = opts.name.as_ref().map(|n| format!("{}/{n}", scope.key()));
        let after = opts.after.clone().map(|a| {
            // The newest version of the gate's name and its current digest (§7.6).
            let current = all.iter().rev().find_map(|(_, cells)| {
                cells
                    .iter()
                    .find(|p| p.cell.options.name.as_deref() == Some(a.as_str()))
                    .map(|p| p.digest.clone())
            });
            Gate::Pending {
                name: a,
                current: current.unwrap_or_default(),
            }
        });
        Some(Inspected {
            presented,
            problems,
            chips,
            superseded_by,
            after,
            cwd,
            cwd_exists,
            lineage,
            defaults: None,
        })
    }

    /// Inspect one note cell (§3.3, §4.4) against the live block, the note's other cells
    /// and its ```` ```cells ```` defaults; also the block itself (its author, invariant 6).
    async fn inspect_note(
        &self,
        cell: &CellRef,
        doc: &DocId,
        key: &NoteKey,
        env: &Env,
    ) -> Result<(Inspected, note::NoteSource)> {
        let source = note::source(doc, key).await?;
        let parsed =
            surface::cell::parse(&source.language, &source.text).ok_or(CellsError::Changed)?;
        let presented = Presented {
            digest: surface::cell::digest(&source.text),
            cell: parsed,
        };
        let md = note::markdown(doc).await.unwrap_or_default();
        let (defaults, second_defaults) = note::defaults_of(&md);
        let others = chat::cells_of(&md);
        let opts = &presented.cell.options;
        let mut names: Vec<String> = others
            .iter()
            .filter_map(|p| p.cell.options.name.clone())
            .collect();
        if let Some(n) = &opts.name
            && !names.contains(n)
        {
            names.push(n.clone());
        }
        let after_edges = others
            .iter()
            .filter(|p| p.cell.options.name != opts.name)
            .filter_map(|p| Some((p.cell.options.name.clone()?, p.cell.options.after.clone()?)))
            .collect();
        let home = env.vars.get("HOME").map(PathBuf::from);
        let cwd = note::cell_cwd(opts.cwd.as_deref(), &defaults, home.as_deref());
        let cwd_exists = cwd.as_deref().is_some_and(Path::is_dir);
        let python = opts.python.as_ref().or(defaults.python.as_ref());
        let monty = match (presented.cell.lang, python) {
            (Lang::Python, Some(PythonChoice::Monty)) => {
                crate::monty::sufficient(&presented.cell.code).await
            }
            _ => None,
        };
        let ctx = ValidateCtx {
            chat: false,
            names,
            after_edges,
            secrets: None,
            defaults: Some(defaults.clone()),
            monty_ok: monty.as_ref().map(Result::is_ok),
            monty_why: monty.and_then(Result::err),
            cwd_exists: cwd.as_ref().map(|_| cwd_exists),
            second_defaults,
        };
        // The gate's current digest is its block's text as the engine reads it (§7.6).
        let after = match &opts.after {
            Some(a) => Some(Gate::Pending {
                name: a.clone(),
                current: note::source(doc, &NoteKey::Name(a.clone()))
                    .await
                    .map(|s| surface::cell::digest(&s.text))
                    .unwrap_or_default(),
            }),
            None => None,
        };
        let lineage = opts
            .name
            .as_ref()
            .map(|n| format!("{}/{n}", cell.scope().key()));
        let inspected = Inspected {
            problems: surface::cell::validate(&presented.cell, &ctx),
            chips: surface::cell::lint(&presented.cell),
            superseded_by: None,
            after,
            cwd,
            cwd_exists,
            lineage,
            defaults: Some(defaults),
            presented,
        };
        Ok((inspected, source))
    }

    /// Resolve a pending `after` gate of `i` against the store (§7.6).
    async fn arm(&self, scope: &Scope, i: &mut Inspected) -> Result<()> {
        let Some(Gate::Pending { name, current }) = &i.after else {
            return Ok(());
        };
        let (name, current) = (name.clone(), current.clone());
        let lineage = format!("{}/{name}", scope.key());
        let gate = self.store()?.latest_finished_of_lineage(&lineage).await?;
        let gate = gate.map(|r| r.run);
        let now = now_ms();
        let max = self.inner.config.after_max_age_mins;
        i.after = Some(
            match gate
                .as_ref()
                .and_then(|g| crate::run::armed(g, &current, now, max))
            {
                Some(a) => Gate::Armed(a),
                None => Gate::Disarmed(crate::run::disarmed_reason(
                    gate.as_ref(),
                    &name,
                    &current,
                    now,
                    max,
                )),
            },
        );
        Ok(())
    }

    /// The press path's checks (§5.6, §7.1, §12.1 inv. 5, 7, 8): read, compare, validate,
    /// gate, resolve. Lint never blocks a single press (inv. 12).
    async fn prepare(
        &self,
        cell: &CellRef,
        seen: &str,
        mode: &RunMode,
        edited: Option<&str>,
        member_ack: Option<&str>,
    ) -> Result<Prepared> {
        let env = self.login_env().await;
        let inspected = match cell {
            CellRef::Chat { session, .. } => {
                let t = chat::transcript(*session).await?;
                let home = env.vars.get("HOME").map(String::as_str);
                Self::inspect(&t, cell, home).ok_or(CellsError::Changed)?
            }
            CellRef::Note { doc, key } => {
                let (inspected, source) = self.inspect_note(cell, doc, key, &env).await?;
                // Invariant 6: a member's text the owner has not run needs its `member_ack`.
                let latest = self.store()?.latest_of_cell(&json(cell)).await?;
                note::member_guard(&source, latest.map(|r| r.run).as_ref(), member_ack)?;
                inspected
            }
        };
        self.prepare_from(inspected, &cell.scope(), seen, mode, edited, &env)
            .await
    }

    /// [`Self::prepare`] once the source is read: gate, refuse, cut the code, resolve.
    async fn prepare_from(
        &self,
        mut inspected: Inspected,
        scope: &Scope,
        seen: &str,
        mode: &RunMode,
        edited: Option<&str>,
        env: &Env,
    ) -> Result<Prepared> {
        self.arm(scope, &mut inspected).await?;
        let cwd = refuse(&inspected, seen, mode)?;
        let c = inspected.presented.cell.clone();
        let base = edited.unwrap_or(&c.code);
        let code = match mode {
            RunMode::Cell => base.to_owned(),
            RunMode::Line { line } => base
                .split('\n')
                .nth((*line as usize).saturating_sub(1))
                .filter(|_| *line >= 1)
                .ok_or_else(|| CellsError::Invalid(format!("no line {line}")))?
                .to_owned(),
            RunMode::Selection { from, to } => byte_slice(base, *from, *to)
                .ok_or_else(|| CellsError::Invalid("the selection is outside the code".into()))?
                .to_owned(),
        };
        let code_sha = if *mode == RunMode::Cell && edited.is_none() {
            inspected.presented.digest.clone()
        } else {
            surface::cell::digest(&code)
        };
        let (spec, interpreter) = self
            .resolve(&c, &cwd, env, inspected.defaults.as_ref())
            .await?;
        Ok(Prepared {
            inspected,
            spec,
            interpreter,
            code,
            code_sha,
        })
    }

    /// Which program runs `c` (§6, §6.5); a note's `shell:` / `python:` defaults apply.
    async fn resolve(
        &self,
        c: &Cell,
        cwd: &Path,
        env: &Env,
        defaults: Option<&NoteDefaults>,
    ) -> Result<(KernelSpec, Interpreter)> {
        match shell_kind(c.lang) {
            Some(kind) => {
                let note_shell = defaults.and_then(|d| d.shell.as_deref());
                let (path, i) = crate::shell::resolve(kind, note_shell, &env.vars).await?;
                Ok((KernelSpec::Shell { kind, path }, i))
            }
            None if c.lang == Lang::Python => {
                let choice = c
                    .options
                    .python
                    .as_ref()
                    .or(defaults.and_then(|d| d.python.as_ref()));
                let r = crate::interp::resolve(choice, cwd, &env.vars, false).await;
                // §6.1 `auto → Monty`: no Python for this folder and Monty can run the cell.
                // ponytail: this cell's sufficiency only, not every auto cell of the scope.
                if matches!(choice, None | Some(PythonChoice::Auto))
                    && matches!(r, Err(CellsError::NoInterpreter(_)))
                    && matches!(crate::monty::sufficient(&c.code).await, Some(Ok(())))
                {
                    return crate::interp::resolve(choice, cwd, &env.vars, true).await;
                }
                r
            }
            None => Err(CellsError::NoInterpreter(
                "JavaScript cells come in a later wave".into(),
            )),
        }
    }

    // ---- the press (§5.10, §7) ----

    /// `cell_run`: guard, read the source, compare `seen`, enqueue; idempotent per run id.
    pub async fn run(&self, req: RunRequest) -> Result<CellRun, CellsError> {
        let store = self.store()?;
        let cell_key = json(&req.cell);
        // Idempotent run ids: the same id is the same run; a reused id elsewhere is refused.
        if let Some(r) = self.find_run(&req.run).await? {
            return if json(&r.cell) == cell_key && r.presented_sha == req.seen {
                Ok(r)
            } else {
                Err(CellsError::RunIdConflict(req.run.0))
            };
        }
        // A second press on a queued/running cell attaches to that run.
        if let Some(existing) = self.live_of_cell(&cell_key) {
            return Ok(existing);
        }
        let p = self
            .prepare(
                &req.cell,
                &req.seen,
                &req.mode,
                req.edited.as_deref(),
                req.member_ack.as_deref(),
            )
            .await?;
        let _ = store;
        self.enqueue(
            req.run,
            req.cell,
            req.mode,
            None,
            req.edited,
            req.seen,
            req.pressed_by,
            p,
        )
        .await
    }

    fn live_of_cell(&self, cell_key: &str) -> Option<CellRun> {
        let s = self.inner.state.lock();
        let id = s.by_cell.get(cell_key)?;
        s.runs.get(id).map(|l| l.record.clone())
    }

    async fn find_run(&self, id: &RunId) -> Result<Option<CellRun>> {
        if let Some(l) = self.inner.state.lock().runs.get(id) {
            return Ok(Some(l.record.clone()));
        }
        Ok(self.store()?.get(id).await?.map(|r| r.run))
    }

    /// Insert the queued row, write `code` + `meta.json`, hand the run to its kernel's FIFO.
    #[allow(clippy::too_many_arguments)]
    async fn enqueue(
        &self,
        id: RunId,
        cell: CellRef,
        mode: RunMode,
        pass: Option<String>,
        edited: Option<String>,
        seen: String,
        pressed_by: String,
        p: Prepared,
    ) -> Result<CellRun> {
        let scope = cell.scope();
        let kid = KernelId {
            scope: scope.clone(),
            spec: p.spec.clone(),
        };
        let key = json(&kid);
        let c = &p.inspected.presented.cell;
        let env = self.login_env().await;
        let label = crate::interp::chip(&p.interpreter, &env.vars);
        let interpreter = match &p.interpreter.path {
            Some(path) => format!("{label} {}", path.display()),
            None => label.clone(),
        };
        let cwd = p.inspected.cwd.clone().unwrap_or_default();
        let record = CellRun {
            id: id.clone(),
            cell: cell.clone(),
            mode,
            pass,
            kernel: kid.clone(),
            interpreter,
            cwd: cwd.to_string_lossy().into_owned(),
            env_names: c.options.env.iter().map(|(k, _)| k.clone()).collect(),
            presented_sha: seen,
            code_sha: p.code_sha.clone(),
            edited: edited.is_some(),
            pressed_by,
            state: RunState::Queued,
            exit: None,
            secs: None,
            expect: Vec::new(),
            summary: None,
            output_bytes: None,
            queued_ms: now_ms(),
            started_ms: None,
            ended_ms: None,
            masked: 0,
            private: c.options.private,
            name: c.options.name.clone(),
            input: None,
        };
        let meta = Meta {
            name: c.options.name.clone(),
            label: c.options.label.clone(),
            private: c.options.private,
            review: c.options.review,
            edited_code: edited,
            expect: c.options.expect.clone(),
        };
        // Files first: `code` keeps placeholders (they only ever live in `env`).
        let dir = self.inner.paths.run_dir(&id);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("code"), &p.code)?;
        std::fs::write(dir.join("meta.json"), json(&meta))?;
        self.make_room(&key)?;
        self.store()?
            .insert(&record, &scope.key(), p.inspected.lineage.as_deref())
            .await?;
        let cell_key = json(&cell);
        let spawn_worker = {
            let mut s = self.inner.state.lock();
            let mut new_rx = None;
            let slot = s.kernels.entry(key.clone()).or_insert_with(|| {
                let (tx, rx) = mpsc::unbounded_channel();
                new_rx = Some(rx);
                Slot {
                    id: kid.clone(),
                    label: label.clone(),
                    kernel: None,
                    log: None,
                    tx,
                    prev_cell: None,
                    last_used: Instant::now(),
                    busy: false,
                    env_warning: env.warning.clone(),
                }
            });
            slot.label = label;
            slot.env_warning = env.warning.clone();
            let tx = slot.tx.clone();
            s.runs.insert(
                id.clone(),
                Live {
                    record: record.clone(),
                    key: key.clone(),
                    code: p.code,
                    cwd,
                    env: c.options.env.clone(),
                    timeout: c.options.timeout_secs.map(Duration::from_secs),
                    meta,
                    executing: false,
                    stop: None,
                    buf: Vec::new(),
                    tx: broadcast::channel(256).0,
                    done: Vec::new(),
                },
            );
            s.by_cell.insert(cell_key, id.clone());
            let _ = tx.send(id.clone());
            new_rx
        };
        if let Some(rx) = spawn_worker {
            let me = self.clone();
            let k = key.clone();
            tokio::spawn(async move { me.worker(k, rx).await });
        }
        self.publish(CellEvent::RunQueued {
            run: id,
            scope: scope.clone(),
        })
        .await;
        self.publish(CellEvent::KernelChanged { kernel: kid }).await;
        Ok(record)
    }

    /// §5.10 budget: a new kernel over `max_kernels` closes the oldest idle one, else refuses.
    fn make_room(&self, key: &str) -> Result<()> {
        let victim = {
            let s = self.inner.state.lock();
            if s.kernels.contains_key(key) {
                return Ok(());
            }
            let live = s.kernels.values().filter(|k| k.kernel.is_some()).count();
            if live < self.inner.config.max_kernels as usize {
                return Ok(());
            }
            let busy = |k: &Slot| k.busy || s.runs.values().any(|l| l.key == json(&k.id));
            s.kernels
                .iter()
                .filter(|(_, k)| k.kernel.is_some() && !busy(k))
                .min_by_key(|(_, k)| k.last_used)
                .map(|(key, _)| key.clone())
        };
        match victim {
            Some(v) => {
                let slot = self.inner.state.lock().kernels.remove(&v);
                if let Some(k) = slot.and_then(|s| s.kernel) {
                    tokio::spawn(async move {
                        let _ = k.shutdown().await;
                    });
                }
                Ok(())
            }
            None => Err(CellsError::KernelBusy(self.inner.config.max_kernels)),
        }
    }

    /// One FIFO per kernel, one run at a time (§5.10).
    async fn worker(self, key: String, mut rx: mpsc::UnboundedReceiver<RunId>) {
        while let Some(id) = rx.recv().await {
            self.execute_one(&key, id).await;
        }
    }

    async fn execute_one(&self, key: &str, id: RunId) {
        let job = {
            let mut s = self.inner.state.lock();
            let Some(l) = s.runs.get_mut(&id) else {
                return; // cancelled while queued
            };
            l.executing = true;
            let job = (
                l.record.cell.clone(),
                l.record.mode.clone(),
                l.code.clone(),
                l.cwd.clone(),
                l.env.clone(),
                l.timeout,
            );
            if let Some(slot) = s.kernels.get_mut(key) {
                slot.busy = true;
            }
            job
        };
        let (cell, mode, code, cwd, env, timeout) = job;
        let (kernel, prev) = match self.ensure_kernel(key).await {
            Ok(k) => k,
            Err(e) => {
                self.idle(key, None, false).await;
                self.end_unstarted(&id, RunState::Failed, Some(e.to_string()))
                    .await;
                return;
            }
        };
        let prologue = crate::kernel::needs_prologue(&mode, &cell, prev.as_ref());
        let env = if prologue {
            match env
                .iter()
                .map(|(k, v)| expand_secret(v).map(|v| (k.clone(), v)))
                .collect::<Result<Vec<_>>>()
            {
                Ok(e) => e,
                Err(e) => {
                    self.idle(key, None, false).await;
                    self.end_unstarted(&id, RunState::Failed, Some(e.to_string()))
                        .await;
                    return;
                }
            }
        } else {
            Vec::new()
        };
        if self
            .inner
            .state
            .lock()
            .runs
            .get(&id)
            .is_some_and(|l| l.stop.is_some())
        {
            self.idle(key, None, false).await;
            self.end_unstarted(&id, RunState::Cancelled, None).await;
            return;
        }
        let (sink, mut events) = mpsc::channel::<KernelEvent>(1024);
        let req = ExecuteRequest {
            run: id.clone(),
            code,
            mode,
            cwd: prologue.then_some(cwd),
            env,
            timeout,
        };
        if let Err(e) = kernel.execute(req, sink).await {
            self.idle(key, None, false).await;
            self.end_unstarted(&id, RunState::Failed, Some(e.to_string()))
                .await;
            return;
        }
        let watchdog = timeout.map(|t| {
            let me = self.clone();
            let id = id.clone();
            let k = Arc::clone(&kernel);
            tokio::spawn(async move { me.watchdog(id, k, t).await })
        });
        let mut outputs: Vec<CellOutput> = Vec::new();
        let mut log_from = None;
        let (exit, log_to, died) = loop {
            match events.recv().await {
                Some(KernelEvent::Started { log_from: lf }) => {
                    log_from = lf;
                    self.started(key, &id, lf).await;
                }
                Some(KernelEvent::Bytes(b)) => self.live_bytes(&id, &b),
                Some(KernelEvent::Output(o)) => outputs.push(o),
                Some(KernelEvent::InputRequest { prompt, password }) => {
                    self.waiting_input(&id, crate::types::InputPrompt { prompt, password })
                        .await;
                }
                Some(KernelEvent::Mark(_)) => {}
                Some(KernelEvent::Finished {
                    exit,
                    log_to,
                    kernel_died,
                }) => break (exit, log_to, kernel_died),
                None => break (None, None, true),
            }
        };
        if let Some(w) = watchdog {
            w.abort();
        }
        self.idle(key, Some(cell), died).await;
        let log = self
            .inner
            .state
            .lock()
            .kernels
            .get(key)
            .and_then(|s| s.log.clone());
        let Some(rec) = self
            .finish(&id, exit, log.as_deref(), log_from, log_to, outputs)
            .await
        else {
            return;
        };
        if rec.pass.is_none() {
            self.deliver_run(&rec, false, None).await;
        }
    }

    /// The slot's kernel, started when absent or dead (§5.3), and the previous run's cell.
    async fn ensure_kernel(&self, key: &str) -> Result<(Arc<dyn Kernel>, Option<CellRef>)> {
        let (id, existing) = {
            let s = self.inner.state.lock();
            let slot = s
                .kernels
                .get(key)
                .ok_or_else(|| CellsError::NotFound("kernel".into()))?;
            let live = slot
                .kernel
                .clone()
                .filter(|k| !matches!(k.phase(), KernelPhase::Dead | KernelPhase::Closed));
            (slot.id.clone(), live.map(|k| (k, slot.prev_cell.clone())))
        };
        if let Some(k) = existing {
            return Ok(k);
        }
        let env = self.login_env().await;
        let owner = match &id.scope {
            Scope::Session(s) => *s,
            Scope::Doc(d) => note::kernel_session(d),
        };
        let ctx = KernelCtx {
            id: id.clone(),
            owner,
            login_env: Arc::clone(&env.vars),
            paths: self.inner.paths.clone(),
            config: Arc::clone(&self.inner.config),
            jobs: ActorRef::<tools::Jobs>::lookup(tools::JOBS_ACTOR)
                .ok()
                .flatten(),
            root: self.inner.rt.data_dir.clone(),
        };
        let (kernel, log): (Box<dyn Kernel>, Option<PathBuf>) = match &id.spec {
            KernelSpec::Shell { .. } => {
                let (k, log) = crate::shell::launch(&id.spec, ctx).await?;
                (k, Some(log))
            }
            spec => (crate::kernel::start_kernel(spec, ctx).await?, None),
        };
        let kernel: Arc<dyn Kernel> = Arc::from(kernel);
        if let Some(slot) = self.inner.state.lock().kernels.get_mut(key) {
            slot.kernel = Some(Arc::clone(&kernel));
            slot.log = log;
            slot.prev_cell = None;
        }
        self.publish(CellEvent::KernelChanged { kernel: id }).await;
        Ok((kernel, None))
    }

    /// The run left the kernel: remember its cell for the CWD rule; a dead kernel restarts
    /// on the next run ("kernel exited — the next run starts a new one").
    async fn idle(&self, key: &str, cell: Option<CellRef>, died: bool) {
        let id = {
            let mut s = self.inner.state.lock();
            let Some(slot) = s.kernels.get_mut(key) else {
                return;
            };
            slot.busy = false;
            slot.last_used = Instant::now();
            if cell.is_some() {
                slot.prev_cell = cell;
            }
            if died {
                slot.kernel = None;
                slot.prev_cell = None;
            }
            slot.id.clone()
        };
        self.publish(CellEvent::KernelChanged { kernel: id }).await;
    }

    async fn started(&self, key: &str, id: &RunId, log_from: Option<u64>) {
        let at = now_ms();
        let (scope, log) = {
            let mut s = self.inner.state.lock();
            let log = s.kernels.get(key).and_then(|k| k.log.clone());
            let Some(l) = s.runs.get_mut(id) else {
                return;
            };
            l.record.state = RunState::Running;
            l.record.started_ms = Some(at);
            l.buf.clear();
            (l.record.cell.scope(), log)
        };
        if let Ok(store) = self.store() {
            let log = log.map(|p| p.to_string_lossy().into_owned());
            let _ = store.started(id, at, log.as_deref(), log_from).await;
        }
        self.publish(CellEvent::RunStarted {
            run: id.clone(),
            scope,
        })
        .await;
    }

    /// L11 / §7.4: the kernel waits on `input()`; the run shows the prompt until answered.
    async fn waiting_input(&self, id: &RunId, prompt: crate::types::InputPrompt) {
        let scope = {
            let mut s = self.inner.state.lock();
            s.runs.get_mut(id).map(|l| {
                l.record.input = Some(prompt);
                l.record.cell.scope()
            })
        };
        if let Some(scope) = scope {
            self.publish(CellEvent::CellsChanged { scope }).await;
        }
    }

    fn live_bytes(&self, id: &RunId, b: &[u8]) {
        let mut s = self.inner.state.lock();
        if let Some(l) = s.runs.get_mut(id) {
            l.buf.extend_from_slice(b);
            if l.buf.len() > ATTACH_BUF {
                let over = l.buf.len() - ATTACH_BUF;
                l.buf.drain(..over);
            }
            let _ = l.tx.send(Arc::from(b));
        }
    }

    /// §7.7: `timeout:` → Stop, Kill `kill_after_secs` later, then close the kernel.
    async fn watchdog(self, id: RunId, kernel: Arc<dyn Kernel>, after: Duration) {
        tokio::time::sleep(after).await;
        let grace = Duration::from_secs(self.inner.config.kill_after_secs.max(1));
        if !self.mark_stop(&id, Stop::Timeout) {
            return;
        }
        let _ = kernel.interrupt().await;
        tokio::time::sleep(grace).await;
        if !self.is_live(&id) {
            return;
        }
        let _ = kernel.kill().await;
        tokio::time::sleep(grace).await;
        if self.is_live(&id) {
            let _ = kernel.shutdown().await;
        }
    }

    fn is_live(&self, id: &RunId) -> bool {
        self.inner.state.lock().runs.contains_key(id)
    }

    /// Record how a live run is being stopped (`stop_after`); `false` when the run is no
    /// longer live.
    fn mark_stop(&self, id: &RunId, how: Stop) -> bool {
        let mut s = self.inner.state.lock();
        let Some(l) = s.runs.get_mut(id) else {
            return false;
        };
        l.stop = stop_after(l.stop, how);
        true
    }

    /// A run that never reached its kernel: `cancelled` or `failed` (with the reason as its
    /// summary).
    async fn end_unstarted(&self, id: &RunId, state: RunState, why: Option<String>) {
        let Some(mut l) = self.take_live(id) else {
            return;
        };
        l.record.state = state;
        l.record.summary = why;
        l.record.ended_ms = Some(now_ms());
        let rec = l.record.clone();
        if let Ok(store) = self.store() {
            let _ = store.finish(&rec, None, None).await;
        }
        self.publish(CellEvent::RunFinished {
            run: id.clone(),
            scope: rec.cell.scope(),
        })
        .await;
        for d in l.done {
            let _ = d.send(rec.clone());
        }
        if state == RunState::Failed && rec.pass.is_none() {
            self.deliver_run(&rec, false, None).await;
        }
    }

    fn take_live(&self, id: &RunId) -> Option<Live> {
        let mut s = self.inner.state.lock();
        let l = s.runs.remove(id)?;
        let ck = json(&l.record.cell);
        if s.by_cell.get(&ck) == Some(id) {
            s.by_cell.remove(&ck);
        }
        Some(l)
    }

    /// The run's end (§8.3, §8.7): files, then the finished row, then `RunFinished`.
    async fn finish(
        &self,
        id: &RunId,
        exit: Option<ExitStatus>,
        log: Option<&Path>,
        log_from: Option<u64>,
        log_to: Option<u64>,
        outputs: Vec<CellOutput>,
    ) -> Option<CellRun> {
        if self.inner.shutting_down.load(Ordering::SeqCst) {
            return None; // the next start settles it `lost` (109 R1)
        }
        let l = self.take_live(id)?;
        let mut rec = l.record;
        let dir = self.inner.paths.run_dir(id);
        let cfg = &self.inner.config;
        let (ansi, text) = if outputs.is_empty() {
            let bytes = match (log, log_from) {
                (Some(p), Some(from)) => read_range(p, from, log_to),
                _ => Vec::new(),
            };
            let _ = std::fs::write(dir.join("term.log"), &bytes);
            let r =
                crate::render::render(&bytes, cfg.pty_cols, cfg.pty_rows, cfg.render_scrollback);
            (r.ansi, r.text)
        } else {
            let _ = std::fs::write(dir.join("outputs.json"), json(&outputs));
            let t = outputs_text(&outputs);
            (t.clone(), t)
        };
        // Vault values exactly over the whole text (§9.4 step 1); credentials stay in the
        // user's own view and are masked on every agent-bound copy.
        let ansi = tools::secrets::redact(&ansi).into_owned();
        let text = tools::secrets::redact(&text).into_owned();
        let _ = std::fs::write(dir.join("out.ansi"), &ansi);
        let _ = std::fs::write(dir.join("out.txt"), &text);
        let items = l.meta.expect.clone();
        rec.expect = crate::run::check_expect(&text, &items);
        rec.summary = crate::run::summary(&text, &rec.expect);
        rec.exit = exit;
        rec.ended_ms = Some(now_ms());
        rec.secs = rec
            .started_ms
            .map(|s| (rec.ended_ms.unwrap_or(s) - s).max(0) as f64 / 1000.0);
        rec.output_bytes = Some(text.len() as u64);
        rec.masked = crate::redact::mask(&text, &[]).1;
        let ok = matches!(rec.exit, Some(ExitStatus::Code(0))) && rec.expect.iter().all(|e| e.met);
        rec.state = match l.stop {
            Some(Stop::Timeout) => RunState::Timeout,
            Some(Stop::Killed) => RunState::Killed,
            Some(Stop::Stopped) => RunState::Stopped,
            None if ok => RunState::Done,
            None => RunState::Failed,
        };
        if let Ok(store) = self.store()
            && let Err(e) = store.finish(&rec, Some(&sha(&text)), log_to).await
        {
            tracing::warn!(run = %id, error = %e, "cells: finished row not written");
        }
        self.publish(CellEvent::RunFinished {
            run: id.clone(),
            scope: rec.cell.scope(),
        })
        .await;
        for d in l.done {
            let _ = d.send(rec.clone());
        }
        Some(rec)
    }

    fn meta(&self, id: &RunId) -> Meta {
        std::fs::read_to_string(self.inner.paths.run_dir(id).join("meta.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    // ---- delivery (§9.1, §9.3) ----

    /// Build and hand over the `<cell_result>` of a finished run. `force_wake`: Ask theuth
    /// (`Some(true)`) or a pass end (`Some(review)`); else the §9.3 rule.
    async fn deliver_run(&self, rec: &CellRun, pass_end: bool, force_wake: Option<bool>) {
        if rec.state == RunState::Cancelled {
            if let Ok(s) = self.store() {
                let _ = s.delivery(&rec.id, "none").await;
            }
            return;
        }
        let dir = self.inner.paths.run_dir(&rec.id);
        let meta = self.meta(&rec.id);
        let text = std::fs::read_to_string(dir.join("out.txt")).unwrap_or_default();
        let (cleaned, _) = crate::redact::mask(&text, &[]);
        let outputs: Vec<CellOutput> = std::fs::read_to_string(dir.join("outputs.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let outputs: Vec<CellOutput> = outputs.into_iter().map(mask_output).collect();
        match &rec.cell {
            CellRef::Chat { session, .. } => {
                let env = self.login_env().await;
                let x = crate::deliver::Extra {
                    name: meta.name.as_deref(),
                    private: rec.private || meta.private,
                    edited_code: meta.edited_code.as_deref(),
                    outputs: &outputs,
                    pass_end,
                    home: env.vars.get("HOME").map(String::as_str),
                };
                let (attrs, body) = crate::deliver::parts(rec, &cleaned, &self.inner.config, &x);
                let wrapped = format!("<cell_result {attrs}>\n{body}</cell_result>");
                let running = turn_running(*session).await;
                let (wake, how) = wake_rule(running, meta.review, force_wake);
                let kind = format!(
                    "cell {}",
                    meta.name.clone().unwrap_or_else(|| match &rec.cell {
                        CellRef::Chat { ordinal, .. } => ordinal.to_string(),
                        CellRef::Note { .. } => "note".into(),
                    })
                );
                match crate::deliver::send(*session, kind, body, wrapped, wake).await {
                    Ok(()) => {
                        if let Ok(s) = self.store() {
                            let _ = s.delivery(&rec.id, how).await;
                        }
                    }
                    Err(e) => tracing::warn!(run = %rec.id, error = %e, "cells: not delivered"),
                }
            }
            CellRef::Note { .. } => {
                if let Err(e) = note::deliver(rec, &cleaned).await {
                    // 171:E2
                    tracing::warn!(run = %rec.id, error = %e, "cells: note result not delivered");
                }
            }
        }
    }

    // ---- boot (§5.3, §9.3) ----

    async fn boot(&self) {
        let me = self.clone();
        tokio::spawn(async move { me.sweeps().await });
        let Ok(store) = self.store() else {
            return;
        };
        // Agents first: a lost result goes to its session.
        if self
            .inner
            .rt
            .lookup_when::<agent::Agents>(agent::AGENT_ACTOR)
            .await
            .is_none()
        {
            return;
        }
        // Parked results survive a restart: park them again (§9.3 W-m) — read before the
        // sweep below parks the lost ones, so nothing goes twice.
        let parked = store.parked(None).await.unwrap_or_default();
        self.settle().await;
        for row in parked {
            self.deliver_run(&row.run, false, Some(false)).await;
        }
    }

    /// §5.3 / inv. 11: rows a previous engine left `queued` end `cancelled`, `running` ones
    /// end `lost` with what their log holds as the last screen; delivered by the §9.3 rule
    /// (parked unless `review`); nothing is ever re-run.
    async fn settle(&self) {
        let Ok(store) = self.store() else {
            return;
        };
        let rows = store.unsettled().await.unwrap_or_default();
        for row in rows {
            let mut rec = row.run;
            if self.is_live(&rec.id) {
                continue; // pressed on this engine before the sweep ran
            }
            rec.ended_ms = Some(now_ms());
            if rec.state == RunState::Queued {
                rec.state = RunState::Cancelled;
                let _ = store.finish(&rec, None, None).await;
                let _ = store.delivery(&rec.id, "none").await;
                continue;
            }
            let dir = self.inner.paths.run_dir(&rec.id);
            let bytes = match (&row.log, row.log_from) {
                (Some(p), Some(from)) => read_range(Path::new(p), from.max(0) as u64, None),
                _ => Vec::new(),
            };
            let cfg = &self.inner.config;
            let r =
                crate::render::render(&bytes, cfg.pty_cols, cfg.pty_rows, cfg.render_scrollback);
            let text = tools::secrets::redact(&r.text).into_owned();
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(dir.join("term.log"), &bytes);
            let _ = std::fs::write(
                dir.join("out.ansi"),
                tools::secrets::redact(&r.ansi).as_ref(),
            );
            let _ = std::fs::write(dir.join("out.txt"), &text);
            rec.state = RunState::Lost;
            rec.secs = rec
                .started_ms
                .map(|s| (rec.ended_ms.unwrap_or(s) - s).max(0) as f64 / 1000.0);
            rec.output_bytes = Some(text.len() as u64);
            rec.masked = crate::redact::mask(&text, &[]).1;
            let _ = store.finish(&rec, Some(&sha(&text)), None).await;
            self.publish(CellEvent::RunFinished {
                run: rec.id.clone(),
                scope: rec.cell.scope(),
            })
            .await;
            self.deliver_run(&rec, false, None).await;
        }
    }

    /// Idle kernels close after `kernel_idle_hours`; old runs go per §8.4.
    async fn sweeps(self) {
        let mut tick = tokio::time::interval(Duration::from_secs(600));
        let mut n: u64 = 0;
        loop {
            tokio::select! {
                _ = tick.tick() => {}
                () = self.inner.rt.shutdown.cancelled() => return,
            }
            let idle = Duration::from_secs(self.inner.config.kernel_idle_hours.max(1) * 3600);
            let closing: Vec<(KernelId, Arc<dyn Kernel>)> = {
                let mut s = self.inner.state.lock();
                let ids: Vec<String> = s
                    .kernels
                    .iter()
                    .filter(|(_, k)| !k.busy && k.kernel.is_some() && k.last_used.elapsed() > idle)
                    .map(|(key, _)| key.clone())
                    .filter(|key| !s.runs.values().any(|l| &l.key == key))
                    .collect();
                ids.iter()
                    .filter_map(|key| s.kernels.remove(key))
                    .filter_map(|k| k.kernel.map(|kernel| (k.id, kernel)))
                    .collect()
            };
            for (id, k) in closing {
                let _ = k.shutdown().await;
                self.publish(CellEvent::KernelChanged { kernel: id }).await;
            }
            if n.is_multiple_of(6) {
                self.retention().await;
            }
            n += 1;
        }
    }

    async fn retention(&self) {
        let Ok(store) = self.store() else {
            return;
        };
        let paths = self.inner.paths.clone();
        let size = move |id: &RunId| dir_size(&paths.run_dir(id));
        let cfg = &self.inner.config;
        match store
            .expired(
                now_ms(),
                cfg.retention_days,
                &size,
                cfg.store_max_mb * 1024 * 1024,
            )
            .await
        {
            Ok(gone) => {
                for id in gone {
                    let _ = std::fs::remove_dir_all(self.inner.paths.run_dir(&id));
                }
            }
            Err(e) => tracing::warn!(error = %e, "cells retention"),
        }
    }

    // ---- the frozen method set ----

    /// `cell_run_all`: the engine pass (§7.2); returns the pass id.
    pub async fn run_all(&self, req: RunAllRequest) -> Result<String, CellsError> {
        self.store()?;
        let pass = req.pass.clone();
        let me = self.clone();
        tokio::spawn(async move { me.drive_pass(req).await });
        Ok(pass)
    }

    /// The pass: cells in order as `cell` runs; stops BEFORE danger, a blocking lint chip,
    /// an invalid cell, a missing cwd or an unarmed `after`; stops AT anything but success;
    /// skips superseded cells (§7.2, inv. 7, 12). Survives the client going away.
    async fn drive_pass(self, req: RunAllRequest) {
        let scope = json(&req.scope);
        if self.inner.state.lock().last_pass.remove(&scope).is_some() {
            self.publish(CellEvent::CellsChanged {
                scope: req.scope.clone(),
            })
            .await;
        }
        let mut pending: Option<CellRun> = None;
        let mut review = false;
        let mut ran = 0u32;
        let mut stopped = None;
        let stop = |cell: &CellRef, why: String, before: bool| {
            Some(PassStop {
                cell: cell.clone(),
                why,
                before,
            })
        };
        for (cell, seen) in &req.cells {
            if self.inner.state.lock().stopped_passes.contains(&req.pass) {
                stopped = stop(cell, "stopped by you".into(), true);
                break;
            }
            // Superseded cells are skipped, not a boundary.
            if let CellRef::Chat { session, .. } = cell
                && let Ok(t) = chat::transcript(*session).await
            {
                let env = self.login_env().await;
                let home = env.vars.get("HOME").map(String::as_str);
                if Self::inspect(&t, cell, home).is_some_and(|i| i.superseded_by.is_some()) {
                    continue;
                }
            }
            let p = match self.prepare(cell, seen, &RunMode::Cell, None, None).await {
                Ok(p) => p,
                Err(e) => {
                    stopped = stop(cell, e.to_string(), true);
                    break;
                }
            };
            let c = &p.inspected.presented.cell;
            if let Some(why) = pass_boundary(c, &p.inspected.chips) {
                stopped = stop(cell, why, true);
                break;
            }
            review |= c.options.review;
            let id = RunId(format!("r_{}", crockford(uuid::Uuid::now_v7().as_u128())));
            let rec = match self
                .enqueue(
                    id.clone(),
                    cell.clone(),
                    RunMode::Cell,
                    Some(req.pass.clone()),
                    None,
                    seen.clone(),
                    req.pressed_by.clone(),
                    p,
                )
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    stopped = stop(cell, e.to_string(), true);
                    break;
                }
            };
            if let Some(prev) = pending.take() {
                self.deliver_run(&prev, false, Some(false)).await;
            }
            let (tx, rx) = oneshot::channel();
            let waited = {
                let mut s = self.inner.state.lock();
                match s.runs.get_mut(&rec.id) {
                    Some(l) => {
                        l.done.push(tx);
                        true
                    }
                    None => false,
                }
            };
            let done = if waited {
                rx.await.ok()
            } else {
                // Finished before we could wait: read the record.
                self.find_run(&rec.id).await.ok().flatten()
            };
            let Some(done) = done else {
                stopped = stop(cell, "the run was lost".into(), false);
                break;
            };
            ran += 1;
            let ok = done.state == RunState::Done;
            if !ok {
                stopped = stop(cell, ran_why(&done), false);
            }
            pending = Some(done);
            if !ok {
                break;
            }
        }
        {
            let mut s = self.inner.state.lock();
            s.stopped_passes.remove(&req.pass);
            s.last_pass.insert(
                scope,
                PassEnd {
                    pass: req.pass.clone(),
                    total: req.cells.len() as u32,
                    ran,
                    stopped,
                },
            );
        }
        self.publish(CellEvent::CellsChanged { scope: req.scope }).await;
        if let Some(last) = pending {
            self.deliver_run(&last, true, Some(review)).await;
        }
    }

    /// `cell_stop`: Stop (or Kill) a run; a queued run is cancelled.
    pub async fn stop(&self, run: RunId, kill: bool) -> Result<(), CellsError> {
        let (executing, key, pass) = {
            let s = self.inner.state.lock();
            let Some(l) = s.runs.get(&run) else {
                return match self.inner.store.as_ref() {
                    Ok(_) => Ok(()), // already finished: Stop is a no-op
                    Err(e) => Err(CellsError::Failed(e.clone())),
                };
            };
            (l.executing, l.key.clone(), l.record.pass.clone())
        };
        if let Some(p) = pass {
            self.inner.state.lock().stopped_passes.insert(p);
        }
        if !executing {
            self.end_unstarted(&run, RunState::Cancelled, None).await;
            return Ok(());
        }
        self.mark_stop(&run, if kill { Stop::Killed } else { Stop::Stopped });
        let kernel = self
            .inner
            .state
            .lock()
            .kernels
            .get(&key)
            .and_then(|k| k.kernel.clone());
        let Some(kernel) = kernel else {
            return Ok(()); // still starting: the worker sees the stop before executing
        };
        if kill {
            kernel.kill().await
        } else {
            kernel.interrupt().await
        }
    }

    /// `cell_input`: bytes typed into the run's live terminal / its `input()` answer.
    pub async fn input(&self, run: RunId, data: Vec<u8>) -> Result<(), CellsError> {
        let key = self
            .inner
            .state
            .lock()
            .runs
            .get(&run)
            .map(|l| l.key.clone())
            .ok_or_else(|| CellsError::NotFound(run.0.clone()))?;
        let kernel = self
            .inner
            .state
            .lock()
            .kernels
            .get(&key)
            .and_then(|k| k.kernel.clone())
            .ok_or_else(|| CellsError::NotFound(run.0.clone()))?;
        let keys = data.len() as u64;
        kernel.input(&run, data).await?;
        // L11: an `input()` answered — the prompt is gone (python: one answer per request).
        let answered = {
            let mut s = self.inner.state.lock();
            s.runs
                .get_mut(&run)
                .and_then(|l| l.record.input.take().map(|_| l.record.cell.scope()))
        };
        if let Some(scope) = answered {
            self.publish(CellEvent::CellsChanged { scope }).await;
        }
        // Inv. 13: counts per device, never contents.
        if let Ok(s) = self.store() {
            let _ = s.typed(&run, "owner", keys).await;
        }
        Ok(())
    }

    /// `cell_ask`: wake the session with that run's result (un-park it).
    pub async fn ask(&self, run: RunId) -> Result<(), CellsError> {
        let row = self
            .store()?
            .get(&run)
            .await?
            .ok_or_else(|| CellsError::NotFound(run.0.clone()))?;
        if row.run.ended_ms.is_none() {
            return Err(CellsError::Failed("the run has not finished".into()));
        }
        self.deliver_run(&row.run, false, Some(true)).await;
        self.store()?.consume_one(&run, now_ms()).await
    }

    /// `cell_state`: problems, chips, armed/superseded/stale per cell, and the scope's kernels.
    pub async fn state(&self, scope: Scope, cells: Vec<CellRef>) -> Result<StateReply, CellsError> {
        let env = self.login_env().await;
        let home = env.vars.get("HOME").map(String::as_str);
        let transcript = match &scope {
            Scope::Session(session) => Some(chat::transcript(*session).await?),
            Scope::Doc(_) => None,
        };
        let mut out = Vec::new();
        for c in &cells {
            let (mut i, source) = match (c, &transcript) {
                (CellRef::Chat { .. }, Some(t)) => match Self::inspect(t, c, home) {
                    Some(i) => (i, None),
                    None => continue,
                },
                (CellRef::Note { doc, key }, None) => {
                    match self.inspect_note(c, doc, key, &env).await {
                        Ok((i, s)) => (i, Some(s)),
                        Err(_) => continue,
                    }
                }
                _ => continue,
            };
            self.arm(&scope, &mut i).await?;
            let latest = self.store()?.latest_of_cell(&json(c)).await?.map(|r| r.run);
            let interpreter = match i.cwd.as_deref() {
                Some(cwd) => match self
                    .resolve(&i.presented.cell, cwd, &env, i.defaults.as_ref())
                    .await
                {
                    Ok((_, interp)) => Some(interp),
                    // B19: no program to run it → invalid, with the reason (Run all stops).
                    Err(CellsError::NoInterpreter(m)) => {
                        i.problems.push(Problem {
                            code: "no_interpreter".into(),
                            severity: Severity::Error,
                            message: m,
                            key: None,
                        });
                        None
                    }
                    Err(_) => None,
                },
                None => None,
            };
            // A note's text can change under its runs; a chat reply cannot.
            let stale = source.is_some()
                && latest
                    .as_ref()
                    .is_some_and(|r| r.presented_sha != i.presented.digest);
            let member_edit = source
                .as_ref()
                .and_then(|s| note::member_edit(s, latest.as_ref()));
            out.push(CellState {
                cell: c.clone(),
                digest: i.presented.digest.clone(),
                problems: i.problems,
                chips: i.chips,
                armed: match i.after {
                    Some(Gate::Armed(a)) => Some(a),
                    _ => None,
                },
                superseded_by: i.superseded_by,
                latest,
                stale,
                member_edit,
                interpreter,
                host: None, // the RPC layer names another engine (§14.2)
            });
        }
        Ok(StateReply {
            cells: out,
            kernels: self.kernels_of(&scope),
            pass: self.inner.state.lock().last_pass.get(&json(&scope)).cloned(),
        })
    }

    fn kernels_of(&self, scope: &Scope) -> Vec<KernelState> {
        let s = self.inner.state.lock();
        s.kernels
            .values()
            .filter(|k| &k.id.scope == scope)
            .map(|k| KernelState {
                id: k.id.clone(),
                phase: match &k.kernel {
                    Some(kernel) => kernel.phase(),
                    None if k.busy || s.runs.values().any(|l| l.key == json(&k.id)) => {
                        KernelPhase::Starting
                    }
                    None => KernelPhase::Closed,
                },
                label: k.label.clone(),
                cwd: k.kernel.as_ref().and_then(|kernel| kernel.live_cwd()),
                env_warning: k.env_warning.clone(),
            })
            .collect()
    }

    /// `cell_run_get`: the record and its outputs (text capped).
    pub async fn run_get(&self, run: RunId) -> Result<RunGet, CellsError> {
        // 171:E2 note runs read the same store; nothing note-specific expected here.
        let rec = self
            .find_run(&run)
            .await?
            .ok_or_else(|| CellsError::NotFound(run.0.clone()))?;
        let dir = self.inner.paths.run_dir(&run);
        let outputs = match std::fs::read_to_string(dir.join("outputs.json")) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => match std::fs::read_to_string(dir.join("out.ansi")) {
                Ok(text) => {
                    let lines: Vec<&str> = text.split("\r\n").collect();
                    let keep = self.inner.config.ui_tail_lines as usize;
                    let tail = lines[lines.len().saturating_sub(keep)..].join("\r\n");
                    vec![CellOutput::Stream {
                        name: "stdout".into(),
                        text: tail,
                    }]
                }
                Err(_) => Vec::new(),
            },
        };
        Ok(RunGet { run: rec, outputs })
    }

    /// `cell_output`: a page of the cleaned (or ANSI) text. `ansi: false` is the agent-safe
    /// copy (§9.4: vault-redacted, credential-masked; refused for `private` runs, §9.1);
    /// `ansi: true` is the owner's own terminal view (credentials kept, private allowed).
    pub async fn output(
        &self,
        run: RunId,
        offset: u64,
        limit: u32,
        ansi: bool,
    ) -> Result<OutputPage, CellsError> {
        let Some(rec) = self.find_run(&run).await? else {
            return Err(CellsError::NotFound(run.0));
        };
        let dir = self.inner.paths.run_dir(&run);
        let text = if ansi {
            std::fs::read_to_string(dir.join("out.ansi")).unwrap_or_default()
        } else {
            if rec.private || self.meta(&run).private {
                return Err(CellsError::Private);
            }
            let t = std::fs::read_to_string(dir.join("out.txt")).unwrap_or_default();
            crate::redact::mask(&t, &[]).0
        };
        Ok(page(&text, offset, limit))
    }

    /// `cell_output_file`: one file of `runs/<run>/`: an image, or `code` (the code that ran,
    /// placeholders kept — the stale/member-diff views).
    pub async fn output_file(&self, run: RunId, name: String) -> Result<Vec<u8>, CellsError> {
        let ok = name == "code"
            || (name.starts_with("img-")
                && name.ends_with(".png")
                && !name.contains('/')
                && !name.contains(".."));
        if !ok {
            return Err(CellsError::NotFound(name));
        }
        std::fs::read(self.inner.paths.run_dir(&run).join(&name))
            .map_err(|_| CellsError::NotFound(format!("{run}/{name}")))
    }

    /// `cell_runs`: the scope's runs, newest first.
    pub async fn runs(&self, scope: Scope, limit: u32) -> Result<Vec<CellRun>, CellsError> {
        Ok(self
            .store()?
            .runs(&scope.key(), limit)
            .await?
            .into_iter()
            .map(|r| r.run)
            .collect())
    }

    /// `cell_interpreters`: `interpreters(cwd)` on this engine (the RPC routes `engine`).
    pub async fn interpreters(&self, cwd: String) -> Result<Vec<Interpreter>, CellsError> {
        let env = self.login_env().await;
        Ok(crate::interp::interpreters(Path::new(&cwd), &env.vars).await) // 171:E2
    }

    /// `cell_kernel_restart`: Close now; the next run starts a fresh kernel.
    pub async fn kernel_restart(&self, kernel: KernelId) -> Result<(), CellsError> {
        self.close_kernel(&kernel, false).await
    }

    /// `cell_kernel_close`: Stop any run, kill the process group, release the PTY.
    pub async fn kernel_close(&self, kernel: KernelId) -> Result<(), CellsError> {
        self.close_kernel(&kernel, true).await
    }

    async fn close_kernel(&self, id: &KernelId, remove: bool) -> Result<()> {
        let key = json(id);
        let (queued, running, kernel) = {
            let mut s = self.inner.state.lock();
            let queued: Vec<RunId> = s
                .runs
                .iter()
                .filter(|(_, l)| l.key == key && !l.executing)
                .map(|(id, _)| id.clone())
                .collect();
            let running: Vec<RunId> = s
                .runs
                .iter()
                .filter(|(_, l)| l.key == key && l.executing)
                .map(|(id, _)| id.clone())
                .collect();
            let kernel = if remove {
                s.kernels.remove(&key).and_then(|k| k.kernel)
            } else {
                s.kernels.get_mut(&key).and_then(|k| {
                    k.prev_cell = None;
                    k.kernel.take()
                })
            };
            (queued, running, kernel)
        };
        for r in queued {
            self.end_unstarted(&r, RunState::Cancelled, None).await;
        }
        for r in running {
            self.mark_stop(&r, Stop::Killed);
        }
        if let Some(k) = kernel {
            k.shutdown().await?;
        } else if remove {
            return Err(CellsError::NotFound("no such kernel".into()));
        }
        self.publish(CellEvent::KernelChanged { kernel: id.clone() })
            .await;
        Ok(())
    }

    /// `cell_env_refresh`: re-capture the login environment.
    pub async fn env_refresh(&self) -> Result<(), CellsError> {
        let e = capture_env(self.inner.config.login_env_timeout_secs).await;
        *self.inner.env.lock().await = Some(e);
        Ok(())
    }

    /// `cell_live`: runs in flight on this engine (quit / update sheet).
    pub async fn live(&self) -> Result<Vec<CellRun>, CellsError> {
        let s = self.inner.state.lock();
        let mut v: Vec<CellRun> = s.runs.values().map(|l| l.record.clone()).collect();
        v.sort_by_key(|r| r.queued_ms);
        Ok(v)
    }

    /// The scope of a run this engine knows (in flight or stored); the RPC layer routes
    /// Stop/input/Ask by it.
    pub async fn run_scope(&self, run: &RunId) -> Option<Scope> {
        self.find_run(run)
            .await
            .ok()
            .flatten()
            .map(|r| r.cell.scope())
    }

    /// `cell_latest`: the replicated latest runs of a note (§13.2).
    pub async fn latest(&self, doc: DocId) -> Result<Vec<CellLatest>, CellsError> {
        note::latest(&doc).await // 171:E2
    }

    /// `cell_attach`: the run's live terminal from mark C: a `reset` chunk, then `data`, then
    /// `exit`.
    pub async fn attach(&self, run: RunId) -> Result<mpsc::Receiver<PtyChunk>, CellsError> {
        let (rows, cols) = (self.inner.config.pty_rows, self.inner.config.pty_cols);
        let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
        let (tx, rx) = mpsc::channel(256);
        let live = {
            let s = self.inner.state.lock();
            s.runs.get(&run).map(|l| (l.buf.clone(), l.tx.subscribe()))
        };
        match live {
            Some((buf, mut brx)) => {
                let _ = tx
                    .send(PtyChunk::Reset {
                        data: b64(&buf),
                        rows,
                        cols,
                    })
                    .await;
                let me = self.clone();
                tokio::spawn(async move {
                    loop {
                        match brx.recv().await {
                            Ok(b) => {
                                if tx.send(PtyChunk::Data { data: b64(&b) }).await.is_err() {
                                    return;
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(_)) => {}
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                    }
                    let exit = match me.find_run(&run).await {
                        Ok(Some(r)) => job_exit(&r),
                        _ => tools::jobs::JobExit::Lost,
                    };
                    let _ = tx.send(PtyChunk::Exit { exit }).await;
                });
            }
            None => {
                let rec = self
                    .find_run(&run)
                    .await?
                    .ok_or_else(|| CellsError::NotFound(run.0.clone()))?;
                let bytes = std::fs::read(self.inner.paths.run_dir(&run).join("term.log"))
                    .unwrap_or_default();
                let tail = &bytes[bytes.len().saturating_sub(ATTACH_BUF)..];
                let _ = tx
                    .send(PtyChunk::Reset {
                        data: b64(tail),
                        rows,
                        cols,
                    })
                    .await;
                let _ = tx
                    .send(PtyChunk::Exit {
                        exit: job_exit(&rec),
                    })
                    .await;
            }
        }
        Ok(rx)
    }

    /// `cell_detach`: drop the attachment (also implied when the receiver is dropped).
    pub async fn detach(&self, run: RunId) -> Result<(), CellsError> {
        let _ = run;
        Ok(())
    }
}

/// §9.3: a turn is in flight (running, awaiting a reply or an approval).
async fn turn_running(session: SessionId) -> bool {
    let Some(agents) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
        .ok()
        .flatten()
    else {
        return false;
    };
    match agents.ask(agent::ActivityOf(vec![session])).await {
        Ok(map) => map.get(&session).is_some_and(|a| {
            matches!(
                a.state,
                agent::AgentState::Running
                    | agent::AgentState::Awaiting { .. }
                    | agent::AgentState::Approval { .. }
            )
        }),
        Err(_) => false,
    }
}

/// The run's end as a PTY exit (`cell_attach`'s last chunk).
fn job_exit(r: &CellRun) -> tools::jobs::JobExit {
    use tools::jobs::JobExit;
    match (r.state, &r.exit) {
        (RunState::Timeout, _) => JobExit::Timeout,
        (RunState::Lost, _) => JobExit::Lost,
        (RunState::Killed | RunState::Stopped | RunState::Cancelled, _) => JobExit::Killed,
        (_, Some(ExitStatus::Code(c))) => JobExit::Exited(u8::try_from(*c).unwrap_or(1)),
        _ => JobExit::Exited(1),
    }
}

/// Credential masks inside one python/Monty output (agent copy).
fn mask_output(o: CellOutput) -> CellOutput {
    let m = |s: &str| crate::redact::mask(s, &[]).0;
    match o {
        CellOutput::Stream { name, text } => CellOutput::Stream {
            name,
            text: m(&text),
        },
        CellOutput::Error {
            ename,
            evalue,
            traceback,
        } => CellOutput::Error {
            ename,
            evalue: m(&evalue),
            traceback: traceback.iter().map(|t| m(t)).collect(),
        },
        CellOutput::DisplayData { mut data, metadata } => {
            mask_bundle(&mut data);
            CellOutput::DisplayData { data, metadata }
        }
        CellOutput::ExecuteResult {
            mut data,
            metadata,
            count,
        } => {
            mask_bundle(&mut data);
            CellOutput::ExecuteResult {
                data,
                metadata,
                count,
            }
        }
    }
}

fn mask_bundle(data: &mut BTreeMap<String, serde_json::Value>) {
    if let Some(serde_json::Value::String(s)) = data.get_mut("text/plain") {
        *s = crate::redact::mask(s, &[]).0;
    }
}

/// Plain text of python/Monty outputs (the record's text, `expect`'s input).
fn outputs_text(outputs: &[CellOutput]) -> String {
    let mut out = String::new();
    for o in outputs {
        match o {
            CellOutput::Stream { text, .. } => out.push_str(text),
            CellOutput::DisplayData { data, .. } | CellOutput::ExecuteResult { data, .. } => {
                if let Some(serde_json::Value::String(s)) = data.get("text/plain") {
                    out.push_str(s);
                    out.push('\n');
                }
            }
            CellOutput::Error { ename, evalue, .. } => {
                out.push_str(&format!("{ename}: {evalue}\n"));
            }
        }
    }
    out
}

/// One `cell_output` page, cut on char boundaries.
fn page(text: &str, offset: u64, limit: u32) -> OutputPage {
    let total = text.len() as u64;
    let mut start = (offset.min(total)) as usize;
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    let mut end = start.saturating_add(limit.max(1) as usize).min(text.len());
    while end < text.len() && !text.is_char_boundary(end) {
        end += 1;
    }
    OutputPage {
        text: text[start..end].to_owned(),
        offset: start as u64,
        total,
        next: (end < text.len()).then_some(end as u64),
    }
}

/// `[from, to)` of a log file (`to = None`: to the end).
fn read_range(path: &Path, from: u64, to: Option<u64>) -> Vec<u8> {
    use std::io::{Read as _, Seek as _, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else {
        return Vec::new();
    };
    if f.seek(SeekFrom::Start(from)).is_err() {
        return Vec::new();
    }
    let mut out = Vec::new();
    match to {
        Some(to) => {
            let _ = f.take(to.saturating_sub(from)).read_to_end(&mut out);
        }
        None => {
            let _ = f.read_to_end(&mut out);
        }
    }
    out
}

fn dir_size(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| e.metadata().ok())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0)
}

/// §5.4: the login shell's environment once, with the drop/set rules; on failure the
/// engine's own environment plus the login `PATH`, and the warning the strip shows.
async fn capture_env(timeout_secs: u64) -> Env {
    let shell = std::env::var_os("SHELL")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            if cfg!(target_os = "macos") {
                "/bin/zsh".into()
            } else {
                "/bin/sh".into()
            }
        });
    let t = Duration::from_secs(timeout_secs.max(1));
    let (mut vars, warning) = match platform::env::login_env(&shell, t).await {
        Ok(v) => (v, None),
        Err(why) => {
            let mut v: BTreeMap<String, String> = std::env::vars().collect();
            if let Ok(p) = platform::env::login_path(&shell, t).await {
                v.insert("PATH".into(), p);
            }
            tracing::warn!(%why, "cells: login environment not captured");
            (
                v,
                Some("your shell's environment could not be read — Refresh".to_owned()),
            )
        }
    };
    apply_env_rules(&mut vars);
    Env {
        vars: Arc::new(vars),
        warning,
    }
}

#[cfg(test)]
mod tests;
