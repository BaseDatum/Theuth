//! Agent layout control and universal panes: `show_view`, `pane_manage`, and `pane_list`.
//!
//! Follows the same engine-to-client request/response broker pattern as [`crate::preview`]
//! and [`crate::webview`]: publish [`LayoutCommandRequested`], wait for one terminal
//! [`LayoutCommandAnswer`] via [`answer`] (RPC `answer_layout_command`), and publish
//! [`LayoutCommandResolved`] on completion, timeout, or drop.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use runtime::{Runtime, SessionId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use tools::{Risk, Tool, ToolCx, ToolOutput};

use crate::webview::clean_title;

/// Longest accepted view route path, in Unicode scalar values.
pub const MAX_PATH_CHARS: usize = 512;

/// Maximum time to wait for a connected UI client to execute a layout command.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

/// Allowed split share range for `pane_manage` `resize`.
const SHARE_RANGE: std::ops::RangeInclusive<f64> = 0.1..=0.9;

/// Correlates a layout command request with its answer and resolution.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(transparent)]
pub struct LayoutCallId(pub uuid::Uuid);

/// Target edge when moving a pane relative to another pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum PaneEdge {
    /// Split to the left of the target pane.
    Left,
    /// Split to the right of the target pane.
    Right,
    /// Split above the target pane.
    Top,
    /// Split below the target pane.
    Bottom,
    /// Group as a tab inside the target pane's tab strip.
    Tab,
    /// Replace or center-drop onto the target pane.
    Center,
}

/// Placement hint when opening an application view with `show_view`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum PanePlacement {
    /// Open beside the active pane (default workspace side-pane placement).
    Beside,
    /// Split to the left of the target (or active) pane.
    Left,
    /// Split to the right of the target (or active) pane.
    Right,
    /// Split above the target (or active) pane.
    Top,
    /// Split below the target (or active) pane.
    Bottom,
    /// Add as a tab alongside the target pane.
    Tab,
    /// Replace the target (or active) pane in place.
    Replace,
}

/// Operation requested by an agent layout tool and executed by the desktop UI.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum LayoutOp {
    /// List open panes, active route, focus/selection state, and layout tree.
    List,
    /// Open (or reuse) an application route in a pane.
    OpenView {
        /// Route path starting with `/`.
        path: String,
        /// Optional pane title override.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        /// Optional placement hint.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        placement: Option<PanePlacement>,
        /// Optional target pane id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<String>,
        /// Whether to focus/select the opened pane.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        focus: Option<bool>,
    },
    /// Close an open pane by id.
    Close {
        /// Pane id to close.
        pane: String,
    },
    /// Focus or select an open pane by id.
    Focus {
        /// Pane id to focus or select.
        pane: String,
    },
    /// Move an open pane relative to a target pane.
    Move {
        /// Pane id to move.
        pane: String,
        /// Target pane id.
        target: String,
        /// Edge relative to `target`.
        edge: PaneEdge,
    },
    /// Resize a pane's share of its split container.
    Resize {
        /// Pane id to resize.
        pane: String,
        /// Fractional share in `0.1..=0.9`.
        share: f64,
    },
}

/// Event: an agent tool wants the connected desktop client to inspect or mutate workspace panes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct LayoutCommandRequested {
    /// Correlation id to pass back to `answer_layout_command`.
    pub id: LayoutCallId,
    /// Owning session of the calling tool.
    pub session: SessionId,
    /// Layout operation to execute.
    pub op: LayoutOp,
}

impl wire::WireEvent for LayoutCommandRequested {
    const TOPIC: &'static str = "artifact.layout-command-requested";
    /// Engine-to-shell request, not an event of the world.
    const EXPOSURE: wire::Exposure = wire::Exposure::Internal;
}

/// Event: a layout command finished, timed out, or was cancelled.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct LayoutCommandResolved {
    /// Correlation id of the resolved request.
    pub id: LayoutCallId,
}

