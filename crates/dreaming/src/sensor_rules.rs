//! The gate rule (design doc 136 §4 phase 0) as a pure function. `sensor.py`
//! implements the same table in the Monty sandbox and must be kept in
//! lockstep: every row of [`tests`] names a reason string the Python prints.

use chrono::{DateTime, Utc};

use crate::config::{DreamConfig, HeavyDay};
use crate::types::SignalsSnapshot;

/// Battery floor when off mains (136 §4 phase 0).
pub const BATTERY_MIN_PCT: u8 = 40;
/// `signals.json` older than this is a dead writer (R-19, 136 D14): the
/// gate holds. The [`crate::signals::HEARTBEAT`] re-stamps it every minute.
pub const STALE_SECS: i64 = 3 * 60;

/// `"HH:MM"` → minutes since local midnight; `None` when malformed.
pub fn parse_hhmm(s: &str) -> Option<u32> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// Minutes since local midnight of `now` shifted by `tz_offset_minutes`.
pub fn local_minutes(now: DateTime<Utc>, tz_offset_minutes: i32) -> u32 {
    let secs = now.timestamp().rem_euclid(86_400) + i64::from(tz_offset_minutes) * 60;
    (secs.rem_euclid(86_400) / 60) as u32
}

/// Minutes elapsed since the window's start, when `local` lies inside
/// `[start, end)` (which may cross midnight); `None` when outside.
pub fn minutes_into_window(local: u32, start: u32, end: u32) -> Option<u32> {
    let len = (end + 1440 - start) % 1440;
    let into = (local + 1440 - start) % 1440;
    (len > 0 && into < len).then_some(into)
}

/// The gate: `Ok(())` fires; `Err(reason)` is the sensor's one-line answer.
/// Every rule of 136 §4 phase 0 in order; the first failure wins.
pub fn gate(
    cfg: &DreamConfig,
    s: &SignalsSnapshot,
    now: DateTime<Utc>,
    tz_offset_minutes: i32,
    self_engine: Option<&str>,
) -> Result<(), &'static str> {
    if let Some(me) = self_engine
        && !s.engine.is_empty()
        && s.engine != me
    {
        return Err("another engine owns dreaming");
    }
    // R-19: a frozen file (the plugin off, the actor dead) must not keep passing.
    if let Some(w) = s.written_at.as_deref()
        && parse_rfc3339(w).is_none_or(|t| now.signed_duration_since(t).num_seconds() > STALE_SECS)
    {
        return Err("signals stale");
    }
    let (Some(start), Some(end)) = (parse_hhmm(&cfg.window[0]), parse_hhmm(&cfg.window[1])) else {
        return Err("window malformed");
    };
    let local = local_minutes(now, tz_offset_minutes);
    let Some(into) = minutes_into_window(local, start, end) else {
        return Err("outside the window");
    };
    let idle_ok = match s.last_root_turn_at.as_deref().and_then(parse_rfc3339) {
        Some(t) => now.signed_duration_since(t).num_minutes() >= i64::from(cfg.idle_minutes),
        None => true,
    };
    if !idle_ok {
        return Err("a root session turned recently");
    }
    if s.desks_doing > 0 {
        return Err("a desk is doing");
    }
    let heavy = is_heavy(&cfg.heavy_day, s);
    let window_start = now - chrono::Duration::minutes(i64::from(into));
    let dreamt_this_window = s
        .last_dream_at
        .as_deref()
        .and_then(parse_rfc3339)
        .is_some_and(|t| t >= window_start);
    if dreamt_this_window && !heavy {
        return Err("already dreamt this window");
    }
    if s.new_concepts_since == 0 && s.sessions_since == 0 {
        return Err("nothing new since the last dream");
    }
    let power_ok = match (s.on_ac, s.battery_pct) {
        (Some(true), _) => true,
        (_, Some(pct)) => pct >= BATTERY_MIN_PCT,
        (Some(false), None) => false,
        (None, None) => true,
    };
    if !power_ok {
        return Err("on battery below 40%");
    }
    Ok(())
}

/// 136 §4.1: a heavy day earns a second window.
pub fn is_heavy(h: &HeavyDay, s: &SignalsSnapshot) -> bool {
    s.new_concepts_since >= h.concepts || s.sessions_since >= h.sessions
}

