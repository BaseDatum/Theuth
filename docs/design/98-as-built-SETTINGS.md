# 98b — as built, SETTINGS (2026-10-05 evening)

The Settings › Channels Telegram card (98 §10.5) for desktop and mobile. Worker 98b-UI on
`feat/98b-ui` off `f16a8c79`; the engine hot start (§10.5 "Hot start") is 98b-ENG's section.


## UI

### Files

- `crates/desktop/ui/src/components/settings/Channels.tsx` — `<Telegram />` after `<Slack />`;
  `CHANNEL_EVENTS` gains `Identity`, `Conflict`, `Unreachable`, `PairingLink`, `Paired` (the
  other six Telegram names already existed as Signal/Slack names) and the `satisfies` union
  carries `TelegramEvent`; the invalidate also hits `queryKeys.telegram`. Exported for tests:
  `telegramState`, `telegramId`, `secondsLeft`, `mmss`.
- `crates/desktop/ui/src/components/settings/Channels.test.ts` — new: one phrase per variant,
  the idle suffix, the id guard, the countdown, and two `renderToStaticMarkup` smokes of the
  whole `Channels` pane (no token; live pairing link) with `useApi` mocked as `SandboxVms.test.ts` does.
- `crates/desktop/ui/src/rpc/api.ts` — the six `telegram_*` typings (after `slack_status`) and
  names (after `tasks_list`, sorted); imports `PairingLink`, `TelegramStatus`.
- `crates/desktop/ui/src/lib/rpc.ts` — `queryKeys.telegram`.
- `crates/desktop/ui/src/rpc/events.ts` — `TelegramEvent` added to `WireEvent` (not in the
  brief's file list, but `on('Identity', …)` does not typecheck without it; one line).

### Judgement calls

- **`bigint` ids.** ts-rs 12 exports `u64`/`i64` as `bigint` (`owner`, `allowed`, `groups`,
  `expires_at_ms`, `Paired.user`, …; the telegram crate is off-limits, so no `ts(type = "number")`).
  The JSON parser hands numbers at runtime and `JSON.stringify` throws on `bigint`, so the
  `api.ts` params are `number` and the card coerces on read (`String(id)` for display and the
  `AllowList` items, `Number(expires_at_ms)` for the countdown). Both work whether the value is a
  `number` or a `bigint`.
- **Digits guard** lives in `withId(kind, call)`: the wrapped `mutationFn` rejects before the RPC
  call, so the message rides `useAction`'s `toast.error` path like any server error. Users: `^\d+$`;
  chats: `^-?\d+$`. The owner-id form uses the same `telegramId` and toasts directly (it is not an
  `AllowList`).
- **Link block hides as a whole at 0** (not just the "expires in" text): an expired code is
  single-use and void, so showing its QR would be a trap. `useCountdown` is one 1 s interval
  keyed on `expires_at_ms`; `secondsLeft` is `ceil`, so a 10-minute link starts at `10:00`.
- **Link source**: `link.data ?? (state.type === 'PairingLink' ? state : null)` — the same shape
  as Signal's `LinkUrl` fallback, so a link minted from `/telegram link` or the agent tool shows
  too. `on('Paired', link.reset)` drops the local copy; the invalidate (from `CHANNEL_EVENTS`)
  moves `state` off `PairingLink`.
- **Token row**: `Unconfigured` (or no state yet) → "not set" + the password form; otherwise
  "set" + **Replace…** (ghost) which reveals the form and a Cancel, and the description says the
  new token applies at the next restart (§10.5 hot-start note). Success toast: "Token saved —
  starting the bot" when unconfigured, "Token saved — applies at the next restart" otherwise.
  Only `queryKeys.telegram` is invalidated by the card; `SecretsChanged` already refreshes
  `queryKeys.secrets` (`lib/rpc.ts`).
- **Pair disabled while unconfigured**: `telegram_link` needs a bot username for the deep link.
- `Connected` carries only `bot`, so its "topics on/off" suffix comes from `status.bot` and is
  omitted when `getMe` has not answered yet. `Unreachable` → `chat <id> unreachable: <reason>`;
  `RateLimited` mirrors Slack's phrase; `null` state → "unknown" (Slack's choice).
- **Mobile**: nothing phone-specific needed. `SettingsDialog` renders the same `Pane` in the
  bottom sheet; the card's inputs are `max-w-64` inside `Row`'s `min-w-0` column, the URL is
  `break-all`, the QR is 192 px as Signal's.
- **Setup notes** is a native `<details>` with the three §10.5 bullets.

### Gates (`crates/desktop/ui`, at `a8fc8c54`)

```
bun install (crates/desktop/ui, packages/apps)      619 + 484 packages installed
just ts-desktop                                      test result: ok. 8 passed; 0 failed (export_bindings)
bun run check                                        tsc -b --noEmit → exit 0
CI=1 bun run test                                    Test Files 139 passed (139) · Tests 785 passed (785) · 138.14s
bunx vitest run src/components/settings/Channels.test.ts src/rpc/api.test.ts   2 files, 8 tests passed
bun run build                                        exit 0 (rolldown chunk-size notes only)
```

### Not exercised

- No engine, no Telegram, no Tauri: the card was never rendered against a live
  `telegram_status`; the `set_secret` → hot start → `Identity` round trip is 98b-ENG's and was
  not run here. `useCountdown` ticking and the `Paired` reset are untested beyond typecheck
  (static markup runs no effects).
- The phone sheet was not viewed; the overflow claim rests on the shared `Row`/`AllowList`
  classes the Signal and Slack cards already use.

