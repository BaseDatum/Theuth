//! Talk lifecycle, spoken input, local simulation and orb presentation.
use super::*;
use std::collections::VecDeque;
use serde::Deserialize;
use rpc_compose::api::clients::*;
use ::talk::{EndReason, State, TalkEvent, TalkId};
use voice::VoiceEvent;
use crate::model::now_ms;
use crate::ui::{OCHRE, SAND};
use iocraft::prelude::{View as IView, element, Text, TextWrap, TextAlign, FlexDirection, AlignItems, JustifyContent, Overflow};
pub(crate) mod state;
use state::{Look, Talking};

#[derive(Default)]
pub(crate) struct Talk {
    pub(crate) talk: Option<Talking>,
    pub(crate) talk_view: bool,
    pub(crate) talk_pending: bool,
    simulation: VecDeque<(u64, Msg)>,
}
#[derive(Clone, Debug)]
#[expect(clippy::large_enum_variant, reason = "The draft moves into an already boxed effect.")]
pub(crate) enum Request {
    TalkStart { session: Option<SessionId>, draft: Option<agent::CreateSessionOptions> },
    TalkStop(TalkId),
}
pub(crate) enum Msg {
    Talk(TalkEvent), Voice(VoiceEvent), Starting(TalkId), Failed(String),
    Created(agent::SessionOpen), Start(SessionId), SimStarting(TalkId, SessionId),
    Done(Result<(), String>), Toggle(bool), Escape,
}
fn simulated() -> bool { std::env::var_os("THEUTH_TALK_SIM").is_some() }
fn send(cx: &mut Cx<'_, '_, Msg>, request: Request) {
    cx.spawn_request(Lane::Concurrent, request, |api, request| async move {
        match request {
            Request::TalkStart { session: None, draft } => match api.create_session(draft.unwrap_or_default()).await {
                Ok(open) => Msg::Created(open), Err(error) => Msg::Failed(crate::reason(error)),
            },
            Request::TalkStart { session: Some(session), .. } if simulated() => Msg::SimStarting(TalkId::new(), session),
            Request::TalkStart { session: Some(session), .. } => match api.talk_start(session, ::talk::Sink::Device, None).await {
                Ok(id) => Msg::Starting(id), Err(error) => Msg::Failed(crate::reason(error)),
            },
            Request::TalkStop(id) if simulated() => Msg::Talk(TalkEvent::TalkFinished { id, reason: EndReason::Stopped }),
            Request::TalkStop(id) => Msg::Done(api.talk_stop(id).await.map_err(crate::reason)),
        }
    });
}
impl Talk {
    fn toggle(&mut self, stop: bool, cx: &mut Cx<'_, '_, Msg>) {
        if let Some(t) = &self.talk {
            if self.talk_view || stop {
                self.talk_view = false;
                send(cx, Request::TalkStop(t.id));
            } else { self.talk_view = true; }
        } else if !self.talk_pending && !(cx.core.session.is_none() && cx.core.awaiting_session) {
            self.talk_pending = true;
            send(cx, Request::TalkStart { session: cx.core.session, draft: cx.core.draft_options.cloned() });
        }
    }
    fn escape(&mut self, cx: &mut Cx<'_, '_, Msg>) {
        match &self.talk {
            Some(t) if matches!(t.state, State::Thinking | State::Working | State::Speaking | State::Holding) => cx.core(CoreAction::CancelSession(t.session)),
            _ => self.talk_view = false,
        }
    }
}
impl Component for Talk {
    type Msg = Msg;
    fn declaration(&self) -> Declaration {
        Declaration { id: "talk", interests: &[
            Interest { topic: "talk.events", version: 1, variants: &[] },
            Interest { topic: "voice.events", version: 1, variants: &["SpeechFinished"] },
        ], slash: &[("/talk", "", "talk with the session aloud; Esc cancels the reply or leaves the view, Ctrl+T stops")],
            bindings: &[Binding { class: FocusClass::Spoken, code: KeyCode::Char('t'), modifiers: KeyModifiers::CONTROL }],
        }
    }
    fn decode(&self, event: &TopicEvent, _: &CoreView<'_>) -> Option<Msg> {
        match event.topic() {
            Some("talk.events") if event.version() == Some(1) => TalkEvent::deserialize(event.event()).ok().map(Msg::Talk),
            Some("voice.events") if event.version() == Some(1) && event.event()["type"] == "SpeechFinished" => VoiceEvent::deserialize(event.event()).ok().map(Msg::Voice),
            None => TalkEvent::deserialize(event.event()).ok().map(Msg::Talk).or_else(|| {
                if event.event()["type"] == "SpeechFinished" { VoiceEvent::deserialize(event.event()).ok().map(Msg::Voice) } else { None }
            }),
            _ => None,
        }
    }
    fn update(&mut self, input: Input<'_, Msg>, cx: &mut Cx<'_, '_, Msg>) {
        if cx.core.reasoning && matches!(&input, Input::Message(Msg::Talk(_) | Msg::Voice(_) | Msg::Starting(_) | Msg::SimStarting(..) | Msg::Failed(_))) {
            cx.plain(Plain::Text("\x1b[0m\n".into()));
        }
        match input {
            Input::Lifecycle(Lifecycle::SessionReset) => *self = Self::default(),
            Input::Message(Msg::Created(open)) => {
                let session = open.session;
                cx.core(CoreAction::SessionOpened(open));
                cx.post(Msg::Start(session));
            }
            Input::Message(Msg::Start(session)) => send(cx, Request::TalkStart { session: Some(session), draft: None }),
            Input::Message(Msg::Starting(id)) => {
                self.talk_pending = false;
                if let Some(session) = cx.core.session { self.talk = Some(Talking::new(id, session)); self.talk_view = true; }
            }
            Input::Message(Msg::SimStarting(id, session)) => {
                self.update(Input::Message(Msg::Starting(id)), cx);
                let mut at = now_ms();
                self.simulation = state::sim(id, session).into();
                for (delay, _) in &mut self.simulation { at += *delay; *delay = at; }
            }
            Input::Message(Msg::Talk(event)) => {
                if let Some(t) = &mut self.talk && let Some(reason) = t.apply(event, cx.core.frame) {
                    self.talk = None; self.talk_view = false; self.simulation.clear();
                    if let EndReason::Error { message } = reason { cx.core(CoreAction::Reject(format!("talk: {message}"))); }
                }
            }
            Input::Message(Msg::Voice(VoiceEvent::SpeechFinished { id, .. })) => { if let Some(t) = &mut self.talk { t.speech_finished(id); } }
            Input::Message(Msg::Failed(message)) => {
                self.talk_pending = false;
                cx.plain(Plain::Text(format!("talk: {message}\n")));
                cx.core(CoreAction::Reject(format!("talk: {message}")));
            }
            Input::Message(Msg::Done(Err(error))) => cx.error(error),
            Input::Message(Msg::Toggle(stop)) => self.toggle(stop, cx),
            Input::Message(Msg::Escape) => self.escape(cx),
            Input::Slash { .. } => self.toggle(false, cx),
            Input::Tick { now_ms, .. } => {
                while self.simulation.front().is_some_and(|(at, _)| *at <= now_ms) {
                    let (_, event) = self.simulation.pop_front().unwrap();
                    self.update(Input::Message(event), cx);
                }
            }
            Input::Key(key) => {
                let ctrl_t = key.code == KeyCode::Char('t') && key.modifiers.contains(KeyModifiers::CONTROL);
                if self.talk_view && !cx.core.approval {
                    match key.code { KeyCode::Esc => self.update(Input::Message(Msg::Escape), cx), _ if ctrl_t => self.update(Input::Message(Msg::Toggle(true)), cx), _ => {} }
                    cx.consume_key();
                } else if ctrl_t && !cx.core.composer_read_only && !cx.core.menu_open && !cx.core.secret_editor {
                    self.update(Input::Message(Msg::Toggle(true)), cx);
                    cx.consume_key();
                }
            }
            _ => {}
        }
    }
    fn view(&self, at: Surface<'_>, cx: &ViewCx<'_>) -> Option<View> {
        match at {
            Surface::Content if self.talk_view => self.talk.as_ref().map(|t| View::Element {
                element: talk_pane(t, cx.frame, u16::try_from(cx.width).unwrap_or(u16::MAX), cx.height), rows: cx.height, replace: true,
            }),
            Surface::ComposerHeader if self.talk_view => Some(View::Element {
                element: element!(Text(content: "● talk  (Esc cancels / leaves · Ctrl+T stops)", color: SAND)).into_any(), rows: 1, replace: true,
            }),
            Surface::Status(StatusSlot::Spoken) if !self.talk_view => self.talk.as_ref().map(|t| View::Text(format!("  ·  ● talk: {} (/talk)", format!("{:?}", t.state).to_lowercase()))),
            _ => None,
        }
    }
    fn captures(&self, class: FocusClass) -> bool { class == FocusClass::Spoken && self.talk_view }
    fn composer_mode(&self) -> ComposerMode { if self.talk_view { ComposerMode::Hidden } else { ComposerMode::Default } }
    fn animating(&self) -> bool { self.talk_view }
    fn wake(&self) -> Option<u64> { self.simulation.front().map(|(at, _)| *at) }
}

