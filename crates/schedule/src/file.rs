//! One schedule file: front matter says *when*, the body is the prompt (34 §2.2, §3).
//! Parsing, `next`/`missed` arithmetic, directory listing, and the rendering
//! plus atomic write the `schedule` tool uses. No actor, no store.

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use chrono::{DateTime, Local, NaiveDateTime};
use serde::{Deserialize, Serialize};

/// Smallest accepted `every:` interval.
const MIN_EVERY: Duration = Duration::from_secs(10);

/// When a schedule fires (34 §3).
#[derive(Clone, Debug)]
pub enum When {
    /// `cron:` — standard 5/6-field expression evaluated in local time.
    Cron(Box<croner::Cron>),
    /// `every:` — an interval measured from the last fire.
    Every(Duration),
    /// `at:` — one instant; the file is consumed by its fire.
    At(DateTime<Local>),
}

impl When {
    /// Next fire strictly after `now`: Cron → next occurrence; Every → `last + d`,
    /// or `now` when never fired; At → `t` while it is still ahead.
    pub fn next(
        &self,
        now: DateTime<Local>,
        last: Option<DateTime<Local>>,
    ) -> Option<DateTime<Local>> {
        match self {
            When::Cron(c) => c.find_next_occurrence(&now, false).ok(),
            When::Every(d) => Some(last.map_or(now, |l| l + *d)),
            When::At(t) => (*t > now).then_some(*t),
        }
    }

    /// Cron only: an occurrence lies in `(last, now]`. Every/At → false.
    pub fn missed(&self, last: DateTime<Local>, now: DateTime<Local>) -> bool {
        match self {
            When::Cron(c) => c.find_next_occurrence(&last, false).is_ok_and(|t| t <= now),
            When::Every(_) | When::At(_) => false,
        }
    }

    /// English line: `Cron::describe()`, `every 30m`, `at 2026-09-16T15:00`.
    pub fn describe(&self) -> String {
        match self {
            When::Cron(c) => c.describe(),
            When::Every(d) => format!("every {}", humantime::format_duration(*d)),
            When::At(t) => format!("at {}", t.format("%Y-%m-%dT%H:%M")),
        }
    }
}

