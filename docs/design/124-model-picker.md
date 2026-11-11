# 124 — Model picker: provider → model navigation and word search

Status: building (2026-10-07). Surfaces: TUI `/model` picker (`crates/tui`), desktop header picker + compact dialog + meeting start form + model fields in forms (`crates/desktop/ui`). No engine or RPC change: `list_models` and `ModelInfo` stay as they are.

## 1. Problem

Sky (2026-10-07): "The text search is not good, too many results that seem to have very little to do with the search parameter. Also with a lot of models the list is just too much, we need a 2-click setup where the user selects the provider first and then the model. Text search still goes over all the models, but the navigation path is that nested setup."

Mechanisms:

- Desktop: every model picker is a cmdk `Command` with cmdk's `defaultFilter` (command-score), a fuzzy *subsequence* matcher — `gpt` matches "**G**emini **P**ro **T**hinking", `pro` matches anything with a p…r…o in order. With 388 models (Sky's engine today: amazon-bedrock 189, cursor 130, openai 53, anthropic 16) most searches return a long list of unrelated rows. The list itself is one flat group of 388 rows.
- TUI: `filter_models` (`crates/tui/src/model.rs`) is a case-insensitive substring test on `id` and `name` — strict, but unranked, and the list is the same flat 388 rows, 10 visible at a time (`MENU_ROWS`).
- Neither surface offers a provider level.

## 2. Search — one specification, implemented twice

