//! ACP configured harness inventory, registry and probe picker.
use super::*;
use ::acp::{EntryInfo, registry::RegistryAgent};
use rpc_compose::api::clients::*;

#[derive(Default)]
pub(crate) struct Acp {
    pub(crate) menu: Option<Menu>,
    pub(crate) awaiting: bool,
}
pub(crate) struct Menu {
    pub(crate) entries: Vec<EntryInfo>,
    pub(crate) registry: Vec<RegistryAgent>,
    pub(crate) cursor: usize,
}
#[derive(Clone, Debug)]
pub(crate) enum Request { ListAcp { probe: bool }, AddAcp(String), RemoveAcp(String), ListAcpRegistry }
pub(crate) enum Msg { Acp(Vec<EntryInfo>), AcpRegistry(Vec<RegistryAgent>), AcpAdded(EntryInfo), Failed(String), Relist }
fn request(cx: &mut Cx<'_, '_, Msg>, request: Request) {
    cx.spawn_request(Lane::Concurrent, request, |api, request| async move {
        let result: Result<Msg, jsonrpsee::core::client::Error> = async {
            Ok(match request {
                Request::ListAcp { probe } => Msg::Acp(api.list_acp(probe).await?),
                Request::ListAcpRegistry => Msg::AcpRegistry(api.list_acp_registry().await?),
                Request::AddAcp(id) => Msg::AcpAdded(api.add_acp(id).await?),
                Request::RemoveAcp(name) => { api.remove_acp(name).await?; Msg::Acp(api.list_acp(false).await?) }
            })
        }.await;
        result.unwrap_or_else(|e| Msg::Failed(crate::reason(e)))
    });
}
/// One line of the `/acp` menu.
#[derive(Debug)]
pub(crate) enum AcpRow<'a> {
    /// A configured harness (`agents.yaml`).
    Entry(&'a EntryInfo),
    /// Blank line between the two halves; the cursor skips it.
    Sep,
    /// A registry agent, addable with Enter.
    Registry(&'a RegistryAgent),
}

impl Acp {

    /// `/acp` menu rows in cursor order: entries, a separator when both
    /// halves are present, then the registry.
    pub(crate) fn rows(&self) -> Vec<AcpRow<'_>> {
        let Some(Menu {
            entries, registry, ..
        }) = &self.menu
        else {
            return Vec::new();
        };
        let mut rows: Vec<AcpRow<'_>> = entries.iter().map(AcpRow::Entry).collect();
        if !entries.is_empty() && !registry.is_empty() {
            rows.push(AcpRow::Sep);
        }
        rows.extend(registry.iter().map(AcpRow::Registry));
        rows
    }

    fn key(&mut self, code: KeyCode, cx: &mut Cx<'_, '_, Msg>) {
        enum Act {
            Move(usize),
            Send(Request),
            Probe,
            Close,
            Nothing,
        }
        let Some(Menu { cursor, .. }) = &self.menu else {
            return;
        };
        let rows = self.rows();
        let last = rows.len().saturating_sub(1);
        let cursor = (*cursor).min(last);
        // Up/Down step over the separator row.
        let step = |to: usize, dir: isize| match rows.get(to) {
            Some(AcpRow::Sep) => to.saturating_add_signed(dir).min(last),
            _ => to,
        };
        let act = match (code, rows.get(cursor)) {
            (KeyCode::Up, _) => Act::Move(step(cursor.saturating_sub(1), -1)),
            (KeyCode::Down, _) => Act::Move(step((cursor + 1).min(last), 1)),
            (KeyCode::Esc, _) => Act::Close,
            (KeyCode::Char('p'), _) => Act::Probe,
            (KeyCode::Enter, Some(AcpRow::Registry(a))) => {
                Act::Send(Request::AddAcp(a.id.clone()))
            }
            (KeyCode::Char('d') | KeyCode::Backspace, Some(AcpRow::Entry(e))) => {
                Act::Send(Request::RemoveAcp(e.name.clone()))
            }
            _ => Act::Nothing,
        };
        let Some(Menu {
            entries,
            cursor: cur,
            ..
        }) = &mut self.menu
        else {
            return;
        };
        *cur = cursor;
        match act {
            Act::Move(to) => *cur = to,
            Act::Probe => {
                for e in entries.iter_mut() {
                    e.status = Some("probing…".into());
                }
                request(cx, Request::ListAcp { probe: true });
            }
            Act::Send(cmd) => request(cx, cmd),
            Act::Close => { self.menu = None; cx.close(); },
            Act::Nothing => {}
        }
    }

}


