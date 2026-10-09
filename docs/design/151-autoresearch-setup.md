# 151 — Autoresearch setup: a collaborative setup chat in the Autoresearch tab

Contract writer C, 2026-10-09, branch `feat/151-c` (cut from main `6d151b8a`). Every `file:line` below is on main `6d151b8a`. Decisions D1–D9 (`~/.theuth/briefs/2026-10-09/151/decisions.md`) are ruled by Sky ("looks good build it") and are not reopened here; the mock is `mock-v2.html` (three screens). Two workers build from this doc without talking: **ENG** (Rust) and **UI** (`crates/desktop/ui`). §12 "Contract as built" is what C committed.

## 1. What and why

Sky, 2026-10-09 12:38: "Auto research should have an interface to collaboratively set up an autoresearch with an agent, minimally, a button in the autoresearch tab to start a chat in that mode."

Today a study is set up by typing `/autoresearch <goal>` in any chat (`slash.ts:529-532` → `new_session` → `use_skill(id, 'autoresearch', arg)`); the agent interviews, writes a benchmark, and ends with one `autoresearch_draft` card (`crates/app/src/study_draft.rs:325-409`, five lines + warnings + Start now · Start in idle window · Start with my uncommitted changes · Edit · Discard). The draft appears only once, at the end; "Edit" means telling the agent what to change; nothing in the Autoresearch tab shows the setup.

151 copies design 108 Routines "Describe it" (`RoutineChat.tsx`, `draft.ts`, `RoutineDetail.tsx`, `RoutinesView.tsx`): the chat is embedded in the tab, the newest draft fills a panel live, phones get a full-screen sheet. Two additions the Routines precedent did not need: **field states computed by the engine** (verified / confirmed / proposed / default / missing / optional) because a study is only worth starting when its benchmark has been *measured*, and an **engine probe** that is the only source of "Verified". After Start the setup chat stays the study's chat (D7).

## 2. The flow

1. **New study.** `+ New study` in the rail's 44 px `SectionRow` (`Explorer.tsx:32-53`), always visible; empty states (`StudyList.tsx:25-29`, `Explorer.tsx:65-78`) become "Set one up with theuth" (primary) with `/autoresearch <goal>` kept as secondary text. Both → `/autoresearch/new`.
2. **Intro box** (route `/autoresearch/new`, no `?chat`): Repository (path field + folder pick where the desktop offers a directory dialog + dropdown of recent repos), optional "What should get better?", button **Start setup chat**.
3. **First send** (= `startRoutineChat`, `draft.ts:138-149`): `new_session({cwd: repo})` → `rename_session(id, "New study: <goal or repo basename>")` → `loadHistory(id, FRESH, 'replace')` → `onSession(id)` (URL becomes `?chat=<id>`, `LiveSession` mounts) → `use_skill(id, 'autoresearch', seed)` (§4.6 seed).
4. **Setup chat.** The agent reads the repo, drafts early with `autoresearch_draft {…, partial: true}` once goal + repo are known, re-drafts after each settled answer with `confirmed: [rows]`, writes the bench under `<root>/drafts/<slug>/`, runs `autoresearch_probe {draft_id, what: bench}` and relays the one-line result. The panel reads `autoresearch.draft {draft_id}` → `DraftView` and refetches on every new `autoresearch_draft` / `autoresearch_probe` tool result in the session.
5. **Panel.** Rows in fixed order with a badge each; per-row **OK** (`autoresearch.draft_confirm`) and **Edit** (`autoresearch.draft_edit` + one user message into the chat); **Discard**. Start buttons unlock when `ready`.
6. **Start** — panel button or the final card's button — `autoresearch.create {draft_id, start}` / `autoresearch_start {draft_id, token}`; both press the same draft, the first wins (`StudyRecord` with that slug exists → the second is refused by `create`'s existing duplicate check; the UI shows "started").
7. **After Start.** `StudyInfo.origin` = the setup session; the panel flips to a status card from `autoresearch.get`; the study page header gets "Setup chat ›"; the chat steers the study with the existing `autoresearch_control` / `autoresearch_status`.

## 3. Facts relied on (main 6d151b8a)

