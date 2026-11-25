//! Owner: B2 — the `PluginsApi` RPC methods (contract §6). Each method admits the caller
//! (`owner`, or the human press for `plugin_approve`), looks the actor up (a missing actor —
//! plugins disabled, Windows — is `NOT_RUNNING`), and sends it one §5.2 message.
// `plugin_action` has seven parameters (contract §6): the generated server and client methods
// (plus `&self` and `ext`) exceed clippy's limit, and the macro output takes no item attribute.
#![allow(clippy::too_many_arguments)]

use std::path::PathBuf;

use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use jsonrpsee::types::ErrorObjectOwned;
use kameo::actor::ActorRef;
use kameo::message::Message;
use runtime::SessionId;
use rpc_core::Server;

use crate::PLUGINS_ACTOR;
use crate::actor::PluginsActor;
use crate::error::PluginsError;
use crate::messages::*;
use crate::press::HumanPress;
use crate::types::*;

/// Default and cap of `plugin_logs`' `tail`.
const LOG_TAIL_DEFAULT: u32 = 200;
const LOG_TAIL_MAX: u32 = 2000;

/// The JSON-RPC error of `e`: its [`PluginsError::code`] and message; `RateLimited` carries
/// `{"retry_after_ms": n}` and `Plugin` `{"code": n}` as data (contract §9.1).
fn rpc_err(e: PluginsError) -> ErrorObjectOwned {
    let data = match &e {
        PluginsError::RateLimited { retry_after_ms } => Some(serde_json::json!({ "retry_after_ms": retry_after_ms })),
        PluginsError::Plugin { code, .. } => Some(serde_json::json!({ "code": code })),
        _ => None,
    };
    ErrorObjectOwned::owned(e.code(), e.to_string(), data)
}

/// The registered host actor; `NOT_RUNNING` ("plugins not running") when there is none.
fn actor() -> RpcResult<ActorRef<PluginsActor>> {
    rpc_core::lookup::<PluginsActor>(PLUGINS_ACTOR)
}

/// One `ask` whose reply resolves to `Result<R, PluginsError>` (directly or delegated); a
/// refusal becomes [`rpc_err`].
async fn ask<M, R>(msg: M) -> RpcResult<R>
where
    PluginsActor: Message<M>,
    <PluginsActor as Message<M>>::Reply: kameo::Reply<Ok = R, Error = PluginsError>,
    M: Send + 'static,
    R: Send + 'static,
{
    match actor()?.ask(msg).await {
        Ok(r) => Ok(r),
        Err(kameo::error::SendError::HandlerError(e)) => Err(rpc_err(e)),
        Err(e) => Err(rpc_core::failed(format!("plugins actor: {e}"))),
    }
}

/// `x-theuth-client` of the call, `desktop` when absent (contract §6).
fn client(ext: &Extensions) -> String {
    ext.get::<rpc_core::Client>().map(|c| c.0).unwrap_or("desktop").to_owned()
}

