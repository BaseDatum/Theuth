# 31 — Link: drive a harness from a phone through an end-to-end encrypted relay

Status: design v0 · 2026-09-15 · extends `00-architecture.md` §7 (RPC), §10
(workspace), §12; `25-desktop.md` §0.4 (the webview owns the socket — kept),
§3 (parity table), §7 (RPC additions: reverses "no bytes upload"); `18-app-launch.md`
(untouched: the local daemon still starts that way). Where 25 and this doc
disagree on the desktop client, this doc wins; 25 §5 packaging stays for
macOS. Recon (§2) is from the tree as of 2026-09-15 and from
`../jarvis/crates/jarvis-bridge` + `../jarvis/docs/design/remote-ui-bridge.md`,
which this design replaces in spirit (§1.1).

Vocabulary. **Harness**: a running `theuth` (terminal or headless). **Device**: a
client that drives it — iPhone/iPad app, the desktop app, or a browser tab.
**Relay**: the `theuth-relay` server both dial into. **Link**: the feature — the
harness-side plugin, the device-side transport, and the pairing flow.

## 0. Decisions in one screen

1. **The relay is a dumb pipe with no state and no accounts.** It maps a
   harness id in the URL to one harness WebSocket and copies opaque binary
   frames between that socket and each device socket. No store, no Redis, no
   tenants, no tokens, no pairing records, no config beyond `--listen`.
   Horizontal scale is the load balancer consistent-hashing on the URL path
   (§5.5). Rejected: jarvis's Redis-backed multi-pod hub with cross-pod
   pub/sub relay (§1.1) — the state it kept exists only because it also did
   auth; end-to-end encryption removes the reason.
