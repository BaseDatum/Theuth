# Recon: oh-my-pi autoresearch (recon-omp-ar, 2026-10-08)

Sources: local clone ~/Develop/oh-my-pi @ c5a8e0e092 (2026-09-15, MIT); upstream HEAD sparse clone in /tmp/ar-recon-omp/omp-head @ 153b37c3 (2026-10-08 04:33 +02:00, omp 18.8.4). Citations: `c5a8e0e:path:line` / `HEAD:path:line`, paths repo-relative. Each claim [fact] or [inference].

_Status: complete 03:53 (started 03:38). All 19 axes filled; §19 lists the re-checked citations._

## 1. Entry & lifecycle

**Entry points** [fact]
- Slash command `/autoresearch [goal | off | clear [--keep-tree|--reset-tree]]` registered by the extension (`HEAD:packages/coding-agent/src/autoresearch/index.ts:125-225`). Bare `/autoresearch` while mode is on = toggle off (`index.ts:146-153`); `off` = off (`:155-162`); `clear` = reset tree + close session (`:164-170`, `handleClear` `:418-461`); any other text is a goal (`:172`).
- Starting: `ensureAutoresearchBranch` runs first (`index.ts:173`; `git.ts:36-98`): pure-jj rejected, no-git → warning and continue without isolation, already on `autoresearch/*` → reuse, dirty tree → **hard error** ("Commit or stash"), else create `autoresearch/<slug>-<yyyymmdd>[-n]` (`git.ts:186-212`).
- If an open session exists for that branch, the command resumes it: refreshes goal/branch in SQLite, turns mode on, re-activates the four tools and sends a *user* message rendered from `command-resume.md` (`index.ts:195-214`). Otherwise it turns mode on and sends the goal text as the user message, or a notify "describe what to optimize in your next message" when no goal was given (`index.ts:216-223`).
- Mode is persisted as a session-log custom entry `autoresearch-control {mode: on|off|clear, goal?}` (`index.ts:117`; replayed by `reconstructControlState`, `state.ts:181-197`; documented as reserved in `HEAD:docs/session.md:330`).
- Shortcuts: `ctrl+x` toggles the inline dashboard, `ctrl+shift+x` opens the overlay (`index.ts:227-245`).
- No CLI flag, no settings key, no `--autoresearch`; the extension is loaded unconditionally as an inline extension (`HEAD:packages/coding-agent/src/sdk.ts:2601`, comment `:3269`). [fact]

**Two phases, selected by whether a SQLite session exists** [fact]
- Phase 1 (setup): `before_agent_start` finds no session → injects `prompt-setup.md` ("build the benchmark harness, not optimise"; MUST write `./autoresearch.sh`, exit 0, print `METRIC <name>=<value>`; deterministic) (`index.ts:356-376`; `prompt-setup.md:3-43`). The only tool that works in Phase 1 is `init_experiment`; the other three answer "no active autoresearch session" (`run-experiment.ts:75-84`, `log-experiment.ts:57-66`, `update-notes.ts:32-41`).
- Phase 2 (loop): `init_experiment` requires `autoresearch.sh` to exist (`init-experiment.ts:69-81`), auto-commits all pending changes as the harness baseline when on an autoresearch branch (`:85-97`), records `baseline_commit = HEAD` and opens the session (`:99-122`). From then on `before_agent_start` injects `prompt.md` with notes, snapshot, recent runs, pending run (`index.ts:377-415`).

**Max iterations** [fact]: `max_iterations` is a per-segment *soft* cap set through `init_experiment` (`init-experiment.ts:31,58-61`; default none). Enforced in `log_experiment`: when logged runs in the current segment ≥ cap, mode is switched off, an `autoresearch-control off` entry is appended and the tools are deactivated (`log-experiment.ts:233-243`); the tool text says "Maximum experiments reached (N). Autoresearch mode is now off." (`:458-463`). No global cap, no wall-clock or cost cap. [fact]

**What enforces "keep iterating"** [fact] — three layers:
1. Prompt: "You MUST keep iterating until the user interrupts you or the configured maximum iteration count is reached" (`prompt.md:20`) and guardrail line 103 about finishing the run/log cycle before addressing a new user message.
2. Harness auto-continue on `agent_end` (`index.ts:256-291`): when mode is on and no user messages are queued (`ctx.hasPendingMessages()` → disarm, `:262-265`), and either a *completed-but-unlogged* run exists or `autoResumeArmed` is true, it sends a hidden message (`customType: "autoresearch-resume"`, `display: false`, `attribution: "agent"`, `deliverAs: "nextTurn", triggerTurn: true`) rendered from `resume-message.md` ("Continue the autoresearch loop now…") (`index.ts:280-290`). `autoResumeArmed` is set true by `init_experiment` (`init-experiment.ts:153`), `run_experiment` (`run-experiment.ts:216`) and `log_experiment` (`log-experiment.ts:212`), and cleared by `setMode`/rehydrate (`index.ts:53,114`). So a turn that ended without any autoresearch tool call is NOT re-prompted — the model can stop by simply not calling the tools [inference from the arming sites].
3. `lastAutoResumePendingRunNumber` dedupes the pending-run nudge so the same unlogged run does not re-trigger forever (`index.ts:273-279`).

**User message mid-loop** [fact]: nothing pre-empts a running `run_experiment`; `agent_end` sees `hasPendingMessages()` and skips the auto-resume, so the user's message is answered in the next turn and the loop resumes only when the model calls a tool again (which re-arms). The prompt tells the model to finish the run+log cycle first (`prompt.md:103`).

**Branch switch mid-loop** [fact]: `before_agent_start` re-reads the branch each turn; if it differs from `session.branch`, mode is dropped silently for that turn, tools detached, prompt not injected, but the DB session is preserved so switching back resumes (`index.ts:64-69, 293-311`).

**Compaction interplay** [fact]: a manual compaction aborts the live turn; `session-maintenance.ts` resumes the interrupted turn so the agent does not "sit idle on a half-finished loop (an autoresearch run, a pending tool result)" (`HEAD:packages/coding-agent/src/session/session-maintenance.ts:1134-1150`; test `agent-session-auto-compaction-queue.test.ts:399-446`).

**Stopping** [fact]: user interrupt (abort) → `agent_end` fires, `runningExperiment` cleared (`index.ts:258`); `/autoresearch off` deactivates tools but keeps the DB session open; `/autoresearch clear` hard-resets to `baseline_commit` + `git clean` when on an autoresearch branch (or `--reset-tree`), removes legacy pi-autoresearch files, closes the session (`index.ts:418-461`, legacy list `:464-473`).

## 2. Data model

Types live in `HEAD:packages/tui/src/tools/autoresearch.ts` (moved from `c5a8e0e:packages/coding-agent/src/autoresearch/types.ts`); runtime-only types in `HEAD:…/autoresearch/types.ts`. [fact]

- **Session** (SQLite `sessions`, `storage.ts:199-218`): id, name, goal, primary_metric, metric_unit, direction (`lower|higher`, default lower), preferred_command (always `bash autoresearch.sh`), branch, baseline_commit, current_segment (0-based), max_iterations, scope_paths_json, off_limits_json, constraints_json, secondary_metrics_json, notes (one markdown blob — the playbook + `## Ideas` section), created_at, closed_at. One open session per branch (`getActiveSessionForBranch`, `:295-310`).
- **Run** (SQLite `runs`, `storage.ts:220-249`): id (= the run number shown to the model), session_id, segment, command, started_at, completed_at, duration_ms, exit_code, timed_out, parsed_primary, parsed_metrics_json, parsed_asi_json, pre_run_dirty_paths_json, log_path, then the logged half: status (`keep|discard|crash|checks_failed`, NULL = pending), description, metric, metrics_json, asi_json, commit_hash, confidence, modified_paths_json, scope_deviations_json, justification, flagged, flagged_reason, logged_at, abandoned_at.
- **Segment**: an integer on the session and on each run; bumped by `init_experiment new_segment: true` (`init-experiment.ts:137-144`, `storage.ts:414-419`). All baseline/best/confidence math filters to the current segment (`autoresearch-data.ts:20-22`). Semantics: "I changed `autoresearch.sh`, old numbers are incomparable" (`prompt.md:14,30`).
- **Metrics**: primary metric is a name + unit + direction; secondary metrics are names, unit inferred from suffix (`_ms`, `µs`, `_s`, `kb`, `mb`; `helpers.ts:157-164`), auto-registered when a logged run carries unknown metric names (`state.ts:215-223`).
- **ASI** ("free-form structured metadata"): `ASIData = {[key]: string|number|boolean|null|array|object}` (`tools/autoresearch.ts:20-25`). Parsed from `ASI key=value` lines (values JSON-parsed when possible, `helpers.ts:117-`), merged with the `asi` tool parameter (`helpers.ts:mergeAsi`, `log-experiment.ts:169`), sanitised against `__proto__/constructor/prototype` (`helpers.ts:9,204-239`). Prompt suggests keys `hypothesis`, `rollback_reason`, `next_action_hint` (`prompt.md:38`); those three are what the per-run summary line shows (`index.ts:509-515`, 220 chars).
- **Notes / ideas**: a single text column; `update_notes body` replaces it, `append_idea` inserts a bullet under `## Ideas` (`update-notes.ts:43-48,73-95`).
- **In-memory runtime** per chat session (`types.ts:28-41`): autoresearchMode, autoResumeArmed, dashboardExpanded, lastAutoResumePendingRunNumber, lastRun{Duration,Asi,ArtifactDir,Number,Summary}, runningExperiment, state (ExperimentState view), goal. Rebuilt from DB on every session start/switch/branch (`index.ts:49-104`).
- **ExperimentState** (view model, `tools/autoresearch.ts:58-77`): results[], bestMetric (= *baseline* metric, see §6), bestDirection, metricName/Unit, secondaryMetrics, name, goal, currentSegment, maxExperiments, confidence, scopePaths, offLimits, constraints, notes, branch, baselineCommit, sessionId. Built by `buildExperimentState` (`state.ts:133-179`).

