# 218 — as built: integration

Integration builder, 2026-11-25 night. Branch `feat/218-whiteboard`, worktree `theuth-218-whiteboard`.

## Base

- **Merged, not rebased.** Step 0 waited 20 min for a quiet tree; B2's `crates/agent/src/whiteboards.rs` stayed
  modified (uncommitted) the whole time (B2 committed it later as `a751a752b`). `git rebase` refuses a dirty tree and
  stash is forbidden, so main was merged into the branch instead; main never touched that file.
- `bba8429ee` — merge of main `81f043d36`. **No conflicts** (git auto-merged `Composer.tsx`, `agent/src/{lib,loop_,store}.rs`).
- `ffec9c51c` — merge of main `0c8a728c4` (tool categories, announced by the orchestrator mid-run). No conflicts.
- `crates/theuth/tests/fixtures/wire/openrpc-v1.json`: main's copy taken at both merges, then regenerated with
  `just wire-schema-accept` (`3c1bc0fc7`).
- `Composer.tsx` after the merge: main's uploads (`useUploads`/`uploadWait`/`seedFile`/`SendButton`, `uploading`
  guard) **and** B4's `useNoteRecorder` extraction are both present; main's upload commit never touched the recorder
  lines, so the extraction stays behaviour-identical.
- Migration: main ends at `0076_session_title_guard.sql`; ours stays `0077_session_whiteboards.sql` (no renumber).

## Wire step

- `just wire-schema-accept` → exit 0; `just ts-desktop` → exit 0 (671 methods, 76 topics).
- `components/whiteboard/wire.ts`: hand types replaced by re-exports of the generated ones (`WhiteboardCreated`,
  `WbBlob`, `WbBlobData`, `WbSkipped`, `WbTurnInfo`, `WbUndoResult` from `method.document.output`;
  `WhiteboardSession` from `method.agent.output`; `WbSendContext`, `WbSentImage` from `method.agent.input`;
  `WhiteboardViewOpened` from the event binding). `wbRpc` kept; every method is now typed by `MethodMap`
  (`WB_METHODS … satisfies readonly (keyof MethodMap)[]`, `WHITEBOARD_KIND … satisfies DocKind`).
- Casts removed in `App.tsx` (`on(WHITEBOARD_VIEW_OPENED, …)` uncast), `requirements.ts`, `useWhiteboards.ts`,
  `tasks/NewTaskSheet.tsx`, `tasks/TaskDetail.tsx`. `grep -rn 218-TEMP crates/desktop/ui/src` → empty. No type drift
  surfaced (`bun run check` exit 0).
- Two UI tests the generated bindings exposed: `tiles/popout.test.ts` had no sample for the new `whiteboard` tile kind
  (`7a0d2c350`); `nav/SectionBand.test.ts` counted 7 Library tabs — the Whiteboards tab is gated on the 218 methods, so
  it appears only once the generated `MethodMap` carries them; now 8 (`9f4a1f3fe`).

## Gate

Each cargo command took `/tmp/theuth-build.lock` on its own (45 s pause between), script `/tmp/wb218-gate.sh`, log
`/tmp/theuth-218-gate.log`, per-command output `/tmp/wb218-gate-N.log`.

| Command | Exit | Note |
|---|---|---|
| `cargo test -p document -- --test-threads=4` | 0 | lib 136 passed; `tests/whiteboard.rs` 12 passed incl. `reads_the_yjs_fixture` (no longer ignored) |
| `cargo test -p agent --lib -- --test-threads=4` | PENDING | |
| `cargo test -p artifact` | PENDING | |
| `cargo test -p rpc-compose` | PENDING | |
| `cargo test -p theuth --test wire_schema_snapshot` | PENDING | |
| `cargo test -p tauri-plugin-ink` | PENDING | |
| `cargo clippy -p document -p agent -p artifact -p tauri-plugin-ink -p desktop --all-targets --no-deps -- -D warnings` | PENDING | |
| `cargo check --workspace --all-targets` | PENDING | |
| `bun run check` (crates/desktop/ui) | 0 | after the wire step |
| `bun run test` (crates/desktop/ui) | 1 | 3526 passed, 1 failed: `disclosure.test.ts` ratchet, red on main (below) |
| `grep -rn 218-TEMP crates/desktop/ui/src` | empty | |
| `cargo tauri ios build --debug --target aarch64-sim …` | 0 | see iPad simulator |

## Pre-existing reds (red on main too)

- UI: `src/components/disclosure.test.ts` › *ratchet: no transcript disclosure keeps its open state…* reports
  `chat/CommunicationNotice.tsx:useState:expanded`. Neither the test nor `CommunicationNotice.tsx` differs from main
  (`git diff main --stat` empty for both), so it is red on main; not touched.

## iPad simulator

- Build: `cargo tauri ios build --debug --target aarch64-sim --config tauri.ios.conf.json` (after `rm -rf
  gen/apple/build`, under the lock) → **exit 0** (≈ 8 min, warm target). Bundle
  `crates/desktop/gen/apple/build/arm64-sim/theuth.app`.
- `nm -gU …/theuth.app/theuth | grep init_plugin_ink` → `_init_plugin_ink` and
  `tauri_plugin_ink::mobile::init_plugin_ink` both present: the Swift package built and linked.
- Installed over the old build on `glass-198-ipad` (574E7C28…, not erased, app not uninstalled), launched
  `io.theuth.ios`: the app starts and is connected to the paired engine **333** (green dot, live sessions listed).
- **Where it stopped.** The paired engine 333 is the owner's installed `/Applications/theuth.app` (main build, no 218
  RPCs). The client gates on engine discovery, so the Library band has no Whiteboards tab and the sidebar has no
  Whiteboards row (`lib/nav.ts` `required: whiteboardRequirements`); the app has no deep-link route either. No
  whiteboard screen can be reached on the simulator until an engine built from this branch serves the iPad. Replacing
  the owner's engine or re-pairing was out of bounds tonight. Hence **not taken**: empty whiteboard light/dark, pill
  with the model picker, the cluster; and no overlay `attach` result, no `wb_capture` from the app.
- Screenshots (`/tmp/wb218-shots/`, rotated to landscape as the simulator stands):
  - `00-launch.png` — new-chat screen after install, light, engine 333 connected.
  - `01-library-light.png` — Library band: Artifacts, Knowledge, Skills, Notes, Notebooks, Meetings, Secrets — no
    Whiteboards tab (the engine gate above).
  - `02-library-dark.png` — same, system appearance dark: the app follows it; sidebar, band and list read well.
    One defect seen, not 218's: a bone-coloured strip below the status bar at the bottom of the window in dark mode
    (native window background); not checked against a main build.
- `crates/desktop/gen/apple/desktop_iOS/Info.plist` rewritten by the build: left unstaged, not committed.

## Unverified / open

- Every 218 surface on iOS (above), and everything in design §6.2 that needs a real iPad and Pencil.
- Uncommitted edits by B1 (`crates/document/src/wb/{engine,outline,pack,place}.rs`, clippy-style) were in the tree when
  the wire export and the iOS build compiled; B1 committed them before the cargo gate ran.
- Review findings m1–m12 of `218-review-build.md` were not in scope here and stay as listed there.
