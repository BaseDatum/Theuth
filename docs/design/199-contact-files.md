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
| Who is the contact | `crates/a2a/src/config.rs:448-455`; `server.rs:970-979`, `:324-330` | `contact_by_eid` maps any member endpoint of a contact's 191 group to its alias. The gate admits only `IrohGate::Contact`. `admit_contact` charges a rate token and a concurrency permit. |
| Mesh forward | `crates/a2a/src/contacts/forward.rs:9-14`; `crates/agent/src/communication/mesh.rs:132`, `:135` | Only unary `SendMessage`, `GetTask` and `CancelTask` are carried to the owning machine: 10 s deadline, 2 MiB maximum. |
| Intake | `crates/agent/src/communication/network/full.rs:413-452`; `crates/agent/src/loop_.rs:9042-9069`, `:9140-9162` | A full message is enqueued as a mesh peer's message. The model sees `<message from id …>body</message>`. The body cannot contain any tag whose name begins with `message`. |
| Cards from tags | `crates/desktop/ui/src/lib/injected.ts:374`, `:393`; `store/reduce.ts:397-405`, `:477-480`; `crates/tui/src/components/peers.rs:426` | The desktop and the TUI turn engine-written tags into cards. Self-closing notes (`session_moved`, `engine_restart`, `contact_inbox`) are parsed already. |
| Wire schema | `crates/agent/src/store.rs:701-723`; `crates/agent/src/store/capture.rs:19-40`; `crates/agent/migrations/` | `SESSIONS_WIRE_SCHEMA = 67`. 197 set it as a value change, with no migration. The session log replicates only the `ATTACHED` tables. The highest migration is `0066_knowledge_identity.sql`. |
| Reusable pieces, and what is missing | `crates/web/src/fetch/ssrf.rs:30`, `:193`; `fetch/mod.rs:296-300`, `:892`; `crates/tools/src/egress.rs:1-9`; `crates/artifact/src/rpc_artifacts.rs:230-292`; `crates/artifact/src/mesh.rs:35`, `:39`, `:306`; `crates/tools/src/fs.rs:122`, `:134`; `crates/agent/src/loop_.rs:8965-8970`; `crates/theuth/src/main.rs:34`, `:1569-1572`; `crates/tools/src/resources.rs:34-40` | The SSRF guard is public-only unless the owner scopes LAN access, and it allows both `http` and `https`. `Fetcher::request` reads the body into memory, with a cap. `tools::egress` is a per-call LAN boolean, not a file gate. `get_attachment` returns a whole file as base64, from fixed folders only. `theuth/artifacts/1` streams in chunks, with one `Req` per stream, a 60 s deadline per peer and SHA-256-prefix names. `read_file` reads the whole file. `Input.files` turns each file into a binary part for the model. The data dir is `--data-dir` or `THEUTH_DATA_DIR`, else `~/.theuth`. `SessionResources.root` is the data dir, or a private chat's scratch root. `blake3` is only a transitive dependency (via iroh). No quarantine-xattr code exists. |
| Secret state | `crates/mesh/src/identity.rs:1-3`, `:24`; `crates/a2a/src/contacts/members.rs:124`; `crates/secrets/src/store.rs:14`; `crates/secrets/src/vault.rs:1797`; `crates/rpc-compose/src/bootstrap.rs:111`; `crates/oauth/src/lib.rs:426` | `mesh/identity` holds the engine's Ed25519 secret, which also signs roster chains. The vault is `secrets.sqlite` with `secrets.key`. The engine's RPC bearer is `token`. OAuth credentials live in `oauth/`. |

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

**D2. Bytes travel on their own stream of the contact connection, straight to the holder.** The receiver opens a new bidirectional stream on the authenticated contact connection (`Net::a2a_stream`, `crates/mesh/src/net.rs:851-870`) to the roster member named in the URI, which is the machine that holds the bytes. It sends `GET /a2a/files/<transfer-id>`. The fetch never goes through owner forwarding, which carries only unary `SendMessage`, `GetTask` and `CancelTask`, within 10 s and 2 MiB (`forward.rs:9-14`). On the serving side, `reach::serve` recognises the request from the request line it already reads (`reach.rs:154`), before `serve_http`. So the request is diverted before `Route::of`, `contact_route` and `forward::route` ever see it. Matching in `Route::of` instead would be too late: `Route::of` runs inside the hyper connection that `HTTP_TIMEOUT` wraps whole, response body included (`reach.rs:239-248`), so the stream would still be killed at 60 s. The file handler has a 60 s idle timeout and no total deadline. The client writes body frames into the partial file as they arrive; it never uses the 2 MiB `Limited` collect (`reach.rs:533-539`). Reason: the unary path cannot carry 5 GB. A second ALPN or a blob-ticket protocol would need its own gate. Riding the contact connection reuses the identity check that already admits the contact.

