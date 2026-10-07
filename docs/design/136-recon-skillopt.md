<!-- Recon sheet for design 136 (dreaming): microsoft/skillopt at 343db229 (2026-10-06), read-only child recon-skillopt, 2026-10-08 00:00. Line numbers refer to the clone at /tmp/dreaming-recon/skillopt. -->

## A. What it is

- README claim: "SkillOpt treats the skill document as the trainable state of a frozen agent … A separate optimizer model turns scored rollouts into bounded add / delete / replace edits on a single skill document; in the default paper-style path, a candidate edit is accepted only when it strictly improves a held-out validation score. A textual learning-rate budget, a rejected-edit buffer, and an epoch-wise slow / meta update make skill training stable while adding zero inference-time model calls at deployment." — `README.md:32-39`
- Deployed artefact is "a compact `best_skill.md` (typically 300–2,000 tokens) that runs against the unchanged target model" — `README.md:41-42`; the shipped checkpoints are actually 2k–13k chars each (`ckpt/README.md:29-31`; file sizes 1,963 B docvqa … 13,335 B spreadsheetbench).
- Two products in one repo: the research trainer `skillopt/` (ReflACT loop over benchmarks) and **SkillOpt-Sleep** `skillopt_sleep/`, "a nightly offline self-evolution engine (harvest → mine → replay → consolidate behind a held-out validation gate)" with zero dependency on the research code — `README.md:18`, `pyproject.toml:78-81`. Sleep is the part that matches the reader's problem.
- License MIT, Microsoft 2026 — `LICENSE:1-3`, `pyproject.toml:10`. Version 0.2.0, "Development Status :: 3 - Alpha", Python ≥ 3.10 — `pyproject.toml:7,17,11`.
- Languages/size (non-hidden files, 442 total): 244 `.py`, 105 `.md`, 40 `.json`, 14 `.sh`, 10 `.yaml`, 5 `.html`, 4 `.ps1`, 4 `.mjs` (my glob count). Package split: `skillopt/` 143 files, `tests/` 97, `plugins/` 63, `skillopt_sleep/` 45, `docs/` 25. Biggest modules: `skillopt/engine/trainer.py` 2,535 lines, `skillopt_sleep/staging.py` 3,161, `skillopt_sleep/backend.py` 2,603, `skillopt/model/codex_harness.py` 2,036.
- Tests: 1,453 `def test_` functions across 89 test files (my grep of `tests/`); CI runs `pytest -q` on Python 3.10/3.11/3.12 — `.github/workflows/ci.yml:15,24`.
- Release cadence: v0.1.0 2026-06-02, v0.2.0 2026-07-02, large `[Unreleased]` section since (OpenCode/Copilot/Cursor/Pi backends, evalkit, multi-skill fan-out) — `CHANGELOG.md:227,142,7-80`. Shallow clone: no tags present (`.git/packed-refs` has only `origin/main`).
- Maturity signals: arXiv paper + MSR blog + press (`README.md:5,17`), Dependabot, ~15 named external contributors per release (`CHANGELOG.md:135-140`), deliberate fail-closed engineering in Sleep (evidence chain, redaction, leak detection). Counter-signals: "Preview … interfaces and defaults may change" (`docs/sleep/README.md:10-11`), single-seed results (`docs/sleep/RESULTS.md:15`).

## B. Core optimization loop

**Artefact.** One Markdown skill document (string), optionally with two protected machine-owned regions `<!-- SLOW_UPDATE_START/END -->` and `<!-- APPENDIX_START/END -->` — `skillopt/optimizer/skill.py:14-30`. For exec harnesses it is rendered as `.agents/skills/skillopt-target/SKILL.md` with YAML frontmatter plus a `task.md` in a fresh workspace — `skillopt/model/codex_harness.py:48-74,109-118`. Not a directory with scripts; no code is optimized.

**Inputs.** A task split dir `train/ val/ test/` (`skillopt/datasets/base.py:19-24`), default ratio `2:1:7` (`:241`); an `EnvAdapter` with `rollout()` returning `[{"id","hard"(0/1),"soft"(0..1),…}]` (`skillopt/envs/base.py:216-230`); a seed skill file or blank (`trainer.py:894-901`).

