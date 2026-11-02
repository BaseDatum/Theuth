# 160 — COSTS: a full-history cost & usage-limits dashboard

Status: building (2026-11-02). Owner: Sky. Worktrees: `~/Develop/theuth-160-eng` (feat/160-eng), `~/Develop/theuth-160-lim` (feat/160-lim), `~/Develop/theuth-160-ui` (feat/160-ui); integration `~/Develop/theuth-160` (integrate/160).
Mock (approved 14:27 "looks good, build it, separate worktree"): `~/.theuth/briefs/2026-11-02/160/costs-mock.html` (real ledger numbers; illustrative limit values).
Recon: `~/Develop/theuth-labs/160-costs/recon-ledger.md`, `~/Develop/theuth-labs/160-costs/recon-limits.md`.

## 0. The ask (Sky, verbatim)

> we need a full costs dashboard feature that shows usage limits for all providers, full cost history for all the information in the database and all new sessions. estimated and known. with totals and graphs, filters, the works, It needs to be easy for a user to understand their hourly, daily, weekly, monthly, and custom range costs, which models, which providers, etc. I want a beautiful modern high quality highly functional cost dashboard in the theuth app. this cost dashboard should be accessible with a little button next to the word spend on the main dashboard.
> design it, build it, make it very good, do not hand roll the charts, find a high quality charting library. shadcn@latest add chart would probably do, but if you know of a better one, go for it.

> oh and it needs to be high performance. there should never be any loading or lagging

> (on the mock) looks good, build it, separate worktree.

## 1. Decisions

