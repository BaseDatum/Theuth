//! The `/talk` view (design doc 76 §0.12, §5.2): the per-talk state, the
//! subtitle reducer and the orb frames. Pure; the keys and the render live
//! in `ui.rs`, the events in `model.rs`, the scripted source in `lib.rs`.

use runtime::SessionId;
use talk::{EndReason, State, TalkEvent, TalkId};
use voice::{SpeechId, VoiceEvent};

use crate::model::UiEvent;

/// Three looks for six states: the orb *is* the state (76 §0.12).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    /// Slow breathing ring (`Idle`, `Listening`, `Interrupted`).
    Idle,
    /// Rotating, pulsing inner texture (`Thinking`, `Working`).
    Thinking,
    /// Radiating rings driven by the voice level (`Speaking`, `Holding` frozen).
    Speaking,
}

/// Orb look for a talk state.
pub fn look(state: State) -> Look {
    match state {
        State::Idle | State::Listening | State::Interrupted => Look::Idle,
        State::Thinking | State::Working => Look::Thinking,
        State::Speaking | State::Holding => Look::Speaking,
    }
}

/// Caption under the subtitles once `Degraded{MicNoAec}` arrived (76 §0.5).
pub const DEGRADED: &str = "no echo cancellation; headphones recommended";

/// The talk this view started, as the bus reports it.
#[derive(Clone, Debug)]
pub struct Talking {
    /// Talk.
    pub id: TalkId,
    /// Its session.
    pub session: SessionId,
    /// Last `TalkState`.
    pub state: State,
    /// Peak of the last `TalkLevel`, 0–1; the speaking rings reach further with it.
    pub level: f32,
    /// The agent sentence being heard (`Saying`); cleared by its
    /// `SpeechFinished` or by `Interrupted`.
    pub agent: Option<(SpeechId, String)>,
    /// The user's utterance so far (`Heard`, partial then final); cleared on `Submitted`.
    pub user: Option<String>,
    /// The headphones caption is on.
    pub degraded: bool,
    /// Animation frame at which `Holding` began: the speaking look freezes there.
    held_at: Option<usize>,
}

impl Talking {
    /// Fresh, nothing heard yet.
    pub fn new(id: TalkId, session: SessionId) -> Self {
        Self {
            id,
            session,
            state: State::Idle,
            level: 0.0,
            agent: None,
            user: None,
            degraded: false,
            held_at: None,
        }
    }

    /// Fold one talk event in at animation `frame`; other ids are ignored.
    /// `Some(reason)` when this talk ended.
    pub fn apply(&mut self, ev: TalkEvent, frame: usize) -> Option<EndReason> {
        if event_id(&ev) != self.id {
            return None;
        }
        match ev {
            TalkEvent::TalkState { state, .. } => {
                self.held_at = (state == State::Holding).then_some(frame);
                self.state = state;
            }
            TalkEvent::TalkLevel { rms, .. } => {
                self.level = rms.iter().copied().fold(0.0, f32::max).clamp(0.0, 1.0);
            }
            TalkEvent::Heard { text, .. } => self.user = Some(text),
            TalkEvent::Saying { speech, text, .. } => self.agent = Some((speech, text)),
            TalkEvent::Submitted { .. } => self.user = None,
            TalkEvent::Interrupted { .. } => self.agent = None,
            TalkEvent::Degraded { .. } => self.degraded = true,
            TalkEvent::TalkFinished { reason, .. } => return Some(reason),
            TalkEvent::TalkStarted { .. }
            | TalkEvent::Holding { .. }
            | TalkEvent::Resumed { .. }
            | TalkEvent::Flush { .. }
            // 145 §0.6: the sentence reaches the agent line through its `Saying`.
            | TalkEvent::Announced { .. } => {}
        }
        None
    }

    /// `SpeechFinished` for `speech`: the agent line is done.
    pub fn speech_finished(&mut self, speech: SpeechId) {
        if self.agent.as_ref().is_some_and(|(id, _)| *id == speech) {
            self.agent = None;
        }
    }

    /// The frame the orb draws at `frame`: frozen while `Holding`.
    pub fn orb_frame(&self, frame: usize) -> usize {
        self.held_at.unwrap_or(frame)
    }
}

