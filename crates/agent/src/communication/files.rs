//! 199 CONTACT-FILES: the frozen shapes of files between contacts, the pure pieces every
//! builder shares, and (P1, 199-E1b) this engine's agent side: [`send`] (offers),
//! [`intake`] (admission rows), [`service`] (queue, publication, States, receipts, the
//! notice outbox, the offers the a2a route serves, recovery), [`notices`] (D17) and
//! [`holder`] (the holder RPCs). [`NOT_YET`] is what a path answers before its service runs.
//!
//! - Types: [`ContactFile`] and its states, the notice operation's answers.
//! - [`sanitise`] and [`candidate`]: the saved name (Storage and sanitisation).
//! - [`placeholders`], [`notice`], [`ended_notice`]: every line the model sees.
//! - [`disk`] (unix): the egress gate, the protected set, BLAKE3 from a handle, the
//!   macOS quarantine and no-clobber publication by inode.
//! - [`HeldContactFile`]: the seam the artifact mesh (P2) asks for a held file.
//! - [`FileOffers`] (this crate implements, a2a's file route calls) and [`FileFetch`] (a2a
//!   implements over the contact connection, this crate's receive queue calls): the seams
//!   between `crates/agent` and `crates/a2a`, which depends on this crate and not the
//!   other way round. Each is a process-wide registration with a no-op default.

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use kameo::message::{Context, Message};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::loop_::attr;
use crate::store::Store;
use crate::supervisor::Agents;

/// Egress gate, protected set, hashing, quarantine and publication.
#[cfg(unix)]
pub mod disk;
/// Holder RPCs: the card, a bounded thumbnail, a no-clobber export.
pub mod holder;
/// Admission: a message's reference parts as `in` rows.
pub mod intake;
/// D17: the typed notice, its admission on the chat's owner and the holder's door.
pub mod notices;
/// Sending: the gate, clones, hashes and staged offers of one message.
#[cfg(unix)]
pub mod send;
/// The queue, publication, States, receipts, the outbox, the offers and recovery.
#[cfg(unix)]
pub mod service;

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
// The a2a ↔ agent seams (E1a ↔ E1b)
// ---------------------------------------------------------------------------

/// Longest failure reason in a receipt, in bytes.
pub const REASON_MAX: usize = 200;

/// A receipt (`POST /a2a/files/<sender-id>`): the receiver's terminal outcome. On the
/// wire `{"state":"received"}` or `{"state":"failed","reason":"…"}` (a2a parses it,
/// `contacts::files::parse_receipt`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum Receipt {
    /// Saved, and its notice durably queued.
    Received,
    /// Failed on the receiver.
    Failed {
        /// Why, at most [`REASON_MAX`] bytes.
        reason: String,
    },
}

/// The `410` text: why an offer can no longer be served.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gone {
    /// Past its deadline.
    Expired,
    /// The source (and any clone) is gone.
    Missing,
    /// The source is another file, or another size, than the one offered.
    Changed,
}

impl Gone {
    /// The `text/plain` body.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Expired => "expired",
            Self::Missing => "missing",
            Self::Changed => "changed",
        }
    }
}

/// Why the sender's side refuses a GET or a receipt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OfferError {
    /// `404`: no such offer, or the caller's contact is not the offer's (one answer for both).
    Unknown,
    /// `410` with its text.
    Gone(Gone),
}

/// An offer opened for serving: the clone, or the original reopened through
/// `disk::egress_reopen` (its `(dev, inode)` and size re-checked by the implementation).
#[derive(Debug)]
pub struct OfferHandle {
    /// Read-only; serve at most `len` bytes of it.
    pub file: std::fs::File,
    /// The declared size.
    pub len: u64,
    /// The declared BLAKE3, 64 lowercase hex.
    pub blake3: String,
    /// The offer's local deadline (D8).
    pub deadline: DateTime<Utc>,
}

