use super::*;

impl Model {

    /// The input box takes keys: no approval prompt, no modal menu (the
    /// slash popup rides on the box) and the todo card not focused (F2). A
    /// shown `secret_request` types into the box (masked).
    pub fn input_focused(&self) -> bool {
        if self.approval().is_some()
            || (self.components.focused() && self.components.editor().is_none())
            || self.components.composer_mode() != crate::components::ComposerMode::Default
            || self.components.captures(crate::components::FocusClass::Suggestion)
            || self.talk_view
        {
            return false;
        }
        if self.components.editor().is_some_and(|(class, _)| class == crate::components::FocusClass::Secret) {
            return true;
        }
        if let Some(a) = self.ask() {
            return a.typing;
        }
        // 141 §10.1: a watch-only child takes no input (the status line says
        // why) — except the one slash command `/` opened the editor for.
        (self.observed.is_none() || self.watch_slash)
            && !self.components.captures(crate::components::FocusClass::Pinned)
            && !self.modal_menu()
            && !self.historical
    }

    /// 141 §10.1: the read-only reason of a watch-only view, for the status
    /// line and as the one line typing gets; `None` for anything else.
    pub fn watch_hint(&self) -> Option<String> {
        let o = self.observed.as_ref()?;
        let stop = if o.can_cancel {
            "Esc stops it"
        } else {
            "cannot be stopped here"
        };
        Some(format!(
            "watch-only — {} runs this subagent · {stop}",
            harness_word(o.source)
        ))
    }

    /// A key the watch-only view swallows: the hint, once per run of keys.
    pub fn watch_swallow(&mut self) {
        if let Some(hint) = self.watch_hint()
            && !matches!(self.entries.last(), Some(Entry::Info(t)) if *t == hint)
        {
            self.entries.push(Entry::Info(hint));
        }
    }

    /// A menu that takes the keys is open: every menu but the slash popup.
    pub fn modal_menu(&self) -> bool {
        self.components.modal() || !matches!(self.menu, None | Some(Menu::Slash { .. }))
    }

}

