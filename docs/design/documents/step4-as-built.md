# 59 — As built, step 4: link devices (2026-09-21)

Step 4 of `59-documents.md` §14 on `feature/documents-link` (worktree off
`feature/documents` 97df6b2 = steps 1–3): 8fc8d49 (desktop: chunked open
with `sv`, IndexedDB queue + replay, reconnect), 026cba4 (document:
awareness rate cap), be2a034 (tests + two-device script), this file. §17–§19
of 59 stand. Where §6 and this file differ, this is what exists. Kept out of
`59-documents.md` on purpose: the sibling worktree (step 5) appends there.

## 1. What was built

**`HarnessProvider` (`crates/desktop/ui/src/lib/doc/provider.ts`).** Takes
the harness scope id (`useScope().id`: `'local'` or a link harness id, 31
§7.4) as a fourth argument, from `useDocument` through `acquire`. On a link
device (`harness !== LOCAL`) it owns a `DocStore` (§2); in-process the queue
stays in memory as in step 1. Start: `DocStore.load()` → the persisted state
applied with origin `'restore'` (never echoed), the persisted FIFO becomes the
queue; if the client is not open at that point the status goes
`offline(queued)` at once so what IndexedDB had is editable without waiting
for a reconnect. `#sync` (on every `open`, and on `Closed{slow}`): wait for the
store, `doc_subscribe` once, `doc_open(sv = encodeStateVector(doc))`, join the
`state` chunks (slices of ONE v1 update, §4.7) and apply them as one
transaction, then **flush rounds**: `splice` the queue, send it as one
`Y.mergeUpdates`, `ack` the rows; repeat while anything was typed during the
round; only then `live`. A failed round puts its batch back at the head and
the status returns to `offline(queued)`. Every local update is enqueued in
the store *before* it is sent; a `doc_update` reply acks its row, a rejection
(socket died with the request in flight) queues it. The whole Y.Doc is
written 1 s after the last change (any origin) and on `destroy`. `queued`
getter for hosts.

**`DocStore` (`lib/doc/queue.ts`, new).** IndexedDB db `documents`, two
stores: `state` (key `<harness>/<doc>` → `encodeStateAsUpdate` blob) and
`pending` (key `[harness, doc, seq]` → one v1 update). One row written per
keystroke, one deleted per ack; `load()` reads both with a bounded key range.
`seq` starts at `Date.now() × 4096` per instance so two tabs of one origin on
the same document never write the same key and an older run's rows sort
first; after `load()` it is at least `max(seq) + 1`. Bound `MAX_BYTES` = 4
MiB per document over state + pending: `saveState` beyond it deletes the
state blob (the harness has it) and `console.warn`s; pending rows are never
dropped. No localStorage (31 §0.8).

**Awareness (§8.5, §6.2).** Editor user = `deviceName()` (`Mac`, `iPad`,
`iPhone`, `Chrome`…) with a colour from a six-entry palette indexed by the
Yjs client id. Actor: `AWARENESS_RATE = 4` states per second per connection
(`RateSlot{since, sent}` per `Option<conn>` on `Open`, fixed one-second
windows); the excess returns `Ok(())` and is neither applied nor relayed
(`trace!`).

**Status line.** Unchanged from step 1: `offline · N queued` now counts the
persisted FIFO; during a flush round the count is what was typed after the
round started.

**Tests.** `lib/doc/provider.test.ts` (vitest, `fake-indexeddb` dev dep): the
FIFO survives a reload in order and acks delete rows; the 4 MiB bound drops
state and keeps pending with one warning; `sv` round-trip against a real
`Y.Doc` authority answering in 8-byte chunks (the request carries the
restored state's vector, the reply is several chunks, applied as one
update); offline edits replay as one merged update and are acked; a
destroyed provider's persisted FIFO replays on the next open; an update
typed while the flush is in flight goes in a second round, never twice; a
failed flush keeps the FIFO; a local provider opens no IndexedDB.
`actor::tests::awareness_cap_admits_four_per_second_then_resets`.
`scripts/doc-link-smoke.ts` (§4).

## 2. Transport findings (section A)

Everything worked over mode 0 with no change to `crates/link`,
`crates/relay`, `src/link/**` or `rpc/client.ts`:

- `doc_subscribe` notifications reach a device: the conn task drains the
  subscription's `mpsc` into records like `subscribe_events` (31 §6.2). Two
  devices and the in-process client on one note all received each other's
  `Update`, `Awareness`, `Exported`, and the `DocumentChanged` bus event.
- Fan-out skips the sender per connection (`Principal::Owner{device:
  Some(conn)}` from `RpcPeer::id()`): the typing device receives its
  `doc_update` reply, `Exported` and `DocumentChanged`, not its own update.
