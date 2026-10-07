# 125 as-built — I (closer)

Branch `feat/125-advisor`. Spec: `docs/design/125-advisor.md` §9 (channels), §11 "Closer" (gates), §12 (seams, closer smoke).
Starting HEAD `6c7c64f1` = contract `d3c5b682` + W3 (`3d619c9c`) + W2 (`05039b26`) + W1 (`6c7c64f1`). The four as-built notes
(`125-as-built-{C,W1,W2,W3}.md`) were read first; every hand-off in them is closed below.

## Merge log

| Commit | What |
|---|---|
| `82dcae88` | **fold** `origin/main` `3c9b2834` (not `5d700dda`: main had moved two commits past the brief — `2a507d72` no_emoji scope + `3c9b2834` its merge — so the current tip was folded; message names it). Seven conflicted files, both features kept: `crates/agent/src/lib.rs` (125 types + 127 `GetFavoriteModels`/`SetModelFavorite`, additive), `crates/agent/src/supervisor.rs` (one import line), `crates/tui/src/lib.rs` (`info` destructure now `(default_model, default_effort, advisor, favorite_models)`; `Links.favorite_models` + `Links.advisor`; `Command::SetAdvisor`/`AdvisorStatus` + `SetModelFavorite` arms; `print_event` both arms), `crates/tui/src/model.rs` (`UiEvent::AdvisorStatus` + `FavoriteModels`; `Command` variants; `pick_current` + `set_favorites`; `models_key` `picked` is `(bool, Option<String>)` with `ModelRow::Favorite` folded in and the `Tab` star arm matching `Some((false, Some(model)))`; the test tail rebuilt from both sides — 127's `models_picker_lists_favorites_first_and_tab_toggles_them` + W2's ten), `crates/tui/src/ui.rs` (`Links`/`App` fields; picker titles keep `{noun}` **and** the `⇥ ★ favorite` hint; the `marks(on, fav)` two-char column also draws the `Off` row; `Favorite \| Model` rows mark `pick_current()`; 127's `model_rows_carry_current_and_favorite_markers` test gains `target: PickTarget::Session` in its three `Menu::Models` literals), `crates/desktop/ui/src/components/chat/ModelMenu.tsx` (`ModelCommand` keeps the Advisor level and gains `favorites`/`onToggleFavorite`/`listClassName`; `AdvisorCommand` passes the same three through to `ModelPicker` and its wrapper is `flex min-h-0 flex-col` so 127 §6's phone-dialog list can fill; `ModelDialog` uses 127's `dialogPlacement`), `crates/desktop/ui/src/rpc/api.ts` (both method groups; `METHODS` stays sorted — `set_advisor*` before `set_approvals`, `set_model_favorite` after `set_model`). Auto-merged seams read: `crates/rpc/src/api.rs` `Info` (`favorite_models` + `advisor`), `bindings/Info.ts`, `lib/rpc.ts`, `store/sessions.ts`, `rpc/extra.ts`, `slack/channel.rs`. |
| `6c3a3093` | channel arms (§9), `Advisory::channel_line`, `rows_since` → `render_with`, `GetAdvisorDefaults` behind `Info.advisor` |
| `cce0c7b3` | `cargo fmt` of the touched crates; `supervisor::render` is a `#[cfg(test)]` shim of `render_with` (its only remaining callers are tests — the folded tree warned `function render is never used`) |
| `0c122bea` | `crates/roster/src/tools.rs` test: `frame::baseline` eighth (`advised`) argument — the only `--workspace --all-targets` red |
| `20b91054` | `crates/telegram/src/api.rs` test: the `assert!(… \|\| true)` tautology removed (clippy `overly_complex_bool_expr`, pre-existing on main) |
| (last) | this note |

## Built (file-level)

- `crates/agent/src/lib.rs` — `impl Advisory { pub fn channel_line(&self) -> Option<String> }`: the card's highest-severity note,
  first line, as `advisor · concern — <line>` / `advisor · blocker — <line>`; `None` for a card of nits (110 §3 rule 3). One
  helper, three channels. `pub struct GetAdvisorDefaults` (`ask` → `AdvisorDefaults`).
- `crates/slack/src/channel.rs` `apply`: `AdvisoryAdded { advisory, .. }` → `post_blocks_or_markdown(ui::advisory_line(&line), &line)`
  (one `context` block in the thread; markdown fallback when Block Kit refuses); `AdvisorChanged` stays ignored. `crates/slack/src/ui.rs`
  `pub fn advisory_line(text) -> Value` (`[context(text)]`). Test `advisory_posts_one_context_line_for_concerns_only` (nit card → no
  post; concern+blocker card → one `context` block reading `advisor · blocker — the test file was deleted`, no `post_markdown`).
- `crates/telegram/src/worker.rs` `event`: `AdvisoryAdded` → `send_rich_or_plain(ctx, brand(ctx, footer(&line)))` beside the
  `MessageDelivered` arm (a `<footer>`-styled line; `AdvisorChanged` falls into the existing `_ => {}`). Test
  `advisory_concern_is_one_footer_line_and_nit_is_silent`.
- `crates/signal/src/channel.rs` `handle_agent`: `AdvisoryAdded` → `flush` what is buffered, then `send_chunks(&to, &line)` plain.
  Test `advisory_blocker_is_one_plain_line_after_the_prose` (`["On it.", "advisor · blocker — tests deleted"]`).
- `crates/agent/src/store/advisor.rs` `rows_since`: calls `supervisor::render_with(…, &advisories)` and `retain`s `seq >= from_seq`;
  the post-pass is gone (W1 hand-off).
- `crates/agent/src/supervisor/advisor.rs`: `impl Message<GetAdvisorDefaults> for Agents` → `self.advisor_cfg.defaults()`.
- `crates/rpc/src/api.rs` `info()`: `advisor: ask(GetAdvisorDefaults)` — the supervisor's live copy, no config re-read (82 §3 rule;
  W1/C hand-off). `AdvisorConfig` import dropped.
