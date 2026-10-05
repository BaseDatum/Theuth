# 99 WRITE-CAP-LINE — as built (2026-10-05, branch `feat/99-write-cap-line` on v2.7.0)

## The ask (Sky, 2026-10-05, verbatim)

> the theuth agent should not have the same restrictions as the subagents. it's the agent that all the work goes through, so we need to think through how that should work.

Exempt set chosen by Sky: **"theuth + you, any depth"** — theuth's desk, the user's own chats and every subagent either spawns write with no cap. Hired agents keep a cap; writes to tasks owned by agents in their own reporting line are free.

## D1–D8 as built

| | Decision | Where |
|---|---|---|
| D1 | Uncapped line: `agent_for(session)` is `None` or `"theuth"` → `charge()` returns `Ok(())` before anything else | `crates/tasks/src/ledger.rs:125` (`const UNCAPPED_AGENT = "theuth"`, comment points at `roster::types::AgentName::DEFAULT`), `:340-373` (`charge`), `:377-379` (`capped_agent`) |
| D2 | Reporting-line ownership: checkout is this session, or owner is the agent, or `manager_of` walk reaches it (≤32 steps, stops on a repeated name); `user`/`system` owners never in line | `ledger.rs:383-401` (`in_line`) |
| D3 | Notes are free: `comment()` no longer calls `charge()`, `check_epoch()` kept | `ledger.rs:1290-1292` |
| D4 | Rolling window: `cross_task_write_window_secs` (default 86 400, 0 = lifetime); migration `0006_write_budget_at.sql` adds `at_ms INTEGER NOT NULL DEFAULT 0`; `Store::budget_write(session, task, cap, now_ms, window_ms) -> WriteCharge { allowed, used }` prunes rows older than the window first, `INSERT OR IGNORE` with `at_ms`, no refresh on repeat; `Store::budget_used(session, now_ms, window_ms)` read-only | `crates/tasks/src/config.rs:16-19,34-35`, `crates/tasks/migrations/0006_write_budget_at.sql`, `crates/tasks/src/store.rs:45-50` (`WriteCharge`), `:490-529` (`budget_write`), `:533-551` (`budget_used`) |
| D5 | Refusal text: `tasks.cross_task_write_cap: this session has written to {used} tasks outside {agent}'s reporting line in the last {window}; the cap is {cap}. Wait for older writes to age out, delegate with task_create, or ask the owner's manager to make the change.`; window 0 → `for this session's lifetime`; `window_text(secs)` = `{n} h` / `{n} min` / `{n} s` | `ledger.rs:128-136` (`window_text`), `:361-372` |
| D6 | `WriteBudgetOf(session) -> Result<Option<WriteBudget>, String>`, `WriteBudget { used, cap, window_secs }`, `None` when uncapped; `Ledger::write_budget`; `task_inbox` appends `\n_cross-task writes: {used}/{cap} in the last {window}_` only when capped and `used > 0` | `crates/tasks/src/lib.rs:197-212` (types), `:549-557` (handler), `ledger.rs:404-418`, `crates/tasks/src/tools.rs:352-365` |
| D7 | Config template: reworded `cross_task_write_cap` comment; new `cross_task_write_window_secs: 86400` line | `crates/theuth/src/main.rs:974-975` |
| D8 | `67-agents.md` §7.3 cap bullet rewritten; `67-contract.md` defaults line gains the window key | `docs/design/67-agents.md:344`, `docs/design/67-contract.md:138` |

Untouched, as required: `ledger.rs::dispatch()`, wake/claim code, `crates/roster/**`, `crates/desktop/**`, `Cargo.lock`. `write_budget` is not replicated, so the schema change needs no replica work.

## Tests

`crates/tasks/src/ledger.rs` tests module: `cross_task_write_cap` rewritten to drive `update()` (third refused with the prefix and `2 tasks`/`24 h`; a `comment()` on the refused task succeeds; repeat write free; a checkout holder's session is free); new `theuth_and_user_sessions_are_uncapped`, `manager_writes_to_its_line_for_free`, `user_owned_task_is_foreign_to_a_hired_agent`, `budget_window_decays`, `window_zero_is_lifetime`, `inbox_trailer_reports_budget` (`ledger.rs:3326-3464`). Test helpers `retitle`, `used`, `reroute` (`:3290-3324`).
`crates/tasks/src/store.rs:592-612` `write_budget_at_ms_is_windowed`: migration applies, `budget_used` counts inside the window and not outside, window 0 counts everything, `budget_write` prunes an aged row.

## Gates (run in this worktree)

```
export PATH=$HOME/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin:$PATH
cd /Users/Sky.Moore/Develop/theuth-wt-99-cap
cargo test -p tasks 2>&1 | tail -30
```
```
test result: ok. 61 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.02s

   Doc-tests tasks

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```
```
cargo check -p roster -p theuth 2>&1 | tail -5
```
```
    Checking slack v2.7.0 (/Users/Sky.Moore/Develop/theuth-wt-99-cap/crates/slack)
    Checking rpc v2.7.0 (/Users/Sky.Moore/Develop/theuth-wt-99-cap/crates/rpc)
    Checking tui v2.7.0 (/Users/Sky.Moore/Develop/theuth-wt-99-cap/crates/tui)
    Checking theuth v2.7.0 (/Users/Sky.Moore/Develop/theuth-wt-99-cap/crates/theuth)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 22m 45s
```
```
cargo clippy -p tasks -- -D warnings 2>&1 | tail -10
```
```
    Checking tasks v2.7.0 (/Users/Sky.Moore/Develop/theuth-wt-99-cap/crates/tasks)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3m 40s
```

Formatting: `rustfmt --edition 2024` on the five touched `crates/tasks/src` files. rustfmt recursed from `lib.rs` into `place.rs` (drift reformat); that file was reverted with `git checkout` and is not in the diff.

## Deviations from the brief

- A private `Ledger::capped_agent(session) -> Option<String>` (`ledger.rs:377`) joins `charge` and `in_line`: it is the D1 test shared by `charge()` and `write_budget()`, so the exemption is stated once instead of twice.
- `WriteBudget` derives `Copy, PartialEq, Eq` so the tests can `assert_eq!` on `Option<WriteBudget>`.
- `inbox_trailer_reports_budget` is a ledger-level check (as the brief allows); the `task_inbox` trailer string itself is exercised by no test (there is no `ToolCx`/actor rig in the tasks tests).
- The brief's §4 described `s-user` as a session "not in the router"; the test also exercises `s-theuth` by swapping a `MemRouter` into the ledger (`reroute`), since `Rig.router` is an `Arc` shared with the ledger and cannot be mutated in place.
- `clippy::manual_is_multiple_of` (Rust 1.98) required `secs.is_multiple_of(…)` in `window_text` instead of `secs % n == 0`.

## Not verified

- No live engine run: the migration against a real `tasks.sqlite` with existing `write_budget` rows (expected: rows get `at_ms = 0` and are pruned on the first charge under a window) and the rendered `task_inbox` trailer were not observed end to end.
- No run of the Tauri CLI, `cargo test --workspace`, or any other worktree, per the brief.
