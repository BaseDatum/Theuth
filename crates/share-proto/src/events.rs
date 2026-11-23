//! Server-sent events of `GET /v1/events` (§5.3): the SSE `event:` field is [`Event::name`], `data:` is the
//! payload's JSON. Event names are additive within a major: [`Event::parse`] returns `None` for an unknown one.

use iroh_base::PublicKey;
use serde::{Deserialize, Serialize};

/// `queue`: a queued name's position moved (§6.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueEvent {
    /// The name.
    pub name: String,
    /// 1-based position.
    pub position: u32,
    /// Estimated seconds until the slot.
    pub eta_s: u64,
}

/// `cert`: a new chain for `name`; fetch it with `GET /v1/names/{name}/cert`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CertEvent {
    /// The name.
    pub name: String,
}

/// `routes`: `host` is now held by `engine`, or freed when `engine` is absent (§5.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutesEvent {
    /// The host.
    pub host: String,
    /// The engine now holding it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<PublicKey>,
}

/// `stats`: every 30 s per host of this tunnel (§9.6); the edge's numbers are authoritative.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatsEvent {
    /// The host.
    pub host: String,
    /// Visitor connections in the period.
    pub conns: u64,
    /// Bytes both ways in the period.
    pub bytes: u64,
    /// Bytes from visitors to the engine in the period (`bytes = bytes_in + bytes_out` on an M3 edge).
    #[serde(default)]
    pub bytes_in: u64,
    /// Bytes from the engine to visitors in the period.
    #[serde(default)]
    pub bytes_out: u64,
    /// Distinct visitor addresses (/64 for IPv6) in the period.
    #[serde(default)]
    pub visitors: u64,
    /// Visitor connections open at the end of the period.
    #[serde(default)]
    pub open: u64,
    /// Length of the period, seconds (30 on an M3 edge; 0 = unknown, an older edge).
    #[serde(default)]
    pub period_s: u64,
}

/// `throttle`: the mesh's tunnels moved to or from `throttle_mbit` (egress above or back under the month's pace,
/// §9.6). The state at `hello` is `welcome.throttled`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThrottleEvent {
    /// Throttled now.
    pub throttled: bool,
    /// The per-tunnel rate now in force, Mbit/s.
    pub mbit: u32,
}

/// `drain`: the edge is restarting; keep serving, close at the deadline and redial after 1–5 s (§5.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DrainEvent {
    /// Seconds until the edge closes.
    pub secs: u64,
}

/// Why the edge killed a tunnel or a host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KillReason {
    /// Operator ban (§9.3).
    EdgeBan,
    /// A byte budget (§9.6).
    Budget,
    /// Stop all from an engine of the mesh (§7.4).
    StopAll,
    /// The engine was revoked (§5.7).
    Revoked,
    /// The mesh was rekeyed (§5.7).
    MeshRekeyed,
    /// The delegation expired without a renewed `hello`.
    DelegationExpired,
    /// `admin pause`: every tunnel closed, `hello` refused until `resume` (§9.3).
    Paused,
    /// A Safe Browsing or Search Console hit on the host, pending review (§9.5).
    Flagged,
    /// A reason this build does not know.
    #[serde(other)]
    Unknown,
}

/// `kill`: one host (`host` set) or the whole tunnel was closed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KillEvent {
    /// The host, or the whole tunnel when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// Why.
    pub reason: KillReason,
}

/// One event of the stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// `queue`.
    Queue(QueueEvent),
    /// `cert`.
    Cert(CertEvent),
    /// `routes`.
    Routes(RoutesEvent),
    /// `stats`.
    Stats(StatsEvent),
    /// `drain`.
    Drain(DrainEvent),
    /// `kill`.
    Kill(KillEvent),
    /// `throttle`.
    Throttle(ThrottleEvent),
}

