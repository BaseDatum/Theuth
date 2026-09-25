//! Election by stability (67 §4.3, §4.5): a pure function every engine runs
//! over the same inputs, so no message ever says "you are hub". [`Follower`]
//! is the other half: how a plugin tracks whether *this* engine holds the
//! lease (67 §4.4; roster, slack, signal, link each keep one).

use crate::types::{EngineClass, EngineId, HubPref, LeaseState};

/// Where this engine stands with the lease. Duties are on until the mesh
/// says otherwise: an engine without a mesh is its own hub.
#[derive(Clone, Debug)]
pub struct Follower {
    /// This engine's id, once the mesh answered `Status`.
    pub self_id: Option<String>,
    /// Whether the duties are on.
    pub holding: bool,
    /// The last lease seen: `Some(holder)` once one arrived.
    seen: Option<Option<String>>,
}

impl Default for Follower {
    fn default() -> Self {
        Self {
            self_id: None,
            holding: true,
            seen: None,
        }
    }
}

impl Follower {
    /// The engine id is known: re-evaluate the last lease seen.
    /// `Some(true)` = just gained, `Some(false)` = just lost, `None` = unchanged.
    pub fn set_self(&mut self, id: String) -> Option<bool> {
        self.self_id = Some(id);
        self.settle()
    }

    /// A lease event. `Some(true)` = just gained (holder is us), `Some(false)`
    /// = just lost (someone else or nobody holds), `None` = unchanged. Before
    /// `self_id` is known nothing changes: the lease is remembered and
    /// [`Self::set_self`] resolves it.
    pub fn observe(&mut self, lease: &LeaseState) -> Option<bool> {
        self.seen = Some(lease.holder.as_ref().map(|h| h.0.clone()));
        self.settle()
    }

    fn settle(&mut self) -> Option<bool> {
        let me = self.self_id.as_deref()?;
        let holder = self.seen.as_ref()?;
        let want = holder.as_deref() == Some(me);
        if want == self.holding {
            return None;
        }
        self.holding = want;
        Some(want)
    }
}

/// One connected, heartbeating engine (this one included) as the election sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    /// Endpoint id (the final tie-break, ascending).
    pub id: EngineId,
    /// `Prefer` first; `Never` is filtered out.
    pub hub_pref: HubPref,
    /// Peer-observed availability over 30 days, 0..=1.
    pub availability_30d: f32,
    /// Machine class; `Server` and `Container` rank equal and highest.
    pub class: EngineClass,
    /// Process start (ms); older = more uptime.
    pub uptime_since_ms: u64,
    /// Identity creation (ms); the probation clock.
    pub first_seen_ms: u64,
    /// Since when it has been continuously connected to this engine (ms); the
    /// handover-grace clock. For self: process start.
    pub connected_since_ms: u64,
}

/// Availability bucketed so noise does not flip elections: 3 = ≥ 99.9 %,
/// 2 = ≥ 99 %, 1 = ≥ 95 %, 0 = less.
pub fn bucket(availability: f32) -> u8 {
    if availability >= 0.999 {
        3
    } else if availability >= 0.99 {
        2
    } else if availability >= 0.95 {
        1
    } else {
        0
    }
}

/// `Server` = `Container` > `Desktop` > `Laptop`.
pub fn class_rank(c: EngineClass) -> u8 {
    match c {
        EngineClass::Server | EngineClass::Container => 2,
        EngineClass::Desktop => 1,
        EngineClass::Laptop => 0,
    }
}

/// Sort key: larger is better except `id`, which breaks ties ascending.
fn key(c: &Candidate) -> (bool, u8, u8, std::cmp::Reverse<u64>, std::cmp::Reverse<String>) {
    (
        c.hub_pref == HubPref::Prefer,
        bucket(c.availability_30d),
        class_rank(c.class),
        std::cmp::Reverse(c.uptime_since_ms),
        std::cmp::Reverse(c.id.0.clone()),
    )
}