impl Component for Acp {
    type Msg = Msg;
    fn declaration(&self) -> Declaration {
        Declaration { id: "acp", interests: &[], slash: &[("/acp", "[add <registry-id>|remove <name>]", "External harnesses (ACP) and the registry")], bindings: &[] }
    }
    fn decode(&self, _: &TopicEvent, _: &CoreView<'_>) -> Option<Msg> { None }
    fn update(&mut self, input: Input<'_, Msg>, cx: &mut Cx<'_, '_, Msg>) {
        if cx.core.reasoning && matches!(&input, Input::Message(Msg::Acp(_) | Msg::AcpRegistry(_) | Msg::AcpAdded(_))) { cx.plain(Plain::Text("\x1b[0m\n".into())); }
        match input {
            Input::Slash { args: "", .. } => {
                self.awaiting = true;
                request(cx, Request::ListAcp { probe: false });
                request(cx, Request::ListAcpRegistry);
            }
            Input::Slash { args, .. } => match args.split_once(' ').map(|(s, r)| (s, r.trim())) {
                Some(("add", id)) if !id.is_empty() => request(cx, Request::AddAcp(id.to_owned())),
                Some(("remove", name)) if !name.is_empty() => request(cx, Request::RemoveAcp(name.to_owned())),
                _ => cx.core(CoreAction::Reject("usage: /acp [add <registry-id> | remove <name>]".into())),
            },
            Input::Message(Msg::Acp(list)) => {
                for e in &list { cx.plain(Plain::Line(format!("{}  {} {}  {}", e.name, e.command, e.args.join(" "), e.status.as_deref().unwrap_or_default()))); }
                match &mut self.menu {
                    Some(Menu { entries, .. }) => *entries = list,
                    _ if self.awaiting => {
                        self.awaiting = false;
                        self.menu = Some(Menu { entries: list, registry: Vec::new(), cursor: 0 }); cx.focus(FocusClass::Modal);
                    }
                    _ => {}
                }
            }
            Input::Message(Msg::AcpRegistry(list)) => {
                for a in &list { cx.plain(Plain::Line(format!("+ {}  {} {}  {}", a.id, a.name, a.version, a.description))); }
                if let Some(Menu { registry, .. }) = &mut self.menu { *registry = list; }
            }
            Input::Message(Msg::AcpAdded(e)) => {
                let line = format!("acp: added `{}` ({} {})", e.name, e.command, e.args.join(" "));
                cx.plain(Plain::Line(line.clone())); cx.core(CoreAction::Notice(line)); cx.post(Msg::Relist);
            }
            Input::Message(Msg::Relist) => request(cx, Request::ListAcp { probe: false }),
            Input::Message(Msg::Failed(error)) => cx.error(error),
            Input::Key(key) if self.menu.is_some() => { self.key(key.code, cx); cx.consume_key(); }
            Input::Lifecycle(Lifecycle::FocusLost) => self.menu = None,
            Input::Lifecycle(Lifecycle::SessionReset) => { *self = Self::default(); cx.close(); }
            _ => {}
        }
    }
    fn view(&self, at: Surface<'_>, cx: &ViewCx<'_>) -> Option<View> {
        if !matches!(at, Surface::Modal) { return None; }
        let cursor = self.menu.as_ref()?.cursor;
        let rows = self.rows().iter().map(|r| match r {
            AcpRow::Entry(e) => {
                // ponytail: fixed 40-char command; pass the terminal width if it grates.
                let cmd: String = format!("{} {}", e.command, e.args.join(" ")).chars().take(40).collect();
                format!("{:<16} {:<40} {}", e.name, cmd, e.status.as_deref().unwrap_or_default())
            }
            AcpRow::Sep => String::new(),
            AcpRow::Registry(a) => {
                let desc: String = a.description.chars().take(50).collect();
                format!("+ {:<16} {:<20} {}", a.id, format!("{} {}", a.name, a.version), desc)
            }
        }).collect();
        Some(View::IntrinsicElement { element: crate::ui::menu_view("acp  p probe  ⏎ add from registry  d remove".into(), rows, cursor, cx.width as u16, cx.height as u16, false), rows: 0, replace: true })
    }
}
