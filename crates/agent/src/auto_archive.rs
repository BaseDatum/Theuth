//! Auto-archive of inactive chats (`agent.auto_archive`): a root chat whose
//! whole tree has had no event for `after` is archived the way the sidebar's
//! Archive action does it ([`crate::ArchiveSession`]), so it can be unarchived.
//!
//! This module holds the config, the duration parsing and the pure eligibility
//! rule; the hourly sweep that applies them lives on the supervisor.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use chrono::{DateTime, Utc};
use runtime::SessionId;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::SessionSummary;

/// The shortest `after` accepted: one hour (Settings offers hours and days).
pub const MIN_AFTER: Duration = Duration::from_secs(3600);
/// `after` when unset: 14 days.
pub const DEFAULT_AFTER: Duration = Duration::from_secs(14 * 86_400);

/// `agent.auto_archive`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AutoArchiveConfig {
    /// Archive inactive chats at all. Default on.
    pub enabled: bool,
    /// How long a chat's tree must have had no event before it is archived:
    /// a humantime string such as `14d` or `36h`, at least `1h`. Default `14d`.
    #[serde(serialize_with = "ser_after", deserialize_with = "de_after")]
    pub after: Duration,
}

impl Default for AutoArchiveConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            after: DEFAULT_AFTER,
        }
    }
}

/// Parse `agent.auto_archive.after`: `14d`, `36h`, `2w`, `90m`, … (humantime),
/// at least [`MIN_AFTER`].
pub fn parse_after(s: &str) -> Result<Duration, String> {
    let d = humantime::parse_duration(s.trim()).map_err(|e| format!("duration `{s}`: {e}"))?;
    check_after(d)
}

/// `d` when it is at least [`MIN_AFTER`].
pub fn check_after(d: Duration) -> Result<Duration, String> {
    if d < MIN_AFTER {
        return Err("auto-archive needs at least 1 hour".into());
    }
    Ok(d)
}

/// `after` as config.yaml writes it: `14d` for whole days, `36h` for whole
/// hours, humantime otherwise.
pub fn format_after(d: Duration) -> String {
    let s = d.as_secs();
    if s > 0 && s % 86_400 == 0 && d.subsec_nanos() == 0 {
        format!("{}d", s / 86_400)
    } else if s > 0 && s % 3600 == 0 && d.subsec_nanos() == 0 {
        format!("{}h", s / 3600)
    } else {
        humantime::format_duration(d).to_string().replace(' ', "")
    }
}

fn ser_after<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&format_after(*d))
}

fn de_after<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
    let s = String::deserialize(d)?;
    parse_after(&s).map_err(serde::de::Error::custom)
}

/// One root chat as the sweep sees it: the root's own flags plus what its
/// whole tree (the root and every descendant) says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// The root session.
    pub root: SessionId,
    /// The latest `updated_at` (the sidebar's activity order) across the tree.
    pub last_activity: DateTime<Utc>,
    /// The root is an agent's desk.
    pub desk: bool,
    /// The root is pinned.
    pub pinned: bool,
    /// The root (so the tree) is archived already.
    pub archived: bool,
    /// This engine owns the root; another engine sweeps its own.
    pub here: bool,
    /// Some member runs a turn, waits on an approval or a reply, has a queued
    /// spawn, or holds a live job, PTY or reminder.
    pub busy: bool,
}

/// How long `c` has been inactive when the sweep should archive it now;
/// `None` when it stays.
pub fn eligible(cfg: &AutoArchiveConfig, c: &Candidate, now: DateTime<Utc>) -> Option<Duration> {
    if !cfg.enabled || c.archived || c.desk || c.pinned || !c.here || c.busy {
        return None;
    }
    let age = (now - c.last_activity).to_std().ok()?;
    (age >= cfg.after).then_some(age)
}

/// Every member of `rows` (every session, children included) mapped to its
/// root: a root to itself, a child, grandchild, … to the root up its `parent`
/// chain. A child whose root is not in `rows` is absent; so is a cycle.
pub fn root_of_each(rows: &[SessionSummary]) -> HashMap<SessionId, SessionId> {
    let parent: HashMap<SessionId, Option<SessionId>> =
        rows.iter().map(|r| (r.id, r.parent)).collect();
    let root_of = |mut id: SessionId| -> Option<SessionId> {
        let mut seen = HashSet::new();
        loop {
            if !seen.insert(id) {
                return None;
            }
            match parent.get(&id)? {
                None => return Some(id),
                Some(p) => id = *p,
            }
        }
    };
    rows.iter()
        .filter_map(|r| Some((r.id, root_of(r.id)?)))
        .collect()
}