```text
# skillopt/engine/trainer.py  ReflACTTrainer.train()
baseline: score skill_init on split "valid_seen" → current_score=best_score       # :1129-1159
for epoch in 1..num_epochs(4):                                                       # :1168
  step_buffer=[]; meta_skill = meta_skill[epoch-1] if use_meta_skill                 # :1187-1192
  for step in steps_per_epoch:                                                       # :1204
    for a in accumulation(1):                                                        # :1238
      ① ROLLOUT  results = adapter.rollout(train_batch(40), current_skill)           # :1264-1268
      ② REFLECT  raw_patches = adapter.reflect(results, current_skill, …,
                   step_buffer_context, meta_skill_context)                          # :1280-1286
                 → failures(hard==0) / successes split, minibatches of M=8,
                   one optimizer call each, ThreadPool(workers=16)                   # gradient/reflect.py:550-558,626-641
    if no patches: save, continue                                                    # :1339-1360
    ③ AGGREGATE merged = merge_patches(fail_patches, succ_patches, batch=8)          # :1364-1370
                 hierarchical LLM merge per group, then failure-first final merge    # gradient/aggregate.py:79-151,195-239
    ④ SELECT    budget = scheduler.step()  (cosine 4→2) | LLM "autonomous" count     # :1394-1415; optimizer/scheduler.py:79-92
                 ranked = rank_and_select(merged, max_edits=budget)  (LLM ranking)   # :1416-1421; optimizer/clip.py:25-109
    ⑤ UPDATE    candidate = apply_patch_with_report(current_skill, ranked)           # :1502; optimizer/skill.py:165-186
                 (or rewrite_from_suggestions / full_rewrite_minibatch modes)        # :1453-1500
    ⑥ EVALUATE  cand_hard,cand_soft = sel_cache[hash] or rollout(valid_seen, cand)   # :1562-1578
                 gate = evaluate_gate(...)  accept iff score > current_score          # :1583-1598; evaluation/gate.py:200-225
                 use_gate=False → force_accept (validation still recorded)           # :1606-1626
    step_buffer += {failure_patterns, rejected_edits, score_before/after}            # :1689-1708
    persist skills/skill_vNNNN.md, best_skill.md, history.json, runtime_state.json  # :1741-1748
  SLOW UPDATE (epoch ≥2): rollout 20 train items under prev-epoch skill AND current
    skill → pairs categorized improved/regressed/persistent_fail/stable_success →
    optimizer writes guidance into the protected SLOW_UPDATE field                  # :1835-1935
    accepted unconditionally (default) or via the selection gate                    # :1951-2060
  META SKILL (epoch ≥2): optimizer-side memory from the same comparison pairs        # :2087-2197
final: evaluate best_skill (and final skill) on "valid_unseen" test split            # :2208-2260
```

- Candidate generation = reflective LLM over full execution traces (`conversation.json` per task, formatted by `fmt_trajectory`), no templates, no mutation operators, no crossover; the only "merge" is LLM deduplication of independently proposed patches with a `support_count` — `gradient/reflect.py:65-106,140-239`; `prompts/merge_failure.md:9-13`.
- Selection = greedy hill-climb on one validation score: strict `>` against the current skill; "accept_new_best" when also > best — `evaluation/gate.py:200-217`. No Pareto frontier, no best-of-N candidates per step (one candidate per step), no tournament. Three update modes: `patch` (default), `rewrite_from_suggestions`, `full_rewrite_minibatch` — `optimizer/update_modes.py:6-8`.
- Stopping: fixed `num_epochs × steps_per_epoch` where `steps_per_epoch = ceil(train_size/(batch_size·accumulation))` — `trainer.py:927-930`. No early stopping, no cost cap. Resume from `runtime_state.json`/`history.json` — `:985-1046`.
- Budget controls: `learning_rate` (max edits/step, default 4), `min_learning_rate` 2, `lr_scheduler` cosine/linear/constant/autonomous — `configs/_base_/default.yaml:93-96`; per-call `max_completion_tokens` 16,384 (64,000 for full rewrite), 3 retries — `gradient/reflect.py:347-352`; `exec_timeout: 120` s per target call — `default.yaml:121`. Tokens are only *recorded* per stage per step (`trainer.py:1716-1730`), never enforced.
- Parallelism: `analyst_workers: 16` thread pool for reflection and merges (`default.yaml:89`; `aggregate.py:129-139`); env rollouts run their own thread pools with a per-task timeout `max(600, exec_timeout+60)` s — `skillopt/envs/searchqa/rollout.py:371-374`.

**Sleep variant (one "night" = one SkillOpt epoch over mined tasks):**

```text
# skillopt_sleep/cycle.py run_sleep_cycle()
state=SleepState.load(); night+=1; backend=build_backend(cfg)                           # :645-666
evidence.jsonl opened under <project>/.skillopt-sleep/staging/<ts>/                    # :679-706
skill = managed SKILL.md (or target_skill_path); memory = <project>/CLAUDE.md          # :709-727
digests = harvest_for_config(since=last_harvest or now-72h, limit=max_tasks*3)         # :741-764
tasks = mine(digests, max_tasks=40, val_fraction=.34, test_fraction=0, llm_miner?)      # :797-809
result = dream_consolidate(backend, tasks, skill, memory, recall_k, dream_rollouts,
                           dream_factor, edit_budget=4, gate_metric="mixed", gate_mode) # :856-870
  consolidate(): baseline on VAL → replay TRAIN → reflect(failures) → apply edits →
    gate trial on VAL (strict >) for skill, then again for memory → fresh final VAL     # consolidate.py:216-451
archive tonight's real tasks (cap 300) for future recall                                # :875; state.py:97-102
stage report.md/report.json/manifest.json + proposed SKILL.md/CLAUDE.md; auto_adopt off # :989-1034,1089-1092
```

## C. Data formats

