//! The engine's cost cube service (160 §3.3): a background task owns the
//! cube, builds it at start (never on the RPC path), refreshes it on
//! ledger events (1 s trailing debounce) and every 15 s (endless settles
//! publish nothing), and publishes `CostCubeChanged` after a change. RPC
//! answers read the latest snapshot from memory.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use parking_lot::RwLock;
use tokio::sync::Notify;

use super::cube::{self, Cube};
use super::{
    CostCallsPage, CostCallsQuery, CostCube, CostCubeChanged, CostCubeCursor, Piece, Prices,
};
use crate::AgentEvent;
use crate::store::Store;

/// Trailing debounce of ledger events.
const DEBOUNCE: Duration = Duration::from_secs(1);
/// Longest a burst of events may defer a refresh.
const DEBOUNCE_MAX: Duration = Duration::from_secs(3);
/// Refresh without an event (endless settles publish nothing).
const TICK: Duration = Duration::from_secs(15);

struct Inner {
    store: Store,
    engine: String,
    /// `(catalog, its prices)`; replaced when the catalog reloads.
    catalog: RwLock<(Arc<models::Catalog>, Arc<Prices>)>,
    /// The latest cube; `None` until the first build lands.
    cube: RwLock<Option<Arc<Cube>>>,
    wake: Notify,
}

/// Cheap to clone; one per engine (durable store only).
#[derive(Clone)]
pub struct CostService {
    inner: Arc<Inner>,
}

/// The engine's service, set once by `supervisor::start`; RPCs read it
/// without a trip through the supervisor's mailbox (D8: never wait).
static ENGINE: OnceLock<CostService> = OnceLock::new();

impl CostService {
    /// A service with no cube yet; [`Self::spawn`] starts its task.
    pub fn new(store: Store, catalog: Arc<models::Catalog>, engine: String) -> Self {
        let prices = Arc::new(Prices::new(&catalog));
        Self {
            inner: Arc::new(Inner {
                store,
                engine,
                catalog: RwLock::new((catalog, prices)),
                cube: RwLock::new(None),
                wake: Notify::new(),
            }),
        }
    }

    /// Make this the engine's service ([`Self::engine`]); a second call is ignored.
    pub fn install(&self) {
        let _ = ENGINE.set(self.clone());
    }

    /// The engine's service; `None` before `supervisor::start` ran.
    pub fn engine() -> Option<&'static CostService> {
        ENGINE.get()
    }

    /// Start the background task: full build, then refresh on events and
    /// ticks until `shutdown`; `publish` receives every `CostCubeChanged`.
    pub fn spawn<F, Fut>(&self, shutdown: tokio_util::sync::CancellationToken, publish: F)
    where
        F: Fn(CostCubeChanged) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send,
    {
        let me = self.clone();
        tokio::spawn(async move {
            loop {
                match me.step().await {
                    Ok(Some(ev)) => publish(ev).await,
                    Ok(None) => {}
                    Err(e) => tracing::warn!("cost cube: {e:#}"),
                }
                tokio::select! {
                    () = shutdown.cancelled() => return,
                    () = me.inner.wake.notified() => {
                        let until = tokio::time::Instant::now() + DEBOUNCE_MAX;
                        loop {
                            tokio::select! {
                                () = me.inner.wake.notified() => {}
                                () = tokio::time::sleep(DEBOUNCE) => break,
                                () = tokio::time::sleep_until(until) => break,
                            }
                        }
                    }
                    () = tokio::time::sleep(TICK) => {}
                }
            }
        });
    }

    /// One build or refresh; the event to publish when the cube moved.
    pub(crate) async fn step(&self) -> anyhow::Result<Option<CostCubeChanged>> {
        let (catalog, prices) = self.inner.catalog.read().clone();
        let current = self.inner.cube.read().clone();
        let next = match current {
            Some(c) if c.price_table == prices.hash => {
                let mut c = (*c).clone();
                match c.refresh(&self.inner.store, &catalog, &prices).await? {
                    // Review 160-ENG #7: nothing for clients, but keep the
                    // advanced revisions / window so the next tick starts there.
                    Some(false) => {
                        *self.inner.cube.write() = Some(Arc::new(c));
                        None
                    }
                    Some(true) => Some(c),
                    None => Some(
                        Cube::build(&self.inner.store, &catalog, &prices, &self.inner.engine)
                            .await?,
                    ),
                }
            }
            _ => Some(Cube::build(&self.inner.store, &catalog, &prices, &self.inner.engine).await?),
        };
        Ok(next.map(|c| {
            let ev = CostCubeChanged {
                epoch: c.epoch.clone(),
                rev: c.rev,
            };
            *self.inner.cube.write() = Some(Arc::new(c));
            ev
        }))
    }

    /// Refresh soon if `ev` can move the ledger or the root dimension.
    pub fn observe(&self, ev: &AgentEvent) {
        if matches!(
            ev,
            AgentEvent::ModelUsageChanged { .. }
                | AgentEvent::SessionDeleted { .. }
                | AgentEvent::TitleChanged { .. }
                | AgentEvent::SessionMetadataChanged { .. }
                | AgentEvent::SubagentSpawned { .. }
        ) {
            self.inner.wake.notify_one();
        }
    }

    /// The catalog reloaded: a changed price table rebuilds the cube.
    pub fn set_catalog(&self, catalog: Arc<models::Catalog>) {
        let prices = Arc::new(Prices::new(&catalog));
        *self.inner.catalog.write() = (catalog, prices);
        self.inner.wake.notify_one();
    }

    /// `cost_cube(since)` from memory; the empty full cube before the first build.
    pub fn cube(&self, since: Option<&CostCubeCursor>) -> CostCube {
        match self.inner.cube.read().clone() {
            Some(c) => c.answer(since),
            None => cube::empty(&self.inner.engine),
        }
    }

    /// The reported leg's placed pieces: the cube's, else read now.
    pub(crate) async fn pieces(&self) -> anyhow::Result<Arc<Vec<Piece>>> {
        if let Some(c) = self.inner.cube.read().clone() {
            return Ok(c.pieces.clone());
        }
        let prices = self.inner.catalog.read().1.clone();
        Ok(Arc::new(cube::pieces(&self.inner.store, &prices).await?))
    }

    /// `cost_calls(query)` (§4).
    pub async fn calls(&self, q: &CostCallsQuery) -> anyhow::Result<CostCallsPage> {
        let pieces = self.pieces().await?;
        let prices = self.inner.catalog.read().1.clone();
        let cube = self.inner.cube.read().clone();
        super::calls::page(&self.inner.store, &prices, &pieces, q, cube.as_deref()).await
    }

    /// `spend_report` with the reported leg (160 H5).
    pub async fn spend_report(
        &self,
        now_ms: u64,
        days: u32,
    ) -> anyhow::Result<crate::cost::SpendReport> {
        let pieces = self.pieces().await?;
        self.inner.store.spend_report(now_ms, days, &pieces).await
    }
}
