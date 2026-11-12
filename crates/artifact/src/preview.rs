//! `page_preview` (120 §11, 150 §D3–D4): a connected desktop/web client
//! renders an HTML page in a hidden sandboxed frame and reports the height
//! it needs plus its console. Same shape as `ask_user` (`tools::ask`):
//! publish [`PagePreviewRequested`], wait for answers through [`answer`],
//! publish [`PagePreviewResolved`]. [`render`] is that round trip; the
//! `page_preview` tool and `show`'s pre-publish page check both run it.
//!
//! Not in `PendingPrompts`: a preview nobody rendered in time is dead, not
//! pending (§11.2). The pending map is a process-wide static — one tool
//! instance, one RPC entry point, no actor needed.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use std::time::Duration;

use runtime::{Runtime, SessionId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::store::Stores;
use tools::{Risk, Tool, ToolCx, ToolOutput};

/// Longest page accepted, in chars (T3's `html_render` limit).
pub const MAX_HTML: usize = 512_000;
/// Default frame width: the reply column.
pub const DEFAULT_WIDTH: u32 = 728;
/// Allowed frame widths.
pub const WIDTH: std::ops::RangeInclusive<u32> = 240..=1600;
/// No `Started` within this → "no renderer connected" (§11.4).
const NO_RENDERER: Duration = Duration::from_secs(2);
/// A `Started` extends the wait to this, measured from the request (§11.4).
const TOTAL: Duration = Duration::from_secs(15);

/// Correlates a request with its answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(transparent)]
pub struct PreviewId(pub uuid::Uuid);

/// The appearance the page is rendered under.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    /// The dark palette.
    Dark,
    /// The light palette.
    Light,
}

/// Which kind of client rendered the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum Client {
    /// The Tauri desktop shell.
    Desktop,
    /// A plain browser (dev server or relay-served build).
    Web,
    /// The iOS bundle.
    Phone,
}

/// Console level, as the page bootstrap posts it (§5.4; `warn` → `warning`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum ConsoleLevel {
    /// `console.log`.
    Log,
    /// `console.info`.
    Info,
    /// `console.warn`.
    Warning,
    /// `console.error`, `window.onerror`, `unhandledrejection`.
    Error,
}

/// One console message the page posted (≤ 500 chars, ≤ 20 per load).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ConsoleLine {
    /// Severity.
    pub level: ConsoleLevel,
    /// The arguments joined by a space; stack traces kept.
    pub text: String,
}

/// Event: a tool call wants a page rendered. Every UI subscribes; every
/// desktop/web client that receives it renders and answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct PagePreviewRequested {
    /// Use in `answer_page_preview`.
    pub id: PreviewId,
    /// Owning session.
    pub session: SessionId,
    /// The agent's bytes; the UI injects the bootstrap (§4.4).
    pub html: String,
    /// Frame width in CSS px.
    pub width: u32,
    /// Requested appearance; `None` = the client's current one.
    pub appearance: Option<Appearance>,
}

impl wire::WireEvent for PagePreviewRequested {
    const TOPIC: &'static str = "artifact.page-preview-requested";
}

/// Event: the request was answered or timed out. Clients still rendering
/// tear their frame down.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct PagePreviewResolved {
    /// The request.
    pub id: PreviewId,
}

impl wire::WireEvent for PagePreviewResolved {
    const TOPIC: &'static str = "artifact.page-preview-resolved";
}

/// A client's answer to a [`PagePreviewRequested`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PagePreviewAnswer {
    /// "I am rendering it": extends the deadline to 15 s.
    Started {
        /// Who is rendering.
        client: Client,
    },
    /// The page loaded (or did not within 10 s: `loaded: false`); first one wins.
    Done {
        /// The height the page reported, clamped 80–2000 by the client.
        content_height: u32,
        /// Console lines in order, ≤ 20 × 500 chars.
        console: Vec<ConsoleLine>,
        /// `false` when `load` never fired within the client's 10 s.
        loaded: bool,
        /// Who rendered.
        client: Client,
    },
    /// The client could not render it (`page_put` failed, …).
    Failed {
        /// Why, for the model.
        reason: String,
    },
}

