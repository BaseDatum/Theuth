//! Wire types of `theuth/sessions/1` (68 §6). Framing as the record replica:
//! `LengthDelimitedCodec` frames of one tag byte + postcard (`Msg` is the
//! postcard payload; the tag is [`Msg::tag`]). Every id is a mesh `EngineId`
//! in text form ([`PeerId`]).

use bytes::{Bytes, BytesMut};
use replica::PeerId;
use serde::{Deserialize, Serialize};

/// Largest frame either side accepts (a snapshot table or one epoch head).
pub const MAX_FRAME: usize = 64 * 1024 * 1024;

/// One `session_log` row as shipped: `agent::store::LogRow` without the
/// local `orphaned` flag (orphans never ship).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogRow {
    /// Origin's log position.
    pub seq: u64,
    /// Session every row of `changeset` belongs to.
    pub session_id: String,
    /// The session's epoch when written; the fence compares against it.
    pub epoch: u32,
    /// `sqlite3session` changeset.
    pub changeset: Vec<u8>,
    /// Origin's HLC ([`replica::Hlc::to_text`]).
    pub hlc: String,
}

impl From<agent::store::LogRow> for LogRow {
    fn from(r: agent::store::LogRow) -> Self {
        Self {
            seq: u64::try_from(r.seq).unwrap_or_default(),
            session_id: r.session_id,
            epoch: u32::try_from(r.epoch).unwrap_or_default(),
            changeset: r.changeset,
            hlc: r.hlc,
        }
    }
}

/// Every message on a `theuth/sessions/1` stream.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Msg {
    /// First frame each way: the sender's cursors (highest seq applied per origin).
    Hello {
        /// `(origin, seq)`; an origin missing here asks for a snapshot.
        cursors: Vec<(PeerId, u64)>,
    },
    /// Log rows of one origin, in seq order (own or relayed).
    Log {
        /// Whose log.
        origin: PeerId,
        /// The rows.
        rows: Vec<LogRow>,
    },
    /// Bootstrap for a peer with no cursor for `origin`: current rows per
    /// table, current to `current_to_seq` of that origin's log.
    Snapshot {
        /// Whose rows.
        origin: PeerId,
        /// The log seq these rows include.
        current_to_seq: u64,
        /// `(table, changeset of every current row)`.
        tables: Vec<(String, Vec<u8>)>,
    },
    /// Taker → owner: stop the actor, flush the log, answer [`Msg::Released`].
    Release {
        /// The session.
        session: String,
    },
    /// Owner → taker: released; the last seq written for `session`.
    Released {
        /// The session.
        session: String,
        /// Last log seq for it on the owner.
        last_seq: u64,
    },
    /// Subscriber → owner: stream live events for `session` (68 §5).
    Events {
        /// The session.
        session: String,
        /// First event seq wanted.
        from_seq: u64,
    },
    /// Owner → subscriber: one event.
    Event {
        /// The session.
        session: String,
        /// Event seq.
        seq: u64,
        /// The event, JSON-encoded `agent::AgentEvent`.
        bytes: Vec<u8>,
    },
    /// Ask the peer for everything it holds for `session` (log + snapshot rows).
    Fetch {
        /// The session.
        session: String,
    },
    /// Receiver → sender: every row of `origin` up to `seq` was applied (or
    /// known); the sender moves `session_cursors(origin, peer)`.
    Ack {
        /// Whose log.
        origin: PeerId,
        /// Highest seq applied.
        seq: u64,
    },
    /// Sender → owner (139 Revision 2): a harness-made input for a session
    /// the receiver owns — a settled job's `<job_result>`, a fired
    /// `<reminder>`, a lost child's result. Answered by [`Msg::HarnessAck`].
    Harness {
        /// The session.
        session: String,
        /// The `from` label `Agents::inject` carries (`job j3`, `reminder r1`, `session:<child>`).
        kind: String,
        /// The engine that minted the envelope.
        origin: PeerId,
        /// Sender-minted UUID; the receiver dedups on it.
        id: String,
        /// The bare body.
        text: String,
        /// The body as the model reads it.
        wrapped: String,
        /// The sender's request scope (50 §8), if the work had one.
        scope: Option<String>,
        /// Unix ms when the sender queued it.
        at: i64,
    },
    /// Owner → sender: `id` was handed to the supervisor here (or seen
    /// before); `error` when it was refused and the sender should keep it.
    HarnessAck {
        /// The envelope.
        id: String,
        /// Why it was refused, if it was.
        error: Option<String>,
    },
}

