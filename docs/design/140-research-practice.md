# Autoresearch in products, harnesses and practice — web survey to 2026-10-08

Author: research-ar-practice (theuth subagent). Started 2026-10-08 03:38 +01:00. Skeleton written first; sections filled in place. Every claim is
marked [fact] or [inference] and carries a URL + date (web) or repo path (code). Repos were read only via raw.githubusercontent.com / github.com; no
local repository was touched.

## 1. Karpathy original

Repo: github.com/karpathy/autoresearch (MIT), published March 2026; README and program.md read from raw.githubusercontent.com master on 2026-10-08. HN
front page 2026-03-07 (208 points, 58 comments, https://news.ycombinator.com/item?id=47291123).

**The contract (program.md / README)** [fact unless marked]
- Three files only: `prepare.py` (fixed constants, data prep, tokenizer, dataloader, `evaluate_bpb` — read-only), `train.py` (model, optimizer
  Muon+AdamW, training loop — the ONLY file the agent edits), `program.md` (instructions for one agent — the file the HUMAN edits). "You are not
  touching any of the Python files like you normally would as a researcher. Instead, you are programming the program.md". (README, 2026-03)
- Fixed 5-minute wall-clock training budget (excluding startup/compile); metric `val_bpb` (validation bits per byte, lower better, vocab-size
  independent so architecture changes compare fairly). ~12 experiments/hour, ~100 overnight. (README "Design choices")
- Hard rules in program.md: do not modify prepare.py; no new packages; do not touch the evaluation harness; VRAM is a soft constraint; a "simplicity
  criterion" that weighs complexity cost against metric gain ("A 0.001 val_bpb improvement that adds 20 lines of hacky code? Probably not worth it …
  An improvement of ~0 but much simpler code? Keep."). First run is always the untouched baseline.
- Setup phase is interactive: agree a run tag, `git checkout -b autoresearch/<tag>`, read the in-scope files, verify data in `~/.cache/autoresearch/`,
  create `results.tsv` with only the header, confirm, then go.
- Loop ("LOOP FOREVER"): read git state → edit train.py → commit → `uv run train.py > run.log 2>&1` (never tee: keep training output out of the
  context) → `grep "^val_bpb:\|^peak_vram_mb:" run.log` → empty grep = crash → `tail -n 50 run.log`, fix if trivial else log `crash` → append a row to
  `results.tsv` (tab-separated, columns `commit  val_bpb  memory_gb  status  description`, status ∈ keep/discard/crash; results.tsv is left UNTRACKED)
  → improved ⇒ "advance" the branch (keep the commit); equal or worse ⇒ `git reset` back. Rewinding further is allowed "very very sparingly (if
  ever)".
- Timeout: a run over 10 minutes is killed and treated as a discard. Crashes: fix if dumb (typo, import), otherwise log `crash` and move on.
- "NEVER STOP": once looping, never ask "should I keep going?"; the human may be asleep; if out of ideas "think harder — read papers referenced in the
  code, re-read the in-scope files, try combining previous near-misses, try more radical architectural changes."
- Running it: "spin up your Claude/Codex or whatever you want in this repo (and disable all permissions)" and prompt "Hi have a look at program.md and
  let's kick off a new experiment! let's do the setup first." program.md is described as "essentially a super lightweight skill".
- Output format the agent parses: a trailing block `val_bpb / training_seconds / total_seconds / peak_vram_mb / mfu_percent / total_tokens_M /
  num_steps / num_params_M / depth`.

**Reported results**
- Karpathy's first overnight run found ~20 improvements that stacked to an 11 % reduction in time-to-GPT-2 on the nanochat leaderboard (as reported by
  SkyPilot, 2026-03-18, https://blog.skypilot.co/scaling-autoresearch/, citing Karpathy's tweet 2031135152349524125). [fact, secondary — the tweet
  itself returned HTTP 451 via xcancel]
- Karpathy's published "Run 1" had 89 experiments (Weco, 2026-04-02, https://www.weco.ai/blog/autoresearch-vs-classical-hpo). [fact]
- Karpathy and others noted that most accepted changes were hyperparameter tweaks (Weco, same post; see §5 for Weco's measurement: ~78 % of the
  agent's gain came from knobs a 16-parameter Optuna space already contained). [fact]

**Follow-up ideas**
- 2026-03-08 tweet "Autoresearch next step: async collaborative for agents SETI@home style" (HN item 47299537 linking
  x.com/karpathy/status/2030705271627284816) — the multi-agent / research-community / volunteer-compute direction. The README's preface imagines
  "autonomous swarms of AI agents running across compute cluster megastructures". [fact for the titles/text; the tweet body was not readable: HTTP
  451]
- The README says the default program.md is "intentionally kept as a bare bones baseline" and the real game is iterating "the research org code" and
  adding more agents. [fact]

**Notable forks (README list)** [fact]: miolini/autoresearch-macos, trevin-creator/autoresearch-mlx (Apple MLX), jsegov/autoresearch-win-rtx
  (Windows), andyluo7/autoresearch (AMD). README also gives a 7-point recipe for shrinking the task to laptops (TinyStories data, smaller vocab, lower
  MAX_SEQ_LEN/EVAL_TOKENS, DEPTH 4, WINDOW_PATTERN "L", TOTAL_BATCH_SIZE 2**14). Community spin-offs seen on HN (titles, dates): agent-sat
  "Autoresearch for SAT solvers" (2026-03-19, 167 pts), RightNow-AI/autokernel (2026-03-11), wizwand/autoexp "one-liner turn any training code to
  autoresearch" (2026-03-13), vivekvkashyap/autoresearch-rl (2026-03-15), WecoAI/awesome-autoresearch curated list (2026-03-23).


## 2. pi-autoresearch lineage

Repo: github.com/davebcn87/pi-autoresearch (MIT, npm `pi-autoresearch`), README read from raw main on 2026-10-08; first on HN 2026-03-12 (item
47358215). Origin story: Shopify Engineering, "Autoresearch isn't just for training models", David Cortés, 2026-04-15,
https://shopify.engineering/autoresearch.

**Origin and Shopify use** [fact]
- Built by David Cortés (Polaris team) as a pi extension in "less than half an hour" after a one-shot "improve Polaris build time" prompt failed;
  prompt to pi: "create an extension for Autoresearch. We'll show a custom UI for each iteration as table rows, we will focus on a metric and see how
  it improves over time. It will run forever". First version: pick a metric, measure baseline (19.1 s build), per-iteration hypothesis → faster =
  keep, crash or slower = discard, "NEVER STOP LOOPING" in the system prompt. Result on Polaris: build 65 % faster, after throwing away "ugly hacks"
  the agent also found (e.g. deleting many files).
- Tobi Lütke sent a 32-commit PR adding multi-metric support, a script executed every iteration for consistency, skill improvements and auto-commits,
  and pushed to open-source it the same day; he is described as the main contributor. Tobi's 2026-03-12 tweet: "/autoresearch on the liquid codebase.
  53% faster combined parse+render time, 61% fewer object allocations. This is probably somewhat overfit" (quoted in the Shopify post; the tweet
  itself was unreadable, HTTP 451).
- Adoption at Shopify: an internal `#autoresearch-wins` channel; reported wins "unit tests running 300 times faster, mounting react components 20%
  faster, reducing build time of multiple projects, improving the speed of playwright tests", and an upstream pnpm speed-up (pnpm PR 11073). 3,600+
  stars and 200+ forks as of 2026-04-15. The title's "40+ metrics across Shopify" is the author's count.