fn parse_rfc3339(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 8, h, m, 0).unwrap()
    }

    fn quiet_night() -> SignalsSnapshot {
        SignalsSnapshot {
            last_root_turn_at: Some("2026-10-08T01:00:00Z".into()),
            desks_doing: 0,
            new_concepts_since: 12,
            sessions_since: 3,
            last_dream_at: Some("2026-10-07T03:00:00Z".into()),
            on_ac: Some(true),
            battery_pct: None,
            engine: "me".into(),
            written_at: None,
            tz_offset_minutes: 0,
        }
    }

    type Row = (
        &'static str,
        DateTime<Utc>,
        i32,
        SignalsSnapshot,
        Result<(), &'static str>,
    );

    /// The table `sensor.py` mirrors: one row per rule, the reason text verbatim.
    fn rows() -> Vec<Row> {
        let s = quiet_night();
        vec![
            (
                "fires at 02:00 local, idle 60 min",
                at(2, 0),
                0,
                s.clone(),
                Ok(()),
            ),
            (
                "another engine",
                at(2, 0),
                0,
                SignalsSnapshot {
                    engine: "other".into(),
                    ..s.clone()
                },
                Err("another engine owns dreaming"),
            ),
            (
                "signals stale: written 45 min ago",
                at(2, 0),
                0,
                SignalsSnapshot {
                    written_at: Some("2026-10-08T01:15:00Z".into()),
                    ..s.clone()
                },
                Err("signals stale"),
            ),
            (
                "signals fresh: written 2 min ago",
                at(2, 0),
                0,
                SignalsSnapshot {
                    written_at: Some("2026-10-08T01:58:00Z".into()),
                    ..s.clone()
                },
                Ok(()),
            ),
            (
                "signals stale: written_at unparseable",
                at(2, 0),
                0,
                SignalsSnapshot {
                    written_at: Some("yesterday".into()),
                    ..s.clone()
                },
                Err("signals stale"),
            ),
            (
                "outside: 00:30 local",
                at(0, 30),
                0,
                s.clone(),
                Err("outside the window"),
            ),
            (
                "outside: 06:00 local (end exclusive)",
                at(6, 0),
                0,
                s.clone(),
                Err("outside the window"),
            ),
            (
                "tz: 02:00 UTC is 22:00 local at −240",
                at(2, 0),
                -240,
                s.clone(),
                Err("outside the window"),
            ),
            (
                "tz: 23:00 UTC is 02:00 local at +180",
                at(23, 0),
                180,
                s.clone(),
                Ok(()),
            ),
            (
                "idle: turned 10 min ago",
                at(1, 10),
                0,
                s.clone(),
                Err("a root session turned recently"),
            ),
            (
                "idle: no turn ever recorded",
                at(1, 10),
                0,
                SignalsSnapshot {
                    last_root_turn_at: None,
                    ..s.clone()
                },
                Ok(()),
            ),
            (
                "desk doing",
                at(2, 0),
                0,
                SignalsSnapshot {
                    desks_doing: 1,
                    ..s.clone()
                },
                Err("a desk is doing"),
            ),
            (
                "already dreamt at 01:30 this window",
                at(3, 0),
                0,
                SignalsSnapshot {
                    last_dream_at: Some("2026-10-08T01:30:00Z".into()),
                    ..s.clone()
                },
                Err("already dreamt this window"),
            ),
            (
                "heavy day allows a second window",
                at(3, 0),
                0,
                SignalsSnapshot {
                    last_dream_at: Some("2026-10-08T01:30:00Z".into()),
                    new_concepts_since: 300,
                    ..s.clone()
                },
                Ok(()),
            ),
            (
                "nothing new",
                at(2, 0),
                0,
                SignalsSnapshot {
                    new_concepts_since: 0,
                    sessions_since: 0,
                    ..s.clone()
                },
                Err("nothing new since the last dream"),
            ),
            (
                "one session is enough",
                at(2, 0),
                0,
                SignalsSnapshot {
                    new_concepts_since: 0,
                    sessions_since: 1,
                    ..s.clone()
                },
                Ok(()),
            ),
            (
                "battery 39 off mains",
                at(2, 0),
                0,
                SignalsSnapshot {
                    on_ac: Some(false),
                    battery_pct: Some(39),
                    ..s.clone()
                },
                Err("on battery below 40%"),
            ),
            (
                "battery 40 off mains",
                at(2, 0),
                0,
                SignalsSnapshot {
                    on_ac: Some(false),
                    battery_pct: Some(40),
                    ..s.clone()
                },
                Ok(()),
            ),
            (
                "off mains, battery unknown",
                at(2, 0),
                0,
                SignalsSnapshot {
                    on_ac: Some(false),
                    battery_pct: None,
                    ..s.clone()
                },
                Err("on battery below 40%"),
            ),
            (
                "power unknown (not macOS) fires",
                at(2, 0),
                0,
                SignalsSnapshot {
                    on_ac: None,
                    battery_pct: None,
                    ..s.clone()
                },
                Ok(()),
            ),
        ]
    }

    #[test]
    fn rule_table() {
        let cfg = DreamConfig::default(); // window 01:00–06:00, idle 20
        let s = quiet_night();
        for (name, now, tz, snap, want) in rows() {
            assert_eq!(gate(&cfg, &snap, now, tz, Some("me")), want, "{name}");
        }
        // No self engine given: the engine rule is skipped.
        assert_eq!(
            gate(
                &cfg,
                &SignalsSnapshot {
                    engine: "other".into(),
                    ..s
                },
                at(2, 0),
                0,
                None
            ),
            Ok(())
        );
    }

    /// `sensor.py`'s `gate` over the same rows (python3 on PATH; skipped
    /// otherwise): the two tables answer the same reason strings.
    #[test]
    fn python_sensor_answers_the_same_table() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("sensor.py"), crate::SENSOR).unwrap();
        let mut cases = Vec::new();
        for (name, now, tz, snap, want) in rows() {
            let mut snap = snap;
            snap.tz_offset_minutes = tz;
            cases.push(serde_json::json!({
                "name": name,
                "now": now.timestamp(),
                "signals": snap,
                "args": {"window": "01:00-06:00", "idle": "20", "heavy_concepts": "300", "heavy_sessions": "40", "engine": "me"},
                "want": want.err().unwrap_or(""),
            }));
        }
        let script = "import json, sys, datetime\nimport sensor\nfor c in json.load(sys.stdin):\n    now = datetime.datetime.fromtimestamp(c['now'], datetime.timezone.utc)\n    got = sensor.gate(c['args'], c['signals'], now)\n    print(json.dumps([c['name'], got, c['want']]))\n";
        let out = match std::process::Command::new("python3")
            .arg("-c")
            .arg(script)
            .current_dir(dir.path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
        {
            Ok(mut child) => {
                use std::io::Write as _;
                child
                    .stdin
                    .take()
                    .unwrap()
                    .write_all(serde_json::to_string(&cases).unwrap().as_bytes())
                    .unwrap();
                child.wait_with_output().unwrap()
            }
            Err(_) => {
                eprintln!("python3 not on PATH; lockstep test skipped");
                return;
            }
        };
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let text = String::from_utf8_lossy(&out.stdout);
        let mut n = 0;
        for line in text.lines() {
            let row: (String, String, String) = serde_json::from_str(line).unwrap();
            assert_eq!(row.1, row.2, "python disagrees on {:?}", row.0);
            n += 1;
        }
        assert_eq!(n, rows().len());
    }

    /// `read_file` answers a `[read_file …]` header line before the text even
    /// with `numbers=False`; `check` must parse the JSON under it (the night
    /// never fired before this: every check was `events=no-signals`).
    #[test]
    fn python_sensor_reads_signals_under_the_read_file_header() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("sensor.py"), crate::SENSOR).unwrap();
        let script = "import asyncio, json\nimport sensor\nasync def read_file(path, numbers=True):\n    body = json.dumps({'last_root_turn_at': None, 'desks_doing': 0, 'new_concepts_since': 3, 'sessions_since': 1, 'last_dream_at': None, 'on_ac': True, 'tz_offset_minutes': 0})\n    return '[read_file ' + path + ': 1 lines, ' + str(len(body)) + ' bytes; showing lines 1–1]\\n' + body + '\\n'\nsensor.read_file = read_file\nr = asyncio.run(sensor.check({}, {'window': '00:00-23:59'}))\nprint(json.dumps(r))\n";
        let out = match std::process::Command::new("python3")
            .arg("-c")
            .arg(script)
            .current_dir(dir.path())
            .output()
        {
            Ok(o) => o,
            Err(_) => {
                eprintln!("python3 not on PATH; header test skipped");
                return;
            }
        };
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(r["fire"], true, "{r}");
        assert!(
            r["payload"]
                .as_str()
                .unwrap()
                .starts_with("subject=dreaming events=go\ngate: open"),
            "{r}"
        );
    }

    #[test]
    fn window_crossing_midnight() {
        let cfg = DreamConfig {
            window: ["23:00".into(), "05:00".into()],
            ..DreamConfig::default()
        };
        let s = SignalsSnapshot {
            last_root_turn_at: None,
            last_dream_at: Some("2026-10-07T23:30:00Z".into()),
            ..quiet_night()
        };
        // 00:30 is inside a window that began at 23:00 yesterday; the 23:30 dream counts.
        assert_eq!(
            gate(&cfg, &s, at(0, 30), 0, None),
            Err("already dreamt this window")
        );
        // The previous night's dream does not.
        let earlier = SignalsSnapshot {
            last_dream_at: Some("2026-10-07T03:00:00Z".into()),
            ..s.clone()
        };
        assert_eq!(gate(&cfg, &earlier, at(0, 30), 0, None), Ok(()));
        assert_eq!(
            gate(&cfg, &s, at(12, 0), 0, None),
            Err("outside the window")
        );
        assert_eq!(minutes_into_window(30, 23 * 60, 5 * 60), Some(90));
        assert_eq!(minutes_into_window(22 * 60, 23 * 60, 5 * 60), None);
        assert_eq!(parse_hhmm("24:00"), None);
        assert_eq!(parse_hhmm("7:05"), Some(425));
        assert_eq!(local_minutes(at(0, 30), -60), 23 * 60 + 30);
    }
}
