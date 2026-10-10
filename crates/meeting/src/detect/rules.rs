//! The rules engine (158 §4.4): Idle → Candidate → Prompting → Recording → Ending as a state
//! machine over [`super::sensor::mic_holders`] ticks, emitting `MeetingEvent::Detected`,
//! `DetectionCleared` and `AppReleased`. Pure: the actor feeds it ticks, the clock and the
//! user's answers, and publishes what comes out. The thresholds are the contract.

use std::collections::HashMap;
use std::time::Duration;

use tokio::time::Instant;

use super::{Cleared, Detection};
use crate::calendar::matcher;
use crate::{MeetingId, ProcessInfo};

/// Continuous hold before a prompt (§4.4 rule 2).
pub const HOLD_SECS: u64 = 15;
/// "Not now" cooldown per app id (§4.4 rule 5).
pub const COOLDOWN_SECS: u64 = 10 * 60;
/// Release before `AppReleased` / `DetectionCleared{Released}` (§4.4 rules 7–8).
pub const RELEASE_SECS: u64 = 30;

const HOLD: Duration = Duration::from_secs(HOLD_SECS);
const COOLDOWN: Duration = Duration::from_secs(COOLDOWN_SECS);
const RELEASE: Duration = Duration::from_secs(RELEASE_SECS);

/// §4.3 app id: the bundle id on macOS, `application.name` on Linux (both ride in
/// `bundle_id`), the lowercase image name with `.exe` on Windows, else the process name.
pub fn app_id(p: &ProcessInfo) -> String {
    match &p.bundle_id {
        Some(b) => b.clone(),
        None if cfg!(target_os = "windows") => format!("{}.exe", p.name.to_lowercase()),
        None => p.name.clone(),
    }
}

/// What a tick or an answer produced; the actor publishes each as a `MeetingEvent`.
#[derive(Clone, Debug, PartialEq)]
pub enum Output {
    /// §4.5 `Detected`.
    Detected(Box<Detection>),
    /// §4.5 `DetectionCleared`.
    Cleared {
        /// Which app's prompt.
        app_id: String,
        /// Why.
        reason: Cleared,
    },
    /// §4.5 `AppReleased`.
    AppReleased {
        /// The recording meeting.
        meeting: MeetingId,
        /// The app that let go.
        app: ProcessInfo,
        /// Seconds since its last input stream.
        released_for_s: u32,
    },
}

/// An eligible app holding the mic.
#[derive(Clone, Debug)]
struct Hold {
    app: ProcessInfo,
    /// First tick that saw it (quiet included: "when the hold began").
    first: Instant,
    /// Held while not quiet (rule 3: quiet pauses the clock).
    held: Duration,
}

/// The recording's detected app (rule 7).
#[derive(Clone, Debug)]
struct Tracked {
    meeting: MeetingId,
    app_id: String,
    app: ProcessInfo,
    last_seen: Instant,
    /// `AppReleased` went out for the current release episode.
    asked: bool,
}

/// The state machine. One per engine; lives in the meeting actor.
#[derive(Debug)]
pub struct Engine {
    /// `meeting.detect.ignore` (rule 1); "Never for" appends.
    pub ignore: Vec<String>,
    holds: HashMap<String, Hold>,
    cooldown: HashMap<String, Instant>,
    /// The open prompt and when its app was last seen holding.
    prompt: Option<(Detection, Instant)>,
    /// The app id a `StartMeeting{source: Detected}` named, until `Begun`.
    pending: Option<String>,
    tracked: Option<Tracked>,
    last_tick: Option<Instant>,
}

impl Engine {
    /// Idle, with `meeting.detect.ignore`.
    pub fn new(ignore: Vec<String>) -> Self {
        Self {
            ignore,
            holds: HashMap::new(),
            cooldown: HashMap::new(),
            prompt: None,
            pending: None,
            tracked: None,
            last_tick: None,
        }
    }

    /// The open prompt, for `meeting_detection` and `Status.detection`.
    pub fn prompt(&self) -> Option<&Detection> {
        self.prompt.as_ref().map(|(d, _)| d)
    }

    fn ignored(&self, app_id: &str) -> bool {
        self.ignore.iter().any(|i| i.eq_ignore_ascii_case(app_id))
    }

