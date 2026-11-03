//! 173: the capture watchdog — a pure state machine the capture threads drive with chunks,
//! flexaudio's events and a periodic tick (plus the source's activity, asked lazily), that
//! decides when to reopen the stream or fall back, and reports capture gaps and the health
//! the user sees. No I/O and no clock of its own: every call takes `now`.
//!
//! An *episode* runs from the first silent moment (last chunk for a stall, first exact-zero
//! chunk for the zero rule, the error for a fault) until non-zero audio comes back — or, with
//! chunks flowing, until the source is seen not playing (genuine silence). Each episode is one
//! [`Note::Gap`]. Health is `Ok` until a reopen fails (or at once for permission / terminal).

use std::time::{Duration, Instant};

use crate::{CaptureFault, ProcessInfo};

/// No chunk for this long is a stall.
pub const STALL: Duration = Duration::from_secs(3);
/// flexaudio announced `StreamStalled`: its own reopen gets this long (from the
/// announcement) to produce `StreamRecovered` before we reopen ourselves.
pub const FLEX_GRACE: Duration = Duration::from_secs(5);
/// Delay before the next reopen after the n-th failed one; the last repeats forever.
pub const BACKOFF: [Duration; 5] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(30),
];
/// The backoff starts over after this long without an episode.
pub const HEALTHY: Duration = Duration::from_secs(30);
/// Exact zeros this long while the source plays → reopen.
pub const ZEROS: Duration = Duration::from_secs(15);
/// Exact zeros this long while the source's activity is unknown → reopen…
pub const ZEROS_UNKNOWN: Duration = Duration::from_secs(30);
/// …at most this often.
pub const UNKNOWN_EVERY: Duration = Duration::from_secs(120);
/// Consecutive reopens without non-zero audio, source playing, before the fallback.
pub const FALLBACK_AFTER: u32 = 3;

/// Whether the tapped source is rendering (`kAudioProcessPropertyIsRunningOutput`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    /// Playing: zeros from it are a broken tap.
    Active,
    /// Not playing: zeros are genuine silence.
    Inactive,
    /// The query failed.
    Unknown,
}

/// The flexaudio events the watchdog acts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signal {
    /// `StreamStalled` / `RecoverableError`: flexaudio is reopening by itself.
    Stalled,
    /// `StreamRecovered`.
    Recovered,
    /// `SilenceWhileSourceActive`: the current zero run is from a playing source.
    SilentWhileActive,
    /// `DeviceLost` / `Error`, or the mic's device error: the stream is gone; reopen now.
    Lost,
    /// `PermissionDenied`: gone, warn at once, retry with backoff.
    PermissionDenied,
    /// `TerminalError`: gone, warn at once, retry with backoff.
    Terminal,
}

/// What the capture thread must do now; it answers with [`Watchdog::reopened`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Tear the stream down and open it again (re-resolving the target).
    Reopen,
    /// Give up the app tap: open `SystemLoopback` for the rest of the meeting.
    Fallback,
}

/// [`crate::CaptureHealth`] before the wall clock is attached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Health {
    /// See [`crate::CaptureHealth::Ok`].
    Ok,
    /// See [`crate::CaptureHealth::Recovering`].
    Recovering(CaptureFault, Instant),
    /// See [`crate::CaptureHealth::Lost`].
    Lost(CaptureFault, Instant),
    /// See [`crate::CaptureHealth::Degraded`].
    Degraded(CaptureFault),
}

/// What the watchdog reports, drained with [`Watchdog::take_notes`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Note {
    /// An episode ended: the track was silence-padded over `start..end`.
    Gap {
        /// First silent moment.
        start: Instant,
        /// Audio back (or the meeting ended).
        end: Instant,
        /// Why.
        reason: CaptureFault,
        /// The pid the tap was moved to during the episode.
        retarget: Option<u32>,
    },
    /// The user-facing health changed.
    Health(Health),
}

