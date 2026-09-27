//! One client shape for both ends of the wire (63 §2.3). [`Engine`] is either
//! the in-process [`Local`] (the whole [`crate::api::Api`] dispatched through
//! its own `Methods`, as the owner peer) or a WebSocket to another process's
//! engine; both implement jsonrpsee's `ClientT`, so the generated
//! [`crate::api::ApiClient`] gives every frontend the same typed methods.
//! [`mirror`] republishes the remote engine's event stream on the local bus,
//! so bus subscribers (the TUI bridge, the ACP relay) work unchanged.

use jsonrpsee::core::client::{
    BatchResponse, ClientT, Error, Subscription, SubscriptionClientT,
};
use jsonrpsee::core::params::BatchRequestBuilder;
use jsonrpsee::core::traits::ToRpcParams;
use jsonrpsee::rpc_params;
use jsonrpsee::types::{Id, Request, Response, ResponsePayload};
use jsonrpsee::ws_client::{HeaderMap, HeaderValue, WsClient, WsClientBuilder};
use jsonrpsee::Methods;
use std::sync::Arc;
use runtime::Runtime;
use serde::de::DeserializeOwned;
use serde_json::value::RawValue;

use crate::api::ApiServer;
use crate::engine::Running;

/// The in-process client: the owner console's peer on every call.
pub struct Local(Methods);

impl Local {
    /// The full API over `rt`'s actors, every call as the owner console.
    pub fn new(rt: Runtime) -> Self {
        let mut methods: Methods = crate::api::Server::new(rt).into_rpc().into();
        methods.extensions_mut().insert(link::RpcPeer::internal());
        Self(methods)
    }
}

impl ClientT for Local {
    async fn notification<Params>(&self, _method: &str, _params: Params) -> Result<(), Error>
    where
        Params: ToRpcParams + Send,
    {
        Err(Error::Custom("notifications are not used in-process".into()))
    }

    async fn request<R, Params>(&self, method: &str, params: Params) -> Result<R, Error>
    where
        R: DeserializeOwned,
        Params: ToRpcParams + Send,
    {
        let params = params.to_rpc_params()?;
        let req = Request::borrowed(method, params.as_deref(), Id::Number(0));
        let (rp, _) = self.0.raw_json_request(&serde_json::to_string(&req)?, 1).await?;
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

    async fn subscribe_to_method<'a, Notif>(
        &self,
        _method: &'a str,
    ) -> Result<Subscription<Notif>, Error>
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
    fn notification<Params>(&self, method: &str, params: Params) -> impl Future<Output = Result<(), Error>> + Send
    where
        Params: ToRpcParams + Send,
    {
        async move {
            match self {
                Engine::Local(l) => l.notification(method, params).await,
                Engine::Remote(r) => r.notification(method, params).await,
            }
        }
    }

    fn request<R, Params>(&self, method: &str, params: Params) -> impl Future<Output = Result<R, Error>> + Send
    where
        R: DeserializeOwned,
        Params: ToRpcParams + Send,
    {
        async move {
            match self {
                Engine::Local(l) => l.request(method, params).await,
                Engine::Remote(r) => r.request(method, params).await,
            }
        }
    }

    fn batch_request<'a, R>(
        &self,
        batch: BatchRequestBuilder<'a>,
    ) -> impl Future<Output = Result<BatchResponse<'a, R>, Error>> + Send
    where
        R: DeserializeOwned + std::fmt::Debug + 'a,
    {
        async move {
            match self {
                Engine::Local(l) => l.batch_request(batch).await,
                Engine::Remote(r) => r.batch_request(batch).await,
            }
        }
    }
}

impl SubscriptionClientT for Engine {
    fn subscribe<'a, Notif, Params>(
        &self,
        subscribe_method: &'a str,
        params: Params,
        unsubscribe_method: &'a str,
    ) -> impl Future<Output = Result<Subscription<Notif>, Error>> + Send
    where
        Params: ToRpcParams + Send,
        Notif: DeserializeOwned,
    {
        async move {
            match self {
                Engine::Local(l) => l.subscribe(subscribe_method, params, unsubscribe_method).await,
                Engine::Remote(r) => r.subscribe(subscribe_method, params, unsubscribe_method).await,
            }
        }
    }

    fn subscribe_to_method<'a, Notif>(
        &self,
        method: &'a str,
    ) -> impl Future<Output = Result<Subscription<Notif>, Error>> + Send
    where
        Notif: DeserializeOwned,
    {
        async move {
            match self {
                Engine::Local(l) => l.subscribe_to_method(method).await,
                Engine::Remote(r) => r.subscribe_to_method(method).await,
            }
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
    headers.insert("Authorization", HeaderValue::from_str(&format!("Bearer {token}"))?);
    Ok(WsClientBuilder::default()
        .set_headers(headers)
        .max_request_size(64 << 20)
        .max_response_size(64 << 20)
        .build(format!("ws://{listen}"))
        .await?)
}

/// `subscribe_events` on `client`, each JSON event decoded back into its bus
/// type (the same serde types the [`crate::bridge`] serialised, matched by
/// their `type` tag) and published on the local bus. Returns when the
/// subscription ends — the socket closed.
pub async fn mirror(rt: &Runtime, client: &WsClient) -> anyhow::Result<()> {
    let mut events: Subscription<serde_json::Value> = client
        .subscribe(
            "subscribe_events",
            rpc_params![serde_json::Value::Null, serde_json::Value::Null],
            "unsubscribe_events",
        )
        .await?;
    while let Some(ev) = events.next().await {
        match ev {
            Ok(v) => publish(rt, &v).await,
            Err(e) => tracing::warn!(error = %e, "event stream"),
        }
    }
    Ok(())
}

/// Decode `v` as the first bus type it fits (tags are unique across the
/// stream) and publish it; the most frequent types are tried first. The
/// `PendingPrompts` snapshot has no bus type: its items are published one by
/// one, so in-process consumers (the TUI) see the pending cards as
/// `*Requested` events.
async fn publish(rt: &Runtime, v: &serde_json::Value) {
    if let Ok(p) = <crate::api::PendingPrompts as serde::Deserialize>::deserialize(v) {
        for a in p.approvals {
            rt.publish(a).await;
        }
        for a in p.asks {
            rt.publish(a).await;
        }
        for s in p.secrets {
            rt.publish(s).await;
        }
        return;
    }
    macro_rules! republish {
        ($($t:ty),* $(,)?) => {$(
            if let Ok(ev) = <$t as serde::Deserialize>::deserialize(v) {
                rt.publish(ev).await;
                return;
            }
        )*};
    }
    republish!(
        agent::AgentEvent,
        tools::JobEvent,
        tools::McpProgress,
        web::progress::ScrapeProgress,
        tools::ApprovalRequested,
        tools::ApprovalResolved,
        tools::AskRequested,
        tools::AskResolved,
        agent::LoginFinished,
        agent::CommunicationChanged,
        agent::PeerMessageChanged,
        agent::PeerDelivered,
        a2a::A2aChanged,
        signal::SignalEvent,
        slack::SlackEvent,
        knowledge::KnowledgeChanged,
        document::DocumentChanged,
        browser::BrowserEvent,
        link::LinkEvent,
        nodes::NodeEvent,
        schedule::ScheduleEvent,
        artifact::ArtifactShown,
        secrets::SecretsEvent,
        update::UpdateReady,
        voice::VoiceEvent,
        meeting::MeetingEvent,
        meeting::BriefEvent,
        app::AppCallChanged,
    );
    tracing::debug!(tag = v["type"].as_str().unwrap_or("?"), "event with no bus type");
}
