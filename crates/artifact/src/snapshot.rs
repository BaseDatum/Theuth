//! Fence snapshots (design doc 164 D3–D6): a connected desktop/web client draws a picture
//! fence as a PNG for the Signal, Telegram and Slack adapters. Same shape as `page_preview`
//! ([`crate::preview`]): publish [`FenceSnapshotRequested`], wait for answers through
//! [`answer`] (RPC `answer_fence_snapshot`), publish [`FenceSnapshotResolved`]. [`render`] is
//! one round trip; [`paint`] splits a channel message into text and pictures, drawing every
//! fence at once and standing the D7 text in for what nobody drew.
//!
//! The cache (D5) and the no-renderer back-off (D3) belong to one engine: they are keyed by
//! the runtime's bus, so every clone of the engine's `Runtime` shares them.

use std::collections::{HashMap, VecDeque};
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::Duration;

use base64::Engine as _;
use kameo::actor::ActorId;
use runtime::Runtime;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use surface::{PictureFence, PictureLang, PicturePart};
use tokio::sync::mpsc;
use tokio::time::Instant;

pub use crate::preview::{Appearance, Client};

/// No `Started` within this → no renderer (D3).
const NO_RENDERER: Duration = Duration::from_secs(3);
/// No terminal answer within this of the request → timed out (D3).
const TOTAL: Duration = Duration::from_secs(20);
/// After a [`SnapshotOutcome::NoRenderer`] window, requests skip straight to the fallback
/// for this long; any answer of that engine ends it early (D3).
const BACKOFF: Duration = Duration::from_secs(60);
/// `done` PNGs kept per engine (D5).
const CACHE: usize = 64;
/// Resolved request ids remembered per engine, so a late answer still ends the back-off.
const RECENT: usize = 64;
/// Largest PNG accepted, in bytes (D4).
pub const MAX_PNG: usize = 4 * 1024 * 1024;
/// Frame width for phone channels (Signal, Telegram), CSS px (D4).
pub const PHONE_WIDTH: u32 = 560;
/// Frame width for Slack, CSS px (D4).
pub const SLACK_WIDTH: u32 = 720;
/// Device-pixel ratio channels ask for (D4).
pub const SCALE: u32 = 2;
/// Columns of the D7 text fallback.
pub const FALLBACK_COLUMNS: usize = 60;
const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// Correlates a snapshot request with its answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(transparent)]
pub struct FenceSnapshotId(pub uuid::Uuid);

/// The fence languages a client draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "kebab-case")]
pub enum FenceLang {
    /// `json-render` (alias `jsonrender`).
    JsonRender,
    /// `mermaid`.
    Mermaid,
    /// `openui`.
    Openui,
}

impl From<PictureLang> for FenceLang {
    fn from(l: PictureLang) -> Self {
        match l {
            PictureLang::JsonRender => Self::JsonRender,
            PictureLang::Mermaid => Self::Mermaid,
            PictureLang::Openui => Self::Openui,
        }
    }
}

/// Event: a channel reply wants a fence drawn as a PNG.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct FenceSnapshotRequested {
    /// Use in `answer_fence_snapshot`.
    pub id: FenceSnapshotId,
    /// The fence language.
    pub lang: FenceLang,
    /// The fence body verbatim (between the fence lines).
    pub source: String,
    /// Frame width in CSS px.
    pub width: u32,
    /// Palette to draw under (`crate::preview::Appearance`).
    pub appearance: Appearance,
    /// Device-pixel ratio of the PNG (1–3).
    pub scale: u32,
}

impl wire::WireEvent for FenceSnapshotRequested {
    const TOPIC: &'static str = "artifact.fence-snapshot-requested";
    /// 216 §7.3: an engine-to-shell request, not an event of the world.
    const EXPOSURE: wire::Exposure = wire::Exposure::Internal;
}

/// Event: the request was answered or timed out; clients drop late renders.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct FenceSnapshotResolved {
    /// The request.
    pub id: FenceSnapshotId,
}

impl wire::WireEvent for FenceSnapshotResolved {
    const TOPIC: &'static str = "artifact.fence-snapshot-resolved";
    /// 216 §7.3: an engine-to-shell request's answer, not an event of the world.
    const EXPOSURE: wire::Exposure = wire::Exposure::Internal;
}

