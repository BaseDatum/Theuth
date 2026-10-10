# SDK frameworks for agent teams — CrewAI, LangGraph, Microsoft Agent Framework vs theuth (recon 2026-10-10)

Clones (shallow, read-only): `crewai` @ 6b93fa0 (2026-10-09, v1.15.27 docs freeze), `langgraph` @ b4991f1 (2026-10-10),
`agent-framework` @ fd52de7 (2026-10-09). Sizes (non-test .py/.cs lines): crewai 535 files / 128,933 lines (315 test files);
langgraph libs 256 files / 84,427 lines (207 test files); agent-framework python 398 files / 194,967 lines (345 test files)
plus dotnet/src 1,042 .cs files / 132,032 lines; 47 ADRs in `docs/decisions`. theuth cites are against main in `/Users/Sky.Moore/Develop/theuth`.

**Cite shorthand.** Full form is `<repo>/<path>#L<a>-L<b>`. After a file's first full mention, a cite may give only the tail:
`crew.py`, `task.py`, `agent/core.py`, `flow/…`, `state/…`, `memory/…`, `telemetry.py`, `base_agent*.py` → `crewai/lib/crewai/src/crewai/…`
(`base_agent.py` = `agents/agent_builder/`, `base_agent_tools.py`/`agent_tools.py` = `tools/agent_tools/`) ·
`types.py`, `pregel/…`, `graph/state.py`, `_internal/_config.py` → `langgraph/libs/langgraph/langgraph/…` ·
`chat_agent_executor.py`, `tool_node.py` → `langgraph/libs/prebuilt/langgraph/prebuilt/…` · `schema.py` → `langgraph/libs/sdk-py/langgraph_sdk/schema.py` ·
`checkpoint/base/__init__.py`, `store/base/__init__.py` → `langgraph/libs/checkpoint/langgraph/…` ·
`_magentic.py`, `_handoff.py`, `_group_chat.py`, `_concurrent.py`, `_sequential.py` → `agent-framework/python/packages/orchestrations/agent_framework_orchestrations/…` ·
`_workflows/…`, `_runner.py`, `_checkpoint.py`, `_workflow.py`, `_tools.py`, `_harness/…`, `observability.py` → `agent-framework/python/packages/core/agent_framework/…` ·
`monty/README.md` → `agent-framework/python/packages/monty/README.md` · ADR `NNNN-….md` → `agent-framework/docs/decisions/NNNN-….md`.

## Executive summary

1. **What they are.** Three libraries for writing a team *in code*; none of them is a control plane over existing harnesses. All three converge on one core: a typed message/state graph run in **supersteps**, **checkpointed per step**, with **human input as a durable pause** that a later call resumes.
2. **CrewAI** (59.5k★, MIT): `role/goal/backstory` agents run Tasks under `Process.sequential | hierarchical`; "hierarchical" is the same sequential loop with a tool-less manager LLM that holds two tools (delegate, ask). **Flows** are an event graph (`@start/@listen/@router`, `or_/and_`), with persistence, event-triggered checkpoints with fork lineage, and an async `@human_feedback` that can **learn lessons** from reviewers.
3. **LangGraph** (43k★, MIT): a Pregel engine. The next step is the union of `Send` packets and nodes whose trigger channels advanced. Writes are saved **per task**. `interrupt()` re-runs the node and replays resume values by index. Time travel works on the checkpoint chain plus `update_state(as_node)`. Each node gets a RetryPolicy and a **two-clock TimeoutPolicy** (hard run cap + idle cap refreshed by heartbeat).
4. **Microsoft Agent Framework** (MAF, 14k★, MIT; successor of AutoGen + Semantic Kernel): superstep workflows checkpointed at every boundary (with a **graph-signature hash**), and `request_info`/`response_handler` HITL that survives restarts. Five orchestration builders: sequential, concurrent, handoff, group chat, **Magentic**. It also ships an experimental "harness" (background agents, plan/execute mode, todos, tool-approval rules), and adapters that run **Claude Agent SDK and GitHub Copilot as agents**.
5. **Idea 1: a progress ledger.** In Magentic, the manager answers five structured questions every round (done? looping? progressing? who next? what instruction?). A stall counter goes +1 or −1 per round, and above 3 the manager resets and replans. This would add an LLM judge of progress on top of theuth's mechanical `no_progress_count` back-off.
6. **Idea 2: liveness and double-texting as policy.** LangGraph's `run_timeout` + `idle_timeout`/`heartbeat()` and its SDK `MultitaskStrategy` (reject / interrupt / rollback / enqueue) would make "is it stuck?" and "what happens to a message that arrives mid-turn?" explicit per-desk settings.
7. **Idea 3: learning from human review.** CrewAI's `learn=True` loop: recall past lessons, pre-review the output with them, collect feedback, then distil generalisable lessons into memory. This maps directly onto theuth's double (126) and dreaming (136).
8. **Principles from MAF's ADRs to copy:** FIDES label-based information-flow control (0024), HMAC-bound approvals (0043), indivisible fail-closed hook seams (0035), "invalidated response ⇒ no side effects" (0041), self-describing storage partitions (0031).
9. **Where theuth is ahead:** org chart and roster, a tasks board with owners, `done_when` and dispositions, routines, a cross-machine mesh, approvals with the double, money budgets with auto-pause, desktop/mobile/chat surfaces. These SDKs have none of that; it is sold separately as platforms (CrewAI AMP, LangGraph Platform/LangSmith, Azure Foundry).
10. **Where they hurt.** LLM-routed delegation is brittle: CrewAI #2838 "Manager agent repeatedly performs tasks", #330 delegation infinite loop. Interrupt semantics bite: LangGraph #7780 extra resumes, #6792 subgraph resume re-runs work. Group chats burn tokens: MAF #754. Default caps are loose: LangGraph's recursion limit is now 10,007, and a MAF group chat with no `max_rounds` never ends.
11. **Verdict:** borrow the orchestration *primitives* (progress ledger, two-clock timeouts, multitask policy, superstep checkpoints with signature checks, HITL as a durable typed request, lesson distillation, a fan-out/fan-in task shape). Adopt none of them as a runtime: theuth's unit of work is a harness session, theirs is an LLM call.
12. **Not verified here:** LangGraph Platform/Studio/A2A endpoint, CrewAI AMP and Foundry hosting behaviour are commercial or outside the clones. Only the open-source code and in-tree docs were read; traction is from the GitHub MCP on 2026-10-10.

## A1 Agent model & identity

### CrewAI
- An agent is a pydantic object with a persona, not a record: `role`, `goal`, `backstory` are its only identity fields
  (`crewai/lib/crewai/src/crewai/agents/agent_builder/base_agent.py#L256-L258`), plus tools, llm,
  `allow_delegation` (default **False**, `#L279-L282`) and `max_iter` (default 25, `#L286-L288`).
- Project scaffolding loads agents/tasks from YAML (`config/agents.yaml`, `config/tasks.yaml`) via `@CrewBase`
  (`crewai/lib/crewai/src/crewai/project/crew_base.py#L153-L156`) — the nearest thing to theuth's `agents/<name>.md`.
- Per-agent `planning`/`reasoning` flags with `max_reasoning_attempts` (`crewai/lib/crewai/src/crewai/agent/core.py#L328-L337`).
- No lifecycle: an agent exists for one `kickoff()`; no hire/pause/retire, no persistent desk. Identity of a *run* is the
  checkpoint lineage (A4), not the agent.

