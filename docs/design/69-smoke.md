# 69 — end-to-end smoke: devices pair to the mesh, failover, documents on the hub

*Worker Z, 2026-09-25. Proves the twelve invariants of the Z brief on the integrated `feature/69-devices` branch (K, D, C2, M, Y, E1, E2, P merged at `d5222bd`), and records what had to change. Companion: `69-as-built-Z.md`. Extends `67-single-engine-smoke.md` and `68-two-engine-smoke.md`; every engine ran on this Mac.*

## 0. Result

| # | Invariant | Result | Evidence (§2) |
|---|---|---|---|
| 1 | Lone engine, `link.enabled`: engine id and mesh id at the relay; old engine-id pairing works | ✓ **after fix 1, changed shape** | engine id at boot; the mesh id registers 2 ms after the first `device_pair_start` and at every later boot; 3-part `link_pair` string pairs (`handshake_ok` +4 ms, first record answered) |
| 2 | `device_pair_start` on B (non-hub) → 4-part string; pending row on A ≤ 1 s; device lands on A; `devices_list` on both | ✓ after fixes 1, 5 | pending row on A within the second; A registers the mesh id 1105 ms after the RPC (the vault row replicated); device `paired`, `info.data_dir = A` at +18 ms; row on B within 3 s |
| 3 | Device opens a session, sends, gets a reply (owned by A) | ✓ | `send_input` over the link → assistant `pong` in 1148 ms; `session_owner` A `here: true`; transcript on B |
| 4 | SIGKILL A → B takes the lease, registers; device sees PRESENCE 0 → reconnects → IK against the same key → `list_sessions` → send works | ✓ | **gap 31.1 s** (QUIC dead-peer detection 31.1 s, lease +6 ms, register +12 ms, device handshake +6 ms); `session_take(user)` 3.3 ms then `pung` in 1145 ms |
| 5 | Restart A: no reclaim while B holds; exactly one register after B's; graceful stop of B → A registers within a heartbeat, device reconnects | ✓ after fixes 6–8 | zero relay registrations during A's restart; B `SIGTERM` → A registered **+42 ms**; device back in 1.0 s. Before the fixes: 5 registrations / 5 device re-handshakes in 7 s |
| 6 | Revoke on a non-hub → next handshake refused (`CLOSE 3`) on the hub | ✓ | revoke on A → hub B closed the conn +1.0 s → redial → `CLOSE 3` during the handshake → device `refused` |
| 7 | Documents: create on hub via the device; B's copy ≈ 2–3 s; edit → B updates; share link on B names the mesh id; guest through the hub; kill A → guest on B, equal content | ✓ | B's dir 2.9 s after create, identical sha; edit → B 12.7 s; URL id = mesh id; guest `editor` in 40 ms; handover + kill → guest re-joined on B in 2.35 s, markdown identical, `resnapshots 0` |
| 8 | Editor on the non-hub's local RPC → forwarded over `theuth/docs/1` → device on the hub sees it ≤ 1 s | ✓ | `doc_open` 7.5 ms, `doc_update` 0.9 ms via the leg; device `Update` notification **40 ms** later |
| 9 | Migration on a copy of `~/.theuth/documents` | ✓ | 36 live rows: 36 moves, 0 merges; `hlc` column added and stamped; 68 mirror files; every knowledge doc opens; second start 0 moves; original untouched |
| 10 | No replication storms (A, B, C + one device) | ✓ after fixes 3, 4, 7, 9 | (a) 306 s idle: 0 record batches, 0 mirror rows on every engine; (b) one edit: hub 4 batches, non-hubs 1 each, `resnapshots 0`, same on the second edit; (c) one pairing: one row batch per writer, flat for 60 s; (d) knowledge edit syncs one way, md sha stable 80 s after the hub's import |
| 11 | Peers follow the owner | ✓ | B → A `send_message(await_reply)` answered "42" in 3.3 s over `theuth/rpc/1`; `discover_peers` on B: alpha `remote`, `engine: A`; kill A → `failed {"OwnerUnreachable": …}` in 32.3 s |
| 12 | Byte-identical vs `main` on a lone engine | ✓ (one documented difference) | `list_sessions`, `link_status` minus `mesh`, `doc_open` state/markdown identical; `doc_list[].bytes` is `0` until the 2 s quiet snapshot (C2 contract §2), then `913` on both |

Nine fixes, each its own commit (`fix(<crate>): 69 smoke — …`), listed in §3. Every fix was forced by a failing invariant on this box, not by reading code.

