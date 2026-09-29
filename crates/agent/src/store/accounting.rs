//! Accounting ledger (design doc 51 §6.1–6.5): `cost_owners` lineage,
//! `model_calls` native attempts and `remote_meters` cumulative meters on the
//! session store's pool. Every mutation bumps the affected owner's revision
//! and every live ancestor's in the same transaction and hands them back so
//! the caller can publish `ModelUsageChanged`. `session_cost` reads only these
//! rows, never transcript state.

use std::collections::{BTreeMap, HashMap};

use runtime::{SessionId, TurnId};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqliteConnection};
use uuid::Uuid;

use super::Store;
use crate::cost::{
    CallId, CallKind, CallStatus, ChildCost, CostBucket, CostObservation, CostScope, Coverage,
    MeterUpdate, Money, Provenance, SessionCost, add_by_currency,
};

/// Ancestry/subtree walks stop here: deeper is a cycle in persisted topology.
pub(super) const LINEAGE_MAX: i64 = 64;

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn parse_id(s: &str) -> anyhow::Result<SessionId> {
    s.parse()
        .map_err(|e| anyhow::anyhow!("bad session id {s:?}: {e}"))
}

/// Bump `owner` and every live ancestor; returns `(owner, new revision)` for
/// each row touched (tombstoned rows are skipped, so a settlement landing on
/// a deleted child still invalidates its surviving ancestors).
pub(super) async fn bump_in(
    conn: &mut SqliteConnection,
    owner: &str,
) -> anyhow::Result<Vec<(SessionId, u64)>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "WITH RECURSIVE up(id, parent_id, deleted_at, depth) AS (
             SELECT id, parent_id, deleted_at, 0 FROM cost_owners WHERE id = ?
             UNION ALL
             SELECT c.id, c.parent_id, c.deleted_at, up.depth + 1
             FROM cost_owners c JOIN up ON c.id = up.parent_id WHERE up.depth < ?
         )
         UPDATE cost_owners SET revision = revision + 1
         WHERE id IN (SELECT id FROM up WHERE deleted_at IS NULL)
         RETURNING id, revision",
    )
    .bind(owner)
    .bind(LINEAGE_MAX)
    .fetch_all(conn)
    .await?;
    rows.into_iter()
        .map(|(id, rev)| Ok((parse_id(&id)?, u64::try_from(rev).unwrap_or(0))))
        .collect()
}

/// Register `id` as an accounting owner under `parent` (idempotent). New
/// owners start `complete`; the parent must already be an owner.
pub(crate) async fn ensure_owner_in(
    conn: &mut SqliteConnection,
    id: SessionId,
    parent: Option<SessionId>,
) -> anyhow::Result<()> {
    let sid = id.to_string();
    let (root, parent) = match parent {
        None => (sid.clone(), None),
        Some(p) => {
            let p = p.to_string();
            let root: Option<(String,)> =
                sqlx::query_as("SELECT root_id FROM cost_owners WHERE id = ?")
                    .bind(&p)
                    .fetch_optional(&mut *conn)
                    .await?;
            let Some((root,)) = root else {
                anyhow::bail!("parent {p} is not an accounting owner");
            };
            (root, Some(p))
        }
    };
    let inserted = sqlx::query(
        "INSERT OR IGNORE INTO cost_owners (id, root_id, parent_id, deleted_at, history_coverage, revision) \
         VALUES (?, ?, ?, NULL, ?, 0)",
    )
    .bind(&sid)
    .bind(&root)
    .bind(&parent)
    .bind(Coverage::Complete.as_str())
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if inserted > 0 && parent.is_some() {
        bump_in(conn, &sid).await?;
    }
    Ok(())
}

/// Child delete: keep lineage and ledger, mark the owner gone.
pub(crate) async fn tombstone_owner_in(
    conn: &mut SqliteConnection,
    id: SessionId,
) -> anyhow::Result<()> {
    let sid = id.to_string();
    let changed =
        sqlx::query("UPDATE cost_owners SET deleted_at = ? WHERE id = ? AND deleted_at IS NULL")
            .bind(now())
            .bind(&sid)
            .execute(&mut *conn)
            .await?
            .rows_affected();
    if changed > 0 {
        bump_in(conn, &sid).await?;
    }
    Ok(())
}

/// Root delete: every call, meter and owner under `root`, owners child-first.
pub(crate) async fn delete_root_ledger_in(
    conn: &mut SqliteConnection,
    root: SessionId,
) -> anyhow::Result<()> {
    let rid = root.to_string();
    sqlx::query(
        "DELETE FROM model_calls WHERE owner_id IN (SELECT id FROM cost_owners WHERE root_id = ?)",
    )
    .bind(&rid)
    .execute(&mut *conn)
    .await?;
    sqlx::query("DELETE FROM remote_meters WHERE owner_id IN (SELECT id FROM cost_owners WHERE root_id = ?)")
        .bind(&rid)
        .execute(&mut *conn)
        .await?;
    // Leaf peeling: each pass removes owners nobody points at; bounded by depth.
    for _ in 0..LINEAGE_MAX {
        let removed = sqlx::query(
            "DELETE FROM cost_owners WHERE root_id = ?1 AND id NOT IN \
             (SELECT parent_id FROM cost_owners WHERE root_id = ?1 AND parent_id IS NOT NULL)",
        )
        .bind(&rid)
        .execute(&mut *conn)
        .await?
        .rows_affected();
        if removed == 0 {
            break;
        }
    }
    Ok(())
}

