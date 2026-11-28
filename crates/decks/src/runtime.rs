//! Monty Python runtime for Decks — reuses `agent::repl` pool, checkout construction,
//! live `ToolRegistry::Invoke` & `ListTools(session)`, full MCP namespace injection,
//! `.monty` persistence + `rebase_dump_stubs`, and `tools::secrets` redaction.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent::repl::{
    DIRECT_ONLY, DUMP_HARD_BYTES, MAX_MEMORY, MontyCheckout, MontyPool, MountSpec, PoolError,
    ReplConfig, ResumeValue, TurnEvent, find_session_tool, from_monty, help, merge_args,
    on_print_sync, os_answer, rebase_dump_stubs, sanitize_ident, session_mounts,
    session_repl_config, session_tool_specs, signature, summary, to_monty, tool_result,
};
use futures::future::join_all;
use kameo::actor::ActorRef;
use monty_types::{CallArgs, ExcType, MontyException, MontyObject, NameLookupResult};
use platform::files::write_atomic;
use runtime::SessionId;
use serde_json::Value;
use tools::{Invoke, ToolCall, ToolOutput, ToolRegistry, ToolSpec};

use crate::hub::{MAX_LIVE_FRAME_BYTES, MAX_SNAPSHOT_BYTES};
use crate::model::{MAX_ACTION_RESULT_BYTES, ScriptRevision};
use crate::store::{StateCheckpoint, sha256_bytes};

/// Maximum concurrent Deck Monty workers in the dedicated Deck pool.
pub const DECK_POOL_MAX: usize = 16;

/// Dump `.monty` heap checkpoint at clean turn boundary every N steps.
pub const CHECKPOINT_EVERY_STEPS: u64 = 5;

/// Roll over to a fresh worker (via dump -> finish -> restore) every N steps so long-running
/// streams never exhaust `max_suspensions` or accumulate worker heap fragmentation.
pub const ROLLOVER_EVERY_STEPS: u64 = 50;

/// Minimum / maximum cooperative `deck_sleep(seconds)` clamp.
pub const MIN_SLEEP_SECS: f64 = 0.1;
pub const MAX_SLEEP_SECS: f64 = 3600.0;

/// Maximum `deck_emit(...)` calls allowed within a single Monty turn (229 H4).
pub const MAX_EMITS_PER_TURN: usize = 16;

/// Maximum byte length for `deck_emit(..., section=...)` after redaction (229 H4).
pub const MAX_SECTION_NAME_BYTES: usize = 64;

/// Maximum captured stdout bytes per Monty turn (UTF-8 boundary-safe, 229 H4).
pub const MAX_STDOUT_BYTES: usize = 16_000;

/// Type-checking `.pyi` stubs appended to the live session tool stubs for Deck cooperative builtins.
pub const DECK_BUILTIN_STUBS: &str = "\
def deck_emit(data: Any, durable: bool = False, section: str | None = None) -> None: ...
def deck_checkpoint(state: Any) -> None: ...
def deck_sleep(seconds: float) -> None: ...
def deck_state() -> Any: ...
def deck_last_snapshot() -> Any: ...
";

/// Tools excluded from Deck script execution:
/// - `DIRECT_ONLY` (`python`, `advise`, `consult`) cannot run inside a Monty REPL cell (229 H1).
/// - Re-entrant `deck_*` and `show_deck` tools are blocked to prevent self-deadlock (229 B4).
///
/// All other builtins, plugins, and configured MCP tools remain exposed automatically.
pub fn is_blocked_deck_tool(name: &str) -> bool {
    DIRECT_ONLY.contains(&name) || name.starts_with("deck_") || name == "show_deck"
}

/// Fetch live session tool specs (including full configured MCP namespace) minus blocked
/// direct-only and re-entrant Deck tools.
pub async fn deck_tool_specs(
    registry: &ActorRef<ToolRegistry>,
    session: SessionId,
) -> Vec<ToolSpec> {
    let mut specs = session_tool_specs(registry, session).await;
    specs.retain(|s| !is_blocked_deck_tool(&s.name));
    specs
}

/// One emission produced by Python calling `deck_emit(data, durable=..., section=...)`.
#[derive(Clone, Debug)]
pub struct EmittedPayload {
    pub data: Value,
    pub durable: bool,
    pub section: Option<String>,
}

