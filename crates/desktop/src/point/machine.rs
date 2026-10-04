//! 96 §4.1: the voice key's gesture machine — hold to talk, double-tap to lock — as a pure
//! reducer over the Carbon `Pressed`/`Released` stream, Esc, the panel's replies and timer ticks.
//! No clock, no I/O: the driver in `mod.rs` owns the timers and executes the [`Effect`]s.

/// Release before this many ms is a tap, not a hold.
pub const HOLD_MS: u64 = 200;
/// A second press within this many ms of a tap's release locks hands-free.
pub const DOUBLE_TAP_MS: u64 = 300;
/// Notes shorter than this are discarded with [`HINT`] (89's `MIN_NOTE_MS`).
pub const MIN_NOTE_MS: u64 = 800;
/// Pointer sampler rate (§4.2).
pub const SAMPLE_HZ: u64 = 60;
/// How long the panel's `done` phase lingers before it calls `point_done` — the panel's timer, kept
/// here so every §4.1 constant has one home.
#[allow(dead_code)]
pub const DONE_MS: u64 = 6_000;
/// Shown on a missed double-tap, a too-short note, and Esc mid-gesture.
pub const HINT: &str = "Hold to talk · double-tap to lock";

/// Times are ms since process start (the driver's clock); `t0` is the press that opened the gesture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum State {
    Idle,
    Armed { t0: u64 },
    Holding { t0: u64 },
    Tap1 { t0: u64, released: u64 },
    Locked { t0: u64 },
    Sending { t0: u64 },
    Replying,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Pressed,
    Released,
    Esc,
    /// A timer the driver armed on entering `Armed` / `Tap1`; `now` is the time it was armed for.
    /// The reducer judges by the clock it is stepped with, so a late tick is still correct.
    Tick {
        now: u64,
    },
    PanelDone,
    Cancel,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Overlay + panel + dictation at 0 ms.
    Start,
    ShowLock,
    /// `reason` = `"release"` | `"tap"` — the `point:stop` payload's.
    Stop {
        reason: &'static str,
    },
    /// `reason` = `"esc"` | `"short"` | `"double-tap-missed"` | `"cancel"` — the `point:cancel` payload's.
    Discard {
        reason: &'static str,
        hint: Option<&'static str>,
    },
    Conceal,
    Refuse {
        reason: String,
    },
}

/// One transition. `refused` is the panel's standing reason (`point_refuse`), answered instead of
/// `Start` on a press from `Idle`/`Replying`.
pub fn step(s: State, e: Event, now: u64, refused: Option<&str>) -> (State, Vec<Effect>) {
    use Effect::*;
    use Event as E;
    use State as S;
    let stay = (s, vec![]);
    match (s, e) {
        (S::Idle | S::Replying, E::Pressed) => match refused {
            Some(reason) => (
                s,
                vec![Refuse {
                    reason: reason.to_owned(),
                }],
            ),
            None => (S::Armed { t0: now }, vec![Start]),
        },
        (S::Armed { t0 }, E::Released) => {
            if now.saturating_sub(t0) >= HOLD_MS {
                send(t0, now, "release")
            } else {
                (S::Tap1 { t0, released: now }, vec![])
            }
        }
        (S::Armed { t0 }, E::Tick { .. }) if now.saturating_sub(t0) >= HOLD_MS => {
            (S::Holding { t0 }, vec![])
        }
        (S::Holding { t0 }, E::Released) => send(t0, now, "release"),
        (S::Armed { .. } | S::Holding { .. } | S::Locked { .. }, E::Esc) => {
            discard("esc", Some(HINT))
        }
        (S::Tap1 { .. } | S::Sending { .. }, E::Esc) => discard("esc", None),
        (S::Armed { .. } | S::Holding { .. } | S::Tap1 { .. } | S::Locked { .. }, E::Cancel) => {
            discard("cancel", None)
        }
        (S::Tap1 { t0, released }, E::Pressed) => {
            if now.saturating_sub(released) <= DOUBLE_TAP_MS {
                (S::Locked { t0 }, vec![ShowLock])
            } else {
                // The tick that should have closed the window has not landed yet: close it and
                // treat this press as a fresh gesture rather than swallow it.
                (
                    S::Armed { t0: now },
                    vec![
                        Discard {
                            reason: "double-tap-missed",
                            hint: Some(HINT),
                        },
                        Start,
                    ],
                )
            }
        }
        (S::Tap1 { released, .. }, E::Tick { .. })
            if now.saturating_sub(released) > DOUBLE_TAP_MS =>
        {
            discard("double-tap-missed", Some(HINT))
        }
        (S::Locked { t0 }, E::Pressed) => send(t0, now, "tap"),
        (S::Sending { .. }, E::PanelDone) => (S::Replying, vec![]),
        (S::Sending { .. }, E::Cancel) => (S::Idle, vec![]),
        (S::Replying, E::Esc | E::PanelDone | E::Cancel) => (S::Idle, vec![Conceal]),
        _ => stay,
    }
}