/// A client's answer (`crate::preview::Client`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FenceSnapshotAnswer {
    /// "I am drawing it": the request waits up to 20 s instead of 3 s.
    Started {
        /// Who is drawing.
        client: Client,
    },
    /// `png`: standard base64 of the PNG bytes, no `data:` prefix; `width`/`height` in PNG pixels.
    Done {
        /// Standard base64, no `data:` prefix.
        png: String,
        /// PNG width in pixels.
        width: u32,
        /// PNG height in pixels.
        height: u32,
        /// Who drew it.
        client: Client,
    },
    /// The client could not draw it.
    Failed {
        /// Why, for the log.
        reason: String,
        /// Who failed.
        client: Client,
    },
}

impl FenceSnapshotAnswer {
    /// A `Done` carrying `png`, base64-encoded here: what a Rust-side answerer (a test's
    /// fake client) sends.
    pub fn done(png: &[u8], width: u32, height: u32, client: Client) -> Self {
        Self::Done {
            png: base64::engine::general_purpose::STANDARD.encode(png),
            width,
            height,
            client,
        }
    }
}

/// How one [`render`] ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SnapshotOutcome {
    /// A client drew the fence (or the cache had it).
    Done {
        /// The PNG bytes, checked: PNG signature, ≤ [`MAX_PNG`].
        png: Vec<u8>,
        /// Width in pixels, read off the PNG header.
        width: u32,
        /// Height in pixels, read off the PNG header.
        height: u32,
        /// Who drew it.
        client: Client,
    },
    /// No event subscriber, no `Started` within 3 s, or the engine is in its back-off.
    NoRenderer,
    /// The client failed, or its PNG did not check out.
    Failed(String),
    /// A `Started` arrived but no terminal answer within 20 s of the request.
    TimedOut,
    /// The pending entry vanished without a terminal answer.
    Cancelled,
}

/// How a [`FenceSnapshotReply`] ended (192): [`SnapshotOutcome`] without its payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum FenceSnapshotStatus {
    /// Drawn: `png`, `width`, `height` are set.
    Done,
    /// Nobody to draw it.
    NoRenderer,
    /// The client failed; `error` says why.
    Failed,
    /// Started but not done within 20 s.
    TimedOut,
    /// Dropped without an answer.
    Cancelled,
}

/// Result of RPC `fence_snapshot` (192): one [`render`] for a native companion.
/// `png`/`width`/`height` are non-null iff `outcome` is `done`; `error` only for `failed`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct FenceSnapshotReply {
    /// How the render ended.
    pub outcome: FenceSnapshotStatus,
    /// Standard base64 of the PNG, no `data:` prefix.
    pub png: Option<String>,
    /// PNG width in pixels.
    pub width: Option<u32>,
    /// PNG height in pixels.
    pub height: Option<u32>,
    /// Why it failed.
    pub error: Option<String>,
}

impl From<SnapshotOutcome> for FenceSnapshotReply {
    fn from(o: SnapshotOutcome) -> Self {
        let bare = |outcome| Self {
            outcome,
            png: None,
            width: None,
            height: None,
            error: None,
        };
        match o {
            SnapshotOutcome::Done {
                png, width, height, ..
            } => Self {
                outcome: FenceSnapshotStatus::Done,
                png: Some(base64::engine::general_purpose::STANDARD.encode(png)),
                width: Some(width),
                height: Some(height),
                error: None,
            },
            SnapshotOutcome::NoRenderer => bare(FenceSnapshotStatus::NoRenderer),
            SnapshotOutcome::Failed(why) => Self {
                error: Some(why),
                ..bare(FenceSnapshotStatus::Failed)
            },
            SnapshotOutcome::TimedOut => bare(FenceSnapshotStatus::TimedOut),
            SnapshotOutcome::Cancelled => bare(FenceSnapshotStatus::Cancelled),
        }
    }
}

/// One picture of a painted message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picture {
    /// PNG bytes.
    pub png: Vec<u8>,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Caption: the root json-render element's `title` (D6).
    pub title: Option<String>,
    /// Alt text (D6).
    pub alt: String,
    /// File name, `<title slug>.png` or `diagram.png` / `panel.png`.
    pub name: String,
}

