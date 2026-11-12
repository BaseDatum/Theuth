//! One client shape for both ends of the wire (63 §2.3). [`Engine`] is either
//! the in-process [`Local`] (the injected owner methods dispatched through
//! its own `Methods`, as the owner peer) or a WebSocket to another process's
//! engine; both implement jsonrpsee's `ClientT`, so the generated
//! owner client traits give every frontend the same typed methods.

use jsonrpsee::Methods;
use jsonrpsee::core::client::{BatchResponse, ClientT, Error, Subscription, SubscriptionClientT};
use jsonrpsee::core::params::BatchRequestBuilder;
use jsonrpsee::core::traits::ToRpcParams;
use jsonrpsee::types::{Id, Request, Response, ResponsePayload};
use jsonrpsee::ws_client::{HeaderMap, HeaderValue, WsClient, WsClientBuilder};
use runtime::Runtime;
use serde::de::DeserializeOwned;
use serde_json::value::RawValue;
use std::sync::Arc;

use rpc_core::ModuleFactory;
use crate::engine::Running;

/// The in-process client: the owner console's peer on every call.
pub struct Local(Methods);

impl Local {
    /// The full API over `rt`'s actors, every call as the owner console.
    pub fn new(rt: Runtime, module: ModuleFactory) -> Self {
        let mut methods: Methods = module(rpc_core::Server::new(rt)).into();
        methods.extensions_mut().insert(rpc_core::RpcPeer::internal());
        Self(methods)
    }
}

impl ClientT for Local {
    async fn notification<Params>(&self, _method: &str, _params: Params) -> Result<(), Error>
    where
        Params: ToRpcParams + Send,
    {
        Err(Error::Custom(
            "notifications are not used in-process".into(),
        ))
    }

    async fn request<R, Params>(&self, method: &str, params: Params) -> Result<R, Error>
    where
        R: DeserializeOwned,
        Params: ToRpcParams + Send,
    {
        let params = params.to_rpc_params()?;
        let req = Request::borrowed(method, params.as_deref(), Id::Number(0));
        let (rp, _) = self
            .0
            .raw_json_request(&serde_json::to_string(&req)?, 1)
            .await?;
        let rp: Response<&RawValue> = serde_json::from_str(rp.get())?;
        match rp.payload {
            ResponsePayload::Success(v) => Ok(serde_json::from_str(v.get())?),
            ResponsePayload::Error(e) => Err(Error::Call(e.into_owned())),
        }
    }

    async fn batch_request<'a, R>(
        &self,
        _batch: BatchRequestBuilder<'a>,
    ) -> Result<BatchResponse<'a, R>, Error>
    where
        R: DeserializeOwned + std::fmt::Debug + 'a,
    {
        Err(Error::Custom("batches are not used in-process".into()))
    }
}

impl SubscriptionClientT for Local {
    /// In-process frontends subscribe to the bus, not to the API.
    async fn subscribe<'a, Notif, Params>(
        &self,
        _subscribe_method: &'a str,
        _params: Params,
        _unsubscribe_method: &'a str,
    ) -> Result<Subscription<Notif>, Error>
    where
        Params: ToRpcParams + Send,
        Notif: DeserializeOwned,
    {
        Err(Error::Custom("subscribe to the bus in-process".into()))
    }

    async fn subscribe_to_method<Notif>(&self, _method: &str) -> Result<Subscription<Notif>, Error>
    where
        Notif: DeserializeOwned,
    {
        Err(Error::Custom("subscribe to the bus in-process".into()))
    }
}

/// Where a frontend's calls go.
pub enum Engine {
    /// This process is the engine.
    Local(Local),
    /// Attached to another process's engine (63 §0.3).
    Remote(Arc<WsClient>),
}

impl ClientT for Engine {
    async fn notification<Params>(&self, method: &str, params: Params) -> Result<(), Error>
    where
        Params: ToRpcParams + Send,
    {
        match self {
            Engine::Local(l) => l.notification(method, params).await,
            Engine::Remote(r) => r.notification(method, params).await,
        }
    }

    async fn request<R, Params>(&self, method: &str, params: Params) -> Result<R, Error>
    where
        R: DeserializeOwned,
        Params: ToRpcParams + Send,
    {
        match self {
            Engine::Local(l) => l.request(method, params).await,
            Engine::Remote(r) => r.request(method, params).await,
        }
    }

    async fn batch_request<'a, R>(
        &self,
        batch: BatchRequestBuilder<'a>,
    ) -> Result<BatchResponse<'a, R>, Error>
    where
        R: DeserializeOwned + std::fmt::Debug + 'a,
    {
        match self {
            Engine::Local(l) => l.batch_request(batch).await,
            Engine::Remote(r) => r.batch_request(batch).await,
        }
    }
}

