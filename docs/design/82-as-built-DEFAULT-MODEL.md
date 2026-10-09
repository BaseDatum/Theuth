Superseded in part by 148 MODEL-DEFAULTS (2026-10-08): in-session and draft picks no longer set the default.

# 82 — as built: DEFAULT-MODEL

The default model for a new chat = the model last picked manually in a
session's model picker. Branch `fix/default-model-last-manual`, base
`4d6a6ed6`. Line numbers in §1 are at the base commit.

## §1 How the default was set before this change

Two values existed and drifted apart inside one process:

**The live default** — `Agents::default_model` (`crates/agent/src/supervisor.rs:114`),
what new sessions are created on (`:637`, `:644`, `:7956`, `:8005`, through
`resolve_model` `:544`, which keeps an unresolvable id raw so the first turn
errors instead of silently falling back). Writers:

| # | Writer | Kind |
|---|--------|------|
| 1 | `supervisor.rs:7327` `default_model: cfg.model.clone()` — `agent.model` from config.yaml at process start | startup |
| 2 | `supervisor.rs:6143–6148` `impl Message<SetModel>`: **unconditionally** `self.default_model = resolved.id` and `write_config_value(config.yaml, "agent", "model", …)`, whether or not a session was given | every `SetModel` |

`:6146` was the only writer of `agent.model` in `config.yaml` in the workspace
(the other `write_config_value(…, "agent", …)` calls write `compact_style`,
`max_tool_rounds`, `stream_idle_secs`, `max_salvages`, `title_model`,
`escalate_model`, `vision_model`, `communication`, `aws_profile`).

`SetModel` callers, i.e. who rewrote the default:

| Caller | file:line | Manual? |
|--------|-----------|---------|
| Desktop/mobile model picker (draft header and in-session header) | `crates/desktop/ui/src/components/chat/ModelMenu.tsx:120` → `rpc/api.ts:742` → `crates/rpc/src/api.rs:3292` | manual |
| Desktop `/model <id>` | `crates/desktop/ui/src/components/chat/slash.ts:305` | manual |
| TUI `/model <id>` and the TUI picker | `crates/tui/src/model.rs:6070`, `:8196` → `crates/tui/src/lib.rs:625` | manual |
| Schedules / watchers with `model:` (`new_session`, on every fire and `BindSession`) | `crates/schedule/src/lib.rs:1434` (`session: Some(s)`) | **automatic** — rewrote the global default each time a scheduled job ran |
| Slack `/model <id>` in a thread | `crates/slack/src/commands.rs:436` | human, but a thread's choice rewrote the engine default |
| Signal `/model <id>` in a session | `crates/signal/src/commands.rs:287` | same |

Not writers (checked): meeting start (`crates/meeting/src/lib.rs` `start_task`
→ `CreateSessionOptions.model`, session-only, `supervisor.rs:7947–7952`);
subagents (`agent.subagents.model` / `SetSubagentModel` → `pick` at
`supervisor.rs:626–650`, session-only); desks (same path); ACP children
(`set_harness_option`, `supervisor.rs:6220ff`, never `SetModel`); the role
models `SetTitleModel` / `SetVisionModel` / `SetEscalateModel`
(`supervisor.rs:2912`, `:3103`, `:2946` write their own keys);
`AddProvider` / `RemoveProvider` (do not touch `default_model`);
`App.tsx:87 setModel` (dictation model state, a name clash). There was no
Settings-page default-model control.

**The displayed default** — what the new-chat draft showed:

- `crates/rpc/src/api.rs:2311–2315` `info().default_model` =
  `rt.section::<AgentConfig>("agent").model`: the figment snapshot taken at
  process start. It never reflected any `set_model` since launch.
- UI: `ModelMenu.tsx:96`, `Composer.tsx:252`, `meeting/StartForm.tsx:45` read
  `info.default_model`. `ModelMenu.tsx:121` patched `info` optimistically only
  for a pick with no session; a pick inside a session (which also rewrote the
  default, writer 2) did not. `settings/shared.tsx:126 refreshInfo` and every
  reconnect (`App.tsx:83`) put the startup value back. The only per-device
  remembered model is `lib/meeting-settings.ts` (meetings only).
- TUI: header from config at start (`tui/src/lib.rs:141`); after `/new`,
  `model.rs:3384 reset()` kept whatever model was last on screen.

So the "random" default: the draft showed the launch-time `agent.model` (or a
stale optimistic patch), while new sessions were actually created on the last
`SetModel` from *any* path — a schedule firing, a Slack thread, a model picked
inside some other session — and a reconnect flipped the display back.

Multiple engines: a device attached to a hub (68/69) reads and writes that
engine's `config.yaml`; the desktop its local one. Defaults are per engine and
may legitimately differ; nothing here replicates them.