/// A parsed schedule file.
#[derive(Clone, Debug)]
pub struct Spec {
    /// File stem.
    pub name: String,
    /// When it fires.
    pub when: When,
    /// The clause as written: `cron 0 9 * * 1-5` / `every 30m` / `at 2026-09-16T15:00`.
    pub when_text: String,
    /// `quiet: true` — the run's reply is not delivered; only `message_user` reaches the user.
    pub quiet: bool,
    /// `missed: run` — a cron occurrence missed while down fires at boot.
    pub missed_run: bool,
    /// `model:` — model for the run's session.
    pub model: Option<String>,
    /// `enabled:` — default true; false is listed and never fires.
    pub enabled: bool,
    /// Body, trimmed; never empty.
    pub prompt: String,
    /// `until:` — shell command run before each fire; exit 0 ends the loop (33 §0.2, §3.1).
    pub until: Option<String>,
    /// `max_runs:` — fires since the last end row before the loop is capped (33 §3.2).
    pub max_runs: Option<u32>,
    /// `cwd:` — where `until` runs and the session is created; `~` expanded (33 §0.9).
    pub cwd: Option<PathBuf>,
    /// `approvals:` — default true; false maps to the per-session switch on every fire (33 §0.8).
    pub approvals: bool,
    /// `agent:` — a roster agent name; each fire creates a task owned by it
    /// instead of running in the schedule's own session (67 §7.4). Absent =
    /// today's behaviour.
    pub agent: Option<String>,
    /// `project:` — the project label of the task an `agent:` fire creates
    /// (67 §7.1); absent = inferred from `cwd`, else none.
    pub project: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct FrontMatter {
    cron: Option<String>,
    every: Option<String>,
    at: Option<String>,
    quiet: Option<bool>,
    missed: Option<String>,
    model: Option<String>,
    enabled: Option<bool>,
    until: Option<String>,
    max_runs: Option<u32>,
    cwd: Option<String>,
    approvals: Option<bool>,
    agent: Option<String>,
    project: Option<String>,
}

/// Parse one file's text. `Err` is the user-facing reason (`cron: …`,
/// `exactly one of cron / every / at`, `every: below 10s`, `at: in the past`,
/// `missed: expected run|skip`, `empty prompt`; loops (33 §2): `every: 0s needs
/// until or max_runs`, `until: empty`, `max_runs: at least 1`, `cwd: not a
/// directory: …`, `at: a one-shot has no loop to stop`).
pub fn parse(name: &str, text: &str) -> Result<Spec, String> {
    let matter = gray_matter::Matter::<gray_matter::engine::YAML>::new();
    let parsed = matter
        .parse::<FrontMatter>(text)
        .map_err(|e| format!("front matter: {e}"))?;
    let fm = parsed.data.unwrap_or_default();
    let prompt = parsed.content.trim();

    let until = fm.until.map(|u| u.trim().to_string());
    if until.as_deref() == Some("") {
        return Err("until: empty".into());
    }
    if fm.max_runs == Some(0) {
        return Err("max_runs: at least 1".into());
    }
    let has_stop = until.is_some() || fm.max_runs.is_some();
    let cwd = match fm.cwd.as_deref().map(str::trim) {
        None => None,
        Some(c) => {
            let path = match c.strip_prefix('~') {
                Some(rest) if rest.is_empty() || rest.starts_with('/') => dirs::home_dir()
                    .ok_or("cwd: no home directory")?
                    .join(rest.trim_start_matches('/')),
                _ => PathBuf::from(c),
            };
            if !path.is_dir() {
                return Err(format!("cwd: not a directory: {}", path.display()));
            }
            Some(path)
        }
    };

    let (when, when_text) = match (&fm.cron, &fm.every, &fm.at) {
        (Some(c), None, None) => {
            let c = c.trim();
            let cron = croner::Cron::from_str(c).map_err(|e| format!("cron: {e}"))?;
            (When::Cron(Box::new(cron)), format!("cron {c}"))
        }
        (None, Some(e), None) => {
            let e = e.trim();
            let d = humantime::parse_duration(e).map_err(|err| format!("every: {err}"))?;
            if d < MIN_EVERY && !has_stop {
                return Err(if d.is_zero() {
                    "every: 0s needs until or max_runs".into()
                } else {
                    "every: below 10s".into()
                });
            }
            (When::Every(d), format!("every {e}"))
        }
        (None, None, Some(a)) => {
            if has_stop {
                return Err("at: a one-shot has no loop to stop".into());
            }
            let a = a.trim();
            let t = parse_at(a)?;
            if t <= Local::now() {
                return Err("at: in the past".into());
            }
            (When::At(t), format!("at {a}"))
        }
        _ => return Err("exactly one of cron / every / at".into()),
    };

    let missed_run = match fm.missed.as_deref().map(str::trim) {
        None | Some("skip") => false,
        Some("run") => true,
        Some(_) => return Err("missed: expected run|skip".into()),
    };
    if prompt.is_empty() {
        return Err("empty prompt".into());
    }

    Ok(Spec {
        name: name.to_string(),
        when,
        when_text,
        quiet: fm.quiet.unwrap_or(false),
        missed_run,
        model: fm
            .model
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty()),
        enabled: fm.enabled.unwrap_or(true),
        prompt: prompt.to_string(),
        until,
        max_runs: fm.max_runs,
        cwd,
        approvals: fm.approvals.unwrap_or(true),
        agent: fm
            .agent
            .map(|a| a.trim().to_string())
            .filter(|a| !a.is_empty()),
        project: fm
            .project
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty()),
    })
}

/// RFC 3339, else `%Y-%m-%dT%H:%M:%S`, else `%Y-%m-%dT%H:%M` read as local.
fn parse_at(s: &str) -> Result<DateTime<Local>, String> {
    if let Ok(t) = DateTime::parse_from_rfc3339(s) {
        return Ok(t.with_timezone(&Local));
    }
    let naive = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M"))
        .map_err(|_| "at: expected RFC 3339 or YYYY-MM-DDTHH:MM[:SS]".to_string())?;
    naive
        .and_local_timezone(Local)
        .earliest()
        .ok_or_else(|| "at: not a valid local time".to_string())
}