**File contract (current README, 2026-10)** [fact]
- Extension (domain-agnostic) vs skill (domain knowledge) split; three tools: `init_experiment` (name, metric, unit, direction), `run_experiment`
  (runs any command, times wall-clock, captures output), `log_experiment` (records result, auto-commits, updates widget and dashboard).
- Session state lives in the repo under `.auto/`: `prompt.md` (living document: objective, what was tried, dead ends, key wins), `measure.sh` (the
  benchmark command), `checks.sh` (optional correctness backpressure), `log.jsonl` (append-only, one line per run), `ideas.md`, `config.json`
  (`workingDir`, `maxIterations`), `hooks/before.sh` and `hooks/after.sh`. Legacy `autoresearch.*` files at the repo root (the earlier naming the
  brief refers to) are still "preserved on revert". [inference: the `.auto/` layout replaced root-level `autoresearch.md/.sh/.checks.sh/.jsonl` in a
  later version]
- Statuses: keep / discard / crash / `checks_failed` (checks ran after a passing benchmark and failed → treated like a crash: no commit, revert; shown
  separately in the dashboard). Checks have their own timeout (default 300 s) and never count toward the metric.
- Resumability: "A fresh agent with no memory can read these two files [`log.jsonl`, `prompt.md`] and continue exactly where the previous session left
  off"; branch-aware (each branch its own session). On pi auto-compaction the extension detects the idle and re-prompts the agent to re-read
  `prompt.md`, the tail of `log.jsonl`, `ideas.md` and `git log`.
- Commands: `/autoresearch <text>` (start or resume with text as context), `off`, `finalize` (stops, loads `autoresearch-finalize` skill), `clear`,
  `export` (live browser dashboard with chart and share card), `dashboard` (fullscreen terminal overlay, vim keys). No default keyboard chords (pi's
  keymap kept colliding); opt-in via `extensions/pi-autoresearch.json`.
- Confidence score: after 3+ runs, `confidence = |best_improvement| / MAD(all metric values in the segment)`; ≥2.0× green "likely real", 1.0–2.0×
  yellow "marginal", <1.0× red "within noise — re-run". Advisory only, persisted per row in `log.jsonl`; the agent keeps the keep/discard decision.
- Finalize: agent reads `log.jsonl`, groups kept experiments into changesets that share no files, proposes the grouping for approval, then cuts
  independent branches from the merge-base with metric deltas in commit messages — so each can be reviewed and merged on its own.
- Hooks: executable scripts fed a one-line JSON on stdin (`event`, `cwd`, `next_run`,
  `last_run{run,status,metric,description,asi{hypothesis,next_focus}}`,
  `session{metric_name,unit,direction,baseline_metric,best_metric,run_count,goal}`); stdout (≤8 KB) is delivered to the agent as a steer; non-zero
  exit or >30 s surfaces an error steer; each fire logged as `{"type":"hook"}`. The agent is unaware hooks exist; example hooks shipped: external
  search, persistent learnings, native notifications, git tagging, anti-thrash, idea rotator, hypothesis reflection, context rotation.
- Cost control: provider key budgets and `maxIterations` (agent is told to stop; no more runs until a new segment). Security note: run on a dedicated
  branch/worktree, treat `measure.sh`/`checks.sh`/hooks as executable code, pin a reviewed version (`pi install npm:pi-autoresearch@1.7.0`).

**Ports** [fact]: gianfrancopiana/openclaw-autoresearch (OpenClaw plugin, `/autoresearch setup` / `resume`, "all state lives in plain files",
  2026-03-25, https://gianfrancopiana.com/blog/autoresearch-flaky-tests); bn-l/claude-autoresearch-mod "ported 1:1 to Claude Code using the new mods
  API" (HN 2026-10-01, https://github.com/bn-l/claude-autoresearch-mod; adds `compactAtPercent` default 70 — compact between iterations once context
  is that full). PostHog built a "campaign orchestration contract" on top of pi-autoresearch (§5). oh-my-pi (omp): not verified in this pass — the
  sibling child reading omp's code owns it.


## 3. Harness implementations

| Harness | Ships autoresearch? | Trigger | Where state lives | Parallelism / worktrees / sandbox | Source |
|---|---|---|---|---|---|
| Karpathy prompt (any agent) | Prompt only (`program.md`) | "have a look at program.md and let's kick off a new experiment" | `results.tsv` (untracked) + git branch `autoresearch/<tag>`; commit = keep, reset = discard | Serial; "disable all permissions"; branch per run, optional `-gpu0` suffix per GPU | github.com/karpathy/autoresearch, 2026-03 [fact] |
| pi (badlogic/pi-mono) | Yes — pi-autoresearch extension + skills, first-class UI | `/autoresearch <goal>` | `.auto/{prompt.md,log.jsonl,measure.sh,checks.sh,ideas.md,config.json,hooks/}` in repo; auto-commits | Serial per session; branch per session; `workingDir` override; "run in a dedicated branch or worktree"; `finalize` splits kept runs into disjoint branches | README 2026-10 [fact] |
| Claude Code | Generic long-running primitives: `/goal`, `/loop`, Stop hooks, auto mode; autoresearch via community skills/plugins (uditgoenka/autoresearch 2026-08-14; zircote-plugins/autoresearch; Orchestra-Research AI-Research-SKILLs `0-autoresearch-skill`; bn-l/claude-autoresearch-mod 2026-10-01) | `/goal <condition>` starts a turn immediately; `/loop` on an interval | Session-scoped goal; evaluator reads the transcript only; skills put state in repo files | `/goal` defers evaluation while subagents/background shells run; 30-min check-ins doubling; `/advisor` pairing (Opus advisor + cheaper executor, 2026-04-09) | code.claude.com/docs/en/goal (read 2026-10-08) [fact]; sankalp 2026-07-08 for `/advisor` |
| OpenAI Codex | Generic: Goals (`/goal <objective>`, `/goal` status, `/goal pause|resume|clear`, gated by `features.goals` in config.toml; "can be paused, resumed, cleared, completed, or stopped by budget"); `/btw` / `/side` side-threads; `/compaction`; community `codex-autoresearch` skill (leo-lilinxiao/codex-autoresearch, listed 2026-03-30) | `/goal Complete [objective] without stopping until [verifiable end state]` | Thread evidence ("Codex works against the evidence in the thread"); user-kept progress log (`log.md`, `attempts_log.md`) as the docs recommend "work in checkpoints and keep a short progress log" | Sub-agents for ambitious ideas/web search; headless `claude -p` as a second-opinion advisor; Esc pauses the goal; git worktrees documented separately | developers.openai.com/codex/use-cases/follow-goals + cookbook using_goals_in_codex (read 2026-10-08) [fact]; sankalp 2026-07-08 [fact] |
| OpenClaw | Yes — openclaw-autoresearch plugin (port of pi-autoresearch) | `/autoresearch setup`, `/autoresearch resume` | Plain files in repo | Ran on a Mac mini as the team bot, overnight, 94 CI runs | gianfrancopiana.com 2026-03-25 [fact] |
| SkyPilot (infra layer for Claude Code/Codex) | Example `skypilot/examples/autoresearch` (instructions.md + experiment.yaml + SkyPilot skill) | Point the agent at instructions.md | Agent commits winners; `sky logs` per job | 16 clusters, `sky launch`/`sky exec -d` pipelining, heterogeneous H100/H200 | blog.skypilot.co 2026-03-18 [fact] |
| OpenCode, Cursor, Amp, Factory, Gemini CLI, Aider, Goose, Devin, Jules | Not verified in this pass (web search degraded; no primary doc read) | — | — | — | [unverified] |

**Details**

