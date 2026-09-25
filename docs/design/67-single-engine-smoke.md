# 67 — single-engine smoke: one laptop, no peers, no network

*Worker Z, 2026-09-24. Proves design 67 decision 5's promise — "a one-laptop user never sees any of this" — on the integrated `feature/67-agents` tree (head after the vault merge, `920e355`), and records what had to change. Companion: `67-as-built-Z.md`.*

## 0. Result

| # | Invariant | Before | After (this branch) |
|---|---|---|---|
| 1 | Cold start offline: serving < 5 s, lease = self < 10 s, no repeating relay/discovery errors | serving 0.4 s, lease 0.3 s; **default config offline: ~3 WARN/s** (mDNS send ×280, pkarr publish ×10, relay probe ×5 per 75 s); mesh tables never replicated (plugin order) | serving 0.3 s, lease 0.27 s; **zero mesh/iroh lines after start** on both the default config and `relay: none` |
| 2 | `_replica_log` bounded with zero peers, 500 tasks | `_replica_log` 0 rows; outbox 152 batches / 1.13 MB; idle: one batch per `capabilities_secs` forever (engine_stats churn — latent, hidden by the order bug) | `_replica_log` 0 rows; outbox 33–89 batches / 1.13 MB (one per 200 ms poll with writes, content-proportional); **idle: 0 growth** over 3 refresh periods |
| 3 | Each new plugin degrades, never fails | ok; SIGTERM with mesh down = exit 143 | ok: one `<crate> unavailable: …` WARN each, RPC serves, methods answer `<crate> not running`; SIGTERM clean regardless |
| 4 | Existing behaviour unchanged vs `main` | `list_sessions` on a fresh dir returned **one pinned `agent:theuth` session** | identical to `main`: `[]`; `knowledge_search`, `list_secrets`, `list_schedules`, a schedule without `agent:` — same keys, same values, no task row |
| 5 | Roster idle with no `agents/*.md` | theuth's desk opened at start, re-opened (a new pinned session, the old one archived) every `episode_idle_secs` | no desk, no session, no wake; `agents_list` = synthesised `theuth` with `desk: null` |

## 1. Recipe

Binary: `cargo build -p theuth --bin t` in the worktree. Client: bun, HTTP JSON-RPC 2.0 on `rpc.listen` with the data dir's bearer token (`<data_dir>/token`).

```sh
# /tmp/67-z/rpc.ts — bun rpc.ts <data_dir> <port> <method> [params-json]
const [dir, port, method, params = "[]"] = Bun.argv.slice(2);
const token = (await Bun.file(`${dir}/token`).text()).trim();
const res = await fetch(`http://127.0.0.1:${port}/`, { method: "POST",
  headers: { "content-type": "application/json", authorization: `Bearer ${token}` },
  body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params: JSON.parse(params) }) });
console.log(JSON.stringify((await res.json()).result ?? (await res.json()).error));
```

No network for the process (macOS): a `sandbox-exec` profile that denies every socket but loopback. Proven by `curl https://example.com` → exit 7 inside it, and by the signal plugin's `Websocket error: reqwest error` in every run.

```
; /tmp/67-z/no-net.sb
(version 1)
(allow default)
(deny network-outbound)
(allow network-outbound (remote ip "localhost:*"))
(allow network-outbound (remote unix-socket))
(deny network-inbound)
(allow network-inbound (local ip "localhost:*"))
```

Config (fresh `--data-dir`, two variants):

```yaml
# default: only what every headless smoke needs on a box with a keychain dialog
secrets:
  key_store: file
```
```yaml
# explicit: never dial anything, even after pairing
secrets:
  key_store: file
mesh:
  relay: none          # bind locally, never dial a relay
  discovery: false     # no DNS/pkarr publish, no mDNS
```

Start (same for every run; `THEUTH_RPC__LISTEN` because a running desktop exports one):

```sh
THEUTH_RPC__LISTEN=127.0.0.1:7911 sandbox-exec -f /tmp/67-z/no-net.sb \
  target/debug/t --headless --data-dir /tmp/67-z/one
# log: /tmp/67-z/one/logs/theuth.log.<date>
```

## 2. Evidence per invariant

### 2.1 Cold start, no peers, no network

