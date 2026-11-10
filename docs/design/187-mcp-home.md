# 187 MCP-HOME — an MCP server has a home machine; a missing runtime fails calmly

Status: built on `feat/mcp-home` (off `19251272`). Rig proof: `/tmp/mcphome/evidence.txt` (+ PNGs there).

## Ask

Sky, 2026-11-10: "shouldn't MCP servers be per harness and not synced, or at least only synced optionally? obviously
the runtime isn't there for this one and that's because I installed it on another machine. we need a better solution
or minimally a more robust failure mode.." Ruling (ask_user 14:49): **Home + visible everywhere** — a server's config
shows on every machine as "on 333" but only runs on its home; a "Run everywhere" toggle covers portable servers; plus
the calm failure mode.

## Before

The record replica mirrors `~/.theuth/mcp/` to every engine (67 §5.1, `crates/replica/src/files.rs`). A stdio server
added on 333 (`mikrotik`) landed on xeus, whose reconcile retried the connect on every reconcile and logged
`WARN mcp connect failed … No such file or directory (os error 2)` each time — most of an 11.8 MB log. The log scrubber
also re-masked a secret value inside a placeholder it had produced (`mikrotik.password` = `mikrotik`), nesting
placeholders.

## As built

| What | Where |
|---|---|
| **Sidecar `mcp/homes.yaml`** (`servers: {<name>: {engine, name, everywhere, off: [engine…]}}`), theuth-owned, keyed by server name so every `mcp.json` shape and multi-server files work and third-party files are never rewritten. It replicates with the directory (one row, last writer wins; a lost claim is made again by the next reconcile of its engine). `.homes.yaml.tmp` is a dotfile, so the mirror never ships a half file | `crates/tools/src/mcp.rs:441` `Homes`/`Home`, `load_homes` :477, `write_homes` :489 |
| **This engine** = its mesh id + name (`tools::set_machine`). `t` seeds it from the stored mesh identity before the plugins start (tools starts before mesh, and `--acp` has no mesh); the mesh plugin sets it at start and on a rename. Never set (no mesh) = every server runs here, nothing is claimed | `mcp.rs:513`; `crates/theuth/src/main.rs:1812`; `crates/mesh/src/lib.rs:1147,1248` |
| **Stamp on add**: `AddServer` writes this engine as home before the entry. Every add path goes through it: `add_mcp_server` RPC, the `add_mcp_server` tool, the UI, Slack/Telegram/Signal `/mcp add` | `mcp.rs:1426` |
| **Placement**: an engine starts a server only if it is the home, or the server runs everywhere, and it is not turned off here; the rest are listed, not started, so their tools never register here | `place` `mcp.rs:570`, `reconcile` :1570, `Settled` :1932 |
| **Home of a hand-written file / migration** (`claim`): a server without a home in `homes.yaml` is claimed by the engine that last wrote its file's replica row — the origin of the row's HLC (`replica::files::writers`), compared with this engine's replica origin (`replica::files::stored_origin`). No row at all means the file never went through the replica, i.e. it appeared on this engine (a peer's file is a row before it is a file), so this engine claims it. Only self-claims are written; a peer's file shows `home not claimed yet` until that peer claims it. A claim is made once: a later edit elsewhere does not move the home | `claim` `mcp.rs:1654`, `written_here` :585; `crates/replica/src/files.rs:149,174` |
| **Migration**: the same rule on the first boot after the upgrade — every existing server is homed on the engine that last wrote its file. The brief's fallback ("unknown → the hub lease holder") is not needed: with a replica, every file has a row once scanned, and a file without one exists only here. One INFO line per server and change: `mcp: <name> home is <machine>` | `tell_homes` `mcp.rs:1750` |
| **Calm failure**: a start that fails with ENOENT is `unavailable_here` with `<command> is not installed on <machine>` (or `<cwd> does not exist on …`); every failure (also other connect errors and login-required) waits out a backoff — 30 s, doubling, capped at 30 min — before the next start; reconciles in between start nothing. A changed entry, `Restart`, a move or re-enabling retries at once. One log line per state change (`said`), never per attempt; repeats at debug | `missing_here` :611, `backoff` :626, `settle` :1765 |
| **RPC**: `list_mcp` rows gain `home`, `home_name`, `home_here`, `everywhere`, `state` (`running · connecting · elsewhere · off_here · off · unavailable_here · error`) and `reason`; `set_mcp_home(server, action)` with `run_everywhere · only_home · move_here · off_here · on_here` | `McpServer` `mcp.rs:994`, `McpState` :1025, `SetMcpHome` :1888; `crates/rpc/src/api.rs:841` |
| **UI**: Settings › MCP gains *Tool servers*: per row `on <home>` (the mesh's current name, else the stamped one; nothing until the mesh answers), the state here (`running`, `not running here (home is 333)`, `not installed here` + reason, `error`, …) and the actions Run everywhere (toggle), Only on <home>, Move home here, Turn off here (only where it would run) / Turn on here. Alone in the mesh only Move home here, for a home on a machine the mesh does not know. Library › Tools rows say why a server is not running here | `crates/desktop/ui/src/lib/mcp-home.ts`, `components/settings/Mcp.tsx`, `components/library/Tools.tsx` |
| **Masking is idempotent**: `scrub` (and `redact_spans`) skip only a placeholder the scrubber itself emits (an exact `sc.placeholders` entry); any other `{{secret:` text is scanned and masked like the rest | `crates/secrets/src/vault.rs` `hits`, `scrub` |

## Upgrade note

After the upgrade, an MCP server runs only on its home machine — the one it was added on, or for an existing or
hand-written file the machine that last wrote it — unless **Run everywhere** is set. Other machines list it as
`not running here (home is …)` and do not offer its tools. Portable servers (remote URLs, runtimes installed
everywhere) need **Run everywhere** once.

## Review round 1

| # | Finding | As built |
|---|---|---|
| 1 | The skip rule left any `{{secret:` … `}}` run unmasked: a value wrapped in the form, `env` output before a JSON close | `hits` protects only an exact emitted placeholder (`Scrubber.held`, at most `held_max` bytes); negative tests: a wrapped value, unknown name, unkept version, multi-line span, `env` + JSON (`redact_masks_values_inside_anything_but_a_real_placeholder`) |
| 2 | Servers homed on an ex-peer ran nowhere; Move home here hidden when alone | Superseded in round 2: adoption is runtime only (below). UI: alone, Move home here for a home the mesh does not know |
| 3 | A broken `homes.yaml` loaded as empty and was written over | `load_homes` → `Result`; on Err the last good homes stay, nothing is claimed or written, one WARN (`note_unreadable`), a reconcile in 30 s; `AddServer`, `RemoveServer`, `SetMcpHome` return the error before writing anything |
| 4 | A replica read error looked like "no row" | `replica::files::writers` / `stored_origin` → `Result`; only a missing file is no rows; a row without a stamp is never this engine's; on Err the claim round is skipped (one WARN, retried in 30 s) |
| 5 | Raw connect errors over RPC | `McpServer.reason` goes through `secrets::redact` |
| 6 | LWW on the sidecar; stale lines of deleted servers | Documented (`SetMcpHome`: one replicated row, set it again if a concurrent change lost it). `claim` drops this engine's own lines of servers no file wants, only when every file parsed |
| 7 | A panicking connect left `pending` set; a stale error beside `unavailable` | The connect task's `JoinHandle` is awaited, a `JoinError` settles as a failure; `unavailable` clears `errors`/`login` |
| 8 | UI flicker and Turn off here everywhere | No chip or actions until `mesh_status` answers; Turn off here only on rows that would run here |

## Review round 2

| # | Finding | As built |
|---|---|---|
| 1 | The round-1 orphan claim was written to the one-row `homes.yaml`: unpair + re-pair left mikrotik homed on xeus for good | `claim` no longer writes adoptions. `place` returns `Place::Adopted` (runs here) for a home not in this engine's paired set when this engine has the lowest id among itself and its peers; `tools::set_paired_check` now hands over the paired set (`Net::paired_ids`), and the mesh reconciles MCP on every paired-set change (`net.rs` `refresh_paired`) and once at start. Test `an_unpaired_home_runs_here_unwritten_and_a_re_pair_restores_it`; rig: unpair → adopted, re-pair → back on its home, `homes.yaml` unchanged |
| 2 | Adoption logging | One line per adoption: `mcp: <name> runs here while its home <machine> is not paired` (`State.adopted`) |
| 3 | Retry timers multiplied | One pending retry for an unreadable homes/replica (`State.unreadable_retry`); the watcher only reacts to `*.json`, `config.yaml` and `homes.yaml` (not `server-audit.jsonl`, images, dotfile temps) |
| 4 | Prune dropped flags of disabled/malformed entries; problem not shown | `claim` prunes only names no file defines at all (`defined`, the `parse_file` shapes, disabled entries included). `McpProgress.homes_problem` carries the error; Settings › MCP › Tool servers shows it |
| 5 | Scrubber edge cases | Pinned: adjacent placeholders, prefix names, case variants (`redact_placeholder_edges_adjacent_prefix_and_case`) |

## Decisions

- Sidecar over a key inside the entry: no rewriting of hand-written or third-party files, no shape detection per layout.
- Home id = mesh engine id (the 182 `tasks.home` / schedule `engine:` convention); the stamped name is display only.
- "Turn off here" is a per-engine list in the replicated sidecar, so every engine can show it; `off` in `config.yaml` stays the everywhere switch.
- PATH-change retries: not built. The engine's own PATH does not change at runtime; the backoff (≤ 30 min) and an edit of the entry cover an install.
- A home on an engine not paired here (it left, an identity reset, a quarantine on one side, a peers row not applied yet) is adopted at runtime only: the lowest engine id among this engine and its paired peers runs it, nothing is written, and a re-pair gives it back. **Move home here** is the way to make an adoption permanent.

## TODO (out of scope)

- Tools of a server homed elsewhere could be forwarded to the home engine over the mesh (RPC forward of the tool call). Today they simply do not appear on other engines.

## Tests

`crates/tools/src/mcp.rs` tests: `add_stamps_this_engine_as_home_and_other_engines_list_it_unstarted`,
`homeless_servers_are_claimed_by_their_last_writer` (hand-written file, migration rule, one line per change),
`only_the_home_starts_a_server_unless_it_runs_everywhere`, `missing_command_backs_off_without_a_hot_loop` (paused
clock: one start per due time 30 s … 30 min cap, 50 reconciles start nothing, a changed entry retries at once, one
log line), `placement_backoff_and_who_may_claim`, `http_401_on_the_discover_probe_is_login_required` (now through the
actor: login-required backs off too). `crates/replica/src/files.rs` `writers_name_the_last_writer_on_every_side`.
`crates/secrets/src/vault.rs` `redact_is_idempotent_for_a_value_inside_its_own_name`. UI:
`components/settings/Mcp.test.ts` (row states, actions, a click sets its own row's home).

## Rig (`/tmp/mcphome/evidence.txt`)

Two scratch engines A and B on this Mac (fresh data dirs, `mesh.relay: none`, loopback binds, paired with
`mesh_pairing_string` → `mesh_add`). `hello-mcp` (a 20-line Python stdio server) is on A's PATH only. Added on A: runs
on A; on B listed `elsewhere` (home A), no start, one `mcp: hello home is A` line. Run everywhere from B: one start,
`unavailable_here` — `hello-mcp is not installed on B`, one WARN line and none per retry. Move home here (UI click on
B, after Only on A): A dropped it, both `homes.yaml` name B; Move home here on A brought it back. A hand-written file
on B was claimed by B from its replica row; A logged `not claimed yet`, then `home is B`. PNGs:
`1-b-run-everywhere-not-installed.png`, `2-a-running-at-home.png`, `3-b-after-move-home-here.png`.

Review round 1 rig (same file, "Review round 1 rig"): a typo in `homes.yaml` on B — both engines warn once, keep their
homes, write nothing over it, and `set_mcp_home` / `add_mcp_server` return the parse error; restored, B claims its new
file. A replica read error (B's `mcp.sqlite` without its table, and at B's next boot before the mirror recreated it) —
one WARN, nothing claimed, claimed on the 30 s retry. A unpairs B: A claims `fresh` and `handb` (homed on B) and backs
off on them; B, still counting A as an offline peer, keeps `hello` homed on A until it removes A too, then claims it.

Review round 2 rig: A and B unpaired, homes reset to the originals (hello on A; handb, fresh on B). Each adopted the
other's servers at runtime (`mcp: handb runs here while its home B is not paired`), `homes.yaml` byte-identical on both.
Re-paired (`mesh_pairing_string` → `mesh_add`): A dropped handb and fresh, B dropped hello, `homes.yaml` still
identical to the original — nothing was written.