impl wire::WireEvent for LayoutCommandResolved {
    const TOPIC: &'static str = "artifact.layout-command-resolved";
    /// Engine-to-shell request resolution, not an event of the world.
    const EXPOSURE: wire::Exposure = wire::Exposure::Internal;
}

/// A client's terminal answer to a [`LayoutCommandRequested`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LayoutCommandAnswer {
    /// The layout command succeeded and produced a structured JSON result.
    Done {
        /// Command-specific JSON payload.
        #[cfg_attr(feature = "ts", ts(type = "unknown"))]
        result: serde_json::Value,
    },
    /// The layout command could not be completed.
    Failed {
        /// Human-readable failure reason.
        reason: String,
    },
}

/// Pending layout command waiters on this engine.
static PENDING: LazyLock<Mutex<HashMap<LayoutCallId, oneshot::Sender<LayoutCommandAnswer>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Whether layout command `id` is currently waiting on this engine.
#[must_use]
pub fn pending_here(id: LayoutCallId) -> bool {
    PENDING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains_key(&id)
}

/// Deliver a client's answer to a pending layout command.
///
/// Returns `true` if `id` was still pending and received the answer; `false` if it had already
/// resolved, timed out, or belongs to another engine.
pub fn answer(id: LayoutCallId, answer: LayoutCommandAnswer) -> bool {
    let tx = PENDING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&id);
    let Some(tx) = tx else { return false };
    tx.send(answer).is_ok()
}

/// Result of one [`call`] round trip.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// The client executed the command.
    Done(serde_json::Value),
    /// The client rejected or failed to execute the command.
    Failed(String),
    /// No external client stream is connected.
    NoClient,
    /// No answer arrived within the command timeout.
    TimedOut,
}

/// Drop guard ensuring a cancelled or panicked [`call`] cleans up its pending entry and emits
/// [`LayoutCommandResolved`].
struct Unresolved {
    rt: Option<Runtime>,
    id: LayoutCallId,
}

impl Drop for Unresolved {
    fn drop(&mut self) {
        use runtime::kameo_actors::message_bus::Publish;
        let Some(rt) = self.rt.take() else { return };
        PENDING
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
        let published = rt
            .bus
            .tell(Publish(LayoutCommandResolved { id: self.id }))
            .try_send();
        if published.is_err() {
            tracing::debug!("layout command resolution not published: the bus is gone or full");
        }
    }
}

/// Execute one layout command round trip against a connected client.
///
/// Fast-fails with [`Outcome::NoClient`] when no external client stream is live, otherwise
/// waits up to 10 seconds for an answer via [`answer`].
pub async fn call(rt: &Runtime, session: SessionId, op: LayoutOp) -> Outcome {
    if !crate::preview::feed_live() {
        return Outcome::NoClient;
    }
    call_with_timeout(rt, session, op, COMMAND_TIMEOUT).await
}

async fn call_with_timeout(
    rt: &Runtime,
    session: SessionId,
    op: LayoutOp,
    timeout: Duration,
) -> Outcome {
    let id = LayoutCallId(uuid::Uuid::now_v7());
    let (tx, rx) = oneshot::channel();
    PENDING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id, tx);
    let mut guard = Unresolved {
        rt: Some(rt.clone()),
        id,
    };
    rt.publish(LayoutCommandRequested { id, session, op }).await;
    let out = match tokio::time::timeout(timeout, rx).await {
        Ok(Ok(LayoutCommandAnswer::Done { result })) => Outcome::Done(result),
        Ok(Ok(LayoutCommandAnswer::Failed { reason })) => Outcome::Failed(reason),
        Ok(Err(_)) | Err(_) => Outcome::TimedOut,
    };
    PENDING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&id);
    guard.rt = None;
    rt.publish(LayoutCommandResolved { id }).await;
    out
}

