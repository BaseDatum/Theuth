# 218 — as built, B2 (agent)

Builder B2, night build 2026-11-25. Contract: `218-contract.md` rev 1, §9 row B2.

## What is built (files)

| Area | Files |
|---|---|
| RPC §4.2 | `crates/agent/src/rpc_whiteboards.rs` — `AgentWhiteboardsApi` (`whiteboard_bind`, `whiteboard_unbind`, `whiteboard_sessions`, `session_whiteboards`, `whiteboard_send_context`), types `WhiteboardSession`, `WbSendContext`, `WbSentImage`; composed in `crates/agent/src/lib.rs` `rpc_module`. All owner-only (`owner(ext)`). |
| Binding table §5 | `crates/agent/migrations/0077_session_whiteboards.sql` (verbatim), `crates/agent/src/store/whiteboards.rs` — `bind_whiteboard` (upsert, `true` = new, refreshes `bound_ms`/`by`), `unbind_whiteboard`, `forget_whiteboard`, `whiteboards_of` (root-resolved via `root_of`), `sessions_of_whiteboard`, `forget_whiteboard_session`. Node-local: not in `capture::ATTACHED`, `SESSIONS_WIRE_SCHEMA` stays 70. |
| Parser, diff, renderer §3.2 | `crates/agent/src/whiteboards.rs` — `parse_outline` → `Facts` (frames, cards with row positions, links, marks; extras opaque), `Baseline::of`, `diff`, `apply_line`, `render_changes` (40-line cap + overflow line), `context_lines`, `brief_line`, `turn_messages` (pure planner: ≤ 3 full, the rest one brief-line message once per context; a whiteboard with a pending send context sorts first). |
| Injection §5 | `whiteboards::bound` (binds `[attached whiteboard …]` markers of the turn's input, `by='marker'`, and gates on "any binding") + `whiteboards::inject` (per whiteboard: `WbInfo` → unbind when not a whiteboard / trashed; `WbTurn{Begin}`; `WbOutline`; takes the pending send context) called from `loop_.rs` right after the notebook block (`chat && round == 0`, skipped when the history tail already starts `<whiteboard`). `whiteboards::end_turn` sends `WbTurn{End}` detached where the turn closes (before the advisor's terminal step / `TurnFinished`). |
| Send context §4.2 | in-memory map keyed (root session \| `None`, whiteboard), 120 s TTL, consumed once; `None` is anyone's. |
| Document hook §3.5 | `whiteboards::install_hooks` (bind with `by='tool'`, detached), called from `agent::rpc_document::install_insight_hooks` (already invoked at bootstrap). Store reached through a new `Agents` message `whiteboards::WbStore`. |
| Attach §5 | `attach.rs` `Kind::Whiteboard`, `ALL: [Kind; 8]`, `as_str`, the PROMPT table row verbatim; fixture case appended to `tests/fixtures/attach_markers.json`. |
| `show_whiteboard` §3.4 | `crates/artifact/src/tileview.rs` — `ShowWhiteboardArgs`, `ShowWhiteboardTool` (ReadOnly, `DIRECT`, `RETURNS = "dict"`; resolve → `MetaOf` as the agent → kind check → `hooks::bind` → publish → receipt `opened whiteboard "<title>"` + `theuth://whiteboards/<id>`), `WhiteboardViewOpened` event; `show_note` refuses a whiteboard (`show_note: this is a whiteboard; use show_whiteboard`). `crates/artifact/src/lib.rs`: re-exports, `register_wire_event`, `RegisterTool`. |
| Enumerations §4.3 | `crates/tools/src/builtins.rs` (both `show_note` lists), `crates/sessionsync/src/events.rs` (`WIRE_RELAYED` + its test), `crates/rpc-compose/src/client_mirror.rs` (`republish!`), `crates/rpc-compose/tests/wire_golden.rs`, `tests/wire_parity_inc5.rs`, fixture `tests/fixtures/wire/artifact.whiteboard-view-opened.json`. |
| Prose | `crates/tasks/src/tools.rs` attachments doc comment lists every kind incl. `whiteboard`. |
| Fixture | `crates/agent/tests/fixtures/whiteboard_outline.txt` — the §3.2 example, cut from the contract by line range (LF, one trailing newline). |

