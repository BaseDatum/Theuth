# 229 — Decks

Status: **Implemented (Vertical Slice)** on `feat/decks`.

## 1. Product Contract & Motivation

A **Deck** is a first-class, collaborative live workspace document in Theuth — combining:

- **Collaborative visual sections** (`json_render`, `note`, strict-CSP `html`) independently created and edited by humans and agents with stable IDs, optimistic concurrency (`expected_rev`), per-section revision history, attribution, and focus presence.
- **Review-gated Monty execution** backed by the exact same session runtime as normal agent sessions (reusing `agent::repl` pool/checkout construction, live `ToolRegistry::Invoke` & `ListTools(session)`, full configured MCP namespace injected as async SDK functions, tool risk/approval gates, secret expansion/redaction, and `.monty` persistence + `rebase_dump_stubs`).
- **Scheduled collection AND persistent streaming** on portable bounded entrypoints (`on_start(last_state)`, `on_step()`, `collect()`, `on_action(action, args, ctx)`), keeping a supervised checkout and globals alive even with zero views open.
- **Interactive UI actions** entering the SAME live checkout via declared JSON-Schema actions, returning per-caller private results while broadcasting shared frames only when Python explicitly calls `deck_emit(...)`.
- **Full-page views and reference-only whiteboard cards** sharing the same live subscription and renderer, with zero whiteboard Y.Doc churn on streaming/snapshot repaints.

Decks are native to Theuth — NOT `crates/plugins`, with no `plugin.yaml`, no external process SDK, and no user build toolchain.

---

## 2. Architecture

### 2.1 Strict Separation of Collaborative State vs. Executable / Runtime State

| Plane | Storage | Authority | Mutated by |
|---|---|---|---|
| **Visual / Collaborative Document** | `DocKind::Deck` in `crates/document` (pure `yrs::Doc` root maps: `meta`, `sections`, `order`, `aliases`, `script_refs`) | `DocumentActor` | Owner UI & Agent `deck_section_*` / `deck_focus` tools |
| **Executable Scripts & Review Gate** | SQLite `deck_script_revisions` in `crates/decks` (`pending_review` / `approved` / `rejected` / `superseded`) | `DeckSupervisor` + owner-only `deck_script_approve` RPC | Propose: Owner or Agent; Approve/Reject: Owner RPC only |
| **Durable Runtime State & Snapshots** | SQLite `deck_meta`, `deck_snapshots`, `deck_state_checkpoints`, `deck_run_log`, `deck_action_idempotency` | `DeckOwner` (single runtime owner per Deck) | `DeckOwner` / Monty cooperative builtins |
| **Transient Live Frames & Push** | In-memory `DeckHub` bounded per-Deck subscription (`deck_subscribe`) with coalescing & rate/size caps | `DeckHub` | `DeckOwner` (`deck_emit(durable=False/True)`) |

**Security invariants**:
- Visual CRDT edits to `DocKind::Deck` can NEVER modify executable Python or self-authorize a script revision. `script_refs` in the Y.Doc stores read-only metadata pointers (`active_rev`, `pending_rev`, `mode`, `schedule`, `updated_at`) only.
- Script creation or modification ALWAYS creates a `pending_review` revision. Starting/running a Deck refuses unapproved revisions; only the owner-only `deck_script_approve` RPC transitions a revision to `approved`.
- High-frequency transient live frames NEVER touch the global `RuntimeWire` (only coarse `DeckChanged` events on `decks.changed@1` are published on metadata/section/script transitions) and NEVER mutate any whiteboard `Y.Doc`.

### 2.2 Collaborative Document Schema (`crates/document/src/deck/mod.rs`)

