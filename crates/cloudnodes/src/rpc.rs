//! The `cloud_*` RPC family (design doc 224 §13): owner only. Every method is one call on the
//! shared [`crate::engine::Cloud`] service; `cloud_create`, `cloud_restart`, `cloud_resize` and
//! `cloud_update` return at once and report on `cloudnodes.events`.

use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use rpc_core::Server;

use crate::rpc_support::{cloud, err};
use crate::types::{
    CloudList, CloudNode, DesktopOpen, K8sInfo, NodeLogs, NodeSpec, OauthStart, ProviderInfo,
    ProviderKind, Quote, SizeList, TerminalOpen,
};

#[rpc_core_macros::rpc_descriptors(owner = "cloudnodes", descriptors = rpc_descriptors, audience = "owner")]
#[rpc(server, client)]
pub trait CloudNodesApi {
    /// Every provider: availability, credential state and source, credentials found on this
    /// machine, sizes, regions and prices.
    #[method(name = "cloud_providers")]
    async fn cloud_providers(&self) -> RpcResult<Vec<ProviderInfo>>;
    /// Copy a discovered CLI login (`Discovered.id`) into the vault; checked first.
    #[method(name = "cloud_import_credential")]
    async fn cloud_import_credential(
        &self,
        provider: ProviderKind,
        id: String,
    ) -> RpcResult<ProviderInfo>;
    /// Store a pasted API token in the vault; checked against the provider first.
    #[method(name = "cloud_set_token")]
    async fn cloud_set_token(
        &self,
        provider: ProviderKind,
        token: String,
    ) -> RpcResult<ProviderInfo>;
    /// Remove the provider's credential from the vault.
    #[method(name = "cloud_forget_credential")]
    async fn cloud_forget_credential(&self, provider: ProviderKind) -> RpcResult<()>;
    /// Start "Sign in with DigitalOcean": open `url` in the system browser; the outcome arrives
    /// as `CloudCredentialChanged`.
    #[method(name = "cloud_oauth_start")]
    async fn cloud_oauth_start(&self, provider: ProviderKind) -> RpcResult<OauthStart>;
    /// Stop waiting for a sign-in.
    #[method(name = "cloud_oauth_cancel")]
    async fn cloud_oauth_cancel(&self, provider: ProviderKind) -> RpcResult<()>;
    /// The monthly cost of a spec (server + disk).
    #[method(name = "cloud_quote")]
    async fn cloud_quote(&self, spec: NodeSpec) -> RpcResult<Quote>;
    /// The provider's sizes priced live from its API (cached 1 h); unpriced when it cannot be
    /// asked (`priced: false`, `error`).
    #[method(name = "cloud_sizes")]
    async fn cloud_sizes(
        &self,
        provider: ProviderKind,
        region: Option<String>,
    ) -> RpcResult<SizeList>;
    /// Kubernetes: contexts, namespaces, storage classes and the CNI of `context`.
    #[method(name = "cloud_k8s_info")]
    async fn cloud_k8s_info(
        &self,
        #[argument(rename = "context")] kube_context: Option<String>,
    ) -> RpcResult<K8sInfo>;
    /// Create a node; returns at once in `creating`, the job streams `CloudNodeProgress`.
    /// `vault_password`: the master password for the node's sealed vault (design doc 225; at
    /// least 12 characters). Held in memory by the create job only, sent once to the node over
    /// the mesh after it joined, then dropped; never stored, in an event or in an answer.
    #[method(name = "cloud_create")]
    async fn cloud_create(
        &self,
        spec: NodeSpec,
        vault_password: Option<String>,
    ) -> RpcResult<CloudNode>;
    /// Every node: this engine's records, every connected provider's tagged nodes, the mesh.
    #[method(name = "cloud_list")]
    async fn cloud_list(&self) -> RpcResult<CloudList>;
    /// One node, with the provider's current view.
    #[method(name = "cloud_status")]
    async fn cloud_status(&self, name: String) -> RpcResult<CloudNode>;
    /// Restart the node's server; returns at once in `busy`.
    #[method(name = "cloud_restart")]
    async fn cloud_restart(&self, name: String) -> RpcResult<CloudNode>;
    /// Change the size (2, 4 or 8 GB); returns at once in `busy`.
    #[method(name = "cloud_resize")]
    async fn cloud_resize(&self, name: String, memory_gb: u8) -> RpcResult<CloudNode>;
    /// Install the newest release now (a restart: the node installs it before it starts).
    #[method(name = "cloud_update")]
    async fn cloud_update(&self, name: String) -> RpcResult<CloudNode>;
    /// Delete the node at its provider; its disk too unless `keep_volume` (default `true`).
    /// Deleting the disk also removes the node from the mesh. Returns what was done.
    #[method(name = "cloud_destroy")]
    async fn cloud_destroy(
        &self,
        name: String,
        keep_volume: Option<bool>,
    ) -> RpcResult<Vec<String>>;
    /// A shell on the node's engine over the mesh (no SSH); attach with `subscribe_pty`.
    #[method(name = "cloud_terminal")]
    async fn cloud_terminal(&self, name: String) -> RpcResult<TerminalOpen>;
    /// The last `tail` lines of the node engine's log, read over the mesh.
    #[method(name = "cloud_logs")]
    async fn cloud_logs(&self, name: String, tail: Option<u32>) -> RpcResult<NodeLogs>;
    /// Watch the node's desktop: a noVNC endpoint on this engine tunnelled over the mesh to the
    /// node, and its password (RPC only, never in an event). Fails when it was created without.
    #[method(name = "cloud_desktop")]
    async fn cloud_desktop(&self, name: String) -> RpcResult<DesktopOpen>;
}

