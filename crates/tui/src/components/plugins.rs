//! External plugins in the terminal (design 216; contract §7.3): the `/plugins` menu, one status
//! segment per visible status placement, and the plugins' `/commands` (text fallbacks only).
//! The TUI never approves: a plugin waiting for approval points at the app or `t plugin approve`.
use super::*;
use jsonrpsee::core::client::ClientT;
use jsonrpsee::rpc_params;
use ::plugins::{
    CommandReply, Dot, LogTail, PlacementKind, PluginInfo, PluginRunState, PluginUi, SurfaceContent, SurfaceFormat,
    SurfaceSnapshot,
};
use serde::Deserialize as _;

/// At most this many plugin segments in the status line (the desktop's cap).
const STATUS_ICONS: usize = 3;
/// `l` in the menu: lines of today's log.
const LOG_LINES: u32 = 40;

/// Plugin list and placement snapshots, refreshed on connect and on `plugins.ui`.
#[derive(Default)]
pub struct Plugins {
    list: Vec<PluginInfo>,
    surfaces: Vec<SurfaceSnapshot>,
    /// Open menu: the highlighted row.
    menu: Option<usize>,
    /// `/plugins` asked for the list; open the menu when it arrives.
    awaiting: bool,
}

/// Typed completions of this owner.
pub enum Msg {
    /// `plugin_list` + `plugin_surfaces`.
    Loaded(Result<(Vec<PluginInfo>, Vec<SurfaceSnapshot>), String>),
    /// A `plugins.ui` event.
    Ui(PluginUi),
    /// Lines to print (restart, enable, logs, content, a command's reply).
    Lines(Result<Vec<String>, String>),
}

impl Plugins {
    /// `(slash, args hint, description)` of every plugin command, in plugin order.
    pub fn commands(&self) -> Vec<(String, String, String)> {
        self.list
            .iter()
            .flat_map(|p| p.placements.iter().filter_map(move |pl| {
                let c = pl.command.as_ref()?;
                Some((c.slash.clone(), c.args.clone().unwrap_or_default(), format!("[plugin {}] {}", p.name, c.description)))
            }))
            .collect()
    }

    /// `(plugin, command name)` behind a typed `/plugin[:cmd]`.
    fn command(&self, slash: &str) -> Option<(String, String)> {
        self.list.iter().find_map(|p| {
            p.placements
                .iter()
                .filter_map(|pl| pl.command.as_ref())
                .find(|c| c.slash == slash)
                .map(|c| (p.name.clone(), c.name.clone()))
        })
    }

    /// Status-line segments: `(● label, dot)`, plugin order, hidden ones out, at most three.
    pub fn segments(&self) -> Vec<(String, Dot)> {
        let mut out = Vec::new();
        for p in &self.list {
            for pl in &p.placements {
                let shows = match pl.kind {
                    PlacementKind::Status => true,
                    PlacementKind::Sidebar => pl.status,
                    _ => false,
                };
                if !shows || pl.hidden {
                    continue;
                }
                let snap = self.surfaces.iter().find(|s| s.plugin == p.name && s.placement == pl.id);
                let dot = snap.map_or_else(|| state_dot(p.state), |s| s.dot);
                let label = snap
                    .and_then(|s| s.label.clone())
                    .or_else(|| (!pl.label.is_empty()).then(|| pl.label.clone()))
                    .unwrap_or_else(|| p.name.clone());
                let text = if dot == Dot::None { label } else { format!("● {label}") };
                out.push((text, dot));
            }
        }
        out.truncate(STATUS_ICONS);
        out
    }

    /// The dot of a menu row: the first status-bearing placement's, else the state's.
    fn row_dot(&self, p: &PluginInfo) -> Dot {
        p.placements
            .iter()
            .filter(|pl| pl.status)
            .find_map(|pl| self.surfaces.iter().find(|s| s.plugin == p.name && s.placement == pl.id))
            .map_or_else(|| state_dot(p.state), |s| s.dot)
    }

