//! `page_preview` (120 §11): the agent hands over an HTML page, a connected
//! desktop/web client renders it in a hidden sandboxed frame and reports the
//! height it needs plus its console. Same shape as `ask_user`
//! (`tools::ask`): publish [`PagePreviewRequested`], wait for answers through
//! [`answer`], publish [`PagePreviewResolved`].
//!
//! Not in `PendingPrompts`: a preview nobody rendered in time is dead, not
//! pending (§11.2). The pending map is a process-wide static — one tool
//! instance, one RPC entry point, no actor needed.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use std::time::Duration;

use runtime::{Runtime, SessionId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc};
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

/// Event: the request was answered or timed out. Clients still rendering
/// tear their frame down.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub struct PagePreviewResolved {
    /// The request.
    pub id: PreviewId,
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

/// Arguments of `page_preview`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct PreviewArgs {
    /// The page: one self-contained HTML document, 1–512 000 chars, exactly what you would pass
    /// to `show`.
    pub html: String,
    /// Frame width in CSS px, 240–1600. Default 728 (the reply column).
    pub width: Option<u32>,
    /// `dark` or `light`. Default: the client's current appearance.
    pub appearance: Option<Appearance>,
}

/// A waiting tool call: its session and where the answers go.
struct Pending {
    session: SessionId,
    tx: mpsc::UnboundedSender<PagePreviewAnswer>,
}

static PENDING: LazyLock<Mutex<HashMap<PreviewId, Pending>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The rpc bridge's broadcast sender, when the rpc plugin runs: its
/// `receiver_count()` is how many event subscribers exist at all (§11.4).
static FEED: OnceLock<broadcast::Sender<Arc<serde_json::Value>>> = OnceLock::new();

/// Install the event feed whose subscriber count gates the "no renderer"
/// short-circuit. Called once by the rpc bridge; later calls are ignored.
pub fn watch_feed(tx: broadcast::Sender<Arc<serde_json::Value>>) {
    let _ = FEED.set(tx);
}

/// A client's answer. `true` if the preview was still pending: the first
/// `Done`/`Failed` resolves it, a `Started` extends the wait and leaves it
/// pending. Every frontend answers through here.
pub fn answer(id: PreviewId, answer: PagePreviewAnswer) -> bool {
    let mut map = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    let terminal = !matches!(answer, PagePreviewAnswer::Started { .. });
    let Some(p) = map.get(&id) else { return false };
    let delivered = p.tx.send(answer).is_ok();
    if terminal {
        map.remove(&id);
    }
    delivered
}

/// The `page_preview` tool (§11.1).
pub struct PagePreviewTool {
    /// For publishing the two events.
    pub rt: Runtime,
}

#[async_trait::async_trait]
impl Tool for PagePreviewTool {
    const NAME: &'static str = "page_preview";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "dict";
    const DIRECT: bool = true;
    type Args = PreviewArgs;

    fn description(&self) -> String {
        "Load an HTML page hidden in the connected theuth desktop and get back `contentHeight` \
         (the height the page needs at `width`) and its console output: log, info, warning, error \
         and uncaught exceptions. `console.log` is a fine way to report your own checks. Use it \
         before `show` when the page has scripts. No screenshot. Fails fast with \"no renderer \
         connected\" when no desktop or web client is open; then pass a sensible `height` to \
         `show` and go on."
            .into()
    }

    async fn call(&self, args: PreviewArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let n = args.html.chars().count();
        if n == 0 || n > MAX_HTML {
            return Ok(ToolOutput::err(format!(
                "page_preview: html must be 1–{MAX_HTML} chars (got {n})"
            )));
        }
        let width = args.width.unwrap_or(DEFAULT_WIDTH);
        if !WIDTH.contains(&width) {
            return Ok(ToolOutput::err(format!(
                "page_preview: width must be {}–{} (got {width})",
                WIDTH.start(),
                WIDTH.end()
            )));
        }
        if FEED.get().is_some_and(|tx| tx.receiver_count() == 0) {
            return Ok(ToolOutput::err(NO_RENDERER_MSG));
        }
        let id = PreviewId(uuid::Uuid::now_v7());
        let (tx, mut rx) = mpsc::unbounded_channel();
        {
            let mut map = PENDING.lock().unwrap_or_else(|e| e.into_inner());
            if map.values().any(|p| p.session == cx.session) {
                return Ok(ToolOutput::err(
                    "page_preview: a preview is already running",
                ));
            }
            map.insert(
                id,
                Pending {
                    session: cx.session,
                    tx,
                },
            );
        }
        self.rt
            .publish(PagePreviewRequested {
                id,
                session: cx.session,
                html: args.html,
                width,
                appearance: args.appearance,
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
                })) => break done(width, content_height, console, loaded, client),
                Ok(Some(PagePreviewAnswer::Failed { reason })) => {
                    break ToolOutput::err(format!("page_preview: {reason}"));
                }
                // Sender dropped only when the map entry was removed, which
                // `answer` does together with the terminal send.
                Ok(None) => break ToolOutput::err("page_preview: cancelled"),
                Err(_) if deadline == started + NO_RENDERER => {
                    break ToolOutput::err(NO_RENDERER_MSG);
                }
                Err(_) => {
                    break ToolOutput::err(
                        "page_preview: preview timed out after 15 s (the page did not finish \
                         loading); console so far: none",
                    );
                }
            }
        };
        PENDING
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
        self.rt.publish(PagePreviewResolved { id }).await;
        Ok(out)
    }
}

const NO_RENDERER_MSG: &str = "page_preview: no renderer connected — open the theuth desktop or \
                               web app, or pass height to show and skip the preview";

/// The `Done` result: the §11.5 message line plus the §11.1 dict.
fn done(
    width: u32,
    content_height: u32,
    console: Vec<ConsoleLine>,
    loaded: bool,
    client: Client,
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
    for l in &console {
        let _ = write!(content, "\n[{}] {}", level_name(l.level), l.text);
    }
    ToolOutput {
        structured: Some(serde_json::json!({
            "width": width,
            "contentHeight": content_height,
            "console": console,
            "loaded": loaded,
            "client": client,
        })),
        ..ToolOutput::ok(content)
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

        let out = done(400, 80, vec![], false, Client::Web);
        assert!(out.content.starts_with("Previewed at 400 px: contentHeight 80, 0 errors."));
        assert!(out.content.contains("did not finish loading"));
    }

    #[test]
    fn answer_without_a_pending_id_is_false() {
        assert!(!answer(
            PreviewId(uuid::Uuid::now_v7()),
            PagePreviewAnswer::Failed {
                reason: "x".into()
            }
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
        assert!(matches!(d, PagePreviewAnswer::Done { content_height: 10, .. }));
        let r = PagePreviewResolved {
            id: PreviewId(uuid::Uuid::nil()),
        };
        assert_eq!(
            serde_json::to_value(&r).unwrap()["type"],
            "PagePreviewResolved"
        );
    }
}
