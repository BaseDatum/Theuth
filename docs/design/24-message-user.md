# 24 — `message_user`: the agent reaches the user out of band

Status: as-built v0 · 2026-09-14 (draft same day) · extends `00-architecture.md` §6 (tools),
`09-signal.md` §8 (outbound), `21-slack.md` §8.7 (other events),
`17-images.md` (files are paths). §9 is the as-built record.

## 0. Why

Everything the model says today reaches the user as the turn's text: the
TUI transcript, one Slack bubble, one Signal message at `TurnFinished`.
Nothing lets the model *hand the user a file* — a screenshot it just took
(23), a PDF it produced, a log it wants read on a phone — and nothing lets
a session that is not bound to a chat thread (a TUI session, a headless
subagent) ping the user where they actually are. The user asked for one
tool, on Slack and Signal, text plus files and images.

## 1. Goal, and what is not one

- One tool, `message_user { text, files?, via? }`, registered by the agent
  crate like `send_message` (peers). It resolves *which user channel* to
  use from the calling session; the model never names a phone number or a
  Slack channel id.
- Both frontends deliver text and files: Signal as a text message plus one
  attachment message per file; Slack as one message carrying every file
  (`files.completeUploadExternal` with `initial_comment`), or a plain
  `chat.postMessage` when there are no files.
- Delivery is confirmed: the tool result names where it went
  (`delivered via slack: #dev-tools thread`, `delivered via signal:
  +1555…`), or errors with why not. Fire-and-forget over the bus is
  rejected (§7) because the model would report "sent" on a dropped message.

Not goals: messaging anyone but the user (no third-party recipients; the
Signal allow-list and Slack owner/thread model stay the trust boundary,
09 §5, 21 §5); the TUI as a channel (the transcript already shows the
tool call and its result — a file path is a file path on the same machine);
delivery to *several* Slack threads or Signal contacts at once; message
editing, reactions, threading controls.

## 2. Resolution: which channel, which target

The tool builds the session's ancestor chain — itself first, then parent,
grandparent, up to the root (`Agents.sessions[id].parent`, the same field
`Deliver { to: "parent" }` uses) — and hands the chain to every registered
sink. Two passes:

1. **Bound.** Each sink checks the chain in order and delivers to the
   first session it renders: Slack `threads.by_session` (21 §6), Signal
   `threads.number_for` (09 §6). A subagent therefore lands in its own
   Slack thread when it has one, else in the nearest ancestor's; on Signal
   (no per-child threads) in the root contact's chat. If any sink delivered,
   stop.
2. **Owner fallback.** Nothing bound the chain — a TUI or RPC session, an
   ACP session. Every enabled sink delivers to its owner: Slack the owner
   DM opened at boot (`Identity.owner_dm`, 21 §3), Signal `signal.owner`.
   Prefixed with `session <last 8> · <title>` on Slack exactly as unbound
   approval cards are (21 §10.1), so the user knows which conversation is
   talking. Signal prefixes the same line as text.

`via: "slack" | "signal" | "telegram"` restricts both passes to one sink. Without it the
fallback pass fans out to every channel — a user who configured both wants
to be reached, and a TUI session has no better hint. Alternative rejected:
"first configured sink only", arbitrary and silently wrong for whichever
channel the user is actually watching.

No sink at all (neither plugin enabled, or both running without a channel
— `theuth` TUI mode starts plugins with `channel: false`) → the tool errors
`no user channel: slack, signal and telegram are not running`, and the model falls
back to saying it in the reply.

## 3. Shape

```rust
// crates/agent/src/user.rs
pub const USER_ACTOR: &str = "user";

/// What one frontend can deliver; registered by the Slack, Signal and Telegram plugins at start.
#[async_trait]
pub trait UserSink: Send + Sync {
    /// `"slack"` / `"signal"` / `"telegram"`: the `via` value and the word in the tool result.
    fn name(&self) -> &'static str;
    /// Deliver `m` to the first session of `m.sessions` this sink renders
    /// (`fallback == false`), or to the owner (`fallback == true`).
    /// `Ok(None)`: not mine (bound pass only). `Ok(Some(where))`: delivered,
    /// human description. `Err(why)`: mine, but the send failed.
    async fn deliver(&self, m: &Outbound, fallback: bool) -> Result<Option<String>, String>;
}

pub struct Outbound {
    /// Calling session first, root last (§2).
    pub sessions: Vec<SessionId>,
    /// `session <last 8> · <title>` for the fallback pass; the sink decides how to show it.
    pub origin: String,
    pub text: String,
    /// Validated by the tool: regular files, ≤ `MAX_FILE` each, with their sniffed MIME.
    pub files: Vec<OutboundFile>,
}
pub struct OutboundFile { pub path: PathBuf, pub name: String, pub mime: String, pub bytes: u64 }

pub struct UserChannels { sinks: Vec<Arc<dyn UserSink>> }          // kameo actor, registered "user"
pub struct RegisterUserSink(pub Arc<dyn UserSink>);                 // ask → ()
pub struct Deliver { pub m: Outbound, pub via: Option<String> }     // ask → Result<Vec<String>, String>
```

