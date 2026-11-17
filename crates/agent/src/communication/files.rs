//! 199 CONTACT-FILES (P0 contract): the frozen shapes of files between contacts, and
//! the pure pieces every builder shares. Nothing here is wired yet: `send_message`
//! refuses `files`, the RPCs answer [`NOT_YET`], and no row is ever written.
//!
//! - Types: [`ContactFile`] and its states, the notice operation's answers.
//! - [`sanitise`] and [`candidate`]: the saved name (Storage and sanitisation).
//! - [`placeholders`], [`notice`], [`ended_notice`]: every line the model sees.
//! - [`disk`] (unix): the egress gate, the protected set, BLAKE3 from a handle, the
//!   macOS quarantine and no-clobber publication by inode.
//! - [`HeldContactFile`]: the seam the artifact mesh (P2) asks for a held file.

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use kameo::message::{Context, Message};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::loop_::attr;
use crate::supervisor::Agents;

/// Egress gate, protected set, hashing, quarantine and publication.
#[cfg(unix)]
pub mod disk;

/// The one limit (D7): 5 GB per file, decimal as Finder shows sizes.
pub const FILE_MAX: u64 = 5_000_000_000;
/// D8: an offer lives this long past its send (sender) or its arrival (receiver).
pub const OFFER_TTL: Duration = Duration::from_secs(7 * 24 * 3600);
/// D8: an active transfer that makes no progress this long loses its slot.
pub const FILE_IDLE: Duration = Duration::from_secs(60);
/// D17: the `input_accepted` journal source of an admitted file notice.
pub const JOURNAL_SOURCE: &str = "contact_file";
/// What every entry point answers until its owner builds it.
pub const NOT_YET: &str = "contact files are not available yet";
/// What `send_message` answers for `files` until 199-E1 builds sending.
pub const SEND_NOT_YET: &str = "sending files is not available yet";

/// Received files: `<root>/files/received/<contact>/<YYYY-MM-DD>/<name>`.
pub const RECEIVED_DIR: &str = "files/received";
/// Downloads in flight: `<root>/files/partial/<local-id>`.
pub const PARTIAL_DIR: &str = "files/partial";
/// Sender clones: `<root>/files/outgoing/<local-id>`.
pub const OUTGOING_DIR: &str = "files/outgoing";
/// P2 copies from another of our machines: `<root>/files/cache/<holder>/<id>/<name>`.
pub const CACHE_DIR: &str = "files/cache";

// 199-E3: P3 intake of an `https` URL part (or an artifact file part) streams it with
// `web::fetch::download(url, &partial, &opts)` (199-W, public-only, HTTPS-only) into
// `PARTIAL_DIR`, counts it against `FILE_MAX`, records the computed BLAKE3, then publishes
// with `disk::publish` like a contact download. Inline `raw` parts skip the download.

/// Which way a file travels, from this engine's view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum FileDirection {
    /// Offered by this engine.
    Out,
    /// Received by this engine.
    In,
}

/// An incoming file's state; `Received` and `Failed` are terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum InState {
    /// Waiting for a download slot.
    Queued,
    /// Bytes are arriving.
    Receiving,
    /// Verified; being linked into place (resolved by inode on restart).
    Publishing,
    /// Published while its exchange is still `accepted` or `held`: waits for it.
    Saved,
    /// Published, and its notice durably queued.
    Received,
    /// Failed; `error` says why.
    Failed,
}

/// An outgoing file's state; `Sent` and `Failed` are terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum OutState {
    /// Offered; the receiver has not started fetching.
    Queued,
    /// The receiver is fetching.
    Sending,
    /// The receiver's `received` receipt arrived.
    Sent,
    /// Failed; `error` says why.
    Failed,
}

/// A file's state, tagged by its direction: on the wire `{"direction":"in","state":"saved"}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "direction", content = "state", rename_all = "snake_case")]
pub enum FileState {
    /// An offered file.
    Out(OutState),
    /// A received file.
    In(InState),
}

impl FileState {
    /// The direction the tag names.
    pub fn direction(self) -> FileDirection {
        match self {
            Self::Out(_) => FileDirection::Out,
            Self::In(_) => FileDirection::In,
        }
    }

    /// `received`, `sent` or `failed`: the row never changes again.
    pub fn terminal(self) -> bool {
        matches!(
            self,
            Self::In(InState::Received | InState::Failed)
                | Self::Out(OutState::Sent | OutState::Failed)
        )
    }
}

