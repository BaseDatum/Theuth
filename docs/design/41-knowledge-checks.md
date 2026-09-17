# 41 — Knowledge checks: memory that tests itself against the world

Status: draft v0 · 2026-09-17 · extends `10-knowledge.md` §2.2 (frontmatter),
§3 (index), §4 (hints), §5.3 (harvest instruction), §6 (tools), §8
(surfaces); `33-loops.md` §3.1 (the shell-check idiom); `25-desktop.md` §8
(knowledge explorer); `00-architecture.md` §8 (TUI pickers). Where this doc
disagrees with 10, this doc wins; §13 lists the edits. Facts about the tree
(`crates/knowledge/src/{store,types,lib}.rs`, `crates/schedule/src/lib.rs`,
`crates/tui/src/model.rs`, `crates/rpc/src/api.rs`) were read 2026-09-17.

## 0. Why

The store (10) is built to be right at harvest time and has no notion of
being wrong later. `timestamp` is its only temporal field; nothing in 10
says *stale*, *verified* or *confidence*. Yet what harvest actually writes
is operational fact about the user's world — *cargo is at
`~/.rustup/toolchains/…/bin`*, *staging Postgres is `pg.internal:5433`*,
*the rewrite lives in `~/Develop/rewrite`* — and those rot: paths move,
hosts are renamed, versions bump, services stop. A wrong memory is worse
than none: it arrives as a `<knowledge>` hint with the harness's authority
and the model acts on it without checking, because the hint told it the
harness already knows.

Every long-lived assistant has this failure. The usual answers — a TTL, a
confidence score, an LLM asked "is this still true?" — all *guess*. This
harness has a shell, a clock and a memory in one process, and the memory
format has a slot for arbitrary keys. So: let a concept carry the command
that proves it, run the command, and mark the concept when it fails. The
model then re-establishes the fact instead of acting on it. The user sees
which memories are rotten and can fix the memory, fix the check, or hand
either to an agent.

## 1. Decisions in one screen

1. **One frontmatter key, `check:`, a shell command.** Exit 0 = the concept
   still holds. Same idiom as 33's `until:` (`$SHELL -lc`, cwd, timeout,
   2 KiB tail) with a 30 s cap: a fact check is `test -x`, `dig`, `pg_isready`,
   `git -C … rev-parse`, not a build. `okf::Frontmatter` preserves unknown
   keys byte for byte, so every other OKF consumer ignores it. Rejected:
   a `checks/` sidecar (splits a concept in two files), a structured
   `verify: {kind, target}` schema (a command already is the universal
   schema).
2. **Files are the truth; results are derived.** The command lives in the
   file. The result (when, exit code, output tail) lives in
   `knowledge.sqlite` beside the FTS rows — deletable, rebuilt by running
   the checks again. The bundle's git history never records a check *run*.