impl Event {
    /// The SSE `event:` name.
    pub fn name(&self) -> &'static str {
        match self {
            Event::Queue(_) => "queue",
            Event::Cert(_) => "cert",
            Event::Routes(_) => "routes",
            Event::Stats(_) => "stats",
            Event::Drain(_) => "drain",
            Event::Kill(_) => "kill",
            Event::Throttle(_) => "throttle",
        }
    }

    /// The SSE `data:` JSON.
    pub fn data(&self) -> String {
        let r = match self {
            Event::Queue(e) => serde_json::to_string(e),
            Event::Cert(e) => serde_json::to_string(e),
            Event::Routes(e) => serde_json::to_string(e),
            Event::Stats(e) => serde_json::to_string(e),
            Event::Drain(e) => serde_json::to_string(e),
            Event::Kill(e) => serde_json::to_string(e),
            Event::Throttle(e) => serde_json::to_string(e),
        };
        r.expect("event payloads always serialise")
    }

    /// Decodes an SSE event; `Ok(None)` for an event name this build does not know.
    pub fn parse(name: &str, data: &str) -> Result<Option<Event>, serde_json::Error> {
        Ok(Some(match name {
            "queue" => Event::Queue(serde_json::from_str(data)?),
            "cert" => Event::Cert(serde_json::from_str(data)?),
            "routes" => Event::Routes(serde_json::from_str(data)?),
            "stats" => Event::Stats(serde_json::from_str(data)?),
            "drain" => Event::Drain(serde_json::from_str(data)?),
            "kill" => Event::Kill(serde_json::from_str(data)?),
            "throttle" => Event::Throttle(serde_json::from_str(data)?),
            _ => return Ok(None),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_round_trip() {
        let evs = [
            Event::Queue(QueueEvent {
                name: "sky".into(),
                position: 3,
                eta_s: 86_400,
            }),
            Event::Cert(CertEvent { name: "sky".into() }),
            Event::Routes(RoutesEvent {
                host: "app.sky.tun.zip".into(),
                engine: None,
            }),
            Event::Stats(StatsEvent {
                host: "app.sky.tun.zip".into(),
                conns: 2,
                bytes: 1024,
                bytes_in: 24,
                bytes_out: 1000,
                visitors: 1,
                open: 1,
                period_s: 30,
            }),
            Event::Throttle(ThrottleEvent {
                throttled: true,
                mbit: 2,
            }),
            Event::Drain(DrainEvent { secs: 30 }),
            Event::Kill(KillEvent {
                host: None,
                reason: KillReason::StopAll,
            }),
            Event::Kill(KillEvent {
                host: Some("a.sky.tun.zip".into()),
                reason: KillReason::Flagged,
            }),
        ];
        for e in evs {
            assert_eq!(Event::parse(e.name(), &e.data()).unwrap(), Some(e));
        }
        assert_eq!(Event::parse("future", "{}").unwrap(), None);
        let k = Event::parse("kill", r#"{"reason":"solar_flare"}"#)
            .unwrap()
            .unwrap();
        assert_eq!(
            k,
            Event::Kill(KillEvent {
                host: None,
                reason: KillReason::Unknown
            })
        );
        assert_eq!(
            Event::Kill(KillEvent {
                host: Some("a.sky.tun.zip".into()),
                reason: KillReason::EdgeBan
            })
            .data(),
            r#"{"host":"a.sky.tun.zip","reason":"edge_ban"}"#
        );

        // An M1 edge's `stats` (no M3 fields) still decodes.
        let old = Event::parse("stats", r#"{"host":"h","conns":1,"bytes":5}"#)
            .unwrap()
            .unwrap();
        assert_eq!(
            old,
            Event::Stats(StatsEvent {
                host: "h".into(),
                conns: 1,
                bytes: 5,
                ..StatsEvent::default()
            })
        );
        assert_eq!(
            Event::Kill(KillEvent {
                host: None,
                reason: KillReason::Paused
            })
            .data(),
            r#"{"reason":"paused"}"#
        );
    }
}
