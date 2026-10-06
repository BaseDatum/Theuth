# 110 — As built: SLACK-BLOCKS-RENDER (`surface::slack_blocks`)

Branch `feat/110-blocks`, cut from `a53e378d`. Design 110 §8 (mapping table, security), §13 item 5.
Renderer only; `crates/slack` untouched — the bubble integration (`Chunk::Blocks`, fallback to
`flatten`) is wave 2.

## Files

| file | what |
|---|---|
| `crates/surface/src/slack.rs` | `slack_blocks(fence_json) -> Result<Vec<Value>, SlackBlocksError>` over `parse` (the tree `flatten`/`render` use). `SlackBlocksError { Invalid(Vec<SpecError>), Unsupported(&'static str), TooManyBlocks(usize) }`. `Render { spec, charts }` walks the tree with a `nested` flag (inside a `container`); per-block helpers `alert`, `table`, `fields`, `metric`, `container`, `header`, `section`, `context`, `list`, `preformatted`, `clip`. Pub consts `MAX_BLOCKS = 50`, `MAX_TABLE_ROWS = 15`; private caps from the Block Kit reference (header 150, alert 200, section 3000, field 2000 ×10, container title 150 / 10 children, chart title 50 / label 20 / 20 points / 2 charts per message, table 20 columns / 20 000 chars / 2000 per cell, image URL 3000 / alt 2000). Tests in `mod tests`. |
| `crates/surface/src/lib.rs` | `pub mod slack;` and `pub use slack::{SlackBlocksError, slack_blocks};` — nothing else. |
| `crates/surface/fixtures/slack/<name>.json` + `<name>.blocks.json` | 15 goldens: `alert`, `table-small`, `table-large` (fallback), `bars`, `sparkline`, `metric-grid`, `keyvalue`, `checklist`, `steps`, `timeline-tags-glossary`, `card-collapsible`, `heading-divider-image`, `linklist-code-quote`, `unsupported-graph` (error), `too-many-blocks` (error). Errors are recorded as `{"error": "<Display>"}`. `UPDATE_GOLDEN=1 cargo test -p surface slack::` regenerates (the 30 §10 convention). |

No `crates/surface/tests/slack_blocks.rs`: the crate keeps tests in-module (`render.rs`, `validate.rs`), so `slack.rs` does too; `tests/` holds goldens only.

## Judgement calls