/// Which rules apply to a track.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rules {
    /// The exact-zero rule (not for the mic: a muted mic is legitimately zero).
    pub zeros: bool,
    /// May fall back to `SystemLoopback` (the per-app tap only).
    pub fallback: bool,
}

struct Episode {
    start: Instant,
    reason: CaptureFault,
    /// Reopens in this episode that did not bring audio back.
    failed: u32,
    /// The next reopen is due at.
    due: Option<Instant>,
    /// The last (successful) reopen is judged at.
    judge: Option<Instant>,
    /// Health shows it.
    warned: bool,
    /// Permission / terminal: `Lost` at once.
    urgent: bool,
    retarget: Option<u32>,
}

/// One track's watchdog.
pub struct Watchdog {
    rules: Rules,
    last_chunk: Instant,
    /// Start of the current exact-zero run.
    zeros_since: Option<Instant>,
    /// flexaudio vouched the zero run is from a playing source.
    silent_hint: bool,
    /// flexaudio announced `StreamStalled` at.
    flex_stalled: Option<Instant>,
    /// The stream is gone (fault or failed reopen); no chunks until a reopen succeeds.
    dead: bool,
    episode: Option<Episode>,
    /// Consecutive failed reopens across episodes; non-zero audio resets it.
    streak: u32,
    /// Index into [`BACKOFF`] of the next delay.
    backoff: usize,
    /// The last episode closed at (for the [`HEALTHY`] backoff reset).
    closed_at: Option<Instant>,
    /// The last reopen made while the activity was unknown.
    last_unknown: Option<Instant>,
    /// Why the fallback happened, once it did.
    fell_back: Option<CaptureFault>,
    health: Health,
    notes: Vec<Note>,
    /// Reopens (and fallbacks) asked for so far.
    pub reopens: u32,
}

impl Watchdog {
    /// A watchdog for a stream that opened at `now`.
    pub fn new(now: Instant, rules: Rules) -> Self {
        Self {
            rules,
            last_chunk: now,
            zeros_since: None,
            silent_hint: false,
            flex_stalled: None,
            dead: false,
            episode: None,
            streak: 0,
            backoff: 0,
            closed_at: None,
            last_unknown: None,
            fell_back: None,
            health: Health::Ok,
            notes: Vec::new(),
            reopens: 0,
        }
    }

    /// A chunk arrived; `silent` = every sample is exactly 0.0.
    pub fn chunk(&mut self, now: Instant, silent: bool) {
        self.last_chunk = now;
        self.dead = false;
        self.flex_stalled = None;
        if silent && self.rules.zeros {
            if self.zeros_since.is_none() {
                self.zeros_since = Some(now);
                self.silent_hint = false;
            }
            return;
        }
        self.zeros_since = None;
        self.silent_hint = false;
        self.streak = 0;
        self.close(now);
        self.update_health();
    }

    /// flexaudio said something.
    pub fn signal(&mut self, now: Instant, s: Signal) {
        match s {
            Signal::Stalled => {
                self.flex_stalled.get_or_insert(now);
            }
            Signal::Recovered => self.flex_stalled = None,
            Signal::SilentWhileActive => self.silent_hint = true,
            Signal::Lost => self.fault(now, CaptureFault::Failed, false),
            Signal::PermissionDenied => self.fault(now, CaptureFault::PermissionDenied, true),
            Signal::Terminal => self.fault(now, CaptureFault::Failed, true),
        }
        self.update_health();
    }

    /// Periodic check (every few hundred ms). `activity` is only asked when a decision needs it.
    pub fn tick(&mut self, now: Instant, activity: &mut dyn FnMut() -> Activity) -> Option<Action> {
        if self.episode.is_none() {
            if self
                .closed_at
                .is_some_and(|t| now.saturating_duration_since(t) >= HEALTHY)
            {
                self.backoff = 0;
                self.closed_at = None;
            }
            self.detect(now, activity);
        }
        let action = self.advance(now, activity);
        self.update_health();
        action
    }

