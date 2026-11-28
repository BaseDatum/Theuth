---
name: decks
description: Design, build, and onboard collaborative live Decks (native json_render + restricted OpenUI + strict HTML + live Markdown note sections powered by Monty Python scripts with full session/MCP tools, sustained triggers, native mesh sources, and optional owner-granted per-Deck trust).
---

# Decks — Onboarding & Authoring Guide

Installed at `<data_dir>/skills/decks.md` by engine startup (`first_run`) and discoverable in `list_skills` / `load_skill("decks")` and the Skills UI.

A **Deck** is a native collaborative live document combining:
- **Visual sections** (`json_render`, `openui` [restricted OpenUI Lang, 64 KiB cap], `note` [safe Markdown with `{{path|default}}` live interpolation], and strict-CSP `html`) stored as stable-ID `Y.Map` entries in a `DocKind::Deck` CRDT document.
- **Executable data & actions** powered by a portable Monty Python script running inside an isolated Deck `SessionId` (`<data_dir>/deck-work/<deck_id>`) with the exact same live `ToolRegistry` & configured MCP namespace as a normal agent session.

## Available Agent Tools

- `deck_list`, `deck_create`, `deck_read` — discover, create (including `example=true` for the Workspace Pulse starter), and inspect Deck structure, runtime status, mesh home/source health, trust state, active/pending script, and last-good snapshot.
- `deck_section_add`, `deck_section_patch`, `deck_section_move`, `deck_section_archive`, `deck_section_history`, `deck_focus` — collaborative section authoring with optimistic CAS (`expected_rev`).
- `deck_script_propose`, `deck_script_read`, `deck_script_run` — propose Monty script revisions (including optional `declared_actions` and sustained `triggers`), read active/pending/historical revisions, or run one bounded step (`run_once`) on an **approved** revision.
- `deck_runtime` — control lifecycle with `pause`, `stop`, or `run_once` only (`start` and `resume` require explicit human owner control).
- `deck_interact` — invoke a declared interactive action (`on_action`) with JSON Schema validation and caller-scoped idempotency.
- `show_deck` — open `/decks/<id>` in the desktop workspace UI.

*(There is intentionally NO agent tool to approve scripts, move mesh home ownership, or enable/revoke Deck trust.)*

> **Unsupported session artifacts**: Decks intentionally do **not** embed session-only `mermaid`, `cells`, or whiteboard-in-Deck adapters. Use `json_render`, `openui`, `note`, or strict-CSP `html` sections instead.

## Onboarding Interview (When Building a New Deck)

Before writing a Deck script, interview the user briefly and inspect real capabilities:
1. **Goal & Audience**: What decisions, metrics, or workflows should this Deck track? Which sections should be native `json_render` (KPIs, tables, charts, buttons), restricted `openui` (stateful OpenUI Lang controls bound to `deck_data`/`deck_status` and declared actions), collaborative `note` (safe live Markdown), or custom strict-CSP `html` visualization?
2. **Real Data Sources (Never Fake Connected Access)**:
   - Call `tools()` or inspect configured MCP servers / existing session tools to see what is actually connected.
   - If a desired source (Linear, Jira, GitHub, Slack, Google, database, etc.) is NOT configured yet, guide the user through the existing MCP/OAuth/secret setup flows rather than fabricating fake live numbers.
   - If the user wants a prototype/demo first, clearly label every synthetic number as **Example Data** (or scaffold with `deck_create(example=true)`).
3. **Cadence & Interactivity**:
   - Scheduled one-shot (`mode="oneshot"` with `schedule_cron` or `interval_secs`) vs. persistent streaming (`mode="streaming"` or `mode="hybrid"` with cooperative `deck_sleep(seconds)`).
   - Which interactive buttons/actions should be declared (`declared_actions` with strict JSON Schema `schema`)?
4. **Owner Review vs. Permanent Per-Deck Trust**:
   - **Default (Reviewed Mode)**: `deck_script_propose(...)` enters `pending_review`. Open the Deck with `show_deck(deck_id)` and ask the owner to inspect the diff/digest and click **Approve** or **Approve & Start**. Tool calls follow standard session approval prompts (`approvals=true`).
   - **Optional Permanent Trust ("Trust this Deck")**: If the owner does not want repeated script/tool approval cards for a specific Deck, the owner can enable **Trust this Deck** inside the full-page Deck UI (`/decks/<id>`) on the Deck's authoritative home engine.

## Permanent Per-Deck Trust (Autonomous Execution Mode)

- **Owner-only, home-only, machine-local**: Trust can ONLY be enabled by the actual owner via `human_press` in the full-page Deck UI on the Deck's authoritative home engine (`deck_trust_set` RPC — never from non-home viewers, embedded whiteboard cards, agent tools, guest/internal/closed/forwarded RPC, raw Yjs CRDT edits, or HTML sections). Grants are stored in local SQLite (`deck_trust_grants` + immutable `deck_trust_audit`), never replicated in CRDT, and never transferred on `deck_move_home`. Each mesh source node runs locally and requires its own separate local grant if autonomous execution is desired there.
- **What Trust enables**:
  - Future script revisions proposed via `deck_script_propose` auto-approve immediately and are logged in `deck_trust_audit` with their canonical `revision_sha256` and `code_sha256` hashes.
  - Enabling trust **preserves current activity** — it never silently starts a paused or stopped Deck. Future auto-approved revisions follow the Deck's previous enabled stream/schedule state (a paused/stopped Deck stays stopped until the owner starts it; an active streaming/scheduled Deck transitions seamlessly to the new revision).
  - The Deck session switches to `approvals=false` so normal read/mutating/MCP tool calls execute without repeated approval cards.
