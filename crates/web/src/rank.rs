//! Post-merge fixes to the ranked list (design doc 16 §4.6; measured
//! 2026-09-27): the `site` and `recent` arguments apply to the rows the
//! agent sees, not only to the read set; off-topic Wikipedia panels, mirrors
//! of a repository the list already has, and stale rows under `recent` are
//! dropped; forks and SEO version aggregators sink behind the canonical host.
//! Every drop is explained in the returned notes.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};

use crate::aggregate::RankedResult;
use crate::date;
use crate::provider::Query;

// ponytail: static host lists; a config knob if these ever need tuning per install.
/// Hosts that republish GitHub repositories under the same `/owner/repo` path.
const MIRROR_HOSTS: &[&str] = &[
    "deepwiki.com",
    "hysenlabs.com",
    "gitmemory.com",
    "githubmemory.com",
    "giters.com",
    "github-wiki-see.page",
];

/// Version trackers and catalogue sites that outrank a project's own
/// release page and are usually stale.
const LOW_QUALITY_HOSTS: &[&str] = &[
    "what-version.com",
    "versionlog.com",
    "newreleases.io",
    "libhunt.com",
    "awesomeopensource.com",
    "allshadcn.com",
    "sourceforge.net",
];

/// Hostname without a leading `www.`; `None` when the URL does not parse.
fn host(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    let h = u.host_str()?.to_ascii_lowercase();
    Some(h.strip_prefix("www.").unwrap_or(&h).to_owned())
}

/// `host` is `site` or a subdomain of it.
fn on_site(host: &str, site: &str) -> bool {
    let site = site.trim().trim_start_matches("www.").to_ascii_lowercase();
    host == site || host.ends_with(&format!(".{site}"))
}

fn in_list(host: &str, list: &[&str]) -> bool {
    list.iter().any(|h| on_site(host, h))
}

/// `(owner, repo)` of a `/owner/repo[/…]` path, lowercased.
fn repo_path(url: &str) -> Option<(String, String)> {
    let u = url::Url::parse(url).ok()?;
    let mut segs = u.path_segments()?.filter(|s| !s.is_empty());
    let owner = segs.next()?.to_ascii_lowercase();
    let repo = segs.next()?.trim_end_matches(".git").to_ascii_lowercase();
    Some((owner, repo))
}

fn shares_token(text: &str, terms: &[String]) -> bool {
    let lower = text.to_lowercase();
    terms.iter().any(|t| lower.contains(t.as_str()))
}

