// FROZEN: copied from jarvis 9d74d5a crates/jarvis-signal/src/presage.rs.
// Protocol code; edit only with a matching note in docs/design/09-signal.md §2.2.
//! `PresageTransport` — the presage-backed [`SignalTransport`]
//! (feature `presage`).
//!
//! Pure-Rust Signal protocol via [presage] (git dependency, pinned rev)
//! with its sqlite store at `<data_dir>/signal/presage.db`. Requires the
//! `curve25519-dalek` `[patch.crates-io]` entry in the workspace root
//! `Cargo.toml` (mandated by presage's README for the libsignal stack).
//!
//! # Architecture: dedicated actor thread
//!
//! presage's receive pipeline spawns `!Send` local tasks internally
//! (`tokio::task::spawn_local`), so its stream must be polled inside a
//! [`tokio::task::LocalSet`]. The transport therefore runs a dedicated OS
//! thread with a **multi-thread** runtime + `LocalSet` hosting the presage
//! [`Manager`] (multi-thread so libsignal websocket keepalives spawned via
//! `tokio::spawn` are not starved by LocalSet work), and the
//! (`Send + Sync`) [`PresageTransport`] handle talks to it over an
//! unbounded command channel:
//!
//! - [`SignalTransport::link_device`] starts
//!   `Manager::link_secondary_device` as a local task and replies with the
//!   provisioning URL as soon as it is issued; linking completes in the
//!   background when the user scans the QR code, after which
//!   `is_registered` flips to `true` and the next `incoming()`
//!   subscription starts receiving.
//! - [`SignalTransport::incoming`] subscribes a bounded envelope queue;
//!   the actor pumps `Manager::receive_messages` into it, downloading
//!   attachments and mapping `Content` → [`SignalEnvelope`]. When the
//!   presage stream ends (disconnect) or the account is not registered
//!   yet, the queue sender is dropped so the caller's stream ends cleanly
//!   — reconnect/backoff stays in `SignalChannel` per spec 06.
//! - Sends resolve the recipient to a [`ServiceId`] (explicit ACI UUID, or
//!   contact lookup by E.164 number in the presage store) and go through
//!   `Manager::send_message`.
//!
//! # Runtime setup (linking)
//!
//! A fresh install is unregistered: `SignalChannel::ensure_registered`
//! calls [`SignalTransport::link_device`], logs the returned URL, and
//! emits it via `Event::SubsystemStatus` detail. Render that URL as a QR
//! code and scan it from the primary device (Signal app → Settings →
//! Linked devices). Group messages are ignored (spec 06 maps one
//! conversation per contact).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use presage::libsignal_service::configuration::SignalServers;
use presage::libsignal_service::content::{Content, ContentBody};
use presage::libsignal_service::prelude::phonenumber::{Mode, PhoneNumber};
use presage::libsignal_service::protocol::{Aci, ServiceId};
use presage::libsignal_service::sender::AttachmentSpec;
use presage::manager::{Manager, Registered};
use presage::model::identity::OnNewIdentity;
use presage::model::messages::Received;
use presage::proto::{
    AttachmentPointer, DataMessage, ReceiptMessage, TypingMessage, attachment_pointer,
    data_message, receipt_message, typing_message,
};
use presage::store::ContentsStore;
use presage_store_sqlite::SqliteStore;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::error::{Result, SignalError};
use crate::transport::{
    IncomingAttachment, IncomingReaction, LinkAttempt, LinkUrl, Recipient, SignalEnvelope,
    SignalTransport,
};

/// Buffered envelopes per `incoming()` subscription before the pump
/// applies backpressure on the websocket.
const INCOMING_BUFFER: usize = 64;

/// Fallback MIME type for attachments without a declared content type.
const OCTET_STREAM: &str = "application/octet-stream";

/// Minimum interval between forced contact re-syncs triggered by failed
/// allowlist number resolution (matches the channel's per-number
/// resolution cooldown).
const FORCED_SYNC_INTERVAL: Duration = Duration::from_secs(300);

/// A presage manager for a linked, registered account.
type RegisteredManager = Manager<SqliteStore, Registered>;

/// Commands sent from the (Send + Sync) transport handle to the actor
/// thread hosting the presage manager.
enum Command {
    LinkDevice {
        device_name: String,
        reply: oneshot::Sender<Result<LinkUrl>>,
        /// Resolves when the provisioning attempt ends: `Ok(())` linked,
        /// `Err` timed out / failed (spec 06 linking state machine).
        done: oneshot::Sender<Result<()>>,
    },
    IsRegistered {
        reply: oneshot::Sender<Result<bool>>,
    },
    Account {
        reply: oneshot::Sender<Result<Option<Recipient>>>,
    },
    ResolveUuid {
        number: String,
        reply: oneshot::Sender<Result<Option<Uuid>>>,
    },
    Subscribe {
        sink: mpsc::Sender<SignalEnvelope>,
    },
    SendText {
        to: Recipient,
        text: String,
        reply: oneshot::Sender<Result<u64>>,
    },
    SendAttachment {
        to: Recipient,
        bytes: Vec<u8>,
        mime: String,
        file_name: Option<String>,
        caption: Option<String>,
        reply: oneshot::Sender<Result<()>>,
    },
    SendTyping {
        to: Recipient,
        active: bool,
        reply: oneshot::Sender<Result<()>>,
    },
    SendReceipt {
        to: Recipient,
        ts: u64,
        reply: oneshot::Sender<Result<()>>,
    },
    SendReaction {
        to: Recipient,
        emoji: String,
        target_author: Recipient,
        target_ts: u64,
        reply: oneshot::Sender<Result<()>>,
    },
}