    /// One sensor tick. `quiet` = a meeting is live or theuth's own dictation/talk is (rule 3).
    pub fn tick(&mut self, now: Instant, holders: &[ProcessInfo], quiet: bool) -> Vec<Output> {
        let mut out = Vec::new();
        let dt = self.last_tick.map_or(Duration::ZERO, |t| now - t);
        self.last_tick = Some(now);
        self.cooldown.retain(|_, until| *until > now);

        // Rule 1–2: eligible holders accumulate; one that dropped out of this tick restarts.
        let present: Vec<(String, &ProcessInfo)> = holders
            .iter()
            .map(|p| (app_id(p), p))
            .filter(|(id, _)| !self.ignored(id))
            .collect();
        self.holds
            .retain(|id, _| present.iter().any(|(p, _)| p == id));
        for (id, app) in &present {
            match self.holds.get_mut(id) {
                Some(h) => {
                    if !quiet {
                        h.held += dt;
                    }
                }
                None => {
                    self.holds.insert(
                        id.clone(),
                        Hold {
                            app: (*app).clone(),
                            first: now,
                            held: Duration::ZERO,
                        },
                    );
                }
            }
        }

        // Rule 8: an unanswered prompt withdraws once its app has let go for 30 s.
        if let Some((det, last_seen)) = self.prompt.as_mut() {
            if present.iter().any(|(id, _)| *id == det.app_id) {
                *last_seen = now;
            } else if now - *last_seen >= RELEASE {
                out.push(Output::Cleared {
                    app_id: det.app_id.clone(),
                    reason: Cleared::Released,
                });
                self.prompt = None;
            }
        }

        // Rule 7: the recording's app let go for 30 s → ask once per release episode (there is
        // no "keep recording" RPC; the island's answer is local). A new episode needs the app
        // to hold the mic again first.
        if let Some(t) = self.tracked.as_mut() {
            if holders.iter().any(|p| app_id(p) == t.app_id) {
                t.last_seen = now;
                t.asked = false;
            } else {
                let released = now - t.last_seen;
                if released >= RELEASE && !t.asked {
                    out.push(Output::AppReleased {
                        meeting: t.meeting,
                        app: t.app.clone(),
                        released_for_s: released.as_secs() as u32,
                    });
                    t.asked = true;
                }
            }
        }

        // Rules 2–5: the longest-held eligible app past 15 s prompts, once, the rest in `also`.
        if self.prompt.is_none() && !quiet {
            let best = self
                .holds
                .iter()
                .filter(|(id, h)| h.held >= HOLD && !self.cooldown.contains_key(*id))
                .max_by(|a, b| {
                    a.1.held
                        .cmp(&b.1.held)
                        .then_with(|| b.1.app.name.cmp(&a.1.app.name))
                });
            if let Some((id, h)) = best {
                let since = chrono::Local::now()
                    - chrono::Duration::from_std(now - h.first).unwrap_or_default();
                let det = Detection {
                    app: h.app.clone(),
                    app_id: id.clone(),
                    since: since.to_rfc3339(),
                    also: present
                        .iter()
                        .filter(|(p, _)| p != id)
                        .map(|(_, app)| (*app).clone())
                        .collect(),
                    // W2 fills the candidates from the event store; `&[]` is "no calendar".
                    event: matcher::best(chrono::Utc::now(), id, &[]),
                    project: None,
                    brief: None,
                };
                out.push(Output::Detected(Box::new(det.clone())));
                self.prompt = Some((det, now));
            }
        }
        out
    }

    /// "Not now" (rule 5: 10-minute cooldown for the prompted app and the ones in `also`)
    /// or "Never for" (rule 1: the app joins `ignore`; the caller persists it).
    pub fn dismiss(&mut self, now: Instant, id: &str, never: bool) -> Vec<Output> {
        let prompted = self.prompt.as_ref().is_some_and(|(d, _)| d.app_id == id);
        if never {
            if !self.ignored(id) {
                self.ignore.push(id.to_owned());
            }
            self.holds.remove(id);
        } else {
            self.cooldown.insert(id.to_owned(), now + COOLDOWN);
            if let Some((d, _)) = self.prompt.as_ref().filter(|_| prompted) {
                for also in &d.also {
                    self.cooldown.insert(app_id(also), now + COOLDOWN);
                }
            }
        }
        if !prompted {
            return Vec::new();
        }
        self.prompt = None;
        vec![Output::Cleared {
            app_id: id.to_owned(),
            reason: if never {
                Cleared::Never
            } else {
                Cleared::NotNow
            },
        }]
    }

    /// A meeting was accepted for start (rule 8 `Started`); `source_app` is the app id a
    /// prompt's one-tap start named, remembered until [`Self::recording`].
    pub fn starting(&mut self, source_app: Option<String>) -> Vec<Output> {
        self.pending = source_app;
        match self.prompt.take() {
            Some((d, _)) => vec![Output::Cleared {
                app_id: d.app_id,
                reason: Cleared::Started,
            }],
            None => Vec::new(),
        }
    }

    /// The meeting records `process` (rule 7): tracked when the start came from a prompt or
    /// its pid is a current holder; a system-audio or unrelated pick is not.
    pub fn recording(&mut self, now: Instant, meeting: MeetingId, process: &ProcessInfo) {
        let app_id = self.pending.take().or_else(|| {
            self.holds
                .values()
                .find(|h| h.app.pid == process.pid)
                .map(|h| app_id(&h.app))
        });
        self.tracked = app_id.map(|app_id| Tracked {
            meeting,
            app_id,
            app: process.clone(),
            last_seen: now,
            asked: false,
        });
    }