Default config (`/tmp/67-z/def`, final binary):
```
23:33:44.699  first log line
23:33:44.720  INFO t: started plugin="tools"
23:33:44.949  INFO mesh: mesh: engine 9a0beff8… on relay https://iroh.relay.theuth.io (discovery true)
23:33:44.965  INFO mesh: mesh: lease → Some("9a0beff8") (elected)
23:33:45.004  INFO t: started plugin="rpc"          ← serving, 0.31 s after the first line
mesh_status → {"peers":[],"lease":{"holder":<self_id>,"reason":"elected"}}   (holder == self_id)
```
Lease held by self 0.27 s after the first line — within one `heartbeat_secs` (10 s), not after `lapse_secs` (30 s): `run_lease` calls `recompute("start")` before its first tick and a lone candidate always wins.

Log noise, 2 minutes offline, grouped by target (WARN/ERROR only):

| target | default config, before | default config, after | `relay: none, discovery: false`, after |
|---|---|---|---|
| `swarm_discovery::socket` (mDNS send) | 280 / 75 s | 0 | 0 |
| `iroh::address_lookup::pkarr` (publish) | 10 / 75 s | 0 | 0 |
| `iroh::net_report::reportgen` (relay probe) | 5 / 75 s | 0 | 0 |
| `mesh` | 0 | 0 | 0 |
| `signal::presage` + `signal::channel` (pre-67 provisioning retry, backoff) | 14 | 14 | 14 |
| `document::helper` (pre-67: dev build has no `theuth-documents-59.5` next to `t`) | 7+ | 7+ | 7+ |
| `slack` (tokens absent, once) | 1 | 1 | 1 |

The mesh's own INFO lines at start: `mesh: engine … on relay … (discovery …)`, `mesh: lease → … (elected)`, and one `mesh: replication unavailable (table peers: needs an hlc TEXT column); local only` (see §3, found defect for M). Nothing repeats.

### 2.2 `_replica_log` bounded with zero peers

`bun tasks500.ts /tmp/67-z/one 7911 500` (500 × `task_create{title, brief, owner:{type:user}}`, sequential):

| run | wall | `tasks.sqlite._replica_log` | `tasks` | `replica.sqlite.outbox` rows / bytes | `col_clock` | `wakes` |
|---|---|---|---|---|---|---|
| first binary | 30.4 s | **0** | 500 | 152 / 1,126,154 | 10,015 | 0 |
| final binary | 17.6 s | **0** | 500 | 89 / 1,124,862 | 10,015 | 0 |
| final binary, warm | 6.2 s | **0** | 500 | 33 / 1,125,797 | 10,015 | 0 |

- `_replica_log` is drained every `POLL` (200 ms) into one batch and deleted (`engine.rs::capture`): at rest it holds 0 rows; in flight at most one poll interval of rows. Bounded as specified.
- `outbox` holds one row per poll interval that saw writes (152 → 33 rows for the same 500 tasks as the client got faster); bytes are proportional to the data written (1.13 MB for 500 tasks ≈ 2.25 KB/task, 20 columns each with an HLC), not to the batch count. It is the replication log every later-paired engine catches up from (`sync.rs`: `Hello{version_vector}` then `batches_after`), and there is no snapshot path — so it is **not pruned** (see deviations in the as-built). `col_clock` = rows × columns (10,000 + 15 built-in skill rows) is the per-column LWW shadow clock; it tracks live data, deleted rows are buried with it.
- Idle growth after the writes: `outbox` count unchanged across 3 × `capabilities_secs: 30` (was: +1 batch per refresh — `refresh_stats` rewrote `engine_stats.updated_ms`/`hlc` every period; now it writes only when the probe changed) and `engine_stats.updated_ms` == start time after 2 min.
- The two ~25 KB + 44 KB batches present before any task are the five built-in skills the skills plugin writes to `skills/*.md` on first run, mirrored into the `skills` table.
- Sizes after 500 tasks: `tasks.sqlite` 348 KB, `replica.sqlite` 2.9 MB (WAL included).

### 2.3 Every new plugin degrades, never fails

For each of `mesh.sqlite`, `replica.sqlite`, `roster.sqlite`, `tasks.sqlite`: fresh dir, `mkdir <dir>/<file>.sqlite`, start under the sandbox.

| directory in place of | one WARN line | still served | affected methods |
|---|---|---|---|
| `mesh.sqlite` | `mesh unavailable: open /tmp/67-z/deg-mesh/mesh.sqlite: … unable to open database file` | `list_sessions`, `agents_list`, `tasks_list`, `task_create`, `list_secrets` ok | `mesh_status` → `mesh not running` |
| `replica.sqlite` | `replica unavailable: open …/replica.sqlite: …` | all of the above + `mesh_status` (lease elected) | none over RPC; roster/tasks/secrets log `local only` at debug |
| `roster.sqlite` | `roster unavailable: open …/roster.sqlite: …` | `mesh_status`, `tasks_list`, `task_create`, `list_secrets` | `agents_list` → `roster not running` |
| `tasks.sqlite` | `tasks unavailable: open …/tasks.sqlite: …` | `mesh_status`, `agents_list`, `list_secrets` | `tasks_list`/`task_create` → `tasks not running` |