/// Presage-backed transport storing protocol state at
/// `<data_dir>/signal/`.
///
/// See the module docs for the actor architecture and the runtime linking
/// flow. The handle is cheap to clone; all clones talk to the same actor.
#[derive(Debug, Clone)]
pub struct PresageTransport {
    store_dir: PathBuf,
    commands: mpsc::UnboundedSender<Command>,
}

impl PresageTransport {
    /// Creates the transport with its store rooted at
    /// `<data_dir>/signal/` and spawns the presage actor thread.
    #[must_use]
    pub fn new(data_dir: &Path) -> Self {
        let store_dir = data_dir.join("signal");
        let (commands, rx) = mpsc::unbounded_channel();
        spawn_actor_thread(store_dir.clone(), rx);
        Self {
            store_dir,
            commands,
        }
    }

    /// Directory the protocol store lives in.
    #[must_use]
    pub fn store_dir(&self) -> &Path {
        &self.store_dir
    }

    /// Sends a command to the actor and awaits its reply.
    async fn request<T>(
        &self,
        make: impl FnOnce(oneshot::Sender<Result<T>>) -> Command,
    ) -> Result<T> {
        let (tx, rx) = oneshot::channel();
        self.commands.send(make(tx)).map_err(|_| worker_gone())?;
        rx.await.map_err(|_| worker_gone())?
    }
}

#[async_trait]
impl SignalTransport for PresageTransport {
    async fn link_device(&self, device_name: &str) -> Result<LinkAttempt> {
        let device_name = device_name.to_owned();
        let (done_tx, done_rx) = oneshot::channel();
        let url = self
            .request(|reply| Command::LinkDevice {
                device_name,
                reply,
                done: done_tx,
            })
            .await?;
        Ok(LinkAttempt {
            url,
            done: Box::pin(async move { done_rx.await.map_err(|_| worker_gone())? }),
        })
    }

    async fn is_registered(&self) -> Result<bool> {
        self.request(|reply| Command::IsRegistered { reply }).await
    }

    async fn account(&self) -> Result<Option<Recipient>> {
        self.request(|reply| Command::Account { reply }).await
    }

    async fn resolve_uuid(&self, number: &str) -> Result<Option<Uuid>> {
        let number = number.to_owned();
        self.request(|reply| Command::ResolveUuid { number, reply })
            .await
    }

    fn incoming(&self) -> BoxStream<'static, SignalEnvelope> {
        let (sink, rx) = mpsc::channel(INCOMING_BUFFER);
        if self.commands.send(Command::Subscribe { sink }).is_err() {
            tracing::error!("presage worker is not running; incoming stream ends immediately");
            return futures::stream::empty().boxed();
        }
        futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|envelope| (envelope, rx))
        })
        .boxed()
    }

    async fn send_text(&self, to: &Recipient, text: &str) -> Result<u64> {
        let (to, text) = (to.clone(), text.to_owned());
        self.request(|reply| Command::SendText { to, text, reply })
            .await
    }

    async fn send_attachment(
        &self,
        to: &Recipient,
        bytes: &[u8],
        mime: &str,
        file_name: Option<&str>,
        caption: Option<&str>,
    ) -> Result<()> {
        let (to, bytes, mime) = (to.clone(), bytes.to_vec(), mime.to_owned());
        let file_name = file_name.map(str::to_owned);
        let caption = caption.map(str::to_owned);
        self.request(|reply| Command::SendAttachment {
            to,
            bytes,
            mime,
            file_name,
            caption,
            reply,
        })
        .await
    }

    async fn send_typing(&self, to: &Recipient, active: bool) -> Result<()> {
        let to = to.clone();
        self.request(|reply| Command::SendTyping { to, active, reply })
            .await
    }

    async fn send_receipt(&self, to: &Recipient, ts: u64) -> Result<()> {
        let to = to.clone();
        self.request(|reply| Command::SendReceipt { to, ts, reply })
            .await
    }

    async fn send_reaction(
        &self,
        to: &Recipient,
        emoji: &str,
        target_author: &Recipient,
        target_ts: u64,
    ) -> Result<()> {
        let (to, emoji, target_author) = (to.clone(), emoji.to_owned(), target_author.clone());
        self.request(|reply| Command::SendReaction {
            to,
            emoji,
            target_author,
            target_ts,
            reply,
        })
        .await
    }
}

