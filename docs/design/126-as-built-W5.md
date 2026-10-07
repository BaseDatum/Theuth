# 126 as-built — wave W5 (TUI, channel lines, chat runtime functions, docs, skill, prompt)

Branch `feat/126-w5`, worktree `/Users/Sky.Moore/Develop/theuth-126-w5`, base `5cdcbfde` (C's contract). Design `docs/design/126-double.md` §10 (TUI / Slack / chat columns), §11, Revision 1 (grace 30 s). Contract: `docs/design/126-as-built-C.md`.

Scope: everything outside the desktop that shows or drives double, plus the documentation. No behaviour of W1/W2 (prediction, ladder, training) is built here; every surface consumes `DoublePredicted` / `DoubleAnswered` and calls the `double_*` RPC / seam signatures.

## What was built, where

### §10 chat runtime functions → `crates/double/src/tool.rs`
- `Seams` trait (tool.rs:26-48): one async method per `double_*` RPC signature (`status`, `classes`, `class(key)`, `train(tier, venue)`, `pause(on)`, `forget(target)`, `rollback(version)`, `set_cap(class, rung, mutating_opt_in)`), returning `anyhow::Result<T>` with C's wire types. The tools call the seam, never the store; the RPC handlers will call the same actor messages (C's `// 126 W1 / W2` marker in api.rs).
- `NotYet(DoubleConfig)` (tool.rs:52-91): the seams before the actor exists — reads answer from the config exactly like C's RPC stubs (`status` = enabled + venue, `classes` = `[]`), mutations fail with `double: … is not available yet (126 W1|W2)`. Never a panic.
- Eight tools, `Risk` per 00 §1 item 7: `double_status`, `double_classes`, `double_class(key)` = `ReadOnly`; `double_train(tier, venue?)`, `double_pause(on)`, `double_rollback(version)`, `double_cap(class, rung, mutating_opt_in=false)` = `Mutating`; `double_forget(class | example_ids)` = `Destructive` (irreversible). `CONSUMES_SECRETS = false` on all (arguments are keys and flags). `RETURNS` names the structured shape (`dict`, `list[dict]`, `int`, `str`); every call puts the typed reply in `ToolOutput.structured` and a one-line summary in `content`; a seam error is `ToolOutput::err`, never `Err` (the model must see it).
- Argument validation in the tool, before the seam: `double_train` refuses tier `c`/`baseline` (owner action, §8) and venue `off`/unknown; `double_forget` needs exactly one of `class` / `example_ids`; `double_cap` needs `rung ≤ 3`.
- `register(reg, Arc<dyn Seams>)` (tool.rs:94-109): `RegisterPrompt { plugin: "double", text: PROMPT }` then the eight `RegisterTool`s — the `knowledge::tools::register` (crates/knowledge/src/tools.rs:25-45) / `decision::tool::register` (crates/decision/src/tool.rs:766-782) shape.
- Registration point: `crates/double/src/lib.rs:84-95` at C's `// 126 W5` marker, `rt.lookup_when::<tools::ToolRegistry>(tools::REGISTRY_ACTOR)` in a spawned task, as `DecisionPlugin::start` does (crates/decision/src/lib.rs:719-727). Only reached when `double.enabled` (C's early return), so the prompt paragraph is present only when double is on — the 92 rule.
- `PROMPT` (tool.rs, `## Double`, 7 lines) and `DOUBLE_SKILL = include_str!("../double.md")`.
- Cargo: `kameo`, `tokio` added to `crates/double/Cargo.toml` (`ActorRef<ToolRegistry>`, `tokio::spawn`).
- Tests (`tool::tests`, fake `Seams`, no model, no store): `names_risks_and_schemas` (names, risk classes, every schema an object, required keys), `calls_route_and_validate` (status text + structured, class error as content, train venue/tier refusals, forget exclusivity and count, cap bounds and line), `not_yet_answers_reads_and_refuses_writes`, `prompt_and_skill_shape` (≤ 8 prompt lines, skill front matter, ≤ 40 body lines, the four required phrases).

