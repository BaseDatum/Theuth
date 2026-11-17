//! 199 CONTACT-FILES on `theuth/a2a/1`: the file reference part, the `/a2a/files/<id>`
//! route and its request and answer shapes, the pure checks both sides run, and this
//! crate's half of the agent seams: the route ([`serve`], matched by `reach::serve` before
//! `serve_http`) serves through [`agent::communication::files::offers`], and
//! [`ContactFetch`] (registered with `agent::communication::files::set_fetch`) is the
//! receiver's client over `Net::a2a_stream`. [`Transfers`] cuts a contact's active
//! transfers on both sides. Both sides speak HTTP/1.1 by hand: one request per stream, the
//! body framed by `content-length` alone, D8's one timer around every read and write.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::io::{self, SeekFrom};
use std::sync::{Arc, Weak};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use agent::communication::files::{
    ContactFile, Expiry, FILE_MAX, Fetch, Fetched, FileFetch, FileOffers, OfferError, Timer,
    TransferError,
};
pub use agent::communication::files::{Gone, REASON_MAX, Receipt};

use super::reach::{READ_TIMEOUT, read_line};
use crate::Ctx;
use crate::config::Staged;

/// The route prefix `reach::serve` matches before `serve_http` (199-E1a).
pub const FILES_PATH: &str = "/a2a/files/";
/// Largest receipt body (`POST /a2a/files/<id>`).
pub const RECEIPT_MAX: usize = 1024;

/// A 32-lowercase-hex local file id.
pub fn is_file_id(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The id of a request path `/a2a/files/<id>`, nothing more and nothing less.
pub fn file_id(path: &str) -> Option<&str> {
    path.strip_prefix(FILES_PATH).filter(|id| is_file_id(id))
}

/// `iroh://<holder>/a2a/files/<id>`: the reference part's URL; the id is the sender's.
pub fn file_url(holder: &str, id: &str) -> String {
    format!("iroh://{holder}{FILES_PATH}{id}")
}

/// `(holder endpoint, sender's id)` of a reference URL, or `None` when the scheme is
/// not `iroh`, the path is not exactly `/a2a/files/<32 lowercase hex>`, or the host is
/// not a bare lowercase endpoint id (no port, userinfo, query or fragment).
pub fn parse_file_url(url: &str) -> Option<(&str, &str)> {
    let rest = url.strip_prefix("iroh://")?;
    let slash = rest.find('/')?;
    let (host, path) = rest.split_at(slash);
    let plain = |b: u8| b.is_ascii_digit() || b.is_ascii_lowercase();
    if host.is_empty() || !host.bytes().all(plain) {
        return None;
    }
    Some((host, file_id(path)?))
}

/// The extension's metadata on a reference part, under the key [`super::EXT_FILES`].
/// Unknown keys are ignored, so a later `v1` field does not refuse the message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileMeta {
    /// Declared size in bytes. ProtoJSON carries part metadata as a `Struct`, whose
    /// numbers are doubles: `2516582` arrives as `2516582.0`, so a whole double is taken.
    #[serde(deserialize_with = "whole_bytes")]
    pub size: u64,
    /// Declared BLAKE3, 64 lowercase hex.
    pub blake3: String,
    /// When the sender stops serving it.
    pub expires: DateTime<Utc>,
}

/// A `u64`, or a non-negative whole double within 2^53 (exact; [`FILE_MAX`] is far below).
fn whole_bytes<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    let n = serde_json::Number::deserialize(d)?;
    if let Some(u) = n.as_u64() {
        return Ok(u);
    }
    match n.as_f64() {
        Some(f) if f >= 0.0 && f.fract() == 0.0 && f <= 9_007_199_254_740_992.0 => Ok(f as u64),
        _ => Err(serde::de::Error::custom(
            "size is not a whole number of bytes",
        )),
    }
}

impl FileMeta {
    /// Admission's checks on the declared values (`expires` already parsed).
    pub fn check(&self) -> Result<(), &'static str> {
        if self.size > FILE_MAX {
            return Err("size is over 5,000,000,000 bytes");
        }
        let hex = |b: u8| matches!(b, b'0'..=b'9' | b'a'..=b'f');
        if self.blake3.len() != 64 || !self.blake3.bytes().all(hex) {
            return Err("blake3 is not 64 lowercase hex");
        }
        Ok(())
    }
}

/// The one rule SDK decoding cannot check (it takes the first content field it
/// recognises): every part of a `SendMessage` JSON-RPC body sets exactly one of
/// `text`, `raw`, `url` or `data` (a JSON `null` is unset, as ProtoJSON reads it).
/// Runs on the raw body inside the 64 KiB bound, before the router decodes it.
/// `Err` is the `invalid_params` text, naming the part.
pub fn check_part_content(body: &serde_json::Value) -> Result<(), String> {
    let Some(parts) = body
        .pointer("/params/message/parts")
        .and_then(|p| p.as_array())
    else {
        return Ok(());
    };
    for (i, p) in parts.iter().enumerate() {
        let set = ["text", "raw", "url", "data"]
            .iter()
            .filter(|k| p.get(**k).is_some_and(|v| !v.is_null()))
            .count();
        if set != 1 {
            return Err(format!(
                "message.parts[{i}] must set exactly one of text, raw, url or data"
            ));
        }
    }
    Ok(())
}

// `Receipt`, `Gone` and `REASON_MAX` live in `agent::communication::files`, beside the
// `FileOffers` / `FileFetch` seams that carry them.

/// A receipt body, or `None` for a 400: over [`RECEIPT_MAX`], any other key or state, a
/// `received` with a reason, a `failed` without one, or a reason over [`REASON_MAX`].
pub fn parse_receipt(body: &[u8]) -> Option<Receipt> {
    // Serde's internally tagged enums ignore unknown keys on unit variants; a flat struct does not.
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Body {
        state: String,
        reason: Option<String>,
    }
    if body.len() > RECEIPT_MAX {
        return None;
    }
    let b: Body = serde_json::from_slice(body).ok()?;
    match (b.state.as_str(), b.reason) {
        ("received", None) => Some(Receipt::Received),
        ("failed", Some(reason)) if reason.len() <= REASON_MAX => Some(Receipt::Failed { reason }),
        _ => None,
    }
}

/// The start of a `Range: bytes=<from>-` header, the only form the route takes;
/// `None` is a 400 (any other unit, an end, a suffix or several ranges).
pub fn parse_range(header: &str) -> Option<u64> {
    let from = header.trim().strip_prefix("bytes=")?.strip_suffix('-')?;
    if from.is_empty() || !from.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    from.parse().ok()
}

/// `bytes <from>-<last>/<size>` → `(from, last, size)`; `None` when malformed.
pub fn parse_content_range(header: &str) -> Option<(u64, u64, u64)> {
    let (span, size) = header.trim().strip_prefix("bytes ")?.split_once('/')?;
    let (from, last) = span.split_once('-')?;
    let num = |s: &str| {
        (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .then(|| s.parse().ok())
            .flatten()
    };
    Some((num(from)?, num(last)?, num(size)?))
}

/// The sender's answer to a GET of an offer of `size` bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Serve {
    /// `200`, the whole file (no `Range` sent).
    Full {
        /// Bytes to send.
        size: u64,
    },
    /// `206`, from `from` to the end.
    Partial {
        /// First byte.
        from: u64,
        /// The whole size.
        size: u64,
    },
    /// `416`: `from ≥ size`.
    Unsatisfiable {
        /// The whole size.
        size: u64,
    },
}

impl Serve {
    /// What to answer for `range` (the parsed `Range`, if any).
    pub fn of(range: Option<u64>, size: u64) -> Self {
        match range {
            None => Self::Full { size },
            Some(from) if from >= size => Self::Unsatisfiable { size },
            Some(from) => Self::Partial { from, size },
        }
    }

    /// The status code.
    pub fn status(self) -> u16 {
        match self {
            Self::Full { .. } => 200,
            Self::Partial { .. } => 206,
            Self::Unsatisfiable { .. } => 416,
        }
    }

    /// The bytes the body carries: exactly this many, never more, even if the source grew.
    pub fn len(self) -> u64 {
        match self {
            Self::Full { size } => size,
            Self::Partial { from, size } => size - from,
            Self::Unsatisfiable { .. } => 0,
        }
    }

    /// Whether [`Self::len`] is zero.
    pub fn is_empty(self) -> bool {
        self.len() == 0
    }

    /// The headers beside the status.
    pub fn headers(self) -> Vec<(&'static str, String)> {
        match self {
            Self::Full { size } => vec![
                ("content-length", size.to_string()),
                ("accept-ranges", "bytes".into()),
            ],
            Self::Partial { from, size } => vec![
                ("content-range", format!("bytes {from}-{}/{size}", size - 1)),
                ("content-length", (size - from).to_string()),
            ],
            Self::Unsatisfiable { size } => vec![("content-range", format!("bytes */{size}"))],
        }
    }
}

/// The receiver's next step with `have` of `size` bytes in its partial.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Next {
    /// All bytes are here: no GET, verify.
    Verify,
    /// GET, with `Range: bytes=<from>-` when `Some`.
    Get {
        /// The resume offset.
        range: Option<u64>,
    },
}

impl Next {
    /// `size == 0` or `have == 0`: one GET without `Range`; `0 < have < size`: resume;
    /// `have == size`: verify.
    pub fn of(have: u64, size: u64) -> Self {
        if size == 0 || have == 0 {
            Self::Get { range: None }
        } else if have >= size {
            Self::Verify
        } else {
            Self::Get { range: Some(have) }
        }
    }

    /// The `Range` header value, when one is sent.
    pub fn range_header(self) -> Option<String> {
        match self {
            Self::Get { range: Some(from) } => Some(format!("bytes={from}-")),
            _ => None,
        }
    }
}