/// One piece of a painted message, in reply order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Piece {
    /// Markdown: prose, and the D7 text of fences nobody drew. Leading blank lines and
    /// trailing whitespace trimmed, never empty.
    Text(String),
    /// A drawn fence.
    Picture(Picture),
}

/// A request waiting for answers.
struct Waiting {
    tx: mpsc::UnboundedSender<FenceSnapshotAnswer>,
    engine: ActorId,
}

/// One drawn PNG.
#[derive(Clone)]
struct Png {
    bytes: Vec<u8>,
    width: u32,
    height: u32,
    client: Client,
}

/// Per-engine state: the D5 cache, the D3 back-off and the ids a late answer may name.
#[derive(Default)]
struct Engine {
    cache: VecDeque<([u8; 32], Png)>,
    backoff_until: Option<Instant>,
    recent: VecDeque<FenceSnapshotId>,
}

static PENDING: LazyLock<Mutex<HashMap<FenceSnapshotId, Waiting>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static ENGINES: LazyLock<Mutex<HashMap<ActorId, Engine>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A client's answer. `true` while the request was pending: the first `Done`/`Failed`
/// resolves it, a `Started` extends the wait. Any answer — late ones too — ends the
/// requesting engine's no-renderer back-off. Every frontend answers through here.
pub fn answer(id: FenceSnapshotId, answer: FenceSnapshotAnswer) -> bool {
    let terminal = !matches!(answer, FenceSnapshotAnswer::Started { .. });
    let (engine, delivered) = {
        let mut map = lock(&PENDING);
        match map.get(&id) {
            Some(w) => {
                let engine = w.engine;
                let delivered = w.tx.send(answer).is_ok();
                if terminal {
                    map.remove(&id);
                }
                (Some(engine), delivered)
            }
            None => (None, false),
        }
    };
    let mut engines = lock(&ENGINES);
    let engine = engine.or_else(|| {
        engines
            .iter()
            .find(|(_, e)| e.recent.contains(&id))
            .map(|(k, _)| *k)
    });
    if let Some(e) = engine.and_then(|k| engines.get_mut(&k)) {
        e.backoff_until = None;
    }
    delivered
}

/// One round trip: draw a `lang` fence with body `source` at `width` CSS px under
/// `appearance` at `scale`. A cached PNG returns at once; no event subscriber, or the
/// engine's back-off, is [`SnapshotOutcome::NoRenderer`] without a request. Otherwise
/// publish [`FenceSnapshotRequested`], wait (no `Started` within 3 s → `NoRenderer` and a
/// 60 s back-off; no terminal answer within 20 s → `TimedOut`), check a `Done`'s PNG and
/// publish [`FenceSnapshotResolved`]. Several may run at once.
pub async fn render(
    rt: &Runtime,
    lang: FenceLang,
    source: &str,
    width: u32,
    appearance: Appearance,
    scale: u32,
) -> SnapshotOutcome {
    render_gated(
        crate::preview::feed_live(),
        rt,
        lang,
        source,
        width,
        appearance,
        scale,
    )
    .await
}

