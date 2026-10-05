# 98 — As built: WIRE (binary, RPC, bindings, docs)

Status: as-built · 2026-10-05 · worktree `theuth-98-wwire`, branch `feat/98-wire`, base
`a46fc577` (stage-1 contract). Brief: `omp-98-WIRE`. Design: `98-telegram.md` §3, §10.4, §12,
§13, §19; contract `98-contract.md` §2.1, §2.7, §4 Deviations.

## Files

| file | change |
|---|---|
| `crates/theuth/Cargo.toml` | `telegram = { workspace = true }` after `slack` |
| `crates/theuth/src/main.rs` | `--telegram-setup` flag (mirrors `--slack-setup`; `telegram_setup` added to the ten `conflicts_with_all` lists that name `slack_setup`, and to `slack_setup`'s own list); dispatch `telegram::setup::run(&data_dir, &mut telegram::setup::Tty)` right after the slack dispatch; `# telegram:` block in `DEFAULT_CONFIG` after `# slack:` with the §12 keys and comments; `TelegramPlugin { channel: false }` in the `--acp` vector and `{ channel: headless }` in the main vector, each right after `SlackPlugin` |
| `crates/rpc/Cargo.toml` | `telegram = { workspace = true }`; `"telegram/ts"` in the `ts` feature |
| `crates/rpc/src/api.rs` | `use telegram::{PairingLink, TelegramStatus}`; trait + impl for `telegram_status`, `telegram_link(owner: Option<u64>) -> Option<PairingLink>`, `telegram_allow(u64)`, `telegram_remove_allowed(u64)`, `telegram_allow_group(i64)`, `telegram_remove_group(i64)`; `fn telegram() -> RpcResult<ActorRef<telegram::Telegram>>` after `talk()`, error text `telegram is disabled` |
| `crates/rpc/src/bridge.rs` | `use telegram::TelegramEvent;` + `forward!` arm + `rt.subscribe::<Bridge, TelegramEvent>` |
| `crates/rpc/src/export.rs` | `telegram::{TelegramEvent, TelegramStatus, PairingLink, BotIdentity, CallStats}` beside the slack exports |
| `crates/agent/src/user.rs` | module doc, `UserSink` doc, `name()` doc, `via` arg doc, `MessageUserTool` doc, the tool description ("Slack, Signal or Telegram"), and the `NO_CHANNEL` string — text only, no logic |
| `docs/design/00-architecture.md` | `telegram` in both dependency lines (§4 ~180 and §10 ~815) and both `secrets` dependents lists; §10 crate line; §13 table row; `tgbot =0.48.1` pin row in the dependency table |
| `docs/design/24-message-user.md` | `via: "slack" \| "signal" \| "telegram"` (§2, §3 args, trait doc); description sentence; `NO_CHANNEL` text; new `## 6a. Telegram` paragraph matching 98 §8.10 |
| `docs/design/45-voice-notes.md` | §4.5 heading "(every channel: Signal, Slack, Telegram)"; new `## 6a. Telegram` (OGG/Opus inbound → `Transcribe`; `sendVoice` MP3 outbound; 98 §8.8) |
| `docs/design/62-broadcast.md` | §5.3 heading "Slack / Signal / Telegram (owner only)" + one sentence |
| `docs/design/67-agents.md` | §4.4 duty row gains the Telegram clause (single-consumer `getUpdates`, hub-only poller, 24 h retention, `(chat:user, update_id)` dedup); §4.7 heading and a third mermaid branch (Telegram servers → hub polls; others idle) |
| `docs/design/09-signal.md` | §16 "Live owner change" gains the 98 `SetOwner` note |
| `docs/plugins.md` | §3.1 dependency order gains `telegram`; §4 step 4 → "Signal / Slack / Telegram … only if a chat channel needs it" |
| `README.md` | `just run -- --telegram-setup` line; crate list entry; `message_user` sentence; config bullet names `telegram` and the yaml block gains the `telegram:` section; a `Telegram:` paragraph after the `Slack:` one (setup flow, the two @BotFather switches, `telegram.groups`, the `/` menu, where approvals land, the tools and RPC methods) |
| `docs/design/98-as-built-WIRE.md` | this note |

## Judgement calls

- **`telegram()` helper, not `ask::<Slack,_>` style.** The brief said "next to `slack()`", but no
  `slack()` helper exists — slack calls `ask::<Slack, _>(SLACK_ACTOR, …)` directly, which yields
  `telegram not running` on a missing actor. plugins.md §3.2 wants `… is disabled`, so the
  methods go through `send(&telegram()?, …)` like `talk()`/`meeting()`.
- **`telegram_link` reply.** Per the contract's §4 Deviations the actor answers
  `Result<Option<PairingLink>, String>`; the RPC method is `-> RpcResult<Option<PairingLink>>`
  (`Some` = minted link, `None` = owner set directly). The actor's `Err` strings surface as
  `ASK_FAILED` (`-32000`) with the text intact (smoke below).
- **`NO_CHANNEL` string updated** ("slack, signal and telegram are not running"). It is model-facing
  text naming the channels — the same category as the description. The test references the
  constant symbolically, so no test changed. 24 §2 updated to match.
- **Export set** includes `BotIdentity` and `CallStats`: both are `ts(export)` types nested in
  `TelegramStatus`, and `export.rs` lists nested types explicitly (`tools::Choice`, …).
- **README** has a `Slack:` paragraph, not a `## Slack` heading, so Telegram is a sibling
  `Telegram:` paragraph in the same voice, placed right after it.
- **TUI**: `crates/tui` surfaces neither `slack_status` nor `signal_status` (grep: no hits), so
  there is nothing symmetrical to add; skipped.
- **Config template comments**: the §12 comments verbatim, plus a two-line header modelled on the
  slack block (what `--telegram-setup` does; the token is secret `telegram_bot`, never in the file).
- **`bindings/` is gitignored** (`.gitignore:4 /bindings`), so the generated `.ts` files are not
  committed and were not force-added. `just ts` ran clean without a bootstrap `methods.json`
  (nodes' `include_str!` is inside its `#[cfg(test)]` module, not compiled by `cargo test -p rpc`).

## Gates (verbatim)

```
cargo check -p rpc --features ts
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 16s          exit=0
cargo check -p theuth
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 12m 50s         exit=0
cargo test -p rpc --features ts --lib --quiet export_bindings            (= just ts)
    running 8 tests  ........  test result: ok. 8 passed; 0 failed; 0 ignored; 36 filtered out
    → bindings/{BotIdentity,CallStats,PairingLink,TelegramEvent,TelegramStatus}.ts written;
      bindings/methods.json lists the six telegram_* methods
cargo clippy -p rpc -p theuth --all-targets -- -D warnings
    error: draining all elements of a collection into a new collection of the same type
       --> crates/talk/src/policy.rs:580:20   (clippy::drain_collect, rust-1.98.0)
    error: could not compile `talk` (lib) due to 1 previous error            exit=101
    → pre-existing in `crates/talk` (untouched by this branch; lint new in this toolchain);
      the run aborts before rpc/theuth are checked. Re-run with only that lint allowed:
cargo clippy -p rpc -p theuth --all-targets -- -D warnings -A clippy::drain_collect
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 7m 06s          exit=0
cargo fmt -p rpc -p theuth -p agent -- --check                                   exit=1
    → every diff is in code this branch did not touch: crates/agent/src/{acp_child,loop_,
      store,supervisor,sensor/mod,store/accounting,store/turn_usage,loop_/*_tests}.rs,
      crates/theuth/src/mcp.rs, and main.rs hunks at 517/669/692/784/1599 (not mine).
      Proof: `git status --short crates/agent/src/acp_child.rs` is clean (identical to base)
      and `rustfmt --edition 2024 --check` on it exits 1 under rustfmt 1.9.0-stable; the four
      files this branch edits in rpc/agent (api.rs, bridge.rs, export.rs, user.rs) each pass
      `rustfmt --check` individually (exit 0). `cargo fmt` was NOT run (CHECK only).
```

## Smoke (scratch dir, offline)

`t --data-dir /tmp/98-wire-smoke --headless` with `telegram: { enabled: true }`, no token, every
dialling plugin disabled, `rpc.listen: 127.0.0.1:7831` (the shell inherited
`THEUTH_RPC__LISTEN` from the desktop app — unset first, or the config port is overridden).

- `logs/theuth.log`: `started plugin="telegram"` → `WARN telegram: secret telegram_bot must be
  set (t --telegram-setup); telegram not started` → `registered tool tool=telegram_link`,
  `tool=telegram_status` → `rpc listening addr=127.0.0.1:7831`; process kept running until
  killed at 75 s. Zero occurrences of `telegram.org` in the log.
- JSON-RPC over the socket (Bun `WebSocket`, `?token=`):
  `telegram_status` → `{ state: { type: "Unconfigured" }, bot: null, polling: false, owner: null,
  allowed: [], groups: [], threads: [], open: 0, calls: {}, dropped_updates: 0 }`;
  `telegram_link [null]` → `-32000 "telegram is not configured: set secret telegram_bot
  (t --telegram-setup)"`; `telegram_allow [123456789]` and `telegram_allow_group
  [-1001234567890]` → `null`, the next `telegram_status` shows both and `config.yaml` gained
  `allowed:`/`groups:` lines in place; `telegram_remove_allowed` / `telegram_remove_group` →
  `null`, lists back to `[]`.
- `telegram.enabled: false`, restart: log `telegram disabled by config`; `telegram_status` →
  `-32001 "telegram is disabled"`.

Not exercised: `--telegram-setup` end to end (stage-1 `setup::run` is a stub that returns
`not implemented yet`; COMMANDS makes it real — the flag only forwards), `TelegramEvent` on the
event stream (no channel without a token), the bindings in the desktop UI (`just ts-desktop`
not run — not a gate here).

## NEEDS

None. Every type and message the wiring calls (`TelegramPlugin { channel }`, `Telegram`,
`TELEGRAM_ACTOR`, `Status`, `Link { owner }`, `Allow`, `RemoveAllowed`, `AllowGroup`,
`RemoveGroup`, `TelegramStatus`, `PairingLink`, `TelegramEvent`, `BotIdentity`, `CallStats`,
`setup::{run, Tty}`) is `pub` with the derives RPC needs.

## Contract changes

None.

## Remaining

- `cargo clippy … -D warnings` is red on `crates/talk` (`drain_collect`) and `cargo fmt --check`
  is red on pre-existing agent/theuth code under this toolchain — both outside this seam; the
  integrator should run `cargo fmt` once at merge and fix or allow `talk/src/policy.rs:580`.
