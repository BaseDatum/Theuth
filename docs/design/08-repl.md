# 08 — The runtime: a persistent Python REPL as the model's working memory

Status: implemented v0 · 2026-09-10 · extends `07-context.md` (frames, epochs,
compaction), `01-config-and-subagents.md` §1.4 (MCP), `00-architecture.md`
§4 (tool contract). Where this doc disagrees with those, this doc wins; §13
lists the edits made. Code: `crates/agent/src/repl.rs`, the `python` arm of
`loop_.rs`, `tools/src/{lib,mcp}.rs` (`direct`, `structured`).

Doc 07 stops the transcript from being rewritten. This doc stops most
things from entering the transcript at all. The model gets a persistent,
sandboxed Python runtime; tool results are values in its heap, not text in
the context window; the model decides what to print. The runtime is
serialised with the session, so `--resume` brings back the exact heap the
model was working with.

Sources: cave-agent (`acodercat/cave-agent`), prime-agent
(`PrimeIntellect-ai/prime-agent`, `docs/rlm.md`, `docs/rlm-runtime.md`),
the user's earlier `../codemcp`, and pydantic `monty` 0.0.23 (Rust crates
`monty-pool`, `monty-types`, `monty-fs`; docs at pydantic.dev/docs/monty).
§1 is what they taught us; the rest is what we build.

## 0. Decisions in one screen