- **Exact Non-Bypassable Safeguard Boundary**:
  - Trust uses `ToolRegistry`'s existing session `approvals=false` semantics — it is **not** blanket root privilege.
  - Non-bypassable safeguards remain enforced: `human_only` tools, `always_ask` policy rules (global/session `always_ask`, secret-bearing/protected-config/`add_mcp_server` writes), hard risk-cap denials (`Deny`), isolated per-Deck working directory / write root (`deck-work/<deck_id>`), blocked re-entrant `deck_*` & `DIRECT_ONLY` tools, automatic secret redaction, and OS-level permissions.
  - Routine approvals remain governed separately by routines (`skills/routines.md`).
- **Immediate Revocation**:
  - Clicking **Revoke trust** immediately cancels any in-flight tool calls and active Monty execution, restores `approvals=true`, stops the runtime, returns future script revisions to `pending_review`, logs a `revoked` audit event, and leaves the last-good durable snapshot visible.
- **Agent runtime restrictions stay intact**: Agents still cannot call `start` or `resume` on a Deck runtime (`deck_runtime` accepts only `pause`, `stop`, `run_once`).

## Portable Monty Deck Script Contract

Write bounded cooperative entrypoints — **never** write an infinite `while True:` loop inside one function:

```python
state = {"count": 0}

async def on_start(last_state):
    global state
    if isinstance(last_state, dict):
        state = dict(last_state)

async def on_step():
    global state
    state["count"] += 1
    # Call real session/MCP tools as async functions: await my_mcp_tool(...)
    deck_emit({"count": state["count"]}, durable=(state["count"] % 5 == 0))
    deck_checkpoint(state)
    deck_sleep(5.0)

async def on_action(action, args, ctx):
    global state
    if action == "reset":
        state["count"] = 0
        deck_checkpoint(state)
        deck_emit({"count": 0}, durable=True)
        return {"ok": True}
    raise ValueError(f"unknown action: {action}")
```

### Cooperative Builtins Available in Every Deck
- `deck_emit(data, durable=False, section=None)` — push transient live frame (`durable=False`) or persist durable last-good snapshot (`durable=True`). Secrets are automatically redacted.
- `deck_metric(name, value, meta=None)` — emit a typed metric sample (finite number or bool, optional small redacted metadata dict) evaluated by host-side sustained triggers on the approved script revision.
- `deck_checkpoint(state)` — persist compact JSON state for cold `on_start(last_state)` recovery across upgrades/restarts.
- `deck_sleep(seconds)` — request cooperative pause before the next `on_step()` (clamped `0.1..=3600`s).
- `deck_state()` / `deck_last_snapshot()` — read the pre-turn persisted state / last-good snapshot (values are captured at the start of each turn).
- `tools(query=None)` / `help(name)` — inspect live session & MCP tools exposed to this Deck's `SessionId`.

### Sustained Triggers Firing Existing Routines
- Pass `triggers` on `deck_script_propose(...)` to wire `deck_metric(...)` conditions (`op`, `threshold`, optional `reset_threshold`, `min_samples`, `sustain_secs`, `cooldown_secs`, `reset_mode`) to an **existing** owner-approved routine (`routine`).
- Routine creation still goes through `routine_draft`; triggers never create routines or bypass routine approvals/caps, and HTML sections cannot fire triggers.
- Triggers are included in the canonical revision digest and displayed in the Deck script approval UI for owner review.

### Native Mesh Decks & Per-Source Triggers
- Every Deck has a single authoritative **home engine** (`home_engine_id`, `home_epoch`); non-home viewers can read/render the live Deck and last-good snapshot but cannot start execution, enable trust, or dispatch routines.
- Registered mesh source nodes (`deck_source_register` / `deck_source_ingest`) run locally with their own local tools/MCP servers, spool offline events in bounded SQLite, and send authenticated epoch-fenced samples to the home engine.
- Fresh, authenticated **per-source** metric samples (`source_id`) evaluate the active approved revision's sustained `triggers` on the home engine and dispatch existing routines with persistent deduplicated dispatch keys; replay, heartbeat, non-home, and stale/offline evidence never open, advance, or fire sustained thresholds.
- Explicit cold-start home transfer (`deck_move_home`) stops prior execution, clears `.monty` dumps, revokes local trust (never transferring grants), and requires owner start on the new home.
- **Deferred follow-up**: Multi-source aggregation/quorum trigger scopes (`any`, `all`, `quorum`, `aggregate`) are not yet implemented; triggers currently evaluate on a per-source (and local home) scope only.

### Collaborative Editing Discipline
- Always edit individual sections with `deck_section_patch(deck_id, section_id, expected_rev=...)` — never rewrite unrelated sections.
- On `section_conflict`, re-read the section (`deck_read`), merge changes cleanly, and retry with the latest `rev`.
- To place a live Deck on a whiteboard, add a `deck` card referencing `deck` (the deck ID, plus optional `section` or `snapshot_id`). Embedded whiteboard cards are view/pin surfaces and never expose trust or script-approval controls.
