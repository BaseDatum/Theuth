# 09 – Signal: a phone-side frontend for the daemon

Status: as-built v0 · 2026-09-10 · extends `00-architecture.md` §4, §7;
`01-config-and-subagents.md` §1.1; `05-oauth.md` (login-flow shape);
`07-context.md` §5 (approvals), §8.6 (compact).

Signal is a frontend, like the terminal UI: it sends `SendInput`/`Steer`/
`Cancel`/`Resolve` and renders `AgentEvent`s. It runs **only in the daemon**
(`t --headless`). It is also the only frontend whose transport code is
*imported*, not written: the presage layer jarvis spent weeks debugging is
copied byte-for-byte and never edited beyond three additive hunks (§2.2).

## 1. Goal, and what is not one

Goal: text the daemon from a phone and get the same agent the terminal gets —
turns, steering, approvals, slash commands, session switching — with Signal's
own affordances (reactions, typing, receipts, quotes) used the way Signal
users expect. Survive restarts, disconnects and re-linking without a human.

Not goals, with the reason:

- **Groups.** presage drops `group_v2` (jarvis `presage.rs:892`); one contact
  = one thread keeps the trust model a list of numbers. Revisit with a spike.
- **Voice/STT/TTS, images to the model.** theuth's `SendInput` is text. Inbound
  files are saved and their path is handed to the model (§7.4), which can
  `read_file` them; that covers the useful 80 %.
- **Streaming to the phone.** Signal cannot edit a sent message; a burst of
  fragments is worse than one reply. Typing indicator carries the "alive"
  signal; the final text is sent once (§8.1).
- **Running in TUI mode.** One presage store is one linked device with one
  websocket; two processes on it corrupt each other. The terminal process
  never opens the store except to link: the one-shot CLI (§11.1) or the
  in-process `signal_link` tool (§11.3).

Deployment fact, stated once: theuth links as a *secondary device* of a Signal
account, so that account needs a primary phone. In the usual setup the daemon
owns a **dedicated number** and the owner messages it from their personal
account. Messaging your own account ("Note to Self") arrives as a sync
message, which the frozen transport does not surface — a spike in §16.

## 2. Layout

```
crates/signal/
  Cargo.toml
  src/
    lib.rs        plugin, `signal` actor, tools, `link_oneshot`, QR  (new, ~610 lines incl. tests)
    channel.rs    link/connect supervisor, inbound, outbound        (new, ~1720 lines incl. tests; control flow and constants ported from jarvis channel.rs)
    commands.rs   slash parser + dispatcher for the phone           (new, ~750 lines incl. tests)
    approvals.rs  codes, message text, reaction/answer routing      (new, ~390 lines incl. tests)
    transport.rs  FROZEN  jarvis crates/jarvis-signal/src/transport.rs
    presage.rs    FROZEN  jarvis crates/jarvis-signal/src/presage.rs
    text.rs       FROZEN  jarvis crates/jarvis-signal/src/text.rs
    error.rs      FROZEN  jarvis error.rs minus the `Core` variant and its `From`
    mock.rs       FROZEN  the `MockTransport` half of jarvis mock.rs (`#[cfg(test)]`)
~/.theuth/signal/
  presage.db      presage's own sqlite (protocol state, identity keys, contacts); schema is presage's
  threads.json    { "+15551234567": { "session": "<uuid>", "aci": "<uuid>" }, … }  contact → live session + ACI (§6)
  inbox/          inbound attachments, `<timestamp>-<name>` (§7.4)
```

### 2.1 Provenance

Frozen files are copied from jarvis commit `9d74d5a7c924dde83268ca41689d96cedc942042`
(presage.rs last touched 2026-07-16). Each carries a two-line header:

```rust
// FROZEN: copied from jarvis 9d74d5a crates/jarvis-signal/src/presage.rs.
// Protocol code; edit only with a matching note in docs/design/09-signal.md §2.2.
```

Their unit tests come with them (`presage.rs` tests need `tempfile`).
`cargo test -p signal` therefore proves the copy still compiles against the
pinned presage rev and behaves as it did in jarvis.

### 2.2 The four additive edits

The first two are needed for reaction-based approvals (§10) and nothing else;
the third for `message_user` (24 §6); the fourth is test-only. All add; none
changes an existing branch.

1. **Surface inbound reactions.** `content_to_envelope` (jarvis `presage.rs:896`)
   returns `None` for a `DataMessage` with no body and no attachments. Before
   that check, one arm: if `data.reaction` is `Some`, return an envelope with
   `text: None` and the new field
   `reaction: Some(IncomingReaction { emoji, target_timestamp, remove })`.
   `SignalEnvelope` gains that `Option` field (default `None`); every existing
   constructor site sets it once.
2. **`send_text` returns its timestamp.** `SignalTransport::send_text` becomes
   `-> Result<u64>`; presage's implementation already computes
   `now_timestamp_millis()` (`presage.rs:602`) and returns it. The mock echoes
   its own clock. A reaction targets `(author, sent_timestamp)`; without this
   the channel cannot tell which of its own messages was reacted to.
3. **`send_attachment` takes a `file_name: Option<&str>`** (before `caption`;
   trait, presage, mock, the presage unit test). presage's `AttachmentSpec`
   already has the field; jarvis never sent files and hard-coded `None`. A
   nameless PDF is unusable on the phone, so the theuth outbound path (§8.4)
   passes the file's name through. Images and audio ignore it.
4. **Mock records the full `Recipient`.** `MockTransport` keeps a
   `recipients: Vec<Recipient>` parallel to `sent` (whose items carry only the
   number), read via `sent_recipients()`. Only so `channel` tests can assert
   that outbound sends keep the inbound ACI (§8.5). `mock.rs` only.

Everything else that touches the wire — linking, receive pump, resolution,
CDSI/PNI handling, contact-sync requests, `OnNewIdentity::Trust`, the
dedicated multi-thread runtime + `LocalSet` (the keepalive fix,
`presage.rs:305-312`) — is untouched.

### 2.3 Cargo

```toml
# workspace Cargo.toml [workspace.dependencies] — the signal crate takes all three with `workspace = true`
presage              = { git = "https://github.com/whisperfish/presage", rev = "c363731038acf176ab6b5ee0600d7de85f8a2da8", features = ["cdsi"] }
presage-store-sqlite = { git = "https://github.com/whisperfish/presage", rev = "c363731038acf176ab6b5ee0600d7de85f8a2da8" }
qrcode               = { version = "0.14", default-features = false }   # unicode QR for the terminal (§11)

