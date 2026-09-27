# 70 — as-built BOARDS step 1: `surface` anchored primitives

Design 71 v2 §5 row 1. Branch `71/step-1-surface` (worktree of `feature/71-boards`). Mechanisms served: M1 (one shape per idea — `Sequence` joins the catalog so a code-bound sequence needs no mermaid), M3 (`Excerpt` for the few spots; `ref`/`code:` links elsewhere), M14 data side (every `Graph` node and `Sequence` step is an addressable unit with a `ref`), M11 (`Excerpt.symbol`). Desktop mirrors are step 3; `crates/desktop` untouched.

## Commit

`bc816f2a feat(surface): Excerpt, Sequence, GraphNode.ref and the anchor grammar (71 step 1)` — `crates/surface/**` only.

- `spec.rs`: `Component` 36 → 38. `Sequence(SequenceProps)` after `Graph`; `Excerpt(ExcerptProps)` after `Diff`.
  - `SequenceProps { actors: Vec<SequenceActor{id, label}>, steps: Vec<SequenceStep{from, to, label, style?: call|return|async, ref?}>, title? }`.
  - `ExcerptProps { path, text, start?: u32 ≥ 1 (default 1), lang?, caption?, mark?: Vec<[u32; 2]>, symbol? }`.
  - `GraphNode.ref?: String` (`r#ref` in Rust; serde/schemars/ts-rs all emit `ref`).