    /// The capture thread carried out an [`Action`]: `ok` = the stream opened; `retarget` =
    /// it now taps that pid instead.
    pub fn reopened(&mut self, now: Instant, ok: bool, retarget: Option<u32>) {
        let delay = if ok { None } else { Some(self.next_delay()) };
        let Some(ep) = self.episode.as_mut() else {
            return;
        };
        if retarget.is_some() {
            ep.retarget = retarget;
        }
        match delay {
            None => {
                self.dead = false;
                self.last_chunk = now;
                self.zeros_since = None;
                self.silent_hint = false;
                self.flex_stalled = None;
                ep.judge = Some(now + STALL);
            }
            Some(delay) => {
                self.dead = true;
                ep.failed += 1;
                ep.warned = true;
                ep.due = Some(now + delay);
                self.streak += 1;
            }
        }
        self.update_health();
    }

    /// The meeting stops: an open episode ends here.
    pub fn finish(&mut self, now: Instant) {
        self.close(now);
    }

    /// The notes since the last call.
    pub fn take_notes(&mut self) -> Vec<Note> {
        std::mem::take(&mut self.notes)
    }

    /// Whether an episode is open (for the health line).
    pub fn troubled(&self) -> bool {
        self.episode.is_some()
    }

    /// The episode reason the fallback was taken for, once it was.
    pub fn fell_back(&self) -> Option<CaptureFault> {
        self.fell_back
    }

    fn detect(&mut self, now: Instant, activity: &mut dyn FnMut() -> Activity) {
        if self.dead {
            return;
        }
        if now.saturating_duration_since(self.last_chunk) >= STALL {
            let due = self.flex_stalled.map_or(now, |t| (t + FLEX_GRACE).max(now));
            self.open(self.last_chunk, CaptureFault::Stalled, due);
            return;
        }
        if !self.rules.zeros {
            return;
        }
        let Some(z) = self.zeros_since else {
            return;
        };
        let run = now.saturating_duration_since(z);
        if run < ZEROS {
            return;
        }
        match self.activity(activity) {
            Activity::Active => self.open(z, CaptureFault::Silent, now),
            Activity::Unknown if run >= ZEROS_UNKNOWN && self.unknown_allowed(now) => {
                self.open(z, CaptureFault::Silent, now)
            }
            _ => {}
        }
    }

    fn advance(&mut self, now: Instant, activity: &mut dyn FnMut() -> Activity) -> Option<Action> {
        self.episode.as_ref()?;
        let flowing = !self.dead && now.saturating_duration_since(self.last_chunk) < STALL;
        // Chunks flow (zeros, or the episode would be closed) but the source is quiet:
        // genuine silence, nothing to recover.
        if flowing && self.rules.zeros && self.activity(activity) == Activity::Inactive {
            self.close(now);
            return None;
        }
        if let Some(j) = self.episode.as_ref().and_then(|e| e.judge)
            && now >= j
        {
            match self.zeros_since {
                // Zeros since the reopen: a playing source gets a full zero window.
                Some(z) if flowing && now.saturating_duration_since(z) < ZEROS => {
                    self.episode.as_mut()?.judge = Some(z + ZEROS);
                }
                _ => {
                    let delay = self.next_delay();
                    let ep = self.episode.as_mut()?;
                    ep.judge = None;
                    ep.failed += 1;
                    ep.warned = true;
                    ep.due = Some(now + delay);
                    self.streak += 1;
                }
            }
        }
        let due = self.episode.as_ref()?.due?;
        if now < due {
            return None;
        }
        let act = self.activity(activity);
        if flowing && act == Activity::Unknown && !self.unknown_allowed(now) {
            let next = self.last_unknown? + UNKNOWN_EVERY;
            self.episode.as_mut()?.due = Some(next);
            return None;
        }
        if act == Activity::Unknown {
            self.last_unknown = Some(now);
        }
        let ep = self.episode.as_mut()?;
        ep.due = None;
        ep.judge = None;
        self.reopens += 1;
        if self.rules.fallback
            && self.fell_back.is_none()
            && self.streak >= FALLBACK_AFTER
            && ep.reason != CaptureFault::PermissionDenied
            && act == Activity::Active
        {
            self.fell_back = Some(ep.reason);
            return Some(Action::Fallback);
        }
        Some(Action::Reopen)
    }