# workspace Cargo.toml — required by libsignal; only presage's tree resolves it
[patch.crates-io]
curve25519-dalek = { git = "https://github.com/signalapp/curve25519-dalek", tag = "signal-curve25519-4.1.3" }
```

`crates/signal/Cargo.toml` adds a `ts` feature (`dep:ts-rs` + `runtime/ts`,
`tools/ts`, `agent/ts`) so `SignalEvent` / `SignalStatus` export with the
other wire types (`rpc/src/export.rs`); dev-deps `tempfile`, `figment`,
`serde_yaml`, `tokio/test-util` (paused-clock supervisor tests).

Reasons recorded here per `00` §1.3: crates.io `presage` is an unrelated
crate; the rev is the one jarvis reviewed and ran; `cdsi` is the E.164→ACI
fallback (§5.2). The crate is always compiled — the first build is minutes,
later builds are cached; a feature flag would put a `cfg` in `main` for a
saving nobody measured. Revisit if `cargo check` time becomes a complaint.

As built (58 §0): `presage-store-sqlite`'s SQLCipher is compiled against
libsignal's BoringSSL on every host — `vendor/libsqlite3-sys` patches
crates.io 0.36.0 to `SQLCIPHER_CRYPTO_OPENSSL` over `boring-sys`'s headers,
with a static `EVP_MAC`-over-`HMAC_CTX` shim — so the encrypted store links
on Linux and Windows without OpenSSL and the on-disk format is the stock
SQLCipher 4 one (verified both ways against the OpenSSL-provider CLI).

## 3. Plugin lifecycle

`main.rs` pushes `Box::new(signal::SignalPlugin { channel: cli.headless })`
after `agent`. `start`:

1. Read `signal:` config (§12). If `enabled: false` → register nothing, log,
   return.
2. `channel == true`: `PresageTransport::new(&data_dir)` (spawns the
   `signal-presage` thread, opens the store) and `channel::spawn(...)`, which
   runs the supervisor under `rt.shutdown` and returns a `channel::Handle`.
   `channel == false` (terminal mode): no transport, no handle.
3. Spawn the `Signal` actor (`handle: Option<Handle>`), register it as
   `"signal"`, register the tools `signal_link` / `signal_status` on the
   tool registry (§11.3; the registry is looked up once — `tools` starts
   before `signal` in `main.rs`).

As built, the terminal-mode actor answers `Status` with
`state: Some(DaemonOnly)`, `registered: <presage.db exists>`,
`number: None`, `owner`/`allowed` from config, `threads` read from
`threads.json`; `Link` → `Err("signal runs in the daemon (t --headless);
link with t --link-signal")`; `Allow`/`RemoveAllowed` edit config only;
`Unlink` deletes `presage.db*` directly.

### 3.1 The supervisor (`channel::run`), ported

Control flow and constants are jarvis's (`channel.rs:1230-1455`), with theuth
events. Nothing here is a design choice; it is the shape that stayed up.

```
loop:
  linking phase (skipped when is_registered() unless forced after an unlink, §11.4)
     link_device(device_name) -> LinkAttempt{url, done}
     publish SignalEvent::LinkUrl{url, expires_at_ms = now + 90 s} (§11); answer queued `Link` asks with the URL; await done
     Err after ~90 s (URL expired) -> pause LINK_CHAIN_PAUSE 3 s; after MAX_FAST_LINK_ATTEMPTS 10 -> LINK_SLOW_PAUSE 300 s; fresh URL
     transport error -> backoff_delay(fault_attempt)
     Ok -> publish SignalEvent::Linked{number}; contact sync happens in the transport
  connected phase
     stream = incoming(); publish Connected; connected_at = now
     select { shutdown -> return; env = stream.next() -> envelope worker; ctrl -> Link answers "already linked as <n>", Unlink runs §11.4 }
     stream None -> disconnected:
        is_registered() == Ok(false) -> publish Unlinked; back to linking (device was unlinked from the phone)
        uptime >= STABLE_CONNECTION 30 s -> attempt = 0
        first retry logs info only (routine idle drop, no status flap); later ones publish Disconnected{retry_in_secs}
        sleep backoff_delay(attempt) = min(2^min(attempt,6), 60) s, still answering ctrl while asleep
```

Two worker tasks (envelope, bus event), each an ordered mpsc, so a slow send
or a `/compact` never stalls the receive stream (`channel.rs:1223-1229`).
Sends: `SEND_RETRIES = 2`, `RETRY_DELAY = 200 ms`, per chunk. Typing:
re-sent every `TYPING_REFRESH_INTERVAL = 10 s` while a turn runs (clients
clear it after ~15 s).

## 4. Status events

One event type on the bus, published by the channel and rendered by any UI
(TUI info line, RPC stream, tool result):

```rust
#[serde(tag = "type")]
pub enum SignalEvent {
    LinkUrl  { url: String, expires_at_ms: u64 },   // a provisioning URL to show as a QR
    Linked   { number: String },                    // link completed
    Connected, Disconnected { retry_in_secs: u64 },
    Unlinked,                                       // registration lost; back to LinkUrl
    DaemonOnly,                                     // terminal mode: no channel in this process
}
```

`Status` (`ask` on `"signal"`) returns `SignalStatus { state: Option<SignalEvent>,
registered, number: Option<String>, owner, allowed, threads: Vec<(number,
SessionId)> }` — `state` is the last event published (`None` before the
supervisor has said anything), `registered`/`number` come from the
transport in the daemon. The same snapshot feeds `/status`, `/signal
status`, the `signal_status` tool (`render_status`) and RPC `signal_status`.
`DaemonOnly` is never published on the bus; it is the terminal-mode
`Status` answer (§3).

## 5. Trust

### 5.1 Allowlist

`signal.owner` (one E.164) and `signal.allowed` (more E.164s). Empty
`allowed` = owner only. Numbers are normalised on both sides
(`normalize_number`: strip ` -.()`, prefix `+` when all digits). A sender not
in the list is dropped with one `warn!` and **no reply** — a bot number
answering strangers advertises itself. Owner-only commands: `/approve`,
`/deny`, `/approvals`, `/signal`. Everything else any allowed number may do,
each in its own thread.

### 5.2 Sealed sender

Most inbound envelopes carry only the sender's ACI (`sender.number == ""`).
The channel keeps an `AciDirectory { by_aci: HashMap<Uuid, String> }` built
lazily: on an unknown ACI it calls `transport.resolve_uuid(number)` for every
configured number not yet resolved (own account → trusted synced contact →
CDSI, ACI only, PNIs rejected → contact resync at most every 300 s — all
inside the frozen transport). Successes cache forever, failures for
`ACI_RESOLVE_COOLDOWN = 300 s`. An allowlisted envelope that carries both
number and ACI is cached too, so a later sealed-sender message from the same
account needs no lookup. The operational consequence goes in the
README: **the owner's number must be saved as a contact on the primary
phone**, or sealed-sender messages cannot be attributed and are dropped.

## 6. Threads and sessions

One contact ↔ one live session, in `signal/threads.json`, written on every
change (`channel::Threads`, a `BTreeMap<String, Thread { session, aci }>`
keyed by the *normalised* number — `Threads::get/bind/unbind` run
`normalize_number` so `"+1 (555) …"` and `"+1555…"` are one key). `aci` is the
contact's account UUID from the last inbound envelope; `bind` keeps the one
already stored when the new sender has none. Files written before `aci`
existed (`{ "+1…": "<session>" }`) still load. Rules:

- First message from a number with no entry → `SendInput { session: None }`;
  the returned id is recorded.
- `/new` → drop the entry; the next message creates a session. The old
  session stays in sqlite like any other.
- `/resume <id>` → point the entry at an existing session (continue a
  terminal session from the phone; `/sessions` lists them). The argument
  matches when it equals the full id, is a unique prefix, or is a unique
  suffix — so the last-8 short id `/sessions` prints is enough.
- Restart → the file is read (missing or corrupt → empty); nothing else to
  restore. Approval codes, typing state and the ACI *directory* (§5.2) are
  process-local by design; the per-thread ACI is not, see §8.5.

Subagent sessions are never bound to a contact; their `AgentEvent`s are
filtered out by session id like every other UI does.

## 7. Inbound

```
envelope (frozen pump: text/attachments/quote/reaction, sender aci+number)
  resolve_sender (§5.2) → is_allowed? no → warn, drop
  send_receipt(ts)                           every allowed message, before anything else
  reaction? → remove → ignored; else Pending::by_reaction(number, target_ts) + decision_for_emoji → Resolve (§10)   done
  slash or bare approval code? → commands::handle (§9)   done, no model call
  text = quote prefix `(replying to: "…")` + body + attachment lines (§7.4); empty → drop
  send_reaction(👀, author=sender, ts)         "seen, working"
  session = threads[number] (create per §6)
  turn in flight for session? → Steer { session, text }   else SendInput
  remember (session → (sender, ts)) for the reaction lifecycle (§8.2)
  ask failed → send `⚠️ <error>`, react ❌
```

Follow-ups while a turn runs become `Steer` — theuth already has the exact
primitive jarvis had to build a FIFO queue for (`channel.rs:1755-1760`). The
steer lands after the current tool batch, which is what a phone user typing
"also check the tests" means.

### 7.4 Attachments

Each inbound attachment is written to `signal/inbox/<ts>-<n>.<ext>` (ext from
the MIME subtype, with `jpg`/`txt`/`mp3`/`m4a`/`bin` special-cased; a
generic declared type — empty, `application/octet-stream`,
`binary/octet-stream`, `application/binary` — is sniffed by magic bytes for
png, jpeg, gif, webp, pdf and ogg, jarvis `channel.rs:139`) and one line is
appended to the user text:

```
[attachment: /Users/x/.theuth/signal/inbox/1757500000000-0.png (image/png, 82 KiB)]
```

The model reads it with `read_file` if it wants to (a PDF or Office file
comes back as Markdown, 20 §2.2). Images, and PDFs up to 32 MB, also ride
along in `files` as native model input (17 §5, 20 §2.5). Voice notes get
the same line with `, voice note, 12 s` appended. Nothing is transcribed.

## 8. Outbound

### 8.1 Text

The channel accumulates `TextDelta` per bound session and sends at
`TurnFinished`: `text::markdown_to_plain` (fences kept verbatim, `#`/`**`/
`_` stripped at word boundaries, `[t](u)` → `t (u)`), then
`text::chunk_text(2000)` (chars, break at newline > whitespace > hard), each
chunk with retry. Empty text (turn ended in a cancel with nothing said) sends
nothing; the reaction says what happened.

`MessageDelivered` (subagent results) and tool cards are not sent — they are
noise on a phone. `Error` events are sent as `⚠️ <message>`.

### 8.2 Reactions — the status line of the phone

Signal keeps one reaction per author per target; sending a new one replaces
the old. The channel uses that as a three-state indicator on the *user's*
message that started the turn:

| moment | reaction on the user's message |
|---|---|
| accepted, `SendInput`/`Steer` sent | 👀 |
| `TurnFinished` stop_reason normal | ✅ |
| `TurnFinished { "cancelled" }` | 🛑 |
| `TurnFinished { "error" }` / `Error` | ❌ |

A `Steer` during a turn gets its own 👀 and then the same terminal reaction as
the turn. jarvis only had 👀 (`channel.rs:117`); the rest is free because the
transport method already exists.

### 8.3 Typing

`TurnStarted` → typing Started sent inline (before the refresh task exists),
then a spawned task re-sends it every `TYPING_REFRESH_INTERVAL = 10 s`;
`TurnFinished` → Stopped. Per-session `AbortHandle` in `Turn.typing`,
aborted on finish or when a new `TurnStarted` replaces it.

### 8.4 `message_user`

The agent's `message_user` tool (24) reaches the phone through
`impl agent::UserSink for channel::Handle` (`user.rs`), registered with the
`"user"` actor at start when the channel runs. Bound pass: the first session
of the tool's ancestor chain with a thread (`threads.recipient_for`, §6); fallback
pass: `signal.owner`, with the `session <last 8> · <title>` origin line as the
first line of the text. The text goes through `markdown_to_plain` + the §8.1
chunker with retry, then one `send_attachment` per file with its file name and
no caption (`Channel::send_attachment`, same `SEND_RETRIES` loop). Result
string: `signal: <number>`. It arrives before the turn's own §8.1 text — out
of band, by design (24 §6).

### 8.5 Who a send goes to

Every outbound built from a bus event or `message_user` is addressed with
`Threads::recipient_for(session)` / `Threads::recipient(number)`: the thread's
number **plus the ACI** learned from its inbound envelopes. `Recipient::new`
(number only) is never used for a thread that has received a message. The
frozen presage sender only falls back to the E.164 contact-store scan when
`uuid` is `None`, and on a freshly linked device that store has no trusted row
for an allowlisted stranger — which is why the ACI is persisted in
`threads.json` (§6) rather than kept in the process-local directory (§5.2):
after a restart, a UI follow-up or `message_user` must still reach the phone
without waiting for a new inbound message. Checked by
`channel::tests::bus_sends_keep_inbound_aci`.

## 9. Slash commands

Same names, arguments and meaning as the terminal (`tui::model::COMMANDS`);
the phone is not a different product. Parsing (`commands::classify`):
trimmed text starting with `/`, first word lowercased, exact name else
prefix-unique completion like the TUI (`/mo` → `/model`). Unknown or
ambiguous → the help text. A bare four-char text that is a known approval
code is `/approve <code>` (§10). Owner-only names from a non-owner answer
`This command is restricted to the owner.`; a failed ask answers `error: …`.
Commands never reach the model.

| command | phone behaviour | asks |
|---|---|---|
| `/help` | this table, one line each, in a fence | – |
| `/status` | `/signal status` block + `session: <last 8>` / `model` / `turn: running\|idle` (no context %; `turn` is `LoadSession.running`) | `Status`, `ListSessions`, `LoadSession` |
| `/new` | unbind thread (§6); replies `new thread; next message starts a session` | – |
| `/sessions` | 10 most recently active top-level sessions: last-8 id · title · age since last activity | `ListSessions` |
| `/resume <id>` | bind thread to session; full id, unique prefix or unique suffix; replies `resumed <id>` | `ListSessions` |
| `/cancel` | abort the turn | `Cancel` |
| `/compact` | fold; replies `compacted → epoch n` | `Compact` |
| `/model [id]` | set → `model: <id>`; without id lists configured models (id · context, ≤ 20, footer `N more; /model <id> to set`) | `ListModels`, `SetModel` |
| `/agents` | this session's subtree: name · state · last · age (state word as the TUI, no glyphs; 19 §4) | `ListAgents` |
| `/cancel <name>` | abort a named child's turn | `ListAgents`, `Cancel` |
| `/prompt` | the assembled system prompt (chunked) | `ShowPrompt` |
| `/skills [add <url>\|rm <name>\|sync]` | as the TUI; `rm` tries a repo first, then a loose skill | `ListSkills`, `AddSkillRepo`, `RemoveSkillRepo`, `RemoveSkill`, `SyncSkills` |
| `/mcp [add <url\|json>\|sync]` | as the TUI, no gate editing (no picker); `add` clones a git URL or adds a JSON entry | `ListServers`, `AddRepo`, `AddServer`, `SyncMcp` |
| `/login [name]` / `/logout <name>` | list providers then OAuth MCP servers; with a name, provider vs MCP server decided by whether `ListLogins` contains the name; login replies `login: open <url>` | `ListLogins`, `ListServers`, `Login`/`McpLogin`, `Logout`/`McpLogout` |
| `/approvals on\|off [always]` | owner only; bare → usage; replies `approvals: on (this session), off (default)` | `SetApprovals` |
| `/approve <code>` / `/deny <code>` | owner only (§10) | `Resolve` |
| `/signal [status\|link\|allow <n>\|rm <n>\|unlink]` | owner only; bare = `status`; `link` replies the current provisioning URL (text, no QR); `allow`/`rm` go through the `"signal"` actor (live list + `write_config_value`), reply `<n> allowed` / `<n> removed`; `unlink` deletes `presage.db` and returns to linking, no reply (the device is gone) | `Status`, `Link`, `Allow`, `RemoveAllowed`, `Unlink` |

Missing on purpose: opening a child (a transcript dump is not phone content —
it is `t --resume <id>` territory, 19 §4), `/quit`/`/exit` (the daemon is not yours to stop
from a phone; `shutdown` stays RPC). Replies are plain text, ≤ 2000 chars
per chunk, tables as aligned monospace lines inside a fence (rendered
verbatim by `markdown_to_plain`).

The dispatcher is a `match` of ~18 arms, each one `ask` plus a formatter —
the same duplication the RPC api and TUI already accept (`00` §7: three
frontends, one actor surface). A shared "command → text" layer would be the
first abstraction with a single new consumer; rejected in §14.

## 10. Approvals

Signal-owned sessions only: an `ApprovalRequested` whose `session` is in
`threads.json` is sent to **that thread** (the owner, or an allowed number
working in its own thread — it is their tool call). Requests from sessions
nobody on Signal owns are not sent; the terminal/RPC client that owns them
sees them.

```
⚠️ shell wants to run
  rm -rf target/
👍 to allow · 👎 to deny · or /approve k7mq · /deny k7mq
```

`approvals::message`: line 1 `⚠️ <tool> wants to run`; then the body
indented two spaces per line — for `shell` the `command` argument verbatim,
for any other tool `serde_json::to_string_pretty(args)` truncated to
`MAX_ARGS_CHARS = 600` chars with a trailing `…`; last line the 👍/👎/code
hint. Sent through the normal chunker; `mark_sent` records the last chunk's
timestamp.

- Code: `CODE_LEN = 4` chars from `abcdefghjkmnpqrstuvwxyz23456789` (no
  `0 O 1 I l`; jarvis `generate_approval_code`, `channel.rs:114`), unique
  among live codes and tombstones; lookups are case-insensitive.
  `Pending` map: `code → Entry { id: ApprovalId, session, number, sent_ts }`.
- **Reaction path:** an inbound reaction (§2.2) whose `target_timestamp`
  matches a pending `sent_ts` and whose author is the thread's `number`
  resolves it: 👍/✅/👌 → `Allow`, 👎/❌/🚫 → `Deny` (variation selectors
  and skin tones stripped first), anything else ignored. `remove: true`
  is ignored before any lookup (un-reacting does not un-approve).
- **Text path:** `/approve <code>` / `/deny <code>`; also the bare `code`
  alone is accepted (typing `k7mq` is enough) — `classify` treats a
  four-char message that is a live or tombstoned code as `/approve`.
  `resolve_by_code` replies: unknown → `unknown code k7mq`; tombstoned →
  `k7mq was already allowed` / `denied` / `timed out`; live → asks
  `tools::Resolve` and replies `k7mq allowed` / `k7mq denied`, or
  `k7mq was already resolved` when the actor answers `false`, or `error: …`.
  It does not touch `Pending`; the bus `ApprovalResolved` does.
- `ApprovalResolved` for a pending code from anywhere (terminal, RPC,
  timeout) → `Pending::resolve` drops the entry and keeps a tombstone for
  `TOMBSTONE_TTL = 600 s` (swept by `sweep`), so a late `/approve k7mq`
  answers "was already …" instead of "unknown code". Timeout additionally
  sends `⌛ k7mq expired` (the model saw a deny).
- The approval message itself gets a reaction from us once resolved
  (✅ / ❌ / ⌛, `reaction_for`) targeting `(own account, sent_ts)`, so the
  thread reads correctly later. Signal allows reacting to one's own message;
  skipped (debug log) when the own account is not known yet.

jarvis's UI-connected delay and reminder timers are dropped: in the daemon
there is no "UI open"; the phone *is* the UI for its own sessions.

## 11. Pairing and setup

Linking follows the `/login` shape (`Login` ask → URL, `LoginFinished`
event): a command returns a URL, the UI renders it, completion arrives as an
event). Signal's URL is a `sgnl://linkdevice?uuid=…&pub_key=…` (~110 chars)
that must be scanned, so every surface renders a QR (`qrcode` → unicode
half-blocks, `Dense1x2`, no quiet zone: QR version ≈ 6, about 41×21 cells,
~900 chars — well under the 2000-char tool display cap). The renderer is
`link::qr` (31 §14), re-exported here as `signal::qr`; `signal` depends on
`link` for it (`link` sits above `runtime` only, so the order stays acyclic).