Implemented in Rust (`crates/tui/src/model.rs`, replacing `filter_models`) and in TypeScript (`crates/desktop/ui/src/lib/model-search.ts`, replacing cmdk's filter for model rows). Both carry the §2.3 test vectors verbatim.

### 2.1 Terms

`query.trim().to_lowercase()` split on whitespace, empty pieces dropped. No terms → **browse mode** (§3), no scoring.

### 2.2 Score

Per model, lowercase `name_l`, `id_l`, `provider_l`; `tail_l` = `id_l` after its first `/` (`openrouter/anthropic/x` → `anthropic/x`). A term `t` has a **boundary match** in a string `s` when `s` contains `t` at an index `i` with `i == 0` or `s[i-1]` not in `[a-z0-9]` — i.e. `t` starts at the start of a word, where words are runs of `[a-z0-9]` (so `4.5` matches "claude sonnet **4.5**" at a boundary, `gpt-5.4` matches "**gpt-5.4** mini" at index 0, and `mini` in "ge**mini**" is *not* a boundary match).

Per-term tier, the first that applies:

| tier | condition |
|---|---|
| 100 | `name_l == t` or `tail_l == t` |
| 90 | `name_l` starts with `t` |
| 80 | boundary match of `t` in `name_l` (index > 0) |
| 70 | `provider_l == t` |
| 65 | `provider_l` starts with `t` |
| 60 | boundary match of `t` in `tail_l` |
| 40 | `name_l` contains `t` (not at a boundary) |
| 30 | `id_l` contains `t` (not at a boundary) |
| 0 | otherwise |

**Gating:** for each term, if any model reaches tier ≥ 60, the models at tier 40/30 for that term are treated as tier 0 — plain substring matches only count when nothing matches at a word boundary (so `mini` does not list every Gemini while a `… mini` model exists, but `aude` still finds the Claude rows). A model **matches** iff every term scores > 0 after gating. Model score = sum of its term tiers. Results are ordered by score descending, ties in **catalog order** (the order `list_models` returned: provider, then id) — a stable sort. No fuzzy / subsequence matching anywhere.

### 2.3 Test vectors

Fixture catalog, in catalog order (provider, id, name):

```
amazon-bedrock  amazon-bedrock/amazon.nova-pro-v1:0                         Nova Pro
amazon-bedrock  amazon-bedrock/anthropic.claude-sonnet-4-5-20250929-v1:0    Claude Sonnet 4.5
anthropic       anthropic/claude-opus-4-1                                   Claude Opus 4.1
anthropic       anthropic/claude-sonnet-4-5                                 Claude Sonnet 4.5
cursor          cursor/gpt-5.4                                              GPT-5.4
google          google/gemini-2.5-flash-thinking                            Gemini 2.5 Flash Thinking
google          google/gemini-2.5-pro                                       Gemini 2.5 Pro
openai          openai/gpt-5.4                                              GPT-5.4
openai          openai/gpt-5.4-mini                                         GPT-5.4 mini
openai          openai/o3-pro                                               o3-pro
```

Expected result ids, in order (scores in parentheses):

| query | result |
|---|---|
| `gpt` | cursor/gpt-5.4, openai/gpt-5.4, openai/gpt-5.4-mini (90 each; **not** gemini-2.5-flash-thinking) |
| `pro` | amazon-bedrock/amazon.nova-pro-v1:0, google/gemini-2.5-pro, openai/o3-pro (80 each; not flash-thinking) |
| `anthropic sonnet` | anthropic/claude-sonnet-4-5 (70+80), amazon-bedrock/anthropic.claude-sonnet-4-5-20250929-v1:0 (60+80) |
| `sonnet 4.5` | amazon-bedrock/anthropic.claude-sonnet-4-5-20250929-v1:0, anthropic/claude-sonnet-4-5 (80+80 each, catalog order) |
| `claude` | amazon-bedrock/anthropic.claude-sonnet-4-5-20250929-v1:0, anthropic/claude-opus-4-1, anthropic/claude-sonnet-4-5 (90 each) |
| `mini` | openai/gpt-5.4-mini (80) only — the Gemini rows are gated out |
| `aude` | the three Claude rows in catalog order (40 each; nothing matches at a boundary, so substrings count) |
| `GPT-5.4` | cursor/gpt-5.4 (100), openai/gpt-5.4 (100), openai/gpt-5.4-mini (90) |
| `anth` | anthropic/claude-opus-4-1 (65), anthropic/claude-sonnet-4-5 (65), amazon-bedrock/anthropic.claude-sonnet-4-5-20250929-v1:0 (60) |
| `openai mini` | openai/gpt-5.4-mini (70+80) |
| `nova pro` | amazon-bedrock/amazon.nova-pro-v1:0 (90+80) |
| `v1:0` | amazon-bedrock/amazon.nova-pro-v1:0, amazon-bedrock/anthropic.claude-sonnet-4-5-20250929-v1:0 (60 each) |
| `xyz` | (empty) |
| `  ` | browse mode: no search |

## 3. Navigation — browse mode

Both surfaces open on the **providers** level; typing enters **search** mode over every model; clearing the search returns to the level the user was on.

### 3.1 Providers level

1. **Current** — the session's (or draft's) current model as the first row, marked (desktop: check; TUI: `●`). Never hidden, in every mode (wave 88 rule: the selected model is always on top and cannot be filtered out). Picking it re-selects it (harmless).
2. **Providers** — one row per distinct `ModelInfo.provider` among the listed models, sorted by provider id (A→Z). Label = the provider id exactly as it appears in `provider/model` (users type it that way); right side = `N models`; the provider of the current model carries the same marker. Desktop: a `ChevronRight` at the right edge.
3. Desktop only, amended 2026-11-11 (Sky): the selected model's effort levels are a **chip row directly under its row**, wherever that row shows (Favorites, Current, the provider's list, search), plus a `Default` chip unless a level is required. One chip row per picker; no bottom Effort group. Under a cmdk row (favorite, provider list) the chips are cmdk items (↑/↓, Enter); under the Current or a selected `none` row they are plain buttons (Tab, Enter/Space) so the §3 start row stays the first cmdk item. Phone widths / coarse pointers: wrapping, 36 px chips.

The highlight starts on the first provider row (the Current row sits above it).

### 3.2 Models-of-a-provider level

Entered by picking a provider (Enter/→ in the TUI, click/Enter in the desktop).

- Desktop: the first row is `← All providers` (a `CommandItem`; selecting it goes back); then the group heading `<provider> · N models`; rows = the provider's models **sorted by name** (case-insensitive, then id), the current one checked; right side shows the context window (`200k`) and `vision` when `image`. **Backspace on an empty search goes back** to the providers level. Escape keeps closing the popover/dialog (Radix default).
- TUI: title `model › <provider> (N)  ⏎ set  ←/⌫ back  esc close`; rows as today (`id`, `name`, context, `· vision`) with `●` on the current model, sorted by name then id. `←`, or Backspace with an empty filter, returns to the providers level. Esc closes.

### 3.3 Search mode