/// How the receiver takes the sender's answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The status and headers fit the request: stream the body into the partial
    /// (EOF before `content-length` is a transport failure, retried with `Range`).
    Stream,
    /// `404` or `410`: fail the file with the server's reason.
    Gone,
    /// Anything else (another status, offset, total or length, a `200` to a `Range`
    /// request, a `416`): "the sender's file changed".
    Changed,
}

/// The receiver's validation of an answer to `GET` with `range` (as [`Next::of`] chose)
/// for an offer of `size` bytes.
pub fn check_answer(
    range: Option<u64>,
    size: u64,
    status: u16,
    content_range: Option<&str>,
    content_length: Option<u64>,
) -> Verdict {
    match (status, range) {
        (404 | 410, _) => Verdict::Gone,
        (200, None) if content_length == Some(size) => Verdict::Stream,
        (206, Some(from))
            if from < size
                && content_range.and_then(parse_content_range) == Some((from, size - 1, size))
                && content_length == Some(size - from) =>
        {
            Verdict::Stream
        }
        _ => Verdict::Changed,
    }
}

/// Each contact's active file transfers, by record id (`ContactFacts.id`): the route's GETs
/// and receipts and this machine's downloads and receipt POSTs all run under the contact's
/// token, and a membership or block change [`Self::cut`]s them on both sides.
#[derive(Default)]
pub struct Transfers(parking_lot::Mutex<HashMap<String, CancellationToken>>);

impl Transfers {
    /// The token a transfer of `contact` runs under.
    pub fn token(&self, contact: &str) -> CancellationToken {
        self.0.lock().entry(contact.to_owned()).or_default().clone()
    }

    /// Cancel every active transfer of `contact`; later ones get a fresh token (and
    /// re-check membership when they start).
    pub fn cut(&self, contact: &str) {
        if let Some(t) = self.0.lock().remove(contact) {
            t.cancel();
        }
    }
}

// ---------------------------------------------------------------------------
// Who may transfer, and what a file-bearing message carries
// ---------------------------------------------------------------------------

/// The record id of the live contact whose group admits `eid` right now (not blocked, the
/// endpoint neither denied nor removed); `None` for anyone else, an import without a record
/// (`""`) included: offers are keyed by record id, and `""` would be every import's.
pub(crate) fn record_of(staged: &Staged, eid: &str) -> Option<String> {
    if staged.deny.contains(eid) || staged.removed.contains(eid) {
        return None;
    }
    staged
        .contacts
        .values()
        .find(|f| !f.blocked && !f.id.is_empty() && f.eids.iter().any(|e| e == eid))
        .map(|f| f.id.clone())
}

/// The record ids whose admitted machines or block state differ between `prev` and `next`
/// (one gone from `next`, removed or merged away, included): `Ctx::pass` cuts their
/// transfers right after the swap.
pub(crate) fn changed_members(prev: &Staged, next: &Staged) -> Vec<String> {
    let of = |s: &Staged| -> BTreeMap<String, (bool, Vec<String>)> {
        s.contacts
            .values()
            .filter(|f| !f.id.is_empty())
            .map(|f| {
                let mut eids: Vec<String> = f
                    .eids
                    .iter()
                    .filter(|e| !s.deny.contains(*e) && !s.removed.contains(*e))
                    .cloned()
                    .collect();
                eids.sort();
                (f.id.clone(), (f.blocked, eids))
            })
            .collect()
    };
    let after = of(next);
    of(prev)
        .into_iter()
        .filter(|(id, v)| after.get(id) != Some(v))
        .map(|(id, _)| id)
        .collect()
}

/// Admission of one `url` part (D1): an `iroh` reference to a file on one of `members` (the
/// caller's machines; none for a caller that is no contact), with sound [`FileMeta`]. `Err`
/// is why, for an `invalid_params` that names the part.
pub fn check_reference(
    url: &str,
    metadata: Option<&HashMap<String, serde_json::Value>>,
    members: &[String],
) -> Result<(), String> {
    let (host, _) = parse_file_url(url)
        .ok_or("the url is not iroh://<machine>/a2a/files/<32 lowercase hex>")?;
    if !members.iter().any(|e| e == host) {
        return Err("the url names a machine that is not the sender's".into());
    }
    let meta = metadata
        .and_then(|m| m.get(super::EXT_FILES))
        .ok_or("no files metadata")?;
    let meta: FileMeta =
        serde_json::from_value(meta.clone()).map_err(|e| format!("files metadata: {e}"))?;
    Ok(meta.check()?)
}

/// The reference part of an outgoing file (its `out` row): its URL on the holder, its name
/// and media type, and [`FileMeta`] under [`super::EXT_FILES`] (`expires` in whole seconds,
/// never later than the offer's deadline).
pub fn reference_part(f: &ContactFile) -> a2a_types::Part {
    let meta = serde_json::json!({
        "size": f.size.unwrap_or_default(),
        "blake3": f.blake3.as_deref().unwrap_or_default(),
        "expires": f.deadline.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    });
    let mut p = a2a_types::Part::url(file_url(&f.holder, &f.id));
    p.filename = Some(f.name.clone());
    p.media_type = Some(f.media_type.clone());
    p.metadata = Some(HashMap::from([(super::EXT_FILES.to_owned(), meta)]));
    p
}

// ---------------------------------------------------------------------------
// HTTP/1.1 on one stream, under D8's timer
// ---------------------------------------------------------------------------

/// Header lines read after the first, at most (each at most `reach::LINE_MAX`).
const HEAD_LINES: usize = 64;
/// One body read or write.
const CHUNK: usize = 64 * 1024;
/// The longest `410` body read (`expired`, `missing`, `changed`).
const GONE_MAX: u64 = 64;

/// `f` under `timer`, from `last` (the last progress), and `cut`: the outer `Err` is what
/// ended the transfer (no progress, the deadline, a cut); the inner one is `f`'s own.
async fn step<T>(
    timer: &Timer,
    last: Instant,
    cut: &CancellationToken,
    f: impl Future<Output = T>,
) -> Result<T, TransferError> {
    let (at, why) = timer.at(last);
    tokio::select! {
        biased;
        () = cut.cancelled() => Err(TransferError::Cancelled),
        r = tokio::time::timeout_at(at, f) => r.map_err(|_| match why {
            Expiry::Idle => TransferError::Idle,
            Expiry::Deadline => TransferError::Deadline,
        }),
    }
}

/// `name: value` of one header line (CRLF-terminated; a name with spaces, an obs-fold or
/// no colon is `None`); the name lowercased.
fn header(line: &[u8]) -> Option<(String, &str)> {
    let l = std::str::from_utf8(line.strip_suffix(b"\r\n")?).ok()?;
    let (name, value) = l.split_once(':')?;
    (!name.is_empty() && name.bytes().all(|b| b.is_ascii_graphic()))
        .then(|| (name.to_ascii_lowercase(), value.trim()))
}