/// Validate a route path for `show_view`.
///
/// Must be non-empty, at most [`MAX_PATH_CHARS`] chars, start with `/` (and not `//`),
/// contain no `..`, and contain no whitespace or control characters.
pub fn validate_view_path(raw: &str) -> Result<String, String> {
    if raw.is_empty() {
        return Err("path must not be empty".into());
    }
    if raw.chars().count() > MAX_PATH_CHARS {
        return Err(format!("path exceeds {MAX_PATH_CHARS} characters"));
    }
    if raw.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("path must not contain whitespace or control characters".into());
    }
    if !raw.starts_with('/') {
        return Err("path must start with '/' (no URL scheme)".into());
    }
    if raw.starts_with("//") {
        return Err("path must not start with '//'".into());
    }
    if raw.contains("..") {
        return Err("path must not contain '..'".into());
    }
    Ok(raw.to_owned())
}

/// Arguments of `show_view`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ShowViewArgs {
    /// App route path starting with `/`, e.g. `/tasks`, `/notebooks`, `/notes`, `/knowledge`,
    /// `/agents`, `/routines`, `/meetings`, `/whiteboards`, `/artifacts`, `/autoresearch`,
    /// `/skills`, `/attention`, `/costs`, or `/settings/appearance`.
    pub path: String,
    /// Optional pane title override.
    pub title: Option<String>,
    /// Where to place the pane: `beside` (default), `left`, `right`, `top`, `bottom`,
    /// `tab`, or `replace`.
    pub placement: Option<PanePlacement>,
    /// Target pane id when placing relative to an existing pane (from `pane_list`).
    pub target: Option<String>,
    /// Whether to select/focus the opened pane.
    pub focus: Option<bool>,
}

/// `show_view`: open any app route as a workspace pane in the connected desktop client.
pub struct ShowViewTool {
    /// Runtime bus used to publish layout command events.
    pub rt: Runtime,
}

#[async_trait::async_trait]
impl Tool for ShowViewTool {
    const NAME: &'static str = "show_view";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "dict";
    const DIRECT: bool = true;
    type Args = ShowViewArgs;

    fn description(&self) -> String {
        "Open any theuth app view in a workspace pane beside the chat (or at a chosen placement). \
         Pass an app route path starting with `/`, such as `/tasks`, `/notebooks`, `/notes`, \
         `/knowledge`, `/agents`, `/routines`, `/meetings`, `/whiteboards`, `/artifacts`, \
         `/autoresearch`, `/skills`, `/attention`, `/costs`, or `/settings/appearance`. \
         Optional `placement` (`beside`, `left`, `right`, `top`, `bottom`, `tab`, `replace`), \
         `target` pane id (from `pane_list`), `title`, and `focus`. Call `pane_list()` for \
         the live `available_views` catalog (including detail templates and plugin views). \
         Reuses an existing pane already showing the same path. Opening a view never closes or \
         replaces a pane showing something else: a new pane joins the panel of other views as a \
         tab, or splits beside the chat; only an explicit `placement: replace` swaps the target \
         pane out."
            .into()
    }

    async fn call(&self, args: ShowViewArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let path = match validate_view_path(&args.path) {
            Ok(p) => p,
            Err(msg) => return Ok(ToolOutput::err(format!("show_view: {msg}"))),
        };
        let target = match args.target {
            Some(t) => {
                let trimmed = t.trim();
                if trimmed.is_empty() {
                    return Ok(ToolOutput::err("show_view: target must not be empty"));
                }
                Some(trimmed.to_owned())
            }
            None => None,
        };
        let title = args
            .title
            .map(|t| clean_title(&t))
            .filter(|t| !t.is_empty());

        let op = LayoutOp::OpenView {
            path: path.clone(),
            title,
            placement: args.placement,
            target,
            focus: args.focus,
        };
        match call(&self.rt, cx.session, op).await {
            Outcome::Done(result) => {
                let pane = result["pane"].as_str().unwrap_or("?");
                let shown_path = result["path"].as_str().unwrap_or(&path);
                let shown_title = result["title"].as_str().unwrap_or(shown_path);
                let reused = result["reused"].as_bool().unwrap_or(false);
                let content = view_receipt(shown_title, shown_path, pane, reused);
                Ok(ToolOutput {
                    structured: Some(result),
                    ..ToolOutput::ok(content)
                })
            }
            Outcome::Failed(reason) => Ok(ToolOutput::err(format!("show_view: {reason}"))),
            Outcome::NoClient => Ok(ToolOutput::err("show_view: no desktop app connected")),
            Outcome::TimedOut => Ok(ToolOutput::err(
                "show_view: timed out waiting for the desktop app to answer",
            )),
        }
    }
}

