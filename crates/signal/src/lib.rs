//! Signal plugin: a phone-side frontend for the daemon (design doc 09).
//!
//! The protocol tier ([`transport`], [`presage`], [`text`], [`error`],
//! [`mock`]) is copied from jarvis and frozen (09 §2.1–2.2). Everything else
//! is theuth glue: [`channel`] supervises link/connect and turns envelopes into
//! agent asks; [`commands`] answers slash commands; [`approvals`] routes
//! approval requests to the phone and reactions back; [`user`] delivers
//! `message_user` text and files (24 §6).
//!
//! - commands on the `"signal"` actor: [`Link`], [`Status`], [`Allow`],
//!   [`RemoveAllowed`], [`Unlink`]
//! - events: [`SignalEvent`]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use runtime::SessionId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tools::{Risk, Tool, ToolCx, ToolOutput};
use transport::SignalTransport;

/// Messaging RPC methods and typed clients.
pub mod rpc_messaging;

/// Assemble this plugin's RPC methods.
pub fn rpc_module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    rpc_core::merge([
        rpc_messaging::rpc_module(server),
    ])
}

pub mod approvals;
pub mod asks;
pub mod channel;
pub mod commands;
pub mod error;
#[cfg(test)]
pub mod mock;
pub mod presage;
pub mod text;
pub mod transport;
pub mod user;

/// Registry name of the plugin's actor.
pub const SIGNAL_ACTOR: &str = "signal";

/// `[signal]` config section (09 §12).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SignalConfig {
    /// The daemon starts the channel when linked.
    pub enabled: bool,
    /// Shown in the phone's Linked devices list.
    pub device_name: String,
    /// Owner E.164; owner-only commands. Empty until linked.
    pub owner: String,
    /// More E.164s allowed to talk; each gets its own thread.
    pub allowed: Vec<String>,
    /// Diagrams (`json-render`, `mermaid`, `openui` fences) go out as PNG
    /// attachments drawn by a connected desktop/web client (164 D9); `false`
    /// sends their text fallback instead (164 D7, D10).
    pub pictures: bool,
}

impl Default for SignalConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            device_name: "theuth".into(),
            owner: String::new(),
            allowed: Vec::new(),
            pictures: true,
        }
    }
}

/// Bus event: link/connection state (09 §4). Any UI renders it; a UI that
/// receives `LinkUrl` shows the URL as a QR.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub enum SignalEvent {
    /// A provisioning URL to scan from the primary phone; single use, ~90 s.
    LinkUrl {
        /// `sgnl://linkdevice?...`.
        url: String,
        /// Unix ms after which the URL is dead and a fresh one follows.
        expires_at_ms: u64,
    },
    /// Linking completed.
    Linked {
        /// The account's E.164.
        number: String,
    },
    /// Receiving messages.
    Connected,
    /// Stream ended; reconnecting after `retry_in_secs`.
    Disconnected {
        /// Backoff before the next attempt.
        retry_in_secs: u64,
    },
    /// Registration lost (device removed on the phone); back to `LinkUrl`.
    Unlinked,
    /// Terminal mode: the channel only runs in `t --headless`.
    DaemonOnly,
}

impl wire::WireEvent for SignalEvent {
    const TOPIC: &'static str = "signal.events";
}

/// Declare wire contracts before starting producers, even when disabled.
pub async fn declare_wire(rt: &runtime::Runtime) -> anyhow::Result<()> {
    use wire::RuntimeWire as _;
    rt.register_wire_event::<SignalEvent>().await
}

/// `ask` → [`SignalStatus`]: the last [`SignalEvent`] plus trust and threads.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Status;

/// Snapshot served by [`Status`], `/signal status`, `signal_status` and RPC.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SignalStatus {
    /// Last state event; `None` before the supervisor has reported.
    pub state: Option<SignalEvent>,
    /// `presage.db` holds a registration.
    pub registered: bool,
    /// The linked account's number, when registered.
    pub number: Option<String>,
    /// Owner E.164.
    pub owner: String,
    /// Allowed E.164s (excluding the owner).
    pub allowed: Vec<String>,
    /// Contact → live session bindings (09 §6).
    pub threads: Vec<(String, SessionId)>,
}

/// `ask` → `Result<String, String>`: start linking if not registered and
/// return the current provisioning URL. Completion arrives as
/// [`SignalEvent::Linked`]. Terminal mode answers `Err` with the CLI hint.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Link;

/// `ask` → `Result<(), String>`: add an E.164 to `signal.allowed` (config
/// file and live list).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Allow(pub String);

