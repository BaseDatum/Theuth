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
- `5c4ab0a2` merge `feat/144-e1` (2aa03993) — clean. `a1498f77` merge `feat/144-e2` (2459df29) — 4 conflicts, both sides kept:
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
- `BUILD_SLOTS=1 build-lock mbx check --workspace --all-targets --exclude desktop` → first run red on
  `nodes` (lib test) `couldn't read bindings/methods.json` (the file exists only after the export; `check-ws.log`);
  after the export → **rc 0, 0 warnings** (`check-ws2.log`). `build-lock mbx check -p desktop` → rc 0 (only the
  pre-existing `block v0.1.6` future-incompat note; `check-desktop.log`) — the Tauri build script and mbx coexisted here.
- Bindings: `BUILD_SLOTS=1 build-lock mbx test -p rpc --features ts --lib --quiet export_bindings` → 8 passed
  (`bindings.log`), 808 files copied with the justfile's three lines. Drift against the hand-written stubs: none in
  shape; `AskBranch.notebooks` / `AskSummary.scope` are `Array<string>` (the stubs said `Array<DocId>`, the same
  type) — no tsc change. `ChildResumed` (146) arrived with the fold's export.
- UI after the real bindings: tsc → 0 errors (`tsc4.log`); full vitest (`BUILD_SLOTS=1`) → **231 files / 1523 tests
  green** (`vitest2.log`; `api.test.ts` pins `METHODS` to the regenerated `methods.json`); vite build → rc 0
  (`vite-build2.log`). `t` built with `build-lock mbx build -p theuth` (`build-t.log`, then `build-t2.log`/`build-t3.log`
  after the two live-proof fixes).
