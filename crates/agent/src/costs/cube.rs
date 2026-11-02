//! The in-memory cube (160 §3.1–3.3): built in full, refreshed by owner
//! revision diff, answered as a full snapshot or as the hours changed since
//! a client's revision.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::Arc;

use super::place::{Piece, place};
use super::{
    CostCells, CostCube, CostCubeCursor, CostCubeMeta, CostModel, CostReason, CostRoot,
    CostRootEntry, Prices, hour_of, hour_text, model_id, model_name, ms,
};
use crate::cost::SpendOwnerKind;
use crate::store::Store;
use crate::store::accounting::NativeGroup;

/// Revisions whose touched hours are kept for deltas.
pub(crate) const LOG_REVS: usize = 1024;
/// More dirty roots than this → full rebuild.
const MAX_DIRTY_ROOTS: usize = 50;
/// A settle can only touch rows created this long before it (max observed
/// reserve → settle latency 540 s; endless 5 × 120 s).
const SETTLE_WINDOW_MS: u64 = 15 * 60 * 1000;
/// Native rows' `via`.
pub(crate) const THEUTH: &str = "theuth";
/// Reported rows' `kind`.
pub(crate) const HARNESS: &str = "harness";

/// Additive measures of one cell.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct M {
    pub(crate) nanos: u64,
    pub(crate) calls: u32,
    pub(crate) failed: u32,
    pub(crate) repriced: u32,
    pub(crate) input: u64,
    pub(crate) output: u64,
    pub(crate) cache_read: u64,
    pub(crate) cache_write: u64,
    pub(crate) cache_saved: u64,
    pub(crate) approx: u64,
}

impl M {
    fn add(&mut self, o: &M) {
        self.nanos += o.nanos;
        self.calls += o.calls;
        self.failed += o.failed;
        self.repriced += o.repriced;
        self.input += o.input;
        self.output += o.output;
        self.cache_read += o.cache_read;
        self.cache_write += o.cache_write;
        self.cache_saved += o.cache_saved;
        self.approx += o.approx;
    }
}

/// Native cell key within one root: hour, model, provider, kind, basis and
/// the unpriced reason (0 = none; else reasons index + 1), which the wire
/// merges away and the meta counts.
type NKey = (u32, u32, u32, u32, u8, u32);
/// Wire cell key: hour, root, model, provider, kind, via, basis.
type CKey = (u32, u32, u32, u32, u32, u32, u8);

/// Append-only string dictionary.
#[derive(Clone, Debug, Default)]
struct Dict {
    items: Vec<String>,
    index: HashMap<String, u32>,
}

impl Dict {
    fn ix(&mut self, s: &str) -> u32 {
        if let Some(&i) = self.index.get(s) {
            return i;
        }
        let i = self.items.len() as u32;
        self.items.push(s.to_owned());
        self.index.insert(s.to_owned(), i);
        i
    }
}

/// The cube state of one epoch.
#[derive(Clone, Debug, Default)]
pub(crate) struct Cube {
    pub(crate) epoch: String,
    pub(crate) rev: u64,
    providers: Dict,
    kinds: Dict,
    vias: Dict,
    reasons: Dict,
    models: Vec<CostModel>,
    model_ix: HashMap<String, u32>,
    /// `(root, revision it last changed at)`, ix = position.
    roots: Vec<(CostRoot, u64)>,
    root_ix: HashMap<String, u32>,
    native: HashMap<u32, BTreeMap<NKey, M>>,
    reported: BTreeMap<CKey, M>,
    pub(crate) pieces: Arc<Vec<Piece>>,
    /// owner → (root, revision) as of the last refresh.
    revisions: HashMap<String, (String, i64)>,
    /// When the last refresh started (unix ms).
    last_refresh_ms: u64,
    /// `(rev, touched hours)`, oldest first.
    pub(crate) log: VecDeque<(u64, Vec<u32>)>,
    pub(crate) price_table: String,
    engine: String,
    pre_ledger: u32,
    built_at_ms: u64,
}

fn now_ms() -> u64 {
    u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0)
}

/// Everything the reported leg reads, fetched without holding the cube.
struct Reported {
    pieces: Vec<Piece>,
    turns: Vec<crate::store::accounting::TurnInput>,
}

async fn fetch_reported(store: &Store, prices: &Prices) -> anyhow::Result<Reported> {
    let meters = store.cost_meters().await?;
    let reports = store.cost_meter_reports().await?;
    let turns = store.cost_turns().await?;
    let pieces = place(&meters, &reports, &turns, prices);
    Ok(Reported { pieces, turns })
}

