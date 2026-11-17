# Contact-files recon — origin/main `46bf524c528c7aba1be42495964ae3cc6b30b1f9` — 2026-11-17

Read-only; no builds/tests or checkout/worktree modifications. Recon began at `0323c5f3f`; main advanced during the read to `46bf524c5`. The intervening changes are Pocket TTS/overview audio, not the contact paths below; Cargo dependency line offsets were refreshed.

Scope follows the revised **design 199** brief: arbitrary file types up to 5 GB, stored bytes plus a model-facing placeholder, one file-egress gate, and automatic **public HTTPS** fetching in P3. Citation drift below covers the retained transport/store/rendering facts, not the superseded quota, normalisation, scoped-document-reader or manifest-authority proposals.

## 1. Cite drift

Aliases retained from the draft: **A** = `crates/a2a/src`; **N** = `crates/agent/src/communication/network.rs`; **F** = `crates/agent/src/communication/network/full.rs`; **S** = `crates/agent/src/store.rs`; **I** = `crates/tools/src/images.rs`; **B** = `crates/artifact/src/mesh.rs`; **UI** = `crates/desktop/ui/src`. **SDK** = `/Users/Sky.Moore/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/a2a-lf-0.4.2/src/types.rs`.

“Holds” means the cited fact remains supported at that range, not byte-for-byte source identity.

| Draft citation | Status / current site |
|---|---|
| `crates/agent/src/peers.rs:296-312` | holds |
| `crates/agent/src/peers.rs:416-420` | holds |
| `crates/agent/src/peers.rs:301-302` | holds |
| `crates/agent/src/communication.rs:80` | holds |
| `A/client.rs:798-819` | holds |
| `A/server.rs:565-603` | holds |
| `A/server.rs:49-50` | holds |
| `A/client.rs:971-981` | holds |
| `A/client.rs:50-62` | holds |
| `A/client.rs:943-957` | holds |
| `A/client.rs:982-1003` | holds |
| `Cargo.toml:378-384` | **moved** — `Cargo.toml:380-387`; `a2a-types` pin is now line 385. |
| `A/server.rs:535-546` | holds |
| `SDK:199-221` | holds |
| `SDK:280-301` | holds |
| `SDK:307-323` | holds |
| `A/contacts/reach.rs:1-10` | holds |
| `A/contacts/reach.rs:30-39` | holds |
| `A/contacts/reach.rs:1-6` | holds |
| `crates/agent/src/communication/mesh.rs:127-138` | holds |
| `A/contacts/forward.rs:142-150` | holds |
| `F:1-8` | holds |
| `F:25-29` | holds |
| `F:110-119` | holds |
| `UI/components/chat/Composer.tsx:742-760` | holds |
| `UI/lib/files.ts:58-61` | holds |
| `crates/agent/src/loop_.rs:8941-8965` | **moved** — `crates/agent/src/loop_.rs:8965-8989`. |
| `crates/agent/src/loop_.rs:8968-9000` | **moved** — `crates/agent/src/loop_.rs:8992-9024`; `file_part` starts at 9000. |
| `crates/agent/src/loop_.rs:8214-8252` | **moved** — `crates/agent/src/loop_.rs:8238-8276`. |
| `crates/agent/src/store/capture.rs:1-5` | holds |
| `crates/agent/src/store/capture.rs:19-39` | holds |
| `B:1-14` | holds |
| `B:392-429` | holds |
| `B:392-417` | holds |
| `crates/mesh/src/net.rs:505-514` | holds |
| `crates/agent/src/store/contacts.rs:1-26` | holds |
| `crates/agent/migrations/0059_mesh_contacts.sql:1-6` | holds |
| `crates/agent/src/communication.rs:761-778` | **moved / citation repair** — `PeerOrigin` is at `crates/agent/src/communication.rs:774-790`; the draft range ends before the struct. |
| `crates/agent/migrations/0065_message_peer.sql:1-7` | holds |
| `S:701-715` — Today/provenance | **changed** — schema is **67**, defined at `S:723`; the additional explanation is at `S:716-722`. |
| `UI/components/chat/MessagePeer.tsx:285-359` | holds |
| `UI/components/chat/MessagePeer.tsx:375-419` | holds |
| `UI/components/chat/SendCard.tsx:110-125` | holds |
| `UI/components/chat/SendCard.tsx:176-227` | holds |
| `UI/components/chat/MessagePeer.tsx:285-298` | holds |
| `UI/components/chat/SendCard.tsx:123-125` | holds |
| `UI/components/chat/MessagePeer.tsx:227-267` | holds |
| `UI/components/chat/MessagePeer.tsx:394-419` | holds |
| `UI/components/chat/Peers.tsx:249-250` | holds |
| `UI/components/chat/Peers.tsx:351-356` | holds |
| `UI/components/chat/Peers.tsx:35-119` | holds |
| `crates/agent/src/loop_.rs:9018-9044` | **moved** — `crates/agent/src/loop_.rs:9042-9068`. |
| `crates/agent/src/loop_.rs:9112-9126` | **moved** — `crates/agent/src/loop_.rs:9136-9150`; the complete neutralisation helper ends at 9160. |
| `crates/tui/src/ui.rs:1151-1178` | holds |
| `crates/tui/src/ui.rs:1197-1204` | holds |
| `N:757` — first-contact hold lifetime | holds |
| `S:701-715` — Store/wire impact | **changed** — the “only column changes” formulation is no longer complete: `S:716-722` documents a semantic JSON-value compatibility bump without a migration. |

