//! Owner: E1 — the run's end (design 171 §7.5–7.6): expect checks on the cleaned text, the
//! summary, `after` arming.

use std::sync::LazyLock;

use regex::Regex;

use crate::types::{Armed, CellRun, Expect, ExpectResult, RunState};

/// Pulumi plan counts: `~ 3 to update`, `+ 1 to create`, `- 2 to delete`, `+-1 to replace`.
static PULUMI: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(\d+) to (create|update|replace|delete)\b")
        .unwrap_or_else(|e| unreachable!("pulumi plan regex: {e}"))
});

/// Terraform plan line: `Plan: 1 to add, 2 to change, 0 to destroy.`
static TERRAFORM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"Plan: (\d+) to add, (\d+) to change, (\d+) to destroy")
        .unwrap_or_else(|e| unreachable!("terraform plan regex: {e}"))
});

/// §7.5 `/regex/` items: Rust regex, multi-line; `None` when the source is invalid.
fn compile(src: &str) -> Option<Regex> {
    regex::RegexBuilder::new(src).multi_line(true).build().ok()
}

/// §7.5: each item met or missed (plain = substring, `/re/` = multi-line regex).
pub fn check_expect(text: &str, items: &[Expect]) -> Vec<ExpectResult> {
    items
        .iter()
        .map(|item| {
            let met = match item {
                Expect::Text(s) => text.contains(s.as_str()),
                Expect::Regex(src) => compile(src).is_some_and(|re| re.is_match(text)),
            };
            ExpectResult {
                item: item.clone(),
                met,
            }
        })
        .collect()
}

/// §7.5: plan summary (`~3 update · 0 replace · 0 delete`), else the met expect lines, else
/// the last non-empty line; ≤ 80 chars.
pub fn summary(text: &str, expect: &[ExpectResult]) -> Option<String> {
    let picked = plan_summary(text)
        .or_else(|| met_lines(text, expect))
        .or_else(|| {
            text.lines()
                .rev()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .map(str::to_owned)
        })?;
    let picked = picked.trim();
    if picked.is_empty() {
        return None;
    }
    Some(cap(picked, 80))
}

/// Terraform's `Plan:` line, else pulumi's `N to <verb>` counts summed per verb.
fn plan_summary(text: &str) -> Option<String> {
    if let Some(c) = TERRAFORM.captures(text) {
        let n = |i: usize| c.get(i).map_or("0", |m| m.as_str());
        return Some(format!(
            "+{} add · ~{} change · -{} destroy",
            n(1),
            n(2),
            n(3)
        ));
    }
    let (mut create, mut update, mut replace, mut delete) = (0u64, 0u64, 0u64, 0u64);
    let mut seen = false;
    for c in PULUMI.captures_iter(text) {
        let Some(n) = c.get(1).and_then(|m| m.as_str().parse::<u64>().ok()) else {
            continue;
        };
        seen = true;
        match c.get(2).map_or("", |m| m.as_str()) {
            "create" => create += n,
            "update" => update += n,
            "replace" => replace += n,
            _ => delete += n,
        }
    }
    if !seen {
        return None;
    }
    let mut out = String::new();
    if create > 0 {
        out.push_str(&format!("+{create} create · "));
    }
    out.push_str(&format!(
        "~{update} update · {replace} replace · {delete} delete"
    ));
    Some(out)
}

/// The first line that met each met expect item, joined with ` · `.
fn met_lines(text: &str, expect: &[ExpectResult]) -> Option<String> {
    let lines: Vec<&str> = expect
        .iter()
        .filter(|r| r.met)
        .filter_map(|r| match &r.item {
            Expect::Text(s) => text.lines().find(|l| l.contains(s.as_str())),
            Expect::Regex(src) => {
                let re = compile(src)?;
                text.lines().find(|l| re.is_match(l))
            }
        })
        .map(str::trim)
        .collect();
    (!lines.is_empty()).then(|| lines.join(" · "))
}

