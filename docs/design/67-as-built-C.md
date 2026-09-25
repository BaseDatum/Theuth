# 67 worker C — report (the contract, commit 0)

Branch `feature/67-agents-contract`, worktree `/Users/Sky.Moore/Develop/theuth-67-contract`. Contract document: `docs/design/67-contract.md`.

## Built, per commit

### `feat(67): crates mesh, replica, roster, tasks — skeletons, plugins, config`
- `crates/replica`: `Cargo.toml`, `migrations/0001_replica.sql` (`sync_state`, `clock`), `src/store.rs` (own `replica.sqlite`), `src/types.rs` (`PeerId`, `Hlc` with derived `Ord`, `Change`, `ChangeBatch`, `PeerEvent`, `ByteStream`, `Transport`), `src/lib.rs` (`ReplicaPlugin`, `Replica` actor as `"replica"`, `RegisterTransport`, `attach` → `Err("not built")`, `ReplicaHandle`, five `ALPN_*` constants, `actor()`).
- `crates/mesh`: `Cargo.toml`, `migrations/0001_mesh.sql` (`peers`, `engine_stats`, `availability`, `lease_log`), `src/{store,config,types,tools,lib}.rs` (`MeshPlugin`, `Mesh` actor as `"mesh"`, `Status`/`AddPeer`/`RemovePeer`/`SetHubPref`/`PairingString`/`Subscribe`, `MeshConfig` with the brief's defaults, `mesh_peers` tool stub).
- `crates/tasks`: `Cargo.toml`, `migrations/0001_tasks.sql` (`tasks`, `wakes`, `task_events`, `write_budget`), `src/{store,config,types,tools,lib}.rs` (`TasksPlugin`, `Tasks` actor as `"tasks"`, `DeskRouter` trait + `RegisterDeskRouter`, `ListTasks`/`GetTask`/`CreateTask`/`UpdateTask`/`CommentTask`/`TaskEvents`, `ScheduleTasks: schedule::TaskCreator` registered with the schedule actor, `TasksConfig`, seven tool stubs with schemas and descriptions).
- `crates/roster`: `Cargo.toml`, `migrations/0001_roster.sql` (`agents`, `episodes`, `proposals`), `src/{store,config,types,file,tools,lib}.rs` (`RosterPlugin`, `Roster` actor as `"roster"`, `ListAgents`/`GetAgent`/`PutAgent`/`PauseAgent`/`ApproveAgent`, `Desks: tasks::DeskRouter` registered with the tasks actor, `RosterConfig`, `agent_propose`/`agents_list` tool stubs, **`file::parse` built for real** with `load_dir`).
- Workspace `Cargo.toml`: four `[workspace.dependencies]` lines. `crates/theuth/Cargo.toml`: four deps. `crates/theuth/src/main.rs`: plugins registered in order mesh, replica, roster, tasks after `schedule`; commented `mesh:`/`roster:`/`tasks:` sections in `DEFAULT_CONFIG`; **also** the `t mesh …` / `t tasks list` subcommands (`Cmd`, `MeshCmd`, `TasksCmd`, `cmd_67`) — see Deviations.

### `feat(67): contract types, migrations, cross-crate traits`
- `crates/schedule/src/file.rs`: `agent: Option<String>` on `FrontMatter` (deny_unknown_fields) and `Spec.agent`.
- `crates/schedule/src/lib.rs`: `pub trait TaskCreator`, `RegisterTaskCreator(Arc<dyn TaskCreator>)`, `Schedule.task_creator` stored and not consulted. Fire behaviour unchanged.

### `feat(67): rpc + tool stubs for mesh, roster, tasks`
- `crates/rpc/src/api.rs`: 16 methods on `Api` (`mesh_status`, `mesh_pairing_string`, `mesh_add`, `mesh_remove`, `mesh_set_hub`, `agents_list`, `agent_get`, `agent_put`, `agent_pause`, `agent_approve`, `tasks_list`, `task_get`, `task_create`, `task_update`, `task_comment`, `task_events`); server impl = one `send` to the actor each, so the actor's `Err("not built")` is the JSON-RPC message; `mesh_actor()`, `roster_actor()`, `tasks_actor()` lookups.
- `crates/rpc/src/export.rs`: TS export list gains the 30 wire types. `crates/rpc/Cargo.toml`: `mesh`, `roster`, `tasks` deps and `ts` features.
- `crates/desktop/ui/src/rpc/api.ts`: 10 binding imports, 16 `Api` methods, 16 `METHODS` entries in sorted order.

### `feat(67): tasks carry a project` (Sky's mid-run addition)
- `tasks::Task`, `TaskCreate`, `TaskPatch`, `TaskFilter` gain `project: Option<String>` — the same label sessions carry (`agent`'s `projects.name`, `0018_project_style.sql`, `set_session_project`).
- `crates/tasks/migrations/0001_tasks.sql`: `project TEXT` column + `tasks_project` index.
- `task_create` schema: `project?` (default: the calling session's); `task_inbox` schema: `project?` filter, description says output is grouped by project (no-project last).
- `t tasks list --project <name>`; contract doc §0 rule + §4 rows updated.

### `docs(67): contract`
- `docs/design/67-contract.md`; this report as `docs/design/67-as-built-C.md`.

## Deviations from the brief
1. **CLI subcommands landed in commit 1**, not commit 3: they live in `main.rs`, which commit 1 already touched for the plugin list and config template; splitting one file's hunks across commits was not worth the risk. Contents are exactly the brief's.
2. **Roster `file::parse` landed in commit 1** (the crate does not compile without its `file` module), tests included.
3. **RPC server impls route to the actors** instead of returning a literal `Err("not built")`: the actors return `Err("not built")`, kameo's `SendError::HandlerError` displays as the inner string, so the wire message is exactly `not built` — and each worker's actor makes its RPC methods real without touching `crates/rpc`.
4. **`Owner` JSON form is adjacently tagged** (`{"type":"agent","name":"ops"}` / `{"type":"user"}`) with `Display`/`FromStr` for the text form `agent:ops` / `user`; serde cannot internally tag a newtype `String` variant. `TaskFilter.owner`, `created_by`, `actor` use the text form.
5. **`Budget` JSON form is tagged** (`{"type":"none"}` / `{"type":"monthly","amount":40,"currency":"USD"}`); the file form `none` / `40 USD/month` is `Display`/`FromStr`.
6. **`AgentName`** has a private field with `AgentName::new` validation (serde `try_from`/`into` String; TS `string`) rather than a pub tuple field, so an invalid name cannot exist.
7. **`AgentFile.title` defaults to the name** when absent, so a body-only file is the minimal valid file; `body` empty is refused (`empty charter`); `theuth` is refused as a file name.
8. **`replica::Change`/`ChangeBatch`/`Hlc` are not TS-exported**: `knowledge::Change` already owns the `Change` binding name and nothing in replica crosses RPC. `PeerId` and `PeerEvent` are exported (they appear in `mesh::MeshEvent`).
9. **`Spec.agent` added** alongside the `FrontMatter.agent` key (the brief said key only; the parsed value has to land somewhere the tasks worker can read).
10. **`replica.sqlite` schema** was unspecified: `sync_state(peer, table_name, last_seq, updated_ms)` + `clock(id=1, wall_ms, counter)`.
11. **`tasks` migration** adds an index on `(owner, state)` and a unique partial index on `(schedule_name, schedule_fire_ms)` (the §7.4 coalescing key); `wakes` gets an index on `(agent, claimed_by, created_ms)`; `episodes` on `(agent, opened_ms)`; `task_events` on `(task, at_ms)`.
12. `mesh::MeshEvent` and `tasks::Disposition` are tagged enums (`type` + `data` / `type`), the repo's `ScheduleEvent` idiom.
13. No `replica:` config section (none was specified).
14. `crates/rpc/src/export.rs` edited (not on the allow-list by name; it is the "rpc server impl file(s)" for TS generation).

## Not built (by design — each worker's brief)
Every actor handler, `replica::attach`, the tool bodies, the schedule fire → `TaskCreator` path, desks, election, the iroh endpoint, the board/Agents UI, `crates/sandbox`.

## Tests written (not run, per the brief)
- `crates/roster/src/file.rs`: `minimal_file_is_body_plus_defaults`, `full_file`, `invalid_name_is_refused` (also covers `theuth`, unknown key, empty charter).
- `crates/roster/src/types.rs`: `budget_forms`.
- `crates/tasks/src/types.rs`: `owner_text_round_trip`, `owner_json_is_tagged`, `where_is_renamed_on_the_wire`.

## Cross-crate handle pattern (documented in the contract §0)
`ActorRef::<A>::lookup(NAME)` for a one-shot; `rt.lookup_when::<A>(NAME).await` inside `tokio::spawn` when the target may start later — exactly `schedule::tools::actor()` and `SchedulePlugin::start`'s wait for `tools::REGISTRY_ACTOR`. Behaviour crosses crates through `Arc<dyn Trait>` registered once by a `Register*` message.

## Checks
- `cargo check -p replica -p mesh -p tasks -p roster --tests` (after commit 1's files): `Finished dev profile … in 4m 09s`, no warnings, no errors.
- `cargo check -p theuth` (after commit 3's files): `Finished dev profile … in 6m 26s`, no warnings, no errors.
- `cargo check -p theuth --tests` (after the project commit): `Finished dev profile … in 27.73s`, no warnings, no errors.
- `just ts-desktop` (= `just ts` + copy into the Vite tree, run once): `test result: ok. 4 passed`, 333 files in `bindings/`; every new type present (`MeshStatus`, `Peer`, `HubPref`, `AgentFile`, `AgentSummary`, `AgentName` → `string`, `Owner` → tagged union, `Task`, `TaskPatch` with `?: T | null` fields). ts-rs printed its usual "failed to parse serde attribute" warnings for `transparent`/`try_from`/`into` (pre-existing types trigger the same); output unaffected. **Ran before the project commit**, so the gitignored `bindings/` lack `project` until the orchestrator's next `just ts`; `api.ts` needs no change for it.
- `bunx tsc --noEmit` in `crates/desktop/ui` (after `bun install --frozen-lockfile`): 60 errors, all pre-existing in `packages/apps/**` and `src/components/apps/AppHost.tsx` (missing `@openuidev/*` packages); **0 errors in `src/rpc/` or `bindings/`**.
- Unit tests written, not run (brief).
- `/tmp/67/contract-ready` touched.

DONE C
