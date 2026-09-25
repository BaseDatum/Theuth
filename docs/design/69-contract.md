# 69 — contract (worker C, 2026-09-25)

*The names K (link runtime), D (device app) and Z (smoke) build on. Written once on branch `feature/69-devices`; not changed after C's commit 5. Line numbers are as of that commit. Design: `69-devices.md` v0.2.*

## 1. Identities and keys

| What | Where | Notes |
|---|---|---|
| Engine link identity | `<data_dir>/link/identity` — `crates/link/src/identity.rs:29` `Identity::load_or_create` | **Untouched.** Document-share URLs and `nodes` keep dialing `/h/<engine id>`. |
| Mesh identity (Noise static, X25519) | vault row `mesh/device_key` — `crates/secrets/src/vault.rs:274` `Vault::MESH_DEVICE_KEY`; created by `Vault::mesh_device_key` (`vault.rs:281`), free fn `secrets::mesh_device_key(rt) -> anyhow::Result<Zeroizing<[u8; 32]>>` (`crates/secrets/src/lib.rs:408`) | Value = base64url(no pad) of 32 random bytes, `set_if_version(name, value, "link", None)`; a race loser (`Ok(None)`) refreshes and reads the winner. Link derives the keypair: `Identity::from_private(*key)` (`crates/link/src/lib.rs:134` `LinkActor::mesh_identity`). The mesh device id = `identity.id` = base64url(pub), 43 chars. |
| Reserved vault prefix | `secrets::MESH_PREFIX = "mesh/"` (`vault.rs:119`); `OAUTH_PREFIX` (`vault.rs:114`) | `writable_name` accepts `mesh/<valid_name>` for `set_if_version` only; `Vault::set` rejects any `/`. `Vault::list()` hides every `/` name (`vault.rs:397`): `oauth/*` was **not** hidden before C — it is now. The placeholder regex has no `/`, so `{{secret:mesh/…}}` never expands (test `backend_rows_are_hidden_and_unexpandable`). |
| Sealing helpers | `Vault::seal_with_mesh_key(aad, plain) -> Vec<u8>` (`vault.rs:310`), `Vault::open_with_mesh_key(aad, bytes) -> Option<Zeroizing<Vec<u8>>>` (`vault.rs:316`) | `nonce(24) ‖ XChaCha20-Poly1305(mesh data key, aad)`. Used for `pending.psk_sealed` with `aad = pending id`. |
| Mesh data key (unchanged) | `Vault::mesh_key()` / `adopt_mesh_key` | Travels in the mesh pairing string (67 §5.4). |

## 2. Lease following

| Name | Where | Semantics |
|---|---|---|
| `mesh::lease::Follower` | `crates/mesh/src/lease.rs:11` (`set_self`, `observe`, `holding`, default `holding: true`) | Moved from roster; `roster::lease::Follower` is `pub use mesh::lease::Follower` (`crates/roster/src/lease.rs:8`). `Duties`/`apply` stay in roster. `holder: None` is a loss once `self_id` is known. |
| `mesh::actor()`, `mesh::Status`, `mesh::MeshEvent::Lease` | `crates/mesh/src/lib.rs:641`, `:76`, `crates/mesh/src/types.rs:130` | K: `lookup_when::<mesh::Mesh>(mesh::MESH_ACTOR)` for self id (link starts before mesh), `Subscribe` for events. |
| `link::mesh_uplink::MeshUplink` | `crates/link/src/mesh_uplink.rs:26`; `start(ctx, identity, relay, token) -> Self` (`:34`), `stop()` (`:42`), `serving()` | `stop` flips the `serve: watch::Sender<bool>` to false **and** cancels. |
| `link::mesh_uplink::supervise(ctx, identity, relay, cancel, serve: watch::Receiver<bool>)` | `mesh_uplink.rs:57` | Redial loop gated on `*serve.borrow()`. **K must keep: a lease loss stops redialing** (relay `register` replaces the holder; a redialing loser steals it back). `run_once` (`:88`) is the stub K fills. |

