//! The a11y tools' arguments and their mapping to agent-desktop commands
//! (161 §4.1), after agent-desktop's own CLI mapping
//! (AD:`src/dispatch/{observation,interaction,app_window}.rs`). Pure: every
//! invalid combination is refused here as INVALID_ARGS before any call.

use agent_desktop_core::commands::helpers::{AppArgs, RefArgs};
use agent_desktop_core::commands::{
    find, focus_window, get, list_apps, list_windows, move_window, resize_window, scroll, select,
    set_value, snapshot, type_text,
};
use agent_desktop_core::{AppError, SnapshotSurface};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::command::Command;

/// A ref action's deadline for resolution, actionability and delivery: the
/// CLI's default (AD:`src/cli_args/mod.rs:290`).
const REF_TIMEOUT_MS: u64 = 5_000;
/// The longest `type` value: upstream budgets 5 s plus 30 ms per character
/// (`type_text::default_timeout_ms`), which 600 keeps under the 30 s worker
/// deadline. Longer text goes through `set_value`.
const MAX_TYPE_CHARS: usize = 600;

/// Arguments for `computer_snapshot`.
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct SnapshotArgs {
    /// Application name, e.g. `Finder` or `Safari`; omitted = the focused window.
    #[serde(default)]
    pub app: Option<String>,
    /// Window id from computer_windows; snapshots that window.
    #[serde(default)]
    pub window_id: Option<String>,
    /// Drill down: a ref from an earlier snapshot (`@<snapshot_id>:eN`) to snapshot beneath instead of the window root. Not combined with app, window_id, surface or skeleton.
    #[serde(default)]
    pub root: Option<String>,
    /// Only elements you can act on (default true); false also lists static text and containers.
    #[serde(default)]
    pub interactive_only: Option<bool>,
    /// Collapse unnamed single-child wrappers (default true).
    #[serde(default)]
    pub compact: Option<bool>,
    /// Maximum tree depth (default 10).
    #[serde(default)]
    pub max_depth: Option<u8>,
    /// Shallow map, depth 3 at most, with `children_count` on cut branches; drill into one with `root`. For dense apps.
    #[serde(default)]
    pub skeleton: bool,
    /// Snapshot an overlay instead of the window (default `window`): `focused`, `menu`, `menubar`, `sheet`, `popover`, `alert`, `toolbar`, `desktop`, `dock`, `spotlight`, `menu_bar_extras`, `notification_center`, `taskbar`, `system_tray`, `system_tray_overflow`, `start_menu`, `action_center` or `quick_settings`; which exist depends on the OS.
    #[serde(default)]
    pub surface: Option<String>,
}

/// Arguments for `computer_find`.
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct FindArgs {
    /// Accessibility role, e.g. `button`, `textfield`, `checkbox`, `menuitem`.
    #[serde(default)]
    pub role: Option<String>,
    /// Accessible name or label.
    #[serde(default)]
    pub name: Option<String>,
    /// Current value.
    #[serde(default)]
    pub value: Option<String>,
    /// Text anywhere in the name, value, title or description.
    #[serde(default)]
    pub text: Option<String>,
    /// Accessible description.
    #[serde(default)]
    pub description: Option<String>,
    /// Match name, description and value exactly (case-insensitive) instead of by substring.
    #[serde(default)]
    pub exact: bool,
    /// State tokens the element must have, e.g. `enabled`, `focused`, `checked=false`.
    #[serde(default)]
    pub state: Vec<String>,
    /// Application name; omitted = the focused window.
    #[serde(default)]
    pub app: Option<String>,
    /// Window id from computer_windows.
    #[serde(default)]
    pub window_id: Option<String>,
    /// Search only beneath this ref from an earlier snapshot.
    #[serde(default)]
    pub root: Option<String>,
    /// Search an overlay instead of the window; the values of computer_snapshot's `surface`.
    #[serde(default)]
    pub surface: Option<String>,
    /// At most this many matches (default 50; 0 = all).
    #[serde(default)]
    pub limit: Option<usize>,
}

/// What `computer_get` reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Property {
    /// What a person reads: the content of a text element, otherwise its name.
    #[default]
    Text,
    /// The current value.
    Value,
    /// The title.
    Title,
    /// Screen bounds.
    Bounds,
    /// The accessibility role.
    Role,
    /// State tokens (enabled, focused, checked, …).
    States,
}

