//! 173: the capture watchdog — a pure state machine the capture threads drive with chunks,
//! flexaudio's events, probe results and a periodic tick (plus the source's activity, asked
//! lazily), that decides when to probe, reopen, fall back or return, and reports capture gaps
//! and the health the user sees. No I/O and no clock of its own: every call takes `now`.
//!
//! An *episode* runs from the first silent moment (last chunk for a stall, first exact-zero
//! chunk for the zero rule, the error for a fault) until non-zero audio comes back — or until
//! the silence is shown to be genuine. Each episode is one [`Note::Gap`]. Health is `Ok` until a
//! reopen fails (or at once for permission / terminal).
//!
//! The zero rule (R2-1): on a per-app tap, 15 s of exact zeros ask for a [`Action::Probe`] — a
//! concurrent 4 s global tap. Audible elsewhere → episode; silent everywhere → genuine silence,
//! re-probed on a 30 / 60 / 120 s schedule. `IsRunningOutput` gates nothing there (the incident:
//! the tapped object stopped running output while the app played from another one). The global
//! tap (pid 0, the fallback) has nothing to probe against and keeps the activity-gated rule, as
//! does a per-app tap whose probe failed to open.

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
/// Exact zeros this long → probe (per-app tap) / reopen if the source plays (global tap).
pub const ZEROS: Duration = Duration::from_secs(15);
/// Exact zeros this long while the source's activity is unknown → reopen…
pub const ZEROS_UNKNOWN: Duration = Duration::from_secs(30);
/// …at most this often.
pub const UNKNOWN_EVERY: Duration = Duration::from_secs(120);
/// Consecutive failed reopens (confirmed by an audible probe, or with the source playing)
/// before the fallback.
pub const FALLBACK_AFTER: u32 = 3;
/// How long a probe listens.
pub const PROBE_WINDOW: Duration = Duration::from_secs(4);
/// After a reopen inside a probed episode: exact zeros this long → judge it with a second probe.
pub const ZERO_JUDGE: Duration = Duration::from_secs(5);
/// Genuine silence (probe silent) is re-probed after these, the last repeating, while the zeros
/// last; non-zero audio resets the schedule.
pub const REPROBE: [Duration; 3] = [
    Duration::from_secs(30),
    Duration::from_secs(60),
    Duration::from_secs(120),
];
/// In the fallback, a return probe of the app's own tap this often.
pub const RETURN_EVERY: Duration = Duration::from_secs(60);
/// A probe 100 ms block counts as audible at this peak or above. Zoom's own natural-pause floor
/// in the 2026-11-02 recording was −88…−95 dB RMS, speech −25…−45 dB.
pub const AUDIBLE_DBFS: f32 = -70.0;
/// Audible elsewhere = at least this much of the probe window in audible blocks.
pub const AUDIBLE_FOR: Duration = Duration::from_secs(1);
/// A return probe brings the app tap back with at least this much non-zero audio.
pub const RETURN_FOR: Duration = Duration::from_millis(500);

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

/// A confirmation probe's verdict ([`Watchdog::probed`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Probe {
    /// Sound is playing somewhere ([`AUDIBLE_FOR`] at [`AUDIBLE_DBFS`]).
    Audible,
    /// Silence everywhere: the zeros are genuine.
    Silent,
    /// The probe stream did not open.
    Failed,
}

/// What the capture thread must do now; it answers with [`Watchdog::reopened`],
/// [`Watchdog::probed`] or [`Watchdog::returned`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Tear the stream down and open it again (re-resolving the target).
    Reopen,
    /// Give up the app tap: open `SystemLoopback` until a return probe succeeds.
    Fallback,
    /// Open a concurrent global tap for [`PROBE_WINDOW`]: is anything audible?
    Probe,
    /// In the fallback: probe the app's tap for [`PROBE_WINDOW`]; back to it on ≥ [`RETURN_FOR`].
    Return,
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
    /// The zero rule is confirmed by a global-tap probe (the per-app tap only).
    pub probe: bool,
}

/// What a running probe is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pending {
    /// 15 s of zeros, no episode yet.
    Confirm,
    /// Zeros after a reopen inside an episode.
    Judge,
    /// The fallback's return probe.
    Return,
}

