//! 227: Settings by machine. The owner's app is connected to one engine (the "hub" of this
//! document — whichever engine the window talks to); Settings tabs whose data lives on each
//! machine reach any other engine of the mesh through [`MachinesApi::engine_call`]:
//! a call for this engine dispatches into this engine's own API ([`install_local_api`]), any
//! other is forwarded once over `theuth/rpc/1` with the existing forwarder
//! ([`forward::to_peer_via`], the core of `call_peer`). Only settings getters and setters of
//! Settings' machine tabs pass ([`SETTINGS_FORWARDABLE`]); nothing that carries a secret value,
//! a credential, chat content or a session action does.
//!
//! No press crosses the hop: a forwarded call runs on the target as the internal peer, which
//! is never a human press. A setter that needs the owner's press therefore answers
//! [`NEEDS_PRESS`] on any other machine ([`needs_press`], [`classify`]); on this engine it runs
//! as the caller's own connection. Agents (the internal peer: MCP console, ACP agent half) may
//! not use these RPCs at all.
//!
//! Errors are normalised for the client: [`NOT_FORWARDABLE`], [`OFFLINE`], [`SEALED`],
//! [`NOT_APPLICABLE`], [`NEEDS_PRESS`] ([`classify`]).

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use crate::rpc_forward as forward;
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use jsonrpsee::types::error::INVALID_PARAMS_CODE;
use jsonrpsee::types::{ErrorObjectOwned, Response, ResponsePayload};
use replica::{PeerId, Transport};
use rpc_core::{Server, ask, send};
use runtime::Runtime;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_json::value::RawValue;

/// JSON-RPC error: the method is not one Settings may run on another machine.
pub const NOT_FORWARDABLE: i32 = -32050;
/// JSON-RPC error: the target engine is not connected (or gave no answer within 30 s).
pub const OFFLINE: i32 = -32051;
/// JSON-RPC error: the target's vault is sealed or locked; the call applies after an unlock.
pub const SEALED: i32 = -32052;
/// JSON-RPC error: the target has no such method, or the subsystem behind it is not running
/// there (cfg'd out on iOS, gated off on a headless node).
pub const NOT_APPLICABLE: i32 = -32053;
/// JSON-RPC error: the method needs the owner's press and this call carries none (any call to
/// another machine, a queued retry); the client offers "press to finish" on that machine.
pub const NEEDS_PRESS: i32 = -32054;

/// Most engines one call fans out to.
pub const MAX_ENGINES: usize = 32;
/// Most getters one `settings_values` reads.
pub const MAX_KEYS: usize = 64;

/// The message every sealed-vault error starts with (feat/sealed-vault's
/// `crates/secrets/src/envelope.rs`); mapped to [`SEALED`] whatever its code.
const SEALED_PREFIX: &str = "the vault is sealed";

