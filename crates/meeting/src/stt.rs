//! Live STT (02 §2): one blocking task per track drains that track's STT ring
//! into a [`voice::live::Utterer`] and reports text pieces to the actor with
//! receipt `Instant`s, which the actor maps onto the recording clock. Pause
//! and stop are the capture's flags; push-to-talk release is a flag the actor
//! sets. A model failure ends the task (`SttDown`); the recording continues.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use kameo::actor::ActorRef;
use rtrb::Consumer;
use voice::live::Utterer;

use crate::capture::{RATE, Sample};
use crate::{Degradation, Degrade, Meeting, Track};

/// Sleep when the ring is empty (a chunk lands every 20 ms).
const IDLE: Duration = Duration::from_millis(10);

/// `tell` to the actor: a text piece of one track. `at` is the receipt stamp
/// of the newest audio the piece saw; the utterance began `behind` before it.
pub struct Piece {
    /// Which side spoke.
    pub track: Track,
    /// Receipt stamp of the newest audio the piece saw.
    pub at: Instant,
    /// How long before `at` the utterance began.
    pub behind: Duration,
    /// Everything said so far in the utterance, trimmed.
    pub text: String,
    /// The utterance closed.
    pub is_final: bool,
    /// Closed by (or right after) a push-to-talk release; mic only.
    pub request: bool,
}

/// What the task needs besides its ring.
pub struct Flags {
    /// Capture's stop: exit, after flushing the open utterance.
    pub stop: Arc<AtomicBool>,
    /// Capture's pause: stop feeding, reset; on resume discard the ring.
    pub paused: Arc<AtomicBool>,
    /// Push-to-talk held (`PushToTalk`); `None` for the system track.
    pub held: Option<Arc<AtomicBool>>,
}

/// Mic utterances that closed while push-to-talk was held: reported as one
/// request on release (53 decision 10), not batched one by one.
#[derive(Default)]
struct Held {
    /// Receipt stamp and `behind` of the first deferred utterance.
    start: Option<(Instant, Duration)>,
    text: String,
}

impl Held {
    fn push(&mut self, p: Piece) {
        let text = p.text.trim();
        if text.is_empty() {
            return;
        }
        if self.start.is_none() {
            self.start = Some((p.at, p.behind));
        }
        if !self.text.is_empty() {
            self.text.push(' ');
        }
        self.text.push_str(text);
    }

    /// The one request piece, ending at `at`; `None` if nothing was said.
    fn take(&mut self, track: Track, at: Instant) -> Option<Piece> {
        let (first_at, first_behind) = self.start.take()?;
        let text = std::mem::take(&mut self.text);
        if text.trim().is_empty() {
            return None;
        }
        Some(Piece {
            track,
            at,
            behind: at.saturating_duration_since(first_at) + first_behind,
            text,
            is_final: true,
            request: true,
        })
    }
}

/// Run one track until stop; the pieces of the final flush (the actor has
/// stopped taking `tell`s by then). Blocking: call from `spawn_blocking`.
pub fn run(
    track: Track,
    mut ring: Consumer<Sample>,
    mut live: Utterer,
    flags: Flags,
    me: ActorRef<Meeting>,
) -> anyhow::Result<Vec<Piece>> {
    live.set_rate(RATE)?;
    let mut last = Instant::now();
    let mut was_paused = false;
    let mut was_held = false;
    let mut held = Held::default();
    let piece = |t: voice::live::Text, at: Instant, position: usize, request: bool| Piece {
        track,
        at,
        behind: Duration::from_secs_f64(
            position.saturating_sub(t.start) as f64 / f64::from(voice::live::MODEL_RATE),
        ),
        text: t.text,
        is_final: t.is_final,
        request,
    };
    let report = |pieces: Vec<Piece>| {
        for p in pieces {
            if me.tell(p).blocking_send().is_err() {
                return false;
            }
        }
        true
    };
    loop {
        if flags.stop.load(Ordering::SeqCst) {
            break;
        }
        let paused = flags.paused.load(Ordering::SeqCst);
        if paused != was_paused {
            was_paused = paused;
            if paused {
                // Nothing more arrives; close what is open and reset the session.
                let texts = live.close()?;
                let position = live.position();
                report(
                    texts
                        .into_iter()
                        .map(|t| piece(t, last, position, false))
                        .collect(),
                );
                live.reset();
            } else {
                // Whatever the producer pushed before its flag flipped is stale.
                while ring.pop().is_ok() {}
                live.reset();
            }
        }
        let is_held = flags
            .held
            .as_ref()
            .is_some_and(|h| h.load(Ordering::SeqCst));
        if was_held && !is_held {
            // Release: whatever is open closes now and joins the held span (53 decision 10).
            let texts = live.close()?;
            let position = live.position();
            for t in texts.into_iter().filter(|t| t.is_final) {
                held.push(piece(t, last, position, true));
            }
            if let Some(p) = held.take(track, last) {
                report(vec![p]);
            }
            live.reset();
        }
        was_held = is_held;
        let Ok((at, pcm)) = ring.pop() else {
            std::thread::sleep(IDLE);
            continue;
        };
        if paused {
            continue;
        }
        last = at;
        let texts = live.chunk(&pcm)?;
        if texts.is_empty() {
            continue;
        }
        let position = live.position();
        let mut pieces = Vec::new();
        for t in texts {
            let p = piece(t, at, position, false);
            if is_held && p.is_final {
                // Partials still show; the final waits for the release.
                held.push(p);
            } else {
                pieces.push(p);
            }
        }
        if !report(pieces) {
            break;
        }
    }
    let texts = live.close()?;
    let position = live.position();
    Ok(texts
        .into_iter()
        .map(|t| piece(t, last, position, false))
        .collect())
}

/// Spawn the task for `track`; `Degradation::SttDown` on a model error or
/// panic. Resolves to the pieces of the final flush at stop.
pub fn spawn(
    track: Track,
    ring: Consumer<Sample>,
    live: Utterer,
    flags: Flags,
    me: ActorRef<Meeting>,
) -> tokio::task::JoinHandle<Vec<Piece>> {
    tokio::spawn(async move {
        let actor = me.clone();
        let r = tokio::task::spawn_blocking(move || run(track, ring, live, flags, me)).await;
        match r {
            Ok(Ok(pieces)) => pieces,
            Ok(Err(e)) => {
                tracing::warn!(?track, "meeting: live STT failed: {e:#}");
                let _ = actor.tell(Degrade(Degradation::SttDown(track))).await;
                Vec::new()
            }
            Err(e) => {
                tracing::warn!(?track, "meeting: live STT task panicked: {e}");
                let _ = actor.tell(Degrade(Degradation::SttDown(track))).await;
                Vec::new()
            }
        }
    })
}