/// The talk an event is about.
pub fn event_id(ev: &TalkEvent) -> TalkId {
    match ev {
        TalkEvent::TalkStarted { id, .. }
        | TalkEvent::TalkState { id, .. }
        | TalkEvent::TalkLevel { id, .. }
        | TalkEvent::Heard { id, .. }
        | TalkEvent::Saying { id, .. }
        | TalkEvent::Submitted { id, .. }
        | TalkEvent::Holding { id, .. }
        | TalkEvent::Resumed { id, .. }
        | TalkEvent::Interrupted { id, .. }
        | TalkEvent::Flush { id, .. }
        | TalkEvent::Degraded { id, .. }
        | TalkEvent::Announced { id, .. }
        | TalkEvent::TalkFinished { id, .. } => *id,
    }
}

/// Tallest orb: `2 * MAX_RADIUS + 1` rows.
pub const MAX_RADIUS: usize = 11;

/// Orb radius for a pane of `width × height` cells: the subtitles keep
/// `reserve` rows, a cell is about twice as tall as wide, never taller than
/// [`MAX_RADIUS`], never smaller than a dot.
pub fn radius(width: usize, height: usize, reserve: usize) -> usize {
    let by_height = height.saturating_sub(reserve).saturating_sub(1) / 2;
    let by_width = width.saturating_sub(1) / 4;
    by_height.min(by_width).clamp(1, MAX_RADIUS)
}

/// The orb at animation `frame` (~60 Hz): `2r+1` rows of exactly `4r+1`
/// cells each, so the caller centres the block, not the glyphs.
/// ponytail: computed per frame (≤ 23×45 cells, a few thousand float ops);
/// precompute per radius if a profile ever shows it.
pub fn orb(look: Look, frame: usize, level: f32, r: usize) -> Vec<String> {
    let t = frame as f32 / 60.0;
    let rf = r as f32;
    let ri = r as i32;
    (-ri..=ri)
        .map(|y| {
            (-2 * ri..=2 * ri)
                .map(|x| {
                    let (fx, fy) = (x as f32 / 2.0, y as f32);
                    let d = (fx * fx + fy * fy).sqrt() / rf;
                    match look {
                        Look::Idle => idle(d, t, rf),
                        Look::Thinking => thinking(d, fy.atan2(fx), t),
                        Look::Speaking => speaking(d, t, level),
                    }
                })
                .collect()
        })
        .collect()
}

/// A ring breathing between 0.62 and 0.86 of the radius every four seconds;
/// the band is at least half a cell wide so a tiny orb still shows.
fn idle(d: f32, t: f32, rf: f32) -> char {
    let ring = 0.74 + 0.12 * (t * std::f32::consts::TAU / 4.0).sin();
    match (d - ring).abs() {
        e if e < f32::max(0.06, 0.3 / rf) => '•',
        e if e < f32::max(0.13, 0.6 / rf) => '·',
        _ => ' ',
    }
}

/// A disc whose three-armed texture turns and pulses, edged with a faint ring.
fn thinking(d: f32, angle: f32, t: f32) -> char {
    if d > 0.92 {
        return ' ';
    }
    if d > 0.82 {
        return '·';
    }
    let v = (3.0 * angle + t * 2.0).sin() * (d * 7.0 - t * 3.0).cos();
    match v {
        v if v > 0.45 => '▓',
        v if v > 0.0 => '▒',
        v if v > -0.5 => '░',
        _ => ' ',
    }
}

/// A core plus three rings running outward; the level stretches how far they
/// go and how big the core is, so the orb moves with the voice.
fn speaking(d: f32, t: f32, level: f32) -> char {
    let reach = 0.55 + 0.45 * level;
    if d < 0.12 + 0.18 * level {
        return '●';
    }
    let phase = (t * 1.5).fract();
    for k in 0..3 {
        let ring = ((phase + k as f32 / 3.0).fract()) * reach;
        if (d - ring).abs() < 0.07 {
            return if ring < reach * 0.5 { '●' } else { '·' };
        }
    }
    ' '
}

