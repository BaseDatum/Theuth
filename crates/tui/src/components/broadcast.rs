//! Broadcast picker, composition and review; nothing sends before review's Enter.
use super::*;
use iocraft::KeyCode;
use agent::{AgentInfo, BroadcastReceipt, BroadcastRequest, BroadcastTargets};
use rpc_compose::api::clients::TurnsApiClient;
/// Existing picker, compose, review and receipt formatting state.
pub mod state;
use state::{Broadcast, BroadcastStage};

#[derive(Default)]
pub(crate) struct BroadcastPanel { pub(crate) menu: Option<Box<Broadcast>> }
#[derive(Clone, Debug)]
pub(crate) enum Request { ListActivity, Broadcast(BroadcastRequest) }
pub(crate) enum Msg { Activity(Result<Vec<AgentInfo>, String>), Broadcasted(Result<BroadcastReceipt, String>) }

impl BroadcastPanel {
    fn send(request: Request, cx: &mut Cx<'_, '_, Msg>) {
        cx.spawn_request(Lane::Concurrent, request, |api, request| async move {
            match request {
                Request::ListActivity => Msg::Activity(api.list_activity().await.map_err(crate::reason)),
                Request::Broadcast(request) => Msg::Broadcasted(api.broadcast(request).await.map_err(crate::reason)),
            }
        });
    }
    fn slash(&mut self, args: &str, cx: &mut Cx<'_, '_, Msg>) {
        if args.is_empty() { Self::send(Request::ListActivity, cx); return; }
        let (word, rest) = args.split_once(char::is_whitespace).map_or((args, ""), |(w, r)| (w, r.trim()));
        let (targets, text) = match word {
            "running" => (BroadcastTargets::Running, rest),
            "roots" => (BroadcastTargets::Roots, rest),
            _ => (BroadcastTargets::Running, args),
        };
        if text.is_empty() { cx.core(CoreAction::Reject("usage: /broadcast [running|roots] [text]".into())); return; }
        let menu = Broadcast::review(targets, text.to_owned());
        Self::send(Request::Broadcast(menu.request(true)), cx);
        self.menu = Some(Box::new(menu)); cx.focus(FocusClass::Modal);
    }
    fn key(&mut self, code: KeyCode, cx: &mut Cx<'_, '_, Msg>) {
        let Some(menu) = &mut self.menu else { return };
        let len = match menu.stage {
            BroadcastStage::Pick => menu.rows.len(), BroadcastStage::Compose => 0,
            BroadcastStage::Review => menu.preview.as_ref().map_or(0, |r| r.rows.len() + 2),
        };
        match (code, menu.stage) {
            (KeyCode::Up, _) => menu.cursor = menu.cursor.saturating_sub(1),
            (KeyCode::Down, _) => menu.cursor = (menu.cursor + 1).min(len.saturating_sub(1)),
            (KeyCode::Char(' '), BroadcastStage::Pick) => menu.toggle(),
            (KeyCode::Char('r'), BroadcastStage::Pick) => menu.tick_roots(|row| state::running(&row.activity)),
            (KeyCode::Char('a'), BroadcastStage::Pick) => menu.tick_roots(|_| true),
            (KeyCode::Char('n'), BroadcastStage::Pick) => menu.selected.clear(),
            (KeyCode::Char('d'), BroadcastStage::Pick) => menu.toggle_delivery(),
            (KeyCode::Enter, BroadcastStage::Pick) if !menu.selected.is_empty() => { menu.stage = BroadcastStage::Compose; menu.error = None; }
            (KeyCode::Esc, BroadcastStage::Pick) => self.menu = None,
            (KeyCode::Char(c), BroadcastStage::Compose) => menu.text.push(c),
            (KeyCode::Backspace, BroadcastStage::Compose) => { menu.text.pop(); }
            (KeyCode::Enter, BroadcastStage::Compose) if !menu.text.trim().is_empty() => {
                menu.select_ticked(); menu.error = None; Self::send(Request::Broadcast(menu.request(true)), cx);
            }
            (KeyCode::Esc, BroadcastStage::Compose) => { menu.stage = BroadcastStage::Pick; menu.cursor = 0; }
            (KeyCode::Enter, BroadcastStage::Review) if menu.preview.is_some() => {
                menu.error = None; Self::send(Request::Broadcast(menu.request(false)), cx);
            }
            (KeyCode::Esc, BroadcastStage::Review) => {
                if menu.picked() { menu.stage = BroadcastStage::Compose; menu.preview = None; menu.cursor = 0; }
                else { self.menu = None; }
            }
            _ => {}
        }
        if self.menu.is_none() { cx.close(); }
    }
}
impl Component for BroadcastPanel {
    type Msg = Msg;
    fn declaration(&self) -> Declaration {
        Declaration { id: "broadcast", interests: &[], slash: &[("/broadcast", "[running|roots] [text]", "send one message to many sessions")], bindings: &[] }
    }
    fn decode(&self, _: &TopicEvent, _: &CoreView<'_>) -> Option<Msg> { None }
    fn update(&mut self, input: Input<'_, Msg>, cx: &mut Cx<'_, '_, Msg>) {
        if cx.core.reasoning && matches!(&input, Input::Message(Msg::Activity(Ok(_)) | Msg::Broadcasted(_))) {
            cx.plain(Plain::Text("\x1b[0m\n".into()));
        }
        match input {
            Input::Slash { args, .. } => self.slash(args, cx),
            Input::Key(key) if self.menu.is_some() => { cx.consume_key(); self.key(key.code, cx); }
            Input::Message(Msg::Activity(Err(error))) => cx.error(error),
            Input::Message(Msg::Activity(Ok(rows))) => {
                for row in &rows {
                    cx.plain(Plain::Line(format!("{}{}  {}  j{} p{}  {}  {}", "  ".repeat(usize::from(row.depth)), row.name, row.activity.state.word(), row.activity.jobs, row.activity.ptys, state::age(row.activity.last_active), crate::model::short(row.id))));
                }
                self.menu = Some(Box::new(Broadcast::pick(rows))); cx.focus(FocusClass::Modal);
            }
            Input::Message(Msg::Broadcasted(reply)) => {
                match &reply {
                    Err(error) => cx.plain(Plain::Line(format!("broadcast: {error}"))),
                    Ok(receipt) => for line in state::result_lines(receipt) { cx.plain(Plain::Line(line)); },
                }
                let Some(menu) = &mut self.menu else { return };
                match (reply, menu.stage, menu.preview.is_some()) {
                    (Err(error), _, _) => menu.error = Some(error),
                    (Ok(receipt), BroadcastStage::Review, true) => {
                        self.menu = None; cx.close();
                        for line in state::result_lines(&receipt) { cx.core(CoreAction::Notice(line)); }
                    }
                    (Ok(receipt), _, _) => { menu.preview = Some(receipt); menu.error = None; menu.stage = BroadcastStage::Review; menu.cursor = 0; }
                }
            }
            Input::Lifecycle(Lifecycle::SessionReset | Lifecycle::FocusLost) => { self.menu = None; cx.close(); }
            _ => {}
        }
    }
    fn view(&self, at: Surface<'_>, cx: &ViewCx<'_>) -> Option<View> {
        if !matches!(at, Surface::Modal) { return None; }
        let menu = self.menu.as_ref()?;
        let spinner = cx.spinner();
        let rows = menu.rows(spinner);
        let count = rows.len().min(10) + 3;
        Some(View::IntrinsicElement { rows: count, replace: false, element: crate::ui::menu_view(menu.title(), rows, menu.cursor, cx.width as u16, cx.height as u16, false) })
    }
}