`Deliver` runs §2 and returns the list of `where` strings (one per sink
that delivered). `MessageUserTool` (`NAME = "message_user"`, `Risk::ReadOnly`
— see §4) lives beside `SendMessageTool`:

```rust
pub struct MessageUserArgs {
    /// What to say. Markdown; Signal receives it as plain text.
    pub text: String,
    /// Absolute paths of files to attach. Images preview inline on both channels.
    #[serde(default)] pub files: Vec<PathBuf>,
    /// Restrict to one channel (`slack`, `signal`, `telegram`). Default: the
    /// channel this session is bound to, else every channel's owner.
    pub via: Option<String>,
}
```

Description (static, cache-safe): "Send a message, optionally with files
or images, to the user on their chat channel (Slack, Signal or Telegram). Arrives
immediately and out of band. Your normal reply already reaches the user —
use this only to hand over files or images, or when asked to report
progress while working. `files` are absolute paths of existing files."

Validation in the tool, before any sink is asked: `text` or `files`
non-empty; every path is a regular file, ≤ `MAX_FILE = 100 MiB` (Signal's
ceiling; Slack's is higher, one cap is enough), MIME by extension
(`mime_guess`, `application/octet-stream` fallback), `name` = file name.
Any failure → `ToolOutput::err`, nothing sent. Same posture as
`supervisor::check_files` (17 §2) for the inbound direction.

Ancestor chain: a new `Ancestors { session }` ask on `Agents` returning the
chain with names/titles (the tool needs the root's title for `origin`).
Alternative rejected: walking `ListSessions` rows in the tool — a full
session listing per call to answer a five-hop question.

## 4. Risk

`Risk::ReadOnly`. The recipient is always the user (or the user's own
thread, where the agent already posts every turn); the files are the
user's own disk. `Mutating` would put an approval prompt in front of
"send this to me" on every call under the default policy (12 §5) — on
Slack and Signal the prompt itself arrives as a message, so the user would
approve a message with a message. Precedent: `send_message` (peers) is
`ReadOnly` for the same reason. The contract stays one `Risk →
PolicyAction`; nothing special-cases this tool.

## 5. Slack (`21` §8.7 addition)

- `impl UserSink for channel::Handle` in `crates/slack/src/user.rs`.
  Bound: `chan.bound(session)` for each session in the chain →
  `(channel, thread_ts)`. Fallback: `identity.owner_dm`, no `thread_ts`,
  text prefixed `_<origin>_` on its own line.
- Files: `SlackApi::upload_file(channel, thread_ts: Option<&str>, files:
  &[(name, mime, bytes)], comment: Option<&str>) -> Result<String /*permalink*/>`
  — one `files.getUploadURLExternal` + `files.upload` per file, one
  `files.completeUploadExternal` listing all of them with `initial_comment`
  = the text (`markdown_text` is not accepted there; the comment is
  mrkdwn, so the text goes through the existing `markdown::Text` renderer,
  21 §4). `upload_text` (21 §8.3) becomes a one-line wrapper over it, so
  the thread-less form (`channel_id` without `thread_ts`, confirmed
  working by 21 §16 spike 6) is shared.
- No files: `post_markdown(channel, thread_ts, text)`.
- The message is a separate post below the live bubble, like `⟵ from` and
  `⚠️` (21 §8.7); it does not touch the stream. Result string:
  `slack: #<channel name or id> thread` / `slack: owner DM`.
- Mid-stream `chat.postMessage` into the same thread is fine (only
  `chat.update` on the streamed message conflicts, 21 §8.4).

## 6. Signal (`09` §8 addition)

- `impl UserSink for channel::Handle` in `crates/signal/src/user.rs`.
  Bound: `threads.number_for(session)` for each session in the chain.
  Fallback: `cfg.owner` — which `resolve_recipient` accepts only if the
  owner is a synced contact, the same README requirement 09 §5.2 already
  states; the error text is passed through.
- Text: `markdown_to_plain` + `send_chunks` (the 2 000-char chunker, 09
  §8), fallback pass prefixed with `origin`. Then one `send_attachment`
  per file, `caption = None` — the text went first so a caption would
  duplicate it. Images and audio show natively; other types show as a
  file with its name.
- **§2.2 hunk on the frozen transport**: `send_attachment` gains
  `file_name: Option<&str>` (trait, presage, mock, the presage unit test).
  jarvis never sent files so it hard-codes `file_name: None`; a nameless
  PDF is unusable on the phone. Recorded in 09 §2.2 like the two earlier
  hunks.
- `Channel::send_attachment(to, file, ..)` with the same `SEND_RETRIES`
  loop as `send_text`. Result string: `signal: +1555…` (the number the
  chain resolved to).
- The bound message arrives *before* the turn's own text, which Signal
  sends at `TurnFinished`. Acceptable and honest: it is out of band.

## 6a. Telegram (`98` §8.10)

- `impl UserSink for channel::Handle` in `crates/telegram/src/user.rs`,
  `name() == "telegram"`. Bound: the first session of the chain with a
  thread → its topic or chat. Fallback: the owner's chat (General), text
  prefixed `_<origin>_` on its own line, so the user knows which
  conversation is talking.
- Text as `send_rich` (plain fallback as 98 §8.1), then one `send_file` per
  file, no caption — `sendPhoto` for images, `sendDocument` otherwise
  (`sendVoice` is the voice-turn path, 45 §6a). Result string:
  `telegram: <chat title or @user>` / `telegram: owner`. Arrives below the
  live draft, out of band by design (§6). Details: 98 §8.10.

## 7. Rejected

- **A bus event both channels subscribe to** (`publish(UserMessage)`): zero
  new actors, but tell-only — the tool cannot say whether anything
  delivered, and a Signal-less, Slack-less headless run would report
  success into the void. The sink registry is ~60 lines.
- **Two tools, one per frontend** (`slack_send`, `signal_send`): the model
  must know where the user is; wrong for subagents and for a user on both.
  `via` covers the rare explicit case.
- **Reusing `ToolRegistry` for the sinks**: saves an actor, mixes gating
  with delivery; a `RegisterProvider`-style registry (16 §3) is the
  existing pattern for "plugins offer capabilities to a consumer".
- **`Mutating` risk** (§4).
- **Bytes on the wire** (`files: Vec<Vec<u8>>`): 17 §2 chose paths for
  inputs; outputs follow. The sink reads the file once.
- **TUI sink**: the tool call card already shows text and paths.
- **`initial_comment` split into text post + file post** on Slack: two
  messages for one thought; one upload call does both.

## 8. Checks

- `agent::user::tests`: bound pass stops at the first delivering sink;
  fallback fans out to every sink when nothing is bound; `via` restricts
  both passes; validation refuses a directory, a missing file, an
  oversize file, and empty `text` + `files`; result lists every `where`.
- `slack::user::tests` with the stub API (21 §15): bound session → thread
  post; child session with a bound parent → the parent's thread; fallback
  → owner DM with the origin line; two files → one `completeUploadExternal`
  with both ids and `initial_comment`.
- `signal::user::tests` with `MockTransport`: text then one
  `SentItem::Attachment` per file with `file_name`; fallback → owner;
  unbound + `fallback == false` → `Ok(None)`.
- Smoke (headless, real Slack workspace and phone): "take a screenshot of
  example.com and send it to me on Slack", then the same on Signal; an
  unbound RPC session's `message_user` lands in the owner DM and on the
  phone with the `session …` line.

## 9. Status (as-built, 2026-09-14)

Shipped: `crates/agent/src/user.rs` (contract types, `UserChannels` actor
registered `"user"`, `MessageUserTool`, `check_files`; 5 tests),
`Ancestors { session } -> Vec<AncestorInfo { id, title, name }>` on
`Agents` (live `parent` chain, cap 32, root title from sqlite);
`crates/slack/src/user.rs` (`impl UserSink for channel::Handle`; 6 tests
with the stub API) plus `SlackApi::upload_file(channel, thread_ts, &[UploadFile],
comment)` with `upload_text` now a provided wrapper over it;
`crates/signal/src/user.rs` (`impl UserSink for channel::Handle`; 3 tests
with `MockTransport`), `Channel::send_attachment` with the `send_text`
retry loop, and the third §2.2 hunk on the frozen transport
(`send_attachment(.., file_name: Option<&str>, caption)`). Docs per §5/§6
and 00 §6, phase 15, README, `harness.md`.

Deviations from the sections above:

- Slack `initial_comment` receives the text unconverted: the slack crate
  has no markdown→mrkdwn renderer (21 §1 dropped it; `markdown_text` made
  it unnecessary). Plain text and `_italic_` agree; headings and links do
  not. `ponytail:` comment in `slack/src/user.rs`.
- The Slack `where` string uses the channel id (`slack: #D0… thread`);
  `Thread` does not cache a channel name.
- Signal fallback fails fast with `signal: no owner configured` /
  `signal: not linked` before any send (the first smoke reported
  `send failed` for an unlinked smoke data dir — useless to the model).
- `browser_screenshot` now says `saved to <path>` (was `attached`): the
  first smoke watched the model run `find` and Python to locate the file
  it had just made before it could hand it over. One line in 23's
  `tools.rs`; the live test asserts the new text.
- `Ok(Vec<String>)` from `Deliver` carries sink failures as extra entries
  (`signal failed: …`) when another sink delivered, so the model sees both.

Smoke (headless, `claude-sonnet-4-5`, `/tmp/theuth-title` with the real Slack
app, no Signal link): "take a screenshot of example.com and send it to me
on Slack" from an unbound RPC session → `browser_open`,
`browser_screenshot`, `message_user { text, files: [shot-1.png] }`; result
`delivered via slack: owner DM, signal failed: signal: no owner configured`;
`conversations.history` on the owner DM shows the message
`_session 7dc76a6f · <title>_` + note with `shot-1.png image/png` attached.

Not exercised live: the bound pass (needs a human message in a Slack
thread to create the binding — unit-tested: bound → thread, child →
parent's thread), Signal end to end (mock-tested; the second process would
have to share the phone's presage store), `via`, a Slack channel thread
(vs DM), oversize files against Slack's real limit.
