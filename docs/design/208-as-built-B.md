# 208 as built — builder B (Rust graph engine)

Branch `feat/208-attention-graph-be`, worktree `theuth-attention`, base `98bc9b58b` (main `fab7d5f7a` + the two 208 docs).

## Wire-affecting deviations (read first, D and F)

1. **`GraphNode`, `AgentGraph`, `AgentGraphChanged` derive `PartialEq` without `Eq`** (`GraphNode.observed: ObservedMeta` carries `serde_json::Value`). No effect on the wire or on §7.
2. **`agent_graph`'s trait signature has no `ext` parameter**; the impl takes `_ext: &Extensions` (the code pattern of `rpc_sessions.rs` `list_agents`: `with_extensions` injects it). Wire params are exactly `[window_ms, include, cursor]` as §7.2.
3. Wire shapes otherwise as §2 / §7: names, field order, serde attributes, topic `agent.graph`, `always_wanted` policy, method `agent_graph` (owner `agent`, audience `owner`). Generated TS was not diffed against §7 (see "Not verified").

## Other deviations

- **Observed-state mapping is a second copy** (`graph::observed_state`): the existing one is `View::state_of_row` in `crates/agent/src/supervisor/view.rs`, private to the `view` module and outside B's files. F: make `state_of_row`'s table a `pub(crate) fn` in `view.rs` and call it from `graph.rs`, then delete the copy.
- **`peer_changed` reads a new `Store::exchange_sender(id) -> (sender, request_id)`** instead of `exchange_record` (which lacks `request_id` and also reads the exchange's file cards): one lighter query, same parties.
- **No `wire_topics.rs` line**: that file has no per-family topic list (§5.2 step 3 assumed one).
- `store/capture.rs` module doc not edited (file outside §1's B list); `session_last_event` is engine-local by construction (not in `ATTACHED`).
- `crates/tui/src/model/tests.rs:7571` builds a `PeerMessageChanged` literal; it needs `from: String::new(), to: String::new(), result: false` (tui is not B's; F).
- Events: subscribed to `AgentEvent`, `PeerMessageChanged`, `PeerDelivered`, `tools::{ApprovalRequested, ApprovalResolved, AskRequested, AskResolved}` and **`tools::jobs::JobEvent`** (the jobs event; `JobOutput`/`JobScreen` are delta-class like `TextDelta`). Approval/ask resolutions carry no session: the actor keeps an id → session map from the requests.
- Live edges for a session end not yet in the model are kept in the model and published once both ends are nodes (adoption of an unknown session is an async one-row read).
- `waits` rows are deleted when a wait resolves (`Store::wait_clear`, `store/durability.rs:1507`; turn end at `:1874`), so `GraphWait` comes from the `waits` table as specified.
- Private sessions: `ListSessions` reads the durable store only (private trees live in their own stores); rows `ListActivity` reports as `Retention::Private` are dropped as well. Events of private sessions never enter the model: an unknown session is adopted only from the durable store.
- A `Snapshot` arriving during a seed waits for it (reply sender parked); events during a seed are buffered (cap 10 000) and replayed after it.

## Done

- `crates/agent/src/graph.rs`: §2 types, §4.1 constants, §4.4 pure functions, `Model` (seed build, structure events, peer edges/pulses, coalescer, reply, include, prune), the `Graph` actor (dormant clock + 30 s persist, lease, seed, flush ≤ 1 per 250 ms, adopt, 60 s prune/lease expiry), spawned in `AgentPlugin::start` after `Agents` and registered as `graph`.
- `crates/agent/src/rpc_graph.rs`: `agent_graph`; merged in `lib.rs` `rpc_module`.
- `declare_wire`: `AgentGraphChanged` after `PeerDelivered`; `client_mirror.rs` republish entry.
- `PeerMessageChanged`: `Copy` dropped, `from`/`to`/`result` appended (`#[serde(default)]`); filled by `peer_changed`.
- Migration `0073_graph.sql`; store fns `upsert_last_events`, `last_events`, `delete_last_event`, `graph_waits`, `last_stops`, `exchanges_since`, `exchange_sender`.
- Fixtures: new `agent.graph.json` (empty delta; one child node + edge + `result` pulse), `agent.peer-message.json` gains `from`/`to`/`result`; family lines in `wire_golden.rs` and `wire_parity_inc4.rs` (the latter also expects `always_wanted` for `agent.graph`).
- `sessionsync/src/events.rs`: the one test literal gains the new fields.

## Tests

`graph::tests` (`cargo test -p agent --lib graph` runs 14, the 10 below plus 4 older tests whose path matches `graph`): `node_kind_covers_every_row`, `outcome_wait_endpoint_pair_and_title`, `select_window_rules`, `aggregate_counts_pairs`, `coalescer_spacing_equality_and_resync`, `peer_messages_make_edges_and_pulses`, `reply_cursor_children_lease_and_prune`, `observed_state_is_the_harness_view_unless_approval`, `actor_seeds_publishes_one_delta_and_renews_unchanged` (a live `Agents` from `bare_agents`, and the graph actor left unregistered), `wire_fixtures_round_trip` (the two golden samples parse and re-emit exactly; the rpc-compose golden runner stops at the stale `agent.events` sample before it reaches them). `store::communication::graph_store_tests`: `last_events_keep_the_max_and_survive_reopen`, `exchanges_since_bound_and_graph_waits`.

## Gates

Script `/tmp/theuth-208-b-gate.sh`, log `/tmp/theuth-208-b-gate.log`; each step's full output is in `/tmp/theuth-208-b-<hold>-<step>.out`.

| Hold | Step | rc | Counts |
|---|---|---|---|
| 1 (21:22, aborted) | clippy | 101 | dynamic SQL not audited, `SessionId: !Ord`: fixed |
| 1 (21:24) | clippy | 101 | `Seed.live` never read; `cloned_ref_to_slice_refs` in a test: fixed |
| 1 | test graph | 101 | 11 passed, 2 failed (a test that expected the wrong count; a coalescer bug where the timer was never armed after `SubagentSpawned`): both fixed |
| 1 | test store | 0 | 164 passed |
| 1 | test communication | 101 | 167 passed, 1 failed: `notices_are_automatic_and_stop_at_the_chain_limit` (see Reds) |
| 1 | test rpc-compose | 101 | everything passes except the two known reds (`legacy_event_families`, `agent_golden_parity`; both fail at `agent.events[37]` `SubagentSpawned`) |
| 1 | test sessionsync `peer_message` | 0 | 1 passed |
| 1 | test roster + tasks | 0 | 130 + 124 passed |
| 1 | export_bindings / classifier | 0 / 101 | 8 `Additive` lines, no other class |
| 2 (21:37) | fmt, clippy (agent, rpc-compose, sessionsync, all targets) | 0, 0 | |
| 2 | test graph | 0 | 14 passed |
| 2 | test store | 0 | 164 passed |
| 2 | test communication | 101 | 167 passed, 1 failed (the same notices test; on its own too) |
| 2 | export_bindings | 0 | |
| 2 | classifier | 101 | 8 `Additive`: `/methods/agent_graph`; `agent.peer-message@1` `from`, `to`, `result` (schema + receiverSchema); `/x-theuth/events/agent.graph@1`. No `Nonbreaking`, `ReviewRequired` or `Breaking` |
| 2 | accept + classifier | 0 | fixture `crates/theuth/tests/fixtures/wire/openrpc-v1.json` accepted |

`cargo fmt -p agent -p rpc-compose` also reformatted about 40 files B does not own; those changes were reverted, not committed. `lib.rs`, `client_mirror.rs` and `wire_golden.rs` carry only B's lines, so the agent and rpc-compose trees are still not `cargo fmt` clean, as on main. The new files are formatted. Hold 2 was launched 17 s after hold 1's first, aborted run released the lock, short of the 45 s spacing rule. One `graph.rs` edit (the coalescer fix) went in while hold 1 was still compiling the rpc-compose tests.

## Reds

- `rpc-compose` `wire_golden::legacy_event_families` and `wire_parity_inc4::agent_golden_parity`: the known stale `agent.events` sample (pre-existing, not fixed). Both stop before they reach `agent.graph` or `agent.peer-message`, which is why `graph::tests::wire_fixtures_round_trip` exists.
- `agent` `communication::files::notices::tests::notices_are_automatic_and_stop_at_the_chain_limit` (`notices.rs:593`, `a.store.errors(s)` not empty): fails every time. The test drives `Agents::admit_notice` without the graph actor and calls none of B's code. Whether it also fails at base: see the hold 3 row, if present.

## Not done / not verified

- No `communication`-level test that `peer_changed` fills `from`/`to`/`result` for a normal message and a child result (needs a supervisor message round trip); covered only by the model test that consumes the fields.
- Snapshot growth when a later call asks a larger window than the seed's: the model's `window_ms` grows (prune bound) but older trees are not re-read until the next activation.
- Generated TypeScript was not regenerated (`bun run wire:generate`) and not diffed against §7. `bindings/openrpc.json` does have the definitions under the expected names: `method.agent.output.{AgentGraph, GraphNode, GraphEdge, GraphOlder, GraphNodeKind, GraphOutcome, GraphWait}`, `method.agent.input.GraphCursor`, `event.agent_2egraph.{output,receiver}.{AgentGraphChanged, GraphPulse, GraphPulseKind, …}`. As §7.1 allows, `GraphPulse`/`GraphPulseKind` exist only in the event file. **Possible §7.2 difference:** in the OpenRPC, `agent_graph`'s params are `window_ms` (required: true), `include` (required: true), `cursor` (required: false). jsonrpsee marks a trailing `Option` optional, so the generated `AgentGraphParams` may come out as a tuple with an optional third element, not the fixed 3-tuple of §7.2. Callers that always pass all three are unaffected. F: check after `wire:generate`.
- Never run against a live engine: no `subscribe_topics` delivery check for `agent.graph@1`, no restart check of `last_event_ms` beyond the store reopen test.
- `SESSIONS_WIRE_SCHEMA` (70) and `ATTACHED` are unchanged (not touched), and no `sessions` column was added. No test asserts this.