    fn rows(&self) -> Vec<String> {
        self.list
            .iter()
            .map(|p| {
                let glyph = match self.row_dot(p) {
                    Dot::Ok | Dot::Busy => "●",
                    Dot::Warn => "◐",
                    Dot::Error => "✕",
                    Dot::Off | Dot::None => "○",
                };
                let mut row = format!("{glyph} {:<20} {}", p.name, state_name(p.state));
                if p.needs_approval {
                    row.push_str(&format!("   approve in the theuth app or with `t plugin approve {}`", p.name));
                } else if let Some(e) = &p.error {
                    row.push_str(&format!("   {e}"));
                }
                row
            })
            .collect()
    }

    fn key(&mut self, code: KeyCode, cx: &mut Cx<'_, '_, Msg>) {
        let Some(cursor) = self.menu.as_mut() else { return };
        let len = self.list.len();
        *cursor = (*cursor).min(len.saturating_sub(1));
        let picked = self.list.get(*cursor).cloned();
        match code {
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Down => *cursor = (*cursor + 1).min(len.saturating_sub(1)),
            KeyCode::Esc => self.menu = None,
            KeyCode::Enter => {
                if let Some(p) = picked {
                    let session = cx.core.session;
                    cx.spawn(Lane::Concurrent, move |api| async move { Msg::Lines(content(&api, &p, session).await) });
                }
            }
            KeyCode::Char('r') => {
                if let Some(p) = picked {
                    cx.spawn(Lane::Concurrent, move |api| async move {
                        let r: Result<PluginInfo, _> = api.request("plugin_restart", rpc_params![p.name]).await;
                        Msg::Lines(r.map(|i| vec![info_line(&i)]).map_err(crate::reason))
                    });
                }
            }
            KeyCode::Char('d') => {
                if let Some(p) = picked {
                    let on = !p.enabled;
                    cx.spawn(Lane::Concurrent, move |api| async move {
                        let r: Result<PluginInfo, _> = api.request("plugin_set_enabled", rpc_params![p.name, on]).await;
                        Msg::Lines(r.map(|i| vec![info_line(&i)]).map_err(crate::reason))
                    });
                }
            }
            KeyCode::Char('l') => {
                if let Some(p) = picked {
                    cx.spawn(Lane::Concurrent, move |api| async move {
                        let r: Result<LogTail, _> = api.request("plugin_logs", rpc_params![p.name, LOG_LINES]).await;
                        Msg::Lines(r.map(|t| {
                            let mut lines = vec![format!("{} (last {LOG_LINES} lines):", t.path)];
                            lines.extend(t.lines);
                            lines
                        }).map_err(crate::reason))
                    });
                }
            }
            _ => {}
        }
    }
}

fn refresh(cx: &mut Cx<'_, '_, Msg>) {
    cx.spawn(Lane::Concurrent, |api| async move {
        let list: Result<Vec<PluginInfo>, _> = api.request("plugin_list", rpc_params![]).await;
        let surfaces: Result<Vec<SurfaceSnapshot>, _> = api.request("plugin_surfaces", rpc_params![]).await;
        Msg::Loaded(list.and_then(|l| surfaces.map(|s| (l, s))).map_err(crate::reason))
    });
}

/// `⏎` on a row: every content placement's text, inline (OpenUI points at the app).
async fn content(api: &rpc::client::Engine, p: &PluginInfo, session: Option<SessionId>) -> Result<Vec<String>, String> {
    let mut lines = vec![format!("{} {} — {}", p.name, p.version, state_name(p.state))];
    if !p.description.is_empty() {
        lines.push(p.description.clone());
    }
    for pl in p.placements.iter().filter(|pl| pl.format.is_some()) {
        let r: Result<SurfaceContent, _> = api.request("plugin_surface", rpc_params![p.name.clone(), pl.id.clone(), session]).await;
        let c = match r {
            Ok(c) => c,
            Err(e) => {
                lines.push(format!("{}: {}", pl.label, crate::reason(e)));
                continue;
            }
        };
        lines.push(format!("— {}", if pl.label.is_empty() { &pl.id } else { &pl.label }));
        lines.extend(surface_text(&c).lines().map(str::to_owned));
    }
    if let Some(e) = &p.error {
        lines.push(format!("error: {e}"));
    }
    if p.needs_approval {
        lines.push(format!("approve in the theuth app or with `t plugin approve {}`", p.name));
    }
    Ok(lines)
}