/// A note ends: send it, or discard one shorter than [`MIN_NOTE_MS`].
fn send(t0: u64, now: u64, reason: &'static str) -> (State, Vec<Effect>) {
    if now.saturating_sub(t0) < MIN_NOTE_MS {
        discard("short", Some(HINT))
    } else {
        (State::Sending { t0 }, vec![Effect::Stop { reason }])
    }
}

fn discard(reason: &'static str, hint: Option<&'static str>) -> (State, Vec<Effect>) {
    (State::Idle, vec![Effect::Discard { reason, hint }])
}

#[cfg(test)]
mod tests {
    use super::Effect::*;
    use super::Event::*;
    use super::State::*;
    use super::*;

    fn s(st: State, e: Event, now: u64) -> (State, Vec<Effect>) {
        step(st, e, now, None)
    }

    #[test]
    fn idle_press_starts_and_arms() {
        assert_eq!(s(Idle, Pressed, 10), (Armed { t0: 10 }, vec![Start]));
        for e in [Released, Esc, Tick { now: 5 }, PanelDone, Cancel] {
            assert_eq!(s(Idle, e, 10), (Idle, vec![]), "{e:?}");
        }
    }

    #[test]
    fn refused_press_answers_refuse_not_start() {
        for st in [Idle, Replying] {
            assert_eq!(
                step(st, Pressed, 10, Some("meeting")),
                (
                    st,
                    vec![Refuse {
                        reason: "meeting".into()
                    }]
                )
            );
        }
        // Only a press is refused; nothing else changes.
        assert_eq!(
            step(Holding { t0: 0 }, Released, 1000, Some("meeting")),
            (Sending { t0: 0 }, vec![Stop { reason: "release" }])
        );
    }

    #[test]
    fn auto_repeat_press_is_ignored() {
        for st in [Armed { t0: 0 }, Holding { t0: 0 }] {
            assert_eq!(s(st, Pressed, 50), (st, vec![]), "{st:?}");
        }
        // Locked's press is the send, not a repeat — covered below; Sending ignores presses too.
        assert_eq!(
            s(Sending { t0: 0 }, Pressed, 50),
            (Sending { t0: 0 }, vec![])
        );
    }

    #[test]
    fn armed_release_before_hold_is_tap1() {
        assert_eq!(
            s(Armed { t0: 0 }, Released, HOLD_MS - 1),
            (
                Tap1 {
                    t0: 0,
                    released: HOLD_MS - 1
                },
                vec![]
            )
        );
    }

    #[test]
    fn armed_release_after_hold_sends() {
        assert_eq!(
            s(Armed { t0: 0 }, Released, MIN_NOTE_MS),
            (Sending { t0: 0 }, vec![Stop { reason: "release" }])
        );
        // A release past HOLD_MS but under MIN_NOTE_MS (a late hold tick) is the same short note
        // Holding would discard.
        assert_eq!(
            s(Armed { t0: 0 }, Released, HOLD_MS + 10),
            (
                Idle,
                vec![Discard {
                    reason: "short",
                    hint: Some(HINT)
                }]
            )
        );
    }

    #[test]
    fn armed_tick_moves_to_holding_only_after_hold() {
        assert_eq!(
            s(Armed { t0: 0 }, Tick { now: HOLD_MS }, HOLD_MS),
            (Holding { t0: 0 }, vec![])
        );
        assert_eq!(
            s(Armed { t0: 0 }, Tick { now: 50 }, HOLD_MS - 1),
            (Armed { t0: 0 }, vec![])
        );
    }

    #[test]
    fn holding_release_sends_or_discards_short() {
        assert_eq!(
            s(Holding { t0: 0 }, Released, MIN_NOTE_MS),
            (Sending { t0: 0 }, vec![Stop { reason: "release" }])
        );
        assert_eq!(
            s(Holding { t0: 0 }, Released, MIN_NOTE_MS - 1),
            (
                Idle,
                vec![Discard {
                    reason: "short",
                    hint: Some(HINT)
                }]
            )
        );
    }

    #[test]
    fn second_press_inside_window_locks() {
        assert_eq!(
            s(
                Tap1 {
                    t0: 0,
                    released: 100
                },
                Pressed,
                100 + DOUBLE_TAP_MS
            ),
            (Locked { t0: 0 }, vec![ShowLock])
        );
    }

    #[test]
    fn late_second_press_closes_the_tap_and_starts_anew() {
        assert_eq!(
            s(
                Tap1 {
                    t0: 0,
                    released: 100
                },
                Pressed,
                100 + DOUBLE_TAP_MS + 1
            ),
            (
                Armed { t0: 401 },
                vec![
                    Discard {
                        reason: "double-tap-missed",
                        hint: Some(HINT)
                    },
                    Start
                ]
            )
        );
    }

    #[test]
    fn tap1_tick_after_window_discards_with_hint() {
        let st = Tap1 {
            t0: 0,
            released: 100,
        };
        assert_eq!(s(st, Tick { now: 401 }, 100 + DOUBLE_TAP_MS), (st, vec![]));
        assert_eq!(
            s(st, Tick { now: 401 }, 100 + DOUBLE_TAP_MS + 1),
            (
                Idle,
                vec![Discard {
                    reason: "double-tap-missed",
                    hint: Some("Hold to talk · double-tap to lock")
                }]
            )
        );
        assert_eq!(s(st, Released, 150), (st, vec![]));
    }

