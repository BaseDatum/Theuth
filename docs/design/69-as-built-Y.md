# 69 Y — as built (2026-09-25)

Branch `feature/69-docs-y` (cut from `feature/69-docs` after C2's commit 4). Contract: `docs/design/69-contract-docs.md`; design `69-devices.md` §5 decisions 11–12. Two commits.

## Commit 1 — `feat(document,rpc,link): theuth/docs/1 forwarding — legs to the hub, hub serve, unary admin forward, guests on the mesh uplink` (`5569b76`)

### Hub gate (`crates/document/src/hub.rs`)
C2's gate stands (Follower + reconcile; gain → `watch::start`; loss → flush exports, snapshot + unload every open Doc, `DocEvent::Closed{hub_moved}`; single engine = hub). Added:
- `Hub::route() -> Result<Option<EngineId>, DocError>`: `Ok(None)` serve here, `Ok(Some(hub))` forward, `Err(NotHub{None})` refuse (nobody holds the lease — nothing to forward to).
- `sync_hub(me: ActorRef<Self>)` takes the actor ref from the handler `Context` instead of the registry lookup (two actors in one process can now follow leases; the plugin behaviour is unchanged). Either flip clears the forwarding legs.

### Streaming forward — front door (`crates/document/src/relay.rs`, `actor.rs`, `share.rs`)
- `relay::Msg` gains two variants **beside** the C2 set (tags append, postcard-compatible): `Hello { member: Option<String> }` (tag 7, first frame: who the client is — a guest's static base64url, or `None` = owner) and `SetName { doc, name }` (tag 8; a member's `doc_set_name` must ride the member's leg or the hub would see the owner).
- `relay::Client` (`Conn(Option<u64>)` for the owner's connection | `Member(String)` by static), `relay::Cmd` (`Open`/`Subscribe`/`Update`/`Awareness`/`Join`/`SetName`, each carrying the caller's `ReplySender`), `relay::Legs { transport, legs: HashMap<(Client, DocId), Sender<Cmd>> }` on `DocumentActor.legs`, `relay::SetTransport(Arc<dyn Transport>)` (`tell`; set by `serve` once the mesh registered a transport; tests hand in a loopback).
- `DocumentActor::forward(hub, doc, client, cmd)`: **one leg per (client, doc)**, opened lazily by the first hub-only call, reused while its task lives, re-opened on the next call after it ended. No transport / hub not in `peers()` / leg queue full → the cmd answers `NotHub{hub}` (the RPC layer types it `-32011`; the client sees the error, as before Y).
- `run_leg`: `transport.open(hub, ALPN_DOCS)` → `Hello` → commands out as frames, frames in: `Event` whose JSON parses as a `DocEvent` → the subscription's `mpsc` (the same receiver `doc_subscribe` already reads, so `lib/doc/provider.ts` is untouched); `Event` that does not (no `type` key: `DocOpen`, `Joined`, `null`) → the oldest pending reply in send order; `Closed{reason}` → `DocEvent::Closed{reason}` to the subscriber, every pending reply fails with `reason` (`DocError::parse`, so `NotHub:<id>` from a hub that just lost the lease stays typed), the leg ends. Stream loss/EOF = `hub_moved`. `Update` over `MAX_UPDATE_B64` is refused at the front door before any frame. `Update`/`Awareness` are answered `Ok(())` once written (the wire has no per-call ack; a hub refusal arrives as `Closed`).
- Handlers: `OpenDoc`, `ApplyUpdate`, `Subscribe`, `AwarenessUpdateMsg` (`actor.rs`) and `Join`, `SetName` (`share.rs`) are now `DelegatedReply<…>` (transparent to `ask`; `doc_send` unchanged) and `match self.hub.route()` → the old body (`open_doc`, `apply_update`, `subscribe`, `awareness_update`, `join`, `set_name`) or `forward`. Everything else keeps C2's gate.

### Streaming forward — hub side (`relay.rs`)
- `relay::serve(rt, actor)`: waits `replica::transport(&rt)`, tells the actor `SetTransport`, then `serve_on(transport, actor, shutdown)`: accepts `ALPN_DOCS` streams, one task per stream (`serve_stream`).
- `serve_stream`: mints one connection id (`rand::random::<u64>()`, as `RpcPeer` does) for the stream; `Hello` fixes the principal (`Owner{device}` | `Member{pub_, device}`); `Open` → `OpenDoc` → `Event(DocOpen JSON)`; `Subscribe` → `Subscribe` → the receiver is pumped as `Event` frames and its `Closed` as `Msg::Closed`; `Update` → `ApplyUpdate`; `Awareness` → `AwarenessUpdateMsg` (errors dropped, best-effort); `Join` → `share::Join` → `Event(Joined JSON)`; `SetName` → `SetName` → `Event(null)`. Any handler error → `Closed{doc, reason}` and the stream ends. Admission, roles, fan-out skipping (the stream's own updates are not echoed) and the awareness rate cap are the actor's, unchanged. The plugin spawns `serve` (disclosed `lib.rs` one-liner).

### Unary admin forward (`crates/rpc/src/forward.rs`, `api.rs`)
- `forward::to_peer(ext, peer, method, params) -> RpcResult<Option<T>>`: one verbatim JSON-RPC over `theuth/rpc/1`, never a second hop (`Forwarded` marker), `Ok(None)` when it cannot be sent from here (no transport, peer not connected); `route`'s `Forward` arm now uses it.
- `api::doc_forward(ext, method, params, r)`: when `r` is `-32011` with `data.NotHub.hub = Some(h)`, re-issue to `PeerId(h)`; `hub: null` or unreachable → the refusal stands. Wired into `doc_resolve` (the `Ensure` fallback), `doc_members`, `doc_member_set`, `doc_member_revoke` (then `DropGuest` locally as well as on the hub), `doc_link_create` (forwarded verbatim; the hub builds the URL from its own `LinkStatus` — same mesh id), `doc_link_revoke`, `doc_restore`, `doc_undo_agent`, `doc_create`, `doc_rename`, `doc_delete`. Those six trait methods gained `with_extensions` (client signature unchanged). Member-callable methods (`doc_open/update/awareness/subscribe/join/set_name`) never take this path: the streaming leg carries the principal.

### Guests on the mesh uplink (`crates/link/src/conn.rs`)
- `conn::Uplink { Engine, Mesh }`; `run(c)` = engine (unchanged behaviour), **`run_mesh(c)`** = the mesh identity: `respond` with `State::mesh_identity`, `known` = `devices.by_key` (the replicated `devices` rows, active only), PSK chooser = `PairHello.pending` → `Pending::take(id)`, else `Pending::single_live()`, `None` when `max_devices` is reached. An unknown static → `Handshake::Guest` → the rate limit, `conns[id].device = key`, msg2, `run_guest` — exactly the engine path (`GUEST_METHODS`, 5 s join deadline, refusal budget). `commit_pairing(.., uplink)`: on the mesh the PSK was already consumed from the replicated rows, so the engine's `live_pairing` check is skipped; it also publishes `LinkEvent::DevicePaired{id}`.
- K's `mesh_uplink::run_once` spawns `conn::run_mesh` per `OPEN` (that file is K's; not touched here).
- Guests arriving on the **engine** uplink of a non-hub engine are forwarded through the legs above: `doc_join` → `Cmd::Join` on the `Client::Member(static)` leg (hub verifies the proof — it is engine-agnostic), then `doc_open`/`doc_subscribe`/`doc_update` on the same leg.

### Share URL / DropGuest (C2)
Unchanged: the URL names the mesh device id; `DropGuest` loops `State::conns` by static — the mesh uplink's conns sit in the same map (`run_mesh` registers the guest static there). `doc_member_revoke` now drops on both the front door and the hub.

### Failover / no duplication
On `Closed{hub_moved}` from the hub the leg ends and the client's subscription hears the same reason; the client re-opens; its `doc_open(sv)` opens a leg to the holder of the day (or is served locally when this engine gained it). The document id is deterministic per binding and one Yjs lineage — the old hub snapshotted on loss and the mirror carried the files — so the state vector asks only for what the client lacks: no duplicated blocks (asserted by the test below: the client that has everything gets an empty diff).

## Commit 2 — `docs(69): as-built Y`
This report as `docs/design/69-as-built-Y.md`.

## Deviations
1. **Two `Msg` variants added beside the contract's seven** (`Hello` 7, `SetName` 8). Without `Hello` the hub could not tell an owner's leg from a member's; without `SetName` a guest's rename would be forwarded as the owner. Tags append; the seven contract variants are byte-identical.
2. **Member legs key by static, not connection** (`Client::Member`): `share::Join` names no connection. Ceiling marked `ponytail:` in `relay.rs`: two conns of one guest static on one doc share a hub-side conn id, so neither sees the other's updates through fan-out (they re-sync on the next open).
3. **`share.rs` edited** (Join and SetName handlers; not in my allow-list): both are hub-only member calls that must ride the leg; a few lines each, plus imports.
4. **No per-call ack on the wire** (contract shape): forwarded `doc_update`/`doc_awareness` answer `Ok(())` once written; a hub refusal (`viewer_readonly`, `not_member`, …) arrives as `Closed{reason}` on the subscription and ends the leg. Locally those are call errors. `member_outcome` therefore marks a forwarded guest conn joined on `Ok` before the hub has spoken; a refusal still closes the leg.
5. **`sync_hub` takes the `ActorRef` from the `Context`** instead of `actor_ref()` (registry lookup) so two actors can follow leases in one process (the round-trip test). The export path still uses the registered name (the test registers the hub actor).
6. **Mesh pairing commit** touched (`commit_pairing(.., Uplink)`): needed so `run_mesh` compiles a complete conn path; the mesh's `live_pairing` check is skipped because `respond` already consumed the pending row. K owns the pairing flow and may replace this.
7. **Test 4 as specified (fake relay + two engines + real guest handshake end to end) is not one test**: the relay + uplink + rpc dispatch stack is outside my allow-list. Split into (a) `document`: two actors over a loopback transport — subscribe/open/update/awareness through the non-hub, fan-out to the hub's and another forwarded client, own updates not echoed, `UpdateTooLarge` at the front door, lease loss → `hub_moved` on every subscriber, snapshot moved to B (the mirror's job, done by hand in the test), B gains, reopen on B serves the same text, A (now off the hub) forwards to B and a client with everything gets an empty diff; (b) `link`: a guest's IK against the **mesh** static on `run_mesh` → registered under its static → gate refuses `doc_link_create` (`-32601`), `doc_join` dispatches as the guest → cancelling the `conns` handle (DropGuest's one-liner) closes with `CLOSE 0`; the engine static is refused on the mesh uplink.
8. `Legs.transport` comes from `relay::serve` (`SetTransport`) rather than a per-call `replica::actor()` lookup: testable, and one place.
9. `doc_link_create` is forwarded verbatim (the hub computes the URL) rather than building the URL here and forwarding only `LinkCreate`: the mesh id is the same on every engine; a pre-mesh single engine is its own hub anyway.

