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
    MeterUpdate, Money, Provenance, SessionCost, SpendDay, SpendOwner, SpendOwnerKind, SpendReport,
    add_by_currency,
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
/// each live row touched. A tombstoned `owner` is bumped too (a settlement
/// landing after a delete must still reach the cost cube, review 160-ENG #5)
/// but not returned; tombstoned ancestors are skipped, so a settlement on a
/// deleted child still invalidates its surviving ancestors.
pub(super) async fn bump_in(
    conn: &mut SqliteConnection,
    owner: &str,
) -> anyhow::Result<Vec<(SessionId, u64)>> {
    let rows: Vec<(String, i64, bool)> = sqlx::query_as(
        "WITH RECURSIVE up(id, parent_id, deleted_at, depth) AS (
             SELECT id, parent_id, deleted_at, 0 FROM cost_owners WHERE id = ?
             UNION ALL
             SELECT c.id, c.parent_id, c.deleted_at, up.depth + 1
             FROM cost_owners c JOIN up ON c.id = up.parent_id WHERE up.depth < ?
         )
         UPDATE cost_owners SET revision = revision + 1
         WHERE id IN (SELECT id FROM up WHERE deleted_at IS NULL OR depth = 0)
         RETURNING id, revision, deleted_at IS NOT NULL",
    )
    .bind(owner)
    .bind(LINEAGE_MAX)
    .fetch_all(conn)
    .await?;
    rows.into_iter()
        .filter(|(_, _, deleted)| !deleted)
        .map(|(id, rev, _)| Ok((parse_id(&id)?, u64::try_from(rev).unwrap_or(0))))
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
    let changed = sqlx::query(
        "UPDATE cost_owners SET deleted_at = ?, \
         harness_model = (SELECT harness_model FROM sessions WHERE sessions.id = cost_owners.id) \
         WHERE id = ? AND deleted_at IS NULL",
    )
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

/// One `model_calls` attempt row (`pending`, coverage `unresolved`) inside
/// the caller's transaction: [`Store::reserve_call`], and the loop's
/// `model_call` journal step (109 §3.3 step 2) which rides the same commit.
#[expect(clippy::too_many_arguments, reason = "one row, one call")]
pub(crate) async fn reserve_call_in(
    conn: &mut SqliteConnection,
    owner: SessionId,
    request_id: &str,
    attempt: u32,
    turn: Option<TurnId>,
    kind: CallKind,
    provider: Option<&str>,
    model: Option<&str>,
) -> anyhow::Result<CallId> {
    let call = CallId(Uuid::now_v7().to_string());
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
    .execute(conn)
    .await?;
    Ok(call)
}