/// A scripted talk for development without the engine (`THEUTH_TALK_SIM`):
/// `(delay ms before it, event)`, three rounds of listen → think → speak →
/// hold → interrupt → speak, Degraded on. The finish comes from `TalkStop`.
pub fn sim(id: TalkId, session: SessionId) -> Vec<(u64, UiEvent)> {
    let talk = |ev: TalkEvent| UiEvent::Talk(ev);
    let state = |s: State| talk(TalkEvent::TalkState { id, state: s });
    let heard = |text: &str, is_final: bool| {
        talk(TalkEvent::Heard {
            id,
            utterance: 0,
            text: text.into(),
            is_final,
        })
    };
    let saying = |speech: SpeechId, text: &str| {
        talk(TalkEvent::Saying {
            id,
            speech,
            text: text.into(),
        })
    };
    let finished = |speech: SpeechId| {
        UiEvent::Voice(VoiceEvent::SpeechFinished {
            id: speech,
            session,
            reason: voice::FinishReason::Done,
            sink: voice::Sink::Device,
        })
    };
    // `seconds` of `TalkLevel` events, one per 160 ms, a slow swell.
    let levels = |out: &mut Vec<(u64, UiEvent)>, seconds: u64| {
        for i in 0..seconds * 6 {
            let base = (i as f32 * 0.7).sin().abs() * 0.7 + 0.15;
            let rms =
                std::array::from_fn(|k| (base + (k as f32 * 0.9).sin() * 0.1).clamp(0.0, 1.0));
            out.push((160, talk(TalkEvent::TalkLevel { id, rms })));
        }
    };
    let mut out = vec![
        (
            0,
            talk(TalkEvent::TalkStarted {
                id,
                session,
                sink: talk::Sink::Device,
                aec: talk::Aec::None,
                since_ms: 0,
            }),
        ),
        (
            0,
            talk(TalkEvent::Degraded {
                id,
                degradation: talk::TalkDegradation::MicNoAec,
            }),
        ),
        (0, state(State::Listening)),
    ];
    for _ in 0..3 {
        let (s1, s2, s3) = (SpeechId::new(), SpeechId::new(), SpeechId::new());
        out.push((2000, heard("what's", false)));
        out.push((400, heard("what's the weather", false)));
        out.push((500, heard("What's the weather like in Berlin today?", true)));
        out.push((
            800,
            talk(TalkEvent::Submitted {
                id,
                text: "[spoken, 2 s] What's the weather like in Berlin today?".into(),
                interrupted: false,
            }),
        ));
        out.push((0, state(State::Thinking)));
        out.push((2500, state(State::Speaking)));
        out.push((
            0,
            saying(
                s1,
                "Berlin is mild and cloudy today, around fourteen degrees.",
            ),
        ));
        levels(&mut out, 3);
        out.push((0, finished(s1)));
        out.push((
            0,
            saying(
                s2,
                "There is a chance of light rain in the evening, so take a jacket if you are out late.",
            ),
        ));
        levels(&mut out, 1);
        out.push((0, heard("actually", false)));
        out.push((
            400,
            talk(TalkEvent::Holding {
                id,
                since_speech_ms: 4000,
            }),
        ));
        out.push((0, state(State::Holding)));
        out.push((900, heard("actually what about", false)));
        out.push((
            0,
            talk(TalkEvent::Interrupted {
                id,
                heard_sentences: 1,
                dropped_sentences: 1,
            }),
        ));
        out.push((0, state(State::Interrupted)));
        out.push((0, finished(s2)));
        out.push((600, heard("Actually, what about tomorrow?", true)));
        out.push((
            700,
            talk(TalkEvent::Submitted {
                id,
                text: "[spoken, 2 s, interrupted] Actually, what about tomorrow?".into(),
                interrupted: true,
            }),
        ));
        out.push((0, state(State::Thinking)));
        out.push((1500, state(State::Speaking)));
        out.push((
            0,
            saying(s3, "Tomorrow looks sunnier, with a high of eighteen."),
        ));
        levels(&mut out, 2);
        out.push((0, finished(s3)));
        out.push((0, state(State::Listening)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use talk::{ResumeReason, Sink, TalkDegradation};
    use uuid::Uuid;

    fn talking() -> Talking {
        Talking::new(TalkId(Uuid::now_v7()), SessionId::new())
    }

    #[test]
    fn seven_states_three_looks() {
        for (state, want) in [
            (State::Idle, Look::Idle),
            (State::Listening, Look::Idle),
            (State::Interrupted, Look::Idle),
            (State::Thinking, Look::Thinking),
            (State::Working, Look::Thinking),
            (State::Speaking, Look::Speaking),
            (State::Holding, Look::Speaking),
        ] {
            assert_eq!(look(state), want, "{state:?}");
        }
    }

    /// Partial → final → cleared on `Submitted`; the agent line clears on its
    /// own `SpeechFinished` (not another's) and on `Interrupted`; a stranger's
    /// events change nothing; `Holding` freezes the orb frame.
    #[test]
    fn subtitle_reducer() {
        let mut t = talking();
        let id = t.id;
        let other = TalkId(Uuid::now_v7());
        assert!(
            t.apply(
                TalkEvent::Heard {
                    id: other,
                    utterance: 0,
                    text: "not ours".into(),
                    is_final: false,
                },
                0,
            )
            .is_none()
        );
        assert_eq!(t.user, None);
        for (text, is_final) in [
            ("what", false),
            ("what time", false),
            ("What time is it?", true),
        ] {
            t.apply(
                TalkEvent::Heard {
                    id,
                    utterance: 0,
                    text: text.into(),
                    is_final,
                },
                0,
            );
            assert_eq!(t.user.as_deref(), Some(text));
        }
        t.apply(
            TalkEvent::Submitted {
                id,
                text: "[spoken, 2 s] What time is it?".into(),
                interrupted: false,
            },
            0,
        );
        assert_eq!(t.user, None);

        let (s1, s2) = (SpeechId::new(), SpeechId::new());
        t.apply(
            TalkEvent::Saying {
                id,
                speech: s1,
                text: "It is noon.".into(),
            },
            0,
        );
        t.speech_finished(s2);
        assert_eq!(
            t.agent.as_ref().map(|(_, s)| s.as_str()),
            Some("It is noon.")
        );
        t.speech_finished(s1);
        assert_eq!(t.agent, None);
        t.apply(
            TalkEvent::Saying {
                id,
                speech: s2,
                text: "Almost.".into(),
            },
            0,
        );
        t.apply(
            TalkEvent::Interrupted {
                id,
                heard_sentences: 1,
                dropped_sentences: 2,
            },
            0,
        );
        assert_eq!(t.agent, None);

        assert!(!t.degraded);
        t.apply(
            TalkEvent::Degraded {
                id,
                degradation: TalkDegradation::MicNoAec,
            },
            0,
        );
        assert!(t.degraded);

        t.apply(
            TalkEvent::TalkLevel {
                id,
                rms: [0.1, 0.9, 0.2, 0.0, 0.0, 0.0, 0.0, 0.0],
            },
            0,
        );
        assert_eq!(t.level, 0.9);

        t.apply(
            TalkEvent::TalkState {
                id,
                state: State::Speaking,
            },
            10,
        );
        assert_eq!(t.orb_frame(50), 50);
        t.apply(
            TalkEvent::TalkState {
                id,
                state: State::Holding,
            },
            60,
        );
        assert_eq!(t.orb_frame(90), 60);
        t.apply(
            TalkEvent::Resumed {
                id,
                reason: ResumeReason::Backchannel,
            },
            91,
        );
        t.apply(
            TalkEvent::TalkState {
                id,
                state: State::Speaking,
            },
            92,
        );
        assert_eq!(t.orb_frame(100), 100);

        assert!(
            t.apply(
                TalkEvent::TalkStarted {
                    id,
                    session: t.session,
                    sink: Sink::Device,
                    aec: talk::Aec::Vpio,
                    since_ms: 0,
                },
                0,
            )
            .is_none()
        );
        assert!(matches!(
            t.apply(
                TalkEvent::TalkFinished {
                    id,
                    reason: EndReason::Stopped,
                },
                0,
            ),
            Some(EndReason::Stopped)
        ));
    }

    /// Every look at every radius is a `2r+1 × 4r+1` block with something in
    /// it; the speaking orb grows with the level; the idle ring breathes.
    #[test]
    fn orb_frames_are_rectangular_and_alive() {
        for r in 1..=MAX_RADIUS {
            for look in [Look::Idle, Look::Thinking, Look::Speaking] {
                for frame in [0, 37, 120, 233] {
                    let rows = orb(look, frame, 0.5, r);
                    assert_eq!(rows.len(), 2 * r + 1, "{look:?} r={r}");
                    assert!(
                        rows.iter().all(|row| row.chars().count() == 4 * r + 1),
                        "{look:?} r={r} frame={frame}"
                    );
                    assert!(
                        rows.iter().any(|row| !row.trim().is_empty()),
                        "{look:?} r={r} frame={frame}: blank"
                    );
                }
            }
        }
        let ink = |rows: Vec<String>| {
            rows.iter()
                .flat_map(|r| r.chars())
                .filter(|c| *c != ' ')
                .count()
        };
        assert!(ink(orb(Look::Speaking, 20, 1.0, 8)) > ink(orb(Look::Speaking, 20, 0.0, 8)));
        assert_ne!(orb(Look::Idle, 0, 0.0, 8), orb(Look::Idle, 60, 0.0, 8));
        assert_ne!(
            orb(Look::Thinking, 0, 0.0, 8),
            orb(Look::Thinking, 30, 0.0, 8)
        );
    }

    #[test]
    fn radius_fits_the_pane() {
        assert_eq!(radius(80, 24, 7), 8);
        assert_eq!(radius(120, 40, 7), MAX_RADIUS);
        assert_eq!(radius(20, 5, 7), 1);
    }
}
