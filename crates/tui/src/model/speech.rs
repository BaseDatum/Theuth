use super::*;

impl Model {
    pub(super) fn apply_voice(&mut self, ev: voice::VoiceEvent) {
        if let voice::VoiceEvent::SpeechFinished { id, .. } = ev
            && let Some(t) = &mut self.talk
        { t.speech_finished(id); }
    }

    /// Fold a talk bus event in (76 §5.2) for the talk this view started;
    /// `TalkFinished` closes the view and reports an error reason.
    pub(super) fn apply_talk(&mut self, ev: talk::TalkEvent) {
        let Some(t) = &mut self.talk else { return };
        let Some(reason) = t.apply(ev, self.frame) else {
            return;
        };
        self.talk = None;
        self.talk_view = false;
        if let talk::EndReason::Error { message } = reason {
            self.entries.push(Entry::Error(format!("talk: {message}")));
        }
    }

    /// `/talk`: no talk → start one on the current session (created first
    /// when there is none) and open the view; one running with the view
    /// closed → re-enter; view open → stop and leave. Ctrl+T (`stop: true`)
    /// stops a running talk from anywhere. Waiting for the start reply: no-op.
    pub fn toggle_talk(&mut self, stop: bool) -> Option<Command> {
        if let Some(t) = &self.talk {
            if self.talk_view || stop {
                self.talk_view = false;
                return Some(Command::TalkStop(t.id));
            }
            self.talk_view = true;
            return None;
        }
        // Waiting for the start reply, or for the session a `Send` is creating
        // (`TalkStart { session: None }` would create a second one): no-op.
        if self.talk_pending || (self.session.is_none() && self.awaiting_session) {
            return None;
        }
        self.talk_pending = true;
        Some(Command::TalkStart {
            session: self.session,
            draft: self.draft_options(),
        })
    }

    /// Esc in the talk view (76 §5.2): while the agent is thinking, working,
    /// speaking or held, it is the agent's cancel — the talk stays on and
    /// returns to `Listening`; otherwise it leaves the view, the talk keeps running.
    pub fn talk_escape(&mut self) -> Option<Command> {
        match &self.talk {
            Some(t)
                if matches!(
                    t.state,
                    talk::State::Thinking
                        | talk::State::Working
                        | talk::State::Speaking
                        | talk::State::Holding
                ) =>
            {
                Some(Command::Cancel(t.session))
            }
            _ => {
                self.talk_view = false;
                None
            }
        }
    }

}
