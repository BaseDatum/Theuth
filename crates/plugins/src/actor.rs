//! Owner: B1 — `PluginsActor` and the `Message` impls of every §5.2 message. B0 stubs: every
//! fallible message answers `PluginsError::NotImplemented`, the lists answer empty.

use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use serde_json::Value;

use crate::config::PluginsConfig;
use crate::error::PluginsError;
use crate::messages::*;
use crate::types::*;
use crate::PLUGINS_ACTOR;

/// The plugins host actor, registered as [`PLUGINS_ACTOR`].
#[derive(kameo::Actor)]
pub struct PluginsActor {
    #[allow(dead_code)] // B1: the host state reads these
    rt: runtime::Runtime,
    #[allow(dead_code)] // B1: the host state reads these
    cfg: PluginsConfig,
}

/// Spawn the actor on its own mailbox and register it as PLUGINS_ACTOR.
pub(crate) fn spawn(rt: &runtime::Runtime, cfg: PluginsConfig) -> anyhow::Result<ActorRef<PluginsActor>> {
    let actor = PluginsActor::spawn(PluginsActor { rt: rt.clone(), cfg });
    actor.register(PLUGINS_ACTOR)?;
    Ok(actor)
}

fn todo<T>(what: &str) -> Result<T, PluginsError> {
    Err(PluginsError::not_implemented(what))
}

impl Message<ListPlugins> for PluginsActor {
    type Reply = Vec<PluginInfo>;
    async fn handle(&mut self, _: ListPlugins, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        Vec::new()
    }
}

impl Message<GetPlugin> for PluginsActor {
    type Reply = Result<PluginDetail, PluginsError>;
    async fn handle(&mut self, _: GetPlugin, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        todo("GetPlugin")
    }
}

impl Message<CheckPath> for PluginsActor {
    type Reply = Result<CheckReport, PluginsError>;
    async fn handle(&mut self, _: CheckPath, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        todo("CheckPath")
    }
}

impl Message<Install> for PluginsActor {
    type Reply = Result<InstallReport, PluginsError>;
    async fn handle(&mut self, _: Install, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        todo("Install")
    }
}

impl Message<New> for PluginsActor {
    type Reply = Result<ScaffoldReport, PluginsError>;
    async fn handle(&mut self, _: New, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        todo("New")
    }
}

impl Message<Approve> for PluginsActor {
    type Reply = Result<PluginInfo, PluginsError>;
    async fn handle(&mut self, _: Approve, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        todo("Approve")
    }
}

impl Message<RequestApproval> for PluginsActor {
    type Reply = Result<InstallCard, PluginsError>;
    async fn handle(&mut self, _: RequestApproval, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        todo("RequestApproval")
    }
}

impl Message<SetEnabled> for PluginsActor {
    type Reply = Result<PluginInfo, PluginsError>;
    async fn handle(&mut self, _: SetEnabled, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        todo("SetEnabled")
    }
}

impl Message<SetHidden> for PluginsActor {
    type Reply = Result<PluginInfo, PluginsError>;
    async fn handle(&mut self, _: SetHidden, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        todo("SetHidden")
    }
}

impl Message<SetOrder> for PluginsActor {
    type Reply = Result<Vec<PluginInfo>, PluginsError>;
    async fn handle(&mut self, _: SetOrder, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        todo("SetOrder")
    }
}

impl Message<Restart> for PluginsActor {
    type Reply = Result<PluginInfo, PluginsError>;
    async fn handle(&mut self, _: Restart, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        todo("Restart")
    }
}

impl Message<Uninstall> for PluginsActor {
    type Reply = Result<(), PluginsError>;
    async fn handle(&mut self, _: Uninstall, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        todo("Uninstall")
    }
}

impl Message<Action> for PluginsActor {
    type Reply = Result<Value, PluginsError>;
    async fn handle(&mut self, _: Action, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        todo("Action")
    }
}

impl Message<Query> for PluginsActor {
    type Reply = Result<Value, PluginsError>;
    async fn handle(&mut self, _: Query, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        todo("Query")
    }
}

impl Message<Command> for PluginsActor {
    type Reply = Result<CommandReply, PluginsError>;
    async fn handle(&mut self, _: Command, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        todo("Command")
    }
}

impl Message<Catalog> for PluginsActor {
    type Reply = Vec<CatalogEntry>;
    async fn handle(&mut self, _: Catalog, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        Vec::new()
    }
}

impl Message<Surfaces> for PluginsActor {
    type Reply = Vec<SurfaceSnapshot>;
    async fn handle(&mut self, _: Surfaces, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        Vec::new()
    }
}

impl Message<Surface> for PluginsActor {
    type Reply = Result<SurfaceContent, PluginsError>;
    async fn handle(&mut self, _: Surface, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        todo("Surface")
    }
}

impl Message<Logs> for PluginsActor {
    type Reply = Result<LogTail, PluginsError>;
    async fn handle(&mut self, _: Logs, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        todo("Logs")
    }
}
