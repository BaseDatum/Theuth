//! The live meeting: the recording clock (wall time minus pauses — the one
//! timeline every phase stamps with, 53 §3.2) and the handles a [`Recording`]
//! owns.

use std::collections::HashSet;
use std::fs::File;
use std::io::Write as _;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use decision::training::{Amendment, ExampleId, Reaction};

use crate::batch::Batcher;
use crate::capture::Capture;
use crate::post::align::Span;
use crate::store::{Gap, Manifest};
use crate::stt::Piece;
use crate::topics::{Record, Topics};
use crate::{BriefId, MeetingConfig, TopicId, Utterance};

/// 07 §3: the live diariser's latest view on the recording clock — labels exist up to
/// `horizon_ms`; `spans` are the labelled turns of the last `diar::KEEP_MS`.
#[derive(Clone, Debug, Default)]
pub struct LiveLabels {
    /// Recording time up to which the diariser has scored.
    pub horizon_ms: u64,
    /// Labelled turns (`s1`..`s8`) of the window, by start.
    pub spans: Vec<Span>,
}

/// Wall `Instant` minus accumulated pause time. Also used by the encoder
/// thread to decide how many frames are due, so file time and manifest time
/// agree.
#[derive(Clone, Debug)]
pub struct RecordingClock {
    started: Instant,
    paused: Duration,
    paused_at: Option<Instant>,
}

impl RecordingClock {
    /// A clock starting at `now`.
    pub fn start_at(now: Instant) -> Self {
        Self {
            started: now,
            paused: Duration::ZERO,
            paused_at: None,
        }
    }

    /// A clock starting now.
    pub fn start() -> Self {
        Self::start_at(Instant::now())
    }

    /// Recording time elapsed at `now`, pauses excluded; frozen while paused.
    pub fn elapsed_at(&self, now: Instant) -> Duration {
        let end = self.paused_at.unwrap_or(now);
        end.saturating_duration_since(self.started)
            .saturating_sub(self.paused)
    }

    /// Recording time in milliseconds.
    pub fn now_ms(&self) -> u64 {
        self.elapsed_at(Instant::now()).as_millis() as u64
    }

    /// Paused right now.
    pub fn is_paused(&self) -> bool {
        self.paused_at.is_some()
    }

    /// Stop the clock at `now`. Idempotent.
    pub fn pause_at(&mut self, now: Instant) {
        if self.paused_at.is_none() {
            self.paused_at = Some(now);
        }
    }

    /// Stop the clock.
    pub fn pause(&mut self) {
        self.pause_at(Instant::now());
    }

    /// Restart the clock at `now`; the closed gap, or `None` if it was running.
    pub fn resume_at(&mut self, now: Instant) -> Option<Gap> {
        let at = self.paused_at.take()?;
        let at_ms = self.elapsed_at(at).as_millis() as u64;
        let dur = now.saturating_duration_since(at);
        self.paused += dur;
        Some(Gap {
            at_ms,
            dur_ms: dur.as_millis() as u64,
        })
    }

    /// Restart the clock.
    pub fn resume(&mut self) -> Option<Gap> {
        self.resume_at(Instant::now())
    }
}

/// The meeting being recorded: its manifest (the live truth, rewritten on
/// every change), clock, devices, the timers riding along (levels,
/// max-length, disk, the 1 s tick) and the phase-2 live state. `StopMeeting`
/// aborts the timers and joins the STT tasks when it takes it apart.
pub struct Recording {
    /// Truth; `gaps` and `degraded` live here, nowhere else.
    pub manifest: Manifest,
    /// Recording time.
    pub clock: RecordingClock,
    /// The three threads and the rings.
    pub capture: Capture,
    /// Level, max-length, disk and tick tasks.
    pub tasks: Vec<tokio::task::JoinHandle<()>>,
    /// Phase 2 (02 §3, §5, §6).
    pub live: Live,
}

/// What a steer the actor sent is waiting for (02 §5.2: how a `meeting_brief`
/// gets its `source`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prompted {
    /// `[meeting tapped "X"]` for this topic.
    Tap(TopicId),
    /// `[meeting deeper on …]` for this brief.
    Deeper(BriefId),
    /// `[meeting request, spoken|typed] …`.
    Request {
        /// Push-to-talk (`true`) or typed.
        spoken: bool,
    },
}

/// Outstanding prompts older than this are forgotten (a brief two minutes
/// after a request is not its answer).
const PROMPT_TTL: Duration = Duration::from_secs(120);

/// A steer the actor sent that a `meeting_brief` may answer.
#[derive(Clone, Debug)]
pub struct Outstanding {
    /// When the steer went out; forgotten after [`PROMPT_TTL`].
    pub at: Instant,
    /// Which steer.
    pub prompt: Prompted,
    /// What was asked (the request text, the tapped label, the deeper'd title) — for the nudge.
    pub text: String,
    /// The model already finished one turn without answering and was reminded once (03 §9).
    pub nudged: bool,
}

