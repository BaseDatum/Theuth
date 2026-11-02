//! One agent-desktop command ready to run on a session worker (161 §3.4),
//! and its dispatch the way agent-desktop's CLI does it: a command scope
//! around `execute` (AD:`src/dispatch/mod.rs:43-61`, `:139`).

use agent_desktop_core::commands::helpers::{AppArgs, RefArgs};
use agent_desktop_core::commands::{
    check, clear, click, collapse, double_click, expand, find, focus, focus_window, get, list_apps,
    list_windows, maximize, minimize, move_window, resize_window, restore, right_click, scroll,
    scroll_to, select, set_value, snapshot, toggle, type_text, uncheck,
};
use agent_desktop_core::context::CommandContext;
use agent_desktop_core::{AppError, PlatformAdapter};
use serde_json::Value;

/// The agent-desktop commands the 161 tools reach (§4). Nothing here spawns
/// a helper: clipboard, overlay, launch, notification and screenshot
/// commands are deliberately absent (§2).
pub(super) enum Command {
    Snapshot(snapshot::SnapshotArgs),
    /// Boxed: by far the largest arguments.
    Find(Box<find::FindArgs>),
    Get(get::GetArgs),
    Click(RefArgs),
    DoubleClick(RefArgs),
    RightClick(RefArgs),
    Focus(RefArgs),
    Clear(RefArgs),
    SetValue(set_value::SetValueArgs),
    Type(type_text::TypeArgs),
    Select(select::SelectArgs),
    Toggle(RefArgs),
    Check(RefArgs),
    Uncheck(RefArgs),
    Expand(RefArgs),
    Collapse(RefArgs),
    Scroll(scroll::ScrollArgs),
    ScrollTo(RefArgs),
    ListWindows(list_windows::ListWindowsArgs),
    ListApps(list_apps::ListAppsArgs),
    FocusWindow(focus_window::FocusWindowArgs),
    Minimize(AppArgs),
    Maximize(AppArgs),
    Restore(AppArgs),
    MoveWindow(move_window::MoveWindowArgs),
    ResizeWindow(resize_window::ResizeWindowArgs),
}

impl Command {
    /// agent-desktop's name for the command (its trace label).
    pub(super) fn name(&self) -> &'static str {
        match self {
            Self::Snapshot(_) => "snapshot",
            Self::Find(_) => "find",
            Self::Get(_) => "get",
            Self::Click(_) => "click",
            Self::DoubleClick(_) => "double-click",
            Self::RightClick(_) => "right-click",
            Self::Focus(_) => "focus",
            Self::Clear(_) => "clear",
            Self::SetValue(_) => "set-value",
            Self::Type(_) => "type",
            Self::Select(_) => "select",
            Self::Toggle(_) => "toggle",
            Self::Check(_) => "check",
            Self::Uncheck(_) => "uncheck",
            Self::Expand(_) => "expand",
            Self::Collapse(_) => "collapse",
            Self::Scroll(_) => "scroll",
            Self::ScrollTo(_) => "scroll-to",
            Self::ListWindows(_) => "list-windows",
            Self::ListApps(_) => "list-apps",
            Self::FocusWindow(_) => "focus-window",
            Self::Minimize(_) => "minimize",
            Self::Maximize(_) => "maximize",
            Self::Restore(_) => "restore",
            Self::MoveWindow(_) => "move-window",
            Self::ResizeWindow(_) => "resize-window",
        }
    }

    /// Whether the command changes the desktop; a read-only command's error
    /// never carries an unknown delivery (§3.4).
    pub(super) fn mutating(&self) -> bool {
        !matches!(
            self,
            Self::Snapshot(_)
                | Self::Find(_)
                | Self::Get(_)
                | Self::ListWindows(_)
                | Self::ListApps(_)
        )
    }

    /// Run inside agent-desktop's command scope.
    pub(super) fn dispatch(
        self,
        adapter: &dyn PlatformAdapter,
        cx: &CommandContext,
    ) -> Result<Value, AppError> {
        let scope = if self.mutating() {
            cx.mutating_command_scope(self.name())?
        } else {
            cx.command_scope(self.name())?
        };
        let result = self.execute(adapter, cx);
        scope.complete(&result)?;
        result
    }

    fn execute(self, a: &dyn PlatformAdapter, cx: &CommandContext) -> Result<Value, AppError> {
        match self {
            Self::Snapshot(args) => snapshot::execute(args, a, cx),
            Self::Find(args) => find::execute(*args, a, cx),
            Self::Get(args) => get::execute(args, a, cx),
            Self::Click(args) => click::execute(args, a, cx),
            Self::DoubleClick(args) => double_click::execute(args, a, cx),
            Self::RightClick(args) => right_click::execute(args, a, cx),
            Self::Focus(args) => focus::execute(args, a, cx),
            Self::Clear(args) => clear::execute(args, a, cx),
            Self::SetValue(args) => set_value::execute(args, a, cx),
            Self::Type(args) => type_text::execute(args, a, cx),
            Self::Select(args) => select::execute(args, a, cx),
            Self::Toggle(args) => toggle::execute(args, a, cx),
            Self::Check(args) => check::execute(args, a, cx),
            Self::Uncheck(args) => uncheck::execute(args, a, cx),
            Self::Expand(args) => expand::execute(args, a, cx),
            Self::Collapse(args) => collapse::execute(args, a, cx),
            Self::Scroll(args) => scroll::execute(args, a, cx),
            Self::ScrollTo(args) => scroll_to::execute(args, a, cx),
            Self::ListWindows(args) => list_windows::execute(args, a),
            Self::ListApps(args) => list_apps::execute(args, a),
            Self::FocusWindow(args) => focus_window::execute(args, a),
            Self::Minimize(args) => minimize::execute(args, a),
            Self::Maximize(args) => maximize::execute(args, a),
            Self::Restore(args) => restore::execute(args, a),
            Self::MoveWindow(args) => move_window::execute(args, a),
            Self::ResizeWindow(args) => resize_window::execute(args, a),
        }
    }
}
