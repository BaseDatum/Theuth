# 67 — as built, worker Z: single-engine invariants

*Headless smoke of the integrated `feature/67-agents` tree (head `920e355`, the vault merge) with no peers and no network; what broke and what was fixed. Evidence, commands and config: `docs/design/67-single-engine-smoke.md`.*

## 1. Per commit

| commit | crate | what |
|---|---|---|
| `43e104a fix(mesh): single-engine — discovery knob, attach after replica, no idle stats churn` | mesh (+ `main.rs` `DEFAULT_CONFIG` comment, `67-contract.md` config line) | `MeshConfig.discovery: bool` (default `true`) wired into `BindOpts`; `replica::attach(["peers","engine_stats","availability"])` moved into a `lookup_when::<replica::Replica>` task — the mesh plugin starts before the replica plugin, so the synchronous attach had always logged `replication unavailable (replica plugin is not running)`; `Ctx::refresh_stats` probes first and writes `engine_stats` only when the probe differs from the cached row (the start-up write moved before `Ctx` is built) — before, every `capabilities_secs` rewrote `updated_ms`/`hlc`, i.e. one replicated batch per period forever on an idle engine; dropped the unused `AsyncWriteExt` import in `net.rs` (warning on the merged tree). |
| `55e34ef fix(roster): single-engine — theuth's desk opens on demand, not at start` | roster | `wanted()` returns only valid, unpaused, approved files; new `allowed(name)` = lease held ∧ (theuth ∨ wanted) decides which open desks stay; `DeliverWake` opens theuth's desk when the wake is for `theuth`, the lease is held and none is open. Before: a fresh data dir had one pinned `agent:theuth` session from start, archived and replaced every `episode_idle_secs`. |
| `556b2e2 fix(mesh): single-engine — SIGTERM handler armed before the store can fail` | mesh | the `stop_signal` → `shutdown.cancel()` task moved to the top of `MeshPlugin::start`; with `mesh.sqlite` unwritable SIGTERM had exited 143. |
| `5e7879b fix(mesh): single-engine — discovery armed on first peer, silent for a lone engine` and `cc7d7c9 fix(mesh): single-engine — relay joined on first peer too; a lone engine dials nothing` | mesh | `Net::join_network` (idempotent, `AtomicBool`): inserts the configured relay(s) with `Endpoint::insert_relay` and adds `PkarrPublisher/PkarrResolver/DnsAddressLookup::n0_dns()` + `MdnsAddressLookup` via `AddressLookupBuilder::into_address_lookup`. The endpoint binds with `RelayMode::Custom(RelayMap::empty())` (or `Disabled` for `relay: none`) and no lookup services. Called from `bind` when the stored peer set is non-empty, from `pairing_string` (before the 3 s `online()` wait so the string carries a relay address), from `add_peer` before dialing, and from `refresh_paired` when the set becomes non-empty. `mesh.relay: public` → `RelayMode::Default.relay_map()`. |
| `61ca440 test(roster): single-engine — no eager desk without agent files` | roster | test below. |
| `docs(67): single-engine smoke + as-built Z` | docs | `docs/design/67-single-engine-smoke.md`, this file as `docs/design/67-as-built-Z.md`, one sentence in `67-contract.md` §mesh config about the lazy join. |

## 2. Invariants — one line each (full evidence in the smoke doc)

1. **Cold start, no peers, no network** — `sandbox-exec` profile denying all but loopback; default config and `relay: none, discovery: false`. Serving (`started plugin="rpc"`) 0.31 s after the first log line; `mesh: lease → Some(self) (elected)` at 0.27 s; `mesh_status.lease.holder == self_id`. After start, 2 min offline: **0** WARN/ERROR from `mesh`, `iroh*`, `swarm_discovery` on either config (before: 280 mDNS + 10 pkarr + 5 relay-probe warnings per 75 s on the default config).
2. **`_replica_log` bounded** — 500 × `task_create`: `_replica_log` = 0 rows at rest (drained every 200 ms), `tasks` = 500, `outbox` 33–152 rows (one per poll interval with writes) / 1.13 MB (proportional to data), `col_clock` 10,015, idle growth 0 over 3 × `capabilities_secs`. Outbox not pruned — see deviation 1.
3. **Degrade, never fail** — `mesh.sqlite`, `replica.sqlite`, `roster.sqlite`, `tasks.sqlite` each replaced by a directory: one `<crate> unavailable: open …: unable to open database file` WARN, `started plugin="rpc"` in every log, RPC serves; only the crate's own methods answer `<crate> not running`.
4. **Existing behaviour unchanged** — vs `main` (`a85a4d2`, `~/Develop/theuth/target/debug/t`): `list_sessions` `[]` (was one pinned `agent:theuth`), `knowledge_search` `[]`, `list_secrets` `{"key_store":"file","secrets":[]}`, `list_schedules` `[]`; an `every: 6h` schedule without `agent:` fires, fails offline with the same model error text, same JSON keys, creates no task row. No new required config key: every run but the explicit one used only `secrets.key_store: file`.
5. **Roster idle** — no `agents/*.md`: `agents_list` = `[theuth (synthesised), status idle, desk null]`, `list_sessions` `[]`, `wakes` 0, no `desk opened` log line over 2 min and 500 user-owned tasks.

