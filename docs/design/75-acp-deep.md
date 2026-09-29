# 75 — Deeper ACP integration: harness skills both ways, live context and cost, live model/effort

Status: draft 2026-09-28. Amends 06 §1 (table row `session/set_config_option`), §3.2 (`--resume` paragraph), §3.3 (event table), §3.6 (store, UI), §3.8 (MCP back-channel moves in scope), §7 (cost-visibility risk retired), §8.2 (rendering paragraph). Research: `~/.theuth/scratch/acp-research/{protocol-surface,theuth-internals,ecosystem}.md`, synthesis `~/.theuth/artifacts/8b91088aab977e8a.md`. Pins: omp 18.3.1, opencode 1.18.30; theuth `agent-client-protocol = "=2.1.0"` (`Cargo.toml:245-246`, schema 1.7.0, no features, `crates/acp/Cargo.toml:9`). Harness source lines are from `can1357/oh-my-pi@v18.3.1` (`packages/coding-agent/src/…`) and `sst/opencode@v1.18.30` (`packages/opencode/src/…`).

## 1. Problem

An ACP child is a black box to theuth beyond text, tool cards and one cumulative cost figure.

- **Skills, harness → theuth.** omp loads the `ponytail` plugin under `omp acp` (same `createAgentSession` as the CLI, `main.ts:2164-2188`) and announces every skill as an `available_commands_update` entry. theuth drops the notification: `crates/agent/src/acp_child.rs:1117-1118`, the `_ => Vec::new()` arm of `translate()`. `Loaded.skills` for an ACP session is `skills::in_frame()` over harness rows (`skills.rs:228`) → always empty.
- **Skills, theuth → harness.** Nothing of `~/.theuth/skills` reaches a child: `session/new` is `NewSessionRequest::new(cwd).meta(meta)` (`crates/acp/src/harness.rs:370-372`), the prompt is the raw task string (`acp_child.rs:643-646`), `_meta.theuth` carries `{model, effort, tools, parent, parent_engine, harness}` only (`crates/acp/src/meta.rs:23-39`).
- **Context.** `usage_update.size` is discarded; `used` survives as `turn.context` (`acp_child.rs:1112-1116`) and lands once, in `TurnFinished.context` (`:792-798`). Clients derive the window from the model catalog keyed by `model` = harness name → 0 → no percentage (`crates/desktop/ui/src/components/chat/ContextMeter.tsx:24-25`, `crates/tui/src/model.rs:6482-6506`). No `ContextUsed` for ACP children.
- **Tokens.** `TurnFinished.usage` is `Usage::default()` (`acp_child.rs:796`); `TokenTotals` (`cost.rs:260-275`) for remote owners is always zero; `remote_meters` (`migrations/0015_session_lifecycle.sql:50-69`) has no model column.
- **Cost guards.** The zero baseline hard-codes `"USD"` (`acp_child.rs:1239-1247`); cumulative semantics are assumed with no detection (`store/accounting.rs:469-509`); `report_id` is always `None` (`acp_child.rs:1038`).
- **Model/effort.** Set only at connect (`harness.rs:439-456`); `SetModel`/`SetEffort` go through `LiveActor::native()?` → `NOT_NATIVE` "not available for an external harness" (`supervisor.rs:267-268, 311-312, 5963, 6040`); `spawn_subagent.effort` is refused for an external harness (`supervisor.rs:3059-3062`); the desktop renders `agent · harnessModel` as plain text, no picker (`ModelMenu.tsx:149-154, 177`); the TUI `/model` sends `SetModel` unguarded (`model.rs:5857-5864`) and gets the refusal back.

## 2. Evidence