/// The settings getters and setters a Settings tab of scope `machine` calls, sorted for
/// [`forwardable`]'s binary search. Left out on purpose (227 §2): anything that carries or
/// returns a secret value, a token or a credential (`set_secret`, `login`/`logout`,
/// `acp_login`, `add_provider` / `add_account` with their key, `mcp_server_get` /
/// `mcp_server_set` / `mcp_server_rotate_token` with the bearer token, external-secrets
/// settings, `list_logins`, `list_secret_connections`, calendar sources whose URLs carry
/// tokens, `sync_providers` / `apply_providers`), chat and session actions or content
/// (`send_input`, `steer`, `desk_open`, `screen_context`, `list_sessions`, meetings,
/// dictation history and audio, terminals, sandbox lifecycle), mesh and vault control
/// (`mesh_set_*`, `mesh_rename`, `vault_*`), contacts and the pairing ceremonies, and plugin
/// install/uninstall. The Vault tab forwards its own RPCs (`engine` param), not through here.
pub const SETTINGS_FORWARDABLE: &[&str] = &[
    "a2a_export_put", "a2a_exports", "a2a_status", "add_acp", "autoresearch.mechanism",
    "autoresearch.settings.get", "autoresearch.settings.set", "calendar_settings_get",
    "calendar_settings_set", "computer_permissions", "config_flag_get", "config_flag_set",
    "decision_loaded", "decision_mode", "decision_stats", "decision_training",
    "delete_decision_training", "delete_tool_category", "dictation_status", "dictionary_counts",
    "dictionary_delete", "dictionary_list", "dictionary_put", "dictionary_search",
    "documents_status", "endless_settings_get", "endless_settings_set", "engine_caps",
    "escalate_effort", "escalate_model", "get_always_ask", "get_approvals", "get_communication",
    "handoffs_list", "info", "knowledge_harvester_effort", "knowledge_harvester_model", "list_acp",
    "list_acp_registry", "list_aws_profiles", "list_builtins", "list_input_devices", "list_mcp",
    "list_models", "list_project_styles", "list_prompt_sections", "list_providers",
    "list_schedules", "list_tool_categories", "local_servers_probe", "mcp_server_audit",
    "meeting_live_diarization", "meeting_settings_get", "meeting_settings_set",
    "meeting_speaker_delete", "meeting_speakers", "meeting_template_delete", "meeting_template_put",
    "meeting_template_reset", "meeting_templates", "mesh_status", "plugin_approve", "plugin_get",
    "plugin_list", "plugin_logs", "plugin_restart", "plugin_set_enabled", "plugin_set_hidden",
    "plugin_set_order", "polish_effort", "polish_model", "provider_models", "refresh_all_models",
    "refresh_provider_models", "reload_a2a", "remove_acp", "remove_provider",
    "reset_prompt_section", "reset_provider_models", "sandboxes_list", "search_provider_test",
    "search_settings_get", "search_settings_set", "search_status", "search_test", "sessions_status",
    "set_advisor_default_effort", "set_advisor_default_model", "set_advisor_immune_turns",
    "set_advisor_max_consults", "set_advisor_notes_per_update", "set_advisor_sync_backlog",
    "set_always_ask", "set_approvals", "set_auto_archive", "set_aws_profile",
    "set_builtins_disabled", "set_communication", "set_compact_style", "set_decision_mode",
    "set_decision_share", "set_decision_training", "set_default_tool_categories", "set_effort",
    "set_end_phrases", "set_escalate_effort", "set_escalate_model", "set_hint_limit",
    "set_input_devices", "set_knowledge_harvester_effort", "set_knowledge_harvester_model",
    "set_max_salvages", "set_max_tool_rounds", "set_mcp_category", "set_mcp_hidden",
    "set_meeting_live_diarization", "set_model", "set_model_favorite", "set_model_limits",
    "set_peek_threshold", "set_polish_effort", "set_polish_enabled", "set_polish_model",
    "set_prompt_section", "set_prompt_section_enabled", "set_pty_idle", "set_schedule_run_timeout",
    "set_shape", "set_shape_enabled", "set_status_every", "set_stream_idle", "set_stt_engine",
    "set_subagent_effort", "set_subagent_model", "set_subagent_timeout", "set_theuth_agent_effort",
    "set_theuth_agent_model", "set_title_effort", "set_title_model", "set_tool_category",
    "set_vision_effort", "set_vision_model", "subagent_effort", "subagent_model",
    "test_summary_call", "theuth_agent_effort", "theuth_agent_model", "title_effort", "title_model",
    "update_apply", "update_status", "upsert_tool_category", "vision_effort", "vision_model",
    "voice_mlx_download", "voice_mlx_status", "voice_settings", "voice_status",
];

/// Substrings no allowlisted name may contain (227 §2 security review).
pub const NEVER_FORWARDABLE: &[&str] = &[
    "send_input", "steer", "terminal", "audio", "meeting_get", "list_sessions", "vault_",
    "mesh_set_", "contact_", "logout", "login", "secret", "token", "calendar_source",
];

/// Whether `method` may run on another machine through [`MachinesApi::engine_call`].
pub fn forwardable(method: &str) -> bool {
    SETTINGS_FORWARDABLE.binary_search(&method).is_ok()
}

