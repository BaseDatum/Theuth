# 128c as-built — SPEND-LEDGER (the HUD's SPEND panel reads the model-call ledger)

Branch `feat/128c-spend-ledger`, worktree `theuth-128c-spend`, cut from main `0c605cc7`; `origin/main` (`7ef01dfd`, design 128b)
folded before the UI step. Brief: `theuth-128-briefs/brief-SPEND-LEDGER.md`. Design: `docs/design/128-hud-scene.md` →
`## Revision 3 — SPEND reads the model-call ledger`.

Sky, 2026-10-07 15:21 — "one other issue I noticed is that the spend shows no data in the HUD, but for sure there is cost data in
the db right? what gives?"

## Cause (as found)

`SpendPanel` summed `AgentSummary.spent_month`, which the roster reads from `roster.sqlite` `budget`; `roster::charge()` writes that
table only for agents with `Budget::Monthly`. No agent had a cap → 0 rows → every row filtered out → empty panel. The ledger that
has the money is `sessions.sqlite` `model_calls` + `cost_owners`.

## Engine (commit `d0f79298`)

| file | change |
| --- | --- |
| `crates/agent/src/cost.rs` | `SpendDay`, `SpendOwnerKind` (`agent \| user \| routine \| other`, snake_case), `SpendOwner`, `SpendReport` — serde + schemars + ts-rs like the other cost types |
| `crates/agent/src/lib.rs` | re-exports; `SpendReportQuery(pub u32)` (`ask` → `Result<SpendReport, String>`) |
| `crates/agent/src/store/accounting.rs` | `SpendRow` + `SpendRow::usd()` / `owner()`, `fold_spend()`, `Store::spend_report(now_ms, days)`: one read (`model_calls ⟕ cost_owners ⟕ sessions` on `root_id`, `created_at >= since − 1 day`), local-day bucketing with `chrono::Local` in Rust, month to date, owners top 8 (`usd` desc, `calls` desc, name) |
| `crates/agent/src/supervisor.rs` | `impl Message<SpendReportQuery> for Agents` on the durable store (`self.store`, not `store_for`: private trees are out) |
| `crates/rpc/src/api.rs` | `#[method(name = "spend_report")] async fn spend_report(&self, days: u32)` (no `with_extensions`, like `list_sessions`) + impl |
| `crates/rpc/src/export.rs` | the four types exported → `SpendReport.ts`, `SpendDay.ts`, `SpendOwner.ts`, `SpendOwnerKind.ts`; `methods.json` gains `spend_report` |

Rules as built: `days.clamp(1, 31)`; `not_dispatched` skipped; `usd = returned ?? estimated` when the currency is `USD`
(another currency → counted in `calls`, `Some(0.0)` money); no money → `unresolved`; day > today (clock skew) skipped.
Attribution by the root's `sessions` row: `desk` → `agent` (equals the roster's `agent_of` for every session in the desk's
tree, durable across restarts); `unattended = 1` on a root → `routine` named by the title before ` · ` (`routine` when the title
is empty); else `user` folded into one `you` row; root row gone (or owner never registered) → one `other` row.

Tests: `store::accounting::tests::spend_report_buckets_days_month_and_owners` (today / yesterday / 40 days ago / EUR / no money;
7 day strings oldest→today; today `$3.35`, 6 calls, 1 unresolved; yesterday `$0.50`; month total conditional on yesterday's
month; owners `[agent builder 2.0, user you 1.5 ×4, routine nightly 0.25, other 0.1]`; clamp 0→1 and 99→31) and
`api::tests::spend_report_is_registered_and_round_trips` (method registered, `[7]` reaches the actor lookup → `NOT_RUNNING`,
`SpendReport` JSON round-trip with `kind: "agent"`).

Gates (under `build-lock`, `CARGO_TARGET_DIR=$PWD/target`, 4 jobs, incremental off): `cargo test -p agent -p rpc` — agent 451
(+ the new one), rpc 50 + 1, prompts 1, all ok · `cargo clippy -p agent -p rpc --all-targets -- -D warnings` clean ·
`just ts-desktop` ok (576 binding files after the fold).

## Fold

`git merge --no-edit origin/main` (`7ef01dfd`): four conflicts, all additive at the same insertion points (`queries.ts` hooks,
`fixtures.ts` return + locals, `scene/model.ts` `SceneData` fields, `model.test.ts` import list) — both sides kept. Merge commit
`9896e5c2`. Bindings regenerated on the merged tree.

## UI (commits `42225c4f` pre-fold additive pieces, then the panel commit)

