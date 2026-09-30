# 79 as-built — F3 (mesh and secrets fixes from the live smoke)

Branch `fix/79-F3` off `feature/79` @ `b4e5c38b`. Owned: `crates/mesh/**`, `crates/secrets/**`, the roster→secrets tell. Brief: `/tmp/79/F3.md` (bugs 5 and 9 of `79-smoke.md`, `SetEngine` from mesh, optional QUIC payload-cap check).

## Commits

1. `6af18cce fix(mesh): keep injected peer addresses across the peer's own advertisement` — **smoke bug 9.**
   - `pairing::merge`: a replacing *live* row keeps the addresses the local row already had — `addrs = union(local, incoming)`, local first, deduped (`pairing::union`, pub). A tombstone keeps its own (empty) list. New rows and older incoming rows unchanged.
   - `Net::add_peer`: the `PairAck.me` row is seeded with the pairing string's `addrs` (`union(payload.addrs, me.addrs)`) before `learn` — the dialed `127.0.0.1:<mesh_port>` was being dropped right there, at pairing time, not later: the inviter's `me()` carries only what its own endpoint sees (relay + `172.16.0.6:7800`).
   - `mesh_status.peers[].addrs` is `PeerEntry.addrs` verbatim (`peer_of`), so both now show; `addr_for` (the connector's dial hints) reads the same column, so the host keeps dialing the loopback port after a restart.
   - Test `pairing::tests::merge_keeps_the_local_addrs_of_a_replaced_live_row`: add with A, advertisement with B (and A again) → `[A, B]`; an older advertisement changes nothing; a removal carries no addrs.
2. `359533c7 fix(mesh): the mesh name is set by the first engine, never by a joiner` — **smoke bug 5.**
   - Root cause: `Store::meta_default` stamped every default `(wall 0, counter 0, origin)`; two fresh engines' defaults tied on the clock and the **larger origin id** won the per-column LWW merge — the guest's `1df3f633…` beat the host's.
   - `Store::meta_default(key, value, first_seen_ms)` stamps `(0, u32::MAX - first_seen_s, origin)`: the older identity's default carries the larger stamp and wins; an explicit `meta_set` (real wall clock) still beats every default. `first_seen_ms` is the identity file's (`identity.first_seen_ms`, already on `Ctx`). This is what the 69 decision 8 comment at `lib.rs:685` claimed ("stamped at this engine's first_seen so the oldest engine's default wins") and the code never did.
   - `MeshPlugin::start`: `alone = store.peers().is_empty()` (live peers at start); the default is written only when `alone`. A joiner — any engine with peers at start — takes the row replication brings.
   - Tests: `store::tests::the_older_engines_default_wins_a_tie_of_defaults` (host `a-host` first seen at T, VM `z-vm` at T+100 s, larger id; batches cross in both orders; both read the host's name), `meta_default_keeps_the_first_value_and_stamps_zero` and `an_explicit_rename_beats_a_later_default_across_engines` updated for the new parameter (a `side()` helper hoisted to module level).
   - Test note: the two-order test pumps batches by `ReplicaHandle::subscribe()` feed, not `flush()` return values — an `apply` captures the receiver's pending local row into a batch `flush` never returns (the first version of the test failed for that reason, not because of the fix).
3. `f8c3f535 fix(mesh): mesh tells secrets its engine id; drop the roster→secrets tell` — **`secrets::SetEngine` from mesh.**
   - `crates/mesh/src/lib.rs`: `tell_secrets_engine(rt, id)` = `lookup_when::<secrets::SecretsActor>(SECRETS_ACTOR)` then `tell(SetEngine(id.0))`, spawned right after `Net::bind` succeeds (before the actor, the attach and the start-up rows). Start order does not matter (`lookup_when` waits; bounded only by shutdown).
   - **Edit outside owned crates (flagged):** `crates/roster/src/lib.rs:2100-2108` — the `lookup_when(secrets)` + `tell(SetEngine)` block and its two comment lines removed; the `SetSelf` tell stays. `roster` keeps its `secrets` dependency (the attention inbox lists `secret_requests`), so `Cargo.toml` is untouched.
   - `crates/secrets/src/lib.rs`: `SetEngine` and `SecretsActor.engine` doc comments say "mesh", not "roster". No code change.
   - Test `tests::secrets_learns_the_engine_id_from_mesh` (mesh crate): a registered `SecretsActor` over a temp file-keyed vault; `tell_secrets_engine(rt, "engine-f3")`; a `Request` opened from a task; the `ListRequests` row carries `engine == "engine-f3"` (the hostname otherwise). No new dev-dependency (`Runtime::new(Default::default(), …)`).
4. `docs(79)` — this report; `79-sandbox-vm.md` §16 rows "§6.3 loopback path" (cap findings) and "§6.6 secrets" (id from mesh).

## Optional: `loopback_mesh` payload cap (checked, **not built**)

iroh 1.2.0 (`Cargo.lock`), read from the registry source:

- **Per connection, dial side only:** `Endpoint::connect_with_opts(addr, alpn, ConnectOptions::new().with_transport_config(QuicTransportConfig))` (`iroh-1.2.0/src/endpoint.rs:1100-1143`). `QuicTransportConfig::builder()` exposes `initial_mtu(u16)` (default 1200), `min_mtu`, `mtu_discovery_config(Option<MtuDiscoveryConfig>)` (`endpoint/quic.rs:286-323`); `MtuDiscoveryConfig::upper_bound(u16)` is quinn's PMTUD ceiling, re-exported at `iroh::endpoint::MtuDiscoveryConfig`. `initial_mtu(1200) + mtu_discovery_config(None)`, or `upper_bound(1452)`, keeps *our* datagrams on that connection ≤ 1452 B.
- **Global otherwise:** accepted connections use `static_config.transport_config` (`endpoint.rs:1760`, `Builder::transport_config` at `:676`); `iroh-gossip` 0.101 dials with plain `endpoint.connect` (`iroh-gossip-0.101.0/src/net.rs:1026`) — not configurable per connection. quinn's `max_udp_payload_size` (what we *accept*) is `EndpointConfig`, not exposed per connection either.
- The mesh's own dial sites (`net.rs:571` add_peer, `:951` connector, `:991` the replica/ALPN transport) could take the per-connection cap when the peer's row holds a loopback address, but the guest's connector also dials the host (accepted → global config) and gossip dials both ways: a dial-side cap alone does not make host→guest ≥ 1500 B datagrams impossible. Per the brief ("if only global, do NOT change it"): **no code**. `sandbox.vm.loopback_mesh` default stays `false`.
- What is possible, for I/Sky: the complete pin is `Builder::transport_config(QuicTransportConfig::builder().mtu_discovery_config(None).build())` on the host endpoint (`Net::bind`) — every mesh path then runs 1200 B datagrams (the relay path is small-framed anyway; direct LAN paths lose ~20 % payload efficiency). Or set it only when `sandbox.vm.loopback_mesh` is true, as a `mesh` config knob. Either flips decision 10 into a default-on candidate; both are a one-line change in `Net::bind` plus a config field.

## Deviations from the brief

- Bug 5: the brief offers "write the default with the minimum HLC the replica accepts, or only when the table has no row after the first sync". Built: the age-ranked stamp plus the no-peers-at-start guard. Reason: two fresh engines (the smoke's host and its first VM) *both* start alone and both legitimately write before pairing — only a tie-break that is not the origin id fixes that case; "after the first sync" needs a sync-completion signal the mesh has no hook for. An explicit `meta_set` still beats every default (test kept).
- Bug 5 "also expose `mesh_name` in the pairing string or the first sync (if cheap)": **not built.** The replica's newcomer snapshot delivers `mesh_meta` within the first sync; carrying the name in the pairing payload would add a field the payload's `Payload` JSON tolerates but changes a versioned string for a value that arrives seconds later anyway.
- Bug 9: union chosen over a `pinned_addrs` column (no migration, no second address list through `MeshDialer`/`addr_for`). Ceiling (marked `ponytail:` in `pairing.rs`): addresses only accumulate; a peer that changes networks often grows iroh's dial list. Not observed as a problem; prune when it is.
- Bug 9 only covers the `Msg::Peers` / pairing merge path. The `peers` table also replicates row-per-column via the record replica: a *third* engine's row about the VM (its `addrs_json`, newer hlc) would still overwrite the host's list. Not in the smoke (two engines); flagged for I.

## Not built

- The optional payload cap (above). No mesh config knob added.
- No `mesh_name` in the pairing string.

## Tests written

`pairing::tests::merge_keeps_the_local_addrs_of_a_replaced_live_row`, `store::tests::the_older_engines_default_wins_a_tie_of_defaults`, `tests::secrets_learns_the_engine_id_from_mesh` (all `crates/mesh`). Two existing store tests updated for the `first_seen_ms` parameter.

## Gate

Run from the worktree with `~/.rustup/toolchains/stable-aarch64-apple-darwin/bin` on `PATH` (no `cargo` shim on the login PATH):

| command | result |
|---|---|
| `cargo check -p mesh -p secrets -p roster -p rpc --tests` | `Finished dev … in 14m 19s` |
| `cargo clippy -p mesh -p secrets -- -D warnings` | `Finished dev … in 44.77s`, no warnings |
| `cargo test -p mesh --lib` | `26 passed; 0 failed` |
| `cargo test -p secrets --lib` | `22 passed; 0 failed` |
| `cargo test -p roster --lib` | `48 passed; 0 failed` (the tell removal breaks nothing) |

## Edits outside owned files (flagged)

- `crates/roster/src/lib.rs` — removed the roster→secrets `SetEngine` tell (commit 3).
- `docs/design/79-sandbox-vm.md` — §16 as-built table, two rows (commit 4).

## Follow-ups for the integrator

1. Decide the global QUIC MTU pin (above) before flipping `sandbox.vm.loopback_mesh`; it is the only complete one.
2. Replicated `peers.addrs_json` from a third engine can still overwrite an injected address (per-column LWW); if a three-engine smoke shows it, make the column's merge a union in the mesh (a `pinned_addrs` column then) — not a replica change.
3. `79-as-built-H.md` deviation "secrets learns its mesh id from the roster" and `79-as-built-INTEGRATE.md` open item 7 are closed by commit 3; left as history.
4. A live re-run of smoke check 3/13: `mesh_status.mesh_name` must stay the host's after `sandbox_create`; `peers[lab].addrs` must list `ip:127.0.0.1:<mesh_port>` alongside the relay and NAT addresses.