/// The reported leg's placed pieces, from the store (no cube needed).
pub(crate) async fn pieces(store: &Store, prices: &Prices) -> anyhow::Result<Vec<Piece>> {
    Ok(fetch_reported(store, prices).await?.pieces)
}

impl Cube {
    /// A full build: new epoch, rev 1.
    pub(crate) async fn build(
        store: &Store,
        catalog: &models::Catalog,
        prices: &Prices,
        engine: &str,
    ) -> anyhow::Result<Cube> {
        let started = now_ms();
        let revisions = store.cost_revisions().await?;
        let groups = store.cost_native(None, None).await?;
        let reported = fetch_reported(store, prices).await?;
        let roots = store.cost_roots().await?;
        let pre_ledger = store.cost_pre_ledger_sessions().await?;
        let mut cube = Cube {
            epoch: uuid::Uuid::now_v7().simple().to_string(),
            rev: 1,
            price_table: prices.hash.clone(),
            engine: engine.to_owned(),
            pre_ledger,
            last_refresh_ms: started,
            built_at_ms: now_ms(),
            ..Default::default()
        };
        cube.vias.ix(THEUTH);
        cube.set_roots(roots);
        for g in &groups {
            cube.add_group(g, catalog, prices, None);
        }
        cube.reported = cube.reported_cells(&reported, catalog, prices);
        cube.pieces = Arc::new(reported.pieces);
        cube.revisions = revisions
            .into_iter()
            .map(|(id, root, rev)| (id, (root, rev)))
            .collect();
        Ok(cube)
    }

    /// Incremental refresh (§3.3). `Ok(None)`: a full rebuild is due
    /// (too many dirty roots); `Ok(Some(changed))` otherwise.
    pub(crate) async fn refresh(
        &mut self,
        store: &Store,
        catalog: &models::Catalog,
        prices: &Prices,
    ) -> anyhow::Result<Option<bool>> {
        let started = now_ms();
        let revisions = store.cost_revisions().await?;
        let mut dirty: BTreeSet<String> = BTreeSet::new();
        let mut seen: HashSet<&str> = HashSet::new();
        for (id, root, rev) in &revisions {
            seen.insert(id);
            if self.revisions.get(id) != Some(&(root.clone(), *rev)) {
                dirty.insert(root.clone());
            }
        }
        // An owner that vanished (not since 160 H1; an older engine's erase)
        // re-aggregates its root's whole history.
        let whole: BTreeSet<String> = self
            .revisions
            .iter()
            .filter(|(id, _)| !seen.contains(id.as_str()))
            .map(|(_, (root, _))| root.clone())
            .collect();
        dirty.retain(|r| !whole.contains(r));
        if dirty.len() + whole.len() > MAX_DIRTY_ROOTS {
            return Ok(None);
        }
        // Review 160-ENG #4: also re-read from the oldest call still in flight
        // at the last refresh, so a call settling after the window lands.
        let in_flight: Vec<u32> = ["pending", "dispatched"]
            .iter()
            .filter_map(|r| self.reasons.index.get(*r).map(|i| i + 1))
            .collect();
        let oldest_in_flight = self
            .native
            .values()
            .flat_map(|m| m.keys())
            .filter(|k| in_flight.contains(&k.5))
            .map(|k| k.0)
            .min();
        let from_hour = hour_of(self.last_refresh_ms.saturating_sub(SETTLE_WINDOW_MS))
            .min(oldest_in_flight.unwrap_or(u32::MAX));
        let from_text = super::ms_text(u64::from(from_hour) * 3_600_000);
        let from_text = &from_text[..13];
        let dirty: Vec<String> = dirty.into_iter().collect();
        let fresh = if dirty.is_empty() {
            Vec::new()
        } else {
            store.cost_native(Some(&dirty), Some(from_text)).await?
        };
        let whole_v: Vec<String> = whole.into_iter().collect();
        let fresh_whole = if whole_v.is_empty() {
            Vec::new()
        } else {
            store.cost_native(Some(&whole_v), None).await?
        };
        let reported = fetch_reported(store, prices).await?;
        let roots = store.cost_roots().await?;
        self.pre_ledger = store.cost_pre_ledger_sessions().await?;

        let next = self.rev + 1;
        let mut touched: BTreeSet<u32> = BTreeSet::new();
        // Native: replace (root, hour ≥ from) for dirty roots, every hour for `whole`.
        let changed_roots = self.set_roots_at(roots, next);
        let mut region = |cube: &mut Cube, roots: &[String], groups: &[NativeGroup], from: u32| {
            let ixs: Vec<u32> = roots.iter().map(|r| cube.root(r)).collect();
            let old: Vec<(u32, BTreeMap<NKey, M>)> = ixs
                .iter()
                .map(|&ix| {
                    let map = cube.native.entry(ix).or_default();
                    let tail = map.split_off(&(from, 0, 0, 0, 0, 0));
                    (ix, tail)
                })
                .collect();
            for g in groups {
                cube.add_group(g, catalog, prices, Some(from));
            }
            for (ix, old) in old {
                let new: BTreeMap<NKey, M> = cube
                    .native
                    .get(&ix)
                    .map(|m| {
                        m.range((from, 0, 0, 0, 0, 0)..)
                            .map(|(k, v)| (*k, *v))
                            .collect()
                    })
                    .unwrap_or_default();
                diff_hours(&old, &new, |k| k.0, &mut touched);
            }
        };
        region(self, &dirty, &fresh, from_hour);
        region(self, &whole_v, &fresh_whole, 0);
        // Reported: rebuilt in full.
        let reported_new = self.reported_cells(&reported, catalog, prices);
        diff_hours(&self.reported, &reported_new, |k| k.0, &mut touched);
        self.reported = reported_new;
        self.pieces = Arc::new(reported.pieces);
        self.revisions = revisions
            .into_iter()
            .map(|(id, root, rev)| (id, (root, rev)))
            .collect();
        self.last_refresh_ms = started;
        if touched.is_empty() && !changed_roots {
            return Ok(Some(false));
        }
        self.rev = next;
        self.built_at_ms = now_ms();
        self.log.push_back((next, touched.into_iter().collect()));
        while self.log.len() > LOG_REVS {
            self.log.pop_front();
        }
        Ok(Some(true))
    }