### §10 chip text → `crates/double/src/chip.rs` (new; shared by every surface)
- `grace_left_s(p, now_ms)`: whole seconds (rounded up, floored at 0) until `will_act_at`; `None` below L2 or on an unparsable stamp.
- `would(p, now_ms)`: `double would: Allow (91 %)` + ` · answers in 18 s unless you do` at L2 — the Slack / Signal / Telegram line.
- `answered(a)`: `double answered Allow (approval:shell:mutating:git push:root)`.
- Test `chip::tests::lines`.

### §10 TUI → `crates/tui`
- Plain (non-interactive) mode prints the chip and the answered line too (`print_event`, lib.rs:1641-1644).
- Events: `Bridge` subscribes to `double::DoublePredicted` / `DoubleAnswered` (lib.rs:101-102) and forwards them as `UiEvent::DoublePredicted` / `UiEvent::DoubleAnswered` (lib.rs:223-224, model.rs:65-68). Unknown events are not subscribed, as before.
- State: `Model.double: HashMap<String, DoublePredicted>` keyed by `question_id` (model.rs:3262); inserted on `DoublePredicted` (a prediction may arrive before or after its card — both orders work), removed on `ApprovalResolved` / `AskResolved` (`id.0.to_string()`) and on `DoubleAnswered`, which also adds an `Entry::Info` line for the shown session or its subtree (model.rs:4463-4473).
- Chip: `double_chip(p, now_ms)` (model.rs:1649-1655) → `double: Allow 91 % · L1`, at L2 `… · L2 · answers in 18 s (answer to take over)`; `Model::double_chip()` picks the shown approval first, else the shown ask (model.rs:3671-3688). Rendered as one italic line in the approval box (ui.rs:650-655, under the args, above the keys) and the ask box (ui.rs:968-976, under "type your own", above the keys).
- Countdown refresh: `Model::animating()` is true while the shown card has `will_act_at` (model.rs:4284, `double_counting_down`), so the 60 Hz frame loop re-renders the text each frame; nothing else animates for it.
- `/double` (model.rs:1480-1484 in `COMMANDS`, dispatch model.rs:6364-6367): `DoubleCmd::parse` (model.rs:1571-1643) → `Command::Double(DoubleCmd)` (model.rs:573) → `execute` (lib.rs:696-731) → one `UiEvent::Info` line. Subcommands: `status` (default) = `double_status` → `double_status_line` (lib.rs:1159-1175); `off` = `double_settings_get` + `enabled: false` + `double_settings_set` and says it takes a restart (§12: `enabled` is not live); `pause [off]` = `double_pause(on)`; `train [a|b] [auto|local|node:<name>]` = `double_train`; `rollback <version>`; `cap <class> <rung> [opt-in]` = `double_set_cap`. A usage error is an `Entry::Error` with `DoubleCmd::USAGE`.
- Cargo: `double` added to `crates/tui/Cargo.toml`.
- Tests (`model::tests`): `double_cmd_parses_the_table` (every row and seven refusals), `double_chip_follows_the_shown_card_and_counts_down` (prediction before the card, chip text, L2 countdown arithmetic and `animating()`, removal on resolve, the answered line). No widget render harness exists in the crate for the approval box (the search probe is the only `element!().to_string()` test), so the chip line is tested at the model level.