/// Arguments of `page_preview`. Exactly one of `html` / `path`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct PreviewArgs {
    /// The page: one self-contained HTML document, 1–512 000 chars. Saved as the draft for
    /// `name` (default `preview.html`), so `show{path}` of the file the result names publishes it.
    pub html: Option<String>,
    /// Instead of `html`: an existing `.html` file to render (1–512 000 chars).
    pub path: Option<PathBuf>,
    /// Draft file name for `html`, e.g. `chart.html`; default `preview.html`.
    pub name: Option<String>,
    /// Frame width in CSS px, 240–1600. Default 728 (the reply column).
    pub width: Option<u32>,
    /// `dark` or `light`. Default: the client's current appearance.
    pub appearance: Option<Appearance>,
}

/// Waiting round trips: where each one's answers go.
static PENDING: LazyLock<Mutex<HashMap<PreviewId, mpsc::UnboundedSender<PagePreviewAnswer>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// External RPC stream liveness, installed by artifact composition.
static FEED: OnceLock<Arc<dyn Fn() -> usize + Send + Sync>> = OnceLock::new();

/// Install the external-stream counter used by the renderer heuristic.
pub fn watch_feed(count: Arc<dyn Fn() -> usize + Send + Sync>) {
    let _ = FEED.set(count);
}

/// A local generic feed consumer does not imply a connected renderer.
pub fn feed_live() -> bool {
    FEED.get().is_some_and(|count| count() > 0)
}

/// A client's answer. `true` if the preview was still pending: the first
/// `Done`/`Failed` resolves it, a `Started` extends the wait and leaves it
/// pending. Every frontend answers through here.
pub fn answer(id: PreviewId, answer: PagePreviewAnswer) -> bool {
    let mut map = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    let terminal = !matches!(answer, PagePreviewAnswer::Started { .. });
    let Some(tx) = map.get(&id) else { return false };
    let delivered = tx.send(answer).is_ok();
    if terminal {
        map.remove(&id);
    }
    delivered
}

/// How one [`render`] ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// A client rendered the page (or gave up after its own 10 s: `loaded: false`).
    Done {
        /// The height the page reported, clamped 80–2000 by the client.
        content_height: u32,
        /// Console lines in order.
        console: Vec<ConsoleLine>,
        /// `false` when `load` never fired on the client.
        loaded: bool,
        /// Who rendered.
        client: Client,
    },
    /// No subscriber at all, or no `Started` within 2 s.
    NoRenderer,
    /// The client could not render it.
    Failed(String),
    /// A `Started` arrived but no `Done` within 15 s.
    TimedOut,
    /// The pending entry vanished without a terminal answer.
    Cancelled,
}

/// One round trip: publish [`PagePreviewRequested`], wait for answers (no
/// `Started` within 2 s → [`Outcome::NoRenderer`]; a `Started` extends the
/// wait to 15 s from the request), publish [`PagePreviewResolved`]. Several
/// may run at once, in one session too: clients key frames by `id`.
pub async fn render(
    rt: &Runtime,
    session: SessionId,
    html: String,
    width: u32,
    appearance: Option<Appearance>,
) -> Outcome {
    if FEED.get().is_some_and(|count| count() == 0) {
        return Outcome::NoRenderer;
    }
    let id = PreviewId(uuid::Uuid::now_v7());
    let (tx, mut rx) = mpsc::unbounded_channel();
    PENDING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id, tx);
    rt.publish(PagePreviewRequested {
        id,
        session,
        html,
        width,
        appearance,
    })
    .await;
    let started = tokio::time::Instant::now();
    let mut deadline = started + NO_RENDERER;
    let out = loop {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Some(PagePreviewAnswer::Started { .. })) => deadline = started + TOTAL,
            Ok(Some(PagePreviewAnswer::Done {
                content_height,
                console,
                loaded,
                client,
            })) => {
                break Outcome::Done {
                    content_height,
                    console,
                    loaded,
                    client,
                };
            }
            Ok(Some(PagePreviewAnswer::Failed { reason })) => break Outcome::Failed(reason),
            // Sender dropped only when the map entry was removed, which
            // `answer` does together with the terminal send.
            Ok(None) => break Outcome::Cancelled,
            Err(_) if deadline == started + NO_RENDERER => break Outcome::NoRenderer,
            Err(_) => break Outcome::TimedOut,
        }
    };
    PENDING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&id);
    rt.publish(PagePreviewResolved { id }).await;
    out
}

