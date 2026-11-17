//! 199 sending (D6, D9): every file of a `send_message` (or a human answer, D15) passes
//! the egress gate once, is cloned into `files/outgoing/` when the volume can (macOS
//! `fclonefileat`) and hashed with BLAKE3 from the clone or the gate's handle, then offered
//! once per contact recipient. The offers are staged under the message's request id
//! ([`staged_tag`]) and become the exchange's when `Store::exchange_record` reads it for
//! the outbound message, so the a2a client sees them as `PeerRecord.files`.

use std::os::unix::fs::DirBuilderExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Utc;
use kameo::actor::ActorRef;
use runtime::SessionId;
use tools::ToolOutput;

use super::service::{FileService, kick};
use super::{FILE_MAX, NOT_YET, OFFER_TTL, OUTGOING_DIR, disk, sanitise};
use crate::communication::PeerReceipt;
use crate::store::contact_files::{FileRow, staged_tag};
use crate::supervisor::Agents;

/// The serialized `SendMessage` request bound (`a2a::server::BODY_LIMIT`).
const ENVELOPE: usize = 64 * 1024;

/// One file the gate let out.
#[derive(Debug)]
struct Picked {
    path: PathBuf,
    name: String,
    media_type: String,
    size: u64,
    blake3: String,
    dev: u64,
    inode: u64,
    /// `files/outgoing/<staging id>`: the clone every offer of it is cloned from.
    clone: Option<PathBuf>,
}

/// A random local id: 32 lowercase hex.
pub fn local_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// A copy-on-write clone of `src` at `dst` (macOS `fclonefileat`, 0600); `false` when the
/// volume (or the platform) cannot clone: the original is served, re-checked (D6).
fn clone_into(src: &std::fs::File, dst: &Path) -> bool {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt as _;
        use std::os::unix::fs::PermissionsExt as _;
        use std::os::unix::io::AsRawFd as _;
        let Ok(c) = std::ffi::CString::new(dst.as_os_str().as_bytes()) else {
            return false;
        };
        // SAFETY: a valid open descriptor and a NUL-terminated path.
        if unsafe { libc::fclonefileat(src.as_raw_fd(), libc::AT_FDCWD, c.as_ptr(), 0) } != 0 {
            return false;
        }
        if std::fs::set_permissions(dst, std::fs::Permissions::from_mode(0o600)).is_err() {
            let _ = std::fs::remove_file(dst);
            return false;
        }
        true
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (src, dst);
        false
    }
}

/// D6 + D9 for one path: gate, clone, hash (blocking).
fn pick(path: &Path, root: &Path) -> Result<Picked, String> {
    let e = disk::egress_open(path, root).map_err(|e| e.to_string())?;
    let outgoing = root.join(OUTGOING_DIR);
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&outgoing)
        .map_err(|err| format!("files: {}: {err}", path.display()))?;
    let staging = outgoing.join(local_id());
    let clone = clone_into(&e.file, &staging).then_some(staging);
    // Hash what is served: the clone when there is one (immutable, ours), else the handle.
    let hashed = match &clone {
        Some(c) => std::fs::File::open(c).and_then(|f| disk::blake3_of(&f, e.size)),
        None => disk::blake3_of(&e.file, e.size),
    };
    let blake3 = hashed.map_err(|err| {
        if let Some(c) = &clone {
            let _ = std::fs::remove_file(c);
        }
        format!("files: {}: {err}", path.display())
    })?;
    let declared = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(Picked {
        path: path.to_path_buf(),
        name: sanitise(&declared),
        media_type: mime_guess::from_path(path)
            .first_or_octet_stream()
            .essence_str()
            .to_owned(),
        size: e.size,
        blake3,
        dev: e.dev,
        inode: e.inode,
        clone,
    })
}

/// What the message's JSON will weigh, generously: the text and one reference part per
/// file (URL, name, type and the extension's metadata).
fn envelope(text: &str, files: &[Picked]) -> usize {
    let parts: usize = files
        .iter()
        .map(|f| 420 + 2 * f.name.len() + f.media_type.len())
        .sum();
    1024 + text.len() + text.len() / 4 + parts
}

/// Offers staged for one `send_message` with files.
pub struct Staged {
    store: crate::store::Store,
    session: SessionId,
    /// Each recipient's request id (the message's, `#<n>` per recipient of several).
    pub request_ids: Vec<String>,
    /// Each recipient's offers, in `files` order.
    offers: Vec<Vec<FileRow>>,
}