### LangGraph
- There is no agent type in the core library: an "agent" is a node or a compiled subgraph; identity is the graph plus a
  `thread_id` (checkpointer key). The prebuilt ReAct agent carries a managed `remaining_steps` in its state
  (`langgraph/libs/prebuilt/langgraph/prebuilt/chat_agent_executor.py#L62-L74`).
- The SDK models server-side objects (assistants, threads, runs, crons) with explicit statuses:
  `RunStatus = pending|running|error|success|timeout|interrupted`, `ThreadStatus = idle|busy|interrupted|error`
  (`langgraph/libs/sdk-py/langgraph_sdk/schema.py#L23-L34`).

### Microsoft Agent Framework
- `Agent` = chat client + instructions + tools + context providers + middleware; session state is split from behaviour
  so providers are stateless and `AgentSession` serialises on its own (ADR
  `agent-framework/docs/decisions/0018-agentthread-serialization.md#L150-L156`).
- Hosted identity is partitioned by self-describing path segments `a-{agent}/u-{user}/c-{context}`, "so a user id can
  never masquerade as an agent name" (`agent-framework/docs/decisions/0031-hosted-per-user-session-storage-isolation.md#L52-L58`).
- External harnesses are wrapped *as agents*: `agent-framework-claude` (Claude Agent SDK) and
  `agent-framework-github-copilot`, where "the GitHub Copilot SDK owns the tool-calling loop" and approval is enforced
  through the SDK's pre-tool hook (`agent-framework/python/packages/github_copilot/README.md#L11-L20`). This is the closest
  any of the three gets to theuth's ACP children.
- The experimental harness adds an operating **mode** (plan / execute) the agent reads and switches with `mode_get/mode_set`
  (`agent-framework/python/packages/core/agent_framework/_harness/_mode.py#L14-L60`).

**theuth vs these:** theuth's agent is a durable file with charter, `reports_to`, model/effort/tools/mcp/budget and
`paused`, hired through a draft card (design 67) — far ahead on identity and lifecycle. Theirs are ahead on *typed*
per-agent contracts (structured output schemas, explicit run/thread statuses).

## A2 Organisation & authority

### CrewAI
- One level of hierarchy only: a manager over a flat crew. A custom manager **must not have tools** — the code logs,
  clears them and raises (`crewai/lib/crewai/src/crewai/crew.py#L1531-L1542`); the default manager is built from i18n
  role/goal/backstory with `AgentTools(agents=self.agents)` (`crew.py#L1544-L1555`).
- Authority = whom you can name. Delegation resolves the coworker by a case-folded, quote-stripped **role string**
  (`crewai/lib/crewai/src/crewai/tools/agent_tools/base_agent_tools.py#L20-L35`, `#L77-L87`); a miss returns an error string
  listing valid coworkers to the LLM rather than raising (`#L99-L108`).
- No permissions or visibility model beyond each agent's tool list.

### LangGraph
- Hierarchy is structural nesting: a node may only route inside its own graph or **up one level** with
  `Command(graph=Command.PARENT)`; `_control_branch` raises `ParentCommand` (`langgraph/libs/langgraph/langgraph/graph/state.py#L1749-L1775`),
  and the task runner either applies it to the current graph or relabels it for the grandparent and re-raises
  (`langgraph/libs/langgraph/langgraph/pregel/_retry.py#L618-L631`).
- The "supervisor" package that popularised the boss pattern, `langchain-ai/langgraph-supervisor-py`, is **archived**
  (GitHub MCP: archived=true, 1,655★); `langgraph-swarm-py` (1,577★) is live. Neither is in this clone.

### Microsoft Agent Framework
- Routing authority sits in orchestrator executors (group chat manager, Magentic manager). Handoff targets are an explicit
  per-agent allow-list: a call to an unknown target raises `ValueError` naming the valid ones
  (`agent-framework/python/packages/orchestrations/agent_framework_orchestrations/_handoff.py#L455-L461`).
- Isolation dimensions are first-class: `AgentSessionStoreKey.Partitions` (ADR `0039-shared-agent-session-store.md#L37-L48`).

**theuth vs these:** theuth has a real org chart (`reports_to`), escalation up the line, reviewers and the double — far
ahead. Worth borrowing: MAF's *explicit handoff allow-list* ("may hand off to: …") as a roster field, and LangGraph's
"route only within your graph or to your parent" as the rule for desk-to-desk routing.

## A3 Work model

### CrewAI
- `Task`: description, `expected_output`, agent, `context` (other Tasks whose outputs are injected), `async_execution`,
  `output_pydantic`, `guardrail`/`guardrails` (function **or a plain-language string** checked by an LLM),
  `guardrail_max_retries` = 3, `human_input` (`crewai/lib/crewai/src/crewai/task.py#L233-L282`).
- `ConditionalTask` skips on a predicate over prior outputs (`crew.py#L1602-L1608`); `planning=True` runs a `CrewPlanner`
  that writes a step plan into each task before kickoff (`crew.py#L1464-L1468`).
- No backlog, no states, no assignment outside the crew definition.

### LangGraph
- Work is graph state on a thread; runs carry statuses incl. `timeout` and `interrupted` (`schema.py#L23`). No task/ticket
  abstraction in the library.

### Microsoft Agent Framework
- Magentic keeps a **task ledger** (facts + plan) and a per-round **progress ledger** (`_magentic.py#L274-L317`); the
  harness has a todo provider (`agent_framework/_harness/_todo.py`, 588 lines) and a plan-mode script that says "write the
  plan to a memory file, so that it is retained even if compaction happens" (`_harness/_mode.py#L57-L58`).

**theuth vs these:** theuth's board (owner, reviewer, `done_when`, checkout, dispositions, blockers, threads) is far
ahead. Two techniques worth taking: CrewAI's **guardrail-as-acceptance** (a function or a sentence judged by an LLM,
retried ≤ N with the failure fed back) as an executable form of `done_when`; and Magentic's split of a stable *task
ledger* from a per-round *progress ledger*.

## A4 Execution & runtime

### CrewAI
- A crew run is one synchronous loop over the task list (`crew.py#L1571-L1640`): `async_execution` tasks become futures
  that are joined at the next synchronous task (`#L1610-L1625`) — a fan-out/fan-in without a scheduler.
- **Resume** = skip to the first task without an output: `_get_execution_start_index` (`crew.py#L1563-L1569`).
- **Checkpoints** are a side effect of the event bus: `CheckpointConfig(on_events=["task_completed"], provider=Json|Sqlite,
  max_checkpoints, restore_from)` (`crewai/lib/crewai/src/crewai/state/checkpoint_config.py#L160-L194`); `RuntimeState.fork()`
  starts a named branch recording parent branch and parent checkpoint id (`crewai/lib/crewai/src/crewai/state/runtime.py#L352-L389`).
- Flows persist method-level state with `@persist` / `SQLiteFlowPersistence` (`flow/persistence/decorators.py#L147`,
  `flow/persistence/sqlite.py#L44`); on resume, completed methods are skipped but their listeners still fire
  (`flow/runtime/__init__.py#L3348-L3351`).
- No placement, workers, liveness or heartbeat; concurrency is threads/asyncio inside one process.

### LangGraph
- **Who runs next** (`langgraph/libs/langgraph/langgraph/pregel/_algo.py#L391-L512`): "the union of all PUSH tasks (Sends)
  and PULL tasks (nodes triggered by edges)" (`#L432-L434`); a PULL node fires when any trigger channel's version is newer
  than the version that node last saw (`_triggers`, `#L1259-L1276`); candidates are sorted for determinism (`#L474-L481`).