/// Error used when the actor thread is gone (spawn failure or shutdown).
fn worker_gone() -> SignalError {
    SignalError::Transport("presage worker is not running".into())
}

/// Wraps a presage-side failure into a transport error with context.
fn transport_err(context: &str, err: impl std::fmt::Display) -> SignalError {
    SignalError::Transport(format!("{context}: {err}"))
}

/// Current time as Signal's millisecond timestamp.
fn now_timestamp_millis() -> u64 {
    u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or_default()
}

/// Formats a phone number in E.164 (the stable identity `SignalChannel`
/// matches config numbers against).
fn e164(number: &PhoneNumber) -> String {
    number.format().mode(Mode::E164).to_string()
}

/// Spawns the dedicated actor thread: multi-thread runtime + `LocalSet`.
///
/// presage's receive pipeline spawns `!Send` local tasks, so the manager
/// lives on a [`LocalSet`]. The runtime itself is multi-thread so that
/// libsignal's websocket keepalive process (`tokio::spawn` from
/// `PushService::ws`) runs on a worker thread and is **not starved** when
/// the LocalSet is busy decrypting/downloading. A current-thread runtime
/// previously let Signal close the socket with `1001 Connection Idle
/// Timeout` because keepalives never fired under load.
fn spawn_actor_thread(store_dir: PathBuf, commands: mpsc::UnboundedReceiver<Command>) {
    let spawned = std::thread::Builder::new()
        .name("signal-presage".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("signal-presage-worker")
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(err) => {
                    tracing::error!(%err, "failed to build presage runtime");
                    return;
                }
            };
            let local = tokio::task::LocalSet::new();
            local.block_on(&runtime, run_actor(store_dir, commands));
        });
    if let Err(err) = spawned {
        tracing::error!(%err, "failed to spawn presage actor thread");
    }
}

/// Opens (creating if needed) the sqlite protocol store.
async fn open_store(store_dir: &Path) -> Result<SqliteStore> {
    tokio::fs::create_dir_all(store_dir)
        .await
        .map_err(|e| transport_err("creating signal store dir", e))?;
    let db_path = store_dir.join("presage.db");
    SqliteStore::open(&db_path.to_string_lossy(), OnNewIdentity::Trust)
        .await
        .map_err(|e| transport_err("opening presage sqlite store", e))
}

/// Actor entry point: owns the store and the registered manager, processes
/// [`Command`]s until every transport handle is dropped.
async fn run_actor(store_dir: PathBuf, mut commands: mpsc::UnboundedReceiver<Command>) {
    let store = match open_store(&store_dir).await {
        Ok(store) => store,
        Err(err) => {
            tracing::error!(%err, "presage store unavailable; signal transport degraded");
            while let Some(cmd) = commands.recv().await {
                fail_command(cmd, &err.to_string());
            }
            return;
        }
    };

    let manager = match Manager::load_registered(store.clone()).await {
        Ok(manager) => {
            tracing::info!("signal account loaded from presage store");
            // An empty contact store can't resolve any sender numbers
            // (allowlisting then drops everything): ask the primary
            // device for a sync. Best-effort; contacts arrive through
            // the receive stream once it is up.
            tokio::task::spawn_local(request_contact_sync(store.clone(), manager.clone(), false));
            Some(manager)
        }
        Err(err) => {
            tracing::info!(%err, "signal account not linked yet; provisioning required");
            None
        }
    };

    let (registered_tx, mut registered_rx) = mpsc::unbounded_channel();
    let mut actor = Actor {
        store,
        manager,
        registered_tx,
        pump: None,
        last_forced_sync: None,
    };

    loop {
        tokio::select! {
            cmd = commands.recv() => {
                let Some(cmd) = cmd else { break };
                actor.handle(cmd).await;
            }
            linked = registered_rx.recv() => {
                if let Some(manager) = linked {
                    tracing::info!("signal device linked; presage transport ready");
                    // Fresh link: the contact store is empty until the
                    // primary device sends a sync — request one now.
                    tokio::task::spawn_local(request_contact_sync(
                        actor.store.clone(),
                        manager.clone(),
                        true,
                    ));
                    actor.manager = Some(manager);
                }
            }
        }
    }

    if let Some(pump) = actor.pump.take() {
        pump.abort();
    }
    tracing::debug!("presage actor stopped (all transport handles dropped)");
}

/// Asks the primary device for a contact sync so E.164 ↔ ACI resolution
/// works (sealed-sender allowlisting depends on it). With `force` false
/// the request is skipped when the store already has contacts. The
/// synced contacts arrive through the receive stream and are stored by
/// presage automatically; this only *requests* the sync (best-effort).
async fn request_contact_sync(store: SqliteStore, manager: RegisteredManager, force: bool) {
    if !force {
        match store.contacts().await {
            Ok(mut contacts) => {
                if contacts.next().is_some() {
                    return; // already synced
                }
            }
            Err(err) => {
                tracing::debug!(%err, "contact store check failed; requesting sync anyway");
            }
        }
    }
    let mut manager = manager;
    match manager.request_contacts().await {
        Ok(()) => tracing::info!("requested signal contact sync from primary device"),
        Err(err) => tracing::warn!(%err, "signal contact sync request failed"),
    }
}

