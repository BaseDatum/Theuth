# 199 — CONTACT-FILES: files and pictures between contacts

Status: Approved for build 2026-11-17 from Sky's rulings. Docs only.

Code read at main `0323c5f3f`. The rulings are in `~/.theuth/briefs/2026-11-17/199-contact-files/rulings.md`. The read-only recon of main (pinned at `46bf524c5`, which differs only in TTS and voice code) is [199-recon.md](199-recon.md), and this design is reconciled with it. The mock is [199-mock.html](199-mock.html). This design replaces the unapproved draft "195-contact-attachments" (surveyed at `acec0d81`). It keeps the draft's A2A part research, its transport choice and its typed cards. Everything else in the draft is dropped (interpretation 12). Read alongside [183](183-remote-sessions-and-a2a.md), [186](186-contact-collaboration.md), [190](190-drop-cautious.md), [191](191-mesh-contacts.md) and [191-notes](191-notes.md); where 191 and its notes disagree, the notes win.

## Sky's words

- 2026-11-14 (this question produced the draft): "do we support sending files, images, etc. other content to contacts agents?"
- 2026-11-17: "did we actually build the ability for contacts to send images and files between one another? ... if not we need to move on that now."
- Scope: "Everything (P0–P3)"
- Q1 sending approval: "Trusted shares send freely"
- Q2 documents: "ANY file should be able to be sent/recieved as long as it's not bigger than 5 GB and there should be no file type restrictions or anything like that.  the file shouldn't be inserted in context, but stored in ~/.theuth somewhere and teh model given a placeholder to know that it was recieved."
- Q3 limits: "no limits, keep it simple."
- Q4 foreign URLs: "Any public HTTPS URL"
- Q5 quota: "no quota, keep it simple"
- Q6 trust: "Keep full shares + egress gate"

## Today (verified at `0323c5f3f`)

| What | Where | Fact |
|---|---|---|
| Tool | `crates/agent/src/peers.rs:296-312`, `:419` | `SendMessageArgs` has `to`, `message`, `await_reply`, `reply_to` and `request_id`, and no files. Its risk is `Mutating`. |
| Sending | `crates/a2a/src/client.rs:798-801`, `:814` | The client builds exactly one `Part::text` and accepts only `text/plain` output. |
| Receiving | `crates/a2a/src/server.rs:576-583`, `:594`, `:50` | `validate_send` refuses any message that sets `extensions`, and any part that is not text (`content_type_not_supported`). The outer body is at most 64 KiB (`BODY_LIMIT`). |
| Answers | `crates/a2a/src/client.rs:973-981`, `:1067-1068` | `Retained::of` and stream updates refuse any remote artifact with a non-text part. |
| A2A parts | `a2a-lf-0.4.2/src/types.rs:203-222`, `:281`, `:308`, `:380`; `Cargo.toml:384` | A `Part` has content `Text`, `Raw`, `Url` or `Data`, plus `filename`, `mediaType` and `metadata`. On the wire, fields are present only when set: `text`, base64 `raw`, `url`, `data`. `Message.extensions` lists extension URIs. The package is pinned at `=0.4.2`. |
| Contact transport | `crates/a2a/src/contacts/reach.rs:1-10`, `:149-158`, `:37`, `:246-248`, `:533-537`, `:550-564`; `crates/mesh/src/net.rs:52`, `:851-870` | Each iroh bidirectional stream on `theuth/a2a/1` carries one HTTP/1.1 request. `serve` reads the first line before routing. The whole exchange has one 60 s deadline. A unary client response is buffered whole, at most 2 MiB. Payloads are ProtoJSON (`a2a_pb::protojson_conv`). `Net::a2a_stream` opens another bidirectional stream on the cached contact connection. Each remote may have 16 streams at once. Large files therefore cannot use the unary path. |
| Card extensions | `crates/a2a/src/contacts.rs:66-68`, `:576-589` | The contact card advertises `machine/v1` and `full/v1` as `AgentExtension`s. |
| Who is the contact | `crates/a2a/src/config.rs:283-285`, `:448-455`; `crates/a2a/src/contacts/records.rs:1090-1097`; `server.rs:970-979`, `:324-330` | `ContactFacts.id` is the contact's record id (the 191 group root). The alias is a local display key. `contact_by_eid` maps any member endpoint of a contact's group to its alias. The gate admits only `IrohGate::Contact`. `admit_contact` charges a rate token and a concurrency permit. |
| Mesh forward | `crates/a2a/src/contacts/forward.rs:9-14`; `crates/agent/src/communication/mesh.rs:132`, `:135` | Only unary `SendMessage`, `GetTask` and `CancelTask` are carried to the owning machine: 10 s deadline, 2 MiB maximum. |
| Intake | `crates/agent/src/communication/network/full.rs:320-350`, `:386-452`; `crates/agent/src/communication/network.rs:741`, `:1330-1334`, `:3903`; `docs/design/191-notes.md:325-329`; `crates/agent/src/loop_.rs:9042-9069`, `:9140-9162` | `deliver_full` loads the stored exchange (`task.dedup_exchange`) and hands it to `full_intake`, which enqueues it as a mesh peer's message. The in-memory `full_inbound` map links the exchange to its task. When a chat moves, an unconsumed message fails with `HOLD_MOVED` ("the chat moved; send again"), because nothing carries it to the new owner. The model sees `<message from id …>body</message>`, and the body cannot contain any tag whose name begins with `message`. |
| Restart and one-row guarantees | `crates/agent/src/supervisor/pause.rs:203-220`; `crates/agent/src/store/communication.rs:1324-1364`, `:2150-2209`; `crates/agent/src/loop_.rs:5687-5727`; `crates/agent/src/communication/network/full.rs:681-829` | At engine start, `fail_interrupted_except` fails every `accepted` exchange (`interrupted_by_restart`) and every non-terminal A2A task, except those in paused trees. `consume_exchange` appends a peer row only while the exchange is `accepted` with no `history_seq`, which is the durable one-row guarantee the loop checks. `full_return` admits a `~return` answer as an exchange of its own, with no incoming task, and claims it with `claim_enqueued` (`:797`). |
| Session front door | `crates/agent/src/rpc_forward.rs:1-22`; `crates/agent/src/rpc_turns.rs:226-262`; `crates/agent/src/lib.rs:313-334`; `crates/agent/src/supervisor.rs:1708-1740`, `:12333`; `crates/agent/src/store/durability.rs:1040-1063`; `crates/agent/src/loop_.rs:5630` | `send_input` goes through `forward::route`, which carries it to the session's owner over `theuth/rpc/1` when the chat lives on another of our machines, or answers `OWNER_UNREACHABLE`. `SendInput.id` dedups retries in memory only (`seen`, 10 min). `append_input` writes the user row and its `input_accepted` step (which has an `input_id` field) in one transaction; the loop passes `None` for it today. |
| Cards from tags | `crates/desktop/ui/src/lib/injected.ts:374`, `:393`; `store/reduce.ts:397-405`, `:477-480`; `crates/tui/src/components/peers.rs:426` | The desktop and the TUI turn engine-written tags into cards. Self-closing notes (`session_moved`, `engine_restart`, `contact_inbox`) are parsed already. |
| Wire schema | `crates/agent/src/store.rs:701-723`; `crates/agent/src/store/capture.rs:19-40`; `crates/agent/migrations/` | `SESSIONS_WIRE_SCHEMA = 67`. 197 set it as a value change, with no migration. The session log replicates only the `ATTACHED` tables. The highest migration is `0066_knowledge_identity.sql`. |
| Reusable pieces, and what is missing | `crates/web/src/fetch/ssrf.rs:30`, `:193`; `fetch/mod.rs:296-300`, `:892`; `crates/tools/src/egress.rs:1-9`; `crates/artifact/src/rpc_artifacts.rs:230-292`; `crates/artifact/src/mesh.rs:35`, `:39`, `:306`; `crates/tools/src/fs.rs:122`, `:134`; `crates/agent/src/loop_.rs:8965-8970`; `crates/theuth/src/main.rs:34`, `:1569-1572`; `crates/tools/src/resources.rs:34-40` | The SSRF guard is public-only unless the owner scopes LAN access, and it allows both `http` and `https`. `Fetcher::request` reads the body into memory, with a cap. `tools::egress` is a per-call LAN boolean, not a file gate. `get_attachment` returns a whole file as base64, from fixed folders only. `theuth/artifacts/1` streams in chunks, with one `Req` per stream, a 60 s deadline per peer and SHA-256-prefix names. `read_file` reads the whole file. `Input.files` turns each file into a binary part for the model. The data dir is `--data-dir` or `THEUTH_DATA_DIR`, else `~/.theuth`. `SessionResources.root` is the data dir, or a private chat's scratch root. `blake3` is only a transitive dependency (via iroh). No quarantine-xattr code exists. |
| Secret state | `crates/secrets/src/store.rs:14`, `vault.rs:52`, `:355`, `key.rs:22`; `crates/mesh/src/identity.rs:1-3`, `:24`, `crates/mesh/src/lib.rs:73`; `crates/a2a/src/contacts/members.rs:124`; `crates/link/src/identity.rs:1`, `:30`; `crates/rpc-compose/src/bootstrap.rs:111`; `crates/oauth/src/lib.rs:426`, `:484-488`; `crates/agent/src/supervisor.rs:11707`; `crates/artifact/src/store.rs:35`; `crates/knowledge/src/lib.rs:67` | The vault is `secrets.sqlite`, keyed by `secrets.key` (the legacy file is `secrets.vault`). `mesh/identity` holds the iroh Ed25519 secret, which also signs roster chains. `link/identity` holds the link X25519 key. The RPC bearer is `token`. OAuth credentials are `oauth/<ns>.json` and `oauth/mcp/*`. The databases are `sessions.sqlite`, `artifacts.sqlite` and `knowledge.sqlite`. |

