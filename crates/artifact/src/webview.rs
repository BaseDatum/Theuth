//! The `show_web` tool and its [`WebViewOpened`] event (205 §4.1, §4.2): a live web page in a tile
//! beside the chat. The engine only validates the address and announces it; the desktop shell
//! hosts the page in a native child webview (205 §4.3). The engine's record of what each tile
//! shows now ([`record_state`], fed by the RPC `web_view_state`) and the `web_views` tool that
//! reads it (209 D9). Agent control of a tile (209 D8): [`call`] publishes
//! [`WebViewCallRequested`] and waits for the client's [`WebViewCallAnswer`]s through [`answer`]
//! (the RPC `answer_web_view_call`), which takes the result only from the connection that claimed
//! the call (209 §4.4).

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use runtime::{Runtime, SessionId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tools::{Risk, Tool, ToolCx, ToolOutput};
use url::{Host, Url};

/// Arguments of `show_web` (205 §4.1).
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ShowWebArgs {
    /// Absolute `http://` or `https://` address. A bare `localhost:3000` or `127.0.0.1:3000/x`
    /// gets `http://` prepended. No `user:password@`.
    pub url: String,
    /// Tile and card title. Default: the address's `host[:port]`.
    pub title: Option<String>,
}

/// Event: a `show_web` call opened (or re-targeted) a web view tile (205 §4.2).
/// Wire shape `{ "type": "WebViewOpened", "session", "call_id", "id", "url", "title" }`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct WebViewOpened {
    /// Calling session.
    pub session: SessionId,
    /// The tool call (joins the card).
    pub call_id: String,
    /// Frame identity: first 16 hex of `sha256(session ‖ "\n" ‖ origin)` (205 D4).
    pub id: String,
    /// The normalised address.
    pub url: String,
    /// Tile and card title.
    pub title: String,
}

impl wire::WireEvent for WebViewOpened {
    const TOPIC: &'static str = "artifact.web-view-opened";
}

/// `show_web`: validate the address, publish [`WebViewOpened`], return the receipt (205 §4.1).
pub struct ShowWebTool {
    /// For publishing the event.
    pub rt: Runtime,
}

#[async_trait::async_trait]
impl Tool for ShowWebTool {
    const NAME: &'static str = "show_web";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "dict";
    const DIRECT: bool = true;
    type Args = ShowWebArgs;

    fn description(&self) -> String {
        "Open a live web page — a dev server on localhost, a dashboard, a docs site — in a tile \
         beside the chat that the user can click, scroll and pop out into its own window. \
         `url` is http(s) only (a bare `localhost:3000` gets `http://`); `title` names the tile \
         (default host[:port]). Showing the same origin again in this session reuses its tile \
         and navigates it. Use it for a page the user should see or use themselves; for web work you do on \
         their behalf use browser_* instead. You do not see the page unless you attach to it: \
         browser_attach {view: \"web:<id>\"} points your browser_* tools at this tile, where the \
         user watches you act; read_url reads it unseen; web_views tells you what the tile shows now. A site the user \
         signs into here is signed in for your browser_* too, on every engine of theirs, so for a \
         sign-in only they can do, open the sign-in page here and ask them to sign in. \
         Not for files — `show` hands those over."
            .into()
    }

    async fn call(&self, args: ShowWebArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let url = match check(&args.url, tools::egress::lan()) {
            Ok(u) => u,
            Err(msg) => return Ok(ToolOutput::err(format!("show_web: {msg}"))),
        };
        let title = args
            .title
            .map(|t| clean_title(&t))
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| host_port(&url));
        let id = frame_id(cx.session, &url);
        let url = url.to_string();
        opened(cx.session, &id, &url, &title);
        self.rt
            .publish(WebViewOpened {
                session: cx.session,
                call_id: cx.call_id.clone(),
                id: id.clone(),
                url: url.clone(),
                title: title.clone(),
            })
            .await;
        let content = receipt(&title, &url, &id, crate::preview::feed_live());
        Ok(ToolOutput {
            structured: Some(serde_json::json!({
                "id": id,
                "url": url,
                "title": title,
                "session": cx.session,
            })),
            ..ToolOutput::ok(content)
        })
    }
}

/// One tile's state as its client reports it: the params of the RPC `web_view_state`
/// (209 D9, §4.4). `view` names the tile as `<id>`, `web:<id>` or the shell's label `web-<id>`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct WebViewState {
    /// The session that opened the tile.
    pub session: SessionId,
    /// The tile.
    pub view: String,
    /// The page's current address.
    pub url: String,
    /// The page's current title.
    pub title: String,
    /// Whether a load is in progress.
    pub loading: bool,
    /// The user closed the tile: the engine forgets it (209 §4.4).
    #[serde(default)]
    pub closed: bool,
}

/// Result of `web_view_state`: `{}` (209 §4.4).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct WebViewStateReply {}

/// What the engine knows of one tile (209 D9).
#[derive(Clone, Debug)]
struct Tile {
    url: String,
    title: String,
    loading: bool,
    /// `false` while only `show_web` has spoken for it: no client reported it yet.
    reported: bool,
    at: Instant,
    /// ms since epoch of the latest `show_web` that opened or re-targeted it; `None` for a tile
    /// only a client reported (the engine restarted under an open tile). Client reports keep it.
    opened_ms: Option<u64>,
}

/// Tiles by session, then by id. Process-wide like `preview`'s pending map: the RPC handler and
/// the tool live in different actors and share nothing else.
static TILES: LazyLock<Mutex<HashMap<SessionId, BTreeMap<String, Tile>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Bumped whenever a tile opens, closes, is forgotten or reports another address or title: the
/// inbox (`roster::attention`) watches it to re-derive its `web_view` items.
static CHANGES: LazyLock<tokio::sync::watch::Sender<u64>> =
    LazyLock::new(|| tokio::sync::watch::channel(0).0);

fn changed() {
    CHANGES.send_modify(|n| *n = n.wrapping_add(1));
}

/// A receiver that sees every later change to the tiles (coalesced, like any `watch`).
#[must_use]
pub fn watch_tiles() -> tokio::sync::watch::Receiver<u64> {
    CHANGES.subscribe()
}

/// One tile a `show_web` opened and the user has not closed: the inbox's `web_view` item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenTile {
    /// The session that opened it.
    pub session: SessionId,
    /// Frame identity (205 D4).
    pub id: String,
    /// The page's address now (the client's report, else the `show_web` address); may be empty.
    pub url: String,
    /// The page's title now; may be empty.
    pub title: String,
    /// `host[:port]` of `url`; empty without an address.
    pub host: String,
    /// ms since epoch of the latest `show_web` for it.
    pub opened_ms: u64,
}

