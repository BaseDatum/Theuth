//! Where a run executes (126 §8.4): `double.training.venue` is `auto | local
//! | node:<name> | off`. `auto` picks a reachable mesh engine whose
//! `EngineStats` says `gpu = true` with a training-grade `accelerator`
//! (`cuda` | `metal`; `cpu` means "probed, nothing usable") and whose
//! class is neither `Laptop` nor `Vm` (Revision 1 ruling 5: a sandbox VM is
//! the laptop's own GPU behind a hypervisor — never), preferring `cuda`, then
//! `metal`, then the higher 30-day availability; else `local`. Bundles leave
//! the laptop only when `training.share_data` is on. Tier A is always local
//! (the scheduler decides that; this module only resolves names).

use std::fmt;

use mesh::{Accelerator, EngineClass, EngineStats, Peer};
use replica::PeerId;

use crate::types::Venue;

/// A resolved venue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolved {
    /// This engine.
    Local,
    /// A reachable mesh engine.
    Node {
        /// Its display name.
        name: String,
        /// Its endpoint id, as the transport names it.
        peer: PeerId,
    },
    /// An explicitly named node that is not in the mesh or not reachable.
    Unreachable(String),
    /// Never train.
    Off,
}

impl fmt::Display for Resolved {
    /// `local` | `node:<name>` | `off` — what `runs.venue` and the events carry.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Local => f.write_str("local"),
            Self::Node { name, .. } | Self::Unreachable(name) => write!(f, "node:{name}"),
            Self::Off => f.write_str("off"),
        }
    }
}

/// The effective request: an explicit `requested` wins; `Auto` defers to the
/// configured string (`Venue::parse`; unparsable → `Auto`).
pub fn effective(requested: &Venue, configured: &str) -> Venue {
    match requested {
        Venue::Auto => Venue::parse(configured).unwrap_or(Venue::Auto),
        other => other.clone(),
    }
}

/// Pure chooser over a mesh snapshot.
pub fn choose(
    requested: &Venue,
    configured: &str,
    share_data: bool,
    peers: &[Peer],
    stats: &[EngineStats],
) -> Resolved {
    match effective(requested, configured) {
        Venue::Off => Resolved::Off,
        Venue::Local => Resolved::Local,
        Venue::Node(name) => peers
            .iter()
            .find(|p| p.name == name && p.reachable)
            .map_or(Resolved::Unreachable(name), |p| Resolved::Node {
                name: p.name.clone(),
                peer: PeerId(p.id.0.clone()),
            }),
        Venue::Auto => {
            if !share_data {
                return Resolved::Local;
            }
            let mut candidates: Vec<(&Peer, &EngineStats)> = peers
                .iter()
                .filter(|p| p.reachable)
                .filter_map(|p| stats.iter().find(|s| s.id == p.id).map(|s| (p, s)))
                .filter(|(_, s)| {
                    s.gpu
                        && matches!(s.accelerator, Some(Accelerator::Cuda | Accelerator::Metal))
                        && !matches!(s.class, EngineClass::Laptop | EngineClass::Vm)
                })
                .collect();
            candidates.sort_by(|a, b| {
                rank(a.1.accelerator)
                    .cmp(&rank(b.1.accelerator))
                    .then(b.1.availability_30d.total_cmp(&a.1.availability_30d))
                    .then(a.0.name.cmp(&b.0.name))
            });
            candidates.first().map_or(Resolved::Local, |(p, _)| Resolved::Node {
                name: p.name.clone(),
                peer: PeerId(p.id.0.clone()),
            })
        }
    }
}

/// Lower is better: `cuda`, then `metal`; `cpu` is not a training venue.
fn rank(a: Option<Accelerator>) -> u8 {
    match a {
        Some(Accelerator::Cuda) => 0,
        Some(Accelerator::Metal) => 1,
        _ => 9,
    }
}