/// The terminal rendering of one surface: markdown and json-render as their text, OpenUI as a
/// pointer to the app plus its text fallback.
fn surface_text(c: &SurfaceContent) -> String {
    if let Some(e) = &c.error {
        return e.clone();
    }
    match c.format {
        Some(SurfaceFormat::Markdown) => c.source.clone().or_else(|| c.text.clone()).unwrap_or_default(),
        Some(SurfaceFormat::Openui) => match &c.text {
            Some(t) => format!("{t}\n(interactive on desktop/web)"),
            None => "interactive on desktop/web".into(),
        },
        Some(SurfaceFormat::JsonRender) | None => c.text.clone().or_else(|| c.source.clone()).unwrap_or_else(|| "loading…".into()),
    }
}

/// A command reply: `text`, else the card's text fallback.
fn reply_text(r: &CommandReply) -> String {
    r.text.clone().or_else(|| r.ui.as_ref().map(|u| u.text.clone())).unwrap_or_default()
}

fn state_name(s: PluginRunState) -> &'static str {
    match s {
        PluginRunState::Invalid => "invalid",
        PluginRunState::AwaitingApproval => "needs approval",
        PluginRunState::Starting => "starting",
        PluginRunState::Running => "running",
        PluginRunState::Backoff => "crashed (backoff)",
        PluginRunState::Failed => "failed",
        PluginRunState::Exited => "exited",
        PluginRunState::Stopping => "stopping",
        PluginRunState::Disabled => "disabled",
    }
}

/// The desktop's `STATE_DOT`.
fn state_dot(s: PluginRunState) -> Dot {
    match s {
        PluginRunState::Running => Dot::Ok,
        PluginRunState::Starting | PluginRunState::Stopping => Dot::Busy,
        PluginRunState::Backoff => Dot::Warn,
        PluginRunState::Failed | PluginRunState::Invalid => Dot::Error,
        PluginRunState::Exited | PluginRunState::Disabled | PluginRunState::AwaitingApproval => Dot::Off,
    }
}

fn dot_color(d: Dot) -> iocraft::Color {
    match d {
        Dot::Ok | Dot::Busy => iocraft::Color::Green,
        Dot::Warn => iocraft::Color::Yellow,
        Dot::Error => iocraft::Color::Red,
        Dot::Off | Dot::None => iocraft::Color::DarkGrey,
    }
}

fn info_line(i: &PluginInfo) -> String {
    format!("{} {}", i.name, state_name(i.state))
}

fn print(cx: &mut Cx<'_, '_, Msg>, lines: Vec<String>) {
    if cx.core.reasoning {
        cx.plain(Plain::Text("\x1b[0m\n".into()));
    }
    for l in &lines {
        cx.plain(Plain::Line(l.clone()));
    }
    cx.core(CoreAction::Notice(lines.join("\n")));
}

