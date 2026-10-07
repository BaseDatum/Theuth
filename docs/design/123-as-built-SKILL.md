# 123 — as built, W4 (the visualize skill, the prompt table, the measurement)

Branch `feat/123-w4-skill`: `5b0ce1ef` (skill, prompt rows, board.md, apps.md, apps test), `24de818f` (measurement, re-tune), then this note.

## Built where
- `crates/surface/visualize.md` — after "Choose the block" (lifecycle/schema rows → `StateMachine`/`Entities`; rows for Swimlane, GraphDelta, Gantt, Bar/Line, Scatter/Quadrant, Sankey/Waterfall/Treemap/Funnel, Fishbone/Loop/Layers, Journey/Venn; mermaid keeps `mindmap`): **Pick the diagram** (44 rows), **Behavioural triggers** (9), **Budgets**, **Taste gate**, **Which fence** (with the recharts-name caveat). json-render rules gain one cross-reference line; `ref` covers every diagram unit. Worked examples: mermaid Lifecycle/Sequence replaced by six json-render examples (StateMachine, Entities, Swimlane, Gantt, GraphDelta, Sankey).
- `crates/surface/src/prompt.rs` — `BLOCK_TAIL`: mermaid line drops state/class/ER/gantt/timeline; 8 new rows, 2 rewritten (lifecycle, schema), 2 adjusted (Timeline `scale: time`; numbers row loses "openui BarChart/LineChart"); Plan recipe → Gantt when dated; Investigation `state: "danger"` (was `"error"`, not in `GraphState`); one **closed-vocabulary** line (the re-tune). Tests: `skill_is_generated_and_valid` asserts `< 48 KB` and the six examples; new `skill_routes_every_taxonomy_type` pins 44 rows + 9 triggers. `EXAMPLES` untouched.
- `crates/surface/examples/prompt_block.rs` — `cargo run -q -p surface --example prompt_block [block|catalog|skill|check]`; `check` validates a reply from stdin (the runner's `--validate`).
- `crates/document/board.md` "One diagram per idea" — table rerouted (lifecycle → `StateMachine`, schema → `Entities`, handoffs → `Swimlane`, change map → `GraphDelta`, dated plan → `Gantt`, mermaid only un-anchored) + the §7 paragraph.
- `crates/app/apps.md` — "When a diagram is an app" (app vs `openui` fence vs json-render; positional order = catalog + `highlight`, `onSelect`; recharts-name caveat) + four recipes: Gantt + Run-now, StateMachine with live state, Sankey spend, inbox as **Swimlane** with a move per card. `packages/apps/test/apps-md.test.ts` runs every complete ```openui program in apps.md through `check()` (8 programs, 9 tests).
- `scripts/123-steering/` — `prompts.json` (20 grammar: 5 lifecycle, 5 schema, 2 plan + 3 handoff, 5 quantity; 10 hold), `run.ts` (Anthropic Messages API, system = one role line + Guidance sentence + the real block; counts picks, `--validate` first-pass validity, exit 0 at ≥ 16/20 ∧ ≤ 1/10), `results/*.json`.

## Deviations
1. **Kanban recipe → Swimlane**: `Kanban` is not an apps component (library.ts registers 15; Kanban/Heatmap/Timeline/Tree are json-render only). Lanes = states, `highlight` = blocked ids, `Select` + `Button` → `task_move`.
2. **No `onSelect`/`$value` in the recipes**: `check.ts` scopes `$value` only inside `Graph`'s `onSelect` (`check.ts:272-278`); on the 15 new components it is a `state-undeclared` error. Recipes use `highlight` from live data and a `Select`-bound `$state` for the action (parser-validated).
3. **Tool names are placeholders with stated result shapes** (`plan_status`, `task_state`, `session_cost`, `task_board`): OpenUI Lang has no `@Map`/`@Now`, and `schedule_status`/`task_inbox` return text or `ScheduleInfo` rows, not component-shaped rows. The paragraph says so and points at apps.md item 4 (an MCP tool of yours).
4. **Prompt rows**: 8 new + 2 rewritten + 2 adjusted (above) + 1 vocabulary line; design said "≤ 10 new, 2 rewritten".
5. **board.md**: the whole routing table, not one paragraph — its old rows routed lifecycles to mermaid and schema changes to `Table` + `Graph`.
6. **Mermaid worked examples removed** (Sequence, Lifecycle) for the byte cap; mermaid styling/rules stay.

## Measured
- `catalog_compact()`: 1,405 chars / 38 (design) → **3,531 chars / 57** (C's tree; W4 adds none) ≈ 950 tokens. `prompt_block()`: 10,350 → 11,110 (rows) → **11,586** (vocabulary line); +1,236 chars ≈ +310 tokens. Skill: 32,700 B (C) → **48,036 B** (cap 49,152).
- Steering (`claude-opus-5-5`, the configured subagent model; `openai/gpt-5.6-sol` unreachable from this shell). **Before** (C's block, old rows): 10/20 (lifecycle 0/5, schema 0/5 → mermaid; plan 2/2, handoff 3/3, quantity 5/5 from the catalog alone), 0/10 false draws. **After rows**: 20/20, 0/10, but first-pass valid 12/30 (`key: "PK"`, `kind: "one-to-many"`, `state: "error"`, `direction: "horizontal"`, Callout `variant` outside `props`). **Re-tune** (vocabulary line + `state: "danger"`): **20/20, 1/10** (a `Quadrant` beside a comparison `Table`), **29/30 first-pass valid** (`kind: "abstract"`). Bar met; `scripts/123-steering/results/`.

## Gates
`cargo test -p surface`: 64 passed, 0 failed (`cargo fmt -p surface --check` clean) · apps `bun run check`: 0 errors · apps `bun test`: 147 pass / 0 fail (one run under two busy build slots: 146/1, not reproduced, not in `apps-md.test.ts`).

## Open items
- **Closer / `packages/apps/src/check.ts:272-278`**: generalise `valueScoped` from `n.name === "Graph"` to every name in `DIAGRAMS` (`diagrams.ts`), `onSelect = mappedProps?.onSelect ?? last positional arg`; then the recipes can gain `onSelect: Action([@Set($pick, $value)])` and `apps-md.test.ts` still passes.
- **Closer / integrator**: the baseline block for a true pre-123 "before" is `git show 78f192f9:crates/surface/src/prompt.rs`; rerun `bun scripts/123-steering/run.ts --validate` on the integration tree (`cargo run --example prompt_block` picks up W1–W3's merged catalog) and against `openai/gpt-5.6-sol` where credentials exist.
- **Validator wording (not mine)**: `Entities.kind` has no `abstract`; `Table` rows must be strings — the vocabulary line covers both now, but a `Table` cell `number` coercion in `validate.rs` would remove the commonest remaining first-pass error.
- `visualize.md` has 1.1 KB under the cap; the next grammar's prose goes to the design doc, not the skill.