/// Every open tile a `show_web` opened (client-only tiles are left out), newest first.
#[must_use]
pub fn open_tiles() -> Vec<OpenTile> {
    let mut out: Vec<OpenTile> = TILES
        .lock()
        .iter()
        .flat_map(|(s, m)| {
            m.iter().filter_map(move |(id, t)| {
                Some(OpenTile {
                    session: *s,
                    id: id.clone(),
                    url: t.url.clone(),
                    title: t.title.clone(),
                    host: Url::parse(&t.url).map(|u| host_port(&u)).unwrap_or_default(),
                    opened_ms: t.opened_ms?,
                })
            })
        })
        .collect();
    out.sort_by(|a, b| b.opened_ms.cmp(&a.opened_ms).then_with(|| a.id.cmp(&b.id)));
    out
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Most tiles remembered across all sessions; the least recently updated goes first.
const MAX_TILES: usize = 512;

/// Record a client's report (the RPC `web_view_state`, 209 D9). A `view` that is not a tile id
/// is ignored; the address is kept only when it is http(s) and at most [`MAX_URL`] bytes. A
/// report with `closed` forgets the tile.
pub fn record_state(s: WebViewState) -> WebViewStateReply {
    let Some(id) = view_id(&s.view) else {
        return WebViewStateReply::default();
    };
    if s.closed {
        let mut all = TILES.lock();
        if let Some(m) = all.get_mut(&s.session) {
            m.remove(&id);
            if m.is_empty() {
                all.remove(&s.session);
            }
        }
        drop(all);
        changed();
        return WebViewStateReply::default();
    }
    let url = match Url::parse(&s.url) {
        Ok(u) if matches!(u.scheme(), "http" | "https") && s.url.len() <= MAX_URL => s.url,
        _ => String::new(),
    };
    let title = clean_title(&s.title);
    let (opened_ms, same) = TILES
        .lock()
        .get(&s.session)
        .and_then(|m| m.get(&id))
        .map_or((None, false), |t| (t.opened_ms, t.url == url && t.title == title));
    put(
        s.session,
        id,
        Tile {
            url,
            title,
            loading: s.loading,
            reported: true,
            at: Instant::now(),
            opened_ms,
        },
    );
    if !same {
        changed();
    }
    WebViewStateReply::default()
}

/// `show_web` opened or re-targeted tile `id`: known, not yet reported.
fn opened(session: SessionId, id: &str, url: &str, title: &str) {
    put(
        session,
        id.to_owned(),
        Tile {
            url: url.to_owned(),
            title: title.to_owned(),
            loading: true,
            reported: false,
            at: Instant::now(),
            opened_ms: Some(now_ms()),
        },
    );
    changed();
}

/// The sessions ended (deleted, or their tree closed): what was known of their tiles goes.
pub fn forget(sessions: &[SessionId]) {
    let mut all = TILES.lock();
    for s in sessions {
        all.remove(s);
    }
    drop(all);
    changed();
}

fn put(session: SessionId, id: String, tile: Tile) {
    let mut all = TILES.lock();
    all.entry(session).or_default().insert(id, tile);
    while all.values().map(BTreeMap::len).sum::<usize>() > MAX_TILES {
        let oldest = all
            .iter()
            .flat_map(|(s, m)| m.iter().map(move |(id, t)| (t.at, *s, id.clone())))
            .min_by_key(|(at, ..)| *at);
        let Some((_, s, id)) = oldest else { break };
        if let Some(m) = all.get_mut(&s) {
            m.remove(&id);
            if m.is_empty() {
                all.remove(&s);
            }
        }
    }
}

/// The tile id of `web:<id>`, `web-<id>` or `<id>`: 1..=64 of `[0-9A-Za-z_-]`.
#[must_use]
pub fn view_id(view: &str) -> Option<String> {
    let id = view
        .strip_prefix("web:")
        .or_else(|| view.strip_prefix("web-"))
        .unwrap_or(view);
    (!id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
    .then(|| id.to_owned())
}

/// The address tile `id` last showed, in whichever session reported it most recently (209 §4.6:
/// the `[tile web:<id> · <host>]` prefix before the first call answered).
#[must_use]
pub fn tile_url(id: &str) -> Option<String> {
    TILES
        .lock()
        .values()
        .filter_map(|m| m.get(id))
        .filter(|t| !t.url.is_empty())
        .max_by_key(|t| t.at)
        .map(|t| t.url.clone())
}

/// The sessions that opened tile `id` (its `show_web` ran in them, or a client reported it for
/// them) and are still known here (209 §9: `browser_attach` only within the opener's tree).
#[must_use]
pub fn openers(id: &str) -> Vec<SessionId> {
    TILES
        .lock()
        .iter()
        .filter(|(_, m)| m.contains_key(id))
        .map(|(s, _)| *s)
        .collect()
}

/// The session that opened tile `id` when it is `session` itself or a session of `session`'s
/// tree (the same [`agent::store::Store::root_of`]), 209 §9. `None`: no session of this tree
/// opened it (or it is closed, or unknown here).
pub async fn opener_in_tree(session: SessionId, id: &str) -> Option<SessionId> {
    let openers = openers(id);
    if openers.contains(&session) {
        return Some(session);
    }
    if openers.is_empty() {
        return None;
    }
    let agents = kameo::actor::ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
        .ok()
        .flatten()?;
    let store = agents.ask(agent::DurableStore).await.ok()?;
    let root = store.root_of(session).await.ok()?;
    for o in openers {
        if store.root_of(o).await.ok() == Some(root) {
            return Some(o);
        }
    }
    None
}

/// Correlates a [`WebViewCallRequested`] with its answers (209 D8).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(transparent)]
pub struct WebViewCallId(pub uuid::Uuid);

/// The op of a [`WebViewCallRequested`] (209 §4.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum WebViewOp {
    /// Run `args.source` in the isolated or the page world.
    Script,
    /// Load `args.url`.
    Navigate,
    /// History back.
    Back,
    /// History forward.
    Forward,
    /// Reload the page.
    Reload,
    /// The visible viewport as a PNG.
    Snapshot,
    /// The client's own op (the user pressed Stop): abandons the tile's waits. The engine never
    /// sends it.
    Stop,
}

impl WebViewOp {
    /// The wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Script => "script",
            Self::Navigate => "navigate",
            Self::Back => "back",
            Self::Forward => "forward",
            Self::Reload => "reload",
            Self::Snapshot => "snapshot",
            Self::Stop => "stop",
        }
    }
}