**Research task/batch schema**
- `BatchSpec(phase, split, seed, batch_size, payload=list[item], metadata)` — `skillopt/datasets/base.py:41-69`. Items are benchmark-specific dicts; the released manifests contain only ids (`data/searchqa_id_split/split_manifest.json:8-16`; `data/README.md:3-6`).
- Rollout result `RolloutResult(id, hard: float, soft: float, n_turns, fail_reason, task_type, task_description, predicted_answer, question, reference_text, target_system_prompt, target_user_prompt, spreadsheet_preview, extras)` — `skillopt/types.py:104-124`. SearchQA sets `hard=int(em)`, `soft=f1`, `fail_reason="EM=0: predicted … but expected …"` — `envs/searchqa/rollout.py:247-253`.
- Trace: `predictions/<id>/conversation.json`, a list of `{"type":"tool_call","cmd","obs"}` | `{"step","action","env_feedback","reasoning"}` | `{"role","content"}`; the evaluator's verdict is appended as a `role: system` message so the analyst sees it — `gradient/reflect.py:71-76`; `envs/searchqa/rollout.py:254-264`. Exec harnesses add `target_system_prompt.txt`, `target_user_prompt.txt`, `codex_trace_summary.txt`, `claude_trace_steps.txt` — `reflect.py:168-223`.
- Edit/patch: `Edit(op ∈ append|insert_after|replace|delete, content, target, support_count, source_type ∈ failure|success, merge_level)`; `Patch(edits, reasoning, ranking_details)`; `RawPatch(patch, source_type, batch_size, failure_summary=[{failure_type,count,description}])` — `types.py:23-40,72-81,182-216`.
- Evaluation result: two scalars per task (`hard`, `soft`) averaged into batch scores — `skillopt/utils/scoring.py:7-23`; gate metric `hard|soft|mixed=(1-w)·hard+w·soft` — `evaluation/gate.py:116-122`. Textual feedback only via `fail_reason` and the analysts' `failure_summary`.
- Skill artefact: plain Markdown; protected regions; no length cap, no required sections (the only "constraint" is an optional density bonus, §D) — `optimizer/skill.py:14-30`; `docs/guide/skill-document.md:45-59`.
- On-disk state under `out_root`: `config.json` (:946), `history.json` (:357-366), `runtime_state.json` (:400-413), `skills/skill_vNNNN.md` (:371-374), `best_skill.md` (:1742), `selection_eval_baseline/`, `steps/step_NNNN/{rollout/predictions/<id>/conversation.json, patches/minibatch_{fail,succ}_NNN.json, merged_patch.json, ranked_edits.json, lr_decision.json, candidate_skill.md, edit_apply_report.json, selection_eval/, trajectory_digest.json, step_record.json}` (:1210-1748), `slow_update/epoch_NN/{rollout_prev,rollout_curr,comparison_pairs.json,candidate_skill.md,slow_result.json}` (:1769-1937), `meta_skill/epoch_NN/meta_skill_result.json` (:2089-2197), `lr_history.jsonl` (:1408), `summary.json` (:2493) — all `skillopt/engine/trainer.py`. Selection cache is in-memory keyed by `sha256(skill)[:16]`, rebuilt from history — `:1077-1082`; `utils/scoring.py:26-28`.

**Sleep schemas**
- `SessionDigest(session_id, project, git_branch, started_at, ended_at, user_prompts[], assistant_finals[], tools_used[], skills_used[], files_touched[], feedback_signals[] e.g. "neg:still broken", n_user_turns, n_assistant_turns, raw_path)` — `skillopt_sleep/types.py:17-40`.
- `TaskRecord(id, project, intent, context_excerpt, system, attempted_solution, outcome ∈ success|fail|mixed|unknown, reference_kind ∈ exact|rubric|rule|none, reference, judge={"kind":"rule","checks":[{op,arg,description}]}, tags, source_sessions, split ∈ train|val|test, origin ∈ real|dream, derived_from, skill_hint)` — `types.py:49-86`.
- `ReplayResult(id, hard, soft, response, fail_reason, task_type, judge_rationale, tools_called, tokens, latency_ms, optimizer_feedback)` — `types.py:100-117`. `optimizer_feedback` is the learning-safe projection; `fail_reason`/`judge_rationale` are audit-only — `:113-117`.
- `EditRecord(target ∈ skill|memory, op ∈ add|delete|replace, content, anchor, rationale)` — `types.py:126-133`; applied as bullet lines inside `<!-- SKILLOPT-SLEEP:LEARNED START/END -->` — `memory.py:15-21,47-56`.
- `SleepReport(night, …, baseline_score, candidate_score, accepted, gate_action, holdout_leaked, edits, rejected_edits, unmatched_edits, skill_groups, tokens_used, gate_trials=[{target, baseline_score, candidate_score, accepted, blocked_by_regression, task_deltas}])` — `types.py:161-196`.
- Evidence log: append-only JSONL `{"ts","seq","stage","event",…}` with best-effort secret redaction and 4,000-char per-field truncation — `skillopt_sleep/evidence.py:24-27,46-64`.
- On-disk: `~/.skillopt-sleep/state.json` = `{night, last_harvest{project→iso}, slow_memory, history[], task_archive[≤300], last_model_key}` — `state.py:26-35,97-102`; `~/.skillopt-sleep/prompts.json` overrides — `prompts.py:164-167`; `<project>/.skillopt-sleep/staging/<ts>/{evidence.jsonl, report.md, report.json, manifest.json, diagnostics.json, backup/…, adopted_skills.json}` — `staging.py:829,1131-1142,1648,1674`; `cycle.py:1048-1079`.