/// Side-effects accumulated during one bounded Monty turn.
#[derive(Clone, Debug, Default)]
pub struct TurnEffects {
    pub emits: Vec<EmittedPayload>,
    pub checkpoint_state: Option<Value>,
    pub next_sleep_secs: Option<f64>,
    pub stdout: String,
    pub tool_calls: u32,
}

/// Outcome of one bounded Monty turn.
#[derive(Clone, Debug)]
pub struct TurnOutcome {
    pub return_value: Value,
    pub effects: TurnEffects,
}

/// Read-only context snapshot provided to cooperative builtins during a turn.
#[derive(Clone, Debug)]
pub struct TurnContext {
    pub last_state: Value,
    pub last_snapshot: Value,
}

/// Mode by which a [`DeckCheckout`] was initialized.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartMode {
    /// Restored live Python heap/globals from compatible `.monty` file.
    WarmRestored,
    /// Fresh checkout loaded script source and ran `on_start(last_state)`.
    ColdStarted,
}

/// Single-owner Monty checkout for one Deck.
/// Never shared concurrently: owned exclusively by the Deck's single `DeckOwner` task.
pub struct DeckCheckout {
    pool: Arc<MontyPool>,
    registry: ActorRef<ToolRegistry>,
    session: SessionId,
    script_rev: u64,
    code_sha256: String,
    revision_sha256: String,
    cfg: ReplConfig,
    mounts: Vec<MountSpec>,
    dump_path: PathBuf,
    dump_hard_bytes: u64,
    checkout: Option<MontyCheckout>,
    steps_since_checkpoint: u64,
    steps_since_rollover: u64,
    call_seq: u64,
    last_dump: Option<Vec<u8>>,
    last_dump_sha256: Option<String>,
}

struct PendingTool {
    seq: u32,
    id: String,
    name: String,
    args: Value,
}

impl DeckCheckout {
    pub fn script_rev(&self) -> u64 {
        self.script_rev
    }

    pub fn code_sha256(&self) -> &str {
        &self.code_sha256
    }

    pub fn revision_sha256(&self) -> &str {
        &self.revision_sha256
    }

    /// SHA-256 hex digest of the last `.monty` dump bytes persisted on disk.
    pub fn last_dump_sha256(&self) -> Option<&str> {
        self.last_dump_sha256.as_deref()
    }

    pub fn is_alive(&self) -> bool {
        self.checkout
            .as_ref()
            .is_some_and(|c| c.worker_id().is_some())
    }

    #[cfg(test)]
    pub fn set_dump_hard_bytes_for_test(&mut self, cap: u64) {
        self.dump_hard_bytes = cap;
    }

    #[cfg(test)]
    pub fn force_rollover_for_test(&mut self) {
        self.steps_since_rollover = ROLLOVER_EVERY_STEPS;
    }

