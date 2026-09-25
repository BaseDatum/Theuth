# 68 as built — worker F (front door: take-on-write, forwarding, remote events, artifacts over the mesh)

Branch `feature/68-sessions-f`, worktree `theuth-68-f`. Design: `68-sessions.md` v0.3 §5, §7 (ruling 5 hybrid); contract `68-contract.md` §4–§6.

## Commits

### 1. `c67fc95` feat(sessionsync): remote session events over `theuth/sessions/1`; `ALPN_RPC`/`ALPN_ARTIFACTS` registered
- **`crates/sessionsync/src/events.rs`** (new). One actor per process, `EventRelay`, registered `"sessions.events"` (`EVENTS_ACTOR`), spawned on the subscriber mailbox and subscribed to `AgentEvent`.
  - Owner side: `serve(relay, framed, session, from_seq)` is called for an accepted `theuth/sessions/1` stream whose first frame is `Msg::Events{session, from_seq}`; it registers `Serve{session, from_seq, tx}` on the actor and writes every `Msg::Event{session, seq, bytes}` the actor hands it until the peer closes the stream or the actor drops the sender. The actor serves a session only while `store.owner(session)` is this engine (a stale request just closes); on `SessionOwnerChanged{here: false}` it drops every stream of that session. `seq` is the owner's per-session, per-process counter; `bytes` is `serde_json::to_vec(&AgentEvent)` (the same JSON the RPC bridge emits). Slow subscribers (256 queued) are dropped, not blocked.
  - Subscriber side: `follow(session) -> Option<Following>` refcounts local interest; the guard's `Drop` is the `Unfollow`. The first follower spawns `follower()`: resolves the owner from the replica row (`store.owner`), opens the stream, sends `Events{from_seq: last+1}`, decodes each `Event` back to `AgentEvent`, recomputes `SessionOwnerChanged.here` for this engine and `rt.publish`es it, so `subscribe_events` filters work unchanged. On stream end: backoff 1→60 s, woken early by `PeerEvent::Connected(owner)`; a finished follower (owner was here, or the row moved) is restarted on `PeerConnected` (`Reconnected`) and on `SessionOwnerChanged{here:false}`; `here:true` aborts it. Loop guard: re-published events reach this engine's own relay, which has no served subscribers for a session it does not own.
  - `framed(stream)` = `LengthDelimitedCodec` with `types::MAX_FRAME`, shared with L's dispatcher.
