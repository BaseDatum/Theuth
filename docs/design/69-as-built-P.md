# 69 as built — worker P (peer messages follow the owner: `send_message` / `discover_peers` across engines)

Branch `feature/69-devices-p`, worktree `theuth-69-p`. Ruling (Sky, 2026-09-25): agents on different engines coordinate by `send_message`, not only through tasks — "sessions everywhere" also means "peers everywhere". Binds to 68 decisions 3–8 and 68 F's front door.

## Commits

### 1. `f6db968` `feat(agent,rpc): peer messages follow the owner — peer_deliver over theuth/rpc/1; remote rows in discover_peers`

**Delivery (design 1, 2, 3, 6).** `crates/agent/src/supervisor.rs`:
- `Message<Deliver>` body moved to `Agents::deliver(m, id: Option<Uuid>)` (the handler is one line). Right after the exchange is `accepted` in the sender's `peer_exchanges` — and after the local reply-settle branch, which only fires for a waiter on this engine — `remote_owner(to)` reads the recipient's replicated row: `!here && owner != ''` → `forward_peer`, else today's `enqueue_peer` path untouched. Resolution is by the replicated `sessions` row only; addresses (`session:<uuid>`, `peer:<name>`, `parent`, child names) are unchanged.
- `forward_peer`: drops the id from `enqueued` (nothing local consumes it), builds `PeerEnvelope{id, from, text (redacted), reply_to, request_id}` and calls `communication::mesh::forward(transport, owner, session, &envelope)`. Owner answered ok → receipt stays `accepted`, `await_reply` registers the same exact waiter a local recipient gets. Any failure → `store.fail_exchange(id, error)` and `Ok(Delivered{receipt: failed, reply: None})` — the model reads a normal `failed` receipt row. Never a take.
- `Agents.transport: Option<Arc<dyn replica::Transport>>`, fetched once from `replica::actor()` → `GetTransport` on first need (`Agents::transport()`); tests inject it. No mesh → `OwnerUnreachable` (why `mesh not running`).
- `Message<PeerDeliver>` → `Agents::peer_deliver(m)`: the owner's intake. Refuses when the row is not `here` (`session:<id> is owned by <owner>, not here` — the sender's replica was stale; no take), mirrors the sender's lineage into the policy (`mirror(envelope.from)`, as `resolve` does for a recipient), then runs `deliver(Deliver{from: envelope.from, to: session:<id>, await_reply: false, …}, Some(envelope.id))`. So the owner's engine evaluates its own communication policy (design 50: both engines must allow), redaction, size bound, quota and reply matching exactly as for a local sender.
- Replies: the replying session's `Deliver` (`reply_to` set) goes through the same code — its `accept_exchange` marks the owner's copy of the original `replied`, `remote_owner(original sender)` forwards it, and the sender's `peer_deliver` intake finds the waiter (`waits[id].session == to && .to == session:<replier>`) and settles it: `await_reply` works across engines because the reply arrives as a forwarded `Deliver`.

`crates/agent/src/communication/mesh.rs` (new):
- `METHOD = "peer_deliver"`, `TIMEOUT` 30 s (F's), `OWNER_UNREACHABLE = -32010`.
- `forward(transport, owner, session, &envelope) -> Result<PeerReceipt, Failure>`: requires `owner ∈ transport.peers()`, opens one `theuth/rpc/1` stream, writes one JSON-RPC request line (`{"jsonrpc":"2.0","id":1,"method":"peer_deliver","params":[session, envelope]}` — positional, trait order, exactly what F's `serve` dispatches with `Methods::raw_json_request`), reads one response line. `Failure::Unreachable{owner, why}` for not connected / transport error / timeout / malformed / a `-32010` from the owner (it moved the session on); `Failure::Refused(message)` for any other JSON-RPC error (the owner's `Deliver` refusal).
- `Failure::receipt_error()`: Unreachable → 68 F's `FrontDoorError` **byte for byte** (`{"OwnerUnreachable":{"owner":"<id>","last_seen":<ms|null>}}`, serialized from a mirror enum so field order matches; `last_seen` from `mesh::Status` like F's `unreachable`); Refused → `denied: <message>` when the message names `communication disabled`, else the owner's message verbatim.
- `engine_names()` / `engine_name()`: one `mesh::Status` ask per listing → `(self_name, id → name)`; no mesh actor → hostname and the id's first eight characters.
- `#[cfg(test)] loopback::{Loop, nodes}`: F's two-node switch, n nodes.