/// `show_view`'s receipt. The first line, `view opened · <title> · <path> · tile <pane>` (`view reused · …` when the
/// pane already showed the path), is pinned: the transcript card (`ViewOpenedCard.tsx`) reads the path and the pane
/// from it, since the transcript keeps the content, not `structured`. The second line is the human text.
fn view_receipt(title: &str, path: &str, pane: &str, reused: bool) -> String {
    let title = clean_title(title);
    let (state, verb) = if reused {
        ("reused", "Reused")
    } else {
        ("opened", "Opened")
    };
    format!("view {state} · {title} · {path} · tile {pane}\n{verb} {title} ({path}) in pane {pane}")
}

/// Action supported by `pane_manage`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PaneAction {
    /// Close the pane.
    Close,
    /// Focus or select the pane.
    Focus,
    /// Move the pane next to or into a target pane.
    Move,
    /// Resize the pane's share of its split container.
    Resize,
}

/// Arguments of `pane_manage`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct PaneManageArgs {
    /// Layout action: `close`, `focus`, `move`, or `resize`.
    pub action: PaneAction,
    /// Pane id to act on (from `pane_list` or `show_view`).
    pub pane: String,
    /// Target pane id (required when `action` is `move`).
    pub target: Option<String>,
    /// Target edge (required when `action` is `move`): `left`, `right`, `top`, `bottom`,
    /// `tab`, or `center`.
    pub edge: Option<PaneEdge>,
    /// Fractional share of the split container in `0.1..=0.9` (required when `action` is `resize`).
    pub share: Option<f64>,
}

/// Validate [`PaneManageArgs`] and convert them into a [`LayoutOp`].
pub fn build_manage_op(args: PaneManageArgs) -> Result<LayoutOp, String> {
    let pane = args.pane.trim().to_owned();
    if pane.is_empty() {
        return Err("pane must not be empty".into());
    }
    match args.action {
        PaneAction::Close => Ok(LayoutOp::Close { pane }),
        PaneAction::Focus => Ok(LayoutOp::Focus { pane }),
        PaneAction::Move => {
            let target = args
                .target
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| "move requires a non-empty target pane id".to_owned())?;
            let edge = args.edge.ok_or_else(|| {
                "move requires an edge (left, right, top, bottom, tab, or center)".to_owned()
            })?;
            Ok(LayoutOp::Move { pane, target, edge })
        }
        PaneAction::Resize => {
            let share = args
                .share
                .ok_or_else(|| "resize requires share in 0.1..=0.9".to_owned())?;
            if !share.is_finite() || !SHARE_RANGE.contains(&share) {
                return Err("share must be between 0.1 and 0.9".into());
            }
            Ok(LayoutOp::Resize { pane, share })
        }
    }
}

/// `pane_manage`: close, focus, move, or resize an open workspace pane.
pub struct PaneManageTool {
    /// Runtime bus used to publish layout command events.
    pub rt: Runtime,
}

#[async_trait::async_trait]
impl Tool for PaneManageTool {
    const NAME: &'static str = "pane_manage";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "dict";
    const DIRECT: bool = true;
    type Args = PaneManageArgs;

    fn description(&self) -> String {
        "Manage open workspace panes: `close`, `focus`, `move` (requires `target` and `edge`: \
         left/right/top/bottom/tab/center), or `resize` (requires `share` in 0.1..=0.9). \
         Use `pane_list` to discover pane ids. Chat panes cannot be tabbed together by agents; \
         split them instead."
            .into()
    }

