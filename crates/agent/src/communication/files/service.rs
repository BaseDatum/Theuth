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

use super::foreign::{self, Failure};
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

/// The queue slot a row takes (D7): its sending member, or, for a foreign file, its sender.
fn member(row: &FileRow) -> String {
    source_of(&row.source).map_or_else(|| row.contact_id.clone(), |(m, _)| m)
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
    /// 199-E3: the template of every foreign download (tests: the loopback allowlist and
    /// root; the limits and the timer are set per download).
    pub web: web::fetch::DownloadOpts,
    door: Arc<dyn NoticeDoor>,
    sched: parking_lot::Mutex<Sched>,
    /// UI-3: each offer's last recorded byte count and when (at most two writes a second).
    served: parking_lot::Mutex<HashMap<String, (u64, Instant)>>,
}

/// `rt`'s engine's running service (the send path reaches it through [`FileServiceOf`]).
pub fn service(rt: &Runtime) -> Option<Arc<FileService>> {
    super::Slot::<FileService>::of(rt).get()
}

/// `ask` → this engine's running file service (its `Runtime`'s), if one started.
pub struct FileServiceOf;

impl kameo::message::Message<FileServiceOf> for crate::supervisor::Agents {
    type Reply = Option<Arc<FileService>>;

    async fn handle(
        &mut self,
        _: FileServiceOf,
        _: &mut kameo::message::Context<Self, Self::Reply>,
    ) -> Self::Reply {
        service(&self.rt)
    }
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
            web: web::fetch::DownloadOpts::default(),
            door,
            sched: parking_lot::Mutex::default(),
            served: parking_lot::Mutex::default(),
        })
    }

    /// Register as this engine's offers and service (in its `rt`; a service without one, a
    /// unit test's, registers nothing), then drive the queue until shutdown.
    pub fn start(self: &Arc<Self>, shutdown: CancellationToken) {
        if let Some(rt) = &self.rt {
            super::set_offers(rt, self.clone());
            super::Slot::of(rt).set(self.clone());
        }
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
        match (&self.fetcher, &self.rt) {
            (Some(f), _) => f.clone(),
            (None, Some(rt)) => super::fetch(rt),
            (None, None) => Arc::new(super::NoFetch),
        }
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
                    // No notice will ever come: a foreign file's authority goes now.
                    foreign::release(notice_id(self.store.engine(), &row.id));
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
        // Round 3: a foreign file that failed before its exchange ended unread never gets a
        // notice either; its authority goes. ponytail: re-scanned each pass while any
        // authority is held; mark released rows if the table grows large.
        if foreign::registered() {
            for id in self.store.foreign_files_noticeless().await? {
                foreign::release(notice_id(self.store.engine(), &id));
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
            let why = match foreign::is_foreign(&row.contact_id) {
                true => foreign::NO_RESPONSE,
                false => "the offer expired",
            };
            let done = self.store.file_fail(r, &row.id, why).await?;
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
                let member = member(&row);
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
                let member = member(&row);
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
        if foreign::is_source(&row.source) {
            return self.foreign(&r, &row).await;
        }
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

    /// 199-E3 (D16): one attempt at a foreign file, from byte 0 (never resumed): fetched
    /// from its `https` URL or decoded from its inline bytes, measured, then published.
    /// SEC-P4: only while the authority it arrived under lives; the download runs under
    /// that cancellation, and an ended one fails the file (`share ended`), never requeued.
    async fn foreign(self: &Arc<Self>, r: &Render, row: &FileRow) -> Requeue {
        let notice = notice_id(self.store.engine(), &row.id);
        let Some(cancel) = foreign::authority(notice) else {
            self.fail(r, row, foreign::SHARE_ENDED).await;
            return Requeue::Done;
        };
        let partial = self.partial(&row.id);
        if let Err(e) = mkdirs(partial.parent().unwrap_or(&self.root)) {
            self.fail(r, row, &format!("saving failed: {e}")).await;
            return Requeue::Done;
        }
        remove(&partial);
        let opts = web::fetch::DownloadOpts {
            max_bytes: super::FILE_MAX,
            idle_timeout: self.idle,
            deadline: Some(instant_at(row.deadline_ms).into_std()),
            cancel,
            ..self.web.clone()
        };
        match foreign::fetch(&row.source, &partial, &opts, &r.machine).await {
            Ok(_) if foreign::authority(notice).is_none() => {
                remove(&partial);
                self.fail(r, row, foreign::SHARE_ENDED).await;
                Requeue::Done
            }
            Ok(got) => {
                if let Err(e) = self
                    .store
                    .file_measured(&row.id, got.bytes, &got.blake3)
                    .await
                {
                    tracing::warn!(file = %row.id, "199: measured: {e:#}");
                }
                self.publish(r, row, got.bytes, &got.blake3).await;
                Requeue::Done
            }
            Err(Failure::Fatal(why)) => {
                self.fail(r, row, &why).await;
                Requeue::Done
            }
            Err(Failure::Retry { idle }) => {
                remove(&partial);
                if let Ok(true) = self.store.file_requeue(&row.id, 0).await
                    && let Ok(Some(row)) = self.store.contact_file(&row.id).await
                {
                    self.changed(r, &row).await;
                }
                match idle {
                    true => Requeue::Back,
                    false => Requeue::Later,
                }
            }
        }
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
                    foreign::release(id);
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
        // Round 2: the holder judges a foreign file's authority; the owner honours its mark.
        let contact = foreign::notice_contact(&row.contact_id, id);
        match self.door.notice(session, id, &contact, text).await {
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
        // A failed read is no answer about the offer (SEC-A4): retryable, never `404`.
        let row = self
            .store
            .contact_file(id)
            .await
            .map_err(|_| OfferError::Unavailable)?
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
        // SEC-A5: step 6 failed after the link: our target goes (by inode) with the partial
        // before the caller records the failure, so no published file is left behind. If
        // that record fails too, the row stays `publishing` and the next start, finding
        // neither, downloads it again.
        Err(e) => {
            disk::abandon(&partial, Some((&target, dev, ino)))?;
            Err(io(e))
        }
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
        // SEC-A4: `200` only once the settlement is durable, or the offer was terminal
        // already (a concurrent first receipt won); a failed write is retryable.
        match self.store.offer_settle(&row.id, why).await {
            Ok(Some(done)) => {
                self.served.lock().remove(&done.id);
                self.drop_clone(&done);
                self.changed(&r, &done).await;
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(id = %row.id, "199: receipt not recorded: {e:#}");
                return Err(OfferError::Unavailable);
            }
        }
        Ok(())
    }

    /// UI-3: the bytes the receiver has, recorded on the `sending` row (its `done`) and
    /// announced, at most twice a second per offer; the last count is written by the receipt.
    fn served(&self, _contact: &str, id: &str, at: u64) {
        let now = Instant::now();
        {
            let mut served = self.served.lock();
            let last = served.get(id).copied();
            if last.is_some_and(|(n, t)| n == at || now.duration_since(t) < PROGRESS_EVERY) {
                return;
            }
            served.insert(id.to_owned(), (at, now));
        }
        let (store, rt, id) = (self.store.clone(), self.rt.clone(), id.to_owned());
        tokio::spawn(async move {
            let Ok(Some(row)) = store.offer_done(&id, at).await else {
                return;
            };
            let (Some(rt), Ok(session)) = (rt, row.session.parse::<SessionId>()) else {
                return;
            };
            let r = render(&store).await;
            let alias = store
                .contact_alias(&row.contact_id)
                .await
                .unwrap_or_default();
            rt.publish(AgentEvent::ContactFileChanged {
                session,
                file: row.card(&r, &alias),
            })
            .await;
        });
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use genai::chat::ChatMessage;
    use tokio::sync::Semaphore;

    use super::*;
    use crate::communication::files::{Fetch, Fetched, NoticeAdmission};
    use crate::store::communication::AcceptExchange;

    const SENDER: &str = "peer:00000000-0000-0000-0000-000000000001";

    /// The sender's side, faked: each member's bytes by sender id, an optional gate per
    /// member (a slow sender), files answered `Idle` once after half their bytes.
    #[derive(Default)]
    struct Fake {
        bytes: parking_lot::Mutex<HashMap<String, Vec<u8>>>,
        gates: parking_lot::Mutex<HashMap<String, Arc<Semaphore>>>,
        idle_once: parking_lot::Mutex<HashSet<String>>,
        active: parking_lot::Mutex<HashMap<String, usize>>,
        peak: parking_lot::Mutex<HashMap<String, usize>>,
        calls: parking_lot::Mutex<Vec<(String, u64)>>,
        receipts: parking_lot::Mutex<Vec<(String, Receipt)>>,
    }

    #[async_trait::async_trait]
    impl FileFetch for Fake {
        async fn download(
            &self,
            fetch: &Fetch,
            progress: &mut (dyn FnMut(u64) + Send),
        ) -> Result<Fetched, TransferError> {
            self.calls.lock().push((fetch.id.clone(), fetch.have));
            {
                let mut a = self.active.lock();
                let n = a.entry(fetch.member.clone()).or_default();
                *n += 1;
                let mut p = self.peak.lock();
                let m = p.entry(fetch.member.clone()).or_default();
                *m = (*m).max(*n);
            }
            let gate = self.gates.lock().get(&fetch.member).cloned();
            if let Some(g) = gate {
                g.acquire().await.unwrap().forget();
            }
            let all = self
                .bytes
                .lock()
                .get(&fetch.id)
                .cloned()
                .unwrap_or_default();
            let idle = self.idle_once.lock().remove(&fetch.id);
            let end = if idle { all.len() / 2 } else { all.len() };
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&fetch.partial)
                .map_err(TransferError::Disk)?;
            std::io::Write::write_all(&mut f, &all[fetch.have as usize..end])
                .map_err(TransferError::Disk)?;
            progress(end as u64);
            *self.active.lock().get_mut(&fetch.member).unwrap() -= 1;
            if idle {
                return Err(TransferError::Idle);
            }
            let whole = std::fs::read(&fetch.partial).map_err(TransferError::Disk)?;
            Ok(Fetched {
                bytes: whole.len() as u64,
                blake3: blake3::hash(&whole).to_hex().to_string(),
            })
        }

        async fn send_receipt(
            &self,
            _: &str,
            _: &str,
            id: &str,
            receipt: &Receipt,
            _: Timer,
        ) -> Result<(), TransferError> {
            self.receipts.lock().push((id.to_owned(), receipt.clone()));
            Ok(())
        }
    }

    /// The chat's owner, faked: it accepts every notice and answers `unknown` to the first
    /// `unknown` status asks; `old` answers method-not-found to everything.
    #[derive(Default)]
    struct Door {
        notices: parking_lot::Mutex<Vec<uuid::Uuid>>,
        unknown: AtomicUsize,
        old: std::sync::atomic::AtomicBool,
    }

    #[async_trait::async_trait]
    impl NoticeDoor for Door {
        async fn notice(
            &self,
            _: SessionId,
            id: uuid::Uuid,
            _: &str,
            _: &str,
        ) -> Result<NoticeAdmission, String> {
            if self.old.load(Ordering::SeqCst) {
                return Err("Method not found (-32601)".into());
            }
            self.notices.lock().push(id);
            Ok(NoticeAdmission::Accepted)
        }

        async fn status(&self, _: SessionId, _: uuid::Uuid) -> Result<NoticeStatus, String> {
            if self.old.load(Ordering::SeqCst) {
                return Err("Method not found (-32601)".into());
            }
            let left = self.unknown.load(Ordering::SeqCst);
            if left > 0 {
                self.unknown.store(left - 1, Ordering::SeqCst);
                return Ok(NoticeStatus::Unknown);
            }
            Ok(NoticeStatus::Committed)
        }
    }

    struct Rig {
        svc: Arc<FileService>,
        fake: Arc<Fake>,
        door: Arc<Door>,
        session: SessionId,
        _dir: crate::TestDir,
    }

    async fn rig() -> Rig {
        let dir = crate::TestDir::new("fsvc");
        let store = Store::open(&dir.0.join("sessions.sqlite"), None)
            .await
            .unwrap();
        let session = SessionId::new();
        store
            .create_session(session, "m", None, None, None)
            .await
            .unwrap();
        let (fake, door) = (Arc::new(Fake::default()), Arc::new(Door::default()));
        let mut svc = FileService::new(store, dir.0.clone(), None, door.clone());
        let s = Arc::get_mut(&mut svc).unwrap();
        s.fetcher = Some(fake.clone());
        s.backoff = Duration::from_millis(20);
        s.idle = Duration::from_millis(200);
        Rig {
            svc,
            fake,
            door,
            session,
            _dir: dir,
        }
    }

    impl Rig {
        async fn exchange(&self) -> uuid::Uuid {
            self.svc
                .store
                .accept_exchange(&AcceptExchange {
                    sender: SENDER,
                    recipient: &format!("session:{}", self.session),
                    sender_session: None,
                    recipient_session: Some(self.session),
                    request_id: None,
                    reply_to: None,
                    request_scope: None,
                    generation: 1,
                    body: "hi",
                })
                .await
                .unwrap()
                .id
        }

        async fn consume(&self, ex: uuid::Uuid) {
            self.svc
                .store
                .consume_exchange(ex, self.session, 1, ChatMessage::user("hi"))
                .await
                .unwrap()
                .unwrap();
        }

        /// One queued `in` file of `bytes` from `member`, named `name`.
        async fn file(&self, ex: uuid::Uuid, member: &str, name: &str, bytes: &[u8]) -> String {
            let sender = uuid::Uuid::new_v4().simple().to_string();
            self.fake
                .bytes
                .lock()
                .insert(sender.clone(), bytes.to_vec());
            let id = uuid::Uuid::new_v4().simple().to_string();
            let row = FileRow {
                id: id.clone(),
                direction: "in".into(),
                session: self.session.to_string(),
                exchange: ex.to_string(),
                contact_id: "rec".into(),
                name: name.into(),
                media_type: "application/pdf".into(),
                size: Some(bytes.len() as i64),
                blake3: Some(blake3::hash(bytes).to_hex().to_string()),
                source: format!("iroh://{member}/a2a/files/{sender}"),
                state: "queued".into(),
                created_ms: now_ms(),
                deadline_ms: now_ms() + 3_600_000,
                ..Default::default()
            };
            self.svc.store.insert_contact_files(&[row]).await.unwrap();
            id
        }

        async fn row(&self, id: &str) -> FileRow {
            self.svc.store.contact_file(id).await.unwrap().unwrap()
        }

        /// Passes until `done` holds (at most 5 s).
        async fn until(&self, what: &str, mut done: impl AsyncFnMut(&Self) -> bool) {
            for _ in 0..250 {
                self.svc.tick().await.unwrap();
                if done(self).await {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            panic!("timed out waiting for {what}");
        }

        fn dir(&self) -> PathBuf {
            received_dir(&self.svc.root, "rec", chrono::Local::now().date_naive())
        }
    }

    /// D7: at most two downloads per sending member, none engine-wide; a slow sender holds
    /// only its own files, which finish once it moves.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn two_per_member_and_a_slow_sender_delays_only_its_own() {
        let rig = rig().await;
        rig.fake
            .gates
            .lock()
            .insert("slow".into(), Arc::new(Semaphore::new(0)));
        let ex = rig.exchange().await;
        let mut slow = Vec::new();
        for n in 0..4 {
            slow.push(rig.file(ex, "slow", &format!("s{n}"), b"slow bytes").await);
        }
        let fast = rig.file(ex, "fast", "f", b"fast bytes").await;
        rig.until("the fast file", async |r| {
            r.row(&fast).await.state == "saved"
        })
        .await;
        for id in &slow {
            assert_ne!(rig.row(id).await.state, "saved");
        }
        assert_eq!(rig.fake.peak.lock().get("slow").copied(), Some(2));
        let gate = rig.fake.gates.lock().get("slow").cloned().unwrap();
        gate.add_permits(4);
        rig.until("every slow file", async |r| {
            for id in &slow {
                if r.row(id).await.state != "saved" {
                    return false;
                }
            }
            true
        })
        .await;
        assert_eq!(rig.fake.peak.lock().get("slow").copied(), Some(2));
    }

    /// D7: an idle download goes back to the end of the queue and resumes from its bytes.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_idle_download_requeues_and_resumes() {
        let rig = rig().await;
        let ex = rig.exchange().await;
        let id = rig.file(ex, "m", "big.bin", &[7u8; 1000]).await;
        let sender = source_of(&rig.row(&id).await.source).unwrap().1;
        rig.fake.idle_once.lock().insert(sender.clone());
        rig.until("the file", async |r| r.row(&id).await.state == "saved")
            .await;
        let calls = rig.fake.calls.lock().clone();
        assert_eq!(calls, vec![(sender.clone(), 0), (sender, 500)]);
    }

    /// Publication, then States: saved until the text is read, then exactly one notice
    /// (delivered once committed, re-sent while unknown) and one received receipt. An
    /// existing `report.pdf` with other bytes survives; ours is `report (2).pdf`, 0600.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn saved_then_received_with_one_notice_and_no_clobber() {
        let rig = rig().await;
        rig.door.unknown.store(1, Ordering::SeqCst);
        mkdirs(&rig.dir()).unwrap();
        std::fs::write(rig.dir().join("report.pdf"), b"theirs").unwrap();
        let ex = rig.exchange().await;
        let id = rig.file(ex, "m", "report.pdf", b"our report").await;
        rig.until("saved", async |r| r.row(&id).await.state == "saved")
            .await;
        assert!(
            rig.fake.receipts.lock().is_empty(),
            "no receipt before received"
        );
        assert!(rig.door.notices.lock().is_empty());
        rig.consume(ex).await;
        rig.until("delivered", async |r| {
            r.row(&id).await.notice_state.as_deref() == Some("delivered")
        })
        .await;
        let row = rig.row(&id).await;
        let ours = rig.dir().join("report (2).pdf");
        assert_eq!(row.path.as_deref(), Some(ours.to_str().unwrap()));
        assert_eq!(std::fs::read(&ours).unwrap(), b"our report");
        assert_eq!(
            std::fs::read(rig.dir().join("report.pdf")).unwrap(),
            b"theirs"
        );
        use std::os::unix::fs::MetadataExt as _;
        assert_eq!(std::fs::metadata(&ours).unwrap().mode() & 0o777, 0o600);
        assert!(!rig.svc.partial(&id).exists());
        // One notice id, sent twice (the status read `unknown` once): one row on the owner.
        let sent = rig.door.notices.lock().clone();
        assert_eq!(sent.len(), 2);
        assert!(
            sent.iter()
                .all(|n| *n == notice_id(rig.svc.store.engine(), &id))
        );
        let notice = row.notice.unwrap();
        assert!(notice.contains("state=\"received\""), "{notice}");
        assert!(notice.contains(&format!("saved=\"{}\"", ours.display())));
        let receipts = rig.fake.receipts.lock().clone();
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0].1, Receipt::Received);
    }

    /// An identical pre-existing file is never adopted.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_identical_existing_file_is_not_adopted() {
        let rig = rig().await;
        mkdirs(&rig.dir()).unwrap();
        std::fs::write(rig.dir().join("report.pdf"), b"same bytes").unwrap();
        let ex = rig.exchange().await;
        rig.consume(ex).await;
        let id = rig.file(ex, "m", "report.pdf", b"same bytes").await;
        rig.until("received", async |r| r.row(&id).await.state == "received")
            .await;
        let row = rig.row(&id).await;
        assert!(row.path.unwrap().ends_with("report (2).pdf"));
    }

    /// HOLD_MOVED after a file was saved: the saved file is rolled back by inode, no
    /// notice is written, and the sender gets a failed receipt with the reason.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_moved_chat_rolls_its_saved_files_back() {
        let rig = rig().await;
        let ex = rig.exchange().await;
        let id = rig.file(ex, "m", "a.txt", b"bytes").await;
        rig.until("saved", async |r| r.row(&id).await.state == "saved")
            .await;
        let saved = PathBuf::from(rig.row(&id).await.path.unwrap());
        assert!(saved.exists());
        rig.svc
            .store
            .fail_exchange(ex, crate::communication::network::HOLD_MOVED)
            .await
            .unwrap();
        rig.until("failed", async |r| r.row(&id).await.state == "failed")
            .await;
        assert!(!saved.exists());
        let row = rig.row(&id).await;
        assert!(row.notice.is_none());
        let receipts = rig.fake.receipts.lock().clone();
        assert_eq!(
            receipts.last().map(|r| r.1.clone()),
            Some(Receipt::Failed {
                reason: crate::communication::network::HOLD_MOVED.into()
            })
        );
        assert!(rig.door.notices.lock().is_empty());
    }

    /// A digest mismatch fails the file at once (no retry), the partial goes, and its
    /// failure notice waits for the text.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_changed_file_fails_without_retry() {
        let rig = rig().await;
        let ex = rig.exchange().await;
        let id = rig.file(ex, "m", "a.txt", b"declared").await;
        let sender = source_of(&rig.row(&id).await.source).unwrap().1;
        rig.fake.bytes.lock().insert(sender, b"replaced".to_vec());
        rig.until("failed", async |r| r.row(&id).await.state == "failed")
            .await;
        let row = rig.row(&id).await;
        assert_eq!(
            row.error.as_deref(),
            Some("the file changed before it was fetched")
        );
        assert!(row.notice.is_none());
        assert!(!rig.svc.partial(&id).exists());
        rig.consume(ex).await;
        rig.until("noticed", async |r| r.row(&id).await.notice.is_some())
            .await;
        assert_eq!(rig.fake.calls.lock().len(), 1);
    }

    /// An owner on an old build answers method-not-found: the outbox keeps the notice
    /// queued and retries (the door is its only path; nothing falls back to send_input).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_old_owner_is_retried() {
        let rig = rig().await;
        rig.door.old.store(true, Ordering::SeqCst);
        let ex = rig.exchange().await;
        rig.consume(ex).await;
        let id = rig.file(ex, "m", "a.txt", b"bytes").await;
        rig.until("received", async |r| r.row(&id).await.state == "received")
            .await;
        for _ in 0..10 {
            rig.svc.tick().await.unwrap();
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        assert_eq!(rig.row(&id).await.notice_state.as_deref(), Some("queued"));
        rig.door.old.store(false, Ordering::SeqCst);
        rig.until("delivered", async |r| {
            r.row(&id).await.notice_state.as_deref() == Some("delivered")
        })
        .await;
    }

    /// Restart after the link, before the commit (a crash after saving, before the notice
    /// was queued): the row is ours by inode, step 6 is finished, exactly one notice.
    /// An unfinished download restarts from byte 0 and its partial goes.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn restart_finishes_ours_and_restarts_downloads() {
        let rig = rig().await;
        let ex = rig.exchange().await;
        rig.consume(ex).await;
        let (a, b) = (
            rig.file(ex, "m", "a.txt", b"aaaa").await,
            rig.file(ex, "m", "b.txt", b"bbbb").await,
        );
        let store = &rig.svc.store;
        // `a` crashed between step 5 (linked) and step 6 (commit).
        let partial = rig.svc.partial(&a);
        mkdirs(partial.parent().unwrap()).unwrap();
        std::fs::write(&partial, b"aaaa").unwrap();
        mkdirs(&rig.dir()).unwrap();
        let target = rig.dir().join("a.txt");
        std::fs::hard_link(&partial, &target).unwrap();
        let (dev, ino) = disk::inode_of(&partial).unwrap();
        store.file_to(&a, &["queued"], "receiving").await.unwrap();
        store
            .file_publishing(&a, &target.to_string_lossy(), dev, ino)
            .await
            .unwrap();
        // `b` was half way.
        std::fs::write(rig.svc.partial(&b), b"bb").unwrap();
        store.file_to(&b, &["queued"], "receiving").await.unwrap();
        store.file_done(&b, 2).await.unwrap();
        recover(store, &rig.svc.root).await;
        let ra = rig.row(&a).await;
        assert_eq!(ra.state, "received");
        assert_eq!(ra.notice_state.as_deref(), Some("queued"));
        assert!(!partial.exists() && target.exists());
        let rb = rig.row(&b).await;
        assert_eq!((rb.state.as_str(), rb.done), ("queued", 0));
        assert!(!rig.svc.partial(&b).exists());
        // A second start changes nothing: still one notice.
        recover(store, &rig.svc.root).await;
        rig.until("both done", async |r| {
            r.row(&a).await.notice_state.as_deref() == Some("delivered")
                && r.row(&b).await.state == "received"
        })
        .await;
        let sent = rig.door.notices.lock().clone();
        assert_eq!(
            sent.iter()
                .filter(|n| **n == notice_id(store.engine(), &a))
                .count(),
            1
        );
        let calls = rig.fake.calls.lock().clone();
        assert!(calls.iter().all(|(_, have)| *have == 0), "{calls:?}");
    }

    /// A restart finding another file at the recorded target (not our inode) never
    /// removes or adopts it: the row republishes from its partial under the next name.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn restart_never_adopts_a_foreign_target() {
        let rig = rig().await;
        let ex = rig.exchange().await;
        let a = rig.file(ex, "m", "report.pdf", b"ours").await;
        let store = &rig.svc.store;
        let partial = rig.svc.partial(&a);
        mkdirs(partial.parent().unwrap()).unwrap();
        std::fs::write(&partial, b"ours").unwrap();
        mkdirs(&rig.dir()).unwrap();
        let target = rig.dir().join("report.pdf");
        std::fs::write(&target, b"theirs").unwrap();
        let (dev, ino) = disk::inode_of(&partial).unwrap();
        store.file_to(&a, &["queued"], "receiving").await.unwrap();
        store
            .file_publishing(&a, &target.to_string_lossy(), dev, ino)
            .await
            .unwrap();
        recover(store, &rig.svc.root).await;
        assert_eq!(std::fs::read(&target).unwrap(), b"theirs");
        let row = rig.row(&a).await;
        assert_eq!(row.state, "saved");
        assert!(row.path.unwrap().ends_with("report (2).pdf"));
    }

    /// The sender's side: a GET serves the clone or the re-checked original; a replaced
    /// or deleted source is `410 changed` / `missing` and fails the offer; another
    /// contact gets `404`; the first terminal receipt wins; past the deadline, `410
    /// expired` and "expired without a receipt".
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn offers_recheck_and_settle_once() {
        let rig = rig().await;
        let store = &rig.svc.store;
        let src = rig.svc.root.join("src.txt");
        let offer = async |name: &str, bytes: &[u8], deadline: i64, state: &str| {
            let path = rig.svc.root.join(name);
            std::fs::write(&path, bytes).unwrap();
            let e = disk::egress_open(&path, &rig.svc.root).unwrap();
            let id = super::super::send::local_id();
            let row = FileRow {
                id: id.clone(),
                direction: "out".into(),
                session: rig.session.to_string(),
                exchange: uuid::Uuid::new_v4().to_string(),
                contact_id: "rec".into(),
                name: name.into(),
                media_type: "text/plain".into(),
                size: Some(bytes.len() as i64),
                blake3: Some(blake3::hash(bytes).to_hex().to_string()),
                source: path.to_string_lossy().into_owned(),
                dev: Some(e.dev as i64),
                inode: Some(e.inode as i64),
                state: state.into(),
                created_ms: now_ms(),
                deadline_ms: deadline,
                ..Default::default()
            };
            store.insert_contact_files(&[row]).await.unwrap();
            (id, path)
        };
        let later = now_ms() + 3_600_000;
        let (id, path) = offer("src.txt", b"twelve bytes", later, "queued").await;
        assert_eq!(src, path);
        assert!(matches!(
            rig.svc.open("other", &id).await,
            Err(OfferError::Unknown)
        ));
        let h = rig.svc.open("rec", &id).await.unwrap();
        assert_eq!(h.len, 12);
        assert_eq!(rig.row(&id).await.state, "sending");
        // A replaced original is not served.
        drop(h);
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, b"twelve bytes").unwrap();
        assert!(matches!(
            rig.svc.open("rec", &id).await,
            Err(OfferError::Gone(Gone::Changed))
        ));
        assert_eq!(rig.row(&id).await.state, "failed");
        let (gone, path) = offer("gone.txt", b"x", later, "queued").await;
        std::fs::remove_file(&path).unwrap();
        assert!(matches!(
            rig.svc.open("rec", &gone).await,
            Err(OfferError::Gone(Gone::Missing))
        ));
        let (ok, _) = offer("ok.txt", b"y", later, "queued").await;
        rig.svc
            .receipt("rec", &ok, Receipt::Received)
            .await
            .unwrap();
        let failed = Receipt::Failed {
            reason: "late".into(),
        };
        rig.svc.receipt("rec", &ok, failed).await.unwrap();
        assert_eq!(rig.row(&ok).await.state, "sent");
        let (old, _) = offer("old.txt", b"z", now_ms() - 1, "queued").await;
        assert!(matches!(
            rig.svc.open("rec", &old).await,
            Err(OfferError::Gone(Gone::Expired))
        ));
        assert_eq!(
            rig.row(&old).await.error.as_deref(),
            Some("expired before rec fetched it")
        );
        // Fetched, never confirmed, past the deadline.
        let (quiet, _) = offer("quiet.txt", b"q", now_ms() - 1, "sending").await;
        rig.svc.tick().await.unwrap();
        assert_eq!(
            rig.row(&quiet).await.error.as_deref(),
            Some("expired without a receipt")
        );
    }

    /// 199-Z1 SEC-A4: a receipt whose write fails is not acknowledged (retryable
    /// `Unavailable`, the offer still `sending`); the retry is. UI-3: what the route served
    /// is the `sending` row's `done` (the card's progress), throttled, and gone after `sent`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn receipts_ack_only_durably_and_serving_records_progress() {
        let rig = rig().await;
        let store = &rig.svc.store;
        let path = rig.svc.root.join("big.bin");
        std::fs::write(&path, vec![1u8; 1000]).unwrap();
        let e = disk::egress_open(&path, &rig.svc.root).unwrap();
        let id = super::super::send::local_id();
        store
            .insert_contact_files(&[FileRow {
                id: id.clone(),
                direction: "out".into(),
                session: rig.session.to_string(),
                exchange: uuid::Uuid::new_v4().to_string(),
                contact_id: "rec".into(),
                name: "big.bin".into(),
                media_type: "application/octet-stream".into(),
                size: Some(1000),
                blake3: Some(blake3::hash(&[1u8; 1000]).to_hex().to_string()),
                source: path.to_string_lossy().into_owned(),
                dev: Some(e.dev as i64),
                inode: Some(e.inode as i64),
                state: "queued".into(),
                created_ms: now_ms(),
                deadline_ms: now_ms() + 3_600_000,
                ..Default::default()
            }])
            .await
            .unwrap();
        drop(rig.svc.open("rec", &id).await.unwrap());
        rig.svc.served("rec", &id, 400);
        rig.until("done 400", async |r| r.row(&id).await.done == 400)
            .await;
        rig.svc.served("rec", &id, 500);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            rig.row(&id).await.done,
            400,
            "throttled: at most two a second"
        );
        tokio::time::sleep(PROGRESS_EVERY).await;
        rig.svc.served("rec", &id, 900);
        rig.until("done 900", async |r| r.row(&id).await.done == 900)
            .await;

        store
            .exec_for_test(
                "CREATE TRIGGER z1_fail BEFORE UPDATE OF state ON contact_files \
                 BEGIN SELECT RAISE(ABORT, 'injected'); END",
            )
            .await;
        assert!(matches!(
            rig.svc.receipt("rec", &id, Receipt::Received).await,
            Err(OfferError::Unavailable)
        ));
        assert_eq!(rig.row(&id).await.state, "sending", "nothing acknowledged");
        store.exec_for_test("DROP TRIGGER z1_fail").await;
        rig.svc
            .receipt("rec", &id, Receipt::Received)
            .await
            .unwrap();
        let row = rig.row(&id).await;
        assert_eq!((row.state.as_str(), row.done), ("sent", 1000));
        rig.svc.served("rec", &id, 950);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(
            rig.row(&id).await.done,
            1000,
            "a late count never rewinds a sent offer"
        );
    }

    /// 199-Z1 SEC-A5: step 6 (`file_published`) fails after the hard link: the published
    /// target is removed by inode with the partial, the file fails with a failed receipt,
    /// and nothing is left under `files/received/`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_failed_commit_after_the_link_leaves_no_published_file() {
        let rig = rig().await;
        rig.svc
            .store
            .exec_for_test(
                "CREATE TRIGGER z1_commit BEFORE UPDATE OF state ON contact_files \
                 WHEN NEW.state IN ('saved', 'received') \
                 BEGIN SELECT RAISE(ABORT, 'injected'); END",
            )
            .await;
        let ex = rig.exchange().await;
        let id = rig.file(ex, "m", "a5.txt", b"published then dropped").await;
        rig.until("failed", async |r| r.row(&id).await.state == "failed")
            .await;
        let row = rig.row(&id).await;
        assert!(
            row.error
                .as_deref()
                .unwrap_or_default()
                .starts_with("saving failed"),
            "{row:?}"
        );
        let target = PathBuf::from(row.path.expect("the link was recorded"));
        assert!(!target.exists(), "{} stranded", target.display());
        assert!(!rig.svc.partial(&id).exists());
        let left: Vec<_> = walk(&rig.svc.root.join("files/received"));
        assert!(left.is_empty(), "{left:?}");
        assert!(matches!(
            rig.fake.receipts.lock().last().map(|r| r.1.clone()),
            Some(Receipt::Failed { .. })
        ));
    }

    fn walk(dir: &Path) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        entries
            .flatten()
            .flat_map(|e| match e.path() {
                p if p.is_dir() => walk(&p),
                p => vec![p],
            })
            .collect()
    }
}