**State machine (transitions)** [fact unless marked]
- (no session, mode off) —`/autoresearch [goal]`→ (no session, mode on, Phase-1 prompt) `index.ts:216-223`
- (no session, mode on) —`init_experiment` with `autoresearch.sh` present→ harness commit (if on branch & dirty) → session open, segment 0, baseline_commit=HEAD, Phase-2 prompt `init-experiment.ts:69-122`
- (session, no pending run) —`run_experiment`→ run row inserted (status NULL) → process runs → `markRunCompleted` → **pending run** (completed, unlogged) `run-experiment.ts:101-178`
- (session, pending run) —`run_experiment` again→ previous pending run `abandoned_at` set, new run starts `run-experiment.ts:88-93`
- (session, pending run) —`log_experiment keep`→ stage+commit modified files (on branch) → run logged → confidence recomputed `log-experiment.ts:114-133,178-202`
- (session, pending run) —`log_experiment discard|crash|checks_failed`→ `git reset --hard HEAD && git clean` (on branch) or restore/rm of run-modified paths (off branch) → run logged `log-experiment.ts:149-161,314-353`
- (session) —segment count ≥ max_iterations after a log→ mode off, tools detached, session stays open `log-experiment.ts:233-243`
- (session) —`init_experiment new_segment:true`→ pending runs abandoned, baseline_commit=HEAD, segment+1 `init-experiment.ts:124-146`
- (session) —`init_experiment` without new_segment→ reconfigure goal/scope/metric in place (pending runs abandoned) `init-experiment.ts:124-136`
- (session, mode on) —`/autoresearch off`→ mode off, session open (resumable) `index.ts:155-162`
- (session) —`/autoresearch clear`→ reset to baseline_commit (on branch), `closed_at` set, mode "clear", goal nulled `index.ts:418-461`
- (session, mode off) —`/autoresearch`→ resume (command-resume message) `index.ts:195-214`
- (any) —branch ≠ session.branch→ effective mode off for the turn, nothing persisted `index.ts:293-311`
- (any) —agent_end with armed/pending and no queued user msg→ hidden "Continue" message, next turn `index.ts:256-291`

## 3. Storage & resume

**Where things are written** [fact]
- SQLite, one DB per project: `~/.omp/autoresearch/<encoded-repo-root>.db` (`HEAD:packages/utils/src/dirs.ts:878-891`; `storage.ts:574-588`), WAL + `synchronous=NORMAL` + foreign keys (`storage.ts:195-197`). `OMP_AUTORESEARCH_DB_DIR` overrides the root (`storage.ts:575`; `HEAD:docs/environment-variables.md:542`). The project key is the git repo root (or cwd when not a repo), encoded (`storage.ts:576-577`).
- Run artifacts: `~/.omp/autoresearch/<encoded>/runs/<0001>/benchmark.log` — full stdout+stderr of each benchmark, streamed to disk chunk by chunk (`run-experiment.ts:110-113, 296-312`; `dirs.ts:893-896`). The run row stores `log_path`.
- Nothing autoresearch-specific is written into the repo except what the *model* writes: `autoresearch.sh` and whatever harness files it creates (`prompt-setup.md:23-30`). The pi-autoresearch-era files (`autoresearch.md`, `.autoresearch/`, `autoresearch.jsonl`, …) are explicitly forbidden (`prompt.md:14`, `prompt-setup.md:43`) and deleted on `clear` (`index.ts:464-487`).
- Git holds: the harness baseline commit (`autoresearch: harness setup` / `Benchmark entrypoint: bash autoresearch.sh` / `Goal: …`, `init-experiment.ts:246-254`) and one commit per kept run (`<description>\n\nResult: {"status":"keep","<metric>":v,…}`, `log-experiment.ts:298-305`). Notes, ideas, run metrics, ASI, confidence, deviations and flags live only in SQLite — a clone of the branch loses them [inference].
- The chat session log holds only `autoresearch-control` entries (mode/goal) (`index.ts:117`, `docs/session.md:330`).

**Crash / restart survival** [fact]
- A run is inserted *before* the process starts (status NULL, `completed_at` NULL); `markRunCompleted` fills exit/metrics after (`run-experiment.ts:101-178`). A run killed by a process crash therefore stays as a never-completed row; `pendingRunSummaryFromRow` ignores rows with `completedAt === null` (`index.ts:489-492`), so it is neither resumed nor logged — it is abandoned implicitly on the next `run_experiment` (`run-experiment.ts:88-93`) or `init_experiment` (`init-experiment.ts:124`). Git state of that run (uncommitted edits) remains in the tree [inference].
- A completed-but-unlogged run survives: on resume the DB row becomes the "Pending run" block of the system prompt (`index.ts:316-320, 404-412`; `prompt.md:85-97`) and the agent_end hook nudges once (`index.ts:273-279`).
- Storage handles are cached per DB path (`storage.ts:552-572`) and closed at shutdown (`closeAllAutoresearchStorages`, `:590-602`).

**Resume path end to end** [fact]
1. `session_start`/`session_switch`/`session_branch`/`session_tree` → `rehydrate` (`index.ts:247-250, 49-104`): replay `autoresearch-control` entries from the chat log → `lastMode`, `goal`, `autoresearchMode`. If autoresearch was *never* activated in this chat the DB is not even opened (avoids creating a file per project, `:56-62`).
2. Read current branch; open DB if it exists; `getActiveSessionForBranch(branch)`; mode is effective only if `session.branch === currentBranch` (`:64-69`).
3. `buildExperimentState(session, listLoggedRuns)` → runtime.state; `getPendingRun` → `lastRunSummary` (`:71-89`); widget refreshed; four tools (de)activated to match mode (`:93-103`).
4. First turn: `before_agent_start` re-checks branch, rebuilds state, injects `prompt.md` with notes + snapshot + pending run (`:293-416`).
5. `/autoresearch` typed on a branch with an open session: updates goal/branch, sets mode on, sends the `command-resume.md` user message ("Use the active session context above as the source of truth…, Inspect recent git history…, Keep iterating") (`index.ts:195-214`; `command-resume.md:1-14`).
6. After each turn, `agent_end` arms the hidden `resume-message.md` continuation while a tool was used in that turn (§1).

## 4. Benchmark contract