/// Event: an agent session wants one operation run in a web view tile (209 D8, §4.4, §4.7).
/// A client claims it only when its window holds the shell label `web-<view>` and the tile was
/// opened by `session`, or `delegated` is set.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct WebViewCallRequested {
    /// Use in `answer_web_view_call`.
    pub id: WebViewCallId,
    /// The agent session acting.
    pub session: SessionId,
    /// The bare tile id (the shell label is `web-<view>`).
    pub view: String,
    /// `true` when `session` is not the session that opened the tile: a session of the opener's
    /// tree attached to it (209 §9).
    #[serde(default)]
    pub delegated: bool,
    /// The op (209 §4.7).
    pub op: WebViewOp,
    /// The op's arguments (209 §4.7).
    #[cfg_attr(feature = "ts", ts(type = "unknown"))]
    pub args: serde_json::Value,
}

impl wire::WireEvent for WebViewCallRequested {
    const TOPIC: &'static str = "artifact.web-view-call-requested";
}

/// Event: the call was answered, failed, timed out or nobody claimed it; the tile's "agent is
/// acting" bar for it goes (209 §9).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct WebViewCallResolved {
    /// The request.
    pub id: WebViewCallId,
}

impl wire::WireEvent for WebViewCallResolved {
    const TOPIC: &'static str = "artifact.web-view-call-resolved";
}

/// A client's answer to a [`WebViewCallRequested`] (209 §4.4).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WebViewCallAnswer {
    /// "This window holds the tile and runs the op": the first one wins, later ones are refused.
    Claimed {
        /// Who runs it (free text, e.g. the client's install id; logs only).
        client: String,
    },
    /// The op ran. Taken only from the connection that claimed the call.
    Done {
        /// The op's JSON result.
        #[cfg_attr(feature = "ts", ts(type = "unknown"))]
        result: serde_json::Value,
        /// `snapshot`: the viewport as a base64 PNG.
        #[serde(default)]
        png: Option<String>,
    },
    /// The op could not run. Taken only from the claimer; before any claim only with a
    /// [`STOP_REASONS`] reason.
    Failed {
        /// Why, for the model.
        reason: String,
    },
}

/// What the user's Stop answers (209 §4.4): the engine refuses the tile for [`STOP_HOLD`].
pub const STOPPED: &str = "stopped by the user";
/// The `Failed` reasons a client may send before any claim: the user stopped the agent or closed
/// the window holding the tile. Nothing else of an unclaimed answer reaches the model.
pub const STOP_REASONS: &[&str] = &[STOPPED, "window closed"];
/// How long a tile the user stopped the agent on refuses calls.
pub const STOP_HOLD: Duration = Duration::from_secs(60);

/// How one [`call`] ended.
#[derive(Clone, Debug, PartialEq)]
pub enum CallOutcome {
    /// The client ran the op.
    Done {
        /// The op's JSON result.
        result: serde_json::Value,
        /// `snapshot`'s PNG, base64.
        png: Option<String>,
    },
    /// The client could not run it.
    Failed(String),
    /// No client claimed the call within [`CLAIM_WAIT`].
    NoClient,
    /// Claimed, but no `Done`/`Failed` within the op's budget.
    TimedOut,
}

/// No `Claimed` within this → [`CallOutcome::NoClient`] (209 §4.7).
pub const CLAIM_WAIT: Duration = Duration::from_secs(2);
/// The shortest budget of a claimed op (209 §4.7: 30 s, or the tool's own wait plus 5 s).
pub const OP_WAIT: Duration = Duration::from_secs(30);

/// One waiting call: where its answers go, and who claimed it ([`answer`]'s `by`).
struct Pending {
    tx: tokio::sync::mpsc::UnboundedSender<WebViewCallAnswer>,
    claimer: Option<String>,
}

/// Waiting calls of one engine, a [`Runtime`] extension: the tool and the RPC handler share the
/// runtime and nothing else.
#[derive(Clone, Default)]
struct Calls(std::sync::Arc<Mutex<HashMap<WebViewCallId, Pending>>>);

fn calls(rt: &Runtime) -> Calls {
    rt.extension(Calls::default)
}

/// Calls seen on this engine's event feed, local or relayed from the session's owner, by id →
/// session: the fallback route of an `answer_web_view_call` that names no session ([`note_feed`]).
static SEEN: LazyLock<Mutex<std::collections::VecDeque<(WebViewCallId, SessionId)>>> =
    LazyLock::new(|| Mutex::new(std::collections::VecDeque::new()));

/// Most calls remembered in [`SEEN`]; the oldest goes first.
const MAX_SEEN: usize = 1024;

/// Whether `id` waits on this engine.
#[must_use]
pub fn pending_here(rt: &Runtime, id: WebViewCallId) -> bool {
    calls(rt).0.lock().contains_key(&id)
}

/// The session a call seen on this engine's feed named, for forwarding its answer.
#[must_use]
pub fn session_of(id: WebViewCallId) -> Option<SessionId> {
    SEEN.lock()
        .iter()
        .rev()
        .find(|(i, _)| *i == id)
        .map(|(_, s)| *s)
}

/// Remember the calls this engine's event feed carries (the artifact plugin runs it on
/// `wire_events`): a requested call's session until it resolves.
pub fn note_feed(topic: &str, event: &serde_json::Value) {
    use wire::WireEvent as _;
    let id = || {
        serde_json::from_value::<WebViewCallId>(event.get("id").cloned().unwrap_or_default()).ok()
    };
    if topic == WebViewCallRequested::TOPIC {
        let session = event
            .get("session")
            .cloned()
            .and_then(|s| serde_json::from_value::<SessionId>(s).ok());
        if let (Some(id), Some(session)) = (id(), session) {
            let mut seen = SEEN.lock();
            seen.push_back((id, session));
            while seen.len() > MAX_SEEN {
                seen.pop_front();
            }
        }
    } else if topic == WebViewCallResolved::TOPIC
        && let Some(id) = id()
    {
        SEEN.lock().retain(|(i, _)| *i != id);
    }
}

/// Run [`note_feed`] over this engine's wire feed until shutdown (the artifact plugin starts it).
pub fn watch_calls(rt: &Runtime) {
    use tokio::sync::broadcast::error::RecvError;
    use wire::RuntimeWire as _;
    let mut rx = rt.wire_events();
    let shutdown = rt.shutdown.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = shutdown.cancelled() => return,
                frame = rx.recv() => match frame {
                    Ok(f) => {
                        if let Some(d) = &f.descriptor {
                            note_feed(d.topic, &f.event);
                        }
                    }
                    Err(RecvError::Lagged(n)) => {
                        tracing::debug!(lost = n, "web view calls: wire feed lagged");
                    }
                    Err(RecvError::Closed) => return,
                },
            }
        }
    });
}

