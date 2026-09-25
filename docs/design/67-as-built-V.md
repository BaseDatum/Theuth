# 67 — As built, worker V (vault, OAuth grants, knowledge sync, files as rows)

Branch `feature/67-agents-vault`, five commits on top of the `67-agents` merge. Files: `crates/secrets/**`, `crates/oauth/**`, `crates/knowledge/src/sync.rs` (+ hook and config in `lib.rs`), `docs/design/67-as-built-V.md`, plus the disclosed one-liners listed under "Outside my crates".

## 1. Per commit

### `feat(secrets): the vault replicates`
- `crates/secrets/migrations/0001_secrets.sql`: `vault(id TEXT PK, name, version, ciphertext BLOB, note, source, updated_ms, hlc, UNIQUE(name, version))`, `vault_current(name PK, version, hlc)`, local-only `meta(k, v)`. `id` = `<name>#<version>` (`store::id`).
- `src/store.rs` (new): `DB = "secrets.sqlite"`, `TABLES = ["vault", "vault_current"]`, pool open (WAL, 4, 5 s, `sqlx::migrate!`), `rows`, `meta_get/put`.
- `src/vault.rs` rewritten over sqlite. Values are sealed `nonce ‖ XChaCha20-Poly1305(mesh key, aad = id)`; the **mesh key** (32 random bytes, generated on first open) is stored in `meta` sealed under this engine's local key (keychain / `secrets.key`, unchanged `key.rs`). Reads (`get`, `get_versioned`, `list`, `expand`, `redact*`) are sync from an in-memory copy; `refresh()` rebuilds it (current version and the `KEEP − 1` below it per name; rows that do not open are skipped with a warning). Writes are async: `set`, `restore`, `delete`, and `set_if_version(name, value, source, expected: Option<u32>) -> Result<Option<SecretInfo>, String>` — one `BEGIN IMMEDIATE` transaction: read the pointer, check the precondition, insert the row, `UPDATE vault_current SET version = ? WHERE name = ? AND version = ?` (or `INSERT OR IGNORE` for a new name), prune `version <= new − KEEP`; `Ok(None)` when the pointer moved. `set` keeps the same-value short cut and `note: None` semantics. `adopt_mesh_key(key)` re-seals every local row under a pairing string's key; `mesh_key()`, `pool()`, `disk_stamp()` (max mtime of the db and its WAL).
- Migration: an existing `secrets.vault` is decrypted with the local key, every kept version becomes a row (numbers and `created` preserved), the file is renamed `secrets.vault.migrated`. `credentials.yaml` import unchanged.
- `src/lib.rs`: `SecretsPlugin::start` awaits `Vault::open`, spawns `replicate`: waits for `replica::Replica` via `lookup_when`, `replica::attach(vault.pool(), store::TABLES)` (`Err` → "local only"); polls `disk_stamp` every second and on change `refresh()` + publishes `SecretsEvent::Changed`. New messages `GetMeshKey` → `[u8; 32]`, `AdoptMeshKey([u8; 32])` → `Result<(), String>`. `pub use vault::OAUTH_PREFIX`.
- Mesh key in the pairing string: `mesh::pairing::Payload.vault_key: Option<String>` (base32, `serde(default)`); `Net::pairing_string` asks `secrets::GetMeshKey`; `Net::add_peer` asks `secrets::AdoptMeshKey` after the handshake, before any rows can arrive.

### `feat(oauth): grants are vault rows; the hub is the only refresher`
- `oauth::Grants` trait (`load(ns) -> Option<(version, json)>`, `async store(ns, json, expected: Option<u32>) -> Result<bool>`, `async remove(ns)`); `secrets` implements it on `Vault` over rows `oauth/<ns>` (`set_if_version`, source `oauth`). `Store::with_grants(Option<Arc<dyn Grants>>)`; without grants the `<data_dir>/oauth/<ns>.json` files work as before. `Store::migrate_files()` moves each file into a row once (renamed `.migrated`).
- `Store::save`/`remove` are async; `save` is conditional on the version read (retried once). `access()`: fresh → served; expiring and **not the hub** → `Err("…the hub engine refreshes it — retry shortly")`, no network; hub → refresh, then `save_if(expected = Some(version read))`; `Ok(false)` → re-read, return the other engine's grant if fresh, else `Err("grant refreshed elsewhere; retry")`.
- Hub flag: `oauth::set_hub(bool)` on a process-wide `AtomicBool` (default `true`, so a lone engine refreshes); `mesh::Ctx::emit` sets it from every `MeshEvent::Lease`. Tests pin a per-`Store` flag.
- Agent wiring: `Auth.oauth = Store::new(dir).with_grants(vault as Arc<dyn Grants>)` + `migrate_files().await`; `Logout` awaits `remove`.

