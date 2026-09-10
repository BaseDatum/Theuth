# 09 – Signal: a phone-side frontend for the daemon

Status: draft v0 · 2026-09-10 · extends `00-architecture.md` §4, §7;
`01-config-and-subagents.md` §1.1; `05-oauth.md` (login-flow shape);
`07-context.md` §5 (approvals), §8.6 (compact).

Signal is a frontend, like the terminal UI: it sends `SendInput`/`Steer`/
`Cancel`/`Resolve` and renders `AgentEvent`s. It runs **only in the daemon**
(`wth --headless`). It is also the only frontend whose transport code is
*imported*, not written: the presage layer jarvis spent weeks debugging is
copied byte-for-byte and never edited beyond two additive lines (§2.2).

## 1. Goal, and what is not one

Goal: text the daemon from a phone and get the same agent the terminal gets —
turns, steering, approvals, slash commands, session switching — with Signal's
own affordances (reactions, typing, receipts, quotes) used the way Signal
users expect. Survive restarts, disconnects and re-linking without a human.

Not goals, with the reason:

- **Groups.** presage drops `group_v2` (jarvis `presage.rs:892`); one contact
  = one thread keeps the trust model a list of numbers. Revisit with a spike.
- **Voice/STT/TTS, images to the model.** wth's `SendInput` is text. Inbound
  files are saved and their path is handed to the model (§7.4), which can
  `read_file` them; that covers the useful 80 %.
- **Streaming to the phone.** Signal cannot edit a sent message; a burst of
  fragments is worse than one reply. Typing indicator carries the "alive"
  signal; the final text is sent once (§8.1).
- **Running in TUI mode.** One presage store is one linked device with one
  websocket; two processes on it corrupt each other. The terminal process
  never opens the store except for the one-shot link command (§11.1).

Deployment fact, stated once: wth links as a *secondary device* of a Signal
account, so that account needs a primary phone. In the usual setup the daemon
owns a **dedicated number** and the owner messages it from their personal
account. Messaging your own account ("Note to Self") arrives as a sync
message, which the frozen transport does not surface — a spike in §16.

## 2. Layout

```
crates/signal/
  Cargo.toml
  src/
    lib.rs        plugin, `signal` actor, commands, tools      (new, ~150 lines)
    channel.rs    link/connect supervisor, inbound, outbound   (new, ~600 lines; control flow and constants ported from jarvis channel.rs)
    commands.rs   slash parser + dispatcher for the phone      (new, ~200 lines)
    approvals.rs  codes, message text, reaction/answer routing (new, ~150 lines)
    transport.rs  FROZEN  jarvis crates/jarvis-signal/src/transport.rs
    presage.rs    FROZEN  jarvis crates/jarvis-signal/src/presage.rs
    text.rs       FROZEN  jarvis crates/jarvis-signal/src/text.rs
    error.rs      FROZEN  jarvis error.rs minus the `Core` variant and its `From`
    mock.rs       FROZEN  the `MockTransport` half of jarvis mock.rs (tests only)
~/.wth/signal/
  presage.db      presage's own sqlite (protocol state, identity keys, contacts); schema is presage's
  threads.json    { "+15551234567": "<session uuid>", … }  contact → live session (§6)
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

### 2.2 The two additive edits

Both are needed for reaction-based approvals (§10) and nothing else. Both add;
neither changes an existing branch.

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

Everything else that touches the wire — linking, receive pump, resolution,
CDSI/PNI handling, contact-sync requests, `OnNewIdentity::Trust`, the
dedicated multi-thread runtime + `LocalSet` (the keepalive fix,
`presage.rs:305-312`) — is untouched.

### 2.3 Cargo

```toml
# crates/signal/Cargo.toml
presage             = { git = "https://github.com/whisperfish/presage", rev = "c363731038acf176ab6b5ee0600d7de85f8a2da8", features = ["cdsi"] }
presage-store-sqlite = { git = "https://github.com/whisperfish/presage", rev = "c363731038acf176ab6b5ee0600d7de85f8a2da8" }
qrcode = { version = "0.14", default-features = false }   # unicode QR for the terminal (§11)