/// Arguments for `computer_get`.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct GetArgs {
    /// A ref from the latest snapshot, `@<snapshot_id>:eN`.
    #[serde(rename = "ref")]
    pub ref_id: String,
    /// `text` (default), `value`, `title`, `bounds`, `role` or `states`.
    #[serde(default)]
    pub property: Option<Property>,
}

/// A headless ref action.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RefAction {
    /// Press the element (AXPress / Invoke).
    Click,
    /// Open the element as a double-click would.
    DoubleClick,
    /// Show the element's context menu.
    RightClick,
    /// Give the element keyboard focus within its window.
    Focus,
    /// Empty a text element.
    Clear,
    /// Replace the element's value with `value`.
    SetValue,
    /// Insert `value` as typed text.
    Type,
    /// Choose the option named `value` in a list, menu or pop-up.
    Select,
    /// Flip a checkbox or switch.
    Toggle,
    /// Make a checkbox checked.
    Check,
    /// Make a checkbox unchecked.
    Uncheck,
    /// Expand a disclosure, row or tree item.
    Expand,
    /// Collapse a disclosure, row or tree item.
    Collapse,
    /// Scroll the element by `amount` in `direction`.
    Scroll,
    /// Scroll the element into view.
    ScrollTo,
}

/// Scroll direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScrollDirection {
    /// Toward the top.
    Up,
    /// Toward the bottom.
    Down,
    /// Toward the start of the line.
    Left,
    /// Toward the end of the line.
    Right,
}

impl From<ScrollDirection> for agent_desktop_core::Direction {
    fn from(d: ScrollDirection) -> Self {
        match d {
            ScrollDirection::Up => Self::Up,
            ScrollDirection::Down => Self::Down,
            ScrollDirection::Left => Self::Left,
            ScrollDirection::Right => Self::Right,
        }
    }
}

/// Arguments for `computer_act`.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ActArgs {
    /// A ref from the latest snapshot, `@<snapshot_id>:eN`.
    #[serde(rename = "ref")]
    pub ref_id: String,
    /// What to do to the element.
    pub action: RefAction,
    /// The text for `set_value` and `type` (at most 600 characters), the option for `select`; not accepted otherwise.
    #[serde(default)]
    pub value: Option<String>,
    /// `scroll` only: `up`, `down` (default), `left` or `right`.
    #[serde(default)]
    pub direction: Option<ScrollDirection>,
    /// `scroll` only: scroll units (default 3).
    #[serde(default)]
    pub amount: Option<u32>,
}

/// What `computer_windows` lists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Listing {
    /// Open windows with the ids `window_id` takes (default).
    #[default]
    Windows,
    /// Running applications.
    Apps,
}

/// Arguments for `computer_windows`.
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct WindowsArgs {
    /// `windows` (default) or `apps`.
    #[serde(default)]
    pub list: Option<Listing>,
    /// Only this application.
    #[serde(default)]
    pub app: Option<String>,
}

/// A window operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WindowAction {
    /// Raise the window and give it focus.
    Focus,
    /// Minimize the window.
    Minimize,
    /// Maximize (zoom) the window.
    Maximize,
    /// Restore a minimized or maximized window.
    Restore,
    /// Move the window's top-left corner to (`x`, `y`).
    Move,
    /// Resize the window to `width` × `height`.
    Resize,
}

/// Arguments for `computer_window`.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct WindowArgs {
    /// Window id from computer_windows.
    pub window_id: String,
    /// What to do to the window.
    pub action: WindowAction,
    /// `move` only: left edge in global logical points.
    #[serde(default)]
    pub x: Option<f64>,
    /// `move` only: top edge in global logical points.
    #[serde(default)]
    pub y: Option<f64>,
    /// `resize` only: width in logical points.
    #[serde(default)]
    pub width: Option<f64>,
    /// `resize` only: height in logical points.
    #[serde(default)]
    pub height: Option<f64>,
}

/// One a11y request: a tool's arguments, mapped to an agent-desktop command
/// on the session's worker by [`super::A11y::run`].
#[derive(Debug)]
pub enum Request {
    /// `computer_snapshot`.
    Snapshot(SnapshotArgs),
    /// `computer_find`.
    Find(FindArgs),
    /// `computer_get`.
    Get(GetArgs),
    /// `computer_act`.
    Act(ActArgs),
    /// `computer_windows`.
    Windows(WindowsArgs),
    /// `computer_window`.
    Window(WindowArgs),
}