No retained citation’s mechanism has disappeared. Most transport/card facts are unchanged; the significant factual change is the wire-schema rule/value.

## 2. Relevant changes since the survey

The requested path-filtered `git log` was run by the parent orchestrator; irrelevant settings, popout, costs, background-strip and voice changes are omitted here.

- `aadf7a582` — isolated agent memory adds migration `0066_knowledge_identity.sql`; consumes migration 66 and adds a replicated session column (`crates/agent/migrations/0066_knowledge_identity.sql:1-2`).
- `6b3d648b5` — design 197 changes context inventory/removal semantics and raises `SESSIONS_WIRE_SCHEMA` to 67 for stored mask semantics, not a new migration (`crates/agent/src/store.rs:716-723`).
- `5973543ce` — desktop design-197 context picker/removal work: attachment controls now sit in the category/occurrence inventory world; existing `Attachments` accepts occurrence records and targets removal by occurrence (`UI/components/chat/Attachments.tsx:195-226`).
- `53fb4e5c7` — retains legacy `ContextKind::Message` for wire compatibility; builders should not replace compatibility handling with a new handwritten attachment adapter.
- `ce42b8603` — accepts design-197 and pending main changes into the OpenRPC baseline; any new attachment RPC/event fields must be generated/reviewed against this newer baseline.
- `d3ade10b4` — removes the desktop 197 adapter in favour of generated bindings; current UI event types come from generated `TopicPayloads`/`TaggedTopicPayloads` (`UI/store/ui-events.ts:1-8`).
- `d1585a441` — pins `contact_message` as a **history-only** peer-row wrapper since 191-P3; do not build new live file delivery around that legacy wrapper.
- `df879e6c9` (merged as `00c518ce7`) — sharing a chat with a contact now works from phone/iPad through capability-gated owner RPCs. New attachment affordances must not assume the controlling UI is local desktop.

**Wire refactor:** it predates `acec0d81`, rather than being post-survey drift:

- `373238ead`, `e4eccc588`, `830cdcea2` — 2026-11-12 wire registry/feed cutover and agent contract declarations.
- `2cd615bca`, `c2b092e45` — 2026-11-13 generated desktop catalogs/discovery gating.
- `b8c885c9f`, `17aed51bf` — TUI peers component and local-card/disclosure port.
- Current integration points are `crates/agent/src/lib.rs:4364-4374`, `UI/store/sessions.ts:925-937`, and `crates/tui/src/components/peers.rs:294-315`; adding a Rust event alone does not make either UI consume it.

No post-survey implementation change was found in the A2A send/validation/reach/forwarding mechanisms cited above.

## 3. Numbers a builder needs

### Migrations and session wire

- Highest migration on main: **0066**, `crates/agent/migrations/0066_knowledge_identity.sql:1-2`.
- Continuous migration numbers **0001–0066**, with no gaps, are present in main and the currently readable worktrees `theuth-glass-mac`, `theuth-glass2`, `theuth-glass3`, `theuth-ort132`, and the newly created `theuth-199-contact-files`.
- The initial `git worktree list` also contained `theuth-tts071`, with migrations through 0066. That directory was removed after its TTS work landed during recon.
- Current **`SESSIONS_WIRE_SCHEMA = 67`**, `crates/agent/src/store.rs:723`.
- **0067 is the next migration filename number, but 67 is already consumed as a wire value.** The source explicitly says the next replicated-column migration must bump past 67—**68 if its migration is 0067** (`crates/agent/src/store.rs:716-722`).

### Design numbering

- Highest tracked design number on main: **198** (`198-liquid-glass.md` and `198-liquid-glass-333.md`).
- **195 is occupied**, beginning at `docs/design/195-phone-calls.md:1`.
- The initial branch/worktree census implied no design number beyond 198; active numeric worktrees were 198, ORT 132, and TTS 071.
- **199 has now been claimed for this design**, with writer worktree `theuth-199-contact-files`. It is not a free candidate for another design.
- Free subsequent candidates in the inspected census: **200, 201, 202**. Recheck immediately before a separate claim.

Untracked design files in the main checkout at census:

```text
docs/design/195-contact-attachments.md
docs/design/195-mock.html
docs/design/58-cross-platform.md
docs/design/cross-platform/recon-android.md
docs/design/cross-platform/recon-crates.md
docs/design/cross-platform/recon-desktop-meetings.md
docs/design/scratch-always-on-recon-grok.md
docs/design/scratch-always-on-recon-theuth.md
docs/design/scratch-broadcast-recon.md
```

## 4. Current shapes

### SendMessageArgs, risk and bounds

`crates/agent/src/peers.rs:296-312`, `:416-422`; `crates/agent/src/communication.rs:78-88`.

`SendMessageArgs` has `to`, `message`, `await_reply`, `reply_to`, `request_id`; no file field. It is `Risk::Mutating`, direct, returning a dictionary. Text is bounded by `PEER_TEXT_MAX = 32 * 1024`; fan-out is at most 16 recipients (`crates/agent/src/peers.rs:282`, `:332-335`). Existing intake-count limits remain independent of any new file quota decision.

### A2A client send and advertised modes

`A/client.rs:798-822`; `A/contacts.rs:572-609`.

The client builds one `Message` containing one `Part::text`, then requests only `text/plain` output, zero history, and immediate asynchronous admission. Contact Agent Cards advertise text/plain default input/output; skills inherit those defaults. Supporting file parts requires changing both construction and capability/negotiation, not just adding paths to tool text.

### validate_send and envelope limits

`A/server.rs:565-603`, `:49-58`, `:276`, `:295`.

Validation requires a message ID and nonempty parts, rejects every non-text part, rejects requested extensions and push notifications, and requires output modes to admit text. The axum request layer caps bodies at **64 KiB**; the forward collector separately enforces the same cap before collecting.

Other byte/count bounds in `crates/a2a/src` that must not be confused with the new file ceiling:

