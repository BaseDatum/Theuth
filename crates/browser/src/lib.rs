//! Embedded obscura headless browser (design doc 23). One V8 isolate per
//! session on its own thread ([`session`]), the [`Browsers`] actor mapping
//! `SessionId → Handle` behind [`BROWSER_ACTOR`], sixteen `browser_*` tools
//! ([`tools`]) and a rendering `web::Provider` ([`render`]) so `read_url
//! rendered: true` works keyless. [`warmup`] runs on the main thread before
//! plugins start (23 §3.1). Frontends poll [`Screenshot`] for a live view of
//! a session's page; [`BrowserEvent::ShowLive`] asks the desktop to open it.
//! [`profile`] carries the shared browser profile (209): `tools::Ctx` loads
//! it into a session's isolate, and every isolate drop saves its delta first.
//! [`tile`] is the tools' second target, the user's web view tile (209 D8).

pub mod profile;
pub mod render;
pub mod session;
pub mod systemone;
pub mod tile;
pub mod tools;

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use ::tools::{
    CloseAck, CloseTree, REGISTRY_ACTOR, RegisterLifecycle, RegisterTool, ToolRegistry, Tracked,
};
use kameo::actor::Spawn;
use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use runtime::{Runtime, SessionId};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Registry name of the [`Browsers`] actor (23 §3.1).
pub const BROWSER_ACTOR: &str = "browser";

/// How often the actor drops isolates idle longer than `idle_secs` (23 §3.2).
const SWEEP_EVERY: Duration = Duration::from_secs(30);

/// Longest a [`CloseTree`] waits for the dropped isolates' threads to end
/// before acking `ok: false` (51 §4.5): a page script can hold a thread for
/// up to `script_deadline_ms`, and a close must not hang on it.
const CLOSE_WAIT: Duration = Duration::from_secs(10);

/// `browser:` config section (23 §6). Every field has a default; the section
/// may be absent.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct BrowserConfig {
    /// `false`: no warmup, no actor, no tools, no render provider.
    pub enabled: bool,
    /// wreq TLS fingerprint + tracker blocklist (23 §0.4).
    pub stealth: bool,
    /// `http://user:pass@host:port` or `socks5://host:port`.
    pub proxy: Option<String>,
    /// Default: obscura's current-Chrome UA for the platform.
    pub user_agent: Option<String>,
    /// IANA zone; `None` = the host's zone (not obscura's `Europe/Berlin`).
    pub timezone: Option<String>,
    /// `"lat,lon"`.
    pub geolocation: Option<String>,
    /// Let pages reach loopback / RFC1918 / link-local (23 §0.8). On by
    /// default (dev servers on `localhost` are the coding-agent case);
    /// `false` turns obscura's SSRF guard back on.
    pub allow_private_network: bool,
    /// Let `browser_import_cookies` copy the user's real browser cookies into
    /// a session so the agent acts as the signed-in user (23 §7). Off by
    /// default: it hands real credentials to the model.
    pub import_cookies: bool,
    /// Isolate teardown after this much inactivity.
    pub idle_secs: u64,
    /// Default cap of `browser_read` / `browser_open` replies.
    pub max_chars: usize,
    /// `OBSCURA_SCRIPT_DEADLINE_MS` for heavy SPAs.
    pub script_deadline_ms: u64,
    /// Request/response exchanges the isolate keeps per page for
    /// `browser_network_capture` (44 §3.1); `OBSCURA_NETWORK_BODY_BUFFER_ENTRIES`.
    pub capture_entries: usize,
    /// Per-body cap of a captured exchange; `OBSCURA_NETWORK_BODY_BUFFER_BYTES`.
    pub capture_body_bytes: usize,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            stealth: true,
            proxy: None,
            user_agent: None,
            timezone: None,
            geolocation: None,
            allow_private_network: true,
            import_cookies: false,
            idle_secs: 300,
            max_chars: 4000,
            script_deadline_ms: 30_000,
            capture_entries: 256,
            capture_body_bytes: 2 * 1024 * 1024,
        }
    }
}