/// A client's answer on this engine, sent by `by` (the RPC connection, 209 §4.4). `true` while the
/// call was pending and the answer was taken: the first `Claimed` is taken and binds the call to
/// `by`, every later one is refused (`false`); `Done`/`Failed` resolve it only from `by`. Before
/// any claim only a `Failed` with a [`STOP_REASONS`] reason is taken, from anyone.
pub fn answer(rt: &Runtime, id: WebViewCallId, answer: WebViewCallAnswer, by: &str) -> bool {
    let calls = calls(rt);
    let mut map = calls.0.lock();
    let Some(p) = map.get_mut(&id) else {
        return false;
    };
    let taken = match (&answer, p.claimer.as_deref()) {
        (WebViewCallAnswer::Claimed { .. }, Some(_)) => return false,
        (WebViewCallAnswer::Claimed { .. }, None) => {
            p.claimer = Some(by.to_owned());
            return p.tx.send(answer).is_ok();
        }
        (_, Some(claimer)) => claimer == by,
        (WebViewCallAnswer::Failed { reason }, None) => STOP_REASONS.contains(&reason.as_str()),
        (WebViewCallAnswer::Done { .. }, None) => false,
    };
    if !taken {
        return false;
    }
    let delivered = p.tx.send(answer).is_ok();
    map.remove(&id);
    delivered
}

/// Front door (209 §4.4): calls another engine issued that this engine forwarded a winning
/// `Claimed` for, by id → the local connection that claimed. The owner binds the call to this
/// engine; this map binds it to the connection, so another client here cannot answer it.
static RELAYED_CLAIMS: LazyLock<Mutex<std::collections::VecDeque<(WebViewCallId, String)>>> =
    LazyLock::new(|| Mutex::new(std::collections::VecDeque::new()));

/// Whether a front door may forward `answer` from `by` for call `id`: a claim always (the owner
/// decides), a `Done`/`Failed` unless another local connection holds the claim.
#[must_use]
pub fn relay_allows(id: WebViewCallId, answer: &WebViewCallAnswer, by: &str) -> bool {
    matches!(answer, WebViewCallAnswer::Claimed { .. })
        || RELAYED_CLAIMS
            .lock()
            .iter()
            .find(|(i, _)| *i == id)
            .is_none_or(|(_, c)| c == by)
}

/// A front door forwarded `answer` from `by` for call `id` and the owner said `taken`: a taken
/// claim binds the call to `by`; the claimer's `Done`/`Failed` ends the binding.
pub fn relayed(id: WebViewCallId, answer: &WebViewCallAnswer, by: &str, taken: bool) {
    let mut claims = RELAYED_CLAIMS.lock();
    match answer {
        WebViewCallAnswer::Claimed { .. } if taken => {
            claims.push_back((id, by.to_owned()));
            while claims.len() > MAX_SEEN {
                claims.pop_front();
            }
        }
        WebViewCallAnswer::Claimed { .. } => {}
        _ => claims.retain(|(i, c)| !(*i == id && c == by)),
    }
}

/// One round trip (209 D8): publish [`WebViewCallRequested`], wait [`CLAIM_WAIT`] for a
/// `Claimed`, then `work` (at least [`OP_WAIT`]) from the claim for `Done`/`Failed`, publish
/// [`WebViewCallResolved`]. `view` is the bare tile id; `delegated` as in the event.
pub async fn call(
    rt: &Runtime,
    session: SessionId,
    view: &str,
    delegated: bool,
    op: WebViewOp,
    args: serde_json::Value,
    work: Duration,
) -> CallOutcome {
    let id = WebViewCallId(uuid::Uuid::now_v7());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let pending = calls(rt);
    pending.0.lock().insert(id, Pending { tx, claimer: None });
    let mut guard = Unresolved {
        rt: Some(rt.clone()),
        id,
    };
    rt.publish(WebViewCallRequested {
        id,
        session,
        view: view.to_owned(),
        delegated,
        op,
        args,
    })
    .await;
    let mut deadline = tokio::time::Instant::now() + CLAIM_WAIT;
    let mut claimed = false;
    let out = loop {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Some(WebViewCallAnswer::Claimed { .. })) => {
                claimed = true;
                deadline = tokio::time::Instant::now() + work.max(OP_WAIT);
            }
            Ok(Some(WebViewCallAnswer::Done { result, png })) => {
                break CallOutcome::Done { result, png };
            }
            Ok(Some(WebViewCallAnswer::Failed { reason })) => break CallOutcome::Failed(reason),
            // The sender lives in the pending map until a terminal answer, sent before removal.
            Ok(None) => break CallOutcome::TimedOut,
            Err(_) if claimed => break CallOutcome::TimedOut,
            Err(_) => break CallOutcome::NoClient,
        }
    };
    pending.0.lock().remove(&id);
    guard.rt = None;
    rt.publish(WebViewCallResolved { id }).await;
    out
}

/// A [`call`] dropped before it resolved (the tool's timeout or cancel): its pending entry goes
/// and `WebViewCallResolved` is still published, so no tile keeps its "agent is acting" bar. The
/// publish is a non-blocking send to the bus, so it needs no async context.
struct Unresolved {
    rt: Option<Runtime>,
    id: WebViewCallId,
}

impl Drop for Unresolved {
    fn drop(&mut self) {
        use runtime::kameo_actors::message_bus::Publish;
        let Some(rt) = self.rt.take() else { return };
        calls(&rt).0.lock().remove(&self.id);
        let published = rt
            .bus
            .tell(Publish(WebViewCallResolved { id: self.id }))
            .try_send();
        if published.is_err() {
            tracing::debug!("web view call resolution not published: the bus is gone or full");
        }
    }
}

/// Arguments of `web_views`: none.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct WebViewsArgs {}

/// `web_views {}`: the calling session's tiles and what each shows now (209 D9).
pub struct WebViewsTool;

#[async_trait::async_trait]
impl Tool for WebViewsTool {
    const NAME: &'static str = "web_views";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "list[dict]";
    type Args = WebViewsArgs;

    fn description(&self) -> String {
        "Where the user is: the web view tiles this session opened with show_web and what each \
         shows now, as the user's app reports it — one line per tile, `web:<id> · <title> · \
         <url> · loading|loaded · <age>`. A tile no app has reported since show_web opened it \
         says so; then the address is the one you opened, not necessarily the page now."
            .into()
    }

