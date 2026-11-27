//! 227: Settings by machine. The owner's app is connected to one engine (the "hub" of this
//! document — whichever engine the window talks to); Settings tabs whose data lives on each
//! machine reach any other engine of the mesh through [`MachinesApi::engine_call`]:
//! a call for this engine dispatches into this engine's own API ([`install_local_api`]), any
//! other is forwarded once over `theuth/rpc/1` with the existing forwarder
//! ([`forward::to_peer_via`], the core of `call_peer`). Only the methods Settings' machine tabs
//! use ([`SETTINGS_FORWARDABLE`]) pass; nothing that carries a secret value does.
//!
//! The owner's press crosses the hop as `"_press": true` on the request line, set only when the
//! UI passed `pressed` AND this engine's [`secrets::rpc::human_press`] saw a press; the target
//! turns it into [`rpc_core::ForwardedPress`] ([`forward::honoured_press`]), which its own
//! `human_press` counts. Calls this engine runs itself run as the caller's own connection.
//!
//! Errors are normalised for the client: [`NOT_FORWARDABLE`], [`OFFLINE`], [`SEALED`],
//! [`NOT_APPLICABLE`] ([`classify`]).

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use crate::rpc_forward as forward;
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
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
/// JSON-RPC error: the method needs the owner's press and this call carries none (a queued
/// retry, an agent); the client offers "press to finish".
pub const NEEDS_PRESS: i32 = -32054;

/// The message every sealed-vault error starts with (feat/sealed-vault's
/// `crates/secrets/src/envelope.rs`); mapped to [`SEALED`] before that code lands everywhere.
const SEALED_PREFIX: &str = "the vault is sealed";

