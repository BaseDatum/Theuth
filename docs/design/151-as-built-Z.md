# 151 — as built, Z (closer: integrate, gate, live-prove)

Branch `integrate/151` (cut from `feat/151-c`), 2026-10-09. Design `docs/design/151-autoresearch-setup.md`; halves `151-as-built-ENG.md`, `151-as-built-UI.md`.

## 1. Integration

| merge | sha | result |
|---|---|---|
| `feat/151-eng` (tip `88c9869c`) | `b086af89` | clean |
| `feat/151-ui` (tip `12e9fa74`) | `e0c1e5d7` | clean |
| `origin/main` (`ece44049`, 150 PAGE-DRAFTS) | `2ca2c7b3` | clean |

TS bindings regenerated (`export_bindings` 8 passed, 816 files, `methods.json` 15 `autoresearch.*`) and copied to `crates/desktop/ui/src/rpc/bindings/`.

## 2. Gates on the merged tree (after the fixes in §4; stable 1.96.1, `BUILD_LABEL=Z151 build-lock mbx`, `CARGO_BUILD_JOBS=4`)

- `mbx test -p autoresearch` — **175 passed** (173 + 2 new), 0 failed.
- `mbx test -p app --lib` — **47 passed**.
- `mbx test -p rpc --lib` — **56 passed** (the known `second_acquire_in_one_process_is_running` flake did not fire, twice).
- `mbx clippy -p autoresearch -p app -p rpc --all-targets -- -D warnings` — clean (0 warnings, twice).
- `mbx check -p theuth` — green (97 s).
- `cargo fmt -p autoresearch -p app -p rpc --check` — clean.
- `bunx tsc -b --noEmit` — 0 errors (checked that a deliberate bad file gives 1).
- `bunx vitest run --maxWorkers=2` (FULL) — **234 files, 1539 passed** (1538 before the new `autoOpens` test); `pairing.test.ts` did not flake.
- `bunx vite build` — ✓ built (3.3 s).

## 3. Live proof (design §10, brief §3)

Scratch engine: branch `t` 2.14.0 (`/tmp/151/bin/t`, rebuilt as `t2` with the §4 engine fixes), data dir `/tmp/151/live` = `/tmp/140/live2/config.yaml` with `rpc.listen 127.0.0.1:17951`, mesh bind `127.0.0.1:18951`, `autoresearch: enabled`, `schedule` on, `anthropic/claude-haiku-4-5`, `secrets: {key_store: file}`. Repo `/tmp/151/repo-sort` = toy-sort fixture, `git init`, one commit `5a57d72`. UI: vite 5191 in this worktree, managed headless Chromium 1280×900 and 390×844 (CDP touch emulation, `maxTouchPoints 5`, `pointer: coarse`). Driver `/tmp/151/call.ts`, global event recorder `/tmp/151/events.jsonl`. Haiku followed the skill: it drafted early, re-drafted, probed, finalised; it also kept building its own OpenUI apps and twice called two probes at once (`asyncio.gather`) — see §4.

Three setup sessions plus one phone session were run (`sort-speed`, `toy-sort-2`, `toy-sort-memory`, `phone-test`).

