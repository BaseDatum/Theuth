//! Owner: B2 — the `PluginsApi` RPC methods (contract §6). B0 stubs: admission (`owner`, or the
//! human press for `plugin_approve`), then `NotImplemented`.
// `plugin_action` has seven parameters (contract §6): the generated server and client methods
// (plus `&self` and `ext`) exceed clippy's limit, and the macro output takes no item attribute.
#![allow(clippy::too_many_arguments)]

use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use jsonrpsee::types::ErrorObjectOwned;
use runtime::SessionId;
use rpc_core::Server;

use crate::error::PluginsError;
use crate::press::HumanPress;
use crate::types::*;

/// The JSON-RPC error of `e`: its [`PluginsError::code`] and message.
fn rpc_err(e: PluginsError) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(e.code(), e.to_string(), None::<()>)
}

fn not_implemented<T>(method: &str) -> RpcResult<T> {
    Err(rpc_err(PluginsError::not_implemented(method)))
}

#[rpc_core_macros::rpc_descriptors(owner = "plugins", descriptors = rpc_descriptors, audience = "owner")]
#[rpc(server, client)]
pub trait PluginsApi {
    /// Every plugin with its state, placements and pending card.
    #[method(name = "plugin_list", with_extensions)]
    async fn plugin_list(&self) -> RpcResult<Vec<PluginInfo>>;
    /// One plugin: manifest, requested and granted powers, pending card, stderr tail.
    #[method(name = "plugin_get", with_extensions)]
    async fn plugin_get(&self, name: String) -> RpcResult<PluginDetail>;
    /// Validate a plugin folder (absolute path) without starting it.
    #[method(name = "plugin_check", with_extensions)]
    async fn plugin_check(&self, path: String) -> RpcResult<CheckReport>;
    /// Copy a local plugin folder into plugins/ and file its install card. Runs nothing.
    #[method(name = "plugin_install", with_extensions)]
    async fn plugin_install(&self, path: String) -> RpcResult<InstallReport>;
    /// Scaffold a new plugin from the bundled SDK template (default `py`; `raw`: no SDK). Runs nothing.
    #[method(name = "plugin_new", with_extensions)]
    async fn plugin_new(&self, name: String, lang: Option<Lang>, raw: Option<bool>) -> RpcResult<ScaffoldReport>;
    /// Approve the pending card with exactly these hashes. Human press only (216 D14).
    #[method(name = "plugin_approve", with_extensions)]
    async fn plugin_approve(&self, name: String, manifest_sha256: String, code_sha256: Option<String>) -> RpcResult<PluginInfo>;
    /// Enable (start, if a verified grant covers it) or disable (stop) a plugin.
    #[method(name = "plugin_set_enabled", with_extensions)]
    async fn plugin_set_enabled(&self, name: String, on: bool) -> RpcResult<PluginInfo>;
    /// Hide or show one placement on every client.
    #[method(name = "plugin_set_hidden", with_extensions)]
    async fn plugin_set_hidden(&self, name: String, placement: String, hidden: bool) -> RpcResult<PluginInfo>;
    /// Reorder sidebar and status-strip items (names, first = top).
    #[method(name = "plugin_set_order", with_extensions)]
    async fn plugin_set_order(&self, order: Vec<String>) -> RpcResult<Vec<PluginInfo>>;
    /// Restart a plugin (clears a crash loop).
    #[method(name = "plugin_restart", with_extensions)]
    async fn plugin_restart(&self, name: String) -> RpcResult<PluginInfo>;
    /// Stop a plugin, drop its grant, move its folder (and, unless keep_data, its data) to plugins/.trash.
    #[method(name = "plugin_uninstall", with_extensions)]
    async fn plugin_uninstall(&self, name: String, keep_data: Option<bool>) -> RpcResult<()>;
    /// A press on a placement (OpenUI Mutation): delivered to the plugin as `action`.
    #[method(name = "plugin_action", with_extensions)]
    async fn plugin_action(&self, name: String, placement: String, action: String, args: serde_json::Value, session: Option<SessionId>, activation: Option<String>, form: Option<serde_json::Value>) -> RpcResult<serde_json::Value>;
    /// An OpenUI Query on a placement: delivered as `query`; rate-limited per plugin.
    #[method(name = "plugin_query", with_extensions)]
    async fn plugin_query(&self, name: String, placement: String, query: String, args: serde_json::Value, session: Option<SessionId>) -> RpcResult<serde_json::Value>;
    /// A `/plugin[:cmd]` command typed in a composer: delivered as `command`.
    #[method(name = "plugin_command", with_extensions)]
    async fn plugin_command(&self, name: String, command: String, args: String, session: Option<SessionId>) -> RpcResult<CommandReply>;
    /// Latest status of every placement (snapshot before following `plugins.ui`).
    #[method(name = "plugin_surfaces", with_extensions)]
    async fn plugin_surfaces(&self) -> RpcResult<Vec<SurfaceSnapshot>>;
    /// One placement's content; asks the plugin to render when none is held.
    #[method(name = "plugin_surface", with_extensions)]
    async fn plugin_surface(&self, name: String, placement: String, session: Option<SessionId>) -> RpcResult<SurfaceContent>;
    /// The event catalog plugins can subscribe to (optionally one topic).
    #[method(name = "plugin_catalog", with_extensions)]
    async fn plugin_catalog(&self, topic: Option<String>) -> RpcResult<Vec<CatalogEntry>>;
    /// The last `tail` lines (default 200, max 2000) of a plugin's log today.
    #[method(name = "plugin_logs", with_extensions)]
    async fn plugin_logs(&self, name: String, tail: Option<u32>) -> RpcResult<LogTail>;
}