/// One machine's outcome in [`MachinesApi::engine_call_all`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum NodeStatus {
    /// Applied; `value` is the method's answer.
    Ok,
    /// The method answered an error (`message`).
    Failed,
    /// Not connected, or no answer within 30 s.
    Offline,
    /// Its vault is sealed or locked.
    Sealed,
    /// The method does not exist or its subsystem is not running there.
    NotApplicable,
    /// Needs the owner's press on that machine; no press crosses the mesh.
    NeedsPress,
}

/// One machine's row of [`MachinesApi::engine_call_all`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct NodeResult {
    /// Mesh id.
    pub engine: String,
    /// Display name (the mesh's; the id when unknown).
    pub name: String,
    /// How the call went.
    pub status: NodeStatus,
    /// The error, for every status but `ok`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub message: Option<String>,
    /// The change applies after theuth restarts there ([`restart_required`]).
    pub restart_required: bool,
    /// The answer on `ok`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub value: Option<Value>,
}

/// What an engine's host offers ([`MachinesApi::engine_caps`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct EngineCaps {
    /// Mesh id (this engine's store id on a lone engine).
    pub engine: String,
    /// Display name.
    pub name: String,
    /// `std::env::consts::OS`: `macos`, `linux`, `ios`, `windows`, …
    pub platform: String,
    /// A display is reachable (the startup probe), or the node's watchable desktop
    /// (`desktop.json`).
    pub display: bool,
    /// An input device is listed (`list_input_devices`); without the voice plugin, the
    /// startup audio probe.
    pub mic: bool,
    /// A person drives this engine (desktop app, TUI, …), not a plain `t --headless`.
    pub interactive_user: bool,
    /// `!interactive_user`: a node nobody sits at.
    pub headless: bool,
    /// This engine holds the hub lease (`true` with no mesh).
    pub hub: bool,
}

/// The host as probed at start (`theuth::subsystems::Env`), installed once by the binary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct HostCaps {
    /// A person drives this engine (not a plain `t --headless`).
    pub interactive: bool,
    /// A display is reachable.
    pub display: bool,
    /// An audio device is present.
    pub audio: bool,
}

static HOST: OnceLock<HostCaps> = OnceLock::new();

/// Record the startup probe; later calls are ignored. The `t` binary installs its
/// `subsystems::Env`; an embedding host (the iOS app) installs its own.
pub fn install_host_caps(caps: HostCaps) {
    let _ = HOST.set(caps);
}

/// The installed probe; without one, nothing is assumed (no person, display or microphone).
fn host_caps() -> HostCaps {
    HOST.get().copied().unwrap_or_default()
}

/// This runtime's full API, for calls this engine runs itself.
#[derive(Clone, Default)]
struct LocalApi(Arc<OnceLock<jsonrpsee::Methods>>);

/// Install the composed API `methods` (`rpc_compose::methods`) as the target of local
/// [`MachinesApi::engine_call`]s; the first install per runtime wins.
pub fn install_local_api(rt: &Runtime, methods: &jsonrpsee::Methods) {
    let _ = rt.extension(LocalApi::default).0.set(methods.clone());
}

fn local_api(rt: &Runtime) -> Option<jsonrpsee::Methods> {
    rt.extension(LocalApi::default).0.get().cloned()
}

