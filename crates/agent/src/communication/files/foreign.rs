//! 199 P3 (D16), 199-E3: files from and to foreign A2A peers (HTTP peers, no files
//! extension).
//!
//! - **In.** An inline `raw` part or an `https` URL part becomes an `in` row of its
//!   exchange, like a contact's reference ([`in_rows`], [`admit`]): the same placeholders,
//!   queue, publication and notices; the holder is this engine. `source` is the URL, or the
//!   bytes themselves as a `data:` URL (a restart loses nothing); `contact_id` is the
//!   sender's binding, `peer:<uuid>` ([`is_foreign`]). The queue calls [`fetch`]: an `https`
//!   source through [`web::fetch::download`] (public-only, HTTPS on every hop, one timer,
//!   BLAKE3 while streaming, at most 5 GB, no resume: a failed attempt restarts from byte
//!   0), a `data:` source decoded into the partial. Size and BLAKE3 are measured here.
//! - **Out.** Toward such a peer a file goes inline as `raw`, only when the whole request
//!   fits the 64 KiB envelope ([`inline_fit`]); nothing is ever uploaded anywhere.

use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;

use a2a_types::{Part, PartContent};
use base64::Engine as _;
use chrono::{DateTime, Utc};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use web::fetch::{DownloadError, DownloadOpts};

use super::{OFFER_TTL, notice_id, sanitise};
use crate::store::Store;
use crate::store::contact_files::FileRow;

/// The `source` prefix of a file that came inline: its bytes follow, standard base64.
pub const DATA: &str = "data:;base64,";
/// The serialized `SendMessage` request bound (`a2a::server::BODY_LIMIT`).
pub const ENVELOPE: usize = 64 * 1024;
/// The failure of an address the public-only guard (or the scheme check) refuses.
pub const NOT_PUBLIC: &str = "not a public HTTPS address";
/// The failure of a foreign download that never finished before its deadline.
pub const NO_RESPONSE: &str = "no response";
/// The failure of a foreign file whose authority ended (grant changed or gone, restart).
pub const SHARE_ENDED: &str = "share ended";
/// The holder's mark on a foreign notice's `contact` when the file's authority ended:
/// the chat's owner records it as the engine-state row (D17 ended share), no turn.
pub const ENDED: &str = "ended:";

/// What each foreign file arrived under, by its notice id: the grant epoch of the HTTP
/// peer's route (credentials, exports and roots as admitted; for an answer, the route it
/// was sent under). Held by the file's holder, in memory only, like a task's admission: a
/// restart forgets it, so an unfinished foreign file then fails `share ended` (fail
/// closed). The holder alone judges it: its outbox marks an ended file's notice
/// ([`notice_contact`]); the chat's owner, possibly another engine of the mesh, honours
/// that mark and, when it holds the entry itself, re-checks it at consumption. Entries
/// go when the notice is delivered, or with an exchange that ends without notices.
static AUTHORITY: LazyLock<parking_lot::Mutex<HashMap<Uuid, CancellationToken>>> =
    LazyLock::new(Default::default);

/// Record `token` as the authority of notice `notice`'s file.
pub fn authorize(notice: Uuid, token: CancellationToken) {
    AUTHORITY.lock().insert(notice, token);
}

/// The live authority of notice `notice`'s file; `None` once it ended or when unknown.
pub fn authority(notice: Uuid) -> Option<CancellationToken> {
    AUTHORITY
        .lock()
        .get(&notice)
        .filter(|t| !t.is_cancelled())
        .cloned()
}

/// `Some(live)` when this engine holds notice `notice`'s authority, `None` when not.
fn standing(notice: Uuid) -> Option<bool> {
    AUTHORITY.lock().get(&notice).map(|t| !t.is_cancelled())
}

/// Forget notice `notice`'s authority (its notice is delivered, or will never exist).
pub fn release(notice: Uuid) {
    AUTHORITY.lock().remove(&notice);
}

/// The holder's `contact` for `contact_file_notice` of notice `notice`: a foreign file's
/// binding, marked [`ENDED`] unless its authority is live here; a contact's record id.
pub fn notice_contact(contact: &str, notice: Uuid) -> String {
    match is_foreign(contact) && authority(notice).is_none() {
        true => format!("{ENDED}{contact}"),
        false => contact.to_owned(),
    }
}

/// D17's share re-check on the chat's owner of notice `notice` from `contact` (as the
/// holder sent it) into `session`: a holder's [`ENDED`] mark ends it; a foreign file whose
/// authority this engine holds needs it live (a live binding alone may reach other
/// sessions); otherwise [`Store::share_live`] (for a foreign file: its binding).
///
/// # Errors
/// The store's.
pub async fn share_live(
    store: &Store,
    session: &str,
    contact: &str,
    notice: Uuid,
) -> anyhow::Result<bool> {
    if contact.starts_with(ENDED) || (is_foreign(contact) && standing(notice) == Some(false)) {
        return Ok(false);
    }
    store.share_live(session, contact).await
}