/// `ask` → `Result<(), String>`: remove an E.164 from `signal.allowed`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoveAllowed(pub String);

/// `ask` → `Result<(), String>`: stop the pump, delete `presage.db`, return
/// to linking. Threads are kept (09 §11.4).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Unlink;

/// `t --link-signal` inputs (09 §11.1, 12 §3).
pub struct LinkOpts {
    /// Owner E.164; prompted on a TTY when absent and not already configured.
    pub owner: Option<String>,
    /// Overrides config `signal.device_name`.
    pub device_name: Option<String>,
}

/// The plugin. `channel: true` (headless) opens the presage store and runs
/// the channel; `false` registers the actor and tools only (09 §3).
/// `link: Some(_)` pairs one-shot instead: QR, persist owner/device name,
/// cancel shutdown (12 §3).
pub struct SignalPlugin {
    /// Run the receive/send channel (daemon mode).
    pub channel: bool,
    /// Pair as a linked device and exit instead of starting.
    pub link: Option<LinkOpts>,
}

/// Terminal-mode answer to [`Link`] (09 §11.1).
const DAEMON_HINT: &str = "signal runs in the daemon (t --headless); link with t --link-signal";

/// How long the terminal-mode `signal_link` tool waits for the first URL.
const FIRST_URL_WAIT: Duration = Duration::from_secs(30);

/// Normalize `s` and check it is E.164: `+` followed by 8–15 digits.
pub fn e164(s: &str) -> Result<String, String> {
    let n = channel::normalize_number(s);
    let digits = n.strip_prefix('+').unwrap_or("");
    if !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
        && (8..=15).contains(&digits.len())
    {
        Ok(n)
    } else {
        Err(format!("not an E.164 number (+ and 8–15 digits): {s}"))
    }
}

/// YAML value for a string key: double-quoted (`+1…` would otherwise parse as an int).
fn yaml_str(s: &str) -> String {
    format!("{s:?}")
}

/// `signal.allowed` as a single-line YAML flow list of quoted strings.
fn yaml_list(items: &[String]) -> String {
    let quoted: Vec<String> = items.iter().map(|s| yaml_str(s)).collect();
    format!("[{}]", quoted.join(", "))
}