/// The `page_preview` tool (§11.1).
pub struct PagePreviewTool {
    /// For publishing the two events.
    pub rt: Runtime,
    /// Where `html` is saved as a draft (150 §D1).
    pub stores: Arc<Stores>,
}

#[async_trait::async_trait]
impl Tool for PagePreviewTool {
    const NAME: &'static str = "page_preview";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "dict";
    const DIRECT: bool = true;
    type Args = PreviewArgs;

    fn description(&self) -> String {
        "Render an HTML page hidden in the connected theuth app WITHOUT publishing it, and get \
         back `contentHeight` (the height it needs at `width`) and its console: log, info, \
         warning, error and uncaught exceptions. Optional: `show` already checks every page \
         before publishing and refuses one that logs errors. Use this to try a page the user \
         should not see yet, or to read its console.log output. Pass `path` for a file that \
         exists, or `html` once — it is saved as a file the result names; publish that with \
         show{path}, never by resending the html. No screenshot. Fails fast with \"no renderer \
         connected\" when no desktop or web client is open."
            .into()
    }

    async fn call(&self, args: PreviewArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let width = args.width.unwrap_or(DEFAULT_WIDTH);
        if !WIDTH.contains(&width) {
            return Ok(ToolOutput::err(format!(
                "page_preview: width must be {}–{} (got {width})",
                WIDTH.start(),
                WIDTH.end()
            )));
        }
        let (html, file) = match (args.html.filter(|h| !h.is_empty()), args.path) {
            (Some(_), Some(_)) => {
                return Ok(ToolOutput::err(
                    "page_preview: pass either html or path, not both",
                ));
            }
            (None, None) => {
                return Ok(ToolOutput::err(
                    "page_preview: pass html (the page) or path (an existing .html file)",
                ));
            }
            (Some(html), None) => {
                let state = if cx.resources.is_private() {
                    self.stores.private_for(cx.session)
                } else {
                    self.stores.state_for(cx.session).ok()
                };
                let Some(state) = state else {
                    return Ok(ToolOutput::err("page_preview: private session is not open"));
                };
                let name = args.name.as_deref().unwrap_or("preview.html");
                let file = match state
                    .save_draft(cx.session, Some(name), Default::default(), &html)
                    .await
                {
                    Ok(f) => f,
                    Err(e) => return Ok(ToolOutput::err(format!("page_preview: {e}"))),
                };
                (html, Some(file))
            }
            (None, Some(path)) => match tokio::fs::read_to_string(&path).await {
                Ok(html) => (html, None),
                Err(e) => {
                    return Ok(ToolOutput::err(format!(
                        "page_preview: {}: {e}",
                        path.display()
                    )));
                }
            },
        };
        let n = html.chars().count();
        if n == 0 || n > MAX_HTML {
            return Ok(ToolOutput::err(format!(
                "page_preview: the page must be 1–{MAX_HTML} chars (got {n})"
            )));
        }
        let out = match render(&self.rt, cx.session, html, width, args.appearance).await {
            Outcome::Done {
                content_height,
                console,
                loaded,
                client,
            } => done(
                width,
                content_height,
                console,
                loaded,
                client,
                file.as_deref(),
            ),
            Outcome::NoRenderer => ToolOutput::err(NO_RENDERER_MSG),
            Outcome::Failed(reason) => ToolOutput::err(format!("page_preview: {reason}")),
            Outcome::Cancelled => ToolOutput::err("page_preview: cancelled"),
            Outcome::TimedOut => ToolOutput::err(
                "page_preview: preview timed out after 15 s (the page did not finish loading); \
                 console so far: none",
            ),
        };
        Ok(out)
    }
}