- **Per-task durability:** `put_writes` stores each task's writes as soon as it finishes (`pregel/_loop.py#L446-L505`), so a
  crash mid-superstep re-runs only the unfinished tasks of that step. `Durability = sync|async|exit` trades latency for
  safety (`langgraph/types.py#L98-L104`).
- **Retry** per node: `RetryPolicy(initial_interval=0.5, backoff_factor=2, max_interval=128, max_attempts=3, jitter, retry_on)`
  (`types.py#L427-L446`).
- **Two-clock timeout** per node attempt: `run_timeout` ("never refreshed by progress signals or `runtime.heartbeat()`")
  and `idle_timeout` ("without observable progress"), `refresh_on = auto|heartbeat` (`types.py#L461-L490`); a single `Send`
  can carry its own timeout (`types.py#L785-L810`).
- Server semantics in the SDK: `MultitaskStrategy = reject|interrupt|rollback|enqueue` for input that arrives while a thread
  is busy, `DisconnectMode = cancel|continue` (`langgraph/libs/sdk-py/langgraph_sdk/schema.py#L74-L88`).
- Checkpointer API includes `copy_thread`, `prune(strategy=keep_latest|delete)` and `delete_for_runs`, with a warning that
  delta-encoded channels need the full parent chain (`langgraph/libs/checkpoint/langgraph/checkpoint/base/__init__.py#L344-L423`).

### Microsoft Agent Framework
- `RunnerImpl.run_until_convergence`: Pregel supersteps; state is committed and a checkpoint written at **every superstep
  boundary**; stops when no messages remain; `max_iterations` (default 100) exceeded with messages pending raises
  `WorkflowConvergenceException` (`agent-framework/python/packages/core/agent_framework/_workflows/_runner.py#L108-L179`).
- `WorkflowCheckpoint` stores messages in transit, committed state only (executor state + partially filled fan-in buffers),
  **pending request_info events**, a `previous_checkpoint_id` lineage, and a `graph_signature_hash` "to validate checkpoint
  compatibility during restore" (`_workflows/_checkpoint.py#L35-L103`); ordering is by lineage, not iteration count (`#L67-L77`).
- Hosted long-running agents: "resilient background" + "steerable conversations" must be switched on at registration, not
  later (ADR `0035-foundry-hosting-resilient-long-running-agents.md#L40-L47`). History persistence per service call is opt-in
  for recoverability (ADR `0022-chat-history-persistence-consistency.md#L74-L76`).
- Harness background agents: `running|completed|failed|lost`, default wait 300 s (`_harness/_background_agents.py#L29-L53`).

**theuth vs these:** theuth is ahead on the machine-level runtime: durable sessions with journal + eager resume (109),
ACP children respawned, mesh placement, sandboxes, routines, reminders. Behind on **step-level** checkpoints *of a
team run*: there is no "fork this multi-agent run at step K and replay", no topology-signature check when a routine or
loop definition changed under a paused run, and no per-child two-clock timeout (a hard cap and an idle cap).

## A5 Coordination between agents

### CrewAI — Process, delegation, Flows
- **Sequential**: tasks in list order, each by `task.agent`, with prior outputs as context (`crew.py#L1522-L1524`, `#L1627-L1633`).
- **Hierarchical**: *the same loop*, but every task is executed by the manager (`_get_agent_to_use` returns `manager_agent`,
  `crew.py#L1727-L1730`); the manager gets exactly two tools, delegate-work and ask-question, whose descriptions list the
  coworkers' roles (`crewai/lib/crewai/src/crewai/tools/agent_tools/agent_tools.py#L22-L36`). Task *order* stays fixed;
  "who acts next" is the manager LLM's tool call, one per task.
- A delegation builds a throw-away `Task(description=task, agent=selected, expected_output=manager_request)` and runs
  `selected_agent.execute_task(...)` synchronously; exceptions come back as text (`base_agent_tools.py#L110-L124`). No
  record of the sub-task survives beyond the tool result.
- `Process.consensual` is still a TODO (`crewai/lib/crewai/src/crewai/process.py#L4-L11`).
- **Flows** (the production path): listeners fire when their condition — a method name, a router label, `or_(...)` or
  `and_(...)` (`flow/dsl/_conditions.py#L22-L30`) — is met by the method that just finished (`_find_triggered_methods`,
  `flow/runtime/__init__.py#L3284-L3305`). Routers return a label; `@listen(label)` methods run next. So in Flows,
  "who acts next" is ordinary code, and LLM judgement is confined to the steps.
- Loop defence in Flows: `max_method_calls = 100` per method; exceeding it raises `RecursionError` with the hint "a @listen
  label matches the method's own name" (`flow/runtime/__init__.py#L644`, `#L3338-L3346`).

### LangGraph — edges, Send, Command, subgraphs
- Fan-out: return `[Send("node", sub_state) ...]` from a conditional edge to run one node many times in parallel on
  different inputs, with a reducer (e.g. `operator.add`) joining results (`langgraph/types.py#L738-L783`).
- **Handoff = `Command`**: `Command(goto=..., update=..., resume=..., graph=None|PARENT)` (`types.py#L833-L858`);
  `_control_branch` turns `goto` into `Send`s or branch writes, and `graph=PARENT` into a `ParentCommand` exception that
  bubbles out of the subgraph (`graph/state.py#L1749-L1775`; runner side `pregel/_retry.py#L618-L631`).
- Parallel tool calls that each hand off are merged into a *single* parent `Command` with all `Send`s
  (`langgraph/libs/prebuilt/langgraph/prebuilt/tool_node.py#L903-L918`) — a careful answer to "two agents asked for two
  handoffs in the same turn".
- Supervisor/swarm topologies are built on these two primitives (separate repos; supervisor archived, see A2).
- "Who decides next" is deterministic given state: Sends ∪ triggered nodes; any LLM choice must be expressed as a
  `Command`/`Send` emitted by a node.

### Microsoft Agent Framework — five builders
- **Sequential** (`_sequential.py`, 277 lines) and **Concurrent**: "dispatcher -> fan-out -> participants -> fan-in -> aggregator",
  the aggregator replaceable by an executor or a callback (`agent_framework_orchestrations/_concurrent.py#L180-L204`).
- **Group chat** (centralised): next speaker from a `selection_func` (round robin etc.) or from an orchestrator agent forced
  into strict structured output `{terminate, reason, next_speaker, final_message}` (`_group_chat.py#L264-L283`); an unknown
  name raises (`#L241-L256`); with neither `max_rounds` nor a `termination_condition` "the conversation will continue
  indefinitely" (`#L141-L145`).
- **Handoff** (decentralised): each agent gets generated `handoff_to_<id>` tools (`_handoff.py#L136`); "the entire
  conversation is maintained and reused on every hop"; by default the workflow asks the human after any reply without a
  handoff; in autonomous mode it re-prompts "User did not respond. Continue assisting autonomously." up to 50 turns
  (`_handoff.py#L20-L29`, `#L235-L238`, `#L478-L496`).
- **Magentic** (manager with ledgers): plan → inner loop. Each round the manager produces a progress ledger answering:
  request satisfied? in a loop? making progress? next speaker (from the names)? instruction for them?
  (`_magentic.py#L205-L244`). Satisfied → final answer; no progress or looping → `stall_count += 1`, else `-1` (floor 0);
  above `max_stall_count` (default 3) → **reset and replan**; an invalid next speaker ends with a final answer
  (`_magentic.py#L1115-L1143`, defaults `#L474-L486`). Reset clears chat history and stall count but keeps the task and the
  round count (`#L388-L396`).