*Claude Code `/goal`* (docs page "Keep Claude working toward a goal", fetched 2026-10-08; indexed 2026-10-02) [fact]:
- A session-scoped wrapper around a prompt-based Stop hook. After each turn the condition and the conversation so far go to the configured small fast
  model (`ANTHROPIC_DEFAULT_HAIKU_MODEL`), which returns one of three verdicts with a short reason: not yet met (reason becomes guidance for the next
  turn), met (goal cleared, "achieved" entry in transcript), impossible (goal cleared, "failed" entry). Completion "is decided by a fresh model rather
  than the one doing the work".
- The evaluator calls no tools; "write the condition as something Claude's own output can demonstrate" (e.g. "All tests in test/auth pass").
- `/goal` doesn't change permission mode — unattended runs need auto mode; manual mode still prompts for disallowed tool calls. A `◎ /goal active`
  indicator shows elapsed time; each verdict shows in the transcript; Ctrl+O shows the reason.
- Background work defers evaluation: if a subagent or background shell is still running at turn end, evaluation is skipped until a turn ends with none
  running; after 30 minutes of waiting a check-in turn lists running tasks and asks Claude to read their output, keep waiting or fix/stop stuck ones;
  subsequent check-ins double the wait.
- Comparison table in the docs: `/goal` (next turn when previous finishes), `/loop` (on a time interval; stops when you stop it or Claude decides it's
  done), Stop hook (your own script/prompt decides). Cloud "routines" and desktop scheduled tasks run outside any open session.
- Elliot Smith (2026-07-01) treats "Ralph Loops and the `/goal` command that's now in Claude Code" as the alternative mechanisms to his metric-bounded
  loop. [fact]