/// Delete `<data_dir>/signal/presage.db` and its `-wal`/`-shm` siblings.
fn delete_store(data_dir: &Path) -> std::io::Result<()> {
    let entries = match std::fs::read_dir(data_dir.join("signal")) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    for entry in entries {
        let entry = entry?;
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with("presage.db")
        {
            std::fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

/// The `"signal"` actor: control surface for the channel (daemon) or the
/// terminal-mode stand-in that only answers `Status` and edits config.
#[derive(kameo::Actor)]
pub struct Signal {
    handle: Option<channel::Handle>,
    cfg: SignalConfig,
    data_dir: PathBuf,
    config_path: PathBuf,
}

impl Signal {
    fn persist_allowed(&self) -> Result<(), String> {
        runtime::write_config_value(
            &self.config_path,
            "signal",
            "allowed",
            &yaml_list(&self.cfg.allowed),
        )
        .map_err(|e| format!("write {}: {e}", self.config_path.display()))
    }
}

impl Message<Link> for Signal {
    type Reply = Result<String, String>;

    async fn handle(&mut self, _: Link, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        match &self.handle {
            Some(h) => h.link().await,
            None => Err(DAEMON_HINT.into()),
        }
    }
}

impl Message<Status> for Signal {
    type Reply = SignalStatus;

    async fn handle(&mut self, _: Status, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        match &self.handle {
            Some(h) => h.status().await,
            None => SignalStatus {
                state: Some(SignalEvent::DaemonOnly),
                registered: self.data_dir.join("signal/presage.db").exists(),
                number: None,
                owner: self.cfg.owner.clone(),
                allowed: self.cfg.allowed.clone(),
                threads: channel::Threads::load(&self.data_dir).all(),
            },
        }
    }
}

impl Message<Allow> for Signal {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: Allow, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let n = e164(&m.0)?;
        if let Some(h) = &self.handle {
            h.allow(n.clone()).await?;
        }
        if !self.cfg.allowed.contains(&n) {
            self.cfg.allowed.push(n);
        }
        self.persist_allowed()
    }
}

impl Message<RemoveAllowed> for Signal {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: RemoveAllowed,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let n = e164(&m.0)?;
        if let Some(h) = &self.handle {
            h.remove_allowed(n.clone()).await?;
        }
        self.cfg.allowed.retain(|a| *a != n);
        self.persist_allowed()
    }
}

impl Message<Unlink> for Signal {
    type Reply = Result<(), String>;

    async fn handle(&mut self, _: Unlink, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        match &self.handle {
            Some(h) => h.unlink().await,
            None => delete_store(&self.data_dir).map_err(|e| format!("delete presage.db: {e}")),
        }
    }
}

#[async_trait::async_trait]
impl runtime::Plugin for SignalPlugin {
    fn name(&self) -> &'static str {
        "signal"
    }

    async fn start(self: Box<Self>, rt: &runtime::Runtime) -> anyhow::Result<()> {
        declare_wire(rt).await?;
        let cfg: SignalConfig = rt.section("signal").unwrap_or_else(|e| {
            tracing::debug!("no [signal] config section, using defaults: {e}");
            SignalConfig::default()
        });
        let config_path = rt.data_dir.join("config.yaml");
        if let Some(opts) = self.link {
            pair(cfg, &rt.data_dir, &config_path, opts).await?;
            rt.shutdown.cancel();
            return Ok(());
        }
        if !cfg.enabled {
            tracing::info!("signal disabled by config");
            return Ok(());
        }
        let handle = self.channel.then(|| {
            let transport: Arc<dyn SignalTransport> =
                Arc::new(presage::PresageTransport::new(&rt.data_dir));
            channel::spawn(transport, cfg.clone(), rt.clone())
        });
        let cfg_for_tool = cfg.clone();
        let sink = handle.clone();
        let actor = Signal::spawn(Signal {
            handle,
            cfg,
            data_dir: rt.data_dir.clone(),
            config_path: config_path.clone(),
        });
        actor.register(SIGNAL_ACTOR)?;

        // First use of the registry (00 §4, 12 §2).
        let rt = rt.clone();
        let daemon = self.channel;
        tokio::spawn(async move {
            let Some(reg) = rt
                .lookup_when::<tools::ToolRegistry>(tools::REGISTRY_ACTOR)
                .await
            else {
                return;
            };
            let _ = reg
                .ask(tools::RegisterTool(Arc::new(SignalLinkTool {
                    data_dir: rt.data_dir.clone(),
                    config_path,
                    cfg: cfg_for_tool,
                    daemon,
                    link: Mutex::new(None),
                })))
                .await;
            let _ = reg
                .ask(tools::RegisterTool(Arc::new(SignalStatusTool)))
                .await;
            // Outbound user channel for `message_user` (24 §6).
            if let Some(sink) = sink
                && let Some(user) = rt
                    .lookup_when::<agent::UserChannels>(agent::USER_ACTOR)
                    .await
            {
                let _ = user.ask(agent::RegisterUserSink(Arc::new(sink))).await;
            }
        });
        Ok(())
    }
}

/// `t --link-signal` (09 §11.1): pair, print the QR, persist
/// `signal.owner`/`signal.device_name`. Runs regardless of `signal.enabled`.
async fn pair(
    mut cfg: SignalConfig,
    data_dir: &Path,
    config_path: &Path,
    opts: LinkOpts,
) -> anyhow::Result<()> {
    if let Some(n) = opts.device_name {
        cfg.device_name = n;
    }
    let owner = opts
        .owner
        .map(|o| e164(&o))
        .transpose()
        .map_err(anyhow::Error::msg)?;
    let number = link_oneshot(data_dir, &cfg, |url| {
        println!(
            "scan from Signal → Settings → Linked devices (expires in ~90 s; a fresh code follows):\n\n{}\n{url}\n",
            qr(url)
        );
    })
    .await?;
    let owner = match owner {
        Some(o) => o,
        None if !cfg.owner.is_empty() => cfg.owner.clone(),
        None => {
            use std::io::{IsTerminal, Write};
            let mut line = String::new();
            if std::io::stdin().is_terminal() {
                print!("owner number (E.164, e.g. +15551234567): ");
                std::io::stdout().flush()?;
                std::io::stdin().read_line(&mut line)?;
            }
            match e164(line.trim()) {
                Ok(o) => o,
                Err(e) => {
                    eprintln!(
                        "{e}; set signal.owner in {} yourself",
                        config_path.display()
                    );
                    String::new()
                }
            }
        }
    };
    if !owner.is_empty() {
        runtime::write_config_value(config_path, "signal", "owner", &yaml_str(&owner))?;
    }
    runtime::write_config_value(config_path, "signal", "device_name", &cfg.device_name)?;
    println!("linked as {number}; owner {owner}; run t --headless");
    Ok(())
}

