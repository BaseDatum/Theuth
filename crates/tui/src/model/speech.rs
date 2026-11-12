use super::*;

impl Model {

    /// Fold a voice bus event in (54 §4.4, 153 §3.1): the model state, the
    /// dictation events for the dictation this view started, and a
    /// dictionary suggestion for this surface. Other ids (another frontend's
    /// dictation) and TTS events are ignored.
    pub(super) fn apply_voice(&mut self, ev: voice::VoiceEvent) {
        use voice::VoiceEvent::*;
        match ev {
            DictationModel { state } => self.dictation_model = state,
            DictationStarted { id } => {
                if let Some(d) = self.dictation.as_mut().filter(|d| d.id == id)
                    && d.phase == Phase::Starting
                {
                    d.phase = Phase::Dictating;
                }
            }
            DictationLevel { id, levels } => {
                if let Some(d) = self.dictation.as_mut().filter(|d| d.id == id) {
                    d.push_levels(&levels);
                }
            }
            DictationText {
                id,
                utterance,
                text,
                is_final,
                shaped,
                command,
                ..
            } => {
                if let Some(d) = self.dictation.as_mut().filter(|d| d.id == id) {
                    d.text_event(utterance as usize, text, shaped, command, is_final);
                }
            }
            DictationFinished { id, reason } => {
                let Some(d) = self.dictation.take_if(|d| d.id == id) else {
                    return;
                };
                // Esc already restored `before + after`.
                if !d.discard {
                    if !dictation::body(&d.utts).is_empty() {
                        self.dictated = Some(id);
                    }
                    let (value, caret) = d.splice();
                    // "send it" (153 §7.4): the spliced value takes the send key's path.
                    if d.send && !value.trim().is_empty() {
                        self.pending_send = Some(value);
                    } else {
                        self.pending_input = Some((value, caret));
                    }
                }
                if let voice::FinishReason::Error { message } = reason {
                    self.entries
                        .push(Entry::Error(format!("dictation: {message}")));
                }
            }
            DictationDevice { id, device, state } => {
                if let Some(d) = self.dictation.as_mut().filter(|d| d.id == id) {
                    d.mic = Some(match state {
                        voice::DeviceState::Lost => "mic lost — reconnecting…".to_owned(),
                        voice::DeviceState::Changed => {
                            format!("mic → {}", device.as_deref().unwrap_or("?"))
                        }
                    });
                }
            }
            DictionarySuggestion {
                id,
                heard,
                typed,
                occurrences,
                surface: voice::Surface::Tui,
                ..
            } => {
                self.suggestion = Some(Suggestion {
                    id,
                    heard,
                    typed,
                    occurrences,
                    shown_ms: now_ms(),
                });
            }
            SpeechFinished { id, .. } => {
                if let Some(t) = &mut self.talk {
                    t.speech_finished(id);
                }
            }
            VoiceModel { .. }
            | SpeechStarted { .. }
            | SpeechAudio { .. }
            | DictionarySuggestion { .. } => {}
        }
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

    /// Ctrl+D / `/dictate` (54 §5.2). Idle with the model `Ready`: anchor the
    /// caret at `before | after` and start. Idle otherwise: say why, do
    /// nothing. Dictating: stop (keep the text); a key during the "send it"
    /// settle keeps editing instead of sending (153 §3.1). Starting or
    /// stopping: no-op.
    pub fn toggle_dictation(&mut self, before: String, after: String) -> Option<Command> {
        self.stop_dictation(false).or_else(|| {
            if self.dictation.is_some() || self.dictation_pending.is_some() {
                return None;
            }
            let reason = match &self.dictation_model {
                voice::ModelState::Ready => {
                    self.dictation_pending = Some((before, after));
                    return Some(Command::DictationStart {
                        pressed: std::time::Instant::now(),
                    });
                }
                voice::ModelState::Loading => "dictation: model loading…".to_owned(),
                voice::ModelState::Missing => {
                    "dictation: model missing (~/.theuth/models/nemotron-streaming-en or voice.dictation_model_dir)"
                        .to_owned()
                }
                voice::ModelState::Failed { message } => format!("dictation: {message}"),
            };
            self.entries.push(Entry::Info(reason));
            None
        })
    }

    /// Stop the running dictation, keeping the text. `send`: ⏎ during the
    /// "send it" settle — "send now" — keeps the submit; any other key drops it.
    pub fn stop_dictation(&mut self, send: bool) -> Option<Command> {
        let d = self
            .dictation
            .as_mut()
            .filter(|d| d.phase == Phase::Dictating)?;
        d.phase = Phase::Stopping;
        d.send &= send;
        Some(Command::DictationStop(d.id))
    }

    /// `y` / `n` / `N` on the suggestion line (153 §6.4); Esc is "not now".
    pub fn answer_suggestion(&mut self, answer: voice::SuggestionAnswer) -> Option<Command> {
        let s = self.suggestion.take()?;
        Some(Command::SuggestionAnswer { id: s.id, answer })
    }

    /// When the suggestion line auto-dismisses (unix ms); `None` without one.
    pub fn suggestion_wake(&self) -> Option<u64> {
        self.suggestion
            .as_ref()
            .map(|s| s.shown_ms + dictation::SUGGESTION_MS)
    }

    /// Drop a suggestion past its 20 s: "not now" without the cooldown, so no RPC.
    pub fn expire_suggestion(&mut self, now_ms: u64) {
        if self.suggestion.as_ref().is_some_and(|s| s.expired(now_ms)) {
            self.suggestion = None;
        }
    }

    /// Esc while dictating: the editor takes `before + after` back now, the
    /// daemon is told to stop, and the later `Finished` splices nothing.
    pub fn discard_dictation(&mut self) -> Option<Command> {
        let d = self.dictation.as_mut()?;
        d.discard = true;
        d.phase = Phase::Stopping;
        self.pending_input = Some((format!("{}{}", d.before, d.after), d.before.len()));
        Some(Command::DictationStop(d.id))
    }
}

/// `/voice [mic <name|default> | shape <full|plain|off>]` (153 §8.1, §4.5).
pub fn voice_cmd(arg: &str) -> Result<Command, &'static str> {
    const USAGE: &str = "usage: /voice [mic <name|default> | shape <full|plain|off>]";
    match arg
        .split_once(char::is_whitespace)
        .map_or((arg, ""), |(c, r)| (c, r.trim()))
    {
        ("", _) => Ok(Command::VoiceStatus),
        ("mic", "default") => Ok(Command::SetInputDevices(Vec::new())),
        ("mic", name) if !name.is_empty() => Ok(Command::SetInputDevices(vec![name.to_owned()])),
        ("shape", "full") => Ok(Command::SetShape(voice::Preset::Full)),
        ("shape", "plain") => Ok(Command::SetShape(voice::Preset::Plain)),
        ("shape", "off") => Ok(Command::SetShape(voice::Preset::Off)),
        _ => Err(USAGE),
    }
}