/// [`render`] with the subscriber check given.
pub(crate) async fn render_gated(
    live: bool,
    rt: &Runtime,
    lang: FenceLang,
    source: &str,
    width: u32,
    appearance: Appearance,
    scale: u32,
) -> SnapshotOutcome {
    if !live {
        return SnapshotOutcome::NoRenderer;
    }
    let engine = rt.bus.id();
    let key = cache_key(lang, source, width, appearance, scale);
    {
        let mut engines = lock(&ENGINES);
        let e = engines.entry(engine).or_default();
        if let Some((_, png)) = e.cache.iter().find(|(k, _)| *k == key) {
            return done(png.clone());
        }
        if e.backoff_until.is_some_and(|t| Instant::now() < t) {
            return SnapshotOutcome::NoRenderer;
        }
    }
    let id = FenceSnapshotId(uuid::Uuid::now_v7());
    let (tx, mut rx) = mpsc::unbounded_channel();
    lock(&PENDING).insert(id, Waiting { tx, engine });
    let waiting = Unpend(id);
    rt.publish(FenceSnapshotRequested {
        id,
        lang,
        source: source.to_owned(),
        width,
        appearance,
        scale,
    })
    .await;
    let asked = Instant::now();
    let mut started = false;
    let out = loop {
        let deadline = asked + if started { TOTAL } else { NO_RENDERER };
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Some(FenceSnapshotAnswer::Started { .. })) => started = true,
            Ok(Some(FenceSnapshotAnswer::Done { png, client, .. })) => {
                break match decode(&png) {
                    Ok((bytes, width, height)) => Ok(Png {
                        bytes,
                        width,
                        height,
                        client,
                    }),
                    Err(why) => Err(SnapshotOutcome::Failed(why)),
                };
            }
            Ok(Some(FenceSnapshotAnswer::Failed { reason, .. })) => {
                break Err(SnapshotOutcome::Failed(reason));
            }
            // The sender goes only with the map entry, which `answer` removes together
            // with the terminal send.
            Ok(None) => break Err(SnapshotOutcome::Cancelled),
            Err(_) if !started => break Err(SnapshotOutcome::NoRenderer),
            Err(_) => break Err(SnapshotOutcome::TimedOut),
        }
    };
    drop(waiting);
    {
        let mut engines = lock(&ENGINES);
        let e = engines.entry(engine).or_default();
        e.recent.push_back(id);
        if e.recent.len() > RECENT {
            e.recent.pop_front();
        }
        match &out {
            Ok(png) => {
                e.cache.push_back((key, png.clone()));
                if e.cache.len() > CACHE {
                    e.cache.pop_front();
                }
            }
            Err(SnapshotOutcome::NoRenderer) => e.backoff_until = Some(Instant::now() + BACKOFF),
            Err(_) => {}
        }
    }
    rt.publish(FenceSnapshotResolved { id }).await;
    out.map_or_else(|o| o, done)
}

/// Removes a request from [`PENDING`] when the round trip ends or is dropped mid-wait.
struct Unpend(FenceSnapshotId);

impl Drop for Unpend {
    fn drop(&mut self) {
        lock(&PENDING).remove(&self.0);
    }
}

fn done(png: Png) -> SnapshotOutcome {
    SnapshotOutcome::Done {
        png: png.bytes,
        width: png.width,
        height: png.height,
        client: png.client,
    }
}

/// sha256(lang, source, width, appearance, scale) (D5).
fn cache_key(
    lang: FenceLang,
    source: &str,
    width: u32,
    appearance: Appearance,
    scale: u32,
) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update([lang as u8, appearance as u8]);
    h.update(width.to_le_bytes());
    h.update(scale.to_le_bytes());
    h.update(source.as_bytes());
    h.finalize().into()
}