### 11.1 CLI, one-shot

```
t --link-signal [--owner <E.164>] [--device-name <name>]
  --link-signal   Pair this machine as a Signal linked device (one-shot: prints a QR,
                  waits for the scan, writes `signal.owner`/`device_name`, exits).
  --owner         Owner E.164 (`+15551234567`) for --link-signal; prompted on a TTY when absent.
  --device-name   Device name shown in the phone's Linked devices list (default: config `signal.device_name`).
```

`--owner` and `--device-name` `requires = "link_signal"`. `main::link_signal`
runs before the runtime: `signal::link_oneshot` opens the store, refuses when
already registered (`already linked as +1555…; t --headless to run,
/signal unlink to relink`), then `link_device`, prints the QR and the URL
(`scan from Signal → Settings → Linked devices (expires in ~90 s; a fresh
code follows):`), waits for `done` rotating URLs per §3.1 (link_device
errors → `backoff_delay`), and returns the account number. Then the owner:
`--owner` (validated by `signal::e164`) → else the existing `signal.owner`
→ else a TTY prompt `owner number (E.164, e.g. +15551234567): `; an invalid
or absent answer prints `…; set signal.owner in <config> yourself` and
leaves it unset. `signal.owner` (when set, double-quoted) and
`signal.device_name` are written with `write_config_value`; prints
`linked as +1555…; owner +1444…; run t --headless`, exits 0.