## Interpretations

These are the orchestrator's readings of the rulings. Each is binding for the build, and Sky may overrule any of them.

1. Any file type and any number of files, with one cap: 5 GB per file, which is 5,000,000,000 bytes. *Sky may overrule.*
2. No other limits and no quotas. Only the existing A2A envelope and text bounds stay. A physical failure fails that one transfer. Transfers queue; they are never refused. *Sky may overrule.*
3. Never in context: the receiving chat gets one typed placeholder per file, and the model opens the file with `read_file` if it chooses to. *Sky may overrule.*
4. Received files live under `~/.theuth` with sanitised, collision-safe names. Bytes go to a temporary file, are BLAKE3-verified, then moved into place. Partial files are removed on failure and on restart. Received files are quarantined on macOS and never opened automatically. *Sky may overrule.*
5. Bytes never ride inside the A2A envelope. Each file is one `iroh://` reference part, streamed on a new stream of the contact connection, resumable with `Range`, with an idle timeout and no total deadline. Only roster members of the same contact are dialled. *Sky may overrule.*
6. One engine-side egress gate: full shares send without an approval card, and theuth's own secret state is always refused. Owner turns and contact turns follow the same rule. *Sky may overrule.*
7. `send_message` gains `files` (absolute paths) for contact recipients. The sender hashes at send time and serves the original, or a cheap clone. A changed or vanished source fails on both sides. Offers persist across restarts and expire after a fixed window. *Sky may overrule.*
8. Both directions from P1: a contact's answer with files uses the same mechanism. *Sky may overrule.*
9. Mesh: the file lands on the engine that owns the receiving chat, and the placeholder replicates with the transcript. Reaching the bytes from another of our machines is P2. There is no manifest authority and no quota authority. *Sky may overrule.*
10. UI: received, sending (with progress), sent and failed cards in the desktop and the TUI, an image thumbnail, and the actions open, reveal, save as and copy path. *Sky may overrule.*
11. P3: accept inline `Raw` parts and URL parts from foreign A2A peers. Fetch any public HTTPS URL through the public-only SSRF guard, at most 5 GB. Toward foreign peers, send only what fits inline. *Sky may overrule.*
12. Dropped from the draft: the admission policy and ledger, limits and quotas, reception consent, the egress approval card, picture normalisation and vision fallback, the scoped document reader, the replicated manifest authority, and the MIME allowlist. *Sky may overrule.*

## Decisions

**D1. Real A2A 1.0 parts.** A file is a standard `Part`: a `url` reference between theuth engines, or inline `raw` toward foreign peers (P3). It is never a filename pasted into text, and never a custom `data` blob. A message that carries file references names the files extension in `Message.extensions`, so an old engine refuses it cleanly with `EXTENSION_SUPPORT_REQUIRED` (`server.rs:576-580`). Plain text messages carry no extension and keep working on every engine. Reason: the A2A part model already says what a file is, and an old engine fails loudly instead of silently dropping files.

**D2. Bytes travel on their own stream, straight to the holder.**
- The receiver opens a new bidirectional stream on the authenticated contact connection (`Net::a2a_stream`, `crates/mesh/src/net.rs:851-870`) to the roster member named in the URI, which holds the bytes.
- Bytes never cross owner forwarding. That path carries only unary `SendMessage`, `GetTask` and `CancelTask`, within 10 s and 2 MiB (`forward.rs:9-14`).
- `reach::serve` matches the `/a2a/files/` request line it has already read (`reach.rs:154`), before `serve_http`. The route is therefore outside the 60 s `HTTP_TIMEOUT`, which wraps the whole hyper connection, body included (`reach.rs:239-248`). It is also outside `Route::of`, `forward::route`, and the A2A token bucket and handler semaphore.
- The client writes body frames into the partial file as they arrive. It never uses the 2 MiB `Limited` collect (`reach.rs:533-539`).

Reason: the unary path cannot carry 5 GB. The contact connection already proves who the peer is, so a second ALPN and a second gate are not needed.

**D3. Text first, then one notice per file.**
- The message's text is delivered at once through today's paths, unchanged: the 190/191 intake (`full_intake`), and `full_return` for `~return`. Every existing rule applies to the text, `HOLD_MOVED` included.
- The delivered row carries one `state="receiving"` placeholder per file right after the wrapper, plus a fixed hint line.
- Each file downloads on its own from the durable queue. Once a file is saved *and* its exchange reads `consumed` or `replied`, the holder durably queues one notice for it. The notice goes to the chat's *current* owner as the typed operation `contact_file_notice` (D17).
- If the exchange instead ends `failed` or `suppressed` (`HOLD_MOVED`, `interrupted_by_restart`, a revocation), all its files fail. Saved files are rolled back by inode, no notice is sent, and the sender's receipts report the reason.
- No file is ever held behind another.

Reason: Sky's ruling cut the held intake. The text path stays exactly as built, and each file is one independent, retryable event.

**D4. Storage and the sanitiser.** `<root>` is the receiving session's `SessionResources.root` (`crates/tools/src/resources.rs:34-40`). For an ordinary chat that is the runtime data dir: `--data-dir` or `THEUTH_DATA_DIR`, else `~/.theuth` (`crates/theuth/src/main.rs:34`, `:1569-1572`). The one explicit exception, for privacy, is a private chat: its files go under its scratch root, which is removed with the chat. Publication is crash-safe and never overwrites, and the macOS quarantine is set and verified before the file is published. Reason: one predictable folder per contact per day is easy to find in Finder. No existing file is ever replaced.

**D5. Placeholders and notices; bytes are never auto-injected.** All file lines the model sees are engine-written `<message_file …/>` elements:
- `state="receiving"` in the message row, after `</message>`;
- `state="received"` or `state="failed"` in that file's own notice row.

Each comes with a fixed hint line. A contact cannot forge one. In the message row, `neutralise_delimiters` (`loop_.rs:9140`) escapes any tag starting with `message` in the body. A notice row is an input that only the holder engine writes, through the front door. Every attribute value is escaped. The promise is "never auto-injected", not "unreadable". The model may call `read_file` on the saved path, and `read_file` then shows the image or converts the document (`crates/tools/src/fs.rs:122`), within D14's bound. Reason: this is Sky's ruling. The notice row also gives P2 its replicated source of truth (D11).

**D6. The egress gate checks the opened file, not its path.** `communication::files::egress_open(path)` is new code; `tools::egress` is only a per-call LAN boolean (`crates/tools/src/egress.rs:1-9`).
1. Open with `O_RDONLY | O_NONBLOCK | O_CLOEXEC`. A FIFO with no writer therefore returns at once instead of blocking.
2. `fstat` the handle. Refuse anything that is not a regular file before reading a byte.
3. Require at most 5 GB.
4. Compare `(dev, inode)` with the protected set.
5. Clear `O_NONBLOCK` with `fcntl` and return the handle.

