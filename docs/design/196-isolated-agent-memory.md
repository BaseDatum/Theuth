# 196 ISOLATED-MEMORY: phase 1 (draft)

## The ask
Sky, 2026-11-16 19:03: "phase one of the design is just an agent that can do everything, just has a completely segregated memory. Don't overcomplicate it. no separate tools filesystem isolation or anything like that, no allow-list for topics, etc. that can be added in later. the value is in being able to create persistent agents that will not pollute the main harness memory used for everyday life stuff."

## What exists today
Design 67 §9 puts every concept in **one** knowledge store, each with an `owner` and a `visibility` (private, team or shared).
- Plain sessions (yours and mine) read everything, by design.
- An agent reads its own concepts, its reporting line's `team` concepts and everything `shared`, which includes all of yours.
- Other agents' private concepts show up only as stubs.

Nothing keeps an agent's memory out of the main store, and the recon found that changing the visibility filter alone would leak. Get, history, revisions, documents, dreaming and sync all bypass `sees()`.

## Phase 1

**1. One setting.** `isolated_memory: true` in `agents/<name>.md`, false by default. (`memory:` is already the context mode, epochs or endless.) You set it on the hire card, in the desktop hire wizard and on the agent's page, and it shows as a badge.

**2. Its own memory store.** Each isolated agent gets an ordinary knowledge bundle: `<data_dir>/agents/<name>/knowledge/` plus its own `knowledge.sqlite`. It has the same OKF files, store, search, git history and tools. The main bundle never contains it, never indexes it and has no trace of it in its git history.

**3. One selector.** For each session: which agent is it, and is that agent isolated? If yes, use that agent's bundle; otherwise use main. Every knowledge entry point uses it:
- the per-turn hints;
- all `knowledge_*` tools;
- the harvest when a desk episode closes;
- `doc_*` on `knowledge:<id>`;
- records written by studies the agent starts.

The agent's subagents and other children inherit it.

**4. Fail closed.** If a session belongs to an isolated agent but its bundle can't be resolved (identity not ready yet, or the bundle lives on another machine), its knowledge tools refuse with a reason. They never fall back to main.

**5. The main memory stays clean.**
- Isolated bundles never appear in main hints, search, lists, history or the Knowledge page. That holds by construction, because they are separate stores.
- Nightly dreaming skips isolated agents' desk journals, sessions and cases. Today it copies every journal.
- An isolated agent sees nothing of the main memory: no hints from it, no `shared` items and no stubs in either direction.

**6. Looking at it.** The agent's page in Agents gets a read-only **Memory** tab (list, then open a concept), through RPC that names the bundle explicitly. Your everyday sessions never get it.

**7. Switching.**
- *Hire with isolated memory:* it starts empty.
- *Turn it on for an agent that already has notes in main:* refused, with the reason. Hire a new isolated agent instead; moving existing notes out of main is a later option.
- *Turn it off:* the bundle is parked and unread, and the agent returns to main memory with a fresh context. Turning it on again resumes the parked bundle.

## Not in phase 1
- **Files and tools:** isolated agents keep every tool, per your ruling. Anything with `read_file` or `shell` can still open either store, so this keeps memory separate but is not a security sandbox.
- **Topic allow-lists, sharing between memories, moving existing agents' notes.**
- **Mesh sync of isolated memory:** it lives on the agent's home machine. Elsewhere, point 4 applies (refuse, never fall back).
- **Chat transcripts:** the agent's chats stay in the sidebar, and explicit past-chat search still finds them. Only dreaming and harvest are kept from extracting them into main.

## Change map (recon 196, file:line in its report)
- `crates/knowledge`: a bundle registry keyed by agent; route the actor, tools, hints, harvest and RPC through the selector (`lib.rs`, `store.rs`, `tools.rs`, `harvest.rs`, `rpc_concepts.rs`); the main sync stays main-only.
- `crates/roster`: the `isolated_memory` field (`file.rs`, `types.rs`, `tools.rs`); the `Scoper` passes the isolation flag; switching rules; the closing prompt says the memory stays the agent's own.
- `crates/app/src/agent_draft.rs`: a hire-card field.
- `crates/dreaming`: exclude isolated journals, sessions and cases (`ledger.rs`, `cases.rs`, `dry_run.rs`, `signals.rs`).
- `crates/document`: `knowledge:` aliases resolve through the selector.
- `crates/autoresearch`: a study records into its originating bundle.
- Desktop: hire wizard field, badge and the agent Memory tab (`components/agents/*`).