/// Who holds the lease. `incumbent` is the holder as this engine last
/// computed it. Guards against flapping (67 §4.5): a candidate first seen less
/// than `probation_ms` ago cannot outrank the incumbent, and a better candidate
/// must have been connected for `handover_ms` before the incumbent yields.
/// Both guards apply only while the incumbent is itself still a candidate.
pub fn elect(
    candidates: &[Candidate],
    incumbent: Option<&EngineId>,
    now_ms: u64,
    probation_ms: u64,
    handover_ms: u64,
) -> Option<EngineId> {
    let mut ranked: Vec<&Candidate> = candidates.iter().filter(|c| c.hub_pref != HubPref::Never).collect();
    ranked.sort_by(|a, b| key(b).cmp(&key(a)));
    let incumbent = incumbent.filter(|id| ranked.iter().any(|c| &c.id == *id));
    let Some(inc) = incumbent else {
        return ranked.first().map(|c| c.id.clone());
    };
    for c in &ranked {
        if &c.id == inc {
            return Some(inc.clone());
        }
        let proven = now_ms.saturating_sub(c.first_seen_ms) >= probation_ms;
        let stable = now_ms.saturating_sub(c.connected_since_ms) >= handover_ms;
        if proven && stable {
            return Some(c.id.clone());
        }
    }
    Some(inc.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: u64 = 3_600_000;
    const NOW: u64 = 100 * 24 * HOUR;
    const PROBATION: u64 = 6 * HOUR;
    const HANDOVER: u64 = 10 * 60_000;

    fn cand(id: &str, class: EngineClass) -> Candidate {
        Candidate {
            id: EngineId(id.into()),
            hub_pref: HubPref::Eligible,
            availability_30d: 1.0,
            class,
            uptime_since_ms: NOW - 24 * HOUR,
            first_seen_ms: NOW - 30 * 24 * HOUR,
            connected_since_ms: NOW - 24 * HOUR,
        }
    }

    fn id(s: &str) -> Option<EngineId> {
        Some(EngineId(s.into()))
    }

    fn lease(holder: Option<&str>) -> LeaseState {
        LeaseState {
            holder: holder.map(|h| EngineId(h.to_owned())),
            since_ms: 0,
            reason: "test".into(),
        }
    }

    #[test]
    fn follower_tracks_the_lease() {
        let mut f = Follower::default();
        assert!(f.holding, "duties on until the mesh says otherwise");
        assert_eq!(f.observe(&lease(Some("b"))), None, "unknown self: remembered, nothing moves");
        assert!(f.holding);
        assert_eq!(f.set_self("a".into()), Some(false), "self known: someone else holds");
        assert_eq!(f.observe(&lease(Some("a"))), Some(true));
        assert_eq!(f.observe(&lease(Some("a"))), None, "same holder again is no change");
        assert_eq!(f.observe(&lease(Some("b"))), Some(false));
        assert_eq!(f.observe(&lease(None)), None, "nobody after someone else: still lost");
        assert_eq!(f.observe(&lease(Some("a"))), Some(true));
    }

    #[test]
    fn set_self_without_a_lease_is_no_change() {
        let mut f = Follower::default();
        assert_eq!(f.set_self("a".into()), None);
        assert!(f.holding);
        assert_eq!(f.observe(&lease(None)), Some(false), "holder None is a loss");
        assert_eq!(f.observe(&lease(None)), None);
    }

    #[test]
    fn server_beats_laptop_regardless_of_id() {
        let c = [cand("a-laptop", EngineClass::Laptop), cand("z-server", EngineClass::Server)];
        assert_eq!(elect(&c, None, NOW, PROBATION, HANDOVER), id("z-server"));
        let c = [cand("z-laptop", EngineClass::Laptop), cand("a-server", EngineClass::Server)];
        assert_eq!(elect(&c, None, NOW, PROBATION, HANDOVER), id("a-server"));
        // Container ranks as server; equal class → older uptime, then id.
        let mut older = cand("b", EngineClass::Container);
        older.uptime_since_ms -= HOUR;
        assert_eq!(elect(&[cand("a", EngineClass::Server), older], None, NOW, PROBATION, HANDOVER), id("b"));
        assert_eq!(
            elect(&[cand("b", EngineClass::Server), cand("a", EngineClass::Server)], None, NOW, PROBATION, HANDOVER),
            id("a")
        );
    }

    #[test]
    fn prefer_beats_everything() {
        let mut laptop = cand("laptop", EngineClass::Laptop);
        laptop.hub_pref = HubPref::Prefer;
        laptop.availability_30d = 0.5;
        let c = [cand("server", EngineClass::Server), laptop];
        assert_eq!(elect(&c, id("server").as_ref(), NOW, PROBATION, HANDOVER), id("laptop"));
        // Never is out even when alone.
        let mut never = cand("never", EngineClass::Server);
        never.hub_pref = HubPref::Never;
        assert_eq!(elect(&[never], None, NOW, PROBATION, HANDOVER), None);
    }

    #[test]
    fn probation_blocks_a_newcomer() {
        let mut newcomer = cand("server", EngineClass::Server);
        newcomer.first_seen_ms = NOW - HOUR;
        let c = [cand("laptop", EngineClass::Laptop), newcomer.clone()];
        // With an incumbent the newcomer waits; without one it wins outright.
        assert_eq!(elect(&c, id("laptop").as_ref(), NOW, PROBATION, HANDOVER), id("laptop"));
        assert_eq!(elect(&c, None, NOW, PROBATION, HANDOVER), id("server"));
        // Probation over → takes it.
        assert_eq!(elect(&c, id("laptop").as_ref(), NOW + PROBATION, PROBATION, HANDOVER), id("server"));
    }

    #[test]
    fn grace_delays_handover() {
        let mut better = cand("server", EngineClass::Server);
        better.connected_since_ms = NOW - 60_000;
        let c = [cand("desktop", EngineClass::Desktop), better];
        assert_eq!(elect(&c, id("desktop").as_ref(), NOW, PROBATION, HANDOVER), id("desktop"));
        assert_eq!(elect(&c, id("desktop").as_ref(), NOW + HANDOVER, PROBATION, HANDOVER), id("server"));
        // An incumbent that is gone (not a candidate) yields at once.
        assert_eq!(elect(&c[1..], id("desktop").as_ref(), NOW, PROBATION, HANDOVER), id("server"));
    }

    #[test]
    fn lone_laptop_is_hub() {
        let mut lone = cand("laptop", EngineClass::Laptop);
        lone.availability_30d = 0.3;
        lone.first_seen_ms = NOW;
        assert_eq!(elect(&[lone], None, NOW, PROBATION, HANDOVER), id("laptop"));
        assert_eq!(elect(&[], None, NOW, PROBATION, HANDOVER), None);
    }

    #[test]
    fn buckets() {
        assert_eq!(bucket(1.0), 3);
        assert_eq!(bucket(0.995), 2);
        assert_eq!(bucket(0.96), 1);
        assert_eq!(bucket(0.9), 0);
    }
}