- A JSON-RPC reply larger than one Noise message is split into records and
  reassembled: the 200-block `doc_open` (≈ 82.0 KB of JSON) arrived as two
  `DATA` frames of 65 538 and 16 503 bytes and rendered 199 paragraphs.
- The 48 KiB `state` chunking (36 KiB raw → 49 152 base64 chars per chunk) is
  independent of the 65 518-byte record limit; both applied and neither had
  to change. Note: the actor has no 48 KiB refusal of a client update (§4.7),
  only the 8 MiB cap — a merged offline flush larger than one record simply
  spans records.

## 3. Measurements

Method: Chrome DevTools Protocol `Network.webSocketFrame{Sent,Received}` on
each headless Chromium page (device sockets carry binary frames: the sizes
below are ciphertext + 1 `op` byte; the in-process Vite page carries the
plaintext JSON, so the same event seen there gives the plaintext size).
Device frame = plaintext + 1 flags + 16 AES-GCM tag + 1 op; the relay adds
a 4-byte conn id on the uplink. Load average 30–50 during the run.

| what | plaintext JSON-RPC | device socket frame | notes |
|---|---|---|---|
| keystroke, device → harness: `doc_update` request | 114 B | 132 B | Yjs update 18 B raw → 24 B base64 (+33 %); envelope ≈ 90 B; 26-char doc id |
| its reply `{"result":null}` | 39 B | 57 B | |
| keystroke, harness → other device: `Update` notification | 169–171 B | 189 B | subscription id 16–20 digits |
| per settled edit (export ≥ 400 ms debounce): `Exported` | 114 B | 132 B | to every subscriber incl. the sender |
| per settled edit: `DocumentChanged` (subscribe_events) | 236 B | 255 B | to every device with the events subscription |
| awareness with cursor, device → harness | ≈ 474 B | 489–492 B | `{user, cursor:{anchor,head}}` as JSON in base64 |
| awareness with cursor, harness → device | ≈ 506 B | 522–525 B | |
| awareness without cursor (`cursor: null`) | 214 B | 234 B | |
| awareness rate: 12 cursor moves in A within 1 139 ms | 12 sent | 5 relayed | 4 in the first window, 1 in the next; the cap works, remote cursor stale until the next state or the 15 s y-protocols renewal |
| `doc_open`, 200-block note, cold | ≈ 82.0 KB | 65 538 + 16 503 = 82 041 B | state 61 171 B raw → 2 chunks (49 152 + 32 412 b64); helper `bytes` in the index 0 until export |
| `doc_open(sv)` after a reload (state restored from IndexedDB) | ≈ 525 B | 543 B | `state: []`; whole reconnect incl. handshake, hello, `info`, two subscribes: 2 391 B inbound |
| offline flush of 51 updates (Enter + 50 chars) | | 1 245 B out | one merged update |
| latency A → B (device → relay → harness → relay → device) | | 23–38 ms | 5 keystrokes, polled at ~10 ms |
| latency A → Vite (in-process) | | 22–38 ms | same keystrokes |
| B reload with 50 pending: restore + `doc_open(sv)` + flush → `live`, all three identical | | 978 ms / 979 ms | from `reload()` start; page load 247 ms of it |
| B socket cut, 10 chars, restore → `live` | | 6 995 ms | the `RpcClient` backoff slot (0.5→10 s); the provider itself converged the same instant |
| `bun scripts/doc-link-smoke.ts --chars 50` | | 2.4 s total | A→B,L 19 ms; 135 B out per keystroke; 50 queued; B live 305 ms after restore; authority agrees |

IndexedDB on device B after the offline burst: `pending` 50 rows, `state`
827 B for the small note (61 171 B for the 200-block one); 0 pending rows
after the replay.

## 4. Convergence check (section F) — what was run

Harness `t --headless` on `127.0.0.1:7801` (`THEUTH_DATA_DIR=/tmp/theuth-59-4`,
`THEUTH_SECRETS__KEY_STORE=file`, `THEUTH_LINK__ENABLED=true`,
`THEUTH_LINK__RELAY=ws://127.0.0.1:8431`, `THEUTH_LINK__NAME="Studio Mac"`),
`theuth-relay --listen 127.0.0.1:8431 --web crates/desktop/ui/dist` (the
`vite build` bundle, 31 §7.6), Vite dev on 1431 with `VITE_THEUTH_URL/TOKEN`
as the in-process UI. Two headless Chromium **browser contexts** (separate
IndexedDB, separate device keys) paired through `link_pair` →
`http://127.0.0.1:8431/link#1.…` (`IKpsk2`; `link_status` listed two `Chrome`
devices online). Note `agqmmam62fzpdi5jaiikh63n44` open in A, B and Vite.