Hashing, cloning (`fclonefileat` from the handle on macOS) and serving all use that handle or the clone. Any reopen (serving the original without a clone, or after a restart) repeats steps 1–4, and also requires the recorded `(dev, inode)` and size. Serving never sends more than the declared length, even if the original grows. Every outgoing file passes the gate: the `send_message` tool, the human answer forms (D15), and inline `raw` sends (P3). Reason: Q1 and Q6 ask for one choke point. Matching the opened handle's inode defeats symlinks, hardlinks, case variants, special files and swap-after-check. The stated limit is 190's trust model: a full share's shell can copy a protected file first, and the copy has another inode.

**D7. The 5 GB cap is the only limit; transfers queue.** No file may exceed 5,000,000,000 bytes. The cap is checked:
- by the gate;
- on the declared `size`;
- on the bytes actually streamed;
- on a P3 response's length and its stream.

There is no type check, count limit, rate limit or quota. The 64 KiB envelope bounds a message at about 150 references. The receiver schedules downloads from its SQLite queue (`contact_files` rows), at most 2 at once per sending member and with no engine-wide cap. A download that loses its slot to the idle timeout returns to the back of the queue and resumes with `Range`, so a slow sender delays only its own files. Transport failures (connect error, stream reset, stream limit, including the 16 streams per remote, `net.rs:52`) retry with D8's backoff. Reason: Q2, Q3 and Q5. Nothing ever answers "too many".

**D8. One timer per active transfer.** Every active transfer runs under a single timer: `min(local deadline, last progress + FILE_IDLE)`, where `FILE_IDLE` = 60 s. This covers a contact GET on either side, the receipt retry and a P3 download.
- The local deadline is `min(declared expiry, received_at + OFFER_TTL)` for an incoming offer, and `sent_at + OFFER_TTL` for an outgoing one, with `OFFER_TTL` = 7 days.
- The deadline cancels a transfer even while bytes keep flowing, so a slow drip ends there.
- The idle part releases the queue slot; the deadline fails the file.
- Contact downloads resume with `Range`, backing off 5 s, doubling up to 10 min, until the deadline.
- An engine restart first resolves `publishing` rows, then deletes `files/partial/` and requeues unfinished downloads from byte 0. That honours interpretations 4 and 5.
- P3 downloads never resume.
- The notice outbox is our own mesh, so it has no deadline. It retries with the same backoff until the notice is delivered.

Reason: neither a peer's expiry nor its pace may keep our transfers alive past our own bound.

**D9. Source and outcome.** At send time the sender hashes from the gate's handle with BLAKE3. `blake3` 1.8.7, already built via iroh, becomes a direct dependency. The sender then tries `fclonefileat` into `files/outgoing/`. If that fails, it records `(dev, inode, size)` and serves the original (D6). The receiver sends one terminal receipt, `POST /a2a/files/<id>` with `received` or `failed`, as soon as the file is terminal (Contract). The sender keeps the clone and the offer until that receipt or the deadline. Its card stays "sending" until then and fails with "expired without a receipt" at the deadline. There is no `verified` state or receipt; nothing needs one now that no intake waits. Reason: "sent" means the file is saved on the receiving machine and its notice is durably queued. The A2A follower stops after 6 h (`client.rs:65`), so it cannot report on a 7-day offer.

**D10. Identity: local ids and the contact record.** Each side keys its rows with its own random local ids; the sender's id appears only inside the source URL the receiver stores. Authorisation uses the contact's immutable record id (`ContactFacts.id`, `records.rs:1090-1097`), never the alias, which is display only. Membership or block changes, including a 191 merge that changes the record id, cancel and re-check active and resumed GETs and receipts on both sides. Reason: aliases can be renamed locally, and a peer-chosen id must never name our rows or files.

**D11. Holder-qualified access; no replicated manifest.** The transfer table is node-local.
- A file's *holder* is the engine that owns the receiving chat when the message arrives. The holder keeps the file there even if the chat moves later.
- Every file-specific read names `(holder, id)`. The holder answers it, over our own mesh; current session ownership never decides.
- For a P2 fetch, the expected `bytes` and `blake3` come from that file's `received` notice in the fetching engine's own replicated copy of the transcript, never from the holder's answer. The fetcher finds the notice by its id (D17) through the same committed predicate as the status query. It has two forms:
  - the row at the `at_seq` of the `input_accepted` step whose `input_id` is the notice id;
  - the ended-share `errors` row keyed by session and notice id.

  "Fetch here" works from either.

Reason: ownership moves and files don't. The notice row replicates only between our own machines, and only the holder writes it, so it can be trusted without a manifest.

**D12. Typed cards; thumbnails only through the bounded pipeline.** The desktop and the TUI render `<message_file>` elements and sender-side states as file cards, never as raw tags. Every surface, the local desktop included, gets thumbnails only from `contact_file_thumbnail(holder, id)`, which runs the guarded decoder (`crates/tools/src/images.rs:118-141`). Nothing loads the original through `asset://`. On failure or oversize the card shows a type icon, and the transfer is unaffected. Reason: keeps the draft's typed cards. A 5 GB file must never reach a webview or base64.

**D13. Both directions.** A contact's agent answers a full-share message with `send_message(reply_to, files)` to the sender's `~return` route (`full.rs:1-8`). `full_return` (`full.rs:681-829`) delivers its text at once, with the receiving placeholders. The files then download and notify like any others, so `~return` is P1. The human answer forms (D15) use `~return` too. A conventional A2A peer answers in `Task.artifacts`. `Retained::of` and the stream-update path (`client.rs:973-981`, `:1067-1068`) take those file parts into the same storage, placeholders and notices. That is P3.

**D14. `read_file` is bounded.** `read_file` stats first. Above 256 MB (268,435,456 bytes) it answers `read_file: <path> is <size>; too large to read whole (limit 256 MB). Use shell tools such as head, tail, file or ffprobe.` It also reads with that cap (`take`), so a file that grows between the stat and the read cannot exceed it. Reason: `read_file` reads whole files into memory (`crates/tools/src/fs.rs:134`), and received files can be 5 GB. This limits reading, not transfers.

**D15. Human attach only on the chat's machine.** These file chips work only on the machine that owns the chat, where the gate can open the file:
- **Answer as this chat**;
- the Messaging sheet answer field;
- the TUI's attach on the answer prompt.

Elsewhere the control says "attach on <machine>". Phones and iPads, which control an engine over RPC (`00c518ce7`), show cards, thumbnails and paths, but never open or save the bytes. Reason: a phone has no engine to serve from, and the existing upload path is capped by type and size (`rpc_attachments.rs:19-25`).

**D16. Foreign peers and public HTTPS (P3).**
- **Accepted:** inline `raw` parts within the 64 KiB envelope, `https` URL parts, and both inside `Task.artifacts`.
- **New client:** `web::fetch::download` in `crates/web/src/fetch/download.rs`. It reuses `GuardedResolver` in public-only mode and never the LAN twin, whatever `tools::egress` allows the turn.
  - It sets no whole-request timeout; the existing client sets one (`client.rs:155-164`). It uses a 10 s connect timeout, and D8's timer runs from the request: the idle budget covers the wait for response headers as well as the gaps between body frames.
  - Redirects are followed by hand. `https` and the guard are re-checked on the initial URL and on every hop, because the guard alone allows `http:`.
  - Userinfo is refused, and no cookies or credentials are sent.
  - The length may be unknown. Bytes are counted against 5 GB, and size and BLAKE3 are computed locally while streaming.
  - A failed download restarts from zero, until the deadline.
- **Toward peers without the extension:** a file goes inline as `raw` only if the serialized request still fits 64 KiB. Otherwise the send fails, and nothing is uploaded publicly.

**D17. A typed file notice: `contact_file_notice`.** Reusing `send_input` would make a contact-derived notice `InputOrigin::External`. That resets the communication and reminder budgets (`continuation.rs:361-374`), skips any contact check, and is acknowledged before anything is durable. So notices get their own operation:

- **Call:** `contact_file_notice(session, notice_id, contact_id, notice)`. `notice_id` is UUIDv5 of `(holder, file id)`.
- **Routing:** it goes through the session front door (`crates/agent/src/rpc_forward.rs:1-22`). It runs locally when the chat is owned here; otherwise `forward::route` carries it over `theuth/rpc/1` to the current owner.
- **Auth:** it is internal-only. It is accepted only from this engine, or from a paired machine of our mesh carrying the front door's `Forwarded` marker, and never from a desktop, phone or contact client. The owner also checks that it owns `session`, and otherwise answers `NOT_OWNER`.
- **Share re-check:** the owner checks the contact's share for that chat: the export grant, or the live `~return` conversation, and the contact neither removed nor blocked.
  - **Live share:** the owner admits the notice as automatic input, `InputOrigin::Automatic` (`continuation.rs:49`), which continues the chain and never resets a budget. The journal source is the new `contact_file`. Session and agent tool ceilings and approvals are unchanged, and there is no approval card. When the chat is idle the notice starts a turn; when it is busy it waits for the next turn, like other automatic input.
  - **Ended share:** the owner records the notice without starting a turn, as a terminal engine-state row. `Store::record_error_once(session, id = notice_id, text)` is `record_error` (`store.rs:2286-2300`) with `INSERT … ON CONFLICT(id) DO NOTHING`. The `errors` table replicates, and its id is the dedup key.
- **Admission answer:** `accepted`. If this process already holds the notice id pending, or the notice is already committed, the owner answers `accepted` again and enqueues nothing. Every terminal path clears the pending id: committed, dropped as a duplicate, recorded as an ended-share row, or refused (not owner, chain limit, enqueue error). A failed attempt therefore never stays pending, and the holder's next re-send is admitted afresh.
- **Commit:** when the loop consumes the notice, `append_input` checks the journal for an `input_accepted` step of this session with `input_id = notice_id`. The check runs in the same write transaction that would insert the row and the step (`store/durability.rs:1040-1063`). If the step exists, the copy is dropped and counts as delivered, so two accepted copies always give one row, even after the in-memory `seen` TTL. Otherwise the row and the step, with `input_id = notice_id` and source `contact_file`, are written together. Today the loop passes `None` for `input_id` (`loop_.rs:5630`). `step_in` records `at_seq` in that transaction (`store/durability.rs:57-79`), so the step identifies the notice's row with no extra link field.
- **Status:** `contact_file_notice_status(session, notice_id) -> committed | unknown`, through the same front door. One shared predicate, `Store::notice_committed(session, notice_id)`, answers `committed` when this session's journal has an `input_accepted` step with that `input_id`, or its `errors` table has a row with that id for this session; otherwise `unknown`. Admission dedup, the commit-time check and the P2 lookup all use that same predicate. Both tables replicate, so a new owner after a move answers correctly.
- **Old owners:** an owner without the operation answers method-not-found. The holder retries with backoff and never falls back to `send_input`. Notices arrive only once the owner is upgraded; meanwhile the holder's file card shows the outcome.

Reason: a notice is contact-derived automatic input. It needs a durable acknowledgement and an authorization check, neither of which human input carries.

## Contract

### `send_message` `files`

```rust
/// Contacts only: absolute paths of local files to send with this message, any type, each
/// at most 5 GB. The receiver gets each file saved on its disk, not put into its context.
#[serde(default)]
pub files: Vec<PathBuf>,
```

- `message` may be empty when `files` is not, and the text part is then omitted.
- With several contact recipients, each gets its own offer, and each file is opened and hashed once.
- The `request_id` key covers the ordered `(blake3, size, name)` list. The same key with different files is refused: `request_id <id> was used with different files`.
- The call returns once the A2A message is accepted. `structured` gains `files: [{id, holder, to, name, size, media_type, blake3}]`, where `holder` is this engine's endpoint id. The text adds `N files offered (name size, …); offers expire <date>`.
- Errors, as `ToolOutput::err`. Nothing is sent for any of them:

| Cause | Text |
|---|---|
| Path is not absolute | `files: <path> is not an absolute path` |
| Missing, not a regular file (directory, FIFO, socket, device), or unreadable | `files: <path>: no such file` · `files: <path>: not a regular file` · `files: <path>: <io error>` |
| Over the cap | `files: <path> is <size> (over the 5 GB limit per file)` |
| Protected file | `files: <path> is theuth's own secret state and is never sent` |
| A recipient is not a contact | `files can go to contacts only; <addr> is not a contact` |
| No files extension on the contact's card | `<contact> cannot receive files: their theuth does not support them yet` |
| P3 foreign peer, file does not fit inline | `<peer> takes files only inside the message, and <name> (<size>) does not fit in 64 KiB` |
| Envelope full | `too many files for one message (over 64 KiB); send them in several messages` |

### A2A parts

A file reference part (P1). The id in the URL is the sender's local id:

```json
{
  "url": "iroh://5b2f0c8e9d41a7b3c6e05f1d2a9b8c7e4f3a2b1c0d9e8f7a6b5c4d3e2f1a0b9c/a2a/files/0f8e2d4c6a1b3957e0d2c4b6a8f01357",
  "filename": "handoff.pdf",
  "mediaType": "application/pdf",
  "metadata": {
    "https://theuth.io/a2a/ext/files/v1": {
      "size": 2516582,
      "blake3": "a3f1c09be27d4458e61f0b9c2d7e8a4f5b6c3d2e1f0a9b8c7d6e5f4a3b2c1d0e",
      "expires": "2026-11-24T14:32:00Z"
    }
  }
}
```

An inline `Raw` part (P3; a 1×1 PNG):

```json
{"raw": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==", "filename": "dot.png", "mediaType": "image/png"}
```

- **Composition:** an optional text part first, then one part per file in `files` order.
- **Extensions:** `"extensions": ["https://theuth.io/a2a/ext/files/v1"]` is set only when a reference part is present. A message without file references carries no extensions and is admitted exactly as today.
- **Wire format:** the contact wire is ProtoJSON, and the proto `Part` uses the same JSON names. Tests round-trip through `a2a_pb::protojson_conv`, not only through serde.
- **`mediaType`:** set by the sender from `mime_guess`, falling back to `application/octet-stream`. It is descriptive only.
- **Admission** refuses the whole message with `invalid_params` naming the part when:
  - the URL scheme is not `iroh`;
  - the path is not exactly `/a2a/files/<32 lowercase hex>`;
  - the host does not map to the caller's contact record id;
  - `size` is above 5,000,000,000;
  - `blake3` is not 64 lowercase hex;
  - `expires` does not parse;
  - a part object carries other than exactly one of `text`, `raw`, `url` or `data`.

  The exactly-one check runs on the raw JSON in the HTTP layer that bounds the body (`server.rs:50`), before the SDK router decodes it.
- `data` parts stay `content_type_not_supported`.
- A message with file references must have `Message.extensions` exactly `[EXT_FILES]`. Any other non-empty value stays `EXTENSION_SUPPORT_REQUIRED`.
- In P3 the same checks admit `raw` parts and `https` URL parts.

### Agent Card extension

```json
{"uri": "https://theuth.io/a2a/ext/files/v1", "required": false, "params": {"maxBytes": 5000000000}}
```

`EXT_FILES` sits beside `EXT_FULL` in `crates/a2a/src/contacts.rs` and is advertised from P1. The sender checks the contact's last fetched card. An owner behind a new member that lacks the extension refuses the message (D1).

### The file route on `theuth/a2a/1`

New module `crates/a2a/src/contacts/files.rs`, with `FILES_PATH = "/a2a/files/"`. `reach::serve` matches the path before `serve_http`, after the gate has admitted the caller as `IrohGate::Contact`; a non-contact gets the existing 401 from `refuse`, before the route. The route answers only 200, 206, 400, 404, 410 and 416.

```
GET /a2a/files/<sender-id>                 (optional) Range: bytes=<from>-
→ 200  content-length: <size>, accept-ranges: bytes          (no Range sent)
→ 206  content-range: bytes <from>-<size-1>/<size>, content-length: <size-from>
→ 404  unknown id, or the caller's contact record id differs from the offer's (one answer for both)
→ 410  text/plain "expired" | "missing" | "changed"
→ 416  content-range: bytes */<size>                          (from ≥ size)
→ 400  malformed request line, header or Range

POST /a2a/files/<sender-id>   {"state":"received"} | {"state":"failed","reason":"<≤200 bytes>"}
→ 200 · 404 · 410 (past the deadline) · 400 (body over 1 KiB, or not one of these)
```

