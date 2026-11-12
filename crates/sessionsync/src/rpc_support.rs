use crate as sessionsync;
use crate::types::SessionOwner;
use agent::{Agents, rpc_support::RpcServerExt};
use agent::rpc_forward as forward;
use mesh::rpc_support::mesh_actor;
use jsonrpsee::{Extensions, core::RpcResult};
use rpc_core::{ask, failed, Server};
use runtime::SessionId;

/// Full session-owner transfer shared independently of the hub wire trait.
pub trait RpcSessionMoveExt {
    /// Move ownership with the existing admission, wait and drain policy.
    fn session_move<'a>(
        &'a self,
        ext: &'a Extensions,
        session: SessionId,
        to: String,
    ) -> impl std::future::Future<Output = RpcResult<SessionOwner>> + Send + 'a;
}

impl RpcSessionMoveExt for Server {
    async fn session_move(
        &self,
        ext: &Extensions,
        session: SessionId,
        to: String,
    ) -> RpcResult<SessionOwner> {
        self.authorize(ext, session).await?;
        let store = ask::<Agents, _>(agent::AGENT_ACTOR, agent::DurableStore).await?;
        let connected =
            ask::<sessionsync::SessionSync, _>(sessionsync::SESSIONS_ACTOR, sessionsync::Peers)
                .await?;
        // The pause map is this engine's, in memory: the target could not refuse it.
        let held = ask::<Agents, _>(agent::AGENT_ACTOR, agent::supervisor::HeldDesks).await?;
        let peers = match mesh_actor() {
            Ok(m) => m
                .ask(mesh::Status)
                .await
                .map(|s| s.peers)
                .unwrap_or_default(),
            Err(_) => Vec::new(),
        };
        let name = peers
            .iter()
            .find(|p| p.id.0 == to)
            .map(|p| p.name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| to.get(..8).unwrap_or(&to).to_owned());
        let to = replica::PeerId(to);
        // The take's worst case: release reply (release wait + drain), then the tail wait.
        let sessions = self
            .rt
            .section::<sessionsync::SessionsConfig>("sessions")
            .unwrap_or_default();
        let drain = self
            .rt
            .section::<agent::durability::DurabilityConfig>("durability")
            .unwrap_or_default()
            .drain_secs;
        let wait = std::time::Duration::from_secs(2 * sessions.release_wait_secs + drain + 15);
        // 183 §4.2: Move to on a viewer runs on the owner, which moves it as
        // today (one hop: a forwarded move for a row not ours is NotOwner).
        let me = store.engine().to_owned();
        if let Some((owner, _)) = store
            .owner(session)
            .await
            .map_err(|e| failed(format!("{e:#}")))?
            && !owner.is_empty()
            && owner != me
        {
            if ext.get::<forward::Forwarded>().is_some() {
                return Err(forward::not_owner(owner));
            }
            let owner = replica::PeerId(owner);
            let call = forward::to_peer_within::<SessionOwner>(
                ext,
                &owner,
                "session_move",
                (session, &to.0),
                wait + std::time::Duration::from_secs(10),
            );
            return match call.await? {
                Some(mut o) => {
                    o.here = o.owner == me;
                    Ok(o)
                }
                None => Err(forward::unreachable(owner.0, "not connected").await),
            };
        }
        // The target's own take: it releases from us (drain) and lands epoch + 1.
        // Its own call even when this move was forwarded from a viewer (the
        // take is the target's, bound to us as the owner, not a relay).
        let direct = Extensions::new();
        let ask_target = async {
            let call = forward::to_peer_within::<SessionOwner>(
                &direct,
                &to,
                "session_take",
                (session, "moved"),
                wait + std::time::Duration::from_secs(5),
            );
            match tokio::time::timeout(wait, call).await {
                Err(_) => Err(sessionsync::owner::MoveFailure::NoAnswer),
                Ok(Ok(o)) => Ok(o.map(|o| (o.owner, o.epoch))),
                Ok(Err(e)) if e.code() == forward::OWNER_UNREACHABLE => Ok(None),
                Ok(Err(e)) => Err(sessionsync::owner::MoveFailure::Refused(
                    e.message().to_owned(),
                )),
            }
        };
        let (owner, epoch) =
            sessionsync::owner::move_to(&store, session, &to, &name, &connected, &held, ask_target)
                .await
                .map_err(failed)?;
        tracing::info!(%session, to = %name, epoch, "181: session moved");
        Ok(SessionOwner {
            owner,
            epoch,
            here: false,
        })
    }
}