    async fn call(&self, _: WebViewsArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let tiles: Vec<(String, Tile)> = TILES
            .lock()
            .get(&cx.session)
            .map(|m| m.iter().map(|(id, t)| (id.clone(), t.clone())).collect())
            .unwrap_or_default();
        if tiles.is_empty() {
            return Ok(ToolOutput::ok(
                "no web view tiles in this session; open one with show_web",
            ));
        }
        let now = Instant::now();
        let lines: Vec<String> = tiles
            .iter()
            .map(|(id, t)| tile_line(id, t, now.saturating_duration_since(t.at)))
            .collect();
        let structured: Vec<serde_json::Value> = tiles
            .iter()
            .map(|(id, t)| {
                serde_json::json!({
                    "view": format!("web:{id}"),
                    "url": t.url,
                    "title": t.title,
                    "loading": t.loading,
                    "reported": t.reported,
                    "age_secs": now.saturating_duration_since(t.at).as_secs(),
                })
            })
            .collect();
        Ok(ToolOutput {
            structured: Some(structured.into()),
            ..ToolOutput::ok(lines.join("\n"))
        })
    }
}

/// One line of `web_views`.
fn tile_line(id: &str, t: &Tile, age: Duration) -> String {
    let state = match (t.reported, t.loading) {
        (false, _) => "opened, not reported by an app yet",
        (true, true) => "loading",
        (true, false) => "loaded",
    };
    let title = if t.title.is_empty() {
        "(untitled)"
    } else {
        &t.title
    };
    let url = if t.url.is_empty() {
        "(no address)"
    } else {
        &t.url
    };
    format!(
        "web:{id} · {title} · {url} · {state} · {}s ago",
        age.as_secs()
    )
}

/// The text the model reads (205 §4.1).
fn receipt(title: &str, url: &str, id: &str, live: bool) -> String {
    let mut s = format!("web view opened · {title} · {url} · tile web:{id}");
    if !live {
        s.push_str("\nnote: no app is connected; the card in the chat opens it later");
    }
    let _ = write!(
        s,
        "\nThe user sees and clicks this page; you do not. To read or act on it in front of them, \
         browser_attach {{\"view\": \"web:{id}\"}}; read_url reads it unseen."
    );
    s
}

/// 205 D4: first 16 hex of `sha256(session ‖ "\n" ‖ origin)`.
fn frame_id(session: SessionId, url: &Url) -> String {
    let mut h = Sha256::new();
    h.update(session.to_string().as_bytes());
    h.update(b"\n");
    h.update(url.origin().ascii_serialization().as_bytes());
    let mut id = hex::encode(h.finalize());
    id.truncate(16);
    id
}

/// `host[:port]`, the port only when it is not the scheme's default.
fn host_port(url: &Url) -> String {
    let host = url.host_str().unwrap_or_default();
    match url.port() {
        Some(p) => format!("{host}:{p}"),
        None => host.to_owned(),
    }
}

/// Longest title kept, in chars (the `…` included).
const MAX_TITLE: usize = 120;
/// Longest accepted address, in bytes.
const MAX_URL: usize = 2048;

/// One line: control characters and whitespace runs become one space, trimmed, capped at
/// [`MAX_TITLE`] chars with `…` when cut. The receipt's first line must stay one line.
pub(crate) fn clean_title(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        if c.is_whitespace() || c.is_control() {
            if !out.is_empty() && !out.ends_with(' ') {
                out.push(' ');
            }
        } else {
            out.push(c);
        }
    }
    let out = out.trim_end();
    if out.chars().count() <= MAX_TITLE {
        return out.to_owned();
    }
    let mut cut: String = out.chars().take(MAX_TITLE - 1).collect();
    cut.truncate(cut.trim_end().len());
    cut.push('…');
    cut
}

/// Whether `s` starts with `scheme://` (RFC 3986 scheme characters).
fn has_scheme(s: &str) -> bool {
    let Some((scheme, _)) = s.split_once("://") else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// Normalise and validate `raw` (205 §4.1, D8). `lan` is the call's local-network grant.
fn check(raw: &str, lan: bool) -> Result<Url, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("url is empty".into());
    }
    if raw.len() > MAX_URL {
        return Err(format!(
            "url is {} bytes; at most {MAX_URL} are allowed",
            raw.len()
        ));
    }
    // `localhost:3000` parses as scheme `localhost`; only an explicit `scheme://` prefix is kept.
    let url = if has_scheme(raw) {
        Url::parse(raw)
    } else {
        Url::parse(&format!("http://{raw}"))
    }
    .map_err(|e| format!("not a valid URL ({e}): {raw}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!("only http and https URLs (got {}:)", url.scheme()));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("credentials in the URL are not allowed".into());
    }
    let local = match url.host() {
        None => return Err("the URL has no host".into()),
        Some(Host::Domain(d)) => local_name(d),
        Some(Host::Ipv4(ip)) => classify(IpAddr::V4(ip))?,
        Some(Host::Ipv6(ip)) => classify(IpAddr::V6(ip))?,
    };
    if local && !lan {
        return Err("local network not allowed for this call".into());
    }
    Ok(url)
}

/// Suffixes of names that only resolve on a local network.
const LOCAL_SUFFIXES: &[&str] = &[
    ".localhost",
    ".local",
    ".lan",
    ".home.arpa",
    ".internal",
    ".localdomain",
    ".intranet",
    ".corp",
    ".home",
];

/// A single-label name (`nas`, `localhost`) or one under a local-only suffix.
fn local_name(d: &str) -> bool {
    let d = d.trim_end_matches('.').to_ascii_lowercase();
    !d.contains('.') || LOCAL_SUFFIXES.iter().any(|s| d.ends_with(s))
}

/// `Ok(true)` for a local-network address, `Err` for a link-local one (always refused, also
/// when embedded in an IPv4-mapped, NAT64 or 6to4 address).
fn classify(ip: IpAddr) -> Result<bool, String> {
    let embedded = match ip {
        IpAddr::V4(v4) => Some(v4),
        IpAddr::V6(v6) => embedded_v4(v6),
    };
    let link_local = match ip {
        IpAddr::V4(v4) => v4.is_link_local(),
        IpAddr::V6(v6) => v6.is_unicast_link_local(),
    } || embedded.is_some_and(|v4| v4.is_link_local());
    if link_local {
        return Err(format!("link-local address {ip} is not allowed"));
    }
    Ok(match (ip, embedded) {
        (_, Some(v4)) => local_v4(v4),
        (IpAddr::V6(v6), None) => local_v6(v6),
        (IpAddr::V4(_), None) => unreachable!("an IPv4 address embeds itself"),
    })
}

/// The IPv4 address carried by an IPv4-mapped (`::ffff:0:0/96`), NAT64 (`64:ff9b::/96`) or
/// 6to4 (`2002::/16`) address.
fn embedded_v4(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    let s = ip.segments();
    let v4 = |hi: u16, lo: u16| Ipv4Addr::from((u32::from(hi) << 16) | u32::from(lo));
    if let Some(v4) = ip.to_ipv4_mapped() {
        Some(v4)
    } else if s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        Some(v4(s[6], s[7]))
    } else if s[0] == 0x2002 {
        Some(v4(s[1], s[2]))
    } else {
        None
    }
}

