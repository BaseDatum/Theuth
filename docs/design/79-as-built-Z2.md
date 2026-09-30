# 79 — As built, worker Z2: fix-wave merge, gate, re-smoke on the trixie image

Branch `feature/79`, worktree `theuth-79`, 2026-09-30 10:35–11:20 BST. Start `b4e5c38b`; end `72d9efa6` + this report. Brief `/tmp/79/Z2.md`. Nothing pushed; no amend/squash/rebase.

## Commits (in order)

| sha | what |
|---|---|
| `dd4fc1f8` | `merge --no-ff fix/79-F3` (`67358a4c`): mesh bugs 5/9, secrets engine id from mesh. Clean. `cargo check -p mesh -p secrets -p roster -p rpc --tests` 1 m 47 s; `cargo test -p mesh -p secrets --lib` 26 + 22 passed |
| `56e6e009` | `merge --no-ff fix/79-F4` (`72016ad4`): UI, `VmSnapshotInfo`, `secret_requests_list`, three `ts(type = "number \| null")`. Clean. `cargo check -p sandbox -p rpc -p secrets -p roster --tests` 27 s |
| `95b7838e` | `merge --no-ff fix/79-F1` (`a1e17d53`): trixie image, base stage, bootstrap, justfile, design decision 3/§6.1/§16. Clean — the expected §16 conflict with F3's rows did not occur (git placed both; verified rows `§6.3 loopback path`, `§6.6 secrets`, `§6.1 image base/image/bootstrap base` all present). `bash -n deploy/vm/bootstrap.sh` ok; `just --list` shows the three `vm-image*` recipes |
| `c98475cf` | `merge --no-ff fix/79-F2` (`4521476d`): sandbox bugs 2/4/6/7/8, `sandbox_terminal`. One conflict, `crates/sandbox/src/actor.rs:26-32` (the `use crate::vm::…` list: F4 renamed `SnapshotInfo → VmSnapshotInfo`, F2 added `VmHandle`) → both kept under F4's name; F2's new `all_snapshots(&self, name) -> Result<Vec<SnapshotInfo>>` (`actor.rs:575`) renamed to `VmSnapshotInfo`. No other stale `SnapshotInfo` (grep: only `document::SnapshotInfo` remains, by design). `VmRuntime::start(h, out)` survived untouched (F4 never touched a caller). `cargo check -p sandbox -p rpc -p theuth --tests` 46 s; `cargo check -p sandbox --examples` 23 s (`f2_wedge`, `vm_spike`). Seam: F2 already returns `{ job, session }` (`api.rs:5171-5192`, `session` defaults to `owner_root()`), F4's `api.ts` declares `{ session: string; job: string }` — nothing to reconcile; `api.test.ts` lost the `PENDING` allowance entirely (the list would have been empty) |
| `a3ad099b` | `ghcr.io/superradcompany/debian-systemd:12` → `ghcr.io/basedatum/theuth-vm-base:trixie` in `crates/theuth/src/main.rs` (`:678` constant, `:240` help text now "our `theuth-vm-base:trixie` systemd base", `:936` config comment), `SandboxSheets.tsx` `STOCK_BASE`, `sandbox-queries.ts` fixture, `crates/sandbox/tests/vm.rs:17`, `examples/vm_spike.rs:23` (+ its `:304` comment), design §4.3 (`:189`) / §4.7 (`:278`) examples, `79-contract.md:304`, and §16 row "§6.1 bootstrap base" now says the constants name it. Left alone on purpose: decision 3 and §16 "designed" columns (history: they say what was rejected), the recon fact sheet, F1's README sentence, the as-builts. `git grep debian-systemd -- crates deploy` → only `deploy/vm/README.md:20` ("no third-party `debian-systemd` image is used anywhere") |
| `d7d8df1d` | Bug 7 origin logs: `shutdown` RPC is now `with_extensions` and logs `WARN shutdown requested over rpc peer=<id> remote=<bool>` (or "unauthenticated connection"); `main.rs` select arms log `exiting: shutdown token cancelled (rpc shutdown, plugin or actor)` / `exiting: SIGINT` / `exiting: parent process gone` before the 100 ms flush and `process::exit(0)` |
| `859b890f` | `merge --no-ff main` (LOCAL `aa34463f`, UI-only `fix/ipad-board-touch`). Clean, 5 files |
| `a4565808` | Found during the smoke: SIGTERM is caught by the **mesh plugin** (`mesh/src/lib.rs:629-633`, `platform::process::stop_signal`) which cancels the shutdown token, so the `main.rs` line alone said only "token cancelled". Added `WARN exiting: SIGTERM (mesh plugin releases the lease and cancels shutdown)` there. Committed after the gate's clippy step had started — re-ran `cargo clippy -p mesh -- -D warnings` (clean, 36 s) and `cargo test -p mesh --lib` (26 passed) by hand |
| `72d9efa6` | `79-smoke.md` "Re-smoke 2026-09-30 (trixie image, fix wave)" — 11 checks, bug 10, timings; `79-sandbox-vm.md` §16: nine rows for F2–F4/Z2 (`§7 UI` rewritten, `VmRuntime::start`, `VmSnapshotInfo`, snapshot list, destroy sweep, boot failure, `sandbox_terminal`, `secret_requests_list`, exit watch), `§14 secret paste` row updated, open list rewritten |

## Gate (`/tmp/79/gate2.sh`, detached, log `/tmp/79/gate2.log`, 10:44:46–10:54:32)

