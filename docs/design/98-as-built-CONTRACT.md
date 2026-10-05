# 98 — As built: CONTRACT (stage 1, the crate skeleton)

Status: 2026-10-05 · branch `feature/98-telegram`, base `99716e6a` · for `98-contract.md` §1–§2.
Four commits: workspace pin → `markdown::{plain, chunks}` → the crate → clippy/fmt fixes.

## What was built

| file | state | lines |
|---|---|---|
| `Cargo.toml` (workspace) | `tgbot = { version = "=0.48.1", default-features = false }` with the §0.1 ZeroVer comment; `telegram = { path = "crates/telegram" }` beside `slack`; `Cargo.lock` committed | — |
| `crates/telegram/Cargo.toml` | slack's shape; deps actually used (see Deviations for the dropped ones); `rustls` with `aws-lc-rs` for the provider install; dev `wiremock`, `tempfile`, `figment`, `serde_yaml`; `ts` feature forwards like slack | — |
| `src/lib.rs` | **real**: `TelegramConfig` (+ `ToolsMode`, `ApprovalsMode`, lowercase serde), `TelegramEvent`, `BotIdentity`, `CallStats`, `TelegramStatus`, `PairingLink`, `Pairing`, `PAIR_TTL`, `CODE_LEN`, `mint_code`, actor `Telegram` + `Status`/`Link`/`Allow`/`RemoveAllowed`/`AllowGroup`/`RemoveGroup`/`SetOwner`, `Plugin::start` (secret `telegram_bot`, `channel == false` → `DaemonOnly`), `TelegramLinkTool` (`telegram_link`, Mutating, link + `link::qr`), `TelegramStatusTool`, `render_status` | 775 |
| `src/api.rs` | **real**: `Target`/`MsgRef`/`Keyboard`/`Button`/`OutFile`/`Ephemeral`/`CommandScope`/`BotCommandSpec`, `ApiError` + `description/is/starts_with`, the §4.3 constants + `is_unreachable`/`is_gone_message`/`is_topic_gone`/`is_not_modified`, `trait Sender` (18 methods), `Api` over `tgbot::api::Client` (`with_max_retries(2).with_max_retry_after(30)`, host override), pacer (per-key gaps + 30/s bucket), counters, `telegram.api{method}` spans, `get_updates`, `file_kind`, `#[cfg(test)] mod mock::MockSender`, 19 wiremock/unit tests | 1 300 |
| `src/poll.rs` | **real**: `RecentKeys`, `PollCtx`, `backoff_delay`, `run` (§3.1 loop incl. the confirming call), `classify` + every §2.5 type, 17 tests (classify on hand-written JSON, three `run` tests over wiremock) | 1 302 |
| `src/threads.rs` | **real**: `key`/`target`, `Topic`, `Thread`, `OpenMsg`, `Threads` (file-backed, saved on every mutation), 5 tests incl. round trip + corrupt file | 417 |
| `src/port.rs` | **real**: `trait Port` (slack's + `load_session`, `ancestors`, `session_title`) + `LivePort` | 182 |
| `src/channel.rs` | **skeleton**: `spawn`, `Bus` (eight event types → one ordered mpsc), identity task (`getMe` with backoff → `Identity` event, `deleteWebhook`, `setMyCommands` × `commands::scopes(owner)`, boot repair, `sync_poller`), lease task (mesh `Status` → `follower.set_self`; `MeshEvent::Lease` → `observe` → `sync_poller`), inbound task (log + count `Ignored`), event task (log + drop), `Handle` real (`status` incl. `api.stats()`, `link`, allow/remove/groups/set_owner), `take_pairing` | 480 |
| `src/worker.rs` | **stub** `NEEDS(RENDER)`: all §2.6 types incl. `WorkerMsg::Output` and `WorkerCtx.child_name`; `spawn` drains, answers `Output` with `None`, logs at debug | 170 |
| `src/commands.rs` | **stub** `NEEDS(COMMANDS)`: `COMMANDS` (signal's table minus approve/deny/signal, plus start/telegram), `owner_only`, `scopes` (real), `CommandCx`, `Reply`, `Outcome`, `Expect`, `help` (real GFM table); `handle`/`continue_prompt` → `Outcome::Reply(help())`; 2 tests | 304 |
| `src/ui.rs` | **stub** `NEEDS(COMMANDS)`: `action` → `None`, `guard` → identity, `table` real; 1 test | 55 |
| `src/setup.rs` | **stub** `NEEDS(COMMANDS)`: `Terminal`, `Tty`, `deep_link` real; `run` → `Err("telegram setup: not implemented yet")`; 1 test | 70 |
| `src/user.rs` | **stub** `NEEDS(ROUTE)`: `impl UserSink for Handle` (`name() == "telegram"`, `deliver` → `Ok(None)` / `Err("telegram: message_user not implemented yet")`) | 23 |
| `crates/markdown/src/plain.rs` + `lib.rs` re-export | **real**: `plain`, `chunks` copied from `crates/signal/src/text.rs` with its 9 tests (signal untouched) | 261 |
| `docs/design/98-contract.md` | §1 markdown row, §2.6 `Output` + `child_name` written as contract, §2.1/§2.8 `Link` reply, §4 Deviations | — |

## Judgement calls

- **Multipart, not JSON.** tgbot sends `sendRichMessage`, `sendRichMessageDraft`, `editMessageText` (rich) and the file methods as multipart forms (object fields as JSON strings). The api tests parse the form into a JSON object (`body()` helper) so assertions read the same for both encodings.
- **Pacing tests use real time** (1.0 s / 1.5 s): wiremock serves from its own runtime thread, so `tokio::time::pause` would not drive the HTTP round trip reliably. The whole crate's suite runs in ~1.6 s.
- **`poll::run` and `getUpdates` offsets.** `last` starts at `-1`, so the first call sends `offset: 0` (Telegram: earliest unconfirmed); the confirming call on stop sends `offset: last + 1, limit: 1, timeout: 0` and omits `allowed_updates`. The `reset_allowed = true` flag stays on until the first `Ok`, so a conflict/backoff on boot still resets the set afterwards (asserted).
- **`message_thread_id` as topic** only when `is_topic_message == true` or the chat is private (private topics chats, Bot API 9.3) — a plain supergroup also sets it for reply chains, which would otherwise create phantom topic keys.
- **Entity offsets are UTF-16 code units**: `classify` slices `@mention` text through a `slice_utf16` helper; the command word is taken from the `bot_command` entity at offset 0 and split on `@` (`for_other_bot` compares the suffix to `me.username` case-insensitively).
- **Photos** carry `mime: Some("image/jpeg")` (Telegram re-encodes) and the largest `PhotoSize` by `width * height`.
- **`Api::new` installs the `aws-lc-rs` rustls provider** when none is installed, like `slack::Api::new`; `main.rs` already does it for the daemon, tests rely on the crate doing it.
- **`send_file` bodies are streamed** (`InputFileReader` over a `Cursor`); tgbot cannot clone a streaming request, so a `retry_after` on an upload is not retried by the client and surfaces as `RateLimited` — the pacer makes that rare.
- **`download`** checks `file_size > max` before fetching and also caps the streamed bytes at `max` (a `file_size`-less answer cannot exceed the cap either). `Bad Request: file is too big` on `getFile` → `TooBig`.
- **Terminal mode `Link`**: without the channel there is no `getMe`, so `Link { owner: None }` is an `Err` telling the user to run `t --headless` or pass the id; `Link { owner: Some(id) }` works everywhere (config write + live when the channel is up).
- **Boot repair** clears keyboards (`edit_markup(None)`) instead of appending `· interrupted` — `OpenMsg` has no text to append to (NEEDS(RENDER) in `channel::repair_open`).
- **`Threads::bind` enforces one session ↔ one thread** (slack's rule without the agent-desk exception; Telegram has no desks).

## Gates (verbatim, `PATH=…/1.98.1…/bin`, `CARGO_TARGET_DIR=…/theuth-98-target`)

```
$ cargo check -p telegram --all-targets
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 5.02s
$ cargo test -p telegram
test result: ok. 49 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.58s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
$ cargo clippy -p telegram --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 5.50s
$ cargo fmt -p telegram -- --check
(clean)
$ cargo test -p markdown
test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
$ cargo clippy -p markdown --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.73s
$ cargo fmt -p markdown -- --check
(clean)
```

No network beyond crates.io; `api.telegram.org` never contacted (every test runs against wiremock on 127.0.0.1 or `MockSender`).

## NEEDS

- `NEEDS(RENDER)`: `worker::spawn` body; decide whether `OpenMsg` persists the message tail so boot repair can append `· interrupted` (today: keyboard cleared only). `markdown::plain` / `chunks` are ready for the plain fallback.
- `NEEDS(ROUTE)`: `channel::handle_inbound` (trust, `tasks::MarkAnswered` lease gate, `/start <code>` via `Channel::take_pairing`, commands, asks, workers, albums), `channel::handle_event` (fan-out), `Channel.cfg` / `State.workers` / `Channel.port` are present and `#[allow(dead_code)]` until then; `user::deliver`.
- `NEEDS(COMMANDS)`: `commands::handle` / `continue_prompt`, `ui::action` / `guard`, `setup::run`.
- `NEEDS(WIRE)`: `main.rs` plugin push + `--telegram-setup`, rpc `telegram_*` + `TelegramEvent` bridge/export, README, docs amendments.

## Remaining

Nothing from the brief is open. Deviations are listed in `98-contract.md` §4.