fn local_v4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_broadcast()
        || ip.is_multicast() // 224.0.0.0/4
        || o[0] == 0 // 0.0.0.0/8
        || (o[0] == 100 && (64..128).contains(&o[1])) // CGNAT 100.64.0.0/10
        || (o[0] == 198 && (18..20).contains(&o[1])) // 198.18.0.0/15
        || (o[0], o[1], o[2]) == (192, 0, 0) // 192.0.0.0/24
}

fn local_v6(ip: Ipv6Addr) -> bool {
    ip.is_loopback() || ip.is_unspecified() || ip.is_unique_local() || ip.is_multicast()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_host_gets_http() {
        let u = check("localhost:3000", true).unwrap();
        assert_eq!(u.as_str(), "http://localhost:3000/");
        let u = check(" 127.0.0.1:3000/x ", true).unwrap();
        assert_eq!(u.as_str(), "http://127.0.0.1:3000/x");
        let u = check("https://example.com/docs?q=1", false).unwrap();
        assert_eq!(u.as_str(), "https://example.com/docs?q=1");
        // `://` later in the address is not a scheme.
        let u = check("localhost:3000/?u=http://x", true).unwrap();
        assert_eq!(u.as_str(), "http://localhost:3000/?u=http://x");
        let u = check("example.com/p?next=https://x", false).unwrap();
        assert_eq!(u.as_str(), "http://example.com/p?next=https://x");
    }

    #[test]
    fn refusals() {
        for bad in [
            "ftp://example.com/x",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "https://user:pw@example.com/",
            "https://user@example.com/",
            "http://",
            "",
            "http://169.254.169.254/latest/meta-data",
            "http://[fe80::1]/",
            "http://[::ffff:169.254.169.254]/",
        ] {
            assert!(check(bad, true).is_err(), "{bad} accepted");
        }
        assert!(
            check("ftp://example.com/", true)
                .unwrap_err()
                .contains("http")
        );
        assert!(
            check("http://a:b@example.com/", true)
                .unwrap_err()
                .contains("credentials")
        );
        assert!(
            check("169.254.169.254", true)
                .unwrap_err()
                .contains("link-local")
        );
        let long = format!("https://example.com/{}", "a".repeat(MAX_URL));
        assert!(check(&long, false).unwrap_err().contains("at most 2048"));
        let edge = format!("https://example.com/{}", "a".repeat(MAX_URL - 20));
        assert_eq!(edge.len(), MAX_URL);
        assert!(check(&edge, false).is_ok());
    }

    #[test]
    fn titles_stay_one_line() {
        assert_eq!(clean_title("  Dev\nserver \r\n\t 3000 "), "Dev server 3000");
        assert_eq!(clean_title("bell\u{7}here"), "bell here");
        assert_eq!(clean_title("\u{7}\n"), "");
        let long = clean_title(&"é".repeat(500));
        assert_eq!(long.chars().count(), MAX_TITLE);
        assert!(long.ends_with('…'));
        assert_eq!(clean_title(&"x".repeat(MAX_TITLE)), "x".repeat(MAX_TITLE));
        let r = receipt(
            &clean_title("a\nb · c"),
            "http://x/",
            "0123456789abcdef",
            true,
        );
        let first = r.lines().next().unwrap();
        assert_eq!(
            first,
            "web view opened · a b · c · http://x/ · tile web:0123456789abcdef"
        );
    }

    #[test]
    fn lan_classification() {
        #[derive(Clone, Copy, Debug, PartialEq)]
        enum K {
            Public,
            Local,
            LinkLocal,
        }
        use K::*;
        let table = [
            ("https://example.com/", Public),
            ("http://8.8.8.8/", Public),
            ("http://198.20.0.1/", Public),
            ("http://192.0.1.1/", Public),
            ("http://[2606:4700::1111]/", Public),
            ("http://[64:ff9b::808:808]/", Public),
            ("http://[2002:808:808::1]/", Public),
            ("localhost:3000", Local),
            ("http://app.localhost/", Local),
            ("http://printer.local/", Local),
            ("http://nas/", Local),
            ("router:8080", Local),
            ("http://nas.lan/", Local),
            ("http://hub.home.arpa/", Local),
            ("http://svc.internal/", Local),
            ("http://box.localdomain/", Local),
            ("http://wiki.intranet/", Local),
            ("http://git.corp/", Local),
            ("http://tv.home/", Local),
            ("http://127.0.0.1:8080/", Local),
            ("http://10.0.0.2/", Local),
            ("http://172.16.0.1/", Local),
            ("http://192.168.1.1/", Local),
            ("http://100.64.1.1/", Local),
            ("http://0.0.0.0/", Local),
            ("http://0.1.2.3/", Local),
            ("http://198.18.0.1/", Local),
            ("http://198.19.255.1/", Local),
            ("http://192.0.0.8/", Local),
            ("http://224.0.0.1/", Local),
            ("http://239.1.1.1/", Local),
            ("http://255.255.255.255/", Local),
            ("http://[::1]:3000/", Local),
            ("http://[::]/", Local),
            ("http://[fd00::1]/", Local),
            ("http://[ff02::1]/", Local),
            ("http://[::ffff:10.0.0.1]/", Local),
            ("http://[64:ff9b::a00:1]/", Local),
            ("http://[2002:c0a8:101::1]/", Local),
            ("http://169.254.169.254/", LinkLocal),
            ("http://169.254.0.1/", LinkLocal),
            ("http://[fe80::1]/", LinkLocal),
            ("http://[::ffff:169.254.169.254]/", LinkLocal),
            ("http://[64:ff9b::a9fe:a9fe]/", LinkLocal),
            ("http://[2002:a9fe:a9fe::1]/", LinkLocal),
        ];
        for (url, kind) in table {
            let (with, without) = (check(url, true), check(url, false));
            match kind {
                Public => assert!(with.is_ok() && without.is_ok(), "{url}: {without:?}"),
                Local => {
                    assert!(with.is_ok(), "{url} refused with lan: {with:?}");
                    let e = without.unwrap_err();
                    assert!(e.contains("local network"), "{url} without lan: {e}");
                }
                LinkLocal => {
                    for r in [with, without] {
                        let e = r.unwrap_err();
                        assert!(e.contains("link-local"), "{url}: {e}");
                    }
                }
            }
        }
    }

    #[tokio::test]
    async fn lan_grant_comes_from_the_egress_scope() {
        let refused = tools::egress::scope(false, async {
            check("localhost:3000", tools::egress::lan())
        })
        .await;
        assert!(refused.is_err());
        let allowed = tools::egress::scope(true, async {
            check("localhost:3000", tools::egress::lan())
        })
        .await;
        assert_eq!(allowed.unwrap().as_str(), "http://localhost:3000/");
    }

    #[test]
    fn id_per_origin_and_session() {
        let a = SessionId::new();
        let b = SessionId::new();
        let u = |s: &str| check(s, true).unwrap();
        let id = frame_id(a, &u("http://localhost:3000/"));
        assert_eq!(id.len(), 16);
        assert!(
            id.bytes()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
        assert_eq!(id, frame_id(a, &u("localhost:3000/other?x=1")));
        assert_ne!(id, frame_id(a, &u("http://localhost:3001/")));
        assert_ne!(id, frame_id(b, &u("http://localhost:3000/")));
        let mut h = Sha256::new();
        h.update(format!("{a}\nhttp://localhost:3000").as_bytes());
        assert_eq!(id, hex::encode(h.finalize())[..16]);
    }

    #[test]
    fn default_title_is_host_port() {
        assert_eq!(
            host_port(&check("localhost:3000", true).unwrap()),
            "localhost:3000"
        );
        assert_eq!(
            host_port(&check("https://example.com:443/", true).unwrap()),
            "example.com"
        );
        assert_eq!(
            host_port(&check("http://[::1]:8080/", true).unwrap()),
            "[::1]:8080"
        );
    }

    #[test]
    fn receipt_text() {
        let live = receipt("Docs", "http://localhost:3000/", "0123456789abcdef", true);
        assert_eq!(
            live,
            "web view opened · Docs · http://localhost:3000/ · tile web:0123456789abcdef\n\
             The user sees and clicks this page; you do not. To read or act on it in front of them, browser_attach {\"view\": \"web:0123456789abcdef\"}; read_url reads it unseen."
        );
        let offline = receipt("Docs", "http://localhost:3000/", "0123456789abcdef", false);
        assert_eq!(
            offline,
            "web view opened · Docs · http://localhost:3000/ · tile web:0123456789abcdef\n\
             note: no app is connected; the card in the chat opens it later\n\
             The user sees and clicks this page; you do not. To read or act on it in front of them, browser_attach {\"view\": \"web:0123456789abcdef\"}; read_url reads it unseen."
        );
    }

    #[test]
    fn event_wire_shape() {
        let session = SessionId::new();
        let v = serde_json::to_value(WebViewOpened {
            session,
            call_id: "c".into(),
            id: "0123456789abcdef".into(),
            url: "http://localhost:3000/".into(),
            title: "localhost:3000".into(),
        })
        .unwrap();
        assert_eq!(v["type"], "WebViewOpened");
        assert_eq!(v["session"], serde_json::json!(session));
        assert_eq!(v.as_object().unwrap().len(), 6);
    }

    fn cx(session: SessionId) -> ToolCx {
        ToolCx {
            session,
            call_id: "c".into(),
            quiet: false,
            resources: tools::SessionResources::durable_arc(std::path::Path::new("/tmp")),
            cancel: tools::CancellationToken::new(),
            promote_after: None,
        }
    }

    #[test]
    fn view_ids() {
        assert_eq!(view_id("web:0123abcd").as_deref(), Some("0123abcd"));
        assert_eq!(view_id("web-0123abcd").as_deref(), Some("0123abcd"));
        assert_eq!(view_id("0123abcd").as_deref(), Some("0123abcd"));
        for bad in ["", "web:", "web:a b", "web:../x", &"a".repeat(65)] {
            assert_eq!(view_id(bad), None, "{bad}");
        }
    }

    /// 205 inbox: `open_tiles` lists what `show_web` opened with its open time, follows the client's
    /// reports for address and title without moving that time, leaves out client-only tiles, drops
    /// closed ones, and every such change is seen by `watch_tiles`.
    #[test]
    fn open_tiles_follow_show_web_reports_and_close() {
        let s = SessionId::new();
        let mine = || open_tiles().into_iter().filter(|t| t.session == s).collect::<Vec<_>>();
        let mut rx = watch_tiles();
        rx.borrow_and_update();
        opened(s, "00000000000000c1", "http://localhost:5173/", "localhost:5173");
        assert!(rx.has_changed().unwrap());
        let first = mine();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].host, "localhost:5173");
        let at = first[0].opened_ms;
        assert!(at > 0);

        rx.borrow_and_update();
        let report = |view: &str, url: &str, title: &str, closed: bool| {
            record_state(WebViewState {
                session: s,
                view: view.into(),
                url: url.into(),
                title: title.into(),
                loading: false,
                closed,
            });
        };
        report("web-00000000000000c1", "http://localhost:5173/cart", "Cart", false);
        assert!(rx.has_changed().unwrap());
        let t = &mine()[0];
        assert_eq!((t.url.as_str(), t.title.as_str(), t.opened_ms), ("http://localhost:5173/cart", "Cart", at));
        // The same page reported again keeps its open time.
        report("web-00000000000000c1", "http://localhost:5173/cart", "Cart", false);
        assert_eq!(mine()[0].opened_ms, at);
        // A tile only a client knows (the engine restarted under it) makes no item.
        report("web:00000000000000c2", "http://localhost:8080/", "Other", false);
        assert_eq!(mine().len(), 1);
        report("web:00000000000000c1", "", "", true);
        assert!(mine().is_empty());
    }

    /// 209 D9: `show_web` seeds the tile, the client's `web_view_state` replaces it, and
    /// `web_views` lists only the calling session's tiles.
    #[tokio::test]
    async fn web_views_lists_reported_state_per_session() {
        let (a, b) = (SessionId::new(), SessionId::new());
        let out = Tool::call(&WebViewsTool, WebViewsArgs {}, cx(a))
            .await
            .unwrap();
        assert!(
            out.content.starts_with("no web view tiles"),
            "{}",
            out.content
        );

        opened(a, "00000000000000aa", "http://localhost:3000/", "dev");
        let out = Tool::call(&WebViewsTool, WebViewsArgs {}, cx(a))
            .await
            .unwrap();
        assert!(
            out.content.starts_with(
                "web:00000000000000aa · dev · http://localhost:3000/ · opened, not reported"
            ),
            "{}",
            out.content
        );

        record_state(WebViewState {
            session: a,
            view: "web-00000000000000aa".into(),
            url: "http://localhost:3000/orders".into(),
            title: "Orders\n".into(),
            loading: false,
            closed: false,
        });
        // Not an http(s) page: the address is dropped, the rest kept.
        record_state(WebViewState {
            session: a,
            view: "web:00000000000000bb".into(),
            url: "javascript:alert(1)".into(),
            title: String::new(),
            loading: true,
            closed: false,
        });
        // Not a tile id: ignored.
        record_state(WebViewState {
            session: a,
            view: "web:../etc".into(),
            url: "http://x/".into(),
            title: "x".into(),
            loading: false,
            closed: false,
        });
        let out = Tool::call(&WebViewsTool, WebViewsArgs {}, cx(a))
            .await
            .unwrap();
        let lines: Vec<&str> = out.content.lines().collect();
        assert_eq!(lines.len(), 2, "{}", out.content);
        assert!(
            lines[0].starts_with(
                "web:00000000000000aa · Orders · http://localhost:3000/orders · loaded · "
            ),
            "{}",
            lines[0]
        );
        assert!(
            lines[1].starts_with("web:00000000000000bb · (untitled) · (no address) · loading · "),
            "{}",
            lines[1]
        );
        let s = out.structured.unwrap();
        assert_eq!(s[0]["view"], "web:00000000000000aa");
        assert_eq!(s[0]["reported"], true);
        // 209 §4.4: a tile the user closed is forgotten: no line, no address for the prefix.
        record_state(WebViewState {
            session: a,
            view: "web:00000000000000bb".into(),
            url: String::new(),
            title: String::new(),
            loading: false,
            closed: true,
        });
        let out = Tool::call(&WebViewsTool, WebViewsArgs {}, cx(a))
            .await
            .unwrap();
        assert_eq!(out.content.lines().count(), 1, "{}", out.content);
        assert!(openers("00000000000000bb").is_empty());
        assert_eq!(openers("00000000000000aa"), vec![a]);

        let out = Tool::call(&WebViewsTool, WebViewsArgs {}, cx(b))
            .await
            .unwrap();
        assert!(
            out.content.starts_with("no web view tiles"),
            "{}",
            out.content
        );
    }

    #[test]
    fn state_reply_is_an_empty_object() {
        assert_eq!(
            serde_json::to_value(WebViewStateReply::default()).unwrap(),
            serde_json::json!({})
        );
    }
}

