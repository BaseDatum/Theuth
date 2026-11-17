//! 199 P1, this engine's side of every file between contacts: the sender's offers (the
//! [`FileOffers`] the a2a route serves through), the receiver's durable queue (D7: at most
//! two downloads per sending member, none engine-wide; an idle one back to the end of the
//! queue, transport failures on D8's backoff), publication by inode with the quarantine
//! first, the States rules, terminal receipts, the holder's notice outbox (D17) and the
//! startup recovery. One task drives it ([`FileService::run`]): a pass on every
//! [`kick`] and once a second.

use std::collections::{HashMap, HashSet};
use std::io;
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use runtime::{Runtime, SessionId};
use tokio::sync::Notify;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::holder::render;
use super::intake::source_of;
use super::notices::NoticeDoor;
use super::{
    FileFetch, FileOffers, Gone, NoticeStatus, OUTGOING_DIR, OfferError, OfferHandle, PARTIAL_DIR,
    REASON_MAX, Receipt, Timer, TransferError, disk, notice_id, received_dir,
};
use crate::AgentEvent;
use crate::store::Store;
use crate::store::contact_files::{FileRow, Render, Settled};

/// One pass at least this often.
const TICK: Duration = Duration::from_secs(1);
/// D8: the first retry delay; it doubles up to [`BACKOFF_MAX`].
const BACKOFF: Duration = Duration::from_secs(5);
/// D8: the longest retry delay.
const BACKOFF_MAX: Duration = Duration::from_secs(600);
/// D7: concurrent downloads per sending member.
const PER_MEMBER: usize = 2;
/// At most two progress events a second per file.
const PROGRESS_EVERY: Duration = Duration::from_millis(500);

static KICK: Notify = Notify::const_new();