| step | result |
|---|---|
| `cd packages/apps && bun install --frozen-lockfile` | 489/504 no changes, exit 0 |
| `cd crates/desktop/ui && bun install --frozen-lockfile` | 625/697 no changes, exit 0 |
| `just ts-desktop` | export `8 passed`, bindings copied, exit 0 (2 m 17 s) |
| `cargo build -p theuth` (added: the smoke needs `target/debug/t`; `monty` symlink for tests) | exit 0, 1 m 48 s |
| `cargo check --workspace --tests` | exit 0, 1 m 30 s |
| `cargo clippy -p sandbox -p rpc -p mesh -p roster -p tasks -p secrets -p theuth -p tui -- -D warnings` | exit 0, 50 s, no warnings |
| `cargo test -p sandbox -p mesh -p roster -p tasks -p secrets -p rpc --lib` | mesh 26 · roster 48 · tasks 36 · sandbox 41 · secrets 22 · rpc 54 = **227 passed, 0 failed**, exit 0 |
| `cd crates/desktop/ui && bun run check` | exit 0 |
| `CI=1 bun run test < /dev/null` | **125 files, 694 tests passed**, exit 0 (F4 reported 124/693; the `api.test.ts` server-list check now runs without the allowance) |
| `bun run build` | exit 0 |
| `bindings/methods.json` has `sandbox_terminal` and `secret_requests_list` | 2 |

No load flakes; nothing re-run except the by-hand mesh clippy/test after `a4565808`.

## Re-smoke (`79-smoke.md` "Re-smoke" has the table)

11/11 pass on the observable criteria; three qualifications: bug 2's wedge did not reproduce (the original `build_microvm: SetUserMemoryRegion` failure was forced with `max_memory_mib: 65024` and the next create succeeded in the same process — the fix is still "defensive"); check 8 ran on `lab2` because `lab`'s vault was orphaned (bug 10 below); the guest prompt block's masked-field rule is not observable because `theuth-vm:dev` carries the pre-fix-wave `t 2.3.2`. Exit watch: the engine never exited unasked in 25 minutes; stderr empty; all three exit-origin lines observed live on the rebuilt binary (SIGTERM, `shutdown` RPC, token).

**New bug 10** (not fixed — a protocol/direction decision): the VM pair step has the host `add_peer` the guest's pairing string, and `add_peer` adopts the *inviter's* vault key (`net.rs:605-618`), so each new VM re-keys the host vault under the guest's fresh key and every other engine's copy stops opening (`lab` lost `anthropic` the second `lab2` paired; timing matches to the second). Three fix candidates are in `79-smoke.md`. Z's smoke could not see it (one VM at a time).

## Deviations and decisions (nobody to ask)

1. `PENDING` in `api.test.ts` removed rather than emptied — an empty allowance list is dead code.
2. `cargo build -p theuth` added to the gate script: the smoke needs the binary and the brief's gate already runs `cargo check --workspace`.
3. The stale `mesh_meta.name = lab` row in the reused `/tmp/79smoke/mesh.sqlite` (left by bug 5 in Z's run) was deleted before the smoke; `meta_default` keeps an existing row, so bug 5's fix was unobservable otherwise. Recorded as a rig decision in the smoke report.
4. Bug 7: three log lines instead of F2's two (the mesh plugin's SIGTERM hook is a third origin). `shutdown` became `with_extensions` to name the peer; no `export.rs`/binding change (return type unchanged).
5. `debian-systemd:12` kept in historical "designed"/"rejected" prose and the recon sheet; replaced only where it is a live default, example or contract line.
6. Guest `t` not rebuilt for the smoke (a Linux build in Docker did not fit the timebox beside the gate and the checks); the F2 prompt text is verified in source only.
7. §16 "§7 UI" row rewritten in place rather than appended (the old row said "Terminal disabled").

## Edits outside the merge (all flagged)

- `crates/sandbox/src/actor.rs:26-28, 575` — conflict resolution / rename only.
- `crates/desktop/ui/src/rpc/api.test.ts` — `PENDING` removed.
- `crates/theuth/src/main.rs` — image constant + two comments; three exit-origin `tracing::warn!` lines in the shutdown `select!`.
- `crates/rpc/src/api.rs:1071-1073, 4416-4423` — `shutdown` `with_extensions` + log.
- `crates/mesh/src/lib.rs:631` — SIGTERM log line.
- `crates/desktop/ui/src/components/settings/{SandboxSheets.tsx,sandbox-queries.ts}`, `crates/sandbox/{tests/vm.rs,examples/vm_spike.rs}` — image ref.
- `docs/design/79-contract.md:304`, `79-sandbox-vm.md` (§4.3/§4.7 examples, §16 rows + open list), `79-smoke.md` (appended section).

## Not built / for the integrator

1. **Bug 10** — decide the fix direction (candidates in the smoke report); until then every `sandbox_create` re-keys the host vault and any other paired engine loses its secrets.
2. Rebuild the Linux `t` from this branch (`/tmp/79/deb-build.sh` on `rust:1-bookworm` or a `rust:1-trixie` builder), reload `theuth-vm:dev`, and re-check the guest prompt block and F2's `runuser`-vs-fallback path (runuser confirmed present in the image).
3. Bug 2's real wedge remains unreproduced; the upstream issue text in as-built F2 is ready if it returns.
4. F3's open items stand: global QUIC MTU pin before `loopback_mesh` defaults on; third-engine `peers.addrs_json` overwrite.
5. `79-contract.md` §2.1 (`start(h, out)`), §2.3 (`Terminal`), §2.4 (`sandbox_terminal → { job, session }`, `secret_requests_list`) still say the designed shapes — the §16 rows record the deviations; the contract text itself was not edited (names in the contract win; these are signature deviations F2/F4 flagged).
6. Rig left: `/tmp/79/{gate2.sh,gate2.log,rpc.ts,prompt.ts,events2.jsonl,check8*.out}`, `/tmp/79smoke*` (data dir + logs). `msb ls` and `msb snapshot ls` empty; no smoke engine running.
