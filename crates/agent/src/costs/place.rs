//! Placing remote-meter money in time (160 §2, recon-ledger §2.4). A meter
//! holds one cumulative total; its reports say when the total grew. Per
//! meter, money-conserving (Σ pieces = `attributed_nanos`):
//! 1. report increments `max(0, a_k − running max)` at report k's arrival (exact);
//! 2. the first kept report's amount (spend before the kept window) spread
//!    over the owner's turns up to that report, weighted by catalog-priced
//!    tokens (uniform when none price), else at that report (approximate);
//! 3. no reports: the whole amount over the owner's turns, else at `updated_at`;
//! 4. any residual at the last report (or `updated_at`).
//!
//! Reports are the kept `remote_meter_reports` rows (160 H2) united with the
//! `source_metadata` ring, deduplicated by `(at, amount)`. USD meters only.

use std::collections::{BTreeSet, HashMap};

use super::{Prices, model_id, ms};
use crate::store::accounting::{MeterInput, TurnInput, metadata_reports};

/// One placed piece of reported money.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Piece {
    /// `remote_meters.id`.
    pub(crate) meter: String,
    /// Piece number within the meter.
    pub(crate) k: u32,
    /// Placed time.
    pub(crate) at_ms: u64,
    /// Root session of the meter's owner.
    pub(crate) root: String,
    /// The meter's owner session.
    pub(crate) owner: String,
    /// Harness (`remote_meters.origin`).
    pub(crate) via: String,
    /// Model id (effort suffix dropped; `unknown` when none is known).
    pub(crate) model: String,
    /// USD nanos.
    pub(crate) nanos: u64,
    /// Placed by a report increment (not by turn weights or a fallback).
    pub(crate) exact: bool,
}

impl Piece {
    /// `cost_calls` id: `meter:<meter id>:<k>`.
    pub(crate) fn id(&self) -> String {
        format!("meter:{}:{}", self.meter, self.k)
    }

    /// Provider of the piece: the model's prefix.
    pub(crate) fn provider(&self) -> &str {
        self.model.split_once('/').map_or("unknown", |(p, _)| p)
    }
}

