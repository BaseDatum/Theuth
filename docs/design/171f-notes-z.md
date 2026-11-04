# 171F-Z — closer notes (decisions)

- **Merge conflicts** (u1b onto u1a, all import/test-list unions; no fix dropped):
  - `components/cells/CellFence.tsx` imports: kept U1a's `followSuperseded, RunbookBar, runAllPairs` + `lastPass, scopeKey` and added U1b's `chatEditable`.
  - `components/cells/NoteCell.tsx` imports: kept U1a's `lastPass, scopeKey, useCells` and added U1b's `interpreterText`.
  - `components/cells/store.test.ts`: union of both import lists; kept U1a's watch-only register test AND U1b's B5 `exitText` test.
  - e1, e2, u1a, u2 merged clean (E1/E2 `actor.rs` hunks were disjoint).
- **U1b ceiling closed**: `OutputArea` now scopes `OutputDisclosure` per output index (`disclosureKey(base, index)`), so the stream toggle no longer needs `text.length` in its key (a growing stream kept resetting; two same-length streams shared a toggle) and two JSON outputs of one run no longer share a path toggle.
- **E2 `.cell.log` rename verified**: only `tools/src/pty.rs` builds a kernel log path (`<id>.cell.log`); `jobs.rs:1210` builds non-PTY job logs (never cells). Every consumer (durability journal, replication, UI "Open log", reaper) takes `job.log`; no `<id>.log` rebuilders found under tools/agent/rpc/desktop/theuth.
- **E2 joined ansi rows vs U1b `trimBlankTail`**: the trim works on line ends, independent of how rows were joined; no change.
- **B14 actor test**: added to `actor/tests.rs` `e2e_real_shells` on the stopped `sleep 5` run (`r_8`): `out.txt` has no `133;D`, no `printf`.
- **U2 `registeredCells`**: still used by `CellFence.tsx` (B8 successor specs, `after` labels); kept.
- **U1a marker**: `open-as-note.ts` `ponytail:` → `limit:` as asked.
- **B29**: `crates/cells/cells.md` §5 now says `doc_insert` refuses the whole insert and names each problem; the engine refusal is unchanged. Also updated the two old Monty quotes (§4 and the worked example) to E1's new reason text.
- **171-cells.md**: besides the asked §6.3/§7.3/§11.2/§11.3, updated §6.2 (Monty reason + Use Python → `auto`), §7.2 (pass end on `cell_state.pass`) and §8.3 (scrolled-out head, line-aligned cuts), because they quoted behaviour 171F changed.