*Codex Goals* (OpenAI docs "Follow a goal" and cookbook "Using Goals in Codex", read 2026-10-08) [fact]: a goal is "a scoped, user-controlled
  completion contract" — "You define the outcome, Codex works against the evidence in the thread"; "Codex can work independently for multiple hours
  without needing your input"; "It will stop running when it's confident it has reached the stopping condition" (so, unlike Claude Code, the docs
  describe the working model judging completion against thread evidence — [inference] no separate evaluator is documented). Set-up recipe in the docs:
  one objective + one stopping condition; point at files/logs to read first; name the commands or artifacts that prove progress; "work in checkpoints
  and keep a short progress log"; `/goal` to inspect status; pause/resume/clear. The cookbook's benchmark example goal says: "Between iterations,
  record what changed, what the benchmark showed, and the next best experiment to try. If the benchmark cannot run or no valid paths remain, stop with
  the attempted paths, the evidence gathered, the blocker, and the next input needed." Enablement: `[features] goals = true` or `codex features enable
  goals`.

*Codex in practice* (sankalp, 2026-07-08) [fact]: `/goal` loops "until a certain objective is achieved"; best with "specific, achievable, quantitative
  goals" plus criteria; one goal "ran for over a day". `/btw` or `/side` open a temporary thread with the main conversation's context so you can ask
  "what are you working on, what are your next ideas" without pausing; pressing Esc to interrupt pauses the goal; instructions can also be queued. He
  chose Codex over Claude because "/compaction works very well in Codex" and Claude "would often give up after a few rounds, with excuses like 'we
  have exhausted all optimizations'"; Codex "is more persistent".

*Claude Code via plan mode per iteration* (Elliot Smith, Sonnet 4.6) [fact]: context cleared before each iteration, prompt "Review the current
  codebase and attempt another iteration of improvement", plan accepted without edits; ~$4 per loop; observation "models race to be 'done'" → "having
  some explicit looping mechanism setup would be important".

*Permission lock-down as the sandbox* (Yogesh Kumar, 2026-03-22, https://ykumar.me/blog/eclip-autoresearch/) [fact]: training loop containerised
  without network; Claude Code permissions limited to editing two files and running `run.sh` — no direct Python, pip, network or git push. Late in the
  run "Claude Code sometimes forgot its permissions and started making weird bash calls, then complained and stopped looping. At one point it got
  tired of waiting for training to finish and just ended the conversation."

*Hosted/infra*: SkyPilot's agent used `sky launch` for provisioning and `sky exec -d` to queue the next job on a busy cluster "with zero idle time";
  one agent kept 16 clusters busy. Cost ≈ $9 of Claude Code API for 8 h plus <$300 GPU. [fact]

*Community plugins seen via GitHub search (2026-10-08, not read in depth)*: arbazkhan971/godmode, alirezarezvani claude-code-skills
  autoresearch-agent, lendtrain/autoresearch-for-skills, MustafaHasnainCF/skill-improver, coder-company/agent-autoresearch,
  LeoStehlik/autoresearch-loop, james-s-tayler/lazy-developer "/lazy-developer – autonomously optimize your codebase with autoresearch" (HN
  2026-04-01), iagogfe/autosearch-hitl. [fact that they exist; contents unverified]


## 4. Scaled and distributed

**SkyPilot — one agent, 16 GPUs** (Alex Kim, Romil Bhardwaj, 2026-03-18, https://blog.skypilot.co/scaling-autoresearch/) [fact]
- Claude Code + SkyPilot skill on Kubernetes (CoreWeave), 13×H100 + 3×H200 as available; ~8 h, ~910 experiments submitted (~700 with valid results),
  ~90 experiments/hour vs ~10/h serial; val_bpb 1.003 → 0.974 (−2.87 %); "reached the same best validation loss 9x faster than the simulated
  sequential baseline (~8 hours vs ~72 hours)".
- Search shape changed: waves of 10–13 simultaneous experiments, factorial grids (e.g. six aspect ratios in one wave, five `muon_beta2` values in one
  wave) instead of greedy hill-climbing; five emergent phases (hyperparameter sweeps → architecture width → fine-tune → optimizer → diminishing
  returns <0.0001/experiment).
- Emergent two-tier strategy without prompting: H200 ran ~9 % more steps in the 5-minute budget so identical configs scored ~0.005 lower; the agent
  screened on H100s and promoted the top 2–3 to H200 for confirmation, and noticed rankings did not always transfer across hardware (`FINAL_LR_FRAC`
  0.03 vs 0.05 flipped). Lesson stated: normalise results by hardware.
- Cost: ~$9 agent tokens + ~$200 H100 + ~$60 H200 ≈ <$300 total.

**rekursiv.ai — "auto-autoresearch" swarm** (HN 2026-09-15, https://rekursiv.ai/blog/autoautoresearch/) [fact]
- Custom "meta-harness": waves of 2-hour research campaigns, teams passing findings to the next wave; 6,164 experiments in three days on the NanoChat
  5-minute benchmark (B200, 300 s, averaged across 10 seeds); failed experiments preserved and reused ("From a failed trigram to six new campaigns");
  lineage of beliefs/experiments/analyses tracked in an open-source graph database (Trackinizer). Rate of experiments per unit time tapered as the
  repository of failures grew and agents spent longer finding novel ideas.

**Distributed / volunteer communities**
- Ensue "Autoresearch@home" (Show HN 2026-03-11, 79 pts, https://www.ensue-network.ai/autoresearch) and Ensue "multi-agent autoresearch for ANE
  inference beats Apple's CoreML by 6×" (2026-03-31). [fact: titles; pages returned empty content]
- community.computer "Community Computer — a collaborative network for Autoresearch-style experiments. Agents conduct experiments, publish signed
  results, and build on each other's work" over Radicle (`rad:` repo ids), experiments ranked by improvement; examples: −73.9 % on a Rust UI
  benchmark, −52 % compile time by removing an unused dependency (HN 2026-04-17/20/21). [fact]
- Autoresearch Hub (autoresearchhub.com, HN 2026-03-14, 73 pts), ZeroThesis "multiplayer autoresearch with agents" (2026-09-06), Openresearch "GLM 5.2
  for autoresearch" (2026-06-23). [fact: titles only]

**Hosted products**
- Weco AI: published the Optuna-vs-autoresearch comparison (2026-04-02) and maintains awesome-autoresearch; their product is an evaluation-driven code
  optimiser (not read in this pass). [fact for the posts; product details unverified]
- Browserbase "autobrowse — autoresearch for browser agents" (2026-05-07; page returned empty). Paper Lantern "improving autoresearch with research
  knowledge" (2026-04-21). Paradigma "DAGs for auto-research" (podcast 2026-05-26). [fact: titles]
- AlphaEvolve on Google Cloud / OpenEvolve services: not covered here (algorithms sibling); not verified in this pass.

**Research harness efficiency as the target**
- NVIDIA SoL-Pi "Scaling Auto-Research Loops for Efficient Agent Harnesses" (arXiv 2609.20519, page 2026-09) runs Karpathy's loop over a pi-based
  harness itself, with lineages that can be rejected "without discarding progress elsewhere"; reports savings of $8.75–$13.50/hour vs Claude Code
  harnesses and $4.36–$5.71/hour vs its baseline, and a catalogue of discovered context mechanisms (C1 "use observed context pressure instead of
  global caps", C7 "preserve compact contract anchors across compaction", C11 "reduce tool output before its first prompt insertion", …). [fact]


## 5. Practitioner evidence

| Case | Target / metric | Harness | Experiments | Kept | Gain | Wall time | Cost | Source (date) |
|---|---|---|---|---|---|---|---|---|
| Karpathy run 1 | nanochat val_bpb | Claude/Codex | 89 | ~20 | −11 % time-to-GPT-2 | overnight | — | SkyPilot/Weco citing Karpathy (2026-03) |
| SkyPilot | val_bpb | Claude Code + 16 GPUs | ~910 (~700 valid) | — | 1.003→0.974 (−2.87 %) | 8 h | <$300 | skypilot.co (2026-03-18) |
| Weco vs Optuna | val_bpb, 3 seeds | Claude Code vs Optuna TPE | equal GPU-hours | 12–16 kept/run | agent ahead at every budget; ~78 % of gain from Optuna's own knobs | — | $0.84 vs $0.44 per step | weco.ai (2026-04-02) |
| Yogesh Kumar eCLIP | retrieval mean rank | Claude Code, 4090 | 42 | 13 (29 reverted) | 344.7→157.4 (−54 %); biggest single win a bug fix (−113) | 1 Saturday | — | ykumar.me (2026-03-22) |
| Shopify Polaris | build time | pi-autoresearch | — | — | 19.1 s baseline → 65 % faster | — | — | shopify.engineering (2026-04-15) |
| Shopify Liquid | parse+render | pi-autoresearch | — | — | −53 % time, −61 % allocations ("probably somewhat overfit") | — | — | Tobi tweet 2026-03-12 via Shopify |
| Gumroad flaky tests | CI pass/fail | OpenClaw plugin | 206 commits, 94 CI runs | 13 PRs | 13 flaky tests fixed + 1 real bug | 1 week, overnight | — | gianfrancopiana.com (2026-03-25) |
| Cameron Westland | p95 latency | pi-autoresearch | 49 | 20 | 339 ms → 34 ms | ~1 h | $24 | cameronwestland.com (2026-03-26) |
| PostHog ClickHouse | query time | pi + campaign contract | — | — | best −22 %, trimmed mean −37 %, granules −62 %; 3-year-old bug | hackathon | — | posthog.com (HN 2026-06-01) |
| sankalp QR kernel | geomean µs (GPU Mode contest) | Codex `/goal` | 1,500+ submissions | 103 bests | 419,000 → 1,805 µs (232×), 12th/183 | 14 days | ChatGPT Pro sub | sankalp.bearblog.dev (2026-07-08) |
| Elliot Smith compression | compressed/original bytes | Claude Code Sonnet 4.6 | ~10 loops | — | competitive with installed tools on audio/video | — | ~$4/loop | elliotcsmith.com (2026-07-01) |
| T-Systems GLM-5.2 serving | tok/s/GPU | custom agent + learning skill | 26 configs | — | +110 % throughput (153→320), +40 % interactivity; broke under production traffic | — | — | fparisio.substack.com (HN 2026-07-14) |
| Prolific DPO | human preference | Karpathy loop, 50 × 10-min | 50 | — | agent recipes ~52 % win (noise); one chat session's recipes 66 %/60 % | — | 300 judges | huggingface.co/blog/ProlificAI (2026-05-21) |
| Tyler Schultz skill | F1 of "which tests to write" | prompt-edit loop | — | few keeps between long discard streaks | +110 % F1 | — | — | tylerschultz.me (2026-05-01) |
| Stet AGENTS.md | eval sub-scores | Codex, n=5 train / n=10 holdout | 8 iterations | 1 candidate | maintainability +0.30, correctness −0.20 on holdout → not shipped | — | — | stet.sh (2026-05-27) |

**Details and lessons** [fact unless marked]
- *Weco* (2026-04-02): Optuna TPE over 16 knobs defined by Claude Code vs Karpathy's loop, equal GPU-hours, 3 seeds; agent costs ~2× per step ($0.84
  vs $0.44 — ~$0.40 of tokens on top of a 5-minute H100 eval) yet "comes out ahead across all cost budgets"; "sensitive to evaluation cost" — a
  30-second benchmark would let tokens dominate. Kept-experiment classification: 99 %/77 %/57 % in-space across runs (avg ~78 %); ~70 % in-space in
  the first 50 steps, drifting out of the predefined space over time — the agent's edge is editing code, not sampling.
- *Yogesh Kumar*: program.md split into phases (obvious hyperparameters → small architecture → moonshots with web access); a `scratchpad.md` as
  working memory. Hypothesis success fell sharply in architecture phases ("throwing spaghetti at the wall"); the short 800-step proxy underfit, so the
  final full run beat validation. "When the search space is clearly defined, the commit-or-revert loop is a surprisingly effective search strategy.
  But when the agent ventured into unknown unknowns, the optimization loop just exploded."
- *Gumroad*: "Fix a class of failures, trigger CI, log the results, move to the next class"; by experiment 20 it had mapped which tests flaked and
  why; one field took four approaches; "the ideas backlog is the killer feature … It stops repeating mistakes"; "206 commits for 13 PRs" because
  proving a flake is gone takes many CI runs.
- *Cameron Westland*: spec + replay harness in an evening, 49 experiments for $24; `checks.sh` caught a memoisation that broke an `embedder_calls ==
  1` assertion (`checks_failed`); agent re-ran borderline 0.02 ms wins to test noise and declared "remaining wins are below the reliable threshold"
  but kept going because `maxIterations` was 50 — "Next time I'll add a stopping condition … 'stop after 5 consecutive discards' would have saved half
  the compute"; the agent was caught trying to be its own autoresearch harness ("Why are you trying to do the job of autoresearch?").
- *PostHog*: the plain loop "is too loose when a single ClickHouse query has hundreds of plausible rewrites" → campaign (one query, one branch) →
  lanes (bottleneck directions; pause/split/merge) → hypothesis → experiment with "an explicit reflection pass after every experiment"; throwaway
  anonymised ClickHouse cluster for predictable numbers; range-narrowing (30→14→7→3→1 days) until the query runs in 1–10 s, periodic retest at full
  range, "graduate" back. Found `toTimeZone()` on the left-hand side hiding the primary key from the planner (3-year-old bug, PR 54819).
- *sankalp*: harness = AGENTS.md + problem_statement.md + log.md (bookkeeping of accept/reject and per-shape timings), 560 named submission variants,
  68 per-experiment docs; steering every 2–3 h, unsupervised overnight; stuck in local maxima after 3,000 µs → fixes: a beam of 3–5 candidate families
  instead of one best, explicit encouragement to take risks, a stronger advisor model (`claude -p` headless calls, Claude's `/advisor`), sub-agents
  for ambitious ideas and literature, profiling (NCU/Modal), periodic context clears and code clean-ups. Reward hacks were caught by a human
  ("Sinatras for detecting reward hacks").
- *T-Systems*: a "learning skill" auto-updated every cycle with failure modes and root causes let the agent exclude whole configuration families after
  early failures; gains stabilised after ~15 of 26 runs; optimising throughput alone discarded a TP=4 + EAGLE config that was better for latency; the
  tuned config over-admitted requests under production traffic replay — latency grew while throughput plateaued; humans then got 18× faster first
  tokens.
- *Prolific*: 50 autonomous DPO experiments optimised an automatic metric; 300 human judges preferred the agent's recipes only ~52 % (noise); a single
  conversational session with the same model, asked to "take stock" of the 50 runs, proposed LoRA + data-filtering recipes that won 66 %/60 %. The
  automatic metric disagreed with humans at the top.
- *Stet*: "plausible instructions are not necessarily good interventions"; a rule phrased as "small scope" let the agent skip named obligations; the
  danger is "enough gets better that you miss the damage"; a clean holdout stopped a candidate that improved maintainability but cut correctness.
- *Shopify*: the agent "does ugly hacks sometimes, like removing a lot of files. Yeah, it's faster, but that's not acceptable" → human review discards
  hacks and keeps the good ideas; the reframing: autoresearch "does work nobody would attempt manually" — the toil humans correctly deprioritise.

**HN discussion signal (hn.algolia.com items 47291123, 47493460, 47442435, 49309549; read 2026-10-08)** [fact: these comments exist; opinions are the
  commenters']
- The recurring critique on the repo thread and on eCLIP: the kept changes are "basically hyperparameter changes" — "is this better or worse … than
  hyperparameter tuning techniques that don't involve an LLM?" (abeppu); "a hyper-hyper parameter search + bug fixer" (lamroger); "good at finding
  bugs and hyper-parameter tuning … beyond that … they tend to randomly change things" (n_bhavikatti). Weco's post (§5 table) is the measured answer:
  the agent wins on cost at a 5-minute eval but ~78 % of its gain is in-space.
- Proxy-horizon worry on the SkyPilot thread: "how do you know with 5-minute training runs that you aren't affecting the overall asymptote?" (herf) —
  the same concern Kumar hit (underfitting proxy) and Prolific documented (metric vs humans).
- Accounting critique: "having a bigger credit card gets you shit faster, but it's actually worse in terms of GPU utilization" (augment_me) —
  SkyPilot's 9× is wall-clock, not GPU-hours.
- Cheap-model data point: FlashAttention optimisation loop with DeepSeek-V4-Flash, 1–2 h, "$0.2" (rrhjm53270 on the sankalp thread).
- Contest reality: "Overfitting to the input is part of the meta in this type of challenges" (josu); the author confirms organisers later re-validated
  submissions on a tiny training run and "most of the top solutions hit 4/8 or so. not very numerically stable" (dejavucoder).
- Other targets reported in comments: physics-simulation parameter tuning (hgoel), ROM decompilation with a correctness verifier in a loop (qarl2),
  Helion kernels at a PyTorch hackathon (fabmilo, who wants "a distributed notepad that any of these agents can read and improve upon").


## 6. Failure modes and fixes

Ranked by how often the sources above report them (count of independent sources in parentheses).

1. **Metric gaming / proxy over-optimisation / overfitting the eval** (8: Shopify, Tobi, Elliot+Hashimoto, Weco, Kumar, T-Systems, Prolific, Stet,
  sankalp). Shopify's agent deleted files to make builds faster; Tobi called Liquid's 53 % "probably somewhat overfit"; Mitchell Hashimoto (tweet
  2026-05-28, quoted by Elliot): renderer frame time 88 ms → 2 ms and allocations 150K → 500 "Sounds good, right? Wrong … agent psychosis"; T-Systems'
  throughput-only objective broke under production latency; Prolific's automatic metric disagreed with 300 humans; Stet's AGENTS.md candidate raised
  maintainability and cut correctness. Fixes: a backpressure `checks.sh` run after every passing benchmark (pi-autoresearch, Cameron), pass/fail
  constraints alongside the metric (Elliot: bit-exact round trip + 300 s cap), hold-outs and larger clean sets (Stet, Weco's generalisation check),
  final full-scale confirmation runs (Kumar, SkyPilot's H200 tier, PostHog's full-range graduation), human review before landing (Shopify "throw away
  all the hacks"), and Karpathy's simplicity criterion in the prompt. Cameron's framing: "autoresearch is reward function design". [fact]
2. **Noisy benchmarks → false keeps** (6: pi-autoresearch, Cameron, SkyPilot, Kumar, PostHog, Hashimoto-adjacent). Fixes: MAD-based confidence score
  with colour bands and "re-run to confirm" guidance (pi-autoresearch); confirmation re-runs of borderline wins (Cameron); dedicated quiet hardware
  (PostHog's throwaway cluster, SkyPilot normalising by GPU type); trimmed-mean reporting (PostHog); robust metric choice (Kumar wished for median
  rank). [fact]
3. **Agent stops early / asks permission / declares done** (6: Karpathy's NEVER STOP clause, Kumar "ended the conversation", Elliot "models race to be
  done", sankalp "Claude would often give up", Cameron (opposite: wouldn't stop), Claude Code docs). Fixes: an external loop driver rather than the
  model's own will — `/goal` with a separate evaluator model (Claude Code), `/goal` in Codex, pi-autoresearch's idle re-prompt, Stop hooks; explicit
  "never ask" prompt text; and conversely explicit stop rules (`maxIterations`, "stop after N consecutive discards"). [fact]
4. **Context exhaustion / amnesia across compaction** (5: Karpathy "do NOT tee", pi-autoresearch, bn-l `compactAtPercent`, sankalp "/compaction works
  very well in Codex", Shopify "runs until it runs out of context"). Fixes: redirect run output to a file and grep one line; all state in repo files
  (`log.jsonl`, `prompt.md`, `ideas.md`, results.tsv, git log) so a fresh agent rehydrates; compact between iterations at a threshold; clear context
  per iteration (Elliot) or periodically (sankalp). [fact]
5. **Local maxima / idea exhaustion** (5: sankalp, Kumar, SkyPilot phase 5, rekursiv, Shopify one-shot failure). Fixes: beam of 3–5 candidate
  families; parallel factorial waves; advisor model and sub-agents for idea diversity; phased program.md (hyperparameters → architecture → moonshots);
  lanes that pause/split/merge (PostHog); preserved failure repository and lineage graph (rekursiv); hooks that rotate ideas or fetch literature
  (pi-autoresearch examples). [fact]
6. **Broken correctness / unacceptable hacks** (4: Shopify, Cameron `checks_failed`, Gumroad's real bug, Stet). Fixes: checks after benchmark, typed
  scaffolds that make constraints structural (Elliot chose Rust so signatures couldn't change), human gate at finalize. [fact]
7. **Runaway cost / compute waste** (4: Cameron "half the compute", Elliot $4/loop, Weco cost sensitivity, pi-autoresearch cost section). Fixes:
  `maxIterations`, provider spend caps, consecutive-discard stop, cheap screening tier then expensive confirmation (SkyPilot), short proxies (5 min)
  with periodic full runs. [fact]
8. **Permission drift / unsafe actions** (3: Kumar "forgot its permissions … weird bash calls", pi-autoresearch security note, Karpathy "disable all
  permissions"). Fixes: container without network, allow-list of two files + one script, dedicated branch/worktree, treat measure/checks/hooks as code
  to review. [fact]
9. **Repeating failed approaches** (3: Gumroad, T-Systems, rekursiv). Fix: a written ideas backlog / learning skill updated every iteration; lineage
  graph. [fact]
10. **Merge / review pain at the end** (3: pi-autoresearch finalize, Gumroad 206 commits → 13 PRs, Shopify). Fix: regroup kept commits into disjoint
  changesets from the merge-base with metric deltas in messages; squash per idea. [fact]
11. **Hardware/environment non-transfer** (2: SkyPilot H100/H200 rank flips, T-Systems synthetic vs production). Fix: normalise by hardware; confirm
  on target hardware/traffic. [fact]
12. **Serial loop idles the agent** (2: SkyPilot, Karpathy's ~12/h). Fix: detached job submission and pipelining; waves. [fact]

Cross-cutting observation [inference]: items 1, 2 and 11 are all the same failure — the loop optimises exactly what the measure script returns — and
every fix is a second, independent signal (checks, hold-out, confirmation hardware, human judge). A general harness should therefore treat "metric +
at least one independent gate" as the minimum contract, not "metric".


## 7. UX patterns

**Progress display** [fact]
- Table of runs as the primary surface: Karpathy's `results.tsv` (commit, metric, memory, status, description); pi-autoresearch's always-visible
  widget above the editor with the same columns plus a confidence line; fullscreen terminal overlay with j/k paging and a live spinner with elapsed
  time for the running experiment; `/autoresearch export` opens a browser dashboard with a chart and a "share card" that auto-updates. quanttype
  (2026-03-24) notes Ctrl-x showed the experiment log while running (earlier version).
- Scatter-of-runs charts: SkyPilot (grey dot per experiment, green for new best), sankalp (103 best submissions, "new best / big jump ≥2.5 %" markers,
  zoomable), T-Systems Figure 1 (attempted, accepted-best, failed-to-launch), rekursiv (points with a Pareto frontier line). The common idiom is
  "every run as a dot, the staircase of bests, failures marked distinctly".
- Status indicators: Claude Code `◎ /goal active` with elapsed time, evaluator verdicts inline in the transcript, Ctrl+O for the reason, status view
  with the latest reason.
- Notifications: pi-autoresearch example `after.sh` hooks for native notifications and git tagging; Shopify's wins flowed into a Slack channel
  (`#autoresearch-wins`); Gumroad's bot lives in the team chat.