impl Component for Plugins {
    type Msg = Msg;
    fn declaration(&self) -> Declaration {
        Declaration {
            id: "plugins",
            interests: &[Interest { topic: ::plugins::UI_TOPIC, version: 1, variants: &[] }],
            slash: &[("/plugins", "", "external plugins: state, content, restart, enable/disable, logs (approve in the app)")],
            bindings: &[],
        }
    }
    fn decode(&self, event: &TopicEvent, _: &CoreView<'_>) -> Option<Msg> {
        if event.topic() != Some(::plugins::UI_TOPIC) || event.version() != Some(1) {
            return None;
        }
        PluginUi::deserialize(event.event()).ok().map(Msg::Ui)
    }
    fn update(&mut self, input: Input<'_, Msg>, cx: &mut Cx<'_, '_, Msg>) {
        match input {
            Input::Lifecycle(Lifecycle::Mount | Lifecycle::EngineChanged | Lifecycle::Resync) => refresh(cx),
            Input::Slash { name: "/plugins", .. } => {
                self.awaiting = true;
                refresh(cx);
            }
            Input::Slash { name, args } => {
                let Some((plugin, command)) = self.command(name) else { return };
                let (args, session) = (args.to_owned(), cx.core.session);
                cx.spawn(Lane::Concurrent, move |api| async move {
                    let r: Result<CommandReply, _> =
                        api.request("plugin_command", rpc_params![plugin, command, args, session]).await;
                    Msg::Lines(r.map(|r| reply_text(&r).lines().map(str::to_owned).collect()).map_err(crate::reason))
                });
            }
            Input::Key(key) if self.menu.is_some() => {
                self.key(key.code, cx);
                cx.consume_key();
                if self.menu.is_none() {
                    cx.close();
                }
            }
            Input::Message(Msg::Loaded(Ok((list, surfaces)))) => {
                self.list = list;
                self.surfaces = surfaces;
                if std::mem::take(&mut self.awaiting) {
                    if self.list.is_empty() {
                        print(cx, vec!["no plugins (`t plugin new <name>` scaffolds one)".into()]);
                    } else {
                        cx.plain(Plain::TtyRequired("menus need a TTY".into()));
                        self.menu = Some(0);
                        cx.focus(FocusClass::Modal);
                    }
                }
            }
            Input::Message(Msg::Ui(PluginUi::PluginStatus { plugin, placement, dot, badge, label, tooltip })) => {
                match self.surfaces.iter_mut().find(|s| s.plugin == plugin && s.placement == placement) {
                    Some(s) => {
                        s.dot = dot;
                        s.badge = badge;
                        s.label = label;
                        s.tooltip = tooltip;
                    }
                    None => self.surfaces.push(SurfaceSnapshot {
                        plugin, placement, dot, badge, label, tooltip, revision: 0, format: None,
                    }),
                }
            }
            Input::Message(Msg::Ui(PluginUi::PluginSurface { .. } | PluginUi::PluginInvalidate { .. })) => {}
            Input::Message(Msg::Ui(PluginUi::PluginState { .. } | PluginUi::PluginsChanged)) => refresh(cx),
            Input::Message(Msg::Lines(Ok(lines))) => {
                if !lines.is_empty() {
                    print(cx, lines);
                }
                refresh(cx);
            }
            Input::Message(Msg::Loaded(Err(error)) | Msg::Lines(Err(error))) => {
                self.awaiting = false;
                cx.error(error);
            }
            Input::Lifecycle(Lifecycle::FocusLost) => self.menu = None,
            _ => {}
        }
    }
    fn view(&self, at: Surface<'_>, cx: &ViewCx<'_>) -> Option<View> {
        match at {
            Surface::Status(StatusSlot::AfterCore) => {
                use iocraft::prelude::*;
                let segments = self.segments();
                if segments.is_empty() {
                    return None;
                }
                Some(super::View::IntrinsicElement {
                    element: element!(View(flex_direction: FlexDirection::Row) {
                        #(segments.into_iter().map(|(text, dot)| element!(Text(content: format!("  {text}"), color: dot_color(dot)))))
                    })
                    .into_any(),
                    rows: 1,
                    replace: false,
                })
            }
            Surface::Modal => {
                let cursor = self.menu?;
                let rows = self.rows();
                let title = "plugins  ⏎ show  r restart  d disable/enable  l log  esc close".to_owned();
                Some(View::IntrinsicElement {
                    rows: rows.len().min(10) + 3,
                    replace: false,
                    element: crate::ui::menu_view(title, rows, cursor, cx.width as u16, cx.height as u16, false),
                })
            }
            _ => None,
        }
    }
    fn slash_fallback(&self, name: &str) -> bool {
        self.command(name).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::plugins::{CommandInfo, Fallback, PlacementInfo};

    fn placement(id: &str, kind: PlacementKind, status: bool) -> PlacementInfo {
        PlacementInfo {
            id: id.into(), kind, label: id.to_uppercase(), icon: "gauge".into(), status, open: None, size: None,
            format: None, fallback: Fallback::Text, hidden: false,
            command: (kind == PlacementKind::Command).then(|| CommandInfo {
                name: id.into(), slash: format!("/turns:{id}"), description: "reset".into(), args: None,
            }),
        }
    }

    fn plugin(name: &str, state: PluginRunState, placements: Vec<PlacementInfo>) -> PluginInfo {
        PluginInfo {
            name: name.into(), version: "1".into(), description: String::new(), state, error: None,
            enabled: true, needs_approval: false, card: None, placements, tools: vec![], dir: String::new(),
            log: String::new(), approved_at: None, order: 0,
        }
    }

    #[test]
    fn segments_follow_status_placements_and_cap() {
        let mut p = Plugins::default();
        let mut hidden = placement("h", PlacementKind::Status, true);
        hidden.hidden = true;
        p.list = vec![
            plugin("turns", PluginRunState::Running, vec![
                placement("turns", PlacementKind::Sidebar, true),
                placement("tile", PlacementKind::Tile, false),
                hidden,
            ]),
            plugin("clock", PluginRunState::Backoff, vec![placement("clock", PlacementKind::Status, true)]),
            plugin("a", PluginRunState::Running, vec![placement("a", PlacementKind::Status, true)]),
            plugin("b", PluginRunState::Running, vec![placement("b", PlacementKind::Status, true)]),
        ];
        p.surfaces = vec![SurfaceSnapshot {
            plugin: "turns".into(), placement: "turns".into(), dot: Dot::Warn, badge: None,
            label: Some("51/50".into()), tooltip: None, revision: 0, format: None,
        }];
        assert_eq!(p.segments(), vec![
            ("● 51/50".to_owned(), Dot::Warn),
            ("● CLOCK".to_owned(), Dot::Warn),
            ("● A".to_owned(), Dot::Ok),
        ]);
    }

    #[test]
    fn commands_route_by_slash() {
        let mut p = Plugins::default();
        p.list = vec![plugin("turns", PluginRunState::Running, vec![placement("reset", PlacementKind::Command, false)])];
        assert_eq!(p.commands(), vec![("/turns:reset".to_owned(), String::new(), "[plugin turns] reset".to_owned())]);
        assert!(p.slash_fallback("/turns:reset"));
        assert!(!p.slash_fallback("/turns:other"));
        assert_eq!(p.command("/turns:reset"), Some(("turns".into(), "reset".into())));
    }

    #[test]
    fn approval_rows_point_at_the_app() {
        let mut p = Plugins::default();
        let mut waiting = plugin("turns", PluginRunState::AwaitingApproval, vec![]);
        waiting.needs_approval = true;
        p.list = vec![waiting];
        assert_eq!(p.rows(), vec!["○ turns                needs approval   approve in the theuth app or with `t plugin approve turns`"]);
    }

    #[test]
    fn openui_is_pointed_at_the_app() {
        let c = SurfaceContent {
            plugin: "turns".into(), placement: "turns".into(), revision: 1, state: ::plugins::SurfaceLoad::Ready,
            format: Some(SurfaceFormat::Openui), source: Some("root = Card()".into()), text: Some("50 turns".into()),
            bindings: vec![], error: None,
        };
        assert_eq!(surface_text(&c), "50 turns\n(interactive on desktop/web)");
        let md = SurfaceContent { format: Some(SurfaceFormat::Markdown), source: Some("**0**".into()), ..c };
        assert_eq!(surface_text(&md), "**0**");
    }

    #[test]
    fn tui_commands_are_reserved_plugin_names() {
        let registry = crate::components::builtin();
        let names: Vec<&str> = crate::model::COMMANDS
            .iter()
            .map(|c| c.0)
            .chain(registry.slash_specs().map(|s| s.0))
            .map(|n| n.trim_start_matches('/'))
            .collect();
        let missing: Vec<_> = names.iter().filter(|n| !::plugins::manifest::RESERVED_NAMES.contains(n)).collect();
        assert!(missing.is_empty(), "not in plugins::manifest::RESERVED_NAMES: {missing:?}");
    }
}
