# 42 — Push: wake a suspended phone when the harness has news

Status: design v0 · 2026-09-17 · extends `31-link.md` §4.2 (relay frames),
§5 (relay), §6 (link plugin), §8.2 (phone behaviour), §8.3 (iOS packaging),
§10 (threat model); `25-desktop.md` §6 (notifications). §12 lists the
amendments to 31.

## 0. Decisions in one screen

1. **APNs is the only wake-up, so the design is about who holds the key,
   not whether.** iOS suspends the process seconds after it leaves the
   screen and freezes its sockets; `beginBackgroundTask` buys ~30 s
   (already taken, `desktop/src/lib.rs` `mobile::background`), and every
   other background strategy Apple offers is either system-scheduled
   (`BGAppRefreshTask`, `BGProcessingTask`), transfer-only (`URLSession`),
   or a push ([Choosing Background Strategies](https://developer.apple.com/documentation/backgroundtasks/choosing-background-strategies-for-your-app),
   [Extending background execution time](https://developer.apple.com/documentation/uikit/extending-your-app-s-background-execution-time)).
   Rejected: VoIP/audio/location modes (App Review 2.5.4), a persistent
   socket (not a thing on iOS), Web Push (Safari home-screen web apps only,
   not WKWebView).
2. **The push carries nothing.** Title `wth`, one of three fixed bodies
   (*Response ready*, *Approval needed*, *Turn failed*), no session id, no
   title, no text. Tapping opens the app; the existing foreground path
   (`RpcClient.#foreground` → reconnect → `reload()`) lands the user on a
   live transcript with the finished chat at the top of the list. What a
   push observer learns beyond 31 §5.6: *this device has news, now*.
3. **The harness decides, the relay delivers, any relay can forward.** The
   harness's link plugin sends a new uplink frame `0x05 PUSH` with the
   device's APNs token and a kind byte. A relay configured with an APNs
   auth key sends to Apple; a relay without one forwards the same bytes to
   `--push-upstream` (default: the operator's public relay); a relay with
   neither drops it and counts. Apple only accepts pushes signed by the
   team that published the bundle id, so one published app ⇒ one upstream
   for every self-hosted relay — the shape Matrix (Sygnal), ntfy and
   Nextcloud use. A self-hoster who also publishes their own build sets
   their own key and needs nobody.
4. **The token rides the handshake, not an RPC.** `raw_json_request` runs
   every device under `ConnectionId(0)` and cannot tell devices apart
   (31 §6.2). The `Noise_IK` message-1 payload today is one mode byte; it
   gains an optional `kind:u8 · len:u8 · token` tail. It is encrypted to
   the harness static and bound to the authenticated device for free, and
   it is re-sent on every connect, so token rotation and unregistration
   (`kind = 0`) need no extra path. `devices.yaml` rows gain `push`.
5. **Push to every registered token, always; iOS de-duplicates.** A
   suspended socket can look open to the harness for up to 45 s (relay
   ping 30 s + pong 15 s), so "push only to offline devices" loses exactly
   the events this design exists for. iOS does not show a remote
   notification while its app is in the foreground (no
   `willPresent` delegate ⇒ silent), so a device that is actually live sees
   nothing twice. The local `notifyIfHidden` path retires on iOS once a
   token is registered; macOS and browser keep it.
6. **No Swift.** The iOS shell is `main.mm` + Rust. Registration is
   `UIApplication.registerForRemoteNotifications` from a Tauri command; the
   two delegate callbacks are added to tao's application-delegate class at
   runtime with `class_addMethod` (objc2, already a dependency), and the
   token reaches JS as a Tauri event. Rejected: a Swift plugin package, a
   Notification Service Extension (nothing to decrypt: §0.2).
7. **The relay grows one HTTP/2 client and one ES256 signer, nothing
   else.** `reqwest` (workspace, rustls, http2) and `p256`; JWT is three
   base64url parts, cached ≤ 50 min. No queue, no retry beyond one; APNs
   is at-most-once by contract and so is this. `POST /push` on an
   upstream is the forwarding endpoint, served only when a key is
   configured, rate-limited per IP and per token.

## 1. Why

31 §8.2 stops at the 30 s grace period and says "no push service: APNs via
the relay would need the relay to see something; it cannot." The second
half is wrong: the relay needs to see a *token* and a *kind*, both of which
it can carry without seeing the traffic. The use is the one 31 was built
for — send a message from the phone, pocket it, get told when the answer
or an approval is waiting. Today that works only if the answer arrives
within ~30 s.

The constraint that shapes everything is Apple's: pushes to the App Store
build of `dev.rwx.wth.ios` can only be signed with the publisher's key.
"Anyone can run a relay" therefore cannot mean "anyone can push" — it means
any relay can *forward* to the one that can, and the forwarded bytes must
be worthless to read (§0.2, §9).

## 2. Facts relied on (recon 2026-09-17)

### 2.1 Relay (`crates/relay`)

- Uplink frame `op:u8 · conn:u32 LE · payload`; ops `HELLO 0x00`, `OPEN
  0x01`, `DATA 0x02`, `CLOSE 0x03`, `PRESENCE 0x04` (`frame.rs:10-18`);
  `MAX_PAYLOAD 65_535`. The uplink read loop matches `DATA` and `CLOSE`
  and closes the uplink with a protocol error on any other op
  (`relay.rs:352-371`) — an old relay meeting a new harness would drop it,
  so §4 gates the frame on the relay's HELLO (§4.1).
- Per-harness state is `Uplink { tx, conns, next, gone }`
  (`relay.rs:105-110`); nothing about the harness is retained past `NK`.
- CLI is clap `Cli { listen, tls, web, max_conns, cookie_secret }`
  (`main.rs:38-57`), env only `WTH_RELAY_COOKIE_SECRET`. Routes `/h/{id}`,
  `/healthz`, `/metrics`, web fallback (`main.rs:110-116`). Per-IP token
  bucket exists (`relay.allow`, 10/s burst 30).
- Deps: tokio, axum(ws), axum-server(tls-rustls), rustls/aws-lc-rs, snow,
  dashmap. No HTTP client, no JWT. Workspace offers `reqwest 0.13`
  (rustls, http2), `http 1`. `deploy/relay/Dockerfile` builds `-p relay`
  musl-static from `scratch`; new deps must be pure Rust or musl-clean.
- k8s: 2 replicas behind haproxy with `load-balance: uri` consistent hash
  (`deploy/relay/k8s/rwx-dev/ingress.yaml`); cookie secret from a Secret.

### 2.2 Link plugin (`crates/link`)

- `devices.yaml` row: `{ id, pub, name, platform, paired_at_ms,
  last_seen_ms }` (`devices.rs:12-26`); `platform` is the device's
  `container()` (`ios | macos | web`) from `PairHello`.
- `IK` message-1 payload is the mode byte, `0` API / `1` ACP
  (`conn.rs:113-116, 279-289`); message 2 carries `SessionHello { name,
  version }`.
- Uplink writer: `frame(op, conn, payload) -> Message` (`uplink.rs:48-54`);
  the `out: mpsc::Sender<Message>` lives in `run_once` and the conn tasks
  only (`uplink.rs:154-162`, `conn.rs:198`). `State` (`state.rs:71-88`)
  has `connected: bool` but no writer handle.
- The plugin publishes `LinkEvent`s and subscribes to nothing. The
  serialized event stream every device receives is `rpc::bridge::start(rt)`'s
  `broadcast::Sender<Arc<serde_json::Value>>` (`bridge.rs:114-147`,
  capacity 1024); `rpc` already hands link `SetDispatch { methods, acp }`.
- Device online = some `ConnHandle.device == key` (`state.rs:100-102`).

### 2.3 Events

- `AgentEvent::TurnFinished { session, turn, stop_reason, usage, context }`
  (`agent/src/lib.rs:904-915`); a failed turn is `stop_reason == "error"`
  (as `slack/board.rs:125-127` and the TUI read it), there is no separate
  variant. `AgentEvent::Error { session, turn, message }` is non-fatal
  text. `SubagentSpawned { parent, child, name }`. `TurnFinished` carries
  no `parent`; the UI skips subagent turns by consulting its session
  store (`App.tsx:226-232`).
- `ApprovalRequested { id, session, tool, args, risk }`
  (`tools/src/approval.rs:35-49`).

### 2.4 iOS shell (`crates/desktop`)

- Plugins: opener, notification (both), barcode-scanner and deep-link
  (mobile) (`lib.rs:20-55`). Commands: `connection` only. `on_event` maps
  `Suspended`/`Resumed` to `background::hold/release`, implemented with
  `objc2 0.6`, `block2 0.6`, `objc2-ui-kit 0.3` (`UIApplication`,
  `UIResponder` features) (`lib.rs:88-131`, `Cargo.toml:58-60`). Mobile
  emits no Tauri events to JS today.
- `gen/apple/Sources/desktop/main.mm` is the only native source; it calls
  `ffi::start_app()`. The Xcode project is committed and regenerated from
  `project.yml` by xcodegen; `package-ios.sh` says do not `tauri ios init`
  over it.
- `desktop_iOS.entitlements`: `associated-domains` only. No
  `aps-environment`. `Info.plist`: no `UIBackgroundModes` (none needed for
  alert pushes). Bundle `dev.rwx.wth.ios`, team `8H548FVL95`, iOS 17.
- UI: `notify.ts` `notifyIfHidden` → `sendNotification` when the window is
  not focused/visible; `primeNotifications` asks permission at launch on
  iOS (`App.tsx:187`). `useEventSideEffects` notifies on
  `ApprovalRequested` and on top-level `TurnFinished` off the current
  route (`App.tsx:213-250`). `@tauri-apps/plugin-deep-link` JS is
  installed but unused.

## 3. Shape

```
  phone (iOS app)                relay A (self-hosted,           relay B (operator's,
                                 no key)                          --apns-key)                 Apple
  ──────────────                 ─────────────────────            ────────────────────        ─────
  registerForRemoteNotifications
   ⇠ token ────────────────────────────── IK msg1: mode · kind · len · token ──────▶ harness
                                                                                      devices.yaml push:
                                                                                      { kind, token, at_ms }
                                                                                    TurnFinished on bus
                                 ◀── uplink PUSH kind·token ────────────────────── link plugin
                                 no key → POST /push ──────────▶ verify, rate-limit
                                                                  JWT ES256 → POST /3/device/<hex> ─▶ APNs
   banner "wth · Response ready" ◀───────────────────────────────────────────────────────────────────
   tap → app foregrounds → RpcClient reconnects → reload()
```

Roles: the harness knows *when* (it has the events and the token); the
relay knows *how* (it has an Internet route and, on the upstream, the key);
Apple knows *whom* (the token). No party gains what another has.

## 4. Protocol

### 4.1 Relay layer additions (31 §4.2)

**Uplink**, harness → relay, `conn = 0`:

| op | payload | meaning |
|---|---|---|
| `0x05 PUSH` | `kind:u8 · len:u8 · token[len]` | deliver one notification of `kind` to `token`; fire and forget |

`kind`: `1` turn finished, `2` approval pending, `3` turn failed. `0` is
reserved (used in the handshake for "no token", never on the wire here).
`len` is `32` for APNs; the relay rejects other lengths for kind ≥ 1 with a
`push_bad` count, not a close. Anything else is ignored: an unknown kind is
dropped and counted so a newer harness never kills an older relay.

**Capability flag.** The relay's `HELLO` (message 1 of `NK`, `conn = 0`)
gains a one-byte plaintext prologue-free trailer after the Noise message:
`caps:u8`, bit 0 = *accepts PUSH*. Old relays send no trailer; the harness
treats absence as `caps = 0` and never emits `PUSH` (§2.1: an old relay
would otherwise close the uplink). The byte is not authenticated and does
not need to be: the worst a tampering relay can do is drop pushes, which it
can do anyway.

### 4.2 Channel layer additions (31 §4.3)

`IK` message-1 payload, device → harness:

```
mode:u8                                  as today (0 API, 1 ACP)
[ kind:u8 · len:u8 · token[len] ]        optional; kind 0 = "I have no token / unregister"
```

A one-byte payload is an old device. The harness writes the tail into the
device's `devices.yaml` row on every successful session handshake:
`kind ≥ 1` sets `push`, `kind = 0` clears it, absent leaves it. Only the
API mode (0) carries a token; ACP conns (32) send the bare byte.

### 4.3 Upstream forwarding

`POST https://<upstream>/push`, body = the `PUSH` payload bytes verbatim
(`kind · len · token`), `content-type: application/octet-stream`, no auth
header. Responses: `204` accepted, `400` malformed, `410` Apple said the
token is unregistered (`410 Unregistered` / `400 BadDeviceToken`), `429`
rate-limited, `503` no key here. The forwarding relay logs and counts the
status and tells the harness nothing: §0.7's at-most-once.

The endpoint exists only on a relay with `--apns-key`; a relay that itself
forwards returns `404` for `/push`, so two misconfigured relays cannot
bounce a token between them.

### 4.4 APNs request

```
POST https://api.push.apple.com/3/device/<hex token>     (api.sandbox.push.apple.com with --apns-sandbox)
authorization: bearer <JWT>          ES256, header {alg, kid}, claims {iss: team, iat}; reissued after 50 min
apns-topic: dev.rwx.wth.ios          the bundle id (--apns-topic)
apns-push-type: alert
apns-priority: 10
apns-expiration: now + 3600          unreachable for an hour → drop; a stale "response ready" is noise
apns-collapse-id: <kind>             three banners at most per app, the latest of each kind wins

{"aps":{"alert":{"title":"wth","body":"Response ready"},"sound":"default","thread-id":"wth"}}
```

Bodies by kind: `1` *Response ready*, `2` *Approval needed*, `3` *Turn
failed*. No `content-available`, no `mutable-content`, no custom keys.

## 5. Relay changes (`crates/relay`)

### 5.1 CLI

```
wth-relay … [--apns-key <AuthKey.p8> --apns-key-id <KID> --apns-team <TEAM> --apns-topic <bundle id> [--apns-sandbox]]
            [--push-upstream <https://host>]
```

Env: `WTH_RELAY_APNS_KEY` (path or the PEM itself), `WTH_RELAY_APNS_KEY_ID`,
`WTH_RELAY_APNS_TEAM`, `WTH_RELAY_APNS_TOPIC`, `WTH_RELAY_APNS_SANDBOX`,
`WTH_RELAY_PUSH_UPSTREAM`. `--apns-key` and `--push-upstream` are mutually
exclusive (clap `conflicts_with`). Defaults: neither. `deploy/relay/compose.yaml`
passes `WTH_RELAY_PUSH_UPSTREAM=${PUSH_UPSTREAM:-https://relay.wth.rwx.dev}`
so a self-hoster gets pushes by default and can blank it to opt out; the
k8s base adds the four APNs vars from a Secret `wth-relay/apns` (key file
mounted, read-only root FS as today).

### 5.2 Module `push.rs`

```rust
pub enum Push { Apns(Apns), Upstream { url: String, http: reqwest::Client }, Off }

pub struct Apns { http: reqwest::Client /* http2_prior_knowledge */, host: &'static str, topic: String,
                  team: String, kid: String, key: p256::ecdsa::SigningKey, jwt: Mutex<(Instant, String)> }

impl Push {
    /// Never blocks the uplink: spawns, one attempt, counts the outcome.
    pub fn send(&self, kind: u8, token: &[u8], metrics: &Metrics)
}
```

- `Apns::jwt()` returns the cached token when younger than 50 min, else
  signs `base64url(header) . base64url(claims)` with ECDSA P-256/SHA-256
  and caches. Apple rejects a new JWT more often than every 20 min and a
  JWT older than 60 min; 50 keeps clear of both.
- One `reqwest::Client` per process, HTTP/2, connection kept warm by
  reqwest's pool; Apple asks for long-lived connections and this is one.
- `send` spawns a task, awaits the response with a 10 s timeout, and bumps
  `push_sent`, `push_gone` (410/BadDeviceToken), `push_failed`. No retry:
  a retry that lands after the user opened the app is worse than a miss,
  and the next event pushes again.
- `Upstream::send` is the same with the body forwarded; `Off::send` counts
  `push_dropped`.

### 5.3 Wiring

- `Relay` gains `push: Push`. The uplink loop gains a `PUSH` arm: decode,
  validate `len`, `relay.push.send(kind, token)`. The HELLO writer appends
  the caps byte when `push` is not `Off`.
- Router gains `POST /push` when `push` is `Apns`: read ≤ 64 bytes, per-IP
  bucket (the existing one) **and** a per-token bucket (`DashMap<[u8;32],
  Bucket>`, 60/h, evicted when idle an hour) so a leaked token cannot be
  used to buzz a phone. Returns §4.3's codes.
- `/metrics` gains the five counters. No per-token rows, no logs of tokens
  (they are logged as the first 4 hex chars).

### 5.4 Deps

`reqwest` (workspace: `rustls-tls`, `http2`, no default features), `p256`
(`ecdsa`, `pem`), `base64` (already). `p256` is pure Rust and builds on
musl; `reqwest` with rustls is what the Dockerfile already proves out for
`axum-server`. Nothing links OpenSSL.

## 6. Link plugin changes (`crates/link`)

### 6.1 Data

```yaml
# <data_dir>/link/devices.yaml
- id: …, pub: …, name: iPad, platform: ios, paired_at_ms: …, last_seen_ms: …
  push: { kind: 1, token: <64 hex>, at_ms: … }      # optional; kind is the token family (1 = APNs)
```

`Row.push: Option<PushToken>`; `#[serde(default, skip_serializing_if =
"Option::is_none")]` so existing files parse and untouched rows do not grow.
`Device` (the UI shape) gains `push: bool` for the Settings › Link row.

### 6.2 Handshake

`conn.rs` `respond` → session branch: after the mode byte, parse the
optional tail; on `mark_online` write it to the row and `devices.save()`
only when it changed (one write per launch at most). Token family `1`
requires `len == 32`; anything else is treated as "no token" and logged
once per device.

### 6.3 Uplink writer handle and capability

`run_once` stores `out.clone()` and the relay's `caps` in `State { uplink:
Option<UplinkHandle { out, caps }> }` when it flips `connected = true`, and
clears it with `set_connected(false)`. `State::push(kind, token)` builds
`frame(OP_PUSH, 0, kind · len · token)` and `try_send`s it — a full uplink
queue drops the push (`OUT_QUEUE 4096`; a queue that full has bigger
problems), never blocks.

### 6.4 The push task

`SetDispatch` gains `events: broadcast::Sender<Arc<serde_json::Value>>`
(the `rpc` plugin already owns it; one more field in the message it sends).
The actor spawns one task on `SetDispatch`:

```rust
let mut rx = events.subscribe();
let mut children: HashSet<String> = HashSet::new();
loop {
    let ev = match rx.recv().await { Ok(v) => v, Err(Lagged(_)) => continue, Err(Closed) => return };
    let kind = match ev["type"].as_str() {
        Some("SubagentSpawned") => { children.insert(ev["child"].as_str()…); continue }
        Some("TurnFinished") if !children.contains(session(&ev)) =>
            if ev["stop_reason"] == "error" { 3 } else { 1 },
        Some("ApprovalRequested") => 2,
        _ => continue,
    };
    for row in state.devices.rows.iter().filter_map(|r| r.push.as_ref()) { state.push(kind, &row.token) }
}
```

- Same stream, same JSON the phone sees; `Lagged` is skipped (the phone
  reloads on its own `Lagged`; a lost push is a lost banner, not lost
  state).
- Top-level only, as the UI (`App.tsx:230`): subagents are `children`,
  learned from `SubagentSpawned`. `ponytail:` children spawned before the
  process started are not children — subagents live in-process, so there
  are none; if a persisted child session is ever resumed as a top-level
  turn it pushes, which is correct.
- `stop_reason == "cancelled"` is the user's own act and still pushes as
  kind 1 — they asked for the turn to end and it did; the phone that
  cancelled is in the foreground and shows nothing (§0.5).
- Every registered token, every time (§0.5). A device row with `push` and
  `platform != ios` cannot exist (only the iOS shell registers) and is
  ignored if hand-edited in.

### 6.5 Revoke and rotation

`Revoke` deletes the row and with it the token — the harness stops pushing
to a revoked phone at once; the phone's own tap-through then fails `IK`
with `CLOSE 3` as today. A token Apple reports gone stays in the row until
the phone's next launch overwrites it; the relay counts it, the harness
does not learn (§0.7). `link_status()` shows `push: true/false` per device.

### 6.6 RPC

No new method. `LinkStatus.devices[].push` is the only visible change;
`export.rs` regenerates `Device.ts`.

## 7. iOS shell changes (`crates/desktop`)

### 7.1 Registration

New Tauri command `register_push` (mobile only; desktop returns `Err`):

```rust
#[tauri::command]
fn register_push(app: AppHandle) -> Result<(), String> {
    let mtm = MainThreadMarker::new().ok_or("not main thread")?;   // run_on_main_thread otherwise
    push::install_delegate_methods(app.clone());                  // idempotent
    UIApplication::sharedApplication(mtm).registerForRemoteNotifications();
    Ok(())
}
```

`push::install_delegate_methods` takes `UIApplication.sharedApplication.delegate`'s
class and `class_addMethod`s two selectors with `extern "C"` implementations:

- `application:didRegisterForRemoteNotificationsWithDeviceToken:` → hex of
  the `NSData` → `app.emit("push-token", hex)`.
- `application:didFailToRegisterForRemoteNotificationsWithError:` →
  `app.emit("push-token", "")` and `tracing::warn!` the description.

`[INFERENCE: tao's iOS delegate is an ordinary ObjC class created with
objc2's declare machinery, so `class_addMethod` after launch is accepted and
UIKit's `respondsToSelector:` check at callback time finds the new IMPs.
Phase 1 (§10) verifies this on a device before anything else is built; the
fallback is a category in `main.mm` on the delegate class by name.]`

`registerForRemoteNotifications` after `UNUserNotificationCenter`
permission is granted (the JS already asks at launch); calling it before
still yields a token but Apple then may not display alerts, so the order
is: `requestPermission()` → `invoke('register_push')`. The `AppHandle` is
kept in a `OnceLock` for the IMPs.

### 7.2 Packaging

- `desktop_iOS.entitlements`: add `aps-environment = production` (Xcode
  swaps to `development` for debug installs on device; TestFlight and App
  Store are production). One-time portal step: enable *Push Notifications*
  on the `dev.rwx.wth.ios` App ID and create an *Apple Push Notifications
  service (APNs)* key — the `.p8`, its Key ID, and the Team ID are §5.1's
  inputs. The key is not in the repo; `.env.signing.example` documents the
  three names.
- No `UIBackgroundModes`, no Info.plist change: alert pushes need
  neither. `capabilities/mobile.json` gains nothing (`core:event:default`
  already allows the listen).
- `package-ios.sh` unchanged; `project.yml` unchanged (the entitlements
  file is referenced by path).

## 8. Device UI changes (`crates/desktop/ui`)

### 8.1 Token lifecycle (`src/lib/push.ts`)

```ts
/** APNs token as hex, or '' when iOS refused; undefined until the shell answers. Persisted (IDB, beside the identity). */
export const usePush = create<{ token?: string }>()

export async function primePush(): Promise<void> {
  if (container() !== 'ios') return
  await listen<string>('push-token', (e) => usePush.setState({ token: e.payload }))   // before invoke: the callback can be immediate
  if (await isPermissionGranted()) await invoke('register_push')
}
```

Called from `App.tsx` right after `primeNotifications()` resolves (which
is the permission prompt). iOS answers `didRegister…` on every launch and
whenever the token changes; the token is persisted so the first `IK` after
launch already carries it (§8.2), and a changed token is carried by the
next reconnect — on iOS that is the next foreground.

### 8.2 Handshake payload (`src/link/transport.ts`, `noise.ts`)

The `IK` message-1 payload builder takes `Uint8Array.of(mode, kind, len,
…token)`: `kind = 1, len = 32` with a token; `kind = 0, len = 0` when the
persisted token is `''` (refused, or notifications turned off in
Settings — §8.4); bare `mode` on macOS/web. `pairing.ts` (`IKpsk2`) is
unchanged: pairing has no session yet; the token arrives on the first
session connect that follows.

### 8.3 Local notifications retire on iOS

`notifyIfHidden` returns early when `container() === 'ios' &&
usePush.getState().token` — the push covers backgrounded *and* suspended,
and firing both would show two banners during the grace window. When the
token is `''` or undefined the local path stays as the fallback it is now.
`primeNotifications` unchanged.

### 8.4 Settings

Settings › General (iOS only): a *Notifications* row with a switch bound to
the token: off → `usePush.setState({ token: '' })` and the next handshake
unregisters (§4.2), on → `register_push` again. Settings › Link's
disclosure sentence (31 §5.6) gains: *"When a turn finishes or an approval
is waiting, the relay is told to wake this device. It learns that, and
when — not what."* Per-device rows show a bell when `push` is set.

### 8.5 Tap

No payload, no handler: the OS foregrounds the app, `visibilitychange`
runs `RpcClient.#foreground`, the router stays mounted (`Shell`, this
series), `reload()` refetches, the sessions list orders by activity. One
more tap reaches the chat or the approval. §13 open 1 is the deep link.

## 9. Threat model (additions to 31 §10)

| threat | answer |
|---|---|
| Upstream operator reads pushes | there is nothing to read: fixed strings, a token that identifies an install only to Apple, a kind byte. Learns: token, kind, time, forwarding relay's IP |
| Upstream operator pushes at will | can only send the three fixed banners to tokens it has seen; the same power the local relay and Apple have. Annoyance, not disclosure; §8.4 turns it off, revoking the device removes the token from the harness |
| Leaked token used to buzz a phone | `/push` per-token bucket 60/h and per-IP bucket; APNs itself enforces per-app throttles; the phone re-registers a fresh token on reinstall |
| Forwarding loop between relays | `/push` is served only where a key is; a forwarding relay 404s it |
| Harness sends pushes for a revoked device | the token dies with the row (§6.5) |
| Correlating pushes to harness activity | already possible from uplink frame timing at the relay (31 §5.6); the upstream additionally learns "some harness behind relay A finished a turn", with no harness id (the `PUSH` frame is `conn 0` and `/push` carries no id) |
| Tampered `caps` byte | can suppress pushes, which the relay can do by dropping frames anyway; no authentication needed |
| APNs key theft from the upstream | the key can push to every install of the app: rotate in the portal (new `.p8`), redeploy; the key is a Secret with the same handling as the cookie secret, plus read-only mount |
| Sandbox/production mismatch | a debug install has a sandbox token; forwarded to a production upstream it is `BadDeviceToken` and counted; developers run `--apns-sandbox` locally |

## 10. Phases and checks

Each ends with a runnable check, not a test suite.

1. **Delegate spike.** `register_push` + `install_delegate_methods` in the
   iOS shell, `aps-environment = development`, a dev build on a device;
   check: `push-token` event with 64 hex chars in the Web Inspector console
   within 2 s of granting permission; a second call does not crash
   (idempotent `class_addMethod`). If the IMPs are not called, switch to
   the `main.mm` category fallback before continuing.
2. **Relay sender.** `push.rs`, CLI, `/push`, metrics; check: `curl -X POST
   --data-binary @<(printf '\x01\x20'; xxd -r -p token.hex) https://localhost:8787/push`
   against `--apns-sandbox` shows *Response ready* on the device; `/metrics`
   `push_sent 1`; a second relay with `--push-upstream` gets `204` end to
   end; a bad length gets `400`.
3. **Harness.** `Row.push`, handshake tail, `UplinkHandle`, push task,
   `caps`; check: pair the phone, `wth` prints the row with `push:`, then
   from the phone send a prompt that takes > 90 s, lock the phone; the
   banner arrives when the turn ends; tap → live transcript. Approve/deny
   path: a `mutating` tool → *Approval needed*. Against an old relay
   (no caps byte) the harness logs `push: relay does not forward` once and
   sends nothing.
4. **UI.** `push.ts`, handshake payload, local-notification gate,
   Settings row and copy; check: with the switch off the row's `push`
   clears on the next connect and no banner arrives; on macOS nothing
   changed (no `register_push`, local notifications as before).
5. **Deploy and docs.** compose/k8s env and Secret, `.env.signing.example`,
   README relay section, 31 amendments (§12); check: `docker compose up`
   with a blank `PUSH_UPSTREAM` runs and counts `push_dropped`.

## 11. Rejected

- **Push only to devices without a live conn.** Loses the 45 s window in
  which a suspended phone still looks online — the case that matters.
  iOS's foreground suppression makes "always" free of duplicates (§0.5).
- **Session id or title in the payload.** Turns a content-free wake into
  metadata the upstream and Apple keep; a UUID alone is meaningless but
  titles are not, and the line is easier to hold at zero. §13 open 1 is
  the encrypted variant if one tap too many hurts.
- **A `register_push` RPC method.** The dispatcher cannot attribute a call
  to a device (31 §6.2, `ConnectionId(0)`); the handshake payload is
  attributed by construction and re-sent on every connect.
- **The link actor subscribing to the bus.** Needs a subscriber mailbox and
  a typed event per trigger; the bridge broadcast already carries the
  serialized events with the shape the UI matches on, and link already
  depends on `rpc` for `SetDispatch`.
- **Notification Service Extension.** Only useful to decrypt or fetch
  content; there is none by design.
- **Swift plugin for registration.** A second toolchain and a Swift
  package for two selectors; objc2 is already linked and used for the
  background task.
- **Silent (`content-available`) pushes to reconnect in the background.**
  Rate-limited to ~3/h by Apple and "the system decides when to launch";
  and the UI would still need to notify — the alert push already is the
  notification.
- **A queue/retry in the relay.** APNs is at-most-once; the next event is
  the retry.
- **Web Push for the browser container.** Different key model (VAPID),
  different platform rules (Safari: home-screen web apps only), no shared
  code with the above; not asked for.

## 12. Amendments to 31 (same commit series)

- §4.2 uplink op table: `0x05 PUSH` (§4.1); HELLO `caps` trailer.
- §4.3: `IK` message-1 payload layout (§4.2).
- §5.2 endpoints: `POST /push`. §5.3 CLI: the APNs and upstream flags.
  §5.6 what the relay learns: *"…and, when a device asked to be woken,
  that a harness behind it had news for that device."*
- §6.3 `devices.yaml`: `push`. §6.2 `SetDispatch.events`.
- §8.2: replace *"No push service: APNs via the relay would need the relay
  to see something; it cannot…"* with a pointer here.
- §8.3: `aps-environment`, the portal capability, the APNs key names.
- §10: the §9 rows.

## 13. Open

| # | question | default |
|---|---|---|
| 1 | Encrypted payload (session id, title) sealed to the device static so a tap deep-links and the banner could show the title via an NSE | not in v0; revisit if the extra tap is complained about; the harness static ↔ device static box is 20 lines each side, the NSE is the cost |
| 2 | `AskRequested` / `SecretRequested` as kinds 4/5 | not in v0; they are approvals in spirit and the same code path when wanted |
| 3 | Per-harness distinguishability on a phone with several harnesses (`thread-id`, title) | fixed `wth`; the harness *name* is the only candidate and is plaintext to the upstream |
| 4 | Android (FCM) | the frame is `kind · len · token`; a `kind` family byte 2 = FCM is a relay-side sender and an Android shell registration; nothing here is iOS-specific but §7 |
| 5 | Should the operator's public relay require anything to forward for it | no; the token is the capability, the buckets bound abuse; revisit on metrics |
