//! The deterministic half of Tidy (design doc 136 §4 phase 2): the
//! candidates a night (or `dry_run`) can find by rule, with no model —
//! fresh entries, titles over 120 chars, duplicate id stems modulo a date /
//! `-vN` suffix, archive-by-use (records past their lifetime; ≥ 50 shown and
//! 0 opened non-warnings), the verification sample. Pure over `Meta` + `Uses`.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use knowledge::{Meta, Status, Uses};

use crate::config::DreamConfig;

/// Title cap (136 §4 phase 2).
pub const TITLE_CHARS: usize = 120;
/// Impressions that make a never-opened entry noise (136 §4 phase 2).
pub const NOISE_SHOWN: u64 = 50;
/// Days an entry must have been on the shelf before use counts against it.
pub const NOISE_DAYS: i64 = 14;

/// What the rules found; ids only (the model decides from here).
#[derive(
    Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct Candidates {
    /// `status: fresh`.
    pub fresh: Vec<String>,
    /// Live entries whose title is over 120 chars.
    pub retitle: Vec<String>,
    /// Groups of live ids sharing a stem (date / `-vN` suffix stripped), ≥ 2 each.
    pub duplicate_stems: Vec<Vec<String>>,
    /// Records past `lifetimes.record`, plus ≥ 50 shown / 0 opened entries ≥ 14 days old
    /// that are neither warnings nor decisions (live night 4 archived 5 decisions by use).
    pub archive_by_use: Vec<String>,
    /// Entries whose `verified` is absent or older than `lifetimes.fact`, checkable types first (≤ caps.verifications).
    pub verify_sample: Vec<String>,
}

/// `id` without a trailing `-YYYY-MM-DD`, `-YYYYMMDD` or `-vN` suffix.
pub fn stem(id: &str) -> &str {
    // Byte slicing below: a non-ASCII id (R-32) is its own stem.
    if !id.is_ascii() {
        return id;
    }
    let bytes = id.as_bytes();
    // -YYYY-MM-DD
    if id.len() > 11 {
        let tail = &id[id.len() - 11..];
        let t = tail.as_bytes();
        if t[0] == b'-'
            && t[1..5].iter().all(u8::is_ascii_digit)
            && t[5] == b'-'
            && t[6..8].iter().all(u8::is_ascii_digit)
            && t[8] == b'-'
            && t[9..11].iter().all(u8::is_ascii_digit)
        {
            return &id[..id.len() - 11];
        }
    }
    // -YYYYMMDD
    if id.len() > 9 {
        let t = &bytes[id.len() - 9..];
        if t[0] == b'-' && t[1..].iter().all(u8::is_ascii_digit) {
            return &id[..id.len() - 9];
        }
    }
    // -vN
    if let Some(i) = id.rfind("-v")
        && i > 0
        && id.len() > i + 2
        && id[i + 2..].bytes().all(|b| b.is_ascii_digit())
    {
        return &id[..i];
    }
    id
}

fn age_days(m: &Meta, now: DateTime<Utc>) -> Option<i64> {
    let t = m.timestamp.as_deref()?;
    let t = DateTime::parse_from_rfc3339(t).ok()?.with_timezone(&Utc);
    Some(now.signed_duration_since(t).num_days())
}

fn verified_age_days(m: &Meta, now: DateTime<Utc>) -> Option<i64> {
    let v = m.verified.as_deref()?;
    let d = chrono::NaiveDate::parse_from_str(v, "%Y-%m-%d").ok()?;
    Some((now.date_naive() - d).num_days())
}

/// Does the title or description carry a claim a read tool can check?
fn checkable(m: &Meta) -> bool {
    let t = format!("{} {}", m.title, m.description).to_lowercase();
    t.contains('/')
        || t.contains("fixed in")
        || t.contains("version")
        || t.contains("commit")
        || t.contains(".local")
        || t.contains("port ")
        || t.split_whitespace()
            .any(|w| w.len() >= 7 && w.chars().all(|c| c.is_ascii_hexdigit()))
}

/// The candidates over `metas` with their `uses` (unknown ids count as zeros).
pub fn candidates(
    metas: &[Meta],
    uses: &[Uses],
    cfg: &DreamConfig,
    now: DateTime<Utc>,
) -> Candidates {
    let used: BTreeMap<&str, &Uses> = uses.iter().map(|u| (u.id.as_str(), u)).collect();
    let mut c = Candidates::default();
    let mut stems: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    let mut verify: Vec<(bool, i64, String)> = Vec::new();
    for m in metas {
        if m.status == Status::Fresh {
            c.fresh.push(m.id.clone());
        }
        if !m.status.is_live() {
            continue;
        }
        if m.title.chars().count() > TITLE_CHARS {
            c.retitle.push(m.id.clone());
        }
        stems.entry(stem(&m.id)).or_default().push(m.id.clone());
        let age = age_days(m, now);
        let record_expired =
            m.type_ == "record" && age.is_some_and(|d| d > i64::from(cfg.lifetimes.record));
        // The skill never archives a decision or a warning by use; do not offer them.
        let noise = !matches!(m.type_.as_str(), "warning" | "decision")
            && age.is_some_and(|d| d >= NOISE_DAYS)
            && used
                .get(m.id.as_str())
                .is_some_and(|u| u.shown >= NOISE_SHOWN && u.opened == 0);
        if record_expired || noise {
            c.archive_by_use.push(m.id.clone());
        }
        if matches!(
            m.type_.as_str(),
            "fact"
                | "procedure"
                | "entity"
                | "hosts"
                | "host"
                | "project"
                | "tool"
                | "architecture"
        ) {
            let stale = match verified_age_days(m, now) {
                Some(d) => d > i64::from(cfg.lifetimes.fact),
                None => true,
            };
            if stale {
                let shown = used.get(m.id.as_str()).map_or(0, |u| u.shown);
                // Checkable claims first, then recently hinted (more impressions).
                verify.push((checkable(m), shown as i64, m.id.clone()));
            }
        }
    }
    c.duplicate_stems = stems.into_values().filter(|g| g.len() > 1).collect();
    verify.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| b.1.cmp(&a.1))
            .then_with(|| a.2.cmp(&b.2))
    });
    c.verify_sample = verify
        .into_iter()
        .take(cfg.caps.verifications as usize)
        .map(|(_, _, id)| id)
        .collect();
    c
}