## Tests
- Routing: an isolated agent's put, get, search, hints and history touch only its bundle.
- No path from main (including plain sessions) reaches an isolated concept.
- Fail-closed when the bundle can't be resolved.
- Dreaming inputs exclude isolated journals, sessions and cases.
- The switching rules.
- The field survives propose, put, the hire card and the file round trip.
- The UI badge and Memory tab.

## Build plan (after "build it")
Contract first, then two omp workers:
- **engine:** knowledge, roster, dreaming, document, autoresearch;
- **desktop:** fields, badge, Memory tab.

Then review, the full gates (the whole vitest suite and the Rust crates touched), and a scratch-engine proof that an isolated agent's notes never reach a plain session. About 2-3 hours.

## Contract

# Design 196 ISOLATED-MEMORY phase 1: CONTRACT (binding for the engine and desktop workers)

Sky approved the phase-1 design on 2026-11-16 at 19:21 ("go for it") with its three defaults:
- isolated memory is local to its home engine (no mesh sync);
- enabling isolation on an agent that already has notes in main is refused (no migration);
- chat transcripts are not segregated, except that dreaming and harvest never ingest an isolated agent's sessions.

## 1. Agent field
- `roster::AgentFile.isolated_memory: bool` uses `#[serde(default)]` and `#[ts(optional)]`, so the generated TS is `isolated_memory?: boolean` and existing UI code compiles unchanged.
- In agent-file frontmatter it renders as `isolated_memory: true` only when true. The parser accepts `true` and `false`, and absent means false. Do NOT reuse `memory:`, which is the epochs/endless context mode.
- `roster::AgentSummary.isolated_memory: bool` (`agents_list`), also `#[ts(optional)]`.
- The native tools `agent_propose`, `agent_put` (`AgentProposeArgs`) and `agent_draft` (`app::agent_draft::AgentDraft`) accept an optional `isolated_memory: bool`, default false. The chat hire card shows a "Memory: isolated" line when it is true, and the approval keeps the field.

## 2. Enabling, disabling, removal
- Enabling means `agent_put` (RPC or native tool) with `isolated_memory: true` when the current file has it false or absent, or when the name is new. If the MAIN bundle holds one or more concepts with `owner: agent:<name>`, refuse:
  - JSON-RPC error code -32602, message exactly `isolated_memory: <name> already has <N> notes in the main memory; hire a new agent with isolated memory instead (moving existing notes is not supported yet)`;
  - the native tool returns the same text as its error.
- Disabling (true → false) is always allowed. The bundle is parked unchanged and nothing reads it. The agent's next desk episode starts a fresh model context.
- Re-enabling resumes the parked bundle; the main-notes check above applies again.
- `agent_remove` of an agent that has a bundle moves `<data_dir>/agent-memory/<name>/` to `<data_dir>/agent-memory/.removed/<name>-<unix_secs>/`. Nothing is deleted. A later hire with the same name starts empty.

## 3. Storage
- Each isolated agent has one ordinary knowledge bundle: `<data_dir>/agent-memory/<name>/knowledge/` (OKF files, its own git repo) plus `<data_dir>/agent-memory/<name>/knowledge.sqlite`.
- It is opened with the existing `Knowledge` store machinery (root parametrised; same FTS, git history and tools) and created lazily on first use.
- NOT under `<data_dir>/agents/`, where the roster keeps the agent `.md` files.
- Not replicated. The main `theuth/knowledge/1` sync stays main-only and must never carry these files.

## 4. The selector (engine-internal; binding for every entry point)
- For each session: which agent is it (roster)? If that agent has `isolated_memory: true`, use its bundle. Otherwise use main, with today's design-67 §9 visibility unchanged.
- It applies to:
  - the per-turn knowledge hints;
  - every knowledge model tool (search, get, list, put, delete, history, stats, share, restore, and any other `knowledge_*` tool);
  - the harvest when a desk episode closes;
  - `doc_*` tools on `knowledge:<id>` documents;
  - autoresearch records written for studies started from that agent's sessions;
  - any theuth-side knowledge call made on behalf of an ACP or external child of such a session.