/// Replies to a command with a transport error (store unavailable path).
fn fail_command(cmd: Command, detail: &str) {
    let err = || SignalError::Transport(format!("presage store unavailable: {detail}"));
    match cmd {
        Command::LinkDevice { reply, .. } => drop(reply.send(Err(err()))),
        Command::IsRegistered { reply } => drop(reply.send(Err(err()))),
        Command::Account { reply } => drop(reply.send(Err(err()))),
        Command::ResolveUuid { reply, .. } => drop(reply.send(Err(err()))),
        Command::Subscribe { sink } => drop(sink),
        Command::SendText { reply, .. } => drop(reply.send(Err(err()))),
        Command::SendAttachment { reply, .. }
        | Command::SendTyping { reply, .. }
        | Command::SendReceipt { reply, .. }
        | Command::SendReaction { reply, .. } => drop(reply.send(Err(err()))),
    }
}

/// Actor state living on the presage thread.
struct Actor {
    store: SqliteStore,
    manager: Option<RegisteredManager>,
    /// Delivers the manager produced by a background linking task.
    registered_tx: mpsc::UnboundedSender<RegisteredManager>,
    /// The active incoming pump, aborted on re-subscription so stale
    /// websockets don't linger.
    pump: Option<tokio::task::JoinHandle<()>>,
    /// Last forced contact re-sync triggered by a failed allowlist
    /// number resolution (rate limit, see [`FORCED_SYNC_INTERVAL`]).
    last_forced_sync: Option<Instant>,
}

