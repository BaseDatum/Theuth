//! Computer permissions and display inventory; no live screen-control surface.
use super::*;
use rpc_compose::api::clients::ScreensApiClient;

/// Stateless request/reply panel.
#[derive(Default)]
pub struct Computer;
/// Request carried by the effect, not a central feature command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Read permission status and attached displays.
    Status,
}
/// Owner-local completion; errors retain the shared command-failure behavior.
pub enum Msg {
    /// Permission and display inventory from the current engine.
    Status(Result<::computer::PermissionStatus, String>),
}
impl Component for Computer {
    type Msg = Msg;
    fn declaration(&self) -> Declaration {
        Declaration {
            id: "computer", interests: &[], bindings: &[],
            slash: &[("/computer", "", "computer control: permissions and displays")],
        }
    }
    fn decode(&self, _: &TopicEvent, _: &CoreView<'_>) -> Option<Msg> { None }
    fn update(&mut self, input: Input<'_, Msg>, cx: &mut Cx<'_, '_, Msg>) {
        match input {
            Input::Slash { .. } => cx.spawn_request(Lane::Concurrent, Request::Status, |api, request| async move {
                match request {
                    Request::Status => Msg::Status(api.computer_permissions().await.map_err(crate::reason)),
                }
            }),
            Input::Message(Msg::Status(Ok(status))) => {
                if cx.core.reasoning { cx.plain(Plain::Text("\x1b[0m\n".into())); }
                let text = computer_line(&status);
                cx.plain(Plain::Line(text.clone()));
                cx.core(CoreAction::Notice(text));
            }
            Input::Message(Msg::Status(Err(error))) => cx.error(error),
            _ => {}
        }
    }
    fn view(&self, _: Surface<'_>, _: &ViewCx<'_>) -> Option<View> { None }
}

/// The `/computer` reply: permission ticks and the attached displays on one
/// line, then the status note on a second line when there is one.
pub fn computer_line(s: &::computer::PermissionStatus) -> String {
    let tick = |ok: bool| if ok { "✓" } else { "✗" };
    let displays: Vec<String> = s
        .displays
        .iter()
        .map(|d| {
            format!(
                "{}{} {}×{} @{}×",
                d.index,
                if d.main { " main" } else { "" },
                d.width,
                d.height,
                d.scale
            )
        })
        .collect();
    let mut line = format!(
        "accessibility {} · screen recording {} · {} displays: {}",
        tick(s.accessibility),
        tick(s.screen_recording),
        s.displays.len(),
        displays.join(", ")
    );
    if !s.note.is_empty() {
        line.push('\n');
        line.push_str(&s.note);
    }
    line
}