### §10 channel lines
- **Slack** `crates/slack/src/approvals.rs`: `card` now delegates to a body-based `blocks(body, id, double, context)` core (approvals.rs:18-60) so `predicted_card(body, id, p, context)` rebuilds a posted card from the `(ts, body)` the channel keeps, with the double's line as a `context` block between the actions and the session context: `[markdown, actions(Allow/Deny), context(double)[, context(session)]]` — design 110's card shape plus one block; the first two blocks are byte-identical to `card`'s, so clicks keep working. `double_context(p)` (one `mrkdwn` element, text from `chip::would`), `with_double(blocks, p)` (appends it to an ask card from `asks::question_card`), `answered_text(a)` = `🤖 double answered …`, `answered_blocks(a)` (one markdown block). Test `double_line_is_one_context_block_before_the_session_context` pins the block order, types and texts. The crate has no `blocks.validate` helper (searched `fn validate|blocks::validate`); the shape test is the equivalent.
- Slack wiring `crates/slack/src/channel.rs` (see Deviations 2): `Event::Predicted` / `Event::DoubleAnswered` (channel.rs:237-240), `Bus` forwards (305-317), subscriptions (559-560). On `Predicted`: an owner-DM approval card is updated in place from `Card.dm` (now `(ts, body, context)`, channel.rs:385, 1865), a thread approval card via `Out::Predicted` to its worker (347, 2121, `Worker::predicted` 2374-2382, from its `approvals: (ts, body)`), an ask card (thread or DM) in place from `AskCard { channel, ts, req, current, context }` (1716-1729). On `DoubleAnswered`: `Out::Note(answered_text)` into the session's thread, else `answered_blocks` into the owner DM (1736-1750). Lookups are by `uuid::parse_str(question_id)`; an unknown or unposted card is ignored.
- **Signal** `crates/signal/src/channel.rs`: `Event::Predicted` / `DoubleAnswered` (402-404), `Bus` forwards (464-476), subscriptions (647-648). Signal messages are not edited, so each line is its own text to the session's recipient: `🤖 double would: Allow (91 %)` only for an approval, or for the ask currently open on that phone (`asks.answerable(number).req.id == question_id`), never for a queued ask; `🤖 double answered …` (1605-1629).
- **Telegram** `crates/telegram/src/channel.rs`: `Event::Predicted` / `DoubleAnswered` (96-98), `forward!` (125-126), subscriptions (452-453). A `note` to `card_target(session)` — the chat the card went to — only when this channel posted that card (`cards[id].at` / `asks[id].at` is `Some`); the answered line to the same target (2385-2413).
- Cargo: `double` added to slack, signal and telegram.

