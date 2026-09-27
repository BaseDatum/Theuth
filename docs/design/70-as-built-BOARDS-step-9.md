# 70 — as-built BOARDS step 9: reference boards, board-check, steering, consolidated as-built

Design 71 v2 §5 row 9. Branch `71/step-9-reference-boards-check` (worktree of `feature/71-boards`, base `69c4c3b9` = steps 1–8). Mechanisms served: all of M1–M14 as acceptance; §3 instructions tuned once from the steering run; §4 recipes exercised by the real binary; §6.4 amendment for 60 §8. Consolidated as-built: `70-as-built-BOARDS.md`.

Environment for every run: scratch engine from this build (`THEUTH_DATA_DIR=/tmp/69/71-dd-theuth-71-s9 THEUTH_SECRETS__KEY_STORE=file THEUTH_TOOLS__APPROVALS=false THEUTH_RPC__LISTEN=127.0.0.1:7899 THEUTH_LINK__LISTEN=127.0.0.1:7889 target/debug/t --headless`, cwd = this worktree, `ANTHROPIC_API_KEY` from the shell, model `anthropic/claude-sonnet-4-5`, `agent.max_tool_rounds` default 32), `packages/documents/dist/theuth-documents-59.5` copied beside `t` (step 8 finding 1), `bunx vite --port 5211` with `VITE_THEUTH_URL/TOKEN` against it, headless Chromium at 1440×900 / 1024×768 / 390×844. Prompts went through RPC `new_session` + `send_input` with a per-session event recorder (throwaway driver `/tmp/69/71w9-steer.ts`; records at `/tmp/69/71w9-runs/{round1,round2,round3,ref}/*.json`). `code_locate` accepted the worktree as a git top-level (`gix` discovers the linked work tree).

## What was built