/// The sender's offers, as a2a's `/a2a/files/<id>` route reaches them (199-E1b implements).
/// `contact` is the caller's immutable record id (`ContactFacts.id`), never an alias; `id`
/// is the sender's local file id from the path.
#[async_trait::async_trait]
pub trait FileOffers: Send + Sync {
    /// `GET`: the offer `id` of `contact`, ready to serve.
    async fn open(&self, contact: &str, id: &str) -> Result<OfferHandle, OfferError>;
    /// `POST`: the receiver's terminal receipt; only the first one applies, a repeat is `Ok`.
    async fn receipt(&self, contact: &str, id: &str, receipt: Receipt) -> Result<(), OfferError>;
}

/// No offers: every GET and receipt is `404`, until 199-E1b registers its own.
pub struct NoOffers;

#[async_trait::async_trait]
impl FileOffers for NoOffers {
    async fn open(&self, _: &str, _: &str) -> Result<OfferHandle, OfferError> {
        Err(OfferError::Unknown)
    }

    async fn receipt(&self, _: &str, _: &str, _: Receipt) -> Result<(), OfferError> {
        Err(OfferError::Unknown)
    }
}

static OFFERS: parking_lot::RwLock<Option<std::sync::Arc<dyn FileOffers>>> =
    parking_lot::const_rwlock(None);

/// Register this engine's offers (the agent plugin, at start; a restart replaces them).
pub fn set_offers(offers: std::sync::Arc<dyn FileOffers>) {
    *OFFERS.write() = Some(offers);
}

/// The registered offers, else [`NoOffers`].
pub fn offers() -> std::sync::Arc<dyn FileOffers> {
    OFFERS
        .read()
        .clone()
        .unwrap_or_else(|| std::sync::Arc::new(NoOffers))
}

/// D8's one timer for an active transfer: it ends at `min(deadline, last progress + idle)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timer {
    /// The local deadline: fails the file.
    pub deadline: tokio::time::Instant,
    /// No progress this long releases the slot ([`FILE_IDLE`]; shorter in tests).
    pub idle: Duration,
}

/// Which bound ended a transfer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expiry {
    /// No progress for `idle`: back to the queue, resumed with `Range`.
    Idle,
    /// The local deadline: the file fails.
    Deadline,
}

impl Timer {
    /// [`FILE_IDLE`] under `deadline`.
    pub fn until(deadline: tokio::time::Instant) -> Self {
        Self {
            deadline,
            idle: FILE_IDLE,
        }
    }

    /// When the transfer ends if nothing moves after `last`, and why.
    pub fn at(&self, last: tokio::time::Instant) -> (tokio::time::Instant, Expiry) {
        let idle = last + self.idle;
        if self.deadline <= idle {
            (self.deadline, Expiry::Deadline)
        } else {
            (idle, Expiry::Idle)
        }
    }
}

/// One download: file `id` (the sender's, from the reference URL) of `contact` (its record
/// id) from roster member `member` (the URL's host, which must map to `contact`).
#[derive(Clone, Debug)]
pub struct Fetch {
    /// The contact's record id.
    pub contact: String,
    /// The endpoint dialled.
    pub member: String,
    /// The sender's local id.
    pub id: String,
    /// The declared size.
    pub size: u64,
    /// `files/partial/<local-id>`, written from `have`.
    pub partial: PathBuf,
    /// Bytes already in the partial (`Range: bytes=<have>-` when `0 < have < size`).
    pub have: u64,
    /// The transfer's timer.
    pub timer: Timer,
}

/// A finished download: the partial holds `bytes`, whose BLAKE3 is `blake3` (the caller
/// compares both with the declared values).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fetched {
    /// Size of the whole partial.
    pub bytes: u64,
    /// BLAKE3 of the whole partial, 64 lowercase hex.
    pub blake3: String,
}

/// Why a download or a receipt POST ended without success.
#[derive(Debug)]
pub enum TransferError {
    /// `404`: the sender does not offer it (to us).
    Unknown,
    /// `410` with the sender's reason.
    Gone(Gone),
    /// Another status, offset, total or length, a `200` to a `Range` request, or a `416`.
    Changed,
    /// No progress for the timer's `idle`: requeue, resume with `Range`.
    Idle,
    /// The local deadline passed.
    Deadline,
    /// The contact's membership or block state changed ([`FileFetch`]'s cancellation).
    Cancelled,
    /// Dial, stream reset, stream limit, or EOF before `content-length`: retry with backoff.
    Transport(String),
    /// Writing the partial failed (disk full …).
    Disk(std::io::Error),
    /// No fetcher is registered (199-E1a builds it).
    NotYet,
}

