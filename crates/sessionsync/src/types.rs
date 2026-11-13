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
    ///
    /// 183 R1 (E-M3): `schema` is the sender's agent schema version
    /// (`Store::schema_version`), `None` from a pre-183 peer. Wire shape: it
    /// is **not** part of the serde encoding (`#[serde(skip)]`, so the
    /// postcard body is byte-identical to the pre-183 `Hello`); [`Msg::encode`]
    /// appends it as one trailing postcard `u32` and [`Msg::decode`] reads it
    /// back when there are bytes left. Old → new: no trailer, `None`. New →
    /// old: `postcard::from_bytes` stops after the last field it knows and
    /// ignores the rest of the frame (it has no end-of-input check; the
    /// codec delimits frames), so the old decoder sees the same `Hello`.
    /// `#[serde(default)]` could not do this: postcard errors at end of
    /// input instead of reporting the field missing.
    Hello {
        /// `(origin, seq)`; an origin missing here asks for a snapshot.
        cursors: Vec<(PeerId, u64)>,
        /// Sender's agent schema version; trailer, see above.
        #[serde(skip)]
        schema: Option<u32>,
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
    /// 183 R1 (E-M3): `schema` is the taker's agent schema version, a
    /// trailer exactly as on [`Msg::Hello`]; an owner on a newer schema
    /// refuses a taker without one or with an older one.
    Release {
        /// The session.
        session: String,
        /// Taker's agent schema version; trailer, see [`Msg::Hello`].
        #[serde(skip)]
        schema: Option<u32>,
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
    /// 183 §5.1 (W-18): subscriber → owner, like [`Msg::Events`] but also
    /// asks for the session's approvals, asks, jobs and cells as
    /// [`Msg::Bus`]. An old owner cannot decode it and closes the stream;
    /// the subscriber then falls back to [`Msg::Events`].
    Watch {
        /// The session.
        session: String,
        /// First event seq wanted.
        from_seq: u64,
    },
    /// 183 §5.1 (W-18): owner → a [`Msg::Watch`] subscriber: one non-agent
    /// bus event of the session.
    Bus {
        /// The session.
        session: String,
        /// Event seq (shared with [`Msg::Event`] on the same stream).
        seq: u64,
        /// `ApprovalRequested` | `ApprovalResolved` | `AskRequested` |
        /// `AskResolved` | `JobEvent` | `CellEvent`; `Watching` (empty
        /// `bytes`, seq 0) is the owner's first frame, proof it speaks Watch.
        kind: String,
        /// The event as JSON.
        bytes: Vec<u8>,
    },
    /// Owner → taker, instead of [`Msg::Released`]: the owner keeps
    /// `session` (a contact's inbox, whose limits live only here); `why` is
    /// shown to the user. Every take reason stops on it, nothing is taken.
    Refused {
        /// The session.
        session: String,
        /// The owner's words.
        why: String,
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
            Self::Watch { .. } => 11,
            Self::Bus { .. } => 12,
            Self::Refused { .. } => 13,
        }
    }

    /// One frame body: tag + postcard (+ the agent-schema trailer of a
    /// [`Msg::Hello`] / [`Msg::Release`] that carries one).
    pub fn encode(&self) -> anyhow::Result<Bytes> {
        let mut body = postcard::to_allocvec(self)?;
        if let Self::Hello {
            schema: Some(v), ..
        }
        | Self::Release {
            schema: Some(v), ..
        } = self
        {
            body.extend(postcard::to_allocvec(v)?);
        }
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
        let (mut msg, rest): (Self, &[u8]) = postcard::take_from_bytes(body)?;
        anyhow::ensure!(msg.tag() == tag, "frame tag {tag} does not match {msg:?}");
        // A pre-183 peer's frame ends with the last serde field: `schema` stays `None`.
        if !rest.is_empty()
            && let Self::Hello { schema, .. } | Self::Release { schema, .. } = &mut msg
        {
            *schema = Some(postcard::from_bytes(rest)?);
        }
        Ok(msg)
    }
}

/// 183 R1 (E-M3): is a peer that reported `theirs` behind an engine at
/// `ours`? A peer that reported nothing is a pre-183 one (schema ≤ 52).
pub fn schema_behind(theirs: Option<u32>, ours: u32) -> bool {
    theirs.is_none_or(|v| v < ours)
}