/// A `Done`'s base64 → checked PNG bytes and the header's `(width, height)`.
fn decode(png: &str) -> Result<(Vec<u8>, u32, u32), String> {
    // Refuse before decoding what cannot fit: 4 base64 chars per 3 bytes.
    if png.len() > MAX_PNG.div_ceil(3) * 4 {
        return Err(format!("PNG over the {MAX_PNG} byte limit"));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(png)
        .map_err(|e| format!("PNG is not valid base64: {e}"))?;
    if bytes.len() > MAX_PNG {
        return Err(format!(
            "PNG is {} bytes, over the {MAX_PNG} byte limit",
            bytes.len()
        ));
    }
    if bytes.len() < 24 || bytes[..8] != PNG_MAGIC || &bytes[12..16] != b"IHDR" {
        return Err("not a PNG".into());
    }
    let side =
        |at: usize| u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    let (width, height) = (side(16), side(20));
    if width == 0 || height == 0 {
        return Err("PNG has no pixels".into());
    }
    Ok((bytes, width, height))
}

/// Split `text` into text and pictures for a channel (D6, D7, D9): every closed picture
/// fence is drawn at `width` CSS px (light, scale 2), all at once; one nobody drew — or every
/// one when `pictures` is `false`, a json-render fence that does not parse, an unclosed fence —
/// becomes its D7 text, merged into the text around it. Never returns a fence body.
pub async fn paint(rt: &Runtime, text: &str, width: u32, pictures: bool) -> Vec<Piece> {
    let parts = surface::picture_parts(text);
    let outcomes = futures::future::join_all(parts.iter().map(|part| async move {
        match part {
            PicturePart::Picture(f) if pictures && drawable(f) => {
                Some(render(rt, f.lang.into(), f.body, width, Appearance::Light, SCALE).await)
            }
            _ => None,
        }
    }))
    .await;
    let mut out = Vec::new();
    let mut buf = String::new();
    for (part, outcome) in parts.into_iter().zip(outcomes) {
        match (part, outcome) {
            (PicturePart::Text(t), _) => buf.push_str(t),
            (
                PicturePart::Picture(f),
                Some(SnapshotOutcome::Done {
                    png, width, height, ..
                }),
            ) => {
                push_text(&mut out, &mut buf);
                let title = f.title();
                out.push(Piece::Picture(Picture {
                    name: file_name(title.as_deref(), f.lang),
                    alt: f.alt(),
                    title,
                    png,
                    width,
                    height,
                }));
            }
            (PicturePart::Picture(f), outcome) => {
                if let Some(o) = outcome {
                    tracing::info!(target: "snapshot", lang = f.lang.as_str(), outcome = ?o, "fence sent as text");
                }
                buf.push_str(&f.fallback(FALLBACK_COLUMNS));
            }
        }
    }
    push_text(&mut out, &mut buf);
    out
}

/// Worth a request: closed, and a json-render body that parses.
fn drawable(f: &PictureFence<'_>) -> bool {
    f.closed && (f.lang != PictureLang::JsonRender || surface::parse(f.body).is_ok())
}

/// `buf` as a [`Piece::Text`] unless blank; `buf` cleared.
fn push_text(out: &mut Vec<Piece>, buf: &mut String) {
    let t = buf.trim_start_matches(['\n', '\r']).trim_end();
    if !t.trim().is_empty() {
        out.push(Piece::Text(t.to_owned()));
    }
    buf.clear();
}

/// `<title slug>.png`, else `diagram.png` / `panel.png`.
fn file_name(title: Option<&str>, lang: PictureLang) -> String {
    let mut slug = String::new();
    for c in title.unwrap_or_default().chars() {
        if c.is_alphanumeric() {
            slug.extend(c.to_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
        if slug.chars().count() >= 40 {
            break;
        }
    }
    let slug = slug.trim_end_matches('-');
    match (slug.is_empty(), lang) {
        (false, _) => format!("{slug}.png"),
        (true, PictureLang::Openui) => "panel.png".into(),
        (true, _) => "diagram.png".into(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use kameo::actor::Spawn;
    use kameo::message::{Context, Message};

    use super::*;

    /// The smallest PNG header that checks out, `w`×`h`.
    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut b = PNG_MAGIC.to_vec();
        b.extend([0, 0, 0, 13]);
        b.extend(b"IHDR");
        b.extend(w.to_be_bytes());
        b.extend(h.to_be_bytes());
        b.extend([8, 6, 0, 0, 0]);
        b
    }

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    /// What the fake client does with each request.
    #[derive(Clone)]
    enum Mode {
        /// Answer these, in order.
        Answer(Vec<FenceSnapshotAnswer>),
        /// Never answer.
        Silent,
    }

    /// A client stand-in on the bus: records every request and resolution, answers per mode.
    #[derive(kameo::Actor)]
    struct Fake {
        mode: Arc<Mutex<Mode>>,
        seen: Arc<Mutex<Vec<FenceSnapshotRequested>>>,
        resolved: Arc<Mutex<Vec<FenceSnapshotId>>>,
    }

    impl Message<FenceSnapshotRequested> for Fake {
        type Reply = ();
        async fn handle(&mut self, m: FenceSnapshotRequested, _: &mut Context<Self, ()>) {
            lock(&self.seen).push(m.clone());
            if let Mode::Answer(answers) = lock(&self.mode).clone() {
                for a in answers {
                    answer(m.id, a);
                }
            }
        }
    }

    impl Message<FenceSnapshotResolved> for Fake {
        type Reply = ();
        async fn handle(&mut self, m: FenceSnapshotResolved, _: &mut Context<Self, ()>) {
            lock(&self.resolved).push(m.id);
        }
    }

    struct Harness {
        rt: Runtime,
        mode: Arc<Mutex<Mode>>,
        seen: Arc<Mutex<Vec<FenceSnapshotRequested>>>,
        resolved: Arc<Mutex<Vec<FenceSnapshotId>>>,
    }

    /// `got` is `want` give or take the scheduler's 100 ms.
    fn near(got: Duration, want: Duration) -> bool {
        got >= want && got < want + Duration::from_millis(100)
    }

    impl Harness {
        async fn new(mode: Mode) -> Self {
            crate::preview::watch_feed(Arc::new(|| 1));
            let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
            let h = Self {
                rt,
                mode: Arc::new(Mutex::new(mode)),
                seen: Arc::default(),
                resolved: Arc::default(),
            };
            let fake = Fake::spawn(Fake {
                mode: h.mode.clone(),
                seen: h.seen.clone(),
                resolved: h.resolved.clone(),
            });
            h.rt.subscribe::<Fake, FenceSnapshotRequested>(&fake).await;
            h.rt.subscribe::<Fake, FenceSnapshotResolved>(&fake).await;
            h
        }

        async fn render(&self, source: &str) -> SnapshotOutcome {
            render(
                &self.rt,
                FenceLang::Mermaid,
                source,
                PHONE_WIDTH,
                Appearance::Light,
                SCALE,
            )
            .await
        }

        fn requests(&self) -> usize {
            lock(&self.seen).len()
        }

        fn set(&self, mode: Mode) {
            *lock(&self.mode) = mode;
        }
    }

    fn started() -> FenceSnapshotAnswer {
        FenceSnapshotAnswer::Started {
            client: Client::Desktop,
        }
    }

    fn done_with(png: String) -> FenceSnapshotAnswer {
        FenceSnapshotAnswer::Done {
            png,
            width: 1,
            height: 1,
            client: Client::Desktop,
        }
    }

    #[test]
    fn wire_shapes_match_the_contract() {
        let id = FenceSnapshotId(uuid::Uuid::nil());
        let req = FenceSnapshotRequested {
            id,
            lang: FenceLang::JsonRender,
            source: "{}".into(),
            width: 560,
            appearance: Appearance::Light,
            scale: 2,
        };
        assert_eq!(
            serde_json::to_value(&req).unwrap(),
            serde_json::json!({"type": "FenceSnapshotRequested", "id": "00000000-0000-0000-0000-000000000000", "lang": "json-render", "source": "{}", "width": 560, "appearance": "light", "scale": 2})
        );
        assert_eq!(
            serde_json::to_value(FenceSnapshotResolved { id }).unwrap()["type"],
            "FenceSnapshotResolved"
        );
        let a: FenceSnapshotAnswer = serde_json::from_value(serde_json::json!(
            {"kind": "done", "png": "AA==", "width": 3, "height": 4, "client": "web"}
        ))
        .unwrap();
        assert_eq!(
            a,
            FenceSnapshotAnswer::Done {
                png: "AA==".into(),
                width: 3,
                height: 4,
                client: Client::Web
            }
        );
        let f: FenceSnapshotAnswer = serde_json::from_value(
            serde_json::json!({"kind": "failed", "reason": "x", "client": "desktop"}),
        )
        .unwrap();
        assert!(matches!(f, FenceSnapshotAnswer::Failed { .. }));
        assert_eq!(serde_json::to_value(FenceLang::Openui).unwrap(), "openui");
    }

    #[tokio::test(start_paused = true)]
    async fn round_trip_started_then_done() {
        let pic = png(1120, 400);
        let h = Harness::new(Mode::Answer(vec![started(), done_with(b64(&pic))])).await;
        let out = h.render("graph LR\n  A --> B\n").await;
        assert_eq!(
            out,
            SnapshotOutcome::Done {
                png: pic,
                width: 1120,
                height: 400,
                client: Client::Desktop
            }
        );
        let seen = lock(&h.seen).clone();
        assert_eq!(seen.len(), 1);
        assert_eq!(
            (
                seen[0].lang,
                seen[0].source.as_str(),
                seen[0].width,
                seen[0].scale
            ),
            (FenceLang::Mermaid, "graph LR\n  A --> B\n", 560, 2)
        );
        assert_eq!(seen[0].appearance, Appearance::Light);
        // Late answers are no longer pending.
        assert!(!answer(seen[0].id, done_with(b64(&png(1, 1)))));
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(*lock(&h.resolved), vec![seen[0].id]);
    }

    #[tokio::test(start_paused = true)]
    async fn failed_answer_is_failed() {
        let h = Harness::new(Mode::Answer(vec![FenceSnapshotAnswer::Failed {
            reason: "too big".into(),
            client: Client::Web,
        }]))
        .await;
        assert_eq!(
            h.render("x").await,
            SnapshotOutcome::Failed("too big".into())
        );
    }

    #[tokio::test(start_paused = true)]
    async fn no_subscriber_is_no_renderer_at_once() {
        let h = Harness::new(Mode::Silent).await;
        let t = Instant::now();
        let out = render_gated(
            false,
            &h.rt,
            FenceLang::Mermaid,
            "x",
            PHONE_WIDTH,
            Appearance::Light,
            SCALE,
        )
        .await;
        assert_eq!(out, SnapshotOutcome::NoRenderer);
        assert_eq!(t.elapsed(), Duration::ZERO);
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(h.requests(), 0, "nothing published");
    }

    /// No `Started` in 3 s → `NoRenderer`; for 60 s after, nothing is published; any answer
    /// ends the back-off early.
    #[tokio::test(start_paused = true)]
    async fn silence_is_no_renderer_after_three_seconds_then_backs_off() {
        let h = Harness::new(Mode::Silent).await;
        let t = Instant::now();
        assert_eq!(h.render("a").await, SnapshotOutcome::NoRenderer);
        assert!(near(t.elapsed(), NO_RENDERER), "{:?}", t.elapsed());
        assert_eq!(h.requests(), 1);

        // Inside the back-off: fallback at once, no request.
        h.set(Mode::Answer(vec![done_with(b64(&png(2, 2)))]));
        let t = Instant::now();
        assert_eq!(h.render("b").await, SnapshotOutcome::NoRenderer);
        assert_eq!(t.elapsed(), Duration::ZERO);
        assert_eq!(h.requests(), 1);

        // Past 60 s: requests again.
        tokio::time::sleep(BACKOFF).await;
        assert!(matches!(h.render("c").await, SnapshotOutcome::Done { .. }));
        assert_eq!(h.requests(), 2);

        // A late answer to a dead request ends a fresh back-off at once.
        h.set(Mode::Silent);
        assert_eq!(h.render("d").await, SnapshotOutcome::NoRenderer);
        h.set(Mode::Answer(vec![done_with(b64(&png(2, 2)))]));
        let late = lock(&h.seen)[2].id;
        assert!(!answer(late, started()), "late: no longer pending");
        assert!(matches!(h.render("e").await, SnapshotOutcome::Done { .. }));
    }

    #[tokio::test(start_paused = true)]
    async fn started_without_an_answer_times_out_at_twenty_seconds() {
        let h = Harness::new(Mode::Answer(vec![started()])).await;
        let t = Instant::now();
        assert_eq!(h.render("x").await, SnapshotOutcome::TimedOut);
        assert!(near(t.elapsed(), TOTAL), "{:?}", t.elapsed());
        // A timeout is not a missing renderer: no back-off.
        h.set(Mode::Answer(vec![done_with(b64(&png(3, 3)))]));
        assert!(matches!(h.render("y").await, SnapshotOutcome::Done { .. }));
    }

    #[tokio::test(start_paused = true)]
    async fn cache_hit_skips_the_request() {
        let pic = png(10, 20);
        let h = Harness::new(Mode::Answer(vec![done_with(b64(&pic))])).await;
        let first = h.render("same").await;
        h.set(Mode::Silent);
        assert_eq!(h.render("same").await, first);
        assert_eq!(h.requests(), 1);
        // Another width is another key.
        let other = render(
            &h.rt,
            FenceLang::Mermaid,
            "same",
            SLACK_WIDTH,
            Appearance::Light,
            SCALE,
        )
        .await;
        assert_eq!(other, SnapshotOutcome::NoRenderer);
        assert_eq!(h.requests(), 2);
        // Engines do not share a cache.
        let h2 = Harness::new(Mode::Silent).await;
        assert_eq!(h2.render("same").await, SnapshotOutcome::NoRenderer);
    }

    #[tokio::test(start_paused = true)]
    async fn bad_pngs_fail() {
        let oversize = {
            let mut b = png(5, 5);
            b.resize(MAX_PNG + 1, 0);
            b
        };
        for (png_b64, want) in [
            ("not base64!".to_owned(), "PNG is not valid base64"),
            (b64(b"GIF89a-not-a-png-at-all-xx"), "not a PNG"),
            (b64(&png(0, 5)), "PNG has no pixels"),
            (b64(&oversize), "PNG is 4194305 bytes, over the"),
        ] {
            let h = Harness::new(Mode::Answer(vec![done_with(png_b64)])).await;
            match h.render("x").await {
                SnapshotOutcome::Failed(why) => assert!(why.starts_with(want), "{why}"),
                other => panic!("{other:?}"),
            }
        }
        // Just under the cap passes.
        let mut big = png(5, 5);
        big.resize(MAX_PNG, 0);
        let h = Harness::new(Mode::Answer(vec![done_with(b64(&big))])).await;
        assert!(matches!(h.render("x").await, SnapshotOutcome::Done { .. }));
    }

    const GRAPH: &str = "{\"root\":\"g\",\"elements\":{\"g\":{\"type\":\"Graph\",\"props\":{\"title\":\"Data flow\",\"nodes\":[{\"id\":\"a\",\"label\":\"A\"},{\"id\":\"b\",\"label\":\"B\"}],\"edges\":[{\"from\":\"a\",\"to\":\"b\"}]}}}}";

    #[tokio::test(start_paused = true)]
    async fn paint_keeps_order_and_falls_back() {
        let pic = png(1120, 300);
        let text = format!(
            "Before.\n\n```json-render\n{GRAPH}\n```\n\nBetween.\n```openui\n<Card/>\n```\nAfter.\n```mermaid\ngraph LR"
        );
        let h = Harness::new(Mode::Answer(vec![done_with(b64(&pic))])).await;
        let pieces = paint(&h.rt, &text, PHONE_WIDTH, true).await;
        assert_eq!(
            pieces,
            vec![
                Piece::Text("Before.".into()),
                Piece::Picture(Picture {
                    png: pic.clone(),
                    width: 1120,
                    height: 300,
                    title: Some("Data flow".into()),
                    alt: "Diagram: 2 nodes, 1 edges".into(),
                    name: "data-flow.png".into(),
                }),
                // openui drawn too; the unclosed mermaid is the placeholder.
                Piece::Text("Between.".into()),
                Piece::Picture(Picture {
                    png: pic.clone(),
                    width: 1120,
                    height: 300,
                    title: None,
                    alt: "interactive panel".into(),
                    name: "panel.png".into(),
                }),
                Piece::Text("After.\n[diagram — open theuth to see it]".into()),
            ]
        );
        assert_eq!(h.requests(), 2, "the unclosed fence is not requested");

        // pictures: false → text only, nothing requested.
        let off = paint(&h.rt, &text, PHONE_WIDTH, false).await;
        assert_eq!(h.requests(), 2);
        assert!(off.iter().all(|p| matches!(p, Piece::Text(_))));
        let joined: String = off
            .iter()
            .map(|p| match p {
                Piece::Text(t) => t.as_str(),
                Piece::Picture(_) => "",
            })
            .collect();
        assert!(
            !joined.contains("\"root\"") && !joined.contains("<Card"),
            "{joined}"
        );
        assert!(
            joined.contains("[interactive panel — open theuth to see it]"),
            "{joined}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn paint_requests_every_fence_at_once() {
        let h = Harness::new(Mode::Answer(vec![started()])).await;
        let text = "```mermaid\ngraph LR\n  A --> B\n```\n```mermaid\ngraph TB\n  C --> D\n```\n";
        let t = Instant::now();
        let pieces = paint(&h.rt, text, PHONE_WIDTH, true).await;
        // Two timeouts in parallel: 20 s, not 40.
        assert!(near(t.elapsed(), TOTAL), "{:?}", t.elapsed());
        assert_eq!(h.requests(), 2);
        assert!(
            pieces.iter().all(|p| matches!(p, Piece::Text(_))),
            "{pieces:?}"
        );
    }

    #[test]
    fn file_names() {
        assert_eq!(
            file_name(Some("Release readiness: Q3!"), PictureLang::JsonRender),
            "release-readiness-q3.png"
        );
        assert_eq!(file_name(Some("  "), PictureLang::Mermaid), "diagram.png");
        assert_eq!(file_name(None, PictureLang::Openui), "panel.png");
    }
}
