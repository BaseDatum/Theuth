//! Run-local todo list: state, semantics, rendering, tool spec (design doc 07 §9).

use schemars::JsonSchema;
use serde::Deserialize;
use tools::{Risk, ToolSpec};

/// Tool name.
pub const NAME: &str = "todo";
/// Rounds without a `todo` call (with open items) before the loop appends a `<todo>` note.
pub const TODO_REMIND_ROUNDS: u32 = 4;
/// Maximum items in a list.
const CAP: usize = 32;

/// One item; identified by its text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Todo {
    /// The item, as the model wrote it.
    pub text: String,
    /// Current state.
    pub status: Status,
}

/// Item state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Not started.
    Pending,
    /// Being worked on (at most one, kept by auto-promotion).
    InProgress,
    /// Finished.
    Done,
    /// Abandoned.
    Dropped,
}

/// Operation.
#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    /// Replace the list with `items`.
    Init,
    /// Append `items`.
    Add,
    /// Mark `task` in progress.
    Start,
    /// Mark `task` done.
    Done,
    /// Mark `task` dropped.
    Drop,
    /// Return the list unchanged.
    View,
}

/// Arguments of the `todo` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct TodoArgs {
    /// What to do.
    pub op: Op,
    /// Item texts for `init` and `add`.
    #[serde(default)]
    pub items: Vec<String>,
    /// Exact text of an existing item for `start`, `done`, `drop`.
    pub task: Option<String>,
}

/// Apply one operation (07 §9). After any change, if nothing is in progress
/// the first pending item is promoted.
pub fn apply(list: &mut Vec<Todo>, args: TodoArgs) -> Result<(), String> {
    match args.op {
        Op::View => return Ok(()),
        Op::Init | Op::Add => {
            let base = if args.op == Op::Init { 0 } else { list.len() };
            if base + args.items.len() > CAP {
                return Err(format!("todo list is capped at {CAP} items"));
            }
            if args.op == Op::Init {
                list.clear();
            }
            list.extend(args.items.into_iter().map(|text| Todo {
                text,
                status: Status::Pending,
            }));
        }
        Op::Start | Op::Done | Op::Drop => {
            let task = args.task.unwrap_or_default();
            let item = list
                .iter_mut()
                .find(|t| t.text == task)
                .ok_or_else(|| format!("unknown task: {task:?}"))?;
            item.status = match args.op {
                Op::Start => Status::InProgress,
                Op::Done => Status::Done,
                _ => Status::Dropped,
            };
        }
    }
    if !list.iter().any(|t| t.status == Status::InProgress)
        && let Some(first) = list.iter_mut().find(|t| t.status == Status::Pending)
    {
        first.status = Status::InProgress;
    }
    Ok(())
}

/// Text-glyph rendering used by the tool output, the `<todo>` note and the compaction head.
pub fn render(list: &[Todo]) -> String {
    if list.is_empty() {
        return "Todo: (empty)".into();
    }
    let done = list.iter().filter(|t| t.status == Status::Done).count();
    let mut out = format!("Todo ({done}/{} done):", list.len());
    for t in list {
        let (glyph, suffix) = match t.status {
            Status::Pending => ("☐", ""),
            Status::InProgress => ("▶", ""),
            Status::Done => ("☑", ""),
            Status::Dropped => ("✗", "   (dropped)"),
        };
        out.push_str(&format!("\n  {glyph} {}{suffix}", t.text));
    }
    out
}

/// Spec of the loop-local `todo` tool.
pub fn spec() -> ToolSpec {
    ToolSpec {
        name: NAME.into(),
        description: "Plan and track multi-step work in a session-local todo list; the \
            result is always the full rendered list. Ops: `init` replaces the list with \
            `items`, `add` appends `items`, `start`/`done`/`drop` set the status of the item \
            whose exact text is `task`, `view` shows the list. Item texts are the \
            identifiers. When nothing is in progress the first pending item is started \
            automatically. At most 32 items."
            .into(),
        schema: serde_json::to_value(schemars::schema_for!(TodoArgs)).unwrap_or_default(),
        risk: Risk::ReadOnly,
        direct: true,
        returns: "str".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(op: Op, items: &[&str], task: Option<&str>) -> TodoArgs {
        TodoArgs {
            op,
            items: items.iter().map(|s| s.to_string()).collect(),
            task: task.map(str::to_string),
        }
    }

    #[test]
    fn auto_promotes_first_pending() {
        let mut list = Vec::new();
        apply(&mut list, args(Op::Init, &["a", "b"], None)).unwrap();
        assert_eq!(list[0].status, Status::InProgress);
        assert_eq!(list[1].status, Status::Pending);
        apply(&mut list, args(Op::Done, &[], Some("a"))).unwrap();
        assert_eq!(list[0].status, Status::Done);
        assert_eq!(list[1].status, Status::InProgress);
        assert_eq!(render(&list), "Todo (1/2 done):\n  ☑ a\n  ▶ b");
    }

    #[test]
    fn unknown_task_and_cap_are_errors() {
        let mut list = Vec::new();
        assert!(apply(&mut list, args(Op::Done, &[], Some("nope"))).is_err());
        let many: Vec<String> = (0..32).map(|i| i.to_string()).collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        apply(&mut list, args(Op::Init, &refs, None)).unwrap();
        assert!(apply(&mut list, args(Op::Add, &["33rd"], None)).is_err());
        assert_eq!(list.len(), 32);
    }
}