- Magentic **plan review**: the plan is sent to a human as `MagenticPlanReviewRequest` (approve / revise with feedback), and
  re-sent when the run stalls (`_magentic.py#L811-L863`, `#L1043-L1053`).

**Who decides who works next — summary.** CrewAI crews: the manager LLM, one delegation per fixed-order task. CrewAI
flows: code (routers). LangGraph: the state machine (Sends ∪ triggered nodes), with LLM choices expressed as
`Command`s. MAF: a pluggable selector (function or structured-output agent) in group chat, the agents themselves in
handoff, and in Magentic an LLM progress ledger with a stall counter. **theuth today:** wakes and dispatch per task owner
(67 §7.3, design 70), `send_message`, checkout. A desk's no-progress is mechanical — "no field change since delivery",
two in a row start an exponential back-off (`theuth/crates/tasks/src/ledger.rs#L2075-L2089`). Nothing judges whether a
team is *looping* semantically, and nothing replans when it stalls.

## A6 Human in the loop & governance

### CrewAI
- `Task.human_input=True`: a human reviews the agent's final answer before the task closes (`task.py#L233-L236`).
- **Guardrails**: a function or a plain-language rule, checked after each output and retried with the failure up to
  `guardrail_max_retries=3` (`task.py#L252-L282`); agents have the same knob (`agent/core.py#L362-L373`, failure message at `#L2126-L2128`).
- **`@human_feedback`** on a Flow step: a message, the `emit` outcomes, an LLM that maps free-text feedback onto one
  outcome (exact match, else the longest outcome contained in the text, else `HumanFeedbackCollapseError`)
  (`flow/human_feedback.py#L87-L110`), a `default_outcome`, and a pluggable `provider` (`#L188-L215`).
- **Async HITL**: a provider (Slack, email, …) raises `HumanFeedbackPending` — "Not an error, a control flow signal"
  (`flow/async_feedback/types.py#L163`). The flow saves a *pending feedback* record and returns; later
  `Flow.from_pending(flow_id)` + `resume(feedback)` continues (`flow/runtime/__init__.py#L1257-L1305`, `#L1345`).
- **Learning from reviews** (`learn=True`): before the human sees an output, memories with `source="hitl"` for that method
  are recalled and an LLM pre-reviews the output against them (`flow/human_feedback.py#L279-L296`). After the feedback, an LLM
  distils "generalizable lessons … reusable rule or preference" (`DistilledLessons`, `#L226-L236`) and stores them
  (`#L337-L369`). `learn_strict` decides whether a failure in this path aborts or falls back to the raw output (`#L321-L334`).

### LangGraph
- `interrupt(value, response_schema=...)` inside a node raises `GraphInterrupt` with a namespaced `Interrupt`. Resume
  is `Command(resume=...)`: the node **re-executes from its start**, and each `interrupt()` call returns the stored resume
  value with its index (`langgraph/types.py#L1019-L1046`). A resume that fails the schema saves nothing, "so the interrupt can be
  answered again" (`#L1001-L1002`). Interrupts need a checkpointer.
- Static breakpoints: `interrupt_before_nodes` / `interrupt_after` (`pregel/main.py#L733`), loop statuses
  `interrupt_before|interrupt_after` (`pregel/_loop.py#L701`, `#L785`).
- **Time travel**: `get_state_history(config, filter, before, limit)` walks the checkpoint chain (`pregel/main.py#L1514-L1522`);
  `update_state(config, values, as_node)` writes a new checkpoint "as if they came from node `as_node`"
  (`pregel/main.py#L2617-L2628`). Invoking from an older checkpoint id forks the thread.

### Microsoft Agent Framework
- Any executor can call `ctx.request_info(request, response_type)` and implement `@response_handler`. The workflow emits
  `request_info` events and goes to `IDLE_WITH_PENDING_REQUESTS`; the caller answers with `run(responses=...)`
  (`_workflows/_workflow.py#L289-L296`). Requests in flight are saved in the checkpoint (`_checkpoint.py#L64-L66`), so a
  question survives a restart.
- Function approval travels *in the message stream* as approval-request/response content (ADR `0006-userapproval.md#L383-L393`).
- Harness approval rules are scoped `tool` or **`tool_with_arguments`** (canonical-JSON exact match, so `{}` never becomes a
  wildcard) (`_harness/_tool_approval.py#L25-L57`).
- Governance ADRs: **FIDES** labels every content item for integrity (TRUSTED/UNTRUSTED) and confidentiality, with
  most-restrictive-wins, enforcement middleware before tool calls, and *variable indirection* that keeps untrusted text out
  of the model's context (ADR `0024-prompt-injection-defense.md#L36-L44`). Hook seams are composed by one factory in a fixed
  order and fail closed on bypass attempts (ADR `0035-dotnet-agent-hooks-enforcement.md#L22-L26`). MCP approvals carry
  header names plus an HMAC over request id and canonical headers; the key stays in trusted host state "never in approval
  payloads" (ADR `0043-python-mcp-runtime-context.md#L27-L35`).
- Caveat from the Monty CodeAct package: FIDES is **not** enforced inside generated code, because host-tool calls there
  bypass the function middleware (`agent-framework/python/packages/monty/README.md#L8-L18`).

**theuth vs these:** theuth's approvals (`tools.policy`), the double (126) answering as the owner, ask_user/handoff
cards, attention inbox, mobile and chat channels, and the review state on tasks are a full governance surface; none of
the three ships one. Worth borrowing: (a) schema-validated answers to a question, where a bad answer leaves it open;
(b) an "always allow *these exact arguments*" rule scope; (c) lesson distillation from review feedback;
(d) FIDES-style labels instead of prompt-only "this is data, not instructions".

## A7 Cost, budgets & runaway control

### CrewAI
- Per agent: `max_iter` 25 (`base_agent.py#L286-L288`), `max_rpm` (`#L275-L278`), `max_execution_time` (`agent/core.py#L258`),
  `max_retry_limit` (`#L302`), `respect_context_window` (`#L298`); per task: `guardrail_max_retries` (`task.py#L279-L281`); per flow:
  `max_method_calls` 100 (`flow/runtime/__init__.py#L644`). Token usage is summed from the manager and the agents
  (`crew.py#L2287-L2293`). Nothing measures money or sets a budget.

### LangGraph
- `recursion_limit` sets `stop = step + recursion_limit + 1` (`pregel/_loop.py#L1836-L1838`), and the tick sets
  `status = "out_of_steps"` past it (`#L638-L639`). The **default is now 10,007** (env `LANGGRAPH_DEFAULT_RECURSION_LIMIT`,
  `langgraph/_internal/_config.py#L32`), so in practice a loop is no longer stopped by default.
- `RemainingSteps` lets an agent stop gracefully *before* the cap: the ReAct agent answers "need more steps" when
  `remaining_steps < 2` and tool calls are pending (`chat_agent_executor.py#L627-L631`).
- RetryPolicy with jittered backoff; TimeoutPolicy run/idle (A4).

### Microsoft Agent Framework
- Function-invocation loop budgets: `max_iterations` (model round-trips, default 40, `_tools.py#L104`), `max_function_calls`
  ("the primary knob for controlling cost"), `max_duration_seconds` (wall clock, **including time spent waiting for human
  approval**, "so that long-running unattended sessions are always bounded"), `max_consecutive_errors_per_request`. At a
  cap the loop *disables tools and forces a text answer* rather than failing. The limits are best-effort, checked after
  each parallel batch (`_tools.py#L1770-L1799`).
