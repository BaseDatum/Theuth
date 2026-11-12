//! Dictation and voice settings owner: state, keys, effects and composer presentation.
use super::*;
use std::fmt::Write as _;
use std::time::Instant;
use serde::Deserialize;
use rpc_compose::api::clients::*;
pub(crate) mod state;
use state::{self as dictation, Dictating, Phase, Suggestion};
use crate::model::now_ms;
use crate::ui::{FAIENCE, OCHRE, SAND};
use iocraft::prelude::{View as IView, element, MixedText, MixedTextContent, Text, TextWrap};

#[derive(Debug, Default)]
pub(crate) struct DictationVoice {
    pub(crate) dictation: Option<Dictating>,
    pub(crate) dictation_pending: Option<(String, String)>,
    pub(crate) dictation_model: voice::ModelState,
    pub(crate) dictated: Option<voice::DictationId>,
    pub(crate) suggestion: Option<Suggestion>,
}
#[derive(Clone, Debug)]
pub(crate) enum Request {
    DictationPrewarm,
    DictationStart { pressed: Instant, commands: Vec<String> },
    DictationStop(voice::DictationId),
    DictationCorrection { id: voice::DictationId, text: String },
    SuggestionAnswer { id: voice::SuggestionId, answer: voice::SuggestionAnswer },
    VoiceStatus, SetInputDevices(Vec<String>), SetShape(voice::Preset),
}
pub(crate) enum Msg {
    Voice(voice::VoiceEvent), Starting(voice::DictationId), Failed(String),
    Info(Result<String, String>), Done(Result<(), String>), CorrectionDone,
    Toggle { before: String, after: String }, Discard,
}
fn send(cx: &mut Cx<'_, '_, Msg>, request: Request) {
    cx.spawn_request(Lane::Concurrent, request, |api, request| async move {
        match request {
            Request::DictationPrewarm => match api.dictation_prewarm().await {
                Ok(state) => Msg::Voice(voice::VoiceEvent::DictationModel { state }),
                Err(error) => Msg::Done(Err(crate::reason(error))),
            },
            Request::DictationStart { pressed, commands } => {
                let opts = voice::DictationOpts {
                    surface: Some(voice::Surface::Tui), destination: Some(voice::Destination::AgentPrompt),
                    app: None, commands, press_ago_ms: u32::try_from(pressed.elapsed().as_millis()).ok(),
                };
                match api.dictation_start(None, Some(opts)).await {
                    Ok(id) => Msg::Starting(id), Err(error) => Msg::Failed(crate::reason(error)),
                }
            }
            Request::DictationStop(id) => Msg::Done(api.dictation_stop(id).await.map_err(crate::reason)),
            Request::DictationCorrection { id, text } => {
                if let Err(error) = api.dictation_correction(id, text).await {
                    tracing::debug!("dictation_correction: {}", crate::reason(error));
                }
                Msg::CorrectionDone
            }
            Request::SuggestionAnswer { id, answer } => Msg::Done(api.dictionary_suggestion_answer(id, answer).await.map_err(crate::reason)),
            Request::VoiceStatus => Msg::Info(async {
                let status = api.voice_status().await.map_err(crate::reason)?;
                let dictation = api.dictation_status().await.map_err(crate::reason)?;
                let devices = api.list_input_devices().await.unwrap_or_default();
                let settings = api.voice_settings().await.ok();
                Ok(voice_line(&status, &dictation, &devices, settings.as_ref()))
            }.await),
            Request::SetInputDevices(names) => Msg::Info(async {
                api.set_input_devices(names.clone()).await.map_err(crate::reason)?;
                Ok(match names.first() { Some(name) => format!("mic → {name}"), None => "mic → system default".to_owned() })
            }.await),
            Request::SetShape(preset) => Msg::Info(async {
                api.set_shape(voice::Surface::Tui, preset).await.map_err(crate::reason)?;
                Ok(format!("voice shape (tui) → {preset:?}").to_lowercase())
            }.await),
        }
    });
}
impl DictationVoice {
    /// Fold a voice bus event in (54 §4.4, 153 §3.1): the model state, the
    /// dictation events for the dictation this view started, and a
    /// dictionary suggestion for this surface. Other ids (another frontend's
    /// dictation) and TTS events are ignored.
    fn apply_voice(&mut self, ev: voice::VoiceEvent, cx: &mut Cx<'_, '_, Msg>) {
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
                        cx.core(CoreAction::SubmitEditor(value));
                    } else {
                        cx.core(CoreAction::ReplaceEditor { value, caret });
                    }
                }
                if let voice::FinishReason::Error { message } = reason {
                    cx.core(CoreAction::Reject(format!("dictation: {message}")));
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
            SpeechFinished { .. }
            | VoiceModel { .. }
            | SpeechStarted { .. }
            | SpeechAudio { .. }
            | DictionarySuggestion { .. } => {}
        }
    }

    /// Ctrl+D / `/dictate` (54 §5.2). Idle with the model `Ready`: anchor the
    /// caret at `before | after` and start. Idle otherwise: say why, do
    /// nothing. Dictating: stop (keep the text); a key during the "send it"
    /// settle keeps editing instead of sending (153 §3.1). Starting or
    /// stopping: no-op.
    fn toggle_dictation(&mut self, before: String, after: String, cx: &mut Cx<'_, '_, Msg>) -> Option<Request> {
        self.stop_dictation(false).or_else(|| {
            if self.dictation.is_some() || self.dictation_pending.is_some() {
                return None;
            }
            let reason = match &self.dictation_model {
                voice::ModelState::Ready => {
                    self.dictation_pending = Some((before, after));
                    return Some(Request::DictationStart {
                        pressed: std::time::Instant::now(),
                        commands: cx.core.commands.iter().map(|(name, _, _)| (*name).to_owned()).collect(),
                    });
                }
                voice::ModelState::Loading => "dictation: model loading…".to_owned(),
                voice::ModelState::Missing => {
                    "dictation: model missing (~/.theuth/models/nemotron-streaming-en or voice.dictation_model_dir)"
                        .to_owned()
                }
                voice::ModelState::Failed { message } => format!("dictation: {message}"),
            };
            cx.core(CoreAction::Notice(reason));
            None
        })
    }

    /// Stop the running dictation, keeping the text. `send`: ⏎ during the
    /// "send it" settle — "send now" — keeps the submit; any other key drops it.
    pub fn stop_dictation(&mut self, send: bool) -> Option<Request> {
        let d = self
            .dictation
            .as_mut()
            .filter(|d| d.phase == Phase::Dictating)?;
        d.phase = Phase::Stopping;
        d.send &= send;
        Some(Request::DictationStop(d.id))
    }

    /// `y` / `n` / `N` on the suggestion line (153 §6.4); Esc is "not now".
    pub fn answer_suggestion(&mut self, answer: voice::SuggestionAnswer) -> Option<Request> {
        let s = self.suggestion.take()?;
        Some(Request::SuggestionAnswer { id: s.id, answer })
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
    fn discard_dictation(&mut self, cx: &mut Cx<'_, '_, Msg>) -> Option<Request> {
        let d = self.dictation.as_mut()?;
        d.discard = true;
        d.phase = Phase::Stopping;
        cx.core(CoreAction::ReplaceEditor { value: format!("{}{}", d.before, d.after), caret: d.before.len() });
        Some(Request::DictationStop(d.id))
    }

}
/// `/voice [mic <name|default> | shape <full|plain|off>]` (153 §8.1, §4.5).
pub(crate) fn voice_cmd(arg: &str) -> Result<Request, &'static str> {
    const USAGE: &str = "usage: /voice [mic <name|default> | shape <full|plain|off>]";
    match arg
        .split_once(char::is_whitespace)
        .map_or((arg, ""), |(c, r)| (c, r.trim()))
    {
        ("", _) => Ok(Request::VoiceStatus),
        ("mic", "default") => Ok(Request::SetInputDevices(Vec::new())),
        ("mic", name) if !name.is_empty() => Ok(Request::SetInputDevices(vec![name.to_owned()])),
        ("shape", "full") => Ok(Request::SetShape(voice::Preset::Full)),
        ("shape", "plain") => Ok(Request::SetShape(voice::Preset::Plain)),
        ("shape", "off") => Ok(Request::SetShape(voice::Preset::Off)),
        _ => Err(USAGE),
    }
}

