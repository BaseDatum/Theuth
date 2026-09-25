# 67 as-built N — remote children: caps, model, parent addressing, `harness@engine`

Worktree `theuth-67-nodes`, branch `feature/67-agents-nodes`. Closes the three gaps of 67 decision 4 / §8 on the existing 32 link: the parent sets `model`/`effort`/`tools` on a remote child, a remote child addresses its parent, and `agent: "<harness>@<engine>"` runs a node's own external harness. Only existing crates touched: `acp`, `nodes`, `agent`, `rpc` (+ one-line struct-literal/prompt-doc changes in `tui`, `theuth`).

## The wire (for the mesh worker: carry these unchanged over `theuth/acp`)

### `session/new` `_meta`
One namespaced key. Absent → the node behaves as for any ACP client. Present but malformed → JSON-RPC `invalid_params` with the serde error in `data`.

```json
"_meta": { "theuth": {
  "model":         "anthropic/claude-opus-4-8",   // optional; resolved on the node (native) or set through the harness's model option (harness)
  "effort":        "high",                        // optional; native children only
  "tools":         "read_only",                   // optional; read_only | mutating | destructive (= no ceiling)
  "parent":        "<parent theuth session id>",  // optional; makes the root a remote child: `parent` peer, uplink
  "parent_engine": "<caller's device id on the node, base64url(pub)[..8]>", // optional, display only
  "harness":       "omp"                          // optional; run this acp/agents.yaml entry of the node instead of a native session
}}
```
Rust: `acp::SessionMeta` (`crates/acp/src/meta.rs`), `acp::META_KEY = "theuth"`, `to_meta()` / `from_meta()`. Not sent on `session/load`.

### Child → parent peer message
ACP extension notification, agent → client, on the session's own conn:

```json
{ "jsonrpc": "2.0", "method": "_theuth/peer_message",
  "params": { "sessionId": "<the child's ACP session id>", "id": "<uuid v7, sender-minted>",
              "replyTo": "<uuid, optional>", "text": "<redacted, ≤ PEER_TEXT_MAX>" } }
```
Rust: `acp::PeerMessage` / `acp::PEER_MESSAGE_METHOD` (`crates/acp/src/peer.rs`), `notification()` → `UntypedMessage`, `parse(&UntypedMessage)`.
Parent → child stays the `<message …>`-wrapped `session/prompt` it is today; there is no second method.

## Per commit