## 1. Recipe

Binaries: `cargo build -p theuth --bin t -p relay --bin theuth-relay` in the worktree; `main`'s `t` (`~/Develop/theuth`, `29b73fe`, 1.16.0) built read-only. The documents helper: the branch worktree has no `packages/documents/dist`, so every engine ran with `THEUTH_DOCUMENTS_HELPER=~/Develop/theuth/packages/documents/dist/theuth-documents-59.5` (helper source identical on both trees, protocol 59.5).

Relay: `theuth-relay --listen 127.0.0.1:8787 --web ~/Develop/theuth/crates/desktop/ui/dist` with `RUST_LOG=info,theuth_relay=debug` — the DEBUG line `uplink registered id=…` per registration is the flap evidence.

Config per engine (`/tmp/69-z/<X>/config.yaml`, `mk.sh <name> <bind port> <hub pref>`):
```yaml
secrets: { key_store: file }
mesh:
  relay: none
  discovery: true
  name: A
  bind: 127.0.0.1:7941     # several engines on one box (68 fix 3)
  hub: prefer              # A prefer / B eligible for inv 2–4; A eligible from inv 5 on; C eligible
  probation_secs: 0        # one box, fresh engines — the 6 h newcomer guard would keep two leases
  handover_secs: 0
link:
  enabled: true
  relay: ws://127.0.0.1:8787
  name: A
agent: { model: anthropic/claude-haiku-4-5, title_model: anthropic/claude-haiku-4-5 }
slack: { enabled: false }
signal: { enabled: false }
```
Start: `THEUTH_RPC__LISTEN=127.0.0.1:7931 t --headless --data-dir /tmp/69-z/A` (A :7931, B :7932, C :7933, lone branch engine :7921, `main` :7923, migration copy :7924); `bun pair.ts A 7931 B 7932` (`mesh_pairing_string` → `mesh_add`). `link.relay` accepted `ws://127.0.0.1:8787` unchanged.

