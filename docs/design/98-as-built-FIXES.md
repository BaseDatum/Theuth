# 98c — as built, FIXES (2026-10-06)

Bug report (Sky, release 2.9.0, Telegram channel on another Mac): (1) a regular message is
answered, then the first slash command gets no reply and **no later message is answered**; the
poller keeps acking (`getUpdates` with an advancing `offset`), nothing is sent. (2) Settings ›
Channels › Telegram "would not load" after pairing and tapping **Sessions**; after a restart
"would not even show up". Branch `fix/98c-telegram-wedge` off `ef1ad0a3`.

## Root cause

`crates/telegram/src/channel.rs:1564-1574` (v0.1, `command_cx`):

```rust
CommandCx {
    user,
    owner: *self.owner.lock(),   // parking_lot guard: a TEMPORARY of the struct literal
    …
    status: self.status(),       // status() does `*self.owner.lock()` again → self-deadlock
    …
}
```

In plain words: a temporary created inside a struct literal lives until the whole literal is
built, so the `owner` mutex guard was still alive when `self.status()` (channel.rs:695-696)
tried to lock the same mutex. parking_lot mutexes are not re-entrant: the task blocked for ever
**synchronously**, pinning its OS thread. `command_cx` is called by every slash command
(`command`, channel.rs:1580), every prompt reply (`continue_prompt`, :1123) and every `cmd:`
panel press (`press_cmd`, :1995) — the two paths in the report. Because inbound dispatch was one
sequential loop (`while let Some(inc) = inbound_rx.recv().await { chan.handle_inbound(inc).await }`,
channel.rs:453-455), the wedged handler also meant every later update was received, acked,
queued and never handled — symptom 1 exactly. The same held `owner` guard blocked
`Handle::status()` → `impl Message<Status> for Telegram` (lib.rs:471-480) → the `telegram_status`
RPC never resolved → `Channels.tsx` rendered `{s && …}` = nothing — symptom 2, first part.

Why no test caught it: no channel test ever sent a command through `handle_message`; the pairing
test (`/start <code>`) returns before `command_cx`, the commands tests build `CommandCx` by hand.

## Hypotheses — verdicts

| | verdict | evidence |
|---|---|---|
| H1 lock across await / nested lock in the command or callback path | **confirmed — the root cause** | `command_cx` above. Repro tests `regular_then_sessions_then_regular_all_answered`, `regular_then_help_then_status_then_regular_all_answered`, `regular_then_sessions_then_panel_press_then_regular_all_answered`, `status_answers_within_a_second_while_a_command_is_in_flight` (channel.rs, "98c" section) all **FAILED on the base code** with `handle_inbound did not return within 10s` / `the Status step finished` (run 2026-10-06 before the fix: 3 FAILED, the 4th hung the harness thread, see "test design" below); all pass after. No lock is held across an `.await` anywhere: `cargo clippy … -D clippy::await_holding_lock` on the crate found nothing in production code (one hit in a test stub I wrote, fixed). |
| H2 an untimed Port ask that never resolves | **refuted as the cause; real hazard, hardened** | The fixture's `Stub` Port answers `list_sessions` immediately (channel.rs `impl Port for Stub`) and the wedge still reproduced, so no ask was pending. `LivePort` (port.rs:68-182 v0.1) and 42 direct `agent()?.ask(..)` arms in commands.rs were untimed — now every Port ask and `MarkAnswered` run under `port::timed` (20 s, WARN at 5 s) and the whole command under `COMMAND_BOUND` (21 s). Test `hung_or_slow_port_ask_does_not_block_the_next_message` (Port pending for ever, and 3 s). |
| H3 a panic in `handle_inbound` ending the inbound task | **refuted** | The panic hook in `crates/theuth/src/main.rs` logs every panic at ERROR; none in Sky's log. In the repro the handler thread never returned (deadlock), it did not unwind. The JoinHandles were indeed never observed (channel.rs:462-473 v0.1: only `abort()`ed at shutdown) — now `watch()` logs ERROR when a loop ends or panics and `guarded()` logs a handler panic with `kind` + `update_id`. |
| H4 the command reply retried for ever inside `send`/tgbot | **refuted** | `Channel::send` (channel.rs:727-743 v0.1) is one `api.send_rich` plus bookkeeping; the tgbot client is built `with_max_retries(2).with_max_retry_after(30)` (api.rs:375-383): bounded. `reply` logs `telegram command reply failed` at WARN on failure (:1807-1808) — Sky's log has no WARN and no `sendRichMessage` for the command at all: the path never reached `reply`. |
| H5 boot: `Status` unanswered or erroring until network work finishes; UI hides the card | **confirmed (symptom 2, second part) — fixed** | lib.rs:591-647 v0.1: `Plugin::start` spawned a task that awaited `secrets::handle(&rt)` (`HANDLE_WAIT` = 10 s worst case) **before** spawning and registering the actor; until then `rpc/src/api.rs:2089-2094` answers `NOT_RUNNING "telegram is disabled"` and `Channels.tsx:356` `{s && (…)}` renders nothing on pending and on error. Fixed on both sides: the actor is registered synchronously in `start`; the card shows `loading…` / the error + Retry. [INFERENCE] "after restart it would not even show up" is this window plus, once the first command after restart wedged `status()` again, a query pending for ever. Test `status_answers_within_a_second_while_get_me_is_slow` (fake host delays getMe 5 s; `Status` answers in < 1 s with `bot: None, polling: false`). |

