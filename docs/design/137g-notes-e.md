# 137G-E notes — bounded endless requests, fold, pause

## Contract (phase A)
- `Backlog { lines, folded, eta_secs, paused }` in `endless/mod.rs` beside `Stalled`; `SessionMemory.backlog` / `EndlessProgress.backlog` are `Option<Backlog>` (`serde(default)`, `ts(optional = nullable)`), `None` when no level-0 line in `[base, T)` is unbuilt and the session is not paused. An epochs session reports `None`.
- `ViewLine.mechanical: bool` (`serde(default)`, `ts(optional)`): the line's node has `mechanical` set.
- `endless_fold(session) -> SessionMemory`, `endless_pause(session, paused) -> SessionMemory`: method table (`rpc/src/api.rs`, `forward::route` like `endless_retry`), `agent::EndlessFold` / `agent::EndlessPause` actor messages, docs §11.2, `Backlog` in the bindings export, TUI `/endless fold|pause|resume`.
- Migration `0051_endless_backlog.sql` (all 137G columns; see the additions below), `endless_nodes.mechanical INTEGER NOT NULL DEFAULT 0`.

## Contract additions (Sky, 16:31; second contract commit)
- `SessionMemory.model_override: Option<String>`, `model_source: String` (`chat|agent|settings|title|session`, `""` for epochs), `spend_today_usd: f64`; `Backlog.paused_by: Option<String>` (`user|spend`). Serde defaults; TS `model_source?: string`, `spend_today_usd?: number` (`ts(as = Option<…>, optional)` — ts-rs refuses `optional` on a non-`Option`).
- RPC `set_session_compactor_model(session, model)`; actor messages `SetSessionCompactorModel`, `SetAgentCompactorModel` (roster → engine).
- Chain `pump::chosen`: chat (`sessions.endless_model`) → agent (`sessions.endless_agent_model`) → `agent.endless.model` → `agent.title_model` → session model. An id that stops resolving at run time falls back to the session model with a warning (as before for `agent.endless.model`); `set_session_compactor_model` refuses one that does not resolve now.
- Agent default: `memory_model:` front-matter key (`crates/roster`: `AgentFile.memory_model`, parse/render, `AgentProposeArgs`; `crates/app` `AgentDraft.memory_model` + a card meta line). `agents_list`/`agent_put` carry it through `AgentFile`. The roster writes it to the desk session at every desk open and when the file changes (on-disk edit or `PutAgent`). Scope note: the roster lives in `crates/roster` and the draft tool in `crates/app`, outside the brief's listed crates; touched only for this field.
- Migration 0051 holds every new column: `sessions.endless_paused TEXT` (`user|spend`), `endless_spend_override TEXT` (UTC day of the user's resume past the cap), `endless_model TEXT`, `endless_agent_model TEXT`; `endless_nodes.mechanical`.
- Spend today: `Store::endless_spend_since` — `model_calls` rows with `kind = 'endless'`, `owner_id = session`, `created_at ≥` 00:00 UTC; returned cost else estimate, USD only.

## Decisions
- **TS name `EndlessBacklog`** (deviation, names only on the TS side): `dreaming::Backlog` (136 §2.7) already exports `bindings/Backlog.ts`, and a second `Backlog.ts` overwrote it. The Rust type is `agent::Backlog` as contracted; its binding is `ts(rename = "EndlessBacklog")` → `bindings/EndlessBacklog.ts`, and `SessionMemory.backlog` / `EndlessProgress.backlog` are typed `EndlessBacklog | null`. The JSON shape is unchanged.
- **"guard of mechanical"**: `endless_nodes` has no guard column and `endless_calls` rows are model attempts (they feed `stats.calls` and the guard-trip counts). The flag is the node's own `mechanical` column instead; a mechanical node has `tries = 0`, `model = NULL`, `free = false`. No `endless_calls` row is written, so `stats.calls` keeps counting model calls only.
- **Fold of a unit that fits `node_bytes`**: its node is the ordinary free node (verbatim, `free = true`, not flagged) — it is exact, not an excerpt.
- **Excerpt**: `tree::mechanical_unit` — the tagged unit, whitespace runs collapsed to one space, then head ⅔ + ` … ` + tail ⅓ of the room, at char boundaries, ≤ `node_bytes` bytes.
- **Fold and calls in flight**: if a level-0 call runs, the State's token is cancelled (no failure; a merge in flight restarts with it). A model reply that raced the cancel and landed first wins over the excerpt (better text); the fold only fills nodes that are still unbuilt.
- **Pause on an epochs session**: unchanged, like fold (nothing is persisted).
- **Pause and settle**: `settle` returns `false` at once while paused (no call can land).