## Engine

### Files

| file | change |
|---|---|
| `crates/telegram/src/lib.rs` | `Telegram` gains `run_channel`, `token_hash: Option<u64>`, `source: TokenSource` (async token read), `start: Starter` (channel spawner); `Telegram::new(handle, cfg, rt, token, run_channel)` wires the vault and `channel::spawn`; `impl Message<secrets::SecretsEvent>` + internal `TokenRead(Option<String>)`; `Telegram::hot_start`; free fns `token_from(vault)`, `token_hash`, `register_sink(rt, handle)`; `Plugin::start` now `spawn_with_mailbox(.., subscriber_mailbox())` + `rt.subscribe::<Telegram, secrets::SecretsEvent>`; `Link` while unconfigured answers `telegram is not configured: set the telegram_bot secret (Settings › Channels, or t --telegram-setup)`; the boot warning says the channel starts when the secret is set; test `secrets_changed_hot_starts_the_channel` |
| `crates/telegram/src/channel.rs` | `// 98b` addition: `pub fn spawn_at(cfg, token, rt, host: Option<&str>)`; `spawn` delegates with `None`. The channel test `start_with_a_live_code_…` builds the actor via `Telegram::new` |
| `docs/design/98-as-built-SETTINGS.md` | this section |

### Flow

`SecretsChanged` → actor (daemon mode only; terminal mode returns at once, D4) → a spawned task
awaits `source()` (= `token_from(secrets::handle(&rt).await.as_ref())`) and `tell`s
`TokenRead(token)` back → `hot_start`: `handle.is_none()` → `(start)(cfg, token, rt)` =
`channel::spawn`, store the `Handle`, `configured = true`, `register_sink` (spawned
`lookup_when::<agent::UserChannels>` → `RegisterUserSink`), `info!("telegram: bot token set;
channel started")`; the channel's own identity task publishes `Identity` when `getMe` answers.
`handle.is_some()` → compare `token_hash(token)` to the stored hash; different → store it and
`info!("telegram: bot token changed; restart the engine to use it")` once per change (D3).
`TokenRead(None)` → `debug!`. `Requested` / `Resolved` are ignored.

### Judgement calls

- **Vault read off the mailbox, state change on it.** The brief offered "task sends
  `Configured(Handle)`" or an `Arc<Mutex<Option<Handle>>>`. Chosen: the task only reads the
  token and sends `TokenRead`; the actor itself calls `channel::spawn` (sync, returns at once).
  Two `SecretsChanged` in a row therefore cannot start two channels — the `is_none()` check and
  the spawn happen in one handler turn.
- **D3 implemented with a `u64` `DefaultHasher` of the token**, seeded at boot from the token read
  in `Plugin::start`; the token itself is never kept in the actor and never logged.
- **Two injected seams (`source`, `start`), not one.** `source` makes the test independent of a
  real vault (`secrets::handle` waits 10 s for the actor otherwise). `start` points the channel
  at a `wiremock` server so the test never dials `api.telegram.org` even with a fake token (the
  brief tolerated the offline dial; the rule "never api.telegram.org" won). This is the one
  `channel.rs` addition (`spawn_at`). Production values are set by `Telegram::new`; only the
  test module overwrites the fields.
- **`register_sink` is shared by boot and hot start** and runs as its own task, so the boot path
  no longer registers the sink after the tools sequentially; order never mattered
  (`lookup_when` waits for the user actor either way).
- **Terminal mode stays `DaemonOnly`/`Unconfigured` on `Changed`** (D4); `configured` is not
  flipped there, so `Status` keeps saying `Unconfigured` until restart. A terminal-mode engine
  cannot pair anyway (`Link` → "pairing needs the daemon").
- **Boot warning reworded**: `telegram: secret `telegram_bot` not set (Settings › Channels, or t
  --telegram-setup); the channel starts when it is` — it is no longer true that telegram "is not
  started" for good. Still `warn`.
- **`Status` unchanged**: `Some(h) => h.status()` covers the hot-started handle; the test asserts
  `state != Unconfigured` after the transition (the live snapshot's state is whatever the
  identity task has published — `Disconnected` against the 404-ing mock within ~ms).
- The design says "`Plugin::start` subscribes the actor" — done after `register`, before the
  `Unconfigured` publish, so a `set_secret` racing boot is never missed (an early `Changed`
  re-reads the vault; a token already in use is a hash match → no log).

### Gates (verbatim)

```
cargo check -p telegram --all-targets
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 12.10s          exit=0
cargo test -p telegram
    test result: ok. 113 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.62s
    test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out  (doc-tests)  exit=0
cargo clippy -p telegram --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 13.53s          exit=0
cargo fmt -p telegram -- --check   (CHECK only; cargo fmt was NOT run)
    (no output; after hand-applying rustfmt's 3 diffs, all in added lines)      exit=0
    first run: exit=1 — Starter alias, the subscribe line, the test's Telegram::new call
cargo check -p rpc
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 5m 33s          exit=0
```

### Not exercised

- No real token exists anywhere; no engine was run against Telegram. The transition was
  exercised by the unit test only (fake token `1:first`, `wiremock` host, `getMe` → 404 →
  backoff, runtime dropped). `Identity` after a real `getMe` is the pre-existing channel path.
- The desktop card's `set_secret` → `SecretsChanged` → hot start round trip end to end (98b-UI
  owns the card; `secrets` publishes `Changed` on every `set` already).

### NEEDS

- 98b-UI: the `Link` error wording above if the card shows it; otherwise none.