/// A decimal header value; `None` for anything else (a sign, spaces, an overflow).
fn decimal(v: &str) -> Option<u64> {
    (!v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
        .then(|| v.parse().ok())
        .flatten()
}

/// What `410` said; an unknown text counts as `changed`.
async fn read_gone(r: &mut (impl AsyncRead + Unpin), len: Option<u64>) -> io::Result<Gone> {
    let mut text = Vec::new();
    r.take(len.unwrap_or(GONE_MAX).min(GONE_MAX))
        .read_to_end(&mut text)
        .await?;
    Ok(match &text[..] {
        b"expired" => Gone::Expired,
        b"missing" => Gone::Missing,
        _ => Gone::Changed,
    })
}

/// The local instant of a wall-clock deadline (now, if it passed).
fn instant_of(at: DateTime<Utc>) -> Instant {
    Instant::now() + (at - Utc::now()).to_std().unwrap_or_default()
}

// ---------------------------------------------------------------------------
// The route (the sender's side)
// ---------------------------------------------------------------------------

/// The first line `reach::serve` read names the file route: any method on [`FILES_PATH`]
/// (one the route does not take is its `400`).
pub(crate) fn is_route(line: &[u8]) -> bool {
    line.split(|b| *b == b' ')
        .nth(1)
        .is_some_and(|t| t.starts_with(FILES_PATH.as_bytes()))
}

/// A request the route takes; anything else is a `400`.
#[derive(Debug, PartialEq, Eq)]
enum Ask {
    /// `GET`, with the `Range` start when one was sent.
    Get { id: String, range: Option<u64> },
    /// `POST` of a receipt of `len` bytes (at most [`RECEIPT_MAX`]).
    Post { id: String, len: usize },
}

/// The request `line` and the rest of its head; `Ok(None)` is a `400`: another method,
/// version or path, a malformed or repeated `Range` or `Content-Length`, any
/// `Transfer-Encoding`, more than [`HEAD_LINES`] headers, a `GET` with a body, a `POST`
/// without one or over [`RECEIPT_MAX`]. The whole head is read either way.
async fn read_ask(s: &mut (impl AsyncRead + Unpin), line: &[u8]) -> io::Result<Option<Ask>> {
    let first = line
        .strip_suffix(b"\r\n")
        .and_then(|l| std::str::from_utf8(l).ok())
        .and_then(|l| {
            let mut it = l.split(' ');
            let (method, target, version) = (it.next()?, it.next()?, it.next()?);
            if it.next().is_some() || version != "HTTP/1.1" {
                return None;
            }
            Some((method.to_owned(), file_id(target)?.to_owned()))
        });
    let (mut range, mut length, mut bad) = (None, None, first.is_none());
    let mut lines = 0;
    loop {
        let l = read_line(s).await?;
        if l == b"\r\n" {
            break;
        }
        lines += 1;
        if lines > HEAD_LINES {
            return Ok(None);
        }
        match header(&l) {
            Some((name, v)) if name == "range" && range.is_none() => {
                range = Some(parse_range(v));
            }
            Some((name, v)) if name == "content-length" && length.is_none() => {
                length = Some(decimal(v));
            }
            Some((name, _))
                if matches!(
                    name.as_str(),
                    "range" | "content-length" | "transfer-encoding"
                ) =>
            {
                bad = true;
            }
            Some(_) => {}
            None => bad = true,
        }
    }
    let (Some((method, id)), false) = (first, bad) else {
        return Ok(None);
    };
    Ok(match (method.as_str(), range, length) {
        ("GET", None, None | Some(Some(0))) => Some(Ask::Get { id, range: None }),
        ("GET", Some(Some(from)), None | Some(Some(0))) => Some(Ask::Get {
            id,
            range: Some(from),
        }),
        ("POST", None, Some(Some(n))) if n <= RECEIPT_MAX as u64 => Some(Ask::Post {
            id,
            len: n as usize,
        }),
        _ => None,
    })
}

/// A status line and `headers`, then `connection: close` and the blank line.
fn head(status: u16, headers: &[(&str, String)]) -> String {
    let reason = match status {
        200 => "OK",
        206 => "Partial Content",
        404 => "Not Found",
        410 => "Gone",
        416 => "Range Not Satisfiable",
        503 => "Service Unavailable",
        _ => "Bad Request",
    };
    let mut h = format!("HTTP/1.1 {status} {reason}\r\n");
    for (k, v) in headers {
        let _ = write!(h, "{k}: {v}\r\n");
    }
    h.push_str("connection: close\r\n\r\n");
    h
}

/// A whole short answer (`text/plain` when it has a body), then the end of the stream, all
/// within [`READ_TIMEOUT`] (SEC-N2: a caller withholding stream credit never holds it longer).
async fn reply(s: &mut (impl AsyncWrite + Unpin), status: u16, body: &str) -> io::Result<()> {
    let mut headers = vec![("content-length", body.len().to_string())];
    if !body.is_empty() {
        headers.push(("content-type", "text/plain".into()));
    }
    let write = async {
        s.write_all(head(status, &headers).as_bytes()).await?;
        s.write_all(body.as_bytes()).await?;
        s.shutdown().await
    };
    tokio::time::timeout(READ_TIMEOUT, write)
        .await
        .unwrap_or_else(|_| Err(io::Error::new(io::ErrorKind::TimedOut, "reply not taken")))
}

fn refused(s: OfferError) -> (u16, &'static str) {
    match s {
        OfferError::Unknown => (404, ""),
        OfferError::Gone(g) => (410, g.as_str()),
        // The sender could not record it now: the receiver retries (SEC-A4).
        OfferError::Unavailable => (503, ""),
    }
}

/// One file route request of the contact `contact` (the caller's record id) on `s`, whose
/// first `line` `reach::serve` read: through `offers`, under `cut`, outside `serve_http`
/// (its 60 s deadline, `Route::of`, `forward::route` and the A2A rate and concurrency
/// bounds). The head, and a receipt's body, within [`READ_TIMEOUT`]; a `GET`'s body under
/// D8's timer to the offer's deadline, exactly the declared bytes even if the source grew.
pub(crate) async fn serve(
    mut s: impl AsyncRead + AsyncWrite + Unpin,
    line: &[u8],
    contact: &str,
    offers: &dyn FileOffers,
    cut: &CancellationToken,
) -> anyhow::Result<()> {
    let run = async {
        let ask = tokio::time::timeout(READ_TIMEOUT, read_ask(&mut s, line)).await??;
        let (id, range) = match ask {
            None => return Ok(reply(&mut s, 400, "").await?),
            Some(Ask::Post { id, len }) => {
                let mut body = vec![0; len];
                tokio::time::timeout(READ_TIMEOUT, s.read_exact(&mut body)).await??;
                let Some(receipt) = parse_receipt(&body) else {
                    return Ok(reply(&mut s, 400, "").await?);
                };
                let (status, text) = match offers.receipt(contact, &id, receipt).await {
                    Ok(()) => (200, ""),
                    Err(e) => refused(e),
                };
                return Ok(reply(&mut s, status, text).await?);
            }
            Some(Ask::Get { id, range }) => (id, range),
        };
        let h = match offers.open(contact, &id).await {
            Ok(h) if h.deadline <= Utc::now() => {
                return Ok(reply(&mut s, 410, Gone::Expired.as_str()).await?);
            }
            Ok(h) => h,
            Err(e) => {
                let (status, text) = refused(e);
                return Ok(reply(&mut s, status, text).await?);
            }
        };
        let answer = Serve::of(range, h.len);
        let mut headers = answer.headers();
        if let Serve::Unsatisfiable { .. } = answer {
            headers.push(("content-length", "0".into()));
        }
        // SEC-N2: the head and the end of the stream are under the same timer as the body.
        let timer = Timer::until(instant_of(h.deadline));
        step(
            &timer,
            Instant::now(),
            cut,
            s.write_all(head(answer.status(), &headers).as_bytes()),
        )
        .await??;
        let mut file = tokio::fs::File::from_std(h.file);
        let mut at = 0;
        if let Serve::Partial { from, .. } = answer {
            file.seek(SeekFrom::Start(from)).await?;
            at = from;
        }
        let (mut left, mut last, mut buf) = (answer.len(), Instant::now(), vec![0u8; CHUNK]);
        while left > 0 {
            let want = usize::try_from(left).map_or(CHUNK, |l| l.min(CHUNK));
            let sent = step(&timer, last, cut, async {
                let n = file.read(&mut buf[..want]).await?;
                s.write_all(&buf[..n]).await?;
                io::Result::Ok(n)
            })
            .await??;
            // A source shorter than declared: the receiver reads a short EOF and resumes
            // (then `open` answers `410 changed`).
            if sent == 0 {
                break;
            }
            left -= sent as u64;
            at += sent as u64;
            last = Instant::now();
            // UI-3: the sender's card counts what the receiver has.
            offers.served(contact, &id, at);
        }
        step(&timer, last, cut, s.shutdown()).await??;
        anyhow::Ok(())
    };
    tokio::select! {
        biased;
        () = cut.cancelled() => anyhow::bail!("199: the contact changed; transfer cut"),
        r = run => r,
    }
}

/// [`serve`] for the caller `remote` of `c`: its record id (a live contact whose record is
/// known), taken with its transfers' token and checked again after it, so a `cut` between
/// the gate and the token cannot miss this request. Anyone else reads `404`.
pub(crate) async fn route(
    c: &Ctx,
    remote: &str,
    line: &[u8],
    mut stream: Box<dyn replica::ByteStream>,
) -> anyhow::Result<()> {
    let record = || {
        let crate::server::IrohGate::Contact { alias, .. } = c.gate(remote) else {
            return None;
        };
        c.shared
            .read()
            .staged
            .contacts
            .get(&alias)
            .map(|f| f.id.clone())
            .filter(|id| !id.is_empty())
    };
    let Some(contact) = record() else {
        return Ok(reply(&mut stream, 404, "").await?);
    };
    let cut = c.transfers.token(&contact);
    if record().as_ref() != Some(&contact) {
        return Ok(reply(&mut stream, 404, "").await?);
    }
    serve(
        stream,
        line,
        &contact,
        &*agent::communication::files::offers(&c.rt),
        &cut,
    )
    .await
}

// ---------------------------------------------------------------------------
// The client (the receiver's side)
// ---------------------------------------------------------------------------

/// The answer's status and the two headers the receiver checks.
struct Answer {
    status: u16,
    content_range: Option<String>,
    content_length: Option<u64>,
}

/// A status line and the head after it: a malformed one (or a repeated `content-range` or
/// `content-length`, any `transfer-encoding`) is `InvalidData`.
async fn read_answer(r: &mut (impl AsyncRead + Unpin)) -> io::Result<Answer> {
    let bad = || io::Error::new(io::ErrorKind::InvalidData, "malformed answer");
    let line = read_line(r).await?;
    let status = std::str::from_utf8(&line)
        .ok()
        .and_then(|l| l.strip_prefix("HTTP/1.1 "))
        .and_then(|l| l.get(..3))
        .and_then(|c| c.parse().ok())
        .ok_or_else(bad)?;
    let (mut content_range, mut content_length) = (None, None);
    for _ in 0..=HEAD_LINES {
        let l = read_line(r).await?;
        if l == b"\r\n" {
            return Ok(Answer {
                status,
                content_range,
                content_length,
            });
        }
        let (name, v) = header(&l).ok_or_else(bad)?;
        match name.as_str() {
            "content-range" if content_range.is_none() => content_range = Some(v.to_owned()),
            "content-length" if content_length.is_none() => {
                content_length = Some(decimal(v).ok_or_else(bad)?);
            }
            "content-range" | "content-length" | "transfer-encoding" => return Err(bad()),
            _ => {}
        }
    }
    Err(bad())
}

fn transport(e: impl std::fmt::Display) -> TransferError {
    TransferError::Transport(e.to_string())
}

/// `member`'s stream, dialled over the contact connection (`Net::a2a_stream`, the dial
/// hints the plugin keeps for every contact machine).
async fn dial(
    net: &mesh::net::Net,
    member: &str,
) -> Result<Box<dyn replica::ByteStream>, TransferError> {
    net.a2a_stream(member, &[], None)
        .await
        .map_err(|e| transport(format!("{e:#}")))
}

/// The receiver's GET: stream `fetch` from its member over `Net::a2a_stream` into its
/// partial from `have` ([`Next::of`], [`check_answer`]), under `fetch.timer` and `cut`,
/// calling `progress` with the partial's size; then the whole partial's size and BLAKE3.
/// `fetch.member` must already be checked (`ContactFetch` does, before this dials).
pub async fn download(
    net: &mesh::net::Net,
    cut: &CancellationToken,
    fetch: &Fetch,
    progress: &mut (dyn FnMut(u64) + Send),
) -> Result<Fetched, TransferError> {
    fetch_with(dial(net, &fetch.member), cut, fetch, progress).await
}

/// [`download`] over the stream `connect` opens (only when a GET is needed): the dial, the
/// request and the answer's head count as no progress, each body frame goes straight into
/// the partial, and whatever ends it, the bytes written are flushed before it returns.
async fn fetch_with<S: AsyncRead + AsyncWrite + Unpin>(
    connect: impl Future<Output = Result<S, TransferError>>,
    cut: &CancellationToken,
    fetch: &Fetch,
    progress: &mut (dyn FnMut(u64) + Send),
) -> Result<Fetched, TransferError> {
    let started = Instant::now();
    let disk = TransferError::Disk;
    let mut opts = tokio::fs::OpenOptions::new();
    opts.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    opts.mode(0o600);
    let mut file = opts.open(&fetch.partial).await.map_err(disk)?;
    let len = file.metadata().await.map_err(disk)?.len();
    let have = fetch.have.min(len).min(fetch.size);
    file.set_len(have).await.map_err(disk)?;
    file.seek(SeekFrom::Start(have)).await.map_err(disk)?;
    let got = match Next::of(have, fetch.size) {
        Next::Verify => Ok(()),
        Next::Get { range } => {
            let body = async {
                let s = step(&fetch.timer, started, cut, connect).await??;
                let mut r = tokio::io::BufReader::new(s);
                let mut req = format!("GET {FILES_PATH}{} HTTP/1.1\r\nhost: theuth\r\n", fetch.id);
                if let Some(from) = range {
                    let _ = write!(req, "range: bytes={from}-\r\n");
                    tracing::info!(id = %fetch.id, size = fetch.size, "199: contact file resumes with Range: bytes={from}-");
                }
                req.push_str("\r\n");
                step(
                    &fetch.timer,
                    started,
                    cut,
                    r.get_mut().write_all(req.as_bytes()),
                )
                .await?
                .map_err(transport)?;
                let a = step(&fetch.timer, started, cut, read_answer(&mut r))
                    .await?
                    .map_err(transport)?;
                let (size, from) = (fetch.size, range.unwrap_or(0));
                match check_answer(
                    range,
                    size,
                    a.status,
                    a.content_range.as_deref(),
                    a.content_length,
                ) {
                    Verdict::Stream => {}
                    Verdict::Changed => return Err(TransferError::Changed),
                    Verdict::Gone if a.status == 404 => return Err(TransferError::Unknown),
                    Verdict::Gone => {
                        let g = step(
                            &fetch.timer,
                            started,
                            cut,
                            read_gone(&mut r, a.content_length),
                        )
                        .await?
                        .map_err(transport)?;
                        return Err(TransferError::Gone(g));
                    }
                }
                let (mut at, mut last, mut buf) = (from, started, vec![0u8; CHUNK]);
                while at < size {
                    let want = usize::try_from(size - at).map_or(CHUNK, |l| l.min(CHUNK));
                    let n = step(&fetch.timer, last, cut, r.read(&mut buf[..want]))
                        .await?
                        .map_err(transport)?;
                    if n == 0 {
                        return Err(transport("the sender's stream ended early"));
                    }
                    file.write_all(&buf[..n]).await.map_err(disk)?;
                    at += n as u64;
                    last = Instant::now();
                    progress(at);
                }
                Ok(())
            };
            body.await
        }
    };
    let flushed = file.flush().await;
    got?;
    flushed.map_err(disk)?;
    file.sync_all().await.map_err(disk)?;
    let file = file.into_std().await;
    let size = fetch.size;
    let blake3 = tokio::task::spawn_blocking(move || digest(&file, size))
        .await
        .map_err(transport)?
        .map_err(disk)?;
    Ok(Fetched {
        bytes: size,
        blake3,
    })
}

#[cfg(unix)]
fn digest(file: &std::fs::File, len: u64) -> io::Result<String> {
    agent::communication::files::disk::blake3_of(file, len)
}

// ponytail: the receiving side (publication, quarantine) is unix-only in `agent`; a port
// starts here.
#[cfg(not(unix))]
fn digest(_: &std::fs::File, _: u64) -> io::Result<String> {
    Err(io::Error::other("contact files need a unix host"))
}

/// The receiver's `POST /a2a/files/<id>` to `member` of `contact`: `Ok` on 200,
/// [`TransferError::Unknown`] on 404, [`TransferError::Gone`] on 410.
/// `member` must already be checked (`ContactFetch` does, before this dials).
pub async fn send_receipt(
    net: &mesh::net::Net,
    cut: &CancellationToken,
    member: &str,
    id: &str,
    receipt: &Receipt,
    timer: Timer,
) -> Result<(), TransferError> {
    receipt_with(dial(net, member), cut, id, receipt, timer).await
}

/// [`send_receipt`] over the stream `connect` opens; the whole exchange makes no progress,
/// so it ends at the timer's idle or deadline. A reason over [`REASON_MAX`] is cut to it
/// (the sender would refuse it, and every retry with it).
async fn receipt_with<S: AsyncRead + AsyncWrite + Unpin>(
    connect: impl Future<Output = Result<S, TransferError>>,
    cut: &CancellationToken,
    id: &str,
    receipt: &Receipt,
    timer: Timer,
) -> Result<(), TransferError> {
    let receipt = match receipt {
        Receipt::Failed { reason } if reason.len() > REASON_MAX => {
            let mut end = REASON_MAX;
            while !reason.is_char_boundary(end) {
                end -= 1;
            }
            Receipt::Failed {
                reason: reason[..end].to_owned(),
            }
        }
        r => r.clone(),
    };
    let body = serde_json::to_vec(&receipt).map_err(transport)?;
    let started = Instant::now();
    let s = step(&timer, started, cut, connect).await??;
    let mut r = tokio::io::BufReader::new(s);
    let req = format!(
        "POST {FILES_PATH}{id} HTTP/1.1\r\nhost: theuth\r\ncontent-type: application/json\r\n\
         content-length: {}\r\n\r\n",
        body.len()
    );
    let exchange = async {
        r.get_mut().write_all(req.as_bytes()).await?;
        r.get_mut().write_all(&body).await?;
        let a = read_answer(&mut r).await?;
        Ok::<_, io::Error>(match a.status {
            200 => Ok(()),
            404 => Err(TransferError::Unknown),
            410 => Err(TransferError::Gone(
                read_gone(&mut r, a.content_length).await?,
            )),
            s => Err(transport(format!("the receipt was answered HTTP {s}"))),
        })
    };
    step(&timer, started, cut, exchange)
        .await?
        .map_err(transport)?
}

/// This crate's [`FileFetch`], registered when the plugin attaches to the mesh endpoint.
/// Before `contacts_ready` (the mesh latch and the first reconcile) every call is the
/// retryable [`TransferError::NotYet`]: what staging shows then is display facts, not the
/// synced admission (SEC-N1). Then each call takes its contact's transfers token first and
/// requires the member to be admitted by the gate (`IrohGate::Contact`) as a machine of
/// that exact record (else [`TransferError::Cancelled`], with nothing dialled): a `cut`
/// after the check ends the call.
pub(crate) struct ContactFetch(pub(crate) Weak<Ctx>);

impl ContactFetch {
    fn live(
        &self,
        contact: &str,
        member: &str,
    ) -> Result<(CancellationToken, Arc<mesh::net::Net>), TransferError> {
        let off = || TransferError::Transport(super::OFF.into());
        let c = self.0.upgrade().ok_or_else(off)?;
        let net = c.net().ok_or_else(off)?;
        if !c.ready.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(TransferError::NotYet);
        }
        let cut = c.transfers.token(contact);
        let crate::server::IrohGate::Contact { alias, .. } = c.gate(member) else {
            return Err(TransferError::Cancelled);
        };
        let shared = c.shared.read();
        let same = shared
            .staged
            .contacts
            .get(&alias)
            .is_some_and(|f| f.id == contact);
        if !same || record_of(&shared.staged, member).as_deref() != Some(contact) {
            return Err(TransferError::Cancelled);
        }
        Ok((cut, net))
    }
}