| Boundary | Current limit and site |
|---|---|
| Server request rate/concurrency/SSE | 10 requests/s with burst 10, 8 handlers per principal, 128 SSE subscribers: `A/server.rs:49-58`, `:88-99`. |
| Ordinary fetched Agent Card | 64 KiB: `A/client.rs:42-43`, `:222-229`. |
| Remote answer accounting | 1 MiB artifact text, 2 MiB retained task JSON, 4096 retained items, 32 KiB status text, 4 KiB remote prose: `A/client.rs:50-62`. |
| SDK HTTP/SSE parsing | 2 MiB unary response; 64 KiB SSE event: `vendor/a2a-client/src/jsonrpc.rs:268-287`. |
| Contact first line/refusal head | 64 KiB per line; refusal drains at most 64 lines; refusal-log identity set resets at 1024: `A/contacts/reach.rs:30-39`, `:176-196`. |
| Contact unary response | 2 MiB before collection/JSON parse: `A/contacts/reach.rs:533-539`. |
| Owner forward | 64 KiB request; 2 MiB answer; at most two owner-resolution attempts: `A/contacts/forward.rs:44`, `:142-147`, `:181-227`, `:397-403`. |
| Contact ceremony/discovery | 3 failures/endpoint, 10/window, 64-character display text, 32 nearby sightings, 8 card fetches, 256 KiB contact card: `A/contacts.rs:46-58`. |
| LAN discovery card connections | 8: `A/contacts/discovery.rs:72`. |
| Offers | 64 offers, 64-character titles, 200-character errors, 4 KiB card-wrapper/JWS margin: `A/contacts/offers.rs:27-35`. |
| Contact alias/export names | `ALIAS_MAX` plus restricted ASCII spelling: `A/config.rs:473-483`; export display validation also caps at 64 bytes: `N:4717-4723`. |
| Roster payloads | 256 members, 8 carried versions, 16 signatures, 64 history entries/path hops, 256 KiB proof path, 192 KiB notice, 16 chains, 256 entries/chain: `A/contacts/rosters.rs:23-40`. |
| Roster maintenance counts | 3 stale/conflict refreshes; version ≤ `i64::MAX`; 2 proof sources: `A/contacts/rosters.rs:42-50`; 8 targets/round, 16 continuations: `A/contacts/members.rs:25-28`. |
| Record merge chain | 16: `A/contacts/records.rs:26-27`; roster chain-name validation additionally caps its string at 64: `A/contacts/rosters.rs:128-139`. |
| Proof-path chain query name | Nonempty, ≤80 ASCII hex/hyphen bytes: `A/contacts/members.rs:85`. |

These are current structural/identity/control-plane bounds, not proposed per-contact file quotas.

### Retained::of and answer artifacts

`A/client.rs:943-1005`, `:1009-1013`.

`Retained::of` rejects any artifact with a non-text part, checks status text, then accounts artifact text, entire serialized snapshot and item count. Items include artifacts, artifact parts, history messages and their parts—not merely file count. Both initial snapshots and streamed updates have accounting; a URL file part still needs explicit support even when its bytes are elsewhere.

### Contact reach, second stream and extra route

`A/contacts/reach.rs:148-168`, `:216-248`, `:252-287`; `crates/mesh/src/net.rs:847-870`, `:903-904`, `:2081-2105`.

Each bidirectional iroh stream carries one HTTP/1.1 exchange; HTTP keep-alive is disabled. `Net::a2a_stream` finds a live cached connection to the endpoint and calls `Connection::open_bi()`; a second call can therefore open another stream on that authenticated connection without another ALPN. Its fallback can redial if the cached connection is unavailable.

Serving accepts each new stream through `serve_a2a`, rechecking contact admission. Dispatch a new attachment GET in `reach::Route::of`/`contact_route`: presently only Card, Notice and roster Path match there; all other requests go to `forward::route`. A GET must not accidentally fall into the unary owner-forwarding path.

### Streaming and 5 GB collisions

