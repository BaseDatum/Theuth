# 164 as built — Z (integration + live Telegram proof)

Branch `integrate/164` = `feat/164-eng` (`a6c8b263`) + `feat/164-ui` (`2c82890f`) + `main` (`f4ae771c`, 163 GRACEFUL-QUIT), worktree `theuth-164-eng`. Evidence: `/tmp/164-z/evidence/`, copied to `~/.theuth/briefs/2026-11-03/163-164/z-evidence/` (with the driver `z.ts`, `setup.sh`, `mock.patch`).

## A. Integration

| commit | what |
|---|---|
| `5d98abfc` | `Merge feat/164-ui into integrate/164` — clean |
| `d12e7f10` | `Merge main into integrate/164` — clean; **no conflict in `crates/desktop/ui/src/App.tsx`** (git auto-merged: the hunks did not overlap). Both intents checked in the result: 163's `QuitOverlay`/`useQuit` import (`:10`), `quit()` (`:93`), `onQuitRequested`/`theuth:quit` wiring, `<QuitOverlay />` (`:730`); 164-UI's `installFenceSnapshot` import (`:24`) and `useEffect(() => (AUX ? undefined : installFenceSnapshot()), [])` next to `installPagePreview` (`:647-648`). |
| `d1b1d3bb` | contract amendment in `164-channel-pictures.md`: `SnapshotId` → `FenceSnapshotId` in the Rust block, the TS block (file name, type, both imports), the RPC signature and the `api.ts` line, plus one amendment note (collision with `crates/sandbox/src/vm/mod.rs:250` `SnapshotId`, which already emits `bindings/SnapshotId.ts`). |
| `ee697d5f` | `scripts/telegram-live-proof/mock.patch`: the ledger recorded a multipart file only as `"<file>"`; it now records `{name, content_type, size, base64}` per file part (`internal/api/ledger.go`, +`encoding/base64`). Needed to verify the uploaded PNG; minimal, said here as the brief asks. |
| `f52fa8e3` | product fix (below, B "Bug found") |

### Bindings
`BUILD_SLOTS=1 BUILD_LABEL=164-Z build-lock mbx test -p rpc --features ts --lib --quiet export_bindings` → `8 passed`; copied into `crates/desktop/ui/src/rpc/bindings/` (gitignored); `bun install --frozen-lockfile` in `crates/desktop/ui` (640 packages) and `packages/apps` (484) → exit 0.

Binding diff (`/tmp/164-z/bindiff.ts`, comments and whitespace stripped, generated file vs the amended contract block): `FenceSnapshotId.ts`, `FenceLang.ts`, `FenceSnapshotRequested.ts`, `FenceSnapshotResolved.ts`, `FenceSnapshotAnswer.ts` all **SAME**; `bindings/methods.json` contains `answer_fence_snapshot`. No shape difference, nothing to fix. (`evidence/binding-diff.txt`)