/// A folder argument: absolute, or `Invalid`.
fn abs_path(path: String) -> RpcResult<PathBuf> {
    let p = PathBuf::from(&path);
    if p.is_absolute() {
        Ok(p)
    } else {
        Err(rpc_err(PluginsError::Invalid {
            message: format!("path must be absolute: {path}"),
        }))
    }
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
        rpc_core::send(&actor()?, ListPlugins).await
    }

    async fn plugin_get(&self, ext: &Extensions, name: String) -> RpcResult<PluginDetail> {
        rpc_core::owner(ext)?;
        ask(GetPlugin { name }).await
    }

    async fn plugin_check(&self, ext: &Extensions, path: String) -> RpcResult<CheckReport> {
        rpc_core::owner(ext)?;
        ask(CheckPath { path: abs_path(path)? }).await
    }

    async fn plugin_install(&self, ext: &Extensions, path: String) -> RpcResult<InstallReport> {
        rpc_core::owner(ext)?;
        ask(Install { path: abs_path(path)? }).await
    }

    async fn plugin_new(
        &self,
        ext: &Extensions,
        name: String,
        lang: Option<Lang>,
        raw: Option<bool>,
    ) -> RpcResult<ScaffoldReport> {
        rpc_core::owner(ext)?;
        ask(New {
            name,
            lang: lang.unwrap_or_default(),
            raw: raw.unwrap_or(false),
        })
        .await
    }

    async fn plugin_approve(
        &self,
        ext: &Extensions,
        name: String,
        manifest_sha256: String,
        code_sha256: Option<String>,
    ) -> RpcResult<PluginInfo> {
        // The press is the only admission (216 D14): nothing else builds a `HumanPress`.
        let press = HumanPress::from_rpc(ext).map_err(rpc_err)?;
        ask(Approve {
            name,
            manifest_sha256,
            code_sha256,
            press,
        })
        .await
    }

    async fn plugin_set_enabled(&self, ext: &Extensions, name: String, on: bool) -> RpcResult<PluginInfo> {
        rpc_core::owner(ext)?;
        ask(SetEnabled { name, on }).await
    }

    async fn plugin_set_hidden(
        &self,
        ext: &Extensions,
        name: String,
        placement: String,
        hidden: bool,
    ) -> RpcResult<PluginInfo> {
        rpc_core::owner(ext)?;
        ask(SetHidden { name, placement, hidden }).await
    }

    async fn plugin_set_order(&self, ext: &Extensions, order: Vec<String>) -> RpcResult<Vec<PluginInfo>> {
        rpc_core::owner(ext)?;
        ask(SetOrder { order }).await
    }

    async fn plugin_restart(&self, ext: &Extensions, name: String) -> RpcResult<PluginInfo> {
        rpc_core::owner(ext)?;
        ask(Restart { name }).await
    }

    async fn plugin_uninstall(&self, ext: &Extensions, name: String, keep_data: Option<bool>) -> RpcResult<()> {
        rpc_core::owner(ext)?;
        ask(Uninstall {
            name,
            keep_data: keep_data.unwrap_or(false),
        })
        .await
    }

    async fn plugin_action(
        &self,
        ext: &Extensions,
        name: String,
        placement: String,
        action: String,
        args: serde_json::Value,
        session: Option<SessionId>,
        activation: Option<String>,
        form: Option<serde_json::Value>,
    ) -> RpcResult<serde_json::Value> {
        let peer = rpc_core::owner(ext)?;
        // Only a real client press carries an activation (as `app_call_tool`).
        if activation.is_some() && peer.is_internal() {
            return Err(rpc_err(PluginsError::NotAPress));
        }
        ask(Action {
            name,
            placement,
            action,
            args,
            session,
            client: client(ext),
            activation,
            form,
        })
        .await
    }

    async fn plugin_query(
        &self,
        ext: &Extensions,
        name: String,
        placement: String,
        query: String,
        args: serde_json::Value,
        session: Option<SessionId>,
    ) -> RpcResult<serde_json::Value> {
        rpc_core::owner(ext)?;
        ask(Query {
            name,
            placement,
            query,
            args,
            session,
            client: client(ext),
        })
        .await
    }

    async fn plugin_command(
        &self,
        ext: &Extensions,
        name: String,
        command: String,
        args: String,
        session: Option<SessionId>,
    ) -> RpcResult<CommandReply> {
        rpc_core::owner(ext)?;
        ask(Command {
            name,
            command,
            args,
            session,
            client: client(ext),
        })
        .await
    }

    async fn plugin_surfaces(&self, ext: &Extensions) -> RpcResult<Vec<SurfaceSnapshot>> {
        rpc_core::owner(ext)?;
        rpc_core::send(&actor()?, Surfaces).await
    }

    async fn plugin_surface(
        &self,
        ext: &Extensions,
        name: String,
        placement: String,
        session: Option<SessionId>,
    ) -> RpcResult<SurfaceContent> {
        rpc_core::owner(ext)?;
        ask(Surface {
            name,
            placement,
            session,
            client: client(ext),
        })
        .await
    }

    async fn plugin_catalog(&self, ext: &Extensions, topic: Option<String>) -> RpcResult<Vec<CatalogEntry>> {
        rpc_core::owner(ext)?;
        rpc_core::send(&actor()?, Catalog { topic }).await
    }

    async fn plugin_logs(&self, ext: &Extensions, name: String, tail: Option<u32>) -> RpcResult<LogTail> {
        rpc_core::owner(ext)?;
        ask(Logs {
            name,
            tail: tail.unwrap_or(LOG_TAIL_DEFAULT).min(LOG_TAIL_MAX),
        })
        .await
    }
}

/// This plugin's RPC methods.
pub fn rpc_module(server: Server) -> rpc_core::DescribedModule {
    rpc_core::DescribedModule::new(PluginsApiServer::into_rpc(server), rpc_descriptors())
        .expect("plugins RPC descriptors")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> Server {
        Server::new(runtime::Runtime::new(Default::default(), std::env::temp_dir()))
    }

    fn internal() -> Extensions {
        let mut ext = Extensions::new();
        ext.insert(rpc_core::RpcPeer::internal());
        ext
    }

    #[tokio::test]
    async fn approve_over_internal_peer_is_32002() {
        let e = server()
            .plugin_approve(&internal(), "turns".into(), "m".repeat(64), None)
            .await
            .unwrap_err();
        assert_eq!(e.code(), -32002);
        assert_eq!(e.message(), PluginsError::NotAPress.to_string());
        // No connection at all is refused the same way.
        let e = server()
            .plugin_approve(&Extensions::new(), "turns".into(), "m".repeat(64), None)
            .await
            .unwrap_err();
        assert_eq!(e.code(), -32002);
    }

    #[tokio::test]
    async fn action_with_activation_from_internal_peer_refused() {
        let s = server();
        let e = s
            .plugin_action(&internal(), "turns".into(), "turns".into(), "reset".into(), serde_json::json!({}), None, Some("a1".into()), None)
            .await
            .unwrap_err();
        assert_eq!(e.code(), -32002);
        // Without an activation the internal peer passes admission (whatever the actor answers).
        let r = s
            .plugin_action(&internal(), "turns".into(), "turns".into(), "reset".into(), serde_json::json!({}), None, None, None)
            .await;
        assert!(r.as_ref().map_or_else(|e| e.code() != -32002, |_| true), "{r:?}");
    }

    #[test]
    fn errors_carry_data() {
        let e = rpc_err(PluginsError::RateLimited { retry_after_ms: 250 });
        assert_eq!(e.code(), -32005);
        assert_eq!(e.data().unwrap().get(), r#"{"retry_after_ms":250}"#);
        let e = rpc_err(PluginsError::Plugin { code: 7, message: "boom".into() });
        assert_eq!((e.code(), e.message()), (-32000, "boom"));
        assert_eq!(e.data().unwrap().get(), r#"{"code":7}"#);
        assert!(rpc_err(PluginsError::NotFound { name: "x".into() }).data().is_none());
    }

    #[test]
    fn relative_paths_refused() {
        assert_eq!(abs_path("plugins/x".into()).unwrap_err().code(), -32602);
        assert!(abs_path("/tmp/x".into()).is_ok());
    }
}