- `anchor.rs` (new, `pub mod anchor`): `parse(&str) -> Result<Anchor, String>` for the §2.3 grammar — `Anchor::Code { path, lines: Option<(u32, u32)> }` (`#L<a>` = `(a, a)`) and `Anchor::Block { id }`; `check_path` is the shared root-relative rule (no leading `/`or `\`, no `.`/`..`/empty segment, no `#`, no scheme — `http:`, `file:`, `C:` — no tabs/newlines). `Excerpt.path` runs through the same `check_path`.
- `validate.rs`: `sequence()` — 2..=12 actors (`/props/actors`), 1..=40 steps (`/props/steps`), duplicate actor id (`/props/actors/i/id`), unknown `from`/`to` (`/props/steps/i/from|to`), label ≤ 80, `ref` grammar (`/props/steps/i/ref`). `excerpt()` — path (`/props/path`), 1..=120 lines and ≤ 16 KiB (`/props/text`), each `mark` pair ordered and inside `start..=start+lines-1` (`/props/mark/i`). `graph()` gains `/props/nodes/i/ref`. `label()` and `anchor_ref()` are shared helpers. Constants in `lib.rs`: `SEQUENCE_ACTORS`, `SEQUENCE_STEPS`, `MAX_EXCERPT_LINES`, `MAX_EXCERPT_BYTES`.
- `sequence.rs` (new): `steps_lines(p, width)` — the TUI/text arm: bold title, then `1. parser → validator  check(spec)   crates/…#L12-L40` per step (`→` call, `⇢` return, `⇾` async — arrows block, not emoji); the dim `ref` moves to its own indented line when it would not fit `width`. `to_mermaid(p)` — `sequenceDiagram` with `participant id as Label`, `->>` / `-->>` / `->`, `title`, `;` → `,`, ids through the shared `graph::mermaid_ids` (now takes any id iterator; `participant`/`actor` added to `RESERVED`). `lib::to_mermaid(&Component) -> Option<String>` dispatches `Graph`/`Sequence` for export channels.
- `render.rs`: `Component::Sequence` → `steps_lines`; `Component::Excerpt` → dim header `path:start-end · symbol · lang`, numbered gutter `▸ 28 │ code` with `▸` on marked rows, caption wrapped dim under. `flatten` gets both for free through `spec_lines_text`.
- `catalog.rs` / `prompt.rs`: 38 names, `EXAMPLES` 38 (Sequence and Excerpt examples quote real `crates/surface` lines), `ty()` prints `integer ≥ 1` when a schema has a minimum > 0 and no maximum (was `integer 1..null`); `mark` reads `integer[][]`.
- Fixtures: `valid-excerpt` (validate.rs `esc`, L26–29, mark L28), `valid-sequence` (6 steps model → parse → validate → render with real refs), `broken-excerpt-mark`, `broken-sequence-actor`, `broken-ref-scheme` (Graph node `http://…` and Sequence step `../x` in one Stack); `graph-architecture` gains two `ref`s. Goldens `valid-{excerpt,sequence}-w{60,100}.txt`; no existing golden changed.
- `visualize.md`: pick-table rows for `Sequence`, `Excerpt` and "a board (board.md)"; intro paragraph routes anchored sequences to `Sequence`; json-render rules for `Sequence`, `Excerpt` and `ref`.

## Done-when — evidence

| Item | Evidence |
|---|---|
| `cargo test -p surface` green | 41 passed, 0 failed (`lib.rs`, `validate.rs`, `sequence.rs`, `anchor.rs`, `render.rs` goldens). `cargo check --workspace --tests` exit 0. `cargo test -p tui --test no_emoji` 1 passed. |
| `catalog_compact()` lists `Excerpt(` and `Sequence(` | `catalog::tests::compact_lists_every_component_with_props` pins `… · Graph(nodes[{id, label, group?, kind?, ref?, state?}], …) · Sequence(actors[{id, label}], steps[{from, to, label, ref?, style?}], title?) · Steps(…` and `Diff(…) · Excerpt(path, text, caption?, lang?, mark?, start?, symbol?) · Tree(`; `just ts` wrote `bindings/surface-catalog-compact.txt` with both; `bindings/GraphNode.ts` and `SequenceStep.ts` carry `ref: string \| null`. |
| TUI golden: numbered gutter with marks, numbered step list | `tests/golden/valid-excerpt-w60.txt`: `crates/surface/src/validate.rs:26-29 · esc · rust` / `  26 │ /// JSON Pointer…` / `▸ 28 │     token.replace(…)`. `tests/golden/valid-sequence-w60.txt`: `1. Model → parse  fence body   crates/surface/src/lib.rs#L90` … `4. validate ⇢ parse  Spec or located errors` with the ref wrapped to the next line at 60 columns and inline at 100. |
| Every `Graph`/`Sequence` fixture lowers to mermaid that merman parses | `graph::tests::every_valid_fixture_lowers_to_mermaid_that_merman_draws` (3 graph fixtures, now with `ref`s) and `sequence::tests::fixture_lowers_to_mermaid_that_merman_draws` (`mermaid_lines(src, 0)` is `Ok(Some)`; source contains no `crates/` — refs dropped). |
| A `ref` of `../x` or `http:` is a located error | `lib::tests::broken_fixtures_yield_the_expected_path`: `broken-ref-scheme` → `/elements/g/props/nodes/0/ref` and `/elements/s/props/steps/0/ref`; `validate::tests::sequence_and_excerpt_errors_are_located_with_messages` pins the messages `path must not carry a scheme: http:` and `L3 is before L9`; `anchor::tests` cover 18 rejects incl. `../x`, `a/../x`, `/etc/passwd`, `C:\x`, `file:///x`, `#L0`, `#L10-L9`. |

## Deviations

1. **`▸` (U+25B8) instead of `▶` (U+25B6) on marked gutter rows.** 70-NO-EMOJI ruled `▶` an emoji-presentation glyph (macOS/Chromium draw it with the emoji font) and already replaced it with `▸` in `surface/render.rs`; the `no_emoji` lint rejects U+25B6. Same shape, single width.
2. **Async steps lower to mermaid `->` not `-)`.** merman 0.7 (the only consumer of the lowering today) declines `-)` and `--)` (`Ok(None)`, probed); `->` — solid line, no head — is mermaid-valid and the nearest mark for an open arrowhead. The TUI list uses `⇾` (U+21FE, open-headed arrow).
3. **`Anchor::Code.lines` is `Option<(u32, u32)>`** with `#L7` = `(7, 7)`, rather than separate `start`/`end` options: one shape for the overlap rule in step 4 and `Locate` in step 7 maps trivially.
4. **`check_path` also rejects `#`** (not in §2.3's prose). An `Excerpt.path` with `#` would make the derived `path#L<start>-L<end>` anchor ambiguous; the same rule on the path half of a `CodeAnchor` costs nothing (the grammar splits on the first `#`).
5. **`GraphNode.ref` is not printed in the adjacency list** (TUI/text form of a `Graph`): lines are already truncated to width; the design does not ask for it. `Sequence` steps do print their `ref` (the design's TUI arm).
6. **The prompt block (`## Show first, then tell`) is unchanged**; only `visualize.md` gained rows, as §3.3 names. Its catalog line picks up the two components automatically. The board routing lives in `board.md` (step 2).
7. **Sequence label cap reuses `MAX_GRAPH_LABEL_CHARS` (80)** via the shared `label()` helper instead of a second constant.
8. `cargo check --workspace --tests` fails on a fresh worktree until `just ts` has run once (`crates/nodes` `include_str!`s `bindings/methods.json`); pre-existing, not touched.