/// The holder's notice outbox column (`contact_files.notice_state`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum NoticeState {
    /// Written with the terminal state; not yet accepted by the chat's owner.
    Queued,
    /// `contact_file_notice` answered `accepted`.
    Sent,
    /// `contact_file_notice_status` answered `committed`.
    Delivered,
}

/// `contact_file_notice`'s answer: the owner holds the notice (pending or committed).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum NoticeAdmission {
    /// Admitted now, or already pending or committed (a repeat enqueues nothing).
    Accepted,
}

/// `contact_file_notice_status`'s answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum NoticeStatus {
    /// The owner's journal has an `input_accepted` step with this `input_id`, or its
    /// `errors` table this id.
    Committed,
    /// Neither: the holder re-sends on its backoff.
    Unknown,
}

/// One file between contacts as a card shows it (`ContactFileChanged`,
/// `PeerDelivered.files`, `contact_file`). Keys: `(holder, id)`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ContactFile {
    /// The holder's local id, 32 lowercase hex.
    pub id: String,
    /// The holder's endpoint id: the engine that stores (in) or serves (out) the bytes.
    pub holder: String,
    /// Direction and state, flattened: `direction`, `state`.
    #[serde(flatten)]
    #[cfg_attr(feature = "ts", ts(flatten))]
    pub state: FileState,
    /// The exchange (message) that carried it.
    pub exchange: Uuid,
    /// `ContactFacts.id`, the contact's record id; never the alias.
    pub contact_id: String,
    /// The contact's display alias.
    pub contact: String,
    /// The sanitised declared name.
    pub name: String,
    /// Descriptive media type (`mime_guess`, else `application/octet-stream`).
    pub media_type: String,
    /// Declared, then actual size; `None` only for a P3 download in flight.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional, type = "number"))]
    pub size: Option<u64>,
    /// Declared or computed BLAKE3, 64 lowercase hex.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub blake3: Option<String>,
    /// Bytes moved so far.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub done: u64,
    /// Why it failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub error: Option<String>,
    /// In: the saved path on the holder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub path: Option<PathBuf>,
    /// The holder's machine name.
    pub machine: String,
    /// The local deadline (D8).
    pub deadline: DateTime<Utc>,
}

/// `contact_file_thumbnail`'s answer: a fitted still image.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ContactFileThumbnail {
    /// The actual MIME of `data` (from `tools::images::Fitted`).
    pub mime: String,
    /// Base64 image bytes.
    pub data: String,
}

/// D17: the notice id, UUIDv5 of `(holder, file id)`. The owner's dedup key.
pub fn notice_id(holder: &str, id: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_OID,
        format!("theuth/contact-file/{holder}/{id}").as_bytes(),
    )
}

/// `<root>/files/received/<sanitised contact>/<YYYY-MM-DD>`.
pub fn received_dir(root: &Path, contact: &str, day: chrono::NaiveDate) -> PathBuf {
    root.join(RECEIVED_DIR)
        .join(sanitise(contact))
        .join(day.format("%Y-%m-%d").to_string())
}

// ---------------------------------------------------------------------------
// Sanitiser
// ---------------------------------------------------------------------------

/// Longest saved name, in UTF-8 bytes.
const NAME_MAX: usize = 200;
/// Longest extension (dot included) the cut keeps.
const EXT_MAX: usize = 20;

/// Bidi controls step 2 drops (Unicode Cc is dropped too).
fn bidi(c: char) -> bool {
    matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// Step 4: spaces and dots off both ends (one `trim_matches` is already the fixed point).
fn trim(s: &str) -> &str {
    s.trim_matches([' ', '.'])
}

/// Windows reserved stems (case-insensitive, before the first dot).
fn reserved(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    match stem.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" => true,
        s => {
            let b = s.as_bytes();
            b.len() == 4
                && (s.starts_with("COM") || s.starts_with("LPT"))
                && (b'1'..=b'9').contains(&b[3])
        }
    }
}