2. **End-to-end encryption is Noise, and the pairing QR is the trust root.**
   Every harness and every device has one X25519 static key. The QR (or the
   pasted key) carries the harness's public key plus a 32-byte one-time
   secret; pairing is `Noise_IKpsk2` (the secret is the PSK: whoever scanned
   the QR is the device, no owner-confirm click needed); every later session
   is `Noise_IK` (the device knows the harness static, sends its own
   encrypted: 1 RTT, mutual auth, forward secrecy — WireGuard's pattern).
   Cipher suite `25519_AESGCM_SHA256` so the device side is **pure WebCrypto**
   (§7.2) — no crypto dependency in the UI. The relay sees ciphertext, frame
   sizes, timing, IPs and the harness id. It never learns which device it is
   talking to (`IK` sends the initiator's static encrypted).
3. **The phone speaks the existing JSON-RPC, byte for byte.** Inside the
   encrypted channel the device sends the same `{"jsonrpc":"2.0",…}` text the
   desktop sends today and receives the same `subscribe_events`
   notifications. The harness-side link plugin dispatches each decrypted
   request with `RpcModule::raw_json_request` (jsonrpsee 0.26,
   `rpc_module.rs:351`) — in-process, no loopback socket, no token, no second
   serialization of `TextDelta`. Parity is free: a command exists on the phone
   the moment it is an `Api` method (00 §8).
4. **One app, three containers.** `crates/desktop/ui` is the UI for the macOS
   menubar app (25), the iOS app (Tauri mobile, §8.3) and a browser build the
   relay can serve (§7.6). Container differences are capability checks
   (`isTauri()`, `platform()`), never forks. The desktop app itself can be a
   link device (drive the home Mac from the laptop).
5. **Multi-harness on the device, multi-device on the harness, both first
   class.** A device keeps one relay WebSocket per paired harness (presence
   for all, instant switch, no re-handshake); only the active harness holds an
   event subscription. UI state is per harness (§7.4) — never two harnesses
   in one store. The harness keeps `link/devices.yaml`; any UI (TUI `/link`,
   Settings › Link) lists, renames and revokes.
6. **One pairing string for every path.** `https://<relay>/link#1.<payload>`:
   the iOS app scans it (in-app camera) or the system camera opens it
   (universal link); a phone without the app lands on the relay-served web
   build and pairs there; a laptop pastes it. The payload is in the URL
   *fragment*, which browsers never send to the server — the relay serving the
   page still learns nothing.
7. **The relay is measured, not described as fast** (§11): 5-byte frame
   header, `Bytes` refcount hand-off from read to write, bounded per-connection
   queues, no JSON, no allocation per frame beyond tungstenite's, no locks on
   the copy path. Budget: p99 added latency < 1 ms at 10 k connections ×
   100 msg/s on one core; ≤ 16 KiB RSS per idle connection.
8. **Browser container: nothing in `localStorage`.** The device key lives in
   IndexedDB as PKCS#8 bytes and is imported as a non-extractable WebCrypto
   `CryptoKey` on load (usable by the origin only; see §7.2 for why not a
   serialized `CryptoKey`); harness list beside it. The relay sets an `HttpOnly; Secure;
   SameSite=Strict; __Host-` cookie on first load and requires it on browser-
   origin WebSocket upgrades — a per-browser session handle the relay can rate
   limit and revoke, not a credential (§7.6 says exactly what it is and is
   not).
9. **Mobile is accommodated, desktop is not degraded** (§8): safe-area insets,
   `100dvh`, 44 px targets, sheets go full-width under `sm`, the settings
   dialog becomes a pushed list, knowledge tree and document become two
   routes, hover-only affordances become visible under `pointer-coarse`,
   Enter inserts a newline on touch keyboards. Every rule is a responsive
   class on an existing component; no mobile component tree.
10. **Everything the desktop can do, the phone can do, except what is
    physically on the harness host** (§9): OAuth logins open a browser *on
    the harness*, System Settings deep links, quitting the daemon. Those show
    "on the harness" copy instead of a broken button. Attachments move to two
    RPC methods (`put_attachment`, `get_attachment`) used by every container;
    the Tauri `save_attachment` command is deleted.

## 1. Why

Signal (09) and Slack (21) reach the harness from a phone as chat channels:
text in, text out, approvals as reactions. They are not the console — no
sessions list, no agent tree, no knowledge, no settings, no streaming
transcript. The desktop (25) is the console, and it only reaches
`127.0.0.1`. The user wants the console in their pocket, against any of their
harnesses, without opening a port on a home network and without trusting
whoever runs the relay.

### 1.1 What jarvis built and what this drops

jarvis's `remote-ui-bridge.md` (1310 lines) and `crates/jarvis-bridge` are the
prior art. Verified against the code, not the doc (the doc promised device
keys and passkeys; the code shipped bearer tokens):

| jarvis | here | why |
|---|---|---|
| Bearer tokens (tenant/device/tunnel), sha256 at rest in Redis (`auth.rs:25-36`); sessions not invalidated on device revoke (`auth.rs:171-181`) | X25519 statics; revoke = delete the row, the next `IK` handshake fails; live channels closed | keys are unforgeable, nothing to leak from a store |
| Relay reads every frame; "honest relay disclosure" in the UI | relay sees ciphertext only | E2E was "research/follow-on" there; here it is the design |
| Redis: tenants, devices, nodes, presence TTL 45 s, owner leases, pairing codes, sessions, pub/sub relay channels (`redis_store.rs`) | none | the relay has nothing to remember |
| Cross-pod attach: subscribe-before-publish, `ready` handshake, 10 s timeout, lossy pub/sub reconnect (`proxy.rs:1-16`, `redis_store.rs:94-100`) | LB hashes on the URL path; every socket for one harness lands on one process | correctness by construction, not by protocol |
| Text frames double-encoded as JSON strings inside `tunnel.frame`; binary base64 through Redis (`registry.rs:312-322`, `redis_store.rs:731`) | binary frames, 5-byte header, payload untouched | one copy fewer, no parse |
| Per-attach loopback WebSocket to the local API with `session.hello` injected, substring-matched (`bridge_uplink.rs:783-868`) | `raw_json_request` in-process | no socket, no token, no string matching |
| Owner clicks Allow after the new device polls `hub.pair_status` at 1.5 s (`bridgePairing.ts:93-105`) | the QR secret is the PSK; scanning is the confirmation | the click proved the same thing the secret proves |
| QR is display-only; no scanner, no deep link (`DevicesPanel.tsx:455`) | in-app scan, universal link, web landing, paste — one string | |
| One attach per socket; switching brains = full reconnect (`connection.ts:321-326`) | one socket per paired harness, kept open | switch is a store swap |
| Passkeys, tenants, community account, recovery kit (design §4) | nothing; a harness is an identity | the console is single-user (15 is shelved); accounts are a product we do not have |

Kept from jarvis, because it was right: daemon dials out (no inbound port);
the data plane is the product protocol, unchanged; approvals on the phone are
the killer feature; the relay is a separate binary never bundled in an app;
the single-reader rule on the uplink socket (`bridge_uplink.rs:8-14`).

## 2. Facts relied on (recon 2026-09-15)

### 2.1 RPC (`crates/rpc`)

- jsonrpsee 0.26 on `rpc.listen` (default `127.0.0.1:7777`), token
  `<data_dir>/token` as `Authorization: Bearer` or `?token=`, plain `==`
  compare, no TLS, no frame limits set (`lib.rs:33-46, 66-96, 128-150`).
- `api::Server { events: broadcast::Sender<Event>, rt }` (`api.rs:441-447`);
  `Server::new(rt)` gives a dummy feed for in-process callers (the TUI);
  `serve` builds the real one from `bridge::start(rt)` (`lib.rs:69, 80-86`).
  `bridge::Bridge` fans 12 bus event types into `broadcast(1024)`
  (`bridge.rs:24-27, 63-76`).
- `subscribe_events(session?, descendants?)` takes `rx` before `accept`,
  `select!`s on `sink.closed()`, emits `{"type":"Lagged","count"}` on lag
  (`api.rs:1030-1079`). Notifications are
  `{"jsonrpc":"2.0","method":"subscribe_events","params":{"subscription":id,"result":…}}`.
- `RpcModule::raw_json_request(&str, buf_size) -> (Box<RawValue>, mpsc::Receiver<Box<RawValue>>)`
  dispatches one request in-process; subscription callbacks get a
  `MethodSink` over that receiver; `ConnectionId(0)` always
  (`jsonrpsee-core-0.26.0/src/server/rpc_module.rs:351-412`). The receiver
  yields the notifications above, already serialized.
- `Info { version, data_dir, default_model, approvals, compact_style }`
  (`api.rs:56-68`). ~100 methods; the host-local ones are in §9.
- `acp.rs:260-278` `save_blob(base64, mime, data_dir)` writes
  `<data_dir>/attachments/<sha256[..16]>.<ext>` — the upload pattern.
- `runtime::write_config_value(path, section, key, value)` persists one key
  (`runtime/src/lib.rs:93-134`); `lookup_when` (`:72-86`).
- `qrcode` 0.14 is a workspace dep; `signal::qr(&str) -> String` renders
  `Dense1x2` unicode (09 §11). No `snow`, `x25519-dalek`, `axum`,
  `tokio-tungstenite` in the workspace (`Cargo.toml`); `curve25519-dalek` is
  patched to Signal's fork under presage (`Cargo.toml:171`).

### 2.2 Desktop (`crates/desktop`)

- Rust side: launch/probe/quit, `save_attachment`, tray, shortcuts, autostart
  — macOS-only, `[[bin]]` only, no `[lib]`, no mobile entry, no `gen/apple`
  (`Cargo.toml:8-10`, `main.rs:33-59`).
- `RpcClient(url, token)`: token as `?token=`, backoff 500 ms→10 s,
  re-subscribes on reconnect, demux by `params.subscription`
  (`ui/src/rpc/client.ts:18-20, 48-59, 112-151`). `useConnection { state,
  client, api, info }` (`store/connection.ts:6-30`). `connection()` Tauri
  command or `VITE_THEUTH_URL/TOKEN` (`lib/tauri.ts:19-32`).
- CSP `connect-src … ws://127.0.0.1:* ws://localhost:*` (`tauri.conf.json:34`).
- shadcn Sidebar already switches to a `Sheet` under 768 px
  (`ui/sidebar.tsx:181-230`, `hooks/use-mobile.ts`), the only responsive
  code. Viewport meta lacks `viewport-fit=cover`; no `safe-area`, no touch
  handling. Fixed widths: `SettingsDialog` 880×640 with a 220 px nav,
  `ApprovalCard` sheet 480, `SessionMenu` sheets 640, `Skills` 560,
  `History` 480, `Explorer` aside 300, `Document` rail 260, `Tools` popover
  380. Hover-only: `MessageUser` actions. Hotkeys all `metaKey`
  (`lib/hotkeys.ts:16-38`). Traffic-light `pl-20` on five headers.
- Tauri-only calls without guards: `saveAttachment` on paste
  (`Composer.tsx:113`), `pickFiles` (`:259`), `openUrl` in five places.
  `Attachments.tsx:10` renders `asset://` under Tauri, a bare path otherwise.
- `qrcode.react` is already a dependency (Signal QR, `Channels.tsx:128`).
- Settings tabs: `TABS` in `SettingsDialog.tsx:24-34` plus the duplicate
  `SettingsTab` union in `slash.ts:66-75`.
- 25 §6.3 measured ≈ 700 `TextDelta`/s peak with 50 streaming sessions; a
  phone shows one session — the link carries ≈ 15–40 events/s in practice,
  700/s worst case.

### 2.3 Platform

- WebCrypto `X25519` (`generateKey`, `deriveBits`): Safari 17, Chrome 133,
  Firefox 130, WKWebView on iOS 17 `[INFERENCE: from release notes; the
  §12 step 4 check runs the handshake in each]`. `AES-GCM` with
  `additionalData`, `HKDF`, `HMAC`, `SHA-256`: everywhere.
- Noise's `HKDF(ck, ikm, 2|3)` is RFC 5869 with `salt = ck`, `info = ""`,
  `L = 64|96` — WebCrypto `HKDF.deriveBits` computes it exactly.
- `tauri-plugin-barcode-scanner` (mobile only), `tauri-plugin-deep-link`,
  `-notification`, `-opener`, `-dialog` support iOS; `-global-shortcut`,
  `-autostart`, `-positioner`, `-window-state`, tray are desktop-only.

## 3. Shape

```
  harness (theuth)                          relay (theuth-relay)                    devices
  ┌──────────────────────┐   wss /h/<id>  ┌───────────────────┐  wss /h/<id>  ┌─────────────┐
  │ link plugin          │──────────────► │ id → uplink       │ ◄─────────────│ iPhone app  │
  │  identity (X25519)   │  one socket,   │ conn ↔ device ws  │  one socket   ├─────────────┤
  │  devices.yaml        │  N conns       │ copy frames       │  per harness  │ laptop app  │
  │  Noise IK/IKpsk2     │                │ no state, no JSON │ ◄─────────────├─────────────┤
  │  raw_json_request ───┼─► rpc::Api     │ serves /  (web)   │               │ browser tab │
  └──────────────────────┘                └───────────────────┘               └─────────────┘
        plaintext JSON-RPC ◄════════ Noise ciphertext, opaque to the relay ════════► plaintext
```

Three deliverables, three crates/dirs:

| part | where | kind | depends on |
|---|---|---|---|
| relay | `crates/relay/` → binary `theuth-relay` | bin, not a plugin | tokio, axum, tokio-tungstenite, snow, tower-http, rustls (optional) |
| link plugin | `crates/link/` | plugin (00 §4; `docs/plugins.md`) | runtime, snow, tokio-tungstenite, jsonrpsee (`Methods` only), qrcode — **not** `rpc`: `rpc::Api` returns `LinkStatus` and `bridge.rs` forwards `LinkEvent`, so `rpc → link`; the dispatch handle arrives as a message (§6.2) |
| device transport + UI | `crates/desktop/ui/src/link/` and responsive changes across `ui/src` | TS | WebCrypto, `@tauri-apps/plugin-barcode-scanner`, `-deep-link` |

Dependency order: `runtime ← link ← signal ← rpc ← tui ← theuth`. `link` sits
with the channel plugins below `rpc` (which names its types, like every
plugin's); `signal` imports `link::qr` (one QR renderer); `tui` renders
`link`'s status and QR. The `Methods` that `link` dispatches into is not
injected by the binary (00 §1.4): `rpc::RpcPlugin::start` sends
`link::SetDispatch { methods, acp }` to the `link` actor (`lookup_when`,
like `signal`/`slack` resolve their peers); a conn arriving first waits for
it (§6.2). `acp` is 32 §4.3's mode-1 server.

## 4. Protocol

Two layers. The **relay layer** is what the relay sees: WebSocket binary
frames with a tiny header. The **channel layer** is what a device and a
harness say to each other inside a conn: Noise handshake messages, then Noise
transport messages carrying JSON-RPC.

### 4.1 Identity

- Harness: one X25519 static keypair, `<data_dir>/link/identity` (32 bytes,
  0600), created on first enable. **Harness id** = `base64url(pub)`, 43
  chars, appears in URLs and in the pairing string. The same 32 bytes are
  this harness's *device* static when it pairs with another harness as its
  parent (32 §4.1): on that node it is a device with id `base64url(pub)[..8]`.
- Device: one X25519 static keypair per install, generated on first pairing
  (§7.2). **Device id** (harness-side display) = first 8 chars of
  `base64url(pub)`.
- Neither side has a second key type. Signatures are not needed: possession
  is proven by completing a Noise handshake.

### 4.2 Relay layer

Subprotocol `theuth-link-1` on the upgrade. All frames binary. Text frames,
unknown ops and oversize frames close the socket (code 1002/1009).

**Harness uplink** — `GET /h/<id>?role=harness`, one per harness. Frame:
`op:u8 · conn:u32 LE · payload`.

| op | dir | meaning |
|---|---|---|
| `0x00 HELLO` | both | registration handshake (`conn = 0`): relay → harness `Noise_NK` message 1 on upgrade; harness → relay message 2. Relay checks the URL id equals the remote static it initiated against; on success the uplink is live. Until then nothing else is accepted. |
| `0x01 OPEN` | relay → harness | a device socket for this harness connected; `conn` is fresh |
| `0x02 DATA` | both | one channel-layer message for `conn` |
| `0x03 CLOSE` | both | `payload = reason:u8` (0 normal, 1 queue full, 2 idle, 3 refused, 4 harness gone) |

`Noise_NK_25519_AESGCM_SHA256`: initiator (relay) has no static and knows
the responder's (the id from the URL); two messages; the harness proves it
holds the secret for the id in its URL, so nobody can squat a harness id.
The relay discards the transport keys — it only needed the proof. A second
uplink for a live id replaces the first (the old one gets `CLOSE 4` on every
conn): the harness restarted faster than its old socket timed out.

**Device socket** — `GET /h/<id>`, one per (device, harness). Frame:
`op:u8 · payload`.

| op | dir | meaning |
|---|---|---|
| `0x02 DATA` | both | one channel-layer message |
| `0x03 CLOSE` | relay → device | `reason:u8` as above; the socket then closes |
| `0x04 PRESENCE` | relay → device | `online:u8`; sent on connect and on every change |

A device socket is **parked** while its harness is offline: the relay holds
it, sends `PRESENCE 0`, and when the harness registers sends `PRESENCE 1`
and `OPEN`s a conn. Any device `DATA` before that is dropped with `CLOSE 3`.
No explicit open from the device: connecting *is* opening.

Keepalive: WebSocket ping every 30 s from the relay, pong timeout 15 s; idle
(no frames in either direction) 10 min on device sockets → `CLOSE 2`. The
uplink never idles out.

### 4.3 Channel layer

Each `DATA` payload is exactly one Noise message (≤ 65 535 bytes). The first
messages on a conn are the handshake; the device is always the initiator.

**Pairing** — the device has scanned or pasted a pairing string (§4.4) and
knows the harness static `Hs` and the one-time secret `psk`:

```
Noise_IKpsk2_25519_AESGCM_SHA256   prologue = "theuth-link-1" ‖ harness_id
→ e, es, s, ss          payload: PairHello { name: String, platform: String }
← e, ee, se, psk        payload: PairAccept { name: String /* harness display name */ }
```

`psk2` mixes the secret after `se`. Message 2 proves to the device that the
harness holds the secret (it cannot be decrypted otherwise). The device's
possession is proven by its **first transport message**, which is encrypted
under keys that include the PSK; so the harness records the device's static
from message 1 into `devices.yaml` **only after** one transport message
decrypts (a stray message 1 with a valid static but no secret enrolls
nothing). The secret is deleted on success; one pairing at a time per
harness; TTL 5 min. On failure the conn is closed with `CLOSE 3` and nothing
is recorded.

**Session** — every later connection, from a device whose static is in
`devices.yaml`:

```
Noise_IK_25519_AESGCM_SHA256       prologue = "theuth-link-1" ‖ harness_id
→ e, es, s, ss          payload: mode:u8 | empty      (empty ≡ mode 0)
← e, ee, se             payload: SessionHello { name, version }
```

`mode` selects what the conn's records dispatch into (table in 32 §4.2):
absent or `0` is this doc's JSON-RPC conn; `1` is the ACP conn a parent
opens per remote child; anything else, or a payload longer than one byte,
is `CLOSE 3`.

The harness learns the device's static by decrypting message 1
(`get_remote_static` after `read_message`) and looks it up in
`devices.yaml`; unknown or revoked → `CLOSE 3` before message 2. Why `IK`
and not `KK` (both statics pre-shared): a `KK` responder must know *which*
device is talking before it can read message 1 (`ss` needs the peer's
static), which means either trying every paired static or the device
announcing its id in the clear — and the relay must not see device ids.
`IK` message 1 carries the static encrypted under `es`; the same message
layout as the pairing pattern, so the harness runs one parser. 1 RTT
through the relay; forward secrecy per connection from the ephemerals; no
session resumption (a reconnect is a handshake — cheap, and it is what makes
revocation immediate).

**Transport records.** After the handshake, each Noise transport message
decrypts to `flags:u8 · bytes`. `flags & 1` = FIN. A JSON-RPC message
(request, response or notification) is one record; a message longer than
65 518 bytes is split across consecutive records and reassembled at FIN.
Nothing else is framed: no length prefixes, no batching, no compression.
`ponytail:` one WebSocket frame per JSON-RPC message costs ≈ 40 bytes of
framing on a ~100-byte `TextDelta`; coalescing records per 4 ms tick is the
upgrade if cellular measurements (§11) say so.

Nonces are Noise's u64 counters; a connection would need 2⁶⁴ messages to
exhaust them, so there is no rekey. Nonce reuse is impossible by
construction because each connection has fresh keys.

### 4.4 Pairing string

```
https://<relay-host>[:port]/link#1.<b64url(Hs ‖ psk)>.<b64url(name)>
```

`1` is the version. `Hs ‖ psk` is 64 bytes → 86 chars. `name` is the
harness display name. Total ≈ 140–170 chars → QR version 7–8 (≈ 45×45
modules, 45×23 terminal cells in `Dense1x2`). The relay's WebSocket URL is
derived: `wss://<relay-host>[:port]/h/<b64url(Hs)>`.

Why a URL and not `theuth://…`: the system camera opens it. With the iOS app
installed and a universal link registered for `/link` (§8.3) the app opens
and pairs; without the app, the relay serves the web build at `/link`, which
reads `location.hash` and pairs in the browser. The fragment is never sent
in the HTTP request, so the relay serving the page does not see the secret
or the harness key. The same string is what "Copy key" copies and what the
paste box accepts; the parser also accepts the bare `1.<…>.<…>` part.

### 4.5 Limits (relay-enforced, also harness-enforced where marked)

| limit | value | on breach |
|---|---|---|
| frame size (payload) | 65 535 + 5 | close 1009 |
| conns per harness | 64 (`--max-conns`); harness `link.max_devices` 16 ⁽ʰ⁾ | `CLOSE 3` |
| new sockets per IP | token bucket 10/s, burst 30 | 429 on upgrade |
| per-conn outbound queue | 256 frames | `CLOSE 1` to that conn only; the uplink is never blocked by one slow device |
| uplink outbound queue | 4096 frames | close the uplink; the harness reconnects; devices see `PRESENCE 0` |
| pending handshake per conn | 10 s to first valid transport record ⁽ʰ⁾ | `CLOSE 3` |
| pairing | one pending, 5 min, single use ⁽ʰ⁾ | new `link_pair` replaces it |

## 5. Relay: `crates/relay` → `theuth-relay`

### 5.1 Hot path

```rust
// per device socket: one task reads the device, one task writes to it
// per uplink: one reader task demuxes by conn id into per-conn mpsc<Bytes>
DashMap<[u8; 32], Arc<Uplink>>           // harness id → uplink (registered after NK)
struct Uplink { tx: mpsc::Sender<Bytes>, conns: DashMap<u32, mpsc::Sender<Bytes>>, next: AtomicU32 }
```

- Read a `Message::Binary(Bytes)` from tungstenite (0.28: payloads are
  `Bytes`); the 5-byte header is read in place; the payload `Bytes::slice`
  is a refcount bump; `try_send` into the peer's queue; the writer task
  prepends the 1- or 5-byte header by writing two `Bytes` chunks (no
  concatenation — `tungstenite` accepts a `Bytes`, so the header and payload
  are joined once with `BytesMut::with_capacity(5 + len)`; `ponytail:`
  vectored writes if the profile says this copy matters).
- `WebSocketConfig { read_buffer_size: 4 KiB, max_message_size: 65 540,
  max_frame_size: 65 540, accept_unmasked_frames: false }`;
  `permessage-deflate` off (ciphertext does not compress).
- No JSON anywhere. No `String` on the copy path. No mutex on the copy path:
  `DashMap` is touched on `OPEN`/`CLOSE`, the per-conn `mpsc::Sender` is
  cloned into the reader task at `OPEN`.
- Routing lookups are one `DashMap::get` per new device socket, none per
  frame.

### 5.2 Endpoints

| path | purpose |
|---|---|
| `GET /h/<id>` (upgrade) | device socket (§4.2); `?role=harness` → uplink |
| `GET /healthz` | 200 |
| `GET /link`, `GET /`, `GET /assets/*` | the web build, when `--web <dir>` is given (§7.6); `/link` serves `index.html` |
| `GET /metrics` | text counters: sockets, uplinks, frames, bytes, closes by reason (no labels, no per-harness rows) |

### 5.3 CLI and config

```
theuth-relay --listen 0.0.0.0:443 [--tls <cert.pem> <key.pem>] [--web <dir>]
          [--max-conns 64] [--cookie-secret <hex>]
```

No config file. `--tls` terminates in-process with rustls (`axum-server`);
without it, plain HTTP behind whatever terminates TLS. `--cookie-secret`
seeds the HMAC that signs the browser cookie (§7.6); absent → random per
start (browser sessions reset on restart; fine).

### 5.4 Deploy

`deploy/relay/Dockerfile`: `rustup target add x86_64-unknown-linux-musl`,
`cargo build --release -p relay --target …-musl` (rustls, no OpenSSL),
`FROM scratch`, `USER 65532`, `ENTRYPOINT ["/theuth-relay"]`. Expected image
≈ 8–12 MB. `deploy/relay/compose.yaml`: the relay plus a Caddy for TLS as
the boring default. No kustomize in v0 — one process is the deployment;
§5.5 is how it grows.

### 5.5 Scale-out and HA

Every socket that belongs to one harness — its uplink and all its device
sockets — has the same URL path `/h/<id>`. Run N relays behind any load
balancer that hashes on the path (HAProxy `balance uri` with `hash-type
consistent`, nginx `hash $uri consistent`, Envoy ring hash on `:path`) and
they co-locate without talking to each other. Relays share nothing; a relay
crash drops its sockets, everyone reconnects (§6.2, §7.3), the hash sends
them to the same surviving relay. A rolling deploy moves ≈ 1/N of harnesses
once each. There is no cross-relay path, no membership, no lease.
`ponytail:` a device whose LB hash disagrees with the harness's (misconfigured
LB) is parked forever with `PRESENCE 0`; the `/metrics` `parked` counter
makes this visible.

### 5.6 What the relay learns

Client IPs, harness ids (public keys, already public in the QR), when each
is online, how many devices talk to it, frame sizes and timing. Not: device
identities, method names, content, session ids, attachment bytes. This is
the disclosure sentence in Settings › Link: *"The relay carries encrypted
traffic between your devices and this harness. It can see that they are
connected, not what they say."*

## 6. Link plugin: `crates/link`

### 6.1 Config

```yaml
link:
  enabled: false                          # `/link on`, Settings toggle, or hand-edit; hot
  relay: wss://relay.theuth.io          # operator-chosen default (§16); self-host: your URL
  name: ""                                # display name in the QR and on devices; "" → hostname
  max_devices: 16
```

`enabled: false` leaves `link/identity` and `link/devices.yaml` in place
(re-enabling does not re-pair). The actor watches its own `Enable{bool}`
command and persists through `write_config_value`; a `relay` change
reconnects. Env: `THEUTH_LINK__ENABLED`, `THEUTH_LINK__RELAY`.

### 6.2 Actor, commands, events

Actor `"link"` (registered name), one per process.

```rust
// commands (ask)
Status  -> LinkStatus { enabled, relay, name, harness_id, connected: bool,
                        devices: Vec<Device>, pairing: Option<Pairing> }
Enable  { enabled: bool, relay: Option<String> } -> ()
Pair    -> Pairing { key: String /* the §4.4 string */, expires_at_ms: u64 }
CancelPair -> ()
Rename  { name: String } -> ()                         // harness display name
RenameDevice { id: String, name: String } -> ()
Revoke  { id: String } -> ()                           // closes live conns, deletes the row

pub struct Device { id: String, name: String, platform: String,
                    paired_at_ms: u64, last_seen_ms: u64, online: bool }

// events (bus, forwarded by rpc::bridge)
#[serde(tag = "type")]
pub enum LinkEvent {
    LinkRelay   { connected: bool },
    LinkPairing { key: Option<String>, expires_at_ms: Option<u64> }, // None = expired/consumed/cancelled
    LinkPaired  { device: Device },
    LinkDevice  { id: String, online: bool },
    LinkRevoked { id: String },
}
```

**Uplink task**: dial `relay`, answer `NK`, then the single-reader loop —
the only `ws.next()` caller, demuxing `DATA` by conn into per-conn tasks;
`OPEN` spawns a conn task; `CLOSE` drops it. Reconnect with backoff 1 s →
30 s ×2 with ±20 % jitter, reset after 60 s connected. `LinkRelay` on every
transition.

**Conn task**: run the responder handshake — read message 1 as `IK`; if it
authenticates and the remote static is in `devices.yaml`, continue as a
session; otherwise, if a pairing is pending, re-read the same bytes as
`IKpsk2` (the two patterns share the message-1 layout but not the handshake
hash, so a message written for one fails cleanly under the other — at most
two DH pairs per attempt); otherwise `CLOSE 3`. Then dispatch by the mode
byte (§4.3): mode 1 hands a `RecordChannel` to `SetDispatch.acp` and pumps
records both ways (32 §4.3); mode 0 loops: decrypt a record → reassemble at FIN →
`methods.raw_json_request(&text, 4096)` on a **spawned** task per request
(a slow method must not block the conn) → encrypt the response → `DATA`.
The `mpsc::Receiver` from the first `subscribe_events` request is drained by
the same conn task into records; its notifications are already the wire
shape (§2.1). Subscription buffer 4096: the phone falls behind → `Lagged` →
it reloads like the desktop does (25 §6.1 step 6). A conn is `Device.online`
from handshake to close; `last_seen_ms` is written on close (one write, not
per frame).

**Methods object**: `rpc::RpcPlugin::start` builds `rpc::methods(rt)`
(`rpc::api::Server { events: rpc::bridge::start(rt).await, rt }.into_rpc()`)
and `tell`s the `link` actor `SetDispatch { methods, acp }`; the actor stores
it in the shared `Ctx` (`OnceCell` + `Notify`) and every conn task awaits it
before its first dispatch — no error path, the handshake deadline bounds
the wait. `rpc::bridge::start` is idempotent (§14): it
registers the `Bridge` actor as `"rpc.events"` and returns the existing
sender when one is registered, so a harness running both the rpc and the
link plugins serializes each event once. `raw_json_request` runs every conn
under `ConnectionId(0)`; subscription ids are random u64s from
`RandomIntegerIdProvider`, so one device cannot unsubscribe another's by
accident, only by guessing a 64-bit id — accepted.

### 6.3 Data on disk

```
<data_dir>/link/identity        32 bytes, 0600; delete to rotate (every device must re-pair)
<data_dir>/link/devices.yaml    - { id, pub: b64url, name, platform, paired_at_ms, last_seen_ms }
```

Both may be deleted; `identity` regenerates, `devices.yaml` starts empty.
Nothing is cached elsewhere.

### 6.4 RPC additions (`rpc::Api`; `docs/plugins.md` §4 step 2 procedure)

| method | → | for |
|---|---|---|
| `link_status()` | `LinkStatus` | Settings › Link, `/link`, the device's harness name refresh |
| `link_enable(enabled, relay?)` | `()` | toggle + relay URL |
| `link_pair()` | `Pairing` | show QR / copy key |
| `link_cancel_pair()` | `()` | |
| `link_rename(name)` | `()` | |
| `link_rename_device(id, name)` | `()` | |
| `link_revoke_device(id)` | `()` | |
| `put_attachment(bytes_b64, ext)` | `PathBuf` | `acp.rs` `save_blob` lifted into `Api`; ext ∈ png jpg jpeg gif webp pdf; 5 MB / 32 MB caps as `supervisor.rs:2293-2312` |
| `get_attachment(path)` | `String` (b64) | only under `<data_dir>/{attachments,browser,computer}`; canonicalized; else `-32000` |
| `LinkEvent` | bridge `forward!` + `export.rs` | live Settings/TUI |
| `Info.name: String` | | the harness display name for the device's switcher (`link.name` or hostname) |

`export.rs` gains `LinkStatus`, `Device`, `Pairing`, `LinkEvent`;
`bindings/methods.json` gains the nine names and the vitest catches drift.

Since 48 (2026-09-18) `get_attachment` also serves the preview files the
history builder materializes for stored binaries (17 §11), written by the
same naming rule — now `agent::context::save_blob`, which `api.rs`
re-exports for `put_attachment` and the ACP prompt path. No new method,
cap or directory: the same `<data_dir>/attachments` guard applies on the
phone as on the desktop.

### 6.5 TUI: `/link`

`COMMANDS` row `("/link", "[on|off|pair|rm <n>|name <n> <name>|relay <url>]",
"pair and manage phones")`; `Menu::Link` (modal) renders:

```
 link · relay.theuth.io · connected · Studio Mac (Hs 7Kq2…)
 ● Sky's iPhone      iOS      paired 3d ago    online
 ○ laptop            macOS    paired 1d ago    seen 2h ago
 ─────────────────────────────────────────────
 p pair a device   r rename   x revoke   n harness name   e disable   Esc
```

`p` → `link_pair` → a transcript card (like `signal_link`, 09 §11): the QR
from `link::qr(key)`, the key on one line, `expires in 5:00` counting down
from `LinkPairing`, replaced by `paired: Sky's iPhone` on `LinkPaired`.
`r`/`n` use the existing `Prompt` multi-step input (`model.rs:1045-1077`).
Bare `/link` opens the menu; `/link on` when disabled enables and opens it.

### 6.6 CLI: `t --link`

For a headless box with no TUI: `t --link` connects to the running daemon
as an RPC client (`rpc.listen` + `<data_dir>/token`, jsonrpsee `ws-client`
— today a dev-dep), calls `link_pair`, prints the QR and the key, waits for
`LinkPaired` on `subscribe_events`, prints `paired: <name>` and exits.
`--link` conflicts with `--headless`/`--serve`/`--acp`/`-p` like
`--link-signal`. If the daemon is not running: `no harness on
<rpc.listen>; start `t --headless` first`.

## 7. Device side (`crates/desktop/ui`)

### 7.1 Transport

`RpcClient` today owns a `WebSocket` and speaks text. It gains a `Transport`
seam and nothing else changes above it:

```ts
interface Transport {                       // src/rpc/transport.ts
  send(text: string): void
  onMessage(cb: (text: string) => void): void
  onState(cb: (s: ConnState) => void): void
  connect(): Promise<void>; close(): void
}
class LocalTransport implements Transport   // today's WebSocket + ?token=
class LinkTransport  implements Transport   // src/link/transport.ts: WebSocket to wss://relay/h/<id>,
                                            // PRESENCE/CLOSE ops, Noise IK, records with FIN
```

`new RpcClient(transport)`; the id counter, pending map, subscription demux
and reconnect policy are unchanged (`client.ts:62-151`). `LinkTransport`
reconnect: the relay socket with the existing 500 ms→10 s backoff; on
`PRESENCE 0` state `reconnecting` with the banner "Studio Mac is offline";
on `PRESENCE 1` a new `IK` handshake, then `RpcClient` re-subscribes as it
does today.

### 7.2 Keys and Noise in WebCrypto (`src/link/noise.ts`, `src/link/keys.ts`)

- Identity: `crypto.subtle.generateKey({ name: 'X25519' }, true, ['deriveBits'])`
  once, exported as PKCS#8 and stored with the raw public key in IndexedDB
  (`db link`, store `identity`); on every load it is imported back with
  `extractable: false`, so the in-memory key never leaves WebCrypto. As-built
  note: v0 stored the non-extractable `CryptoKey` itself. WKWebView wraps a
  serialized `CryptoKey` with a WebCrypto master key that did not survive an
  app relaunch on iOS, so the row came back unusable and every handshake
  failed — a paired phone looked unpaired after each restart. A legacy
  `CryptoKey` row is kept while it still derives (desktop), else replaced.
  Every DH is `deriveBits({ name: 'X25519', public }, priv, 256)`.
- `MixKey`/`MixKeyAndHash`: `importKey('raw', dh, 'HKDF')` →
  `deriveBits({ name: 'HKDF', hash: 'SHA-256', salt: ck, info: empty }, 512|768)`
  — exactly Noise's HKDF (§2.3). Cipher keys are imported as non-extractable
  `AES-GCM` keys; `encrypt/decrypt` with `iv = 0⁴ ‖ n:u64 BE`,
  `additionalData = h`. `MixHash` is `digest('SHA-256')`.
- Patterns implemented: `IKpsk2` initiator, `IK` initiator. ≈ 200 lines,
  no dependency; the §12 step 4 check runs `noise.ts` against `snow` in
  both directions for both patterns with test vectors.
- Harness list: IndexedDB store `harnesses` `{ id, name, relayHost,
  pairedAt, lastSeen, order }`, and `active`. Nothing in `localStorage`,
  nothing in `sessionStorage`. `ponytail:` the PKCS#8 bytes are readable by
  the origin; if a Keychain-only store is ever required on iOS, a Tauri
  command wrapping `security-framework` replaces the IDB identity store
  behind the same `keys.ts` interface.

### 7.3 Connection model

```ts
// src/link/harnesses.ts (zustand, persisted to IDB)
{ harnesses: Harness[]; active: string /* 'local' | harness id */;
  presence: Map<id, 'online'|'offline'|'connecting'> }
```

- `local` is the desktop's own daemon (25's launch path), present only when
  `isTauri() && platform === 'macos'`. It is a harness like any other in the
  switcher, named from `Info.name`. 32 §8.1 adds `parent?: string` to
  `Harness`: a node the parent introduced (32 §8.2) is stored under it.
- On app start every paired harness gets a `LinkTransport` and an
  `RpcClient`; all connect (presence dots); **only the active one** calls
  `subscribe_events(null, null)`. Switching: the old client unsubscribes,
  the new one subscribes and the UI remounts (§7.4). N sockets idle at zero
  bytes; on iOS the OS closes them in the background and they reconnect on
  foreground (`visibilitychange`).
- `Info.name` from each harness refreshes the stored name after connect.
- *ASK-SYNC addendum (2026-09-27).* Every `subscribe_events` stream — first
  subscribe, relay reconnect, foreground re-issue — opens with a
  `PendingPrompts` snapshot (approvals, asks, secret requests still pending
  on the engine); the client `replace`s its three pending stores with it.
  A `*Resolved` lost in the gap can therefore not leave a stale card, and a
  `*Requested` lost in the gap still shows. Answering a prompt another
  client already answered returns `false`; the card drops locally.

### 7.4 Per-harness state

Every zustand store the events feed (`useSessions`, `useApprovals`, …) and
every React Query key is scoped by harness: stores are created by factories
and held in `Map<harnessId, Stores>`; `HarnessProvider` puts the active
harness's stores in context and the hooks read from context; the route tree
is rendered under `<HarnessProvider key={active}>` so a switch is a remount.
Query keys are prefixed `[active, …]`. Hidden harnesses cost nothing: no
subscription, no store updates. Rule from jarvis kept: never fan two
harnesses into one store.

*48 addendum (2026-09-18).* The context-removal controls (25 §3 `/shake`
row) make this scoping load-bearing rather than cosmetic: every
`remove_context` is issued against the `Stores` captured for its harness
before the await (`scope(req.harness)`), and the post-commit settled reload
is dispatched only if that harness is still `active`. An RPC that completes
after a switch therefore lands in the harness it was meant for or is
dropped; it never touches the newly active harness's slice. Switching away
`unload()`s the inactive harness's sessions (`loaded: false`,
`contextStale`, `historyGeneration + 1`), so returning to it refetches
authoritative history and no removed item can be shown from a stale cache.