/// Group `rows` (every session, children included, as `list_sessions` reads
/// them) into one [`Candidate`] per root. `busy` says whether one session is
/// busy. A child whose root is not in `rows` is skipped; so is a cycle.
pub fn candidates(rows: &[SessionSummary], busy: impl Fn(SessionId) -> bool) -> Vec<Candidate> {
    let root_of = root_of_each(rows);
    let mut out: HashMap<SessionId, Candidate> = rows
        .iter()
        .filter(|r| r.parent.is_none())
        .map(|r| {
            (
                r.id,
                Candidate {
                    root: r.id,
                    last_activity: r.updated_at,
                    desk: r.desk.is_some(),
                    pinned: r.pinned,
                    archived: r.archived,
                    here: r.here,
                    busy: busy(r.id),
                },
            )
        })
        .collect();
    for r in rows.iter().filter(|r| r.parent.is_some()) {
        let Some(c) = root_of.get(&r.id).and_then(|root| out.get_mut(root)) else {
            continue;
        };
        c.last_activity = c.last_activity.max(r.updated_at);
        c.busy |= busy(r.id);
    }
    let mut out: Vec<Candidate> = out.into_values().collect();
    out.sort_by_key(|c| c.last_activity);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> AutoArchiveConfig {
        AutoArchiveConfig::default()
    }

    fn now() -> DateTime<Utc> {
        "2026-11-24T12:00:00Z".parse().unwrap()
    }

    fn idle(days: i64) -> Candidate {
        Candidate {
            root: SessionId::new(),
            last_activity: now() - chrono::Duration::days(days),
            desk: false,
            pinned: false,
            archived: false,
            here: true,
            busy: false,
        }
    }

    #[test]
    fn defaults_are_on_after_fourteen_days() {
        let c = cfg();
        assert!(c.enabled);
        assert_eq!(c.after, Duration::from_secs(14 * 86_400));
        let parsed: AutoArchiveConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(parsed, c);
    }

    #[test]
    fn parses_days_and_hours() {
        assert_eq!(parse_after("14d").unwrap(), Duration::from_secs(14 * 86_400));
        assert_eq!(parse_after("36h").unwrap(), Duration::from_secs(36 * 3600));
        assert_eq!(parse_after(" 1h ").unwrap(), MIN_AFTER);
        assert!(parse_after("59m").is_err());
        assert!(parse_after("soon").is_err());
        assert!(parse_after("").is_err());
        let c: AutoArchiveConfig =
            serde_json::from_str(r#"{"enabled": false, "after": "36h"}"#).unwrap();
        assert_eq!((c.enabled, c.after), (false, Duration::from_secs(36 * 3600)));
        assert!(serde_json::from_str::<AutoArchiveConfig>(r#"{"after": "10m"}"#).is_err());
    }

    #[test]
    fn formats_days_then_hours() {
        assert_eq!(format_after(Duration::from_secs(14 * 86_400)), "14d");
        assert_eq!(format_after(Duration::from_secs(36 * 3600)), "36h");
        assert_eq!(format_after(Duration::from_secs(48 * 3600)), "2d");
        assert_eq!(format_after(Duration::from_secs(5400)), "1h30m");
        for d in ["14d", "36h", "1h30m"] {
            assert_eq!(format_after(parse_after(d).unwrap()), d);
        }
    }

    #[test]
    fn enabled_switch_gates_everything() {
        let c = idle(30);
        assert_eq!(eligible(&cfg(), &c, now()), Some(Duration::from_secs(30 * 86_400)));
        let off = AutoArchiveConfig {
            enabled: false,
            ..cfg()
        };
        assert_eq!(eligible(&off, &c, now()), None);
    }

    #[test]
    fn threshold_is_inclusive() {
        let at = idle(14);
        assert!(eligible(&cfg(), &at, now()).is_some());
        let just_under = Candidate {
            last_activity: at.last_activity + chrono::Duration::seconds(1),
            ..at.clone()
        };
        assert_eq!(eligible(&cfg(), &just_under, now()), None);
        // Activity after `now` (a skewed clock) is never stale.
        let future = Candidate {
            last_activity: now() + chrono::Duration::hours(1),
            ..at
        };
        assert_eq!(eligible(&cfg(), &future, now()), None);
        let hours = AutoArchiveConfig {
            after: Duration::from_secs(36 * 3600),
            ..cfg()
        };
        assert!(eligible(&hours, &idle(2), now()).is_some());
        assert!(eligible(&hours, &idle(1), now()).is_none());
    }

    #[test]
    fn desks_pins_running_remote_and_archived_stay() {
        let c = idle(30);
        for kept in [
            Candidate { desk: true, ..c.clone() },
            Candidate { pinned: true, ..c.clone() },
            Candidate { busy: true, ..c.clone() },
            Candidate { here: false, ..c.clone() },
            Candidate { archived: true, ..c.clone() },
        ] {
            assert_eq!(eligible(&cfg(), &kept, now()), None, "{kept:?}");
        }
    }

    fn row(id: SessionId, parent: Option<SessionId>, days: i64) -> SessionSummary {
        let mut r: SessionSummary = serde_json::from_value(serde_json::json!({
            "id": id,
            "created_at": now() - chrono::Duration::days(60),
            "updated_at": now() - chrono::Duration::days(days),
            "title": "t",
            "model": "m",
        }))
        .unwrap();
        r.parent = parent;
        r
    }

    #[test]
    fn a_tree_is_as_recent_and_busy_as_its_most_active_member() {
        let (root, child, grandchild, other) =
            (SessionId::new(), SessionId::new(), SessionId::new(), SessionId::new());
        let orphan = SessionId::new();
        let rows = vec![
            row(root, None, 30),
            row(child, Some(root), 20),
            row(grandchild, Some(child), 3),
            row(other, None, 40),
            row(orphan, Some(SessionId::new()), 1),
        ];
        let got = candidates(&rows, |id| id == grandchild);
        assert_eq!(got.len(), 2, "one per root; the orphan has none");
        let tree = got.iter().find(|c| c.root == root).unwrap();
        assert_eq!(tree.last_activity, now() - chrono::Duration::days(3));
        assert!(tree.busy);
        assert_eq!(eligible(&cfg(), tree, now()), None);
        let lone = got.iter().find(|c| c.root == other).unwrap();
        assert!(!lone.busy);
        assert!(eligible(&cfg(), lone, now()).is_some());
    }
}
