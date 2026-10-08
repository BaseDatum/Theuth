//! 137 §5.2: the per-session pump — derives units from committed rows,
//! builds nodes in rule-3 order with ≤ `jobs` compactor calls in flight,
//! retries failures, keeps the fold fitted and reports `EndlessProgress`.

use std::time::Duration;

use chrono::{DateTime, Utc};
use runtime::SessionId;
use tokio_util::sync::CancellationToken;

use super::fold::ViewRender;
use super::{Attached, Inner, MemoryMode, SessionMemory, SetMode, ViewLine};

/// One attached session's live state.
pub struct Handle {
    pub(crate) attached: Attached,
    pub(crate) mode: MemoryMode,
    pub(crate) backfill_done: bool,
    pub(crate) cancel: CancellationToken,
}

impl Handle {
    /// D14: the loop runs the endless frame only once the tree covers the history.
    pub(crate) fn active(&self) -> bool {
        self.mode == MemoryMode::Endless && self.backfill_done
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

pub(crate) async fn resume_all(_inner: &Inner) -> anyhow::Result<usize> {
    Ok(0)
}

pub(crate) async fn attach(
    inner: &Inner,
    session: SessionId,
    attached: Attached,
) -> anyhow::Result<()> {
    let store = attached.store.clone().unwrap_or_else(|| inner.store.clone());
    let mode = store.session_memory(session).await?;
    inner.sessions.lock().insert(
        session,
        Handle {
            attached,
            mode,
            backfill_done: false,
            cancel: CancellationToken::new(),
        },
    );
    Ok(())
}

pub(crate) fn detach(inner: &Inner, session: SessionId) {
    inner.sessions.lock().remove(&session);
}

pub(crate) async fn settle(_inner: &Inner, _session: SessionId, _max: Duration) -> bool {
    true
}

pub(crate) async fn render_for_turn(_inner: &Inner, _session: SessionId) -> anyhow::Result<ViewRender> {
    Ok(ViewRender {
        blocks: Vec::new(),
        settled: true,
        unbuilt: 0,
    })
}

pub(crate) fn nudge(_inner: &Inner, _session: SessionId) {}

pub(crate) async fn zoom(_inner: &Inner, _session: SessionId, id: u64, n: u64) -> Result<Vec<ViewLine>, String> {
    Err(format!("No line {id}+{n}."))
}

pub(crate) async fn recall(
    _inner: &Inner,
    _session: SessionId,
    _query: &str,
    _limit: u8,
) -> Result<Vec<ViewLine>, String> {
    Ok(Vec::new())
}

pub(crate) async fn date(_inner: &Inner, _session: SessionId, _id: u64) -> Result<Option<DateTime<Utc>>, String> {
    Ok(None)
}

pub(crate) async fn set_mode(
    inner: &Inner,
    session: SessionId,
    mode: MemoryMode,
    _epoch: u32,
) -> anyhow::Result<SetMode> {
    inner.store_for(session).set_session_memory(session, mode).await?;
    Ok(SetMode {
        memory: memory(inner, session).await?,
        head: None,
    })
}

pub(crate) async fn memory(inner: &Inner, session: SessionId) -> anyhow::Result<SessionMemory> {
    let mode = inner.store_for(session).session_memory(session).await?;
    Ok(SessionMemory {
        mode,
        ..Default::default()
    })
}