- `crates/sessionsync/src/lib.rs` (C/L's file; disclosed hooks): `pub mod events`; the plugin starts `EventRelay` before `SessionSync`; `start_transport` hands the transport to the relay (`SetTransport`); `PeerConnected` tells the relay `Reconnected`; **`SessionSync::serve` now reads the first frame and dispatches `Msg::Events` to `events::serve`**, everything else logs "shipping not built yet" (L replaces that arm; the `Events` arm must stay).
- `crates/sessionsync/Cargo.toml`: `serde_json` (regular); dev: `mesh`, `figment`, `parking_lot`, `tempfile`.
- `crates/replica/src/lib.rs`: `ALPN_RPC = "theuth/rpc/1"`, `ALPN_ARTIFACTS = "theuth/artifacts/1"` beside the other five (the crate every ALPN user already shares; rpc/artifact cannot be named from mesh without a cycle).
- `crates/mesh/src/net.rs`: `STREAM_ALPNS` 4 → 6 (both new ALPNs accepted through `Transport::accept`).

### 2. `668ed98` feat(artifact): blobs, spill files and records over `theuth/artifacts/1`
- **`crates/artifact/src/mesh.rs`** (new). Wire: postcard in `LengthDelimitedCodec` frames (`MAX_FRAME` 64 MiB); one `Req` per stream, then `Reply` frames.
  - `Req::Get{hash}` — a bare file name under `<data_dir>/artifacts/` (`<sha16>.<ext>`, `<sha16>.preview.png`, `<sha16>.md`); path separators / `..` refused. `Req::GetSpill{session, call_id}` — `<data_dir>/spill/<session>/<sanitize(call_id)>.txt`. `Req::Record{call_id}` — the `Artifact` row as JSON (★ added: the artifacts index is engine-local, so a replica cannot even name the blob without it).
  - `Reply::Missing` | `Reply::Chunk(bytes)`… `Reply::End` (files stream in 4 MiB chunks) | `Reply::Record(json)`.
  - `serve(rt, stores)` waits for the mesh transport then accepts until shutdown; `answer()` serves from the durable store (`stores.durable().dir`, `State::get`) and the data dir — any engine serves what it has.
  - Client: `fetch_blob(dir, name) -> PathBuf`, `fetch_spill(data_dir, session, call_id) -> PathBuf`, `fetch_record(data_dir, call_id) -> Artifact` (paths rewritten to this engine's `artifacts/`). Each asks every connected peer in turn (60 s per peer), first found wins; files land in a `.part-<pid>` temp beside the destination and are renamed into place (a fresh temp per peer attempt, so a peer failing mid-stream never pollutes the next). Existing local file → returned without a fetch. No transport / no peer has it → `Err("unavailable: owner offline")`.
  - `spill_path(data_dir, session, call_id)` mirrors `agent::spill::cap`'s naming.
- `crates/artifact/src/lib.rs`: `pub mod mesh`; the plugin spawns `mesh::serve` (one line). `crates/artifact/Cargo.toml`: `replica`, `tokio-util` (codec), `futures`, `bytes`, `postcard`.
- `crates/agent/src/spill.rs`: `sanitize` made `pub` (one word) so the spill path is computed once, not copied.

### 3. `f952e8f` feat(rpc): front door
- **`crates/rpc/src/forward.rs`** (new).
  - `classify(store, session) -> Route`: `Here` when the row is unknown, `owner` empty or `owner == store.engine()`; else `Take` when `sessions.agent` is `NULL` (user session), `Forward(owner)` when set (agent desk).
  - `route(ext, session, method, params) -> RpcResult<Option<T>>`: the one call every mutating RPC makes after `authorize`. `Ok(None)` = proceed locally (owner here, sessions plugin absent, request already forwarded once, or the take succeeded); `Ok(Some(reply))` = the owner's answer; `Err` = the owner's JSON-RPC error verbatim or `OWNER_UNREACHABLE`. `Take` asks `SessionSync::TakeSession{reason: Write}`; any `Err(why)` from it → `OwnerUnreachable`. `Forward` requires the owner in `transport.peers()`; otherwise `OwnerUnreachable("not connected")`.
  - `forward(transport, owner, method, params)`: one `theuth/rpc/1` stream per call, one JSON-RPC request line (`Id::Number(1)`, positional params in trait order), one response line, `TIMEOUT` 30 s; transport failure / timeout / malformed reply → `Err(Ok(why))` (becomes `OwnerUnreachable`), a JSON-RPC error → `Err(Err(e))` (passed through).
  - `serve(rt, transport, methods)`: the owner's accept loop; each request is dispatched with `Methods::raw_json_request` as `link::RpcPeer::internal()` plus the `Forwarded` extension marker — `route` sees the marker and never forwards a second hop (if the owner moved meanwhile, the store's `NotOwner` guard answers). Started from `acp::serve_mesh` beside the ACP accept loop (`crate::methods(rt)` = the full API fed by the bridge).
  - Typed error (per Sky's mid-run addition, matching U's parser): code `OWNER_UNREACHABLE = -32010`, message `owner <id8> unreachable: <why>`, `data` = `FrontDoorError` in serde's **external enum form**: `{"OwnerUnreachable": {"owner": "<engine id>", "last_seen": <ms epoch | null>}}` (`last_seen` from mesh `Peer::last_seen_ms`; `null` when the mesh is off or never saw the owner). `ts` export for U.
- `crates/rpc/src/api.rs` (disclosed; the brief's step 1 cannot be done elsewhere). Each session-mutating method gained one guarded call right after `authorize`:
  `if let Some(r) = forward::route(ext, session, "<method>", (<params in trait order>)).await? { return Ok(r); }`.
  **Enumerated (24):** `send_input`, `steer`, `withdraw`, `cancel`, `compact`, `undo`, `redo`, `remove_context`, `regenerate`, `edit_turn`, `rename_session`, `pin_session`, `archive_session`, `delete_session`, `set_session_project`, `set_model` (when `session: Some`), `set_effort` (`Some`), `set_mcp_hidden`, `set_approvals` (`Some`), `set_communication` (`Some`), `set_peer_scope`, `answer_peer_request`, `use_skill`, `knowledge_harvest`. `send_input`/`use_skill` with `session: None` resolve the owner root first (always here).
  Also: `subscribe_events` holds a `sessionsync::events::follow(session)` guard for the life of the subscription task (remote owner's events flow onto this bus while a client watches); `get_artifact` falls back to `artifact::mesh::fetch_record` when the local actor has no record; `get_attachment` fetches `…/artifacts/<name>` from a peer into `<root>/artifacts/` when the path does not exist locally (matched on the parent directory name, since the recorded path is the owner's absolute one), then serves it under the usual allow-list.
- `crates/rpc/src/acp.rs`: `serve_mesh` spawns `forward::serve`. `crates/rpc/src/lib.rs`: `pub mod forward`. `crates/rpc/Cargo.toml`: dev `tempfile`.

### 4. this commit — `FrontDoorError` external-enum error data (Sky's mid-run addition), `EventRelay::spawn_on` split, `docs/design/68-as-built-F.md`, `Cargo.lock`.

## Deviations / decisions
1. ★ **`api.rs` is edited** (not in F's allow-list): 24 one-line `route` hooks, the `follow` guard, two artifact fallbacks. Unavoidable for step 1; every hook is the same shape and sits right after `authorize`.
2. ★ **Framing on `theuth/rpc/1` is newline-delimited JSON-RPC**, one request per stream, not a `{method, params}` struct in a length-delimited frame: the request *is* jsonrpsee's `Request`, the owner dispatches it with `Methods::raw_json_request`, and the response line is jsonrpsee's `Response` — zero re-encoding on either side and the owner's errors pass through byte-for-byte.
3. ★ **One-hop bound** via the `Forwarded` extension marker instead of a TTL field.
4. ★ Any `Err` from `TakeSession` maps to `OwnerUnreachable` (O's error vocabulary — `OwnerBusy` | `OwnerUnreachable` — does not exist yet; the string is carried in the message). When O lands, `route` should pass genuine non-reachability errors (e.g. `NotOwner` races) through as plain errors; one match arm.
5. `last_seen` is ms since epoch or `null`, not a timestamp string.
6. **`sessions.agent`** is the ACP external-harness key (`Store::set_acp`), which is what the design names as the desk/user discriminator. A roster desk running a native theuth session therefore classifies as a *user* session (take-on-write). Followed the contract literally; flag for the design owner.
7. ★ `Req::Record{call_id}` added to the artifacts ALPN (see commit 2). `fetch_record` is not cached: the index is engine-local by decision 8.
8. **Spill-marker expansion**: there is no expansion in `load_session` today — the marker names an absolute path and the model's `read_file` is the reader. `fetch_spill`/`GetSpill` are built and tested, but no caller reads spill through them yet: wiring belongs in `tools`' `read_file` (not F's) or in a future marker rewrite. `spill::sweep` unchanged.
9. `put_session_attachment` is **not** routed: it writes a file locally and returns a path; a later forwarded `send_input{files}` would name a path the owner does not have. Known gap, same class as item 8 (attachments over the mesh).
10. Not routed, deliberately: `broadcast` (multi-session, already two-phase), `fork_session` (creates a new local session from the replica; the source is untouched), `publish_app`, PTY/job calls (bound to the live actor's process; a desk's jobs would need forwarding of streams, out of scope), `resolve_approval`/`answer_ask` (id-keyed, no session), `create_session`/`new_session` (always here).
11. `Msg::Events.from_seq` is honoured as a filter only; no owner-side replay buffer (the log carries history; the stream is for liveness).
12. `SessionSync::serve` (L's) gained the first-frame dispatch; L's protocol must keep the `Msg::Events` arm or call `events::serve` from its own dispatcher.

## Not built
- Attachments (`<data_dir>/attachments`) over the mesh; forwarded `send_input{files}` paths (item 9).
- `read_file` fallback to `fetch_spill` (item 8).
- Owner-first peer ordering for artifact fetches (all connected peers are tried; the owner is among them).
- Desktop card for `OWNER_UNREACHABLE` (U's; the error code and the `FrontDoorError` type/shape are exported).

## Tests written (all `#[cfg(test)]`, loopback `Transport` as `replica/tests/sync_loopback.rs`)
- `rpc::forward::tests`: `classify_user_vs_desk` (rows written under engine "them", read as "me": own → `Here`, unknown → `Here`, user → `Take`, `agent` set → `Forward("them")`); `forwarded_send_input_reaches_the_owner_dispatch_as_internal_forwarded_peer` (fake owner `RpcModule` sees the text, the `Forwarded` marker and the internal `RpcPeer`); `owner_errors_pass_through_verbatim`; `a_silent_owner_is_unreachable_after_the_timeout` (paused clock, 30 s); `owner_unreachable_error_is_the_external_enum_form` (code -32010, `data == {"OwnerUnreachable": {"owner": "engine-a", "last_seen": null}}`).
- `sessionsync::events::tests::events_are_relayed_and_unsubscribed`: two stores (owner / front with the row owned by "owner"), two relays over a loopback pair, owner accept loop dispatching `Events` to `serve`; a `TitleChanged` published on the owner's bus arrives on the front's bus; dropping the guard stops delivery.
- `artifact::mesh::tests::blob_and_spill_are_fetched_from_a_peer_and_a_missing_one_is_reported`: a 4 MiB+17 B blob (two chunks) and a spill file fetched from the owner; `Missing` for an unknown name; `../artifacts.sqlite` refused; `fetch_blob` without a transport → "unavailable: owner offline".

## Verification (exact results)
- `cargo check -p sessionsync --tests`: clean (commit 1).
- `cargo check -p artifact --tests`: clean after removing one unused import (commit 2).
- `cargo check -p rpc --tests`: `Finished dev profile [unoptimized + debuginfo] target(s) in 31.95s`, no warnings (commit 3).
- Tests run (item 5 of the brief allows `cargo test -p rpc -p artifact -p sessionsync --lib`; filtered to F's modules):
  - `cargo test -p rpc --lib forward::` → `test result: ok. 5 passed; 0 failed`.
  - `cargo test -p artifact --lib mesh::` → `test result: ok. 1 passed; 0 failed`.
  - `cargo test -p sessionsync --lib events::` → first run failed on `name already registered` (two relays in one process); fixed by splitting `EventRelay::spawn_on` (unregistered) from `start`; rerun → `test result: ok. 1 passed; 0 failed`.
- Commit 4 (this doc, `Cargo.lock`, the error-shape change, the `spawn_on` split) was not re-`cargo check`ed separately: the three test runs above compiled every crate in its final state.
- No TypeScript touched; `bunx tsc` not run.

DONE F
