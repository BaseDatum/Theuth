use std::fmt::Display;
use crate as rpc_core;

use jsonrpsee::{Extensions, core::RpcResult, types::ErrorObjectOwned};
use kameo::{Actor, actor::ActorRef, error::SendError, message::Message, reply::Reply};

/// `ask` failed (handler error, actor stopped mid-call).
pub const ASK_FAILED: i32 = -32000;
/// The named actor is not registered.
pub const NOT_RUNNING: i32 = -32001;
/// The call reached the method without an authenticated connection behind it.
pub const UNAUTHORIZED: i32 = -32002;

pub fn lookup<A: Actor>(name: &str) -> RpcResult<ActorRef<A>> {
    match ActorRef::<A>::lookup(name) {
        Ok(Some(a)) => Ok(a),
        Ok(None) => Err(ErrorObjectOwned::owned(
            NOT_RUNNING,
            format!("{name} not running"),
            None::<()>,
        )),
        Err(e) => Err(ErrorObjectOwned::owned(
            ASK_FAILED,
            e.to_string(),
            None::<()>,
        )),
    }
}

/// A `dreaming::rpc` / `autoresearch::rpc_impl` error → JSON-RPC: "not running" is `NOT_RUNNING`, anything else `ASK_FAILED`.
pub fn dreaming_err(e: String) -> ErrorObjectOwned {
    let code = if e.ends_with("is not running") {
        NOT_RUNNING
    } else {
        ASK_FAILED
    };
    ErrorObjectOwned::owned(code, e, None::<()>)
}

/// Look up `name`, `ask` it `msg`, map every failure to a JSON-RPC error.
pub async fn ask<A, M>(name: &str, msg: M) -> RpcResult<<A::Reply as Reply>::Ok>
where
    A: Actor + Message<M>,
    M: Send + 'static,
    <A::Reply as Reply>::Error: Display,
{
    send(&lookup::<A>(name)?, msg).await
}

/// `ask` a resolved actor; handler errors and a stopped actor become `ASK_FAILED`.
pub async fn send<A, M>(actor: &ActorRef<A>, msg: M) -> RpcResult<<A::Reply as Reply>::Ok>
where
    A: Actor + Message<M>,
    M: Send + 'static,
    <A::Reply as Reply>::Error: Display,
{
    actor.ask(msg).await.map_err(|e: SendError<M, _>| {
        ErrorObjectOwned::owned(ASK_FAILED, e.to_string(), None::<()>)
    })
}

/// The authenticated connection behind a `with_extensions` call. Both
/// listeners (local socket, link conn) put one in every admitted request;
/// its absence means nothing authenticated the call — never "local".
pub fn peer(ext: &Extensions) -> RpcResult<rpc_core::RpcPeer> {
    ext.get::<rpc_core::RpcPeer>().cloned().ok_or_else(|| {
        ErrorObjectOwned::owned(UNAUTHORIZED, "no authenticated connection", None::<()>)
    })
}

/// Owner-only methods (59 §8: `doc_members`, links): a guest peer is refused before the actor sees it.
pub fn owner(ext: &Extensions) -> RpcResult<rpc_core::RpcPeer> {
    let p = peer(ext)?;
    if p.guest().is_some() {
        return Err(ErrorObjectOwned::owned(
            UNAUTHORIZED,
            "owner only",
            None::<()>,
        ));
    }
    Ok(p)
}

/// An `ASK_FAILED` (`-32000`) error with `msg`.
pub fn failed(msg: impl Into<String>) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(ASK_FAILED, msg.into(), None::<()>)
}

/// 158 §15.2: a wave module's `anyhow` error as `ASK_FAILED` (the stubs' "not built yet" too).
pub fn stub_err(e: anyhow::Error) -> ErrorObjectOwned {
    failed(format!("{e:#}"))
}