- ponytail is in a theuth-spawned child's context: `~/.omp/agent/sessions/-Develop-theuth/2026-09-28T12-31-26-001Z_01a0e7ff-a531-70b2-8463-d2241b8d4245.jsonl:6`, assistant thinking "Since the ponytail skill is active, I should check that first". The plugin (`@dietrichgebert/ponytail` 4.9.0, `~/.omp/plugins/omp-plugins.lock.json:3-7`) injects its rules from `before_agent_start` (`pi-extension/index.js:204-210`) and ships six `SKILL.md`s; ACP mode disables none of it (protocol-surface §0).
- The wire carries `skill:<name>` + `/ponytail*` in `available_commands_update` 50 ms after `session/new` (omp `modes/acp/acp-agent.ts:2125-2135`, `slash-commands/available-commands.ts:64-86`, `source` stripped `:112-118`); opencode sends bare skill names merged with commands (`acp/service.ts:761-772, 936-956`) from a `setTimeout(0)` after the `session/new` response.
- `usage_update {used, size, cost?}` is stable (RFD `session-usage`, 0.13.6; `cost` = cumulative session cost). `PromptResponse.usage` is unstable (`schema.unstable.json`, feature `unstable_end_turn_token_usage`), semantics disputed (issue #1860): omp sends a true per-turn delta (`acp-agent.ts:2214-2237`), opencode the last request's tokens (`service.ts:824-841`, `acp/usage.ts:90-103`).
- `session/set_config_option` is accepted at any time by both: omp `acp-agent.ts:777-821` (`mode`, `model`, `thinking`; unknown id → error), opencode `service.ts:400-455` (`model`, `effort`, `mode`; unknown → `InvalidConfigOptionError`). 06 line 9 lists it as "not exercised".
- No ACP client passes skills over the protocol; the ecosystem injects via cwd skill dirs or `mcpServers` (ecosystem §1, §3; Zed: "Zed Skills do not apply" to external agents).

## 3. Harness skills manifest (S1)

**Where the arm goes: the mailbox, not `translate()`.** The manifest arrives with no turn open (omp 50 ms after `session/new`, opencode after the response) and the mailbox drops turnless updates (`acp_child.rs:1409-1414`). It goes beside the two session-level arms that already run before the turn check, `Plan` (`:1397`) and `ConfigOptionUpdate` (`:1400-1404`):

```rust
Update::Session(SessionUpdate::AvailableCommandsUpdate(u)) => self.set_harness_commands(u.available_commands).await,
```

**Event and store**, following `HarnessModelChanged` (`lib.rs:1728-1733`) → `set_harness_model` (`acp_child.rs:370-384`) → `0021_harness_model.sql:7`:

```rust
pub struct HarnessCommand { pub name: String, pub description: String, pub hint: Option<String> }  // input.hint
AgentEvent::HarnessCommandsChanged { session: SessionId, commands: Vec<HarnessCommand> }
```

- `sessions.harness_commands TEXT` (JSON array), migration `0027_harness_commands.sql`; `Store::set_harness_commands` writes once per distinct value; the event is published after the row, only on change (omp re-sends on every metadata change, opencode once per prompt).
- `Loaded.harness_commands: Vec<HarnessCommand>` (`lib.rs:678-705`, TS `crates/desktop/ui/src/rpc/bindings/Loaded.ts`). Not on `SessionSummary`: no list view renders skills and the array is not small.

**Classification**, one client-side function:

| harness | entry | shown as |
|---|---|---|
| omp | `skill:<name>` | skill `<name>` |
| omp | `/ponytail`, `/ponytail-review`, … (extension commands, `available-commands.ts:75-86`) | command |
| opencode | any name (`source` stripped before the wire, `service.ts:948-951`) | command — skills and commands are indistinguishable |
| others | any name | command |

**Rendering.**

- Desktop `components/library/Skills.tsx` reads `skills` (`:42`) and dots loaded skills (`:109-113`). For a session with `agent` set it reads `harnessCommands` instead: header "Harness skills", skill rows first, a "Commands" group after; no `always` badge (`:114`). Slice `harnessCommands` on `SessionSlice` (`store/sessions.ts:186-191`), reducer case like `SkillsChanged` (`reduce.ts:640-641`), `loadHistory` mapping (`sessions.ts:374-387`).
- TUI: the `skills:` status segment (`model.rs:6417-6420`) shows `harness skills: ponytail, caveman, …` for ACP sessions; the line printer prints `harness commands → N` on change, like `harness model →` (`:5622-5628`).

**Not knowable.** Whether the model *used* a skill in a turn. omp and opencode emit no per-use signal (omp's `/skill:<name>` invocation path `acp-agent.ts:1049-1073` is client-initiated); claude-agent-acp's `skill` update covers inferred use only. The manifest says *loaded*; the UI wording says "loaded".

## 4. theuth skills → child (L1)

**Mechanism: files in the child's cwd.**

- omp walks up from `cwd` for `.omp/skills`, `.claude/skills`, `.agents/skills` (`extensibility/skills.ts:141-192`; `enableAgentsProject`/`enableClaudeProject` default on `:147-151`). opencode: `.claude`/`.agents` walk-up plus `.opencode/skills` (`skill/index.ts:196-208`). Codex and Gemini read `.agents/skills`; Claude Code `.claude/skills` (ecosystem §1).
- No per-process lever exists: neither harness reads `_meta`, env or a flag for skills (`acp-agent.ts:693-703`, `service.ts:169-196`); omp's `skills.customDirectories` is a settings key and `settings.cloneForCwd(cwd)` (`main.ts:494`) re-reads *project* settings, so an overlay dir would need `<cwd>/.omp/settings.json` — more invasive than the skills. Whether either reads skills from `additionalDirectories` was not checked. cwd it is.

| harness (`agents.yaml` entry) | directory |
|---|---|
| default (omp, opencode, Codex, Gemini) | `<cwd>/.agents/skills/<name>/SKILL.md` |
| `skills_dir: .claude/skills` (Claude Code via `claude-agent-acp`) | `<cwd>/.claude/skills/<name>/SKILL.md` |

One directory per child, never both: omp and opencode read both roots and warn on name collisions (`skills.ts:194-196`).

**Config.** `Entry` (`crates/acp/src/agents.rs:21-38`, no `deny_unknown_fields`) gains:

```yaml
omp:
  command: omp
  args: [acp]
  skills: true               # default; false opts the harness out
  skills_dir: .agents/skills # default; relative to the child's cwd
```

**Which skills.** `skills::index(dir)` entries with `always: true` (`skills.rs:86-136`) plus those named in a new `spawn_subagent.skills: Vec<String>` (`peers.rs:35-56`; unknown name → tool error listing the index). No per-harness skill list in `agents.yaml`: the model choosing per task is the existing pattern for `tools`, `model`, `cwd`.

**Format mapping.** theuth's `FrontMatter { name, description, always }` (`skills.rs:33-41`, gray_matter YAML); a skill is a `<dir>/SKILL.md` or a loose `*.md` named by file stem (`:100-109`) with the description defaulting to the first non-empty body line (`:110-116`).

| theuth | materialised `SKILL.md` |
|---|---|
| `SkillMeta.name` (resolved) | `name:` |
| `SkillMeta.description` (resolved) | `description:` |
| `always` | dropped — no harness reads it; `always` decides *selection* (above) |
| body (`skills::body`, `:140`) | body |
| sibling files of a `SKILL.md` skill | copied beside it (progressive disclosure per agentskills spec) |

**Git and cleanup.**

- `Agents::acp_session` (`supervisor.rs:757-844`) materialises before `AcpChild::spawn` (`:821`); an existing `<name>/` is skipped (the user's wins, logged).
- Inside a git worktree: one line `/<skills_dir>/<name>/  # theuth <session>` per skill appended to `git rev-parse --git-path info/exclude` (worktrees have a `.git` file, so no path guessing).
- The written paths ride on the actor; `AcpChild` stop (any reason, incl. `shutdown`) removes them and their exclude lines. A crash leaves files that the next spawn on that cwd reuses or the exclude hides; no reconciler.

**Interaction with S1.** A materialised skill appears in the child's manifest as `skill:<name>` (omp) / `<name>` (opencode). That is the verification, and why L1 follows S1.

Not in scope: per-turn prompt-text injection (costs child context each turn, invisible as a skill; the brief already carries the task). Re-materialising on `session/load`: done by §10's resume from the row's stored names, since the stopped actor removed the files and a new harness process reads skills at start.

## 5. Live context (C1)

- `UsageUpdate` moves from `translate()` (`acp_child.rs:1112-1116`) to the mailbox before the turn check, beside `ConfigOptionUpdate`: `used`/`size`/`cost` are session facts. opencode sends the update from the prompt's completion path (`service.ts:527`); its order relative to the `session/prompt` response is not established, and a turnless update must not be dropped. `Turn.context`/`Turn.meter` (`acp_child.rs:153-157`) become actor fields; `finish` (`:756`) reads them as before; metering (`:1424-1433`) moves with them.
- Publish per update: `AgentEvent::ContextUsed { session, turn, context: used, size: Option<u32> }` (`lib.rs:1886-1893`; native rounds at `loop_.rs` pass `None`). `turn` = the open turn or the last finished one (actor field). Desktop `reduce.ts:647-648` and TUI `model.rs:5612-5615` already consume the event; they take `size` when present.
- Carry `size`: `sessions.context_size INTEGER` (migration `0028_context_size.sql`, written once per distinct value), `Loaded.context_size: Option<u32>`, `SessionSummary.context_size: Option<u32>` (one integer; a sidebar percentage later is free).
- Clients: `limit = contextSize ?? catalogLimit` — `ContextMeter.tsx:24` and `model.rs:6482-6489`. Thresholds move from 70/90 (`ContextMeter.tsx:32-36`, `model.rs:6486-6489`) to the RFD's 75 / 90 / 95 three-band, for every session, one constant each side.

| fill | band | RFD wording |
|---|---|---|
| < 75 % | normal | — |
| 75–90 % | warning | "context filling up" |
| 90–95 % | danger | "start new session or summarise" |
| > 95 % | danger, pulsing | "next prompt may fail" |

- Cadence: omp once per turn at `agent_end` (`acp-agent.ts:1469-1472, 2172-2187`); opencode after every prompt/command/compact (`service.ts:527,551,573`). "Live" for omp is the gauge moving at turn end, not per tool call. `compaction_update` (schema 1.9 unstable, RFD Preview 2026-09-23) would explain sudden `used` drops — watch, do not build.
- Slack: `Footer` (`crates/slack/src/bubble.rs`, built on `TurnFinished` in `channel.rs`) gains ` · ctx used/size` (k/M-formatted, `16.5k/1.0M`) when `size` is known: the worker keeps the last `ContextUsed.size` per thread and `TurnFinished.context` is `used`. `ModelUsageChanged` stays ignored there.

## 6. Usage guards (C3/C4)

**Semantics stay cumulative.** The spec defines `usage_update.cost` as cumulative session cost (RFD `session-usage:62-72`); omp (`acp-agent.ts:2184`, omitted when 0) and opencode (`usage.ts:111-115`, always present) comply. `apply_meter` (`accounting.rs:439-556`) keeps the high-water rule: equal → no charge (`:473-481`), greater → attribute the delta (`:482-495`), lower → `coverage=ambiguous, boundary=lower_total` (`:496-506`).

**Keep the raw update.** `remote_meters.source_metadata` (`0015:66`) holds a ring of the last 8 raw reports `{amount, currency, at}`. A second `Ordering::Less` within one epoch sets `delta_suspected: true` there; `SessionCost.remote_meter_gaps` (`cost.rs:344-365`) already surfaces the gap and `CostDialog` adds "harness may report per-turn cost". No automatic summing: a lower total is also what a harness-side reset looks like, and summing double-charges.

**Currency from the update.** Today the connect-time zero row is `"USD"` (`acp_child.rs:1244`); a first report in another currency opens an `imported` epoch with `coverage=unknown` (`accounting.rs:519-528`), which also suppresses outbound `acp_cost` (`crates/rpc/src/acp.rs:533-544`). Fix, one branch in `apply_meter`: when the current epoch is `known_zero` with `high_water_nanos = 0` and the report's currency differs, rewrite that row's `currency` in place — nothing was observed in the old currency, so the zero baseline is currency-agnostic. Only a currency change after attribution opens an epoch.

**Non-USD harness afterwards.** `Money` never converts (`cost.rs:33-37`); `SessionCost.total.returned: Vec<Money>` is per currency and the UI lists each bucket; `acp_cost` still requires a single currency; the roster budget charges only its own currency (`crates/roster/src/lib.rs:1029`), so EUR child spend does not count toward a USD budget. Stated, not fixed here.

## 7. Per-turn tokens (C2)

| route | gives | costs |
|---|---|---|
| A: `agent-client-protocol = "=2.2.0"` + feature `unstable_end_turn_token_usage`; read `PromptResponse.usage` | on the wire from both harnesses; omp = true turn delta, opencode = last request only (#1860); no model id | pin change — 06 §7 pinned for API churn; the 2.1→2.2 diff is unverified until `cargo check` on `crates/acp` and `crates/rpc/src/acp.rs` |
| B: read omp's ledger `~/.omp/agent/sessions/<cwd-slug>/*.jsonl` (assistant entries: `provider, model, usage{input,output,cacheRead,cacheWrite,cost{…}}`, `packages/ai/src/types.ts:1101-1137`) | exact per-request model, cache split and cost; continuation-turn spend | omp-only; private format (`version: 3` header); cwd→slug mapping; a file watcher; opencode needs `opencode.db` (schema not inspected) |

**Decision: A.** One field read that works for every harness; B is a second protocol. Fallback to B only if the SDK diff is expensive.

- `Harness::prompt`'s `Turn` (`harness.rs:501`) resolves to `(StopReason, Option<Usage>)`; `finish` maps `inputTokens → prompt`, `outputTokens + thoughtTokens → completion`, `cachedReadTokens → cached` into `TurnFinished.usage` (`lib.rs:1540-1547`).
- Monotonicity guard (Copilot sends cumulative totals, ecosystem §2a): if every field is non-decreasing across ≥ 2 turns of a session, the response is treated as cumulative and diffed; otherwise per-turn. opencode's last-request figure is a lower bound and is labelled so in `/cost`.
- Per-model breakdown: `0029_remote_turns.sql`

```sql
CREATE TABLE remote_turns (owner_id TEXT NOT NULL, turn_id TEXT NOT NULL, model TEXT,
  prompt INTEGER NOT NULL, completion INTEGER NOT NULL, cached INTEGER NOT NULL, at TEXT NOT NULL,
  PRIMARY KEY (owner_id, turn_id));
```

  One row per finished ACP turn, `model = sessions.harness_model` at turn end — constant within a turn because §8 applies changes at turn boundaries. `TokenTotals` for remote owners sums it; `GROUP BY model` is the breakdown. Money stays in `remote_meters`; tokens carry no cost, nothing double-counts.

## 8. Live model and effort (required)

**Harness side.**

| | omp 18.3.1 | opencode 1.18.30 |
|---|---|---|
| handler | `setSessionConfigOption` `acp-agent.ts:777-821` | `setSessionConfigOption` `service.ts:400-455` |
| `category: model` | id `model`, value `provider/id` (`:1765-1776`, `#toModelId :1830`); unknown → `Unknown ACP model` | id `model`, value `provider/model[/variant]` (`config-option.ts:38-50, 112-130`); unknown → `InvalidModelError` |
| `category: thought_level` | id `thinking` (`:1779-1788`); values `off`, `auto`, then the model's subset of `minimal, low, medium, high, xhigh, max` (`:1792-1801`, `pi-tui/thinking.ts:9-46`); unknown → `Unknown ACP thinking level` | id `effort`, **only when the current model defines variants** (`config-option.ts:53-70, 98-108`); values = variant keys; unknown → `InvalidEffortError`. The 06 §8.2 probe saw `model` + `mode` only because `opencode/big-pickle` has no variants |
| `category: mode` | id `mode`, plus `current_mode_update` (`:800-805`) | id `mode` |
| unknown id | `Unknown ACP config option` | `InvalidConfigOptionError` |
| response | full `configOptions`; also pushes `config_option_update` (`:807-818`) | full `configOptions` |

**Rule for a missing option: refuse, never ignore.** No `thought_level` in the harness's `configOptions` → `set_harness_option` fails with `"<agent>: this harness offers no effort option (has: model, mode)"`. An unknown value → the harness's error text plus the option's `options[].value` list. Nothing is applied silently.

**Effort values.** No theuth→harness mapping table: the picker shows the harness's `options[]` verbatim and sends `value`. `spawn_subagent.effort` on an external harness (today refused, `supervisor.rs:3059-3062`; `peers.rs:52` "Not for an external harness") is applied at connect after the model (`harness.rs:439-456` pattern): the `thought_level` option whose `value` equals the keyword (omp: `low|medium|high|…`; opencode: variant key), else the spawn fails naming the list. The refusal and the `peers.rs:52` wording go.

**Path.**

```
desktop ModelMenu · TUI `/model [v]`, `/effort <v>`
  → harness_options(session) -> Vec<ConfigOption>                   (live process; error when gone)
  → set_harness_option(session, config_id, value) -> SetOptionResult { options, pending: bool }
  → Agents: LiveActor::Acp → AcpChild::SetOption { config_id, value }
  → Harness::set_config_option(id, value)      new fn; today inline SetSessionConfigOptionRequest at harness.rs:443
  → response.config_options → Harness.config_options; acp::current_model() (harness.rs:535) → set_harness_model
      (0021 row, HarnessModelChanged, as at connect acp_child.rs:1229-1234)
```

- `set_model`/`set_effort` (`api.rs:562-563`; `supervisor.rs:5945-5983, 6012-6041`) stay native-only. The option list is the harness's, not the catalog's, so a separate pair beats teaching `ModelInfo` about harness ids.
- `Harness` keeps `config_options: Vec<SessionConfigOption>` (today only `model: Option<String>`, `harness.rs:179-180`), refreshed from `session/new|load` (`Link::open` already returns the list, `:343-350`), every `config_option_update`, every `set_config_option` response.
- `ConfigOption { id, name, category, current, options: Vec<{value, name, description}> }` is a theuth type with ts-rs bindings; the SDK type stays inside `crates/acp`.

**Mid-turn.** omp's handler has no streaming guard (`acp-agent.ts:777-795`); `AgentSession.setModel` delegates to `#models.setModel` (`session/agent-session.ts:8909-8919`) and whether the request in flight switches was not established. Safe default: idle → send now; turn open (incl. a 06 §8.3 continuation turn) → queue on the actor, send at `PromptDone` before any queued input's prompt; the RPC returns `pending: true` and the UI shows `→ <value> (after this turn)`. This also keeps `harness_model` constant within a turn (§7). Reversible to immediate once verified.

**UI.**

- Desktop `ModelMenu`: an ACP child (`m.agent`) renders a picker from `harness_options` — a `Model` group from the `category: model` option and an `Effort` group from `thought_level` when present, current values marked, the same `CommandGroup` shapes as the native menu (`ModelMenu.tsx:116-130`). The plain-text row (`:149-154`, `:177`) stays for a gone process, title "harness process not running".
- TUI: on an ACP session (`Loaded.agent`), `/model` with no argument lists `harness_options`; `/model <value>` and `/effort <value>` send `Command::SetHarnessOption` (`model.rs:5857-5864` dispatch). `pending` prints `model → <value> after this turn`.
- Effort is not persisted: the live list is the truth and a gone process has nothing to pick. Model persists through the existing `harness_model` path.

## 9. MCP back-channel (L3) — later slice

- `theuth mcp --session <id>`: a stdio MCP server (rmcp `server` feature) proxying to the daemon over jsonrpsee with the session's token. Passed as `mcpServers: [{name: "theuth", command: <theuth binary>, args: ["mcp", "--session", id]}]` in `session/new` (`harness.rs:370-372`; `Connect` `:144-164`; `Agents::acp_session` `supervisor.rs:775-800`).
- Tools: `load_skill(name)` / `list_skills` (`skills::load_text`, `skills.rs:188`); `send_message(text)` → parent `Deliver` in the `_theuth/peer_message` uplink shape (`acp_child.rs:386-415`, `rpc/acp.rs:842-844`); mutating tools pass `ToolRegistry::ExternalApproval` (06 §8.8) so one modal covers harness and back-channel calls.
- Why local stdio: opencode crashed on an unreachable *remote* MCP (synthesis §1b); a subprocess the harness spawns cannot be unreachable. omp: client MCP servers are its only MCP source in ACP mode (`main.ts:485-490`) and `type: "acp"` is rejected (`acp-agent.ts:2717-2743`) — stdio is the one shape both accept today.
- On by default; `agents.yaml` `mcp: false` opts out (owner ruling 2026-09-29: the tools are read-only skill lookups plus `send_message` to the parent; the approval gate for mutating tools stays a follow-up). L1 covers skills without this; L3 is for the child's voice and on-demand skill bodies. 06 §3.8's trigger ("one transcript showing a stuck child that should have asked") has arrived with this request.

## 10. `load_session` resume

- Both harnesses advertise `loadSession` and replay history (`acp-agent.ts:705-715`, `service.ts:235`); `Link::open` already loads with a fallback to new (`harness.rs:355-368, 398-400`); replay is dropped (06 §8.11). Today the only trigger is `send_message` to a child whose actor is gone (06 §3.2; `supervisor.rs:803-808`).
- Add `resume_child(session)` — TUI `/resume` (no argument, on an ACP child), a Resume button beside the harness picker of an idle ACP child in the desktop: stop the live actor if any (`AcpChild::Shutdown`: the connection closes and the SDK kills the process group — theuth holds no pid, so an orphan from an earlier engine process is out of reach), respawn with `resume: Some(acp_session)`, no prompt. The omp session continues under theuth's session, transcript and ledger intact. The row's `acp_skills` (migration 0030, the `spawn_subagent.skills` names) are materialised again — the stopped actor removed them (§4). Refused: native session, a node's session, no stored ACP session id.
- **The cancelled prompt is not resumed** (2026-09-29). `session/load` replays the history, including a turn the user cancelled, and the harness would pick that work up again on the next prompt. When the resumed row's last turn ended `cancelled` — the last `TurnFinished` this engine saw for it, or a turn `resume_child` itself cut with `Shutdown` — the supervisor tells the new actor a `Note`, and the first `session/prompt` carries `<runtime>your previous turn was cancelled by the user before it finished; do not continue it — answer only the message below</runtime>` ahead of the user's text. Wire only: the persisted user message is the text alone; a busy retry re-sends the same prefixed prompt; a plain resume of a finished child sends nothing. Nothing new is persisted, so a resume in a later engine process (no `last_stop`) sends no note.
- Replaces `procedures/omp-acp-timeout-recovery` (`omp -r <id>` in a PTY, commit its work by hand) for children cut by `max_runtime_secs`. Not for remote nodes (their `acp_session` belongs to another engine). claude-agent-acp's replay bug (#998) does not apply to the pinned harnesses.

## 11. Amendments to 06

| 06 | change |
|---|---|
| §1 table, verification line 9 | `session/set_config_option` moves from "not exercised" to used (§8) |
| §3.2 `--resume` paragraph | `resume_child` is the explicit trigger; the `send_message` respawn stays (§10) |
| §3.3 row `usage_update` | `ContextUsed {used, size}` per update; `size` stored (§5); handled in the mailbox, not `translate()` |
| §3.3 row `available_commands_update, …` | `available_commands_update` → `HarnessCommandsChanged` (§3); `config_option_update` refreshes the option list (§8); `current_mode_update`, `session_info_update` still ignored |
| §3.3 row `session/prompt` response | `usage` from `PromptResponse.usage` when present (§7) |
| §3.6 store, RPC | `harness_commands` (0027), `context_size` (0028), `remote_turns` (0029), `acp_skills` (0030); `Loaded.harness_commands/context_size`, `SessionSummary.context_size`; RPC `harness_options`, `set_harness_option`, `resume_child` |
| §3.8 | MCP back-channel in scope as §9; `~/.theuth/mcp/*.json` forwarding stays out |
| §7 cost visibility | retired: both pinned harnesses send `usage_update`; tokens via §7 |
| §8.2 rendering | "no picker, no effort menu; `set_model` on a harness is not possible" superseded by §8 |
| §8.11 | two exceptions: `usage_update` and `available_commands_update` are applied during replay (current-state facts) |

## 12. Rejected / not now

- `_meta`-based skill passing: neither omp (`acp-agent.ts:693-703`) nor opencode (`service.ts:169-196`) reads `_meta` on `session/new` or `session/prompt`; no client in the ecosystem does it.
- SACP / MCP-over-ACP (`type: "acp"`), proxy chains: draft RFDs, Rust SDK rewrite in flight, omp rejects the transport today.
- Forwarding `~/.theuth/mcp/*.json` to children: 06 §3.8 holds (doubles processes, bypasses gates).
- SDK bump beyond what C2 needs; v2 protocol; `session_notice`/`compaction_update` consumers.
- omp `_omp/extensions/toggle` for a per-child "disable ponytail": vendor-specific; revisit when a transcript needs it.
- Route B (omp jsonl reader): fallback only.
- Persisting effort for ACP children; a per-harness skill list in `agents.yaml`; an `always`-style flag on the wire.

## 13. Implementation order

Live `set_config_option` is the hard requirement and leads. S1 before L1: L1's verification is S1's manifest. C3/C4 before C2: same ledger files, and the currency fix is one branch. `resume_child` after C2 because it is independent and small; L3 last, as 06 scoped it.

| # | slice | files | verification |
|---|---|---|---|
| 1 | §8 live model/effort | `crates/acp/src/harness.rs` (`set_config_option`, `config_options`), `acp_child.rs` (`SetOption`, queue at `PromptDone`, connect-time effort), `supervisor.rs` (route, drop the effort refusal), `rpc/src/api.rs` + bindings, `ModelMenu.tsx`, `tui/src/model.rs` | switch a running omp child's model from the ModelMenu: the next assistant entry in `~/.omp/agent/sessions/<slug>/*.jsonl` carries the new `model`, preceded by a `model_change` entry; `/effort xhigh` → `thinking_level_change`; `set_harness_option(effort)` on an opencode child without variants returns the "no effort option" error; mid-turn switch shows `pending` and lands at turn end |
| 2 | §3 manifest | `acp_child.rs` mailbox arm, `lib.rs` event + `Loaded`, store + `0027`, `Skills.tsx`, `store/sessions.ts`, `reduce.ts`, `model.rs` | spawn an omp child: the Skills panel shows `ponytail` (+ the caveman skills) under Harness skills and `/ponytail*` under Commands; an opencode child lists its commands; `load_session` after restart shows the same |
| 3 | §5 context | `acp_child.rs` (mailbox `UsageUpdate`, `ContextUsed`), `lib.rs`, `0028`, `ContextMeter.tsx`, `model.rs`, `bubble.rs` | after one omp turn the desktop gauge reads `used / size (N%)` in the 75/90/95 bands; TUI header `ctx N / M (P%)`; Slack footer `ctx` |
| 4 | §6 guards | `store/accounting.rs`, `acp_child.rs:1236-1247` | unit tests: zero epoch + EUR report rewrites currency, no new epoch; two lower totals set `delta_suspected`; omp session → `remote_meter_gaps = 0` unchanged |
| 5 | §4 skills into cwd | `supervisor.rs::acp_session`, `acp_child.rs` cleanup, `agents.rs` (`skills`, `skills_dir`), `peers.rs` (`skills`), `skills.rs` (materialise) | `spawn_subagent{agent:"omp", skills:["find-skills"]}` → `.agents/skills/find-skills/SKILL.md` exists, `info/exclude` has the line, the manifest shows `skill:find-skills`; killing the child removes both; `git status` clean throughout |
| 6 | §7 tokens | `Cargo.toml`, `crates/acp/Cargo.toml`, `harness.rs` (`Turn` usage), `acp_child.rs::finish`, `0029`, `cost.rs`/`accounting.rs` | `cargo check` on 2.2.0 across crates; an omp turn's `TurnFinished.usage.prompt > 0`; `/cost` lists tokens per model after a mid-session switch |
| 7 | §10 resume | `supervisor.rs`, `api.rs`, TUI/desktop command | cancel an omp child by `max_runtime_secs`, `/resume`, `send_message` answers from the prior context; no orphan `omp acp` in `pgrep` |
| 8 | §9 MCP | `crates/theuth` (`mcp` subcommand), rmcp server, `harness.rs` mcpServers, `agents.rs` `mcp` | a child calls `load_skill` and `send_message`; the parent receives `<message from=…>`; a denied mutating call shows one modal |

## 14. Risks

- **SDK 2.1→2.2 API churn** — 06 §7's reason for the pin. Contained to slice 6; route B is the fallback.
- **Turnless updates applied during replay.** `usage_update`/`available_commands_update` are current-state facts, so applying them is right, but 06 §8.11's "drop everything" gains two exceptions; a replayed stale `usage_update` is overwritten by the first live one.
- **Mid-turn option change is queued.** A user expecting an instant switch sees "after this turn". Reversible once omp's in-flight behaviour is verified.
- **cwd pollution.** L1 writes into the user's tree; a crash leaves `.agents/skills/<name>` behind (git-excluded, visible in the manifest). No reconciler by design.
- **opencode token semantics** (last request, not turn) under-report; the monotonicity guard cannot fix it. Labelled lower bound.
- **Two skill roots.** A user with a same-named skill in the other root sees a harness collision warning; theuth skips only its own root.
- **`thought_level` absent for many opencode models** (no variants) — the picker shows Model only; `/effort` fails with the clear error. Expected, not a bug.
- **Stale TS bindings.** The UI imports `crates/desktop/ui/src/rpc/bindings/`; `crates/agent/bindings/` already lacks `harness_model`/`ContextUsed`. Regenerate (`just ts-desktop`) in every slice that adds a field.
