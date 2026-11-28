---
name: decks
description: Design, build, and onboard collaborative live Decks (native json-render + restricted OpenUI + strict HTML + live Markdown note sections powered by Monty Python scripts with full session/MCP tools).
---

# Decks — Onboarding & Authoring Guide

A **Deck** is a native collaborative live document combining:
- **Visual sections** (`json_render`, `openui` [restricted OpenUI Lang, 64 KiB cap], `note` [safe Markdown with `{{path|default}}` live interpolation], and strict-CSP `html`) stored as stable-ID `Y.Map` entries in a `DocKind::Deck` CRDT document.
- **Executable data & actions** powered by a portable Monty Python script running inside an isolated Deck `SessionId` with the exact same live `ToolRegistry` & configured MCP namespace as a normal agent session.

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
4. **Owner Approval Gate**:
   - Propose the script with `deck_script_propose(...)`. It always enters `pending_review`.
   - Open the Deck with `show_deck(deck_id)` and ask the owner to review and click **Approve & Start**.
   - Never assume visual section edits authorize code; only owner approval activates a script revision.

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
- `deck_checkpoint(state)` — persist compact JSON state for cold `on_start(last_state)` recovery across upgrades/restarts.
- `deck_sleep(seconds)` — request cooperative pause before the next `on_step()` (clamped `0.1..=3600`s).
- `deck_state()` / `deck_last_snapshot()` — read the pre-turn persisted state / last-good snapshot (values are captured at the start of each turn).
- `tools(query=None)` / `help(name)` — inspect live session & MCP tools exposed to this Deck's `SessionId`.

### Collaborative Editing Discipline
- Always edit individual sections with `deck_section_patch(deck_id, section_id, expected_rev=...)` — never rewrite unrelated sections.
- On `section_conflict`, re-read the section (`deck_read`), merge changes cleanly, and retry with the latest `rev`.
- To place a live Deck on a whiteboard, add a `deck` card referencing `deck` (the deck ID, plus optional `section` or `snapshot_id`).