## 3. Deviations from the brief

1. **Outbox not pruned.** The brief says "if it grows, add pruning: with no known peers, acknowledge locally after capture". `_replica_log` (the capture log the invariant names) is already bounded; the `outbox` in `replica.sqlite` is the replication history a later-paired engine catches up from (`sync.rs`: `Hello{version_vector}` → `batches_after(origin, their_seq)`), and the replica has no snapshot path — pruning it on a lone engine would make the first engine paired after a period of solo use never receive the rows written before pairing, silently. That is the primary 67 use case. Left as is; measured instead: one row per 200 ms of write activity, bytes ≈ the data written (2.25 KB per 20-column task), 0 idle growth once the `engine_stats` churn was removed. The upgrade path, if it matters: own-origin compaction into one snapshot batch (`row_as_changes` per live row + tombstones, `seq` = current max) — sound because every `Change` carries its own HLC — not a delete.
2. **theuth's desk is lazy.** Decision 5 lists "the default agent's desk" among the hub duties; §6.2 does not say when it opens. Opening it at start put a pinned session into every fresh data dir (invariants 4 and 5 both fail), so it now opens on the first wake addressed to `theuth` and is kept while the lease is held. `task_place`'s ambiguous case asks theuth's desk (`PlaceContextReply.desk`) — with no desk open it returns `theuth's desk is not open`, as before when the desk was closing; a lone engine never reaches the ambiguous case (one engine).
3. **Relay and discovery are joined lazily**, not only switchable. The brief asked for `mesh.relay: none` (already supported) and a `mesh.discovery` knob (added); with the default config an offline laptop still logged ~3 warnings a second from iroh (mDNS sends, pkarr publishes, relay probes), so the default is now "dial nothing until there is a peer". `discovery: false` / `relay: none` remain the hard switches. Cost: the first `pairing_string` on a fresh engine waits up to 3 s for the relay to come up (it already did).
4. **`mesh` tables still local.** Surfacing the attach after the replica is up revealed it can never succeed: `peers` has no `hlc` column and `availability`'s primary key is composite (`replica::attach` refuses both). Not fixed here — `peers` is already exchanged by the mesh's own `Peers` message and merged by `pairing::merge`, so a second (per-column LWW) writer would need a decision from M; `engine_stats` rides on gossip heartbeats and `on_heartbeat` writes remote rows locally, so replicating it would make every engine re-emit every peer's row under its own clock. One INFO line per start says so.
5. **Ran `cargo test -p mesh -p replica`** once after the fixes (allowed by the Z exception); the roster test was written, not run.

## 4. Not built / observed, left alone

- Pre-67 noise in an offline dev build: `signal::presage` provisioning retries (backoff, ~14 lines / 5 min), `document::helper` (`theuth-documents-59.5` is not next to a `cargo build`-only `t`; a release bundle has it) — and through it one `tasks::thread: task thread not created` WARN per `task_create`.
- The roster creates an empty `agents/` directory on start (it watches it). `main` has none. Harmless; left.
- Built-in skills: five files are written to `skills/` on first run and mirrored into the replicated `skills` table (~69 KB of outbox per fresh engine, own clock). Two paired engines converge by LWW; not a single-engine concern.
- `lookup_when` polls at 50 ms forever when the target actor never registers (e.g. roster → mesh with `mesh.sqlite` unwritable). Pre-existing runtime idiom.

## 5. Tests written (not run)

- `roster::tests::no_files_means_no_eager_desk_and_theuth_on_demand_only` — a loaded roster with no files and the lease held: `wanted()` empty, `allowed("theuth")`, `!allowed("ops")`; after writing `ops.md` and `reconcile()`: `wanted() == ["ops"]`, both allowed; lease released → nothing allowed, nothing wanted.

## 6. Checks

- `cargo check -p mesh -p roster` (after 43e104a/55e34ef): `Finished dev` — one warning, the pre-existing unused `AsyncWriteExt` import, fixed in the same commit — wrongly: the test module uses it, and the orchestrator restored it as a `#[cfg(test)]`-side import in `4077819`; my `cargo check -p mesh` without `--tests` did not see it.
- `cargo check -p mesh` (556b2e2), `cargo check -p mesh --tests` (5e7879b, cc7d7c9), `cargo check -p roster --tests` (61ca440): `Finished dev`, no warnings.
- `cargo build -p theuth --bin t`: once per fix (four builds), all `Finished dev`.
- `cargo test -p mesh -p replica` after the fixes: see the line appended below.

`cargo test -p mesh -p replica`: mesh 12 passed (lease ×6, pairing ×2, identity, stats ×2, `net::tests::pair_connect_and_round_trip_records` — pairing through the lazy `join_network` with `relay: ""`, `discovery: false`); replica 10 lib + 4 spike + 2 loopback passed; 0 failed.

DONE Z
