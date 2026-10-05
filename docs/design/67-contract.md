# 67 — Contract (commit 0)

*Status: as-built skeleton, 2026-09-24. Everything named here compiles on `feature/67-agents-contract`; nothing here works yet. Four workers fill it in parallel without changing the names below. Decisions are in `67-agents.md` §0; this document is the exact Rust surface they produced.*

## 0. Rules every worker inherits

- **Names are frozen.** Every type, trait, message, RPC method, tool name, table and column below keeps its name and path. Add fields, add messages, add tables; do not rename or move what is here. A worker that must change a shared name says so in its report and touches every caller in the same commit.
- **Each plugin owns its sqlite file** (`<data_dir>/<name>.sqlite`, `sqlx::migrate!("./migrations")` in `store.rs`, WAL, 4 connections, 5 s busy timeout — copied from `schedule::store`). New migrations are `0002_<name>_<what>.sql`; never edit `0001_*`.
- **Cross-crate handles go through the kameo registry**, the way every plugin already does: `ActorRef::<A>::lookup(NAME)` for a one-shot (`schedule::tools::actor()`), `rt.lookup_when::<A>(NAME).await` inside a `tokio::spawn` when the target may start later (`SchedulePlugin::start` waiting for `tools::REGISTRY_ACTOR`). Each crate exports `pub fn actor() -> Option<ActorRef<…>>` and `pub const <NAME>_ACTOR: &str`. No plugin holds another plugin's struct; no god-wiring (`00-architecture.md` §1).
- **Cross-crate behaviour goes through the traits below**, registered once by an `ask` of a `Register*` message: `replica::RegisterTransport` (mesh → replica), `schedule::RegisterTaskCreator` (tasks → schedule), `tasks::RegisterDeskRouter` (roster → tasks). The registering plugin already does this in its `start`; the receiving actor already stores the `Arc<dyn …>`. The worker on each side fills the trait impl / the call site.
- **Actors answer `Result<T, String>`**; `Err("not built")` today. The RPC layer maps a handler `Err` to JSON-RPC `-32000` with the string as message, so `t mesh status` prints `not built` end to end.
- **Every ms field is `u64` ms since the Unix epoch** and carries `#[cfg_attr(feature = "ts", ts(type = "number"))]`.
- **Dependency direction** (no cycles): `replica` → nothing new; `mesh` → `replica`; `tasks` → `schedule`, `document`, `agent`; `roster` → `tasks`, `agent`, `knowledge`, `tools`. `rpc` → `mesh`, `roster`, `tasks`. `theuth` → all four.
- **Tasks belong to projects.** `Task.project` is the same free-text label sessions carry (`sessions.project`, styled by `agent`'s `projects` table — `crates/agent/migrations/0018_project_style.sql`, `set_session_project`, `list_project_styles`). The tasks plugin stores the label only; icon and colour come from `list_project_styles` in the client. A task created from a session defaults to that session's project.
- **Plugin order in `crates/theuth/src/main.rs::engine_plugins`**: `mesh`, `replica`, `roster`, `tasks` (after `schedule`). Order carries no promise: every cross-plugin dependency is resolved with `lookup_when`.

## 1. Who implements what

| Worker | Fills | Implements (trait) | Calls (trait) | Touches outside its crate |
|---|---|---|---|---|
| **mesh** | `crates/mesh`: iroh endpoint + identity file, pairing, peer set, gossip, lease election (§4.3–4.6), engine stats, `mesh_peers` tool body, all `Mesh` handlers | `replica::Transport` on the endpoint; registers it with `replica::RegisterTransport` | — | `deploy/relay` (iroh-relay), `mesh:` template in `main.rs` if keys change |
| **replica** | `crates/replica`: `attach` (session extension capture → change feed), `ReplicaHandle`, sync over `ALPN_RECORDS`, HLC, per-column LWW apply, `sync_state`/`clock` | — | `replica::Transport` (`peers`, `open`, `subscribe`) | nothing; other crates call `replica::attach(store.pool(), &["…"])` themselves once it works |
| **tasks** | `crates/tasks`: record CRUD, state guards, wakes (coalesce, back-off, dispatch as a hub duty), checkout/orphan (§7.5), event log, write cap, the seven tool bodies, `ScheduleTasks::create_for_schedule`; **and** the schedule fire path: `Schedule::fire` consults `task_creator` when `Spec.agent` is `Some` (§7.4) | `schedule::TaskCreator` (`tasks::ScheduleTasks`) | `tasks::DeskRouter` (`desk_for`, `deliver`) | `crates/schedule/src/lib.rs` fire path only |
| **roster** | `crates/roster`: directory watcher + `agents` mirror, desks/episodes/journal (§6.2), budgets (§6.4), proposals + `hire_agent` approval (§6.5), `agent_propose`/`agents_list` bodies, all `Roster` handlers, `AgentFile` render + atomic write; knowledge owner/visibility (§9) and the MCP allowlist (§6.3) | `tasks::DeskRouter` (`roster::Desks`) | — | `crates/knowledge` (owner/visibility front matter), `crates/tools/src/mcp` (allowlist consult), `crates/agent` (`model`/`effort`/`tools` on a remote child) |

The RPC methods and the desktop `api.ts` entries exist; each worker's actor makes its methods real without touching `crates/rpc`. The board and Agents view (§12) are a later UI worker.

## 2. `crates/replica`

Plugin `replica::ReplicaPlugin`, actor `replica::Replica` registered as `replica::REPLICA_ACTOR = "replica"`, file `replica::DB = "replica.sqlite"`, `replica::actor()`.

### Types (`replica::types`, re-exported at the crate root)
| Rust | Shape | Notes |
|---|---|---|
| `replica::PeerId(pub String)` | transparent | the mesh engine id as text; `Ord`, `Display`; TS-exported |
| `replica::Hlc { wall_ms: u64, counter: u32, origin: PeerId }` | struct | derived `Ord` in field order = HLC order, `origin` tie-break |
| `replica::Change { table: String, pk: String, column: Option<String>, value: Option<Vec<u8>>, hlc: Hlc, deleted: bool }` | struct | `column: None` = whole row; not TS-exported (`knowledge::Change` owns the binding name) |
| `replica::ChangeBatch { origin: PeerId, seq: u64, changes: Vec<Change> }` | struct | acked by `seq` into `sync_state` |
| `replica::PeerEvent { Connected(PeerId), Disconnected(PeerId) }` | enum, `tag = "type", content = "peer"`, snake_case | TS-exported |
| `replica::ByteStream: AsyncRead + AsyncWrite + Unpin + Send` | trait, blanket impl | one QUIC stream |
| `replica::Transport: Send + Sync` | `#[async_trait]` | `fn peers(&self) -> Vec<PeerId>`; `async fn open(&self, peer: &PeerId, alpn: &str) -> anyhow::Result<Box<dyn ByteStream>>`; `async fn accept(&self, alpn: &str) -> anyhow::Result<(PeerId, Box<dyn ByteStream>)>` (added by the replica worker: the next inbound stream on `alpn` and who opened it); `fn subscribe(&self) -> broadcast::Receiver<PeerEvent>` |
| `replica::ReplicaHandle` | opaque, `Clone` | returned by `attach` |

### Functions and constants
- `pub async fn replica::attach(pool: &sqlx::SqlitePool, tables: &[&str]) -> anyhow::Result<ReplicaHandle>` — `Err("not built")`.
- `replica::ALPN_RECORDS = "theuth/records/1"`, `ALPN_ACP = "theuth/acp/1"`, `ALPN_SESSIONS = "theuth/sessions/1"`, `ALPN_KNOWLEDGE = "theuth/knowledge/1"`, `ALPN_GOSSIP = "theuth/gossip/1"`.

### Messages on `Replica`
| Message | Reply | Today |
|---|---|---|
| `replica::RegisterTransport(pub Arc<dyn Transport>)` | `()` | stores it |

### Migration `crates/replica/migrations/0001_replica.sql`
`sync_state(peer TEXT, table_name TEXT, last_seq INTEGER, updated_ms INTEGER, PRIMARY KEY(peer, table_name))`, `clock(id INTEGER PK CHECK(id=1), wall_ms INTEGER, counter INTEGER)`. Bookkeeping only; replicated rows stay in their owner's file.

## 3. `crates/mesh`

Plugin `mesh::MeshPlugin`, actor `mesh::Mesh` as `mesh::MESH_ACTOR = "mesh"`, file `mesh::DB = "mesh.sqlite"`, identity dir `mesh::DIR = "mesh"`, `mesh::actor()`. Config `mesh::MeshConfig` (section `mesh`).

### Types (`mesh::types`, re-exported at the root; all TS-exported)
| Rust | Shape |
|---|---|
| `mesh::EngineId(pub String)` | transparent; `From<EngineId> for replica::PeerId` and back |
| `mesh::HubPref { Prefer, Eligible, Never }` | lowercase; `Default = Eligible` |
| `mesh::EngineClass { Server, Desktop, Laptop, Container }` | lowercase |
| `mesh::Peer { id: EngineId, name: String, addrs: Vec<String>, reachable: bool, last_seen_ms: u64, hub_pref: HubPref }` | struct |
| `mesh::EngineStats { id: EngineId, uptime_since_ms: u64, class: EngineClass, availability_30d: f32, harnesses: Vec<String>, repos: Vec<String>, display: bool, gpu: bool }` | struct |
| `mesh::LeaseState { holder: Option<EngineId>, since_ms: u64, reason: String }` | struct |
| `mesh::MeshEvent { Peer(PeerEvent), Lease(LeaseState) }` | `tag = "type", content = "data"`, snake_case |
| `mesh::MeshStatus { self_id: EngineId, peers: Vec<Peer>, lease: LeaseState }` | struct |
| `mesh::PeerId`, `mesh::PeerEvent` | re-exports of `replica::` |

### Messages on `Mesh`
| Message | Reply | Today |
|---|---|---|
| `mesh::Status` | `Result<MeshStatus, String>` | `Err("not built")` |
| `mesh::AddPeer(pub String)` (pairing string) | `Result<Peer, String>` | `Err("not built")` |
| `mesh::RemovePeer(pub EngineId)` | `Result<(), String>` | `Err("not built")` |
| `mesh::SetHubPref(pub HubPref)` | `Result<(), String>` | `Err("not built")` |
| `mesh::PairingString` | `Result<String, String>` | `Err("not built")` |
| `mesh::Subscribe` | `broadcast::Receiver<MeshEvent>` | works (channel of 256, nothing published yet) |

### Config `mesh:` (`mesh::MeshConfig`, `#[serde(default)]`)
`relay: "https://iroh.relay.theuth.io"` (`none`/`off`/empty = no relay), `discovery: true`, `hub: eligible`, `probation_secs: 21600`, `handover_secs: 600`, `heartbeat_secs: 10`, `lapse_secs: 30`, `capabilities_secs: 300`, `workspaces: []`. Documented (commented) in `DEFAULT_CONFIG` in `main.rs`.
A lone engine (empty peer set) binds its endpoint but dials nothing: the relay is inserted and DNS/pkarr + mDNS lookup started by `Net::join_network` on the first pairing (`t mesh pair` / `t mesh add`, either side) or on bind when peers are stored (worker Z, single-engine invariant 1).

### Tool
`mesh_peers` (`mesh::tools::MeshPeersTool`, `Risk::ReadOnly`, args `{ reachable?: bool }`) — body `not built`.

### Migration `crates/mesh/migrations/0001_mesh.sql`
`peers(id TEXT PK, name, addrs_json, hub_pref, added_ms)`, `engine_stats(id TEXT PK, uptime_since_ms, class, harnesses_json, repos_json, display INT, gpu INT, updated_ms, hlc TEXT)`, `availability(observer, subject, from_ms, to_ms, reachable INT, PRIMARY KEY(observer, subject, from_ms))`, `lease_log(at_ms, holder, reason)`. `peers` and `engine_stats` are the replicated tables.

## 4. `crates/tasks`

Plugin `tasks::TasksPlugin`, actor `tasks::Tasks` as `tasks::TASKS_ACTOR = "tasks"`, file `tasks::DB = "tasks.sqlite"`, `tasks::actor()`. Config `tasks::TasksConfig` (section `tasks`).

### Types (`tasks::types`, re-exported at the root; all TS-exported)
| Rust | Shape |
|---|---|
| `tasks::TaskId(pub String)` | transparent; `TaskId::new()` = uuidv7 text |
| `tasks::TaskState { Todo, Doing, Review, Blocked, Done, Cancelled }` | snake_case; `as_str()` |
| `tasks::Owner { Agent(String), User, System }` | JSON `tag = "type", content = "name"`: `{"type":"agent","name":"ops"}` / `{"type":"user"}` / `{"type":"system"}`; text form via `Display`/`FromStr`: `agent:ops` / `user` / `system` (columns, filters, `created_by`, `actor`). `System` (added by worker T) is the reviewer of a schedule task its `until` closed (67 §7.4) |
| `tasks::Checkout { session: String, engine: String, epoch: u32, progress_at_ms: u64, orphaned: bool }` | struct; `orphaned` (added by worker T, `#[serde(default)]`) is set by the orphan sweep (67 §7.5) — the checkout is kept for the record, `epoch` already bumped |
| `tasks::Task { id, title, brief, owner, state, reviewer: Option<Owner>, blockers: Vec<String>, blocks: Vec<TaskId>, parent: Option<TaskId>, project: Option<String>, where_: Option<String>, harness: Option<String>, external: Vec<String>, thread: Option<String>, checkout: Option<Checkout>, schedule: Option<(String, u64)>, created_by: String, created_ms: u64, updated_ms: u64, no_progress_count: u32, next_wake_after_ms: Option<u64> }` | `where_` is `#[serde(rename = "where")]` everywhere; `project` is the label sessions carry (`agent`'s `projects.name`, 51 §3, `set_session_project`), `None` = no project; `blocks` (added by worker T) is the inverse of `blockers` — tasks whose blockers name this one — computed on `task_get`, never stored, empty in `tasks_list` |
| `tasks::WakeReason { Assigned, Comment, Mention, Schedule, Answer, Review, Resume, Manual }` | snake_case; `as_str()` |
| `tasks::Wake { id: String, agent: String, task: Option<TaskId>, reason: WakeReason, key: String, count: u32, claimed_by: Option<String>, created_ms: u64 }` | `Wake::key_of(agent, task, reason)` = `"{agent}|{task or empty}|{reason}"` |
| `tasks::Disposition { Done, Review { reviewer: Owner }, Blocked { on: String }, Delegated { to: TaskId }, Continuing { by: String } }` | `tag = "type"`, snake_case |
| `tasks::TaskEvent { at_ms: u64, actor: String, session: Option<String>, field: String, before: Option<String>, after: Option<String> }` | struct |
| `tasks::TaskFilter { owner: Option<String>, state: Option<Vec<TaskState>>, engine: Option<String>, project: Option<String> }` | `#[serde(default)]`; `owner` is the text form |
| `tasks::TaskCreate { title, brief, owner: Owner, reviewer?, parent?, project?, where_?, harness?, external: Vec<String> }` | struct |
| `tasks::TaskPatch { title?, brief?, owner?, reviewer?, state?, blockers?, where_?, project?, harness?, external? }` | every field `Option`, `#[serde(default)]`; TS `optional_fields = nullable` |

### Trait
`tasks::DeskRouter: Send + Sync` (`#[async_trait]`): `fn desk_for(&self, agent: &str) -> Option<String>`; `async fn deliver(&self, agent: &str, wake: &Wake, task: Option<&Task>) -> anyhow::Result<()>`. `agent` is the bare name (no `agent:`). Added by worker T, each with a default (`None` / `None` / empty) so the roster fills them at its pace: `fn manager_of(&self, agent: &str) -> Option<String>` (`reports_to` as an agent name; `None` = the user), `fn agent_for(&self, session: &str) -> Option<String>` (the agent whose desk or work session it is), `fn agents(&self) -> Vec<String>` (every name, for `@`-less mentions).

### Messages on `Tasks`
| Message | Reply | Today |
|---|---|---|
| `tasks::RegisterDeskRouter(pub Arc<dyn DeskRouter>)` | `()` | stores it |
| `tasks::ListTasks(pub TaskFilter)` | `Result<Vec<Task>, String>` | `Err("not built")` |
| `tasks::GetTask(pub TaskId)` | `Result<Task, String>` | `Err("not built")` |
| `tasks::CreateTask { input: TaskCreate, created_by: String, session: Option<String> }` | `Result<Task, String>` | `Err("not built")` |
| `tasks::UpdateTask { id: TaskId, patch: TaskPatch, actor: String, session: Option<String> }` | `Result<Task, String>` | `Err("not built")` |
| `tasks::CommentTask { id: TaskId, text: String, progress: bool, actor: String, session: Option<String> }` | `Result<(), String>` | `Err("not built")` |
| `tasks::TaskEvents(pub TaskId)` | `Result<Vec<TaskEvent>, String>` | built |
| `tasks::TaskBlocks(pub TaskId)` | `Result<Vec<TaskId>, String>` | added by worker T: the inverse link |
| `tasks::CheckoutTask { id, session, actor }` | `Result<Task, String>` | added by worker T (`task_checkout`) |
| `tasks::PlaceTask { id, where_, harness, reason, actor, session }` | `Result<Task, String>` | added by worker T (`task_place`) |
| `tasks::Escalate { task, question, options, actor, session }` | `Result<String, String>` (the `decision:<id>`) | added by worker T (`escalate`) |
| `tasks::NoteDisposition { id, text, actor, session }` | `Result<(), String>` | added by worker T: `continuing:<by>` / `delegated:<child>` on the event log |
| `tasks::EpisodeClosing { agent, session }` | `Result<Vec<TaskId>, String>` | added by worker T: the roster sends it before closing a desk (67 §7.3 last rule) |
| `tasks::SetDispatching(pub bool)` | `()` | added by worker T: hub duties (dispatch every 5 s, sweep every 30 s) on/off; default on |
| `tasks::tools::CallerOf(pub String)` | `String` | added by worker T: `agent:<name>` \| `user` for a session |

`tasks::ScheduleTasks` implements `schedule::TaskCreator` (`Err("not built")`); the plugin registers it with the schedule actor via `lookup_when` + `RegisterTaskCreator`.

### Config `tasks:` (`tasks::TasksConfig`)
`orphan_secs: 120`, `stale_secs: 900`, `note_secs: 300`, `cross_task_write_cap: 20`, `cross_task_write_window_secs: 86400`, `backoff_min_secs: 120`, `backoff_max_secs: 1800`.

### Tools (`tasks::tools`, bodies `not built`, descriptions final)
| Name | Struct | Risk | Args |
|---|---|---|---|
| `task_create` | `TaskCreateTool` | Mutating | `{ title, brief, owner?, reviewer?, parent?, project?, where?, harness?, external? }` (owner/reviewer as text form; `project` defaults to the calling session's) |
| `task_inbox` | `TaskInboxTool` | ReadOnly | `{ state?: TaskState[], project?: string }` — output grouped by project, no-project last |
| `task_checkout` | `TaskCheckoutTool` | Mutating | `{ id: TaskId }` |
| `task_update` | `TaskUpdateTool` | Mutating | `{ id, state?, owner?, reviewer?, blockers?, title?, brief?, external? }` |
| `task_note` | `TaskNoteTool` | Mutating | `{ id, text, progress?: bool }` |
| `task_place` | `TaskPlaceTool` | Mutating | `{ id, needs?: string }` |
| `escalate` | `EscalateTool` | Mutating | `{ task: TaskId, question, options?: string[] }` |

### Migration `crates/tasks/migrations/0001_tasks.sql`
`tasks(id TEXT PK, title, brief, owner, state, reviewer, blockers_json, parent, project, where_, harness, external_json, thread, checkout_json, schedule_name, schedule_fire_ms, created_by, created_ms, updated_ms, no_progress_count, next_wake_after_ms, hlc TEXT)` + indexes `(project)`, `(owner, state)` + unique partial index `(schedule_name, schedule_fire_ms)`; `wakes(id TEXT PK, agent, task, reason, key TEXT UNIQUE, count, claimed_by, created_ms, hlc TEXT)`; `task_events(id INTEGER PK, task, at_ms, actor, session, field, before, after)`; `write_budget(session, task, PRIMARY KEY(session, task))`. `tasks` and `wakes` are the replicated tables.

## 5. `crates/schedule` (additions only)

- `schedule::file::Spec.agent: Option<String>` from the `agent:` front-matter key (trimmed, empty = `None`); `Spec.project: Option<String>` from `project:` (worker T) — the task's project label, else inferred from `cwd:` by the tasks side, else none.
- `schedule::TaskCreator: Send + Sync` (`#[async_trait]`): `async fn create_for_schedule(&self, schedule: &str, fire_ms: u64, agent: &str, prompt: &str, project: Option<&str>, cwd: Option<&str>) -> anyhow::Result<String>` (returns the task id; `project`/`cwd` added by worker T) and `async fn close_for_schedule(&self, schedule: &str, why: &str) -> anyhow::Result<()>` (worker T: `until` passed → the schedule's open tasks are `done`, reviewer `system`).
- `schedule::RegisterTaskCreator(pub Arc<dyn TaskCreator>)` → `()`; stored in `Schedule.task_creator`. `Schedule::fire` with `spec.agent` set calls `create_for_schedule` with the wrapped prompt instead of a session input, records a finished run row `task <id>` (counted toward `max_runs`), publishes `ScheduleEvent::Changed`; `Fire` (`/schedules run`) on such a schedule answers `Err("created task <id>")` since there is no session to open (67 §7.4).

## 6. `crates/roster`

Plugin `roster::RosterPlugin`, actor `roster::Roster` as `roster::ROSTER_ACTOR = "roster"`, file `roster::DB = "roster.sqlite"`, directory `roster::file::DIR = "agents"`, `roster::actor()`. Config `roster::RosterConfig` (section `roster`).

### Types (`roster::types`, re-exported at the root; all TS-exported)
| Rust | Shape |
|---|---|
| `roster::AgentName(String)` | private field; `AgentName::new(&str) -> Result<Self, String>` validates `[a-z0-9-]{1,32}`; `as_str()`, `peer()` = `agent:<name>`; `AgentName::DEFAULT = "theuth"` (reserved, never a file); serde `try_from = String, into = String`; TS `string` |
| `roster::Budget { None, Monthly { amount: f64, currency: String } }` | JSON `tag = "type"`, snake_case; file form via `Display`/`FromStr`: `none` / `40 USD/month` |
| `roster::AgentFile { name: AgentName, title: String, reports_to: String, model: Option<String>, effort: Option<String>, tools: Option<String>, mcp: Vec<String>, budget: Budget, capabilities: Option<String>, paused: bool, proposed_by: Option<String>, body: String }` | `reports_to` default `"theuth"`, `title` default = name |
| `roster::AgentStatus { Idle, Working, Blocked, Paused, Proposed, Unreachable }` | snake_case |
| `roster::AgentSummary { file: AgentFile, status: AgentStatus, desk: Option<String>, open_tasks: u32, spent_month: f64 }` | struct |
| `roster::Episode { id: String, agent: String, session: String, opened_ms: u64, closed_ms: Option<u64>, reason: Option<String> }` | struct |

### Functions
- `roster::file::parse(name: &str, text: &str) -> Result<AgentFile, String>` — **built**: gray_matter YAML front matter (`deny_unknown_fields`: `title, reports_to, model, effort, tools, mcp, budget, capabilities, paused, proposed_by`), body trimmed = charter (`empty charter` refused), `tools` must be `read_only|mutating|destructive`, `theuth` refused. Three unit tests in `file.rs`.
- `roster::file::load_dir(dir: &Path) -> Vec<(String, Result<AgentFile, String>)>` — built, mirrors `schedule::file::load_dir`.

### Messages on `Roster`
| Message | Reply | Today |
|---|---|---|
| `roster::ListAgents` | `Result<Vec<AgentSummary>, String>` | `Err("not built")` |
| `roster::GetAgent(pub String)` | `Result<AgentFile, String>` | `Err("not built")` |
| `roster::PutAgent(pub AgentFile)` | `Result<(), String>` | `Err("not built")` |
| `roster::PauseAgent { name: String, paused: bool }` | `Result<(), String>` | `Err("not built")` |
| `roster::ApproveAgent(pub String)` | `Result<(), String>` | `Err("not built")` |

`roster::Desks` implements `tasks::DeskRouter` (`desk_for` → `None`, `deliver` → `Err("not built")`); the plugin registers it with the tasks actor via `lookup_when` + `RegisterDeskRouter`.

### Config `roster:` (`roster::RosterConfig`)
`decision_wait_secs: 120`, `episode_idle_secs: 21600`, `episode_context_pct: 60`, `journal_max_bytes: 8192`.

### Tools (`roster::tools`, bodies `not built`)
| Name | Struct | Risk | Args |
|---|---|---|---|
| `agent_propose` | `AgentProposeTool` | Mutating | `{ name: AgentName, title, reports_to?, charter, budget?, tools?, mcp?: string[], capabilities? }` |
| `agents_list` | `AgentsListTool` | ReadOnly | `{ all?: bool }` |

### Migration `crates/roster/migrations/0001_roster.sql`
`agents(name TEXT PK, front_matter_json, body, mtime_ms, hlc TEXT)`, `episodes(id TEXT PK, agent, session, opened_ms, closed_ms, reason)`, `proposals(name TEXT PK, proposed_by, approval_id, created_ms)`. `agents` is the replicated table.

## 7. RPC (`crates/rpc/src/api.rs`, trait `Api`)

Each is one `send(&<actor>()?, <message>)`; the actor's `Err` string is the JSON-RPC error message. Actor lookups: `mesh_actor()`, `roster_actor()`, `tasks_actor()` beside `schedule()`.

| Method | Params | Returns | Message |
|---|---|---|---|
| `mesh_status` | — | `mesh::MeshStatus` | `mesh::Status` |
| `mesh_pairing_string` | — | `String` | `mesh::PairingString` |
| `mesh_add` | `pairing: String` | `mesh::Peer` | `mesh::AddPeer` |
| `mesh_remove` | `id: String` | `()` | `mesh::RemovePeer(EngineId(id))` |
| `mesh_set_hub` | `pref: mesh::HubPref` | `()` | `mesh::SetHubPref` |
| `agents_list` | — | `Vec<roster::AgentSummary>` | `roster::ListAgents` |
| `agent_get` | `name: String` | `roster::AgentFile` | `roster::GetAgent` |
| `agent_put` | `file: roster::AgentFile` | `()` | `roster::PutAgent` |
| `agent_pause` | `name: String, paused: bool` | `()` | `roster::PauseAgent` |
| `agent_approve` | `name: String` | `()` | `roster::ApproveAgent` |
| `tasks_list` | `filter: tasks::TaskFilter` | `Vec<tasks::Task>` | `tasks::ListTasks` |
| `task_get` | `id: String` | `tasks::Task` | `tasks::GetTask` |
| `task_create` | `input: tasks::TaskCreate` | `tasks::Task` | `tasks::CreateTask { created_by: "user", session: None }` |
| `task_update` | `id: String, patch: tasks::TaskPatch` | `tasks::Task` | `tasks::UpdateTask { actor: "user", session: None }` |
| `task_comment` | `id: String, text: String` | `()` | `tasks::CommentTask { progress: false, actor: "user", session: None }` |
| `task_events` | `id: String` | `Vec<tasks::TaskEvent>` | `tasks::TaskEvents` |
| `task_blocks` | `id: String` | `Vec<tasks::TaskId>` | `tasks::TaskBlocks` (worker T: the inverse of `blockers`) |

TS: `crates/rpc/src/export.rs` exports every type above; `crates/desktop/ui/src/rpc/api.ts` has the `Api` methods and `METHODS` entries. `just ts` regenerates `bindings/` (gitignored).

## 8. CLI (`crates/theuth/src/main.rs`)

`clap::Subcommand` `Cmd` on `Cli.cmd: Option<Cmd>`, dispatched right after the config is built (`cmd_67`): `t mesh status | pair | add <pairing> | remove <id> | hub <prefer|eligible|never>`, `t tasks list [--owner agent:<name>|user] [--state todo,doing,…] [--engine <name>] [--project <name>]`. Each connects like `t --link` (`rpc::engine::discover` → `link_cli::connect`), calls the RPC method, pretty-prints the JSON reply; a server error prints its message (`not built`) and exits 1.

## 9. Not in this commit (each worker's brief)

Session extension spike and the sync engine; iroh endpoint, identity, relay deployment; election, gossip, lease log; desk sessions and episodes; the fire → task path; the board and Agents view; `t mesh add --ssh|--docker` (`crates/sandbox`); chat routing by name; Signal per-engine linking.