## 3. Tables

### `mesh.sqlite` (`crates/mesh/migrations/0004_mesh_meta.sql:3`)
`mesh_meta(key TEXT PRIMARY KEY, value TEXT, hlc TEXT)` — attached with `peers`, `engine_stats`, `availability` (`crates/mesh/src/lib.rs:545`). `Store::meta_get(key)` (`crates/mesh/src/store.rs:288`), `meta_set(key, value)` (`:297`), `meta_default(key, value, at_ms)` (`:315`, insert-if-absent stamped at `at_ms` = the engine's `first_seen_ms`, so the oldest engine's default wins LWW and any `meta_set` beats every default). Key `name` is written at start with this engine's `mesh.name`/hostname (`lib.rs:535`).

### `link.sqlite` (`crates/link/migrations/0001_link.sql`, `link::store::DB = "link.sqlite"` `crates/link/src/store.rs:20`)
```sql
CREATE TABLE devices (id TEXT PRIMARY KEY, name TEXT, platform TEXT, static_pub TEXT, paired_ms INTEGER, last_seen_ms INTEGER, revoked_ms INTEGER, hlc TEXT);   -- :12
CREATE TABLE pending (id TEXT PRIMARY KEY, psk_sealed TEXT, name TEXT, created_ms INTEGER, expires_ms INTEGER, hlc TEXT);                                        -- :22
```
- `devices.id` = first 8 chars of base64url(static_pub); `static_pub` = base64url, 43 chars; revoke = `revoked_ms`, **never delete**; `last_seen_ms` written on session close only.
- `pending.id` = base64url of 16 random bytes (22 chars); `psk_sealed` = base64url(`seal_with_mesh_key(aad = id, psk)`); TTL `link::devices::PENDING_TTL_MS = 300_000` (`crates/link/src/devices.rs:220`); consumed/expired rows are deleted (written once, so the tombstone is newer).
- `hlc` is left NULL by link; the replica stamps it at capture.
- Attached with `link::store::TABLES = ["devices", "pending"]` (`store.rs:23`) via `lookup_when::<replica::Replica>` → `replica::attach` (`crates/link/src/lib.rs:411`); `Err` → local only.
- `link::store::Store` (`store.rs:74`): `open(path)`, `pool()`, `write(Op)` (ordered writer task), `apply(&Op)`, `devices() -> Vec<Row>`, `pending() -> Vec<PendingRow>`; `Op` (`store.rs:42`) = `PutDevice | Revoke | Rename | TouchLastSeen | PutPending | DeletePending`; `PendingRow` (`store.rs:27`).
- One-time import: `Devices::open(store, <data_dir>/link)` (`devices.rs:110`) — table empty + `devices.yaml` present → rows inserted active (`paired_ms = paired_at_ms`); the file stays, unread afterwards.

## 4. In-memory registries (`crates/link/src/devices.rs`, held in `State`)

| Type | API | Notes |
|---|---|---|
| `link::devices::Row` (`:21`) | `id, key: [u8;32], name, platform, paired_at_ms, last_seen_ms, revoked_ms: Option<u64>`; `device(online)`, `info()`, `active()` | Serde shape = the old YAML row + optional `revoked_ms`. |
| `link::devices::Devices` (`:96`) | `empty()`, `open(store, link_dir).await`, `reload(Vec<Row>)`, `rows()` (active), `all()` (incl. revoked), `len()`, `by_key(&key)` (**revoked = unknown**), `by_id(id)`, `insert(Row)`, `remove(id) -> Option<Row>` (sets `revoked_ms`), `rename(id, name) -> bool`, `touch_last_seen(&key, at_ms) -> bool` | Sync reads; mutations mirror then queue an `Op`. K: poll `Store::devices()` (or the replica feed) and `reload` so rows other engines wrote appear. |
| `link::devices::Pending` (`:235`) | `new(store: Option<Store>, vault: Option<Arc<secrets::Vault>>)`, `create(name) -> Result<(id, psk, expires_ms), String>`, `take(id) -> Option<[u8;32]>`, `single_live() -> Option<(id, psk)>`, `sweep()`, `len()`, `expires_ms(id)`, `reload(Vec<PendingRow>)` | PSKs open only under the local mesh key; `create` fails without the vault. K: `reload` from `Store::pending()` on the hub so rows engine B wrote are usable. |
| `link::state::State` (`crates/link/src/state.rs`) | `devices`, `pairing: Option<pairing::Pending>` (ENGINE identity, in-memory single), `pending: devices::Pending` (`:87`), `mesh_identity: Option<Identity>` (`:90`), `mesh: Option<MeshLink>` (`:93`) | `status()` fills `LinkStatus.mesh` from `mesh`. |

