# 125 as-built — W2 (TUI)

Branch `feat/125-w2`, cut from the contract commit `d3c5b682`. Spec: `docs/design/125-advisor.md` §7 (+ §3, §6.2–6.3, §10 "W2", §11 "W2").
Only `crates/tui/src/{model.rs, ui.rs, lib.rs}` and this note were touched.

## Built (file-level)

- `crates/tui/src/model.rs`
  - `COMMANDS` row `("/advisor", "[off|on|status|<provider/model>]", …)`; `"/advisor" => self.advisor_cmd(arg, tx)` in `submit_with`;
    `advisor_cmd` (bare → `pick_target = Advisor` + `ListModels`; `off` → `SetAdvisor { model: None }`; `on` → the remembered
    `advisor_default` or `Entry::Error("/advisor on: no remembered advisor model — pick one with /advisor")`; `status` → `AdvisorStatus`;
    `<id>` → `SetAdvisor`; ACP child → `Entry::Error("/advisor: not available for an external harness session")`); `set_advisor`
    (marks the reply as a pick so it chains into the advisor effort picker); `set_advisor_status` (installs, remembers the model as the
    `/advisor on` default, prints the one `advisor → <model>` / `advisor → off` line when the model or off-state changed — whichever of
    the RPC reply or `AdvisorChanged` lands first).
  - `Command::SetAdvisor { session, model, effort }`, `Command::AdvisorStatus(SessionId)`, `UiEvent::AdvisorStatus(AdvisorStatus)`
    (reply handling: silent install; `status` prints §7.3; a pick chains `open_efforts(levels, PickTarget::Advisor)`).
  - `PickTarget { Session, Advisor }` on `Menu::Models` and `Menu::Efforts`; `ModelRow::Off { current }`; `models_rows` leads with `Off`
    on both browse levels for the advisor target, `●` marks `pick_current()` (the advisor's model, or the session's); `models_home` skips
    the lead rows; `models_key` ⏎ → `SetAdvisor` (model or `Off`) for the advisor target; `efforts_key` ⏎ for the advisor target re-sends
    its model with the effort. 124's search/ranking untouched.
  - `advisor_status_lines` (§7.3 exact lines, `—` for unknowns, `k()` / `cost_short`), `advisor_changed_line`, `advisory_title`,
    `advisory_lines`, `advisory_severity`, `severity_name`, `advisor_state_name`, `ADVISOR_GLYPH = "◎"`, `ADVISORY_COLLAPSED = 3`.
  - `Entry::Advisory { advisory, expanded }`; `Entry::height` arm (`4 + min(3, n) + (n > 3)` collapsed, `4 + n` expanded); `toggle_card`
    toggles it only when it has hidden notes; `from_history(HistoryEntry::Advisory)` → one card (placeholder replaced); live
    `AgentEvent::AdvisoryAdded` → `interject` (above a streaming tail), idempotent per `seq` (`advisory_seqs`); a `TurnStarted` whose
    `turn_seq` is a carded advisory, or whose text is only an `<advisory …>` wrapper (`injected` learned the tag), is not echoed as `◆ you`.
  - `AgentEvent::AdvisorChanged` / `AdvisoryAdded` are handled in the pre-tail block (with `TodoChanged`/`SubagentSpawned`) so they never
    close a streaming `Entry::Assistant`/`Reasoning` (placeholder replaced).
  - `Model.advisor: AdvisorStatus`, `advisor_default: AdvisorDefaults` (survives `reset()`), `refresh_advisor` (set by `load`, consumed by
    the ui loop → `Command::AdvisorStatus`), `pick_target`, `advisor_reply`, `advisory_seqs`.
  - Status/header: `advisor_segment()` → `("  ·  ◎ advisor: idle|⠋ reviewing…|paused|halted", state)`; `animating()` includes
    `state == Reviewing`; `advisor_tag()` → `"  ·  advisor $0.012"` after `cost_tag` in both header formats.
  - Board: `UiEvent::Agents` filters `kind == Advisor`; `SubagentSpawned` whose `child == advisor.advisor` inserts no row and prints no
    `↳ spawned` line.
- `crates/tui/src/ui.rs`: `Links.advisor` (from `Info.advisor`) seeds `advisor_default`; the refresh loop sends `AdvisorStatus` on
  `refresh_advisor`; status row = `m.status()` + the coloured advisor segment (idle SAND, reviewing FAIENCE, paused OCHRE, halted CARNELIAN)
  in one left `View`; `Entry::Advisory` card arm (Round box in the worst severity colour, title `◎ advisor · N notes[   ctrl+o expands|collapses]`
  only when notes are hidden, `[nit]`/`[concern]`/`[blocker]` badges SAND/OCHRE/CARNELIAN, note text uncoloured, `… +N more notes`);
  picker title noun `advisor`/`model`, `Off` row `● Off … no advisor`, provider title counts models only.
- `crates/tui/src/lib.rs`: `run_commands` arms `SetAdvisor` → `api.set_advisor(session, model, effort)`, `AdvisorStatus` →
  `api.advisor_status(session)`, both → `UiEvent::AdvisorStatus`; `Info.advisor` threaded into `Links`; line printer: `print_history`
  Advisory → title + `[sev] note` lines (placeholder replaced), `print_event` prints `AdvisoryAdded` the same way, `AdvisorChanged` as the
  `advisor → …` line, `UiEvent::AdvisorStatus` as the §7.3 lines.