impl std::fmt::Display for TransferError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown => f.write_str("the sender no longer offers it"),
            Self::Gone(Gone::Expired) => f.write_str("the offer expired"),
            Self::Gone(Gone::Missing) => f.write_str("the sender's file is gone"),
            Self::Gone(Gone::Changed) | Self::Changed => f.write_str("the sender's file changed"),
            Self::Idle => f.write_str("no progress"),
            Self::Deadline => f.write_str("the offer expired"),
            Self::Cancelled => f.write_str("the contact changed"),
            Self::Transport(e) => write!(f, "transfer failed: {e}"),
            Self::Disk(e) => write!(f, "saving failed: {e}"),
            Self::NotYet => f.write_str(NOT_YET),
        }
    }
}

impl std::error::Error for TransferError {}

/// The receiver's side of the contact connection (199-E1a implements it in a2a over
/// `Net::a2a_stream`; 199-E1b's queue calls it). Both calls run under the a2a side's
/// per-contact cancellation, so a membership or block change cuts them.
#[async_trait::async_trait]
pub trait FileFetch: Send + Sync {
    /// Stream `fetch` into its partial from `have`, calling `progress` with the partial's
    /// size as bytes land.
    async fn download(
        &self,
        fetch: &Fetch,
        progress: &mut (dyn FnMut(u64) + Send),
    ) -> Result<Fetched, TransferError>;
    /// `POST /a2a/files/<id>` to `member` of `contact`, under `timer`.
    async fn send_receipt(
        &self,
        contact: &str,
        member: &str,
        id: &str,
        receipt: &Receipt,
        timer: Timer,
    ) -> Result<(), TransferError>;
}

/// No fetcher: every call is [`TransferError::NotYet`].
pub struct NoFetch;

#[async_trait::async_trait]
impl FileFetch for NoFetch {
    async fn download(
        &self,
        _: &Fetch,
        _: &mut (dyn FnMut(u64) + Send),
    ) -> Result<Fetched, TransferError> {
        Err(TransferError::NotYet)
    }

    async fn send_receipt(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: &Receipt,
        _: Timer,
    ) -> Result<(), TransferError> {
        Err(TransferError::NotYet)
    }
}

static FETCH: parking_lot::RwLock<Option<std::sync::Arc<dyn FileFetch>>> =
    parking_lot::const_rwlock(None);

/// Register the contact fetcher (the a2a plugin, once it serves `theuth/a2a/1`).
pub fn set_fetch(fetch: std::sync::Arc<dyn FileFetch>) {
    *FETCH.write() = Some(fetch);
}

/// The registered fetcher, else [`NoFetch`].
pub fn fetch() -> std::sync::Arc<dyn FileFetch> {
    FETCH
        .read()
        .clone()
        .unwrap_or_else(|| std::sync::Arc::new(NoFetch))
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
        m: HeldContactFile,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        held(&self.store, &m.id).await.unwrap_or_else(|e| {
            tracing::warn!(id = %m.id, error = %e, "held contact file");
            None
        })
    }
}

/// The `received` `in` row `id` of `contact_files` with its path, `(dev, inode)`, size
/// and BLAKE3 recorded. An `out` row, an unfinished or failed one, or an unknown id: `None`.
pub(crate) async fn held(store: &Store, id: &str) -> anyhow::Result<Option<HeldFile>> {
    let row: Option<(String, i64, i64, i64, String)> = sqlx::query_as(
        "SELECT path, dev, inode, size, blake3 FROM contact_files \
         WHERE id = ? AND direction = 'in' AND state = 'received' AND path IS NOT NULL \
         AND dev IS NOT NULL AND inode IS NOT NULL AND size IS NOT NULL AND blake3 IS NOT NULL",
    )
    .bind(id)
    .fetch_optional(store.pool())
    .await?;
    // SQLite keeps the u64s as i64 bit for bit.
    Ok(row.map(|(path, dev, inode, size, blake3)| HeldFile {
        path: path.into(),
        dev: dev as u64,
        inode: inode as u64,
        size: size as u64,
        blake3,
    }))
}