/// `[a-z0-9][a-z0-9-]*`, 1..=40 chars.
pub fn valid_name(name: &str) -> bool {
    let n = name.len();
    (1..=40).contains(&n)
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Every `*.md` in `dir` (non-recursive; names starting with `.` ignored), sorted
/// by name: `(stem, raw text, parse result)`. Missing dir → empty; an unreadable
/// file → `Err` with the io error.
pub fn load_dir(dir: &Path) -> Vec<(String, String, Result<Spec, String>)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<_> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let path = e.path();
            let file = path.file_name()?.to_str()?;
            if file.starts_with('.') || path.extension()? != "md" {
                return None;
            }
            let name = path.file_stem()?.to_str()?.to_string();
            Some(match std::fs::read_to_string(&path) {
                Ok(text) => {
                    let spec = parse(&name, &text);
                    (name, text, spec)
                }
                Err(e) => (name, String::new(), Err(e.to_string())),
            })
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Inputs for [`render`]: what the `schedule` tool received.
#[derive(Clone, Copy, Debug)]
pub struct FileArgs<'a> {
    /// `cron:` expression.
    pub cron: Option<&'a str>,
    /// `every:` interval.
    pub every: Option<&'a str>,
    /// `at:` instant.
    pub at: Option<&'a str>,
    /// `quiet:` — written only when true.
    pub quiet: bool,
    /// `model:` — written only when present.
    pub model: Option<&'a str>,
    /// `until:` — the stop command (33 §2); written only when present.
    pub until: Option<&'a str>,
    /// `max_runs:` — the cap (33 §2); written only when present.
    pub max_runs: Option<u32>,
    /// `cwd:` — where `until` runs and the session starts (33 §2); written only when present.
    pub cwd: Option<&'a str>,
    /// `approvals:` — written only when false (33 §0.8).
    pub approvals: bool,
    /// `enabled:` — written only when false: the draft (33 §0.6).
    pub enabled: bool,
    /// `missed: run` — written only when true.
    pub missed_run: bool,
    /// `agent:` — written only when present (67 §7.4).
    pub agent: Option<&'a str>,
    /// `project:` — written only when present (67 §7.1).
    pub project: Option<&'a str>,
    /// Body.
    pub prompt: &'a str,
}

/// One schedule file as the desktop editor reads and writes it: every front
/// matter key as written (no defaults applied, `cwd` unexpanded) plus the body.
/// Read leniently so a file that fails [`parse`] can still be repaired.
#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ScheduleFile {
    /// File stem.
    pub name: String,
    /// `cron:` expression.
    pub cron: Option<String>,
    /// `every:` interval.
    pub every: Option<String>,
    /// `at:` instant.
    pub at: Option<String>,
    /// `quiet:` (default false).
    pub quiet: bool,
    /// `missed: run` (default skip).
    pub missed_run: bool,
    /// `model:`.
    pub model: Option<String>,
    /// `enabled:` (default true).
    pub enabled: bool,
    /// `until:`.
    pub until: Option<String>,
    /// `max_runs:`.
    pub max_runs: Option<u32>,
    /// `cwd:` as written.
    pub cwd: Option<String>,
    /// `approvals:` (default true).
    pub approvals: bool,
    /// `agent:`.
    pub agent: Option<String>,
    /// `project:`.
    pub project: Option<String>,
    /// Body, trimmed.
    pub prompt: String,
}

