//! iOS standalone implementations of the [`Mesh`] actor message handlers and
//! [`MeshPlugin`] (mesh networking is disabled on iOS where `iroh` is gated out).

use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use runtime::Runtime;
use tokio::sync::broadcast;

use crate::{
    AddPeer, ConfirmPeer, DocsGate, DocsGateState, EngineId, EngineStats, ListStats, Mesh,
    MeshEvent, MeshPlugin, MeshStatus, Nearby, NearbyEngine, NearbyWatch, PairAnswer, PairCeremony,
    PairNearby, PairingString, Peer, PreviewUnpair, RemovePeer, RemovedPeers, Rename, RotateVault,
    SetHubPref, SetMessaging, SetNearby, Status, Subscribe, Unpair, UnpairPreview, declare_wire,
};

const UNAVAILABLE: &str = "mesh networking is unavailable on iOS";

impl Message<Status> for Mesh {
    type Reply = Result<MeshStatus, String>;

    async fn handle(&mut self, _: Status, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        Err(UNAVAILABLE.into())
    }
}

impl Message<RemoteTicket> for Mesh {
    type Reply = Result<String, String>;

    async fn handle(&mut self, _: RemoteTicket, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        Err(UNAVAILABLE.into())
    }
}

impl Message<AddPeer> for Mesh {
    type Reply = DelegatedReply<Result<Peer, String>>;

    async fn handle(&mut self, _: AddPeer, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        if let Some(tx) = tx {
            tx.send(Err(UNAVAILABLE.into()));
        }
        reply
    }
}

impl Message<RemovePeer> for Mesh {
    type Reply = Result<(), String>;

    async fn handle(&mut self, _: RemovePeer, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        Err(UNAVAILABLE.into())
    }
}

impl Message<Unpair> for Mesh {
    type Reply = Result<(), String>;

    async fn handle(&mut self, _: Unpair, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        Err(UNAVAILABLE.into())
    }
}

impl Message<ConfirmPeer> for Mesh {
    type Reply = Result<(), String>;

    async fn handle(&mut self, _: ConfirmPeer, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        Err(UNAVAILABLE.into())
    }
}

impl Message<SetHubPref> for Mesh {
    type Reply = Result<(), String>;

    async fn handle(&mut self, _: SetHubPref, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        Err(UNAVAILABLE.into())
    }
}

impl Message<PairingString> for Mesh {
    type Reply = Result<String, String>;

    async fn handle(
        &mut self,
        _: PairingString,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        Err(UNAVAILABLE.into())
    }
}

impl Message<ListStats> for Mesh {
    type Reply = Result<Vec<EngineStats>, String>;

    async fn handle(&mut self, _: ListStats, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        Ok(Vec::new())
    }
}

impl Message<Subscribe> for Mesh {
    type Reply = broadcast::Receiver<MeshEvent>;

    async fn handle(&mut self, _: Subscribe, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        broadcast::channel(1).1
    }
}

impl Message<Nearby> for Mesh {
    type Reply = Result<Vec<NearbyEngine>, String>;

    async fn handle(&mut self, _: Nearby, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        Ok(Vec::new())
    }
}

impl Message<NearbyWatch> for Mesh {
    type Reply = Result<(), String>;

    async fn handle(&mut self, _: NearbyWatch, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        Err(UNAVAILABLE.into())
    }
}

impl Message<PairNearby> for Mesh {
    type Reply = DelegatedReply<Result<PairCeremony, String>>;

    async fn handle(&mut self, _: PairNearby, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        if let Some(tx) = tx {
            tx.send(Err(UNAVAILABLE.into()));
        }
        reply
    }
}

impl Message<PairAnswer> for Mesh {
    type Reply = Result<(), String>;

    async fn handle(&mut self, _: PairAnswer, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        Err(UNAVAILABLE.into())
    }
}

impl Message<SetMessaging> for Mesh {
    type Reply = Result<(), String>;

    async fn handle(&mut self, _: SetMessaging, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        Err(UNAVAILABLE.into())
    }
}

impl Message<SetNearby> for Mesh {
    type Reply = Result<(), String>;

    async fn handle(&mut self, _: SetNearby, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        Err(UNAVAILABLE.into())
    }
}

impl Message<Rename> for Mesh {
    type Reply = Result<(), String>;

    async fn handle(&mut self, _: Rename, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        Err(UNAVAILABLE.into())
    }
}

impl Message<PreviewUnpair> for Mesh {
    type Reply = Result<UnpairPreview, String>;

    async fn handle(
        &mut self,
        _: PreviewUnpair,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        Err(UNAVAILABLE.into())
    }
}

impl Message<RotateVault> for Mesh {
    type Reply = Result<secrets::RotationState, String>;

    async fn handle(&mut self, _: RotateVault, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        Err(UNAVAILABLE.into())
    }
}

impl Message<DocsGate> for Mesh {
    type Reply = DocsGateState;

    async fn handle(&mut self, _: DocsGate, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        DocsGateState {
            floor_ok: true,
            homes_on: true,
        }
    }
}

impl Message<RemovedPeers> for Mesh {
    type Reply = Vec<EngineId>;

    async fn handle(&mut self, _: RemovedPeers, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        Vec::new()
    }
}

#[async_trait::async_trait]
impl runtime::Plugin for MeshPlugin {
    fn name(&self) -> &'static str {
        "mesh"
    }

    async fn declare_wire(&mut self, rt: &Runtime) -> anyhow::Result<()> {
        declare_wire(rt).await
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        declare_wire(rt).await?;
        tracing::info!("mesh: disabled on iOS (standalone engine)");
        Ok(())
    }
}
