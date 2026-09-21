//! Identity and metadata of a document (design 59 §4.1–§4.2) and the wire
//! types the RPC layer exports (§6.1).

use std::fmt;

use serde::{Deserialize, Serialize};

/// A document id: uuid v7 rendered as 26 lowercase base32 characters (§4.1).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(transparent)]
pub struct DocId(pub String);

const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

impl DocId {
    /// A fresh time-ordered id.
    pub fn new() -> Self {
        Self::from_uuid(uuid::Uuid::now_v7())
    }

    /// Render `u` (16 bytes → 26 chars, RFC 4648 alphabet lowercased, no padding).
    pub fn from_uuid(u: uuid::Uuid) -> Self {
        let bytes = u.as_bytes();
        let mut out = String::with_capacity(26);
        let mut acc: u32 = 0;
        let mut bits = 0;
        for &b in bytes {
            acc = (acc << 8) | u32::from(b);
            bits += 8;
            while bits >= 5 {
                bits -= 5;
                out.push(ALPHABET[((acc >> bits) & 31) as usize] as char);
            }
        }
        if bits > 0 {
            out.push(ALPHABET[((acc << (5 - bits)) & 31) as usize] as char);
        }
        Self(out)
    }

    /// Validate a client-supplied id: 26 chars of the alphabet.
    pub fn parse(s: &str) -> Option<Self> {
        (s.len() == 26 && s.bytes().all(|b| ALPHABET.contains(&b))).then(|| Self(s.to_owned()))
    }

    /// The id as a path segment.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for DocId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for DocId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What a document is (§4.2 `kind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum DocKind {
    /// A knowledge concept; the derived file is `knowledge/<id>.md` and the first block is the frontmatter.
    Knowledge,
    /// A markdown artifact (step 5).
    Artifact,
    /// A meeting's note (step 5).
    MeetingNote,
    /// A free-standing note (step 3).
    Note,
}

impl DocKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Knowledge => "knowledge",
            Self::Artifact => "artifact",
            Self::MeetingNote => "meeting_note",
            Self::Note => "note",
        }
    }
}

impl fmt::Display for DocKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which owner-side thing a document is bound to (§3.1, §4.2 `binding`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum Binding {
    /// A concept id (`facts/foo`).
    Knowledge(String),
    /// A markdown artifact by its `(session, name)`.
    Artifact {
        /// Owning session.
        session: String,
        /// Artifact name (`report.md`).
        name: String,
    },
    /// A meeting's note.
    MeetingNote(String),
    /// A note has no other owner.
    Note,
}

impl Binding {
    /// The index column: canonical JSON, so `Lookup` is one `WHERE binding = ?`.
    pub fn key(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// The kind a new document of this binding gets.
    pub fn kind(&self) -> DocKind {
        match self {
            Self::Knowledge(_) => DocKind::Knowledge,
            Self::Artifact { .. } => DocKind::Artifact,
            Self::MeetingNote(_) => DocKind::MeetingNote,
            Self::Note => DocKind::Note,
        }
    }

    /// The derived markdown path relative to the data dir (§4.1); a note's is set by the store.
    pub fn derived(&self, id: &DocId) -> String {
        match self {
            Self::Knowledge(c) => format!("knowledge/{c}.md"),
            Self::Artifact { .. } | Self::MeetingNote(_) => String::new(),
            Self::Note => format!("documents/{id}/note.md"),
        }
    }
}

/// A member admitted through a share link (§8; empty in step 1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Member {
    /// X25519 static, base64url.
    #[serde(rename = "pub")]
    pub pub_: String,
    /// Display name.
    pub name: String,
    /// `editor` or `viewer`.
    pub role: String,
    /// When admitted.
    pub added_ms: i64,
    /// The link they came through.
    pub via: Option<String>,
    /// Last connection.
    pub last_seen_ms: i64,
}

/// A share link (§8.2; empty in step 1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ShareLink {
    /// `lnk_…`.
    pub id: String,
    /// `editor` or `viewer`.
    pub role: String,
    /// base64url(sha256(secret)).
    pub secret_hash: String,
    /// Preset display name for joiners.
    pub name: Option<String>,
    /// Created.
    pub created_ms: i64,
    /// Expiry, if any.
    pub expires_ms: Option<i64>,
    /// Revoked, if so.
    pub revoked_ms: Option<i64>,
}