/// A peer's reported schema for a log line.
pub fn schema_label(theirs: Option<u32>) -> String {
    theirs.map_or_else(|| "unknown".to_owned(), |v| v.to_string())
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
        let watch = Msg::Watch {
            session: "s".into(),
            from_seq: 3,
        };
        let f = watch.encode().unwrap();
        assert_eq!(f[0], 11);
        assert_eq!(Msg::decode(&f).unwrap(), watch);
        let bus = Msg::Bus {
            session: "s".into(),
            seq: 4,
            kind: "ApprovalRequested".into(),
            bytes: b"{}".to_vec(),
        };
        let f = bus.encode().unwrap();
        assert_eq!(f[0], 12);
        assert_eq!(Msg::decode(&f).unwrap(), bus);
    }

    /// The pre-183 `Msg` up to `Release` (variant indexes 0..=3), as that
    /// binary's derived serde impls read and write them.
    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    enum Old {
        Hello { cursors: Vec<(PeerId, u64)> },
        Log {},
        Snapshot {},
        Release { session: String },
    }

    /// 183 R1 (E-M3): a pre-183 `Hello` / `Release` frame decodes with no
    /// schema, and a 183 frame decodes under the old type: the trailer is
    /// the only difference on the wire.
    #[test]
    fn schema_trailer_is_compatible_both_ways() {
        let cursors = vec![(PeerId("a".into()), 7)];
        // Bytes the pre-183 encoder writes: tag, variant, vec len, "a", 7.
        let old_hello = [0, 0, 1, 1, b'a', 7];
        assert_eq!(
            Msg::decode(&old_hello).unwrap(),
            Msg::Hello {
                cursors: cursors.clone(),
                schema: None
            }
        );
        let old_release = [3, 3, 1, b's'];
        assert_eq!(
            Msg::decode(&old_release).unwrap(),
            Msg::Release {
                session: "s".into(),
                schema: None
            }
        );

        // A new frame is the old frame plus one varint, and the old type reads it.
        let hello = Msg::Hello {
            cursors: cursors.clone(),
            schema: Some(53),
        };
        let f = hello.encode().unwrap();
        assert_eq!(&f[..], [0, 0, 1, 1, b'a', 7, 53]);
        assert_eq!(Msg::decode(&f).unwrap(), hello);
        assert_eq!(
            postcard::from_bytes::<Old>(&f[1..]).unwrap(),
            Old::Hello {
                cursors: cursors.clone()
            }
        );
        let release = Msg::Release {
            session: "s".into(),
            schema: Some(300),
        };
        let f = release.encode().unwrap();
        assert_eq!(&f[..], [3, 3, 1, b's', 0xAC, 0x02]);
        assert_eq!(Msg::decode(&f).unwrap(), release);
        assert_eq!(
            postcard::from_bytes::<Old>(&f[1..]).unwrap(),
            Old::Release {
                session: "s".into()
            }
        );
        // The old encoder's own bytes for the same values.
        let bytes = postcard::to_allocvec(&Old::Hello { cursors }).unwrap();
        assert_eq!(bytes, old_hello[1..]);
    }

    #[test]
    fn schema_behind_treats_a_missing_version_as_old() {
        assert!(schema_behind(None, 53));
        assert!(schema_behind(Some(52), 53));
        assert!(!schema_behind(Some(53), 53));
        assert!(!schema_behind(Some(54), 53));
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

/// `sessions_sync` entry: this engine's `theuth/sessions/1` session-log
/// replication with one peer, from memory (no store read). Seqs are log
/// positions: ours of our own log, the peer's of its own.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PeerSync {
    /// The peer (mesh `EngineId` as text).
    pub peer: String,
    /// A stream with the peer is up now.
    pub up: bool,
    /// When `up` last changed (RFC 3339); `None` before the first stream.
    pub since: Option<String>,
    /// When the peer's last `Hello` arrived (RFC 3339).
    pub last_hello: Option<String>,
    /// The agent schema the peer's `Hello` reported (`None`: pre-183 or no `Hello` yet).
    pub schema: Option<u32>,
    /// Our log head.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub head: u64,
    /// How far of our log the peer has acked (`None` before its first `Hello`).
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub acked: Option<u64>,
    /// Rows of ours the peer has not acked (`head - acked`), once `acked` is known.
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub behind_out: Option<u64>,
    /// The peer's log head as last seen (its `Hello`, then its rows).
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub peer_head: Option<u64>,
    /// How far of the peer's log we have applied (`None`: nothing of it yet).
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub applied: Option<u64>,
    /// Rows of the peer's we have not applied, when its head is known.
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub behind_in: Option<u64>,
    /// We hold our log from the peer (its agent schema is older).
    pub held: bool,
    /// The peer holds its log from us (our agent schema is older).
    pub held_by_peer: bool,
    /// The last failure with this peer, `"<phase>: <error>"`.
    pub last_error: Option<String>,
    /// When `last_error` happened (RFC 3339).
    pub last_error_at: Option<String>,
}

impl PeerSync {
    /// One line for `mesh_peers`: `sessions=up|down[ since …] out=… in=…[ held…][ error …]`.
    pub fn line(&self) -> String {
        let num = |v: Option<u64>| v.map_or_else(|| "?".to_owned(), |v| v.to_string());
        let mut s = format!(
            "sessions={}{} out={} behind (acked {}/{}) in={} behind (applied {}/{}) schema={}",
            if self.up { "up" } else { "down" },
            self.since
                .as_ref()
                .map(|t| format!(" since {t}"))
                .unwrap_or_default(),
            num(self.behind_out),
            num(self.acked),
            self.head,
            num(self.behind_in),
            num(self.applied),
            num(self.peer_head),
            self.schema
                .map_or_else(|| "?".to_owned(), |v| v.to_string()),
        );
        if self.held {
            s.push_str(" held(we hold our log: its schema is older)");
        }
        if self.held_by_peer {
            s.push_str(" held_by_peer(it holds its log: our schema is older)");
        }
        if let (Some(e), Some(at)) = (&self.last_error, &self.last_error_at) {
            let e: String = e.chars().take(200).collect();
            s.push_str(&format!(" last_error={at} {e:?}"));
        }
        s
    }
}