/// Gate, clone, hash and stage `paths` for each of `to` (canonical addresses, every one
/// a contact). `request_id` is the caller's key; a repeat of it must carry the same
/// `(blake3, size, name)` list. Nothing is sent when this fails.
pub async fn stage(
    agents: &ActorRef<Agents>,
    session: SessionId,
    to: &[String],
    paths: &[PathBuf],
    request_id: Option<&str>,
    text: &str,
) -> Result<Staged, String> {
    let svc: Arc<FileService> = agents
        .ask(super::service::FileServiceOf)
        .await
        .ok()
        .flatten()
        .ok_or(NOT_YET)?;
    let mut contacts = Vec::with_capacity(to.len());
    for c in to {
        let alias = c.strip_prefix("peer:").unwrap_or_default();
        let contact = !alias.is_empty()
            && agents
                .ask(crate::communication::network::IsContact(alias.to_owned()))
                .await
                .unwrap_or(false);
        let record = match contact {
            true => svc
                .store
                .contact_of_alias(alias.split('/').next().unwrap_or(alias))
                .await
                .map_err(|e| format!("{e:#}"))?,
            false => None,
        };
        let Some((record, _)) = record else {
            return Err(format!(
                "files can go to contacts only; {c} is not a contact"
            ));
        };
        contacts.push(record);
    }
    let root = svc.root.clone();
    let list = paths.to_vec();
    let picked = tokio::task::spawn_blocking(move || {
        let mut out: Vec<Picked> = Vec::with_capacity(list.len());
        for p in &list {
            match pick(p, &root) {
                Ok(f) => out.push(f),
                Err(e) => {
                    for f in &out {
                        if let Some(c) = &f.clone {
                            let _ = std::fs::remove_file(c);
                        }
                    }
                    return Err(e);
                }
            }
        }
        Ok(out)
    })
    .await
    .map_err(|e| format!("files: {e}"))??;
    let staging: Vec<PathBuf> = picked.iter().filter_map(|f| f.clone.clone()).collect();
    let drop_staging = || {
        for c in &staging {
            let _ = std::fs::remove_file(c);
        }
    };
    if envelope(text, &picked) > ENVELOPE {
        drop_staging();
        return Err(
            "too many files for one message (over 64 KiB); send them in several messages".into(),
        );
    }
    debug_assert!(picked.iter().all(|f| f.size <= FILE_MAX));
    let base = request_id.map_or_else(local_id, str::to_owned);
    let request_ids: Vec<String> = match to.len() {
        1 => vec![base.clone()],
        n => (0..n).map(|i| format!("{base}#{i}")).collect(),
    };
    let sender = format!("session:{session}");
    let now = Utc::now().timestamp_millis();
    let deadline = now + OFFER_TTL.as_millis() as i64;
    let mut requests = Vec::with_capacity(to.len());
    for (rid, contact) in request_ids.iter().zip(&contacts) {
        let tag = staged_tag(&session.to_string(), rid);
        let mut mine = Vec::with_capacity(picked.len());
        for (seq, f) in picked.iter().enumerate() {
            let id = local_id();
            let clone = f.clone.as_ref().and_then(|c| {
                let dst = svc.root.join(OUTGOING_DIR).join(&id);
                let src = std::fs::File::open(c).ok()?;
                clone_into(&src, &dst).then_some(dst)
            });
            mine.push(FileRow {
                id,
                direction: "out".into(),
                session: session.to_string(),
                exchange: tag.clone(),
                seq: seq as i64,
                contact_id: contact.clone(),
                name: f.name.clone(),
                media_type: f.media_type.clone(),
                size: Some(f.size as i64),
                blake3: Some(f.blake3.clone()),
                source: f.path.to_string_lossy().into_owned(),
                clone: clone.map(|c| c.to_string_lossy().into_owned()),
                dev: Some(f.dev as i64),
                inode: Some(f.inode as i64),
                state: "queued".into(),
                created_ms: now,
                deadline_ms: deadline,
                ..Default::default()
            });
        }
        requests.push((rid.clone(), tag, mine));
    }
    drop_staging();
    // SEC-A3: the request id and its files are reserved together, atomically: a repeat (or
    // a concurrent retry) of the key with the same files reuses the offers already staged
    // or adopted, with other files nothing is sent.
    let reserved = svc.store.reserve_offers(&sender, &requests).await;
    let ours: std::collections::HashSet<String> = match &reserved {
        Ok(Some(offers)) => offers.iter().flatten().map(|r| r.id.clone()).collect(),
        _ => Default::default(),
    };
    for r in requests.iter().flat_map(|(_, _, rows)| rows) {
        if !ours.contains(&r.id)
            && let Some(c) = &r.clone
        {
            let _ = std::fs::remove_file(c);
        }
    }
    let offers = match reserved {
        Ok(Some(offers)) => offers,
        Ok(None) => {
            return Err(format!(
                "request_id {} was used with different files",
                request_id.unwrap_or_default()
            ));
        }
        Err(e) => return Err(format!("files: {e:#}")),
    };
    Ok(Staged {
        store: svc.store.clone(),
        session,
        request_ids,
        offers,
    })
}