- **Auth:** the caller's endpoint maps to a contact through `contact_by_eid`, and that contact's `ContactFacts.id` must equal the offer's `contact_id`. Any current member of the group qualifies. The receiver dials only an endpoint that maps to the record id the message came from, so a URL never introduces an endpoint.
- **Serving:** the sender serves exactly `size - from` bytes, from the clone or from the original reopened with D6's checks. A failed check answers `410 changed`, or `410 missing` if the file is gone.
- **Timers:** headers within `READ_TIMEOUT` (30 s), then D8's timer.
- **Receiver validation:**
  - `size == 0`: one GET without `Range`, which must answer 200 with `content-length: 0`.
  - `0 < have < size`: `Range: bytes=<have>-`, which must answer 206 with `content-range: bytes <have>-<size-1>/<size>` and `content-length: size - have`.
  - `have == size`: no GET; go straight to verification.
  - Without `Range`, the answer must be 200 with `content-length: size`.
  - Any other status, offset, total or length, a 200 to a `Range` request, or a 416 fails the file with "the sender's file changed".
  - EOF before `content-length` is a transport failure, retried with `Range`.
  - `404` or `410` fails the file with the server's reason.
- **Receipt:**
  - One terminal receipt per file, sent when the file is terminal on the receiver:
    - `received`: published, and its notice durably queued, in one transaction;
    - `failed`: with the reason.
  - It retries with D8's backoff until 200, 404 or 410, or until the deadline.
  - The sender applies the first terminal receipt only. A repeat answers 200 and changes nothing.
  - An outgoing file with no receipt by its deadline fails with "expired without a receipt".
- **Re-checks:** every GET, resume and POST re-checks membership, block state and the deadline on both sides. A contact reload cancels any active stream that no longer passes.

### Text first, then notices

**Admission.** The engine that owns the chat admits a message that carries file references. In `deliver_full` (`full.rs:320-350`) or `full_return` (`full.rs:681-829`), it writes one `in` row per file, in the same transaction that stores the exchange. Each row gets the holder's random local id and `exchange` = the exchange id. `Store::exchange_record` fills a new `PeerRecord.files` from those rows, in `seq` order. `peer_wrapper` (`loop_.rs:9042-9069`) writes them after `</message>` as `state="receiving"` placeholders, plus the receiving hint. The text is then delivered exactly as today: `full_intake`, or `full_return`'s own intake. It is consumed once, by `consume_exchange` (`store/communication.rs:1324-1364`, checked at `loop_.rs:5687-5727`).

**Downloads.** Downloads run from the queue, keyed by the local file id. The per-member scheduling, resume, timer and gate rules (D6–D8) are unchanged, and each download is independent of the others.

**States.**

```rust
enum InState { Queued, Receiving, Publishing, Saved, Received, Failed }  // Received, Failed terminal
enum OutState { Queued, Sending, Sent, Failed }                           // Sent, Failed terminal
enum FileDirection { Out, In }
enum NoticeState { Queued, Sent, Delivered }
```

Every transition is a conditional update (`… WHERE id=? AND state=<expected>`), and a terminal row never changes state again. The outbox columns `notice` and `notice_state` are not state; they may still be written on a terminal row, under the rules below.
- **Published while the exchange reads `accepted` or `held`:** the file is `saved`. It waits for its exchange.
- **Exchange reads `consumed` or `replied`:** in one transaction, `saved` becomes `received`, with `notice` = the exact notice text and `notice_state` = `queued`. A file published after that point goes straight to `received` in step 6. Writing the notice is `UPDATE … SET notice=? WHERE id=? AND notice IS NULL`, so enqueueing twice leaves one.
- **Exchange reads `failed` or `suppressed`** (`HOLD_MOVED`, `interrupted_by_restart`, a revocation): every non-terminal file of it becomes `failed` with the exchange's reason, and a `saved` file's published path is unlinked if its inode matches. No notice is written.
- **Download fails:** the file becomes `failed`, which is terminal; its failed receipt goes at once.
  - If its exchange is already consumed or replied, the failure notice is queued in the same transaction.
  - Otherwise the notice waits. When the exchange later reads `consumed` or `replied`, `UPDATE contact_files SET notice=?, notice_state='queued' WHERE id=? AND state='failed' AND notice IS NULL` writes it. Only the outbox columns change, never the state.
  - If the exchange later reads `failed` or `suppressed` instead, no notice is ever written, and the row stays as it is.

The terminal receipt follows the terminal state: `received`, or `failed` with the reason.

**Notice outbox.** The outbox is two columns on `contact_files`: `notice`, and `notice_state` (`queued`, `sent` or `delivered`).
- **queued → sent:** `contact_file_notice` answers `accepted`.
- **sent → delivered:** `contact_file_notice_status` answers `committed`.
- While the status answers `unknown`, the holder re-sends the notice on its D8 backoff ticks; the owner's commit-time dedup makes a re-send safe.
- Errors such as offline, `NOT_OWNER` or method-not-found retry on the same backoff, with no deadline: it is our own mesh.
- On restart, `queued` and `sent` notices resume from where they stopped.

**Chat moved.** A notice follows the chat: the front door sends it, and its status query, to whichever machine owns the session at that moment. The file stays on its holder. An unconsumed text still fails with `HOLD_MOVED` as built (`191-notes.md:325-329`), and its files fail under the third rule above.

**RPCs.** Each `(holder, id)` call is carried to `holder` over `theuth/rpc/1`, the way owner-forwarded calls are. A holder that is not connected answers `<machine> is offline`.
- `contact_file(holder, id) -> ContactFile`. Error: `no such file on <machine>`.
- `contact_file_thumbnail(holder, id) -> { mime, data }`. Errors: `not an image`, `too large for a thumbnail`, `no such file on <machine>`.
- `contact_file_export(holder, id, dest) -> PathBuf`. A no-clobber copy on the holder's disk. Errors: `no such file on <machine>`, `<dest> exists`.
- `contact_file_fetch(session, holder, id) -> PathBuf` (P2). It runs on the fetching engine. Errors: `no notice for <id> in this chat`, `<machine> is offline`, `verification failed`.

### Migration `0067_contact_files.sql`