/// BLAKE3 of `bytes`, lowercase hex.
pub fn blake3_hex(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// The design's refusal of a file too large to go inline to `peer`.
pub fn inline_too_large(peer: &str, name: &str, size: u64) -> String {
    format!(
        "{peer} takes files only inside the message, and {name} ({size} bytes) does not fit in \
         64 KiB"
    )
}

/// A foreign file URL: absolute `https`, with a host and no userinfo. Every redirect hop
/// is checked again (and the address resolved public-only) by the download itself.
///
/// # Errors
/// Why the URL is refused, for `message.parts[i]: <why>`.
pub fn https_url(url: &str) -> Result<url::Url, &'static str> {
    let u = url::Url::parse(url).map_err(|_| "the url does not parse")?;
    if u.scheme() != "https" {
        return Err("the url is not https");
    }
    if !u.username().is_empty() || u.password().is_some() {
        return Err("the url carries credentials");
    }
    if u.host().is_none() {
        return Err("the url has no host");
    }
    Ok(u)
}

/// `contact_files.contact_id` of a foreign peer's file: its binding, `peer:<uuid>` (a
/// contact's is its record id).
pub fn is_foreign(contact: &str) -> bool {
    contact.starts_with("peer:")
}

/// A foreign row's `source`: an `https` URL or inline bytes (a contact's is `iroh://`).
pub fn is_source(source: &str) -> bool {
    source.starts_with("https://") || source.starts_with(DATA)
}

/// A foreign file part: inline `raw`, or an `https` URL [`https_url`] admits.
pub fn is_file(p: &Part) -> bool {
    match &p.content {
        PartContent::Raw(_) => true,
        PartContent::Url(u) => https_url(u).is_ok(),
        _ => false,
    }
}

/// The declared name: `filename`, else the URL's last path segment, else `file`; sanitised.
fn name_of(p: &Part) -> String {
    let segment = || match &p.content {
        PartContent::Url(u) => url::Url::parse(u)
            .ok()?
            .path_segments()?
            .next_back()
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
        _ => None,
    };
    let declared = p
        .filename
        .clone()
        .filter(|n| !n.trim().is_empty())
        .or_else(segment);
    sanitise(declared.as_deref().unwrap_or("file"))
}

/// The `in` rows of `parts` for `session`'s exchange `exchange` from the foreign sender
/// `sender` (`peer:<binding>`), at `now`: one per foreign file part, in order. An inline
/// file's size is known; a URL's is not until it is fetched. The deadline is `now +
/// OFFER_TTL` (D8: there is no declared expiry).
pub fn in_rows(
    session: &str,
    exchange: &str,
    sender: &str,
    parts: &[Part],
    now: DateTime<Utc>,
) -> Vec<FileRow> {
    let deadline = now + chrono::Duration::from_std(OFFER_TTL).unwrap_or_default();
    parts
        .iter()
        .filter_map(|p| match &p.content {
            PartContent::Raw(b) => Some((
                p,
                format!(
                    "{DATA}{}",
                    base64::engine::general_purpose::STANDARD.encode(b)
                ),
                Some(b.len() as i64),
            )),
            // SEC-P5: the parsed URL's canonical form (`HTTPS://X/` is `https://x/`).
            PartContent::Url(u) => https_url(u).ok().map(|u| (p, u.to_string(), None)),
            _ => None,
        })
        .enumerate()
        .map(|(seq, (p, source, size))| {
            let name = name_of(p);
            let media_type = p
                .media_type
                .clone()
                .filter(|m| !m.trim().is_empty())
                .unwrap_or_else(|| {
                    mime_guess::from_path(&name)
                        .first_or_octet_stream()
                        .essence_str()
                        .to_owned()
                });
            FileRow {
                id: uuid::Uuid::new_v4().simple().to_string(),
                direction: "in".into(),
                session: session.to_owned(),
                exchange: exchange.to_owned(),
                seq: seq as i64,
                contact_id: sender.to_owned(),
                name,
                media_type,
                size,
                source,
                state: "queued".into(),
                created_ms: now.timestamp_millis(),
                deadline_ms: deadline.timestamp_millis(),
                ..Default::default()
            }
        })
        .collect()
}

/// Write the `in` rows of a foreign sender's (`peer:<binding>`) file parts into exchange
/// `exchange` of `session`, once, before its text is delivered. `authority` is what they
/// arrived under (SEC-P3/P4): the route's grant epoch; each file's download and notice
/// live only while it does.
///
/// # Errors
/// The store's.
pub async fn admit(
    store: &Store,
    session: runtime::SessionId,
    exchange: uuid::Uuid,
    sender: &str,
    parts: &[Part],
    authority: &CancellationToken,
) -> anyhow::Result<()> {
    let rows = in_rows(
        &session.to_string(),
        &exchange.to_string(),
        sender,
        parts,
        Utc::now(),
    );
    if admit_rows(store, &exchange.to_string(), &rows, authority).await? {
        #[cfg(unix)]
        super::service::kick();
    }
    Ok(())
}