No exhaustive `Kind` match exists in `tasks`, `tui`, `roster` or `app` (grep + check), so nothing changed there. Slack's `is_deliverable` is a `matches!` mirrored by `ToolRunCard.tsx` (B4's file), not exhaustive: left alone.

## Deviations (with reasons)

1. **`whiteboard_send_context`'s third parameter is named `send`, not `context`.** jsonrpsee's server macro binds a local `context` (the module context) and a parameter of that name shadows it (`no method as_ref on WbSendContext`). Params are positional, so B4's `wbRpc.sendContext` is unaffected; only the OpenRPC param name differs.
2. **`~cN edited` carries the extras:** rendered `~cN edited · 4 lines · #a1b2`. The bare form cannot heal the baseline (applying it would not know the new extras, so the same line would repeat every turn), which breaks `changes_round_trip`. B1 never parses changes, so this touches no other builder.
3. **`~fN preset none`** when a frame loses its preset — the grammar has no form for it; same healing reason.
4. **Order of change lines:** `-`, `~`, `+`; inside each, `~title` first, then frames, cards, links, marks, each by alias number (frames before cards so `+frame f2` precedes `+c9 … · in f2 row 1`).
5. **`selected:` aliases are comma-separated** like `in view:` (the contract shows one alias only).
6. **Bind on marker happens at injection (round 0 of the turn), not in `send_input`.** Same engine, same turn; it avoids touching the input-acceptance paths. The marker is parsed from the turn's input text.
7. **Sessions on another engine: local only.** The new RPCs do not forward (`set_model`'s `authorize_opt_fwd` pattern is not a copy-paste here: the store is reached through the agent actor, and the table is node-local by design).
8. **`whiteboard_bind` uses `by = 'composer'`** (the client is the composer); `'rpc'` is unused.
9. **The artifact tests are unit-level** (`show_whiteboard_publishes_the_event` publishes through a real `Runtime` wire feed and checks topic, shape and receipt; the two refusals test `note_kind` / `not_a_whiteboard`). There is no in-crate harness for a running `DocumentActor`, so the full `call()` path is unverified.
10. A retried round 0 whose first try produced no whiteboard message sends `WbTurn{Begin}` again (an empty turn in B1's undo list).

## Checks (exit codes)

All under the build lock with the §0.4 caps.

| Command | Exit | Result |
|---|---|---|
| `cargo check -p agent -p artifact -p tasks -p tui -p roster -p app -p sessionsync -p tools` (seam, first try) | 101 | `context` param shadowing (deviation 1); fixed |
| `cargo check -p agent -p artifact --tests` | 0 | |
| `cargo test -p agent --lib whiteboards -- --test-threads=4` | 0 | 16 passed: `whiteboards::tests::{parses_the_contract_outline, baseline_resets_on_a_block, baseline_applies_changes, changes_without_a_block_are_ignored, brief_line_is_not_a_baseline, diff_golden, changes_round_trip, cap_at_40_lines_heals_next_turn, full_block_again_after_compaction, a_mark_reaches_each_bound_session_once, context_and_image_lines, more_than_three_whiteboards, quote_cuts_and_escapes, send_context_is_taken_once_and_falls_back_to_any_session}`, `store::whiteboards::tests::{bind_unbind_root_resolved, node_local_no_wire_bump}` |
| `cargo test -p artifact --lib tileview -- --test-threads=4` | 0 | 14 passed incl. `show_whiteboard_publishes_the_event`, `show_note_refuses_a_whiteboard`, `show_whiteboard_refuses_a_note` |

Queued behind the shared build lock when this note was committed (results, if they land in time, are appended below): `cargo test -p agent --lib attach`, `cargo test -p agent --lib store`, `cargo check -p tasks -p tui -p roster -p app -p sessionsync -p tools`, `cargo test -p sessionsync --lib relay`, `cargo test -p rpc-compose --test wire_golden --test wire_parity_inc5`, `cargo clippy -p agent -p artifact --all-targets --no-deps -- -D warnings`.

The fixture `crates/agent/tests/fixtures/whiteboard_outline.txt` was cut from the contract (lines 349–361). `diff crates/document/tests/fixtures/whiteboard/outline.txt crates/agent/tests/fixtures/whiteboard_outline.txt` → exit 0 (identical to B1's copy).

## For other builders / the orchestrator

- B4: `wbRpc.sendContext` is positional — the renamed param (`send`) changes nothing on the wire. `whiteboard_bind` refuses a trashed or non-whiteboard doc and an unknown session.
- B1: B2 calls `WbInfo` → `WbTurn{Begin}` → `WbOutline` per bound whiteboard per turn, and `WbTurn{End}` detached at turn end. An error whose text contains `not a whiteboard` / `no such document` / `not found` unbinds the session.
- Orchestrator: B1's `rpc-compose/src/api/clients.rs` line `pub use agent::rpc_whiteboards::AgentWhiteboardsApiClient;` resolves against this commit. Run `just wire-schema-accept` after B1 and B2 (new methods, `AttachKind` gains `whiteboard`, the event).

## Unverified

- The live injection path end to end (needs a running engine with B1's actor and a bound session); covered by the pure planner tests only.
- `show_whiteboard`'s `call()` against a real document actor.

## Fix round (review M5, M6)

- **M5 — marker binding vetted.** `loop_.rs` records `Turn::input_owner`: true only when the session is top level (`depth == 0`), the turn has no contact cap, and no input of the turn carries a `peer`, an A2A `scope` or a `channel` (Slack/Telegram/…). `whiteboards::bound(store, session, input, owner)` binds a marker only when owner, `MetaOf` as `Principal::Owner` succeeds (the doc exists and the owner may read it), its kind is `DocKind::Whiteboard`, and `WbInfo` says not trashed (`marker_refusal`). Every refusal is logged at warn and binds nothing. Test: `whiteboards::tests::marker_binds_only_an_owners_readable_whiteboard`.
- **M6 — unreadable outline is said, not swallowed.** When `WbInfo`/`WbOutline` fail for a bound whiteboard (other than "gone", which unbinds), or an owner's marker cannot be read on this engine, injection logs at warn and adds one line: `<whiteboard_unavailable id="…" reason="another engine serves it"/> This chat is bound to that whiteboard, but its outline cannot be read here, so the wb_* functions will fail too. Tell the user, …`. Once per context (scanned like the brief line); not a baseline, so the full block follows as soon as the outline is readable. `whiteboard_bind` already refuses off the serving engine (its `WbInfo` check fails `NotHub`, `-32011`). Test: `whiteboards::tests::unreadable_outline_is_a_notice_once_per_context`.
- Migration stays `0077` (the orchestrator renumbers at merge if 219 lands first).
- Limit: the owner test is by input provenance (`peer`/`scope`/`channel`/depth/cap); a channel input from the owner (e.g. the owner's own Slack DM) does not bind by marker — the composer's `whiteboard_bind` still does.
- Checks (fix round, under the lock): `cargo test -p agent --lib whiteboards` 0 (18 passed) · `--lib attach` 0 (13) · `--lib store` 0 (171) · `cargo check -p tasks -p tui -p roster -p app -p sessionsync -p tools` 0 · `cargo test -p rpc-compose --test wire_golden --test wire_parity_inc5` 0 (2 + 1) · `cargo clippy -p agent -p artifact --all-targets --no-deps -- -D warnings` 101 on `type_complexity` (the send-context map) and two undocumented fields → fixed (`ContextKey` alias, field docs, a redundant test import removed); clippy re-run result below if it lands.
- Clippy re-run (`cargo clippy -p agent -p artifact --all-targets --no-deps -- -D warnings`): exit 0.
