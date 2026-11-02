//! theuth's own caps (160 §5.3): roster `Budget::Monthly` from the agent
//! files (month-to-date spend of the desk's roots, live or deleted (H1):
//! native calls plus placed external-harness money) and session caps
//! (`sessions.budget`: the latest capped run per routine). Read-only SQL on
//! the engine's ledger; rows only where a cap exists. USD caps only — the
//! ledger prices in USD.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use chrono::{Datelike, TimeZone};
use serde::Deserialize;
use sqlx::SqlitePool;

use super::BudgetLimit;
use crate::costs::Piece;

const NANOS: f64 = 1_000_000_000.0;
/// `returned ?? estimated`, USD only (`Store::budget_spent`'s measure); a
/// macro so the queries stay `&'static str` (sqlx audits dynamic SQL).
macro_rules! usd {
    () => {
        "COALESCE(SUM(CASE \
         WHEN m.returned_nanos IS NOT NULL THEN IIF(m.returned_currency = 'USD', m.returned_nanos, 0) \
         WHEN m.estimated_currency = 'USD' THEN m.estimated_nanos ELSE 0 END), 0)"
    };
}

/// Every cap row, agents first, then routines and one-off sessions.
/// `reported`: the cost cube's placed meter money (USD).
pub(super) async fn read(
    db: &SqlitePool,
    data_dir: &Path,
    now_ms: u64,
    reported: &[Piece],
) -> anyhow::Result<Vec<BudgetLimit>> {
    let mut out = monthly(db, &data_dir.join("agents"), now_ms, reported).await?;
    out.extend(sessions(db).await?);
    Ok(out)
}

/// Providers with a dispatched call in the last 30 days (shown even when no
/// longer configured, §5.3).
pub(super) async fn recent_providers(
    db: &SqlitePool,
    now_ms: u64,
) -> anyhow::Result<HashSet<String>> {
    let since = rfc3339(now_ms.saturating_sub(30 * 86_400_000));
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT DISTINCT provider FROM model_calls \
         WHERE created_at >= ?1 AND provider IS NOT NULL AND status <> 'not_dispatched'",
    )
    .bind(since)
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(|(p,)| p).collect())
}

fn rfc3339(ms: u64) -> String {
    chrono::Utc
        .timestamp_millis_opt(i64::try_from(ms).unwrap_or(i64::MAX))
        .single()
        .unwrap_or_default()
        .to_rfc3339()
}

/// `(first instant of this UTC month, first of the next)`, ms.
fn month_bounds(now_ms: u64) -> (u64, u64) {
    let now = chrono::Utc
        .timestamp_millis_opt(i64::try_from(now_ms).unwrap_or(0))
        .single()
        .unwrap_or_default();
    let first = chrono::Utc
        .with_ymd_and_hms(now.year(), now.month(), 1, 0, 0, 0)
        .single()
        .unwrap_or_default();
    let (y, m) = if now.month() == 12 {
        (now.year() + 1, 1)
    } else {
        (now.year(), now.month() + 1)
    };
    let next = chrono::Utc
        .with_ymd_and_hms(y, m, 1, 0, 0, 0)
        .single()
        .unwrap_or_default();
    let ms = |t: chrono::DateTime<chrono::Utc>| u64::try_from(t.timestamp_millis()).unwrap_or(0);
    (ms(first), ms(next))
}

/// `budget:` from one agent file's front matter, in USD per month
/// (roster `Budget`'s file form `40 USD/month`; `none` and other currencies → `None`).
fn monthly_usd(text: &str) -> Option<f64> {
    #[derive(Deserialize, Default)]
    struct Fm {
        budget: Option<String>,
    }
    let matter = gray_matter::Matter::<gray_matter::engine::YAML>::new();
    let fm: Fm = matter.parse::<Fm>(text).ok()?.data?;
    let s = fm.budget?;
    let (amount, rest) = s.trim().split_once(char::is_whitespace)?;
    let currency = rest.trim().strip_suffix("/month")?.trim();
    let amount: f64 = amount.parse().ok()?;
    (currency.eq_ignore_ascii_case("USD") && amount.is_finite() && amount > 0.0).then_some(amount)
}

