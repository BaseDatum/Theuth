# 184 — as built: E-doc (engine: documents)

Slice E-doc of `184-notes-any-machine.md` (§4, §5, §7.4, §9.1), on `feat/184-notes-any-machine` from the W0 contract commit f170554c. Paths are under `crates/`. Decisions taken where the design left room are in `184-notes.md` "E-doc decisions"; contract changes after W0 in "Contract changes after W0 (E-doc)".

## File by file

| File | What it does now |
|---|---|
| `document/src/mirror.rs` | `merge_meta(local, incoming, opens)`: `home`/`home_ms` register (greater `(home_ms, home)`, `None` lowest, W-a), fence `(serve_epoch, served_by)` (greater wins), and the seal rule of §7.4 (local if it opens, else incoming if it opens, else local unless empty). Test `meta_merge_home_fence_and_the_seal_that_opens`. |
| `document/src/hub.rs` | `Hub.homes_on`, `self_id`, `is_me`. Routing: `route_doc` (rules 1-6, starts claims), `serves`, `server_of` (never claims; a `Pending` claim counts as here), `gate_doc`, `route_home` (homes off = today's gate, never forwarded; rule 6 for relayed creates), `resolve_home(Homing, kind)`, `home_of`/`home_peek` (open meta, memo, `meta.json`, then `pending_homes`). Claims: `Claim::{Pending{from, rest, epoch, parked}, Held{epoch}}`; `claim` (sources = first, `served_by`, holder; reachable, not self), `hold` (fence + re-seal, written by `Tick`), `send_handback` (leg `(Conn(None), doc)`, `HANDBACK_TIMEOUT_MS`), `HandbackDone` (apply as `File` + export, merge meta, take missing `agents.json` entries, next source or `finish_claim`: fence above every epoch seen, snapshot, re-send parked calls in arrival order, reroute). Parking (W-t): `park` (re-sends as `Relayed` when it came so; `busy` after `JOIN_PARK_MS`), `gate_here`, `drop_claim` on unload. `reroute` (§4.5: unload what moved with `server_moved`/`hub_moved`, drop legs pointing elsewhere, §4.4 hand back from a reachable `dual`, holder asks `RemovedPeers`). §4.4 `fence_beaten`/`set_dual` (`DocEvent::Server`). Messages: `Relayed<M>` (actor flag `relaying` around the inner handler), `Handback` (home or holder only, else `not_home`; unloads `server_moved`), `Handover` (merge, claim with the old server first, answer the epoch once `Held`), `MoveHome` (checks of §4.8 in order; on the server an explicit `Handover`, else write + reroute), `DocServer`, internal `Leg<T,E>` (a reply sender for the actor's own leg calls), `HomesOn` (the gate; a flip reroutes), `Reroute`, `Rehome` (§4.9, idempotent), `MeshEvent` (lease → `sync_hub`; peer → reroute; every event polls `DocsGate`). `sync_hub` keeps the watcher and runs `reroute` instead of its unload loop and `legs.clear()`. |
| `document/src/actor.rs` | `DocumentActor` implements `kameo::Actor` by hand to keep a weak ref to itself (`actor_ref` no longer depends on the registered name). Fields `claims`, `homes`, `pending_homes`, `relaying`, `last_gate_poll`; `Open.dual`, `Open.meta_dirty`. `OpenDoc`, `ApplyUpdate`, `Subscribe`, `AwarenessUpdateMsg` (dropped when parked), `Edit` use `route_doc`; `ImportMarkdown`, `UndoLastAgentEdit`, `Rename`, `DeleteNote`, `Snapshot`, `SetDerived`, `Bind`, `Restore`, `Delete` use `gate_doc` (the last five plain replies became `DelegatedReply` so they can park). `DocOpen` carries `server`, `dual`, `sv`. Creates: `CreateNote`/`Create` → `create_routed` (`resolve_home` + `route_home`: here, or `Cmd::Create` with `pending_homes`; W-f lookup for a relayed keyed create), `CreateBoard` always `Here`, `Ensure` → `route_home(None)`. `create_doc(.., home, ..)` stamps `home`, `home_ms`, fence 1 and `Held{1}` when homed. §5.3 first snapshot once a `CreatedNote` import's reply and update both landed. `schedule_export`, quiet snapshot, `FileChanged` (after the lookup) and the throwaway-cache rule follow `serves`. `MirrorChanged`: memo refresh and reroute on a moved home (closed docs), W-s (served doc whose files vanished under a tombstone → `delete_doc`), open-meta merge with `opens`, `fence_beaten` on a moved fence, reroute on a moved home. `Tick` writes `meta_dirty` metas, polls the gate every 30 s, expires `pending_homes`. `unload` drops the claim and memoises the home; `delete_doc` drops both. |
| `document/src/relay.rs` | `Legs.legs` keyed `(Client, DocId) → (EngineId, Sender)`; `Legs::reachable`; `Legs::clear` gone; `forward` replaces a leg that points at another engine; `SetTransport` reroutes. `serve_stream(actor, wire, peer)`: every call is `Relayed(..)`; tag 10 `Create` (runs `CreateNote`/`Create`/`Ensure`+`MetaOf`/`NotebookCreate` as `Relayed`, reply JSON), 11 `Handback` (`from` = the stream's peer), 12 `Block` (`Relayed(CodeBlockText)`, E-cells' handler), 13 `Handover`, 14 `MayRead` (the stream's principal); 10-13 still `not_owner` on member streams. Tests: `Loop` gains `nodes()` and `set_cut()`; the fake helper answers `import`. |
| `document/src/share.rs` | `seal_opens`; `reseal_links` pub(crate); `reseal` — the holder over every document as before, any other engine over what it serves or homes, `true` with a vault. `Join`/`SetName` use `route_doc` (`Join` parks ≤ `JOIN_PARK_MS`, then `busy`); `LinkCreate`, `LinkRevoke`, `MemberSet`, `MemberRevoke`, `MembersOf` use `gate_doc` (now `DelegatedReply`). |
| `document/src/notebook.rs` | `NotebookCreate` → `route_home(None)`, `Cmd::Create{Notebook}` off the holder. `MayRead`: a local refusal of a member, homes on, document served elsewhere → `Cmd::MayRead` to the server on the member's leg (not when relayed). |
| `document/src/store.rs` (E-cells, touched) | `mirror_apply`'s meta merge passes `opens` (`seal_opens` with the store's vault). |
| `tasks/src/thread.rs` | `Thread::create(.., home)` → `Create{TaskThread, home: Engine(task.home) / Hub}`; `Thread::set_home(doc, home)` → `MoveHome{checks: false}`. |
| `tasks/src/ledger.rs` | `create` passes `t.home`; `move_home` calls `thread.set_home` after saving; `MemThread` implements both. |
| `meeting/src/note.rs`, `dreaming/src/journal.rs`, `autoresearch/src/finalize.rs`, `document/src/{tools.rs, nb_tools.rs, lib.rs, ingest/*}` | Unchanged: W0 already passes `Homing::Here`/`Hub`; the actor routes. |