    /// Open a Deck checkout for `script`:
    /// - When `allow_warm_restore` is true and the `.monty` file bytes on disk match the
    ///   checkpoint's `(monty_dump_rev, monty_dump_revision_sha256, monty_dump_sha256)`,
    ///   warm-restores `<session>.monty` (rebasing live session/MCP stubs).
    /// - On any mismatch or tampering, deletes the stale/tampered `.monty` file and falls
    ///   back cleanly to cold start (`script.code` + `on_start(last_state)`).
    #[allow(clippy::too_many_arguments)]
    pub async fn open_or_restore(
        pool: Arc<MontyPool>,
        registry: ActorRef<ToolRegistry>,
        session: SessionId,
        cwd: &Path,
        spill_root: &Path,
        dump_path: PathBuf,
        script: &ScriptRevision,
        checkpoint: Option<&StateCheckpoint>,
        allow_warm_restore: bool,
        turn_cx: &TurnContext,
        on_emit: &mut (dyn FnMut(&EmittedPayload) + Send),
    ) -> Result<(Self, StartMode, TurnEffects), String> {
        let specs = deck_tool_specs(&registry, session).await;
        let cfg = session_repl_config(&specs, DECK_BUILTIN_STUBS, MAX_MEMORY);
        let spill = spill_root.join(session.to_string());
        let mounts = session_mounts(cwd, &spill);

        let mut rt = Self {
            pool,
            registry,
            session,
            script_rev: script.rev,
            code_sha256: script.code_sha256.clone(),
            revision_sha256: script.revision_sha256.clone(),
            cfg,
            mounts,
            dump_path,
            dump_hard_bytes: DUMP_HARD_BYTES,
            checkout: None,
            steps_since_checkpoint: 0,
            steps_since_rollover: 0,
            call_seq: 0,
            last_dump: None,
            last_dump_sha256: None,
        };

        // Attempt warm `.monty` restore ONLY when:
        // 1. Caller allows warm restore (false after rollover dump-too-large),
        // 2. Checkpoint matches script rev AND canonical revision_sha256, AND
        // 3. Actual `.monty` file bytes SHA-256 matches checkpoint's monty_dump_sha256 (229 B2).
        let expected_dump_sha = if allow_warm_restore {
            checkpoint.and_then(|cp| {
                let rev_ok = cp.monty_dump_rev == Some(script.rev)
                    && cp.monty_dump_revision_sha256.as_deref()
                        == Some(script.revision_sha256.as_str());
                rev_ok
                    .then_some(cp.monty_dump_sha256.as_deref())
                    .flatten()
                    .filter(|s| !s.is_empty())
            })
        } else {
            None
        };

        if let Some(expected_sha) = expected_dump_sha
            && let Ok(meta) = std::fs::metadata(&rt.dump_path)
            && meta.len() > 0
            && meta.len() <= rt.dump_hard_bytes
            && let Ok(mut bytes) = std::fs::read(&rt.dump_path)
        {
            let actual_sha = sha256_bytes(&bytes);
            if actual_sha != expected_sha {
                tracing::warn!(
                    deck = %script.deck_id,
                    rev = script.rev,
                    expected = expected_sha,
                    actual = %actual_sha,
                    "refusing warm .monty restore: dump file SHA-256 mismatch; falling back to cold start"
                );
                let _ = std::fs::remove_file(&rt.dump_path);
            } else {
                if let Some(fresh_stubs) = rt.cfg.type_check_stubs.as_deref() {
                    rebase_dump_stubs(&mut bytes, None, fresh_stubs);
                }
                match rt.pool.checkout(&rt.cfg).await {
                    Ok(mut c) => {
                        let mut quiet = on_print_sync(|_, _| {});
                        if c.restore(bytes.clone(), Vec::new(), &mut quiet)
                            .await
                            .is_ok()
                        {
                            rt.checkout = Some(c);
                            rt.last_dump = Some(bytes);
                            rt.last_dump_sha256 = Some(actual_sha);
                            return Ok((rt, StartMode::WarmRestored, TurnEffects::default()));
                        }
                        drop(c);
                        tracing::warn!(
                            deck = %script.deck_id,
                            rev = script.rev,
                            "warm .monty restore failed; falling back to cold on_start"
                        );
                    }
                    Err(e) => {
                        tracing::warn!(deck = %script.deck_id, "checkout failed on warm restore: {e}");
                    }
                }
            }
        }

        // Cold start: fresh checkout -> load module code (type-checked!) -> call on_start(last_state).
        let c = rt
            .pool
            .checkout(&rt.cfg)
            .await
            .map_err(|e| format!("monty checkout failed: {e}"))?;
        rt.checkout = Some(c);

        let mut effects = TurnEffects::default();
        // 1. Load & type-check user script module.
        let load_out = rt
            .drive_snippet(&script.code, Vec::new(), false, turn_cx, on_emit)
            .await?;
        merge_effects(&mut effects, load_out.effects);

        // 2. Call optional `on_start(last_state)` with durable JSON last_state.
        let start_snippet = "\
try:
    _deck_start_fn = on_start
except NameError:
    _deck_start_fn = None
if _deck_start_fn is not None:
    await _deck_start_fn(_deck_last_state)
";
        let start_out = rt
            .drive_snippet(
                start_snippet,
                vec![("_deck_last_state".into(), to_monty(&turn_cx.last_state))],
                true,
                turn_cx,
                on_emit,
            )
            .await?;
        merge_effects(&mut effects, start_out.effects);

        // Immediately snapshot initial heap so warm restore works on next restart.
        let _ = rt.checkpoint_heap().await;

        Ok((rt, StartMode::ColdStarted, effects))
    }