## Not built
- Agent tool edits off the hub (`Edit`, `ImportMarkdown`, `Ensure` from tools/knowledge/artifact/meeting — decision 11's last clause): still answer `NotHub:<id>` (String form). The owner plugins' callers and `tools.rs` are outside my allow-list; the RPC `doc_*` surface is complete.
- K's `mesh_uplink::run_once` (dial/register/serve) — K; it must spawn `conn::run_mesh`.
- `DropGuest` end to end through a `LinkActor` (its constructor is private to `lib.rs`); the mechanism it relies on is tested in `conn.rs`.
- TS: nothing exported changed (`Msg` is not `ts_rs`); `bunx tsc` not run.

## Tests written
- `relay.rs`: `frames_round_trip_with_matching_tag` (extended with tags 7–8), `forward_open_update_awareness_round_trip_then_failover_without_duplication`, `forward_refuses_with_not_hub_when_the_hub_cannot_be_reached`.
- `conn.rs`: `mesh_uplink_conn_admits_a_guest_against_the_mesh_static_and_drop_guest_closes_it`.
- Existing `tests/actor.rs::hub_gate_follows_the_lease` still holds (off the hub with no transport → `NotHub:other`); not run (needs the helper binary).

## cargo check / test
```
cargo check -p document -p rpc -p link --tests
  Finished `dev` profile — 0 errors; 2 pre-existing warnings (`vlink`, `elink` unused in crates/document/tests/actor.rs:774,806)
cargo test -p document -p rpc -p link --lib
  document: test result: ok. 32 passed; 0 failed; 0 ignored
  link:     test result: ok. 22 passed; 0 failed; 0 ignored
  rpc:      test result: ok. 29 passed; 0 failed; 0 ignored
```

## Disclosed edits outside the allow-list
- `crates/document/src/share.rs` (Join/SetName routing), `crates/document/src/lib.rs` (`tokio::spawn(relay::serve(..))`), `crates/document/Cargo.toml` (`futures`; dev `parking_lot`), `Cargo.lock`.

DONE Y