`A/contacts/reach.rs:154`, `:239-248`, `:444-449`, `:475-539`.

The underlying `ByteStream` and Hyper response body can stream incrementally; HTTP/1.1 does not require buffering the whole response. The **existing unary client does buffer**, using `Limited(..., 2 MiB).collect()`. A new GET client must consume body frames/chunks into a file instead.

Deadlines are currently: first line/header **30 s**, whole inbound HTTP exchange **60 s**, whole outbound contact call **30 s**. The ordinary A2A client also wraps admission in **30 s** (`A/client.rs:44-49`, `:828-832`). An attachment route needs its own transfer lifetime/idle handling; merely adding a GET match leaves the 60-second whole-connection timeout intact.

The existing card GET helper is also a small-body helper, not a file downloader: it has a 15-second call budget and collects under the 256-KiB card limit (`A/contacts/offers.rs:33`, `:230-274`).

### Owner forwarding and strict ownership

`A/contacts/forward.rs:1-10`, `:180-227`, `:330-403`; `crates/agent/src/communication/mesh.rs:127-147`.

191-P3 first serves locally; a strict `not owner` result or a task’s engine prefix selects the owner. Only JSON-RPC POST `SendMessage`, `GetTask`, `CancelTask` is carried over `theuth/rpc/1`. The owner independently checks readiness/contact identity and its router’s ownership decision; it never forwards again.

The forward has a **10-second** deadline and a **2-MiB UTF-8 answer body**, collected before returning. This is suitable for file references/control responses, not a 5-GB byte tunnel.

### Full replies versus conventional artifacts

`F:1-8`, `:25-29`, `:110-119`; `crates/agent/src/supervisor.rs:6691-6692`.

A full-share request completes when consumed. Its later replies are separate `send_message` calls to the contact-only `~return` route, with a sliding seven-day conversation grant. Conventional A2A `Task.artifacts` and the full-share return path are distinct file-bearing paths; updating only one leaves asymmetric behaviour.

### Agent Card extensions

`A/contacts.rs:551-593`; `A/contacts/offers.rs:217-227`, `:311-330`; `A/contacts/rosters.rs:17-21`.

The common card builder adds machine/v1 and full/v1 in `capabilities.extensions`. The contact-specific card adds members/v1 parameters after budgeting its carried roster payload and before the signed card is returned. `card_members` finds the extension by URI and deserializes its params.

A new attachment extension belongs in this card capability path, with a matching reader analogous to `card_members`. It must coexist with the card’s size/JWS budget. Current cards advertise streaming generally, while contact streaming RPCs explicitly refuse it (`A/contacts/reach.rs:593-612`); binary GET streaming is a separate capability.

### Actual A2A wire conversion

`A/contacts/reach.rs:550-564`; `Cargo.toml:380-387`; `SDK:199-221`, `:280-323`.

The SDK supports Text/Raw/Url/Data and the cited serde field-presence shapes, but `IrohTransport::typed` actually calls `a2a_pb::protojson_conv::{to_value,from_value}`. Validate and test the real ProtoJSON ingress/egress, not only `serde_json::<Part>` round trips. The SDK’s permissive “first recognised content field” serde parser remains a reason not to infer oneof validation from that unit alone.

### Data directory and storage conventions

`crates/theuth/src/main.rs:34`, `:1569-1572`; `crates/desktop/src/launch.rs:194-199`; `crates/tools/src/resources.rs:34-67`.

The engine supports `--data-dir`/`THEUTH_DATA_DIR`, falling back to `~/.theuth`; desktop uses the same environment/default helper. Use the supplied runtime/resource root rather than hardcoding the owner’s home. `SessionResources` distinguishes durable data roots from private scratch roots.

