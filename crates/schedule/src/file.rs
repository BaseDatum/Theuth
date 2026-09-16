//! One schedule file: front matter says *when*, the body is the prompt (34 §2.2, §3).
//! Parsing, `next`/`missed` arithmetic, directory listing, and the rendering
//! plus atomic write the `schedule` tool uses. No actor, no store.

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use chrono::{DateTime, Local, NaiveDateTime};
use serde::Deserialize;

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
}

/// Parse one file's text. `Err` is the user-facing reason (`cron: …`,
/// `exactly one of cron / every / at`, `every: below 10s`, `at: in the past`,
/// `missed: expected run|skip`, `empty prompt`).
pub fn parse(name: &str, text: &str) -> Result<Spec, String> {
    let matter = gray_matter::Matter::<gray_matter::engine::YAML>::new();
    let parsed = matter
        .parse::<FrontMatter>(text)
        .map_err(|e| format!("front matter: {e}"))?;
    let fm = parsed.data.unwrap_or_default();
    let prompt = parsed.content.trim();

    let (when, when_text) = match (&fm.cron, &fm.every, &fm.at) {
        (Some(c), None, None) => {
            let c = c.trim();
            let cron = croner::Cron::from_str(c).map_err(|e| format!("cron: {e}"))?;
            (When::Cron(Box::new(cron)), format!("cron {c}"))
        }
        (None, Some(e), None) => {
            let e = e.trim();
            let d = humantime::parse_duration(e).map_err(|err| format!("every: {err}"))?;
            if d < MIN_EVERY {
                return Err("every: below 10s".into());
            }
            (When::Every(d), format!("every {e}"))
        }
        (None, None, Some(a)) => {
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
    /// Body.
    pub prompt: &'a str,
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
    s.push_str("---\n\n");
    s.push_str(a.prompt.trim());
    s.push('\n');
    s
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

    #[test]
    fn render_parse_round_trip() {
        let text = render(&FileArgs {
            cron: Some("0 9 * * 1-5"),
            every: None,
            at: None,
            quiet: true,
            model: Some("claude-haiku-4-5"),
            prompt: "Brief me.\n\nTwice.",
        });
        let s = parse("brief", &text).unwrap();
        assert_eq!(s.when_text, "cron 0 9 * * 1-5");
        assert!(s.quiet && s.enabled && !s.missed_run);
        assert_eq!(s.model.as_deref(), Some("claude-haiku-4-5"));
        assert_eq!(s.prompt, "Brief me.\n\nTwice.");

        let text = render(&FileArgs {
            cron: None,
            every: Some("30m"),
            at: None,
            quiet: false,
            model: None,
            prompt: "p",
        });
        assert!(!text.contains("quiet") && !text.contains("model"));
        let s = parse("q", &text).unwrap();
        assert!(!s.quiet && s.model.is_none());
        assert_eq!(s.when_text, "every 30m");

        let text = render(&FileArgs {
            cron: None,
            every: None,
            at: Some("2099-01-01T10:00"),
            quiet: false,
            model: None,
            prompt: "p",
        });
        assert_eq!(parse("a", &text).unwrap().when_text, "at 2099-01-01T10:00");
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
