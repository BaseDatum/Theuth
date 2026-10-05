# 98 — As built: COMMANDS (native commands, panels, keyboards, `guard`, `t --telegram-setup`)

Worktree `theuth-98-wcmd`, branch `feat/98-cmd`, base `a46fc577` (stage-1 contract). Design
`98-telegram.md` §7.2, §8.1 (a), §10.3, §10.4; contract `98-contract.md` §2.7 + §4.

## Files

- `crates/telegram/src/commands.rs` — the table (unchanged from stage 1), `owner_only`, `scopes`,
  `CommandCx`, `Reply` (+ `text`, `code`), `Outcome` (+ `CloseTopic`), `Expect`, `handle`,
  `continue_prompt`, `help`, `prompt`; every Signal arm ported (`crates/signal/src/commands.rs`)
  with the same asks and wording; `#[cfg(test)] pub(crate) mod testing` (`FakePort`, fixtures)
  shared with `ui`.
- `crates/telegram/src/ui.rs` — panels `help`, `status`, `sessions`, `agents`, `model`, `skills`,
  `mcp`, `logins`, `login_url`, `approvals`, `no_session`; `action` (`cmd:` router); `guard`;
  `table`; `short`, `age`, `k`.
- `crates/telegram/src/setup.rs` — `Terminal`, `Tty`, `DAEMON_RUNNING`, `run`, `run_with_host`,
  `deep_link`.
- this note.

## What the dispatcher does (98 §10.3)

- Owner-only names from a non-owner → `This command is restricted to the owner.` before any ask.
- Session = `cx.session` (ROUTE fills thread-or-latest). None → `ui::no_session()`: the
  `no session yet: send a message first` line with a `Sessions` button (`cmd:sessions`).
- Explicit session override: `cancel <prefix>`, `compact <prefix>`, `agents <prefix>` (full id,
  unique prefix or unique suffix; ambiguity refused). `cancel <arg>` tries a session first (what the
  panels send), then a subagent of the thread's session by name or `parent/name`.
- `start`/`help`/unknown → `ui::help()`. `new` → `NewThread`. `resume` → `Resume { session, title,
  reply }`. `cancel` → `Cancelled`. `telegram …` → `Allow / RemoveAllowed / AllowGroup /
  RemoveGroup / Link`, bare or `status` → `render_status` in a fence; non-numeric ids → usage.
- `skills add` / `mcp add` without an argument (typed or via the `Add…` button) → `Outcome::Prompt
  { reply: "Reply with the URL…", keyboard { rows: [], force_reply: true }, expect }`;
  `continue_prompt` runs `add <text>`.
- `model <id>`: `ListModels` first; a unique id prefix completes (button data is cut to 64 bytes),
  then `SetModel { session: cx.session, manual: session.is_none() }` as Signal.
- `status`: `ui::status` table (polling · bot · topics · threads · session · model · turn · context)
  with per-method counts in `<details>`; context = `Loaded.context_estimate`.
- `jobs`, `prompt` → fenced text; `provider`, `peers`, `requests` → GFM tables via `ui::table`.

## Panels and presses (`ui.rs`)

`callback_data` always `cmd:…`, cut to 64 bytes; keyboards ≤ 100 buttons, ≤ 8 per row (`keyboard`
/ `fit`). Sessions 10 per page, models 24 per page (grouped by provider, one row group each),
skills/mcp 20 lines per page; `More…` → `cmd:page:<panel>:<n>`. `agents`: `Cancel <name>` →
`cmd:cancel:<last8>` while running; `Close topic <name>` → `cmd:close:<last8>` for a non-running
child whose session is bound to a `<chat>:<topic>` key in `TelegramStatus.threads` → `action`
resolves the id and returns `Outcome::CloseTopic(session)`. `approvals` presses map to
`approvals on | off | off always` through `handle` (owner gate applies).

## `guard` (98 §8.1 (a))

Escapes `<`, `>`, `&` outside fenced blocks and backtick spans; leaves known tags whole (to their
`>`), existing entities (`&amp;`, `&#60;`, `&#x3c;`) and a line-leading `>` quote marker alone.
Idempotent. Fences: ≥ 3 backticks/tildes at line start, closed by a run ≥ as long of the same
char; spans: CommonMark exact-length backtick runs; indented code blocks are not recognised.