impl SubscriptionClientT for Engine {
    async fn subscribe<'a, Notif, Params>(
        &self,
        subscribe_method: &'a str,
        params: Params,
        unsubscribe_method: &'a str,
    ) -> Result<Subscription<Notif>, Error>
    where
        Params: ToRpcParams + Send,
        Notif: DeserializeOwned,
    {
        match self {
            Engine::Local(l) => {
                l.subscribe(subscribe_method, params, unsubscribe_method)
                    .await
            }
            Engine::Remote(r) => {
                r.subscribe(subscribe_method, params, unsubscribe_method)
                    .await
            }
        }
    }

    async fn subscribe_to_method<Notif>(&self, method: &str) -> Result<Subscription<Notif>, Error>
    where
        Notif: DeserializeOwned,
    {
        match self {
            Engine::Local(l) => l.subscribe_to_method(method).await,
            Engine::Remote(r) => r.subscribe_to_method(method).await,
        }
    }
}

/// Connect to a running engine: 127.0.0.1 only, `<data_dir>/token` as the
/// bearer on the upgrade. `Err` when the engine serves no RPC.
pub async fn connect(running: &Running) -> anyhow::Result<WsClient> {
    let listen = running
        .record
        .listen
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("{running}"))?;
    let token = running
        .url
        .as_deref()
        .and_then(|u| u.rsplit_once("token=").map(|(_, t)| t))
        .ok_or_else(|| anyhow::anyhow!("no <data_dir>/token"))?;
    anyhow::ensure!(
        listen.starts_with("127.0.0.1:") || listen.starts_with("localhost:"),
        "engine listens on {listen}; only loopback is attachable"
    );
    let mut headers = HeaderMap::new();
    headers.insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {token}"))?,
    );
    let client = WsClientBuilder::default()
        .set_headers(headers)
        .max_request_size(64 << 20)
        .max_response_size(64 << 20)
        .build(format!("ws://{listen}"))
        .await?;
    // Take the cache out while awaiting the peer: unrelated endpoints never
    // wait behind a network operation, and no old support escapes this barrier.
    static CACHE: std::sync::LazyLock<parking_lot::Mutex<std::collections::HashMap<String, rpc_core::DiscoveryCache>>> = std::sync::LazyLock::new(Default::default);
    let caches = &*CACHE;
    let mut cache = caches.lock().remove(listen).unwrap_or_default();
    let negotiated = cache.negotiate(&client, listen, rpc_core::DiscoveryView::Owner).await;
    caches.lock().insert(listen.to_owned(), cache);
    negotiated?;
    Ok(client)
}



#[cfg(test)]
mod discovery_tests {
    use super::*;
    use rpc_core::{Discovery, DiscoveryCache, DiscoveryView};

    fn module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
        crate::rpc_module(server).module
    }

    fn composed(plugins: &[&str], version: &str) -> Local {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let described = crate::rpc_module(rpc_core::Server::new(rt.clone()));
        let discovery = Discovery {
            descriptors: described.descriptors,
            plugins: plugins.iter().map(|name| (*name).to_owned()).collect(),
            ..Default::default()
        };
        rpc_core::freeze_discovery(&rt, &discovery, version).unwrap();
        Local::new(rt, module)
    }

    #[tokio::test]
    async fn reconnect_fetches_fresh_summary_and_failover_invalidates_old_contract() {
        let mut cache = DiscoveryCache::default();
        let first = cache.negotiate(&composed(&["rpc"], "old"), "engine", DiscoveryView::Owner).await.unwrap().unwrap();
        let reconnect = cache.negotiate(&composed(&["rpc"], "rebuilt"), "engine", DiscoveryView::Owner).await.unwrap().unwrap();
        assert_eq!(first.capabilities.schema_hash, reconnect.capabilities.schema_hash);
        assert_eq!(reconnect.capabilities.server_version, "rebuilt");
        let failover = cache.negotiate(&composed(&["rpc", "new-plugin"], "failover"), "engine", DiscoveryView::Owner).await.unwrap().unwrap();
        assert_ne!(first.capabilities.schema_hash, failover.capabilities.schema_hash);
        let document: serde_json::Value = serde_json::from_str(failover.document.0.get()).unwrap();
        assert_eq!(document["x-theuth"]["plugins"], serde_json::json!(["new-plugin", "rpc"]));
        let guest = cache.negotiate(&composed(&["rpc"], "guest-view"), "other-engine", DiscoveryView::Guest).await.unwrap().unwrap();
        assert_eq!(guest.capabilities.server_version, "guest-view");
    }

    #[tokio::test]
    async fn only_method_not_found_selects_legacy_never_a_failed_current_summary() {
        fn legacy(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
            jsonrpsee::RpcModule::new(server)
        }
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let mut cache = DiscoveryCache::default();
        cache.negotiate(&composed(&["rpc"], "build"), "engine", DiscoveryView::Owner).await.unwrap().unwrap();
        assert!(cache.negotiate(&Local::new(rt.clone(), module), "engine", DiscoveryView::Owner).await.is_err());
        assert!(cache.negotiate(&Local::new(rt, legacy), "engine", DiscoveryView::Owner).await.unwrap().is_none());
    }
}