/// What a fetching machine expects of `(holder, id)`: its `received` notice in this
/// engine's own copy of the session (D11), never the holder's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceivedNotice {
    /// The sanitised declared name (the cache file's name).
    pub name: String,
    /// Actual size.
    pub size: u64,
    /// Actual BLAKE3, 64 lowercase hex.
    pub blake3: String,
    /// The holder's machine name.
    pub machine: String,
}

/// D11: the `received` notice of `(holder, id)` in `session`, found by its notice id
/// through the committed predicate, in either form: the row at the `at_seq` of its
/// `input_accepted` step, or the ended-share `errors` row. `None`: no such notice, a
/// `failed` one, or one that names another file.
pub(crate) async fn received_notice(
    store: &Store,
    session: runtime::SessionId,
    holder: &str,
    id: &str,
) -> anyhow::Result<Option<ReceivedNotice>> {
    let notice = notice_id(holder, id);
    if !store.notice_committed(session, notice).await? {
        return Ok(None);
    }
    let (s, n) = (session.to_string(), notice.to_string());
    let step: Option<String> = sqlx::query_scalar(
        "SELECT m.body FROM turn_steps t JOIN messages m \
         ON m.session_id = t.session_id AND m.seq = t.at_seq \
         WHERE t.session_id = ?1 AND t.kind = 'input_accepted' \
         AND json_extract(t.payload, '$.input_id') = ?2 LIMIT 1",
    )
    .bind(&s)
    .bind(&n)
    .fetch_optional(store.pool())
    .await?;
    let step = match step {
        Some(body) => Some(crate::loop_::text_of(
            &serde_json::from_str::<genai::chat::ChatMessage>(&body)?.content,
        )),
        None => None,
    };
    let ended: Option<String> =
        sqlx::query_scalar("SELECT text FROM errors WHERE session_id = ?1 AND id = ?2")
            .bind(&s)
            .bind(&n)
            .fetch_optional(store.pool())
            .await?;
    Ok([step, ended]
        .into_iter()
        .flatten()
        .find_map(|t| parse_received(&t, holder, id)))
}

