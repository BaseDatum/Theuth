//! Design doc 109 §3.3 step 6: the agent's [`tools::Journal`] over the
//! session store, installed on the tool registry at supervisor start
//! (`// 109: journal seam`). Every method is one store write; a failure is
//! logged, never surfaced to the tool that triggered it.

use std::sync::Arc;

use futures::future::BoxFuture;
use runtime::SessionId;
use tools::{JobRow, Journal, PtyScreenRow, QuestionRow, ToolOutput, WaitRow};

use crate::store::Store;

/// The store half of the seam.
pub struct StoreJournal(pub Arc<Store>);

fn log(what: &str, r: anyhow::Result<impl Sized>) {
    if let Err(e) = r {
        tracing::error!("109 journal: {what}: {e:#}");
    }
}

impl Journal for StoreJournal {
    fn call_started(&self, session: SessionId, call_id: &str) -> BoxFuture<'_, ()> {
        let call_id = call_id.to_owned();
        Box::pin(async move {
            log("call_started", self.0.call_started(session, &call_id).await);
        })
    }

    fn call_finished(
        &self,
        session: SessionId,
        call_id: &str,
        out: &ToolOutput,
    ) -> BoxFuture<'_, ()> {
        let call_id = call_id.to_owned();
        let out = out.clone();
        Box::pin(async move {
            log(
                "call_finished",
                self.0.call_finished(session, &call_id, &out).await,
            );
        })
    }

    fn question(&self, q: QuestionRow) -> BoxFuture<'_, ()> {
        Box::pin(async move { log("question", self.0.question_put(&q).await) })
    }

    fn wait(&self, w: Option<WaitRow>, session: SessionId, call_id: &str) -> BoxFuture<'_, ()> {
        let call_id = call_id.to_owned();
        Box::pin(async move {
            match w {
                Some(w) => log("wait_put", self.0.wait_put(&w).await),
                None => log("wait_clear", self.0.wait_clear(session, &call_id).await),
            }
        })
    }

    fn job(&self, mut j: JobRow) -> BoxFuture<'_, ()> {
        // 139 §10 (D14): the store knows the engine; the tools crate does not.
        j.engine = self.0.engine().to_owned();
        Box::pin(async move { log("job", self.0.job_put(&j).await) })
    }

    fn pty_screen(&self, s: PtyScreenRow) -> BoxFuture<'_, ()> {
        Box::pin(async move { log("pty_screen", self.0.pty_screen_put(&s).await) })
    }

    fn approvals(&self, session: SessionId, pin: Option<bool>) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            log(
                "approvals",
                self.0.set_session_approvals(session, pin).await,
            )
        })
    }
}
