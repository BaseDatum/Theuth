//! The channel: identity, the hub-lease-gated poller, the bus subscriber and
//! the control [`Handle`] (design doc 98 §3, §7, §11).
//!
//! NEEDS(ROUTE): stage 1 is the skeleton. [`spawn`] builds the [`Api`], the
//! [`Channel`], subscribes the [`Bus`] to the eight event types into one
//! ordered mpsc, runs the identity / lease / inbound / event tasks and keeps
//! [`Handle`] real; `handle_inbound` and `handle_event` log and drop. ROUTE
//! fills in trust, routing, workers, approvals, asks, `show` and
//! `message_user`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use agent::AgentEvent;
use artifact::ArtifactShown;
use kameo::Actor;
use kameo::actor::Spawn;
use kameo::message::{Context, Message};
use parking_lot::{Mutex, MutexGuard};
use runtime::Runtime;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tools::{ApprovalRequested, ApprovalResolved, AskRequested, AskResolved};

use crate::api::{Api, MsgRef, Sender, is_not_modified};
use crate::poll::{self, Incoming, PollCtx, RecentKeys};
use crate::port::{LivePort, Port};
use crate::threads::{ThreadKey, Threads};
use crate::worker::WorkerMsg;
use crate::{
    BotIdentity, PAIR_TTL, Pairing, PairingLink, TelegramConfig, TelegramEvent, TelegramStatus,
    commands, mint_code, setup,
};

/// `<data_dir>/telegram/threads.json`.
#[must_use]
pub fn threads_path(data_dir: &Path) -> PathBuf {
    data_dir.join("telegram").join("threads.json")
}

/// Bus events the channel renders, in bus order.
#[allow(dead_code)] // NEEDS(ROUTE): every variant is routed in stage 2.
enum Event {
    Agent(AgentEvent),
    Requested(ApprovalRequested),
    Resolved(ApprovalResolved),
    Ask(AskRequested),
    AskDone(AskResolved),
    Artifact(ArtifactShown),
    Secret(secrets::SecretsEvent),
    /// 67 §4.4: the hub lease moved.
    Lease(mesh::LeaseState),
}

/// Bus subscriber: forwards into the ordered event task.
#[derive(Actor)]
struct Bus {
    tx: mpsc::UnboundedSender<Event>,
}

macro_rules! forward {
    ($ty:ty => $variant:ident) => {
        impl Message<$ty> for Bus {
            type Reply = ();
            async fn handle(&mut self, m: $ty, _: &mut Context<Self, ()>) {
                let _ = self.tx.send(Event::$variant(m));
            }
        }
    };
}

forward!(AgentEvent => Agent);
forward!(ApprovalRequested => Requested);
forward!(ApprovalResolved => Resolved);
forward!(AskRequested => Ask);
forward!(AskResolved => AskDone);
forward!(ArtifactShown => Artifact);
forward!(secrets::SecretsEvent => Secret);

impl Message<mesh::MeshEvent> for Bus {
    type Reply = ();
    async fn handle(&mut self, m: mesh::MeshEvent, _: &mut Context<Self, ()>) {
        if let mesh::MeshEvent::Lease(l) = m {
            let _ = self.tx.send(Event::Lease(l));
        }
    }
}

/// Mutable channel state; one lock, never held across an await.
pub(crate) struct State {
    pub(crate) allowed: Vec<u64>,
    pub(crate) groups: Vec<i64>,
    pub(crate) threads: Threads,
    /// NEEDS(ROUTE): one worker per bound thread, spawned on first use.
    #[allow(dead_code)]
    pub(crate) workers: HashMap<ThreadKey, mpsc::UnboundedSender<WorkerMsg>>,
    pub(crate) last_event: Option<TelegramEvent>,
    pub(crate) dropped: u64,
    pub(crate) pairing: Option<Pairing>,
    /// 67 §4.4: polling is a hub duty; it follows the lease.
    pub(crate) follower: roster::lease::Follower,
    /// The running poller's token; `None` while stopped.
    pub(crate) poller: Option<CancellationToken>,
}

