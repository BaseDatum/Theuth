# 199 — as-built notes

## P0 CONTRACT (199-C)

Built on `feat/199-build` from main `ce19b614c`, against the design at docs commit `4d9343b5a` (FINAL; landed on main as `852d43f1e`). No behaviour change: every new path is unreachable or answers a typed "not available yet".

### What landed
- **Types** (`crates/agent/src/communication/files.rs`): `FileDirection`, `InState` (`Queued, Receiving, Publishing, Saved, Received, Failed`), `OutState` (`Queued, Sending, Sent, Failed`), `FileState` (`In(InState) | Out(OutState)`, tagged by direction), `NoticeState` (`Queued, Sent, Delivered`), `ContactFile`, `ContactFileThumbnail`, `NoticeAdmission` (`accepted`), `NoticeStatus` (`committed | unknown`). Constants `FILE_MAX` (5,000,000,000), `OFFER_TTL` (7 d), `FILE_IDLE` (60 s), `JOURNAL_SOURCE` (`contact_file`), the `files/…` directories, `NOT_YET`, `SEND_NOT_YET`. `notice_id(holder, id)` (UUIDv5), `received_dir`.
- **`PeerRecord.files`, `PeerDelivered.files`** (`#[serde(default)]`, empty today; `PeerDelivered.files` is the record's, so it fills itself once `Store::exchange_record` does).
- **`AgentEvent::ContactFileChanged { session, file: ContactFile }`** on `agent.events`.
- **`SendMessageArgs.files: Vec<PathBuf>`** (`#[serde(default)]`); a non-empty list answers `sending files is not available yet` before anything resolves or sends (`peers.rs`, `SendMessageTool::call`).
- **RPCs** on `AgentMessagingApi` (`crates/agent/src/rpc_messaging.rs`): `contact_file(holder, id) -> ContactFile`, `contact_file_thumbnail(holder, id) -> ContactFileThumbnail`, `contact_file_export(holder, id, dest) -> PathBuf`, `contact_file_fetch(session, holder, id) -> PathBuf`, `contact_file_notice(session, notice_id, contact_id, notice) -> NoticeAdmission`, `contact_file_notice_status(session, notice_id) -> NoticeStatus`. Each answers `ASK_FAILED` (-32000) `contact files are not available yet`.
- **Migration `crates/agent/migrations/0067_contact_files.sql`**, exactly the design's table and three indexes (the outbox index covers `queued` and `sent`). Node-local: not in `capture::ATTACHED`; `SESSIONS_WIRE_SCHEMA` stays 67. 0067 was free here and in every `theuth-*` worktree at commit time.
- **Store**: `Store::record_error_once(session, id, text)` (`record_error` now delegates to it with a fresh v7 id; `ON CONFLICT(id) DO NOTHING` never fires for those). `Store::notice_committed(session, notice_id)` and `store::durability::notice_committed_in(conn, …)`: the one `committed` predicate (journal `input_accepted` step with that `input_id`, or an `errors` row with that id, same session).
- **Sanitiser** `files::sanitise`, collision names `files::candidate`.
- **What the model sees**: `files::placeholders(from, files)`, `files::notice(from, file)`, `files::ended_notice(from, file)`, every attribute through `loop_::attr` (now `pub(crate)`).
- **Disk** (`crates/agent/src/communication/files/disk.rs`, `#[cfg(unix)]`): `egress_open(path, root)`, `egress_reopen(path, root, dev, inode, size)`, `open_checked(path, &set)`, `protected_set(&[roots])`, `EgressError`/`Refusal` (its `Display` is the design's `files:` texts); `blake3_of(&File, len)` (pread, never past `len`); `quarantine_value`, `quarantine(&File, value)` (set, read back, compare; no-op off macOS); `publish(partial, dir, name, value, record)` (steps 3–5), `finish`, `abandon`, `resolve -> Restart { Finish, Republish, Requeue }`, `inode_of`.
- **A2A** (`crates/a2a/src/contacts.rs`): `EXT_FILES` beside `EXT_FULL`, not advertised. `crates/a2a/src/contacts/files.rs`: `FILES_PATH`, `RECEIPT_MAX`, `REASON_MAX`, `is_file_id`, `file_id`, `file_url`, `parse_file_url`, `FileMeta` (+ `check`), `check_part_content` (exactly one of `text|raw|url|data` per part, on raw JSON), `Receipt` + `parse_receipt`, `Gone`, `parse_range`, `parse_content_range`, `Serve` (200/206/416 and headers), `Next` (the receiver's request), `check_answer -> Verdict { Stream, Gone, Changed }`.
- **P2**: `artifact::mesh::Req::GetContactFile { id }`, answered `Missing`.
- **D14**: `read_file` reads through `tools::fs::read_bounded` (stat, refuse above `READ_MAX` = 268,435,456, then `take(READ_MAX)`).
- **Deps**: `blake3 = "1.8.7"` in the workspace and in `agent` (the lock already had 1.8.7 via iroh); `libc` in `agent`; `chrono` in `a2a`.

### Seams
- **Artifact → held file**: the kameo message `agent::communication::files::HeldContactFile { id } -> Option<HeldFile { path, dev, inode, size, blake3 }>` on `Agents` (handler in `files.rs`, answers `None`). `artifact` already depends on `agent` and asks `Agents` elsewhere (`artifact/src/search.rs`, `store.rs`, `tools.rs`), so a message is the existing pattern and there is no cycle to break; no trait or callback registry was added.
- **Publication ↔ store**: `disk::publish` takes an async `record(target, dev, inode)` callback (`AsyncFnMut`), so E1 persists `state='publishing'`, the target and the partial's inode in its own write before each `hard_link`. Step 6 (the commit) stays E1's; `finish` drops the partial after it.
- **Commit-time dedup**: `notice_committed_in(&mut conn, …)` takes the write transaction's connection.

### a2a ↔ agent seams (E1a ↔ E1b, commit "199-C: E1 split seams")
`crates/a2a` depends on `crates/agent`, never the reverse, so every trait both builders share lives in `agent::communication::files` (a2a re-exports `Gone`, `Receipt`, `REASON_MAX` from `a2a::contacts::files`). Each side registers its implementation in a process-wide slot; an unregistered slot answers a no-op.
- **Sender side, E1b implements, E1a calls** (`a2a` route → agent):
  ```rust
  #[async_trait] pub trait FileOffers: Send + Sync {
      async fn open(&self, contact: &str, id: &str) -> Result<OfferHandle, OfferError>;
      async fn receipt(&self, contact: &str, id: &str, receipt: Receipt) -> Result<(), OfferError>;
  }
  pub struct OfferHandle { pub file: std::fs::File, pub len: u64, pub blake3: String, pub deadline: DateTime<Utc> }
  pub enum OfferError { Unknown /* 404 */, Gone(Gone) /* 410 expired|missing|changed */ }
  pub fn set_offers(Arc<dyn FileOffers>);  pub fn offers() -> Arc<dyn FileOffers>;  // default NoOffers: 404
  ```
  `contact` is the caller's immutable record id (`ContactFacts.id`), `id` the sender's local id from the path. The `(dev, inode, size)` re-check (`disk::egress_reopen`) happens inside `open`. Registered at agent start (`supervisor::start`, now `NoOffers`, marked `199-E1b`).
- **Receiver side, E1a implements, E1b calls** (agent queue → a2a client):
  ```rust
  #[async_trait] pub trait FileFetch: Send + Sync {
      async fn download(&self, fetch: &Fetch, progress: &mut (dyn FnMut(u64) + Send)) -> Result<Fetched, TransferError>;
      async fn send_receipt(&self, contact: &str, member: &str, id: &str, receipt: &Receipt, timer: Timer) -> Result<(), TransferError>;
  }
  pub struct Fetch { pub contact: String, pub member: String, pub id: String, pub size: u64, pub partial: PathBuf, pub have: u64, pub timer: Timer }
  pub struct Fetched { pub bytes: u64, pub blake3: String }  // the whole partial; the caller verifies
  pub enum TransferError { Unknown, Gone(Gone), Changed, Idle, Deadline, Cancelled, Transport(String), Disk(io::Error), NotYet }
  pub struct Timer { pub deadline: tokio::time::Instant, pub idle: Duration }  // Timer::until(d) = FILE_IDLE; at(last) -> (Instant, Expiry::{Idle, Deadline})
  pub fn set_fetch(Arc<dyn FileFetch>);  pub fn fetch() -> Arc<dyn FileFetch>;  // default NoFetch: NotYet
  ```
  a2a's implementation is `a2a::contacts::files::ContactFetch(Weak<Ctx>)`, registered in `Ctx::attach` beside `reach::Gate`; it calls the free functions `download(net, cut, fetch, progress)` and `send_receipt(net, cut, member, id, receipt, timer)` (both `NotYet`, marked `199-E1a`). `Changed` covers every Range or length mismatch and a `416` (`check_answer`); `Idle` requeues and resumes, `Deadline` fails the file; `progress` gets the partial's size.
- **Cancellation**: `a2a::contacts::files::Transfers` (field `Ctx.transfers`): `token(contact) -> CancellationToken` for every route GET/receipt and every download/receipt POST of that record id; `cut(contact)` cancels them all and the next transfer starts with a fresh token. `ContactFetch` takes the token itself; the cut goes in `Ctx::pass` right after the live snapshot swap (marker `199-E1a`), for every contact whose members or block state changed.

### Deviations from the design (and the brief)
1. **Type names follow the design, not the brief's list.** The brief named `FileState, FileDirection, IntakeState`; the design (rounds 3–5) has `InState`, `OutState`, `FileDirection`, `NoticeState`, and `FileState` = "`InState` or `OutState`, tagged by `direction`". `IntakeState` went with the held intake (round 3). The brief's `contact_files` RPC is the design's `contact_file`.
2. **`ContactFile.direction` and `.state` are one flattened field** `state: FileState` (`#[serde(flatten)]`, adjacently tagged): the wire still carries `"direction": "in", "state": "saved"`, and an `out` state under `in` cannot be expressed. `FileState::direction()` reads the tag.
3. **Sizes in error texts are exact bytes**: `files: <path> is 5000000001 bytes (over the 5 GB limit per file)`, `read_file: <path> is 300000000 bytes; too large to read whole (limit 256 MB). …` (the design leaves `<size>` unformatted).
4. **Gate refusals**: a socket does not open (`ENXIO` on Linux, `EOPNOTSUPP` on macOS); a failed open on an existing non-regular file reports `not a regular file`. `Refusal::Changed` (`files: <path> changed since it was offered`) is the reopen's mismatch; the route turns it into `410 changed`.
5. **`FileMeta` ignores unknown keys** (a later v1 field does not refuse a message); the receipt body is strict (anything else is a 400, as the design says).
6. **`ContactFileChanged` is an `AgentEvent` variant** as the design says, so the exhaustive matches in `crates/slack/src/channel.rs` and `crates/tui/src/model/agents.rs` gained an arm (slack ignores it; the TUI arm is `199-UIT`).
7. **The quarantine timestamp is 8 hex digits** (`{:08x}`), as Safari and curl write it.
8. **`notice_id`** is `Uuid::new_v5(NAMESPACE_OID, "theuth/contact-file/<holder>/<id>")`.
9. **`Store::notice_committed`** is frozen here (round 5's shared predicate), with a test; nothing calls it yet.
10. **ProtoJSON makes `size` a double.** Part metadata is a `google.protobuf.Struct`, so a `FileMeta.size` of `2516582` comes back from `a2a_pb::protojson_conv` as `2516582.0` (found by the round-trip test). `FileMeta.size` therefore deserialises a `u64` or a non-negative whole double up to 2^53 (exact; `FILE_MAX` is far below); a fraction or a negative is refused. Senders still write an integer.
11. **The receipt parser is a flat struct** with `deny_unknown_fields`, not the tagged enum: serde's internally tagged enums let unknown keys through on unit variants (`{"state":"received","x":1}` parsed). `Receipt` itself only serialises.

### Ownership map
New modules belong to one builder; shared hot files list the exact functions each may touch. Every stub carries a `// 199-<owner>:` marker.

| Owner | Owns outright | May touch in shared files |
|---|---|---|
| **E1a** (P1, `crates/a2a`) | the file route (GET/POST, `Serve`, `410` texts, 60 s-free timers) and the receiver's `download` / `send_receipt` in `crates/a2a/src/contacts/files.rs`, `ContactFetch`, `Transfers` | `a2a/src/contacts/reach.rs` `serve` (route match, serving through `offers()` under `transfers.token`); `a2a/src/lib.rs` the reconcile after the snapshot swap (`transfers.cut`); `a2a/src/server.rs` `validate_send` and the body layer (`check_part_content`); `a2a/src/client.rs` message construction (reference parts, `extensions`); `a2a/src/contacts.rs` card builder (`EXT_FILES` advertisement) |
| **E1b** (P1, `crates/agent`) | new `crates/agent/src/store/contact_files.rs` (queue rows, transitions, outbox), new modules under `crates/agent/src/communication/files/` (send path and offers, the `FileOffers` impl, the receive queue calling `fetch().download`, publication, notices) | `agent/src/peers.rs` `SendMessageTool::call` (the `files` stub) and `description`; `agent/src/supervisor.rs` `start` (`set_offers`); `agent/src/rpc_messaging.rs` `contact_file`, `contact_file_thumbnail`, `contact_file_export`, `contact_file_notice`, `contact_file_notice_status`, `answer_peer_request` (D15 `files`); `agent/src/store/communication.rs` `TryFrom<ExchangeRow> for PeerRecord` / `Store::exchange_record` (`files`); `agent/src/loop_.rs` `peer_wrapper` (placeholders) and the `append_input` call (input id, `JOURNAL_SOURCE`); `agent/src/store/durability.rs` `append_input` (commit-time check via `notice_committed_in`); `agent/src/communication/network/full.rs` `deliver_full`, `full_return`; `agent/src/supervisor/pause.rs` startup (resolve `publishing`, clear `files/partial/`, fail interrupted files) |
| **E2** (P2) | `files/cache/`; the fetch function in `crates/artifact/src/mesh.rs` (new, beside `fetch_attachment`) | `artifact/src/mesh.rs` `answer` (`Req::GetContactFile` arm); `agent/src/communication/files.rs` `impl Message<HeldContactFile> for Agents`; `agent/src/rpc_messaging.rs` `contact_file_fetch` |
| **W** | `crates/web/src/fetch/download.rs` and its `mod`/re-export line | — |
| **E3** (P3) | a new `crates/agent/src/communication/files/foreign.rs` (calls `web::fetch::download`) | `a2a/src/server.rs` `validate_send` (`raw`/`https` admission); `a2a/src/client.rs` `Retained::of`, the stream-update path, inline `raw` sends, `accepted_output_modes` |
| **UID** (desktop) | a new file-card component | `ui/src/store/reduce.ts` `peerBody`, the `PeerDelivered` case, a new `ContactFileChanged` case; `ui/src/lib/injected.ts` `classify`; `ui/src/components/chat/SendCard.tsx` `SendCard`; the error renderer (ended-share rows) |
| **UIT** (TUI) | new file-card rendering | `tui/src/components/peers.rs` `contact_row`; `tui/src/model/agents.rs` the `ContactFileChanged` arm; the error renderer |

E1a and E1b meet only at the seams above: a change to a seam signature is a contract change for both. E1a and E3 both edit `validate_send` and the message construction in `a2a/src/client.rs`: E3 builds on E1a's version of those two functions. UID and UIT read `ContactFile` from the generated bindings; `files` on `PeerDelivered` is always present (possibly empty).

### Gates (P0)
Logs in `/tmp/199c/`. Crate-level `-p` only, through `build-lock` (label `199-c`).
- **check** `mbx check -p agent -p a2a -p artifact -p tools -p slack -p tui -p schedule --tests`: 0 errors, 0 warnings (3 m 05 s cold, 24 s warm).
- **clippy** `--no-deps --tests -- -D warnings -A clippy::chunks_exact_to_as_chunks -A deprecated` on `agent a2a artifact tools slack schedule`: clean (56 s). `tui` with `-D warnings -A clippy::large_enum_variant`: clean (98 s). Known red on main, untouched: `crates/tui/src/components/mcp.rs:24` `Request` (`clippy::large_enum_variant`).
- **tests** `mbx test -p agent -p a2a -p tools --no-run` (96 s), then bare with the same set (13 s): 23 new tests (agent 15: `communication::files` 4, `communication::files::disk` 8, `store` 2, `store::durability` 1; a2a `contacts::files` 6; tools `fs` 2) plus their neighbours (`peers::`, `store::communication`, `peer_wrapper`, `server::tests`, `fs::tests`, `sessions_wire_schema_tracks_the_replicated_columns`): a2a 16, agent 41, tools 23 passed, 0 failed. The quarantine read-back ran for real on macOS (APFS, case-insensitive).
- **bindings** `BUILD_SLOTS=1 … mbx test -p theuth --bin t --quiet export_bindings` (4 m 54 s). **Classifier** `mbx test -p theuth --test wire_schema_snapshot` before acceptance: 14 changes, all `Additive`, **Breaking 0** (six new methods; `files` on `peer_message`/`list_peer_messages` results and on `agent.peer-delivered`; the `ContactFileChanged` variant of `agent.events`); after `cp bindings/openrpc.json crates/theuth/tests/fixtures/wire/openrpc-v1.json`: 1 passed.
- **desktop** `bun run wire:generate` (587 methods, 58 topics) and the copy; `bunx tsc -b --noEmit`: 0 errors (as main). Five `PeerDelivered` literals in `reduce.test.ts` / `QueuedChips.test.ts` gained `files: []`; `vitest run` on those two files: 102 passed.
- **fmt**: `rustfmt --edition 2024` on the new files and on `tools/src/fs.rs`, `agent/src/store/durability.rs` (both clean on main); hunks in files main leaves unformatted (`agent/src/lib.rs`, `rpc_messaging.rs`, `a2a/src/contacts.rs`, `schedule/src/lib.rs`, the TUI files) were formatted by hand, not by a whole-file `cargo fmt`.

### Regenerating bindings (any builder)
1. `BUILD_SLOTS=1 BUILD_LABEL=<label> ~/.theuth/bin/build-lock mbx test -p theuth --bin t --quiet export_bindings` (writes `bindings/`).
2. The wire-contract classifier, `crates/theuth/tests/wire_schema_snapshot.rs` (`rpc_core::compatibility::classify`): `BUILD_LABEL=<label> ~/.theuth/bin/build-lock mbx test -p theuth --test wire_schema_snapshot --no-run`, then `mbx test -p theuth --test wire_schema_snapshot` bare. Its report lists every change with its class; accept only with no breaking change: `cp bindings/openrpc.json crates/theuth/tests/fixtures/wire/openrpc-v1.json`, then rerun it green.
3. Desktop: `cd crates/desktop/ui && build-lock bun install --frozen-lockfile && build-lock bun run wire:generate`, then `rm -rf src/rpc/bindings && mkdir -p src/rpc/bindings && cp -R ../../../bindings/typescript/. src/rpc/bindings/ && cp ../../../bindings/surface-catalog* ../../../bindings/methods.json src/rpc/bindings/`, and `build-lock bunx tsc -b --noEmit`. In a fresh tree `packages/apps` needs its own `build-lock bun install --frozen-lockfile` first (else 164 `@openuidev/*` errors). Both binding trees are gitignored; only the OpenRPC fixture is committed.
