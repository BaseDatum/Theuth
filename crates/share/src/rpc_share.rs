//! The share RPC surface (210 §7.2 table). Method names follow the repo's snake convention
//! (`share_list` for the design's `share.list`).

use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use rpc_core::{Server, failed, owner};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::config::EdgeChoice;
use crate::engine::{CallOrigin, Consent, Published};
use crate::model::Share;
use crate::names::{self, HeldName};
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
    /// The choice has a ticket to dial. `false` for `none` and for `default` while the shipped
    /// tickets are placeholders: Settings shows [`crate::config::NO_SHARING_EDGE`] and publishing
    /// is refused with it.
    pub configured: bool,
    /// The mesh's share identity (the public half of `mesh/share_key`): what another mesh pastes
    /// into `share_names_transfer`. `None` until the key exists.
    #[serde(default)]
    pub mesh_id: Option<String>,
}

/// `share_stop` answer.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Stopped {
    /// Shares stopped.
    pub stopped: u32,
    /// Stop all only: why the edge has not confirmed the mesh-wide stop yet (detached, or the
    /// edge refused); it is sent again at the next attach. `None` = done.
    #[serde(default)]
    pub edge_pending: Option<String>,
}

/// `share_names_check` answer: the edge's `GET /v1/names/{name}`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct NameCheck {
    /// Free to claim.
    pub available: bool,
    /// Why not.
    pub reason: Option<String>,
    /// With reason `cooldown`: when the name is free again.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub reclaim_after_ms: Option<u64>,
}

/// `share_names_release` answer.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct NameReleasedInfo {
    /// The released name.
    pub name: String,
    /// Until then only this mesh may claim it again.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub reclaim_after_ms: u64,
}

fn err(e: anyhow::Error) -> jsonrpsee::types::ErrorObjectOwned {
    failed(format!("{e:#}"))
}

/// Share RPC methods owned by this plugin.
#[rpc_core_macros::rpc_descriptors(owner = "share", descriptors = rpc_descriptors, audience = "owner")]
#[rpc(server, client)]
pub trait ShareApi {
    /// Every share of the mesh (30-day history included).
    #[method(name = "share_list")]
    async fn share_list(&self) -> RpcResult<Vec<Share>>;

    /// Stop a share or `all` (§7.4). Never gated.
    #[method(name = "share_stop", with_extensions)]
    async fn share_stop(&self, id: String) -> RpcResult<Stopped>;

    /// Publish from Settings or the card; the owner-only approval row precedes it.
    #[method(name = "share_publish", with_extensions)]
    async fn share_publish(&self, args: PublishArgs) -> RpcResult<Published>;

    /// The master switch and edge.
    #[method(name = "share_settings_get")]
    async fn share_settings_get(&self) -> RpcResult<ShareSettings>;

    /// Set the switch and/or the edge. On and edge changes need the approval row (§7.2).
    #[method(name = "share_settings_set", with_extensions)]
    async fn share_settings_set(
        &self,
        enabled: Option<bool>,
        edge: Option<String>,
    ) -> RpcResult<ShareSettings>;

    /// `GET /v1/names/{name}` at the edge.
    #[method(name = "share_names_check")]
    async fn share_names_check(&self, name: String) -> RpcResult<NameCheck>;

    /// Accept an edge's terms revision (first-run card).
    #[method(name = "share_terms_accept", with_extensions)]
    async fn share_terms_accept(&self, edge: String, terms_rev: String) -> RpcResult<()>;

    /// The mesh's names: the replicated `share_names` rows (released ones included) joined
    /// with the edge's view from the last `welcome`.
    #[method(name = "share_names_list")]
    async fn share_names_list(&self) -> RpcResult<Vec<HeldName>>;

    /// Claim a reserved name (`PUT /v1/names/{name}`) after the owner's approval; the first
    /// reserved name becomes primary (§5.2).
    #[method(name = "share_names_reserve", with_extensions)]
    async fn share_names_reserve(&self, name: String) -> RpcResult<HeldName>;

    /// Stop the name's shares on every machine, then release it at the edge.
    #[method(name = "share_names_release", with_extensions)]
    async fn share_names_release(&self, name: String) -> RpcResult<NameReleasedInfo>;

    /// Make a held name the one new shares use.
    #[method(name = "share_names_primary", with_extensions)]
    async fn share_names_primary(&self, name: String) -> RpcResult<HeldName>;