/// Every method a Settings tab of scope `machine` calls (its `required.methods`, the getters and
/// setters its components call), sorted for [`forwardable`]'s binary search. Left out on
/// purpose: anything that carries a secret value or a credential (`set_secret`, `login`,
/// `acp_login`, `add_provider` / `add_account` with their key, `mcp_server_get` with the token,
/// `mcp_server_rotate_token`, `set_external_setting`, `sync_providers` / `apply_providers`, which
/// may carry keys and have their own owner-press flow) and the pairing ceremonies whose tickets
/// are credentials and which need a person at the machine (`contact_window`, `contact_pair`,
/// `contact_answer`, `contact_nearby`, `mesh_nearby`, `mesh_nearby_watch`, `mesh_pair_nearby`,
/// `mesh_pair_answer`).
pub const SETTINGS_FORWARDABLE: &[&str] = &[
    "a2a_export_put", "a2a_exports", "a2a_status", "add_acp", "autoresearch.mechanism",
    "autoresearch.settings.get", "autoresearch.settings.set", "calendar_events",
    "calendar_settings_get", "calendar_settings_set", "calendar_source_add",
    "calendar_source_delete", "calendar_source_refresh", "calendar_sources", "computer_permissions",
    "config_flag_get", "config_flag_set", "contact_first_answer", "contact_first_waiting",
    "contact_list", "contact_remove", "contact_update", "decision_loaded", "decision_mode",
    "decision_stats", "decision_training", "delete_decision_training", "delete_tool_category",
    "desk_open", "dictation_audio_get", "dictation_history", "dictation_history_forget",
    "dictation_status", "dictionary_counts", "dictionary_delete", "dictionary_list",
    "dictionary_put", "dictionary_search", "documents_status", "endless_settings_get",
    "endless_settings_set", "engine_caps", "escalate_effort", "escalate_model", "get_always_ask",
    "get_approvals", "get_communication", "get_external_settings", "handoffs_list", "info",
    "knowledge_harvester_effort", "knowledge_harvester_model", "list_acp", "list_acp_registry",
    "list_aws_profiles", "list_builtins", "list_input_devices", "list_logins", "list_mcp",
    "list_models", "list_project_styles", "list_prompt_sections", "list_providers",
    "list_schedules", "list_secret_connections", "list_sessions", "list_tool_categories",
    "local_servers_probe", "logout", "mcp_server_audit", "mcp_server_set", "meeting_get",
    "meeting_list", "meeting_live_diarization", "meeting_settings_get", "meeting_settings_set",
    "meeting_speaker_delete", "meeting_speakers", "meeting_template_delete", "meeting_template_put",
    "meeting_template_reset", "meeting_templates", "mesh_rename", "mesh_set_hub", "mesh_set_nearby",
    "mesh_status", "plugin_approve", "plugin_get", "plugin_list", "plugin_logs", "plugin_new",
    "plugin_restart", "plugin_set_enabled", "plugin_set_hidden", "plugin_set_order",
    "plugin_uninstall", "polish_effort", "polish_model", "provider_models", "refresh_all_models",
    "refresh_provider_models", "reload_a2a", "remove_acp", "remove_provider",
    "reset_prompt_section", "reset_provider_models", "sandbox_create", "sandbox_destroy",
    "sandbox_install_runtime", "sandbox_pause", "sandbox_preflight", "sandbox_resize",
    "sandbox_restore", "sandbox_resume", "sandbox_retry", "sandbox_set_shares", "sandbox_snapshot",
    "sandbox_start", "sandbox_stop", "sandbox_terminal", "sandboxes_list", "screen_context",
    "search_provider_test", "search_settings_get", "search_settings_set", "search_status",
    "search_test", "send_input", "sessions_status", "set_advisor_default_effort",
    "set_advisor_default_model", "set_advisor_immune_turns", "set_advisor_max_consults",
    "set_advisor_notes_per_update", "set_advisor_sync_backlog", "set_always_ask", "set_approvals",
    "set_auto_archive", "set_aws_profile", "set_builtins_disabled", "set_communication",
    "set_compact_style", "set_decision_mode", "set_decision_share", "set_decision_training",
    "set_default_tool_categories", "set_effort", "set_end_phrases", "set_escalate_effort",
    "set_escalate_model", "set_hint_limit", "set_input_devices", "set_knowledge_harvester_effort",
    "set_knowledge_harvester_model", "set_max_salvages", "set_max_tool_rounds", "set_mcp_category",
    "set_mcp_hidden", "set_meeting_live_diarization", "set_model", "set_model_favorite",
    "set_model_limits", "set_peek_threshold", "set_polish_effort", "set_polish_enabled",
    "set_polish_model", "set_prompt_section", "set_prompt_section_enabled", "set_pty_idle",
    "set_schedule_run_timeout", "set_shape", "set_shape_enabled", "set_status_every",
    "set_stream_idle", "set_stt_engine", "set_subagent_effort", "set_subagent_model",
    "set_subagent_timeout", "set_theuth_agent_effort", "set_theuth_agent_model", "set_title_effort",
    "set_title_model", "set_tool_category", "set_vision_effort", "set_vision_model", "steer",
    "subagent_effort", "subagent_model", "test_summary_call", "theuth_agent_effort",
    "theuth_agent_model", "title_effort", "title_model", "update_status", "upsert_tool_category",
    "vault_rotate", "vault_rotation", "vision_effort", "vision_model", "voice_mlx_download",
    "voice_mlx_status", "voice_settings", "voice_status",
];

/// Whether `method` may run on another machine through [`MachinesApi::engine_call`].
pub fn forwardable(method: &str) -> bool {
    SETTINGS_FORWARDABLE.binary_search(&method).is_ok()
}

/// What the UI passes with a call an owner press started. Advisory only: authority is the
/// engine the app is connected to, which forwards the press only when its own
/// `human_press` check of the calling connection passes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PressToken {
    /// When the press happened (ms since epoch).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub at: u64,
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
    /// Needs the owner's press; this call had none.
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
    /// A display is reachable: macOS/iOS/Windows always; Linux a Wayland/X11 display or the
    /// node's watchable desktop (`desktop.json`).
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostCaps {
    /// A person drives this engine (not a plain `t --headless`).
    pub interactive: bool,
    /// A display is reachable.
    pub display: bool,
    /// An audio device is present.
    pub audio: bool,
}

