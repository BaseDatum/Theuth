# 98c — as built, LIVE-PROOF (2026-10-06)

The Telegram slash-command wedge (user report on 2.9.0) reproduced and then proved fixed on a
real engine against a local Bot API mock. Worker omp-98c-proof on `test/98c-live-proof` off
`ef1ad0a3` (= origin/main). Script: `scripts/telegram-live-proof/` (README there; one command).

- Reproduction (A): `ef1ad0a3` + the seam `e3828e4a` (cherry-picked as `e3ffe49e`).
- Fix (B): throwaway `tmp/98c-integration` = `test/98c-live-proof` + `git merge fix/98c-telegram-wedge`
  (fix commit `69ee546e fix(telegram): a slash command no longer wedges the channel (98c)`).
- Mock: [telegram-mock-ai](https://github.com/skrashevich/telegram-mock-ai) `314c594` +
  `scripts/telegram-live-proof/mock.patch` (7 files, +253/−22).
- Final (C): `af348178` on `fix/98c-telegram-wedge` round 2, built from the sibling's worktree
  (`ENGINE_SRC`); adds P3b (prompt reply whose continuation hangs).
- Evidence dirs (kept on this machine, not committed): `/tmp/98c-proof/out-main/` (A),
  `/tmp/98c-proof/out-fix/` (B), `/tmp/98c-proof/out-final/` (C); earlier rounds in `*-r1/`.

## Result

| | A: main + seam | B: fix |
|---|---|---|
| P1 regular message | PASS | PASS |
| P2 `/sessions` | **FAIL** (no reply in 120 s) | PASS (panel in 1.0 s) |
| P3 panel button | SKIP (no panel) | PASS |
| P4 message after the command | **FAIL** (never dispatched, 120 s) | PASS (1.0 s) |
| P5 `telegram_status` during P2–P4 | **FAIL** (78 of 80 samples timed out) | PASS (max 2 ms) |
| P6 restart + message | PASS | PASS |

One paragraph: the first slash command self-deadlocks the channel. `Channel::command_cx`
builds `CommandCx { owner: *self.owner.lock(), …, status: self.status(), … }`; the guard from
the `owner` field initializer is a temporary that lives until the whole struct literal is
built, and `status()` locks the same (non-re-entrant parking_lot) mutex again. The single
sequential inbound task parks on an OS condvar forever, so no later update is answered while
the poller — its own task — keeps `getUpdates` going and acks the ignored updates (offset
`3 → 4` after P4). Every `telegram_status` (`Telegram` actor `Status` → `Handle::status` →
`Channel::status` → `owner.lock()`) then parks the actor on the same mutex, so the Settings
card never gets an answer. A restart clears it until the next command. Both stacks are in
the macOS thread sample below. The fix removes the second mutex (owner = `AtomicU64`),
fences every Port ask with a timeout and runs commands/presses off the message lanes; on it
all eight steps pass twice in a row.

## The mock (telegram-mock-ai `314c594`)

Read first (`internal/`), as the brief asked:

- **Dispatch**: `/bot<token>/<method>` (method also accepted in body/query), case-insensitive
  table in `internal/api/router.go`. Any token auto-registers a bot (`NewRegistry(true)`).
- **Implemented** (of what theuth calls): `getMe`, `getUpdates` (long poll ≤ 50 s, `offset`
  confirms `< offset`, update ids start at 1), `deleteWebhook`, `setMyCommands` (scope ignored,
  stored per bot), `setMessageReaction` (400 unless the message is stored), `sendChatAction`,
  `sendMessage`, `sendRichMessage`, `sendRichMessageDraft`, `editMessageText`
  (`rich_message` supported), `editMessageReplyMarkup`, `answerCallbackQuery`, `getFile`,
  `deleteMessage`. **Not implemented**: `createForumTopic` (topics are off in the proof:
  `getMe.has_topics_enabled=false`, private chat).
- **Unknown method** → HTTP 400 `{"ok":false,"error_code":400,"description":"Bad Request:
  method not found"}` (api.telegram.org answers 404; either way tgbot surfaces an error).
- **Private chat**: `seed.chats[].type: private` with `id == user id` and `members: [id]` is
  accepted; `sendRichMessage*` even auto-creates a private chat for an unknown `chat_id`.
- **Injecting a user message**: `POST /api/chats/<chat_id>/messages {"user_id","text"}` stores
  the message (so reactions on it work) and enqueues an `Update` for every registered bot.
- **Injecting a `callback_query`** (or any raw update): `POST /api/bots/<token>/updates
  <Update JSON>`; it is enqueued as-is (not stored). `inject.ts callback <message_id> <data>`
  builds it from the stored message and the owner user.
- **Side effects to disable**: LLM replies (`TELEGRAM_MOCK_LLM_ENABLED=false`), proactive
  traffic (`proactive.enabled: false`) and — not configurable upstream — a "welcome" user
  message the mock injects into every chat on a bot's first API call (would start a model
  turn under the proof's feet).

### Gaps found by running theuth against it → `mock.patch`

| gap | symptom in theuth | patch |
|---|---|---|
| `getMe` returns a bare `User` | tgbot `Bot` requires `can_join_groups`, `can_read_all_group_messages`, `supports_inline_queries`, `has_topics_enabled`, … → "error decoding response body", getMe retried forever, channel never `Identity` | `handler_info.go`: full Bot API 9.x+ shape, all flags present (`has_topics_enabled=false`) |
| private `Chat` without `first_name` | tgbot `PrivateChat.first_name: String` → every `Message` response undecodable | `store.go` `CreateChat`: private chat takes the peer user's names (or "Private") |
| `multipart/form-data` never parsed (`ParseForm` ignores multipart) | tgbot sends `sendRichMessage*` as multipart → 400 `chat_id is required` → rich → plain fallback | `params.go`: `ParseMultipartForm` for multipart |
| `sendRichMessageDraft` answers a `Message` | tgbot's response type is `bool` → decode error → `StreamFallback reason="draft: …"` | `handler_messages.go`: answers `True` like api.telegram.org |
| `sendRichMessage` drops `reply_markup` | P3 cannot find a `cmd:` button | `handler_messages.go`: stores `reply_markup` on the message |
| injected text has no entities | theuth parses commands from the `bot_command` entity at offset 0, not from `/` | `admin.go`: a leading `/word` gets `bot_command` (0, len) unless `entities` is given |
| no record of what the bot called | nothing to assert on | `ledger.go` + `GET/DELETE /api/calls?since=`: seq, time, method, params (JSON / multipart fields), status, response, duration |
| welcome message on first connect | spurious model turn | `router.go`: `TELEGRAM_MOCK_WELCOME_ENABLED=false` |
| injected message cannot be a reply | the `force_reply` continuation needs `reply_to_message.message_id` | `admin.go`: `reply_to_message_id` on `POST /api/chats/<id>/messages` |
| `force_reply` markup echoed as `{"inline_keyboard":null}` | tgbot cannot decode a null keyboard on the prompt message quoted in the reply | `handler_messages.go`: only inline keyboards are stored on the message |

The remaining distinction the proof relies on: "wedged" = the ledger shows `getUpdates`
continuing while no `setMessageReaction`/`send*` follows an injected message; "mock gap" =
a `4xx` row in the ledger (none in the final runs).

## Recipe deviations (documented, not silent)

- **The vault is not copied.** `~/.theuth/secrets.sqlite` seals its mesh key under a per-data-dir
  key held in the macOS Keychain (`service theuth`, `account secrets:<absolute data_dir>`,
  `crates/secrets/src/key.rs`). Under `/tmp/98c-proof/data-*` a fresh key is minted and the
  copied vault fails: `secrets vault unavailable: …/secrets.sqlite: the mesh key does not open
  with this engine's key` (first attempt, `out-main-r1` predecessor). Cloning the user's key
  into a second Keychain item (with an any-app ACL so the debug `t` can read it) was rejected
  as both unsafe and non-rerunnable. `proof.sh` instead runs `THEUTH_SECRETS__KEY_STORE=file`
  and seeds the scratch vault from `$ANTHROPIC_API_KEY` through the engine's own
  `credentials.yaml` import (read and unlinked on first open).
- **Model**: therefore `THEUTH_AGENT__MODEL=anthropic/claude-haiku-4-5` (`PROOF_MODEL` env
  overrides), not `openai/gpt-5.6-sol` (whose `oauth/openai` lives only in the sealed vault).
  The wedge is in `crates/telegram` dispatch; the model only has to produce a reply.
- **Isolation env** so a scratch engine cannot touch the user's live one on the same box:
  `THEUTH_RPC__LISTEN=127.0.0.1:0` (the copied config says `127.0.0.1:7777`),
  `THEUTH_SLACK__ENABLED=false` (the vault would otherwise be empty anyway),
  `THEUTH_MESH__DISCOVERY=false`, `THEUTH_MESH__RELAY=none` (an isolated mesh still takes the
  hub lease: `mesh: lease → Some(…) (prefer)`, so the poller runs).
- `telegram.owner` = the mock's seed user `777000111`; the private chat id is the same.

## Run A — reproduction (main `ef1ad0a3` + seam)

`proof.sh main`, 13:13–13:18 local, engine pid 25771, `/tmp/98c-proof/out-main/`.

| step | status | ms | detail |
|---|---|---:|---|
| P0 | PASS | 1139 | getMe+setMyCommands seen; telegram_status 34 ms → Identity theuth_proof_bot polling=true |
| P1 | PASS | 1138 | 👀 → sendRichMessageDraft @606ms, final sendRichMessage @404ms, closed by setMessageReaction(👌) @2ms |
| P2 | FAIL | 134246 | no sessions panel within 120s; telegram_status now: 5232 ms ERR rpc telegram_status timed out after 5000 ms |
| P3 | SKIP | 0 | no panel from P2 |
| P4 | FAIL | 132589 | no setMessageReaction (👀) within 120s: the message was never dispatched; telegram_status now: 5232 ms ERR timed out |
| P5 | FAIL | 0 | samples=80 max=3ms errors=78 during_P2-P4=78 (errors 78) |
| P6 | PASS | 1667 | restart; telegram_status 33 ms, 98 ms after engine.json → Identity theuth_proof_bot polling=true |
| P6b | PASS | 1855 | 👀 → sendRichMessageDraft @615ms, final sendRichMessage @1014ms, closed by setMessageReaction @2ms |

Identical shape in the first round (`out-main-r1`, 12:52–12:58): P1 PASS, P2/P4/P5 FAIL, P6 PASS.

### Mock ledger around the failure (`ledger-with-getupdates.txt`, local time)

```
  6 13:13:10.728 getUpdates           200  1136ms {"allowed_updates":[],"limit":100,"offset":0,"timeout":30}
  7 13:13:11.868 setMessageReaction   200     0ms {"chat_id":777000111,"message_id":1,"reaction":[{"type":"emoji","emoji":"👀"}]}   ← P1 (update 1)
  9 13:13:11.876 sendChatAction       200     0ms {"action":"typing","chat_id":777000111}
 10 13:13:12.461 sendRichMessageDraft 200     0ms {"can_stop":true,"chat_id":777000111,"draft_id":1,"rich_message":{…}}
 11 13:13:12.779 sendRichMessage      200     0ms {"chat_id":777000111,"rich_message":{"markdown":"**thought 0 s** … Hello."}}
 12 13:13:12.779 setMessageReaction   200     0ms {… "message_id":1, "emoji":"👌"}                                    ← LAST call before silence
 13 13:13:11.867 getUpdates           200  1168ms {"limit":100,"offset":2,"timeout":30}   ← returns update 2 = "/sessions" at 13:13:13.035
 14 13:13:13.037 getUpdates           200 30001ms {"limit":100,"offset":3,"timeout":30}   ← update 2 acked; nothing sent for it
 15 13:14:40.801 getUpdates           200 30002ms {"limit":100,"offset":3,"timeout":30}
 16 13:15:10.804 getUpdates           200 21894ms {"limit":100,"offset":3,"timeout":30}   ← returns update 3 = P4 "hello again" at 13:15:32.68
 17 13:15:32.684 getUpdates           200 30002ms {"limit":100,"offset":4,"timeout":30}   ← update 3 acked; no 👀, no send
 18–21               getUpdates           200 30000ms {"limit":100,"offset":4,"timeout":30}   (×4, until SIGTERM)
 22 13:18:00.957 getUpdates           200     0ms {"limit":1,"offset":4,"timeout":0}      ← shutdown confirmation
```

Injections (`mock.out`): 13:13:11.865 P1, 13:13:13.035 `/sessions` (entity `bot_command` 0/9),
13:15:32.682 P4, 13:18:04.644 P6b. Between row 12 and the SIGTERM the bot made **no call other
than `getUpdates`** — exactly the user's "poller keeps calling getUpdates with an advancing
offset, nothing is sent".

### Engine log (`RUST_LOG=info,telegram=debug,tgbot=debug`), same window

```
12:13:12.778Z DEBUG telegram.api{method="sendRichMessage"}: tgbot::api::payload: Sending multipart body …
12:13:12.779Z DEBUG telegram.api{method="setMessageReaction"}: tgbot::api::payload: Sending JSON body …
12:13:13.037Z DEBUG telegram.api{method="getUpdates"}: tgbot::api::payload: Sending JSON body …   ← offset 3
12:13:13.626Z  INFO agent::loop_: title session=01a11121-… generation=0 chars=Some(8)             ← P1's title, unrelated
12:14:40.800Z DEBUG telegram.api{method="getUpdates"} …
12:15:10.804Z DEBUG telegram.api{method="getUpdates"} …
12:15:32.683Z DEBUG telegram.api{method="getUpdates"} …   ← offset 4 (P4 acked)
… one getUpdates every 30 s, nothing else from `telegram` …
12:18:00.956Z  WARN mesh: exiting: SIGTERM
```

Not one `telegram::channel` line for `/sessions`: `handle_message` → `command` never reaches a
log statement (main has no info! per command; the fix adds one).

### Thread sample while wedged (`threads-P2.txt`, `sample 25771 2`, demangled with rustfilt)

Of 16 `tokio-rt-worker` threads, two are parked on the **same** mutex,
`lock_api::Mutex<parking_lot::RawMutex, Option<u64>>` = `Channel::owner`:

```
Thread tokio-rt-worker                                    (the channel's inbound task)
  telegram::channel::run::{closure#0}::{closure#2}
  <telegram::channel::Channel>::handle_inbound
  <telegram::channel::Channel>::handle_message
  <telegram::channel::Channel>::command
  <telegram::channel::Channel>::command_cx                channel.rs:1566-1576
  <telegram::channel::Channel>::status                    channel.rs:698  `*self.owner.lock()`
  <lock_api::mutex::Mutex<parking_lot::raw_mutex::RawMutex, core::option::Option<u64>>>::lock
  <parking_lot::raw_mutex::RawMutex>::lock_slow → parking_lot_core::park → _pthread_cond_wait → __psynch_cvwait

Thread tokio-rt-worker                                    (the `Telegram` kameo actor)
  kameo::actor::spawn::recv_mailbox_loop::<telegram::Telegram>
  <telegram::Telegram as kameo::message::Message<telegram::Status>>::handle
  <telegram::channel::Handle>::status
  <telegram::channel::Channel>::status                    channel.rs:698
  <lock_api::mutex::Mutex<parking_lot::raw_mutex::RawMutex, core::option::Option<u64>>>::lock
  <parking_lot::raw_mutex::RawMutex>::lock_slow → … → __psynch_cvwait
```

The holder is the first thread itself: `command_cx` evaluates `owner: *self.owner.lock()` (guard
temporary alive for the rest of the struct literal) and then `status: self.status()`, which
locks `owner` again. Same two stacks in `threads-P4.txt` (13:17:44) and in both round-1 samples.
The remaining 14 workers are idle in the scheduler; the poller is on its own task and not
blocked, which is why `getUpdates` continues.

### `telegram_status` latency (`latency.csv`, one sample per 2 s over the whole run)

- Before P2: 2 samples, 1–2 ms (`Unconfigured` → `Connected`).
- From the `/sessions` injection to SIGTERM: **78 of 78 samples failed** — 38 × `rpc
  telegram_status timed out after 5000 ms`, 40 × `connect timeout` (the sampler reconnects
  after a timeout; every other WebSocket handshake then failed to complete within 2 s —
  observed, not explained; the two parked workers do not account for it). The step-level
  probes agree: `telegram_status` 5232 ms → timeout after P2 and after P4, 10 s timeout for the
  pre-restart snapshot.
- So: **the status call stalls (lock), it does not merely answer stale state**. The Settings card
  reads exactly this RPC → blank card.
- After the restart (P6): 33 ms, `Identity theuth_proof_bot polling=true`; P6b turn answered in
  1.9 s. The user's "after a restart it wouldn't even show up" did **not** reproduce here: the
  restarted engine (same data dir, token already in the vault) comes up `Identity` within 98 ms
  of `engine.json`. [INFERENCE] his restarted engine most likely received another command (or the
  desktop issued one) before the card loaded.

### Variance worth knowing

In an earlier smoke run (25 s timeouts, mock still answering 400 on multipart rich sends and
P1 falling back to `sendMessage`) the poller **also** stopped after the wedge (`getUpdates`
offset 3 returned with the next update and no further poll followed until SIGTERM) and new RPC
WebSocket connections timed out throughout. Not reproduced in the three 120 s runs, where the
poller kept going; recorded here because it is a second face of the same deadlock (a parked
worker is a parked OS thread), not a different bug.

## Run B — the fix (`tmp/98c-integration` = `test/98c-live-proof` ∪ `fix/98c-telegram-wedge`)

`proof.sh fix`, 13:20 local, `/tmp/98c-proof/out-fix/`. Same mock binary and patch as run A.

| step | status | ms | detail |
|---|---|---:|---|
| P0 | PASS | 1162 | getMe+setMyCommands seen; telegram_status 45 ms → Identity theuth_proof_bot polling=true |
| P1 | PASS | 1969 | 👀 → sendRichMessageDraft @1012ms, final sendRichMessage @811ms, closed by setMessageReaction @1ms |
| P2 | PASS | 1087 | sendRichMessage message_id=4 @1014ms: `\| id \| title \| model \| age \| … \| e7b28579 \| hello, reply with one word \| anthropic/… \|` |
| P3 | PASS | 886 | pressed `cmd:resume:e7b28579` → answerCallbackQuery, editMessageText @809ms (`resumed e7b28579 · Greeting…`) |
| P4 | PASS | 970 | 👀 → sendRichMessageDraft @608ms, final sendRichMessage @204ms, closed by setMessageReaction @1ms |
| P5 | PASS | 0 | samples=4 max=2ms errors=0 during_P2-P4=2 (errors 0) |
| P6 | PASS | 1285 | restart; telegram_status 25 ms, 80 ms after engine.json → Identity theuth_proof_bot polling=true |
| P6b | PASS | 1136 | 👀 → sendRichMessageDraft @606ms, final sendRichMessage @406ms, closed by setMessageReaction @1ms |

First round (`out-fix-r1`, 13:02–13:07, draft still falling back): all PASS as well
(P2 940 ms, P3 954 ms, P4 1411 ms).

Ledger excerpt (B): the command and the press now produce calls, and the fix's new log lines
name them:

```
 13 13:20:31.694 setMessageReaction   {… "message_id":1, "emoji":"👌"}                        ← P1 done
 15 13:20:32.694 sendRichMessage      {"chat_id":777000111,"reply_markup":{"inline_keyboard":[[{"text":"Resume e7b28579","callback_data":"cmd:resume:e7b28579"}]]},"rich_message":{"markdown":"| id | title | model | age | …"}}
 17 13:20:32.934 answerCallbackQuery  {"callback_query_id":"1791289232932"}
 18 13:20:33.698 editMessageText      {"chat_id":777000111,"message_id":4,"rich_message":{"markdown":"resumed e7b28579 · Greeting…"}}
 20 13:20:33.846 setMessageReaction   {… "message_id":5, "emoji":"👀"}                        ← P4 dispatched
 24 13:20:34.723 sendRichMessage      {"chat_id":777000111,"rich_message":{"markdown":"**thought 0 s** … Hi."}}
```
```
12:20:31.816Z INFO telegram::channel: telegram command name=sessions user=777000111 chat=777000111 outcome="Reply"
12:20:32.934Z INFO telegram::channel: telegram press kind="cmd" user=777000111 chat=777000111
```

P5 caveat: the fixed run is so short (P0–P4 in 7 s) that the 2 s sampler got 4 samples, 2 of
them inside P2–P4 (1 ms and 0 ms). Every step-level `telegram_status` probe in B answered in
25–45 ms.

## Run C — final branch `af348178` (`fix/98c-telegram-wedge` round 2, 14:08 local)

Built directly from the sibling's worktree (`ENGINE_SRC=/Users/Sky.Moore/Develop/theuth-98c`,
HEAD `af348178`, which already folds in this proof branch and main `5044521e`). New step
**P3b** before P4: `/skills add` with no argument → `force_reply` prompt; the owner replies to
that prompt (`reply_to_message_id` = the prompt's id) with `http://10.255.255.1/hang.git` (a
non-routable host, so the engine's `AddSkillRepo` clone stalls) and immediately sends a plain
"still there? one word". Script changes: `inject.ts message … --reply-to <id>`,
`inject.ts await … --exclude <regex>` (a channel timeout notice is not a model reply),
`proof.sh` `ENGINE_SRC`, P6 records the sequence of states seen. Evidence `/tmp/98c-proof/out-final/`
(first attempt with the loose matcher: `out-final-r1/`).

| step | status | ms | detail |
|---|---|---:|---|
| P0 | PASS | 1219 | getMe+setMyCommands seen; telegram_status 35 ms → Identity theuth_proof_bot polling=true |
| P1 | PASS | 1202 | 👀 → sendRichMessageDraft @812ms, final sendRichMessage @204ms, closed by 👌 @1ms |
| P2 | PASS | 888 | sendRichMessage message_id=4 @807ms: sessions table, `Resume ae7b92e2` button |
| P3 | PASS | 966 | pressed `cmd:resume:ae7b92e2` → answerCallbackQuery, editMessageText @809ms |
| P3b | PASS* | 75827 | 👀 @0 s → **channel notice `⚠️ engine did not answer in 20 s` + 😱 @20 s** → model reply (draft @65.5 s, final +2 ms), closed by 👌; prompt itself answered `error: engine did not answer in 20 s` @21 s |
| P4 | PASS | 1166 | 👀 → sendRichMessageDraft @607ms, final sendRichMessage @404ms, closed by 👌 @1ms |
| P5 | PASS | — | samples=42 max=7ms errors=0 during_P2-P4=40 (errors 0) — `telegram_status` never stalled, including through the 75 s clone |
| P6 | PASS | 1041 | restart; telegram_status 24 ms, 69 ms after engine.json → Identity theuth_proof_bot polling=true; states seen: Identity |
| P6b | PASS | 1933 | 👀 → sendRichMessageDraft @810ms, final sendRichMessage @810ms, closed by 👌 @1ms |

\* PASS by the stated criterion (👀 and a model reply within 120 s), **with a finding**: the
channel is no longer the bottleneck — the prompt reply ran free, bounded and answered, and the
plain message was dispatched instantly — but the *engine* did not take `SendInput` for the
whole duration of the stalled clone. The reply arrived at +75 s after the hang URL only because
macOS gave up the TCP connect (`agent::skills` reloads skills at the same instant). A host
that accepts the connection and then stalls would hold every Telegram turn until the clone ends;
the channel would keep posting `⚠️ engine did not answer in 20 s` + 😱 per message (as it did
for both P3b and P4 in `out-final-r1`, where P4 was sent 20 s into the clone). The engine logs
nothing about the clone start or failure.

### P3b ledger (`out-final/ledger.txt`, local time; injections from `mock.out`)

```
injected 14:08:22.24  "/skills add"                              (msg 5)
 20 14:08:23.00 sendRichMessage      {"chat_id":777000111,"reply_markup":{"inline_keyboard":[],"force_reply":true},"rich_message":{"markdown":"Reply with the URL"}}   (msg 6, the prompt)
injected 14:08:23.11  "http://10.255.255.1/hang.git" reply_to_message_id=6   (msg 7, the reply)
injected 14:08:23.18  "still there? one word"                    (msg 8, the plain message)
 22 14:08:23.183 setMessageReaction   {"chat_id":777000111,"message_id":8,"reaction":[{"emoji":"👀"}]}        ← dispatched at once (lane free)
 23 14:08:43.186 sendRichMessage      {"chat_id":777000111,"rich_message":{"markdown":"⚠️ engine did not answer in 20 s"}}   ← SendInput ask bound (20 s)
 24 14:08:43.187 setMessageReaction   {… "message_id":8, "emoji":"😱"}
 25 14:08:44.187 sendRichMessage      {"chat_id":777000111,"rich_message":{"markdown":"error: engine did not answer in 20 s"}}   ← the prompt reply's own bound (21 s)
 28 14:09:38.190 sendChatAction       {"action":"typing","chat_id":777000111}                                 ← engine unblocked (+75 s)
 29 14:09:38.690 sendRichMessageDraft {… "draft_id":1 …}
 30 14:09:38.891 sendRichMessage      {"chat_id":777000111,"rich_message":{"markdown":"**thought 0 s** … Yes."}}   ← the late model reply to msg 8
injected 14:09:39.06  "hello again, one word" (P4)               → 👀 @ .068, draft @ .692, final @ .971, 👌 @ .972
```

### Engine log (round-2 lines)

```
13:08:22.242Z INFO telegram::channel: telegram command name=skills user=777000111 chat=777000111 outcome="Prompt"
13:08:28.185Z WARN telegram::port: telegram: engine ask still unanswered ask="SendInput" waited_s=5
13:08:43.184Z WARN telegram::port: telegram: engine ask timed out ask="SendInput" waited_s=20
13:08:43.184Z WARN telegram::channel: agent ask failed error=engine did not answer in 20 s
13:08:44.115Z WARN telegram::channel: telegram command timed out command="prompt reply" bound_s=21
13:08:44.116Z INFO telegram::channel: telegram prompt reply expect=SkillsAdd user=777000111 chat=777000111 outcome="Reply"
13:09:38.206Z WARN agent::skills: skill front matter invalid, using file as body path=…/skills/pdf.md   ← skills reloaded = clone attempt over
```

P6 on this branch: `rpc wait` connects only once the RPC server accepts, by which time
`TokenRead` has already run, so the 200 ms status poll saw `Identity` straight away and never
caught `Starting` (the state exists; it is just shorter than the RPC start-up here).

Mock additions for this round (in `mock.patch`): `reply_to_message_id` on injected messages
(stored, so the engine sees `reply_to_message.message_id`; 400 if the quoted message is not in
the chat), and `force_reply` / non-inline markups are no longer echoed as
`"reply_markup":{"inline_keyboard":null}` on the stored message (tgbot cannot decode a null
keyboard; the `force_reply` request is still visible in the ledger params).

## Not covered versus real Telegram

- **Draft pacing / animation**: the mock accepts `sendRichMessageDraft` at any rate and only
  answers `True`; Telegram's per-chat draft cadence, the "stop" button, and draft→message
  replacement semantics are untested (both runs show draft → `sendRichMessage` → 👌 only).
- **Rich Markdown acceptance**: the mock stores `rich_message` as opaque JSON. Whether
  Telegram accepts `<tg-thinking>`, `<blockquote expandable>`, tables and `<footer>` as sent
  is not proven here (98 §8.1 guard untested).
- **Topics / forum groups**: `has_topics_enabled=false`, private chat only; `createForumTopic`
  is not implemented by the mock. The `Resume` press therefore took the flat-chat branch.
- **Reactions set**: the mock accepts any emoji and never answers `REACTION_INVALID`; the
  👀/👌/😱/🤷 set is not validated against Telegram's allowed list.
- **Rate limits (429 / `retry_after`)**, `409 Conflict` on a second poller, file uploads
  (`getFile`/`sendDocument` paths), group mention gating, ephemeral messages' visibility.
- **Desktop card**: `telegram_status` was exercised over RPC, not through the Settings UI.
- **The user's model/provider** (`openai/gpt-5.6-sol`): see deviations.

## Rerun

```sh
export ANTHROPIC_API_KEY=…                       # model key for the scratch engine
scripts/telegram-live-proof/proof.sh main        # on ef1ad0a3 + seam → expect P2/P4/P5 FAIL
ENGINE_SRC=/path/to/fix-worktree scripts/telegram-live-proof/proof.sh final   # build from another checkout → expect all PASS
```

Prereqs, knobs (`MODEL_TIMEOUT`, `STEP_TIMEOUT`, `PROOF_MODEL`, `--keep`, `--no-build`) and the
evidence layout: `scripts/telegram-live-proof/README.md`. The script SIGTERMs only the engine it
started (matched on `--data-dir /tmp/98c-proof/data-<label>`) and the mock it started, and
removes its data dir unless `--keep`.