/// `meta.json` (§4.2) and the wire `DocMeta` of `doc_open`.
#[derive(
    Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DocMeta {
    /// The id.
    pub id: DocId,
    /// What it is.
    pub kind: DocKind,
    /// Display title (knowledge: the frontmatter `title`; else the first heading).
    pub title: String,
    /// What it is bound to.
    pub binding: Binding,
    /// Derived markdown path relative to the data dir.
    pub derived: String,
    /// Name of the `Y.XmlFragment` the blocks live in.
    pub fragment: String,
    /// Members (§8); empty until step 7.
    #[serde(default)]
    pub members: Vec<Member>,
    /// Share links (§8); empty until step 7.
    #[serde(default)]
    pub links: Vec<ShareLink>,
    /// The agent's cursor colour in this document.
    pub agent: AgentStyle,
    /// sha256 (hex) of the derived file as last written or imported: the
    /// "our own write" check of §4.4, persisted so an unloaded document can
    /// tell its own export from a foreign edit.
    #[serde(default)]
    pub last_export: Option<String>,
    /// Created, unix ms.
    pub created_ms: i64,
    /// Last accepted update, unix ms.
    pub updated_ms: i64,
}

/// `meta.json` `agent` (§4.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AgentStyle {
    /// CSS colour.
    pub color: String,
}

impl Default for AgentStyle {
    fn default() -> Self {
        Self {
            color: "#7c3aed".to_owned(),
        }
    }
}

/// One row of `doc_list` (§6.1).
#[derive(
    Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DocSummary {
    /// The id.
    pub id: DocId,
    /// What it is.
    pub kind: DocKind,
    /// Title.
    pub title: String,
    /// Binding.
    pub binding: Binding,
    /// Created, unix ms.
    pub created_ms: i64,
    /// Last update, unix ms.
    pub updated_ms: i64,
    /// Approximate Yjs state size.
    pub bytes: i64,
}

impl From<&DocMeta> for DocSummary {
    fn from(m: &DocMeta) -> Self {
        Self {
            id: m.id.clone(),
            kind: m.kind,
            title: m.title.clone(),
            binding: m.binding.clone(),
            created_ms: m.created_ms,
            updated_ms: m.updated_ms,
            bytes: 0,
        }
    }
}

/// Bus event (§7.3, `subscribe_events`): a document was created, exported (title/`updated_ms`
/// current), renamed or deleted — the notes list refreshes on it. Wire shape `{ "type": "DocumentChanged", … }`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub struct DocumentChanged {
    /// Document.
    pub doc: DocId,
    /// What it is.
    pub kind: DocKind,
    /// Title after the change.
    pub title: String,
    /// Last update, unix ms.
    pub updated_ms: i64,
    /// Moved to the trash.
    pub deleted: bool,
}

impl DocumentChanged {
    /// The event for `meta` as it stands.
    pub fn of(meta: &DocMeta, deleted: bool) -> Self {
        Self {
            doc: meta.id.clone(),
            kind: meta.kind,
            title: meta.title.clone(),
            updated_ms: meta.updated_ms,
            deleted,
        }
    }
}

/// One kept snapshot (§4.6).
#[derive(
    Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SnapshotInfo {
    /// File stem: `<ts>-<label>`; the argument of `doc_restore`.
    pub name: String,
    /// Label.
    pub label: String,
    /// Taken, unix ms.
    pub at_ms: i64,
    /// Bytes.
    pub bytes: i64,
}

/// Who applied an update (§3.1 `Principal`, §5.5). Persisted as a one-byte
/// tag (+ session id for the agent) before every update in `updates.log`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Principal {
    /// The owner, from one of their devices; `device` is the connection id, used to skip the sender in fan-out.
    Owner {
        /// RPC connection id.
        device: Option<u64>,
    },
    /// The agent, in a session (step 2).
    Agent {
        /// Session id.
        session: uuid::Uuid,
    },
    /// The helper's own transaction (import, edit).
    Helper,
    /// A share-link member (step 7).
    Member {
        /// Their static key, base64url.
        pub_: String,
    },
    /// The derived file, or a store hook that stands for it.
    File,
}

impl Principal {
    /// Log tag byte (§5.5).
    pub fn tag(&self) -> u8 {
        match self {
            Self::Owner { .. } => 1,
            Self::Agent { .. } => 2,
            Self::Helper => 3,
            Self::Member { .. } => 4,
            Self::File => 5,
        }
    }

