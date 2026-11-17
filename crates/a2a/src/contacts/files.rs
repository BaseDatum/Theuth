//! 199 CONTACT-FILES on `theuth/a2a/1` (P0 contract): the file reference part, the
//! `/a2a/files/<id>` route's request and answer shapes, the pure checks both sides run,
//! and this crate's half of the agent seams: the route serves through
//! [`agent::communication::files::offers`], and [`ContactFetch`] (registered with
//! `agent::communication::files::set_fetch`) is the receiver's client over
//! `Net::a2a_stream`. [`Transfers`] cuts a contact's active transfers on both sides.
//! Nothing is wired yet: [`super::EXT_FILES`] is not advertised, no route matches
//! [`FILES_PATH`], [`check_part_content`] is not called by admission, and the client
//! answers `NotYet`.

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use agent::communication::files::{FILE_MAX, Fetch, Fetched, FileFetch, Timer, TransferError};
pub use agent::communication::files::{Gone, REASON_MAX, Receipt};

use crate::Ctx;

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

/// The receiver's GET: stream `fetch` from its member over `Net::a2a_stream` into its
/// partial from `have` ([`Next::of`], [`check_answer`]), under `fetch.timer` and `cut`,
/// calling `progress` with the partial's size; then the whole partial's size and BLAKE3.
pub async fn download(
    _net: &mesh::net::Net,
    _cut: &CancellationToken,
    _fetch: &Fetch,
    _progress: &mut (dyn FnMut(u64) + Send),
) -> Result<Fetched, TransferError> {
    // 199-E1a: dial only a member of `fetch.contact`; headers within `reach::READ_TIMEOUT`,
    // then the timer; body frames straight into the partial, never a `Limited` collect.
    Err(TransferError::NotYet)
}

/// The receiver's `POST /a2a/files/<id>` to `member` of `contact`: `Ok` on 200,
/// [`TransferError::Unknown`] on 404, [`TransferError::Gone`] on 410.
pub async fn send_receipt(
    _net: &mesh::net::Net,
    _cut: &CancellationToken,
    _member: &str,
    _id: &str,
    _receipt: &Receipt,
    _timer: Timer,
) -> Result<(), TransferError> {
    // 199-E1a: body `serde_json::to_vec(receipt)` (at most `RECEIPT_MAX`).
    Err(TransferError::NotYet)
}

/// This crate's [`FileFetch`], registered when the plugin attaches to the mesh endpoint.
pub(crate) struct ContactFetch(pub(crate) Weak<Ctx>);

impl ContactFetch {
    fn live(&self) -> Result<(Arc<Ctx>, Arc<mesh::net::Net>), TransferError> {
        let c = self
            .0
            .upgrade()
            .ok_or_else(|| TransferError::Transport(super::OFF.into()))?;
        let net = c
            .net()
            .ok_or_else(|| TransferError::Transport(super::OFF.into()))?;
        Ok((c, net))
    }
}

#[async_trait::async_trait]
impl FileFetch for ContactFetch {
    async fn download(
        &self,
        fetch: &Fetch,
        progress: &mut (dyn FnMut(u64) + Send),
    ) -> Result<Fetched, TransferError> {
        let (c, net) = self.live()?;
        let cut = c.transfers.token(&fetch.contact);
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
        let (c, net) = self.live()?;
        let cut = c.transfers.token(contact);
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
}