    /// Rotate the name key (lease holder; forwarded to it from other machines). Answers the new
    /// generation.
    #[method(name = "share_names_rotate", with_extensions)]
    async fn share_names_rotate(&self, name: String) -> RpcResult<u32>;

    /// Offer a name to another mesh for 7 days (D20) after the owner's approval. Answers
    /// `transfer_until_ms`.
    #[method(name = "share_names_transfer", with_extensions)]
    async fn share_names_transfer(&self, name: String, to_mesh: String) -> RpcResult<u64>;

    /// Family-internal (§5.7): the lease holder signs a delegation for a live engine,
    /// `issued_ms = min(holder clock, edge_now_ms)`. Answers `share_proto::Signed` as JSON.
    #[method(name = "share_delegate", with_extensions)]
    async fn share_delegate(
        &self,
        engine: String,
        edge: String,
        domain: String,
        edge_now_ms: u64,
    ) -> RpcResult<serde_json::Value>;

    /// Family-internal (§6.2): the lease holder creates the name key if missing, settles and
    /// answers the settled row's SPKI hash (lowercase hex).
    #[method(name = "share_name_key_ensure", with_extensions)]
    async fn share_name_key_ensure(&self, domain: String, name: String) -> RpcResult<String>;

    /// The password generated for a password share in this run of the engine, for the owner's
    /// surfaces (never the model). `None` after a restart or for a public share.
    #[method(name = "share_password", with_extensions)]
    async fn share_password(&self, id: String) -> RpcResult<Option<String>>;
}

// §7.2: `share_publish`, `share_settings_set {enabled: true | edge}` and `share_terms_accept`
// raise a human-only approval row ([`Consent::ask`]) and act only on the owner's `Allow`;
// stopping and switching off are never gated. M2: `share_names_reserve` and
// `share_names_transfer` ask the same way. TODO(210-M2): `share_extend`, `share_set_access`.
#[async_trait]
impl ShareApiServer for Server {
    async fn share_list(&self) -> RpcResult<Vec<Share>> {
        engine()?.list().await.map_err(|e| failed(format!("{e:#}")))
    }

    async fn share_stop(&self, ext: &Extensions, id: String) -> RpcResult<Stopped> {
        let e = engine()?;
        if id == "all" {
            // Asked by another machine of the family (`theuth/rpc/1`): stop here only.
            let forwarded = rpc_core::peer(ext).is_ok_and(|p| p.is_internal());
            let outcome = if forwarded {
                e.stop_all_forwarded().await
            } else {
                e.stop_all().await
            };
            return outcome
                .map(|o| Stopped {
                    stopped: o.stopped as u32,
                    edge_pending: o.edge_pending,
                })
                .map_err(|e| failed(format!("{e:#}")));
        }
        e.stop(&id, crate::model::StoppedBy::User)
            .await
            .map(|_| Stopped {
                stopped: 1,
                edge_pending: None,
            })
            .map_err(|e| failed(format!("{e:#}")))
    }

    async fn share_publish(&self, ext: &Extensions, args: PublishArgs) -> RpcResult<Published> {
        owner(ext)?;
        let e = engine()?;
        let card = serde_json::to_value(&args).map_err(|e| failed(e.to_string()))?;
        let p = plan(args, e.cfg.default_expiry, crate::engine::random_bytes()).map_err(failed)?;
        let consent = Consent::ask("share_publish", card)
            .await
            .map_err(|e| failed(format!("{e:#}")))?;
        e.publish(p, CallOrigin::Settings, consent)
            .await
            .map_err(|e| failed(format!("{e:#}")))
    }