1. Type in A → B and Vite show it in 23–38 ms (table). Remote cursors with
   labels render on the device (`/tmp/theuth-59-smoke/step4-device-b.png`:
   two `Chrome` cursors, teal and orange).
2. Cut B: `window.WebSocket` wrapped in B's main world (CDP
   `Runtime.evaluate`) to record live sockets and, while `__offline`, hand
   the transport a socket that never opens; live sockets closed. Footer
   `offline · 0 queued` 4 ms after the close; the switcher showed
   `reconnecting`.
3. B: Enter + 50 chars → `offline · 50 queued`; IndexedDB `pending` 50,
   `state` 827 B; the transport's reconnect attempts hit the dead socket
   (backoff 0.5, 1, 2, 4, 8, 10 s). A meanwhile typed ` (A live)` into the
   heading; Vite followed A, B did not.
4. B reloaded (the killed-app path — see §5 item 8 for why): the editor
   showed B's offline text *and* A's heading edit on the first poll (restore
   from IndexedDB, `doc_open(sv)` diff), `live` at 978 ms, A = B = Vite at
   979 ms, `pending` 0, `note.md` = the three editors' block text
   (`# Link smoke (A live)` / `first paragraphabcd` / `B-offline …`).
5. Second cycle without reload: cut, 10 chars in B (` +offline2`) and
   ` +A2` in A at the same position of the same paragraph, restore →
   `live` 6 995 ms later (a backoff slot), all three
   `first paragraphabcd +A2 +offline2` at that instant.
6. Agent chip over link: `send_input` "Add one bullet 'Agent was here'…" →
   B's footer chip `theuth` from +4.88 s (edit start) to +6.90 s (2 s after
   the update), the bullet attributed `data-authored="theuth · 11:22 PM"`,
   `note.md` gained `* Agent was here`.

Client-side markdown exports were not compared (the BlockNote editor
instance is not reachable from outside the page); block text of the three
editors and the harness's derived `note.md` were.

## 5. Deviations and observations

1. **Names (§8.5).** No owner-name setting exists in the UI or `Info`; the
   awareness user is `deviceName()`. Two Chromium clients both read `Chrome`
   (colour tells them apart); on real hardware it is `Mac` vs `iPad`. The
   per-device suffix when two devices share a name is not implemented.
2. The offline flush is one merged update per round, not one `doc_update`
   per FIFO entry (as in §17 deviation 9): one round trip for 2 minutes of
   typing; the FIFO's rows are acked as a batch after the round succeeds.
3. `DocStore` keys use a time-based `seq` (see §1) rather than a counter
   from `load()`; the FIFO order is the key order.
4. The state blob is saved on every update of any origin (1 s debounce), not
   only local ones, so a reload has the newest harness state too.
5. `fake-indexeddb` is a new dev dependency of `crates/desktop/ui`
   (`package.json`/`bun.lock`, outside the allow-list, required by the brief).
6. Bindings were copied from the step-3 worktree (`../theuth-documents`,
   same commit 97df6b2) instead of `just ts-desktop`; no Rust wire type
   changed in this step.
7. Rate cap semantics: fixed one-second windows per connection. A dropped
   state leaves the remote cursor where the last relayed state put it until
   the next state or y-protocols' 15 s renewal (which the provider forwards).
   A dropped *leave* (`null` state) would linger until the 30 s awareness
   timeout — accepted; the desktop's own y-prosemirror cursor traffic is
   capped the same way (it also goes through `doc_awareness` with a conn id).
8. Test-harness finding, not a product defect: my first socket wrapper in B
   nested three patches and its constructor threw; `LinkTransport.#open`
   then left `#pending` unsettled and the `RpcClient` never retried (that is
   why step 4 above reloaded B). A browser's `new WebSocket()` throws
   synchronously only for `SecurityError` (blocked port, insecure scheme
   from an https page — the relay build always uses `wss` there), so not
   fixed; if it ever matters, catch around the constructor in `#open` and
   route through `#lost`.
9. `meta` is not persisted: a note opened cold while the harness is
   unreachable is editable with its IndexedDB content, but the bar title
   (`meta.title`) is empty until the first `doc_open`. Not exercised.
10. The 48 KiB client-update refusal of §4.7 does not exist in the actor
    (only the 8 MiB cap); a merged flush over 65 518 bytes spans records.
    Left for step 8 (limits).
11. `RUST_LOG=link=debug` did not surface conn logs in `logs/theuth.log.*`
    (INFO file sink); the relay's `/metrics` and the devices' CDP frames
    were the instrumentation. No temporary tracing was added to the link
    layer.