## D. Overfitting and generality

- Research path: a fixed validation split (`valid_seen`, all items when `sel_env_num: 0`) is re-scored after every step; test (`valid_unseen`) only at the end — `trainer.py:1569-1578,2208`; `datasets/base.py:503-505`. Minibatch M=8 trajectories per analyst call, L=4 edits per call, batch 40 tasks/step, 4 epochs — `configs/_base_/default.yaml:79-93`.
- "Really better" = one rollout per validation item, candidate mean strictly greater than current mean; no statistical test, no repeated runs, no threshold margin — `evaluation/gate.py:200-225`. The docs concede `soft` exists "when a small held-out selection set has too few items for hard accuracy to be sensitive" — `gate.py:17-19`. Identical candidates are not re-scored (hash cache) — `trainer.py:1562-1567`.
- Regularizers: the edit budget itself (cosine 4→2) — `scheduler.py:79-92`; the rejected-edit/failure-pattern step buffer shown to later analysts "to avoid repeating ineffective edits" — `trainer.py:634-637,1697-1706`; a rejected step leaves the skill unchanged. No length penalty or diff-size limit exists (skill length is only logged, `:1738`); the optional `use_semantic_density` adds `0.05 × share of words in {MUST, ALWAYS, NEVER, …}` to the gate score — `gate.py:46-57,128-130` (default off, `trainer.py:1096`).
- Readability constraints live only in prompts: "Edits must be generalizable; do not hardcode task-specific values. Only patch gaps in the skill — do not duplicate existing content." — `prompts/analyst_error.md:13-14`; ranking criteria systematic impact / complementarity / generality / actionability — `prompts/ranking.md:4-12`. Human-in-the-loop in the research path: none.
- Sleep: `val_fraction 0.34`, `test_fraction 0.0` by default; split by `sha256(seed+task.id) % 100` so a task keeps its split across nights; dream (synthetic) tasks are forced to `train`; val is promoted non-empty when ≥2 real tasks — `mine.py:272-347`. Recall of archived tasks excludes anything that was ever val/test — `dream.py:93-94`.
- Sleep gate: per-target trials (skill, then memory) each require strict improvement on val; optional `gate_no_regression` rejects a candidate if any single val task scores lower; a fresh final replay must also beat baseline; if val was not disjoint from train the verdict is `reject_unverified` and edits are staged but not certified — `consolidate.py:281-308,386-434`. Attempt/judge responses are cached by prompt hash, so re-scoring is deterministic (n=1) — `backend.py:420-423`.
- Human-in-the-loop in Sleep: everything is staged; adoption is an explicit `adopt --legacy|--skill|--all-skills`; `auto_adopt` defaults to false and never promotes fan-out proposals — `cycle.py:1089-1092`; `config.py:89`; `docs/sleep/README.md:369-372`. The `handoff` backend makes the user's own agent answer each model call "in a FRESH context, so the session's own history cannot contaminate the held-out gate" — `handoff_backend.py:9-12`.
- Statistics exist but are not in the gate: `evalkit` does McNemar + percentile-bootstrap CIs on a paired manifest and "does not change the nightly gate" — `skillopt_sleep/evalkit.py:1-16,12`. Results docs warn single-seed noise is ±1–2 pts — `docs/sleep/README.md:441-445`.

## E. Execution

- Research targets: `openai_chat`/`azure_openai`/`claude_chat`/`qwen_chat`/`minimax_chat`/`copilot_chat`/`openai_compatible` chat backends, or exec harnesses `codex_exec`, `claude_code_exec`, `cursor_exec`, `copilot_exec` — `README.md:67-73`; `configs/_base_/default.yaml:4-36`. Each env's `rollout()` decides; SearchQA calls `run_target_exec` when `is_target_exec_backend()` else `chat_target` — `envs/searchqa/rollout.py:207-215,276-298`.
- Codex exec: fresh workspace per task with `SKILL.md` + `task.md`, then `codex exec --skip-git-repo-check -C <work_dir> --sandbox <ws-write> -c approval_policy="never" -m <model> --output-last-message <file> <prompt>` under `subprocess.run(timeout=…)` — `codex_harness.py:96-118,1352-1391`. Trace capture = stdout parsed into a summary (sandbox, whether `task.md`/`SKILL.md` were read, shell errors, answer format, tokens) — `:188-253`; Claude Code SDK/CLI traces are parsed into step lists — `:526-604`. Trace-to-optimizer is gated by `codex_trace_to_optimizer`/`claude_trace_to_optimizer` flags — `default.yaml:37-38`; `reflect.py:192-223`.
- Scoring is env-local and programmatic (SearchQA EM/F1, SpreadsheetBench executes generated openpyxl and diffs workbooks) — `envs/searchqa/rollout.py:242-248`; `docs/sleep/RESULTS.md:26`. Timeouts: `exec_timeout` 120 s per call, per-task hard timeout `max(600, exec_timeout+60)` producing `hard=0, fail_reason="task-timeout-…"` — `rollout.py:371-374,404-414`.
- Sleep replay is single-shot text by default: the task is rendered through the `attempt` template with skill+memory and answered with `max_tokens=512`; `tool_called` checks use a backend tool-aware path or fail explicitly — `backend.py:386-423`; `replay.py:6-9,40-43`. Sleep backends are CLIs (`claude -p`, `codex`, `copilot`, `cursor-agent`, `pi`, `opencode`), Azure/OpenAI-compatible HTTP, `mock`, or `handoff` — `config.py:48`; `cycle.py:649-664`. "Fresh-worktree replay is not implemented" — `config.py:69`.

