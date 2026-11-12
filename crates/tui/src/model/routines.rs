use super::*;

/// The `/schedules` usage line, shown on a bad argument.
pub const SCHEDULES_USAGE: &str =
    "usage: /schedules [run <name>|test <name>|history <name>|pause <name>|resume <name>]";

/// `/schedules`: one row per file — `name · when · next · runs · last · result · state`,
/// plus `last check` (`10:15:03 quiet 1200 ms`, 40 §6) when any row is a watcher that was checked.
/// `runs` is `—` for a plain schedule, `N` or `N/max` for a loop (33 §3.2);
/// `state` is `running`, `off`, `error: …` (then `next` is `—`), or empty.
pub fn schedules_table(rows: &[schedule::ScheduleInfo]) -> String {
    if rows.is_empty() {
        return "no schedules".into();
    }
    let stamp = |t: Option<chrono::DateTime<chrono::Local>>| {
        t.map_or("—".to_owned(), |t| t.format("%a %d %b %H:%M").to_string())
    };
    let checks = rows.iter().any(|s| s.last_check.is_some());
    let mut header = "name · when · next · runs · last · result · state".to_owned();
    if checks {
        header.push_str(" · last check");
    }
    let mut out = vec![header];
    out.extend(rows.iter().map(|s| {
        let state = match (&s.error, s.running, s.enabled) {
            (Some(e), ..) => format!("error: {e}"),
            (None, true, _) => "running".to_owned(),
            (None, false, false) => "off".to_owned(),
            (None, false, true) => String::new(),
        };
        let next = if s.error.is_some() { None } else { s.next };
        let runs = match s.max_runs {
            Some(max) => format!("{}/{max}", s.runs),
            None if s.until.is_some() => s.runs.to_string(),
            None => "—".to_owned(),
        };
        let mut row = format!(
            "{} · {} · {} · {runs} · {} · {} · {state}",
            s.name,
            s.when,
            stamp(next),
            stamp(s.last),
            s.last_result.as_deref().unwrap_or("—"),
        );
        if checks {
            row.push_str(&s.last_check.as_ref().map_or_else(
                || " · —".to_owned(),
                |c| {
                    format!(
                        " · {} {} {} ms",
                        c.at.format("%H:%M:%S"),
                        c.outcome,
                        c.elapsed_ms
                    )
                },
            ));
        }
        row
    }));
    out.join("\n")
}

/// `/schedules history <name>`: `fired · took · result · session · report` rows,
/// newest first; `took` is `…` while the run is still going, `report` is the
/// run's first line cut at 60 chars. A loop with `until` or `approvals: false`
/// gets a line naming those above the header (33 §6).
pub fn schedule_history_table(
    name: &str,
    info: Option<&schedule::ScheduleInfo>,
    rows: &[schedule::RunInfo],
) -> String {
    if rows.is_empty() {
        return format!("no runs of {name}");
    }
    let mut out = Vec::with_capacity(rows.len() + 2);
    if let Some(info) = info {
        let mut loop_line: Vec<String> = Vec::new();
        if let Some(until) = &info.until {
            loop_line.push(format!("until: {until}"));
        }
        if !info.approvals {
            loop_line.push("approvals off".to_owned());
        }
        if !loop_line.is_empty() {
            out.push(format!("{name}: {}", loop_line.join(" · ")));
        }
    }
    out.push(format!("{name}: fired · took · result · session · report"));
    out.extend(rows.iter().map(|r| {
        let took = r
            .finished_at
            .map_or("…".to_owned(), |f| elapsed(f - r.fired_at));
        let session = r.session.map_or("—".to_owned(), |s| {
            s.to_string().chars().take(8).collect()
        });
        let report = r
            .report
            .as_deref()
            .and_then(|t| t.lines().map(str::trim).find(|l| !l.is_empty()))
            .map_or("—".to_owned(), |l| {
                if l.chars().count() > 60 {
                    let mut cut: String = l.chars().take(59).collect();
                    cut.push('…');
                    cut
                } else {
                    l.to_owned()
                }
            });
        format!(
            "{} · {took} · {} · {session} · {report}",
            r.fired_at.format("%a %d %b %H:%M"),
            r.result.as_deref().unwrap_or("—"),
        )
    }));
    out.join("\n")
}
