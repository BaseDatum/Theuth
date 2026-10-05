# 98 — as built, INTEGRATE (2026-10-05)

Integration of the design-98 wave on `feature/98-telegram`: contract `a46fc577` → RENDER
`e656fd7e..23ddf66d`, COMMANDS `9d6f80e8..e67313b1`, ROUTE `c6d6f4a4..52d81d9c` (+ its last
commit), WIRE `bfcbfb78..13f72c8c`; merged in that order (`5f157dca`, `13fcb32d`, `9efc5824`,
`6f13cdc1`), then local `main` `df6f259a` (waves 99 + 100) merged in (`2327e73a`). Orchestrated
from chat; four omp workers (Fable, effort high) in four worktrees off the contract sha, one
shared `CARGO_TARGET_DIR`. Two engine restarts during the wave killed the running omps (no
process survives; the worktrees kept their files); workers were re-spawned from the same briefs.

## Cross-seam fixes made at integration

1. **`Outcome::CloseTopic`** — COMMANDS added the variant for the `/agents` panel's `Close
   topic` button (its as-built lists it under Contract changes); ROUTE's `apply_outcome` match
   did not know it → `E0004` on the merged tree. Added the arm in `channel.rs` (`117a2a26`):
   look the session up in `Threads::by_session`, `closeForumTopic` when it has a topic, unbind,
   reply `closed the topic of <last 8>`; no topic / not bound → a one-line reply.
2. **`ui::guard` was never called** — RENDER noted it sends `raw()` unguarded (NEEDS
   COMMANDS), COMMANDS implemented `guard` and called it nowhere. Applied once in the `Api`
   façade for `send_rich`, `edit_rich` and `draft` (`75d29e47`), so every rich send of every
   module is guarded and no module needs to remember. `MockSender` is unaffected (tests see the
   unguarded text they passed).
3. **`lib.rs` `mod` lines** — RENDER (board, bubble, cards) and ROUTE (approvals, asks)
   conflicted on adjacent lines; resolved by listing all five alphabetically.
4. **Stage-2 deps pre-added** (`a46fc577`, before cutting the worktrees): `markdown`, `tasks`,
   `mime_guess` in `crates/telegram/Cargo.toml`, so no worker edited the shared manifest.

## Gates on the merged tree (`75d29e47`, before the main merge)

```
cargo check -p telegram --all-targets                   Finished `dev` profile in 35.80s
cargo test -p telegram                                  test result: ok. 112 passed; 0 failed; 0 ignored
cargo test -p markdown                                  test result: ok. 19 passed; 0 failed
cargo clippy -p telegram --all-targets -- -D warnings   Finished `dev` profile in 43.80s
cargo fmt -p telegram -- --check                        exit 0, no diff
cargo check -p rpc --features ts                        Finished `dev` profile in 46.75s
cargo check -p theuth                                   Finished `dev` profile in 2m 39s
```

After merging `main` (`2327e73a`): `cargo check -p telegram -p rpc -p theuth --all-targets` and
`cargo test -p telegram` re-run — see the landing commit message for the lines.

Not run: `cargo test --workspace` (per-crate gates only, per the wave rules; WIRE's own gates
covered `rpc` bindings and the `theuth` check, with the pre-existing `talk` `drain_collect`
clippy lint allowed); the desktop UI gates (no UI change in 98 beyond the generated bindings,
which are gitignored).

## Not exercised — Sky tests against a real bot

No Telegram API call was made by anyone during the wave (every test runs on `MockSender` or
`wiremock` on 127.0.0.1; WIRE's scratch-engine smoke ran with `telegram.enabled: true` and no
token and confirmed the `Unconfigured` path, the RPC methods and `telegram is disabled`). Design
98 §16's spikes were skipped by decision; the facts they would have settled are now the first
things a live test will show:

- drafts: pacing tolerance, animation of 1 s full-text re-sends, `<tg-thinking>` rendering,
  `can_stop` → `stopped_message_generation`, draft inside a private topic;
- Rich Markdown fidelity on model output, `<footer>` inside Markdown mode (`bubble::footer` is
  the one place to flip to an italic line), `<tg-button>` in the card body (fallback: a
  `reply_markup` keyboard), `<details>` with a fence inside;
- private-chat topics: General semantics, bot-created topics receiving the user's messages with
  `message_thread_id`, `editForumTopic`/`closeForumTopic` in private chats;
- reactions on the user's message in a private chat (👀 → 👌/🤷/😱), groups without
  `can_react_to_messages`;
- ephemeral replies to `is_ephemeral` group commands;
- privacy mode: whether `@mention` text reaches a non-admin bot;
- `getFile` over 20 MB error text; `sendPhoto` dimension fallback;
- lease handover between two engines.

## Follow-ups (not blocking)

- `/a2a reload` answers "needs the terminal" (COMMANDS: `a2a` is not a dependency of the crate;
  `/a2a` reports the network line via `GetCommunication`). Port `signal::commands::a2a_cmd` with
  an `a2a` dep if wanted.
- Flat-chat children (§9.2): messages are prefixed `**name** ·` but not sent as replies to the
  `↳ spawned` marker (RENDER has no marker id; ROUTE keeps marker → child for steering only).
- Boot repair of `threads.json.open` is `edit_markup(None)` per entry; `· interrupted` cannot be
  appended without the message text (CONTRACT/RENDER NEEDS).
- `Unauthorized` (401, revoked token) backs off like any error instead of stopping the poller
  (CONTRACT deviation).
- `MarkAnswered Ok(false)` drop is implemented (signal's code) but has no unit test
  (`tasks::Tasks` is not constructible in the crate's tests).
- `ui::guard` scans code spans/fences with its own CommonMark rules (no `pulldown_cmark` dep
  in the crate); indented code blocks are not recognised.
- Design 98 §2.1 line counts are estimates; as built the crate is ~13 000 lines incl. tests.
