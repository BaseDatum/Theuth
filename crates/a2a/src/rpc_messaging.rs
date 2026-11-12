use crate as a2a;
use a2a::A2aStatus;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use rpc_core::{send, failed, Server};
use a2a::rpc_support::{a2a, contacts, name_paired_on};

/// Messaging RPC methods owned by this plugin.
#[rpc_core_macros::rpc_descriptors(owner = "a2a", descriptors = rpc_descriptors, audience = "owner")]
#[rpc(server, client)]
pub trait A2aMessagingApi {
    /// A2A network status (50 §10): listener, configured peers and exports,
    /// and the config/listener/reload error when any. `a2a.enabled: false`
    /// answers the disabled state; enabled without a live actor is an error.
    #[method(name = "a2a_status")]
    async fn a2a_status(&self) -> RpcResult<A2aStatus>;
    /// Re-read `peers.yaml` and the `a2a` section, validate and swap
    /// atomically (50 §6.1); a failed reload keeps the old configuration and
    /// is the `Err`. `A2aChanged` follows on the stream.
    #[method(name = "reload_a2a")]
    async fn reload_a2a(&self) -> RpcResult<A2aStatus>;
    /// 183 §7.4: open (`true`, the Add contact sheet; returns the invite ticket) or close the
    /// Add contact window.
    #[method(name = "contact_window")]
    async fn contact_window(&self, open: bool) -> RpcResult<Option<a2a::contacts::ContactWindow>>;
    /// 183 §7.1: A2A agents the browse sees while the window is open, newest first, ≤ 32.
    #[method(name = "contact_nearby")]
    async fn contact_nearby(&self) -> RpcResult<Vec<a2a::contacts::NearbyAgent>>;
    /// 183 §7.4: start the ceremony with a Nearby agent, a ticket or a card URL.
    #[method(name = "contact_pair")]
    async fn contact_pair(
        &self,
        target: a2a::contacts::ContactTarget,
    ) -> RpcResult<mesh::PairCeremony>;
    /// 183 §7.4: this side's Accept (`accept`) or Reject for a contact ceremony.
    #[method(name = "contact_answer")]
    async fn contact_answer(&self, ceremony: uuid::Uuid, accept: bool) -> RpcResult<()>;
    /// 183 §12.7: this machine's contacts.
    #[method(name = "contact_list")]
    async fn contact_list(&self) -> RpcResult<Vec<a2a::contacts::Contact>>;
    /// 183 §12.7: rename, cap, grant exports, block or unblock a contact.
    #[method(name = "contact_update")]
    async fn contact_update(
        &self,
        name: String,
        patch: a2a::contacts::ContactPatch,
    ) -> RpcResult<a2a::contacts::Contact>;
    /// 183 §8.4: Remove (`block: false`) or Block a contact: local and immediate.
    #[method(name = "contact_remove")]
    async fn contact_remove(&self, name: String, block: bool) -> RpcResult<()>;
    /// 183 §7.6, D9: this machine's exports (agents and chats), with `paused`.
    #[method(name = "a2a_exports")]
    async fn a2a_exports(&self) -> RpcResult<Vec<a2a::contacts::Export>>;
    /// 183 §7.6: create or replace an export by name.
    #[method(name = "a2a_export_put")]
    async fn a2a_export_put(&self, export: a2a::contacts::Export) -> RpcResult<()>;
    /// 183 §12.10: remove an export and its grants (Stop sharing).
    #[method(name = "a2a_export_remove")]
    async fn a2a_export_remove(&self, name: String) -> RpcResult<()>;
    /// 183 §7.6 (W-27): the `contact_first` Attention item's Allow once · Always · Block.
    #[method(name = "contact_first_answer")]
    async fn contact_first_answer(
        &self,
        contact: String,
        export: String,
        answer: a2a::config::FirstContact,
    ) -> RpcResult<()>;
    /// 183 F3, F11: every (contact, export) whose first message waits for the owner's
    /// answer — the source of the `contact_first` Attention item; refetch on `A2aChanged`. 186
    /// review 1: then every contact message queued behind its chat's running turn (`queued`).
    #[method(name = "contact_first_waiting")]
    async fn contact_first_waiting(&self) -> RpcResult<Vec<a2a::contacts::FirstWaiting>>;
}