- Root `Y.Map`s: `meta`, `sections`, `order`, `aliases`, `script_refs`.
- Each section in `sections[id]` has stable ULID/hex ID, human alias (`s1`, `s2`, ...), `kind` (`json_render | note | html`), `title`, `content`, `col_span` (`1..=12`), optional `data_key`, `archived`, monotonic `rev`, `created_at/by`, `updated_at/by`, and bounded per-section `history` (`Y.Array`, capped at 50 revisions).
- Ordering uses fractional float ranks in `order[section_id]`: moving one section updates a single key in `order` without rewriting sibling sections.
- Optimistic CAS: `SectionPatch` and `SectionArchive` accept optional `expected_rev` and return structured `DeckDocError::Conflict(SectionConflict)` on mismatch.

### 2.3 Monty Session Runtime & Single-Owner Execution (`crates/decks/src/{runtime,owner}.rs`)

- Each Deck receives its own persisted `SessionId` and isolated `.monty` checkout directory (`decks/<deck_id>/checkout.monty`), reusing `agent::repl` (`open_default_pool`, `session_repl_config`, `session_mounts`, `session_tool_specs`, `find_session_tool`, `rebase_dump_stubs`, `tool_result`, `os_answer`).
- **Full tool parity**: Every `ToolCall` / `ListTools` runs directly through `ToolRegistry::Invoke` and `ToolRegistry::ListTools` with the Deck's `SessionId`, exposing the full configured MCP namespace as async Python SDK functions (`mcp__<server>__<tool>` / `tools()`), normal tool policies/approvals, and secret expansion + output redaction (`tools::secrets::redact` / `redact_value`).
- **Portable bounded entrypoints & cooperative builtins**:
  - `on_start(last_state)` — cold start / state restoration entrypoint.
  - `on_step()` — bounded streaming iteration (never one infinite feed; respects worker/inproc clamps).
  - `collect()` — scheduled / one-shot collection entrypoint.
  - `on_action(action, args, ctx)` — interactive action handler on the SAME live checkout.
  - Cooperative host builtins: `deck_emit(payload, durable=False, section=None)`, `deck_checkpoint(state)`, `deck_sleep(seconds)`, `deck_state()`, `deck_last_snapshot()`.
- **Warm restore + cold fallback**: On startup/restart, `DeckCheckout` attempts warm `.monty` heap restore + `rebase_dump_stubs` when the saved `script_rev` and `code_sha256` match; on mismatch or corruption, it falls back cleanly to a fresh checkout and calls `on_start(last_state)` with the last durable JSON checkpoint.
- **Periodic clean-boundary rollover**: After `CHECKPOINT_ROLLOVER_STEPS` clean step boundaries, the checkout dumps heap state, refreshes tool stubs, and rolls cleanly without leaking memory.
- **Single-flight serialization & fencing**: One `DeckOwner` task per Deck owns the checkout exclusively, serializing `on_step`, scheduled `collect`, and `on_action` calls at cooperative boundaries. Every run transition bumps `run_epoch`; stale frames/checkpoints from superseded epochs are fenced out.
- **Failure / stale semantics**: Collection or step failures preserve `last_ok_snapshot_id` and mark `runtime.stale = true` with `last_error` and `consecutive_failures` without clobbering the last-good snapshot.
- **Tool approval pause surfacing**: `DeckApprovalWatcher` subscribes to `ApprovalRequested` / `ApprovalResolved` on `RuntimeWire` for Deck sessions, immediately transitioning runtime state to `waiting_approval`, pushing `DeckRuntimeEvent::ApprovalPending` to `DeckHub` subscribers, and resuming once resolved via standard `resolve_approval`.

### 2.4 Interactive Actions & Stream Controls

