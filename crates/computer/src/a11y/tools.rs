//! The six accessibility-tree tools (161 §4): one `Risk` each; the result is
//! agent-desktop's `data`, or its error envelope, as compact JSON.

use std::sync::Arc;

use tools::{Risk, Tool, ToolCx, ToolOutput};

use super::Request;
use super::args::{ActArgs, FindArgs, GetArgs, SnapshotArgs, WindowArgs, WindowsArgs};
use crate::tools::{Ctx, computer_tool, try_out};

computer_tool!(
    /// `computer_snapshot` → agent-desktop `snapshot`.
    SnapshotTool, "computer_snapshot", Risk::ReadOnly, SnapshotArgs,
    "Read an app's accessibility tree as JSON. Every element you can act on gets a ref `@<snapshot_id>:eN` for computer_act, computer_get and `root`. Headless: it does not move the pointer or take focus. `app` names the app (omitted = the focused window). For a dense app take `skeleton: true` first, then drill into one region with `root`. Refs belong to one snapshot: after the UI changes, snapshot again.",
    |ctx, a, cx| Ok(ToolOutput::ok(try_out!(ctx.a11y.run(cx.session, Request::Snapshot(a)).await)))
);

computer_tool!(
    /// `computer_find` → agent-desktop `find`.
    FindTool, "computer_find", Risk::ReadOnly, FindArgs,
    "Find elements in an app's accessibility tree by role, name, value, text, description or state, and return them with refs for computer_act. Cheaper than a full snapshot when you know what you want; narrow it with `root`, `app`, `window_id` or `surface`.",
    |ctx, a, cx| Ok(ToolOutput::ok(try_out!(ctx.a11y.run(cx.session, Request::Find(a)).await)))
);

computer_tool!(
    /// `computer_get` → agent-desktop `get`.
    GetTool, "computer_get", Risk::ReadOnly, GetArgs,
    "Read one property of an element by ref: `text` (default), `value`, `title`, `bounds`, `role` or `states`. Use it to verify what an action changed.",
    |ctx, a, cx| Ok(ToolOutput::ok(try_out!(ctx.a11y.run(cx.session, Request::Get(a)).await)))
);

computer_tool!(
    /// `computer_act` → one headless agent-desktop ref action.
    ActTool, "computer_act", Risk::Mutating, ActArgs,
    "Act on an element by its ref from the latest snapshot, headlessly through the accessibility API: no pointer movement, no focus stealing, no synthetic keys. Actions: click, double_click, right_click, focus, clear, set_value / type / select (with `value`), toggle, check, uncheck, expand, collapse, scroll (`direction`, `amount`), scroll_to. Where no headless path exists it fails without acting. On STALE_REF or AMBIGUOUS_TARGET take a new snapshot; retry only when the error's `retry` is `safe`.",
    |ctx, a, cx| Ok(ToolOutput::ok(try_out!(ctx.a11y.run(cx.session, Request::Act(a)).await)))
);

computer_tool!(
    /// `computer_windows` → agent-desktop `list-windows` / `list-apps`.
    WindowsTool, "computer_windows", Risk::ReadOnly, WindowsArgs,
    "List open windows (default) with the ids computer_snapshot and computer_window take, or running apps with `list: apps`. `app` keeps one application.",
    |ctx, a, cx| Ok(ToolOutput::ok(try_out!(ctx.a11y.run(cx.session, Request::Windows(a)).await)))
);

computer_tool!(
    /// `computer_window` → agent-desktop's window commands.
    WindowTool, "computer_window", Risk::Mutating, WindowArgs,
    "Focus, minimize, maximize, restore, move (`x`, `y`) or resize (`width`, `height`) a window by its id from computer_windows.",
    |ctx, a, cx| Ok(ToolOutput::ok(try_out!(ctx.a11y.run(cx.session, Request::Window(a)).await)))
);