- Draft file: `<root>/drafts/<slug>/spec.json` = `Draft { spec, origin, token }` (`crates/autoresearch/src/tools.rs:123-134`), written by `write_draft` (`tools.rs:143-152`), read by `read_draft` (`:155-161`), refusals in `validate_spec` (`:165-224`); `drafts_dir` (`lib.rs:74-76`). The `autoresearch_draft` tool is `app`'s: `crates/app/src/study_draft.rs:325-409`, `type Args = StudySpec` (`:329`), registered at `crates/app/src/lib.rs:369-372`.
- Start: `autoresearch_start` tool (`tools.rs:653-697`, token check `:672`), RPC `autoresearch.create` (`crates/rpc/src/api.rs:1886-1891` → `:6772-6780` → `rpc_impl::create` `rpc_impl.rs:105-107` → `actor::start_draft` `actor.rs:1963-1974` → `create` `:1683` `StudyRecord::new(spec, Some(origin))`). `StudyRecord.origin` exists (`store.rs:89`, column `origin` `:700`) and is used by `deliver(study.origin, …)` (`actor.rs:1413-1430`, `:1877`, `:3961`). `StudyInfo` is built in `info_from` (`actor.rs:1118-1173`) — `origin` is simply not copied.
- Sampling: `runner::sample(spec, cfg.bench, SampleRequest, cancel)` (`runner.rs:56-132`, `SampleRequest` `:31-48`) does load wait + exclusive slot + stream/scan; `runner::command(command, timeout, req, cancel)` → `(exit, tail, wall_ms)` (`:156`). Baseline loop `actor::baseline` (`actor.rs:1884-1958`); noise = `gate::pooled_sigma(&[values]) / mean.abs()` (`actor.rs:1829-1833`, `gate.rs:25`), refused above `cfg.noise.max_cv` (`config.rs:108`, default 0.05 `:297`); smallest detectable ≈ `cfg.gate.z × cv` (`actor.rs:1838`).
- Git: `git::worktree_add_detached(repo, path, sha)` (`git.rs:626-632`), `worktree_remove` (`:266`), `resolve_base(repo, base, include_dirty)` (`:178`, refuses dirty unless `include_dirty`), `is_git_repo` (`:710`), `head` (`:282`).
- Root tools refuse study sessions through `refuse_study_sessions(cx.session)` (`tools.rs:811-825`); tool registration `tools::register` (`:69-86`). `ToolCx { session, call_id, cancel, promote_after, … }` (`crates/tools/src/lib.rs:305-319`): a long tool call is promoted to a job and cancelled through `cancel` (145).
- RPC: methods `api.rs:1847-1900`, bodies `:6712-6800`, `dreaming_err` (`:2537`), `METHODS` (`rpc_impl.rs:14-25`), test `autoresearch_methods_are_registered` (`api.rs:8229-8235`). Bindings: `crates/rpc/src/export.rs` (`export_bindings`), copied to `crates/desktop/ui/src/rpc/bindings/` (gitignored, `crates/desktop/.gitignore:5`).
- UI: `Api` interface `api.ts:283` (autoresearch entries `:1047-1065`), `METHODS` sorted `:1069-` (autoresearch `:1098-1107`), `api.test.ts:12-19` checks `METHODS` against `bindings/methods.json`. Queries `components/autoresearch/queries.ts` (`KEY = ['autoresearch']` `:19`, `['autoresearch','list']` `:49`, `[…, slug, 'detail']` `:55`, events `:26-45`). Explorer rail `Explorer.tsx:20-62` (`active` from `useParams({strict:false}).slug` `:23`; `Outlet` only when `active` `:57`). Study header `StudyView.tsx:68-80` (`SectionRow`), `study session` link `:87-101`. Routes `routes/autoresearch.tsx:5-8`, `routes/autoresearch.$slug.tsx:5-8`; `routeTree.gen.ts` is generated. `chatSearch` validator `routes/routines.new.tsx:6-10`. Embedded chat `RoutineChat.tsx:62-63` (`<LiveSession session first={false} />` from `@/routes/c.$session`); `latestDraft(entries)` `draft.ts:99-107` (entry `kind:'tool'`, `name`, `args`, `callId`; `store/sessions.ts:111-115`); first send `draft.ts:138-149`; `RoutineDraftBridge` `RoutineChat.tsx:101-116` (hears `theuth:app-press` from `AppHost.tsx:443`). Phone: `useIsMobile()` (`hooks/use-mobile.ts:21`), `Sheet` usage `RoutineDetail.tsx:223-230`. Sending a user message: `api.send_input(session, text, [], null, id?)` (`api.ts:917-923`, `Composer.tsx:771`). `CreateSessionOptions.cwd` exists (`new_session` `api.ts:819`).