/// Raw `model_calls` columns the summary needs.
#[derive(FromRow)]
struct CallRow {
    owner_id: String,
    status: String,
    dispatched: bool,
    returned_nanos: Option<i64>,
    returned_currency: Option<String>,
    estimated_nanos: Option<i64>,
    estimated_currency: Option<String>,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    cache_read_tokens: Option<i64>,
    cache_write_tokens: Option<i64>,
}

/// Raw `remote_meters` columns the summary needs.
#[derive(FromRow)]
struct MeterRow {
    owner_id: String,
    currency: String,
    attributed_nanos: Option<i64>,
    coverage: String,
    source_metadata: Option<String>,
}

/// Latest epoch of one meter identity.
#[derive(FromRow)]
struct MeterState {
    id: String,
    owner_id: String,
    epoch: i64,
    currency: String,
    baseline_nanos: Option<i64>,
    high_water_nanos: Option<i64>,
    last_report_id: Option<String>,
    source_metadata: Option<String>,
    baseline_kind: String,
}

/// Raw reports kept per epoch in `remote_meters.source_metadata` (75 §6).
const RAW_REPORTS: usize = 8;

/// `remote_meters.source_metadata` (75 §6): the last [`RAW_REPORTS`] raw
/// reports of the epoch and whether a per-turn (delta) harness is suspected.
#[derive(Debug, Default, Serialize, Deserialize)]
struct MeterMetadata {
    /// `{amount, currency, at}` as reported, oldest first.
    #[serde(default)]
    reports: std::collections::VecDeque<RawReport>,
    /// A second lower total within this epoch (75 §6): the harness may
    /// report per-turn cost, not a cumulative total. Never summed (a
    /// harness-side reset looks the same); the row's `ambiguous` coverage
    /// already counts in `remote_meter_gaps`.
    #[serde(default)]
    delta_suspected: bool,
    /// Lower totals seen in this epoch.
    #[serde(default)]
    lower_totals: u32,
}

#[derive(Debug, Serialize, Deserialize)]
struct RawReport {
    amount: i64,
    currency: String,
    at: String,
}

impl MeterMetadata {
    fn parse(json: Option<&str>) -> Self {
        json.and_then(|j| serde_json::from_str(j).ok())
            .unwrap_or_default()
    }

    fn push(&mut self, nanos: i64, currency: &str) {
        if self.reports.len() == RAW_REPORTS {
            self.reports.pop_front();
        }
        self.reports.push_back(RawReport {
            amount: nanos,
            currency: currency.to_owned(),
            at: now(),
        });
    }

    fn json(&self) -> anyhow::Result<String> {
        Ok(serde_json::to_string(self)?)
    }
}

fn money(nanos: Option<i64>, currency: Option<&str>) -> Option<Money> {
    Some(Money {
        currency: currency?.to_owned(),
        nanos: u64::try_from(nanos?).ok()?,
    })
}

fn tokens(n: Option<i64>) -> u64 {
    n.and_then(|n| u64::try_from(n).ok()).unwrap_or(0)
}

/// Fold one attempt into its owner's direct bucket (§6.2 partition).
fn fold_call(b: &mut CostBucket, r: &CallRow) {
    let Ok(status) = r.status.parse::<CallStatus>() else {
        return;
    };
    if status == CallStatus::NotDispatched {
        return;
    }
    b.calls = b.calls.saturating_add(1);
    if r.dispatched {
        b.dispatched = b.dispatched.saturating_add(1);
    }
    match status {
        CallStatus::Unresolved => {
            b.unresolved = b.unresolved.saturating_add(1);
            return;
        }
        CallStatus::Pending | CallStatus::Dispatched => return,
        _ => {}
    }
    if let Some(m) = money(r.returned_nanos, r.returned_currency.as_deref()) {
        if add_by_currency(&mut b.returned, &m).is_err() {
            b.unavailable = b.unavailable.saturating_add(1);
        }
    } else if let Some(m) = money(r.estimated_nanos, r.estimated_currency.as_deref()) {
        if add_by_currency(&mut b.estimated_for_unavailable, &m).is_err() {
            b.unavailable = b.unavailable.saturating_add(1);
        }
    } else {
        b.unavailable = b.unavailable.saturating_add(1);
    }
    let t = &mut b.tokens;
    if r.input_tokens.is_none() && r.output_tokens.is_none() {
        t.unknown_calls = t.unknown_calls.saturating_add(1);
    }
    t.input = t.input.saturating_add(tokens(r.input_tokens));
    t.output = t.output.saturating_add(tokens(r.output_tokens));
    t.cache_read = t.cache_read.saturating_add(tokens(r.cache_read_tokens));
    t.cache_write = t.cache_write.saturating_add(tokens(r.cache_write_tokens));
}

/// One owner in the loaded subtree.
struct Node {
    parent: Option<String>,
    deleted: bool,
    direct: CostBucket,
    scope: CostScope,
}

/// Recursive total of `id`: direct plus every child's total.
fn total_of(
    id: &str,
    nodes: &HashMap<String, Node>,
    children: &BTreeMap<String, Vec<String>>,
) -> CostBucket {
    let mut total = nodes[id].direct.clone();
    for c in children.get(id).into_iter().flatten() {
        total.absorb(&total_of(c, nodes, children));
    }
    total
}

