//! The autonomy ladder (126 §9) and the §7.4 numbers it is computed from.
//! Pure functions over [`Scored`] rows — the class's prequential ledger
//! (`predictions` joined to `labels`) for the last [`WINDOW_DAYS`] — plus the
//! one query that loads them. Promotion is evaluated after every label, one
//! rung at a time; demotion is immediate on an override (the actor calls
//! [`demote`] on the event). Hard caps (destructive ≤ L1 always, mutating ≤ L1
//! without the per-class opt-in, `read_only` cap from config — 2 in v1,
//! Revision 1 ruling 4) are [`hard_cap`].

use std::collections::HashMap;

use chrono::{DateTime, Datelike, Duration, Utc};
use sqlx::Row as _;

use crate::config::Caps;
use crate::store::{BASELINE, Store};
use crate::types::{CurvePoint, Kpis, LabelSource, Outcome};

/// Prequential rows older than this do not count (§9).
pub const WINDOW_DAYS: i64 = 30;
/// L0 → L1: at least this many labelled rows …
pub const L1_N: usize = 20;
/// … with this accuracy …
pub const L1_ACC: f32 = 0.90;
/// … and at most this ECE (L1 and L2).
pub const MAX_ECE: f32 = 0.05;
/// L1 → L2: at least this many labelled rows …
pub const L2_N: usize = 50;
/// … with this accuracy, and zero overrides in the last [`RECENT`].
pub const L2_ACC: f32 = 0.97;
/// L2 → L3: this long at L2 with no strike.
pub const L3_DAYS: i64 = 30;
/// "the last 20": the demotion window at L1 and the override window at L2.
pub const RECENT: usize = 20;
/// L1 → L0 when this many of the last [`RECENT`] suggestions were wrong.
pub const L1_STRIKES: usize = 3;
/// ECE bins (§7.4): equal width over `p_top`.
pub const BINS: usize = 10;
/// coverage@ε: ε = 2 % (§7.4).
pub const COVERAGE_EPS: f32 = 0.02;
/// Interruption cost per avoided card, seconds (§7.4 "minutes saved").
pub const INTERRUPTION_S: f32 = 30.0;

/// One prequential row with what its labels say, in time order.
#[derive(Clone, Debug, PartialEq)]
pub struct Scored {
    /// When the prediction was written.
    pub at: DateTime<Utc>,
    /// Predicted option label.
    pub choice: String,
    /// Every option incl. `none`.
    pub probs: Vec<(String, f32)>,
    /// Top-1 probability.
    pub p_top: f32,
    /// The owner's answer (override wins over direct), when labelled.
    pub label: Option<String>,
    /// The double answered the card.
    pub acted: bool,
    /// The card went through an L2 grace.
    pub graced: bool,
    /// The owner took over / reversed it (`override` label).
    pub overridden: bool,
    /// §9: an override or a hindsight negative with weight ≥ 0.5 on an acted row.
    pub strike: bool,
}

impl Scored {
    /// Labelled and the label matches.
    pub fn correct(&self) -> Option<bool> {
        self.label.as_deref().map(|l| l == self.choice)
    }
}

/// The ladder's verdict on one card: which rung behaviour applies (§9 caps:
/// `none` winning or the gate's `Escalate` is L0 behaviour whatever the rung).
pub fn effective_rung(rung: u8, escalated: bool) -> u8 {
    if escalated { 0 } else { rung }
}

/// The hard cap for a class key and why, when it is below the configured
/// `read_only` cap. The risk is the key's third segment for `approval:*`
/// keys; every other kind takes `read_only`.
pub fn hard_cap(caps: &Caps, key: &str, mutating_opt_in: bool) -> (u8, Option<String>) {
    let risk = key
        .strip_prefix("approval:")
        .and_then(|rest| rest.split(':').nth(1))
        .and_then(|r| r.parse::<tools::Risk>().ok());
    match risk {
        Some(tools::Risk::Destructive) => (
            caps.for_risk(risk),
            Some("destructive — never past L1".to_owned()),
        ),
        Some(tools::Risk::Mutating) if !mutating_opt_in => (
            caps.mutating.min(1),
            Some("mutating — opt in to allow L2".to_owned()),
        ),
        Some(tools::Risk::Mutating) => (caps.read_only, None),
        _ => (caps.read_only, None),
    }
}