## 4. Frozen names

### 4.1 Rust wire types (`crates/autoresearch/src/types.rs`, §"151 setup", `wire!` macro → serde + JsonSchema + `ts(export)`; C committed these)

```rust
pub enum FieldState { Verified, Confirmed, Proposed, Default, Missing, Optional }   // snake_case; ts "DraftFieldState"
pub struct DraftField { name: String, value: String, state: FieldState, evidence: Option<String> }
pub struct DraftView {
    draft_id: String, spec: StudySpec, origin: Option<SessionId>,
    fields: Vec<DraftField>,          // §5 rows in fixed order, main rows then advanced
    settled: u32, total: u32,         // settled = main rows not proposed/missing; total = 9
    ready: bool, blocking: Vec<String>, // footer: "Scope needs your OK", "bench.command is empty", …
    dirty: bool,                      // repo tree dirty → "Start with my uncommitted changes"
    partial: bool, version: u32, updated_at: String,
    probes: Vec<ProbeResult>,         // probe.json entries whose fingerprint still matches (for the sparkline)
    started: bool,                    // a StudyRecord with this slug exists → UI flips to the status card
}
pub struct DraftSummary { draft_id, goal, repo: String, origin: Option<SessionId>, settled: u32, total: u32, ready: bool, updated_at: String }
pub enum ProbeKind { Bench, Check, Holdout }   // snake_case
pub struct ProbeResult { kind: ProbeKind, ok: bool, values: Vec<f64>, median: Option<f64>, noise_pct: Option<f64>,
    unit: String, samples: u32, duration_ms: u64, exit: Option<i32>, log_tail: String, at: String, fingerprint: String }
pub struct DraftArgs { #[serde(flatten)] spec: StudySpec, partial: bool, confirmed: Vec<String> }   // ts "StudyDraftArgs"
```

`StudyInfo` and `StudyDetail` (`types.rs:1183`, `:1235`) gain `origin: Option<SessionId>`; `info_from` copies `study.origin` (`StudyDetail` carries it through `info`).

### 4.2 Draft file `<root>/drafts/<slug>/spec.json` (`tools::Draft`, all new fields `#[serde(default)]`)

`{ spec, origin, token, partial: bool, confirmed: [row], user_set: [row], version: u32, updated_at: rfc3339 }`. `write_draft(root, spec, origin, partial, confirmed)` keeps `user_set` and bumps `version` from the existing file (C committed the storage; §6 is the behaviour ENG adds). `probe.json` beside it: `{ "bench": ProbeResult?, "check": ProbeResult?, "holdout": ProbeResult? }` (ENG, `setup::ProbeFile`). `probe/` = scratch worktree + `probe/runs/` logs, removed after every probe.

### 4.3 Rows and leaves

Rows (= `DraftField.name`, `confirmed`, `user_set`, `draft_confirm.field`), fixed order, main nine then advanced:
`goal · repo · metric · bench · checks · holdout · scope · budget · window` | `secondary_guards · warm · parallel · models · evaluator_paths`.
UI labels: Goal · Repository · Metric · Benchmark · Check · Holdout · Scope · Budget · When | Advanced.

Leaves (= `draft_edit.field`): `goal, repo, base, metric.name, metric.unit, metric.direction, metric.min_effect, bench.command, bench.timeout, bench.repeats, bench.exclusive, bench.smoke, checks.command, checks.timeout, holdout.command, holdout.every, holdout.repeats, scope, off_limits, evaluator_paths, budget.usd, budget.per_experiment_usd, budget.hours, budget.experiments, window, warm, parallel, secondary_guards, models.executor, models.review, models.advisor`. Row of a leaf = first segment, except `base → repo`, `off_limits → scope`. `value` is text: lists split on `,`/newline (trimmed, empties dropped); bools `true|false|on|off|yes|no`; numbers parsed; `metric.direction` `lower|higher`; `window` empty = start now; `secondary_guards` one guard per line `name max_regression [lower|higher]`; `models.*` empty = default; `checks.command`/`holdout.command` empty = `None`.