/// The first `<message_file …/>` element of `text` when it is the `received` notice of
/// `(holder, id)`. Values are [`attr`]-escaped, so neither `"` nor `/>` occurs inside one.
fn parse_received(text: &str, holder: &str, id: &str) -> Option<ReceivedNotice> {
    let el = &text[text.find("<message_file ")? + "<message_file".len()..];
    let mut rest = &el[..el.find("/>")?];
    let mut attrs = std::collections::HashMap::new();
    while let Some((key, r)) = rest.split_once("=\"") {
        let (value, r) = r.split_once('"')?;
        let value = value
            .replace("&quot;", "\"")
            .replace("&gt;", ">")
            .replace("&lt;", "<")
            .replace("&amp;", "&");
        attrs.insert(key.trim(), value);
        rest = r;
    }
    let get = |k: &str| attrs.get(k).map(String::as_str);
    if get("state") != Some("received") || get("holder") != Some(holder) || get("file") != Some(id)
    {
        return None;
    }
    let blake3 = get("blake3")
        .filter(|b| b.len() == 64 && b.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f')))?;
    Some(ReceivedNotice {
        name: get("name")?.to_owned(),
        size: get("bytes")?.parse().ok()?,
        blake3: blake3.to_owned(),
        machine: get("machine").unwrap_or_default().to_owned(),
    })
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
    fn the_timer_ends_at_the_nearer_bound() {
        let now = tokio::time::Instant::now();
        let far = Timer::until(now + Duration::from_secs(3600));
        assert_eq!(far.at(now), (now + FILE_IDLE, Expiry::Idle));
        let near = Timer::until(now + Duration::from_secs(5));
        assert_eq!(
            near.at(now),
            (now + Duration::from_secs(5), Expiry::Deadline)
        );
        // A drip keeps resetting `idle`, never the deadline.
        assert_eq!(near.at(now + Duration::from_secs(4)).1, Expiry::Deadline);
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

    async fn store() -> (Store, runtime::SessionId, crate::TestDir) {
        let dir = crate::TestDir::new("files");
        let store = Store::open(&dir.0.join("sessions.sqlite"), None)
            .await
            .unwrap();
        let id = runtime::SessionId::new();
        store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        (store, id, dir)
    }

    /// 199 P2: only a `received` `in` row is held; `out`, unfinished, failed and unknown
    /// ids are not (the holder answers `Missing`).
    #[tokio::test]
    async fn only_a_received_incoming_row_is_held() {
        let (store, session, _dir) = store().await;
        let rows = [
            ("a", "in", "received"),
            ("b", "in", "saved"),
            ("c", "out", "sent"),
            ("d", "in", "failed"),
        ];
        for (id, direction, state) in rows {
            sqlx::query(
                "INSERT INTO contact_files (id, direction, session, exchange, seq, contact_id, \
                 name, media_type, size, blake3, source, dev, inode, path, state, created_ms, \
                 deadline_ms) VALUES (?, ?, ?, 'x', 0, 'rec', 'a.zip', 'application/zip', 7, \
                 'b3', 'src', -1, 2, '/p/a.zip', ?, 0, 0)",
            )
            .bind(id.repeat(32))
            .bind(direction)
            .bind(session.to_string())
            .bind(state)
            .execute(store.pool())
            .await
            .unwrap();
        }
        assert_eq!(
            held(&store, &"a".repeat(32)).await.unwrap(),
            Some(HeldFile {
                path: "/p/a.zip".into(),
                dev: u64::MAX,
                inode: 2,
                size: 7,
                blake3: "b3".into(),
            })
        );
        for id in ["b", "c", "d", "e"] {
            assert_eq!(held(&store, &id.repeat(32)).await.unwrap(), None, "{id}");
        }
    }

    /// 199 P2 (D11): the expected values come from the committed notice in either form,
    /// the row at its journal step's `at_seq` or the ended-share `errors` row, with every
    /// attribute unescaped. A failed notice, one naming another file, or none: `None`.
    #[tokio::test]
    async fn the_received_notice_is_read_in_either_form() {
        let (store, session, _dir) = store().await;
        let from = "peer:maya";
        let mut f = file(FileState::In(InState::Received));
        f.blake3 = Some("7c".repeat(32));
        let want = ReceivedNotice {
            name: f.name.clone(),
            size: 1_200_000_000,
            blake3: "7c".repeat(32),
            machine: f.machine.clone(),
        };
        let found = async |id: &str| received_notice(&store, session, &f.holder, id).await;
        assert_eq!(found(&f.id).await.unwrap(), None, "no notice yet");

        // The journal form: the row, its step at that seq, then later rows.
        store
            .append(
                session,
                &genai::chat::ChatMessage::user(notice(from, &f).unwrap()),
            )
            .await
            .unwrap();
        let input = notice_id(&f.holder, &f.id).to_string();
        store
            .input_accepted(
                session,
                runtime::TurnId::new(),
                Some(&input),
                JOURNAL_SOURCE,
            )
            .await
            .unwrap();
        store
            .append(session, &genai::chat::ChatMessage::assistant("later"))
            .await
            .unwrap();
        assert_eq!(found(&f.id).await.unwrap(), Some(want.clone()));

        // The ended-share form.
        let ended = |g: &ContactFile| {
            let text = ended_notice(from, g).unwrap();
            let store = store.clone();
            let id = notice_id(&f.holder, &g.id);
            async move { store.record_error_once(session, id, &text).await.unwrap() }
        };
        let mut g = f.clone();
        g.id = "41b8d2f0c7e65a1394b2a8d0e3f1c5a7".into();
        ended(&g).await;
        assert_eq!(found(&g.id).await.unwrap(), Some(want));

        // A failed notice has no bytes to fetch.
        let mut failed = f.clone();
        failed.id = "c".repeat(32);
        failed.state = FileState::In(InState::Failed);
        ended(&failed).await;
        assert_eq!(found(&failed.id).await.unwrap(), None);

        // A notice id whose row names another file is never trusted for this one.
        let mut lying = f.clone();
        lying.id = "d".repeat(32);
        let text = ended_notice(from, &f).unwrap();
        store
            .record_error_once(session, notice_id(&f.holder, &lying.id), &text)
            .await
            .unwrap();
        assert_eq!(found(&lying.id).await.unwrap(), None);
    }
}