/// `request` → its agent-desktop command, or the refusal the model reads.
pub(super) fn command(request: Request) -> Result<Command, AppError> {
    match request {
        Request::Snapshot(a) => snapshot(a),
        Request::Find(a) => find(a),
        Request::Get(a) => Ok(get(a)),
        Request::Act(a) => act(a),
        Request::Windows(a) => Ok(windows(a)),
        Request::Window(a) => window(a),
    }
}

fn snapshot(a: SnapshotArgs) -> Result<Command, AppError> {
    if a.root.is_some()
        && (a.app.is_some() || a.window_id.is_some() || a.surface.is_some() || a.skeleton)
    {
        return Err(AppError::invalid_input(
            "root drills into an earlier snapshot's element, which already names its window: give root alone, without app, window_id, surface or skeleton",
        ));
    }
    Ok(Command::Snapshot(snapshot::SnapshotArgs {
        surface: surface(a.surface.as_deref())?,
        app: a.app,
        window_id: a.window_id,
        max_depth: a.max_depth.unwrap_or(10),
        include_bounds: false,
        interactive_only: a.interactive_only.unwrap_or(true),
        compact: a.compact.unwrap_or(true),
        skeleton: a.skeleton,
        root_ref: a.root,
        timeout_ms: None,
        force_electron_a11y: false,
    }))
}

fn find(a: FindArgs) -> Result<Command, AppError> {
    let selectors = [&a.role, &a.name, &a.value, &a.text, &a.description];
    if selectors.iter().all(|s| s.is_none()) && a.state.is_empty() {
        return Err(AppError::invalid_input(
            "give at least one of role, name, value, text, description or state",
        ));
    }
    let states = a
        .state
        .iter()
        .map(|raw| find::parse_state_flag(raw))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Command::Find(Box::new(find::FindArgs {
        surface: surface(a.surface.as_deref())?,
        app: a.app,
        window_id: a.window_id,
        root: a.root,
        filter: find::FindFilterArgs {
            role: a.role,
            name: a.name,
            description: a.description,
            native_id: None,
            value: a.value,
            text: a.text,
            exact: a.exact,
        },
        states,
        selection: find::FindSelectionArgs {
            count: false,
            first: false,
            last: false,
            nth: None,
            limit: a.limit,
        },
        timeout_ms: None,
    })))
}

fn get(a: GetArgs) -> Command {
    let property = match a.property.unwrap_or_default() {
        Property::Text => get::GetProperty::Text,
        Property::Value => get::GetProperty::Value,
        Property::Title => get::GetProperty::Title,
        Property::Bounds => get::GetProperty::Bounds,
        Property::Role => get::GetProperty::Role,
        Property::States => get::GetProperty::States,
    };
    Command::Get(get::GetArgs {
        ref_id: a.ref_id,
        property,
    })
}