As built there is no dedicated "a daemon holds the store" message: a locked
or unreadable store surfaces as the transport's own error from
`is_registered`.

### 11.2 In the daemon

`Link` on the `"signal"` actor returns the current provisioning URL: in the
linking phase the supervisor is already rotating them (a `Link` during a
between-URL pause cuts the pause short and is answered with the next URL);
in the connected phase it answers `Err("already linked as <number>")`.
`SignalEvent::LinkUrl` / `Linked` follow on the bus. Surfaces:

- RPC (`rpc/src/api.rs`): `signal_link() -> String` (URL),
  `signal_status() -> SignalStatus`, `signal_allow(number)`,
  `signal_remove_allowed(number)`, `signal_unlink()`; `SignalEvent` is one
  of the event types the bridge forwards on the stream (`rpc/src/bridge.rs`),
  and `SignalEvent`/`SignalStatus` are in the TS export set.
- Any UI that renders `SignalEvent::LinkUrl` shows the QR (the TUI, when it
  is an RPC client later; today the TUI process has no channel).

`Allow`/`RemoveAllowed` on the actor validate with `e164`, update the live
list through the channel `Handle` (daemon), then rewrite `signal.allowed` in
`config.yaml` as a quoted YAML flow list (`["+1555…", "+1444…"]`) with
`write_config_value`. The owner cannot be added to `allowed`.