3. **The model proposes, the user confirms.** A check written by harvest or
   `knowledge_put` lands as `check_proposed:`; only the user promotes it to
   `check:` (a key in the TUI picker, a button in the explorer, or the raw
   editor). The sweep never runs a proposed check. A check is a command the
   harness will run unattended every day; the grant has to be the user's,
   for the same reason the `schedule` tool is `Mutating` (34 §6). Rejected:
   a classifier over the command text (28 §0.4's reasoning), `Risk`-gating
   each proposed check at harvest time (harvest is a dead-end branch with
   no turn to ask in).
4. **A daily sweep, sequential, in the plugin.** One `tokio::time::interval`
   of 24 h in `KnowledgePlugin::start`, first pass 60 s after open, checks
   run one at a time. No schedule file (it would be a harness-owned file in
   the user's directory), no new actor (the store is the actor).
5. **The mark is one suffix on the hint line.** `(stale: check failed
   2026-09-17)`. That word changes the model's behaviour from *act* to
   *re-establish*; it then `knowledge_put`s the corrected concept, and the
   next run clears the mark. Passing checks add nothing to the hint (no
   `(verified)` chatter). Proposed, never-run checks add nothing either.
6. **Harvest writes checks, and reconciles stale concepts.** The instruction
   (10 §5.3) gains one clause asking for a check when the fact was
   established by a command, and the `{related}` block flags stale
   concepts so the model fixes them while the evidence is in the frame.
7. **The user can always see, run, edit and delegate.** Every list of
   concepts shows a chip (`✓` passed · `✗` failed · `?` proposed or never
   run · nothing when no check). Opening a concept shows the command, runs
   it on demand with exit code and output, lets the user edit the command,
   or starts a new session with a brief that links the concept and quotes
   the check and its last result so an agent updates the knowledge or the
   check. Same four actions in the TUI and the desktop app.
8. **Nothing without a check changes.** Decisions, preferences and
   procedures have no command that proves them and get none; they also do
   not rot the same way. The ceiling is stated: this covers the operational
   layer — paths, hosts, ports, versions, repos, tools, services — which is
   most of what harvest writes and all of what the model acts on blindly.

## 2. The keys

```markdown
---
type: fact
title: wth's Rust toolchain lives under rustup, not on PATH
description: cargo/rustc are at ~/.rustup/toolchains/stable-aarch64-apple-darwin/bin
tags: [rust, toolchain, wth]
scope: /Users/Sky.Moore/Develop/wth
source: harvest
timestamp: 2026-09-10T09:41:03Z
check: test -x ~/.rustup/toolchains/stable-aarch64-apple-darwin/bin/cargo
---
```

| key | written by | meaning |
|---|---|---|
| `check` | the user (raw editor, TUI `e`, explorer Edit, Accept on a proposal) | confirmed; the sweep runs it; `knowledge_verify` may run it |
| `check_proposed` | harvest, `knowledge_put{check}` | a candidate the user has not accepted; shown with `?`; runnable only by the user from a UI, where the command is on screen first |

Both may be present at once (a confirmed check plus a newer proposal).
Accept replaces `check:` with the proposal and deletes `check_proposed:`.
Reject deletes `check_proposed:`. Every promotion is a normal write: one
commit, `KnowledgeChanged { source: user }`.

Rules:

- A check is one line, ≤ 1 KiB, non-empty after trim. Anything else is a
  parse warning on that key only (the concept still indexes) and the chip
  reads `✗ invalid check`.
- `cwd` for the run is `scope` when set and it is a directory, else `$HOME`.
- `knowledge_put` (which rebuilds frontmatter, 25 §2.3) **carries an
  existing `check:` forward unchanged**; with a `check` argument it writes
  `check_proposed:` beside it. Without this rule every model update of a
  concept would silently drop the user's check.
- `knowledge_write` (raw text) writes whatever the text says; it is the
  user's editor.

## 3. Running a check

```rust
// knowledge/src/check.rs — the `until_check` shape (schedule/src/lib.rs:206-225) with a shorter cap
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(30);

pub struct Outcome { pub at: DateTime<Utc>, pub exit: Option<i32>, pub tail: String, pub took_ms: u32 }
pub async fn run(cmd: &str, cwd: &Path) -> Outcome   // $SHELL -lc, stdin null, stdout+stderr, tail 2 KiB
```

`exit == Some(0)` → passed. `Some(n)` → failed. `None` → timeout or spawn
failure, recorded as failed with `tail = "no exit in 30s"` /
the io error. The runner is shared by the sweep, `knowledge_verify` and
the UI's run action, so all three produce the same row.

Output is scrubbed of vault values before it is stored or shown
(`tools::secrets::redact`, 37 §3.4) — a check like
`curl -u user:{{secret:tok}} …` is legal, and its output must not leak the
value into the index or a card. Placeholders in the command are expanded at
run time by the same hook (37 §3.4); a check with a placeholder shows the
placeholder in every UI, as tool arguments do.

## 4. The sweep

In `KnowledgePlugin::start`, after the actor is registered:

```
sleep 60 s
loop:
  for id in concepts with a confirmed check, by id:
    if a run for id is in flight (UI or tool) → skip
    outcome = run(check, cwd)
    store.record(id, outcome)                 // §5
    publish KnowledgeChecked { id, passed, at }
  sleep 24 h
```

Sequential, so a bundle with 200 checks takes at most 200 × 30 s on the
worst day and a few seconds on a normal one. Not on the write mutex: a check
is I/O outside the bundle. A check whose concept file vanished mid-sweep is
skipped (the next `sync` deletes its row). `knowledge.enabled: false` means
no plugin, so no sweep.

Rejected: running checks at hint time (a 30 s command in the model's
critical path); at turn start with `sync` (same); through a schedule file
(the harness writing into the user's `schedules/`).

## 5. Index

One new table beside `concepts` and `links` (`store.rs:38-44`), created by
`open_db` with the same `IF NOT EXISTS`; the index stays migration-free
(10 §3.1):

```sql
CREATE TABLE IF NOT EXISTS checks (
    id       TEXT PRIMARY KEY,   -- concept id
    cmd      TEXT,               -- `check:` as of the last sync; NULL when only a proposal exists
    proposed TEXT,               -- `check_proposed:`
    at       TEXT,               -- RFC 3339 of the last run of `cmd`; NULL = never
    exit     INTEGER,            -- last exit code; NULL with `at` set = no exit (timeout / spawn failure)
    tail     TEXT,               -- last 2 KiB of output, scrubbed
    took_ms  INTEGER
);
```

`sync` (10 §3.2) upserts `cmd`/`proposed` from the file for every changed
row and **clears `at/exit/tail/took_ms` when `cmd` changed** — a new command
has no history. A concept with neither key has no row. Rows whose concept
row is deleted go with it.

`Meta` (`types.rs:62-80`) gains:

```rust
pub check: Option<Check>,

pub struct Check {
    pub cmd: Option<String>,        // confirmed
    pub proposed: Option<String>,
    pub state: CheckState,          // Passed | Failed | Unrun | Proposed   (derived: cmd+exit==0 → Passed; cmd+at+exit!=0 → Failed; cmd+no at → Unrun; proposed only → Proposed)
    pub at: Option<String>,
    pub exit: Option<i32>,
    pub tail: Option<String>,       // present on `knowledge_get`; `None` on list/search rows to keep them small
    pub took_ms: Option<u32>,
}
```

`Meta` is what `knowledge_list`, `knowledge_search` hits and `knowledge_get`
all return, so every surface gets the chip from the type it already has.
Bindings via `just ts`.

## 6. Hints and harvest

### 6.1 Hint line (10 §4.1)

```
- `facts/wth-toolchain` — wth's Rust toolchain lives under rustup, not on PATH [fact] (stale: check failed 2026-09-17): cargo/rustc are at …
```

Only `Failed` adds a suffix. The ranker (10 §3.3) is unchanged: a stale
concept still surfaces — the point is that the model sees it *and* the
mark, not that it disappears. The lead sentence of the note gains four
words: *"call knowledge_get before relying on one; re-establish a stale
one before acting on it"*.

### 6.2 Harvest instruction (10 §5.3)

Two additions to `HARVEST_INSTRUCTION`:

- In the concept template, after `scope`:
  ```
  check: <cmd>          # only for a fact a command can test: exit 0 iff it still holds, prints nothing on success, finishes in seconds (test -x, dig, pg_isready, git -C … rev-parse). Omit for decisions and preferences.
  ```
  and one sentence in the prose: *"When you established a fact by running
  a command, the check is usually that command made silent."*
- `{related}` lines carry the same `(stale: …)` suffix as hints, and the
  prose gains: *"A related concept marked stale has a check that failed;
  if this conversation shows what is true now, update it (same id); if the
  check itself is wrong, propose a corrected one."*

`parse_harvest` (10 §5.5) reads `check:` from the emitted frontmatter and
writes it as `check_proposed:` (§1.3). Nothing else in the harvest path
changes; the branch is still one call on the cached frame.

### 6.3 Compaction head (10 §7)

The `Runtime:` part lists stale concept ids that were hinted this epoch,
one line: `Stale knowledge hinted: facts/wth-toolchain, entities/pg-staging`.
Fixed shape, only when non-empty.

## 7. Tools (10 §6)

| tool | Risk | args | returns |
|---|---|---|---|
| `knowledge_verify` | ReadOnly | `{ id?: String }` | no id: one line per concept with a confirmed check, `id · passed\|failed exit N · at`, stale first; with id: runs the **confirmed** check now and returns `passed` / `failed exit N` + the tail. `Err("check not confirmed: ask the user to accept it")` when only a proposal exists; `Err("no check")` otherwise |
| `knowledge_put` | ReadOnly (unchanged) | + `check?: String` | writes `check_proposed:`; reply ends `· check proposed (the user accepts it in /knowledge)` |
| `knowledge_get` | unchanged | | `Concept.meta.check` filled with the tail |

`knowledge_verify{id}` is `ReadOnly` because the command it runs is one the
user confirmed for unattended execution; the model running it on demand is
the sweep running early. A proposed check is not runnable through the tool:
the model that wants the answer runs the same command through `shell` and
takes the gate.

Prompt block (`knowledge` `RegisterPrompt`, 10 §4.3) gains one sentence:
*"A `(stale: …)` hint means the concept's check failed: find out what is
true now, then update the concept. When you record a fact you established
by running a command, give it a `check`."*

## 8. RPC (`crates/rpc/src/api.rs`)

```
knowledge_check      { id: String }                                 → Check      run the confirmed check, or the proposal when `proposed: true` (UI only: the command is on screen), record, return the row
knowledge_set_check  { id: String, check: Option<String> }          → Meta       write `check:` (None deletes it and any proposal); user promotion path; Accept = set_check(id, proposal)
knowledge_list / knowledge_search / knowledge_get                   Meta gains `check` (§5); `knowledge_list` gains `stale?: bool` filter
```

`knowledge_check` and `knowledge_set_check` `ask` the `"knowledge"` actor
(`Check { id, proposed }`, `SetCheck { id, check }`). Event
`knowledge::KnowledgeChecked { id, passed: bool, at }` on the stream — the
plugin's own struct, published by the sweep and by every run, no session.

The brief a UI sends to a new session (§9.3) is plain `new_session` +
`send_input`; no new method.

## 9. Surfaces

### 9.1 TUI (`crates/tui`)

`/knowledge` and `/knowledge <query>` become a **picker**, `Menu::Knowledge`
(the `Menu::Artifacts` shape, `model.rs:1119`, `ui.rs:820`), replacing the
static card of 10 §8. Rows:

```
✗ facts/wth-toolchain      — wth's Rust toolchain lives under rustup [fact]   failed 2026-09-17
✓ entities/pg-staging      — staging Postgres [entity]                        passed 3h ago
? facts/relay-port         — relay listens on 8787 [fact]                     proposed
  decisions/kameo-bus      — kameo over ractor [decision]
```

Chip column first (text glyphs, 00 §8: never emoji); the result column on
the right shows `passed <ago>` / `failed <date>` / `proposed` / `never run`.
Status line: `knowledge  ⏎ show  c check  e edit check  a accept  f fix with an agent  s stale only  esc close`.

| key | does |
|---|---|
| `⏎` | `Command::KnowledgeShow(id)` → `knowledge_get` → an `Entry::Info` card: header `id · title [type]`, then a `check` section when present — `$ <cmd>` (dim `proposed` tag when unconfirmed), `exit N · 0.3 s · 2026-09-17 09:12`, the tail in a fence — then the body through the markdown path. The picker stays open. |
| `c` | `Command::KnowledgeCheck(id, proposed: bool)` → `knowledge_check`. The row shows a braille spinner in the chip column until `KnowledgeChecked` (or the reply) lands; then an `Entry::Info` card `⚙ check <id>`: `$ <cmd>`, `exit N · took`, tail in a fence — the tool-card shape (00 §8) so a check looks like what it is, a command that ran. |
| `e` | Puts `/knowledge check <id> <current cmd>` in the editor through the `recall` hook (29 §0.7 `/edit` uses it) and closes the picker; Enter runs the command (below); Esc cancels. |
| `a` | On a `?` row: `knowledge_set_check(id, proposal)`; info line `accepted check for <id>`; the chip becomes `?` never-run until `c` or the sweep. On a row with no proposal: no-op. |
| `f` | `Command::KnowledgeFix(id)` (§9.3); opens the new session as `/sessions` does. |
| `s` | Toggle: list only `Failed` / `Proposed` rows (`knowledge_list { stale: true }`). |

Commands (`Model::knowledge_cmd`, first word decides, 10 §8):

| input | does |
|---|---|
| `/knowledge check <id> <cmd…>` | `knowledge_set_check(id, Some(cmd))` → info `check set for <id>`; the picker's `e` lands here |
| `/knowledge check <id>` | `knowledge_check(id)` → the `⚙ check` card |
| `/knowledge uncheck <id>` | `knowledge_set_check(id, None)` |
| `/knowledge stale` | the picker with `s` on |
| `/knowledge fix <id>` | §9.3 |
| `history`, `restore`, `harvest` | unchanged |

`KnowledgeChecked` events from the sweep are **not** interjected in the
transcript (the daily sweep would spam it); the picker re-reads on open.
A failure found by the sweep shows up the next time the model hints it, in
the picker, and in the explorer.

### 9.2 Desktop (`crates/desktop/ui`, 25 §8)

- **Tree.** Each node with a check gets a shadcn `Badge` after the `type`
  badge: `✓` (success colour) · `✗` (danger) · `?` (muted, tooltip
  *proposed by the model — accept to run it daily* or *never run*).
  Directory nodes show `✗ N` when any child is stale. Tree header gains a
  *stale* toggle (same filter as `s`).
- **Document header card** gains a **Check** section under the meta row,
  present when the concept has either key:
  - the command in mono; a `proposed` chip when unconfirmed, with
    **Accept** and **Dismiss** buttons (`knowledge_set_check(id, proposal)`
    / delete the proposal via the raw text: `knowledge_write` with the line
    removed — no separate RPC);
  - **Run** → `knowledge_check(id, proposed?)`; spinner on the button;
    result line `exit 1 · 0.3 s · just now`; output in a collapsible mono
    pane (open by default on failure, closed on success);
  - **Edit** → the command becomes an input inline (`⌘S`/Enter saves via
    `knowledge_set_check`, Esc cancels); **Remove** → `set_check(id, None)`
    after a confirm Dialog;
  - **Fix with an agent** → §9.3, then `navigate({ to: '/c/$session' })`.
- **Document body** unchanged; the raw editor already shows and saves both
  keys (it is the user's editor).
- `KnowledgeChecked` invalidates `['knowledge','list']` and the open
  document, like `KnowledgeChanged`. A toast only for a run the user
  started; sweep results are silent.

### 9.3 Fix with an agent — the brief

Both UIs do the same two calls: `new_session()` then `send_input(session,
FIX_BRIEF)`. `knowledge::fix_brief(concept) -> String` builds it so the two
frontends do not carry two templates:

```
<knowledge_check id="facts/wth-toolchain" title="wth's Rust toolchain lives under rustup, not on PATH" state="failed" at="2026-09-17T09:12:03Z" exit="1" took="0.3s">
$ test -x ~/.rustup/toolchains/stable-aarch64-apple-darwin/bin/cargo
(no output)
</knowledge_check>

The check on the knowledge concept `facts/wth-toolchain` (/knowledge/facts/wth-toolchain) failed.
Read it with knowledge_get. Find out what is true now — run commands, look at the machine.
Then do one or both, and say which and why:
- the fact changed → update the concept with knowledge_put, same id, with the evidence;
- the check is wrong or brittle → propose a better one with knowledge_put's `check` argument
  (exit 0 iff the fact holds, silent on success, finishes in seconds); I will accept it.
Do not delete the concept without asking me.
```

`state="proposed"` variants swap the first line for *"has a check proposed
by the model that I have not accepted"* and the task for *"run it through
`shell`, decide whether it is a sound test of the fact, and either propose
a better one or tell me to accept this one"*. `state="passed"` /
`"unrun"` variants exist for the user who wants to revisit a concept
anyway: *"review whether this concept and its check are still right"*.

The path `/knowledge/<id>` is a link the desktop resolves (25 §8's
`resolveLink`); in the TUI it is plain text. The session is an ordinary
top-level session: the user watches, approvals apply, the result is a
`knowledge_put` the user sees as `KnowledgeChanged` and a `?` chip to
accept. Nothing is automatic past that point; the user asked for an agent,
not for a loop.

## 10. Config

```yaml
knowledge:
  enabled: true
  sweep: true        # false: no daily run; checks still run from the UI and knowledge_verify
```

One bool. The interval and timeout are constants (07 §14 "one knob";
36 §1: no reason to differ per install). Rejected: `check_every`,
`check_timeout`, a per-concept `check_every:` key (a check is cheap or it
is not a check).

## 11. Security

- **A check is unattended execution.** The only checks the sweep runs are
  ones the user wrote or accepted with the command on screen (§1.3). The
  model cannot promote; `knowledge_put` cannot write `check:`; the tool
  cannot run a proposal. The raw editor and `knowledge_write` can write
  `check:` — they are the user's editor, and an agent with `write_file`
  into `knowledge/` could too, under the gate, as it could write
  `schedules/` (33 §7). This doc does not pretend otherwise.
- **Output may contain secrets.** Scrubbed (§3) before the index, the bus
  and every card.
- **A check can be slow or hang.** 30 s cap, sequential, never on the
  write mutex or a turn's path.
- **The chip is not a proof.** `✓` means *the command exited 0 at that
  time*; a weak check passes for a wrong fact. The brief (§9.3) asks the
  agent to judge the check, not just the fact, and the `Accept` step keeps
  a human on every new command.

## 12. Failure modes

| failure | behaviour |
|---|---|
| check exceeds 30 s | `Failed`, `tail = "no exit in 30s"`; the sweep moves on |
| `$SHELL` missing | `Failed`, spawn error in the tail (same as `until`) |
| `scope` not a directory | cwd falls back to `$HOME`; noted in the tail's first line |
| concept file edited so `check:` changes | `sync` clears the result; chip `?` never run until the next sweep or a manual run |
| `knowledge.sqlite` deleted | `checks` rebuilt empty at open; every chip `?` until the first sweep (60 s) |
| both `check:` and `check_proposed:` present | chip reflects the confirmed one; the proposal is shown beside it with Accept |
| a run in flight from the UI while the sweep reaches the id | the sweep skips it |
| `knowledge_put` on a concept with a confirmed check, no `check` arg | `check:` carried forward, result kept (the command did not change) |

## 13. Edits to other docs (on build)

- **10 §2.2** key table: `check` / `check_proposed` rows (§2). **§3.1**: the
  `checks` table. **§3.2**: the upsert/clear rule. **§4.1**: the suffix and
  the lead sentence. **§5.3**: the two clauses. **§5.5**: `check` →
  `check_proposed`. **§6**: `knowledge_verify`, `knowledge_put.check`.
  **§7**: the head line. **§8**: RPC rows, TUI picker, config `sweep`.
  **§9** cache table: `KnowledgeChecked | index only | sweep / UI / tool |
  no effect on the frame` and `(stale) suffix | hint line | when Failed |
  append`. **§14** rejected: TTL decay, confidence, LLM-judged verification
  (each guesses; a command knows).
- **25 §8**: the badge, the Check section, Fix with an agent.
- **00 §8** TUI: `/knowledge` is a picker. **§10** `knowledge/` line:
  `+ check: sweep, marks, knowledge_verify (41)`. **§12** phases: row 28.
- **`harness.md`**: the two keys, `knowledge_verify`, the stale hint, how to
  accept a check.

## 14. Plan and checks

Each step ends with something runnable (00 §12), not a suite.

1. **`types.rs` + `store.rs`**: `Check`, `checks` table, sync upsert/clear,
   `record`, `Meta.check`. Unit: a file gaining/changing/losing `check:`
   round-trips through `sync` into the expected row; a changed `cmd` clears
   the result. `cargo test -p knowledge`.
2. **`check.rs` + sweep + `knowledge_verify` + `knowledge_put.check`**.
   Check, headless on a scratch data dir with `sweep: true` and the first
   pass shortened by env for the test: a concept with `check: test -d /tmp`
   → `passed`; `check: test -d /nope` → `failed exit 1`; `check: sleep 40`
   → `failed`, `no exit in 30s`; `knowledge_put{check}` → file has
   `check_proposed:`; `knowledge_verify{id}` on it → `Err(check not
   confirmed)`; `knowledge_put` without `check` on a checked concept → the
   `check:` line survives.
3. **Hints + harvest**: a failed concept hinted with `(stale: …)`; a real
   harvest on a turn that ran `which cargo` emits `check:` → stored as
   `check_proposed:`; the compaction head lists the stale id.
4. **RPC**: `knowledge_check`, `knowledge_set_check`, `stale` filter,
   `KnowledgeChecked` on the stream, bindings (`just ts`).
5. **TUI**: the picker, six keys, `/knowledge check|uncheck|stale|fix`. On a
   pty: `✗` row → `c` → `⚙ check` card with `exit 1`; `e` → editor
   prefilled → Enter → `check set`; `?` row → `a` → accepted; `f` → a new
   session whose first card is the brief and whose reply calls
   `knowledge_get`.
6. **Desktop**: badge, Check section, Fix with an agent. In the webview:
   Run shows the output pane; Edit saves; Accept flips `?`→ never run;
   Fix navigates to the new session with the brief as the first bubble.
7. Doc edits (§13), `harness.md`, results into §15.

Estimated: `knowledge` ≈ 250 lines (`check.rs` 60, store 80, types 30,
tools 40, harvest 20, sweep 20), `agent` ≈ 20 (hint suffix, head line),
`rpc` ≈ 40, `tui` ≈ 180, desktop ≈ 200.

## 15. Status

Not started.
