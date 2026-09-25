# 67 — As built, worker A: `crates/roster`, knowledge owner/visibility, MCP allowlist

*Branch `feature/67-agents-roster`, 2026-09-24. Contract: `docs/design/67-contract.md` §6; design: `67-agents.md` §6, §9, §11, §12.*

## Commits

### 1 — `feat(roster): agents directory, rows, the built-in theuth agent`
- `file.rs`: besides the contract's `parse`/`load_dir`: `render(&AgentFile)` (front matter with only non-default keys; `parse(render(f)) == f`), `set_key(text, key, Option<value>)` (rewrites one front-matter line byte-for-byte elsewhere, the schedule crate's `set_enabled` shape), `write_atomic(dir, name, text)` (dotfile temp + rename), `default_agent(data_dir)` (theuth synthesised: body = `<data_dir>/prompt.md` trimmed, a one-line stand-in when empty; `reports_to: user`, `tools: destructive`, `mcp: []`, `budget: none`).
- `types.rs`: added `AgentName::theuth()`, `AgentName::is_default()`, `AgentStatus::Invalid`, `AgentSummary.error: Option<String>` (serde default, TS optional).
- `store.rs`: `upsert_agent`, `delete_agent`, `agent_names`, `open_episode`, `close_episode`, `open_episodes`, `insert_proposal`, `delete_proposal`.
- `desk.rs` (new): `Shared`, the RwLock read model other plugins consult synchronously: files by name, `theuth`, agent → desk session, session → agent (desk + descendants via `inherit`), `line(agent)` (reporting line up and down, incl. `theuth` and `user`).
- `lib.rs`: `Roster::load` → `reconcile()`: `load_dir` → `Shared::set_files` + `agents` rows (valid files only; a parse error keeps the last good row and shows as an `Invalid` summary; a stem that is not a valid agent name, or `theuth.md`, is skipped with a warning). Watcher = schedule idiom (300 ms debounce → `Reconcile`). Handlers: `ListAgents` (theuth first, then every file; invalid → `Invalid` row with `error`), `GetAgent` (theuth synthesised; `no such agent`), `PutAgent` (theuth refused with a clear error; render → parse validate → write; the read model is updated in place so the next `GetAgent` is current), `PauseAgent` (theuth refused; `set_key(paused)`), `ApproveAgent` (deletes the `proposals` row; refuses `not a proposal` when neither a row nor `proposed_by` exists; drops `paused` and `proposed_by` lines), `ProposeAgent { file, proposed_by } -> approval_id` (new message; refuses an existing or reserved name, writes the file paused, records the proposal).
- `tools.rs`: `agents_list` body (asks `ListAgents`; theuth excluded; `all` includes paused/proposed/invalid; text lines + structured rows), `agent_propose` body (proposer = the agent owning the calling session, else `theuth`; asks `ProposeAgent`).
- Tests: `file::{render_parse_round_trip, set_key_rewrites_one_line, default_agent_is_theuth}`, `desk::{line_walks_up_and_down, sessions_follow_the_desk}`, `lib::{scan_valid_invalid_and_theuth, put_round_trip_and_theuth_refused}`.