#[cfg(test)]
mod answer_tests {
    use super::*;

    /// The ids waiting on `rt`, in no order.
    fn waiting(rt: &Runtime) -> Vec<WebViewCallId> {
        calls(rt).0.lock().keys().copied().collect()
    }

    /// A call on `rt` running on its own task, and its id once published.
    async fn start(rt: &Runtime) -> (tokio::task::JoinHandle<CallOutcome>, WebViewCallId) {
        let before = waiting(rt);
        let r = rt.clone();
        let task = tokio::spawn(async move {
            call(
                &r,
                SessionId::new(),
                "0123456789abcdef",
                false,
                WebViewOp::Snapshot,
                serde_json::json!({}),
                Duration::ZERO,
            )
            .await
        });
        loop {
            if let Some(id) = waiting(rt).into_iter().find(|i| !before.contains(i)) {
                return (task, id);
            }
            tokio::task::yield_now().await;
        }
    }

    fn claimed() -> WebViewCallAnswer {
        WebViewCallAnswer::Claimed { client: "x".into() }
    }

    fn failed(reason: &str) -> WebViewCallAnswer {
        WebViewCallAnswer::Failed {
            reason: reason.into(),
        }
    }

    /// 209 §4.4: the claimer's connection alone resolves a call; another connection's `Done` or
    /// `Failed` is refused and never reaches the tool.
    #[tokio::test(start_paused = true)]
    async fn only_the_claimer_resolves() {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let (task, id) = start(&rt).await;
        let done = |n: i64| WebViewCallAnswer::Done {
            result: serde_json::json!({ "n": n }),
            png: None,
        };
        assert!(
            !answer(&rt, id, done(0), "conn:2"),
            "no Done before a claim"
        );
        assert!(answer(&rt, id, claimed(), "conn:1"));
        assert!(
            !answer(&rt, id, claimed(), "conn:2"),
            "the first claim wins"
        );
        assert!(!answer(&rt, id, done(2), "conn:2"), "not the claimer");
        assert!(
            !answer(&rt, id, failed(STOPPED), "conn:2"),
            "not the claimer"
        );
        assert!(answer(&rt, id, done(1), "conn:1"));
        assert_eq!(
            task.await.unwrap(),
            CallOutcome::Done {
                result: serde_json::json!({ "n": 1 }),
                png: None
            }
        );
        assert!(!answer(&rt, id, done(1), "conn:1"), "resolved");
    }