# workspace Cargo.toml — required by libsignal; only presage's tree resolves it
[patch.crates-io]
curve25519-dalek = { git = "https://github.com/signalapp/curve25519-dalek", tag = "signal-curve25519-4.1.3" }
```

Reasons recorded here per `00` §1.3: crates.io `presage` is an unrelated
crate; the rev is the one jarvis reviewed and ran; `cdsi` is the E.164→ACI
fallback (§5.2). The crate is always compiled — the first build is minutes,
later builds are cached; a feature flag would put a `cfg` in `main` for a
saving nobody measured. Revisit if `cargo check` time becomes a complaint.

## 3. Plugin lifecycle

`main.rs` pushes `Box::new(signal::SignalPlugin { channel: cli.headless })`
after `agent`. `start`:

1. Read `signal:` config (§12). If `enabled: false` → register nothing, log,
   return.
2. Spawn the `Signal` actor, register it as `"signal"`, register the tools
   `signal_link` / `signal_status` on the `"tools"` registry (§11.3).
3. `channel == false` (terminal mode): stop here. The actor answers `Status`
   with `{ state: "daemon only", registered: <peek at presage.db exists> }`
   and refuses `Link` with the CLI hint (§11.1). No transport thread.
4. `channel == true`: `PresageTransport::new(&data_dir)` (spawns the
   `signal-presage` thread, opens the store) and spawn `channel::run(...)`
   under `rt.shutdown`.

### 3.1 The supervisor (`channel::run`), ported

Control flow and constants are jarvis's (`channel.rs:1230-1455`), with wth
events. Nothing here is a design choice; it is the shape that stayed up.

```
loop:
  if !is_registered():  linking phase
     link_device(device_name) -> LinkAttempt{url, done}
     publish SignalLinkUrl{url, expires_at} (§11); await done
     Err after ~90 s (URL expired) -> pause LINK_CHAIN_PAUSE 3 s; after MAX_FAST_LINK_ATTEMPTS 10 -> LINK_SLOW_PAUSE 300 s; fresh URL
     transport error -> backoff_delay(fault_attempt)
     Ok -> publish SignalLinked{number}, force contact sync happens in the transport
  connected phase
     stream = incoming(); connected_at = now
     select { shutdown -> return; env = stream.next() -> envelope worker; ev = bus -> event worker }
     stream None -> disconnected:
        is_registered() == Ok(false) -> back to linking (device was unlinked from the phone)
        uptime >= STABLE_CONNECTION 30 s -> attempt = 0, log info (routine idle drop, no status flap)
        sleep backoff_delay(attempt) = min(2^min(attempt,6), 60) s, still draining bus events while asleep
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
}
```

`Status` (`ask` on `"signal"`) returns the last of these plus `owner`,
`allowed`, `threads: Vec<(number, SessionId)>` — the same snapshot the
`/signal` command and `signal_status` tool print.

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
`ACI_RESOLVE_COOLDOWN = 300 s`. The operational consequence goes in the
README: **the owner's number must be saved as a contact on the primary
phone**, or sealed-sender messages cannot be attributed and are dropped.

## 6. Threads and sessions

One contact ↔ one live session, in `signal/threads.json`, written on every
change. Rules:

- First message from a number with no entry → `SendInput { session: None }`;
  the returned id is recorded.
- `/new` → drop the entry; the next message creates a session. The old
  session stays in sqlite like any other.
- `/resume <id>` → point the entry at an existing session (continue a
  terminal session from the phone; `/sessions` lists them).
- Restart → the file is read; nothing else to restore. Approval codes,
  typing state and the ACI cache are process-local by design.

Subagent sessions are never bound to a contact; their `AgentEvent`s are
filtered out by session id like every other UI does.

## 7. Inbound

```
envelope (frozen pump: text/attachments/quote/reaction, sender aci+number)
  resolve_sender (§5.2) → is_allowed? no → warn, drop
  send_receipt(Read, ts)                     every allowed message, before anything else
  reaction? → approvals::on_reaction (§10)   done
  slash? → commands::reply (§9)              done, no model call
  text = quote prefix + body + attachment lines (§7.4); empty → drop
  send_reaction(👀, author=sender, ts)         "seen, working"
  session = threads[number] (create per §6)
  turn in flight for session? → Steer { session, text }   else SendInput
  remember (session → (sender, ts)) for the reaction lifecycle (§8.2)
