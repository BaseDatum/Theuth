use crate as search_providers;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use rpc_core::{ask, Server};

/// Hub RPC methods owned by this plugin.
#[rpc(server, client)]
pub trait SearchProvidersHubApi {
    /// Settings › Search (16 §8): the four external providers (enabled, base
    /// URL, secret name and whether it is set - never the value, timeout,
    /// priority, registered) plus the `web` read/budget defaults.
    #[method(name = "search_settings_get")]
    async fn search_settings_get(&self) -> RpcResult<search_providers::SearchSettings>;
    /// Persist `search_providers.*` and the `web` defaults to config.yaml and
    /// apply them live: providers are re-registered with the `web` actor, the
    /// next `search_web` uses the new limits. A failed write changes nothing.
    #[method(name = "search_settings_set")]
    async fn search_settings_set(
        &self,
        settings: search_providers::SearchSettings,
    ) -> RpcResult<search_providers::SearchSettings>;
}

#[async_trait]
impl SearchProvidersHubApiServer for Server {

    async fn search_settings_get(&self) -> RpcResult<search_providers::SearchSettings> {
        let defaults = ask::<web::WebActor, _>(web::WEB_ACTOR, web::settings::GetDefaults).await?;
        let providers = ask::<search_providers::SearchProviders, _>(
            search_providers::SEARCH_PROVIDERS_ACTOR,
            search_providers::GetProviders,
        )
        .await?;
        Ok(search_providers::SearchSettings {
            providers,
            defaults,
        })
    }

    async fn search_settings_set(
        &self,
        settings: search_providers::SearchSettings,
    ) -> RpcResult<search_providers::SearchSettings> {
        let defaults = ask::<web::WebActor, _>(
            web::WEB_ACTOR,
            web::settings::SetDefaults(settings.defaults),
        )
        .await?;
        let providers = ask::<search_providers::SearchProviders, _>(
            search_providers::SEARCH_PROVIDERS_ACTOR,
            search_providers::SetProviders(settings.providers),
        )
        .await?;
        Ok(search_providers::SearchSettings {
            providers,
            defaults,
        })
    }
}

pub(super) fn rpc_module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    SearchProvidersHubApiServer::into_rpc(server)
}