**D3. Intake waits for the message's files.** An admitted message that carries files is held until every file is received or has failed. It then enters the chat as one row through the ordinary peer intake (`enqueue_peer_chained`, `full.rs:450`): the `<message>` wrapper, then one placeholder per file. It never goes through `Input.files`, which would turn each file into a binary part for the model (`loop_.rs:8965-8970`). The A2A task completes on consumption, as today (`full.rs:1-8`). Until then, the receiving chat shows a live "receiving" card. Reason: one row and one notice. The model never answers text whose files are still missing, and no second, late notice has to wake a turn.

**D4. Storage layout and the filename sanitiser.** `<root>` is the receiving session's `SessionResources.root` (`crates/tools/src/resources.rs:34-40`). For an ordinary chat that is the runtime data dir: `--data-dir` or `THEUTH_DATA_DIR`, else `~/.theuth` (`crates/theuth/src/main.rs:34`, `:1569-1572`). For a private chat it is the scratch root, and the files die with the chat. Nothing hardcodes `~/.theuth`. Locations:

- received files: `<root>/files/received/<contact>/<YYYY-MM-DD>/<name>`;
- partial downloads: `<root>/files/partial/<transfer-id>`;
- sender clones: `<root>/files/outgoing/<transfer-id>`.

Directories are 0700 and files 0600. The sanitising rules are fixed in the Contract. The macOS quarantine is new code: one `setxattr` call. Reason: one predictable folder per contact per day is easy to find in Finder. Names sanitised on our side cannot traverse paths, hide, or spoof their direction.

**D5. The placeholder notice; bytes are never auto-injected.** Each file becomes one engine-written `<message_file …/>` element after `</message>`, plus one fixed hint line. The element names the sender, name, type, size, saved path and machine, or the failure. The tag name begins with `message`, so an existing rule (`neutralise_delimiters`, `loop_.rs:9140`) already stops a contact's body from forging one. `Raw` bytes are never projected as model input either. The promise is "never auto-injected", not "unreadable". The model may later call `read_file` on the saved path. `read_file` then shows the image or converts the document, as it does for any file (`crates/tools/src/fs.rs:122`). Reason: this is Sky's ruling. Reusing the wrapper's neutralisation makes the notice unforgeable without new escaping code.

**D6. The egress gate.** Every outgoing file passes the new `communication::files::egress_check(path)`: the `send_message` tool, the human answer forms from P2, and inline `Raw` sends from P3. It is new code; `tools::egress` is a per-call LAN boolean, not a file gate (`crates/tools/src/egress.rs:1-9`). The gate resolves the path (symlinks, `..`) and requires a regular file of at most 5 GB. It refuses the deny-list in the Contract. Nothing else is checked, and there is no approval card. Reason: Q1 and Q6 together ask for one choke point that lets trusted shares send freely but never exports theuth's own keys, vault or databases. The gate is not a sandbox: a full share's shell can still copy a file elsewhere first. That is 190's trust model, unchanged.

**D7. The 5 GB cap is the only limit.** No file may exceed 5,000,000,000 bytes. The cap is checked in four places: by the sender's gate, by the receiver on the declared `size`, by the receiver on the bytes actually streamed, and on a P3 download's `content-length` and its stream. There is no type check, count limit, rate limit or quota. The existing 64 KiB envelope still bounds how many references fit in one message, about 150. Transfers queue: at most 2 downloads per sending member and 4 per engine run at once. Reason: Q2, Q3 and Q5 rule this. Queueing keeps each receiver within the sender's 8 handlers per peer (`server.rs:58`, `:324-330`) and the 16 streams per remote (`net.rs:52`), so a transfer never has to be refused.

**D8. Resume, restart and expiry.** After a dropped stream, the receiver resumes with `Range: bytes=<have>-`. It backs off from 5 s, doubling up to 10 min, until the offer expires. When the receiving engine restarts, it deletes `files/partial/` and starts its unfinished downloads again from byte 0. Offers persist in SQLite and expire 7 days after sending (`OFFER_TTL`). Reason: interpretation 5 asks for Range resume and interpretation 4 for removing partials on restart. This plan does both. Seven days covers a contact who is away for a long weekend.