**The device is a bun script**, not headless Chromium (the brief's fallback; chosen for exact timestamps of every relay frame): `/tmp/69-z/device.ts` imports the UI's own `noise.ts` (IK / IKpsk2 on WebCrypto), `frames.ts`, `keys.ts` (persisted X25519 key → the same device across restarts) and `guest.ts` (the §8.3 join proof), plus `yjs` and `packages/documents/src/convert.ts` (BlockNote-shaped edits via `convert.edit`). Modes: `--pair <4-part string>` (IKpsk2 with `PairHello.pending`, first record `info`), `--mesh <mesh device id>` (IK session), `--guest <share url>` (IK as an unknown static → `doc_join`). Behaviour mirrors `transport.ts`: `PRESENCE 0` parks, `CLOSE r` closes and redials after 1 s, `CLOSE 3` during a handshake is sticky `refused`; like `App.loadInfo` it sends `info` on every open (a silent session is closed by the harness after the 10 s handshake deadline). A control HTTP port takes `POST /rpc {method, params}` (JSON-RPC over the link), `POST /doc/open|edit|md`, `GET /log` (every frame with ms/ISO). Helpers: `rpc.ts` (HTTP JSON-RPC with `<dir>/token`), `edit.ts` (an editor on an engine's local RPC), `mdof.ts` (a `snapshot.yjs` → markdown, the "read on B" of a replica), `cmp.ts` (inv 12), `counters.sh` (inv 10: `mesh_status.replica`/`.traffic`, per-origin `MAX(seq)` of `replica.sqlite.outbox`, `doc_files` rows, `doc_stats`).

## 2. Evidence per invariant

### 2.1 Lone engine
On `d5222bd` the relay logged `uplink registered` for the engine id and 1 ms later for the mesh id; `link_status.mesh = {device_id, connected: true}`. Then A and B, each run alone first, each minted its own `mesh/device_key`: after pairing A served `SAODMY9…` while B held `Ceg1D3…`, and A logged `replica: vault pk mesh/device_key#1: NOT NULL constraint failed: vault.name; changes skipped` — the row had been written before the vault attached to the replica, so only the pairing re-seal's `ciphertext` column ever shipped (fix 1). After fix 1: boot → `link_status = {connected: true, mesh: null}`; `device_pair_start(null)` → 4 parts, pending `qNJbh14igs9Skfem4pKIhg`; log `mesh uplink starting … id=9RIOHkdC…` inside the `DevicePairStart` handler → `registered` 2 ms later; `link_status.mesh.connected = true`. Old flow: `link_pair` (3 parts) → IKpsk2 → `handshake_ok` +4 ms → `info` answered +2 ms → `B7NziiI3 oldphone bun` in `link_status.devices` and `devices_list`.

### 2.2 Pairing shown on B, device lands on A
A `prefer`, B `eligible`, both agree holder A. `device_pair_start(null)` on B → `{pending_id: 2nCvacHZf8hPlLS4SzrnBg, parts: 4}`, name "B" (`mesh_name`). A: `mesh uplink starting … MeshIdentityLoaded` → `registered` **1105 ms** after the RPC returned (the vault row replicated → `SecretsEvent::Changed` → reconcile); pending row on A within the same second. Device (IKpsk2 + pending hint): `handshake_ok` +16 ms, `paired` +18 ms with `info.data_dir = /tmp/69-z/A`. `devices_list` on A and B: `Eu7TXMbV phone bun` (B ≤ 3 s); the consumed pending row gone on both.

### 2.3 Session through the device
`new_session` → `send_input(sid, "Reply with exactly the word pong.", [], null)` over the link; `load_session` polled at 500 ms: `[user, assistant "pong"]`, `running: false` **1148 ms** after send. `session_owner` A `{epoch 1, here: true}`, B `here: false`; B's `load_session` shows the same 2 rows.

### 2.4 SIGKILL A — gap 31.1 s
`kill -9 A` 14:29:46.786. Device `CLOSE 4` at .822 (+36 ms), redial → `PRESENCE 0` (parked). B `mesh: disconnected aeb54621` 14:30:17.929 (**31.1 s**: QUIC dead-peer detection) → `lease → B (disconnect)` +6 ms → `hub lease gained` → `mesh uplink registered` .941 (+12 ms) → device `PRESENCE 1` .942 → `handshake_ok hello.name=B` .947 against the same mesh key: `gap_ms 31125`. `info.data_dir = /tmp/69-z/B`; `list_sessions` via the link shows the session `{owner: A, epoch 1, here: false}`. `send_input` → `-32010 OwnerUnreachable … takeover after 300s` (68 §5, as designed); the UI's `OwnerOfflineCard` path `session_take(sid, "user")` → `{owner: B, epoch 2, here: true}` in 3.3 ms → `send_input` → `pung` in 1145 ms.

### 2.5 Restart A, then release B
Before fixes 6–8 (A `prefer`, binary `8a0a280`): A restarted at 14:31:53 and elected itself over `{A}` in 0.3 s, registering the mesh id before it had reconnected to B; the relay replaced the holder 5× in 7 s (A 53.567, B 54.680, A 55.669, B 57.976, A 60.011), the device re-handshook 5×. After: A (`eligible`) restarted 14:50:35 → `mesh: lease → None (start: 1 stored peer(s), waiting for one to connect)` → link `hub lease lost` (no dial, `LinkStatus.mesh = null`) → peer connected → `lease → B (elected)` +3 s. Relay: **zero** `registered id=F9KN…` lines between B's (14:44:35) and 14:51:26; both devices saw no `close`/`presence` event. Release: `SIGTERM B` 14:51:25.998 → A `lease → A (disconnect)` 26.037 → `mesh uplink registered` 26.040 (**+42 ms**) → devices `CLOSE 4` (B's socket dropped) → redial → `hello A` 27.044, guest re-`doc_join`ed; gap 1.01 s.

### 2.6 Revoke
`device_revoke("Eu7TXMbV")` on A (non-hub) at 14:53:35.859 → hub B's registry refresh cancelled the live conn: device `CLOSE 0` at 36.858 → redial → IK msg1 → `CLOSE 3` during the handshake at 37.871 → `refused`; `devices_list` on B `revoked: true` (row kept).

### 2.7 Documents
1. Device → hub `doc_create("Smoke doc", "# Smoke\n\nhello from the device\n")` → `agqnsbkonr3afhbrpwggsf5azy` in 80 ms.
2. B: `documents/<id>/{meta.json,snapshot.yjs}` present **2865 ms** later; `mdof.ts` on A and B: sha `5741c7d31cae0d49`, 505 B, identical markdown; B's replicated `doc_list` row `bytes 505`.
3. Device `doc_update` (convert.edit insert, 243 B): B's snapshot changed after **12.7 s** (2 s quiet snapshot + the file mirror's cadence), sha `02a26bb7c08e5ebf` on both; `doc_stats` A `{merged 0}`, B `{merged 2, resnapshots 0}`.
4. `doc_link_create(doc, editor, "guest link")` on **B** (forwarded to the hub) → 56 ms → `http://127.0.0.1:8787/d#1.F9KNAsIp…8XQ.<doc>.<secret>` — the mesh device id.
5. Guest (fresh key, IK against the mesh id → guest conn on A) → `doc_join(proof)` → `{role: editor}` in 40 ms; `doc_open` → the same markdown; a non-guest method → `-32601`.
6. `mesh_set_hub(eligible)` on A made B outrank it → **graceful handover**: A `CLOSE 5` to both device conns 14:44:33.185, devices parked, B `lease → B (handover)` 35.525, registered .528, devices `hello B` .532 (**2.35 s**), guest re-joined (editor) .534; `kill -9 A` at 35.25. Guest `doc_open` on B: 746 B state, markdown identical, no duplicated blocks.