- `crates/agent/src/supervisor.rs`: the full `render` doc moved onto `render_with`; `render` is `#[cfg(test)]`. `mod advisor; // 125 W1`
  (stale "stubs; W1 owns" comment).
- `crates/roster/src/tools.rs`, `crates/telegram/src/api.rs`: the two test fixes above.

## Hand-offs closed

| From | Hand-off | Closed by |
|---|---|---|
| C, W1 | `Store::rows_since` → `render_with`, delete the post-pass | `store/advisor.rs` (above) |
| C, W1 | `Agents::history` / page loading emit `HistoryEntry::Advisory` | already W1's: `supervisor.rs` `load_session` and the page loader both call `render_with(…, &store.advisories(session))` (verified by read; smoke (d) shows `load_session` roles) |
| W1 | `store/accounting.rs:374` fmt wrap | `cargo fmt -p agent` clean at `cce0c7b3` (W1 had reverted its own churn; nothing left to wrap) |
| W1, C | `info()` live advisor defaults | `GetAdvisorDefaults` (above); smoke (a) shows `Info.advisor.model` move from `null` to the pick without a restart |
| W1 | channels §9 | the three arms (above) |
| W2 | never emit `SubagentSpawned` for the advisor child | already true: the only emission is the spawn handler (`supervisor.rs` `SubagentSpawned` publish after `self.session(Some(id), Some((parent, &name)), …)`); `advisor_on` creates the child through `store.create_session_kind` + `self.session(Some(id), None, None, …)` and never passes there. No change. |
| W2 | `tests/no_emoji.rs` red (21 glyphs in `crates/surface`) | fixed on main by `2a507d72` (scope narrowed to `agent` + `rpc`, Sky 2026-10-07); green after the fold — no glyph edits needed |
| W2 | engine stub `Err("advisor: not built")` | W1 replaced it; smoke (a) returns a real `AdvisorStatus` |
| W3 (optional) | refetch `sessionCost` on `AdvisorChanged` if the advisor's calls do not bump the advised | not needed: `loop_::settle` publishes `ModelUsageChanged` for "the owner and every live ancestor the store bumped"; the advisor's parent is the advised session (smoke: 5 `ModelUsageChanged` on the advised stream while the advisor ran) |
| C (optional) | `session_advisors` in `store::capture::ATTACHED` | not done (engine-local like `pty_screens`; design 68 moves are out of 125's scope) — open item |
| C | `125 C placeholder` arms | slack: replaced (above). telegram/signal matched `_` (nothing to replace). `session_read.rs` / `rpc/acp.rs` `Advisory` arms are real implementations (C's note calls them "125 C", not placeholders) and stay. |

## Gates (exact result lines)

All through `build-lock` (`BUILD_LABEL=125-closer`, `CARGO_BUILD_JOBS=4`, `CARGO_INCREMENTAL=0`, `CARGO_TARGET_DIR=…/theuth-125-target`).

1. `BUILD_SLOTS=1 cargo check --workspace --all-targets` → first run `error[E0061]: this function takes 8 arguments but 7 arguments were supplied` ×2 at `crates/roster/src/tools.rs:416,417` (test-only; fixed in `0c122bea`); rerun → `Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 40.56s` (one pre-existing future-incompat note for `block v0.1.6`).
2. `cargo test -p agent -p rpc -p tui` →
   `agent`: `test result: ok. 447 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 46.20s`
   `rpc`: `test result: ok. 47 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 11.55s` · `tests/prompts.rs`: `ok. 1 passed`
   `tui`: `test result: ok. 141 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.58s` · `tests/no_emoji.rs`: `test result: ok. 1 passed; 0 failed`
   (139 W2 + 2 from 127 — W2's ten §7.6 tests and 127's picker test coexist.)
3. `cargo test -p slack -p telegram -p signal` →
   `signal`: `test result: ok. 64 passed; 0 failed` · `slack`: `test result: ok. 143 passed; 0 failed` · `telegram`: `test result: ok. 122 passed; 0 failed`
   (the three §9 tests included; the known `rpc::engine::tests::second_acquire_in_one_process_is_running` flake did not fire in gate 2; the `tools` crate's `jobs::` flakes were not in any gate).
4. `cargo clippy -p agent -p rpc -p tui -p slack -p telegram -p signal --all-targets -- -D warnings` → first run `error: this boolean expression contains a logic bug --> crates/telegram/src/api.rs:1714` (pre-existing; fixed in `20b91054`); rerun → `Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 32.87s`.
5. `BUILD_SLOTS=1 just ts-desktop` → `test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 47 filtered out` then the `cp -R bindings/.` step; `git status` clean afterwards (the tracked `crates/rpc/bindings/*.ts` already matched).
6. `cd crates/desktop/ui && bun run check` → `$ tsc -b --noEmit`, exit 0.
7. `cd crates/desktop/ui && BUILD_SLOTS=1 bun run test` → `Test Files  179 passed (179)` · `Tests  1228 passed (1228)` (`pairing.test.ts` did not flake).
8. `cd crates/desktop/ui && BUILD_SLOTS=1 bun run build` → `✓ 13982 modules transformed.` · `✓ built in 3.06s`, exit 0.

`cargo fmt -p agent -p rpc -p tui -p slack -p telegram -p signal -p roster` applied (`crates/signal/src/commands.rs` wanted two
pre-existing wraps from main's 127 checkpoint; reverted — not 125's file, nothing of ours there).

## Scratch-engine smoke

`cargo build -p theuth` (debug, `BUILD_SLOTS=1`) → `Finished … in 3m 59s`. Data dir `/tmp/125-smoke`: `config.yaml` + `skills/`
copied from `~/.theuth`; the copy edited by a throwaway Bun script (`Bun.YAML`, key names printed, never values): `slack`/`telegram`/
`signal`/`link`/`a2a`/`mesh` `enabled: false` (mesh and link off too so the scratch engine never dials the live one), `rpc.listen:
127.0.0.1:7925`. Engine: `env -u THEUTH_RPC__LISTEN -u THEUTH_LAUNCH <target>/debug/t --data-dir /tmp/125-smoke --headless` →
`engine.json {listen: "127.0.0.1:7925", version: "2.10.0", host: "headless"}`. Driver: Bun + native `WebSocket` on
`ws://127.0.0.1:7925/?token=<token>` (`crates/rpc/src/engine.rs` `running`), positional params in `api.rs` order, `subscribe_events(session,
descendants: true)`. Model for both the advised session (`set_model(session, …, manual: false)` — the config default is an OpenAI id with
no key in the shell) and the advisor: `anthropic/claude-haiku-4-5` (listed by `list_models`, 205 entries). Two runs, 23 s each.

| Step | Check | Result |
|---|---|---|
| (a) | `new_session` → `set_advisor(s, Some("anthropic/claude-haiku-4-5"), None)` | `AdvisorStatus { state: "idle", advisor: 01a11672-3467-…, model: "anthropic/claude-haiku-4-5", context_limit: 200000, step: 0, backlog: 0, notes: 0/0/0, cost: {calls: 0} }`. `info().advisor` before: `{model: null, effort: null, max_notes_per_update: 4, immune_turns: 3, sync_backlog: "off"}`; after: `model: "anthropic/claude-haiku-4-5"` (live `GetAdvisorDefaults`). |
| (b) | hidden everywhere but `advisor_status` | `list_sessions` has the advised, **not** the advisor (`has_advisor: false`); `list_agents(s)` → `[]`; `advisor_status(s)` → `{state: "idle", model: …, advisor: 01a11672-3467-…}`. |
| (c) | `send_input` (read `smoke-note.txt` with `read_file`, report its first line) → stream | advised tool calls `python`, `read_file`; `TurnFinished` in 23 s. `AdvisorChanged` ×5 in order: `reviewing step 1 backlog 0` (after the first tool batch) → `reviewing step 2 backlog 1` (terminal step while the review ran) → `idle step 2 backlog 1, tokens 7064/228` (advisor's first turn done) → `reviewing step 2 backlog 0` (the backlog drained as one update, §4.6) → `idle, tokens 14173/472`. The advisor session's own `TurnStarted`/`TurnFinished` are **not** on the `descendants: true` stream (`events.for_advisor_session: []` — `descendants()` hides `kind = advisor`, §4.2); its two turns show as `cost.calls: 2`, `dispatched: 2`, and as 5 `ModelUsageChanged` on the advised session. |
| (d) | advisory? | **Silent review, both runs.** `AdvisoryAdded: []`, `load_session` roles `[user, reasoning, tool, tool, assistant]`, `advisor_status.notes` 0/0/0, `last_note: null`, `detail: null`, `cost.estimated_for_unavailable: $0.01196`. Run 2 asked the agent to claim "I ran the full test suite … all 40 tests pass" without running anything (the user prompt authorised the role-play, so the advisor's silence is defensible); the advisor's transcript (sqlite, `messages.body`, advisor id) shows two §4.5 updates — `### Session update` / `**user**:` / `**agent**:` / `_thinking:_` — each answered with reasoning and no `advise` call. The `AdvisoryAdded` → card / channel line paths are covered by the unit tests (W2 §7.6, W3 §8.6, the three §9 tests), not by this live run. |
| (e) | `set_advisor(s, None)` | reply `{state: "off", advisor: null, model: null}`; exactly one `AdvisorChanged` follows and nothing else (`events_after_off: ["AdvisorChanged"]`); `advisor_status` → `off`. sqlite: `sessions` keeps both advisor rows (`kind = advisor`, `parent` = advised, `name = advisor`, 4 messages each); `session_advisors` is empty (link row deleted, §4.1 "disposes"); `messages.advisory` all NULL. No further advisor turns. |

Engine stopped (`up 3m35s`), `/tmp/125-smoke` and the driver scripts removed, the keychain item `theuth / secrets:/tmp/125-smoke` the
engine minted on first run deleted.

## Deviations (and why)

1. **Folded `3c9b2834`, not `5d700dda`** — main moved while the brief was written; the two extra commits are the no_emoji scope fix
   the W2 hand-off asked for. Commit message names the real sha.
2. **`~/.theuth/secrets.vault` does not exist** (it is `secrets.vault.migrated`; the vault is `secrets.sqlite` sealed with a keychain
   key scoped to the absolute data dir, `crates/secrets/src/key.rs`) — same finding as `101-live-proof-2026-10-06.md`. Nothing was copied;
   the scratch engine ran with an empty vault and the provider key came from the shell (`ANTHROPIC_API_KEY`, name only checked).
3. **Smoke step (c) read `smoke-note.txt`, not `config.yaml`** — rule 9 forbids printing the config and the model's answer would have
   put its first key into the transcript and this note. A two-line fixture gives the same two-step shape.
4. **The advised session's model was pinned with `set_model(…, manual: false)`** instead of editing `agent.model` in the copied config:
   fewer edits to the copy, and it exercises the picker path the desktop uses.
5. **`channel_line` posts the card's highest-severity note** (ties → the last of them). §9 says "the first line of the note"; a card may
   carry several notes (nits batched, §4.8), so the line is the one that made the card a concern/blocker.
6. **`render` is `#[cfg(test)]`** rather than deleted: ten test call sites across `supervisor.rs`, `acp_child.rs`, `loop_/thinking_tests.rs`
   use the three-argument form; a shim is smaller than ten edits.
7. **`AdvisorCommand` wrapper gained `min-h-0`** so 127 §6's `flex-1` list can shrink inside the phone dialog's bounded `DialogContent`.
   Not rendered in this wave (no browser surface in the smoke); harmless on desktop. Sky's manual smoke step 1 on a phone would show it.
8. **`mesh`/`link`/`a2a` disabled in the scratch config** beyond the three channels the brief names — a scratch engine with the live
   mesh/link config would otherwise dial the live engine and replicate.

## Open items (for Sky's manual smoke, §12)

- §12 steps 1–10 remain manual: the pill and header picker on desktop, `/advisor status` four lines and `◎ advisor: idle` in the TUI,
  `/advisor off`/`on` round trip with a seeded cursor, the Edit-turn replay preface, the restart preface, the missing-key `halted`
  state, and a Slack-bound concern posting one `context` line (unit-tested here, not live — no channel was enabled).
- A live advisory never fired in two scratch runs (silent reviews). Step 3 of §12 ("claim done without running the check") with a
  stronger model or a real check failure is the way to see `AdvisoryAdded` → card → channel line end to end.
- `session_advisors` is not in `store::capture::ATTACHED` (C's optional hand-off): an advised session moved between engines (68) loses
  its link; the advisor session row (a normal `sessions` row) does move.
- Advisor transcripts are reachable only via the engine's own sqlite (`messages.body` by advisor id); `session_read` refuses them (W1).
- The scratch engine's `secrets` plugin logged nothing fatal with an empty vault; a scratch run that needs vault secrets must set
  them with `t --secret-set` against the scratch data dir.