    /// Clean-boundary maintenance: refresh tool/MCP stubs if `ListTools(session)` changed,
    /// or roll over worker after `ROLLOVER_EVERY_STEPS`. Preserves Python globals via `.monty` dump!
    /// Never installs an empty uninitialized VM when dump exceeds cap or fails (229 B8): returns
    /// `Err("rollover_dump_too_large")` / `Err("rollover_no_dump")` so `DeckOwner` performs a
    /// clean cold start (`on_start(last_state)`).
    pub async fn maybe_refresh_or_rollover(&mut self) -> Result<bool, String> {
        let specs = deck_tool_specs(&self.registry, self.session).await;
        let fresh_cfg = session_repl_config(&specs, DECK_BUILTIN_STUBS, MAX_MEMORY);
        let stubs_changed = fresh_cfg.type_check_stubs != self.cfg.type_check_stubs;
        let rollover_due = self.steps_since_rollover >= ROLLOVER_EVERY_STEPS;
        let dead = !self.is_alive();
        if !stubs_changed && !rollover_due && !dead {
            return Ok(false);
        }

        let mut dump_too_large = false;
        let mut bytes = if !dead {
            if let Some(c) = self.checkout.as_mut() {
                match c.dump().await {
                    Ok(b) if (b.len() as u64) <= self.dump_hard_bytes => Some(b),
                    Ok(_) => {
                        dump_too_large = true;
                        None
                    }
                    Err(_) => self.last_dump.clone(),
                }
            } else {
                self.last_dump.clone()
            }
        } else {
            self.last_dump.clone()
        };

        // Release old checkout cleanly.
        if let Some(old) = self.checkout.take() {
            let _ = old.finish().await;
        }

        let prev_stubs = self.cfg.type_check_stubs.clone();
        self.cfg = fresh_cfg;

        if dump_too_large {
            self.last_dump = None;
            self.last_dump_sha256 = None;
            let _ = std::fs::remove_file(&self.dump_path);
            return Err("rollover_dump_too_large".into());
        }

        let Some(ref mut dump_bytes) = bytes else {
            self.last_dump = None;
            self.last_dump_sha256 = None;
            return Err("rollover_no_dump".into());
        };

        if let Some(fresh_stubs) = self.cfg.type_check_stubs.as_deref() {
            rebase_dump_stubs(dump_bytes, prev_stubs.as_deref(), fresh_stubs);
        }

        let mut next = self
            .pool
            .checkout(&self.cfg)
            .await
            .map_err(|e| format!("monty rollover checkout failed: {e}"))?;
        let mut quiet = on_print_sync(|_, _| {});
        if next
            .restore(dump_bytes.clone(), Vec::new(), &mut quiet)
            .await
            .is_ok()
        {
            self.last_dump = Some(dump_bytes.clone());
            self.checkout = Some(next);
            self.steps_since_rollover = 0;
            return Ok(true);
        }
        drop(next);
        Err("rollover_restore_failed".into())
    }

    /// Execute one bounded collection / streaming step (`on_step()` or fallback `collect()`).
    pub async fn run_step(
        &mut self,
        turn_cx: &TurnContext,
        on_emit: &mut (dyn FnMut(&EmittedPayload) + Send),
    ) -> Result<TurnOutcome, String> {
        let snippet = "\
_deck_step_fn = None
try:
    _deck_step_fn = on_step
except NameError:
    try:
        _deck_step_fn = collect
    except NameError:
        _deck_step_fn = None
if _deck_step_fn is None:
    raise RuntimeError('Deck script must define async def on_step() or async def collect()')
await _deck_step_fn()
";
        let out = self
            .drive_snippet(snippet, Vec::new(), true, turn_cx, on_emit)
            .await?;
        self.steps_since_checkpoint += 1;
        self.steps_since_rollover += 1;
        if out.effects.checkpoint_state.is_some()
            || self.steps_since_checkpoint >= CHECKPOINT_EVERY_STEPS
        {
            let _ = self.checkpoint_heap().await;
            self.steps_since_checkpoint = 0;
        }
        Ok(out)
    }