### `feat(knowledge): bundle sync over the mesh`
- `crates/knowledge/src/sync.rs` (new). `start(k, sync_secs)`: waits ≤ 120 s for `replica::transport()`, then an accept loop on `ALPN_KNOWLEDGE` and a dial to every peer on each `PeerEvent::Connected` and every `knowledge.sync_secs` (default 300, new `KnowledgeConfig.sync_secs`). Protocol on one stream, frames `u32 LE ‖ JSON`: `Manifest { head, files: path → { blob: Option<id>, seconds } }` each way, then `Files(Vec<{ path, bytes: Option }>)` each way (dialer speaks first at each step). The manifest is one walk of `Repo::changes_from(HEAD)` newest first: the first commit naming a path is its last touch; a deletion is a tombstone (`blob: None`). Rule per path: the peer's entry wins iff its blob differs and `(seconds, blob) >` mine — newer commit wins, tie by blob id so both sides agree. A path the peer lacks is only sent when it is live; a peer's newer tombstone removes the file. Received files are written atomically, committed as `sync from <engine>` via `Repo::commit`, then `Knowledge::sync()` refreshes rows/FTS and publishes `KnowledgeChanged` (its own commit is a no-op because the tree already matches).
- `replica`: `Engine.transport` stored on `register_transport`, `Engine::transport()`, message `replica::GetTransport` → `Option<Arc<dyn Transport>>`, and `pub async fn replica::transport(wait) -> Option<Arc<dyn Transport>>` (polls the actor every second).

### `feat(67): schedules, skills, mcp as replicated files`
- `replica::files::mirror_at(db, dir, table, wait)`: waits for the replica plugin, opens `db` (WAL, 4, 5 s), calls `mirror`, logs "replicates" / "stays local". One spawned line in each plugin's start: `schedule` (`schedule.sqlite`, table `schedules`), `tools::mcp::start` (`mcp.sqlite`, table `mcp`, includes `mcp/config.yaml`), agent supervisor's tool registration (`skills.sqlite`, table `skills`). Rule (documented on `mirror_at`): mirrors are flat — `skills/<repo>/` git clones and `mcp/repos/` are directories and never rows; `sync_skills` keeps clones current. Hot reload: the schedule and mcp watchers fire on projected files; skills are read fresh per round.

### `docs(67): as-built V`
This file.

## 2. Deviations from the brief
1. **`vault` primary key is `id TEXT` (`name#version`) with `UNIQUE(name, version)`**, not `PRIMARY KEY(name, version)`: `replica::attach` refuses composite keys. Extra column `source TEXT` so `VersionInfo.source` survives. `meta` table added.
2. **"Re-encrypt at rest with the local key"** is realised as: rows sealed under the mesh key, the mesh key sealed under the local key in `meta`. A row's ciphertext must be byte-identical on every engine to replicate; wrapping it per engine would break that. A stolen `secrets.sqlite` needs the keychain item exactly as `secrets.vault` did.
3. **`Vault::open/set/restore/delete` are async** (sqlx). 37 §3.5's reload-on-mtime per read is replaced by the plugin's 1 s `disk_stamp` poll + `refresh`; `t --secret-set` lands in the daemon within a second. Tests call `refresh()` explicitly.
4. **Joiner adopts the inviter's mesh key** (`t mesh add` side); its own rows are re-sealed. Two engines that each wrote secrets before pairing keep both sets; a name present on both resolves by the `vault_current` LWW pointer.
5. **Concurrent `set` of one name on two engines** mints the same `(name, version)` id → per-column LWW picks a ciphertext and the pointer picks a winner (`ponytail:` note in `write_row`). Rare; a re-set fixes it.
6. **Hub flag is a process-global `AtomicBool`** in `oauth` set by the mesh, not a `MeshEvent` subscription in `oauth` (a `oauth → mesh` dependency would cycle through `tools`). Default `true` until the first lease result: two engines may both refresh in that window — §4.4 accepts a double refresh.
7. **`oauth::Grants` trait instead of `oauth → secrets`** (cycle via `tools`); `secrets → oauth` is the new edge.
8. **Non-hub `access()` on an expiring grant errors** rather than waiting for the hub's refresh; the next call sees the replicated grant.
9. **Knowledge sync manifests carry tombstones**; "never delete on sync" is kept by only honouring a peer's newer deletion commit, never its absence.
10. **Per-plugin mirror tests live in `crates/replica/src/files.rs`** (`schedules_skills_mcp_round_trip`, one loop over the three names): the harness R built (`Side`, `pump`, `until`) is `pub(crate)` there; three copies in three crates would be the same 30 lines thrice.
11. **Fixed a merge artifact**: `crates/replica/src/types.rs` had `Transport::accept` twice (R and M both added it); nothing compiled until one went.

