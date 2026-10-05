# 98 — As built: ROUTE (the channel)

Worktree `theuth-98-wroute`, branch `feat/98-route`, base `a46fc577` (stage-1 contract).
Scope: `crates/telegram/src/{channel,approvals,asks,user}.rs` and the two `mod` lines in
`lib.rs`. Nothing else edited.

## Files

- `channel.rs` — the real channel over the stage-1 skeleton: trust (§5), the thread key and
  shapes (§6), inbound routing (§7) with files/voice (§7.1), commands and `/start` (§7.2,
  §10.3, §10.4), callbacks (`ap:` / `ask:` / `cmd:` / `out:`), events → workers, subagents
  (§9.1 topics, §9.2 flat markers, §9.3 `ChildRow` fan-out), approvals (§10.1), asks (§10.2),
  `ArtifactShown` (§8.10), secret notices, `WorkerNote`s, the §4.3 unreachable / topic-gone
  table on every channel send, album coalescing, the `message_user` helpers; `channel::tests`.
- `approvals.rs` — pure: `body`, `card`, `keyboard(id)` (`ap:<id>:a|d`), `resolved` (verdict
  line + one `Disabled` button), `parse`; the remembered `Card`.
- `asks.rs` — pure: `question(req, i, context) -> (String, Keyboard)` (option rows ≤ 8,
  Skip/Dismiss row, `force_reply: true`), `answered`, `closed`, `parse`; `Card` with `step`
  (the signal/slack one-question-at-a-time pacing).
- `user.rs` — `impl UserSink for Handle`: bound pass over `m.sessions` with `target_for`,
  owner-chat fallback with the `_<origin>_` line, `send_rich` → plain chunks on a 400, one
  `send_file` per file, `telegram: <chat title or @user>` / `telegram: owner`.
- `lib.rs` — `pub mod approvals; pub mod asks;` only.

## Judgement calls

- **`Channel.api` is `Arc<dyn Sender>`** plus `live: Option<Arc<Api>>` (slack's `api` /
  `live` split) so `channel::tests` run on `api::mock::MockSender`; the poller and
  `on_event` use `live` and are skipped under a mock. `Channel::new` returns the
  `WorkerNote` receiver; `run` drains it into `handle_note`.
- **Flat children (§9.2) have no thread key.** They live in `State.flat_children`
  (child → parent's `Target` + user); their worker is keyed `"<chat>#<session>"` in
  `State.workers` and gets the parent's `to` with `child_name: Some(name)`. `bound(session)`
  answers for threads first, then flat children, so approvals / asks / artifacts /
  `message_user` all land in the parent's chat for a flat child. `threads.json` is untouched.
- **Child topics are `named_by_user: true`** so RENDER never renames `↳ <name>` after the
  session title.
- **`answerCallbackQuery` carries the toast**, so for `ap:` it is sent after `port.resolve`
  (a local actor ask) — still the first *Bot API* call of the press. `cmd:` / `out:` /
  accepted `ask:` presses answer first with no toast, then edit/send.
- **Ephemeral `not allowed`** (§5) is attempted only when the non-allowed member addressed
  the bot (mention or command); on `ApiError` it is dropped and logged once per chat.
- **Command replies in groups** try `Ephemeral { user, callback_query_id: None }` first and
  fall back to a normal send; `cmd:` presses `edit_rich` the panel in place and fall back to
  a send when the edit fails.
- **`/resume` in General** (topics shape) creates a topic named after the title and replies
  inside it; elsewhere it rebinds the key. A session change on a key shuts the old worker
  down (`rebind`).
- **Lease gate** sits after trust and service messages, before `MarkAnswered` and commands
  (signal's order). `MarkAnswered.sender = "<chat>:<user>"`, `timestamp = update_id`.
- **Inbox names** are `telegram/inbox/<date>.<message_id>[.<n>]-<name>` (`<n>` for the 2nd+
  file of an album); `<name>` = Telegram's `file_name`, else `<kind>.<ext>` from the remote
  path or the effective MIME (declared → sniffed → `mime_guess` on the name).
- **`out:<call_id>`** files are named `<call_id>-output.txt`: the `Output` reply carries only
  the text, not the tool name.
- **`topics_shape(chat)`**: private → `BotIdentity.topics`; groups → `is_forum` as last seen
  on an inbound message (`State.chats`), which also keeps the chat title / `@user` for the
  `message_user` label.
- **Owner-chat cards** need `cfg.approvals == All` and an owner; the context line comes from
  `port.session_title` (`Unknown` on `Err`/`None`).
- `Handle::link` delegates to `Channel::mint_link` (shared with `Outcome::Link`).

## Contract changes

- `threads.rs`: none.
- `channel.rs` (mine): `Channel.api: Arc<dyn Sender>`, new `live`, `notes_tx`; `Channel::new`
  returns `(Channel, UnboundedReceiver<WorkerNote>)`; `State` gains the routing maps listed
  above. `Handle` is as in §2.8. New `pub fn secret_notice`, `pub(crate) fn clip`.

## NEEDS

- `// NEEDS(RENDER)`: `WorkerCtx` has no marker message id, so a flat child's messages cannot
  be replies to its `↳ spawned` marker (§9.2); the channel remembers marker → child for
  steering only.
- `// NEEDS(RENDER)` (inherited from stage 1): boot repair of `threads.json.open` cannot
  append `· interrupted`.

## Tests (`channel::tests`, `approvals::tests`, `asks::tests`)

Design §15 item 4 as listed, on `MockSender` + a stubbed `Port`, except **`MarkAnswered
Ok(false)` drops**: `tasks::Tasks` is not constructible outside its crate (private ledger),
so the gate is untested here; the code is `crates/signal/src/channel.rs:1146-1156` verbatim.

## Gates

```
$ cargo check -p telegram --all-targets
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 9m 19s
$ cargo test -p telegram
test result: ok. 75 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.75s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
$ cargo clippy -p telegram --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 41.40s
$ cargo fmt -p telegram -- --check
(no diff)
```

Formatting was applied with `rustfmt` on the four owned files only (never `cargo fmt --all`).

## Remaining

- `MarkAnswered Ok(false)` test (see above).