| step | result | evidence |
|---|---|---|
| 1 Tab shows `+ New study`; empty state "Set one up with theuth" | **PASS** | body text contains both; screenshot `assets/151/01-intro-box.webp` (after the click) |
| 2 Pane → repo + goal → Start setup chat → session with cwd = repo, titled `New study: …`, seed marker sent with `use_skill` | **PASS** | `sessions` row `01a12131…: title "New study: make the sort faster", cwd /tmp/151/repo-sort`; chat shows "Skill autoresearch added" + `[autoresearch setup · repo /tmp/151/repo-sort]\nmake the sort faster`. Note: the engine's title model later renamed it ("Sort performance study setup") — H3 |
| 3 Answer in the embedded composer until ≥ 2 partial drafts; panel fills with engine states; chat shows compact lines | **PASS** | `sort-speed` v1 partial (2 of 9) and v5 partial (8 of 9); panel `data-state`s `goal:proposed repo:verified metric:proposed …`; chat text `Study draft sort-speed · 2 of 9 settled`. Later cards called from inside the agent's `python` tool render inside the collapsed "n tools" group (chat behaviour, not 151's) |
| 4 Agent writes `bench.sh` under the drafts dir, probes bench + check → Verified with evidence + sparkline; no `probe/` left; repo untouched | **PASS** | `/tmp/151/live/autoresearch/drafts/sort-speed/` = `bench.sh probe.json spec.json` (no `probe/`); rows `bench:verified` evidence `3 runs · 4738 ms · ±1.2%`, 1 `<polyline>`; `checks:verified` `passed in 69 ms`; `git status --porcelain` empty before and after; `git worktree list` has no probe worktree; manifest identical except `__pycache__/sorter.cpython-314.pyc` (gitignored) created by the agent's own hand-run `bash bench.sh` in its cwd per the skill, not by the probe. First probe run used the repo's `bench.sh`; the agent wrote the drafts-dir copy when asked. The agent's first two probe pairs failed (F1) |
| 5 OK on a Needs-your-OK row → Confirmed, no chat message | **PASS** | Scope → `data-state=confirmed`, `spec.json confirmed:["scope"]`, settled 4→5; no `InputInjected`/`TurnStarted` event after |
| 6 Edit Budget → new value Confirmed + ONE `[study draft …] you set …` message; agent acknowledges | **PASS** | row `$3 · 30 experiments …` Confirmed, `user_set:["budget"]`; exactly one `[study draft sort-speed] you set budget.usd to 3 in the panel`; reply "Got it — budget reduced to $3 …". Sticky rule seen live: the agent's next draft changed `experiments` 30→20 → Budget back to Needs your OK (`assets/151/06-…` shows 8 of 9) |
| 7 Start now → study starts; panel → status card; rail shows the study; study header `Setup chat ›` opens this chat; start card in the chat | **PASS / FAIL** | `sort-speed`: Start from a `ready` panel → study **failed at once** (custody: holdout script was an absolute path into the user's checkout) — F2, fixed. `toy-sort-2` (relative scripts, fixed binary): Start → `running · experiment 2 · novelty`, status card `1/10 · $0.02 of $2 · Pause · Stop · Open study`, rail row `toy-sort-2 running`, no "Setting up" row; study page header `Setup chat ›` → `/autoresearch/new?chat=01a1214a…`; study finished 10/10. **The start card never reached the chat**: `card not delivered: no user channel: slack, signal and telegram are not running` — H1 |
| 8 Second throwaway setup, agent drafts once, rail "Discard draft" → row gone, `drafts/<slug2>/` gone | **PASS** | `toy-sort-memory`: rail row removed, `autoresearch.drafts` → `[]`, `autoresearch.draft` → `no draft …`, `[study draft toy-sort-memory] discarded in the panel` delivered, agent answered one line |
| 9 Phone 390×844: full-screen pane, `Draft n of m` pill opens the panel | **PASS** | intro fills the screen with back `‹`; pill `Draft 7 of 9`; tap → bottom sheet `[data-slot=sheet-content]` 390×717 at y 127 (85 dvh) with the rows. The pill sits below the composer (UI as-built), partly under the assistant FAB — H4 |
| 10 Final draft's full card Discard deletes the draft (ENG addition A) | **PASS** | `app_call_tool {record toolu_01UHps5…, statement discard, trigger press}` → `draft `phone-test` discarded`; dir gone; a stale card Start press → `autoresearch_start: no draft `phone-test`` |

Screenshots (`docs/design/assets/151/`, ≤ 1024 px webp): `01-intro-box`, `02-panel-verified-and-needs-ok`, `03-budget-edit-message`, `04-after-start-status-card`, `05-study-header-setup-chat-link`, `06-redraft-no-sheet-rail-synced`, `07-phone-intro`, `08-phone-chat-pill`, `09-phone-panel-sheet`.

Spend: 205 model calls on the scratch engine (incl. the `toy-sort-2` study's 10 experiments), 4.92 M input (4.63 M cache read, 0.28 M cache write), 67.6 k output → ≈ **$1.2** at haiku-4.5 prices. Engine, vite and Chromium stopped; `/tmp/151` left (binaries 2.5 GB under `/tmp/151/bin`).

## 4. Fixes (each with a test that failed before)

| id | defect (live evidence) | fix | sha · test |
|---|---|---|---|
| F1 | Two `autoresearch_probe` calls in one agent turn (`asyncio.gather`) raced on `probe/wt`: `git worktree add … already exists` / `cannot create directory at 'fixtures'` — both failed, twice | `setup::probe` holds one process-wide `tokio::sync::Mutex` for its duration (ponytail: per-draft locks if concurrent setups matter) | `1c2bd76b` · `setup::tests::concurrent_probes_of_one_draft_both_succeed` (two spawned tasks; failed before with the live error) |
| F2 | A `ready` panel started `sort-speed` with `holdout.command = /tmp/151/repo-sort/holdout.sh`; the study failed immediately in custody (R-8: only repo-relative or drafts-dir scripts). The probe had even Verified `bench` on such a path | `setup::view_of` appends `custody::check_commands` failures to `blocking` (so `ready` is false with custody's words); `probe` refuses the same way before creating anything | `1c2bd76b` · `setup::tests::scripts_outside_custody_block_start_and_the_probe` |
| F3 | Rail row stayed `Setting up · 6 of 9 settled` while the panel said 3 of 9 after the agent's re-draft (`drafts` was only invalidated by edit/confirm) | `useDraft` invalidates both `draft` and `drafts` keys on a new draft/probe result | `1c2bd76b` · no vitest (effect in the node-only suite); proven live: rail and panel both `9 of 9` after a re-draft (`06-…webp`) |
| F4 | Every draft card (`study-drafts.openui`, an app artifact) auto-opened the chat's artifact sheet over the pane, hiding the panel on each re-draft | `autoOpens(artifactName, pathname)` in `setup.ts`; `ArtifactPanel`'s `ArtifactShown` path skips the `study-drafts` artifact under `/autoresearch/new` | `1c2bd76b` + `1733867b` (the first compared the bare app id; live showed the name is `study-drafts.openui`) · vitest `autoOpens` in `setup.test.ts`; proven live (no sheet after re-draft) |

## 5. Hand-offs (larger than a closer fix)

- **H1 — the start / failure card never reaches a desktop setup chat.** `actor::deliver` → `agent::UserChannels` has only Slack/Signal/Telegram sinks; log `card not delivered: no user channel` for both studies. D7's premise ("origin receives the start card") holds only for messaging sessions. Needs an engine route into a native session (a pending message / note on `origin`), outside `crates/autoresearch` ownership of a closer.
- **H2 — the agent (haiku) deviates from the skill**: final draft without `confirmed`, budget changed after the user set it (correctly dropped to Needs your OK), its own OpenUI apps each turn, `asyncio.gather` probes (now safe). Prompt-level; consider stating "one probe at a time; list every row the user agreed to in `confirmed`" in `skill.md`.
- **H3 — `rename_session` title is overwritten by the title model** ("Sort performance study setup" after turn 2; `Titled` only guards the first-turn placeholder, `title_pinned` is meeting-only). Same for Routines' "Describe it".
- **H4 — phone pill below the composer**, partly under the assistant FAB at 390 px; D2 says above the composer. UI layout, small, not blocking.
- **H5 — footer says "Everything is settled"** when `ready` while a default-able row (Budget) still reads Needs your OK (§5 excludes it from `ready`); wording only.
- Environment: a copied `t` needs `theuth-app-parser-<hash>` (and `theuth-documents-*`) beside it, else every `autoresearch_draft` fails with `apps are unavailable on this harness`; copied from `/Applications/theuth.app/Contents/MacOS/` (same hash `5e864f05…`). Add to the scratch-engine recipe. Vite's file watcher did not see edits in this worktree (restart to pick up changes), and vite died twice silently — run it under a restart loop (`/tmp/151/vite.sh`).

## 6. Deviations from the brief

- Step 4's "agent writes bench.sh under the drafts dir": the agent first reused the repo's `bench.sh` (skill allows it); the drafts-dir script came after one user request, and that probe ran (3 samples, Verified). The brief's single success path for step 7 needed a second setup after F2.
- Screenshots are the capture tool's 1024-px webp renders (original 1600×1125 / 488×1055), not separate 1600-px files.
- Step 10 exercised through `app_call_tool` (the press path), as the brief allows; the card inside the collapsed `python` tool group was awkward to click.