**D9. Source and outcome.** At send time, the sender hashes each file with BLAKE3. `blake3` 1.8.7 is already built through iroh (`Cargo.lock`); `crates/agent` names it directly at that version. The sender then tries a copy-on-write clone into `files/outgoing/` (`clonefile(2)` on APFS). If cloning fails (another volume, or no copy-on-write support), it serves the original. When the receiver finishes, it reports the outcome with `POST /a2a/files/<transfer-id>`, and the sender's card moves to sent or failed. Reason: on a Mac, a clone makes "file edited after sending" a non-event. Elsewhere, the digest check catches it, as interpretation 7 says. An explicit receipt is a single small request. The A2A task follower stops after `FOLLOW_MAX` = 6 h (`client.rs:65`), so it cannot report on a 7-day offer.

**D10. Mesh behaviour; no replicated table.** The transfer table is node-local. The file lands on the engine that owns the receiving chat. A message taken by another member reaches the owner through `a2a_forward`; the references are small and fit its 64 KiB request. The owner then dials the sending member named in each URI directly (D2), so bytes never cross owner forwarding. The placeholder sits in `messages.body`, so it replicates with the transcript. P1 cards on other machines say "stored on <machine>". P2 fetches the bytes over `theuth/artifacts/1`. Reason: 191 already routes ownership. A node-local table needs no quorum and moves no wire number.

**D11. Typed cards.** The desktop and the TUI render `<message_file>` elements and sender-side states as file cards, never as raw tags. The desktop renders an image thumbnail from the stored file. Card actions are open, reveal, save as and copy path. Reason: this keeps the draft's D8. Model-facing tags must have a matching UI entry in the same phase.

**D12. Foreign peers and public HTTPS (P3).** Accepted from foreign peers: inline `raw` parts within the unchanged 64 KiB envelope, `https` URL parts, and the same parts in remote task artifacts. `Fetcher::request` buffers the body (`fetch/mod.rs:892`), so URL parts are fetched by a new `web::fetch::download`, which writes body frames to the partial file as they arrive. It always uses the public-only `GuardedResolver`, never the LAN twin, whatever `tools::egress` allows the turn (`crates/tools/src/egress.rs:1-9`). The guard alone would allow `http:`, so the download requires `https` on the initial URL and on every redirect hop. It re-checks every hop with the guard, and refuses userinfo. It sends no cookies or credentials, applies the 60 s idle timeout and the 5 GB cap, and resumes with `Range` when the server sends `Accept-Ranges: bytes`. Toward a peer without the files extension, a file goes inline as `raw` only if the serialized request still fits 64 KiB. Anything larger fails, and nothing is uploaded publicly. Reason: interpretation 11, using the guard the web crate already has (`ssrf.rs:1-15`).

**D13. Both directions.** A contact's agent answers a full-share message with `send_message(reply_to, files)`, which goes to the sender's `~return` route (`full.rs:1-8`). It uses the same parts, file route and intake as the first message, so replies with files are P1. The human answer forms (P2) use `~return` too. A conventional A2A peer answers in `Task.artifacts`. Its file parts are taken by `Retained::of` and the stream-update path (`client.rs:973-981`, `:1067-1068`) into the same storage and placeholder, which is P3. Reason: interpretation 8 asks for both directions from P1. The two answer paths are different code, so both are named.

## Contract

### `send_message` `files`

```rust
/// Contacts only: absolute paths of local files to send with this message, any type, each
/// at most 5 GB. The receiver gets each file saved on its disk, not put into its context.
#[serde(default)]
pub files: Vec<PathBuf>,
```

- `message` may be empty when `files` is not empty. The text part is then omitted.
- The `request_id` key covers the ordered `(blake3, size, name)` list. The same key with different files is refused: `request_id <id> was used with different files`.
- The call returns once the A2A message is accepted. Transfers continue in the background. `structured` gains `files: [{id, to, name, size, media_type, blake3}]`. The text result adds `N files offered (name size, …); offers expire <date>`.
- With several contact recipients, each recipient gets its own offer, and the file is hashed once.
- Errors, as `ToolOutput::err`. Nothing is sent for any of them:

| Cause | Text |
|---|---|
| Path is not absolute | `files: <path> is not an absolute path` |
| Missing, a directory, or unreadable | `files: <path>: no such file` · `files: <path>: not a regular file` · `files: <path>: <io error>` |
| Over the cap | `files: <path> is <size> (over the 5 GB limit per file)` |
| Deny-list | `files: <path> is theuth's own secret state and is never sent` |
| A recipient is not a contact | `files can go to contacts only; <addr> is not a contact` |
| No files extension on the contact's card (P1, P2) | `<contact> cannot receive files: their theuth does not support them yet` |
| P3 foreign peer, file does not fit inline | `<peer> takes files only inside the message, and <name> (<size>) does not fit in 64 KiB` |
| Envelope full | `too many files for one message (over 64 KiB); send them in several messages` |

### A2A parts

A file reference part (P1):

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

- Composition: an optional text part first, then one part per file in `files` order. `"extensions": ["https://theuth.io/a2a/ext/files/v1"]` appears whenever a reference part is present. The contact wire is ProtoJSON. The proto `Part` uses the same JSON names (`text`, `raw`, `url`, `filename`, `mediaType`, `metadata`). Tests must round-trip through `a2a_pb::protojson_conv`, not only through serde.
- The sender sets `mediaType` with `mime_guess` from the extension, falling back to `application/octet-stream`. It is descriptive only, and the receiver never enforces it.
- Receiver admission (`validate_send` for shape, the contact handler for the roster check) refuses the whole message with `invalid_params` naming the part when:
  - the scheme is not `iroh`;
  - the path is not exactly `/a2a/files/<32 lowercase hex>`;
  - the host is not a current member of the same contact (`contact_by_eid(host) == caller's alias`);
  - `size` is above 5,000,000,000;
  - `blake3` is not 64 lowercase hex;
  - `expires` does not parse.

  `data` parts stay `content_type_not_supported`. `Message.extensions` is accepted when it is exactly `[EXT_FILES]`; any other value stays `EXTENSION_SUPPORT_REQUIRED`. Each part object must carry exactly one of `text`, `raw`, `url` or `data`. Two content fields mean `invalid_params`. The check runs in the HTTP layer that already bounds the body (`server.rs:50`), before the SDK router decodes the request. That way neither the SDK's first-match serde nor the proto oneof decides which field wins. In P3 the same check admits `raw` and `https` URL parts.

### Agent Card extension

```json
{"uri": "https://theuth.io/a2a/ext/files/v1", "required": false, "params": {"maxBytes": 5000000000}}
```

`EXT_FILES` sits beside `EXT_FULL` in `crates/a2a/src/contacts.rs` and is advertised from P1. The sender checks the contact's last fetched card. If a member advertises the extension but the owner behind it does not, the owner refuses (D1). That is the "old peer" row in the failure table.

### The file route on `theuth/a2a/1`

New module `crates/a2a/src/contacts/files.rs`, with `FILES_PATH = "/a2a/files/"`.

```
GET /a2a/files/<transfer-id> HTTP/1.1          (optional) Range: bytes=<from>-
→ 200  content-type: application/octet-stream, content-length: <size>, accept-ranges: bytes, then the bytes
→ 206  content-range: bytes <from>-<size-1>/<size>, then the rest
→ 404  unknown id, or an offer made to another contact (one answer for both)
→ 410  text/plain "expired" | "missing" (offer past OFFER_TTL, or the source is gone)
→ 416  bad range · 401 not a contact (gate) · 429 rate (admit_contact)

POST /a2a/files/<transfer-id> HTTP/1.1         {"state":"received"} | {"state":"failed","reason":"<≤200 bytes>"}
→ 204 · 404 unknown · 413 body over 1 KiB
```

- Auth uses roster membership. The caller's endpoint must be `IrohGate::Contact`, and its alias must equal the alias the offer was made to. Any current member of that contact's 191 group qualifies, because the chat's owner may be a different member from the one that took the message.
- The receiver only dials an endpoint that `contact_by_eid` maps to the alias the message came from. A URL can never introduce a new endpoint.
- Each request is admitted by `admit_contact` and holds its permit for the stream.
- Headers must arrive within `READ_TIMEOUT` (30 s).
- Body: `FILE_IDLE = 60 s` with no bytes moved closes the stream. There is no total deadline.
- The receiver retries a `POST` until it gets `204` or `404`, or until the offer expires.