**Reflection prompts (verbatim, trimmed).** Research failure analyst — `skillopt/prompts/analyst_error.md:1-34`:
```text
You are an expert failure-analysis agent for AI agent tasks.
You will be given MULTIPLE failed agent trajectories from a single minibatch and the current skill document.
Your job is to identify the most important COMMON failure patterns across the batch and propose a concise set of skill edits.
## Analysis Process
1. Read ALL trajectories in the minibatch. 2. Identify the most prevalent, systematic failure patterns across them.
3. For each pattern, classify its failure type. 4. Propose skill edits that address the COMMON patterns — not individual edge cases.
5. Edits must be generalizable; do not hardcode task-specific values. 6. Only patch gaps in the skill — do not duplicate existing content.
You will be told the maximum number of edits (the budget L). Produce AT MOST L edits …
Respond ONLY with a valid JSON object …
{ "batch_size": <n>, "failure_summary": [{"failure_type": "<type>", "count": <int>, "description": "<one-line>"}],
  "patch": { "reasoning": "<why…>", "edits": [
    {"op": "append", "content": "<markdown>"}, {"op": "insert_after", "target": "<exact heading/text>", "content": "<markdown>"},
    {"op": "replace", "target": "<exact text>", "content": "<replacement>"}, {"op": "delete", "target": "<exact text>"} ] } }
```
User message = `## Current Skill` + `## Edit Budget … L=4` + `## Previous Steps in This Epoch` (step buffer) + meta-skill context + `## Failed Trajectories (N total)` — `gradient/reflect.py:321-344`. Env-specific overrides live at `skillopt/envs/<env>/prompts/analyst_{error,success}.md` — `envs/base.py:298-318`.

Sleep reflect — `skillopt_sleep/prompts.py:91-119`:
```text
You are SkillOpt's optimizer. The agent keeps failing the recurring tasks below. Propose at most __EDIT_BUDGET__ bounded edits to the __TARGET__ document so it stops failing. Each edit MUST be a short, GENERAL, reusable rule or preference (never task-specific, never an answer to a single task). Use the actionable semantic feedback to fix user-visible behavior, not to imitate the evaluator.
BE CONCRETE about public task requirements such as thresholds and section names, but NEVER quote or reconstruct private verifier implementation details such as regexes, check expressions, or judge source text. Vague rules do not change behavior, while verifier-specific rules overfit.
IMPORTANT: your edits are APPENDED to a 'Learned preferences' block; you CANNOT delete the existing instructions above. If the current __TARGET__ text conflicts with a criterion … write an explicit, forceful OVERRIDE rule …
HARD CONSTRAINT: every rule you write MUST be consistent with the 'Task output contract' below (if shown). …
Return ONLY a JSON array: [{"op":"add|replace|delete","content":"<rule>","anchor":"<text to replace/delete, optional>","rationale":"<why>"}].
# Current __TARGET__
__CUR_DOC__
__GUARD__ __CRITERIA__ __PREFS__
# Recurring failures
__FAILURES__
```
`__FAILURES__` = up to 8 failures as `- wanted: <intent[:160]> / got: <response[:160]> / actionable-feedback: <…[:240]>`; `__CRITERIA__` = a `Counter` of semantic feedback lines "(observed Nx)"; one retry with "your previous reply was not valid JSON" — `backend.py:473-536`. Contrastive variant: "Identify what the GOOD attempts did that the BAD ones did not, and propose at most N SHORT, GENERAL, reusable rules" over the 6 highest-spread tasks — `skillopt_sleep/rollout.py:126,147-163`. Slow update: "Write a SHORT, durable guidance block (2-5 bullet points) capturing the longitudinal lessons … If prior guidance is given, refine it rather than restate it." — `skillopt_sleep/slow_update.py:122-132`.

## F. Providers/models