struct Episode {
    /// Distinguishes episodes, so a probe's verdict reaches only the episode that asked.
    id: u64,
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
    /// An audible probe confirmed it (the fallback needs no activity check then).
    confirmed: bool,
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
    /// Why the fallback happened, while it lasts.
    fell_back: Option<CaptureFault>,
    /// A probe is running: what for, and the episode it judges (0: none).
    probing: Option<(Pending, u64)>,
    /// Episodes opened so far (the next [`Episode::id`]).
    episodes: u64,
    /// The last gap's end: a new episode never starts before it (R3 #16).
    last_gap_end: Option<Instant>,
    /// Genuine silence: the next confirmation probe is due at.
    next_probe: Option<Instant>,
    /// Index into [`REPROBE`].
    probe_step: usize,
    /// A probe failed to open in this zero run: the activity-gated rule applies.
    probe_broken: bool,
    /// In the fallback: the next return probe is due at.
    next_return: Option<Instant>,
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
            probing: None,
            episodes: 0,
            last_gap_end: None,
            next_probe: None,
            probe_step: 0,
            probe_broken: false,
            next_return: None,
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
        self.next_probe = None;
        self.probe_step = 0;
        self.probe_broken = false;
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
        let action = self.decide(now, activity);
        self.update_health();
        action
    }

    /// The capture thread carried out a [`Action::Reopen`] / [`Action::Fallback`]: `ok` = the
    /// stream opened; `retarget` = it now taps that pid instead.
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

    /// The [`Action::Probe`] finished. A verdict applies only if the state it was asked in
    /// still holds (R3 #2): Confirm needs no episode and a live stream; Judge needs the same
    /// live episode that asked. Stale verdicts are dropped — a dead stream's episode is the
    /// reopen schedule's, not the probe's.
    pub fn probed(&mut self, now: Instant, p: Probe) {
        let Some((kind, asked)) = self.probing.take_if(|(k, _)| *k != Pending::Return) else {
            return;
        };
        let live = !self.dead;
        let current = match (kind, &self.episode) {
            (Pending::Confirm, None) => live,
            (Pending::Judge, Some(ep)) => live && ep.id == asked,
            _ => false,
        };
        if !current {
            return;
        }
        match (kind, p) {
            (_, Probe::Failed) => {
                self.probe_broken = true;
                // Judge the reopen again, now by the activity-gated rule.
                if let Some(ep) = self.episode.as_mut() {
                    ep.judge = Some(now);
                }
            }
            (Pending::Confirm, Probe::Audible) => {
                if let Some(z) = self.zeros_since {
                    self.open(z, CaptureFault::Silent, now);
                    if let Some(ep) = self.episode.as_mut() {
                        ep.confirmed = true;
                    }
                }
            }
            (Pending::Judge, Probe::Audible) => {
                if let Some(ep) = self.episode.as_mut() {
                    ep.confirmed = true;
                }
                self.fail_reopen(now);
            }
            (_, Probe::Silent) => {
                self.close(now);
                self.next_probe = Some(now + REPROBE[self.probe_step.min(REPROBE.len() - 1)]);
                self.probe_step += 1;
            }
            (Pending::Return, _) => {}
        }
        self.update_health();
    }

    /// The [`Action::Return`] probe finished; `back` = the capture thread is on the app tap
    /// again (it heard ≥ [`RETURN_FOR`] and the tap reopened).
    pub fn returned(&mut self, now: Instant, back: bool) {
        if !matches!(self.probing, Some((Pending::Return, _))) {
            return;
        }
        self.probing = None;
        if back {
            self.fell_back = None;
            self.next_return = None;
            self.streak = 0;
            self.backoff = 0;
            self.dead = false;
            self.last_chunk = now;
            self.zeros_since = None;
            self.flex_stalled = None;
            self.next_probe = None;
            self.probe_step = 0;
            self.probe_broken = false;
            self.close(now);
        } else {
            self.next_return = Some(now + RETURN_EVERY);
        }
        self.update_health();
    }

    /// The meeting stops: an open episode ends here (dead or not).
    pub fn finish(&mut self, now: Instant) {
        self.dead = false;
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

    /// The episode reason the fallback was taken for, while it lasts.
    pub fn fell_back(&self) -> Option<CaptureFault> {
        self.fell_back
    }

    /// Probes confirm the zero rule (per-app tap, not in the fallback, probe working).
    fn probe_ok(&self) -> bool {
        self.rules.probe && self.fell_back.is_none() && !self.probe_broken
    }

    fn decide(&mut self, now: Instant, activity: &mut dyn FnMut() -> Activity) -> Option<Action> {
        if self.fell_back.is_some()
            && self.probing.is_none()
            && self.next_return.is_some_and(|t| now >= t)
        {
            self.probing = Some((Pending::Return, 0));
            return Some(Action::Return);
        }
        if self.episode.is_none() {
            if self
                .closed_at
                .is_some_and(|t| now.saturating_duration_since(t) >= HEALTHY)
            {
                self.backoff = 0;
                self.closed_at = None;
            }
            if let Some(a) = self.detect(now, activity) {
                return Some(a);
            }
        }
        self.advance(now, activity)
    }

    fn detect(&mut self, now: Instant, activity: &mut dyn FnMut() -> Activity) -> Option<Action> {
        if self.dead {
            return None;
        }
        if now.saturating_duration_since(self.last_chunk) >= STALL {
            let due = self.flex_stalled.map_or(now, |t| (t + FLEX_GRACE).max(now));
            self.open(self.last_chunk, CaptureFault::Stalled, due);
            return None;
        }
        if !self.rules.zeros {
            return None;
        }
        let z = self.zeros_since?;
        let run = now.saturating_duration_since(z);
        if run < ZEROS {
            return None;
        }
        if self.probe_ok() {
            if self.probing.is_none() && self.next_probe.is_none_or(|t| now >= t) {
                self.probing = Some((Pending::Confirm, 0));
                return Some(Action::Probe);
            }
            return None;
        }
        match self.activity(activity) {
            Activity::Active => self.open(z, CaptureFault::Silent, now),
            Activity::Unknown if run >= ZEROS_UNKNOWN && self.unknown_allowed(now) => {
                self.open(z, CaptureFault::Silent, now)
            }
            _ => {}
        }
        None
    }

    fn advance(&mut self, now: Instant, activity: &mut dyn FnMut() -> Activity) -> Option<Action> {
        self.episode.as_ref()?;
        let flowing = !self.dead && now.saturating_duration_since(self.last_chunk) < STALL;
        let probe = self.probe_ok();
        // Activity-gated rule: chunks flow (zeros, or the episode would be closed) but the
        // source is quiet — genuine silence, nothing to recover.
        if flowing && self.rules.zeros && !probe && self.activity(activity) == Activity::Inactive {
            self.close(now);
            return None;
        }
        if let Some(j) = self.episode.as_ref().and_then(|e| e.judge)
            && now >= j
        {
            match self.zeros_since {
                // Probed: 5 s of zeros after the reopen, then a second probe judges it.
                Some(z) if flowing && probe => {
                    if now.saturating_duration_since(z) < ZERO_JUDGE {
                        self.episode.as_mut()?.judge = Some(z + ZERO_JUDGE);
                    } else if self.probing.is_none() {
                        let ep = self.episode.as_mut()?;
                        ep.judge = None;
                        self.probing = Some((Pending::Judge, ep.id));
                        return Some(Action::Probe);
                    }
                }
                // Activity-gated: a playing source gets a full zero window.
                Some(z) if flowing && now.saturating_duration_since(z) < ZEROS => {
                    self.episode.as_mut()?.judge = Some(z + ZEROS);
                }
                _ => self.fail_reopen(now),
            }
        }
        let due = self.episode.as_ref()?.due?;
        if now < due {
            return None;
        }
        let confirmed = self.episode.as_ref()?.confirmed;
        let gated = flowing && !probe;
        let act = if gated || !confirmed {
            self.activity(activity)
        } else {
            Activity::Active
        };
        if gated && act == Activity::Unknown {
            if !self.unknown_allowed(now) {
                let next = self.last_unknown? + UNKNOWN_EVERY;
                self.episode.as_mut()?.due = Some(next);
                return None;
            }
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
            && (ep.confirmed || act == Activity::Active)
        {
            self.fell_back = Some(ep.reason);
            self.next_return = Some(now + RETURN_EVERY);
            return Some(Action::Fallback);
        }
        Some(Action::Reopen)
    }

    /// The last reopen did not bring audio back: count it, warn, schedule the next one —
    /// at once when this failure decides the fallback (R3 #11: no extra backoff step).
    fn fail_reopen(&mut self, now: Instant) {
        let falls_back = self.rules.fallback
            && self.fell_back.is_none()
            && self.streak + 1 >= FALLBACK_AFTER
            && self
                .episode
                .as_ref()
                .is_some_and(|ep| ep.confirmed && ep.reason != CaptureFault::PermissionDenied);
        let delay = if falls_back {
            Duration::ZERO
        } else {
            self.next_delay()
        };
        if let Some(ep) = self.episode.as_mut() {
            ep.judge = None;
            ep.failed += 1;
            ep.warned = true;
            ep.due = Some(now + delay);
            self.streak += 1;
        }
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
        self.episodes += 1;
        self.episode = Some(Episode {
            id: self.episodes,
            // R3 #16: gaps never overlap.
            start: self.last_gap_end.map_or(start, |end| start.max(end)),
            reason,
            failed: 0,
            due: Some(due),
            judge: None,
            warned: false,
            urgent: false,
            confirmed: false,
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
        let Some(ep) = self.episode.as_mut() else {
            return;
        };
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

    /// End the episode: one gap. A no-op while the stream is dead (R3 #2: a dead stream always
    /// has an episode, so its reopen schedule cannot be lost); [`Watchdog::finish`] clears it.
    fn close(&mut self, now: Instant) {
        if self.dead {
            return;
        }
        if let Some(ep) = self.episode.take() {
            let end = now.max(ep.start);
            self.notes.push(Note::Gap {
                start: ep.start,
                end,
                reason: ep.reason,
                retarget: ep.retarget,
            });
            self.closed_at = Some(now);
            self.last_gap_end = Some(end);
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

/// What the tapped app is, fixed at the meeting's start (R2-2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    /// The pid the user picked.
    pub pid: u32,
    /// Its `kAudioProcessPropertyBundleID`.
    pub bundle: Option<String>,
    /// Its executable (`proc_pidpath`), for pid-reuse checks (R3 #15).
    pub exe: Option<String>,
    /// The outermost `.app` directory of its executable ([`outer_app`]).
    pub app: Option<String>,
}

impl Target {
    /// The target from its pid, bundle id and executable path.
    pub fn new(pid: u32, bundle: Option<String>, exe: Option<String>) -> Self {
        let app = exe.as_deref().and_then(outer_app).map(str::to_owned);
        Self {
            pid,
            bundle,
            exe,
            app,
        }
    }
}

/// The outermost `.app` bundle directory of an executable path:
/// `/Applications/zoom.us.app/Contents/Frameworks/aomhost.app/Contents/MacOS/aomhost` →
/// `/Applications/zoom.us.app`.
pub fn outer_app(path: &str) -> Option<&str> {
    path.find(".app/").map(|i| &path[..i + ".app".len()])
}

/// Whether `p` belongs to the target's app: the target itself (same pid AND, R3 #15, the same
/// bundle id, or the same executable when both resolve — a reused pid is a stranger); the same
/// bundle id or one under it (`us.zoom.xos.*`, not `com.google.drivefs` for
/// `com.google.Chrome`); or an executable inside the target's outermost `.app`. Never our own
/// pid. `path` resolves a pid's executable.
pub fn in_family(
    t: &Target,
    p: &ProcessInfo,
    path: &dyn Fn(u32) -> Option<String>,
    me: u32,
) -> bool {
    if p.pid == me {
        return false;
    }
    if p.pid == t.pid {
        let same = match (&t.bundle, &p.bundle_id) {
            (Some(a), Some(b)) => a == b,
            _ => match (&t.exe, path(p.pid)) {
                (Some(a), Some(b)) => *a == b,
                // Nothing to compare: trust the pid.
                _ => true,
            },
        };
        if same {
            return true;
        }
    }
    if let (Some(tb), Some(pb)) = (&t.bundle, &p.bundle_id)
        && pb
            .strip_prefix(tb.as_str())
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
    {
        return true;
    }
    t.app.as_deref().is_some_and(|app| {
        path(p.pid).is_some_and(|exe| {
            exe.strip_prefix(app)
                .is_some_and(|rest| rest.starts_with('/'))
        })
    })
}

/// The source's activity in `procs`: any family member running output; `None` (the global
/// tap) = any process at all (`procs` already lacks our own pid).
pub fn activity(
    target: Option<&Target>,
    procs: &[ProcessInfo],
    path: &dyn Fn(u32) -> Option<String>,
    me: u32,
) -> Activity {
    let playing = procs
        .iter()
        .filter(|p| p.active)
        .any(|p| target.is_none_or(|t| in_family(t, p, path, me)));
    if playing {
        Activity::Active
    } else {
        Activity::Inactive
    }
}

/// The pid to tap on a reopen or return: `current` while it runs output (and is still of the
/// family), else a family member that does, else `current`.
pub fn retarget(
    t: &Target,
    current: u32,
    procs: &[ProcessInfo],
    path: &dyn Fn(u32) -> Option<String>,
    me: u32,
) -> u32 {
    let playing = |p: &&ProcessInfo| p.active && in_family(t, p, path, me);
    if procs.iter().filter(playing).any(|p| p.pid == current) {
        return current;
    }
    procs.iter().find(playing).map_or(current, |p| p.pid)
}

/// The probe floor from `THEUTH_MEETING_PROBE_DBFS`'s value: `(floor, complaint)`. Unset →
/// [`AUDIBLE_DBFS`]; a finite number → clamped to −140…0 dBFS; anything else (unparsable, NaN,
/// ±inf — NaN would make every probe silent) → [`AUDIBLE_DBFS`] and a complaint to log (R3 #10).
pub fn probe_floor(raw: Option<&str>) -> (f32, Option<String>) {
    let Some(raw) = raw else {
        return (AUDIBLE_DBFS, None);
    };
    match raw.trim().parse::<f32>() {
        Ok(v) if v.is_finite() => (v.clamp(-140.0, 0.0), None),
        _ => (
            AUDIBLE_DBFS,
            Some(format!(
                "THEUTH_MEETING_PROBE_DBFS={raw:?} is not a finite number; using {AUDIBLE_DBFS} dBFS"
            )),
        ),
    }
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
    /// flexaudio `ChunkDropped` chunks, as deltas of its per-stream running total.
    pub dropped: u64,
    /// flexaudio `Clipped` events.
    pub clipped: u64,
    /// The current stream's last `ChunkDropped` total (kept across minutes).
    dropped_total: u64,
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

    /// flexaudio's `ChunkDropped { count }`: `count` is the stream's running total (R3 #3).
    pub fn chunk_dropped(&mut self, total: u64) {
        self.dropped += total.saturating_sub(self.dropped_total);
        self.dropped_total = total;
    }

    /// Start the next minute's counters (the stream's drop total carries over).
    pub fn next_minute(&mut self) {
        *self = Self {
            dropped_total: self.dropped_total,
            ..Self::default()
        };
    }

    /// The stream was replaced: its drop total starts at 0.
    pub fn new_stream(&mut self) {
        self.dropped_total = 0;
    }
}

/// A probe's measurement at 48 kHz mono: frames seen, 100 ms blocks at or above the floor,
/// non-zero frames.
#[derive(Debug)]
pub struct ProbeMeter {
    floor: f32,
    block_peak: f32,
    block_len: usize,
    frames: usize,
    loud: usize,
    nonzero: usize,
}

impl ProbeMeter {
    /// Frames per block: 100 ms.
    const BLOCK: usize = super::RATE as usize / 10;

    /// Blocks count as loud at `floor_dbfs` peak or above.
    pub fn new(floor_dbfs: f32) -> Self {
        Self {
            floor: 10f32.powf(floor_dbfs / 20.0),
            block_peak: 0.0,
            block_len: 0,
            frames: 0,
            loud: 0,
            nonzero: 0,
        }
    }

    /// Count samples.
    pub fn add(&mut self, pcm: &[f32]) {
        for &s in pcm {
            self.frames += 1;
            self.nonzero += usize::from(s != 0.0);
            self.block_peak = self.block_peak.max(s.abs());
            self.block_len += 1;
            if self.block_len == Self::BLOCK {
                if self.block_peak >= self.floor {
                    self.loud += Self::BLOCK;
                }
                self.block_peak = 0.0;
                self.block_len = 0;
            }
        }
    }

    /// The confirmation verdict: under 1 s of frames is no evidence (R3 #4: `Failed`, not
    /// "genuine silence"); else audible at ≥ [`AUDIBLE_FOR`] of loud blocks.
    pub fn verdict(&self) -> Probe {
        if Self::secs(self.frames) < Duration::from_secs(1) {
            Probe::Failed
        } else if self.loud() >= AUDIBLE_FOR {
            Probe::Audible
        } else {
            Probe::Silent
        }
    }

    /// Time in loud blocks.
    pub fn loud(&self) -> Duration {
        Self::secs(self.loud)
    }

    /// Time in non-zero samples.
    pub fn nonzero(&self) -> Duration {
        Self::secs(self.nonzero)
    }

    fn secs(frames: usize) -> Duration {
        Duration::from_secs_f64(frames as f64 / f64::from(super::RATE))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The per-app tap.
    const APP: Rules = Rules {
        zeros: true,
        fallback: true,
        probe: true,
    };
    /// The global tap (pid 0).
    const GLOBAL: Rules = Rules {
        zeros: true,
        fallback: false,
        probe: false,
    };
    const MIC: Rules = Rules {
        zeros: false,
        fallback: false,
        probe: false,
    };

    /// Drives a watchdog on a fake clock: `ms` since `t0`. Probes answer [`PROBE_WINDOW`]
    /// after they were asked for, with `probe` (confirm/judge) or `back` (return).
    struct Rig {
        t0: Instant,
        now: Duration,
        wd: Watchdog,
        activity: Activity,
        probe: Probe,
        back: bool,
        /// What reopens report as the new pid.
        retarget: Option<u32>,
        pending: Option<(u64, Action)>,
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
                probe: Probe::Audible,
                back: false,
                retarget: None,
                pending: None,
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
        /// 250 ms; reopens and fallbacks are answered with `reopen_ok`.
        fn run(&mut self, ms: u64, chunks: bool, zero: bool, reopen_ok: bool) {
            while self.ms() < ms {
                self.now += Duration::from_millis(10);
                let now = self.t0 + self.now;
                let t = self.ms();
                if chunks && t.is_multiple_of(20) {
                    self.wd.chunk(now, zero);
                }
                if let Some((due, kind)) = self.pending
                    && t >= due
                {
                    self.pending = None;
                    match kind {
                        Action::Return => self.wd.returned(now, self.back),
                        _ => self.wd.probed(now, self.probe),
                    }
                }
                if t.is_multiple_of(250) {
                    let a = self.activity;
                    if let Some(action) = self.wd.tick(now, &mut || a) {
                        self.actions.push((t, action));
                        match action {
                            Action::Reopen | Action::Fallback => {
                                self.wd.reopened(now, reopen_ok, self.retarget)
                            }
                            Action::Probe | Action::Return => {
                                self.pending = Some((t + PROBE_WINDOW.as_millis() as u64, action));
                            }
                        }
                    }
                }
                self.notes.extend(self.wd.take_notes());
            }
        }

        fn at_kinds(&self) -> Vec<(u64, Action)> {
            self.actions.clone()
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

    use Action::{Fallback, Probe as P, Reopen, Return};

    #[test]
    fn healthy_stream_does_nothing() {
        let mut r = Rig::new(APP);
        r.run(120_000, true, false, true);
        assert!(r.actions.is_empty());
        assert!(r.notes.is_empty());
    }

    #[test]
    fn stall_reopens_and_recovers_without_a_warning() {
        let mut r = Rig::new(APP);
        r.run(10_000, true, false, true);
        r.run(13_000, false, false, true);
        assert_eq!(r.actions, vec![(13_000, Reopen)]);
        r.run(14_000, false, false, true);
        r.run(20_000, true, false, true);
        assert_eq!(r.actions.len(), 1);
        assert_eq!(r.gaps(), vec![(10_000, 14_020, CaptureFault::Stalled)]);
        assert!(r.healths().is_empty(), "one reopen that worked: no warning");
    }

    #[test]
    fn flexaudio_recovery_needs_no_reopen_of_ours() {
        let mut r = Rig::new(APP);
        r.run(10_000, true, false, true);
        r.wd.signal(r.at(12_000), Signal::Stalled);
        r.run(15_000, false, false, true);
        r.wd.signal(r.at(15_000), Signal::Recovered);
        r.run(20_000, true, false, true);
        assert!(r.actions.is_empty());
        assert_eq!(r.gaps(), vec![(10_000, 15_020, CaptureFault::Stalled)]);
        assert!(r.healths().is_empty());
    }

    #[test]
    fn flexaudio_grace_expires_then_we_reopen() {
        let mut r = Rig::new(APP);
        r.run(10_000, true, false, true);
        r.wd.signal(r.at(12_000), Signal::Stalled);
        r.run(18_000, false, false, true);
        assert_eq!(r.actions, vec![(17_000, Reopen)]);
    }

    #[test]
    fn stall_backoff_warns_after_first_failed_reopen_then_resets() {
        let mut r = Rig::new(GLOBAL);
        r.run(1_000, true, false, true);
        r.run(70_000, false, false, true);
        let at: Vec<u64> = r.actions.iter().map(|a| a.0).collect();
        // Stall at 4 s; each reopen is judged 3 s later, then 1, 2, 5, 10, 30 s of backoff.
        assert_eq!(at, vec![4_000, 8_000, 13_000, 21_000, 34_000, 67_000]);
        let h = r.healths();
        assert_eq!(h[0], Health::Recovering(CaptureFault::Stalled, r.at(1_000)));
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
    fn incident_zeros_from_an_idle_object_are_probed_and_reopened() {
        // The tapped object no longer runs output; the app plays elsewhere: the probe hears it.
        let mut r = Rig::new(APP);
        r.activity = Activity::Inactive;
        r.run(10_000, true, false, true);
        r.run(30_000, true, true, true);
        // 15 s of zeros (from 10.02 s) → probe at 25.25 s; audible at 29.25 s → reopen.
        assert_eq!(r.at_kinds(), vec![(25_250, P), (29_250, Reopen)]);
        r.run(31_000, true, false, true);
        assert_eq!(r.gaps(), vec![(10_020, 30_020, CaptureFault::Silent)]);
        assert!(
            r.healths().is_empty(),
            "the first reopen worked: no warning"
        );
    }

    #[test]
    fn muted_call_is_genuine_silence_even_while_running_output() {
        // Zoom runs output but everyone is muted and nothing else plays: no episode, ever;
        // re-probed 30, 60, then every 120 s.
        let mut r = Rig::new(APP);
        r.probe = Probe::Silent;
        r.run(10_000, true, false, true);
        r.run(400_000, true, true, true);
        let at: Vec<u64> = r.actions.iter().map(|a| a.0).collect();
        assert_eq!(at, vec![25_250, 59_250, 123_250, 247_250, 371_250]);
        assert!(r.actions.iter().all(|a| a.1 == P));
        assert!(r.notes.is_empty(), "no gap, no warning");
        // Sound returns, then zeros again: the schedule starts over (15 s of zeros, probe).
        r.run(401_000, true, false, true);
        r.actions.clear();
        r.run(420_000, true, true, true);
        assert_eq!(r.at_kinds(), vec![(416_250, P)]);
    }

    #[test]
    fn judge_probe_silent_ends_the_episode_without_a_warning() {
        let mut r = Rig::new(APP);
        r.run(10_000, true, false, true);
        r.run(34_000, true, true, true);
        assert_eq!(r.at_kinds(), vec![(25_250, P), (29_250, Reopen)]);
        // After the reopen: 5 s of zeros → judge probe at 34.5 s; it hears nothing.
        r.probe = Probe::Silent;
        r.run(70_000, true, true, true);
        assert_eq!(r.actions[2], (34_500, P));
        assert_eq!(r.gaps(), vec![(10_020, 38_500, CaptureFault::Silent)]);
        assert!(r.healths().is_empty());
        // Genuine silence now: the next confirmation probe 30 s later.
        assert_eq!(r.actions[3], (68_500, P));
    }

    #[test]
    fn confirmed_failures_warn_then_fall_back_then_return() {
        let mut r = Rig::new(APP);
        r.activity = Activity::Inactive;
        r.run(1_000, true, false, true);
        r.run(51_010, true, true, true);
        assert_eq!(
            r.at_kinds(),
            vec![
                (16_250, P),
                (20_250, Reopen),
                (25_500, P),
                (30_500, Reopen),
                (35_750, P),
                (41_750, Reopen),
                (47_000, P),
                // R3 #11: decided by the third confirmed failure itself, no extra backoff.
                (51_000, Fallback),
            ]
        );
        // Warned when the first judge probe was audible (29.5 s), Lost after the third (51 s).
        let h = r.healths();
        assert_eq!(h[0], Health::Recovering(CaptureFault::Silent, r.at(1_020)));
        assert_eq!(h[1], Health::Lost(CaptureFault::Silent, r.at(1_020)));
        // The global tap hears the sound the probes heard: degraded, gap closed.
        r.run(60_000, true, false, true);
        assert_eq!(
            r.healths().last(),
            Some(&Health::Degraded(CaptureFault::Silent))
        );
        assert_eq!(r.gaps(), vec![(1_020, 51_020, CaptureFault::Silent)]);
        // Return probes every 60 s; the app tap is still silent at 111 s, back at 175 s.
        r.run(121_000, true, false, true);
        assert_eq!(r.actions[8], (111_000, Return));
        r.back = true;
        r.run(185_000, true, false, true);
        assert_eq!(r.actions[9], (175_000, Return));
        assert_eq!(r.healths().last(), Some(&Health::Ok));
        assert_eq!(r.wd.fell_back(), None);
        assert_eq!(r.actions.len(), 10);
    }

    #[test]
    fn failed_probe_uses_the_activity_gated_rule() {
        let mut r = Rig::new(APP);
        r.probe = Probe::Failed;
        r.run(1_000, true, false, true);
        r.run(21_000, true, true, true);
        // Probe at 16.25 s fails at 20.25 s; the source runs output → reopen at once.
        assert_eq!(r.at_kinds(), vec![(16_250, P), (20_250, Reopen)]);
        let mut r = Rig::new(APP);
        r.probe = Probe::Failed;
        r.activity = Activity::Inactive;
        r.run(1_000, true, false, true);
        r.run(200_000, true, true, true);
        assert_eq!(
            r.at_kinds(),
            vec![(16_250, P)],
            "inactive: round 1's genuine silence"
        );
    }

    #[test]
    fn global_tap_keeps_the_activity_gated_rule() {
        let mut r = Rig::new(GLOBAL);
        r.activity = Activity::Inactive;
        r.run(10_000, true, false, true);
        r.run(300_000, true, true, true);
        assert!(r.actions.is_empty());
        let mut r = Rig::new(GLOBAL);
        r.run(10_000, true, false, true);
        r.run(26_000, true, true, true);
        assert_eq!(r.at_kinds(), vec![(25_250, Reopen)]);
        r.run(200_000, true, true, true);
        assert!(
            r.actions.iter().all(|a| a.1 == Reopen),
            "no probe, no fallback"
        );
    }

    #[test]
    fn source_going_quiet_ends_a_global_zero_episode() {
        let mut r = Rig::new(GLOBAL);
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
        let mut r = Rig::new(GLOBAL);
        r.activity = Activity::Unknown;
        r.run(1_000, true, false, true);
        r.run(400_000, true, true, true);
        let at: Vec<u64> = r.actions.iter().map(|a| a.0).collect();
        // 30 s after the first zero chunk (1.02 s), then every 2 min.
        assert_eq!(at, vec![31_250, 151_250, 271_250, 391_250]);
    }

    #[test]
    fn permission_denied_warns_at_once_and_retries_with_backoff() {
        let mut r = Rig::new(APP);
        r.run(5_000, true, false, true);
        r.wd.signal(r.at(5_000), Signal::PermissionDenied);
        r.notes.extend(r.wd.take_notes());
        assert_eq!(
            r.healths(),
            vec![Health::Lost(CaptureFault::PermissionDenied, r.at(5_000))]
        );
        // Reopens keep failing (still denied): 1, 2, 5, 10 s apart; never a fallback.
        r.run(25_000, false, false, false);
        let at: Vec<u64> = r.actions.iter().map(|a| a.0).collect();
        assert_eq!(at, vec![6_000, 8_000, 13_000, 23_000]);
        assert!(r.actions.iter().all(|a| a.1 == Reopen));
        // Granted mid-meeting: the next reopen (30 s later) opens, audio returns, the warning clears.
        r.run(53_000, false, false, true);
        assert_eq!(r.actions.last(), Some(&(53_000, Reopen)));
        r.run(60_000, true, false, true);
        assert_eq!(r.healths().last(), Some(&Health::Ok));
        assert_eq!(
            r.gaps(),
            vec![(5_000, 53_020, CaptureFault::PermissionDenied)]
        );
    }

    #[test]
    fn terminal_error_is_lost_at_once() {
        let mut r = Rig::new(APP);
        r.run(5_000, true, false, true);
        r.wd.signal(r.at(5_000), Signal::Terminal);
        r.notes.extend(r.wd.take_notes());
        assert_eq!(
            r.healths(),
            vec![Health::Lost(CaptureFault::Failed, r.at(5_000))]
        );
        r.run(6_000, false, false, true);
        assert_eq!(r.actions, vec![(6_000, Reopen)]);
    }

    #[test]
    fn device_lost_reopens_at_once_without_warning() {
        let mut r = Rig::new(MIC);
        r.run(5_000, true, false, true);
        r.wd.signal(r.at(5_000), Signal::Lost);
        // The stream is gone: no chunks until the reopen.
        r.run(5_300, false, false, true);
        r.run(8_000, true, false, true);
        assert_eq!(r.actions, vec![(5_250, Reopen)]);
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
        let mut r = Rig::new(APP);
        r.run(1_000, true, false, true);
        r.run(9_000, false, false, false);
        r.wd.finish(r.at(9_000));
        r.notes.extend(r.wd.take_notes());
        assert_eq!(r.gaps(), vec![(1_000, 9_000, CaptureFault::Stalled)]);
    }

    fn proc(pid: u32, bundle: Option<&str>, active: bool) -> ProcessInfo {
        ProcessInfo {
            pid,
            name: format!("p{pid}"),
            bundle_id: bundle.map(str::to_owned),
            active,
        }
    }

    #[test]
    fn family_is_bundle_prefix_or_inside_the_outer_app() {
        let zoom = Target::new(
            100,
            Some("us.zoom.xos".into()),
            Some("/Applications/zoom.us.app/Contents/MacOS/zoom.us".into()),
        );
        assert_eq!(zoom.app.as_deref(), Some("/Applications/zoom.us.app"));
        let paths = |pid: u32| -> Option<String> {
            match pid {
                300 => Some("/Applications/zoom.us.app/Contents/Frameworks/aomhost.app/Contents/MacOS/aomhost".into()),
                400 => Some("/Applications/zoom.us.appendix/bin/x".into()),
                500 => Some("/Applications/Other.app/Contents/MacOS/zoom.us.app/x".into()),
                _ => None,
            }
        };
        let me = 999;
        let fam = |p: &ProcessInfo| in_family(&zoom, p, &paths, me);
        assert!(fam(&proc(100, None, false)), "the target itself");
        assert!(fam(&proc(200, Some("us.zoom.xos"), true)), "same bundle");
        assert!(
            fam(&proc(201, Some("us.zoom.xos.Helper"), true)),
            "bundle prefix + '.'"
        );
        assert!(
            !fam(&proc(202, Some("us.zoom.xosx"), true)),
            "a longer id is not a child"
        );
        assert!(fam(&proc(300, None, true)), "inside zoom.us.app");
        assert!(
            !fam(&proc(400, None, true)),
            "a sibling directory sharing the prefix"
        );
        assert!(!fam(&proc(500, None, true)), "another app's nested path");
        assert!(
            !fam(&proc(me, Some("us.zoom.xos"), true)),
            "never our own pid"
        );
        let chrome = Target::new(1, Some("com.google.Chrome".into()), None);
        assert!(!in_family(
            &chrome,
            &proc(2, Some("com.google.drivefs"), true),
            &paths,
            me
        ));
        assert!(in_family(
            &chrome,
            &proc(3, Some("com.google.Chrome.helper"), true),
            &paths,
            me
        ));
        assert_eq!(outer_app("/usr/bin/afplay"), None);
    }

    /// 173 R3 #15: the target's pid is the target only while it is the same program.
    #[test]
    fn a_reused_target_pid_is_not_the_family() {
        let zoom = Target::new(
            100,
            Some("us.zoom.xos".into()),
            Some("/Applications/zoom.us.app/Contents/MacOS/zoom.us".into()),
        );
        let paths = |pid: u32| match pid {
            100 => Some("/usr/bin/afplay".to_owned()),
            _ => None,
        };
        let none = |_: u32| None;
        let me = 999;
        assert!(
            !in_family(&zoom, &proc(100, Some("com.apple.afplay"), true), &none, me),
            "another bundle on the pid"
        );
        assert!(
            !in_family(&zoom, &proc(100, None, true), &paths, me),
            "another executable on the pid"
        );
        assert!(
            in_family(&zoom, &proc(100, None, true), &none, me),
            "nothing to compare: trust the pid"
        );
        assert!(in_family(
            &zoom,
            &proc(100, Some("us.zoom.xos"), false),
            &paths,
            me
        ));
        // retarget does not keep a reused pid that happens to run output.
        let procs = [
            proc(100, Some("com.apple.afplay"), true),
            proc(200, Some("us.zoom.xos"), true),
        ];
        assert_eq!(retarget(&zoom, 100, &procs, &none, me), 200);
    }

    #[test]
    fn retarget_and_activity_follow_the_family() {
        let zoom = Target {
            pid: 51437,
            bundle: Some("us.zoom.xos".into()),
            exe: None,
            app: Some("/Applications/zoom.us.app".into()),
        };
        let paths = |pid: u32| {
            (pid == 61000).then(|| "/Applications/zoom.us.app/Contents/Frameworks/aomhost.app/Contents/MacOS/aomhost".to_owned())
        };
        let me = 7;
        let moved = [
            proc(51437, Some("us.zoom.xos"), false),
            proc(60000, Some("com.other"), true),
            proc(61000, None, true),
        ];
        assert_eq!(retarget(&zoom, 51437, &moved, &paths, me), 61000);
        assert_eq!(activity(Some(&zoom), &moved, &paths, me), Activity::Active);
        // The tapped pid still plays: keep it.
        let playing = [
            proc(51437, Some("us.zoom.xos"), true),
            proc(61000, None, true),
        ];
        assert_eq!(retarget(&zoom, 51437, &playing, &paths, me), 51437);
        // Nothing of the family plays: keep the pid; another app does not count.
        let quiet = [
            proc(51437, Some("us.zoom.xos"), false),
            proc(60000, Some("com.other"), true),
        ];
        assert_eq!(retarget(&zoom, 51437, &quiet, &paths, me), 51437);
        assert_eq!(
            activity(Some(&zoom), &quiet, &paths, me),
            Activity::Inactive
        );
        // The global tap: any process.
        assert_eq!(activity(None, &quiet, &paths, me), Activity::Active);
        assert_eq!(activity(None, &[], &paths, me), Activity::Inactive);
    }

    #[test]
    fn probe_meter_counts_loud_blocks_and_nonzero_time() {
        let mut m = ProbeMeter::new(AUDIBLE_DBFS);
        let block = ProbeMeter::BLOCK;
        // 1 s at −60 dBFS (loud), 1 s at −80 dBFS (non-zero, quiet), 1 s of zeros.
        m.add(&vec![1e-3; 10 * block]);
        m.add(&vec![1e-4; 10 * block]);
        m.add(&vec![0.0; 10 * block]);
        assert_eq!(m.loud(), Duration::from_secs(1));
        assert_eq!(m.nonzero(), Duration::from_secs(2));
        // One loud sample makes its whole block loud.
        let mut m = ProbeMeter::new(AUDIBLE_DBFS);
        let mut pcm = vec![0.0; block];
        pcm[17] = 0.5;
        m.add(&pcm);
        assert_eq!(m.loud(), Duration::from_millis(100));
    }

    /// 173 R3 #4: a probe that heard (almost) nothing is no evidence of silence.
    #[test]
    fn probe_verdict_needs_a_second_of_frames() {
        let block = ProbeMeter::BLOCK;
        assert_eq!(
            ProbeMeter::new(AUDIBLE_DBFS).verdict(),
            Probe::Failed,
            "no frames"
        );
        let mut m = ProbeMeter::new(AUDIBLE_DBFS);
        m.add(&vec![1e-2; 9 * block]);
        assert_eq!(m.verdict(), Probe::Failed, "0.9 s of frames, loud or not");
        let mut m = ProbeMeter::new(AUDIBLE_DBFS);
        m.add(&vec![0.0; 40 * block]);
        assert_eq!(m.verdict(), Probe::Silent);
        m.add(&vec![1e-3; 10 * block]);
        assert_eq!(m.verdict(), Probe::Audible);
    }

    /// 173 R3 #10: the env knob rejects what would switch the zero rule off.
    #[test]
    fn probe_floor_parses_finite_numbers_only() {
        assert_eq!(probe_floor(None), (AUDIBLE_DBFS, None));
        assert_eq!(probe_floor(Some("-90")), (-90.0, None));
        assert_eq!(probe_floor(Some(" -200 ")), (-140.0, None), "clamped");
        assert_eq!(probe_floor(Some("5")), (0.0, None), "clamped");
        for bad in ["nan", "inf", "-inf", "loud", ""] {
            let (floor, complaint) = probe_floor(Some(bad));
            assert_eq!(floor, AUDIBLE_DBFS, "{bad:?}");
            assert!(complaint.is_some(), "{bad:?} is reported");
        }
    }

    /// 173 R3: flexaudio's `SilenceWhileSourceActive` counts as playing on the gated rule.
    #[test]
    fn silence_while_source_active_arms_the_gated_rule() {
        let mut r = Rig::new(GLOBAL);
        r.activity = Activity::Inactive;
        r.run(1_000, true, false, true);
        r.run(10_000, true, true, true);
        r.wd.signal(r.at(10_000), Signal::SilentWhileActive);
        r.run(17_000, true, true, true);
        assert_eq!(
            r.at_kinds(),
            vec![(16_250, Reopen)],
            "15 s of zeros, flexaudio vouched"
        );
    }

    /// 173 R3: the pid a reopen retargeted to reaches the gap.
    #[test]
    fn retarget_reaches_the_gap_note() {
        let mut r = Rig::new(APP);
        r.retarget = Some(61_000);
        r.run(10_000, true, false, true);
        r.run(30_000, true, true, true);
        r.run(31_000, true, false, true);
        assert!(matches!(
            r.notes.iter().find(|n| matches!(n, Note::Gap { .. })),
            Some(Note::Gap {
                retarget: Some(61_000),
                ..
            })
        ));
    }

    /// 173 R3 #16: an episode after a Judge-silent end never starts inside the closed gap.
    #[test]
    fn gaps_never_overlap() {
        let mut r = Rig::new(APP);
        r.run(10_000, true, false, true);
        r.run(34_000, true, true, true);
        r.probe = Probe::Silent;
        r.run(39_000, true, true, true);
        // Judge silent at 38.5 s closed the first gap; then sound plays again elsewhere.
        r.probe = Probe::Audible;
        r.run(73_000, true, true, true);
        r.run(74_000, true, false, true);
        let g = r.gaps();
        assert_eq!(g.len(), 2, "{g:?}");
        assert_eq!(g[0], (10_020, 38_500, CaptureFault::Silent));
        assert!(
            g[1].0 >= g[0].1,
            "second gap starts at or after the first's end: {g:?}"
        );
    }

    /// 173 R3 #2: a Confirm verdict that arrives after the stream died must not close the
    /// dead stream's episode (round 2: no reopen ever again, health `Ok`).
    #[test]
    fn stale_confirm_verdict_after_the_stream_died_does_not_wedge() {
        let mut r = Rig::new(APP);
        r.probe = Probe::Silent;
        r.run(10_000, true, false, true);
        r.run(26_000, true, true, true);
        assert_eq!(r.at_kinds(), vec![(25_250, P)]);
        r.wd.signal(r.at(26_000), Signal::Lost);
        r.run(120_000, false, false, false);
        assert!(
            r.actions.iter().any(|a| a.0 > 60_000 && a.1 == Reopen),
            "still reopening: {:?}",
            r.actions
        );
        assert_ne!(r.healths().last(), Some(&Health::Ok));
        assert!(
            r.gaps().is_empty(),
            "no audio came back: the gap is still open"
        );
    }

    /// 173 R3 #2: a Judge probe that fails after the stream died is not a second failure.
    #[test]
    fn failed_judge_verdict_on_a_dead_episode_is_ignored() {
        let mut r = Rig::new(APP);
        r.run(10_000, true, false, true);
        r.run(34_000, true, true, true);
        assert_eq!(r.at_kinds(), vec![(25_250, P), (29_250, Reopen)]);
        r.probe = Probe::Failed;
        r.run(35_000, true, true, true);
        assert_eq!(r.actions[2], (34_500, P), "the judge probe runs");
        r.wd.signal(r.at(35_000), Signal::Lost);
        r.run(60_000, false, false, false);
        let after: Vec<u64> = r.actions[3..].iter().map(|a| a.0).collect();
        // Backoff 1, 2, 5, 10 s: the stale verdict at 38.5 s neither counts nor re-times.
        assert_eq!(after, vec![35_250, 36_250, 38_250, 43_250, 53_250]);
    }

    /// 173 R3 #2: a Confirm verdict that arrives after a stall opened an episode is ignored.
    #[test]
    fn confirm_verdict_after_a_stall_is_ignored() {
        let mut r = Rig::new(APP);
        r.probe = Probe::Silent;
        r.run(10_000, true, false, true);
        r.run(25_500, true, true, true);
        r.run(33_000, false, false, true);
        assert_eq!(
            r.at_kinds(),
            vec![(25_250, P), (28_500, Reopen), (32_500, Reopen)]
        );
        assert!(
            r.gaps().is_empty(),
            "the stall episode was not closed by the verdict"
        );
        assert_eq!(
            r.healths(),
            vec![Health::Recovering(CaptureFault::Stalled, r.at(25_500))]
        );
    }

    /// 173 R3 #3: `ChunkDropped { count }` is the stream's running total.
    #[test]
    fn chunk_dropped_counts_deltas_per_stream() {
        let mut m = Meter::default();
        for total in [1, 2, 3] {
            m.chunk_dropped(total);
        }
        assert_eq!(m.dropped, 3);
        m.next_minute();
        m.chunk_dropped(5);
        assert_eq!(m.dropped, 2, "the total carries over the minute");
        m.new_stream();
        m.chunk_dropped(1);
        assert_eq!(m.dropped, 3, "a new stream counts from 0");
    }
}
