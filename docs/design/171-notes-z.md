# 171 CELLS — notes of the closer Z

## Merge (Phase I)
- Order per the relay's MERGE PLAN: A1 25da2e85 → U1 ff9ddc76 → D2 2865d21f → E2 031ebd4c → U2 fe528654 → fold origin/main (6f5e32cc: 173 watchdog + 175 MCP auto-lifecycle) → E1 (final sha from `171-E1.final`).
- Conflicts: only `crates/desktop/ui/src/components/cells/Cell.tsx` at U2 (footer: U1 R1's `masked` / `run.private ?? o.private` vs U2's `extras.footer` / `o.private`) → union: `masked`, `extras.footer`, U1's private line (the L11 field wins over the header flag).
- `crates/document/src/cells.rs`: D2 then E2 left ONE copy of E2's `171:E2-ADD` block (git merged the identical text).
- Fold of origin/main: clean, no conflict.

## Decisions
- `shellscript` (BlockNote's stored language for every shell block) parses as `bash` in `surface::cell::lang_of` and `grammar.ts`; parity case `lang-shellscript-is-bash` in `crates/surface/tests/cells.json`. `shell` is NOT added (no producer).
- `doc_export` goes through `document::reader_markdown(&docs, doc, None, annotate)` (PDF keeps `annotate: true`), so .md and PDF exports carry cell outputs + run lines. No rpc-level test exists for `doc_export` (needs the documents helper); `reader_markdown`'s own test covers the text; proved live in Phase II.
- Tray after a cancelled quit: `desktop::tray_menu` (extracted from `setup`) is re-set and the tooltip reset to `theuth` on `quit-cancel`; the title comes back with the webview's next badge write.
- `CellEvent` joined `WireEvent` (`rpc/events.ts`); the cells store uses the typed `on` instead of a cast.
- Monty sufficiency: both pool checks stay (`cells::monty::sufficient` caches, waits ≤ 5 s, answers `None` when unchecked; `agent::repl::monty_sufficient` answers `Ok` when unchecked) — they share the one static rule `surface::cell::monty_scan`, which is what the relay asks; merging them would change the "unchecked" semantics of one side.

## E1 wiring (after E1 ef30d5bb)
- Note scope in `Cells::{prepare, state}` through one `inspect_note`: the block via `note::source`, parsed with `surface::cell::parse` and digested over the block's WHOLE text (U2's `seen`); names / `after` edges from the note's markdown (`note::markdown`, new: replaces the unused `note::defaults(doc)`), `note::defaults_of` + `note::cell_cwd` for defaults and cwd, `ValidateCtx{chat:false, defaults, second_defaults, monty_ok}` (`monty_ok` only for `python: monty`), the `after` gate's current digest = its own block's text.
- Invariant 6: `prepare` calls `note::member_guard(src, latest, member_ack)` for every note press (Run all passes `None`: a member's unrun edit stops the pass). `cell_state` fills `member_edit` (`note::member_edit`) and `stale` (latest `presented_sha` ≠ the block digest). Test `actor::tests::member_edit_needs_the_members_ack` (fails before: the press ran).
- `resolve` takes the note defaults: `shell:` → `shell::resolve(kind, note_shell)`, `python:` default under the cell's own; `auto → Monty` = `interp::resolve(.., false)` failing `NoInterpreter`, then `monty::sufficient(code)` → retry with `true` (this cell only; ponytail-marked).
- One chip function: `interp::chip` (E2, §6.3) names python/Monty and delegates shells to E1's `shell::chip`; the kernel label and the run's `interpreter` use it.
- `deliver::cell_result` (the note path) takes `name` / `private` from the record (L11) — before, a private note cell's output reached the origin session. Test `deliver::tests::cell_result_takes_name_and_private_from_the_record`.
- `cell_output_file` serves `code` (U1/U2's stale and member views read it) besides `img-*.png`. Test `actor::tests::output_file_serves_images_and_code_only`.
- `cell_runs` rows: `name` / `private` from the record (A1's two `Z (L11)` points); `note_name` removed.
- `CellState.host` is the peer's mesh name (`mesh::Status` peers), else the id's first 8 chars (`rpc/src/cells.rs host_of`).
- `cells::InputPrompt` added to the `171:C` export block. U1's `cells/wire.ts` DELETED (not aliased): its five importers and U2's two local `…X` types use the generated `CellRun`/`CellState`/`KernelState`; fixtures carry the L11 fields.
- `note::DOCS_TEST` (test-only mutex): the two tests that install a fake document side hold it (the fake is process-global).
- UI: the gutter glyph `▶` (an emoji per `src/lib/no-emoji.test.ts`) → `►` in `CellEditor.tsx` (+ comments).