/// One-time V8 setup, **on the main thread, before any plugin starts**
/// (23 §2, §3.1): exports obscura's environment knobs from config while the
/// process is still effectively single-threaded (the only place `set_var`
/// is safe), then builds and drops one `ObscuraJsRuntime` exactly as
/// `obscura-cdp` does, because the first isolate built off-main once
/// segfaulted in `InitializeBuiltinJSDispatchTable`. No-op when disabled.
pub fn warmup(cfg: &BrowserConfig) {
    if !cfg.enabled {
        return;
    }
    let timezone = cfg
        .timezone
        .clone()
        .or_else(|| match iana_time_zone::get_timezone() {
            Ok(tz) => Some(tz),
            Err(e) => {
                tracing::debug!("host timezone unknown, keeping obscura's default: {e}");
                None
            }
        });
    let knobs = [
        (
            "OBSCURA_ALLOW_PRIVATE_NETWORK",
            cfg.allow_private_network.then(|| "1".to_owned()),
        ),
        ("OBSCURA_TIMEZONE", timezone),
        ("OBSCURA_GEOLOCATION", cfg.geolocation.clone()),
        (
            "OBSCURA_SCRIPT_DEADLINE_MS",
            Some(cfg.script_deadline_ms.to_string()),
        ),
        (
            "OBSCURA_NETWORK_BODY_BUFFER_ENTRIES",
            Some(cfg.capture_entries.to_string()),
        ),
        (
            "OBSCURA_NETWORK_BODY_BUFFER_BYTES",
            Some(cfg.capture_body_bytes.to_string()),
        ),
    ];
    for (key, value) in knobs {
        if let Some(value) = value {
            // SAFETY: called from `main` before plugins spawn any thread that
            // reads the environment; obscura reads these lazily per isolate.
            unsafe { std::env::set_var(key, value) };
        }
    }
    let started = Instant::now();
    drop(obscura_js::runtime::ObscuraJsRuntime::new());
    tracing::info!(ms = started.elapsed().as_millis(), "browser warmup done");
}

/// The per-session isolate map, registered as [`BROWSER_ACTOR`] (23 §3.2).
/// `Call` creates the handle on first use; a sweeper drops handles idle
/// longer than `idle_secs`; `Close` drops one now; `rt.shutdown` drops all;
/// [`CloseTree`] drops a whole tree and waits for the threads to end (51 §4.5).
#[derive(kameo::Actor)]
pub struct Browsers {
    cfg: BrowserConfig,
    sessions: HashMap<SessionId, (session::Handle, Instant)>,
    /// Sessions that ran `browser_run` and are still in a turn / open (57 §2.1): while any
    /// is here the browser checkpoint stays resident; the last one leaving releases it.
    systemone: HashSet<SessionId>,
    /// The worker that runs [`profile::changed`] for each `WebProfileChanged`, in order and off
    /// this actor's turn (it reads the profile back); started on the first event.
    profile_events: Option<tokio::sync::mpsc::UnboundedSender<webprofile::WebProfileChanged>>,
}

impl Browsers {
    /// Empty map with the config the isolates are spawned from.
    pub fn new(cfg: BrowserConfig) -> Self {
        Self {
            cfg,
            sessions: HashMap::new(),
            systemone: HashSet::new(),
            profile_events: None,
        }
    }

    /// Live handle for `session`, spawning a thread on first use or after the
    /// previous one died; stamps `last_used`.
    fn handle(&mut self, session: SessionId) -> session::Handle {
        let cfg = &self.cfg;
        let (handle, last_used) = self
            .sessions
            .entry(session)
            .or_insert_with(|| (session::spawn(cfg), Instant::now()));
        if handle.is_closed() {
            tracing::warn!(%session, "browser thread gone, spawning a fresh one");
            *handle = session::spawn(cfg);
            // The new isolate holds nothing: the next tool call loads the profile again.
            profile::take(session);
        }
        *last_used = Instant::now();
        handle.clone()
    }

    /// `session` is done with the decision model for now (turn ended, `browser_close`, isolate
    /// dropped): when it was the last one, release the browser checkpoint (57 §2.1 item 1).
    fn systemone_done(&mut self, session: SessionId, reason: &'static str) {
        if self.systemone.remove(&session) && self.systemone.is_empty() {
            tokio::spawn(async move {
                let Ok(Some(engine)) =
                    kameo::actor::ActorRef::<decision::Engine>::lookup(decision::DECISION_ACTOR)
                else {
                    return;
                };
                let _ = engine
                    .tell(decision::Release {
                        checkpoint: Some(decision::BROWSER_CHECKPOINT.to_owned()),
                        reason,
                    })
                    .await;
            });
        }
    }