#[async_trait::async_trait]
impl FileFetch for ContactFetch {
    async fn download(
        &self,
        fetch: &Fetch,
        progress: &mut (dyn FnMut(u64) + Send),
    ) -> Result<Fetched, TransferError> {
        let (cut, net) = self.live(&fetch.contact, &fetch.member)?;
        download(&net, &cut, fetch, progress).await
    }

    async fn send_receipt(
        &self,
        contact: &str,
        member: &str,
        id: &str,
        receipt: &Receipt,
        timer: Timer,
    ) -> Result<(), TransferError> {
        let (cut, net) = self.live(contact, member)?;
        send_receipt(&net, &cut, member, id, receipt, timer).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cutting_a_contact_cancels_its_transfers_only() {
        let t = Transfers::default();
        let (a, a2, b) = (t.token("rec-a"), t.token("rec-a"), t.token("rec-b"));
        t.cut("rec-a");
        assert!(a.is_cancelled() && a2.is_cancelled() && !b.is_cancelled());
        assert!(
            !t.token("rec-a").is_cancelled(),
            "a later transfer starts afresh"
        );
        t.cut("never-seen");
    }

    const ID: &str = "0f8e2d4c6a1b3957e0d2c4b6a8f01357";
    const HOLDER: &str = "5b2f0c8e9d41a7b3c6e05f1d2a9b8c7e4f3a2b1c0d9e8f7a6b5c4d3e2f1a0b9c";

    #[test]
    fn file_urls_and_paths() {
        let url = file_url(HOLDER, ID);
        assert_eq!(parse_file_url(&url), Some((HOLDER, ID)));
        assert_eq!(file_id(&format!("/a2a/files/{ID}")), Some(ID));
        for bad in [
            format!("https://{HOLDER}/a2a/files/{ID}"),
            format!("iroh://{HOLDER}/a2a/files/{}", ID.to_uppercase()),
            format!("iroh://{HOLDER}/a2a/files/{ID}/x"),
            format!("iroh://{HOLDER}/a2a/files/{ID}?q"),
            format!("iroh://{HOLDER}:80/a2a/files/{ID}"),
            format!("iroh://u@{HOLDER}/a2a/files/{ID}"),
            format!("iroh:///a2a/files/{ID}"),
            format!("iroh://{HOLDER}/a2a/files/{}", &ID[1..]),
            format!("iroh://{HOLDER}/a2a/file/{ID}"),
        ] {
            assert_eq!(parse_file_url(&bad), None, "{bad}");
        }
    }

    #[test]
    fn exactly_one_content_field() {
        let body = |parts: serde_json::Value| serde_json::json!({"jsonrpc": "2.0", "method": "SendMessage", "params": {"message": {"parts": parts}}});
        assert!(check_part_content(&body(serde_json::json!([{"text": "hi"}, {"url": "iroh://x/a2a/files/y", "filename": "a"}]))).is_ok());
        assert!(
            check_part_content(&body(serde_json::json!([{"raw": "AA==", "url": null}]))).is_ok()
        );
        assert_eq!(
            check_part_content(&body(
                serde_json::json!([{"text": "hi"}, {"text": "x", "url": "iroh://x"}])
            )),
            Err("message.parts[1] must set exactly one of text, raw, url or data".into())
        );
        assert!(check_part_content(&body(serde_json::json!([{"filename": "a"}]))).is_err());
        assert!(check_part_content(&body(serde_json::json!(["text"]))).is_err());
        assert!(check_part_content(&body(serde_json::json!([{"data": {}, "raw": ""}]))).is_err());
        // No parts at all is `validate_send`'s to refuse.
        assert!(check_part_content(&serde_json::json!({"params": {}})).is_ok());
    }

    #[test]
    fn file_reference_part_round_trips_through_protojson() {
        let meta = FileMeta {
            size: 2_516_582,
            blake3: "a3f1c09be27d4458e61f0b9c2d7e8a4f5b6c3d2e1f0a9b8c7d6e5f4a3b2c1d0e".into(),
            expires: "2026-11-24T14:32:00Z".parse().unwrap(),
        };
        assert_eq!(meta.check(), Ok(()));
        let mut part = a2a_types::Part::url(file_url(HOLDER, ID));
        part.filename = Some("handoff.pdf".into());
        part.media_type = Some("application/pdf".into());
        part.metadata = Some(
            [(
                super::super::EXT_FILES.to_owned(),
                serde_json::to_value(&meta).unwrap(),
            )]
            .into(),
        );
        let mut message = a2a_types::Message::new(
            a2a_types::Role::User,
            vec![a2a_types::Part::text("see"), part.clone()],
        );
        message.message_id = "m1".into();
        message.extensions = Some(vec![super::super::EXT_FILES.to_owned()]);
        let req = a2a_types::SendMessageRequest {
            message,
            configuration: None,
            metadata: None,
            tenant: None,
        };
        let wire = a2a_pb::protojson_conv::to_value(&req).unwrap();
        let p = &wire["message"]["parts"][1];
        assert_eq!(p["url"], file_url(HOLDER, ID).as_str());
        assert_eq!(
            (p["filename"].as_str(), p["mediaType"].as_str()),
            (Some("handoff.pdf"), Some("application/pdf"))
        );
        assert!(p.get("text").is_none() && p.get("raw").is_none());
        let back: a2a_types::SendMessageRequest = a2a_pb::protojson_conv::from_value(wire).unwrap();
        let got = &back.message.parts[1];
        assert_eq!(
            (&got.content, &got.filename, &got.media_type),
            (&part.content, &part.filename, &part.media_type)
        );
        // The metadata `Struct` brings `size` back as a double; `FileMeta` takes it whole.
        assert_eq!(
            got.metadata.as_ref().unwrap()[super::super::EXT_FILES]["size"],
            serde_json::json!(2_516_582.0)
        );
        let got: FileMeta = serde_json::from_value(
            back.message.parts[1].metadata.as_ref().unwrap()[super::super::EXT_FILES].clone(),
        )
        .unwrap();
        assert_eq!(got, meta);
        // Declared values admission refuses.
        assert!(
            FileMeta {
                size: FILE_MAX + 1,
                ..meta.clone()
            }
            .check()
            .is_err()
        );
        assert!(
            FileMeta {
                blake3: meta.blake3.to_uppercase(),
                ..meta.clone()
            }
            .check()
            .is_err()
        );
        assert!(
            serde_json::from_value::<FileMeta>(
                serde_json::json!({"size": 1, "blake3": meta.blake3, "expires": "soon"})
            )
            .is_err()
        );
    }

    #[test]
    fn receipts() {
        assert_eq!(
            parse_receipt(br#"{"state":"received"}"#),
            Some(Receipt::Received)
        );
        assert_eq!(
            parse_receipt(br#"{"state":"failed","reason":"disk full on Mac"}"#),
            Some(Receipt::Failed {
                reason: "disk full on Mac".into()
            })
        );
        let long = format!(r#"{{"state":"failed","reason":"{}"}}"#, "x".repeat(201));
        assert_eq!(parse_receipt(long.as_bytes()), None);
        let big = format!(r#"{{"state":"received","pad":"{}"}}"#, "x".repeat(1100));
        assert_eq!(parse_receipt(big.as_bytes()), None);
        assert_eq!(parse_receipt(br#"{"state":"received","x":1}"#), None);
        assert_eq!(parse_receipt(br#"{"state":"verified"}"#), None);
        assert_eq!(parse_receipt(b"received"), None);
        assert_eq!(parse_receipt(br#"{"state":"received","reason":"x"}"#), None);
        assert_eq!(parse_receipt(br#"{"state":"failed"}"#), None);
        // What the receiver sends is what the sender takes.
        for r in [
            Receipt::Received,
            Receipt::Failed {
                reason: "gone".into(),
            },
        ] {
            assert_eq!(parse_receipt(&serde_json::to_vec(&r).unwrap()), Some(r));
        }
        // A size the metadata `Struct` made a double is still whole bytes; a fraction is not.
        let meta = |size: serde_json::Value| {
            serde_json::from_value::<FileMeta>(
                serde_json::json!({"size": size, "blake3": "", "expires": "2026-11-24T14:32:00Z"}),
            )
        };
        assert_eq!(
            meta(serde_json::json!(5_000_000_000.0)).unwrap().size,
            FILE_MAX
        );
        assert!(meta(serde_json::json!(1.5)).is_err() && meta(serde_json::json!(-1)).is_err());
    }

    #[test]
    fn range_requests() {
        assert_eq!(parse_range("bytes=0-"), Some(0));
        assert_eq!(parse_range("bytes=1048576-"), Some(1_048_576));
        for bad in [
            "bytes=5-10",
            "bytes=-5",
            "bytes=0-,5-",
            "items=0-",
            "bytes=x-",
            "bytes=-",
            "bytes= 5-",
            "",
        ] {
            assert_eq!(parse_range(bad), None, "{bad}");
        }
        assert_eq!(Serve::of(None, 10), Serve::Full { size: 10 });
        assert_eq!(
            Serve::of(None, 10).headers(),
            [
                ("content-length", "10".to_owned()),
                ("accept-ranges", "bytes".to_owned())
            ]
        );
        let p = Serve::of(Some(4), 10);
        assert_eq!((p.status(), p.len()), (206, 6));
        assert_eq!(
            p.headers(),
            [
                ("content-range", "bytes 4-9/10".to_owned()),
                ("content-length", "6".to_owned())
            ]
        );
        let u = Serve::of(Some(10), 10);
        assert_eq!(
            (u.status(), u.headers()),
            (416, vec![("content-range", "bytes */10".to_owned())])
        );
        assert_eq!(Serve::of(Some(0), 0).status(), 416);
        assert_eq!(
            Serve::of(None, 0).headers()[0],
            ("content-length", "0".to_owned())
        );
    }

    #[test]
    fn receiver_steps_and_answers() {
        assert_eq!(Next::of(0, 0), Next::Get { range: None });
        assert_eq!(Next::of(0, 10), Next::Get { range: None });
        assert_eq!(Next::of(4, 10), Next::Get { range: Some(4) });
        assert_eq!(Next::of(4, 10).range_header().as_deref(), Some("bytes=4-"));
        assert_eq!(Next::of(10, 10), Next::Verify);
        assert_eq!(Next::of(0, 10).range_header(), None);
        // What the sender serves, the receiver takes.
        for (have, size) in [(0, 0), (0, 10), (4, 10), (9, 10)] {
            let Next::Get { range } = Next::of(have, size) else {
                unreachable!()
            };
            let s = Serve::of(range, size);
            let h = s.headers();
            let get = |k: &str| h.iter().find(|(n, _)| *n == k).map(|(_, v)| v.as_str());
            let len = get("content-length").map(|v| v.parse().unwrap());
            assert_eq!(
                check_answer(range, size, s.status(), get("content-range"), len),
                Verdict::Stream,
                "{have}/{size}"
            );
        }
        // Zero-length: 200 with content-length 0.
        assert_eq!(check_answer(None, 0, 200, None, Some(0)), Verdict::Stream);
        // A 416, a 200 to a Range request, a wrong total, offset or length.
        assert_eq!(
            check_answer(Some(4), 10, 416, Some("bytes */10"), None),
            Verdict::Changed
        );
        assert_eq!(
            check_answer(Some(4), 10, 200, None, Some(10)),
            Verdict::Changed
        );
        assert_eq!(
            check_answer(Some(4), 10, 206, Some("bytes 4-9/11"), Some(6)),
            Verdict::Changed
        );
        assert_eq!(
            check_answer(Some(4), 10, 206, Some("bytes 0-9/10"), Some(10)),
            Verdict::Changed
        );
        assert_eq!(
            check_answer(Some(4), 10, 206, Some("bytes 4-9/10"), Some(7)),
            Verdict::Changed
        );
        assert_eq!(
            check_answer(Some(4), 10, 206, None, Some(6)),
            Verdict::Changed
        );
        assert_eq!(
            check_answer(None, 10, 200, None, Some(11)),
            Verdict::Changed
        );
        assert_eq!(
            check_answer(None, 10, 206, Some("bytes 0-9/10"), Some(10)),
            Verdict::Changed
        );
        assert_eq!(check_answer(None, 10, 500, None, None), Verdict::Changed);
        assert_eq!(check_answer(Some(4), 10, 404, None, None), Verdict::Gone);
        assert_eq!(check_answer(None, 10, 410, None, None), Verdict::Gone);
        assert_eq!(Gone::Changed.as_str(), "changed");
    }

    fn staged_facts(id: &str, eids: &[&str], blocked: bool) -> crate::config::ContactFacts {
        crate::config::ContactFacts {
            id: id.into(),
            eid: eids[0].into(),
            eids: eids.iter().map(|e| (*e).to_owned()).collect(),
            display: String::new(),
            machine: String::new(),
            first_contact: Default::default(),
            blocked,
            paired_ms: 0,
            addrs: Vec::new(),
        }
    }

    /// D10: only a live contact's current machine transfers, by its record id; an import
    /// without a record never does. A pass cuts exactly the records whose admitted machines
    /// or block state changed (a reorder is no change).
    #[test]
    fn who_may_transfer_and_whom_a_pass_cuts() {
        let mut s = Staged::default();
        s.contacts.insert(
            "maya".into(),
            staged_facts("rec-maya", &["m1", "m2"], false),
        );
        s.contacts
            .insert("old".into(), staged_facts("", &["i1"], false));
        s.contacts
            .insert("bob".into(), staged_facts("rec-bob", &["b1"], true));
        assert_eq!(record_of(&s, "m2").as_deref(), Some("rec-maya"));
        for nobody in ["i1", "b1", "x"] {
            assert_eq!(record_of(&s, nobody), None, "{nobody}");
        }
        let mut denied = s.clone();
        denied.deny.insert("m2".into());
        assert_eq!(record_of(&denied, "m2"), None);

        assert!(changed_members(&s, &s.clone()).is_empty());
        let mut reordered = s.clone();
        reordered.contacts.get_mut("maya").unwrap().eids.reverse();
        assert!(changed_members(&s, &reordered).is_empty());
        let maya = || vec!["rec-maya".to_owned()];
        let mut grown = s.clone();
        grown
            .contacts
            .get_mut("maya")
            .unwrap()
            .eids
            .push("m3".into());
        assert_eq!(changed_members(&s, &grown), maya());
        let mut blocked = s.clone();
        blocked.contacts.get_mut("maya").unwrap().blocked = true;
        assert_eq!(changed_members(&s, &blocked), maya());
        let mut gone = s.clone();
        gone.contacts.remove("maya");
        assert_eq!(changed_members(&s, &gone), maya());
        let mut removed = s.clone();
        removed.removed.insert("m1".into());
        assert_eq!(changed_members(&s, &removed), maya());
        assert_eq!(changed_members(&s, &denied), maya());
    }

    /// The route and the client over an in-memory stream, against a fake of the agent's
    /// offers (E1b implements the real one).
    #[cfg(unix)]
    mod transfer {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        use std::path::{Path, PathBuf};
        use std::time::Duration;

        use agent::communication::files::OfferHandle;
        use tokio::io::DuplexStream;
        use tokio::task::JoinHandle;

        use super::*;

        const ME: &str = "rec-me";
        const F1: &str = "11111111111111111111111111111111";
        const F2: &str = "22222222222222222222222222222222";

        fn bytes(n: u64) -> Vec<u8> {
            (0..n).map(|i| (i * 7 % 251) as u8).collect()
        }

        fn blake3(path: &Path, len: u64) -> String {
            agent::communication::files::disk::blake3_of(&std::fs::File::open(path).unwrap(), len)
                .unwrap()
        }

        /// One offer as the fake keeps it: its source, declared length and inode at offer time.
        struct Offer {
            contact: String,
            path: PathBuf,
            len: u64,
            inode: u64,
            deadline: DateTime<Utc>,
        }

        /// E1b's answers in miniature: another contact's id is unknown, a vanished source
        /// `missing`, another inode `changed`, a receipt past the deadline `expired`.
        #[derive(Default)]
        struct Fake {
            offers: parking_lot::Mutex<HashMap<String, Offer>>,
            receipts: parking_lot::Mutex<Vec<Receipt>>,
        }

        impl Fake {
            fn offer(&self, id: &str, path: &Path, len: u64) {
                let offer = Offer {
                    contact: ME.into(),
                    path: path.into(),
                    len,
                    inode: std::fs::metadata(path).unwrap().ino(),
                    deadline: Utc::now() + chrono::Duration::days(7),
                };
                self.offers.lock().insert(id.into(), offer);
            }

            fn expire(&self, id: &str) {
                self.offers.lock().get_mut(id).unwrap().deadline =
                    Utc::now() - chrono::Duration::seconds(1);
            }
        }

        #[async_trait::async_trait]
        impl FileOffers for Fake {
            async fn open(&self, contact: &str, id: &str) -> Result<OfferHandle, OfferError> {
                let all = self.offers.lock();
                let o = all
                    .get(id)
                    .filter(|o| o.contact == contact)
                    .ok_or(OfferError::Unknown)?;
                let file =
                    std::fs::File::open(&o.path).map_err(|_| OfferError::Gone(Gone::Missing))?;
                if file.metadata().map(|m| m.ino()).ok() != Some(o.inode) {
                    return Err(OfferError::Gone(Gone::Changed));
                }
                Ok(OfferHandle {
                    file,
                    len: o.len,
                    blake3: String::new(),
                    deadline: o.deadline,
                })
            }

            async fn receipt(&self, contact: &str, id: &str, r: Receipt) -> Result<(), OfferError> {
                let all = self.offers.lock();
                let o = all
                    .get(id)
                    .filter(|o| o.contact == contact)
                    .ok_or(OfferError::Unknown)?;
                if o.deadline <= Utc::now() {
                    return Err(OfferError::Gone(Gone::Expired));
                }
                self.receipts.lock().push(r);
                Ok(())
            }
        }

        /// The route for `contact` on one end of a pipe; the other end is the caller's.
        fn route_to(
            fake: &Arc<Fake>,
            contact: &str,
            cut: &CancellationToken,
            cap: usize,
        ) -> (DuplexStream, JoinHandle<anyhow::Result<()>>) {
            let (near, mut far) = tokio::io::duplex(cap);
            let (fake, contact, cut) = (fake.clone(), contact.to_owned(), cut.clone());
            let h = tokio::spawn(async move {
                let line = read_line(&mut far).await?;
                serve(far, &line, &contact, &*fake, &cut).await
            });
            (near, h)
        }

        /// `req` sent raw to the route: the answer's head and body.
        async fn ask(fake: &Arc<Fake>, contact: &str, req: &str) -> (String, Vec<u8>) {
            let (mut s, h) = route_to(fake, contact, &CancellationToken::new(), 1 << 20);
            s.write_all(req.as_bytes()).await.unwrap();
            let mut all = Vec::new();
            s.read_to_end(&mut all).await.unwrap();
            h.await.unwrap().unwrap();
            let at = all.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
            (
                String::from_utf8(all[..at].to_vec()).unwrap(),
                all[at + 4..].to_vec(),
            )
        }

        /// A request's head, as a sender reads it.
        async fn read_head(s: &mut DuplexStream) -> String {
            let mut head = String::new();
            loop {
                let l = read_line(s).await.unwrap();
                head.push_str(std::str::from_utf8(&l).unwrap());
                if l == b"\r\n" {
                    return head;
                }
            }
        }

        /// A sender that answers `answer` to whatever is asked, then sends `frames` (each
        /// after its delay) and closes, or stalls (holding the stream) when `stall`. The
        /// handle has the request's head.
        fn sender(
            answer: String,
            frames: Vec<(Duration, Vec<u8>)>,
            stall: bool,
        ) -> (DuplexStream, JoinHandle<String>) {
            let (near, mut far) = tokio::io::duplex(1 << 20);
            let h = tokio::spawn(async move {
                let head = read_head(&mut far).await;
                if far.write_all(answer.as_bytes()).await.is_err() {
                    return head;
                }
                for (wait, f) in frames {
                    tokio::time::sleep(wait).await;
                    if far.write_all(&f).await.is_err() {
                        return head;
                    }
                }
                if stall {
                    std::future::pending::<()>().await;
                }
                let _ = far.shutdown().await;
                head
            });
            (near, h)
        }

        fn ok_head(len: u64) -> String {
            head(200, &[("content-length", len.to_string())])
        }

        fn far_timer() -> Timer {
            Timer::until(Instant::now() + Duration::from_secs(7 * 24 * 3600))
        }

        fn job(dir: &Path, size: u64, have: u64, timer: Timer) -> Fetch {
            Fetch {
                contact: ME.into(),
                member: "m1".into(),
                id: F1.into(),
                size,
                partial: dir.join("partial"),
                have,
                timer,
            }
        }

        async fn over<S: AsyncRead + AsyncWrite + Unpin>(
            s: S,
            cut: &CancellationToken,
            f: &Fetch,
            seen: &mut Vec<u64>,
        ) -> Result<Fetched, TransferError> {
            fetch_with(async { Ok(s) }, cut, f, &mut |n| seen.push(n)).await
        }

        /// The route answers only 200, 206, 400, 404, 410 and 416, by the design's rules,
        /// and never sends more than the declared length.
        #[tokio::test]
        async fn the_route_answers_by_the_range_rules() {
            let dir = tempfile::tempdir().unwrap();
            let fake = Arc::new(Fake::default());
            let src = dir.path().join("src");
            std::fs::write(&src, bytes(10)).unwrap();
            fake.offer(F1, &src, 10);
            let get = |id: &str, extra: &str| {
                format!("GET /a2a/files/{id} HTTP/1.1\r\nhost: theuth\r\n{extra}\r\n")
            };

            let (h, b) = ask(&fake, ME, &get(F1, "")).await;
            assert!(h.starts_with("HTTP/1.1 200 OK\r\n"), "{h}");
            assert!(
                h.contains("content-length: 10\r\naccept-ranges: bytes"),
                "{h}"
            );
            assert_eq!(b, bytes(10));
            let (h, b) = ask(&fake, ME, &get(F1, "Range: bytes=4-\r\n")).await;
            assert!(h.starts_with("HTTP/1.1 206 "), "{h}");
            assert!(
                h.contains("content-range: bytes 4-9/10\r\ncontent-length: 6"),
                "{h}"
            );
            assert_eq!(b, bytes(10)[4..]);
            let (h, b) = ask(&fake, ME, &get(F1, "range: bytes=10-\r\n")).await;
            assert!(h.starts_with("HTTP/1.1 416 "), "{h}");
            assert!(h.contains("content-range: bytes */10"), "{h}");
            assert!(b.is_empty());

            for bad in [
                get(F1, "range: bytes=4-6\r\n"),
                get(F1, "range: bytes=4-\r\nrange: bytes=5-\r\n"),
                get(F1, "content-length: 3\r\n"),
                get(F1, "transfer-encoding: chunked\r\n"),
                get(F1, "no colon\r\n"),
                get("xyz", ""),
                format!("PUT /a2a/files/{F1} HTTP/1.1\r\n\r\n"),
                format!("GET /a2a/files/{F1} HTTP/1.0\r\n\r\n"),
                format!("GET /a2a/files/{F1}/x HTTP/1.1\r\n\r\n"),
                format!("POST /a2a/files/{F1} HTTP/1.1\r\n\r\n"),
                format!("POST /a2a/files/{F1} HTTP/1.1\r\ncontent-length: 2000\r\n\r\n"),
            ] {
                let (h, _) = ask(&fake, ME, &bad).await;
                assert!(h.starts_with("HTTP/1.1 400 "), "{bad:?}: {h}");
            }

            // Another contact's offer and an unknown id read the same 404.
            let (h, _) = ask(&fake, "rec-other", &get(F1, "")).await;
            assert!(h.starts_with("HTTP/1.1 404 "), "{h}");
            let (h, _) = ask(&fake, ME, &get(&"3".repeat(32), "")).await;
            assert!(h.starts_with("HTTP/1.1 404 "), "{h}");

            // A grown original is served at its declared length.
            {
                use std::io::Write as _;
                let mut f = std::fs::OpenOptions::new().append(true).open(&src).unwrap();
                f.write_all(b"grown").unwrap();
            }
            let (h, b) = ask(&fake, ME, &get(F1, "")).await;
            assert!(h.contains("content-length: 10\r\n"), "{h}");
            assert_eq!(b, bytes(10));
            // Replaced by another file (another inode), then gone.
            let other = dir.path().join("other");
            std::fs::write(&other, bytes(10)).unwrap();
            std::fs::rename(&other, &src).unwrap();
            let (h, b) = ask(&fake, ME, &get(F1, "")).await;
            assert!(h.starts_with("HTTP/1.1 410 Gone\r\n"), "{h}");
            assert!(h.contains("content-type: text/plain"), "{h}");
            assert_eq!(b, b"changed");
            std::fs::remove_file(&src).unwrap();
            assert_eq!(ask(&fake, ME, &get(F1, "")).await.1, b"missing");

            // Zero length: 200 with content-length 0; any Range is past its end.
            let empty = dir.path().join("empty");
            std::fs::write(&empty, b"").unwrap();
            fake.offer(F2, &empty, 0);
            let (h, b) = ask(&fake, ME, &get(F2, "")).await;
            assert!(
                h.starts_with("HTTP/1.1 200 ") && h.contains("content-length: 0"),
                "{h}"
            );
            assert!(b.is_empty());
            let (h, _) = ask(&fake, ME, &get(F2, "range: bytes=0-\r\n")).await;
            assert!(h.starts_with("HTTP/1.1 416 "), "{h}");
            fake.expire(F2);
            assert_eq!(ask(&fake, ME, &get(F2, "")).await.1, b"expired");

            // Receipts: a strict body, the offer's contact only, within its deadline.
            let post = |body: &str| {
                format!(
                    "POST /a2a/files/{F1} HTTP/1.1\r\ncontent-length: {}\r\n\r\n{body}",
                    body.len()
                )
            };
            let received = post(r#"{"state":"received"}"#);
            assert!(
                ask(&fake, ME, &received)
                    .await
                    .0
                    .starts_with("HTTP/1.1 200 ")
            );
            assert!(
                ask(&fake, "rec-other", &received)
                    .await
                    .0
                    .starts_with("HTTP/1.1 404 ")
            );
            let verified = post(r#"{"state":"verified"}"#);
            assert!(
                ask(&fake, ME, &verified)
                    .await
                    .0
                    .starts_with("HTTP/1.1 400 ")
            );
            assert_eq!(*fake.receipts.lock(), [Receipt::Received]);
        }

        /// Resume: a cut stream leaves its bytes in the partial (a transport failure), the
        /// next GET asks `Range: bytes=<have>-` and the whole partial is verified; all
        /// bytes present dials nothing; a zero-length file is one plain GET.
        #[tokio::test]
        async fn a_download_resumes_after_a_cut_and_verifies_the_whole_partial() {
            let dir = tempfile::tempdir().unwrap();
            let fake = Arc::new(Fake::default());
            let size = 3 * CHUNK as u64 + 17;
            let src = dir.path().join("src");
            std::fs::write(&src, bytes(size)).unwrap();
            fake.offer(F1, &src, size);
            let want = blake3(&src, size);
            let cut = CancellationToken::new();
            let f = job(dir.path(), size, 0, far_timer());

            let half = CHUNK as u64 + 5;
            let (s, asked) = sender(
                ok_head(size),
                vec![(Duration::ZERO, bytes(size)[..half as usize].to_vec())],
                false,
            );
            let mut seen = Vec::new();
            let err = over(s, &cut, &f, &mut seen).await.unwrap_err();
            assert!(matches!(err, TransferError::Transport(_)), "{err}");
            assert!(!asked.await.unwrap().contains("range:"));
            let have = std::fs::metadata(&f.partial).unwrap().len();
            assert_eq!((have, seen.last()), (half, Some(&half)));
            assert_eq!(
                std::fs::metadata(&f.partial).unwrap().permissions().mode() & 0o777,
                0o600
            );

            let (s, _h) = route_to(&fake, ME, &cut, 1 << 20);
            let got = over(s, &cut, &Fetch { have, ..f.clone() }, &mut seen)
                .await
                .unwrap();
            assert_eq!(
                got,
                Fetched {
                    bytes: size,
                    blake3: want.clone()
                }
            );
            assert_eq!(std::fs::read(&f.partial).unwrap(), bytes(size));
            assert_eq!(seen.last(), Some(&size));

            let never = async { Err::<DuplexStream, _>(transport("dialled")) };
            let all = Fetch {
                have: size,
                ..f.clone()
            };
            let got = fetch_with(never, &cut, &all, &mut |_| {}).await.unwrap();
            assert_eq!(got.blake3, want);

            let empty = dir.path().join("empty");
            std::fs::write(&empty, b"").unwrap();
            fake.offer(F2, &empty, 0);
            let (s, _h) = route_to(&fake, ME, &cut, 1 << 20);
            let zero = Fetch {
                id: F2.into(),
                partial: dir.path().join("zero"),
                ..job(dir.path(), 0, 0, far_timer())
            };
            let got = over(s, &cut, &zero, &mut Vec::new()).await.unwrap();
            assert_eq!((got.bytes, got.blake3), (0, blake3(&empty, 0)));
        }

        /// Every answer that does not fit the request fails the file as changed; 404 and 410
        /// carry the sender's reason; a short EOF or a malformed head is a transport failure.
        #[tokio::test]
        async fn answers_that_do_not_fit_the_request() {
            let dir = tempfile::tempdir().unwrap();
            let cut = CancellationToken::new();
            let partial = dir.path().join("partial");
            let cases: Vec<(u64, String, Vec<u8>, &str)> = vec![
                (4, ok_head(10), bytes(10), "changed"),
                (
                    4,
                    head(
                        206,
                        &[
                            ("content-range", "bytes 4-9/11".into()),
                            ("content-length", "6".into()),
                        ],
                    ),
                    bytes(6),
                    "changed",
                ),
                (
                    4,
                    head(416, &[("content-range", "bytes */10".into())]),
                    vec![],
                    "changed",
                ),
                (
                    0,
                    head(
                        206,
                        &[
                            ("content-range", "bytes 0-9/10".into()),
                            ("content-length", "10".into()),
                        ],
                    ),
                    bytes(10),
                    "changed",
                ),
                (
                    0,
                    head(404, &[("content-length", "0".into())]),
                    vec![],
                    "unknown",
                ),
                (
                    0,
                    head(410, &[("content-length", "7".into())]),
                    b"missing".to_vec(),
                    "missing",
                ),
                (0, ok_head(10), bytes(3), "transport"),
                (0, "garbage\r\n\r\n".into(), vec![], "transport"),
            ];
            for (have, answer, body, want) in cases {
                std::fs::write(&partial, bytes(have)).unwrap();
                let (s, asked) = sender(answer.clone(), vec![(Duration::ZERO, body)], false);
                let f = job(dir.path(), 10, have, far_timer());
                let err = over(s, &cut, &f, &mut Vec::new()).await.unwrap_err();
                let got = match err {
                    TransferError::Changed => "changed",
                    TransferError::Unknown => "unknown",
                    TransferError::Gone(Gone::Missing) => "missing",
                    TransferError::Transport(_) => "transport",
                    other => panic!("{answer:?}: {other}"),
                };
                assert_eq!(got, want, "{answer:?}");
                let asked = asked.await.unwrap();
                assert_eq!(asked.contains("range: bytes=4-\r\n"), have == 4, "{asked}");
            }
        }

        /// D8: one timer. A slow transfer past the old 60 s bound completes; a head that
        /// never comes and a body that stalls end at idle (the bytes so far kept); a drip
        /// that is never idle ends at the deadline.
        #[tokio::test(start_paused = true)]
        async fn one_timer_idle_between_frames_and_the_deadline_over_all() {
            let dir = tempfile::tempdir().unwrap();
            let cut = CancellationToken::new();
            let sec = Duration::from_secs;

            let t0 = Instant::now();
            let drip = (0..4).map(|b| (sec(30), vec![b])).collect();
            let (s, _h) = sender(ok_head(4), drip, false);
            let got = over(
                s,
                &cut,
                &job(dir.path(), 4, 0, far_timer()),
                &mut Vec::new(),
            )
            .await
            .unwrap();
            assert_eq!(got.bytes, 4);
            assert!(t0.elapsed() >= sec(120), "{:?}", t0.elapsed());

            let t0 = Instant::now();
            let (near, mut far) = tokio::io::duplex(1024);
            let _silent = tokio::spawn(async move {
                read_head(&mut far).await;
                std::future::pending::<()>().await;
            });
            let err = over(
                near,
                &cut,
                &job(dir.path(), 4, 0, far_timer()),
                &mut Vec::new(),
            )
            .await
            .unwrap_err();
            assert!(matches!(err, TransferError::Idle), "{err}");
            let idle = agent::communication::files::FILE_IDLE;
            assert!(t0.elapsed() >= idle, "{:?}", t0.elapsed());

            let (s, _h) = sender(ok_head(10), vec![(Duration::ZERO, bytes(3))], true);
            let f = job(dir.path(), 10, 0, far_timer());
            let err = over(s, &cut, &f, &mut Vec::new()).await.unwrap_err();
            assert!(matches!(err, TransferError::Idle), "{err}");
            assert_eq!(std::fs::read(&f.partial).unwrap(), bytes(3));

            let t0 = Instant::now();
            let drip = (0..100).map(|b| (sec(59), vec![b])).collect();
            let (s, _h) = sender(ok_head(100), drip, false);
            let short = Timer::until(t0 + sec(300));
            let err = over(s, &cut, &job(dir.path(), 100, 0, short), &mut Vec::new())
                .await
                .unwrap_err();
            assert!(matches!(err, TransferError::Deadline), "{err}");
            assert!(
                t0.elapsed() >= sec(300) && t0.elapsed() < sec(301),
                "{:?}",
                t0.elapsed()
            );
        }

        /// D10: a cut ends a download mid-body (`Cancelled`, its bytes kept) and a route
        /// mid-serve (a receiver that stopped reading would otherwise hold it to its idle).
        #[tokio::test]
        async fn a_cut_ends_both_sides_mid_transfer() {
            let dir = tempfile::tempdir().unwrap();
            let t = Transfers::default();
            let cut = t.token(ME);
            let (s, _h) = sender(ok_head(10), vec![(Duration::ZERO, bytes(3))], true);
            let f = job(dir.path(), 10, 0, far_timer());
            let err = fetch_with(async { Ok(s) }, &cut, &f, &mut |n| {
                if n == 3 {
                    t.cut(ME);
                }
            })
            .await
            .unwrap_err();
            assert!(matches!(err, TransferError::Cancelled), "{err}");
            assert_eq!(std::fs::metadata(&f.partial).unwrap().len(), 3);

            let fake = Arc::new(Fake::default());
            let src = dir.path().join("src");
            std::fs::write(&src, bytes(4 * CHUNK as u64)).unwrap();
            fake.offer(F1, &src, 4 * CHUNK as u64);
            let cut = t.token(ME);
            let (mut near, h) = route_to(&fake, ME, &cut, 1024);
            near.write_all(format!("GET /a2a/files/{F1} HTTP/1.1\r\n\r\n").as_bytes())
                .await
                .unwrap();
            assert!(read_head(&mut near).await.starts_with("HTTP/1.1 200 "));
            t.cut(ME);
            let served = tokio::time::timeout(Duration::from_secs(10), h)
                .await
                .expect("the cut ends the route at once")
                .unwrap();
            assert!(served.unwrap_err().to_string().contains("cut"));
        }

        /// The receipt reaches the offer through the route; a reason over the bound is cut
        /// to it at a character boundary; another contact reads 404, a late one 410.
        #[tokio::test]
        async fn receipts_reach_the_offer() {
            let dir = tempfile::tempdir().unwrap();
            let fake = Arc::new(Fake::default());
            let src = dir.path().join("src");
            std::fs::write(&src, b"x").unwrap();
            fake.offer(F1, &src, 1);
            let cut = CancellationToken::new();
            let send = async |contact: &str, r: Receipt| {
                let (s, _h) = route_to(&fake, contact, &cut, 1 << 20);
                receipt_with(async { Ok(s) }, &cut, F1, &r, far_timer()).await
            };
            send(ME, Receipt::Received).await.unwrap();
            let long = Receipt::Failed {
                reason: "é".repeat(150),
            };
            send(ME, long).await.unwrap();
            let err = send("rec-other", Receipt::Received).await.unwrap_err();
            assert!(matches!(err, TransferError::Unknown), "{err}");
            fake.expire(F1);
            let err = send(ME, Receipt::Received).await.unwrap_err();
            assert!(matches!(err, TransferError::Gone(Gone::Expired)), "{err}");
            let got = fake.receipts.lock().clone();
            assert_eq!(got[0], Receipt::Received);
            let Receipt::Failed { reason } = &got[1] else {
                panic!("{got:?}")
            };
            assert_eq!(reason.len(), REASON_MAX);
        }

        /// 199-Z1 SEC-N2: a caller that sends its request and then never reads (withheld
        /// stream credit) holds the route no longer than its bounds: the response head of a
        /// known offer ends at D8's idle, a short `404` within the control timeout.
        #[tokio::test(start_paused = true)]
        async fn a_blocked_head_or_reply_write_ends_within_its_bound() {
            let dir = tempfile::tempdir().unwrap();
            let fake = Arc::new(Fake::default());
            let src = dir.path().join("src");
            std::fs::write(&src, bytes(100_000)).unwrap();
            fake.offer(F1, &src, 100_000);
            for (id, bound) in [
                (F1, agent::communication::files::FILE_IDLE),
                (F2, READ_TIMEOUT),
            ] {
                let (mut s, h) = route_to(&fake, ME, &CancellationToken::new(), 1);
                let req = format!("GET {FILES_PATH}{id} HTTP/1.1\r\nhost: theuth\r\n\r\n");
                s.write_all(req.as_bytes()).await.unwrap();
                let started = tokio::time::Instant::now();
                let served = tokio::time::timeout(bound + Duration::from_secs(1), h)
                    .await
                    .expect("the write is bounded")
                    .unwrap();
                assert!(served.is_err(), "{id}: nothing was taken, yet it succeeded");
                assert!(started.elapsed() >= bound - Duration::from_secs(1), "{id}");
                drop(s);
            }
        }

        /// SEC-A4: a receipt the sender could not record answers `503`, which the receiver
        /// takes as retryable (a transport error), never as the sender's acknowledgement.
        #[tokio::test]
        async fn an_unrecorded_receipt_is_retryable() {
            struct Down;
            #[async_trait::async_trait]
            impl FileOffers for Down {
                async fn open(&self, _: &str, _: &str) -> Result<OfferHandle, OfferError> {
                    Err(OfferError::Unavailable)
                }
                async fn receipt(&self, _: &str, _: &str, _: Receipt) -> Result<(), OfferError> {
                    Err(OfferError::Unavailable)
                }
            }
            let (near, mut far) = tokio::io::duplex(1 << 16);
            let cut = CancellationToken::new();
            let route = {
                let cut = cut.clone();
                tokio::spawn(async move {
                    let line = read_line(&mut far).await?;
                    serve(far, &line, ME, &Down, &cut).await
                })
            };
            let err = receipt_with(
                async { Ok(near) },
                &cut,
                F1,
                &Receipt::Received,
                far_timer(),
            )
            .await
            .unwrap_err();
            assert!(matches!(err, TransferError::Transport(_)), "{err}");
            route.await.unwrap().unwrap();
        }
    }
}
