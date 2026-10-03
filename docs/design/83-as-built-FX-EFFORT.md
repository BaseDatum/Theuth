# 83 — as built: FX-EFFORT

The default effort for a new chat = the effort last picked manually (desktop/
mobile effort picker, TUI `/effort`, an explicit session-less set). Same rule
as 82 DEFAULT-MODEL, for `SetEffort`. Branch `fix/effort-default-manual`, base
`2cb6a14e`. Line numbers in §1 are at the base commit.

## §1 How the default effort was set before this change

`Agents::default_effort` (`crates/agent/src/supervisor.rs:115`) is what new
sessions start on (`:640`, `:646` via `pick`, `:8035` in `CreateSession`).
Writers:

| # | Writer | Kind |
|---|--------|------|
| 1 | `supervisor.rs:7353` `default_effort: cfg.effort.clone()` — `agent.effort` from config.yaml at process start | startup |
| 2 | `supervisor.rs:6230–6239` `impl Message<SetEffort>`: **unconditionally** `self.default_effort = m.effort` and `write_config_value(config.yaml, "agent", "effort", …)`, with or without a session | every `SetEffort` |

`SetEffort` callers, i.e. who rewrote the default:

| Caller | file:line | Manual? |
|--------|-----------|---------|
| Desktop/mobile effort picker inside a session | `crates/desktop/ui/src/components/chat/ModelMenu.tsx:131` → `rpc/api.ts:730` → `crates/rpc/src/api.rs:3309` | manual |
| Desktop draft picker → replayed onto the created session | `ModelMenu.tsx:132 onDraftEffort` (local state only) → `Composer.tsx:702 set_effort(id, draftEffort)` | manual (the user picked it), but the draft showed its own local state, not the engine's default |
| Desktop `/effort <level|clear>` | `crates/desktop/ui/src/components/chat/slash.ts:309` | manual |
| TUI `/effort` and the TUI effort picker | `crates/tui/src/model.rs:6118`, `:8238` → `:6839 set_effort` → `crates/tui/src/lib.rs:634` | manual |
| Signal `/effort <level|clear>` in a session | `crates/signal/src/commands.rs:332` | human, but a session's choice rewrote the engine default |
| Steering probe script, per run | `scripts/apps/steering.ts:199` | **automatic** |

Not writers (checked): `apply_effort` (`supervisor.rs:6179`) — the path a
`SetModel` uses to **drop** an effort the new model cannot take
(`:6150–6158`) — writes the session row and `EffortChanged` only, never
`default_effort`; `pick`/`CreateSession` (`:640`, `:646`, `:660`, `:8035`)
read the default and write the row; schedules/watchers (`crates/schedule`)
and Slack (`crates/slack`, no `/effort`) never call `SetEffort`; meetings pass
`CreateSessionOptions.effort` (session-only). `Info` had no `default_effort`
field: the desktop draft could not show the engine's default at all — it
showed `undefined` until the user picked, and the TUI header showed no effort
before the first turn.

## §2 The rule, as built

1. `SetEffort` gains `manual: bool` (`crates/agent/src/lib.rs`, serde default
   `true`). `Agents::set_effort` (`supervisor.rs`, split out of the handler)
   validates and switches `session` as before; **only `manual`** sets
   `default_effort`, rewrites `agent.effort`, and publishes the new global
   `AgentEvent::DefaultEffortChanged { effort: Option<String> }`
   (`session() == None`). `manual: false` with no session validates the value
   and does nothing else.
2. New `GetDefaultEffort` ask → `Option<String>`; `Info.default_effort`
   (`crates/rpc/src/api.rs`, `#[serde(default)]`) answers with the live value.
3. RPC `set_effort(session, effort, manual?: bool)`; `manual` defaults to
   `true` so older frontends keep behaving; forwarded to a remote owner with
   the flag.