    /// Execute one declared interactive action on the SAME checkout (`on_action(action, args, ctx)`).
    pub async fn run_action(
        &mut self,
        action: &str,
        args: &Value,
        ctx: &Value,
        turn_cx: &TurnContext,
        on_emit: &mut (dyn FnMut(&EmittedPayload) + Send),
    ) -> Result<TurnOutcome, String> {
        let snippet = "\
try:
    _deck_action_fn = on_action
except NameError:
    raise RuntimeError('Deck script does not define async def on_action(action, args, ctx)')
await _deck_action_fn(_deck_action_name, _deck_action_args, _deck_action_ctx)
";
        let out = self
            .drive_snippet(
                snippet,
                vec![
                    ("_deck_action_name".into(), MontyObject::string(action)),
                    ("_deck_action_args".into(), to_monty(args)),
                    ("_deck_action_ctx".into(), to_monty(ctx)),
                ],
                true,
                turn_cx,
                on_emit,
            )
            .await?;
        let ret_bytes = serde_json::to_vec(&out.return_value)
            .map(|b| b.len())
            .unwrap_or(usize::MAX);
        if ret_bytes > MAX_ACTION_RESULT_BYTES {
            return Err(format!(
                "action return payload ({ret_bytes} bytes) exceeds cap ({MAX_ACTION_RESULT_BYTES} bytes)"
            ));
        }
        self.steps_since_checkpoint += 1;
        self.steps_since_rollover += 1;
        if out.effects.checkpoint_state.is_some()
            || self.steps_since_checkpoint >= CHECKPOINT_EVERY_STEPS
        {
            let _ = self.checkpoint_heap().await;
            self.steps_since_checkpoint = 0;
        }
        Ok(out)
    }

    /// Persist `.monty` heap snapshot atomically to disk at a clean turn boundary,
    /// returning the SHA-256 hex digest of the written `.monty` bytes (229 B2).
    pub async fn checkpoint_heap(&mut self) -> Result<String, String> {
        let Some(c) = self.checkout.as_mut() else {
            return Err("no active checkout".into());
        };
        if c.worker_id().is_none() {
            return Err("checkout worker is dead".into());
        }
        let bytes = c
            .dump()
            .await
            .map_err(|e| format!("monty dump failed: {e}"))?;
        if (bytes.len() as u64) > self.dump_hard_bytes {
            return Err(format!(
                "deck heap dump ({} bytes) exceeds cap ({})",
                bytes.len(),
                self.dump_hard_bytes
            ));
        }
        if let Some(parent) = self.dump_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let digest = sha256_bytes(&bytes);
        write_atomic(&self.dump_path, &bytes)
            .map_err(|e| format!("write {}: {e}", self.dump_path.display()))?;
        self.last_dump = Some(bytes);
        self.last_dump_sha256 = Some(digest.clone());
        Ok(digest)
    }

    /// Checkpoint and gracefully release the Monty worker back to the pool.
    /// Returns the SHA-256 digest of the saved `.monty` file bytes if checkpointing succeeded.
    pub async fn stop_and_release(mut self) -> Result<Option<String>, String> {
        let digest = self.checkpoint_heap().await.ok();
        if let Some(c) = self.checkout.take() {
            let _ = c.finish().await;
        }
        Ok(digest)
    }