### 4.4 RPC (`crates/rpc/src/api.rs`, bodies delegate to `autoresearch::rpc_impl`, errors through `dreaming_err`)

| method | params | result |
|---|---|---|
| `autoresearch.draft` | `draft_id: String` | `DraftView` (Err "no draft `<id>`" after discard) |
| `autoresearch.drafts` | — | `Vec<DraftSummary>` newest `updated_at` first; drafts without a study; empty when the actor is off |
| `autoresearch.draft_edit` | `draft_id, field: leaf, value: String` | `DraftView` |
| `autoresearch.draft_confirm` | `draft_id, field: row` | `DraftView` |
| `autoresearch.discard_draft` | `draft_id` | `()` |

### 4.5 Tools

- `autoresearch_draft` (app): args `DraftArgs` (flattened spec + `partial` default false + `confirmed` default `[]`). Partial → compact card; final → today's card.
- `autoresearch_probe` (autoresearch, root tool, `Risk::Mutating`, `RETURNS "dict"`): args `ProbeArgs { draft_id: String, what: ProbeKind, repeats: Option<u8> }`; structured result = the `ProbeResult`.

### 4.6 Texts

- Seed (first `use_skill` text): line 1 `[autoresearch setup · repo <abs path>]`, line 2 the goal or `Help me set up a study for this repository`.
- Session title: `New study: <goal or repo basename>`.
- Panel edit message (one `send_input` after a successful `draft_edit`): `[study draft <slug>] you set <leaf> to <value> in the panel`. Discard message: `[study draft <slug>] discarded in the panel`. Confirms send nothing.
- Compact card (partial draft): header `study draft <slug>`, one line `Study draft <slug> · <settled> of <total> settled`, one button **Open in Autoresearch** = `@ToAssistant("study draft <slug>: open in autoresearch")`; the UI's `StudyDraftBridge` hears the press and navigates (§4.8).
- Sticky note in the tool result: `<row> was set by the user in the panel; your change needs their OK`.
- Rail row for a draft: `Setting up · <settled> of <total> settled`; study header link `Setup chat ›`.

### 4.7 Routes, components, query keys (UI)

- Routes: `routes/autoresearch.new.tsx` → `/autoresearch/new` (`validateSearch` = `chatSearch` from `routines.new.tsx:6-10`: `chat?: string`); `routes/autoresearch.$slug.tsx` unchanged. Static `new` wins over `$slug` in TanStack; ENG reserves the slug (§6.1). `Explorer.tsx:23` `active` becomes `slug ?? (pathname === '/autoresearch/new' ? 'new' : undefined)` so the outlet shows.
- Components (`components/autoresearch/`): `SetupPane.tsx` (the page: header row, `SetupChat`, `DraftPanel`, phone `Sheet`s), `SetupChat.tsx` (= `RoutineChat`: intro box | `LiveSession`), `DraftPanel.tsx` (rows, badges, OK/Edit, footer, Start buttons, status card after Start), `setup.ts` (`startSetupChat(api, repo, goal, onSession)`, `latestStudyDraft(entries)` = newest `autoresearch_draft` call → `{ draftId: args.slug, callId }`, `lastSetupResultId(entries)` = callId of the newest `autoresearch_draft`/`autoresearch_probe` *result*, `seedText`, `editMessage`, `rowLabel`), `StudyDraftBridge` (root-mounted in `__root.tsx` beside `RoutineDraftBridge`).
- Query keys: `[...KEY, 'drafts']` → `autoresearch.drafts`; `[...KEY, 'draft', draftId]` → `autoresearch.draft`, refetched when `lastSetupResultId` changes (D3: no new event type) and invalidated by `draft_edit` / `draft_confirm`. `[...KEY, 'repos']` is derived client-side from `['autoresearch','list']` ∪ `drafts` (no RPC).
- Hooks in `queries.ts`: `useDrafts()`, `useDraft(draftId)`, `useDraftEdit(draftId)`, `useDraftConfirm(draftId)`, `useDiscardDraft()` (all `useAction` like `useControl` `queries.ts:94-102`).

### 4.8 Card press bridge

`StudyDraftBridge` (UI, `components/autoresearch/SetupPane.tsx`, mounted in `__root.tsx`): hears `theuth:app-press` with `app === 'study-drafts'` and message `/^study draft ([a-z0-9-]{1,32}): open in autoresearch$/m`; unless already under `/autoresearch/new`, navigates to `/autoresearch/new?chat=<d.session>`. Mirror of `RoutineChat.tsx:101-116`.

