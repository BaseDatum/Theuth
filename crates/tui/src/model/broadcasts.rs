use super::*;

impl Model {

    /// `/broadcast [running|roots] [text]` (62 §5.1): bare opens the picker
    /// over `list_activity()`; with text the dry run goes straight to review
    /// (`running` when no selector is named).
    pub(super) fn broadcast_cmd(&mut self, arg: &str, tx: &UnboundedSender<Command>) {
        if arg.is_empty() {
            send(tx, Command::ListActivity);
            return;
        }
        let (word, rest) = arg
            .split_once(char::is_whitespace)
            .map_or((arg, ""), |(w, r)| (w, r.trim()));
        let (targets, text) = match word {
            "running" => (BroadcastTargets::Running, rest),
            "roots" => (BroadcastTargets::Roots, rest),
            _ => (BroadcastTargets::Running, arg),
        };
        if text.is_empty() {
            self.entries.push(Entry::Error(
                "usage: /broadcast [running|roots] [text]".into(),
            ));
            return;
        }
        let b = Broadcast::review(targets, text.to_owned());
        send(tx, Command::Broadcast(b.request(true)));
        self.menu = Some(Menu::Broadcast(Box::new(b)));
    }

    /// A `broadcast` reply: a dry run fills the review; a send closes the
    /// menu and prints the receipt; a refusal stays on the menu's error line.
    pub(super) fn broadcasted(&mut self, r: Result<BroadcastReceipt, String>) {
        let Some(Menu::Broadcast(b)) = &mut self.menu else {
            return;
        };
        match (r, b.stage, b.preview.is_some()) {
            (Err(e), _, _) => b.error = Some(e),
            (Ok(receipt), BroadcastStage::Review, true) => {
                self.menu = None;
                self.entries.extend(
                    crate::broadcast::result_lines(&receipt)
                        .into_iter()
                        .map(Entry::Info),
                );
            }
            (Ok(receipt), _, _) => {
                b.preview = Some(receipt);
                b.error = None;
                b.stage = BroadcastStage::Review;
                b.cursor = 0;
            }
        }
    }

    /// `/broadcast` keys: Pick ticks (`space`, `r` running roots, `a` all
    /// roots, `n` none, `d` delivery) and ⏎ composes; Compose edits the line
    /// and ⏎ asks for the dry run; Review's ⏎ sends. Esc backs one stage.
    pub(super) fn broadcast_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        let Some(Menu::Broadcast(b)) = &mut self.menu else {
            return MenuKey::Consumed;
        };
        let len = match b.stage {
            BroadcastStage::Pick => b.rows.len(),
            BroadcastStage::Compose => 0,
            BroadcastStage::Review => b.preview.as_ref().map_or(0, |r| r.rows.len() + 2),
        };
        match (code, b.stage) {
            (KeyCode::Up, _) => b.cursor = b.cursor.saturating_sub(1),
            (KeyCode::Down, _) => b.cursor = (b.cursor + 1).min(len.saturating_sub(1)),
            (KeyCode::Char(' '), BroadcastStage::Pick) => b.toggle(),
            (KeyCode::Char('r'), BroadcastStage::Pick) => {
                b.tick_roots(|a| crate::broadcast::running(&a.activity));
            }
            (KeyCode::Char('a'), BroadcastStage::Pick) => b.tick_roots(|_| true),
            (KeyCode::Char('n'), BroadcastStage::Pick) => b.selected.clear(),
            (KeyCode::Char('d'), BroadcastStage::Pick) => b.toggle_delivery(),
            (KeyCode::Enter, BroadcastStage::Pick) => {
                if !b.selected.is_empty() {
                    b.stage = BroadcastStage::Compose;
                    b.error = None;
                }
            }
            (KeyCode::Esc, BroadcastStage::Pick) => self.menu = None,
            (KeyCode::Char(c), BroadcastStage::Compose) => b.text.push(c),
            (KeyCode::Backspace, BroadcastStage::Compose) => {
                b.text.pop();
            }
            (KeyCode::Enter, BroadcastStage::Compose) => {
                if !b.text.trim().is_empty() {
                    b.select_ticked();
                    b.error = None;
                    send(tx, Command::Broadcast(b.request(true)));
                }
            }
            (KeyCode::Esc, BroadcastStage::Compose) => {
                b.stage = BroadcastStage::Pick;
                b.cursor = 0;
            }
            (KeyCode::Enter, BroadcastStage::Review) => {
                if b.preview.is_some() {
                    b.error = None;
                    send(tx, Command::Broadcast(b.request(false)));
                }
            }
            (KeyCode::Esc, BroadcastStage::Review) => {
                if b.picked() {
                    b.stage = BroadcastStage::Compose;
                    b.preview = None;
                    b.cursor = 0;
                } else {
                    self.menu = None;
                }
            }
            _ => {}
        }
        MenuKey::Consumed
    }
}