/// Root delete (160 H1): the books stay. Every owner of the subtree is
/// tombstoned like a child delete; calls, meters and turns are kept; one
/// `cost_root_tombstones` row keeps the root's attribution (desk,
/// unattended, routine name, project, creation time) for when its session
/// row is gone. Run before the session rows are deleted, in their transaction.
pub(crate) async fn tombstone_root_in(
    conn: &mut SqliteConnection,
    root: SessionId,
) -> anyhow::Result<()> {
    let rid = root.to_string();
    let at = now();
    type Head = (Option<String>, bool, String, Option<String>, String);
    let head: Option<Head> = sqlx::query_as(
        "SELECT desk, unattended, title, project, created_at FROM sessions WHERE id = ?",
    )
    .bind(&rid)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some((desk, unattended, title, project, created_at)) = head {
        let routine = unattended.then(|| routine_name(&title).to_owned());
        sqlx::query(
            "INSERT OR REPLACE INTO cost_root_tombstones \
             (root_id, desk, unattended, routine, project, created_at, deleted_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&rid)
        .bind(desk)
        .bind(unattended)
        .bind(routine)
        .bind(project)
        .bind(created_at)
        .bind(&at)
        .execute(&mut *conn)
        .await?;
    }
    sqlx::query(
        "UPDATE cost_owners SET deleted_at = ?, \
         harness_model = (SELECT harness_model FROM sessions WHERE sessions.id = cost_owners.id) \
         WHERE root_id = ? AND deleted_at IS NULL",
    )
    .bind(&at)
    .bind(&rid)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// A scheduler root's routine: its title before ` · ` (`routine` when empty).
fn routine_name(title: &str) -> &str {
    let name = title.split(" · ").next().unwrap_or("").trim();
    if name.is_empty() { "routine" } else { name }
}

/// Who a root belongs to (128 R3, 160 D6): the desk's agent, a scheduler
/// root's routine, the user; a root whose session row is gone keeps its
/// tombstone's attribution (160 H1), else is `other`.
pub(crate) fn root_owner(
    live: Option<(&str, Option<&str>, bool)>,
    tomb: Option<(Option<&str>, bool, Option<&str>)>,
) -> (SpendOwnerKind, String) {
    let (desk, unattended, routine) = match (live, tomb) {
        (Some((title, desk, unattended)), _) => (desk, unattended, Some(routine_name(title))),
        (None, Some((desk, unattended, routine))) => (desk, unattended, routine),
        (None, None) => return (SpendOwnerKind::Other, "other".to_owned()),
    };
    if let Some(agent) = desk {
        return (SpendOwnerKind::Agent, agent.to_owned());
    }
    if unattended {
        return (
            SpendOwnerKind::Routine,
            routine.unwrap_or("routine").to_owned(),
        );
    }
    (SpendOwnerKind::User, "you".to_owned())
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

/// One `model_calls` row with its root session, for [`Store::spend_report`].
#[derive(FromRow)]
struct SpendRow {
    created_at: String,
    status: String,
    returned_nanos: Option<i64>,
    returned_currency: Option<String>,
    estimated_nanos: Option<i64>,
    estimated_currency: Option<String>,
    /// `cost_owners.root_id`; `NULL` when the owner was never registered.
    root_id: Option<String>,
}

impl SpendRow {
    /// Dollars of this attempt (`returned ?? estimated`, USD only); `None`
    /// when no money was observed, `Some(0.0)` for another currency.
    fn usd(&self) -> Option<f64> {
        let m = money(self.returned_nanos, self.returned_currency.as_deref())
            .or_else(|| money(self.estimated_nanos, self.estimated_currency.as_deref()))?;
        Some(if m.currency == "USD" {
            m.nanos as f64 / crate::cost::NANOS_PER_UNIT as f64
        } else {
            0.0
        })
    }
}

/// One accounting root with its live session row and its tombstone (160 H1)
/// when either exists: the cube's root dimension and `spend_report`'s owners.
#[derive(FromRow)]
pub(crate) struct RootRow {
    pub(crate) id: String,
    /// `NULL` when the root's session row is gone.
    pub(crate) title: Option<String>,
    pub(crate) desk: Option<String>,
    pub(crate) unattended: Option<bool>,
    pub(crate) project: Option<String>,
    pub(crate) created_at: Option<String>,
    pub(crate) t_present: bool,
    pub(crate) t_desk: Option<String>,
    pub(crate) t_unattended: Option<bool>,
    pub(crate) t_routine: Option<String>,
    pub(crate) t_project: Option<String>,
    pub(crate) t_created_at: Option<String>,
}

impl RootRow {
    /// spend_report's owner rules (see [`root_owner`]).
    pub(crate) fn owner(&self) -> (SpendOwnerKind, String) {
        root_owner(
            self.title
                .as_deref()
                .map(|t| (t, self.desk.as_deref(), self.unattended == Some(true))),
            self.t_present.then(|| {
                (
                    self.t_desk.as_deref(),
                    self.t_unattended == Some(true),
                    self.t_routine.as_deref(),
                )
            }),
        )
    }
}

/// Add one attempt to a day/month bucket.
fn fold_spend(d: &mut SpendDay, usd: Option<f64>) {
    d.calls = d.calls.saturating_add(1);
    match usd {
        Some(v) => d.usd += v,
        None => d.unresolved = d.unresolved.saturating_add(1),
    }
}

/// Raw `remote_meters` columns the summary needs.
#[derive(FromRow)]
struct MeterRow {
    owner_id: String,
    currency: String,
    attributed_nanos: Option<i64>,
    coverage: String,
    source_metadata: Option<String>,
    /// `CostScope` as stored: `opaque_remote_inclusive` for a harness's own
    /// meter, `observed` for a watch-only child's (141 §7.6).
    scope: String,
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

    fn push(&mut self, nanos: i64, currency: &str, at: &str) {
        if self.reports.len() == RAW_REPORTS {
            self.reports.pop_front();
        }
        self.reports.push_back(RawReport {
            amount: nanos,
            currency: currency.to_owned(),
            at: at.to_owned(),
        });
    }

    fn json(&self) -> anyhow::Result<String> {
        Ok(serde_json::to_string(self)?)
    }
}

/// The raw reports kept in a meter's `source_metadata`, oldest first, as
/// `(cumulative nanos, currency, arrival time)`; empty when absent or unreadable.
pub(crate) fn metadata_reports(json: Option<&str>) -> Vec<(i64, String, String)> {
    MeterMetadata::parse(json)
        .reports
        .into_iter()
        .map(|r| (r.amount, r.currency, r.at))
        .collect()
}

/// 160 H2: keep one applied meter report beside the metadata ring.
async fn record_report_in(
    conn: &mut SqliteConnection,
    meter: &str,
    nanos: i64,
    currency: &str,
    at: &str,
) -> anyhow::Result<()> {
    // Review 160-ENG #3: an unchanged total (every `usage_update` repeats it)
    // is not kept again; lower totals are (placement needs them).
    sqlx::query(
        "INSERT INTO remote_meter_reports (meter_id, amount_nanos, currency, at) \
         SELECT ?1, ?2, ?3, ?4 WHERE NOT EXISTS (SELECT 1 FROM ( \
             SELECT amount_nanos, currency FROM remote_meter_reports WHERE meter_id = ?1 \
             ORDER BY at DESC, rowid DESC LIMIT 1) WHERE amount_nanos = ?2 AND currency = ?3)",
    )
    .bind(meter)
    .bind(nanos)
    .bind(currency)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
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
        let mut tx = self.begin_write().await?;
        let call = reserve_call_in(
            &mut tx, owner, request_id, attempt, turn, kind, provider, model,
        )
        .await?;
        tx.commit().await?;
        Ok(call)
    }

    /// 136 §2.2: what `owner`'s own attempts created at or after `since`
    /// (RFC 3339, the night's budget stamp — 136 R-1) have cost in
    /// `currency` — `returned_nanos ?? estimated_nanos` of every settled row,
    /// other currencies not counted, plus one `estimate` per row still
    /// `pending`/`dispatched` (money committed but not yet settled, 136
    /// R-27) — and the estimate for its next call: the newest settled
    /// attempt of `model` (same measure), 0 when there is none. Children are
    /// not counted.
    // ponytail: owner's own rows only; sum the `cost_owners` subtree if routines grow children.
    pub async fn budget_spent(
        &self,
        owner: SessionId,
        currency: &str,
        model: &str,
        since: &str,
    ) -> anyhow::Result<(u64, u64)> {
        let sid = owner.to_string();
        let (settled, in_flight): (i64, i64) = sqlx::query_as(
            "SELECT COALESCE(SUM(CASE \
                WHEN returned_nanos IS NOT NULL THEN IIF(returned_currency = ?2, returned_nanos, 0) \
                WHEN estimated_currency = ?2 THEN estimated_nanos ELSE 0 END), 0), \
                COALESCE(SUM(status IN (?4, ?5)), 0) \
             FROM model_calls WHERE owner_id = ?1 AND created_at >= ?3",
        )
        .bind(&sid)
        .bind(currency)
        .bind(since)
        .bind(CallStatus::Pending.as_str())
        .bind(CallStatus::Dispatched.as_str())
        .fetch_one(&self.read)
        .await?;
        /// `(returned_nanos, returned_currency, estimated_nanos, estimated_currency)`.
        type MoneyRow = (Option<i64>, Option<String>, Option<i64>, Option<String>);
        let last: Option<MoneyRow> = sqlx::query_as(
            "SELECT returned_nanos, returned_currency, estimated_nanos, estimated_currency \
                 FROM model_calls WHERE owner_id = ?1 AND requested_model = ?2 AND status = ?3 \
                 ORDER BY created_at DESC LIMIT 1",
        )
        .bind(&sid)
        .bind(model)
        .bind(CallStatus::Settled.as_str())
        .fetch_optional(&self.read)
        .await?;
        let estimate = last
            .and_then(|(rn, rc, en, ec)| {
                money(rn, rc.as_deref()).or_else(|| money(en, ec.as_deref()))
            })
            .filter(|m| m.currency == currency)
            .map_or(0, |m| m.nanos);
        let settled = u64::try_from(settled).unwrap_or(0);
        let in_flight = u64::try_from(in_flight).unwrap_or(0);
        Ok((
            settled.saturating_add(in_flight.saturating_mul(estimate)),
            estimate,
        ))
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
        self.apply_meter_scoped(owner, update, CostScope::OpaqueRemoteInclusive)
            .await
    }

    /// [`Self::apply_meter`] for a watch-only child's own cost (141 §7.6
    /// `counted`): the rows carry [`CostScope::Observed`] so
    /// `session_cost().children[]` names them; they roll up through
    /// `cost_owners` like any child's.
    pub async fn apply_observed_meter(
        &self,
        owner: SessionId,
        update: MeterUpdate,
    ) -> anyhow::Result<Vec<(SessionId, u64)>> {
        self.apply_meter_scoped(owner, update, CostScope::Observed)
            .await
    }

    async fn apply_meter_scoped(
        &self,
        owner: SessionId,
        update: MeterUpdate,
        scope: CostScope,
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
                    sqlx::query(
                        "UPDATE remote_meters SET currency = ?, updated_at = ? WHERE id = ?",
                    )
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
                    let at = now();
                    meta.push(total_nanos, &total.currency, &at);
                    record_report_in(&mut tx, &s.id, total_nanos, &total.currency, &at).await?;
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
        let at = now();
        meta.push(total_nanos, &total.currency, &at);
        let id = Uuid::now_v7().to_string();
        sqlx::query(
            "INSERT INTO remote_meters (id, owner_id, origin, external_session, epoch, source, currency, \
             baseline_nanos, high_water_nanos, attributed_nanos, baseline_kind, scope, coverage, \
             coverage_boundary, last_report_id, source_metadata, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
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
        .bind(scope.as_str())
        .bind(if known_zero { Coverage::Complete } else { Coverage::Unknown }.as_str())
        .bind(if known_zero { None } else { Some("imported_baseline") })
        .bind(&update.report_id)
        .bind(meta.json()?)
        .bind(&at)
        .execute(&mut *tx)
        .await?;
        record_report_in(&mut tx, &id, total_nanos, &total.currency, &at).await?;
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
            "SELECT owner_id, currency, attributed_nanos, coverage, source_metadata, scope \
             FROM remote_meters WHERE owner_id IN (SELECT id FROM sub)"
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
            n.scope = m.scope.parse().unwrap_or(CostScope::OpaqueRemoteInclusive);
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

    /// Model-call spend by local day (128 Rev 3): the last `days` local
    /// calendar days ending on `now_ms`'s day (clamped to 1..=31), the month
    /// to date, and the window's top 8 owners by root session. Local day =
    /// this host's timezone, computed here, not in SQL. 160 H5: `reported`
    /// (the cost cube's placed remote-meter money) is added to the days, the
    /// month and the owners' dollars at its placed time; it adds no calls.
    pub(crate) async fn spend_report(
        &self,
        now_ms: u64,
        days: u32,
        reported: &[crate::costs::Piece],
    ) -> anyhow::Result<SpendReport> {
        use chrono::{Datelike, Local, TimeZone};
        let days = days.clamp(1, 31);
        let now = chrono::Utc
            .timestamp_millis_opt(i64::try_from(now_ms).unwrap_or(i64::MAX))
            .single()
            .ok_or_else(|| anyhow::anyhow!("now_ms {now_ms} out of range"))?;
        let today = now.with_timezone(&Local).date_naive();
        let first = today - chrono::Days::new(u64::from(days - 1));
        let month_first = today.with_day(1).unwrap_or(today);
        // `created_at` is RFC3339 text, so the SQL bound is only a coarse
        // prefilter (one day early covers any stored offset); the exact day
        // test is on the parsed timestamp below.
        let since = (first.min(month_first) - chrono::Days::new(1))
            .and_hms_opt(0, 0, 0)
            .unwrap_or_default()
            .and_utc()
            .to_rfc3339();
        let rows: Vec<SpendRow> = sqlx::query_as(
            "SELECT m.created_at, m.status, m.returned_nanos, m.returned_currency, \
             m.estimated_nanos, m.estimated_currency, c.root_id \
             FROM model_calls m \
             LEFT JOIN cost_owners c ON c.id = m.owner_id \
             WHERE m.created_at >= ?",
        )
        .bind(&since)
        .fetch_all(&self.read)
        .await?;
        let roots: HashMap<String, (SpendOwnerKind, String)> = self
            .cost_roots()
            .await?
            .into_iter()
            .map(|r| {
                let owner = r.owner();
                (r.id, owner)
            })
            .collect();
        let owner_of = |root: Option<&str>| {
            root.and_then(|r| roots.get(r))
                .cloned()
                .unwrap_or((SpendOwnerKind::Other, "other".to_owned()))
        };

        let mut report = SpendReport {
            days: (0..days)
                .map(|i| SpendDay {
                    day: (first + chrono::Days::new(u64::from(i)))
                        .format("%Y-%m-%d")
                        .to_string(),
                    ..Default::default()
                })
                .collect(),
            month: SpendDay {
                day: today.format("%Y-%m").to_string(),
                ..Default::default()
            },
            owners: Vec::new(),
        };
        let mut owners: HashMap<(SpendOwnerKind, String), SpendOwner> = HashMap::new();
        let mut owner_add = |root: Option<&str>, usd: f64, calls: u32| {
            let (kind, name) = owner_of(root);
            let o = owners
                .entry((kind, name.clone()))
                .or_insert_with(|| SpendOwner {
                    kind,
                    name,
                    usd: 0.0,
                    calls: 0,
                });
            o.usd += usd;
            o.calls = o.calls.saturating_add(calls);
        };
        for r in &rows {
            if r.status.parse::<CallStatus>().ok() == Some(CallStatus::NotDispatched) {
                continue;
            }
            let Ok(at) = chrono::DateTime::parse_from_rfc3339(&r.created_at) else {
                continue;
            };
            let day = at.with_timezone(&Local).date_naive();
            if day > today {
                continue;
            }
            let usd = r.usd();
            if day >= month_first {
                fold_spend(&mut report.month, usd);
            }
            if day < first {
                continue;
            }
            let i = usize::try_from((day - first).num_days()).unwrap_or(0);
            fold_spend(&mut report.days[i], usd);
            owner_add(r.root_id.as_deref(), usd.unwrap_or(0.0), 1);
        }
        for p in reported {
            let Some(at) = chrono::Utc
                .timestamp_millis_opt(i64::try_from(p.at_ms).unwrap_or(i64::MAX))
                .single()
            else {
                continue;
            };
            let day = at.with_timezone(&Local).date_naive();
            if day > today {
                continue;
            }
            let usd = p.nanos as f64 / crate::cost::NANOS_PER_UNIT as f64;
            if day >= month_first {
                report.month.usd += usd;
            }
            if day < first {
                continue;
            }
            let i = usize::try_from((day - first).num_days()).unwrap_or(0);
            report.days[i].usd += usd;
            owner_add(Some(&p.root), usd, 0);
        }
        let mut owners: Vec<SpendOwner> = owners.into_values().collect();
        owners.sort_by(|a, b| {
            b.usd
                .total_cmp(&a.usd)
                .then_with(|| b.calls.cmp(&a.calls))
                .then_with(|| a.name.cmp(&b.name))
        });
        owners.truncate(8);
        report.owners = owners;
        Ok(report)
    }

    /// Child delete (see [`tombstone_owner_in`]).
    pub async fn tombstone_owner(&self, id: SessionId) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        tombstone_owner_in(&mut tx, id).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Root delete keeping the books (see [`tombstone_root_in`]).
    pub async fn tombstone_root(&self, root: SessionId) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        tombstone_root_in(&mut tx, root).await?;
        tx.commit().await?;
        Ok(())
    }

    /// 160 cube: every accounting root with its session row and tombstone.
    pub(crate) async fn cost_roots(&self) -> anyhow::Result<Vec<RootRow>> {
        Ok(sqlx::query_as(
            "SELECT c.id, s.title, s.desk, s.unattended, s.project, s.created_at, \
             t.root_id IS NOT NULL AS t_present, t.desk AS t_desk, t.unattended AS t_unattended, \
             t.routine AS t_routine, t.project AS t_project, t.created_at AS t_created_at \
             FROM cost_owners c LEFT JOIN sessions s ON s.id = c.id \
             LEFT JOIN cost_root_tombstones t ON t.root_id = c.id \
             WHERE c.id = c.root_id",
        )
        .fetch_all(&self.read)
        .await?)
    }

    /// 160 cube: `(owner, root, revision)` of every accounting owner.
    pub(crate) async fn cost_revisions(&self) -> anyhow::Result<Vec<(String, String, i64)>> {
        Ok(
            sqlx::query_as("SELECT id, root_id, revision FROM cost_owners")
                .fetch_all(&self.read)
                .await?,
        )
    }

    /// 160 H3: owners whose history predates the ledger.
    pub(crate) async fn cost_pre_ledger_sessions(&self) -> anyhow::Result<u32> {
        let n: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM cost_owners WHERE history_coverage = ?")
                .bind(Coverage::Unknown.as_str())
                .fetch_one(&self.read)
                .await?;
        Ok(u32::try_from(n).unwrap_or(u32::MAX))
    }

    /// 160 cube, native leg: `model_calls ⋈ cost_owners` (not
    /// `not_dispatched`) grouped by UTC hour, root, provider, model, kind,
    /// money class and, for rows without money, the reason; rows without
    /// money but with tokens stay one group each (`solo`) so the builder
    /// reprices them exactly as `cost_calls` does. `roots`/`since` narrow an
    /// incremental refresh (`cost_owners_root` → `model_calls_created`).
    pub(crate) async fn cost_native(
        &self,
        roots: Option<&[String]>,
        since: Option<&str>,
    ) -> anyhow::Result<Vec<NativeGroup>> {
        let roots_json = roots.map(|r| serde_json::to_string(r).unwrap_or_default());
        Ok(sqlx::query_as(
            "SELECT substr(m.created_at, 1, 13) AS hour, c.root_id, \
             COALESCE(m.provider, '') AS provider, COALESCE(m.requested_model, '') AS model, m.kind, \
             CASE WHEN m.returned_nanos IS NOT NULL THEN 0 WHEN m.estimated_nanos IS NOT NULL THEN 2 ELSE 3 END AS cls, \
             CASE WHEN m.returned_nanos IS NULL AND m.estimated_nanos IS NULL \
                  THEN COALESCE(m.unknown_reason, m.status) END AS reason, \
             CASE WHEN m.returned_nanos IS NULL AND m.estimated_nanos IS NULL \
                  AND (m.input_tokens IS NOT NULL OR m.output_tokens IS NOT NULL) THEN m.call_id END AS solo, \
             COUNT(*) AS calls, \
             SUM(m.status IN ('failed', 'cancelled', 'unresolved')) AS failed, \
             SUM(CASE WHEN m.returned_nanos IS NOT NULL THEN IIF(m.returned_currency = 'USD', m.returned_nanos, 0) \
                 WHEN m.estimated_nanos IS NOT NULL THEN IIF(m.estimated_currency = 'USD', m.estimated_nanos, 0) \
                 ELSE 0 END) AS nanos, \
             SUM(COALESCE(m.input_tokens, 0)) AS input, SUM(COALESCE(m.output_tokens, 0)) AS output, \
             SUM(COALESCE(m.cache_read_tokens, 0)) AS cache_read, \
             SUM(COALESCE(m.cache_write_tokens, 0)) AS cache_write \
             FROM cost_owners c JOIN model_calls m ON m.owner_id = c.id AND m.created_at >= COALESCE(?2, '') \
             WHERE (?1 IS NULL OR c.root_id IN (SELECT value FROM json_each(?1))) \
             AND m.status <> 'not_dispatched' \
             GROUP BY 1, 2, 3, 4, 5, 6, 7, 8",
        )
        .bind(roots_json)
        .bind(since)
        .fetch_all(&self.read)
        .await?)
    }

    /// 160 cube, reported leg: every remote meter with its owner's root and
    /// harness model (the live session's, else the one its tombstone kept).
    pub(crate) async fn cost_meters(&self) -> anyhow::Result<Vec<MeterInput>> {
        Ok(sqlx::query_as(
            "SELECT r.id, r.owner_id, c.root_id, r.origin, r.currency, \
             COALESCE(r.baseline_nanos, 0) AS baseline_nanos, COALESCE(r.attributed_nanos, 0) AS attributed_nanos, \
             r.updated_at, r.source_metadata, COALESCE(s.harness_model, c.harness_model) AS harness_model \
             FROM remote_meters r JOIN cost_owners c ON c.id = r.owner_id \
             LEFT JOIN sessions s ON s.id = r.owner_id",
        )
        .fetch_all(&self.read)
        .await?)
    }

    /// 160 H2: every kept meter report `(meter, cumulative nanos, currency, at)`.
    pub(crate) async fn cost_meter_reports(
        &self,
    ) -> anyhow::Result<Vec<(String, i64, String, String)>> {
        Ok(sqlx::query_as(
            "SELECT meter_id, amount_nanos, currency, at FROM remote_meter_reports ORDER BY meter_id, at",
        )
        .fetch_all(&self.read)
        .await?)
    }

    /// 160 cube, reported leg: every remote turn with its owner's root.
    pub(crate) async fn cost_turns(&self) -> anyhow::Result<Vec<TurnInput>> {
        Ok(sqlx::query_as(
            "SELECT t.owner_id, c.root_id, t.harness, t.model, t.input_tokens, t.output_tokens, \
             t.cache_read_tokens, t.cache_write_tokens, t.at \
             FROM remote_turns t JOIN cost_owners c ON c.id = t.owner_id ORDER BY t.owner_id, t.at",
        )
        .fetch_all(&self.read)
        .await?)
    }

    /// 160 §4: native ledger rows newest first with `created_at < upper`
    /// (and `>= lower`); `priced` = model ids the catalog prices, so a row
    /// without money but with tokens counts as estimated.
    ///
    /// 160 R2: the WHERE holds only the filters present (one `=` / `IN` term
    /// each), and the scan is driven by `f.driver`, an index forced with
    /// `INDEXED BY` that the caller picks from the cube's counts. The planner
    /// has no table statistics, so it guesses which filter is selective; the
    /// cube knows. A driving list of several values (a model's aliases, two
    /// providers) becomes one ordered, `LIMIT`ed branch per value, merged by
    /// `UNION ALL`, so no branch sorts its whole match set.
    pub(crate) async fn cost_call_rows(
        &self,
        f: &CallFilter<'_>,
        upper: &str,
        lower: Option<&str>,
        limit: u32,
    ) -> anyhow::Result<Vec<CallRowOut>> {
        use sqlx::{QueryBuilder, Sqlite};
        let (index, driving): (&str, Option<(&str, &[String])>) = match f.driver {
            CallDriver::Time | CallDriver::Roots => ("model_calls_created_at", None),
            CallDriver::Provider => (
                "model_calls_provider_created",
                f.providers.map(|v| ("m.provider", v)),
            ),
            CallDriver::Model => (
                "model_calls_model_created",
                f.models.map(|v| ("m.requested_model", v)),
            ),
            CallDriver::Kind => ("model_calls_kind_created", f.kinds.map(|v| ("m.kind", v))),
            CallDriver::Unpriced => ("model_calls_unpriced_created", None),
            CallDriver::Known => ("model_calls_known_created", None),
        };
        // One branch per value of the driving list; `None` = a single branch.
        let branches: Vec<Option<&str>> = match driving {
            Some((_, values)) => values.iter().map(|v| Some(v.as_str())).collect(),
            None => vec![None],
        };
        if branches.is_empty() {
            return Ok(Vec::new());
        }
        let roots_json = f
            .roots
            .map(|r| serde_json::to_string(r).unwrap_or_default());
        let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new("");
        for (i, value) in branches.iter().enumerate() {
            if i > 0 {
                qb.push(" UNION ALL ");
            }
            qb.push(
                "SELECT * FROM (SELECT m.call_id, m.created_at, c.root_id, m.owner_id, m.provider, \
                 m.requested_model, m.kind, m.status, m.returned_nanos, m.returned_currency, \
                 m.estimated_nanos, m.estimated_currency, m.input_tokens, m.output_tokens, \
                 m.cache_read_tokens, m.cache_write_tokens, m.unknown_reason, \
                 CASE WHEN m.returned_nanos IS NOT NULL THEN 0 WHEN m.estimated_nanos IS NOT NULL THEN 2 \
                      WHEN (m.input_tokens IS NOT NULL OR m.output_tokens IS NOT NULL) \
                           AND m.requested_model IN (SELECT value FROM json_each(",
            );
            qb.push_bind(f.priced);
            qb.push(")) THEN 2 ELSE 3 END AS basis FROM ");
            match (&f.driver, &roots_json) {
                (CallDriver::Roots, Some(roots)) => {
                    qb.push("(SELECT DISTINCT value FROM json_each(");
                    qb.push_bind(roots.as_str());
                    qb.push(
                        ")) r CROSS JOIN cost_owners c ON c.root_id = r.value \
                         CROSS JOIN model_calls m ON m.owner_id = c.id",
                    );
                }
                _ => {
                    qb.push(format_args!(
                        "model_calls m INDEXED BY {index} JOIN cost_owners c ON c.id = m.owner_id"
                    ));
                }
            }
            qb.push(" WHERE m.created_at < ");
            qb.push_bind(upper);
            qb.push(" AND m.created_at >= ");
            qb.push_bind(lower.unwrap_or(""));
            qb.push(" AND m.status <> 'not_dispatched'");
            if let (Some((col, _)), Some(v)) = (driving, value) {
                qb.push(format_args!(" AND {col} = "));
                qb.push_bind(*v);
            }
            let lists: [(&str, Option<&[String]>, bool); 4] = [
                ("c.root_id", f.roots, f.driver == CallDriver::Roots),
                ("m.provider", f.providers, f.driver == CallDriver::Provider),
                ("m.requested_model", f.models, f.driver == CallDriver::Model),
                ("m.kind", f.kinds, f.driver == CallDriver::Kind),
            ];
            for (col, values, driven) in lists {
                let Some(values) = values.filter(|_| !driven) else {
                    continue;
                };
                qb.push(format_args!(" AND {col} IN ("));
                let mut sep = qb.separated(", ");
                for v in values {
                    sep.push_bind(v.as_str());
                }
                qb.push(")");
            }
            if let Some(bases) = f.bases {
                qb.push(" AND basis IN (");
                let mut sep = qb.separated(", ");
                for b in bases {
                    sep.push_bind(i64::from(b.code()));
                }
                qb.push(")");
            }
            // The partial indexes' own predicates (`INDEXED BY` needs them in the WHERE).
            match f.driver {
                CallDriver::Unpriced => {
                    qb.push(" AND m.returned_nanos IS NULL AND m.estimated_nanos IS NULL");
                }
                CallDriver::Known => {
                    qb.push(" AND m.returned_nanos IS NOT NULL");
                }
                _ => {}
            }
            qb.push(" ORDER BY m.created_at DESC LIMIT ");
            qb.push_bind(i64::from(limit));
            qb.push(")");
        }
        if branches.len() > 1 {
            qb.push(" ORDER BY created_at DESC LIMIT ");
            qb.push_bind(i64::from(limit));
        }
        Ok(qb.build_query_as().fetch_all(&self.read).await?)
    }
}

/// The index a [`Store::cost_call_rows`] scan runs on (160 R2), chosen by
/// the caller from the cost cube's counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CallDriver {
    /// `model_calls_created_at`, newest first, every filter checked per row.
    Time,
    /// From the root list through `cost_owners_root` → `model_calls_created`
    /// (reads every row of those roots, then sorts: for small root sets).
    Roots,
    /// `model_calls_provider_created`, one branch per provider.
    Provider,
    /// `model_calls_model_created`, one branch per model id.
    Model,
    /// `model_calls_kind_created`, one branch per kind.
    Kind,
    /// `model_calls_unpriced_created` (only when `bases` is `[unpriced]`).
    Unpriced,
    /// `model_calls_known_created` (only when `bases` is `[known]`).
    Known,
}

/// One group of [`Store::cost_native`].
#[derive(FromRow)]
pub(crate) struct NativeGroup {
    /// `YYYY-MM-DDTHH` (UTC).
    pub(crate) hour: String,
    pub(crate) root_id: String,
    pub(crate) provider: String,
    pub(crate) model: String,
    pub(crate) kind: String,
    /// 0 known · 2 estimated · 3 no money.
    pub(crate) cls: i64,
    pub(crate) reason: Option<String>,
    pub(crate) solo: Option<String>,
    pub(crate) calls: i64,
    pub(crate) failed: i64,
    pub(crate) nanos: i64,
    pub(crate) input: i64,
    pub(crate) output: i64,
    pub(crate) cache_read: i64,
    pub(crate) cache_write: i64,
}

/// One meter of [`Store::cost_meters`].
#[derive(FromRow)]
pub(crate) struct MeterInput {
    pub(crate) id: String,
    pub(crate) owner_id: String,
    pub(crate) root_id: String,
    pub(crate) origin: String,
    pub(crate) currency: String,
    pub(crate) baseline_nanos: i64,
    pub(crate) attributed_nanos: i64,
    pub(crate) updated_at: String,
    pub(crate) source_metadata: Option<String>,
    pub(crate) harness_model: Option<String>,
}

/// One turn of [`Store::cost_turns`].
#[derive(FromRow)]
pub(crate) struct TurnInput {
    pub(crate) owner_id: String,
    pub(crate) root_id: String,
    pub(crate) harness: String,
    pub(crate) model: Option<String>,
    pub(crate) input_tokens: i64,
    pub(crate) output_tokens: i64,
    pub(crate) cache_read_tokens: i64,
    pub(crate) cache_write_tokens: i64,
    pub(crate) at: String,
}

/// SQL-side filter of [`Store::cost_call_rows`].
#[derive(Clone, Copy)]
pub(crate) struct CallFilter<'a> {
    pub(crate) roots: Option<&'a [String]>,
    pub(crate) providers: Option<&'a [String]>,
    pub(crate) models: Option<&'a [String]>,
    pub(crate) kinds: Option<&'a [String]>,
    pub(crate) bases: Option<&'a [crate::costs::CostBasis]>,
    /// JSON array of catalog-priced model ids.
    pub(crate) priced: &'a str,
    /// The index the scan runs on.
    pub(crate) driver: CallDriver,
}

/// One row of [`Store::cost_call_rows`].
#[derive(FromRow)]
pub(crate) struct CallRowOut {
    pub(crate) call_id: String,
    pub(crate) created_at: String,
    pub(crate) root_id: String,
    pub(crate) owner_id: String,
    pub(crate) provider: Option<String>,
    pub(crate) requested_model: Option<String>,
    pub(crate) kind: String,
    pub(crate) status: String,
    pub(crate) returned_nanos: Option<i64>,
    pub(crate) returned_currency: Option<String>,
    pub(crate) estimated_nanos: Option<i64>,
    pub(crate) estimated_currency: Option<String>,
    pub(crate) input_tokens: Option<i64>,
    pub(crate) output_tokens: Option<i64>,
    pub(crate) cache_read_tokens: Option<i64>,
    pub(crate) cache_write_tokens: Option<i64>,
    pub(crate) unknown_reason: Option<String>,
    pub(crate) basis: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fresh() -> (Store, crate::TestDir) {
        let scratch = crate::TestDir::new("acct");
        let store = Store::open(&scratch.0.join("sessions.sqlite"), None)
            .await
            .unwrap();
        (store, scratch)
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
        let (store, _dir) = fresh().await;
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

        // 160 H1: a root delete keeps the books — owners tombstoned, rows kept.
        store.tombstone_root(r).await.unwrap();
        let rc = store.session_cost(r).await.unwrap().unwrap();
        assert_eq!(returned(&rc.total), vec![("USD", 600_000_000)]);
        let (calls, live): (i64, i64) = sqlx::query_as(
            "SELECT (SELECT COUNT(*) FROM model_calls), \
             (SELECT COUNT(*) FROM cost_owners WHERE deleted_at IS NULL)",
        )
        .fetch_one(&store.read)
        .await
        .unwrap();
        assert_eq!((calls, live), (3, 0));
    }

    #[tokio::test]
    async fn partition_returned_estimate_unavailable() {
        let (store, _dir) = fresh().await;
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
        let (store, _dir) = fresh().await;
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

    /// 141 §7.6: a watch-only child's `counted` meter rows are stored with
    /// the `observed` scope and read back as such on the child and on the
    /// parent's `children[]`, while rolling up like any child's spend; the
    /// harness's own rows stay `opaque_remote_inclusive`.
    #[tokio::test]
    async fn observed_meter_rows_keep_their_scope_and_roll_up() {
        let (store, _dir) = fresh().await;
        let (acp, child) = (SessionId::new(), SessionId::new());
        store.ensure_owner(acp, None).await.unwrap();
        store.ensure_owner(child, Some(acp)).await.unwrap();
        let upd = |ext: &str, amount: f64| MeterUpdate {
            origin: "omp".into(),
            external_session: ext.into(),
            cumulative: Money::from_f64(amount, "USD").unwrap(),
            confirmed_new_session: true,
            report_id: None,
        };
        store.apply_meter(acp, upd("s1", 0.13948325)).await.unwrap();
        store
            .apply_observed_meter(child, upd("s1/ReadA", 0.1149025))
            .await
            .unwrap();
        store
            .apply_observed_meter(child, upd("s1/ReadA", 0.12128025))
            .await
            .unwrap();
        let c = store.session_cost(child).await.unwrap().unwrap();
        assert_eq!(c.scope, CostScope::Observed);
        assert_eq!(
            returned(&c.direct),
            vec![("USD", 121_280_250)],
            "high water once"
        );
        let p = store.session_cost(acp).await.unwrap().unwrap();
        assert_eq!(p.scope, CostScope::OpaqueRemoteInclusive);
        assert_eq!(p.children[0].scope, CostScope::Observed);
        assert_eq!(returned(&p.total), vec![("USD", 260_763_500)]);
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
        let (store, _dir) = fresh().await;
        let child = SessionId::new();
        store.ensure_owner(child, None).await.unwrap();
        store
            .apply_meter(child, guard_upd(0.0, "USD"))
            .await
            .unwrap();
        store
            .apply_meter(child, guard_upd(1.0, "EUR"))
            .await
            .unwrap();
        let rows = meters(&store).await;
        assert_eq!(rows.len(), 1, "no new epoch");
        assert_eq!(
            (rows[0].0, rows[0].1.as_str(), rows[0].2.as_str()),
            (0, "EUR", "complete")
        );
        let cost = store.session_cost(child).await.unwrap().unwrap();
        assert_eq!(
            cost.direct.returned,
            vec![Money::from_f64(1.0, "EUR").unwrap()]
        );
        assert_eq!(cost.remote_meter_gaps, 0);
        // After attribution a currency change still opens an epoch.
        store
            .apply_meter(child, guard_upd(0.5, "USD"))
            .await
            .unwrap();
        assert_eq!(meters(&store).await.len(), 2);
    }

    /// 75 §6: a monotone (omp-like) series keeps the epoch complete with its
    /// raw reports kept; two lower totals flag a suspected per-turn reporter.
    #[tokio::test]
    async fn lower_totals_flag_delta_suspected() {
        let (store, _dir) = fresh().await;
        let child = SessionId::new();
        store.ensure_owner(child, None).await.unwrap();
        for amount in [0.0, 0.2, 0.2, 0.35, 0.5] {
            store
                .apply_meter(child, guard_upd(amount, "USD"))
                .await
                .unwrap();
        }
        let cost = store.session_cost(child).await.unwrap().unwrap();
        assert_eq!(cost.remote_meter_gaps, 0);
        let meta = |rows: &[(i64, String, String, Option<String>)]| {
            MeterMetadata::parse(rows[0].3.as_deref())
        };
        let m = meta(&meters(&store).await);
        assert_eq!(m.reports.len(), 5);
        assert!(!m.delta_suspected);

        store
            .apply_meter(child, guard_upd(0.1, "USD"))
            .await
            .unwrap();
        let m = meta(&meters(&store).await);
        assert_eq!((m.lower_totals, m.delta_suspected), (1, false));
        assert!(
            !store
                .session_cost(child)
                .await
                .unwrap()
                .unwrap()
                .delta_suspected
        );
        store
            .apply_meter(child, guard_upd(0.2, "USD"))
            .await
            .unwrap();
        let rows = meters(&store).await;
        let m = meta(&rows);
        assert_eq!((m.lower_totals, m.delta_suspected), (2, true));
        assert_eq!(rows[0].2, "ambiguous");
        // The ring keeps the last eight reports only.
        for amount in [0.3, 0.4, 0.6] {
            store
                .apply_meter(child, guard_upd(amount, "USD"))
                .await
                .unwrap();
        }
        let m = meta(&meters(&store).await);
        assert_eq!(m.reports.len(), RAW_REPORTS);
        assert_eq!(m.reports.back().unwrap().amount, 600_000_000);
        // High water is still the rule: 0.6 > 0.5 attributed the delta, nothing summed.
        let cost = store.session_cost(child).await.unwrap().unwrap();
        assert_eq!(
            cost.direct.returned,
            vec![Money::from_f64(0.6, "USD").unwrap()]
        );
        assert!(
            cost.delta_suspected,
            "surfaced on the summary for the cost dialog"
        );
    }

    /// 128 Rev 3: local-day buckets, month to date, `unresolved` for calls
    /// without money, another currency counted but not summed, and owners
    /// attributed by root session (desk → agent, unattended root → routine,
    /// plain root → `you`, gone root → `other`).
    #[tokio::test]
    async fn spend_report_buckets_days_month_and_owners() {
        use chrono::{Datelike, Local};
        let (store, _dir) = fresh().await;
        let now = chrono::Utc::now();
        let now_ms = u64::try_from(now.timestamp_millis()).unwrap();
        let today = now.with_timezone(&Local).date_naive();
        let yesterday = now - chrono::Duration::days(1);
        // Outside every window (≤ 31 days) and outside this month.
        let long_ago = now - chrono::Duration::days(40);
        async fn at(store: &Store, call: &CallId, when: chrono::DateTime<chrono::Utc>) {
            sqlx::query("UPDATE model_calls SET created_at = ? WHERE call_id = ?")
                .bind(when.to_rfc3339())
                .bind(&call.0)
                .execute(&store.read)
                .await
                .unwrap();
        }
        let obs = |m: Option<Money>, estimated: bool| CostObservation {
            returned: (!estimated).then(|| m.clone()).flatten(),
            estimated: estimated.then(|| m.clone()).flatten(),
            ..Default::default()
        };

        // The user's own root: today $1.00, yesterday $0.50 (estimate), 40 days ago $10,
        // today EUR 3 (counted, not summed), today no money (unresolved).
        let me = SessionId::new();
        store
            .create_session(me, "m", None, None, None)
            .await
            .unwrap();
        store.set_title(me, "my chat").await.unwrap();
        charge(&store, me, obs(Some(usd(1.0)), false)).await;
        let c = charge(&store, me, obs(Some(usd(0.5)), true)).await;
        at(&store, &c, yesterday).await;
        let c = charge(&store, me, obs(Some(usd(10.0)), false)).await;
        at(&store, &c, long_ago).await;
        charge(
            &store,
            me,
            obs(Some(Money::from_f64(3.0, "EUR").unwrap()), false),
        )
        .await;
        charge(&store, me, obs(None, false)).await;

        // An agent's desk root with the call on a child: attributed to the root's desk.
        let (desk, child) = (SessionId::new(), SessionId::new());
        store
            .create_session(desk, "m", None, None, None)
            .await
            .unwrap();
        store.set_desk(desk, Some("builder")).await.unwrap();
        store
            .create_session(child, "m", Some(desk), Some("sub"), None)
            .await
            .unwrap();
        charge(&store, child, obs(Some(usd(2.0)), false)).await;

        // A scheduler root: `unattended`, titled `<name> · <subject>`.
        let routine = SessionId::new();
        store
            .create_session(routine, "m", None, None, None)
            .await
            .unwrap();
        store.set_unattended(routine).await.unwrap();
        store.set_title(routine, "nightly · report").await.unwrap();
        charge(&store, routine, obs(Some(usd(0.25)), false)).await;

        // A root whose session row is gone but whose ledger remains.
        let gone = SessionId::new();
        store
            .create_session(gone, "m", None, None, None)
            .await
            .unwrap();
        charge(&store, gone, obs(Some(usd(0.1)), false)).await;
        sqlx::query("DELETE FROM sessions WHERE id = ?")
            .bind(gone.to_string())
            .execute(&store.read)
            .await
            .unwrap();

        let r = store.spend_report(now_ms, 7, &[]).await.unwrap();
        assert_eq!(r.days.len(), 7);
        assert_eq!(r.days[6].day, today.format("%Y-%m-%d").to_string());
        assert_eq!(
            r.days[0].day,
            (today - chrono::Days::new(6))
                .format("%Y-%m-%d")
                .to_string()
        );
        let t = &r.days[6];
        assert!((t.usd - 3.35).abs() < 1e-9, "{t:?}");
        assert_eq!((t.calls, t.unresolved), (6, 1));
        let y = &r.days[5];
        assert!((y.usd - 0.5).abs() < 1e-9, "{y:?}");
        assert_eq!((y.calls, y.unresolved), (1, 0));
        assert!(r.days[..5].iter().all(|d| d.usd == 0.0 && d.calls == 0));

        // Month to date: today's rows, yesterday's only when it is this month, never the old one.
        let yesterday_in_month =
            yesterday.with_timezone(&Local).date_naive().month() == today.month();
        let month = 3.35 + if yesterday_in_month { 0.5 } else { 0.0 };
        assert_eq!(r.month.day, today.format("%Y-%m").to_string());
        assert!((r.month.usd - month).abs() < 1e-9, "{:?}", r.month);
        assert_eq!(r.month.unresolved, 1);

        let owners: Vec<(SpendOwnerKind, &str, f64, u32)> = r
            .owners
            .iter()
            .map(|o| (o.kind, o.name.as_str(), o.usd, o.calls))
            .collect();
        assert_eq!(
            owners,
            vec![
                (SpendOwnerKind::Agent, "builder", 2.0, 1),
                (SpendOwnerKind::User, "you", 1.5, 4),
                (SpendOwnerKind::Routine, "nightly", 0.25, 1),
                (SpendOwnerKind::Other, "other", 0.1, 1),
            ]
        );

        // `days` is clamped: 0 → one day (today), 99 → 31.
        assert_eq!(
            store.spend_report(now_ms, 0, &[]).await.unwrap().days.len(),
            1
        );
        assert_eq!(
            store
                .spend_report(now_ms, 99, &[])
                .await
                .unwrap()
                .days
                .len(),
            31
        );
    }

    /// 136 R-1: the cap is per night — spend before `since` is not counted.
    /// A $0.90 night followed by a fresh $1 cap: the first call goes out,
    /// settles at $0.20, and the call after it (estimate $0.20) is refused.
    #[tokio::test]
    async fn budget_spent_windows_from_since() {
        let (store, _dir) = fresh().await;
        let s = SessionId::new();
        store.ensure_owner(s, None).await.unwrap();
        charge(
            &store,
            s,
            CostObservation {
                returned: Some(usd(0.90)),
                ..Default::default()
            },
        )
        .await;
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let since = now();
        let cap = 1_000_000_000;
        let (spent, est) = store.budget_spent(s, "USD", "m", &since).await.unwrap();
        assert_eq!(spent, 0, "last night's $0.90 is not tonight's");
        assert!(spent + est <= cap, "the first call of the night goes out");
        charge(
            &store,
            s,
            CostObservation {
                returned: Some(usd(0.20)),
                ..Default::default()
            },
        )
        .await;
        let (spent, est) = store.budget_spent(s, "USD", "m", &since).await.unwrap();
        assert_eq!((spent, est), (200_000_000, 200_000_000));
        // The lifetime view (since the epoch) still sees both rows.
        let (all, _) = store
            .budget_spent(s, "USD", "m", "2000-01-01T00:00:00+00:00")
            .await
            .unwrap();
        assert_eq!(all, 1_100_000_000);
    }

    /// 136 R-27: a call in flight counts one estimate toward the spend, so
    /// concurrent side calls cannot each pass the guard against the same total.
    #[tokio::test]
    async fn budget_spent_counts_in_flight_calls_at_the_estimate() {
        let (store, _dir) = fresh().await;
        let s = SessionId::new();
        store.ensure_owner(s, None).await.unwrap();
        let since = "2000-01-01T00:00:00+00:00";
        charge(
            &store,
            s,
            CostObservation {
                returned: Some(usd(0.30)),
                ..Default::default()
            },
        )
        .await;
        let a = store
            .reserve_call(s, "r1", 0, None, CallKind::Dream, Some("p"), Some("m"))
            .await
            .unwrap();
        store.mark_dispatched(&a).await.unwrap();
        let _pending = store
            .reserve_call(s, "r2", 0, None, CallKind::Dream, Some("p"), Some("m"))
            .await
            .unwrap();
        let (spent, est) = store.budget_spent(s, "USD", "m", since).await.unwrap();
        assert_eq!(est, 300_000_000);
        assert_eq!(spent, 900_000_000, "settled + 2 in flight × estimate");
        // Settling the dispatched row swaps its estimate for the real cost.
        store
            .settle_call(
                &a,
                CallStatus::Settled,
                CostObservation {
                    returned: Some(usd(0.10)),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let (spent, est) = store.budget_spent(s, "USD", "m", since).await.unwrap();
        assert_eq!(est, 100_000_000, "newest settled attempt");
        assert_eq!(spent, 300_000_000 + 100_000_000 + 100_000_000);
    }
}