```

Follow-ups while a turn runs become `Steer` — wth already has the exact
primitive jarvis had to build a FIFO queue for (`channel.rs:1755-1760`). The
steer lands after the current tool batch, which is what a phone user typing
"also check the tests" means.

### 7.4 Attachments

Each inbound attachment is written to `signal/inbox/<ts>-<n>.<ext>` (ext from
MIME; `application/octet-stream` is sniffed by magic bytes, jarvis
`channel.rs:139`) and one line is appended to the user text:

```
[attachment: /Users/x/.wth/signal/inbox/1757500000000-0.png (image/png, 82 KiB)]
```

The model reads it with `read_file` if it wants to. Voice notes get the same
line with `voice note, 12 s`. Nothing is transcribed.

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

`TurnStarted` → typing Started, refreshed every 10 s; `TurnFinished` →
Stopped. Per-session `JoinHandle`, aborted on stop or replace.

## 9. Slash commands

Same names, arguments and meaning as the terminal (`tui::model::COMMANDS`);
the phone is not a different product. Parsing: trimmed text starting with `/`,
first word lowercased, prefix-unique completion like the TUI (`/mo` →
`/model`). Unknown → the help text. Commands never reach the model.

| command | phone behaviour | asks |
|---|---|---|
| `/help` | this table, one line each | – |
| `/status` | model, context %, turn in flight?, link state | `Status`, session summary |
| `/new` | unbind thread (§6) | – |
| `/sessions` | last 10 sessions: short id · title · age | `ListSessions` |
| `/resume <id>` | bind thread to session; id prefix accepted when unique | `ListSessions` |
| `/cancel` | abort the turn | `Cancel` |
| `/compact` | fold; replies `compacted → epoch n` | `Compact` |
| `/model [id]` | show / set; without id lists configured models (≤ 20) | `ListModels`, `SetModel` |
| `/agents` | this session's subagents and their state | `ListChildren` |
| `/prompt` | the assembled system prompt (chunked) | `ShowPrompt` |
| `/skills [add <url>\|rm <name>\|sync]` | as the TUI | skills asks |
| `/mcp [add <url\|json>\|sync]` | as the TUI, no gate editing (no picker) | mcp asks |
| `/login [name]` / `/logout <name>` | list, or start a login and send the URL | `ListLogins`, `Login`, `Logout` |
| `/approvals [on\|off] [always]` | owner only | `SetApprovals` |
| `/approve <code>` / `/deny <code>` | owner only (§10) | `Resolve` |
| `/signal [allow <n>\|rm <n>\|status\|unlink]` | owner only; edits config via `write_config_value`, `unlink` deletes `presage.db` and returns to linking | `Status` |

Missing on purpose: `/focus` (a transcript dump is not phone content — use
`/resume` on the child), `/quit`/`/exit` (the daemon is not yours to stop
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

- Code: 4 chars from an alphabet without `0 O 1 I l` (jarvis
  `generate_approval_code`, `channel.rs:114`). Pending map:
  `code → (ApprovalId, session, sent_timestamp)`.
- **Reaction path:** an inbound reaction (§2.2) whose `target_timestamp`
  matches a pending `sent_timestamp` and whose author is the thread's number
  resolves it: 👍/✅/👌 → `Allow`, 👎/❌/🚫 → `Deny`, anything else ignored.
  `remove: true` is ignored (un-reacting does not un-approve).
- **Text path:** `/approve <code>` / `/deny <code>`; also the bare `code`
  alone is accepted (typing `k7mq` is enough).
- `ApprovalResolved` for a pending code from anywhere (terminal, RPC,
  timeout) → drop the entry, keep a tombstone for `APPROVAL_TOMBSTONE = 600 s`
  so a late `/approve k7mq` answers "k7mq was already allowed / denied /
  timed out" instead of "unknown code". Timeout additionally sends
  `⌛ k7mq expired` (the model saw a deny).
- The approval message itself gets a reaction from us once resolved
  (✅ / ❌ / ⌛) so the thread reads correctly later. Signal allows reacting to
  one's own message.

jarvis's UI-connected delay and reminder timers are dropped: in the daemon
there is no "UI open"; the phone *is* the UI for its own sessions.

## 11. Pairing and setup

Linking follows the `/login` shape (`Login` ask → URL, `LoginFinished`
event): a command returns a URL, the UI renders it, completion arrives as an
event. Signal's URL is a `sgnl://linkdevice?uuid=…&pub_key=…` (~110 chars)
that must be scanned, so every surface renders a QR (`qrcode` → unicode
half-blocks, `Dense1x2`, no quiet zone: QR version ≈ 6, about 41×21 cells,
~900 chars — well under the 2000-char tool display cap).