- Research: `model.backend`, `optimizer_backend`, `target_backend`, `optimizer`, `target` deployments, `reasoning_effort` — `configs/_base_/default.yaml:4-10`; roles resolved in `trainer.py:706-714`; per-role Azure endpoints/keys/auth modes (`default.yaml:39-56`); generic OpenAI-compatible via `OPENAI_COMPATIBLE_BASE_URL/API_KEY/MODEL/TEMPERATURE/MAX_TOKENS/TIMEOUT_SECONDS` with `OPTIMIZER_`/`TARGET_` prefixes — `docs/reference/config.md:229-243`; CLI example `--cfg-options model.optimizer_backend=openai_compatible model.target_backend=openai_compatible model.optimizer=deepseek-chat` — `docs/reference/cli.md:87-95`.
- Sleep: single `backend`+`model`, or dual `optimizer_backend/optimizer_model` (mines, judges, writes edits) vs `target_backend/target_model` (runs attempts) — `skillopt_sleep/config.py:48-56`. OpenAI-compatible endpoints reuse the `azure_openai` backend with `AZURE_OPENAI_AUTH_MODE=openai_compatible`, `AZURE_OPENAI_ENDPOINT`, `AZURE_OPENAI_API_KEY`, `SKILLOPT_SLEEP_COMPAT_MAX_TOKENS` (8192) — `docs/sleep/openai-compatible-endpoints.md:29-37,64-70`. `copilot` CLI reports no token counts, so usage totals are zero — `CHANGELOG.md:49`.
- Cost/latency: no dollar or wall-clock numbers are claimed anywhere I found. Tokens per stage per step are recorded (`trainer.py:1716-1730`); Sleep records tokens and `latency_ms` per replay in the evidence log (`replay.py:71-77`) and offers a multi-objective reward normalised at 2,000 tokens / 15,000 ms (`replay.py:144-169`, not wired into the gate). Handoff "typically 3–6 rounds per night" — `handoff_backend.py:22-23`. Planning constant "~1.5k tokens per rollout" — `budget.py:68`. `max_tokens_per_night: 400_000` is declared (`config.py:43`) but never read by the cycle (grep finds only the definition); the README itself says limits "are not hard call, token, time, or monetary budgets" — `docs/sleep/README.md:327-328`.

## G. Reported results

- Paper claims: "Across six benchmarks, seven target models, and three execution harnesses (direct chat, Codex CLI, Claude Code CLI), SkillOpt is best or tied-best on all 52 evaluated (model, benchmark, harness) cells and on GPT-5.5 lifts the average no-skill accuracy by +23.5 points in direct chat, +24.8 inside the Codex agentic loop, and +19.1 inside Claude Code" — `README.md:42-47`. Splits used: SearchQA 400/200/1400, LiveMath 35/17/125, DocVQA 107/53/374, OfficeQA 50/24/172, SpreadsheetBench 80/40/280, ALFWorld 39/18/134 — `data/README.md:31-36`. Not reproducible from this repo alone (payloads must be materialized; only GPT-5.5 checkpoints shipped — `ckpt/README.md:3-16`).
- Sleep (5 nights × 10 new tasks, optimizer GPT-5.5, seed 42, test scored before night 1 and after night 5): SearchQA 1,400-item test, gated, `dream_rollouts=5`: `recall_k=10` 0.802→0.834 (+3.1), `recall_k=20` 0.803→0.848 (+4.5), full-history 0.796→0.851 (+5.6); SpreadsheetBench GPT-5.4-nano gate-free 0.279→0.314 (+3.6); gbrain-evals seeds 0.00→1.00 with Claude Code and Codex targets — `docs/sleep/README.md:413-439`. "On saturated or noisy benchmarks … the effect is flat within run-to-run noise … ±1–2 pts" — `:441-445`.
- Safety result: GPT-5.4-nano on SearchQA, gate off 0.554→0.026 (−52.8) because the optimizer learned "answer with the document-title string, verbatim"; gate on 0.570→0.570 — `docs/sleep/RESULTS.md:40-52`.

## H. Extension points

- New benchmark = `skillopt/envs/<name>/` with adapter (`build_train_env`, `build_eval_env`, `rollout`, optional `reflect`, `get_task_types`), dataloader subclassing `SplitDataLoader.load_split_items`, YAML config, optional `skills/initial.md` and `prompts/analyst_*.md` — `envs/base.py:9-24,187-230,298-318`; `datasets/base.py:26-28`; template at `skillopt/envs/_template/`; guide `docs/guide/new-benchmark.md`.
- New backend: `skillopt/model/<name>_backend.py` registered via `common.py`, `backend_config.py`, `model/__init__.py`; exec targets share `codex_harness.py` — `README.md:65-75`.
- Custom reflection: override `EnvAdapter.reflect` (default delegates to `run_minibatch_reflect`) or supply env prompt files — `envs/base.py:234-253`. Custom proposer/selector = swap `merge_patches`/`rank_and_select` callers in the trainer (no registry).
- Research CLI: `skillopt-train --config <yaml> --cfg-options k=v…`, `skillopt-eval --config … --skill … --split train|valid_seen|valid_unseen|all` — `docs/reference/cli.md:14-61`; `pyproject.toml:66-69`. Config schema = `configs/_base_/default.yaml:1-122` (sections `model`, `train`, `gradient`, `optimizer`, `evaluation`, `env`), env configs inherit via `_base_`.
- Sleep CLI: `run | dry-run | status | adopt | harvest | schedule | unschedule | evalkit` — `skillopt_sleep/__main__.py:854-886`; flags include `--project --scope --source --backend --model --max-sessions --max-tasks --lookback-hours --edit-budget --preferences --target-skill-path --tasks-file --auto-adopt --progress --json --seed` (my grep of `add_argument`). Transcript sources are a fixed `if/elif` chain (`claude|codex|copilot|copilot_cli|cursor|pi|opencode|auto`) — `harvest_sources.py:16-86`; a new source means editing it. Reviewed task files: `harvest --output f.json`, edit, set `"reviewed": true`, then `--tasks-file` — `plugins/claude-code/commands/skillopt-sleep.md:82-83`.
- Sleep config (`~/.skillopt-sleep/config.json`, defaults): `transcript_source "claude", projects "invoked", lookback_hours 72, max_tasks_per_night 40, val_fraction 0.34, test_fraction 0.0, backend "mock", gate_mode "on", edit_budget 4, preferences "", gate_metric "mixed", gate_mixed_weight 0.5, gate_no_regression false, dream_rollouts 1, dream_factor 0, recall_k 0, evolve_memory true, evolve_skill true, llm_mine true, target_skill_path "", evidence_log true, evidence_max_chars 4000, multi_skill_fanout, auto_adopt false, managed_skill_name "skillopt-sleep-learned", redact_secrets true, seed 42` — `skillopt_sleep/config.py:26-93`. Prompt templates (`miner`, `attempt`, `judge`, `reflect`) overridable via `prompts.json`, mtime-checked per call — `prompts.py:9-12,122-155,164-191`.
- Backend ABC for Sleep: `attempt`, `attempt_with_tools`, `judge`, `reflect`, `_call` — `backend.py:59-82`.