- Orchestration caps: workflow `max_iterations` 100 (`_runner.py#L55`, `#L178-L179`); Magentic `max_round_count`,
  `max_stall_count`=3, `max_reset_count` (`_magentic.py#L474-L486`); handoff autonomous limit 50 (`_handoff.py#L238`); group chat
  unbounded unless configured (`_group_chat.py#L141-L145`).
- ADR 0041: when a provider invalidates a partial response, abandon the iteration, clear request budget state, restore the
  last valid continuation, and run **no** local side effects (`0041-handle-provider-invalidated-responses.md#L50-L56`).

**theuth vs these:** theuth is ahead on money: per-agent budgets with auto-pause, a spend ledger and cost attribution.
It is behind on *graceful* caps. Two techniques to borrow: "tools off, answer now" degradation when a cap is reached,
and a wall-clock budget that counts approval waits. Also worth copying: MAF's stall counter, which decays (−1 per good
round) instead of resetting to zero.

## A8 Memory, knowledge & learning

### CrewAI
- **Docs and code disagree; trust the code.** `docs/edge/en/concepts/memory.mdx#L10` says one unified `Memory` "replaces
  separate short-term, long-term, entity, and external memory types". But `docs/edge/en/concepts/crews.mdx#L24` and `#L315`
  still advertise short/long/entity memory. The code has only the unified class (`crewai/lib/crewai/src/crewai/memory/`:
  `unified_memory.py`, `recall_flow.py`, `encoding_flow.py`, `memory_scope.py`).
- `Memory`: an LLM infers scope, categories and importance on save. Recall scores are a composite of **0.5 semantic + 0.3
  recency + 0.2 importance**, with a recency half-life of 30 days. On save, records above 0.85 similarity are consolidated
  (comparing against at most 5). Recall depth adapts: confidence ≥ 0.8 returns at once, < 0.5 explores deeper
  (`memory/unified_memory.py#L76-L135`). `root_scope` nests memories, e.g. `/crew/research/<inferred>` (`#L157-L162`).
- HITL lessons are stored as memories with `source="hitl"` (A6), so review feedback becomes recall material.

### LangGraph
- Short-term memory = the thread's checkpoints. Long-term memory = `BaseStore`: items under tuple namespaces,
  `search` with filters and an optional semantic index (`IndexConfig`), and **TTL with refresh-on-read**
  (`langgraph/libs/checkpoint/langgraph/store/base/__init__.py#L545-L563`, `#L578`, `#L708-L728`).
- Delta-encoded channels: `DeltaChannel` writes only a sentinel per step plus a full snapshot every `snapshot_frequency`
  updates, and reconstruction walks the parent chain (`checkpoint/base/__init__.py#L411-L423`). This is how message
  history stays cheap in checkpoints.

### Microsoft Agent Framework
- Context providers + middleware (ADR 0016) and a standalone `CompactionStrategy` object that annotates messages instead
  of keeping side containers (ADR `0019-python-context-compaction-strategy.md#L996-L998`). Memory backends ship as packages
  (mem0, redis, cosmos, postgres, qdrant…). The harness has file memory, and plan mode writes the plan to a memory file to
  survive compaction (`_harness/_mode.py#L57-L58`).
- Learning from outcomes: `packages/lab` (gaia, tau2, lightning RL training samples). Evals via `_evaluation.py` and the
  Foundry evals ADR 0023.

**theuth vs these:** theuth's knowledge base + nightly dreaming (136) + endless summary tree (137) is broader than any
of them. Worth borrowing: a composite recency/importance/semantic score with a save-time consolidation threshold for
`knowledge_search` ranking; TTL with refresh-on-read for low-value facts; HITL lesson capture (A6); and the
delta-plus-snapshot encoding idea for `sessions.sqlite` growth.

## A9 Observability & UX

### CrewAI
- An event bus (`crewai/events/`) carries kickoff/task/tool/checkpoint/fork events. Checkpointing itself is a listener on
  that bus (A4). `Flow.plot()` writes an HTML graph (`flow/runtime/__init__.py#L3963`). The UI is the commercial AMP product.
- Anonymous telemetry is **on by default**, opt-in `share_crew` sends more (`crewai/lib/crewai/src/crewai/telemetry/telemetry.py#L1-L7`).
  A code comment records an incident: installing a global TracerProvider made "every OTel-instrumented library in the host
  process … export to our collector" (`telemetry.py#L141-L150`).

### LangGraph
- Streaming is the UX contract (`StreamMode`, `langgraph_sdk/schema.py#L51`) and `debug` checkpoints/tasks events. Studio
  and LangSmith are commercial, not in the clone (not verified here).

### Microsoft Agent Framework
- OpenTelemetry GenAI semantic conventions (`gen_ai.operation.name`, `invoke_agent`) (`agent_framework/observability.py#L231`,
  `#L345`). Message content is recorded only with `enable_sensitive_data` / `ENABLE_SENSITIVE_DATA` (`#L1007-L1016`).
- Typed workflow events: `superstep_started/completed` (`_runner.py#L125`, `#L170`), `handoff_sent` (`_handoff.py#L472-L474`),
  `PROGRESS_LEDGER_UPDATED` carrying the whole ledger (`_magentic.py#L1099-L1108`).
- **DevUI** for local debugging: `devui ./agents` serves registered agents/workflows
  (`agent-framework/python/packages/devui/agent_framework_devui/__init__.py#L45-L92`).

**theuth vs these:** theuth is far ahead on what the human sees: desktop/web/mobile/TUI, Slack/Signal/Telegram, HUD and
attention inbox, tasks board, org chart, activity monitor (155). Worth borrowing is the *event vocabulary*. Publish
"progress ledger updated", "handoff sent" and "step N committed" as typed activity events, so the monitor can show *why*
a team is stalled, not only that it is quiet.

## A10 Extensibility & integration

### CrewAI
- MCP client with transports, filters and a tool resolver (`crewai/lib/crewai/src/crewai/mcp/`). A2A client/server with
  extensions and auth (`crewai/lib/crewai/src/crewai/a2a/`). An open issue shows A2A delegation fragility: #4166 "Second A2A
  delegation fails — task is in terminal state ('completed')". Pluggable checkpoint providers (Json/Sqlite), memory
  storage (LanceDB default, Qdrant edge) and hooks.

### LangGraph
- No MCP or A2A in `libs/` (grep for `mcp`/`a2a` in `libs/langgraph/langgraph` returned nothing). MCP comes from the
  separate `langchain-mcp-adapters`. An A2A endpoint is a platform feature, not verified here. Extensibility is the graph
  itself plus checkpointer and store backends (memory, sqlite, postgres) and a conformance suite
  (`libs/checkpoint-conformance`).

### Microsoft Agent Framework
- The widest surface: `a2a`, `hosting-a2a`, `hosting-mcp`, `hosting-responses`, `hosting-telegram`, `ag-ui`, `chatkit`,
  `copilotstudio`, `claude`, `github_copilot`, `monty`, `declarative` (YAML agents/workflows), `purview`, many model and
  memory providers (`python/packages/`, 43 dirs).
- Hosting channels decision: protocol packages only parse input and render output, plus optional execution state and id
  helpers like `telegram_session_id(update)` (ADR `0027-hosting-channels.md#L64-L71`).
- Skills: a multi-source skills design and "skills over MCP" (ADR 0037, ADR 0029). CodeAct: one `execute_code` tool with
  **bundled approval** for the whole invocation (ADR `0038-codeact-integration.md#L139-L146`).