| file | change |
| --- | --- |
| `rpc/api.ts` | `spend_report(days): Promise<SpendReport>` + `'spend_report'` in `METHODS` (sorted after `speak`) |
| `components/hud/queries.ts` | `SPEND_KEY = ['spend', 7]`, `useSpendReport()` — `staleTime` 20 s, `refetchInterval` 60 s, invalidated on `ModelUsageChanged`, `?sim` → fixture |
| `components/hud/Hud.tsx` | `useSpendReport()` → `sceneData.spend`; `'spend'` in the error list |
| `components/hud/scene/model.ts` | `SceneData.spend?`, `localDay`, `fmtUsd`, `SpendBar` / `SpendOwnerRow` / `SpendPanelModel`, pure `spendPanel(report, agents, now)`; `SceneModel.spend`; `footer(…, ledgerMonth?)` so `THIS MONTH` shows the ledger's month once loaded |
| `components/hud/scene/Scene.tsx` | `SpendPanel({ p })` rewritten: `TODAY $x` meta, `$month` big number → `/agents`, seven bars (today `--status-info-fg`, else `--fg-secondary`, `$0` = 1-unit hairline), weekday letters, ≤ 4 owner rows with glyph + link (`ownerTarget`), cap rows only for budgeted agents as far as they fit above y 212, share bar success / primary ink past 85 % (no carnelian); `Panels` passes `m.spend`; footer uses `fmtUsd` |
| `components/hud/scene/SceneCompact.tsx` | `div[aria-label="Spend"]`: month, today (faience), top two owners (linked where a route exists) above the panels |
| `components/hud/scene/fixtures.ts` | `spend`: a week ending today (`[412.1, 388.4, 540.9, 0, 611.2, 849.0, 1302.38]`), month `$4,102.00` / 36,861 calls, five owners (you, builder, research, nightly-digest, ops-desk) |
| tests | `model.test.ts`: `spendPanel` labels `tfssmtw` for 2026-10-07, today flag, `$0` bar, month/today, owners capped at 4, caps only with a cap (`docs-desk, builder, research, ops-desk`), >7-day report keeps the last 7, no report = seven `$0` days, `localDay`, `fmtUsd`; footer `month` now `4102`. `Scene.test.ts`: new Revision 3 test — meta, big number, 7 `rect`s width 28 (one faience; today `[40, 52]`, the `$0` Saturday `[91, 1]`), label order, `you` unlinked at y 124, `builder` → `/agents/builder` with the ring dot, `nightly-digest` → `/routines/nightly-digest` with the square, fifth owner cut, `docs-desk cap` row, `analyst` absent, no `--fg-accent` / `--status-danger-fg` in the panel; compact line; empty data = seven hairlines, `TODAY $0.00`, no owner rows. Footer pin `$31.20 THIS MONTH` → `$4,102.00 THIS MONTH` |

Gates (under `build-lock`, from `crates/desktop/ui`): `bun run check` exit 0 · `bunx vitest run src/components/hud` 4 files,
41/41 · `bunx vitest run src/rpc` 8/8 (`METHODS` vs `methods.json`) · `bunx vite build` ok (the usual >500 kB chunk warning).

Visual smoke: the dev UI needs an engine (it shows the pairing screen), so the fixture scene was rendered to static HTML with the
tests' router mock and painted into the themed page in a headless Chromium — the SPEND panel shows seven bars with Wednesday in
faience and Saturday's `$0` as a hairline, `you / ○ builder / ○ research / ■ nightly-digest` rows, the `docs-desk cap $2.9 / $3`
row with an ink share bar, `TODAY $1,302.38` and `$4,102.00`; the compact line reads `$4,102.00 month · $1,302.38 today · you
$2,210.50 · builder $1,104.20`. The throwaway test and HTML were removed.

## Deviations from the brief

- **Four binding types, not three**: `kind` is a Rust enum `SpendOwnerKind` (→ `SpendOwnerKind.ts`), which is what ts-rs emits
  for `"agent" | "user" | "routine" | "other"`; the wire shape is exactly the brief's.
- **No roster call for attribution**: `sessions.desk` (the durable desk mark the roster writes when it opens a desk) gives the
  same answer as `agent_of` at the root, inside the one SQLite read — no actor hop from the rpc handler.
- **Routine kind** uses the only durable marker there is: `sessions.unattended` on a ROOT (the scheduler is the sole opener of
  unattended roots; subagents are unattended but have a parent). The name is the schedule's title before ` · `. There is no
  `origin`/`schedule` column on `sessions`.
- **`user` roots folded into one `you` row** (the brief's permitted option), `other` likewise one row.
- **Footer `THIS MONTH`** reads the ledger's month when the report has loaded (not asked; the chrome would otherwise contradict
  the panel it sits above). Falls back to the roster sum until then.
- **Cap rows past 85 %** are primary ink, not carnelian: the brief forbids carnelian anywhere in this panel (D12's old threshold
  belonged to the retired panel).
- **Private trees** are not in the report (durable store only) — documented, not asked.
