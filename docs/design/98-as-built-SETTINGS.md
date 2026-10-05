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
