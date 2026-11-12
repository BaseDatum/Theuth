//! Mesh device inventory and the node/mesh shared pairing countdown.
use super::*;
use rpc_compose::api::clients::NetworkApiClient;
use crate::model::{ago, now_ms};
use super::link::Countdown;

/// Stateless inventory commands plus the current pairing card.
#[derive(Default)]
pub struct MeshDevices {
    pub(crate) invite: Option<Countdown>,
}
/// Owner request carried into the RPC task.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Read replicated device inventory.
    MeshDevicesList,
    /// Start pairing with an optional name.
    MeshDevicesAdd(Option<String>),
    /// Revoke a device, then read inventory.
    MeshDevicesRevoke(String),
    /// Rename a device, then read inventory.
    MeshDevicesRename(String, String),
}
/// Owner-local replies; the inventory has no live menu subscription.
pub enum Msg {
    /// Mutation confirmation precedes the authoritative list or its error.
    Listed {
        /// A mutation succeeded even if its subsequent read failed.
        notice: Option<String>,
        /// The replicated registry read.
        result: Result<Vec<::link::DeviceInfo>, String>,
    },
    /// New mesh pairing capability.
    Pairing(Result<::link::PairingPayload, String>),
}
impl MeshDevices {
    fn request(&self, request: Request, cx: &mut Cx<'_, '_, Msg>) {
        cx.spawn_request(Lane::Concurrent, request, |api, request| async move {
            if let Request::MeshDevicesAdd(name) = request {
                return Msg::Pairing(api.device_pair_start(name).await.map_err(crate::reason));
            }
            let mutation = async {
                Ok(match request {
                    Request::MeshDevicesList => None,
                    Request::MeshDevicesRevoke(id) => {
                        api.device_revoke(id.clone()).await.map_err(crate::reason)?;
                        Some(format!("revoked device {id}"))
                    }
                    Request::MeshDevicesRename(id, name) => {
                        api.device_rename(id.clone(), name.clone()).await.map_err(crate::reason)?;
                        Some(format!("renamed device {id} → {name}"))
                    }
                    Request::MeshDevicesAdd(_) => unreachable!(),
                })
            }.await;
            match mutation {
                Ok(notice) => Msg::Listed { notice, result: api.devices_list().await.map_err(crate::reason) },
                Err(error) => Msg::Listed { notice: None, result: Err(error) },
            }
        });
    }
    fn slash(&mut self, args: &str, cx: &mut Cx<'_, '_, Msg>) {
        const USAGE: &str = "usage: /mesh devices [list|add [name]|rm <id>|name <id> <name>]";
        let (sub, rest) = args.split_once(char::is_whitespace).map_or((args, ""), |(s, r)| (s, r.trim()));
        let request = if sub == "devices" {
            let (verb, rest) = rest.split_once(char::is_whitespace).map_or((rest, ""), |(s, r)| (s, r.trim()));
            match (verb, rest) {
                ("" | "list", _) => Some(Request::MeshDevicesList),
                ("add", name) => Some(Request::MeshDevicesAdd((!name.is_empty()).then(|| name.to_owned()))),
                ("rm", id) if !id.is_empty() => Some(Request::MeshDevicesRevoke(id.to_owned())),
                ("name", rest) if rest.contains(char::is_whitespace) => {
                    let (id, name) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
                    Some(Request::MeshDevicesRename(id.to_owned(), name.trim().to_owned()))
                }
                _ => None,
            }
        } else { None };
        match request { Some(request) => self.request(request, cx), None => cx.core(CoreAction::Reject(USAGE.into())) }
    }
}
impl Component for MeshDevices {
    type Msg = Msg;
    fn declaration(&self) -> Declaration {
        Declaration { id: "mesh-devices", interests: &[], bindings: &[], slash: &[("/mesh",
            "devices [list|add [name]|rm <id>|name <id> <name>]", "devices paired with the mesh (any node serves them)")] }
    }
    fn decode(&self, _: &TopicEvent, _: &CoreView<'_>) -> Option<Msg> { None }
    fn update(&mut self, input: Input<'_, Msg>, cx: &mut Cx<'_, '_, Msg>) {
        match input {
            Input::Slash { args, .. } => self.slash(args, cx),
            Input::Message(Msg::Listed { notice, result }) => {
                if cx.core.reasoning { cx.plain(Plain::Text("\x1b[0m\n".into())); }
                if let Some(notice) = notice { cx.plain(Plain::Line(notice.clone())); cx.core(CoreAction::Notice(notice)); }
                match result {
                    Err(error) => cx.error(error),
                    Ok(list) => {
                        if list.is_empty() { cx.plain(Plain::Line("mesh devices: none paired".into())); cx.core(CoreAction::Notice("mesh devices: none paired".into())); }
                        let now = now_ms();
                        for device in list {
                            let line = mesh_device_line(&device, now);
                            cx.plain(Plain::Line(line.clone())); cx.core(CoreAction::Notice(line));
                        }
                    }
                }
            }
            Input::Message(Msg::Pairing(result)) => match result {
                Err(error) => cx.error(error),
                Ok(pairing) => {
                    if cx.core.reasoning { cx.plain(Plain::Text("\x1b[0m\n".into())); }
                    cx.plain(Plain::Line(format!("{}\n{}\npending {} — expires in 5 min", ::link::qr(&pairing.string), pairing.string, pairing.pending_id)));
                    self.invite = Some(Countdown::append("node-invite", format!("mesh device pairing · pending {}", pairing.pending_id),
                        ::link::Pairing { key: pairing.string, expires_at_ms: pairing.expires_ms }, cx));
                }
            },
            Input::Tick { now_ms, .. } => if let Some(card) = &mut self.invite && !card.tick(now_ms, cx) { self.invite = None; },
            Input::Lifecycle(Lifecycle::SessionReset | Lifecycle::TranscriptReplaced) => self.invite = None,
            _ => {}
        }
    }
    fn view(&self, _: Surface<'_>, _: &ViewCx<'_>) -> Option<View> { None }
    fn wake(&self) -> Option<u64> { self.invite.as_ref().map(Countdown::wake) }
}
/// One `/mesh devices` row, preserving the existing relative-time formatting.
pub fn mesh_device_line(d: &::link::DeviceInfo, now: u64) -> String {
    let since = |ms: u64| ago(chrono::Duration::milliseconds(now.saturating_sub(ms) as i64));
    let state = if d.revoked { "revoked".to_owned() } else if d.last_seen_ms == 0 {
        "never seen".to_owned()
    } else { format!("seen {}", since(d.last_seen_ms)) };
    format!("{}  {}  {}  paired {} · {state}", d.id, d.name, d.platform, since(d.paired_ms))
}

#[cfg(test)]
#[test]
fn mesh_pairing_freezes_previous_node_invite() {
    use crate::model::{CardKind, Entry, Model, UiEvent};
    let mut model = Model::default();
    model.components.register(super::nodes::Nodes::default()).unwrap();
    model.components.register(MeshDevices::default()).unwrap();
    let now = now_ms();
    let node_token = model.components.token("nodes");
    model.apply(UiEvent::Component(LocalReply { to: node_token, engine_epoch: 0,
        body: Box::new(super::nodes::Msg::Invited(Ok(("build-box".into(), ::link::Pairing {
            key: "https://relay.example/link#1.BBBB.QQ".into(), expires_at_ms: now + 300_000,
        })))) }));
    let body = |model: &Model, index: usize| match &model.entries[index] {
        Entry::Card { kind: CardKind::Component { group: "node-invite", key, .. }, title, text, .. } =>
            (*key, title.clone(), text.view().plain.to_owned() + &text.view().lines()
                .flat_map(|line| line.iter().map(|span| span.text.as_str())).collect::<String>()),
        _ => panic!("shared invite card"),
    };
    let frozen = body(&model, 0);
    let mesh_token = model.components.token("mesh-devices");
    model.apply(UiEvent::Component(LocalReply { to: mesh_token, engine_epoch: 0,
        body: Box::new(Msg::Pairing(Ok(::link::PairingPayload {
            string: "https://relay.example/link#1.AAAA.d29yaw.QWJDLV8xMjM".into(),
            expires_ms: now + 300_000, pending_id: "AbC-_123".into(),
        }))) }));
    model.component_tick(now + 2_000);
    assert_eq!(body(&model, 0), frozen);
    assert_eq!(body(&model, 1).1, "mesh device pairing · pending AbC-_123");
    assert!(body(&model, 1).2.contains("expires in 4:58"));
    assert!(model.components.get::<super::nodes::Nodes>().unwrap().wake().is_none());
    assert!(model.components.get::<MeshDevices>().unwrap().wake().is_some());
}