### Migration `0067_contact_files.sql`

The table is node-local and not in `ATTACHED`, so the session log never carries it. `SESSIONS_WIRE_SCHEMA` stays 67. Had 199 needed a replicated column (say `messages.files`), migration 0067 would have to set the wire value to 68, because 67 is already used (`store.rs:716-722`). A record-replica table would need no bump. The draft's rule ("only a column change moves the schema") was wrong. The number moves when a migration changes a replicated column, *or* when a replicated value changes in a way an older engine would misapply. 197's 67 is such a value change. 199 makes neither kind of change. The only thing that must replicate is the placeholder, and it is ordinary text in `messages.body`. An older desktop shows that text unparsed inside the peer card (`reduce.ts:397-403`). That only affects display: an older engine that takes the row feeds its model the same text, and nothing removed is replayed.

```sql
-- 199 CONTACT-FILES: files offered to and fetched from contacts. Node-local: not attached
-- to the session log, so SESSIONS_WIRE_SCHEMA is unchanged (67).
CREATE TABLE contact_files (
    id         TEXT PRIMARY KEY,   -- transfer id, 32 lowercase hex (random)
    direction  TEXT NOT NULL,      -- 'out' | 'in'
    session    TEXT NOT NULL,      -- the local chat
    message    TEXT NOT NULL,      -- the exchange id (A2A messageId)
    seq        INTEGER NOT NULL,   -- order within the message
    contact    TEXT NOT NULL,      -- contact alias (its 191 group)
    name       TEXT NOT NULL,      -- sanitised display name
    media_type TEXT NOT NULL,
    size       INTEGER NOT NULL,
    blake3     TEXT,               -- NULL only for P3 foreign files
    source     TEXT NOT NULL,      -- out: path served (clone or original); in: the URL
    path       TEXT,               -- in: saved path once received
    done       INTEGER NOT NULL DEFAULT 0,  -- bytes served / written
    state      TEXT NOT NULL,      -- queued | sending | sent | receiving | received | failed
    error      TEXT,
    created_ms INTEGER NOT NULL,
    expires_ms INTEGER NOT NULL
);
CREATE INDEX contact_files_session ON contact_files (session);
```

Rows are kept after they settle (about 300 bytes each) so that cards stay right. Clones and partial files are deleted when a row settles or expires. An hourly sweep, which also runs at startup, fails expired rows. Before naming the migration, the builder re-checks that 0067 is still free in `crates/agent/migrations` and in every `theuth-*` worktree.

### The placeholder the model sees

These lines come after `</message>`, in message order. Every value is attribute-escaped (`loop_.rs` `attr`):

```
<message_file from="peer:maya" message="0193f0a2-6b1c-7d3e-9f40-1a2b3c4d5e6f" name="layout.png" type="image/png" size="864 KB" saved="/Users/sky/.theuth/files/received/maya/2026-11-17/layout.png" machine="Sky's MacBook Pro"/>
<message_file from="peer:maya" message="0193f0a2-6b1c-7d3e-9f40-1a2b3c4d5e6f" name="drawings.pdf" type="application/pdf" size="24.1 MB" failed="the sender's copy changed before it was fetched"/>
Files are saved on disk, not shown to you; open one with read_file if you need it.
```

- `from` is the same address the `<message>` wrapper uses.
- `size` is decimal, as Finder shows it: KB, MB or GB with one decimal under 10.
- `name` is the sanitised declared name. The saved basename may add ` (2)`.
- The hint line is written once per row, exactly as shown.
- The desktop's `classify` and the TUI's `contact_row` parse the element as a self-closing note and drop the hint line.

### Wire events and RPC

