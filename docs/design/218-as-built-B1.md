# 218 — as-built B1 (document)

Builder B1, `crates/document/**` (+ the `first_run` skill entry, one `harness.md` paragraph, two client re-exports).

## Built (files)

- `crates/document/src/meta.rs` — `DocKind::Whiteboard` (`"whiteboard"`, free-standing, homed, not movable), `Binding::Whiteboard` (keyless, `documents/{id}/whiteboard.md`).
- `src/wb/mod.rs` — §3.5 seam: `WbOutline`/`WbOutlineReply`, `WbInfo`/`WbInfoReply`, `WbTurn`/`TurnPhase`, `hooks::{Hooks, install, bind}` (once per (session, whiteboard) per process), `quote`/`unquote`, `DOC_REFUSAL`; RPC types `WhiteboardCreated`, `WbBlob`, `WbBlobData`, `WbTurnInfo`, `WbUndoResult`, `WbSkipped`; messages `CreateWhiteboard`, `BlobPut`, `BlobGet`, `TurnOf`, `UndoTurn`.
- `src/wb/model.rs` — §2 Rust twin: every type, tolerant `read` (skips unparseable entries, ignores unknown keys, reads Int/Float/BigInt numbers; JS float64 timestamps read as integers), `read_meta`, field-wise writers (`Maps`, `card_fields`/`link_fields`/`frame_fields`/`mark_fields`), `ulid()` (ms + 80 bits of a v4 uuid), `is_ulid`, `ENGINE_CLIENT`.
- `src/wb/pack.rs` — §2.6 codec (golden vector, long-step interpolation).
- `src/wb/aliases.rs` — §2.8 `missing`/`ensure` (per prefix, ULID order, never reused).
- `src/wb/outline.rs` — §3.2 `<whiteboard>` renderer (rows rule, frames in reading order, `vK/K`, extras incl. FNV-1a `#xxxx`, link/mark lines, 200-card overflow), `render_frame` (`<whiteboard_frame …>` — deliberately *not* a `<whiteboard id=` prefix so B2's baseline never takes a partial view), `source_summary`, `export_text`.
- `src/wb/place.rs` — §3.1 placement (pure): default sizes, beside `near` (right then below, alternating 24 px steps), frame rows (append to the last row, wrap past 1600 px, grow the frame), loose fallback; avoids live/dimmed/placeholder cards and free-stroke boxes with 8 px clearance; `frame_around` (32 px padding + 40 px title band).
- `src/wb/engine.rs` — the actor side: every engine write = scratch copy → one `encode_diff_v1` → `DocumentActor::apply` (no write when nothing changed); `ensure_aliases` before and after every write; per (whiteboard, session) turn log (10 kept) with inverses (created / field / deleted / version pushed / version replaced) and owner-change detection; `WbTurn` begin/end + placeholder sweep; current-whiteboard memory; create path (`wb_create_here`, `CreateReq::Whiteboard` forwarding); blobs (`<store>/<id>/blobs/<key with / → _>.<ext>`, key regex, mime, 3 MiB); `whiteboard_turn`, `whiteboard_undo_turn`; the R8 bake (`Bake` after an apply past 2 MiB, `BakeNow` for tests); `WbCall`/`Op`/`WbOut` behind the tools.
- `src/wb/tools.rs` — all 13 `wb_*` tools (incl. stretch `wb_focus` → `meta.focus`, `wb_export(format="note")` → a note via `CreateNote`), `whiteboard_id` accepts bare / `whiteboard:` / `theuth://whiteboards/…#…`.
- `src/rpc_whiteboards.rs` — `DocumentWhiteboardsApi` (§4.1, five methods, positional params in contract order), composed into `document::rpc_module`.
- `src/actor.rs` — kind branches: `load` (no helper mirror, no fragment observer), `apply` (no `helper.update`; bake check), `export` (outline file, never the helper), `unsynced_file`, `start_import`/`start_edit` refusal, `Outline`/`Markdown` refusal, `Headings` (empty, no helper), `Rename` (meta only + re-export), `Restore` (refused); `Open.bytes` now `pub(crate)`; `wb: WbState` field.
- `src/relay.rs` (`CreateReq::Whiteboard` + relayed arm), `src/hub.rs` (`one_shot` arm; explicit `move_checks` arm), `src/index/mod.rs` (`INDEXABLE` gains the kind; outline item lines as blocks), `src/index/chunk.rs` (`whiteboard` label), `src/lib.rs` (modules, RPC composition, tool registration).
- `src/tools.rs` — `BOARDS_PROMPT` final sentence, `WHITEBOARDS_PROMPT` (verbatim §11), `WHITEBOARD_SKILL`, register text `[PROMPT, BOARDS_PROMPT, WHITEBOARDS_PROMPT]`.
- `crates/document/whiteboard.md` — the A3 skill.
- `crates/document/tests/whiteboard.rs`, `tests/fixtures/whiteboard/outline.txt` (the §3.2 example byte for byte; identical to B2's copy, `diff` clean).
- `crates/theuth/src/main.rs` (`first_run` entry + doc comment), `crates/theuth/harness.md` (one paragraph), `crates/rpc-compose/src/api/clients.rs` (both lines).

## Deviations

1. **Reply types.** The five §3.5/§4.1 actor messages are `DelegatedReply<Result<T, String>>`, not plain `Result<T, String>`: they go through `gate_here`, so a homed whiteboard whose claim is pending parks instead of failing. `ask(..)` yields the same `Result<T, SendError<_, String>>` to callers.
2. **`CreateWhiteboard` carries `home: Homing`** (the RPC passes `Here`) so the relayed create keeps the caller's home.
3. **`wb_update` on in-place kinds** (sticky/note/diagram) answers *every* sent mark on the card, not only `ver < new version` (the version number stays 1, so the contract rule would never answer them).
4. **Placeholder sweep** deletes every empty placeholder whose `by` is the session (memory-free, so it also works after a restart), at both `Begin` and `End`.
5. **`wb_read(frame=…)`** opens with `<whiteboard_frame whiteboard="…" frame="fN">` (see outline.rs above).
6. **Frame row width**: a frame row grows to 1600 px before wrapping (contract says "next free slot in the last row, else a new row" without a width rule).
7. **`wb_card`** also returns `href` (`theuth://whiteboards/<doc>#<card>`); text appends sticky text / diagram code / excerpt lines.
8. **Restore of a snapshot** is refused for whiteboards (the helper-based restore cannot run; no Yjs-level restore tonight).

## Checks

All under the build lock with the §0.4 caps.

- `cargo check -p document --tests` — exit 0 (before commit 8bf393525).
- `cargo test -p document --lib -- wb:: tools::tests` — all pass: `wb::pack::tests::{golden_vector, round_trip, long_step_is_interpolated}`, `wb::model::tests::{write_read_round_trip, tolerates_unknown_fields, reads_bigint_numbers}`, `wb::outline::tests::{golden_outline, rows_reading_order, quote_unquote}`, `wb::place::tests::{never_overlaps_cards_or_ink, fills_frame_row_then_wraps}`, `wb::aliases::tests::stable_and_never_reused`, `tools::tests::whiteboard_skill_is_wellformed` (and the board skill test with the new `BOARDS_PROMPT` sentence). `wb::tools::tests::whiteboard_ids_in_every_spelling` failed once (my test used the contract's uuid-shaped id; a `DocId` is 26 base32 chars) — fixed in the fix round.
- `cargo test -p document --test whiteboard` — 11/11 pass incl. `reads_the_yjs_fixture` (run with `--include-ignored`, B3's `example-v1.update.b64`): `create_place_fill_link_push`, `create_never_calls_the_helper`, `export_writes_the_outline`, `doc_tools_refuse_a_whiteboard`, `update_answers_sent_marks` (failed once on ULID order of two marks minted in one ms; test fixed), `aliases_same_for_two_sessions`, `undo_turn_skips_what_the_owner_changed`, `placeholder_swept_at_turn_begin_and_end`, `blob_put_get_and_key_validation`, `bake_deletes_old_answered_strokes`.
- Not run by me (lock time): full `cargo test -p document` (only the filtered sets above), `cargo check -p theuth`, `just wire-schema-accept` + `wire_schema_snapshot` — the orchestrator's gate re-accepts the wire fixture anyway (contract §9 B1 Checks allows it).

Note for the orchestrator: the contract (§1, §4) calls a whiteboard id a UUIDv7; in this tree `DocId` is a 26-char base32 id (`meta.rs` `DocId::parse`). The §3.2 example id is only text in the fixture; nothing parses it.

## Not built / unverified

- No hub forwarding for `wb_*`, `WbOutline`/`WbInfo`/`WbTurn` and the blob/turn RPCs (contract §12.16) — they fail `NotHub` off the serving engine.
- The 2 MiB bake trigger itself is not exercised by a test (selection logic is, through `BakeNow`).
- `wb_crop(region=…)` refuses as specified.
- `reads_the_yjs_fixture` is a normal test since the fix round.

## Fix round (review 218-review-build.md)

- **B1 (blocker)** — `wb/engine.rs` `check_source`: a `web` source must be an `http`/`https` URL with a host (trimmed); `asset:`, `theuth-page:`, `tauri:`, `file:`, `javascript:`, `data:`, `ipc:`, scheme-less and host-less URLs are refused with `wb_place|wb_update: a web card takes an http(s) URL with a host; "…" is refused — …`. Test `wb::engine::tests::web_cards_take_http_urls_only`.
- **M7** — `reads_the_yjs_fixture` passed against B3's fixture as is (no reader change); `#[ignore]` removed.
- Also fixed my `whiteboard_ids_in_every_spelling` test (real `DocId`).