- Descendants (subagents, children, ACP children) inherit their root desk's resolution, and that resolution survives an engine restart.
- Inside an isolated bundle every concept is the agent's:
  - `knowledge_share` returns the error `isolated memory cannot be shared`;
  - no stubs are produced;
  - nothing from main is visible, and no main hints.
- **Fail closed:** a session that belongs to, or descends from, an isolated agent but cannot open that bundle on this engine (identity not yet resolved, bundle on another machine) gets the error `isolated memory unavailable here: <reason>` from its knowledge tools, and its hints are empty. It never falls back to main.
- **Main side:** plain sessions, theuth and non-isolated agents can never reach an isolated bundle through any model-facing tool or hint.

## 5. Dreaming and the desk closing prompt
- Main dreaming skips:
  - isolated agents' desk journals (`<data_dir>/roster/journal/<name>.md`);
  - sessions belonging to isolated agents, as ledger, signals or candidates;
  - case ingestion from those sessions.
  This covers dry-run too.
- An isolated agent's closing prompt does not ask it to share with its team. It says its memory stays its own.

## 6. RPC for the desktop (owner audience only)
- `knowledge_search(query, limit, bundle?)`, `knowledge_get(id, bundle?)` and `knowledge_list(prefix, type, tag, limit, bundle?)` gain a trailing optional `bundle: string | null`:
  - `null` or absent means main (unchanged);
  - `"agent:<name>"` means that agent's isolated bundle, read-only. A parked bundle (isolation now off) is still readable.
  - An unknown agent, or one that never had isolated memory, returns -32602 `unknown bundle: <bundle>`.
- The four existing desktop call sites (`AttachPicker.tsx`, `knowledge/Document.tsx`, `SearchBox.tsx`, `Tree.tsx`) must still type-check. If the generated client makes the new parameter required, the engine worker passes `null` at those four sites and touches nothing else in the UI.
- RPC writes (`knowledge_put`, `write`, `delete`, `restore`) stay main-only in phase 1.
- `crates/mcp-server` catalog: unchanged and main-only. Do not expose `bundle` there.

