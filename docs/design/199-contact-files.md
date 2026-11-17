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

**D1. Real A2A 1.0 parts.** A file is a standard `Part`: a `url` reference between theuth engines, or inline `raw` toward foreign peers (P3). It is never a filename pasted into text or a custom `data` blob. A message that carries references names the files extension in `Message.extensions`, so an old engine refuses it cleanly with `EXTENSION_SUPPORT_REQUIRED` (`server.rs:576-580`). Reason: the A2A part model already says what a file is. Any A2A peer can read these parts, and an old engine fails loudly instead of silently dropping files.

**D2. Bytes travel on their own stream, straight to the holder.**
- The receiver opens a new bidirectional stream on the authenticated contact connection (`Net::a2a_stream`, `crates/mesh/src/net.rs:851-870`), to the roster member named in the URI, which is the machine that holds the bytes.
- Bytes never cross owner forwarding, which carries only unary `SendMessage`, `GetTask` and `CancelTask`, within 10 s and 2 MiB (`forward.rs:9-14`).
- `reach::serve` matches the `/a2a/files/` request line it already reads (`reach.rs:154`) before `serve_http`. The file route is therefore outside the 60 s `HTTP_TIMEOUT`, which wraps the whole hyper connection including the body (`reach.rs:239-248`). It is also outside `Route::of`, `forward::route`, and the A2A token bucket and handler semaphore.
- The route has a 60 s idle timeout and no total deadline.
- The client writes body frames into the partial file as they arrive. It never uses the 2 MiB `Limited` collect (`reach.rs:533-539`).

Reason: the unary path cannot carry 5 GB. Riding the contact connection reuses the identity check that already admits the contact, so a second ALPN and a second gate are not needed.

**D3. Intake waits for the message's files; the contract is durable.** A message that carries files is held until every file is terminal (received or failed). It then enters the chat as one row through the ordinary peer intake (`enqueue_peer_chained`): the `<message>` wrapper, then one placeholder per file. It never goes through `Input.files`, which would turn each file into a binary part for the model (`loop_.rs:8965-8970`). The full contract (identity, states, restart, a moved chat) is under "Durable intake" in the Contract. Reason: one row and one notice. The model never answers text whose files are still missing.

**D4. Storage and the sanitiser.** `<root>` is the receiving session's `SessionResources.root` (`crates/tools/src/resources.rs:34-40`). For an ordinary chat that is the runtime data dir: `--data-dir` or `THEUTH_DATA_DIR`, else `~/.theuth` (`crates/theuth/src/main.rs:34`, `:1569-1572`). The one explicit exception, for privacy, is a private chat: its files go under its scratch root, which is removed with the chat. Received files are linked into place without overwriting anything, after the macOS quarantine has been set and verified on the temporary file. Reason: one predictable folder per contact per day is easy to find in Finder. Names are sanitised on our side, and no existing file is ever replaced.

**D5. The placeholder; bytes are never auto-injected.** Each file becomes one engine-written `<message_file …/>` element after `</message>`, plus one fixed hint line. The tag name begins with `message`, so `neutralise_delimiters` (`loop_.rs:9140`) already stops a contact's body from forging one. Every attribute value is escaped. The promise is "never auto-injected", not "unreadable". The model may call `read_file` on the saved path, and `read_file` then shows the image or converts the document (`crates/tools/src/fs.rs:122`), within D14's bound. Reason: Sky's ruling. Reusing the wrapper's neutralisation makes the notice unforgeable without new escaping rules.

**D6. The egress gate checks the opened file, not its path.** `communication::files::egress_open(path)` is new code; `tools::egress` is a per-call LAN boolean (`crates/tools/src/egress.rs:1-9`). The gate:
- opens the file once and returns the open handle;
- `fstat`s that handle and requires a regular file of at most 5 GB;
- compares its `(dev, inode)` with the protected set (Contract).

Hashing, cloning (`fclonefileat` from the handle on macOS) and serving all use that handle or the clone. Any reopen (serving the original without a clone, or after a restart) re-checks `(dev, inode)`, size and the protected set. Serving never sends more than the declared length, even if the original grows. Every outgoing file passes the gate: the `send_message` tool, the human answer forms (D15), and inline `raw` sends (P3). Reason: Q1 and Q6 ask for one choke point. Matching by inode on the opened handle defeats symlinks, hardlinks, case variants and swap-after-check. The stated limit is 190's trust model: a full share's shell can copy a protected file elsewhere first, and the copy is a different inode.

**D7. The 5 GB cap is the only limit; transfers queue.** No file may exceed 5,000,000,000 bytes. The cap is checked:
- by the gate;
- on the declared `size`;
- on the bytes actually streamed;
- on a P3 response's length and its stream.

