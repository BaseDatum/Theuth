# 70 as built — LINK-MSG: the relay link client names its failure

Branch `fix/link-client-failure-messages` off `90c8cc32`. Brief: `/tmp/69/omp-LINK-MSG.md`. Wire protocol unchanged (31 §4.2, 69 decision 6); relay and engine crates untouched.

## Why

2026-09-27: an iPad with a pre-2.0 pairing could not connect; the client showed "Reconnecting…" while the relay counted `close_reason_3 36` (REFUSED). Everything the relay tells a device — `PRESENCE 0` (parked, no uplink), `CLOSE 1–5`, an upgrade that never opens — collapsed to presence `offline`/`connecting`.

## What changed (`crates/desktop/ui/src`)

- **`lib/link-status.ts`** (new, pure) — `LinkEvent` (`connecting | presence | close | lost | ready`) → `reduce(state, event)` → `LinkState { phase, parkedAt, sticky, lastClose, attempts, retryAt }`; `failure(state, now, engine)` → `LinkFailure { kind, title, detail, action? }` or `null`; `diagnostics(state, now, engine)` → text. `PARKED_AFTER_MS = 10_000`. `retryAt = lostAt + backoff(attempts − 1)` mirrors `RpcClient`'s redial (`rpc/client.ts` now exports `backoff(attempt)` and uses it itself).
- **`link/transport.ts`** — `LinkTransport` folds one event at each existing hook (`#open`, PRESENCE, CLOSE, `onclose`, handshake done) and mirrors the state into `useEngines.link[id]` (`link/engines.ts`: `link`, `setLink`, dropped on `remove`). Presence handling is unchanged; `refused` presence now coincides with `failure.kind === 'revoked'`.
- **`components/link/LinkFailure.tsx`** (new) — `useLinkFailure(id)` (1 s tick while not connected, so the parked/redial counters move), `FailureText` (full and one-line `compact` forms, Re-pair button when `action === 'repair'`), `LinkBanner` (strip under the chat header for the active link engine).
- Surfaces: `launch/StartupScreen.tsx` `LinkStartup` renders the failure (else "Connecting to <name>…") plus **Copy diagnostics**; `routes/c.$session.tsx` and `chat/NewChatPane.tsx` mount `LinkBanner`; `link/EngineSwitcher.tsx` engine rows trail the failure title (`Trail`, tooltip = title + detail); `settings/Mesh.tsx` `Reachable` rows show `relay · paired` and the failure with Re-pair (`EngineLine` replaces `MeshLine`).
- **Re-pair**: `routes/link.tsx` takes `?repair=<id>`; `PairScreen` gets `replace?: Engine` — heading "Re-pair <name>", the new row takes the old entry's `name` and `order`, the old id is removed when it differs, the "add as node" choice is skipped. `LinkStartup` (outside the router) renders `<PairScreen replace>` inline.
- **Diagnostics**: `settings/General.tsx` `LinkDiagnostics` — a "Connection" section for the active link engine: relay URL, engine id prefix, phase + failure kind, last close reason + ISO timestamp, time parked, attempts, in a `pre` with `whitespace-pre-wrap break-all` (no horizontal overflow at 390 px), plus **Copy diagnostics**. The same text is behind the startup overlay's button, since an entry that never connects cannot reach Settings.

## Mapping and the heuristic

| observed | kind | title |
|---|---|---|
| `PRESENCE 0`, ≥ 10 s | `engine-offline` | `<name> is not connected to the relay` · detail `Waiting… (Ns). If it is running, this pairing may be stale — re-pair this device.` · Re-pair |
| `CLOSE 3`, `PRESENCE 0` seen on this socket | `stale-pairing` | `This pairing no longer matches an engine on the relay` · Re-pair |
| `CLOSE 3`, no `PRESENCE 0` first | `engine-full` | `<name> is serving its maximum number of devices` |
| `CLOSE 3` while our IK handshake is out | `revoked` (pre-existing `refused` presence) | `<name> refused this device` · Re-pair |
| `CLOSE 4` / `5` / `1` / `2` | `engine-gone` / `hub-moved` / `queue-full` / `idle` | … with `Retrying in Ns.` |
| socket closed before `open`, page served by this relay | `rate-limited` | `The relay is rate-limiting this network` · `Retrying in Ns.` |
| socket closed before `open`, otherwise | `unreachable` | `Cannot reach <relay host> from this network` · `A proxy may be blocking it.` |

- A `CLOSE` failure is *sticky* across the redial until a handshake finishes; a parked socket ≥ 10 s overrides it with `engine-offline` **except** `stale-pairing` and `revoked`, which need the user and win.
- **Reason 3 is one byte for three causes**; the socket's history separates them (`closeKind` in `link-status.ts`, marked `ponytail:`). Known miss: an engine that registers *after* we parked and is then full also reads as stale. Accepted per the brief.
- The current client never sends DATA while parked (handshake only after `PRESENCE 1`), so the relay's REFUSED-while-parked path fires for older clients or foreign traffic; a stale id on today's client presents as `engine-offline` — hence the Re-pair action and stale hint on that state too.
- **429 vs unreachable**: the browser hides the upgrade's HTTP status and the relay's WS close code is generic (1000/1001/1002). In web mode (`location.host === relayHost`) the relay just served the page, so a refused upgrade is its rate limit (or a stale cookie, which `#open`'s refetch cures on the next try) → `rate-limited`. Cross-origin (iOS/macOS shells) every never-opened socket is `unreachable`. No probe request added (`ponytail:` in `transport.ts`).

## Tests

- `lib/link-status.test.ts` — every kind; the 10 s threshold (9.999 s → null, then `Waiting… (13s)`); PRESENCE-then-REFUSED vs REFUSED-alone vs REFUSED-with-handshake; sticky vs parked precedence; `CLOSE 0` names nothing; backoff countdown (`1s`, then `2s` on the third socket); `diagnostics` lines.
- `link/transport-status.test.ts` — the real `LinkTransport` over a scripted fake socket with fake timers: `PRESENCE 0` → `CLOSE 3` → `stale-pairing`; `CLOSE 3` alone → `engine-full`; never-opened → `unreachable` with `retryAt`.
- `components/link/LinkFailure.test.ts` — `FailureText` full/compact through `renderToStaticMarkup` (vitest runs in node without a DOM harness; no testing-library in the repo).
- Gates: `bunx tsc --noEmit` 0; `CI=1 bunx vitest run` green including `no-emoji`; `cargo check --workspace --tests` clean (see report).

## Manual check

`just relay` on `ws://127.0.0.1:8787`, driven by the real `LinkTransport` over Node's WebSocket (throwaway vitest file, deleted): with nothing registered the socket parked at once; at 10.5 s `failure()` returned `engine-offline` / `Waiting… (11s)`; a DATA frame sent on that parked socket made the relay answer `CLOSE 3` + close (relay `/metrics`: `close_reason_3 2`) and the client reported `stale-pairing` with Re-pair; diagnostics text printed as designed. `ws://127.0.0.1:1` → `unreachable`. Not exercised: `t link pair` against the local relay (needs the full engine; the relay's parked-DATA path does not depend on what else is registered), the pixel rendering of the overlay/banner/Settings section (no web build in this run; layout uses existing `Row`/`Section`/strip classes and `break-all`), and a real 429 (needs > 30 upgrades/s from one IP).