- **Declared actions only**: Every script revision declares `declared_actions: Vec<DeclaredAction>` with `name`, `title`, `description`, JSON `schema`, `mutates_state`, and `allow_while_paused`.
- `deck_interact` validates: (1) action is declared, (2) `args` match JSON Schema via `jsonschema::validate`, (3) optional `expected_rev` matches active approved revision (`revision_mismatch`), (4) `request_id` idempotency cache in SQLite (`deck_action_idempotency` prevents replay on retry/disconnect), (5) bounded queue/busy/timeout handling, and (6) `allow_while_paused` when paused.
- **Privacy**: `on_action` return values are returned ONLY in the caller's `InteractResponse` (`private: true`); other viewers see shared updates ONLY when Python explicitly invokes `deck_emit(...)`.
- **Stream lifecycle**: `start` (approved rev only), `pause` (retains live checkout & globals, stops stepping), `resume` (wakes existing checkout), `stop` (checkpoints and releases checkout), `run_once` (bounded one-shot execution).

### 2.5 Hybrid UI, Strict CSP Sandbox, Navigation & Whiteboard Integration

- **Navigation**: `/decks` and `/decks/$id` inside normal harness chrome, registered in `nav.ts`, `view-catalog.ts`, `view-title.ts`, and `tileview.rs` (`show_view` / `show_deck` split & popout support). Collapsed left rail has **exactly ONE** `DecksRail` button opening a popover listing recent Decks (capped at `POPOVER_CAP`), `New deck`, `Workspace Pulse (Example)`, and `All decks →`.
- **Native interactive catalog (`crates/surface`)**: Added `ActionButton`, `StreamControls`, `ResultBanner` with strict schema parity in `crates/surface/src/spec.rs` and `crates/desktop/ui/src/components/chat/surface/{catalog.ts,registry.tsx}`. Outside `DeckInteractContext` (ordinary chat fences), all three render disabled/inert (`data-deck-inert="true"`).
- **Strict Deck HTML sandbox**:
  - Rust (`crates/desktop/src/pages.rs`): `PageProfile::DeckStrict` (`profile: "deck"`) serves `theuth-page` responses with `Content-Security-Policy: sandbox allow-scripts; default-src 'none'; connect-src 'none'; form-action 'none'; base-uri 'none'; object-src 'none'; frame-src 'none'; img-src data:; font-src data:; style-src 'unsafe-inline'; script-src 'unsafe-inline'`. Standard artifact pages remain untouched.
  - UI (`DeckHtmlSection.tsx` + `deck-html-bridge.ts`): `<iframe sandbox="allow-scripts">` (NO `allow-same-origin`, NO `allow-forms`), strips `<base>` and author CSP `<meta>` tags, injects locked-down CSP meta + narrow per-iframe nonce `window.theuthDeck` bridge (`onData`, `action`, `control`). Host `parseDeckGuestIntent` strictly rejects anything outside declared intents (no arbitrary tool/MCP/RPC calls).
- **Whiteboard live card (`CardKind::Deck`)**:
  - `CardKind::Deck` is `in_place = true`, `fits_content = false` in both Rust (`crates/document/src/wb/model.rs`) and UI (`canvas/ops.ts`, `canvas/fit.ts`).
  - `Source::Deck { deck, section, snapshot_id }` stores references ONLY — zero script, HTML, or snapshot bytes in the whiteboard Y.Doc.
  - `DeckCardBody` renders the shared `DeckView` (`embedded`) backed by `useDeckLive`; streaming repaints update React state inside the card with zero whiteboard Y.Doc writes, zero card versions, and zero card resizing/movement.

### 2.6 Agent Tools & Onboarding Skill

- 15 native tools in `crates/decks/src/tools.rs`: `deck_list`, `deck_create`, `deck_read`, `deck_section_add`, `deck_section_patch`, `deck_section_move`, `deck_section_archive`, `deck_section_history`, `deck_focus`, `deck_script_propose`, `deck_script_read`, `deck_script_run`, `deck_runtime`, `deck_interact`, and `show_deck` (`DIRECT = true`, opens `/decks/<id>` via `artifact::layout::call`).
- Bundled onboarding skill `crates/decks/decks.md` installed on first run (`crates/theuth/src/main.rs`) guiding agents to interview the user, discover real existing MCP/tools/secrets/OAuth connections, label synthetic example data honestly, and propose scripts for explicit owner review.