## 7. Desktop (consumes 1, 2 and 6)
- **Toggle:** "Isolated memory" in the hire wizard and in AgentDetail's runtime fields, with the help text `Keeps its own memory. What it learns never reaches your main memory, and it doesn't see yours.` Saving calls `agent_put`; a refusal error is shown verbatim next to the toggle.
- **Badge:** "Isolated memory" on Agents list rows and in the AgentDetail header, when true.
- **Memory tab:** in AgentDetail, shown only when `isolated_memory` is true. A read-only list (`knowledge_list` with bundle), a search box (`knowledge_search` with bundle), and a read-only concept view (`knowledge_get` with bundle) that reuses the existing knowledge document rendering.
- **Unchanged:** the main Knowledge page stays main-only. No other screens change.

## As built

### Engine implementation

- `isolated_memory` is an independent, serde-defaulted agent boolean. Frontmatter emits it only when true; summaries, proposals, drafts and draft approval receipts preserve it. Draft cards say `Memory: isolated`. Generated TypeScript makes the field optional.
- The main knowledge handle caches lazy, ordinary knowledge stores rooted at `agent-memory/<name>/`. Each has its own `knowledge/` git repository and `knowledge.sqlite`; ownership and visibility are fixed to that agent/private. The main actor, sync loop and MCP catalog remain main-only. Isolated knowledge events are not published on the shared runtime.
- Migration 66 persists `(agent, isolated_memory, home_engine)` in `sessions.knowledge_identity`. Root identity is immutable; descendant resolution walks durable parents, and forks copy the identity. Unknown, parked, removed and non-home isolated identities fail closed. The selector trait requires an explicit implementation; it has no default-to-main method.
- Mode changes pause/drain and cancel the old tree, retire its root, stop its local document editor and park its journal before a fresh context is used. Enabling checks main-owned notes and returns the exact refusal. Disabling parks the existing store; re-enabling resumes it after the same check. Removal closes the store and moves the whole directory into `.removed/<name>-<unix-seconds>/`, including when the knowledge plugin is off.
- Isolated knowledge documents use unregistered local document actors and separate runtime event buses. Reads, direct document IDs, edits and notebook citations resolve the selected bundle; edits await export and reconcile only that original bundle. They do not enter the global document index or mesh replication.
- Dreaming excludes durable isolated roots, descendants and forks even after retirement, plus their journals, hint/call/session counters, rewind signals, pending/earlier rewinds and saved/replayed cases. Research writes and playbooks select from the study's saved origin.

### Entry-point map

Paths are repository-relative. Native tool rows select through `Knowledge::for_session` before doing any work.

| Entry point | Routing |
| --- | --- |
| Session selector | `crates/knowledge/src/store.rs:699`; durable roster resolution `crates/roster/src/lib.rs:3708`; identity persistence/walk `crates/agent/src/store.rs:1356,1375` |
| `knowledge_search` | `crates/knowledge/src/tools.rs:127` |
| `knowledge_get`, including revisions | `crates/knowledge/src/tools.rs:182` |
| `knowledge_list` | `crates/knowledge/src/tools.rs:261` |
| `knowledge_put` | `crates/knowledge/src/tools.rs:305` |
| `knowledge_share` | `crates/knowledge/src/tools.rs:356`; selected isolated stores refuse sharing |
| `knowledge_delete` | `crates/knowledge/src/tools.rs:382` |
| `knowledge_history` | `crates/knowledge/src/tools.rs:417` |
| `knowledge_restore` | `crates/knowledge/src/tools.rs:460` |
| `knowledge_stats` | `crates/knowledge/src/tools.rs:494` |
| `knowledge_merge` | `crates/knowledge/src/tools.rs:544` |
| `knowledge_supersede` | `crates/knowledge/src/tools.rs:594` |
| `knowledge_archive` | `crates/knowledge/src/tools.rs:636` |
| `knowledge_verify` | `crates/knowledge/src/tools.rs:731` |
| `knowledge_retitle` | `crates/knowledge/src/tools.rs:769` |
| Turn hints and harvest | `crates/agent/src/loop_.rs:4896,8280`; capture the selected handle, suppress hints on resolution failure |
| Sensor knowledge bridge | `crates/agent/src/sensor/bridge.rs:435`; invokes the selected native search tool |
| Grouped `search` knowledge/history | `crates/rpc-compose/src/search.rs:577,761`; session-selected stores, fixed group order retained |
| Knowledge document read and edits | `crates/document/src/tools.rs:180,234,643`; selected aliases/direct IDs; local actors at `crates/document/src/memory.rs:62` |
| Notebook knowledge citations | `crates/document/src/notebook.rs:1293`; agent principals use the same router |
| Research record/playbook | `crates/autoresearch/src/record.rs:200,243`; saved `study.origin` |
| Owner RPC search/get/list | `crates/knowledge/src/rpc_concepts.rs:74,87,95`; explicit inspection selector `crates/knowledge/src/store.rs:724`; absent/null is main, `agent:<name>` selects that enabled/parked bundle, unknown is `-32602` |
| Other owner knowledge RPCs | `crates/knowledge/src/lib.rs:615,623`; retain the single explicitly main actor/store |
| MCP search/get/list | `crates/mcp-server/src/catalog.rs:738,745,749`; unchanged schemas and positional calls omit bundle, therefore main |
| Background sync | `crates/knowledge/src/lib.rs:630`; only main |
| Dreaming ledger/journals/rewinds | `crates/agent/src/store/dreaming.rs:103`; `crates/dreaming/src/ledger.rs:85,112,287` |
| Dreaming counters and signals | `crates/agent/src/store.rs:1990`; `crates/dreaming/src/dry_run.rs:174,184,185,230`; `crates/dreaming/src/signals.rs:338` |
| Dreaming cases | `crates/dreaming/src/cases.rs:64,109`; durable provenance exclusion `crates/dreaming/src/isolation.rs:5` |
| Enable/park/resume/remove | `crates/roster/src/lib.rs:3117,1913,3269`; bundle close/move `crates/knowledge/src/store.rs:785` |

### Regression coverage and gates

New Rust coverage: three knowledge isolation tests (native read/write/revision/history/maintenance paths, sharing, ordinary-session invisibility, owner RPC defaults/parked/unknown bundles, removal/rehire and unavailable identities/bundles); durable retired/restarted/forked lineage; roster refusal/park/resume/removal; fail-closed document aliases; origin-attributed research records; and two dreaming exclusion/signal tests. Existing frontmatter/card tests and the registered grouped-search test were extended; ordinary test fixtures now install an explicit main selector.

All logs are in `/tmp/196e/`.

| Gate | Observed result |
| --- | --- |
| Frozen UI/apps/documents dependencies and compiled helpers | Passed; document helper self-test passed |
| Locked `export_bindings`, wire generation and root/UI binding copies | Passed; export test 1 passed |
| UI pre-change contract / head `tsc -b --noEmit` | 0 / 0 errors |
| Full `vitest run src` | 349 files, 2,435 tests passed, including disclosure |
| Four allowed UI callsites wording scan | 0 matches; no UI source edit was needed because generated trailing bundle parameters are optional |
| Rustfmt | Engine touched files formatted; the three mechanical TUI edits retain their original legacy formatting to avoid unrelated whole-file churn |
| Final nine-crate default-parallel Rust run | 2,037 passed, 5 failed, 4 ignored; `rust-gates-parallel-final.log` |
| Supplementary serialized nine-crate Rust run | 2,023 passed, 19 failed, 4 ignored; app 50/50, document actor 12/12 and rpc-compose 65/65 passed; `rust-gates-final.log` |
| Latest `isolat` selection after making the selector mandatory | 11 passed, 0 failed; `selector-tests.log` |
| TUI formatting-only cleanup | Nine-crate test build passed; exact agent-picker test 1 passed; `tui-format-compile.log`, `tui-format-test.log` |
| Clippy, eight engine crates, `--no-deps -- -D warnings` | Passed; `selector-clippy.log` |
| Full clippy / added TUI no-deps clippy | Blocked by diagnostics in unchanged `voice/src/history/mod.rs:465` (`chunks_exact_to_as_chunks`) / `tui/src/components/mcp.rs:24` (`large_enum_variant`); no suppressions added |
| Final locked `BUILD_SLOTS=1` engine build | Passed; `engine-build-final.log` |

The five default-parallel Rust failures are the brief's known `repl::tests::capping_binds_out_name` failure (also fails alone with `--exact`), three app helper timeout/protocol assertions in unchanged `crates/app/src/helper.rs:516,529,618`, and the TUI emoji-source guard reporting unchanged `crates/agent/src/rpc_jobs/history.rs:434,462,470`. The app helper tests all passed in the serialized run. That serialized run instead exposed 17 inbox cases reporting `actor not running` from the existing registry-ask path (`crates/agent/src/communication/network.rs:3493-3504`); those cases passed in both default-parallel runs. These gates are not reported as clean.

Verification deviation: the pre-change UI wire contract was reconstructed from the current export by removing only the new `isolated_memory` properties and the three trailing `bundle` parameters. It was not a second checkout of main. The committed wire fixture was stale for unrelated APIs; the reconstructed baseline and actual head both typechecked with zero errors (`tsc-pre196.log`, `tsc-head.log`). No behavioral contract deviation is intended. The required Rust TUI caller/summary edits are mechanical. A formatter initially rewrote their legacy formatting; a separate cleanup commit restores the original formatting while retaining only the three trailing `None` arguments and the summary boolean.

### Scratch proof

`/tmp/196e/proof.md` records the loopback-only, fake-model commands/results, safe configuration before first start, file-log `rpc listening` readiness, native call receipts, hint positive control, main git history, restarts and process cleanup. The first scratch fake incorrectly skipped the genuine user's `<time>` prefix; it was corrected. A receipt assertion was corrected from `body` to the RPC's existing `text` field. Both failed attempts are retained; neither required an engine change.

All eight requested proof steps passed, plus an isolated document read/edit followed immediately by `knowledge_get` and owner RPC inspection. Isolated search returned no main hits/stubs; the plain proof session had no isolated hits or hints; default RPC search returned nothing while `bundle: "agent:iso-proof"` returned the private note. The same desk UUID reopened after restart. The legacy enable refusal matched exactly, disabling created a fresh context while owner inspection still read the parked bundle, re-enabling resumed it, and removal preserved it at `/tmp/196e/eng/agent-memory/.removed/iso-proof-1794863892/`.

Both managed engines exited 0; both fake service instances were stopped. A final process-table check found none of the nine captured engine/fake/worker/document-helper PIDs remaining. Every finite build/test command finished. No live checkout, other engine, push or merge was used.

This is a knowledge-bundle boundary, not a transcript or raw-filesystem sandbox. Phase 1 does not replicate isolated bundles or add bundle-aware MCP tools.