/// [`choose`] over the live mesh (`mesh::Status` + `mesh::ListStats`); a
/// one-engine mesh or no mesh actor means `local`.
pub async fn resolve(requested: &Venue, configured: &str, share_data: bool) -> Resolved {
    let (peers, stats) = match mesh::actor() {
        Some(m) => {
            let peers = m
                .ask(mesh::Status)
                .await
                .map(|s| s.peers)
                .unwrap_or_default();
            let stats = m.ask(mesh::ListStats).await.unwrap_or_default();
            (peers, stats)
        }
        None => (Vec::new(), Vec::new()),
    };
    choose(requested, configured, share_data, &peers, &stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mesh::{EngineId, HubPref};

    fn peer(name: &str, reachable: bool) -> Peer {
        Peer {
            id: EngineId(format!("id-{name}")),
            name: name.into(),
            addrs: vec![],
            reachable,
            last_seen_ms: 0,
            hub_pref: HubPref::Eligible,
        }
    }

    fn st(name: &str, class: EngineClass, gpu: bool, acc: Option<Accelerator>, avail: f32) -> EngineStats {
        EngineStats {
            id: EngineId(format!("id-{name}")),
            uptime_since_ms: 0,
            class,
            availability_30d: avail,
            harnesses: vec![],
            repos: vec![],
            display: false,
            gpu,
            accelerator: acc,
        }
    }

    #[test]
    fn auto_prefers_a_reachable_cuda_server_never_a_laptop_or_vm() {
        let peers = vec![
            peer("mac2", true),
            peer("vmgpu", true),
            peer("box", true),
            peer("rig", false),
            peer("studio", true),
        ];
        let stats = vec![
            st("mac2", EngineClass::Laptop, true, Some(Accelerator::Metal), 1.0),
            st("vmgpu", EngineClass::Vm, true, Some(Accelerator::Cuda), 1.0),
            st("box", EngineClass::Server, true, Some(Accelerator::Cuda), 0.9),
            st("rig", EngineClass::Server, true, Some(Accelerator::Cuda), 1.0),
            st("studio", EngineClass::Desktop, true, Some(Accelerator::Metal), 1.0),
        ];
        let r = choose(&Venue::Auto, "auto", true, &peers, &stats);
        assert_eq!(
            r,
            Resolved::Node {
                name: "box".into(),
                peer: PeerId("id-box".into())
            }
        );
        assert_eq!(r.to_string(), "node:box");
        // Without the CUDA box, the Metal desktop wins over the laptop.
        let r = choose(&Venue::Auto, "auto", true, &peers[..2].iter().chain(&peers[3..]).cloned().collect::<Vec<_>>(), &stats);
        assert_eq!(r.to_string(), "node:studio");
        // `gpu = true` without an accelerator, or `cpu`, is not enough.
        let weak = vec![
            st("box", EngineClass::Server, true, None, 1.0),
            st("studio", EngineClass::Server, true, Some(Accelerator::Cpu), 1.0),
        ];
        assert_eq!(choose(&Venue::Auto, "auto", true, &peers, &weak), Resolved::Local);
        // share_data off → never leaves the laptop.
        assert_eq!(choose(&Venue::Auto, "auto", false, &peers, &stats), Resolved::Local);
        // One-engine mesh → local.
        assert_eq!(choose(&Venue::Auto, "auto", true, &[], &[]), Resolved::Local);
    }

    #[test]
    fn explicit_requests_win_over_the_config() {
        let peers = vec![peer("box", true), peer("down", false)];
        assert_eq!(choose(&Venue::Local, "node:box", true, &peers, &[]), Resolved::Local);
        assert_eq!(choose(&Venue::Off, "auto", true, &peers, &[]), Resolved::Off);
        assert_eq!(choose(&Venue::Auto, "off", true, &peers, &[]), Resolved::Off);
        assert_eq!(choose(&Venue::Auto, "local", true, &peers, &[]), Resolved::Local);
        assert_eq!(
            choose(&Venue::Auto, "node:box", false, &peers, &[]),
            Resolved::Node {
                name: "box".into(),
                peer: PeerId("id-box".into())
            },
            "an explicit node needs no stats"
        );
        assert_eq!(choose(&Venue::Node("down".into()), "auto", true, &peers, &[]), Resolved::Unreachable("down".into()));
        assert_eq!(choose(&Venue::Node("ghost".into()), "auto", true, &peers, &[]), Resolved::Unreachable("ghost".into()));
        assert_eq!(choose(&Venue::Auto, "garbage", true, &peers, &[]), Resolved::Local);
    }
}