- **Entrypoint is fixed**: `bash autoresearch.sh` in the working directory (`HARNESS_FILENAME`, `DEFAULT_HARNESS_COMMAND`, `HEAD:packages/tui/src/tools/autoresearch.ts:149-152`); `run_experiment` has no `command` parameter — only `timeout_seconds?` (`run-experiment.ts:36-38`). [fact] (The tool description still says "Run any benchmark command", `run-experiment.ts:68`, a leftover.) On Windows the exec line uses the configured POSIX shell (Git Bash) instead of `bash` on PATH to avoid WSL (`run-experiment.ts:254-259`, CHANGELOG 18.5.0 `HEAD:packages/coding-agent/CHANGELOG.md:414`).
- **Required behaviour** (`prompt-setup.md:23-28`): exit 0 on success, non-zero on failure; one line `METRIC <name>=<value>` for the primary metric; extra `METRIC` lines for secondaries; deterministic (no network, fixed seeds). Compile-only checks are forbidden as benchmarks (`prompt-setup.md:42`).
- **Line grammar** [fact] (`helpers.ts:10-11`): metric `^METRIC\s+([\w.µ-]+)=(\S+)\s*$` — name may contain letters, digits, `_`, `.`, `µ`, `-`; value must parse with `Number()` to a finite number or it is dropped (`helpers.ts:76-78`). ASI `^ASI\s+([\w.-]+)=(.+)\s*$` — value tried as JSON, else string (`helpers.ts:117-`). Later lines overwrite earlier ones with the same name (Map set). `__proto__`, `constructor`, `prototype` keys are refused (`helpers.ts:9`).
- **Streaming scanner**: since HEAD (commit 9895b045, 2026-10-07) output is scanned chunk-wise with O(line) memory (`ExperimentOutputScanner`, `helpers.ts:27-84`) instead of re-reading the whole log (`c5a8e0e:…/run-experiment.ts` read `fs.promises.readFile(opts.logPath)`; diff hunk `@@ -321,13 +322,23 @@`). [fact]
- **Primary metric selection**: `parsedPrimary = metrics.get(session.primaryMetric)` (`run-experiment.ts:165`); everything else is a secondary (`buildRunText`, `:372-381`).
- **Secondary metrics**: parsed ones are merged with the `metrics` parameter of `log_experiment`, tool parameter wins (`log-experiment.ts:379-393`).
- **Checks / validation**: there is no checks script and no validation hook; `checks_failed` is a status the *model* chooses after running its own validation "through the regular `bash` tool" (`prompt.md:37`). [fact] (pi-autoresearch had `autoresearch.checks.sh`; it is on the legacy delete list, `index.ts:467`.)
- **Timeout**: default 600 s; `timeout_seconds` floors to ms; `0` means effectively none (2 147 000 000 ms) (`run-experiment.ts:129, 307`). A timeout surfaces as `killed`/`timedOut` and `TIMEOUT after Ns` (`:189-190, 358-359`). Abort signal from the UI → throws "aborted" (`:321-323`).
- **Output capture & truncation**: everything to `benchmark.log`; the model sees a tail capped at `EXPERIMENT_MAX_LINES = 10` / `EXPERIMENT_MAX_BYTES = 4 KiB` (`helpers.ts:6-7`; `run-experiment.ts:332-335`) plus "Output truncated (4 KiB limit). Full output: <path>" (`:387-391`); the TUI shows a larger tail (`DEFAULT_MAX_BYTES/LINES` from streaming-output). Progress snapshots every 1 s with elapsed + tail (`:290-294`).
- **Exit codes**: `passed = exitCode === 0 && !killed` (`run-experiment.ts:180`); `crashed = exitCode !== 0 || killed` (`:189`). The tool does not refuse anything on failure — the model decides `crash` vs `discard`.
- **Environment**: the process is run by the normal `executeBash` with `cwd = ctx.cwd`, `sessionKey = autoresearch:<cwd>`, `chunkThrottleMs: 0`; no autoresearch-specific environment variables are injected (`run-experiment.ts:304-319`). [fact]
- **Returned text to the model** (`buildRunText`, `:355-394`): run number + directory; PASSED/FAILED/TIMEOUT with seconds; `Current baseline <metric>: …`; `Parsed <metric>: v` + `Next log_experiment metric: v`; secondaries + `Next log_experiment metrics: {…}`; `Parsed ASI keys`; blank line; 10-line tail; truncation note. A header "Note: abandoned prior pending run #N" when applicable (`:227-231`).

## 5. Tools

All four are `defaultInactive: true` and are (de)activated as a set by mode (`EXPERIMENT_TOOL_NAMES`, `index.ts:30, 93-103`). Schemas use omptype (`type({...})`). [fact]