## 5. Wire shapes

### Pairing string (`crates/link/src/pairing.rs`)
`build(relay, harness_pub, secret, name, pending: Option<&str>)` (`:48`), `parse(s) -> Option<Parsed>` (`:84`), `Parsed { relay_host, secure, harness_pub, secret, name, pending: Option<String> }` (`:67`).

```
https://<relay-host>[:port]/link#1.<b64url(pub ‖ secret)>.<b64url(name)>[.<b64url(pending_id)>]
```
3 parts = the engine identity's own pairing (`link_pair`, unchanged); 4 parts = a mesh pairing (`device_pair_start`). 5+ parts or a non-base64url 4th part → not a pairing string. Example with pub = 32×0x01, secret = 32×0x02, name `work`, pending `AbC-_123`:
```
https://relay.example/link#1.AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQECAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAg.d29yaw.QWJDLV8xMjM
```

### `PairHello` (IKpsk2 msg1 payload, `crates/link/src/conn.rs:36`)
```json
{"name":"<device name>","platform":"ios|macos|web|theuth|…","pending":"<pending_id>"}
```
`pending` optional (absent on 3-part strings; older apps). Unknown fields tolerated. `PairAccept`/`SessionHello` unchanged.

### `conn::respond` (`conn.rs:120`)
`respond(msg1, identity, name, known: Fn(&[u8;32]) -> bool, psk: FnOnce(Option<&str>) -> Option<[u8;32]>) -> Option<Handshake>`; `Handshake::Pair { transport, key, hello, psk, msg2 }`. IKpsk2 msg1 is read with a placeholder PSK (psk2 is mixed in msg2), the chooser gets `hello.pending`, `set_psk(2, …)`, then msg2. Engine path passes `|_| st.live_pairing().map(|p| p.secret)`; **K's mesh path passes** `|hint| hint.and_then(|id| pending.take(id)).or_else(|| pending.single_live().map(|(_, p)| p))` — none → `None` → `CLOSE 3`.

### Close codes
| Code | Name | Where |
|---|---|---|
| 4 | `HARNESS_GONE` | relay `crates/relay/src/frame.rs:36`; link `crates/link/src/uplink.rs:38` `CLOSE_HARNESS_GONE`; UI `crates/desktop/ui/src/link/frames.ts:22` `CLOSE_HARNESS_GONE` |
| 5 | `HUB_MOVED` | relay `frame.rs:41` (`COUNT = 6`; the relay forwards whatever byte the uplink sends: `relay.rs` `frame::CLOSE` arm); link `uplink.rs:41` `CLOSE_HUB_MOVED`; UI `frames.ts:17` (`'hub moved'`), `:24` `CLOSE_HUB_MOVED` |
The UI treats every reason but 3 as transient (`crates/desktop/ui/src/link/transport.ts:166`), so 5 already behaves like 4. K sends 5 to every mesh device conn on a graceful handover.

## 6. RPC (`crates/rpc/src/api.rs`) and events

