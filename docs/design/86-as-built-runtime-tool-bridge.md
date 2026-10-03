# 86 as built — RUNTIME-BRIDGE: runtime functions named as python calls; a guessed direct call is bridged, not refused

Branch `fix/86-runtime-tool-bridge` off main 4e618ae9, two commits, 2026-10-03.
Brief: `/tmp/mc-2026-10-03/omp-86-bridge.md`.

## Why

Measured from `~/.theuth/sessions.sqlite` (2026-10-03): 30 direct `tool_use`
calls of runtime-only names in 24 of 329 tool-using sessions (7 %);
claude-opus-5-5 11/57 sessions (19 %), claude-fable-5-1 7/67 (10 %), opus-4-8
1/58, gpt-5.6-sol 0/32. Almost never on the first tool call (1 of 24): it
happens after 3–21 earlier calls, when prose names a function. Top guesses:
`knowledge_put` 6, `knowledge_get` 4, `discover_peers` 3, `cancel_reminder`
3, `send` 2, then `task_update`, `task_note`, `shell`, `session_read`,
`read_file`, `meeting_speakers`, `meeting_read`, `load_skill`,
`computer_screenshot`, `brief_put`, `artifact_read` once each. Root cause:
the rule lived only in the index heading at the end of the prompt while prose
above named those functions as if direct, and the `<knowledge>` hint said
"call knowledge_get before relying on one".

## Commit 1 — prompt wording (`prompt: runtime functions are named as python calls everywhere; one rule up front`)

Convention: a runtime-only function is written `` `name()` ``; direct tools
(`python`, `todo`, `show`, `app`, `ask_user`, `spawn_subagent`,
`send_message`, `message_user`, `remind_me_in`, `secret_request`,
`secret_request_form`, `desktop_handoff`, `meeting_topic`, `meeting_brief`)
stay bare. New first sentences of `GUIDANCE_HEAD` (crates/agent/src/frame.rs):

> Direct tools are exactly the ones defined in your tool list. Every other
> tool named in this prompt — written `like_this()` — is a runtime function:
> call it inside `python` as `r = await like_this(...)`; a direct `tool_use`
> of such a name is bridged for you once and costs a round-trip.

Index heading (crates/agent/src/repl.rs `RUNTIME_BLOCK`):

> Runtime functions (NOT direct tools — call each as `r = await name(...)`
> inside `python`; all `async`):

`<knowledge>` hint header (crates/knowledge/src/harvest.rs):

> Concepts that may be relevant; read one with `await knowledge_get(id)` in
> python before relying on it:

Meeting sentence (crates/meeting/src/tools.rs), verified against
`MeetingConfig::live_toolset` (`runtime: false`, allowlist `meeting.live_tools`):

> In a live meeting session (inputs beginning `[meeting …]`) there is no
> `python` and every tool in your tool list — the meeting's allowlist, by
> default meeting_topic, meeting_brief, brief_search, brief_put,
> knowledge_search, knowledge_get, meeting_read, meeting_search, search_web,
> read_url — is called directly by name; outside one, `meeting_topic` and
> `meeting_brief` are direct tools and the others named here are runtime
> functions.

The old claim ("every tool you see is called directly; there is no python")
was true; it is now precise about which tools and that it is the allowlist.

Files: crates/agent/src/{frame,repl,attach,skills,peers,supervisor}.rs,
crates/knowledge/src/{harvest,tools}.rs, crates/tasks/src/tools.rs,
crates/roster/src/{tools,desk}.rs, crates/schedule/src/tools.rs,
crates/secrets/src/tools.rs, crates/sandbox/src/tools.rs,
crates/rpc/src/search.rs, crates/document/src/tools.rs,
crates/meeting/src/tools.rs, crates/computer/src/tools.rs,
crates/web/src/tools.rs, crates/scrape/src/tools.rs,
crates/codeindex/src/tools.rs, crates/routes/src/tools.rs,
crates/artifact/src/tools.rs, crates/tools/src/lib.rs (MCP block),
docs/design/10-knowledge.md and docs/skills.md (examples of the live text).
Tests updated, none deleted: `knowledge::harvest::tests::hints_clip_description_and_omit_empty`,
`roster::tools::tests::hiring_block_reaches_theuth_and_the_desk_manual_points_at_propose`,
`roster::desk::tests::roster_block_lists_active_others_with_capabilities`.
No tool name, schema or machine-parsed text changed; `~/.theuth/prompt.md` untouched.

