use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use runtime::SessionId;
use tools::ToolRegistry;
use rpc_core::{ask, Server};

#[rpc(server, client)]
pub trait WebSettingsApi {
    /// One probe query through one registered provider (built-in or external):
    /// ok, latency, rows, error text. Unknown / unregistered names are errors.
    #[method(name = "search_provider_test")]
    async fn search_provider_test(
        &self,
        name: String,
        query: String,
    ) -> RpcResult<web::settings::ProviderProbe>;
    /// Per-provider health from the last calls (rate-limited, captcha, parse
    /// error, …) for every registered provider, in registration order.
    #[method(name = "search_status")]
    async fn search_status(&self) -> RpcResult<Vec<web::health::ProviderStatus>>;
    /// Run the whole `search_web` tool once (fan-out, rank, read) from Settings
    /// › Search "Run a test search"; returns the model-facing text and the JSON.
    #[method(name = "search_test")]
    async fn search_test(
        &self,
        query: String,
        site: Option<String>,
        recent: Option<web::Recency>,
    ) -> RpcResult<web::settings::SearchTestResult>;
}

#[async_trait]
impl WebSettingsApiServer for Server {

    async fn search_provider_test(
        &self,
        name: String,
        query: String,
    ) -> RpcResult<web::settings::ProviderProbe> {
        ask::<web::WebActor, _>(web::WEB_ACTOR, web::settings::Probe { name, query }).await
    }

    async fn search_status(&self) -> RpcResult<Vec<web::health::ProviderStatus>> {
        ask::<web::WebActor, _>(web::WEB_ACTOR, web::settings::Status).await
    }

    async fn search_test(
        &self,
        query: String,
        site: Option<String>,
        recent: Option<web::Recency>,
    ) -> RpcResult<web::settings::SearchTestResult> {
        let out = ask::<ToolRegistry, _>(
            tools::REGISTRY_ACTOR,
            tools::Invoke(tools::ToolCall {
                id: format!("search-test-{}", uuid::Uuid::now_v7()),
                name: "search_web".into(),
                args: serde_json::json!({ "query": query, "site": site, "recent": recent }),
                session: SessionId::new(),
                quiet: true,
            }),
        )
        .await?;
        Ok(web::settings::SearchTestResult {
            text: out.content,
            is_error: out.is_error,
            structured: out.structured.unwrap_or(serde_json::Value::Null),
        })
    }
}

pub(super) fn rpc_module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    WebSettingsApiServer::into_rpc(server)
}