/// Settings-by-machine methods owned by this plugin.
#[rpc_core_macros::rpc_descriptors(owner = "agent", descriptors = rpc_descriptors, audience = "owner")]
#[rpc(server, client)]
pub trait MachinesApi {
    /// 227: run Settings method `method` with `params` (by name or position) on mesh engine
    /// `engine` (this engine or a paired one; this engine: here, as the caller). Only
    /// [`SETTINGS_FORWARDABLE`] methods (else `-32050`). No press crosses the mesh: a setter
    /// that needs one is `-32054` on another machine. Errors: `-32051` offline, `-32052` vault
    /// sealed, `-32053` not applicable there, else the target's own. The owner's app only:
    /// agents (the internal peer) and guests are refused.
    #[method(name = "engine_call", with_extensions)]
    async fn engine_call(&self, engine: String, method: String, params: Value) -> RpcResult<Value>;
    /// 227: [`engine_call`](MachinesApiServer::engine_call) on every engine of `engines`
    /// (deduplicated, ≤ 32, each this one or a paired one; default: this one and every paired
    /// engine), concurrently, 30 s each; one row per engine, never an error for one machine.
    #[method(name = "engine_call_all", with_extensions)]
    async fn engine_call_all(
        &self,
        method: String,
        params: Value,
        engines: Option<Vec<String>>,
    ) -> RpcResult<Vec<NodeResult>>;
    /// 227: what this engine's host offers (display, microphone, a person at it). Cheap. The
    /// owner's app, or another engine forwarding for it.
    #[method(name = "engine_caps", with_extensions)]
    async fn engine_caps(&self) -> RpcResult<EngineCaps>;
    /// 227 ("differs on N"): each getter of `keys` (allowlisted, ≤ 64, called without params)
    /// on each engine of `engines` (as `engine_call_all`; `[]`: this one and every paired
    /// engine): `{engine: {key: value | null}}`, `null` where the getter failed or does not
    /// apply; offline engines are omitted.
    #[method(name = "settings_values", with_extensions)]
    async fn settings_values(
        &self,
        engines: Vec<String>,
        keys: Vec<String>,
    ) -> RpcResult<BTreeMap<String, BTreeMap<String, Option<Value>>>>;
}

/// Where one call can go: this engine (id, its own API) and the mesh.
#[derive(Clone)]
pub struct Reach {
    /// This engine's ids: the mesh id and the store's (they differ only on a lone engine).
    pub me: Vec<String>,
    /// This engine's composed API ([`install_local_api`]); `None` before it is installed.
    pub local: Option<jsonrpsee::Methods>,
    /// The mesh transport; `None` without a mesh.
    pub transport: Option<Arc<dyn Transport>>,
}

impl Reach {
    fn is_me(&self, engine: &str) -> bool {
        engine.is_empty() || self.me.iter().any(|m| m == engine)
    }
}

fn error(code: i32, msg: impl Into<String>) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(code, msg.into(), None::<()>)
}

fn not_forwardable(method: &str) -> ErrorObjectOwned {
    error(NOT_FORWARDABLE, format!("not forwardable: {method}"))
}

/// The owner's app: an authenticated, non-guest connection that is not the internal peer
/// (an agent's MCP console or ACP half). `forwarded_ok`: a call another engine forwarded
/// (internal by construction) is admitted too — `engine_caps` only.
pub fn owner_app(ext: &Extensions, forwarded_ok: bool) -> RpcResult<()> {
    let peer = rpc_core::owner(ext)?;
    if peer.is_internal() && !(forwarded_ok && ext.get::<forward::Forwarded>().is_some()) {
        return Err(error(
            rpc_core::UNAUTHORIZED,
            "settings across machines are for the owner's app, not agents",
        ));
    }
    Ok(())
}

/// The client's error codes for a call's raw error: unreachable → [`OFFLINE`]; sealed vault
/// (`-32052`, or a message starting "the vault is sealed", or a locked vault) → [`SEALED`];
/// method not found / plugin not running → [`NOT_APPLICABLE`]; a refusal for want of a press
/// → [`NEEDS_PRESS`]; anything else unchanged.
pub fn classify(e: ErrorObjectOwned) -> ErrorObjectOwned {
    let msg = e.message();
    let lower = msg.to_ascii_lowercase();
    let code = match e.code() {
        forward::OWNER_UNREACHABLE => OFFLINE,
        SEALED => SEALED,
        _ if msg.starts_with(SEALED_PREFIX)
            || lower.contains("vault is locked")
            || lower.contains("vault is sealed") =>
        {
            SEALED
        }
        jsonrpsee::types::error::METHOD_NOT_FOUND_CODE | rpc_core::NOT_RUNNING => NOT_APPLICABLE,
        _ if lower.contains("needs a human press") || lower.contains("needs the owner's press") => {
            NEEDS_PRESS
        }
        c => c,
    };
    if code == e.code() {
        return e;
    }
    ErrorObjectOwned::owned(code, msg.to_owned(), e.data().map(|d| d.to_owned()))
}