`crates/agent/src/communication.rs`: `PeerEnvelope` (ts-exported), `PeerDeliver` (`ask → Result<PeerReceipt, String>`), `PeerState::Remote`, `PeerInfo.engine: Option<String>` (serde default). `pub mod mesh`. `crates/agent/src/lib.rs`: re-exports `PeerDeliver`, `PeerEnvelope`.

`crates/agent/src/store/communication.rs`: `accept_in(conn, a, id: Option<Uuid>)` — the id is minted unless supplied; `Store::accept_exchange_as(id, &a)` beside `accept_exchange`. One id per message on both engines is what makes `reply_to` match without an id map.

**RPC (design 1).** `crates/rpc/src/api.rs`: `peer_deliver(session, envelope) -> PeerReceipt` in the trait beside `steer`; the impl `authorize`s the internal peer, then **requires the `forward::Forwarded` marker** (a client on the bridge gets `-32002 peer_deliver is mesh-internal`), then asks the supervisor `PeerDeliver`. No `forward::route` (never a take; the supervisor refuses a row it does not own). `crates/rpc/src/export.rs`: `agent::PeerEnvelope` in the bindings list.

**discover_peers (design 4).** `Agents::directory`: `engine_names()` once; a row with `!here && owner != ''` is `state: remote`, `engine: <owner's mesh name>`, no activity (it is left out of `activity_for`), `title` as before (own tree only), `tags` from the replicated row; local rows gain `engine: <self name>`. `remote_parent_row` carries `p.engine`; A2A `peer:` rows have `engine: None`. `crates/agent/src/peers.rs`: description names `engine` and `remote`; the text line appends ` engine=<name>` **only for remote rows** (local text unchanged). `arrange(active_only)` drops rows without activity, so remote rows never pass `active_only` — documented in the tool description ("never a `remote` one").

**Children across engines (design 5)**: untouched. `acp_child::uplink` still enters through `Deliver{to: "parent"}` (`Target::Uplink`), before the owner check.

## Deviations / decisions
1. ★ **Allow-list**: the brief's "You own" section was not in `/tmp/69/omp-P.md`; touched exactly the files the brief names plus `store/communication.rs` (one parameter, one method), `communication.rs` (types), `communication/network.rs` (one struct-literal field), `lib.rs` (re-export), `rpc/src/export.rs` (one line).
2. ★ **The agent crate writes the JSON-RPC line itself** (`serde_json`, ~40 lines) instead of calling `rpc::forward::forward`: `rpc` depends on `agent`, so the call cannot go the other way, and a hook installed by `rpc` would be more machinery than the line. Wire-compatibility with F's `serve` is proven by `rpc::forward::tests::peer_deliver_line_round_trips_through_the_owner_serve`.
3. ★ **Envelope id = sender's minted id**, kept by the owner's intake (`accept_exchange_as`) — ids are global like session ids under 68 — instead of remapping ids per engine via `request_id` (the 67 ACP uplink pattern). Dedup by `(sender, request_id)` still applies on the owner; a retried forward returns the existing row.
4. **Receipt state on the sender stays `accepted`** after the owner's intake commits (brief item 2); the owner's later `consumed`/`replied` transitions are its own rows (engine-local, 68 decision 8). The owner's copy of a *reply* it forwarded likewise stays `accepted` — the consumption happens on the sender's engine.
5. **`active_only` and remote rows**: no cheap replicated per-session "turn running" read model exists (`sessions_status` is a per-engine summary), so remote activity is unknown and remote rows are dropped by `active_only`; stated in the tool description.
6. **Local text output of `discover_peers` is unchanged** (`engine` only in `structured`, and ` engine=<name>` in text for remote rows) to keep the mesh-of-one behaviour byte-identical; the structured field is on every row as the brief asks.
7. **Policy re-evaluation on the owner** uses the owner's own `Policy::route(from, to)` with `from` mirrored from the replicated row: the sender's root scope (`owner`/`isolated`) is read from the replica, the owner's global switch and per-session opt-outs are the owner's. A refusal comes back as a JSON-RPC error → `failed` receipt with `denied: send_message: communication disabled: …`.
8. `remote_owner` uses `SessionSummary.here` (68: `owner == '' || owner == engine`), so a lone engine (`owner = ''`) and an unknown row both take the local path, per design 1.
9. `Agents.transport` is cached after the first successful `GetTransport`; the mesh registers one transport per process and replaces it only on restart, so no invalidation.