    /// The connection an update came from, when it came from a connection.
    pub fn conn(&self) -> Option<u64> {
        match self {
            Self::Owner { device } => *device,
            _ => None,
        }
    }
}

/// Who authored an update, as clients see it (§6.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "who", rename_all = "snake_case")]
pub enum Who {
    /// The owner (any device).
    Owner,
    /// The agent.
    Agent {
        /// Session id.
        session: String,
    },
    /// A member.
    Member {
        /// Static key.
        #[serde(rename = "pub")]
        pub_: String,
        /// Display name.
        name: String,
    },
    /// The derived file (a `sed`, `knowledge_put`, git pull, restore).
    File,
}

impl From<&Principal> for Who {
    fn from(p: &Principal) -> Self {
        match p {
            Principal::Owner { .. } => Self::Owner,
            Principal::Agent { session } => Self::Agent {
                session: session.to_string(),
            },
            Principal::Member { pub_ } => Self::Member {
                pub_: pub_.clone(),
                name: String::new(),
            },
            // A helper transaction always stands for an import (step 1) or an agent edit (step 2, stamped Agent).
            Principal::Helper | Principal::File => Self::File,
        }
    }
}

/// One item of `doc_subscribe` (§6.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub enum DocEvent {
    /// An accepted update (base64 v1) from someone else.
    Update {
        /// base64 v1 update.
        update: String,
        /// Author.
        by: Who,
    },
    /// The agent (or, later, a member) wrote these blocks in the update that follows (§5.5, §6.4);
    /// the UI keys the reveal and the hover attribution on it. Sent before the matching `Update`.
    Authored {
        /// Block ids the transaction inserted or replaced.
        blocks: Vec<String>,
        /// Author.
        by: Who,
    },
    /// An awareness update (base64 y-protocols) from someone else.
    Awareness {
        /// base64 awareness update.
        state: String,
    },
    /// The derived file was rewritten.
    Exported,
    /// A snapshot was written.
    Snapshot {
        /// Its label.
        label: String,
    },
    /// The subscription ends.
    Closed {
        /// Why (`deleted`, `unloaded`, `slow`).
        reason: String,
    },
}

/// Reply to `doc_open` (§6.1): the state missing from the client's vector, chunked ≤ 48 KiB base64.
#[derive(
    Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DocOpen {
    /// Metadata.
    pub meta: DocMeta,
    /// base64 chunks of one v1 update; concatenate the decoded bytes, apply once.
    pub state: Vec<String>,
    /// base64 awareness update with every known state (may be empty).
    pub awareness: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doc_id_is_26_base32_chars_and_round_trips_the_alphabet() {
        let id = DocId::new();
        assert_eq!(id.0.len(), 26);
        assert!(DocId::parse(id.as_str()).is_some());
        assert!(DocId::parse("ABC").is_none());
        assert!(DocId::parse(&"1".repeat(26)).is_none());
        // Known vector: all-zero uuid → 26 × 'a'.
        assert_eq!(DocId::from_uuid(uuid::Uuid::nil()).0, "a".repeat(26));
        // v7 ids sort by time, and so do their base32 renderings (first bits are the timestamp).
        let a = DocId::new();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = DocId::new();
        assert!(a.0 < b.0);
    }

    #[test]
    fn binding_key_and_derived_paths() {
        let b = Binding::Knowledge("facts/foo".into());
        assert_eq!(b.key(), r#"{"knowledge":"facts/foo"}"#);
        assert_eq!(b.kind(), DocKind::Knowledge);
        let id = DocId::from_uuid(uuid::Uuid::nil());
        assert_eq!(b.derived(&id), "knowledge/facts/foo.md");
        assert_eq!(
            Binding::Note.derived(&id),
            format!("documents/{id}/note.md")
        );
    }

    #[test]
    fn wire_shapes() {
        assert_eq!(
            serde_json::to_string(&Who::Agent {
                session: "s".into()
            })
            .unwrap(),
            r#"{"who":"agent","session":"s"}"#
        );
        assert_eq!(
            serde_json::to_string(&DocEvent::Exported).unwrap(),
            r#"{"type":"Exported"}"#
        );
        assert_eq!(Principal::File.tag(), 5);
        assert_eq!(Who::from(&Principal::Helper), Who::File);
    }
}