---

## 3. As-Built Implementation Map

- **Document & Whiteboard**: `crates/document/src/deck/mod.rs`, `crates/document/src/{meta.rs,lib.rs,actor.rs,hub.rs}`, `crates/document/src/wb/{model.rs,place.rs,engine.rs,outline.rs}`, `crates/artifact/src/tileview.rs`.
- **Surface Catalog**: `crates/surface/src/{spec.rs,validate.rs,catalog.rs,prompt.rs,render.rs,export.rs,slack.rs,lib.rs}`, `crates/surface/fixtures/json-render/valid-deck-controls.json`, `crates/surface/tests/golden/valid-deck-controls-w{60,100}.txt`.
- **Agent REPL Extraction**: `crates/agent/src/repl.rs` (pub(crate)/pub helpers for pool, mounts, tool specs, stub rebasing, tool results, OS answers, and `test_pool`).
- **Decks Engine Crate**: `crates/decks/src/{lib.rs,model.rs,store.rs,hub.rs,runtime.rs,owner.rs,example.rs,rpc.rs,tools.rs}`, `crates/decks/decks.md`.
- **Desktop Sandbox & App Wiring**: `crates/desktop/src/pages.rs`, `crates/theuth/src/main.rs`, `crates/theuth/Cargo.toml`, `crates/theuth/tests/wire_exposure.rs`.
- **Desktop UI**: `crates/desktop/ui/src/components/decks/*`, `crates/desktop/ui/src/components/sidebar/{Decks.tsx,Decks.test.ts,AppSidebar.tsx}`, `crates/desktop/ui/src/components/chat/surface/{deck-context.ts,catalog.ts,registry.tsx}`, `crates/desktop/ui/src/components/whiteboard/{add.ts,CardSheet.tsx,OutlineView.tsx,canvas/model.ts,canvas/ops.ts,canvas/cards/bodies.tsx}`, `crates/desktop/ui/src/lib/{pages.ts,nav.ts,view-catalog.ts,view-title.ts}`, `crates/desktop/ui/src/routes/{decks.tsx,decks_.$id.tsx}`.

### 3.1 Security & Runtime Hardening (As-Built)