## 5. Field-state algorithm (ENG, `setup::view`; the UI never computes a state)

Required rows: `goal, repo, metric, bench, checks, scope`. Optional rows: `holdout, window, secondary_guards, warm, evaluator_paths`. Default-able rows: `budget, parallel, models`.

Per row, first rule that matches wins:
1. **missing** — required and absent: `goal` empty; `repo` empty; `metric` → `metric.name` empty; `bench` → `bench.command` empty; `checks` → `None` or empty command; `scope` → empty vec.
2. **optional** — optional row and absent (`holdout None/empty`, `window None/empty`, `secondary_guards []`, `warm None/empty`, `evaluator_paths []`).
3. **verified** — `repo`: `is_git_repo(repo)` and `resolve_base(repo, base, true)` Ok (evidence `<sha8> · clean tree` or `<sha8> · N uncommitted files`); `bench` / `checks` / `holdout`: `probe.json` has that kind, `ok`, and `fingerprint == fingerprint(kind, spec, base_sha)` (evidence from the result: `5 runs · 412 ms · ±1.6%`, `passed in 1 m 50 s`). Verified implies the row counts as settled regardless of `confirmed`.
4. **confirmed** — row ∈ `confirmed ∪ user_set`.
5. **default** — `budget == BudgetSpec::default()` / `parallel == 1` / `models == ModelsSpec::default()` and row ∉ `user_set ∪ confirmed`.
6. **proposed** — everything else that is present.