Any non-empty search text, on either level: the list becomes the flat §2 result list over **all** models — Current row first (marked, even when it does not match, omitted from the results below so it never appears twice), then results. Each row shows the model name with the provider at the right (desktop) / `id name context vision` (TUI). The highlight starts on the **first result**, not on the Current row, so Enter picks the best match. Empty results show `No model matches.` (desktop) / an empty list under the title (TUI). The desktop keeps the effort chips under the Current row while searching. Clearing the text returns to the remembered level (the provider stays selected).

### 3.4 Unchanged

`/model <id>` (TUI) and `set_model` semantics, the manual-pick-sets-the-default rule (82 §2) and `/model` on an ACP session (harness options, 75 §8) are untouched. Amended 2026-11-11: the desktop ACP picker is `HarnessPicker`, the standard `ModelPicker` over the harness's model options (rows) and its thought_level option (chips, no `Default`); `HarnessCommand`/`HarnessGroup`/`pinnedFilter` are gone. Every desktop model or effort pick renders `ModelPicker` (`ModelMenu`/`ModelDialog`, `ModelCombobox`, `HarnessPicker`); the agent runtime effort and the session ⋯ menu's Effort › moved into the chips.

### 3.5 One provider only

When the listed models come from a single provider, the providers level is skipped: the picker opens on that provider's models level with no `← All providers` row (desktop) and no `←` hint (TUI); search mode works as usual.

## 4. Desktop components

`crates/desktop/ui/src/lib/model-search.ts` (pure, tested): `searchTerms(query)`, `scoreModel(m, terms)`, `rankModels(models, query)`, `providersOf(models) → {id, count}[]`, `modelsOfProvider(models, id)` (name-sorted).

`crates/desktop/ui/src/components/chat/ModelPicker.tsx` (new): the cmdk body described in §3 — `<ModelPicker models model onSelect effort? onEffort? none?>` rendering `CommandInput` (controlled `value`/`onValueChange`), the levels, and the search results; `Command` runs with `shouldFilter={false}` (ranking is ours); the highlighted item is controlled (`value`/`onValueChange` on `Command`) so it lands on the first result after each keystroke. Optional `none={{ value, label }}` renders a first-level row for "Engine default" / "Off" style choices (forms, §5). Optional `extra` ids (an on-file model id missing from the catalog) are listed under an `Other` provider row so a stale value stays selectable.

`ModelMenu.tsx`: `ModelCommand` becomes `<ModelPicker …>` (header popover and `ModelDialog`); the effort group moves inside it (shown on the providers level). `ModelItems`, `orderModels`, `modelItemValue` and the model use of `pinnedFilter` go away (keep `pinFirst`, `pinnedFilter`, `harnessItemValue` for `HarnessGroup`, which is unchanged). `ModelMenu.test.ts` loses the model-pin tests and keeps the harness ones; new tests live in `lib/model-search.test.ts` (the §2.3 vectors) and `ModelPicker.test.ts` (providers/level logic, pure helpers).

`meeting/StartForm.tsx`: the popover body becomes `<ModelPicker models model onSelect>`.

## 5. Desktop forms (same wave, after §4 is green)

A `ModelCombobox` (`components/chat/ModelCombobox.tsx`: trigger `Button variant="outline"` showing the current name or the `none` label, `Popover` + `ModelPicker`) replaces the Radix `Select` over all models in:

- `routines/RoutineDetail.tsx` Model (`none` = "Engine default"; an unknown on-file id stays selectable),
- `agents/RuntimeAccessFields.tsx` Model (`none` = "Node default"),
- `settings/Loop.tsx` — every model `Select` (`none` = "Off" where the select has an `Off` row; the vision select keeps its `image`-filtered list; an `unavailable` id stays selectable).

Semantics of the stored values are unchanged (same ids, same `null`/`off` sentinels). If a form's Select carries behaviour the combobox cannot express, leave that one Select and say so in the as-built note.

## 6. Gates

- TUI: `cargo fmt -p tui`, `cargo clippy -p tui --all-targets -- -D warnings`, `cargo test -p tui` (new unit tests: the §2.3 vectors, `providers`, level transitions in `models_key`, the updated `menu_box_is_tight_at_any_width`).
- Desktop: `bun run check` (tsc), `bunx vitest run src/lib src/components/chat src/components/meeting src/components/routines src/components/agents src/components/settings`, then one full `bun run test`.

## 7. Out of scope

Engine-side provider display names (`ProviderInfo.name` is only the gateway prefix), recents/favourites, the ACP harness picker, Slack/Telegram `/model`.