There is no type check, count limit, rate limit or quota. The 64 KiB envelope bounds a message at about 150 references. The receiver schedules downloads from its SQLite queue (`contact_files` rows): at most 2 at once per sending member, and no engine-wide cap. A download that makes no progress for the idle timeout gives up its slot and returns to the back of the queue, resuming with `Range`. A slow-drip sender therefore delays only its own files. Transport failures (connect error, stream reset, stream limit, including the 16 streams per remote, `net.rs:52`) retry with D8's backoff. Reason: Q2, Q3 and Q5 rule this. Nothing ever answers "too many".

**D8. Deadlines, resume and restart.**
- Every incoming offer gets a local deadline: `min(declared expiry, received_at + OFFER_TTL)`, with `OFFER_TTL` = 7 days. The deadline is enforced on every GET, resume and receipt, not only at admission.
- Contact downloads resume with `Range` after a dropped stream, backing off 5 s, doubling up to 10 min, until the deadline.
- When the receiving engine restarts, it deletes `files/partial/` and requeues unfinished downloads from byte 0. This honours interpretations 4 and 5 together.
- P3 downloads never resume; they restart from zero.

Reason: a sender's clock or expiry must not keep our transfers alive past our own bound. Seven days covers a contact who is away for a long weekend.

**D9. Source and outcome.** At send time the sender hashes from the gate's handle with BLAKE3. `blake3` 1.8.7, already built via iroh, becomes a direct dependency. It then tries `fclonefileat` into `files/outgoing/`; if that fails, it records `(dev, inode, size)` and serves the original (D6). When the receiver is done it reports with `POST /a2a/files/<id>`, and the sender's card moves to sent or failed. Reason: a clone makes "edited after sending" a non-event on a Mac. Elsewhere, the reopen check and the digest catch it. The A2A task follower stops after 6 h (`client.rs:65`), so it cannot report on a 7-day offer.

**D10. Identity: local ids and the contact record.** Each side keys its rows with its own random local ids. The sender's id appears only inside the source URL that the receiver stores. Authorisation uses the contact's immutable record id (`ContactFacts.id`, `records.rs:1090-1097`), never the alias, which is display only. Membership or block changes, including a 191 merge that changes the record id, cancel and re-check active and resumed GETs and receipts on both sides. Reason: aliases can be renamed locally. A peer-chosen id must never name our rows or files.

**D11. Mesh behaviour; node-local tables.** The transfer tables are node-local. The file lands on the engine that owns the receiving chat; a message taken by another member reaches that owner through `a2a_forward` (the references fit 64 KiB). The replicated placeholder names the stable holder (endpoint id) and the holder's file id, so any of our machines can fetch from the holder, even after the chat moves (P2). Reason: 191 already routes ownership. Node-local tables need no quorum and move no wire number.

**D12. Typed cards; thumbnails only through the bounded pipeline.** The desktop and the TUI render `<message_file>` elements and sender-side states as file cards, never as raw tags. Every surface, the local desktop included, gets thumbnails only from `contact_file_thumbnail`, which runs the guarded decoder (`crates/tools/src/images.rs:118-141`). Nothing loads the original through `asset://`. On failure or oversize the card shows a type icon, which never fails the transfer. Reason: keeps the draft's typed cards. A 5 GB file must never reach a webview or base64.

**D13. Both directions.** A contact's agent answers a full-share message with `send_message(reply_to, files)` to the sender's `~return` route (`full.rs:1-8`): same parts, same file route, same intake. That is P1. The human answer forms (D15) use `~return` too. A conventional A2A peer answers in `Task.artifacts`; `Retained::of` and the stream-update path (`client.rs:973-981`, `:1067-1068`) take those file parts into the same storage and placeholder. That is P3.

**D14. `read_file` is bounded.** `read_file` stats the file before reading it. Above 256 MB (268,435,456 bytes) it answers `read_file: <path> is <size>; too large to read whole (limit 256 MB). Use shell tools such as head, tail, file or ffprobe.` It also reads with that cap (`take`), so a file that grows between the stat and the read cannot exceed it. Reason: `read_file` reads whole files into memory (`crates/tools/src/fs.rs:134`), and received files can be 5 GB. This limits reading, not transfers.

**D15. Human attach only on the chat's machine.** The file chips in **Answer as this chat** and the Messaging sheet answer field, and the TUI's attach on the answer prompt, work only on the machine that owns the chat, where the gate can open the file. Elsewhere the control says "attach on <machine>". Phones and iPads, which control an engine over RPC (`00c518ce7`), show cards, thumbnails and paths but never open or save the bytes. Reason: a phone has no engine to serve from, and the existing upload path is type- and size-capped (`rpc_attachments.rs:19-25`).

