//! `cost_calls` (160 §4): the ledger's rows newest first, keyset-paged on
//! `(at_ms, id)` — native attempts from the store, reported money as the
//! cube's placed pieces, interleaved by time. Money per row is exactly what
//! the cube sums, so Σ over all pages = the cube's total for the filter.

use super::cube::{Cube, HARNESS, THEUTH};
use super::{CostBasis, CostCall, CostCallsCursor, CostCallsPage, CostCallsQuery, Piece, Prices};
use super::{ms, ms_text};
use crate::store::Store;
use crate::store::accounting::{CallDriver, CallFilter, CallRowOut};

/// Default and maximum page sizes.
const DEFAULT_LIMIT: u32 = 200;
const MAX_LIMIT: u32 = 1000;
/// Rows of one millisecond fetched to complete a page's last bucket.
const SAME_MS: u32 = 100_000;

fn wants(list: &Option<Vec<String>>, v: &str) -> bool {
    list.as_ref().is_none_or(|l| l.iter().any(|x| x == v))
}

fn key(c: &CostCall) -> (u64, &str) {
    (c.at_ms, c.id.as_str())
}

fn before(c: &CostCall, cursor: Option<&CostCallsCursor>) -> bool {
    cursor.is_none_or(|k| key(c) < (k.at_ms, k.id.as_str()))
}

fn usd(n: Option<i64>, cur: Option<&str>) -> u64 {
    if cur == Some("USD") {
        n.and_then(|n| u64::try_from(n).ok()).unwrap_or(0)
    } else {
        0
    }
}

fn native(r: CallRowOut, prices: &Prices) -> Option<CostCall> {
    let at_ms = ms(&r.created_at)?;
    let t = |v: Option<i64>| v.and_then(|v| u64::try_from(v).ok());
    let (input, output, cache_read, cache_write) = (
        t(r.input_tokens),
        t(r.output_tokens),
        t(r.cache_read_tokens),
        t(r.cache_write_tokens),
    );
    let model = r.requested_model.clone().unwrap_or_default();
    let (basis, nanos, repriced) = match r.basis {
        0 => (
            CostBasis::Known,
            Some(usd(r.returned_nanos, r.returned_currency.as_deref())),
            false,
        ),
        2 if r.estimated_nanos.is_some() => (
            CostBasis::Estimated,
            Some(usd(r.estimated_nanos, r.estimated_currency.as_deref())),
            false,
        ),
        2 => (
            CostBasis::Estimated,
            prices.rate(&model).map(|rate| {
                Prices::estimate(
                    rate,
                    input.unwrap_or(0),
                    output.unwrap_or(0),
                    cache_read.unwrap_or(0),
                    cache_write.unwrap_or(0),
                )
            }),
            true,
        ),
        _ => (CostBasis::Unpriced, None, false),
    };
    let reason = (basis == CostBasis::Unpriced)
        .then(|| r.unknown_reason.clone().unwrap_or_else(|| r.status.clone()));
    Some(CostCall {
        id: r.call_id,
        at_ms,
        root: r.root_id,
        session: r.owner_id,
        via: THEUTH.to_owned(),
        provider: r.provider,
        model: r.requested_model,
        kind: r.kind,
        status: r.status,
        basis,
        nanos,
        input,
        output,
        cache_read,
        cache_write,
        repriced,
        reason,
    })
}

fn reported(p: &Piece) -> CostCall {
    CostCall {
        id: p.id(),
        at_ms: p.at_ms,
        root: p.root.clone(),
        session: p.owner.clone(),
        via: p.via.clone(),
        provider: Some(p.provider().to_owned()),
        model: Some(p.model.clone()),
        kind: HARNESS.to_owned(),
        status: "reported".to_owned(),
        basis: CostBasis::Reported,
        nanos: Some(p.nanos),
        input: None,
        output: None,
        cache_read: None,
        cache_write: None,
        repriced: false,
        reason: None,
    }
}

