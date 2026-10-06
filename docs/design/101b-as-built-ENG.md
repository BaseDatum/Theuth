# 101b — as built (ENG: crates/tasks, crates/document, crates/roster, crates/rpc)

Branch `feat/101b-eng`, base `f83042b7` (Revision 2 docs). Design: `docs/design/101-decision-review-surface.md`
§3 + **Revision 2 — MINI-BOARDS** (R1, R2, R3-docs, R6). Contract to the UI worker: `Packet.board: string | null`.

## What was built

- **R1 `Packet.board`** — `crates/tasks/src/packet.rs:178` `pub board: Option<String>` (`#[serde(default)]`, so rows
  written before this revision deserialize with `board: None`); `Ask.board` (:198); `Escalate.board`
  (`crates/tasks/src/lib.rs:252`), `ReviewPacket.board` (:295); `EscalateArgs.board` (`crates/tasks/src/tools.rs:1240`),
  `TaskUpdateArgs.board` (:477), `TaskMoveArgs.board` (:655); carried through `review_gate` (:741), `ledger.escalate`
  (`crates/tasks/src/ledger.rs:1450`), `ledger.review_packet` (:1533, new `board` parameter). Stored in the packet
  JSON column — no migration. `thread_markdown` (`packet.rs:427`) adds `Board: theuth://notes/<id>` after the summary
  (the link form `doc_create` returns and the board skill ends with).
- **D1 extended** — `require_for_user` (`packet.rs:361`): both kinds need `summary` AND `board` (`pointers` no longer
  required for a review). Texts: `NEEDS_BOARD_DECISION` (:35) / `NEEDS_BOARD_REVIEW` (:42) carry the recipe
  (`doc_create(kind="board", root=…)` → `doc_insert` → `board=r["doc"]`) and the skeleton headings verbatim;
  `NEEDS_SUMMARY` (:29) / `NEEDS_REVIEW_SUMMARY` (:32) now say "one short paragraph — the abstract". `NEEDS_POINTERS`
  removed. Board missing is reported before summary missing (the board text also names the summary).