1. **Runtime = monty, used as its authors intend from Rust**: `monty-pool`
   driving `monty` worker subprocesses (their "recommended Rust embedding
   surface"), `request_timeout` and `max_memory` set as they say to for
   model-written code, type checking on with stubs for our host names as
   they recommend. A language-level sandbox with no ambient authority: no
   files, env, network or clock unless the host answers for them. Dumps
   are opaque bytes that restore in another process.
2. **One tool, `python(code)`.** Persistent REPL semantics: globals,
   functions and imports survive across calls, turns, compactions and
   restarts.
3. **Every registered tool is a coroutine in the runtime, by its registered
   name.** `r = await github_search(q="…")`; `await asyncio.gather(…)`
   runs several at once. The call goes through the same `Invoke` → policy →
   approval path as a direct tool call. The sandbox is suspended while a
   human decides.
4. **Transparent to the user.** MCP servers are added exactly as today
   (`mcp/*.json`, hot-reloaded, gates in `mcp/config.yaml`). The harness
   decides how their tools reach the model: **MCP tools are runtime
   functions; native tools are both direct tools and runtime functions.**
   No gateway, no per-server flag, no code-mode switch.
5. **Output is capped small and nothing is lost.** `python`'s result is
   stdout + the last expression's repr, each capped at 8 000 chars. Oversize
   stdout is bound to a runtime variable (`out3`) and the cap marker names
   it. Tool results never touch the transcript unless printed.
6. **The harness knows what the heap holds.** It tracks top-level names the
   model binds, verifies them with a probe feed, and renders `name: type
   (len)` after every cell, in the compaction head, and after a restore.
7. **Dump after every cell** to `<data_dir>/runtime/<session>.monty`
   (atomic rename); the worker is released at turn end; the next turn
   restores in milliseconds. Crash or memory-limit → restore the previous
   dump, tell the model which cell was lost.
8. **No guard rails in the code path.** No pre-flight validation, no
   mutation allow-list, no dry-run, no AST deny-lists. Effects are gated at
   the tool boundary by the approval system that already exists; errors are
   Python exceptions the model reads and fixes.

## 1. What the three harnesses taught us

| harness | what it does | what we take | what we leave |
|---|---|---|---|
| **cave-agent** | Persistent IPython namespace; model emits one fenced ```python block per turn; injected `Variable`/`Function`/`Type` registries rendered into the system prompt; stdout > 5 000 chars bound to `_output_N` with a 2 000-char preview and a "do not re-run" instruction; two-tier compaction (clear old results, then summarise) with a "Current state — runtime variables" section | the `_output_N` pointer + preview; "No output" placeholder so silence is never mistaken for success; summary asking for runtime state | the 460-line fenced-code parser (we have tool calls); advisory AST security rules (monty is a sandbox); IPython embedding and its cross-thread gates; no snapshot at all — state dies with the process |
| **prime-agent** | Single `ipython` tool over a CPython kernel subprocess; "assign read/search results to named variables"; per-stream 64 KiB caps; `dill` per-variable snapshot after every cell (1.5 s debounce, 16 MiB/var, 256 MiB total) revived on resume; `<ipython_state>` names list re-injected after compaction; `rlm()` subagents return admission handles, results arrive as messages later | one tool; tool calls are `await` expressions; snapshot-after-cell; the post-compaction and post-restore inventory notes (we add type + length — names alone left the model guessing) | the kernel process zoo (1 656-line `repl-manager.ts`: protocol repair, busy-kernel prompts, orphan journals, background-output attribution) — monty's protocol is the pool's problem; three independent 64 KiB caps (too generous for the point of the exercise); `dill` (lossy, unpicklables dropped) — monty dumps the whole heap exactly |
| **codemcp** (ours) | Rust MCP gateway exposing `execute_python`; CPython worker over WebSocket; generated `sdk.py` with one typed function per upstream tool; prompt = two lines per tool (signature + summary); bench on Sonnet 4.6 vs direct MCP: −10 % / −55 % / −90 % input tokens on 1 / 2 / 2-call tasks, 2.9× from overlapping four calls | two lines per tool in the prompt (the entire measured token win); JSON-Schema → Python signature mapping; concurrency of independent tool calls; final-expression-as-result; structured `tool_error` naming the failing call; bench discipline (provider usage, cache split, 3 repeats) | the guard rails the user hit: static pre-flight typo/kwarg/field checks that rejected a whole run on a dead branch (0 field rejections in 90 bench runs), the verb-heuristic mutation gate and `allow_mutations`, `dry_run` with fake return values, a 30 s timeout that could not preempt CPU-bound code, a 1 MiB cap that *discarded* data with "write it to a file", shape-learning that mutated the tool list mid-session (cache bust, no gain) — and the namespace rebuilt from scratch on every run, so the "persistent worker" persisted nothing |

The pattern: every harness rediscovered that the transcript is the wrong
place for data, then spent most of its code on the runtime's *process*
(kernel lifecycle, serialisation, isolation, output plumbing). Monty puts
that below our line: it is a library whose interpreter holds no OS
resources, so pause/dump/restore/kill are all trivially safe.

## 2. Monty facts we rely on

Verified against pydantic.dev/docs/monty and crates.io 0.0.23 (2026-09-05),
then corrected against the spikes and `repl::tests::capping_binds_out_name`
(2026-09-10). Where the two disagreed the code is the source below.

- **Crates.** `monty-pool` (parent side: `Pool`, `PoolConfig`, `Checkout`,
  `ReplConfig`, `TurnEvent`, `ResumeValue`, `PoolError`, `MountSpec`,
  `MountSpecMode`, `OnPrint`, `on_print_sync`), `monty-types`
  (`MontyObject`, `MontyException`, `ExcType`, `ResourceLimits`,
  `NameLookupResult`, `MontyDateTime`/`MontyDate`). The interpreter itself
  lives in the `monty` worker binary (`monty-runtime` crate — binary-only;
  `monty subprocess` is the worker mode). Host crates do not link the
  interpreter.
- **Pool.** `PoolConfig::subprocess(&bin)` then fields (`min_processes`,
  `max_processes`, `request_timeout: Option<Duration>`,
  `max_checkouts_per_worker: Option<usize>`); `Pool::new(cfg).await` is
  **async** and can fail; `pool.checkout(&ReplConfig).await -> Checkout`;
  `pool.close().await`. `ReplConfig { limits: Option<ResourceLimits>,
  type_check: bool, type_check_stubs: Option<String>, .. }`,
  `ResourceLimits { max_memory: Option<usize>, max_suspensions: usize, .. }`
  (both `Default`).
- **Session protocol.** `Checkout::feed(code, inputs: Vec<(String,
  MontyObject)>, mounts: Vec<MountSpec>, skip_type_check: bool, on_print:
  OnPrint) -> Result<TurnEvent, PoolError>`; mounts are passed **per feed**.
  `TurnEvent::{Complete(MontyObject), FunctionCall { function_name, args,
  kwargs, call_id: u32, .. }, NameLookup { name, .. }, OsCall {
  function_name, args, .. }, ResolveFutures { pending_call_ids: Vec<u32> }}`,
  answered by `resume(ResumeValue, on_print)`,
  `resume_name_lookup(NameLookupResult::{Value, Undefined, ..}, on_print)`,
  `resume_from_mounts(on_print) -> Option<TurnEvent>` (`None` when no mount
  covers the call), `resume_futures(Vec<(u32, ResumeValue)>, on_print)`.
  `ResumeValue::{Return(MontyObject), Error(MontyException), Future,
  NotFound, NotHandled}` — `Future` is a unit variant.
- **Names.** *Calling* an undefined name surfaces as `FunctionCall`
  directly — no `NameLookup` first. `NameLookup` fires only when a name is
  *read as a value* (`f = github_search`), so the bridge resolves tools in
  both arms. A value answered to `NameLookup` is cached in globals.
- **Inputs.** `inputs` bind as persistent globals only when `code` is
  non-empty (an empty-code feed binds nothing). The type checker learns
  nothing from `inputs` or from `skip_type_check` feeds, and it hides
  `_`-prefixed globals from later cells. Consequences: oversize stdout is
  bound as `outN` (not `_outN`) via a type-checked placeholder feed (`out3
  = ''`) followed by the probe feed's `inputs` overwriting the value; the
  prompt block tells the model that `_` names do not carry across cells.
- **Async host functions.** Answering a `FunctionCall` with
  `ResumeValue::Future` lets the sandbox `await` it; when every task is
  blocked the run yields `ResolveFutures` with all pending ids —
  `asyncio.gather` over host calls becomes one event the host answers in
  parallel. Top-level `await` works in a feed.
  **Bug (monty 0.0.23, verified with a deterministic repro):** an
  `Error` answered through `resume_futures` into a coroutine that is
  running as a `gather` task (even a gather of one) bypasses that
  coroutine's `try/except` and surfaces at the `gather` await; two such
  errors in one gather produce `RuntimeError: Internal error in monty:
  asyncio scheduler stalled` (the downgraded panic of pydantic/monty#732,
  which upstream never reproduced). A `try/except` *around* the gather, or
  around a directly awaited call, works. The bridge appends the workaround
  to that error text and the Runtime prompt block states the rule; remove
  both when upstream fixes it.
- **Errors.** An uncaught exception is `Err(PoolError::Runtime(
  MontyException))`; `exc.exc_type()` and `Display` give the CPython-style
  traceback with `<python-input-N>` as the file name. A type-check failure
  is `Err(PoolError::Typing(String))` before anything runs. A killed or
  aborted worker is `Err(PoolError::Crashed)`; later calls on that checkout
  are `PoolError::Finished`. `MemoryError` arrives as a `Runtime` exception
  but the heap is untrusted afterwards (treated as a crash, §5).
- **Dumps.** `Checkout::dump().await -> Vec<u8>` (idle or suspended);
  `Checkout::restore(bytes, mounts, on_print)` on a fresh checkout, possibly
  a different worker or machine. The dump carries resource limits and the
  cumulative time budget; mounts and host-object handles do not travel; the
  format is versioned and a mismatched build or corrupt file refuses it
  (`restore` → `Err`). `Checkout::pid()` names the worker; `finish().await`
  hands it back.
- **Limits.** `ResourceLimits { max_memory, max_duration (cumulative
  execution time, session-lifetime, travels in dumps), max_recursion_depth,
  gc_interval, max_suspensions (per checkout) }`. Pool-side:
  `request_timeout` is a hard deadline **per protocol turn** — each resume
  after a host call starts a new one, so time spent waiting on the host
  (an approval, a slow MCP server) is free.
- **Crash isolation.** `kill -9` of the worker mid-session → the next
  `feed` fails, the pool respawns; the session is lost, the parent is not;
  a dump restores into a new checkout with state intact (tested). Workers
  start with an empty environment.
- **Mounts.** `MountSpec::new(virtual, host, MountSpecMode::{ReadOnly,
  ReadWrite, ..}) -> Result`; per feed; serviced on the parent via
  `resume_from_mounts` with `cap_std` containment; `..`/symlink escapes are
  structurally impossible. `open()` works in the sandbox but is missing from
  monty's trimmed typeshed, so the stubs declare it (§4.6).
- **The subset.** CPython 3.14 semantics for: functions, closures,
  lambdas, decorators on functions/classes, simple classes (no
  inheritance, no `@classmethod`/`@property`), `@dataclass(eq, frozen)`,
  comprehensions, `try/except/finally`, `with`, f-strings, `async/await`
  + `asyncio.run/gather`, imports of `asyncio base64 binascii collections
  dataclasses datetime functools itertools json math os pathlib re sys
  typing unicodedata`. **Not**: generators/`yield`, `match`, `del`, class
  inheritance, user exception classes, `eval/exec/globals/locals/dir/vars`,
  third-party packages, `time random hashlib uuid urllib io copy`.
  `enumerate/zip/map/filter/reversed` are eager. `re` is `fancy-regex`.
- **Values across the boundary.** `MontyObject`: `None`, `Bool`, `Int`
  (bigint), `Float`, `String`, `Bytes`, `List`, `Tuple`, dict (`MontyObject::
  dict(Vec<(k, v)>)`), set, `DateTime(MontyDateTime)`/`Date(MontyDate)`,
  `TimeZone`, dataclass instances, `Function { name, docstring }`;
  `py_repr()` renders the Python repr. A host exception crosses as
  `MontyException::new(ExcType::{RuntimeError, ValueError, TypeError,
  PermissionError, TimeoutError, …}, Some(msg))` and is catchable in the
  sandbox.
- **Type checking** (`ty`, `ReplConfig { type_check: true,
  type_check_stubs }`) runs every fed snippet before execution unless
  `skip_type_check`; host names must be declared in the stubs or they fail
  the check. The checker's config is fixed per checkout — new stubs need a
  new checkout (§4.6). The docs' advice is "turn on type checking rather
  than memorising this page"; we do.

## 3. What the model sees

### 3.1 The `python` tool

```rust
// agent::repl — a LOOP-LOCAL tool (07 §5): not in the registry. `Turn::tools`
// appends `repl::spec()` to the direct list when the pool is on and
// `Turn::round` dispatches it to `Turn::python`, so it bypasses the approval
// policy; Risk::ReadOnly: the sandbox has no ambient authority, every effect
// is a gated tool call inside it (§4.3).
pub struct PythonArgs { pub code: String }
pub fn spec() -> ToolSpec;      // NAME = "python", Risk::ReadOnly, direct: true
```

Description (static bytes):

> Run Python in your persistent runtime. Globals, functions and imports
> persist across calls and across sessions. Tools are async functions
> named as listed under "Runtime" in the system prompt: `r = await
> github_search(q="x")`; run several at once with `await
> asyncio.gather(a(), b())`. The value of the last expression is returned;
> stdout is returned too, capped — keep data in variables and print
> summaries. `tools(query)` lists tools, `help(name)` shows one tool's
> full parameters and description. `/cwd` is the working directory
> (read-only); write files with `write_file`.

### 3.2 System prompt block (static within a turn; §07 §4 rebase on change)

```
## Runtime
`python` runs a sandboxed Python 3.14 subset (Monty): no third-party packages, no
generators, no class inheritance; stdlib: json re math datetime collections itertools
functools dataclasses pathlib base64 asyncio(gather). State persists, except names
starting with `_`, which do not carry across cells.

Tools available in the runtime (all `async`; native tools are also direct tool calls):
async def github__search_repos(query: str, page: int | None = None) -> Any  # Search for GitHub repositories.
async def read_file(path: str, limit: int | None = None, offset: int | None = None) -> str  # Read a text file.
…
```

`repl::prompt_block(ListTools)`, appended to the baseline by `Turn::rebase`
(07 §4). One line per tool, `repl::signature(spec)`: a Python signature
derived from the JSON Schema (codemcp `py_type`: `string→str, integer→int,
number→float, boolean→bool, array→list[T], object→dict, enum→Literal[…]`,
`anyOf`/`oneOf`→`A | B`, `type: [T, "null"]`→`T | None`; required
parameters first in schema order, then optional sorted by name as `T | None
= None` — a type that is already nullable renders once, `str | None =
None`, never `str | None | None`), a return annotation (`-> str` for native
tools, whose output is text; `-> Any` for MCP tools, whose value is whatever
`structuredContent`/JSON parses to — not `-> dict`), and the first sentence
of the description, cut at 120 chars. This is the codemcp result: two lines
per tool was the whole token win over direct exposure, and the full schema
is one `help()` call away when the model needs it.

The block changes only when the registered tool set changes (a server
added or removed, a gate flipped) — user-initiated, rare, and handled as
a rebase at the next turn start. Mid-turn MCP churn changes nothing the
model sees: names resolve live at call time (§4.2) and the stubs follow
the live set (§4.6).

### 3.3 What comes back from a cell

```
<stdout, capped at 8 000 chars>
→ <repr of the last expression, capped at 8 000 chars>      (omitted when None)
bindings: r (list, 42) · total (int) · summarise (function)  (names bound by this cell)
```

- Empty everything → `(no output)`.
- Uncaught exception → stdout so far, then the monty traceback
  (CPython-style, `<python-input-N>` as the file name, ending e.g.
  `ZeroDivisionError: division by zero`) in place of the result; `is_error
  = true`.
- Type-check failure (§4.6) → `type check failed:\n<ty diagnostic>`;
  `is_error = true`; nothing ran.
- stdout over the cap → head 6 000 + tail 2 000 chars and the marker
  `[… 41 203 chars omitted; full output bound to out3 — slice or search it]`,
  where the harness bound the full string as a global (cave's `_output_N`,
  exact and free: no disk, no re-run; `outN` rather than `_outN` because
  the type checker hides `_` names from later cells, §2). The binding shows
  in the `bindings:` line (`out3 (str, 49203)`). Same for an oversize
  result repr, minus the binding: `[… N chars omitted; the value is in the
  heap — slice or search it]`.
- Tool errors surface as exceptions the model's code could have caught:
  `RuntimeError: github_search: <tool output>` (tool returned `is_error`),
  `PermissionError: shell: denied by user` / `denied by policy`,
  `TimeoutError: shell: approval timed out` (the registry's own texts,
  classified by suffix in `repl::tool_result`) — all monty built-ins, all
  catchable.
- Worker lost → `RuntimeError: runtime crashed (<reason>); state restored to
  after the previous cell — this cell's effects are lost` (§5).
- Saved state unrestorable → `<runtime> saved state could not be restored
  (<reason>); the runtime is empty. It held: r (list, 3) · …` as the first
  line of the next cell's result (§5, §6).

## 4. The bridge

`agent::repl::Repl` — one per live turn, owning a `Checkout`.

### 4.1 Driving a cell

```
Repl::run(call_id, code):
  refresh_stubs()                      -- §4.6: tool set changed since the last cell → fresh checkout from the last dump
  ensure_checkout()                    -- restore the last dump into a worker if we hold none
  drive(code, inputs=[], skip_type_check=false, on_print=append to stdout buf):
    feed(code, inputs, [cwd_ro, spill_ro], skip_type_check, on_print)
    loop on TurnEvent:
      Complete(v)                       → done
      NameLookup{name}                  → tools/help  → Value(Function{name})               (name read as a value, §2)
                                           tool name  → Value(Function{name, docstring: summary})
                                           else       → Undefined (NameError in the sandbox)
      FunctionCall{tools|help, args}    → answer immediately (Return | Error(ValueError/TypeError))
      FunctionCall{tool, args, kwargs}  → publish ToolCallStarted{call_id: "<python call_id>.<n>", args: merge_args(..)}
                                           resume(ResumeValue::Future)     -- always async
      FunctionCall{unknown}             → resume(ResumeValue::NotFound)   -- calling an undefined name skips NameLookup (§2)
      ResolveFutures{ids}               → join_all(Invoke for each id) → publish ToolCallFinished×n
                                           resume_futures([(id, Return(value) | Error(exc))])
      OsCall{..}                        → resume_from_mounts; None (not covered):
                                           datetime.now/date.today → host clock;
                                           os.getenv/environ       → default / {};
                                           anything else           → ResumeValue::NotHandled (monty raises the call's own default)
  bind oversize stdout as outN (§3.3), probe (§6), dump (§5), render
```

Every tool function is a coroutine: one calling convention, and
`asyncio.gather` gives parallel tool calls for free through a single
`ResolveFutures` event — the same `join_all` the loop already uses for a
direct tool batch. codemcp measured 2.9× on four overlapped calls.

Inner calls publish the ordinary `ToolCallStarted`/`ToolCallFinished`
events with `call_id = "{python_call_id}.{n}"` and the raw `args` JSON, so
every UI shows them as tool cards under the `python` card with the same
`describe` rendering, and approvals show the same modal. The user sees
exactly what ran.

### 4.2 Names → tools, transparently

The registry is the source of truth (`ListTools`, already sorted). A
tool's runtime name is its registered name with non-identifier characters
replaced by `_`, a leading digit prefixed with `_` (`sanitize_ident`); a
clash after sanitising resolves first-by-sorted-name (`find_tool`: first
match in `ListTools` order; the draft's warning is not emitted). Resolution
is **live**: `FunctionCall` (or `NameLookup`, when the name is read as a
value) asks the registry at call time, so a server that connected mid-turn
is callable in the next cell without any prompt change (and type-checks,
§4.6), and a server that vanished raises `NameError` (or `RuntimeError`
from `Invoke`) exactly where the model can catch it.

Arguments: kwargs map to schema properties by name; positional arguments
map in signature order (required properties in schema order, then the
rest sorted by name — the same order the prompt shows). Unknown kwargs
pass through (MCP servers may accept more than they declare). Return
value (`repl::tool_result`): `ToolOutput.structured` when present (MCP
`structuredContent`, set by `tools/mcp.rs`), else the text content parsed
as JSON when it parses to an object or array, else the string. Native
tools return their text (`shell` → the output string; `list_dir` → `str`).

`tools(query: str | None = None) -> list[str]` returns the prompt's
signature lines (filtered by substring over the whole line); `help(name:
str) -> str` returns the full description and the JSON Schema rendered as
a parameter list. Both are host functions answered without an `Invoke`
(`help` is absent from monty's builtins, so the call reaches us as a
`FunctionCall`; declared in the stubs so the checker accepts them).
`help("search_repositories")`:

```
async def search_repositories(query: str, page: int | None = None, perPage: int | None = None) -> dict

Search for GitHub repositories. Uses the GitHub search syntax, e.g. "language:rust stars:>100".

  query    str          required   Search query (see GitHub search syntax)
  page     int          optional   Page number for pagination (default: 1)
  perPage  int          optional   Results per page (default: 30, max: 100)
```

Nested `object` parameters render as `dict` in the signature and are
expanded one level in `help` (`  filters  dict {state: str, labels: list[str]}`).
Return shape is not described anywhere — it is in the value
(`r.keys()`); codemcp's shape arms showed no gain and a cache bust.

How the model discovers a mistake: a wrong or missing argument is
rejected by the tool (native: `serde` deserialisation error; MCP: the
server's error) and arrives as `RuntimeError: search_repositories: <message>`
at the call site, catchable, naming the tool — the model reads it and
calls `help()`. Most such mistakes never get that far: the type check
(§4.6) rejects a misspelled name or wrong type before the cell runs.

### 4.3 Approvals inside code

An inner call is an `Invoke`; the registry applies `tools.policy` / MCP
gates; `ask` publishes `ApprovalRequested` and awaits `Resolve`. The
sandbox is suspended at the `await` with no CPU burning (monty's "approval
gate" use case). Deny → `PermissionError`; approval timeout →
`TimeoutError`; both catchable, so a loop over twenty writes can skip the
denied one and continue. `/approvals off` applies as today.

### 4.4 Mounts

Two read-only mounts per feed: `/cwd` → the process working directory,
`/spill` → `<data_dir>/spill/<session>` (doc 07 §6). Reading is policy-
equivalent to `read_file` (`Risk::ReadOnly`, `allow`), and a read-only
mount has no 64 KiB cap, so a whole file or a whole spilled tool output
can be loaded into a variable: `text = open('/cwd/big.log').read()`.
Writes go through `write_file` (`Mutating`, gated). No read-write mount:
it would be a second write path outside the approval system.

### 4.5 Limits

`ReplConfig.limits = Some(ResourceLimits { max_memory: Some(512 MiB),
max_suspensions: 100_000, ..Default })` — no `max_duration`, default
recursion depth. No `max_duration` because it is a cumulative
session-lifetime budget that travels in
the dump; a long session would spend it and every later cell would fail.
Compute is bounded instead by the pool's `request_timeout = 60 s` per
protocol turn — pure interpreter time between suspensions — which is the
right unit (a cell that awaits an approval for ten minutes is fine; a
cell that spins for a minute is not). `max_suspensions` is per checkout
and checkouts are per turn, so a loop that calls a tool 1 000 times is
fine and a runaway loop is not.

Pool: `PoolConfig::subprocess(&monty_bin)` with `min_processes 1`,
`max_processes = subagents.max_concurrent + 1`, `request_timeout Some(60
s)`, `max_checkouts_per_worker Some(100)`; `Pool::new(cfg).await` in
`repl::open_pool`, called from `AgentPlugin::start`. Sessions without a
running turn hold no worker.

### 4.6 Type checking with generated stubs

`ReplConfig { type_check: true, type_check_stubs: Some(repl::stubs(specs)) }`,
where the stubs are a `.pyi` rendered from the same schemas as the prompt
index — `from typing import Any, Literal`, one declaration per tool, `tools`,
`help`, and `open` (monty implements it; its trimmed typeshed omits it):

```python
from typing import Any, Literal
async def search_repositories(query: str, page: int | None = None, perPage: int | None = None, **extra: Any) -> Any: ...
def tools(query: str | None = None) -> list[str]: ...
def help(name: str) -> str: ...
def open(file: str, mode: str = 'r', encoding: str | None = None) -> Any: ...
```

`ty` then rejects a cell before it runs when it calls an unknown tool,
misspells a parameter, passes the wrong type, or uses a stdlib name monty
lacks — the failure the codemcp pre-flight tried to hand-build, delivered
by the interpreter's own checker with no false positives from dead
branches (it is a type check, not an execution guess). The diagnostic
comes back as `PoolError::Typing(String)` and is returned to the model as
the cell result `type check failed:\n<diag>` (`is_error`); `await nope(1)`
never reaches a tool.

The stubs are session configuration, not prompt text: regenerating them
when the tool set changes costs nothing cache-wise. The checker's config is
fixed per checkout, so `Repl::refresh_stubs` runs before **every** cell:
it renders the stubs from `ListTools`, and when they differ from the
checkout's it `finish()`es the worker and the next `ensure_checkout`
restores the last dump into a fresh one — milliseconds, heap
byte-identical. A server that connected mid-turn is therefore callable and
type-checkable in the next cell, not the next turn. `**extra: Any` keeps
pass-through kwargs (§4.2) legal.

## 5. Persistence

```
<data_dir>/runtime/<session>.monty        Checkout::dump() bytes, written atomically (tmp + rename)
<data_dir>/runtime/<session>.names.json   the tracked inventory (§6): [{name, kind, len}]
```

- **After every completed cell**: `dump()` → write both files. A dump is a
  heap copy; for the data this runtime holds (JSON-shaped tool results,
  strings, small helper functions) it is KBs to a few MB and takes
  milliseconds. No debounce needed; add one if a transcript shows it.
- **Turn end**: dump, `finish()` the checkout (worker back to the pool)
  — `Repl::close`, called by `Turn::run` after the last round.
- **First cell of a turn / resume / subagent lazy load**: `Repl::open`
  reads the dump into `last_dump` and `ensure_checkout` does `checkout` →
  `restore(bytes, [], on_print)` (mounts travel per feed, not with the
  restore). Restore failure (version mismatch after a monty upgrade,
  corrupt file) → fresh checkout, file renamed `.stale`, inventory
  cleared, and the next cell result opens with `<runtime> saved state
  could not be restored (<err>); the runtime is empty. It held: <names>.`
- **Crash / memory limit / timeout** (any `PoolError` other than
  `Runtime(non-MemoryError)` and `Typing`): the heap is untrusted; drop
  the checkout, `ensure_checkout` restores `last_dump` (state as of the
  previous cell) into a fresh worker, and the result is `RuntimeError:
  runtime crashed (<reason>); state restored to after the previous cell —
  this cell's effects are lost` (or `state could not be restored`).
  Verified by `kill -9` of the worker mid-session in
  `repl::tests::capping_binds_out_name`.
- **Cancelled turn** while a cell runs: dropping the checkout future
  discards the worker (monty rule). The next turn restores the last dump.
- **Deletion**: none in v0; the files are the session's. Add to a future
  `/sessions rm`.

Restores are byte-exact (monty serialises the heap, not pickles of it), so
"the same runtime is ready when the session is resumed" is literally true,
not best-effort.

## 6. Inventory: what the harness knows about the heap

Monty deliberately omits `globals()/dir()/vars()`, and the host cannot
enumerate a session's globals through the pool. So the harness keeps the
inventory itself:

1. **Track** — before each cell, `repl::candidates(code)` scans column-0
   lines: `^([A-Za-z_]\w*)\s*(?::[^=\n]+)?=[^=]`, `^(?:async\s+)?def\s+(\w+)`,
   `^class\s+(\w+)`, `^import\s+(\w+)(?:\S*\s+as\s+(\w+))?`,
   `^from\s+\S+\s+import\s+(.+)` (each item, `as` alias wins); `_`-prefixed
   names and `tools`/`help` are dropped; union into the session's candidate
   set (seeded from `.names.json` on open). Tuple-unpacking targets and
   augmented assignments are caught by the model printing; ponytail: regex
   over column-0 lines, a parser if a transcript shows it missing something
   that matters.
2. **Probe** — after the cell, one hidden feed built from the candidate
   set:
   ```python
   __inv = []
   try: __inv.append(("r", type(r).__name__, len(r) if isinstance(r, (str, bytes, list, tuple, dict, set)) else None))
   except NameError: pass
   …
   __inv
   ```
   Fed with `skip_type_check = true` and the `outN` binding (§3.3) as its
   `inputs`. A missing name raises `NameError`; the result is the exact
   inventory. Names that came back undefined are dropped from the set. The
   probe leaves `__inv` in the heap; `_`-prefixed names never enter the
   set. Skipped entirely when there is nothing to probe.
3. **Render** — `r (list, 42) · cfg (dict, 3) · summarise (function)`;
   appended to the cell result as the `bindings:` line (only names this
   cell bound, `outN` included), written to `.names.json`, rendered in full
   as `Runtime: …` in the compaction head (`Repl::render_inventory`, or
   `repl::saved_inventory` from `.names.json` when no `Repl` is open — a
   `/compact` between turns) and in the restore-failure line.

This is prime-agent's `<ipython_state>` with types and lengths, computed
from truth rather than from the model's memory.

## 7. Interaction with 07 (frames, epochs, compaction)

- **Append-only holds.** A cell is a tool call: the code is in the
  assistant message, the capped result is the tool message. Both are
  tail appends. Nothing here edits history.
- **Direct tools list shrinks and stabilises.** MCP tools are no longer
  direct tool definitions, so MCP servers connecting or flapping mid-turn
  no longer change the `tools` array (the most likely mid-turn cache reset
  in 07 §5). The runtime index lives in the baseline and changes only at a
  turn boundary.
- **Compaction head** (07 §8.4) gains a `runtime` section — the full
  inventory, harness truth like the roster and the todo list — so the model
  knows after the cut that `r` still holds 42 results and can go on from
  the heap instead of re-fetching.
- **`/spill` mount** turns 07 §6's spill files into loadable data rather
  than something to `read_file` in 64 KiB pages.
- **Summariser instruction** (07 §8.3) gains under `## Notes`: "runtime
  variables you rely on and what they hold".
- **Harness notes** (07 §8.7) are untouched: a lost or crashed runtime
  reports itself as the opening `<runtime>` line of the cell result (§5),
  not as a separate note.
- **Usage.** The runtime's own compute is not tokens; the only token cost
  of a cell is code + capped result. This is where the −55 % / −90 % came
  from in the codemcp bench, and it is the token budget 07's compaction
  threshold now rarely reaches.

## 8. Subagents

Each session has its own runtime file and its own checkout while it
runs; nothing is shared. `spawn_subagent` is hidden from a max-depth
child's *direct* list but resolves inside `python` (names come from the
unfiltered registry); the supervisor rejects the call at that depth
(01 §2.6), which the model sees as a catchable `RuntimeError`. Values
cross between peers as text through
`send_message`, and because `spawn_subagent`/`send_message` are runtime
functions too, programmatic fan-out/fan-in is plain Python with the tools
that already exist:

```python
names = [await spawn_subagent(task=t) for t in tasks]          # detached (01 §2)
answers = await asyncio.gather(*(send_message(to=n, message="report", await_reply=True) for n in names))
```

That is prime-agent's blog-era `llm_batch` (values back into the program)
built from wth's existing detached model, without a new tool.

## 9. The worker binary

This is how monty is meant to be embedded from Rust: the parent links
`monty-pool`/`monty-types` only, and the interpreter runs in `monty`
worker subprocesses that the pool spawns (`<path> subprocess`), recycles
and replaces on crash. wth finds the binary as `agent.repl.monty`
(config; must be a file) → `monty` on `PATH` → `<dir of rustup which
cargo>/monty` (`repl::find_monty`). `just setup` installs it the way the
docs show:
`command -v monty || cargo install monty-runtime --version 0.0.23 --no-default-features --root <toolchain root>`
(the root is the directory above `rustup which cargo`'s `bin/`;
`--no-default-features` is the worker-only build: no REPL, no TLS). The
PyPI wheel `pydantic-monty-runtime` is the same binary for users who have
`uv` and no Rust toolchain; either path yields a `monty` on `PATH`.

Not found at start (or `Pool::new` fails) → `repl::open_pool` returns
`None` with one `warn!` (`monty worker not found: python runtime disabled.
Install with …`) and the runtime is off for the process: `Turn::tools`
adds no `python`, skips the `direct` filter so MCP tools are **direct**
tool definitions again, and `Turn::rebase` appends no `## Runtime` block.
Transparent degradation, no half-state. Verified (§11.11).

## 10. Types, config, store

```rust
// agent::repl
pub const NAME: &str = "python";
pub const RUN_OUTPUT_CHARS: usize = 8_000;                 // §3.3; HEAD 6_000 + TAIL 2_000
pub const MAX_MEMORY: usize = 512 << 20;                   // §4.5
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
pub struct PythonArgs { pub code: String }
pub fn spec() -> ToolSpec;                                 // loop-local, §3.1
pub struct RunCx { pub session: SessionId, pub turn: TurnId, pub rt: Runtime, pub registry: ActorRef<ToolRegistry>,
                   pub spill_dir: PathBuf, pub cwd: PathBuf }
pub async fn open_pool(cfg: &ReplConfig, max_processes: usize) -> Option<Arc<Pool>>;   // §9
pub fn sanitize_ident(name: &str) -> String;
pub fn signature(spec: &ToolSpec) -> String;   // "async def name(a: str, b: int | None = None) -> str  # summary"
pub fn stubs(specs: &[ToolSpec]) -> String;    // §4.6, the .pyi for ReplConfig.type_check_stubs
pub fn prompt_block(specs: &[ToolSpec]) -> String;   // §3.2
pub fn to_monty(v: &Value) -> MontyObject;  pub fn from_monty(o: &MontyObject) -> Value;
pub fn candidates(code: &str) -> BTreeSet<String>;   // §6
pub fn saved_inventory(data_dir: &Path, session: SessionId) -> Option<String>;   // §6, from .names.json
pub struct Repl { pool, cfg: ReplConfig, cx: RunCx, checkout: Option<Checkout>, mounts: Vec<MountSpec>,
                  dump_path, names_path, last_dump: Option<Vec<u8>>, names: BTreeMap<String, Slot>,
                  candidates: BTreeSet<String>, out_seq: u32, lost: Option<String> }
impl Repl {
    pub async fn open(pool: &Arc<Pool>, session: SessionId, data_dir: &Path, cx: RunCx) -> Repl;  // never fails; restores the dump
    pub async fn run(&mut self, call_id: &str, code: &str) -> ToolOutput;                        // §4.1 + §6 + dump
    pub fn render_inventory(&self) -> Option<String>;                                           // compaction head
    pub async fn close(self);                                                                   // dump + finish
}

// tools
pub struct ToolSpec { …, pub risk: Risk, pub direct: bool }   // true for native tools; false for MCP (tools/mcp.rs)
pub struct ToolOutput { pub content: String, pub is_error: bool, pub structured: Option<serde_json::Value> }
// tools::mcp sets `structured` from `structuredContent`

// agent (lib.rs)
pub struct ReplConfig { pub monty: Option<PathBuf> }
AgentConfig { …, repl: ReplConfig }
// loop_.rs: Shared.pool: Option<Arc<Pool>>; Turn.repl: Option<Repl>, opened on the first `python` call of a turn

// config
agent:
  repl:
    monty: null        # path to the `monty` worker; default: `monty` on PATH, then the toolchain bin dir
```

One config key. Everything else is a constant with its reason in this doc.
The `Pool` is created once by `AgentPlugin::start` (or not, §9) with
`max_processes = subagents.max_concurrent + 1` and shared by sessions via
`Shared.pool`. Store: no schema change — the runtime files are keyed by
session id; `Loaded`/`ListSessions` are untouched.

Where the code lives: `agent::repl` (tool spec, bridge, inventory,
persistence — ~1 200 lines with tests) plus `Turn::python` in `loop_.rs`.
It needs the registry (`ListTools`, `Invoke`), the bus (events), the
session id and data dir, all of which the agent crate already has; it is
not a plugin and not a crate (`00 §1.6`).

## 11. Verification

Spikes first, as throwaway `spikes/examples/*.rs`, deleted after (00 §11).
**All six done**; their residue is §2 (the corrections: async `Pool::new`,
`FunctionCall` without `NameLookup`, unit `Future`, `inputs` needing
non-empty code, `_` names invisible to the checker, `<python-input-N>`,
`open()` missing from the typeshed):

1. `monty-pool` checkout; feed `x = 21`, then `x * 2` → `Int(42)`; dump;
   restore into a second checkout; `x + 1` → 22. Kill the worker process
   by pid mid-session → `PoolError::Crashed`; restore the dump into a new
   checkout → state intact.
2. Async bridge: `f`, `g` answered as `Function`; `await
   asyncio.gather(f(1), g(2))` → one `ResolveFutures` with two ids; answer
   both; `Complete(List[..])`. Top-level `await f(1)` → one
   `ResolveFutures` with one id.
3. `MountSpec` `/cwd` read-only over a temp dir; `open('/cwd/a.txt').read()`
   → contents; `open('/cwd/a.txt', 'w')` → `PermissionError` in the sandbox.
4. `inputs` binding (led to the `outN` placeholder-feed design, §2).
5. `to_monty`/`from_monty` round trip over a nested JSON fixture; a dict
   with int keys → stringified.
6. Type check: `ReplConfig { type_check: true, type_check_stubs }` with a
   two-function stub; a cell calling a misspelled name → `PoolError::Typing`
   and the session stays usable; a correct cell runs.

Then, runnable checks (not a suite). Results as of 2026-09-10:

6. **Unit** (`agent::repl`): `signature()` from a schema fixture matches
   the documented form (nullable optionals once, `anyOf` → `A | B`,
   `-> Any` for MCP); `sanitize_ident` on `a-b.c` → `a_b_c`, `9lives` →
   `_9lives`; `candidates` on a scripted cell; JSON round trip; and, against
   a real worker (`capping_binds_out_name`, skipped when `monty` is
   absent): 3 000 printed lines → `out1 (str, 30000)` bound and named by
   the marker, `len(out1)` next cell; `await nope(1)` → `type check
   failed:`; `kill -9` → crash line, `r` intact from the previous dump;
   close/reopen → `r + [n]`; `1/0` traceback; gathered inner calls with
   positional and keyword args; `RuntimeError: read_file:` caught in the
   sandbox; `/cwd` mount read; `tools()`/`help()`; corrupt dump → the
   `<runtime>` loss line naming `n · out1 · r`. **Pass.**
7. **Headless e2e, `everything` MCP server**: `python("await
   echo(message='hi')")` → `→ 'Echo: hi'` and a nested
   `ToolCallStarted/Finished` pair for `echo` rendered as an `↳ echo` card
   under the `python` card. **Pass.** The `echo: ask` gate → deny →
   `PermissionError` path was not exercised end to end (the classification
   is unit-level, `repl::tool_result`).
8. **Token check**: a tool returning ~1 MB assigned to a variable produces
   a `python` tool message under 400 chars; `TurnFinished.usage.prompt`
   grows by < 500 tokens for that round. **Not exercised end to end**; the
   cap and binding are unit-tested (item 6).
9. **Persistence**: bind values, quit `wth`, `--resume`, read them back;
   `runtime/<id>.monty` exists; corrupt the file → the next cell's result
   opens with the `<runtime>` loss line. **Pass** — `x = 21`, `r = 'Echo:
   hi'`, `data = {'a': [1, 2, 3]}` bound in one process; after exit and
   `--resume` the cell `(x, r, data, len(data["a"]))` returned `(21,
   'Echo: hi', {'a': [1, 2, 3]}, 3)`; `<data_dir>/runtime/<id>.monty` (5.5
   KB) and `.names.json` present. The corrupt-file arm is covered by item 6.
10. **Compaction**: after a forced compaction the head contains `Runtime:
    r (list, 3)`; the next cell can use `r`. **Not exercised end to end**;
    `render_inventory` / `saved_inventory` are unit-tested and the head
    assembly is the 07 §12.5 path.
11. **Degradation**: point `agent.repl.monty` at a missing file with no
    `monty` on `PATH` or in the toolchain bin dir → start logs `WARN
    agent::repl: monty worker not found: python runtime disabled. Install
    with …`, the model's tool list has no `python`, and `echo` (MCP) is a
    direct tool call that works. **Pass.**

## 12. Implementation order (done)

Each step left `wth` runnable.

1. **Spikes 1–6.** Residue recorded in §2.
2. **Bridge + tool.** `ToolOutput.structured`; `agent::repl` with `open`/
   `run`/`close`, name resolution, async calls + `ResolveFutures`, mounts,
   OS-call answers, output capping + `outN`, inventory, dump/restore;
   `python` as a loop-local tool when the pool is on; pool in
   `AgentPlugin::start`; `just setup` installs the worker.
3. **Exposure switch.** `ToolSpec.direct` (`false` from `tools/mcp.rs`);
   `Turn::tools` retains `direct` when the runtime is on (no `ListTools`
   filter — the loop filters); baseline gains the Runtime block (07 §4
   rebase handles changes); `tools()`/`help()`.
4. **Type checking.** `stubs()` from `ListTools`, `type_check: true`,
   refreshed before every cell (§4.6); `help()` text and the stubs share
   one schema renderer.
5. **07 integration.** Head `Runtime:` section (live or from
   `.names.json`), `/spill` mount, summariser line. The `<runtime>` loss
   line is part of the cell result rather than a separate harness note.
6. **Docs** (this pass). 07 edits (§13), `01 §1.4`, `00 §5`, README
   (`monty` in `just setup`, `agent.repl.monty`).

## 13. Edits to 07 (applied)

- §2.3 table: row `runtime inventory | Repl state … | every cell | append; harness truth`.
- §5: direct tools list = native tools + `todo` + `python`; MCP churn no
  longer touches it when the runtime is on; `todo`/`python` loop-local.
- §6: `/spill` is mounted read-only into the runtime (08 §4.4).
- §8.3 instruction: `## Notes` adds "runtime variables you rely on and what they hold".
- §8.4 head: `Runtime: …` from `Repl::render_inventory` or `repl::saved_inventory` after `todos`.
- §8.7: two note kinds; the runtime loss line is the first line of the cell
  result (§3.3, §5), not a `Turn::note`.
- §14 "Rejected": added "keeping MCP tools as direct tool definitions when
  the runtime is on — see 08 §0.4".

## 14. Rejected

- **Fenced ```python blocks as the protocol** (cave-agent). Provider tool
  calls give structured boundaries; cave-agent's parser is 460 lines of
  state machine because it did not have them.
- **CPython worker** (prime-agent, cave-agent's `IPyKernelRuntime`,
  codemcp). Full stdlib and pandas are real advantages, and the price is a
  Python install, `dill` (lossy), a process to babysit, and no sandbox.
  wth is not a coding agent; the runtime is for orchestrating tools and
  holding their results, which the monty subset covers. If a transcript
  shows the model needing `pandas`, the shell tool has the user's Python.
- **Anything other than `monty-pool` + the `monty` worker binary**: the
  in-process `monty` crate (a sandbox abort would take every session down)
  or a fork that embeds the worker loop in `wth` (a lib target upstream
  does not have, plus `monty-alloc` as the whole harness's allocator). The
  pool over subprocess workers is the documented Rust embedding; we use it
  as documented.
- **Pre-flight validation** (codemcp typo/kwarg/field checks). Rejected a
  whole run for an error in a branch that never ran; 0 field rejections in
  90 bench runs. Python's own `NameError`/`TypeError` at the call site is
  the same information, only when it matters.
- **Mutation gate / `allow_mutations` / `dry_run`** (codemcp). A verb
  heuristic (`run`, `sync`, `check` misclassified) forcing the model to
  restate function names, plus fake return values the model then reasoned
  over. The approval system gates the real effect at the real boundary.
- **Advisory AST deny-lists** (cave-agent `security/`). Monty has no
  operation to deny; a deny-list on top is false confidence.
- **Shape learning / `# returns:` in the tool list** (codemcp). Mutated the
  tool list mid-session (cache bust) for no measured gain; the result's
  shape is in the result.
- **Per-variable snapshot with size caps and pruning** (prime-agent).
  Monty dumps the heap exactly; the bound is `max_memory`. Pruning
  variables behind the model's back was the failure mode prime-agent then
  had to explain with a names list.
- **Generous output caps** (prime-agent 3 × 64 KiB). 8 000 chars total is
  the research design (RLM 8 192) and the point: data stays in the heap.
- **A variable table in the system prompt** (cave-agent). Changes every
  cell → busts the prefix every round. Ours is in the cell result (tail)
  and the compaction head.
- **`rlm()`-style admission handles** (prime-agent). Programmatic fan-in
  needs values; §8 gets them with `send_message(await_reply=True)` and
  `gather`.
- **Read-write mount of cwd.** A second write path around approvals.
- **A `runtime` crate / plugin.** Only the agent uses it (00 §1.6).