#[async_trait]
impl PluginsApiServer for Server {
    async fn plugin_list(&self, ext: &Extensions) -> RpcResult<Vec<PluginInfo>> {
        rpc_core::owner(ext)?;
        not_implemented("plugin_list")
    }

    async fn plugin_get(&self, ext: &Extensions, _name: String) -> RpcResult<PluginDetail> {
        rpc_core::owner(ext)?;
        not_implemented("plugin_get")
    }

    async fn plugin_check(&self, ext: &Extensions, _path: String) -> RpcResult<CheckReport> {
        rpc_core::owner(ext)?;
        not_implemented("plugin_check")
    }

    async fn plugin_install(&self, ext: &Extensions, _path: String) -> RpcResult<InstallReport> {
        rpc_core::owner(ext)?;
        not_implemented("plugin_install")
    }

    async fn plugin_new(
        &self,
        ext: &Extensions,
        _name: String,
        _lang: Option<Lang>,
        _raw: Option<bool>,
    ) -> RpcResult<ScaffoldReport> {
        rpc_core::owner(ext)?;
        not_implemented("plugin_new")
    }

    async fn plugin_approve(
        &self,
        ext: &Extensions,
        _name: String,
        _manifest_sha256: String,
        _code_sha256: Option<String>,
    ) -> RpcResult<PluginInfo> {
        HumanPress::from_rpc(ext).map_err(rpc_err)?;
        not_implemented("plugin_approve")
    }

    async fn plugin_set_enabled(&self, ext: &Extensions, _name: String, _on: bool) -> RpcResult<PluginInfo> {
        rpc_core::owner(ext)?;
        not_implemented("plugin_set_enabled")
    }

    async fn plugin_set_hidden(
        &self,
        ext: &Extensions,
        _name: String,
        _placement: String,
        _hidden: bool,
    ) -> RpcResult<PluginInfo> {
        rpc_core::owner(ext)?;
        not_implemented("plugin_set_hidden")
    }

    async fn plugin_set_order(&self, ext: &Extensions, _order: Vec<String>) -> RpcResult<Vec<PluginInfo>> {
        rpc_core::owner(ext)?;
        not_implemented("plugin_set_order")
    }

    async fn plugin_restart(&self, ext: &Extensions, _name: String) -> RpcResult<PluginInfo> {
        rpc_core::owner(ext)?;
        not_implemented("plugin_restart")
    }

    async fn plugin_uninstall(&self, ext: &Extensions, _name: String, _keep_data: Option<bool>) -> RpcResult<()> {
        rpc_core::owner(ext)?;
        not_implemented("plugin_uninstall")
    }

    async fn plugin_action(
        &self,
        ext: &Extensions,
        _name: String,
        _placement: String,
        _action: String,
        _args: serde_json::Value,
        _session: Option<SessionId>,
        _activation: Option<String>,
        _form: Option<serde_json::Value>,
    ) -> RpcResult<serde_json::Value> {
        rpc_core::owner(ext)?;
        not_implemented("plugin_action")
    }

    async fn plugin_query(
        &self,
        ext: &Extensions,
        _name: String,
        _placement: String,
        _query: String,
        _args: serde_json::Value,
        _session: Option<SessionId>,
    ) -> RpcResult<serde_json::Value> {
        rpc_core::owner(ext)?;
        not_implemented("plugin_query")
    }

    async fn plugin_command(
        &self,
        ext: &Extensions,
        _name: String,
        _command: String,
        _args: String,
        _session: Option<SessionId>,
    ) -> RpcResult<CommandReply> {
        rpc_core::owner(ext)?;
        not_implemented("plugin_command")
    }

    async fn plugin_surfaces(&self, ext: &Extensions) -> RpcResult<Vec<SurfaceSnapshot>> {
        rpc_core::owner(ext)?;
        not_implemented("plugin_surfaces")
    }

    async fn plugin_surface(
        &self,
        ext: &Extensions,
        _name: String,
        _placement: String,
        _session: Option<SessionId>,
    ) -> RpcResult<SurfaceContent> {
        rpc_core::owner(ext)?;
        not_implemented("plugin_surface")
    }

    async fn plugin_catalog(&self, ext: &Extensions, _topic: Option<String>) -> RpcResult<Vec<CatalogEntry>> {
        rpc_core::owner(ext)?;
        not_implemented("plugin_catalog")
    }

    async fn plugin_logs(&self, ext: &Extensions, _name: String, _tail: Option<u32>) -> RpcResult<LogTail> {
        rpc_core::owner(ext)?;
        not_implemented("plugin_logs")
    }
}

/// This plugin's RPC methods.
pub fn rpc_module(server: Server) -> rpc_core::DescribedModule {
    rpc_core::DescribedModule::new(PluginsApiServer::into_rpc(server), rpc_descriptors())
        .expect("plugins RPC descriptors")
}