    /// Drop `session`'s isolate (if any) and its System-1 claim. A session with the shared
    /// profile loaded saves its delta first (209 D4): the export runs on a clone of the handle,
    /// bounded by [`profile::DROP_EXPORT_WAIT`], and the clone goes right after it, so the thread
    /// ends (and a `CloseTree` ack comes) at most that much later; the push runs after.
    fn drop_session(&mut self, session: SessionId) -> Option<session::Handle> {
        self.systemone_done(session, "browser session closed");
        let handle = self.sessions.remove(&session).map(|(h, _)| h);
        if let Some(loaded) = profile::take(session)
            && let Some(h) = handle.as_ref().filter(|h| !h.is_closed())
        {
            tokio::spawn(profile::save_dropped(session, h.clone(), loaded));
        }
        handle
    }
}

/// `ask` → `Result<Value, String>`: `Ok` is obscura-mcp's `result` object
/// (`{ content, isError }`, 23 §3.3); `Err` is `browser restarted: <reason>`
/// when the session's thread died under this call. Runs off the mailbox so a
/// slow page never blocks another session.
///
/// Sent bare, the command is **uncertified**: nothing in the activity ledger
/// (47 §4) covers it. Callers that own model-visible work send
/// [`Tracked<Call>`] (`Tracked::new(Call { .. })` inside an invocation) so
/// the originating invocation stays counted until the isolate has actually
/// finished the command, even after the caller's timeout dropped the reply.
/// `session` is the isolate key, never the activity owner: the render
/// provider and scrape slots use private ids here (47 §4.2 rule 5).
#[derive(Clone, Debug)]
pub struct Call {
    /// Owning session; subagents are sessions, so they get their own isolate.
    pub session: SessionId,
    /// obscura-mcp tool name (`browser_navigate`, ...).
    pub name: String,
    /// obscura-mcp tool arguments, passed through unchanged.
    pub args: Value,
}

/// `tell`/`ask` → `()`: drop the session's isolate now.
#[derive(Clone, Copy, Debug)]
pub struct Close {
    /// Session whose thread ends.
    pub session: SessionId,
}

/// `tell`: `session` ran `browser_run` (`active: true`) — the browser checkpoint stays
/// resident until every such session is done: its turn ends (`AgentEvent::TurnFinished`), it
/// calls `browser_close` (`active: false`), or its isolate is dropped (57 §2.1 item 1).
#[derive(Clone, Copy, Debug)]
pub struct SystemOne {
    /// The session.
    pub session: SessionId,
    /// `true` = ran `browser_run`; `false` = done with it.
    pub active: bool,
}

/// `ask` → `Option<String>`: base64 PNG of the session's current page for a
/// live viewer (23 §5). `None` when the session has no isolate, no open
/// page, or its thread died. Never spawns an isolate and never refreshes
/// the idle stamp: a viewer's poll must not keep a browser alive.
#[derive(Clone, Copy, Debug)]
pub struct Screenshot {
    /// Session whose page is shown.
    pub session: SessionId,
}

/// Bus events for frontends, on the shared RPC event stream (23 §5).
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub enum BrowserEvent {
    /// `browser_show`: open the live view of this session's browser.
    #[serde(rename = "BrowserShowLive")]
    ShowLive {
        /// Owning session.
        session: SessionId,
    },
}

impl wire::WireEvent for BrowserEvent {
    const TOPIC: &'static str = "browser.events";
}

/// `data` of the first `type == "image"` block of an obscura-mcp result.
pub(crate) fn image_data(result: &Value) -> Option<&str> {
    result
        .get("content")?
        .as_array()?
        .iter()
        .find(|b| b.get("type").and_then(Value::as_str) == Some("image"))?
        .get("data")?
        .as_str()
}

/// `tell`: drop handles idle longer than `idle_secs` (the plugin sends it every
/// 30 s); a session with the shared profile saves first (209 D4).
#[derive(Clone, Copy, Debug)]
pub struct Sweep;

/// `tell`: drop every isolate (the plugin sends it when `rt.shutdown` fires); a session with the
/// shared profile saves first (209 D4).
#[derive(Clone, Copy, Debug)]
pub struct CloseAll;