    /// Core turn driver: mirrors `agent::repl::Repl::drive`, wiring full session/MCP tools
    /// through `ToolRegistry::Invoke` + cooperative Deck builtins.
    async fn drive_snippet(
        &mut self,
        code: &str,
        inputs: Vec<(String, MontyObject)>,
        skip_type_check: bool,
        turn_cx: &TurnContext,
        on_emit: &mut (dyn FnMut(&EmittedPayload) + Send),
    ) -> Result<TurnOutcome, String> {
        let checkout = self
            .checkout
            .as_mut()
            .ok_or_else(|| "deck checkout is closed".to_owned())?;
        if checkout.worker_id().is_none() {
            return Err("deck checkout worker died".into());
        }

        let mut effects = TurnEffects::default();
        let stdout_buf = Arc::new(parking_lot::Mutex::new(String::new()));
        let stdout_cap = stdout_buf.clone();
        let mut print_cb = on_print_sync(move |_, text| {
            let mut s = stdout_cap.lock();
            if s.len() < MAX_STDOUT_BYTES {
                let remaining = MAX_STDOUT_BYTES - s.len();
                if text.len() <= remaining {
                    s.push_str(text);
                } else {
                    let mut cut = remaining;
                    while cut > 0 && !text.is_char_boundary(cut) {
                        cut -= 1;
                    }
                    s.push_str(&text[..cut]);
                }
            }
        });

        let mut pending: HashMap<u32, PendingTool> = HashMap::new();
        let mut inner_seq = 0u32;
        self.call_seq += 1;
        let turn_tag = format!("deck.{}.{}", self.session, self.call_seq);

        let mut ev = checkout
            .feed(
                code,
                inputs,
                self.mounts.clone(),
                skip_type_check,
                &mut print_cb,
            )
            .await
            .map_err(format_pool_err)?;

        let result_obj = loop {
            ev = match ev {
                TurnEvent::Complete(v) => break v,
                TurnEvent::NameLookup { name, .. } => {
                    let answer = match name.as_str() {
                        "deck_emit" | "deck_checkpoint" | "deck_sleep" | "deck_state"
                        | "deck_last_snapshot" | "tools" | "help" | "bindings" => {
                            NameLookupResult::Value(MontyObject::function(name, None))
                        }
                        blocked if is_blocked_deck_tool(blocked) => {
                            // Resolve blocked re-entrant/direct-only tools to a callable that
                            // fails with a clear RuntimeError instead of deadlocking (229 B4/H1).
                            NameLookupResult::Value(MontyObject::function(name, None))
                        }
                        _ => {
                            let specs = deck_tool_specs(&self.registry, self.session).await;
                            match find_session_tool(&specs, &name) {
                                Some(spec) => NameLookupResult::Value(MontyObject::function(
                                    name,
                                    Some(summary(&spec.description)),
                                )),
                                None => NameLookupResult::Undefined,
                            }
                        }
                    };
                    checkout
                        .resume_name_lookup(answer, &mut print_cb)
                        .await
                        .map_err(format_pool_err)?
                }
                TurnEvent::FunctionCall {
                    function_name,
                    args,
                    call_id: fid,
                    ..
                } => {
                    let answer = match function_name.as_str() {
                        "deck_emit" => handle_deck_emit(&args, &mut effects, on_emit),
                        "deck_checkpoint" => handle_deck_checkpoint(&args, &mut effects),
                        "deck_sleep" => handle_deck_sleep(&args, &mut effects),
                        "deck_state" => ResumeValue::Return(to_monty(&turn_cx.last_state)),
                        "deck_last_snapshot" => {
                            ResumeValue::Return(to_monty(&turn_cx.last_snapshot))
                        }
                        "bindings" => ResumeValue::Return(MontyObject::list(
                            [
                                "bindings",
                                "deck_checkpoint",
                                "deck_emit",
                                "deck_last_snapshot",
                                "deck_sleep",
                                "deck_state",
                                "help",
                                "tools",
                            ]
                            .into_iter()
                            .map(MontyObject::string),
                        )),
                        "tools" => {
                            let query = first_arg(&args, "query").and_then(|q| q.as_str());
                            let specs = deck_tool_specs(&self.registry, self.session).await;
                            ResumeValue::Return(MontyObject::list(
                                specs
                                    .iter()
                                    .map(signature)
                                    .filter(|line| query.is_none_or(|q| line.contains(q)))
                                    .map(MontyObject::string),
                            ))
                        }
                        "help" => match first_arg(&args, "name").and_then(|q| q.as_str()) {
                            Some(name) => {
                                let specs = deck_tool_specs(&self.registry, self.session).await;
                                match find_session_tool(&specs, name) {
                                    Some(spec) => {
                                        ResumeValue::Return(MontyObject::string(help(spec)))
                                    }
                                    None => ResumeValue::Error(MontyException::new(
                                        ExcType::ValueError,
                                        Some(format!("help: unknown tool {name:?}")),
                                    )),
                                }
                            }
                            None => ResumeValue::Error(MontyException::new(
                                ExcType::TypeError,
                                Some("help(name: str) takes one string argument".into()),
                            )),
                        },
                        blocked if is_blocked_deck_tool(blocked) => {
                            ResumeValue::Error(MontyException::new(
                                ExcType::RuntimeError,
                                Some(format!(
                                    "tool `{blocked}` cannot be called from inside a Deck script (re-entrant or direct-only)"
                                )),
                            ))
                        }
                        _ => {
                            let specs = deck_tool_specs(&self.registry, self.session).await;
                            match find_session_tool(&specs, &function_name) {
                                Some(spec) => {
                                    inner_seq += 1;
                                    effects.tool_calls += 1;
                                    let call_id = format!("{turn_tag}.{inner_seq}");
                                    let merged = merge_args(spec, &args);
                                    pending.insert(
                                        fid,
                                        PendingTool {
                                            seq: inner_seq,
                                            id: call_id,
                                            name: spec.name.clone(),
                                            args: merged,
                                        },
                                    );
                                    ResumeValue::Future
                                }
                                None => ResumeValue::NotFound,
                            }
                        }
                    };
                    checkout
                        .resume(answer, &mut print_cb)
                        .await
                        .map_err(format_pool_err)?
                }
                TurnEvent::ResolveFutures {
                    pending_call_ids, ..
                } => {
                    let mut calls: Vec<(u32, PendingTool)> = pending_call_ids
                        .iter()
                        .filter_map(|id| pending.remove(id).map(|c| (*id, c)))
                        .collect();
                    calls.sort_unstable_by_key(|(_, c)| c.seq);

                    let registry = self.registry.clone();
                    let session = self.session;
                    let outs = join_all(calls.iter().map(|(_, c)| {
                        let reg = registry.clone();
                        async move {
                            reg.ask(Invoke(ToolCall {
                                id: c.id.clone(),
                                name: c.name.clone(),
                                args: c.args.clone(),
                                session,
                                quiet: false,
                            }))
                            .send()
                            .await
                            .unwrap_or_else(|e| ToolOutput::err(format!("tool registry: {e}")))
                        }
                    }))
                    .await;

                    let mut results = Vec::with_capacity(calls.len());
                    for ((fid, call), out) in calls.into_iter().zip(outs) {
                        results.push((fid, tool_result(&call.name, out)));
                    }
                    for id in pending_call_ids {
                        if !results.iter().any(|(fid, _)| *fid == id) {
                            results.push((
                                id,
                                ResumeValue::Error(MontyException::new(
                                    ExcType::RuntimeError,
                                    Some("unknown pending call".into()),
                                )),
                            ));
                        }
                    }
                    checkout
                        .resume_futures(results, &mut print_cb)
                        .await
                        .map_err(format_pool_err)?
                }
                TurnEvent::OsCall {
                    system_sleep: Some(delay),
                    ..
                } => {
                    // Clamp any direct time.sleep/asyncio.sleep inside one turn to 5s; scripts
                    // should use cooperative `deck_sleep(seconds)` for cadence between steps.
                    tokio::time::sleep(delay.min(std::time::Duration::from_secs(5))).await;
                    checkout
                        .resume(ResumeValue::Return(MontyObject::none()), &mut print_cb)
                        .await
                        .map_err(format_pool_err)?
                }
                TurnEvent::OsCall {
                    function_name,
                    args,
                    ..
                } => match checkout
                    .resume_from_mounts(&mut print_cb)
                    .await
                    .map_err(format_pool_err)?
                {
                    Some(next) => next,
                    None => checkout
                        .resume(os_answer(&function_name, &args), &mut print_cb)
                        .await
                        .map_err(format_pool_err)?,
                },
            };
        };

        let mut return_value = from_monty(result_obj.as_ref());
        tools::secrets::redact_value(&mut return_value);
        let raw_stdout = std::mem::take(&mut *stdout_buf.lock());
        effects.stdout = tools::secrets::redact(&raw_stdout).into_owned();
        Ok(TurnOutcome {
            return_value,
            effects,
        })
    }
}