    #[test]
    fn locked_press_sends_or_discards_short() {
        assert_eq!(
            s(Locked { t0: 0 }, Pressed, MIN_NOTE_MS - 1),
            (
                Idle,
                vec![Discard {
                    reason: "short",
                    hint: Some(HINT)
                }]
            )
        );
        assert_eq!(
            s(Locked { t0: 0 }, Pressed, MIN_NOTE_MS),
            (Sending { t0: 0 }, vec![Stop { reason: "tap" }])
        );
        assert_eq!(
            s(Locked { t0: 0 }, Released, 50),
            (Locked { t0: 0 }, vec![])
        );
        assert_eq!(
            s(Locked { t0: 0 }, Tick { now: 50 }, 50),
            (Locked { t0: 0 }, vec![])
        );
    }

    #[test]
    fn esc_in_every_active_state() {
        let hinted = |st| {
            assert_eq!(
                s(st, Esc, 50),
                (
                    Idle,
                    vec![Discard {
                        reason: "esc",
                        hint: Some(HINT)
                    }]
                ),
                "{st:?}"
            );
        };
        hinted(Armed { t0: 0 });
        hinted(Holding { t0: 0 });
        hinted(Locked { t0: 0 });
        for st in [
            Tap1 {
                t0: 0,
                released: 10,
            },
            Sending { t0: 0 },
        ] {
            assert_eq!(
                s(st, Esc, 50),
                (
                    Idle,
                    vec![Discard {
                        reason: "esc",
                        hint: None
                    }]
                ),
                "{st:?}"
            );
        }
        assert_eq!(s(Replying, Esc, 50), (Idle, vec![Conceal]));
    }

    #[test]
    fn cancel_in_every_active_state() {
        for st in [
            Armed { t0: 0 },
            Holding { t0: 0 },
            Tap1 {
                t0: 0,
                released: 10,
            },
            Locked { t0: 0 },
        ] {
            assert_eq!(
                s(st, Cancel, 50),
                (
                    Idle,
                    vec![Discard {
                        reason: "cancel",
                        hint: None
                    }]
                ),
                "{st:?}"
            );
        }
        assert_eq!(s(Sending { t0: 0 }, Cancel, 50), (Idle, vec![]));
        assert_eq!(s(Replying, Cancel, 50), (Idle, vec![Conceal]));
    }

    #[test]
    fn panel_done_only_matters_in_sending_and_replying() {
        assert_eq!(s(Sending { t0: 0 }, PanelDone, 50), (Replying, vec![]));
        assert_eq!(s(Replying, PanelDone, 50), (Idle, vec![Conceal]));
        for st in [
            Armed { t0: 0 },
            Holding { t0: 0 },
            Tap1 {
                t0: 0,
                released: 10,
            },
            Locked { t0: 0 },
        ] {
            assert_eq!(s(st, PanelDone, 50), (st, vec![]), "{st:?}");
        }
    }

    #[test]
    fn replying_press_starts_a_new_question() {
        assert_eq!(
            s(Replying, Pressed, 9000),
            (Armed { t0: 9000 }, vec![Start])
        );
        for e in [Released, Tick { now: 1 }] {
            assert_eq!(s(Replying, e, 9000), (Replying, vec![]), "{e:?}");
        }
    }

    #[test]
    fn full_hold_then_double_tap_sequence() {
        // Hold: press, tick → Holding, release at 1.2 s → Stop.
        let (st, fx) = s(Idle, Pressed, 0);
        assert_eq!(fx, vec![Start]);
        let (st, _) = s(st, Tick { now: HOLD_MS }, HOLD_MS);
        assert_eq!(st, Holding { t0: 0 });
        let (st, fx) = s(st, Released, 1200);
        assert_eq!(
            (st, fx),
            (Sending { t0: 0 }, vec![Stop { reason: "release" }])
        );
        let (st, _) = s(st, PanelDone, 1300);
        let (st, fx) = s(st, PanelDone, 7300);
        assert_eq!((st, fx), (Idle, vec![Conceal]));
        // Lock: press, release at 100, press at 250 → Locked, press at 2000 → Stop{tap}.
        let (st, _) = s(Idle, Pressed, 0);
        let (st, _) = s(st, Released, 100);
        let (st, fx) = s(st, Pressed, 250);
        assert_eq!((st, fx), (Locked { t0: 0 }, vec![ShowLock]));
        let (st, _) = s(st, Released, 300);
        let (st, fx) = s(st, Pressed, 2000);
        assert_eq!((st, fx), (Sending { t0: 0 }, vec![Stop { reason: "tap" }]));
    }

    #[test]
    fn state_serialises_tagged_snake_case() {
        assert_eq!(serde_json::to_string(&Idle).unwrap(), r#"{"state":"idle"}"#);
        assert_eq!(
            serde_json::to_string(&Tap1 { t0: 1, released: 2 }).unwrap(),
            r#"{"state":"tap1","t0":1,"released":2}"#
        );
    }
}
