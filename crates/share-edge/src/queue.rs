//! The certificate slot queue (210 §6.5, F-M9, F-M8): per-CA token buckets split into an invited and an open
//! class (`acme.invite_share`), an invited token unused for 24 h spilling into the open class, the fair order
//! within a class (every mesh's first name before any additional name; round-robin across network keys, IPv4
//! /24 or IPv6 /48, each key one slot per round in order of its oldest job; FIFO within a key), position and
//! ETA, and `names_max` counted at slot time with eviction of never-published names and the `edge full` state.
//! Renewals and rotations never draw on it: only a `new` job of a name not yet `slotted` comes here.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use share_proto::events::QueueEvent;
use share_proto::{Event, PublicKey, QueuePos, queue_eta_s};

use crate::limits::{DAY_MS, WEEK_MS, wide_key};
use crate::state::Edge;
use crate::store::{BucketSpec, JobRow};

/// An invited token unused this long may go to the open class.
pub const SPILL_AFTER_MS: u64 = DAY_MS;
/// Retry of a job waiting on `names_max`.
const FULL_RETRY_MS: u64 = 10 * 60_000;

/// The §6.5 order of one class: first names, then additional names; inside each, round-robin over network
/// keys ordered by their oldest job, FIFO within a key. `jobs` must be oldest first.
pub fn order<'a>(jobs: &'a [JobRow], class: &str) -> Vec<&'a JobRow> {
    let mut out = Vec::new();
    for first in [1, 0] {
        // Keys in order of their oldest job (BTreeMap keyed by first-seen index), FIFO lists per key.
        let mut seen: HashMap<String, usize> = HashMap::new();
        let mut lanes: BTreeMap<usize, Vec<&JobRow>> = BTreeMap::new();
        for j in jobs.iter().filter(|j| j.class == class && (j.first_of_mesh != 0) == (first == 1)) {
            let n = seen.len();
            let idx = *seen.entry(wide_key(&j.net)).or_insert(n);
            lanes.entry(idx).or_default().push(j);
        }
        let rounds = lanes.values().map(Vec::len).max().unwrap_or(0);
        for r in 0..rounds {
            out.extend(lanes.values().filter_map(|l| l.get(r).copied()));
        }
    }
    out
}

/// The bucket of a CA's class: invited = ⌈per_week × invite_share⌉, open = the rest (each at least 1), one
/// token per 7 days ÷ size.
pub fn bucket(edge: &Edge, ca: &str, class: &str) -> BucketSpec {
    let per_week = f64::from(edge.cfg.acme.cas.letsencrypt.new_certs_per_week.max(2));
    let share = edge.cfg.acme.invite_share.clamp(0.0, 1.0);
    let invited = (per_week * share).ceil().clamp(1.0, per_week - 1.0);
    let cap = if class == "invited" { invited } else { per_week - invited };
    BucketSpec {
        name: format!("{ca}_{class}"),
        cap,
        #[allow(clippy::cast_precision_loss)]
        refill_ms: WEEK_MS as f64 / cap,
    }
}

fn spill_mark(ca: &str) -> String {
    format!("{ca}_invited_taken")
}

/// One class's queue at a moment.
#[derive(Debug, Clone)]
pub struct ClassView {
    /// Job ids in §6.5 order.
    pub ids: Vec<String>,
    /// Whole tokens available (with the spill for the open class).
    pub tokens: u32,
    /// Seconds per token.
    pub refill_s: u64,
    /// Seconds to the next token (0 when one is available).
    pub next_token_s: u64,
    /// The open class may take an invited token now.
    pub spill: bool,
}