impl Actor {
    async fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::IsRegistered { reply } => {
                let _ = reply.send(Ok(self.manager.is_some()));
            }
            Command::Account { reply } => {
                let account = self.manager.as_ref().map(|manager| {
                    let data = manager.registration_data();
                    Recipient {
                        number: e164(&data.phone_number),
                        uuid: Some(data.service_ids.aci),
                    }
                });
                let _ = reply.send(Ok(account));
            }
            Command::ResolveUuid { number, reply } => {
                let _ = reply.send(self.resolve_uuid(&number).await);
            }
            Command::LinkDevice {
                device_name,
                reply,
                done,
            } => self.link_device(device_name, reply, done),
            Command::Subscribe { sink } => self.subscribe(sink),
            Command::SendText { to, text, reply } => {
                let _ = reply.send(self.send_text(&to, &text).await);
            }
            Command::SendAttachment {
                to,
                bytes,
                mime,
                file_name,
                caption,
                reply,
            } => {
                let _ = reply.send(
                    self.send_attachment(&to, bytes, &mime, file_name, caption)
                        .await,
                );
            }
            Command::SendTyping { to, active, reply } => {
                let _ = reply.send(self.send_typing(&to, active).await);
            }
            Command::SendReceipt { to, ts, reply } => {
                let _ = reply.send(self.send_receipt(&to, ts).await);
            }
            Command::SendReaction {
                to,
                emoji,
                target_author,
                target_ts,
                reply,
            } => {
                let _ = reply.send(
                    self.send_reaction(&to, &emoji, &target_author, target_ts)
                        .await,
                );
            }
        }
    }

    /// Starts one linked-device provisioning attempt. Replies with the
    /// URL as soon as presage issues it; `done` resolves when the attempt
    /// ends — `Ok(())` once the user scanned the QR and linking completed
    /// (the manager arrives via `registered_tx`), `Err` when the
    /// provisioning websocket timed out or failed (the channel then
    /// chains a fresh attempt, spec 06 linking state machine).
    fn link_device(
        &self,
        device_name: String,
        reply: oneshot::Sender<Result<LinkUrl>>,
        done: oneshot::Sender<Result<()>>,
    ) {
        let (url_tx, url_rx) = futures::channel::oneshot::channel();
        let store = self.store.clone();
        let registered_tx = self.registered_tx.clone();
        tokio::task::spawn_local(async move {
            match Manager::link_secondary_device(
                store,
                SignalServers::Production,
                device_name,
                url_tx,
            )
            .await
            {
                Ok(manager) => {
                    let _ = registered_tx.send(manager);
                    let _ = done.send(Ok(()));
                }
                Err(err) => {
                    tracing::warn!(%err, "signal provisioning attempt ended without a link");
                    let _ = done.send(Err(transport_err("provisioning attempt", err)));
                }
            }
        });
        tokio::task::spawn_local(async move {
            let url = match url_rx.await {
                Ok(url) => Ok(LinkUrl(url.to_string())),
                Err(_) => Err(SignalError::Transport(
                    "provisioning ended before a link URL was issued".into(),
                )),
            };
            let _ = reply.send(url);
        });
    }

    /// Starts pumping incoming messages into `sink`. When the account is
    /// not registered the sink is dropped immediately so the subscriber's
    /// stream ends and `SignalChannel` retries with backoff.
    fn subscribe(&mut self, sink: mpsc::Sender<SignalEnvelope>) {
        let Some(manager) = self.manager.clone() else {
            tracing::debug!("incoming subscription while unregistered; stream ends immediately");
            return;
        };
        if let Some(previous) = self.pump.take() {
            previous.abort();
        }
        self.pump = Some(tokio::task::spawn_local(pump_incoming(manager, sink)));
    }

    /// The registered manager, or [`SignalError::NotRegistered`].
    fn manager_mut(&mut self) -> Result<&mut RegisteredManager> {
        self.manager.as_mut().ok_or(SignalError::NotRegistered)
    }

    async fn send_text(&mut self, to: &Recipient, text: &str) -> Result<u64> {
        let manager = self.manager_mut()?;
        let recipient = resolve_recipient(manager, to).await?;
        let message = DataMessage {
            body: Some(text.to_owned()),
            ..Default::default()
        };
        // wth 09 §2.2 edit 2: the sent timestamp is what a reaction targets.
        let timestamp = now_timestamp_millis();
        manager
            .send_message(recipient, message, timestamp)
            .await
            .map_err(|e| transport_err("sending text", e))?;
        Ok(timestamp)
    }

    async fn send_attachment(
        &mut self,
        to: &Recipient,
        bytes: Vec<u8>,
        mime: &str,
        file_name: Option<String>,
        caption: Option<String>,
    ) -> Result<()> {
        let manager = self.manager_mut()?;
        let recipient = resolve_recipient(manager, to).await?;
        let spec = AttachmentSpec {
            content_type: mime.to_owned(),
            length: bytes.len(),
            file_name,
            preview: None,
            voice_note: Some(mime.starts_with("audio/")),
            borderless: None,
            width: None,
            height: None,
            caption: caption.clone(),
            blur_hash: None,
        };
        let pointer = manager
            .upload_attachment(spec, bytes)
            .await
            .map_err(|e| transport_err("uploading attachment", e))?
            .map_err(|e| transport_err("uploading attachment", e))?;
        let message = DataMessage {
            body: caption,
            attachments: vec![pointer],
            ..Default::default()
        };
        manager
            .send_message(recipient, message, now_timestamp_millis())
            .await
            .map_err(|e| transport_err("sending attachment", e))
    }

    async fn send_typing(&mut self, to: &Recipient, active: bool) -> Result<()> {
        let manager = self.manager_mut()?;
        let recipient = resolve_recipient(manager, to).await?;
        let action = if active {
            typing_message::Action::Started
        } else {
            typing_message::Action::Stopped
        };
        let message = TypingMessage {
            timestamp: Some(now_timestamp_millis()),
            action: Some(action.into()),
            group_id: None,
        };
        manager
            .send_message(recipient, message, now_timestamp_millis())
            .await
            .map_err(|e| transport_err("sending typing indicator", e))
    }

    /// Resolves an E.164 number to the account's ACI UUID. Used by the
    /// channel to recognize allowlisted senders on sealed-sender
    /// envelopes that carry only an ACI. Resolution order (mirrors
    /// signal-cli's recipient resolution):
    ///
    /// 1. the linked account itself (note-to-self),
    /// 2. the contact store (synced from the primary device; bare stubs
    ///    persisted by an earlier CDSI bug are ignored, see
    ///    [`trusted_contact`]),
    /// 3. CDSI contact discovery (ACI results only; bare lookups
    ///    usually yield PNIs, which are rejected).
    ///
    /// When every step fails, a contact re-sync is requested from the
    /// primary device (rate-limited) so a number saved as a contact
    /// there eventually becomes resolvable here.
    async fn resolve_uuid(&mut self, number: &str) -> Result<Option<Uuid>> {
        if let Some(manager) = &self.manager {
            let data = manager.registration_data();
            if e164(&data.phone_number) == number {
                return Ok(Some(data.service_ids.aci));
            }
        }
        let contacts = self
            .store
            .contacts()
            .await
            .map_err(|e| transport_err("listing contacts", e))?;
        for contact in contacts.flatten() {
            if contact
                .phone_number
                .as_ref()
                .is_some_and(|n| e164(n) == number)
            {
                if trusted_contact(&contact) {
                    return Ok(Some(contact.uuid));
                }
                tracing::warn!(
                    number = %number,
                    uuid = %contact.uuid,
                    "ignoring bare stub contact (likely a PNI persisted by an \
                     earlier CDSI bug); re-resolving"
                );
            }
        }
        match self.resolve_uuid_via_cdsi(number).await {
            Ok(None) => {
                self.request_contact_resync();
                Ok(None)
            }
            resolved => resolved,
        }
    }

    /// Resolves a number through Signal's CDSI directory service.
    ///
    /// CDSI only returns an account's **ACI** when the request includes
    /// that account's unidentified-access key (derived from their
    /// profile key). presage sends a bare lookup, so in practice CDSI
    /// answers with the account's **PNI** instead. A PNI is useless for
    /// allowlisting: incoming sealed-sender envelopes identify the
    /// sender by ACI, so a PNI can never match. An earlier version of
    /// this code treated the PNI's raw UUID as the ACI and persisted it
    /// as a contact, permanently poisoning resolution (see
    /// [`trusted_contact`] for the store repair). PNI results are now
    /// rejected; the E.164 → ACI mapping for such numbers arrives via
    /// contact sync from the primary device instead.
    async fn resolve_uuid_via_cdsi(&mut self, number: &str) -> Result<Option<Uuid>> {
        let Some(manager) = self.manager.as_mut() else {
            return Ok(None);
        };
        tracing::info!(number = %number, "looking up signal number via CDSI contact discovery");
        let discovered = manager
            .discover_contacts_by_phone_number([number])
            .await
            .map_err(|e| transport_err("cdsi contact discovery", e))?;
        for (phone, service_id) in discovered {
            let Some(service_id) = service_id else {
                continue;
            };
            if e164(&phone) != number {
                continue;
            }
            let Some(aci) = cdsi_aci(&service_id) else {
                tracing::warn!(
                    number = %number,
                    service_id = %service_id.service_id_string(),
                    "CDSI returned a PNI (not an ACI) for this number; sealed-sender \
                     envelopes carry ACIs, so this can't be used for allowlisting — \
                     waiting for a contact sync from the primary device instead \
                     (make sure the number is saved as a contact there)"
                );
                continue;
            };
            return Ok(Some(aci));
        }
        Ok(None)
    }

    /// Requests a contact re-sync from the primary device after a failed
    /// allowlist number resolution, at most once per
    /// [`FORCED_SYNC_INTERVAL`]. Synced contacts are the only reliable
    /// E.164 → ACI source now that PNI-only CDSI answers are rejected.
    fn request_contact_resync(&mut self) {
        let Some(manager) = self.manager.clone() else {
            return;
        };
        if self
            .last_forced_sync
            .is_some_and(|at| at.elapsed() < FORCED_SYNC_INTERVAL)
        {
            return;
        }
        self.last_forced_sync = Some(Instant::now());
        tracing::info!(
            "allowlisted signal number unresolved; requesting contact re-sync from primary device"
        );
        tokio::task::spawn_local(request_contact_sync(self.store.clone(), manager, true));
    }

    async fn send_receipt(&mut self, to: &Recipient, ts: u64) -> Result<()> {
        let manager = self.manager_mut()?;
        let recipient = resolve_recipient(manager, to).await?;
        let message = ReceiptMessage {
            r#type: Some(receipt_message::Type::Read.into()),
            timestamp: vec![ts],
        };
        manager
            .send_message(recipient, message, now_timestamp_millis())
            .await
            .map_err(|e| transport_err("sending read receipt", e))
    }

    async fn send_reaction(
        &mut self,
        to: &Recipient,
        emoji: &str,
        target_author: &Recipient,
        target_ts: u64,
    ) -> Result<()> {
        let manager = self.manager_mut()?;
        let recipient = resolve_recipient(manager, to).await?;
        // Prefer the envelope's ACI; otherwise resolve via contact store.
        let author = match target_author.uuid {
            Some(u) => ServiceId::Aci(Aci::from(u)),
            None => resolve_recipient(manager, target_author).await?,
        };
        let message = DataMessage {
            reaction: Some(data_message::Reaction {
                emoji: Some(emoji.to_owned()),
                remove: Some(false),
                target_author_aci: Some(author.service_id_string()),
                target_sent_timestamp: Some(target_ts),
                ..Default::default()
            }),
            ..Default::default()
        };
        manager
            .send_message(recipient, message, now_timestamp_millis())
            .await
            .map_err(|e| transport_err("sending reaction", e))
    }
}