- **Container children are restricted by Slack** (`context`, `divider`, `header`, `image`, `rich_text`, `section`, plus actions/file/input/table/video). So inside a `container`: a nested `Card`/`Accordion`/`Tabs` hoists its children under a `header` (+ a `context` line for the description); `Table` and `Bars`/`Sparkline` take their text form (`rich_text_preformatted`); `Alert`/`Callout` become a `section` (mrkdwn, 3000 cap). Containers never nest.
- **A `container` needs a title.** `Card` without `title` (and `Stack`/`Grid`, which have none) hoists its children inline at the current level. A titled `Card` with no renderable children emits `header` + optional `context` instead of an empty container. Containers with more than 10 children split into several containers carrying the same title (and subtitle / collapsed flags).
- **`Accordion` → one `container` per pane** (`titles[i]`, `is_collapsible: true`, `default_collapsed: true`). `Tabs` map the same way with `labels[i]` (design §8 lists Tabs under flattening; the mapping is free and a strict superset).
- **`Alert` level**: `type` maps 1:1 (`info` when omitted, the catalog default). `Callout` has no severity variant, so `level` is omitted (Slack's `default`), per the design's "else `default`". Alert text is `*title*\nmessage` clipped at 200.
- **`Table`**: `data_table` with `caption: "Table"` (required by Slack, the component has no title), header cells `raw_text`, body cells `raw_number {value, text}` when the cell parses as a finite number (numeric sort for free) else `raw_text`. Falls back to the TUI table (`markdown::layout_table` + `render::plain_text`) as `rich_text_preformatted` when: nested, 0 rows, > 15 rows, > 20 columns, any cell > 2000 chars, or > 20 000 chars in total. Empty cells render `—` (Slack cells have `minLength: 1`).
- **Charts**: one series named `Value`; `axis_config.categories` are the point labels after clipping to 20 chars, so the label-matching rule holds by construction. `Bars` title `Values`; `Sparkline` title = `label` or `Trend`, points labelled `1..n`. Falls back to a `label: value` preformatted listing when: nested, no points, > 20 points (Bars), clipped labels collide, or the message already has two `data_visualization` blocks. `Sparkline` over 20 values shows its last 20 (`ponytail:` comment). `Metric.spark` is not drawn.
- **`Metric`/`MetricGrid`/`KeyValueList`** → `section.fields` of mrkdwn `*label*\nvalue ↑ +12%` (trend glyphs ↑ ↓ →), 10 per section, split beyond.
- **`Text`**: `body`/`lead` → `section` mrkdwn (3000); `muted`/`caption` → `context`; `code` → `section` with a fenced block. The `markdown` block is not used (not allowed inside containers; one path everywhere). `Badge` → `context` with `` `text` ``.
- **Lists**: `Checklist` (`✓`/`○`), `Timeline` (`✓`/`▶`/`○`, `when — *title* — detail`), `Glossary` (`*term* — text`) are `rich_text_list` bullet; `Steps` ordered with bold title; `LinkList` bullet of `link` elements with ` — note`; `Tags` one `context` line `` `a` · `b` ``; `Code` → optional bold title section + `rich_text_preformatted`; `Quote` → `rich_text_quote` with an italic ` — source`.
- **`Image`**: `http(s)://` only (case-insensitive scheme). A `data:image/` src — which the validator accepts for the desktop — or a URL over 3000 chars is `Err(Unsupported("Image"))`, so the fence flattens; the brief said "http(s) and data:image", the design's security note (§8, 30 §11) says no `data:` images since Slack fetches them; the design wins. `caption` → `title`.
- **Clipping**: `clip(s, cap)` keeps `cap - 1` chars + `…` by `char` count (Slack counts characters, roughly); the empty string becomes `—` (every text object has `minLength: 1`).
- **Key order** is `json!` insertion order (the workspace enables `serde_json/preserve_order`); appended optional keys (`subtitle`, `level`, `is_collapsible`) come last. No `block_id`s, no timestamps.
- **Error over 50 blocks** counts top-level blocks only (container children do not count); checked after the walk so `Unsupported` wins when both apply.
- Unsupported set exactly as the brief: Graph, Sequence, Heatmap, Kanban, Calendar, Gauge, Excerpt, Progress, Person, FileList, Tree, Diff.

## Contract / seam changes

None outside the owned files. `flatten.rs`, `render.rs`, `validate.rs`, `catalog.rs`, `spec.rs`, `prompt.rs` untouched — `render::{plain_text, span}` were already `pub(crate)` and `markdown::layout_table` is public, so no accessor was needed. `crates/slack` untouched.

## Gates

Run with `PATH=$HOME/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin:$PATH CARGO_TARGET_DIR=/Users/Sky.Moore/Develop/theuth-110-target`:

```
cargo check -p surface --all-targets                 → Finished `dev` profile [unoptimized + debuginfo] target(s) in 18.65s
cargo test -p surface                                → test result: ok. 57 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.09s
                                                       test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
cargo clippy -p surface --all-targets -- -D warnings → Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.55s
cargo fmt -p surface -- --check                      → (no diff, exit 0)
```

## Tests added

`surface::slack::tests::{fixtures_match_golden_files, container_children_cap_at_ten, nested_card_hoists_under_a_header_and_nested_table_is_text, fields_split_at_ten_per_section, urls_are_http_only, strings_clip_at_slack_caps_with_an_ellipsis, third_chart_and_long_series_fall_back_to_text, invalid_body_is_an_error}`.

Design §13 item 5 names `table_over_fifteen_rows_falls_back_to_preformatted`, `unmapped_component_is_an_error_so_the_caller_flattens` and `over_fifty_blocks_is_an_error`: those are the `table-large`, `unsupported-graph` and `too-many-blocks` goldens rather than separate functions (same assertion, one mechanism).

## Deviations from design 110

- §8 table: `Stack`, `Grid`, `Tabs`, `Badge`, `Text` are mapped (brief: Stack/Grid → container; here Stack/Grid hoist because a container needs a title; Tabs as Accordion; Badge → context; Text → section/context), not flattened. Any fence containing only mapped components renders; the unsupported list is the brief's twelve.
- §8 "Card, Accordion → `container` … children mapped recursively": recursion hoists under a `header` instead of nesting containers (Slack forbids container-in-container and `data_table`/`alert`/`data_visualization` inside one).
- §8 `Text → markdown block or section`: always `section` (or `context` for muted/caption).
- §8 Image: `data:image/` refused (`Unsupported`), per the design's security paragraph over the brief's wording.
- Alert blocks are documented as "currently only supported in modals" (docs.slack.dev alert-block). Rendered as the design asks; the §14.5 `blocks.validate` spike decides whether `alert` stays or wave 2 swaps it for a `section`. The `alert` goldens are the ones to re-record.
- §13 item 5 test names: three realised as goldens (above).