**D16. Foreign peers and public HTTPS (P3).**
- **Accepted from foreign peers:** inline `raw` parts within the 64 KiB envelope; `https` URL parts; and both in `Task.artifacts`.
- **The download client is new:** `web::fetch::download` in `crates/web/src/fetch/download.rs`. It reuses `GuardedResolver` in public-only mode, and never the LAN twin, whatever `tools::egress` allows the turn.
- **Settings:**
  - no whole-request timeout (the existing client sets one, `client.rs:155-164`);
  - a 10 s connect timeout and a 60 s idle timeout per body read;
  - redirects followed by hand;
  - `https` and the guard re-checked on the initial URL and on every hop, because the guard alone allows `http:`;
  - userinfo refused;
  - no cookies or credentials.
- **Length and digest:** the length may be unknown. The client counts bytes against 5 GB and computes size and BLAKE3 locally while it streams, because a foreign peer supplies no digest.
- **No resume:** a P3 download restarts from zero on failure, until the deadline.
- **Toward peers without the files extension:** a file goes inline as `raw` only if the serialized request still fits 64 KiB. Otherwise the send fails, and nothing is uploaded publicly.

## Contract

### `send_message` `files`

```rust
/// Contacts only: absolute paths of local files to send with this message, any type, each
/// at most 5 GB. The receiver gets each file saved on its disk, not put into its context.
#[serde(default)]
pub files: Vec<PathBuf>,
```

- `message` may be empty when `files` is not empty, and the text part is then omitted. With several contact recipients, each gets its own offer, and each file is opened and hashed once.
- The `request_id` key covers the ordered `(blake3, size, name)` list. The same key with different files is refused: `request_id <id> was used with different files`.
- The call returns once the A2A message is accepted. `structured` gains `files: [{id, to, name, size, media_type, blake3}]`, and the text adds `N files offered (name size, …); offers expire <date>`.
- Errors, as `ToolOutput::err`. Nothing is sent for any of them:

| Cause | Text |
|---|---|
| Path is not absolute | `files: <path> is not an absolute path` |
| Missing, a directory, or unreadable | `files: <path>: no such file` · `files: <path>: not a regular file` · `files: <path>: <io error>` |
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

- **Composition:** an optional text part first, then one part per file in `files` order. `"extensions": ["https://theuth.io/a2a/ext/files/v1"]` is set when any reference part is present.
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
- `Message.extensions` must be exactly `[EXT_FILES]`; anything else stays `EXTENSION_SUPPORT_REQUIRED`.
- In P3 the same checks admit `raw` parts and `https` URL parts.

### Agent Card extension

```json
{"uri": "https://theuth.io/a2a/ext/files/v1", "required": false, "params": {"maxBytes": 5000000000}}
```

`EXT_FILES` sits beside `EXT_FULL` in `crates/a2a/src/contacts.rs` and is advertised from P1. The sender checks the contact's last fetched card. An owner behind a new member that lacks the extension refuses the message (D1).

### The file route on `theuth/a2a/1`

New module `crates/a2a/src/contacts/files.rs`, with `FILES_PATH = "/a2a/files/"`. `reach::serve` matches it before `serve_http`, after the gate has admitted the caller as `IrohGate::Contact`; a non-contact gets the existing 401 from `refuse`, before the route. The route answers only 200, 206, 400, 404, 410 and 416.

```
GET /a2a/files/<sender-id>                 (optional) Range: bytes=<from>-
→ 200  content-length: <size>, accept-ranges: bytes          (no Range sent)
→ 206  content-range: bytes <from>-<size-1>/<size>, content-length: <size-from>
→ 404  unknown id, or the caller's contact record id differs from the offer's (one answer for both)
→ 410  text/plain "expired" | "missing" | "changed"
→ 416  content-range: bytes */<size>                          (from ≥ size)
→ 400  malformed request line, header or Range

POST /a2a/files/<sender-id>                {"state":"received"} | {"state":"failed","reason":"<≤200 bytes>"}
→ 200 · 404 · 410 (expired) · 400 (body over 1 KiB or not this JSON)
```