/// Ask the running service for a pass now (new rows, a consumed exchange …).
pub fn kick() {
    KICK.notify_one();
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// `ms` since the epoch as a tokio instant (a far deadline is capped at twice the TTL).
fn instant_at(ms: i64) -> Instant {
    let cap = 2 * super::OFFER_TTL.as_millis() as i64;
    let left = ms.saturating_sub(now_ms()).clamp(0, cap);
    Instant::now() + Duration::from_millis(left as u64)
}

/// At most [`REASON_MAX`] bytes, cut at a character boundary.
fn reason(s: &str) -> String {
    let mut n = s.len().min(REASON_MAX);
    while !s.is_char_boundary(n) {
        n -= 1;
    }
    s[..n].to_owned()
}

/// `dir` and its missing parents, 0700.
fn mkdirs(dir: &Path) -> io::Result<()> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

fn remove(p: &Path) {
    if let Err(e) = std::fs::remove_file(p)
        && e.kind() != io::ErrorKind::NotFound
    {
        tracing::warn!(path = %p.display(), "199: remove: {e}");
    }
}

/// The failure text of a download error, `None` when it goes back to the queue.
fn fatal(e: &TransferError, machine: &str) -> Option<String> {
    match e {
        TransferError::Idle | TransferError::Transport(_) | TransferError::NotYet => None,
        TransferError::Disk(e) if e.raw_os_error() == Some(libc::ENOSPC) => {
            Some(format!("disk full on {machine}"))
        }
        other => Some(other.to_string()),
    }
}

/// In-memory scheduling state; never held across an await.
#[derive(Default)]
struct Sched {
    /// `in` rows a transfer or publication holds.
    running: HashSet<String>,
    /// Active downloads per sending member.
    members: HashMap<String, usize>,
    /// Not before, and the last delay: a row (`<id>`) or its notice (`n:<id>`).
    wait: HashMap<String, (Instant, Duration)>,
    /// Idle requeues go to the back of the queue: their order key.
    requeued: HashMap<String, u64>,
    turn: u64,
    /// Outbox rows in flight.
    notices: HashSet<String>,
    /// Receipt POSTs in flight.
    receipts: HashSet<String>,
}

impl Sched {
    fn due(&self, key: &str, now: Instant) -> bool {
        self.wait.get(key).is_none_or(|(at, _)| *at <= now)
    }

    /// Back off `key` once more (D8: 5 s doubling to 10 min).
    fn back_off(&mut self, key: &str, base: Duration) {
        let next = self
            .wait
            .get(key)
            .map_or(base, |(_, d)| (*d * 2).min(BACKOFF_MAX.max(base)));
        self.wait
            .insert(key.to_owned(), (Instant::now() + next, next));
    }
}

/// The file service of one engine.
pub struct FileService {
    /// `sessions.sqlite`.
    pub store: Store,
    /// `<root>` (D4): this engine's data dir.
    pub root: PathBuf,
    rt: Option<Runtime>,
    /// D8's idle bound ([`super::FILE_IDLE`]; shorter in tests).
    pub idle: Duration,
    /// D8's first backoff ([`BACKOFF`]; shorter in tests).
    pub backoff: Duration,
    /// A fetcher in place of the registered one (tests).
    pub fetcher: Option<Arc<dyn FileFetch>>,
    door: Arc<dyn NoticeDoor>,
    sched: parking_lot::Mutex<Sched>,
}

static SERVICE: parking_lot::RwLock<Option<Arc<FileService>>> = parking_lot::const_rwlock(None);

/// The running service (the send path and the RPCs reach it here).
pub fn service() -> Option<Arc<FileService>> {
    SERVICE.read().clone()
}

impl FileService {
    /// A service over `store` with `root` its data dir; `rt` publishes the events.
    pub fn new(
        store: Store,
        root: PathBuf,
        rt: Option<Runtime>,
        door: Arc<dyn NoticeDoor>,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            root,
            rt,
            idle: super::FILE_IDLE,
            backoff: BACKOFF,
            fetcher: None,
            door,
            sched: parking_lot::Mutex::default(),
        })
    }

    /// Register as this engine's offers and service, then drive the queue until shutdown.
    pub fn start(self: &Arc<Self>, shutdown: CancellationToken) {
        super::set_offers(self.clone());
        *SERVICE.write() = Some(self.clone());
        let me = self.clone();
        tokio::spawn(async move {
            me.resend_receipts().await;
            loop {
                if let Err(e) = me.tick().await {
                    tracing::warn!("199: file pass: {e:#}");
                }
                tokio::select! {
                    () = shutdown.cancelled() => return,
                    () = KICK.notified() => {}
                    () = tokio::time::sleep(TICK) => {}
                }
            }
        });
    }

    fn fetcher(&self) -> Arc<dyn FileFetch> {
        self.fetcher.clone().unwrap_or_else(super::fetch)
    }

    /// The partial of local id `id`.
    pub fn partial(&self, id: &str) -> PathBuf {
        self.root.join(PARTIAL_DIR).join(id)
    }

    /// This engine as cards name it.
    pub async fn render(&self) -> Render {
        render(&self.store).await
    }

    /// `ContactFileChanged` for `row`.
    pub async fn changed(&self, r: &Render, row: &FileRow) {
        let Some(rt) = &self.rt else { return };
        let Ok(session) = row.session.parse::<SessionId>() else {
            return;
        };
        let alias = self
            .store
            .contact_alias(&row.contact_id)
            .await
            .unwrap_or_default();
        rt.publish(AgentEvent::ContactFileChanged {
            session,
            file: row.card(r, &alias),
        })
        .await;
    }

    /// One pass: the States rules, deadlines, the download queue and the outbox.
    pub async fn tick(self: &Arc<Self>) -> anyhow::Result<()> {
        let r = self.render().await;
        let skip = self.sched.lock().running.clone();
        for (row, kind) in self.store.settle_files(&r, &skip).await? {
            self.changed(&r, &row).await;
            match (kind, row.direction.as_str()) {
                (Settled::Received, _) => self.send_receipt(&row, Receipt::Received),
                (Settled::Noticed, _) => {}
                (Settled::Failed, "in") => {
                    // Saved files of a failed exchange are rolled back by inode.
                    let target = row.path.as_deref().map(Path::new);
                    let ours = target.zip(row.ino()).map(|(t, (d, i))| (t, d, i));
                    if let Err(e) = disk::abandon(&self.partial(&row.id), ours) {
                        tracing::warn!(file = %row.id, "199: rollback: {e}");
                    }
                    self.send_receipt(
                        &row,
                        Receipt::Failed {
                            reason: reason(row.error.as_deref().unwrap_or_default()),
                        },
                    );
                }
                (Settled::Failed, _) => self.drop_clone(&row),
            }
        }
        for row in self.store.expired_files(now_ms(), &skip).await? {
            self.expire(&r, row).await?;
        }
        self.schedule().await?;
        self.outbox().await?;
        Ok(())
    }

    async fn expire(&self, r: &Render, row: FileRow) -> anyhow::Result<()> {
        let done = if row.direction == "out" {
            let alias = self.store.contact_alias(&row.contact_id).await?;
            let why = match row.state.as_str() {
                "queued" => format!("expired before {alias} fetched it"),
                _ => "expired without a receipt".to_owned(),
            };
            let done = self.store.offer_fail(&row.id, &why).await?;
            if let Some(d) = &done {
                self.drop_clone(d);
            }
            done
        } else {
            let done = self
                .store
                .file_fail(r, &row.id, "the offer expired")
                .await?;
            remove(&self.partial(&row.id));
            done
        };
        if let Some(d) = done {
            self.changed(r, &d).await;
        }
        Ok(())
    }

    fn drop_clone(&self, row: &FileRow) {
        if let Some(c) = &row.clone {
            remove(Path::new(c));
        }
    }

    /// D7: start every queued download whose member has a free slot.
    async fn schedule(self: &Arc<Self>) -> anyhow::Result<()> {
        let mut queued = self.store.queued_files().await?;
        let now = Instant::now();
        let mut start = Vec::new();
        {
            let mut s = self.sched.lock();
            queued.sort_by_key(|r| s.requeued.get(&r.id).copied().unwrap_or(0));
            for row in queued {
                if s.running.contains(&row.id) || !s.due(&row.id, now) {
                    continue;
                }
                let member = source_of(&row.source).map_or_else(String::new, |(m, _)| m);
                let n = s.members.entry(member).or_default();
                if *n >= PER_MEMBER {
                    continue;
                }
                *n += 1;
                s.running.insert(row.id.clone());
                start.push(row);
            }
        }
        for row in start {
            let me = self.clone();
            tokio::spawn(async move {
                let member = source_of(&row.source).map_or_else(String::new, |(m, _)| m);
                let id = row.id.clone();
                let requeue = me.download(row).await;
                {
                    let mut s = me.sched.lock();
                    s.running.remove(&id);
                    if let Some(n) = s.members.get_mut(&member) {
                        *n = n.saturating_sub(1);
                    }
                    match requeue {
                        Requeue::Done => {
                            s.wait.remove(&id);
                            s.requeued.remove(&id);
                        }
                        Requeue::Back => {
                            s.turn += 1;
                            let t = s.turn;
                            s.requeued.insert(id, t);
                        }
                        Requeue::Later => s.back_off(&id, me.backoff),
                    }
                }
                kick();
            });
        }
        Ok(())
    }

    /// One download attempt of `row`, through publication.
    async fn download(self: &Arc<Self>, row: FileRow) -> Requeue {
        let r = self.render().await;
        let row = match self.store.file_to(&row.id, &["queued"], "receiving").await {
            Ok(Some(row)) => row,
            Ok(None) => return Requeue::Done,
            Err(e) => {
                tracing::warn!(file = %row.id, "199: start download: {e:#}");
                return Requeue::Later;
            }
        };
        self.changed(&r, &row).await;
        let (Some((member, sender)), Some(size)) = (source_of(&row.source), row.size) else {
            self.fail(&r, &row, "malformed file reference").await;
            return Requeue::Done;
        };
        let size = size as u64;
        let partial = self.partial(&row.id);
        let have = match self.open_partial(&partial, size) {
            Ok(n) => n,
            Err(e) => {
                self.fail(&r, &row, &format!("saving failed: {e}")).await;
                return Requeue::Done;
            }
        };
        let fetch = super::Fetch {
            contact: row.contact_id.clone(),
            member,
            id: sender,
            size,
            partial: partial.clone(),
            have,
            timer: Timer {
                deadline: instant_at(row.deadline_ms),
                idle: self.idle,
            },
        };
        let seen = Arc::new(AtomicU64::new(have));
        let flush = {
            let (me, seen, r, mut row) = (self.clone(), seen.clone(), r.clone(), row.clone());
            tokio::spawn(async move {
                let mut last = have;
                loop {
                    tokio::time::sleep(PROGRESS_EVERY).await;
                    let n = seen.load(Ordering::Relaxed);
                    if n != last {
                        last = n;
                        let _ = me.store.file_done(&row.id, n).await;
                        row.done = n as i64;
                        me.changed(&r, &row).await;
                    }
                }
            })
        };
        let mut progress = |n: u64| seen.store(n, Ordering::Relaxed);
        let got = self.fetcher().download(&fetch, &mut progress).await;
        flush.abort();
        let got = match got {
            Ok(got) => got,
            Err(e) => {
                if let Some(why) = fatal(&e, &r.machine) {
                    self.fail(&r, &row, &why).await;
                    return Requeue::Done;
                }
                let done = std::fs::metadata(&partial).map_or(0, |m| m.len());
                if let Ok(true) = self.store.file_requeue(&row.id, done).await
                    && let Ok(Some(row)) = self.store.contact_file(&row.id).await
                {
                    self.changed(&r, &row).await;
                }
                return match e {
                    TransferError::Idle => Requeue::Back,
                    _ => Requeue::Later,
                };
            }
        };
        if got.bytes != size || Some(&got.blake3) != row.blake3.as_ref() {
            self.fail(&r, &row, "the file changed before it was fetched")
                .await;
            return Requeue::Done;
        }
        self.publish(&r, &row, size, &got.blake3).await;
        Requeue::Done
    }

    /// The partial at `p`, created 0600 when missing: the bytes already in it (a partial
    /// longer than `size` starts over).
    fn open_partial(&self, p: &Path, size: u64) -> io::Result<u64> {
        mkdirs(p.parent().unwrap_or(&self.root))?;
        let f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(p)?;
        let n = f.metadata()?.len();
        if n > size {
            f.set_len(0)?;
            return Ok(0);
        }
        Ok(n)
    }

    /// Storage steps 3–6 for a verified partial of `row`, then its receipt when received.
    async fn publish(self: &Arc<Self>, r: &Render, row: &FileRow, size: u64, blake3: &str) {
        match publish(&self.store, &self.root, r, row, size, blake3).await {
            Ok(Some(done)) => {
                self.changed(r, &done).await;
                if done.state == "received" {
                    self.send_receipt(&done, Receipt::Received);
                }
            }
            Ok(None) => {}
            Err(e) => {
                let why = match e.raw_os_error() {
                    Some(libc::ENOSPC) => format!("disk full on {}", r.machine),
                    _ => format!("saving failed: {e}"),
                };
                self.fail(r, row, &why).await;
            }
        }
    }

    /// Fail `row` (terminal; its notice queued when its text was read), drop the partial
    /// and send the failed receipt.
    async fn fail(self: &Arc<Self>, r: &Render, row: &FileRow, why: &str) {
        let done = match self.store.file_fail(r, &row.id, why).await {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(file = %row.id, "199: fail: {e:#}");
                None
            }
        };
        remove(&self.partial(&row.id));
        if let Some(d) = done {
            self.changed(r, &d).await;
            self.send_receipt(
                &d,
                Receipt::Failed {
                    reason: reason(why),
                },
            );
        }
    }

    /// The terminal receipt of `row`, retried on D8's backoff until the sender answers
    /// (200, 404, 410) or the deadline.
    fn send_receipt(self: &Arc<Self>, row: &FileRow, receipt: Receipt) {
        let Some((member, sender)) = source_of(&row.source) else {
            return;
        };
        if !self.sched.lock().receipts.insert(row.id.clone()) {
            return;
        }
        let (me, contact, id, deadline) = (
            self.clone(),
            row.contact_id.clone(),
            row.id.clone(),
            row.deadline_ms,
        );
        tokio::spawn(async move {
            let mut delay = me.backoff;
            loop {
                let timer = Timer {
                    deadline: instant_at(deadline),
                    idle: me.idle,
                };
                match me
                    .fetcher()
                    .send_receipt(&contact, &member, &sender, &receipt, timer)
                    .await
                {
                    Err(
                        TransferError::Transport(_) | TransferError::Idle | TransferError::NotYet,
                    ) if now_ms() < deadline => {
                        tokio::time::sleep(delay).await;
                        delay = (delay * 2).min(BACKOFF_MAX);
                    }
                    _ => break,
                }
            }
            me.sched.lock().receipts.remove(&id);
        });
    }

    /// Startup: every terminal file whose offer still lives gets its receipt again (a
    /// crash may have come between the commit and the POST; the sender applies the first).
    async fn resend_receipts(self: &Arc<Self>) {
        let rows = match self.store.receipts_due(now_ms()).await {
            Ok(rows) => rows,
            Err(e) => return tracing::warn!("199: receipts: {e:#}"),
        };
        for row in rows {
            let receipt = match row.state.as_str() {
                "received" => Receipt::Received,
                _ => Receipt::Failed {
                    reason: reason(row.error.as_deref().unwrap_or_default()),
                },
            };
            self.send_receipt(&row, receipt);
        }
    }

    /// D17's outbox: each queued notice to the chat's owner, each sent one asked for until
    /// committed (re-sent while unknown), on D8's backoff, with no deadline.
    async fn outbox(self: &Arc<Self>) -> anyhow::Result<()> {
        let rows = self.store.notice_outbox().await?;
        let now = Instant::now();
        for row in rows {
            let key = format!("n:{}", row.id);
            {
                let mut s = self.sched.lock();
                if !s.due(&key, now) || !s.notices.insert(row.id.clone()) {
                    continue;
                }
            }
            let me = self.clone();
            tokio::spawn(async move {
                let delivered = me.notice_step(&row).await;
                let mut s = me.sched.lock();
                s.notices.remove(&row.id);
                if delivered {
                    s.wait.remove(&key);
                } else {
                    s.back_off(&key, me.backoff);
                }
            });
        }
        Ok(())
    }

    /// One outbox step of `row`; `true` once it is delivered.
    async fn notice_step(&self, row: &FileRow) -> bool {
        let (Ok(session), Some(text)) = (row.session.parse::<SessionId>(), row.notice.as_deref())
        else {
            return false;
        };
        let id = notice_id(self.store.engine(), &row.id);
        let send = || self.send_notice(session, id, row, text);
        match row.notice_state.as_deref() {
            Some("queued") => {
                if send().await {
                    let _ = self.store.notice_to(&row.id, "queued", "sent").await;
                }
                false
            }
            Some("sent") => match self.door.status(session, id).await {
                Ok(NoticeStatus::Committed) => {
                    let _ = self.store.notice_to(&row.id, "sent", "delivered").await;
                    true
                }
                Ok(NoticeStatus::Unknown) => {
                    send().await;
                    false
                }
                Err(e) => {
                    tracing::info!(file = %row.id, %session, "199: notice status: {e}");
                    false
                }
            },
            _ => true,
        }
    }

    /// `contact_file_notice` for `row` through the door; `true` when accepted.
    async fn send_notice(
        &self,
        session: SessionId,
        id: uuid::Uuid,
        row: &FileRow,
        text: &str,
    ) -> bool {
        match self.door.notice(session, id, &row.contact_id, text).await {
            Ok(_) => true,
            Err(e) => {
                tracing::info!(file = %row.id, %session, "199: notice not accepted yet: {e}");
                false
            }
        }
    }

    /// `true` when `contact` (the caller's record id) is the offer's contact.
    async fn same_contact(&self, row: &FileRow, contact: &str) -> bool {
        row.contact_id == contact
            || self
                .store
                .share_root_eq(&row.contact_id, contact)
                .await
                .unwrap_or(false)
    }

    /// The offer `id` as the route may serve it to `contact`.
    async fn offer(&self, contact: &str, id: &str) -> Result<FileRow, OfferError> {
        let row = self
            .store
            .contact_file(id)
            .await
            .ok()
            .flatten()
            .filter(|r| r.direction == "out" && !r.exchange.starts_with("pending:"))
            .ok_or(OfferError::Unknown)?;
        if !self.same_contact(&row, contact).await {
            return Err(OfferError::Unknown);
        }
        Ok(row)
    }
}

