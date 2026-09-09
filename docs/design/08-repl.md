# 08 — The runtime: a persistent Python REPL as the model's working memory

Status: draft v0 · 2026-09-09 · extends `07-context.md` (frames, epochs,
compaction), `01-config-and-subagents.md` §1.4 (MCP), `00-architecture.md`
§4 (tool contract). Where this doc disagrees with those, this doc wins; §13
lists the edits.

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
   stdout is bound to a runtime variable (`_out3`) and the cap marker names
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

Verified against pydantic.dev/docs/monty and crates.io 0.0.23 (2026-09-05).

- **Crates.** `monty-pool` (parent side: `Pool`, `Checkout`, `TurnEvent`),
  `monty-types` (`MontyObject`, `ResourceLimits`, `MontyException`),
  `monty-fs` (`MountSpec`). The interpreter itself lives in the `monty`
  worker binary (`monty-runtime` crate — binary-only; `monty subprocess`
  is the worker mode). Host crates do not link the interpreter.
- **Session protocol.** `Checkout::feed(code, inputs, mounts,
  skip_type_check, on_print) -> TurnEvent`; `TurnEvent::{Complete(value),
  FunctionCall{function_name, args, kwargs, call_id}, NameLookup{name},
  OsCall{function_name, args}, ResolveFutures{pending_call_ids}}`; answered
  by `resume(ResumeValue::{Return, Error, Future, NotFound, NotHandled})`,
  `resume_name_lookup(value | Undefined | Error)`, `resume_from_mounts()`,
  `resume_futures(Vec<(id, ResumeValue)>)`.
  `inputs` bind eagerly as globals; an undefined name surfaces as
  `NameLookup` and a value answered there is cached in globals.
- **Async host functions.** Answering a `FunctionCall` with a pending
  future lets the sandbox `await` it; when every task is blocked the run
  yields `ResolveFutures` with all pending ids — `asyncio.gather` over
  host calls becomes one event the host answers in parallel. Top-level
  `await` works in a feed.
- **Dumps.** `Checkout::dump() -> Vec<u8>` (idle or suspended);
  `Checkout::restore(bytes, mounts, on_print)` on a fresh checkout,
  possibly a different worker or machine. The dump carries resource limits
  and the cumulative time budget; mounts and host-object handles do not
  travel; the format is versioned (`MONTY\0` + version) and a mismatched
  build refuses it.
- **Limits.** `ResourceLimits { max_memory, max_duration (cumulative
  execution time, session-lifetime, travels in dumps), max_recursion_depth
  (1000), gc_interval, max_suspensions (1000 per checkout) }`. Pool-side:
  `request_timeout` is a hard deadline **per protocol turn** — each resume
  after a host call starts a new one, so time spent waiting on the host
  (an approval, a slow MCP server) is free. Memory/time limits are
  terminal for the heap: discard the session afterwards.
- **Crash isolation.** Stack-overflow or allocator abort kills the worker;
  the pool reports `PoolError::Crashed` and respawns; the session is lost,
  the parent is not. Workers start with an empty environment.
- **Mounts.** `MountSpec::new(virtual, host, ReadOnly|ReadWrite|Overlay)`;
  per-feed; serviced on the parent via `resume_from_mounts` with `cap_std`
  containment; `..`/symlink escapes are structurally impossible.
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
- **Values across the boundary.** `MontyObject`: `None`, bool, int
  (bigint), float, str, bytes, list, tuple, dict, set, datetime/date,
  dataclass instances, `Function { name, docstring }`. Nesting caps (~48
  lists / 32 dicts), 256 MiB per frame. A host exception crosses as a
  monty built-in exception type by name (`RuntimeError`, `ValueError`,
  `PermissionError`, `TimeoutError`, …) and is catchable in the sandbox.
- **Type checking** (`ty`, `ReplConfig { type_check: true,
  type_check_stubs }`) runs every fed snippet before execution; host names
  must be declared in the stubs or they fail the check. The docs' advice
  is "turn on type checking rather than memorising this page"; we do (§4.6).

## 3. What the model sees

### 3.1 The `python` tool

```rust
// agent::repl — registered like load_skill; Risk::ReadOnly: the sandbox has no
// ambient authority, every effect is a gated tool call inside it.
struct PythonArgs { code: String }
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
functools dataclasses pathlib base64 asyncio(gather). State persists.

Tools available in the runtime (all `async`; native tools are also direct tool calls):
async def github_search(q: str, limit: int | None = None) -> dict  # Search repositories by query
async def github_create_issue(owner: str, repo: str, title: str, body: str | None = None) -> dict  # Create an issue
…
```