**`init_experiment`** (`init-experiment.ts:21-33`)
- Params: `name` (string, required), `goal?`, `primary_metric` (required), `metric_unit?`, `direction?` (`'lower'|'higher'`, default lower), `secondary_metrics?` (string[]), `scope_paths?`, `off_limits?`, `constraints?`, `max_iterations?` (number → floored, ≤0/NaN → none), `new_segment?` (bool).
- Validation: path specs normalised (`\`→`/`, strip `./`, trailing `/`) and deduped (`helpers.ts:166-190`); when no session exists or `new_segment` is set, `./autoresearch.sh` must exist or the tool errors with a Phase-1 reminder (`:69-81`).
- Side effects: on an `autoresearch/*` branch with a dirty tree → `git add -A` + commit `autoresearch: harness setup` (`:85-97`); baseline_commit = HEAD (`:99`); open session or update + optional segment bump; **abandons pending runs** on re-init (`:124`); runtime state rebuilt; mode on; auto-resume armed (`:148-161`).
- Returns: lines like `Started session #1: <name>` / `Bumped segment to N…` / `Updated session…`, `Metric: <name> (<unit|unitless>, lower is better)`, `Benchmark entrypoint: bash autoresearch.sh`, scope/off-limits/max-iterations/branch/baseline lines, "Phase 2: iteration loop is active. Run the baseline experiment with `run_experiment` and log it." or "Run a fresh baseline for the new segment.", off-branch warning (`:163-210`).

**`run_experiment`** (`run-experiment.ts:36-38`)
- Params: `timeout_seconds?` (default 600). Nothing else — command is fixed.
- Validation: session for current branch must exist (`:72-84`).
- Side effects: abandons any pending run (`:88-93`); records pre-run dirty paths (`:96-98`); inserts run row, creates `runs/NNNN/benchmark.log`; runs the process with 1 s progress updates; `markRunCompleted`; sets `lastRunSummary`, arms auto-resume (`:203-217`); refreshes state and widget.
- Returns: see §4 "Returned text".

**`log_experiment`** (`log-experiment.ts:26-40`)
- Params: `metric` (number, required), `status` (`'keep'|'discard'|'crash'|'checks_failed'`), `description` (string), `metrics?` ({[string]: number}), `asi?` ({[string]: unknown}), `commit?` (override hash), `justification?`, `flag_runs?` ([{run_id: integer, reason}]).
- Validation: session + pending run must exist (`:57-72`); `flag_runs` targets must belong to this session, others silently skipped (`:77-82`); a logged `metric` that differs from the parsed primary is accepted with a warning "Both values stored" (`:171-175`).
- Side effects: flags runs; computes modified paths (on branch: all dirty paths; off branch: dirty-now minus dirty-before, `:87-103`); computes scope deviations (`:365-377`); **keep** → stage listed files, commit when there is a staged diff (`:279-312`), warnings for unjustified deviations (`:140-148`); **other statuses** → `reset --hard HEAD` + `clean` on branch, else `restore --source HEAD --staged --worktree` of tracked + `rm -rf` of untracked run files (`:314-353`); `markRunLogged`; confidence recomputed for the segment and stored on the run (`:193-202`); may switch mode off at the cap (`:233-243`); arms auto-resume.
- Returns (`buildLogText`, `:403-472`): `Logged run #N: <status> - <description>`, `Wall clock: Ns`, `Baseline <metric>: v`, `This run: v (+x.x%)`, `Secondary metrics: a: v (+x%)  b: …`, `Best kept <metric>: v` (only when ≠ baseline), `ASI: k: v | …` (120-char values), `Confidence: 2.3x noise floor (likely real|marginal|within noise)`, `Git: committed <sha12>|worktree reset to HEAD|reverted N files|nothing to commit`, `Progress: n/max runs in current segment`, `Maximum experiments reached…`, `Flagged: #id (reason)`, `Warning: …`.

**`update_notes`** (`update-notes.ts:12-15`)
- Params: `body` (string, required even when appending), `append_idea?`.
- Side effects: `append_idea` (non-blank) → bullet inserted at the end of the `## Ideas` section (created if absent) (`:73-95`); else notes replaced by `body`; state rebuilt; widget refreshed.
- Returns: `Appended idea (N chars total).` or `Notes updated (N chars).`

## 6. Decision & statistics

- **Who decides**: the model. The tools never compute keep/discard; they record what the model says and perform the git consequence (`log-experiment.ts:114-161`). The prompt rule is: keep when the primary improves, discard when it regresses or stays flat, crash when the run fails, checks_failed when validation fails (`prompt.md:33-37`). [fact]
- **Direction**: `isBetter(current, best, direction)` = `<` for lower, `>` for higher (`autoresearch-data.ts:15-17`); default `lower` (`init-experiment.ts:51`).
- **Baseline** = the *first* unflagged `keep` in the current segment (`findBaselineResult`, `autoresearch-data.ts:25-27`) — i.e. the model is expected to log the untouched baseline run as `keep`. `ExperimentState.bestMetric` is set to this baseline (`state.ts:176`), so the "Baseline" line in tool output and the dashboard is the first keep, not a running best. [fact]
- **Best kept** = optimum metric over unflagged keeps in the segment (`findBestKeptMetric`, `state.ts:80-93`; `bestKeptResult`, `index.ts:517-533`). Deltas shown are relative to the baseline, not to the previous best (`log-experiment.ts:420-423`; dashboard `autoresearch-dashboard.ts:424-427`).
- **Confidence / noise floor** (`computeConfidence`, `state.ts:105-131`) [fact, exact]:
  - sample = runs in current segment with `!flagged && metric > 0` (all statuses, including discards and crashes with a metric); need ≥ 3 else `null`;
  - `median` = sorted median of sample metrics; `mad` = median of |value − median|; `mad === 0` → `null`;
  - `baseline` = first unflagged keep's metric; `bestKept` = best unflagged keep with metric > 0; if none or `bestKept === baseline` → `null`;
  - **confidence = |bestKept − baseline| / mad**.
  - Thresholds: `≥ 2` "likely real", `≥ 1` "marginal", else "within noise" (`log-experiment.ts:452`; dashboard colours `autoresearch-dashboard.ts:522-524`). The value is stored on each logged run (`log-experiment.ts:197-202`) and on the state (`state.ts:177`).
  - [inference] This is a session-level signal (best vs baseline over the spread of *all* runs), not a per-run significance test; discards that regress widen the MAD and lower confidence; it says nothing about whether the *latest* keep is real.
- **Re-runs**: no automatic re-run. The prompt tells the model to re-run promising changes when confidence is low (`prompt.md:39`); a repeated `run_experiment` simply becomes a new pending run after abandoning the unlogged one (`run-experiment.ts:88-93`). [fact]
- **`flag_runs` and exclusion**: flagged runs are excluded from baseline, best and the confidence sample (`autoresearch-data.ts:26`, `state.ts:87,110`) and shown with `FLAGGED: <reason>` in the prompt (`prompt.md:73-75`). Flagging never reverts a commit — the code stays on the branch (`log-experiment.ts:77-82` only updates the row). [fact]
- **Segments**: a segment bump resets baseline/best/confidence by filtering; earlier runs are "Archived" in the dashboard (`autoresearch-dashboard.ts:406-408`).
- **Statuses and git** (summary): keep → commit; discard / crash / checks_failed → revert; all four logged with a metric (the schema requires `metric` even for crash, the model passes 0 or the parsed value) (`log-experiment.ts:27`). Runs with `metric ≤ 0` are excluded from the confidence sample (`state.ts:110`).

## 7. Git & workspace

- **VCS layer**: `@oh-my-pi/pi-natives/vcs` (Rust pi-vcs crate unifying Git and Jujutsu, commit 2a7db585 2026-08-28); the extension never shells out to `git`. [fact]
- **Branch**: `autoresearch/<slug(goal) ≤ 48 chars>-<YYYYMMDD>[-2, -3…]` (`git.ts:6-7, 186-212`); created with `checkoutNewBranch` only when the tree is clean (`:80-97`). Already on `autoresearch/*` → reused. The session stores the branch and only matches on it (`storage.ts:295-310`).
- **Dirty tree at start**: hard error with up to 5 paths listed ("Commit or stash these changes before starting autoresearch — a fresh autoresearch/* branch needs a clean baseline", `git.ts:80-86, 234-237`); test `autoresearch-state.test.ts:568`. Not a git repo → soft warning, no isolation (`git.ts:52-61`). [fact]
- **Jujutsu**: pure jj (`.jj` without colocated `.git`) is rejected with an instruction to `jj git init --colocate` (`git.ts:41-50`); colocated jj-git works as git; nested cases pinned by `autoresearch-git.test.ts:46-131` (6 tests). CHANGELOG 18.0.7 (2026-08-26): autoresearch *reads* work in pure jj (`c5a8e0e:packages/coding-agent/CHANGELOG.md:1130`). [fact]
- **Harness baseline commit**: `init_experiment` on a branch stages everything (`stageFiles([])`) and commits `autoresearch: harness setup` (`init-experiment.ts:85-97, 246-254`). Off-branch the harness stays uncommitted and the tool warns that discard will not restore it (`:206-210`; also prompt warning `index.ts:359-361`).
- **Commit on keep**: only the detected modified paths are staged (`stageFiles(files)`), committed only if `hasDiff({cached:true, files})`; message `<description>` + blank + `Result: {"status":"keep","<primary>":v,"<secondary>":v…}` (`log-experiment.ts:288-311`). Recorded `commit_hash` = new HEAD (or `commit` param) (`:109-132`). Off-branch: no commit, warning "Auto-commit skipped" (`:133-136`).
- **Revert on discard/crash/checks_failed**: on branch `reset --hard HEAD` + `clean` (untracked removed; comment: never rewinds prior keep commits, `log-experiment.ts:319-331`); off branch: `git restore --source HEAD --staged --worktree <tracked run files>` and `rm -rf` of untracked run files, where "run files" = dirty-now minus dirty-before-run (`:333-353`; `git.ts:310-327`). [fact]
- **Untracked files**: status is read with `untracked: "all"` and NUL termination (`helpers.ts:243`); renames/copies contribute both paths (`git.ts:264-270`); quoted paths unquoted (`git.ts:178-184`); paths relativised to the cwd's prefix inside the repo (`git.ts:110-123`), so a subdirectory cwd only sees its own subtree [fact]; dirty paths outside the prefix are dropped (`:119-121`).
- **Modified-path detection for a run**: on branch = all currently dirty paths (clean-tree invariant); off branch = dirty-now minus `pre_run_dirty_paths` (lossy: a pre-dirty file edited again is invisible) (`log-experiment.ts:87-103`; test `autoresearch-tools.test.ts:755,800`).
- **scope_paths / off_limits / deviation**: prefix match after normalisation (`pathMatchesSpec`, `helpers.ts:173-178`; `.` matches everything); a path is a deviation if it matches any off-limits spec, or scope is non-empty and it matches none (`log-experiment.ts:365-377`). **Edits are never blocked** ("post-hoc accountability replaces edit guards", test `autoresearch-state.test.ts:604`; prompt `prompt.md:42`). Deviations are stored per run, surfaced as warnings and as the "Unjustified deviations" prompt block for up to the last 3 unjustified keeps (`index.ts:349-355`; `prompt.md:78-84`). [fact]
- **Worktrees**: none. The loop edits the user's checkout in place on the autoresearch branch; omp's agent-worktree feature (`OMP_WORKTREE_DIR`) is unrelated. [fact]
- **`clear`**: `reset --hard <baseline_commit>` + `clean` when on an autoresearch branch or `--reset-tree`; `--keep-tree` skips; the branch itself is left in place (never deleted) (`index.ts:426-442`). [fact]

## 8. Prompting & context

**Injection mechanism** [fact]: `before_agent_start` *replaces* the whole system prompt with a rendered template whose first line is `{{base_system_prompt}}` (the upstream prompt blocks joined by blank lines, `index.ts:326`), so the autoresearch block is appended to omp's normal system prompt on **every turn** while mode is on (`index.ts:293-416`; templates `prompt.md`, `prompt-setup.md`). Nothing is appended to the user message; nothing is injected as a tool result. The guard for `event.systemPrompt === undefined` (issue #3665, commit 00f98d59 2026-06-27) is pinned by `autoresearch-before-agent-start.test.ts:98`.

**What `prompt.md` carries each iteration** (`index.ts:379-413`; `prompt.md:1-103`) [fact]
- Goal (or "infer from the latest user message and capture it with `update_notes`"), working dir, active branch, baseline commit (12 chars).
- The MUST-keep-iterating sentence, the four tool one-liners, the 7-step operating protocol, the scope/accountability rules.
- `### Your notes` — the full notes blob verbatim (no cap) when non-empty.
- `### Current segment snapshot` — segment number (1-based for display), run count, baseline metric, best kept metric + run #.
- `Recent runs:` — **the last 3** results of the current segment (`currentSegmentResults.slice(-3)`, `index.ts:333`), each as `run #N: status metric — description`, optional `ASI: hypothesis | rollback_reason | next_action_hint` (≤220 chars, `index.ts:509-515`), `Modified outside scope: … (no justification)`, `FLAGGED: reason`.
- `### Unjustified deviations` — up to the last 3 kept, unflagged, unjustified deviating runs (`index.ts:349-355`).
- `### Pending run` — run #, command, parsed primary, passed/failed, "Finish the `log_experiment` step before starting another benchmark."
- `### Guardrails` — never game the benchmark, never overfit synthetic inputs, preserve correctness, finish the current run/log cycle before new user input.

**Setup prompt** (`prompt-setup.md`): Phase-1 framing, the `autoresearch.sh` contract, "You MAY edit anything else needed… committed for you when you call `init_experiment`", a 4-step plan, and the NEVER list (no run/log/notes before init; no compile-only benchmark; no legacy files). Off-branch adds a baseline warning (`index.ts:357-361`).

**Resume messages**: `resume-message.md` (hidden, agent-attributed, after `agent_end`) and `command-resume.md` (visible user message from `/autoresearch`), both ≤ 14 lines; both say "Inspect recent git history for context" and "Keep iterating until interrupted or until the configured iteration cap" (`resume-message.md:1-10`; `command-resume.md:1-14`). [fact]

**Context growth & compaction** [fact/inference]
- The design is "state outside the transcript": everything needed to continue (notes, last 3 runs, baseline/best, pending run) is re-rendered into the system prompt from SQLite each turn, so compaction can drop old tool outputs without losing the loop (fact for the mechanism; inference for the intent).
- Only the last 3 runs are in the prompt; older runs are reachable through git log (`Inspect recent git history`) and the dashboard, not through the prompt. Notes are the model's own responsibility (`update_notes`), and the prompt tells it to capture the goal there (`prompt.md:11`).
- Compaction: the session-maintenance code recognises an autoresearch run as a case where an aborted turn must be resumed after manual compaction (`session-maintenance.ts:1134-1150`); the "active goal" threshold-compaction tests (`agent-session-auto-compaction-queue.test.ts:1354-1808`) cover long tool loops generally. There is no autoresearch-specific compaction hook, no summary of runs beyond 3, and the `benchmark.log` tail is 10 lines/4 KiB per run (§4), which keeps per-iteration transcript growth small.

**How notes/ideas feed back**: notes are injected verbatim every turn (`prompt.md:47-52`, "The notes are injected into your system prompt every iteration", `prompt.md:26`); ideas are just a `## Ideas` section inside the same blob (`update-notes.ts:73-95`). There is no automatic pruning or ranking of ideas. [fact]

## 9. UI

Code: `HEAD:packages/tui/src/apps/autoresearch-dashboard.ts` (869 lines; moved from `c5a8e0e:packages/coding-agent/src/autoresearch/dashboard.ts`, 436 lines, by commits 0d6dbd32/dbf3afad on 2026-09-17) plus the four tool renderers in `HEAD:packages/tui/src/tools/autoresearch.ts:198-435`. [fact]

- **Surfaces**: (a) an inline *widget* registered under key `"autoresearch"` via `ctx.ui.setWidget` (`autoresearch-dashboard.ts:97-123`), shown whenever mode is on, results exist, a run is running or a run is pending (`shouldShowDashboard`, `:539-546`); (b) an *overlay* (`ctrl+shift+x`) with a scroll view, spinner/elapsed running line and footer (`:136-215`); (c) a native "Tern" sheet role `omp.overlay.autoresearch` for the GUI frontend (`:153-154`; CHANGELOG 18.8.4 fixes its missing Close button, `HEAD:packages/coding-agent/CHANGELOG.md:15`). [fact]
- **Collapsed line** (default): `autoresearch pending run #N pass|fail | metric=v | log_experiment required [| mode off]`, or `autoresearch baseline pending | <name> | run the baseline`, or a one-line summary with kept/crashed/checks_failed counts, best metric and archived-run count (`:279-343, 560-628`). `ctrl+x` expands (`index.ts:227-238`).
- **Expanded / overlay content** (`describeDashboard`, `:348-513`): a `kv` grid — Current segment (`N runs  k kept  d discarded  c crashed  f checks_failed`), Baseline `v (#run)`, Archived, Pending run, Mode, Best `v (#run) ±x.x%  conf 2.3x` coloured by threshold (`confidenceToken`, `:522-524`), Secondary deltas; a **bars chart** of the metric per run when the terminal supports `chart` and ≥2 rows (`:482-511`); a **table** of the latest 8 runs in the widget (`maxRows = 8`, `:123`) or all runs in the overlay (`maxRows = 0`, `:175`) with columns #, status, metric, each secondary, description (`:445-512`, ANSI fallback `:629-795`).
- **Running state**: while `run_experiment` executes, the widget shows `autoresearch running... | <name> | bash autoresearch.sh` and the tool card streams elapsed + output tail every second (`:528-537`; `run-experiment.ts:138-149, 290-294`).
- **Tool cards**: `init_experiment` shows name + metric/segment lines; `run_experiment` shows PASSED/FAILED + parsed metric + tail; `log_experiment` colours keep=success, discard=warning, else error and shows the metric delta; `update_notes` shows a 100-char preview (`tools/autoresearch.ts:198-435`).
- **Notifications**: only `ctx.ui.notify` toasts for mode on/off/cleared, branch created/warnings, "No autoresearch results yet", reset-to-baseline results (`index.ts:151,160,175,179,222,232,433-441,460`). No OS notification, no sound, no chat-channel message. [fact]
- Status line / footer: none specific to autoresearch (the widget is the only persistent surface). [fact]

## 10. Guards

- **Reward hacking**: prompt-only — "NEVER game the benchmark", "NEVER overfit to synthetic inputs", "MUST preserve correctness" (`prompt.md:100-102`), "log honestly" (`:32`), plus the retroactive `flag_runs` mechanism that excludes a suspect run from baseline/best math (`log-experiment.ts:77-82`; `prompt.md:45`). The harness itself is in the repo and editable by the model; the only protection is the instruction not to edit `autoresearch.sh` mid-segment and to bump the segment when it does (`prompt.md:14,30`). There is no hash of the harness, no diff check of `autoresearch.sh` on keep, no detection that the metric line was printed by something other than the workload. [fact]; [inference] weakest point, see §15.
- **Scope**: post-hoc only; deviations recorded, never blocked (§7); unjustified deviating keeps are re-surfaced for 3 iterations (`index.ts:349-355`).
- **Flagging**: per-run, reason stored, shown as `FLAGGED:` in the recent list; does not revert code (§6).
- **Max iterations**: soft per-segment cap, switches mode off (§1).
- **Timeouts**: 600 s default per run, caller-settable, 0 = unbounded (§4).
- **Interrupts**: the run honours the tool abort signal (`run-experiment.ts:137, 321-323`); user messages queued during a turn suppress the auto-resume (`index.ts:262-265`).
- **Dirty-tree guard** at start and the clean-tree invariant per iteration on the branch (§7).
- **Parser hardening**: prototype-pollution keys refused in METRIC/ASI/metrics maps (`helpers.ts:9, 192-239`; `storage.ts:682`).
- **Cost limits**: none — no token, dollar, wall-clock or run-count limit beyond `max_iterations`. [fact]
- **Pending-run discipline**: a completed run must be logged before the next (prompt + `log_experiment` precondition), but `run_experiment` does not refuse — it abandons the pending one and says so (`run-experiment.ts:88-93, 228-230`; test `autoresearch-tools.test.ts:412`). [fact]

## 11. Parallelism

- Strictly sequential: one process at a time per `run_experiment` call, one pending run per session (`getPendingRun … LIMIT 1`, `storage.ts:519-525`), no subagents, no worktrees, no parallel experiments. [fact]
- One open session per branch and the storage key is the repo root, so two omp sessions in the same repo on the same autoresearch branch would share the DB and the pending-run slot; a second `run_experiment` would abandon the first's pending run (`run-experiment.ts:88-93`). Two sessions on *different* autoresearch branches in the same checkout cannot coexist either, because the branch is the working tree's. [fact → inference on hazards]
- The in-memory runtime is per chat session id (`runtimeStore`, `state.ts:199-213`); the DB is shared, cached per path (`storage.ts:552`). SQLite WAL + `busy_timeout` handling came from a generic fix (commit 03bead1e 2026-06-12, "retried SQLITE_BUSY family"). [fact]
- The run's `executeBash` uses `sessionKey: autoresearch:<cwd>` (`run-experiment.ts:306`), which scopes it to the omp bash session manager, not a lock. [fact]

## 12. Testing

HEAD test files (`HEAD:packages/coding-agent/test/`): [fact]
- `autoresearch-tools.test.ts` (883 lines, 19 `it`): `init_experiment` opens/updates/bumps segment, rejects without `autoresearch.sh`, auto-commits harness only on an autoresearch branch (`:183-347`); `run_experiment` rejects without session, parses METRIC/ASI and stores a run, abandons a prior pending run instead of blocking (`:362-438`); `log_experiment` rejects without pending run, keep updates baseline, flags deviations + warns without justification, records justification, `flag_runs` works, off-branch discard reverts only run-modified files, on-branch discard preserves prior commits, on-branch keep commits files dirty *before* `run_experiment`, off-scope pre-dirty files still flagged (`:488-839`); `update_notes` replaces and refreshes state (`:854`).
- `autoresearch-state.test.ts` (624 lines, 18 `it`): baseline = first kept, ignores flagged; best-kept direction + flag exclusion; confidence null under 3 runs / zero MAD / flagged excluded from noise floor (`:50-109`); SQLite round-trips: session, run insert→complete→log→flag, segment bump, abandon count, `buildExperimentState` from rows, notes (`:126-371`); control-entry replay: last entry wins, `clear` nulls the goal (`:372-381`); slash command: bare → notify, argument → user message + slug branch, dirty tree → error (`:537-585`); **no `tool_call` hook registered** (edit guards deliberately absent, `:604`).
- `autoresearch-git.test.ts` (131 lines, 6 `it`): jj guardrails — pure jj rejected, colocated ok, plain git ok, no-VCS soft warning, nested pure-jj-in-git rejected, nested git-in-jj ok.
- `autoresearch-before-agent-start.test.ts` (145 lines, 2 `it`): systemPrompt undefined (issue #3665) and array join.
- `agent-session-auto-compaction-queue.test.ts` (1980 lines, 29 `it`): not autoresearch-specific; `:399` "resumes the turn a manual compaction interrupted" is the case the comment at `:401` attributes to autoresearch loops.
- Total autoresearch-named: 45 `it` blocks over 1,783 lines. Local c5a8e0e differs by a handful of lines in the state/tools tests (import paths after the pi-tui move). [fact]
- Notable edge cases pinned: pre-dirty files on branch get committed on keep (clean-tree invariant), off-branch lossy detection, abandon-instead-of-block, zero-MAD confidence, flagged exclusion, jj nesting. Not pinned: auto-resume on `agent_end`, max_iterations switch-off, `clear` reset, timeout/kill path, streaming scanner across chunk boundaries (HEAD helpers), Windows shell resolution. [fact by absence]

## 13. Configuration

- **Env**: `OMP_AUTORESEARCH_DB_DIR` — override root for the per-project DB and artifact dir (`storage.ts:575-583`; `HEAD:docs/environment-variables.md:542`). That is the only autoresearch env var. [fact]
- **Settings**: none. No `autoresearch.*` key in omp settings; the only setting consulted is the generic shell config on Windows (`run-experiment.ts:256`). [fact]
- **Per-session (tool-set) config**: everything else is set by the model through `init_experiment` — metric name/unit/direction, scope, off-limits, constraints, secondary metrics, `max_iterations`, segment — and `run_experiment timeout_seconds`. [fact]
- **Defaults**: direction `lower`; unit `""`; timeout 600 s; max_iterations none; branch prefix `autoresearch/`; slug ≤ 48 chars; recent-runs window 3; unjustified window 3; LLM tail 10 lines / 4 KiB; dashboard table 8 rows; ASI summary 220 chars; ASI value in log text 120 chars; confidence thresholds 1× / 2×; min 3 samples. (Sources in §18.)
- **Paths**: `~/.omp/autoresearch/<encoded>.db`, `~/.omp/autoresearch/<encoded>/runs/NNNN/benchmark.log` (`dirs.ts:878-896`).
- **Activation**: always loaded (`sdk.ts:2601`); tools inactive until mode on. [fact]

## 14. History

**Lineage** [fact, web]: omp's feature is an in-tree re-implementation of *pi-autoresearch* (davebcn87/pi-autoresearch, MIT, "Autonomous experiment loop extension for pi", 8.2k stars / 465 forks / 179 commits as of 2026-10-08, https://github.com/davebcn87/pi-autoresearch), which itself generalised Karpathy's autoresearch (March 2026) from ML training to "any optimization target" (https://rywalker.com/research/pi-autoresearch, 2026-06-11). pi-autoresearch's file-based state (`autoresearch.md`, `autoresearch.sh`, `.autoresearch/`, `autoresearch.jsonl`, `autoresearch.config.json`, `autoresearch.checks.sh`, `program.md`, `ideas.md`) is exactly omp's `LEGACY_ARTIFACTS` delete list (`index.ts:464-473`) — omp kept `autoresearch.sh` and moved everything else into SQLite. [fact + inference]

**Commits touching `packages/coding-agent/src/autoresearch` (GitHub API, newest first; 49 total, full list in /tmp/ar-recon-omp/autoresearch-dir-commits.txt; filtered to the relevant 28)** [fact]:
- 2026-10-07 `9895b045` perf(coding-agent): stream memory rollout windows and bound experiment output (#14718)
- 2026-10-03 `47b11566` fix: made the full test suite pass on Windows without regressing Linux
- 2026-09-17 `dbf3afad` refactor: restructured terminal UI into modular @oh-my-pi/pi-tui package
- 2026-09-17 `0d6dbd32` refactor: migrated UI renderers to @oh-my-pi/pi-tui, decoupled core
- 2026-08-28 `2a7db585` feat: added pi-vcs crate unifying Git and Jujutsu, dropping old calls
- 2026-06-27 `00f98d59` fix(autoresearch): guarded before_agent_start against undefined event.systemPrompt
- 2026-06-21 `e8b60408` Merge PR #1939: fix(coding-agent): guard Git-mutating automation in pure jj workspaces (@roboomp)
- 2026-06-12 `f34610b1` Merge remote-tracking branch 'origin/farm/428ce54a/sqlite-busy-recovery-during-concurrent-s'
- 2026-06-12 `03bead1e` fix(auth): retried SQLITE_BUSY family and hoisted busy_timeout
- 2026-06-10 `cdd54a21` docs(prompts): standardized RFC keywords across prompt surface
- 2026-06-05 `d7c7d2b0` fix(coding-agent): guarded nested pure jj before outer-git fallback
- 2026-06-05 `7ba2227c` fix(coding-agent): guarded Git-mutating automation in pure jj workspaces
- 2026-06-04 `c8b4bf09` prompts: undo experiment, update system
- 2026-06-04 `95defa71` docs(compaction): rewrote prompts in terse scratchpad style
- 2026-05-31 `47c70f64` refactor(autoresearch): migrated executeProcess to use executeBash
- 2026-05-04 `8c323666` feat: added ordered systemPrompt arrays and normalized context prompts
- 2026-05-02 `31ae1b96` feat(coding-agent/autoresearch): added branch-specific state restore
- 2026-05-01 `3293231a` feat(coding-agent): implemented sqlite-backed storage for autoresearch
- 2026-05-01 `c6cb17e8` refactor(coding-agent/autoresearch): simplified ASI requirements validation
- 2026-04-25 `5c1b8eaa` feat(coding-agent): compressed examples in prompts
- 2026-04-08 `a21a542a` refactor(prompt-templates): migrated prompt utilities to pi-utils package
- 2026-04-06 `266aabf2` feat(autoresearch): enabled selective segment creation and working tree preservation in experiments
- 2026-04-06 `3d292b0b` refactor(autoresearch): overhaul tools, state management, and contract handling
- 2026-03-23 `2c936557` feat(autoresearch): added auto-resume, path validation, and security guards
- 2026-03-23 `003f46f4` feat(coding-agent/autoresearch): added contract validation and run tracking system
- 2026-03-22 `cac628cc` feat(coding-agent): added git isolation and keybinding utilities for autoresearch
- 2026-03-22 `66a478e6` feat(coding-agent): implemented dynamic tool activation for autoresearch lifecycle management
- 2026-03-22 `2771c239` feat(autoresearch): introduced autonomous experiment loop with metric-driven optimization

**Reading of the evolution** [inference from messages + code]:
1. 2026-03-22: feature lands in one day (loop + dynamic tool activation + git isolation), days after Karpathy's autoresearch repo (March 2026).
2. 2026-03-23: "contract validation and run tracking", then "auto-resume, path validation, and security guards" — the agent_end auto-continue and the prototype-pollution guards date from here.
3. 2026-04-06: "overhaul tools, state management, and contract handling" + "selective segment creation and working tree preservation" — segments and `--keep-tree` appear.
4. 2026-05-01/02: SQLite storage replaces file state; branch-specific restore (session ↔ branch matching).
5. 2026-05-31: `executeProcess` moved onto omp's `executeBash` (shared process manager).
6. 2026-06-05/21: jj guards (pure jj rejected, nested cases) — pitfall: git automation ran "behind jj's back".
7. 2026-06-27: `event.systemPrompt` undefined crash (issue #3665) — pitfall: a prompt hook that throws kills the turn.
8. 2026-08-28: pi-vcs native crate replaces git CLI calls.
9. 2026-09-17: UI moved to `@oh-my-pi/pi-tui` (dashboard, renderers, shared types) — the main local-vs-HEAD difference.
10. 2026-10-03: Windows: `bash` on PATH was WSL — harness now runs under the configured POSIX shell (CHANGELOG 18.5.0, `HEAD:packages/coding-agent/CHANGELOG.md:414`).
11. 2026-10-07: streaming output scanner + bounded memory for long benchmarks (#14718).

**CHANGELOG mentions** (the feature is almost never in the changelog) [fact]: 18.0.7 (2026-08-26) "autoresearch reads now work in pure Jujutsu workspaces" (`c5a8e0e:packages/coding-agent/CHANGELOG.md:1130`); 18.5.0 (2026-10-03) Windows WSL fix (`HEAD:packages/coding-agent/CHANGELOG.md:414`); 18.8.4 (2026-10-08) Tern autoresearch sheet Close button (`HEAD:packages/coding-agent/CHANGELOG.md:15`). `c5a8e0e:packages/coding-agent/DEVELOPMENT.md:75` files `autoresearch/` under "background research".

**Local c5a8e0e (18.2.0, 2026-09-15) vs HEAD 153b37c3 (18.8.4)** — `diff -ru` = 882 lines, 10 files, +186/−378 (`/tmp/ar-recon-omp/ar.diff`) [fact]:
- `dashboard.ts` deleted from coding-agent; lives at `packages/tui/src/apps/autoresearch-dashboard.ts` (869 lines, grew with native/Tern nodes).
- `types.ts` 168 → 66 lines: domain types (`ExperimentResult`, `ExperimentState`, `ASIData`, `RunDetails`, `LogDetails`, `DashboardController`) moved to `packages/tui/src/tools/autoresearch.ts`.
- `state.ts` 273 → 234: `currentResults`, `findBaseline*`, `findBaselineSecondary`, `isBetter` moved to `packages/tui/src/apps/autoresearch-data.ts`; `computeConfidence` unchanged.
- `helpers.ts` 218 → 255: `+ExperimentOutputScanner` (streaming), `−formatNum/fmtNum/commas/formatElapsed/isBetter` (moved).
- `run-experiment.ts` 407 → 394: streaming scanner + `TailBuffer` instead of re-reading the log; `resolveHarnessExecLine` for Windows; renderers moved out.
- `log-experiment.ts` 521 → 477, `init-experiment.ts` 270 → 254, `update-notes.ts` 109 → 95: renderers moved out; logic unchanged.
- `storage.ts`, prompts, `git.ts`, `index.ts` logic: unchanged (index only re-imports).
- Behavioural deltas for a borrower: none in the decision/git/prompt logic; only output scanning memory and Windows shell. [fact]

## 15. Weaknesses

All items [inference] unless a line cites code that makes them [fact].

1. **The benchmark is in the model's hands.** `autoresearch.sh` is a tracked file the model wrote and may edit; `log_experiment keep` commits whatever is dirty, including the harness (`log-experiment.ts:87-96, 115-124`). Nothing hashes the harness at `init_experiment`, nothing compares it on keep, nothing checks that `METRIC` lines came from the workload. The only defence is prose (`prompt.md:14,100`). A segment bump is voluntary.
2. **Keep/discard is self-reported.** The tool accepts any `metric` and any `status`; a logged metric that contradicts the parsed one is only a warning (`log-experiment.ts:171-175`). The harness never refuses a keep that regressed the parsed primary.
3. **The confidence number is weak statistics.** One scalar per segment = |best − baseline| / MAD over *all* runs (including crashes with metric 0 excluded, discards included) (`state.ts:105-131`). It is not per-run, does not use repeated measurements of the same commit, and flips to `null` whenever MAD is 0 (e.g. integer metrics that repeat). No automatic re-run, no variance estimate of the baseline.
4. **The loop's persistence is weaker than the prompt claims.** Continuation depends on `autoResumeArmed`, which is set only by tool calls (`init/run/log-experiment`) and cleared on rehydrate (`index.ts:53`); a turn that ends in prose, a provider error that ends the turn without `agent_end` arming, or a restart of omp leaves the loop idle until the user types `/autoresearch`. There is no scheduler, no watchdog, no "N iterations remaining" accounting across restarts.
5. **No budget.** No wall-clock, token, cost, or total-run limit; `max_iterations` is per segment and optional (`init-experiment.ts:58-61`). An unattended loop runs until the model stops or the user interrupts.
6. **Single working tree, destructive git.** `reset --hard` + `clean` on every discard (`log-experiment.ts:325-326`) and on `clear` (`index.ts:431-432`); any file the user adds on that branch mid-loop is destroyed. Off-branch mode has a lossy path diff (`log-experiment.ts:97-103`) and no revert of pre-dirty harness files. No worktree isolation, so the user cannot work in the repo while the loop runs.
7. **Notes are unbounded and unstructured.** One text column injected whole every turn (`prompt.md:47-52`); a long-running session's notes grow the system prompt linearly; ideas are bullets with no status/priority, no dedupe, never pruned.
8. **Only the last 3 runs are visible to the model** (`index.ts:333`); older learning must survive via notes or git log. No per-run artifacts beyond `benchmark.log` (no diff snapshot, no profile) — a discarded idea's code is gone after the reset, only its description remains.
9. **One session per branch, one project per DB, no cross-session memory**: a new goal on a new branch starts from zero; nothing carries "what worked last time in this repo" except whatever the model reads from old branches.
10. **No interleaving with the user.** A mid-loop user message is deferred by instruction (`prompt.md:103`) and by the auto-resume skip (`index.ts:262-265`); there is no way to queue a hint or a new idea into the loop without a turn.
11. **Fragile surfaces**: `event.systemPrompt` undefined crash (fixed 2026-06-27); jj behind-the-back commits (fixed 2026-06); Windows `bash` = WSL (fixed 2026-10-03); unbounded memory reading the whole `benchmark.log` (fixed 2026-10-07). Pattern: each shell/VCS/prompt seam needed a fix after release.
12. **Observability is TUI-only**: no notification when a run finishes or the loop stops, no export of the run table, no link from a kept commit back to the run row other than the `Result:` JSON in the commit message.
13. **Error swallowing**: `tryGitStatus`/`tryGitPrefix`/`tryReadBranch` return `""`/`null` on any failure (`helpers.ts:241-255`; `index.ts:535-541`), so a broken git state looks like "no repo" and silently disables isolation.

## 16. Techniques worth borrowing

Ranked; each: why, and what another harness (theuth) needs to have it.

1. **State outside the transcript, re-rendered every turn** (`index.ts:293-416`; `prompt.md:47-97`). Why: compaction-proof, restart-proof, and the model always sees the same compact truth (goal, notes, baseline/best, last 3 runs, pending run). Needs: a per-turn system-prompt hook that can read a store, and a store (SQLite table or knowledge concept) keyed by project + branch.
2. **Pending-run invariant with "abandon, don't block"** (`storage.ts:519-525`; `run-experiment.ts:88-93`; `prompt.md:85-97`). Why: a crash between run and log is recoverable, and the model is never dead-locked by its own mistake. Needs: run rows with `status NULL` = pending, `abandoned_at`, and a prompt block for the pending run.
3. **Harness-as-a-file contract with a one-line metric grammar** (`prompt-setup.md:23-28`; `helpers.ts:10-11`). Why: trivially testable by hand (`bash autoresearch.sh`), language-agnostic, machine-parsable; secondaries and free-form `ASI key=value` ride the same channel. Needs: a streaming line scanner (borrow the O(line) design, `helpers.ts:27-84`) and a per-run log file.
4. **Phase 1 / Phase 2 split enforced by tool availability** (`init-experiment.ts:69-81`; `run-experiment.ts:75-84`; `prompt-setup.md`). Why: the model cannot start optimising before a measurable, committed baseline exists. Needs: dynamic tool activation per mode and a "no session" error from the loop tools.
5. **Baseline commit + per-keep commits with structured trailers; discard = reset to HEAD** (`init-experiment.ts:85-99, 246-254`; `log-experiment.ts:298-331`). Why: git is the undo log and the audit trail; every kept step is bisectable. Needs: a VCS layer and a dedicated branch (or, better, a worktree — see §17).
6. **Segments** (`init-experiment.ts:137-144`; `autoresearch-data.ts:20-22`). Why: changing the benchmark invalidates comparisons; filtering by segment keeps old runs without poisoning stats. Needs: a segment integer on session and run.
7. **Post-hoc scope accountability instead of edit blocking** (`log-experiment.ts:365-377`; `index.ts:349-355`; test `autoresearch-state.test.ts:604`). Why: edits are never refused (no stuck agent), but every out-of-scope keep is recorded and nagged for 3 turns unless justified. Needs: dirty-path detection per run and `justification` on the log tool.
8. **Retroactive `flag_runs`** (`log-experiment.ts:77-82`). Why: lets the model (or the user) disown a reward-hacked or noisy result without rewriting history; stats exclude it. Needs: `flagged`/`flagged_reason` columns and exclusion in baseline/best/confidence.
9. **Noise-floor confidence in the tool's own words** ("2.3x noise floor (likely real)", `log-experiment.ts:451-453`). Why: a cheap, explainable nudge toward re-running; borrow the *presentation*, improve the math (per-commit repeats, see §15.3). Needs: ≥3 metric samples per segment.
10. **Hidden, agent-attributed continuation message on `agent_end`, suppressed when the user has queued input** (`index.ts:256-291`). Why: keeps the loop going without faking user turns, and the user always wins. Needs: a hook at turn end that can enqueue a next-turn message with `triggerTurn` and `display:false` semantics; theuth has routines/schedules and `remind_me_in`, which could implement the same with a budget.
11. **Mode persisted as a replayable session entry** (`autoresearch-control`, `index.ts:117`; `state.ts:181-197`). Why: resume, branch and fork all reconstruct mode from the log, no side files. Needs: custom entries in the session log.
12. **Legacy-artifact hygiene** (`index.ts:464-487`; prompt NEVER-list). Why: the previous generation of the feature left files in repos; the new one removes them and forbids re-creation. Needs: nothing.
13. **Ideas backlog as an appendable section of the notes** (`update-notes.ts:73-95`). Why: one blob, one tool, zero schema; good enough for a first version. Needs: nothing; a better version would be structured (status/priority).

## 17. Do-not-borrow

- **TUI widget / overlay / ctrl+x keybindings** (`autoresearch-dashboard.ts`, `index.ts:227-245`): omp-specific rendering; theuth has a desktop app and chat channels — render the run table as a card/HTML and notify through channels instead.
- **Slash-command toggle semantics** (bare `/autoresearch` toggles off, `index.ts:146-153`): easy to trigger by accident; prefer explicit start/stop.
- **Editing the user's checkout in place with `reset --hard` + `clean`** (`log-experiment.ts:325-326`; `index.ts:431-432`): destructive and blocks the user; use a worktree (theuth already has per-worker worktrees) and never `clean` a tree the user may touch.
- **Prompt-only reward-hacking defence**: add mechanical checks (harness hash at baseline, refuse keep when harness changed without segment bump, require parsed metric to match the logged one, cross-check secondaries).
- **Confidence = |best − baseline| / MAD of all runs** (`state.ts:105-131`): misleading as a significance measure; replace with repeated measurements of baseline and candidate and a proper comparison.
- **Unbounded notes injected whole** (`prompt.md:47-52`): cap and structure.
- **`metric` required on crash/checks_failed** (`log-experiment.ts:27`): forces fake zeros into the stats; make it optional for non-keep statuses.
- **Off-branch "lossy" mode** (`log-experiment.ts:97-103`): running without isolation is a trap; refuse or always isolate.
- **"Run any benchmark command" description with a fixed command** (`run-experiment.ts:68, 95`): confusing to the model; keep the description honest.
- **Single hidden continuation message with no budget** (`index.ts:280-290`): borrow the mechanism (§16.10) but add a wall-clock/run budget and a user-visible status.
- **No `checks` script**: omp dropped pi-autoresearch's `autoresearch.checks.sh` and left validation to the model's judgement (`prompt.md:37`); a borrower should keep a mandatory, harness-run correctness check that gates `keep`.

## 18. Numbers

**LOC** (`wc -l`) [fact]
| file | c5a8e0e | HEAD 153b37c3 |
|---|---|---|
| index.ts | 542 | 541 |
| state.ts | 273 | 234 |
| storage.ts | 700 | 700 |
| git.ts | 327 | 327 |
| helpers.ts | 218 | 255 |
| dashboard.ts | 436 | — (→ tui/apps/autoresearch-dashboard.ts 869) |
| types.ts | 168 | 66 (→ tui/tools/autoresearch.ts 435, tui/apps/autoresearch-data.ts 64) |
| tools/init-experiment.ts | 270 | 254 |
| tools/run-experiment.ts | 407 | 394 |
| tools/log-experiment.ts | 521 | 477 |
| tools/update-notes.ts | 109 | 95 |
| prompt.md / prompt-setup.md / resume-message.md / command-resume.md | 103 / 43 / 10 / 14 | same |
| **total in autoresearch/** | **4,141** | **3,513** (+1,368 in pi-tui) |

**Tests** [fact]: 4 files, 1,783 lines, 45 `it` blocks (tools 19, state 18, git 6, before-agent-start 2); compaction-queue test 1,980 lines / 29 `it` (shared).

**Limits & defaults** [fact]
- run timeout default **600 s** (`run-experiment.ts:37,129`); `0` → 2,147,000,000 ms (`:307`)
- progress update interval **1 s** (`run-experiment.ts:293`)
- model-visible output tail **10 lines / 4 KiB** (`helpers.ts:6-7`); tail buffer 2× display budget (`run-experiment.ts:270`)
- recent runs in prompt **3**; unjustified deviations in prompt **3** (`index.ts:333,351`)
- ASI summary in prompt **220 chars** (`index.ts:514`); ASI value in log text **120 chars** (`log-experiment.ts:476`)
- confidence: min **3** samples, thresholds **≥1** marginal / **≥2** likely real (`state.ts:111`; `log-experiment.ts:452`)
- dashboard widget rows **8**; overlay all (`autoresearch-dashboard.ts:123,175`)
- branch slug **≤48 chars**, prefix `autoresearch/`, date `YYYYMMDD`, suffix `-2…` (`git.ts:6-7,186-212`)
- dirty-path preview in start error **5** paths (`git.ts:234-237`)
- run directory zero-padded to **4** digits (`run-experiment.ts:110`)
- commit sha shown **12** chars (`index.ts:389`; `log-experiment.ts:217,308`)
- `max_iterations` default **none**; `direction` default `lower`; `metric_unit` default `""` (`init-experiment.ts:51-61`)
- SQLite: 2 tables, 2 indexes, WAL (`storage.ts:195-252`); sessions 18 columns, runs 28 columns
- commits on the directory: **49** (2026-03-22 → 2026-10-07); first commit 2771c239; CHANGELOG mentions **3**
- pi-autoresearch upstream: 8.2k stars, 465 forks, 179 commits (2026-10-08, web)

## 19. Citations checked

Re-opened after writing (read_file at the cited offset, needle = the quoted token); all paths under HEAD 153b37c3 unless marked. 24 checked, 24 ok, 0 wrong.

- ok — HEAD:…/autoresearch/index.ts:262-265 agent_end skips auto-resume when user messages pending
- ok — HEAD:…/index.ts:280-290 hidden autoresearch-resume message, deliverAs nextTurn, triggerTurn
- ok — HEAD:…/index.ts:333 recent runs = last 3
- ok — HEAD:…/index.ts:464-473 LEGACY_ARTIFACTS list
- ok — HEAD:…/state.ts:105-131 computeConfidence = |bestKept-baseline|/mad, <3 → null, mad 0 → null
- ok — HEAD:…/autoresearch-data.ts:25-27 baseline = first unflagged keep
- ok — HEAD:…/log-experiment.ts:233-243 max_iterations switches mode off + detaches tools
- ok — HEAD:…/log-experiment.ts:319-331 discard on branch = reset --hard HEAD + clean
- ok — HEAD:…/log-experiment.ts:298-305 commit message with Result JSON
- ok — HEAD:…/log-experiment.ts:452 confidence thresholds 2 / 1
- ok — HEAD:…/run-experiment.ts:129 timeout default 600
- ok — HEAD:…/helpers.ts:6-7,10-11 EXPERIMENT_MAX_LINES 10 / 4 KiB; METRIC/ASI regex
- ok — HEAD:…/init-experiment.ts:85-97 harness auto-commit on branch when dirty
- ok — HEAD:…/git.ts:80-86 dirty tree → hard error at start
- ok — HEAD:…/storage.ts:574-588 DB path + OMP_AUTORESEARCH_DB_DIR
- ok — HEAD:docs/environment-variables.md:542 OMP_AUTORESEARCH_DB_DIR documented
- ok — HEAD:packages/utils/src/dirs.ts:878-896 ~/.omp/autoresearch paths
- ok — HEAD:…/prompt.md:103 finish current run/log cycle before new user input
- ok — HEAD:…/index.ts:117 autoresearch-control entry appended by setMode
- ok — HEAD:…/init-experiment.ts:153 / run-experiment.ts:216 / log-experiment.ts:212 arm auto-resume
- ok — HEAD:…/state.ts:176-177 bestMetric = baseline, confidence stored on state
- ok — HEAD:…/test/autoresearch-state.test.ts:604 no tool_call hook
- ok — HEAD:…/sdk.ts:2601 extension loaded unconditionally
- ok — HEAD:…/autoresearch-dashboard.ts:123 widget rows 8

Not re-verified (stated as read once): line numbers inside `c5a8e0e` files other than CHANGELOG:1130 and DEVELOPMENT.md:75 (the local tree was only used for `wc -l` and the diff); the pi-autoresearch star/fork/commit counts (single GitHub page read, 2026-10-08); the Karpathy-origin claim rests on the rywalker.com page (2026-06-11), not on a primary source.
