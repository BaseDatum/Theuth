//! Run-local todo list: state, semantics, rendering, tool spec (design docs 07 §9, 47 §2/§5.1).

use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tools::{Risk, ToolOutput, ToolSpec};

/// Tool name.
pub const NAME: &str = "todo";
/// Rounds without a `todo` call (with open items) before the loop appends a `<todo>` note.
pub const TODO_REMIND_ROUNDS: u32 = 4;
/// Yield reminders allowed per externally initiated request (47 §4.5).
pub const MAX_CONTINUE_REMINDERS: u8 = 3;
/// Harness-note kind of the yield reminder (47 §2.3).
pub const CONTINUE_NOTE: &str = "todo_continue";
/// Maximum items in a list.
const CAP: usize = 32;
/// Actionable items listed in a yield reminder before eliding the rest.
const REMINDER_ITEMS: usize = 5;

/// One item; identified by its exact text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Todo {
    /// The item, as the model wrote it.
    pub text: String,
    /// Current state.
    pub status: Status,
    /// Why the item is blocked; `Some` iff `status == Blocked`.
    #[serde(default)]
    pub reason: Option<String>,
}

impl Todo {
    fn pending(text: String) -> Self {
        Self {
            text,
            status: Status::Pending,
            reason: None,
        }
    }
}

/// Item state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Not started.
    Pending,
    /// Being worked on (at most one, kept by normalization).
    InProgress,
    /// Verified finished.
    Done,
    /// Intentionally removed from scope.
    Dropped,
    /// Waiting on user input or another external prerequisite (`reason` says which).
    Blocked,
}

impl Status {
    /// Wire/storage name (the serde form).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Done => "done",
            Self::Dropped => "dropped",
            Self::Blocked => "blocked",
        }
    }

    /// Still workable by the model: pending or in progress.
    pub const fn is_actionable(self) -> bool {
        matches!(self, Self::Pending | Self::InProgress)
    }

    /// Done or dropped: never silently reopened.
    const fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Dropped)
    }
}

impl FromStr for Status {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        Ok(match s {
            "pending" => Self::Pending,
            "in_progress" => Self::InProgress,
            "done" => Self::Done,
            "dropped" => Self::Dropped,
            "blocked" => Self::Blocked,
            other => return Err(format!("unknown todo status: {other:?}")),
        })
    }
}

/// Who owns the plan a snapshot shows (47 §5.1/§5.3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum TodoSource {
    /// The loop-local `todo` tool.
    #[default]
    Native,
    /// An external harness's `SessionUpdate::Plan`.
    Acp,
}

impl TodoSource {
    /// Wire/storage name (the serde form).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Acp => "acp",
        }
    }
}

impl FromStr for TodoSource {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "native" => Ok(Self::Native),
            "acp" => Ok(Self::Acp),
            other => Err(format!("unknown todo source: {other:?}")),
        }
    }
}

/// The canonical plan of one session: a full list, replacing any earlier one (47 §5.1).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct TodoSnapshot {
    /// Monotonic per session, including rewind; `0` means "never had a plan".
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub version: u64,
    /// Who wrote the list.
    pub source: TodoSource,
    /// The list.
    pub items: Vec<Todo>,
}

/// Operation.
#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    /// Replace the list with `items` (`[]` clears it).
    Init,
    /// Append `items`.
    Add,
    /// Mark the pending `task` in progress (not an implicit unblock).
    Start,
    /// Mark `task` done: verified completion.
    Done,
    /// Mark `task` dropped: intentional scope removal.
    Drop,
    /// Mark the open `task` blocked with `reason`.
    Block,
    /// Return the blocked `task` to pending and clear its reason.
    Unblock,
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
    /// Text of an existing item for `start`, `done`, `drop`, `block`, `unblock`
    /// (exact; surrounding quotes, case and spacing are forgiven when unambiguous).
    pub task: Option<String>,
    /// Why the item is blocked; required by `block`.
    pub reason: Option<String>,
}