/// What the queue does with a download after an attempt.
enum Requeue {
    /// Finished (published, failed, or not queued any more).
    Done,
    /// Idle: to the back of the queue, resumed with `Range`.
    Back,
    /// A transport failure: after D8's backoff.
    Later,
}

/// Storage steps 3–6 for `row`'s verified partial: quarantine, persist `publishing` and
/// link under the first free name in `files/received/<contact>/<day>/`, then commit.
/// `Ok(None)`: the row failed meanwhile (our target, if linked, is removed by inode).
///
/// It runs on a blocking thread (the quarantine, the links and the fsyncs are blocking
/// I/O): `disk::publish`'s `AsyncFnMut` callback yields a future rustc cannot prove
/// `Send` for every argument lifetime, so it never runs inside a spawned task's future.
pub async fn publish(
    store: &Store,
    root: &Path,
    r: &Render,
    row: &FileRow,
    size: u64,
    blake3: &str,
) -> io::Result<Option<FileRow>> {
    let (store, root, r, row, blake3) = (
        store.clone(),
        root.to_path_buf(),
        r.clone(),
        row.clone(),
        blake3.to_owned(),
    );
    let rt = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        rt.block_on(publish_here(&store, &root, &r, &row, size, &blake3))
    })
    .await
    .map_err(io::Error::other)?
}