4. Callers: ModelMenu, desktop `/effort`, TUI `/effort` + picker pass `true`.
   Signal passes `manual: cx.session.is_none()` (in a session the change is
   the session's; the session-less form has nothing but the default to set).
   `scripts/apps/steering.ts` passes `false`. Slack gains the
   `DefaultEffortChanged` arm in its exhaustive `AgentEvent` match.
5. Desktop: the draft's local `draftEffort`/`onDraftEffort` state is gone
   (`NewChatPane`, `Header`, `ModelMenu`, `Composer`). The draft picker shows
   `info.default_effort` and a pick there is `set_effort(null, level, true)`:
   the engine's default moves, `DefaultEffortChanged` is folded into the
   publishing engine's `connection.info` by `store/sessions.ts dispatch`, and
   the created session starts on it (so `Composer` no longer replays the
   draft pick with a second `set_effort`). Every device attached to that
   engine follows.
6. TUI: `Model.default_effort` from `Info.default_effort` (config.yaml as
   fallback when `info` fails), updated by `DefaultEffortChanged`; the draft
   header (no session open) follows the event; `/new` (`reset`) shows the
   default instead of carrying the previous session's effort over. The line
   printer prints `default effort → …`.

## §3 Tests

- Rust `crates/agent/src/supervisor.rs` `only_a_manual_effort_becomes_the_default`:
  `manual: false` with no session → nothing; with a session → row set, default
  and config.yaml untouched, no `DefaultEffortChanged`; `manual: true` → row,
  `default_effort`, `  effort: "high"` line in config.yaml, exactly one
  `DefaultEffortChanged { Some("high") }` on the bus; `CreateSession` without
  an effort starts on the default; `GetDefaultEffort` answers it.
- Rust `crates/tui/src/model.rs` `default_effort_follows_the_engine`:
  `DefaultEffortChanged` moves the draft header but not an open session's;
  `reset()` shows the default.
- Vitest `crates/desktop/ui/src/store/default-effort.test.ts`:
  `DefaultEffortChanged` updates the publishing engine's `Info` only, a clear
  lands as `null`, and nothing before `info()` answered.

## §4 Gates

- `cargo test -p agent -p rpc -p tui -p signal -p slack`: agent 359 passed;
  rpc 36 + 1 (prompts); tui 122 + 1 (no_emoji); signal 63; slack 95 — 0 failed.
- `cargo clippy -p agent -p rpc -p tui -p signal -p slack --all-targets
  --no-deps -- -D warnings`: clean. (With deps the run fails in the untouched
  `talk` crate, pre-existing at base, per 82.)
- `cargo fmt -p <crate> -- --check`: base drift untouched; every hunk this
  branch adds is rustfmt-clean (checked by filtering the per-file `rustfmt
  --check` diff for the new identifiers: no lines).
- `just ts-desktop` regenerated `crates/desktop/ui/src/rpc/bindings`
  (`AgentEvent.ts` carries `DefaultEffortChanged`, `Info.ts` `default_effort`);
  `bun run check` (tsc) clean; `CI=1 bun run test`: 129 files, 709 tests passed.
- Toolchain: `~/.rustup/toolchains/1.98.1-aarch64-apple-darwin`.
- No live daemon smoke run: the engine path is exercised through the actor
  (`Agents::spawn` + `CreateSession` ask) in the Rust test, the TUI header
  through `Model::apply`/`reset`, the UI path through `dispatch` in vitest;
  `info()`/`set_effort` RPC are thin wrappers.

## §5 Deviations

- `Info` had no `default_effort`; added (`crates/rpc/src/api.rs`,
  `crates/desktop/ui/src/rpc/extra.ts`) since the desktop draft and the TUI
  header need the engine's live default to follow it.
- The desktop draft kept a local effort state replayed onto the new session
  (`Composer.tsx:702`); removed rather than flagged, because a draft pick is
  a manual pick of the default (the model draft has no such local state).
- `scripts/apps/steering.ts:199` (outside the enumerated crates/UI) is a
  programmatic per-run `set_effort`; one-token change to `manual: false`.
- `crates/rpc/bindings/Info.ts` (tracked, stale at base) was rewritten by the
  ts-rs export run and restored; the export target is `/bindings`
  (gitignored), copied into `crates/desktop/ui/src/rpc/bindings` by
  `just ts-desktop`, as in 82.

## §6 Open follow-ups

- The `--config <other>` file is still not written back (pre-existing
  `ponytail:` note at the config write).
- Slack has no `/effort`; if one is added it should pass
  `manual: cx.session.is_none()` like Signal.
