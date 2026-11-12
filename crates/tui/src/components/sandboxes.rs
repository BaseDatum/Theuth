//! Sandbox inventory and desktop URL; passwords never cross the presentation ports.
use super::*;
use rpc::api::clients::SandboxesApiClient;

/// Stateless RPC-only surface, not an embedded VM desktop.
#[derive(Default)]
pub struct Sandboxes;
/// Requests remain owner-local and drive their corresponding effects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Inventory on the current engine.
    List,
    /// Desktop capability for one VM.
    Desktop(String),
}
/// Typed completions deliberately have no Debug implementation.
pub enum Msg {
    /// Inventory result.
    List(Result<Vec<sandbox::SandboxInfo>, String>),
    /// Desktop result; the password is never formatted.
    Desktop(String, Result<sandbox::types::DesktopAccess, String>),
}
impl Component for Sandboxes {
    type Msg = Msg;
    fn declaration(&self) -> Declaration {
        Declaration {
            id: "sandboxes", interests: &[], bindings: &[],
            slash: &[("/sandbox", "[desktop <name>]", "sandbox VMs on this engine (design doc 79); `desktop` prints the noVNC URL")],
        }
    }
    fn decode(&self, _: &TopicEvent, _: &CoreView<'_>) -> Option<Msg> { None }
    fn update(&mut self, input: Input<'_, Msg>, cx: &mut Cx<'_, '_, Msg>) {
        match input {
            Input::Slash { args, .. } => {
                let (sub, rest) = args
                    .split_once(char::is_whitespace)
                    .map_or((args, ""), |(s, r)| (s, r.trim()));
                let request = match (sub, rest) {
                    ("" | "ls" | "list", _) => Request::List,
                    ("desktop", name) if !name.is_empty() => Request::Desktop(name.to_owned()),
                    _ => {
                        cx.core(CoreAction::Reject("usage: /sandbox [desktop <name>]".into()));
                        return;
                    }
                };
                cx.spawn_request(Lane::Concurrent, request, |api, request| async move {
                    match request {
                        Request::List => Msg::List(api.sandboxes_list().await.map_err(crate::reason)),
                        Request::Desktop(name) => Msg::Desktop(name.clone(), api.sandbox_desktop_access(name).await.map_err(crate::reason)),
                    }
                });
            }
            Input::Message(Msg::List(Ok(list))) => {
                if cx.core.reasoning { cx.plain(Plain::Text("\x1b[0m\n".into())); }
                if list.is_empty() { line(cx, "sandboxes: none".into()); }
                for item in list { line(cx, sandbox_line(&item)); }
            }
            Input::Message(Msg::Desktop(name, Ok(desktop))) => {
                if cx.core.reasoning { cx.plain(Plain::Text("\x1b[0m\n".into())); }
                cx.plain(Plain::Line(format!("{name} desktop: {}", desktop.http_url)));
                cx.core(CoreAction::Notice(format!(
                    "{name} desktop: {} (password in the vault as sandbox/{name}/vnc; `t sandbox desktop {name}` prints it)",
                    desktop.http_url
                )));
            }
            Input::Message(Msg::List(Err(error)) | Msg::Desktop(_, Err(error))) => cx.error(error),
            _ => {}
        }
    }
    fn view(&self, _: Surface<'_>, _: &ViewCx<'_>) -> Option<View> { None }
}
fn line(cx: &mut Cx<'_, '_, Msg>, text: String) {
    cx.plain(Plain::Line(text.clone()));
    cx.core(CoreAction::Notice(text));
}

/// One `/sandbox` line: name, backend, state, engine, size.
pub fn sandbox_line(i: &sandbox::SandboxInfo) -> String {
    let state = serde_json::to_value(&i.state)
        .ok()
        .and_then(|v| v.get("state").and_then(|s| s.as_str()).map(str::to_owned))
        .unwrap_or_default();
    let mut s = format!(
        "{} {} {} engine={}",
        i.name,
        i.backend,
        state,
        i.engine.as_deref().unwrap_or("-")
    );
    if let Some(vm) = &i.vm {
        let r = vm.resources;
        s.push_str(&format!(
            " cpus={} memory={}M disk={}M shares={} desktop={}",
            r.cpus,
            r.memory_mib,
            r.root_disk_mib,
            vm.shares.len(),
            if vm.desktop.is_some() { "yes" } else { "no" }
        ));
    }
    s
}

#[cfg(test)]
#[test]
fn desktop_password_never_reaches_output_transcript_or_debug() {
    use crate::model::{Entry, Model, UiEvent};
    let mut model = Model::default();
    let reply = LocalReply {
        to: model.components.token("sandboxes"), engine_epoch: 0,
        body: Box::new(Msg::Desktop("dev".into(), Ok(sandbox::types::DesktopAccess {
            ws_url: "ws://localhost/websockify".into(),
            http_url: "http://localhost/vnc.html".into(),
            password: "never-render-this-vnc-password".into(),
            geometry: "1280x720".into(), host_engine: "engine".into(),
        }))),
    };
    assert!(!format!("{reply:?}").contains("never-render-this-vnc-password"));
    let mut output = Vec::new();
    model.receive(UiEvent::Component(reply), &mut |event| {
        crate::print_output(&mut output, event);
    });
    assert_eq!(output, b"dev desktop: http://localhost/vnc.html\n");
    assert!(matches!(model.entries.last(), Some(Entry::Info(text)) if text ==
        "dev desktop: http://localhost/vnc.html (password in the vault as sandbox/dev/vnc; `t sandbox desktop dev` prints it)"));
    assert!(!format!("{:?}", model.entries).contains("never-render-this-vnc-password"));
}