- Tests (§7.6, ten): `model.rs` `advisor_cmd_bare_opens_picker_with_off_row`, `advisor_cmd_off_sends_none`,
  `advisor_cmd_on_uses_remembered_model_or_errors`, `advisor_status_prints_four_lines`, `advisory_added_interjects_card_above_streaming_tail`,
  `advisory_card_collapses_to_three_notes`, `history_advisory_row_renders_card_not_user_bubble`, `advisor_changed_updates_header_cost_and_status`,
  `spawned_advisor_never_reaches_board`; `ui.rs` `advisory_card_is_tight_at_any_width`. `tests/no_emoji.rs` unchanged.

## Gates (exact result lines)

- `cargo test -p tui` → `test result: ok. 139 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` (unittests `src/lib.rs`; the ten
  §7.6 tests included) · `tests/no_emoji.rs` → `test result: FAILED. 0 passed; 1 failed` — **pre-existing, not W2**: all 21 hits are in
  `crates/surface/src/{elk_family.rs, export.rs}` (`▶`, `⚠`, `☹`, `☺`; design 123 commits `39d8b79e`/`63952bc8`, already failing at the
  contract commit `d3c5b682`); zero hits in `crates/tui` — the W2 glyph `◎` passes. See hand-offs.
- `cargo clippy -p tui --all-targets -- -D warnings` → `Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 2m 12s` — no warnings.
- `cargo fmt -p tui -- --check` → exit 0 (after one `cargo fmt -p tui`).
- One run of the clippy gate failed in `sentencepiece-sys v0.13.2`'s build script (`exit status: 101`, a dependency of the workspace, nothing
  in `tui`); the rerun compiled it and passed — transient on the shared machine.

## Deviations from the design (and why)

1. **`SubagentSpawned` carries no `kind`** (`crates/agent/src/lib.rs` `AgentEvent::SubagentSpawned { parent, child, name, cap }`), so the
   §7.5 guard `row.info.kind != Advisor` cannot be written on that event. The TUI guards by the link's id instead
   (`child == self.advisor.advisor`, known from `AdvisorChanged`/`AdvisorStatus`) and by `kind` on the `list_agents` reply. A stale engine
   that emits `SubagentSpawned` for the advisor *before* any status reached the TUI would still put it on the board until the next
   `list_agents` (which filters it). Hand-off below.
2. **The `Off` lead row shows on both browse levels** (providers and one provider's models), not only the providers level: 124 §3.5's
   single-provider shortcut opens the picker directly on the models level, where an `Off` row would otherwise be unreachable. Search mode
   has no `Off` row (type `/advisor off` or clear the search).
3. **The `advisor → …` line is printed from `set_advisor_status`**, not from the `AdvisorChanged` arm alone: the RPC reply
   (`UiEvent::AdvisorStatus`) and the bus event race; printing on whichever lands first (model or off-state changed) gives exactly one line.
   Consequence: a session opened with a surviving link prints `advisor → <model>` once on load.
4. **`AdvisorChanged`/`AdvisoryAdded` are handled before the streaming-tail bookkeeping**: the main `match ev` finishes an open
   `Entry::Assistant` on any non-`TextDelta` event, and `AdvisorChanged` fires on every counter change mid-stream — handling it there
   would split the reply.
5. **`UiEvent::ModelSet` is not the chain point for the advisor** (the reply of `set_advisor` is an `AdvisorStatus`, not a `ModelInfo`):
   the chain reads the levels from `catalog.closest(advisor.model)` when the reply is marked as a pick.
6. **C's test fixtures**: `d3c5b682` added `kind: Default::default()` to two `AgentEvent::SubagentSpawned` literals in `model.rs` tests;
   the variant has no such field, so `cargo test -p tui` did not compile at the contract commit. Removed (the five `AgentInfo` fixtures keep `kind`).
7. **`k()` renders `120k`, not the design's `120.4k`**: the existing helper drops the decimal from 100k up (`model.rs` `k`); §7.3 names
   `k()` as the formatter, so the code's output stands and the test asserts `in 120k`.
8. **`/advisor` with no session yet** (`status`/`off`/`on`/`<id>`) prints `no session yet: send a message first` instead of sending
   (the RPC needs a `SessionId`); bare `/advisor` still opens the picker, and ⏎ on a row then prints the same error.

## Hand-offs for the closer

- **W1 / closer — engine**: never emit `SubagentSpawned` for the advisor child (§4.2), or add `kind: SessionKind` to the event; the TUI's
  id guard covers the normal order (status before spawn) only.
- **W1 — history**: `Agents::history` / page loading must emit `HistoryEntry::Advisory` (C's as-built hand-off); until then an advisory
  that arrives as `HistoryEntry::User` with an `<advisory …>` wrapper is dropped silently by `user_entries` (no `◆ you`, no card).
- **Closer — `tests/no_emoji.rs`**: `crates/surface/src/elk_family.rs:442,468,928` (`▶`) and `crates/surface/src/export.rs:534,541,800,
  822,824` (`⚠`, `☹`, `☺`) fail the no-emoji scan on main since design 123; either swap the glyphs (`▸`, `!`, `:(`/`:)`) or end those lines
  with `// no-emoji: allow`. Outside W2's ownership.
- **Engine stub**: `set_advisor` currently replies `Err("advisor: not built")` → the TUI shows it as `UiEvent::Error` text; nothing to
  change here once W1 lands.

## Open items

- The status row is now a left `View` of two `Text`s; `m.status()` remains one SAND string (R3 §5), only the advisor segment carries colour.
- No new key chord (design §7.5): Ctrl+O still toggles the newest expandable entry; an Advisory card with ≤ 3 notes is skipped.