### 2 — `feat(roster): desks, episodes, journal`
- `desk.rs`: `manual(&AgentFile)` (the operating paragraph, below), `opening(file, journal)` (`<desk agent="…">` charter + journal + manual), `CLOSING` (the journal prompt), `wake_text(&Wake, Option<&Task>)` (67 §7.3), `read_journal`/`write_journal` (`<data_dir>/roster/journal/<name>.md`, overwrite, cut to `journal_max_bytes` at a char boundary), `Desk { session, episode, last_active_ms, working, primed, model }` with `idle_due(now_ms, idle_secs)`, `context_due(context, window, pct)`.
- `lib.rs`: `sync_desks()` after every reconcile and every `Tick` (60 s): a desk for `theuth` and every valid, unpaused, approved agent; desks of paused/removed/invalid agents are closed (`reason: paused`). `open_desk`: `agent::CreateSession { model }` (session-only model, config untouched) → `RenameSession` to `agent:<name>` → `PinSession` → `tools::SetCap` from `tools:` → `episodes` row → `Shared::set_desk`. `close_desk(name, reason)`: the desk stops taking wakes at once; a primed desk gets `CLOSING` as one input and is tracked in `closing` (its `TextDelta`s accumulate; `TurnFinished` → journal written → episode closed → session unpinned + archived → a fresh desk opens); an unprimed desk is put away directly; a closing turn that gives nothing within 10 min is finished without a journal. Rotation triggers: idle (`Tick`, `Desk::idle_due`), context (`TurnFinished.context` vs the model's window from `agent::ListModels`, cached), thread end (`SessionDeleted` of the desk → episode `thread_end`, new desk, no journal), `EpisodeClose { agent, reason }` (public message for the mesh lease handler / operator). At startup the previous run's open episodes are closed `restart` without a journal turn and fresh desks open.
- `DeskRouter` (`Desks`): `desk_for` = `Shared::desk`; `deliver` asks the actor `DeliverWake { agent, wake, task }`: the scoped wake text, headed by `opening(...)` when the episode has had no input yet (`primed`), as one `agent::SendInput`. `Err("no desk for <agent>")` while paused/closing/not open — the tasks plugin re-dispatches.
- `agent::AgentEvent` subscription: `TurnStarted`/`TurnFinished` (working flag, last activity, model), `TextDelta` (closing text), `SubagentSpawned` (child inherits the agent), `SessionDeleted`.
- Tests: `desk::{wake_text_snapshot, opening_has_charter_journal_and_disposition_rule, idle_rotation_uses_the_given_clock, journal_is_written_and_capped}`.

### 3 — `feat(roster): focus caps — tools ceiling and MCP allowlist`
- `tools:` → `tools::SetCap` on the desk root at `open_desk` (commit 2); children inherit by the registry's min-over-ancestors rule.
- `desk::mcp_visible(agent, server, tool) -> bool`: empty `mcp:` = no limit; an entry is a server (`grafana`) or `server.tool` (`argocd_prod.list_applications`).
- `Desks` implements the new `tools::McpAccess` (`allows(session, server, tool)`): the agent owning the session (desk or descendant, via `Shared`) → `mcp_visible`; unknown sessions and `theuth` → `true`. Installed with `tools::InstallMcpAccess` before the two tools are registered.
- Tests: `desk::{mcp_allowlist_forms, mcp_access_follows_sessions_and_children}` (both forms, inheritance to a child, theuth and no-agent sessions unaffected).

### 4 — `feat(knowledge): owner and visibility; scoped priming and search`
- `types.rs`: `Visibility { Private, Team, Shared(default) }` (`as_str`, `parse`), `USER_OWNER`, `Meta.owner: String` (serde default `user`), `Meta.visibility` (default shared), `Put.visibility: Option<Visibility>`, `AgentScope { owner, line }` with `sees(scope, owner, visibility)` and `may_share(scope, owner)`, trait `Scoper { scope_for(session) -> Option<AgentScope> }`.
- `store.rs`: FTS5 table gains `owner UNINDEXED, visibility UNINDEXED` (`META_COLS` shared by every select; an older index is detected by a probe in `open_db`, dropped and rebuilt by `sync`); `Indexed::new` parses the keys with the 67 §9 defaults; `upsert` writes them. `Knowledge.scoper` + `set_scoper`. `put_for(p, source, session)`: stamps `owner` (`agent:<name>` from the scope, else `user`) and `visibility` (`p.visibility`, else `private` for an agent session, `shared` otherwise); `put` = `put_for(.., None)`. `search_for`/`hint_for`/`list_for` take the session: own items rank first; items the scope may not read are stubs in search (`Meta { id, owner, visibility, title: "private — ask <owner>", type }`, empty description/tags/snippet, no body), dropped from hints and lists. `share(id, visibility, session)`: owner or its reporting line (no scope = the user, always allowed); rewrites the `visibility` key, commits `share <id> <vis>`.
- `tools.rs`: `knowledge_search`, `knowledge_list`, `knowledge_put` pass `cx.session`; new `knowledge_share { id, visibility }` (`ShareTool`, `Risk::ReadOnly` like `knowledge_put` so a `read_only` desk's closing turn can share).
- `lib.rs`: `RegisterScoper(Arc<dyn Scoper>)` message → `set_scoper`. The RPC `Search` stays unscoped (the user).
- Roster: `impl knowledge::Scoper for Desks` (owner `agent:<name>`, line = `Shared::line` in `agent:` form + `user`; `theuth` and unknown sessions → `None`); registered via `lookup_when::<KnowledgeActor>` + `RegisterScoper`.
- Harvest at episode close: the closing prompt tells the desk to `knowledge_put` (stamped `owner: agent:<name>`, `visibility: private` by the scope) and to `knowledge_share` what its team or everyone should know. The agent loop's own harvest path (`loop_.rs`) now calls `put_for(.., Some(session))`, so a desk's or work session's harvest is stamped the same way.
- Tests: `store::{existing_concepts_default_to_user_and_shared, agent_put_is_private_to_it_and_search_is_scoped, share_is_for_the_owner_and_its_line}`, `roster::lib::knowledge_scope_is_the_agent_and_its_line`.

### 5 — `docs(67): as-built A` (this document)

## The desk operating paragraph (verbatim, `desk::manual`)

> You are the desk of agent `{name}` ({title}). You take wakes, read your inbox with `task_inbox`, check out one task with `task_checkout`, open a child session for it with `spawn_subagent` exactly where the task says (`agent: "<harness>@<engine>"` from the task's `harness` and `where`, or the engine name alone for the native harness; pass `model` and `tools` from your file: model {model}, tools {tools}), review the result, then set the disposition with `task_update`. Never do the work in this session: no files, no shells, no browsing here, only through a child. Handle the task the wake names before any other; write progress with `task_note`; call `escalate` when you need a decision you may not make yourself, and wait for the answer. Disposition rule: an episode's closing turn must leave every task it checked out in `review | blocked | done(by reviewer) | delegated (child task exists) | continuing (a schedule, reminder or live child will wake it)`. A task left in `doing` with none of these is set `blocked: "no continuation"` and the manager is woken. Your `knowledge_put` writes are private to you by default; `knowledge_share` opens them to your team or everyone. When asked to write your journal, answer with the journal only.

Delivered inside `<desk agent="…"><charter>…</charter><journal>…</journal>{manual}</desk>` as the head of the episode's first input (see deviations). The closing prompt (`desk::CLOSING`): "Write your journal: Done / Open / Next / Notes, ≤ 8 KiB. Before you answer, record anything durable you learned this episode with `knowledge_put` (it is private to you by default) and call `knowledge_share` with `visibility: team` or `shared` for the items your team or everyone should know. Every task you checked out must already have its disposition. Answer with the journal text only: it is written to your journal file as is and is what your next episode starts from."

## The wake text (verbatim shape, `desk::wake_text`)

`Wake: task <id> — <title>. Reason: <reason>.[ (Raised N times.)] Do not switch to another task until this wake is handled.` then a blank line and `<task id state owner [reviewer] [project] [where] [harness] [thread]>brief[ Blockers: …][ External: …]</task>`. Without a task record: `Wake: task <id>.` / `Wake: no task.` and the same tail. The tasks crate passes no thread entries through `DeskRouter::deliver` today; the `thread` document id is on the tag for the desk to read.

## The tools-crate call site

`crates/tools/src/registry.rs`: `pub trait McpAccess { fn allows(&self, session, server, tool) -> bool }`, `pub struct InstallMcpAccess(pub Arc<dyn McpAccess>)` + handler, field `ToolRegistry.mcp_access: Option<Arc<dyn McpAccess>>` (initialised `None` in both constructors), and `fn mcp_hidden(&self, id, name) -> bool` (looks the tool up, `ToolSource::Mcp { server }` → the remote name is the registered name minus a `<server>__` clash prefix → `!allows`). Its one call site is the first line of `is_hidden`, which `ListTools`, `DescribeTools`, `visible` (`Invoke`/`AppInvoke`/`InspectTool`) and `HiddenTools` already consult — so a disallowed MCP tool is neither listed nor callable. `crates/tools/src/lib.rs`: re-exports `InstallMcpAccess, McpAccess`.

## The agent-crate attribute for "which agent owns this session"

None added. The agent crate has no such attribute (`sessions.agent` is the ACP harness key; `tags` belong to the title model). The roster owns it: `Shared.sessions: HashMap<SessionId, String>` — the desk session at `open_desk`, every descendant through `AgentEvent::SubagentSpawned { parent, child }` (`Shared::inherit`), forgotten on `SessionDeleted`. Survives a restart only for desks (the `episodes` table; fresh desks open anyway). The tools and knowledge plugins reach it through the `McpAccess` / `Scoper` hooks, never through a roster type.

## Edits outside `crates/roster`

- `crates/tools/src/registry.rs`, `crates/tools/src/lib.rs`: the allowlist hook above.
- `crates/knowledge/src/{types,store,search,tools,lib}.rs`: owner/visibility as in commit 4 (`search.rs`: the one `SELECT` uses `META_COLS`).
- `crates/agent/src/loop_.rs`: three call sites switched to the scoped variants — `hint_for(.., Some(self.shared.id))` (round hints), `hint_for(.., Some(session))` and `put_for(.., Some(session))` (harvest) — plus `visibility: None` on the harvest `Put` literal.
- `crates/rpc/src/export.rs`: `knowledge::Visibility` added to the TS export list.
- `crates/tui/src/model.rs`, `crates/tui/src/ui.rs`: two test `Meta` literals gained `owner`/`visibility`.

## Deviations

1. **No system-prompt override exists per session** (`frame::baseline` reads only `prompt.md` and static plugin blocks). The charter + journal + manual are delivered as the head of the episode's **first input** (the first wake), inside `<desk agent="…">`, not as a system prompt. No model turn is spent on opening; `primed` marks the transcript as journal-worthy.
2. **`replica::files::mirror` is absent** (the replica crate has no `files` module; `attach` answers `not built`). The plain watcher plus `Store::upsert_agent`/`delete_agent` is the `agents` mirror; nothing is attached to the replica.
3. **Desk effort is not applied**: `SetModel`/`SetEffort` with a session also rewrite the default and `config.yaml`; only `CreateSessionOptions.model` is session-only. The desk gets the file's `model`; `effort` waits for a session-only setter (67 §16 step 1 gap).
4. **Desk naming**: the title is `agent:<name>` via `RenameSession`; roots have no `name` column and `peer:<name>` (50) is a remote-peer alias, so `desk_for` (the session id) is the routing key.
5. **`ProposeAgent` mints the approval id** (uuid v7) and records the proposal; it does not file a `hire_agent` request with the approval actor (00 §1.7). `agent_approve` (RPC / Agents view queue) resolves it.
6. **RPC `agents_list` includes `theuth` first** (the Agents view needs the root of the tree, its desk and status); the `agents_list` tool excludes it as its description says.
7. **`AgentStatus::Invalid` and `AgentSummary.error`** added for files that do not parse (the brief's "row with an error status"). A file whose stem is not a valid agent name, or `theuth.md`, cannot be a row and is skipped with a warning.
8. **`PutAgent` re-renders the file** (comments and formatting of a hand-written file are lost; keys at their defaults are omitted); `PauseAgent`/`ApproveAgent` rewrite single lines and keep everything else byte-for-byte.
9. **Restart**: the previous run's open episodes are closed `restart` without a journal turn; the journal on disk is the one from their last proper close. New desks open (sessions accumulate as archived).
10. **Thread end** is taken as `SessionDeleted` of the desk session (no journal possible); `stop_reason` errors do not end an episode.
11. **`Blocked`/`Unreachable` status**: `Blocked` = every open task of the agent is `blocked` (via `tasks::ListTasks`; `not built` → 0 open tasks, `Idle`); `Unreachable` is never produced (needs the mesh).
12. **Team items outside the line are stubbed like private ones** in search (the same information leak level: existence and owner only).
13. **`knowledge_list` is scoped too** (drops what the caller may not read) though the brief named only search and hints — otherwise another agent's private titles would show there.
14. **`knowledge_share` is `Risk::ReadOnly`**, like `knowledge_put`, so a `read_only` desk can follow the closing prompt.
15. **`Search` RPC message** (the user's UI) stays unscoped: the user sees everything.

## Not built
- Budgets (67 §6.4): `spent_month` is `0.0`; no 80 %/100 % checks, no `budget_override`.
- `hire_agent` approval through the permission contract (see deviation 5).
- Proposals in the Agents view / `Unreachable` status / lease-following desks: UI and mesh workers; `EpisodeClose { reason: "lease" }` is the hook for the latter.
- Thread entries inline in the wake: the `DeskRouter::deliver` signature carries none; the `thread` id is on the `<task>` tag.
- No roster bus event on file changes (the Agents view would poll or a later worker adds one).

## Tests written (not run, per the rules)
`crates/roster`: 3 in `file.rs` (plus the contract's 3), 6 in `desk.rs`, 3 in `lib.rs`. `crates/knowledge/src/store.rs`: 3. Existing knowledge/tui tests updated for the new `Meta`/`Put` fields.

## `cargo check`
- Commit 1: `cargo check -p roster --tests` — 5 kameo `ask` result-shape errors, fixed before the commit was made (the fix compiled in commit 2's check).
- Commit 2: `cargo check -p roster --tests` — `Finished` (clean).
- Commit 3: `cargo check -p roster --tests` (compiles `tools` too) — `Finished` (clean).
- Commit 4: `cargo check -p roster -p knowledge -p tui -p rpc --tests` — one error, `expected trait, found struct Sync` at `knowledge/src/types.rs:85` (the crate's own `types::Sync` shadows `std::marker::Sync`); fixed to `Send + std::marker::Sync` before the commit. The check aborted at `knowledge (lib)`, so the roster/tui/rpc compile against the new knowledge surface is unverified by me — the orchestrator's build is the first to see it.
- `Cargo.lock` gained `figment` under `roster` (dev-dependency from commit 1); committed with commit 5.

## Fix round A2

`cargo test -p roster -p knowledge -p tools --no-fail-fast`: knowledge 30/30, roster 18/18, tools 153/153. `cargo check -p theuth`: `Finished` (clean).

1. **`knowledge` did not compile** — four `E0277: dynamic SQL strings should be audited` (sqlx 0.9 refuses `&String` in `query_as`). The four `META_COLS` selects (`search.rs` `CurrentIndex::build`; `store.rs` `get`, `list_for`, `ranked`) now use `sqlx::AssertSqlSafe(format!(..))`, the `crates/artifact/src/store.rs` pattern; only the const is interpolated, every value is bound. Commit `fix(knowledge): wrap META_COLS selects in AssertSqlSafe for sqlx 0.9`.
2. **`roster::tests::put_round_trip_and_theuth_refused`** — `agent_names()` was empty after `write_file` + `reconcile`. Root cause: `write_text` primed the read model, and `reconcile` writes an `agents` row only for files that differ from the read model, so a file written through `PutAgent`/`PauseAgent`/`ApproveAgent`/`ProposeAgent` never reached the mirror (the watcher's reconcile would have found it unchanged too). `write_text` is now async and upserts the row itself; the four handlers await it.
3. **`desk::tests::mcp_allowlist_forms`** — `parse("qa", "QA.")` → `empty charter`. gray_matter 0.3.2 returns empty `content` for input no longer than its `---` delimiter (3 bytes). `parse` takes the whole text as the body in that case.
4. **`file::tests::full_file`** — a test-literal bug: the `\n\` string continuations stripped the two-space indentation of the folded `capabilities: >` scalar, so the YAML failed. Rewritten with `concat!`; the parser was right. Not a weakening — the assertions are unchanged.

Commit `fix(roster): mirror PutAgent/Pause/Approve writes into agents; parse 3-byte charters` covers 2–4. No test was weakened; none contradicted 67 §6/§9.

DONE A