async fn monthly(
    db: &SqlitePool,
    dir: &Path,
    now_ms: u64,
    reported: &[Piece],
) -> anyhow::Result<Vec<BudgetLimit>> {
    let mut caps: BTreeMap<String, f64> = BTreeMap::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let path = e.path();
            if path.extension().is_none_or(|x| x != "md") {
                continue;
            }
            let Some(name) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if let Some(cap) = std::fs::read_to_string(&path)
                .ok()
                .as_deref()
                .and_then(monthly_usd)
            {
                caps.insert(name.to_owned(), cap);
            }
        }
    }
    if caps.is_empty() {
        return Ok(Vec::new());
    }
    let (first, next) = month_bounds(now_ms);
    // The root's desk from its live row, else its tombstone (160 H1: a deleted
    // desk root keeps its spend under the agent).
    let spent: Vec<(String, i64)> = sqlx::query_as(concat!(
        "SELECT COALESCE(s.desk, t.desk) AS d, ",
        usd!(),
        " FROM model_calls m \
         JOIN cost_owners c ON c.id = m.owner_id \
         LEFT JOIN sessions s ON s.id = c.root_id \
         LEFT JOIN cost_root_tombstones t ON t.root_id = c.root_id \
         WHERE m.created_at >= ?1 AND COALESCE(s.desk, t.desk) IS NOT NULL GROUP BY d"
    ))
    .bind(rfc3339(first))
    .fetch_all(db)
    .await?;
    let mut spent: BTreeMap<String, i64> = spent.into_iter().collect();
    let month: Vec<&Piece> = reported.iter().filter(|p| p.at_ms >= first).collect();
    if !month.is_empty() {
        let desks: BTreeMap<String, String> = sqlx::query_as(
            "SELECT id, desk FROM sessions WHERE desk IS NOT NULL \
             UNION ALL SELECT root_id, desk FROM cost_root_tombstones WHERE desk IS NOT NULL",
        )
        .fetch_all(db)
        .await?
        .into_iter()
        .collect();
        for p in month {
            if let Some(desk) = desks.get(&p.root) {
                let nanos = i64::try_from(p.nanos).unwrap_or(i64::MAX);
                let e = spent.entry(desk.clone()).or_default();
                *e = e.saturating_add(nanos);
            }
        }
    }
    Ok(caps
        .into_iter()
        .map(|(name, cap)| BudgetLimit {
            id: format!("agent:{name}"),
            spent_usd: spent.get(&name).copied().unwrap_or(0).max(0) as f64 / NANOS,
            label: name,
            period: "month".into(),
            cap_usd: cap,
            resets_at_ms: Some(next),
        })
        .collect())
}