### 11.3 With the agent

Tools registered by the plugin, so "set up Signal for me" works in a chat:

- `signal_link { owner?: string }` (`Mutating`). `owner`, when given, is
  validated (`e164`) and written to `signal.owner` in `config.yaml` first —
  **config only**: the running channel keeps the owner it started with until
  restart. In the daemon → `Link` and return `scan this from Signal → Settings
  → Linked devices:` + QR + URL + `(expires in ~90 s; a fresh code follows
  in the daemon log)`. In terminal mode → starts `link_oneshot` *in
  process* as a background task (it writes `signal.device_name` when the
  scan completes), waits up to `FIRST_URL_WAIT = 30 s` for the first URL and
  returns the same text with `(expires in ~90 s; call signal_link again for
  a fresh code)`. It does **not** block until `done`: a repeat call while the
  task is running returns the *current* URL; once the task has finished a
  repeat call starts a new one. No URL within 30 s → `no provisioning URL
  within 30 s (already linked, or the store is busy); see the log`.
- `signal_status {}` (`ReadOnly`): the §4 snapshot as text
  (`render_status`: `state:` — for `LinkUrl` the QR and URL — `registered:`,
  `number:`, `owner:`, `allowed:`, `threads:` one `number → session` per line).

The tool output is the QR; the TUI card shows it monospace. Verified in
§15.6 that the dense rendering survives `DISPLAY_OUTPUT_CHARS`.