    async fn call(&self, args: PaneManageArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let action = args.action;
        let op = match build_manage_op(args) {
            Ok(op) => op,
            Err(msg) => return Ok(ToolOutput::err(format!("pane_manage: {msg}"))),
        };
        match call(&self.rt, cx.session, op).await {
            Outcome::Done(result) => {
                let content = match action {
                    PaneAction::Close => {
                        format!("Closed pane {}", result["closed"].as_str().unwrap_or("?"))
                    }
                    PaneAction::Focus => {
                        format!("Focused pane {}", result["focused"].as_str().unwrap_or("?"))
                    }
                    PaneAction::Move => format!(
                        "Moved pane {} to {} of {}",
                        result["moved"].as_str().unwrap_or("?"),
                        result["edge"].as_str().unwrap_or("?"),
                        result["target"].as_str().unwrap_or("?"),
                    ),
                    PaneAction::Resize => format!(
                        "Resized pane {} to share {}",
                        result["resized"].as_str().unwrap_or("?"),
                        result["share"]
                            .as_f64()
                            .map_or_else(|| "?".into(), |s| format!("{s:.2}")),
                    ),
                };
                Ok(ToolOutput {
                    structured: Some(result),
                    ..ToolOutput::ok(content)
                })
            }
            Outcome::Failed(reason) => Ok(ToolOutput::err(format!("pane_manage: {reason}"))),
            Outcome::NoClient => Ok(ToolOutput::err("pane_manage: no desktop app connected")),
            Outcome::TimedOut => Ok(ToolOutput::err(
                "pane_manage: timed out waiting for the desktop app to answer",
            )),
        }
    }
}

/// Arguments of `pane_list`: none.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct PaneListArgs {}

/// `pane_list`: inspect the current workspace panes, focus/selection, and layout tree.
pub struct PaneListTool {
    /// Runtime bus used to publish layout command events.
    pub rt: Runtime,
}

#[async_trait::async_trait]
impl Tool for PaneListTool {
    const NAME: &'static str = "pane_list";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "dict";
    const DIRECT: bool = false;
    type Args = PaneListArgs;

    fn description(&self) -> String {
        "List the open workspace panes in the connected desktop app: active route, focused chat, \
         selected pane, every pane's id/kind/title/path/panel, and the layout tree. Use the \
         returned pane ids with `show_view` (`target`) and `pane_manage`."
            .into()
    }

    async fn call(&self, _: PaneListArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        match call(&self.rt, cx.session, LayoutOp::List).await {
            Outcome::Done(result) => {
                let content = format_pane_list(&result);
                Ok(ToolOutput {
                    structured: Some(result),
                    ..ToolOutput::ok(content)
                })
            }
            Outcome::Failed(reason) => Ok(ToolOutput::err(format!("pane_list: {reason}"))),
            Outcome::NoClient => Ok(ToolOutput::err("pane_list: no desktop app connected")),
            Outcome::TimedOut => Ok(ToolOutput::err(
                "pane_list: timed out waiting for the desktop app to answer",
            )),
        }
    }
}

