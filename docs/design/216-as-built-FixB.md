# 216 — As built: Fix-B (runtime robustness)

Fix round after `/tmp/theuth-216/review-rust-1.md` and the second review (S2, S8). Files: `crates/plugins/src/{actor,host,proc,watch,attention}.rs`.

## What changed

| Finding | Change |
|---|---|
| R2 | `Outbox::first` slot drained before `lagged`, control and events; `start()` queues `hello` before the outbox reaches `e.line`, the router target or a request. `Launch.hello` removed. |
| R3 (actor side) | `Boot` carries the vault; `GrantsFile::attach` after load (fail closed). `verified` → `GrantsFile::verified`. `sign_and_store` → `bump` (when an entry already exists) + `insert_signed`. Uninstall → `revoke`. |
| R5 | `needed_card`: a hashless grant whose current request needs a code hash files a code-changed card when `plugins.dev` is off. |
| R8 | stderr read with `proc::LineReader` (`max_line_bytes`, lossy UTF-8); a long line is logged as discarded. |
| R9 | `ping`/`catalog` paced by `query_per_s` (`-32005`). Control lane capped at 4096 lines / 16 MiB: overflow ends the outbox and the supervisor kills the process as a crash ("control lane overflow"). Refusal `log` lines: 5/s per plugin, the rest counted into the next. |
| R13 | Nothing in actor: `call` passes the running grant; Fix-A's slot reopens the session when `max_risk` differs. |
| R14 | One process-wide lock for `.pgids`. |
| R15 | `.pgids` line: `<pgid>\t<leader lstart>\t<argv>`; the reaper requires the same start time (`ps -ww -o lstart= -o command=`, `LC_ALL=C`) before the argv match. |
| R16 | `Logs` for an unknown plugin runs `manifest::check_name` first. |
| R18 | Fatal exit without a `ProcessTree` → `child.start_kill()`. |
| R19 | `.pgids` cleared only when `ShutdownAll` answered in time. |
| R20 | `Install` parses `plugin.yaml` and checks the name; the folder rules run on the installed copy. |
| R21 | `reload` (parse, hashing), Boot scan + grants load, grants save, install copy, uninstall moves and `Logs` tail run in `spawn_blocking`, awaited in the handler (ordering kept). |
| R23 | Watcher ignores paths under `target`, `__pycache__`, `node_modules`, `.git`. |
| R25 | `RequestApproval`: 3 per plugin per minute → `RateLimited`. |
| R28 | `expect`s in production paths replaced. |
| R30 | `hello.engine` = mesh self name (`agent::communication::mesh::engine_names`). A covered manifest change with the same `run`/`cwd`/`env` and code (narrowing, or an approved widening) updates the running grant, router target and tools and sends `granted`; anything else restarts. |
| R31 | `Conn::request_with` re-checks `gone` after inserting its pending entry. |
| S2 | Install copy: source canonicalised, refused when it is, holds or sits inside the plugins folder; symlinks refused (`Invalid`); at most 10 000 files / 256 MiB. |
| S8 | `notify` `level` (info/warn/error) and `open.placement` (only a granted placement) are kept on the note; item title prefixed `Warning:`/`Error:`, detail ends with `theuth://plugins/<name>/<placement>`. |

## Deviations

1. **R3 on disable**: enable/disable re-sign the entry after a generation bump instead of `revoke`. Revoking on disable would make enable need a new press (contract: `SetEnabled` on starts without one); the bump already makes the earlier `enabled: true` entry unverifiable. Uninstall revokes.
2. **R15 argv**: the argv check stays "whole run or its last word" (interpreter shims and `sh -c` exec past the recorded program); the start time is what rules out a recycled pid.
3. **R30 `hello.clients`** stays `{}`: there is no live client registry in rpc; adding one is outside `crates/plugins`.
4. **S8 OS notification** for `warn`/`error` is deferred to v1.1. `AttentionItem` has no level or link field (roster), so both are carried in title/detail; a typed field is a roster hand-off.

## Hand-offs

- Fix-A / manifest owner: `CheckPath` still uses `manifest::load` / `check_offline`, which apply the folder-name rule to the source path, so `plugin_check ~/Downloads/turns-main` reports a name error that `plugin_install` no longer has.
- Roster owner: `AttentionItem` fields for a deep link and a level (S8).