/// The `/voice` reply (153 §8.1, §8.7, §4.5): the mic preference and what the host
/// enumerates, the running dictation's device, the shaping preset per surface, the
/// last dictation's timings.
pub fn voice_line(
    status: &voice::VoiceStatus,
    dictation: &voice::DictationState,
    devices: &[voice::InputDevice],
    settings: Option<&voice::VoiceSettings>,
) -> String {
    let ms = |v: Option<u32>| v.map_or_else(|| "—".to_owned(), |v| format!("{v} ms"));
    let mut out = format!(
        "voice: model {:?} · mic preference: {}",
        status.model,
        if status.input_devices.is_empty() {
            "system default".to_owned()
        } else {
            status.input_devices.join(" › ")
        }
    );
    for d in devices {
        let _ = write!(
            out,
            "\n  {} {}{}",
            if status.input_devices.contains(&d.name) {
                "●"
            } else {
                "○"
            },
            d.name,
            if d.default { "  (system default)" } else { "" }
        );
    }
    if let Some(dev) = &dictation.input_device {
        let _ = write!(out, "\n  dictating on {dev}");
    }
    if let Some(s) = settings {
        let p = |p: voice::Preset| format!("{p:?}").to_lowercase();
        let _ = write!(
            out,
            "\n  shape: tui {} · composer {} · companion {} · phone {} · any app {}",
            p(s.shape.tui),
            p(s.shape.composer),
            p(s.shape.companion),
            p(s.shape.phone),
            p(s.shape.any_app)
        );
    }
    match &dictation.last {
        Some(t) => {
            let _ = write!(
                out,
                "\n  last dictation: {:?} · mic {} · first words {} · final {} · stop → final {}{}",
                t.surface,
                ms(t.press_to_first_pcm_ms),
                ms(t.press_to_first_partial_ms),
                ms(t.press_to_first_final_ms),
                ms(t.stop_to_final_ms),
                t.device
                    .as_deref()
                    .map_or_else(String::new, |d| format!(" · {d}"))
            );
        }
        None => out.push_str("\n  last dictation: none yet"),
    }
    out.push_str("\n  /voice mic <name|default> picks the mic · /voice shape <full|plain|off> sets the TUI preset");
    out
}
