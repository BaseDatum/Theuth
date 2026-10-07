# 125 as-built — C (contract)

Branch `feat/125-advisor`, off main `5bc474b9`. Spec: `docs/design/125-advisor.md` §6 (frozen) + §10 "C".
Nothing behavioural: types, migration, store, stubs, RPC, bindings, config block.

## Built (file-level)

- `crates/agent/migrations/0034_advisor.sql` — new, exactly §6.1 (`sessions.kind TEXT NOT NULL DEFAULT 'chat'`,
  `session_advisors` 1:1 side table, `messages.advisory TEXT NULL`). **Number used: 0034** (0033 was the latest).
- `crates/agent/src/lib.rs` — as committed: `SessionKind { Chat, Advisor }`, `AdvisorSeverity { Nit, Concern, Blocker }`,
  `AdvisoryNote`, `Advisory`, `HistoryEntry::Advisory { advisory, context }` (`role = "advisory"`),
  `AdvisorState { Off, Idle, Reviewing, Paused, Halted }`, `AdvisorNoteCounts`, `AdvisorStatus`, `AdvisorLink`,
  `SyncBacklog { Off, One("1"), Three("3"), Five("5") }`, `AdvisorDefaults`,
  `AgentEvent::{AdvisorChanged { session, status }, AdvisoryAdded { session, seq, advisory }}` (+ `AgentEvent::session()` arms),
  `SetAdvisor`, `AdvisorStatusOf`, `SetAdvisorDefaults`, `SetAdvisorOn(pub bool)`, `AppendAdvisory { advisory, text }`;
  `SessionSummary.kind` and `AgentInfo.kind` (`#[serde(default)]`); `AgentConfig.advisor: AdvisorConfig` (`#[serde(skip)]`)
  with `AdvisorConfig { model, effort, max_notes_per_update: 4, immune_turns: 3, sync_backlog: Off }` and
  `AdvisorConfig::defaults() -> AdvisorDefaults`.
- `crates/agent/src/store.rs` — `SessionRow.kind` + `session_cols!()` + `SessionRow::summary` sets `kind`;
  `create_session` delegates to new `create_session_kind(…, kind)`; `list_sessions()` / `descendants(root)` keep their
  signatures and hide advisors, new `list_sessions_incl(include_advisors)` / `descendants_incl(root, include_advisors)`;
  `delete_session` uses `descendants_incl(root, true)` so the hidden child is deleted with its parent;
  `search_titles` / `search_bodies` exclude `kind = 'advisor'`; `Row.advisory` read by every row reader (`rows_in`, `hidden`),
  copied by `fork`, and `Row::is_note()` (= vision or advisory) replaces the three `vision.is_some()/is_none()` turn-reader checks
  (`turn_starts`, last-turn text, trim), so an advisory row never opens a turn anywhere in the store.
- `crates/agent/src/store/advisor.rs` — new: `advisor_link`, `advisor_links`, `put_advisor_link` (upsert), `delete_advisor_link`,
  `rows_since`, `append_advisory`, `first_turn` (all §6.1), plus `advisories(session) -> BTreeMap<i64, Advisory>` (typed metadata
  by seq, fail-closed like `vision_notes`) and `pub(super) kind_str / kind_of / advisory_of`. Tests (4):
  `link_upserts_round_trips_and_deletes`, `appended_advisory_opens_no_turn_and_renders_typed`,
  `advisors_hidden_from_lists_trees_and_search_unless_asked`, `migration_applies_on_a_0033_database`
  (`sqlx::migrate!().run_to(33)` then `Store::open`).
- `crates/agent/src/supervisor/advisor.rs` — new: stub `impl Message<SetAdvisor | AdvisorStatusOf | SetAdvisorDefaults> for Agents`
  (`Err("advisor: not built")`, `AdvisorStatus { session, ..default }`); `supervisor.rs` gains the one `mod advisor;` line (+ one forced
  field, see hand-offs).
- `crates/rpc/src/api.rs` — `set_advisor`, `advisor_status` (`with_extensions`, `authorize(session)`), `set_advisor_default_effort`,
  `set_advisor_notes_per_update`, `set_advisor_immune_turns`, `set_advisor_sync_backlog` (plain, the `set_schedule_run_timeout`
  shape); `Info.advisor: AdvisorDefaults` from `rt.section::<AdvisorConfig>("advisor")`. `crates/rpc/src/export.rs` — the ten
  `export!` lines. `crates/rpc/src/acp.rs` — one `replay` arm (forced, see hand-offs).
- `crates/desktop/ui/src/rpc/api.ts` — six `Api` methods + sorted `METHODS` (+ three binding imports).
- `crates/theuth/src/main.rs` — commented `advisor:` block in `DEFAULT_CONFIG` (model, effort, max_notes_per_update, immune_turns,
  sync_backlog).

