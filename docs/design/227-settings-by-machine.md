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

Contract fill-ins (no change to §1's shapes):

- `PressToken = { at: number }` (ms epoch of the UI press). Advisory only: the engine the app is connected to
  forwards a press only when `secrets::rpc::human_press` passes for the calling connection; an agent (internal
  peer), a guest, an unauthenticated or closed connection never gets one, whatever it sends.
- `ForwardedPress` lives in `rpc_core` (so `secrets::rpc::human_press` can see it) and is re-exported from
  `agent::rpc_forward` beside `Forwarded`. Wire: `"_press": true` as a top-level field of the `theuth/rpc/1`
  request line. Trust model: no MAC — trusted because `theuth/rpc/1` is accepted only from engines in the live paired
  set (iroh endpoint key + mesh `Inbound` gate), and every paired engine is the owner's (contacts use `theuth/a2a/1`,
  guests never reach the mesh). The receiver (`rpc_forward::serve`) inserts the marker only when the line's method is
  on `SETTINGS_FORWARDABLE` and the opening peer is in `transport.peers()`; `_press` on any other method, inside
  `params`, or from an engine no longer paired is ignored. Plugins' own `HumanPress` (install/approve code) still
  refuses every forwarded call.
- The hub forwards the method itself (not `engine_call`) to the target, so an engine without 227 still answers
  settings calls (without the press).
- A call `engine_call` runs on this engine dispatches into this engine's composed API as the caller's own connection
  (`rpc_compose::methods` installs it per runtime); `pressed` matters only across the hop.
- `params` by name (object) or position (array); `null` = no params.
- `settings_values` with `engines: []` = this engine and every paired engine. Getters are called without params;
  `null` where a getter failed or does not apply.
- Error mapping (`classify`): `-32010` owner unreachable → `-32051`; `-32052`, a message starting "the vault is
  sealed" (feat/sealed-vault, `crates/secrets/src/envelope.rs`), or "vault is locked" → `-32052`; `-32601` method
  not found and `-32001` actor not running → `-32053`. `engine_call_all` turns them into `status` per row.
- `restart_required`: the reply's own `restart_required: true` (`config_flag_set`, plugin flags), or
  `config_flag_set` of a `subsystems.*` key.
- `engine_caps`: `display`/`interactive_user` from the startup probe `theuth::subsystems::Env` (installed by
  `start_plugins`; a host that installs none — iOS — reports a person's device); `display` also when
  `<data_dir>/desktop.json` (the node's watchable desktop) exists; `mic` from `list_input_devices` (non-empty),
  else the startup audio probe; `headless = !interactive_user`; `hub = mesh::is_hub()`.
- Allowlist exclusions beyond §1: `mcp_server_get` (returns the token), `add_account`, `acp_login`,
  `sync_providers` / `apply_providers` (may carry keys; own owner-press flow), and the pairing ceremonies
  (`contact_window`, `contact_pair`, `contact_answer`, `contact_nearby`, `mesh_nearby`, `mesh_nearby_watch`,
  `mesh_pair_nearby`, `mesh_pair_answer`): their tickets are credentials and they need a person at the machine.