impl ScheduleFile {
    /// Front matter + body of `text`; `Err("front matter: …")` only when the YAML itself is broken.
    pub fn read(name: &str, text: &str) -> Result<Self, String> {
        let matter = gray_matter::Matter::<gray_matter::engine::YAML>::new();
        let parsed = matter
            .parse::<FrontMatter>(text)
            .map_err(|e| format!("front matter: {e}"))?;
        let fm = parsed.data.unwrap_or_default();
        Ok(Self {
            name: name.to_string(),
            cron: fm.cron,
            every: fm.every,
            at: fm.at,
            quiet: fm.quiet.unwrap_or(false),
            missed_run: fm.missed.as_deref().map(str::trim) == Some("run"),
            model: fm.model,
            enabled: fm.enabled.unwrap_or(true),
            until: fm.until,
            max_runs: fm.max_runs,
            cwd: fm.cwd,
            approvals: fm.approvals.unwrap_or(true),
            agent: fm.agent,
            project: fm.project,
            prompt: parsed.content.trim().to_string(),
        })
    }

    /// Borrow as [`render`] input; empty strings count as absent.
    pub fn args(&self) -> FileArgs<'_> {
        fn some(s: &Option<String>) -> Option<&str> {
            s.as_deref().map(str::trim).filter(|s| !s.is_empty())
        }
        FileArgs {
            cron: some(&self.cron),
            every: some(&self.every),
            at: some(&self.at),
            quiet: self.quiet,
            model: some(&self.model),
            until: some(&self.until),
            max_runs: self.max_runs,
            cwd: some(&self.cwd),
            approvals: self.approvals,
            enabled: self.enabled,
            missed_run: self.missed_run,
            agent: some(&self.agent),
            project: some(&self.project),
            prompt: &self.prompt,
        }
    }
}

/// YAML double-quoted scalar: `\` and `"` escaped, so `:`, `#`, `$(…)` and a
/// leading `~` survive.
fn yaml_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Render the file text: front matter with only the keys present, a blank
/// line, the prompt, trailing newline.
pub fn render(a: &FileArgs<'_>) -> String {
    let mut s = String::from("---\n");
    if let Some(c) = a.cron {
        s.push_str(&format!("cron: \"{}\"\n", c.trim()));
    }
    if let Some(e) = a.every {
        s.push_str(&format!("every: {}\n", e.trim()));
    }
    if let Some(t) = a.at {
        s.push_str(&format!("at: {}\n", t.trim()));
    }
    if a.quiet {
        s.push_str("quiet: true\n");
    }
    if let Some(m) = a.model {
        s.push_str(&format!("model: {}\n", m.trim()));
    }
    if let Some(u) = a.until {
        s.push_str(&format!("until: {}\n", yaml_str(u.trim())));
    }
    if let Some(n) = a.max_runs {
        s.push_str(&format!("max_runs: {n}\n"));
    }
    if let Some(c) = a.cwd {
        s.push_str(&format!("cwd: {}\n", yaml_str(c.trim())));
    }
    if !a.approvals {
        s.push_str("approvals: false\n");
    }
    if !a.enabled {
        s.push_str("enabled: false\n");
    }
    if a.missed_run {
        s.push_str("missed: run\n");
    }
    if let Some(v) = a.agent {
        s.push_str(&format!("agent: {}\n", v.trim()));
    }
    if let Some(v) = a.project {
        s.push_str(&format!("project: {}\n", yaml_str(v.trim())));
    }
    s.push_str("---\n\n");
    s.push_str(a.prompt.trim());
    s.push('\n');
    s
}

/// Rewrite the `enabled:` line inside the front matter (33 §3.5): replace it
/// when present, else insert `enabled: <v>` right after the opening `---`.
/// Text without front matter gets one. Everything else is byte-for-byte.
pub fn set_enabled(text: &str, enabled: bool) -> String {
    let line = format!("enabled: {enabled}\n");
    if !text.starts_with("---\n") {
        return format!("---\n{line}---\n{text}");
    }
    let mut lines = text.split_inclusive('\n');
    let open = lines.next().unwrap_or_default();
    let mut out = String::with_capacity(text.len() + line.len());
    out.push_str(open);
    let mut replaced = false;
    for l in &mut lines {
        if l.trim_end() == "---" {
            out.push_str(l);
            break;
        }
        if l.starts_with("enabled:") {
            out.push_str(&line);
            replaced = true;
        } else {
            out.push_str(l);
        }
    }
    if !replaced {
        out.insert_str(open.len(), &line);
    }
    out.extend(lines);
    out
}

