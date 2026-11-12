//! Bounded replication waits for remotely created session rows.

use agent::Agents;
use runtime::SessionId;
use tokio::time::Instant;
use rpc_core::{ask, Server};

/// Wait briefly for a session created by another engine to replicate here.
pub trait RpcSessionWaitExt {
    /// Wait up to the initial deadline, then monitor late arrival in the background.
    fn wait_for_row<'a>(&'a self, session: SessionId, engine: &'a str)-> impl Future<Output = ()> + Send + 'a;
}

impl RpcSessionWaitExt for Server {
    /// 183 §5.6: wait, at most [`CREATED_ROW_WAIT`], for a session `engine`
    /// just created to replicate here (§13 row 11: on timeout the caller
    /// answers anyway; calls for it are forwarded by id meanwhile). A row
    /// still missing after [`CREATED_ROW_LAG`] is a WARN: normal replication
    /// takes a few seconds and logs nothing.
    async fn wait_for_row(&self, session: SessionId, engine: &str) {
        let Ok(store) = ask::<Agents, _>(agent::AGENT_ACTOR, agent::DurableStore).await else {
            return;
        };
        if row_within(&store, session, CREATED_ROW_WAIT).await {
            return;
        }
        let engine = engine.to_owned();
        tokio::spawn(async move {
            if !row_within(&store, session, CREATED_ROW_LAG - CREATED_ROW_WAIT).await {
                tracing::warn!(
                    target: "rpc::api",
                    "sessions: new row {session} from {engine} not here after {}s; forwarding by id",
                    CREATED_ROW_LAG.as_secs()
                );
            }
        });
    }

}

/// `session`'s row is in `store` within `wait` (polled every 100 ms).
async fn row_within(
    store: &agent::store::Store,
    session: SessionId,
    wait: std::time::Duration,
) -> bool {
    let deadline = Instant::now() + wait;
    loop {
        if store.session(session).await.ok().flatten().is_some() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

/// 183 §5.6: how long `create_session{engine}` waits for the new row before it
/// answers (the chat works by id meanwhile; the wait only spares a lookup).
const CREATED_ROW_WAIT: std::time::Duration = std::time::Duration::from_secs(1);

/// 183 §5.6: a new row from another engine not here after this long is a WARN.
const CREATED_ROW_LAG: std::time::Duration = std::time::Duration::from_secs(6);
