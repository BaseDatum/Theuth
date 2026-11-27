# 227 — Settings by machine

Status: contract fixed 2026-11-27 (`feat/settings-machine-picker`). Both builders (engine: `crates/**`; UI:
`crates/desktop/ui/**`) code against §1 verbatim. Deviations land here first in a commit titled `contract: …`.

## 1. Contract

### Engine RPCs (crates/agent, registered like other rpc_* modules, owner-only)

1. `engine_call(engine: String, method: String, params: Value, pressed: Option<PressToken>) -> Value`
   - If `engine` is this engine's id → dispatch locally. Else forward with rpc_forward::call_peer (engine_peer to
     resolve) — NO new forwarder.
   - Allowlist: a static list `SETTINGS_FORWARDABLE: &[&str]` of every method used by Settings tabs whose scope is
     'machine' (derive from crates/desktop/ui/src/components/settings/categories/*.tsx `required.methods` + setters
     those tabs call; exclude anything that carries secret VALUES: set_secret, secret_*, login, provider key setters
     that take a key, mcp_server_rotate_token, external-secrets setters). Anything else → error code -32050 "not
     forwardable".
   - Owner press: the UI passes `pressed` only for calls started by an owner UI press. On the hub, verify the caller
     with secrets::rpc::human_press(ext); if true, forward with a `ForwardedPress` marker (new, next to `Forwarded` in
     rpc_forward.rs) carried in the forwarded request (e.g. a `_press` field in the JSON-RPC line, signed/MAC'd with
     the mesh link key or simply trusted ONLY because the theuth/rpc/1 stream is authenticated owner-mesh — document
     which), and on the receiving engine `human_press(ext)` returns true when `ForwardedPress` is present AND the
     forwarding peer is an owner engine of this mesh. Never set it for agent-originated calls. Tests must prove an
     agent/internal call cannot obtain it.
   - Errors: peer offline → JSON-RPC error code -32051 "offline"; vault sealed (whatever error sealed-vault/secrets
     returns, else map 'sealed'/'locked' errors) → -32052 "sealed"; method not found / not applicable on target (cfg'd
     out on iOS, subsystem off) → -32053 "not_applicable" with message.
2. `engine_call_all(method, params, engines: Option<Vec<String>>, pressed) -> Vec<NodeResult>` where NodeResult =
   `{engine, name, status: "ok"|"failed"|"offline"|"sealed"|"not_applicable", message?, restart_required: bool,
   value?}`; concurrent; default = this engine + every mesh peer; per-node 30 s timeout (call_peer's).
   `restart_required` true when the method is config_flag_set for a plugin/*.enabled or subsystems.* key (see
   rpc-compose api.rs ~L134-141, theuth/src/subsystems.rs).
3. `engine_caps() -> {engine, name, platform, display: bool, mic: bool, interactive_user: bool, headless: bool, hub:
   bool}` — local, cheap; display/user from the platform (macOS: a GUI session present; Linux:
   DISPLAY/WAYLAND_DISPLAY or the node's desktop stream; cloud nodes without desktop → false), mic from the existing
   input-device listing. Also forwardable (in the allowlist).
4. `settings_values(engines: Vec<String>, keys: Vec<String>) -> {[engine]: {[key]: Value|null}}` for piece 4 'differs
   on N': keys are the getter methods the copyable tabs read (e.g. 'get_model_settings' style names — use the real
   getters); implemented via engine_call_all of getters; offline engines omitted.

Wire: add to the openrpc schema / TS bindings per repo convention (`just ts-desktop`; accept the baseline with `just
wire-schema-accept` — additive only).

Sealed mapping (addendum, orchestrator 2026-11-27): feat/sealed-vault returns JSON-RPC -32052 with a message starting
"the vault is sealed" (`crates/secrets/src/envelope.rs`). A peer's -32052 maps to status `sealed` unchanged; ANY error
whose message starts with "the vault is sealed" is also mapped to -32052 (engines running that branch before the code
lands).

### Client (crates/desktop/ui)

- `lib/mesh-machines.ts`: `useMeshMachines()` → `Machine[] = {id, name, kind: 'local'|'hub'|'peer'|'cloud', online,
  sealed, direct|relay, caps?}` from mesh_status peers + cloud list (components/cloud/queries.ts useCloudList) +
  engine_caps; LOCAL first (label from existing engines.ts LOCAL label — 'This Mac' today, 'This iPhone/iPad' once
  feat/ios-engine lands), then hub, then peers. `components/settings/MachineRow.tsx`: dot (online/offline/sealed),
  name, kind/presence tag. These become shared with CloudNodesSettings later (another session will migrate it — do not
  edit components/cloud/).
- `lib/engine-call.ts`: `forEngine(api, target)` Proxy: when target === active engine → the api itself; else wraps
  each method m(...args) as api.engine_call(target, m, params, pressed?) (positional→named param mapping via the
  bindings' param names).
- Settings: a machine picker at the top of Settings (phone: header row + menu; iPad/desktop: top of the settings
  sidebar), state per window, default = connected engine. Tabs with scope 'machine' use `forEngine(...)`;
  'everywhere'/'hub'/'device' tabs ignore the picker and say so subtly when a non-default machine is picked. Each
  machine tab header shows 'Editing <name>'. Unreachable/offline machines are listed disabled. After writes invalidate
  that target's queries; while the target is remote, refetch the visible tab every 5 s.
- n/a: use engine_caps of the picked machine with the existing settings/scope.ts notApplicable (replace its TODO).
- 'Set on all machines…' (piece 3): on each field/section of copyable tabs (start with: models defaults, helpers,
  loop, memory/context, approvals, advisor, categories, builtins, search settings, system prompt sections; providers
  via existing sync_providers/apply_providers flow — never keys): opens a sheet listing machines with checkboxes (all
  reachable ticked by default, INCLUDING this iPhone/iPad/Mac local engine; hub-only and n/a machines shown
  unticked+disabled with the reason), then calls engine_call_all and shows per-machine results exactly like the mock
  (✓ applied / ◷ offline — applies when it reconnects / ◷ vault locked — applies after unlock / – skipped: reason / ✗
  failed: msg + 'restart needed' chip). 'Retry pending' re-runs for pending ones. Pending ones are kept in a small
  client queue (per window, localStorage) and retried automatically when that machine comes online/unseals while the
  app is open; document that it is client-held, not engine-held.
- 'Differs on N' (piece 4): on copyable fields, after loading settings_values for reachable machines, show a small
  amber 'differs on N' next to the field when values differ; clicking opens a compare popover listing each machine's
  value with 'Use this everywhere' per row (→ the Set-on-all flow pre-filled).

Mock: `mesh-settings-mock.html` (owner drafts, session 01a2181b).

## 2. Engine as built (`crates/agent/src/rpc_machines.rs`)

**Contract change (security review, 2026-11-27): no press is forwarded.** §1's `pressed` / `PressToken` and the
`ForwardedPress` marker are gone. The review showed `_press` was forgeable: any local process holding
`<data_dir>/token` (agents' `t mcp` / ACP children) passes `human_press` on the hub, and a paired engine (an agent
with root on a cloud node) could write `_press` itself. A call forwarded over `theuth/rpc/1` runs on the target as
the internal peer and is never a press, as before 227. Final signatures:

- `engine_call(engine, method, params) -> Value`
- `engine_call_all(method, params, engines?) -> NodeResult[]`
- `engine_caps() -> EngineCaps`
- `settings_values(engines, keys) -> {engine: {key: Value | null}}`

Behaviour:

- **Callers.** The owner's app only: `rpc_core::owner` (no guest) and not the internal peer (MCP console, ACP agent
  half), so an agent cannot push settings to every machine. `engine_caps` also admits a call another engine
  forwarded (that engine's app asked for it).
- **Engines.** `engine` / `engines` must be this engine (or empty) or a paired engine of the roster
  (`mesh_status`), else `-32602 unknown engine <id>`; `engines` is deduplicated, at most 32 (also for the default:
  this engine and every paired engine). `settings_values` with `engines: []` = the default; `keys` deduplicated,
  allowlisted, at most 64; getters are called without params; `null` where a getter failed or does not apply;
  offline engines omitted.
- **Dispatch.** This engine: into its own composed API as the caller's own connection (`rpc_compose::methods`
  installs it per runtime). Another engine: the method itself (not `engine_call`) over the existing forwarder, 30 s,
  one hop (a forwarded call is never forwarded again). `params` by name (object) or position (array); `null` = none.
- **`-32054 "needs_press"`** and row `status: "needs_press"` (a sixth status). A setter that needs the owner's press
  answers it whenever no press is present — on any other machine always, here for an internal caller — refused up
  front for `plugin_approve` and `set_always_ask` with `on: false` (it would park a human-only card on the target),
  and mapped from a target's own refusal ("needs a human press", "needs the owner's press"). The UI shows "press to
  finish on <machine>" (the owner connects to that machine to press there); a client-queued retry gets the same.
- **Error mapping (`classify`).** `-32010` owner unreachable → `-32051`; `-32052` (feat/sealed-vault, final message
  "the vault is sealed on <name> — …", data `{engine, methods}`) passed through unchanged; any message starting "the
  vault is sealed" or containing "vault is locked" → `-32052`; `-32601` method not found and `-32001` actor not
  running → `-32053`.
- **`restart_required`** is the reply's own `restart_required: true` only — what `config_flag_set` reports for a
  plugin's `<section>.enabled`. (There is no `subsystems.*` key in `ConfigFlagKey`; `plugin_set_enabled` starts and
  stops live.)
- **`engine_caps`.** `display` / `interactive_user` / audio from the startup probe `theuth::subsystems::Env`,
  installed by `start_plugins` (`agent::rpc_machines::install_host_caps`). Without a probe nothing is assumed (no
  person, display or mic): the iOS engine installs `HostCaps { interactive: true, display: true, audio: true }` and its
  own composed API as the local target (`crates/desktop/src/ios_engine.rs`, `boot_engine`). `display` also when `<data_dir>/desktop.json` (the node's
  watchable desktop) exists; `mic` from `list_input_devices` (non-empty), else the probe's audio;
  `headless = !interactive_user`; `hub = mesh::is_hub()`.
- **The Vault tab** does not use `engine_call`: the vault RPCs take `engine` as their first positional param and
  forward themselves; no vault method is on the allowlist.
- **`SETTINGS_FORWARDABLE`** holds settings getters and setters only, including every getter `settings_values` reads
  (`info`, `subagent_model`, `theuth_agent_model`, `title_*` / `vision_*` / `escalate_*` /
  `knowledge_harvester_*`, `endless_settings_get`, `search_settings_get`, `list_prompt_sections`, `list_builtins`,
  `list_tool_categories`, …). Left out: anything carrying or returning a secret, token or credential (`set_secret`,
  `login` / `logout`, `list_logins`, `acp_login`, `add_provider` / `add_account`, `mcp_server_get` / `mcp_server_set`
  / `mcp_server_rotate_token`, external-secrets settings, `list_secret_connections`, calendar sources and events,
  `sync_providers` / `apply_providers`); chat and session actions or content (`send_input`, `steer`, `desk_open`,
  `screen_context`, `list_sessions`, `meeting_get` / `meeting_list`, dictation history and audio, `sandbox_terminal` and the sandbox
  lifecycle); mesh and vault control (`mesh_set_*`, `mesh_rename`, `vault_*`); contacts and pairing ceremonies;
  plugin install/uninstall. A test asserts no name contains `send_input`, `steer`, `terminal`, `audio`,
  `meeting_get`, `list_sessions`, `vault_`, `mesh_set_`, `contact_`, `logout`, `login`, `secret`, `token` or
  `calendar_source`. Machine tabs that use those methods (Calendar sources, Network › Machine, Messaging contacts,
  VMs, voice history, MCP server) stay usable on the connected engine only; on another machine they answer
  `-32050`.
- Out of scope, raised separately: the loopback WS token counts as a human press for any local process holding it
  (`crates/rpc/src/lib.rs:102`, pre-existing).

