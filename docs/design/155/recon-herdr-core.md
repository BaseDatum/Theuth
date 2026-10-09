# herdr — THEIRS-CORE (A1–A4)

_Recon of https://github.com/herdrdev/herdr at /tmp/herdr-recon (v0.9.3, HEAD 2563803d, 2026-10-08). Read-only; cites are path#La-Lb relative to the clone root. Written 2026-10-09 01:16–02:05 by the theirs-core recon child; no code copied._

## Summary

- **A1** A 300 ms per-pane task identifies the agent from the shell's foreground process group, then classifies the live bottom-buffer text + OSC title/progress with per-agent TOML manifests (22 bundled, 1,864 lines; omp has none — its state needs the extension); "blocked" only on positive chrome, else `idle` + `default_known_agent_idle_fallback`; a 6.4 K-line arbiter makes complete hook integrations authoritative and lets a visible blocker override a stale non-blocked hook; remote catalog from herdr.dev every 30 min, version-monotonic, no signature; `herdr agent explain`.
- **A2** Every pane exports `HERDR_ENV/PANE_ID/BIN_PATH/SOCKET_PATH`; agents call back `pane.report_agent[_session]`/`release_agent` with a monotonic `seq`; omp (pi extension) and opencode (plugin) report full lifecycle + session ref, Claude/Codex/Copilot/… report *session identity only* after a 0.6.7 reversal; resume is a per-agent argv table typed into a fresh shell, deduped on `source\0agent\0kind\0value` (or cwd+argv for reported commands); configs edited as JSONC CSTs through hard-link/symlink-safe atomic writes.
- **A3** One server per session owns the PTYs; `session.json` (debounced 5 s, 48 rotating snapshots) restores layout and resumes agents but never processes; `herdr update --handoff` ships every master fd over SCM_RIGHTS to a new server with restored/ready/committed/owned and a rollback that re-binds the old sockets; logind delay inhibitor on Linux only; sha256-checked self-update.
- **A4** NDJSON on a 0600 socket, 102 `noun.verb` methods, 512-event hub with `events_lost`, `pane.wait_for_output` and `agent.wait --until`, `agent.prompt --wait` with a 5 s activity gate (`agent_prompt_stalled`), schemars bundle diffed by a test, and a skill that begins with `test "$HERDR_ENV" = 1`.
- **Three most borrowable ideas:** (1) the prompt-effect gate + `until`-state waits for PTY/ACP children (A4, S/M) — directly answers "Sky misses that a child is blocked" and "did my send land"; (2) fd handoff of PTY jobs and ACP pipes across an engine replacement with the rollback half (A3, L) — the only mechanism that keeps work alive through an update; (3) the tiered trust rule — an agent owns its state only if its event stream is complete, otherwise it only contributes identity — plus positive-evidence blocked with a typed fallback reason (A1/A2, S).

## A1 Agent state detection

**One line.** Per pane, a 300 ms detection task first identifies *which* agent owns the pane from the shell's foreground process group, then classifies the live bottom-of-buffer text (plus the last OSC 0/2 title and OSC 9 progress payload) with a per-agent TOML manifest of prioritised rules; a central arbiter in `TerminalState` lets an installed hook integration override the screen, and a strict policy makes "blocked" require positive visible evidence.

### The state machine
- Four states: `Idle` ("agent finished, prompt visible"), `Working`, `Blocked` ("needs human input"), `Unknown` ("plain shell or unrecognized program") — `src/detect/mod.rs#L9-L20`.
- A screen classification is richer than a state: `AgentDetection { state, skip_state_update, visible_idle, visible_blocker, visible_working }`. `visible_blocker` is documented as "stronger than arbitrary prompt-like text in the scrollback and may override a non-blocked integration state"; `visible_working` is explicitly "diagnostic metadata … PTY activity is the normal working authority" — `src/detect/mod.rs#L22-L39`.
- Unknown program → `Unknown` without reading the screen at all: `detect_agent_with_osc(None, …)` short-circuits before the manifest engine — `src/detect/mod.rs#L305-L328`.