const NO_RENDERER_MSG: &str = "page_preview: no renderer connected — open the theuth desktop or \
                               web app. `show` checks pages itself and publishes unchecked when \
                               no app is connected, so you can skip the preview and show the page";

/// The `Done` result: the §11.5 message line plus the §11.1 dict; `file` is
/// the draft `html` was saved as.
fn done(
    width: u32,
    content_height: u32,
    console: Vec<ConsoleLine>,
    loaded: bool,
    client: Client,
    file: Option<&std::path::Path>,
) -> ToolOutput {
    let errors = console
        .iter()
        .filter(|l| l.level == ConsoleLevel::Error)
        .count();
    let mut content = format!(
        "Previewed at {width} px: contentHeight {content_height}, {errors} error{}. Pass height: \
         {content_height} to show, or less to make it scroll.",
        if errors == 1 { "" } else { "s" }
    );
    if !loaded {
        content.push_str(
            " The page did not finish loading within 10 s; the height is what it reported so far.",
        );
    }
    if let Some(f) = file {
        let _ = write!(
            content,
            " · file {f} — show{{path: \"{f}\"}} publishes exactly this page",
            f = f.display()
        );
    }
    for l in &console {
        let _ = write!(content, "\n[{}] {}", level_name(l.level), l.text);
    }
    let mut structured = serde_json::json!({
        "width": width,
        "contentHeight": content_height,
        "console": console,
        "loaded": loaded,
        "client": client,
    });
    if let Some(f) = file {
        structured["file"] = serde_json::Value::String(f.display().to_string());
    }
    ToolOutput {
        structured: Some(structured),
        ..ToolOutput::ok(content)
    }
}

impl Client {
    /// The wire spelling (`"desktop"`, …), for receipts.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::Web => "web",
            Self::Phone => "phone",
        }
    }
}