WARN/ERROR counts per run: exactly one line from the failed crate; the rest is the pre-67 signal/document/slack noise above. `started plugin="rpc"` present in every log. Stop: `mesh.sqlite` run exited 143 on SIGTERM (the graceful handler lived after the mesh's early return) — fixed, now armed first thing in `MeshPlugin::start`.

### 2.4 Existing behaviour unchanged

`main` binary = `~/Develop/theuth/target/debug/t` (`a85a4d2`), same sandbox, same `key_store: file`, port 7912, data dir `/tmp/67-z/main`.

| call | `main` | this branch (`/tmp/67-z/one`) |
|---|---|---|
| `list_sessions` (fresh) | `[]` | `[]` (was: one pinned `agent:theuth`) |
| `knowledge_search ["mesh", 5]` | `[]` | `[]` |
| `list_secrets` | `{"key_store":"file","secrets":[]}` | same |
| `list_schedules` (fresh) | `[]` | `[]` |
| `schedules/plain.md` = `every: 6h` + `Say hello.` (no `agent:`) | fires at once, session `plain` `running:true`, then `last_result: "error: model: Web stream error … api.anthropic.com"` (offline), `runs: 0` | identical text and keys; `tasks_list` unchanged (500 → 500), `tasks WHERE schedule_name IS NOT NULL` = 0 |
| `list_sessions` keys | `agent, archived, created_at, cwd, effort, harness_model, id, model, name, parent, pinned, project, retention, running, tags, title, updated_at` | identical |
| `list_schedules` keys | `approvals, cwd, describe, enabled, error, last, last_result, max_runs, name, next, quiet, running, runs, session, until, when` | identical |
| `mesh_status`, `agents_list` | `Method not found` | present |

Default config: no new required key — every run above except the explicit one used only `secrets.key_store: file`, and the `deg-*` runs used the defaults for roster/tasks/replica. New files in a fresh data dir vs `main`: `mesh/` (identity), `mesh.sqlite`, `replica.sqlite`, `roster.sqlite`, `tasks.sqlite`, `secrets.sqlite` (vault, V), and an empty `agents/` (the roster creates it to watch it).

### 2.5 Roster idle

Fresh dir, no `agents/*.md`, 2 minutes: `agents_list` = `[{file:{name:"theuth", reports_to:"user", tools:"destructive", …}, status:"idle", desk:null, open_tasks:0}]`; `list_sessions` = `[]`; `tasks.sqlite.wakes` = 0 after 500 user-owned tasks; no `desk opened` line in the log. Before the fix the log had `roster: desk opened agent=theuth` at start and `episode closed … reason=idle` + a fresh desk every `episode_idle_secs` (6 h); on shutdown `hub lease elsewhere: desks close`.

## 3. What changed (commits on `feature/67-agents`)

- `43e104a fix(mesh)`: `mesh.discovery` knob (`MeshConfig`, `DEFAULT_CONFIG`, contract); mesh tables attach to the replica when it is up (`lookup_when`; mesh starts before replica, so the attach had always failed as "replica plugin is not running"); `refresh_stats` writes `engine_stats` only on change.
- `55e34ef fix(roster)`: theuth's desk opens on the first wake addressed to it (`DeliverWake`), not at start; `wanted()` = files only, `allowed()` keeps it open while the lease is held.
- `556b2e2 fix(mesh)`: SIGTERM handler armed before the store can fail.
- `5e7879b`, `cc7d7c9 fix(mesh)`: `Net::join_network` — the endpoint binds with an empty relay map and no address lookup; relay(s) are inserted (`Endpoint::insert_relay`) and DNS/pkarr/mDNS lookup added on the first pairing (`pairing_string`, `add_peer`, `refresh_paired` with a non-empty set) or at bind with stored peers. `mesh.relay: none` still disables the relay transport for good.
- `61ca440 test(roster)`: `no_files_means_no_eager_desk_and_theuth_on_demand_only`.

Found, not fixed (owner M): `replica::attach(["peers","engine_stats","availability"])` cannot succeed — `peers` has no `hlc` column, `availability` has a composite primary key — so the mesh's records stay local (one INFO line per start). `engine_stats` also travels by gossip heartbeat, and `on_heartbeat` writes the remote row locally; replicating that table as well would make every engine re-emit every other engine's row with its own clock. Left local-only on purpose.