### 2.8 Non-hub editor forwarding
Hub B (after its restart the 30-day availability ranking put it first), editor on A's local RPC: `doc_open` **7.5 ms**, `doc_update` 0.9 ms (A's `traffic`: `theuth/docs/1 frames_out 3 / in 1`); the device subscribed on hub B received `Update{by: owner}` **40 ms** after the send and applied it.

### 2.9 Migration
Copy of `~/.theuth/documents` (44 rows, 36 live: 24 artifact, 9 knowledge, 3 meeting_note; no `hlc` column). Lone engine on the copy: **36 `documents: id converged from=<v7> to=<v5>`, 0 merges**; `docs.hlc` added and all 36 live rows stamped; `mirror/` 68 files (36 meta + 32 snapshots); `doc_list(knowledge)` = 9, each `doc_open`s (1 chunk, title/binding intact); second start: 0 moves; `~/.theuth/documents` untouched.

### 2.10 No storms — A, B, C, one device, one guest
Counters: `mesh_status.replica`, per-origin `MAX(seq)` of the outbox (= record batches emitted), `mesh_status.traffic` per ALPN, `doc_files` rows, `doc_stats`. Gossip heartbeats ride iroh-gossip's own connections (not in `traffic`): 1 per 10 s per engine. Knowledge bundle sync (`theuth/knowledge/1`): one content-free exchange per ~38 s per peer pair (+16 frames in/out per engine per 5 min).
- (c) pairing via C's QR (device landed on hub B): C +1 batch (pending), B +1 (device row), A +0; then **60 s flat** on every counter; 3 device rows / 0 pending everywhere.
- (a) idle 15:04:19 → 15:09:25: record batches **0** on every engine (outbox seqs 95/49/84 unchanged; captured/applied/relayed unchanged; `echo 0`); `doc_files` rows 2 → 2, same max hlc; records frames unchanged.
- (b) one `doc_update` on the hub: quiescent within 40 s, identical at 70 s. Hub **4 batches** (8 column changes: `docs` row + 2 `doc_files` rows), non-hubs **1 batch each** (2 columns: the `docs` row re-touched after adopting), `merged +1` on each non-hub, **`resnapshots 0`** everywhere, `doc_files` rows still 2. Second identical edit: same document counters (`merged 2/0/2`, `resnapshots 0`), snapshot sha `20bf01cbcf9967ee` on all three; its batch count was confounded by a lease handover B→C (C's availability bucket outranked B — 67 §4.5 with `handover_secs 0`; devices `CLOSE 5`, back in 5.4–5.9 s).
- (d) `knowledge_write facts/smoke-concept` on hub C → B `1 files synced from C` +2:00, A +2:09; git heads: C `write …`, A and B one `sync from C` each, no back-sync; md sha `07156506ed7c9081` on all. `doc_resolve({knowledge})` on the hub imported the document → mirrored to A and B ≤ 20 s (snapshot sha `8fb869f307b9b7c2`), hub 4 batches, non-hubs **0**; **+80 s: md sha, snapshot sha and knowledge frames unchanged** on every engine.
No counter climbed while idle.

### 2.11 Peers follow the owner
`list_peers(SB)` on B: alpha `{state: remote, engine: A, send: true, reply: true, activity: null}`. B's turn: `send_message(session:<SA>, await_reply)` → A's session ran on the peer message and replied with `reply_to` → B's model reported **"42"** verbatim; 3.3 s; `theuth/rpc/1` on B 2 frames in / 2 out. Exchange rows share ids on both engines (`replied`/`consumed` on B, `replied`/`accepted` on A). `kill -9 A`, resend → receipt `failed`, error `{"OwnerUnreachable":{"owner":"aeb54621…","last_seen":…}}` in **32.3 s** (the 30 s forward timeout).

### 2.12 Byte-identical vs `main`
Fresh lone engines, identical configs: `main` (:7923) vs the branch (:7921). `cmp.ts`: `list_sessions`, `link_status` minus `mesh`, `doc_list`, `doc_create`, `doc_open` (meta + state decoded to markdown — `doc_read` is an agent tool over the same `Markdown` path; there is no RPC of that name), `doc_list`, `doc_snapshots`, `documents_status`, `list_sessions`, ids/times normalised. `diff`: `link_status.name` (config), the doc id inside `derived`, and `doc_list[].bytes` `913` (main) vs `0` (branch) right after `doc_create` — `913` on both once the 2 s quiet snapshot ran (C2 contract §2: `bytes` written at snapshot, not per update). Everything else identical, incl. the markdown `# Cmp\n\nsame on both.\n\n* one\n* two\n`.

## 3. What changed (commits on `feature/69-devices`, after `d5222bd`)

1. `9f210e6 fix(secrets,link)` — a hub **loads** `mesh/device_key`; only `device_pair_start` mints (`Vault::mesh_device_key_if_set`, `secrets::mesh_device_key_existing`, `link::existing_mesh_identity`; link subscribes to `SecretsEvent::Changed`); `secrets::replicate` calls `announce_unstamped()` after the attach. Root: two lone engines each minted a key and the vault row written before the attach never shipped whole.
2. `b47058f fix(link)` — the no-identity-yet line at debug.
3. `41ac159 fix(replica)` — **stage-1 snapshot**: a peer whose Hello lies below the oldest batch still held gets every live row (under its column clocks) plus tombstones as one batch before the tail (`Store::oldest_seq`, `Engine::snapshot_batch`, `sync::run_stream`). Root: the prune keeps only what *known* peers hold; a late joiner received single-column tails. Test `a_late_joiner_behind_the_pruned_outbox_gets_a_snapshot`.
4. `a799a1b fix(mesh)` — `mesh_meta.name` and the self `engine_stats` row written after the attach (10 s bound). Root: neither was captured; no peer could elect this engine.
5. `8a0a280 fix(mesh)` — a **connected peer is a candidate**; heartbeat age no longer gates it. Root: iroh-gossip delivery paused 30–60 s at a time on loopback → `lapse` → two hubs, the relay replacing the holder 16× in 3 min.
6. `0ec3fb3 fix(mesh)` — an engine with stored peers starts with `holder: None` until one connects (or `lapse_secs`); a lone engine elects itself as before.
7. `3ccfb6a fix(replica)` — `apply_row` **UPDATEs an existing row; INSERTs only when absent**. Root (verified in sqlite3 3.51): `INSERT … ON CONFLICT DO UPDATE` with a column subset trips NOT NULL before the conflict target, so no remote column write ever landed on `docs`, `cost_owners`, `availability`, `engine_stats`, `vault`. Test `a_partial_update_lands_on_a_not_null_table`.
8. `6a3c3ab`, `e9e64e6 fix(link)` — the first reconcile waits for the mesh's self id and the lease `mesh_status` reports (`SetSelf(id, lease)`, `mesh_ready`); no mesh within 10 s → serve. Root: the eager reconcile registered the mesh id at boot before the follower knew the lease.
9. `883abe5 fix(mesh)` — `merge_peers` refreshes the in-memory paired set when the replica delivered a peer row first. Root: C (paired to A) was closed by B as a stranger every 4 s; three lease holders.

Found, not fixed: `mesh_meta.name` default — `meta_default` stamps at `first_seen_ms` meaning "oldest wins", but LWW is newest-wins, so the **youngest** engine's name wins (A+B → "B", +C → "C"; a rename still beats every default); `pkarr publish` WARN every 1–13 s on engines with stored peers and `discovery: true` when `dns.iroh.link` is unreachable (pre-existing); a snapshot batch reaching a fresh engine before its file mirrors attached logs `touches unattached table doc_files/skills; skipped` (the mirror's own scan converges later); each non-hub re-touches the `docs` row after adopting a hub snapshot (N−1 two-column batches per edit, bounded); K deviation 6 stands: a graceful *shutdown* tells devices `HARNESS_GONE 4`, only a lease loss says `HUB_MOVED 5`; `session_take(user)` is needed before a device can continue an A-owned session on B (68 §5 — the UI does it from `OwnerOfflineCard`); the availability ranking hands the lease over as soon as a bucket changes when `handover_secs` is 0 (production 600 s).
