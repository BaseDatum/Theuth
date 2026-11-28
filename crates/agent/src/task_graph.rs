//! Ephemeral task-graph send context for the floating Task Graph chat composer.
//!
//! Modeled on [`crate::whiteboards`] send context: the UI calls `task_graph_send_context`
//! right before `send_input`/`steer`, storing a short-lived (120 s) snapshot keyed by
//! [`SessionId`]. At turn start (`round == 0`), [`inject`] consumes the snapshot once and
//! formats a `<task_graph ...>` user-role message listing the board view and instructing the
//! agent to mutate tasks directly with `task_create` / `task_update`.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use runtime::SessionId;
use serde::{Deserialize, Serialize};

/// A pending task-graph send context is dropped after this long.
pub const CONTEXT_TTL: Duration = Duration::from_secs(120);

/// Maximum number of task rows listed inside one `<task_graph>` message.
pub const MAX_TASKS: usize = 80;

/// Guidance appended to every `<task_graph>` message.
pub const INSTRUCTION: &str = "The user is speaking to you from the live Task Graph view. \
When they ask to add, link, edit, or organize tasks on this board, call `task_create` / `task_update` directly \
(setting `project` to the active project when one is shown, and `state: \"backlog\"` unless they ask to \
start/dispatch immediately) instead of `task_draft`, so the tasks appear immediately on the graph for them to open and refine.";

/// One task row carried by [`TaskGraphSendContext`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct TaskGraphContextItem {
    /// Task id.
    pub id: String,
    /// One-line task title.
    pub title: String,
    /// Current state (`backlog`, `todo`, `doing`, `review`, `blocked`, `done`, `cancelled`).
    pub state: String,
    /// Parent task id, if any.
    #[serde(default)]
    pub parent: Option<String>,
    /// Blocker task ids or external blockers.
    #[serde(default)]
    pub blockers: Vec<String>,
}

/// Ephemeral view context handed over by the Task Graph floating composer right before `send_input`/`steer`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct TaskGraphSendContext {
    /// Active project filter on the graph; `None` means all projects.
    #[serde(default)]
    pub project: Option<String>,
    /// Currently selected task id on the graph, if any.
    #[serde(default)]
    pub selected: Option<String>,
    /// Tasks currently visible on the graph (up to [`MAX_TASKS`] are emitted).
    #[serde(default)]
    pub tasks: Vec<TaskGraphContextItem>,
}

/// Pending task-graph send contexts: session → (stored at, context).
static CONTEXTS: LazyLock<Mutex<HashMap<SessionId, (Instant, TaskGraphSendContext)>>> =
    LazyLock::new(Default::default);

/// Store `ctx` for the next turn of `session` (replaces any earlier entry, evicts expired ones).
pub fn put_context(session: SessionId, ctx: TaskGraphSendContext) {
    let mut map = CONTEXTS.lock().unwrap_or_else(|p| p.into_inner());
    map.retain(|_, (at, _)| at.elapsed() < CONTEXT_TTL);
    map.insert(session, (Instant::now(), ctx));
}

/// Consume the pending context of `session`, if one was stored within [`CONTEXT_TTL`].
pub fn take_context(session: SessionId) -> Option<TaskGraphSendContext> {
    let mut map = CONTEXTS.lock().unwrap_or_else(|p| p.into_inner());
    map.retain(|_, (at, _)| at.elapsed() < CONTEXT_TTL);
    map.remove(&session).map(|(_, c)| c)
}

/// Consume `session`'s pending task-graph context (if any) and format the `<task_graph>` message.
pub fn inject(session: SessionId) -> Option<String> {
    take_context(session).map(|ctx| format_context(&ctx))
}

fn attr_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn flatten(s: &str) -> String {
    s.chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect()
}