/// The `/voice` reply (153 §8.1, §8.7, §4.5): the mic preference and what the host
/// enumerates, the running dictation's device, the shaping preset per surface, the
/// last dictation's timings.
pub(crate) fn voice_line(
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

impl Component for DictationVoice {
    type Msg = Msg;
    fn declaration(&self) -> Declaration {
        Declaration { id: "dictation-voice", interests: &[Interest { topic: "voice.events", version: 1, variants: &[] }],
            slash: &[("/dictate", "", "toggle dictation (Ctrl+D)"), ("/voice", "[mic <name|default> | shape <full|plain|off>]", "dictation: mics and last timings; pick the mic; the TUI's shaping preset")],
            bindings: &[Binding { class: FocusClass::Spoken, code: KeyCode::Char('d'), modifiers: KeyModifiers::CONTROL }],
        }
    }
    fn decode(&self, event: &TopicEvent, _: &CoreView<'_>) -> Option<Msg> {
        if event.topic().is_some() && (event.topic() != Some("voice.events") || event.version() != Some(1)) { return None; }
        let ev = voice::VoiceEvent::deserialize(event.event()).ok()?;
        if matches!(ev, voice::VoiceEvent::SpeechFinished { .. }) { return None; }
        Some(Msg::Voice(ev))
    }
    fn update(&mut self, input: Input<'_, Msg>, cx: &mut Cx<'_, '_, Msg>) {
        if cx.core.reasoning && matches!(&input, Input::Message(Msg::Voice(_) | Msg::Starting(_) | Msg::Failed(_) | Msg::Info(Ok(_)))) {
            cx.plain(Plain::Text("\x1b[0m\n".into()));
        }
        match input {
            Input::Lifecycle(Lifecycle::Mount) if cx.core.interactive => send(cx, Request::DictationPrewarm),
            Input::Lifecycle(Lifecycle::SessionReset) => { self.dictated = None; self.suggestion = None; }
            Input::Lifecycle(Lifecycle::ComposerEdited("")) => self.dictated = None,
            Input::Lifecycle(Lifecycle::ComposerSubmitted(text)) => {
                if let Some(id) = self.dictated.take() { send(cx, Request::DictationCorrection { id, text: text.to_owned() }); }
            }
            Input::Message(Msg::Toggle { before, after }) => {
                if let Some(request) = self.toggle_dictation(before, after, cx) { send(cx, request); }
            }
            Input::Message(Msg::Discard) => { if let Some(request) = self.discard_dictation(cx) { send(cx, request); } }
            Input::Message(Msg::Voice(event)) => self.apply_voice(event, cx),
            Input::Message(Msg::Starting(id)) => {
                let (before, after) = self.dictation_pending.take().unwrap_or_default();
                self.dictation = Some(Dictating::new(id, before, after));
            }
            Input::Message(Msg::Failed(message)) => {
                self.dictation_pending = None; self.dictation = None;
                cx.plain(Plain::Text(format!("dictation: {message}\n")));
                cx.core(CoreAction::Reject(format!("dictation: {message}")));
            }
            Input::Message(Msg::Info(Ok(text))) => { cx.plain(Plain::Text(format!("{text}\n"))); cx.core(CoreAction::Notice(text)); }
            Input::Message(Msg::Done(Err(error)) | Msg::Info(Err(error))) => cx.error(error),
            Input::Slash { name: "/dictate", .. } => self.update(Input::Message(Msg::Toggle { before: String::new(), after: String::new() }), cx),
            Input::Slash { name: "/voice", args } => match voice_cmd(args) { Ok(request) => send(cx, request), Err(error) => cx.core(CoreAction::Reject(error.into())) },
            Input::Tick { now_ms, .. } => self.expire_suggestion(now_ms),
            Input::Key(key) => {
                let ctrl_d = key.code == KeyCode::Char('d') && key.modifiers.contains(KeyModifiers::CONTROL);
                if let Some(d) = &self.dictation {
                    let armed = d.send;
                    let request = match key.code {
                        _ if ctrl_d => self.toggle_dictation(String::new(), String::new(), cx),
                        KeyCode::Esc => { self.update(Input::Message(Msg::Discard), cx); None },
                        KeyCode::Enter if armed => self.stop_dictation(true),
                        KeyCode::Char(_) if armed => self.stop_dictation(false),
                        _ => None,
                    };
                    if let Some(request) = request { send(cx, request); }
                    cx.consume_key();
                } else if ctrl_d && cx.core.input_focused && !cx.core.menu_open && !cx.core.secret_editor {
                    let (value, at) = cx.core.editor.unwrap_or(("", 0));
                    let at = if value.is_char_boundary(at) { at } else { value.len() };
                    self.update(Input::Message(Msg::Toggle { before: value[..at].to_owned(), after: value[at..].to_owned() }), cx);
                    cx.consume_key();
                } else if self.suggestion.is_some() {
                    use voice::SuggestionAnswer::*;
                    let answer = match key.code { KeyCode::Char('y') => Some(Add), KeyCode::Char('n') | KeyCode::Esc => Some(NotNow), KeyCode::Char('N') => Some(Never), KeyCode::Char('p') => { cx.core(CoreAction::Notice("play what you said: in the desktop's suggestion card".into())); None }, _ => None };
                    if let Some(answer) = answer && let Some(request) = self.answer_suggestion(answer) { send(cx, request); }
                    cx.consume_key();
                }
            }
            _ => {}
        }
    }
    fn view(&self, at: Surface<'_>, cx: &ViewCx<'_>) -> Option<View> {
        match at {
            Surface::ComposerHeader => self.dictation.as_ref().map(|d| View::Element {
                element: element!(MixedText(contents: dictation_header(d, cx.frame), wrap: TextWrap::NoWrap)).into_any(), rows: 1, replace: true,
            }),
            Surface::Composer => self.dictation.as_ref().map(|d| {
                let contents = dictation_region(d);
                let (text, _) = d.splice();
                let rows = text.split('\n').map(|line| unicode_width::UnicodeWidthStr::width(line).max(1).div_ceil(cx.width.saturating_sub(2).max(1))).sum();
                View::Element { element: element!(MixedText(contents, wrap: TextWrap::Wrap)).into_any(), rows, replace: true }
            }),
            Surface::Hint => self.suggestion.as_ref().map(|s| View::Element {
                element: element!(IView(height: 1, padding_left: 1) { Text(content: s.line(), color: OCHRE, wrap: TextWrap::NoWrap) }).into_any(), rows: 1, replace: false,
            }),
            _ => None,
        }
    }
    fn captures(&self, class: FocusClass) -> bool { match class { FocusClass::Spoken => self.dictation.is_some(), FocusClass::Suggestion => self.suggestion.is_some(), _ => false } }
    fn animating(&self) -> bool { self.dictation.is_some() }
    fn wake(&self) -> Option<u64> { self.suggestion_wake() }
    fn composer_mode(&self) -> ComposerMode { if self.dictation.is_some() { ComposerMode::ReadOnly } else { ComposerMode::Default } }
}

/// Header row while dictating (54 §6): a blinking `●`, the level meter in
/// cyan, the elapsed clock, the mic note (153 §8.2) and the keys in grey.
/// An armed "send it" (153 §3.1) names the settle and its two keys.
fn dictation_header(d: &Dictating, frame: usize) -> Vec<MixedTextContent> {
    let dot = if (frame / 30).is_multiple_of(2) {
        "● "
    } else {
        "  "
    };
    let state = match d.phase {
        Phase::Starting => "starting",
        Phase::Dictating if d.send => "sending…",
        Phase::Dictating => "dictating",
        Phase::Stopping => "stopping",
    };
    let keys = if d.send {
        " (⏎ send now · any key keeps editing · Esc discard)"
    } else {
        " (Ctrl+D stop · Esc discard)"
    };
    let mut out = vec![
        MixedTextContent::new(format!("{dot}{state}  ")).color(FAIENCE),
        MixedTextContent::new(dictation::meter(&d.levels, dictation::METER_CELLS)).color(FAIENCE),
        MixedTextContent::new(format!("  {}", d.clock())),
    ];
    if let Some(mic) = &d.mic {
        out.push(MixedTextContent::new(format!("  {mic}")).color(OCHRE));
    }
    out.push(MixedTextContent::new(keys).color(SAND));
    out
}

/// The composer while dictating (54 §5.1): `before`, `lead`, each utterance
/// (grey until the VAD closes it), `trail`, `after`. Empty utterances are
/// skipped, so the joins mirror `splice`.
fn dictation_region(d: &Dictating) -> Vec<MixedTextContent> {
    let mut out = vec![MixedTextContent::new(&d.before)];
    let mut first = true;
    for u in d.utts.iter().filter(|u| !u.text.is_empty()) {
        out.push(MixedTextContent::new(if first { d.lead() } else { " " }));
        first = false;
        let mut c = MixedTextContent::new(&u.text);
        if !u.is_final {
            c = c.color(SAND);
        }
        out.push(c);
    }
    if !first {
        out.push(MixedTextContent::new(d.trail()));
    }
    out.push(MixedTextContent::new(&d.after));
    if d.before.is_empty() && d.after.is_empty() && first {
        // An empty box still needs a row for the border to wrap.
        out.push(MixedTextContent::new(" "));
    }
    out
}


#[test]
fn active_dictation_precedes_approval_but_idle_shortcut_obeys_editor_predicates() {
    let id = voice::DictationId(uuid::Uuid::new_v4());
    let mut registry = Registry::empty();
    registry.register(DictationVoice {
        dictation_model: voice::ModelState::Ready,
        dictation: Some(Dictating::new(id, "before".into(), "after".into())),
        ..Default::default()
    }).unwrap();
    registry.get_mut::<DictationVoice>().unwrap().dictation.as_mut().unwrap().phase = Phase::Dictating;
    let core = CoreView { approval: true, input_focused: false, ..tests::core() };
    let key = KeyEvent::new(iocraft::KeyEventKind::Press, KeyCode::Esc);
    let out = registry.key(FocusClass::Spoken, &key, &core);
    assert!(out.consumed());
    assert!(matches!(out.effects[0].request::<Request>(), Some(Request::DictationStop(stopped)) if *stopped == id));
    assert!(matches!(&out.actions[0], CoreAction::ReplaceEditor { value, caret: 6 } if value == "beforeafter"));
    registry.get_mut::<DictationVoice>().unwrap().dictation = None;
    let mut key = KeyEvent::new(iocraft::KeyEventKind::Press, KeyCode::Char('d'));
    key.modifiers = KeyModifiers::CONTROL;
    for core in [
        CoreView { approval: true, input_focused: false, ..tests::core() },
        CoreView { menu_open: true, ..tests::core() },
        CoreView { secret_editor: true, ..tests::core() },
    ] {
        let out = registry.key(FocusClass::Spoken, &key, &core);
        assert!(!out.consumed() && out.effects.is_empty());
        assert!(registry.get::<DictationVoice>().unwrap().dictation_pending.is_none());
    }
    key.modifiers = KeyModifiers::CONTROL | KeyModifiers::ALT;
    let core = CoreView { editor: Some(("αβ", "α".len())), ..tests::core() };
    let out = registry.key(FocusClass::Spoken, &key, &core);
    assert!(out.consumed());
    assert_eq!(registry.get::<DictationVoice>().unwrap().dictation_pending, Some(("α".into(), "β".into())));
}