/// One page for `q`; `pieces` are the reported leg sorted by `(at_ms, id)`;
/// `cube` (when built) picks the index each native query runs on (160 R2).
pub(crate) async fn page(
    store: &Store,
    prices: &Prices,
    pieces: &[Piece],
    q: &CostCallsQuery,
    cube: Option<&Cube>,
) -> anyhow::Result<CostCallsPage> {
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT) as usize;
    let cursor = q.before.as_ref();
    let base = |b: CostBasis| q.bases.as_ref().is_none_or(|l| l.contains(&b));
    let in_range = |at: u64| q.from_ms.is_none_or(|f| at >= f) && q.to_ms.is_none_or(|t| at < t);

    // Native rows: text keyset on `created_at`, completed per millisecond.
    let mut rows: Vec<CostCall> = Vec::new();
    let native_bases = [CostBasis::Known, CostBasis::Estimated, CostBasis::Unpriced];
    if wants(&q.vias, THEUTH) && native_bases.into_iter().any(base) {
        let mut filter = CallFilter {
            roots: q.roots.as_deref(),
            providers: q.providers.as_deref(),
            models: q.models.as_deref(),
            kinds: q.kinds.as_deref(),
            bases: q.bases.as_deref(),
            priced: &prices.priced_json,
            driver: CallDriver::Time,
        };
        let lower = q.from_ms.map(ms_text);
        let mut upper_ms = match (cursor.map(|c| c.at_ms.saturating_add(1)), q.to_ms) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        loop {
            let upper = upper_ms.map_or_else(|| "9999".to_owned(), ms_text);
            let want = (limit + 1 - rows.len()) as u32;
            filter.driver = cube.map_or(CallDriver::Time, |c| c.call_driver(q, upper_ms, want));
            let got = store
                .cost_call_rows(&filter, &upper, lower.as_deref(), want)
                .await?;
            let more = got.len() as u32 == want;
            let mut batch: Vec<CostCall> =
                got.into_iter().filter_map(|r| native(r, prices)).collect();
            let Some(last) = batch.iter().map(|c| c.at_ms).min() else {
                break;
            };
            if more {
                // Every row of the oldest millisecond, so it is never split
                // (a one-millisecond window: the time index is always right).
                let edge_filter = CallFilter {
                    driver: CallDriver::Time,
                    ..filter
                };
                let edge = store
                    .cost_call_rows(
                        &edge_filter,
                        &ms_text(last + 1),
                        Some(&ms_text(last)),
                        SAME_MS,
                    )
                    .await?;
                batch.retain(|c| c.at_ms != last);
                batch.extend(
                    edge.into_iter()
                        .filter_map(|r| native(r, prices))
                        .filter(|c| c.at_ms == last),
                );
            }
            rows.extend(
                batch
                    .into_iter()
                    .filter(|c| before(c, cursor) && in_range(c.at_ms)),
            );
            if !more || rows.len() > limit || last == 0 {
                break;
            }
            upper_ms = Some(last);
        }
    }

    // Reported pieces matching the filter, newest first.
    if wants(&q.kinds, HARNESS) && base(CostBasis::Reported) {
        rows.extend(
            pieces
                .iter()
                .rev()
                .filter(|p| {
                    in_range(p.at_ms)
                        && wants(&q.roots, &p.root)
                        && wants(&q.providers, p.provider())
                        && wants(&q.models, &p.model)
                        && wants(&q.vias, &p.via)
                })
                .map(reported)
                .filter(|c| before(c, cursor))
                .take(limit + 1),
        );
    }
    rows.sort_by(|a, b| key(b).cmp(&key(a)));
    rows.dedup_by(|a, b| a.id == b.id);
    let next = (rows.len() > limit).then(|| {
        let c = &rows[limit - 1];
        CostCallsCursor {
            at_ms: c.at_ms,
            id: c.id.clone(),
        }
    });
    rows.truncate(limit);
    Ok(CostCallsPage { calls: rows, next })
}