## What changed

### `crates/telegram`

- **channel.rs**
  - `Channel.owner` is an `AtomicU64` (`0` = none) behind `owner()` / `set_owner()`; the
    second mutex is gone, so there is no lock order left to keep. The lock discipline is one
    comment on `State` (never across an `.await`, never nested, beware struct-literal
    temporaries). `command_cx` takes `owner` and `status()` into locals before the literal.
  - `dispatch_inbound`: a lane (`mpsc` + task) per chat key for plain messages, Stop and
    `my_chat_member` (§7 order kept where it matters: consecutive messages in one thread see each
    other's binding); commands and callback presses are spawned free. `lane_key` decides.
    `ponytail:` lanes are never reaped (one idle task per chat key ever seen).
  - `watch(name, JoinHandle, shutdown)`: ERROR when a dispatch loop (inbound / events / notes)
    ends or panics while the channel lives; `run` keeps the abort handles. `guarded(kind,
    update_id, fut)` + `handle_guarded`: a handler panic is an ERROR with the update kind, the
    lane survives. events/notes loops are guarded too.
  - `COMMAND_BOUND` (= `ASK_TIMEOUT` + 1 s) around `commands::handle` and `ui::action`;
    `command_timed_out` → `error: engine did not answer in 20 s` + WARN. `MarkAnswered` goes
    through `port::timed`.
  - Logging: `info!` `telegram command name= user= chat= outcome=`, `telegram press kind= user=
    chat=`, `telegram panel action data= … outcome=`; `debug!` at every early exit of
    `handle_message` / `route_message` / `press_cmd` / `handle_callback` / `Stopped` naming the
    §7 step.
  - `spawn()` honours `cfg.api_host` (the seam commit).
- **port.rs**: `ASK_TIMEOUT` 20 s, `ASK_SLOW` 5 s, `ASK_TIMED_OUT`, `timed(what, ask)` — a WARN
  at 5 s while still waiting, a WARN and `Err(ASK_TIMED_OUT)` at 20 s; every `LivePort` method
  uses it.
- **poll.rs**: `Incoming::kind()` / `update_id()` for logs.
- **commands.rs**: `Outcome::variant()` for logs.
- **lib.rs**: `#![deny(clippy::await_holding_lock)]`; `TelegramConfig.api_host`; `Plugin::start`
  spawns and registers the actor first (`handle: None`), then on a task subscribes, reads the
  vault and `tell`s `TokenRead(token)`; `TokenRead` → `hot_start` (daemon) or `configured` +
  `DaemonOnly` (terminal). Boot and hot start are now one path.
- **Tests** (channel.rs, "98c" section): 1a, 1b, 1c with `Fixture::inbound_timed` (handler on
  its own OS thread + runtime, `recv_timeout(10 s)`); 1d through the real `dispatch_inbound`
  (`Fixture::start_dispatch`, `Stub.list_delay` = pending for ever and 3 s; the regular message
  is answered in < 2 s); 1e `Status` on the actor within 1 s while `/sessions` runs; 4 `Status`
  within 1 s while wiremock delays getMe by 5 s.

  *Test design.* A parking_lot deadlock blocks the OS thread, so a `tokio::time::timeout` on
  the same runtime never fires and dropping a multi-thread runtime joins the stuck worker; the
  repro steps therefore run on a throwaway thread and the test waits with
  `std::sync::mpsc::recv_timeout` — a hang fails the step in 10 s, never the suite.

### `crates/desktop/ui`

- `components/settings/Channels.tsx` `<Telegram/>`: `status.isPending` → `Row "Bot" loading…`;
  `status.isError` → `Row "Bot" status unavailable: <message>` with a **Retry** button
  (`status.refetch()`); the card body still renders when data exists. Never nothing.
- `Channels.test.ts`: the pending paint shows `loading…`; a rejecting `telegram_status` paints
  the error text and the Retry button (`retryOnMount: false` so a fresh server-render observer
  does not optimistically show the errored query as pending again).

### `docs/design/98-telegram.md`

Dated **Revision 2026-10-06 (98c)** paragraphs in §3 (boot order), §7 (dispatch lanes, ask
deadlines, logging), §10.3 (`command_cx`, the timeout reply), §11 (one lock, owner atomic), §13
(`Status` always answers, new consts, the card states); `owner: AtomicU64` in the §13 types; a
98c line in §20; `api_host` in §12 (seam commit).

## Files touched outside the brief's list

- `crates/telegram/src/poll.rs` (`Incoming::kind/update_id`), `commands.rs`
  (`Outcome::variant`), `port.rs`, `lib.rs` — all inside `crates/telegram`, which the brief
  names as the crate. `crates/desktop/ui/src/components/settings/Channels.test.ts` as asked.
  Nothing else.

## Gates (run from the worktree after the last code commit)

```
export PATH=/Users/Sky.Moore/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin:$PATH
export CARGO_TARGET_DIR=/Users/Sky.Moore/Develop/theuth-98c-target
```

Result lines (code state = commit `69ee546e` + docs; this note's results commit is docs-only):

```
cargo test -p telegram
  test result: ok. 119 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.63s
  test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s   (doc-tests)
cargo clippy -p telegram --all-targets -- -D warnings -D clippy::await_holding_lock
  error: could not compile `tasks` (lib) due to 4 previous errors        ← exit 101, see below
cargo clippy -p telegram --all-targets --no-deps -- -D warnings -D clippy::await_holding_lock
  Finished `dev` profile [unoptimized + debuginfo] target(s) in 38.88s   ← exit 0: zero lints in `telegram`
cargo fmt -p telegram -- --check
  (no output) exit 0
cargo check -p rpc -p theuth
  Finished `dev` profile [unoptimized + debuginfo] target(s) in 18.74s   exit 0
cd crates/desktop/ui && bun run check
  $ tsc -b --noEmit   exit 0
bunx vitest run src/components/settings/Channels.test.ts
  Test Files  1 passed (1)
  Tests  8 passed (8)
```

The brief's clippy line as written fails **in `crates/tasks`** (`collapsible_match` at
`tasks/src/packet.rs:406`, two `doc_lazy_continuation` at `tasks/src/tools.rs:480-481`,
`too_many_arguments` at `tasks/src/tools.rs:741`): `cargo clippy -p <crate>` also lints
workspace path dependencies, and `-D warnings` turns those pre-existing lints into errors.
`crates/tasks` is untouched on this branch (`git diff --stat ef1ad0a3 -- crates/tasks` is
empty), so the same line fails identically on `ef1ad0a3`; it is outside this fix's file list.
The `--no-deps` run is the same lint set restricted to `telegram` and is clean, including
`await_holding_lock` (also `#![deny]`ed in `lib.rs`; rustc ignores clippy lint names, so only the
clippy gate enforces it — `cargo test` does not).
The UI gates ran after `bun install` in `crates/desktop/ui` and `packages/apps` and one
`just ts-desktop` (bindings unchanged by this fix).

## Review round (98c-r2, 2026-10-06 13:34 review; code commit `d87bc64f`, UI/docs commit `6f8a7c6f`)

Worktree HEAD had moved to `e7e96d89` (live-proof branch + local `main` folded in); built on it,
no rebase, no amend.

| # | item | what changed | where |
|---|---|---|---|
| 1 (blocking) | prompt replies ran on the chat lane with an unbounded `continue_prompt` | `Channel::lane_key` sends a reply to a live `force_reply` prompt (`prompts.contains_key`) free; the continuation runs under `COMMAND_BOUND` exactly like `command()` and logs `telegram prompt reply expect=… outcome=…`. Test `hung_prompt_reply_does_not_block_the_next_message`: `/skills add` → prompt, the reply's handler wedged (the mock's `hang_next("send_rich")` — the fixture has no engine actor to hang `AddSkillRepo` on, so the hang is injected at the handler's reply send), then a plain message in the same chat is answered and the prompt was consumed. Negative check done: with the lane_key prompt test disabled the test fails at the 10 s step. | `d87bc64f` channel.rs `dispatch_inbound`/`lane_key`, `handle_message`; api.rs `MockSender::hang_next` |
| 2 | H5 untested | `plugin_start_registers_the_actor_before_the_vault_answers` (lib.rs): `TelegramPlugin::start` on a runtime with no secrets actor; `telegram_actor()` is found at once, `Status` answers `Starting` within 1 s and well under `HANDLE_WAIT`; `Link` answers `telegram is starting`. Tests that register the `"telegram"` name serialise on `channel::tests::REGISTRY` and `kill_registered` waits for the name to clear (kameo unregisters on stop). | `d87bc64f` lib.rs tests; channel.rs tests |
| 3 | 1e had no command in flight | `list_delay = Some(Duration::MAX)` before `/sessions`; `Status` still answers in < 1 s while that free task is pending for ever. | `d87bc64f` channel.rs `status_answers_within_a_second_while_a_command_is_in_flight` |
| 4 | timing margins | 1d asserts ordering: `Stub.list_done == 0` when the regular message's input landed (the 3 s and the never case) — no wall-clock bound. 1e / test 4 keep the 1 s deadline the brief specifies: both steps are pure in-process actor asks (no network, no disk), so 1 s is two orders of magnitude of margin; under a scheduler stall that long the whole suite is already degraded. | `d87bc64f` |
| 5 | boot flash "no bot token" | `Telegram.configured: Option<bool>`; `Telegram::starting()` at boot → `TelegramEvent::Starting` (status-only, never on the bus) until `TokenRead`; `Link` says `telegram is starting`. UI: `starting…` phrase, token row `checking the vault…` with no input/Replace, Pair disabled. Bindings regenerated (`just ts-desktop`), `Channels.test.ts` covers the phrase and the card. | `d87bc64f` lib.rs; `6f8a7c6f` Channels.tsx/test, bindings |
| 6 | remaining lane-blocking asks | `transcribe()` under `TRANSCRIBE_BOUND` = 60 s → attachment line `(transcription timed out)`; `pair()`'s `SetOwner` and `admin()`'s `Allow/RemoveAllowed/AllowGroup/RemoveGroup` under `port::timed` (20 s). §20 now says "every `Port` ask and every lane-blocking ask is timed (the direct command arms sit under the one `COMMAND_BOUND`)". | `d87bc64f` channel.rs; `6f8a7c6f` 98-telegram.md §7, §20 |
| 7 | docs | §7: ordering caveat (`/new`, `/resume`, `/cancel` vs a same-second message; `/new` wins by an HTTP round-trip in practice, not guaranteed); "(step 519 above)" → "the `session = threads[key]` step of the block above"; §11: nesting left is state → `Api.calls` (`api.stats()` inside `status()`), never the reverse; this note no longer claims `cargo test` enforces the clippy deny. | `6f8a7c6f` |
| 8 | one constant; two asks; lane send | `port::ask_timed_out()` formats from `ASK_TIMEOUT`; §7 states `send_input(None, …)` is two timed asks (up to 40 s on the lane) — kept as two because `NewRootSession` and `SendInput` are two actor round-trips with their own failure texts; `lane.send` error is logged at debug with kind + update_id. | `d87bc64f` port.rs, channel.rs; `6f8a7c6f` §7 |

Gates after the last r2 commit:

```
cargo test -p telegram
  test result: ok. 121 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.70s
  test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s   (doc-tests)
cargo clippy -p telegram --all-targets -- -D warnings -D clippy::await_holding_lock
  error: could not compile `tasks` (lib) due to 4 previous errors        ← exit 101; pre-existing `crates/tasks` lints, unchanged on this branch (see the first round)
cargo clippy -p telegram --all-targets --no-deps -- -D warnings -D clippy::await_holding_lock
  Finished `dev` profile [unoptimized + debuginfo] target(s) in 38.08s   exit 0
cargo fmt -p telegram -- --check
  (no output) exit 0
cargo check -p rpc -p theuth
  Finished `dev` profile [unoptimized + debuginfo] target(s) in 43.02s   exit 0
just ts-desktop
  cp -R bindings/. crates/desktop/ui/src/rpc/bindings/   exit 0 (bindings are gitignored; TelegramEvent.ts now carries `Starting`)
cd crates/desktop/ui && bun run check
  $ tsc -b --noEmit   exit 0
bunx vitest run src/components/settings/Channels.test.ts
  Test Files  1 passed (1)
  Tests  9 passed (9)
```

## Not exercised

- No real bot token exists on this machine: **no Telegram API call was made**. The wiremock host
  only serves `getMe` in the new test; the live proof (real Agents actor, `LivePort`, RPC) is
  the sibling worker's, via the `telegram.api_host` seam commit
  `feat(telegram): telegram.api_host config seam (tests/proofs only)`.
- The fixture's `Port` is a stub: the 20 s `ASK_TIMEOUT` reply text is exercised by the 1d test
  only as "the next message is not delayed", not by waiting 20 s for the text.
- The restart half of symptom 2 is explained ([INFERENCE] above), not reproduced end to end.

## What Sky should see when re-testing (`RUST_LOG=info,telegram=debug,tgbot=debug`)

- At boot, before the vault answers: the Settings card shows **Bot · loading…**, then the real
  state; if the engine is down it shows the error text and **Retry** instead of blank.
- Per slash command one line `telegram command name=sessions user=… chat=… outcome=Reply`;
  per button press `telegram press kind=cmd user=…` then `telegram panel action data=sessions …
  outcome=Reply`. A command that the engine does not answer logs `telegram: engine ask still
  unanswered ask=ListSessions waited_s=5` at WARN after 5 s and `telegram: engine ask timed out`
  at 20 s, and the chat gets `error: engine did not answer in 20 s`; messages sent meanwhile are
  answered normally.
- A dropped update always leaves a `debug!` naming why (`not the hub`, `unaddressed … group
  message`, `message with nothing to send`, …). An ERROR line `telegram dispatch loop ended` or
  `telegram handler panicked` is a bug report by itself.