/// Resolves a [`Recipient`] to a Signal [`ServiceId`]: explicit ACI UUID
/// when known, otherwise a contact lookup by E.164 number in the presage
/// store (contacts are synced from the primary device after linking).
async fn resolve_recipient(manager: &RegisteredManager, to: &Recipient) -> Result<ServiceId> {
    if let Some(uuid) = to.uuid {
        return Ok(Aci::from(uuid).into());
    }
    let contacts = manager
        .store()
        .contacts()
        .await
        .map_err(|e| transport_err("listing contacts", e))?;
    for contact in contacts.flatten() {
        if contact
            .phone_number
            .as_ref()
            .is_some_and(|number| e164(number) == to.number)
            && trusted_contact(&contact)
        {
            return Ok(Aci::from(contact.uuid).into());
        }
    }
    Err(SignalError::NotFound(format!(
        "no synced signal contact for {}; wait for contact sync or message the account first",
        to.number
    )))
}

/// Consumes the presage receive stream, forwarding mapped envelopes into
/// `sink` until the stream ends (disconnect) or the subscriber goes away.
/// Dropping `sink` ends the subscriber's stream — reconnect/backoff is
/// `SignalChannel`'s job.
async fn pump_incoming(mut manager: RegisteredManager, sink: mpsc::Sender<SignalEnvelope>) {
    let messages = match manager.receive_messages().await {
        Ok(messages) => messages,
        Err(err) => {
            tracing::warn!(%err, "failed to open incoming signal stream");
            return;
        }
    };
    futures::pin_mut!(messages);
    while let Some(received) = messages.next().await {
        match received {
            Received::QueueEmpty | Received::Contacts => {}
            Received::Content(content) => {
                if let Some(envelope) = content_to_envelope(&manager, *content).await
                    && sink.send(envelope).await.is_err()
                {
                    tracing::debug!("incoming subscriber dropped; stopping pump");
                    return;
                }
            }
        }
    }
    tracing::warn!("incoming signal stream ended; supervisor will reconnect");
}