/// Params as sent: `null` → no params.
fn wire_params(params: &Value) -> Value {
    match params {
        Value::Null => Value::Array(Vec::new()),
        p => p.clone(),
    }
}

/// Run `method` in `local` as the caller: the caller's extensions (its connection, any
/// [`forward::Forwarded`] marker) ride along.
async fn dispatch(
    local: &jsonrpsee::Methods,
    ext: &Extensions,
    method: &str,
    params: &Value,
) -> RpcResult<Value> {
    let mut methods = local.clone();
    methods.extensions_mut().extend(ext.clone());
    let line = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": wire_params(params),
    })
    .to_string();
    let (raw, _) = methods
        .raw_json_request(&line, 1)
        .await
        .map_err(|e| rpc_core::failed(e.to_string()))?;
    let rp: Response<&RawValue> =
        serde_json::from_str(raw.get()).map_err(|e| rpc_core::failed(e.to_string()))?;
    match rp.payload {
        ResponsePayload::Success(v) => {
            serde_json::from_str(v.get()).map_err(|e| rpc_core::failed(e.to_string()))
        }
        ResponsePayload::Error(e) => Err(e.into_owned()),
    }
}

/// Allowlisted calls that refuse (or would park a human-only card) without the owner's press:
/// `plugin_approve` and an always-ask removal (`set_always_ask` with `on: false`). Refused up
/// front as [`NEEDS_PRESS`] wherever no press is present, so no card waits on a target.
pub fn needs_press(method: &str, params: &Value) -> bool {
    match method {
        "plugin_approve" => true,
        "set_always_ask" => {
            params.get("on").or_else(|| params.get(2)).and_then(Value::as_bool) == Some(false)
        }
        _ => false,
    }
}

/// One call: here when `engine` is this engine, else forwarded once (never with a press).
/// Errors are [`classify`]'d.
pub async fn call_one(
    reach: &Reach,
    ext: &Extensions,
    engine: &str,
    method: &str,
    params: &Value,
) -> RpcResult<Value> {
    if !forwardable(method) {
        return Err(not_forwardable(method));
    }
    let here = reach.is_me(engine);
    // Here the caller's own connection may be the press; a forwarded call never is.
    if needs_press(method, params) && !(here && secrets::rpc::human_press(ext)) {
        let what = if here { "" } else { " on that machine" };
        return Err(error(NEEDS_PRESS, format!("{method} needs the owner's press{what}")));
    }
    if here {
        let Some(local) = &reach.local else {
            return Err(error(NOT_APPLICABLE, "this engine has no local API to run settings calls"));
        };
        return match tokio::time::timeout(forward::TIMEOUT, dispatch(local, ext, method, params))
            .await
        {
            Ok(r) => r.map_err(classify),
            Err(_) => Err(rpc_core::failed(format!(
                "{method}: no answer within {}s",
                forward::TIMEOUT.as_secs()
            ))),
        };
    }
    if ext.get::<forward::Forwarded>().is_some() {
        return Err(error(
            NOT_APPLICABLE,
            format!("{engine}: a forwarded call is never forwarded again"),
        ));
    }
    let peer = PeerId(engine.to_owned());
    let sent = forward::to_peer_via::<Value>(
        ext,
        reach.transport.clone(),
        &peer,
        method,
        wire_params(params),
        forward::TIMEOUT,
    )
    .await;
    match sent {
        Ok(Some(v)) => Ok(v),
        Ok(None) => Err(error(OFFLINE, format!("{engine} is offline"))),
        Err(e) => Err(classify(e)),
    }
}

/// Whether an answer means a restart is due: the reply's own `restart_required` — what
/// `config_flag_set` says for a plugin's `<section>.enabled` (plugins keep their boot state).
pub fn restart_required(value: &Value) -> bool {
    value.get("restart_required").and_then(Value::as_bool) == Some(true)
}