/// The batcher, the topics, the session's turn/approval state, the PTT flag,
/// the STT tasks and the two append-only logs.
pub struct Live {
    /// Transcript batching (02 §3).
    pub batcher: Batcher,
    /// Live/dismissed topics and every brief (02 §5).
    pub topics: Topics,
    /// The session has a turn in flight (`AgentEvent::TurnStarted/Finished`).
    pub turn_running: bool,
    /// Unanswered `ApprovalRequested`s of the session.
    pub approvals: HashSet<tools::ApprovalId>,
    /// Where the island says the user is reading (`SetCursor`); mirrored only.
    pub cursor: Option<BriefId>,
    /// Push-to-talk held.
    pub held: bool,
    /// Mirror of `held` for the mic STT task: finals closed while held merge into one request.
    pub held_flag: Arc<AtomicBool>,
    /// Steers sent that a `meeting_brief` may be answering, newest last.
    pub outstanding: Vec<Outstanding>,
    /// The two track tasks; resolve to their final flush at stop.
    pub stt: Vec<tokio::task::JoinHandle<Vec<Piece>>>,
    /// 07 §2: the live diariser task; resolves to its final view at stop. `None` when
    /// live diarization is off, the models are missing, or it went down.
    pub diar: Option<tokio::task::JoinHandle<Option<crate::diar::Labels>>>,
    /// 07 §3: what the diariser has scored so far.
    pub labels: LiveLabels,
    /// 07 §3: closed system utterances waiting for the diariser's horizon to pass their
    /// end before they are logged and batched, in close order.
    pub held_system: Vec<Utterance>,
    /// 57 §3.2: batches the oracle kept from the model (`(stamp, utterances)`), written as
    /// context into the next delivered batch line, then cleared. Oldest first.
    pub carry: Vec<(String, Vec<Utterance>)>,
    /// 57 §3.4: oracle verdicts whose model turn has not finished — the event to update with
    /// the outcome, what the oracle expected, and the training example to amend (57 §11.4).
    pub pending: Vec<(
        crate::OracleDecision,
        crate::oracle::Expected,
        Option<ExampleId>,
    )>,
    /// Cached rows (and titles) `meeting_topic` registered since the last turn ended.
    pub published: Vec<(Option<BriefId>, String)>,
    /// The topics behind `published`, same order, for the tap/dismiss amend rows.
    pub published_topics: Vec<TopicId>,
    /// 57 §11.4: live topics whose brief a captured example produced (the oracle's own publish,
    /// or the model's in a turn that had captured batches) → the example ids to amend on tap/dismiss.
    pub captured: Vec<(TopicId, ExampleId)>,
    /// 02 §4.1: speakers heard so far (`Utterance::who`), in order of first utterance.
    pub speakers: Vec<String>,
    /// 02 §4.1: the compaction head last pushed to the session; a re-render that
    /// equals it is not pushed again.
    pub last_head: Option<String>,
    /// `live.jsonl`, append-only.
    live_log: Option<File>,
    /// `briefs.jsonl`, append-only.
    briefs_log: Option<File>,
}

impl Live {
    fn open_log(dir: &Path, name: &str) -> Option<File> {
        File::options()
            .create(true)
            .append(true)
            .open(dir.join(name))
            .map_err(|e| tracing::warn!("meeting: open {name}: {e}"))
            .ok()
    }

    /// One `live.jsonl` line; the speaker joins [`Live::speakers`] on first hearing.
    pub fn log_utterance(&mut self, u: &Utterance) {
        let who = u.who();
        if !self.speakers.contains(&who) {
            self.speakers.push(who);
        }
        Self::append(self.live_log.as_mut(), u);
    }

    /// One `briefs.jsonl` line.
    pub fn log_record(&mut self, r: &Record) {
        Self::append(self.briefs_log.as_mut(), r);
    }

    fn append<T: serde::Serialize>(f: Option<&mut File>, v: &T) {
        if let (Some(f), Ok(mut line)) = (f, serde_json::to_string(v)) {
            line.push('\n');
            if let Err(e) = f.write_all(line.as_bytes()) {
                tracing::warn!("meeting: append log: {e}");
            }
        }
    }

    /// 57 §11.4: the user tapped or dismissed `topic` — amend every captured example behind it,
    /// once; the topic is gone from the live set after this.
    pub fn react(&mut self, topic: TopicId, reaction: Reaction) {
        self.captured.retain(|(t, id)| {
            if *t == topic {
                crate::oracle::amend(*id, Amendment::User(reaction));
            }
            *t != topic
        });
    }

    /// Remember a steer that a brief may answer.
    pub fn prompted(&mut self, prompt: Prompted, text: impl Into<String>) {
        self.expire_prompts();
        self.outstanding.push(Outstanding {
            at: Instant::now(),
            prompt,
            text: text.into(),
            nudged: false,
        });
    }