- **RPC & subscription authorization (B1)**: Every `DecksApi` method and `deck_subscribe` enforces `owner_only` (authenticated, non-closed owner peer); security-sensitive transitions (`deck_script_approve`, `deck_script_reject`, `deck_runtime_control`) additionally enforce `human_press` (rejecting share guests, engine-internal peers, closed peers, and `Forwarded` calls).
- **Protected internal state & fail-closed path resolution (B2)**: `tools::seat::real` anchors relative paths and resolves components left-to-right (failing closed `None` on empty/root escape). `tools::seat::refuse_internal_state` and `ToolRegistry` hard-refuse all writes into `<data_dir>/decks` (including `<nonexistent>/..` traversals), with post-`create_dir_all` re-checks in `WriteFile::call`.
- **Canonical revision & `.monty` integrity (B2, B3)**: Every `ScriptRevision` persists and recomputes `code_sha256` and canonical `revision_sha256` over `(code, declared_actions, mode, interval_secs, schedule_cron)` on every SQLite load and approval. Warm `.monty` restore additionally verifies the actual `.monty` file bytes SHA-256 against `monty_dump_sha256`, deleting tampered files and falling back cleanly to cold `on_start(last_state)`.
- **Digest-bound, pending-only, manual-only approval (B3)**: `deck_script_approve(deck_id, rev, expected_digest)` runs inside `BEGIN IMMEDIATE`, requires `status = 'pending_review'` (prior pending proposals are atomically marked `superseded` on new proposals), verifies `expected_digest == revision_sha256`, stops any prior revision, and leaves `schedule_enabled = 0, stream_enabled = 0` until explicit owner `Start`. Agent `deck_runtime` restricts `op` to `AgentRuntimeOp::{Pause, Stop, RunOnce}` (no `start`/`resume`).
- **Deadlock-free supervision & re-entrancy isolation (B4)**: `DeckSupervisor` reads `GetDeck`/`ListDecks`/`SubscribeDeckMsg` from a lock-free `LiveDeckCache` without awaiting `DeckOwner`, sends `RevisionApproved` non-blockingly, and excludes `DIRECT_ONLY`, `deck_*`, and `show_deck` from Deck scripts while keeping full normal session & MCP tool parity.
- **Caller-scoped idempotency privacy (B7)**: `deck_action_idempotency` is keyed by `(deck_id, caller_id, request_id)` and checks `(action_name, args_sha256, script_rev)` so one caller never reads another caller's private result and mismatched key reuse returns `InteractErrorCode::Conflict`.
- **Safe rollover & cold recovery (B8)**: If a rollover heap dump exceeds `DUMP_HARD_BYTES`, `maybe_refresh_or_rollover` returns `Err("rollover_dump_too_large")` instead of installing an empty VM, prompting `DeckOwner` to clear `.monty` metadata and cold-start `on_start(last_state)`.
- **Per-Deck isolation, lifecycle & boot recovery (H1–H4, H9–H10, M1–M3, L1, L3)**: Each Deck runs with isolated `<data_dir>/deck-work/<deck_id>` as `cwd`/`write_root` and per-session `SetApprovals { enabled: true }` pinned; phantom Decks are rejected; `DocumentChanged { deleted: true }` tears down SQLite, `.monty`, spill, work dir, and seat before broadcasting `Closed { reason: "deck_deleted" }`; startup awaits actor/watcher/tool registration, honors `decks.enabled`, resumes previously enabled Decks (`RecoverEnabledDecksMsg`), and wires `DecksPlugin` into `cli.acp`; all script/emit/stdout/action-return/interval caps and `checked_add`/`checked_sub` arithmetic are enforced; concurrent tool approvals queue per Deck and restore the exact pre-approval `RuntimeState`; `rebase_dump_stubs` uses sentinel-delimited managed blocks (`STUBS_BEGIN`/`STUBS_END`) supporting both `async def` and sync `def`.

---

## 4. Honest Remaining Limitations & Follow-Ups

1. **Cron expression parsing**: `interval_secs` scheduled polling is implemented and tested; full cron expression evaluation (`schedule_cron` field is persisted and surfaced in metadata) currently falls back to `interval_secs` (default 60s) when `interval_secs` is unset. Adding a lightweight cron evaluator (`croner`/`cron`) is a clean follow-up.
2. **Real-time Yjs section sync in Desktop UI**: Full-page Deck views currently load sections via `deck_get` and refresh on `decks.changed@1` events (with per-section optimistic CAS via `deck_section_patch`), rather than opening a live browser `Y.Doc` provider for character-level note typing. The backend `DocKind::Deck` already lives in `DocumentActor` with full `yrs` updates, so binding a live `y-provider` in `DeckView` is incremental.
3. **Whiteboard "+ Add Deck" picker UI**: Agents and tests can place `CardKind::Deck` cards on whiteboards via `wb_place` / `addCard`, and whiteboards render and open them live; adding a dedicated "+ Add → Deck" modal picker in the whiteboard cluster toolbar is a small UI follow-up.
4. **Script editor UI**: Owners can review, approve, approve-and-start, and reject proposed script revisions in the Deck UI, and agents/RPC can propose new revisions; an inline CodeMirror script editor in the Script drawer for direct human Python authoring is a follow-up.
5. **External SaaS / OAuth discovery wizard**: The bundled `decks.md` skill guides agents to use existing Theuth MCP/secrets/OAuth flows; an interactive one-click integration discovery wizard is optional follow-up per the product contract.