fn act(a: ActArgs) -> Result<Command, AppError> {
    let takes_value = matches!(
        a.action,
        RefAction::SetValue | RefAction::Type | RefAction::Select
    );
    if takes_value != a.value.is_some() {
        return Err(AppError::invalid_input(if takes_value {
            "set_value, type and select need value"
        } else {
            "value belongs to set_value, type and select only"
        }));
    }
    if a.action != RefAction::Scroll && (a.direction.is_some() || a.amount.is_some()) {
        return Err(AppError::invalid_input(
            "direction and amount belong to scroll only",
        ));
    }
    let value = a.value.unwrap_or_default();
    if a.action == RefAction::Type && value.chars().count() > MAX_TYPE_CHARS {
        return Err(AppError::invalid_input_with_suggestion(
            format!("type takes at most {MAX_TYPE_CHARS} characters"),
            "use set_value for long text",
        ));
    }
    let ref_args = |ref_id| RefArgs {
        ref_id,
        timeout_ms: Some(REF_TIMEOUT_MS),
    };
    Ok(match a.action {
        RefAction::Click => Command::Click(ref_args(a.ref_id)),
        RefAction::DoubleClick => Command::DoubleClick(ref_args(a.ref_id)),
        RefAction::RightClick => Command::RightClick(ref_args(a.ref_id)),
        RefAction::Focus => Command::Focus(ref_args(a.ref_id)),
        RefAction::Clear => Command::Clear(ref_args(a.ref_id)),
        RefAction::Toggle => Command::Toggle(ref_args(a.ref_id)),
        RefAction::Check => Command::Check(ref_args(a.ref_id)),
        RefAction::Uncheck => Command::Uncheck(ref_args(a.ref_id)),
        RefAction::Expand => Command::Expand(ref_args(a.ref_id)),
        RefAction::Collapse => Command::Collapse(ref_args(a.ref_id)),
        RefAction::ScrollTo => Command::ScrollTo(ref_args(a.ref_id)),
        RefAction::SetValue => Command::SetValue(set_value::SetValueArgs {
            ref_id: a.ref_id,
            value,
            timeout_ms: Some(REF_TIMEOUT_MS),
        }),
        RefAction::Type => Command::Type(type_text::TypeArgs {
            ref_id: a.ref_id,
            timeout_ms: Some(type_text::default_timeout_ms(&value)),
            text: value,
        }),
        RefAction::Select => Command::Select(select::SelectArgs {
            ref_id: a.ref_id,
            value,
            timeout_ms: Some(REF_TIMEOUT_MS),
        }),
        RefAction::Scroll => Command::Scroll(scroll::ScrollArgs {
            ref_id: a.ref_id,
            direction: a.direction.unwrap_or(ScrollDirection::Down).into(),
            amount: a.amount.unwrap_or(3),
            timeout_ms: Some(REF_TIMEOUT_MS),
        }),
    })
}

fn windows(a: WindowsArgs) -> Command {
    match a.list.unwrap_or_default() {
        Listing::Windows => Command::ListWindows(list_windows::ListWindowsArgs {
            app: a.app,
            timeout_ms: None,
        }),
        Listing::Apps => Command::ListApps(list_apps::ListAppsArgs {
            app: a.app,
            timeout_ms: None,
        }),
    }
}

fn window(a: WindowArgs) -> Result<Command, AppError> {
    let target = || AppArgs {
        app: None,
        window_id: Some(a.window_id.clone()),
    };
    let point = (a.x, a.y);
    let size = (a.width, a.height);
    Ok(match (a.action, point, size) {
        (WindowAction::Focus, (None, None), (None, None)) => {
            Command::FocusWindow(focus_window::FocusWindowArgs {
                window_id: Some(a.window_id),
                app: None,
                title: None,
            })
        }
        (WindowAction::Minimize, (None, None), (None, None)) => Command::Minimize(target()),
        (WindowAction::Maximize, (None, None), (None, None)) => Command::Maximize(target()),
        (WindowAction::Restore, (None, None), (None, None)) => Command::Restore(target()),
        (WindowAction::Move, (Some(x), Some(y)), (None, None)) => {
            Command::MoveWindow(move_window::MoveWindowArgs {
                app: None,
                window_id: Some(a.window_id),
                x,
                y,
            })
        }
        (WindowAction::Resize, (None, None), (Some(width), Some(height))) => {
            Command::ResizeWindow(resize_window::ResizeWindowArgs {
                app: None,
                window_id: Some(a.window_id),
                width,
                height,
            })
        }
        _ => {
            return Err(AppError::invalid_input(
                "move takes x and y, resize takes width and height, and the other actions take neither",
            ));
        }
    })
}

/// A surface name → agent-desktop's own [`SnapshotSurface`], so the list
/// follows upstream (161 W-f); omitted = the window.
fn surface(name: Option<&str>) -> Result<SnapshotSurface, AppError> {
    let Some(name) = name else {
        return Ok(SnapshotSurface::Window);
    };
    serde_json::from_value(serde_json::Value::from(name)).map_err(|_| {
        AppError::invalid_input(format!(
            "unknown surface `{name}`; see the surface field for the list"
        ))
    })
}

#[cfg(test)]
mod tests {
    use agent_desktop_core::response::output::ErrorPayload;
    use agent_desktop_core::{RetryDisposition, StatePredicate};
    use serde_json::{Value, json};

    use super::*;

    /// The model's JSON → a command, through the same serde the tool uses.
    fn map<A: serde::de::DeserializeOwned>(
        wrap: fn(A) -> Request,
        args: Value,
    ) -> Result<Command, AppError> {
        command(wrap(serde_json::from_value(args).unwrap()))
    }