fn level_name(l: ConsoleLevel) -> &'static str {
    match l {
        ConsoleLevel::Log => "log",
        ConsoleLevel::Info => "info",
        ConsoleLevel::Warning => "warning",
        ConsoleLevel::Error => "error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(level: ConsoleLevel, text: &str) -> ConsoleLine {
        ConsoleLine {
            level,
            text: text.into(),
        }
    }

    #[test]
    fn done_message_counts_errors_and_lists_console() {
        let out = done(
            728,
            612,
            vec![
                line(ConsoleLevel::Log, "ok"),
                line(ConsoleLevel::Error, "boom"),
            ],
            true,
            Client::Desktop,
            None,
        );
        assert!(!out.is_error);
        assert_eq!(
            out.content,
            "Previewed at 728 px: contentHeight 612, 1 error. Pass height: 612 to show, or less \
             to make it scroll.\n[log] ok\n[error] boom"
        );
        let s = out.structured.unwrap();
        assert_eq!(s["contentHeight"], 612);
        assert_eq!(s["client"], "desktop");
        assert_eq!(s["console"][1]["level"], "error");
        assert!(s.get("file").is_none());

        let out = done(
            400,
            80,
            vec![],
            false,
            Client::Web,
            Some(std::path::Path::new("/d/preview.html")),
        );
        assert!(
            out.content
                .starts_with("Previewed at 400 px: contentHeight 80, 0 errors.")
        );
        assert!(out.content.contains("did not finish loading"));
        assert!(out.content.contains(
            " · file /d/preview.html — show{path: \"/d/preview.html\"} publishes exactly this page"
        ));
        assert_eq!(out.structured.unwrap()["file"], "/d/preview.html");
    }

    #[test]
    fn answer_without_a_pending_id_is_false() {
        assert!(!answer(
            PreviewId(uuid::Uuid::now_v7()),
            PagePreviewAnswer::Failed { reason: "x".into() }
        ));
    }

    #[test]
    fn wire_shapes_match_the_design() {
        let a = PagePreviewAnswer::Started {
            client: Client::Phone,
        };
        assert_eq!(
            serde_json::to_value(&a).unwrap(),
            serde_json::json!({"kind": "started", "client": "phone"})
        );
        let d: PagePreviewAnswer = serde_json::from_value(serde_json::json!({
            "kind": "done", "content_height": 10, "console": [{"level": "warning", "text": "w"}],
            "loaded": true, "client": "desktop"
        }))
        .unwrap();
        assert!(matches!(
            d,
            PagePreviewAnswer::Done {
                content_height: 10,
                ..
            }
        ));
        let r = PagePreviewResolved {
            id: PreviewId(uuid::Uuid::nil()),
        };
        assert_eq!(
            serde_json::to_value(&r).unwrap()["type"],
            "PagePreviewResolved"
        );
    }

    /// A client stand-in (150 §D3/§D4): a store on a temp dir, a feed with one
    /// live subscriber so `feed_live()` holds, and a bus subscriber that hands
    /// every request id to the test, which answers through [`answer`].
    mod client {
        use super::*;
        use crate::ShowArgs;
        use crate::store::State;
        use kameo::actor::Spawn;
        use kameo::message::{Context, Message};
        use tools::{ToolCx, ToolOutput};

        #[derive(kameo::Actor)]
        struct Capture(mpsc::UnboundedSender<PreviewId>);

        impl Message<PagePreviewRequested> for Capture {
            type Reply = ();
            async fn handle(&mut self, m: PagePreviewRequested, _: &mut Context<Self, ()>) {
                let _ = self.0.send(m.id);
            }
        }

        pub(super) struct Harness {
            pub rt: Runtime,
            pub stores: Arc<Stores>,
            pub state: Arc<State>,
            pub requests: tokio::sync::Mutex<mpsc::UnboundedReceiver<PreviewId>>,
            pub dir: tempfile::TempDir,
        }

        impl Harness {
            pub async fn new() -> Self {
                // The liveness hook is process-wide and set once.
                watch_feed(Arc::new(|| 1));
                assert!(feed_live());
                let dir = tempfile::tempdir().unwrap();
                let rt = Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
                let state = Arc::new(
                    State::open(dir.path(), rt.clone(), Vec::new())
                        .await
                        .unwrap(),
                );
                let (tx, requests) = mpsc::unbounded_channel();
                let cap = Capture::spawn(Capture(tx));
                rt.subscribe::<Capture, PagePreviewRequested>(&cap).await;
                Self {
                    stores: Arc::new(Stores::new(state.clone())),
                    rt,
                    state,
                    requests: tokio::sync::Mutex::new(requests),
                    dir,
                }
            }

            pub fn cx(&self, session: SessionId) -> ToolCx {
                ToolCx {
                    session,
                    call_id: "c".into(),
                    quiet: false,
                    resources: tools::SessionResources::durable_arc(self.dir.path()),
                    cancel: tools::CancellationToken::new(),
                    promote_after: None,
                }
            }

            pub async fn preview(&self, session: SessionId, args: PreviewArgs) -> ToolOutput {
                let tool = PagePreviewTool {
                    rt: self.rt.clone(),
                    stores: self.stores.clone(),
                };
                Tool::call(&tool, args, self.cx(session)).await.unwrap()
            }

            /// The next request's id, answered with `a`.
            pub async fn reply(&self, a: PagePreviewAnswer) -> PreviewId {
                let id = self.requests.lock().await.recv().await.unwrap();
                assert!(answer(id, a));
                id
            }

            /// Whether a request is waiting unanswered.
            pub async fn pending(&self) -> bool {
                self.requests.lock().await.try_recv().is_ok()
            }
        }

        pub(super) fn page(name: &str, html: &str) -> ShowArgs {
            ShowArgs {
                content: Some(html.into()),
                name: Some(name.into()),
                ..ShowArgs::default()
            }
        }

        pub(super) fn done(
            console: Vec<ConsoleLine>,
            client: Client,
            height: u32,
        ) -> PagePreviewAnswer {
            PagePreviewAnswer::Done {
                content_height: height,
                console,
                loaded: true,
                client,
            }
        }
    }

    use crate::ShowArgs;
    use client::{Harness, done as answered, page};

    /// 150 §D3: `path` reads the file; `html` is saved as the named draft and the result says so.
    #[tokio::test]
    async fn page_preview_takes_a_path_or_saves_html_as_a_draft() {
        let h = Harness::new().await;
        let sess = SessionId::new();
        let file = h.dir.path().join("p.html");
        std::fs::write(&file, "<p>from disk</p>").unwrap();
        let (out, _) = tokio::join!(
            h.preview(
                sess,
                PreviewArgs {
                    path: Some(file.clone()),
                    ..Default::default()
                }
            ),
            h.reply(answered(vec![], Client::Desktop, 300))
        );
        assert!(!out.is_error, "{}", out.content);
        assert!(
            out.content
                .starts_with("Previewed at 728 px: contentHeight 300, 0 errors.")
        );
        assert!(!out.content.contains(" · file "), "{}", out.content);

        let (out, _) = tokio::join!(
            h.preview(
                sess,
                PreviewArgs {
                    html: Some("<p>x</p>".into()),
                    name: Some("chart.html".into()),
                    ..Default::default()
                }
            ),
            h.reply(answered(vec![], Client::Desktop, 120))
        );
        let draft = h.state.drafts_dir(sess).join("chart.html");
        assert_eq!(std::fs::read_to_string(&draft).unwrap(), "<p>x</p>");
        assert!(
            out.content.ends_with(&format!(
                " · file {p} — show{{path: \"{p}\"}} publishes exactly this page",
                p = draft.display()
            )),
            "{}",
            out.content
        );
        assert_eq!(out.structured.unwrap()["file"], draft.display().to_string());
        // Default name.
        let (_, _) = tokio::join!(
            h.preview(
                sess,
                PreviewArgs {
                    html: Some("<p>y</p>".into()),
                    ..Default::default()
                }
            ),
            h.reply(answered(vec![], Client::Desktop, 120))
        );
        assert!(h.state.drafts_dir(sess).join("preview.html").exists());

        for (args, want) in [
            (
                PreviewArgs {
                    html: Some("<p>x</p>".into()),
                    path: Some(file.clone()),
                    ..Default::default()
                },
                "page_preview: pass either html or path, not both",
            ),
            (
                PreviewArgs::default(),
                "page_preview: pass html (the page) or path (an existing .html file)",
            ),
            (
                PreviewArgs {
                    html: Some(String::new()),
                    ..Default::default()
                },
                "page_preview: pass html (the page) or path (an existing .html file)",
            ),
        ] {
            let out = h.preview(sess, args).await;
            assert!(out.is_error);
            assert_eq!(out.content, want);
        }
        let out = h
            .preview(
                sess,
                PreviewArgs {
                    path: Some(h.dir.path().join("nope.html")),
                    ..Default::default()
                },
            )
            .await;
        assert!(
            out.is_error && out.content.starts_with("page_preview: "),
            "{}",
            out.content
        );
    }

    /// 150 §D4: `show` checks an `.html` page before publishing it.
    #[tokio::test]
    async fn show_checks_pages_before_publishing() {
        let h = Harness::new().await;
        let sess = SessionId::new();
        let err = |t: &str| ConsoleLine {
            level: ConsoleLevel::Error,
            text: t.into(),
        };
        let warn = |t: &str| ConsoleLine {
            level: ConsoleLevel::Warning,
            text: t.into(),
        };

        // Error lines block publication: no row, the draft kept, the fix line present.
        let (r, _) = tokio::join!(
            h.state
                .show(sess, "c1".into(), page("x.html", "<script>boom()</script>")),
            h.reply(answered(
                vec![
                    warn("w"),
                    err("ReferenceError: boom is not defined"),
                    err(&"y".repeat(400))
                ],
                Client::Desktop,
                200
            ))
        );
        let msg = r.unwrap_err();
        let draft = h.state.drafts_dir(sess).join("x.html");
        assert!(
            msg.starts_with(&format!(
                "x.html: not published — the page logged 2 error(s):\n[error] ReferenceError: boom \
                 is not defined\n[error] {}\nsaved at {}",
                "y".repeat(300),
                draft.display()
            )),
            "{msg}"
        );
        assert!(
            msg.ends_with("; do not resend the page, or pass check: false to publish anyway"),
            "{msg}"
        );
        assert_eq!(
            std::fs::read_to_string(&draft).unwrap(),
            "<script>boom()</script>"
        );
        assert!(h.state.list(Some(sess), false).await.unwrap().is_empty());

        // A clean Done publishes with the check in the receipt.
        let (r, _) = tokio::join!(
            h.state.show(sess, "c2".into(), page("x.html", "<p>ok</p>")),
            h.reply(answered(vec![warn("w")], Client::Desktop, 612))
        );
        let shown = r.unwrap();
        assert_eq!(
            shown.receipt.check,
            Some(crate::kind::Check::Ok {
                client: "desktop",
                height: Some(612),
                warnings: 1
            })
        );
        let line = crate::kind::receipt(&shown.artifact, &shown.receipt);
        assert!(
            line.contains(" · checked on desktop: height 612 · 1 warning(s) · file "),
            "{line}"
        );
        // The web client's unmeasured 80.
        let (r, _) = tokio::join!(
            h.state
                .show(sess, "c3".into(), page("x.html", "<p>ok2</p>")),
            h.reply(answered(vec![], Client::Web, 80))
        );
        let shown = r.unwrap();
        assert_eq!(
            shown.receipt.check,
            Some(crate::kind::Check::Ok {
                client: "web",
                height: None,
                warnings: 0
            })
        );

        // A client failure publishes unchecked.
        let (r, _) = tokio::join!(
            h.state.show(sess, "c4".into(), page("y.html", "<p>y</p>")),
            h.reply(PagePreviewAnswer::Failed {
                reason: "page_put failed".into()
            })
        );
        let shown = r.unwrap();
        assert_eq!(
            shown.receipt.check,
            Some(crate::kind::Check::Skipped(
                "client failed: page_put failed".into()
            ))
        );

        // check: false skips it: no request reaches the client.
        let shown = h
            .state
            .show(
                sess,
                "c5".into(),
                ShowArgs {
                    check: Some(false),
                    ..page("z.html", "<p>z</p>")
                },
            )
            .await
            .unwrap();
        assert_eq!(shown.receipt.check, None);
        assert!(!h.pending().await);

        // Two page shows in one session run in parallel and are both checked.
        let (a, b, _, _) = tokio::join!(
            h.state.show(sess, "p1".into(), page("p1.html", "<p>1</p>")),
            h.state.show(sess, "p2".into(), page("p2.html", "<p>2</p>")),
            h.reply(answered(vec![], Client::Phone, 100)),
            h.reply(answered(vec![], Client::Phone, 100))
        );
        for r in [a, b] {
            let shown = r.unwrap();
            assert_eq!(
                shown.receipt.check,
                Some(crate::kind::Check::Ok {
                    client: "phone",
                    height: Some(100),
                    warnings: 0
                })
            );
        }
    }

    /// 150 §D4: a subscriber that never answers → the 2 s window → published unchecked.
    #[tokio::test]
    async fn show_publishes_unchecked_without_a_renderer() {
        let h = Harness::new().await;
        let sess = SessionId::new();
        let shown = h
            .state
            .show(sess, "c1".into(), page("x.html", "<p>x</p>"))
            .await
            .unwrap();
        assert!(h.pending().await, "the request went out");
        assert_eq!(
            shown.receipt.check,
            Some(crate::kind::Check::Skipped("no renderer connected".into()))
        );
        assert!(
            crate::kind::receipt(&shown.artifact, &shown.receipt)
                .contains(" · not checked: no renderer connected · file "),
        );
    }
}
