//! Updater cache survives reset; no RPC or focus, and no non-TTY update notice.
use serde::Deserialize;
use super::*;

/// Persistent updater fact and deduplication state.
#[derive(Default)]
pub struct Update { ready: Option<::update::UpdateReady> }
/// Owner-local event, absent from the central UI enum.
pub enum Msg {
    /// A staged or installed update; repeated identical facts are silent.
    Ready(::update::UpdateReady),
}

impl Component for Update {
    type Msg = Msg;
    fn declaration(&self) -> Declaration {
        Declaration {
            id: "update",
            interests: &[Interest { topic: "update.ready", version: 1, variants: &["UpdateReady"] }],
            slash: &[], bindings: &[],
        }
    }
    fn decode(&self, event: &TopicEvent, _: &CoreView<'_>) -> Option<Msg> {
        if !((event.topic() == Some("update.ready") && event.version() == Some(1)) || event.topic().is_none())
            || event.event()["type"] != "UpdateReady"
        { return None; }
        ::update::UpdateReady::deserialize(event.event()).ok().map(Msg::Ready)
    }
    fn update(&mut self, input: Input<'_, Msg>, cx: &mut Cx<'_, '_, Msg>) {
        if let Input::Message(Msg::Ready(event)) = input {
            // Preserve print_event's reasoning fence, but never print the notice.
            if cx.core.reasoning { cx.plain(Plain::Text("\x1b[0m\n".into())); }
            // Once per fact: applied=false -> true for one version still announces.
            if self.ready.as_ref() != Some(&event) {
                let text = if event.applied {
                    format!(
                        "theuth {} is installed; it runs the next time you start theuth.",
                        event.version
                    )
                } else {
                    format!(
                        "theuth {} is available; `t --update` installs it (the binary's directory is not writable).",
                        event.version
                    )
                };
                self.ready = Some(event);
                cx.interject(text);
            }
        }
    }
    fn view(&self, at: Surface<'_>, _: &ViewCx<'_>) -> Option<View> {
        if !matches!(at, Surface::Header(super::HeaderSlot::Update)) { return None; }
        self.ready.as_ref().map(|ready| View::Text(if ready.applied {
            format!("  ·  {} ready", ready.version)
        } else {
            format!("  ·  {} available", ready.version)
        }))
    }
}