#[async_trait]
impl CloudNodesApiServer for Server {
    async fn cloud_providers(&self) -> RpcResult<Vec<ProviderInfo>> {
        Ok(cloud().await?.providers())
    }

    async fn cloud_import_credential(
        &self,
        provider: ProviderKind,
        id: String,
    ) -> RpcResult<ProviderInfo> {
        cloud().await?.import(provider, id).await.map_err(err)
    }

    async fn cloud_set_token(
        &self,
        provider: ProviderKind,
        token: String,
    ) -> RpcResult<ProviderInfo> {
        cloud().await?.set_token(provider, token).await.map_err(err)
    }

    async fn cloud_forget_credential(&self, provider: ProviderKind) -> RpcResult<()> {
        cloud().await?.forget(provider).await.map_err(err)
    }

    async fn cloud_oauth_start(&self, provider: ProviderKind) -> RpcResult<OauthStart> {
        cloud().await?.oauth_start(provider).map_err(err)
    }

    async fn cloud_oauth_cancel(&self, provider: ProviderKind) -> RpcResult<()> {
        cloud().await?.oauth_cancel(provider);
        Ok(())
    }

    async fn cloud_quote(&self, spec: NodeSpec) -> RpcResult<Quote> {
        cloud().await?.quote(&spec).await.map_err(err)
    }

    async fn cloud_sizes(
        &self,
        provider: ProviderKind,
        region: Option<String>,
    ) -> RpcResult<SizeList> {
        Ok(cloud().await?.sizes(provider, region).await)
    }

    async fn cloud_k8s_info(&self, kube_context: Option<String>) -> RpcResult<K8sInfo> {
        cloud().await?.k8s_info(kube_context).await.map_err(err)
    }

    async fn cloud_create(
        &self,
        spec: NodeSpec,
        vault_password: Option<String>,
    ) -> RpcResult<CloudNode> {
        let password = vault_password.map(zeroize::Zeroizing::new);
        cloud().await?.create(spec, password).await.map_err(err)
    }

    async fn cloud_list(&self) -> RpcResult<CloudList> {
        Ok(cloud().await?.list().await)
    }

    async fn cloud_status(&self, name: String) -> RpcResult<CloudNode> {
        cloud().await?.status(&name).await.map_err(err)
    }

    async fn cloud_restart(&self, name: String) -> RpcResult<CloudNode> {
        cloud().await?.restart(&name).await.map_err(err)
    }

    async fn cloud_resize(&self, name: String, memory_gb: u8) -> RpcResult<CloudNode> {
        cloud().await?.resize(&name, memory_gb).await.map_err(err)
    }

    async fn cloud_update(&self, name: String) -> RpcResult<CloudNode> {
        cloud().await?.update_now(&name).await.map_err(err)
    }

    async fn cloud_destroy(
        &self,
        name: String,
        keep_volume: Option<bool>,
    ) -> RpcResult<Vec<String>> {
        cloud()
            .await?
            .destroy(&name, keep_volume.unwrap_or(true))
            .await
            .map_err(err)
    }

    async fn cloud_terminal(&self, name: String) -> RpcResult<TerminalOpen> {
        cloud().await?.terminal(&name).await.map_err(err)
    }

    async fn cloud_logs(&self, name: String, tail: Option<u32>) -> RpcResult<NodeLogs> {
        cloud().await?.logs(&name, tail).await.map_err(err)
    }

    async fn cloud_desktop(&self, name: String) -> RpcResult<DesktopOpen> {
        cloud().await?.desktop(&name).await.map_err(err)
    }
}

/// The `cloud_*` methods with their descriptors.
pub fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule {
    rpc_core::DescribedModule::new(CloudNodesApiServer::into_rpc(server), rpc_descriptors())
        .expect("cloudnodes RPC descriptors")
}