static HOST: OnceLock<HostCaps> = OnceLock::new();

/// Record the startup probe; later calls are ignored.
pub fn install_host_caps(caps: HostCaps) {
    let _ = HOST.set(caps);
}

/// The installed probe; a host that installs none (the iOS app, tests) is a person's device.
fn host_caps() -> HostCaps {
    HOST.get().copied().unwrap_or(HostCaps { interactive: true, display: true, audio: true })
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
    /// `engine` (this engine: here, as the caller). Only [`SETTINGS_FORWARDABLE`] methods
    /// (else `-32050`). `pressed`: the call started with an owner press; carried to the target
    /// only when this engine sees the caller as a press. Errors: `-32051` offline, `-32052`
    /// vault sealed, `-32053` not applicable there, else the target's own. Owner only.
    #[method(name = "engine_call", with_extensions)]
    async fn engine_call(
        &self,
        engine: String,
        method: String,
        params: Value,
        pressed: Option<PressToken>,
    ) -> RpcResult<Value>;
    /// 227: [`engine_call`](MachinesApiServer::engine_call) on every engine of `engines`
    /// (default: this one and every paired engine), concurrently, 30 s each; one row per
    /// engine, never an error for one machine. Owner only.
    #[method(name = "engine_call_all", with_extensions)]
    async fn engine_call_all(
        &self,
        method: String,
        params: Value,
        engines: Option<Vec<String>>,
        pressed: Option<PressToken>,
    ) -> RpcResult<Vec<NodeResult>>;
    /// 227: what this engine's host offers (display, microphone, a person at it). Cheap.
    #[method(name = "engine_caps", with_extensions)]
    async fn engine_caps(&self) -> RpcResult<EngineCaps>;
    /// 227 ("differs on N"): each getter of `keys` (allowlisted, called without params) on
    /// each engine of `engines` (`[]`: this one and every paired engine): `{engine: {key:
    /// value | null}}`, `null` where the getter failed or does not apply; offline engines
    /// are omitted. Owner only.
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
/// [`forward::Forwarded`] / [`rpc_core::ForwardedPress`] marker) ride along.
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

/// Whether a call carries the owner's press across the hop: the UI said so AND this engine
/// sees its caller as a press. An agent (internal peer), a guest, or a forwarded call without
/// [`rpc_core::ForwardedPress`] never does, whatever `pressed` says.
pub fn press_for(ext: &Extensions, pressed: Option<&PressToken>) -> bool {
    pressed.is_some() && secrets::rpc::human_press(ext)
}

/// Allowlisted calls that refuse (or would park a human-only card) without the owner's press:
/// `plugin_approve` and an always-ask removal (`set_always_ask` with `on: false`). Refused up
/// front as [`NEEDS_PRESS`] so a queued retry never leaves a card waiting on the target.
pub fn needs_press(method: &str, params: &Value) -> bool {
    match method {
        "plugin_approve" => true,
        "set_always_ask" => {
            params.get("on").or_else(|| params.get(2)).and_then(Value::as_bool) == Some(false)
        }
        _ => false,
    }
}

/// One call: here when `engine` is this engine, else forwarded once. Errors are [`classify`]'d.
pub async fn call_one(
    reach: &Reach,
    ext: &Extensions,
    engine: &str,
    method: &str,
    params: &Value,
    press: bool,
) -> RpcResult<Value> {
    if !forwardable(method) {
        return Err(not_forwardable(method));
    }
    // Here the caller's own connection is the press; across the hop, the carried one.
    let pressing = if reach.is_me(engine) { secrets::rpc::human_press(ext) } else { press };
    if !pressing && needs_press(method, params) {
        return Err(error(NEEDS_PRESS, format!("{method} needs the owner's press")));
    }
    if reach.is_me(engine) {
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
        press,
    )
    .await;
    match sent {
        Ok(Some(v)) => Ok(v),
        Ok(None) => Err(error(OFFLINE, format!("{engine} is offline"))),
        Err(e) => Err(classify(e)),
    }
}

/// Whether `method`'s answer `value` means a restart is due: the reply's own
/// `restart_required` (`config_flag_set`, plugin flags), or a `subsystems.*` key written
/// through `config_flag_set` (read at start only, `theuth::subsystems`).
pub fn restart_required(method: &str, params: &Value, value: &Value) -> bool {
    if value.get("restart_required").and_then(Value::as_bool) == Some(true) {
        return true;
    }
    if method != "config_flag_set" {
        return false;
    }
    let key = params
        .get("key")
        .or_else(|| params.get(0))
        .and_then(Value::as_str)
        .unwrap_or_default();
    key.starts_with("subsystems.")
}

/// [`call_one`] on every `(engine, name)` of `targets`, concurrently, as rows.
pub async fn call_all(
    reach: &Reach,
    ext: &Extensions,
    targets: &[(String, String)],
    method: &str,
    params: &Value,
    press: bool,
) -> Vec<NodeResult> {
    let calls = targets.iter().map(|(engine, name)| async move {
        let r = call_one(reach, ext, engine, method, params, press).await;
        node_result(engine, name, method, params, r)
    });
    futures::future::join_all(calls).await
}

fn node_result(
    engine: &str,
    name: &str,
    method: &str,
    params: &Value,
    r: RpcResult<Value>,
) -> NodeResult {
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
        restart_required: value.as_ref().is_some_and(|v| restart_required(method, params, v)),
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
struct Roster {
    me: Vec<String>,
    name: String,
    peers: Vec<(String, String)>,
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
    /// `engines` with names, or this engine then every peer when `None`.
    fn targets(&self, engines: Option<Vec<String>>) -> Vec<(String, String)> {
        let me = || (self.me[0].clone(), self.name.clone());
        match engines {
            None => std::iter::once(me()).chain(self.peers.iter().cloned()).collect(),
            Some(ids) => ids
                .into_iter()
                .map(|id| {
                    if self.me.contains(&id) {
                        return (id, self.name.clone());
                    }
                    let name = self
                        .peers
                        .iter()
                        .find(|(p, _)| *p == id)
                        .map_or_else(|| id.clone(), |(_, n)| n.clone());
                    (id, name)
                })
                .collect(),
        }
    }
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
        pressed: Option<PressToken>,
    ) -> RpcResult<Value> {
        rpc_core::owner(ext)?;
        if !forwardable(&method) {
            return Err(not_forwardable(&method));
        }
        let roster = roster().await?;
        let reach = reach(&self.rt, roster.me).await;
        call_one(&reach, ext, &engine, &method, &params, press_for(ext, pressed.as_ref())).await
    }

    async fn engine_call_all(
        &self,
        ext: &Extensions,
        method: String,
        params: Value,
        engines: Option<Vec<String>>,
        pressed: Option<PressToken>,
    ) -> RpcResult<Vec<NodeResult>> {
        rpc_core::owner(ext)?;
        if !forwardable(&method) {
            return Err(not_forwardable(&method));
        }
        let roster = roster().await?;
        let targets = roster.targets(engines);
        let reach = reach(&self.rt, roster.me).await;
        let press = press_for(ext, pressed.as_ref());
        Ok(call_all(&reach, ext, &targets, &method, &params, press).await)
    }

    async fn engine_caps(&self, ext: &Extensions) -> RpcResult<EngineCaps> {
        rpc_core::owner(ext)?;
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
        rpc_core::owner(ext)?;
        if let Some(k) = keys.iter().find(|k| !forwardable(k)) {
            return Err(not_forwardable(k));
        }
        let roster = roster().await?;
        let targets = roster.targets((!engines.is_empty()).then_some(engines));
        let reach = reach(&self.rt, roster.me).await;
        let rows = futures::future::join_all(
            keys.iter().map(|k| call_all(&reach, ext, &targets, k, &Value::Null, false)),
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