async fn publish_here(
    store: &Store,
    root: &Path,
    r: &Render,
    row: &FileRow,
    size: u64,
    blake3: &str,
) -> io::Result<Option<FileRow>> {
    let io = |e: anyhow::Error| io::Error::other(format!("{e:#}"));
    let partial = root.join(PARTIAL_DIR).join(&row.id);
    let alias = store.contact_alias(&row.contact_id).await.map_err(io)?;
    let dir = received_dir(root, &alias, chrono::Local::now().date_naive());
    mkdirs(&dir)?;
    std::fs::set_permissions(&partial, std::fs::Permissions::from_mode(0o600))?;
    let value = disk::quarantine_value(
        chrono::Utc::now().timestamp().max(0) as u64,
        uuid::Uuid::new_v4(),
    );
    let (writer, id) = (store.clone(), row.id.clone());
    let mut linked = None;
    let published = disk::publish(&partial, &dir, &row.name, &value, |t: &Path, dev, ino| {
        linked = Some((t.to_path_buf(), dev, ino));
        let (store, id, t) = (writer.clone(), id.clone(), t.to_string_lossy().into_owned());
        async move {
            match store.file_publishing(&id, &t, dev, ino).await {
                Ok(true) => Ok(()),
                Ok(false) => Err(io::Error::other("the file is no longer being received")),
                Err(e) => Err(io::Error::other(format!("{e:#}"))),
            }
        }
    })
    .await;
    let target = match published {
        Ok(t) => t,
        Err(e) => {
            disk::abandon(&partial, None)?;
            return Err(e);
        }
    };
    let (_, dev, ino) = linked.expect("publish recorded its target");
    match store.file_published(r, &row.id, size, blake3).await {
        Ok(Some(done)) => {
            disk::finish(&partial)?;
            Ok(Some(done))
        }
        Ok(None) => {
            disk::abandon(&partial, Some((&target, dev, ino)))?;
            Ok(None)
        }
        // Left `publishing`: the next start resolves it by inode.
        Err(e) => Err(io(e)),
    }
}

