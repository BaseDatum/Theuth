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

**Engine phase (R1, turn 2)**
- `…` merge `feat/144-e1` (2aa03993) — clean. `a1498f77` merge `feat/144-e2` (2459df29) — 4 conflicts, both sides kept:
  `crates/agent/src/notebooks.rs:405` (E2's doc line on `invoke`), `crates/rpc/src/api.rs` trait method list (E1's seven
  then E2's four) and the two interleaved impl blocks (`notebook_context_set` … `insight_skills` from E1, `notebook_ask`
  … `notebook_overviews` from E2). `lib.rs` mods and `export.rs` lists auto-merged with both.
- Fold `origin/main` bbf68677 (146 + 146b + 140 windows fix): clean, 62 files. Migrations: main's 146 took 0043/0044,
  ours stays `0045_session_context.sql` — unique, no renumber. Second fold `05b9a8ae` = d5a8666e (148 MODEL-DEFAULTS):
  clean, 54 files; `slash.ts` keeps U2's `/ask` row + case beside 148's edits; `rpc/api.ts` METHODS gained 148's names.
- Hand-offs applied (`870b5497`, `3e22a9a5`):
  - **U1 dial before the first send** — `components/notebooks/NotebookContext.ts` `useEnsureNotebookSession(notebook)`:
    no thread yet → `create_session { retention: durable, notebook }` (wave 1's binding at creation), `rename_session`
    to the notebook title (what the composer does), remember it as the open thread (the composer re-keys onto it, so
    the first message goes there), `applyContextDefaults` (the device's remembered dials → rows), invalidate the chat
    list. `useDial` (`SourceCard.tsx:103`) awaits it before every `notebook_context_set/clear/set_all`; `ChatColumn`'s
    `onCreated` shares `applyContextDefaults`. Test `NotebookContext.test.ts` (2).
  - **E1 trailer** — `lib/citations.ts` `provenanceOf(md): 'chat' | 'source' | null` (the insight trailer `*From
    [<source>](theuth://sources/…)…*`, plain or emphasis-split); `isProvenanceLine` = `!== null`; the Notes column row
    reads `AI from chat` / `AI from source`. Tests in `lib/citations.test.ts`.
  - **E1 store** — `crates/agent/src/store.rs:2917-2921` `DELETE FROM session_context WHERE session = ?` inside
    `delete_session`'s transaction; asserted in `tests/notebooks_binding.rs::session_context_rows_round_trip_per_session`.
    `CallKind::Insight` not added (147-F1 owns `side_call.rs` tonight) — Hand-offs.
  - **`rpc/api.ts`** — the eleven methods on `Api` + `METHODS` (sorted positions); `lib/notebooks-rpc.ts` `ContextApi =
    Pick<Api, …>`, `useContextApi = useApi`; `lib/ask-rpc.ts` `AskApi = Pick<Api, …>`, `useAskApi = useApi`; the
    `contextApi`/`askApi` client shims and `ask-rpc.test.ts`'s shim test removed (`api.test.ts` pins the names).

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

**Engine phase (R1, turn 2)** — mbx under build-lock, logs in `/tmp/144/w2/z2/`:
- `cargo fmt --check` → clean (`fmt.log`).
- `build-lock mbx test -p agent -p document -p rpc -p voice -p decision --no-run` → 0 warnings (`test-norun.log`,
  `test-norun2.log` after the store hand-off). Bare `MBX_SCHEDULER_TESTS=0 mbx test` with the **same** `-p` set,
  `--lib --tests --no-fail-fast` (`test-run.log`): agent lib 579 + binding 2 + block 6; document lib 74 + actor 11 +
  cite_check_links 4 + index_search 3 + ingest_audio 1 + ingest_job 5 + insights 2 + replicate 1; rpc 56 + prompts 1;
  voice 27; decision lib 93 (+ embed_actor 3, gliner 3, model 4). Two reds, neither ours:
  `agent supervisor::tests::human_input_releases_parked_items_as_steers` (146 code; `a turn starts: Elapsed` while the
  decision lib tests held the machine for 1005 s) → **passes alone in 0.62 s** (`test-rerun-agent.log`);
  `decision tests/decide.rs::decide_many_releases_the_checkpoint_it_loaded` (`released 173 MB > before 71 MB + 50 MB`,
  a memory-accounting assertion) — the brief's `decision --lib only` exclusion; not run again.
- `build-lock mbx clippy -p agent -p document -p rpc -p voice --all-targets -- -D warnings` → clean (`clippy.log`).
- `BUILD_SLOTS=1 build-lock mbx check --workspace --all-targets --exclude desktop` → see below (`check-ws.log`).

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