/// Apply one operation (47 §2.2). Validation happens on a working copy: an
/// error leaves `list` untouched. After a mutation at most one item is in
/// progress and, if none is, the earliest pending item is promoted. A no-op
/// (`view`, or a transition into the current state) leaves `list` equal to
/// its input, which is how callers detect "nothing changed".
pub fn apply(list: &mut Vec<Todo>, args: TodoArgs) -> Result<(), String> {
    if args.op == Op::View {
        return Ok(());
    }
    // ponytail: the list is capped at 32, so a clone per mutation is cheaper than undo logic.
    let mut next = list.clone();
    match args.op {
        Op::View => unreachable!(),
        Op::Init | Op::Add => {
            if args.op == Op::Init {
                next.clear();
            }
            if next.len() + args.items.len() > CAP {
                return Err(format!("todo list is capped at {CAP} items"));
            }
            for text in args.items {
                let text = text.trim();
                if text.is_empty() {
                    return Err("todo items must not be blank".into());
                }
                if next.iter().any(|t| key(&t.text) == key(text)) {
                    return Err(format!("duplicate task: {text}"));
                }
                next.push(Todo::pending(text.to_owned()));
            }
        }
        Op::Start | Op::Done | Op::Drop | Op::Block | Op::Unblock => {
            let idx = find(&next, args.task.as_deref().unwrap_or_default())?;
            let cur = next[idx].status;
            let target = match args.op {
                Op::Start => Status::InProgress,
                Op::Done => Status::Done,
                Op::Drop => Status::Dropped,
                Op::Block => Status::Blocked,
                _ => Status::Pending,
            };
            let task = &next[idx].text;
            if cur.is_terminal() && cur != target {
                return Err(format!(
                    "task {task:?} is already {}; done/dropped items are not reopened",
                    cur.as_str()
                ));
            }
            match args.op {
                Op::Start if cur == Status::Blocked => {
                    return Err(format!("task {task:?} is blocked; `unblock` it first"));
                }
                Op::Unblock if cur != Status::Blocked => {
                    return Err(format!("task {task:?} is not blocked"));
                }
                Op::Block => {
                    let reason = args.reason.as_deref().map(str::trim).unwrap_or_default();
                    if reason.is_empty() {
                        return Err("`block` requires a non-empty `reason`".into());
                    }
                    next[idx].reason = Some(reason.to_owned());
                }
                _ => next[idx].reason = None,
            }
            next[idx].status = target;
            if args.op == Op::Start {
                for (i, t) in next.iter_mut().enumerate() {
                    if i != idx && t.status == Status::InProgress {
                        t.status = Status::Pending;
                    }
                }
            }
        }
    }
    if !next.iter().any(|t| t.status == Status::InProgress)
        && let Some(first) = next.iter_mut().find(|t| t.status == Status::Pending)
    {
        first.status = Status::InProgress;
    }
    *list = next;
    Ok(())
}

/// The loop-local `todo` call on `list` (a working copy): the rendered list,
/// or the error. Shared by the direct call and the python runtime's `todo()`.
pub fn call(list: &mut Vec<Todo>, args: serde_json::Value) -> ToolOutput {
    let applied = serde_json::from_value::<TodoArgs>(args)
        .map_err(|e| format!("bad arguments: {e}"))
        .and_then(|a| apply(list, a));
    match applied {
        Ok(()) => ToolOutput::ok(render(list)),
        Err(e) => ToolOutput::err(e),
    }
}

/// One pair of matching surrounding quotes stripped, if `s` has them.
fn unquote(s: &str) -> Option<&str> {
    ['"', '\'', '`', '“', '‘']
        .into_iter()
        .zip(['"', '\'', '`', '”', '’'])
        .find_map(|(open, close)| s.strip_prefix(open)?.strip_suffix(close))
}

