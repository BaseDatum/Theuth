# 184 as built — E-cells (cells run where viewed)

Branch `feat/184-cells` from the W0 contract f170554c. Design: `184-notes-any-machine.md` §6, §9.3, §11. Paths are under `crates/`.

## Build items

| # | Item | Where |
|---|---|---|
| 1 | Note cells run on the viewer's engine: `note::route`, `note::Route`, `Docs::hub`, `Live::hub` deleted (W-m); `route_scope`'s `Scope::Doc` arm is `Ok(None)`, so run, run_all, stop, input, ask, kernel restart/close, state, runs, run reads and attach all answer here; `host_of` answers `None` for a note (no host chip). Chat routing untouched. | `cells/src/note.rs`, `rpc/src/cells.rs` |
| 2 | The block is read on the document's server (W-i, W-u, S2): `CodeBlockText` routes with `route_doc`; `Serve`/`Park` read the live Doc; `Forward(server)` sends `Cmd::Block{key, seen}` on the owner leg; any leg failure, refusal or `HANDBACK_TIMEOUT_MS` silence (an older server) falls back to the local replica. Fallback author: no members → the replica's; members → `Cmd::Open` to the holder with the replica's state vector, the returned state merged into a throwaway copy of the replica, the author from the holder's `meta.authors`; nobody answers → `last_author = Member{pub_: "?"}`, which `member_guard` turns into `MemberAck("?")` (the confirm), accepted only as `member_ack = "?"`. The member guard itself is unchanged. | `document/src/cells.rs`, `document/src/note_cells.rs` (`blocks_of`), `cells/src/note.rs` (`source(doc, key, seen)`), `cells/src/actor.rs` (`inspect_note`) |
| 2 | Block wait (W-u): with `seen`, a block that does not match parks the reply in a spawned task subscribed to the document; every `DocEvent::Update` re-reads; the reply goes on a match or after `BLOCK_WAIT_MS` with what the Doc has. Nothing sleeps in the actor. A press's reply carries the server's `Markdown{blocks: None, annotate: false}`; `inspect_note` uses it, else reads locally as before. | `document/src/cells.rs` (`serve_block`, `wait_block`) |
| 3 | `cell_latest` (W-h, S1): `put_cell_latest` writes `payload` (the whole row as JSON) plus the old columns; `Store::cell_latest` reads `payload` when it parses, else the old columns with `engine = ""`. `PutCellLatest` is ungated. `latest_row` stamps `engine = mesh::self_id()`. `watch_latest` subscribes to the replica handle and publishes `CellLatestChanged{doc}` for each document a foreign batch's `cell_latest` changes touch. | `document/src/cells.rs`, `cells/src/note.rs` |
| 4 | `after:` gates stay per engine (W-l): no code change; `arm` reads this engine's `cells.sqlite`, `disarmed_reason`/`ran_past` untouched; one kernel per `(Scope, Spec)` per engine as before. | — |
| 5 | Results to a writer agent elsewhere: `last_author` now comes from the server's `agents.json` (item 2); `inject_as` notes `(ran on <me>)` for a `cell …` result queued to a remote owner. | `agent/src/supervisor.rs` |
| 6 | Model text, §11 verbatim: cwd row, `after` row, note defaults `shell`, writing rule, `interpreters(cwd)`, anti-pattern, run-sheet example `#| shell: bash`; `PROMPT` lines 13 and 16. | `cells/cells.md`, `cells/src/prompt.rs` |

## Files

- `cells/src/note.rs`: route deleted; `Docs::code_block(doc, key, seen)`; `source(doc, key, seen)`; `latest_row` stamps the engine; tests: route assertion removed, engine stamp, `?` author confirm.
- `cells/src/actor.rs`: `inspect_note(.., seen)`; press passes `Some(seen)`, `cell_state` `None`; the server's markdown when present. `cells/src/actor/tests.rs`: the `Blocks` fake follows the trait.
- `cells/src/prompt.rs`, `cells/cells.md`: §11 text.
- `cells/Cargo.toml` (unowned): `mesh` dependency for `self_id` (already a transitive dependency).
- `rpc/src/cells.rs`: note scope local, no host chip; test `note_scope_routes_here`.
- `document/src/cells.rs`: `CodeBlockText` handler (now `DelegatedReply`), internal messages `ReadBlock`, `BlockLeg`, `OpenAt`, `BlockHere`; `UNKNOWN_AUTHOR`; `watch_latest`, `latest_docs`; payload read/write; ungated `PutCellLatest`.
- `document/src/store.rs`: unchanged (W0 already adds `payload` and the `ALTER`).
- `document/src/note_cells.rs` (unowned): `blocks_of` extracted from `code_blocks` (shared with the fallback merge); tests: ungated upsert, payload wins over mixed columns, pre-184 row; fake-server `Block` read, unknown author, old server + holder author, server-side wait.
- `document/tests/replicate.rs`: partitionable switch, `CellLatestChanged` on the receiving engine only, two engines' runs never mix.
- `agent/src/supervisor.rs` (§15.2 names the line): `(ran on <me>)`.

## Deviations

1. No `block_waits` map or `Tick` in `DocumentActor` (that file is E-doc's): the wait is a task subscribed to the document's events, which is the same parked reply without a field.
2. `relayed`: the handler calls `route_doc(doc, false)`; how a `Relayed(CodeBlockText)` reaches it with `true` is E-doc's generic `Relayed<M>` impl.
3. `Park` reads here like `Serve` (the claim's hand-back arrives as an update the wait re-checks), until E-doc's re-send park exists.
4. "Homes off" is not tested separately: the read always tries the server, and an old or absent server fails the leg into the fallback.
5. `markdown` is filled only when `seen` is set (a press); `cell_state` and the other reads use the local markdown as before.
6. Every server error (including "no code block") falls back to the replica; the press's digest check (`Changed`) still guards what runs.
7. The fallback's `Cmd::Open` rides the `(Client::Conn(None), doc)` leg; W0's `Legs` key has no engine, so after a timed-out `Block` that leg may still point at the server. §9.2's `(EngineId, Sender)` legs (E-doc) remove this.
8. A lagged `watch_latest` receiver skips those batches (`ponytail:` comment).

## Tests

See `184-notes.md` "E-cells" for the gate lines.
