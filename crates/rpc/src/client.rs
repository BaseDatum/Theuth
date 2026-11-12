//! One client shape for both ends of the wire (63 §2.3). [`Engine`] is either
//! the in-process [`Local`] (the whole [`crate::api`] dispatched through
//! its own `Methods`, as the owner peer) or a WebSocket to another process's
//! engine; both implement jsonrpsee's `ClientT`, so the generated
//! [`crate::api::clients`] gives every frontend the same typed methods.
//! [`mirror`] republishes the remote engine's event stream on the local bus,
//! so bus subscribers (the TUI bridge, the ACP relay) work unchanged.

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
pub use crate::client_mirror::mirror;
use crate::new_server;

/// The in-process client: the owner console's peer on every call.
pub struct Local(Methods);

impl Local {
    /// The full API over `rt`'s actors, every call as the owner console.
    pub fn new(rt: Runtime, module: ModuleFactory) -> Self {
        let mut methods: Methods = module(new_server(rt)).into();
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
    Ok(WsClientBuilder::default()
        .set_headers(headers)
        .max_request_size(64 << 20)
        .max_response_size(64 << 20)
        .build(format!("ws://{listen}"))
        .await?)
}