/// At most `max` chars; a cut ends in `…`.
fn cap(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// §7.6: `Some` when `gate` (the named cell's latest run) succeeded, ran `current_code_sha`,
/// and ended within `max_age_mins` of `now_ms`.
pub fn armed(
    gate: &CellRun,
    current_code_sha: &str,
    now_ms: i64,
    max_age_mins: u64,
) -> Option<Armed> {
    if gate.state != RunState::Done || gate.code_sha != current_code_sha {
        return None;
    }
    let ended = gate.ended_ms?;
    if !fresh(ended, now_ms, max_age_mins) {
        return None;
    }
    Some(Armed {
        by: gate.id.clone(),
        at_ms: ended,
        summary: gate.summary.clone().unwrap_or_default(),
    })
}

/// `ended` is at most `max_age_mins` before `now_ms`.
fn fresh(ended: i64, now_ms: i64, max_age_mins: u64) -> bool {
    let max_ms = i64::try_from(max_age_mins.saturating_mul(60_000)).unwrap_or(i64::MAX);
    now_ms.saturating_sub(ended) <= max_ms
}

/// §7.6: the reason an `after: <after_name>` cell whose gate is not armed warns with.
pub fn disarmed_reason(
    gate: Option<&CellRun>,
    after_name: &str,
    current_code_sha: &str,
    now_ms: i64,
    max_age_mins: u64,
) -> String {
    let Some(gate) = gate else {
        return format!("after: {after_name} — run it first");
    };
    let ended = match gate.ended_ms {
        Some(ended) if gate.state == RunState::Done => ended,
        _ => return format!("after: {after_name} did not succeed — run it again"),
    };
    if gate.code_sha != current_code_sha {
        return format!("after: {after_name} was edited since it ran — run it again");
    }
    if !fresh(ended, now_ms, max_age_mins) {
        let age = if max_age_mins > 0 && max_age_mins.is_multiple_of(60) {
            format!("{} h", max_age_mins / 60)
        } else {
            format!("{max_age_mins} min")
        };
        return format!("the {after_name} run is more than {age} old — run it again");
    }
    format!("after: {after_name} — run it first")
}

/// §7.6: the one line a Run anyway run carries (its history entry, its `<cell_result>`), from
/// the gate's name and its [`disarmed_reason`]:
/// `ran past after: preview — the preview run is more than 1 h old`.
pub fn ran_past(after_name: &str, reason: &str) -> String {
    let why = reason
        .strip_suffix(" — run it again")
        .or_else(|| reason.strip_suffix(" — run it first"))
        .unwrap_or(reason);
    let why = match why.strip_prefix(&format!("after: {after_name}")) {
        Some("") => "it had not run".to_owned(),
        Some(rest) => format!("it{rest}"),
        None => why.to_owned(),
    };
    format!("ran past after: {after_name} — {why}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CellRef, KernelId, KernelSpec, RunId, RunMode, Scope, SessionId};

    const SHA: &str = "sha-a";
    const HOUR_MS: i64 = 60 * 60_000;

    fn done_run(ended_ms: i64) -> CellRun {
        CellRun {
            id: RunId("r_1".into()),
            cell: CellRef::Chat {
                session: SessionId(uuid::Uuid::nil()),
                entry: "m".into(),
                ordinal: 1,
            },
            mode: RunMode::Cell,
            pass: None,
            kernel: KernelId {
                scope: Scope::Session(SessionId(uuid::Uuid::nil())),
                spec: KernelSpec::Monty,
                instance: 0,
            },
            interpreter: "monty".into(),
            cwd: "~".into(),
            env_names: Vec::new(),
            presented_sha: SHA.into(),
            code_sha: SHA.into(),
            edited: false,
            pressed_by: "desktop".into(),
            state: RunState::Done,
            exit: Some(crate::types::ExitStatus::Code(0)),
            secs: Some(1.0),
            expect: Vec::new(),
            summary: Some("~3 update · 0 replace · 0 delete".into()),
            output_bytes: Some(10),
            queued_ms: ended_ms - 2_000,
            started_ms: Some(ended_ms - 1_000),
            ended_ms: Some(ended_ms),
            masked: 0,
            private: false,
            name: None,
            input: None,
            ran_past: None,
        }
    }

    const PULUMI_TEXT: &str = "Previewing update (ir-prod1):
     Type                              Name                          Plan       Info
     pulumi:pulumi:Stack               sa-frp-integration-ir-prod1
 ~   ├─ aws:secretsmanager:Secret      frp-api-credentials           update     [diff: ~tags]
Resources:
    ~ 3 to update
    4 unchanged";

    #[test]
    fn armed_fresh_preview() {
        let now = 10 * HOUR_MS;
        let gate = done_run(now - 59 * 60_000);
        let a = armed(&gate, SHA, now, 60).expect("armed");
        assert_eq!(a.by, RunId("r_1".into()));
        assert_eq!(a.at_ms, now - 59 * 60_000);
        assert_eq!(a.summary, "~3 update · 0 replace · 0 delete");
    }

    #[test]
    fn armed_edited_preview_disarms() {
        let now = 10 * HOUR_MS;
        let gate = done_run(now - 1_000);
        assert!(armed(&gate, "sha-b", now, 60).is_none());
        assert_eq!(
            disarmed_reason(Some(&gate), "preview", "sha-b", now, 60),
            "after: preview was edited since it ran — run it again"
        );
    }

    #[test]
    fn armed_old_preview_disarms() {
        let now = 10 * HOUR_MS;
        assert!(armed(&done_run(now - 61 * 60_000), SHA, now, 60).is_none());
        assert!(armed(&done_run(now - 59 * 60_000), SHA, now, 60).is_some());
        assert_eq!(
            disarmed_reason(Some(&done_run(now - 61 * 60_000)), "preview", SHA, now, 60),
            "the preview run is more than 1 h old — run it again"
        );
        assert_eq!(
            disarmed_reason(Some(&done_run(now - 61 * 60_000)), "preview", SHA, now, 45),
            "the preview run is more than 45 min old — run it again"
        );
    }

    #[test]
    fn armed_failed_run_disarms() {
        let now = 10 * HOUR_MS;
        let mut gate = done_run(now - 1_000);
        gate.state = RunState::Failed;
        gate.exit = Some(crate::types::ExitStatus::Code(1));
        assert!(armed(&gate, SHA, now, 60).is_none());
        assert_eq!(
            disarmed_reason(Some(&gate), "preview", SHA, now, 60),
            "after: preview did not succeed — run it again"
        );
        assert_eq!(
            disarmed_reason(None, "preview", SHA, now, 60),
            "after: preview — run it first"
        );
    }

    #[test]
    fn ran_past_names_the_gate_and_why_once() {
        let now = 10 * HOUR_MS;
        let old = done_run(now - 61 * 60_000);
        let mut failed = done_run(now - 1_000);
        failed.state = RunState::Failed;
        let line = |gate: Option<&CellRun>, sha: &str| {
            ran_past("prepare", &disarmed_reason(gate, "prepare", sha, now, 60))
        };
        assert_eq!(
            line(Some(&old), SHA),
            "ran past after: prepare — the prepare run is more than 1 h old"
        );
        assert_eq!(line(None, SHA), "ran past after: prepare — it had not run");
        assert_eq!(
            line(Some(&failed), SHA),
            "ran past after: prepare — it did not succeed"
        );
        assert_eq!(
            line(Some(&done_run(now - 1_000)), "sha-b"),
            "ran past after: prepare — it was edited since it ran"
        );
    }

    #[test]
    fn check_expect_text_and_regex() {
        let items = [
            Expect::Text("3 to update".into()),
            Expect::Regex(r"^\s+~ \d+ to update$".into()),
            Expect::Text("to delete".into()),
            Expect::Regex("(unclosed".into()),
        ];
        let met: Vec<bool> = check_expect(PULUMI_TEXT, &items)
            .iter()
            .map(|r| r.met)
            .collect();
        assert_eq!(met, [true, true, false, false]);
    }

    #[test]
    fn summary_pulumi_plan() {
        assert_eq!(
            summary(PULUMI_TEXT, &[]).as_deref(),
            Some("~3 update · 0 replace · 0 delete")
        );
        assert_eq!(
            summary(
                "+ 1 to create\n~ 2 to update\n- 1 to delete\n+-1 to replace",
                &[]
            )
            .as_deref(),
            Some("+1 create · ~2 update · 1 replace · 1 delete")
        );
    }

    #[test]
    fn summary_terraform_plan() {
        assert_eq!(
            summary("...\nPlan: 1 to add, 2 to change, 0 to destroy.\n", &[]).as_deref(),
            Some("+1 add · ~2 change · -0 destroy")
        );
    }

    #[test]
    fn summary_met_expect_lines() {
        let text = "starting\nhealth: ok\nbye";
        let expect = check_expect(text, &[Expect::Regex("^health: .*".into())]);
        assert_eq!(summary(text, &expect).as_deref(), Some("health: ok"));
    }

    #[test]
    fn summary_falls_back_to_last_line() {
        assert_eq!(summary("a\nb\n  last  \n\n", &[]).as_deref(), Some("last"));
        assert_eq!(summary("", &[]), None);
        assert_eq!(summary("  \n\n", &[]), None);
    }

    #[test]
    fn summary_capped_at_80_chars() {
        let long = "é".repeat(100);
        let s = summary(&long, &[]).expect("summary");
        assert_eq!(s.chars().count(), 80);
        assert!(s.ends_with('…'));
        assert!(s.starts_with(&"é".repeat(79)));
    }
}