- **Auth:** the caller endpoint maps to a contact through `contact_by_eid`, and that contact's `ContactFacts.id` must equal the offer's `contact_id`. Any current member of the group qualifies. The receiver dials only an endpoint that maps to the record id the message came from, so a URL never introduces a new endpoint.
- **Serving:** the sender serves exactly `size - from` bytes from the clone, or from the original reopened with D6's checks. If a reopen fails those checks the answer is `410 changed` (and `410 missing` when the file is gone).
- **Timing:** headers within `READ_TIMEOUT` (30 s). On the body, `FILE_IDLE` = 60 s with no bytes moved closes the stream. There is no total deadline.
- **Receiver validation:**
  - `size == 0`: one GET with no `Range`; it must be 200 with `content-length: 0`.
  - `0 < have < size`: `Range: bytes=<have>-`; it must be 206 with `content-range: bytes <have>-<size-1>/<size>` and `content-length: size - have`.
  - `have == size`: no GET; verify.
  - No Range sent: the answer must be 200 with `content-length: size`.
  - Any other status, offset, total or length, a 200 to a Range request, or a 416: the file fails with "the sender's file changed".
  - EOF before `content-length` is a transport failure, retried with `Range`.
  - `404` or `410`: the file fails with the server's reason.
- **Receipt:** the receiver retries the `POST` until it gets 200, 404 or 410, or until the deadline.
- **Re-checks:** every GET, resume and POST re-checks membership, block state and the deadline on both sides. A contact reload cancels any active stream that no longer passes.

### Durable intake

- **Identity:** the stored exchange record. `deliver_full` loads it by `task.dedup_exchange` (`full.rs:320-350`). Its `peer_exchanges` state stays `accepted` while files are pending. The intake state lives in `contact_file_intakes`, keyed by that exchange id.
- **States:**

  ```rust
  enum IntakeState { AwaitingFiles, Delivered, Failed }
  enum FileState { Queued, Sending, Sent, Receiving, Received, Failed }
  enum FileDirection { Out, In }
  ```

- **Link:** each incoming `contact_files` row carries `exchange`. Instead of calling `full_intake` directly (`full.rs:386-452`), `deliver_full` writes the intake row and its file rows in one transaction, then starts the downloads.
- **Delivery:** when every file row of an exchange is terminal, a single `UPDATE contact_file_intakes SET state='delivered' WHERE exchange=? AND state='awaiting_files'` runs. Only the caller that changed the row calls `full_intake` with the file notices, so a second call does nothing. File transitions are just as conditional: `… WHERE id=? AND state NOT IN ('received','failed')`. Terminal states never change again.
- **Restart:** on start, every `awaiting_files` intake rebuilds its `full_inbound` entry from the stored `TaskRef` columns (`network.rs:1330-1334`). Its files are re-resolved (deadline, membership, generation) and requeued from byte 0. The text stays withheld until every file is terminal. An intake whose route or contact no longer resolves fails.
- **Chat moved:** `network_session_drained` (`network.rs:3903`) cancels the downloads, deletes partial files and any files already received for that exchange, and fails the intake and its task with `HOLD_MOVED`. Each file's receipt reports `failed: the chat moved; send again`, so the sender's cards go failed. This follows 191's as-built rule that a waiting message is carried nowhere (`191-notes.md:325-329`).
- **Other failures:** a revoked route, a removed or blocked contact, or an enqueue refusal fails the intake with that reason. The task fails through `fail_task`, and the remaining files report failed.
- **RPC results and errors:**
  - `contact_files(session) -> Vec<ContactFile>`
  - `contact_file_thumbnail(session, id) -> { mime, data }`; errors `not an image`, `too large for a thumbnail`, `no such file`
  - `contact_file_export(session, id, dest) -> PathBuf`; errors `no such file`, `stored on <machine>`, `<dest> exists`
  - `contact_file_fetch(holder, id) -> PathBuf` (P2); errors `<machine> is offline`, `no such file on <machine>`, `verification failed`

### Migration `0067_contact_files.sql`