/// Round 2: `rows` of `exchange` with their authority registered before they are visible
/// to the queue (a pass between the commit and a later registration would fail a valid
/// file); a duplicate admission or a failed write takes its registrations back. `true`
/// when the rows were written now.
async fn admit_rows(
    store: &Store,
    exchange: &str,
    rows: &[FileRow],
    authority: &CancellationToken,
) -> anyhow::Result<bool> {
    let notices: Vec<Uuid> = rows
        .iter()
        .map(|r| notice_id(store.engine(), &r.id))
        .collect();
    for n in &notices {
        authorize(*n, authority.clone());
    }
    let admitted = store.admit_in_files(exchange, rows).await;
    if !matches!(admitted, Ok(true)) {
        notices.iter().for_each(|n| release(*n));
    }
    admitted
}

/// A fetched foreign file: the partial holds exactly `bytes` bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Got {
    /// Bytes written.
    pub bytes: u64,
    /// Their BLAKE3, lowercase hex.
    pub blake3: String,
}

/// Why [`fetch`] ended without the file; the partial is gone either way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The file fails with this reason.
    Fatal(String),
    /// Back to the queue, from byte 0: `idle` (no progress for the idle bound) to its
    /// end, else after D8's backoff.
    Retry {
        /// It was the idle bound.
        idle: bool,
    },
}

/// Fetch `source` into `partial` (its directory exists; the file must not): an `https`
/// source by [`web::fetch::download`] under `opts`, a `data:` source by decoding it.
/// `machine` names this engine in a disk-full reason.
///
/// # Errors
/// [`Failure`]: refusals of the address are [`NOT_PUBLIC`].
pub async fn fetch(
    source: &str,
    partial: &Path,
    opts: &DownloadOpts,
    machine: &str,
) -> Result<Got, Failure> {
    let Some(b64) = source.strip_prefix(DATA) else {
        return match web::fetch::download(source, partial, opts).await {
            Ok(d) => Ok(Got {
                bytes: d.bytes,
                blake3: d.blake3,
            }),
            Err(e) => Err(failure(&e, machine)),
        };
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|_| Failure::Fatal("malformed file reference".into()))?;
    let written = write_new(partial, &bytes);
    if let Err(e) = written {
        let _ = std::fs::remove_file(partial);
        return Err(disk(&e, machine));
    }
    Ok(Got {
        bytes: bytes.len() as u64,
        blake3: blake3::hash(&bytes).to_hex().to_string(),
    })
}

/// `path` created exclusively (0600), `bytes` written and synced.
fn write_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut open = std::fs::OpenOptions::new();
    open.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut open, 0o600);
    let mut f = open.open(path)?;
    f.write_all(bytes)?;
    f.sync_all()
}

fn disk(e: &std::io::Error, machine: &str) -> Failure {
    #[cfg(unix)]
    if e.raw_os_error() == Some(libc::ENOSPC) {
        return Failure::Fatal(format!("disk full on {machine}"));
    }
    let _ = machine;
    Failure::Fatal(format!("saving failed: {e}"))
}

/// A download error as the file's outcome: the address refusals are [`NOT_PUBLIC`]; a
/// stall, a transport error, a 429 or a 5xx go back to the queue; its authority ending
/// (the cancellation) is [`SHARE_ENDED`], never requeued; the rest are terminal with the
/// error's own (card-sized) text.
fn failure(e: &DownloadError, machine: &str) -> Failure {
    match e {
        DownloadError::NotHttps
        | DownloadError::NotPublic
        | DownloadError::Userinfo
        | DownloadError::InvalidUrl => Failure::Fatal(NOT_PUBLIC.into()),
        DownloadError::IdleTimeout(_) => Failure::Retry { idle: true },
        DownloadError::Cancelled => Failure::Fatal(SHARE_ENDED.into()),
        DownloadError::Transport(_) => Failure::Retry { idle: false },
        DownloadError::HttpStatus(s) if *s == 429 || *s >= 500 => Failure::Retry { idle: false },
        DownloadError::Io(io) => disk(io, machine),
        other => Failure::Fatal(other.to_string()),
    }
}