**theuth vs these:** theuth is at parity or ahead: gated MCP per server/tool, ACP harnesses (omp, opencode), A2A at the
boundary, three chat channels, skills and an app tool. MAF is the only one that wraps *external coding harnesses* as
agents (Claude Agent SDK, Copilot) and the only one using **Monty** for CodeAct, the sandbox theuth's python runtime
uses. Its README's warning that label policies do not reach tool calls made inside generated code applies to theuth too.

## A11 Engineering, licence & traction

### CrewAI
- Python, MIT. 59,527★, 8,678 forks, 609 open issues+PRs, created 2023-10-27 (GitHub MCP, 2026-10-10). 535 source files /
  128,933 lines, 315 test files. Weekly-ish releases (docs freeze for v1.15.27 at HEAD). Business: CrewAI AMP (enterprise
  platform) plus default-on telemetry.

### LangGraph
- Python (+ JS SDK), MIT. 43,012★, 7,308 forks, 801 open, created 2023-08-09. libs: 256 files / 84,427 lines, 207 test files,
  plus a checkpointer conformance suite. Business: LangGraph Platform / LangSmith. Repo topics now lead with `deepagents`.

### Microsoft Agent Framework
- Python + .NET, MIT. 14,044★, 2,451 forks, 761 open, created 2025-04-28. Python 398 files / 194,967 lines (345 test files),
  .NET 1,042 files / 132,032 lines, 47 ADRs. Business: Azure AI Foundry hosting. Many APIs are marked experimental.

### Context: AutoGen, MetaGPT, ChatDev
- `microsoft/autogen` 61,340★, 1,102 open. MAF is its successor, and Magentic-One's ledger loop lives on in MAF's
  `_magentic.py`. MetaGPT (70.8k★) and ChatDev (34.5k★) encode a *software company* as fixed role SOPs, the same "org
  as code" idea as CrewAI's roles, with no runtime governance (from AXES; not cloned).

## ADRs worth copying as principles (agent-framework/docs/decisions)

47 ADRs plus two templates; most are SDK API shape. These carry over to a meta-harness (techniques only):

1. **0024 FIDES, deterministic prompt-injection defence** (`0024-prompt-injection-defense.md#L36-L44`). Label every content
   item (integrity, confidentiality; most-restrictive-wins), propagate labels through tool calls, enforce policy *before*
   a tool runs, and hand the model variable references instead of untrusted text. *For theuth:* today "screen text,
   doc-member blocks, peer bodies are data" is a prompt rule. A label on tool results (web, mail, peer, doc-member), plus
   a policy check in `tools.policy` for any mutating call made while untrusted labels are in context, would make it
   deterministic.
2. **0043 approvals bound by HMAC** (`0043-python-mcp-runtime-context.md#L27-L35`). An approval names exactly what was
   approved (request id + canonical headers), and the binding key never travels with the approval. *For theuth:* bind each
   approval/double answer to a hash of the tool name + canonical args. A replayed or edited call then cannot reuse it.
3. **0035 hooks: one factory, fixed seam order, fail closed** (`0035-dotnet-agent-hooks-enforcement.md#L22-L26`).
   Enforcement points are composed by one constructor that cannot be partially bypassed. *For theuth:* the approval →
   double → policy → execution chain should have one composition point, so ACP children and native tools cannot diverge.
4. **0041 an invalidated response has no side effects** (`0041-handle-provider-invalidated-responses.md#L50-L56`). Execute
   usable calls by default; when the provider says the response was invalidated, drop the whole iteration, its budget
   state and its persistence. This pairs with theuth's 109 rule of repairing dangling calls as "not run".
5. **0031 self-describing partitions** (`0031-hosted-per-user-session-storage-isolation.md#L52-L58`). Storage paths use
   prefixed segments (`a-`, `u-`, `c-`), so ids from different namespaces can never be confused. Applies to theuth's mesh
   paths, spill and briefs folders.
6. **0022 opt-in per-call persistence for recoverability** (`0022-chat-history-persistence-consistency.md#L74-L76`). Keep
   the cheap per-run default and let long runs opt in to persistence after every model call.
7. **0018 state separate from behaviour** (`0018-agentthread-serialization.md#L150-L156`). Providers are stateless, so
   one instance serves any number of sessions and a session serialises on its own.
8. **0035 (Foundry) resilience is a registration-time choice** (`0035-foundry-hosting-resilient-long-running-agents.md#L40-L47`).
   Durability and steering are fixed when a run is registered, not toggled later.
9. **0038 CodeAct: bundled approval for `execute_code`** (`0038-codeact-integration.md#L139-L146`), with the caveat from the
   Monty README that label policies do not reach calls inside generated code (A6). theuth's per-call approval inside
   python is stronger. Keep it.
10. **0019 compaction carried on the messages themselves** (`0019-python-context-compaction-strategy.md#L996-L998`), not in
   side containers. This is consistent with theuth's endless summary-tree approach.

## Map to the three questions

### Q1 What do they offer versus theuth?

| Axis | Theirs (best of the three) | theuth | Gap |
|---|---|---|---|
| A1 identity | persona objects; typed run/thread statuses (LangGraph SDK); harness-as-agent adapters (MAF) | agent files, roster, hire cards, paused | theuth ahead; borrow typed run statuses |
| A2 authority | structural (subgraph → parent only), handoff allow-lists | org chart, reports_to, escalate, reviewer, double | theuth ahead; borrow allow-list |
| A3 work | Task + expected_output + guardrails; Magentic task/progress ledgers | board, done_when, dispositions, blockers | theuth ahead; borrow guardrail-as-check |
| A4 runtime | per-superstep / per-task checkpoints, fork, graph signature, run+idle timeouts, multitask strategy | 109 durability, mesh, sandboxes, ACP respawn | mixed: theuth ahead at machine level, behind at team-run step level |
| A5 coordination | Send fan-out, Command handoffs, group-chat selector, **Magentic progress ledger** | send_message, wakes, checkout, mechanical no-progress back-off | **gap: semantic stall detection + replan; fan-out/fan-in shape** |
| A6 HITL | durable typed requests, schema-validated resume, plan review, lesson distillation, FIDES | approvals, double, ask_user, inbox, mobile | theuth ahead on surface; borrow validation, arg-scoped rules, lessons |
| A7 runaway | graceful caps (tools off → answer), duration incl. approval waits, decaying stall counter | budgets + auto-pause, spend ledger | theuth ahead on money; borrow graceful degradation |
| A8 memory | composite recall score, consolidation, TTL, delta checkpoints | knowledge + dreaming + endless | theuth broader; borrow scoring/consolidation |
| A9 UX | DevUI, plots, OTel GenAI | desktop/mobile/HUD/monitor | theuth far ahead; borrow event vocabulary |
| A10 integration | MCP, A2A, channels, CodeAct on Monty | MCP gates, ACP, A2A, 3 channels | parity |
| A11 traction | 59.5k / 43k / 14k★, big teams, paid platforms | single-user product | n/a |

### Q2 What can we learn / borrow? (techniques only)

1. **Progress ledger with a decaying stall counter.** Each round a judge answers {satisfied, looping, progressing, next,
   instruction} in strict JSON. Stall is +1 when not progressing or looping and −1 otherwise. Above N the team resets its
   working context and replans; a plan review can go to the human. Cite `_magentic.py#L205-L244`, `#L1115-L1143`.
   *Cost if wrong:* an extra cheap model call per supervisor pass; a false "stalled" verdict triggers a needless replan.
   Mitigate with a threshold of 3 and a human-visible ledger.
