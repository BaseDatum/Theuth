# 152 INPUT-CARDS — as built (2026-10-09, branch fix/152-input-cards)

## The ask (Sky, 2026-10-09 15:02, verbatim)

> found a couple more things in desktop/mobile that need UI for them.
> `<resume_handoff>The theuth engine restarted at 14:54:32 while you were working on: "…". Your session with this harness has been restored with its context; … Report the result when you are done.</resume_handoff>`
> and when an endless chat session actually hands it's subagent it's session another blob of text goes in like that that doesn't actually render any UI, it just shows the raw text kinda like this resume handoff thing.

Rule (Sky, 2026-09-26): "When anything from the harness goes into the session as a user message that gets a UI component not raw text in a message bubble." HarnessCards: "Nothing is hidden: every part stays readable, just folded."

## D5 sweep — every harness text that lands as a USER row

| wrapper / marker | producer | status |
|---|---|---|
| `<skill name>` + words | agent/src/skills.rs:246 | carded (`skill`) |
| `<message from id at …>` (peers, child results, bound replies) | agent/src/loop_.rs:7750, supervisor/child_waits.rs:105 | carded (`peer`) |
| `<job_result>`, `<job_idle>`, `<reminder>` | agent/src/supervisor.rs:3081, 3103, 3125 | carded |
| `<schedule name at quiet …>` (nested `<until>`/`<request>`/`<when>`) | schedule/src/lib.rs:1220 | carded; nested blocks show raw inside the body |
| `<rounds>` `<context_pressure>` `<todo>` `<knowledge>` `<recovery>` `<continue>` `<steer>` `<vision>` `<todo_continue>` | agent/src/loop_.rs:3613 `note()` and callers | carded (engine notice) |
| `<nudge>` `<journal>` `Wake: … <task> <roster>` | roster/src/desk.rs:44, 233 | carded |
| `<advisory severity guidance>` | agent/src/advisor/advise.rs:70 | carded (history row renders from metadata) |
| `<surface_error [final]>` `<notebook>` `<context_summary>` `<runtime>` | surface/src/prompt.rs:330, agent/src/notebooks.rs:594, compact.rs:629, acp_child.rs:405 | hidden (own surfaces) |
| `<vision_observation>` | agent/src/vision.rs:599 | rendered from `context.vision` metadata, never through classify |
| `[broadcast …]` `[meeting …]` `[spoken, N s]` `[attached …]` `[app … ]` `[screen …]` `[voice note …]` `[board …]` | supervisor.rs:6351, meeting/src/session.rs:51, talk/src/policy.rs:597, attach.rs:131, UI lifecycle | human words with a marker (`parseBroadcast` / `parseMeetingLine` / `parseSpoken` / `parseAppInput` / MessageUser parsers) |
| `<time now=…/>` | agent/src/clock.rs:51 | stripped by `render_with` / `splitTime` |
| `<resume_handoff>` restored / fresh | agent/src/durability/handoff.rs:82, 102 | **gap fixed in 152** (D1) — 3 rows in sessions.sqlite |
| `<child_lost name session at why>` | agent/src/supervisor/acp_resume.rs:342 | **gap fixed in 152** (D2) — 8 rows |
| `<chat>…</chat>` parent view before a native child's brief | agent/src/endless/fold.rs:281 via `OpenerContext` (supervisor.rs:4473, loop_.rs:7722) | **gap fixed in 152** (D3) — 0 stored rows yet (fixture from `fold::render`'s shape) |
| `<budget spent cap currency>` | agent/src/loop_.rs:2663 `budget_note`, persisted 5466 | **gap fixed in 152** — the one notice with attributes; `ENGINE_NOTICES.budget`, matched by name (injected.ts:249); 0 stored rows |
| `### Session update …` markdown into the advisor session | agent/src/advisor/render.rs:213 → supervisor/advisor.rs:341 | gap left: plain markdown with no stable wrapper; advisor sessions are not opened in the desktop transcript |
| `[file … unreadable: …]` extra Text part | agent/src/loop_.rs:7727 | left: words inside the user's own bubble, not a harness row |
| `<engine_restart …/>`, `append_local_note` text | loop_.rs:7839 (Tool role), store.rs:931 (System role) | not user rows |

`subagent_result` and `desk` remain in `CARD` for stored legacy rows; no Rust producer writes them any more.

## As built