fn first_arg<'a>(args: &'a CallArgs, name: &str) -> Option<monty_types::ObjectRef<'a>> {
    args.arg(0).or_else(|| args.kwarg(name))
}

fn handle_deck_emit(
    args: &CallArgs,
    effects: &mut TurnEffects,
    on_emit: &mut (dyn FnMut(&EmittedPayload) + Send),
) -> ResumeValue {
    if effects.emits.len() >= MAX_EMITS_PER_TURN {
        return ResumeValue::Error(MontyException::new(
            ExcType::ValueError,
            Some(format!(
                "deck_emit per-turn cap ({MAX_EMITS_PER_TURN}) exceeded"
            )),
        ));
    }
    let Some(data_ref) = first_arg(args, "data") else {
        return ResumeValue::Error(MontyException::new(
            ExcType::TypeError,
            Some("deck_emit(data, durable=False, section=None) requires data".into()),
        ));
    };
    let mut data = from_monty(data_ref);
    tools::secrets::redact_value(&mut data);

    let durable = args
        .arg(1)
        .or_else(|| args.kwarg("durable"))
        .map(from_monty)
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let raw_section = args
        .arg(2)
        .or_else(|| args.kwarg("section"))
        .and_then(|s| s.as_str().map(str::to_owned))
        .filter(|s| !s.is_empty());
    let section = match raw_section {
        Some(s) => {
            let redacted = tools::secrets::redact(&s).into_owned();
            if redacted.len() > MAX_SECTION_NAME_BYTES {
                return ResumeValue::Error(MontyException::new(
                    ExcType::ValueError,
                    Some(format!(
                        "deck_emit section ({} bytes) exceeds cap ({MAX_SECTION_NAME_BYTES} bytes)",
                        redacted.len()
                    )),
                ));
            }
            Some(redacted)
        }
        None => None,
    };

    let max_bytes = if durable {
        MAX_SNAPSHOT_BYTES
    } else {
        MAX_LIVE_FRAME_BYTES
    };
    let encoded_len = serde_json::to_vec(&data)
        .map(|b| b.len())
        .unwrap_or(usize::MAX);
    if encoded_len > max_bytes {
        return ResumeValue::Error(MontyException::new(
            ExcType::ValueError,
            Some(format!(
                "deck_emit payload ({encoded_len} bytes) exceeds limit ({max_bytes} bytes)"
            )),
        ));
    }

    let payload = EmittedPayload {
        data,
        durable,
        section,
    };
    on_emit(&payload);
    effects.emits.push(payload);
    ResumeValue::Return(MontyObject::none())
}