/// The `/talk` pane (76 §0.12, §5.2) in a `width × height` content area: the
/// orb centred, the agent line (foreground) and the user line (muted) under
/// it, two rows each and tail-clamped, then the headphones caption. Colours
/// are the palette's: accent while speaking, muted while idle, the
/// terminal's own foreground while thinking.
pub(crate) fn talk_pane(t: &Talking, frame: usize, width: u16, height: usize) -> AnyElement<'static> {
    // Two subtitle blocks of two rows, one caption row, two gap rows.
    const RESERVE: usize = 7;
    let look = state::look(t.state);
    let color = match look {
        Look::Idle => Some(SAND),
        Look::Thinking => None,
        Look::Speaking => Some(OCHRE),
    };
    let r = state::radius(usize::from(width), height, RESERVE);
    let rows = state::orb(look, t.orb_frame(frame), t.level, r);
    let agent = t.agent.as_ref().map_or("", |(_, s)| s.as_str()).to_owned();
    let user = t.user.clone().unwrap_or_default();
    let caption = if t.degraded { state::DEGRADED } else { "" };
    element! {
        IView(width: 100pct, height: 100pct, flex_direction: FlexDirection::Column, align_items: AlignItems::Center, justify_content: JustifyContent::Center) {
            IView(flex_direction: FlexDirection::Column) {
                #(rows.into_iter().map(|row| element!(Text(content: row, color, wrap: TextWrap::NoWrap))))
            }
            // Bottom-aligned in a fixed two-row box: a long line clips at its head, the tail stays.
            IView(width: 100pct, height: 2, margin_top: 1, padding_left: 2, padding_right: 2, overflow: Overflow::Hidden, flex_direction: FlexDirection::Column, justify_content: JustifyContent::FlexEnd) {
                Text(content: agent, wrap: TextWrap::Wrap, align: TextAlign::Center)
            }
            IView(width: 100pct, height: 2, margin_top: 1, padding_left: 2, padding_right: 2, overflow: Overflow::Hidden, flex_direction: FlexDirection::Column, justify_content: JustifyContent::FlexEnd) {
                Text(content: user, color: SAND, wrap: TextWrap::Wrap, align: TextAlign::Center)
            }
            IView(height: 1) {
                Text(content: caption, color: SAND, wrap: TextWrap::NoWrap)
            }
        }
    }
    .into_any()
}