### Checks
| command | result |
|---|---|
| `BUILD_LABEL=164-Z nice -n 10 build-lock bunx tsc -b --noEmit` (crates/desktop/ui, real bindings) | exit 0, **0 errors** |
| `… bunx vitest run src/rpc/api.test.ts src/lib/fence-snapshot.test.ts src/lib/quit-progress.test.ts src/lib/no-emoji.test.ts src/components/chat/surface/diagrams/fixtures.test.ts` | **5 files, 108 tests passed** |
| `cargo metadata --locked --format-version 1 > /dev/null` | exit 0 |
| `BUILD_LABEL=164-Z build-lock mbx build -p theuth` | exit 0 (rebuilt after the fix) |
| after the fix: `build-lock mbx test -p telegram --lib --no-run`, then `mbx test -p telegram --lib` bare (NO_COLOR unset) | **140 passed**, 0 failed |
| `build-lock mbx clippy -p telegram --all-targets --no-deps` | 0 warnings in telegram (only main's pre-existing `link` `fetch_update` deprecation) |

The workers' own Rust suites were not re-run (brief).

## B. Live proof (scratch engine + telegram-mock-ai + web-client renderer)

Setup: mock `skrashevich/telegram-mock-ai@314c594` + `mock.patch` (Bot API 127.0.0.1:8081, Admin 127.0.0.1:8082, LLM/proactive/welcome off) built with `/opt/homebrew/bin/go`; engine `target/debug/t --headless --data-dir /tmp/164-z/data-<n>` with the brief's `env -u … THEUTH_RPC__LISTEN=127.0.0.1:0 …`, setsid-detached; minimal block config (`agent.model: anthropic/claude-haiku-4-5`, `secrets.key_store: file`, `tools.approvals: false`, mesh off, the 18 listed plugins `{enabled: false}`, `telegram: {enabled: true, api_host: "http://127.0.0.1:8081", owner: 777000111, pictures: true}` — these are the exact keys in `crates/telegram/src/lib.rs` `TelegramConfig`); vault seeded via `credentials.yaml` from `ANTHROPIC_API_KEY` (never printed); bot token via RPC `set_secret("telegram_bot", …)` → `getMe` + `setMyCommands` seen. Renderer: `bunx vite --port 1464 --strictPort` in `crates/desktop/ui` with `VITE_THEUTH_URL`/`VITE_THEUTH_TOKEN`, opened in Playwright's headless Chromium (`chromium_headless_shell-1226`, driven by `playwright-core` 1.64.1 under `/tmp/164-z/driver`), page kept open; its WebSocket frames (`subscribe_events`, `FenceSnapshotRequested`, `answer_fence_snapshot`) are logged to `page-frames-*.txt`.

### Final table (run 3, engine at `f52fa8e3`)

| step | status | ms | evidence |
|---|---|---:|---|
| Z1 connect | PASS | 2118 | `telegram_status` `Connected` @theuth_proof_bot, polling; page `subscribe_events` answered after 1828 ms |
| Z2 Graph (6 nodes) | PASS | 5656 | `sendRichMessage#175` (line 1, no body) → `sendPhoto#176` `z2-pipeline.png` 53 039 B PNG 596×912, caption "Z2 pipeline" → `sendRichMessage#179` (last line); no leak in any of the turn's 12 calls |
| Z3 mermaid flowchart | PASS | 5249 | `#188` → `sendPhoto#189` `diagram.png` 50 309 B 832×1020 → `#191`; no leak |
| Z4 Table + StateMachine | PASS | 6079 | `#199` → `sendPhoto#200` (Table, 1120×326) → `#202` → `sendPhoto#203` `task-lifecycle.png` 630×464 → `#206`; no leak |
| Z5a page closed | PASS | 5651 | no photo; D7 box in place (`Z5a-text.txt`); last draft → fallback message **4 ms**; engine `outcome=NoRenderer` |
| Z5b silent subscriber | PASS | 6663 | no photo; D7 box; last draft → fallback message **3006 ms**; the subscriber received exactly 1 `FenceSnapshotRequested` |
| Z5c again, 1.5 s later | PASS | 4658 | no photo; D7 box; last draft → fallback message 275 ms (a draft tick, no 3 s wait); **0** new `FenceSnapshotRequested` → D3 back-off holds |
| Z6 `pictures: false` (config edit + restart, page open and subscribed) | PASS | 4042 | no photo; D7 box; the page received **0** `FenceSnapshotRequested` |
| Z7 `message_user` (pictures back on, restart) | PASS | 7891 | `sendRichMessage#279` "Delivered through message_user." → `sendPhoto#280` `z7-pipeline.png` 52 694 B 596×912 → `sendRichMessage#282`; tool card shows `text: <723 chars>`; then the reply "sent" |

"No leak" = no string in any Bot API call's params (drafts, sends, edits, captions; file bytes excluded) matches `"root"`, `"elements"`, ```` ```json-render|mermaid|openui ````, `-->` or `flowchart TD`. Every PNG checked for the PNG signature, `IHDR`, sane sides and byte count = multipart size; saved under `evidence/png/`. Per-step ledgers `ledger-Z*.json`, full ledger with file bytes `ledger-full.json`, chat store `messages.json`, engine logs `engine-logs/`, snapshot log lines `engine-snapshot-lines.txt`, results `results.jsonl` / `table.md`, page screenshots `page-*.png`.

Z1 note: the run's first Z1 sample read `Identity` (`results.jsonl` row 1, FAIL): `Connected` is set by the first answered `getUpdates`, and the mock long-polls, so it lags `set_secret` by up to one poll window. Z1 now polls `telegram_status` for up to 60 s; re-run → PASS (row 8). Harness timing, not a product fault.

### Bug found (run 1) and fixed — `f52fa8e3`
Run 1 (`evidence/run1/`): Z2 and Z4 FAILED on leaks. Haiku's thinking repeated the user's fence verbatim, and Telegram's reasoning quote (`bubble::Reasoning::quote`) and the card's thinking entry (`cards.rs` `Item::Thinking`) printed reasoning raw: `"root"`, `"elements"` and ```` ```json-render ```` appeared in drafts **and in the final message** (`run1/ledger-Z2.json` #14–#18). Fix: `Reasoning::shown()` — `surface::picture_preview` while the run streams (fence → `_drawing…_`, half-typed openers included), `surface::flatten(…, FALLBACK_COLUMNS)` once it ended (D7 text) — used by both render sites. Test `bubble::tests::reasoning_never_shows_a_picture_fence_body` (closed fence live, half-typed fence live, ended run). Exercised live in run 3 Z5c: draft `#237` shows `_drawing…_` in the quote, `#238/#239` the D7 box.

### Decisions (with reasons)
1. **Z5 split into Z5a/b/c.** ENG deviation 2: with no event subscriber the engine answers `NoRenderer` at once and starts no back-off, so closing the page alone cannot show the ≈ 3 s wait or D3. Z5a proves the closed-page path (no wait); Z5b adds a connected-but-silent `subscribe_events` socket (the D3 case: 3006 ms); Z5c proves the 60 s back-off (no request, no wait).
2. **D3 evidence from the wire, not the engine log.** The engine logs only `snapshot: fence sent as text … outcome=…`; neither requests nor the back-off are logged. The silent subscriber's received `FenceSnapshotRequested` count (1, then 0) and the draft-to-message gap are the proof. No log line added (not a bug).
3. **A different Graph per step** (title/first label `Z2`, `Z5a`, …): D5 caches PNGs by source, so repeating Z2's fence in Z5 would have been served from cache with no renderer.
4. **Vite as a managed service.** In run 2 a backgrounded vite died silently mid-run (page console `server connection lost`; the dynamic `import()` of `FenceSnapshot.tsx` then failed and every request answered `failed`, `run2-vite-died/`). Re-run under the tool's service supervisor; restarted for each engine restart because `THEUTH_RPC__LISTEN=127.0.0.1:0` changes the port (kept `:0` as the brief says).
5. **Fresh engine for the final run.** The run-2 retry started within 60 s of a Z5b back-off, so Z2–Z4 got `NoRenderer` with the page connected (`run2-backoff/`). That is D3 as designed (a renderer that connects during a back-off is used only after the window or its first answer); run 3 used a fresh engine.
6. **Leak patterns = the brief's criterion.** A first version also flagged the bare word `json-render`; the model's prose ("The json-render block…") is not a fence info string. Dropped.
7. **Table fixture without `title`.** Run 1's Table carried `title`, which Table does not accept: the model got a surface error round. Fixture bug, removed.
8. **One-off bare `bun`** for the 15-line binding-diff script and one JSON peek; every other bun/bunx/tsc/vitest ran under `build-lock` (the proof driver as one long command per phase). vite ran bare (brief).

### What the mock cannot vouch for
- Telegram's real rendering of `rich_message` markdown: the expandable quote, `<details>` cards, a D7 box (a code block) inside the reasoning quote, `_drawing…_` drafts. The mock stores, it does not render.
- Real `sendPhoto` behaviour: compression/resizing, the 10 MB / 10 000 px / 20:1 limits (the photo-vs-document switch is unit-tested only), caption rendering, flood limits on a photo between two messages.
- `sendRichMessageDraft` semantics on real clients (the mock returns `True`).
- Long-poll timing: `Connected` appeared only after the first poll answered.

## Residual risks and open items
- **Signal and Slack are proven by unit tests only** (ENG's tests a–g); no live run here.
- Slack reasoning leak: confirmed and fixed in R1 (below); proven by unit tests only, not live. Signal sends no reasoning text.
- **Real WKWebView hidden-window throttling**: the renderer was headless Chromium in a visible tab; Sky's app renders in a WKWebView that may be hidden/occluded (timer throttling; 164-UI notes one unreproduced Chrome-hidden stall).
- **Sky's real-bot test** on Telegram (and Slack/Signal) is still owed before calling 164 done for users.
- D3: a working renderer that connects during a 60 s back-off is not asked until the window ends (by design; seen in run 2).

## R1 — Slack reasoning never shows a picture fence body (`2804f8d7`)

Orchestrator R1: Slack showed model reasoning raw, the same class as the Telegram bug above. Every place in `crates/slack/src/bubble.rs` that shows reasoning now applies the rule of Telegram's `Reasoning::shown()`:

| where | before | now |
|---|---|---|
| stream-mode live card `ReasoningRun::chunk` (`reasoning-<n>` `output`; also re-emitted on rotation) | `self.text.raw()` | `reasoning_settled(raw)`: settled picture fences (closed, closing line broken) as their D7 text; nothing from the first unsettled one on (open, closing line unbroken, or a half-typed opener via the existing `half_opener`) |
| stream-mode ended card (`end_reasoning`) and popped trailing thought (`pop_thought`): `output` and the update-mode card text `Queued::Chunk { card }` (`Segment::quote`) | raw | `reasoning_text(text, false)` = `surface::flatten(text, FALLBACK_COLUMNS)` (D7) |
| their `details`, and the `💭` thought line inside a batch card (`Thought::line`, both modes) | `first_line(raw)` | `reasoning_line(text, live)`: first line of `picture_preview` while live (`_drawing…_`), first line of `strip_pictures` once ended |
| update-mode quoted reasoning message (`Segment::body`, also the stream→update demotion) | raw committed prefix | `picture_preview` while live, `flatten` once `text.is_done()`; `Segment::finish` forces the final `chat.update` when the last one showed a `_drawing…_` stand-in, even if no new bytes were committed (otherwise the placeholder would stay) |

Decisions:
1. **Stream card stays append-safe either way.** Design 110 records that a re-sent `task_update` id *replaces* the card in place (the code already relied on it: the card is retitled `thinking` → `Thought N s`). The brief's R1 asks for append-only behaviour; `reasoning_settled` is prefix-monotone — every live output is a prefix of the next and of the ended D7 output (`flatten` keeps prose byte-identical, a settled fence's D7 text never changes, raw only grows) — so nothing is retracted or resent under either semantics, and no byte of a fence body ever goes out.
2. **One-line summaries skip the D7 box.** `details` and the thought line are one line; the first line of a D7 box is its bare ```` ``` ```` opener, so once ended they take the first prose line (`strip_pictures`), else none. Still never a body. Small deviation from "D7 text once ended", made only for these one-line fields.
3. **Signal shows no reasoning:** `crates/signal/src` never handles `AgentEvent::ReasoningDelta`/`ReasoningDone`; the only hits are the `/reasoning` effort command strings in `commands.rs`. Nothing to change.

Tests (`bubble::tests`, `FENCED_THINKING` = prose + a json-render Graph fence + prose, streamed in 3-byte deltas with a tick after each):
- `stream_reasoning_never_shows_a_picture_fence_body`: every `reasoning-1` output extends the previous one; none contains `"root"`, `"elements"`, ```` ```j ````/```` ```m ````/```` ```o ```` or the body; one live output is exactly the prose before the fence; the last is `Complete`, holds the D7 box (`Alpha`) and ends with ```` ```\n\nDone. ````. The batch `💭` line: `_drawing…_` live, no leak ended.
- `update_reasoning_never_shows_a_picture_fence_body`: every `chat.update` of the quoted reasoning message has no leak, one shows `_drawing…_`, the last carries the D7 box and ends with `> Done.`.

Gates: `cargo fmt -p slack`; `BUILD_LABEL=164-Z build-lock mbx clippy -p slack --all-targets --no-deps -- -D warnings` → exit 0, 0 warnings; `BUILD_LABEL=164-Z build-lock mbx test -p slack --lib --no-run`, then `mbx test -p slack --lib` bare → **164 passed**, 0 failed (the listed flake did not fire). No live Slack run (no mock exists for Slack in this repo); logs `evidence/slack-test.log`, `evidence/slack-clippy.log`.