/// Format a [`TaskGraphSendContext`] as a `<task_graph ...>...</task_graph>` message.
pub fn format_context(ctx: &TaskGraphSendContext) -> String {
    let project = ctx
        .project
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .unwrap_or("all");
    let selected = ctx
        .selected
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let mut out = format!("<task_graph project=\"{}\"", attr_escape(project));
    if let Some(sel) = selected {
        let _ = write!(out, " selected=\"{}\"", attr_escape(sel));
    }
    out.push_str(">\n");
    out.push_str(INSTRUCTION);
    out.push('\n');

    if let Some(sel) = selected {
        let _ = writeln!(out, "selected: {sel}");
    }

    if ctx.tasks.is_empty() {
        out.push_str("(no tasks)\n");
    } else {
        for item in ctx.tasks.iter().take(MAX_TASKS) {
            let _ = write!(
                out,
                "{} | {} | {}",
                flatten(item.id.trim()),
                flatten(item.state.trim()),
                flatten(item.title.trim())
            );
            if let Some(parent) = item
                .parent
                .as_deref()
                .map(str::trim)
                .filter(|p| !p.is_empty())
            {
                let _ = write!(out, " | parent={}", flatten(parent));
            }
            if !item.blockers.is_empty() {
                let blockers: Vec<String> = item
                    .blockers
                    .iter()
                    .map(|b| flatten(b.trim()))
                    .filter(|b| !b.is_empty())
                    .collect();
                if !blockers.is_empty() {
                    let _ = write!(out, " | blockers={}", blockers.join(", "));
                }
            }
            out.push('\n');
        }
        if ctx.tasks.len() > MAX_TASKS {
            let _ = writeln!(
                out,
                "… {} more tasks omitted (showing first {MAX_TASKS})",
                ctx.tasks.len() - MAX_TASKS
            );
        }
    }

    out.push_str("</task_graph>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_take_and_inject_consume_once() {
        let session = SessionId::new();
        assert_eq!(take_context(session), None);
        assert_eq!(inject(session), None);

        let ctx = TaskGraphSendContext {
            project: Some("theuth".into()),
            selected: Some("t-1".into()),
            tasks: vec![TaskGraphContextItem {
                id: "t-1".into(),
                title: "Ship task graph composer".into(),
                state: "backlog".into(),
                parent: Some("t-0".into()),
                blockers: vec!["t-pre".into()],
            }],
        };
        put_context(session, ctx.clone());
        assert_eq!(take_context(session), Some(ctx.clone()));
        assert_eq!(take_context(session), None, "consumed on first take");

        put_context(session, ctx);
        let msg = inject(session).expect("injected once");
        assert!(msg.starts_with("<task_graph project=\"theuth\" selected=\"t-1\">\n"));
        assert!(msg.contains(INSTRUCTION));
        assert!(msg.contains("t-1 | backlog | Ship task graph composer | parent=t-0 | blockers=t-pre"));
        assert!(msg.ends_with("</task_graph>"));
        assert_eq!(inject(session), None, "consumed on inject");
    }

    #[test]
    fn format_empty_defaults_to_all_projects() {
        let msg = format_context(&TaskGraphSendContext::default());
        assert!(msg.starts_with("<task_graph project=\"all\">\n"), "{msg}");
        assert!(msg.contains(INSTRUCTION), "{msg}");
        assert!(msg.contains("(no tasks)\n"), "{msg}");
        assert!(msg.ends_with("</task_graph>"), "{msg}");
    }

    #[test]
    fn format_project_scoped_with_optional_fields() {
        let msg = format_context(&TaskGraphSendContext {
            project: Some("relay".into()),
            selected: Some("t2".into()),
            tasks: vec![
                TaskGraphContextItem {
                    id: "t1".into(),
                    title: "Root epic".into(),
                    state: "doing".into(),
                    parent: None,
                    blockers: vec![],
                },
                TaskGraphContextItem {
                    id: "t2".into(),
                    title: "Child task".into(),
                    state: "blocked".into(),
                    parent: Some("t1".into()),
                    blockers: vec!["t0".into(), "decision:d1".into()],
                },
            ],
        });
        assert!(msg.starts_with("<task_graph project=\"relay\" selected=\"t2\">\n"));
        assert!(msg.contains("selected: t2\n"));
        assert!(msg.contains("t1 | doing | Root epic\n"));
        assert!(msg.contains("t2 | blocked | Child task | parent=t1 | blockers=t0, decision:d1\n"));
    }

    #[test]
    fn format_caps_at_80_tasks() {
        let tasks: Vec<TaskGraphContextItem> = (1..=95)
            .map(|i| TaskGraphContextItem {
                id: format!("t-{i}"),
                title: format!("Task {i}"),
                state: "backlog".into(),
                parent: None,
                blockers: vec![],
            })
            .collect();
        let msg = format_context(&TaskGraphSendContext {
            project: Some("big".into()),
            selected: None,
            tasks,
        });
        assert!(msg.contains("t-1 | backlog | Task 1\n"));
        assert!(msg.contains("t-80 | backlog | Task 80\n"));
        assert!(!msg.contains("t-81 | backlog | Task 81\n"));
        assert!(msg.contains("… 15 more tasks omitted (showing first 80)\n"));
    }
}