## 6. Checks

| # | check | outcome |
|---|---|---|
| 1 | `cargo check --workspace` (1 m 48 s warm); `cargo test -p document` (16 unit incl. the cap test + 3 integration); `cargo clippy -p document --no-deps --all-targets -D warnings`; UI `tsc -b --noEmit`; vitest 37 files, 205 pass + 1 skipped (198 + 8 new); `vite build`; helper `bun test` 75 | all green (final run after be2a034, load ≈ 30) |
| 2 | A: pairing, `doc_*` over mode 0, sizes | §2–§3 |
| 3 | F: two contexts + Vite, cut/type/restore, convergence, `note.md` | §4 |
| 4 | `bun scripts/doc-link-smoke.ts --rpc ws://127.0.0.1:7801 --token … --chars 50` | exit 0, 2.4 s: `A→B,L: 19 ms`, `B offline: 50 queued`, `B live 305 ms … converged +0 ms; authority agrees: true` |

Environment: fresh worktree; `CARGO_TARGET_DIR` pointed at the idle step-3
worktree's `target/` (cold `cargo build -p theuth -p relay` 3 m 41 s at load
30–50); `bun install` in the three packages; `just documents-helper` (59.2).

## 7. Manual device pass (Sky, iPad/iPhone + Mac)

Prerequisites: harness with `link.enabled: true` and a reachable relay
(`link.relay`); the iOS app or a browser on the device; one note.

1. **Pair.** Settings › Link on the Mac → Pair → scan the QR (app) or open
   the `https://<relay>/link#1.…` on the device. Settings › Link lists the
   device online. *Look for:* device name (`iPad`/`iPhone`), `online`.
2. **Open the same note on both.** Notes → the note, on the Mac and on the
   device. *Look for:* footer `live` on both; the device's first open of a
   long note (a few hundred blocks) renders in one go — chunked `doc_open`;
   Safari's inspector (if attached) shows `documents` in IndexedDB with
   `state` and `pending`.
3. **Live editing.** Type on the Mac, then on the device. *Look for:* text
   on the other side within a blink (LAN: tens of ms; cellular: your RTT);
   remote cursor labelled `Mac` on the device and `iPad`/`iPhone` on the Mac,
   different colours; no duplicated characters.
4. **Airplane mode.** On the device: airplane mode on, wait until the
   footer reads `offline · 0 queued` (the switcher turns `reconnecting`).
   Type a paragraph. *Look for:* `offline · N queued` counting each
   keystroke; typing stays fluid. Meanwhile edit another paragraph on the
   Mac (and one shared paragraph, to see the merge).
5. **Kill the app while offline** (swipe it away), then reopen it still in
   airplane mode and open the note. *Look for:* your offline text is there
   and editable, footer `offline · N queued` with the same N.
6. **Reconnect.** Airplane mode off. *Look for:* footer `live` within the
   reconnect backoff (≤ 10 s after the network is back); the Mac's edits
   appear on the device and the device's paragraph on the Mac; the shared
   paragraph contains both edits; `cat <data_dir>/documents/<id>/note.md`
   on the Mac matches. Two minutes offline is the §14 row: repeat step 4
   with a 2-minute wait before step 6.
7. **Agent.** From the note's chat panel on the Mac ask for a bullet at the
   end. *Look for:* the device shows the `theuth` chip in the footer while
   the edit runs and the new block hover-attributed `theuth · hh:mm`.
8. **Cursor storm.** Hold an arrow key on the device for a second. *Look
   for:* the Mac's remote cursor moves in steps (≤ 4 states/s) and settles
   at the right place on the next move or within 15 s.

What to send back: the footer states you saw at each step, the reconnect
time in step 6, whether the shared paragraph merged, and any `console.warn`
(`doc sync failed`, `doc store unavailable`, `…exceed the 4194304 IndexedDB
bound`) from Safari's inspector.

## 8. Left (and noticed on the way)

- Owner name from Settings and the two-devices-one-name suffix (§8.5).
- `meta` in IndexedDB for a cold offline open (title, agent colour).
- `daily` snapshot (§17.6, still open); the 48 KiB update refusal and other
  limits (step 8).
- iOS specifics not exercised here: WKWebView IndexedDB eviction under
  storage pressure (the 4 MiB bound is ours, the OS may evict the whole
  origin — the FIFO is then gone and the harness's state wins), background
  socket closure and `visibilitychange` reconnect (31 §7.3).
- `LinkTransport.#open` should not leave `#pending` unsettled if
  `new WebSocket()` throws (deviation 8).
- The browser-context proxy cannot compare BlockNote's own markdown export
  per client; the device pass compares text and `note.md`.