- `AgentEvent::ContactFileChanged { session, file: ContactFile }` is sent on every state change, and at most twice a second per file while bytes move. It rides the already registered `AgentEvent` (`crates/agent/src/lib.rs:4364-4374`). The desktop handles it in `store/sessions.ts` `dispatch` and the TUI in its agent-event model. A Rust event alone renders nothing.
- `ContactFile` contains `{ id, direction, message, contact, name, media_type, size, blake3?, done, state, error?, path?, machine, expires_at }`. It is generated with ts-rs and lands in the OpenRPC and wire catalogs in the same change.
- `PeerDelivered` (`crates/agent/src/communication.rs:844-856`) gains `files: Vec<ContactFile>`, empty by default. Its `text` is the body without the elements, so the live card would otherwise lack the files that the history card shows.
- `contact_files(session) -> Vec<ContactFile>` is owner-forwarded and lists that session's rows, for reloads and for other machines.
- The receiving card, while the message is held (D3), comes from the `in` rows. Once the row commits, the `<message_file>` elements in history replace it.
- On the sending side, the desktop's `SendCard` reads `files` from the call's arguments and `structured`, and the live state from the event or the RPC. In the TUI, a send is a generic tool entry (`crates/tui/src/model/agents.rs:404-412`), which gains the same file lines.
- Consumers: the desktop reads the history card in `store/reduce.ts` (`peerBody`/`peerEntry`, `:397-410`) and the live card in its `PeerDelivered` case (`:801`). `ContactFileChanged` arrives through `store/sessions.ts` `dispatch` (`:925-937`). The TUI reads the history card in `components/peers.rs` (`contact_row`, `:426`) and the live card from the `agent.peer-delivered` topic.
- No client gets file bytes through `get_attachment`, which returns the whole file as base64 (`rpc_artifacts.rs:230-292`); its allowed folders stay as they are. Card actions, by where the UI runs:
  - The desktop shell on the machine that holds the file (`isLocal`, `lib/files.ts:64-65`):
    - open (`openPath`);
    - reveal in Finder (`revealItemInDir`);
    - save as: the dialog plugin picks a destination, and the engine copies the file there with the new `contact_file_export(session, id, dest)`, like `export_artifact`;
    - copy path.

    No bytes cross RPC.
  - The TUI: the same actions, using `platform::open::open_path`, opening the folder to reveal, and `arboard` to copy. Save as goes to `~/Downloads/<name>` through `contact_file_export`.
  - A desktop on another of our machines: "stored on <machine>" and copy path in P1. P2 adds **Fetch here**, which calls `contact_file_fetch(session, id)` on that machine's own engine (P2 row). After the fetch, the local actions work.
  - Phone and iPad, which control an engine over RPC (`00c518ce7`): name, size, state, thumbnail and copy path. Opening or saving the bytes on the device is not in 199 (Flag 1).
- Thumbnails: the new `contact_file_thumbnail(session, id) -> base64 PNG` is owner-forwarded. It returns the stored file fitted to a 512 px longest edge by `tools::images::fit` (`crates/tools/src/images.rs:118`), and only when the file decodes as PNG, JPEG, GIF or WebP. Every client uses it except the local desktop shell, which loads `asset://` (scope: the data dir). Otherwise the card shows the file icon. *ponytail:* `fit` holds the whole file in memory, so images over 64 MB get the icon instead of a thumbnail. Switch to a streaming decoder if that matters.

### Storage and sanitisation

Applied to the declared `filename` (P3: the last URL path segment; otherwise `file`) and to the contact folder name:

1. Keep only the text after the last `/` or `\`.
2. Drop control characters (Unicode Cc) and bidi controls: U+061C, U+200E, U+200F, U+202A–U+202E, U+2066–U+2069.
3. Replace each of `: * ? " < > |` with `_`.
4. Trim whitespace. Strip leading and trailing `.` (`.zshrc` becomes `zshrc`, and `..` becomes empty).
5. An empty result becomes `file`. A Windows reserved stem (CON, PRN, AUX, NUL, COM1–9, LPT1–9) gets a `_` prefix.
6. Cap at 200 UTF-8 bytes, cut at a character boundary. Keep the extension when it is 20 bytes or less.
7. Avoid collisions by trying `stem (2).ext`, `stem (3).ext`, … with `std::fs::hard_link(partial, candidate)`, which fails if the name exists. Then remove the partial. The partial and the target are on the same volume.

Publish order:

1. Stream to the partial file.
2. Check that the length equals `size`.
3. BLAKE3 the whole file and compare.
4. Link into place.
5. On macOS, set `com.apple.quarantine`. Use the flags a browser download gets, `;<hex unix time>;theuth;<uuid>`; check the flags with `xattr -p` on a real download.
6. Mark the row received.

A failure at any step removes the partial and fails the file. Nothing is ever opened or executed automatically.

### Egress deny-list

The path is resolved (`std::fs::canonicalize`) and compared with the canonical `<data_dir>`. A file is refused when, inside `<data_dir>`, it is:

- anything under `mesh/` (`mesh/identity`, the iroh key that also signs roster chains, and `mesh/secrets.*`);
- a top-level `secrets.*` file (`secrets.sqlite`, its `-wal`/`-shm`, `secrets.key`, `secrets.vault`, `secrets.vault.migrated`);
- the top-level `token` file;
- anything under `oauth/`;
- at any depth, a file ending in `.sqlite`, `.sqlite-wal`, `.sqlite-shm`, `.sqlite-journal`, `.db`, `.db-wal`, `.db-shm` or `.db-journal` (`sessions.sqlite`, `agent.sqlite`, `mesh.sqlite`, `replica.sqlite`, `roster.sqlite`, `voice.db`, …).

Nothing else is refused, and received files may be forwarded.

## Failure modes

| Cause | Sender sees | Receiver sees | Cleaned up |
|---|---|---|---|
| Disk full (receiver) | Failed: "<contact>'s disk is full" (from the `POST`) | Failed placeholder: "disk full on <machine>"; the message is still delivered | Partial deleted |
| Connection lost mid-transfer | Sending, "waiting for <contact>" | Receiving, "reconnecting"; resumes with `Range` | Nothing until it succeeds or expires |
| Receiving engine restarts | Sending; it continues | Receiving restarts from 0% | `files/partial/` emptied at startup |
| Digest mismatch | Failed: "the file changed before it was fetched" | Failed placeholder (same reason); no retry | Partial deleted; clone deleted |
| Sender file changed or deleted | Changed: as digest mismatch. Deleted: failed "the file is gone" (`410 missing`) | Failed placeholder with that reason | Partial deleted; row settles |
| Receiver offline at send | Tool error and failed card: "<contact> is offline" (as text sends today) | Nothing | Offer row failed; clone deleted |
| Owner machine offline (191) | Failed: "the chat's machine is offline" (`ContactRefusal::OwnerOffline`; nothing queued) | Nothing | Offer row failed; clone deleted |
| Message forwarded to the owner (191) | Normal | Normal: the owner fetches from the sending member directly | — |
| Sending member offline mid-transfer | That machine's card is frozen; others say "stored on <machine>" | Receiving, "waiting for <contact>"; retries until expiry | On expiry |
| Old peer without the extension | P1: tool error before anything leaves. Behind a new member: failed "their machine for that chat needs updating" | Nothing | Nothing created, or the offer row failed |
| Expired offer | Failed: "expired before <contact> fetched it" (sweep) | Failed placeholder: "the offer expired" (`410 expired`) | Clone and partial deleted |
| Non-public URL (P3) | — (foreign sender) | Failed placeholder: "not a public HTTPS address". A static `http:` or userinfo URL refuses the whole message | Nothing written |
| Oversize file | Tool error before anything leaves | Declared over 5 GB: whole message refused. Streamed past `size`, or past 5 GB in P3: failed placeholder | Partial deleted |

## Phases

