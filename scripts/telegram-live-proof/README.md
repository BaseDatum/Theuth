# Telegram live proof (98c)

Runs a scratch theuth engine against a local [telegram-mock-ai](https://github.com/skrashevich/telegram-mock-ai)
and drives the slash-command wedge scenario end to end: P0 connect, P1 model turn,
P2 `/sessions`, P3 panel button, P4 model turn after the command, P5 `telegram_status`
latency, P6 restart + turn. Prints a PASS/FAIL table; exit 0 only when all pass.

```sh
scripts/telegram-live-proof/proof.sh main            # label = data dir + evidence dir name
scripts/telegram-live-proof/proof.sh fix --no-build  # reuse the last engine binary
```

## Prerequisites

- `go` (`/opt/homebrew/bin/go`), `bun`, `sqlite3`, `perl`, `curl`
- the toolchain the script exports:
  `PATH=/Users/Sky.Moore/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin:$PATH`,
  `CARGO_TARGET_DIR=<repo>-target` (override with `CARGO_TARGET_DIR`)
- a `~/.theuth` with `config.yaml`, `acp/agents.yaml`, `skills/` (`THEUTH_SRC_HOME` overrides).
  Only those are copied; the live dir is never written. The vault is **not** copied (its key is a
  per-data-dir Keychain item): set `ANTHROPIC_API_KEY`; the scratch engine gets a file-keyed vault
  seeded from it and runs `anthropic/claude-haiku-4-5` (`PROOF_MODEL` overrides).
- a tree that has the `telegram.api_host` config seam (`feat(telegram): telegram.api_host config seam`).

## What it does

1. Clones the mock to `/tmp/98c-proof/telegram-mock-ai`, applies `mock.patch` (idempotent),
   builds `/tmp/98c-proof/mock-bin`, starts it with the LLM, proactive traffic and welcome
   messages off (`mock-config.yaml`: Bot API 127.0.0.1:8081, Admin API 127.0.0.1:8082,
   owner 777000111 in a private chat with the same id, bot token `123456:ABC-DEF…`).
2. `cargo build -p theuth` (skip with `--no-build`).
3. Scratch data dir `/tmp/98c-proof/data-<label>`: copies the three items above, writes
   `credentials.yaml` from `ANTHROPIC_API_KEY` (imported and unlinked by the engine on first
   open), appends `telegram: {enabled, api_host, owner}` to the config.
4. Starts `t --headless --data-dir …` with `RUST_LOG=info,telegram=debug,tgbot=debug` and these
   env overrides so the scratch engine cannot collide with a live one on the same box:
   `THEUTH_RPC__LISTEN=127.0.0.1:0`, `THEUTH_SLACK__ENABLED=false`,
   `THEUTH_MESH__DISCOVERY=false`, `THEUTH_MESH__RELAY=none`.
5. Samples `telegram_status` over RPC every 2 s (`latency.csv`), sets the bot token via
   `set_secret` (the channel hot-starts), and runs the steps with hard timeouts
   (`MODEL_TIMEOUT`, default 120 s, for model turns; `STEP_TIMEOUT`, default 10 s, otherwise).
6. Writes evidence to `/tmp/98c-proof/out-<label>/`: `table.md`, `proof.log`, `ledger*.txt`
   (the mock's Bot API call ledger, with and without `getUpdates`), `messages*.json`,
   `latency.csv`, `engine-*.out`, the engine's `theuth.log.<date>`, and on a P1/P2/P4/P6b
   failure a macOS `sample` of the engine's threads (`threads-<step>.txt`; demangle with
   `cargo install rustfilt && rustfilt -i threads-P2.txt`).
7. SIGTERMs only the processes it started and removes the data dir (`--keep` keeps it).

## Files

- `proof.sh` — orchestrator (steps, timings, table).
- `rpc.ts` — `bun rpc.ts call|wait|latency <data_dir> …`: JSON-RPC over the engine's WebSocket
  (`engine.json` + `token`), every call with a hard timeout.
- `inject.ts` — `bun inject.ts message|callback|seq|await|calls|messages|ledger …`: the mock's
  Admin API (inject a user message or a `callback_query`, wait for a Bot API call in the ledger).
- `mock.patch` — against mock commit `314c594`: Bot API call ledger (`GET/DELETE /api/calls`),
  `reply_markup` stored on `sendRichMessage`, `bot_command` entity on injected `/commands`
  (+ optional `entities`), `TELEGRAM_MOCK_WELCOME_ENABLED=false`.
- `mock-config.yaml` — the mock's seed.

## Reading a failure

- `P1`/`P4`/`P6b` `no setMessageReaction (👀)` — the update was polled (see `getUpdates`
  offsets in `ledger-with-getupdates.txt`) but never dispatched: the inbound loop is wedged.
- `P2` `no sessions panel` — `/sessions` was dispatched (or not) and no `sendRichMessage`
  followed: look for the last `telegram` debug line in `theuth.log.*`.
- `P5` errors — `telegram_status` stalled: a lock, not a dispatch wedge.