/// Every meter's pieces, sorted by `(at_ms, id)`.
pub(crate) fn place(
    meters: &[MeterInput],
    reports: &[(String, i64, String, String)],
    turns: &[TurnInput],
    prices: &Prices,
) -> Vec<Piece> {
    let mut by_meter: HashMap<&str, Vec<(&str, i64, &str)>> = HashMap::new();
    for (m, amount, cur, at) in reports {
        by_meter
            .entry(m.as_str())
            .or_default()
            .push((at.as_str(), *amount, cur.as_str()));
    }
    let mut by_owner: HashMap<&str, Vec<(u64, &TurnInput)>> = HashMap::new();
    for t in turns {
        if let Some(at) = ms(&t.at) {
            by_owner
                .entry(t.owner_id.as_str())
                .or_default()
                .push((at, t));
        }
    }
    for v in by_owner.values_mut() {
        v.sort_by_key(|(at, _)| *at);
    }
    let mut out = Vec::new();
    for m in meters {
        if m.currency != "USD" {
            continue;
        }
        let attributed = u64::try_from(m.attributed_nanos).unwrap_or(0);
        if attributed == 0 {
            continue;
        }
        let meta = metadata_reports(m.source_metadata.as_deref());
        let mut rs: BTreeSet<(String, i64)> = meta
            .iter()
            .filter(|(_, cur, _)| *cur == m.currency)
            .map(|(a, _, at)| (at.clone(), *a))
            .collect();
        for (at, a, cur) in by_meter.get(m.id.as_str()).into_iter().flatten() {
            if *cur == m.currency {
                rs.insert(((*at).to_owned(), *a));
            }
        }
        let rs: Vec<(u64, u64)> = rs
            .into_iter()
            .filter_map(|(at, a)| {
                Some((ms(&at)?, u64::try_from(a - m.baseline_nanos).unwrap_or(0)))
            })
            .collect();
        let owner_turns = by_owner
            .get(m.owner_id.as_str())
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let fallback_model = m
            .harness_model
            .as_deref()
            .map(model_id)
            .unwrap_or("unknown");
        let model_at = |at: u64| -> String {
            owner_turns
                .iter()
                .rev()
                .find(|(t, _)| *t <= at)
                .or(owner_turns.first())
                .and_then(|(_, t)| t.model.as_deref())
                .map_or(fallback_model, model_id)
                .to_owned()
        };
        // (at, model, nanos, exact)
        let mut raw: Vec<(u64, String, u64, bool)> = Vec::new();
        let updated = ms(&m.updated_at).unwrap_or(0);
        let last_at = rs.last().map_or(updated, |(at, _)| *at);
        let spread = |amount: u64, upto: Option<u64>, at: u64, raw: &mut Vec<_>| {
            let ts: Vec<&(u64, &TurnInput)> = owner_turns
                .iter()
                .filter(|(t, _)| upto.is_none_or(|u| *t <= u))
                .collect();
            if ts.is_empty() {
                raw.push((at, model_at(at), amount, false));
                return;
            }
            let w: Vec<u64> = ts
                .iter()
                .map(|(_, t)| {
                    let model = t.model.as_deref().map(model_id).unwrap_or(fallback_model);
                    prices.rate(model).map_or(0, |r| {
                        Prices::estimate(
                            r,
                            u64::try_from(t.input_tokens).unwrap_or(0),
                            u64::try_from(t.output_tokens).unwrap_or(0),
                            u64::try_from(t.cache_read_tokens).unwrap_or(0),
                            u64::try_from(t.cache_write_tokens).unwrap_or(0),
                        )
                    })
                })
                .collect();
            let total: u128 = w.iter().map(|&x| u128::from(x)).sum();
            let mut shares: Vec<u64> = if total == 0 {
                vec![amount / ts.len() as u64; ts.len()]
            } else {
                w.iter()
                    .map(|&x| (u128::from(amount) * u128::from(x) / total) as u64)
                    .collect()
            };
            // Rounding residue to the heaviest turn (the last one when uniform).
            let rest = amount - shares.iter().sum::<u64>();
            let heavy = (0..ts.len()).max_by_key(|&i| (w[i], i)).unwrap_or(0);
            shares[heavy] += rest;
            for ((at, t), share) in ts.into_iter().zip(shares) {
                if share > 0 {
                    let model = t.model.as_deref().map(model_id).unwrap_or(fallback_model);
                    raw.push((*at, model.to_owned(), share, false));
                }
            }
        };
        if let Some(&(first_at, a0)) = rs.first() {
            let mut running = a0;
            for &(at, a) in &rs[1..] {
                if a > running {
                    raw.push((at, model_at(at), a - running, true));
                    running = a;
                }
            }
            if a0 > 0 {
                spread(a0, Some(first_at), first_at, &mut raw);
            }
        } else {
            spread(attributed, None, updated, &mut raw);
        }
        // Conserve: cap at `attributed`, then the residual at the last report.
        let mut left = attributed;
        raw.retain_mut(|p| {
            p.2 = p.2.min(left);
            left -= p.2;
            p.2 > 0
        });
        if left > 0 {
            raw.push((last_at, model_at(last_at), left, false));
        }
        raw.sort_by_key(|p| p.0);
        out.extend(
            raw.into_iter()
                .enumerate()
                .map(|(k, (at_ms, model, nanos, exact))| Piece {
                    meter: m.id.clone(),
                    k: k as u32,
                    at_ms,
                    root: m.root_id.clone(),
                    owner: m.owner_id.clone(),
                    via: m.origin.clone(),
                    model,
                    nanos,
                    exact,
                }),
        );
    }
    out.sort_by_cached_key(|p| (p.at_ms, p.id()));
    out
}