impl Store {
    /// Register `id` as an accounting owner under `parent` (see [`ensure_owner_in`]).
    pub async fn ensure_owner(
        &self,
        id: SessionId,
        parent: Option<SessionId>,
    ) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        ensure_owner_in(&mut tx, id, parent).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Durably reserve one attempt (`pending`, coverage `unresolved`) before
    /// transport; committed on return.
    #[expect(clippy::too_many_arguments, reason = "one row, one call")]
    pub async fn reserve_call(
        &self,
        owner: SessionId,
        request_id: &str,
        attempt: u32,
        turn: Option<TurnId>,
        kind: CallKind,
        provider: Option<&str>,
        model: Option<&str>,
    ) -> anyhow::Result<CallId> {
        let call = CallId(Uuid::now_v7().to_string());
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "INSERT INTO model_calls (call_id, owner_id, request_id, attempt, turn_id, kind, status, \
             provider, requested_model, coverage, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&call.0)
        .bind(owner.to_string())
        .bind(request_id)
        .bind(i64::from(attempt))
        .bind(turn.map(|t| t.to_string()))
        .bind(kind.as_str())
        .bind(CallStatus::Pending.as_str())
        .bind(provider)
        .bind(model)
        .bind(CallStatus::Unresolved.as_str())
        .bind(now())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(call)
    }

