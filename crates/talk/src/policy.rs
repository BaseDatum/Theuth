//! The talk's decisions as a pure function of what happened (76 §4.2,
//! §4.4–§4.8, §0.9): the state machine, the turn policy, the barge-in
//! classifier and the heard-prefix ledger. No clocks, no actors, no devices —
//! the actor feeds [`Policy::step`] every input with the time it arrived and
//! carries out the [`Action`]s it returns, in order.

use std::time::{Duration, Instant};

use voice::SpeechId;
use voice::out::Played;

use crate::{ResumeReason, Sink, State, TalkConfig};

/// The actor's timers; each fires as [`Input::Timer`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Timer {
    /// End of turn after a final (76 §4.7).
    EndOfTurn,
    /// The gate has been open `barge_in_min_ms` (stage 1).
    Hold,
    /// `talk.decide_ms` after onset without a partial (stage 2 fallback).
    Decide,
}

/// What happened.
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    /// The gate opened.
    Onset,
    /// The gate closed.
    Close,
    /// A text piece of utterance `utterance`.
    Text {
        /// Utterance index.
        utterance: u32,
        /// Whole utterance so far.
        text: String,
        /// The utterance closed.
        is_final: bool,
    },
    /// A timer the policy started fired.
    Timer(Timer),
    /// Agent `TextDelta`.
    Delta(String),
    /// Agent `ToolCallPending`: the prose so far is a block of its own.
    BlockEnd,
    /// Agent `TurnStarted { turn_seq }`: a turn opened on the session (ours,
    /// or typed elsewhere).
    TurnStarted(i64),
    /// Agent `TurnFinished { stop_reason }`.
    TurnFinished(String),
    /// Voice `SpeechStarted` for one of the ledger's sentences.
    SpeechStarted(SpeechId),
    /// Voice `SpeechFinished`; `done` = every sample published.
    SpeechFinished {
        /// Utterance.
        speech: SpeechId,
        /// `FinishReason::Done`.
        done: bool,
    },
    /// Voice `SpeechAudio` (client sink): a packet of `speech` was published.
    Packet {
        /// Utterance.
        speech: SpeechId,
        /// Its 80 ms frame index.
        seq: u32,
    },
    /// Client sink: the highest `seq` the client finished playing (76 §4.6).
    PlayedSeq(u32),
}

/// What the actor does.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Publish `TalkState`.
    State(State),
    /// Publish `Heard`.
    Heard {
        /// Utterance index.
        utterance: u32,
        /// Text so far.
        text: String,
        /// Closed.
        is_final: bool,
    },
    /// `Voice::Speak { queue: true }` for ledger sentence `index`; report the id with [`Policy::spoken`].
    Speak {
        /// Ledger index.
        index: usize,
        /// The sentence.
        text: String,
    },
    /// Publish `Saying`.
    Saying {
        /// Utterance.
        speech: SpeechId,
        /// Sentence.
        text: String,
    },
    /// `Agents::SendInput` with `text`; publish `Submitted`.
    Submit {
        /// Marker included.
        text: String,
        /// The marker says `interrupted`.
        interrupted: bool,
    },
    /// `Agents::Cancel { keep }`.
    Cancel(Option<String>),
    /// `Agents::TrimAssistant { turn, keep }`: the turn had already finished
    /// when the user cut in (76 §7, the post-turn case).
    Trim {
        /// Seq of the user row that opened it.
        turn: i64,
        /// The heard prefix.
        keep: String,
    },
    /// `Voice::Stop { id: None }` (client sink: publish `Flush` first).
    StopSpeech,
    /// `Voice::Pause` (device sink); report the reply with [`Policy::paused`].
    Pause,
    /// `Voice::Resume`; publish `Resumed`.
    Resume(ResumeReason),
    /// Publish `Holding`.
    Holding {
        /// Into the agent's speech.
        since_speech_ms: u64,
    },
    /// Stage 2 said real: fetch the player position (device sink, unless a
    /// pause already reported it) and call [`Policy::commit`].
    Commit,
    /// Publish `Interrupted`.
    Interrupted {
        /// Kept.
        heard: u32,
        /// Dropped.
        dropped: u32,
    },
    /// Start (`Some(after)`) or cancel (`None`) a timer.
    Timer(Timer, Option<Duration>),
}

/// Where a sentence of the agent's turn is (76 §4.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// `Speak` sent.
    Queued,
    /// `SpeechStarted`.
    Playing,
    /// `SpeechFinished { Done }`: every sample published (device: in the ring).
    Played,
    /// Never heard: stopped, errored, or cut by an interruption.
    Dropped,
}

/// One spoken sentence of the agent's turn.
#[derive(Clone, Debug, PartialEq)]
pub struct Sentence {
    /// Spoken form.
    pub text: String,
    /// Its utterance, once `Speak` replied.
    pub speech: Option<SpeechId>,
    /// Where it is.
    pub status: Status,
    /// Client sink: highest packet published.
    last_seq: Option<u32>,
}

/// The turn ledger (76 §4.6): what was generated and what was heard.
#[derive(Debug, Default)]
pub struct Ledger {
    /// In speaking order.
    pub sentences: Vec<Sentence>,
    /// Client sink: sentences the client finished playing (a prefix).
    remote_heard: usize,
    /// Client sink: the last `played_seq` reported.
    last_played_seq: Option<u32>,
    /// `TurnFinished` arrived; nothing more will be enqueued.
    turn_done: bool,
    /// `TurnStarted.turn_seq` of the turn being spoken.
    turn: Option<i64>,
}

impl Ledger {
    fn index_of(&self, speech: SpeechId) -> Option<usize> {
        self.sentences.iter().position(|s| s.speech == Some(speech))
    }

    /// Anything still to be heard.
    fn busy(&self) -> bool {
        self.sentences
            .iter()
            .any(|s| matches!(s.status, Status::Queued | Status::Playing))
    }

    /// Sentences the user heard in full (76 §4.6). Device: the leading run of
    /// `Played` sentences, cut at the one the player was inside when it
    /// paused or flushed (`played.id`). Client: what `played_seq` advanced.
    pub fn heard(&self, sink: Sink, played: Option<Played>) -> usize {
        match sink {
            Sink::Client => self.remote_heard,
            Sink::Device => {
                let cut = played
                    .and_then(|p| p.id)
                    .and_then(|id| self.index_of(id))
                    .unwrap_or(self.sentences.len());
                // ponytail: a sentence shorter than the ring can be `Played` (all
                // samples pushed) with the cursor still in the gap before it; the
                // Player's entry ledger would tell — expose it through `Voice` if
                // a kept prefix ever runs one sentence long.
                self.sentences[..cut]
                    .iter()
                    .take_while(|s| s.status == Status::Played)
                    .count()
            }
        }
    }