/// Unicode half-block QR without quiet zone (09 §11); one renderer, owned by `link` (31 §14).
pub use link::qr;

/// One-shot linking outside the daemon (09 §11.1): opens the store, refuses
/// when already registered, rotates provisioning URLs per 09 §3.1 (each
/// passed to `on_url`) until the phone scans one, returns the account number.
pub async fn link_oneshot(
    data_dir: &Path,
    cfg: &SignalConfig,
    mut on_url: impl FnMut(&str),
) -> anyhow::Result<String> {
    let t = presage::PresageTransport::new(data_dir);
    if t.is_registered().await? {
        let number = t
            .account()
            .await
            .ok()
            .flatten()
            .map_or_else(|| "<unknown>".to_owned(), |r| r.number);
        anyhow::bail!("already linked as {number}; t --headless to run, /signal unlink to relink");
    }
    let mut expired: u32 = 0;
    let mut faults: u32 = 0;
    loop {
        let attempt = match t.link_device(&cfg.device_name).await {
            Ok(a) => a,
            Err(e) => {
                let delay = channel::backoff_delay(faults);
                faults += 1;
                tracing::warn!(error = %e, retry_in = ?delay, "link_device failed");
                tokio::time::sleep(delay).await;
                continue;
            }
        };
        faults = 0;
        on_url(&attempt.url.0);
        match attempt.done.await {
            Ok(()) => break,
            Err(e) => {
                expired += 1;
                let pause = if expired >= channel::MAX_FAST_LINK_ATTEMPTS {
                    channel::LINK_SLOW_PAUSE
                } else {
                    channel::LINK_CHAIN_PAUSE
                };
                tracing::info!(error = %e, attempt = expired, pause = ?pause, "link attempt ended; fresh URL follows");
                tokio::time::sleep(pause).await;
            }
        }
    }
    t.account()
        .await?
        .map(|r| r.number)
        .filter(|n| !n.is_empty())
        .ok_or_else(|| anyhow::anyhow!("linked, but the account number is not known yet"))
}

/// Tool text for a provisioning URL.
fn link_text(url: &str, tail: &str) -> String {
    format!(
        "scan this from Signal → Settings → Linked devices:\n\n{}\n{url}\n({tail})",
        qr(url)
    )
}

/// Arguments for [`SignalLinkTool`].
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct SignalLinkArgs {
    /// Owner E.164 (`+15551234567`) written to `signal.owner`; owner-only commands come from it.
    pub owner: Option<String>,
}

/// Terminal-mode in-process link: latest URL receiver + the `link_oneshot` task.
type InflightLink = (watch::Receiver<Option<String>>, tokio::task::JoinHandle<()>);

/// Starts linking and returns the QR to scan (09 §11.3).
pub struct SignalLinkTool {
    data_dir: PathBuf,
    config_path: PathBuf,
    cfg: SignalConfig,
    daemon: bool,
    /// Terminal mode: the one-shot link, if one is running.
    link: Mutex<Option<InflightLink>>,
}

impl SignalLinkTool {
    /// Current URL receiver, starting a one-shot link when none is running.
    fn terminal_link(&self) -> watch::Receiver<Option<String>> {
        let mut guard = self.link.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((rx, task)) = &*guard
            && !task.is_finished()
        {
            return rx.clone();
        }
        let (tx, rx) = watch::channel(None);
        let data_dir = self.data_dir.clone();
        let config_path = self.config_path.clone();
        let cfg = self.cfg.clone();
        let task = tokio::spawn(async move {
            match link_oneshot(&data_dir, &cfg, |u| drop(tx.send(Some(u.to_owned())))).await {
                Ok(number) => {
                    tracing::info!(%number, "signal linked");
                    if let Err(e) = runtime::write_config_value(
                        &config_path,
                        "signal",
                        "device_name",
                        &cfg.device_name,
                    ) {
                        tracing::warn!(error = %e, "write signal.device_name");
                    }
                }
                Err(e) => tracing::warn!(error = %e, "signal link failed"),
            }
        });
        *guard = Some((rx.clone(), task));
        rx
    }
}

#[async_trait::async_trait]
impl Tool for SignalLinkTool {
    const NAME: &'static str = "signal_link";
    const RISK: Risk = Risk::Mutating;
    type Args = SignalLinkArgs;

    fn description(&self) -> String {
        "Link this machine as a Signal device: returns a QR code to scan from the phone's Signal → Settings → Linked devices. Optionally sets the owner number.".into()
    }