### 7.5 Pairing UX

**Harness side (desktop Settings › Link, TUI `/link`):**

```
 Link                                                   [on ]
 Relay   relay.theuth.io                 connected ●
 Name    Studio Mac                                    [edit]
 ────────────────────────────────────────────────────────────
 Pair a device                                [Show QR]
   ┌────────┐  Scan with your phone's camera, or
   │  QR    │  [Copy key]  · expires in 4:32
   └────────┘
 Devices
   ● Sky's iPhone   iOS    paired 3 d ago   online       [rename] [revoke]
   ○ laptop         macOS  paired 1 d ago   seen 2 h ago [rename] [revoke]
 The relay carries encrypted traffic between your devices and this harness.
 It can see that they are connected, not what they say.
```

Toggle → `link_enable`; QR from `qrcode.react` on `Pairing.key`;
`LinkPaired` → toast "Paired Sky's iPhone", the row appears, the QR
collapses. This tab is available when the app is itself connected through
link (it then manages *that* harness's devices — the sidebar says which).

**Device side:**

- First run with nothing paired (iOS, browser, or desktop with no local
  daemon): route `/link` — big "Scan QR" (Tauri mobile only) and "Paste key"
  with a text box; both parse §4.4, show "Pairing with Studio Mac…", then
  navigate to `/` on `PairAccept`. Errors are the three that exist: expired
  or used key, harness offline, relay unreachable.
- Universal link / web landing: `/link#…` → same route, auto-starts.
- Add another: harness switcher › "Add harness…" → `/link`.
- The switcher: a button in `SidebarHeader` (`● Studio Mac ▾`) listing
  paired harnesses with presence dots, "Add harness…", "Manage…" (→ Settings
  › Harnesses, renamed from today's ACP "Harnesses" tab which becomes
  "Agents" — see §14). Manage: rename locally, reorder, "Forget" (deletes
  the local entry; the harness still lists the device until revoked there —
  the copy says so). On phones the switcher stays in the sidebar, reached from
  the chat header's sidebar button. 32 §8.1 nests nodes under their parent in
  this list.
- `/link` with a key from a node's QR (32 §4.4) offers the choice "as a
  node of <parent>" beside plain pairing (32 §8.3); Settings › Nodes sits
  beside Settings › Link on the parent (32 §8.3); revoking a device on the
  parent cascades to its nodes, so the parent's revoke copy becomes "…and
  on its N nodes" (32 §8.4).

### 7.6 Browser container

`bun run build` already produces a static Vite bundle. The relay serves it
with `--web ui/dist`. Rules:

- First `GET /` or `/link` without the cookie sets
  `__Host-link=<random 32 B, HMAC-signed with --cookie-secret>; Secure;
  HttpOnly; SameSite=Strict; Path=/; Max-Age=34560000`. WebSocket upgrades
  whose `Origin` is the relay's own origin **must** carry a valid cookie
  (else 403); other origins (`tauri://localhost`, `http://tauri.localhost`,
  none) are not asked for one. The relay keeps a per-cookie token bucket and
  a deny-list (`/metrics` shows counts). This is the browser-session
  requirement met without `localStorage`: the cookie is invisible to page
  script, cannot be exfiltrated by XSS, and gives the relay a handle to
  throttle or cut off one browser. **It is not the credential.** The
  end-to-end identity is the WebCrypto key (§7.2); a stolen cookie lets an
  attacker open sockets the harness will refuse at `IK` message 1.
- CSP on served pages: `default-src 'self'; connect-src 'self' wss: ws:;
  img-src 'self' data: blob: https: http:; style-src 'self' 'unsafe-inline'`
  (streamdown needs inline styles and images load from the web, as in
  Tauri today).
- Trust: a browser container trusts the origin that serves its JavaScript.
  A relay operator who ships modified JS can read plaintext in that browser.
  The native apps do not have this exposure. Settings › About says so in one
  sentence; self-hosting the relay removes it.

### 7.7 Capability gating

`src/lib/platform.ts`: `container(): 'macos' | 'ios' | 'web'`, `touch()`
(`matchMedia('(pointer: coarse)')`). Every `isTauri()`-guarded call that
today assumes macOS is re-keyed on `container()`; the unguarded ones in
§2.2 (`saveAttachment`, `pickFiles`, `openUrl`) are replaced (§9).

### 7.8 Device-socket liveness (95)

`LinkTransport` arms exactly one timer per socket (`src/link/transport.ts`):

- `CONNECT_MS = 15_000` — socket creation → first `PRESENCE`; expiry abandons
  the socket like `reset()` (`lost`, `reconnecting`, the `RpcClient` backoff).
- `HANDSHAKE_MS = 10_000` — `PRESENCE 1` → `ready`; same expiry.
- `PARKED_RECYCLE_MS = 60_000` — while parked (`PRESENCE 0`) the socket is
  replaced in place, silently: `connect()`'s promise is kept, no `connecting`
  event/state/presence, and the fresh socket's repeated `PRESENCE 0` is not
  re-reported. The relay drops a dead parked socket at 45 s and only notifies
  its *current* parked set, so the swap is also the presence poll.

`RpcClient.#foreground` resets and redials in `connecting` as well as
`reconnecting`; `probe()` stays `open`-only.

## 8. Mobile accommodations

### 8.1 Layout rules (applied once, in `styles.css` and the shell)

| rule | where |
|---|---|
| `<meta name="viewport" content="width=device-width, initial-scale=1, minimum-scale=1, maximum-scale=1, viewport-fit=cover, interactive-widget=resizes-content">`; `color-scheme` follows the theme (`:root` light, `.dark` dark) so form controls and the iOS keyboard match | `index.html`, `styles.css` |
| iOS keyboard: WebKit shrinks only the visual viewport, so `lib/viewport.ts` publishes `--visible-band` (layout height, or the band above the keyboard while an editable has focus) and `data-keyboard-open` on `<html>`; full-height shells use `h-band`, and `--safe-bottom` is `0` while the keyboard is open. The native "‹ › Done" accessory bar is removed by building the window in Rust with `with_input_accessory_view_builder(\|_\| None)` (`lib.rs`, `tauri.ios.conf.json` `windows: []`) | `viewport.ts`, `styles.css`, `__root.tsx`, `SettingsDialog.tsx`, `SearchDialog.tsx`, `PairScreen.tsx`, `src/lib.rs` |
| safe areas as `--safe-{top,right,bottom,left}` on `SidebarInset` and every `Sheet` side; the mobile settings sheet is `top-0 h-band pt-[var(--safe-top)]` with the X in its header row (the absolute one landed on the first row's chevron) | `styles.css`, `__root.tsx`, `ui/sidebar.tsx`, `ui/sheet.tsx`, `SettingsDialog.tsx` |
| composer pinned bottom with `pb-[max(0.5rem,env(safe-area-inset-bottom))]`; grows to 6 lines on touch (8 on desktop) | `Composer.tsx` |
| min target `min-h-11 min-w-11` on icon buttons under `pointer-coarse:`; `SidebarMenuButton` height 44 under `pointer-coarse:` | `ui/button.tsx`, `ui/sidebar.tsx` |
| sheets: `w-full sm:w-[480px]` (approvals, history), `sm:w-[640px]` (prompt, jobs), `sm:w-[560px]` (skills) — mobile full-width slide-up | the five components in §2.2 |
| `SettingsDialog`: under `sm` a full-screen sheet: tab list → pushes the tab's content with a back chevron; ≥ `sm` unchanged 880×640 | `SettingsDialog.tsx` |
| knowledge: under `md` `/knowledge` shows the tree, `/knowledge/$id` the document with a back chevron; the 260 px rail becomes a collapsible section at the bottom | `Explorer.tsx`, `Document.tsx` |
| `Tools` popover `w-[min(380px,calc(100vw-2rem))]` | `Tools.tsx` |
| hover-only actions visible under `pointer-coarse:` (`opacity-100`), `group-hover` kept for pointers | `MessageUser.tsx`; shadcn `showOnHover` already does this for `SessionItem` |
| Enter: sends on `pointer-fine`, inserts newline on `pointer-coarse`; send button always visible on touch | `Composer.tsx:219-222` |
| hotkeys: registered only when `!touch()`, except ⌘K/Ctrl+K search, which stays bound so a hardware keyboard on a touch device opens it (49 §5.3); labels `⌘` on macOS, `Ctrl` elsewhere; `Shortcuts` tab hidden on touch | `hotkeys.ts`, `Shortcuts.tsx`, `AppSidebar.tsx:45,52` |
| traffic-light `pl-20` and `data-tauri-drag-region` only when `container() === 'macos'` | the five headers in §2.2 |
| transcript `max-w-[768px]` → `w-full md:max-w-[768px]`; user bubble `max-w-[85%] md:max-w-[70%]` | `Transcript.tsx`, `MessageUser.tsx` |
| `body { cursor: default }` only on `pointer-fine`; `-webkit-tap-highlight-color: transparent`; `overscroll-behavior: none` on the shell | `styles.css` |
| search (49): one dialog mounted in the root shell outside the mobile Sheet, so opening it from the drawer closes the drawer and keeps search; full-screen under `md` (the sidebar breakpoint) inside `h-band`, query and wrapping category chips fixed above the scrolling results, 44 px targets; the magnifier sits left of the sidebar toggle in the collapsed/mobile chat and explorer headers and at the trailing edge of mobile knowledge/artifact detail headers, so search is one tap away without expanding the sidebar | `__root.tsx`, `SearchDialog.tsx`, `SearchTrigger.tsx`, `Header.tsx`, `Explorer.tsx`, `Document.tsx`, `Viewer.tsx` |
| approval card buttons 44 px, Allow/Deny side by side full width on touch | `ApprovalCard.tsx` |

Nothing above changes a desktop pixel at ≥ 768 px with a fine pointer; the
§12 step 5 check screenshots the four 25 §4.4 screens before and after.

### 8.2 Behaviour on phones

- Notifications: `tauri-plugin-notification` on iOS for approvals and
  finished turns while the app is backgrounded — same `notifyIfHidden`
  (`notify.ts`), gated on `document.hidden`; permission is asked once at
  launch (`primeNotifications`), since iOS shows the prompt only in the
  foreground. Backgrounding holds a `UIBackgroundTask` (`lib.rs`
  `mobile::background`, on tao's `Suspended`/`Resumed`): the socket and the
  webview keep running for the ~30 s iOS grants, so a turn that finishes
  right after the user leaves still posts its notification. Beyond that iOS
  suspends the process and the socket dies without telling the page; on
  `visibilitychange`/`pageshow` the `RpcClient` reconnects at once when
  `reconnecting` (backoff skipped, a parked socket dropped) and probes an
  `open` socket with `info`, `reset()`ing the transport after 5 s of silence.
  After a reconnect `reload()` marks every transcript unloaded so the chat
  route refetches when the router remounts. No push service: APNs via the
  relay would need the relay to see something; it cannot. A harness-side
  push through the user's Signal/Slack (`message_user`, 24) is the honest
  route if wanted later.
- Attachments: `pickFiles` → `<input type="file" accept="image/*,application/pdf">`
  everywhere except macOS Tauri (native dialog kept); paste and drop use
  HTML5 events; bytes go through `put_attachment` (§9). Images render via
  `resolveFile(path)`: `asset://` on macOS local, `get_attachment` → blob URL
  (LRU 50) otherwise.
- Camera: `tauri-plugin-barcode-scanner` `scan({ windowed: true, formats: [QR_CODE] })`
  in the `/link` route; the webview background is made transparent for the
  duration as the plugin requires.
- Keyboard: `interactive-widget=resizes-content` keeps the composer above
  the keyboard; the transcript's stick-to-bottom logic (25 §3) already
  handles the resize.

### 8.3 iOS packaging

- `crates/desktop`: add `[lib] name = "desktop_lib" crate-type = ["staticlib", "cdylib", "rlib"]`
  with `#[cfg_attr(mobile, tauri::mobile_entry_point)] pub fn run()`;
  `main.rs` becomes `fn main() { desktop_lib::run() }`. Everything in
  `launch.rs`, tray, activation policy, global-shortcut, autostart,
  positioner, window-state, the `main` window's `titleBarStyle` and
  `save_attachment` (deleted anyway) goes behind `#[cfg(desktop)]`; the
  plugin list is split `#[cfg(desktop)]` / `#[cfg(mobile)]` (`barcode-scanner`,
  `deep-link`).
- `cargo tauri ios init` → `gen/apple/` (Xcode project committed; `build/` and
  Externals gitignored). Bundle id `io.theuth.ios`;
  `tauri.ios.conf.json` overlay: `identifier`, `bundle.iOS.minimumSystemVersion
  "17.0"`, no `externalBin`, CSP `connect-src 'self' ipc: http://ipc.localhost wss:`.
- Universal link: `apple-app-site-association` served by the relay at
  `/.well-known/apple-app-site-association` (`--aasa <team-id>`) for path
  `/link`; `tauri-plugin-deep-link` delivers the URL to the webview.
- ATS default (wss only). Orientation: all. iPad: same layout, the sidebar
  is a panel at ≥ 768 pt.
- `just ios` (`cargo tauri ios dev`), `just ios-bundle` (`cargo tauri ios
  build --export-method app-store-connect`); signing via Xcode's automatic
  signing with the team in `.env.signing` (`scripts/load-signing-env.sh`
  already exists).
- The macOS `.app` is unchanged (25 §5): still a menubar app, still launches
  the local daemon, now also a link device.

## 9. Parity over link

Everything in 25 §3 is an `Api` call and works unchanged through §4.3. The
rows that assume the client is on the harness host:

| flow | today | over link |
|---|---|---|
| paste/drop/pick attachment | Tauri `save_attachment` writes `<data_dir>/attachments` | `put_attachment(bytes_b64, ext)` → path → `send_input.files` as today; **every container** uses it; the Tauri command is deleted |
| render an attachment or tool image | `asset://` scoped to `data_dir` | `resolveFile`: `asset://` on macOS local, else `get_attachment` → blob URL |
| `login(provider)`, `mcp_login` | returns URL and the daemon runs `open` on its host; callback on the daemon's loopback | the row shows "Sign in from the harness (Studio Mac)" and the URL as text; the daemon still opens its own browser, which is where the login happens |
| Settings › Computer "Open System Settings" | opener deep link | hidden unless `container() === 'macos' && active === 'local'`; the permission rows still show (they are the harness's) |
| quit / relaunch / launch log / `shutdown` | owned daemon only | hidden unless `active === 'local' && owned` |
| `Info.data_dir` | shown in Settings › General | shown with the harness name; it is the harness's path |
| `message_user` desktop sink | none (25 §7) | none |
| `browser_screenshot`, `computer_*` | base64 over RPC | unchanged — they were already remote-shaped |
| Signal QR (`signal_link`) | `QRCodeSVG` | unchanged |
| search: `search`, `search_refresh`, `get_artifact`, `artifact_search_match`, `knowledge_read_revision` (49) | ordinary `Api` calls; matching, extraction and history walks run inside the harness | unchanged: the device receives typed groups, snippets and ids over the channel, never the corpus; a hit opens through the same RPC reads (no local filesystem shortcut). A device whose UI predates 49 gets Method not found for `search` (`search_sessions` is gone) and must update; it is not a sessions-only fallback |
| Settings › Communication, peer picker, peer cards, `a2a_status`/`reload_a2a` (50, 2026-09-18) | `Api` methods and `CommunicationChanged`/`PeerMessageChanged`/`A2aChanged` on the one subscription | unchanged: the same records over `raw_json_request`; a paired device sees the harness's effective switch state and peer requests, and toggling from the phone invalidates the desktop through the same events. Link carries no A2A frames: the relay is not an A2A transport and gains no mode for it (§13.3) |

## 10. Threat model

| threat | answer |
|---|---|
| Relay operator reads traffic | Noise E2E; relay sees §5.6 metadata only |
| Relay operator swaps the served web build | browser container only (§7.6); native apps unaffected; disclosed; self-host |
| Someone photographs the QR before the phone scans | 5-min TTL, single use, the first `IKpsk2` wins; the owner sees an unexpected device name in the toast and row → revoke. Optional `link.confirm: true` adds an Allow prompt on the harness (not default; the QR is already the consent) |
| Stolen pairing string after use | dead: the secret was consumed |
| Stolen phone | Revoke on the harness (TUI or desktop): live conns closed, next `IK` refused. Device key is non-extractable and WebKit-wrapped (§7.2) |
| Stolen harness `identity` | attacker can impersonate the harness to paired devices; rotate (`rm link/identity`, restart) and re-pair. Same blast radius as a stolen `<data_dir>/token` today, and `data_dir` is 0700 |
| Harness id squatting on the relay | `NK` registration (§4.2) |
| Relay DoS by opening device sockets | per-IP bucket, per-harness cap, parked sockets counted; handshake failures cost the harness one DH |
| Slow device stalls the harness | per-conn queue; `CLOSE 1` to that conn only (§4.5) |
| MITM between device and relay, or relay and harness | TLS to the relay; even without it, Noise `IK` authenticates both statics — the relay link is not the trust boundary |
| Replay of a `IK` message 1 | fresh ephemeral per handshake; a replayed message 1 yields a session the replayer cannot read |
| Downgrade to pairing by an unknown device | `IKpsk2` is attempted only while a pairing is pending, and completes only with the PSK |
| Cross-harness confusion on the device | stores keyed by harness id (§7.4); the transport is bound to one `/h/<id>` |
| Timing side channel on the relay | nothing secret to leak: no compare, no keys held past `NK` |
| A remote A2A agent uses link or the console token to reach `Api` | it cannot: A2A peers are served by the `a2a` plugin's own HTTPS listener with per-peer vault credentials (`a2a::config::stage` refuses a peer secret equal to `<data_dir>/token`); no A2A relay mode, no proxying of A2A through `Api` credentials, and an A2A principal is never a link device (50 §6.2, 32 §13.1) |
| Peer text arriving over A2A forges console provenance | peer bodies are bounded (64 KiB before decode, `PEER_TEXT_MAX` after) and redacted before persistence; the device renders `PeerRecord` fields as provenance, never the body's own claims (50 §9, 37 §15.3) |

## 11. Performance budgets and how they are measured

| what | budget | check |
|---|---|---|
| relay added latency | p99 < 1 ms, one core, 10 k device sockets on 100 harness uplinks, 100 msg/s each (1 M frames/s aggregate) | `crates/relay/bench/` throwaway: a bun or Rust driver that plays both ends against `theuth-relay` on loopback and histograms echo RTT; deleted after the number is recorded in §13 |
| relay memory | ≤ 16 KiB RSS per idle socket at 100 k sockets (≈ 1.6 GB) | same driver, idle phase, `ps` |
| relay CPU per frame | ≤ 2 µs | driver, `perf`/Instruments |
| phone end-to-end | a `TextDelta` published on the harness renders on the phone within 40 ms over LAN (relay on the LAN), 150 ms over cellular | timestamp in a debug event; measured once |
| handshake | `IK` through a LAN relay ≤ 30 ms; cold app start with three paired harnesses shows presence for all within 500 ms | Instruments / `performance.now()` |
| stream rendering on iPhone | 60 fps on one streaming session (25 §6's rAF drain unchanged) | Safari Web Inspector timeline over the WKWebView |
| bytes on cellular | one hour of active chat ≤ 5 MB | measured; decides the `ponytail:` batching note in §4.3 |

## 12. Phases and checks

Each ends with a runnable check, not a test suite.

| step | deliverable | check |
|---|---|---|
| 1 | `crates/relay`: uplink `NK`, device sockets, `OPEN/DATA/CLOSE/PRESENCE`, limits, `/healthz`, `/metrics`, `--tls`, Dockerfile | bench (§11) numbers recorded; two harness ids and eight devices on one relay: frames never cross conns (assert payload tags); a slow device gets `CLOSE 1` while its siblings keep streaming; killing the relay and restarting it: uplink and devices reconnect and resume |
| 2 | `crates/link`: identity, uplink, `IKpsk2` pairing, `IK` sessions, records, `raw_json_request` dispatch, `devices.yaml`, actor + events; `rpc`: idempotent `bridge::start`, `Api` additions, bindings | a bun script implementing the device side with WebCrypto (`noise.ts`, run under Node 22) pairs with a headless harness through a local relay, calls `list_sessions`, subscribes, sends `send_input`, sees `TextDelta`s, uploads a PNG with `put_attachment` and gets it back with `get_attachment`; `snow ⇄ noise.ts` vectors both directions; revoke mid-stream → the script's socket closes and its next `IK` is refused; `Lagged` observed when the script stops reading |
| 3 | TUI `/link` menu + QR card; `t --link` | pair a second `theuth` (as the script) from the TUI card; rename and revoke from the menu; `t --link` against a headless daemon prints a scannable QR (phone camera reads the URL) |
| 4 | UI: `Transport` seam, `LinkTransport`, `noise.ts`, IDB keys, harness store, per-harness stores, `/link` route (paste), switcher, Settings › Link/Harnesses, `put/get_attachment` in every path, `save_attachment` deleted | Chromium at `localhost:1420`: paste a key, chat with a streaming tool call and an approval card, paste an image, switch to a second harness and back with no event bleed (the perf fixture from 25 §6.3 runs on harness A while B is visible: B's store receives nothing); vitest: `noise.ts` vectors, pairing-string parse, `resolveFile`, `methods.json` ⇄ `api.ts` |
| 5 | mobile accommodations (§8.1) | Chromium device emulation iPhone 15 and iPad: every §8.1 row visible; desktop screenshots of 25 §4.4's four screens byte-identical before/after at 1280×860 |
| 6 | iOS packaging (§8.3): `[lib]`, `cfg(desktop)` split, `tauri ios init`, scanner, deep link, AASA | on an iPhone over cellular against a harness at home through a self-hosted relay: scan from Settings › Link → chat → approve a `shell` call → switch harness → background 10 min → foreground reconnects; camera-app scan opens the app and pairs; revoke from the Mac → phone shows "revoked by Studio Mac" |
| 7 | browser container: `--web`, cookie, `/link#` landing, CSP | a phone without the app scans the QR, Safari opens the relay-served build, pairs, chats; DevTools: `localStorage` empty, cookie `HttpOnly`; an upgrade from the relay origin without the cookie → 403 |
| 8 | docs: 00 §10/§12, 25 §3/§7 amendments; README `## Link` (pair a phone, self-host a relay in four lines) | docs and tree agree |

## 13. Status (as-built v0, 2026-09-15)

Shipped: crates `relay` (`theuth-relay`, §5) and `link` (§6); `rpc` gains the
§6.4 methods, `LinkEvent` forwarding and the four exports; `theuth` gains
`--link` and the plugin wiring; `tui` gains `/link`. UI: `src/link/*`
(`transport.ts`, `noise.ts`, `keys.ts`, `idb.ts`, `harnesses.ts`,
`frames.ts`, `pairing.ts`), `src/rpc/transport.ts`, `store/harness-scope.tsx`,
`components/link/*` (`PairScreen`, `HarnessSwitcher`, `Harnesses`),
`settings/Link.tsx`, the §8.1 rules. Desktop: `[lib]` + `cfg(desktop)` /
`cfg(mobile)` split, `tauri.ios.conf.json`, `capabilities/mobile.json`.
`deploy/relay/` (Dockerfile, compose). Recipes: `just ios-init ios
ios-bundle relay web-build`.

Deviations from the sections above:

- **`link` does not depend on `rpc`** (§3 table, §6.2 "Methods object"): the
  dispatch arrives as a `SetDispatch { methods, acp }` message from
  `rpc::RpcPlugin::start` (32 §3.1); the `MethodsFactory` that `theuth/main.rs`
  first injected was removed as 00 §1.4 god-wiring.
- **`LinkPaired` is published before `LinkPairing { None }`** (§6.2 events):
  a listener sees the device before the pending row clears.
- **The harness's local label in the device store is never overwritten by
  `hello.name`** (§7.3: "`Info.name` … refreshes the stored name"); rename
  locally is the only writer.
- **Device-side Noise HKDF is HMAC-based** (§7.2: WebCrypto `HKDF.deriveBits`);
  byte-identical output, checked against `snow`.
- **`handshakeIK`/`handshakeIKpsk2` are two-step** (`message`, `finish`), not
  one call (§7.2).
- **TUI**: `Menu::Link { cursor }` with status in `Model.link`, a new
  `CardKind::Link` for the QR card, and `/link cancel` (§6.5 listed no
  cancel verb).
- **Mobile Settings deep link `/settings/<tab>` shows the tab list first**
  (§8.1 `SettingsDialog`: tab list → push); landing on the pane is an open
  nit.
- **`relay` adds a direct `rustls 0.23` dep with `aws-lc-rs`** (§3 table:
  "rustls (optional)"): `ServerConfig::builder()` needs a process provider
  for `--tls`.
- **`tauri-plugin-barcode-scanner` / `-deep-link` are direct deps in
  `crates/desktop/Cargo.toml`**, not workspace (§8.3).
- **Settings tab `Harnesses` (ACP) renamed `Agents`** — as §14 said; noted
  here because it lands in this series.

Verified (2026-09-15):

- Gates: `cargo check` / `clippy -D warnings` / `test` green across the
  workspace; `cargo check -p rpc --features ts`; `just ts-desktop` exported
  `LinkStatus`/`Device`/`Pairing`/`LinkEvent` and `methods.json` gained the
  nine methods; UI `tsc -b`, vitest 15 files / 89 tests, `vite build`.
- Live: `theuth-relay --listen 127.0.0.1:8787 --web crates/desktop/ui/dist` +
  `t --headless` with `link.enabled` → relay `/metrics uplinks 1`. `theuth
  --link` printed a scannable QR + key and `paired: Chrome` after the
  browser paired.
- Browser container: headless Chromium at 390×844 opened
  `http://127.0.0.1:8787/link#1.<…>`, paired (`IKpsk2`), URL hash stripped,
  landed in chat over the relay (`IK` session); `localStorage` empty,
  `document.cookie` empty (HttpOnly `__Host-link` set, CSP on).
- Device script (`crates/desktop/ui/scripts/link-smoke.ts`, bun): paired,
  reconnected with a persisted identity, `list_sessions` round-tripped;
  revoke closed its conn and the next `IK` was refused.
- Relay restart: harness re-registered in < 4 s, browser reconnected in
  < 3 s — after a fix: the relay's per-start cookie secret 403'd retries;
  the transport now re-fetches `/` for a fresh cookie when an upgrade is
  refused.
- TUI on a real pty: `/link` menu; `p` → QR card with countdown; device
  paired from the script → card `paired: tui-phone`, header badge `link ●1`,
  menu row with presence. Settings › Link and the harness switcher rendered
  on the phone viewport.

Not exercised: iOS build (`cargo tauri ios init` not run); camera scan;
universal link; real cellular; `send_input` streaming over link with a live
model (no API key in the scratch dir; subscription events covered by vitest
against a fake harness); the §11 bench numbers (`crates/relay/examples/relay_bench.rs`
exists, not run); `--tls`; Docker image build.

Open: §11 bench numbers; the mobile Settings deep link landing on the pane.

- 32 (nodes) amends §4.1, §4.3, §6.2, §7.3, §7.5; the `SetDispatch` message
  now also carries `acp`.

### 13.1 Context removal over Link (48), as-built 2026-09-18

Nothing here changed on the wire. `context_items`, `remove_context`,
`load_session(settled_only)` and `AgentEvent::ContextChanged` are ordinary
`Api` methods and bridged events (25 §7 addendum), so they reach a paired
device through the existing Noise channel byte for byte — no new op, frame
type or transport, and the relay learns nothing new. What the device side
had to get right:

- **Reconnect invalidates loaded state.** A `LinkTransport` reconnect
  (`PRESENCE 0 → 1`, iOS foreground, `Lagged`) runs `App.reload(st)`:
  `resetLagged()`, `unload()` — every slice's `historyGeneration` bumps and
  `contextStale` is set when it had been loaded — then the visible session
  is refetched. A page or settled snapshot requested before the gap lands
  under an older generation and is discarded (`sessions.ts`, `loadHistory`
  refusal). A `ContextChanged` missed during the gap therefore cannot leave
  a removed image on screen, and a control rendered from pre-gap history
  cannot fire: its pinned revision no longer matches.
- **Harness switch invalidates the same way** (§7.4 addendum): switching
  away unloads; an in-flight `remove_context` completes into the harness
  that issued it or is dropped, never into the newly active one.
- **Stale targeting is refused, not retried.** Every control pins the
  `contextRevision` its transcript was loaded at; the review dialog rechecks
  the slice's revision, `running` and `contextStale` before the call, and
  the server rechecks `expected_revision`. A refusal or a transport error
  with unknown outcome (`-32300`, socket not `open`) shows the message and
  offers to reload the inventory; the destructive RPC is never replayed
  automatically. On stale, the user reviews again from fresh data — no
  retargeting by position.
- **Attachments.** Previews for stored binaries come through
  `get_attachment` (§6.4 note), so a phone sees the same removable chips
  as the desktop for an image sent from any client, after resume.

Verified by Main 2026-09-18 in the shared React surface: a second HTTP
client's removal converged on a 390×844 coarse-pointer viewport through
`ContextChanged`; an open confirmation was refused after a remote removal;
switching the active harness to an isolated offline scope, mutating the
original harness while unsubscribed and switching back reloaded the masks
with nothing restored. That exercised the per-harness store lifecycle and
the invalidation rules above against `ws://127.0.0.1` transports — **not**
a Noise/relay pairing, a real phone or the iOS container; those remain in
§13's "not exercised" list.

### 13.2 Unified search over Link (49), as-built 2026-09-18

Nothing new on the wire again: `search`, `search_refresh`, `get_artifact`,
`artifact_search_match` and `knowledge_read_revision` are ordinary `Api`
methods, so a paired device runs the same four-category search the desktop
does and the relay sees ciphertext. The dialog state (chips, last view for
Back to results) lives in the per-harness scope of §7.4 and is dropped on a
harness switch like every other slice; ready groups arriving for a harness
that is no longer active are discarded. `search_sessions` no longer exists:
an older bundled UI or browser build against a newer harness sees Method not
found and must be updated together with it. A search and a subsequent
historical-concept/artifact read were exercised through an encrypted Link
channel in the implementation smoke; a real phone, the iOS keyboard and the
App Store container remain in §13's "not exercised" list.

### 13.3 Agent communication (50), as-built 2026-09-18 — pending verification

50 §11 asks this doc for "parity tables, settings/events and reload behaviour
over the same existing console transports; no new link wire mode". As
implemented, with nothing yet exercised through a Noise/relay pairing:

- **No new wire mode.** `IK` message-1 payload is still `mode:u8 | empty`
  with modes `0` (Api) and `1` (ACP, 32 §4.2). A2A is HTTPS between the
  `a2a` plugin and an independently operated agent; it does not ride the
  relay, is not proxied through a device's conn, and a device cannot reach
  a peer's A2A endpoint through the harness. The relay learns nothing new.
- **Same console methods over link.** The §9 row above is `Api` through
  `raw_json_request`; the device side reuses the shared React surface's
  communication queries (25 §12.2) with the per-harness `QueryClient`, so
  keys carry no harness id and switching harnesses unloads them like every
  other store (§7.4). A `LinkTransport` reconnect invalidates
  `['communication']`, `['peers']`, `['peer-messages']` and `['a2a']` in the
  same handler as sessions/knowledge (§13.1's rule, extended).
- **Owner-only from a device.** A paired device is the owner (§10): it may
  flip the global/session switches, set root scope and call `reload_a2a`.
  Those are console methods, not model tools, and the events they raise are
  owner subscriptions — never forwarded to an A2A client (50 §10).
- **Credentials stay apart.** Link identity, `devices.yaml` and the console
  token authenticate *devices*; A2A peers get their own random vault
  secrets and see none of the former. Nothing here syncs credentials over
  link (32 §15 row 3 still open).

Not verified: the communication pane over a real relay pairing, a phone
toggling while a desktop watches, iOS; the §13 "not exercised" list is
unchanged. User visual approval is owed.

## 14. Amendments (same commit series)

- **00 §7**: `raw_json_request` as the second in-process consumer of
  `rpc::Api` beside the TUI; `put_attachment`/`get_attachment` as `Api`
  methods; `rpc::bridge::start` idempotent (registered `"rpc.events"`).
- **00 §10**: `link/  plugin: relay uplink, Noise pairing and sessions,
  device registry (31)`; `relay/  bin: theuth-relay, stateless encrypted-frame
  relay (31)`; dependency order `runtime ← link ← signal ← rpc ← tui ← theuth`;
  `signal::qr` moves to `link::qr`.
- **00 §12**: row 19 `link (31)`, with §12 here as its check column.
- **25 §0.4**: still true (the webview owns the socket) — now through a
  `Transport`. **25 §3**: the "no TUI counterpart" table gains "connect to
  another harness: yes (31)". **25 §7**: "bytes upload: not added, same
  host" is reversed by §6.4 here; `save_attachment` is removed from
  `commands.rs` and §4.2's file list. **25 §5**: unchanged for macOS; §8.3
  here is the iOS sibling. Settings tab "Harnesses" (ACP) is renamed
  "Agents" so "Harnesses" can mean theuth harnesses (`TABS`, `SettingsTab`,
  `slack.rs`/`slash.ts` references, 06 §4 wording).
- **09 §11**: `qr` now imported from `link`.
- **`docs/plugins.md` §1 table**: a row for "a network peer that
  authenticates itself with keys and proxies `Api`" → plugin (`link`), and
  a note that `raw_json_request` is how a plugin serves `Api` to a socket it
  owns.

## 15. Rejected

- **A stateful hub with accounts, tenants, pairing records and presence in
  Redis** (jarvis): every row exists to authenticate someone the hub can
  read. With E2E the hub cannot read anything, so it has nothing to
  authenticate. §1.1.
- **Cross-relay mesh or pub/sub**: LB path hashing co-locates a harness's
  sockets with zero coordination; the failure mode (mis-hashed LB) is
  visible in `/metrics`. §5.5.
- **Ed25519 identity + X25519 for Noise**: a second key type to sign a
  registration nonce; `Noise_NK` proves the same with the key we have. §4.2.
- **`Noise_KK` for sessions**: the responder needs the initiator's static
  before reading message 1, so either every paired static is tried or the
  device announces itself in the clear to the relay. `IK` sends it encrypted
  and shares message-1 layout with the pairing pattern. §4.3.
- **Owner-confirm click on pairing**: proves possession of the QR, which the
  PSK already proves. Optional `link.confirm` kept for the paranoid. §10.
- **Passkeys / community account**: the console is single-user; a harness is
  an identity; there is no server to hold an account. §1.1.
- **`theuth://` custom scheme in the QR**: cannot be opened by a phone without
  the app; a URL with a fragment serves every path and hides the secret.
  §4.4.
- **Loopback WebSocket from the link plugin to the rpc server** (jarvis B):
  a second socket, the token in memory, one more copy per frame;
  `raw_json_request` is the same dispatch without any of it. §6.2.
- **Rust-side Noise in the Tauri app with RPC over Tauri IPC**: 25 §0.4's
  rejection stands (double serialization of every delta), and the browser
  container has no Rust. WebCrypto covers every container with one
  implementation. §7.2.
- **ChaChaPoly / BLAKE2s suite**: needs a JS cipher library; `AESGCM_SHA256`
  is native everywhere and hardware-accelerated on phones.
- **Private key in a cookie or in `localStorage`**: a cookie cannot be used
  by script (so it cannot do DH) and `localStorage` is readable by any
  script on the origin. Non-extractable WebCrypto key + HttpOnly session
  cookie is the only shape where nothing readable holds a secret. §7.6.
- **`permessage-deflate` / zstd**: ciphertext is incompressible;
  compress-before-encrypt is deferred until §11's cellular number asks for
  it.
- **One multiplexed device socket for all harnesses**: defeats path hashing
  (§5.5) and tells the relay which harnesses one device pairs with. One
  socket per harness costs nothing measurable.
- **Push notifications via the relay**: the relay cannot know when to push.
  §8.2.
- **A SwiftUI/native iOS app**: 25 §4.1's reasoning; the UI already runs in
  WKWebView.
- **Session resumption / 0-RTT**: 1 RTT per reconnect is the price of
  immediate revocation and forward secrecy per connection.

## 16. Open

| # | question | default |
|---|---|---|
| 1 | default `link.relay` hostname | `wss://relay.theuth.io`; the operator of that host decides; self-host is one binary |
| 2 | `curve25519-dalek` fork under presage vs `x25519-dalek` 2 (snow's default resolver) | spike in step 2; if features do not unify, snow's `ring-resolver` |
| 3 | `link.confirm` (Allow prompt on the harness) default | off |
| 4 | record batching per tick | decided by §11's cellular measurement |
| 5 | Android | nothing here is iOS-specific except §8.3; `tauri android init` is the same shape; not in scope until asked |