    /// The meeting ended (or never began): back to Idle. Every hold restarts and the app
    /// that was recorded gets the cooldown, so stopping mid-call does not re-prompt at once.
    pub fn ended(&mut self, now: Instant) {
        self.pending = None;
        if let Some(t) = self.tracked.take() {
            self.cooldown.insert(t.app_id, now + COOLDOWN);
        }
        self.holds.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str, bundle: &str, pid: u32) -> ProcessInfo {
        ProcessInfo {
            pid,
            name: name.into(),
            bundle_id: Some(bundle.into()),
            active: true,
        }
    }

    fn zoom() -> ProcessInfo {
        app("zoom.us", "us.zoom.xos", 100)
    }

    fn teams() -> ProcessInfo {
        app("Teams", "com.microsoft.teams2", 200)
    }

    fn engine() -> Engine {
        Engine::new(
            super::super::DEFAULT_IGNORE
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
        )
    }

    const S: Duration = Duration::from_secs(1);

    /// Ticks every 2 s from `t0` for `secs`, returning everything emitted.
    fn hold(
        e: &mut Engine,
        t0: Instant,
        secs: u64,
        holders: &[ProcessInfo],
        quiet: bool,
    ) -> Vec<Output> {
        let mut out = Vec::new();
        let mut t = 0;
        while t <= secs {
            out.extend(e.tick(t0 + S * t as u32, holders, quiet));
            t += 2;
        }
        out
    }