    async fn share_settings_get(&self) -> RpcResult<ShareSettings> {
        let e = engine()?;
        let enabled = e.store.enabled().await.map_err(|e| failed(e.to_string()))?;
        let edge = e.store.edge().await.map_err(|e| failed(e.to_string()))?;
        Ok(ShareSettings {
            enabled,
            configured: !edge.tickets().is_empty(),
            edge: edge.to_setting(),
            mesh_id: crate::wiring::mesh_id(),
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
        let choice = edge
            .as_deref()
            .map(EdgeChoice::parse)
            .transpose()
            .map_err(|e| failed(e.to_string()))?;
        // Off is never gated and applies first, before any ask.
        if enabled == Some(false) {
            e.disable().await.map_err(|e| failed(e.to_string()))?;
        }
        if choice.is_some() || enabled == Some(true) {
            let consent = Consent::ask(
                "share_settings_set",
                serde_json::json!({ "enabled": enabled, "edge": edge }),
            )
            .await
            .map_err(|e| failed(format!("{e:#}")))?;
            if let Some(choice) = &choice {
                e.set_edge(choice, &consent)
                    .await
                    .map_err(|e| failed(e.to_string()))?;
            }
            if enabled == Some(true) {
                e.enable(&consent).await.map_err(|e| failed(e.to_string()))?;
            }
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
                reclaim_after_ms: a.reclaim_after_ms,
            })
            .map_err(|e| failed(format!("{e:#}")))
    }

    async fn share_terms_accept(
        &self,
        ext: &Extensions,
        edge: String,
        terms_rev: String,
    ) -> RpcResult<()> {
        owner(ext)?;
        let e = engine()?;
        let consent = Consent::ask(
            "share_terms_accept",
            serde_json::json!({ "edge": edge, "terms_rev": terms_rev }),
        )
        .await
        .map_err(|e| failed(format!("{e:#}")))?;
        e.accept_terms(&edge, &terms_rev, &consent)
            .await
            .map_err(|e| failed(e.to_string()))
    }

    async fn share_names_list(&self) -> RpcResult<Vec<HeldName>> {
        names::list(&*engine()?).await.map_err(err)
    }

    async fn share_names_reserve(&self, ext: &Extensions, name: String) -> RpcResult<HeldName> {
        owner(ext)?;
        let e = engine()?;
        Consent::ask("share_names_reserve", serde_json::json!({ "name": name }))
            .await
            .map_err(err)?;
        names::reserve(&e, &name).await.map_err(err)
    }

    async fn share_names_release(&self, ext: &Extensions, name: String) -> RpcResult<NameReleasedInfo> {
        owner(ext)?;
        let r = names::release(&*engine()?, &name).await.map_err(err)?;
        Ok(NameReleasedInfo {
            name: r.name,
            reclaim_after_ms: r.reclaim_after_ms,
        })
    }

    async fn share_names_primary(&self, ext: &Extensions, name: String) -> RpcResult<HeldName> {
        owner(ext)?;
        names::primary(&*engine()?, &name).await.map_err(err)
    }

    async fn share_names_rotate(&self, ext: &Extensions, name: String) -> RpcResult<u32> {
        owner(ext)?;
        names::rotate(&*engine()?, &name).await.map_err(err)
    }

    async fn share_names_transfer(
        &self,
        ext: &Extensions,
        name: String,
        to_mesh: String,
    ) -> RpcResult<u64> {
        owner(ext)?;
        let e = engine()?;
        Consent::ask(
            "share_names_transfer",
            serde_json::json!({ "name": name, "to_mesh": to_mesh }),
        )
        .await
        .map_err(err)?;
        names::transfer(&e, &name, &to_mesh).await.map_err(err)
    }

    async fn share_delegate(
        &self,
        ext: &Extensions,
        engine_id: String,
        edge: String,
        domain: String,
        edge_now_ms: u64,
    ) -> RpcResult<serde_json::Value> {
        owner(ext)?;
        let parse = |s: &str, what: &str| {
            s.parse::<share_proto::PublicKey>()
                .map_err(|_| failed(format!("{what} is not an endpoint id")))
        };
        let signed = crate::wiring::delegate_as_holder(
            parse(&engine_id, "engine")?,
            parse(&edge, "edge")?,
            &domain,
            edge_now_ms,
        )
        .await
        .map_err(err)?;
        serde_json::to_value(signed).map_err(|e| failed(e.to_string()))
    }

    async fn share_name_key_ensure(
        &self,
        ext: &Extensions,
        domain: String,
        name: String,
    ) -> RpcResult<String> {
        owner(ext)?;
        let store = crate::wiring::VaultNameKeys(None);
        let key = crate::wiring::name_key_as_holder(&store, &domain, &name)
            .await
            .map_err(err)?;
        let spki = crate::holder::row_spki(&key, &name, &domain).map_err(err)?;
        Ok(crate::holder::spki_hex(&spki))
    }

    async fn share_password(&self, ext: &Extensions, id: String) -> RpcResult<Option<String>> {
        owner(ext)?;
        Ok(engine()?.password(&id))
    }
}

pub(super) fn rpc_module(server: Server) -> rpc_core::DescribedModule {
    rpc_core::DescribedModule::new(ShareApiServer::into_rpc(server), rpc_descriptors())
        .expect("share RPC descriptors")
}