    /// Refused as INVALID_ARGS that is safe to retry.
    fn refused(r: Result<Command, AppError>) -> bool {
        matches!(r, Err(e) if e.code() == "INVALID_ARGS"
            && ErrorPayload::from_app_error(&e).disposition.retry() == RetryDisposition::Safe)
    }

    #[test]
    fn snapshot_defaults_to_an_interactive_compact_window_tree() {
        let Ok(Command::Snapshot(s)) = map(Request::Snapshot, json!({})) else {
            panic!()
        };
        assert!(s.app.is_none() && s.window_id.is_none() && s.root_ref.is_none());
        assert!(s.interactive_only && s.compact && !s.skeleton && !s.include_bounds);
        assert_eq!(s.max_depth, 10);
        assert_eq!(s.surface, SnapshotSurface::Window);
        assert_eq!(s.timeout_ms, None);
    }

    #[test]
    fn snapshot_maps_every_field() {
        let args = json!({"app": "Notes", "window_id": "w1", "interactive_only": false,
            "compact": false, "max_depth": 4, "skeleton": true, "surface": "menu_bar_extras"});
        let Ok(Command::Snapshot(s)) = map(Request::Snapshot, args) else {
            panic!()
        };
        assert_eq!(
            (s.app.as_deref(), s.window_id.as_deref()),
            (Some("Notes"), Some("w1"))
        );
        assert!(!s.interactive_only && !s.compact && s.skeleton);
        assert_eq!(s.max_depth, 4);
        assert_eq!(s.surface, SnapshotSurface::MenuBarExtras);
        let Ok(Command::Snapshot(s)) = map(Request::Snapshot, json!({"surface": "system-tray"}))
        else {
            panic!()
        };
        assert_eq!(
            s.surface,
            SnapshotSurface::SystemTray,
            "upstream's dash alias"
        );
        assert!(refused(map(
            Request::Snapshot,
            json!({"surface": "sidebar"})
        )));
    }

    #[test]
    fn snapshot_root_is_a_drill_down_alone() {
        let Ok(Command::Snapshot(s)) = map(Request::Snapshot, json!({"root": "@s1:e3"})) else {
            panic!()
        };
        assert_eq!(s.root_ref.as_deref(), Some("@s1:e3"));
        for extra in [
            json!({"app": "Notes"}),
            json!({"window_id": "w"}),
            json!({"surface": "menu"}),
            json!({"skeleton": true}),
        ] {
            let mut args = extra;
            args["root"] = json!("@s1:e3");
            assert!(refused(map(Request::Snapshot, args.clone())), "{args}");
        }
    }

    #[test]
    fn find_maps_selectors_states_and_limit() {
        let args = json!({"role": "button", "name": "OK", "value": "v", "text": "t",
            "description": "d", "exact": true, "state": ["enabled", "checked=false"],
            "app": "Notes", "root": "@s1:e2", "limit": 0});
        let Ok(Command::Find(f)) = map(Request::Find, args) else {
            panic!()
        };
        assert_eq!(f.filter.role.as_deref(), Some("button"));
        assert_eq!(f.filter.name.as_deref(), Some("OK"));
        assert_eq!(f.filter.value.as_deref(), Some("v"));
        assert_eq!(f.filter.text.as_deref(), Some("t"));
        assert_eq!(f.filter.description.as_deref(), Some("d"));
        assert!(f.filter.exact && f.filter.native_id.is_none());
        assert_eq!(
            f.states,
            vec![
                StatePredicate {
                    token: "enabled".into(),
                    expected: None
                },
                StatePredicate {
                    token: "checked".into(),
                    expected: Some(false)
                },
            ]
        );
        assert_eq!(
            (f.app.as_deref(), f.root.as_deref()),
            (Some("Notes"), Some("@s1:e2"))
        );
        assert_eq!(f.surface, SnapshotSurface::Window);
        let sel = &f.selection;
        assert!(!sel.count && !sel.first && !sel.last && sel.nth.is_none());
        assert_eq!(sel.limit, Some(0));
    }

    #[test]
    fn find_needs_a_selector_and_valid_states() {
        assert!(refused(map(Request::Find, json!({"app": "Notes"}))));
        assert!(matches!(
            map(Request::Find, json!({"state": ["focused"]})),
            Ok(Command::Find(_))
        ));
        assert!(refused(map(
            Request::Find,
            json!({"role": "button", "state": ["checked=maybe"]})
        )));
    }