/// Maps a decrypted presage [`Content`] to a [`SignalEnvelope`]:
/// direct-contact data messages only (group and non-data messages are
/// skipped), attachments downloaded to bytes.
async fn content_to_envelope(
    manager: &RegisteredManager,
    content: Content,
) -> Option<SignalEnvelope> {
    let sender_id = content.metadata.sender;
    let ContentBody::DataMessage(data) = content.body else {
        return None;
    };
    if data.group_v2.is_some() {
        tracing::debug!("skipping group message (spec 06 maps contacts only)");
        return None;
    }
    // wth 09 §2.2 edit 1: a reaction to one of our messages becomes an
    // envelope of its own (the channel maps it to an approval decision).
    if let Some(reaction) = data.reaction.as_ref().filter(|r| r.emoji.is_some()) {
        let number = sender_number(manager, &sender_id).await;
        let timestamp = data.timestamp.unwrap_or_else(|| {
            u64::try_from(content.metadata.timestamp.timestamp_millis()).unwrap_or_default()
        });
        return Some(SignalEnvelope {
            sender: Recipient {
                number,
                uuid: Some(sender_id.raw_uuid()),
            },
            timestamp,
            text: None,
            quote_text: None,
            attachments: Vec::new(),
            reaction: Some(IncomingReaction {
                emoji: reaction.emoji.clone().unwrap_or_default(),
                target_timestamp: reaction.target_sent_timestamp.unwrap_or_default(),
                remove: reaction.remove.unwrap_or(false),
            }),
        });
    }
    if data.body.is_none() && data.attachments.is_empty() {
        // Profile-key updates, expire-timer changes, ...
        return None;
    }

    let mut attachments = Vec::with_capacity(data.attachments.len());
    for pointer in &data.attachments {
        match manager.get_attachment(pointer).await {
            Ok(bytes) => attachments.push(map_attachment(pointer, bytes)),
            Err(err) => tracing::warn!(%err, "failed to download signal attachment; skipping"),
        }
    }
    if data.body.is_none() && attachments.is_empty() {
        return None;
    }

    let number = sender_number(manager, &sender_id).await;
    let timestamp = data.timestamp.unwrap_or_else(|| {
        u64::try_from(content.metadata.timestamp.timestamp_millis()).unwrap_or_default()
    });
    // Swipe-to-reply: quote carries the parent message text (and optional
    // attachment metadata we ignore). Empty / whitespace-only is dropped.
    let quote_text = data
        .quote
        .as_ref()
        .and_then(|q| q.text.as_ref())
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .map(str::to_owned);
    Some(SignalEnvelope {
        sender: Recipient {
            number,
            uuid: Some(sender_id.raw_uuid()),
        },
        timestamp,
        text: data.body,
        quote_text,
        attachments,
        reaction: None,
    })
}

/// Maps a downloaded attachment pointer + bytes to the transport type.
fn map_attachment(pointer: &AttachmentPointer, bytes: Vec<u8>) -> IncomingAttachment {
    let voice_note = pointer.flags() & attachment_pointer::Flags::VoiceMessage as u32 != 0;
    IncomingAttachment {
        bytes,
        mime: pointer
            .content_type
            .clone()
            .unwrap_or_else(|| OCTET_STREAM.to_owned()),
        file_name: pointer.file_name.clone(),
        voice_note,
        duration_ms: None,
    }
}

/// Extracts the ACI from a CDSI-returned [`ServiceId`], rejecting PNIs.
///
/// Incoming sealed-sender envelopes identify the sender by ACI, so a
/// PNI can never match the allowlist directory — and treating a PNI's
/// raw UUID as an ACI (as an earlier version did) silently breaks
/// allowlisting for every number CDSI answers with a PNI (which is all
/// of them for bare lookups that carry no unidentified-access keys).
fn cdsi_aci(service_id: &ServiceId) -> Option<Uuid> {
    match service_id {
        ServiceId::Aci(_) => Some(service_id.raw_uuid()),
        ServiceId::Pni(_) => None,
    }
}

/// Whether a stored contact is trustworthy as an E.164 → ACI mapping.
///
/// An earlier CDSI code path persisted bare stubs (empty name, empty
/// profile key) whose `uuid` is actually a **PNI**, poisoning every
/// later lookup for that number. Contacts synced from the primary
/// device carry a name and/or profile key; bare stubs are ignored so a
/// real synced contact (or a fresh CDSI ACI answer) can win instead.
fn trusted_contact(contact: &presage::model::contacts::Contact) -> bool {
    !contact.name.is_empty() || !contact.profile_key.is_empty()
}