impl ClassView {
    /// Position (1-based) and ETA of a job.
    pub fn pos(&self, id: &str) -> Option<QueuePos> {
        let p = u32::try_from(self.ids.iter().position(|i| i == id)? + 1).unwrap_or(u32::MAX);
        Some(QueuePos {
            position: p,
            eta_s: queue_eta_s(p, self.tokens, self.refill_s, self.next_token_s),
        })
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn whole(t: f64) -> u32 {
    t.max(0.0).floor() as u32
}

/// The view of `class` over `jobs` (as from [`crate::store::Store::slot_jobs`]).
pub async fn view(edge: &Edge, ca: &str, class: &str, jobs: &[JobRow], now_ms: u64) -> anyhow::Result<ClassView> {
    let b = bucket(edge, ca, class);
    let own = edge.store.bucket_tokens(&b, now_ms).await?;
    let mut tokens = whole(own);
    let mut spill = false;
    if class == "open" && tokens == 0 && !jobs.iter().any(|j| j.class == "invited") {
        // The 24 h run from the last invited take, or from the first time the edge looked (a fresh edge
        // does not hand its invite share to the open queue at once).
        let last = match edge.store.marked(&spill_mark(ca)).await? {
            Some(t) => t,
            None => {
                edge.store.mark(&spill_mark(ca), now_ms).await?;
                now_ms
            }
        };
        let inv = bucket(edge, ca, "invited");
        let inv_tokens = whole(edge.store.bucket_tokens(&inv, now_ms).await?);
        if now_ms.saturating_sub(last) >= SPILL_AFTER_MS && inv_tokens > 0 {
            tokens = inv_tokens;
            spill = true;
        }
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let next_token_s = if tokens > 0 {
        0
    } else {
        (((1.0 - own.fract()) * b.refill_ms) / 1000.0).ceil() as u64
    };
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let refill_s = (b.refill_ms / 1000.0).ceil() as u64;
    Ok(ClassView {
        ids: order(jobs, class).into_iter().map(|j| j.id.clone()).collect(),
        tokens,
        refill_s,
        next_token_s,
        spill,
    })
}

/// The answer of [`try_slot`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Slot {
    /// The job holds a slot now.
    Slotted,
    /// Not yet: its position, ETA, and when to look again.
    Wait {
        /// Position and ETA.
        pos: QueuePos,
        /// Ms until the runner should lease it again.
        retry_ms: u64,
    },
}

/// Give `job` a slot if its turn has come (§6.5) and `names_max` allows (§9.4): at `names_max` evict one name
/// never live `evict_unpublished_after` after allocation, else mark the edge full and wait.
pub async fn try_slot(edge: &Arc<Edge>, ca: &str, job: &JobRow, now_ms: u64) -> anyhow::Result<Slot> {
    let domain = edge.cfg.domain.as_str();
    let jobs = edge.store.slot_jobs(domain).await?;
    let v = view(edge, ca, &job.class, &jobs, now_ms).await?;
    let pos = v.pos(&job.id).unwrap_or(QueuePos {
        position: 1,
        eta_s: 0,
    });
    if !has_room(edge, now_ms).await? {
        return Ok(Slot::Wait {
            pos,
            retry_ms: FULL_RETRY_MS,
        });
    }
    let wait = |p: QueuePos, v: &ClassView| {
        let ahead_short = u64::from(p.position.saturating_sub(v.tokens.max(1)));
        let retry = (v.next_token_s + ahead_short * v.refill_s).max(1) * 1000;
        Slot::Wait { pos: p, retry_ms: retry }
    };
    if pos.position > v.tokens {
        return Ok(wait(pos, &v));
    }
    let from = if v.spill { "invited" } else { job.class.as_str() };
    let b = bucket(edge, ca, from);
    if edge.store.buckets_take(&[b], now_ms).await?.is_err() {
        return Ok(wait(pos, &v));
    }
    if from == "invited" {
        edge.store.mark(&spill_mark(ca), now_ms).await?;
    }
    Ok(Slot::Slotted)
}

/// Whether one more name may hold a slot: under `names_max`, or after evicting one never-published name.
/// Sets and clears the edge's full state.
pub async fn has_room(edge: &Arc<Edge>, now_ms: u64) -> anyhow::Result<bool> {
    let domain = edge.cfg.domain.as_str();
    let max = u64::from(edge.cfg.limits.names_max);
    if edge.store.slotted_count(domain).await? < max {
        edge.set_full(false);
        return Ok(true);
    }
    let after = u64::try_from(edge.cfg.limits.evict_unpublished_after.as_millis()).unwrap_or(u64::MAX);
    for name in edge.store.unpublished_names(domain, now_ms.saturating_sub(after)).await? {
        if evict(edge, &name, true, now_ms).await? && edge.store.slotted_count(domain).await? < max {
            edge.set_full(false);
            return Ok(true);
        }
    }
    edge.set_full(true);
    Ok(false)
}

/// Evict a name (§9.4): row deleted (no cooldown), chain revoked, records and routes removed.
pub async fn evict(edge: &Arc<Edge>, name: &str, only_unpublished: bool, now_ms: u64) -> anyhow::Result<bool> {
    let Some(owner) = edge
        .store
        .evict(&edge.cfg.domain, name, only_unpublished, now_ms)
        .await?
    else {
        return Ok(false);
    };
    tracing::info!(name, %owner, "evicted");
    edge.drop_routes(|_, r| r.name == name);
    crate::k8s::gone(edge, name);
    crate::certs::retire(edge, name, crate::acme::Reason::CessationOfOperation).await;
    Ok(true)
}

/// Position and ETA of a job waiting for a slot, if it is one.
pub async fn position(edge: &Edge, ca: &str, job: &JobRow, now_ms: u64) -> anyhow::Result<Option<QueuePos>> {
    let jobs = edge.store.slot_jobs(&edge.cfg.domain).await?;
    Ok(view(edge, ca, &job.class, &jobs, now_ms).await?.pos(&job.id))
}

/// Positions of every waiting name of `mesh` (for `welcome.names[].queue`).
pub async fn mesh_positions(edge: &Edge, ca: &str, mesh: &str, now_ms: u64) -> anyhow::Result<HashMap<String, QueuePos>> {
    let jobs = edge.store.slot_jobs(&edge.cfg.domain).await?;
    let mut out = HashMap::new();
    for class in ["invited", "open"] {
        if !jobs.iter().any(|j| j.class == class && j.mesh == mesh) {
            continue;
        }
        let v = view(edge, ca, class, &jobs, now_ms).await?;
        for j in jobs.iter().filter(|j| j.class == class && j.mesh == mesh) {
            if let Some(p) = v.pos(&j.id) {
                out.insert(j.name.clone(), p);
            }
        }
    }
    Ok(out)
}

/// Send a `queue` event to every waiting name's mesh whose position changed since the last call (`last`:
/// job id → position, kept by the caller).
pub async fn announce(
    edge: &Edge,
    ca: &str,
    now_ms: u64,
    last: &std::sync::Mutex<HashMap<String, u32>>,
) -> anyhow::Result<()> {
    let jobs = edge.store.slot_jobs(&edge.cfg.domain).await?;
    let mut now_pos = HashMap::new();
    for class in ["invited", "open"] {
        let v = view(edge, ca, class, &jobs, now_ms).await?;
        for j in jobs.iter().filter(|j| j.class == class) {
            if let Some(p) = v.pos(&j.id) {
                now_pos.insert(j.id.clone(), (j, p));
            }
        }
    }
    let changed: Vec<(&JobRow, QueuePos)> = {
        let mut last = last.lock().expect("queue positions");
        let out = now_pos
            .values()
            .filter(|(j, p)| last.get(&j.id) != Some(&p.position))
            .map(|(j, p)| (*j, *p))
            .collect();
        *last = now_pos.iter().map(|(id, (_, p))| (id.clone(), p.position)).collect();
        out
    };
    for (j, p) in changed {
        let Ok(mesh) = j.mesh.parse::<PublicKey>() else { continue };
        for t in edge.mesh_tunnels(&mesh) {
            t.send(Event::Queue(QueueEvent {
                name: j.name.clone(),
                position: p.position,
                eta_s: p.eta_s,
            }));
        }
    }
    Ok(())
}

/// Every minute (§9.4, §9.2): evict `allocated` names without a CSR after `allocated_ttl`, and purge
/// `conn_log` rows older than `conn_log_retention` (30 days).
pub async fn housekeeping(edge: &Arc<Edge>, now_ms: u64) -> anyhow::Result<()> {
    let l = &edge.cfg.limits;
    let ttl = u64::try_from(l.allocated_ttl.as_millis()).unwrap_or(u64::MAX);
    for name in edge.store.stale_allocated(&edge.cfg.domain, now_ms.saturating_sub(ttl)).await? {
        evict(edge, &name, false, now_ms).await?;
    }
    let keep = u64::try_from(l.conn_log_retention.as_millis()).unwrap_or(u64::MAX);
    let n = edge.store.purge_conn_log(now_ms.saturating_sub(keep)).await?;
    if n > 0 {
        tracing::debug!(rows = n, "conn_log purged");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(id: &str, class: &str, mesh: &str, net: &str, first: bool, at: i64) -> JobRow {
        JobRow {
            id: id.into(),
            domain: "tun.zip".into(),
            name: id.into(),
            kind: "new".into(),
            state: "queued".into(),
            class: class.into(),
            mesh: mesh.into(),
            net: net.into(),
            first_of_mesh: i64::from(first),
            enqueued_ms: at,
            attempts: 0,
            next_at_ms: at,
            last_error: None,
        }
    }

    #[test]
    fn first_names_then_round_robin_by_network_then_fifo() {
        let jobs = vec![
            // One /24 floods first: three additional names and one first name.
            job("a1", "open", "ma", "203.0.113.1", false, 1),
            job("a2", "open", "mb", "203.0.113.2", false, 2),
            job("a3", "open", "mc", "203.0.113.3", false, 3),
            job("b1", "open", "md", "198.51.100.9", false, 4),
            job("f1", "open", "me", "203.0.113.4", true, 5),
            job("f2", "open", "mf", "2001:db8:1:2::", true, 6),
            job("f3", "open", "mg", "2001:db8:1:3::", true, 7), // same /48 as f2
            job("i1", "invited", "mh", "203.0.113.5", false, 0),
        ];
        let ids: Vec<&str> = order(&jobs, "open").iter().map(|j| j.id.as_str()).collect();
        // Firsts: lanes 203.0.113/24 [f1], 2001:db8:1/48 [f2, f3]. Then additional: 203.0.113/24 [a1 a2 a3],
        // 198.51.100/24 [b1] → a1 b1 a2 a3.
        assert_eq!(ids, ["f1", "f2", "f3", "a1", "b1", "a2", "a3"]);
        let ids: Vec<&str> = order(&jobs, "invited").iter().map(|j| j.id.as_str()).collect();
        assert_eq!(ids, ["i1"]);
    }

    #[test]
    fn eta_counts_tokens_refill_and_jobs_ahead() {
        let v = ClassView {
            ids: vec!["x".into(), "y".into(), "z".into()],
            tokens: 1,
            refill_s: 26_297,
            next_token_s: 100,
            spill: false,
        };
        assert_eq!(v.pos("x"), Some(QueuePos { position: 1, eta_s: 0 }));
        // y: one short → next token, plus one order ahead.
        assert_eq!(v.pos("y").unwrap().eta_s, 100 + 180);
        // z: two short → next token + one refill, plus two orders ahead.
        assert_eq!(v.pos("z").unwrap().eta_s, 100 + 26_297 + 360);
        assert_eq!(v.pos("nope"), None);
    }
}