## Not built
- Owner-side `consumed`/`replied` propagation back to the sender's row beyond `accepted` (item 4).
- Remote activity for `active_only` (item 5).
- Full `cargo test -p agent --lib` / `-p rpc --lib` runs (the orchestrator's; P ran only its own tests).

## Tests written (P's own runs filtered to them, see Verification)
`agent` (`cargo test -p agent --lib`):
- `communication::mesh::tests::the_request_line_is_jsonrpc_and_the_owners_receipt_comes_back` — the exact request line (`method`, positional `[session, envelope]`) and the receipt parsed back.
- `communication::mesh::tests::owner_errors_become_refused_or_unreachable` — `-32000 … communication disabled` → `Refused` → `denied: …`; `-32010` → `Unreachable` → `{"OwnerUnreachable":{"owner":"d","last_seen":null}}`.
- `communication::mesh::tests::a_peer_that_is_not_connected_is_unreachable_without_a_stream`.
- `supervisor::tests::peer_messages_follow_the_owner_and_replies_round_trip` — two `Agents` values on two stores (engines `a`, `b`; each store seeded with the other's row through a store opened as the owner), loopback switch, each engine serving `peer_deliver` into its own `Agents::peer_deliver` (the shape `rpc::forward::serve` + `api::peer_deliver` have). `a_root → b_root` with `await_reply`: sender row `accepted`, not in `enqueued`, no actor for `b_root` on `a`, wait registered; owner row exists under the same id with `from = session:a_root`. `b_root → a_root` with `reply_to`: forwarded back, the wait resolves with `replied`, `reply = "42"`, `answered_by = session:b_root`; no waits left.
- `supervisor::tests::an_unreachable_owner_is_a_failed_receipt_in_the_front_door_shape` — `b` not on the switch: `failed`, `error == {"OwnerUnreachable":{"owner":"b","last_seen":null}}`, durable, no wait; `peer_deliver` for a row owned elsewhere is refused (`owned by b`).
- `supervisor::tests::directory_marks_rows_owned_elsewhere_as_remote` — from `a`: the `b` row is `remote`, `Root`, `engine = "b"`, no activity, dropped by `arrange(active_only)`; from `b`: the `a` row is `remote`, `engine = "a"`.
- `supervisor::tests::a_lone_engine_delivers_locally_as_before` — `agents_with(true)`, root → child: `remote_owner == None`, `accepted`, in `enqueued`, `transport` never consulted, directory rows not `remote` with `engine = hostname`.
- Test scaffolding: `agents_on(dir, engine, communication)` (`agents_with` now calls it), `two_engines()`, `serve_peer_deliver()`.

`rpc` (`cargo test -p rpc --lib`):
- `forward::tests::peer_deliver_line_round_trips_through_the_owner_serve` — `agent::communication::mesh::forward` against `forward::serve` with a fake jsonrpsee `peer_deliver`: params parse as `(SessionId, PeerEnvelope)`, the receipt returns, the dispatch carries `Forwarded(a)` and the internal `RpcPeer`; a `-32000` refusal is `Failure::Refused(message)`.

## Verification (exact results)
- `cargo check -p agent --tests` (commit 1): first run failed on two errors (my `use crate::communication::mesh` shadowed the `mesh` crate in `supervisor.rs`; renamed to `peer_mesh`), second run failed on a missing `use replica::Transport` in a test helper; third run `Finished dev profile [unoptimized + debuginfo] target(s) in 1m 11s`, no warnings. `cargo check -p rpc --tests`: `Finished dev profile [unoptimized + debuginfo] target(s) in 5m 56s`, no warnings.
- Tests run (design item 7 allows `cargo test -p agent --lib` / `cargo test -p rpc --lib`; filtered to P's tests, `monty` not needed by them):
  - `cargo test -p rpc --lib forward::` → `test result: ok. 7 passed; 0 failed` (F's 6 + `peer_deliver_line_round_trips_through_the_owner_serve`).
  - `cargo test -p agent --lib -- communication::mesh:: peer_messages_follow_the_owner an_unreachable_owner directory_marks_rows_owned_elsewhere a_lone_engine_delivers_locally` → `test result: ok. 7 passed; 0 failed; 300 filtered out; finished in 0.41s`.
- `just ts` → `5 passed` (`export_bindings`); `bindings/` is gitignored in this tree and the desktop copy (`crates/desktop/ui/src/rpc/bindings`) tracks no `Peer*` files, so nothing to commit. Generated `PeerState.ts` gains `"remote"`, `PeerInfo.ts` gains `engine: string | null`, `PeerEnvelope.ts` is new.
- No TypeScript sources touched; `bunx tsc` not run.

DONE P