/// Everything shared by the tasks and the [`Handle`].
pub(crate) struct Channel {
    pub(crate) api: Arc<Api>,
    pub(crate) rt: Runtime,
    pub(crate) identity: OnceLock<BotIdentity>,
    pub(crate) owner: Mutex<Option<u64>>,
    pub(crate) state: Mutex<State>,
    /// NEEDS(ROUTE): the asks behind routing.
    #[allow(dead_code)]
    pub(crate) port: Arc<dyn Port>,
    /// NEEDS(ROUTE): `tools`, `reasoning`, `approvals` drive the workers and owner cards.
    #[allow(dead_code)]
    pub(crate) cfg: TelegramConfig,
    /// Process-wide `update_id` dedup, shared with the poller (98 §11).
    pub(crate) recent: Arc<Mutex<RecentKeys>>,
    inbound_tx: mpsc::UnboundedSender<Incoming>,
}

/// Control handle the `"telegram"` actor uses. Cheap to clone.
#[derive(Clone)]
pub struct Handle {
    pub(crate) chan: Arc<Channel>,
}

impl Handle {
    /// The 98 §13 snapshot.
    #[must_use]
    pub fn status(&self) -> TelegramStatus {
        self.chan.status()
    }

    /// `owner: Some(id)` sets the owner live and returns `Ok(None)` (no link
    /// needed); `None` mints a pairing code and returns the deep link.
    /// `Err` before `getMe` answered.
    pub async fn link(&self, owner: Option<u64>) -> Result<Option<PairingLink>, String> {
        if let Some(id) = owner {
            self.set_owner(id);
            return Ok(None);
        }
        let me = self
            .chan
            .identity
            .get()
            .ok_or_else(|| "bot identity unknown yet".to_string())?;
        let code = mint_code();
        let url = setup::deep_link(&me.username, &code);
        let expires_at_ms = u64::try_from(chrono::Utc::now().timestamp_millis())
            .unwrap_or(0)
            .saturating_add(PAIR_TTL.as_millis() as u64);
        self.chan.lock().pairing = Some(Pairing {
            code,
            expires: Instant::now() + PAIR_TTL,
        });
        let link = PairingLink { url, expires_at_ms };
        self.chan
            .publish(TelegramEvent::PairingLink {
                url: link.url.clone(),
                expires_at_ms,
            })
            .await;
        Ok(Some(link))
    }

    /// Adds `user` to the live allowlist (the config file is `lib.rs`'s job).
    pub fn allow(&self, user: u64) {
        let mut s = self.chan.lock();
        if !s.allowed.contains(&user) {
            s.allowed.push(user);
        }
    }

    /// Removes `user` from the live allowlist.
    pub fn remove_allowed(&self, user: u64) {
        self.chan.lock().allowed.retain(|u| *u != user);
    }

    /// Adds `chat` to the live group list.
    pub fn allow_group(&self, chat: i64) {
        let mut s = self.chan.lock();
        if !s.groups.contains(&chat) {
            s.groups.push(chat);
        }
    }

    /// Removes `chat` from the live group list.
    pub fn remove_group(&self, chat: i64) {
        self.chan.lock().groups.retain(|c| *c != chat);
    }

    /// Sets the owner live (pairing, `telegram_link { owner }`); the owner
    /// leaves the allowlist.
    pub fn set_owner(&self, user: u64) {
        *self.chan.owner.lock() = Some(user);
        self.chan.lock().allowed.retain(|u| *u != user);
    }
}

/// Spawns the channel under `rt.shutdown`; returns immediately. Fails only
/// when the HTTP client cannot be built.
pub fn spawn(cfg: TelegramConfig, token: String, rt: Runtime) -> anyhow::Result<Handle> {
    let api = Arc::new(Api::new(&token, None)?);
    let (inbound_tx, inbound_rx) = mpsc::unbounded_channel();
    let chan = Arc::new(Channel::new(api, cfg, rt, Arc::new(LivePort), inbound_tx));
    tokio::spawn(run(Arc::clone(&chan), inbound_rx));
    Ok(Handle { chan })
}