One line per tool: a Python signature derived from the JSON Schema
(codemcp `py_type`: `string→str, integer→int, number→float, boolean→bool,
array→list[T], object→dict, enum→Literal[…]`, `anyOf→A | B`; required
parameters first, optional as `T | None = None`) and the first sentence of
the description, cut at 120 chars. This is the codemcp result: two lines
per tool was the whole token win over direct exposure, and the full
schema is one `help()` call away when the model needs it.

The block changes only when the registered tool set changes (a server
added or removed, a gate flipped) — user-initiated, rare, and handled as
a rebase at the next turn start. Mid-turn MCP churn changes nothing the
model sees: names resolve live at call time (§4.2).

### 3.3 What comes back from a cell

```
<stdout, capped at 8 000 chars>
→ <repr of the last expression, capped at 8 000 chars>      (omitted when None)
bindings: r (list, 42) · total (int) · summarise (function)  (names bound by this cell)
```

- Empty everything → `(no output)`.
- Uncaught exception → the monty traceback (CPython-style, `<cell N>` as
  the file name) in place of the result; `is_error = true`.
- stdout over the cap → head 6 000 + tail 2 000 chars and the marker
  `[… 41 203 chars omitted; full output bound to _out3 — slice or search it]`,
  where the harness bound the full string as a global via `inputs` (cave's
  `_output_N`, exact and free: no disk, no re-run). Same for an oversize
  result repr, minus the binding (the value is already in the heap; the
  marker says so).
- Tool errors surface as exceptions the model's code could have caught:
  `RuntimeError: github_search: <tool output>` (tool returned `is_error`),
  `PermissionError: shell: denied by user`, `TimeoutError: shell: no
  approval within 300s` — all monty built-ins, all catchable.
- Runtime lost → `<runtime>` line first (§6).

## 4. The bridge

`agent::repl::Repl` — one per live turn, owning a `Checkout`.

### 4.1 Driving a cell

