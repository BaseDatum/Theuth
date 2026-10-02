# Meetings phase 6 — the live toolset: a lean, direct-call session while recording

Status: phase design v0 · 2026-09-20 · extends 53 §4.4/§4.7 (the meeting
session) and 08 §0.4 (direct vs runtime tools). Depends on phase 2 (session
setup/teardown in `StartMeeting`/`StopMeeting`) and phase 5 (`brief_*`
tools). Small: one agent message, one config key, two call sites in
`meeting`. Runs after phase 5, before the integration pass.

## 0. Why

Observed across the phase 2–5 dogfoods: the meeting session's fast model
fumbles python-mediated tool calls — 12 refused `meeting_topic` calls before
phase 2 made the two live tools `DIRECT` (02 §10 dev. 4), `python` cells that
could not see `meeting_search` (04 §10 not-exercised 2), text answers where a
`meeting_brief` was due (03 §9 dev. 8). A live meeting is the one place in the
harness where a fumbled call is a *missed* brief: the subject has moved on
before the retry lands. The session also receives every plugin's prompt block
and every registered tool (`shell`, `browser_*`, `computer_*`, MCP servers…),
none of which it can use in the seconds it has, and each of which is a
distraction for a small model.

Today: `agent.runtime_only` is global (`agent/src/lib.rs:1508`); the python
pool is fixed per session at construction (`loop_.rs:527, 595`); the tool list
is one global registry filtered per turn by `direct_tools` (`loop_.rs:2324-2342`)
and the registry's per-session `SetHidden` **denylist** (`registry.rs:112`);
the prompt is every plugin's block joined (`loop_.rs:2278-2291`).

## 1. Decisions

1. **A per-session override, set while recording, cleared at stop.**
   `agent::SetToolset { session, toolset: Option<Toolset> }` (`tell`), where
   `Toolset { tools: Vec<String>, prompts: Vec<&'static str>, runtime: bool }`.
   Stored on `Shared` as `Arc<RwLock<Option<Toolset>>>` exactly like
   `SetCompactionOverride` (`loop_.rs:289-299, 513, 1500-1510`) so it applies
   to the running turn. `None` (the default and the value at stop) = today's
   behaviour: the session is an ordinary session afterwards — python per the
   global setting, every tool, every prompt block. **This is the whole
   post-meeting story**: nothing about the session is different once
   `StopMeeting` has run; the follow-up chat, `Summarize`, harvest and
   compaction all see a normal session.
2. **Allowlist, not the registry's denylist.** `SetHidden` hides names; MCP
   tools appear and vanish as servers reconcile (`mcp.rs:1629-1660`), so a
   denylist made at start is stale by the first batch. The allowlist is read
   in `direct_tools`: keep specs whose name is in `tools`; skip the loop-local
   `todo` and `python` unless listed; `runtime: false` also drops the repl
   prompt block (`loop_.rs:2288-2291`) — the meeting model never hears about
   python. Rejected: `CreateSessionOptions.runtime` (fixed for the session's
   life — wrong, the session must recover its runtime at stop); a second
   session for the meeting (the follow-up chat must *be* the meeting's
   session, 53 §4.7).
3. **Every allowed tool is called directly.** With `runtime: false` the floor
   in `direct_tools` (`floor = runtime && cfg.runtime_only`) is off, so every
   listed tool is in the provider's tool array whatever its `DIRECT` flag.
   Their outputs land in the transcript (a few hundred tokens each; the
   session compacts at `context_tokens`, 02 §4). No tool changes its `DIRECT`
   value: `brief_search`/`knowledge_search` return data and stay
   runtime-tools in every other session (08 §0.4).
4. **Prompt blocks are allowlisted too.** `prompts` filters `ListPrompts` by
   plugin name before `frame::baseline`. The meeting block already says what
   the session is for; the knowledge block says "knowledge first". The
   `agent`/`tools`/`surface`/`browser`… blocks describe capabilities the
   session does not have. Fewer instructions → fewer wrong moves, and a
   shorter cached prefix.