- **D1** `src/lib/injected.ts:43` kind `resume`; `resume()` :147 — `RESTORED` / `FRESH` header regexes, `SECTIONS` :131 (line-start heads, bodies run to the next head, quoted bodies lose the closing quote unless cut), `OMITTED`, `FOOTER`. The header sentences, any unparsed lead and the footer go into a final `Note` section, so a malformed note is still a card and no character is lost. `classify` :250/:259 accepts `resume_handoff` as a whole-input wrapper among the others.
- **D2** `CARD.child_lost` :50; kind `lost` :45; attributes unescaped, body trimmed (:297).
- **D3** `splitParentView` :197 (exact shape only: `<chat>\n`, every non-empty line `^\d+\+\d+\|`, `</chat>`; sentinel `0+base|(earlier history…)` → `earlier: true`; `bytes` = UTF-8 bytes of the inner text; `rest` = everything after `</chat>`). `src/store/reduce.ts:325` `userText()` runs it before `splitTime` for history rows, `TurnStarted` and `InputInjected`; the brief that follows is processed exactly as before and gets the stamp's `at` (reduce.test "152: an endless parent view opener…"). Live `TurnStarted` carries no view (loop_.rs:158), so the card appears on reload/reseed only — no engine change.
- **D4** `src/components/chat/HarnessCards.tsx` `ResumeCard` :116 (RotateCcw; `text-warning` for fresh; time chip; muted `Working on:` / `previous harness session <short> could not be loaded — context not restored`; fold = full note, or one `Section` per parsed section with git status/diff as `<pre>` code and the omitted names as a muted line), `ChildLostCard` :151 (Unplug `text-danger`; chip links `/c/$session` like PromptCard; `why` muted; body folded), `ParentViewCard` :186 (lucide `Infinity`; `N lines` and `≈ X KB` chips via `fmtBytes`; lines mount only while open (`onToggle` state) inside `max-h-80 overflow-y-auto`; sentinel as an italic note row; the Memory panel's `LineNode` is tree-bound (kids/cursor/toggle) so its row style was copied, not exported). Header layout `HEAD`/`MUTED` :21: chips wrap in their own column, the chevron stays at the right, the muted line clamps to 1 line under 768 px and 2 above (CSS `md:` = `useIsMobile`'s breakpoint). `Message.tsx:54-59` dispatch, :144-149 labels, the `Card` remove wrapper includes the three kinds. `sessions.ts:139-142` Entry kinds; `reduce.ts:546-551` adopt signatures.
- **D5** budget notice (above). Notices otherwise still match by bare tag so a human's `<rounds source="user">` keeps a bubble (existing test).
- **D6** `crates/tui/src/model.rs` `CardKind::Restart` (`↻`) :1139 and `CardKind::Memory` (`∞`) :1141, colour SAND (`PROMPT`) in `ui.rs:1917`; `injected()` :1480-1483 learns `budget` (quiet note), `resume_handoff` (title `resumed after restart · HH:MM:SS` / `restarted fresh · HH:MM:SS`, body = inner text) and `child_lost` (`lost · <name> · <at> · <why>`); `parent_view()` :10961 + `user_entries` :10926 emit `∞ parent's memory · N lines` (fenced lines) before the brief's `you` row. Tests `injected_parses_restart_cards` :15082, `injected_parent_view_leads_the_brief` :15127.

## Deviations / decisions

- `Injected`/`Entry` field is `clock`, not `at`: `Entry.at` is the row's ms timestamp and `{ ...card, at }` would overwrite the wrapper's `HH:MM:SS` (caught by the adopt test).
- A live `<resume_handoff>` (an ACP child's respawn prompt, only echoed by `TurnStarted`) is now shown live (`reduce.ts:903`): the "wrapped inputs are delivered by other events" rule has no delivery event for it. `<child_lost>` live arrives as `MessageDelivered` (peer card from `session:<child>`) and becomes the `lost` card on reload, like every injected wrapper.
- The fresh card's `Note` section carries the header and footer sentences (session_read pointer) so the fold shows every character of the note.
- The proof page needed a `TooltipProvider`, a memory-history router with `/c/$session`, a stub `api` on the `local` connection scope and the app's vite config minus the router codegen; root stayed the app dir (`proof-152` symlink → `/tmp/152/proof`, removed before the final commit) so Tailwind's source scan covers `src`.

## Gates

- tsc: baseline 1 error (`src/components/autoresearch/fixtures.ts(344,9)` TS2741, 151's) → after 1, `comm -13` empty; no error names a changed file.
- vitest `src/lib src/store src/components/chat`: 145 files, 1150 tests, all green (+14 tests in injected.test.ts, +2 in reduce.test.ts).
- tui: `mbx test -p tui --lib -- injected` 7 passed / 0 failed (2 new); `cargo fmt -p tui --check` clean; `mbx clippy -p tui --lib -- -D warnings` exit 0, 0 warnings.
- emoji: no new literal emoji under `crates/desktop/ui/src` (TUI glyphs `↻` U+21BB, `∞` U+221E are outside `tests/no_emoji.rs`' ranges).

## Proof screenshots (`/tmp/152/shots/`, not committed)

62 PNGs, `<card>-<state>-<width>-<theme>.png` for cards `resume-restored`, `resume-fresh`, `resume-fresh-cut`, `child-lost`, `child-lost-two`, `parent-view`, `budget` × `folded|open` × `1000|390` × `light|dark`; `parent-view-long-open-390-{light,dark}.png` and `parent-view-long-folded-390-light.png` (400 lines, ≈ 41 KB, inner scroll); `control-folded-{1000-light,390-light,1000-dark}.png` (the same fixtures through `origin/main`'s `classify` → raw `you` bubbles). Driven by headless Chromium (puppeteer/CDP) against the vite page `/tmp/152/proof/`.

## Follow-ups

- Live `<child_lost>` shows as a peer card until the reload turns it into the `lost` card; a `ChildLost` event (engine) would make them one.
- The advisor session's `### Session update` rows and the schedule wrapper's nested `<until>/<request>/<when>` blocks render raw markdown/text inside their cards; card them if advisor sessions ever get a transcript surface.
- `<job_result tool="…">` (the `tool=` form) reads an empty `command` in the UI label — cosmetic.