### 11.1 CLI, one-shot

```
wth --link-signal [--owner +1555…] [--device-name wth]
```

Runs before the runtime: opens the store, `link_device`, prints the QR and
the URL, waits for `done` (rotating URLs per §3.1), then writes
`signal.owner` (prompting on a TTY when `--owner` is absent) and
`signal.device_name` with `write_config_value`, prints
`linked as +1555…; owner +1444…; run wth --headless`, exits. Refuses when
already registered (`already linked as +1555…; wth --headless to run,
/signal unlink to relink`) and when a daemon holds the store (sqlite busy →
"a daemon is running; use /signal there").

### 11.2 In the daemon

`Link` on the `"signal"` actor starts the linking phase if not registered
and returns the current URL (the supervisor is already rotating them);
`SignalEvent::LinkUrl` / `Linked` follow on the bus. Surfaces:

- RPC: `signal_link() -> String` (URL), `signal_status() -> Status`,
  `signal_allow(number)`, `signal_unlink()`; events on the stream.
- Any UI that renders `SignalEvent::LinkUrl` shows the QR (the TUI, when it
  is an RPC client later; today the TUI process has no channel).

### 11.3 With the agent

Tools registered by the plugin, so "set up Signal for me" works in a chat:

- `signal_link { owner?: string }` (`Mutating`): in the daemon → `Link`
  and return `URL + QR + "scan from Settings → Linked devices; I'll get a
  Linked event"`. In terminal mode → runs the one-shot §11.1 flow *in
  process* (no daemon can be holding the store if we are in terminal mode
  and linking is wanted; if the store is busy the tool says so) and returns
  the same text; the tool blocks until `done` or the URL expires (90 s), so
  the model's turn waits with the user. Sets `owner` when given.
- `signal_status {}` (`ReadOnly`): the §4 snapshot as text.

The tool output is the QR; the TUI card shows it monospace. Verified in
§15.6 that the dense rendering survives `DISPLAY_OUTPUT_CHARS`.

### 11.4 Unlink

`/signal unlink`, RPC `signal_unlink`, or deleting the device from the phone.
The last is detected by `is_registered() == false` after a stream end (§3.1)
and returns to linking; the first two stop the pump, delete `presage.db`, and
do the same. `threads.json` is kept — sessions outlive the device.

## 12. Config

```yaml
signal:
  enabled: true            # daemon starts the channel when linked
  device_name: wth         # shown in the phone's Linked devices list
  owner: "+15551234567"    # set by --link-signal / signal_link; owner-only commands
  allowed: []              # more E.164s; each gets its own thread
```

Env overrides as everywhere (`WTH_SIGNAL__OWNER`). No quiet hours, no
approval timers (§10), no phone_number (the account tells us).

## 13. Types

