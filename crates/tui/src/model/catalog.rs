use super::*;


impl Model {


    /// `/model <id>` or a picker pick (148 R1): `SetModel` for the shown
    /// session only; with none yet the draft's model, shown in the header.
    pub(super) fn pick_model(&mut self, model: String) {
        match self.session {
            Some(_) => {}
            None => {
                self.entries
                    .push(Entry::Info(format!("model → {model} (next session)")));
                // 194 D4c, as `set_model` does: drop a draft effort the new model
                // does not take (one the catalog does not know is kept).
                let draft = self.draft.get_or_insert_default();
                let known = self.catalog.as_ref().and_then(|c| c.closest(&model));
                if let (Some(e), Some(known)) = (&draft.effort, known)
                    && known.validate_effort(e).is_err()
                {
                    self.entries
                        .push(Entry::Info(format!("effort reset: not offered by {model}")));
                    draft.effort = None;
                    self.effort = self.default_effort.clone();
                }
                self.model_id = model.clone();
                draft.model = Some(model);
            }
        }
    }

    /// `/effort <v>` or the picker (148 R1): `SetEffort` for the shown session
    /// only (the reply announces it); with none yet the draft's effort
    /// (`None` = the engine default again).
    pub(super) fn set_effort(&mut self, effort: Option<String>) {
        match self.session {
            Some(_) => {}
            None => {
                self.entries.push(Entry::Info(match &effort {
                    Some(e) => format!("effort: {e} (next session)"),
                    None => "effort cleared (next session)".to_owned(),
                }));
                self.effort = effort.clone().or_else(|| self.default_effort.clone());
                self.draft.get_or_insert_default().effort = effort;
            }
        }
    }


    /// The draft's options for a command that opens the session (148 R1):
    /// `Some` only while a pick is pending and no session is shown.
    pub(super) fn draft_options(&self) -> Option<CreateSessionOptions> {
        Self::draft_options_ref(self.draft.as_ref(), self.session).cloned()
    }

    /// Borrow the same picks without cloning on every component dispatch.
    pub(super) fn draft_options_ref(draft: Option<&CreateSessionOptions>, session: Option<SessionId>) -> Option<&CreateSessionOptions> {
        draft.filter(|d| session.is_none() && (d.model.is_some() || d.effort.is_some()))
    }
}





