//! Log pruning (68 §6, 67 loose ends 2.4): every [`PRUNE_SECS`] the log
//! drops rows every known peer has acked — `seq ≤ min(session_cursors(me,
//! peer))` — and, with no known peer, everything but the head row: a peer
//! without a cursor bootstraps from a snapshot built on demand from the
//! live tables, so a solo engine's log is dead weight. The head row always
//! stays: `Store::open` seeds `seq` and the clock from `MAX(seq)`/`MAX(hlc)`,
//! and a restart at seq 1 would collide with every peer's cursor. Orphaned
//! rows are kept `orphan_keep_days` (68 §4.3) and then dropped.

use std::sync::Arc;
use std::time::Duration;

use agent::store::Store;
use replica::PeerId;
use tokio::task::JoinHandle;

use crate::sync::Node;

/// Prune period.
pub const PRUNE_SECS: u64 = 60;

/// The timer task.
pub fn start(node: Arc<Node>, orphan_keep_days: u64) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(PRUNE_SECS)).await;
            match tick(&node.store, &node.me, orphan_keep_days).await {
                Ok(0) => {}
                Ok(n) => tracing::debug!("sessions: pruned {n} log rows"),
                Err(e) => tracing::warn!("sessions: prune: {e:#}"),
            }
        }
    })
}

/// One prune pass; returns the rows deleted.
pub async fn tick(store: &Store, me: &PeerId, orphan_keep_days: u64) -> anyhow::Result<u64> {
    let head = store.log_seq();
    let acked = store
        .cursors()
        .await?
        .into_iter()
        .filter(|c| c.origin == me.0 && !c.peer.is_empty())
        .map(|c| c.seq)
        .min();
    // `prune_log_below(n)` deletes `seq < n`; never past the head.
    let below = acked.map_or(head, |a| (a + 1).min(head));
    let mut n = store.prune_log_below(below).await?;
    let keep_ms = orphan_keep_days.saturating_mul(24 * 60 * 60 * 1000);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    n += store.prune_orphans_before(now.saturating_sub(keep_ms)).await?;
    Ok(n)
}
