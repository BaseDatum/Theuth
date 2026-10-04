# 95 LINK-WATCHDOG — as-built (2026-10-04)

Branch `feat/95-link-watchdog` off main `1336a0eb`. TypeScript only (`crates/desktop/ui`); no Rust, no relay change.

## What changed

### `src/link/transport.ts`

- Exported constants: `CONNECT_MS = 15_000` (socket creation → first PRESENCE), `HANDSHAKE_MS = 10_000`
  (PRESENCE 1 → ready), `PARKED_RECYCLE_MS = 60_000` (parked socket swapped in place).
- One `#watch` timer per transport, `#arm(ws, ms)` / `#disarm()`. Armed with `CONNECT_MS` at the end of
  `#open()`; re-armed with `HANDSHAKE_MS` on PRESENCE 1; replaced by the `#recycle()` timer on PRESENCE 0;
  disarmed in `#lost`, `close()` and on `ready`.
- Watchdog expiry = `#lost(ws, this.#opened)` + `ws.close(1000)` (the `reset()` shape): `#pending` rejected
  → `RpcClient` redials with backoff; `lost` event with `retryAt`; `reconnecting` reported.
- `#opened` is an instance field (set in `onopen`, reset in `#open()`); `reset()` uses it instead of `true`,
  so a never-opened socket reads as `opened: false` (sticky `unreachable` until the next `ready`).
- `#recycle()` (parked): `#ws = null; ws.close(1000); #open(true)` with `#pending` kept. `#open(recycle)`
  skips the `connecting` state/presence/event when `recycle` is true but still arms `CONNECT_MS`.
- `#report(s)` wraps `#state(s)` and remembers `#last`; the PRESENCE 0 branch reports `reconnecting` only
  when `#last !== 'reconnecting'`, so the fresh socket's repeated PRESENCE 0 is silent. `#lost`,
  `connecting`, `open`, `closed` are not deduped.
- `connect()` and `#recycle()` call `#open().catch((e) => this.#abort(e))`; `#abort` acts only when
  `#ws === null` (the constructor threw before a socket existed): disarm, drop, `lost` event
  (`opened: false, reachable: false`), presence offline, `reconnecting`, reject `#pending`.
- Post-await guards `if (this.#ws !== ws) return` after `getIdentity`, `handshakeIK`, `decrypt` and
  `hs.finish` in `#frame`.
- Header comment names the three timers and the silent recycle.

### `src/rpc/client.ts`

`#foreground` resets and redials at once in `connecting` as well as `reconnecting` (comment updated).
`probe()` unchanged (`open`-only).

### Tests

- `src/link/transport-status.test.ts`: `Scripted` counts `send()`; the `connect()` helper takes the socket
  class, passes a generated identity, records `onState` reports and returns the first `connect()` promise.
  New: (a)+(e) CONNECT_MS abandon then a redial opens a new socket; (b) HANDSHAKE_MS abandon after msg1;
  (c) silent parked recycle (`parkedAt`/`attempts` unchanged, no `onState` call across the recycle and the
  fresh PRESENCE 0, handshake starts on the recycled socket, the original `connect()` promise is still the
  pending one); (d) throwing WebSocket constructor rejects `connect()`, reports `reconnecting`, next
  `connect()` builds a new socket. The handshake uses real WebCrypto (threadpool), so msg1 is awaited with
  `vi.waitFor(…, { interval: 0 })`, which polls real event-loop turns without moving fake time.
- `src/rpc/client.test.ts`: "foreground while connecting resets and redials at once".
- `src/link/pairing.test.ts`: S5 test "a handshake that finishes after reset() does not resurrect the
  forgotten socket": the first socket's `onmessage` queues `t.reset()` one microtask behind msg2, so the reset
  lands while `#frame` is parked on `hs.finish`; the test then connects again and proves the new session with
  an `info` round trip. Verified to fail with the `hs.finish` guard removed (the stale continuation resolves
  the second `connect()` early and the round trip never answers).

### `docs/design/31-link.md`

Appended §7.8 "Device-socket liveness (95)" (the brief said §7.7, which already exists: "Capability gating").

## State table

| state | before | now |
|---|---|---|
| S1 parked zombie (relay dropped the parked socket, no close frame) | `#pending` never settles; chip "reconnecting…" forever; only relaunch helps | `PARKED_RECYCLE_MS` swaps the socket silently every 60 s; the fresh socket's PRESENCE is the poll; a missed PRESENCE 1 is caught within a minute. Foreground also `reset()`s (unchanged) |
| S2 socket fires neither `onopen` nor `onclose` | waits forever in `connecting` | `CONNECT_MS` (15 s) abandons it → `reconnecting`, backoff redial; foreground in `connecting` replaces it at once |
| S3 PRESENCE 1, msg1 out, msg2 never comes | waits forever in `connecting` | `HANDSHAKE_MS` (10 s) abandons it; foreground in `connecting` replaces it at once |
| S4 `new WebSocket` throws inside `#open()` | `#pending` set, `#ws` null forever; `reset()` no-op | `#abort(e)` rejects `connect()`, reports `reconnecting`; the next `connect()` starts a new attempt |
| S5 handshake finishes after `#lost`/`reset()` forgot the socket | `#session` set for a null socket; chip online, every call hangs | post-await guards drop the continuation |

## Gates

Bindings: `just ts-desktop` (log `/tmp/mc-2026-10-04/wd-ts.log`, last lines):

```
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 36 filtered out; finished in 3.44s

rm -rf crates/desktop/ui/src/rpc/bindings
mkdir -p crates/desktop/ui/src/rpc/bindings
cp -R bindings/. crates/desktop/ui/src/rpc/bindings/
```

(The `EXIT=0` echo never reached the log — the recipe's last step ran and `src/rpc/bindings/` holds 464
files; `bun run check` passing is the proof the bindings are complete.)

`cd crates/desktop/ui && bun run check`:

```
$ tsc -b --noEmit
CHECK_EXIT=0
```

`CI=1 bun run test < /dev/null` (first run, no re-run needed):

```
 Test Files  131 passed (131)
      Tests  738 passed (738)
   Start at  17:11:05
   Duration  38.33s (import 56%, transform 42%, tests 1%)
```

Touched files alone (`src/link/transport-status.test.ts src/rpc/client.test.ts src/link/pairing.test.ts`):
20 tests passed.

## Deviations

- `docs/design/31-link.md`: subsection numbered §7.8, not §7.7 (§7.7 "Capability gating" already exists).
- S5 test added to `pairing.test.ts` (the brief made it optional).
- `#watch: ReturnType<typeof setTimeout>` kept as specified (matches `RpcClient.#timer`), although a repo lint
  rule prefers named types for published contracts; this is a private field.