## Gates (exact result lines)

- `cargo test -p agent --lib store::` → `test result: ok. 74 passed; 0 failed; 0 ignored; 0 measured; 359 filtered out`
  (the 4 new tests included).
- `cargo test -p rpc` → `test result: ok. 40 passed; 1 failed` on the first run: `engine::tests::second_acquire_in_one_process_is_running`
  (engine lock file, unrelated to 125); rerun 3× in isolation `test result: ok. 4 passed; 0 failed` each — flaky under the
  parallel suite, not a 125 regression.
- `cargo clippy -p agent -p rpc --all-targets -- -D warnings` → `Finished dev profile` — no warnings, no errors.
- `cargo check -p tui -p theuth` → `Finished dev profile` — every dependant compiles against the stubs.
- `just ts-desktop` (BUILD_SLOTS=1) → `test result: ok. 8 passed; 0 failed` (export_bindings); the ten `Advisor*`/`SessionKind`/`SyncBacklog` `.ts` files and the six names in `methods.json` present; copied into `crates/desktop/ui/src/rpc/bindings/`.
- `cd crates/desktop/ui && bun run check` (after `bun install` in `packages/apps` and `crates/desktop/ui`) → `tsc -b --noEmit` exit 0.
- `cd crates/desktop/ui && bunx vitest run src/rpc` → `Test Files  2 passed (2)` / `Tests  8 passed (8)` (`rpc/api.test.ts` sees the six new names).

## Deviations from §6 (and why)

1. **`create_session` keeps its signature**; `kind` lives on `create_session_kind(id, model, parent, name, project, kind)`.
   §6.1 said `create_session` "gains `kind`", but it has ~60 callers across files C does not own (loop_.rs, supervisor.rs, tests
   in six crates). W1 calls `create_session_kind(…, SessionKind::Advisor)` when creating the advisor child.
2. **`list_sessions()` / `descendants(root)` keep their signatures** (hiding advisors by default); the `include_advisors: bool`
   forms are `list_sessions_incl` / `descendants_incl`. Same reason: every caller is in supervisor.rs, which C must not touch;
   default behaviour of every existing caller is "advisors hidden", as the design wants. W1's §10 items "`agents()`/`list_sessions()`
   pass `include_advisors: false`" are therefore already true; the Advisors actor and cost readers call the `_incl(true)` forms.
3. **`SetAdvisorDefaults` Reply is `Result<AdvisorDefaults, String>`**, not `AdvisorDefaults`: persisting through
   `write_config_value` and the 1–32 / 0–10 range checks can fail, and a stub cannot honestly return defaults. Only
   `supervisor/advisor.rs` (W1's file) and `api.rs` (C's) see the Reply type; the RPC return type is `AdvisorDefaults` as specified.
4. **`AdvisorStatus` / `AdvisorDefaults` also derive `kameo::Reply`** (required for a plain `Reply = AdvisorStatus`), and
   `AdvisoryNote`, `Advisory`, `AdvisorNoteCounts`, `AdvisorLink`, `AdvisorDefaults` derive `PartialEq, Eq` (tests compare them).
   Supersets of §6.2; wire shape unchanged.