### 11.4 Unlink

`/signal unlink`, RPC `signal_unlink`, or deleting the device from the phone.
The last is detected by `is_registered() == false` after a stream end (§3.1)
and returns to linking; the first two stop the pump, delete `presage.db`
(+ `-wal`/`-shm`), publish `Unlinked` and do the same. `Unlink` while not
registered answers `Err("not linked")`. `threads.json` is kept — sessions
outlive the device.

As built (known limitation, see `channel::Channel::linking`'s doc comment):
the frozen `PresageTransport` keeps its registration state in memory, so
after an explicit unlink `is_registered()` still answers `true` until the
process restarts. The supervisor therefore re-enters the linking phase with
`skip_check = true` and calls `link_device` without consulting
`is_registered` for that first pass. Relinking in the same process works;
`/signal status` may report `registered: true` in between. A restart clears
it.

## 12. Config

```yaml
signal:
  enabled: true            # daemon starts the channel when linked
  device_name: theuth         # shown in the phone's Linked devices list
  owner: "+15551234567"    # set by --link-signal / signal_link; owner-only commands
  allowed: []              # more E.164s; each gets its own thread
```

Env overrides as everywhere (`THEUTH_SIGNAL__OWNER`). No quiet hours, no
approval timers (§10), no phone_number (the account tells us).

## 13. Types

```rust
// signal/src/lib.rs
pub const SIGNAL_ACTOR: &str = "signal";
pub struct SignalPlugin { pub channel: bool }             // headless → true
pub struct Link;                                          // ask → Result<String /* url */, String>
pub struct Status;                                        // ask → SignalStatus
pub struct Allow(pub String); pub struct RemoveAllowed(pub String); pub struct Unlink;   // ask → Result<(), String>
pub struct SignalStatus { state: Option<SignalEvent>, registered: bool, number: Option<String>,
                          owner: String, allowed: Vec<String>, threads: Vec<(String, SessionId)> }
pub fn e164(&str) -> Result<String, String>;              // normalise + validate (+, 8–15 digits)
pub fn qr(&str) -> String;                                // Dense1x2, no quiet zone
pub async fn link_oneshot(&Path, &SignalConfig, on_url) -> anyhow::Result<String /* number */>;

// signal/src/channel.rs
pub struct Handle { chan: Arc<Channel> }                  // link / status / allow / remove_allowed / unlink
struct Channel {
    transport: Arc<dyn SignalTransport>,
    port: Arc<dyn Port>,                                  // agent/approval asks behind a trait (tests stub it)
    rt: Runtime, owner: String, device_name: String,
    control: mpsc::UnboundedSender<Control>,              // Link / Unlink into the supervisor loop
    state: Mutex<State>,                                  // allowed, threads: Threads (BTreeMap + save()), aci: AciDirectory,
                                                          // turns: HashMap<SessionId, Turn { in_flight, text, marks: Vec<(Recipient, ts)>, typing: Option<AbortHandle> }>,
                                                          // last_event, link_url
    approvals: tokio::sync::Mutex<approvals::Pending>,    // code ↔ Entry, tombstones
}
// bus subscriptions: AgentEvent, ApprovalRequested, ApprovalResolved
// asks (commands.rs): agent (SendInput, Steer, Cancel, Compact, ListSessions, LoadSession, ListAgents, ListModels, SetModel,
//       ShowPrompt, ListSkills, AddSkillRepo, RemoveSkillRepo, RemoveSkill, SyncSkills, ListLogins, Login, Logout),
//       tools registry (SetApprovals), approval actor (Resolve), mcp (ListServers, AddRepo, AddServer, SyncMcp, McpLogin, McpLogout)
```

## 14. Rejected

- **Editing the protocol tier beyond §2.2.** Every jarvis gotcha (multi-thread
  runtime + LocalSet keepalive starvation, PNI rejection, trusted-contact
  filter, stable-disconnect backoff reset, URL rotation cadence) is either
  inside the frozen files or ported as constants. Rewriting "in theuth style"
  is how the bugs come back.
- **Text-only approvals (jarvis parity).** Two additive lines buy 👍/👎; the
  code path stays as fallback. Cheap, and the phone-native gesture.
- **Queueing follow-ups** (jarvis). `Steer` exists.
- **Streaming fragments / progress messages.** No edits on Signal; typing
  indicator + one reply.
- **Sessions column for the thread mapping.** A plugin-local JSON file avoids
  a cross-crate migration for one map.
- **Shared command layer with the TUI/RPC.** Third copy of ~18 asks; still
  cheaper than a new crate boundary with one consumer each. Revisit if a
  fourth frontend appears.
- **Approvals for non-Signal sessions on the phone.** Useful ("approve my
  headless RPC job from bed") but needs an "owner sees everything" mode and a
  way to say which sessions; not in v0. One config key later.
- **Feature-gating the crate.** One `cfg` saved, unmeasured build time; the
  workspace already compiles rmcp, brush and iocraft.
- **Groups, STT, TTS, image parts.** §1.

## 15. Checks

Unit tests live in the module they check (`cargo test -p signal`):

1. **Frozen copy.** `cargo test -p signal` runs the ported presage/text tests
   unchanged (`tempfile` dev-dep). `diff <(sed 1,2d src/presage.rs) ../jarvis/…/presage.rs`
   differs only in the §2.2 hunk — recorded in the PR, not automated.
2. **Supervisor** (unit, `MockTransport`, paused clock) — `channel::tests::
   reconnect_backoff_and_stable_reset`, `link_url_rotation_slows_after_ten`,
   `unregistered_after_drop_returns_to_linking`, `backoff_and_numbers`:
   stream end reconnects with 1,2,4,…,60 s; a 30 s-stable session resets to
   1 s; `is_registered=false` after a drop returns to linking; an expired
   URL rotates after 3 s and slows to 300 s after ten.
3. **Trust** (unit) — `channel::tests::stranger_dropped_silently`,
   `sealed_sender_resolved_and_cached`, `commands::tests::
   owner_only_commands_are_gated`: stranger dropped silently; sealed-sender
   ACI resolved via `resolve_uuid` and cached; a failed lookup lands in
   `failed_at` (the 300 s cooldown itself is not clock-tested); owner-only
   commands from an allowed non-owner get the refusal line.
4. **Inbound** (unit) — `channel::tests::
   receipt_first_then_eyes_then_send_input_and_bind`,
   `slash_never_reaches_the_agent`, `attachment_saved_and_quote_prefixed`,
   `eyes_then_check_on_same_message_and_steer_in_flight`, `mime_helpers`,
   `threads_roundtrip`; `commands::tests::completes_unique_prefixes_only`,
   `bare_pending_code_is_approve`, `help_lists_every_command_once`,
   `table_aligns_columns`: receipt before anything; slash never reaches the
   agent; idle → `SendInput`, in flight → `Steer`; attachment saved and its
   line appended; quote prefixed.
5. **Reactions** (unit) — `channel::tests::
   eyes_then_check_on_same_message_and_steer_in_flight`,
   `approval_reaction_resolves_allow`; `approvals::tests::
   codes_unique_from_alphabet`, `by_code_case_insensitive`,
   `reaction_needs_number_and_ts`, `resolve_tombstones_and_sweep`,
   `emoji_variants`, `reactions`, `shell_message_shows_command`,
   `resolve_by_code_replies`: 👀 then ✅/🛑 on the same `(author, ts)`; an
   inbound 👍 on a pending approval's `sent_ts` resolves `Allow`, 🎉 ignored,
   a 👍 from a non-owner number ignored; ✅ reaction on our own approval
   message after `ApprovalResolved`. (👎 → `Deny` is covered at the
   `decision_for_emoji` level, not through the channel.)
6. **QR fits** — `lib::tests::qr_fits_a_tool_card` (plus `e164_validation`,
   `allowed_list_is_quoted_flow_yaml`): render a `sgnl://linkdevice?uuid=…&pub_key=…`
   with `Dense1x2`, assert `< 2000` chars and ≥ 20 lines.
7. **e2e, manual, real account** — **pending the user's test**, to be
   recorded in the PR: `t --link-signal` → scan → `linked as`;
   `t --headless`; text "hi" → 👀 → typing → reply → ✅; "run `ls`" with
   `mutating: ask` → approval message → 👍 → tool runs; `/new`, `/sessions`,
   `/resume` round-trip; unlink from the phone → `Unlinked` → new QR in the
   log within 5 s; kill the daemon mid-turn, restart → thread still bound.

## 16. Open — spikes before widening scope

- **Note to Self.** If presage's `Received::Content` with
  `ContentBody::SynchronizeMessage(Sent { destination == self })` can be
  turned into an envelope, a single-account user needs no second number.
  Additive to `content_to_envelope`; measure before designing.
- **Groups.** presage supports `group_v2` sends; the trust question (who in
  the group may talk to the agent) is the design work, not the code.
- **Owner sees all approvals.** §14.
- **In-memory registration after unlink** (§11.4). `PresageTransport` answers
  `is_registered() == true` until restart once the store is deleted; the
  supervisor works around it with `skip_check`. A clean fix is a `reset` on
  the frozen transport — a third §2.2-style hunk, only if the workaround bites.
- **Live owner change.** `signal_link { owner }` and `--link-signal --owner`
  write config only; the running channel's `owner` is fixed at start. Add an
  `SetOwner` ask if changing the owner without a restart is ever wanted —
  98 does it live for Telegram (`SetOwner`); the same ask shape fits here.

## 17. Order

1. ✅ Crate skeleton + frozen files + Cargo/patch; `cargo test -p signal`
   green on the ported tests. (§2, §15.1)
2. ✅ §2.2 edits + mock; `channel::run` supervisor with `MockTransport` tests.
   (§3, §15.2)
3. ✅ Trust, threads, inbound → `SendInput`/`Steer`, outbound text, reactions,
   typing. (§5–8, §15.3–5)
4. ✅ Commands, approvals. (§9–10)
5. ✅ `signal` actor, tools, RPC methods, `--link-signal`, QR. (§11, §15.6)
6. README `## Signal` section ✅; this doc to as-built ✅; manual e2e (§15.7)
   pending.