- **R2 skeleton validation** — `packet::check_board(kind, &[(level, text)])` (:377): the first three `##` of
  `DECISION_SECTIONS` (:50) / `REVIEW_SECTIONS` (:58) in order, compared by `fold` (:415: lowercase, curly → straight
  apostrophe, whitespace collapsed); `#`, `###` and later `##` ignored; the error names the section number, the
  expected heading, what was found (or "is missing") and the whole skeleton.
  - Document side: new message `Headings(DocId) → Result<Option<DocHeadings {kind, headings}>, String>`
    (`crates/document/src/actor.rs:1519–1565`): `None` when the id is neither open nor on disk; else the kind from
    `meta` and the helper's `outline(depth=1)` reduced to its `[hN] …` heading blocks (`fn headings` :1567).
    Exported from `document` (`lib.rs:35`).
  - Tasks side: `Thread::headings(doc)` (`crates/tasks/src/thread.rs:23`) — the ledger's existing seam to the
    document plugin; `DocThread` (:83) does `Handle::lookup()` (the `ActorRef::lookup(DOCUMENT_ACTOR)` idiom),
    `DocId::parse` (malformed id → `Ok(None)`), then asks `Headings`. `ledger.check_board(kind, id)`
    (`ledger.rs:1566`): unknown → "board `<id>` does not exist"; wrong kind → "`<id>` is a note, not a board (create it
    with doc_create(kind=\"board\"))"; else `packet::check_board`, prefixed "board `<id>`: ".
  - Enforced for the user audience only, before any write, where D1 runs: `ledger.escalate` (:1479–1483) and
    `review_gate` (`tools.rs:774–784`) through the new actor message `CheckBoard {kind, board}` (`lib.rs:314`, handler
    :589) — `review_gate` already asked `EffectiveReviewer` before the write.
- **Roster** — `attention.rs` unchanged apart from the test fixture's new field. Bindings: `just ts` regenerated
  `bindings/Packet.ts` with `board: string | null` (line 43).
- **R6 prompts** — `escalate` (`tools.rs:1231`), `task_update` (:498), `task_move` (:673) descriptions name the
  mini-board, its skeleton headings and the lens ("write for the person deciding — what, why it matters to them,
  the whole picture; diagram and link heavy; short"); argument docs for `summary`/`board`/`pointers` updated.
  `desk.rs manual` step 2(iv) (`crates/roster/src/desk.rs:77`) and step 4 (:92) say the same, one sentence each.
- **R3 skill** — `crates/document/board.md:66–141` "## Mini-boards for decisions and reviews" (76 lines) after
  "## Sections, in this order": both skeletons with one line per section, the lens, the ref/link rule, the
  `root` note, how to pass the id, and a six-block decision example (frontmatter `root:` line, five `##`
  sections, one `Graph` with `ref`s, one `###` per option, one `Excerpt`) inside a four-backtick fence so the
  inner `json-render` fences stay intact; `board_skill_examples_validate_and_rules_are_present` validates them.

## Tests added / updated

- `packet::tests`: `check_board_matches_the_first_three_sections` (right order both kinds; case / whitespace /
  curly apostrophe accepted; `#`, `###`, later `##` ignored; missing second; swapped first two; too short; empty;
  a `###` in a section's place), `user_audience_rule` (with/without board for both kinds, blank board, summary
  after board, the texts carry the headings), `thread_block_and_head` (the `Board:` line), `wire_shape_matches_the_contract`
  (old JSON without `board` → `None`; `board` serialized).
- `ledger::tests`: `escalate_to_user_needs_a_summary_and_a_board` (no board → "needs a `board`" with the headings;
  `nope` → does not exist; a note → "is a note, not a board"; two-section board → "section 2 … must be"; board but no
  summary → "needs a `summary`"; nothing written: thread, state, blockers, wakes, packets),
  `escalate_with_a_packet_stores_and_mirrors_it` (refused without a board, then stored with `board = b1` and the
  thread carries `Board: theuth://notes/b1`), `review_packet_replaces_and_mirrors` (board stored and replaced),
  `escalate_user_path` passes a board. `MemThread::headings` (:2094) derives kind (a `---\nroot:` first block = board)
  and headings from the stored blocks; fixture `board(thread, id, sections)` (:2122).
- `tools::review_gate_tests::review_gate_enforces_the_user_audience`: no board → refused naming `board`; summary
  alone → refused with `## What was done`; explicit `reviewer: user` → refused; through the spawned `Tasks` actor an
  unknown board and a two-section board are refused with nothing written; happy path stores `board = rb`, replaced on
  the second hand-over.
- `thread::tests::headings_reach_the_document_actor` (the actor hop): spawns a real `DocumentActor` over a temp store
  with a `sh` helper whose `outline` answers one canned tree, registers it as `DOCUMENT_ACTOR`, creates a board and
  a note; `DocThread::headings` returns `Board` + `[(1,"T"),(2,"What was done")]`, `Note` for the note, `None` for an
  unknown id and a malformed id. New dev-dependency `figment` (workspace) for `runtime::Runtime::new`.
- `document::actor::tests::headings_reads_the_outline_tree` (frontmatter / paragraph / code blocks skipped, empty
  heading text kept).
- `roster::attention::tests` fixture gains `board: None`.

## Gates (this worktree, toolchain 1.98.1-aarch64-apple-darwin)

```
cargo test -p tasks -p roster -p rpc -p document --no-fail-fast   # EXIT=0  (log /tmp/mc-2026-10-05/101b-eng-test.log)
  document  unittests: 45 passed; tests/actor.rs: 11 passed
  roster    unittests: 51 passed
  rpc       unittests: 36 passed; tests/prompts.rs: 1 passed
  tasks     unittests: 78 passed
cargo check --workspace                                            # EXIT=0, Finished in 9m 55s (future-incompat note: block v0.1.6, pre-existing)
just ts                                                            # EXIT=0 (8 export tests); bindings/Packet.ts line 43: `board: string | null,`
```

rustfmt (edition 2024) per touched file; `crates/tasks/src/place.rs` reformatted as a side effect and reverted (as
in wave 101). `bindings/` is gitignored; nothing to commit there.

## Deviations

- **Where the R2 hop lives.** The brief suggested asking the document actor directly from the ledger/tool; the
  ledger already reaches the document plugin through the `Thread` trait (`thread.rs`), so the hop is one more method
  on it (`headings`) rather than a second lookup path. Tests keep `MemThread`; one real-actor test covers the hop.
- **Documents unavailable** (`Handle::lookup()` is `None`, plugin disabled or not started): `check_board` returns
  `Err("documents are not running; the board cannot be read")`, so an escalation / hand-over to the user is refused.
  A board cannot exist without the plugin, so there is nothing to trust; chosen over skipping the check.
- **Message name**: `Headings(DocId)` instead of the brief's `Outline(DocId)` — `document::Outline` already exists
  (the §5.1 outline `doc_read` uses).
- **Unknown-id detection** in the `Headings` handler is "not open and `read_meta` fails" — a corrupt `meta.json`
  reads as "does not exist" rather than a read error.
- **R3 "or carry an external URL"**: the surface validator refuses a `ref` with a scheme
  (`crates/surface/src/validate.rs` "path must not carry a scheme"), and `GraphNode` has no URL field, so the skill
  says: every unit `ref`s `path#La-Lb` or `block:<id>`; a unit that is not code is left out or named in the label of
  the unit that calls it; PRs / dashboards / invoices are plain markdown links in the prose. The example follows that.
- **Error order** in `require_for_user`: missing board is reported before missing summary (the board text also
  mentions the summary), so an agent that gave neither gets the bigger instruction first.
- **`packet_wanted`** for an agent audience now also includes `board.is_some()` (a board given to an agent manager
  is stored and shown).
- `thread_markdown` writes `Board: theuth://notes/<id>` as a bare URL (no other thread text uses markdown links).

## Not verified

- The `escalate` / `task_update` / `task_move` tool `call` bodies end-to-end through the tool registry (need a
  `ToolCx`); `review_gate`, the `Escalate` handler's `Ask` construction and the ledger paths are tested.
- `Headings` against the real `theuth-documents` helper (its outline `[hN] text` format is read from
  `packages/documents/src/convert.ts` `tag()`/`outlineLine()`; the test uses a `sh` helper with the same shape).
  Heading text in the outline is truncated at 120 chars — longer than any skeleton heading.
- The desktop reader of `Packet.board` (UI worker, `feat/101b-ui`).