## 3. Outside my crates (disclosed)
- `crates/mesh`: `pairing.rs` (+`vault_key` field, test literal), `net.rs` (+`BASE32_NOPAD` import, 4 lines in `pairing_string`, 8 in `add_peer`), `lib.rs` (`emit` sets `oauth::set_hub`), `Cargo.toml` (+`secrets`, `oauth`).
- `crates/replica`: `types.rs` (duplicate `accept` removed), `engine.rs` (+`transport` field/getter), `sync.rs` (store transport), `lib.rs` (`GetTransport`, `transport()`), `files.rs` (`mirror_at`, one test).
- `crates/agent`: `supervisor.rs` (`Auth` construction + `migrate_files`, `Logout` `.await`, skills `mirror_at` spawn), `providers.rs` (test `.await`), `Cargo.toml` (+`replica`).
- `crates/schedule`: `lib.rs` (`mirror_at` spawn), `Cargo.toml` (+`replica`). `crates/tools`: `mcp.rs` (`mirror_at` spawn), `Cargo.toml` (+`replica`).
- `crates/slack/src/setup.rs` (`open_vault` async, `.await`s, tests), `crates/theuth/src/main.rs` (4 `.await`), `crates/theuth/src/secret_cli.rs` (`run` async, module doc).
- `Cargo.lock`.

## 4. Not built
- `secret_request` over the hub's channels for agents on other engines (64 §5.1 routes) — unchanged from today.
- Knowledge sync: no rate limiting, whole-file transfer (no delta), `index.md`/`log.md` are per-engine derived files and not synced; a local write racing a received file for the same path in the same instant lands as two commits (`ponytail:` note in `sync::apply`).
- No UI for the mesh key or for "grant refreshed elsewhere"; the `secrets.vault.migrated` / `*.json.migrated` files are left for the user to delete.

## 5. Tests written (not run)
- `secrets::vault::tests` (11): `round_trip_and_reopen` (rows sealed under the mesh key, wrong key/aad opens nothing), `versions_keep_five_and_v_is_monotonic` (+ pruned rows gone), `set_if_version_is_a_precondition`, `expand_placeholders`, `redact_all_versions_and_base64`, `imports_credentials_yaml_once`, `legacy_file_becomes_rows_once`, `names`, `second_instance_sees_writes_after_refresh`, `two_replicas_converge` (two `replica::engine::Engine`s: set on A resolves on B, restore replicates with its source, names equal both ways, delete replicates), `adopting_a_mesh_key_reseals_rows`.
- `oauth::tests` (+2): `non_hub_never_refreshes` (no endpoint call, grant unchanged, fresh grant served), `hub_refresh_with_stale_precondition_re_reads` (in-memory `Grants` with a race hook: the refused write re-reads the winner; a clean refresh lands as the next version keeping the refresh token). Existing tests adapted to the async API.
- `knowledge::sync::tests` (2): `two_bundles_converge_both_ways_and_deletions_travel`, `conflicting_edit_newer_wins_and_history_keeps_both` (over `tokio::io::duplex`; commit times are whole seconds, so each uses one 1.1 s sleep).
- `replica::files::tests::schedules_skills_mcp_round_trip` (1).
- `mesh::pairing::tests::pairing_string_round_trip` now carries `vault_key`.

## 6. `cargo check`
One `cargo check --tests` per commit over the touched crates, each `Finished` with no warnings and no errors on the final run:
1. `-p secrets -p mesh -p slack -p agent` (first run: the merge artifact; second: 3 test errors + 8 doc warnings, fixed; third clean).
2. `-p oauth -p secrets -p mesh -p agent`: clean first run.
3. `-p knowledge -p replica`: one borrow error in a test (`PeerId` temporaries), then 4 doc warnings, then clean.
4. `-p replica -p schedule -p tools -p agent`: clean first run.

Last run (commit 4):

```
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4m 01s
```

## 7. Fix round V2 — `files::tests::schedules_skills_mcp_round_trip`
Panic: `timed out` in `until` (files.rs:373), only when run alongside the other `files::tests`; alone it passed. Cause: the test waited for `a.handle.flush()` (and later `b.handle.flush()`) to return `Some`. The engine's capture poll (`POLL` = 200 ms) empties `_replica_log` on its own; once it wins the race, `flush` is `None` forever and the 5 s deadline fires. Under load the poll wins reliably. The test asserted something the design does not promise: `flush` returns "the batch if there was anything", not "the batch this edit produced" — who captures is an engine detail.

`mirror_at`, the flat-mirror exclusion (`skills/<repo>/`, `mcp/repos/` are never rows — the clone assertion passes) and projection are unchanged; nothing in 67 §5.1 or §2 above is wrong. The looped test already used a fresh `tempdir` per side per iteration, so no isolation change was needed.

Fix (test only): `row` takes its table from `handle.tables()[0]` instead of hard-coding `agents`; the two `until(flush().is_some())` waits became `until(row(..).is_some())` / `until(row(..) deleted == 1)`. `pump` ships from the store, so it is indifferent to whether the poll or a flush captured the batch. Commit `fix(replica): files mirror — test raced flush against the 200 ms capture poll`. `cargo test -p replica` run three times: 10 + 4 + 2 passed each run, 0 failed.

DONE V