- **`scripts/board-check.py`** (new, stdlib only) — the M12 machine read-back over a board's markdown (`doc_read(format="markdown")`, the derived `documents/<id>/note.md`, or a fixture): M2 heading order (subset of the six names, no repeat, no seventh), M12 every `Excerpt.text` verbatim against `root/path` lines `start..` (Rust `str::lines` semantics), M14 every `Graph` node / `Sequence` step has a `ref`, §2.3 every anchor resolves (file exists, range inside it; `block:` needs the id in the export), M3 ≤ 3 excerpts per section + a warning for "How it works" paragraphs without a `code:` link, M1 warning for two diagrams in one section. `--root` overrides the frontmatter root, `--reference` adds the §5 row-9 thresholds (≥ 4 sections, ≥ 3 excerpts), `--json` for tabulation, `--self-test` builds a two-line repo and asserts the pass and the four failure modes. Exit 1 on any FAIL. Tolerates the three JSON slips the engine's jsonrepair pass forgives and models made in these runs: raw newlines in strings, `\`` escapes, single-quoted strings.
- **Three reference boards** — `crates/desktop/ui/src/components/board/fixtures/reference/{surface-fence-render,step-1-commit,popout-boot}.md`, the derived `note.md` of boards `agqocxziuz4xfnmygggmnhglbq`, `agqocxxs2j7hbgeqhpqvkzuade`, `agqocysy2r2yddfdpj4miioega`, drawn by the real binary from the skill alone (no hand edits; prompts below). `board-check --reference --root .` passes all three.
- **§3.1 `## Boards` prompt block** (`crates/document/src/tools.rs`) — tuned once, after round 1 (1/8 boards): the routing sentence names what it overrides ("**When the question is about code in the working tree … the reply is a board, not an inline `Graph`/`Sequence` fence**: the Show-first shapes are for everything else"), every `doc_*` call is shown from python (`r = await doc_create(…)`; step 8 finding 3), the anchor form is `path#L10-L24` everywhere (the old text taught `path:line`), `never adding a seventh` section, `doc_read(…, format="markdown")` (step 8 finding 2).
- **`board.md` skill** — tuned from the runs (front matter quoted so it parses — see fix 4; "Board or chat" names the Show-first override and the python-only tools; a seventh `##` goes under How it works as `###`; `never path:10`; `ref` on every step including returns, directories point at a file, non-code actors are `Sequence` actors; `symbol` is the definition's exact name or omitted; three excerpts per board is the norm, each its own insert; the read-back is not optional; `doc_read(blocks, format="markdown")`).
- **Small fixes exposed by the runs** (each minimal, listed):
  1. `crates/surface/src/anchor.rs` + `chat/surface/anchor.ts` (+ both test lists): a `:` anywhere in an anchor path is refused with `path must not contain ':'; a line is #L<n>, not :<n>`. Round 1's only board wrote all 10 `ref`s as `path:1` — valid to the grammar, dead in the reader.
  2. `crates/document/src/tools.rs` `check_excerpts`: every `code:` link and every unit `ref` of an insert/replace is located too (existence only, no hash); `missing` is refused as `` `<anchor>` is a dead anchor: <path> has N lines | there is no file <path> (check the name) | <path> is a directory — point at a file in it ``. 3 of 6 round-3 boards carried a link range past EOF or a directory `ref`. The refusal header is now `nothing changed: an anchor does not match the tree`. `tests/actor.rs` covers a range past EOF, a missing file and a good link.
  3. `check_excerpts` `missing` refusal for an `Excerpt` with a `symbol` adds `no definition named `X` in <path> — if the lines exist, drop `symbol` or name the definition that starts at L<start>` (round 1 G: the model named a symbol that does not exist and dropped the excerpt).
  4. `crates/document/board.md` front matter: `description:` held an unquoted `: ` since step 2, `gray_matter` failed, the skills index read `- board: ---`; quoted, the index now shows the description (`pdf.md` has the same defect; not this step's file).
  5. `crates/surface/src/prompt.rs` (Show-first block, two additive lines): a pick-table row `how code in this repository works, or what a change did, in three or more parts → a board (see Boards)` and the `"How does X work"` shape line gains `— unless X is code in this repository and the answer has three or more parts: that is a board`. The direct contradiction round 1 exposed.
  6. `components/board/Tour.tsx`: a stop whose anchor no excerpt covers renders `LiveWindow` (exported from `SidePane.tsx`) instead of the step-4 placeholder — on the popout board every tour stop read "No excerpt on this board covers these lines." (`rootOf` takes `readonly DocBlock[]`).
- **Docs** — `60-codeindex.md` §8 amendment (71 §6.4 "Steps 7/9"); `70-as-built-BOARDS.md` (consolidated).

## Done-when — evidence

| Item | Evidence |
|---|---|
| Each board ≥ 4 sections, ≥ 3 `Excerpt`s, every unit has a `ref`, `board-check` 100 % verbatim, every anchor resolvable | `scripts/board-check.py --reference --root . fixtures/reference/*.md` → `PASS popout-boot: sections 4 · Excerpts 3/3 verbatim (100 %) · diagrams 2 · units 12/12 with ref · anchors 22/22 resolvable · How it works 2/2 paragraphs anchored` · `PASS step-1-commit: sections 5 · 3/3 · units 17/17 · anchors 40/40 · 2/2` · `PASS surface-fence-render: sections 5 · 3/3 · units 14/14 · anchors 40/40 · 3/3`; exit 0. Verbatim is by construction: 7b's insert-time check refused every retyped excerpt in these runs (11 refusals across the reference draws) and none landed. |
| ≥ 8/10 of the first eight prompts produce a board | **Not met: 5/8 after the one tune** (round 1: 1/8 untuned; round 2: 2/8 tuned; round 3: 5/8 tuned + the skill description visible). Tables below. The two comparison prompts stayed inline in all three rounds (6/6). |
| 2/2 comparisons stay inline | I (zustand vs context) and J (`Sequence` vs mermaid): inline in rounds 1, 2, 3 — `Table`/`Tabs` fences, no `doc_create`. |
| times: create → first block | Reference draws: 68 ms (R1c), 9 333 ms (R2c), 6 695 ms (R3d); round 3 boards: 6 832 / 68 / 89 / 96 / 23 375 ms. Under 100 ms = `doc_create` and the first `doc_insert` in one python cell (the skill's "immediately"); seconds = a model round between them. `doc_create` itself landed 11–165 s after the prompt (orientation reads first). |
| times: 6-insert cell | `/tmp/69/71w9-insert.ts` (six `doc_update`s, one Yjs transaction each, from `/tmp/69/71w6-six.md`) on an open board: batch 90 ms; the page stamped the six new `[data-block]`s at +49, +76, +105, +161, +181, +181 ms after the batch started, each with `data-rise` and `animation-name: board-rise`. |
| times: 40-block first paint | The 56-block popout board (`agqocvizpb6ifkhrmb2bwreb6i`): reload → `.board-column [data-block]` present at 572 ms after navigation start, all 56 blocks in the first render (56 at +1.5 s too), `first-paint` 228 ms, `first-contentful-paint` 320 ms, 0 `[data-rise]`, 0 `board-*` animations. |
| three widths' screenshots | `/tmp/69/71w9-1440-open.png`, `71w9-1440-pinned.png` (grid `0px 610px 10px 560px`, live window `same`), `71w9-1440-ref-popout.png`, `71w9-1440-ref-tour.png` (6 stops, `836px 10px 594px`), `71w9-1024-open.png`/`-pinned.png` (`0px 764px 0px 0px`, pill 32 px, sheet 286 px = 40 % of 716), `71w9-390-open.png`/`-pinned.png` (one column 390, prose 350, `scrollWidth` 390, sheet 317 px), `71w9-plain-note.png`. |
| serif default ON | `lib/boards.ts`: `serif: globalThis.localStorage?.getItem('boards.serif') !== 'false'`; on the reference board `.board-prose p` computes `Newsreader, Georgia, serif`, `.board-chrome` `ui-monospace, SFMono-Regular, Menlo, …`, `data-serif` unset. |
| no-emoji lints | `cargo test -p tui --test no_emoji` 1 passed; `CI=1 bunx vitest run src/lib/no-emoji.test.ts` 1 passed (with the three fixtures under `src/`). |
| a plain note is unchanged | `/notes` → **New note** → `/notes/agqocxyhuj35feqj53o2o7baqq`: `.bn-editor` true, `.board-column` false, no `board` chip, no Edit/Read toggle, no `.board-chrome`. |
| M4 on a real board | Hover `[…](code:crates/desktop/ui/src/routes/__root.tsx#L20-L27)` → 6 elements `data-anchor-active` (three spans of that range, a `Graph` node `#L22-L26`, a `Sequence` step `#L22`, the span); click → pane `0px 610px 10px 560px`, `[data-live-window="same"]`. Tour from the `Sequence`: 6 stops, stops 1–3 and 6 live windows (`same`), 4–5 covering excerpts; `Escape` → focus on `.board-tour-button`. |
| toolchain | `cargo check --workspace --tests` clean; `cargo test -p surface` 41; `cargo test -p document` 44 + 11 (the new dead-link asserts included); `cargo test -p tui --test no_emoji` 1; `bunx tsc --noEmit` 0; `bunx vite build` ✓; `CI=1 bunx vitest run` 94 files / 549 tests. |

### Steering — round 1 (untuned prompt block, invalid skill front matter)

| id | kind | prompt | outcome | s | tool calls | inline fences | doc |
|---|---|---|---|---|---|---|---|
| A | how | How does the surface crate render a fence? | inline | 108 | 30 | 8 | — |
| B | how | How does a popout window boot? | inline | 78 | 10 | 2 | — |
| C | how | How does doc_insert validate a json-render block before it lands in a note? | inline | 20 | 0 | 0 | — |
| D | how | How does code_locate decide between same, changed, moved and missing? | inline (asked which tool I meant) | 10 | 0 | 0 | — |
| E | how | How does the board reader decide which anchor is active? | inline | 49 | 11 | 2 | — |
| F | walk | Walk me through what commit bc816f2a changed. | inline | 40 | 7 | 2 | — |
| G | walk | Walk me through the change in commit b619dcd6 (code_locate). | **board** | 123 | 46 | 0 | `agqocpjs…` — 6 `##` + `## Integration`, 0 excerpts landed (1 refused: wrong `symbol`), 10/10 `ref`s written `path:line` (dead), 0 `doc_read` |
| H | walk | Walk me through what commit 3833f19f changed. | inline | 44 | 6 | 2 | — |
| I | cmp | Compare zustand and React context for per-board UI state… | inline | 42 | 0 | 2 | — |
| J | cmp | Sequence vs mermaid sequenceDiagram in this repo… | inline | 29 | 0 | 3 | — |

Boards 1/8; comparisons inline 2/2. The model followed the Show-first shape line (`"How does X work": Graph or sequenceDiagram first`) and drew `Stack`/`Sequence` fences inline; B also used `show`.

### Steering — round 2 (tuned §3.1 + Show-first row; skill front matter still invalid)

| id | outcome | s | tool calls | inline fences | doc |
|---|---|---|---|---|---|
| A | inline | 105 | 16 | 2 | — |
| B | inline | 69 | 19 | 0 | — |
| C | inline | 17 | 0 | 0 | — |
| D (reworded: "the codeindex crate's Locate message") | inline | 32 | 7 | 1 | — |
| E | inline | 31 | 11 | 1 | — |
| F | **board** | 174 | 71 | 0 | `agqocrif…` — 5 `##` out of order (Checks before Decisions), 0 excerpts, 9/12 `ref`s |
| G | **board** | 135 | 63 | 0 | `agqocrjv…` — 3/3 excerpts verbatim, 14/16 `ref`s, 24/25 anchors (one link past EOF) |
| H | inline | 50 | 8 | 3 | — |
| I | inline | 39 | 0 | 2 | — |
| J | inline | 25 | 8 | 1 | — |

Boards 2/8; comparisons inline 2/2. `get_prompt` confirmed the tuned block in place; the skills index still read `- board: ---`.

### Steering — round 3 (same prompt; skill front matter fixed → description in the index)

| id | outcome | s | tool calls | create→first block ms | inserts | refusals | board-check on the result |
|---|---|---|---|---|---|---|---|
| A | **board** | 152 | 68 | 6 832 | 10 | 3 | 4 sections · 1/1 verbatim · 15/15 refs · 33/34 anchors (a directory link) · 9/11 paragraphs anchored |
| B | inline | 93 | 13 | — | 0 | 0 | — |
| C | inline (answered from the prompt block, 0 tool calls) | 21 | 0 | — | 0 | 0 | — |
| D | **board** | 145 | 65 | 68 | 12 | 2 | 5 · 1/1 · 17/17 · 34/34 · 7/8 |
| E | inline | 82 | 10 | — | 0 | 0 | — |
| F | **board** | 113 | 64 | 89 | 5 | 3 | 4 · 0/0 · 5/7 · 19/20 · 3/4 |
| G | **board** | 150 | 74 | 96 | 12 | 5 | 5 · 2/2 · 13/17 · 32/33 · 3/3 |
| H | **board** | 150 | 81 | 23 375 | 12 | 1 | 5 · 3/3 · 7/7 · 12/13 · 1/10 |
| I | inline | 29 | 0 | — | 0 | 0 | — |
| J | inline | 34 | 7 | — | 0 | 0 | — |

Boards 5/8; comparisons inline 2/2. Every excerpt that landed is verbatim (7b); the remaining defects are un-`ref`'d return steps / non-code nodes, link ranges never read (past EOF — fix 2 closes this at insert), and one to two excerpts where the skill asks for three.

### Reference draws (prompts `Draw a board: <topic>.`, third round `… Anchor every diagram unit and quote three excerpts.`)

| run | doc | sections | Excerpts verbatim | units with ref | anchors | create→first block ms | inserts | refusals | `doc_read` | s | board-check |
|---|---|---|---|---|---|---|---|---|---|---|---|
| R1 | `agqocvji` | 5 | 2/2 | 15/15 | 43/43 | 71 | 15 | 4 | 2 | 186 | 2 excerpts |
| R1b | two docs | — | — | — | — | — | 16 | 5 | 3 | 235 | the model looped on a misspelt file (`fence.rs`) five times, created two boards, both empty — fix 2's message now names "there is no file" |
| **R1c** | `agqocxzi` | 5 | 3/3 | 14/14 | 40/40 | 68 | 9 | 2 | 1 | 178 | **PASS** → `surface-fence-render.md` |
| R2 | `agqocvfy` | 5 | 2/2 | 11/13 | 23/23 | 6 839 | 12 | 3 | 2 | 153 | 2 units without ref, 2 excerpts |
| R2b | `agqocwjm` | 6 | 2/2 | 12/12 | 32/32 | 8 014 | 12 | 2 | 1 | 165 | 2 excerpts |
| **R2c** | `agqocxxs` | 5 | 3/3 | 17/17 | 40/40 | 9 333 | 16 | 4 | 2 | 174 | **PASS** → `step-1-commit.md` |
| R3 | `agqocviz` | 6 | 3/3 | 10/11 | 48/48 | 11 316 | 6 | 1 | 0 | 145 | `webview` node without ref |
| R3b | `agqocwm5` | 0 (`###` headings) | 6/6 | 8/14 | 28/28 | 8 844 | 6 | 3 | 1 | 230 | sections as `###`, 6 excerpts in one section |
| R3c | `agqocx2e` | 4 | 3/3 | 16/17 | 29/29 | 68 | 10 | 3 | 1 | 128 | one return step without ref |
| **R3d** | `agqocysy` | 4 | 3/3 | 12/12 | 22/22 | 6 695 | 10 | 2 | 1 | 145 | **PASS** → `popout-boot.md` |
| R3e | `agqocyrz` | (parallel spare; not checked into fixtures) | | | | 9 202 | 10 | 2 | | 174 | — |

Refusals across all reference draws: enum guesses (`kind: "component"`, `style: "sync"`, `direction: "TD"`), a `Table` with non-string columns, an unknown `Col` component, retyped excerpts (7b, 4×), dead anchors (fix 2, 6× — including `fixtures/json-render` and `tests/golden` directories, `validate.rs#L835-L9xx` past EOF, `fence.rs` misspelt), a wrong `symbol` (fix 3, 1×). Every refused block was either corrected on the next call or dropped; none landed.

## Deviations

1. **Reference prompts say `Draw a board:`** (and, third round, `Anchor every diagram unit and quote three excerpts.`). The plain questions routed inline for two of the three topics in every round (B never boarded); the row demands boards drawn "from the skill alone", which I read as no hand edits and no injected blocks — the skill drew every block, the engine refused what did not check out. Recorded here so the routing number is not read from the reference draws.
2. **≥ 8/10 not reached: 5/8.** One §3.1 tune was allowed and used (after round 1); round 3 added only the skill-front-matter fix. The lever that moved the number was making the skill description visible (2/8 → 5/8), not prose in the prompt block. What is left is a model preference: sonnet-4-5 under a 63 k-character system prompt with the Show-first block answers "how does X work" inline about 40 % of the time and "how does *tool* work" from the prompt with 0 tool calls (C, all rounds). Not re-tuned; see Findings.
3. **Prompt D reworded** after round 1 (`code_locate` → "the codeindex crate's Locate message"): the model read `code_locate` as a tool it does not have and asked which one was meant — a prompt defect, not a steering signal.
4. **Three rounds, not one.** The design says the ten prompts run once and the prompt is tuned once from that; the fixed skill front matter (a bug since step 2) invalidated round 2 as a measure of the tune, so the set ran a third time. All three are tabulated.
5. **`board-check` enforces a `ref` on every unit** (row 9's "every unit of every diagram has a ref"), stricter than §3.2's "units that describe code". `--reference` thresholds are a flag, not the default, because M2/M3 are board rules and ≥ 4 / ≥ 3 are the reference-board bar. The "≥ 1 anchored claim per paragraph in How it works" (M3 check) is reported as a warning, not a failure: intro paragraphs before the diagram legitimately carry none.
6. **The dead-anchor insert check (fix 2) is stricter than the reader.** The reader shows `missing` on a dead link; the engine now refuses the block. Same rationale as 7b: the correction round is cheap, the dead link is permanent. A directory `ref` is refused (the reader could not open a window for it).
7. **Skill tuning went beyond §3.2** (return steps carry a `ref`, directories point at a file, three excerpts is the norm, the read-back "is not optional", `format="markdown"`), each line from a failure in these runs.
8. **`board-check` repairs JSON like the engine does** (raw newlines, `\``, single quotes) rather than requiring strict JSON: the engine stores the block as the model wrote it and `surface::parse` repairs at read time, so a strict checker would fail excerpts the reader renders fine.
9. **Screenshots are of the drawn boards in the browser against the scratch engine**, not the committed fixtures re-imported: same bytes (`note.md` is the fixture).

## Findings

1. **Routing is the open problem; drawing is solved.** Once a board is chosen, `board-check` passes within three draws per topic and every landed excerpt is verbatim. A structural lever remains untried: the `## Boards` block sits ~50 lines into a 63 k-character prompt while the Show-first block is 60 lines with a 30-row table; a one-row entry in the Show-first *table* (done) did not outweigh the shape line's habit. Candidates for a later step: put the board routing sentence at the head of the Show-first block itself, or have the `document` plugin register the board block *before* the surface block.
2. **The model does not read the skill description as an instruction to load the skill** — in round 3 it loaded `board` in every boarding run and in none of the inline ones; loading is a consequence of the routing decision, not a cause.
3. **`###` for section headings** (R3b) is a failure `board-check` catches and the reader would render as one section with no TOC entries; an insert-time refusal of a `###` that spells one of the six names with no `##` above it would give the model its correction round. Not added.
4. **Un-`ref`'d return steps / non-code nodes** are the most frequent remaining `board-check` failure (5 of 12 boards). The skill now says how to anchor them; enforcing `ref` on every unit at insert would be the 7b move again, but §3.2 allows units that do not describe code.
5. **`pdf.md` has the same invalid front matter** as `board.md` had (`- pdf: ---` in the skills index).
6. **`code_locate` on a worktree root works** (`gix` finds the linked work tree), so a board's `root` may be any checkout.
7. **The model wrote `root="/cwd"` once** (A, round 3) before correcting to the absolute path; the prompt block's `root=<absolute repo root>` is read as a placeholder by some turns.