    fn root(&mut self, id: &str) -> u32 {
        if let Some(&ix) = self.root_ix.get(id) {
            return ix;
        }
        // A call whose root has no `cost_owners` root row: attribute it to `other`.
        let ix = self.roots.len() as u32;
        self.roots.push((
            CostRoot {
                id: id.to_owned(),
                title: String::new(),
                owner_kind: SpendOwnerKind::Other,
                owner: "other".to_owned(),
                project: None,
                deleted: true,
                created_at_ms: 0,
            },
            // In the next delta: added while a refresh builds `rev + 1`.
            self.rev + 1,
        ));
        self.root_ix.insert(id.to_owned(), ix);
        ix
    }

    fn model(&mut self, catalog: &models::Catalog, id: &str) -> u32 {
        if let Some(&ix) = self.model_ix.get(id) {
            return ix;
        }
        let ix = self.models.len() as u32;
        let (name, provider) = model_name(catalog, id);
        self.models.push(CostModel {
            id: id.to_owned(),
            name,
            provider,
        });
        self.model_ix.insert(id.to_owned(), ix);
        ix
    }

    fn set_roots(&mut self, rows: Vec<crate::store::accounting::RootRow>) {
        self.set_roots_at(rows, self.rev);
    }

    /// Refresh the root dimension; true when any root was added or changed.
    fn set_roots_at(&mut self, rows: Vec<crate::store::accounting::RootRow>, rev: u64) -> bool {
        let mut changed = false;
        for r in rows {
            let (owner_kind, owner) = r.owner();
            let deleted = r.title.is_none();
            let root = CostRoot {
                id: r.id.clone(),
                title: r.title.clone().unwrap_or_default(),
                owner_kind,
                owner,
                project: if deleted {
                    r.t_project.clone()
                } else {
                    r.project.clone()
                },
                deleted,
                created_at_ms: r
                    .created_at
                    .as_deref()
                    .or(r.t_created_at.as_deref())
                    .and_then(ms)
                    .unwrap_or(0),
            };
            match self.root_ix.get(&r.id) {
                Some(&ix) => {
                    let slot = &mut self.roots[ix as usize];
                    if slot.0 != root {
                        *slot = (root, rev);
                        changed = true;
                    }
                }
                None => {
                    self.root_ix.insert(r.id, self.roots.len() as u32);
                    self.roots.push((root, rev));
                    changed = true;
                }
            }
        }
        changed
    }