## `t --telegram-setup` (98 §10.4)

`run_with_host(data_dir, term, host)`; `run` = `None`. Steps: (0) `engine.lock` advisory
`try_lock` — held → prints `daemon running: use /telegram link or the telegram_link tool` and
returns; (1) vault token verified with `getMe` (kept and said so), else prompt ×3 with the
@BotFather steps; prints `@username · topics on|off` and the Bot Settings → Topics path; (2) vault
`telegram_bot` (`Vault::open` with `secrets.key_store` from config / `THEUTH_SECRETS__KEY_STORE`,
source `telegram setup`); (3) `setMyCommands` for `scopes(None)`; (4) owner already set → `re-pair?
[y/N]` (kept unless `y`); else mint, print deep link + `link::qr`, one-shot poll (`offset -1, limit 1`
skip, then `offset last+1, limit 100, timeout ≤ 30`, backoff on errors) until `/start <code>` from
a user or `PAIR_TTL`; (5) `telegram.owner`, `telegram.enabled: true` via `write_config_value`,
the owner's `Chat` command scope, the group note, `run t --headless and message the bot`.

## Judgement calls

- `pulldown_cmark` is not reachable (`markdown` re-exports only `Alignment`; Cargo.toml is not a
  COMMANDS seam) → `guard` finds code with its own CommonMark backtick/fence rules (tested).
- `a2a` is not a dependency of the crate → `/a2a` answers the network line from
  `agent::GetCommunication { session: None }`; `reload` says it needs the terminal.
- `figment` / `serde_yaml` are dev-deps only → `setup::config_value` sniffs the two-level YAML for
  `secrets.key_store` and `telegram.owner` (tests cover quotes, comments, `null`).
- Running-engine check is the same advisory lock `rpc::engine::acquire` takes, inlined (`rpc`
  depends on `telegram`, not the reverse).
- `/model gpt sets` is covered Slack-style: without an actor the `SetModel` ask fails with
  `error: agent actor is not running`, proving the arm reached the ask, not the panel.
- `help` groups: session (new sessions resume cancel compact clear undo redo model effort agents
  jobs prompt broadcast), harness (help status start skills mcp login logout provider), setup
  (approvals telegram communication peers requests a2a); the test forces every name into exactly
  one group.
- `>` is escaped too (brief: `Vec&lt;String&gt;`), except as a leading quote marker.

## Contract changes

- `Outcome` gains `CloseTopic(SessionId)` and derives `PartialEq, Eq`.
- `Reply::text(..)`, `Reply::code(..)` constructors; `commands::{OWNER_MSG, NO_SESSION}` and
  `commands::prompt(Expect) -> Outcome` are `pub`.
- `ui::{short, age, k}` are `pub`; `ui::status(&TelegramStatus, Option<[String; 4]>)`,
  `ui::agents(&[AgentInfo], &[(String, SessionId)])`, `ui::model(&[ModelInfo], &str, page)`.
- `setup::run_with_host(data_dir, term, host: Option<&str>)` and `setup::DAEMON_RUNNING` added.

## NEEDS

- `NEEDS(integrator)`: add `a2a = { workspace = true }` to `crates/telegram/Cargo.toml` and port
  Signal's `a2a_cmd` (status with peers/exports, `reload`) — `commands::a2a_cmd`.
- `NEEDS(integrator)`: `pulldown-cmark` (or a `markdown` re-export) would let `guard` use the
  event stream instead of its scanner — `ui::guard`.
- ROUTE: act on `Outcome::CloseTopic(session)` (close the topic bound to it); call `ui::guard` on
  every rich send (already planned); fill `CommandCx.session` with thread-or-latest.
- WIRE: `setup::run(&data_dir, &mut setup::Tty)` behind `t --telegram-setup`.

## Gates

```
$ cargo check -p telegram --all-targets
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 20.57s
$ cargo test -p telegram
test result: ok. 67 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.69s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
$ cargo clippy -p telegram --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 9m 36s
$ cargo fmt -p telegram -- --check
(no diff, exit 0)
```

## Remaining

None known.