    /// Take the outstanding prompt `pick` matches, oldest first.
    pub fn answer(&mut self, pick: impl Fn(Prompted) -> bool) -> Option<Prompted> {
        self.expire_prompts();
        let i = self.outstanding.iter().position(|o| pick(o.prompt))?;
        Some(self.outstanding.remove(i).prompt)
    }

    /// A turn ended: the oldest request or deeper the model neither answered with a brief nor was
    /// reminded of, marked reminded. Taps are exempt (the precomputed brief already showed; the
    /// prompt block lets the model stay silent). At most one nudge per prompt — no loop.
    pub fn unanswered(&mut self) -> Option<(Prompted, String)> {
        self.expire_prompts();
        let o = self
            .outstanding
            .iter_mut()
            .find(|o| !o.nudged && !matches!(o.prompt, Prompted::Tap(_)))?;
        o.nudged = true;
        Some((o.prompt, o.text.clone()))
    }

    fn expire_prompts(&mut self) {
        let now = Instant::now();
        self.outstanding
            .retain(|o| now.duration_since(o.at) < PROMPT_TTL);
    }
}

impl Recording {
    /// Start the clock now; open `live.jsonl` and `briefs.jsonl` in `dir`.
    pub fn new(manifest: Manifest, capture: Capture, cfg: &MeetingConfig, dir: &Path) -> Self {
        let now = Instant::now();
        Self {
            manifest,
            clock: RecordingClock::start_at(now),
            capture,
            tasks: Vec::new(),
            live: Live {
                batcher: Batcher::new(cfg.batch_secs, cfg.batch_utterances, now),
                topics: Topics::new(cfg.topic_ttl_secs, cfg.max_topics),
                turn_running: false,
                approvals: HashSet::new(),
                cursor: None,
                held: false,
                held_flag: Arc::new(AtomicBool::new(false)),
                outstanding: Vec::new(),
                stt: Vec::new(),
                diar: None,
                labels: LiveLabels::default(),
                held_system: Vec::new(),
                carry: Vec::new(),
                pending: Vec::new(),
                published: Vec::new(),
                published_topics: Vec::new(),
                captured: Vec::new(),
                speakers: Vec::new(),
                last_head: None,
                live_log: Live::open_log(dir, crate::LIVE_LOG),
                briefs_log: Live::open_log(dir, crate::BRIEFS_LOG),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live() -> Live {
        Live {
            batcher: Batcher::new(25, 6, Instant::now()),
            topics: Topics::new(600, 6),
            turn_running: false,
            approvals: HashSet::new(),
            cursor: None,
            held: false,
            held_flag: Arc::new(AtomicBool::new(false)),
            outstanding: Vec::new(),
            stt: Vec::new(),
            diar: None,
            labels: LiveLabels::default(),
            held_system: Vec::new(),
            carry: Vec::new(),
            pending: Vec::new(),
            published: Vec::new(),
            published_topics: Vec::new(),
            captured: Vec::new(),
            speakers: Vec::new(),
            last_head: None,
            live_log: None,
            briefs_log: None,
        }
    }

    #[test]
    fn an_unanswered_request_is_nudged_once_and_a_brief_settles_it() {
        let mut l = live();
        let t = TopicId(uuid::Uuid::now_v7());
        l.prompted(Prompted::Tap(t), "KEDA");
        l.prompted(Prompted::Request { spoken: false }, "who owns KEDA");
        // Taps never nudge; the request does, once.
        assert_eq!(
            l.unanswered(),
            Some((Prompted::Request { spoken: false }, "who owns KEDA".into()))
        );
        assert_eq!(l.unanswered(), None, "one reminder per prompt");
        // The brief that finally comes still consumes it (source inference unchanged).
        assert_eq!(
            l.answer(|p| matches!(p, Prompted::Request { .. })),
            Some(Prompted::Request { spoken: false })
        );
        assert_eq!(l.answer(|p| p == Prompted::Tap(t)), Some(Prompted::Tap(t)));
        assert!(l.outstanding.is_empty());
    }

    #[test]
    fn pauses_are_gaps_and_do_not_advance_the_clock() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut c = RecordingClock::start_at(t0);
        assert_eq!(c.elapsed_at(ms(1000)).as_millis(), 1000);
        c.pause_at(ms(1000));
        c.pause_at(ms(1500)); // idempotent
        assert_eq!(
            c.elapsed_at(ms(3000)).as_millis(),
            1000,
            "frozen while paused"
        );
        let gap = c.resume_at(ms(6000)).expect("a gap closes");
        assert_eq!((gap.at_ms, gap.dur_ms), (1000, 5000));
        assert!(c.resume_at(ms(6000)).is_none(), "not paused → no gap");
        assert_eq!(c.elapsed_at(ms(7000)).as_millis(), 2000);
        c.pause_at(ms(7000));
        let gap = c.resume_at(ms(7500)).unwrap();
        assert_eq!((gap.at_ms, gap.dur_ms), (2000, 500));
        assert_eq!(c.elapsed_at(ms(8000)).as_millis(), 2500);
    }
}