    /// The source's activity; flexaudio's `SilenceWhileSourceActive` counts as playing.
    fn activity(&self, activity: &mut dyn FnMut() -> Activity) -> Activity {
        if self.silent_hint {
            Activity::Active
        } else {
            activity()
        }
    }

    fn unknown_allowed(&self, now: Instant) -> bool {
        self.last_unknown
            .is_none_or(|t| now.saturating_duration_since(t) >= UNKNOWN_EVERY)
    }

    fn next_delay(&mut self) -> Duration {
        let d = BACKOFF[self.backoff.min(BACKOFF.len() - 1)];
        self.backoff += 1;
        d
    }

    fn open(&mut self, start: Instant, reason: CaptureFault, due: Instant) {
        self.episode = Some(Episode {
            start,
            reason,
            failed: 0,
            due: Some(due),
            judge: None,
            warned: false,
            urgent: false,
            retarget: None,
        });
        self.closed_at = None;
    }

    /// The stream is gone: open (or escalate) an episode; `urgent` warns at once and waits
    /// a backoff step before retrying, otherwise the reopen is immediate.
    fn fault(&mut self, now: Instant, reason: CaptureFault, urgent: bool) {
        self.dead = true;
        if self.episode.is_none() {
            let start = self.zeros_since.unwrap_or(self.last_chunk).min(now);
            self.open(start, reason, now);
        }
        let delay = if urgent {
            Some(self.next_delay())
        } else {
            None
        };
        let ep = self.episode.as_mut().expect("opened above");
        ep.judge = None;
        if urgent {
            ep.reason = reason;
            ep.warned = true;
            ep.urgent = true;
        }
        if ep.due.is_none() || urgent {
            ep.due = Some(delay.map_or(now, |d| now + d));
        }
    }

    fn close(&mut self, now: Instant) {
        if let Some(ep) = self.episode.take() {
            self.notes.push(Note::Gap {
                start: ep.start,
                end: now.max(ep.start),
                reason: ep.reason,
                retarget: ep.retarget,
            });
            self.closed_at = Some(now);
        }
    }

    fn update_health(&mut self) {
        let h = match &self.episode {
            Some(ep) if ep.warned && (ep.urgent || ep.failed >= FALLBACK_AFTER) => {
                Health::Lost(ep.reason, ep.start)
            }
            Some(ep) if ep.warned => Health::Recovering(ep.reason, ep.start),
            _ => self.fell_back.map_or(Health::Ok, Health::Degraded),
        };
        if h != self.health {
            self.health = h;
            self.notes.push(Note::Health(h));
        }
    }
}

/// The source's activity in `procs`: `pid`'s, or any same-bundle process's; `None` (the
/// global tap) = any process at all (`procs` already lacks our own pid).
pub fn activity(pid: Option<u32>, bundle: Option<&str>, procs: &[ProcessInfo]) -> Activity {
    let playing = procs.iter().filter(|p| p.active).any(|p| match pid {
        None => true,
        Some(pid) => p.pid == pid || bundle.is_some_and(|b| p.bundle_id.as_deref() == Some(b)),
    });
    if playing {
        Activity::Active
    } else {
        Activity::Inactive
    }
}

/// The pid to tap on a reopen: `pid` while it plays (or nothing else of its bundle does),
/// else a same-bundle process that is playing.
pub fn retarget(pid: u32, bundle: Option<&str>, procs: &[ProcessInfo]) -> u32 {
    if procs.iter().any(|p| p.pid == pid && p.active) {
        return pid;
    }
    bundle
        .and_then(|b| {
            procs
                .iter()
                .find(|p| p.active && p.bundle_id.as_deref() == Some(b))
        })
        .map_or(pid, |p| p.pid)
}