    /// Client sink: the client finished playing packet `seq`. A sentence
    /// counts as heard when its last packet ended, or when the client's seq
    /// dropped (it moved on to the next utterance).
    fn played_seq(&mut self, seq: u32) {
        let moved_on = self.last_played_seq.is_some_and(|prev| seq < prev);
        self.last_played_seq = Some(seq);
        let advance = |s: &Sentence, by_seq: bool| {
            s.status == Status::Played && (!by_seq || s.last_seq.is_some_and(|l| seq >= l))
        };
        if moved_on
            && self
                .sentences
                .get(self.remote_heard)
                .is_some_and(|s| advance(s, false))
        {
            self.remote_heard += 1;
        }
        if self
            .sentences
            .get(self.remote_heard)
            .is_some_and(|s| advance(s, true))
        {
            self.remote_heard += 1;
        }
    }

    /// The last ~2 s of the agent's speech: the sentence playing and the one
    /// before it (76 §4.4 step 3).
    fn recent(&self) -> String {
        let end = self
            .sentences
            .iter()
            .rposition(|s| matches!(s.status, Status::Playing | Status::Played))
            .map_or(0, |i| i + 1);
        self.sentences[end.saturating_sub(2)..end]
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// The candidate utterance while the agent speaks (76 §0.6).
#[derive(Debug)]
struct Candidate {
    /// Gate opened.
    onset: Instant,
    /// Utterance index once text named it.
    utterance: Option<u32>,
    /// First partial (or final) text.
    text: Option<String>,
    /// Stage 1 reached: the player is paused (or `hold: none` let it run).
    holding: bool,
    /// The gate closed already.
    closed: bool,
}

/// The state machine and everything it decides on. One per talk.
pub struct Policy {
    cfg: TalkConfig,
    sink: Sink,
    /// Where we are (76 §4.2).
    pub state: State,
    /// The agent's turn being spoken.
    pub ledger: Ledger,
    /// Streamed markdown of the current block run; `None` between blocks.
    md: Option<markdown::Text>,
    /// Bytes of `md` already spoken.
    spoken_to: usize,
    /// Utterances of the turn being built, space-joined at submit.
    pending: Vec<String>,
    /// First onset of the turn being built.
    turn_start: Option<Instant>,
    /// Last close of the turn being built.
    turn_end: Option<Instant>,
    /// The next marker says `interrupted`.
    interrupted: bool,
    /// The barge-in candidate, while `Speaking`/`Holding`/`Thinking`.
    candidate: Option<Candidate>,
    /// An utterance stage 2 dismissed (backchannel/echo): its final is not text.
    discard: Option<u32>,
    /// The `Pause` reply, for the heard prefix at commit.
    played: Option<Played>,
    /// First `SpeechStarted` of the turn, for `Holding.since_speech_ms`.
    speech_start: Option<Instant>,
    /// We cancelled the turn ourselves; its `TurnFinished` is not news.
    cancelled: bool,
}

impl Policy {
    /// A talk that just started, `Listening`.
    pub fn new(cfg: TalkConfig, sink: Sink) -> Self {
        Self {
            cfg,
            sink,
            state: State::Listening,
            ledger: Ledger::default(),
            md: None,
            spoken_to: 0,
            pending: Vec::new(),
            turn_start: None,
            turn_end: None,
            interrupted: false,
            candidate: None,
            discard: None,
            played: None,
            speech_start: None,
            cancelled: false,
        }
    }

    /// `Speak` replied for ledger sentence `index`.
    pub fn spoken(&mut self, index: usize, speech: SpeechId) {
        if let Some(s) = self.ledger.sentences.get_mut(index) {
            s.speech = Some(speech);
        }
    }

    /// `Pause` replied: where the player stopped.
    pub fn paused(&mut self, played: Played) {
        self.played = Some(played);
    }

    /// `Speak` for ledger sentence `index` failed: it will never be heard.
    pub fn failed(&mut self, index: usize) -> Vec<Action> {
        let mut out = Vec::new();
        if let Some(s) = self.ledger.sentences.get_mut(index) {
            s.status = Status::Dropped;
            self.maybe_listen(&mut out);
        }
        out
    }

    /// One input in; the actions out, in order.
    pub fn step(&mut self, input: Input, now: Instant) -> Vec<Action> {
        let mut out = Vec::new();
        match input {
            Input::Onset => self.onset(now, &mut out),
            Input::Close => self.close(now, &mut out),
            Input::Text {
                utterance,
                text,
                is_final,
            } => self.text(utterance, text, is_final, now, &mut out),
            Input::Timer(Timer::EndOfTurn) => self.submit(now, &mut out),
            Input::Timer(Timer::Hold) => self.hold(now, &mut out),
            Input::Timer(Timer::Decide) => self.decide(now, &mut out),
            Input::Delta(text) => {
                self.md.get_or_insert_with(markdown::Text::new).push(&text);
                self.speak_committed(&mut out);
            }
            Input::BlockEnd => self.block_end(&mut out),
            // A turn typed elsewhere during the talk is spoken like ours.
            Input::TurnStarted(turn) => {
                if self.state == State::Listening {
                    self.ledger = Ledger::default();
                    self.md = None;
                    self.spoken_to = 0;
                    self.speech_start = None;
                    self.set(State::Thinking, &mut out);
                }
                self.ledger.turn = Some(turn);
            }
            Input::TurnFinished(reason) => self.turn_finished(&reason, &mut out),
            Input::SpeechStarted(speech) => {
                if let Some(i) = self.ledger.index_of(speech) {
                    let s = &mut self.ledger.sentences[i];
                    s.status = Status::Playing;
                    out.push(Action::Saying {
                        speech,
                        text: s.text.clone(),
                    });
                    self.speech_start.get_or_insert(now);
                    if self.state == State::Thinking {
                        self.set(State::Speaking, &mut out);
                    }
                }
            }
            Input::SpeechFinished { speech, done } => {
                if let Some(i) = self.ledger.index_of(speech) {
                    let s = &mut self.ledger.sentences[i];
                    if s.status != Status::Dropped {
                        s.status = if done {
                            Status::Played
                        } else {
                            Status::Dropped
                        };
                    }
                    self.maybe_listen(&mut out);
                }
            }
            Input::Packet { speech, seq } => {
                if let Some(i) = self.ledger.index_of(speech) {
                    self.ledger.sentences[i].last_seq = Some(seq);
                }
            }
            Input::PlayedSeq(seq) => self.ledger.played_seq(seq),
        }
        out
    }

    fn set(&mut self, state: State, out: &mut Vec<Action>) {
        if self.state != state {
            self.state = state;
            out.push(Action::State(state));
        }
    }

    // ---- the user's side -------------------------------------------------

    fn onset(&mut self, now: Instant, out: &mut Vec<Action>) {
        match self.state {
            State::Listening => {
                out.push(Action::Timer(Timer::EndOfTurn, None));
                self.turn_start.get_or_insert(now);
            }
            State::Thinking | State::Speaking => {
                self.candidate = Some(Candidate {
                    onset: now,
                    utterance: None,
                    text: None,
                    holding: false,
                    closed: false,
                });
                out.push(Action::Timer(
                    Timer::Hold,
                    Some(Duration::from_millis(self.cfg.barge_in_min_ms)),
                ));
            }
            // A second onset while already deciding: the candidate stands.
            State::Holding | State::Idle | State::Interrupted => {}
        }
    }

    fn close(&mut self, now: Instant, out: &mut Vec<Action>) {
        match self.state {
            State::Listening => self.turn_end = Some(now),
            _ => {
                if let Some(c) = self.candidate.as_mut() {
                    c.closed = true;
                    if !c.holding {
                        // Shorter than `barge_in_min_ms`: a cough, a swallowed word.
                        self.candidate = None;
                        out.push(Action::Timer(Timer::Hold, None));
                    }
                }
            }
        }
    }

    fn text(
        &mut self,
        utterance: u32,
        text: String,
        is_final: bool,
        now: Instant,
        out: &mut Vec<Action>,
    ) {
        let discarded = self.discard == Some(utterance);
        if discarded && is_final {
            self.discard = None;
        }
        // An empty final (a cough the VAD opened on) is nothing for the frontend to show.
        if !text.is_empty() {
            out.push(Action::Heard {
                utterance,
                text: text.clone(),
                is_final,
            });
        }
        if let Some(c) = self.candidate.as_mut().filter(|_| !discarded) {
            c.utterance.get_or_insert(utterance);
            if c.utterance == Some(utterance) && c.text.is_none() {
                if !text.is_empty() {
                    c.text = Some(text.clone());
                    if c.holding {
                        self.settle(now, out);
                        return;
                    }
                } else if is_final && c.holding {
                    self.settle(now, out);
                    return;
                }
            }
        }
        // 76 §4.7: partials are for the frontend; empty finals (a cough the VAD
        // opened on) do not start the timer.
        if !is_final || self.state != State::Listening || discarded {
            return;
        }
        if text.is_empty() {
            // Nothing said yet: the turn's clock restarts at the next onset.
            if self.pending.is_empty() {
                self.turn_start = None;
            }
            return;
        }
        self.pending.push(text);
        self.arm_end_of_turn(out);
    }

    /// Start the end-of-turn clock for the last pending utterance (76 §4.7).
    fn arm_end_of_turn(&self, out: &mut Vec<Action>) {
        let Some(last) = self.pending.last() else {
            return;
        };
        let wait = if last.ends_with(['.', '?', '!']) {
            self.cfg.end_of_turn_punct_ms
        } else {
            self.cfg.end_of_turn_ms
        };
        out.push(Action::Timer(
            Timer::EndOfTurn,
            Some(Duration::from_millis(wait)),
        ));
    }

    /// The end-of-turn timer fired: `[spoken, N s] …` to the session.
    fn submit(&mut self, now: Instant, out: &mut Vec<Action>) {
        if self.state != State::Listening || self.pending.is_empty() {
            return;
        }
        let start = self.turn_start.take().unwrap_or(now);
        let end = self.turn_end.take().unwrap_or(now);
        let secs = end.duration_since(start).as_millis().div_ceil(1000).max(1);
        let body = self.pending.drain(..).collect::<Vec<_>>().join(" ");
        let interrupted = std::mem::take(&mut self.interrupted);
        let marker = if interrupted {
            format!("[spoken, {secs} s, interrupted] ")
        } else {
            format!("[spoken, {secs} s] ")
        };
        self.ledger = Ledger::default();
        self.md = None;
        self.spoken_to = 0;
        self.speech_start = None;
        self.played = None;
        self.cancelled = false;
        out.push(Action::Submit {
            text: marker + &body,
            interrupted,
        });
        self.set(State::Thinking, out);
    }

    // ---- barge-in ----------------------------------------------------------

    /// Stage 1: the gate has been open `barge_in_min_ms`.
    fn hold(&mut self, now: Instant, out: &mut Vec<Action>) {
        let Some(c) = self.candidate.as_mut() else {
            return;
        };
        if c.closed {
            self.candidate = None;
            return;
        }
        c.holding = true;
        let (onset, has_text) = (c.onset, c.text.is_some());
        match self.state {
            // 76 §0.9, gated on a transcript: a noise the VAD opened on while
            // the model thinks must not throw the turn away (smoke, 2026-09-28).
            State::Thinking if has_text => self.late_addition(now, out),
            State::Thinking => {}
            State::Speaking => self.pause(now, out),
            _ => {
                // The turn ended under the candidate: it is ordinary speech now.
                self.turn_start.get_or_insert(onset);
                self.candidate = None;
            }
        }
    }

    /// Stage 1 in `Speaking`: pause the player, decide now or at the deadline.
    fn pause(&mut self, now: Instant, out: &mut Vec<Action>) {
        let Some(c) = self.candidate.as_ref() else {
            return;
        };
        let (onset, has_text) = (c.onset, c.text.is_some());
        if self.cfg.hold == crate::Hold::Pause && self.sink == Sink::Device {
            out.push(Action::Pause);
        }
        let since_speech_ms = self
            .speech_start
            .map_or(0, |t| now.duration_since(t).as_millis() as u64);
        out.push(Action::Holding { since_speech_ms });
        self.set(State::Holding, out);
        if has_text {
            self.decide(now, out);
        } else {
            let left = Duration::from_millis(self.cfg.decide_ms).saturating_sub(now.duration_since(onset));
            out.push(Action::Timer(Timer::Decide, Some(left)));
        }
    }

    /// A held candidate got its first text (or an empty final): resolve it.
    fn settle(&mut self, now: Instant, out: &mut Vec<Action>) {
        let has_text = self.candidate.as_ref().is_some_and(|c| c.text.is_some());
        match (self.state, has_text) {
            (State::Holding, _) => self.decide(now, out),
            (State::Thinking, true) => self.late_addition(now, out),
            (State::Speaking, true) => self.pause(now, out),
            // Held for a noise that said nothing.
            _ => self.candidate = None,
        }
    }

    /// Stage 2: backchannel, echo, or a real cut-in.
    fn decide(&mut self, now: Instant, out: &mut Vec<Action>) {
        if self.state != State::Holding {
            return;
        }
        let Some(c) = self.candidate.take() else {
            return;
        };
        out.push(Action::Timer(Timer::Decide, None));
        let resume = match c.text.as_deref() {
            // Closed with nothing said: a noise, not a cut-in. Open and silent
            // at the deadline: real (76 §4.4).
            None if c.closed => Some(ResumeReason::Backchannel),
            None => None,
            Some(t) if self.is_backchannel(t) => Some(ResumeReason::Backchannel),
            Some(t) if self.is_echo(t) => Some(ResumeReason::Echo),
            Some(_) => None,
        };
        match resume {
            Some(reason) => {
                out.push(Action::Resume(reason));
                self.played = None;
                self.discard = c.utterance.filter(|_| !c.closed);
                self.set(State::Speaking, out);
            }
            None => {
                // The candidate is the first utterance of the next turn.
                self.turn_start = Some(c.onset);
                if c.closed {
                    self.turn_end = Some(now);
                }
                if let Some(t) = c.text.filter(|t| !t.is_empty() && c.closed) {
                    self.pending.push(t);
                }
                out.push(Action::Commit);
            }
        }
    }

    /// The five cuts (76 §4.5), given where the player was.
    pub fn commit(&mut self, played: Option<Played>) -> Vec<Action> {
        let mut out = Vec::new();
        let played = self.played.take().or(played);
        let heard = self.ledger.heard(self.sink, played);
        let kept = self.ledger.sentences[..heard]
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let dropped = self.ledger.sentences.len() - heard;
        for s in &mut self.ledger.sentences[heard..] {
            s.status = Status::Dropped;
        }
        out.push(Action::StopSpeech);
        // 76 §7: a finished turn has its reply persisted in full — the heard
        // prefix is written over it; a running one is cut with `keep` and
        // answers with `TurnFinished { interrupted }`.
        match self.ledger.turn.filter(|_| self.ledger.turn_done) {
            Some(turn) => out.push(Action::Trim { turn, keep: kept }),
            None => {
                out.push(Action::Cancel(Some(kept)));
                self.cancelled = true;
            }
        }
        self.ledger.turn_done = true;
        self.md = None;
        self.interrupted = true;
        out.push(Action::Interrupted {
            heard: heard as u32,
            dropped: dropped as u32,
        });
        self.set(State::Listening, &mut out);
        // A final that already arrived starts the end-of-turn clock now.
        self.arm_end_of_turn(&mut out);
        out
    }

    /// 76 §0.9: speech while `Thinking` — cancel the turn nothing was heard of;
    /// the new utterance is submitted on its own (the cancelled input's user
    /// row stays in history, so re-sending its text would duplicate it).
    fn late_addition(&mut self, now: Instant, out: &mut Vec<Action>) {
        let Some(c) = self.candidate.take() else {
            return;
        };
        out.push(Action::StopSpeech);
        out.push(Action::Cancel(None));
        self.cancelled = true;
        self.ledger = Ledger::default();
        self.md = None;
        self.turn_start = Some(c.onset);
        if let Some(t) = c.text.filter(|t| !t.is_empty() && c.closed) {
            self.turn_end = Some(now);
            self.pending.push(t);
        }
        self.set(State::Listening, out);
        self.arm_end_of_turn(out);
    }

    fn is_backchannel(&self, text: &str) -> bool {
        let words: Vec<String> = normalize(text);
        if words.is_empty() {
            return false;
        }
        let list: Vec<Vec<String>> = self.cfg.backchannels.iter().map(|b| normalize(b)).collect();
        // Every token (or listed multi-word phrase) is in the list; two or more
        // strangers make it real (76 §4.4 step 2).
        let mut i = 0;
        let mut strangers = 0;
        while i < words.len() {
            let hit = list
                .iter()
                .filter(|p| !p.is_empty() && words[i..].starts_with(p))
                .map(Vec::len)
                .max();
            match hit {
                Some(n) => i += n,
                None => {
                    strangers += 1;
                    i += 1;
                }
            }
        }
        strangers == 0
    }

    fn is_echo(&self, text: &str) -> bool {
        let recent = self.ledger.recent();
        let echo = is_echo(text, &recent, self.cfg.echo_ratio);
        if echo {
            tracing::debug!(candidate = text, recent, "talk: barge-in rejected as echo");
        }
        echo
    }

    // ---- the agent's side --------------------------------------------------

    /// Speak the blocks `md` committed since the last call (76 §4.8).
    fn speak_committed(&mut self, out: &mut Vec<Action>) {
        let Some(md) = self.md.as_ref() else { return };
        let end = md.committed();
        if end <= self.spoken_to {
            return;
        }
        let block = &md.raw()[self.spoken_to..end];
        self.spoken_to = end;
        let spoken = markdown::spoken(block);
        for line in spoken.lines().flat_map(sentences) {
            self.ledger.sentences.push(Sentence {
                text: line.clone(),
                speech: None,
                status: Status::Queued,
                last_seq: None,
            });
            out.push(Action::Speak {
                index: self.ledger.sentences.len() - 1,
                text: line,
            });
        }
    }

    /// Close the block run: the tail is a block too.
    fn block_end(&mut self, out: &mut Vec<Action>) {
        if let Some(md) = self.md.as_mut() {
            md.finish();
        }
        self.speak_committed(out);
        self.md = None;
        self.spoken_to = 0;
    }

    fn turn_finished(&mut self, reason: &str, out: &mut Vec<Action>) {
        if std::mem::take(&mut self.cancelled) {
            return;
        }
        match reason {
            // Someone else cut the turn (Esc in the TUI): nothing more to say.
            "cancelled" | "interrupted" | "error" => {
                self.md = None;
                self.spoken_to = 0;
                if self.ledger.busy() {
                    out.push(Action::StopSpeech);
                }
                for s in &mut self.ledger.sentences {
                    if matches!(s.status, Status::Queued | Status::Playing) {
                        s.status = Status::Dropped;
                    }
                }
                self.ledger.turn_done = true;
                self.candidate = None;
                out.push(Action::Timer(Timer::Hold, None));
                out.push(Action::Timer(Timer::Decide, None));
                if matches!(
                    self.state,
                    State::Thinking | State::Speaking | State::Holding
                ) {
                    self.set(State::Listening, out);
                }
            }
            _ => {
                self.block_end(out);
                self.ledger.turn_done = true;
                self.maybe_listen(out);
            }
        }
    }

    /// Turn finished and the queue drained → `Listening` (76 §4.8).
    fn maybe_listen(&mut self, out: &mut Vec<Action>) {
        if self.ledger.turn_done
            && !self.ledger.busy()
            && matches!(self.state, State::Thinking | State::Speaking)
        {
            self.set(State::Listening, out);
        }
    }
}

/// One `spoken` line (a block) → its sentences: split after `.`, `?`, `!`
/// followed by whitespace. ponytail: no abbreviation list — the prompt asks
/// for plain spoken prose; add one if "e.g." starts splitting replies.
pub fn sentences(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = line.trim().chars().peekable();
    while let Some(c) = chars.next() {
        cur.push(c);
        if matches!(c, '.' | '?' | '!') && chars.peek().is_some_and(|n| n.is_whitespace()) {
            out.push(std::mem::take(&mut cur).trim().to_owned());
        }
    }
    let tail = cur.trim();
    if !tail.is_empty() {
        out.push(tail.to_owned());
    }
    out
}

/// Lowercase words without punctuation.
fn normalize(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| c.is_alphanumeric() || *c == '-')
                .flat_map(char::to_lowercase)
                .collect::<String>()
        })
        .filter(|w| !w.is_empty())
        .collect()
}