#[async_trait]
impl A2aMessagingApiServer for Server {

    async fn a2a_status(&self) -> RpcResult<A2aStatus> {
        match a2a(&self.rt)? {
            Some(actor) => send(&actor, a2a::GetStatus).await,
            None => Ok(A2aStatus::disabled()),
        }
    }

    async fn reload_a2a(&self) -> RpcResult<A2aStatus> {
        match a2a(&self.rt)? {
            Some(actor) => send(&actor, a2a::Reload).await,
            None => Ok(A2aStatus::disabled()),
        }
    }

    // 183 Phase 3 (§10.1): each asks the A2A actor, which runs whatever `a2a.enabled` says
    // (W-26). Only a process without the a2a plugin answers the `off` values.

    async fn contact_window(&self, open: bool) -> RpcResult<Option<a2a::contacts::ContactWindow>> {
        let off = if open {
            Err(failed(a2a::contacts::OFF))
        } else {
            Ok(None)
        };
        contacts(&self.rt, a2a::contacts::ContactWindowOpen(open), off).await
    }

    async fn contact_nearby(&self) -> RpcResult<Vec<a2a::contacts::NearbyAgent>> {
        contacts(&self.rt, a2a::contacts::ContactsNearby, Ok(Vec::new())).await
    }

    async fn contact_pair(
        &self,
        target: a2a::contacts::ContactTarget,
    ) -> RpcResult<mesh::PairCeremony> {
        let off = Err(failed(a2a::contacts::OFF));
        contacts(&self.rt, a2a::contacts::ContactPair(target), off).await
    }

    async fn contact_answer(&self, ceremony: uuid::Uuid, accept: bool) -> RpcResult<()> {
        let msg = a2a::contacts::ContactAnswer { ceremony, accept };
        contacts(&self.rt, msg, Err(failed(a2a::contacts::OFF))).await
    }

    async fn contact_list(&self) -> RpcResult<Vec<a2a::contacts::Contact>> {
        let mut out = contacts(&self.rt, a2a::contacts::ContactsList, Ok(Vec::new())).await?;
        name_paired_on(&mut out).await;
        Ok(out)
    }

    async fn contact_update(
        &self,
        name: String,
        patch: a2a::contacts::ContactPatch,
    ) -> RpcResult<a2a::contacts::Contact> {
        let msg = a2a::contacts::ContactUpdate { name, patch };
        let mut c = contacts(&self.rt, msg, Err(failed(a2a::contacts::OFF))).await?;
        name_paired_on(std::slice::from_mut(&mut c)).await;
        Ok(c)
    }

    async fn contact_remove(&self, name: String, block: bool) -> RpcResult<()> {
        let msg = a2a::contacts::ContactRemove { name, block };
        contacts(&self.rt, msg, Err(failed(a2a::contacts::OFF))).await
    }

    async fn a2a_exports(&self) -> RpcResult<Vec<a2a::contacts::Export>> {
        contacts(&self.rt, a2a::contacts::ExportsList, Ok(Vec::new())).await
    }

    async fn a2a_export_put(&self, export: a2a::contacts::Export) -> RpcResult<()> {
        let msg = a2a::contacts::ExportPut(export);
        contacts(&self.rt, msg, Err(failed(a2a::contacts::OFF))).await
    }

    async fn a2a_export_remove(&self, name: String) -> RpcResult<()> {
        let msg = a2a::contacts::ExportRemove { name };
        contacts(&self.rt, msg, Err(failed(a2a::contacts::OFF))).await
    }

    async fn contact_first_answer(
        &self,
        contact: String,
        export: String,
        answer: a2a::config::FirstContact,
    ) -> RpcResult<()> {
        let msg = a2a::contacts::ContactFirstAnswer {
            contact,
            export,
            answer,
        };
        contacts(&self.rt, msg, Err(failed(a2a::contacts::OFF))).await
    }

    async fn contact_first_waiting(&self) -> RpcResult<Vec<a2a::contacts::FirstWaiting>> {
        contacts(&self.rt, a2a::contacts::ContactsWaiting, Ok(Vec::new())).await
    }
}

pub(super) fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule {
    rpc_core::DescribedModule::new(A2aMessagingApiServer::into_rpc(server), rpc_descriptors())
        .expect("a2a RPC descriptors")
}