5. **The set is config with a reviewed default.** `meeting.live_tools:
   Vec<String>` (default = §2's *included* column, web included) and
   `meeting.live_prompts` (default `["meeting", "knowledge", "web"]`). The
   user can widen it (an MCP tool by name, `read_file`) or narrow it (drop
   `search_web`) without a code change. No separate `live_web` flag — the
   lists are the knob.
   **81 MEETING-DIET:** `Toolset.hint_limit: Option<u32>` caps the lines per
   `<knowledge>` note for the session (`None` = the configured
   `knowledge.hint_limit`; `Some(0)` = no hints and no knowledge search at
   turn start). The meeting sets it from `meeting.live_hint_limit` (default
   `2`; `0` = none): on the 2026-09-28 session five long hint lines were ~1.8
   KB of every ~2 KB batch input, so they — not the transcript — dominated a
   3-hour meeting's context growth.
6. **Set after `CreateSession`, before the first line; cleared on every exit
   path.** `StartMeeting` step order (02 §10 dev. 9): session → `SetCompaction`
   → **`SetToolset(Some)`** → directory → devices → STT → tools → `started.`
   line. `StopMeeting` (02 §10 dev. 10): … → `SetCompaction{None}` →
   **`SetToolset(None)`** → `UnregisterTool` × 2 → hand-over to post. Also on
   `Starting → Failed`. A daemon restart mid-meeting loses both overrides
   with the recording — the session comes back ordinary, which is right.
   **81:** `PinTitle{true}` follows `RenameSession` at start;
   `PinTitle{false}` follows `SetToolset(None)` at stop and on
   `Starting → Failed`.
7. **Title pinned while recording (81 MEETING-DIET).** The meeting names its
   session (`RenameSession` at start). `title::due` fires on `epoch_changed`
   (a meeting compacts often) and on `turns_since`, since `[meeting …]` lines
   count as user text — 68 title calls in 32 minutes on the 2026-09-28
   session. `agent::PinTitle { session, pinned }` is an in-memory flag on the
   session (`Shared.title_pinned`, like the toolset override): `maybe_title`
   returns before computing anything while it is set. `title.rs` is
   unchanged; nothing is persisted, so a restarted session is unpinned and
   post-meeting turns title the session like any other.

## 2. Tool review table

Every tool the agent can see (inventory 2026-09-20). *Live* = in
`meeting.live_tools` by default. Post-meeting the session sees everything
again (decision 1), so this column is only about the recording window.

| tool | plugin | risk | live | why |
|---|---|---|---|---|
| `meeting_topic` | meeting | ReadOnly, DIRECT | **yes** | the output of the session |
| `meeting_brief` | meeting | ReadOnly, DIRECT | **yes** | requests, taps, deeper |
| `brief_search` | meeting (05) | ReadOnly | **yes** | cache first — the fastest brief is one that exists |
| `brief_put` | meeting (05) | ReadOnly | **yes** | refresh a stale cached brief in place (05 §0.7); `meeting_topic` caches new ones itself |
| `brief_delete` | meeting (05) | ReadOnly | no | eviction is a prefill-chat action; mid-meeting the model should `brief_put` the fix |
| `brief_list` | meeting (05) | ReadOnly | no | `brief_search` covers it |
| `knowledge_search` | knowledge | ReadOnly | **yes** | "knowledge first" (53 §3.4) |
| `knowledge_get` | knowledge | ReadOnly | **yes** | the second half of every knowledge lookup |
| `knowledge_list` | knowledge | ReadOnly | no | browsing; search is the live shape |
| `knowledge_put` | knowledge | ReadOnly | no | harvest at compaction records the meeting's facts (53 §4.7); a put mid-meeting is a turn spent not briefing |
| `knowledge_delete` / `_history` / `_restore` | knowledge | Destr./RO/Mut. | no | maintenance |
| `meeting_read` | meeting (04) | ReadOnly | **yes** | re-read earlier minutes after compaction ("what did they say about X ten minutes ago"); reads `live.jsonl` while recording |
| `meeting_search` | meeting (04) | ReadOnly | **yes** | previous meetings: "last time we agreed…" |
| `meeting_speakers` / `meeting_list` | meeting (04) | ReadOnly | no | post-meeting review shapes |
| `search_web` / `read_url` | web | ReadOnly | **yes** | a subject the user has never heard of is exactly what a brief is for; when knowledge and the cache have nothing, the model reaches the web (prompt order: cache → knowledge → web). Seconds per call, so the prompt says "one search, one read, then brief" |
| `http_request` | web | Mutating | no | asks; nothing live to call |
| `shell`, `start`, `send`, `screen`, `wait`, `jobs`, `cancel_job` | tools | Destr./Mut./RO | no | processes; seconds to minutes; approvals |
| `read_file`, `list_dir`, `grep` | tools | ReadOnly | no | the user's files are in knowledge if they matter to a meeting; add by name in `live_tools` if a repo doc is the source of truth for you |
| `write_file`, `edit` | tools | Mutating | no | never during a call |
| `ask_user` | tools | ReadOnly, DIRECT | no | nobody answers: the user is in the meeting; requests flow the other way (`[meeting request]`) |
| `add_mcp_server`, `remove_mcp_server`, `add_mcp_repo`, `sync_mcp` | tools | Destr./Mut. | no | configuration |
| MCP passthrough (`<server>__<tool>`) | tools | per gate | no (allowlist) | dynamic, unknown latency; add a specific name in `live_tools` if a server is your ticket system |
| `load_skill`, `add_skill`, `remove_skill`, `add_skill_repo`, `remove_skill_repo`, `sync_skills` | agent | RO/Mut./Destr. | no | skills are for doing work; the meeting session notices and briefs |
| `spawn_subagent`, `send_message`, `discover_peers`, `peer_message` | agent | RO/Mut., DIRECT | no | a subagent's result arrives after the moment; peers are another feature |
| `message_user` | agent | ReadOnly, DIRECT | no | the island is the channel; a Slack DM mid-call is noise |
| `list_models` | agent | ReadOnly | no | — |
| `todo` (loop-local) | agent | ReadOnly, DIRECT | no | no plan to keep; every turn is one batch |
| `python` (loop-local) | agent | ReadOnly, DIRECT | no (`runtime: false`) | the point of this phase |
| `show` | artifact | ReadOnly, DIRECT | no | briefs are the surface; a card would land in a window the user is not looking at |
| `app` | app | ReadOnly, DIRECT | no | same |
| `search` | rpc | ReadOnly | no | sessions/artifacts fuzzy search; `meeting_search` + knowledge cover the live need |
| `browser_*` (18) | browser | RO/Mut. | no | seconds per step, a window on the user's screen, approvals |
| `computer_*` (8) | computer | RO/Mut. | no | drives the user's desktop during their call |
| `site_learn` / `site_routes` / `site_call` | routes | ReadOnly | no | learned-route calls; add `site_call` by name if a route is your ticket system |
| `scrape_url` / `crawl` / `map_site` | scrape | ReadOnly | no | seconds to minutes |
| `secret_*` (4) | secrets | RO, one DIRECT | no | a masked prompt to a user who is talking |
| `schedule` / `schedule_done` / `schedule_status` | schedule | Mut./RO | no | — |
| `signal_link` / `signal_status` / `slack_status` | signal, slack | Mut./RO | no | — |

Prompt blocks: `meeting`, `knowledge`, `web` in; out:
`tools` (MCP dir, policy), `agent` (harness layout), `surface`, `app`,
`artifact`, `computer`, `routes`, `search`, `schedule`, `scrape`, `secrets`,
`voice`, plus the repl block (dropped by `runtime: false`).

## 3. Changes

- `crates/agent/src/{lib.rs,loop_.rs,supervisor.rs}`: `Toolset`,
  `SetToolset` (supervisor `ask` → session `tell`, the `SetCompaction`
  route); `Shared.toolset`; `direct_tools` applies `tools`/`runtime`;
  `rebase` applies `prompts` and gates the repl block on the effective
  runtime. `Toolset` is not on the wire (like `SetCompaction`).
- `crates/meeting/src/lib.rs`: `MeetingConfig { live_tools, live_prompts }`
  with §2 defaults; `StartMeeting`/`StopMeeting`/`Failed` call
  sites (decision 6); `DEFAULT_CONFIG` in `theuth/src/main.rs` gains the keys
  (commented, with the default list spelled out so the user can edit it).
- `crates/meeting/src/tools.rs` `PROMPT`: one sentence — *"In a live meeting
  every tool you see is called directly; there is no python."*
- Docs: 53 §4.7 gains the paragraph; 08 §0.4 an amendment line
  ("a session may run with an allowlisted, direct-only toolset — 53").

## 4. Check

1. `cargo test -p agent`: `direct_tools` with a `Toolset` keeps exactly the
   listed names (a listed non-`DIRECT` tool is present; `python`/`todo`
   absent; `runtime_only` ignored); `None` restores the pre-existing
   behaviour (extend the existing `direct_tools`/`rebase` tests).
2. Live: `meeting_start` → `list_tools(session)` shows only the default set,
   all direct; `get_prompt(session)` has the meeting and knowledge blocks
   and no repl block; the model's first batch turn calls `brief_search` /
   `knowledge_search` **directly** (no `python` cell, no "not a direct tool
   here" error in the turn's tool results); a `[meeting request]` gets a
   `meeting_brief` within one turn.
3. `meeting_stop` → the same session: `list_tools` shows everything,
   `python` is back (when the global runtime is on), `get_prompt` has every
   block; a follow-up question runs a python cell that calls `meeting_read`
   (this also side-steps 04 §10 not-exercised 2: the session's first python
   dump happens after every tool exists).
4. `live_tools` without `search_web` → it is gone from `list_tools`; a
   `live_tools` entry naming an MCP tool appears when the server is up. A
   subject in neither cache nor knowledge (a public product name) → the
   model calls `search_web` then `meeting_topic` within one turn.
5. `Starting → Failed` (deny a TCC prompt or pick a dead pid) → the session
   has no override (`list_tools` full).

## 5. Status

Implemented 2026-09-20. `agent`: `Toolset`, `SetToolset` (lib.rs), the
session-side `SetToolsetOverride`/`GetToolset` and `Shared.toolset`
(loop_.rs), `direct_specs`/`prompt_blocks` (the filtering, now free functions
`rebase` and `ShowPrompt` share), supervisor routing beside `SetCompaction`.
`tools`: `ListPrompts` replies `(plugin, block)` pairs so the prompt allowlist
has names to match. `meeting`: `MeetingConfig::{live_tools, live_prompts}`,
`Setup.toolset`, the three call sites (decision 6), the `PROMPT` sentence.
`theuth`: the two commented keys. `08-repl.md` §0.4 amendment line.

Exercised:

1. §4.1 — `cargo test -p agent` (233 green) with
   `toolset_allowlists_tools_and_prompt_blocks`: a listed non-`DIRECT` tool
   is in the array with `runtime_only`; `python` only with a pool *and*
   `runtime: true`; `todo` only when listed; the `spawn_subagent` depth gate
   still applies to a listed name; prompt blocks filtered by plugin; `None`
   is byte-identical to the pre-existing behaviour (floor on with a pool,
   `todo` + `python` always, every block).
2. `cargo check --workspace --all-targets` green; `cargo clippy -p agent
   -p meeting -p theuth --all-targets --no-deps` reports exactly the 14
   pre-existing `agent` sites and nothing in the changed code (`tools` has
   three pre-existing sites of its own; untouched).

Not exercised — the user runs them on a codesigned build (the headless
scratch run was dropped from this phase's scope):

3. §4.2 live start: the direct array, the filtered `get_prompt`, the first
   batch turn's direct calls, `[meeting request]` → `meeting_brief`.
4. §4.3 stop: everything back, python back, a follow-up python cell calling
   `meeting_read`.
5. §4.4 config: `live_tools` without `search_web`; an MCP name; the
   web-then-`meeting_topic` path for an unknown subject.
6. §4.5 `Starting → Failed` clears the override.

Deviations:

1. **`ShowPrompt` mirrors the override.** §4.2 reads `get_prompt(session)`;
   the supervisor rebuilds that prompt from scratch, so it asks the live
   session actor (`GetToolset`, an `ask`) and filters the same way. A
   session with no live actor has no override (it is in memory only).
2. **`list_tools` is not session-scoped** (`rpc::list_tools` takes no
   session; it lists the registry). The direct array is observable through
   the turn's tool calls and the absence of `not a direct tool here`, not
   through that RPC. Not widened here.
3. **`Toolset.prompts: Vec<String>`**, not `Vec<&'static str>` (§1.1): it
   comes from config.
4. **The runtime floor is off whenever a toolset is set**, not only with
   `runtime: false`: an explicit allowlist is the reviewed answer to "which
   tools are direct", so `Toolset { runtime: true, .. }` (not shipped by
   `meeting`) is allowlist + `python`, with no `DIRECT` filtering on top.
5. **The `Starting → Failed` clear lives in the `Begun` handler**, gated on
   `manifest.session` being non-nil (the placeholder before `CreateSession`),
   rather than at each failure return in `start_task` — one site instead of
   two, and it covers a `store.create` failure, which leaves no directory.
6. `53-meetings.md` §4.7 paragraph (§3 "Docs") not written here; it belongs
   to the integration pass with the other 53 edits.