fn handle_deck_checkpoint(args: &CallArgs, effects: &mut TurnEffects) -> ResumeValue {
    let Some(state_ref) = first_arg(args, "state") else {
        return ResumeValue::Error(MontyException::new(
            ExcType::TypeError,
            Some("deck_checkpoint(state) requires state".into()),
        ));
    };
    let mut state = from_monty(state_ref);
    tools::secrets::redact_value(&mut state);
    let encoded_len = serde_json::to_vec(&state)
        .map(|b| b.len())
        .unwrap_or(usize::MAX);
    if encoded_len > MAX_SNAPSHOT_BYTES {
        return ResumeValue::Error(MontyException::new(
            ExcType::ValueError,
            Some(format!(
                "deck_checkpoint state ({encoded_len} bytes) exceeds limit ({MAX_SNAPSHOT_BYTES} bytes)"
            )),
        ));
    }
    effects.checkpoint_state = Some(state);
    ResumeValue::Return(MontyObject::none())
}

fn handle_deck_sleep(args: &CallArgs, effects: &mut TurnEffects) -> ResumeValue {
    let Some(arg) = first_arg(args, "seconds") else {
        return ResumeValue::Error(MontyException::new(
            ExcType::TypeError,
            Some("deck_sleep(seconds) requires one numeric argument".into()),
        ));
    };
    let val = from_monty(arg);
    let secs = match &val {
        Value::Number(n) => match n.as_f64() {
            Some(f) => f,
            None => {
                return ResumeValue::Error(MontyException::new(
                    ExcType::ValueError,
                    Some("deck_sleep(seconds) requires a finite numeric value".into()),
                ));
            }
        },
        _ => {
            return ResumeValue::Error(MontyException::new(
                ExcType::TypeError,
                Some(
                    "deck_sleep(seconds) requires a finite int or float (not bool/str/None)".into(),
                ),
            ));
        }
    };
    if !secs.is_finite() || secs < 0.0 {
        return ResumeValue::Error(MontyException::new(
            ExcType::ValueError,
            Some("deck_sleep(seconds) requires a non-negative finite number".into()),
        ));
    }
    effects.next_sleep_secs = Some(secs.clamp(MIN_SLEEP_SECS, MAX_SLEEP_SECS));
    ResumeValue::Return(MontyObject::none())
}

fn merge_effects(dst: &mut TurnEffects, src: TurnEffects) {
    dst.emits.extend(src.emits);
    if src.checkpoint_state.is_some() {
        dst.checkpoint_state = src.checkpoint_state;
    }
    if src.next_sleep_secs.is_some() {
        dst.next_sleep_secs = src.next_sleep_secs;
    }
    if !src.stdout.is_empty() {
        if !dst.stdout.is_empty() {
            dst.stdout.push('\n');
        }
        dst.stdout.push_str(&src.stdout);
    }
    dst.tool_calls += src.tool_calls;
}

fn format_pool_err(e: PoolError) -> String {
    let raw = format!("{e}");
    tools::secrets::redact(&raw).into_owned()
}

/// Ensure identifier helper is linked so scripts match `agent::repl` naming identically.
pub fn deck_tool_ident(name: &str) -> String {
    sanitize_ident(name)
}