/// [`call_one`] on every `(engine, name)` of `targets`, concurrently, as rows.
pub async fn call_all(
    reach: &Reach,
    ext: &Extensions,
    targets: &[(String, String)],
    method: &str,
    params: &Value,
) -> Vec<NodeResult> {
    let calls = targets.iter().map(|(engine, name)| async move {
        node_result(engine, name, call_one(reach, ext, engine, method, params).await)
    });
    futures::future::join_all(calls).await
}

fn node_result(engine: &str, name: &str, r: RpcResult<Value>) -> NodeResult {
    let (status, message, value) = match r {
        Ok(v) => (NodeStatus::Ok, None, Some(v)),
        Err(e) => {
            let status = match e.code() {
                OFFLINE => NodeStatus::Offline,
                SEALED => NodeStatus::Sealed,
                NOT_APPLICABLE => NodeStatus::NotApplicable,
                NEEDS_PRESS => NodeStatus::NeedsPress,
                _ => NodeStatus::Failed,
            };
            (status, Some(e.message().to_owned()), None)
        }
    };
    NodeResult {
        engine: engine.to_owned(),
        name: name.to_owned(),
        restart_required: value.as_ref().is_some_and(restart_required),
        status,
        message,
        value,
    }
}

/// [`settings_values`](MachinesApiServer::settings_values) from each key's rows: `null` for a
/// failed or inapplicable getter, engines offline for every key left out.
pub fn values_of(
    keys: &[String],
    rows: Vec<Vec<NodeResult>>,
) -> BTreeMap<String, BTreeMap<String, Option<Value>>> {
    let mut out: BTreeMap<String, BTreeMap<String, Option<Value>>> = BTreeMap::new();
    for (key, rows) in keys.iter().zip(rows) {
        for row in rows {
            if row.status == NodeStatus::Offline {
                continue;
            }
            out.entry(row.engine).or_default().insert(key.clone(), row.value);
        }
    }
    out
}

/// The caps from the startup probe, the watchable desktop and the device listing.
pub fn caps_of(
    engine: String,
    name: String,
    host: HostCaps,
    desktop: bool,
    mic: Option<bool>,
    hub: bool,
) -> EngineCaps {
    EngineCaps {
        engine,
        name,
        platform: std::env::consts::OS.to_owned(),
        display: host.display || desktop,
        mic: mic.unwrap_or(host.audio),
        interactive_user: host.interactive,
        headless: !host.interactive,
        hub,
    }
}

/// The mesh as Settings names it: this engine (ids, name) and every paired engine.
pub struct Roster {
    /// This engine's ids, the mesh id first.
    pub me: Vec<String>,
    /// This engine's display name.
    pub name: String,
    /// Every paired engine: `(id, name)`.
    pub peers: Vec<(String, String)>,
}

async fn roster() -> RpcResult<Roster> {
    let store = ask::<crate::Agents, _>(crate::AGENT_ACTOR, crate::DurableStore).await?;
    let mut me = vec![store.engine().to_owned()];
    let status = match mesh::rpc_support::mesh_actor() {
        Ok(m) => send(&m, mesh::Status).await.ok(),
        Err(_) => None,
    };
    let Some(st) = status else {
        return Ok(Roster { me, name: platform::process::hostname().to_owned(), peers: Vec::new() });
    };
    if !me.contains(&st.self_id.0) {
        me.insert(0, st.self_id.0.clone());
    }
    Ok(Roster {
        me,
        name: st.self_name,
        peers: st.peers.into_iter().map(|p| (p.id.0, p.name)).collect(),
    })
}

impl Roster {
    /// `engine` with its name, when it is this engine (or empty) or a paired one.
    pub fn target(&self, engine: &str) -> RpcResult<(String, String)> {
        if engine.is_empty() || self.me.iter().any(|m| m == engine) {
            return Ok((engine.to_owned(), self.name.clone()));
        }
        self.peers
            .iter()
            .find(|(p, _)| p == engine)
            .cloned()
            .ok_or_else(|| error(INVALID_PARAMS_CODE, format!("unknown engine {engine}")))
    }