impl Message<Tracked<Call>> for Browsers {
    type Reply = DelegatedReply<Result<Value, String>>;

    async fn handle(
        &mut self,
        m: Tracked<Call>,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let handle = self.handle(m.message.session);
        // The guard moves into the future *before* spawn and lives until
        // `handle.call` resolves: the worker replied, or its thread died and
        // dropped the queued command with it. Either way nothing is still
        // running, so the last clone may report `Finished`; a panic in here
        // is marked unknown by `scope`. The isolate's idle page pump is not
        // tracked: ambient timers owe the model nothing.
        tokio::spawn(m.scope(|call| async move {
            let out = handle
                .call(&call.name, call.args)
                .await
                .map_err(|reason| format!("browser restarted: {reason}"));
            tx.send(out);
        }));
        reply
    }
}

impl Message<Call> for Browsers {
    type Reply = DelegatedReply<Result<Value, String>>;

    async fn handle(&mut self, m: Call, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        Message::<Tracked<Call>>::handle(
            self,
            Tracked {
                message: m,
                activity: None,
            },
            ctx,
        )
        .await
    }
}

impl Message<Screenshot> for Browsers {
    type Reply = DelegatedReply<Option<String>>;

    async fn handle(&mut self, m: Screenshot, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let Some((handle, _)) = self.sessions.get(&m.session) else {
            tx.send(None);
            return reply;
        };
        let handle = handle.clone();
        tokio::spawn(async move {
            let out = handle
                .call("browser_screenshot", serde_json::json!({}))
                .await
                .ok()
                .filter(|v| v.get("isError").and_then(Value::as_bool) != Some(true))
                .and_then(|v| image_data(&v).map(str::to_owned));
            tx.send(out);
        });
        reply
    }
}

impl Message<Close> for Browsers {
    type Reply = ();

    async fn handle(&mut self, m: Close, _: &mut Context<Self, Self::Reply>) {
        self.drop_session(m.session);
    }
}

impl Message<SystemOne> for Browsers {
    type Reply = ();

    async fn handle(&mut self, m: SystemOne, _: &mut Context<Self, Self::Reply>) {
        if m.active {
            self.systemone.insert(m.session);
        } else {
            self.systemone_done(m.session, "browser_close");
        }
    }
}

impl Message<agent::AgentEvent> for Browsers {
    type Reply = ();

    /// 57 §2.1: a turn ending releases the browser checkpoint. 209 §4.6: a deleted session's
    /// tile target goes.
    async fn handle(&mut self, e: agent::AgentEvent, _: &mut Context<Self, Self::Reply>) {
        match e {
            agent::AgentEvent::TurnFinished { session, .. } => {
                self.systemone_done(session, "turn finished");
            }
            agent::AgentEvent::SessionDeleted { session } => tile::forget(&[session]),
            _ => {}
        }
    }
}

impl Message<webprofile::WebProfileChanged> for Browsers {
    type Reply = ();

    /// 209 D14, §6: a site written to the profile makes acting calls on it credentialed; a site
    /// forgotten, or a sign out everywhere, leaves the known sites and every session's isolate
    /// before that session's next call ([`profile::changed`]). Here or on another engine. The
    /// read-back runs on the actor's event worker, never awaited here.
    async fn handle(&mut self, e: webprofile::WebProfileChanged, _: &mut Context<Self, Self::Reply>) {
        profile::changed_later(&mut self.profile_events, e);
    }
}

impl Message<Sweep> for Browsers {
    type Reply = ();

    async fn handle(&mut self, _: Sweep, _: &mut Context<Self, Self::Reply>) {
        let idle = Duration::from_secs(self.cfg.idle_secs);
        let gone: Vec<SessionId> = self
            .sessions
            .iter()
            .filter(|(_, (_, last_used))| last_used.elapsed() >= idle)
            .map(|(s, _)| *s)
            .collect();
        for session in gone {
            tracing::info!(%session, "browser isolate idle, dropping");
            self.drop_session(session);
        }
    }
}

impl Message<CloseAll> for Browsers {
    type Reply = ();

    async fn handle(&mut self, _: CloseAll, _: &mut Context<Self, Self::Reply>) {
        for session in self.sessions.keys().copied().collect::<Vec<_>>() {
            self.drop_session(session);
        }
    }
}