2. **Two-clock liveness.** `run_timeout` (hard) + `idle_timeout` refreshed by progress events or an explicit heartbeat
   (`langgraph/types.py#L461-L490`). *Cost if wrong:* killing a slow but healthy child. Mitigate with defaults far above
   today's p95 and by counting tool/ACP output as progress.
3. **Multitask strategy per desk/session**: reject | interrupt | rollback | enqueue for input arriving mid-turn
   (`langgraph_sdk/schema.py#L81-L88`). *Cost if wrong:* a dropped or reordered message. Make enqueue the default and keep
   today's "joins the running turn" as `interrupt`.
4. **HITL lesson distillation.** Before review: recall lessons for this kind of output and pre-review with them. After
   review: distil generalisable rules into memory with a source tag (`flow/human_feedback.py#L279-L296`, `#L337-L369`).
   *Cost if wrong:* bad lessons bias future work. Route them through dreaming's replay gate (136) before they reach
   hints.
5. **Durable typed question.** A request with a response schema; an invalid answer saves nothing and leaves it open
   (`langgraph/types.py#L1001-L1002`, `#L1019-L1046`); pending requests live in the checkpoint (`_checkpoint.py#L64-L66`).
   *Cost if wrong:* low. theuth already re-arms questions after restart (109); this adds the validation.
6. **Graceful cap degradation.** At max calls or max duration, disable tools and force a final text answer instead of
   failing; let the duration budget include approval waits (`_tools.py#L1770-L1799`). *Cost if wrong:* a premature
   "final" answer. Label it as cap-truncated.
7. **Fan-out/fan-in as one task shape.** `Send` map-reduce (`types.py#L738-L783`) and MAF's dispatcher → participants →
   aggregator (`_concurrent.py#L180-L204`). *For theuth:* a parent task that spawns N child tasks from a list and an
   aggregator step that runs when all N are done. *Cost if wrong:* N× spend. Gate it with the budget.
8. **Checkpoint compatibility hash.** Store a hash of the definition (routine file, loop brief, team topology) with each
   durable checkpoint and refuse or warn on resume after a change (`_checkpoint.py#L52-L53`, `#L86-L87`). *Cost if wrong:*
   a spurious refusal. Make it a warning first.
9. **Fork/time-travel of a team run.** Lineage ids on checkpoints, fork into a named branch (`crewai/state/runtime.py#L352-L389`),
   replay from an older checkpoint (`pregel/main.py#L1514-L1522`, `#L2617-L2628`). *Cost if wrong:* storage growth and
   confusing branches. Limit it to autoresearch (140) and routines first.
10. **Handoff allow-list + parallel-handoff merge.** "May hand off to" per agent (`_handoff.py#L455-L461`). Merge
    simultaneous handoffs into one parent command (`tool_node.py#L903-L918`). *Cost if wrong:* low.
11. **Information-flow labels (FIDES) and approval hashes (ADR 0024/0043).** *Cost if wrong:* false blocks on mutating
    calls after reading untrusted content. Ship it as observe-only first.
12. **Composite memory score + consolidation** (`memory/unified_memory.py#L100-L135`). *Cost if wrong:* ranking churn in
    hints. A/B it on `knowledge_search`.

### Q3 How should theuth improve? (ranked by value ÷ effort)

1. **Progress ledger in the supervisor/desk wake path** — high value, small-to-medium effort. After a wake completes with
   no field change (today's `no_progress_count`, `theuth/crates/tasks/src/ledger.rs#L2075-L2089`), run one cheap
   structured judgement over the task thread. Feed a decaying stall score; at the threshold, escalate up the reporting
   line with the ledger attached, instead of only backing off. Lands in `crates/tasks` (ledger), the desk protocol
   (design 70) and the activity monitor (155).
2. **Two-clock timeouts for subagents and ACP children** — high value, small effort. Add `idle_secs` next to the existing
   run caps (`run_timeout`, job caps); refresh it on tool/ACP output. Lands in `crates/agent` (subagent supervision),
   `crates/acp`, and the routine front matter (`run_timeout:` already exists).
3. **Multitask strategy as a session/desk setting** — medium value, small effort: enqueue | interrupt | reject for inputs
   arriving mid-turn (peer messages, wakes, chat). Lands in `crates/agent` (turn intake) and the desk config (67).
4. **Graceful "tools off, answer now" at caps** — medium value, small effort. Lands in the agent loop's turn budget and in
   autoresearch/routines.
5. **Schema-validated `ask_user`/escalate answers + arg-hash-bound approvals** — medium value, small effort. Lands in the
   approvals and double paths (126) and `tools.policy`.
6. **HITL lesson capture from review dispositions** — medium-high value, medium effort. On `review → done/send back` with
   a comment, distil lessons tagged `source: review` into knowledge as candidates. Dreaming (136) replays and keeps them.
   Lands in `crates/tasks` (review transitions) and `crates/dreaming`.
7. **Fan-out task template** — medium value, medium effort: `task_create(fan_out=[...], aggregate=...)`, with children and
   an aggregator wake when all are done. Lands in `crates/tasks` (blockers/parents) and the board UI.
8. **Definition hash on durable runs** — low-medium value, small effort. Lands in `crates/schedule` (loops) and
   `crates/autoresearch`.
9. **FIDES-style labels, observe-only** — high value long-term, large effort. Lands in `crates/tools` (result labels) and
   `tools.policy`. Do it after 1–5.
10. **Run fork/replay** — medium value, large effort. Only for autoresearch (140) at first.

Not recommended: adopting any of the three as theuth's runtime, or a CrewAI-style manager LLM whose only tools are
delegate/ask. Both are the weakest patterns in the field (see below), and theuth's checkout/dispatch already does that
job deterministically.

## Their weaknesses and failure modes

**CrewAI**
- *Delegation is brittle, and the code says so.* Coworkers are matched by a normalised role string. A comment explains
  that quotes are stripped because "less-powerful LLM's have difficulty producing valid JSON"
  (`base_agent_tools.py#L66-L72`). Every miss and every exception becomes text fed back to the LLM (`#L99-L124`), so
  failures turn into retries the model may or may not handle.
- *Hierarchical mode is not a planner.* Task order is fixed and the manager does every task (`crew.py#L1727-L1730`). Users
  report the manager redoing work: #2838 "[BUG] Manager agent repeatedly performs tasks" (16 comments), #330
  "allow_delegation=True leading to infinite loop" (12), #2606 a type error in hierarchical delegation (18), and #466
  "Force complex hierarchy within the crew" (one level only).
- *A2A delegation state:* #4166 "Second A2A delegation fails — task is in terminal state". Remote task lifecycles leak
  into delegation.
- *Docs drift:* the memory concepts page and the crews page disagree (A8). The `max_retries` → `guardrail_max_retries`
  deprecation is still live (`task.py#L275-L281`).
- *Telemetry on by default*, and a past incident hijacked hosts' OTel pipelines (`telemetry.py#L141-L150`).
- `Process.consensual` has been a TODO since the start (`process.py#L11`).

**LangGraph**
- *Interrupt semantics bite.* The node re-runs from the top on resume, so side effects before `interrupt()` repeat
  unless they are idempotent (`types.py#L917-L922` docstring). Open issues: #7780 "Interrupt() in a loop will cause extra
  resumes" (19 comments, open); #6792 "Resuming after interrupt doesn't reuse prior task outputs when interrupt is in
  subgraph" (open); #6956 `get_state().next` empty after a node calls `interrupt()` twice. Closed: #8653 "update_state can
  permanently erase a thread's messages". The HEAD commit itself is a fix in that area (`update_state` writes on the
  checkpoint it updates, b4991f1).