```rust
// signal/src/lib.rs
pub const SIGNAL_ACTOR: &str = "signal";
pub struct SignalPlugin { pub channel: bool }             // headless → true
pub struct Link;                                          // ask → Result<String /* url */, String>
pub struct Status;                                        // ask → SignalStatus
pub struct Allow(pub String); pub struct Unlink;          // ask → Result<(), String>
pub struct SignalStatus { state: SignalEvent, owner: String, allowed: Vec<String>, threads: Vec<(String, SessionId)> }

// signal/src/channel.rs
struct Channel {
    transport: Arc<dyn SignalTransport>,
    cfg: SignalConfig,
    threads: Threads,                                     // HashMap<String, SessionId> + save()
    aci: AciDirectory,
    turns: HashMap<SessionId, Turn>,                      // { number, user_ts, text: String, typing: JoinHandle }
    approvals: approvals::Pending,                        // code ↔ (ApprovalId, session, sent_ts), tombstones
}
// bus subscriptions: AgentEvent, ApprovalRequested, ApprovalResolved
// asks: agent (SendInput, Steer, Cancel, Compact, ListSessions, ListChildren, ListModels, SetModel, ShowPrompt, skills, logins),
//       tools (SetApprovals, Resolve), mcp (ListServers, AddServer, SyncMcp)
```

## 14. Rejected

- **Editing the protocol tier beyond §2.2.** Every jarvis gotcha (multi-thread
  runtime + LocalSet keepalive starvation, PNI rejection, trusted-contact
  filter, stable-disconnect backoff reset, URL rotation cadence) is either
  inside the frozen files or ported as constants. Rewriting "in wth style"
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

1. **Frozen copy.** `cargo test -p signal` runs the ported presage/text tests
   unchanged (`tempfile` dev-dep). `diff <(sed 1,2d src/presage.rs) ../jarvis/…/presage.rs`
   differs only in the §2.2 hunk — recorded in the PR, not automated.
2. **Supervisor** (unit, `MockTransport`): stream end reconnects with
   1,2,4,…,60 s; a 30 s-stable session resets to 1 s; `is_registered=false`
   after a drop returns to linking; a provisioning `Err` rotates after 3 s and
   slows to 300 s after ten.
3. **Trust** (unit): stranger dropped silently; sealed-sender ACI resolved via
   `resolve_uuid` and cached; failure cooldown 300 s; owner-only commands from
   an allowed non-owner get the refusal line.
4. **Inbound** (unit): receipt before anything; slash never reaches the agent;
   idle → `SendInput`, in flight → `Steer`; attachment saved and its line
   appended; quote prefixed.
5. **Reactions** (unit): 👀 then ✅/🛑/❌ on the same `(author, ts)`; an
   inbound 👍 on a pending approval's `sent_ts` resolves `Allow`, 👎 `Deny`,
   🎉 ignored; a reaction from a non-owner number is ignored.
6. **QR fits.** Render a real `sgnl://linkdevice?uuid=…&pub_key=…` (jarvis
   log sample) with `Dense1x2`, assert `len() < DISPLAY_OUTPUT_CHARS`.
7. **e2e, manual, real account** (recorded in the PR): `wth --link-signal`
   → scan → `linked as`; `wth --headless`; text "hi" → 👀 → typing → reply
   → ✅; "run `ls`" with `mutating: ask` → approval message → 👍 → tool runs;
   `/new`, `/sessions`, `/resume` round-trip; unlink from the phone →
   `Unlinked` → new QR in the log within 5 s; kill the daemon mid-turn,
   restart → thread still bound.

## 16. Open — spikes before widening scope

- **Note to Self.** If presage's `Received::Content` with
  `ContentBody::SynchronizeMessage(Sent { destination == self })` can be
  turned into an envelope, a single-account user needs no second number.
  Additive to `content_to_envelope`; measure before designing.
- **Groups.** presage supports `group_v2` sends; the trust question (who in
  the group may talk to the agent) is the design work, not the code.
- **Owner sees all approvals.** §14.

## 17. Order

1. Crate skeleton + frozen files + Cargo/patch; `cargo test -p signal`
   green on the ported tests. (§2, §15.1)
2. §2.2 edits + mock; `channel::run` supervisor with `MockTransport` tests.
   (§3, §15.2)
3. Trust, threads, inbound → `SendInput`/`Steer`, outbound text, reactions,
   typing. (§5–8, §15.3–5)
4. Commands, approvals. (§9–10)
5. `signal` actor, tools, RPC methods, `--link-signal`, QR. (§11, §15.6)
6. Manual e2e (§15.7); README `## Signal` section; this doc to as-built.
