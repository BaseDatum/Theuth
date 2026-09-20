//! The live meeting: the recording clock (wall time minus pauses — the one
//! timeline every phase stamps with, 53 §3.2) and the handles a [`Recording`]
//! owns.

use std::time::{Duration, Instant};

use crate::capture::Capture;
use crate::store::{Gap, Manifest};

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
/// every change), clock, devices and the timers riding along (levels,
/// max-length, disk). `StopMeeting` aborts the timers when it takes it apart.
pub struct Recording {
    /// Truth; `gaps` and `degraded` live here, nowhere else.
    pub manifest: Manifest,
    /// Recording time.
    pub clock: RecordingClock,
    /// The three threads and the rings.
    pub capture: Capture,
    /// Level, max-length and disk tasks.
    pub tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Recording {
    /// Start the clock now.
    pub fn new(manifest: Manifest, capture: Capture) -> Self {
        Self {
            manifest,
            clock: RecordingClock::start(),
            capture,
            tasks: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