    /// Fold one native group into its root's cells (`from`: only hours ≥ it).
    fn add_group(
        &mut self,
        g: &NativeGroup,
        catalog: &models::Catalog,
        prices: &Prices,
        from: Option<u32>,
    ) {
        let Some(hour) = hour_text(&g.hour) else {
            return;
        };
        if from.is_some_and(|f| hour < f) {
            return;
        }
        let root = self.root(&g.root_id);
        let model = self.model(catalog, &g.model);
        let provider = self.providers.ix(&g.provider);
        let kind = self.kinds.ix(&g.kind);
        let rate = prices.rate(&g.model);
        let n = |v: i64| u64::try_from(v).unwrap_or(0);
        let mut m = M {
            nanos: n(g.nanos),
            calls: u32::try_from(g.calls).unwrap_or(0),
            failed: u32::try_from(g.failed).unwrap_or(0),
            input: n(g.input),
            output: n(g.output),
            cache_read: n(g.cache_read),
            cache_write: n(g.cache_write),
            cache_saved: Prices::cache_saved(rate, n(g.cache_read)),
            ..Default::default()
        };
        let (basis, reason) = match g.cls {
            0 => (0, 0),
            2 => (2, 0),
            _ => match (g.solo.is_some(), rate) {
                (true, Some(r)) => {
                    m.nanos = Prices::estimate(r, m.input, m.output, m.cache_read, m.cache_write);
                    m.repriced = m.calls;
                    (2, 0)
                }
                _ => (
                    3,
                    self.reasons.ix(g.reason.as_deref().unwrap_or("unknown")) + 1,
                ),
            },
        };
        self.native
            .entry(root)
            .or_default()
            .entry((hour, model, provider, kind, basis, reason))
            .or_default()
            .add(&m);
    }

    fn reported_cells(
        &mut self,
        r: &Reported,
        catalog: &models::Catalog,
        prices: &Prices,
    ) -> BTreeMap<CKey, M> {
        let mut out: BTreeMap<CKey, M> = BTreeMap::new();
        let kind = self.kinds.ix(HARNESS);
        for p in &r.pieces {
            let root = self.root(&p.root);
            let model = self.model(catalog, &p.model);
            let provider = self.providers.ix(p.provider());
            let via = self.vias.ix(&p.via);
            let c = out
                .entry((hour_of(p.at_ms), root, model, provider, kind, via, 1))
                .or_default();
            c.nanos += p.nanos;
            if !p.exact {
                c.approx += p.nanos;
            }
        }
        for t in &r.turns {
            let Some(at) = ms(&t.at) else { continue };
            let id = t.model.as_deref().map(model_id).unwrap_or("unknown");
            let root = self.root(&t.root_id);
            let model = self.model(catalog, id);
            let provider = self
                .providers
                .ix(id.split_once('/').map_or("unknown", |(p, _)| p));
            let via = self.vias.ix(&t.harness);
            let n = |v: i64| u64::try_from(v).unwrap_or(0);
            let c = out
                .entry((hour_of(at), root, model, provider, kind, via, 1))
                .or_default();
            c.add(&M {
                calls: 1,
                input: n(t.input_tokens),
                output: n(t.output_tokens),
                cache_read: n(t.cache_read_tokens),
                cache_write: n(t.cache_write_tokens),
                cache_saved: Prices::cache_saved(prices.rate(id), n(t.cache_read_tokens)),
                ..Default::default()
            });
        }
        out
    }

    /// Wire cells of `hours` (every hour when `None`), merged by wire key.
    fn cells(&self, hours: Option<&BTreeSet<u32>>) -> BTreeMap<CKey, M> {
        let via = 0; // THEUTH is the first via of every epoch.
        let mut out: BTreeMap<CKey, M> = BTreeMap::new();
        let keep = |h: u32| hours.is_none_or(|s| s.contains(&h));
        for (&root, map) in &self.native {
            match hours {
                None => {
                    for (k, m) in map {
                        out.entry((k.0, root, k.1, k.2, k.3, via, k.4))
                            .or_default()
                            .add(m);
                    }
                }
                Some(set) => {
                    for &h in set {
                        for (k, m) in map.range((h, 0, 0, 0, 0, 0)..(h + 1, 0, 0, 0, 0, 0)) {
                            out.entry((k.0, root, k.1, k.2, k.3, via, k.4))
                                .or_default()
                                .add(m);
                        }
                    }
                }
            }
        }
        for (k, m) in &self.reported {
            if keep(k.0) {
                out.entry(*k).or_default().add(m);
            }
        }
        out
    }