impl Msg {
    /// Frame tag byte (stable; new variants append).
    pub fn tag(&self) -> u8 {
        match self {
            Self::Hello { .. } => 0,
            Self::Log { .. } => 1,
            Self::Snapshot { .. } => 2,
            Self::Release { .. } => 3,
            Self::Released { .. } => 4,
            Self::Events { .. } => 5,
            Self::Event { .. } => 6,
            Self::Fetch { .. } => 7,
            Self::Ack { .. } => 8,
            Self::Harness { .. } => 9,
            Self::HarnessAck { .. } => 10,
        }
    }

    /// One frame body: tag + postcard.
    pub fn encode(&self) -> anyhow::Result<Bytes> {
        let body = postcard::to_allocvec(self)?;
        let mut b = BytesMut::with_capacity(1 + body.len());
        b.extend_from_slice(&[self.tag()]);
        b.extend_from_slice(&body);
        Ok(b.freeze())
    }

    /// Inverse of [`Msg::encode`]; the tag must match the variant.
    pub fn decode(frame: &[u8]) -> anyhow::Result<Self> {
        let (&tag, body) = frame
            .split_first()
            .ok_or_else(|| anyhow::anyhow!("empty frame"))?;
        let msg: Self = postcard::from_bytes(body)?;
        anyhow::ensure!(msg.tag() == tag, "frame tag {tag} does not match {msg:?}");
        Ok(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_with_matching_tag() {
        let m = Msg::Log {
            origin: PeerId("a".into()),
            rows: vec![LogRow {
                seq: 7,
                session_id: "s".into(),
                epoch: 2,
                changeset: vec![1, 2, 3],
                hlc: "x".into(),
            }],
        };
        let f = m.encode().unwrap();
        assert_eq!(f[0], 1);
        assert_eq!(Msg::decode(&f).unwrap(), m);
        let mut bad = f.to_vec();
        bad[0] = 0;
        assert!(Msg::decode(&bad).is_err());
        let h = Msg::Harness {
            session: "s".into(),
            kind: "job j1".into(),
            origin: PeerId("a".into()),
            id: "u".into(),
            text: "done".into(),
            wrapped: "<job_result>done</job_result>".into(),
            scope: None,
            at: 7,
        };
        let f = h.encode().unwrap();
        assert_eq!(f[0], 9);
        assert_eq!(Msg::decode(&f).unwrap(), h);
        let ack = Msg::HarnessAck {
            id: "u".into(),
            error: Some("unknown session".into()),
        };
        let f = ack.encode().unwrap();
        assert_eq!(f[0], 10);
        assert_eq!(Msg::decode(&f).unwrap(), ack);
    }
}

/// `session_owner` reply and the payload of a completed `session_take`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SessionOwner {
    /// Owner engine (mesh `EngineId` as text; `""` on a lone engine).
    pub owner: String,
    /// Current epoch.
    pub epoch: u32,
    /// The owner is this engine.
    pub here: bool,
}

/// One undelivered stretch of a session's history on this engine (68 §4.3):
/// rows this engine wrote as owner in `epoch` that a later owner superseded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SessionOrphan {
    /// The superseded epoch.
    pub epoch: u32,
    /// First orphaned log seq.
    pub from_seq: u64,
    /// Last orphaned log seq.
    pub to_seq: u64,
    /// When the first orphaned row was written (RFC 3339, from its HLC).
    pub at: String,
}

/// `sessions_status` reply (68 §10, Settings → Mesh → Sessions).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SessionsStatus {
    /// This engine's mesh `EngineId` as text; `""` on a lone engine.
    pub engine: String,
    /// Durable sessions owned here.
    pub here: u32,
    /// Durable sessions owned by another engine.
    pub elsewhere: u32,
    /// Last `session_log.seq` written by this engine (0 when the log is empty).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub log_seq: u64,
    /// Newest snapshot this engine built (RFC 3339); `None` until worker L
    /// records one.
    pub last_snapshot: Option<String>,
}