Current ordinary attachments are `<root>/attachments/<sha256[..16]>.<ext>` (`crates/agent/src/context.rs:1935-1946`). Artifacts use `<data_dir>/artifacts/` plus `artifacts.sqlite`, with unique temporary writes and rename (`crates/artifact/src/store.rs:1-5`, `:726-750`). The attachment helper is a whole-slice writer; it is not a 5-GB streaming sink.

### Session attachment RPC contract

`crates/agent/src/rpc_artifacts.rs:16-54`; `crates/agent/src/rpc_attachments.rs:7-35`; `crates/artifact/src/rpc_artifacts.rs:230-292`.

`put_attachment`/`put_session_attachment` take whole base64 strings, decode into a `Vec`, allowlist extensions, and enforce 5-MiB image, 32-MiB PDF or 16-MiB audio limits. Private upload resolves session authority first. This is not an arbitrary-file/5-GB ingestion API.

`get_attachment(path, session?) -> String` returns **whole-file base64**, not a streaming download or MIME response. It canonicalizes into approved directories; missing artifact/attachment/spill files may be fetched from paired nodes, owner preferred. A new contact-files directory is not automatically in its allowed-directory set at `:283-287`.

### How attachments currently reach the model

`crates/agent/src/loop_.rs:8965-9020`; `crates/agent/src/vision.rs:205-225`, `:398-460`.

`Input.files` becomes one binary model part per file; unreadable files become text placeholders. `file_part` runs the image guard on a blocking thread and constructs base64 binary content. This is exactly the path **not** to use for automatic placeholder-only contact intake.

For text-only requests, the vision projection substitutes stable image markers and vision observations/notes rather than blindly resending binary parts; provider image rejection also flips subsequent projection to text (`crates/agent/src/loop_.rs:8238-8276`).

### Explicit read_file on a saved image today

`crates/tools/src/fs.rs:130-151`, `:181-210`; `I:108-159`, `:164-217`, `:296-319`.

`read_file` first reads the file into memory, sniffs supported image bytes, fits/saves an image under the session resource tree, and returns a tool output with a file path. That tool image then follows the model-image/vision-fallback machinery. A saved path is therefore not a promise that an explicit later read stays text-only.

The guard decodes under side/allocation checks and applies orientation; GIF/WebP/APNG are made still, alpha is preserved in PNG or flattened for JPEG. A fitting still PNG/JPEG is returned unchanged, so there is **no blanket EXIF/GPS-stripping guarantee**. This is existing explicit-read behaviour, not a proposed intake normalisation requirement.

### Record replica versus session log

`crates/agent/src/store/contacts.rs:17-27`, `:252-326`; `crates/agent/src/store.rs:994-999`; `crates/agent/migrations/0059_mesh_contacts.sql:1-6`.

Contact record tables are explicitly named in `RECORD_TABLES`, attached by `replica::attach`; typed writer support also names table keys/columns in `RecordTable`. The table contract is one primary-key column plus `hlc TEXT`, avoiding constraints that could permanently reject remote merged rows. Reconcile performs validity checks.

A new record-only table **does not itself bump `SESSIONS_WIRE_SCHEMA`**. The stronger “only session-column changes ever bump it” assertion is false now: incompatible meanings inside replicated JSON also require a bump (`crates/agent/src/store.rs:707-723`).

### Incoming and outgoing rendering

`UI/components/chat/MessagePeer.tsx:275-305`, `:322-360`, `:365-419`; `UI/components/chat/SendCard.tsx:120-125`, `:176-227`.

Desktop incoming contact messages use `MessagePeer`/`OriginCard`/`PeerCard`: typed sender provenance, reply quote, clamped body and actions. Full-share bodies use Markdown; neither peer props nor sent-card argument extraction has attachments. Outgoing sends use the separate “You → recipient” `SendCard`, receipt query and delivery track.