## §2 The rule, as built

1. `SetModel` gains `manual: bool` (`crates/agent/src/lib.rs`, serde default
   `true`). `Agents::set_model` (`supervisor.rs`, split out of the handler)
   switches `session` as before; **only `manual`** sets `default_model`,
   rewrites `agent.model`, and publishes the new global
   `AgentEvent::DefaultModelChanged { model }` (`session() == None`).
   `manual: false` with no session resolves the id and does nothing else.
2. RPC `set_model(session, model, manual?: bool)` (`crates/rpc/src/api.rs`;
   `manual` defaults to `true` so older frontends keep behaving; forwarded to
   a remote owner with the flag).
3. Callers: ModelMenu, desktop `/model`, TUI `/model` + picker pass `true`.
   Schedules pass `false` (`crates/schedule/src/lib.rs`). Slack/Signal pass
   `manual: cx.session.is_none()`: in a thread the switch is the thread's; the
   thread-less form has nothing but the default to set, so it still sets it.
4. New `GetDefaultModel` ask; `info().default_model` now answers with the
   supervisor's live value instead of the startup figment snapshot.
5. UI: `store/sessions.ts dispatch` folds `DefaultModelChanged` into the
   publishing engine's `connection.info.default_model`; the draft
   (`Composer`, `ModelMenu`, meeting `StartForm` fallback) already reads
   that field, so it follows live on every device attached to that engine.
   The optimistic patch in `ModelMenu` is gone. No localStorage model exists
   for the chat draft; `meeting-settings.model` stays a per-device meeting
   preference and is passed as `CreateSessionOptions.model` (session-only).
6. TUI: `Model.default_model` from `Info.default_model` (config.yaml as
   fallback when `info` fails), updated by `DefaultModelChanged`; `/new`
   (`reset`) shows it instead of the previous session's model; the draft
   header follows the event.

## §3 Tests

- Rust `crates/agent/src/supervisor.rs` `only_a_manual_pick_becomes_the_default`:
  `manual: false` with no session → nothing; with a session → row switched,
  default and config.yaml untouched; `manual: true` → row, `default_model`,
  `  model: …` line in config.yaml, and `DefaultModelChanged` on the bus;
  `CreateSession` without a model starts on the default; with
  `CreateSessionOptions.model` (a meeting) uses it and leaves the default and
  the file alone.
- Vitest `crates/desktop/ui/src/store/default-model.test.ts`:
  `DefaultModelChanged` updates the publishing engine's `Info` only, and
  nothing before `info()` answered.

## §4 Gates

- `cargo test -p agent` 357 passed; `-p rpc` 36 + 1; `-p tui` 121 + 1;
  `-p schedule` 61; `-p slack` 95; `-p signal` 63 — 0 failed.
- `cargo clippy -p agent -p rpc -p tui -p schedule -p slack -p signal
  --all-targets --no-deps -- -D warnings`: clean. Without `--no-deps` the run
  fails in the untouched `talk` crate (`clippy::drain_collect`,
  `crates/talk/src/policy.rs:580`), pre-existing at base.
- `cargo fmt --all -- --check`: 279 pre-existing drift hunks, none in lines
  this branch touched (checked by grepping the diff for the new identifiers).
- `just ts-desktop` regenerated `crates/desktop/ui/src/rpc/bindings`
  (`AgentEvent.ts` carries `DefaultModelChanged`); `bun run check` clean;
  `CI=1 bun run test`: 126 files, 699 tests passed.
- Toolchain: `~/.rustup/toolchains/1.98.1-aarch64-apple-darwin`.
- No live daemon smoke run: the engine path is exercised through the actor
  (`Agents::spawn` + `CreateSession` ask) in the Rust test, the UI path
  through `dispatch` in vitest; `info()`/`set_model` RPC are thin wrappers.

## §5 Deviations

- Slack and Signal `/model` are not in the brief's manual list; classified as
  session-scoped when a thread/session is bound, default-setting otherwise
  (the only possible meaning of the thread-less form). Two-line changes in
  `crates/slack/src/commands.rs` and `crates/signal/src/commands.rs`, plus
  one arm in `crates/slack/src/channel.rs`'s exhaustive `AgentEvent` match.
- `crates/rpc/bindings/*.ts` (56 tracked files) are not the `just ts` export
  target (`/bindings`, gitignored) and are stale at base; left untouched.
- A meeting-crate test was not added: the agent-level `CreateSessionOptions.model`
  test covers the path the meeting head uses.

## §6 Open follow-ups

- `SetEffort` still makes every effort change the default (`agent.effort`),
  manual or not — same shape of bug, out of this brief. **Done in 83 FX-EFFORT.**
- The `--config <other>` file is still not written back (pre-existing
  `ponytail:` note at the config write).
