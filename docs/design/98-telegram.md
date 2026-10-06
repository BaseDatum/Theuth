# 98 — Telegram: a phone-side frontend for the daemon, on the Bot API

Status: as-built v0.1 · 2026-10-05 (§20; v0 the same morning chose a teloxide Git pin; §0.1 was
revised after a library survey — knowledge `facts/telegram-rust-bot-library-survey-2026-10-05`)
· extends `00-architecture.md` §4, §7; `09-signal.md`
(sibling frontend: trust, threads, reactions, slash table and approval wording are reused by
reference); `21-slack.md` (sibling frontend: bubble, cards, board, approvals, panels, setup shape);
`24-message-user.md` (sink), `45-voice-notes.md` §4.5 (channel contract), `62-broadcast.md` §5.3,
`67-agents.md` §4.4/§4.7 (hub lease), `35-artifacts.md` §8 (`show` on a channel), `37-secrets.md`.
Where this doc disagrees with 09 or 21 about Telegram, this doc wins. Telegram facts were read
from core.telegram.org/bots/api (**Bot API 10.3**, 2026-08-24) and its changelog on 2026-10-05;
tgbot facts from `tgbot 0.48.1` (crates.io, 2026-09-25) and tg-rs/tgbot `master` the same day;
teloxide and frankenstein facts from crates.io and their repositories the same day. Every
`file:line` of theuth was read on `main` `e1e55598`.

Telegram is a frontend, like the terminal, Signal and Slack: it sends `SendInput` / `Steer` /
`Cancel` / `Resolve` and renders `AgentEvent`s. It runs **only in the daemon** (`t --headless`).
It is the first of the two phone channels whose transport can *edit*, *stream* and *thread*: the
Bot API of 2026 has message drafts (a streamed, animated preview with a native Stop button), rich
messages (the model's Markdown rendered by Telegram itself — tables, headings, fences, collapsible
blocks, 32 768 characters), topics inside a private chat, inline keyboards, reactions, native
command menus and ephemeral replies in groups. The rule from 21 holds: *Telegram's primitive
first, our message second*.

Scope: everything a user can do from Signal (09) or Slack (21) they can do from Telegram, with the
same names and the same meaning — turns, steering, cancel, approvals, `ask_user`, slash commands,
session switching, subagent threads and the board, `message_user`, `show`, voice notes in and
spoken blocks out, secret-request notices, broadcast/communication/peers/requests/a2a (owner),
status over RPC and a tool, restart and lease survival. Parity is of *capability*; the mechanism is
whatever Telegram does natively (buttons where Signal has codes, drafts where Slack has a stream).