TUI incoming messages use the **Peers component**, `peer_card` and the wrapper/history adapter (`crates/tui/src/components/peers.rs:274-315`, `:402-438`; `crates/tui/src/model/history.rs:143-165`). Outgoing sends are generic `Entry::Tool` start/result entries, not a desktop-style SendCard (`crates/tui/src/model/agents.rs:404-446`; `crates/tui/src/model/history.rs:212-218`). Final Markdown paint strips terminal controls and unsafe hyperlink targets (`crates/tui/src/ui.rs:1151-1204`).

### Notices/placeholders and live event routing

`crates/agent/src/supervisor.rs:6672-6684`; `crates/agent/src/communication.rs:837-859`; `UI/store/reduce.ts:791-815`.

A consumed exchange publishes typed `PeerDelivered { session, origin, text, seq, turn }`; replay provenance comes from the stored history row. Desktop deduplicates by exchange ID; TUI consumes the `agent.peer-delivered` topic. A text placeholder can be carried without file bytes, but there is currently no typed file-notice/attachment field in this event.

Held/blocked/refused delivery is **not automatically a new chat bubble**. Sender state updates persist/refetch the receipt (`N:4603-4620`, `:4667-4669`, `:4731-4737`). Receiver first-contact holds update Attention via `ContactsHeldChanged` → `A2aChanged` (`A/lib.rs:1345-1357`; `A/contacts.rs:3057-3079`). A new persistent file notice must deliberately choose its history representation and wire/UI consumers, rather than publishing an ephemeral event alone.

### Existing shared Attachments UI

`UI/components/chat/Attachments.tsx:83-111`, `:195-226`; `UI/lib/files.ts:58-61`.

`Attachments` accepts local file paths or model-context occurrence records. The current viewer affordance is for images/PDFs; context removal is occurrence-based. Arbitrary-file download/save and large-file access need an additional contract; passing a 5-GB file through `get_attachment` would still base64-buffer it.

### Artifact mesh fetch and D7-style lazy own-node fetch

`B:30-75`, `:156-203`, `:247-338`, `:392-430`; `crates/mesh/src/net.rs:2109-2136`.

This is a reusable **family transport mechanism**: paired-only `theuth/artifacts/1`, length-delimited postcard frames, 4-MiB chunks, 64-MiB maximum frame, preferred owner first, temporary file then rename. Existing attachment fetch verifies the filename’s SHA-256 prefix on newly fetched bytes; existing cache hits return without rehashing.

It is not a drop-in contact-file service: `GetAttachment` names only a file under `attachments/`, has no session/manifest authorization, and the fetch attempt has a **60-second per-peer** deadline. There is no aggregate file-byte counter in `receive_file`; “bounded” here means frame/time bounds, not a total blob ceiling. Reuse the chunked transport pattern, but account explicitly for new directory/identity, 5-GB counting and transfer lifetime.

### Public-only HTTPS fetching for P3

`crates/web/src/fetch/ssrf.rs:23-79`, `:98-142`; `crates/web/src/fetch/mod.rs:29`, `:347-367`, `:417-430`, `:805-865`.

Public API: `web::fetch::check_url(raw, false)`, `is_blocked_hostname(host, false)`, `is_blocked_ip(ip, false)`. `false` means public-only. The normal fetcher checks every redirect and uses a guarded resolver so connection addresses are vetted, rather than trusting an earlier DNS check.

The guard accepts **HTTP and HTTPS**, so “public HTTPS only” requires an additional scheme rule on the initial URL **and every redirect**. A bare `check_url(false)` followed by an ordinary unguarded client would lose rebinding/redirect protection.

A fresh `Fetcher::new_with_timeouts` starts public-only; its high-level `request` still buffers/truncates into a `Vec` (`crates/web/src/fetch/mod.rs:861-865`, `:892-920`). A streaming-to-disk API is needed for 5 GB. Do not inherit the turn’s LAN twin: full-share contact tools intentionally keep LAN access (`crates/tools/src/egress.rs:1-9`); only this **new automatic URL-file fetch** must force public-only.

### BLAKE3 and macOS quarantine