```
feed(code, inputs, [cwd_ro, spill_ro], skip_type_check=false, on_print=append to stdout buf)
loop on TurnEvent:
  Complete(v)                       → done
  NameLookup{name}                  → tools/help  → Function
                                       tool name  → Function{name, docstring: first line}
                                       else       → Undefined (NameError in the sandbox)
  FunctionCall{tools|help, args}    → answer immediately (Return)
  FunctionCall{tool, args, kwargs}  → publish ToolCallStarted{call_id: "<python call>.<n>"}
                                       resume(ResumeValue::Future)     -- always async
  ResolveFutures{ids}               → join_all(Invoke for each id) → publish ToolCallFinished×n
                                       resume_futures([(id, Return(value) | Error(exc))])
  OsCall{..}                        → resume_from_mounts; if not covered:
                                       datetime.now/date.today → host clock;
                                       os.getenv/environ       → None / {};
                                       anything else           → ResumeValue::NotHandled (monty raises the call's own default)
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
replaced by `_` (`sanitize_ident`); a clash after sanitising is resolved
first-by-sorted-name with a warning, the same rule the registry applies to
server clashes. Resolution is **live**: `NameLookup` asks the registry at
call time, so a server that connected mid-turn is callable in the next
cell without any prompt change, and a server that vanished raises
`NameError` (or `RuntimeError` from `Invoke`) exactly where the model can
catch it.

Arguments: kwargs map to schema properties by name; positional arguments
map in signature order (required properties in schema order, then the
rest sorted by name — the same order the prompt shows). Unknown kwargs
pass through (MCP servers may accept more than they declare). Return
value: MCP `structuredContent` when present, else the joined text content
parsed as JSON when it parses, else the string. Native tools return their
text (`shell` → the output string). This requires `ToolOutput` to gain
`structured: Option<serde_json::Value>`, set by the MCP source.

`tools(query: str | None = None) -> list[str]` returns the prompt's
signature lines (filtered by substring over name+summary); `help(name:
str) -> str` returns the full description and the JSON Schema rendered as
a parameter list. Both are host functions answered without an `Invoke`
(`help` is absent from monty's builtins, so the name reaches us as a
`NameLookup`). `help("search_repositories")`:

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

`ReplConfig.limits = ResourceLimits { max_memory: 512 MiB, max_duration:
None, max_recursion_depth: default, max_suspensions: 100_000 }`. No
`max_duration`: it is a cumulative session-lifetime budget that travels in
the dump; a long session would spend it and every later cell would fail.
Compute is bounded instead by the pool's `request_timeout = 60 s` per
protocol turn — pure interpreter time between suspensions — which is the
right unit (a cell that awaits an approval for ten minutes is fine; a
cell that spins for a minute is not). `max_suspensions` is per checkout
and checkouts are per turn, so a loop that calls a tool 1 000 times is
fine and a runaway loop is not.

Pool: `PoolConfig::subprocess(monty_bin)` with `min_processes 1`,
`max_processes = subagents.max_concurrent + 1`, `max_checkouts_per_worker
100`. Sessions without a running turn hold no worker.

### 4.6 Type checking with generated stubs

`ReplConfig { type_check: true, type_check_stubs: Some(stubs) }`, where
`stubs` is a `.pyi` rendered from the same schemas as the prompt index —
one declaration per tool plus `tools` and `help`:

```python
async def search_repositories(query: str, page: int | None = None, perPage: int | None = None, **extra: Any) -> dict: ...
def tools(query: str | None = None) -> list[str]: ...
def help(name: str) -> str: ...
```

`ty` then rejects a cell before it runs when it calls an unknown tool,
misspells a parameter, passes the wrong type, or uses a stdlib name monty
lacks — the failure the codemcp pre-flight tried to hand-build, delivered
by the interpreter's own checker with no false positives from dead
branches (it is a type check, not an execution guess). The diagnostic
comes back as `PoolError::Typing` and is returned to the model as the cell
result (`is_error`), rendered per `type_check_config`.

The stubs are session configuration, not prompt text: regenerating them
when the tool set changes costs nothing cache-wise. They are rebuilt at
each `Repl::open` (turn start) from `ListTools`; a dump carries the
type-check state of the session that made it, and a restore adopts the
dump's, so the checkout's `ReplConfig` stubs apply from the next `open`.
`**extra: Any` keeps pass-through kwargs (§4.2) legal.

## 5. Persistence

```
<data_dir>/runtime/<session>.monty        Checkout::dump() bytes, written atomically (tmp + rename)
<data_dir>/runtime/<session>.names.json   the tracked inventory (§6): [{name, kind, len}]
```

- **After every completed cell**: `dump()` → write both files. A dump is a
  heap copy; for the data this runtime holds (JSON-shaped tool results,
  strings, small helper functions) it is KBs to a few MB and takes
  milliseconds. No debounce needed; add one if a transcript shows it.
- **Turn end**: dump, `finish()` the checkout (worker back to the pool).
- **First cell of a turn / resume / subagent lazy load**: `checkout` →
  `restore(bytes, mounts)` when the file exists. Restore failure (version
  mismatch after a monty upgrade, corrupt file) → fresh session, file
  renamed `.stale`, the cell result opens with a `<runtime>` line saying
  the state was lost and listing the names that were in the inventory.
- **Crash / memory limit** (`PoolError::Crashed`, `MemoryError`): the
  heap is untrusted; take a fresh checkout, restore the last dump (state
  as of the previous cell), and return `RuntimeError: runtime crashed
  (<reason>); state restored to after the previous cell — this cell's
  effects are lost`.
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

1. **Track** — before each cell, scan the code for top-level bindings:
   `^([A-Za-z_]\w*)\s*(?::[^=\n]+)?=[^=]`, `^(?:async\s+)?def\s+(\w+)`,
   `^class\s+(\w+)`, `^import\s+(\w+)`, `^from\s+\S+\s+import\s+(.+)`;
   union into the session's candidate set. Tuple-unpacking targets and
   augmented assignments are caught by the next step or by the model
   printing; ponytail: regex over column-0 lines, a parser if a transcript
   shows it missing something that matters.
2. **Probe** — after the cell, one hidden feed built from the candidate
   set:
   ```python
   __inv = []
   try: __inv.append(("r", type(r).__name__, len(r) if isinstance(r, (str, bytes, list, tuple, dict, set)) else None))
   except NameError: pass
   …
   __inv
   ```
   A missing name raises `NameError` (the harness answers its `NameLookup`
   with `Undefined`); the result is the exact inventory. Names that came
   back undefined are dropped from the set. The probe leaves `__inv` in
   the heap; `_`-prefixed names are hidden from every listing.
3. **Render** — `r (list, 42) · cfg (dict, 3) · summarise (function)`,
   `_`-prefixed and `tools/help` omitted; appended to the cell result as
   the `bindings:` line (only names this cell bound), written to
   `.names.json`, rendered in full in the compaction head and the restore
   note.

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
- **Harness notes** (07 §8.7) gain a `runtime` kind for the restore-failure
  and crash cases.
- **Usage.** The runtime's own compute is not tokens; the only token cost
  of a cell is code + capped result. This is where the −55 % / −90 % came
  from in the codemcp bench, and it is the token budget 07's compaction
  threshold now rarely reaches.

## 8. Subagents

Each session has its own runtime file and its own checkout while it
runs; nothing is shared. Values cross between peers as text through
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
(config) → `monty` on `PATH` → `<toolchain bin>/monty`. `just setup`
installs it the way the docs show:
`cargo install monty-runtime --no-default-features --root <toolchain dir>`
(`--no-default-features` is the worker-only build: no REPL, no TLS). The
PyPI wheel `pydantic-monty-runtime` is the same binary for users who have
`uv` and no Rust toolchain; either path yields a `monty` on `PATH`.

Not found at start → the runtime is disabled for the process: `python` is
not registered, MCP tools are registered as **direct** tools (today's
behaviour, unchanged code path), and one `warn!` names the install
command. Transparent degradation, no half-state.

## 10. Types, config, store

```rust
// agent::repl
pub struct Repl { checkout: Checkout, names: BTreeSet<String>, out_seq: u32 }
impl Repl {
    pub async fn open(pool: &Pool, session: SessionId, data_dir: &Path) -> Result<Self>;   // restore if dump exists
    pub async fn run(&mut self, code: &str, cx: &RunCx) -> ToolOutput;                    // §4.1 + §6 + dump
    pub async fn close(self) -> Result<()>;                                                // dump + finish
}
pub struct PythonTool;                     // NAME = "python", Risk::ReadOnly, Args { code }
pub fn signature(spec: &ToolSpec) -> String;   // "async def name(a: str, b: int | None = None) -> dict  # summary"
pub fn sanitize_ident(name: &str) -> String;
pub fn to_monty(v: &serde_json::Value) -> MontyObject;  pub fn from_monty(o: &MontyObject) -> serde_json::Value;
pub const RUN_OUTPUT_CHARS: usize = 8_000;
pub const MAX_MEMORY: u64 = 512 << 20;
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
pub fn stubs(specs: &[ToolSpec]) -> String;      // §4.6, the .pyi for ReplConfig.type_check_stubs