- *Loose default cap:* `DEFAULT_RECURSION_LIMIT = 10007` (`_internal/_config.py#L32`), so a runaway loop costs ~10k steps
  before it stops.
- *Delta-channel footguns:* pruning or copying a thread naively breaks reconstruction. The base class carries three
  warnings (`checkpoint/base/__init__.py#L364-L423`).
- *Multi-agent packaging churn:* `langgraph-supervisor-py` is archived. Patterns move to `Command` handoffs and "deep
  agents".

**Microsoft Agent Framework**
- *Young and wide:* 761 open issues against 14k★, many APIs `@experimental` (e.g. the whole harness,
  `_background_agents.py#L56`), and 43 packages to keep in step across Python and .NET.
- *Orchestration bugs reported:* #4544 "Race Condition in Handoff Workflow as Agent?" (.NET), #5621 "Checkpointing in
  Handoff Workflow" (open), #754 "GroupChatOrchestration uses too many tokens when MaximumInvocationCount > 3".
- *Unbounded by default:* group chat "will continue indefinitely" without `max_rounds` or a termination condition
  (`_group_chat.py#L144-L145`). Handoff's autonomous mode quietly injects "User did not respond. Continue assisting
  autonomously." (`_handoff.py#L237`).
- *Best-effort caps:* `max_function_calls` and `max_duration_seconds` are checked after each parallel batch, so 20
  parallel calls can run past a limit of 10 (`_tools.py#L1783-L1786`).
- *Security gaps admitted in-tree:* FIDES does not cover CodeAct nested calls (`monty/README.md#L8-L18`).
- *Magentic resets lose context:* a reset clears the whole chat history (`_magentic.py#L388-L396`). Repeated stalls throw
  away partial work unless the facts ledger captured it.

**Common to all three:** "who acts next" is decided either by code (flows, graphs) or by one LLM call per round with no
memory of *why* earlier rounds failed, beyond the transcript or ledger. None of them has owners, reviewers, budgets in
money, or a human attention surface. These are exactly the parts Paperclip-style control planes and theuth add.

## Cites self-check

I re-opened every cite below at the end of the run with `read_file(offset, limit)` and checked for a keyword per range: 153 range checks over 200 `#L` cites. Each cite below **held**; the six corrections follow.

- held — `crewai/lib/crewai/src/crewai/crew.py#L1531-L1542` — custom manager with tools raises
- held — `crewai/lib/crewai/src/crewai/crew.py#L1727-L1730` — hierarchical: every task executed by the manager
- held — `crewai/lib/crewai/src/crewai/tools/agent_tools/base_agent_tools.py#L110-L124` — delegation = throw-away Task + sync execute_task
- held — `crewai/lib/crewai/src/crewai/process.py#L4-L11` — consensual still TODO
- held — `crewai/lib/crewai/src/crewai/flow/runtime/__init__.py#L3338-L3346` — max_method_calls → RecursionError
- held — `crewai/lib/crewai/src/crewai/flow/human_feedback.py#L279-L296` — pre-review recalls hitl lessons
- held — `crewai/lib/crewai/src/crewai/flow/human_feedback.py#L337-L369` — lesson distillation into memory
- held — `crewai/lib/crewai/src/crewai/state/runtime.py#L352-L389` — fork with parent branch/checkpoint lineage
- held — `crewai/lib/crewai/src/crewai/memory/unified_memory.py#L100-L135` — composite weights, half-life, consolidation
- held — `crewai/docs/edge/en/concepts/memory.mdx#L10 vs crews.mdx#L315` — docs disagree on memory types
- held — `langgraph/libs/langgraph/langgraph/pregel/_algo.py#L432-L434 + #L1259-L1276` — next step = Sends ∪ triggered nodes
- held — `langgraph/libs/langgraph/langgraph/pregel/_loop.py#L446-L505` — per-task put_writes
- held — `langgraph/libs/langgraph/langgraph/types.py#L461-L490` — run_timeout vs idle_timeout + heartbeat
- held — `langgraph/libs/langgraph/langgraph/types.py#L1019-L1046` — interrupt index/resume replay
- held — `langgraph/libs/langgraph/langgraph/_internal/_config.py#L32` — default recursion limit 10007
- held — `langgraph/libs/langgraph/langgraph/graph/state.py#L1749-L1775` — Command(graph=PARENT) → ParentCommand
- held — `langgraph/libs/prebuilt/langgraph/prebuilt/tool_node.py#L903-L918` — parallel handoffs merged
- held — `langgraph/libs/sdk-py/langgraph_sdk/schema.py#L81-L88` — MultitaskStrategy
- held — `agent-framework/python/packages/orchestrations/agent_framework_orchestrations/_magentic.py#L205-L244` — five-question progress ledger
- held — `agent-framework/python/packages/orchestrations/agent_framework_orchestrations/_magentic.py#L1115-L1143` — stall ±1, >3 → reset/replan
- held — `agent-framework/python/packages/orchestrations/agent_framework_orchestrations/_handoff.py#L478-L496` — human_in_loop vs autonomous
- held — `agent-framework/python/packages/orchestrations/agent_framework_orchestrations/_group_chat.py#L141-L145` — unbounded by default
- held — `agent-framework/python/packages/core/agent_framework/_workflows/_runner.py#L108-L179` — checkpoint per superstep, convergence cap
- held — `agent-framework/python/packages/core/agent_framework/_workflows/_checkpoint.py#L35-L103` — graph signature + pending requests
- held — `agent-framework/python/packages/core/agent_framework/_tools.py#L1770-L1799` — call/duration budgets incl. approval waits
- held — `agent-framework/python/packages/core/agent_framework/_harness/_tool_approval.py#L25-L57` — tool vs tool_with_arguments rules
- held — `agent-framework/docs/decisions/0024-prompt-injection-defense.md#L36-L44` — FIDES
- held — `agent-framework/docs/decisions/0041-handle-provider-invalidated-responses.md#L50-L56` — invalidated ⇒ no side effects
- held — `agent-framework/python/packages/monty/README.md#L8-L18` — FIDES not enforced in CodeAct
- held — `theuth/crates/tasks/src/ledger.rs#L2075-L2089` — theuth no_progress back-off

**Corrected (6, all ADR line offsets: I had cited the `## Decision Outcome` heading instead of the chosen-option text; ranges widened):**
- corrected — `agent-framework/docs/decisions/0043-python-mcp-runtime-context.md#L27-L35` → `0043-python-mcp-runtime-context.md#L27-L35`
- corrected — `agent-framework/docs/decisions/0022-chat-history-persistence-consistency.md#L74-L76` → `0022-chat-history-persistence-consistency.md#L74-L76`
- corrected — `agent-framework/docs/decisions/0019-python-context-compaction-strategy.md#L996-L998` → `0019-python-context-compaction-strategy.md#L996-L998`
- corrected — `agent-framework/docs/decisions/0006-userapproval.md#L383-L393` → `0006-userapproval.md#L383-L393`
- corrected — `agent-framework/docs/decisions/0039-shared-agent-session-store.md#L37-L48` → `0039-shared-agent-session-store.md#L37-L48`
- corrected — `agent-framework/docs/decisions/0027-hosting-channels.md#L64-L71` → `0027-hosting-channels.md#L64-L71`

**Not verifiable from the clones:** LangGraph supervisor/swarm internals (separate repos; archive status from the GitHub MCP), LangGraph Platform A2A/Studio, CrewAI AMP, issue contents beyond titles and comment counts (GitHub MCP `search_issues`).