    fn detected(out: &[Output]) -> Vec<&Detection> {
        out.iter()
            .filter_map(|o| match o {
                Output::Detected(d) => Some(&**d),
                _ => None,
            })
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn fourteen_seconds_is_nothing_fifteen_is_one_prompt() {
        let mut e = engine();
        let t0 = Instant::now();
        assert!(hold(&mut e, t0, 14, &[zoom()], false).is_empty());
        let out = e.tick(t0 + S * 15, &[zoom()], false);
        let d = detected(&out);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].app_id, "us.zoom.xos");
        assert!(d[0].also.is_empty());
        // Still prompting: no second event while it holds.
        assert!(hold(&mut e, t0 + S * 16, 60, &[zoom()], false).is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn not_now_is_quiet_for_ten_minutes_then_prompts_again() {
        let mut e = engine();
        let t0 = Instant::now();
        hold(&mut e, t0, 16, &[zoom()], false);
        let out = e.dismiss(t0 + S * 16, "us.zoom.xos", false);
        assert_eq!(
            out,
            vec![Output::Cleared {
                app_id: "us.zoom.xos".into(),
                reason: Cleared::NotNow
            }]
        );
        let t1 = t0 + S * 17;
        assert!(hold(&mut e, t1, COOLDOWN_SECS - 2, &[zoom()], false).is_empty());
        let out = hold(&mut e, t1 + S * (COOLDOWN_SECS as u32), 2, &[zoom()], false);
        assert_eq!(detected(&out).len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn two_holders_one_event_with_also() {
        let mut e = engine();
        let t0 = Instant::now();
        e.tick(t0, &[zoom()], false);
        let out = hold(&mut e, t0 + S * 2, 20, &[zoom(), teams()], false);
        let d = detected(&out);
        assert_eq!(d.len(), 1, "{out:?}");
        assert_eq!(d[0].app_id, "us.zoom.xos", "the longest-held app");
        assert_eq!(d[0].also, vec![teams()]);
    }

    #[tokio::test(start_paused = true)]
    async fn ignored_holder_is_invisible() {
        let mut e = engine();
        let t0 = Instant::now();
        let siri = app("Siri", "com.apple.Siri", 300);
        assert!(hold(&mut e, t0, 40, std::slice::from_ref(&siri), false).is_empty());
        let out = hold(&mut e, t0 + S * 42, 20, &[siri, zoom()], false);
        let d = detected(&out);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].app_id, "us.zoom.xos");
        assert!(
            d[0].also.is_empty(),
            "ignored apps are not in `also` either"
        );
    }

    /// Minutes #244: a dictation tool holding the mic is not a call.
    #[tokio::test(start_paused = true)]
    async fn dictation_app_never_prompts() {
        let mut e = engine();
        let t0 = Instant::now();
        let flow = app("Wispr Flow", "com.electron.wispr-flow", 400);
        assert!(hold(&mut e, t0, 120, &[flow], false).is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn live_meeting_is_quiet() {
        let mut e = engine();
        let t0 = Instant::now();
        assert!(hold(&mut e, t0, 60, &[zoom()], true).is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn dictation_pauses_the_clock_without_resetting_it() {
        let mut e = engine();
        let t0 = Instant::now();
        hold(&mut e, t0, 8, &[zoom()], false); // 8 s held
        let t1 = t0 + S * 10;
        assert!(hold(&mut e, t1, 60, &[zoom()], true).is_empty()); // dictating: paused
        let t2 = t1 + S * 62;
        // 8 s banked; 7 more needed. 6 → nothing, 8 → prompt.
        assert!(hold(&mut e, t2, 4, &[zoom()], false).is_empty());
        let out = e.tick(t2 + S * 8, &[zoom()], false);
        assert_eq!(detected(&out).len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn release_asks_once_per_episode() {
        let mut e = engine();
        let t0 = Instant::now();
        hold(&mut e, t0, 16, &[zoom()], false);
        let t1 = t0 + S * 18;
        assert_eq!(e.starting(Some("us.zoom.xos".into())).len(), 1);
        let meeting = MeetingId::new();
        e.recording(t1, meeting, &zoom());
        // Recording: quiet, and zoom still holds; `gone` is the last tick that saw it.
        assert!(hold(&mut e, t1, 10, &[zoom()], true).is_empty());
        let gone = t1 + S * 10;
        // Zoom lets go: 28 s nothing, 30 s one ask, then silence while the release continues.
        let out = hold(&mut e, gone + S * 2, 26, &[], true);
        assert!(out.is_empty(), "{out:?}");
        let out = e.tick(gone + S * 30, &[], true);
        assert!(
            matches!(&out[..], [Output::AppReleased { meeting: m, released_for_s: 30, .. }] if *m == meeting),
            "{out:?}"
        );
        let out = hold(&mut e, gone + S * 32, 120, &[], true);
        assert!(out.is_empty(), "no re-send on the same episode: {out:?}");
        // Back, then gone again: a new episode asks once more; nothing stops by itself.
        let back = gone + S * 160;
        assert!(hold(&mut e, back, 10, &[zoom()], true).is_empty());
        let gone2 = back + S * 10;
        assert!(hold(&mut e, gone2 + S * 2, 26, &[], true).is_empty());
        let out = e.tick(gone2 + S * 30, &[], true);
        assert!(
            matches!(
                &out[..],
                [Output::AppReleased {
                    released_for_s: 30,
                    ..
                }]
            ),
            "{out:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn never_grows_the_ignore_list_and_the_app_never_prompts() {
        let mut e = engine();
        let t0 = Instant::now();
        hold(&mut e, t0, 16, &[zoom()], false);
        let out = e.dismiss(t0 + S * 16, "us.zoom.xos", true);
        assert_eq!(
            out,
            vec![Output::Cleared {
                app_id: "us.zoom.xos".into(),
                reason: Cleared::Never
            }]
        );
        assert!(e.ignore.iter().any(|i| i == "us.zoom.xos"));
        assert!(hold(&mut e, t0 + S * 18, 120, &[zoom()], false).is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn unanswered_prompt_withdraws_thirty_seconds_after_release() {
        let mut e = engine();
        let t0 = Instant::now();
        hold(&mut e, t0, 16, &[zoom()], false); // prompt at the 16 s tick; last seen then
        let gone = t0 + S * 16;
        assert!(hold(&mut e, gone + S * 2, 26, &[], false).is_empty());
        let out = e.tick(gone + S * 30, &[], false);
        assert_eq!(
            out,
            vec![Output::Cleared {
                app_id: "us.zoom.xos".into(),
                reason: Cleared::Released
            }]
        );
        assert!(e.prompt().is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn a_manual_start_clears_the_prompt_and_stopping_does_not_nag() {
        let mut e = engine();
        let t0 = Instant::now();
        hold(&mut e, t0, 16, &[zoom()], false);
        assert!(e.prompt().is_some());
        let out = e.starting(None);
        assert!(matches!(
            &out[..],
            [Output::Cleared {
                reason: Cleared::Started,
                ..
            }]
        ));
        let t1 = t0 + S * 18;
        e.recording(t1, MeetingId::new(), &zoom()); // pid matches a holder → tracked
        e.ended(t1 + S * 60);
        // Zoom still holds after the stop: cooled down, not re-prompted.
        assert!(hold(&mut e, t1 + S * 62, 60, &[zoom()], false).is_empty());
    }

    #[test]
    fn app_ids_per_os() {
        assert_eq!(app_id(&zoom()), "us.zoom.xos");
        let bare = ProcessInfo {
            pid: 1,
            name: "Zoom".into(),
            bundle_id: None,
            active: true,
        };
        if cfg!(target_os = "windows") {
            assert_eq!(app_id(&bare), "zoom.exe");
        } else {
            assert_eq!(app_id(&bare), "Zoom");
        }
    }
}