/// One track's counters for the once-a-minute health line.
#[derive(Debug, Default)]
pub struct Meter {
    /// Chunks.
    pub chunks: u64,
    /// Frames (samples, mono).
    pub frames: u64,
    /// Frames that were exactly 0.0.
    pub zero_frames: u64,
    /// Largest |sample|.
    pub peak: f32,
    /// flexaudio `ChunkDropped` counts.
    pub dropped: u64,
    /// flexaudio `Clipped` events.
    pub clipped: u64,
}

impl Meter {
    /// Count one chunk; whether every sample was exactly 0.0.
    pub fn add(&mut self, pcm: &[f32]) -> bool {
        let zeros = pcm.iter().filter(|&&s| s == 0.0).count() as u64;
        self.chunks += 1;
        self.frames += pcm.len() as u64;
        self.zero_frames += zeros;
        self.peak = pcm.iter().fold(self.peak, |m, s| m.max(s.abs()));
        zeros == pcm.len() as u64
    }

    /// Exact-zero fraction in percent.
    pub fn zero_pct(&self) -> f64 {
        100.0 * self.zero_frames as f64 / self.frames.max(1) as f64
    }

    /// Peak in dBFS (−200 for digital silence).
    pub fn peak_dbfs(&self) -> f64 {
        20.0 * f64::from(self.peak.max(1e-10)).log10()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SYSTEM: Rules = Rules {
        zeros: true,
        fallback: true,
    };
    const MIC: Rules = Rules {
        zeros: false,
        fallback: false,
    };

    /// Drives a watchdog on a fake clock: `ms` since `t0`.
    struct Rig {
        t0: Instant,
        now: Duration,
        wd: Watchdog,
        activity: Activity,
        actions: Vec<(u64, Action)>,
        notes: Vec<Note>,
    }

    impl Rig {
        fn new(rules: Rules) -> Self {
            let t0 = Instant::now();
            Self {
                t0,
                now: Duration::ZERO,
                wd: Watchdog::new(t0, rules),
                activity: Activity::Active,
                actions: Vec::new(),
                notes: Vec::new(),
            }
        }

        fn at(&self, ms: u64) -> Instant {
            self.t0 + Duration::from_millis(ms)
        }

        fn ms(&self) -> u64 {
            self.now.as_millis() as u64
        }

        /// Run until `ms`: a chunk every 20 ms when `chunks` (`zero` = all-zero), a tick every
        /// 250 ms; every action is answered with `reopen_ok`.
        fn run(&mut self, ms: u64, chunks: bool, zero: bool, reopen_ok: bool) {
            while self.ms() < ms {
                self.now += Duration::from_millis(10);
                let now = self.t0 + self.now;
                let t = self.ms();
                if chunks && t.is_multiple_of(20) {
                    self.wd.chunk(now, zero);
                }
                if t.is_multiple_of(250) {
                    let a = self.activity;
                    if let Some(action) = self.wd.tick(now, &mut || a) {
                        self.actions.push((t, action));
                        self.wd.reopened(now, reopen_ok, None);
                    }
                }
                self.notes.extend(self.wd.take_notes());
            }
        }

        fn healths(&self) -> Vec<Health> {
            self.notes
                .iter()
                .filter_map(|n| match n {
                    Note::Health(h) => Some(*h),
                    Note::Gap { .. } => None,
                })
                .collect()
        }

        fn gaps(&self) -> Vec<(u64, u64, CaptureFault)> {
            self.notes
                .iter()
                .filter_map(|n| match *n {
                    Note::Gap {
                        start, end, reason, ..
                    } => Some((
                        start.duration_since(self.t0).as_millis() as u64,
                        end.duration_since(self.t0).as_millis() as u64,
                        reason,
                    )),
                    Note::Health(_) => None,
                })
                .collect()
        }
    }

    #[test]
    fn healthy_stream_does_nothing() {
        let mut r = Rig::new(SYSTEM);
        r.run(120_000, true, false, true);
        assert!(r.actions.is_empty());
        assert!(r.notes.is_empty());
    }

    #[test]
    fn stall_reopens_and_recovers_without_a_warning() {
        let mut r = Rig::new(SYSTEM);
        r.run(10_000, true, false, true);
        r.run(13_000, false, false, true);
        assert_eq!(r.actions, vec![(13_000, Action::Reopen)]);
        r.run(14_000, false, false, true);
        r.run(20_000, true, false, true);
        assert_eq!(r.actions.len(), 1);
        assert_eq!(r.gaps(), vec![(10_000, 14_020, CaptureFault::Stalled)]);
        assert!(r.healths().is_empty(), "one reopen that worked: no warning");
    }

    #[test]
    fn flexaudio_recovery_needs_no_reopen_of_ours() {
        let mut r = Rig::new(SYSTEM);
        r.run(10_000, true, false, true);
        r.wd.signal(r.at(12_000), Signal::Stalled);
        r.run(14_000, false, false, true);
        // Its grace runs to 12 s + 5 s; it recovers at 15 s.
        r.run(15_000, false, false, true);
        r.wd.signal(r.at(15_000), Signal::Recovered);
        r.run(20_000, true, false, true);
        assert!(r.actions.is_empty());
        assert_eq!(r.gaps(), vec![(10_000, 15_020, CaptureFault::Stalled)]);
        assert!(r.healths().is_empty());
    }

    #[test]
    fn flexaudio_grace_expires_then_we_reopen() {
        let mut r = Rig::new(SYSTEM);
        r.run(10_000, true, false, true);
        r.wd.signal(r.at(12_000), Signal::Stalled);
        r.run(18_000, false, false, true);
        assert_eq!(r.actions, vec![(17_000, Action::Reopen)]);
    }

    #[test]
    fn stall_backoff_warns_after_first_failed_reopen_then_resets() {
        // The global tap: the backoff alone, no fallback after the third failure.
        let mut r = Rig::new(Rules {
            zeros: true,
            fallback: false,
        });
        r.run(1_000, true, false, true);
        r.run(70_000, false, false, true);
        let at: Vec<u64> = r.actions.iter().map(|a| a.0).collect();
        // Stall at 4 s; each reopen is judged 3 s later, then 1, 2, 5, 10, 30 s of backoff.
        assert_eq!(at, vec![4_000, 8_000, 13_000, 21_000, 34_000, 67_000]);
        let h = r.healths();
        assert_eq!(
            h[0],
            Health::Recovering(CaptureFault::Stalled, r.at(1_000)),
            "warned at the first failed judgement (7 s), not before"
        );
        assert_eq!(h[1], Health::Lost(CaptureFault::Stalled, r.at(1_000)));
        // Audio back; after 30 s healthy the backoff starts over.
        r.run(110_000, true, false, true);
        assert_eq!(r.healths().last(), Some(&Health::Ok));
        r.actions.clear();
        r.run(150_000, false, false, true);
        let at: Vec<u64> = r.actions.iter().map(|a| a.0).collect();
        assert_eq!(at, vec![113_000, 117_000, 122_000, 130_000, 143_000]);
    }

    #[test]
    fn zeros_while_active_reopen_after_15_s() {
        let mut r = Rig::new(SYSTEM);
        r.run(10_000, true, false, true);
        r.run(26_000, true, true, true);
        assert_eq!(r.actions, vec![(25_250, Action::Reopen)]);
        r.run(30_000, true, false, true);
        assert_eq!(r.gaps(), vec![(10_020, 26_020, CaptureFault::Silent)]);
    }

    #[test]
    fn zeros_while_inactive_is_genuine_silence() {
        let mut r = Rig::new(SYSTEM);
        r.activity = Activity::Inactive;
        r.run(10_000, true, false, true);
        r.run(300_000, true, true, true);
        assert!(r.actions.is_empty());
        assert!(r.notes.is_empty());
    }

    #[test]
    fn source_going_quiet_ends_a_zero_episode() {
        let mut r = Rig::new(SYSTEM);
        r.run(1_000, true, false, true);
        r.run(20_000, true, true, true);
        assert_eq!(r.actions.len(), 1);
        r.activity = Activity::Inactive;
        r.run(21_000, true, true, true);
        assert_eq!(r.gaps(), vec![(1_020, 20_250, CaptureFault::Silent)]);
        r.run(200_000, true, true, true);
        assert_eq!(r.actions.len(), 1);
    }

    #[test]
    fn unknown_activity_waits_30_s_and_reopens_at_most_every_2_min() {
        let mut r = Rig::new(SYSTEM);
        r.activity = Activity::Unknown;
        r.run(1_000, true, false, true);
        r.run(400_000, true, true, true);
        let at: Vec<u64> = r.actions.iter().map(|a| a.0).collect();
        // 30 s after the first zero chunk (1.02 s), then every 2 min.
        assert_eq!(at, vec![31_250, 151_250, 271_250, 391_250]);
        assert!(
            r.actions.iter().all(|a| a.1 == Action::Reopen),
            "no fallback without a playing source"
        );
    }

    #[test]
    fn three_failed_reopens_while_active_fall_back() {
        let mut r = Rig::new(SYSTEM);
        r.run(1_000, true, false, true);
        r.run(75_000, true, true, true);
        let kinds: Vec<Action> = r.actions.iter().map(|a| a.1).collect();
        assert_eq!(
            kinds,
            vec![
                Action::Reopen,
                Action::Reopen,
                Action::Reopen,
                Action::Fallback
            ]
        );
        // 15 s of zeros, then a fresh 15 s zero window after each reopen plus 1, 2, 5 s of backoff.
        let at: Vec<u64> = r.actions.iter().map(|a| a.0).collect();
        assert_eq!(at, vec![16_250, 32_500, 49_750, 70_000]);
        // The warning came with the first failed reopen (≈ 31 s), Lost after the third.
        let h = r.healths();
        assert_eq!(h[0], Health::Recovering(CaptureFault::Silent, r.at(1_020)));
        assert_eq!(h[1], Health::Lost(CaptureFault::Silent, r.at(1_020)));
        // The fallback hears audio: degraded, gap closed, no second fallback ever.
        r.run(85_000, true, false, true);
        assert_eq!(
            r.healths().last(),
            Some(&Health::Degraded(CaptureFault::Silent))
        );
        assert_eq!(r.gaps(), vec![(1_020, 75_020, CaptureFault::Silent)]);
        r.run(400_000, true, true, true);
        assert!(r.actions[4..].iter().all(|a| a.1 == Action::Reopen));
    }

    #[test]
    fn global_tap_never_falls_back() {
        let mut r = Rig::new(Rules {
            zeros: true,
            fallback: false,
        });
        r.run(1_000, true, false, true);
        r.run(200_000, true, true, true);
        assert!(r.actions.len() >= 4);
        assert!(r.actions.iter().all(|a| a.1 == Action::Reopen));
    }

    #[test]
    fn permission_denied_warns_at_once_and_retries_with_backoff() {
        let mut r = Rig::new(SYSTEM);
        r.run(5_000, true, false, true);
        r.wd.signal(r.at(5_000), Signal::PermissionDenied);
        r.notes.extend(r.wd.take_notes());
        assert_eq!(
            r.healths(),
            vec![Health::Lost(CaptureFault::PermissionDenied, r.at(5_000))]
        );
        // Reopens keep failing (still denied): 1, 2, 5, 10 s apart.
        r.run(25_000, false, false, false);
        let at: Vec<u64> = r.actions.iter().map(|a| a.0).collect();
        assert_eq!(at, vec![6_000, 8_000, 13_000, 23_000]);
        assert!(
            r.actions.iter().all(|a| a.1 == Action::Reopen),
            "no fallback"
        );
        // Granted mid-meeting: the next reopen (30 s later) opens, audio returns, the warning clears.
        r.run(53_000, false, false, true);
        assert_eq!(r.actions.last(), Some(&(53_000, Action::Reopen)));
        r.run(60_000, true, false, true);
        assert_eq!(r.healths().last(), Some(&Health::Ok));
        assert_eq!(
            r.gaps(),
            vec![(5_000, 53_020, CaptureFault::PermissionDenied)]
        );
    }

    #[test]
    fn terminal_error_is_lost_at_once() {
        let mut r = Rig::new(SYSTEM);
        r.run(5_000, true, false, true);
        r.wd.signal(r.at(5_000), Signal::Terminal);
        r.notes.extend(r.wd.take_notes());
        assert_eq!(
            r.healths(),
            vec![Health::Lost(CaptureFault::Failed, r.at(5_000))]
        );
        r.run(6_000, false, false, true);
        assert_eq!(r.actions, vec![(6_000, Action::Reopen)]);
    }

    #[test]
    fn device_lost_reopens_at_once_without_warning() {
        let mut r = Rig::new(MIC);
        r.run(5_000, true, false, true);
        r.wd.signal(r.at(5_000), Signal::Lost);
        // The stream is gone: no chunks until the reopen.
        r.run(5_300, false, false, true);
        r.run(8_000, true, false, true);
        assert_eq!(r.actions, vec![(5_250, Action::Reopen)]);
        assert_eq!(r.gaps(), vec![(5_000, 5_320, CaptureFault::Failed)]);
        assert!(r.healths().is_empty());
    }

    #[test]
    fn mic_zeros_are_never_a_problem() {
        let mut r = Rig::new(MIC);
        r.run(300_000, true, true, true);
        assert!(r.actions.is_empty());
        assert!(r.notes.is_empty());
    }

    #[test]
    fn open_episode_is_a_gap_at_finish() {
        let mut r = Rig::new(SYSTEM);
        r.run(1_000, true, false, true);
        r.run(9_000, false, false, false);
        r.wd.finish(r.at(9_000));
        r.notes.extend(r.wd.take_notes());
        assert_eq!(r.gaps(), vec![(1_000, 9_000, CaptureFault::Stalled)]);
    }

    #[test]
    fn retarget_follows_the_bundle() {
        let p = |pid, bundle: &str, active| ProcessInfo {
            pid,
            name: "zoom.us".into(),
            bundle_id: Some(bundle.into()),
            active,
        };
        let zoom = Some("us.zoom.xos");
        let moved = [
            p(51437, "us.zoom.xos", false),
            p(60000, "com.other", true),
            p(51500, "us.zoom.xos", true),
        ];
        assert_eq!(retarget(51437, zoom, &moved), 51500);
        assert_eq!(activity(Some(51437), zoom, &moved), Activity::Active);
        // Still playing itself, or the pid gone: keep / find the sibling.
        assert_eq!(retarget(51500, zoom, &moved), 51500);
        assert_eq!(retarget(1, zoom, &moved), 51500);
        // Nothing of the bundle plays: keep the pid; inactive (another app does not count).
        let quiet = [p(51437, "us.zoom.xos", false), p(60000, "com.other", true)];
        assert_eq!(retarget(51437, zoom, &quiet), 51437);
        assert_eq!(activity(Some(51437), zoom, &quiet), Activity::Inactive);
        assert_eq!(retarget(51437, None, &moved), 51437);
        // The global tap: any process.
        assert_eq!(activity(None, None, &quiet), Activity::Active);
        assert_eq!(activity(None, None, &[]), Activity::Inactive);
    }
}
