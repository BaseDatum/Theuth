# 70 — as-built SURFACES: openui inline, thirty-six json-render components, agents draw more

Branch `feat/surfaces-inline-openui-more-components`. Sky's 2026-09-25 ruling: openui fences merge into the reply like json-render (reverses the 09-23 "Open as app" card), more json-render components, agents use them more.

## Commits

1. `feat(surface): seventeen more json-render components and Metric.spark`
   - `crates/surface/src/spec.rs`: `Component` gains, after `Graph`: Steps, Callout (variant `note|tip|question|important`), Code, Diff (`before/after`, `from?/to?` headings), Tree (flat rows with `depth` 0..6), Glossary, Quote, Image, LinkList, Tags (Badge variants), Gauge, Sparkline, Heatmap (`rows × cols`, `cells`), Person, FileList, Kanban (columns of cards, read-only), Calendar (days with Timeline states). `MetricProps.spark?: number[]` is the "Stat with sparkline" (MetricGrid items get it for free).
   - `validate.rs`: Heatmap shape (`/props/cells`, `/props/cells/<i>`), `Image.src` must be `http(s)://` or `data:image/`, `LinkList.items[].url` must be `http(s)://` (28: the two byte-carrying props). Test `heatmap_shape_and_url_schemes`.
   - `render.rs`: one TUI arm per component (sparkline glyphs `▁…█`, heatmap shades `·░▒▓█`, Kanban/Diff/Calendar go side by side when they fit and stack otherwise). `Badge` and `Tags` share `tag()`.
   - Fixtures: `valid-{steps,callout,code,diff,tree,glossary,quote,image,links,tags,gauge,sparkline,heatmap,person,files,kanban,calendar}.json`, `valid-metrics.json` gains a spark; `broken-{heatmap-ragged,image-src,link-url}.json`; golden pairs at 60/100 columns regenerated with `UPDATE_GOLDEN=1`. `lib.rs` `VALID` 15→32, `LOCATED` 7→10; `catalog.rs` names/compact pins 19→36.
   - `prompt.rs`: `EXAMPLES` 19→36; **Show first, then tell** rewritten — (a) "Most substantive replies contain at least one rendered block; a prose-only reply is the exception you chose", (b) the pick-by-shape table names every new component (Steps, Diff/Heatmap, Kanban, Calendar, Gauge/Sparkline, Callout vs Alert, Glossary, Code, FileList/Tree, Tags, LinkList, Image, Person, Quote), (c) "Three fenced blocks render inline, merged into your reply" and the openui line says "rendered inline exactly like json-render (not as an app card)" with the explanatory signature set, (d) rules kept (one picture per idea, ≤ ~12 nodes/~15 rows, ≤ 4-word labels, complete strict JSON, `<surface_error>`). Unattended gating untouched (`loop_::prompt_blocks`, `frame::REPORT_VISUALLY`).
   - `visualize.md`: intro says most replies carry a block and all three fences render inline; Choose-the-block table covers every component; json-render rules add Heatmap/Image/LinkList.
2. `feat(desktop): openui fences merge into the reply; renderers for the seventeen new json-render components`
   - `OpenUIFence.tsx`: wrapper `div.group.relative.my-4.text-sm[data-streamdown=openui]` (the json-render `Surface` margins); no card, no title bar; hover/focus toolbar (absolute top-right) with ghost **Open as app** (existing `publish_app` path, only in a live session, disabled while streaming) and **Show source** / **Hide source** (a `<pre>` under the render). Error path unchanged: `FenceFallback` → skeleton / nothing / `◌ diagram omitted · show source`.
   - `catalog.ts` (zod mirror, 36), `parse.ts` (three cross-field rules, same paths/messages as Rust), `registry.tsx` (17 renderers; helpers `Tag` shared by Badge/Tags/Kanban, `Spark` shared by Metric/Sparkline, `Picture` for Image, `TreeRows` nesting by depth over `Collapsible`, `DiffSide` positional compare — `// ponytail:` names the ceiling). Remote `Image.src` loads only after the reader clicks **Load image**; an inline `data:image/` shows at once.
   - Tests: `registry.test.ts` (18, one behaviour per component through the real fence → parse → registry path), `parse.test.ts` rule test, `catalog.test.ts` pin 36, `OpenUIFence.test.ts` asserts no card chrome and the toolbar.