/// D16 toward a peer without the files extension, before anything is staged: `files`
/// (`(name, media type, size)`) go inline as `raw` beside `text` only when the request
/// stays within [`ENVELOPE`]. Text and names are measured JSON-escaped (SEC-P1), plus
/// base64, the part keys and a 1 KiB request envelope. The client measures the final
/// encoded request again before it sends.
///
/// # Errors
/// The design's text, naming the first file that does not fit.
pub fn inline_fit<'a>(
    peer: &str,
    text: &str,
    files: impl IntoIterator<Item = (&'a str, &'a str, u64)>,
) -> Result<(), String> {
    let escaped = |s: &str| serde_json::to_string(s).map_or(usize::MAX, |j| j.len());
    let mut used = 1024usize.saturating_add(escaped(text));
    for (name, media_type, size) in files {
        let b64 = usize::try_from(size.div_ceil(3).saturating_mul(4)).unwrap_or(usize::MAX);
        used = used
            .saturating_add(b64)
            .saturating_add(escaped(name))
            .saturating_add(escaped(media_type))
            .saturating_add(64);
        if used > ENVELOPE {
            return Err(inline_too_large(peer, name, size));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(mut p: Part, name: Option<&str>, media: Option<&str>) -> Part {
        p.filename = name.map(str::to_owned);
        p.media_type = media.map(str::to_owned);
        p
    }

    /// One row per foreign file part, in order: inline bytes as a `data:` source with their
    /// size, an `https` URL with none; the name from `filename`, else the URL's last
    /// segment, sanitised; the media type declared, else guessed. `http:`, userinfo, `data`
    /// parts, text and iroh references are not foreign files.
    #[test]
    fn foreign_file_parts_become_queued_rows() {
        let now: DateTime<Utc> = "2026-11-17T00:00:00Z".parse().unwrap();
        let parts = vec![
            Part::text("hi"),
            named(
                Part::raw(vec![1, 2, 3]),
                Some("../dot.png"),
                Some("image/png"),
            ),
            Part::url("https://files.example/a/report.pdf?x=1"),
            Part::url("http://files.example/plain.txt"),
            Part::url("https://user:pw@files.example/secret"),
            Part::url(format!("iroh://abc/a2a/files/{}", "0".repeat(32))),
            Part::data(serde_json::json!({"k": 1})),
            Part::url("https://files.example/"),
        ];
        let rows = in_rows("s", "e", "peer:b", &parts, now);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].source, format!("{DATA}AQID"));
        assert_eq!(
            (rows[0].name.as_str(), rows[0].media_type.as_str()),
            ("dot.png", "image/png")
        );
        assert_eq!(rows[0].size, Some(3));
        assert_eq!(rows[1].source, "https://files.example/a/report.pdf?x=1");
        assert_eq!(
            (rows[1].name.as_str(), rows[1].media_type.as_str()),
            ("report.pdf", "application/pdf")
        );
        assert_eq!(rows[1].size, None);
        assert_eq!(rows[2].name, "file");
        assert_eq!((rows[0].seq, rows[1].seq, rows[2].seq), (0, 1, 2));
        assert!(rows.iter().all(|r| r.contact_id == "peer:b"
            && r.state == "queued"
            && r.blake3.is_none()
            && r.deadline_ms == (now + chrono::Duration::days(7)).timestamp_millis()));
        assert!(is_source(&rows[0].source) && is_source(&rows[1].source));
        assert_eq!(
            https_url("https://user@x.example/f").unwrap_err(),
            "the url carries credentials"
        );
    }

    /// The design's error names the first file the 64 KiB envelope cannot take inline.
    #[test]
    fn a_send_too_large_for_inline_names_the_file() {
        let small = ("dot.png", "image/png", 1_000u64);
        assert!(inline_fit("acme", "see", [small, small]).is_ok());
        let err = inline_fit(
            "acme",
            "see",
            [small, ("big.zip", "application/zip", 50_000), small],
        )
        .unwrap_err();
        assert_eq!(
            err,
            "acme takes files only inside the message, and big.zip (50000 bytes) does not fit \
             in 64 KiB"
        );
        assert!(inline_fit("acme", "", [("f", "x/y", 5_000_000_000)]).is_err());
    }

    /// SEC-P1: text is measured as JSON writes it. 12,000 U+0001 are 12,000 bytes raw but
    /// 72,000 escaped (`\u0001`): a one-byte file beside them does not fit.
    #[test]
    fn escaped_text_counts_toward_the_inline_envelope() {
        let control = "\u{1}".repeat(12_000);
        assert_eq!(
            inline_fit("acme", &control, [("dot.png", "image/png", 1)]).unwrap_err(),
            inline_too_large("acme", "dot.png", 1)
        );
        assert!(inline_fit("acme", &"x".repeat(12_000), [("dot.png", "image/png", 1)]).is_ok());
    }

    /// SEC-P5: an admitted `HTTPS://` URL is stored in its canonical form, so the queue
    /// dispatches it as a foreign source.
    #[test]
    fn admitted_urls_are_stored_canonical() {
        let now: DateTime<Utc> = "2026-11-17T00:00:00Z".parse().unwrap();
        let rows = in_rows(
            "s",
            "e",
            "peer:b",
            &[Part::url("HTTPS://Files.Example/a/Report.pdf")],
            now,
        );
        assert_eq!(rows[0].source, "https://files.example/a/Report.pdf");
        assert!(is_source(&rows[0].source));
        assert_eq!(rows[0].name, "Report.pdf");
    }
}

