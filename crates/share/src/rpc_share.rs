//! The share RPC surface (210 §7.2 table). Method names follow the repo's snake convention
//! (`share_list` for the design's `share.list`).

use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use rpc_core::{Server, failed, owner};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::config::EdgeChoice;
use crate::engine::{CallOrigin, Published};
use crate::model::Share;
use crate::rpc_support::engine;
use crate::tools::{PublishArgs, plan};

/// `share_settings_get` answer.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ShareSettings {
    /// "Allow public sharing" on this machine.
    pub enabled: bool,
    /// `default`, `none` or a ticket.
    pub edge: String,
}

/// `share_stop` answer.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Stopped {
    /// Shares stopped.
    pub stopped: u32,
}

/// `share_names_check` answer: the edge's `GET /v1/names/{name}`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct NameCheck {
    /// Free to claim.
    pub available: bool,
    /// Why not.
    pub reason: Option<String>,
}

/// Share RPC methods owned by this plugin.
#[rpc_core_macros::rpc_descriptors(owner = "share", descriptors = rpc_descriptors, audience = "owner")]
#[rpc(server, client)]
pub trait ShareApi {
    /// Every share of the mesh (30-day history included).
    #[method(name = "share_list")]
    async fn share_list(&self) -> RpcResult<Vec<Share>>;

    /// Stop a share or `all` (§7.4). Never gated.
    #[method(name = "share_stop")]
    async fn share_stop(&self, id: String) -> RpcResult<Stopped>;

    /// Publish from Settings or the card; the owner-only approval row precedes it.
    #[method(name = "share_publish", with_extensions)]
    async fn share_publish(&self, args: PublishArgs) -> RpcResult<Published>;

    /// The master switch and edge.
    #[method(name = "share_settings_get")]
    async fn share_settings_get(&self) -> RpcResult<ShareSettings>;

    /// Set the switch and/or the edge. On and edge changes need the approval row (§7.2).
    #[method(name = "share_settings_set", with_extensions)]
    async fn share_settings_set(&self, enabled: Option<bool>, edge: Option<String>) -> RpcResult<ShareSettings>;

    /// `GET /v1/names/{name}` at the edge.
    #[method(name = "share_names_check")]
    async fn share_names_check(&self, name: String) -> RpcResult<NameCheck>;

    /// Accept an edge's terms revision (first-run card).
    #[method(name = "share_terms_accept", with_extensions)]
    async fn share_terms_accept(&self, edge: String, terms_rev: String) -> RpcResult<()>;
}

// TODO(210-M1, crate tools): `share_publish`, `share_settings_set {enabled: true | edge}` and
// `share_terms_accept` resolve the owner-only approval row before acting; today they check only
// that the caller is the owner. TODO(210-M2): `share_extend`, `share_set_access`,
// `share_names_reserve|release|primary|rotate|transfer`, `share_delegate`, `share_name_key_ensure`.
#[async_trait]
impl ShareApiServer for Server {
    async fn share_list(&self) -> RpcResult<Vec<Share>> {
        engine()?.list().await.map_err(|e| failed(format!("{e:#}")))
    }

    async fn share_stop(&self, id: String) -> RpcResult<Stopped> {
        let e = engine()?;
        let n = if id == "all" {
            e.stop_all().await
        } else {
            e.stop(&id, crate::model::StoppedBy::User).await.map(|_| 1)
        };
        n.map(|n| Stopped { stopped: n as u32 })
            .map_err(|e| failed(format!("{e:#}")))
    }

    async fn share_publish(&self, ext: &Extensions, args: PublishArgs) -> RpcResult<Published> {
        owner(ext)?;
        let e = engine()?;
        let p = plan(args, e.cfg.default_expiry, crate::engine::random_bytes()).map_err(failed)?;
        e.publish(p, CallOrigin::Settings)
            .await
            .map_err(|e| failed(format!("{e:#}")))
    }

    async fn share_settings_get(&self) -> RpcResult<ShareSettings> {
        let e = engine()?;
        let enabled = e.store.enabled().await.map_err(|e| failed(e.to_string()))?;
        let edge = e.store.edge().await.map_err(|e| failed(e.to_string()))?;
        Ok(ShareSettings {
            enabled,
            edge: edge.to_setting(),
        })
    }

    async fn share_settings_set(
        &self,
        ext: &Extensions,
        enabled: Option<bool>,
        edge: Option<String>,
    ) -> RpcResult<ShareSettings> {
        owner(ext)?;
        let e = engine()?;
        if let Some(edge) = edge {
            let choice = EdgeChoice::parse(&edge).map_err(|e| failed(e.to_string()))?;
            e.set_edge(&choice).await.map_err(|e| failed(e.to_string()))?;
        }
        if let Some(on) = enabled {
            e.set_enabled(on).await.map_err(|e| failed(e.to_string()))?;
        }
        self.share_settings_get().await
    }

    async fn share_names_check(&self, name: String) -> RpcResult<NameCheck> {
        engine()?
            .check_name(&name)
            .await
            .map(|a| NameCheck {
                available: a.available,
                reason: a.reason,
            })
            .map_err(|e| failed(format!("{e:#}")))
    }

    async fn share_terms_accept(&self, ext: &Extensions, edge: String, terms_rev: String) -> RpcResult<()> {
        owner(ext)?;
        engine()?
            .accept_terms(&edge, &terms_rev)
            .await
            .map_err(|e| failed(e.to_string()))
    }
}

pub(super) fn rpc_module(server: Server) -> rpc_core::DescribedModule {
    rpc_core::DescribedModule::new(ShareApiServer::into_rpc(server), rpc_descriptors())
        .expect("share RPC descriptors")
}
