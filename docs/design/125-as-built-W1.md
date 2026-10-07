# 125 as-built — W1 (engine)

Branch `feat/125-w1`, cut from the contract commit `d3c5b682`. Spec: `docs/design/125-advisor.md` §3–§5, §10 "W1", §11 "W1";
decisions D1–D9. Built against C's as-built (`125-as-built-C.md`): `create_session_kind`, `*_incl`, `store/advisor.rs`,
`SetAdvisorDefaults → Result<AdvisorDefaults, String>`. Nothing in `lib.rs` types, `store.rs`, `store/advisor.rs`, `rpc`,
`tui`, `desktop` or the channel crates was changed (one `pub mod advisor;` line in `lib.rs`).

## Built (file-level)

- `crates/agent/src/advisor.rs` — new: the `Advisors` kameo actor (`ADVISORS_ACTOR = "advisors"`), one `Link` per advised
  session: `AdvisorLink` row + delivered-prefix fingerprints, `Guard`, `steps`/`immune_until`, `backlog` (pending steps),
  the in-flight `Review` (held notes, `advised`, `unknown_tool`, `tainted`, streamed text), `last_update`/`last_opener`,
  failure latches (`failures`/`dropped`/`quarantines`), `reprimed`, `retained`, parked D5 `waiters`, counters.
  Messages: from `Agents` — `Attach { link, context_limit }` (ask → `AdvisorStatus`; seeds `cursor` at the current rendered
  length, D1), `Detach(session)`, `StatusOf(session)`, `SetDefaults(AdvisorConfig)`; from the advised loop —
  `AdvisorStep { session, turn, will_continue }` (ask, `DelegatedReply`, parked while `backlog ≥ sync_backlog` at a terminal
  step), `Advise { advisor, note, severity }` (ask → ack string); internal `Retry`. To `Agents` (tell only — the two actors
  never wait on each other): `AdvisorInput { advisor, text }` (one update = one turn), `DeliverAdvisory { session, advisory,
  text, steer }`, plus the existing `Cancel` / `EditTurn` / `Regenerate` (the latter two through `restart()`, a detached
  retry of "turn in flight": the advisor's `TurnFinished` is published a hair before its state is handed back).
  `impl Message<AgentEvent>`: advisor sessions — `TurnStarted` (opener seq), `TextDelta` (quarantine scan), `ToolCallFinished`
  (unknown-tool refusal), `Error` (classification text), `ContextUsed` (re-prime at `compact_at × context_limit`, once per
  batch, via cancel → `RestartTurn` at the first opener with the pending update), `TurnFinished` (tokens, `finish_review`);
  advised sessions — `TurnFinished { cancelled }` (review cancelled, held notes preserved as a card), `SessionDeleted`.
  `finish_review` (§4.13): permanent regex → `halted`; quota regex → `paused` + `retained`; overflow → one re-prime;
  retriable → up to 3 attempts 1 s apart (`Regenerate` of the update opener) then the batch is dropped, 3 drops → `halted`;
  quarantine (unknown tool with no `advise`, or an override/destructive/denial pattern in a note or the reply that the
  update did not contain) → silent replay, second → `advisor quarantined twice — reset` card + replay, third → `halted`;
  a good turn resets the latches and delivers the held notes as one passive card. Boot `load()` (§4.14): cursor re-seeded,
  empty prefix, restart preface, `reviewing → idle`, `paused`/`halted` kept.
- `crates/agent/src/advisor/render.rs` — `render_update(rows, UpdateOpts { steps, will_continue, preface, reasoning })`
  (§4.5 format: markers shared by consecutive same-role rows, `_thinking:_` 400 chars, `→ tool(primary ≤ 120) ⇒ ok · N lines`
  / `⇒ error · N lines — first` / `⇒ pending`, `Tool result:` fenced and capped 8 KiB / 80 lines middle-elided, harness
  wrappers as one `_(harness input: tag from X, N lines)_` line, `<message>` as `from` + first line, `_(context compacted —
  epoch N)_`, 48 KiB cap eliding oldest tool results then oldest rows, `(N steps)` heading, `[in progress — more steps
  follow]` trailer); `fingerprint(entry)` over the variant tag and content fields only; `counted()` (Advisory/Error rows
  excluded).
- `crates/agent/src/advisor/guard.rs` — `Guard`: `normalise` (lowercase, non-alphanumeric runs → one space), the verbatim
  omp noise list, FIFO-bounded (4096) dedupe with rank escalation, per-update non-blocker budget (1–32) with displacement of
  the lowest-rank pending slot, `begin_update` (slots only) / `reset`; the six §4.8 ack strings.
- `crates/agent/src/advisor/advise.rs` — `NAME`, `spec()` (§5.3 description, `AdviseArgs` schema), `parse`, `card_text`
  (`<advisory severity=… guidance="weigh, don't blindly obey">` per note, body XML-escaped).
- `crates/agent/src/advisor/identity.rs` + `advisor/system.md` — `identity(data_dir, max_notes)`: the §5.1 prompt verbatim
  (`include_str!`, `{{max_notes_per_update}}` substituted), `<project-context>` with `prompt.md` (32 KiB cap),
  `~/.theuth/advisor.md` inside `<attention>` when present.
- `crates/agent/src/supervisor/advisor.rs` — the real handlers: `SetAdvisor` on → `create_session_kind(…, Advisor)` then
  `Agents::session` (resume path: the row exists), `install_cap(ReadOnly)`, registry `SetApprovals { enabled: false }`,
  `SetToolsetOverride` with the ten data tools (`runtime: false`, `hint_limit: Some(0)`), `set_effort(manual: false)`;
  re-pick → `set_model/set_effort { manual: false }` on the advisor; `advisor.model`/`advisor.effort` written with
  `write_config_value` (never `agent.*`); `Attach` ask; `SetAdvisorOn(true)` to the live advised actor. Off → `Detach`,
  `SetAdvisorOn(false)`. `AdvisorStatusOf` → `StatusOf`. `SetAdvisorDefaults` → range checks (1–32 / 0–10), config writes,
  live `Agents.advisor_cfg`, `SetDefaults` to the actor. `AdvisorInput` → `ledger.admit(Automatic)` + `route` (the `inject`
  path minus `MessageDelivered`). `DeliverAdvisory` → `SteerAdvisory` (with an `Automatic` ticket) or `AppendAdvisory` to
  the advised session's actor.
- `crates/agent/src/supervisor.rs` — `Agents.advisors: HashSet<SessionId>` and `Agents.advisor_cfg: AdvisorConfig`;
  `start()` reads the `advisor:` section into `cfg.advisor` and starts `Advisors` right after `Agents` registers (before the
  activity registration that triggers `resume_scan`); `Agents::session` carries `row.kind` into the actor (`with_kind`) and
  the link flag (`with_advisor`), records advisor ids in `advisors` and `Policy::record_hidden`; `identity_of` returns the
  advisor identity for advisor sessions; `peer_context` is `""` for them; `resolve` and `enqueue_peer` refuse an advisor id
  (`… is not a peer`); `ReadSession` refuses an advisor target; `render_with(messages, entries, thinking, advisories)` emits
  `HistoryEntry::Advisory` and treats such rows like vision rows in `turn_openers` (`render` delegates with an empty map;
  `ReadSession` and `LoadSession` use `render_with` with `store.advisories`); `load_topology` records every linked advisor
  as hidden; `ShowPrompt` passes `advised`; test-only `test_auth()`.
- `crates/agent/src/loop_.rs` — `Shared.kind: SessionKind`, `Shared.advisor: Arc<AtomicBool>` (+ `Session::with_kind` /
  `with_advisor`); `Input.advisory: Option<Advisory>`, `Input.passive: bool`; `impl Message<SetAdvisorOn | AppendAdvisory |
  SteerAdvisory> for Session` (idle: passive append without a turn / blocker opens a turn / concern joins queued inputs or is
  preserved; busy: onto `shared.steer`); `CancelTurn` marks queued advisory steers passive (D4); `TurnDone` appends
  passive-only leftovers without a turn; `take_input` stores an advisory row with its metadata (`append_advisory` +
  `input_accepted("advisory")`, no clock stamp) and publishes `AdvisoryAdded`; `Turn::advisor_step` — the mid-turn hook at
  the top of the `Round::Tools` arm (tell) and the terminal hook after `finish_turn`, before `PeerTurnFinished`/`TurnFinished`
  (ask, `timeout(CATCH_UP_WAIT)`); `Turn::advise` + loop-local dispatch beside `todo`/`python` for `kind == Advisor`;
  `rebase` uses `frame::advisor_baseline(identity)` (identity from the supervisor, else computed locally), passes `advised`
  to `frame::baseline`, drops `todo` and adds `advise` to the direct array for advisors, skips the `## Runtime` block;
  `round` skips the compaction trigger / `<context_pressure>`, `<todo>` and `<knowledge>` rows and the overflow compaction
  for advisors.
- `crates/agent/src/frame.rs` — `baseline(…, advised: bool)` inserting `ADVISORY_PARAGRAPH` after the plugin blocks, before
  the guidance; `advisor_baseline(identity)` = identity + `Working directory:` / `Date:` lines only.
- `crates/agent/src/communication.rs` — `Policy.hidden` + `record_hidden(id)`; `route` refuses a hidden end as
  `PolicyError::UnknownSession`.
- Tests: `crates/agent/src/advisor/tests.rs` (`advisors_contract`: a real `Agents` registered as `AGENT_ACTOR` + registry +
  scripted wiremock gateway; scenarios 1, 2, 4, 5, 6, 7, 8, 9, 10, 11, off/defaults; `restart_reseeds_and_prefaces`: 12),
  `crates/agent/src/loop_/advisor_tests.rs` (hooks per batch / at turn end + the advised paragraph; passive asides never
  open a turn and land after the batch; idle concern preserved vs blocker opening a turn; Stop preserves a queued steer as a
  card with no auto-resume; an advisor-kind session runs `advise` loop-locally with the ack as the result, refuses `todo`,
  and sends the bare `<conventions>` prompt), unit tests in `render.rs`, `guard.rs`, `advise.rs`, `identity.rs`, `frame.rs`.

## §4.2 exclusion checklist

- [x] Sidebar / `AgentTree` — `Store::list_sessions` / `descendants` hide advisors (C); `Agents::agents`/`list_sessions` use
      them unchanged.
- [x] Subagent strip, Agents page, `/agents` — `ListAgents` → `descendants` (hidden); pinned in `isolation`.
- [x] Roster line — `Roster` → `agents()`; pinned (`""`).
- [x] `discover_peers` / `send_message` — `directory` over `list_sessions` (hidden); `resolve` refuses `session:<advisor>`
      with "not a peer"; `enqueue_peer` refuses; `Policy::route` refuses hidden ends. Pinned (`ListPeers`, `Resolve`).
- [x] `## Peers` block of the advisor — `peer_context` returns `""`.
- [x] Search — store queries exclude `kind = 'advisor'` (C); pinned via `search_titles` in `isolation`.
- [x] Attention inbox — nothing to filter (read_only cap, approvals bypassed, no `ask_user`/`show` in the toolset).
- [x] Channels — never bound; events ignored by id as any unbound session's.
- [x] Durability resume — not filtered; an advisor's open turn resumes like any session.
- [x] Cost — not filtered; `session_cost` walks `cost_owners` (advisor included); `advisor_status.cost` = the advisor's own total.
- [x] `session_read` of an advisor id — refused "not a peer".

## Gates (exact result lines)

- `cargo test -p agent --lib -- advisor:: loop_::advisor_tests` → `test result: ok. 17 passed; 0 failed; 0 ignored; 0 measured; 430 filtered out`
  (the brief's form `cargo test -p agent --lib advisor:: loop_::advisor_tests` is rejected by cargo — two positional filters
  need `--`).
- `cargo test -p agent` → `test result: ok. 446 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 54.60s`
  (+ `0 passed` doc-tests). No `jobs::` flake on this run.
- `cargo clippy -p agent --all-targets -- -D warnings` → `Finished dev profile` — no warnings.
- `cargo fmt -p agent -- --check` → one diff, `crates/agent/src/store/accounting.rs:374` (`reserve_call_in` call wrapping) —
  pre-existing at `d3c5b682` (C's file; W1 reverted the churn `cargo fmt` applied to it). Every W1 file is fmt-clean.
- `cargo check -p rpc -p tui -p theuth` → `Finished dev profile` (a first attempt died in `sentencepiece-sys`'s native build
  script right after the machine reboot; the rerun passed — nothing of W1's is involved).

## Deviations from the design (and why)

1. **Coalescing without rounds.** The actor keeps no `pending` deltas: a step while reviewing bumps `backlog`; when the review
   ends, everything past the cursor is rendered as one update headed `(N steps)`. No wait for more deltas, so the "≤ 3
   coalesce rounds" cap has nothing to cap. Same observable behaviour, less state.
2. **Steer delivery is a loop message, not `UserInput`.** `UserInput` is destructured in `acp_child.rs` (not W1's) and
   `passive` must never cross the wire, so the card travels as `SteerAdvisory`/`AppendAdvisory` to the native actor with the
   `Automatic` ticket admitted by `Agents` (`DeliverAdvisory`). The channel decision (§4.10 table) lives in the actor, which
   knows idle/busy/queued: blocker idle → turn; concern idle → joins queued inputs, else preserve; everything busy → after
   the batch. Approval/`ask_user` waits need no special case: nothing lands inside a batch, so a steer waits for the batch
   boundary after the answer, exactly the table's "next boundary after the answer".
3. **Advisory row metadata** is written with C's `append_advisory` + `input_accepted(…, "advisory")` (two transactions) —
   `append_input` lives in `store/durability.rs` (C's) and has no metadata parameter.
4. **`render` keeps its signature**; `render_with(…, advisories)` is the typed form. `Store::rows_since` (C's) keeps its
   post-pass — hand-off below.
5. **`advisor_baseline(identity)`** computes the two header lines itself (the design named a `(identity, header)` pair).
6. **Guard normalisation has no NFKC** (`unicode-normalization` is not a dependency of the crate); lowercase + alphanumeric
   collapse only — `ponytail:` note in `guard.rs`.
7. **`halted` info line**: no separate transcript info row; `AdvisorChanged.detail` carries `advisor halted: <first line>` /
   `advisor paused: …` / `review dropped: …` for the UIs to print (W2/W3 print one info line per change already).
8. **Advisor identity fallback**: `identity_of` (live `max_notes_per_update`) is the source; a loop without a supervisor
   (tests) computes `advisor::identity::identity` from the boot config.
9. **A paused batch is regenerated on re-pick** via `Regenerate` of the advisor's last update opener (or the update text
   re-sent when it never started).
10. **`SetAdvisor` validates the model only through `Catalog::resolve`** (`provider/model` always passes through); an
    unconfigured provider surfaces as the advisor's first turn error → `halted` with the provider's line (smoke step 9).

## Forced edits outside W1's ownership (each one line)

- `crates/agent/src/lib.rs`: `pub mod advisor;` (module declaration only; no type touched).
- `crates/agent/src/loop_/vision_fallback_tests.rs:548`: the `Input` fixture gains `advisory: None, passive: false`
  (`Input` grew two fields; this is the one literal outside `loop_.rs`).

## Hand-offs for the closer

- **`Store::rows_since`** (`store/advisor.rs`): call `crate::supervisor::render_with(&h.messages, &h.entries, &h.thinking,
  &advisories)` and delete its post-pass (W1's `render_with` does the same mapping and fixes `turn_openers` for rows after an
  advisory).
- **`cargo fmt -p agent`**: `store/accounting.rs:374` wants one wrap (pre-existing).
- **`crates/rpc/src/api.rs` `info()`**: still reads `rt.section::<AdvisorConfig>("advisor")`; `Agents.advisor_cfg` is the live
  copy — add an `AdvisorDefaultsOf`-style ask if `Info.advisor` must follow `set_advisor_*` without a config re-read (the
  setters do write `config.yaml`, so the file read is correct after each call).
- **Channels (§9)**: `AdvisoryAdded { session, seq, advisory }` is published for every card (steer or aside) — one arm each.

## Open items

- Advisor transcripts are reachable only through the engine's own debugging (`session_read` refuses them from any session);
  the design's smoke step 7 reads them that way.