    fn meta(&self) -> CostCubeMeta {
        let mut reasons: BTreeMap<u32, u32> = BTreeMap::new();
        let mut first: Option<u32> = None;
        for map in self.native.values() {
            if let Some((k, _)) = map.iter().next() {
                first = Some(first.map_or(k.0, |f| f.min(k.0)));
            }
            for (k, m) in map {
                if k.5 > 0 {
                    *reasons.entry(k.5 - 1).or_default() += m.calls;
                }
            }
        }
        if let Some((k, _)) = self.reported.iter().next() {
            first = Some(first.map_or(k.0, |f| f.min(k.0)));
        }
        let first_piece = self.pieces.first().map(|p| p.at_ms);
        let mut unpriced: Vec<CostReason> = reasons
            .into_iter()
            .map(|(ix, calls)| CostReason {
                reason: self.reasons.items[ix as usize].clone(),
                calls,
            })
            .collect();
        unpriced.sort_by(|a, b| b.calls.cmp(&a.calls).then_with(|| a.reason.cmp(&b.reason)));
        CostCubeMeta {
            engine: self.engine.clone(),
            first_call_ms: first
                .map(|h| u64::from(h) * 3_600_000)
                .into_iter()
                .chain(first_piece)
                .min(),
            built_at_ms: self.built_at_ms,
            pre_ledger_sessions: self.pre_ledger,
            unpriced,
            price_table: self.price_table.clone(),
        }
    }

    /// `cost_cube(since)` (§3.2): full when `since` is absent, of another
    /// epoch, ahead of this cube or older than the change log; else the hours
    /// changed since `since.rev` (empty when current).
    pub(crate) fn answer(&self, since: Option<&CostCubeCursor>) -> CostCube {
        let delta = since.filter(|s| {
            s.epoch == self.epoch
                && s.rev <= self.rev
                && (s.rev == self.rev
                    || self
                        .log
                        .front()
                        .is_some_and(|(first, _)| *first <= s.rev + 1))
        });
        let (full, hours, roots): (bool, Option<BTreeSet<u32>>, Vec<CostRootEntry>) = match delta {
            None => (
                true,
                None,
                self.roots
                    .iter()
                    .enumerate()
                    .map(|(ix, (root, _))| CostRootEntry {
                        ix: ix as u32,
                        root: root.clone(),
                    })
                    .collect(),
            ),
            Some(s) => (
                false,
                Some(
                    self.log
                        .iter()
                        .filter(|(rev, _)| *rev > s.rev)
                        .flat_map(|(_, h)| h.iter().copied())
                        .collect(),
                ),
                self.roots
                    .iter()
                    .enumerate()
                    .filter(|(_, (_, rev))| *rev > s.rev)
                    .map(|(ix, (root, _))| CostRootEntry {
                        ix: ix as u32,
                        root: root.clone(),
                    })
                    .collect(),
            ),
        };
        let cells = self.cells(hours.as_ref());
        let mut c = CostCells::default();
        for (k, m) in cells {
            c.hour.push(k.0);
            c.root.push(k.1);
            c.model.push(k.2);
            c.provider.push(k.3);
            c.kind.push(k.4);
            c.via.push(k.5);
            c.basis.push(k.6);
            c.nanos.push(m.nanos);
            c.calls.push(m.calls);
            c.failed.push(m.failed);
            c.repriced.push(m.repriced);
            c.input.push(m.input);
            c.output.push(m.output);
            c.cache_read.push(m.cache_read);
            c.cache_write.push(m.cache_write);
            c.cache_saved_nanos.push(m.cache_saved);
            c.approx_nanos.push(m.approx);
        }
        CostCube {
            epoch: self.epoch.clone(),
            rev: self.rev,
            full,
            hours: hours.map(|h| h.into_iter().collect()).unwrap_or_default(),
            providers: self.providers.items.clone(),
            kinds: self.kinds.items.clone(),
            vias: self.vias.items.clone(),
            models: self.models.clone(),
            roots,
            cells: c,
            meta: self.meta(),
        }
    }
}

/// Hours where `old` and `new` differ.
fn diff_hours<K: Ord + Copy, V: PartialEq>(
    old: &BTreeMap<K, V>,
    new: &BTreeMap<K, V>,
    hour: impl Fn(&K) -> u32,
    touched: &mut BTreeSet<u32>,
) {
    for (k, v) in old {
        if new.get(k) != Some(v) {
            touched.insert(hour(k));
        }
    }
    for k in new.keys() {
        if !old.contains_key(k) {
            touched.insert(hour(k));
        }
    }
}

/// The empty cube answered before the first build lands.
pub(crate) fn empty(engine: &str) -> CostCube {
    CostCube {
        full: true,
        meta: CostCubeMeta {
            engine: engine.to_owned(),
            ..Default::default()
        },
        ..Default::default()
    }
}
