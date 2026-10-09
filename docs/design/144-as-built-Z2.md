# 144 NOTEBOOKS wave 2 — as built, Z2 (closer): integrate E1 · E2 · U1 · U2, fold main, bindings, gates, live proof

Brief: `~/Develop/theuth-144-briefs/wave2/Z2.md`. Contract: `docs/design/144-wave2-contract.md`. Worktree
`/Users/Sky.Moore/Develop/theuth-144-z2`, branch `integrate/144-w2` off `feat/144-w2-base` 6ab1905f. Order change
01:10: the UI phase ran first (U1/U2 finished before E1/E2); the engine phase and the live proof follow on R1.

## Integration

**UI phase (turn 1)**
- `bc561df3` merge `feat/144-u1` (af578a75) — clean, no conflicts.
- `29a5ac55` merge `feat/144-u2` (020c5fcc) — clean, no conflicts: U1 and U2 both touched `NotesColumn.tsx`'s
  `OverviewsSection` line identically (U1 kept the one line the contract §5 names; U2 replaced only the placeholder bodies of
  `NotebookHeaderActions.tsx` / `OverviewsSection.tsx`). `NotebookPage.tsx` is U2's. 48 files, +2710/−239 over the base.
- `origin/main` = dd19bf63 at merge time, already the base's parent → nothing to fold yet (re-checked before each gate).
- U2 hand-off applied: `crates/desktop/src/commands.rs:42-43` — `popout-notebook-*` windows open at 1100 × 720
  (notes keep 880 × 1000, everything else 960 × 720). Tauri crate compiles in the engine phase's workspace check.
- Deferred to the engine phase (needs the regenerated bindings): the eleven `rpc/api.ts` `Api`/`METHODS` additions
  (U1's seven context/insight methods, U2's four ask/overview methods) and flipping `ContextApi`/`AskApi` to `Pick<Api>`.
  `api.test.ts` pins `METHODS` to `bindings/methods.json`, which only the real export produces.
- Bindings for tsc: the base export (795 files, orchestrator's copy, `/tmp/144/w2/bindings.log`) + the workers' hand-written
  stubs `/tmp/144/w2/u1-stubs/*` (5) and `/tmp/144/w2/u2-stubs/*` (9) copied over them → 806 files, gitignored.

**Engine phase (R1)** — pending.

## Gates

**UI phase (turn 1)** — logs in `/tmp/144/w2/z2/`:
- `node_modules` was missing in all three packages (fresh worktree): `BUILD_LABEL=144-Z2 build-lock bun install
  --frozen-lockfile` in `crates/desktop/ui` (622), `packages/apps` (484), `packages/documents` (107) — rc 0 each.
- `BUILD_LABEL=144-Z2 build-lock bun run check` (tsc -b, 1609 root files) → rc 0, **0 errors** (`tsc.log`; re-run after the
  fixes below: `tsc2.log`, 0 errors).
- Full `BUILD_LABEL=144-Z2 BUILD_SLOTS=1 build-lock bunx vitest run` → first run **227 files: 224 passed, 3 failed; 1505
  tests: 1502 passed, 2 failed, 1 skipped** (`vitest.log`, 136 s). All three reds are integration-only, caused by this wave:
  1. `src/components/notebooks/NotesColumn.test.ts` (suite) — U2's `OverviewsSection` imports `useFileUrl` from
     `chat/Attachments`, which pulls `MessageAssistant` → `markdownComponents.a = CitationLink`; U1's test mocked
     `chat/Citations` with `citationCount` only (green on U1's branch, where the section was a placeholder). Fix: the
     mock gains `CitationLink: () => null` (`NotesColumn.test.ts:25`).
  2. `src/lib/meeting-link.test.ts` — U2's `openPopout` passed `window.open(url, label, undefined)` for every kind; 143's
     pin expects two arguments. Fix in `lib/popout.ts:130-131`: the features string is passed only when `popoutSize`
     returns one (notebook); other kinds keep the two-argument call.
  3. `src/lib/mobile-zoom.test.ts` — the two-line scan flagged `AskDialog.tsx:76`: the `Textarea` is fine, the `text-sm`
     belongs to the `<label>` on the next line. Fix: a JSX comment line between them (`AskDialog.tsx:77`); no style change.
  Re-run of the three files + `popout.test.ts` + `AskDialog.test.ts` → 5 files / 18 tests green (`vitest-rerun.log`,
  `vitest-rerun2.log`). No wave-1 or 140 test changed.
- `BUILD_LABEL=144-Z2 build-lock bun run build` (vite) → rc 0, 14072 modules, built in 22.4 s (`vite-build.log`);
  `routeTree.gen.ts` unchanged by the build (U2's hand-merged `/ask_/$session` matches the generator).
- `origin/main` re-fetched after the gates: still dd19bf63 → no fold, engine gates not re-run in this turn (no engine
  code moved; they run in full in the engine phase).

**Engine phase (R1)** — pending.

## Live proof

Pending (after the engine phase; recipe: `144-as-built-Z.md` §Proof on `/tmp/144-engine-w2`).

## Deviations

- Nothing to fold from `origin/main` in turn 1 (dd19bf63 is already an ancestor); the brief's known overlaps
  (`slash.ts`, `nav.ts`, `SectionBand.test.ts`, `model.rs`, `side_call.rs`) are re-checked when main moves.
- The `commands.rs` pop-out size hunk is applied but not yet compiled (no cargo in the UI turn); it is covered by the
  engine phase's `mbx check --workspace --all-targets`.
- `mobile-zoom.test.ts`'s two-line heuristic was satisfied with a comment line rather than `// mobile-zoom: allow` (the
  opt-out is a line-end `//` comment, impossible inside JSX attributes) or restyling the label (`md:text-sm` would change
  the phone's label size for a false positive).

## Hand-offs

(none yet)