    async fn call(&self, args: SignalLinkArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        if let Some(owner) = args.owner {
            let owner = match e164(&owner) {
                Ok(o) => o,
                Err(e) => return Ok(ToolOutput::err(e)),
            };
            runtime::write_config_value(&self.config_path, "signal", "owner", &yaml_str(&owner))?;
        }
        if self.daemon {
            let Ok(Some(actor)) = ActorRef::<Signal>::lookup(SIGNAL_ACTOR) else {
                return Ok(ToolOutput::err("signal actor is not running"));
            };
            return Ok(match actor.ask(Link).await {
                Ok(url) => ToolOutput::ok(link_text(
                    &url,
                    "expires in ~90 s; a fresh code follows in the daemon log",
                )),
                Err(e) => ToolOutput::err(e.to_string()),
            });
        }
        let mut rx = self.terminal_link();
        let url = tokio::time::timeout(FIRST_URL_WAIT, rx.wait_for(|u| u.is_some()))
            .await
            .ok()
            .and_then(|r| r.ok())
            .and_then(|u| u.clone());
        Ok(match url {
            Some(url) => ToolOutput::ok(link_text(
                &url,
                "expires in ~90 s; call signal_link again for a fresh code",
            )),
            None => ToolOutput::err(
                "no provisioning URL within 30 s (already linked, or the store is busy); see the log",
            ),
        })
    }
}

/// Arguments for [`SignalStatusTool`]: none.
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct SignalStatusArgs {}

/// The 09 §4 snapshot as text.
pub struct SignalStatusTool;

#[async_trait::async_trait]
impl Tool for SignalStatusTool {
    const NAME: &'static str = "signal_status";
    const RISK: Risk = Risk::ReadOnly;
    type Args = SignalStatusArgs;

    fn description(&self) -> String {
        "Signal link/connection state, owner, allowed numbers and phone threads.".into()
    }

    async fn call(&self, _: SignalStatusArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let Ok(Some(actor)) = ActorRef::<Signal>::lookup(SIGNAL_ACTOR) else {
            return Ok(ToolOutput::err("signal actor is not running"));
        };
        let s = actor.ask(Status).await?;
        Ok(ToolOutput::ok(render_status(&s)))
    }
}

/// Plain-text rendering of a [`SignalStatus`].
pub fn render_status(s: &SignalStatus) -> String {
    let state = match &s.state {
        None => "unknown".to_owned(),
        Some(SignalEvent::LinkUrl { url, .. }) => format!("waiting for scan\n{}\n{url}", qr(url)),
        Some(SignalEvent::Linked { number }) => format!("linked as {number}"),
        Some(SignalEvent::Connected) => "connected".into(),
        Some(SignalEvent::Disconnected { retry_in_secs }) => {
            format!("disconnected; retry in {retry_in_secs} s")
        }
        Some(SignalEvent::Unlinked) => "unlinked; waiting to link".into(),
        Some(SignalEvent::DaemonOnly) => "daemon only (runs in t --headless)".into(),
    };
    let mut out = format!(
        "state: {state}\nregistered: {}\nnumber: {}\nowner: {}\nallowed: {}\n",
        s.registered,
        s.number.as_deref().unwrap_or("-"),
        if s.owner.is_empty() { "-" } else { &s.owner },
        if s.allowed.is_empty() {
            "-".to_owned()
        } else {
            s.allowed.join(", ")
        },
    );
    if s.threads.is_empty() {
        out.push_str("threads: -");
    } else {
        out.push_str("threads:");
        for (number, session) in &s.threads {
            out.push_str(&format!("\n  {number} → {session}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qr_fits_a_tool_card() {
        let s = qr(
            "sgnl://linkdevice?uuid=AAAAAAAAAAAAAAAAAAAAAA&pub_key=BQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        );
        assert!(s.chars().count() < 2000, "{}", s.chars().count());
        assert!(s.lines().count() >= 20, "{}", s.lines().count());
    }

    #[test]
    fn e164_validation() {
        assert_eq!(e164("+1 (555) 123-4567").unwrap(), "+15551234567");
        assert_eq!(e164("15551234567").unwrap(), "+15551234567");
        assert!(e164("+1234567").is_err());
        assert!(e164("+1234567890123456").is_err());
        assert!(e164("hello").is_err());
    }

    #[test]
    fn allowed_list_is_quoted_flow_yaml() {
        let l = yaml_list(&["+15551234567".into(), "+14445556666".into()]);
        assert_eq!(l, r#"["+15551234567", "+14445556666"]"#);
        let v: Vec<String> = serde_yaml::from_str(&l).unwrap();
        assert_eq!(v[0], "+15551234567");
    }
}