/// Write `<dir>/<name>.md` atomically (dotfile temp in `dir`, then rename) so the
/// watcher never reads half of it. Creates `dir`. Returns the final path.
pub fn write_atomic(dir: &Path, name: &str, text: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{name}.md.tmp"));
    let path = dir.join(format!("{name}.md"));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Timelike};

    fn md(front: &str, body: &str) -> String {
        format!("---\n{front}\n---\n{body}\n")
    }

    #[test]
    fn exactly_one_of_cron_every_at() {
        assert_eq!(
            parse("x", &md("quiet: true", "p")).unwrap_err(),
            "exactly one of cron / every / at"
        );
        assert_eq!(
            parse("x", &md("cron: \"0 9 * * *\"\nevery: 30m", "p")).unwrap_err(),
            "exactly one of cron / every / at"
        );
        assert!(parse("x", "no front matter at all").is_err());
        assert!(parse("x", &md("cron: \"0 9 * * *\"", "p")).is_ok());
    }

    #[test]
    fn every_bounds() {
        assert_eq!(
            parse("x", &md("every: 5s", "p")).unwrap_err(),
            "every: below 10s"
        );
        let s = parse("x", &md("every: 1h 30m", "p")).unwrap();
        assert!(matches!(s.when, When::Every(d) if d == Duration::from_secs(5400)));
        assert_eq!(s.when_text, "every 1h 30m");
    }

    #[test]
    fn at_forms() {
        assert_eq!(
            parse("x", &md("at: 2020-01-01T10:00", "p")).unwrap_err(),
            "at: in the past"
        );
        assert!(parse("x", &md("at: 2099-01-01T10:00:00+02:00", "p")).is_ok());
        assert!(parse("x", &md("at: 2099-01-01T10:00:00", "p")).is_ok());
        let s = parse("x", &md("at: 2099-01-01T10:00", "p")).unwrap();
        assert!(matches!(s.when, When::At(t) if t.hour() == 10));
        assert_eq!(s.when_text, "at 2099-01-01T10:00");
        assert!(parse("x", &md("at: tomorrow", "p")).is_err());
    }

    #[test]
    fn misc_keys() {
        assert_eq!(
            parse("x", &md("cron: \"0 9 * * *\"\nmissed: maybe", "p")).unwrap_err(),
            "missed: expected run|skip"
        );
        assert_eq!(
            parse("x", &md("cron: \"0 9 * * *\"", "  \n")).unwrap_err(),
            "empty prompt"
        );
        assert!(
            parse("x", &md("cron: \"0 99 * * *\"", "p"))
                .unwrap_err()
                .starts_with("cron: ")
        );
        let s = parse(
            "n",
            &md("cron: \"0 9 * * *\"\nmissed: run\nenabled: false", "  p  "),
        )
        .unwrap();
        assert!(s.missed_run && !s.enabled && !s.quiet);
        assert_eq!((s.name.as_str(), s.prompt.as_str()), ("n", "p"));
    }

    #[test]
    fn cron_next_holds_wall_clock_across_dst() {
        let c = When::Cron(Box::new(croner::Cron::from_str("0 9 * * *").unwrap()));
        // Two days before the 2026-11-01 fall-back; walk four fires across it.
        let mut now = Local.with_ymd_and_hms(2026, 10, 30, 12, 0, 0).unwrap();
        for _ in 0..4 {
            let next = c.next(now, None).unwrap();
            assert!(next > now);
            assert_eq!((next.hour(), next.minute()), (9, 0));
            now = next;
        }
        assert_eq!(
            now.date_naive(),
            chrono::NaiveDate::from_ymd_opt(2026, 11, 3).unwrap()
        );
    }

    #[test]
    fn every_next_and_missed() {
        let now = Local.with_ymd_and_hms(2026, 9, 16, 12, 0, 0).unwrap();
        let e = When::Every(Duration::from_secs(1800));
        assert_eq!(e.next(now, None), Some(now));
        let last = now - chrono::Duration::minutes(10);
        assert_eq!(
            e.next(now, Some(last)),
            Some(last + chrono::Duration::minutes(30))
        );
        assert!(!e.missed(last, now));

        let c = When::Cron(Box::new(croner::Cron::from_str("0 9 * * *").unwrap()));
        let last = Local.with_ymd_and_hms(2026, 9, 15, 9, 0, 0).unwrap();
        assert!(c.missed(last, now)); // 16th 09:00 lies between
        assert!(!c.missed(Local.with_ymd_and_hms(2026, 9, 16, 9, 0, 0).unwrap(), now));

        let at = When::At(now + chrono::Duration::hours(1));
        assert_eq!(at.next(now, None), Some(now + chrono::Duration::hours(1)));
        assert_eq!(at.next(now + chrono::Duration::hours(2), None), None);
    }

    const BASE: FileArgs<'static> = FileArgs {
        cron: None,
        every: None,
        at: None,
        quiet: false,
        model: None,
        until: None,
        max_runs: None,
        cwd: None,
        approvals: true,
        enabled: true,
        missed_run: false,
        agent: None,
        project: None,
        prompt: "p",
    };

    #[test]
    fn schedule_file_round_trip() {
        let f = ScheduleFile {
            name: "x".into(),
            cron: Some("0 9 * * 1-5".into()),
            quiet: true,
            missed_run: true,
            model: Some("m".into()),
            enabled: false,
            cwd: Some("~/src".into()),
            approvals: false,
            agent: Some("alice".into()),
            project: Some("web: v2".into()),
            prompt: "do it".into(),
            ..ScheduleFile::default()
        };
        let text = render(&f.args());
        let back = ScheduleFile::read("x", &text).unwrap();
        assert_eq!(back.cron.as_deref(), Some("0 9 * * 1-5"));
        assert!(back.quiet && back.missed_run && !back.enabled && !back.approvals);
        assert_eq!(back.cwd.as_deref(), Some("~/src"));
        assert_eq!(back.agent.as_deref(), Some("alice"));
        assert_eq!(back.project.as_deref(), Some("web: v2"));
        assert_eq!(back.prompt, "do it");
        // Broken semantics (no clause) still read: the editor can repair it.
        let broken = ScheduleFile::read("y", "---\nquiet: true\n---\n").unwrap();
        assert!(broken.cron.is_none() && broken.quiet && broken.prompt.is_empty());
    }

    #[test]
    fn render_parse_round_trip() {
        let text = render(&FileArgs {
            cron: Some("0 9 * * 1-5"),
            quiet: true,
            model: Some("claude-haiku-4-5"),
            prompt: "Brief me.\n\nTwice.",
            ..BASE
        });
        let s = parse("brief", &text).unwrap();
        assert_eq!(s.when_text, "cron 0 9 * * 1-5");
        assert!(s.quiet && s.enabled && !s.missed_run && s.approvals);
        assert_eq!(s.model.as_deref(), Some("claude-haiku-4-5"));
        assert_eq!(s.prompt, "Brief me.\n\nTwice.");
        assert!(s.until.is_none() && s.max_runs.is_none() && s.cwd.is_none());

        let text = render(&FileArgs {
            every: Some("30m"),
            ..BASE
        });
        assert!(!text.contains("quiet") && !text.contains("model"));
        assert!(!text.contains("approvals") && !text.contains("enabled"));
        let s = parse("q", &text).unwrap();
        assert!(!s.quiet && s.model.is_none());
        assert_eq!(s.when_text, "every 30m");

        let text = render(&FileArgs {
            at: Some("2099-01-01T10:00"),
            ..BASE
        });
        assert_eq!(parse("a", &text).unwrap().when_text, "at 2099-01-01T10:00");
    }

    #[test]
    fn loop_keys_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_str().unwrap();
        let text = render(&FileArgs {
            every: Some("0s"),
            until: Some("test \"$(gh issue view 42 -q .state)\" = CLOSED # done: closed"),
            max_runs: Some(200),
            cwd: Some(cwd),
            approvals: false,
            enabled: false,
            ..BASE
        });
        let s = parse("port", &text).unwrap();
        assert!(matches!(s.when, When::Every(d) if d.is_zero()));
        assert_eq!(
            s.until.as_deref(),
            Some("test \"$(gh issue view 42 -q .state)\" = CLOSED # done: closed")
        );
        assert_eq!(s.max_runs, Some(200));
        assert_eq!(s.cwd.as_deref(), Some(dir.path()));
        assert!(!s.approvals && !s.enabled);
    }

    #[test]
    fn loop_rules() {
        assert_eq!(
            parse("x", &md("every: 0s", "p")).unwrap_err(),
            "every: 0s needs until or max_runs"
        );
        let s = parse("x", &md("every: 0s\nmax_runs: 1", "p")).unwrap();
        assert!(matches!(s.when, When::Every(d) if d.is_zero()));
        assert!(parse("x", &md("every: 5s\nuntil: cargo test", "p")).is_ok());
        assert_eq!(
            parse("x", &md("at: 2099-01-01T10:00\nuntil: ./done.sh", "p")).unwrap_err(),
            "at: a one-shot has no loop to stop"
        );
        assert!(
            parse("x", &md("every: 1h\ncwd: /definitely/not/here", "p"))
                .unwrap_err()
                .starts_with("cwd: not a directory")
        );
        assert_eq!(
            parse("x", &md("every: 1h\nmax_runs: 0", "p")).unwrap_err(),
            "max_runs: at least 1"
        );
        assert_eq!(
            parse("x", &md("every: 1h\nuntil: \"  \"", "p")).unwrap_err(),
            "until: empty"
        );
        if let Some(home) = dirs::home_dir() {
            let s = parse("x", &md("every: 1h\ncwd: \"~\"", "p")).unwrap();
            assert_eq!(s.cwd.as_deref(), Some(home.as_path()));
        }
    }

    #[test]
    fn set_enabled_rewrites_or_inserts() {
        let text = md("every: 1h\nenabled: true\nquiet: true", "p");
        let off = set_enabled(&text, false);
        assert_eq!(off, md("every: 1h\nenabled: false\nquiet: true", "p"));
        assert!(!parse("x", &off).unwrap().enabled);
        assert_eq!(set_enabled(&off, true), text);

        let text = md("every: 1h", "p");
        let off = set_enabled(&text, false);
        assert_eq!(off, md("enabled: false\nevery: 1h", "p"));
        assert!(!parse("x", &off).unwrap().enabled);

        assert_eq!(set_enabled("p\n", false), "---\nenabled: false\n---\np\n");
    }

    #[test]
    fn valid_name_boundaries() {
        assert!(valid_name("a"));
        assert!(valid_name("morning-brief"));
        assert!(valid_name("0abc"));
        assert!(valid_name(&"a".repeat(40)));
        assert!(!valid_name(&"a".repeat(41)));
        assert!(!valid_name(""));
        assert!(!valid_name("-a"));
        assert!(!valid_name("Brief"));
        assert!(!valid_name("a b"));
        assert!(!valid_name("a_b"));
    }

    #[test]
    fn load_dir_filters_and_reports() {
        let dir = tempfile::tempdir().unwrap();
        let good = md("cron: \"0 9 * * *\"", "p");
        std::fs::write(dir.path().join("b.md"), &good).unwrap();
        std::fs::write(dir.path().join("a.md"), md("every: 1h", "p")).unwrap();
        std::fs::write(dir.path().join("broken.md"), md("every: 1s", "p")).unwrap();
        std::fs::write(dir.path().join(".hidden.md"), &good).unwrap();
        std::fs::write(dir.path().join("notes.txt"), &good).unwrap();

        let got = load_dir(dir.path());
        let names: Vec<_> = got.iter().map(|(n, _, _)| n.as_str()).collect();
        assert_eq!(names, ["a", "b", "broken"]);
        assert_eq!(got.iter().filter(|(_, _, r)| r.is_err()).count(), 1);
        assert_eq!(got[2].2.as_ref().unwrap_err(), "every: below 10s");
        assert_eq!(got[1].1, good);

        assert!(load_dir(&dir.path().join("missing")).is_empty());
    }

    #[test]
    fn write_atomic_lands_and_leaves_no_temp() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_atomic(&dir.path().join("schedules"), "x", "hello").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1
        );
    }
}