The table is node-local and not in `ATTACHED`, so `SESSIONS_WIRE_SCHEMA` stays 67.
- A replicated column would have needed the wire value 68, because 67 is already used (`store.rs:716-722`). A record-replica table would need no bump.
- The number moves for a replicated column change, or for a replicated value change that an older engine would misapply (197's 67). 199 makes neither.
- The notice outbox does not replicate. The notice itself becomes a transcript row, which replicates the ordinary way.
- Placeholders and notices are ordinary text in `messages.body`. An older desktop shows them unparsed (`reduce.ts:397-403`); that affects display only.

```sql
-- 199 CONTACT-FILES. Node-local: not attached to the session log, so
-- SESSIONS_WIRE_SCHEMA is unchanged (67).
CREATE TABLE contact_files (
    id           TEXT PRIMARY KEY,  -- this engine's local id, 32 lowercase hex (random)
    direction    TEXT NOT NULL,     -- out | in
    session      TEXT NOT NULL,
    exchange     TEXT NOT NULL,     -- the exchange that carried it (in) or sent it (out)
    seq          INTEGER NOT NULL,  -- order within the message
    contact_id   TEXT NOT NULL,     -- ContactFacts.id, never the alias
    name         TEXT NOT NULL,     -- sanitised
    media_type   TEXT NOT NULL,
    size         INTEGER,           -- declared, then actual; NULL only for a P3 download in flight
    blake3       TEXT,              -- declared or computed; actual once received
    source       TEXT NOT NULL,     -- out: original path; in: the source URL (holds the sender's id)
    clone        TEXT,              -- out: clone path, if any
    dev          INTEGER,           -- out: the gate handle's; in: the partial's
    inode        INTEGER,
    path         TEXT,              -- in: target path, set before linking
    done         INTEGER NOT NULL DEFAULT 0,
    state        TEXT NOT NULL,     -- in: queued|receiving|publishing|saved|received|failed
                                    -- out: queued|sending|sent|failed
    error        TEXT,
    notice       TEXT,              -- in: the exact notice text, written with the terminal state
    notice_state TEXT,              -- in: queued | sent | delivered
    created_ms   INTEGER NOT NULL,
    deadline_ms  INTEGER NOT NULL
);
CREATE INDEX contact_files_session ON contact_files (session);
CREATE INDEX contact_files_exchange ON contact_files (exchange);
CREATE INDEX contact_files_outbox ON contact_files (notice_state) WHERE notice_state IN ('queued', 'sent');
```

Rows are kept after they settle, so cards stay right. Clones and partials are deleted when a row is terminal, or past its deadline. A sweep runs at startup and hourly. Before naming the migration, the builder re-checks that 0067 is still free in `crates/agent/migrations` and in every `theuth-*` worktree.

### What the model sees

**The message row.** Delivered at once, with the placeholders after `</message>`:

```
<message from="peer:maya" id="0193f0a2-6b1c-7d3e-9f40-1a2b3c4d5e6f" at="14:31">
Site survey and the old drawings, as promised.
</message>
<message_file from="peer:maya" message="0193f0a2-6b1c-7d3e-9f40-1a2b3c4d5e6f" name="site-survey.zip" type="application/zip" bytes="1200000000" holder="5f3a…(64 hex)" file="9d0c4b7e21a35f6880e1c2d3b4a59687" state="receiving"/>
<message_file from="peer:maya" message="0193f0a2-6b1c-7d3e-9f40-1a2b3c4d5e6f" name="drawings.pdf" type="application/pdf" bytes="24100000" holder="5f3a…(64 hex)" file="41b8d2f0c7e65a1394b2a8d0e3f1c5a7" state="receiving"/>
Files from peer:maya are arriving; a notice follows when each one is saved or fails.
```

**Each notice row.** One input per file, from theuth:

```
<message_file from="peer:maya" message="0193f0a2-6b1c-7d3e-9f40-1a2b3c4d5e6f" name="site-survey.zip" type="application/zip" state="received" saved="/Users/sky/.theuth/files/received/maya/2026-11-17/site-survey.zip" bytes="1200000000" blake3="7c1e…(64 hex)" holder="5f3a…(64 hex)" file="9d0c4b7e21a35f6880e1c2d3b4a59687" machine="Sky's MacBook Pro"/>
File notice from theuth, not from peer:maya: no reply to the contact is needed. The file is saved on disk, not shown to you; open it with read_file if you need it.
```

```
<message_file from="peer:maya" message="0193f0a2-6b1c-7d3e-9f40-1a2b3c4d5e6f" name="drawings.pdf" type="application/pdf" state="failed" reason="the sender's file changed" holder="5f3a…(64 hex)" file="41b8d2f0c7e65a1394b2a8d0e3f1c5a7"/>
File notice from theuth, not from peer:maya: no reply to the contact is needed.
```

- Every attribute value, and the sender inside each hint, is escaped with `loop_.rs` `attr` (`&`, `<`, `>`, `"`). That covers `from`, `message`, `name`, `type`, `bytes`, `blake3`, `saved`, `machine`, `holder`, `file`, `state` and `reason`.
- `from` is the address the `<message>` wrapper uses.
- `bytes` is the declared size in a placeholder and the actual size in a `received` notice. `blake3` is actual.
- `holder` is the receiving engine's endpoint id, and `file` is its local id.
- `name` is the sanitised declared name; the saved basename may add ` (2)`.
- The hint lines are fixed text as shown, with only the sender substituted.
- The desktop's `classify` and the TUI's `contact_row` parse each element as a self-closing note and drop its hint line. A notice row renders as a file card, never as a user bubble.
- **Ended share:** when the contact's share for the chat ended before the notice arrived, the owner records it as an engine-state row instead of an input (D17). No turn starts, and the model never sees it. The text is the same element followed by: `File notice from theuth: the share with <sender> has ended, so this was not given to the agent.`
  - The desktop and the TUI recognise an `errors` row whose text starts with `<message_file`. They render it as the same file card, labelled "not given to the agent", in history and in the live `AgentEvent::Error` path.
  - The generic error renderer never shows the raw `<message_file>` markup. If parsing fails, it shows "File notice (unreadable)".

### Wire events and consumers

- `AgentEvent::ContactFileChanged { session, file: ContactFile }` fires on each state change, and at most twice a second per file while bytes move. It rides the registered `AgentEvent` (`crates/agent/src/lib.rs:4364-4374`).
- `ContactFile` holds `{ id, holder, direction: FileDirection, state: FileState, exchange, contact_id, contact, name, media_type, size?, blake3?, done, error?, path?, machine, deadline }`. `FileState` is `InState` or `OutState`, tagged by `direction`. It is generated with ts-rs and lands in the OpenRPC and wire catalogs.
- `PeerDelivered` (`crates/agent/src/communication.rs:844-856`) gains `files: Vec<ContactFile>`, empty by default, so the live card shows the receiving placeholders.
- **Desktop:**
  - message rows through `store/reduce.ts` (`peerBody`/`peerEntry`, `:397-410`), and live through its `PeerDelivered` case (`:801`);
  - notice rows through `classify` (`lib/injected.ts:374`) as user rows;
  - progress from `ContactFileChanged`, which arrives through `store/sessions.ts` `dispatch` (`:925-937`), plus `contact_file(holder, id)` on load;
  - the sending side in `SendCard`, which takes `holder` and `id` from `structured`.
- **TUI:** rows in `components/peers.rs` (`contact_row`, `:426`), live from `agent.peer-delivered`. Its sending side is the generic tool entry (`model/agents.rs:404-412`).
- **Card actions** never move bytes by base64, and `get_attachment` is unchanged.
  - On the holder, the desktop shell has open (`openPath`), reveal (`revealItemInDir`), save-as (the dialog plugin, then `contact_file_export`) and copy path. The TUI has `platform::open::open_path`, opening the folder to reveal, `arboard` to copy, and save to `~/Downloads` through `contact_file_export`.
  - Another of our machines shows "stored on <machine>", plus the thumbnail and metadata from the holder (P1). P2 adds **Fetch here** (`contact_file_fetch`).
  - Phones and iPads follow D15.
- **Thumbnails:** `contact_file_thumbnail` runs on the holder. It reads at most 64 MB, calls `tools::images::fit(bytes, 512)`, and returns the actual MIME from `Fitted`.

### Storage and sanitisation

Locations under `<root>` (D4); directories are 0700 and files 0600:
- `files/received/<contact>/<YYYY-MM-DD>/<name>`
- `files/partial/<local-id>`
- `files/outgoing/<local-id>` (clones)
- `files/cache/<holder>/<id>/<name>` (P2)

Sanitising, applied to the declared `filename` (P3: the last URL path segment; else `file`) and to the contact's display name for the folder:
1. Keep only the text after the last `/` or `\`.
2. Drop control characters (Unicode Cc) and bidi controls: U+061C, U+200E, U+200F, U+202A–U+202E, U+2066–U+2069.
3. Replace each of `: * ? " < > |` with `_`.
4. Trim spaces and dots from both ends, repeatedly, until nothing changes: `. report .` becomes `report`, and `.zshrc` becomes `zshrc`.
5. An empty result becomes `file`. A Windows reserved stem (CON, PRN, AUX, NUL, COM1–9, LPT1–9) gets a `_` prefix.
6. Cap at 200 UTF-8 bytes at a character boundary, keeping an extension of 20 bytes or less. Then repeat step 4.

Names are not normalised. The filesystem's own equality decides collisions, and no-clobber publication handles every clash. APFS ignores normalisation, and ignores case unless the volume is case-sensitive, so case-collision test expectations depend on the test volume.

Publication. Ownership of a published file is decided by inode, never by name or hash.
1. Stream into `files/partial/<local-id>` and `fsync`.
2. Check the size against the declared size (P3: within 5 GB). Compute BLAKE3 and compare it with the declared digest; for P3, record it.
3. On macOS, set `com.apple.quarantine` on the partial, read it back and compare. The value is `0081;<unix seconds, lowercase hex>;theuth;<UUID, uppercase>`.
   - The format is `flags;timestamp;agent;uuid`. 0x0081 = 0x0001 (quarantined download) + 0x0080, as non-sandboxed downloaders write it. Without 0x0002 (sandboxed creator) or 0x0040 (user-approved), Gatekeeper checks the file at first open.
   - Apple does not document these bits. Sources: Eclectic Light, "xattr: com.apple.quarantine, the quarantine flag" (2017-12-11, https://eclecticlight.co/2017/12/11/xattr-com-apple-quarantine-the-quarantine-flag/), and Mac Forensics, "Quarantine Events and Gatekeeper" (https://www.macforensics.app/en/blog/quarantine-events-and-gatekeeper).
   - No `QuarantineEventsV2` record is written.
4. Choose a target name. In one write, persist `state='publishing'`, `path=<target>`, and the partial's `(dev, inode)`.
5. `std::fs::hard_link(partial, target)`. It fails if the name exists; on a clash, go back to step 4 with the next `stem (n).ext`.
6. In one transaction, record the path, inode, actual size and BLAKE3. The state becomes `received`, with `notice` and `notice_state='queued'`, if the exchange is consumed or replied, and `saved` otherwise (States). Only after this commit is the partial's link removed.

A failure before step 5 removes the partial. A failure after step 5 removes the target only if the target's inode equals the recorded one; it then removes the partial. Nothing is ever opened or executed automatically.

**Restart.** Each `publishing` row is resolved *before* the global `files/partial/` cleanup:
- The target exists with the recorded inode: it is ours. If the partial still exists, finish step 6; its bytes were verified in step 2, so at most one notice is queued. If the partial is gone, remove the target and requeue the download.
- The target exists with a different inode: it is never removed, never adopted. If the partial exists, the row retries publication under the next collision-suffixed name. Otherwise it requeues the download.
- No target: retry step 5 from the partial, or requeue the download if the partial is gone.

Only then are the remaining partials deleted, and unfinished downloads requeued from byte 0.

### Protected set

Every theuth-owned secret, key and store that the engine creates under its root, taken from the source. Each SQLite file comes with its `-wal`, `-shm` and `-journal` siblings. No store writes a backup copy; the only backups in the tree are dreaming's skill-file backups, which are not stores. There is no extension ban.

| File, relative to the root | Source |
|---|---|
| `secrets.sqlite` | `crates/secrets/src/store.rs:14`; `vault.rs:355` |
| `secrets.key` | `crates/secrets/src/key.rs:22` |
| `secrets.vault`, `secrets.vault.migrated` | `crates/secrets/src/vault.rs:51-52` |
| `mesh/identity` (iroh key; also signs rosters) | `crates/mesh/src/identity.rs:24`; `lib.rs:73`; `crates/a2a/src/contacts/members.rs:124` |
| `link/identity` (X25519 key) | `crates/link/src/identity.rs:1`, `:30` |
| `token` (RPC bearer) | `crates/rpc-compose/src/bootstrap.rs:111` |
| `oauth/*.json`, `oauth/mcp/*` | `crates/oauth/src/lib.rs:426`, `:484-488` |
| `signal/presage.db` (Signal protocol keys) | `crates/signal/src/presage.rs:350`; `channel.rs:1009-1010` |
| `sessions.sqlite` | `crates/agent/src/supervisor.rs:11707` |
| `skills.sqlite` | `crates/agent/src/supervisor.rs:12153` |
| `artifacts.sqlite` | `crates/artifact/src/store.rs:35` |
| `knowledge.sqlite` | `crates/knowledge/src/lib.rs:67`; `store.rs:647` |
| `replica.sqlite` | `crates/replica/src/lib.rs:39`, `:189` |
| `mesh.sqlite` | `crates/mesh/src/lib.rs:64`, `:293` |
| `roster.sqlite` | `crates/roster/src/lib.rs:68`, `:3806` |
| `link.sqlite` | `crates/link/src/store.rs:20`; `lib.rs:568` |
| `double/double.sqlite` | `crates/double/src/store.rs:22-32` |
| `schedule.sqlite` | `crates/schedule/src/lib.rs:49`, `:3337` |
| `tasks.sqlite` | `crates/tasks/src/lib.rs:65`, `:1623` |
| `mcp.sqlite` | `crates/tools/src/mcp.rs:2418` |
| `voice.db` | `crates/voice/src/history/mod.rs:245` |
| `meetings/meetings.sqlite`, `meetings/briefs.sqlite` | `crates/meeting/src/store.rs:23`, `:302`; `cache.rs:19`, `:129`; `lib.rs:88` |
| `documents/index.sqlite`, `documents/mirror.sqlite` | `crates/document/src/store.rs:26-27`, `:191`; `index/db.rs:15`; `lib.rs:401` |
| `autoresearch/autoresearch.sqlite` (default root) | `crates/autoresearch/src/store.rs:29`; `config.rs:398`; `lib.rs:78` |
| `cells/cells.sqlite` | `crates/cells/src/kernel.rs:98`; `lib.rs:50` |
| `codeindex/*.sqlite` (per-project index and `-map.sqlite`) | `crates/codeindex/src/lib.rs:64`; `map/store.rs:2`, `:110-115` |

The set is built at each gate call, for this engine's runtime root and, when different, the default `~/.theuth`. Building it stats the listed files and expands the `oauth/` and `codeindex/` globs. The opened handle's `(dev, inode)` must not be in it, so symlinks, hardlinks and case variants cannot evade it. Every other file may be sent, including a received `research.sqlite`. The stated limit: the protected files of another custom data root on the same machine, or of an autoresearch root configured outside `<root>`, are not in the set.

## Failure modes

| Cause | Sender sees | Receiver sees | Cleaned up |
|---|---|---|---|
| Disk full (receiver) | Failed: "<contact>'s disk is full" (receipt) | The text as usual; a failed notice: "disk full on <machine>" | Partial |
| Connection loss mid-transfer | Sending, "waiting for <contact>" | Queued again; resumes with `Range` | Nothing until it ends |
| Slow drip | Sending | That sender's files wait; nothing else does. It ends at the deadline (D8) | At the deadline |
| Receiver crash mid-download or mid-publish | Sending | After restart: publishing resolved by inode, downloads resume, one notice per file | Partials; our own targets only |
| Digest mismatch | Failed: "the file changed before it was fetched" | Failed notice; no retry | Partial; clone after the receipt |
| Sender file changed or deleted | `410 changed` / `410 missing` → failed | Failed notice with that reason | Partial |
| Receiver offline at send | Tool error and failed card: "<contact> is offline" | Nothing | Offer row; clone |
| Owner machine offline (191) at send | Failed: "the chat's machine is offline" | Nothing | Offer row; clone |
| Chat's owner offline when a notice is due | Sent (the receipt does not wait for the notice) | The notice arrives when the owner is back (the outbox retries) | — |
| Owner crashes after accepting a notice, before consuming it | Sent | The status stays `unknown`, the holder re-sends, and exactly one row results | — |
| Share revoked, contact removed or blocked, before the notice | Sent | The notice is recorded as an engine-state row; no turn starts | — |
| Owner on an old build (no `contact_file_notice`) | Sent | No notice until the owner upgrades; the holder retries and never uses `send_input`; the holder's card shows the outcome | — |
| Forwarded to the owner (191) | Normal | Normal: the owner fetches from the holder named in the URI | — |
| Chat moved mid-download, text already consumed | Sending, then sent | The notice reaches the new owner through the front door; the file stays on its holder | — |
| Exchange failed or suppressed (`HOLD_MOVED`, restart, revocation) | Failed with that reason | No notices; saved files rolled back by inode | Partials; our own targets |
| Contact removed, blocked or re-membered | Active GETs cut; failed | Downloads cancelled; failed notices | Partials |
| Holder offline mid-transfer | Card frozen on that machine | Queued; retries until the deadline | At the deadline |
| No receipt by the deadline | Failed: "expired without a receipt" | — | Clone |
| Old peer without the extension | Tool error before anything leaves; behind a new member: "their machine for that chat needs updating" | Nothing | Offer row |
| Offer past its deadline | Failed: "expired before <contact> fetched it" | Failed notice: "the offer expired" | Clone; partial |
| Non-public URL, or headers never sent (P3) | — | Failed notice: "not a public HTTPS address" / "no response". A static `http:` or userinfo URL refuses the message | Nothing written |
| Oversize | Tool error before anything leaves | Declared over 5 GB: message refused. Streamed past `size` or past 5 GB: failed notice | Partial |

## Phases

| Phase | Gate | Tests (minimum) |
|---|---|---|
| **P0 contract** | <ul><li>Types and enums (`InState`, `OutState`, `FileDirection`, `NoticeState`, `ContactFile`, `PeerDelivered.files`, `PeerRecord.files`).</li><li>RPC signatures, including `contact_file_notice` and `contact_file_notice_status`.</li><li>The `contact_file` journal source.</li><li>`0067_contact_files.sql`.</li><li>`egress_open` and the protected set.</li><li>The sanitiser.</li><li>Publication with inode ownership.</li><li>`blake3` as a direct dependency.</li><li>D14.</li><li>`EXT_FILES` defined, not advertised.</li><li>OpenRPC baseline accepted with Breaking 0.</li></ul> | <ul><li>Gate: a FIFO with no writer is refused at once, without blocking; a socket and a directory are refused.</li><li>Every file in the protected-set table, and its siblings, is refused, as are a symlink, a hardlink and a case variant of `mesh/identity`. A file swapped after the gate opened is caught. A received `research.sqlite` passes.</li><li>Sanitiser table: traversal, `\`, bidi, controls, `. report .`, leading dot, reserved stems, the 200-byte cut.</li><li>Publication against a real temporary directory: NFC and NFD forms of one name, and `Report.pdf` beside `report.pdf` (expectation per the volume's case sensitivity). An existing `report.pdf` with different bytes survives a crash injected after each of steps 1–6, and after each restart branch. An identical pre-existing `report.pdf` is never adopted; ours becomes `report (2).pdf`.</li><li>Migration: it applies, the table is not in `ATTACHED`, and `SESSIONS_WIRE_SCHEMA == 67`.</li><li>`read_file` refuses a 300 MB file and cuts a file that grows during the read at 256 MB.</li></ul> |
| **P1 contacts, both ways; desktop and TUI cards** | <ul><li>The extension advertised.</li><li>The file route.</li><li>The queue and D8's timer.</li><li>Resume.</li><li>Receipts.</li><li>Placeholders in the message row.</li><li>`contact_file_notice` and `contact_file_notice_status` (D17): automatic origin, owner auth, share re-check, commit-time dedup.</li><li>The notice outbox.</li><li>Holder RPCs.</li><li>Cards and thumbnails.</li><li>`~return` with files.</li></ul> | <ul><li>Two-engine e2e: image and PDF A→B, and B→A over `~return`. Text arrives at once with receiving placeholders, then one notice per file.</li><li>A plain text message with no extensions works on new and old engines.</li><li>ProtoJSON round-trip both ways; a part with both `text` and `url` is refused.</li><li>A 5,000,000,001-byte sparse file is refused.</li><li>The exact placeholder, notice and hint lines, with `"`, `<` and `&` in name, media type, machine, sender and reason.</li><li>`<message_file` in a body is neutralised. No binary part reaches the provider.</li><li>Transfer: `Range` resume; a 416, a 200 answer to a `Range` request, a wrong `content-range` total and a short EOF; a zero-length file.</li><li>Timing: a transfer longer than 60 s completes; a stalled one loses its slot and resumes; a 1-byte-per-59-s drip is killed at a short test deadline.</li><li>Source: a replaced source gives `410 changed`; a grown original is served at its declared length.</li><li>A contact blocked mid-transfer cuts both sides. A URL host belonging to another contact is refused without dialling.</li><li>A crash after saving, before the notice is queued: restart queues exactly one notice.</li><li>A notice enqueued twice gives one `contact_files` notice.</li><li>The owner crashes after accepting the notice, before consuming it: the holder re-sends, and exactly one row results.</li><li>Two copies consumed: one row.</li><li>The share is revoked before the notice arrives: an engine-state row, no turn.</li><li>The share is revoked after admission but before consumption: the ended-share policy applies (an engine-state row, no turn), and the pending id is cleared.</li><li>A refused admission clears the pending id; the next re-send is admitted.</li><li>A download fails before the text is consumed. When the exchange is later consumed, one failure notice is written and the file's state is unchanged. When the exchange later fails, no notice is written and the state is unchanged.</li><li>The notice is `InputOrigin::Automatic`: neither the reminder budget nor the communication chain resets.</li><li>A call without the `Forwarded` marker from a non-mesh client is refused; a call for a session owned elsewhere answers `NOT_OWNER`.</li><li>Chat moved mid-download: the notice and its status go to the new owner.</li><li>Owner offline: the notice is retried and delivered later.</li><li>An old owner answers method-not-found: the holder retries and never calls `send_input`.</li><li>The exchange fails with `HOLD_MOVED` after one file is saved: that file is rolled back, no notice is sent, and the sender gets a failed receipt.</li><li>No receipt by the deadline gives "expired without a receipt".</li><li>n1 forwards to the owner n2, which fetches from the holder.</li><li>The quarantine xattr read back matches the spec.</li><li>A thumbnail still works after the chat has moved (answered by the holder).</li><li>UI: receiving placeholder card, notice card, engine-state row as a file card in history and live (never raw markup in the error renderer), live view equals history, sender states, TUI lines, and the icon when a thumbnail fails.</li></ul> |
| **P2 human attach; access from another of our machines** | <ul><li>D15 attach.</li><li>New `Req::GetContactFile { id }` on `theuth/artifacts/1`, plus `contact_file_fetch(session, holder, id)`:<ul><li>The holder serves only a `received` row to a paired machine, in `receive_file`'s chunks (`crates/artifact/src/mesh.rs:306`), under D8's idle timer in place of the 60 s per-ask `TIMEOUT` (`:35`).</li><li>The fetching machine takes the expected `bytes` and `blake3` from the `received` notice in its own copy of the session (D11, either form), never from the holder.</li><li>It publishes to `files/cache/<holder>/<id>/<name>` with the same inode-owned steps, and verifies on fetch and on every cache hit. It never takes the existing cache-hit shortcut (`mesh.rs:395-420`).</li></ul></li></ul> | <ul><li>A human answer with two files arrives.</li><li>A protected pick is refused. Attach on a non-owning machine says "attach on <machine>".</li><li>After the chat moves to node 3, node 2 fetches a 1 GB file (over 60 s) from the holder, and its thumbnail still shows.</li><li>"Fetch here" works for a file whose notice was recorded as an ended-share `errors` row.</li><li>A holder answering different bytes, and a tampered cache entry, are refused.</li><li>An `out` id, or an id with no notice, is refused.</li></ul> |
| **P3 foreign A2A and public HTTPS** | <ul><li>`raw` and `https` admitted.</li><li>Artifact file parts through `Retained::of`.</li><li>`web::fetch::download` (D16).</li><li>Inline `raw` toward peers without the extension.</li><li>`accepted_output_modes` adds `*/*`.</li><li>Text, status and artifact text bounds unchanged.</li></ul> | <ul><li>A `raw` part is saved with its computed BLAKE3 and notified.</li><li>A public file with no `content-length` is fetched.</li><li>A transfer longer than the old client's whole-request timeout completes.</li><li>An HTTPS server that never sends headers is killed at idle.</li><li>Refused: `https://127.0.0.1`, a redirect to `http:`, a redirect to `10.0.0.1`, CGNAT, `169.254.169.254`, userinfo.</li><li>Over 5 GB by header and by stream.</li><li>A cut download restarts from zero.</li><li>An artifact file part in an answer.</li><li>A file too large for inline gives the error text.</li></ul> |

## Verification (the closer's live proof)

Two scratch engines, A and B, are paired as contacts over real iroh with no relay, fake models and scratch data dirs. Node A2 joins A's mesh for P2. Record a log or screenshot for each step.

1. A's agent calls `send_message(to=B's contact, files=[a PNG, a 1 GB file])`.
   - B's chat shows the text at once, with two `state="receiving"` placeholders and the hint.
   - Then one notice per file arrives. Each file exists at `saved`, with mode 0600 and a matching BLAKE3, and `xattr -p com.apple.quarantine` matches the spec.
   - The fake provider log shows no file bytes.
   - A's cards go from sending to sent.
2. B answers with `reply_to` and a file: A gets the text at once, then the notice.
3. Mid-1 GB:
   - Cut the stream: the download resumes with `Range: bytes=N-`.
   - Restart B: it resumes from 0%, and exactly one notice follows.
   - Kill B's chat owner right after it accepts the notice: the holder's status query reads `unknown`, it re-sends, and exactly one row results.
   - Move B's chat to another machine of B's mesh: the notice lands there.
4. A refuses `mesh/identity`, a hardlink to it outside the data dir, `SESSIONS.SQLITE`, and a FIFO. A received `research.sqlite` is forwarded.
5. Desktop and TUI captures: a receiving placeholder; a received image with a thumbnail; a failed notice (source deleted mid-transfer); the sender's sending, sent and failed cards.
6. P2: after the chat moves, A2 fetches and opens a file held by A. P3: a public HTTPS URL is fetched, and a loopback URL is refused.