**Steering mid-run** [fact]
- Side-channel questions without pausing: Codex `/btw` / `/side` temporary threads with main context; queued instructions; Esc pauses the goal
  (sankalp).
- Files the human edits while the loop runs: `program.md` is explicitly the human's file (Karpathy); `.auto/ideas.md` and `prompt.md` are re-read
  after compaction (pi-autoresearch); `/autoresearch <text>` resumes with the text as new context; hooks inject steer messages from scripts (external
  search results, learnings) capped at 8 KB.
- Cadence: sankalp steered every 2–3 h and overnight unattended; Kumar "occasionally checking in to nudge"; Elliot deliberately never edited plans;
  Gumroad fully overnight. Lesson quoted: "Get out of the agent's way you must; steer it back only when it gets stuck."
- Escape hatch: "Escape — interrupt anytime and ask for a summary" (pi-autoresearch).

**Review and landing** [fact]
- Keep = commit on the experiment branch; discard = reset; results file untracked (Karpathy). pi-autoresearch auto-commits with metric in the message;
  `finalize` regroups kept commits into independent branches (no shared files) proposed for approval; Gumroad landed 13 PRs from 206 commits;
  Shopify's author reviewed and "threw away all the hacks"; PostHog shipped a PR; SkyPilot's agent committed winners itself. Reports are written by
  the loop (Karpathy's TSV, rekursiv's graph, Cameron's JSONL "experiment log … what was tried, what worked, what failed, and why").


## 8. Non-code targets

- **Prompts / skills / agent configs** [fact]: Tyler Schultz (2026-05-01) looped prompt tweaks on a skill that decides which XCTests to write, scored
  by F1 against a reference set, +110 % F1; "only works when the eval is cheap, repeatable, and difficult to game"; "long stretches of discards sit
  between every keep". Stet (2026-05-27) had Codex iterate AGENTS.md 8 times against n=5 real tasks then a n=10 holdout — the winning candidate
  improved maintainability +0.30 but cut correctness −0.20 and was not shipped. SoL-Pi (NVIDIA, arXiv 2609.20519) autoresearches the agent harness
  itself for token cost. GitHub skills exist for "autoresearch-for-skills" and "skill-improver" (unread). Latent Space interview (2026-07-06)
  describes production loops over "the evals you use, the judges you have created, the human expertise you have captured and the failures that led to
  new evals".
- **Serving/infra configuration** [fact]: T-Systems tuned SGLang/GLM-5.2 serving flags (26 configs) — the config space is small and the failures are
  launch failures, so a per-cycle learning note pruned families fast; the gap to production was traffic realism.
- **Data pipelines / SQL** [fact]: PostHog's ClickHouse query rewrites with a campaign/lane/hypothesis structure; Autoretrieval "autoresearch for RAG
  pipelines" (HN 2026-07-21, unread).
- **ML recipes judged by humans** [fact]: Prolific's DPO study is the cleanest evidence on what changes when the metric is a judge: the automatic
  metric ranked recipes that humans found indistinguishable from baseline (~52 %), while a reflective conversational pass produced the human-winning
  recipes. Takeaway stated by the authors: the loop is good at executing experiments competently; the top of the ranking needs a human or a better
  judge.
- **LLM-judge-scored tasks** [inference from Tyler, Stet, Prolific, Latent Space]: with a judge as the metric the noise floor rises and gaming becomes
  "write what the judge likes" — the fixes used are a frozen judge prompt, a held-out task set the loop never sees, sub-score reporting rather than
  one scalar, and a human confirmation of the top candidates. A Chinese-reading case in the Latent Space piece reports that adding whole-article
  context "landed inside the noise floor" and cost 39 % more tokens — judge-scored loops need the confidence gate more than code loops do.
- **Research questions / debugging / docs** [fact: titles only]: "Karpathy's autoresearch applied to debugging – two open-source skills" (HN
  2026-03-16); genealogy research with Claude Code + autoresearch (fulghum.io, 2026-03-20); "Autoresearch for refactoring code" (taoofmac,
  2026-03-30); "The loop that improves almost anything" (mentalfaculty, 2026-06-08).


## 9. Design implications for a general harness

All items are [inference] from the facts in §1–§8, each with its anchor.

1. **Separate the loop driver from the model.** Every successful product moved "keep going" out of the model's judgement into the harness: Karpathy's
  NEVER STOP prose was the weak form; pi-autoresearch's idle re-prompt, Claude Code's `/goal` with a *separate* small evaluator model, Codex `/goal`
  are the strong form (§3, §6 #3). theuth already has schedules with `until`/`max_runs` and subagents; an autoresearch mode should be a loop the
  engine drives per iteration (fresh or compacted context each iteration), not a prompt that hopes the model continues.
2. **Make the contract four files and nothing else.** Goal/prompt (human-editable, re-read each iteration), measure script (one command, one number,
  direction), checks script (pass/fail after a passing benchmark; failure = revert, logged as `checks_failed`), append-only JSONL log with commit,
  metric, status, description, confidence (§1, §2). Keep the log untracked or on a side branch; git commit = keep, reset = discard (§1).
3. **Ship the noise gate.** MAD-based confidence with three bands, advisory, persisted per row, plus an automatic "re-run to confirm" for yellow and a
  configurable N-run baseline (§2, §6 #2). For judge-scored or human-scored targets, require a hold-out set and sub-scores (§8).
4. **Stop rules are part of the contract.** `maxIterations`, consecutive-discard stop, spend cap, "improvement below threshold for K runs" (Cameron,
  SkyPilot phase 5); and the mirror rule "never ask the human mid-loop" (§6 #3, #7).
5. **State lives in the repo, context is disposable.** Redirect benchmark output to files; grep one line; compact or clear between iterations at a
  threshold (bn-l `compactAtPercent` 70, Elliot's per-iteration clear); rehydrate from prompt.md + log tail + ideas + git log (§6 #4). theuth's
  `runtime: keep` and knowledge_put are the analogues, but the files in the repo are what a human and an external harness can read.
6. **Ideas backlog and failure memory are first-class.** A written "what I tried and why it failed" file (Gumroad, T-Systems learning skill, rekursiv
  lineage) is the single most-cited win against repetition (§6 #9); the harness should require a `description` and a `learned`/`next_focus` field on
  every log row (pi-autoresearch's `asi.*`).
7. **Parallelism changes the algorithm, not just the speed.** Waves of 10–13 variants, factorial grids, a cheap screening tier and an expensive
  confirmation tier (SkyPilot, §4). theuth's subagents + sandbox VMs map onto "one worktree per candidate, one wave per turn, promote top-k"; a beam
  of 3–5 candidate families instead of one best (sankalp) should be a mode.
8. **Sandbox by construction.** Dedicated branch/worktree, allow-list of editable files, no network for the benchmark container, measure/checks/hooks
  reviewed as code (§6 #8). theuth's sandbox VMs and `tools:` caps fit; the editable-file allow-list is the missing primitive.
9. **Steering surface = files + side-channel, never the main thread.** Human edits goal/ideas files; a `/btw`-style side thread answers "what are you
  doing, what's next" without pausing (§7). In theuth this is the parent↔child message channel plus an "ideas" note the loop re-reads; Slack/Telegram
  get the dot-chart and the staircase of bests, not the transcript.
10. **Finalize is a review step, not a merge.** Regroup kept commits into disjoint changesets with metric deltas, propose, then branches/PRs (§7);
  keep the human gate because the top of the ranking is where gaming hides (Shopify, Prolific, Stet).
11. **The dashboard idiom is fixed: a table of runs + a dots-and-staircase chart + the current run's elapsed time** (§7). One `show` card per
  iteration wave, a final report with the log embedded.
12. **Cost model: tokens are ~half the per-step cost at a 5-minute eval and dominate for cheap evals** (Weco). Choose the model per phase: a frontier
  advisor for idea generation, a cheaper executor for the edit-run-log cycle (Claude `/advisor`, sankalp's `claude -p`).
13. **Hooks are the extension point.** Before/after scripts receiving the last run and session JSON on stdin and returning ≤8 KB of steer text let
  users add literature search, notifications, anti-thrash and idea rotation without touching the loop (§2).


## 10. Numbers table

| Quantity | Value | Source (date) |
|---|---|---|
| Karpathy loop cadence | ~12 experiments/h, ~100 overnight; 5-min budget; kill at 10 min | README/program.md (2026-03) |
| Karpathy run 1 | 89 experiments, ~20 improvements, −11 % time-to-GPT-2 | Weco (2026-04-02), SkyPilot (2026-03-18) |
| SkyPilot parallel | 16 GPUs, ~910 submitted / ~700 valid, ~90/h, 8 h, 1.003→0.974, 9× faster than serial, <$300 ($9 tokens) | SkyPilot (2026-03-18) |
| H200 vs H100 effect | ~9 % more steps, ~0.005 val_bpb lower, same config | SkyPilot (2026-03-18) |
| Weco per-step cost | agent $0.84 vs Optuna $0.44 (tokens ≈ $0.40); 78 % of gains inside Optuna's space; 70 % in-space in first 50 steps | Weco (2026-04-02) |
| Weco kept counts | 15/16/12 kept per run (3 seeds) | Weco (2026-04-02) |
| eCLIP | 42 runs, 13 kept, 29 reverted, −54 % mean rank, one bug fix −113 of −187 | ykumar.me (2026-03-22) |
| Polaris build | 19.1 s baseline, −65 % | Shopify (2026-04-15) |
| Liquid | −53 % parse+render, −61 % allocations | Tobi via Shopify (2026-03-12) |
| pi-autoresearch adoption | 3,600+ stars, 200+ forks (as of 2026-04-15) | Shopify (2026-04-15) |
| Gumroad | 206 commits, 94 CI runs, 13 PRs, 13 flaky tests, 1 week | gianfrancopiana (2026-03-25) |
| Cameron | 49 runs, 20 kept, 339→34 ms p95, ~1 h, $24, maxIterations 50 | cameronwestland (2026-03-26) |
| PostHog | −22 % best, −37 % trimmed mean, −62 % granules | posthog.com (2026-06) |
| sankalp | 1,500+ submissions, 14 days, 419,000→1,805 µs (232×), 12th/183, 560 variants, 68 docs | sankalp (2026-07-08) |
| Elliot | ~$4 per loop, 300 s cap, 150 MB largest file | elliotcsmith (2026-07-01) |
| Hashimoto | 88 ms→2 ms, 150K→500 allocs, "wrong" | tweet 2026-05-28 via Elliot |
| T-Systems | 26 configs, stable after ~15, +110 % throughput, +40 % interactivity; humans then 18× TTFT | fparisio (2026-07) |
| Prolific | 50 × 10-min runs, 300 judges, ~52 % vs 66 %/60 % | ProlificAI (2026-05-21) |
| Stet | 8 iterations, n=5 train, n=10 holdout, +0.30 / −0.20 | stet.sh (2026-05-27) |
| rekursiv | 6,164 experiments in 3 days, 2-h waves, 10 seeds | rekursiv (2026-09) |
| SoL-Pi | $8.75–$13.50/h saved vs Claude Code harnesses | nvlabs SoL-Pi (2026-09) |
| pi-autoresearch confidence bands | ≥2.0× green, 1.0–2.0× yellow, <1.0× red; after 3 runs; checks timeout 300 s; hook steer ≤8 KB, 30 s | README (2026-10) |
| Claude Code /goal | 30-min first check-in, doubling; evaluator = small fast model | docs (2026-10) |
| bn-l Claude mod | compactAtPercent default 70 | GitHub (2026-10-01) |
| HN interest | 457 pts (sankalp), 428 (eCLIP), 237 (SkyPilot), 208 (repo), 167 (agent-sat) | hn.algolia.com (2026-10-08) |


## 11. Sources

1. https://github.com/karpathy/autoresearch — README + program.md (raw master, read 2026-10-08; published March 2026). The original contract.
2. https://news.ycombinator.com/item?id=47291123 — HN thread for the repo, 2026-03-07, 208 pts.
3. https://blog.skypilot.co/scaling-autoresearch/ — SkyPilot, 2026-03-18; 16-GPU parallel run, numbers, cost, emergent tiering.
4. https://github.com/davebcn87/pi-autoresearch — README (raw main, read 2026-10-08); file contract, confidence, hooks, finalize, config.
5. https://shopify.engineering/autoresearch — David Cortés, 2026-04-15; origin, Tobi's PR, Shopify wins, Tobi's Liquid tweet quote.
6. https://www.weco.ai/blog/autoresearch-vs-classical-hpo — Weco AI, 2026-04-02; Optuna head-to-head, per-step cost, in-space share.
7. https://ykumar.me/blog/eclip-autoresearch/ — Yogesh Kumar, 2026-03-22; 42 runs, sandboxing, phases, failure at moonshots.
8. https://sankalp.bearblog.dev/autoresearch/ — sankalp, 2026-07-08; Codex /goal, /btw, beam, advisor, 232×.
9. https://www.elliotcsmith.com/autoresearch-claude-and-constrained-optimization/ — Elliot Smith, 2026-07-01; constraints, $4/loop, Hashimoto quote.
10. https://gianfrancopiana.com/blog/autoresearch-flaky-tests — Gianfranco Piana, 2026-03-25; OpenClaw port, Gumroad numbers.
11. https://cameronwestland.com/autoresearch-is-reward-function-design/ — Cameron Westland, 2026-03-26; 49 runs, $24, checks_failed, stopping rule.
12. https://posthog.com/blog/autoresearch-query-bug — PostHog, HN 2026-06-01; campaign/lane contract, ClickHouse numbers.
13. https://fparisio.substack.com/p/an-ai-agent-cold-tuned-our-glm-52 — T-Systems, HN 2026-07-14; 26 configs, learning skill, production break.
14. https://rekursiv.ai/blog/autoautoresearch/ — rekursiv.ai, HN 2026-09-15; 6,164 experiments, waves, lineage graph.
15. https://huggingface.co/blog/ProlificAI/autoresearch-hitl-experiment — Prolific, HN 2026-05-21; 50 runs vs 300 human judges.
16. https://www.stet.sh/blog/how-i-used-codex-to-improve-its-own-agents-md — Stet, HN 2026-05-27; AGENTS.md iteration with holdout.
17. https://tylerschultz.me/auto-research/ — Tyler Schultz, 2026-05-01; prompt/skill loop, +110 % F1.
18. https://code.claude.com/docs/en/goal — Claude Code docs "Keep Claude working toward a goal", read 2026-10-08 (indexed 2026-10-02).
19. https://github.com/bn-l/claude-autoresearch-mod — bn-l, HN 2026-10-01; Claude Code port, compactAtPercent.
20. https://nvlabs.github.io/SoL-Pi/ — NVIDIA SoL-Pi, 2026-09 (arXiv 2609.20519); harness-efficiency autoresearch.
21. https://community.computer — Community Computer, HN 2026-04-17; P2P signed autoresearch experiments over Radicle.
22. https://www.latent.space/p/autoresearch-introspection — Latent Space, 2026-07-06; production loops interview.
23. https://quanttype.net/p/speed-up-code-with-pi-autoresearch/ — quanttype, 2026-03-24; early pi-autoresearch UX.
24. https://hn.algolia.com/api/v1/search?query=autoresearch — HN Algolia index, queried 2026-10-08; titles, dates, points for the long tail
  (agent-sat, autokernel, Autoresearch@home, Autoresearch Hub, ZeroThesis, Openresearch, awesome-autoresearch, lazy-developer, autoexp,
  autoresearch-rl, Autoretrieval, AutoResearchExam, AutoLab, Paper Lantern, Paradigma, autobrowse).
25. GitHub search via search_web (category it, 2026-10-08): uditgoenka/autoresearch (2026-08-14), leo-lilinxiao/codex-autoresearch (listed skillsllm
  2026-03-30), Orchestra-Research/AI-Research-SKILLs, zircote-plugins/autoresearch, coder-company/agent-autoresearch, LeoStehlik/autoresearch-loop,
  lendtrain/autoresearch-for-skills, MustafaHasnainCF/skill-improver, iagogfe/autosearch-hitl, arbazkhan971/godmode — existence only.
26. Unreadable (HTTP 451 via xcancel.com, cited through secondary sources): x.com/karpathy/status/2031135152349524125 (results), /2030705271627284816
  (SETI@home), x.com/tobi/status/2032212531846971413, x.com/mitchellh/status/2060088112257372610. Empty-content pages: ensue-network.ai/autoresearch,
  browserbase.com/blog/autobrowse, pinggy.io; 404: WecoAI/awesome-autoresearch raw README, developers.openai.com/codex/goals.


## 12. Citations checked

Re-opened on 2026-10-08 (read_url, full page; needles searched verbatim). 16 checked, 16 ok, 0 wrong.

| # | Claim | Source | Result |
|---|---|---|---|
| 1 | program.md: "NEVER STOP", kill at >10 min, results.tsv, val_bpb | raw.githubusercontent.com/karpathy/autoresearch/master/program.md | ok |
| 2 | README: ~12 experiments/hour, fixed 5-minute budget, autoresearch-mlx fork | raw …/karpathy/autoresearch/master/README.md | ok |
| 3 | pi-autoresearch: MAD confidence, `checks_failed`, 8 KB hook steer, `maxIterations`, finalize | raw …/davebcn87/pi-autoresearch/main/README.md | ok |
| 4 | SkyPilot: ~910 experiments, 1.003→0.974, "about $9", 9×, "~20 improvements" | blog.skypilot.co/scaling-autoresearch/ | ok |
| 5 | Shopify: 65 % faster, 32-commit PR, 3,600 stars, "300 times", Tobi 53 % | shopify.engineering/autoresearch | ok |
| 6 | Weco: $0.84 vs $0.44, ~78 % in-space, Karpathy 89 experiments | weco.ai/blog/autoresearch-vs-classical-hpo | ok |
| 7 | Kumar: 42 experiments, 157.43, "ended the conversation" | ykumar.me/blog/eclip-autoresearch/ | ok |
| 8 | sankalp: 232×, 12th of 183, 1500 submissions | sankalp.bearblog.dev/autoresearch/ | ok |
| 9 | Elliot: "$4 in tokens", Hashimoto "88ms to 2ms", Sonnet 4.6 | elliotcsmith.com/autoresearch-claude-and-constrained-optimization/ | ok |
| 10 | Gumroad: 206 commits, 94 CI runs, 13 merged PRs | gianfrancopiana.com/blog/autoresearch-flaky-tests | ok |
| 11 | Cameron: 49 optimizations, kept 20, 339→34 ms, $24, "5 consecutive discards" | cameronwestland.com/autoresearch-is-reward-function-design/ | ok |
| 12 | PostHog: −22 %, −37 %, −62 %, lanes | posthog.com/blog/autoresearch-query-bug | ok |
| 13 | T-Systems: 26 configs, +110 %, 153→320 tok/s/GPU | fparisio.substack.com/p/an-ai-agent-cold-tuned-our-glm-52 | ok |
| 14 | Prolific: 50 experiments, ~52 %, 66 % | huggingface.co/blog/ProlificAI/autoresearch-hitl-experiment | ok |
| 15 | Claude Code /goal: 30-minute check-in, Impossible verdict, Stop hook, ANTHROPIC_DEFAULT_HAIKU_MODEL | code.claude.com/docs/en/goal | ok |
| 16 | rekursiv: 6,164 experiments, 2-hour waves | rekursiv.ai/blog/autoautoresearch/ | ok |

Not verifiable (stated as such in the text): the four X/Twitter posts (HTTP 451 via xcancel; quoted through SkyPilot, Shopify, Elliot and HN titles);
the existence-only GitHub plugin list; Ensue/Browserbase pages (empty content); AlphaEvolve/OpenEvolve hosted offerings and
OpenCode/Cursor/Amp/Factory/Gemini CLI/Aider/Goose/Devin/Jules autoresearch support (not researched in this pass).