    /// Mark a pending attempt dispatched; commit before invoking transport.
    pub async fn mark_dispatched(&self, call: &CallId) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        let n = sqlx::query(
            "UPDATE model_calls SET status = ?, dispatched_at = ? WHERE call_id = ? AND status = ?",
        )
        .bind(CallStatus::Dispatched.as_str())
        .bind(now())
        .bind(&call.0)
        .bind(CallStatus::Pending.as_str())
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        anyhow::ensure!(n == 1, "call {call} is not pending");
        Ok(())
    }

    /// Settle an attempt (replay-safe: the row is updated, never appended) and
    /// bump the owner and live ancestors. `status` must be terminal.
    pub async fn settle_call(
        &self,
        call: &CallId,
        status: CallStatus,
        obs: CostObservation,
    ) -> anyhow::Result<Vec<(SessionId, u64)>> {
        anyhow::ensure!(status.is_terminal(), "{status} is not a settlement status");
        for m in obs.returned.iter().chain(obs.estimated.iter()) {
            anyhow::ensure!(crate::cost::valid_currency(&m.currency), "invalid currency");
            anyhow::ensure!(i64::try_from(m.nanos).is_ok(), "amount out of range");
        }
        let (coverage, unknown_reason) = if obs.returned.is_some() || obs.estimated.is_some() {
            (Coverage::Complete, None)
        } else {
            (
                Coverage::Unknown,
                Some(
                    obs.unknown_reason
                        .as_deref()
                        .unwrap_or("no cost observation"),
                ),
            )
        };
        let mut tx = self.begin_write().await?;
        let owner: Option<(String,)> = sqlx::query_as(
            "UPDATE model_calls SET status = ?, input_tokens = ?, output_tokens = ?, cache_read_tokens = ?, \
             cache_write_tokens = ?, context_tokens = ?, provider = COALESCE(?, provider), \
             requested_model = COALESCE(?, requested_model), served_model = ?, provider_request_id = ?, \
             returned_nanos = ?, returned_currency = ?, returned_source = ?, \
             estimated_nanos = ?, estimated_currency = ?, estimate_source = ?, \
             unknown_reason = ?, coverage = ?, settled_at = ? WHERE call_id = ? RETURNING owner_id",
        )
        .bind(status.as_str())
        .bind(obs.input_tokens.map(i64::from))
        .bind(obs.output_tokens.map(i64::from))
        .bind(obs.cache_read_tokens.map(i64::from))
        .bind(obs.cache_write_tokens.map(i64::from))
        .bind(obs.context_tokens.map(i64::from))
        .bind(&obs.provider)
        .bind(&obs.requested_model)
        .bind(&obs.served_model)
        .bind(&obs.provider_request_id)
        .bind(obs.returned.as_ref().map(|m| m.nanos as i64))
        .bind(obs.returned.as_ref().map(|m| &m.currency))
        .bind(obs.returned.as_ref().map(|_| Provenance::ProviderReported.as_str()))
        .bind(obs.estimated.as_ref().map(|m| m.nanos as i64))
        .bind(obs.estimated.as_ref().map(|m| &m.currency))
        .bind(obs.estimated.as_ref().map(|_| Provenance::AdapterEstimated.as_str()))
        .bind(unknown_reason)
        .bind(coverage.as_str())
        .bind(now())
        .bind(&call.0)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((owner,)) = owner else {
            anyhow::bail!("unknown call {call}");
        };
        let bumped = bump_in(&mut tx, &owner).await?;
        tx.commit().await?;
        Ok(bumped)
    }

    /// A conclusively local pre-dispatch failure: excluded from every total.
    pub async fn mark_not_dispatched(&self, call: &CallId, reason: &str) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        let n = sqlx::query(
            "UPDATE model_calls SET status = ?, coverage = ?, unknown_reason = ?, settled_at = ? \
             WHERE call_id = ? AND status = ?",
        )
        .bind(CallStatus::NotDispatched.as_str())
        .bind(Coverage::Complete.as_str())
        .bind(reason)
        .bind(now())
        .bind(&call.0)
        .bind(CallStatus::Pending.as_str())
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        anyhow::ensure!(n == 1, "call {call} is not pending");
        Ok(())
    }

    /// Startup: attempts still `pending`/`dispatched` become `unresolved`
    /// (neither lost nor resent). Returns how many.
    pub async fn unresolve_open_calls(&self) -> anyhow::Result<u64> {
        let mut tx = self.begin_write().await?;
        let owners: Vec<(String,)> = sqlx::query_as(
            "UPDATE model_calls SET status = ?, settled_at = ? WHERE status IN (?, ?) RETURNING owner_id",
        )
        .bind(CallStatus::Unresolved.as_str())
        .bind(now())
        .bind(CallStatus::Pending.as_str())
        .bind(CallStatus::Dispatched.as_str())
        .fetch_all(&mut *tx)
        .await?;
        let n = owners.len() as u64;
        let mut seen = std::collections::HashSet::new();
        for (owner,) in owners {
            if seen.insert(owner.clone()) {
                bump_in(&mut tx, &owner).await?;
            }
        }
        tx.commit().await?;
        Ok(n)
    }

    /// Apply one cumulative meter observation (§6.3 state machine) keyed by
    /// `(origin, external_session, epoch)`; persists before returning.
    pub async fn apply_meter(
        &self,
        owner: SessionId,
        update: MeterUpdate,
    ) -> anyhow::Result<Vec<(SessionId, u64)>> {
        let total = &update.cumulative;
        anyhow::ensure!(
            crate::cost::valid_currency(&total.currency),
            "invalid currency"
        );
        let total_nanos =
            i64::try_from(total.nanos).map_err(|_| anyhow::anyhow!("amount out of range"))?;
        let oid = owner.to_string();
        let mut tx = self.begin_write().await?;
        let state: Option<MeterState> = sqlx::query_as(
            "SELECT id, owner_id, epoch, currency, baseline_nanos, high_water_nanos, last_report_id, \
             source_metadata, baseline_kind \
             FROM remote_meters WHERE origin = ? AND external_session = ? ORDER BY epoch DESC LIMIT 1",
        )
        .bind(&update.origin)
        .bind(&update.external_session)
        .fetch_optional(&mut *tx)
        .await?;

        let (epoch, known_zero) = match state {
            None => (0, update.confirmed_new_session),
            Some(mut s) => {
                anyhow::ensure!(s.owner_id == oid, "meter belongs to another owner");
                if update.report_id.is_some() && update.report_id == s.last_report_id {
                    return Ok(Vec::new());
                }
                // 75 §6: a known-zero baseline with nothing observed is
                // currency-agnostic — the wrapper's connect-time row names a
                // placeholder. Take the first report's currency in place; only
                // a currency change after attribution opens an epoch.
                if s.currency != total.currency
                    && s.baseline_kind == "known_zero"
                    && s.high_water_nanos.unwrap_or(0) == 0
                {
                    sqlx::query("UPDATE remote_meters SET currency = ?, updated_at = ? WHERE id = ?")
                        .bind(&total.currency)
                        .bind(now())
                        .bind(&s.id)
                        .execute(&mut *tx)
                        .await?;
                    s.currency = total.currency.clone();
                }
                if s.currency == total.currency {
                    let high = s.high_water_nanos.unwrap_or(0);
                    let base = s.baseline_nanos.unwrap_or(0);
                    let mut meta = MeterMetadata::parse(s.source_metadata.as_deref());
                    meta.push(total_nanos, &total.currency);
                    let changed = match total_nanos.cmp(&high) {
                        std::cmp::Ordering::Equal => {
                            sqlx::query(
                                "UPDATE remote_meters SET last_report_id = ?, source_metadata = ?, \
                                 updated_at = ? WHERE id = ?",
                            )
                            .bind(&update.report_id)
                            .bind(meta.json()?)
                            .bind(now())
                            .bind(&s.id)
                            .execute(&mut *tx)
                            .await?;
                            false
                        }
                        std::cmp::Ordering::Greater => {
                            sqlx::query(
                                "UPDATE remote_meters SET high_water_nanos = ?, attributed_nanos = ?, \
                                 last_report_id = ?, source_metadata = ?, updated_at = ? WHERE id = ?",
                            )
                            .bind(total_nanos)
                            .bind(total_nanos - base)
                            .bind(&update.report_id)
                            .bind(meta.json()?)
                            .bind(now())
                            .bind(&s.id)
                            .execute(&mut *tx)
                            .await?;
                            true
                        }
                        std::cmp::Ordering::Less => {
                            meta.lower_totals = meta.lower_totals.saturating_add(1);
                            meta.delta_suspected = meta.lower_totals >= 2;
                            sqlx::query(
                                "UPDATE remote_meters SET coverage = ?, coverage_boundary = ?, \
                                 last_report_id = ?, source_metadata = ?, updated_at = ? WHERE id = ?",
                            )
                            .bind(Coverage::Ambiguous.as_str())
                            .bind("lower_total")
                            .bind(&update.report_id)
                            .bind(meta.json()?)
                            .bind(now())
                            .bind(&s.id)
                            .execute(&mut *tx)
                            .await?;
                            true
                        }
                    };
                    let bumped = if changed {
                        bump_in(&mut tx, &oid).await?
                    } else {
                        Vec::new()
                    };
                    tx.commit().await?;
                    return Ok(bumped);
                }
                // Currency change: unobserved boundary, new epoch with an imported baseline.
                sqlx::query(
                    "UPDATE remote_meters SET coverage_boundary = ?, updated_at = ? WHERE id = ?",
                )
                .bind("currency_change")
                .bind(now())
                .bind(&s.id)
                .execute(&mut *tx)
                .await?;
                (s.epoch + 1, false)
            }
        };
        let baseline = if known_zero { 0 } else { total_nanos };
        let mut meta = MeterMetadata::default();
        meta.push(total_nanos, &total.currency);
        sqlx::query(
            "INSERT INTO remote_meters (id, owner_id, origin, external_session, epoch, source, currency, \
             baseline_nanos, high_water_nanos, attributed_nanos, baseline_kind, scope, coverage, \
             coverage_boundary, last_report_id, source_metadata, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(Uuid::now_v7().to_string())
        .bind(&oid)
        .bind(&update.origin)
        .bind(&update.external_session)
        .bind(epoch)
        .bind(Provenance::OpaqueRemoteReported.as_str())
        .bind(&total.currency)
        .bind(baseline)
        .bind(total_nanos)
        .bind(total_nanos - baseline)
        .bind(if known_zero { "known_zero" } else { "imported" })
        .bind(CostScope::OpaqueRemoteInclusive.as_str())
        .bind(if known_zero { Coverage::Complete } else { Coverage::Unknown }.as_str())
        .bind(if known_zero { None } else { Some("imported_baseline") })
        .bind(&update.report_id)
        .bind(meta.json()?)
        .bind(now())
        .execute(&mut *tx)
        .await?;
        let bumped = bump_in(&mut tx, &oid).await?;
        tx.commit().await?;
        Ok(bumped)
    }

    /// Owner summary over retained accounting ancestry, one read transaction.
    /// `None` when `id` is not an owner.
    pub async fn session_cost(&self, id: SessionId) -> anyhow::Result<Option<SessionCost>> {
        let sid = id.to_string();
        let mut tx = self.read.begin().await?;
        let head: Option<(String, i64)> =
            sqlx::query_as("SELECT history_coverage, revision FROM cost_owners WHERE id = ?")
                .bind(&sid)
                .fetch_optional(&mut *tx)
                .await?;
        let Some((history_coverage, revision)) = head else {
            return Ok(None);
        };
        /// `?1` = subtree root, `?2` = depth cap; prefix for the three subtree reads.
        macro_rules! subtree {
            ($tail:literal) => {
                concat!(
                    "WITH RECURSIVE sub(id, depth) AS (SELECT ?1, 0 UNION ALL \
                     SELECT c.id, sub.depth + 1 FROM cost_owners c JOIN sub ON c.parent_id = sub.id \
                     WHERE sub.depth < ?2) ",
                    $tail
                )
            };
        }
        let owners: Vec<(String, Option<String>, bool)> = sqlx::query_as(subtree!(
            "SELECT c.id, c.parent_id, c.deleted_at IS NOT NULL FROM cost_owners c \
             WHERE c.id IN (SELECT id FROM sub)"
        ))
        .bind(&sid)
        .bind(LINEAGE_MAX)
        .fetch_all(&mut *tx)
        .await?;
        let calls: Vec<CallRow> = sqlx::query_as(subtree!(
            "SELECT owner_id, status, dispatched_at IS NOT NULL AS dispatched, returned_nanos, \
             returned_currency, estimated_nanos, estimated_currency, input_tokens, output_tokens, \
             cache_read_tokens, cache_write_tokens FROM model_calls WHERE owner_id IN (SELECT id FROM sub)"
        ))
        .bind(&sid)
        .bind(LINEAGE_MAX)
        .fetch_all(&mut *tx)
        .await?;
        let meters: Vec<MeterRow> = sqlx::query_as(subtree!(
            "SELECT owner_id, currency, attributed_nanos, coverage, source_metadata FROM remote_meters \
             WHERE owner_id IN (SELECT id FROM sub)"
        ))
        .bind(&sid)
        .bind(LINEAGE_MAX)
        .fetch_all(&mut *tx)
        .await?;
        let turns = super::turn_usage::sums(&mut tx, &sid).await?;
        let names: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT c.id, s.name FROM cost_owners c LEFT JOIN sessions s ON s.id = c.id WHERE c.parent_id = ?",
        )
        .bind(&sid)
        .fetch_all(&mut *tx)
        .await?;
        drop(tx);

        let mut nodes: HashMap<String, Node> = owners
            .into_iter()
            .map(|(id, parent, deleted)| {
                (
                    id,
                    Node {
                        parent,
                        deleted,
                        direct: CostBucket::default(),
                        scope: CostScope::Native,
                    },
                )
            })
            .collect();
        for r in &calls {
            if let Some(n) = nodes.get_mut(&r.owner_id) {
                fold_call(&mut n.direct, r);
            }
        }
        for r in &turns {
            if let Some(n) = nodes.get_mut(r.owner()) {
                r.add_to(&mut n.direct.tokens);
            }
        }
        let remote_tokens = super::turn_usage::by_model(&turns);
        let mut remote_meter_gaps: u32 = 0;
        let mut delta_suspected = false;
        for m in &meters {
            if m.coverage != Coverage::Complete.as_str() {
                remote_meter_gaps = remote_meter_gaps.saturating_add(1);
            }
            delta_suspected |= MeterMetadata::parse(m.source_metadata.as_deref()).delta_suspected;
            let Some(n) = nodes.get_mut(&m.owner_id) else {
                continue;
            };
            n.scope = CostScope::OpaqueRemoteInclusive;
            match money(m.attributed_nanos, Some(&m.currency)) {
                Some(money) if add_by_currency(&mut n.direct.returned, &money).is_ok() => {}
                _ => n.direct.unavailable = n.direct.unavailable.saturating_add(1),
            }
        }
        let mut children: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (id, n) in &nodes {
            if let Some(p) = &n.parent
                && p != id
            {
                children.entry(p.clone()).or_default().push(id.clone());
            }
        }
        // Deterministic child order (owner ids are time-ordered uuids).
        for v in children.values_mut() {
            v.sort();
        }
        let names: HashMap<String, Option<String>> = names.into_iter().collect();
        let mut child_costs = Vec::new();
        let mut descendants = CostBucket::default();
        for c in children.get(&sid).into_iter().flatten() {
            let total = total_of(c, &nodes, &children);
            descendants.absorb(&total);
            let n = &nodes[c];
            child_costs.push(ChildCost {
                session: parse_id(c)?,
                name: names.get(c).cloned().flatten(),
                deleted: n.deleted,
                total,
                scope: n.scope,
            });
        }
        let me = nodes
            .remove(&sid)
            .ok_or_else(|| anyhow::anyhow!("owner {sid} vanished"))?;
        let mut total = me.direct.clone();
        total.absorb(&descendants);
        Ok(Some(SessionCost {
            session: id,
            revision: u64::try_from(revision).unwrap_or(0),
            history_coverage: history_coverage.parse().unwrap_or(Coverage::Unknown),
            scope: me.scope,
            direct: me.direct,
            descendants,
            total,
            children: child_costs,
            remote_meter_gaps,
            delta_suspected,
            remote_tokens,
        }))
    }

    /// Child delete (see [`tombstone_owner_in`]).
    pub async fn tombstone_owner(&self, id: SessionId) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        tombstone_owner_in(&mut tx, id).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Root delete (see [`delete_root_ledger_in`]).
    pub async fn delete_root_ledger(&self, root: SessionId) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        delete_root_ledger_in(&mut tx, root).await?;
        tx.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fresh() -> Store {
        let dir = std::env::temp_dir().join(format!("theuth-acct-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        Store::open(&dir.join("sessions.sqlite"), None)
            .await
            .unwrap()
    }

    fn usd(amount: f64) -> Money {
        Money::from_f64(amount, "USD").unwrap()
    }

    async fn charge(store: &Store, owner: SessionId, obs: CostObservation) -> CallId {
        let call = store
            .reserve_call(owner, "req", 0, None, CallKind::Round, Some("p"), Some("m"))
            .await
            .unwrap();
        store.mark_dispatched(&call).await.unwrap();
        store
            .settle_call(&call, CallStatus::Settled, obs)
            .await
            .unwrap();
        call
    }

    fn returned(b: &CostBucket) -> Vec<(&str, u64)> {
        b.returned
            .iter()
            .map(|m| (m.currency.as_str(), m.nanos))
            .collect()
    }

    #[tokio::test]
    async fn worked_example_lineage_replay_tombstone_delete() {
        let store = fresh().await;
        let (r, c, g) = (SessionId::new(), SessionId::new(), SessionId::new());
        store.ensure_owner(r, None).await.unwrap();
        store.ensure_owner(c, Some(r)).await.unwrap();
        store.ensure_owner(g, Some(c)).await.unwrap();
        assert!(
            store
                .ensure_owner(SessionId::new(), Some(SessionId::new()))
                .await
                .is_err()
        );

        charge(
            &store,
            r,
            CostObservation {
                returned: Some(usd(0.10)),
                ..Default::default()
            },
        )
        .await;
        charge(
            &store,
            c,
            CostObservation {
                returned: Some(usd(0.20)),
                ..Default::default()
            },
        )
        .await;
        let gc = charge(
            &store,
            g,
            CostObservation {
                returned: Some(usd(0.30)),
                ..Default::default()
            },
        )
        .await;

        let rc = store.session_cost(r).await.unwrap().unwrap();
        assert_eq!(returned(&rc.direct), vec![("USD", 100_000_000)]);
        assert_eq!(returned(&rc.total), vec![("USD", 600_000_000)]);
        assert_eq!(rc.total.calls, 3);
        assert_eq!(rc.history_coverage, Coverage::Complete);
        assert_eq!(rc.children.len(), 1);
        assert_eq!(rc.children[0].session, c);
        assert_eq!(returned(&rc.children[0].total), vec![("USD", 500_000_000)]);
        let gcst = store.session_cost(g).await.unwrap().unwrap();
        assert_eq!(returned(&gcst.total), vec![("USD", 300_000_000)]);

        // Replay: same call_id settled again → totals unchanged, revisions of the chain bumped.
        let before = rc.revision;
        let bumped = store
            .settle_call(
                &gc,
                CallStatus::Settled,
                CostObservation {
                    returned: Some(usd(0.30)),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(bumped.len(), 3);
        let rc = store.session_cost(r).await.unwrap().unwrap();
        assert_eq!(returned(&rc.total), vec![("USD", 600_000_000)]);
        assert_eq!(rc.total.calls, 3);
        assert!(rc.revision > before);

        store.tombstone_owner(g).await.unwrap();
        store.tombstone_owner(c).await.unwrap();
        let rc = store.session_cost(r).await.unwrap().unwrap();
        assert_eq!(returned(&rc.total), vec![("USD", 600_000_000)]);
        assert!(rc.children[0].deleted);
        // A late settlement on a tombstoned child bumps only live ancestors.
        let bumped = store
            .settle_call(
                &gc,
                CallStatus::Failed,
                CostObservation {
                    returned: Some(usd(0.30)),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(bumped.iter().map(|(s, _)| *s).collect::<Vec<_>>(), vec![r]);

        store.delete_root_ledger(r).await.unwrap();
        assert!(store.session_cost(r).await.unwrap().is_none());
        assert!(store.session_cost(g).await.unwrap().is_none());
        let (calls,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM model_calls")
            .fetch_one(&store.read)
            .await
            .unwrap();
        assert_eq!(calls, 0);
    }

    #[tokio::test]
    async fn partition_returned_estimate_unavailable() {
        let store = fresh().await;
        let o = SessionId::new();
        store.ensure_owner(o, None).await.unwrap();
        charge(
            &store,
            o,
            CostObservation {
                returned: Some(usd(0.10)),
                estimated: Some(usd(9.0)),
                ..Default::default()
            },
        )
        .await;
        charge(
            &store,
            o,
            CostObservation {
                estimated: Some(usd(0.02)),
                input_tokens: Some(5),
                ..Default::default()
            },
        )
        .await;
        charge(&store, o, CostObservation::default()).await;
        let skipped = store
            .reserve_call(o, "req", 0, None, CallKind::Title, None, None)
            .await
            .unwrap();
        store
            .mark_not_dispatched(&skipped, "no model")
            .await
            .unwrap();
        let open = store
            .reserve_call(o, "req2", 0, None, CallKind::Round, None, None)
            .await
            .unwrap();
        store.mark_dispatched(&open).await.unwrap();
        assert_eq!(store.unresolve_open_calls().await.unwrap(), 1);

        let c = store.session_cost(o).await.unwrap().unwrap();
        assert_eq!(returned(&c.direct), vec![("USD", 100_000_000)]);
        assert_eq!(c.direct.estimated_for_unavailable, vec![usd(0.02)]);
        assert_eq!(c.direct.unavailable, 1);
        assert_eq!(c.direct.unresolved, 1);
        assert_eq!(c.direct.calls, 4);
        assert_eq!(c.direct.dispatched, 4);
        assert_eq!(c.direct.tokens.unknown_calls, 2);
        assert_eq!(c.direct.tokens.input, 5);
        assert!(
            store
                .settle_call(&skipped, CallStatus::Pending, CostObservation::default())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn meter_state_machine() {
        let store = fresh().await;
        let (r, child) = (SessionId::new(), SessionId::new());
        store.ensure_owner(r, None).await.unwrap();
        store.ensure_owner(child, Some(r)).await.unwrap();
        charge(
            &store,
            r,
            CostObservation {
                returned: Some(usd(0.10)),
                ..Default::default()
            },
        )
        .await;
        let upd = |amount: f64, cur: &str, new: bool| MeterUpdate {
            origin: "omp".into(),
            external_session: "ext-1".into(),
            cumulative: Money::from_f64(amount, cur).unwrap(),
            confirmed_new_session: new,
            report_id: None,
        };
        store
            .apply_meter(child, upd(0.20, "USD", true))
            .await
            .unwrap();
        assert!(
            store
                .apply_meter(child, upd(0.20, "USD", true))
                .await
                .unwrap()
                .is_empty()
        );
        let bumped = store
            .apply_meter(child, upd(0.35, "USD", true))
            .await
            .unwrap();
        assert_eq!(bumped.len(), 2);
        let rc = store.session_cost(r).await.unwrap().unwrap();
        assert_eq!(returned(&rc.total), vec![("USD", 450_000_000)]);
        assert_eq!(rc.children[0].scope, CostScope::OpaqueRemoteInclusive);
        assert_eq!(rc.children[0].total.calls, 0);
        assert_eq!(rc.remote_meter_gaps, 0);

        // Lower total: ambiguous, high water kept.
        store
            .apply_meter(child, upd(0.30, "USD", true))
            .await
            .unwrap();
        let rc = store.session_cost(r).await.unwrap().unwrap();
        assert_eq!(returned(&rc.total), vec![("USD", 450_000_000)]);
        assert_eq!(rc.remote_meter_gaps, 1);

        // Currency change: new epoch, grouped separately, boundary unknown.
        store
            .apply_meter(child, upd(1.0, "EUR", true))
            .await
            .unwrap();
        store
            .apply_meter(child, upd(1.5, "EUR", true))
            .await
            .unwrap();
        let cc = store.session_cost(child).await.unwrap().unwrap();
        assert_eq!(
            returned(&cc.direct),
            vec![("EUR", 500_000_000), ("USD", 350_000_000)]
        );
        assert_eq!(cc.remote_meter_gaps, 2);

        // Imported baseline: first observation attributes nothing.
        let other = SessionId::new();
        store.ensure_owner(other, None).await.unwrap();
        let imported = |amount: f64, report: &str| MeterUpdate {
            origin: "omp".into(),
            external_session: "ext-2".into(),
            cumulative: usd(amount),
            confirmed_new_session: false,
            report_id: Some(report.into()),
        };
        store.apply_meter(other, imported(0.50, "a")).await.unwrap();
        let oc = store.session_cost(other).await.unwrap().unwrap();
        assert_eq!(returned(&oc.direct), vec![("USD", 0)]);
        assert_eq!(oc.remote_meter_gaps, 1);
        assert!(
            store
                .apply_meter(other, imported(0.70, "a"))
                .await
                .unwrap()
                .is_empty()
        );
        store.apply_meter(other, imported(0.70, "b")).await.unwrap();
        let oc = store.session_cost(other).await.unwrap().unwrap();
        assert_eq!(returned(&oc.direct), vec![("USD", 200_000_000)]);
        assert!(store.apply_meter(r, imported(0.80, "c")).await.is_err());
    }
    fn guard_upd(amount: f64, cur: &str) -> MeterUpdate {
        MeterUpdate {
            origin: "omp".into(),
            external_session: "ext-1".into(),
            cumulative: Money::from_f64(amount, cur).unwrap(),
            confirmed_new_session: true,
            report_id: None,
        }
    }

    async fn meters(store: &Store) -> Vec<(i64, String, String, Option<String>)> {
        sqlx::query_as(
            "SELECT epoch, currency, coverage, source_metadata FROM remote_meters ORDER BY epoch",
        )
        .fetch_all(&store.read)
        .await
        .unwrap()
    }

    /// 75 §6: the connect-time zero row is currency-agnostic; a first report
    /// in EUR takes over the row instead of opening an imported epoch.
    #[tokio::test]
    async fn zero_baseline_takes_first_currency() {
        let store = fresh().await;
        let child = SessionId::new();
        store.ensure_owner(child, None).await.unwrap();
        store.apply_meter(child, guard_upd(0.0, "USD")).await.unwrap();
        store.apply_meter(child, guard_upd(1.0, "EUR")).await.unwrap();
        let rows = meters(&store).await;
        assert_eq!(rows.len(), 1, "no new epoch");
        assert_eq!((rows[0].0, rows[0].1.as_str(), rows[0].2.as_str()), (0, "EUR", "complete"));
        let cost = store.session_cost(child).await.unwrap().unwrap();
        assert_eq!(cost.direct.returned, vec![Money::from_f64(1.0, "EUR").unwrap()]);
        assert_eq!(cost.remote_meter_gaps, 0);
        // After attribution a currency change still opens an epoch.
        store.apply_meter(child, guard_upd(0.5, "USD")).await.unwrap();
        assert_eq!(meters(&store).await.len(), 2);
    }

    /// 75 §6: a monotone (omp-like) series keeps the epoch complete with its
    /// raw reports kept; two lower totals flag a suspected per-turn reporter.
    #[tokio::test]
    async fn lower_totals_flag_delta_suspected() {
        let store = fresh().await;
        let child = SessionId::new();
        store.ensure_owner(child, None).await.unwrap();
        for amount in [0.0, 0.2, 0.2, 0.35, 0.5] {
            store.apply_meter(child, guard_upd(amount, "USD")).await.unwrap();
        }
        let cost = store.session_cost(child).await.unwrap().unwrap();
        assert_eq!(cost.remote_meter_gaps, 0);
        let meta = |rows: &[(i64, String, String, Option<String>)]| {
            MeterMetadata::parse(rows[0].3.as_deref())
        };
        let m = meta(&meters(&store).await);
        assert_eq!(m.reports.len(), 5);
        assert!(!m.delta_suspected);

        store.apply_meter(child, guard_upd(0.1, "USD")).await.unwrap();
        let m = meta(&meters(&store).await);
        assert_eq!((m.lower_totals, m.delta_suspected), (1, false));
        assert!(!store.session_cost(child).await.unwrap().unwrap().delta_suspected);
        store.apply_meter(child, guard_upd(0.2, "USD")).await.unwrap();
        let rows = meters(&store).await;
        let m = meta(&rows);
        assert_eq!((m.lower_totals, m.delta_suspected), (2, true));
        assert_eq!(rows[0].2, "ambiguous");
        // The ring keeps the last eight reports only.
        for amount in [0.3, 0.4, 0.6] {
            store.apply_meter(child, guard_upd(amount, "USD")).await.unwrap();
        }
        let m = meta(&meters(&store).await);
        assert_eq!(m.reports.len(), RAW_REPORTS);
        assert_eq!(m.reports.back().unwrap().amount, 600_000_000);
        // High water is still the rule: 0.6 > 0.5 attributed the delta, nothing summed.
        let cost = store.session_cost(child).await.unwrap().unwrap();
        assert_eq!(cost.direct.returned, vec![Money::from_f64(0.6, "USD").unwrap()]);
        assert!(cost.delta_suspected, "surfaced on the summary for the cost dialog");
    }
}