3. `feat(apps): OptionCards curated; prompt signatures cover the explanatory set; apps skill says fences render inline`
   - `packages/apps/src/library.ts`: `OptionCards`, `OptionCard` appended (upstream Forms group); Chips, Carousel, ImageGallery, RadarChart, ScatterChart were already curated. Regenerated `generated/{library.json,catalog.txt,library-hash.txt}` — 56 components, library hash `4691f8041bf6` (the hash gates the parser helper: `just helper` rebuilds it; committed apps under the old hash re-check, 46 §8.3).
   - `crates/app/src/tool.rs`: `PROMPT_COMPONENTS` 23→31 (+Tag, Accordion, AccordionItem, CodeBlock, Chips, ChipItem, OptionCards, OptionCard); prompt prose says an openui fence renders inline like json-render, apps are for live data/actions; skill-only sentence now names Carousel/ImageGallery/RadarChart/ScatterChart/forms. Test updated accordingly.
   - `crates/app/apps.md`: new section "An inline fence in a reply".
4. `docs(design): 30 surfaces updated; 70 as-built SURFACES` — `docs/design/30-surfaces.md` §6.2 now points at `prompt.rs` as the verbatim source (the copy had drifted) and summarises the ruling; §8 openui inline paragraph; §10 thirty-six components; §11 Image/LinkList as the two carriers and their mitigations; §16 count.

## Deviations

- Brief line 11 (67 template) names `/tmp/67/…` and `67-as-built`; line 18 (this brief) names `/tmp/69/report-SURFACES.md` and `docs/design/70-as-built-SURFACES.md`. Followed line 18.
- "Stat with sparkline" is `Metric.spark?` rather than a separate `Stat` component: MetricGrid items get it for free, one fewer name for the model to learn.
- Tree is a flat list with `depth` per row, not a recursive struct: schemars inlining and the schema-subset validator do not do recursion; the desktop rebuilds the nesting.
- Diff is two-column only (no unified mode); positional line compare.
- Image: remote `src` is click-to-load in the desktop (design 30 §11 exfiltration sink); not in the brief, added deliberately. TUI shows `🖼 alt` + caption.
- Callout icons: note/tip/question/important (lucide Info/Lightbulb/CircleHelp/Flag); TUI uses `▍` bar + glyph.
- Long-press affordance for touch: not built; the toolbar is hover + focus-within (desktop app, keyboard reachable).
- Design doc 30's verbatim prompt copy replaced by a pointer to `prompt.rs` rather than re-duplicated.
- Line 7 of the brief says do not run tests; line 18 lists the verification to run. Ran line 18's: `cargo test -p surface --lib`, `cargo test -p app --lib`, `just ts-desktop`, `bunx tsc --noEmit`, vitest on the surface directory, `bun run build`.

## Not built

- TUI rendering of ```openui fences (still a plain code block in the terminal; `surface::fence` handles mermaid/json-render only). Out of the brief's scope (desktop inline); noted for parity.
- No storybook or mock page exists in `crates/desktop/ui`; no screenshots. Renderer output is pinned by `registry.test.ts` markup assertions and the TUI goldens in `crates/surface/tests/golden/`.
- `docs/skills/` does not exist; the visualize and apps skills are generated from `crates/surface/visualize.md` and `crates/app/apps.md` (updated).
- Editor `CodeBlock.tsx` still treats only mermaid/json-render as diagram languages (openui in the editor was never in scope).

## Tests written

- Rust: `validate::tests::heatmap_shape_and_url_schemes`; 17 new valid fixtures + 3 broken ones flow through `valid_fixtures_parse_strictly_cover_the_catalog_and_round_trip`, `located_*`, `valid_fixtures_match_golden_files` (34 new golden files), `every_catalog_example_parses`, `skill_is_generated_and_valid`; catalog/prompt pins updated (36).
- Desktop: `registry.test.ts` (18 tests), `parse.test.ts` (+1), `catalog.test.ts` (36 + parity rows for the new names), `OpenUIFence.test.ts` (inline chrome).
- App: `tool.rs` prompt test updated (Accordion/OptionCards in, Carousel skill-only, inline clause).

## Verification results

- `cargo check -p surface --all-features`: clean.
- `cargo test -p surface --lib`: 35 passed, 0 failed.
- `cargo test -p app --lib`: 39 passed, 0 failed (a first run failed `helper::tests::wrong_hash_or_missing_binary_never_serves` — untouched `helper.rs`, process-spawning test; passed alone and on the full rerun).
- `just ts-desktop`: bindings regenerated (`surface-catalog.json` carries 36 variants).
- `bunx tsc --noEmit` (crates/desktop/ui): exit 0.
- vitest `src/components/chat/surface/{catalog,registry,parse,Graph,FenceFallback,OpenUIFence}.test.ts`: 99 passed, 0 failed.
- `bun run build` (tsc -b && vite build): exit 0.

DONE SURFACES