### Inputs that move the state (in evaluation order inside one tick)
1. **Process identity, not prompt text.** The pane's shell PID is asked for its controlling-tty foreground process group (`e_tpgid` via `proc_pidinfo` on macOS; `tcgetpgrp` for an fd) — `src/platform/macos.rs#L509-L541`; the processes of that group are matched by basename/alias table (`claude`/`claude-code`, `opencode`/`opencode2`/`open-code`, `omp`, `cursor-agent`, …) — `src/detect/mod.rs#L198-L228`, with special-case unwrapping for node/python/powershell wrappers and a `HERDR_AGENT=<label>` env hint on the foreground process for sandboxes that hide the binary (`src/platform/mod.rs#L547`, docs `docs/next/website/src/content/docs/agents.mdx#L68-L72`). This is how "shell prompt vs agent prompt" is told apart: a shell at its prompt has itself as the foreground group → `agent = None` → `Unknown`; `pane_shell_is_idle()` wraps the same probe (`src/detect/mod.rs#L376-L379`).
2. **Screen tail + OSC.** The detection text is the live bottom of the buffer (not the scrolled view); the manifest evaluator receives `DetectionInput { screen, osc_title, osc_progress }` — `src/detect/manifest.rs#L16-L25`. OSC capture is an always-on passive tracker that keeps the last OSC 0/2 title (sanitised, ≤ 256 chars) and the raw OSC 9 payload such as `4;3;` (ConEmu progress) — `src/pane/osc.rs#L483-L530`.
3. **Integration hook reports** (`pane.report_agent` over the socket, A2) become a `HookAuthority { source, agent_label, state, message, reported_at, session_ref }` on the terminal — `src/terminal/state.rs#L17-L26`.
4. **Process exit / job control.** A ctrl-z'd or backgrounded agent is tracked by pid + start token so it is not mistaken for an exit; "the visible screen belongs to another job while the agent waits in the background, so it says nothing about its state" — `src/pane/background_agent.rs#L1-L36`, `src/pane.rs#L3139-L3145`.
5. **Alt screen / transcript viewers** are handled in-manifest: a rule with `state = "unknown"` and `skip_state_update = true` (e.g. Claude's `transcript_viewer`, priority 1000) freezes the published state while a viewer covers the live prompt — `distribution/agent-detection/claude.toml#L64-L77`; the validator refuses `skip_state_update` on any state other than `unknown` or combined with visible-evidence flags — `src/detect/manifest.rs#L931-L944`.

### Unknown programs, shell prompts and the `HERDR_AGENT` hint
- A pane whose foreground group is not a known agent is `Unknown` and the sidebar shows a plain terminal; the screen is never classified for it (`src/detect/mod.rs#L305-L319`). This is also why a shell prompt is never mistaken for an idle agent: identity comes from the kernel's foreground pgid, not from text.
- When the agent process cannot be seen (a sandbox such as `fence` or `nono`), `HERDR_AGENT=<label>` on the *host-visible* foreground process selects the manifest to use; the docs warn against exporting it globally "unless every inherited foreground process should be treated as that agent" — `docs/next/website/src/content/docs/agents.mdx#L68-L72`.
- Restricted Linux runtimes without a foreground pgid can opt into `HERDR_PROCESS_DETECTION=child-groups`, documented as best effort ("a newer background job can be mistaken for the foreground job") — `docs/next/website/src/content/docs/agents.mdx#L72`, `src/platform/linux.rs#L33`.
- An agent inside tmux inside a pane is invisible: herdr sees `tmux`; the docs supply a `HERDR_ENV` guard for shell rc files that auto-attach tmux — `docs/next/website/src/content/docs/agents.mdx#L105-L112`.

### Manifest format
- Top level: `id`, `version` (dotted numeric, compared segment-wise), `min_engine_version` (engine is at 3), `updated_at`, `aliases`, `[[rules]]` — `src/detect/manifest.rs#L139-L151`, `src/detect/manifest_update.rs#L15-L18`.
- A rule: `id`, `state`, `priority` (i32, default 0), `region` (default `whole_recent`), the three evidence flags, `skip_state_update`, and a gate tree of `all` / `any` / `not` lists whose leaves are `contains` (substring, case-insensitive), `regex`, `line_regex` (per-line anchored) — `src/detect/manifest.rs#L153-L199`. `deny_unknown_fields` is on both structs so a typo fails the whole manifest.
- Regions are named slices of the tail: `whole_recent`, `after_last_prompt_marker`, `before_current_prompt_marker`, `current_prompt_block_marker`, `prompt_box_body`, `above_prompt_box`, `last_non_empty_above_prompt_box`, `after_last_horizontal_rule`, `osc_title`, `osc_progress`, plus `bottom_lines(n)` / `bottom_non_empty_lines(n)` / `top_non_empty_lines(n)` — `src/detect/manifest.rs#L1094-L1117`.
- Complexity caps: ≤ 128 rules, gate depth ≤ 8, ≤ 512 gates, ≤ 32 matchers per gate, ≤ 1024 matchers, ≤ 512 chars per matcher; every positive gate needs a positive matcher; regexes are compiled at load — `src/detect/manifest.rs#L268-L273`, `src/detect/manifest.rs#L977-L1009`.
- Evaluation: every rule is evaluated (for explain), the highest priority match wins, earlier wins ties; the evidence flags are only honoured when the rule's state matches (`visible_blocker && state == Blocked`) — `src/detect/manifest.rs#L431-L517`.
- **Strictness is structural, not a flag.** When no rule matches for a *known* agent the result is `Idle` with `fallback_reason = "default_known_agent_idle_fallback"` and no visible flags — `src/detect/manifest.rs#L519-L574`; the docs state the policy: "Herdr only marks `blocked` when the live bottom-buffer snapshot matches known visible approval, question, or permission UI" and admit new prompts "may initially show as `idle`" — `docs/next/website/src/content/docs/agents.mdx#L74-L78`. There are no generic "? " or "y/n" heuristics anywhere in the engine: a blocked rule exists only if someone wrote one for that agent's chrome.

### How opencode and omp are detected (theuth runs both)
- **opencode** has a 3-rule screen manifest (version `2026.06.10.1`, aliases `open-code`, `herdr:opencode`): `permission_required` (blocked, priority 300, `visible_blocker`, `any` of the literal `△ Permission required` or the chrome combination `esc dismiss` + {`enter confirm`|`enter submit`|`enter toggle`} + {`↑↓ select`|`⇆ tab`}); `interrupt_hint_working` (working, 110: `esc to interrupt`, `ctrl+c to interrupt`, a line matching `(?i).*opencode.*esc (again to )?interrupt`); `progress_bar_working` (working, 100: four or more `■`/`⬝` glyphs) — `distribution/agent-detection/opencode.toml#L7-L37`. opencode *also* ships a plugin integration (`herdr:opencode`) which, once live, is **full-lifecycle authoritative** and the screen is only a fallback — `src/detect/mod.rs#L330-L340`.
- **omp has no screen manifest at all**: `Agent::Omp` is in `ALL` (24) but not in `SCREEN_MANIFEST_AGENTS` (22), and the docs table says "state requires the integration" — `src/detect/mod.rs#L71-L121`, `docs/next/website/src/content/docs/agents.mdx#L28`. Without the integration an omp pane is identified (label `omp`) but its state is the known-agent idle fallback. With the integration (a pi-style TypeScript extension, `herdr:omp`) state comes entirely from pi events: `agent_start` → working; `tool_approval_requested`/`_resolved` and the `ask` tool's `tool_execution_start`/`_end` → a blocked counter; `agent_end` → idle after a 250 ms debounce (`HERDR_OMP_IDLE_DEBOUNCE_MS`), unless the last assistant message stopped with a retryable provider error, in which case the pane is *held working* for 2.5 s (`HERDR_OMP_RETRY_GRACE_MS`) and then flips to blocked with the error text — `src/integration/assets/omp/herdr-agent-state.ts#L79-L82`, `src/integration/assets/omp/herdr-agent-state.ts#L281-L292`, `src/integration/assets/omp/herdr-agent-state.ts#L404-L464`. A nested omp (env `OMPCODE=1`) refuses to report so it cannot overwrite the pane's root session — `src/integration/assets/omp/herdr-agent-state.ts#L17-L24`.
- Claude, by contrast, is the richest manifest (16 rules, version `2026.09.11.1`, engine 2): OSC title spinner at priority 1100, `live_turn_working` on the bottom 12 non-empty lines, background-agent and MCP-task working rules with `not` gates for permission chrome, and `live_blocked_form` = `esc to cancel` + (`enter to confirm` | `enter to select` + a navigate hint) in `after_last_horizontal_rule` — `distribution/agent-detection/claude.toml#L7-L25`, `distribution/agent-detection/claude.toml#L79-L95`.

### The two richest manifests, rule by rule (for the theuth engineer who writes the first omp/claude PTY rules)
Claude (`distribution/agent-detection/claude.toml`, 16 rules, highest priority wins):

| priority | id | state | region | what it keys on |
|---|---|---|---|---|
| 1100 | `osc_title_working` | working | `osc_title` | braille / half-circle spinner glyph at the start of the title (`distribution/agent-detection/claude.toml#L7-L14`) |
| 1000 | `transcript_viewer` | unknown + `skip_state_update` | `bottom_non_empty_lines(3)` | "showing detailed transcript" + `ctrl+o`/`ctrl+e` hints (`distribution/agent-detection/claude.toml#L64-L77`) |
| 980 | `live_blocked_form` | blocked | `after_last_horizontal_rule` | `esc to cancel` + `enter to confirm` / `enter to select` + navigate hint (`distribution/agent-detection/claude.toml#L79-L95`) |
| 980 | `dynamic_workflow_prompt`, `mcp_elicitation_prompt` | blocked | `whole_recent` | "run a dynamic workflow?"; `MCP server "…" requests your input` + Accept/Decline (`distribution/agent-detection/claude.toml#L97-L123`) |
| 975 | `btw_overlay_working` | working | `bottom_non_empty_lines(5)` | `/btw` overlay with "esc to close" (`distribution/agent-detection/claude.toml#L53-L62`) |
| 970 | `live_turn_working` | working | `bottom_non_empty_lines(12)` | `⏸/⏵ … esc to interrupt` or spinner glyph + `…` + elapsed `(12s` (`distribution/agent-detection/claude.toml#L16-L25`) |
| 965 | `background_agents_working`, `background_mcp_task_working` | working | above prompt box / bottom 12 | "Waiting for N background agents", "N MCP tasks still running" with `not` gates for permission chrome (`distribution/agent-detection/claude.toml#L27-L51`) |
| 950 | `live_prompt_box` | idle + `visible_idle` | `prompt_box_body` | a line starting with `❯`, `not` any select/cancel/navigate hint (`distribution/agent-detection/claude.toml#L125-L138`) |
| 900 | `model_picker_menu` | unknown | `whole_recent` | menu chrome (`distribution/agent-detection/claude.toml#L140-L150`) |
| 850 / 840 | `bash_permission_prompt`, `generic_permission_prompt` | blocked | whole / after rule | the classic "Do you want to proceed?" shapes (`distribution/agent-detection/claude.toml#L152-L196`) |
| 300 | `legacy_no_prompt_blocker` | blocked | `whole_recent` | older permission UI (`distribution/agent-detection/claude.toml#L198-L216`) |
| 250 | `osc_title_idle`, `osc_progress_idle` | idle | `osc_title` / `osc_progress` | title starting `✳ `; OSC 9 payload `4;0` = progress cleared (`distribution/agent-detection/claude.toml#L218-L230`) |

Codex (`distribution/agent-detection/codex.toml`, 9 rules): `osc_title_blocked` 1100 and `osc_title_working` 1050 on the title, `transcript_viewer` 1000 (`after_last_prompt_marker`), `trust_directory` 950 on `top_non_empty_lines(20)`, `startup_update` 950 on the bottom 20, `live_strong_blocker` 900 after the last prompt marker, `weak_blocker` 600 on `whole_recent_without_current_prompt_marker`, `screen_working_fallback` 500 `before_current_prompt_marker`, `osc_title_idle` 100 (`distribution/agent-detection/codex.toml#L7-L112`). Two patterns recur across all 22 manifests: **the title/progress OSC channel outranks the screen** when the agent emits it, and **viewer/menu overlays are explicit `unknown` rules** rather than special cases in code.

### What `herdr agent explain --json` returns
`explain_to_json_value` serialises: `agent`, `state`, `source {kind: bundled|remote|local override, label}`, `matched_rule {id, priority, region, state}`, `screen_detection_skipped` (+ `screen_detection_skip_reason: full_lifecycle_hook_authority` when a hook owns the pane), `visible_idle/blocker/working`, `skip_state_update` + `skipped_update_reason`, `fallback_reason`, `manifest_version`, `cached_remote_version`, `local_override_shadowing_remote`, `remote_update_status`/`remote_update_error`, and `evaluated_rules[]` each with `matched` and `evidence {contains[], regex[], line_regex[], all_count, any_count, not_count, region_bytes, region_preview}` — `src/detect/manifest.rs#L821-L874`, `src/app/api/agents.rs#L332-L347`. The important property for theuth: *every* rule's evidence is returned, not just the winner, so a wrong state is debuggable from one JSON blob without re-running anything.

### Arbitration, debounce and hysteresis
- The arbiter is one struct and one formula: `state = Blocked if visible_blocker_overrides_hook() else hook_authority.state (if effective) else fallback_state` — `src/terminal/state.rs#L2431-L2439`. The file header says it plainly: "Full lifecycle Herdr hook integrations are hook-authoritative while live; screen recovery remains only for session-only/custom hook paths and fallback detection. Process-exit updates clear matching hook authority before recomputing state." — `src/terminal/state.rs#L5-L8`.
- "Full lifecycle" authority is a hard-coded allow-list of (source, agent) pairs: `herdr:pi`, `herdr:omp`, `herdr:mastracode`, `herdr:opencode`, `herdr:kilo`, `herdr:kimi`; `herdr:hermes`, `herdr:qwen`, `herdr:letta`, `herdr:antigravity_cli` are "session identity only" (they report a session ref, never state) — `src/detect/mod.rs#L330-L350`. Claude/Codex hooks are neither: they report state but the screen's `visible_blocker` may still override a non-blocked hook state, provided the screen observation is not older than the hook report — `src/terminal/state.rs#L2097-L2108`, `src/terminal/state.rs#L836-L841`.
- A hook authority is only *effective* while the detected foreground agent is the same agent and no recent process exit was observed — `src/terminal/state.rs#L2051-L2056`; while a full-lifecycle authority is live the detection task does not even scan the screen — `src/pane.rs#L3139-L3145`.
- Tick rates: 500 ms when no agent is identified, 300 ms when one is, 50 ms while a graceful release is pending — `src/pane.rs#L2851-L2853`; process re-probe every 5 s once identified, 30 s when there is no foreground group, an 8 s acquisition window with 500 ms rechecks for the first 1.5 s then 2 s — `src/pane.rs#L413-L420`.
- Hysteresis: a `Working → Idle` transition without visible idle chrome is held for up to three 100 ms confirmations or a 700 ms cap (`PendingIdleConfirmation`); a stable visible blocker is re-published every 800 ms; a newly identified agent gets a 3 s startup grace during which no screen state is published — `src/pane/agent_detection.rs#L5-L13`, `src/pane/agent_detection.rs#L39-L77`, `src/pane.rs#L3147-L3161`. Screen scans are skipped entirely when the content sequence has not changed since the last scan and the state is a stable Idle — `src/pane/agent_detection.rs#L91-L105`.
- A self-reported (custom-source) agent whose process herdr cannot identify is considered gone only when the pane's shell is back at its prompt, checked once per second — `src/pane.rs#L383-L411`.

### Explainability
`DetectionExplain` carries agent, final state, manifest source (bundled / remote:path / override), matched rule, every evaluated rule with its evidence (which `contains`/`regex`/`line_regex` hit, gate counts, region bytes and a preview), skipped-update reason, fallback reason, manifest and cached-remote versions, override-shadowing flag and the remote update status/error — `src/detect/manifest.rs#L27-L47`. `herdr agent explain <target> [--json|--verbose]` asks the running server (so it reflects the live cache) and `--file screen.txt --agent codex` evaluates offline — `src/cli/agent.rs#L41-L143`, docs `docs/next/website/src/content/docs/agents.mdx#L98-L103`.

### Remote catalog
- URL `https://herdr.dev/agent-detection/index.toml` (override via `HERDR_AGENT_DETECTION_MANIFEST_CATALOG_URL`), fetched with `curl -sfL --retry 2 --connect-timeout 5 --max-time 15`, capped at 256 KiB — `src/detect/manifest_update.rs#L15-L18`, `src/detect/manifest_update.rs#L509-L523`. The catalog is `schema_version = 1` plus `[[agents]] id/path` pairs (22 entries); unknown ids are skipped, absolute/`..`/URL paths and duplicates are rejected — `src/detect/manifest_update.rs#L349-L387`, `distribution/agent-detection/index.toml#L1-L10`.
- Cadence: one check at server start and then every 30 min (shared `AUTO_UPDATE_CHECK_INTERVAL`), disabled by `[update] manifest_check = false` — `src/app/mod.rs#L42`, `src/app/mod.rs#L587-L606`, `src/app/mod.rs#L645-L648`.
- Versioning instead of signatures: there is **no checksum or signature**; safety comes from (a) a remote manifest must carry `version` and `min_engine_version ≤ 3`, (b) a remote older than the bundled one is ignored with a warning, (c) a remote equal in version but different in content is rejected ("changed content without a version bump"), (d) per-agent status with `last_result ∈ {updated, current, failed}` and `last_error` written to `state/agent-detection/status.toml` — `src/detect/manifest.rs#L744-L810`, `src/detect/manifest_update.rs#L314-L347`. Files are committed with tmp-file + fsync + rename + parent-dir fsync — `src/detect/manifest_update.rs#L439-L471`.
- Precedence: local override `~/.config/herdr/agent-detection/<agent>.toml` always wins (if it compiles and its `id`/alias names that agent), else the newer of cached-remote and bundled; a broken override is ignored with a warning that surfaces in explain — `src/detect/manifest.rs#L589-L686`, `src/detect/manifest.rs#L1119-L1125`. Reload is in-process (`reload_manifests_for_agents`) after an update, and `herdr server reload-agent-manifests` / `update-agent-manifests` exist — `src/detect/manifest_update.rs#L168-L197`, docs `docs/next/website/src/content/docs/agents.mdx#L80-L96`.
- Limits of the mechanism, stated by the docs: remote manifests can only patch *rules* for agents the binary already knows; a new agent needs a binary release — `docs/next/website/src/content/docs/agents.mdx#L94`.

### Weaknesses
- The arbiter is 6.4 K lines in one file (`src/terminal/state.rs`, 103 unit tests) with suppression/stale-session/replacement-report machinery that exists mostly to survive hook reports arriving out of order or from the wrong process; the vocabulary (suppressed full-lifecycle report, reanchor sequence, stale session) is the complexity cost of mixing three evidence sources.
- Screen rules are brittle by construction: the CHANGELOG is full of chrome-chasing fixes (Claude's 2.1.228 spinner glyph change, "activity spinner … recognized correctly" `CHANGELOG.md#L112`, background MCP tasks `CHANGELOG.md#L161`), which is exactly why the remote catalog exists.
- `Blocked` is under-reported by policy: any new prompt shape is `idle` until a rule lands; the docs acknowledge this (`docs/next/website/src/content/docs/agents.mdx#L78`).
- No signature on the remote catalog; trust is TLS to herdr.dev plus monotonic versions.
- `Working` from PTY activity is asserted in a comment but the manifests still carry many `working` rules, so the two can disagree during spinners that do not emit bytes.
- Detection is per pane/shell; agents inside tmux or a VM are invisible unless `HERDR_AGENT` is set on a host-visible wrapper (`docs/next/website/src/content/docs/agents.mdx#L68-L72`, `docs/next/website/src/content/docs/agents.mdx#L105-L107`).

### Borrow candidates for theuth
1. **Positive-evidence blocked policy + typed fallback reason** — size **S**
   - lands in: crates/runtime `screen()`/job_idle and crates/acp attention
   - why: theuth's job_idle fires after a quiet minute with no notion of *why*; adopting the rule "blocked only on recognised chrome, otherwise idle-with-reason" and surfacing `fallback_reason` in the `<job_idle>` input gives Sky an honest signal instead of a guess.
2. **Per-agent TOML tail manifests with named regions and an explain command** — size **M**
   - lands in: crates/runtime: a `job_state(id)` / `explain` runtime function; data under `~/.theuth/agent-detection/`
   - why: Start with omp/opencode/claude PTYs; the region vocabulary (`bottom_non_empty_lines(n)`, `after_last_horizontal_rule`, `osc_title`) is the reusable idea, not the rules.
3. **Foreground-process-group identity before text** — size **S**
   - lands in: crates/runtime PTY jobs
   - why: theuth has the child pid of every PTY; `tcgetpgrp` on the master fd tells whether omp is still in front or the shell is back, which is a cheaper and more reliable "did it exit / is it at the prompt" than regexing the screen.
4. **Pending-idle confirmation (3 × 100 ms, 700 ms cap) and startup grace (3 s)** — size **S**
   - lands in: crates/runtime wait/idle
   - why: Prevents the false "done" between two tool calls that today makes `wait(pattern)` and idle detection flap.
5. **Hook-authoritative state when the agent can report** — size **M**
   - lands in: crates/acp + the design-75 MCP back-channel
   - why: For ACP children theuth already has structured events; the herdr lesson is to make them *authoritative* and treat screen/PTY heuristics as fallback only, with a per-(source, agent) allow-list rather than a generic merge.
6. **Remote-updatable rule catalogs with version monotonicity + content-change-without-bump rejection** — size **S**
   - lands in: crates/update or the knowledge bundle
   - why: Cheap to copy for any data table theuth ships (harness capability profiles, prompt-shape rules).

## A2 Agent integrations & session identity

**One line.** herdr installs a small file into each agent's own extension/hook mechanism (a pi/omp TypeScript extension, an OpenCode plugin, a `SessionStart` shell hook in Claude's `settings.json`, Codex hooks, …) that reads four environment variables every pane exports and calls back over the socket with `pane.report_agent` / `pane.report_agent_session` / `pane.release_agent`; the server turns the reported session id or path into an `AgentSessionRef`, persists it with the pane, and on restore types an agent-specific resume command into a fresh shell in the same directory — once per dedupe key.

### What every pane exports, and what the agent sends back
- Every process in a pane inherits `HERDR_ENV=1`, `HERDR_PANE_ID`, `HERDR_BIN_PATH` (the exact binary that owns the pane, so hooks never call "whichever herdr is first on PATH" — CHANGELOG `CHANGELOG.md#L226`) and `HERDR_SOCKET_PATH`; plus `HERDR_TAB_ID` / `HERDR_WORKSPACE_ID` — `src/integration/env.rs#L8-L33`, docs `docs/next/website/src/content/docs/add-herdr-support.mdx#L19-L27`.
- Three report kinds: **state** (`idle|working|blocked`, optional `--message`), **session** (`agent_session_id` and/or `agent_session_path`, plus `session_start_source ∈ {startup, resume, clear, compact, branch, new, fork, select}`), **release**. Each carries `source`, `agent`, and a monotonically increasing `seq`; late or out-of-order reports are dropped per source — `src/agent_resume.rs#L153-L161`, `src/terminal/state.rs#L1761-L1769`, `docs/next/website/src/content/docs/add-herdr-support.mdx#L28-L49`.
- The transport from the agent side is a one-shot newline-delimited JSON request on the Unix socket / named pipe with a 500 ms timeout and one 1.5 s retry (omp), or 500 ms single-shot (opencode, Claude's python snippet) — `src/integration/assets/omp/herdr-agent-state.ts#L28-L61`, `src/integration/assets/opencode/herdr-agent-state.js#L61-L103`, `src/integration/assets/claude/herdr-agent-state.sh#L88-L99`. Reports are queued and only the latest state is kept while one is in flight ("send only the latest state") — omp `src/integration/assets/omp/herdr-agent-state.ts#L182-L210`, docs `docs/next/website/src/content/docs/add-herdr-support.mdx#L93-L94`.

### Per agent (what is installed, what comes back)
| agent | installed where | mechanism | reports |
|---|---|---|---|
| **omp** | `~/.omp/agent/extensions/herdr-omp-agent-state.ts` (`PI_CODING_AGENT_DIR` / `PI_CONFIG_DIR` aware; refuses to install when pi and omp resolve to the same dir) | pi extension API: `session_start`, `session_switch`, `agent_start`, `agent_end`, `tool_approval_requested/resolved`, `tool_execution_start/end` for the `ask` tool, custom `herdr:blocked` event | full lifecycle state + session **path** (preferred) or id via `ctx.sessionManager.getSessionFile()/getSessionId()`; `session_start_source` from `session_switch.reason` — `src/integration/env.rs#L42-L56`, `src/integration/targets.rs#L89-L107`, omp asset `src/integration/assets/omp/herdr-agent-state.ts#L99-L123`, `src/integration/assets/omp/herdr-agent-state.ts#L375-L402` |
| **opencode** | V1 server plugin `herdr-agent-state.js`, V1 TUI plugin `herdr-tui-session.js` registered in `tui.jsonc`, V2 TUI entry `herdr-opencode/tui.js` registered in `cli.json` (registration deferred while a V1→V2 migration is pending) | opencode plugin events: `session.status` mapped by status word, `permission.asked`/`question.asked`/`session.error` → blocked, `session.idle` → idle, `tool.execute.*` → working; child sessions tracked by `parentID` so a subagent's permission prompt blocks the *root* session | full lifecycle + session id — `src/integration/assets/opencode/herdr-agent-state.js#L15-L24`, `src/integration/assets/opencode/herdr-agent-state.js#L37-L53`, `src/integration/assets/opencode/herdr-agent-state.js#L145-L216`; `src/integration/opencode_config.rs#L44-L74` |
| **Claude Code** | `~/.claude/settings.json` (`CLAUDE_CONFIG_DIR`): one `SessionStart` command hook with matcher `^(startup|resume|clear|compact|fork)$`, timeout 10 s; install *removes* herdr's older `PostToolUse`/`PermissionRequest`/`Stop`/`UserPromptSubmit`/`SessionEnd` state hooks | the `.sh` hook captures stdin JSON, exits unless `hook_event_name == SessionStart`, ignores Cursor's Claude-compatible events and subagents (`agent_id`), then sends `pane.report_agent_session` with `session_id` + `transcript_path` | **session identity only** — state is screen-detected — `src/integration/claude_settings.rs#L17-L82`, `src/integration/assets/claude/herdr-agent-state.sh#L15-L86` |
| Codex, Copilot, Devin, Droid, Kimi, Qoder, Cursor, Grok, Hermes, Qwen, Letta, Antigravity | per-agent hook/config files under `CODEX_HOME`, `COPILOT_HOME`, … | hooks or plugins | session identity (and, for Kimi/Kilo/Hermes, state) — `src/integration/env.rs#L12-L26`, `src/agent_resume.rs#L163-L175` |

The 0.3.0 → 0.6.7 history matters: herdr first shipped "authoritative hook-driven state reporting" for pi, Claude, Codex and OpenCode (`CHANGELOG.md#L1166`), then **walked Claude/Codex/Copilot/Droid/Kimi/Qoder back to session-identity-only** because their hook sets do not cover every transition ("Devin hooks do not cover every permission cancellation and user interrupt transition", "Copilot hooks do not provide complete lifecycle coverage") — `CHANGELOG.md#L650`, `CHANGELOG.md#L556`, `CHANGELOG.md#L646`. The rule that survived: an agent may own its state only if its event stream is complete (pi, omp, opencode, kilo, kimi, hermes-plugin-era, custom sources); otherwise it may only tell herdr *who it is*.

### How a pane learns the session id, and what an `AgentSessionRef` is
- `AgentSessionRef { kind: Id | Path, value }`; ids ≤ 512 chars, paths ≤ 4096 chars and absolute, no control chars — `src/agent_resume.rs#L5-L21`, `src/agent_resume.rs#L351-L360`. Only pi and omp may use a *path* (their session file); every other official source is id-only, and a non-official source gets no ref at all from `session_ref_from_report` — `src/agent_resume.rs#L116-L133`, `src/agent_resume.rs#L177-L196`.
- A report lands in `TerminalState::set_hook_authority_at` → `route_full_lifecycle_hook_report` (seq guard, stale-generation re-anchoring) → owner-conflict checks (a `herdr:codex` report naming a different session than the pane's persisted one is ignored; a different owner may take over only if the foreground process confirms it) — `src/terminal/state.rs#L701-L770`. A Codex pane launched as `codex resume <id>` gets its session from the launch argv without any hook — `src/agent_resume.rs#L135-L151`.
- The pane also keeps a `persisted_agent_session` after the hook authority is cleared (process exit), so the session survives the agent exiting and is still offered for resume — `src/terminal/state.rs#L619-L633`.

### How a hook report is admitted (`src/terminal/state.rs#L701-L770`, in order)
1. Identity-only sources (`herdr:hermes`, `herdr:qwen`, `herdr:letta`, `herdr:antigravity_cli`) never set state: return early.
2. A non-full-lifecycle source whose agent was just seen exiting is ignored (stale report after exit).
3. `route_full_lifecycle_hook_report`: the per-source `seq` must be newer; a report belonging to a previous agent generation is ignored unless it carries a fresh session ref or a process exit was proven, in which case the sequence guard is re-anchored (`CHANGELOG.md#L533`).
4. A known-agent label that contradicts the foreground-detected agent is ignored ("conflicting known-agent hook labels are ignored once native foreground-process detection identifies a different known agent", `CHANGELOG.md#L847`).
5. `herdr:codex` reports naming a session other than the pane's persisted one are dropped (Codex's launch-argv identity wins).
6. A different *owner* (source) may take the pane over only when the foreground process confirms the new agent.
7. The session ref is reconciled against the persisted one; then the authority is stored with `reported_at = now` and effective state is recomputed.

### Resume plans and the dedupe key
- `plan(source, agent, ref)` is a hard-coded table: `claude --resume <id>`, `codex resume <id>`, `copilot --resume=<id>`, `kimi --session <id>`, `pi --session <path|id>`, `omp --resume=<path|id>` (comment: "omp resume is `-r, --resume=<value>` … it has no `--session` flag, unlike pi"), `opencode --session <id>`, `kilo --session <id>`, `hermes --resume <id>`, `letta --conversation <id>`, … — `src/agent_resume.rs#L198-L318`.
- Agent-reported resume commands (`-- my-agent --resume … --model …`, herdr ≥ 0.9.2) are validated: ≤ 64 args, ≤ 8 KiB, no control chars, no apostrophes, first word a bare command name (not a path) because "restore types the command into the pane's shell, so … shells disagree on how to invoke a quoted path"; the source must already hold the pane or the answer is `resume_not_accepted` — `src/agent_resume.rs#L56-L89`, `docs/next/website/src/content/docs/add-herdr-support.mdx#L69-L76`.
- **Dedupe key** = `source \0 agent \0 kind \0 value` for native plans, and `source \0 agent \0 cwd \0 argv…` for reported commands — the directory is part of the identity because "the same command can name different sessions in different directories, for example `agent --continue`" — `src/agent_resume.rs#L38-L53`, `src/agent_resume.rs#L320-L325`. On restore the first pane to claim a key reserves it; later panes with the same key get no plan and are restored as plain shells (with saved history), so a session is never opened twice — `src/persist/restore.rs#L813-L863`.
- Execution: resume is deferred until the client has reported its terminal theme (750 ms max wait), then one pane per `startup_per_agent_delay_ms` (default 100 ms); each spawns a shell in the saved cwd and types the shell-quoted argv + `\r`; a missing directory or shell failure becomes a visible `restore_error` on the pane instead of a retry — `src/app/agent_resume.rs#L48-L97`, `src/app/agent_resume.rs#L226-L318`, `src/app/mod.rs#L43`, `src/config/model.rs#L270-L279`. When a native resume plan exists the saved presentation history is *not* replayed into that terminal ("native agent resume owns the conversation history") — `src/persist/restore.rs#L820-L830`.

### Resume plan table (`src/agent_resume.rs#L198-L318`)

| source → agent | ref kind | argv typed into the restored shell |
|---|---|---|
| `herdr:claude` → claude | id | `claude --resume <id>` |
| `herdr:codex` → codex | id | `codex resume <id>` |
| `herdr:copilot` → copilot | id | `copilot --resume=<id>` |
| `herdr:devin` / `herdr:droid` / `herdr:hermes` / `herdr:qodercli` / `herdr:qwen` | id | `<agent> --resume <id>` |
| `herdr:kimi` / `herdr:kilo` / `herdr:opencode` | id | `<agent> --session <id>` |
| `herdr:pi` → pi | **path** or id | `pi --session <path-or-id>` |
| `herdr:omp` → omp | **path** or id | `omp --resume=<path-or-id>` ("it has no `--session` flag, unlike pi") |
| `herdr:mastracode` | id | `mastracode --thread <id>` |
| `herdr:cursor` | id | `cursor-agent --resume <id>` (`cursor-agent.cmd` on Windows) |
| `herdr:letta` | id | `letta --conversation <id>` (or `--conversation default --agent <agent-id>`) |
| `herdr:antigravity_cli` → agy, `herdr:grok` | id | `agy --conversation <id>`, `grok --resume <id>` |
| any custom source | — | only the agent-reported `resume_argv` (`src/agent_resume.rs#L38-L53`) |

Everything else (`persisted_session_from_launch_args`, `session_ref_from_snapshot`) is the inverse mapping so a pane launched by hand as `codex resume <id>` or restored from an older snapshot still gets an identity — `src/agent_resume.rs#L135-L151`, `src/agent_resume.rs#L177-L196`.

### The "add herdr support" contract (quoted)
From `add-herdr-support.mdx`: "Only report when `HERDR_ENV=1` and the other variables are set. Outside Herdr, your integration should do nothing." (`docs/next/website/src/content/docs/add-herdr-support.mdx#L34`); "`--seq` … must increase with every report from your source, including across sessions and restarts of your agent. A timestamp works well. Herdr ignores reports whose number is not higher than the last one it accepted" (`docs/next/website/src/content/docs/add-herdr-support.mdx#L49`); "Don't start it with `herdr:`, because Herdr's own integrations use that prefix" (`docs/next/website/src/content/docs/add-herdr-support.mdx#L48`); "Releasing clears your agent's name, state, and resume command from the pane right away. Only release when the user actually quits. If your agent replaces one session with another in the same process, report the new session instead of releasing." (`docs/next/website/src/content/docs/add-herdr-support.mdx#L87`); "If your agent exits without releasing, Herdr notices once the pane is back at its idle shell prompt … It is a safety net, not a replacement for releasing." (`docs/next/website/src/content/docs/add-herdr-support.mdx#L89`); "Don't let Herdr slow your agent down. Send reports in the background or with a short timeout, and ignore failures. Send only the latest state." (`docs/next/website/src/content/docs/add-herdr-support.mdx#L93-L94`). Verification recipe: `herdr pane get`, `herdr agent list`, and a named test session stop/start — `docs/next/website/src/content/docs/add-herdr-support.mdx#L101-L114`.

### Installing, updating and removing safely
- Every managed asset starts with a header `HERDR_INTEGRATION_ID=<id>` / `HERDR_INTEGRATION_VERSION=<n>` ("managed by herdr; reinstalling or updating overwrites this file; add custom hooks beside this file") — omp asset `src/integration/assets/omp/herdr-agent-state.ts#L1-L5`, Claude `.sh` `src/integration/assets/claude/herdr-agent-state.sh#L2-L6`; `herdr integration status` compares versions and the docs list the minimum version per agent for native restore — `docs/next/website/src/content/docs/integrations.mdx#L64`.
- Claude's `settings.json` is edited with a **CST** (`jsonc_parser::cst`) so comments, ordering and formatting survive: parse strictly, reject duplicate keys, remove herdr's legacy entries by matching the command string variants, append the canonical hook, and verify the result parses back to the desired value — `src/integration/claude_settings.rs#L193-L257`, `src/integration/config_edit.rs#L60-L105`. OpenCode's `tui.jsonc`/`cli.json` use the same CST approach — `src/integration/opencode_config.rs#L1-L52`.
- Writes go through `config_file::write_config`: refuse hard-linked targets, resolve (even dangling) symlinks, open the existing file for write first so a writable parent cannot bypass file permissions, stage into a private temp file, replace atomically; on Windows an in-place backup-first update keeps ACLs (`.herdr-backup`, `.herdr-backup.pending`) — `src/integration/config_file.rs#L13-L80`, `src/integration/config_file.rs#L87-L135`, `docs/next/website/src/content/docs/integrations.mdx#L56`. Letta's install uses explicit stage → publish (rename original to backup, rename staged in) → rollback — `src/integration/targets.rs#L1066-L1148`.
- Uninstall removes only herdr's entries and files and keeps the user's other hooks/plugins — `src/integration/claude_settings.rs#L97-L126`.

### Weaknesses
- Eighteen (source, agent) pairs are hard-coded in four separate allow-lists (`is_official_agent_source`, `full_lifecycle_hook_authority`, `session_identity_only_integration`, `is_reserved_native_state_source`) plus the resume table — adding an agent touches all of them; the docs admit a new agent needs a binary release.
- Resume is "type a command into a shell": it depends on PATH, shell quoting rules (apostrophes banned outright), the agent's own CLI flags staying stable, and the saved cwd still existing; failures are surfaced but not retried.
- The hook snippets fork `python3` (Claude) or a Node socket per event; herdr mitigates with short timeouts, but every report is a process or connection.
- Session identity for Claude is only captured at `SessionStart` (startup/resume/clear/compact/fork); a session that changes id by another route is missed until the next matching event (`docs/next/website/src/content/docs/integrations.mdx#L114`).
- Ownership arbitration (owner conflicts, foreground-confirmed takeover, stale generations) is the largest source of CHANGELOG fixes in A2 (`CHANGELOG.md#L527`, `CHANGELOG.md#L533`, `CHANGELOG.md#L534`).

### Borrow candidates for theuth
1. **Session-identity-only vs full-lifecycle tiers per harness** — size **S**
   - lands in: crates/acp + crates/agent
   - why: classify each external harness by whether its event stream is complete; only complete ones may drive attention state, others only contribute a resume handle. This is the single most useful lesson from herdr's 0.3 → 0.6.7 reversal.
2. **`(source, agent, kind, value)` dedupe key with cwd for command-shaped resumes** — size **S**
   - lands in: crates/acp resume + durability 109
   - why: theuth's PTY resume recipe (`omp --resume <id>` in a worktree) has exactly the `--continue`-in-two-dirs ambiguity; make the worktree part of the identity and reserve before spawning.
3. **Env-var contract for children** — size **S**
   - lands in: crates/runtime `start()` + crates/acp spawn
   - why: export `THEUTH_ENV=1`, `THEUTH_JOB_ID`/`THEUTH_SESSION`, `THEUTH_BIN_PATH`, `THEUTH_SOCKET_PATH` into every PTY and ACP child so a hook or a wrapper script can report back without configuration; pair with the "do nothing outside theuth" rule.
4. **Monotonic `seq` per source with drop-older semantics** — size **S**
   - lands in: crates/acp SessionUpdate ingestion, attention inbox
   - why: cheap protection against out-of-order hook deliveries that today could flip an attention row back to "working".
5. **CST-preserving edits of foreign JSONC configs with header-versioned managed assets** — size **M**
   - lands in: crates/acp harness install / `~/.theuth/skills` materialisation
   - why: theuth already writes into Claude/opencode config dirs for skills and MCP; copying the hard-link/symlink/permission discipline of `config_file.rs` and the `HERDR_INTEGRATION_VERSION` header avoids the silent-overwrite class of bugs.
6. **Deferred, spaced resume with visible per-pane `restore_error`** — size **S**
   - lands in: durability 109 resume path
   - why: 100 ms spacing and "saved directory is unavailable" as a first-class row state rather than a log line.

## A3 Durability of running work

**One line.** One background server process per *session name* owns every PTY; clients (TUI, CLI, agents) are disposable socket connections; layout + agent session refs are snapshotted to `session.json` (debounced 5 s, atomic rename, 48 rotating layout snapshots); a restart reconstructs layout and *resumes* agents natively, and — the interesting part — `herdr update --handoff` replaces the server process without killing anything by shipping every PTY master fd over `SCM_RIGHTS` to a freshly spawned server, with a commit protocol and a rollback that restores the old server's sockets.

### Process model
- `herdr` with no subcommand: check the client socket, spawn a detached daemon if none answers, wait ≤ 15 s (50 ms polls) for the socket, then attach — `src/server/autodetect.rs#L1-L26`. Two sockets per session: the binary client protocol socket (`herdr-client.sock`, mode 0600) and the JSON API socket (`herdr.sock`); `--session <name>` or `HERDR_SESSION` selects a data dir and both paths; `HERDR_SOCKET_PATH` overrides the API socket and the client path is derived from it — `src/server/socket_paths.rs#L1-L58`.
- The server ignores SIGHUP from process start ("must outlive the terminal or shell that launched it, like tmux"; a handler rather than SIG_IGN so children keep normal hangup behaviour) and routes SIGINT/SIGTERM to a logged quit — `src/platform/unix_common.rs#L269-L317`; `server_survives_hangup_and_logs_why_it_stops` and `server_stop_request_logs_its_caller` prove it — `tests/server_headless.rs#L578-L615`.
- Headless means the server runs with no attached terminal: panes created without a client use a configured headless size, and output accumulated while detached is visible on reattach — `tests/detach_reattach.rs#L576-L744`.

### What is persisted and what comes back
- `SessionSnapshot { version: 3, workspaces[ { identity_cwd, worktree_space, tabs[ { layout (BSP tree), panes{ cwd, label, agent_name, managed_agent_kind, agent_session{source, agent, kind, value}, agent_resume{source, agent, argv}, launch_argv }, zoomed, focused } ], active_tab } ], active, selected, sidebar… }` — `src/persist/snapshot.rs#L11-L135`. Nothing about processes, scrollback or hook authority is in it.
- Written to `<data_dir>/session.json` via `json.tmp` + rename (symlink-aware); a separate `session-history.json` holds opt-in pane screen history (`[experimental] pane_history`, off by default "because pane output can include secrets") and is replayed only when the layout fingerprint matches — `src/persist/io.rs#L44-L60`, `docs/next/website/src/content/docs/session-state.mdx#L58-L71`.
- Cadence: `SESSION_SAVE_DEBOUNCE` 5 s after any change, plus pane-exit checkpoints and shutdown — `src/app/mod.rs#L44`, `src/persist/writer.rs#L8-L59`. Before overwriting, the previous layout is copied into `session-snapshots/` at most once per 15 min, ≤ 48 kept, unchanged layouts not duplicated, and the interval survives restarts so an exit burst cannot rotate history away; an unparseable or too-new `session.json` is preserved in `session-backups/` (3 kept) before anything replaces it — `src/persist/writer.rs#L78-L120`, `docs/next/website/src/content/docs/session-state.mdx#L39-L47`. Recovery is manual (copy a file back); there are no prompts.
- After a cold restart: layout, cwd, focus and labels return; every pane becomes a new shell in its saved cwd, except panes with a resumable agent session, which get the resume command typed in (A2); a missing directory keeps the pane in the layout with a `restore_error` instead of moving it to `$HOME` — `docs/next/website/src/content/docs/session-state.mdx#L29-L35`, `src/app/agent_resume.rs#L257-L264`. The docs table is explicit that processes do **not** survive a server restart — `docs/next/website/src/content/docs/session-state.mdx#L10-L15`.

#### What comes back, by restart kind
| | detach/reattach | server restart (cold) | `--handoff` replacement | engine crash |
|---|---|---|---|---|
| processes | yes | **no** | yes (fds moved) | no |
| layout, cwd, labels, agent names | yes | yes | yes | yes (≤ 5 s stale) |
| scrollback | live | only with `pane_history` | last 8 KiB of primary screen + live | only with `pane_history` |
| agent session / resume | n/a | native resume (A2) | hook authority + session carried | native resume |
| waits, subscriptions, in-flight requests | kept | lost | **lost by design** | lost |
| hook authority seq guards | kept | reset | carried (`HandoffAgentState`) | reset |

Sources: `docs/next/website/src/content/docs/session-state.mdx#L10-L15`, `src/handoff_runtime.rs#L4-L32`, `src/terminal/state.rs#L28-L34`.

### Live server replacement (`herdr update --handoff`, `herdr --remote … --handoff`)
Old server (`perform_live_handoff`, `src/server/headless/lifecycle.rs#L25-L224`):
1. Bind a private Unix socket `herdr-handoff-<pid>.sock` with a one-time token; disconnect all clients and reject pending connections — `src/server/handoff.rs#L59-L62`, `src/server/headless/lifecycle.rs#L40-L59`.
2. Pause every pane's PTY reader (2 s timeout) so no bytes are consumed on both sides; capture the normal `SessionSnapshot` — `src/server/headless/lifecycle.rs#L61-L78`.
3. Build a `HandoffRuntimeState` per pane: `child_pid, rows, cols, cell px, keyboard protocol flags/ansi, input_state, terminal_title, initial_history_ansi (≤ 8 KiB of primary-screen scrollback, none if the pane has an agent session or is on the alt screen), agent_state` (the live `HookAuthority` + its sequence + acquisition flag) — `src/handoff_runtime.rs#L4-L32`, `src/pane.rs#L2292-L2325`, `src/server/headless/lifecycle.rs#L80-L102`.
4. Spawn `<new exe> server --handoff-import <socket> <token>` detached (the new binary is passed as `import_exe` by the updater), dup every master fd, accept the child, validate the manifest (`HANDOFF_VERSION`, source/expected version and protocol), send the fds in batches of 64 per `SCM_RIGHTS` message, wait for `restored` — `src/server/handoff.rs#L20-L50`, `src/server/handoff.rs#L65-L108`, `src/server/headless/lifecycle.rs#L115-L173`.
5. Remove its own API and client socket files, wait ≤ 30 s for the child's `ready`, send `committed`, then drain its runtimes with `preserve_processes_on_drop = true` (PTY actor released, detection task aborted, nothing killed) and wait ≤ 500 ms for `owned`; finally exit without persisting (`persist_session = false`) — `src/server/headless/lifecycle.rs#L175-L231`, `src/pane.rs#L2254-L2267`.

New server (`run_handoff_import_server`, `src/server/headless/bootstrap.rs#L133-L199`): receive manifest + fds, rebuild each pane from `from_handoff_fd` (a fresh ghostty terminal seeded with title, input state, keyboard protocol and the replayed history, the master fd wrapped as `OwnedFd`, the *same child pid* kept) — `src/pane.rs#L2502-L2565`; report `restored`; wait for the old public sockets to disappear; bind the API/client sockets; report `ready`; wait for `committed`; `assume_handoff_ownership` + unpause readers; report `owned`.

#### The handoff wire protocol in one table
| step | old server | new server (`--handoff-import`) | timeout / limit | on failure |
|---|---|---|---|---|
| 0 | bind `herdr-handoff-<pid>.sock`, token `<pid>-<nanos>`; disconnect clients | — | — | `handoff_in_progress = false` |
| 1 | pause PTY readers; `persist::capture` snapshot | — | 2 s per reader | rollback (unpause) |
| 2 | spawn child `server --handoff-import <sock> <token>`; dup master fds | connect, send token | — | kill child, rollback |
| 3 | `accept_and_validate_on`: manifest `{version:1, source_version, source_protocol, expected_*, snapshot, panes[], api_window_title}` | validate expected version/protocol | `READY_TIMEOUT` 30 s | kill child, rollback (`live_handoff_bad_expected_protocol_rolls_back_old_server`) |
| 4 | `send_fds` in batches of 64 via `SCM_RIGHTS` | rebuild panes from fds (`from_handoff_fd`), `App::new_from_handoff`; send `restored` | — | kill child, rollback |
| 5 | remove own API + client socket files | wait for old public sockets to close (≤ 5 s); bind API + client; send `ready` | 30 s | wait for sockets to close, **re-bind own sockets**, rollback (`live_handoff_after_restored_failure_rolls_back_old_server`) |
| 6 | send `committed`; drain runtimes with `preserve_processes_on_drop`; wait `owned` | `assume_handoff_ownership`, unpause readers, send `owned` | 500 ms for `owned` | same as 5 |
| 7 | `finish_live_handoff_shutdown`: `persist_session = false`, exit | serves clients; first title sent is the carried override | — | — |

Sources: `src/server/handoff.rs#L20-L50`, `src/server/handoff.rs#L147-L226`, `src/server/headless/lifecycle.rs#L25-L231`, `src/server/headless/bootstrap.rs#L133-L199`.

What is deliberately not preserved, and why: "in-flight requests, waits, subscriptions, client sockets, or pane-to-pane messages; clients reconnect and retry those operations after replacement" — `src/handoff_runtime.rs#L6-L10`, `docs/next/website/src/content/docs/session-state.mdx#L123`. Only the primary screen's last 8 KiB travels; the new server re-renders from that plus live output. The child processes are *not* re-parented (the old server exits, so they become orphans of init); the new server detects their death through EOF on the master and `kill(pid, 0)` liveness checks rather than `waitpid` — `src/platform/macos.rs#L1183-L1193`.

Fallback: every step before `committed` calls `rollback_handoff_before_commit` (unpause readers, kill the import child, remove the handoff socket); if the child fails *after* the old sockets were removed, the old server waits for them to close and re-binds its own API/client listeners (`restore_public_sockets_after_failed_handoff`) so the session is never left headless — `src/server/headless/lifecycle.rs#L181-L212`, `src/server/headless/lifecycle.rs#L242-L284`. Proven by `live_handoff_bad_expected_protocol_rolls_back_old_server` and `live_handoff_after_restored_failure_rolls_back_old_server` (`tests/live_handoff.rs#L1959`, `tests/live_handoff.rs#L2118`), while `live_handoff_preserves_pane_process_io` runs `sh` loops with a `trap HUP` and asserts no HUP and continued I/O across the swap (`tests/live_handoff.rs#L989-L1048`), plus python/http servers, >64 panes, keyboard protocol, named-session sockets and hook-reported agent state (`tests/live_handoff.rs#L755`, `tests/live_handoff.rs#L1179`, `tests/live_handoff.rs#L1458`, `tests/live_handoff.rs#L1613`, `tests/live_handoff.rs#L1794`).

Clients: the TUI client is disconnected on purpose and reconnects through autodetect (socket wait ≤ 15 s); CLI callers get a disconnect and retry. Capability is advertised in `RuntimeStatus.capabilities.live_handoff` so the updater only offers handoff to servers that support it, and prompts to stop old servers otherwise — `src/update.rs#L849-L853`, `src/update.rs#L1186-L1227`. Handoff is Unix-only (`src/server/headless/lifecycle.rs#L233-L239`).

### Host shutdown per OS
- **Linux**: a logind `Inhibit("shutdown", "Herdr", "Save terminal workspace layout", "delay")` lock is taken at startup; on `PrepareForShutdown(true)` the server sets a flag, wakes the loop to save, and *holds the fd forever* ("logind caps the delay even if the server gets stuck"); the watcher reconnects with 1 s → 60 s backoff — `src/platform/linux/shutdown.rs#L7-L82`; test `host_shutdown_saves_layout_before_releasing_delay_lock` against a private D-Bus — `tests/host_shutdown.rs#L110`.
- **macOS**: no shutdown monitor (`monitor_host_shutdown` returns `None` off Linux); the server relies on SIGTERM and the 5 s autosave — `src/platform/mod.rs#L31-L42`.
- **Windows**: `GetSystemMetrics(SM_SHUTTINGDOWN)` is consulted when a child exits so a host shutdown is not mistaken for a pane failure — `src/platform/windows.rs#L171-L177`.

### Self-update
`herdr update` reads `https://herdr.dev/latest.json`, downloads the asset and refuses to proceed without a SHA-256 match — `src/update.rs#L25`, `src/update.rs#L408-L413`, `src/update.rs#L657-L664`. With `--handoff` it asks each running server (via the API, A4) to hand off to the new binary; without it, protocol-compatible servers keep running and only "restart-required" servers prompt for a stop ("This stops active pane processes, including shells, dev servers, and tests") — `src/update.rs#L1186-L1227`, `src/update.rs#L1243-L1269`, `docs/next/website/src/content/docs/session-state.mdx#L117-L136`. Package-manager installs (brew, mise, nix) cannot handoff.

### Crash behaviour
A server crash is a cold restart: children die with their PTYs (the server is their parent and no keeper process exists), `session.json` is at most 5 s stale, and agents come back only through native resume. The handoff path is the only mechanism that keeps processes alive across a server boundary, and it needs a cooperative old server.

### Which test proves what (A3)
- Processes and I/O survive replacement, no SIGHUP reaches children: `tests/live_handoff.rs#L989` (`live_handoff_preserves_pane_process_io`), `tests/live_handoff.rs#L1794` (python http server), `tests/live_handoff.rs#L1866` (servers across multiple sessions).
- > 64 panes in one handoff (SCM_RIGHTS batching): `tests/live_handoff.rs#L755`.
- Named-session socket paths and leaked env do not break import: `tests/live_handoff.rs#L827`, `tests/live_handoff.rs#L860`, `tests/live_handoff.rs#L907`.
- Keyboard protocol / modifyOtherKeys state carried: `tests/live_handoff.rs#L1179`, `tests/live_handoff.rs#L1275`; plugins carried: `tests/live_handoff.rs#L942`; hook-reported agent state and names carried: `tests/live_handoff.rs#L1458`, `tests/live_handoff.rs#L1613`, `tests/live_handoff.rs#L1727`.
- Rollback paths: bad expected protocol `tests/live_handoff.rs#L1959`; failure after `restored` `tests/live_handoff.rs#L2118`; unknown pane exit during handoff still persists the session `tests/live_handoff.rs#L665`; one master fd per pane invariant `tests/live_handoff.rs#L583`.
- Detach semantics: `tests/detach_reattach.rs#L286-L744` (clean disconnect, state on reattach, processes survive, headless sizes, output accumulated while detached).
- Server lifecycle: `tests/server_headless.rs#L162-L615` (both sockets, stale socket cleanup, duplicate start fails gracefully, handshake version rejection, no-hello close in 5 s, SIGHUP survival, stop-request logging).
- Host shutdown: `tests/host_shutdown.rs#L110` against a private D-Bus login1 stub; restore under blocked git metadata: `tests/session_restore.rs#L57`.

### Weaknesses
- No keeper/supervisor: processes are only as durable as the server process; a panic or OOM-kill loses every shell (docs admit it, `docs/next/website/src/content/docs/session-state.mdx#L13`).
- Handoff is opt-in, experimental, Unix-only, cooperative (old server must be ≥ the version that batches fds), and drops waits/subscriptions — an agent blocked in `herdr agent wait` across an update must retry.
- The 8 KiB replay means the new server's scrollback starts almost empty unless `pane_history` is on.
- macOS has no shutdown hook; Windows only classifies exits.
- `session.json` restore re-creates shells by typing commands; nothing re-creates non-agent processes (dev servers, tests).

### Borrow candidates for theuth
1. **fd handoff for PTY jobs on engine restart** — size **L**
   - lands in: crates/runtime + crates/update + durability 109
   - why: the exact pain in the brief — engine restarts kill PTY jobs and ACP children. The herdr protocol (pause readers → snapshot → spawn `--handoff-import` with token → SCM_RIGHTS in batches → restored/ready/committed/owned → old exits) maps onto `t --headless` replacing itself; ACP children are stdio pipes, which are fds too. Keep herdr's "transient coordination is not preserved; callers retry" rule so the scope stays tractable.
2. **Rollback that re-binds the old sockets** — size **M**
   - lands in: same seam
   - why: the non-obvious half of handoff; without it a failed import leaves no engine listening.
3. **Layout/session snapshot rotation with an interval that survives restarts + failed-load backups** — size **S**
   - lands in: crates/agent sessions.sqlite or `~/.theuth/state`
   - why: cheap insurance against the "restart burst wipes everything" failure.
4. **logind delay inhibitor on Linux hosts and sandbox VMs** — size **S**
   - lands in: crates/nodes / crates/sandbox agents
   - why: save state before the VM or Linux node goes down; hold the fd until saved.
5. **Explicit "what survives" matrix in the docs** — size **S**
   - lands in: docs/design 109 as-built
   - why: the four-row table in `docs/next/website/src/content/docs/session-state.mdx#L10-L15` is the clearest statement of durability guarantees in any of the recon'd projects; theuth's users (and its own agents) need the same table for engine restart vs host reboot vs update.
6. **Capability-gated update actions** — size **S**
   - lands in: crates/update
   - why: advertise `capabilities.live_handoff` in status and let the updater choose per server, rather than one global behaviour.

## A4 Agent-native control API

**One line.** One newline-delimited JSON request per connection on a 0600 Unix socket (named pipe with an owner-only DACL on Windows), 102 methods named `noun.verb` behind a `schemars`-typed `Method` enum, a 512-event in-memory hub that feeds `events.subscribe` streams and the two blocking waits (`pane.wait_for_output`, `agent.wait`), an `agent.prompt --wait` that distinguishes "my prompt landed" from "some unrelated state change", a JSON Schema artifact regenerated by a test, and a 214-line skill that teaches agents to use the CLI — never the socket — and only when `HERDR_ENV=1`.

### Transport and framing
- "Herdr uses newline-delimited JSON over a local socket. On Unix, that socket is a Unix domain socket. On Windows, it is a named pipe. Send one request per line … A successful response includes the same `id`" — `docs/next/website/src/content/docs/socket-api.mdx#L606-L628`. Request = `{ id, method, params }` with `#[serde(tag = "method", content = "params")]` over the `Method` enum — `src/api/schema.rs#L35-L47`.
- Server side: a thread per connection, initial request line ≤ 1 MiB within 5 s, app dispatch timeout 5 s, 100 ms connection poll while waiting, 5 s write timeout — `src/api/server.rs#L29-L35`. The connection closes after one response unless it became a subscription or a wait.
- Auth = filesystem: socket file mode 0600 and the data dir; Windows pipes are created with SDDL `D:P(A;;GA;;;SY)(A;;GA;;;OW)` (SYSTEM + owner only) — `src/api/server.rs#L29`, `src/ipc.rs#L136-L157`, `tests/windows_server_security.rs`. No tokens, no peer-cred check beyond that: anything running as the user can drive herdr, which is the intended model (the agents *are* the user's processes).
- Socket resolution order: `--session <name>` → `HERDR_SOCKET_PATH` → `HERDR_SESSION` → default `~/.config/herdr/herdr.sock`; named sessions under `sessions/<name>/` — `docs/next/website/src/content/docs/socket-api.mdx#L630-L652`.

### One request's life
1. Client connects (`ApiClient::connect`) and writes one JSON line; the server thread reads it within 5 s / 1 MiB — `src/api/client.rs#L50-L60`, `src/api/server.rs#L29-L35`.
2. The line is parsed into `Request { id, method }`; an unparsable line gets an error with an empty `id` — `src/api/schema.rs#L35-L47`, `docs/next/website/src/content/docs/socket-api.mdx#L623-L626`.
3. Ordinary methods are dispatched to the app thread over an mpsc channel and answered within `APP_RESPONSE_TIMEOUT` (5 s) with `{ id, result: { type, … } }`; UI-affecting ones mark the UI dirty (`request_changes_ui`).
4. `pane.wait_for_output`, `agent.wait`, `agent.prompt --wait` and `events.wait` keep the thread and the connection alive, polling every 100 ms and watching `should_stop_connection` so a disconnecting client ends the wait — `src/api/wait.rs#L53-L57`.
5. `events.subscribe` answers once (`subscription_started`) then streams envelopes until the client closes or `events_lost` — `docs/next/website/src/content/docs/socket-api.mdx#L787-L799`.

### Methods by noun (102 total, `src/api/schema.rs#L47-L251`)
- `server.*`: stop, live_handoff, reload_config, ssh_agent.register, agent_manifests, reload_agent_manifests; `ping` returns version + capabilities (`live_handoff`, `detached_server_daemon`, `endpoint_protocol_generation`, `surface_interest`, `health_check`) — `src/api/server.rs#L70-L79`.
- `workspace.*` (create/list/get/focus/rename/move/move_block/report_metadata/close), `tab.*`, `worktree.*` (list/create/open/remove), `layout.*` (export/apply/set_split_ratio), `session.snapshot`.
- `pane.*`: split/swap/move/zoom/layout/process_info/neighbor/edges/focus_direction/resize/scroll/clear/edit_scrollback/selection.read/copy_motion/copy_search/list/current/get/focus/input.set/link.*/rename/**send_text/send_keys/send_input/read**/**report_agent/report_agent_session/report_metadata/clear_agent_authority/release_agent**/close/**wait_for_output**.
- `agent.*`: list/get/**read**/**explain**/send_keys/rename/view.set/view.clear/focus/**start**/**prompt**/**wait**.
- `events.subscribe`, `events.wait`; `integration.*`; `plugin.*`; `notification.show`; `command.invoke`; `client.window_title.*`.
- Pane reads have four sources — `visible`, `recent`, `recent-unwrapped` (soft wraps joined, "prefer it for logs"), `detection` (the exact bottom-buffer text the manifests see) — plus `--format ansi|text` and a `--lines` that can pull application-owned history for idle agents — `skills/herdr/SKILL.md#L191-L200`.

### Events and subscriptions
- `EventHub` is a mutex-guarded `Vec` of the last **512** envelopes with a monotonically increasing sequence; readers poll `events_after(seq)`; a reader whose cursor fell behind the oldest retained sequence gets `EventHistoryError::Lost` — `src/api/event_hub.rs#L1-L60`. The connection then receives `error.code: "events_lost"` and is closed; the docs tell clients to resubscribe, take `session.snapshot` on another connection and treat events as invalidation signals, because "Event history is not durable" and "Snapshots and events have no shared sequence boundary" — `src/api/subscriptions.rs#L332`, `docs/next/website/src/content/docs/socket-api.mdx#L794-L815`.
- 30 event kinds (`src/api/schema/events.rs#L194`): workspace/tab/pane lifecycle, `pane.agent_detected`, `pane.agent_status_changed`, `pane.output_matched`, `pane.scroll_changed`, `layout.updated`, worktree events — `docs/next/website/src/content/docs/socket-api.mdx#L817-L839`. A subscribe request with any unknown pane is rejected whole ("refresh the pane list and retry rather than assuming the other entries were subscribed") — `docs/next/website/src/content/docs/socket-api.mdx#L788-L790`.

### Waits
- `pane.wait_for_output { pane_id, match: substring|regex, source, lines, timeout_ms }`: a polling loop (100 ms) that re-reads the pane with `intent: Passive` and returns `OutputMatched { revision, matched_line, read }` or `timeout`; the first read happens immediately, so text already on screen matches — `src/api/wait.rs#L22-L130`, `skills/herdr/SKILL.md#L189`.
- `agent.wait { target, until[], timeout_ms }`: resolve the agent, return at once if the status already matches, else replay the hub from the sequence captured *before* the first read and probe on every relevant event; the wait ends `agent_not_running` if the pane closes/exits/moves or a different agent appears, `timeout` otherwise — `src/api/wait.rs#L132-L175`, `src/api/wait.rs#L364-L514`. Status vocabulary: `idle | working | blocked | done | unknown`; `done` = idle-and-not-yet-seen, decided server-side by focus commands, not reads — `src/api/schema/common.rs#L160-L166`, `skills/herdr/SKILL.md#L59`.
- "Genuinely blocked" is not a separate API flag; it is the composition of A1's strict policy with `until: blocked`: the wait only fires on the arbitrated `Blocked` state, which for screen agents requires positive chrome evidence and for hook agents a `tool_approval_requested`/`ask` report. `agent.prompt` refuses to type into an already-blocked agent (`agent_blocked`) "before sending any input. Inspect the blocked UI and ask the user before answering it." — `skills/herdr/SKILL.md#L145`.
- **Agent-to-agent prompting and knowing it landed** (`prompt_agent`, `src/api/wait.rs#L177-L322`): capture the target's status, submit text + encoded Enter as one ordered write (bracketed-paste aware), verify the responder is still the same terminal/name/agent, then — unless the agent was already working — run a first wait for *activity* (`working | blocked`) gated on `after_state_change_seq` so a stale status cannot satisfy it, bounded by `AGENT_PROMPT_EFFECT_TIMEOUT_MS = 5_000` and reported as `agent_prompt_stalled` (or `timeout` if the caller's budget is smaller); only after activity does the second wait for the settled `until` set run, replayed from before submission so a fast turn is not missed — `src/api/wait.rs#L20`, `src/api/wait.rs#L244-L321`. The skill is honest about the limit: "This wait tracks lifecycle state, not an individual turn; if the agent is already working, completion of the active turn may satisfy it" and "A timeout or stalled response does not prove the prompt was never delivered; do not blindly submit it again" — `skills/herdr/SKILL.md#L147`, `skills/herdr/SKILL.md#L171`.
- `agent.start { name, kind, pane, args }` returns only when the expected agent is detected in that pane and idle; `agent_not_ready` if it is blocked during startup; 30 s default, 3–300 s allowed — `docs/next/website/src/content/docs/agent-automation.mdx#L46`.

### Error vocabulary agents are expected to branch on
`agent_blocked` (prompt refused, agent already at a dialog), `agent_prompt_stalled` (no activity within 5 s of submission), `agent_not_running` (identity changed, pane closed/exited/moved mid-wait), `agent_not_ready` (blocked during `agent.start`), `agent_not_found`, `agent_name_taken`, `agent_launch_pending`, `agent_pane_unavailable`, `agent_prompt_failed`, `agent_start_input_failed`, `timeout`, `invalid_regex`, `events_lost`, `pane_not_found`, `pane_layout_unavailable`, `pane_send_failed`, `stale_content`, `stale_target`/`stale_pane_target`, `workspace_not_found`, `workspace_group_close_required`, `resume_not_accepted`, `invalid_resume_argv`, `unknown_method`, `invalid_request`, `server_unavailable`, `internal_error` — collected from `src/api/*.rs` and `src/app/api/*.rs` string literals; every one is a `{ id, error: { code, message } }` line. The distinction that matters most for theuth is `agent_prompt_stalled` ≠ `timeout` ≠ `agent_not_running`: three different next actions (inspect/re-send, wait longer, re-resolve the target).

### Schema generation, versioning, compatibility
- Every request/response/event type derives `schemars::JsonSchema`; a unit test builds the bundle (requests, success/error responses, events, subscription events, no dangling `$ref`s) and diffs it against `docs/next/api/herdr-api.schema.json` (10,759 lines); `HERDR_UPDATE_API_SCHEMA=1 just test-one generated_protocol_schema_artifact_is_current` rewrites it, otherwise CI fails with "generated API schema artifact is stale" — `src/api/schema/tests.rs#L182-L207`. `herdr api schema [--json|--output]` prints the copy bundled in the binary — `docs/next/website/src/content/docs/socket-api.mdx#L20-L34`.
- Compatibility rules are stated, not implied: "JSON API clients should ignore unknown fields and handle unsupported methods as normal errors"; the TUI's binary protocol is separately numbered (`PROTOCOL_VERSION = 22`) behind a stable `ENDPOINT_PROTOCOL_GENERATION = 1` negotiated at connect, and capabilities are advertised per server so a missing method "disables only that action" — `docs/next/website/src/content/docs/socket-api.mdx#L940-L956`, `src/protocol/wire.rs#L20`, `src/protocol/endpoint.rs#L15`.

### The CLI as a thin client
`ApiClient` connects, writes one line, reads one line (`src/api/client.rs#L31-L60`); every `herdr <noun> <verb>` maps to one method, prints the JSON result on stdout, JSON errors on stderr with exit 1 (syntax errors exit 2) — `skills/herdr/SKILL.md#L214`. The waits are the one place the CLI is more than a pass-through: the server keeps the connection open and the CLI blocks on it. `herdr agent explain` has an offline `--file` mode that evaluates locally without a server — `src/cli/agent.rs#L41-L143`.

### What the skill teaches (quoted)
`skills/herdr/SKILL.md`: front matter restricts use to "only when the user explicitly mentions Herdr … Do not use merely because a task could benefit from a background terminal, delegation, or parallel work. Requires HERDR_ENV=1" (`skills/herdr/SKILL.md#L1-L4`); first instruction `test "${HERDR_ENV:-}" = 1` — "If the check fails, say that you are not running inside Herdr and stop" (`skills/herdr/SKILL.md#L10-L16`); "The installed binary is the authority for command syntax. Start with `herdr --help`" and "Do not run bare `herdr` for discovery; it launches or attaches the TUI. Do not probe a mutating nested command by omitting arguments" (`skills/herdr/SKILL.md#L20-L43`); "Most control commands return JSON. Read identifiers and state from those responses instead of predicting them" (`skills/herdr/SKILL.md#L45`); topology rule "Default to a sibling pane in the current tab and the current working directory … Split a wide pane to the right and a narrow or tall pane down … `--no-focus`" (`skills/herdr/SKILL.md#L107-L123`); "`idle` and `done` both mean the agent is ready for input … `unknown` means an agent is present but Herdr cannot classify it confidently; it does not prove completion" (`skills/herdr/SKILL.md#L59`); safety list: never close what you did not create, never `server stop` or kill the main process, check `herdr status` because client and server versions can differ (`skills/herdr/SKILL.md#L204-L214`). Installation: `npx skills add herdrdev/herdr --skill herdr -g`, or `herdr --skill` prints the release-matched copy; a separate `herdr.dev/agent-guide.md` exists for an agent *teaching a human* — `docs/next/website/src/content/docs/agent-skill.mdx#L6-L44`.

### Weaknesses
- Polling everywhere: waits sleep 100 ms and re-issue `pane.read`/`agent.get`; a few hundred concurrent waits would be a few thousand app round-trips per second.
- 512-event history with `events_lost` + reconcile is correct but pushes real complexity onto every subscriber; there is no durable cursor.
- Prompting is "type text into a TTY": `agent.prompt` cannot know a turn's result, only lifecycle; the agent's answer must be scraped with `agent read` or a file hand-off (`skills/herdr/SKILL.md#L202`).
- Filesystem-only auth means any local process (or a prompt-injected agent in a pane) can drive every other pane; the skill's `HERDR_ENV` guard is advisory.
- Live handoff drops in-flight waits (A3), so long waits across an update return errors the caller must handle.

### Borrow candidates for theuth
1. **Prompt-effect gate (5 s activity wait keyed on a state-change sequence) for `send()`/ACP prompts** — size **S**
   - lands in: crates/runtime `send`/`wait` + crates/acp `send_message`
   - why: theuth's omp PTYs "never wake you" and a `send` that landed on a dead prompt is indistinguishable from a slow one; `agent_prompt_stalled` vs `timeout` is exactly the signal the orchestrator needs.
2. **`until`-state waits over an arbitrated agent state, with `agent_not_running` on identity change** — size **M**
   - lands in: crates/runtime: `wait(id, until="blocked")` alongside the pattern wait; crates/agent attention
   - why: Sky missing a blocked child is a missing primitive, not a missing notification — a wait that returns on the *state* `blocked` lets the orchestrator block on it.
3. **`detection`/`recent-unwrapped` read sources on `screen()`** — size **S**
   - lands in: crates/runtime
   - why: soft-wrap-joined text and the exact tail used for classification make log reading and rule debugging far cheaper than the rendered grid.
4. **Schema artifact diff test + `t api schema`** — size **S**
   - lands in: crates/rpc
   - why: theuth already ships ts-rs bindings; adding a committed JSON Schema regenerated by a failing test gives external harnesses and the companion a contract without reading Rust.
5. **Stated compat rules + per-server capability flags** — size **S**
   - lands in: crates/rpc + crates/mesh
   - why: "ignore unknown fields, unsupported method = normal error, capabilities advertised" is what the mesh needs across engine versions on different nodes.
6. **Skill shape** — size **S**
   - lands in: `~/.theuth/skills` + crates/acp skill materialisation
   - why: a skill that starts with an environment guard, tells the agent to learn syntax from `--help` rather than the skill, forbids discovery via mutating defaults, and lists what never to close/kill — theuth's subagent briefs would benefit from the same four sections.

## Numbers

| subsystem | LOC (Rust, incl. tests) | notes |
|---|---|---|
| `src/detect/` | 5,515 | mod 2,182 · manifest 1,536 · manifest_update 1,012 · manifest/tests 785 |
| `src/pane/` (+ `pane.rs`) | 13,007 + 6,821 | agent_detection 707 · background_agent 281 · osc 1,477 · terminal 7,625 |
| `src/terminal/` | 9,099 | state.rs 6,439 = the arbiter, 103 unit tests |
| `src/integration/` | 11,692 | targets 1,791 · tests 4,758 · claude_settings 859 · config_edit 911 · opencode_config 509 |
| integration assets | omp 471 TS · opencode 224+604 JS · claude 100 sh + 52 ps1 | header-versioned (`HERDR_INTEGRATION_VERSION` omp 10, claude 10, opencode 13) |
| `src/agent_resume.rs` + `src/app/agent_resume.rs` | 953 + 970 | 18 official (source, agent) pairs; 6 full-lifecycle; 4 identity-only |
| `src/persist/` | 5,139 | snapshot 1,466 · restore 2,230 · writer 676 · io 316 |
| `src/server/` | 29,504 | headless 3,434 + headless/ 4,888 · client_transport 2,435 · handoff 577 · autodetect 684 |
| `src/api/` + `src/app/api/` | 9,268 + 18,717 | server 1,953 · wait 854 · subscriptions 1,014 · schema/ ≈ 3,200 |
| `src/cli/` + `cli.rs` | 10,460 + 1,191 | thin client |
| `src/update.rs` | 4,021 | |
| `src/platform/` | 14,715 | windows 5,458 · macos 1,415 |
| `docs/next/api/herdr-api.schema.json` | 10,759 lines | regenerated by a test |
| `skills/herdr/SKILL.md` | 214 lines | |

Manifests: 22 bundled (`distribution/agent-detection/*.toml`, 1,864 lines incl. index), rule counts claude 16 · grok 13 · letta 10 · qwen 10 · codex 9 · hermes 9 · kiro 8 · … · opencode 3 · pi 2 · gemini 2 · kilo 2; omp 0 (no manifest). Catalog: 22 entries, `schema_version = 1`, engine version 3.

Timeouts and intervals (all from constants, nothing measured):

| mechanism | value | where |
|---|---|---|
| detection tick, agent identified / not / pending release | 300 ms / 500 ms / 50 ms | `src/pane.rs#L2851-L2853` |
| foreground process re-probe (identified / no fg group) | 5 s / 30 s | `src/pane.rs#L413-L415` |
| process acquisition window; fast window / recheck; slow recheck; idle reset | 8 s; 1.5 s / 500 ms; 2 s; 2 s | `src/pane.rs#L416-L420` |
| pending-idle confirmations / cap; stable-blocker refresh; startup grace | 3 × 100 ms / 700 ms; 800 ms; 3 s | `src/pane/agent_detection.rs#L5-L13` |
| self-reported-agent shell recheck; release re-acquire suppression | 1 s; 1 s | `src/pane.rs#L384`, `src/pane.rs#L69` |
| omp extension idle debounce / retry grace / socket attempts | 250 ms / 2.5 s / 500 ms + 1.5 s | `src/integration/assets/omp/herdr-agent-state.ts#L57-L61`, `src/integration/assets/omp/herdr-agent-state.ts#L79-L80` |
| opencode plugin socket timeout; Claude hook socket timeout / hook timeout | 500 ms; 500 ms / 10 s | `src/integration/assets/opencode/herdr-agent-state.js#L97`, `src/integration/assets/claude/herdr-agent-state.sh#L90`, `src/integration/claude_settings.rs#L76-L82` |
| manifest catalog check; curl connect / max-time / size | 30 min (and at start); 5 s / 15 s / 256 KiB | `src/app/mod.rs#L42`, `src/detect/manifest_update.rs#L18`, `src/detect/manifest_update.rs#L509-L523` |
| session save debounce; snapshot rotation interval / kept; failed-load backups | 5 s; 15 min / 48; 3 | `src/app/mod.rs#L44`, `src/persist/writer.rs#L78-L79`, `docs/next/website/src/content/docs/session-state.mdx#L41` |
| resume: theme wait; per-agent spacing; agent start default / bounds | 750 ms; 100 ms; 30 s / 3–300 s | `src/app/mod.rs#L43`, `src/config/model.rs#L279`, `docs/next/website/src/content/docs/agent-automation.mdx#L46` |
| handoff ready/commit; owned ack; reader pause; fds per message; replay per pane | 30 s; 500 ms; 2 s; 64; 8 KiB | `src/server/handoff.rs#L20-L33`, `src/server/headless/lifecycle.rs#L64` |
| autodetect server-ready / poll; status probe | 15 s / 50 ms; 2 s | `src/server/autodetect.rs#L18-L26` |
| API initial request / size; app dispatch; write; connection poll; accept backoff | 5 s / 1 MiB; 5 s; 5 s; 100 ms; 50 ms | `src/api/server.rs#L29-L35` |
| prompt-effect gate; event hub retention | 5 s; 512 events | `src/api/wait.rs#L20`, `src/api/event_hub.rs#L19` |

Tests (`#[test]`/`#[tokio::test]` counts): detect/mod 55 · manifest/tests 22 · manifest_update 12 · pane/agent_detection 14 · background_agent 10 · terminal/state 103 · integration/tests (file 4,758 lines) · tests/live_handoff 19 · detach_reattach 8 · server_headless 13 · host_shutdown 1 · session_restore 1 · auto_detect 9 · api_ping 21 · client_mode 21 · machine_api 16 · machine_setup 11 · multi_client 4 · session_delete 6 · cross_area 5 · broken_pipe 3 · windows_server_security 1.

## Do-not-borrow

- **The 6.4 K-line hook/screen arbiter as a whole** (`src/terminal/state.rs`). Its suppression/stale-session/replacement-report machinery exists because three evidence sources (process, screen, hooks from possibly-wrong processes) are merged for ~20 agents herdr does not control. theuth's ACP children have one authoritative stream; borrow the *tiers* and the *formula*, not the state machine.
- **Screen-regex manifests as the primary state source for ACP harnesses.** For omp/opencode theuth already has structured events; manifests belong only to the PTY-job fallback path.
- **Typing resume commands into a shell.** The apostrophe ban, PATH dependence and cwd-must-exist rules are consequences of that choice; theuth resumes ACP sessions by `session/load` and PTY omps by a known argv — keep that, add the dedupe key.
- **Polling waits at 100 ms over an in-process mutex `Vec`.** theuth's event bus already has backlog + lag handling (design 129); keep the `events_lost` semantics, not the implementation.
- **`python3`-forking shell hooks** for every event (Claude asset). If theuth needs a callback from a foreign agent, prefer a long-lived socket from an extension/plugin or an MCP back-channel.
- **Unsigned remote rule catalogs** without at least a checksum in the index.
- **Filesystem-only socket auth** for anything that reaches beyond the local user (theuth's RPC is already token-bearing; do not weaken it to match).
- **One `session.json` plus 48 copies** as the durability store; theuth's sessions.sqlite journal (109) is strictly stronger — borrow only the rotation-interval-survives-restart idea.

## Open questions

1. How does the imported server learn that an orphaned child exited when the master fd stays open because a grandchild still holds the slave? The code relies on EOF plus `kill(pid, 0)`; whether a stuck `sh -c` keeping the slave open delays the "exited" state after handoff was not traced (`src/pane.rs#L2502-L2565`, `src/platform/macos.rs#L1183-L1193`).
2. Where exactly "PTY activity is the normal working authority" is implemented: the comment is in `src/detect/mod.rs#L35-L38`, but the working-from-bytes path was not located in `pane.rs`; the `visible_working` flag is ignored by `set_detected_state_with_screen_signals_at` (`src/terminal/state.rs#L364-L365`). Worth one targeted grep by whoever builds the theuth equivalent.
3. The pending `[Unreleased]` section of `CHANGELOG.md#L3` and the `HERDR_PROCESS_DETECTION=child-groups` fallback (`agents.mdx`) suggest process-group detection is still being hardened on Linux containers — relevant if theuth runs omp inside sandbox VMs.
4. Does opencode 2.x `--mini`/headless (which theuth's ACP path resembles) report anything? The plugin says V2 Mini "do not run the TUI plugin" (`docs/next/website/src/content/docs/integrations.mdx#L198`), so herdr's opencode state coverage may not transfer to ACP-driven opencode at all.
5. omp's `herdr:blocked` custom event (`src/integration/assets/omp/herdr-agent-state.ts#L363-L373`) — which omp versions emit it, and for what (advisor? ask tool?). If omp exposes the same over ACP, theuth gets "blocked" for free.
6. No test proves hook `seq` monotonicity across an agent restart when the agent's clock goes backwards (timestamps are recommended as seq); the docs say "a timestamp works well" but a resumed agent on a machine with NTP step-back would be silently ignored.
7. Numbers in this report are from `wc`/`grep` on the shallow clone; nothing was built or run, so timings are as written in constants, not measured.