5. **TS optionality of `kind`**: `SessionSummary.kind` / `AgentInfo.kind` carry `#[cfg_attr(feature = "ts", ts(optional = nullable))]`
   so the binding is `kind?: SessionKind` — the four existing desktop `SessionSummary` fixtures (W3's files) keep type-checking,
   and an older harness omits the field anyway. W3 filters `kind === 'advisor'`, which works either way.
6. **`rows_since` post-processes `supervisor::render`**: `render` (W1's file) still emits `HistoryEntry::User` for an advisory row;
   `rows_since` swaps it for `HistoryEntry::Advisory` using `Store::advisories`. Until W1 updates `render`, the `turn` of entries
   *after* an advisory inside `render`'s output can point at the advisory seq (render's `turn_openers` closure only knows `vision`).
7. **Only the two session-scoped RPC methods are `with_extensions`** (they `authorize(session)`); the four global setters follow the
   `set_schedule_run_timeout` shape with no extensions, like every other global setter. No `forward::route` for `set_advisor` /
   `advisor_status` (the advisor is engine-local; mesh forwarding is W1/closer's call).
8. **No `ContextEntry.advisory` field**: `context.rs` is not C's and `ContextEntry` has ~10 struct literals in other crates' tests.
   The typed metadata is reachable through `Store::advisories(session)` instead (one query, `BTreeMap<i64, Advisory>`).
9. **Step 4 bridge finding**: `crates/rpc/src/bridge.rs` `forward!(AgentEvent, …)` forwards the whole enum — no per-variant list,
   nothing to add; `subscribe_events` delivers `AdvisorChanged` / `AdvisoryAdded` unchanged.

## Forced edits outside C's ownership (each one line, compile-forced; nothing behavioural)

Adding `HistoryEntry::Advisory`, the two `AgentEvent` variants and the `kind` fields breaks every exhaustive match / struct literal
downstream, and the brief's gate requires `cargo check -p tui -p theuth` to pass at C's commit. Rule 5's "stub locally" for an enum
variant *is* the arm, so these are in, marked `125 C placeholder` where a later wave replaces them:

- `crates/agent/src/supervisor.rs:~5325` `info_row`: `kind: row.kind` (AgentInfo literal).
- `crates/agent/src/session_read.rs`: outline arm ignores `Advisory`; `turn_text` prints `#{n} advisory [Sev]: note` per note;
  test fixture `kind: Default::default()`.
- `crates/rpc/src/acp.rs` `replay`: `Advisory` → one `UserMessageChunk` joining `[advisor Sev] note` lines.
- `crates/tui/src/model.rs`: `apply_agent` ignores the two events (placeholder — W2 §7.5); `from_history` maps `Advisory` to one
  `Entry::Info` per note (placeholder — W2 §7.4 `Entry::Advisory`); `SubagentSpawned` AgentInfo literal + 4 test fixtures `kind`.
- `crates/tui/src/lib.rs` `print_history`: `Advisory` → `advisor [Sev]: note` lines (placeholder — W2).
- `crates/tui/src/attach.rs`, `crates/slack/src/ui.rs` (×2), `crates/slack/src/board.rs`, `crates/telegram/src/commands.rs` (×2),
  `crates/telegram/src/ui.rs`, `crates/roster/src/desk.rs`: test fixtures gain `kind: Default::default()`.
- `crates/slack/src/channel.rs` `apply`: the two events ignored (placeholder — the closer's §9 `AdvisoryAdded` arm).
- `crates/desktop/ui/src/store/reduce.ts` `historyEntries`: exhaustive `switch (h.role)` (tsc TS2366) gains `case "advisory"` → one
  `info` entry per note (placeholder — W3 §8.3 `advisory` Entry kind).
- `crates/rpc/bindings/{Info,AdvisorDefaults,SyncBacklog}.ts`: ts-rs's per-type export tests write rpc's own types here and the
  directory is tracked (56 files before); regenerated output committed as the repo already does.

## Hand-offs for the closer / next waves

- **W1 `render` (supervisor.rs)**: emit `HistoryEntry::Advisory` for seqs in `store.advisories(session)` and extend the
  `turn_openers` closure to `entries[seq].vision.is_some() || advisories.contains_key(&seq)`; then delete the post-pass in
  `Store::rows_since` (it becomes a no-op). `Agents::history` / page loading should do the same so the desktop/TUI receive
  `role: "advisory"` entries on load.
- **W1**: create the advisor child with `store.create_session_kind(id, model, Some(advised), Some("advisor"), None, SessionKind::Advisor)`;
  read links with `advisor_links()` at boot; the Advisors actor and `session_cost` use `descendants_incl(root, true)` /
  `list_sessions_incl(true)` where the advisor must be seen.
- **W1**: replace the three stubs in `supervisor/advisor.rs`; `SetAdvisorDefaults` reply stays `Result<AdvisorDefaults, String>`
  (or change it together with `api.rs`'s four one-liners — they compile either way).
- **W1**: fill `AgentConfig.advisor` in `supervisor::start` from `rt.section::<AdvisorConfig>("advisor")` (the `surface:` pattern);
  `info()` currently reads the section directly — switch it to a live `Agents` ask once the actor owns the defaults (82 §3 rule).
- **W2**: replace the three `125 C placeholder` arms in `crates/tui/src/model.rs` / `lib.rs`.
- **W3**: replace the `125 C placeholder` arm in `store/reduce.ts` with the `advisory` Entry kind.
- **Closer**: `crates/slack/src/channel.rs` placeholder → the §9 arm; telegram/signal matches have wildcards (no edit was needed).
- **Closer (optional)**: `session_advisors` is not in `store::capture::ATTACHED` (engine-local like `pty_screens`); add it if an
  advised session may move between engines with its link (68).

## Open items

- `cargo test -p rpc`: `engine::tests::second_acquire_in_one_process_is_running` is flaky under the full parallel suite
  (passes alone); pre-existing, unrelated to 125.
- `crates/desktop/ui/src/rpc/bindings/**` is gitignored; regenerated locally by `just ts-desktop`, W3 regenerates in its own tree.