impl Candidates {
    /// Work waiting for a night (136 §2.7 `backlog.untidied`): everything a
    /// rule found that is not merely fresh.
    pub fn untidied(&self) -> u64 {
        (self.retitle.len()
            + self.duplicate_stems.iter().map(Vec::len).sum::<usize>()
            + self.archive_by_use.len()) as u64
    }
}

#[cfg(test)]
pub(crate) fn meta(id: &str, type_: &str, title: &str, days_old: i64, status: Status) -> Meta {
    Meta {
        id: id.into(),
        type_: type_.into(),
        title: title.into(),
        timestamp: Some(
            (Utc::now() - chrono::Duration::days(days_old))
                .format("%Y-%m-%dT%H:%M:%SZ")
                .to_string(),
        ),
        status,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stems_strip_dates_and_versions_only() {
        assert_eq!(
            stem("facts/theuth-dev-machine-state-2026-10-05"),
            "facts/theuth-dev-machine-state"
        );
        assert_eq!(stem("facts/rulings-20261005"), "facts/rulings");
        assert_eq!(stem("procedures/build-v3"), "procedures/build");
        assert_eq!(stem("facts/x-2026-13"), "facts/x-2026-13");
        assert_eq!(stem("facts/v2"), "facts/v2");
        assert_eq!(stem("facts/plain"), "facts/plain");
        // R-32: a multibyte id never panics on the byte slices.
        assert_eq!(stem("facts/café-2026-10-05"), "facts/café-2026-10-05");
        assert_eq!(stem("facts/日本語"), "facts/日本語");
    }

    #[test]
    fn rules_find_each_candidate_kind_and_skip_the_dead() {
        let cfg = DreamConfig::default();
        let long = "x".repeat(121);
        let metas = vec![
            meta("facts/a", "fact", "short", 1, Status::Fresh),
            meta("facts/b", "fact", &long, 40, Status::Tidy),
            meta("facts/c-2026-10-01", "record", "day 1", 20, Status::Tidy),
            meta("facts/c-2026-10-02", "record", "day 2", 3, Status::Tidy),
            meta("facts/noise", "fact", "mcp recon", 30, Status::Tidy),
            meta(
                "facts/warn",
                "warning",
                "never paste keys",
                30,
                Status::Tidy,
            ),
            meta("facts/young-noise", "fact", "new", 2, Status::Tidy),
            meta(
                "decisions/ruling",
                "decision",
                "no pulumi",
                30,
                Status::Tidy,
            ),
            meta("facts/dead", "fact", &long, 40, Status::Archived),
            meta(
                "facts/checkable",
                "fact",
                "fixed in commit abc1234 at /usr/bin",
                40,
                Status::Tidy,
            ),
        ];
        let uses = vec![
            Uses {
                id: "facts/noise".into(),
                shown: 361,
                opened: 0,
                ..Default::default()
            },
            Uses {
                id: "facts/warn".into(),
                shown: 283,
                opened: 0,
                ..Default::default()
            },
            Uses {
                id: "facts/young-noise".into(),
                shown: 90,
                opened: 0,
                ..Default::default()
            },
            Uses {
                id: "decisions/ruling".into(),
                shown: 120,
                opened: 0,
                ..Default::default()
            },
            Uses {
                id: "facts/b".into(),
                shown: 60,
                opened: 2,
                ..Default::default()
            },
        ];
        let c = candidates(&metas, &uses, &cfg, Utc::now());
        assert_eq!(c.fresh, ["facts/a"]);
        assert_eq!(c.retitle, ["facts/b"]);
        assert_eq!(
            c.duplicate_stems,
            vec![vec![
                "facts/c-2026-10-01".to_owned(),
                "facts/c-2026-10-02".to_owned()
            ]]
        );
        assert_eq!(c.archive_by_use, ["facts/c-2026-10-01", "facts/noise"]);
        // Verify sample: every fact without `verified`, checkable first, then most shown.
        assert_eq!(c.verify_sample[0], "facts/checkable");
        assert_eq!(c.verify_sample[1], "facts/noise");
        assert!(
            !c.verify_sample
                .iter()
                .any(|i| i == "facts/dead" || i == "facts/warn")
        );
        assert_eq!(c.untidied(), 1 + 2 + 2);
        let small = DreamConfig {
            caps: crate::config::Caps {
                verifications: 2,
                ..cfg.caps
            },
            ..cfg
        };
        assert_eq!(
            candidates(&metas, &uses, &small, Utc::now())
                .verify_sample
                .len(),
            2
        );
    }
}