/// Levenshtein distance over chars.
fn levenshtein(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, ca) in a.chars().enumerate() {
        cur[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            cur[j + 1] = (prev[j] + usize::from(ca != cb))
                .min(prev[j + 1] + 1)
                .min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// `1 − distance / longest`; 1.0 for two empty strings.
pub fn ratio(a: &str, b: &str) -> f32 {
    let longest = a.chars().count().max(b.chars().count());
    if longest == 0 {
        return 1.0;
    }
    1.0 - levenshtein(a, b) as f32 / longest as f32
}

/// The candidate is (fuzzily) a run of words the agent just said (76 §4.4 step 3).
pub fn is_echo(candidate: &str, recent: &str, min_ratio: f32) -> bool {
    let c = normalize(candidate);
    let r = normalize(recent);
    if c.is_empty() || r.is_empty() || c.len() > r.len() {
        return false;
    }
    let c = c.join(" ");
    r.windows(c.split(' ').count())
        .any(|w| ratio(&c, &w.join(" ")) >= min_ratio)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Hold;

    fn ms(t: Instant, n: u64) -> Instant {
        t + Duration::from_millis(n)
    }

    fn policy(sink: Sink) -> (Policy, Instant) {
        let t0 = Instant::now();
        (Policy::new(TalkConfig::default(), sink), t0)
    }

    fn final_text(p: &mut Policy, u: u32, text: &str, at: Instant) -> Vec<Action> {
        p.step(
            Input::Text {
                utterance: u,
                text: text.into(),
                is_final: true,
            },
            at,
        )
    }

    fn timer(actions: &[Action], t: Timer) -> Option<Option<Duration>> {
        actions.iter().rev().find_map(|a| match a {
            Action::Timer(k, d) if *k == t => Some(*d),
            _ => None,
        })
    }

    /// Talk the agent into `Speaking` with `n` sentences: the first playing, the rest queued.
    fn speaking(p: &mut Policy, t0: Instant, sentences: &[&str]) -> Vec<SpeechId> {
        p.step(Input::Onset, t0);
        final_text(p, 0, "Hello.", ms(t0, 500));
        p.step(Input::Timer(Timer::EndOfTurn), ms(t0, 750));
        assert_eq!(p.state, State::Thinking);
        p.step(Input::TurnStarted(7), ms(t0, 800));
        let md = sentences.join(" ");
        p.step(Input::Delta(md), ms(t0, 1000));
        let acts = p.step(Input::TurnFinished("end_turn".into()), ms(t0, 1100));
        let ids: Vec<SpeechId> = acts
            .iter()
            .filter_map(|a| match a {
                Action::Speak { index, .. } => {
                    let id = SpeechId::new();
                    p.spoken(*index, id);
                    Some(id)
                }
                _ => None,
            })
            .collect();
        assert_eq!(ids.len(), sentences.len());
        p.step(Input::SpeechStarted(ids[0]), ms(t0, 1300));
        assert_eq!(p.state, State::Speaking);
        ids
    }

    #[test]
    fn end_of_turn_timing_with_and_without_punctuation() {
        let (mut p, t0) = policy(Sink::Device);
        p.step(Input::Onset, t0);
        let a = final_text(&mut p, 0, "what time is it", ms(t0, 1200));
        assert_eq!(
            timer(&a, Timer::EndOfTurn),
            Some(Some(Duration::from_millis(700)))
        );
        // An onset inside the window cancels the timer and the turn goes on.
        let a = p.step(Input::Onset, ms(t0, 1500));
        assert_eq!(timer(&a, Timer::EndOfTurn), Some(None));
        p.step(Input::Close, ms(t0, 2300));
        let a = final_text(&mut p, 1, "in Lisbon?", ms(t0, 2400));
        assert_eq!(
            timer(&a, Timer::EndOfTurn),
            Some(Some(Duration::from_millis(250)))
        );
        // Empty finals are ignored: no timer, no text.
        let a = final_text(&mut p, 2, "", ms(t0, 2500));
        assert!(timer(&a, Timer::EndOfTurn).is_none());
        let a = p.step(Input::Timer(Timer::EndOfTurn), ms(t0, 2650));
        assert_eq!(
            a,
            vec![
                Action::Submit {
                    text: "[spoken, 3 s] what time is it in Lisbon?".into(),
                    interrupted: false,
                },
                Action::State(State::Thinking),
            ]
        );
        // Partials are for the frontend only.
        let (mut p, t0) = policy(Sink::Device);
        let a = p.step(
            Input::Text {
                utterance: 0,
                text: "what".into(),
                is_final: false,
            },
            t0,
        );
        assert_eq!(
            a,
            vec![Action::Heard {
                utterance: 0,
                text: "what".into(),
                is_final: false
            }]
        );
    }

    #[test]
    fn classifier_backchannel_echo_real() {
        let (mut p, t0) = policy(Sink::Device);
        p.ledger.sentences.push(Sentence {
            text: "The build is green.".into(),
            speech: None,
            status: Status::Played,
            last_seq: None,
        });
        p.ledger.sentences.push(Sentence {
            text: "Two tests were skipped on the way.".into(),
            speech: None,
            status: Status::Playing,
            last_seq: None,
        });
        assert!(p.is_backchannel("Yeah."));
        assert!(p.is_backchannel("mm-hmm, okay"));
        assert!(p.is_backchannel("I see"));
        assert!(!p.is_backchannel("yeah but wait"));
        assert!(!p.is_backchannel(""));
        assert!(p.is_echo("two tests were skipped"));
        assert!(p.is_echo("the build is green"));
        assert!(p.is_echo("tests were skip"));
        assert!(!p.is_echo("stop, that is wrong"));
        assert!(!p.is_echo("the build is red and the tests were skipped on the way home"));
        assert!((ratio("kitten", "sitting") - (1.0 - 3.0 / 7.0)).abs() < 1e-6);
        assert_eq!(ratio("", ""), 1.0);
        let _ = t0;
    }

    #[test]
    fn heard_prefix_from_ledger_and_position() {
        let (mut p, _) = policy(Sink::Device);
        let ids: Vec<SpeechId> = (0..4).map(|_| SpeechId::new()).collect();
        for (i, id) in ids.iter().enumerate() {
            p.ledger.sentences.push(Sentence {
                text: format!("s{i}."),
                speech: Some(*id),
                status: match i {
                    0 | 1 => Status::Played,
                    2 => Status::Playing,
                    _ => Status::Queued,
                },
                last_seq: None,
            });
        }
        let at = |id: Option<SpeechId>| {
            Some(Played {
                id,
                seq: 3,
                samples: 0,
            })
        };
        // The player is inside sentence 2: 0 and 1 were heard.
        assert_eq!(p.ledger.heard(Sink::Device, at(Some(ids[2]))), 2);
        // The player is still inside sentence 1 (all of it in the ring): only 0.
        assert_eq!(p.ledger.heard(Sink::Device, at(Some(ids[1]))), 1);
        // Between utterances: the leading `Played` run.
        assert_eq!(p.ledger.heard(Sink::Device, at(None)), 2);
        assert_eq!(p.ledger.heard(Sink::Device, None), 2);

        // Client sink: `played_seq` advances the cursor, one utterance at a time.
        let (mut p, _) = policy(Sink::Client);
        for (i, id) in ids.iter().enumerate() {
            p.ledger.sentences.push(Sentence {
                text: format!("s{i}."),
                speech: Some(*id),
                status: if i < 3 {
                    Status::Played
                } else {
                    Status::Playing
                },
                last_seq: Some([4, 9, 2, 7][i]),
            });
        }
        p.step(Input::PlayedSeq(3), Instant::now());
        assert_eq!(p.ledger.heard(Sink::Client, None), 0);
        p.step(Input::PlayedSeq(4), Instant::now());
        assert_eq!(p.ledger.heard(Sink::Client, None), 1);
        p.step(Input::PlayedSeq(6), Instant::now());
        assert_eq!(p.ledger.heard(Sink::Client, None), 1);
        // The seq dropped: the client moved on past sentence 1 (its tail packet
        // ended between two reports); sentence 2's seq 2 ends it too.
        p.step(Input::PlayedSeq(2), Instant::now());
        assert_eq!(p.ledger.heard(Sink::Client, None), 3);
        // The one still playing never counts.
        p.step(Input::PlayedSeq(7), Instant::now());
        assert_eq!(p.ledger.heard(Sink::Client, None), 3);
    }

    #[test]
    fn barge_in_commits_the_five_cuts_and_marks_the_next_input() {
        let (mut p, t0) = policy(Sink::Device);
        let ids = speaking(&mut p, t0, &["One.", "Two.", "Three."]);
        p.step(
            Input::SpeechFinished {
                speech: ids[0],
                done: true,
            },
            ms(t0, 2000),
        );
        p.step(Input::SpeechStarted(ids[1]), ms(t0, 2001));
        // Stage 1.
        let a = p.step(Input::Onset, ms(t0, 3000));
        assert_eq!(
            timer(&a, Timer::Hold),
            Some(Some(Duration::from_millis(400)))
        );
        let a = p.step(Input::Timer(Timer::Hold), ms(t0, 3400));
        assert_eq!(
            a,
            vec![
                Action::Pause,
                Action::Holding {
                    since_speech_ms: 2100
                },
                Action::State(State::Holding),
                Action::Timer(Timer::Decide, Some(Duration::from_millis(500))),
            ]
        );
        p.paused(Played {
            id: Some(ids[1]),
            seq: 2,
            samples: 0,
        });
        // Stage 2: a real cut-in on the first partial.
        let a = p.step(
            Input::Text {
                utterance: 1,
                text: "no wait".into(),
                is_final: false,
            },
            ms(t0, 3500),
        );
        assert!(a.contains(&Action::Commit));
        let a = p.commit(None);
        assert_eq!(
            a,
            vec![
                Action::StopSpeech,
                Action::Trim {
                    turn: 7,
                    keep: "One.".into()
                },
                Action::Interrupted {
                    heard: 1,
                    dropped: 2
                },
                Action::State(State::Listening),
            ]
        );
        assert_eq!(p.ledger.sentences[1].status, Status::Dropped);
        // No `TurnFinished` follows a trim; the candidate finishes the turn and
        // its marker still says interrupted.
        p.step(Input::Close, ms(t0, 4000));
        final_text(&mut p, 1, "no wait, use the other one", ms(t0, 4100));
        let a = p.step(Input::Timer(Timer::EndOfTurn), ms(t0, 4800));
        assert_eq!(
            a[0],
            Action::Submit {
                text: "[spoken, 1 s, interrupted] no wait, use the other one".into(),
                interrupted: true,
            }
        );
        assert_eq!(p.state, State::Thinking);
    }

    #[test]
    fn backchannel_resumes_and_its_final_is_not_text() {
        let (mut p, t0) = policy(Sink::Device);
        let _ids = speaking(&mut p, t0, &["One.", "Two."]);
        p.step(Input::Onset, ms(t0, 3000));
        p.step(Input::Timer(Timer::Hold), ms(t0, 3400));
        assert_eq!(p.state, State::Holding);
        let a = p.step(
            Input::Text {
                utterance: 1,
                text: "Mhm.".into(),
                is_final: false,
            },
            ms(t0, 3500),
        );
        assert!(a.contains(&Action::Resume(ResumeReason::Backchannel)));
        assert_eq!(p.state, State::Speaking);
        // Its final neither reaches the frontend as text nor the pending turn.
        p.step(Input::Close, ms(t0, 3600));
        let a = final_text(&mut p, 1, "Mhm.", ms(t0, 3700));
        assert_eq!(
            a,
            vec![Action::Heard {
                utterance: 1,
                text: "Mhm.".into(),
                is_final: true
            }]
        );
        assert!(p.pending.is_empty());
        // A cough shorter than the hold never holds.
        let a = p.step(Input::Onset, ms(t0, 4000));
        assert!(timer(&a, Timer::Hold).is_some());
        let a = p.step(Input::Close, ms(t0, 4200));
        assert_eq!(timer(&a, Timer::Hold), Some(None));
        assert_eq!(p.step(Input::Timer(Timer::Hold), ms(t0, 4400)), vec![]);
        assert_eq!(p.state, State::Speaking);
        // Held, then closed with an empty final: a noise, playback resumes.
        p.step(Input::Onset, ms(t0, 4500));
        p.step(Input::Timer(Timer::Hold), ms(t0, 4900));
        assert_eq!(p.state, State::Holding);
        p.step(Input::Close, ms(t0, 4950));
        let a = final_text(&mut p, 3, "", ms(t0, 4960));
        assert!(
            a.contains(&Action::Resume(ResumeReason::Backchannel)),
            "{a:?}"
        );
        assert_eq!(p.state, State::Speaking);
        // No partial by the deadline: real.
        p.step(Input::Onset, ms(t0, 5000));
        p.step(Input::Timer(Timer::Hold), ms(t0, 5400));
        let a = p.step(Input::Timer(Timer::Decide), ms(t0, 5900));
        assert!(a.contains(&Action::Commit));
    }

    #[test]
    fn hold_none_keeps_playing_until_the_decision() {
        let (mut p, t0) = policy(Sink::Device);
        p.cfg.hold = Hold::None;
        speaking(&mut p, t0, &["One."]);
        p.step(Input::Onset, ms(t0, 3000));
        let a = p.step(Input::Timer(Timer::Hold), ms(t0, 3400));
        assert!(!a.contains(&Action::Pause));
        assert_eq!(p.state, State::Holding);
    }

    #[test]
    fn speech_while_thinking_cancels_and_submits_the_new_text() {
        let (mut p, t0) = policy(Sink::Device);
        p.step(Input::Onset, t0);
        p.step(Input::Close, ms(t0, 900));
        final_text(&mut p, 0, "book a table", ms(t0, 1000));
        let a = p.step(Input::Timer(Timer::EndOfTurn), ms(t0, 1700));
        assert_eq!(
            a[0],
            Action::Submit {
                text: "[spoken, 1 s] book a table".into(),
                interrupted: false
            }
        );
        // A noise the VAD opened on while thinking: held, said nothing, dropped —
        // and an empty final is nothing for the frontend either.
        p.step(Input::Onset, ms(t0, 1800));
        assert_eq!(p.step(Input::Timer(Timer::Hold), ms(t0, 2200)), vec![]);
        p.step(Input::Close, ms(t0, 2300));
        let a = final_text(&mut p, 1, "", ms(t0, 2350));
        assert_eq!(a, vec![], "empty final publishes nothing: {a:?}");
        assert_eq!(p.state, State::Thinking);
        // The user goes on before any audio: the cancel waits for the transcript.
        p.step(Input::Onset, ms(t0, 2600));
        assert_eq!(p.step(Input::Timer(Timer::Hold), ms(t0, 3000)), vec![]);
        let a = p.step(
            Input::Text {
                utterance: 2,
                text: "for two".into(),
                is_final: false,
            },
            ms(t0, 3100),
        );
        assert_eq!(
            a[1..],
            [
                Action::StopSpeech,
                Action::Cancel(None),
                Action::State(State::Listening)
            ]
        );
        assert_eq!(
            p.step(Input::TurnFinished("cancelled".into()), ms(t0, 3150)),
            vec![]
        );
        p.step(Input::Close, ms(t0, 3600));
        final_text(&mut p, 2, "for two at eight.", ms(t0, 3700));
        let a = p.step(Input::Timer(Timer::EndOfTurn), ms(t0, 3950));
        assert_eq!(
            a[0],
            Action::Submit {
                text: "[spoken, 1 s] for two at eight.".into(),
                interrupted: false
            }
        );
    }

    #[test]
    fn state_transitions_of_4_2() {
        let (mut p, t0) = policy(Sink::Device);
        assert_eq!(p.state, State::Listening);
        let ids = speaking(&mut p, t0, &["One.", "Two."]);
        // Speaking until the last sentence is done, then Listening.
        p.step(
            Input::SpeechFinished {
                speech: ids[0],
                done: true,
            },
            ms(t0, 2000),
        );
        assert_eq!(p.state, State::Speaking);
        p.step(Input::SpeechStarted(ids[1]), ms(t0, 2001));
        let a = p.step(
            Input::SpeechFinished {
                speech: ids[1],
                done: true,
            },
            ms(t0, 3000),
        );
        assert_eq!(a, vec![Action::State(State::Listening)]);

        // A tool round: prose before the batch is spoken, the state stays.
        let (mut p, t0) = policy(Sink::Device);
        p.step(Input::Onset, t0);
        final_text(&mut p, 0, "Go.", ms(t0, 500));
        p.step(Input::Timer(Timer::EndOfTurn), ms(t0, 750));
        p.step(Input::Delta("Let me check.".into()), ms(t0, 1000));
        let a = p.step(Input::BlockEnd, ms(t0, 1100));
        assert!(matches!(a[0], Action::Speak { index: 0, .. }));
        assert_eq!(p.state, State::Thinking);
        // A cancel from elsewhere (Esc) with sentences queued: stop, Listening.
        p.spoken(0, ids[0]);
        p.step(Input::SpeechStarted(ids[0]), ms(t0, 1200));
        let a = p.step(Input::TurnFinished("cancelled".into()), ms(t0, 1300));
        assert_eq!(a[0], Action::StopSpeech);
        assert_eq!(p.state, State::Listening);
        assert_eq!(p.ledger.sentences[0].status, Status::Dropped);

        // An empty reply (tools only) goes straight back to Listening.
        let (mut p, t0) = policy(Sink::Device);
        p.step(Input::Onset, t0);
        final_text(&mut p, 0, "Go.", ms(t0, 500));
        p.step(Input::Timer(Timer::EndOfTurn), ms(t0, 750));
        let a = p.step(Input::TurnFinished("end_turn".into()), ms(t0, 1000));
        assert_eq!(a, vec![Action::State(State::Listening)]);
    }

    #[test]
    fn a_paragraph_is_spoken_when_it_closes_not_at_turn_end() {
        let (mut p, t0) = policy(Sink::Device);
        p.step(Input::Onset, t0);
        final_text(&mut p, 0, "Go.", ms(t0, 500));
        p.step(Input::Timer(Timer::EndOfTurn), ms(t0, 750));
        let a = p.step(
            Input::Delta("First one. Still first.\n\nSecond".into()),
            ms(t0, 1000),
        );
        let spoken: Vec<&str> = a
            .iter()
            .filter_map(|a| match a {
                Action::Speak { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(spoken, vec!["First one.", "Still first."]);
        let a = p.step(Input::TurnFinished("end_turn".into()), ms(t0, 1500));
        assert!(a.contains(&Action::Speak {
            index: 2,
            text: "Second.".into()
        }));
    }

    /// 76 §7 post-turn case: the commit chooses `Trim` for a finished turn
    /// and `Cancel { keep }` for a running one, from the ledger alone.
    #[test]
    fn barge_in_trims_a_finished_turn_and_cancels_a_running_one() {
        // Running: the paragraph closed and is being spoken, no `TurnFinished` yet.
        let (mut p, t0) = policy(Sink::Device);
        p.step(Input::Onset, t0);
        final_text(&mut p, 0, "Go.", ms(t0, 500));
        p.step(Input::Timer(Timer::EndOfTurn), ms(t0, 750));
        p.step(Input::TurnStarted(7), ms(t0, 800));
        let a = p.step(Input::Delta("One. Two.\n\nThree".into()), ms(t0, 1000));
        let ids: Vec<SpeechId> = a
            .iter()
            .filter_map(|a| match a {
                Action::Speak { index, .. } => {
                    let id = SpeechId::new();
                    p.spoken(*index, id);
                    Some(id)
                }
                _ => None,
            })
            .collect();
        assert_eq!(ids.len(), 2);
        p.step(Input::SpeechStarted(ids[0]), ms(t0, 1300));
        p.step(
            Input::SpeechFinished {
                speech: ids[0],
                done: true,
            },
            ms(t0, 2000),
        );
        p.step(Input::SpeechStarted(ids[1]), ms(t0, 2001));
        let a = p.commit(Some(Played {
            id: Some(ids[1]),
            seq: 2,
            samples: 0,
        }));
        assert_eq!(a[1], Action::Cancel(Some("One.".into())));
        // Our own cancel's `TurnFinished` is not news.
        assert_eq!(
            p.step(Input::TurnFinished("interrupted".into()), ms(t0, 2100)),
            vec![]
        );

        // Finished: `speaking` feeds `TurnFinished` before the first sentence
        // plays (a short reply) — the reply is already in history in full.
        let (mut p, t0) = policy(Sink::Device);
        let ids = speaking(&mut p, t0, &["One.", "Two.", "Three."]);
        p.step(
            Input::SpeechFinished {
                speech: ids[0],
                done: true,
            },
            ms(t0, 2000),
        );
        p.step(Input::SpeechStarted(ids[1]), ms(t0, 2001));
        let a = p.commit(Some(Played {
            id: Some(ids[1]),
            seq: 2,
            samples: 0,
        }));
        assert_eq!(
            a[1],
            Action::Trim {
                turn: 7,
                keep: "One.".into()
            }
        );
        assert!(!p.cancelled, "no TurnFinished follows a trim");
        assert!(p.interrupted);
    }

    #[test]
    fn typed_turn_is_spoken_and_noise_does_not_age_the_clock() {
        let (mut p, t0) = policy(Sink::Device);
        // Noise the VAD opened on, then silence: the turn clock restarts.
        p.step(Input::Onset, t0);
        p.step(Input::Close, ms(t0, 400));
        final_text(&mut p, 0, "", ms(t0, 450));
        p.step(Input::Onset, ms(t0, 30_000));
        p.step(Input::Close, ms(t0, 31_000));
        final_text(&mut p, 1, "Hello.", ms(t0, 31_100));
        let a = p.step(Input::Timer(Timer::EndOfTurn), ms(t0, 31_350));
        assert_eq!(
            a[0],
            Action::Submit {
                text: "[spoken, 1 s] Hello.".into(),
                interrupted: false
            }
        );
        // A turn typed in the transcript during the talk is spoken with barge-in.
        let (mut p, t0) = policy(Sink::Device);
        assert_eq!(
            p.step(Input::TurnStarted(3), t0),
            vec![Action::State(State::Thinking)]
        );
        p.step(Input::Delta("Typed.".into()), ms(t0, 100));
        let a = p.step(Input::TurnFinished("end_turn".into()), ms(t0, 200));
        assert!(matches!(a[0], Action::Speak { index: 0, .. }));
        assert_eq!(p.state, State::Thinking);
    }
}