/// One rung down, at once (any override at L2/L3, §9).
pub fn demote(rung: u8) -> u8 {
    rung.saturating_sub(1)
}

/// The rung after one evaluation over `rows` (the class's last-30-days
/// ledger, time order): at most one step up or down, never above `cap`.
/// `l2_since` is when the class last reached L2 (for the L3 clock).
pub fn next_rung(
    current: u8,
    cap: u8,
    rows: &[Scored],
    l2_since: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> u8 {
    let labelled: Vec<&Scored> = rows.iter().filter(|r| r.label.is_some()).collect();
    let recent = &labelled[labelled.len().saturating_sub(RECENT)..];
    let acc = accuracy(&labelled);
    let ece = ece(&labelled);
    let next = match current {
        0 => {
            if labelled.len() >= L1_N && acc >= L1_ACC && ece <= MAX_ECE {
                1
            } else {
                0
            }
        }
        1 => {
            let wrong = recent.iter().filter(|r| r.correct() == Some(false)).count();
            if wrong >= L1_STRIKES {
                0
            } else if labelled.len() >= L2_N
                && acc >= L2_ACC
                && ece <= MAX_ECE
                && recent.iter().all(|r| !r.overridden)
            {
                2
            } else {
                1
            }
        }
        2 => {
            let strikes = rows.iter().any(|r| r.strike);
            let long_enough = l2_since.is_some_and(|t| now - t >= Duration::days(L3_DAYS));
            if !strikes && long_enough { 3 } else { 2 }
        }
        r => r,
    };
    next.min(cap)
}

/// Share of labelled rows whose choice matched.
pub fn accuracy(labelled: &[&Scored]) -> f32 {
    if labelled.is_empty() {
        return 0.0;
    }
    let ok = labelled.iter().filter(|r| r.correct() == Some(true)).count();
    ok as f32 / labelled.len() as f32
}

/// Mean multi-class Brier score: Σ (p_i − y_i)² over the row's options.
pub fn brier(labelled: &[&Scored]) -> f32 {
    if labelled.is_empty() {
        return 0.0;
    }
    let sum: f32 = labelled
        .iter()
        .map(|r| {
            r.probs
                .iter()
                .map(|(l, p)| {
                    let y = if r.label.as_deref() == Some(l) { 1.0 } else { 0.0 };
                    (p - y) * (p - y)
                })
                .sum::<f32>()
        })
        .sum();
    sum / labelled.len() as f32
}

/// Expected calibration error over [`BINS`] equal-width bins of `p_top`.
pub fn ece(labelled: &[&Scored]) -> f32 {
    if labelled.is_empty() {
        return 0.0;
    }
    let mut n = [0usize; BINS];
    let mut conf = [0f32; BINS];
    let mut hit = [0f32; BINS];
    for r in labelled {
        let b = ((r.p_top.clamp(0.0, 1.0) * BINS as f32) as usize).min(BINS - 1);
        n[b] += 1;
        conf[b] += r.p_top;
        if r.correct() == Some(true) {
            hit[b] += 1.0;
        }
    }
    let total = labelled.len() as f32;
    (0..BINS)
        .filter(|&b| n[b] > 0)
        .map(|b| {
            let k = n[b] as f32;
            (k / total) * ((hit[b] / k) - (conf[b] / k)).abs()
        })
        .sum()
}

/// coverage@[`COVERAGE_EPS`]: the largest share of labelled rows, taken in
/// descending `p_top`, whose error rate stays ≤ ε.
pub fn coverage(labelled: &[&Scored]) -> f32 {
    if labelled.is_empty() {
        return 0.0;
    }
    let mut by_p: Vec<&Scored> = labelled.to_vec();
    by_p.sort_by(|a, b| b.p_top.total_cmp(&a.p_top));
    let mut wrong = 0usize;
    let mut best = 0usize;
    for (i, r) in by_p.iter().enumerate() {
        if r.correct() == Some(false) {
            wrong += 1;
        }
        if wrong as f32 / (i + 1) as f32 <= COVERAGE_EPS {
            best = i + 1;
        }
    }
    best as f32 / labelled.len() as f32
}

/// The §7.4 numbers over `rows`; `median_answer_ms` is the class's (or the
/// overall) owner time-to-answer, for "minutes saved".
pub fn kpis(rows: &[Scored], median_answer_ms: u32) -> Kpis {
    let labelled: Vec<&Scored> = rows.iter().filter(|r| r.label.is_some()).collect();
    let acted = rows.iter().filter(|r| r.acted).count();
    let overrides = rows.iter().filter(|r| r.overridden).count();
    // An undo during grace: the card went through a grace and the double did not act.
    let undos = rows.iter().filter(|r| r.graced && !r.acted).count();
    let per_100 = |k: usize| {
        if acted == 0 {
            0.0
        } else {
            k as f32 * 100.0 / acted as f32
        }
    };
    Kpis {
        n: labelled.len() as u32,
        accuracy: accuracy(&labelled),
        brier: brier(&labelled),
        ece: ece(&labelled),
        coverage_2pct: coverage(&labelled),
        asks_avoided: acted as u32,
        minutes_saved: acted as f32 * (median_answer_ms as f32 / 1000.0 + INTERRUPTION_S) / 60.0,
        overrides_per_100: per_100(overrides),
        undo_rate_per_100: per_100(undos),
    }
}

/// What one more correct answer would do (the chip popover, §10).
pub fn promote_hint(current: u8, cap: u8, rows: &[Scored]) -> String {
    let labelled: Vec<&Scored> = rows.iter().filter(|r| r.label.is_some()).collect();
    if current >= cap {
        return format!("at its cap (L{cap})");
    }
    let acc = accuracy(&labelled);
    let (need_n, need_acc) = match current {
        0 => (L1_N, L1_ACC),
        1 => (L2_N, L2_ACC),
        _ => return format!("{L3_DAYS} days at L2 with no strike"),
    };
    let more_n = need_n.saturating_sub(labelled.len());
    if more_n > 0 {
        return format!("{more_n} more labelled decisions (n ≥ {need_n})");
    }
    if acc < need_acc {
        // k more correct in a row lift acc = ok/n to (ok+k)/(n+k) ≥ need_acc.
        let ok = labelled.iter().filter(|r| r.correct() == Some(true)).count() as f32;
        let n = labelled.len() as f32;
        let k = ((need_acc * n - ok) / (1.0 - need_acc)).ceil().max(1.0) as usize;
        return format!("{k} more correct in a row (acc ≥ {need_acc:.2})");
    }
    if ece(&labelled) > MAX_ECE {
        return format!("better calibration (ECE ≤ {MAX_ECE})");
    }
    format!("promotes to L{} on the next label", current + 1)
}

/// Weekly points, oldest first: `current` and `baseline` rows grouped by the
/// Monday of their ISO week.
pub fn curve(current: &[Scored], baseline: &[Scored], median_answer_ms: u32) -> Vec<CurvePoint> {
    fn week_of(t: DateTime<Utc>) -> DateTime<Utc> {
        let d = t.date_naive();
        let monday = d - Duration::days(i64::from(d.weekday().num_days_from_monday()));
        monday.and_hms_opt(0, 0, 0).expect("midnight").and_utc()
    }
    let mut weeks: HashMap<DateTime<Utc>, (Vec<Scored>, Vec<Scored>)> = HashMap::new();
    for r in current {
        weeks.entry(week_of(r.at)).or_default().0.push(r.clone());
    }
    for r in baseline {
        weeks.entry(week_of(r.at)).or_default().1.push(r.clone());
    }
    let mut keys: Vec<_> = weeks.keys().copied().collect();
    keys.sort();
    keys.into_iter()
        .map(|w| {
            let (c, b) = &weeks[&w];
            CurvePoint {
                week: w.to_rfc3339(),
                current: kpis(c, median_answer_ms),
                baseline: kpis(b, median_answer_ms),
            }
        })
        .collect()
}

/// Load the ledger: predictions since `since` — the `baseline` version's or
/// every other version's (`baseline: false` = the live, active-at-the-time
/// curve) — for one class or all, joined to their labels. Time order.
pub async fn ledger(
    store: &Store,
    class: Option<&str>,
    baseline: bool,
    since: DateTime<Utc>,
) -> anyhow::Result<Vec<Scored>> {
    let op = if baseline { "=" } else { "!=" };
    let class_clause = if class.is_some() { "AND e.class = ?" } else { "" };
    let sql = format!(
        "SELECT p.example_id, p.choice, p.probs, p.p_top, p.acted, p.grace_until, p.at
         FROM predictions p JOIN examples e ON e.id = p.example_id
         WHERE p.version {op} '{BASELINE}' AND p.at >= ? {class_clause}
         ORDER BY p.at, p.example_id"
    );
    let mut q = sqlx::query(sqlx::AssertSqlSafe(sql)).bind(since.to_rfc3339());
    if let Some(c) = class {
        q = q.bind(c);
    }
    let preds = q.fetch_all(store.pool()).await?;
    let sql = format!(
        "SELECT l.example_id, l.source, l.choice, l.weight
         FROM labels l JOIN examples e ON e.id = l.example_id
         WHERE e.asked_at >= ? {class_clause}"
    );
    let mut q = sqlx::query(sqlx::AssertSqlSafe(sql)).bind(since.to_rfc3339());
    if let Some(c) = class {
        q = q.bind(c);
    }
    let mut labels: HashMap<String, Vec<(LabelSource, String, f32)>> = HashMap::new();
    for r in q.fetch_all(store.pool()).await? {
        let id: String = r.try_get("example_id")?;
        let source: String = r.try_get("source")?;
        let Some(source) = LabelSource::parse(&source) else {
            continue;
        };
        let choice: String = r.try_get("choice")?;
        let weight: f64 = r.try_get("weight")?;
        labels
            .entry(id)
            .or_default()
            .push((source, choice, weight as f32));
    }
    preds
        .into_iter()
        .map(|r| {
            let id: String = r.try_get("example_id")?;
            let probs: String = r.try_get("probs")?;
            let p_top: f64 = r.try_get("p_top")?;
            let acted: i64 = r.try_get("acted")?;
            let grace: Option<String> = r.try_get("grace_until")?;
            let at: String = r.try_get("at")?;
            let ls = labels.get(&id).map(Vec::as_slice).unwrap_or(&[]);
            let (label, overridden, strike) = summarise(ls, acted != 0);
            Ok(Scored {
                at: DateTime::parse_from_rfc3339(&at)?.with_timezone(&Utc),
                choice: r.try_get("choice")?,
                probs: serde_json::from_str(&probs)?,
                p_top: p_top as f32,
                label,
                acted: acted != 0,
                graced: grace.is_some(),
                overridden,
                strike,
            })
        })
        .collect()
}

/// `(label, overridden, strike)` from an example's labels: the override's
/// choice wins over the direct one; a timeout is not a label (§6).
pub fn summarise(labels: &[(LabelSource, String, f32)], acted: bool) -> (Option<String>, bool, bool) {
    let mut label = None;
    let mut overridden = false;
    let mut strike = false;
    for (source, choice, weight) in labels {
        match source {
            LabelSource::Override => {
                label = Some(choice.clone());
                overridden = true;
                strike = true;
            }
            LabelSource::Direct => {
                if label.is_none() {
                    label = Some(choice.clone());
                }
            }
            LabelSource::HindsightRewind
            | LabelSource::HindsightCorrection
            | LabelSource::HindsightInterrupt => {
                if acted && *weight >= 0.5 {
                    strike = true;
                }
            }
            LabelSource::Timeout | LabelSource::Excluded => {}
        }
    }
    (label, overridden, strike)
}

/// Outcome of a prediction's `choice` against the example's labels (§7.4):
/// `Unlabelled` until a direct / override label exists.
pub fn outcome(choice: &str, labels: &[(LabelSource, String, f32)]) -> Outcome {
    match summarise(labels, false).0 {
        None => Outcome::Unlabelled,
        Some(l) if l == choice => Outcome::Correct,
        Some(_) => Outcome::Wrong,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(correct: bool, p: f32) -> Scored {
        Scored {
            at: Utc::now(),
            choice: "Allow".into(),
            probs: vec![("Allow".into(), p), ("Deny".into(), 1.0 - p)],
            p_top: p,
            label: Some(if correct { "Allow" } else { "Deny" }.into()),
            acted: false,
            graced: false,
            overridden: false,
            strike: false,
        }
    }

    fn rows(n: usize, wrong: usize, p: f32) -> Vec<Scored> {
        (0..n).map(|i| row(i >= wrong, p)).collect()
    }

    /// Promotes exactly at the §9 thresholds, one rung at a time, never over the cap.
    #[test]
    fn promotes_at_the_thresholds() {
        let now = Utc::now();
        // L0 → L1: n ≥ 20, acc ≥ 0.90.
        assert_eq!(next_rung(0, 2, &rows(19, 0, 0.92), None, now), 0);
        assert_eq!(next_rung(0, 2, &rows(20, 2, 0.92), None, now), 1); // acc 0.90
        assert_eq!(next_rung(0, 2, &rows(20, 3, 0.92), None, now), 0); // acc 0.85
        // One step only: 60 perfect rows at L0 still land on L1.
        assert_eq!(next_rung(0, 2, &rows(60, 0, 0.99), None, now), 1);
        // L1 → L2: n ≥ 50, acc ≥ 0.97.
        assert_eq!(next_rung(1, 2, &rows(49, 0, 0.99), None, now), 1);
        assert_eq!(next_rung(1, 2, &rows(50, 1, 0.99), None, now), 2); // acc 0.98
        assert_eq!(next_rung(1, 2, &rows(100, 4, 0.99), None, now), 1); // acc 0.96
        // The cap holds.
        assert_eq!(next_rung(1, 1, &rows(50, 0, 0.99), None, now), 1);
        assert_eq!(next_rung(0, 0, &rows(50, 0, 0.99), None, now), 0);
    }

    /// Miscalibration blocks promotion: confident (0.99) and 10 % wrong → ECE 0.09.
    #[test]
    fn ece_blocks_promotion() {
        let now = Utc::now();
        // acc 0.90 would promote; p_top 0.99 → ECE 0.09 > 0.05 holds it at L0.
        assert_eq!(next_rung(0, 2, &rows(20, 2, 0.99), None, now), 0);
    }

    /// Three wrong in the last 20 at L1 → L0; an override at L2 → L1 (demote).
    #[test]
    fn demotes() {
        let now = Utc::now();
        let mut r = rows(40, 0, 0.95);
        r.extend(rows(20, 3, 0.95));
        assert_eq!(next_rung(1, 2, &r, None, now), 0);
        assert_eq!(demote(2), 1);
        assert_eq!(demote(0), 0);
        // A recent override blocks L1 → L2 even with perfect accuracy.
        let mut r = rows(60, 0, 0.99);
        r[59].overridden = true;
        assert_eq!(next_rung(1, 2, &r, None, now), 1);
        // A strike at L2 blocks L3; 30 quiet days with the cap at 3 promote.
        let r = rows(60, 0, 0.99);
        let since = now - Duration::days(31);
        assert_eq!(next_rung(2, 3, &r, Some(since), now), 3);
        assert_eq!(next_rung(2, 2, &r, Some(since), now), 2);
        let mut s = r.clone();
        s[0].strike = true;
        assert_eq!(next_rung(2, 3, &s, Some(since), now), 2);
        assert_eq!(next_rung(2, 3, &r, Some(now - Duration::days(5)), now), 2);
    }

    /// Hard caps: destructive ≤ 1 always; mutating ≤ 1 without opt-in; asks take `read_only`.
    #[test]
    fn hard_caps() {
        let caps = Caps::default();
        assert_eq!(
            hard_cap(&caps, "approval:shell:destructive:rm -rf:root", true).0,
            1
        );
        let (cap, why) = hard_cap(&caps, "approval:shell:mutating:git push:root", false);
        assert_eq!((cap, why.as_deref()), (1, Some("mutating — opt in to allow L2")));
        assert_eq!(hard_cap(&caps, "approval:shell:mutating:git push:root", true), (2, None));
        assert_eq!(hard_cap(&caps, "approval:read_file:read_only:root", false), (2, None));
        assert_eq!(hard_cap(&caps, "ask:b9474647c6a4:root", false), (2, None));
        assert_eq!(effective_rung(2, true), 0);
        assert_eq!(effective_rung(2, false), 2);
    }

    /// ECE is 0 when confidence equals accuracy per bin, > 0 otherwise; coverage counts
    /// the confident-and-right prefix; Brier is 0 for a perfect one-hot.
    #[test]
    fn metrics() {
        // 10 rows at p 0.9, 9 right: bin accuracy 0.9 = confidence → ECE 0.
        let r = rows(10, 1, 0.9);
        let l: Vec<&Scored> = r.iter().collect();
        assert!(ece(&l) < 1e-6, "{}", ece(&l));
        assert!((accuracy(&l) - 0.9).abs() < 1e-6);
        // All 0.99 and all right: ECE 0.01.
        let r = rows(10, 0, 0.99);
        let l: Vec<&Scored> = r.iter().collect();
        assert!((ece(&l) - 0.01).abs() < 1e-5);
        assert!(brier(&l) < 1e-3);
        // Coverage: 100 rows, the 2 wrong ones at the lowest p → 98 % covered… the prefix of
        // 98 confident rows has 0 errors; adding the 99th makes 1/99 ≈ 1 % ≤ 2 %; the 100th 2 %.
        let mut r = rows(98, 0, 0.9);
        r.extend(rows(2, 2, 0.5));
        let l: Vec<&Scored> = r.iter().collect();
        assert!((coverage(&l) - 1.0).abs() < 1e-6);
        // 3 wrong of 100 at low p: the prefix stops where 3/100 > 2 % → 99 %.
        let mut r = rows(97, 0, 0.9);
        r.extend(rows(3, 3, 0.5));
        let l: Vec<&Scored> = r.iter().collect();
        assert!((coverage(&l) - 0.99).abs() < 1e-6);
        let k = kpis(&r, 2_000);
        assert_eq!(k.n, 100);
        assert_eq!(k.asks_avoided, 0);
        assert_eq!(k.overrides_per_100, 0.0);
    }

    #[test]
    fn hints() {
        assert_eq!(promote_hint(0, 2, &rows(5, 0, 0.9)), "15 more labelled decisions (n ≥ 20)");
        assert_eq!(promote_hint(0, 2, &rows(20, 4, 0.9)), "4 more correct in a row (acc ≥ 0.90)");
        assert_eq!(promote_hint(1, 1, &rows(60, 0, 0.9)), "at its cap (L1)");
    }

    #[test]
    fn summary_and_outcome() {
        let ls = vec![
            (LabelSource::Direct, "Allow".to_owned(), 1.0),
            (LabelSource::Override, "Deny".to_owned(), 3.0),
        ];
        assert_eq!(summarise(&ls, true), (Some("Deny".into()), true, true));
        assert_eq!(outcome("Allow", &ls), Outcome::Wrong);
        assert_eq!(outcome("Deny", &ls), Outcome::Correct);
        let hs = vec![
            (LabelSource::Direct, "Allow".to_owned(), 1.0),
            (LabelSource::HindsightRewind, "Allow".to_owned(), 0.5),
        ];
        assert_eq!(summarise(&hs, true).2, true);
        assert_eq!(summarise(&hs, false).2, false);
        assert_eq!(outcome("Allow", &[(LabelSource::Timeout, "Deny".into(), 0.25)]), Outcome::Unlabelled);
    }
}