- **D1 Route and entry points.** New route `/costs` (`crates/desktop/ui/src/routes/costs.tsx`). Entry: a small chart icon button immediately right of the word `SPEND` in the HUD SPEND panel header (`components/hud/scene/Scene.tsx` `SpendPanel` → `Panel`), linking to `/costs`; the panel's big month number links to `/costs` too (was `agents`). No new sidebar rail row (Sky's 97/143 rail rulings keep the rail short); if the search/command palette lists pages, add `Costs` there. Hover/focus on the button preloads the route (`preload="intent"`).
- **D2 Charts: shadcn/ui chart (Recharts v3).** `bunx shadcn@latest add chart` (adds `components/ui/chart.tsx` + `recharts`), and `bunx shadcn@latest add calendar` for the custom-range picker (react-day-picker) inside the existing `popover`. Every chart on the page is a Recharts chart wrapped in `ChartContainer`/`ChartTooltip`/`ChartLegend` — bars, areas, lines, the donut (Pie), the limit gauges (RadialBar) and the hour × weekday heat grid (Scatter with a square `shape`). No hand-drawn SVG charts. Series colours come from new theme tokens `--chart-1..8` in `styles.css` (both `:root` bone and `.dark` soot), mapped onto the mineral palette: faience, sand, lapis, malachite, plum, slate, nile, umber. Gold-ochre (needs-you / warning) and carnelian (error / over limit) are never series colours.
- **D3 One hourly cube of the whole ledger, sliced in the client.** The engine keeps, in memory, the entire cost history aggregated to UTC hours × (root session, provider, model, kind, via, basis) with summed measures (§3). On Sky's ledger that is ≈ 3.5 k rows for 142,858 calls (7 weeks). The UI receives the cube once (columnar JSON), then live deltas, and computes every number, chart, breakdown and filter in memory. Nothing on the page waits on the engine after the first snapshot.
- **D4 Time.** A call is bucketed by the UTC hour of `model_calls.created_at` (reserve time). The client maps hours to local hours/days/ISO weeks (Monday)/months in the browser's time zone (= the engine's on the desktop). Half-hour time zones get hour-precision day boundaries (documented, accepted).
- **D5 Basis (estimated and known).** Every dollar carries one of four bases, shown everywhere as a legend and filter: **Known** (a provider-returned price, `returned_*`), **Reported** (an external harness reported money, e.g. omp remote meters), **Estimated** (theuth's price table × tokens, `estimated_*`), **Unpriced** (a call with no money: count only). Exact mapping in §2.
- **D6 Owners** exactly as `spend_report` (128 R3): root `sessions.desk` → agent `<desk>`; root `unattended = 1` → routine named by the title before ` · `; else `you`; root row gone → `other`. Plus `project` from the root's `sessions.project`.
- **D7 Live.** New sessions and calls appear within ~1 s (≤ 15 s worst case): the engine publishes a debounced `CostCubeChanged { epoch, rev }` event; the UI fetches the delta. Usage limits are an in-memory engine snapshot the UI reads every 15 s while `/costs` is visible, kicking the engine's pollers on mount (never a provider call on the RPC path).
- **D8 Never loading, never lagging** (§6): prefetch at app start, IndexedDB snapshot for a cold start, route chunk preloaded at idle, synchronous in-memory queries (< 4 ms on 50 k rows), no chart animations on update, ≤ 400 buckets per chart, server-paged ledger with previous rows kept, no spinners or skeletons anywhere on the page.
- **D9 URL state.** Range, granularity, group-by, metric, compare and filters live in validated search params so back/forward and reload restore the view.

## 2. What the cube counts (recon-ledger.md, verified on Sky's ledger 2026-11-02 14:30)

| leg | source | time | basis |
|---|---|---|---|
| native | `model_calls` (every attempt whose `status <> 'not_dispatched'`), owner via `cost_owners.root_id` | UTC hour of `created_at` | `returned_nanos` → known · `estimated_nanos` → estimated · neither → unpriced (counted, $0) |
| native, repriced | the 764 settled calls with tokens but no money (2026-09-19 → 09-21, before `eed1e31a`), priced at read time from `crates/models/data/models.json` (≈ $111.20); never written back | same | estimated (+ `repriced` count) |
| reported | `remote_meters.attributed_nanos` (omp/opencode; $5,942.55 over 519 meters), placed in time by the recon §2.4 algorithm: report increments from `source_metadata.reports` (exact, 83.7 %), the pre-window remainder token-weighted over the owner's `remote_turns` (6.4 %), else the last report hour (9.9 %); money-conserving per meter | placed hour | reported (+ `approx_nanos` for the non-exact part) |
| reported, bench | `autoresearch.sqlite samples.spend_nanos` by `samples.at` (0 rows today; include when non-empty) | `samples.at` | reported, via `bench` |

**Excluded** (double counts or not money): `endless_calls` (row-for-row detail of `model_calls kind='endless'`, identical $369.29); `remote_turns` as money (same usage as the meters; its tokens ARE added to the reported cells' token measures); `session_cost`/`SessionCost` roll-ups; roster `budget.spent`; dreaming/autoresearch derived spend; `not_dispatched` rows.

**Model** = `requested_model` (`served_model` is NULL on every row), with a canonical display name from the catalog (aliases such as `claude-haiku-4-5-20251001` → `claude-haiku-4-5`); reported rows take `remote_turns.model` (else `sessions.harness_model`, else `unknown`) with any `:effort` suffix stripped. **Provider** = `model_calls.provider`; reported rows take the model's provider prefix. **Via** = `theuth` for native rows, the harness name (`omp`, `opencode`, `bench`) for reported rows. **Kind** = the `CallKind` string; reported rows `harness`.

**History rules.**
- **H1 Root delete keeps the books.** Today `delete_session` on a root erases its ledger (`store.rs#L2867-L2893` → `accounting.rs#L155-L186` `delete_root_ledger_in`), contradicting the doc comment "Spend stays"; ≥ 3 owners are already gone. From 160 on a root delete tombstones the subtree's `cost_owners` (`deleted_at`) like a child delete, keeps `model_calls` / `remote_meters` / `remote_turns`, and writes one `cost_root_tombstones` row (root id, desk, unattended, routine name — the title prefix only when unattended — project, created_at, deleted_at) in the same transaction. The deleted chat's free-text title is not kept; the dashboard shows "Deleted session · <date>" under its owner.
- **H2 Meter reports are kept.** Each applied remote meter report also appends a `remote_meter_reports` row (meter id, cumulative nanos, currency, arrival time) so future placement is exact beyond the 8-report window.
- **H3 Pre-ledger sessions** (86 sessions 2026-09-13 → 09-18, `history_coverage = 'unknown'`) have no usage stored anywhere; they are not estimated. The page states it: "Cost records start 19 Sep 2026 · 86 earlier sessions have no usage data".
- **H4 One engine's books.** The ledger is engine-local (not replicated over the mesh); the band names the engine.
- **H5 HUD agrees.** `spend_report` (HUD SPEND) includes the reported leg from 160 on, so the HUD's today / month / 7-day numbers equal the dashboard's for the same windows (today the HUD omits $5,942.55, 24.6 % of all money).
- **H6 Unpriced is a lower bound.** Failed and cancelled attempts have no tokens; a cancelled stream may have been billed upstream. The Basis card says so.

## 3. Engine: the cost cube

### 3.1 Shape (frozen contract; Rust in `crates/agent/src/costs/types.rs`, exported with the crate's existing `#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]` + `schemars::JsonSchema` pattern)

```rust
/// Where a cell's money comes from (160 D5).
#[serde(rename_all = "snake_case")]
pub enum CostBasis { Known, Reported, Estimated, Unpriced }

/// A cube state the client already holds.
pub struct CostCubeCursor { pub epoch: String, #[ts(type = "number")] pub rev: u64 }

/// One root session of the ledger, live or deleted — the cube's session dimension.
pub struct CostRoot {
    pub id: String,
    /// The root's title; "" for a deleted root (H1).
    pub title: String,
    pub owner_kind: SpendOwnerKind,     // existing enum: agent | user | routine | other
    /// Agent name, routine name, "you" or "other" (spend_report's rules, D6).
    pub owner: String,
    pub project: Option<String>,
    pub deleted: bool,
    #[ts(type = "number")] pub created_at_ms: u64,
}

/// A dimension entry with its stable index (indices never change within an epoch).
pub struct CostRootEntry { pub ix: u32, pub root: CostRoot }

pub struct CostModel {
    /// Raw id as recorded (`anthropic/claude-haiku-4-5-20251001`).
    pub id: String,
    /// Canonical display name (`claude-haiku-4-5`); aliases share it.
    pub name: String,
    pub provider: String,
}

/// Columnar cells: every array has the same length; index i is one cell =
/// one (hour, root, model, provider, kind, via, basis) combination.
pub struct CostCells {
    /// UTC epoch hour of the bucket = floor(unix_seconds / 3600).
    pub hour: Vec<u32>,
    pub root: Vec<u32>,       // index into roots
    pub model: Vec<u32>,      // index into models
    pub provider: Vec<u32>,   // index into providers
    pub kind: Vec<u32>,       // index into kinds
    pub via: Vec<u32>,        // index into vias
    /// 0 known · 1 reported · 2 estimated · 3 unpriced.
    pub basis: Vec<u8>,
    #[ts(type = "Array<number>")] pub nanos: Vec<u64>,
    pub calls: Vec<u32>,       // attempts (native) or turns (reported)
    pub failed: Vec<u32>,      // failed + cancelled + unresolved attempts
    pub repriced: Vec<u32>,    // attempts priced at read time
    #[ts(type = "Array<number>")] pub input: Vec<u64>,        // input tokens INCLUDING cache read and cache write
    #[ts(type = "Array<number>")] pub output: Vec<u64>,
    #[ts(type = "Array<number>")] pub cache_read: Vec<u64>,
    #[ts(type = "Array<number>")] pub cache_write: Vec<u64>,
    /// cache_read × (input rate − cache-read rate) from the catalog; 0 when a rate is missing.
    #[ts(type = "Array<number>")] pub cache_saved_nanos: Vec<u64>,
    /// Reported money whose hour is approximate (turn-weighted or fallback placement).
    #[ts(type = "Array<number>")] pub approx_nanos: Vec<u64>,
}

pub struct CostCubeMeta {
    /// This engine's name (the ledger is per engine, H4).
    pub engine: String,
    #[ts(type = "number | null")] pub first_call_ms: Option<u64>,
    #[ts(type = "number")] pub built_at_ms: u64,
    /// Sessions before the ledger existed (history_coverage = 'unknown'), H3.
    pub pre_ledger_sessions: u32,
    /// Unpriced attempts by reason (status / unknown_reason), for the Basis card.
    pub unpriced: Vec<CostReason>,
    /// Hash of the price table used for repricing and cache savings.
    pub price_table: String,
}
pub struct CostReason { pub reason: String, pub calls: u32 }

/// `cost_cube(since)` answer: a full snapshot, or the changes since `since`.
pub struct CostCube {
    pub epoch: String,
    #[ts(type = "number")] pub rev: u64,
    /// true: replace everything the client holds. false: a delta.
    pub full: bool,
    /// Delta only: every cell of these UTC hours is replaced by the cells of this answer
    /// (cells carry every root of those hours). Empty when `full`.
    pub hours: Vec<u32>,
    /// Small dictionaries, always complete and append-only within an epoch.
    pub providers: Vec<String>,
    pub kinds: Vec<String>,
    pub vias: Vec<String>,
    pub models: Vec<CostModel>,
    /// Full: every root (ix = position). Delta: new or changed roots only.
    pub roots: Vec<CostRootEntry>,
    pub cells: CostCells,
    pub meta: CostCubeMeta,
}
```

TS as ts-rs emits it (the UI hand-writes these files as gitignored stubs until integration regenerates them):

```ts
export type CostBasis = "known" | "reported" | "estimated" | "unpriced";
export type CostCubeCursor = { epoch: string, rev: number, };
export type CostRoot = { id: string, title: string, owner_kind: SpendOwnerKind, owner: string, project: string | null, deleted: boolean, created_at_ms: number, };
export type CostRootEntry = { ix: number, root: CostRoot, };
export type CostModel = { id: string, name: string, provider: string, };
export type CostCells = { hour: Array<number>, root: Array<number>, model: Array<number>, provider: Array<number>, kind: Array<number>, via: Array<number>, basis: Array<number>, nanos: Array<number>, calls: Array<number>, failed: Array<number>, repriced: Array<number>, input: Array<number>, output: Array<number>, cache_read: Array<number>, cache_write: Array<number>, cache_saved_nanos: Array<number>, approx_nanos: Array<number>, };
export type CostReason = { reason: string, calls: number, };
export type CostCubeMeta = { engine: string, first_call_ms: number | null, built_at_ms: number, pre_ledger_sessions: number, unpriced: Array<CostReason>, price_table: string, };
export type CostCube = { epoch: string, rev: number, full: boolean, hours: Array<number>, providers: Array<string>, kinds: Array<string>, vias: Array<string>, models: Array<CostModel>, roots: Array<CostRootEntry>, cells: CostCells, meta: CostCubeMeta, };
```

### 3.2 RPC and event

- `cost_cube(since: CostCubeCursor | null) -> CostCube` — `since` null, a different epoch, or a rev older than the retained change log (keep the last 1024 revs' touched hours) → `full: true`; `since.rev == rev` → an empty delta. Answers from memory (< 5 ms). Registered like `spend_report` (`crates/rpc/src/api.rs` method + `crates/desktop/ui/src/rpc/api.ts` + `methods.json`).
- `cost_calls(query: CostCallsQuery) -> CostCallsPage` (§4).
- `usage_limits() -> UsageLimits` (§5).
- Wire event `CostCubeChanged { epoch: string, rev: number }`, published at most once per second after the cube changed.

### 3.3 Build and refresh (recon §9.4, "Recommended cube definition" F)
- Full build at engine start in the background (0.3 s on Sky's ledger), on a price-table change, and when > 50 roots are dirty. Never on the RPC path: until the first build lands, `cost_cube` answers the empty cube with `full: true` and the UI keeps its IndexedDB snapshot.
- Incremental: keep `owner → revision`; on a trigger, diff `SELECT id, root_id, revision FROM cost_owners` → dirty roots; re-aggregate `cost_owners ⋈ model_calls` for those roots from `dirty_from = hour(last_refresh − 15 min)` (indexes `cost_owners_root`, `model_calls_created` suffice; ~20 ms); rebuild the reported leg and the root dimension in full (~20 ms); bump rev; record touched hours.
- Triggers: `ModelUsageChanged` (debounced 1 s trailing), session create/delete/retitle, and a 15 s timer — endless settles publish nothing (`endless/pump.rs#L683-L686`, 34 % of all calls) — so any new call is on the page within ≤ 15 s, typically ~1 s. (Optionally publish an internal cube-dirty signal from the endless settle path instead of the timer.)
- Equivalence: a test proves incremental == full rebuild after a scripted mix of reserve/settle/fail/meter/delete events.

### 3.4 Acceptance numbers (on a copy of Sky's ledger, 14:25 UTC)
Σ native nanos = `select sum(coalesce(returned_nanos,estimated_nanos,0)) from model_calls where status <> 'not_dispatched'` (≈ $18,216.96) + repriced $111.197; Σ reported = Σ `remote_meters.attributed_nanos` ($5,942.55); Σ calls = count of non-`not_dispatched` rows (≈ 143,217); Σ estimated nanos of kind `endless` = Σ `endless_calls.usd_estimate` (±1 nano per attempt); cube month-to-date = `spend_report(…).month.usd` after H5; placement conserves each meter's `attributed_nanos`.

### 3.5 Cache economics
`cache_saved_nanos` per cell from the catalog (`crates/models/data/models.json` rates): at today's rates cache reads saved ≈ $131.9k and write premiums cost ≈ $2.0k against $18.2k paid. The Tokens KPI shows "cache saved ≈ $X" only when the rates exist.

## 4. Engine: the calls ledger page

```rust
pub struct CostCallsCursor { #[ts(type = "number")] pub at_ms: u64, pub id: String }
pub struct CostCallsQuery {
    #[ts(type = "number | null")] pub from_ms: Option<u64>,
    #[ts(type = "number | null")] pub to_ms: Option<u64>,
    /// Root session ids (the UI maps owner / project / session filters to roots).
    pub roots: Option<Vec<String>>,
    pub providers: Option<Vec<String>>,
    /// Raw model ids.
    pub models: Option<Vec<String>>,
    pub kinds: Option<Vec<String>>,
    pub vias: Option<Vec<String>>,
    pub bases: Option<Vec<CostBasis>>,
    /// Exclusive cursor: rows strictly older than it.
    pub before: Option<CostCallsCursor>,
    /// Default 200, max 1000.
    pub limit: Option<u32>,
}
pub struct CostCall {
    /// `call_id`, or `meter:<meter id>:<k>` for a placed piece of reported money.
    pub id: String,
    #[ts(type = "number")] pub at_ms: u64,
    pub root: String,
    /// The owner session (may be a subagent of `root`).
    pub session: String,
    pub via: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub kind: String,
    pub status: String,
    pub basis: CostBasis,
    #[ts(type = "number | null")] pub nanos: Option<u64>,
    #[ts(type = "number | null")] pub input: Option<u64>,
    #[ts(type = "number | null")] pub output: Option<u64>,
    #[ts(type = "number | null")] pub cache_read: Option<u64>,
    #[ts(type = "number | null")] pub cache_write: Option<u64>,
    pub repriced: bool,
    /// Why there is no money (status / unknown_reason), for unpriced rows.
    pub reason: Option<String>,
}
pub struct CostCallsPage { pub calls: Vec<CostCall>, pub next: Option<CostCallsCursor> }
```
```ts
export type CostCallsCursor = { at_ms: number, id: string, };
export type CostCallsQuery = { from_ms: number | null, to_ms: number | null, roots: Array<string> | null, providers: Array<string> | null, models: Array<string> | null, kinds: Array<string> | null, vias: Array<string> | null, bases: Array<CostBasis> | null, before: CostCallsCursor | null, limit: number | null, };
export type CostCall = { id: string, at_ms: number, root: string, session: string, via: string, provider: string | null, model: string | null, kind: string, status: string, basis: CostBasis, nanos: number | null, input: number | null, output: number | null, cache_read: number | null, cache_write: number | null, repriced: boolean, reason: string | null, };
export type CostCallsPage = { calls: Array<CostCall>, next: CostCallsCursor | null, };
```
Newest first, keyset-paged on `(created_at, call_id)`; reported pieces interleave by their placed time; for any filter Σ page money over all pages = the cube's total for the same filter. Indexed (< 30 ms at 150 k rows); add an index on `model_calls(created_at)` only if the plan needs it (the recon found the existing ones sufficient for the cube; the unfiltered newest-first page may not be).

## 5. Engine: usage limits (recon-limits.md)

### 5.1 What exists on Sky's machine (2026-11-02)
| provider | auth in theuth | traffic | limit source | v1 |
|---|---|---|---|---|
| Anthropic | API key from the engine's environment (`ANTHROPIC_API_KEY`) | ≈141k calls, live | `anthropic-ratelimit-{requests,tokens,input-tokens,output-tokens}-{limit,remaining,reset}` (RFC 3339 resets) + `retry-after` on every `/v1/messages` response — verified live; no poll works with a normal key (admin APIs 401, `count_tokens` carries no headers) | headers |
| OpenAI | Codex OAuth grant `oauth/openai` | 1,727 calls, last 2026-10-11 | `GET https://chatgpt.com/backend-api/wham/usage` (Bearer + `ChatGPT-Account-Id`): `plan_type`, `rate_limit.primary_window` / `secondary_window` {`used_percent`, `limit_window_seconds`, `reset_after_seconds`, `reset_at`}, `credits`; and `x-codex-{primary,secondary}-{used-percent,window-minutes,reset-at}` + `x-codex-credits-*` response headers (shapes from codex-rs `aef7d27`) | poll + headers |
| Amazon Bedrock | SigV4, profile `sre-stage1` | 81 calls, last 2026-10-10 | none in band | "no usage API" row |
| Cursor | login, 0 calls | – | undocumented, low confidence | "unavailable" row |
| Anthropic OAuth (Claude Pro/Max) | no grant here | – | `GET https://api.anthropic.com/api/oauth/usage` (Bearer + `anthropic-beta: oauth-2025-04-20`): `five_hour` / `seven_day` {utilization %, resets_at}, `limits[]`, `extra_usage`; `anthropic-ratelimit-unified-{5h,7d}-{utilization,reset,status}` headers (fractions) | implemented, shown only when a grant exists |
| OpenAI API key, Groq, xAI, named gateways | not configured | – | generic `x-ratelimit-{limit,remaining,reset}-{requests,tokens}` (+ `-project-tokens`; durations like `6m0s`) | parser, shown when configured |
| OpenRouter / OpenCode Go / DeepSeek | not configured | – | `GET /api/v1/key` · `GET /zen/go/v1/usage` · `GET /user/balance` | poll, shown when configured |
| theuth budgets | roster `Budget::Monthly` (0 declared), session caps (dreaming `$25/night`) | – | roster + sessions.budget | budget rows when any exist |

### 5.2 Contract (frozen; `crates/agent/src/limits/mod.rs`, same derive pattern as §3.1)

```rust
#[serde(rename_all = "snake_case")]
pub enum LimitStatus { Ok, Warn, Limited, Stale, Unavailable, NotConfigured, Error }

/// A subscription usage window (Codex primary/secondary, Claude 5-hour/weekly…).
pub struct UsageWindow {
    pub id: String,              // "primary" | "secondary" | "five_hour" | "seven_day" | "seven_day_opus" | …
    pub label: String,           // "5-hour" | "Weekly" | "Weekly · Opus"
    /// 0.0 = unused, 1.0 = exhausted (percent / 100); may exceed 1.0.
    pub used_fraction: f64,
    #[ts(type = "number | null")] pub resets_at_ms: Option<u64>,
    #[ts(type = "number | null")] pub window_secs: Option<u64>,
}

/// A per-minute API rate-limit bucket observed in response headers.
pub struct RateLimit {
    pub id: String,              // "requests" | "tokens" | "input_tokens" | "output_tokens" | "project_tokens"
    pub label: String,           // "Requests / min"
    /// Model class the bucket applies to when the provider scopes it (Anthropic: per model group).
    pub scope: Option<String>,
    #[ts(type = "number")] pub limit: u64,
    #[ts(type = "number")] pub remaining: u64,
    #[ts(type = "number | null")] pub resets_at_ms: Option<u64>,
}

/// Credits / key limit / prepaid balance.
pub struct Balance {
    pub label: String,
    pub currency: String,        // "USD" or "credits"
    pub remaining: Option<f64>,
    pub limit: Option<f64>,
    pub used: Option<f64>,
}

pub struct ProviderLimits {
    /// Stable row key, e.g. "anthropic:api_key", "openai:oauth", "amazon-bedrock".
    pub id: String,
    pub provider: String,
    pub label: String,           // "Anthropic API", "ChatGPT · Codex", "Amazon Bedrock"
    pub auth: String,            // "api_key" | "oauth" | "aws" | "gateway" | "login"
    /// Truncated account id / email exactly as the source states it — never a credential.
    pub account: Option<String>,
    pub plan: Option<String>,
    pub status: LimitStatus,
    pub message: Option<String>,
    pub windows: Vec<UsageWindow>,
    pub rates: Vec<RateLimit>,
    pub balance: Option<Balance>,
    /// "response headers" | "usage endpoint" | "headers + usage endpoint" | "none".
    pub source: String,
    #[ts(type = "number | null")] pub observed_at_ms: Option<u64>,
}

pub struct BudgetLimit {
    pub id: String,              // "agent:<name>" | "routine:<name>" | "session:<id>"
    pub label: String,
    pub period: String,          // "month" | "night" | "run"
    pub cap_usd: f64,
    pub spent_usd: f64,
    #[ts(type = "number | null")] pub resets_at_ms: Option<u64>,
}

pub struct UsageLimits {
    pub providers: Vec<ProviderLimits>,
    pub budgets: Vec<BudgetLimit>,
    #[ts(type = "number")] pub generated_at_ms: u64,
}

/// The engine-wide limits cache. Cheap to clone (Arc inside).
#[derive(Clone, Default)]
pub struct LimitsHub { /* LIM */ }
impl LimitsHub {
    /// The process-wide hub (OnceLock). Production code uses only this one; tests build their own with `default()`.
    pub fn global() -> &'static LimitsHub;
    /// The current snapshot, from memory (microseconds; never touches the network).
    pub fn snapshot(&self) -> UsageLimits;
    /// One response's headers (lower-case names) from a model call to `provider` with `auth`,
    /// for `model` (scope derivation). Called on every response, success or failure.
    pub fn observe_headers(&self, provider: &str, auth: &str, model: &str, status: u16, headers: &[(String, String)], now_ms: u64);
    /// Ask the pollers to refresh soon (debounced per source; returns immediately).
    pub fn kick(&self, provider: Option<&str>);
}
```
```ts
export type LimitStatus = "ok" | "warn" | "limited" | "stale" | "unavailable" | "not_configured" | "error";
export type UsageWindow = { id: string, label: string, used_fraction: number, resets_at_ms: number | null, window_secs: number | null, };
export type RateLimit = { id: string, label: string, scope: string | null, limit: number, remaining: number, resets_at_ms: number | null, };
export type Balance = { label: string, currency: string, remaining: number | null, limit: number | null, used: number | null, };
export type ProviderLimits = { id: string, provider: string, label: string, auth: string, account: string | null, plan: string | null, status: LimitStatus, message: string | null, windows: Array<UsageWindow>, rates: Array<RateLimit>, balance: Balance | null, source: string, observed_at_ms: number | null, };
export type BudgetLimit = { id: string, label: string, period: string, cap_usd: number, spent_usd: number, resets_at_ms: number | null, };
export type UsageLimits = { providers: Array<ProviderLimits>, budgets: Array<BudgetLimit>, generated_at_ms: number, };
```

Wiring without shared plumbing: ENG's RPC handlers call `crate::limits::LimitsHub::global()`; LIM calls `LimitsHub::global().observe_headers(…)` from `Backend::stream` and starts its pollers with one marked line (`// 160:LIM pollers`) in `supervisor::start` right after `Auth` is built. ENG ships a stub `limits/mod.rs` with exactly these types and a neutral hub (empty `providers`, `budgets`, no-op `observe_headers`/`kick`); LIM's tree has the full file; at merge LIM's file wins whole.

RPC (ENG registers both; the handlers call the hub): `usage_limits() -> UsageLimits` and `usage_limits_refresh(provider: string | null) -> null` (kicks the pollers; the UI calls it when `/costs` mounts and every 60 s while visible, and polls `usage_limits()` every 15 s while visible — a memory read). No event.

### 5.3 Behaviour (LIM)
- **Status**: `ok`; `warn` when any window ≥ 75 % or any rate bucket ≤ 15 % remaining; `limited` when a window ≥ 100 % / a 429 was seen and its reset has not passed; `stale` when the newest observation is older than its source's expected cadence × 2 (headers: 15 min) or the last poll failed transiently (keep last-good values); `unavailable` for providers with no source (Bedrock, Cursor v1, Gemini); `not_configured` providers are omitted unless they had calls in the last 30 days; `error` for a hard failure (expired login: message "login expired — run /login openai").
- **Header merge**: many streams update one bucket — key by (provider, scope, bucket), keep the observation with the newest server `date`, tie → lowest remaining; a bucket whose reset passed shows as full until re-observed.
- **Pollers** (agent crate, started in `supervisor::start` right after `Auth` is built, `supervisor.rs#L9463-L9471`; copy the tick-loop pattern of `crates/update/src/lib.rs#L580-L674`): Codex 60 min background, ≤ 1 per 5 min on `kick`; Claude OAuth 10 min / ≤ 1 per min; OpenRouter, OpenCode Go, DeepSeek 10 min / ≤ 1 per min — each only when its credential exists. 5 s timeout, one in flight per source, ±20 % jitter, never retry a 429 inside a poll, cool-down 5 → 30 min after 429, kill switch `limits.<provider>.poll: false` in config. Tokens via `auth.oauth.access(&oauth::OPENAI | ANTHROPIC)` (`oauth/src/lib.rs#L780-L847`; hub engine only — on a non-hub engine mark `stale`, not `error`).
- **No paid probe by default.** Anthropic API-key limits come only from real calls. An idle probe (Haiku, `max_tokens: 1`, ≈ $1.5e-6) exists behind `limits.anthropic.probe: true` (default false).
- **Persistence**: atomic private snapshot `<data_dir>/limits/snapshot.json` (`platform::files::write_private`), loaded at start and served as `stale` until refreshed.
- **Budgets**: roster monthly caps (agent spend this month from the ledger vs cap) and session caps (`sessions.budget`, e.g. dreaming's `$25/night`: the latest capped run's spend vs cap). Rows only when a cap exists.

### 5.4 Header capture seam (LIM)
genai is Sky's fork `skymoore/rust-genai` pinned at `617cafe` (`Cargo.toml#L200,#L389,#L418`, `Cargo.lock#L7021-L7024`). It drops response headers on every success path. Change, in a clone of the fork on a new branch `theuth-160-response-headers` (pushed to the fork's origin; never force-push, never touch other branches): a defaulted `ChatFrameSink::on_response(&FrameCtx, http::StatusCode, &http::HeaderMap)` (`src/chat/frame_sink.rs#L163-L178`), forwarded by `FrameTap` (`src/webc/frame_tap.rs#L20-L54`) and called from `WebStream::poll_next` before the `is_success()` branch (`src/webc/web_stream.rs#L104-L134`), plus a unit test; then bump theuth's pin (all three `Cargo.toml` places + `Cargo.lock`). In theuth: `Backend::stream` (`crates/agent/src/backend.rs#L194-L241`, the single call site of every genai model call) installs a header sink that calls `LimitsHub::observe_headers`; additionally harvest `e.status()` / `e.headers()` from a failed call before `provider_error` (`backend.rs#L236`, `genai/src/error.rs#L195-L215`) so 429s are seen even without the fork change. Cursor: none in v1 (`crates/cursor/src/run.rs#L66-L82` is where headers would be read).

## 6. Performance contract — "there should never be any loading or lagging"

Hard rules; the closer's live proof measures each one.

1. **Prefetch at app start.** A `useCostsPrefetch()` hook mounted once in the root layout (`routes/__root.tsx` or `App.tsx`, after the RPC connection is up) fetches `cost_cube(null)` and `usage_limits()` into React Query (`['costs','cube']`, `['costs','limits']`, `staleTime: Infinity`, `gcTime: Infinity`) and subscribes to `CostCubeChanged` for deltas. The HUD's `useSpendReport` stays as is.
2. **Cold start from disk.** The merged cube is written to IndexedDB (`theuth-costs` db, store `cube`, key `v1:<engine url>`), debounced 2 s and in `requestIdleCallback`; at boot it is read before the first `/costs` render and used as `initialData`, then reconciled with a delta (`since = {epoch, rev}` from the snapshot) or a full answer when the epoch changed. Use the existing `link/idb.ts` helper if it fits; otherwise a 40-line wrapper.
3. **Route chunk preloaded.** `/costs` may be a lazy route, but its chunk is preloaded at idle after start (`router.preloadRoute({ to: '/costs' })` inside `requestIdleCallback`) and on hover/focus of the SPEND button and the sidebar row.
4. **No spinners, no skeletons, no "Loading…".** Every section renders from the in-memory cube synchronously. The only empty state is a genuinely empty ledger ("No model calls recorded yet"). The ledger table keeps its previous rows while the next page arrives (`placeholderData: keepPreviousData`); a 1 px progress hairline may appear only if a page takes > 150 ms.
5. **Interaction budget.** Any control change (range, granularity, group-by, metric, filter chip, legend toggle, brush) re-renders the page in < 16 ms on Sky's ledger (≈ 3.5 k cube rows) and < 50 ms at 100 k rows. The client cube (`lib/costs/cube.ts`) stores measures in typed arrays, precomputes per-row local hour/day/week/month keys once per ingest, and answers each query in one pass (target < 4 ms at 50 k rows; a vitest asserts < 50 ms on a 100 k-row synthetic cube so it never flakes under load). Use `useDeferredValue` for the secondary panels if the main chart must stay at 60 fps while typing in the session search.
6. **Charts.** `isAnimationActive={false}` for every series after the first mount (a single 300 ms entry animation is allowed); data arrays memoised by query key; ≤ 400 buckets per chart — the granularity control disables choices that would exceed it (e.g. Hour beyond 14 days) and auto-picks on range change (Today/24H → hour, ≤ 31 d → day, ≤ 180 d → week, else month). Tooltips are the shadcn `ChartTooltipContent`; hover never triggers a data recompute.
7. **Engine side.** The cube is built in the background when the engine starts (never on the RPC path); `cost_cube` answers from memory in < 5 ms; a delta refresh re-aggregates only the touched hours (indexed range query, § 3.5) and runs at most once per second; `cost_calls` is an indexed page query (< 30 ms at 150 k rows); `usage_limits` returns the cached snapshot (no network on the RPC path).
8. **Numbers move, layout doesn't.** KPI figures use `@number-flow/react` (already a dependency) so live updates tick instead of flashing; card sizes are fixed so nothing shifts when a value changes.

## 7. The page (`/costs`)

Layout follows the approved mock (`costs-mock.html`); theme tokens only (no hard-coded hex), both themes, phone ≤ 768 px = one column with horizontally scrolling tables.

1. **Band** (52 px, like SectionBand pages): title `Costs`, a live dot + `ledger since <first call date>`, an `Export CSV` button (exports the current view's buckets × series, client-side).
2. **Range bar.** Presets `Today · 24H · 7D · 30D · MTD · 90D · All` (default 30D) + `Custom` (popover with the shadcn range Calendar, two months, quick picks "last month", "this week", "last week"); granularity `Hour · Day · Week · Month` (disabled choices explained in a tooltip); dragging across the main chart selects a custom range (Recharts `ReferenceArea` while dragging); a `previous period` toggle.
3. **Filter chips.** Provider, Model, Owner (you / each agent / each routine / other), Kind, Via (theuth / each harness), Basis, Project, and a session search (title substring). Each chip opens a `Command` multi-select with per-option spend for the current range; active chips are filled and removable. Clicking any bar segment, legend entry, breakdown row or table row's owner/model adds that filter.
4. **KPI row** (6 cards): Spend in range (+ delta vs the previous equal-length period, sparkline); Today (this hour, burn rate of the last 60 min, yesterday); This month (MTD, projection at the month's pace, last month total); Calls (failed count, $/call); Tokens (output, cache-hit rate = cache_read / (input incl. cache), cache savings when the price table allows — §3.6); Basis (stacked known / reported / estimated bar, unpriced call count).
5. **Main chart.** Stacked bars over time; group by `Model · Provider · Owner · Kind · Via · Basis · Project` (top 7 + "other"); metric `$ · Calls · Tokens`; previous-period overlay as a dashed line; clickable legend toggles series; brush shown when > 40 buckets.
6. **Usage limits** (§5): one card per provider snapshot — subscription windows as RadialBar gauges (warning ≥ 75 %, danger ≥ 90 %), rate limits as meters, balances, status tag (`ok · warn · limited · stale · unavailable · not configured · error`) and `source · observed N ago`; theuth budget caps as meters; providers that expose nothing say so in one line.
7. **Breakdowns** grid: By model (horizontal bars with $, calls, $/call, tokens), By owner, By provider (donut + legend list), By kind, By project — all for the selected range and filters.
8. **When**: hour-of-day × weekday heat grid (local time) for the range, and an hour-of-day profile.
9. **Tokens**: input mix per bucket (cache read / cache write / uncached, 100 % stacked area) and output tokens per bucket.
10. **Sessions** table: root sessions in range with owner, project, models, calls, tokens, cost (inline bar), basis, last call; sortable; click → `/c/$session`. Computed from the cube (no RPC).
11. **Ledger** table: individual calls, newest first, server-paged `cost_calls` with the active filters, virtualised (`@tanstack/react-virtual`), columns time · session · kind · model · in · cache · out · cost · basis · status; click → the session.

## 8. Who owns what (parallel workers; shared files are frozen to their owner)

| worker | worktree / branch | owns (may create/edit) | must NOT touch |
|---|---|---|---|
| **ENG** | `~/Develop/theuth-160-eng` · `feat/160-eng` | `crates/agent/src/costs/**` (new), the cube/calls/tombstone/meter-report changes in `crates/agent/src/store/accounting.rs`, `crates/agent/src/store.rs` (root delete only), `crates/agent/src/lib.rs` (`mod costs;` + exports + the `CostCubeQuery`/`CostCallsQuery`/`UsageLimitsQuery` messages), `crates/agent/src/supervisor.rs` (handlers + the cube actor wiring + `CostCubeChanged` publication), the new migration(s) under the agent crate's migrations dir, `crates/rpc/src/api.rs` + `crates/rpc/src/export.rs` (three methods + the event + binding exports), and a **stub** `crates/agent/src/limits/mod.rs` holding exactly the §5 types and a neutral `LimitsHub` (LIM replaces the whole file at merge) | `crates/desktop/**`, the real limits implementation, `crates/agent/src/backend.rs`, `crates/oauth/**`, `crates/cursor/**` |
| **LIM** | `~/Develop/theuth-160-lim` · `feat/160-lim` | `crates/agent/src/limits/**` (full implementation; same §5 types and `LimitsHub` API as the stub), header capture in the provider HTTP path (files named in §5.4), `crates/agent/src/lib.rs` (`pub mod limits;` only), the poller start at engine start (one marked line `// 160:LIM pollers` in `supervisor::start`), `Cargo.toml`/`Cargo.lock` (the genai pin bump only), and the genai fork clone + branch (§5.4) | `crates/agent/src/costs/**`, `crates/agent/src/store/**`, `crates/rpc/**`, `crates/desktop/**` |
| **UI** | `~/Develop/theuth-160-ui` · `feat/160-ui` | `crates/desktop/ui/**` except generated bindings (hand-written binding stubs for the new types are gitignored and never committed); `src/rpc/api.ts` method list (committed) | every `.rs` file, `bindings/` |
| **Z** (closer, later) | `~/Develop/theuth-160` · `integrate/160` | merges, bindings regeneration, fixes found by the live proof | — |

`api.test.ts` compares the UI's METHODS with `bindings/methods.json`: the UI tree shows exactly that one expected red until Z regenerates the bindings.

## 9. Gates
- ENG/LIM: `mbx test -p agent -p rpc` (same `-p` set for `--no-run` and the run), `mbx clippy -p agent -p rpc --all-targets`, `cargo fmt -p agent -p rpc`; ENG also the bindings export.
- UI: `bun run check` (tsc), `CI=1 bunx vitest run src/lib/costs src/components/costs src/components/hud` then the full `CI=1 bun run test` under `BUILD_SLOTS=1`, `bunx vite build` (heavy pool).
- Z: all of the above on the merged tree + `mbx check --workspace` once (`BUILD_SLOTS=1`), then the live proof.

## 10. Live proof (Z)
Scratch engine (own data dir under /tmp, own ports) started from the merged build with a **copy** of Sky's `~/.theuth/sessions.sqlite` (and `roster.sqlite`, `autoresearch.sqlite` if present) — never the live files, never `secrets.vault`. Vite dev server against it; headless Chrome at 1280×900 and 390×844, soot and bone. Steps, each PASS/FAIL with a screenshot: HUD SPEND button visible and opens `/costs` with no spinner; numbers match §3.4; range presets / custom range / drag-zoom / granularity; every group-by; each filter chip + cross-filter click; previous-period overlay; limits panel states (configured / not configured / unavailable); sessions table → opens a session; ledger paging (scroll 3 pages, no spinner); cold reload paints from IndexedDB before the engine answers (throttle the socket to prove it); a live call on the scratch engine (one cheap chat on a haiku model, < $0.05) appears within 15 s on the page and on the HUD; interaction timings (React profiler or `performance.now()` around a filter change) < 16 ms on the real cube.