/// The queue end to end over a loopback HTTPS server (199-W's test CA and exact
/// `ip:port` allowlist, `crates/web/tests/fixtures/tls`).
#[cfg(all(test, unix))]
mod queue {
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use genai::chat::ChatMessage;
    use runtime::SessionId;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::{TcpListener, TcpStream};
    use tokio_rustls::rustls;
    use tokio_rustls::rustls::pki_types::pem::PemObject as _;
    use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};

    use super::*;
    use crate::communication::files::notices::NoticeDoor;
    use crate::communication::files::service::FileService;
    use crate::communication::files::{NoticeAdmission, NoticeStatus, received_dir};
    use crate::store::communication::AcceptExchange;

    type Conn = tokio_rustls::server::TlsStream<TcpStream>;

    const CA: &[u8] = include_bytes!("../../../../web/tests/fixtures/tls/ca.pem");
    const CERT: &[u8] = include_bytes!("../../../../web/tests/fixtures/tls/server.pem");
    const KEY: &[u8] = include_bytes!("../../../../web/tests/fixtures/tls/server.key");

    /// HTTPS on `127.0.0.1:0`; `handler(request head, connection)` answers each request.
    async fn serve<F, Fut>(handler: F) -> SocketAddr
    where
        F: Fn(String, Conn) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let config = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from_pem_slice(CERT).unwrap()],
                PrivateKeyDer::from_pem_slice(KEY).unwrap(),
            )
            .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handler = Arc::new(handler);
        tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let (acceptor, handler) = (acceptor.clone(), handler.clone());
                tokio::spawn(async move {
                    let Ok(mut tls) = acceptor.accept(tcp).await else {
                        return;
                    };
                    let mut head = Vec::new();
                    let mut b = [0u8; 1];
                    while !head.ends_with(b"\r\n\r\n") && tls.read(&mut b).await.unwrap_or(0) == 1 {
                        head.push(b[0]);
                    }
                    handler(String::from_utf8_lossy(&head).into_owned(), tls).await;
                });
            }
        });
        addr
    }

    async fn send(tls: &mut Conn, bytes: &[u8]) {
        let _ = tls.write_all(bytes).await;
        let _ = tls.flush().await;
    }

    /// The request path of a head.
    fn path(head: &str) -> &str {
        head.split_whitespace().nth(1).unwrap_or_default()
    }

    fn payload(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i % 251) as u8).collect()
    }

    /// The chat's owner, faked: every notice accepted and committed.
    #[derive(Default)]
    struct Door(parking_lot::Mutex<Vec<uuid::Uuid>>);

    #[async_trait::async_trait]
    impl NoticeDoor for Door {
        async fn notice(
            &self,
            _: SessionId,
            id: uuid::Uuid,
            _: &str,
            _: &str,
        ) -> Result<NoticeAdmission, String> {
            self.0.lock().push(id);
            Ok(NoticeAdmission::Accepted)
        }

        async fn status(&self, _: SessionId, _: uuid::Uuid) -> Result<NoticeStatus, String> {
            Ok(NoticeStatus::Committed)
        }
    }

    struct Rig {
        svc: Arc<FileService>,
        door: Arc<Door>,
        session: SessionId,
        /// `peer:<binding>` of the foreign peer `acme`.
        sender: String,
        /// The route's grant epoch the files arrive under (SEC-P3/P4).
        authority: CancellationToken,
        _dir: crate::TestDir,
    }

    /// A service whose foreign downloads trust the test CA and may reach `allow`.
    async fn rig(allow: Option<SocketAddr>, idle: Duration) -> Rig {
        let dir = crate::TestDir::new("foreign");
        let store = Store::open(&dir.0.join("sessions.sqlite"), None)
            .await
            .unwrap();
        let session = SessionId::new();
        store
            .create_session(session, "m", None, None, None)
            .await
            .unwrap();
        let binding = store
            .bind_peer("acme", "https://acme.example/")
            .await
            .unwrap();
        let door = Arc::new(Door::default());
        let mut svc = FileService::new(store, dir.0.clone(), None, door.clone());
        let s = Arc::get_mut(&mut svc).unwrap();
        s.backoff = Duration::from_millis(20);
        s.idle = idle;
        s.web.test_allow = allow.into_iter().collect();
        s.web.test_root_pem = Some(CA.to_vec());
        Rig {
            svc,
            door,
            session,
            sender: format!("peer:{}", binding.id),
            authority: CancellationToken::new(),
            _dir: dir,
        }
    }

    impl Rig {
        /// A consumed message from `acme` carrying `parts`: its file rows admitted.
        async fn message(&self, parts: &[Part]) -> Vec<FileRow> {
            let ex = self.accepted(parts).await;
            let store = &self.svc.store;
            store
                .consume_exchange(ex, self.session, 1, ChatMessage::user("see attached"))
                .await
                .unwrap()
                .unwrap();
            store.exchange_files(&ex.to_string()).await.unwrap()
        }

        /// A message from `acme` carrying `parts`, accepted (not yet read): its exchange.
        async fn accepted(&self, parts: &[Part]) -> uuid::Uuid {
            let store = &self.svc.store;
            let ex = store
                .accept_exchange(&AcceptExchange {
                    sender: &self.sender,
                    recipient: &format!("session:{}", self.session),
                    sender_session: None,
                    recipient_session: Some(self.session),
                    request_id: None,
                    reply_to: None,
                    request_scope: None,
                    generation: 1,
                    body: "see attached",
                })
                .await
                .unwrap()
                .id;
            admit(
                store,
                self.session,
                ex,
                &self.sender,
                parts,
                &self.authority,
            )
            .await
            .unwrap();
            ex
        }

        async fn row(&self, id: &str) -> FileRow {
            self.svc.store.contact_file(id).await.unwrap().unwrap()
        }

        /// Passes until `id` is terminal with its notice delivered (at most `secs`).
        async fn settled(&self, id: &str, secs: u64) -> FileRow {
            let until = Instant::now() + Duration::from_secs(secs);
            while Instant::now() < until {
                self.svc.tick().await.unwrap();
                let row = self.row(id).await;
                if row.terminal() && row.notice_state.as_deref() == Some("delivered") {
                    return row;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            panic!("{id} never settled: {:?}", self.row(id).await);
        }
    }

    /// An inline `raw` part is published like a contact's file: its BLAKE3 computed here,
    /// 0600 under the peer's alias, one received notice naming it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_raw_part_is_saved_with_its_computed_blake3_and_notified() {
        let rig = rig(None, Duration::from_secs(5)).await;
        let bytes = payload(3000);
        let rows = rig
            .message(&[
                Part::text("see attached"),
                Part::raw(bytes.clone()).with_filename("dot.png"),
            ])
            .await;
        assert_eq!(rows.len(), 1);
        let row = rig.settled(&rows[0].id, 5).await;
        assert_eq!(row.state, "received");
        let digest = blake3::hash(&bytes).to_hex().to_string();
        assert_eq!(row.blake3.as_deref(), Some(digest.as_str()));
        assert_eq!(row.size, Some(3000));
        let saved = std::path::PathBuf::from(row.path.unwrap());
        assert_eq!(
            saved,
            received_dir(&rig.svc.root, "acme", chrono::Local::now().date_naive()).join("dot.png")
        );
        assert_eq!(std::fs::read(&saved).unwrap(), bytes);
        use std::os::unix::fs::MetadataExt as _;
        assert_eq!(std::fs::metadata(&saved).unwrap().mode() & 0o777, 0o600);
        let notice = row.notice.unwrap();
        assert!(
            notice.starts_with("<message_file from=\"peer:acme\""),
            "{notice}"
        );
        assert!(notice.contains(&format!("blake3=\"{digest}\"")), "{notice}");
        assert!(notice.contains("state=\"received\""), "{notice}");
        assert_eq!(rig.door.0.lock().len(), 1);
    }

    /// A public file with no `Content-Length`, dripped for longer than the old fetcher's 8 s
    /// whole-request timeout (and many idle budgets), arrives whole with its BLAKE3.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_slow_https_file_without_length_arrives_after_the_old_timeout() {
        const CHUNKS: usize = 18;
        let addr = serve(|_, mut tls| async move {
            send(
                &mut tls,
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
            )
            .await;
            for i in 0..CHUNKS {
                tokio::time::sleep(Duration::from_millis(500)).await;
                send(&mut tls, format!("4\r\n{i:04}\r\n").as_bytes()).await;
            }
            send(&mut tls, b"0\r\n\r\n").await;
            let _ = tls.shutdown().await;
        })
        .await;
        let rig = rig(Some(addr), Duration::from_secs(2)).await;
        let rows = rig
            .message(&[Part::url(format!("https://{addr}/survey.bin"))])
            .await;
        assert_eq!((rows[0].size, rows[0].name.as_str()), (None, "survey.bin"));
        let started = Instant::now();
        let row = rig.settled(&rows[0].id, 30).await;
        assert!(
            started.elapsed() > Duration::from_secs(8),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(row.state, "received", "{:?}", row.error);
        let want: String = (0..CHUNKS).map(|i| format!("{i:04}")).collect();
        assert_eq!(row.size, Some(want.len() as i64));
        assert_eq!(
            row.blake3.as_deref(),
            Some(blake3::hash(want.as_bytes()).to_hex().as_str())
        );
        assert_eq!(std::fs::read(row.path.unwrap()).unwrap(), want.as_bytes());
    }

    /// A download cut mid-body goes back to the queue and restarts from byte 0: no `Range`,
    /// the whole file the second time.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_cut_download_restarts_from_zero() {
        let body = payload(10_000);
        let heads = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let calls = Arc::new(AtomicUsize::new(0));
        let addr = {
            let (body, heads, calls) = (body.clone(), heads.clone(), calls.clone());
            serve(move |head, mut tls| {
                let (body, heads, calls) = (body.clone(), heads.clone(), calls.clone());
                async move {
                    heads.lock().push(head.to_ascii_lowercase());
                    let first = calls.fetch_add(1, Ordering::SeqCst) == 0;
                    let h = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    send(&mut tls, h.as_bytes()).await;
                    let n = if first { 4000 } else { body.len() };
                    send(&mut tls, &body[..n]).await;
                    let _ = tls.shutdown().await;
                }
            })
            .await
        };
        let rig = rig(Some(addr), Duration::from_secs(5)).await;
        let rows = rig
            .message(&[Part::url(format!("https://{addr}/big.bin"))])
            .await;
        let row = rig.settled(&rows[0].id, 10).await;
        assert_eq!(row.state, "received", "{:?}", row.error);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(
            heads.lock().iter().all(|h| !h.contains("\r\nrange:")),
            "{heads:?}"
        );
        assert_eq!(std::fs::read(row.path.unwrap()).unwrap(), body);
    }

    /// Refusals: what the public-only guard refuses (loopback, RFC 1918, CGNAT, the metadata
    /// address, directly or by a redirect, and a redirect to `http:`) is "not a public HTTPS
    /// address"; over 5 GB by header or by stream fails; a stall and a 5xx go back to the
    /// queue; a 404 fails. Nothing is left at the partial. Through the queue, a refused
    /// address fails the file with a failed notice.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn refusals_fail_or_requeue_with_their_reason() {
        let addr = serve(|head: String, mut tls: Conn| async move {
            let reply = |status: &str, extra: &str| {
                format!(
                    "HTTP/1.1 {status}\r\n{extra}Content-Length: 0\r\nConnection: close\r\n\r\n"
                )
            };
            let out = match path(&head) {
                "/to-http" => reply("302 Found", "Location: http://example.com/f\r\n"),
                "/to-private" => reply("302 Found", "Location: https://10.0.0.1/f\r\n"),
                "/to-metadata" => {
                    reply("302 Found", "Location: https://169.254.169.254/latest\r\n")
                }
                "/missing" => reply("404 Not Found", ""),
                "/busy" => reply("503 Service Unavailable", ""),
                "/huge" => "HTTP/1.1 200 OK\r\nContent-Length: 5000000001\r\n\r\n".to_owned(),
                "/stream" => {
                    send(
                        &mut tls,
                        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n",
                    )
                    .await;
                    for _ in 0..4 {
                        send(&mut tls, b"8\r\n01234567\r\n").await;
                    }
                    std::future::pending::<()>().await;
                    unreachable!()
                }
                _ => {
                    // "/hang": never a header.
                    std::future::pending::<()>().await;
                    unreachable!()
                }
            };
            send(&mut tls, out.as_bytes()).await;
            if path(&head) == "/huge" {
                std::future::pending::<()>().await;
            }
            let _ = tls.shutdown().await;
        })
        .await;
        let dir = crate::TestDir::new("foreign-refusals");
        let partial = dir.0.join("partial");
        let public = DownloadOpts::default();
        let local = DownloadOpts {
            test_allow: vec![addr],
            test_root_pem: Some(CA.to_vec()),
            idle_timeout: Duration::from_millis(300),
            ..DownloadOpts::default()
        };
        let not_public = Err(Failure::Fatal(NOT_PUBLIC.into()));
        for url in [
            "https://127.0.0.1/f",
            "https://10.0.0.1/f",
            "https://100.64.0.1/f",
            "https://169.254.169.254/latest",
        ] {
            assert_eq!(
                fetch(url, &partial, &public, "m").await,
                not_public,
                "{url}"
            );
            assert!(!partial.exists());
        }
        let small = DownloadOpts {
            max_bytes: 10,
            ..local.clone()
        };
        let too_large = Err(Failure::Fatal("too large: over 5 GB".into()));
        let cases: [(&str, &DownloadOpts, Result<Got, Failure>); 8] = [
            ("/to-http", &local, not_public.clone()),
            ("/to-private", &local, not_public.clone()),
            ("/to-metadata", &local, not_public.clone()),
            ("/huge", &local, too_large),
            (
                "/stream",
                &small,
                Err(Failure::Fatal("too large: over 10 bytes".into())),
            ),
            ("/hang", &local, Err(Failure::Retry { idle: true })),
            ("/busy", &local, Err(Failure::Retry { idle: false })),
            (
                "/missing",
                &local,
                Err(Failure::Fatal("failed: HTTP 404".into())),
            ),
        ];
        for (p, opts, want) in cases {
            let url = format!("https://{addr}{p}");
            assert_eq!(fetch(&url, &partial, opts, "m").await, want, "{p}");
            assert!(!partial.exists(), "{p}");
        }

        let rig = rig(None, Duration::from_secs(5)).await;
        let rows = rig.message(&[Part::url("https://127.0.0.1:1/x.pdf")]).await;
        let row = rig.settled(&rows[0].id, 5).await;
        assert_eq!(row.state, "failed");
        assert_eq!(row.error.as_deref(), Some(NOT_PUBLIC));
        let notice = row.notice.unwrap();
        assert!(notice.contains("state=\"failed\""), "{notice}");
        assert!(
            notice.contains(&format!("reason=\"{NOT_PUBLIC}\"")),
            "{notice}"
        );
    }

    /// SEC-P4: the authority a file arrived under bounds its download. Ended before the
    /// queue starts it: it fails `share ended` and nothing is requested; ended mid-transfer
    /// (a server still dripping bytes): the download stops at once and fails the same way,
    /// never requeued; a file whose authority is unknown (after a restart) fails too.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_ended_authority_stops_queued_and_active_downloads() {
        let calls = Arc::new(AtomicUsize::new(0));
        let addr = {
            let calls = calls.clone();
            serve(move |_, mut tls| {
                let calls = calls.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    send(
                        &mut tls,
                        b"HTTP/1.1 200 OK\r\nContent-Length: 100000\r\nConnection: close\r\n\r\n",
                    )
                    .await;
                    for _ in 0..1000 {
                        send(&mut tls, &[b'x'; 100]).await;
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }
            })
            .await
        };
        let url = format!("https://{addr}/drip.bin");

        // Ended before it starts.
        let early = rig(Some(addr), Duration::from_secs(5)).await;
        early.authority.cancel();
        let rows = early.message(&[Part::url(url.clone())]).await;
        let row = early.settled(&rows[0].id, 5).await;
        assert_eq!(
            (row.state.as_str(), row.error.as_deref()),
            ("failed", Some(SHARE_ENDED))
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0, "nothing requested");

        // Ended mid-transfer.
        let active = rig(Some(addr), Duration::from_secs(5)).await;
        let rows = active.message(&[Part::url(url.clone())]).await;
        let until = Instant::now() + Duration::from_secs(5);
        while calls.load(Ordering::SeqCst) == 0 {
            assert!(Instant::now() < until, "the download never started");
            active.svc.tick().await.unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        let cut = Instant::now();
        active.authority.cancel();
        let row = active.settled(&rows[0].id, 5).await;
        assert!(
            cut.elapsed() < Duration::from_secs(3),
            "{:?}",
            cut.elapsed()
        );
        assert_eq!(
            (row.state.as_str(), row.error.as_deref()),
            ("failed", Some(SHARE_ENDED))
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1, "never requeued");
        assert!(!active.svc.partial(&row.id).exists());

        // Unknown authority (a restart forgets it).
        let restarted = rig(None, Duration::from_secs(5)).await;
        let rows = restarted.message(&[Part::raw(b"tiny".to_vec())]).await;
        release(notice_id(restarted.svc.store.engine(), &rows[0].id));
        let row = restarted.settled(&rows[0].id, 5).await;
        assert_eq!(row.error.as_deref(), Some(SHARE_ENDED));
    }

    /// Round 2: a file's authority exists before its row is visible, so a queue pass racing
    /// the admission never fails a valid file `share ended`; a duplicate admission of the
    /// same exchange registers nothing that stays.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn admission_never_races_the_queue_and_duplicates_leave_nothing() {
        let rig = Arc::new(rig(None, Duration::from_secs(5)).await);
        let stop = CancellationToken::new();
        let ticking = {
            let (rig, stop) = (rig.clone(), stop.clone());
            tokio::spawn(async move {
                while !stop.is_cancelled() {
                    let _ = rig.svc.tick().await;
                    tokio::task::yield_now().await;
                }
            })
        };
        let mut ids = Vec::new();
        for n in 0..20u8 {
            let rows = rig.message(&[Part::raw(vec![n; 16])]).await;
            ids.push(rows[0].id.clone());
        }
        for id in &ids {
            let row = rig.settled(id, 10).await;
            assert_eq!(row.state, "received", "{id}: {:?}", row.error);
        }
        stop.cancel();
        ticking.await.unwrap();

        let ex = rig.accepted(&[Part::raw(b"once".to_vec())]).await;
        let again = in_rows(
            &rig.session.to_string(),
            &ex.to_string(),
            &rig.sender,
            &[Part::raw(b"again".to_vec())],
            Utc::now(),
        );
        let store = &rig.svc.store;
        assert!(
            !admit_rows(store, &ex.to_string(), &again, &rig.authority)
                .await
                .unwrap()
        );
        let unused = notice_id(store.engine(), &again[0].id);
        assert!(
            !AUTHORITY.lock().contains_key(&unused),
            "a duplicate's registration went"
        );
        assert_eq!(
            rig.svc
                .store
                .exchange_files(&ex.to_string())
                .await
                .unwrap()
                .len(),
            1
        );
    }

    /// Round 2 (SHOULD): an exchange that ends without notices (here `HOLD_MOVED` before its
    /// text was read) releases its files' authority.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_exchange_ending_without_notices_releases_its_authority() {
        let rig = rig(None, Duration::from_secs(5)).await;
        let ex = rig.accepted(&[Part::raw(b"tiny".to_vec())]).await;
        let id = rig.svc.store.exchange_files(&ex.to_string()).await.unwrap()[0]
            .id
            .clone();
        let notice = notice_id(rig.svc.store.engine(), &id);
        assert!(AUTHORITY.lock().contains_key(&notice));
        rig.svc
            .store
            .fail_exchange(ex, crate::communication::network::HOLD_MOVED)
            .await
            .unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        while rig.row(&id).await.state != "failed" {
            assert!(Instant::now() < until, "never settled");
            rig.svc.tick().await.unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(!AUTHORITY.lock().contains_key(&notice), "released");
        assert!(rig.row(&id).await.notice.is_none());
    }
}