`Cargo.lock:2391-2402` contains **blake3 1.8.7**, transitively used by iroh (`Cargo.lock:9783-9789`). Searches found no direct `blake3` dependency/use in workspace crate manifests/Rust source. Existing attachment/artifact identity is SHA-256-prefix based (`crates/agent/src/context.rs:1938-1941`; `B:24`, `:415-425`); adding BLAKE3 would be a new direct storage contract, not reuse of current file hashes.

No received-file `com.apple.quarantine`/xattr-setting implementation was found. Existing xattr handling is packaging-side removal, e.g. `scripts/bundle-companion.sh:88`; update archives explicitly omit xattrs (`scripts/package-update-macos.sh:51-54`). Those scripts are not an incoming-download quarantine service.

## 5. Surprises for the revised design

1. **The wire number has outrun migration numbering.** Migration 0067 may be free, but a replicated-column change in it must use wire 68, not 67 (`S:716-723`). A record-only addition still needs no session-wire bump.
2. **Five gigabytes requires a new byte path end to end.** Contact unary response, owner forward, upload RPC, UI attachment fetch and web fetch all currently collect/buffer or apply small limits (`A/contacts/reach.rs:533-539`; `A/contacts/forward.rs:397-403`; `crates/agent/src/rpc_attachments.rs:19-35`; `crates/artifact/src/rpc_artifacts.rs:289-292`; `crates/web/src/fetch/mod.rs:892-920`).
3. **A new GET route alone does not remove the 60-second transfer kill.** That timeout surrounds Hyper’s whole connection, including response-body completion (`A/contacts/reach.rs:239-248`). Binary-transfer timing must be separated from unary RPC timing.
4. **“No quotas/type restrictions” does not erase control-envelope bounds.** The 32-KiB message and 64-KiB A2A JSON limits remain; large bytes and potentially large file lists cannot simply be inserted into `SendMessage` JSON (`crates/agent/src/communication.rs:80`; `A/server.rs:49-50`).
5. **Placeholder-only intake must avoid the existing attachment pipeline.** `Input.files` automatically becomes model binary content, while explicit later `read_file` can show the saved image or convert a document (`crates/agent/src/loop_.rs:8965-9020`; `crates/tools/src/fs.rs:130-151`, `:181-218`). Distinguish “not automatically injected” from a stronger claim that tools can never read those bytes.
6. **Held/refused notices are not an existing general transcript-notice service.** Holds drive Attention and receipt state; consumed messages drive peer history/events (`A/lib.rs:1345-1357`; `N:4731-4737`; `crates/agent/src/supervisor.rs:6672-6684`). Persistent file placeholders need an explicit history/live/replay implementation.
7. **The own-mesh fetch is chunked, but not already a verified 5-GB contact cache.** It uses SHA-256 prefixes, skips cache-hit verification and times out each peer after 60 seconds (`B:30-35`, `:306-338`, `:395-425`). It is a useful substrate, not the finished feature.
8. **P3 must deliberately force public-only and HTTPS-only.** Full-share web tools retain LAN by design; the existing SSRF guard permits HTTP as well (`crates/tools/src/egress.rs:1-9`; `crates/web/src/fetch/ssrf.rs:58-67`). Preserve the former while adding the stricter automatic-fetch lane.
9. **The existing `tools::egress` scope is a LAN boolean, not a file-export approval/gate.** Its API is only `scope(lan, future)`/`lan()` (`crates/tools/src/egress.rs:15-24`). Do not mistake it for the new single file-egress gate.
10. **Arbitrary-file UI is new work even if image/PDF UI is reused.** Current peer/sent cards have no file fields, and the shared attachment viewer only treats image/PDF paths as viewable (`UI/components/chat/MessagePeer.tsx:275-298`; `UI/components/chat/SendCard.tsx:123-125`; `UI/components/chat/Attachments.tsx:83-111`).