/// Engine start, before anything is downloaded (Restart): every `publishing` row is
/// resolved by inode first, then `files/partial/` is emptied and every unfinished download
/// restarts from byte 0. Offers staged by a send that never got its exchange go, and so do
/// clones of settled offers. Failed exchanges' files fail at the service's first pass.
pub async fn recover(store: &Store, root: &Path) {
    if let Err(e) = recover_in(store, root).await {
        tracing::error!("199: file recovery: {e:#}");
    }
}

async fn recover_in(store: &Store, root: &Path) -> anyhow::Result<()> {
    for row in store.drop_staged_offers("pending:%").await? {
        if let Some(c) = &row.clone {
            remove(Path::new(c));
        }
    }
    let r = render(store).await;
    let partials = root.join(PARTIAL_DIR);
    for row in store.files_in_state("publishing").await? {
        let partial = partials.join(&row.id);
        let (Some(target), Some((dev, ino)), Some(size), Some(blake3)) = (
            row.path.as_deref(),
            row.ino(),
            row.size,
            row.blake3.as_deref(),
        ) else {
            store.file_requeue(&row.id, 0).await?;
            continue;
        };
        match disk::resolve(&partial, Path::new(target), dev, ino)? {
            disk::Restart::Finish => {
                if store
                    .file_published(&r, &row.id, size as u64, blake3)
                    .await?
                    .is_some()
                {
                    disk::finish(&partial)?;
                }
            }
            disk::Restart::Republish => {
                if let Err(e) = publish(store, root, &r, &row, size as u64, blake3).await {
                    tracing::warn!(file = %row.id, "199: republish: {e}");
                    store.file_requeue(&row.id, 0).await?;
                }
            }
            disk::Restart::Requeue => {
                store.file_requeue(&row.id, 0).await?;
            }
        }
    }
    for row in store.files_in_state("receiving").await? {
        store.file_requeue(&row.id, 0).await?;
    }
    for row in store.files_in_state("queued").await? {
        if row.done != 0 {
            store.file_requeue_queued(&row.id).await?;
        }
    }
    match std::fs::read_dir(&partials) {
        Ok(entries) => {
            for e in entries.flatten() {
                remove(&e.path());
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    if let Ok(entries) = std::fs::read_dir(root.join(OUTGOING_DIR)) {
        for e in entries.flatten() {
            let id = e.file_name().to_string_lossy().into_owned();
            let live = store
                .contact_file(&id)
                .await?
                .is_some_and(|row| !row.terminal() && row.deadline_ms > now_ms());
            if !live {
                remove(&e.path());
            }
        }
    }
    Ok(())
}

#[async_trait::async_trait]
impl FileOffers for FileService {
    async fn open(&self, contact: &str, id: &str) -> Result<OfferHandle, OfferError> {
        let row = self.offer(contact, id).await?;
        let r = self.render().await;
        let deadline = crate::store::contact_files::at_ms(row.deadline_ms);
        if row.deadline_ms <= now_ms() {
            let _ = self.expire(&r, row).await;
            return Err(OfferError::Gone(Gone::Expired));
        }
        if row.state == "failed" {
            return Err(OfferError::Gone(Gone::Missing));
        }
        let (Some(size), Some(blake3)) = (row.size, row.blake3.clone()) else {
            return Err(OfferError::Unknown);
        };
        let size = size as u64;
        let file = match row.clone.as_deref().map(std::fs::File::open) {
            Some(Ok(f)) if f.metadata().is_ok_and(|m| m.len() == size) => Ok(f),
            _ => {
                let (dev, ino) = row.ino().unwrap_or_default();
                disk::egress_reopen(Path::new(&row.source), &self.root, dev, ino, size)
                    .map(|e| e.file)
                    .map_err(|e| match e.refusal {
                        disk::Refusal::NotFound => Gone::Missing,
                        _ => Gone::Changed,
                    })
            }
        };
        let file = match file {
            Ok(f) => f,
            Err(gone) => {
                let why = match gone {
                    Gone::Missing => "the file is gone".to_owned(),
                    _ => format!("{} changed since it was offered", row.source),
                };
                if let Ok(Some(done)) = self.store.offer_fail(&row.id, &why).await {
                    self.drop_clone(&done);
                    self.changed(&r, &done).await;
                }
                return Err(OfferError::Gone(gone));
            }
        };
        if let Ok(Some(sending)) = self.store.file_to(&row.id, &["queued"], "sending").await {
            self.changed(&r, &sending).await;
        }
        Ok(OfferHandle {
            file,
            len: size,
            blake3,
            deadline,
        })
    }

    async fn receipt(&self, contact: &str, id: &str, receipt: Receipt) -> Result<(), OfferError> {
        let row = self.offer(contact, id).await?;
        if row.terminal() {
            return Ok(());
        }
        let r = self.render().await;
        if row.deadline_ms <= now_ms() {
            let _ = self.expire(&r, row).await;
            return Err(OfferError::Gone(Gone::Expired));
        }
        let why = match &receipt {
            Receipt::Received => None,
            Receipt::Failed { reason } => Some(reason.as_str()),
        };
        if let Ok(Some(done)) = self.store.offer_settle(&row.id, why).await {
            self.drop_clone(&done);
            self.changed(&r, &done).await;
        }
        Ok(())
    }
}