Both tables are node-local and not in `ATTACHED`, so `SESSIONS_WIRE_SCHEMA` stays 67.
- A replicated column would have needed migration 0067 to set the wire value to 68, because 67 is already used (`store.rs:716-722`). A record-replica table would need no bump.
- The number moves for a replicated column change, or for a replicated value change that an older engine would misapply (197's 67). 199 makes neither kind of change.
- The placeholder is ordinary text in `messages.body`. An older desktop shows it unparsed inside the peer card (`reduce.ts:397-403`); that affects display only.

```sql
-- 199 CONTACT-FILES. Node-local: neither table is attached to the session log, so
-- SESSIONS_WIRE_SCHEMA is unchanged (67).
CREATE TABLE contact_file_intakes (
    exchange    TEXT PRIMARY KEY,  -- peer_exchanges.id (task.dedup_exchange)
    session     TEXT NOT NULL,
    task        TEXT NOT NULL,     -- TaskRef: id, principal, export
    principal   TEXT NOT NULL,
    export      TEXT NOT NULL,
    chain       INTEGER NOT NULL,  -- FullWire chain count for enqueue_peer_chained
    contact_id  TEXT NOT NULL,     -- ContactFacts.id
    state       TEXT NOT NULL,     -- awaiting_files | delivered | failed
    error       TEXT,
    created_ms  INTEGER NOT NULL
);
CREATE TABLE contact_files (
    id          TEXT PRIMARY KEY,  -- this engine's local id, 32 lowercase hex (random)
    direction   TEXT NOT NULL,     -- out | in
    session     TEXT NOT NULL,
    exchange    TEXT NOT NULL,     -- in: contact_file_intakes.exchange; out: the outbound exchange
    seq         INTEGER NOT NULL,  -- order within the message
    contact_id  TEXT NOT NULL,     -- ContactFacts.id, never the alias
    name        TEXT NOT NULL,     -- sanitised
    media_type  TEXT NOT NULL,
    size        INTEGER,           -- declared; actual once received (NULL only for a P3 download in flight)
    blake3      TEXT,              -- declared or computed; actual once received (never NULL then)
    source      TEXT NOT NULL,     -- out: original path; in: the source URL (holds the sender's id)
    clone       TEXT,              -- out: clone path, if any
    dev         INTEGER,           -- out: (dev, inode) of the gate's handle
    inode       INTEGER,
    path        TEXT,              -- in: published path
    done        INTEGER NOT NULL DEFAULT 0,
    state       TEXT NOT NULL,     -- queued | sending | sent | receiving | received | failed
    error       TEXT,
    created_ms  INTEGER NOT NULL,
    deadline_ms INTEGER NOT NULL   -- in: min(declared expiry, received + OFFER_TTL); out: sent + OFFER_TTL
);
CREATE INDEX contact_files_session ON contact_files (session);
CREATE INDEX contact_files_exchange ON contact_files (exchange);
```

Rows are kept after they settle so that cards stay right. Clones and partial files are deleted when a row settles or passes its deadline. A sweep runs at startup and hourly. Before naming the migration, the builder re-checks that 0067 is still free in `crates/agent/migrations` and in every `theuth-*` worktree.

### The placeholder the model sees

These lines come after `</message>`, in message order. Every attribute value is escaped with `loop_.rs` `attr` (`&`, `<`, `>`, `"`): `from`, `message`, `name`, `type`, `bytes`, `blake3`, `saved`, `machine`, `holder`, `file` and `failed`.

```
<message_file from="peer:maya" message="0193f0a2-6b1c-7d3e-9f40-1a2b3c4d5e6f" name="layout.png" type="image/png" bytes="864211" blake3="7c1e…(64 hex)" saved="/Users/sky/.theuth/files/received/maya/2026-11-17/layout.png" machine="Sky's MacBook Pro" holder="5f3a…(64 hex)" file="9d0c4b7e21a35f6880e1c2d3b4a59687"/>
<message_file from="peer:maya" message="0193f0a2-6b1c-7d3e-9f40-1a2b3c4d5e6f" name="drawings.pdf" type="application/pdf" bytes="24100000" failed="the sender's file changed"/>
Files are saved on disk, not shown to you; open one with read_file if you need it.
```

- `from` is the same address the `<message>` wrapper uses.
- `bytes` and `blake3` are the actual values, computed locally.
- `holder` is the receiving engine's endpoint id, and `file` is its local id.
- `name` is the sanitised declared name; the saved basename may add ` (2)`.
- The hint line is written once per row, exactly as shown.
- The desktop's `classify` and the TUI's `contact_row` parse the element as a self-closing note and drop the hint line. Sizes are shown in decimal units, as Finder shows them.

### Wire events and consumers

- `AgentEvent::ContactFileChanged { session, file: ContactFile }` is sent on each state change, and at most twice a second per file while bytes move. It rides the registered `AgentEvent` (`crates/agent/src/lib.rs:4364-4374`).
- `ContactFile` contains `{ id, direction: FileDirection, exchange, contact_id, contact, name, media_type, size?, blake3?, done, state: FileState, error?, path?, machine, holder, deadline }`. It is generated with ts-rs and lands in the OpenRPC and wire catalogs.
- `PeerDelivered` (`crates/agent/src/communication.rs:844-856`) gains `files: Vec<ContactFile>`, empty by default, so the live card matches the history card.
- **Desktop:** history in `store/reduce.ts` (`peerBody`/`peerEntry`, `:397-410`); live in its `PeerDelivered` case (`:801`); `ContactFileChanged` through `store/sessions.ts` `dispatch` (`:925-937`); the sending side in `SendCard`.
- **TUI:** history in `components/peers.rs` (`contact_row`, `:426`); live from `agent.peer-delivered`; the sending side in its generic tool entry (`model/agents.rs:404-412`).
- **Card actions:** no bytes travel by base64; `get_attachment` is unchanged.
  - The desktop shell on the holding machine opens with `openPath`, reveals with `revealItemInDir`, saves as with the dialog plugin plus `contact_file_export` (no-clobber copy), and copies the path.
  - The TUI uses `platform::open::open_path`, opens the folder to reveal, copies with `arboard`, and saves as through `contact_file_export` into `~/Downloads`.
  - Another of our machines shows "stored on <machine>" in P1. P2 adds **Fetch here**.
  - Phones and iPads follow D15.
- **Thumbnails:** `contact_file_thumbnail` reads at most 64 MB and runs `tools::images::fit(bytes, 512)`. It returns the actual MIME from `Fitted` (PNG or JPEG) and is owner-forwarded.

### Storage and sanitisation

Locations under `<root>` (D4):
- `files/received/<contact>/<YYYY-MM-DD>/<name>`
- `files/partial/<local-id>`
- `files/outgoing/<local-id>` (clones)
- `files/cache/<holder>/<id>/<name>` (P2)

Directories are 0700 and files 0600.

Sanitising, applied to the declared `filename` (P3: the last URL path segment; otherwise `file`) and to the contact's display name for the folder:
1. Keep only the text after the last `/` or `\`.
2. Drop control characters (Unicode Cc) and bidi controls: U+061C, U+200E, U+200F, U+202A–U+202E, U+2066–U+2069.
3. Replace each of `: * ? " < > |` with `_`.
4. Trim spaces and dots from both ends, repeatedly, until nothing changes. `. report .` becomes `report`, and `.zshrc` becomes `zshrc`.
5. An empty result becomes `file`. A Windows reserved stem (CON, PRN, AUX, NUL, COM1–9, LPT1–9) gets a `_` prefix.
6. Cap at 200 UTF-8 bytes at a character boundary. Keep the extension when it is 20 bytes or less, then re-apply step 4.

The name is not normalised. The filesystem decides equality (APFS ignores both normalisation and case), and no-clobber publication handles any clash.

Publication:
1. Stream into `files/partial/<local-id>` and `fsync`.
2. Check that the size equals the declared size (P3: within 5 GB). Compute BLAKE3 and compare it with the declared digest; P3 records it.
3. On macOS, set `com.apple.quarantine` on the partial file, read it back and compare. The value is `0081;<unix seconds, lowercase hex>;theuth;<UUID, uppercase>`.
   - The format is `flags;timestamp;agent;uuid`. 0x0081 = 0x0001 (quarantined download) + 0x0080, as non-sandboxed downloaders write it. It has neither 0x0002 (sandboxed creator) nor 0x0040 (user-approved), so Gatekeeper still checks the file at first open.
   - Apple does not document these bits. The reading follows Eclectic Light, "xattr: com.apple.quarantine, the quarantine flag" (2017-12-11, https://eclecticlight.co/2017/12/11/xattr-com-apple-quarantine-the-quarantine-flag/) and Mac Forensics, "Quarantine Events and Gatekeeper" (https://www.macforensics.app/en/blog/quarantine-events-and-gatekeeper).
   - No `QuarantineEventsV2` record is written.
4. `std::fs::hard_link(partial, target)`, which fails if the name exists, trying `stem (2).ext`, `stem (3).ext`, … in turn. Then unlink the partial.
5. Commit the row as `received` with the actual size, BLAKE3 and path.

A failure before step 4 removes the partial. A failure after step 4 unlinks the published file too. Nothing is ever opened or executed automatically.

### Protected set

These are theuth-owned secrets, keys and databases, each with its `-wal` and `-shm` siblings where it is SQLite:

| File, relative to the root | Source |
|---|---|
| `secrets.sqlite` | `crates/secrets/src/store.rs:14`, opened at `vault.rs:355` |
| `secrets.key` | `crates/secrets/src/key.rs:22` |
| `secrets.vault` (legacy) | `crates/secrets/src/vault.rs:52` |
| `mesh/identity` (iroh key; also signs rosters) | `crates/mesh/src/identity.rs:24`, `lib.rs:73`, `crates/a2a/src/contacts/members.rs:124` |
| `link/identity` (link X25519 key) | `crates/link/src/identity.rs:1`, `:30` |
| `token` (RPC bearer) | `crates/rpc-compose/src/bootstrap.rs:111` |
| `oauth/*.json`, `oauth/mcp/*` | `crates/oauth/src/lib.rs:426`, `:484-488` |
| `sessions.sqlite` | `crates/agent/src/supervisor.rs:11707` |
| `artifacts.sqlite` | `crates/artifact/src/store.rs:35` |
| `knowledge.sqlite` | `crates/knowledge/src/lib.rs:67` |

The set is built at each gate call, for this engine's runtime root and, when different, the default `~/.theuth`. It stats the listed files and walks `oauth/`, then matches the opened handle's `(dev, inode)`, so symlinks, hardlinks and case variants cannot evade it. Every other file, a received `research.sqlite` included, may be sent. The stated limit: the protected files of another custom data root on the same machine are not in the set.

## Failure modes

| Cause | Sender sees | Receiver sees | Cleaned up |
|---|---|---|---|
| Disk full (receiver) | Failed: "<contact>'s disk is full" (receipt) | Failed placeholder: "disk full on <machine>"; the text is still delivered | Partial |
| Connection loss mid-transfer | Sending, "waiting for <contact>" | Queued again; resumes with `Range` | Nothing until it ends |
| Slow-drip sender | Sending | That sender's files wait; nothing else does (D7) | At the deadline |
| Receiving engine restarts | Sending | Receiving restarts from 0%; the text stays withheld | `files/partial/` |
| Digest mismatch | Failed: "the file changed before it was fetched" | Failed placeholder; no retry | Partial; clone |
| Sender file changed or deleted | `410 changed` / `410 missing` → failed | Failed placeholder with that reason | Partial |
| Receiver offline at send | Tool error and failed card: "<contact> is offline" | Nothing | Offer row; clone |
| Owner machine offline (191) | Failed: "the chat's machine is offline" | Nothing | Offer row; clone |
| Forwarded to the owner (191) | Normal | Normal: the owner fetches from the holder named in the URI | — |
| Chat moved while files pending | Failed: "the chat moved; send again" | Nothing delivered (`HOLD_MOVED`) | Partials and already-received files of that exchange |
| Contact removed, blocked or re-membered | Active GETs cut; failed | Downloads cancelled; failed placeholders, or the intake fails if the route is gone | Partials |
| Holder offline mid-transfer | Card frozen on that machine | Queued; retries until the deadline | At the deadline |
| Old peer without the extension | Tool error before anything leaves; behind a new member: "their machine for that chat needs updating" | Nothing | Offer row |
| Offer past its deadline | Failed: "expired before <contact> fetched it" | Failed placeholder: "the offer expired" | Clone; partial |
| Non-public URL (P3) | — | Failed placeholder: "not a public HTTPS address". A static `http:` or userinfo URL refuses the message | Nothing written |
| Oversize | Tool error before anything leaves | Declared over 5 GB: message refused. Streamed past `size` or past 5 GB: failed placeholder | Partial |

## Phases

| Phase | Gate | Tests (minimum) |
|---|---|---|
| **P0 contract** | <ul><li>Types and enums: `FileState`, `FileDirection`, `IntakeState`, `ContactFile`, `PeerDelivered.files`.</li><li>RPC signatures.</li><li>`0067_contact_files.sql`.</li><li>`egress_open` and the protected set.</li><li>The sanitiser.</li><li>`blake3` as a direct dependency.</li><li>`read_file`'s preflight and bounded read (D14).</li><li>`EXT_FILES` defined, not advertised.</li><li>OpenRPC baseline accepted with Breaking 0.</li></ul> | <ul><li>Sanitiser table: traversal, `\`, bidi, controls, `. report .`, leading dot, reserved stems, 200-byte cut.</li><li>Publication against a real temporary directory: NFC and NFD forms of one name, `Report.pdf` beside `report.pdf`, an existing name. Each gets its own file and none is overwritten.</li><li>Protected set: a symlink, a hardlink and a case variant of `mesh/identity`, and a file swapped after the gate opened the original. A received `research.sqlite` passes.</li><li>The migration applies; neither table is in `ATTACHED`; `SESSIONS_WIRE_SCHEMA == 67`.</li><li>`read_file` refuses a 300 MB file, and a file grown past 256 MB during the read is cut at the cap.</li></ul> |
| **P1 contacts, both ways; desktop and TUI cards** | <ul><li>The extension advertised.</li><li>The file route.</li><li>The download queue.</li><li>Resume.</li><li>Receipts.</li><li>Durable intake.</li><li>The placeholder.</li><li>Quarantine.</li><li>Cards and thumbnails.</li><li>`contact_file_export`.</li><li>`~return` replies with files.</li></ul> | <ul><li>Two-engine e2e: image and PDF A→B, and B→A over `~return`.</li><li>ProtoJSON round-trip both ways; a part with `text` and `url` refused.</li><li>A 5,000,000,001-byte sparse file refused.</li><li>The exact placeholder, with every attribute escaped: a `"`/`<`/`&` in the name, media type, machine, sender and failure reason.</li><li>`<message_file` in a body neutralised.</li><li>No binary part reaches the provider.</li><li>Range resume.</li><li>A 416, a 200 answer to a Range request, a wrong `content-range` total and a short EOF.</li><li>A zero-length file.</li><li>A transfer longer than 60 s completes. A stalled one loses its slot and resumes; a slow-drip sender delays only its own files.</li><li>Stream-limit retry.</li><li>A reopen after the source is replaced gives `410 changed`. A grown original is served at its declared length only.</li><li>A declared expiry beyond `OFFER_TTL` is cut to the local deadline during a resume.</li><li>A contact blocked mid-transfer cuts both sides.</li><li>A URL host of another contact refused without dialling.</li><li>Restart with an `awaiting_files` intake: text withheld, files restart, then one delivery. Delivery runs twice: one row.</li><li>Drain mid-transfer: `HOLD_MOVED`, the sender's cards failed, files removed.</li><li>n1 forwards to owner n2, which fetches from the holder.</li><li>The quarantine xattr read back equals the spec.</li><li>UI: the `message_file` card, live equals history, four sender states, the TUI lines, the icon when a thumbnail fails.</li></ul> |
| **P2 human attach; access from another of our machines** | <ul><li>D15 attach on the owning machine.</li><li>New `Req::GetContactFile { id }` on `theuth/artifacts/1`, plus `contact_file_fetch(holder, id)`. The holder serves only a `received` row to a paired machine, in `receive_file`'s chunks (`crates/artifact/src/mesh.rs:306`), with a 60 s idle deadline in place of the 60 s per-ask `TIMEOUT` (`:35`).</li><li>The fetching machine writes `files/cache/<holder>/<id>/<name>` through the same no-clobber publication and verifies size and BLAKE3 from the placeholder, on fetch and on every cache hit. It never takes the existing cache-hit shortcut (`mesh.rs:395-420`).</li></ul> | <ul><li>A human answer with two files arrives.</li><li>A protected pick refused.</li><li>Attach on a non-owning machine shows "attach on <machine>".</li><li>Node 2 fetches a 1 GB file (over 60 s) from the holder after the chat moved to node 3.</li><li>Tampered bytes and a tampered cache entry refused.</li><li>An `out` or unknown id answers `Missing`.</li></ul> |
| **P3 foreign A2A and public HTTPS** | <ul><li>`raw` and `https` admitted.</li><li>Artifact file parts through `Retained::of`.</li><li>`web::fetch::download` (D16).</li><li>Inline `raw` toward peers without the extension.</li><li>`accepted_output_modes` adds `*/*`.</li><li>Text, status and artifact text bounds unchanged.</li></ul> | <ul><li>A `raw` part saved with its computed BLAKE3.</li><li>A public file with no `content-length` fetched.</li><li>A transfer longer than the old client's whole-request timeout completes.</li><li>`https://127.0.0.1`, a redirect to `http:`, a redirect to `10.0.0.1`, CGNAT and `169.254.169.254` refused.</li><li>Userinfo refused.</li><li>Over 5 GB by header and by stream.</li><li>A cut download restarts from zero.</li><li>An artifact file part in an answer.</li><li>A send too large for inline gives the error text.</li></ul> |

## Verification (the closer's live proof)

Two scratch engines A and B, paired as contacts over real iroh with no relay, fake models and scratch data dirs; node A2 joins A's mesh for P2. Record a log or screenshot for each step:

1. A's agent calls `send_message(to=B's contact, files=[a PNG, a 1 GB file])`. B's row shows the exact `<message_file>` lines. Each file exists at `saved`, mode 0600, with a matching BLAKE3, and `xattr -p com.apple.quarantine` matches the spec. The fake provider log shows no file bytes.
2. B answers with `reply_to` and a file, and A receives it.
3. Mid-1 GB: a cut stream resumes with `Range: bytes=N-`. Restarting B leaves `files/partial/` empty and the text withheld until delivery.
4. A refuses `mesh/identity`, a hardlink to it outside the data dir, and `SESSIONS.SQLITE` (a case variant). A received `research.sqlite` is forwarded.
5. Desktop and TUI captures: received image with a thumbnail; sending with progress; sent; failed (source deleted mid-transfer).
6. P2: A2 fetches and opens a file received on A. P3: a public HTTPS URL is fetched, and a loopback URL is refused.