/// Apply the fixes; returns the renumbered list and one note per kind of
/// drop or adjustment ("3 off-site rows dropped (site=docs.rs)").
#[must_use]
pub fn refine(
    results: Vec<RankedResult>,
    q: &Query,
    terms: &[String],
    now: DateTime<Utc>,
) -> (Vec<RankedResult>, Vec<String>) {
    let mut notes = Vec::new();
    let mut rows: Vec<(RankedResult, String)> = results
        .into_iter()
        .map(|r| {
            let h = host(&r.url).unwrap_or_default();
            (r, h)
        })
        .collect();

    if let Some(site) = &q.site {
        let before = rows.len();
        rows.retain(|(_, h)| on_site(h, site));
        let dropped = before - rows.len();
        if dropped > 0 {
            notes.push(format!("{dropped} off-site rows dropped (site={site})"));
        }
    }

    let before = rows.len();
    rows.retain(|(r, h)| !(h.ends_with("wikipedia.org") && !shares_token(&r.title, terms)));
    let dropped = before - rows.len();
    if dropped > 0 {
        notes.push(format!("{dropped} unrelated Wikipedia rows dropped"));
    }

    let canonical_repos: HashSet<(String, String)> = rows
        .iter()
        .filter(|(_, h)| h == "github.com")
        .filter_map(|(r, _)| repo_path(&r.url))
        .collect();
    let before = rows.len();
    rows.retain(|(r, h)| {
        !(in_list(h, MIRROR_HOSTS)
            && repo_path(&r.url).is_some_and(|p| canonical_repos.contains(&p)))
    });
    let dropped = before - rows.len();
    if dropped > 0 {
        notes.push(format!(
            "{dropped} mirror rows dropped (canonical repository present)"
        ));
    }

    if let Some(recent) = q.recent {
        let before = rows.len();
        rows.retain(|(r, _)| {
            r.published
                .as_deref()
                .is_none_or(|p| date::within(p, recent, now))
        });
        let dropped = before - rows.len();
        if dropped > 0 {
            notes.push(format!(
                "{dropped} rows older than recent={} dropped",
                crate::aggregate::recency_name(recent)
            ));
        }
        let undated = rows.iter().filter(|(r, _)| r.published.is_none()).count();
        if undated > 0 {
            notes.push(format!(
                "dates unavailable for {undated} rows (listed after dated ones)"
            ));
        }
    }

    // Canonical repo per name: the owner the query names, else the best rank.
    let mut canonical_owner: HashMap<String, String> = HashMap::new();
    for (r, h) in &rows {
        if h != "github.com" {
            continue;
        }
        let Some((owner, repo)) = repo_path(&r.url) else {
            continue;
        };
        let named = shares_token(&owner, terms) || q.text.to_lowercase().contains(&owner);
        match canonical_owner.get(&repo) {
            None => {
                canonical_owner.insert(repo, owner);
            }
            Some(cur) if named && !shares_token(cur, terms) => {
                canonical_owner.insert(repo, owner);
            }
            Some(_) => {}
        }
    }
    let mut forks = 0;
    let mut aggregators = 0;
    let tier = |r: &RankedResult, h: &str| -> u8 {
        if in_list(h, LOW_QUALITY_HOSTS) {
            return 2;
        }
        if h == "github.com" {
            if let Some((owner, repo)) = repo_path(&r.url) {
                if canonical_owner.get(&repo).is_some_and(|c| *c != owner) {
                    return 1;
                }
            }
        }
        0
    };
    let mut keyed: Vec<(u8, bool, RankedResult)> = rows
        .into_iter()
        .map(|(r, h)| {
            let t = tier(&r, &h);
            match t {
                1 => forks += 1,
                2 => aggregators += 1,
                _ => {}
            }
            let undated = q.recent.is_some() && r.published.is_none();
            (t, undated, r)
        })
        .collect();
    keyed.sort_by_key(|(t, undated, _)| (*t, *undated));
    if forks > 0 {
        notes.push(format!(
            "{forks} fork rows listed after the canonical repository"
        ));
    }
    if aggregators > 0 {
        notes.push(format!("{aggregators} version-aggregator rows listed last"));
    }
    let results = keyed
        .into_iter()
        .enumerate()
        .map(|(i, (_, _, mut r))| {
            r.rank = u32::try_from(i + 1).unwrap_or(u32::MAX);
            r
        })
        .collect();
    (results, notes)
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::provider::{Category, Recency};

    fn row(rank: u32, title: &str, url: &str, published: Option<&str>) -> RankedResult {
        RankedResult {
            rank,
            title: title.into(),
            url: url.into(),
            snippet: String::new(),
            providers: vec!["a".into()],
            published: published.map(str::to_owned),
        }
    }

    fn query(text: &str, site: Option<&str>, recent: Option<Recency>) -> Query {
        Query {
            text: text.into(),
            category: Category::General,
            limit: 8,
            site: site.map(str::to_owned),
            recent,
        }
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 27, 12, 0, 0).unwrap()
    }

    fn urls(rows: &[RankedResult]) -> Vec<&str> {
        rows.iter().map(|r| r.url.as_str()).collect()
    }

    #[test]
    fn site_filter_drops_off_site_rows_and_says_so() {
        let q = query("PrintToPdf", Some("learn.microsoft.com"), None);
        let terms = crate::excerpt::tokens(&q.text);
        let rows = vec![
            row(
                1,
                "PDF - Wikipedia",
                "https://en.wikipedia.org/wiki/PDF",
                None,
            ),
            row(2, "PrintToPdf", "https://learn.microsoft.com/en-us/x", None),
            row(
                3,
                "PrintToPdf sub",
                "https://docs.learn.microsoft.com/y",
                None,
            ),
        ];
        let (out, notes) = refine(rows, &q, &terms, now());
        assert_eq!(
            urls(&out),
            [
                "https://learn.microsoft.com/en-us/x",
                "https://docs.learn.microsoft.com/y"
            ]
        );
        assert_eq!(out[0].rank, 1);
        assert_eq!(out[1].rank, 2);
        assert_eq!(
            notes,
            ["1 off-site rows dropped (site=learn.microsoft.com)"]
        );
    }

    #[test]
    fn unrelated_wikipedia_panel_is_dropped() {
        let q = query("bun version", None, None);
        let terms = crate::excerpt::tokens(&q.text);
        let rows = vec![
            row(
                1,
                "Android 17",
                "https://en.wikipedia.org/wiki/Android_17",
                None,
            ),
            row(
                2,
                "Bun (software)",
                "https://en.wikipedia.org/wiki/Bun_(software)",
                None,
            ),
            row(
                3,
                "Bun releases",
                "https://github.com/oven-sh/bun/releases",
                None,
            ),
        ];
        let (out, notes) = refine(rows, &q, &terms, now());
        assert_eq!(
            urls(&out),
            [
                "https://en.wikipedia.org/wiki/Bun_(software)",
                "https://github.com/oven-sh/bun/releases"
            ]
        );
        assert_eq!(notes, ["1 unrelated Wikipedia rows dropped"]);
    }

    #[test]
    fn mirrors_dropped_and_forks_sink_behind_canonical() {
        let q = query("shadcn-labs pdfcn", None, None);
        let terms = crate::excerpt::tokens(&q.text);
        let rows = vec![
            row(
                1,
                "pdfcn fork",
                "https://github.com/kalebnation/pdfcn",
                None,
            ),
            row(
                2,
                "pdfcn deepwiki",
                "https://deepwiki.com/shadcn-labs/pdfcn",
                None,
            ),
            row(3, "pdfcn", "https://github.com/shadcn-labs/pdfcn", None),
            row(4, "pdfcn all", "https://allshadcn.com/pdfcn", None),
            row(5, "pdfcn.dev", "https://pdfcn.dev/", None),
        ];
        let (out, notes) = refine(rows, &q, &terms, now());
        assert_eq!(
            urls(&out),
            [
                "https://github.com/shadcn-labs/pdfcn",
                "https://pdfcn.dev/",
                "https://github.com/kalebnation/pdfcn",
                "https://allshadcn.com/pdfcn",
            ]
        );
        assert!(
            notes.iter().any(|n| n.starts_with("1 mirror rows dropped")),
            "{notes:?}"
        );
        assert!(
            notes.iter().any(|n| n.starts_with("1 fork rows")),
            "{notes:?}"
        );
        assert!(
            notes.iter().any(|n| n.starts_with("1 version-aggregator")),
            "{notes:?}"
        );
    }

    #[test]
    fn fork_without_named_owner_keeps_best_rank_as_canonical() {
        let q = query("pdfcn", None, None);
        let terms = crate::excerpt::tokens(&q.text);
        let rows = vec![
            row(1, "pdfcn", "https://github.com/shadcn-labs/pdfcn", None),
            row(
                2,
                "pdfcn fork",
                "https://github.com/kalebnation/pdfcn",
                None,
            ),
        ];
        let (out, _) = refine(rows, &q, &terms, now());
        assert_eq!(
            urls(&out),
            [
                "https://github.com/shadcn-labs/pdfcn",
                "https://github.com/kalebnation/pdfcn"
            ]
        );
    }

    #[test]
    fn recent_drops_stale_and_lists_undated_last() {
        let q = query("iroh release notes", None, Some(Recency::Month));
        let terms = crate::excerpt::tokens(&q.text);
        let rows = vec![
            row(
                1,
                "iroh 0.1",
                "https://a.example/old",
                Some("2024-01-01T00:00:00Z"),
            ),
            row(2, "iroh notes", "https://b.example/undated", None),
            row(
                3,
                "iroh 0.9",
                "https://c.example/new",
                Some("2026-09-20T00:00:00Z"),
            ),
        ];
        let (out, notes) = refine(rows, &q, &terms, now());
        assert_eq!(
            urls(&out),
            ["https://c.example/new", "https://b.example/undated"]
        );
        assert_eq!(
            notes,
            [
                "1 rows older than recent=month dropped",
                "dates unavailable for 1 rows (listed after dated ones)"
            ]
        );
    }

    #[test]
    fn version_aggregators_sink_behind_canonical_host() {
        let q = query("bun version", None, None);
        let terms = crate::excerpt::tokens(&q.text);
        let rows = vec![
            row(1, "Bun version", "https://what-version.com/bun", None),
            row(
                2,
                "Releases",
                "https://github.com/oven-sh/bun/releases",
                None,
            ),
        ];
        let (out, _) = refine(rows, &q, &terms, now());
        assert_eq!(
            urls(&out),
            [
                "https://github.com/oven-sh/bun/releases",
                "https://what-version.com/bun"
            ]
        );
    }
}