### §11 docs, skill, prompt, release note
- `docs/double.md`: what it is; the two dependencies (0033 journal, `tools.approvals`); what it learns from; what it never does (secrets/handoffs/MFA, destructive > L1, mutating without opt-in, never rewrites the question, never answers twice, excluded rows); the ladder in plain words with the 30 s grace and the demotions; the controls table (desktop / TUI / chat / config); training tiers, venue rules incl. "never a sandbox VM", scrubbing, retention, export, delete all; FAQ "why did it answer X", "why did it stop acting", "how do I make it forget", "no chip", "does it slow cards".
- `crates/double/double.md` — the skill, front matter `name: double`, body 33 lines (≤ 40): reading the chip and `none`, the four rules (a double answer is the owner's answer; never argue; one-line disagreement at L1; never shape a card into a class), the functions. Installed by `first_run` beside `decide.md` (crates/theuth/src/main.rs:1219, doc comment 1203) — the `decision::tool::DECIDE_SKILL` precedent.
- The `## Double` prompt paragraph: `double::tool::PROMPT`, 7 lines, registered with `RegisterPrompt { plugin: "double" }` only when `double.enabled` (the plugin returns before registration otherwise) — mirrors `decision::tool::PROMPT` (crates/decision/src/tool.rs:786-792, registered at lib.rs:709-727 under the same rule). Verbatim:

```
## Double
The owner has a double: a local model of their own past decisions that predicts their answer to every approval and ask card before they see it.
A chip `double · Allow · 91 %` on a card is its prediction; `none` means it is unsure and the owner decides.
At L2 it answers for the owner after a 30 s grace; an answer marked `double` is the owner's answer — act on it and never argue with it or ask again.
When you disagree with an L1 suggestion, say so in one line and let the owner decide.
`double_status()`, `double_classes()`, `double_class(key)` are its numbers; `double_pause`, `double_train`, `double_forget`, `double_rollback`, `double_cap` change it and are only for when the owner asks.
Load the `double` skill for the ladder and the controls.
```

- Release-note stub: `docs/release-notes-2.10.0.md` (the tree's pattern is `docs/release-notes-<version>.md`, one file, 2.0.0; the workspace is 2.9.2, so the stub is a new file for the next minor — rename if the release is numbered differently). One paragraph under `### The owner's double (design 126)`.

## Deviations from the design / brief

1. **Seams, not actor messages.** The contract gives no actor message types (W1's), so the tools call a `tool::Seams` trait mirroring the RPC signatures; `NotYet` fills it until W1 hands the actor's handle to `tool::register` (lib.rs:84-86). One `impl Seams for <W1 handle>` is the whole hand-off; no tool code changes.
2. **Slack wiring outside `approvals.rs` / `asks.rs`.** The brief named the two pure modules; the chip arrives after the card is posted (design §7.1: it never delays the card), so appending the context line means a `chat.update` of the posted card from the channel. `channel.rs` got the event plumbing and three update paths (~90 lines); no sibling wave touches `crates/slack`. `Card.dm` carries the session context so the DM card keeps it after the update.
3. **Signal and Telegram lines are their own messages**, not edits of the card text: Signal cannot edit; Telegram could, but a follow-up note is the few-line change the brief asked for and the same text on both. The approval card itself is unchanged.
4. `/double` also has `rollback <version>` (design §10 lists it; the brief's list did not) and `pause off` (resume); `off` writes `enabled: false` through `double_settings_set` and says a restart is needed (§12).
5. `double_train`'s `venue` is optional (default `auto`), and the tool refuses tier `c` (design: owner action only) — the RPC accepts any tier; the refusal is in the tool so the model cannot start a full fine-tune from chat.
6. `crates/theuth/src/main.rs` (C's file) got one list entry + one doc line for the skill install; `crates/double/src/lib.rs` (C's) the W5 marker code only.
7. The TUI `DoubleAnswered` info line is shown for the current session and its subtree only (as approvals are), not for every session.

## Gates (all through `BUILD_LABEL=126-W5 /Users/Sky.Moore/.theuth/bin/build-lock`, `CARGO_BUILD_JOBS=4`, `CARGO_INCREMENTAL=0`, target `/Users/Sky.Moore/Develop/theuth-126-w5-target`, in the foreground)

| # | command | result |
|---|---|---|
| 1 | `cargo test -p double` | 16 passed, 0 failed (11 of C's + `tool::tests` ×4 + `chip::tests::lines`); 0 doc-tests; no warnings |
| 1' | `cargo clippy -p double --all-targets -- -D warnings` | clean |
| 1'' | `cargo fmt -p double` | applied (only `double`; no other crate formatted) |
| 2 | `cargo test -p double -p tui -p slack -p signal -p telegram` | lib tests: double 16, signal 63, slack 143, telegram 121, tui 131 — all passed, 0 failed. `tui/tests/no_emoji.rs` (`no_emoji_in_terminal_or_chrome_strings`) **fails at the base commit too**: 21 emoji lines, every one in `crates/surface/src/{elk_family,export,render}.rs`, which W5 does not touch (`git diff --stat 5cdcbfde -- crates/surface` is empty); no W5 string in the scanned crates (`tui`, `surface`, `agent`, `rpc`) carries an emoji — the TUI chip uses `·`, the `🤖` prefix lives in slack/signal/telegram, which already use emoji and are not scanned |

Two compile fixes along the way, both caught by the gate: the tool tests' `call` was ambiguous between `Tool::call` and `ErasedTool::call` (specs now go through a `&dyn ErasedTool` closure), and `print_event`'s exhaustive `UiEvent` match (plain-mode TUI) needed the two new arms. One logic fix caught by a W5 test: `/double cap` split the class key on whitespace, but a shell class key holds a space (`…:git push:root`) — `cap` now parses its trailing `<rung> [opt-in]` first.

No model checkpoint is loaded by any test here; no decision-crate test was run.

## Open items / hand-offs

- **W1 / integrator:** `impl double::tool::Seams for <the actor handle>` (eight methods = the eight `double_*` RPC calls) and replace `tool::NotYet(cfg)` at crates/double/src/lib.rs:86 with it. `double_settings_set`'s live semantics for `capture` / `ladder` / `training` are the actor's; `/double off` only persists `enabled: false`.
- **Integrator (`crates/rpc/src/client.rs`, not W5's file):** add `double::DoublePredicted, double::DoubleAnswered` to the `republish!` list (client.rs:260-290) so an **attached** TUI (remote engine over WebSocket) gets the chip; in-process the TUI's own `rt.subscribe` already does. Two lines; `rpc` already depends on `double`.
- **W1:** `DoublePredicted.question_id` must be `ApprovalId.0.to_string()` / `AskId.0.to_string()` (uuid hyphenated) — every surface here keys on that; `session` must be `Some` for approval/ask cards (the channels route by it).
- **W4 / Settings › Labs:** not touched; the `set_decision_mode` `double` prefix is still C's open item.
- The TUI has no render harness for the approval box; a snapshot test can follow if one is added.
- `docs/release-notes-2.10.0.md` is a guess at the next version number.