    /// `engines` deduplicated with names (each this engine or a paired one), or this engine
    /// then every peer when `None`; at most [`MAX_ENGINES`].
    pub fn targets(&self, engines: Option<Vec<String>>) -> RpcResult<Vec<(String, String)>> {
        let out: Vec<(String, String)> = match engines {
            None => std::iter::once((self.me[0].clone(), self.name.clone()))
                .chain(self.peers.iter().cloned())
                .collect(),
            Some(ids) => {
                let mut out: Vec<(String, String)> = Vec::new();
                for id in ids {
                    let t = self.target(&id)?;
                    if !out.iter().any(|(e, _)| *e == t.0) {
                        out.push(t);
                    }
                }
                out
            }
        };
        if out.len() > MAX_ENGINES {
            return Err(error(INVALID_PARAMS_CODE, format!("at most {MAX_ENGINES} engines")));
        }
        Ok(out)
    }
}

/// `keys` deduplicated, each allowlisted, at most [`MAX_KEYS`].
pub fn getter_keys(keys: Vec<String>) -> RpcResult<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for k in keys {
        if !forwardable(&k) {
            return Err(not_forwardable(&k));
        }
        if !out.contains(&k) {
            out.push(k);
        }
    }
    if out.len() > MAX_KEYS {
        return Err(error(INVALID_PARAMS_CODE, format!("at most {MAX_KEYS} keys")));
    }
    Ok(out)
}

async fn reach(rt: &Runtime, me: Vec<String>) -> Reach {
    Reach { me, local: local_api(rt), transport: forward::transport().await }
}

#[async_trait]
impl MachinesApiServer for Server {
    async fn engine_call(
        &self,
        ext: &Extensions,
        engine: String,
        method: String,
        params: Value,
    ) -> RpcResult<Value> {
        owner_app(ext, false)?;
        if !forwardable(&method) {
            return Err(not_forwardable(&method));
        }
        let roster = roster().await?;
        roster.target(&engine)?;
        let reach = reach(&self.rt, roster.me).await;
        call_one(&reach, ext, &engine, &method, &params).await
    }

    async fn engine_call_all(
        &self,
        ext: &Extensions,
        method: String,
        params: Value,
        engines: Option<Vec<String>>,
    ) -> RpcResult<Vec<NodeResult>> {
        owner_app(ext, false)?;
        if !forwardable(&method) {
            return Err(not_forwardable(&method));
        }
        let roster = roster().await?;
        let targets = roster.targets(engines)?;
        let reach = reach(&self.rt, roster.me).await;
        Ok(call_all(&reach, ext, &targets, &method, &params).await)
    }

    async fn engine_caps(&self, ext: &Extensions) -> RpcResult<EngineCaps> {
        owner_app(ext, true)?;
        let roster = roster().await?;
        let mic = match local_api(&self.rt) {
            Some(local) => dispatch(&local, ext, "list_input_devices", &Value::Null)
                .await
                .ok()
                .map(|v| v.as_array().is_some_and(|a| !a.is_empty())),
            None => None,
        };
        let desktop = self.rt.data_dir.join(crate::rpc_engine::DESKTOP_FILE).exists();
        Ok(caps_of(roster.me[0].clone(), roster.name, host_caps(), desktop, mic, mesh::is_hub()))
    }

    async fn settings_values(
        &self,
        ext: &Extensions,
        engines: Vec<String>,
        keys: Vec<String>,
    ) -> RpcResult<BTreeMap<String, BTreeMap<String, Option<Value>>>> {
        owner_app(ext, false)?;
        let keys = getter_keys(keys)?;
        let roster = roster().await?;
        let targets = roster.targets((!engines.is_empty()).then_some(engines))?;
        let reach = reach(&self.rt, roster.me).await;
        let rows = futures::future::join_all(
            keys.iter().map(|k| call_all(&reach, ext, &targets, k, &Value::Null)),
        )
        .await;
        Ok(values_of(&keys, rows))
    }
}

pub(super) fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule {
    rpc_core::DescribedModule::new(MachinesApiServer::into_rpc(server), rpc_descriptors())
        .expect("agent RPC descriptors")
}

#[cfg(test)]
#[path = "rpc_machines/tests.rs"]
mod tests;