| Method | Line | Reply |
|---|---|---|
| `mesh_status` | `:1210` | `mesh::MeshStatus { self_id, self_name: String, mesh_name: Option<String>, peers, lease }` (`crates/mesh/src/types.rs:136`) |
| `link_status` | `:913` | `link::LinkStatus { …, harness_id (unchanged), mesh: Option<MeshLink { device_id, connected }> }` (`crates/link/src/types.rs:118`, `MeshLink` `:72`) |
| `devices_list` | `:934` | `Vec<link::DeviceInfo { id, name, platform, paired_ms, last_seen_ms, revoked: bool }>` (`types.rs:83`) — revoked rows included |
| `device_revoke(id)` | `:937` | `()`; same actor message as `link_revoke_device` (`Revoke`); publishes `LinkRevoked { id }` + `DevicesChanged` |
| `device_rename(id, name)` | `:940` | `()`; same as `link_rename_device` (`RenameDevice`); publishes `DevicesChanged` |
| `device_pair_start(name?)` | `:946` | `link::PairingPayload { string, expires_ms, pending_id }` (`types.rs:103`); loads the mesh identity, writes a `pending` row; `name: None` → `mesh_status.mesh_name`, else `self_name`, else `link.name`/hostname; refuses at `link.max_devices` |
| `link_*` (engine identity) | `:913–:932` | unchanged |

Actor messages (`crates/link/src/types.rs`): `DevicesList` (`:237`) → `Vec<DeviceInfo>`; `DevicePairStart { name: Option<String> }` (`:243`) → `Result<PairingPayload, String>`; handlers `crates/link/src/lib.rs:317`, `:325`.

`LinkEvent` additions (`types.rs:173–185`, TS tag `type`): `MeshRelay { connected }` (K publishes on mesh uplink register/loss), `DevicePaired { id }` (K publishes when a device pairs on the mesh path; the engine path publishes `LinkPaired` as before), `DevicesChanged` (published on pair, rename, revoke; K also on a replicated `reload` that changed rows). TUI: any of the three → re-ask `link_status` (`crates/tui/src/model.rs:5141`).

Bindings: `just ts` writes `bindings/` (gitignored) — new files `MeshLink.ts`, `DeviceInfo.ts`, `PairingPayload.ts`; `MeshStatus.ts`, `LinkStatus.ts`, `LinkEvent.ts` regenerated; export list `crates/rpc/src/export.rs:174`.

## 7. CLI (`crates/theuth/src/main.rs`)
`t mesh status` prints the JSON reply (now with `self_name`, `mesh_name`). `t mesh devices list | add [name] | revoke <id> | rename <id> <name>` (`MeshCmd::Devices` `:203`, `DevicesCmd` `:210`, dispatch `:305`); `add` prints the QR, the string and the pending id.

## 8. Config keys
No new keys. `link.relay`, `link.name`, `link.max_devices` apply to the mesh uplink and `device_pair_start` too; `mesh.name` seeds `mesh_meta.name`.

## 9. Allow-lists

- **K** (link runtime): `crates/link/src/{mesh_uplink.rs, follow.rs (new), conn.rs (mesh path only), lib.rs}`; relay `crates/relay/src/{frame.rs, relay.rs}` for code 5 only. Duties: `Follower` fed by `MeshEvent::Lease` + `lookup_when::<mesh::Mesh>`; gain → `mesh_identity()` + `MeshUplink::start`; loss → `stop()` + `CLOSE_HUB_MOVED` to device conns; `Devices::reload`/`Pending::reload` from the store; `State::mesh.connected` + `MeshRelay`; `DevicePaired`.
- **D** (device app): `crates/desktop/ui/src/**` — `link/` (`harnesses.ts` `kind: 'engine' | 'mesh'`, `meshName`, per-mesh key `identity/<mesh id>`, 4-part string parsing, send `pending` in `PairHello`, treat close 5 like 4), `components/settings/{Link,Connections,Mesh}.tsx`, `PairScreen`, `StartupScreen`; `crates/tui` for `/mesh devices`.
- **Z** (smoke): `docs/design/69-smoke.md` + fixes anywhere, each disclosed.
- Nobody changes the names in §1–§7 after C's commit 5; additions go beside them.