Not in this doc: Telegram as a data source (reading other people's groups, channels, history) —
theuth has no Telegram read tools; a Mini App UI; payments, Stars, gifts, business accounts,
inline mode, guest mode; webhooks (§0.2); Telegram as a *bot platform* for other users (one bot,
one owner, a handful of allowed people, 09 §5).

Names: **bot** = the Telegram bot account the daemon drives (token from @BotFather). **owner** =
the Telegram user whose id is `telegram.owner`. **chat** = a private chat, group or supergroup
(`ChatId`, `i64`). **topic** = a forum thread inside a chat (`message_thread_id`); since Bot API
9.3 a bot can have topics in its *private* chats. **thread key** = `<chat>` or `<chat>:<topic>`,
the unit a session binds to (§6). **draft** = `sendRichMessageDraft`, Telegram's ephemeral
streamed preview (§8.2). **bubble** = one persisted prose block of a turn. **card** = the activity
message of one tool batch (§8.4). **board** = the parent's live table of children (§9.3).

## 0. Decisions in one screen

Every decision names the alternative it beat (00 line 9–10).

1. **Library: `tgbot 0.48.1` (tg-rs), exact pin on crates.io, no default features.** It is the
   only maintained Bot API client that is both *current* and *built for a daemon*: Bot API 10.3
   landed in it on 2026-09-06 (every 2026 Bot API release within ~2 weeks), it is on `reqwest
   0.13` with rustls (the workspace's, `Cargo.toml:168`), its `Client` retries `retry_after`
   itself, downloads files, scrubs the token out of HTTP errors, and its `UpdateType::Unknown
   (JsonValue)` keeps the poller alive when Telegram ships an update kind it does not know.
   Everything §8–§10 needs — drafts with `can_stop`, rich messages, `stopped_message_generation`,
   ephemeral parameters, topics in private chats, `force_reply` keyboards, disabled buttons,
   command scopes with `is_ephemeral` — is typed in the crate (§2.2), so there is no bridge
   module and no fork. Rejected: **teloxide** (crates.io 0.17.0 is Bot API 9.1 on `reqwest
   0.12`; `master` stops at 9.2, the `sendMessageDraft` PR #1443 was closed unmerged in March
   2026 and the 9.4/9.5 PR #1447 has been open since — a Git pin plus a ~250-line hand-written
   payload module was v0's plan and it rebuilt half the API surface Telegram shipped this year;
   its `Throttle` and typed `ApiError` variants are the two things lost, §4.2–4.3 say what
   replaces them); **frankenstein 0.52.1** (also 10.3, more contributors, but a bare client — no
   polling, retry or download helpers — and a flattened `UpdateContent` with no unknown-kind
   fallback, so a Bot API 10.4 update type would wedge `getUpdates` until an upgrade; WTFPL);
   **telers** (1.0.0-beta, an aiogram-style framework with dispatcher and FSM — more than a
   channel needs, and beta); **grammers**/**ferogram** (MTProto user clients — a bot is the
   right identity); a hand-rolled `reqwest` client (00 §1.2). tgbot's risks are a bus factor of
   one (one maintainer since 2019, 0 open issues, CI + coverage) and ZeroVer churn (0.47 renamed
   much of its surface): the exact pin and the one-module façade (`api.rs` is the only file that
   names a tgbot type) contain both.
2. **Long polling, not webhooks.** A webhook needs a public HTTPS URL; a daemon can keep a long
   poll up (the Socket Mode argument, 21 §1). `getUpdates` is **single-consumer** — a second
   poller on the same token gets `409 Conflict: terminated by other getUpdates request` — so
   polling is a **hub duty that follows the lease** exactly like the Slack socket (67 §4.4,
   `crates/slack/src/channel.rs:671-699`), *not* like Signal's every-engine-listens. Telegram
   keeps unconfirmed updates for 24 h and our loop confirms the offset when it stops (§3.1), so
   a lease handover loses nothing and the double-hub window is deduplicated with the replicated
   `tasks::MarkAnswered` marker Signal already uses (`crates/signal/src/channel.rs:1138-1150`).
   Rejected: polling on every engine (409 storms with exponential backoff on both sides, so half
   the updates would wait minutes).
3. **Ids, not names; pairing by deep link.** `telegram.owner` and `telegram.allowed` are numeric
   user ids (usernames change and may be absent). Nobody types an id: `t --telegram-setup`, the
   `telegram_link` tool and `/telegram link` mint a one-time code and print
   `https://t.me/<bot>?start=<code>` plus a QR (`link::qr`, the 09 §11 shape); the first
   `/start <code>` within 10 minutes becomes the owner and the change applies **live** (09 §16's
   open "live owner change" is closed here). Rejected: `@username` arguments (the Bot API cannot
   resolve a username to an id without a prior message), "copy your id from @userinfobot".
4. **Threads are topics.** With **Topics** enabled for the bot in @BotFather (`getMe.
   has_topics_enabled`, Bot API 9.3) the private chat is Slack-shaped: **one topic per session,
   one topic per subagent**, the General area for commands and new sessions, topic names kept in
   step with session titles. Without topics the private chat is Signal-shaped: one live session,
   `/new`, `/resume`, subagents inline. Forum supergroups in `telegram.groups` behave like the
   private topics chat; plain groups like the no-topics chat, on `@mention` or reply-to-bot (§6).
   Rejected: Telegram reply chains as threads (invisible, no title, no list), one bot per session,
   a required supergroup (a second chat to set up when the private chat can already do it).
5. **Stream with drafts, persist with rich messages.** While the model writes, the thread shows
   a `sendRichMessageDraft` — animated in place, `<tg-thinking>` while reasoning, a native **Stop**
   button (`can_stop`) whose press arrives as `stopped_message_generation` → `Cancel`. At every
   block boundary that ends a bubble (the first tool call of a batch, the end of the turn, the size
   cap) the text is persisted once with `sendRichMessage { markdown }`: **the model's Markdown
   verbatim**. Rich Markdown is GitHub-flavoured (headings, fences with language, tables, task
   lists, quotes, footnotes), 32 768 characters and 500 blocks per message, and may embed
   `<details>`, `<blockquote expandable>` and `<tg-button>`. Rejected: streaming by
   `editMessageText` (1 msg/s/chat and 4 096 plain characters — kept only as the fallback for
   groups and repair, §8.3); MarkdownV2 (escape every `.` and `-`; the renderer we spent 21 §0
   deleting); an HTML converter of our own (a second grammar beside `markdown`, 00 §1.2).
6. **Tool observability is one activity card per tool batch**, not one message per call and not
   a Slack timeline (Telegram has none): a rich message with one `<details>` per call — collapsed
   `✓ shell · cargo test -p agent · 1.2 s`, expanded the described args and the first 2 500
   characters of output — edited as calls progress, with the full output of a long result one
   button away (`out:<call_id>` → `sendDocument`, sent on demand, never spammed). Reasoning is
   `<tg-thinking>` in the live draft and an `<blockquote expandable>` in the persisted bubble
   (`telegram.reasoning`). Rejected: a message per tool call (21 §8.2's update mode — twenty
   calls are twenty bubbles on a phone); tool cards inside the prose bubble (a 10-minute batch
   would keep a draft alive that Telegram expires after 30 s).
7. **Status is reactions + typing + Stop.** The 09 §8.2 three-state reaction on the user's
   message, mapped onto Telegram's **fixed** reaction set (73 emoji; ✅ ❌ 🛑 are not in it):
   👀 accepted → 👌 done / 🤷 cancelled / 😱 error. `sendChatAction typing` every 4 s between
   drafts (the status lives 5 s). Topic names follow session titles (`editForumTopic`) unless the
   user named the topic. Rejected: a status message edited in place (noise; the draft already
   shows liveness).
8. **Approvals are inline keyboards** `[Allow] [Deny]` with `callback_data = ap:<ApprovalId>:a|d`
   (≤ 64 bytes; self-describing, so a card from before a restart still works, 21 §10.1). Who may
   press: the owner or the thread's user. Resolved from anywhere → the card is edited to the verdict
   and its buttons become `DisabledButton`s. Approvals of sessions no Telegram thread owns go to the
   **owner's chat** with a `session <last 8> · <title>` context line (`telegram.approvals: all |
   threads`). Rejected: short codes (09's path exists because Signal has no buttons), reactions
   (a second path for the same press; and 👍 is not distinguishable from "I liked that").
9. **`ask_user` is one inline keyboard per question** — a button per option (≤ 8 per row), Skip,
   Dismiss — with `InlineKeyboardMarkup.force_reply` (Bot API 10.3) so a typed answer arrives as a
   Telegram *reply to the card* and is attributed without ambiguity. Same card lifecycle as
   `crates/slack/src/asks.rs` (answered / closed / dismissed / timed out).
10. **Commands are native bot commands** (`setMyCommands`), the 09 §9 table with the same names —
    Telegram shows them in the `/` menu with completion, so the phone needs no prefix-matching of
    its own. Three scopes: everyone gets the public set, the owner's chat gets the owner-only ones
    too, groups get a short set flagged `is_ephemeral`. Replies are rich messages with inline
    keyboards for anything the user would otherwise have to type an id into (`/model`,
    `/sessions`, `/agents`, `/skills`, `/mcp`, `/login`, `/approvals`) — the 21 §10.2 panels, edited
    in place on `cmd:` presses. In groups, replies are **ephemeral** (visible only to the invoker)
    where the API allows it. Not carried: `/approve`, `/deny` (buttons), `/signal`, `/slack`;
    added: `/telegram`, `/start` (pairing and help).
11. **Trust is the 09 §5 list.** `telegram.owner`, `telegram.allowed`; `telegram.groups` are the
    chats where mentions and commands are honoured. A stranger in a private chat is dropped with one
    `warn!` and **no reply**; a non-allowed member in an allowed group gets an *ephemeral* `not
    allowed` when the API gives us a way to send one (within 15 s of their command, or any time when
    the bot is an admin), else silence. Ids only; owner-only commands as 09.
12. **Files.** Inbound photos, documents, voice and video notes are fetched with `getFile`
    (≤ 20 MB — larger files get a line saying so) into `telegram/inbox/`, announced with the 09
    §7.4 `[attachment: …]` line; images and PDFs ride in `files`; voice notes (OGG/Opus — symphonia
    demuxes them, `crates/voice/src/lib.rs:92-93`) are transcribed into `[voice note, N s] …` per
    45 §4.5. Outbound: `sendDocument` ≤ 50 MB, `sendPhoto` for images ≤ 10 MB, `sendVoice` for
    the 45 clips (MP3 is accepted and renders as a voice bubble). `message_user` (24) and `show`
    (35 §8) land exactly where they do on Signal, with `via: "telegram"`.
13. **Secrets never travel through Telegram.** `SecretsEvent::Requested` → the 09 notice to the
    owner (`signal::channel::secret_notice`'s text, 37); a bot chat is a cloud chat and an update
    log, not a vault.
14. **Setup is BotFather + one command.** `t --telegram-setup` validates the token (`getMe`),
    stores it in the vault as `telegram_bot`, registers the command menus, prints the deep link +
    QR, waits for the owner's `/start <code>`, writes `telegram.owner` / `telegram.enabled: true`,
    and prints the two @BotFather switches that matter (Topics on; privacy off or admin for
    groups). No manifest (Telegram has none); re-runnable and idempotent like `--slack-setup`.
15. **Budget ~2 800 lines plus tests.** 21 planned 2 000 and shipped 9 800; the seams here are
    pre-cut (§2) so `channel.rs` does not become 3 800 lines again, and the library carries the
    whole API surface, so nothing here is a type table.

## 1. Goal, and what is not one

Goal: talk to the daemon from Telegram — the private chat with the bot, or a group it was added
to — and get the same agent the terminal gets, rendered with Telegram's own primitives: a live
streamed draft with a Stop button, the final answer as a rich message in the model's own Markdown,
one collapsible card per tool batch, approval and question buttons, reactions for turn state, a
topic per session and per subagent with a live board in the parent, the native command menu, and
`message_user` / `show` / voice clips arriving in the right thread. Every Bot API failure is
visible (bus event, status counters, log) and never silent.

Not goals, with the reason:

- **Webhooks.** §0.2. The daemon has no public URL and the relay (31) is rendezvous only.
- **Running in TUI mode.** One token, one poller (§0.2). The terminal process never polls; the
  actor answers `DaemonOnly` (09 §3 shape).
- **Telegram as a data source.** No read tools; when one exists it gets its own doc. `my_chat_member`
  is consumed only to notice removal from a group.
- **Several owners / a bot for the public.** 09 §5: a bot that answers strangers advertises
  itself. One owner, a list of allowed ids, a list of allowed groups.
- **MTProto.** Bots are second-class in MTProto and first-class in the Bot API; nothing we need is
  Bot-API-only missing.
- **Mini Apps, inline mode, payments, business accounts.** Product surface, not a frontend.
- **Plan mode.** As 21 §1: `TodoChanged` is published today (`crates/slack/src/channel.rs:2374`
  lists it) but no channel renders it; a task-list message is a 20-line follow-up (§17).

## 2. Layout

```
crates/telegram/
  Cargo.toml
  src/
    lib.rs        plugin, `telegram` actor + messages, config, TelegramEvent/TelegramStatus, tools   (~300)
    api.rs        façade over `tgbot::api::Client`: our methods, the pacer, counters, exact
                  description matching; with `poll::classify` the only code that names tgbot types   (~400)
    poll.rs       lease-gated getUpdates loop (ours, §3.1), backoff, dedup, update classification     (~250)
    channel.rs    inbound routing, per-thread workers, owner chat, event fan-out                     (~700)
    bubble.rs     Draft | Edit mode over markdown::Text, rotation, repair, footer                     (~350)
    cards.rs      activity cards (tool batches), output-on-demand, reasoning quote                   (~250)
    threads.rs    thread key ↔ session map + topics, `threads.json`                                  (~150)
    commands.rs   native commands → asks → replies; session resolution                              (~400)
    ui.rs         rich-message + inline-keyboard builders for the panels; `cmd:` routing             (~300)
    approvals.rs  approval cards and presses                                                         (~120)
    asks.rs       ask_user cards, presses, forced replies                                            (~200)
    board.rs      the parent's board message                                                         (~150)
    user.rs       `impl agent::UserSink` for the channel (24)                                        (~120)
    setup.rs      `t --telegram-setup`, pairing codes                                                (~250)
~/.theuth/telegram/
  threads.json    { threads: { "<chat>[:<topic>]": { session, user, title, topic? } }, open: [ { chat, thread, message_id } ] }   (§6, §11)
  inbox/          inbound files, `<ts>-<name>` (09 §7.4)
```

`channel.rs` past 900 lines splits at the seams already named (owner chat, groups), not before.

### 2.1 Dependencies

```toml
# workspace Cargo.toml [workspace.dependencies]
# Telegram Bot API client (98 §0.1). ZeroVer: exact pin, bumped by hand against its CHANGELOG.
tgbot = { version = "=0.48.1", default-features = false }
```

What the pin gives, checked in tg-rs/tgbot `master` on 2026-10-05 (0.48.1 is its head): the
whole Bot API 10.3 method and type set as one struct per method executed by
`Client::execute(method) -> Result<M::Response, ExecuteError>` (`src/api/client.rs`), with
`retry_after` honoured inside `execute` (`Client::with_max_retries(u8)`,
`with_max_retry_after(u64)` cap), `Client::download_file(file_path)` for `getFile` results,
`Client::with_host(url)` for tests, and HTTP errors passed through `sanitize_http_err` so the
token in the URL never reaches a log. The pieces §8–§10 lean on, by name: `SendRichMessage`,
`SendRichMessageDraft` (`draft_id`, `message_thread_id`, `can_stop`, `keep_on_stop`),
`SendMessageDraft`, `InputRichMessage::{markdown, html}`, `EditMessageText::for_chat_message_rich`
(`src/types/rich/message.rs`, `src/types/message/methods.rs`); `EphemeralMessageParameters`
(`src/types/ephemeral.rs`); `UpdateType::StoppedMessageGeneration(MessageGenerationStopped)` and
the `#[serde(untagged)] UpdateType::Unknown(JsonValue)` fallback, `GetUpdates::{with_offset,
with_limit, with_timeout, with_allowed_updates}` (`src/types/update.rs`); `GetBot` → `Bot {
has_topics_enabled, allows_users_to_create_topics }`, `SetBotCommands` with `BotCommandScope`
and `BotCommand.is_ephemeral` (`src/types/bot.rs`); `InlineKeyboardMarkup::with_force_reply`,
`InlineKeyboardButton::disabled` (`src/types/reply/markup/inline_keyboard.rs`);
`SetMessageReaction` / `ReactionType::Emoji` (`src/types/reaction.rs`); `SendChatAction` with
`message_thread_id` (`src/types/chat/action.rs`); `CreateForumTopic`, `EditForumTopic`,
`CloseForumTopic`, `ForumTopicCreated.is_name_implicit` (`src/types/forum.rs`); `GetFile`,
`SendDocument`, `SendPhoto`, `SendVoice`; `AnswerCallbackQuery` with `show_alert`;
`DeleteWebhook`. Edition 2024, MIT, `reqwest 0.13` with `rustls`/`json`/`multipart`/`stream`
(no default features of its own; `webhook` and `webpki-roots` are the only optional ones).

TLS: `reqwest/rustls` through tgbot; `main.rs:1200-1202` already installs the `aws-lc-rs`
provider for the Slack/relay mix, so nothing new. Other deps: `serde_json`, `uuid`, `chrono`,
`mime_guess`, `qrcode` through `link::qr`. No second HTTP client: downloads go through
`Client::download_file`.

### 2.2 What the pin lacks, and the ZeroVer rule

Three things teloxide had and tgbot does not, each answered inside `crates/telegram`:

| missing | answer |
|---|---|
| a rate-limit adaptor (`Throttle`) | §4.2's `pacer` — it had to exist anyway, because no crate throttles drafts, edits or reactions, which is most of this channel's traffic; `carapax`'s `governor` middleware is for its handler framework, not a client |
| typed error variants (`ApiError::MessageNotModified`, …) | `ExecuteError::Response(ResponseError { description, error_code, parameters })` matched by **exact `description` string** in `api.rs` (§4.3); teloxide's `errors.rs` is kept as the reference list of Telegram's verbatim descriptions |
| a polling stream that confirms the offset on graceful stop | our own loop in `poll.rs` (§3.1): tgbot's `LongPoll` spawns a task per update (unordered) and its shutdown only breaks the loop, so it is not used |

Nothing is bridged: the design uses no Bot API method the crate lacks. If Telegram ships 10.4
before this lands, an unknown update kind arrives as `UpdateType::Unknown` and is counted (§7),
and a new *method* would be one `tgbot::api::Method` impl in `api.rs` until the next release —
the same shape as the crate's own methods, no fork.

**ZeroVer.** tgbot bumps the minor on every Bot API release and reshapes its API when it wants
to (0.47.0 moved form serialisation into `Method::into_payload`, removed factory methods and
constructors across the board). The workspace pins `=0.48.1`; a bump is a deliberate commit
that reads the changelog, and `api.rs` plus `poll::classify` are the only code that names tgbot types (`channel`,
`bubble`, `cards`, `commands` see our `Target`/`MsgRef`/`Keyboard`/`Incoming` types), so a
rename costs two files.

Limits the design relies on (Bot API, 2026-10-05): rich message 32 768 UTF-8 characters, 500
blocks, 16 nesting levels, 20 table columns; plain `sendMessage` 4 096; caption 1 024;
`callback_data` 1–64 bytes; `getFile` 20 MB; `sendDocument`/`sendVoice` 50 MB; `sendPhoto`
10 MB and ≤ 10 000 px summed; a draft is a "temporary 30-second preview", private chats only,
animated when the same `draft_id` is re-sent; `sendChatAction` lasts 5 s; `/start` payload
≤ 64 chars of `A-Za-z0-9_-`; command names 1–32 chars of `a-z0-9_`; updates are kept 24 h.

## 3. Plugin lifecycle

`main.rs` pushes `Box::new(telegram::TelegramPlugin { channel: cli.headless })` after `slack`
(`main.rs:1455`, `:1548`). `start`:

1. Read `telegram:` config (§12); `enabled: false` → log, return. Read secret `telegram_bot`
   from the vault (`secrets::Handle`, 37 §3.6); missing → `warn!` naming the key, spawn the actor
   in `Unconfigured`, return. The harness comes up regardless (plugins.md §3.2).
2. `channel == true`: `channel::spawn(cfg, token, rt)` → `Handle`; runs the supervisor under
   `rt.shutdown`. `channel == false`: no poller, no API client beyond what `telegram_link` needs.
3. Spawn the `Telegram` actor (`handle: Option<Handle>`), register as `"telegram"`; register
   tools `telegram_link` / `telegram_status` via `lookup_when::<ToolRegistry>`; register the
   `UserSink` with `lookup_when::<agent::UserChannels>(agent::USER_ACTOR)` (24 §3, as
   `crates/slack/src/lib.rs:421-424`).

**Revision 2026-10-06 (98c, `98-as-built-FIXES.md`).** The order is now 3 → 1 → 2: the
`Telegram` actor is spawned and registered *synchronously in `start`* with `handle: None`, so
`telegram_status` answers from the first RPC; the vault read runs on a task and feeds the actor a
`TokenRead`, which is the same path as the §10.5 hot start (`hot_start`). Nothing network-bound
(getMe, `setMyCommands`, `repair_open`) is ever awaited inside an actor handler — the supervisor
owns it. Before this, `start` awaited the vault (up to `secrets::HANDLE_WAIT` = 10 s) *before*
registering, and the Settings card errored with `telegram is disabled` meanwhile. Until the
`TokenRead` arrives the actor's `configured` is unknown (`Option<bool>` = `None`) and `Status`
reports `TelegramEvent::Starting` — not `Unconfigured`, which would flash "no bot token" and the
token input on a configured bot (r2); `Link` answers `telegram is starting` meanwhile.

### 3.1 The supervisor (`poll::run`)

```
identity: GetBot with backoff_delay until ok → BotIdentity { id, username, topics, users_may_create_topics }
          publish TelegramEvent::Identity; DeleteWebhook (a webhook set by a previous life makes getUpdates 409)
commands: SetBotCommands × 3 scopes (§10.3), once per boot, idempotent; failures logged, not fatal
boot repair: every entry in threads.json.open → edit it to append `· interrupted` (best effort), clear the list (§11)
lease:    roster::lease::Follower (67 §4.4); holding && identity → start the poller task; lost → stop it
          (the stop path below confirms the offset) and publish Disconnected { reason: "hub lease held elsewhere" }.
          Idempotent, like slack::Channel::sync_socket.
poller (ours, ~60 lines; §2.2 says why not tgbot's LongPoll):
  loop:
    GetUpdates { offset: last + 1, limit: 100, timeout: 30 s, allowed_updates: [] } via client.execute
      `allowed_updates: []` = Telegram's default set (everything but chat_member and the two reaction kinds),
      which includes stopped_message_generation; an empty list also resets a narrower set left by a previous instance
    Ok(batch) → attempt = 0; first Ok after a (re)start publishes Connected { bot }
                per update: last = max(last, id); RecentKeys(id) ? drop : inbound_tx.send(classify(update))
    Err(Response { description == CONFLICT }) → publish Conflict (another poller on this token: a second daemon,
                a stale setup run, or the double-hub window); sleep backoff_delay(attempt)
    Err(other) → publish Disconnected { reason }; sleep backoff_delay(attempt) = min(2^min(attempt, 6), 60) s
                (the 09 §3.1 constants; a `retry_after` was already slept inside `execute`)
    stop requested (lease lost, shutdown) → one GetUpdates { offset: last + 1, limit: 1, timeout: 0 } confirms
                what was consumed (what teloxide's graceful stop does), then return
```

`RecentKeys` is the 1 024-entry FIFO set from 21 §3.1 keyed by `update_id` (Telegram never
re-sends a confirmed update, but a crash between `getUpdates` and `SendInput` re-delivers the
batch; the set makes that a steer, not a double turn — and the set is lost on restart, so the
*replicated* marker below covers the rest). Inbound is one ordered `mpsc` worker
(`channel::inbound`), so a slow agent ask never blocks the poll loop; ordering is the reason the
loop is ours — tgbot's `LongPoll` hands each update to a spawned task.

### 3.2 The hub lease, and what Telegram adds to 67 §4.4

The poller runs **only** on the engine holding the hub lease. Outbound is not gated: a session
running on a non-hub engine that calls `message_user`, or whose `TurnFinished` the bubble must
persist, sends through its own `Bot` — Telegram does not care which process posts (67 decision
22, the same rule Signal applies at `channel.rs:1138`). Two engines each believing they are hub
(a partition, or the 30 s heartbeat gap) both poll: each `getUpdates` terminates the other's
(`409`), both back off, and because neither confirmed the offset before the other read, both
can see the same update. Before the ask, inbound calls `tasks::MarkAnswered { sender:
"<chat>:<user>", timestamp: update_id }` (the 67 §4.7 marker; `Ok(false)` = another hub already
took it → drop silently; `None`/`Err` must not mute Telegram, exactly `signal/channel.rs:1144-1150`).
A graceful lease release stops the poller first, so the normal handover has no duplicate at all,
and Telegram's 24 h retention means a handover gap loses nothing — strictly better than Slack's
short redelivery window (67 §4.6).

## 4. Bot API (`api.rs`)

### 4.1 One façade

```rust
pub struct Api { client: tgbot::api::Client /* with_max_retries(2).with_max_retry_after(30) */, me: OnceLock<BotIdentity>,
                 pacer: Mutex<HashMap<PaceKey, Instant>>, calls: Mutex<BTreeMap<&'static str, CallStats>> }
impl Api {
    pub async fn send_rich(&self, to: &Target, md: &str, markup: Option<Keyboard>, reply_to: Option<MessageId>) -> Result<MessageId>;
    pub async fn edit_rich(&self, at: &MsgRef, md: &str, markup: Option<Keyboard>) -> Result<()>;
    pub async fn draft(&self, to: &Target, draft_id: i64, md: &str, can_stop: bool) -> Result<()>;
    pub async fn send_text(&self, to: &Target, text: &str) -> Result<MessageId>;          // plain 4 096 fallback, no parse_mode
    pub async fn send_ephemeral(&self, chat: ChatId, user: UserId, cb: Option<&str>, md: &str) -> Result<()>;
    pub async fn edit_markup(&self, at: &MsgRef, markup: Keyboard) -> Result<()>;
    pub async fn react(&self, at: &MsgRef, emoji: Option<&str>) -> Result<()>;            // None clears
    pub async fn typing(&self, to: &Target) -> Result<()>;
    pub async fn answer_callback(&self, id: &str, toast: Option<&str>) -> Result<()>;
    pub async fn send_file(&self, to: &Target, f: &OutFile, caption: Option<&str>) -> Result<MessageId>;   // photo | voice | document by mime/size
    pub async fn download(&self, file_id: &str, max: u64) -> Result<(Bytes, Option<String> /*file_path name*/)>;
    pub async fn create_topic(&self, chat: ChatId, name: &str) -> Result<ThreadId>;
    pub async fn rename_topic(&self, chat: ChatId, topic: ThreadId, name: &str) -> Result<()>;
    pub async fn set_commands(&self, scope: Scope, cmds: &[(&str, &str)]) -> Result<()>;
    pub async fn me(&self) -> Result<BotIdentity>;
}
pub struct Target { chat: ChatId, thread: Option<ThreadId> }     // `message_thread_id` on every send/draft/typing
pub struct MsgRef { chat: ChatId, id: MessageId }
pub enum ApiError { Api(&'static str /* interned description */), RateLimited(Duration), Transport(String), TooBig }
```

Every call is a `tracing` span `telegram.api{method}` and bumps `calls[method].{ok,err,rate_limited}`
for `Status`. A `retry_after` from any method is slept and retried inside `Client::execute`
(twice, capped at 30 s); one that still comes back returns `RateLimited` and publishes
`TelegramEvent::RateLimited { method, retry_after_secs }`. Every method the façade sends is one
of tgbot's structs; `api.rs` converts between them and the channel's own `Target` / `MsgRef` /
`Keyboard` / `Incoming` types, so no other module imports `tgbot`.

### 4.2 Pacing

Telegram's documented limits (bots FAQ): ~30 messages/s overall, 1 message/s sustained per
private chat, 20/min per group, `retry_after` seconds on excess. tgbot has no rate limiter
(`carapax`'s is handler middleware), and the one teloxide had covered only plain sends — never
drafts, edits, reactions or chat actions, which is most of what this channel does. So `pacer`
is the whole budget: a minimum gap per key across all workers, plus one overall token bucket:

| key | gap | why |
|---|---|---|
| `Chat(chat)` — any call that creates or edits a message | 1 000 ms | the per-chat rate; groups additionally 3 000 ms (20/min) |
| `Draft(chat, thread)` | 1 000 ms | one animated preview update per second; more adds nothing on a phone and courts `retry_after` |
| `Edit(message)` | 1 500 ms | the 21 §8.2 `chat.update` cadence; Telegram counts edits against the per-chat rate |
| overall | 30/s bucket | the global broadcast limit |
| reactions, typing, callback answers, topics, `getFile` | 0 | cheap and rare |

A `retry_after` that arrives anyway (Telegram's limits are approximate) is handled by
`Client::execute` (§4.1). A turn costs: 1 draft/s while text streams, 1 `sendRichMessage` per
bubble, 1 send + ~1 edit per 2 s per activity card, 2 reactions, 1 topic rename at most, 1
`sendChatAction` per 4 s between drafts. A 20-tool, 10-minute turn is ~250 calls, all inside the
per-chat budget.

### 4.3 Error codes

Telegram errors are `ok:false` with an `error_code` and a `description` string; tgbot hands
them over as `ExecuteError::Response(ResponseError { description, error_code, parameters })`.
The façade matches `description` **exactly** (21 §4.1 rule: never by substring) against a
`const` table whose strings are Telegram's verbatim texts — teloxide's `errors.rs` enumerates
them and is the reference — with two prefix matches where Telegram appends detail
(`Bad Request: message is not modified: …`, `Too Many Requests: retry after N`, the latter
already consumed as `retry_after()`). The ones the design branches on:

| description | branch |
|---|---|
| `Conflict: terminated by other getUpdates request; make sure that only one bot instance is running` | §3.1 `Conflict` |
| `Bad Request: message is not modified: …` | an edit that changed nothing: ignored |
| `Bad Request: message to edit not found` / `Bad Request: message can't be edited` | the message is gone or too old → the bubble rotates (§8.5) |
| `Forbidden: bot was blocked by the user` / `Forbidden: user is deactivated` / `Bad Request: chat not found` / `Forbidden: bot was kicked from the supergroup chat` | unbind the thread, `warn!`, `TelegramEvent::Unreachable { chat }` |
| `Bad Request: TOPIC_CLOSED` / `Bad Request: message thread not found` | the user deleted or closed the topic: unbind; the next message makes a new one |
| `Bad Request: not enough rights …` on `createForumTopic` | no `can_manage_topics` in the group: the flat shape for that chat, logged once |
| any other `Bad Request` on `sendRichMessage` / `sendRichMessageDraft` | the Markdown or the draft was refused → plain fallback / Edit mode and `StreamFallback` (§8.3); the exact text is logged and reported in §16.3 |
| `Bad Request: file is too big` on `getFile` | §7.1's too-large line |
| `Unauthorized` (401) | the token was revoked: publish `Disconnected { reason }`, stop polling until restart, status says so |

Anything unmatched is counted under its `error_code` in `Status.calls` and logged with the
description, so a new Telegram string shows up in `/status` before it becomes a bug report.

## 5. Trust

`telegram.owner` (one user id) and `telegram.allowed` (more). `telegram.groups`: the group and
supergroup ids where the bot listens; empty = private chats only. A private-chat message from a
user not in the list is dropped with one `warn!` and no reply (09 §5.1). A message in an unlisted
group is ignored. A non-allowed member of a listed group who mentions the bot or sends it a command
gets one ephemeral `not allowed` when the API allows it — within 15 s of a command or callback
(`callback_query_id` / the command's ephemeral id) or at any time when the bot is an admin of the
group — else nothing. Owner-only: the 09 §9 `OWNER_ONLY` set with `/telegram` in place of
`/signal`; owner-chat cards (§10.1). Buttons in a thread: the owner or the thread's user. Ids,
never names; a `from.is_bot` sender is dropped before any other check (bots talking to bots is
10.0's bot-to-bot feature and not this design).

The owner is special in one more way: `/start <code>` from *anyone* while a pairing code is live
makes them the owner (§10.4) — that is the whole pairing protocol, so codes are 16 characters from
the 09 §10 alphabet, single-use, 10 minutes, and minting one is owner- or CLI-initiated.

## 6. Chats, topics and sessions (`threads.rs`)

One thread key ↔ one session: `Thread { session, user, title, topic: Option<Topic { id,
named_by_user }> }` keyed by `"<chat>"` or `"<chat>:<topic>"`, in `telegram/threads.json`,
written on every change (09 §6 / 21 §6 shape; `Threads::bind/unbind/by_session/recipient_for`
plus the `open` list, §11). The chat's **shape** — topics or flat — is decided per message from
`Chat.is_forum` (groups) and `BotIdentity.topics` (private chats), never configured.

### 6.1 Private chat with topics (the default once Topics is on in @BotFather)

- A message in the **General** area (no `message_thread_id`) that is not a command starts a
  session: `SendInput { session: None }`, then `createForumTopic { name: first line ≤ 128 }`
  in the same chat, bind `<chat>:<topic>`, and every reply — drafts, bubbles, cards, approvals —
  goes into that topic (`message_thread_id` on every send). General itself gets nothing but
  command replies and owner cards; the client lists topics with unread counts, so the user
  finds the answer where Telegram puts new threads.
- A message in a **topic the user created** (`allows_users_to_create_topics`) binds that topic
  on first use; its name is theirs (`named_by_user: true` — the `ForumTopicCreated.is_name_implicit`
  flag says whether the client derived it from the text) and is never rewritten.
- A message in a **bound topic** → `Steer` while a turn runs, else `SendInput` (09 §7 rule).
- `/new` in a topic → unbind it (the topic stays as history); the next message there starts a
  fresh session in the same topic. `/new` in General is a no-op with the reply `send a message to
  start a session; it gets its own topic`.
- `/resume <id>` in a topic → rebind that topic to the session (full id, unique prefix or
  suffix, 09 §6); in General → `createForumTopic` named after the session's title, bind, reply
  `resumed <id> · <title>` inside it.
- Subagents: `SubagentSpawned { parent, child, name }` with a bound parent → a new topic in the
  same chat, bound to the child (§9.1).
- A topic the user deleted or closed (`Bad Request: message thread not found` / `TOPIC_CLOSED` on
  a send, or the `forum_topic_closed` service message) → unbind; the session lives on in sqlite.
- Restart → the file is read (missing or corrupt → empty). Workers, bubbles, cards, drafts and
  the dedup set are process-local (§11).

### 6.2 Private chat without topics

The 09 shape: key = `<chat>`; the first message binds a new session; `/new` unbinds; `/resume`
rebinds; subagents render inline in the one chat (§9.2). A chat that later gains topics (the
owner flips the switch; `getMe` is re-read on the next boot) keeps its flat binding until `/new`.

### 6.3 Groups

- **Forum supergroup** in `telegram.groups` (bot added as admin with `can_manage_topics`, which
  also lifts privacy mode): exactly §6.1 with the group as the chat — an unbound topic needs an
  `@bot` mention or a command to start; a bound topic needs neither; General behaves as above.
  Without the right, `createForumTopic` fails and the chat falls back to §6.3's plain shape with
  one `warn!`.
- **Plain group** in `telegram.groups`: key = `<chat>`, one live session per group. Every input
  must be an `@bot` mention, a command, or a **reply to one of the bot's messages** (Telegram's
  thread gesture; the client shows the quoted bubble) — unaddressed chatter in a group is never an
  input even when the bot sees it. `<@bot>` text is stripped like Slack's `<@U…>`.
- Privacy mode: a non-admin bot with privacy on receives commands, replies to itself and (per
  Telegram's docs) little else; `t --telegram-setup` prints both remedies (admin, or
  `/setprivacy` → Disable and re-add). Whether an `@mention` text message reaches a privacy-mode
  bot is spike §16.7; the design does not depend on it.
- Channels (broadcast chats) are ignored entirely.

## 7. Inbound (`channel::inbound`)

```
Update (deduped on update_id, already past MarkAnswered when it reaches an ask)
  Message m  (skip: from.is_bot, service messages, our own, edited_message, channel_post)
    where: private → sender ∈ allowed? no → warn!, drop
           group ∈ telegram.groups? no → drop; sender ∈ allowed? no → ephemeral `not allowed` if possible, drop
    `/start <code>` → pairing (§10.4)                                         done
    bot command (entity `bot_command` at offset 0; `@bot` suffix stripped) → commands::handle (§10.3)   done, no model call
    reply to a live ask card → asks::text_answer (§10.2)                     done
    reply to a child marker (§9.2) → that child's session
    key = thread key (§6); plain group: require mention | reply-to-bot, else drop
    text = (forwarded? `(forwarded from <name>)` line) + (reply_to_message? `(replying to: "…")` prefix, 09 §7)
           + text|caption with the mention stripped + one line per attachment (§7.1);
           a voice/video note is transcribed instead → `[voice note, N s] …` (45 §4.5); empty → drop
    react 👀 on m                                                            "seen, working"
    session = threads[key] (create per §6; topic created after SendInput returns the id)
    turn in flight? Steer : SendInput (files → `files` for images/PDFs)
    remember (session → m) for the terminal reaction (§8.7)
    ask failed → reply `⚠️ <error>` in the thread, react 😱
  CallbackQuery q → answerCallbackQuery(q.id, toast?) always, first (the client spins until answered)
    data prefix: `ap:` approvals (§10.1) · `ask:` questions (§10.2) · `cmd:` panels (§10.3) · `out:` tool output (§8.4)
    presser not owner and not the thread's user → toast `not allowed`, nothing else
  stopped_message_generation { chat, thread?, draft_id } (`UpdateType::StoppedMessageGeneration`) → the worker owning draft_id → Cancel(session)
  my_chat_member: bot removed/kicked from a listed group → unbind that chat's threads, warn!; added → log (the list is config)
  anything else (reactions, polls, edits, channel posts, chat_member) → counted in Status.dropped_updates, debug!
```

Follow-ups while a turn runs become `Steer` — the steer lands after the current tool batch
(09 §7). Telegram delivers one `Message` per photo in an album (`media_group_id`): the worker
coalesces an album for 1 s into one input with several attachment lines, so three photos are one
turn, not three.

**Revision 2026-10-06 (98c; r2 the same day).** Dispatch is no longer one sequential `while let
… recv().await` (`channel::dispatch_inbound`): a plain message, a Stop and a `my_chat_member` run
on a **lane per chat key** (`chat[:topic]`), so two messages in one thread still see each other's
binding (the `session = threads[key]` step of the block above); a **command, a callback press or
a reply to a live `force_reply` prompt runs on its own task**, so an engine ask that stalls
delays one reply and nothing else. *Ordering caveat:* because commands run free, `/new`,
`/resume` and `/cancel` can race a plain message sent in the same second in the same thread —
in practice `/new` wins (its unbind is synchronous before its first await; the message's bind
waits for `SendInput`'s HTTP round-trip), but the design does not guarantee it. A handler panic
is logged at ERROR with the update kind and `update_id` and the lane lives on; a dispatch loop
(inbound/events/notes) that ends while the channel lives is an ERROR, never silence.

Deadlines: every `Port` ask, `MarkAnswered`, and the `"telegram"`-actor asks in `pair`/`admin`
run under `port::ASK_TIMEOUT` (20 s; WARN at 5 s while still waiting; `Err(port::ask_timed_out())`
= `engine did not answer in 20 s`). `send_input(None, …)` is two timed asks (`NewRootSession`,
then `SendInput`): up to 40 s on the lane before the ⚠️ line. The whole of a command, panel action
or prompt continuation runs under `COMMAND_BOUND` (21 s) — the ~42 direct `agent()?.ask(..)` arms
in `commands.rs` are bounded only by that outer deadline, not individually. `transcribe()` (STT on
the lane) is bounded at `TRANSCRIBE_BOUND` = 60 s; the attachment line then says `(transcription
timed out)`. Each early exit above logs a `debug!` naming its step; each command, press and
prompt reply logs an `info!` (`telegram command name=… user=… chat=… outcome=…`, `telegram press
kind=… user=…`, `telegram prompt reply expect=… outcome=…`).

### 7.1 Files and voice notes

| Telegram field | saved as | line |
|---|---|---|
| `photo[]` (largest `PhotoSize`) | `inbox/<ts>-photo.jpg` | `[attachment: <path> (image/jpeg, 82 KiB)]`, and in `files` |
| `document` | `inbox/<ts>-<file_name>` (sniffed MIME when Telegram's is generic, 09 §7.4) | `[attachment: …]`; PDF also in `files` |
| `voice` (OGG/Opus) | `inbox/<ts>-voice.ogg` | `Transcribe { path }` → `[voice note, N s] <text>`; failure → `[attachment: … voice note, N s] (transcription unavailable)` |
| `video_note` / `video` / `audio` / `animation` | `inbox/<ts>-<name>` | `[attachment: …]`; `video_note` and `audio` are also transcribed (symphonia demuxes mp4/aac/mp3) |
| `sticker` | not saved | `[sticker: <emoji>]` |
| `location` / `venue` / `contact` | not saved | one descriptive line each |

Download: `getFile` → `https://api.telegram.org/file/bot<token>/<file_path>`, bot token only in
the URL the façade builds (never logged). Files over the Bot API's 20 MB download cap get
`[attachment: <name> — too large for the Bot API (20 MB); not downloaded]` so the model knows,
as Slack's download-failed line does (21 §7.1). The 45 §4.5 voice contract applies verbatim: a
voice input marks the turn `voice`, a typed one clears it.

### 7.2 Commands and `/start`

A command is a message whose first entity is `bot_command` at offset 0 (`/model gpt`), with an
optional `@<bot username>` suffix (groups). Everything after the command word is the argument.
Telegram already completes names in its menu, so there is no prefix matching; an unknown command
answers the help panel. A **bare text that is a live pairing code** is not a command and not
pairing — pairing requires `/start <code>` (what the deep link sends), so a stranger cannot guess
their way in by typing codes. Commands never reach the model.

## 8. Outbound (`channel::worker`, `bubble.rs`, `cards.rs`)

One ordered worker task per bound thread key, spawned on first use, holding all per-thread state;
the bus subscriber routes each event by `session → key` into that worker's `mpsc` (21 §8). One
in-flight API call per thread ⇒ nothing reorders; a slow thread never stalls another; the pacer
(§4.2) is the only cross-thread coupling. Events for unbound sessions are dropped at the router —
except `ApprovalRequested` / `AskRequested` / `ArtifactShown` / `message_user`, which fall through
to the owner chat per their own rules.

```rust
struct Worker {
    key: ThreadKey, to: Target, user: UserId,
    turn: Option<Turn>,                        // TurnStarted..TurnFinished
    marks: Vec<MessageId>,                     // the user's message(s) of this turn: 👀 → 👌/🤷/😱 (§8.7)
    approvals: HashMap<ApprovalId, MsgRef>, asks: HashMap<AskId, Vec<MsgRef>>,
    board: Option<Board>, children: BTreeMap<SessionId, Row>,   // §9.3, parent threads only
    title_sent: Option<String>, context_size: Option<u64>,
}
struct Turn {
    started: Instant, model: String, usage: Usage, voice: bool, clip_tail: Option<JoinHandle<()>>,
    bubble: Option<Bubble>,                    // §8.2; None until the first TextDelta/ReasoningDelta of a block
    card: Option<Card>,                        // §8.4; None until the first ToolCallPending of a batch
    reasoning: Option<Segment>,                // §8.6
    draft_seq: i64, tools: u32, spoken: usize,
}
```

Worker loop: `select! { ev = rx.recv() => apply(ev), _ = tick(1 s) => flush() }`.

### 8.1 Markdown: the model's text, verbatim

The unit of truth is `markdown::Text` (03 §2): `push(delta)`, `raw()` is the whole text,
`committed()` the bytes up to the last closed block. What goes on the wire is `raw()` (drafts)
or `raw()` at a block boundary (persisted bubbles) **unchanged** — Rich Markdown is
"compatible with GitHub Flavored Markdown where possible": `**bold**`, `*italic*`, `~~strike~~`,
`` `code` ``, fences with a language, `#`–`######` headings, lists, task lists, `>` quotes,
pipe tables with alignment, footnotes, `---`. Telegram renders tables as tables and headings as
headings on every client — things Signal flattens and Slack approximates. Two seams the sender
owns: (a) a bare `<` outside code (`Vec<String>` in prose) is HTML to the parser — `ui::guard`
escapes `<` and `&` outside code spans and fences (the `pulldown-cmark` event stream the
`markdown` crate already exposes says which is which), decided by spike §16.3; (b) the per-message
caps (32 768 chars, 500 blocks) drive rotation (§8.5). Nothing else is rewritten: no mrkdwn, no
MarkdownV2, no HTML conversion of our own.

Plain fallback (`send_text`, 4 096 chars, no `parse_mode`) exists for one case only: a
`sendRichMessage` the API refuses (§4.3). Then the text goes through `markdown::plain` +
`markdown::chunks(4096)` (the 09 §8.1 pair, copied from the frozen `signal::text` into `markdown` so
both crates share one implementation) and the worker publishes `StreamFallback { session, reason }`
once per process per reason.

### 8.2 The bubble: Draft mode (private chats, with or without topics)

Telegram's draft is Slack's stream with a 30-second lifetime: `sendRichMessageDraft { chat_id,
message_thread_id, draft_id, rich_message: { markdown }, can_stop: true }` shows an animated
preview bubble that is replaced on every call with the same `draft_id` and vanishes 30 s after the
last call or when the bot sends a message. So the bubble is **one prose block of the turn**:

```rust
pub struct Bubble {
    mode: Mode,                 // Draft { id: i64 } | Edit { msg: MsgRef } (§8.3)
    text: markdown::Text,       // this block's text; raw() is the truth
    sent: usize,                // bytes of raw() the last draft/edit carried
    last_call: Instant, opened: Instant,
    reasoning: Option<String>,  // committed reasoning text to show above the prose (§8.6)
}
```

**Open** on the first `TextDelta` or `ReasoningDelta` of a block (never on `TurnStarted`: an empty
draft is a "Thinking…" placeholder we have not earned yet): `draft_id = turn.draft_seq += 1`, the
first `draft()` call. **Flush** on the tick while `raw().len() > sent`: `draft(raw)` — the whole
text each time (drafts are replaced, not appended), `sent = raw.len()`. Raw deltas go on the wire:
Telegram's draft renderer animates the accumulated Markdown, and a half-open `**bol` is Telegram's
to show and ours to complete. **Keepalive**: a draft with no new text for 25 s is re-sent unchanged
once per 25 s while the turn still runs and no card is live (a stalled model; the 21 §8.3 rule),
so the bubble does not blink out mid-sentence. **Close** at a block boundary — the first
`ToolCallPending` of a batch, `TurnFinished`, `Error`, a cancel, or the size cap — with
`text.finish()` and one `send_rich(raw + footer?)`: the draft disappears as the real message
appears, which is the behaviour the API documents ("once the output is finalized, you must call
sendRichMessage with the complete message to persist it"). Empty text closes nothing. A turn with
several batches therefore reads in order: bubble · card · bubble · card · bubble — the TUI's
transcript, which is also the 45 §4.5 block rule, so voice clips need no second cursor.

`can_stop: true` on every draft call puts Telegram's own **Stop** button under the preview; a press
arrives as `stopped_message_generation { chat, thread, draft_id }` → the worker owning that
`draft_id` asks `Cancel(session)` (21's native stop button, §7). `keep_on_stop: false`: the
cancelled turn's text is persisted by the normal close on `TurnFinished { cancelled }` (the user
hears how far it got, 45 §4.5.5), so the draft need not linger.

**Footer.** The last bubble of a turn ends with a `<footer>` block (Rich HTML; if Markdown mode
refuses it, an italic last line — spike §16.3): `claude-sonnet-4-5 · 12.3k in / 1.8k out · 42 s ·
3 tools · ctx 41k/200k` (21 §8.1 with the 75 `ctx` addition). A turn that produced no text but ran
tools gets the footer on its card instead.

### 8.3 Edit mode: groups, and the repair path

Drafts are private-chat only ("Unique identifier for the target private chat"). In groups, and
whenever a draft call fails with anything but `RateLimited`, the same `Bubble` runs in `Edit`
mode: `send_rich(raw[..committed()])` on the first tick with a committed block, then `edit_rich
{ rich_message: { markdown: raw[..committed()] } }` on each tick where `committed()` grew (≥ 1.5 s
apart, §4.2); the close is the last edit with the full text (+ footer). Only committed blocks are
shown, so an edit never shows a half-table (21 §8.2's argument: paragraph-granular reveal, zero
flicker). `MessageNotModified` is ignored; `MessageToEditNotFound` rotates (§8.5). Switching
modes publishes `StreamFallback { session, reason }` once per process per reason. Edit mode is
also the repair path, which is why it is a mode of the same `Bubble` and not a second code path.

### 8.4 Activity cards (tool observability)

On the first `ToolCallPending` of a batch the worker closes the open bubble (§8.2) and opens one
**card**: a rich message with one entry per call, edited as the batch progresses (coalesced by the
tick, ≥ 1.5 s apart, last edit at the batch's end):

````markdown
<details><summary>✓ read_file · crates/agent/src/loop_.rs · 0.3 s</summary>

```text
1  //! The turn loop …
```
</details>
<details open><summary>⚙ shell · cargo test -p agent · running 12 s</summary>

```sh
cargo test -p agent
```
</details>
<details><summary>✗ edit · src/lib.rs · 0.1 s</summary>

```text
old text not found
```
<tg-button type="callback_data" data="out:7f3a…">full output · 12 KB</tg-button>
</details>
````

| event | entry |
|---|---|
| `ToolCallPending { call_id, name }` | `⏳ <name>` |
| `ToolCallStarted { args }` | `⚙ <name> · <describe(args)> · running` + the args in a fence (`shell` → the command; others → `key: value`, 00 §4) |
| `ToolCallFinished { output, is_error }` | `✓`/`✗ <name> · <describe> · 1.2 s` + the first `TOOL_OUTPUT_CHARS = 2 500` chars of output in a fence; longer → `… n more chars` and an `out:<call_id>` button |

`<details>` is collapsed by default, `open` while a call runs; the summary is one line, so a
20-call batch is twenty lines until the reader expands one. `describe` is the TUI's. Up to 3
`http(s)` URLs found in the output (`markdown::urls`) are appended as a link line (the 21 §8.3
`sources` without Slack's citation UI). An `out:<call_id>` press → `answerCallbackQuery` then
`sendDocument` of `<name>-output.txt` as a reply to the card; outputs are kept in memory for one
hour per call id (owner or thread user only; after that the toast says `output expired`).
`telegram.tools: all | errors | none` as 21 §12: `errors` opens a card only when a result is
`is_error`, `none` opens no cards. The `<tg-button>`-in-body form is spike §16.3's second
question; the fallback is one `reply_markup` keyboard on the card with a button per oversized call.

A card lasts as long as its batch; a batch that runs for minutes is edited at most every 1.5 s
and only when something changed, so a 10-minute `shell` costs one edit per status change plus a
running-time refresh every 30 s. There is no 30-second expiry to fight: a card is a real message.

### 8.5 Rotation and repair: one routine, three triggers

`rotate(reason)`: close the current bubble — `send_rich(raw)` for a draft, a final `edit_rich`
for an edit-mode message, or a fresh `send_rich(raw)` when the edit target is gone — then the
next event opens a new bubble in the same thread. Triggers: **size** (`committed()` crosses
`BUBBLE_SOFT_MAX = 30 000` chars or 450 blocks: finish this bubble at that block boundary, the
remainder re-pushed into the new bubble's `Text`, so no construct spans two messages; a single
block over the cap — a giant fence — is cut at its last newline and the fence re-opened in the
continuation); **death** (a draft call fails, an edit answers `MessageToEditNotFound` /
`MessageCantBeEdited`, a persisted send is refused → Edit mode or the plain fallback, §8.3); the
**30 s expiry** is prevented by the keepalive, not repaired. No roll counter, no tail
reconciliation: `raw()` is complete and a bubble is one call.

### 8.6 Reasoning

While a reasoning run streams, the live draft shows `<tg-thinking>thinking · 12 s</tg-thinking>`
(the one tag Telegram reserves for drafts) above whatever prose exists; with `telegram.reasoning:
true` (default) the committed reasoning text is also carried inside an `<blockquote expandable>`
in the draft and in the persisted bubble, collapsed by default, where it happened — before the
prose it preceded, or as its own `<details><summary>thinking</summary>` entry in the card when it
fell between tool calls. `ReasoningDone { ms }` (91) finishes the quote's heading as
`thought 9 s`. Providers that expose no reasoning emit nothing; `reasoning: false` keeps the
`<tg-thinking>` indicator and drops the text.

### 8.7 Status: reactions, typing, Stop, titles

Reactions on the *user's* message — Telegram keeps one reaction per bot per message, so a new
`setMessageReaction` replaces the old, exactly Signal's three-state indicator (09 §8.2), with the
emoji drawn from Telegram's fixed set (`ReactionTypeEmoji`, 73 values; ✅ ❌ 🛑 are not among
them — checked against the Bot API page on 2026-10-05):

| moment | reaction |
|---|---|
| accepted (`SendInput`/`Steer` sent) | 👀 |
| `TurnFinished` normal | 👌 |
| `TurnFinished { cancelled }` | 🤷 |
| `TurnFinished { error }` / `Error` | 😱 |

One `const REACTIONS` table; a steer gets its own 👀 and the turn's terminal mark. Reactions in
groups need the bot to be allowed to react (`can_react_to_messages`, 10.0); a refused reaction is
logged once per chat and skipped.

**Typing.** `sendChatAction { typing, message_thread_id }` on `TurnStarted` and every 4 s from
the tick while the turn runs and no draft or card was touched in the last 4 s (a draft is its own
liveness; a sent message clears the typing status anyway). Stops at `TurnFinished`.

**Stop.** The draft's `can_stop` button (§8.2) and `/cancel`. Both reach `Cancel`; the terminal
reaction is 🤷.

**Title.** After `TurnFinished`, `ListSessions` → this session's `SessionSummary.title` (07 §15);
if it differs from `title_sent` and the thread is a topic the user did not name
(`named_by_user == false`), `editForumTopic { name ≤ 128 }`. A `forum_topic_edited` service
message from the user sets `named_by_user` and we stop. One ask and at most one call per turn;
flat chats have no title to set (09 parity). Child topics use `↳ <name> · <task first line>`.

### 8.8 Voice turns (45 §4.5)

When the input that started or last steered the turn was a voice note, each prose block — the
bubble closed at a batch boundary, and the tail — is synthesised (`Synthesize`) and sent as
`sendVoice` (MP3 is an accepted voice format and renders as a voice bubble), in order, through
the `clip_tail` chain of 45 §4.5.4, right after the bubble it speaks. Failures are logged; the
text already landed. A typed steer clears `Turn.voice`.

### 8.9 Other events

| event | thread message |
|---|---|
| `MessageDelivered { from, text }` | `⟵ **from**` + body as a rich message (≤ 32 768, truncated with a count); its own message, never inside a bubble (21 §8.7) |
| `Error { message }` | `⚠️ <message>` |
| `Compacted { epoch, folded }` | `⟲ compacted → epoch n (m messages folded)` |
| `InputQueued` | 👀 on the steer message (done inbound); nothing else |
| `SubagentSpawned` | §9 |
| `ContextUsed { size }` | remembered for the footer |
| `ArtifactShown` | §8.10 |
| `ModelChanged`, `EffortChanged`, `SkillsChanged`, `Harvest*`, `Compacting`, `Input{Injected,Withdrawn}`, `TodoChanged`, `TitleChanged`, `ToolCallProgress`, `VisionProgress`, `Session*`, `*Default*`, `Harness*` | nothing (the 21 ignore list, `channel.rs:2355-2380`) |

### 8.10 `show` and `message_user`

- `ArtifactShown` (35 §8): the session's thread, else the nearest bound ancestor's
  (`port.ancestors`, as `signal/channel.rs:1585-1625`); the file as `send_file` — `sendPhoto`
  for `Kind::Image` ≤ 10 MB (inline preview; larger or odd-sized images go as a document, which
  keeps full fidelity), `sendDocument` for everything else (PDF previews render natively), caption
  = the title (≤ 1 024) and for an app its `.openui` source with the 46 §8.2 note. Over 50 MB →
  one line `artifact too large for Telegram (50 MB): <path>`.
- `message_user` (24): `impl UserSink for channel::Handle` in `user.rs`, `name() == "telegram"`.
  Bound pass: the first session of the chain with a thread → its topic/chat; fallback: the
  owner's chat (General), text prefixed `_<origin>_` on its own line. Text as `send_rich`
  (plain fallback as §8.1), then one `send_file` per file, no caption. Result string:
  `telegram: <chat title or @user>` / `telegram: owner`. Arrives below the live draft, out of
  band by design (24 §6). The tool's description and `via` gain `telegram` (§19).

## 9. Subagents

19 §1: a subagent is a session, so a child gets what a session gets — its own thread with
bubbles, cards and approvals — and the parent's thread gets a board (21 §9).

### 9.1 A topic per child (chats with topics)

`SubagentSpawned { parent, child, name }` with a bound parent in a topics-capable chat →
`createForumTopic { name: "↳ <name>" }` in the same chat, bind `<chat>:<topic>` to `child`.
The child's first `TurnStarted.text` (the task) is posted into its topic as `▸ **task**` + text
(≤ 3 000); then the child's turns render there through the same worker code — nothing is
special-cased by depth. In the **parent** topic: one line `↳ spawned **grep-logs**` (no permalink:
Telegram has no stable deep link into a private topic; the topic list is one tap away) ; the
result arrives as the usual `⟵ grep-logs` message (§8.9); a child's `Error` or cancelled
`TurnFinished` interjects one line (19 §3.3). Typing in a child's topic steers the child; an
`Awaiting` child gets its answer (19 §2.3). Depth 2 works the same. Bounded by `max_concurrent`
(8) per parent; a chat accumulates one topic per child, which is what the user asked for by
turning Topics on — `/agents` offers `close` for finished ones (`closeForumTopic`).

### 9.2 Without topics (flat private chat, plain group)

Children render in the parent's chat: the `↳ spawned **grep-logs**` line is the child's
**marker**; the child's bubbles and cards post into the same chat prefixed `**grep-logs** ·` on
their first line and as **replies to the marker** (Telegram quotes the marker above each, which is
the client's own thread affordance); a user **reply to the marker** or to any of the child's
messages steers that child (`reply_to_message.message_id` ∈ the child's message set, kept in the
worker). The board (§9.3) sits in the chat too. This is more than Signal offers (09 §6: children
invisible, nothing steerable) and less than topics; it exists so a user who keeps Topics off still
sees and steers what was spawned.

### 9.3 The board

While at least one descendant of a bound session is not idle, its thread holds one **board**
message edited in place (≥ 2 s apart, coalesced by the tick), a rich-message table:

```
**agents**
| | name | state | detail | age |
|---|---|---|---|---|
| ⠹ | grep-logs | running | read_file crates/agent/src/loop_.rs | 1m32s |
| ⏳ | fix-tests | awaiting | parent: "should I also update the docs?" | 48s |
| ⚠ | scout | approval | shell `cargo test -p agent` | — |
```

Rows are 19 §3.1's, derived from the child events the worker already routes (`RowChanged`
forwarded from the child's worker on `TurnStarted/Finished`, `ToolCallStarted/Finished`,
`Error`, `ApprovalRequested/Resolved`), capped at 8 rows with `… n more`; child events fan out to
every bound ancestor so a top-level board shows grandchildren (21 §19). When every child is idle
the last edit reads `**agents** · all idle` and the message stays as history. After a restart the
board is rebuilt on the next child event from `list_agents` (19 §2.1). No new `AgentEvent`.

## 10. Interactivity

### 10.1 Approvals

An `ApprovalRequested` is posted as a rich message with an inline keyboard:

````markdown
⚠️ **shell** wants to run
```sh
rm -rf target/
```
[ Allow ]  [ Deny ]
````

Body as `approvals::message` in 09 §10 (`shell` → the command verbatim in a fence; others →
pretty JSON ≤ 600 chars). Buttons: `callback_data` `ap:<ApprovalId>:a` / `ap:<ApprovalId>:d`
(37 + 2 bytes of the 64 allowed) — self-describing, so a card from before a restart still works.

Where it goes, `telegram.approvals` (§12):

- session bound to a Telegram thread → **that thread** (a child's approval lands in the child's
  topic; the parent board row says `approval`). Who may press: owner or the thread's user.
- session not bound (terminal, RPC, ACP, a schedule run) and `approvals: all` (default) → the
  **owner's chat**, General, with a context line `session <last 8> · <title>` (title from the
  `ListSessions` the worker already makes; `Unknown` when the ask fails). Only the owner may
  press. `approvals: threads` disables this and matches 09.

A press → `answerCallbackQuery` (toast `allowed` / `denied`), allowed presser? → `ask Resolve
{ id, decision }`; `false` (already resolved) → toast `already resolved`; not allowed → toast
`not allowed`. `ApprovalResolved` from anywhere (terminal, RPC, timeout) → the card is edited to
`✓ allowed by <name>` / `✗ denied` / `⌛ timed out` and its buttons become `DisabledButton`s
showing the verdict (10.3), so the thread reads correctly later. Secrets-bearing calls and
destructive ones use the same card; the risk word is in the first line.

Rejected: codes and `/approve` (buttons are the native gesture, 21 §10.1); 👍/👎 reactions
(a second path; and inbound reactions need `message_reaction` in `allowed_updates` plus admin
rights in groups); countdown edits (21 §0).

### 10.2 `ask_user`

`AskRequested { id, questions }` → one card per question, in sequence as `crates/signal/src/asks.rs`
paces them, each a rich message:

```
❓ **Which database?** (1/2)
1. **Postgres** — managed, we know it  _(recommended)_
2. **SQLite** — zero ops
_(or type your answer as a reply)_
[ Postgres ] [ SQLite ]
[ Skip ] [ Dismiss ]
```

Keyboard: one button per option (`ask:<id>:<q>:<k>`, ≤ 8 per row, ≤ 23 options as Slack's
cap), `Skip` (`ask:<id>:<q>:s`), `Dismiss` (`ask:<id>:x`, the whole set), and
`force_reply: true` on the markup so the client opens the reply box targeting the card; a
message whose `reply_to_message` is a live card is `Answer::Text`. Presses by anyone but the owner
or the thread's user get a `not allowed` toast. `AskResolved` → each card is edited to the
answered / skipped / dismissed / timed-out / answered-elsewhere form of `asks.rs` and its keyboard
removed. Owner-chat cards (unbound sessions, like approvals) carry the session context line.
The `wth-ask-user-relay-no-replay` lesson (a one-shot broadcast) does not apply: the card is a
persisted message and the id is in the button.

### 10.3 Commands

The 09 §9 table, same names, arguments and meaning, registered as native bot commands so the
client offers them with completion and descriptions (the `(&str, &str, &str)` table is the one
source for `setMyCommands` and `/help`):

| scope | commands |
|---|---|
| `BotCommandScopeDefault` | `start help status new sessions resume cancel compact clear undo redo model effort agents jobs prompt skills mcp login logout provider` |
| `BotCommandScopeChat { owner }` | the above + `approvals telegram communication peers requests broadcast a2a` |
| `BotCommandScopeAllGroupChats` | `help status new sessions resume cancel compact model agents`, each `is_ephemeral: true` (10.2) so the command is invisible to other members and the reply can be ephemeral |

Owner-only names from a non-owner answer `This command is restricted to the owner.` (09). A
session-scoped command (`cancel`, `compact`, `agents`, `model <id>`, `effort`, `prompt`, `undo`,
`redo`, `clear`, `jobs`) resolves the session as the **thread the command was typed in**; in
General or a flat chat with no binding, the invoker's most recently active thread in that chat
(21 §10.2 rule); none → the reply says so and offers the sessions panel. Explicit
`… <session-prefix>` overrides.

**Replies are rich messages with inline keyboards, not fenced tables** (21 §10.2's revision,
adopted from the start). `ui.rs` builds them; `commands::handle` returns `Reply { markdown,
keyboard: Option<Keyboard> }`; presses arrive as `cmd:` callbacks and re-render the panel in
place with `edit_rich` (no `response_url` dance: Telegram edits are first-class). In groups the
reply is ephemeral to the invoker (`ephemeral_message_parameters` with the command's
`ephemeral_message_id` or the press's `callback_query_id`), so the group stays clean — the 21
ephemeral parity.

| command | panel | presses (`callback_data` → effect) |
|---|---|---|
| `/start` | pairing (§10.4) when a code follows; otherwise the help panel | – |
| `/help` | one `<details>` per group (session · harness · setup), commands with their hints; buttons Sessions · Agents · Models · Status | `cmd:sessions`, `cmd:agents`, `cmd:models`, `cmd:status` re-render in place |
| `/status` | a table: polling · bot · topics · threads · session · model · turn · context, then per-method call counts in a collapsed `<details>` | `cmd:refresh` |
| `/sessions` | 10 newest top-level sessions as a table (last-8 · title · model · age); a button row per page: `Resume <last-8>` ×n, `More…` | `cmd:resume:<id>` binds per §6 (a new topic in topics chats), `cmd:page:sessions:<n>` |
| `/resume <id>` | binds, replies `resumed <id> · <title>` in the bound thread | – |
| `/new` | §6 | – |
| `/agents` | one row per `list_agents` entry (glyph · name · state · detail · age), `Cancel <name>` buttons while running, `Close topic` for finished children with a topic | `cmd:cancel:<id>`, `cmd:close:<id>` |
| `/cancel [name]`, `/compact […]`, `/clear`, `/undo`, `/redo`, `/prompt`, `/jobs [kill <id>]`, `/effort [level]` | result text (prompt in a fence; it is text); Signal's wording | – |
| `/model [id]` | `current: …` + one button per configured model grouped by provider (≤ 8 per row, paginated at 24) | `cmd:model:<id>` → `SetModel`, confirmation replaces the panel |
| `/skills […]`, `/mcp […]` | one line per entry with source and count, buttons Sync · Add…; `add` with an argument runs directly | `cmd:skills:sync`, `cmd:mcp:sync`; `Add…` replies with a `force_reply` prompt for the URL/JSON (no modals on Telegram) |
| `/login [name]`, `/logout <name>` | one row per provider with its state and a Log in / Log out button; `login` answers with a URL button (`InlineKeyboardButton.url`) | `cmd:login:<name>`, `cmd:logout:<name>` |
| `/provider […]` | as Signal (text) | – |
| `/approvals on\|off [always]` | three buttons: ask · off this session · off always | `cmd:approvals:<choice>` |
| `/telegram [status\|link\|allow <id>\|rm <id>\|group add <id>\|group rm <id>]` | owner only; bare = `status`; `link` replies the deep link + QR (§10.4); `allow`/`rm`/`group` go through the `"telegram"` actor (live list + `write_config_value`) | – |
| `/communication`, `/peers`, `/requests`, `/broadcast`, `/a2a` | owner only; the Signal implementations' text replies (50, 62 §5.3) | – |

Rules: every `callback_data` ≤ 64 bytes (session ids travel as last-8 and are resolved like
`/resume`); ≤ 100 buttons per keyboard, ≤ 8 per row; a press by a user who is neither owner nor
allowed gets `not allowed` and no effect; owner-only panels reject non-owner presses the same way;
a panel older than the process (restart) still works because every press carries what it needs.
The dispatcher is a `match` over the table's names, each one `ask` plus a formatter — the third
copy of the asks the TUI, Signal and Slack already accept (09 §14 priced a shared layer; §14).

**Revision 2026-10-06 (98c).** `command_cx` is the one place `CommandCx` is built; it takes its
snapshots (`owner`, `status()`) into locals *before* the struct literal. The v0.1 literal held the
`owner` mutex guard as a temporary while `status()` locked it again — a self-deadlock on every
command, which also pinned the single inbound task (98c root cause). A command whose asks do
not answer replies `error: engine did not answer in 20 s` and the log carries
`telegram command … outcome=Reply` plus the `engine ask timed out` WARN.

### 10.4 Pairing and setup

Pairing follows the `/login` shape (09 §11): a command returns a URL, every surface renders it (as
text and a QR), completion arrives as an event.

- **The link.** `https://t.me/<bot username>?start=<code>`; `code` is 16 chars of the 09 §10
  alphabet, valid `PAIR_TTL = 10 min`, single-use, held in the channel (`Pairing { code,
  expires }`); minting a new one voids the old. Opening the link on any device shows the bot
  with a Start button; pressing it sends `/start <code>` as the user, which is the only message
  from a stranger the channel acts on: code live → `owner = from.id` applied **live** to the
  trust list and written to `config.yaml` (`write_config_value`, double-quoted), publish
  `TelegramEvent::Paired { user, name }`, reply with a welcome and the help panel, register the
  owner command scope. Code unknown or expired → silence (09 §5.1). If `telegram.owner` is
  already set, a fresh code **re-pairs** (owner change) — minting is owner-only, so that is the
  owner's choice.
- **`t --telegram-setup`** (`setup.rs`, one interactive terminal session, no daemon running —
  a running engine (63) makes it print `daemon running: use /telegram link or the telegram_link
  tool` and exit):
  1. Asks for the **bot token** from @BotFather (`/newbot`; the prompt prints the steps); verifies
     with `getMe`; prints `@username`, whether Topics are on (`has_topics_enabled`) and, if not,
     the @BotFather path to turn them on (Bot Settings → Topics) and what changes (§6).
  2. Writes `telegram_bot` into the vault (`Vault::open` directly, source `telegram setup`, 37
     §3.6; or the user runs `t --secret-set telegram_bot` on a box without a TTY).
  3. Registers the command scopes (§10.3) — the only step that needs the API beyond `getMe`.
  4. Mints a code, prints the deep link and the QR (`link::qr`, Dense1x2, ~900 chars, 09 §11),
     and runs a **one-shot poller** (the §3.1 loop, starting at `offset: -1` so pending updates are skipped) until `/start <code>`
     arrives or 10 minutes pass; prints `paired with <name> (<id>)`.
  5. Writes `telegram.owner` and `telegram.enabled: true`; prints the group note (admin with
     *Manage topics*, or `/setprivacy` → Disable and re-add; then `/telegram group add <chat id>`
     — the id is shown by `/status` once the bot is in the group) and `run t --headless and
     message the bot`.
  Every step is idempotent and re-runnable; a step whose input is present (a token in the vault
  that passes `getMe`, an owner already set) is skipped with a line saying so.
- **In the daemon.** `/telegram link` (owner) and the `telegram_link` tool mint the code and reply
  with link + QR; `TelegramEvent::PairingLink { url, expires_at_ms }` goes on the bus for any UI.
- **With the agent.** Tools registered by the plugin, so "set up Telegram for me" works in a chat:
  - `telegram_link { owner?: u64 }` (`Mutating`): with `owner`, writes `telegram.owner` and
    applies it live (no link needed — the user already knew the id); without, mints a code and
    returns `open this from the owner's Telegram:` + link + QR + `(expires in 10 min; call again
    for a fresh one)`. In terminal mode (no channel) it needs only the token: it can still mint
    a link, but completion requires the daemon, and the text says so.
  - `telegram_status {}` (`ReadOnly`): the §13 snapshot as text (`render_status`).
- **Unpair.** `/telegram rm <owner id>` is refused (the owner cannot remove themselves);
  re-pairing replaces the owner; revoking the token is @BotFather's `/revoke`, after which the
  poller fails `Unauthorized` and the status says so. `threads.json` is kept — sessions outlive
  the bot (09 §11.4).

### 10.5 Desktop and mobile: the Telegram card in Settings › Channels (added 2026-10-05 evening)

Signal and Slack each have a card at the bottom of Settings › Channels
(`crates/desktop/ui/src/components/settings/Channels.tsx`: `Section`/`Row`/`Dot`/`AllowList`
over `signal_*` / `slack_*`, invalidated by their bus events). Telegram gets the third card,
same components, same place, on desktop and the phone (one React tree), and it is the one
channel that can be set up **entirely from the card** — no terminal, no restart:

| row | shows | action |
|---|---|---|
| **Bot** | `Dot` + `telegramState(s)` — one phrase per `TelegramEvent` variant (`Unconfigured` → "no bot token", `Identity`/`Connected` → "connected as @bot · topics on/off", `Disconnected` → reason, `Conflict` → "another poller holds this token", `RateLimited`, `StreamFallback` → "connected", `Unreachable`, `PairingLink` → "waiting for the owner to tap Start", `Paired` → "paired with <name>", `DaemonOnly`); `polling` false while another engine holds the lease → "idle (hub elsewhere)" | — |
| **Bot token** | "set" / "not set" (from `state == Unconfigured`; the value is never shown) | a masked input + **Save** → `set_secret("telegram_bot", value, "Telegram bot token (@BotFather /newbot)")`; the engine hot-starts the channel on `SecretsChanged` (§3, below) and the card refreshes on `Identity`. "Replace…" reveals the input when a token is set |
| **Owner** | the owner id, or "not paired" | **Pair** (or **Re-pair**) → `telegram_link(null)` → `PairingLink { url, expires_at_ms }` → `QRCodeSVG` of the deep link + the URL + copy button + "expires in m:ss"; on `Paired` the row shows the id and the link disappears; a direct id can be typed into a small input → `telegram_link(id)` |
| **Allowed users** | numeric ids | `AllowList` → `telegram_allow` / `telegram_remove_allowed` (digits only) |
| **Groups** | chat ids (`-100…`) | `AllowList` → `telegram_allow_group` / `telegram_remove_group` |
| **Setup notes** | collapsed `details`: BotFather → Bot Settings → Topics (threads per session), privacy off or admin for groups, `/telegram` commands | — |

`CHANNEL_EVENTS` gains every `TelegramEvent` type; `queryKeys.telegram`; `api.ts` gains the six
`telegram_*` methods (the bindings already carry `TelegramStatus`, `TelegramEvent`,
`PairingLink`, `BotIdentity`). Phone: the QR is 192 px like Signal's; rows stack as the other
cards do; nothing else is phone-specific.

**Hot start (engine).** `Plugin::start` subscribes the `Telegram` actor to `SecretsEvent`; when
the actor is `Unconfigured` (no channel) and a `SecretsChanged` arrives, it re-reads the vault;
if `telegram_bot` is now present and this process runs the channel (`channel == true`), it
calls `channel::spawn` exactly as boot would have, stores the `Handle`, registers the
`UserSink`, and publishes `Identity` when `getMe` answers. A token *replaced* while the channel
runs is picked up at the next restart (the running `Api` keeps its client); the card says so.
Rejected: a `telegram_set_token` RPC (the vault already has `set_secret`; one secret path, 37)
and restarting the engine from the card (heavier than it needs to be).

## 11. State, restart, and what is truth

Truth on disk: `threads.json` — thread key → session (+ user, title, topic) and the `open` list
of edit-mode messages (bubbles and cards) mid-edit. On boot every `open` entry is edited once to
append `· interrupted` (best effort: `MessageToEditNotFound` is fine) and the list is cleared —
Slack's dangling "streaming…" fix (21 §11) in Telegram terms; drafts need nothing (they expire in
30 s on their own). Everything else is derived or ephemeral and rebuilt from events: workers spawn
on first event, boards from `list_agents`, a tool call in flight across a restart shows its result
in a fresh card. The dedup set is lost; the batch Telegram re-delivers after a crash becomes steers
at worst, and `MarkAnswered` (§3.2) catches what another hub already answered. Approval and ask
cards survive because their buttons carry the ids. Pairing codes are process-local by design.

Per thread, in memory: the `Worker` (§8). Per process: `Threads`, `RecentKeys`, the pacer, the
`BotIdentity`, the call counters, tool outputs for `out:` presses (1 h), the pairing code.

**Revision 2026-10-06 (98c).** Per process, one lock on channel state: `Channel::state`
(parking_lot). The owner is an `AtomicU64` (`0` = none), not a second mutex. The only nesting
left is **state → `Api.calls`** (`status()` holds the state guard while `api.stats()` takes the
façade's counters lock); never the reverse. A guard never crosses an `.await`
(`#![deny(clippy::await_holding_lock)]` on the crate, enforced by the clippy gate) and is never
taken twice on the same task.

## 12. Config

```yaml
telegram:
  enabled: true            # daemon polls when the token is present
  owner: 123456789         # user id; set by --telegram-setup / telegram_link / the deep link; owner-only commands; receives unbound cards
  allowed: []              # more user ids; each owns their threads
  groups: []               # chat ids (−100…) where @mentions and commands are honoured; empty = private chats only
  tools: all               # all | errors | none — activity cards per tool batch
  reasoning: true          # reasoning text in the expandable quote (the thinking indicator stays)
  approvals: all           # all | threads — where approvals/questions for non-Telegram sessions go (§10.1)
  # api_host: http://127.0.0.1:8081   # tests and proofs only; overrides https://api.telegram.org (default unset)
```

The token is secret `telegram_bot` in the encrypted vault (37; `t --telegram-setup` writes it,
or `t --secret-set telegram_bot`) — never in `config.yaml` (01 §1.3). Env overrides as everywhere
(`THEUTH_TELEGRAM__OWNER`). No pacing knobs: they are Telegram's limits, not preferences. No
`topics:` key: the shape follows `getMe` and the chat (§6). `DEFAULT_CONFIG` in `main.rs` gains
the commented section; the README its bullet.

## 13. Types

```rust
// telegram/src/lib.rs
pub const TELEGRAM_ACTOR: &str = "telegram";
pub struct TelegramPlugin { pub channel: bool }                        // headless → true
pub struct Status;                                                     // ask → TelegramStatus
pub struct Link { pub owner: Option<u64> };                            // ask → Result<PairingLink { url, expires_at_ms }, String>
pub struct Allow(pub u64); pub struct RemoveAllowed(pub u64);          // ask → Result<(), String>; config + live
pub struct AllowGroup(pub i64); pub struct RemoveGroup(pub i64);

#[serde(tag = "type")]
pub enum TelegramEvent {
    Identity      { bot: String /* @username */, topics: bool },  // getMe answered
    Connected     { bot: String },                                 // first successful getUpdates after a (re)start
    Disconnected  { reason: String },                              // poll error (the loop backs off), or "hub lease held elsewhere"
    Conflict,                                                      // 409: another poller on this token
    RateLimited   { method: String, retry_after_secs: u64 },
    StreamFallback{ session: SessionId, reason: String },          // a bubble left Draft mode, or rich → plain
    Unreachable   { chat: i64, reason: String },                   // BotBlocked / ChatNotFound / kicked: thread unbound
    PairingLink   { url: String, expires_at_ms: u64 },             // show as a QR (09 LinkUrl shape)
    Paired        { user: u64, name: String },
    Unconfigured,                                                  // token missing; nothing started
    DaemonOnly,                                                    // terminal mode
    Starting,                                                      // 98c r2: actor up, vault not answered yet (Status only, never on the bus)
}
pub struct TelegramStatus {
    state: Option<TelegramEvent>, bot: Option<BotIdentity { id: u64, username: String, topics: bool, users_may_create_topics: bool }>,
    polling: bool /* holds the lease and the poller is up */, owner: Option<u64>, allowed: Vec<u64>, groups: Vec<i64>,
    threads: Vec<(String /* chat[:topic] */, SessionId)>, open: u32,
    calls: BTreeMap<String, CallStats { ok: u64, err: u64, rate_limited: u64 }>, dropped_updates: u64,
}

// telegram/src/channel.rs
pub struct Handle { chan: Arc<Channel> }
struct Channel { api: Api /* the only tgbot-aware module */, rt: Runtime, identity: OnceLock<BotIdentity>, owner: AtomicU64 /* 98c: 0 = none */,
                 state: Mutex<State { allowed, groups, threads: Threads, workers: HashMap<ThreadKey, mpsc::UnboundedSender<Out>>,
                                      recent: RecentKeys, last_event, dropped, pairing: Option<Pairing>, lease: roster::lease::Follower }>,
                 approvals: Mutex<HashMap<ApprovalId, MsgRef>>, asks: Mutex<asks::Pending>, outputs: Mutex<Outputs /* call_id → text, 1 h */>,
                 port: Arc<dyn Port> }   // 09 §13's trait + cancel, list_sessions, list_agents, ancestors, resolve (the slack one)
// bus subscriptions: AgentEvent, ApprovalRequested, ApprovalResolved, AskRequested, AskResolved, ArtifactShown, secrets::SecretsEvent, mesh::MeshEvent
//   — the eight both channels subscribe to today (signal/channel.rs:621-628, slack/channel.rs:509-516)
// asks (commands.rs): the Signal set — agent (SendInput, Steer, Cancel, Compact, Undo, Redo, ListSessions, LoadSession, ListAgents,
//   ListModels, SetModel, SetEffort, ShowPrompt, ListJobs, KillJob, ListSkills, …, Broadcast, communication::*), tools (SetApprovals, Resolve,
//   Answer), mcp (ListServers, AddRepo, AddServer, SyncMcp, McpLogin, McpLogout), providers
```

Tools: `telegram_link` (`Mutating`), `telegram_status` (`ReadOnly`), §10.4. RPC
(`rpc/src/api.rs`, beside `slack_*`): `telegram_status() -> TelegramStatus`,
`telegram_link(owner?) -> PairingLink`, `telegram_allow(user)`, `telegram_remove_allowed(user)`,
`telegram_allow_group(chat)`, `telegram_remove_group(chat)`; `TelegramEvent` forwarded in
`bridge.rs`, both types in `export.rs` (`ts` feature forwards to `telegram/ts`).

**Revision 2026-10-06 (98c).** `Status` is answered from the actor's own state at any time —
before the vault read, while getMe retries, while a command is in flight (`channel.rs`
`status_answers_within_a_second_*` tests). `port::{ASK_TIMEOUT, ASK_SLOW, ASK_TIMED_OUT, timed}`
are the ask deadline; `Incoming::{kind, update_id}` name an update in logs. The Settings card
renders `loading…` while the query is pending and the error text with a Retry button when it
fails (`Channels.tsx`), never nothing.

## 14. Rejected

- **Webhooks**, **MTProto** (§0.2, §1).
- **teloxide** — crates.io 0.17 (reqwest 0.12, Bot API 9.1) and a Git pin of `master` (9.2)
  plus a hand-written module for everything 2026 added, which was v0 of this doc (§0.1).
  **frankenstein** (no unknown-update fallback, no runtime helpers), **telers** (beta
  framework), **grammers** (MTProto). **tgbot's `LongPoll`** (unordered, no offset confirm;
  §2.2).
- **Streaming by `editMessageText`** as the primary path (§0.5): 1/s per chat, 4 096 plain chars,
  no tables; it is the group and repair path only.
- **MarkdownV2** and **our own HTML renderer** (§0.5, §8.1).
- **One message per tool call** (21 §8.2 update mode) and **tool cards inside the prose bubble**
  (§0.6).
- **Short codes and reaction approvals** (§10.1). **Modals** — Telegram has none; `force_reply`
  prompts cover the two inputs (`skills add`, `mcp add`).
- **A required forum supergroup** as the only threaded shape (§0.4): private-chat topics do it
  with nothing to create.
- **Reply chains as threads** in flat chats beyond §9.2's child markers: invisible structure.
- **Per-user sticky defaults**, **a shared frontend layer** with Signal/Slack (third copy of the
  asks; 09 §14 priced it; a fourth frontend is the trigger and this is the fourth — see §17).
- **Reading inbound reactions** (`message_reaction` in `allowed_updates`): nothing consumes them;
  groups would need admin rights for a feature nobody asked for.
- **Polling on every engine** (§0.2).

## 15. Checks

Unit tests in the module they check (`cargo test -p telegram`); logic a plausible bug would
break, not wiring (plugins.md §5):

1. **Poll** (`poll::tests`, `wiremock` answering `getUpdates`): a duplicate `update_id` is
   dropped once, the 1 025th key evicts the first; a `stopped_message_generation` update is
   classified as `Stopped { chat, thread, draft_id }`; an update kind the crate does not know
   arrives as `UpdateType::Unknown`, is counted and does not stop the loop; a lease loss stops
   the poller with the confirming `getUpdates { limit 1, timeout 0 }` and a gain restarts it;
   the `Conflict` description publishes `Conflict` and keeps the loop alive with backoff.
2. **Api** (`wiremock` as the Bot API host via `Client::with_host`): `ok:false` descriptions
   map to the exact branches of §4.3 and unknown ones are counted by `error_code`; two drafts to
   one chat are ≥ 1 000 ms apart, two edits of one message ≥ 1 500 ms, two sends to one group
   ≥ 3 000 ms; a `retry_after: 2` is slept and retried by the client and a repeated one surfaces
   as `RateLimited`; `send_file` picks photo/voice/document by MIME and size; `download` refuses
   over 20 MB before fetching; no HTTP error text contains the token.
3. **Bubble** (fake `Api` recording calls): the first delta opens a draft with `can_stop`;
   full-text re-sends on the tick; a 25 s silence re-sends the draft once; `ToolCallPending`
   closes the bubble with one `sendRichMessage` whose text equals the deltas; a card opens with
   `⏳ name`, moves to `⚙ … running` with the described args, and ends `✓` with the first output
   line and an `out:` button past 2 500 chars; `telegram.tools: errors` opens a card only for an
   error; a 70 000-char turn splits at a block boundary into bubbles with no construct spanning
   them and a fence re-opened; a draft failure switches to Edit mode on the same text and never
   sends past `committed()`; `MessageNotModified` is ignored; the footer is on the last bubble;
   reasoning renders the expandable quote and `reasoning: false` drops the text; `ui::guard`
   escapes a bare `<` outside code and leaves fences alone.
4. **Routing** (`channel::tests`, stubbed `Port`): a General message in a topics chat →
   `SendInput(None)` + `createForumTopic` + bind + 👀; a message in a bound topic while running →
   `Steer`; a flat-chat first message binds `<chat>`; a plain-group message without mention or
   reply is ignored, with mention binds; a stranger DM is dropped silently and makes no call; a
   non-allowed group member gets one ephemeral `not allowed` when an admin and nothing otherwise;
   an album of three photos is one input with three lines; `SubagentSpawned` for a bound parent
   creates the child topic (or posts the marker in a flat chat) and binds; a reply to the marker
   steers the child; child events update the parent's board row; `ApprovalRequested` for a child
   lands in the child's topic, for an unbound session in the owner's chat (and nowhere with
   `approvals: threads`); a press by the thread user resolves, by a stranger toasts `not allowed`;
   `ApprovalResolved` edits the card and disables the buttons; `AskRequested` posts the first
   question with `force_reply`, a reply to the card answers it, `AskResolved` edits it;
   `stopped_message_generation` for a live draft asks `Cancel`; `TurnFinished` renames the topic
   once per title and never after the user renamed it; `BotBlocked` unbinds and publishes
   `Unreachable`; `SecretsEvent::Requested` sends the notice to the owner's chat only;
   `ArtifactShown` for a child lands in the nearest bound ancestor's thread with the right method
   by kind; the lease `holding == false` drops inbound after recording.
5. **Commands** (`commands::tests`): the `@bot` suffix is stripped; owner-only gating; session
   resolution picks the current topic, then the invoker's latest thread in the chat; `/start
   <live code>` pairs, `/start` with a dead code is silent, a bare code as text does nothing; the
   three `setMyCommands` scopes are derived from the one table and `help` lists every command
   once; every `callback_data` the panels emit is ≤ 64 bytes.
6. **Threads** round-trip incl. `topic`, `named_by_user` and `open`; corrupt file → empty.
7. **QR fits** — a `t.me/…?start=…` link renders under 2 000 chars (09 §15.6).
8. **e2e, manual, real bot** — recorded in the PR: `t --telegram-setup` → token → link → tap →
   `paired`; `t --headless`; DM "hi" → 👀 → draft animates → final rich message with a table →
   👌 → topic appears and is named; "run `ls -la` then summarise" with `mutating: ask` → card with
   buttons → Allow → activity card `shell · ls -la` running → done with the first line → text
   continues in a new bubble below; a `sleep 90` tool keeps the card alive and the draft away; Stop
   on a draft → 🤷; `spawn_subagent` → child topic, board updates, result message in the parent;
   typing in the child topic steers it; a terminal session's approval shows up in General and
   Allow works; `/model` panel press sets the model; in a supergroup `@bot` starts a topic and a
   non-member's command gets an ephemeral refusal; a voice note → transcript → spoken clip back;
   kill the daemon mid-draft, restart → the topic is still bound, the next message continues;
   second engine takes the lease → the first stops polling, the second answers, no duplicate.

## 16. Spikes first

Facts the doc assumes and the API docs do not settle. Each is a throwaway
`crates/telegram/examples/*.rs` against a scratch bot, deleted after (00 §11 pattern); results are
folded into §8 before step 3 of §18.

1. **The pin builds and the 10.x payloads are right.** `tgbot = "=0.48.1"` resolves against the
   workspace (one reqwest 0.13, one rustls provider, no openssl; `socks` is in its reqwest
   feature set — confirm it costs nothing); `SendRichMessageDraft` / `SendRichMessage` /
   `EditMessageText::for_chat_message_rich` round-trip against the live API (0.48.1 fixed
   multipart uploads on 2026-09-25 — rich messages with media are untested by us); a synthetic
   update of an unknown kind deserialises as `UpdateType::Unknown`; `Client::download_file` on
   a `getFile` path; `with_host` works for `wiremock`.
2. **Drafts.** How many `sendRichMessageDraft`/s per chat before `RetryAfter`; whether a draft
   counts toward the 1 msg/s chat budget; what 1 s full-text re-sends look like on iOS/Android/
   Desktop; `message_thread_id` on a draft inside a private topic; the Stop button's
   `MessageGenerationStopped` payload; what the client shows inside `<tg-thinking>…</tg-thinking>`;
   whether a `sendRichMessage` from a different process (non-hub engine) replaces the draft as the
   docs say.
3. **Rich Markdown fidelity.** Typical model output end to end (nested lists, a table wider than
   the phone, a 30 000-char message, `Vec<String>` in prose, a `<details>` with a fence inside,
   `<footer>` inside Markdown mode, a `<tg-button type="callback_data">` in the body: does it
   arrive as an ordinary `CallbackQuery` with `data`, and does it survive `editMessageText {
   rich_message }`); the exact `description` strings for a refused message.
4. **Private-chat topics.** The thread id (if any) of General messages; whether a bot-created
   topic delivers the user's messages with `message_thread_id`; `is_name_implicit`;
   `editForumTopic` and `closeForumTopic` in private chats; `sendChatAction` with a thread id;
   how many topics a private chat tolerates; the `forum_topic_edited` / `_closed` service messages.
5. **Reactions.** `setMessageReaction` on the user's message in a private chat; replacing 👀 with
   👌 (one reaction per bot?); groups without `can_react_to_messages`.
6. **Ephemeral.** `is_ephemeral` commands in the group menu; the reply with
   `reply_parameters.ephemeral_message_id`; the 15 s window; admin-any-time delivery.
7. **Privacy mode.** Does an `@mention` text reach a non-admin bot with privacy on? (The docs'
   list omits mentions.) Decides one line of the README.
8. **Files.** `getFile` on a 25 MB document: the exact error text; `sendPhoto` with a 9 000 px
   screenshot (the summed-dimension rule) → document fallback threshold.
9. **Lease handover.** Stop the poller on engine A, start on B: no duplicate; kill A mid-poll,
   start B within the heartbeat gap: measure the duplicate window and confirm `MarkAnswered`
   drops it.

## 17. Open

- **Plan mode.** `TodoChanged` as one message per turn edited in place with a GFM task list
  (`- [x]`) — Rich Markdown renders it natively; ~20 lines once a channel wants it (21 §17).
- **A shared frontend layer.** This is the fourth frontend 09 §14 named as the trigger: the
  command table + dispatcher (`commands.rs`) and the approval/ask lifecycles are now three near
  copies. Extracting a `frontend` library crate (table, asks, card state machines; no rendering)
  is a separate amendment once Telegram is as-built and the fourth copy can be diffed against the
  other three.
- **Feedback.** Nothing consumes a 👍 on a turn; when something does (knowledge harvest
  weighting?), inbound `message_reaction` is one `allowed_updates` entry away.
- **Rich blocks instead of Markdown** (`InputRichMessage.blocks`) if spike §16.3 finds the
  Markdown parser lossy on model output: `markdown`'s event stream → `InputRichBlock`s is
  mechanical but is a second renderer, so only on evidence.
- **Owner sees all approvals from any device** is `approvals: all` here — Telegram's owner chat
  is the one place that is always on a phone; 09 §16's open item has its answer in this channel.

## 18. Order

Each step leaves `theuth` runnable; formatters and the suite once per step.

1. **Spikes** (§16) on a scratch bot; fold results into §8.
2. **Dependency + skeleton**: workspace pin `tgbot = "=0.48.1"`, `Cargo.toml`, `lib.rs`
   (config, actor, `TelegramEvent`, `TelegramStatus`, `Unconfigured`/`DaemonOnly`), `api.rs` +
   pacer + the §4.3 description table, `poll.rs` with the lease gate, backoff and dedup,
   `markdown::{plain, chunks}` (a copy of `signal::text`'s two functions; the frozen file stays).
   Check: §15 items 1–2; `t --headless` with a token logs `Identity` and `Connected`, a DM shows
   up as a debug line, `/help` answers.
3. **Threads + inbound + bubble**: `threads.rs` with topics, `channel::inbound`, workers,
   `bubble.rs` (Draft + Edit, rotation, repair, footer), reactions, typing, Stop, titles.
   Check: §15 items 3 (text half), 4 (routing half), 6; e2e "hi" → draft → rich message → 👌 in a
   named topic.
4. **Cards, reasoning, approvals (thread + owner chat), asks, other events, `show`,
   `message_user`, voice** (§8.4–8.10, §10.1–10.2). Check: §15 item 3 (cards, reasoning), item 4
   (approvals, asks, artifacts); e2e `ls -la` flow, terminal approval in General, a voice note.
5. **Subagents**: child topics, markers in flat chats, task line, board, parent interjects (§9).
   Check: §15 item 4 (child half); e2e spawn flow.
6. **Commands + panels** (§10.3), `--telegram-setup` + pairing (§10.4), tools, RPC methods +
   `bridge.rs` + `export.rs`, boot-time `open` repair (§11). Check: §15 items 5, 7;
   `cargo check -p rpc --features ts`; `just ts` emits the types.
7. **Docs**: README `## Telegram` section; the §19 amendments; this doc to as-built with the
   §15 item 8 record and what was not exercised.

## 19. Amendments to other docs (same commit series)

- **00 §4 / §10 / §13**: dependency line gains `telegram` in the `{signal, slack, …}` set;
  `telegram/   plugin: Telegram channel over Bot API long polling (added in 98)` in §10; §13 row
  `telegram → telegram crate → plugin → 98`; the §1.3 patch/pin table gains the exact `tgbot`
  pin with the ZeroVer reason (§2.2).
- **24 §2–3**: `via: "slack" | "signal" | "telegram"`; the tool description reads "(Slack, Signal
  or Telegram)"; §5–6 gain a §7 "Telegram" paragraph pointing at §8.10 here.
- **45 §4.5**: "Signal and Slack alike" → "every channel"; §5–6 gain the Telegram paragraph
  (`sendVoice` MP3; OGG/Opus inbound).
- **62 §5.3**: title "Slack / Signal / Telegram (owner only)".
- **67 §4.4**: the duty row gains Telegram — "polling is single-consumer; runs on the hub only;
  updates are retained 24 h; the double-hub window is deduplicated by `MarkAnswered` keyed
  `(chat:user, update_id)`" — and §4.7's diagram a third branch.
- **09 §16**: "Live owner change" notes that 98 does it live and the `SetOwner` ask shape is
  reusable.
- **plugins.md §4 step 4**: "Signal" → "Signal / Slack / Telegram — only if the phone or
  workspace needs it".
- **README**: `## Telegram` (setup, the two @BotFather switches, groups, what the commands menu
  shows), the crate list, the config bullet.

## 20. Status (as-built v0.1, 2026-10-05)

Evening addendum (98b, same day): the Settings › Channels card for desktop and mobile and the
engine's hot start on a newly set `telegram_bot` secret (§10.5) — `98-as-built-SETTINGS.md`.

2026-10-06 (98c, `98-as-built-FIXES.md`): a slash command deadlocked the channel (`command_cx`
held the owner guard across `status()`); owner is an atomic, dispatch is per-key lanes + free
tasks for commands/presses/prompt replies, every `Port` ask and every lane-blocking ask is
timed (the direct command arms sit under the one `COMMAND_BOUND`), the actor registers before
the vault wait and reports `Starting` until it answers, the Settings card never renders blank.
Revisions in §3, §7, §10.3, §11, §13.

Shipped on `main` the same day as the design: `crates/telegram` (`lib`, `api`, `poll`, `port`,
`threads`, `worker`, `bubble`, `cards`, `board`, `channel`, `approvals`, `asks`, `commands`,
`ui`, `setup`, `user`), `markdown::{plain, chunks}`, `rpc` methods/bridge/export, `t
--telegram-setup`, `TelegramPlugin` in both plugin vectors, `DEFAULT_CONFIG`, README and the
§19 amendments. Built as a contract-first omp wave (`98-contract.md`; as-built notes
`98-as-built-{CONTRACT,RENDER,ROUTE,COMMANDS,WIRE,INTEGRATE}.md`) with the `tgbot = "=0.48.1"`
pin from §0.1. Steps 2–7 of §18 done; **step 1 (the spikes) was skipped by decision** — there
is no bot to test against from the harness, and Sky tests against a real bot independently;
`98-as-built-INTEGRATE.md` lists the facts that live test must settle first.

Exercised: `cargo test -p telegram` 112 tests (api façade on wiremock, pacer spacing, error
table, poll classify/dedup/conflict/confirming stop, threads round-trip, renderer drafts/edit
mode/rotation/cards/reasoning/reactions/titles/board, channel routing/trust/shapes/albums/
approvals/asks/artifacts/secrets/lease/pairing, commands/panels/guard/setup against wiremock),
`cargo test -p markdown` 19, clippy `-D warnings` and rustfmt on the crate, `cargo check -p rpc
--features ts` with `just ts` emitting the `telegram_*` methods and types, `cargo check -p
theuth`, and a scratch-engine smoke of the `Unconfigured` path and the RPC methods. **No
Telegram API call was made.**

Deviations from the draft, recorded in `98-contract.md` §4 and the as-built notes: `Link`
answers `Option<PairingLink>`; `Incoming::Message` boxed; `Channel.api` is `Arc<dyn Sender>`
with a `live` `Api` beside it (slack's split); flat-chat children have no thread key
(`State.flat_children`); child topics are `named_by_user: true`; `answerCallbackQuery` carries
the toast; `guard` runs in the `Api` façade, not per module; `Outcome::CloseTopic` added by the
`/agents` panel and handled in the channel at integration; `/a2a reload` deferred to the
terminal; 401 backs off instead of stopping. Follow-ups are in `98-as-built-INTEGRATE.md`.