// tools
pub struct ToolOutput { pub content: String, pub is_error: bool, pub structured: Option<serde_json::Value> }  // new field
// tools::mcp sets `structured` from `structuredContent`

// config
agent:
  repl:
    monty: null        # path to the `monty` worker; default: `monty` on PATH, then the toolchain bin dir
```

One config key. Everything else is a constant with its reason in this doc.
The `Pool` is created once by `AgentPlugin::start` (or not, §9) and shared
by sessions via `Shared`. Store: no schema change — the runtime files are
keyed by session id; `Loaded`/`ListSessions` are untouched.

Where the code lives: `agent::repl` (tool, bridge, inventory, persistence
— ~500 lines). It needs the registry (`Invoke`), the bus (events), the
session id and data dir, all of which the agent crate already has; it is
not a plugin and not a crate (`00 §1.6`).

## 11. Verification

Spikes first, as throwaway `spikes/examples/*.rs`, deleted after (00 §11):

1. `monty-pool` checkout; feed `x = 21`, then `x * 2` → `Int(42)`; dump;
   restore into a second checkout; `x + 1` → 22. Kill the worker process
   by pid mid-session → `PoolError::Crashed`; restore the dump into a new
   checkout → state intact.
2. Async bridge: `NameLookup` for `f`, `g` answered with `Function`;
   `await asyncio.gather(f(1), g(2))` → one `ResolveFutures` with two
   ids; answer both; `Complete(List[..])`. Top-level `await f(1)` → one
   `ResolveFutures` with one id.
3. `MountSpec` `/cwd` read-only over a temp dir; `open('/cwd/a.txt').read()`
   → contents; `open('/cwd/a.txt', 'w')` → `PermissionError` in the sandbox.
4. `inputs=[("_out1", Str(1 MiB))]` with empty code → later feed
   `len(_out1)` → correct; dump size and time for a 5 MB heap.
5. `to_monty`/`from_monty` round trip over a nested JSON fixture; a dict
   with int keys → stringified.
6. Type check: `ReplConfig { type_check: true, type_check_stubs }` with a
   two-function stub; a cell calling a misspelled name → `PoolError::Typing`
   and the session stays usable; a correct cell runs.

Then, runnable checks (not a suite):

6. **Unit** (`agent::repl`): `signature()` from a schema fixture matches
   the documented form; `sanitize_ident` on `a-b.c` → `a_b_c`; the
   inventory regex + probe on a scripted cell set yields the expected
   `(name, kind, len)` list; output capping binds `_outN` and the marker
   names it.
7. **Headless e2e, `everything` MCP server**: `python("await echo(message='hi')")`
   → `→ 'Echo: hi'` and a nested `ToolCallStarted/Finished` pair for
   `echo`. A gate `echo: ask` in `mcp/config.yaml` → `ApprovalRequested`
   appears while the cell is suspended; `deny` → the cell result is
   `PermissionError` from inside `try/except` when the code catches it.
8. **Token check**: a tool returning ~1 MB (`longRunningOperation` or a
   `read_file` of a large file via `/cwd`) assigned to a variable produces
   a `python` tool message under 400 chars; `TurnFinished.usage.prompt`
   grows by < 500 tokens for that round.
9. **Persistence**: bind `r = [1,2,3]`, quit `wth`, `--resume`, `python("r")`
   → `[1, 2, 3]`; `runtime/<id>.monty` exists; corrupt the file → the next
   cell's result opens with the `<runtime>` loss line naming `r`.
10. **Compaction** (`compact_at: 0.05`): after a forced compaction the
    head contains `runtime: r (list, 3)`; the next cell can use `r`.
11. **Degradation**: rename the `monty` binary → start logs the install
    hint, `python` is absent, `echo` is a direct tool, everything else
    works.

## 12. Implementation order

Each step leaves `wth` runnable.

1. **Spikes 1–6** (one afternoon; they decide nothing in this doc is
   wrong about monty). Record residue in §2.
2. **Bridge + tool.** `ToolOutput.structured`; `agent::repl` with `open`/
   `run`/`close`, name resolution, async calls + `ResolveFutures`, mounts,
   OS-call answers, output capping + `_outN`, inventory, dump/restore;
   `python` tool registered when the binary is found; pool in
   `AgentPlugin::start`; `just setup` installs the worker. MCP tools still
   direct at this step. Check: 6, 7, 8, 9.
3. **Exposure switch.** MCP tools registered runtime-only when the runtime
   is enabled (registry flag `direct: bool` on `ToolSpec`; `ListTools`
   gains a filter); baseline gains the Runtime block and index (07 §4
   rebase handles changes); `tools()`/`help()`. Check: 7 again (echo no
   longer a direct tool), 11.
4. **Type checking.** `stubs()` from `ListTools`, `type_check: true`;
   `help()` text and the stubs share one schema renderer. Check: a
   misspelled kwarg returns a `ty` diagnostic as the cell result without a
   tool firing.
5. **07 integration.** Head `runtime` section, `runtime` note kind,
   `/spill` mount, summariser line. Check: 10.
6. **Docs.** 07 edits (§13 below), 01 §1.4 ("tools registered … under
   their name" → "callable in the runtime; direct when the runtime is
   off"), 00 §5 tool paragraph, README (`monty` in `just setup`,
   `agent.repl.monty`), `docs/skills.md` unchanged.

## 13. Edits to 07

- §2.3 table: row `runtime inventory | head + cell results | every cell | append; harness truth (08 §6)`.
- §5: direct tools list = native tools + `python`; MCP churn no longer
  touches it when the runtime is on.
- §6: `/spill` is mounted read-only into the runtime (08 §4.4).
- §8.3 instruction: `## Notes` adds "runtime variables you rely on and what they hold".
- §8.4 head: `runtime = repl::render_inventory(&names)` after `todos`.
- §8.7: third note kind, `runtime` (restore failure, crash).
- §14 "Rejected": remove nothing; add "keeping MCP tools as direct tool
  definitions when the runtime is on — see 08 §0.4".

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