impl Message<CloseTree> for Browsers {
    type Reply = DelegatedReply<CloseAck>;

    async fn handle(&mut self, m: CloseTree, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        // Same as `Close` per session: dropping the map's handle closes the
        // command channel. The thread (and with it the isolate's cookie jar
        // and storage) ends once any in-flight command finishes; we wait for
        // that off the mailbox, bounded, so a hung page cannot stall the ack.
        tile::forget(&m.subtree);
        let exits: Vec<_> = m
            .subtree
            .iter()
            .filter_map(|s| self.drop_session(*s))
            .map(|handle| handle.exited())
            .collect();
        let Some(tx) = tx else { return reply };
        tokio::spawn(async move {
            let n = exits.len();
            let drained = tokio::time::timeout(CLOSE_WAIT, futures::future::join_all(exits)).await;
            tx.send(match drained {
                Ok(_) => CloseAck::ok("browser"),
                Err(_) => CloseAck::failed(
                    "browser",
                    format!(
                        "{n} isolate(s) still finishing a command after {}s",
                        CLOSE_WAIT.as_secs()
                    ),
                ),
            });
        });
        reply
    }
}

/// Declare browser wire contracts even when the browser service is disabled.
pub async fn declare_wire(rt: &Runtime) -> anyhow::Result<()> {
    use wire::RuntimeWire as _;
    rt.register_wire_event::<BrowserEvent>().await?;
    Ok(())
}

/// Subscribe `actor` to the bus events it handles (the plugin does this at start): `AgentEvent`
/// (57 §2.1, a turn ending releases the browser checkpoint) and `WebProfileChanged` (209 §6, a
/// forget or sign out everywhere, local or replicated, reaches live isolates).
pub async fn subscribe(rt: &Runtime, actor: &kameo::actor::ActorRef<Browsers>) {
    rt.subscribe::<Browsers, agent::AgentEvent>(actor).await;
    rt.subscribe::<Browsers, webprofile::WebProfileChanged>(actor).await;
}

/// The plugin (23 §3.1): reads `browser`, runs the sweeper, spawns
/// [`Browsers`] as [`BROWSER_ACTOR`], registers the tools once the registry
/// is up and the render provider once `web` is up.
pub struct BrowserPlugin;

#[async_trait::async_trait]
impl runtime::Plugin for BrowserPlugin {
    fn name(&self) -> &'static str {
        "browser"
    }

    async fn declare_wire(&mut self, rt: &runtime::Runtime) -> anyhow::Result<()> {
        declare_wire(rt).await
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        declare_wire(rt).await?;
        let cfg: BrowserConfig = rt.section("browser").unwrap_or_else(|e| {
            tracing::debug!("no [browser] config section, using defaults: {e}");
            BrowserConfig::default()
        });
        if !cfg.enabled {
            tracing::info!("browser disabled by config");
            return Ok(());
        }
        let actor =
            Browsers::spawn_with_mailbox(Browsers::new(cfg.clone()), runtime::subscriber_mailbox());
        actor.register(BROWSER_ACTOR)?;
        subscribe(rt, &actor).await;
        tokio::spawn(profile::learn_sites());

        let sweeper = actor.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(SWEEP_EVERY);
            tick.tick().await;
            loop {
                tick.tick().await;
                if sweeper.tell(Sweep).await.is_err() {
                    break;
                }
            }
        });
        let shutdown = rt.shutdown.clone();
        let me = actor.clone();
        tokio::spawn(async move {
            shutdown.cancelled().await;
            let _ = me.tell(CloseAll).await;
        });

        tokio::spawn(render::register(rt.clone(), actor.clone()));
        let rt = rt.clone();
        tokio::spawn(async move {
            if let Some(reg) = rt.lookup_when::<ToolRegistry>(REGISTRY_ACTOR).await {
                let _ = reg
                    .ask(RegisterLifecycle {
                        owner: "browser",
                        opened: None,
                        close: actor.clone().reply_recipient(),
                    })
                    .await;
                for tool in tools::all(actor, cfg, rt) {
                    let _ = reg.ask(RegisterTool(tool)).await;
                }
            }
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::BrowserConfig;

    #[test]
    fn private_network_allowed_by_default() {
        assert!(BrowserConfig::default().allow_private_network);
    }
}
