<!-- 159 recon sheet A: read-only seam inventory of crates/autoresearch, crates/dreaming and the autoresearch UI, written by the recon child ar-variants-seams on 2026-10-10 13:12 over main (~c22556ea). The orchestrator re-opened 10 load-bearing cites at 13:40: all held. Re-open every line you cite in this worktree; main has moved to f49ec7a3. -->

# Autoresearch variants — seam inventory

Read-only pass over `main` in /Users/Sky.Moore/Develop/theuth. Every cite is a range I read or a grep/outline hit. "(outline)" means I only have the symbol's location from `code_outline` and did not read its body.

## A. Architecture now

- **Crate and modules.** The design 140 plugin is `crates/autoresearch` (crates/autoresearch/Cargo.toml#L1-L6). Its modules, with their wave owners:
  - C: config, types, store, rpc_impl.
  - W1: runner, scanner, gate, custody, slot.
  - W2: git, finalize, record.
  - W3: actor, binding, schedule_file, stop.
  - W6: tools, brief, novelty, review, skill.md.
  - W5: wave.
  - Source: crates/autoresearch/src/lib.rs#L1-L14. setup.rs is the 151 engine side (crates/autoresearch/src/setup.rs#L1-L4).
  - The `Studies` actor holds `store`, `cfg` and a `Backend` seam that tests swap for doubles (crates/autoresearch/src/lib.rs#L93-L112).
  - types.rs is the only source of wire types: "a new field is a change here" (crates/autoresearch/src/types.rs#L1-L6).
- **Study record and config.**
  - `StudySpec` has: slug, goal, repo, base, one `metric: MetricSpec`, `secondary_guards`, bench, checks, holdout, evaluator_paths, scope, off_limits, warm, budget, `parallel`, models, window, include_dirty (crates/autoresearch/src/types.rs#L36-L79).
  - `MetricSpec` is name / unit / direction / min_effect (crates/autoresearch/src/types.rs#L106-L120).
  - The `autoresearch:` config section is crates/autoresearch/src/config.rs#L21-L61. Its defaults: max_parallel 4, confirm_budget_usd 50 (crates/autoresearch/src/config.rs#L242-L265).
- **Ledger.** `autoresearch.sqlite`, with JSON columns holding the serde form of the types (crates/autoresearch/migrations/0001_init.sql#L1-L2). Migrations run through `sqlx::migrate!` (crates/autoresearch/src/store.rs#L846-L846). Tables:
  - `studies` (L4-L28): includes `incumbent_sha`, `incumbent_mean`, `sigma`, `segment`, `playbook`.
  - `segments` (L30-L40): custody hashes and `baseline_mean`.
  - `experiments` (L42-L74):
    - `wave` and `slot` (L45-L46);
    - **`parent_sha`** (L48);
    - **`category`** (L52);
    - `delta`, `delta_pct`, `z`, `metric REAL` (L60-L64);
    - **`patch_hash`** (L67);
    - `cost_nanos` (L70).
  - `samples` (L76-L96): `value REAL`, **`secondaries` and `cases` as JSON maps** (L83-L86).
  - `ideas` (L98-L109), `steers` (L111-L119), `events` (L121-L128).
  - FTS5 over idea, description and learned (L130-L145).
  - Migration 0002 adds `slot_holders` and `failure` (crates/autoresearch/migrations/0002_sample_detail.sql#L1-L4).
- **METRIC/CASE parsing.**
  - The line grammar is `METRIC`, `CASE <id> <name>=`, `ASI`; last occurrence wins; nan or inf counts as a crash (crates/autoresearch/src/scanner.rs#L1-L4).
  - **CASE drops `<name>` and keeps one number per id** (`_name`, crates/autoresearch/src/scanner.rs#L92-L106).
  - The runner pulls the primary out by `spec.metric.name`; every other METRIC becomes a secondary (crates/autoresearch/src/runner.rs#L94-L117).
- **Noise gate.**
  - Threshold: `T = max(z σ √(1/n_c+1/n_i), min_effect·|x̄_i|)` (crates/autoresearch/src/gate.rs#L1-L5).
  - σ: pooled within-tree, switching to the robust pairwise-median form from 12 repeated samples (crates/autoresearch/src/gate.rs#L9-L49).
  - Deterministic metrics (σ=0) skip the z-tests (L129-L138).
  - One-sample path: z_fast / z_discard (L150-L172).
  - Confirm phase: `confirm_runs` more samples, plus an incumbent re-measure (L119-L127, L140-L149).
  - Suspicious rule: z ≥ z_suspicious or more than 3× the largest earlier step (L84-L105).
  - Drift: a re-measure more than 3σ off (L226-L229).
  - Defaults: z 2.0, z_fast 3.5, z_discard 1.0, z_suspicious 8.0, confirm_runs 2, remeasure_after 6, remeasure_every 5, regime_sigma 10 (crates/autoresearch/src/config.rs#L280-L293). Baseline noise must stay under max_cv 0.05 (crates/autoresearch/src/config.rs#L295-L299).
  - Regime change: a keep with |Δ| ≥ regime_sigma·σ opens a new segment (crates/autoresearch/src/actor.rs#L4572-L4606).
- **Guards.** Each secondary is judged in its own direction against `max_regression` (crates/autoresearch/src/gate.rs#L183-L202; crates/autoresearch/src/types.rs#L135-L148).
- **Holdout.**
  - `command`, `every`, `repeats` (crates/autoresearch/src/types.rs#L199-L211).
  - Its numbers "never the brief" (crates/autoresearch/src/actor.rs#L932-L973).
  - Overfit = holdout more than 2σ worse while the search metric improved; it is recorded as an event (crates/autoresearch/src/actor.rs#L975-L1033).
- **Reviewer.**
  - One SideCall per keep with a CLEAN/SUSPICIOUS rubric, a 40 KiB diff cap and 180 s per call. It fails open (crates/autoresearch/src/review.rs#L1-L28, L71-L89).
  - `on_suspicious` defaults to revert (crates/autoresearch/src/config.rs#L142-L158, L316-L324).
- **Waves.** N candidate worktrees cut from the incumbent, with distinct categories per slot (crates/autoresearch/src/wave.rs#L1-L13). Promotion sorts by Δ then z, cherry-picks disjoint winners and re-confirms them (crates/autoresearch/src/wave.rs#L595-L671).
- **Budgets and spend — units are dollars, not tokens.**
  - `BudgetSpec` is usd 20, per_experiment_usd 1.5, hours 10, experiments 120 (crates/autoresearch/src/types.rs#L213-L237).
  - Spend is the session tree's model-call cost via `SessionCostQuery`, keeping only `currency == "USD"`, in nanos/1e9 (crates/autoresearch/src/actor.rs#L446-L467).
  - Per-experiment cost is stored as `experiments.cost_nanos` (0001_init.sql#L70).
  - The per-fire cap is written to the schedule as `budget: <n> USD` (crates/autoresearch/src/schedule_file.rs#L38-L49, L76-L79).
  - The money stop is `spend >= budget.usd` (crates/autoresearch/src/actor.rs#L2085, grep hit).
- **Schedule-driven loop.**
  - One file `autoresearch-<slug>.md` with: `every: 0s` (or `every: 1m` + `when:` for a window), `max_runs = experiments + 10`, `run_timeout`, `approvals: false`, `quiet: true`, `runtime: clear`, `cwd: <worktree>` (crates/autoresearch/src/schedule_file.rs#L51-L108).
  - `run_timeout = (crash_retries + confirm_runs + 2) × bench.timeout + checks + bench.wait + 15m`, with a 30 m floor (L115-L124).
  - `Fired` binds the session (crates/autoresearch/src/binding.rs#L117-L140).
  - `TurnFinished` closes a dangling experiment as abandoned, interrupted or timeout (crates/autoresearch/src/binding.rs#L170-L200).
- **The toolset at fire.** Exactly 19 tools: the four experiment_* tools, read_file, grep, glob, edit, write_file, list_dir, shell, the five code_* tools, knowledge_search, knowledge_get, todo. `spawn_subagent` is added only when `parallel > 1`. Settings: `runtime: false`, `hint_limit: Some(0)`, prompts autoresearch / codeindex / knowledge (crates/autoresearch/src/binding.rs#L25-L70).
  - Only named registry tools are offered, and `runtime:false` drops python (crates/agent/src/lib.rs#L604-L621).
  - **So there is no `search_web`, `read_url` (python runtime functions) or MCP tool.**
  - Writes are confined to the worktree (crates/autoresearch/src/binding.rs#L72-L115).
- **Bench timeouts and network.**
  - Bench default 600 s per sample; checks 300 s (crates/autoresearch/src/types.rs#L167-L196).
  - Runs as `/bin/bash -lc` in the worktree in its own process group, TERM then KILL at the timeout, with `AUTORESEARCH_*` env and `GIT_TERMINAL_PROMPT=0` (crates/autoresearch/src/runner.rs#L207-L264). No network isolation appears in that code.
  - "No network" is guidance only (crates/autoresearch/skill.md#L46).
  - Studies are refused on Windows (crates/autoresearch/src/lib.rs#L60-L63).
- **Surfaces.**
  - Study view: StudyView / StudyChart / RunTable / Explorer under crates/desktop/ui/src/components/autoresearch (glob). The chart has one series per metric (crates/desktop/ui/src/components/autoresearch/StudyChart.tsx#L10-L38).
  - Draft card: "the press, not the model, starts the study" (crates/app/src/study_draft.rs#L1-L14).
  - Setup chat: crates/desktop/ui/src/components/autoresearch/SetupChat.tsx#L13-L14.
  - Probe: the `autoresearch_probe` tool (crates/autoresearch/src/setup.rs#L1070; crates/autoresearch/src/tools.rs#L89-L91), body at crates/autoresearch/src/setup.rs#L843-L991.
  - RPC: 15 methods (crates/autoresearch/src/rpc_impl.rs#L13-L30; crates/rpc/src/api.rs#L1942-L2017).
  - TUI: `/autoresearch` (crates/tui/src/model.rs#L427-L431, L7225-L7260; crates/tui/src/lib.rs#L589-L623).
  - Skill: binary-owned, rewritten whenever the text differs (crates/autoresearch/src/lib.rs#L64-L91).

## B. Per wave

### C — contract wave
- **Seams:**
  - The `wire!` types with ts export (crates/autoresearch/src/types.rs#L13-L27; crates/autoresearch/Cargo.toml#L41-L42).
  - `GateCfg`, `GateInput`, `GateStep` (crates/autoresearch/src/types.rs#L566-L629).
  - Migrations (crates/autoresearch/src/store.rs#L846).
  - RPC `METHODS` (crates/autoresearch/src/rpc_impl.rs#L13-L30).
  - Setup `LEAVES` and `MAIN_ROWS` (crates/autoresearch/src/setup.rs#L28-L75).
  - `validate_spec` (crates/autoresearch/src/tools.rs#L244-L295, outline).
- **Reuse:**
  - The ALTER-TABLE-ADD-COLUMN pattern (0002_sample_detail.sql#L3-L4).
  - `#[serde(default)]` on `StudySpec`, so old spec JSON still loads (crates/autoresearch/src/types.rs#L39).
  - The `Backend` test seam (crates/autoresearch/src/lib.rs#L110-L111).
- **Add:**
  - A study `kind` / objective enum in `StudySpec`.
  - A generalised `GateStep::Decide` (today it holds one Δ, one Δ%, one z: types.rs#L619-L628).
  - Per-objective columns or a JSON vector beside `incumbent_mean`.
  - New RPC shapes and regenerated TS bindings.
- **Hardest constraint:** the `delta_pct` convention is "the one convention the ledger, brief, trailer, headline and UI share" (crates/autoresearch/src/gate.rs#L72-L75). Generalising it touches every surface in D.

### W1 — several objectives (Pareto)
- **Seams:**
  - `gate::step` and `guard_failures` (crates/autoresearch/src/gate.rs#L59-L202).
  - `gate_cfg`, which takes a single direction and min_effect (crates/autoresearch/src/actor.rs#L569-L586).
  - `promote_env`'s Δ/z sort (crates/autoresearch/src/wave.rs#L595-L601).
  - `ChartSeries` (crates/autoresearch/src/types.rs#L1424-L1441).
- **Reuse:**
  - Secondaries are already stored per sample (crates/autoresearch/src/types.rs#L535-L536; 0001_init.sql#L84).
  - Guards already carry their own directions (crates/autoresearch/src/gate.rs#L183-L202).
  - `pooled_sigma` works on any vector per tree (crates/autoresearch/src/gate.rs#L17-L49).
- **Add:** per-objective σ, a noise-aware dominance test, a front (archive) of incumbents, and a 2-D front chart.
- **Hardest constraint:** the ratchet has one incumbent — `studies.incumbent_sha` plus one `incumbent_mean` (0001_init.sql#L16-L17; `Incumbent` crates/autoresearch/src/types.rs#L817-L829) — and promotion cherry-picks onto one study branch (crates/autoresearch/src/wave.rs#L635-L671).

### W2 — prompt/skill studies, per case
- **Seams:**
  - The CASE scan (crates/autoresearch/src/scanner.rs#L92-L106).
  - `Sample.cases` (crates/autoresearch/src/types.rs#L537-L538).
  - `cases_moved`: top 5, last sample only (crates/autoresearch/src/actor.rs#L3038-L3071).
  - `CaseDelta` (crates/autoresearch/src/types.rs#L733-L747).
  - The brief's "cases moved" (crates/autoresearch/src/brief.rs#L1-L5).
- **Reuse:**
  - `samples.cases` JSON (0001_init.sql#L85).
  - The holdout as held-out cases (crates/autoresearch/src/types.rs#L199-L211).
  - Dreaming's paired gate, judge and split (section C below).
- **Add:**
  - A paired per-case gate.
  - A failing-case feed into the brief for reflective rewrites (today only 5 moved cases).
  - A case-set custody.
  - Bench-side model calls charged to the study.
- **Hardest constraint:** CASE keeps one number per id with its name dropped (scanner.rs#L97). The gate decides on the primary's mean only (crates/autoresearch/src/gate.rs#L60-L75). Spend counts the session's model calls only (crates/autoresearch/src/actor.rs#L446-L467), so LLM calls made inside a bash bench are **invisible to the budget**.

### W3 — judged objectives (no scalar)
- **Seams:**
  - `Verdict` (crates/autoresearch/src/types.rs#L452-L499).
  - The reviewer SideCall (crates/autoresearch/src/review.rs#L1-L8).
  - Sample value extraction (crates/autoresearch/src/runner.rs#L94-L109).
- **Reuse:**
  - SideCalls charged to the study session (crates/autoresearch/src/review.rs#L7-L8).
  - Parsing a one-line verdict (crates/autoresearch/src/review.rs#L71-L89).
  - The model roles (`ModelsSpec` crates/autoresearch/src/types.rs#L239-L251).
  - Dreaming's 0–1 judge prompt (crates/dreaming/src/replay.rs#L24-L25).
- **Add:** a blind pairwise runner with order randomisation, a ratings table (Elo / Bradley–Terry with a CI), and a decision rule that replaces the z test.
- **Hardest constraint:** a sample is only valid with a numeric `METRIC <name>` line (crates/autoresearch/src/runner.rs#L95-L109). The skill tells setup to *decline* judge-only metrics without a numeric proxy and a holdout (crates/autoresearch/skill.md#L18).

### W4 — population search
- **Seams:**
  - `experiments.parent_sha`, `category`, `patch_hash` (0001_init.sql#L48, L52, L67).
  - `cand_dir` (crates/autoresearch/src/wave.rs#L56-L59).
  - `assign_categories` (crates/autoresearch/src/wave.rs#L305-L353, outline).
  - The novelty check (crates/autoresearch/src/novelty.rs#L1-L10).
- **Reuse:**
  - `parent_sha` is already rewritten when combining (crates/autoresearch/src/wave.rs#L648-L652).
  - Patch-hash dedup against discarded experiments, and FTS similarity (crates/autoresearch/src/novelty.rs#L1-L10; 0001_init.sql#L130-L145).
- **Add:** an archive / cells table, parent selection that is not always the incumbent, behaviour descriptors (from secondaries or ASI), and behavioural novelty rejection.
- **Hardest constraint:**
  - Slots are cut "from the incumbent" (crates/autoresearch/src/wave.rs#L1-L2), and the gate compares against "the incumbent's samples" (crates/autoresearch/src/types.rs#L594-L595).
  - `max_parallel` defaults to 4 (crates/autoresearch/src/config.rs#L258).
  - `exclusive` takes the machine bench slot, which serialises measuring (crates/autoresearch/src/types.rs#L160-L161).

### W5 — ideas from the literature, and hooks
- **Seams:**
  - `TOOLS` / `toolset` (crates/autoresearch/src/binding.rs#L25-L70).
  - `IdeaSource`: User / Model / Advisor / Setup (crates/autoresearch/src/types.rs#L1079-L1091).
  - The `ideas` table (0001_init.sql#L98-L109).
  - The advisor, 5 ideas (crates/autoresearch/src/review.rs#L1-L5, L25-L26).
- **Reuse:**
  - The `autoresearch.idea` RPC (crates/autoresearch/src/rpc_impl.rs#L21).
  - `warm` and `smoke` as the only pre-steps today (crates/autoresearch/src/types.rs#L65-L66, L162-L163).
- **Add:** a citation column or source, a literature IdeaSource, a plugin-side search call (or direct web tools), and before/after hook commands under custody.
- **Hardest constraint:** `search_web` and `read_url` are python runtime functions, and the study toolset sets `runtime: false` and lists none of them (crates/autoresearch/src/binding.rs#L54-L70; crates/agent/src/lib.rs#L604-L621). `hint_limit: Some(0)` also removes knowledge hints.

### W6 — real-world metrics
- **Seams:**
  - The schedule text: `every: 0s`, `approvals: false` (crates/autoresearch/src/schedule_file.rs#L66-L71, L84).
  - `run_timeout` (L115-L124).
  - `ExperimentState`, which has no waiting state (crates/autoresearch/src/types.rs#L400-L430).
  - `on_turn_finished` (crates/autoresearch/src/binding.rs#L170-L200).
  - The hours budget, "wall hours since Start" (crates/autoresearch/src/types.rs#L221-L222).
- **Reuse:**
  - `when:` sensor windows (crates/autoresearch/src/schedule_file.rs#L60-L71, L98-L100).
  - `PauseReason::BudgetConfirm` (crates/autoresearch/src/types.rs#L358-L359).
  - The press-starts invariant as the model for per-launch approval (crates/app/src/study_draft.rs#L5-L6).
  - Segments and the overfit check (crates/autoresearch/src/actor.rs#L975-L1033).
- **Add:** deploy and measure as separate fires, a pending-measurement state, sequential (always-valid) tests, exposure caps, MCP-backed measurement, and a non-model money ledger.
- **Hardest constraint:** a measurement is one bash sample killed at `bench.timeout` inside one fire (crates/autoresearch/src/runner.rs#L207-L264). A turn that ends with the experiment open closes it as abandoned or timeout (crates/autoresearch/src/binding.rs#L170-L200). Every study file is written `approvals: false` (crates/autoresearch/src/schedule_file.rs#L84).

### W7 — the loop improves the loop
- **Seams:**
  - `PROMPT_BLOCK` (crates/autoresearch/src/tools.rs#L46-L54).
  - The fire prompt (crates/autoresearch/src/schedule_file.rs#L102-L106).
  - The brief template (crates/autoresearch/src/brief.rs#L95-L102).
  - `studies.playbook` (0001_init.sql#L20) and its cap (crates/autoresearch/src/config.rs#L197-L198).
  - The gate (crates/autoresearch/src/gate.rs#L59-L181).
- **Reuse:**
  - The `fixtures/toy-sort` fixture (list_dir of crates/autoresearch/fixtures).
  - Gate simulation tests `simulate` / `null_false_keep_rate_at_most_one_percent` (crates/autoresearch/src/gate.rs#L387-L449, outline).
  - `evaluator_paths` and `off_limits` (crates/autoresearch/src/types.rs#L59-L64).
- **Add:** the inner playbook and prompts as data files, a fixture-suite bench, and gate/custody/scoring files listed as evaluator or off-limits.
- **Hardest constraint:** the inner mechanism is compiled into the binary — prompts are `const`, and the skill is reinstalled whenever it differs (crates/autoresearch/src/lib.rs#L79-L91). Changing it means a rebuild and engine restart, and the same `gate.rs` judges the outer study.

## C. Dreaming
- **Where cases live.**
  - `<data_dir>/dreaming/cases.jsonl`, an append-only log folded by id (crates/dreaming/src/cases.rs#L1-L6, L26-L29).
  - `Case` fields: intent, context ≤ 1,200 chars, happened, user_words, ideal, skill_hint, split, origin, state (crates/dreaming/src/types.rs#L83-L121).
  - Kinds (L18-L38); `Split` Train/Val (L40-L49); `CaseOrigin` Real/Synthetic (L51-L63); `CaseState` Open / Closed / Reopened / Settled (L65-L81).
  - Split rule: `sha256("136"+id) % 100 < 34` → val; synthetic cases are never val (crates/dreaming/src/cases.rs#L17-L24, L41-L54).
- **replay_case.**
  - Read-only, but it spends budget (crates/dreaming/src/tools.rs#L444-L490).
  - It answers in a fresh SideCall (`CallKind::Dream`) and then the judge tier scores 0–1. The judge never sees the proposer's rationale (crates/dreaming/src/replay.rs#L1-L11, L24-L28).
  - Each replay is persisted as a `ReplayRow`, keyed by `candidate_sha256` (crates/dreaming/src/replay.rs#L41-L56).
- **skill_learned_edit.**
  - At most 4 bounded edits, in two stages: candidate (in memory) and record (crates/dreaming/src/tools.rs#L378-L442).
  - The record pass recomputes the gate from the replay rows, refuses when no replays were logged, and lets the rows win over the caller's claim. It writes only when accepted, backing up first (crates/dreaming/src/edits.rs#L499-L539).
  - The gate pairs the latest baseline and candidate per case (crates/dreaming/src/replay.rs#L97-L132). It accepts iff mean Δ ≥ 0.1 and min Δ ≥ −0.2 over at least 3 pairs (crates/dreaming/src/replay.rs#L287-L325; crates/dreaming/src/config.rs#L175-L181, L258-L261).
  - **The pairing code does not read `split`** (replay.rs#L106-L131). The val-only rule is in the skill text (crates/dreaming/src/skill.md#L87).
- **Who may write.** Mutations are allowed only in the dreaming session or the owner's root, never in a private session (crates/dreaming/src/tools.rs#L35-L51).
- **What a prompt study could reuse directly:** the judge prompt, the 0–1 parse, `gate()` over paired case scores, the stable split, replay rows keyed by candidate hash, and the bounded edit ops with backup.
- **What would conflict:**
  - A study session fails `may_consolidate` (crates/dreaming/src/tools.rs#L42-L48).
  - Two gates, the z-gate and the margin gate, would judge the same artefact.
  - Spend would be split between `CallKind::Dream` and `CallKind::Autoresearch` (crates/dreaming/src/replay.rs#L3-L4; crates/autoresearch/src/review.rs#L7-L8).
  - The study write root is a git worktree (crates/autoresearch/src/binding.rs#L72-L100), while sidecars live in `skills/learned/` (crates/dreaming/src/tools.rs#L381).

## D. Hard-coded single scalar
- **Spec and wire types:**
  - `StudySpec.metric` (crates/autoresearch/src/types.rs#L49-L50); `MetricSpec` (L106-L120).
  - `Sample.value` (L533-L534).
  - `GateCfg` / `GateInput` (L566-L606); `GateStep::Decide` (L619-L628).
  - `RunReport` Δ/z (L652-L681).
  - `Incumbent.mean` (L821-L828).
  - `StudyInfo.metric` / `delta_pct` / `headline` (L1192-L1209).
  - `ProbeResult` values / median / noise_pct / unit (L1379-L1391).
  - `ChartPoint.metric` (L1447-L1456).
- **Engine:**
  - The primary is chosen by name (crates/autoresearch/src/runner.rs#L95-L109).
  - `gate::step` (crates/autoresearch/src/gate.rs#L60-L181).
  - `gate_cfg` (crates/autoresearch/src/actor.rs#L569-L586).
  - Overfit (crates/autoresearch/src/actor.rs#L1007-L1016).
  - Regime change (crates/autoresearch/src/actor.rs#L4578-L4592).
  - Wave sort (crates/autoresearch/src/wave.rs#L595-L601).
  - The `max_cv` noise gate (crates/autoresearch/src/config.rs#L104-L110).
- **Ledger:**
  - `studies.incumbent_mean` and `sigma` (crates/autoresearch/migrations/0001_init.sql#L16-L18).
  - `segments.baseline_mean` (L37).
  - `experiments.delta` / `delta_pct` / `z` / `metric` (L60-L64).
  - `samples.value` (L83).
- **Report:**
  - The brief's metric and noise lines (crates/autoresearch/src/brief.rs#L85-L102).
  - The reviewer's "improvement in {metric}" (crates/autoresearch/src/review.rs#L41, L56).
  - The finalize headline and CI (crates/autoresearch/src/finalize.rs#L546-L569).
  - The knowledge record (crates/autoresearch/src/record.rs#L60-L96).
- **UI:**
  - The chart (crates/desktop/ui/src/components/autoresearch/StudyChart.tsx#L15-L36).
  - The run table (crates/desktop/ui/src/components/autoresearch/RunTable.tsx#L23, L60, L90-L92).
  - Formatters and holdout summary (crates/desktop/ui/src/components/autoresearch/model.ts#L233, L332-L353).
- **Draft card:** "Optimise `{}` ({dir} is better…)" and the time-unit warning (crates/app/src/study_draft.rs#L42-L49, L107-L108, L157-L160).
- **Setup and probe:**
  - One `metric` row, with leaves `metric.*` (crates/autoresearch/src/setup.rs#L28-L53).
  - The probe's unit, median and noise for one value series (crates/autoresearch/src/setup.rs#L901-L929, L971-L972).
- **Skill and prompt text:**
  - "A scalar the user trusts" (crates/autoresearch/skill.md#L12).
  - Judge-only → decline (L18).
  - The Metric row (L28).
  - The METRIC contract (L44).
  - "a benchmark that prints a number" (crates/autoresearch/src/tools.rs#L47-L48).

## E. Risks and open questions
- **False-keep rate.** The gate's guarantee is tested for one metric (`null_false_keep_rate_at_most_one_percent`, crates/autoresearch/src/gate.rs#L443-L449, outline). Pareto keeps over k objectives multiply the false-keep rate unless thresholds are corrected.
- **CASE format.** Changing it (dropped name, one value per id: crates/autoresearch/src/scanner.rs#L92-L106) changes the shape of `samples.cases` JSON already stored (0001_init.sql#L85).
- **Hidden spend.** Bench-side model calls (W2, W3) sit outside the USD spend stop (crates/autoresearch/src/actor.rs#L446-L467).
- **Tools for W5 and W6.** Web and MCP access needs a toolset change or plugin-side calls. The allowlist is closed and python-free (crates/autoresearch/src/binding.rs#L54-L70).
- **Multi-day waits (W6)** clash with the one-fire experiment, `run_timeout` and the `TurnFinished` closure (crates/autoresearch/src/schedule_file.rs#L115-L124; crates/autoresearch/src/binding.rs#L170-L200).
- **Per-launch approval (W6)** contradicts `approvals: false` in every study file (crates/autoresearch/src/schedule_file.rs#L84).
- **W7's artefact is compiled code** (crates/autoresearch/src/tools.rs#L46-L54; crates/autoresearch/src/lib.rs#L79-L91). Keeping `gate.rs` off-limits needs its path in evaluator_paths or off_limits (crates/autoresearch/src/types.rs#L59-L64).
- **Population search (W4)** needs more than one parent and branch, but the ledger has one incumbent and one study branch (0001_init.sql#L11-L16).
- **Dreaming and W2 overlap.** Unify the gates, or keep each mechanism to its own artefacts? Today their write permissions and ledgers are disjoint (crates/dreaming/src/tools.rs#L35-L51; crates/autoresearch/src/binding.rs#L72-L100).
- **Windows** stays unsupported for every variant (crates/autoresearch/src/lib.rs#L60-L63).