/// The forgiving identity of an item text: trimmed, one pair of quotes off,
/// whitespace collapsed, lowercase. Two items never share one ([`apply`]).
fn key(s: &str) -> String {
    let s = s.trim();
    let s = unquote(s).unwrap_or(s);
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// The item `task` names: the exact text (trimmed), else the text without
/// one pair of surrounding quotes, else the one item with the same [`key`].
/// Errors show the text as given and list the items to copy from.
fn find(list: &[Todo], task: &str) -> Result<usize, String> {
    let task = task.trim();
    let exact = |t: &str| list.iter().position(|i| i.text == t);
    if let Some(i) = exact(task).or_else(|| exact(unquote(task)?.trim())) {
        return Ok(i);
    }
    let k = key(task);
    let hits: Vec<usize> = (0..list.len())
        .filter(|&i| key(&list[i].text) == k)
        .collect();
    let numbered = |idx: &[usize]| {
        idx.iter()
            .take(CAP)
            .map(|&i| format!("\n{}. {}", i + 1, list[i].text))
            .collect::<String>()
    };
    match hits[..] {
        [i] => Ok(i),
        [] if list.is_empty() => Err(format!("unknown task: {task}. The list is empty.")),
        [] => Err(format!(
            "unknown task: {task}. Items:{}",
            numbered(&(0..list.len()).collect::<Vec<_>>())
        )),
        _ => Err(format!(
            "ambiguous task: {task} matches several items; pass one exactly:{}",
            numbered(&hits)
        )),
    }
}

/// Anything pending or in progress remains (47 §2.1/§3): the predicate behind
/// the in-turn `<todo>` note and the yield reminder.
pub fn actionable(list: &[Todo]) -> bool {
    list.iter().any(|t| t.status.is_actionable())
}

/// Text-glyph rendering used by the tool output, the `<todo>` note and the compaction head.
pub fn render(list: &[Todo]) -> String {
    if list.is_empty() {
        return "Todo: (empty)".into();
    }
    let done = list.iter().filter(|t| t.status == Status::Done).count();
    let mut out = format!("Todo ({done}/{} done):", list.len());
    for t in list {
        let glyph = match t.status {
            Status::Pending => "☐",
            Status::InProgress => "▸",
            Status::Done => "✓",
            Status::Dropped => "✗",
            Status::Blocked => "⊘",
        };
        out.push_str(&format!("\n  {glyph} {}", t.text));
        match t.status {
            Status::Dropped => out.push_str("   (dropped)"),
            Status::Blocked => {
                out.push_str("   (blocked: ");
                out.push_str(t.reason.as_deref().unwrap_or_default());
                out.push(')');
            }
            _ => {}
        }
    }
    out
}

/// Body of the `<todo_continue>` yield reminder (47 §2.3) for reminder
/// `attempt` (1-based) of [`MAX_CONTINUE_REMINDERS`]. `None` when nothing is
/// actionable or the attempt budget is spent; the loop wraps the body with
/// [`CONTINUE_NOTE`] through its normal harness-note path. The in-progress item
/// leads, then pending items in list order, at most [`REMINDER_ITEMS`] lines.
pub fn render_reminder(list: &[Todo], attempt: u8) -> Option<String> {
    if attempt == 0 || attempt > MAX_CONTINUE_REMINDERS || !actionable(list) {
        return None;
    }
    let mut out = String::from("You yielded with actionable todo items remaining:");
    let items = list
        .iter()
        .filter(|t| t.status == Status::InProgress)
        .chain(list.iter().filter(|t| t.status == Status::Pending));
    let total = items.clone().count();
    for t in items.take(REMINDER_ITEMS) {
        out.push_str("\n- ");
        out.push_str(&escape(&t.text));
    }
    if total > REMINDER_ITEMS {
        out.push_str(&format!("\n- … and {} more", total - REMINDER_ITEMS));
    }
    out.push_str(
        "\nContinue the work. Mark items done only when verified; block items that require \
         external input and explain the blocker. Do not poll background work.",
    );
    out.push_str(&format!(
        "\nReminder {attempt} of {MAX_CONTINUE_REMINDERS} for this request."
    ));
    Some(out)
}

/// Task text as data inside the `<todo_continue>` wrapper: no markup leaks into the delimiters.
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Spec of the loop-local `todo` tool.
pub fn spec() -> ToolSpec {
    ToolSpec {
        name: NAME.into(),
        description: "Plan and track multi-step work in a session-local todo list; the \
            result is always the full rendered list. Ops: `init` replaces the list with \
            `items` (`[]` clears it), `add` appends `items`, `start`/`done`/`drop` set the \
            status of the item whose exact text is `task`, `block` marks it blocked with a \
            required `reason` (user input or another external prerequisite), `unblock` \
            returns it to pending, `view` shows the list. Item texts are the identifiers: \
            unique, non-blank, short and stable. `done` means verified completion; `drop` \
            is an intentional scope removal, not unfinished work. At most one item is in \
            progress; when none is, the first pending item is started automatically. \
            `start` does not unblock and done/dropped items are not reopened. At most 32 items."
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
            reason: None,
        }
    }

    fn block(task: &str, reason: Option<&str>) -> TodoArgs {
        TodoArgs {
            reason: reason.map(str::to_string),
            ..args(Op::Block, &[], Some(task))
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
        assert_eq!(render(&list), "Todo (1/2 done):\n  ✓ a\n  ▸ b");
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

    #[test]
    fn task_lookup_forgives_padding_quotes_case_and_spacing() {
        let item = "Map the harness as it stands";
        for given in [
            "  Map the harness as it stands \n",
            "\"Map the harness as it stands\"",
            "'Map the harness as it stands'",
            "“Map the harness as it stands”",
            "‘Map the harness as it stands’",
            "`Map the harness as it stands`",
            " \" Map the harness as it stands\" ",
            "map  the HARNESS\tas it stands",
            "\"map the harness  as it stands\"",
        ] {
            let mut list = Vec::new();
            apply(&mut list, args(Op::Init, &[item, "Other"], None)).unwrap();
            apply(&mut list, args(Op::Done, &[], Some(given)))
                .unwrap_or_else(|e| panic!("{given:?}: {e}"));
            assert_eq!(list[0].status, Status::Done, "{given:?}");
        }
        // One pair only, and only a matching pair.
        let mut list = Vec::new();
        apply(&mut list, args(Op::Init, &[item], None)).unwrap();
        for given in [
            "\"\"Map the harness as it stands\"\"",
            "\"Map the harness as it stands'",
        ] {
            assert!(
                apply(&mut list, args(Op::Done, &[], Some(given))).is_err(),
                "{given:?}"
            );
        }
        // An item whose text really is quoted matches exactly first.
        apply(&mut list, args(Op::Init, &["\"a\"", "b"], None)).unwrap();
        apply(&mut list, args(Op::Done, &[], Some("\"a\""))).unwrap();
        assert_eq!(list[0].status, Status::Done);
    }

    #[test]
    fn ambiguous_and_unknown_tasks_list_the_items() {
        // A list from before the normalised duplicate check can hold two items with one key.
        let mut list = vec![
            Todo::pending("Ship it".into()),
            Todo::pending("ship  it".into()),
        ];
        let err = apply(&mut list, args(Op::Done, &[], Some("SHIP IT"))).unwrap_err();
        assert_eq!(
            err,
            "ambiguous task: SHIP IT matches several items; pass one exactly:\n1. Ship it\n2. ship  it"
        );
        // Exact still wins over the ambiguity.
        apply(&mut list, args(Op::Done, &[], Some("ship  it"))).unwrap();
        assert_eq!(list[1].status, Status::Done);

        let mut list = Vec::new();
        assert_eq!(
            apply(&mut list, args(Op::Done, &[], Some("\"x\""))).unwrap_err(),
            "unknown task: \"x\". The list is empty."
        );
        apply(&mut list, args(Op::Init, &["a", "b"], None)).unwrap();
        assert_eq!(
            apply(&mut list, args(Op::Done, &[], Some("Map the harness"))).unwrap_err(),
            "unknown task: Map the harness. Items:\n1. a\n2. b"
        );
        let many: Vec<String> = (0..CAP).map(|i| format!("t{i}")).collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        apply(&mut list, args(Op::Init, &refs, None)).unwrap();
        let err = apply(&mut list, args(Op::Done, &[], Some("nope"))).unwrap_err();
        assert!(
            err.ends_with("\n32. t31") && err.matches('\n').count() == CAP,
            "{err}"
        );
    }

    #[test]
    fn duplicates_use_the_lookup_key() {
        let mut list = Vec::new();
        apply(&mut list, args(Op::Init, &["Ship it"], None)).unwrap();
        for dup in ["ship  IT", "\"Ship it\"", "‘ship it’"] {
            assert_eq!(
                apply(&mut list, args(Op::Add, &[dup], None)).unwrap_err(),
                format!("duplicate task: {dup}")
            );
        }
        assert!(apply(&mut list, args(Op::Init, &["x", " X "], None)).is_err());
        apply(&mut list, args(Op::Add, &["Ship it now"], None)).unwrap();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn invalid_mutations_leave_the_list_untouched() {
        let mut list = Vec::new();
        apply(&mut list, args(Op::Init, &["a", "b"], None)).unwrap();
        let before = list.clone();
        // Second item duplicates an existing one: the valid first item is not added either.
        assert!(apply(&mut list, args(Op::Add, &["c", "a"], None)).is_err());
        assert!(apply(&mut list, args(Op::Add, &["  "], None)).is_err());
        assert!(apply(&mut list, args(Op::Init, &["x", "x"], None)).is_err());
        assert!(apply(&mut list, block("b", None)).is_err());
        assert!(apply(&mut list, block("b", Some("  "))).is_err());
        assert!(apply(&mut list, args(Op::Unblock, &[], Some("b"))).is_err());
        assert_eq!(list, before);
        // View and same-state transitions are no-ops.
        apply(&mut list, args(Op::View, &[], None)).unwrap();
        apply(&mut list, args(Op::Start, &[], Some("a"))).unwrap();
        assert_eq!(list, before);
    }

    #[test]
    fn blocked_is_neither_promoted_nor_started() {
        let mut list = Vec::new();
        apply(&mut list, args(Op::Init, &["a", "b", "c"], None)).unwrap();
        apply(&mut list, block("a", Some("needs the API key"))).unwrap();
        assert_eq!(list[0].status, Status::Blocked);
        assert_eq!(list[0].reason.as_deref(), Some("needs the API key"));
        // Promotion skipped the blocked item.
        assert_eq!(list[1].status, Status::InProgress);
        assert!(apply(&mut list, args(Op::Start, &[], Some("a"))).is_err());
        assert_eq!(list[0].status, Status::Blocked);
        // Blocking the in-progress item promotes the next pending one.
        apply(&mut list, block("b", Some("waiting on user"))).unwrap();
        assert_eq!(list[2].status, Status::InProgress);
        // Unblock returns to pending, clears the reason, and does not steal in-progress.
        apply(&mut list, args(Op::Unblock, &[], Some("a"))).unwrap();
        assert_eq!(list[0].status, Status::Pending);
        assert_eq!(list[0].reason, None);
        assert_eq!(list[2].status, Status::InProgress);
        // Starting a pending item demotes the current one: one in progress.
        apply(&mut list, args(Op::Start, &[], Some("a"))).unwrap();
        assert_eq!(list[0].status, Status::InProgress);
        assert_eq!(list[2].status, Status::Pending);
        assert_eq!(
            list.iter()
                .filter(|t| t.status == Status::InProgress)
                .count(),
            1
        );
        assert!(render(&list).contains("waiting on user"));
    }

    #[test]
    fn terminal_items_are_not_reopened() {
        let mut list = Vec::new();
        apply(&mut list, args(Op::Init, &["a", "b"], None)).unwrap();
        apply(&mut list, args(Op::Done, &[], Some("a"))).unwrap();
        assert!(apply(&mut list, args(Op::Start, &[], Some("a"))).is_err());
        assert!(apply(&mut list, block("a", Some("r"))).is_err());
        assert!(apply(&mut list, args(Op::Drop, &[], Some("a"))).is_err());
        assert_eq!(list[0].status, Status::Done);
        apply(&mut list, args(Op::Done, &[], Some("a"))).unwrap();
        assert!(actionable(&list));
        apply(&mut list, args(Op::Drop, &[], Some("b"))).unwrap();
        assert!(!actionable(&list));
        apply(&mut list, args(Op::Init, &[], None)).unwrap();
        assert!(list.is_empty());
    }

    #[test]
    fn render_reminder_is_bounded_and_escaped() {
        let mut list = Vec::new();
        apply(&mut list, args(Op::Init, &["a", "b <x> & y", "c"], None)).unwrap();
        apply(&mut list, args(Op::Start, &[], Some("c"))).unwrap();
        let body = render_reminder(&list, 1).unwrap();
        // In-progress leads; markup in task text is data.
        let c = body.find("- c").unwrap();
        assert!(c < body.find("- a").unwrap());
        assert!(body.contains("b &lt;x&gt; &amp; y"));
        assert!(!body.contains("<x>"));
        assert!(body.contains("1 of 3"));
        assert!(render_reminder(&list, 0).is_none());
        assert!(render_reminder(&list, MAX_CONTINUE_REMINDERS + 1).is_none());
        apply(&mut list, args(Op::Init, &[], None)).unwrap();
        assert!(render_reminder(&list, 1).is_none());
        let many: Vec<String> = (0..10).map(|i| i.to_string()).collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        apply(&mut list, args(Op::Init, &refs, None)).unwrap();
        let body = render_reminder(&list, 3).unwrap();
        assert_eq!(body.matches("\n- ").count(), REMINDER_ITEMS + 1);
        assert!(body.contains("5 more"));
    }

    #[test]
    fn snapshot_round_trips() {
        let snap = TodoSnapshot {
            version: 7,
            source: TodoSource::Acp,
            items: vec![
                Todo::pending("a".into()),
                Todo {
                    text: "b".into(),
                    status: Status::Blocked,
                    reason: Some("why".into()),
                },
            ],
        };
        let json = serde_json::to_value(&snap).unwrap();
        assert_eq!(json["source"], "acp");
        assert_eq!(json["items"][0]["status"], "pending");
        assert!(json["items"][0]["reason"].is_null());
        assert_eq!(json["items"][1]["status"], "blocked");
        assert_eq!(serde_json::from_value::<TodoSnapshot>(json).unwrap(), snap);
        assert_eq!(
            TodoSnapshot::default(),
            TodoSnapshot {
                version: 0,
                source: TodoSource::Native,
                items: vec![],
            }
        );
        for s in [TodoSource::Native, TodoSource::Acp] {
            assert_eq!(s.as_str().parse::<TodoSource>().unwrap(), s);
        }
        for s in [
            Status::Pending,
            Status::InProgress,
            Status::Done,
            Status::Dropped,
            Status::Blocked,
        ] {
            assert_eq!(s.as_str().parse::<Status>().unwrap(), s);
            assert_eq!(serde_json::to_value(s).unwrap(), s.as_str());
        }
    }
}