- **Final round after the third fold (5924e511, incl. 147-F1's `side_call.rs`)**: `cargo fmt --check` clean (`fmt2.log`);
  `build-lock mbx test … --no-run` 0 warnings (`test-norun4.log`); bare `mbx test` same `-p` set, `--lib --tests
  --no-fail-fast -- --skip decide_many_releases_the_checkpoint_it_loaded` → **19 binaries, 881 passed, 0 failed**
  (`test-run2.log`; incl. the two live-proof fixes' tests); clippy clean (`clippy2.log`); `BUILD_SLOTS=1 mbx check
  --workspace --all-targets --exclude desktop` rc 0 (`check-ws3.log`); `mbx check -p desktop` rc 0 (`check-desktop2.log`).
  The fold changed 140's `StudyDetail`/`AutoresearchGateConfig` → bindings re-exported (`bindings2.log`, 808 files) →
  tsc **0 errors** (`tsc7.log`; `tsc6.log` shows the 6 pre-regen errors), full vitest **231 files / 1526 tests green**
  (`vitest3.log`) + `src/rpc` + `src/components/autoresearch` re-run after the regen 4/30 (`vitest4.log`), vite build
  rc 0 (`vite-build3.log`).

## Live proof

Scratch engine `target/debug/t --headless --data-dir /tmp/144-engine-w2` (`rpc 127.0.0.1:7146`, `anthropic/claude-sonnet-4-5`
through `ANTHROPIC_API_KEY`, `link.enabled`, `tools.policy.mutating: allow`, `voice.model_dir: ~/.theuth/models/pocket-tts`
(read-only; the one `peter_yearsley` voice), `secrets.key_store: file` — see Deviations), the gte checkpoint **symlinked**
from `/tmp/144-engine/models/`, the helper built from this tree (`THEUTH_DOCUMENTS_HELPER`), UI via bare `bunx vite --port
5246` with `VITE_THEUTH_URL/TOKEN`, Chromium through the harness (`browser`, raw Puppeteer for the pop-out target).
Data: notebook **DR failover** (`agqr5trtev5…`: Drill notes, Failover runbook, + Rollback rehearsal added from the pop-out,
+ Post-drill retrospective) and **Vendor review** (`agqr5trty5…`: Vendor A — Sentinel, Vendor B — Beacon), all pasted text,
each `completed` with `insights: 1` within ~10 s of ingest. Screenshots `/tmp/144/w2/z2-shots/` + `144-briefs/z2-shots/`.

- **a. Return to the notebook** — `a1-source-back-to-notebook.png`: `/sources/<id>?nb=` at 1400 px shows
  `Back to DR failover` (`data-testid="notebook-back"`, href `/notebooks/<nb>?tab=sources`) at the left of the row; click
  → the notebook page with the Sources column. `a2-note-back-to-notebook-desktop.png`: the saved note opened from the
  notes column (`/notes/<id>?nb=`) → `Back to DR failover` at x=268,y=57 (visible, not `md:hidden`); click →
  `/notebooks/<nb>?tab=notes` with the Notes column (`a3-…png`). `a4-note-from-library-chip.png`: the same note without
  `?nb` → back control for its first notebook + `in 1 notebook ▾` chip listing `DR failover → /notebooks/<nb>?tab=notes`.
  `a5-phone-note-back.png` / `a6-phone-lands-on-notes-tab.png`: 390 px → back → the notebook's **Notes · 1** tab selected.
- **b. Pop-out** — `b1-popout-window.png`: header **Pop out** → Puppeteer `targetcreated`: a new page target
  `/notebooks/<id>?window=popout-notebook-<id>`, `window.name = popout-notebook-<id>`, 1100 × 720, three columns, no
  sidebar/band, no Pop out button inside. Working inside it: `b2-popout-source-added.png` (Add → Paste → `Rollback
  rehearsal` card), `b3-popout-chat-turn.png` + `b4-popout-reload-keeps-notebook.png` (a turn answered with chip `(1)` →
  References row `Failover runbook #b034419c…`; `reload` keeps `/notebooks/<id>?window=…`, same name, three columns),
  `g1-popout-chip-pulses-source-block.png` (chip click inside the pop-out → `/sources/<id>?nb=…#b034419c…`, exactly one
  `.theuth-cite-pulse` on "The ICD portal fails over … 3 min 40 s"; no back control in the pop-out, 143's rule).
  `b5-library-row-menu-pop-out.png`: Library row menu = Rename · Archive · **Pop out** · Delete. Tauri path: `openPopout`
  → `invoke('open_window', { label: popout-notebook-<id>, url })`, the same helper 143 R1's note pop-out uses; sized by
  label in `commands.rs` (1100 × 720) — compiled, not driven (no Tauri shell in this proof).
- **c. Dial + context line** — `c1-dial-off-full-context-line.png`: Drill notes → `off`, Failover runbook → `full` →
  `in context: 1 source · ~577 of 40k tokens` + `1 omitted` chip, the off card dashed with an `omitted` pill and the
  pinned dot + `×`. `c2-all-off-nothing-in-context.png`: both off → `nothing in context — the agent sees only the
  description` + `2 omitted`, before any send. Then a turn (`c3-turn-answer.png`); the frame
  (`/tmp/144/w2/z2/frames/c3-notebook-block-frame.json`, `messages.seq=1` of session `01a11ece…`): `<notebook id=… >` with
  the description, ONE `<item kind="source" id="xh2rpgr…" level="full" status="available">` carrying the whole annotated
  runbook incl. its `## Insights`, no item for the `off` source, `You may cite only these ids: xh2rpgr3gbjwrdbzr5mv7km2ma`;
  `session_context` rows `(01a11ece…, 5xahb6…, source, off)`, `(…, xh2rpgr…, source, full)`.
- **d. Default insight** — `d1-insights-tab-dense-summary.png`: `Post-drill retrospective` → Insights tab `Insights · 1`,
  `Dense summary · anthropic/claude-sonnet-4-5 · 2026-10-09` with its body (after the fix below). `d2-run-menu.png`:
  Run ▾ = Key insights · Table of contents · Open questions; `d3-key-insights-appended.png`: `Running…` → `Insights · 2`
  with the `Key insights` section. `d4-promote-toast.png` / `d5-promoted-note-in-notes-column.png`: Promote to note →
  toast `Promoted to a note · Open` → Notes column row `Key insights — Post-drill retrospective · AI from source · 1
  citations kept`; the note's last line is `*From* *[Post-drill retrospective](theuth://sources/jc4mn5…#bee0d463…)** ·
  anthropic/claude-sonnet-4-5 · 2026-10-09*`.
- **e. Ask across notebooks** — `e1-ask-dialog.png` (scope: DR failover `this notebook` + Vendor review, save into
  Vendor review); `e2-ask-plan-steps-branches.png`: `/ask/01a11edc…` with the plan as 4 numbered steps (notebook titles +
  queries), `Branch 1 · … 10 hits` with chips 1–10 into Failover runbook / Drill notes / the saved note, partial answers
  streaming; `e3-ask-synthesis.png`: the synthesis with 53 chips, 12 into DR failover documents and 40 into Vendor
  review documents; `e4-ask-after-reload-transcript.png`: header `2 notebooks · 1m · done`, the question + answer in the
  session's transcript. `notebook_ask_list` → `done: true`; Vendor review's `notebook_items_list` gained
  `note agqr5xdnhb…` (the "Save as note into" target). Readability fix below.
  `e5-library-ask-tab.png`: Library → **Ask** sub-tab lists the ask (`2 notebooks · 40m · done`) with the **Ask across
  notebooks** button; `e6-ask-synthesis-after-prompt-fix.png`: a second ask started there (`/ask/01a11f01…`) after
  prompt fix 2 — partials and synthesis carry `[§]` chips after intact sentences ("…exposed that PagerDuty was not
  configured to alert on DNS probe failures.¹"), `done: true`, the answer is the session's assistant turn.
- **f. Overviews** — `f1-overviews-menu.png` (Briefing · FAQ · Timeline · Mind map · Audio overview);
  `f2-briefing-v1-ready.png`: `briefing.md · v1 · ready · Open` (`/artifacts/toolu_01Rgno…`), the artifact filed as a
  notebook item (`added_by agent:01a11ee4…`, the `Overview · Briefing` session); `f3-briefing-v2-running-faq-running.png`:
  Regenerate → `v2 · running…` with `▾ versions · 1`, FAQ `v1 · running…`, Audio `overview.mp3 · v1 · running…`.
  `f4-briefing-v2-faq-v1-audio-running.png`: `briefing.md · v2 · ready` (`▾ versions · 1` holds v1), `faq.md · v1 ·
  ready`. `f5-faq-artifact-rendered.png`: the FAQ artifact renders as design 59's `Accordion` (questions open to cited
  answers — the fence bodies show the chip links as raw markdown, Hand-offs). `f6-briefing-v2-artifact.png`: the briefing
  artifact (`v2` picker in the header) with 16 numbered chips after fix 3 (was 16 × `[blocked]`);
  `f7-briefing-chip-opens-source-pulse.png`: a briefing chip → `/sources/xh2rpgr…#b034419c…` at the cited block.
  **Audio** — `f8-audio-overview-row-player-note.png`: `overview.mp3 · v1 · ready` with `<audio controls>` over the blob
  (muted `play()` in the proof: `currentTime 2.27 s` after 2.5 s, `duration 316.368`, `readyState 4`, `paused: false`),
  the note **`one voice available — both hosts read by peter_yearsley`**, and the `overview-script.md` row (`Open`) of
  the same version beneath. The artifact: `get_artifact("nb-01a11efc…")` → `kind: file`, `mime: audio/mpeg`,
  **1,898,208 bytes**, title `Audio overview · 6 min`; `ffprobe`: `codec mp3, sample_rate 24000, channels 1, duration
  316.368 s, bit_rate 48000`; `afinfo`: `1 ch, 24000 Hz, .mp3, estimated duration 316.368 sec`. The script turn ran as a
  real turn in the `Overview · Audio` session (`show overview-script.md (v1 · 50 lines · 8 KB)` → "The script is
  shown."), TTS on `CPUExecutionProvider` took ~20 min for 1,300 words while the machine compiled.
  Timeline / Mind map: not run (the kinds share the briefing/FAQ path — one `show` per kind; time went to the audio).
- **g. Wave 1 not regressed** — `g1-popout-chip-pulses-source-block.png` (chip → `.theuth-cite-pulse` on the cited
  block, 1 element at 1.5 s); `g2-save-answer-as-note.png` + `g3-saved-note-chips-provenance.png`: **Save answer as
  note** → toast `Saved as a note · Open`, Notes column `Last Drill RTO Measured at 3 Minutes 40 Seconds · AI from chat ·
  2 citations kept`, the note carries `theuth://sources/xh2rpgr…#b034419c…?s=17ff2de7` and `theuth://sessions/01a11ece…#2`
  with the `*Answer from* *[chat](…)** · model · date*` line (the title came from the live side call this time);
  `g4-share-guest-page.png`: `doc_link_create(note, editor)` → `https://relay.theuth.io/d#1.…`, a second tab entered
  `z2-guest` and saw the note live (`You are editing as z2-guest · shared by 144-w2-scratch`, 1 editor, the chip and the
  provenance line rendered).

## Deviations

**Turn 1 (UI phase)**
- Nothing to fold from `origin/main` in turn 1 (dd19bf63 was already an ancestor).
- `mobile-zoom.test.ts`'s two-line heuristic was satisfied with a comment line rather than `// mobile-zoom: allow` (the
  opt-out is a line-end `//` comment, impossible inside JSX attributes) or restyling the label (`md:text-sm` would change
  the phone's label size for a false positive).

**Turn 2 (engine phase + live proof)**
- Three folds of `origin/main` (bbf68677 → d5a8666e → 5924e511: 146/146b, 148, 140 follow-ups): all clean; migration
  0045 stayed unique; `slash.ts` kept `/ask` beside 148; `nav.ts`/`SectionBand.test.ts` are 140's as landed; E2 does not
  construct `SideReply`, so 147-F1's `side_call.rs` change (in the third fold) merged without conflict.
- **`CallKind::Insight` not added** (E1 hand-off): `runtime/src/side_call.rs` is 147-F1's tonight; insight side calls stay
  tagged `CallKind::Title`. Hand-offs.
- **`mbx test` ran `--lib --tests` for all five crates** (one invocation, the same `-p` set as the `--no-run`, per the mbx
  owner's 01:36 note) rather than `decision --lib` alone; `decision tests/decide.rs::decide_many_releases_the_checkpoint_
  it_loaded` is the one red that produces (memory accounting, pre-existing) and is `--skip`ped in the final round.
- **`mbx check -p desktop` worked** in this worktree (rc 0), so the `--exclude desktop` workspace check and the desktop
  check together cover every crate.
- **Dial before the first send** (R1 item 4) is implemented as a thread-creating hook on the dial (`useEnsureNotebookSession`)
  rather than a `Composer` pre-send hook: the composer re-keys onto the remembered thread, so no `Composer` change was
  needed. The dial-created thread is renamed to the notebook title (the composer's behaviour) and receives every remembered
  default of the device, not only the touched item.
- **`provenanceOf` replaces `isProvenanceLine` as the primary export** (the latter kept as `!== null`): the Notes column
  now says `AI from source` for a promoted insight instead of mislabelling it `from chat`.
- **Live-proof fix 1 (engine, `bc0f2b16`)** — Sonnet's default insight opened with `# Summary` / `# Key points`; both
  E1's `section_of` and U1's `insightSections` end a `###` section at the next `#`–`###`, so the Insights tab showed the
  heading with an empty body and a promoted note would have been empty. `insights::append` now demotes any `#`/`##`/`###`
  line in the model's text to `####` (code fences skipped); unit test `model_headings_inside_an_insight_are_demoted_below_
  the_section`. Existing insights written before the fix keep their headings (the scratch engine's first four sources).
- **Live-proof fix 2 (engine, prompts)** — the branch/synthesis answers wrapped whole sentences as link text; the chat
  renderer shows a `theuth://` link as a numbered chip and drops its text, so the synthesis read "The recommended vendor is
  1." `BRANCH_SYSTEM`/`SYNTH_SYSTEM` now spell out `[§](<href>)` after the full stop. Prompt-only; the first ask in this
  proof (`e3`) predates it.
- **Live-proof fix 3 (UI, `Viewer.tsx`)** — a markdown artifact rendered every `theuth://` chip as streamdown's
  `[blocked]` (`Blocked URL: undefined`); the overviews are chip-dense by design (§10), so the viewer's `Markdown` now uses
  the chat's `rehypePlugins` + `markdownComponents` inside a `CitationsProvider` with `References`, and its click capture
  leaves `theuth://` to `CitationLink`. The FAQ's `Accordion` fence bodies still show raw `[text](theuth://…)` — the OpenUI
  fence renders its own text (design 59), outside this wave; Hand-offs.
- **Scratch engine vault** — the rebuilt `t` (different binary hash) blocked in `security_framework::find_generic_password`
  on the keychain item the first build had created (a GUI prompt nobody can answer in headless); the scratch config gained
  `secrets.key_store: file` and the empty vault was moved aside (`/tmp/144-engine-w2/keychain-vault-backup/`). Sky's live
  engine is untouched. Worth a line in the proof recipe for the next closer.
- **Vite's watcher missed an edit** (`Viewer.tsx` served stale for the bare URL while `?t=` served the new module); the
  dev server was restarted (`--host 127.0.0.1`). Not a code change.
- The gte checkpoint is a **symlink** into `/tmp/144-engine/models/` (disk), the voice checkpoint is read in place from
  `~/.theuth/models/pocket-tts` via `voice.model_dir` (read-only; nothing copied).
- The live transcript showed the tool card of a turn but not its final assistant text until a reload (both notebook
  turns, both windows); the store had the text at once. Not investigated further tonight — pre-existing or main's
  (`135b` transcript rework is in the fold); Hand-offs with the symptom.

## Hand-offs

- **runtime (`side_call.rs`, after 147-F1 lands)**: `CallKind::Insight` (`"insight"`) so insight cost rows stop reading
  `title`; then `agent::notebook_context::insight_side_call` sets `kind: CallKind::Insight`, and E2's ask calls could take
  an `Ask` kind the same way.
- **Artifacts viewer / OpenUI fences (design 59)**: `Accordion`/`Timeline`/`Tree` fence bodies render their markdown
  without the chat's citation components, so an FAQ/Timeline/Mind-map overview shows `[text](theuth://…)` literally inside
  the fence (`f5-faq-artifact-rendered.png`). The plain-markdown parts of those artifacts render chips after fix 3.
- **Transcript (chat UI)**: in the notebook chat (shell and pop-out) a turn's final assistant text did not appear live
  (the tool card did); a reload shows it. Store had it immediately (`messages.seq=6`/`9` of `01a11ece…`). Symptom only;
  reproduce on `/c/<session>` to tell notebook-column from transcript.
- **E2**: the ask session's `Partial`/`Answer` events can carry sentences wrapped as link text from older runs; the prompt
  fix covers new runs only. A `filter_chips`-adjacent normaliser (`[long text](href)` → `text [§](href)`) would make the
  renderer robust to the model regardless of prompt compliance.
- **Orchestrator / next closer**: the proof recipe needs `secrets.key_store: file` in the scratch config when `t` is
  rebuilt mid-proof (keychain prompt on a changed binary), and vite started with `--host 127.0.0.1`.

## Status after wave 2

- `integrate/144-w2` = base 6ab1905f + U1 + U2 + E1 + E2 + `origin/main` 5924e511, every gate green at the final sha;
  nothing pushed. Commits on top of the merges: UI-phase fixes (`0393640d`), hand-offs (`870b5497`, `3e22a9a5`), insight
  heading demotion (`bc0f2b16`), ask chip-grammar prompts, artifact-viewer chips, three folds, this as-built.
- Proven live: a–g except Timeline / Mind map overviews (FAQ stood in as the second kind) and the Tauri window itself
  (compiled only; the pop-out proof ran in Chromium through the same `openPopout` helper).
- Left for inspection: `/tmp/144-engine-w2` (sessions, documents, `notebooks/{asks,overviews}.json`, `artifacts/*.mp3`),
  logs in `/tmp/144/w2/z2/`, screenshots in `/tmp/144/w2/z2-shots/` and `144-briefs/z2-shots/`. The scratch engine, vite
  and Chromium are stopped; the gte checkpoint was a symlink, no model copied.
- Open items are under Hand-offs (`CallKind::Insight`, OpenUI fence chips, live transcript text, chip normaliser).