/// Session caps: a routine's latest capped run (title before ` · ` names an
/// unattended root's routine, spend_report's rule) or a one-off capped session.
/// Spend counts the session's own calls since the cap's `since` — what
/// `Reserved::open` enforces.
async fn sessions(db: &SqlitePool) -> anyhow::Result<Vec<BudgetLimit>> {
    let rows: Vec<(String, Option<String>, Option<i64>, String)> = sqlx::query_as(
        "SELECT id, title, unattended, budget FROM sessions WHERE budget IS NOT NULL",
    )
    .fetch_all(db)
    .await?;
    // key → (id, label, period, cap, since)
    let mut latest: BTreeMap<String, (String, String, &str, u64, String)> = BTreeMap::new();
    for (id, title, unattended, budget) in rows {
        let Ok(b) = serde_json::from_str::<crate::Budget>(&budget) else {
            continue;
        };
        if b.currency != "USD" || b.nanos == 0 {
            continue;
        }
        let title = title.unwrap_or_default();
        let (key, label, period) = if unattended.unwrap_or(0) != 0 {
            let name = title
                .split(" · ")
                .next()
                .unwrap_or(&title)
                .trim()
                .to_owned();
            (format!("routine:{name}"), name, "night")
        } else {
            let label = if title.is_empty() {
                format!("Session {}", id.get(..8).unwrap_or(&id))
            } else {
                title
            };
            (format!("session:{id}"), label, "run")
        };
        if latest
            .get(&key)
            .is_none_or(|(_, _, _, _, since)| *since < b.since)
        {
            latest.insert(key, (id, label, period, b.nanos, b.since));
        }
    }
    let mut out = Vec::with_capacity(latest.len());
    for (key, (id, label, period, cap, since)) in latest {
        let (spent,): (i64,) = sqlx::query_as(concat!(
            "SELECT ",
            usd!(),
            " FROM model_calls m WHERE m.owner_id = ?1 AND m.created_at >= ?2"
        ))
        .bind(&id)
        .bind(&since)
        .fetch_one(db)
        .await?;
        out.push(BudgetLimit {
            id: key,
            label,
            period: period.into(),
            cap_usd: cap as f64 / NANOS,
            spent_usd: spent.max(0) as f64 / NANOS,
            resets_at_ms: None,
        });
    }
    // Routines before one-off sessions.
    out.sort_by_key(|b| !b.id.starts_with("routine:"));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cost::{CallKind, CallStatus, CostObservation, Money};
    use runtime::SessionId;

    #[test]
    fn agent_file_budget_forms() {
        assert_eq!(
            monthly_usd("---\nbudget: 40 USD/month\n---\nQA."),
            Some(40.0)
        );
        assert_eq!(
            monthly_usd("---\nbudget: 12.5 usd/month\ntitle: x\n---\nQA."),
            Some(12.5)
        );
        assert_eq!(monthly_usd("---\nbudget: none\n---\nQA."), None);
        assert_eq!(monthly_usd("---\nbudget: 40 EUR/month\n---\nQA."), None);
        assert_eq!(monthly_usd("---\ntitle: x\n---\nQA."), None);
        assert_eq!(monthly_usd("no front matter"), None);
    }

    #[test]
    fn month_bounds_roll_over_december() {
        // 2026-12-15T10:00:00Z
        let (first, next) = month_bounds(1_797_328_800_000);
        assert_eq!(rfc3339(first), "2026-12-01T00:00:00+00:00");
        assert_eq!(rfc3339(next), "2027-01-01T00:00:00+00:00");
    }

    async fn charge(store: &crate::store::Store, owner: SessionId, usd: f64) {
        let call = store
            .reserve_call(
                owner,
                "req",
                0,
                None,
                CallKind::Round,
                Some("anthropic"),
                Some("m"),
            )
            .await
            .unwrap();
        store.mark_dispatched(&call).await.unwrap();
        store
            .settle_call(
                &call,
                CallStatus::Settled,
                CostObservation {
                    estimated: Some(Money::from_f64(usd, "USD").unwrap()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn monthly_caps_and_latest_routine_run() {
        let dir = crate::TestDir::new("limits-budgets");
        let path = dir.0.join("sessions.sqlite");
        let store = crate::store::Store::open(&path, None).await.unwrap();
        std::fs::create_dir_all(dir.0.join("agents")).unwrap();
        std::fs::write(
            dir.0.join("agents/ivan.md"),
            "---\nbudget: 40 USD/month\n---\nQA.",
        )
        .unwrap();
        std::fs::write(dir.0.join("agents/stas.md"), "---\ntitle: S\n---\nQA.").unwrap();

        // Ivan's desk root and a child: both count toward his month.
        let desk = SessionId::new();
        store
            .create_session(desk, "m", None, None, None)
            .await
            .unwrap();
        store.set_desk(desk, Some("ivan")).await.unwrap();
        store.ensure_owner(desk, None).await.unwrap();
        let child = SessionId::new();
        store
            .create_session(child, "m", Some(desk), None, None)
            .await
            .unwrap();
        store.ensure_owner(child, Some(desk)).await.unwrap();
        charge(&store, desk, 1.25).await;
        charge(&store, child, 0.75).await;

        // Two dreaming nights: only the latest is shown, spend since its stamp.
        let raw = sqlx::SqlitePool::connect(&format!("sqlite:{}", path.display()))
            .await
            .unwrap();
        let mut nights = Vec::new();
        for _ in 0..2 {
            let s = SessionId::new();
            store
                .create_session(s, "m", None, None, None)
                .await
                .unwrap();
            store.ensure_owner(s, None).await.unwrap();
            charge(&store, s, 3.0).await;
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            let since = chrono::Utc::now().to_rfc3339();
            sqlx::query("UPDATE sessions SET title = 'dreaming · dreaming', unattended = 1, budget = ?1 WHERE id = ?2")
                .bind(format!(r#"{{"nanos":25000000000,"currency":"USD","since":"{since}"}}"#))
                .bind(s.to_string())
                .execute(&raw)
                .await
                .unwrap();
            nights.push(s);
        }
        charge(&store, nights[1], 2.5).await;

        let db = super::super::open_db(&path);
        let now = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap();
        let got = read(&db, &dir.0, now, &[]).await.unwrap();
        let rows: Vec<_> = got
            .iter()
            .map(|b| {
                (
                    b.id.as_str(),
                    b.period.as_str(),
                    b.cap_usd,
                    (b.spent_usd * 100.0).round() / 100.0,
                )
            })
            .collect();
        assert_eq!(
            rows,
            [
                ("agent:ivan", "month", 40.0, 2.0),
                ("routine:dreaming", "night", 25.0, 2.5),
            ],
            "stas has no cap; the earlier night's $3 is before its stamp"
        );
        assert_eq!(got[0].resets_at_ms, Some(month_bounds(now).1));
        assert!(
            recent_providers(&db, now)
                .await
                .unwrap()
                .contains("anthropic")
        );

        // Review 160-LIM #6: placed harness money this month counts (last
        // month's does not), and deleting the desk root keeps its spend (H1).
        let piece = |at_ms: u64, usd: u64| Piece {
            meter: "m1".into(),
            k: 0,
            at_ms,
            root: desk.to_string(),
            owner: desk.to_string(),
            via: "omp".into(),
            model: "anthropic/m".into(),
            nanos: usd * 250_000_000,
            exact: true,
        };
        let (first, _) = month_bounds(now);
        let reported = [piece(now, 2), piece(first - 1, 4)];
        store.delete_session(desk).await.unwrap();
        let got = read(&db, &dir.0, now, &reported).await.unwrap();
        assert_eq!(got[0].id, "agent:ivan");
        assert_eq!((got[0].spent_usd * 100.0).round() / 100.0, 2.5, "{got:?}");
    }
}