fn format_pane_list(result: &serde_json::Value) -> String {
    let mut out = String::new();
    let route = result["active_route"].as_str().unwrap_or("");
    let focused = result["focused"].as_str().unwrap_or("none");
    let selected = result["selected"].as_str().unwrap_or("none");
    let _ = writeln!(
        out,
        "route: {route} | focused: {focused} | selected: {selected}"
    );
    if let Some(panes) = result["panes"].as_array() {
        if panes.is_empty() {
            out.push_str("no panes open");
        } else {
            for p in panes {
                let id = p["id"].as_str().unwrap_or("?");
                let kind = p["kind"].as_str().unwrap_or("?");
                let title = p["title"].as_str().unwrap_or("");
                let path = p["path"].as_str().unwrap_or("");
                let _ = writeln!(out, "- {id} ({kind}) {title} {path}");
            }
        }
    }
    out.trim_end().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use kameo::actor::Spawn;
    use kameo::message::{Context, Message};
    use tokio::sync::mpsc;

    #[test]
    fn path_validation_accepts_valid_routes() {
        assert_eq!(validate_view_path("/tasks").unwrap(), "/tasks");
        assert_eq!(
            validate_view_path("/settings/appearance").unwrap(),
            "/settings/appearance"
        );
        assert_eq!(
            validate_view_path("/notes/abc-123?tab=outline#top").unwrap(),
            "/notes/abc-123?tab=outline#top"
        );
        let max_len = format!("/{}", "a".repeat(MAX_PATH_CHARS - 1));
        assert_eq!(validate_view_path(&max_len).unwrap(), max_len);
    }

    #[test]
    fn path_validation_rejects_invalid_routes() {
        assert!(validate_view_path("").is_err());
        assert!(validate_view_path("tasks").is_err());
        assert!(validate_view_path("https://example.com/tasks").is_err());
        assert!(validate_view_path("javascript:alert(1)").is_err());
        assert!(validate_view_path("//example.com/tasks").is_err());
        assert!(validate_view_path("/../etc/passwd").is_err());
        assert!(validate_view_path("/tasks/../settings").is_err());
        assert!(validate_view_path("/tasks/..").is_err());
        assert!(validate_view_path("/tasks foo").is_err());
        assert!(validate_view_path("/tasks\n/other").is_err());
        assert!(validate_view_path("/tasks\tfoo").is_err());
        assert!(validate_view_path("/tasks\x00").is_err());
        let too_long = format!("/{}", "a".repeat(MAX_PATH_CHARS));
        assert!(validate_view_path(&too_long).is_err());
    }

    /// `ViewOpenedCard.tsx` parses the first line; keep both in step.
    #[test]
    fn view_receipt_pins_its_first_line() {
        assert_eq!(
            view_receipt("Tasks", "/tasks", "tile:view::%2Ftasks", false),
            "view opened · Tasks · /tasks · tile tile:view::%2Ftasks\n\
             Opened Tasks (/tasks) in pane tile:view::%2Ftasks"
        );
        let reused = view_receipt(
            "My\nboard",
            "/whiteboards",
            "tile:view::%2Fwhiteboards",
            true,
        );
        assert_eq!(
            reused.lines().next(),
            Some("view reused · My board · /whiteboards · tile tile:view::%2Fwhiteboards")
        );
        assert_eq!(reused.lines().count(), 2);
    }

    #[test]
    fn manage_action_validation() {
        assert_eq!(
            build_manage_op(PaneManageArgs {
                action: PaneAction::Close,
                pane: "tile:view::%2Ftasks".into(),
                target: None,
                edge: None,
                share: None,
            })
            .unwrap(),
            LayoutOp::Close {
                pane: "tile:view::%2Ftasks".into()
            }
        );
        assert_eq!(
            build_manage_op(PaneManageArgs {
                action: PaneAction::Focus,
                pane: "p1".into(),
                target: None,
                edge: None,
                share: None,
            })
            .unwrap(),
            LayoutOp::Focus { pane: "p1".into() }
        );
        assert_eq!(
            build_manage_op(PaneManageArgs {
                action: PaneAction::Move,
                pane: "p1".into(),
                target: Some("p2".into()),
                edge: Some(PaneEdge::Right),
                share: None,
            })
            .unwrap(),
            LayoutOp::Move {
                pane: "p1".into(),
                target: "p2".into(),
                edge: PaneEdge::Right,
            }
        );
        assert_eq!(
            build_manage_op(PaneManageArgs {
                action: PaneAction::Resize,
                pane: "p1".into(),
                target: None,
                edge: None,
                share: Some(0.1),
            })
            .unwrap(),
            LayoutOp::Resize {
                pane: "p1".into(),
                share: 0.1,
            }
        );
        assert_eq!(
            build_manage_op(PaneManageArgs {
                action: PaneAction::Resize,
                pane: "p1".into(),
                target: None,
                edge: None,
                share: Some(0.9),
            })
            .unwrap(),
            LayoutOp::Resize {
                pane: "p1".into(),
                share: 0.9,
            }
        );

        // Invalid cases
        for action in [
            PaneAction::Close,
            PaneAction::Focus,
            PaneAction::Move,
            PaneAction::Resize,
        ] {
            assert!(
                build_manage_op(PaneManageArgs {
                    action,
                    pane: "   ".into(),
                    target: Some("t".into()),
                    edge: Some(PaneEdge::Left),
                    share: Some(0.5),
                })
                .is_err()
            );
        }
        // Move missing target or edge
        assert!(
            build_manage_op(PaneManageArgs {
                action: PaneAction::Move,
                pane: "p1".into(),
                target: None,
                edge: Some(PaneEdge::Left),
                share: None,
            })
            .is_err()
        );
        assert!(
            build_manage_op(PaneManageArgs {
                action: PaneAction::Move,
                pane: "p1".into(),
                target: Some("  ".into()),
                edge: Some(PaneEdge::Left),
                share: None,
            })
            .is_err()
        );
        assert!(
            build_manage_op(PaneManageArgs {
                action: PaneAction::Move,
                pane: "p1".into(),
                target: Some("p2".into()),
                edge: None,
                share: None,
            })
            .is_err()
        );
        // Resize missing or out-of-range share
        for bad in [
            None,
            Some(0.05),
            Some(0.95),
            Some(f64::NAN),
            Some(f64::INFINITY),
        ] {
            assert!(
                build_manage_op(PaneManageArgs {
                    action: PaneAction::Resize,
                    pane: "p1".into(),
                    target: None,
                    edge: None,
                    share: bad,
                })
                .is_err()
            );
        }
    }

    #[test]
    fn wire_shapes_match_contract() {
        let req = LayoutCommandRequested {
            id: LayoutCallId(uuid::Uuid::nil()),
            session: SessionId::default(),
            op: LayoutOp::OpenView {
                path: "/tasks".into(),
                title: Some("Tasks".into()),
                placement: Some(PanePlacement::Beside),
                target: None,
                focus: Some(true),
            },
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["type"], "LayoutCommandRequested");
        assert_eq!(v["op"]["op"], "open_view");
        assert_eq!(v["op"]["path"], "/tasks");
        assert_eq!(v["op"]["placement"], "beside");
        assert!(v["op"].get("target").is_none());

        let res = LayoutCommandResolved {
            id: LayoutCallId(uuid::Uuid::nil()),
        };
        let rv = serde_json::to_value(&res).unwrap();
        assert_eq!(rv["type"], "LayoutCommandResolved");
        assert_eq!(rv["id"], "00000000-0000-0000-0000-000000000000");

        let done: LayoutCommandAnswer = serde_json::from_value(serde_json::json!({
            "kind": "done",
            "result": { "closed": "p1" }
        }))
        .unwrap();
        assert_eq!(
            done,
            LayoutCommandAnswer::Done {
                result: serde_json::json!({ "closed": "p1" })
            }
        );

        let failed: LayoutCommandAnswer = serde_json::from_value(serde_json::json!({
            "kind": "failed",
            "reason": "cannot close the last remaining pane"
        }))
        .unwrap();
        assert_eq!(
            failed,
            LayoutCommandAnswer::Failed {
                reason: "cannot close the last remaining pane".into()
            }
        );
    }

    #[test]
    fn answer_unknown_id_returns_false() {
        assert!(!answer(
            LayoutCallId(uuid::Uuid::now_v7()),
            LayoutCommandAnswer::Failed {
                reason: "unknown".into()
            }
        ));
    }

    #[derive(kameo::Actor)]
    struct CaptureReq(mpsc::UnboundedSender<LayoutCommandRequested>);

    impl Message<LayoutCommandRequested> for CaptureReq {
        type Reply = ();
        async fn handle(&mut self, m: LayoutCommandRequested, _: &mut Context<Self, ()>) {
            let _ = self.0.send(m);
        }
    }

    #[derive(kameo::Actor)]
    struct CaptureRes(mpsc::UnboundedSender<LayoutCommandResolved>);

    impl Message<LayoutCommandResolved> for CaptureRes {
        type Reply = ();
        async fn handle(&mut self, m: LayoutCommandResolved, _: &mut Context<Self, ()>) {
            let _ = self.0.send(m);
        }
    }

    async fn test_bus() -> (
        Runtime,
        mpsc::UnboundedReceiver<LayoutCommandRequested>,
        mpsc::UnboundedReceiver<LayoutCommandResolved>,
    ) {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let (req_tx, req_rx) = mpsc::unbounded_channel();
        let (res_tx, res_rx) = mpsc::unbounded_channel();
        let req_cap = CaptureReq::spawn(CaptureReq(req_tx));
        let res_cap = CaptureRes::spawn(CaptureRes(res_tx));
        rt.subscribe::<CaptureReq, LayoutCommandRequested>(&req_cap)
            .await;
        rt.subscribe::<CaptureRes, LayoutCommandResolved>(&res_cap)
            .await;
        (rt, req_rx, res_rx)
    }

    #[tokio::test]
    async fn broker_done_and_failed_answers() {
        let (rt, mut req_rx, mut res_rx) = test_bus().await;
        let sess = SessionId::new();

        // Done round trip
        let rt_call = rt.clone();
        let fut = tokio::spawn(async move {
            call_with_timeout(&rt_call, sess, LayoutOp::List, Duration::from_secs(2)).await
        });
        let req = tokio::time::timeout(Duration::from_secs(2), req_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(req.session, sess);
        assert_eq!(req.op, LayoutOp::List);
        assert!(pending_here(req.id));
        assert!(answer(
            req.id,
            LayoutCommandAnswer::Done {
                result: serde_json::json!({ "panes": [] })
            }
        ));
        // Duplicate answer is refused.
        assert!(!answer(
            req.id,
            LayoutCommandAnswer::Failed {
                reason: "late".into()
            }
        ));
        let out = fut.await.unwrap();
        assert_eq!(out, Outcome::Done(serde_json::json!({ "panes": [] })));
        assert!(!pending_here(req.id));
        let res = tokio::time::timeout(Duration::from_secs(2), res_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(res.id, req.id);

        // Failed round trip
        let rt_call = rt.clone();
        let fut = tokio::spawn(async move {
            call_with_timeout(
                &rt_call,
                sess,
                LayoutOp::Close {
                    pane: "only".into(),
                },
                Duration::from_secs(2),
            )
            .await
        });
        let req = tokio::time::timeout(Duration::from_secs(2), req_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(answer(
            req.id,
            LayoutCommandAnswer::Failed {
                reason: "cannot close the last remaining pane".into()
            }
        ));
        let out = fut.await.unwrap();
        assert_eq!(
            out,
            Outcome::Failed("cannot close the last remaining pane".into())
        );
        let res = tokio::time::timeout(Duration::from_secs(2), res_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(res.id, req.id);
    }

    #[tokio::test]
    async fn broker_times_out_and_resolves() {
        let (rt, mut req_rx, mut res_rx) = test_bus().await;
        let sess = SessionId::new();
        let rt_call = rt.clone();
        let fut = tokio::spawn(async move {
            call_with_timeout(&rt_call, sess, LayoutOp::List, Duration::from_millis(40)).await
        });
        let req = tokio::time::timeout(Duration::from_secs(2), req_rx.recv())
            .await
            .unwrap()
            .unwrap();
        let out = fut.await.unwrap();
        assert_eq!(out, Outcome::TimedOut);
        assert!(!pending_here(req.id));
        let res = tokio::time::timeout(Duration::from_secs(2), res_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(res.id, req.id);
    }

    #[tokio::test]
    async fn broker_drop_guard_cleans_up_and_resolves() {
        let (rt, mut req_rx, mut res_rx) = test_bus().await;
        let sess = SessionId::new();
        let rt_call = rt.clone();
        let handle = tokio::spawn(async move {
            let _ =
                call_with_timeout(&rt_call, sess, LayoutOp::List, Duration::from_secs(10)).await;
        });
        let req = tokio::time::timeout(Duration::from_secs(2), req_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(pending_here(req.id));
        handle.abort();
        let _ = handle.await;
        assert!(!pending_here(req.id));
        let res = tokio::time::timeout(Duration::from_secs(2), res_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(res.id, req.id);
    }
}