## I. Stated limitations and hard requirements

- Research path hard-requires a labelled benchmark with a programmatic scorer per env (EM/F1, executed workbooks, simulator success) and materialized `train/val/test` payloads — `data/README.md:3-6,40-53`; `envs/base.py:226-230`.
- Sleep hard-requires a supported transcript store on disk (`~/.claude/projects/*.jsonl`, Codex, VS Code Copilot, Cursor, Pi, OpenCode SQLite) — `harvest.py:6-12`; `harvest_sources.py:16-86`; and a CLI/HTTP backend. Nothing requires a specific agent for deployment: the artefact is Markdown.
- Sleep replay is textual, not end-to-end: the Cursor backend "evaluates textual guidance rather than end-to-end repository, CLI, browser, or service workflows" — `docs/sleep/README.md:312-316`; `tool_called` is approximated via a `TOOL_CALL: <name>` marker unless a tool-aware backend is used — `judges.py:15-17`; "fresh-worktree replay is not implemented" — `config.py:69`.
- Feedback detection is keyword-based and English-only (`"wrong"`, `"no,"`, `"thanks"`…) — `harvest.py:26-39`; heuristic mining takes the *first* user prompt as the intent and the *last* assistant message as the attempt — `mine.py:175,187`.
- Shape-only judges are gameable and were gamed "twice in practice, first with section_present and then with contains" — `llm_miner.py:111-115`; `judges.py:19-29`. Non-rule judge rationale is replaced by a generic sentence, removing useful feedback — `docs/sleep/README.md:128-142`.
- Data boundary: "Outbound prompts are not currently guaranteed to be secret-free" — `docs/sleep/README.md:46-52`.
- README/CHANGELOG vs code: CHANGELOG lists "Slow-update long-term memory field (runs even with the gate off)" for Sleep (`CHANGELOG.md:159`) and `slow_update.py:13-16` says it "still runs at the end of the run", but `run_sleep_cycle` never calls `run_slow_update`; only `experiments/run_gbrain.py:98-102` does. Likewise `Budget`/`plan_depth` are used only by `experiments/run_gbrain.py:173-176`, and `max_tokens_per_night` is unenforced (`config.py:43`). `gradient.max_analyst_rounds` "was parsed and logged but never reached reflection" — `CHANGELOG.md:89-93`.
- Single seed, noise ±1–2 pts, "treat sub-~1.5 pt differences as noise" — `docs/sleep/RESULTS.md:15`; `docs/sleep/README.md:441-445`. Multi-skill fan-out multiplies cost per hinted skill — `docs/sleep/README.md:357-361`.

## J. Borrow list (for a harness improving its own skills from its own session logs)