### `b760cb6` feat(67): model, effort and tools travel to a remote theuth child
- `crates/acp/src/meta.rs` (new) — `SessionMeta`, tests (round trip, absent, malformed).
- `crates/acp/src/harness.rs` — `Connect.meta`; `Link::open(.., meta, ..)` puts it on `NewSessionRequest`; `Dialer::caller_id() -> Option<String>` (default `None`).
- `crates/nodes/src/dialer.rs` — `NodeDialer::caller_id` = `identity.id[..8]` (32 §4.1 device id).
- `crates/agent/src/peers.rs` — `cap_of` no longer refuses `tools` with `agent`; argument docs.
- `crates/agent/src/lifecycle.rs` — `CreateSessionOptions` gains `effort`, `cap: Option<Risk>` (later commits: `parent_session`, `parent_engine`, `harness`), all `#[serde(default)]`; literals in `agent/session_read.rs`, `tui/src/model.rs` use `..Default::default()`.
- (Also carries, because the files are shared: `crates/acp/src/peer.rs`, `Update::Peer`, `uuid` dep — commit 2's wire type. Commit 1 does not build standalone; the branch head does.)

### `bf5188d` feat(67): remote child can address its parent (A2A over the node conn)
- `crates/agent/src/supervisor.rs`
  - `session(acp: Option<(name, cwd, cap)>, pick)`; `acp_session(.., pick, cap)` builds `SessionMeta { model, effort, tools, parent, parent_engine }` for a `Transport::Dial`.
  - `SpawnChild`: an `agent` that is a registered node accepts `model`/`effort`/`tools` (model validated on the node, not against the local catalog); a local harness with `tools` gets `Limits: tools <cap>` as the first line of its brief.
  - Root `session()` honours the effort pick; `new_root(cwd, scope, model, effort)`; `create_durable(cwd, project, model, effort, cap, remote)`; `install_cap(id, cap)` shared with `SpawnChild` (registry `SetCap` + `Agents.caps`). `CreateSession` validates effort like `SetEffort` without touching the default or `config.yaml`.
  - `Agents.remote_parent: HashMap<SessionId, RemoteParent { session, engine }>`; `Target::Uplink`; `resolve(from, "parent")` → Uplink for such a root; `Agents::uplink` publishes `PeerUplink` on the bus and returns an `accepted` receipt `to: "parent"`; `await_reply` refused with a clear text; `directory()` lists the parent first (`address: "parent"`, `kind: Parent`, `relationship: Parent`, `send: true`, `reply: true`); `peer_context` names the parent session and engine; parent-side rows for node children now `reply: true` (32 §13.1's leaf rule superseded).
- `crates/agent/src/communication.rs` — `PeerKind::Parent`; bus type `PeerUplink { session, id, reply_to, text }`.
- `crates/agent/src/acp_child.rs` — `AcpChild::new(.., meta, ..)`; `Update::Peer(m)` → `uplink`: checks `m.session_id` is this harness's session, then `Deliver { from: child, to: "parent", reply_to, request_id: "uplink:<id>" }` to `Agents` — the ordinary peer route (switch, redaction, size bound, reply matching, durable exchange, `<message from="session:<child>">` into the parent).
- `crates/rpc/src/acp.rs` — `new_session_options(&NewSessionRequest)` → `CreateSessionOptions`; `Relay` subscribes `PeerUplink` and sends `_theuth/peer_message` on the owning conn.
- Tests: `acp::meta::tests`, `acp::peer::tests`, `rpc::acp::tests::session_new_meta_becomes_create_options`, `agent::supervisor::tests::remote_root_gets_cap_and_effort` (cap, effort, `parent` resolution, directory row, peer context, uplink receipt / await_reply refusal).

### `f522f67` feat(67): agent: "<harness>@<engine>" — external harness on a remote engine
- `crates/agent/src/peers.rs` — `parse_agent(s) -> (Option<harness>, engine)` (`rsplit_once('@')`; empty parts / second `@` are an error); `spawn_subagent` checks the engine is a node when a harness is named; node line of the description mentions `<harness>@<node>`. Test `agent_parses_as_optional_harness_at_engine`.
- `crates/agent/src/supervisor.rs` — `SpawnChild` classifies: native / node / external (local harness or `harness@node`); `model` allowed everywhere (local catalog check only for a local native child), `effort` only for theuth, `Limits:` line for any external harness. `acp_session` parses the stored name (`omp@build-A` survives resume in the row's `agent` column): a local harness gets `Connect.model`, a node gets `_meta` with `harness`. `directory` kind: `<node>` → `Node`, `<harness>@<node>` → `Acp`. `CreateSession` with `harness`: the root is an `AcpChild` of *this* engine's `acp/agents.yaml` entry (`self.session(None, None, Some((harness, cwd, None)), (model, None))`); unknown name → `acp::unknown` text (the names it has); private retention refused.
- `crates/acp/src/harness.rs` — `Connect.model`; after `session/new` (never on load) `session/set_config_option` on the model select (`model_option`, same lookup as `current_model`: `category: "model"`, else id/name containing "model"), `Harness.model()` reads the response; no option / refusal → warn, harness keeps its own.
- `crates/agent/src/lifecycle.rs` — `CreateSessionOptions.harness`.
- `crates/rpc/src/acp.rs` — passes `meta.harness`; an error for a remote-harness create is prefixed with this node's hostname. Test extended with the harness branch.
- `crates/theuth/harness.md` — the system-prompt line for `spawn_subagent` (one paragraph).

## Deviations (decided alone; nobody to ask)
1. **Parent → child stays on `session/prompt`.** The brief said "both directions" as a notification; the parent's `send_message` already reaches the child as a `<message>`-wrapped prompt with the parent-side id, and the child's `reply_to` on the uplink matches that id on the parent. A node-side inbound handler would need a new `ApiClient` method for no behavioural gain. Only child → parent is `_theuth/peer_message`.
2. **`await_reply` to a remote parent is refused** ("its answer arrives as a `<message>`"). The uplink has no durable exchange on the node; the parent's answer arrives as a prompt.
3. **The uplink is not stored on the node.** `peer_message{id}` on the node does not know the uplink id; the parent's exchange (`request_id: uplink:<id>`) is the durable record and dedupes re-sends.
4. **Caps stay in-memory** (`Agents.caps`, per `decisions/theuth-subagent-tool-cap-v1`); no store column. The "SessionSummary-visible marker" is not a store field — the board row (`SubagentInfo.cap`) is the existing surface and it reads `Agents.caps`, which now also holds remote roots.
5. **`PeerKind::Parent`** was added (ts-exported enum; `PeerRelationship::Parent` alone could not mark the row's backend). No Rust match is exhaustive over `PeerKind`.
6. **32 §13.1 "a node child remains a leaf"** is superseded for theuth-native node children: parent-side `PeerInfo.reply` is now `true` for `PeerKind::Node`.
7. **Effort for a harness** (local or remote) is refused; `effort` has no ACP equivalent. `model` is accepted for every `agent` kind.
8. **Local harness `model`** now also works (`Connect.model` → set_config_option) — the brief's "as the local external path does today" described a path that did not exist; it exists now, in one place, used by both the local and the node side.
9. **A node's `harness` must be one of its own `agents.yaml` entries**; a name that is a node of the node is refused rather than chained.
10. **Commit split**: commit 1 carries commit 2's `acp::peer` wire type and `Update::Peer` (same files); commit 1 does not build standalone (agent/rpc land in commit 2). Head builds.
11. **`session()`'s `acp` tuple** grew a third element (`Option<Risk>`) instead of a struct — one caller passes `Some`.

## Not built
- A `Relay`-level test for `PeerUplink` → notification (needs a live `ConnectionTo`; the wire type and the supervisor side are unit-tested).
- An end-to-end `AcpChild` test receiving `_theuth/peer_message` (needs a running `Agents` actor in the registry).
- No desktop UI changes for `PeerKind::Parent` / new `CreateSessionOptions` fields (out of my allow-list; TS types regenerate from `ts(export)`).

## Tests written (not run, per the rules)
- `acp::meta::tests::{round_trip, empty_is_absent, malformed_is_an_error}`
- `acp::peer::tests::{round_trip, other_methods_and_bad_bodies_are_none}`
- `agent::peers::tests::spawn_tools_arg_parses_to_a_cap_or_a_clear_error` (updated), `agent_parses_as_optional_harness_at_engine` (new), `spawn_description_and_unknown_list_nodes` (updated line)
- `agent::supervisor::tests::remote_root_gets_cap_and_effort` (new)
- `rpc::acp::tests::session_new_meta_becomes_create_options` (new)

## `cargo check`
- `cargo check -p nodes --tests` fails before my change: `crates/nodes/src/client.rs:123` `include_str!("../../../bindings/methods.json")` — generated file absent in this worktree. `cargo check -p nodes` (lib) passes.
- Commits 1+2: `cargo check -p nodes` and `cargo check -p acp -p agent -p rpc -p tui --tests` → `Finished dev profile` (clean, no warnings shown).
- Commit 3: `cargo check -p acp -p agent -p rpc -p theuth --tests` → `Finished dev profile` (clean; one earlier iteration failed on a `'static` borrow in `harness.rs`, fixed before this result)

DONE N