| Phase | Gate | Tests (minimum) |
|---|---|---|
| **P0 contract** | Types (`SendMessageArgs.files`, `ContactFile`, `ContactFileChanged`, `PeerDelivered.files`, `contact_files`, `contact_file_thumbnail`, `contact_file_export`), `0067_contact_files.sql`, `egress_check` and the sanitiser as pure functions. `blake3` becomes a direct dependency of `crates/agent` at 1.8.7. `EXT_FILES` defined but not advertised. OpenRPC baseline accepted with Breaking 0. | Sanitiser table (traversal, `\`, bidi, controls, leading dot, reserved stems, 200-byte cut keeping the extension, collision suffix). Deny-list, including a symlink to `mesh/identity` and a `..` path. The migration applies, `contact_files` is not in `ATTACHED`, and `SESSIONS_WIRE_SCHEMA == 67`. |
| **P1 files both ways between theuth contacts; desktop and TUI cards** | The extension is advertised, plus the file route, streaming downloads, resume, receipts, the placeholder, quarantine, `PeerDelivered.files`, the cards, the thumbnail and export RPCs, "stored on <machine>", and `~return` replies with files. | Two-engine e2e (`contacts/e2e_tests`): image and PDF A→B, and B→A with `reply_to` over `~return`. File parts round-trip through `a2a_pb::protojson_conv` in both directions, and a part with both `text` and `url` is refused. A 5,000,000,001-byte sparse file is refused before the network. The exact placeholder text. `<message_file` in a contact body is neutralised. A file-bearing message never reaches `Input.files` (the provider request carries no binary part). A cut stream resumes with `Range`. A transfer longer than 60 s completes; a stalled one hits the idle timeout. A source edited on a volume without clones gives a mismatch on both sides. A deleted source gives `410`. A short-TTL expiry. A URL naming a non-member endpoint is refused without dialling. Another contact's id gets `404`. A restart deletes partials and restarts. n1 forwards to owner n2, and n2 fetches from the sending member directly. A card without the extension is refused. UI: `classify` parses `message_file`, the live and history cards match, SendCard shows all four states, and the TUI renders the lines. |
| **P2 human attach; access from another of our machines** | File chips in **Answer as this chat** and the Messaging sheet answer field, plus a TUI attach on the answer prompt. All of them go through `egress_check` on the chat's machine. New `Req::GetContactFile { session, id }` on `theuth/artifacts/1`, and `contact_file_fetch(session, id)` to trigger it:<br>• It serves only a received (`in`) row's saved file, to a paired machine.<br>• It reuses `receive_file`'s chunked pattern (`crates/artifact/src/mesh.rs:306`). The bytes are verified by the row's BLAKE3 and size, not a SHA-256 prefix.<br>• A 60 s idle deadline replaces the 60 s per-ask `TIMEOUT` (`:35`).<br>• The file is cached at the same relative path under the fetching machine's data dir. | A human answer with two files arrives with placeholders. A deny-listed pick is refused in the form. Node 2 fetches a 1 GB file received on node 1, which takes longer than 60 s, verifies it, and then opens it locally. Tampered bytes are refused. An unknown id or a sent (`out`) row answers `Missing`. |
| **P3 foreign A2A and public HTTPS** | `validate_send` admits `raw` and `https` parts. `Retained::of` and stream updates take file parts from artifacts into the same storage and placeholder. `web::fetch::download`. Inline `raw` toward peers without the extension. `accepted_output_modes` adds `*/*`. Text, status and artifact text bounds unchanged. | A foreign `raw` part is saved. A public HTTPS file is fetched. `https://127.0.0.1/…`, a redirect to `10.0.0.1`, CGNAT and `169.254.169.254` are refused. `http:` and userinfo are refused at admission. Over 5 GB by header and by stream. `Range` resume when supported. An artifact file part in an answer. Sending too large for inline gives the error text. |

## Verification (the closer's live proof)

Use two scratch engines A and B, paired as contacts over real iroh with no relay, fake models, and scratch data dirs. Plus node A2 in A's mesh for P2. Record a log or screenshot for each step:

1. A's agent calls `send_message(to=B's contact, files=[a PNG, a 1 GB file])`. B's chat row shows the exact `<message_file>` lines and the hint line. Each file exists at `saved` with a matching BLAKE3, mode 0600, and a `com.apple.quarantine` value (`xattr -p`). The fake provider's request log shows no file bytes.
2. B answers with `reply_to` and a file, and A receives it the same way.
3. The 1 GB transfer, interrupted mid-stream, resumes with a `Range: bytes=N-` log line. B restarted mid-transfer leaves `files/partial/` empty and restarts at 0%.
4. Sending `<data_dir>/mesh/identity`, a symlink to it, and `sessions.sqlite` is refused. A 5,000,000,001-byte sparse file is refused.
5. Desktop screenshots: received image card with thumbnail; sending with progress; sent; failed (source deleted mid-transfer). Plus a TUI capture of the same.
6. P2: A2 opens the file received on A. P3: a public HTTPS URL is fetched, and a loopback URL is refused.

## Flags for Sky

1. **Phone and iPad send and save, and attaching from a machine that doesn't own the chat.** The bytes must start where an engine can serve them, and a phone has no engine. Today's upload path is type- and size-capped (`rpc_attachments.rs:19-25`), and `get_attachment` returns a whole file as base64. *Recommendation:* in 199, human attach works on the machine that owns the chat; elsewhere the attach control says "attach on <machine>". Phones and iPads show cards, thumbnails and paths but don't open or save the bytes. A chunked upload and download RPC can come later if Sky wants to use files from the phone.
2. **`read_file` reads a whole file into memory (`crates/tools/src/fs.rs:134`).** A model that opens a 4 GB received video with it would allocate 4 GB. *Recommendation:* in P1, `read_file` checks the size first. Above 256 MB it answers "too large to read whole (<size>); use shell tools such as head or ffprobe". This is a reading limit, not a transfer limit.