/// The longest prefix of `s` within `max` bytes, cut at a character boundary.
fn cut(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// A declared file name (or a contact's display name, for its folder) as saved: the
/// six steps of Storage and sanitisation. Never empty, never a path, never hidden.
pub fn sanitise(declared: &str) -> String {
    // 1. The text after the last separator.
    let base = declared.rsplit(['/', '\\']).next().unwrap_or_default();
    // 2–3. No controls or bidi; the Windows-forbidden punctuation becomes `_`.
    let clean: String = base
        .chars()
        .filter(|&c| !c.is_control() && !bidi(c))
        .map(|c| {
            if matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    // 4–5.
    let mut name = trim(&clean).to_owned();
    if name.is_empty() {
        name = "file".into();
    }
    if reserved(&name) {
        name.insert(0, '_');
    }
    // 6. Cap, keeping a short extension, then trim again.
    if name.len() > NAME_MAX {
        let ext = match name.rfind('.') {
            Some(i) if i > 0 && name.len() - i <= EXT_MAX => &name[i..],
            _ => "",
        };
        let stem = cut(&name[..name.len() - ext.len()], NAME_MAX - ext.len());
        name = trim(&format!("{stem}{ext}")).to_owned();
    }
    name
}

/// The `n`th name tried for `name` (1-based): `name`, then `stem (2).ext`, `stem (3).ext`, …
pub fn candidate(name: &str, n: u32) -> String {
    if n <= 1 {
        return name.to_owned();
    }
    match name.rfind('.') {
        Some(i) if i > 0 => format!("{} ({n}){}", &name[..i], &name[i..]),
        _ => format!("{name} ({n})"),
    }
}

// ---------------------------------------------------------------------------
// What the model sees
// ---------------------------------------------------------------------------

/// Attributes every element starts with: `from`, `message`, `name`, `type`.
fn head(from: &str, f: &ContactFile) -> String {
    format!(
        "<message_file from=\"{}\" message=\"{}\" name=\"{}\" type=\"{}\"",
        attr(from),
        attr(&f.exchange.to_string()),
        attr(&f.name),
        attr(&f.media_type)
    )
}

/// `holder`, `file`.
fn tail(f: &ContactFile) -> String {
    format!(" holder=\"{}\" file=\"{}\"", attr(&f.holder), attr(&f.id))
}

/// The message row's lines after `</message>`: one `state="receiving"` element per
/// file, then the receiving hint. Empty for no files.
pub fn placeholders(from: &str, files: &[ContactFile]) -> String {
    if files.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for f in files {
        let bytes = f.size.map(|n| n.to_string()).unwrap_or_default();
        out.push_str(&format!(
            "{} bytes=\"{}\"{} state=\"receiving\"/>\n",
            head(from, f),
            attr(&bytes),
            tail(f)
        ));
    }
    out.push_str(&format!(
        "Files from {} are arriving; a notice follows when each one is saved or fails.",
        attr(from)
    ));
    out
}

/// The element of one terminal file's notice (no hint line); `None` unless it is
/// `received` or `failed`.
fn element(from: &str, f: &ContactFile) -> Option<String> {
    match f.state {
        FileState::In(InState::Received) => {
            let saved = f
                .path
                .as_deref()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            let bytes = f.size.map(|n| n.to_string()).unwrap_or_default();
            Some(format!(
                "{} state=\"received\" saved=\"{}\" bytes=\"{}\" blake3=\"{}\"{} machine=\"{}\"/>",
                head(from, f),
                attr(&saved),
                attr(&bytes),
                attr(f.blake3.as_deref().unwrap_or_default()),
                tail(f),
                attr(&f.machine)
            ))
        }
        FileState::In(InState::Failed) => Some(format!(
            "{} state=\"failed\" reason=\"{}\"{}/>",
            head(from, f),
            attr(f.error.as_deref().unwrap_or_default()),
            tail(f)
        )),
        _ => None,
    }
}

/// The exact text of one file's notice row: its element and hint line. `None` unless
/// the file is `received` or `failed`.
pub fn notice(from: &str, f: &ContactFile) -> Option<String> {
    let el = element(from, f)?;
    let from = attr(from);
    Some(match f.state {
        FileState::In(InState::Received) => format!(
            "{el}\nFile notice from theuth, not from {from}: no reply to the contact is needed. \
             The file is saved on disk, not shown to you; open it with read_file if you need it."
        ),
        _ => format!(
            "{el}\nFile notice from theuth, not from {from}: no reply to the contact is needed."
        ),
    })
}

/// D17 ended share: the engine-state row recorded instead of an input.
pub fn ended_notice(from: &str, f: &ContactFile) -> Option<String> {
    let el = element(from, f)?;
    Some(format!(
        "{el}\nFile notice from theuth: the share with {} has ended, so this was not given to the agent.",
        attr(from)
    ))
}

// ---------------------------------------------------------------------------
// The P2 seam
// ---------------------------------------------------------------------------

/// A received file as its holder stores it, for the artifact mesh to serve.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeldFile {
    /// The published path.
    pub path: PathBuf,
    /// `(dev, inode)` recorded at publication; serving reopens and compares.
    pub dev: u64,
    /// See `dev`.
    pub inode: u64,
    /// Actual size.
    pub size: u64,
    /// Actual BLAKE3, 64 lowercase hex.
    pub blake3: String,
}

/// Ask (`artifact` → `Agents`): the `received` `in` row with local id `id`, or `None`.
/// The artifact crate already depends on `agent` and asks `Agents` elsewhere
/// (`artifact::search`), so this message is the seam: no trait, no cycle.
#[derive(Clone, Debug)]
pub struct HeldContactFile {
    /// The holder's local id.
    pub id: String,
}

impl Message<HeldContactFile> for Agents {
    type Reply = Option<HeldFile>;

    async fn handle(
        &mut self,
        _: HeldContactFile,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        // 199-E2: look up a `received` `in` row of `contact_files`; none exist before 199-E1.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitiser_table() {
        let long = "a".repeat(300);
        let cases: &[(&str, &str)] = &[
            ("report.pdf", "report.pdf"),
            ("../../etc/passwd", "passwd"),
            ("..", "file"),
            ("/", "file"),
            ("", "file"),
            ("C:\\Users\\me\\evil.exe", "evil.exe"),
            ("dir/sub\\x.txt", "x.txt"),
            ("in\u{202E}fdp.exe", "infdp.exe"),
            ("a\u{200E}b\u{2066}c\u{061C}.txt", "abc.txt"),
            ("tab\there\nnl\u{7f}.txt", "tabherenl.txt"),
            ("what?<is>:this*|\".txt", "what__is__this___.txt"),
            (". report .", "report"),
            (" . .. report . . ", "report"),
            (".zshrc", "zshrc"),
            ("...", "file"),
            ("CON", "_CON"),
            ("con.txt", "_con.txt"),
            ("LPT9.log", "_LPT9.log"),
            ("COM0.txt", "COM0.txt"),
            ("CONSOLE.txt", "CONSOLE.txt"),
        ];
        for (input, want) in cases {
            assert_eq!(sanitise(input), *want, "{input:?}");
        }
        // The 200-byte cut keeps a short extension …
        let cut = sanitise(&format!("{long}.pdf"));
        assert_eq!(cut.len(), 200);
        assert!(cut.ends_with("aaa.pdf"));
        // … at a character boundary …
        let wide = sanitise(&format!("{}.txt", "é".repeat(150)));
        assert!(wide.len() <= 200 && wide.ends_with("é.txt"), "{wide}");
        // … and drops an over-long one.
        let ext = format!(".{}", "x".repeat(40));
        assert_eq!(sanitise(&format!("{long}{ext}")).len(), 200);
        assert!(!sanitise(&format!("{long}{ext}")).contains('x'));
        // The cut cannot leave a trailing dot or space.
        let dotty = sanitise(&format!("{} .{}", "b".repeat(198), "y".repeat(30)));
        assert!(!dotty.ends_with(['.', ' ']), "{dotty}");
    }

    #[test]
    fn collision_suffix() {
        assert_eq!(candidate("report.pdf", 1), "report.pdf");
        assert_eq!(candidate("report.pdf", 2), "report (2).pdf");
        assert_eq!(candidate("archive.tar.gz", 3), "archive.tar (3).gz");
        assert_eq!(candidate("README", 2), "README (2)");
    }

    fn file(state: FileState) -> ContactFile {
        ContactFile {
            id: "9d0c4b7e21a35f6880e1c2d3b4a59687".into(),
            holder: "5f3a\"<&>".into(),
            state,
            exchange: Uuid::parse_str("0193f0a2-6b1c-7d3e-9f40-1a2b3c4d5e6f").unwrap(),
            contact_id: "rec".into(),
            contact: "maya".into(),
            name: "a\"b<c>&d.zip".into(),
            media_type: "application/x-\"<&".into(),
            size: Some(1_200_000_000),
            blake3: Some("7c1e".into()),
            done: 0,
            error: Some("the \"sender's\" <file> & changed".into()),
            path: Some("/Users/sky/.theuth/files/received/maya/2026-11-17/a\"b<c>&d.zip".into()),
            machine: "Sky's \"Mac\" <&>".into(),
            deadline: DateTime::parse_from_rfc3339("2026-11-24T14:32:00Z")
                .unwrap()
                .into(),
        }
    }

    #[test]
    fn placeholder_and_notices_escape_every_attribute() {
        let from = "peer:m\"<&>";
        let f = file(FileState::In(InState::Receiving));
        assert_eq!(
            placeholders(from, &[f.clone(), f]),
            "<message_file from=\"peer:m&quot;&lt;&amp;&gt;\" message=\"0193f0a2-6b1c-7d3e-9f40-1a2b3c4d5e6f\" name=\"a&quot;b&lt;c&gt;&amp;d.zip\" type=\"application/x-&quot;&lt;&amp;\" bytes=\"1200000000\" holder=\"5f3a&quot;&lt;&amp;&gt;\" file=\"9d0c4b7e21a35f6880e1c2d3b4a59687\" state=\"receiving\"/>\n\
             <message_file from=\"peer:m&quot;&lt;&amp;&gt;\" message=\"0193f0a2-6b1c-7d3e-9f40-1a2b3c4d5e6f\" name=\"a&quot;b&lt;c&gt;&amp;d.zip\" type=\"application/x-&quot;&lt;&amp;\" bytes=\"1200000000\" holder=\"5f3a&quot;&lt;&amp;&gt;\" file=\"9d0c4b7e21a35f6880e1c2d3b4a59687\" state=\"receiving\"/>\n\
             Files from peer:m&quot;&lt;&amp;&gt; are arriving; a notice follows when each one is saved or fails."
        );
        assert_eq!(placeholders(from, &[]), "");
        assert_eq!(
            notice(from, &file(FileState::In(InState::Received))).unwrap(),
            "<message_file from=\"peer:m&quot;&lt;&amp;&gt;\" message=\"0193f0a2-6b1c-7d3e-9f40-1a2b3c4d5e6f\" name=\"a&quot;b&lt;c&gt;&amp;d.zip\" type=\"application/x-&quot;&lt;&amp;\" state=\"received\" saved=\"/Users/sky/.theuth/files/received/maya/2026-11-17/a&quot;b&lt;c&gt;&amp;d.zip\" bytes=\"1200000000\" blake3=\"7c1e\" holder=\"5f3a&quot;&lt;&amp;&gt;\" file=\"9d0c4b7e21a35f6880e1c2d3b4a59687\" machine=\"Sky's &quot;Mac&quot; &lt;&amp;&gt;\"/>\n\
             File notice from theuth, not from peer:m&quot;&lt;&amp;&gt;: no reply to the contact is needed. The file is saved on disk, not shown to you; open it with read_file if you need it."
        );
        let failed = file(FileState::In(InState::Failed));
        let el = "<message_file from=\"peer:m&quot;&lt;&amp;&gt;\" message=\"0193f0a2-6b1c-7d3e-9f40-1a2b3c4d5e6f\" name=\"a&quot;b&lt;c&gt;&amp;d.zip\" type=\"application/x-&quot;&lt;&amp;\" state=\"failed\" reason=\"the &quot;sender's&quot; &lt;file&gt; &amp; changed\" holder=\"5f3a&quot;&lt;&amp;&gt;\" file=\"9d0c4b7e21a35f6880e1c2d3b4a59687\"/>";
        assert_eq!(
            notice(from, &failed).unwrap(),
            format!(
                "{el}\nFile notice from theuth, not from peer:m&quot;&lt;&amp;&gt;: no reply to the contact is needed."
            )
        );
        assert_eq!(
            ended_notice(from, &failed).unwrap(),
            format!(
                "{el}\nFile notice from theuth: the share with peer:m&quot;&lt;&amp;&gt; has ended, so this was not given to the agent."
            )
        );
        // Only terminal incoming files have a notice.
        assert!(notice(from, &file(FileState::In(InState::Saved))).is_none());
        assert!(notice(from, &file(FileState::Out(OutState::Sent))).is_none());
    }

    #[test]
    fn state_is_tagged_by_direction_on_the_wire() {
        let f = file(FileState::In(InState::Saved));
        let v = serde_json::to_value(&f).unwrap();
        assert_eq!(
            (v["direction"].as_str(), v["state"].as_str()),
            (Some("in"), Some("saved"))
        );
        let back: ContactFile = serde_json::from_value(v).unwrap();
        assert_eq!(back, f);
        let out = serde_json::json!({"direction": "out", "state": "queued"});
        assert_eq!(
            serde_json::from_value::<FileState>(out).unwrap(),
            FileState::Out(OutState::Queued)
        );
        // An `out` state under `in` is refused, not misread.
        assert!(
            serde_json::from_value::<FileState>(
                serde_json::json!({"direction": "in", "state": "sent"})
            )
            .is_err()
        );
    }
}