## Tests (all in `document`)

| Test | Covers |
|---|---|
| `mirror::tests::meta_merge_home_fence_and_the_seal_that_opens` | §9.1 registers, ties, an old engine's absent fields, §7.4 seal rule |
| `relay::tests::route_doc_rules_one_to_six` | rules 2-6 through `DocServer` and relayed calls; homes off; no holder; a home cut off still serves |
| `relay::tests::a_home_takes_the_fallbacks_edits_before_serving_and_move_hands_over` | fallback serves while the home is cut off; the home's parked open answers after the hand-back with the fallback's edits (no mirror); `Closed{server_moved}`; fence above every epoch; front door after; `not_home`; Move to with an explicit `Handover` |
| `relay::tests::a_parked_join_answers_busy_and_a_silent_source_times_out` | Park, `busy` after 2 s, a silent source costs 5 s, then the parked open is served |
| `relay::tests::creates_off_the_hub_run_where_the_document_will_be_served` | `CreateNote{Here}`, `CreateBoard`, meeting note, thread homed here: local with `home`; thread homed on the holder: forwarded, routed before the mirror, idempotent; `Ensure`, `NotebookCreate`, `CreateNote{Hub}`: on the holder; homes off: today's `NotHub` |
| `relay::tests::move_home_refuses_each_kind_with_its_copy` | §4.8 checks and copy |
| `relay::tests::unpair_rehomes_to_the_holder_once` | §4.9 and idempotence |
| `relay::tests::a_lone_engine_creates_and_serves_as_before` | §16 |
| `hub::tests::off_the_hub_reseal_covers_what_this_engine_homes` | §7.4 reseal scope |

## Deviations

- `Claim::Pending` has no `since` or `done: watch::Sender`: parked calls are an ordered list inside the claim, re-sent by one task in arrival order (§4.3 step 4's "re-sends in arrival order"), and a dropped claim re-sends them too.
- The W-f lookup (a keyed create answers the existing document) runs only for a relayed create, so a lone engine keeps today's "already bound" refusal.
- The §5.3 first snapshot covers `CreateNote`, `CreateBoard` and `Create`, not `Ensure` or `NotebookCreate` (hub kinds, unchanged).
- `MoveHome` refusals name engine ids (`<id> is offline`, `already lives on <id>`, `moved to <id>; …`): the document crate has no machine names.
- A source answering `Handback` does not fire a pending export before it unloads (the claimant's apply schedules the export).

## Gates

Set `-p document -p tasks -p meeting -p dreaming -p autoresearch -p theuth` (built `--no-run` under build-lock, run bare).

- `mbx check … --tests`: `Finished`, 0 errors, 0 warnings.
- `mbx clippy … --tests --no-deps -- -D warnings`: `Finished`, 0.
- `mbx test … --no-fail-fast`, twice: document lib 94, actor 12, cite 4, index_search 3, ingest_audio 1, ingest_job 5, insights 2, replicate 2; tasks 119; dreaming 64; meeting lib 165 (+ note 1, oracle 1); autoresearch 304; theuth 12 — all `ok`, except:
  - `meeting/tests/calendar.rs` `fastmail_floating_time_is_local_and_unknown_hosts_are_not_links` fails both passes (`10:00` vs `09:00`, a local-time/DST expectation); the meeting crate is untouched by this slice.
  - pass 2 only: `tasks` `episode_closing_answers_while_probe_is_out` ("the sweep did not apply", a 5 s poll under load); it passed in pass 1 and 3 of 3 reruns.
- Fail-before: with W0's `merge_meta` body and W0's reseal gate put back, `meta_merge_home_fence_and_the_seal_that_opens` and `off_the_hub_reseal_covers_what_this_engine_homes` FAIL; restored, both pass. The relay tests drive `HomesOn`/`Rehome`, which W0 lacks, so they cannot run on W0 (there `CreateNote` off the hub answers `NotHub`, `MoveHome`/`Handback` answer `not built yet`).