impl Staged {
    /// After every `Deliver`: offers no exchange adopted (the send was refused before it
    /// was accepted) go, with their clones; the service settles the rest.
    pub async fn finish(&self) {
        for rid in &self.request_ids {
            match self
                .store
                .drop_staged_offers(&staged_tag(&self.session.to_string(), rid))
                .await
            {
                Ok(rows) => {
                    for r in rows {
                        if let Some(c) = &r.clone {
                            let _ = std::fs::remove_file(c);
                        }
                    }
                }
                Err(e) => tracing::warn!("199: staged offers: {e:#}"),
            }
        }
        kick();
    }

    /// The tool's answer gains `structured.files` and one line naming the offers.
    pub fn annotate(&self, receipts: &[PeerReceipt], out: &mut ToolOutput) {
        let holder = self.store.engine().to_owned();
        let mut listed = Vec::new();
        let mut names = Vec::new();
        let mut deadline = None;
        for (r, offers) in receipts.iter().zip(&self.offers) {
            if r.state == crate::communication::PeerMessageState::Failed {
                continue;
            }
            for f in offers {
                listed.push(serde_json::json!({
                    "id": f.id,
                    "holder": holder,
                    "to": r.to,
                    "name": f.name,
                    "size": f.size,
                    "media_type": f.media_type,
                    "blake3": f.blake3,
                }));
                names.push(format!("{} {}", f.name, f.size.unwrap_or_default()));
                deadline = deadline.max(Some(f.deadline_ms));
            }
        }
        if let Some(serde_json::Value::Object(m)) = out.structured.as_mut() {
            m.insert("files".into(), serde_json::Value::Array(listed));
        }
        if let Some(d) = deadline {
            let when = crate::store::contact_files::at_ms(d).format("%Y-%m-%d %H:%M UTC");
            out.content.push_str(&format!(
                "\n{} files offered ({}); offers expire {when}",
                names.len(),
                names.join(", ")
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every refusal comes before anything is staged: theuth's own secret state by
    /// symlink, hardlink and (on a case-insensitive volume) case variant; a FIFO; a
    /// 5,000,000,001-byte sparse file; a relative path. A plain file (a received store
    /// too) is cloned where the volume can and hashed once.
    #[test]
    fn the_gate_refuses_before_anything_is_staged() {
        let d = crate::TestDir::new("send");
        let root = d.0.join("root");
        std::fs::create_dir_all(root.join("mesh")).unwrap();
        let identity = root.join("mesh/identity");
        std::fs::write(&identity, b"secret").unwrap();
        let link = d.0.join("link");
        std::os::unix::fs::symlink(&identity, &link).unwrap();
        let hard = d.0.join("hard");
        std::fs::hard_link(&identity, &hard).unwrap();
        let fifo = d.0.join("fifo");
        {
            use std::os::unix::ffi::OsStrExt as _;
            let c = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
            // SAFETY: a valid NUL-terminated path.
            assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        }
        let big = d.0.join("big.bin");
        std::fs::File::create(&big)
            .unwrap()
            .set_len(FILE_MAX + 1)
            .unwrap();
        let err = |p: &Path| pick(p, &root).unwrap_err();
        for p in [&identity, &link, &hard] {
            assert_eq!(
                err(p),
                format!(
                    "files: {} is theuth's own secret state and is never sent",
                    p.display()
                )
            );
        }
        let upper = root.join("MESH/IDENTITY");
        if upper.exists() {
            assert!(err(&upper).contains("theuth's own secret state"));
        }
        assert_eq!(
            err(&fifo),
            format!("files: {}: not a regular file", fifo.display())
        );
        assert_eq!(
            err(&big),
            format!(
                "files: {} is 5000000001 bytes (over the 5 GB limit per file)",
                big.display()
            )
        );
        assert_eq!(
            err(Path::new("rel.txt")),
            "files: rel.txt is not an absolute path"
        );
        let outgoing = root.join(OUTGOING_DIR);
        let staged = || std::fs::read_dir(&outgoing).map_or(0, |d| d.count());
        assert_eq!(staged(), 0, "nothing staged for a refusal");
        let ok = d.0.join("research.sqlite");
        std::fs::write(&ok, b"a received store").unwrap();
        let p = pick(&ok, &root).unwrap();
        assert_eq!((p.name.as_str(), p.size), ("research.sqlite", 16));
        assert_eq!(
            p.blake3,
            blake3::hash(b"a received store").to_hex().to_string()
        );
        if let Some(c) = &p.clone {
            assert_eq!(std::fs::read(c).unwrap(), b"a received store");
        }
    }

    /// The envelope estimate: a handful of files fits, a few hundred do not.
    #[test]
    fn many_files_overflow_the_envelope() {
        let f = |n: usize| Picked {
            path: PathBuf::from(format!("/x/{n}")),
            name: format!("file-{n:04}.pdf"),
            media_type: "application/pdf".into(),
            size: 1,
            blake3: "0".repeat(64),
            dev: 0,
            inode: 0,
            clone: None,
        };
        assert!(envelope("hi", &(0..10).map(f).collect::<Vec<_>>()) < ENVELOPE);
        assert!(envelope("hi", &(0..200).map(f).collect::<Vec<_>>()) > ENVELOPE);
    }
}