#[cfg(test)]
#[test]
fn approval_over_visible_talk_preserves_spoken_precedence_and_matching_speech() {
    use super::dictation_voice::{DictationVoice, state::{Dictating, Phase}};
    for talk_first in [true, false] {
        let session = SessionId::new();
        let id = TalkId::new();
        let speech = voice::SpeechId::new();
        let mut talking = Talking::new(id, session);
        talking.state = State::Speaking;
        talking.agent = Some((speech, "first sentence".into()));
        let owner = Talk { talk: Some(talking), talk_view: true, ..Default::default() };
        let mut registry = Registry::empty();
        if talk_first {
            registry.register(owner).unwrap();
            registry.register(DictationVoice::default()).unwrap();
        } else {
            registry.register(DictationVoice::default()).unwrap();
            registry.register(owner).unwrap();
        }
        let core = CoreView { session: Some(session), approval: true, input_focused: false, frame: 77, ..tests::core() };
        let escape = KeyEvent::new(iocraft::KeyEventKind::Press, KeyCode::Esc);
        let out = registry.key(FocusClass::Spoken, &escape, &core);
        assert!(!out.consumed() && out.actions.is_empty());
        assert!(registry.get::<Talk>().unwrap().talk_view);
        let mut stop = KeyEvent::new(iocraft::KeyEventKind::Press, KeyCode::Char('t'));
        stop.modifiers = KeyModifiers::CONTROL;
        let out = registry.key(FocusClass::Spoken, &stop, &core);
        assert!(out.consumed());
        assert!(matches!(out.effects[0].request::<Request>(), Some(Request::TalkStop(stopped)) if *stopped == id));
        assert!(!registry.get::<Talk>().unwrap().talk_view);
        registry.get_mut::<Talk>().unwrap().talk_view = true;
        let dictation_id = voice::DictationId(uuid::Uuid::new_v4());
        let mut dictating = Dictating::new(dictation_id, "before".into(), "after".into());
        dictating.phase = Phase::Dictating;
        registry.get_mut::<DictationVoice>().unwrap().dictation = Some(dictating);
        let core = CoreView { composer_read_only: true, ..core };
        let out = registry.key(FocusClass::Spoken, &stop, &core);
        assert!(out.consumed() && out.effects.is_empty());
        assert!(registry.get::<Talk>().unwrap().talk_view);
        let core = CoreView { approval: false, ..core };
        let out = registry.key(FocusClass::Spoken, &escape, &core);
        assert!(out.consumed());
        assert!(matches!(&out.actions[0], CoreAction::CancelSession(target) if *target == session));
        assert_eq!(registry.get::<DictationVoice>().unwrap().dictation.as_ref().unwrap().id, dictation_id);
        let finished = |id| serde_json::to_value(VoiceEvent::SpeechFinished { id, session, reason: voice::FinishReason::Done, sink: voice::Sink::Device }).unwrap();
        registry.wire(&tests::topic("voice.events", 1, finished(voice::SpeechId::new())), &core);
        assert_eq!(registry.get::<Talk>().unwrap().talk.as_ref().unwrap().agent.as_ref().map(|(id, _)| *id), Some(speech));
        registry.wire(&TopicEvent::Legacy(Arc::new(finished(speech))), &core);
        assert!(registry.get::<Talk>().unwrap().talk.as_ref().unwrap().agent.is_none());
        registry.wire(&tests::topic("talk.events", 1, serde_json::to_value(TalkEvent::TalkState { id, state: State::Holding }).unwrap()), &core);
        assert_eq!(registry.get::<Talk>().unwrap().talk.as_ref().unwrap().orb_frame(999), 77);
    }
}