`settled` = main rows (9) whose state ∉ {proposed, missing}; `total` = 9. `ready` = no required row `missing`/`proposed`, `window` not `proposed`, and `validate_spec(&spec)` Ok; `blocking` lists each reason in row order (`"<Label> needs your OK"`, `"<Label> is missing"`, the `validate_spec` message). `dirty` = `resolve_base(repo, base, false)` Err while `(…, true)` Ok. `started` = `store.get_study(slug)` is Some. **Sticky**: `write_draft` from the tool compares the new spec with the old one row by row (serde_json of the row's leaves); a changed row ∈ `user_set` is removed from `confirmed` (stays in `user_set`) and named in the tool result (§4.6). `draft_edit` adds the row to both `user_set` and `confirmed`; `draft_confirm` adds to `confirmed` (refused for `missing`). `DraftField.value` is the engine's one-line rendering (`study_draft::lines`-style, e.g. metric `startup_ms · lower · keep ≥ 3%`).

## 6. Engine (ENG)

### 6.1 Validation
`validate_spec` additionally refuses slug `new` (route collision, §4.7). `validate_partial(&spec)`: slug rule + `repo` non-empty absolute path; nothing else. `autoresearch_draft` with `partial: true` uses it, writes the draft, publishes the compact card; `partial: false` is today's path unchanged plus the sticky check.

### 6.2 Probe (`setup::probe(root, cfg, draft_id, what, repeats, cancel) -> Result<ProbeResult>`, called by `ProbeTool`)
1. `refuse_study_sessions(cx.session)`; `read_draft`; `what`'s command present else refused ("no `<kind>.command` in the draft").
2. `base_sha = resolve_base(repo, base, true)`; `wt = drafts_dir/probe/wt`; `worktree_remove(repo, wt)` if left over; `worktree_add_detached(repo, wt, base_sha)`. Never the user's checkout; detached, no branch, no commit.
3. `warm` once through `runner::command` (timeout `bench.timeout`), failure = `ok: false`.
4. `SampleRequest { slug: draft_id, n: 0, k, tree_sha: base_sha, role: SampleRole::Baseline, worktree: wt, evaluator: drafts_dir (AUTORESEARCH_EVALUATOR = the scripts dir, where the skill puts them), runs_dir: drafts_dir/probe/runs }`.
   - `bench`: `repeats` = arg or `spec.bench.repeats`, clamped 3..=10; `runner::sample` each (exclusive slot + load gate honoured by `sample`); crash → `ok: false`, `exit`, `log_tail`.
   - `check`: `runner::command(checks.command, checks.timeout, req, cancel)` once; `ok = exit == 0`.
   - `holdout`: `sample` with a spec clone whose `bench.command = holdout.command`, `holdout.repeats.unwrap_or(bench.repeats)` samples.
5. `values` = non-crash samples; `median`; `noise_pct = 100 × gate::pooled_sigma(&[values]) / mean.abs()` (the baseline's estimator, `actor.rs:1829-1833`); `unit = metric.unit`; `fingerprint = sha256(json!({kind, repo, base_sha, warm, bench: {command, timeout, repeats, exclusive, smoke}, metric_name, checks_command, holdout: {command, repeats}}))` — one function `setup::fingerprint(kind, spec, base_sha)`, same inputs for every kind (a changed bench invalidates all three; cheap and honest).
6. **Cleanup on every path** (ok, error, cancel): `worktree_remove(repo, wt)`; a `scopeguard`-style `Drop` struct, no `?` before it is armed.
7. Write `probe.json` (merge by kind), return. Tool text: `bench: 5 samples · median 412 ms · noise ±1.6% · smallest detectable ≈ 3.2%` (= `cfg.gate.z × cv`), `check: passed in 1 m 50 s` / `check: failed (exit 1) in 12 s\n<tail>`, `holdout: …` like bench. Long runs promote to a job (`ToolCx.promote_after`) and die with `cx.cancel`.

### 6.3 Drafts list, edit, confirm, discard (`rpc_impl`, plain fns like `rpc_impl.rs:46`)
Root = `ask(Settings).await?.root(data_dir)` — `data_dir` comes from the actor too (`Studies` holds `root`; ENG adds `Message<DraftRoot>` → `PathBuf` or reads it from `Settings` + `Seams`; pick the former). `drafts()` scans `<root>/drafts/*/spec.json`, skips slugs with a `StudyRecord`, builds `DraftSummary` from `view`. `draft_edit` parses `value` per leaf (§4.3), `validate_partial`, writes, returns `view`. `discard_draft` removes `drafts_dir(root, slug)` recursively; later `read_draft` → "no draft" refuses Starts from the stale card. ponytail: file writes are unsynchronised between the tool and the RPC; one user, last write wins.

### 6.4 Compact card (`study_draft.rs`)
`partial: true` → `source_compact(slug, settled, total)`; tool text: `drafted study `<slug>` (partial, v<version>): <settled> of <total> settled; missing: <rows>; needs OK: <rows>; <sticky notes>. Keep talking; re-draft with confirmed: […] after each settled answer; run autoresearch_probe after writing or changing bench/check/holdout; draft without partial when everything is settled.`

### 6.5 Skill (`crates/autoresearch/skill.md`, D8)
New section "Setup in the Autoresearch tab" after "The draft": the seed marker means the user is in the tab; draft early with `partial: true`; re-draft after each settled answer listing `confirmed`; probe after writing/changing bench/check/holdout and never state a number the probe did not produce; `[study draft <slug>] …` messages are the user's own panel edits and win; final draft without `partial` when everything is settled; after Start this chat steers the study.

## 7. After Start (D7)

- `StudyInfo.origin` / `StudyDetail.info.origin` populated (C committed).
- UI: `StudyView.tsx:68-80` header gains `Setup chat ›` when `info.origin`: `/autoresearch/new?chat=<origin>`. `DraftView.started` → `DraftPanel` renders the status card from `useStudy(draftId)`: state/phase, experiments / budget, best vs base (`delta_pct`), Pause · Stop (`useControl`) · Open study (`/autoresearch/$slug`). The pane header shows the `StatePill` instead of "Setting up with theuth".
- Rail (`StudyList.tsx`): drafts from `useDrafts()` first — dashed amber dot, `Setting up · n of m settled`, click → `/autoresearch/new?chat=<origin>`; then studies as today.

## 8. Ownership

| file | owner |
|---|---|
| `crates/autoresearch/src/{setup.rs (new), tools.rs, types.rs, rpc_impl.rs, actor.rs, lib.rs, git.rs}`, `crates/autoresearch/skill.md`, `crates/app/src/study_draft.rs`, `crates/rpc/src/api.rs` | ENG |
| `crates/desktop/ui/src/components/autoresearch/*`, `routes/autoresearch*.tsx`, `routes/__root.tsx` (one mount line), `rpc/api.ts` (only if a method's TS type changes; C committed the five), `rpc/bindings/` (regenerated, never hand-edited) | UI |
| `docs/design/151-as-built-ENG.md`, `151-as-built-UI.md` | each its own |

No file has two owners. The TS bindings are regenerated by UI from ENG's committed types; a Rust type change after C's contract is an ENG as-built deviation UI reads before regenerating.

## 9. Test plan

**ENG** (`mbx test -p autoresearch`, `-p app --lib`, `-p rpc --lib`): `setup::tests::field_states_follow_the_rules` (each rule once: missing, optional, verified repo via a tempdir git repo, confirmed, default, proposed; `settled`/`total`/`ready`/`blocking`); `setup::tests::user_set_rows_are_sticky` (a re-draft changing a user-set row drops it to proposed and the note is in the text); `setup::tests::probe_runs_in_a_detached_scratch_worktree_and_cleans_up` (toy-sort fixture: `probe/` gone after ok and after a crashing command, user checkout untouched, fingerprint changes with `bench.command`); `setup::tests::fingerprint_mismatch_drops_verified`; `tools::tests::probe_is_refused_in_a_study_session`; `rpc_impl::tests::drafts_skip_started_and_discarded`; `study_draft::tests::partial_draft_renders_the_compact_card` and `validate_partial_accepts_a_bare_spec`; `validate_spec` refuses slug `new`; `draft_edit` leaf grammar (`setup::tests::edit_parses_every_leaf`).

**UI** (`bunx vitest run src/components/autoresearch src/rpc`): `setup.test.ts` — `startSetupChat` call order with a mocked api (`new_session({cwd})` → `rename_session` → `use_skill` seed text), `latestStudyDraft` picks the newest `autoresearch_draft` args, `lastSetupResultId` changes on a new probe result, `editMessage` text; `DraftPanel.test.tsx` — badge per state, Start disabled while not ready with the footer text, status card when `started`; `api.test.ts` still green (METHODS = `methods.json`).

## 10. Live proof (closer)

Scratch engine with `autoresearch:` enabled; repo = a copy of `crates/autoresearch/fixtures/toy-sort` initialised as a git repo with one commit. Click path: Autoresearch tab → `+ New study` → repo path, goal "sort faster" → Start setup chat → agent drafts partial (panel fills, badges) → agent writes `bench.sh` under `drafts/<slug>/`, runs `autoresearch_probe bench` → Bench row flips to Verified with the sparkline → OK on Scope → Edit Budget in the panel (message appears in the chat) → Start now → panel flips to the status card; study header shows `Setup chat ›`. Screenshots: (1) intro box, (2) panel mid-setup with a Verified row and a Needs-your-OK row, (3) after Start status card, (4) study page header with the link, (5) phone width: sheet + `Draft n of m` pill. Prove `ls <root>/drafts/<slug>/` has no `probe/` after the probe and `git -C <repo> status --porcelain` is empty.

## 11. Decisions made here (none open)

1. Rows vs leaves (§4.3): states/confirmations are per row (what the user sees), edits are per leaf (what the user types); the mapping is two exceptions. Alternative (JSON `value`) rejected: the panel edits text.
2. `draft_edit.value: String` with a per-leaf grammar — one parser in ENG, no form schema in UI.
3. Discard deletes the directory; "refuses later Starts" falls out of `read_draft`. No `discarded` flag to keep in sync.
4. The probe runs inside the tool call (job promotion + `cx.cancel`), not as an actor message: the only caller is the agent; cancel = cancel the turn.
5. One fingerprint over every probe-relevant input for all three kinds (a changed bench drops check's Verified too) — simpler than three partial hashes, and re-probing a check is cheap.
6. `DraftView.started` instead of a `study` object: the UI already has `useStudy(slug)`; slug = draft_id.
7. Slug `new` is reserved by `validate_spec` (static route beats `$slug`).
8. Recent repositories are derived client-side from list ∪ drafts; no RPC.
9. The compact card's "Open in Autoresearch" is `@ToAssistant` + the root bridge (108 §6.5 pattern), since OpenUI has no in-app navigation action.
10. `autoresearch.discard_draft` keeps the decisions' name (verb-first like `settings.set`); the other four are `draft*`.

## 12. Contract as built (C, branch `feat/151-c`)

Committed so ENG and UI compile against it from minute one. Exact names:

- `crates/autoresearch/src/types.rs` — new section "151 setup": `FieldState` (ts `DraftFieldState`, snake_case, default `Proposed`), `DraftField`, `DraftView`, `DraftSummary`, `ProbeKind` (snake_case, default `Bench`), `ProbeResult` (`duration_ms` ts `number`), `DraftArgs` (ts `StudyDraftArgs`; `#[serde(flatten)] spec: StudySpec` + `partial` + `confirmed`, all defaulted). `StudyInfo.origin: Option<SessionId>` added after `session`; `actor::info_from` copies `study.origin` (`StudyDetail` carries it as `info.origin`). Test `types::tests::draft_args_flatten_the_spec_with_default_flags`.
- `crates/autoresearch/src/tools.rs` — `Draft` gains `partial, confirmed, user_set, version, updated_at` (all `#[serde(default)]`; old files read); `write_draft(root, spec, origin, partial, confirmed)` keeps `user_set`, bumps `version` (1 on first write), stamps `updated_at = store::now()`; `register` adds `setup::ProbeTool`. Test `draft_spec_round_trips_on_disk_with_a_press_token` extended (version 1 → 2 → 3, `partial`/`confirmed` stored, `user_set` kept).
- `crates/autoresearch/src/setup.rs` (new, `pub mod setup`) — `NOT_BUILT = "151: not built yet"`, `ProbeArgs { draft_id, what: ProbeKind, repeats: Option<u8> }`, `ProbeTool` (`autoresearch_probe`, `Risk::Mutating`, `RETURNS "dict"`, the D4 description) whose `call` bails with `NOT_BUILT`. ENG fills this file (§5–§6).
- `crates/autoresearch/src/rpc_impl.rs` — `METHODS` is 15; `draft`, `drafts` (returns `Ok(vec![])`), `draft_edit`, `draft_confirm`, `discard_draft` return `Err("… 151: not built yet")`.
- `crates/rpc/src/api.rs` — trait methods `autoresearch.draft | drafts | draft_edit | draft_confirm | discard_draft` and bodies after `autoresearch_create`; `drafts` maps "is not running" to an empty list like `list`. `autoresearch_methods_are_registered` covers them. `crates/rpc/src/export.rs` exports the seven new types.
- `crates/app/src/study_draft.rs` — `type Args = DraftArgs`; `partial` / `confirmed` are destructured and passed to `write_draft`; validation and the card are unchanged (ENG: §6.1, §6.4).
- `crates/desktop/ui/src/rpc/api.ts` — the five methods on `Api` (types from `@/rpc/bindings/{DraftView,DraftSummary}`; `discard_draft` → `Promise<null>`) and in `METHODS` (sorted). `components/autoresearch/fixtures.ts` — the fixture `StudyInfo` gains `origin: \`setup-<slug>\``. Bindings regenerated (816 files; `bindings/methods.json` has 15 `autoresearch.*`).

Deviations from decisions.md / the brief: (1) `origin` lives on `StudyInfo` only — `StudyDetail.info.origin` is the detail's copy; a second field would duplicate it. (2) `drafts()` answers `[]` from `rpc_impl` directly (no actor ask) until ENG scans the directory, so the rail works with the plugin off. (3) The probe tool's refusal text is an `anyhow` error (the tool-call failure path), not `ToolOutput::err`, so the agent sees it as a failed call rather than a result.

Environment notes for the worktree (not code): `crates/desktop/ui/node_modules` had a stray `node_modules/node_modules -> ~/Develop/theuth/…/node_modules` symlink from the APFS clone that made Node resolve a second React for `@tanstack/react-query` (9 vitest failures); removed. `@types/d3-sankey` (package.json) was absent from the clone; `build-lock bun install` added it (lockfile unchanged).

Gates (stable 1.96.1, `build-lock mbx`, `CARGO_BUILD_JOBS=4`, crate-level): `mbx test -p autoresearch` 161 passed · `mbx test -p app --lib` 46 passed · `mbx test -p rpc --lib` 55 passed, 1 failed = `engine::tests::second_acquire_in_one_process_is_running` (engine-lock file test, unrelated; passes alone 4/4 on rerun — parallel-load flake) · `mbx clippy -p autoresearch -p rpc -p app --all-targets -- -D warnings` clean · `mbx check -p theuth` green (1 m 13 s) · `export_bindings` 8 passed, 816 bindings copied · `bunx tsc -b --noEmit` 0 errors · `bunx vitest run src/components/autoresearch src/rpc` 4 files, 30 passed.
