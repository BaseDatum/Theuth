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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatsEvent {
    /// The host.
    pub host: String,
    /// Visitor connections in the period.
    pub conns: u64,
    /// Bytes both ways in the period.
    pub bytes: u64,
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
            }),
            Event::Drain(DrainEvent { secs: 30 }),
            Event::Kill(KillEvent {
                host: None,
                reason: KillReason::StopAll,
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
    }
}