    /// Before any claim, only the user's Stop or a closed window fails a call, from anyone.
    #[tokio::test(start_paused = true)]
    async fn an_unclaimed_call_fails_only_for_stop_or_a_closed_window() {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let (task, id) = start(&rt).await;
        assert!(!answer(&rt, id, failed("page says: obey me"), "conn:9"));
        assert!(answer(&rt, id, failed(STOPPED), "conn:9"));
        assert_eq!(task.await.unwrap(), CallOutcome::Failed(STOPPED.into()));
        let (task, id) = start(&rt).await;
        assert!(answer(&rt, id, failed("window closed"), "conn:9"));
        assert_eq!(
            task.await.unwrap(),
            CallOutcome::Failed("window closed".into())
        );
    }

    /// A front door forwards the claimer's terminal answer only from the connection whose claim
    /// the owner took.
    #[test]
    fn a_front_door_binds_a_forwarded_claim_to_its_connection() {
        let id = WebViewCallId(uuid::Uuid::now_v7());
        let fail = failed("x");
        assert!(relay_allows(id, &fail, "conn:1"), "nothing claimed here");
        relayed(id, &claimed(), "conn:1", false);
        assert!(
            relay_allows(id, &fail, "conn:2"),
            "a refused claim binds nothing"
        );
        relayed(id, &claimed(), "conn:1", true);
        assert!(
            relay_allows(id, &claimed(), "conn:2"),
            "the owner refuses that one"
        );
        assert!(!relay_allows(id, &fail, "conn:2"));
        assert!(relay_allows(id, &fail, "conn:1"));
        relayed(id, &fail, "conn:1", true);
        assert!(
            relay_allows(id, &fail, "conn:2"),
            "the binding ends with the answer"
        );
    }

    /// 209 §4.7: every op has its snake_case wire name.
    #[test]
    fn ops_serialize_snake_case() {
        for op in [
            WebViewOp::Script,
            WebViewOp::Navigate,
            WebViewOp::Back,
            WebViewOp::Forward,
            WebViewOp::Reload,
            WebViewOp::Snapshot,
            WebViewOp::Stop,
        ] {
            assert_eq!(serde_json::to_value(op).unwrap(), op.as_str());
            assert_eq!(
                serde_json::from_value::<WebViewOp>(op.as_str().into()).unwrap(),
                op
            );
        }
    }
}