/// Best-effort E.164 number for a sender: the linked account itself
/// (note-to-self / sync), then contact lookup in the presage store
/// (synced from the primary device). Sealed-sender envelopes only carry
/// the sender's ACI, so unknown contacts yield an empty number — the
/// channel then falls back to matching the ACI against its resolved
/// allowlist ACI directory before enforcing the allowlist.
async fn sender_number(manager: &RegisteredManager, id: &ServiceId) -> String {
    let data = manager.registration_data();
    if id.raw_uuid() == data.service_ids.aci {
        return e164(&data.phone_number);
    }
    match manager.store().contact_by_id(id).await {
        Ok(Some(contact)) => contact.phone_number.as_ref().map(e164).unwrap_or_default(),
        Ok(None) => {
            tracing::debug!(
                aci = %id.raw_uuid(),
                "no synced signal contact for sender; forwarding UUID-only envelope"
            );
            String::new()
        }
        Err(err) => {
            tracing::debug!(%err, aci = %id.raw_uuid(), "signal contact lookup failed");
            String::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transport() -> (tempfile::TempDir, PresageTransport) {
        let dir = tempfile::tempdir().expect("tempdir");
        let transport = PresageTransport::new(dir.path());
        (dir, transport)
    }

    #[tokio::test]
    async fn fresh_store_is_unregistered() {
        let (_dir, transport) = transport();
        assert_eq!(transport.store_dir().file_name().unwrap(), "signal");
        assert!(!transport.is_registered().await.expect("is_registered"));
        assert!(transport.store_dir().join("presage.db").exists());
    }

    #[tokio::test]
    async fn sends_fail_cleanly_when_unregistered() {
        let (_dir, transport) = transport();
        let to = Recipient::new("+15550001111");
        assert!(matches!(
            transport.send_text(&to, "hi").await,
            Err(SignalError::NotRegistered)
        ));
        assert!(matches!(
            transport
                .send_attachment(&to, b"x", "image/png", None, None)
                .await,
            Err(SignalError::NotRegistered)
        ));
        assert!(matches!(
            transport.send_typing(&to, true).await,
            Err(SignalError::NotRegistered)
        ));
        assert!(matches!(
            transport.send_receipt(&to, 1).await,
            Err(SignalError::NotRegistered)
        ));
    }

    #[tokio::test]
    async fn incoming_ends_cleanly_when_unregistered() {
        let (_dir, transport) = transport();
        // Force store init to finish first so the subscribe isn't queued.
        let _ = transport.is_registered().await.expect("is_registered");
        let mut stream = transport.incoming();
        assert!(stream.next().await.is_none());
    }

    #[test]
    fn attachment_mapping_flags_voice_notes() {
        let pointer = AttachmentPointer {
            content_type: Some("audio/aac".into()),
            file_name: Some("note.aac".into()),
            flags: Some(attachment_pointer::Flags::VoiceMessage as u32),
            ..Default::default()
        };
        let mapped = map_attachment(&pointer, vec![1, 2, 3]);
        assert!(mapped.voice_note);
        assert_eq!(mapped.mime, "audio/aac");
        assert_eq!(mapped.file_name.as_deref(), Some("note.aac"));
        assert_eq!(mapped.bytes, vec![1, 2, 3]);

        let plain = AttachmentPointer::default();
        let mapped = map_attachment(&plain, Vec::new());
        assert!(!mapped.voice_note);
        assert_eq!(mapped.mime, OCTET_STREAM);
    }

    #[test]
    fn cdsi_pni_results_are_rejected() {
        use presage::libsignal_service::protocol::Pni;
        let uuid = Uuid::new_v4();
        let aci: ServiceId = Aci::from(uuid).into();
        assert_eq!(cdsi_aci(&aci), Some(uuid));
        // Sealed-sender envelopes carry ACIs; a PNI can never match the
        // allowlist directory and must not masquerade as one.
        let pni: ServiceId = Pni::from(uuid).into();
        assert_eq!(cdsi_aci(&pni), None);
    }

    #[test]
    fn bare_stub_contacts_are_untrusted() {
        fn contact(name: &str, profile_key: Vec<u8>) -> presage::model::contacts::Contact {
            presage::model::contacts::Contact {
                uuid: Uuid::new_v4(),
                phone_number: None,
                name: name.into(),
                verified: Default::default(),
                profile_key,
                expire_timer: 0,
                expire_timer_version: 2,
                inbox_position: 0,
                avatar: None,
            }
        }
        // Shape of the poisoned rows persisted by the old CDSI path.
        assert!(!trusted_contact(&contact("", Vec::new())));
        assert!(trusted_contact(&contact("Alice", Vec::new())));
        assert!(trusted_contact(&contact("", vec![1, 2, 3])));
    }
}
