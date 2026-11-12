use super::*;


impl Model {

    /// `/advisor [off|on|status|<provider/model>]` (125 §7.1): bare opens the
    /// `/model` picker targeted at the advisor; `on` re-uses the remembered
    /// model; anything else is a `provider/model` id. Not on an ACP child:
    /// the advisor reads theuth's transcript rows, which a harness does not produce.
    pub(super) fn advisor_cmd(&mut self, arg: &str, tx: &UnboundedSender<Command>) {
        if self.agent.is_some() {
            self.entries.push(Entry::Error(
                "/advisor: not available for an external harness session".into(),
            ));
            return;
        }
        if arg.is_empty() {
            use crate::components::models::{Models, Msg, Pick, Selection};
            let session = self.session;
            let commands = tx.clone();
            let selection = Selection::external(
                "advisor".into(), self.advisor.model.clone(), self.advisor.effort.clone(), true, false, true,
                Box::new(move |pick| {
                    match (session, pick) {
                        (Some(session), Pick::Model(model, effort)) => send(&commands, Command::SetAdvisor { session, model, effort }),
                        (Some(session), Pick::Effort(model @ Some(_), effort)) => send(&commands, Command::SetAdvisor { session, model, effort }),
                        (None, _) => return Some(crate::components::CoreAction::Reject("no session yet: send a message first".into())),
                        _ => {}
                    }
                    None
                }),
            );
            self.component_message::<Models>(Msg::Open { selection, chips: false });
            return;
        }
        // R1.2: the cap is an engine default, not a per-session setting.
        if let Some(n) = arg.strip_prefix("consults") {
            match n.trim().parse::<u8>() {
                Ok(n) if n <= 32 => send(tx, Command::SetAdvisorMaxConsults(n)),
                _ => self.entries.push(Entry::Error(format!(
                    "/advisor consults: expected a number 0-32, got {:?}",
                    n.trim()
                ))),
            }
            return;
        }
        let Some(session) = self.session else {
            self.entries
                .push(Entry::Error("no session yet: send a message first".into()));
            return;
        };
        let (model, effort) = match arg {
            "status" => {
                self.advisor_reply = Some(AdvisorReply::Status);
                send(tx, Command::AdvisorStatus(session));
                return;
            }
            "off" => (None, None),
            "on" => match self.advisor_default.model.clone() {
                Some(m) => (Some(m), self.advisor_default.effort.clone()),
                None => {
                    self.entries.push(Entry::Error(
                        "/advisor on: no remembered advisor model — pick one with /advisor".into(),
                    ));
                    return;
                }
            },
            id => (Some(id.to_owned()), None),
        };
        self.set_advisor(session, model, effort, tx);
    }

    /// `SetAdvisor` for `session`: a model pick or a chip; the reply installs silently.
    fn set_advisor(
        &mut self,
        session: SessionId,
        model: Option<String>,
        effort: Option<String>,
        tx: &UnboundedSender<Command>,
    ) {
        self.advisor_reply = None;
        send(
            tx,
            Command::SetAdvisor {
                session,
                model,
                effort,
            },
        );
    }

    /// Install the shown session's advisor status (a reply or `AdvisorChanged`,
    /// whichever lands first): one `advisor → …` line when the model or the
    /// off state changed (125 §7.5), the D-R1.5 pairing warning once per link
    /// after it; a model remembers itself as the `/advisor on` default.
    pub(super) fn set_advisor_status(&mut self, status: AdvisorStatus) {
        let off = |s: &AdvisorStatus| s.state == AdvisorState::Off || s.model.is_none();
        let changed = status.model != self.advisor.model || off(&status) != off(&self.advisor);
        let warn = status.warning.is_some()
            && !off(&status)
            && (changed || status.warning != self.advisor.warning);
        if status.model.is_some() {
            self.advisor_default.model.clone_from(&status.model);
            self.advisor_default.effort.clone_from(&status.effort);
        }
        self.advisor = status;
        self.component_message::<crate::components::models::Models>(crate::components::models::Msg::Current {
            noun: "advisor".into(), model: self.advisor.model.clone(), effort: self.advisor.effort.clone(),
        });
        if changed {
            self.interject(Entry::Info(advisor_changed_line(&self.advisor)));
        }
        if let Some(w) = self.advisor.warning.as_deref().filter(|_| warn) {
            self.interject(Entry::Info(format!("advisor: {w}")));
        }
    }

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


impl Model {


    /// `/aws`: Enter selects the highlighted profile for the whole harness;
    /// row 0 restores the AWS default credential chain.
    pub(super) fn aws_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        let Some(Menu::AwsProfiles { info, cursor }) = &mut self.menu else {
            return MenuKey::Consumed;
        };
        match code {
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Down => *cursor = (*cursor + 1).min(info.profiles.len()),
            KeyCode::Esc => self.menu = None,
            KeyCode::Enter => {
                let picked = cursor.checked_sub(1).map(|i| info.profiles.get(i).cloned());
                self.menu = None;
                match picked {
                    None => send(tx, Command::SetAwsProfile(None)),
                    Some(Some(p)) => send(tx, Command::SetAwsProfile(Some(p))),
                    Some(None) => {}
                }
            }
            _ => {}
        }
        MenuKey::Consumed
    }

    /// `/aws` picker rows: the default chain, then every SDK-discovered
    /// profile; `●` marks the selection in force.
    pub fn aws_rows(&self) -> Vec<String> {
        let Some(Menu::AwsProfiles { info, .. }) = &self.menu else {
            return Vec::new();
        };
        let mark = |on: bool| if on { "●" } else { " " };
        std::iter::once(format!(
            "{} default credential chain ({})",
            mark(info.selected.is_none()),
            info.default_profile
        ))
        .chain(
            info.profiles
                .iter()
                .map(|p| format!("{} {p}", mark(info.selected.as_deref() == Some(p.as_str())))),
        )
        .collect()
    }
}


/// The `/vision` reply: the model that inspects images for a text-only main
/// model (52 §3), or off.
pub fn vision_model_line(m: Option<&ModelInfo>) -> String {
    match m {
        Some(m) => format!(
            "vision fallback: {} ({}) — used only when the main model cannot view images; it receives the image and a question written by the main model, an extra call to that provider",
            m.id, m.name
        ),
        None => "vision fallback: off — a text-only main model gets images as omitted (/vision <model> to set one)".into(),
    }
}

/// The `/aws` reply: which AWS profile Bedrock calls use, harness-wide.
pub fn aws_profile_line(p: &AwsProfiles) -> String {
    let now = match &p.selected {
        Some(s) if p.profiles.contains(s) => s.clone(),
        Some(s) => {
            format!("{s} (not found by the AWS SDK; `/aws clear` restores the default chain)")
        }
        None => format!(
            "default credential chain ({}), unless a Bedrock API key is configured",
            p.default_profile
        ),
    };
    let found = if p.profiles.is_empty() {
        "none found".to_owned()
    } else {
        p.profiles.join(", ")
    };
    format!(
        "aws profile: {now} — applies to every session's Bedrock calls from the next request\n  profiles: {found}"
    )
}