1. **Machine-owned protected block inside each skill** (`<!-- …LEARNED START/END -->` with a banner; hand-written text never touched; step edits cannot target SLOW_UPDATE/APPENDIX regions) — `skillopt_sleep/memory.py:15-21,47-56`; `skillopt/optimizer/skill.py:14-30,94-96`. Assumes one managed SKILL.md; replace with a marked block per skill file in the skills directory, plus git history as the backup/receipt.
2. **Bounded edit ops with exact-text anchors and an apply report** (`append|insert_after|replace|delete`, statuses `applied_*|skipped_*|error`, `unmatched` edits surfaced so "zero edits" nights are not silent) — `skill.py:85-146`; `memory.py:87-151`; `types.py:181-185` (sleep). Assumes nothing external; transfers as-is.
3. **Textual learning rate + rejected-edit buffer**: cap edits per cycle, decay it, and feed previous rejected edits and unsolved failure patterns into the next proposal prompt — `scheduler.py:79-92`; `trainer.py:622-676,1697-1706`. Assumes a fixed step count for cosine decay; replace with a per-night budget and a knowledge-base note of rejected edits.
4. **Stable held-out split by hash of task id, synthetic tasks never in val** — `mine.py:282-286,319-338`; `dream.py:93-94`. Assumes a scalar score per task; replace the score with hindsight oracles from the owner's own sessions (a later user correction, the final accepted output, tool errors) or an LLM rubric judged with a threshold (`backend.py:453` uses `soft ≥ 0.8 → hard`).
5. **Strict-improvement gate with per-task no-regression and an "unverified" verdict when train and val overlap** — `consolidate.py:281-308,432-434`; `gate.py:200-225`. Assumes cheap deterministic re-runs; replace single-run `>` with paired evidence over several runs (the repo's own `evalkit.py:1-16` McNemar/bootstrap) because self-logs are noisier than benchmarks.
6. **Harvest hygiene**: drop the engine's own headless sessions and other agents' sub-agent sessions by prompt markers and <3 s single-turn heuristics; skip slash-command/meta prompts; skip subagent sidechains — `harvest.py:137-236,405-407`. Assumes Claude Code jsonl; replace with a sessions.sqlite query that excludes the dreaming routine's own sessions and child sessions by id.
7. **Conservative LLM miner that emits a generalized intent + rubric + checks + satisfaction flag, drops uncheckable tasks, and groups tasks by the single skill the session invoked** — `prompts.py:32-73`; `llm_miner.py:18-20,46-120`; `mine.py:38-47,242-269`. Assumes the user types satisfaction phrases; replace the outcome signal with re-asks/corrections/tool-failure chains and explicit approvals in the harness.
8. **Minibatch analysts → hierarchical merge with `support_count` → ranking, failure patches prioritized over success patches** — `reflect.py:550-558`; `aggregate.py:156-239`; `merge_final.md:7-16`. Assumes dozens of trajectories per step; replace minibatches with per-skill clusters of sessions across a week.
9. **Longitudinal slow update**: re-run the same tasks under the previous and current skill, categorize improved/regressed/persistent_fail/stable_success, and let a separate prompt rewrite a protected guidance block that step edits cannot touch — `trainer.py:1835-1935`; `prompts/slow_update.md:9-18,38-41`. Assumes re-rollouts; replace with week-over-week outcome comparison on recurring intents.
10. **Optimizer-side meta memory** ("which kinds of edits tend to help / be too vague, redundant, brittle") revised each epoch from evidence — `prompts/meta_skill.md:14-34`; `trainer.py:2087-2197`. Transfers directly as a knowledge-base concept the dreaming routine reads before proposing.
11. **Contrastive reflection on high-spread tasks and skill-defect vs execution-lapse triage** (don't edit the body when the rule existed and was ignored; write an appendix reminder instead) — `skillopt_sleep/rollout.py:93-146`; `skill_aware.py:17-21,61-91`. Assumes K re-rollouts per task; replace spread with contrast between two past sessions of the same intent (one succeeded, one failed).
12. **Evidence chain + diagnostics + live prompt registry + stage-then-adopt** (append-only JSONL per night, `diagnostics.json` so a 0→0 night self-explains, `prompts.json` overrides, proposals staged with manifest/backup and adopted only on explicit command, warning when the model changed between nights) — `evidence.py:24-27`; `cycle.py:1040-1092,665`; `prompts.py:9-12`; `staging.py:1131-1142`. Transfers as artifacts + draft cards; the harness's git-backed knowledge base already gives the backup/receipt.

## K. Anti-borrow list

1. **Shape-only or literal-string checks as the reward** (`section_present`, `max_chars`, `contains`): the optimizer satisfies them by telling the model to emit the string; observed twice — `llm_miner.py:111-115`; `judges.py:19-29`. A self-improving harness must grade substance (rubric/outcome), never format.
2. **Feeding raw verifier output into the proposer**: a regex leaked into reflection produced "a rule that imitates the regex" — `docs/sleep/README.md:107-111`; the fix (generic sentence for non-rule judges) throws information away — `:128-142`. Design a typed, learning-safe feedback channel from the start.
3. **Greedy/ungated acceptance** ("gate_mode off"): −52.8 points in five nights on a weak model — `docs/sleep/RESULTS.md:40-52`; `consolidate.py:264-270`. Never accept a skill edit without a held-out comparison, even for "memory".
4. **Keyword feedback heuristics as ground truth** (`"no,"`, `"wrong"`, `"nice"` → outcome labels) and first-prompt = task / last-message = solution — `harvest.py:30-39`; `mine.py:175-187`. For a general assistant with multi-task sessions these mislabel; use them only as weak pre-filters.
5. **Single-run strict `>` on a ~14-task val set with a deterministic response cache** — `mine.py:275`; `config.py:42,45`; `backend.py:420-423`; `gate.py:200`. With ±1–2 pt noise (`docs/sleep/README.md:443-444`) this accepts noise; require paired repeats or a margin.
6. **"Semantic density" bonus rewarding MUST/ALWAYS/NEVER word share** — `gate.py:46-57,128-130`. It rewards shouting, not quality; and the research path has no length penalty at all while shipped skills grew to 13 KB (`ckpt/README.md:29`). Add an explicit length/readability budget instead.
7. **Prompt-size extremes**: research analysts get full untruncated trajectories ("Truncation is disabled") — `reflect.py:54-62` — while Sleep gives 8 failures × 160 chars — `backend.py:473-477`. Pick a middle: distilled per-session digests with the decisive tool results.

uncertain: test-function count and per-language file counts are my own greps (hidden dirs excluded); the number of val replays per night (~14 of 40 tasks × 3–4 scoring passes) is derived from defaults, not measured in the repo.