## Commit 2 — bridge (`agent: bridge a direct call of a runtime function through python (08 amendment)`)

- `repl::indexed(specs, runtime_only)`: the one filter behind `prompt_block`;
  `Turn::rebase` fills `Turn::runtime_names` from it (registry name and its
  `sanitize_ident` form) only when the index was appended (pool present, no
  `toolset` or `toolset.runtime`), so a hidden/capped/flapped name is in
  neither the index nor the set, and `runtime_only: false` leaves it empty.
- In the tool batch (`loop_.rs`, after `ToolCallStarted`): a call whose name
  is not in `self.tools` and is in `runtime_names` is rewritten in place to
  `fn_name = python`, `fn_arguments = {"code": repl::bridge_cell(name, args)}`
  — same call_id, same loop-local python path (`Turn::python` → `Repl::run`,
  so the repl's per-inner-call registry `Invoke` with policy, approvals,
  caps and `inner_calls` accounting is unchanged). Its output gets
  `repl::bridge_note(name)` as the first line; `is_error` is the cell's.
  Any other unknown name: `NAME is not available in this session (not a
  direct tool and not a runtime function you can see)`.
- `repl::bridge_cell`: `import json as _j\n_v = await NAME(**_j.loads(ARGS))\n_v`
  with ARGS from `repl::py_str_literal(serde_json::to_string(args))`; null
  args → `'{}'`; `NAME` is `sanitize_ident(name)`.
- docs/design/08-repl.md: dated amendment at "The floor is enforced, not
  advisory" with the measurement; the example index heading updated.

Tests (crates/agent):
- `loop_::runtime_bridge_tests::guessed_direct_calls_are_bridged_or_refused`
  — through a real `Turn` against the wiremock fake provider, which now can
  answer with one tool call (`Fake::tool_call`) and the harness can carry a
  worker pool (`Harness::pool`): (a) direct `list_dir{path}` → outer
  `ToolCallStarted` keeps the model's name/args, inner `call_bridge_1.1`
  `list_dir` carries the same args through the repl, `ToolCallFinished`
  under the same call_id is not an error, its text is the note line then the
  function's value, and the next provider request's `tool` message starts
  with the note; (b) `nope_tool` refused with the new text, no inner call;
  (c) `list_dir` hidden for the session (`SetHidden`) refused, not run.
  (a)/(c) need a worker (`$THEUTH_WORKER`, else `monty`/`t` on PATH) and
  skip with a note otherwise, like the existing repl tests; ran here with
  `/opt/homebrew/bin/t`.
- `repl::tests::bridge_cell_and_py_str_literal` (quotes, backslashes,
  newlines/tabs, unicode, empty object, null args, a `-` name) and
  `repl::tests::runtime_index_excludes_direct_tools` extended for `indexed`
  in both modes.

## Deviations

- The brief's item (d) "the existing refusal test is updated" — there was no
  test asserting `is not a direct tool here` anywhere in `crates/`; the
  wording existed only in the loop and in docs. Nothing to update; the new
  module covers both refusal texts.
- A bridged `load_skill` does not light the skills badge: the loop's
  `skills::LOAD` check keys on the direct name, and the bridged call is a
  `python` call — the same as when the model calls `load_skill` from python
  today. Not changed here.
- `rustfmt` was run on every touched `.rs` file; three of them
  (`crates/agent/src/supervisor.rs`, `crates/sandbox/src/tools.rs`,
  `crates/schedule/src/tools.rs`) were not fmt-clean before this branch, so
  their reformat was reverted and only the one-string edits kept.
- Besides the sites the brief enumerates, the same convention was applied
  where the grep found bare runtime names in prompt text: the desk manual
  and nudges (`crates/roster/src/desk.rs`), the `## Skills` heading, the
  MCP block, the computer/web/scrape/codeindex/routes/artifact blocks, the
  `send_message`/`spawn_subagent` descriptions (`discover_peers()`,
  `peer_message()`, `load_skill()`), and the `agent` block's `secret_list()`.
  Argument sketches like `task_update disposition: done` were left as they
  are (they are not plain names).