    #[test]
    fn get_reads_text_by_default() {
        let Ok(Command::Get(g)) = map(Request::Get, json!({"ref": "@s1:e1"})) else {
            panic!()
        };
        assert_eq!(g.ref_id, "@s1:e1");
        assert!(matches!(g.property, get::GetProperty::Text));
        let Ok(Command::Get(g)) = map(Request::Get, json!({"ref": "@s1:e1", "property": "bounds"}))
        else {
            panic!()
        };
        assert!(matches!(g.property, get::GetProperty::Bounds));
    }

    #[test]
    fn act_maps_each_plain_action_to_its_command_with_the_ref_deadline() {
        let cases = [
            ("click", "click"),
            ("double_click", "double-click"),
            ("right_click", "right-click"),
            ("focus", "focus"),
            ("clear", "clear"),
            ("toggle", "toggle"),
            ("check", "check"),
            ("uncheck", "uncheck"),
            ("expand", "expand"),
            ("collapse", "collapse"),
            ("scroll_to", "scroll-to"),
        ];
        for (action, name) in cases {
            let cmd = map(Request::Act, json!({"ref": "@s1:e4", "action": action})).unwrap();
            assert_eq!(cmd.name(), name);
            assert!(cmd.mutating(), "{action}");
            let (Command::Click(r)
            | Command::DoubleClick(r)
            | Command::RightClick(r)
            | Command::Focus(r)
            | Command::Clear(r)
            | Command::Toggle(r)
            | Command::Check(r)
            | Command::Uncheck(r)
            | Command::Expand(r)
            | Command::Collapse(r)
            | Command::ScrollTo(r)) = cmd
            else {
                panic!("{action} → {name}");
            };
            assert_eq!(
                (r.ref_id.as_str(), r.timeout_ms),
                ("@s1:e4", Some(REF_TIMEOUT_MS))
            );
        }
    }

    #[test]
    fn act_value_actions_carry_value() {
        let act = |action: &str| {
            map(
                Request::Act,
                json!({"ref": "@s1:e4", "action": action, "value": "hi"}),
            )
        };
        let Ok(Command::SetValue(s)) = act("set_value") else {
            panic!()
        };
        assert_eq!(
            (s.ref_id.as_str(), s.value.as_str(), s.timeout_ms),
            ("@s1:e4", "hi", Some(REF_TIMEOUT_MS))
        );
        let Ok(Command::Type(t)) = act("type") else {
            panic!()
        };
        assert_eq!(
            (t.text.as_str(), t.timeout_ms),
            ("hi", Some(type_text::default_timeout_ms("hi")))
        );
        let Ok(Command::Select(s)) = act("select") else {
            panic!()
        };
        assert_eq!(
            (s.value.as_str(), s.timeout_ms),
            ("hi", Some(REF_TIMEOUT_MS))
        );
    }

    #[test]
    fn act_scroll_defaults_down_by_three() {
        let Ok(Command::Scroll(s)) =
            map(Request::Act, json!({"ref": "@s1:e4", "action": "scroll"}))
        else {
            panic!()
        };
        assert!(matches!(s.direction, agent_desktop_core::Direction::Down));
        assert_eq!((s.amount, s.timeout_ms), (3, Some(REF_TIMEOUT_MS)));
        let args = json!({"ref": "@s1:e4", "action": "scroll", "direction": "left", "amount": 7});
        let Ok(Command::Scroll(s)) = map(Request::Act, args) else {
            panic!()
        };
        assert!(matches!(s.direction, agent_desktop_core::Direction::Left));
        assert_eq!(s.amount, 7);
    }

    #[test]
    fn act_refuses_misplaced_arguments() {
        for args in [
            json!({"ref": "@s1:e4", "action": "set_value"}),
            json!({"ref": "@s1:e4", "action": "type"}),
            json!({"ref": "@s1:e4", "action": "select"}),
            json!({"ref": "@s1:e4", "action": "click", "value": "x"}),
            json!({"ref": "@s1:e4", "action": "click", "direction": "up"}),
            json!({"ref": "@s1:e4", "action": "toggle", "amount": 2}),
        ] {
            assert!(refused(map(Request::Act, args.clone())), "{args}");
        }
    }