/// Supervisor (98 §3.1): bus, lease, inbound and event tasks; identity with
/// backoff; `deleteWebhook`; command scopes; boot repair; then the poller
/// follows the lease until shutdown.
async fn run(chan: Arc<Channel>, mut inbound_rx: mpsc::UnboundedReceiver<Incoming>) {
    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel::<Event>();
    let bus = Bus::spawn_with_mailbox(Bus { tx: ev_tx }, runtime::subscriber_mailbox());
    chan.rt.subscribe::<Bus, AgentEvent>(&bus).await;
    chan.rt.subscribe::<Bus, ApprovalRequested>(&bus).await;
    chan.rt.subscribe::<Bus, ApprovalResolved>(&bus).await;
    chan.rt.subscribe::<Bus, AskRequested>(&bus).await;
    chan.rt.subscribe::<Bus, AskResolved>(&bus).await;
    chan.rt.subscribe::<Bus, ArtifactShown>(&bus).await;
    chan.rt.subscribe::<Bus, secrets::SecretsEvent>(&bus).await;
    chan.rt.subscribe::<Bus, mesh::MeshEvent>(&bus).await;
    {
        let hook = Arc::clone(&chan);
        chan.api.on_event(Arc::new(move |ev| {
            let chan = Arc::clone(&hook);
            tokio::spawn(async move { chan.publish(ev).await });
        }));
    }
    // 67 §4.7: learn our engine id once the mesh is up; the lease resolves then.
    let me = {
        let chan = Arc::clone(&chan);
        tokio::spawn(async move {
            let Some(mesh) = chan.rt.lookup_when::<mesh::Mesh>(mesh::MESH_ACTOR).await else {
                return;
            };
            if let Ok(st) = mesh.ask(mesh::Status).await {
                chan.lock().follower.set_self(st.self_id.0);
                chan.sync_poller().await;
            }
        })
    };
    let inbound = {
        let chan = Arc::clone(&chan);
        tokio::spawn(async move {
            while let Some(inc) = inbound_rx.recv().await {
                chan.handle_inbound(inc).await;
            }
        })
    };
    let events = {
        let chan = Arc::clone(&chan);
        tokio::spawn(async move {
            while let Some(ev) = ev_rx.recv().await {
                chan.handle_event(ev).await;
            }
        })
    };
    let shutdown = chan.rt.shutdown.clone();
    let mut attempt: u32 = 0;
    let identity = loop {
        match chan.api.me().await {
            Ok(me) => break Some(me),
            Err(e) => {
                let delay = poll::backoff_delay(attempt);
                attempt = attempt.saturating_add(1);
                tracing::warn!(error = %e, delay_s = delay.as_secs(), "telegram getMe failed; retrying");
                chan.publish(TelegramEvent::Disconnected {
                    reason: format!("getMe: {e}"),
                })
                .await;
                tokio::select! {
                    () = tokio::time::sleep(delay) => {},
                    () = shutdown.cancelled() => break None,
                }
            }
        }
    };
    if let Some(me) = identity {
        tracing::info!(bot = %me.username, id = me.id, topics = me.topics, "telegram identity resolved");
        chan.publish(TelegramEvent::Identity {
            bot: me.username.clone(),
            topics: me.topics,
        })
        .await;
        let _ = chan.identity.set(me);
        if let Err(e) = chan.api.delete_webhook().await {
            tracing::warn!(error = %e, "telegram deleteWebhook failed");
        }
        let owner = *chan.owner.lock();
        for (scope, cmds) in commands::scopes(owner.and_then(|o| i64::try_from(o).ok())) {
            if let Err(e) = chan.api.set_commands(scope, &cmds).await {
                tracing::warn!(error = %e, ?scope, "telegram setMyCommands failed");
            }
        }
        chan.repair_open().await;
        chan.sync_poller().await;
        shutdown.cancelled().await;
    }
    me.abort();
    inbound.abort();
    events.abort();
    bus.kill();
}

impl Channel {
    fn new(
        api: Arc<Api>,
        cfg: TelegramConfig,
        rt: Runtime,
        port: Arc<dyn Port>,
        inbound_tx: mpsc::UnboundedSender<Incoming>,
    ) -> Self {
        let owner = cfg.owner;
        let allowed = cfg
            .allowed
            .iter()
            .copied()
            .filter(|u| Some(*u) != owner)
            .collect();
        Self {
            api,
            identity: OnceLock::new(),
            owner: Mutex::new(owner),
            state: Mutex::new(State {
                allowed,
                groups: cfg.groups.clone(),
                threads: Threads::load(&threads_path(&rt.data_dir)),
                workers: HashMap::new(),
                last_event: None,
                dropped: 0,
                pairing: None,
                follower: roster::lease::Follower::default(),
                poller: None,
            }),
            port,
            cfg,
            recent: Arc::new(Mutex::new(RecentKeys::default())),
            inbound_tx,
            rt,
        }
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock()
    }

