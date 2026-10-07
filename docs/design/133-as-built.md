# 133 STOP-SCOPE — as built

Branch `feat/133-stop-scope` off v2.12.0 (1354b6d8). Contract: `133-stop-scope.md`.

## File:line map

| What | Where |
|---|---|
| `CancelScope { Root, Tree }` (`#[default] Root`, serde lowercase, ts export) and `Cancel.scope` (`#[serde(default)]`) | `crates/agent/src/lib.rs:294-325` |
| Scope-aware `impl Message<Cancel> for Agents`: `Root` → `ids = [session]`, `Tree` → `+ descendants`; rest unchanged | `crates/agent/src/supervisor.rs:2965-2999` |
| Test `root_cancel_spares_running_children_tree_cancel_does_not` | `crates/agent/src/supervisor.rs:12562-12622` |
| `answer_ask` dismiss → Root | `crates/agent/src/lib.rs:341-345` |
| Advisor cancels (leaf) → Root ×4 | `crates/agent/src/advisor.rs` |
| RPC `cancel(session, scope: Option<CancelScope>)`, forwarded as `(session, &scope)` | `crates/rpc/src/api.rs:254, 2769-2788` |
| ACP `CancelNotification` → `cancel(session, None)` | `crates/rpc/src/acp.rs:414` |
| Binding export `agent::CancelScope` (end of list) | `crates/rpc/src/export.rs:466` |
| Schedule `cancel_tree` root cancel → Tree (33 §3.4) | `crates/schedule/src/lib.rs:1641-1650` |
| Signal / Slack / Telegram / talk barge-in → Root | `signal/src/commands.rs:229,245`, `slack/src/channel.rs:191`, `slack/src/commands.rs:401`, `telegram/src/port.rs:171`, `talk/src/lib.rs:636` |
| TUI `Command::CancelTree` (appended), `Model::cancel` → `cancel_at(now)`, `esc_taps`, `ESC_TAPS_WINDOW`, "stopped N agents", `/cancel` help line | `crates/tui/src/model.rs:868, 1645, 3302, 3673, 6900-6932`; `crates/tui/src/lib.rs:485-492` |
| TUI test `triple_esc_cancels_the_tree` | `crates/tui/src/model.rs:14559` |
| Esc reducer `step` / `progress` (`HOLD_MS` 700, `TAP_WINDOW_MS` 1000, `TAPS` 3) + tests | `crates/desktop/ui/src/lib/stop-gesture.ts`, `stop-gesture.test.ts` |
| `StopButton`: 89 hold reducer, pill portal "↑ Stop everything · N agents", armed on `lock`, Esc ring | `crates/desktop/ui/src/components/chat/StopButton.tsx` |
| Composer: `descendants` via `stripRows(children)`, `stop(scope)` + toast, Esc keydown → reducer, keyup/blur/tick effect, `<StopButton>` | `crates/desktop/ui/src/components/chat/Composer.tsx:812-856, 996-1001, 1312` |
| TS client `cancel(session, scope?)` | `crates/desktop/ui/src/rpc/api.ts:262` |
| 19 §2.3 revision note | `docs/design/19-subagent-board.md:188-190` |

## Decisions

- `cancel` keeps answering `()`; "N agents" is derived client-side (desktop: `stripRows` of the store's subtree; TUI:
  live rows of `agents`). No `CancelReport`, no new event.
- The stop button's `up` while `locked` fires `tree` and resets; the reducer's `locked` phase otherwise only leaves
  by `reset`. `interrupt`/`pointercancel`/blur reset the gesture (no lock-on-interrupt here: nothing to keep running).
- The runtime cap (`ChildTimedOut`) never built a `Cancel`; it `tell_cancel`s the one child. Unchanged.
- `schedule::cancel_tree` keeps its one-level child loop and marks the root cancel `Tree`, so grandchildren go too.
- Steer/interject (`Composer.tsx` ×4, `slash.ts`, `talk.$session.tsx`, `App.tsx` quit) untouched: Root by default.

## Deviations

- Toolchain 1.98.1 (the brief's PATH) fails `-D warnings` on main: `double/src/store.rs:546`
  `chunks_exact_to_as_chunks` and two `large_enum_variant` in `tui/src/search.rs`. Fixed the first with `as_chunks`
  (same semantics), allowed the two enums (`#[allow(clippy::large_enum_variant)]`, short-lived messages). Both
  outside 133's scope but blocking its gates.
- `cargo fmt` under 1.98.1 reflows pre-existing code in agent/rpc/tui/slack/signal/talk/telegram/schedule; only my
  hunks were kept formatted, the unrelated churn reverted. `cargo fmt --check` therefore still reports pre-existing
  diffs (`durability.rs:304`, `api.rs:1488,5286`, `tui/lib.rs`, `tui/model.rs:24,37`).
- Not smoke-run in the desktop app or a live TUI (no engine with a running subtree on this machine); covered by the
  reducer tests, the supervisor test and the model test.

## Gates

- `cargo clippy -p agent -p rpc -p tui -- -D warnings` clean.
- `cargo test -p agent --lib`: 458 passed.
- `cargo test -p rpc`: 51 + 1 passed. `cargo test -p tui`: 144 passed + `no_emoji` passed.
- `cargo clippy -p agent -p rpc -p tui -p signal -p talk -p schedule -- -D warnings` clean.
- `just ts-desktop` regenerated `bindings/CancelScope.ts`.
- `bunx tsc -b --noEmit` clean; `bunx vitest run src/lib src/components/chat`: 113 files, 897 tests passed
  (including 6 new `stop-gesture` tests).