    #[test]
    fn type_refuses_text_past_the_deadline_budget() {
        let typing = |n: usize| {
            map(
                Request::Act,
                json!({"ref": "@s1:e4", "action": "type", "value": "é".repeat(n)}),
            )
        };
        assert!(matches!(typing(MAX_TYPE_CHARS), Ok(Command::Type(_))));
        let Err(e) = typing(MAX_TYPE_CHARS + 1) else {
            panic!("601 characters were accepted")
        };
        assert_eq!(e.code(), "INVALID_ARGS");
        let retry = ErrorPayload::from_app_error(&e).disposition.retry();
        assert_eq!(retry, RetryDisposition::Safe);
        assert_eq!(e.suggestion(), Some("use set_value for long text"));
        let set = map(
            Request::Act,
            json!({"ref": "@s1:e4", "action": "set_value", "value": "x".repeat(5_000)}),
        );
        assert!(
            matches!(set, Ok(Command::SetValue(_))),
            "set_value takes long text"
        );
    }

    #[test]
    fn windows_lists_windows_by_default_or_apps() {
        let Ok(Command::ListWindows(w)) = map(Request::Windows, json!({"app": "Notes"})) else {
            panic!()
        };
        assert_eq!(w.app.as_deref(), Some("Notes"));
        let Ok(Command::ListApps(a)) = map(Request::Windows, json!({"list": "apps"})) else {
            panic!()
        };
        assert!(a.app.is_none());
    }

    #[test]
    fn window_maps_each_action_by_id() {
        let w = |args: Value| map(Request::Window, args);
        let Ok(Command::FocusWindow(f)) = w(json!({"window_id": "w1", "action": "focus"})) else {
            panic!()
        };
        assert_eq!(
            (f.window_id.as_deref(), f.app.as_deref(), f.title.as_deref()),
            (Some("w1"), None, None)
        );
        for (action, name) in [
            ("minimize", "minimize"),
            ("maximize", "maximize"),
            ("restore", "restore"),
        ] {
            let cmd = w(json!({"window_id": "w1", "action": action})).unwrap();
            assert_eq!(cmd.name(), name);
            let (Command::Minimize(t) | Command::Maximize(t) | Command::Restore(t)) = cmd else {
                panic!()
            };
            assert_eq!(
                (t.window_id.as_deref(), t.app.as_deref()),
                (Some("w1"), None)
            );
        }
        let Ok(Command::MoveWindow(m)) =
            w(json!({"window_id": "w1", "action": "move", "x": 10.0, "y": -5.0}))
        else {
            panic!()
        };
        assert_eq!((m.window_id.as_deref(), m.x, m.y), (Some("w1"), 10.0, -5.0));
        let Ok(Command::ResizeWindow(r)) =
            w(json!({"window_id": "w1", "action": "resize", "width": 800.0, "height": 600.0}))
        else {
            panic!()
        };
        assert_eq!(
            (r.window_id.as_deref(), r.width, r.height),
            (Some("w1"), 800.0, 600.0)
        );
    }

    #[test]
    fn window_refuses_missing_or_misplaced_geometry() {
        for args in [
            json!({"window_id": "w1", "action": "move", "x": 1.0}),
            json!({"window_id": "w1", "action": "resize", "width": 1.0}),
            json!({"window_id": "w1", "action": "move", "x": 1.0, "y": 1.0, "width": 1.0}),
            json!({"window_id": "w1", "action": "minimize", "x": 1.0, "y": 1.0}),
            json!({"window_id": "w1", "action": "focus", "height": 1.0}),
        ] {
            assert!(refused(map(Request::Window, args.clone())), "{args}");
        }
    }

    #[test]
    fn only_observations_are_read_only() {
        let read_only = [
            map(Request::Snapshot, json!({})),
            map(Request::Find, json!({"role": "button"})),
            map(Request::Get, json!({"ref": "@s1:e1"})),
            map(Request::Windows, json!({})),
            map(Request::Windows, json!({"list": "apps"})),
        ];
        for cmd in read_only {
            assert!(!cmd.unwrap().mutating());
        }
        assert!(
            map(
                Request::Window,
                json!({"window_id": "w", "action": "focus"})
            )
            .unwrap()
            .mutating()
        );
    }
}