    /// Publishes `ev` on the bus and remembers it for `Status`.
    pub(crate) async fn publish(&self, ev: TelegramEvent) {
        self.lock().last_event = Some(ev.clone());
        self.rt.publish(ev).await;
    }

    /// 67 §4.4: starts the poller when this engine holds the lease and
    /// identity is up; stops it when the lease went elsewhere. Idempotent.
    async fn sync_poller(self: &Arc<Self>) {
        let stopped = {
            let mut s = self.lock();
            let me = self.identity.get().filter(|_| s.follower.holding).cloned();
            match (me, s.poller.take()) {
                (Some(_), Some(tok)) => {
                    s.poller = Some(tok);
                    false
                }
                (Some(me), None) => {
                    let tok = self.rt.shutdown.child_token();
                    let chan = Arc::clone(self);
                    let ctx = PollCtx {
                        api: Arc::clone(&self.api),
                        me,
                        tx: self.inbound_tx.clone(),
                        rt: self.rt.clone(),
                        stop: tok.clone(),
                        on_state: Arc::new(move |ev| {
                            let chan = Arc::clone(&chan);
                            tokio::spawn(async move { chan.publish(ev).await });
                        }),
                        recent: Arc::clone(&self.recent),
                    };
                    tokio::spawn(poll::run(ctx));
                    s.poller = Some(tok);
                    false
                }
                (None, Some(tok)) => {
                    tok.cancel();
                    true
                }
                (None, None) => false,
            }
        };
        if stopped {
            tracing::info!("telegram poller stopped: another engine holds the hub lease");
            self.publish(TelegramEvent::Disconnected {
                reason: "hub lease held elsewhere".into(),
            })
            .await;
        }
    }

    /// 98 §11: edit-mode messages a previous process left mid-edit. Best
    /// effort: the keyboard goes; the text is RENDER's (`OpenMsg` carries no
    /// text to append `· interrupted` to).
    // NEEDS(RENDER): either persist the tail in `OpenMsg` or accept this repair.
    async fn repair_open(&self) {
        let open = self.lock().threads.open_take();
        for m in open {
            let at = MsgRef {
                chat: m.chat,
                id: m.message_id,
            };
            match self.api.edit_markup(&at, None).await {
                Ok(()) => {}
                Err(e) if is_not_modified(&e) => {}
                Err(e) => {
                    tracing::debug!(chat = m.chat, message_id = m.message_id, error = %e, "stale telegram message not repaired")
                }
            }
        }
    }

    /// The 98 §13 snapshot.
    pub(crate) fn status(&self) -> TelegramStatus {
        let owner = *self.owner.lock();
        let s = self.lock();
        TelegramStatus {
            state: s.last_event.clone(),
            bot: self.identity.get().cloned(),
            polling: s.poller.is_some(),
            owner,
            allowed: s.allowed.clone(),
            groups: s.groups.clone(),
            threads: s.threads.list(),
            open: s.threads.open_count(),
            calls: self.api.stats(),
            dropped_updates: s.dropped,
        }
    }

    /// Pairing code live right now, if any (98 §10.4).
    #[allow(dead_code)] // NEEDS(ROUTE): `/start <code>` consumes it.
    pub(crate) fn take_pairing(&self, code: &str) -> bool {
        let mut s = self.lock();
        match &s.pairing {
            Some(p) if p.code == code && p.expires > Instant::now() => {
                s.pairing = None;
                true
            }
            _ => false,
        }
    }

    // NEEDS(ROUTE): 98 §7 — trust, `/start`, commands, asks, routing to workers.
    async fn handle_inbound(&self, inc: Incoming) {
        match inc {
            Incoming::Ignored { update_id, kind } => {
                self.lock().dropped += 1;
                tracing::debug!(update_id, kind, "telegram update ignored");
            }
            other => tracing::debug!(inbound = ?other, "telegram inbound (routing lands in ROUTE)"),
        }
    }

    // NEEDS(ROUTE): 98 §8–§10 — fan events out to workers and cards.
    async fn handle_event(self: &Arc<Self>, ev: Event) {
        match ev {
            Event::Lease(lease) => {
                self.lock().follower.observe(&lease);
                self.sync_poller().await;
            }
            Event::Agent(ev) => tracing::trace!(?ev, "telegram agent event dropped (RENDER/ROUTE)"),
            _ => tracing::trace!("telegram bus event dropped (ROUTE)"),
        }
    }
}
