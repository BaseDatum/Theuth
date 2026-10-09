//! 151 — the setup chat's engine side (design doc 151 §5–§6): the living
//! draft's field states (`DraftView`), the probe that is the only source of
//! "Verified", and the panel's edit / confirm / discard. The UI never computes
//! a state; everything it shows comes from [`view`].

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio_util::sync::CancellationToken;
use tools::{Risk, Tool, ToolCx, ToolOutput};

use crate::config::AutoresearchConfig;
use crate::error::{Error, Result};
use crate::runner::{self, SampleRequest};
use crate::tools::{Draft, Seams, draft_spec_path, read_draft, validate_spec};
use crate::types::{
    BudgetSpec, Direction, DraftField, DraftSummary, DraftView, FieldState, Guard, ModelsSpec,
    ProbeKind, ProbeResult, SampleRole, StudySpec,
};
use crate::{drafts_dir, git};

// ───────────────────────────── rows and leaves (§4.3) ─────────────────────────────

/// The nine main rows, in panel order.
pub const MAIN_ROWS: [&str; 9] = [
    "goal", "repo", "metric", "bench", "checks", "holdout", "scope", "budget", "window",
];
/// The advanced rows, in panel order.
pub const ADVANCED_ROWS: [&str; 5] = [
    "secondary_guards",
    "warm",
    "parallel",
    "models",
    "evaluator_paths",
];
/// Rows Start needs (§5).
const REQUIRED: [&str; 6] = ["goal", "repo", "metric", "bench", "checks", "scope"];
/// Rows that are always present and read `default` while equal to the defaults and untouched.
const DEFAULTABLE: [&str; 3] = ["budget", "parallel", "models"];

/// Every leaf `draft_edit.field` accepts (§4.3).
pub const LEAVES: [&str; 32] = [
    "goal",
    "repo",
    "base",
    "metric.name",
    "metric.unit",
    "metric.direction",
    "metric.min_effect",
    "bench.command",
    "bench.timeout",
    "bench.repeats",
    "bench.exclusive",
    "bench.smoke",
    "checks.command",
    "checks.timeout",
    "holdout.command",
    "holdout.every",
    "holdout.repeats",
    "scope",
    "off_limits",
    "evaluator_paths",
    "budget.usd",
    "budget.per_experiment_usd",
    "budget.hours",
    "budget.experiments",
    "window",
    "warm",
    "parallel",
    "secondary_guards",
    "models.executor",
    "models.review",
    "models.advisor",
    "include_dirty",
];

/// The panel label of a row.
pub fn label(row: &str) -> &'static str {
    match row {
        "goal" => "Goal",
        "repo" => "Repository",
        "metric" => "Metric",
        "bench" => "Benchmark",
        "checks" => "Check",
        "holdout" => "Holdout",
        "scope" => "Scope",
        "budget" => "Budget",
        "window" => "When",
        "secondary_guards" => "Secondary guards",
        "warm" => "Warm",
        "parallel" => "Parallel",
        "models" => "Models",
        "evaluator_paths" => "Evaluator paths",
        _ => "?",
    }
}

/// Row of a leaf: the first segment, except `base → repo`, `off_limits → scope`.
pub fn row_of(leaf: &str) -> &str {
    match leaf {
        "base" => "repo",
        "off_limits" => "scope",
        l => l.split('.').next().unwrap_or(l),
    }
}

fn non_empty(s: &str) -> bool {
    !s.trim().is_empty()
}

fn opt_non_empty(s: Option<&String>) -> bool {
    s.is_some_and(|s| non_empty(s))
}

/// Whether the row has a value (rule 1/2 of §5).
pub fn present(spec: &StudySpec, row: &str) -> bool {
    match row {
        "goal" => non_empty(&spec.goal),
        "repo" => non_empty(&spec.repo),
        "metric" => non_empty(&spec.metric.name),
        "bench" => non_empty(&spec.bench.command),
        "checks" => spec.checks.as_ref().is_some_and(|c| non_empty(&c.command)),
        "holdout" => spec.holdout.as_ref().is_some_and(|h| non_empty(&h.command)),
        "scope" => !spec.scope.is_empty(),
        "window" => opt_non_empty(spec.window.as_ref()),
        "secondary_guards" => !spec.secondary_guards.is_empty(),
        "warm" => opt_non_empty(spec.warm.as_ref()),
        "evaluator_paths" => !spec.evaluator_paths.is_empty(),
        _ => DEFAULTABLE.contains(&row),
    }
}

/// The row's leaves as JSON — what the sticky check compares across re-drafts.
pub fn row_json(spec: &StudySpec, row: &str) -> serde_json::Value {
    match row {
        "goal" => serde_json::json!(spec.goal),
        "repo" => serde_json::json!({ "repo": spec.repo, "base": spec.base }),
        "metric" => serde_json::json!(spec.metric),
        "bench" => serde_json::json!(spec.bench),
        "checks" => serde_json::json!(spec.checks),
        "holdout" => serde_json::json!(spec.holdout),
        "scope" => serde_json::json!({ "scope": spec.scope, "off_limits": spec.off_limits }),
        "budget" => serde_json::json!(spec.budget),
        "window" => serde_json::json!(spec.window),
        "secondary_guards" => serde_json::json!(spec.secondary_guards),
        "warm" => serde_json::json!(spec.warm),
        "parallel" => serde_json::json!(spec.parallel),
        "models" => serde_json::json!(spec.models),
        "evaluator_paths" => serde_json::json!(spec.evaluator_paths),
        _ => serde_json::Value::Null,
    }
}

fn trim_float(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

fn dir_word(d: Direction) -> &'static str {
    match d {
        Direction::Lower => "lower",
        Direction::Higher => "higher",
    }
}

/// The engine's one-line rendering of a row (empty when absent).
pub fn value(spec: &StudySpec, row: &str) -> String {
    if !present(spec, row) {
        return String::new();
    }
    let join = |v: &[String]| v.join(", ");
    match row {
        "goal" => spec.goal.trim().to_owned(),
        "repo" => format!(
            "{} · base {}{}",
            spec.repo.trim(),
            if non_empty(&spec.base) {
                spec.base.trim()
            } else {
                "HEAD"
            },
            if spec.include_dirty {
                " + your uncommitted changes"
            } else {
                ""
            }
        ),
        "metric" => {
            let m = &spec.metric;
            let mut parts = vec![m.name.trim().to_owned()];
            if non_empty(&m.unit) {
                parts.push(m.unit.trim().to_owned());
            }
            parts.push(dir_word(m.direction).to_owned());
            if non_empty(&m.min_effect) {
                parts.push(format!("keep ≥ {}", m.min_effect.trim()));
            }
            parts.join(" · ")
        }
        "bench" => {
            let b = &spec.bench;
            format!(
                "`{}` · {}× · {} each{}{}",
                b.command.trim(),
                b.repeats,
                b.timeout.trim(),
                if b.exclusive { " · exclusive" } else { "" },
                b.smoke
                    .as_deref()
                    .filter(|s| non_empty(s))
                    .map(|s| format!(" · smoke `{}`", s.trim()))
                    .unwrap_or_default()
            )
        }
        "checks" => {
            let c = spec.checks.as_ref().expect("present");
            format!("`{}` · {}", c.command.trim(), c.timeout.trim())
        }
        "holdout" => {
            let h = spec.holdout.as_ref().expect("present");
            format!(
                "`{}` · every {} keeps{}",
                h.command.trim(),
                h.every.max(1),
                h.repeats.map(|r| format!(" · {r}×")).unwrap_or_default()
            )
        }
        "scope" => {
            if spec.off_limits.is_empty() {
                join(&spec.scope)
            } else {
                format!(
                    "{} · off-limits {}",
                    join(&spec.scope),
                    join(&spec.off_limits)
                )
            }
        }
        "budget" => {
            let b = &spec.budget;
            format!(
                "${} · {} experiments · {} h · ${} per experiment",
                trim_float(b.usd),
                b.experiments,
                trim_float(b.hours),
                trim_float(b.per_experiment_usd)
            )
        }
        "window" => format!(
            "idle window `{}`",
            spec.window.as_deref().unwrap_or_default().trim()
        ),
        "secondary_guards" => spec
            .secondary_guards
            .iter()
            .map(|g| {
                format!(
                    "{} ≤ {} worse ({} is better)",
                    g.name.trim(),
                    g.max_regression.trim(),
                    dir_word(g.direction)
                )
            })
            .collect::<Vec<_>>()
            .join(", "),
        "warm" => format!("`{}`", spec.warm.as_deref().unwrap_or_default().trim()),
        "parallel" => {
            if spec.parallel > 1 {
                format!("waves of {}", spec.parallel)
            } else {
                "1".to_owned()
            }
        }
        "models" => {
            let m = &spec.models;
            let parts: Vec<String> = [
                ("executor", &m.executor),
                ("review", &m.review),
                ("advisor", &m.advisor),
            ]
            .into_iter()
            .filter_map(|(k, v)| {
                v.as_deref()
                    .filter(|v| non_empty(v))
                    .map(|v| format!("{k} {v}"))
            })
            .collect();
            if parts.is_empty() {
                "defaults".to_owned()
            } else {
                parts.join(" · ")
            }
        }
        "evaluator_paths" => join(&spec.evaluator_paths),
        _ => String::new(),
    }
}

// ───────────────────────────── validation (§6.1) ─────────────────────────────

/// What a partial draft must still satisfy: the slug rule and an absolute repo path.
pub fn validate_partial(spec: &StudySpec) -> std::result::Result<(), String> {
    crate::tools::check_slug(&spec.slug)?;
    if spec.repo.trim().is_empty() || !Path::new(spec.repo.trim()).is_absolute() {
        return Err("repo must be an absolute path".into());
    }
    Ok(())
}

// ───────────────────────────── probe.json ─────────────────────────────

/// `<root>/drafts/<id>/probe.json` — the newest result per kind.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ProbeFile {
    /// Newest `bench` probe.
    #[serde(default)]
    pub bench: Option<ProbeResult>,
    /// Newest `check` probe.
    #[serde(default)]
    pub check: Option<ProbeResult>,
    /// Newest `holdout` probe.
    #[serde(default)]
    pub holdout: Option<ProbeResult>,
}

impl ProbeFile {
    fn path(dir: &Path) -> PathBuf {
        dir.join("probe.json")
    }

    /// The file, or empty when absent / unreadable.
    pub fn read(dir: &Path) -> Self {
        std::fs::read(Self::path(dir))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// Write the file (creating `dir`).
    pub fn write(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        std::fs::write(Self::path(dir), serde_json::to_vec_pretty(self)?)
    }

    /// The newest result of `kind`.
    pub fn get(&self, kind: ProbeKind) -> Option<&ProbeResult> {
        match kind {
            ProbeKind::Bench => self.bench.as_ref(),
            ProbeKind::Check => self.check.as_ref(),
            ProbeKind::Holdout => self.holdout.as_ref(),
        }
    }

    /// Replace the result of `r.kind`.
    pub fn set(&mut self, r: ProbeResult) {
        let slot = match r.kind {
            ProbeKind::Bench => &mut self.bench,
            ProbeKind::Check => &mut self.check,
            ProbeKind::Holdout => &mut self.holdout,
        };
        *slot = Some(r);
    }
}

/// sha256 over every input a probe result depends on — the same inputs for
/// every kind (§6.2 step 5: a changed bench invalidates all three).
pub fn fingerprint(kind: ProbeKind, spec: &StudySpec, base_sha: &str) -> String {
    let b = &spec.bench;
    let v = serde_json::json!({
        "kind": kind,
        "repo": spec.repo.trim(),
        "base_sha": base_sha,
        "warm": spec.warm,
        "bench": { "command": b.command, "timeout": b.timeout, "repeats": b.repeats, "exclusive": b.exclusive, "smoke": b.smoke },
        "metric_name": spec.metric.name,
        "checks_command": spec.checks.as_ref().map(|c| &c.command),
        "holdout": spec.holdout.as_ref().map(|h| serde_json::json!({ "command": h.command, "repeats": h.repeats })),
    });
    let mut h = Sha256::new();
    h.update(v.to_string().as_bytes());
    format!("{:x}", h.finalize())
}

fn kind_of_row(row: &str) -> Option<ProbeKind> {
    match row {
        "bench" => Some(ProbeKind::Bench),
        "checks" => Some(ProbeKind::Check),
        "holdout" => Some(ProbeKind::Holdout),
        _ => None,
    }
}

/// `412 ms` / `12 s` / `1 m 50 s`.
pub fn duration(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms} ms")
    } else if ms < 60_000 {
        format!("{} s", ms / 1000)
    } else {
        format!("{} m {} s", ms / 60_000, (ms % 60_000) / 1000)
    }
}

fn num(v: f64) -> String {
    if v.abs() >= 100.0 {
        format!("{v:.0}")
    } else {
        trim_float(v)
    }
}

fn evidence(r: &ProbeResult) -> String {
    match r.kind {
        ProbeKind::Check => format!("passed in {}", duration(r.duration_ms)),
        _ => format!(
            "{} runs · {} {} · ±{:.1}%",
            r.samples,
            r.median.map(num).unwrap_or_default(),
            r.unit.trim(),
            r.noise_pct.unwrap_or(0.0)
        ),
    }
}

// ───────────────────────────── the view (§5) ─────────────────────────────

/// `autoresearch.draft {draft_id}`: the draft on disk as the panel's data.
pub fn view(root: &Path, draft_id: &str, started: bool) -> Result<DraftView> {
    Ok(view_of(root, read_draft(root, draft_id)?, started))
}

/// [`view`] on an already-read draft.
pub fn view_of(root: &Path, d: Draft, started: bool) -> DraftView {
    let spec = &d.spec;
    let repo = Path::new(spec.repo.trim());
    let (base_sha, dirty) = if non_empty(&spec.repo) && git::is_git_repo(repo) {
        match git::resolve_base(repo, &spec.base, true) {
            Ok(sha) => {
                let n = git::dirty_paths(repo).map(|v| v.len()).unwrap_or(0);
                (Some(sha), n)
            }
            Err(_) => (None, 0),
        }
    } else {
        (None, 0)
    };
    let file = ProbeFile::read(&drafts_dir(root, &spec.slug));
    let probes: Vec<ProbeResult> = [ProbeKind::Bench, ProbeKind::Check, ProbeKind::Holdout]
        .into_iter()
        .filter_map(|k| file.get(k))
        .filter(|r| {
            base_sha
                .as_deref()
                .is_some_and(|sha| r.fingerprint == fingerprint(r.kind, spec, sha))
        })
        .cloned()
        .collect();
    // Rule 4 reads `confirmed` alone: `draft_edit` puts a row in both lists, and the sticky
    // check removes a changed user-set row from `confirmed` precisely so it reads "proposed".
    let settled_by_user = |row: &str| d.confirmed.iter().any(|r| r == row);
    let state_of = |row: &str| -> (FieldState, Option<String>) {
        if !present(spec, row) {
            return if REQUIRED.contains(&row) {
                (FieldState::Missing, None)
            } else {
                (FieldState::Optional, None)
            };
        }
        if row == "repo"
            && let Some(sha) = &base_sha
        {
            let tree = if dirty == 0 {
                "clean tree".to_owned()
            } else {
                format!(
                    "{dirty} uncommitted file{}",
                    if dirty == 1 { "" } else { "s" }
                )
            };
            return (
                FieldState::Verified,
                Some(format!("{} · {tree}", &sha[..sha.len().min(8)])),
            );
        }
        if let Some(kind) = kind_of_row(row)
            && let Some(r) = probes.iter().find(|r| r.kind == kind && r.ok)
        {
            return (FieldState::Verified, Some(evidence(r)));
        }
        if settled_by_user(row) {
            return (FieldState::Confirmed, None);
        }
        let is_default = match row {
            "budget" => spec.budget == BudgetSpec::default(),
            "parallel" => spec.parallel == 1,
            "models" => spec.models == ModelsSpec::default(),
            _ => false,
        };
        if is_default {
            (FieldState::Default, None)
        } else {
            (FieldState::Proposed, None)
        }
    };
    let fields: Vec<DraftField> = MAIN_ROWS
        .iter()
        .chain(ADVANCED_ROWS.iter())
        .map(|row| {
            let (state, evidence) = state_of(row);
            DraftField {
                name: (*row).to_owned(),
                value: value(spec, row),
                state,
                evidence,
            }
        })
        .collect();
    let settled = fields
        .iter()
        .take(MAIN_ROWS.len())
        .filter(|f| !matches!(f.state, FieldState::Proposed | FieldState::Missing))
        .count() as u32;
    let mut blocking = Vec::new();
    for f in fields.iter().take(MAIN_ROWS.len()) {
        let row = f.name.as_str();
        match f.state {
            FieldState::Missing if REQUIRED.contains(&row) => {
                blocking.push(format!("{} is missing", label(row)));
            }
            FieldState::Proposed if REQUIRED.contains(&row) || row == "window" => {
                blocking.push(format!("{} needs your OK", label(row)));
            }
            _ => {}
        }
    }
    if let Err(e) = validate_spec(spec) {
        blocking.push(e);
    }
    // What `create` refuses at Start (custody R-8: an absolute script path outside the drafts
    // dir, a relative one that is no repo file) blocks Start here, with the same words.
    if base_sha.is_some()
        && let Err(e) = crate::custody::check_commands(spec, repo, &drafts_dir(root, &spec.slug))
    {
        blocking.push(e.to_string());
    }
    DraftView {
        draft_id: spec.slug.clone(),
        origin: d.origin,
        fields,
        settled,
        total: MAIN_ROWS.len() as u32,
        ready: blocking.is_empty(),
        blocking,
        dirty: dirty > 0,
        partial: d.partial,
        version: d.version,
        updated_at: d.updated_at.clone(),
        probes,
        started,
        spec: d.spec,
    }
}

/// `autoresearch.drafts`: every readable draft under `<root>/drafts/` without a
/// study, newest `updated_at` first. A directory without a readable `spec.json`
/// (half-written, or a draft older than the format) is skipped, not an error.
pub fn drafts(root: &Path, started: &[String]) -> Vec<DraftSummary> {
    let Ok(entries) = std::fs::read_dir(root.join("drafts")) else {
        return Vec::new();
    };
    let mut out: Vec<DraftSummary> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if started.contains(&name) {
                return None;
            }
            let d = read_draft(root, &name).ok()?;
            let v = view_of(root, d, false);
            Some(DraftSummary {
                draft_id: v.draft_id,
                goal: v.spec.goal,
                repo: v.spec.repo,
                origin: v.origin,
                settled: v.settled,
                total: v.total,
                ready: v.ready,
                updated_at: v.updated_at,
            })
        })
        .collect();
    out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    out
}

// ───────────────────────────── edit / confirm / discard (§6.3) ─────────────────────────────

/// Rows whose leaves changed between `old` and `new` and that the user set in
/// the panel (§5 sticky): they lose `confirmed` and are named in the tool text.
pub fn sticky_rows(old: &Draft, new: &StudySpec) -> Vec<String> {
    old.user_set
        .iter()
        .filter(|row| row_json(&old.spec, row) != row_json(new, row))
        .cloned()
        .collect()
}

/// Rows of `old.confirmed` whose value did not change — they stay confirmed
/// across a re-draft (a panel OK the agent never saw must not be lost).
pub fn still_confirmed(old: &Draft, new: &StudySpec) -> Vec<String> {
    old.confirmed
        .iter()
        .filter(|row| row_json(&old.spec, row) == row_json(new, row))
        .cloned()
        .collect()
}

/// The sticky note for the tool result (§4.6).
pub fn sticky_note(row: &str) -> String {
    format!("{row} was set by the user in the panel; your change needs their OK")
}

fn push_unique(v: &mut Vec<String>, row: &str) {
    if !v.iter().any(|r| r == row) {
        v.push(row.to_owned());
    }
}

/// Write a draft back after a panel change: version bumped, `updated_at` stamped.
fn save(root: &Path, d: &mut Draft) -> Result<()> {
    d.version += 1;
    d.updated_at = crate::store::now();
    std::fs::write(
        draft_spec_path(root, &d.spec.slug),
        serde_json::to_vec_pretty(d)?,
    )?;
    Ok(())
}

fn parse_bool(v: &str) -> std::result::Result<bool, String> {
    match v.trim().to_ascii_lowercase().as_str() {
        "true" | "on" | "yes" | "1" => Ok(true),
        "false" | "off" | "no" | "0" => Ok(false),
        other => Err(format!("`{other}` is not true|false|on|off|yes|no")),
    }
}

fn parse_num<T: std::str::FromStr>(v: &str) -> std::result::Result<T, String> {
    v.trim()
        .parse()
        .map_err(|_| format!("`{}` is not a number", v.trim()))
}

fn parse_list(v: &str) -> Vec<String> {
    v.split([',', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

fn opt(v: &str) -> Option<String> {
    non_empty(v).then(|| v.trim().to_owned())
}

fn humantime(v: &str) -> std::result::Result<String, String> {
    humantime::parse_duration(v.trim()).map_err(|e| format!("`{}`: {e}", v.trim()))?;
    Ok(v.trim().to_owned())
}

/// Set one leaf from the panel's text (§4.3 grammar).
pub fn set_leaf(spec: &mut StudySpec, leaf: &str, v: &str) -> std::result::Result<(), String> {
    match leaf {
        "goal" => spec.goal = v.trim().to_owned(),
        "repo" => spec.repo = v.trim().to_owned(),
        "base" => spec.base = opt(v).unwrap_or_else(|| "HEAD".into()),
        "metric.name" => spec.metric.name = v.trim().to_owned(),
        "metric.unit" => spec.metric.unit = v.trim().to_owned(),
        "metric.direction" => {
            spec.metric.direction = match v.trim().to_ascii_lowercase().as_str() {
                "lower" => Direction::Lower,
                "higher" => Direction::Higher,
                other => return Err(format!("`{other}` is not lower|higher")),
            }
        }
        "metric.min_effect" => {
            crate::types::Effect::parse(v)?;
            spec.metric.min_effect = v.trim().to_owned();
        }
        "bench.command" => spec.bench.command = v.trim().to_owned(),
        "bench.timeout" => spec.bench.timeout = humantime(v)?,
        "bench.repeats" => spec.bench.repeats = parse_num(v)?,
        "bench.exclusive" => spec.bench.exclusive = parse_bool(v)?,
        "bench.smoke" => spec.bench.smoke = opt(v),
        "checks.command" => match opt(v) {
            None => spec.checks = None,
            Some(c) => spec.checks.get_or_insert_with(Default::default).command = c,
        },
        "checks.timeout" => {
            spec.checks.get_or_insert_with(Default::default).timeout = humantime(v)?;
        }
        "holdout.command" => match opt(v) {
            None => spec.holdout = None,
            Some(c) => {
                let h = spec.holdout.get_or_insert_with(Default::default);
                h.command = c;
                if h.every == 0 {
                    h.every = 5;
                }
            }
        },
        "holdout.every" => spec.holdout.get_or_insert_with(Default::default).every = parse_num(v)?,
        "holdout.repeats" => {
            spec.holdout.get_or_insert_with(Default::default).repeats = match opt(v) {
                None => None,
                Some(r) => Some(parse_num(&r)?),
            }
        }
        "scope" => spec.scope = parse_list(v),
        "off_limits" => spec.off_limits = parse_list(v),
        "evaluator_paths" => spec.evaluator_paths = parse_list(v),
        "budget.usd" => spec.budget.usd = parse_num(v)?,
        "budget.per_experiment_usd" => spec.budget.per_experiment_usd = parse_num(v)?,
        "budget.hours" => spec.budget.hours = parse_num(v)?,
        "budget.experiments" => spec.budget.experiments = parse_num(v)?,
        "window" => spec.window = opt(v),
        "warm" => spec.warm = opt(v),
        "parallel" => spec.parallel = parse_num(v)?,
        "secondary_guards" => {
            spec.secondary_guards = v
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(|l| {
                    let t: Vec<&str> = l.split_whitespace().collect();
                    let (name, max, dir) = match t.as_slice() {
                        [n, m] => (n, m, Direction::Lower),
                        [n, m, "lower"] => (n, m, Direction::Lower),
                        [n, m, "higher"] => (n, m, Direction::Higher),
                        _ => {
                            return Err(format!(
                                "guard `{l}`: expected `name max_regression [lower|higher]`"
                            ));
                        }
                    };
                    crate::types::Effect::parse(max).map_err(|e| format!("guard `{l}`: {e}"))?;
                    Ok(Guard {
                        name: (*name).to_owned(),
                        max_regression: (*max).to_owned(),
                        direction: dir,
                    })
                })
                .collect::<std::result::Result<_, _>>()?;
        }
        "models.executor" => spec.models.executor = opt(v),
        "models.review" => spec.models.review = opt(v),
        "models.advisor" => spec.models.advisor = opt(v),
        // R1: the panel's "Start with my uncommitted changes" (= `tools::mark_include_dirty`);
        // not a row, so `edit` marks nothing user-set or confirmed.
        "include_dirty" => spec.include_dirty = parse_bool(v)?,
        other => return Err(format!("unknown field `{other}`")),
    }
    Ok(())
}

/// `autoresearch.draft_edit`: set a leaf, mark its row user-set and confirmed, save.
pub fn edit(root: &Path, draft_id: &str, leaf: &str, value: &str) -> Result<()> {
    let mut d = read_draft(root, draft_id)?;
    set_leaf(&mut d.spec, leaf, value).map_err(|e| Error::refused(format!("{leaf}: {e}")))?;
    validate_partial(&d.spec).map_err(Error::refused)?;
    if leaf != "include_dirty" {
        let row = row_of(leaf);
        push_unique(&mut d.user_set, row);
        push_unique(&mut d.confirmed, row);
    }
    save(root, &mut d)
}

/// `autoresearch.draft_confirm`: the user OK'd a row; refused for a missing one.
pub fn confirm(root: &Path, draft_id: &str, row: &str) -> Result<()> {
    if !MAIN_ROWS.contains(&row) && !ADVANCED_ROWS.contains(&row) {
        return Err(Error::refused(format!("unknown row `{row}`")));
    }
    let mut d = read_draft(root, draft_id)?;
    if !present(&d.spec, row) {
        return Err(Error::refused(format!(
            "{} is missing; nothing to confirm",
            label(row)
        )));
    }
    push_unique(&mut d.confirmed, row);
    save(root, &mut d)
}

/// `autoresearch.discard_draft`: the drafts dir goes; later Starts fail in `read_draft`.
pub fn discard(root: &Path, draft_id: &str) -> Result<()> {
    let dir = drafts_dir(root, draft_id);
    if !dir.is_dir() {
        return Err(Error::refused(format!("no draft `{draft_id}`")));
    }
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}

// ───────────────────────────── the probe (§6.2) ─────────────────────────────

/// Removes the scratch worktree and `probe/` however the probe ends.
struct Cleanup {
    repo: PathBuf,
    wt: PathBuf,
    probe: PathBuf,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = git::worktree_remove(&self.repo, &self.wt);
        let _ = std::fs::remove_dir_all(&self.probe);
    }
}

fn command_of(spec: &StudySpec, what: ProbeKind) -> Option<(String, String)> {
    match what {
        ProbeKind::Bench => Some((spec.bench.command.clone(), spec.bench.timeout.clone())),
        ProbeKind::Check => spec
            .checks
            .as_ref()
            .map(|c| (c.command.clone(), c.timeout.clone())),
        ProbeKind::Holdout => spec
            .holdout
            .as_ref()
            .map(|h| (h.command.clone(), spec.bench.timeout.clone())),
    }
    .filter(|(c, _)| non_empty(c))
}

/// Measure a draft's bench / check / holdout in a detached scratch worktree of
/// the repo at `base` (never the user's checkout), merge the result into
/// `probe.json` and return it.
pub async fn probe(
    root: &Path,
    cfg: &AutoresearchConfig,
    draft_id: &str,
    what: ProbeKind,
    repeats: Option<u8>,
    cancel: &CancellationToken,
) -> Result<ProbeResult> {
    let d = read_draft(root, draft_id)?;
    let spec = d.spec;
    let kind_word = match what {
        ProbeKind::Bench => "bench",
        ProbeKind::Check => "checks",
        ProbeKind::Holdout => "holdout",
    };
    let Some((command, timeout)) = command_of(&spec, what) else {
        return Err(Error::refused(format!(
            "no `{kind_word}.command` in the draft"
        )));
    };
    let timeout = runner::parse_duration(&timeout)?;
    let repo = PathBuf::from(spec.repo.trim());
    let base_sha = git::resolve_base(&repo, &spec.base, true)?;
    let dir = drafts_dir(root, draft_id);
    // The probe measures what the study will run: a script the custody refuses is refused here.
    crate::custody::check_commands(&spec, &repo, &dir)?;
    // ponytail: one lock for every probe — two calls in one agent turn (`asyncio.gather`) raced
    // on `probe/wt` and both failed; bench probes hold the exclusive slot anyway. Per-draft
    // locks if concurrent setups ever matter.
    static PROBES: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _one_at_a_time = PROBES.lock().await;
    let probe_dir = dir.join("probe");
    let wt = probe_dir.join("wt");
    let runs = probe_dir.join("runs");
    git::worktree_remove(&repo, &wt)?;
    let _ = std::fs::remove_dir_all(&probe_dir);
    git::worktree_add_detached(&repo, &wt, &base_sha)?;
    // Armed before anything can fail: ok, error and cancel all pass through `Drop`.
    let _cleanup = Cleanup {
        repo: repo.clone(),
        wt: wt.clone(),
        probe: probe_dir.clone(),
    };
    // In a study the custody copy makes a bare repo script (`bench.sh`) resolvable; the
    // probe has no custody, so a repo script that exists in the scratch worktree is named by
    // its path there. Draft scripts (`<root>/drafts/<id>/x.sh`) resolve through the runner.
    let command = repo_scripts(&command, &wt, &dir);
    let started = Instant::now();
    let req = |k: u32| SampleRequest {
        slug: draft_id,
        n: 0,
        k,
        tree_sha: &base_sha,
        role: SampleRole::Baseline,
        worktree: &wt,
        evaluator: &dir,
        runs_dir: &runs,
    };
    let mut r = ProbeResult {
        kind: what,
        ok: true,
        unit: spec.metric.unit.clone(),
        fingerprint: fingerprint(what, &spec, &base_sha),
        ..ProbeResult::default()
    };
    let finish = |mut r: ProbeResult| -> Result<ProbeResult> {
        r.duration_ms = started.elapsed().as_millis() as u64;
        r.at = crate::store::now();
        if r.values.len() >= 2 {
            let m = r.values.iter().sum::<f64>() / r.values.len() as f64;
            let sigma = crate::gate::pooled_sigma(std::slice::from_ref(&r.values));
            r.noise_pct = Some(if m != 0.0 {
                100.0 * sigma / m.abs()
            } else {
                0.0
            });
        }
        if !r.values.is_empty() {
            let mut sorted = r.values.clone();
            sorted.sort_by(f64::total_cmp);
            let n = sorted.len();
            r.median = Some(if n % 2 == 1 {
                sorted[n / 2]
            } else {
                (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
            });
        }
        let mut file = ProbeFile::read(&dir);
        file.set(r.clone());
        file.write(&dir)?;
        Ok(r)
    };
    if let Some(w) = spec.warm.as_deref().filter(|w| non_empty(w)) {
        let (exit, tail, _) = runner::command(w, timeout, req(0), cancel).await?;
        if exit != Some(0) {
            r.ok = false;
            r.exit = exit;
            r.log_tail = format!("warm `{}` failed:\n{tail}", w.trim());
            return finish(r);
        }
    }
    match what {
        ProbeKind::Check => {
            let (exit, tail, _) = runner::command(&command, timeout, req(1), cancel).await?;
            r.samples = 1;
            r.exit = exit;
            r.ok = exit == Some(0);
            if !r.ok {
                r.log_tail = tail;
            }
        }
        ProbeKind::Bench | ProbeKind::Holdout => {
            let n = repeats
                .or(match what {
                    ProbeKind::Holdout => spec.holdout.as_ref().and_then(|h| h.repeats),
                    _ => None,
                })
                .unwrap_or(spec.bench.repeats)
                .clamp(3, 10);
            let mut measured = spec.clone();
            measured.bench.command = command;
            for k in 1..=u32::from(n) {
                let s = runner::sample(&measured, &cfg.bench, req(k), cancel).await?;
                r.samples += 1;
                r.exit = s.exit;
                if cancel.is_cancelled() {
                    return Err(Error::refused("probe cancelled"));
                }
                match s.value {
                    Some(v) => r.values.push(v),
                    None => {
                        r.ok = false;
                        let log = s
                            .log_path
                            .as_deref()
                            .and_then(|p| Path::new(p).file_name().map(|f| runs.join(f)));
                        r.log_tail = format!(
                            "sample {k}: {}\n{}",
                            s.failure.unwrap_or_default(),
                            log.map(|l| runner::tail(&l)).unwrap_or_default()
                        );
                        break;
                    }
                }
            }
        }
    }
    finish(r)
}

/// The one-line tool text (§6.2 step 7).
pub fn render_probe(r: &ProbeResult, z: f64) -> String {
    let kind = match r.kind {
        ProbeKind::Bench => "bench",
        ProbeKind::Check => "check",
        ProbeKind::Holdout => "holdout",
    };
    let took = duration(r.duration_ms);
    if !r.ok {
        let exit = match r.exit {
            Some(e) => format!("exit {e}"),
            None => "killed".to_owned(),
        };
        return format!(
            "{kind}: failed ({exit}) in {took}\n{}\nFix the cause and probe again; the row stays unverified until a probe passes.",
            r.log_tail.trim_end()
        );
    }
    match r.kind {
        ProbeKind::Check => format!("{kind}: passed in {took}"),
        _ => {
            let noise = r.noise_pct.unwrap_or(0.0);
            format!(
                "{kind}: {} samples · median {} {} · noise ±{noise:.1}% · smallest detectable ≈ {:.1}%",
                r.samples,
                r.median.map(num).unwrap_or_default(),
                r.unit.trim(),
                z * noise
            )
        }
    }
}

/// Every whitespace token that names a repo file (relative, `./x` too) which is
/// not shadowed by a draft script becomes the worktree copy's quoted absolute path.
fn repo_scripts(command: &str, wt: &Path, drafts: &Path) -> String {
    command
        .split(' ')
        .map(|tok| match crate::custody::command_path(tok, drafts) {
            Some(crate::custody::CommandPath::Repo(rel))
                if !drafts.join(rel).is_file() && wt.join(rel).is_file() =>
            {
                format!(
                    "'{}'",
                    wt.join(rel).to_string_lossy().replace('\'', "'\\''")
                )
            }
            _ => tok.to_owned(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// ───────────────────────────── the tool ─────────────────────────────

/// `autoresearch_probe` arguments (151 D4).
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ProbeArgs {
    /// The draft id (the spec's slug).
    pub draft_id: String,
    /// `bench | check | holdout`.
    #[serde(default)]
    pub what: ProbeKind,
    /// Bench / holdout samples; default the spec's `bench.repeats`, clamped 3..=10.
    #[serde(default)]
    pub repeats: Option<u8>,
}

/// `autoresearch_probe` (151 D4): measure a draft's bench / check / holdout in
/// a scratch detached worktree of the repo at `base`; the result feeds the
/// panel's "Verified" badge. A root tool like `autoresearch_draft`; refused
/// inside a study session. A long run is promoted to a job by the harness and
/// dies with `cx.cancel`.
pub struct ProbeTool(pub Arc<Seams>);

#[async_trait::async_trait]
impl Tool for ProbeTool {
    const NAME: &'static str = "autoresearch_probe";
    const RISK: Risk = Risk::Mutating;
    const RETURNS: &'static str = "dict";
    type Args = ProbeArgs;

    fn description(&self) -> String {
        "Measure a drafted autoresearch study before it starts: `what: bench` runs the draft's benchmark `repeats` \
         times (3–10) in a scratch worktree of the repo at `base` and reports samples, median and noise; `check` runs \
         the correctness check once; `holdout` runs the holdout. The result is the only thing that marks a draft row \
         Verified in the Autoresearch tab — call it after writing or changing the bench, check or holdout, and never \
         state a measurement it did not produce. Never touches the user's checkout."
            .into()
    }

    async fn call(&self, a: ProbeArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        crate::tools::refuse_study_sessions(cx.session, Self::NAME).await?;
        let cfg = crate::tools::live_cfg().await?;
        let root = cfg.root(&self.0.data_dir);
        let r = probe(
            &root,
            &cfg,
            a.draft_id.trim(),
            a.what,
            a.repeats,
            &cx.cancel,
        )
        .await
        .map_err(|e| anyhow::anyhow!("autoresearch_probe: {e}"))?;
        Ok(crate::tools::structured(render_probe(&r, cfg.gate.z), r))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::write_draft;
    use crate::types::{BenchSpec, CommandSpec, HoldoutSpec, MetricSpec};
    use runtime::SessionId;

    fn sh(cwd: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A toy-sort repo with one commit on `main`.
    fn toy_repo(tmp: &Path) -> PathBuf {
        let repo = tmp.join("repo");
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/toy-sort");
        std::fs::create_dir_all(repo.join("fixtures")).unwrap();
        for f in ["bench.sh", "main.py", "sorter.py", "test.sh", "holdout.sh"] {
            std::fs::copy(src.join(f), repo.join(f)).unwrap();
        }
        for f in ["input.txt", "expected.txt"] {
            std::fs::copy(src.join("fixtures").join(f), repo.join("fixtures").join(f)).unwrap();
        }
        sh(tmp, &["init", "-q", "-b", "main", "repo"]);
        sh(&repo, &["add", "-A"]);
        sh(
            &repo,
            &[
                "-c",
                "user.name=u",
                "-c",
                "user.email=u@x",
                "commit",
                "-q",
                "-m",
                "base",
            ],
        );
        repo
    }

    fn spec(repo: &Path) -> StudySpec {
        StudySpec {
            slug: "toy".into(),
            goal: "sort faster".into(),
            repo: repo.display().to_string(),
            metric: MetricSpec {
                name: "time_ms".into(),
                unit: "ms".into(),
                direction: Direction::Lower,
                min_effect: "0.5%".into(),
            },
            bench: BenchSpec {
                command: "bench.sh".into(),
                exclusive: false,
                ..BenchSpec::default()
            },
            checks: Some(CommandSpec {
                command: "./test.sh".into(),
                ..CommandSpec::default()
            }),
            scope: vec!["*.py".into()],
            ..StudySpec::default()
        }
    }

    fn cfg() -> AutoresearchConfig {
        let mut c = AutoresearchConfig::default();
        c.bench.max_load = f64::INFINITY;
        c.bench.wait = "1s".into();
        c
    }

    fn state(v: &DraftView, row: &str) -> FieldState {
        v.fields.iter().find(|f| f.name == row).unwrap().state
    }

    #[test]
    fn field_states_follow_the_rules() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = toy_repo(tmp.path());
        let root = tmp.path().join("root");
        let s = spec(&repo);
        write_draft(&root, &s, SessionId::new(), true, vec!["metric".into()]).unwrap();
        let v = view(&root, "toy", false).unwrap();
        assert_eq!(state(&v, "goal"), FieldState::Proposed);
        assert_eq!(state(&v, "repo"), FieldState::Verified, "{v:?}");
        let repo_row = v.fields.iter().find(|f| f.name == "repo").unwrap();
        assert!(
            repo_row
                .evidence
                .as_deref()
                .unwrap()
                .ends_with("· clean tree")
        );
        assert_eq!(state(&v, "metric"), FieldState::Confirmed);
        assert_eq!(state(&v, "bench"), FieldState::Proposed);
        assert_eq!(state(&v, "checks"), FieldState::Proposed);
        assert_eq!(state(&v, "holdout"), FieldState::Optional);
        assert_eq!(state(&v, "scope"), FieldState::Proposed);
        assert_eq!(state(&v, "budget"), FieldState::Default);
        assert_eq!(state(&v, "window"), FieldState::Optional);
        assert_eq!(state(&v, "parallel"), FieldState::Default);
        assert_eq!(state(&v, "models"), FieldState::Default);
        assert_eq!(state(&v, "warm"), FieldState::Optional);
        assert_eq!((v.settled, v.total, v.ready, v.dirty), (5, 9, false, false));
        assert_eq!(
            v.blocking,
            vec![
                "Goal needs your OK",
                "Benchmark needs your OK",
                "Check needs your OK",
                "Scope needs your OK",
            ]
        );
        assert_eq!(v.fields[2].value, "time_ms · ms · lower · keep ≥ 0.5%");
        assert_eq!(
            v.fields[7].value,
            "$20 · 120 experiments · 10 h · $1.5 per experiment"
        );
        assert_eq!(v.fields.len(), 14);
        assert!(v.partial && v.version == 1);

        // Missing rows block; a non-default budget is proposed; a dirty tree is reported.
        let mut bare = s.clone();
        bare.goal.clear();
        bare.checks = None;
        bare.budget.usd = 5.0;
        bare.window = Some("idle.py".into());
        write_draft(
            &root,
            &bare,
            SessionId::new(),
            true,
            vec!["bench".into(), "scope".into()],
        )
        .unwrap();
        std::fs::write(repo.join("scratch.txt"), "x").unwrap();
        let v = view(&root, "toy", false).unwrap();
        assert_eq!(state(&v, "goal"), FieldState::Missing);
        assert_eq!(state(&v, "checks"), FieldState::Missing);
        assert_eq!(state(&v, "budget"), FieldState::Proposed);
        assert_eq!(state(&v, "window"), FieldState::Proposed);
        assert!(v.dirty);
        assert!(
            v.fields[1]
                .evidence
                .as_deref()
                .unwrap()
                .ends_with("· 1 uncommitted file")
        );
        assert_eq!(v.blocking[0], "Goal is missing");
        assert!(v.blocking.contains(&"Check is missing".to_owned()));
        assert!(v.blocking.contains(&"When needs your OK".to_owned()));
        assert!(
            v.blocking.last().unwrap().starts_with("goal is empty"),
            "{:?}",
            v.blocking
        );
        assert_eq!(v.version, 2);

        // Everything confirmed and valid → ready; a probe makes bench verified.
        let all: Vec<String> = MAIN_ROWS.iter().map(|r| (*r).to_owned()).collect();
        write_draft(&root, &s, SessionId::new(), false, all).unwrap();
        let v = view(&root, "toy", true).unwrap();
        assert!(v.ready, "{:?}", v.blocking);
        assert!(v.started);
        assert_eq!(v.settled, 9);
        // A repo that is not git: proposed, not verified; no probes match.
        let mut nogit = s.clone();
        nogit.repo = tmp.path().join("plain").display().to_string();
        std::fs::create_dir_all(&nogit.repo).unwrap();
        write_draft(&root, &nogit, SessionId::new(), true, vec![]).unwrap();
        let v = view(&root, "toy", false).unwrap();
        assert_eq!(state(&v, "repo"), FieldState::Proposed);
    }

    #[test]
    fn user_set_rows_are_sticky() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        let s = spec(Path::new("/x/repo"));
        write_draft(&root, &s, SessionId::new(), true, vec!["goal".into()]).unwrap();
        edit(&root, "toy", "budget.usd", "7").unwrap();
        confirm(&root, "toy", "scope").unwrap();
        let d = read_draft(&root, "toy").unwrap();
        assert_eq!(d.user_set, vec!["budget"]);
        assert_eq!(d.confirmed, vec!["goal", "budget", "scope"]);
        assert_eq!((d.spec.budget.usd, d.version), (7.0, 3));
        // R1: `include_dirty` is the panel's third Start button, not a row — the flag flips,
        // the token stays, nothing becomes user-set or confirmed, `settled` is unchanged.
        let settled_before = view_of(&root, read_draft(&root, "toy").unwrap(), false).settled;
        edit(&root, "toy", "include_dirty", "true").unwrap();
        let d2 = read_draft(&root, "toy").unwrap();
        assert!(d2.spec.include_dirty);
        assert_eq!(
            (&d2.token, &d2.user_set, &d2.confirmed),
            (&d.token, &d.user_set, &d.confirmed)
        );
        assert_eq!(view_of(&root, d2, false).settled, settled_before);
        edit(&root, "toy", "include_dirty", "no").unwrap();
        assert!(!read_draft(&root, "toy").unwrap().spec.include_dirty);
        // A re-draft that changes the user-set budget: applied, but the row drops to proposed
        // and is named; the untouched panel OK on scope survives; goal (changed) does not.
        let mut again = s.clone();
        again.budget.usd = 9.0;
        again.goal = "sort much faster".into();
        let (_, sticky) = write_draft(&root, &again, SessionId::new(), true, vec![]).unwrap();
        assert_eq!(sticky, vec!["budget"]);
        let d = read_draft(&root, "toy").unwrap();
        assert_eq!(d.spec.budget.usd, 9.0);
        assert_eq!(d.user_set, vec!["budget"]);
        assert_eq!(d.confirmed, vec!["scope"]);
        let v = view_of(&root, d, false);
        assert_eq!(state(&v, "budget"), FieldState::Proposed);
        assert_eq!(state(&v, "scope"), FieldState::Confirmed);
        assert_eq!(state(&v, "goal"), FieldState::Proposed);
        assert_eq!(
            sticky_note("budget"),
            "budget was set by the user in the panel; your change needs their OK"
        );
        // A re-draft that changes nothing: no sticky note, the dropped row stays proposed
        // until the user OKs it again, the panel OK on scope is still there.
        let (_, sticky) = write_draft(&root, &again, SessionId::new(), true, vec![]).unwrap();
        assert!(sticky.is_empty());
        let d = read_draft(&root, "toy").unwrap();
        assert_eq!(d.confirmed, vec!["scope"]);
        // The user OKs it in the panel; the next unchanged re-draft keeps it.
        confirm(&root, "toy", "budget").unwrap();
        write_draft(&root, &again, SessionId::new(), true, vec![]).unwrap();
        assert_eq!(
            read_draft(&root, "toy").unwrap().confirmed,
            vec!["scope", "budget"]
        );
        // Confirming a missing row is refused; an unknown row too.
        let mut missing = s.clone();
        missing.holdout = None;
        write_draft(&root, &missing, SessionId::new(), true, vec![]).unwrap();
        let e = confirm(&root, "toy", "holdout").unwrap_err().to_string();
        assert_eq!(e, "Holdout is missing; nothing to confirm");
        assert!(confirm(&root, "toy", "nope").is_err());
    }

    #[test]
    fn edit_parses_every_leaf() {
        let mut s = StudySpec::default();
        let cases: [(&str, &str); 32] = [
            ("goal", " faster "),
            ("repo", "/r"),
            ("base", ""),
            ("metric.name", "t"),
            ("metric.unit", "ms"),
            ("metric.direction", "Higher"),
            ("metric.min_effect", "2%"),
            ("bench.command", "b.sh"),
            ("bench.timeout", "5m"),
            ("bench.repeats", "4"),
            ("bench.exclusive", "off"),
            ("bench.smoke", "make"),
            ("checks.command", "t.sh"),
            ("checks.timeout", "10m"),
            ("holdout.command", "h.sh"),
            ("holdout.every", "7"),
            ("holdout.repeats", "4"),
            ("scope", "a/**, b/**\nc"),
            ("off_limits", "t/**"),
            ("evaluator_paths", "fixtures/"),
            ("budget.usd", "3.5"),
            ("budget.per_experiment_usd", "0.5"),
            ("budget.hours", "2"),
            ("budget.experiments", "30"),
            ("window", "idle.py"),
            ("warm", "make deps"),
            ("parallel", "3"),
            ("secondary_guards", "alloc_mb 5%\nhit_rate 0.01 higher"),
            ("models.executor", "x/y"),
            ("models.review", "off"),
            ("models.advisor", ""),
            ("include_dirty", "true"),
        ];
        for (leaf, v) in cases {
            assert!(LEAVES.contains(&leaf), "{leaf}");
            set_leaf(&mut s, leaf, v).unwrap_or_else(|e| panic!("{leaf}: {e}"));
        }
        assert_eq!(s.goal, "faster");
        assert_eq!(s.base, "HEAD");
        assert_eq!(s.metric.direction, Direction::Higher);
        assert_eq!((s.bench.repeats, s.bench.exclusive), (4, false));
        assert_eq!(s.bench.smoke.as_deref(), Some("make"));
        assert_eq!(s.checks.as_ref().unwrap().timeout, "10m");
        let h = s.holdout.as_ref().unwrap();
        assert_eq!(
            (h.command.as_str(), h.every, h.repeats),
            ("h.sh", 7, Some(4))
        );
        assert_eq!(s.scope, vec!["a/**", "b/**", "c"]);
        assert_eq!(s.budget.usd, 3.5);
        assert_eq!(s.budget.experiments, 30);
        assert_eq!(s.parallel, 3);
        assert_eq!(s.secondary_guards.len(), 2);
        assert_eq!(s.secondary_guards[1].direction, Direction::Higher);
        assert_eq!(s.models.review.as_deref(), Some("off"));
        assert_eq!(s.models.advisor, None);
        assert!(s.include_dirty);
        // Emptying optional leaves clears them; bad values are refused with the leaf named.
        set_leaf(&mut s, "checks.command", "").unwrap();
        assert!(s.checks.is_none());
        set_leaf(&mut s, "holdout.command", " ").unwrap();
        assert!(s.holdout.is_none());
        set_leaf(&mut s, "window", "").unwrap();
        assert_eq!(s.window, None);
        assert!(set_leaf(&mut s, "metric.direction", "up").is_err());
        assert!(set_leaf(&mut s, "bench.timeout", "soon").is_err());
        assert!(set_leaf(&mut s, "bench.repeats", "many").is_err());
        assert!(set_leaf(&mut s, "secondary_guards", "alloc").is_err());
        assert!(set_leaf(&mut s, "nope", "x").is_err());
        assert_eq!(
            (
                row_of("base"),
                row_of("off_limits"),
                row_of("bench.timeout")
            ),
            ("repo", "scope", "bench")
        );
        // A fresh holdout from `holdout.command` alone gets `every: 5`.
        let mut t = StudySpec::default();
        set_leaf(&mut t, "holdout.command", "h.sh").unwrap();
        assert_eq!(t.holdout.unwrap().every, 5);
    }

    #[test]
    fn validate_partial_accepts_a_bare_spec() {
        let s = StudySpec {
            slug: "s".into(),
            repo: "/abs".into(),
            ..StudySpec::default()
        };
        validate_partial(&s).unwrap();
        assert!(validate_spec(&s).is_err());
        assert!(
            validate_partial(&StudySpec {
                repo: "rel".into(),
                ..s.clone()
            })
            .is_err()
        );
        assert!(
            validate_partial(&StudySpec {
                slug: "new".into(),
                ..s.clone()
            })
            .is_err()
        );
        assert!(
            validate_partial(&StudySpec {
                slug: "Bad".into(),
                ..s
            })
            .is_err()
        );
    }

    #[tokio::test]
    async fn probe_runs_in_a_detached_scratch_worktree_and_cleans_up() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = toy_repo(tmp.path());
        let root = tmp.path().join("root");
        let s = spec(&repo);
        write_draft(&root, &s, SessionId::new(), true, vec![]).unwrap();
        let before = std::process::Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&repo)
            .output()
            .unwrap();
        let cancel = CancellationToken::new();
        let r = probe(&root, &cfg(), "toy", ProbeKind::Bench, Some(3), &cancel)
            .await
            .unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!((r.samples, r.values.len()), (3, 3));
        assert!(r.median.is_some() && r.noise_pct.is_some());
        assert_eq!(r.unit, "ms");
        let probe_dir = drafts_dir(&root, "toy").join("probe");
        assert!(!probe_dir.exists(), "probe/ removed after ok");
        let text = render_probe(&r, 2.0);
        assert!(
            text.starts_with("bench: 3 samples · median ")
                && text.contains("· noise ±")
                && text.contains("smallest detectable ≈"),
            "{text}"
        );
        // The view now shows bench Verified with the result's evidence.
        let v = view(&root, "toy", false).unwrap();
        assert_eq!(state(&v, "bench"), FieldState::Verified, "{v:?}");
        assert!(
            v.fields[3]
                .evidence
                .as_deref()
                .unwrap()
                .starts_with("3 runs · ")
        );
        assert_eq!(v.probes.len(), 1);
        // The check passes too.
        let r = probe(&root, &cfg(), "toy", ProbeKind::Check, None, &cancel)
            .await
            .unwrap();
        assert!(r.ok, "{}", r.log_tail);
        assert!(render_probe(&r, 2.0).starts_with("check: passed in "));
        assert_eq!(
            state(&view(&root, "toy", false).unwrap(), "checks"),
            FieldState::Verified
        );
        // A crashing command: ok false, exit, tail; probe/ gone as well.
        let mut crash = s.clone();
        crash.bench.command = "echo boom >&2; exit 3".into();
        write_draft(&root, &crash, SessionId::new(), true, vec![]).unwrap();
        let r = probe(&root, &cfg(), "toy", ProbeKind::Bench, None, &cancel)
            .await
            .unwrap();
        assert!(!r.ok);
        assert_eq!((r.exit, r.samples), (Some(3), 1));
        assert!(r.log_tail.contains("boom"), "{}", r.log_tail);
        assert!(render_probe(&r, 2.0).starts_with("bench: failed (exit 3) in "));
        assert!(!probe_dir.exists(), "probe/ removed after a crash");
        // A missing command is refused before anything is created.
        let e = probe(&root, &cfg(), "toy", ProbeKind::Holdout, None, &cancel)
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(e, "no `holdout.command` in the draft");
        // The user's checkout is untouched: same status, no stray worktrees, HEAD on main.
        let after = std::process::Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&repo)
            .output()
            .unwrap();
        assert_eq!(before.stdout, after.stdout);
        let wts = std::process::Command::new("git")
            .args(["worktree", "list"])
            .current_dir(&repo)
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&wts.stdout).lines().count(),
            1,
            "{}",
            String::from_utf8_lossy(&wts.stdout)
        );
        assert_eq!(git::branch(&repo).unwrap().as_deref(), Some("main"));
        // The fingerprint changes with the bench command.
        assert_ne!(
            fingerprint(ProbeKind::Bench, &s, "abc"),
            fingerprint(ProbeKind::Bench, &crash, "abc")
        );
        assert_ne!(
            fingerprint(ProbeKind::Bench, &s, "abc"),
            fingerprint(ProbeKind::Check, &s, "abc")
        );
    }

    /// Live proof Z (2026-10-09): the agent called two probes in one turn (`asyncio.gather`,
    /// two tool tasks); both raced on `probe/wt` and both failed. Probes now run one at a time.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_probes_of_one_draft_both_succeed() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = toy_repo(tmp.path());
        let root = tmp.path().join("root");
        write_draft(&root, &spec(&repo), SessionId::new(), true, vec![]).unwrap();
        let task = || {
            let root = root.clone();
            tokio::spawn(async move {
                probe(
                    &root,
                    &cfg(),
                    "toy",
                    ProbeKind::Check,
                    None,
                    &CancellationToken::new(),
                )
                .await
            })
        };
        let (a, b) = tokio::join!(task(), task());
        assert!(a.unwrap().unwrap().ok && b.unwrap().unwrap().ok);
        assert!(!drafts_dir(&root, "toy").join("probe").exists());
    }

    /// Live proof Z: a study with `holdout.command = <user checkout>/holdout.sh` was Started from
    /// a `ready` panel and failed at once in custody. The view blocks Start with custody's
    /// reason and the probe refuses to measure such a script.
    #[tokio::test]
    async fn scripts_outside_custody_block_start_and_the_probe() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = toy_repo(tmp.path());
        let root = tmp.path().join("root");
        let mut s = spec(&repo);
        s.bench.command = repo.join("bench.sh").display().to_string();
        write_draft(&root, &s, SessionId::new(), false, vec![]).unwrap();
        let v = view(&root, "toy", false).unwrap();
        assert!(!v.ready, "{v:?}");
        assert!(
            v.blocking
                .iter()
                .any(|b| b.starts_with("bench: `") && b.contains("is not a file in the repo")),
            "{:?}",
            v.blocking
        );
        let e = probe(
            &root,
            &cfg(),
            "toy",
            ProbeKind::Bench,
            None,
            &CancellationToken::new(),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(e.contains("is not a file in the repo"), "{e}");
        assert!(!drafts_dir(&root, "toy").join("probe").exists());
    }

    #[test]
    fn fingerprint_mismatch_drops_verified() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = toy_repo(tmp.path());
        let root = tmp.path().join("root");
        let s = spec(&repo);
        write_draft(&root, &s, SessionId::new(), true, vec![]).unwrap();
        let sha = git::resolve_base(&repo, "HEAD", true).unwrap();
        let dir = drafts_dir(&root, "toy");
        let mut file = ProbeFile::default();
        file.set(ProbeResult {
            kind: ProbeKind::Bench,
            ok: true,
            values: vec![1.0, 1.1, 0.9],
            median: Some(1.0),
            noise_pct: Some(1.6),
            unit: "ms".into(),
            samples: 3,
            duration_ms: 412,
            fingerprint: fingerprint(ProbeKind::Bench, &s, &sha),
            ..ProbeResult::default()
        });
        file.set(ProbeResult {
            kind: ProbeKind::Check,
            ok: true,
            samples: 1,
            duration_ms: 110_000,
            fingerprint: fingerprint(ProbeKind::Check, &s, &sha),
            ..ProbeResult::default()
        });
        file.write(&dir).unwrap();
        let v = view(&root, "toy", false).unwrap();
        assert_eq!(state(&v, "bench"), FieldState::Verified);
        assert_eq!(
            v.fields[3].evidence.as_deref(),
            Some("3 runs · 1 ms · ±1.6%")
        );
        assert_eq!(state(&v, "checks"), FieldState::Verified);
        assert_eq!(v.fields[4].evidence.as_deref(), Some("passed in 1 m 50 s"));
        assert_eq!(v.probes.len(), 2);
        // A changed bench command drops both to proposed (one fingerprint for every kind).
        let mut changed = s.clone();
        changed.bench.command = "other.sh".into();
        write_draft(&root, &changed, SessionId::new(), true, vec![]).unwrap();
        let v = view(&root, "toy", false).unwrap();
        assert_eq!(state(&v, "bench"), FieldState::Proposed);
        assert_eq!(state(&v, "checks"), FieldState::Proposed);
        assert!(v.probes.is_empty());
        // A failed probe is kept for the sparkline but never verifies.
        let mut failed = ProbeFile::default();
        failed.set(ProbeResult {
            kind: ProbeKind::Bench,
            ok: false,
            fingerprint: fingerprint(ProbeKind::Bench, &changed, &sha),
            ..ProbeResult::default()
        });
        failed.write(&dir).unwrap();
        let v = view(&root, "toy", false).unwrap();
        assert_eq!(state(&v, "bench"), FieldState::Proposed);
        assert_eq!(v.probes.len(), 1);
    }

    #[test]
    fn durations_and_values_read_well() {
        assert_eq!(duration(412), "412 ms");
        assert_eq!(duration(12_400), "12 s");
        assert_eq!(duration(110_000), "1 m 50 s");
        let mut s = spec(Path::new("/x"));
        s.holdout = Some(HoldoutSpec {
            command: "h.sh".into(),
            every: 5,
            repeats: Some(4),
        });
        s.off_limits = vec!["t/**".into()];
        s.parallel = 3;
        s.models.review = Some("off".into());
        assert_eq!(value(&s, "repo"), "/x · base HEAD");
        assert_eq!(value(&s, "bench"), "`bench.sh` · 3× · 600s each");
        assert_eq!(value(&s, "checks"), "`./test.sh` · 300s");
        assert_eq!(value(&s, "holdout"), "`h.sh` · every 5 keeps · 4×");
        assert_eq!(value(&s, "scope"), "*.py · off-limits t/**");
        assert_eq!(value(&s, "parallel"), "waves of 3");
        assert_eq!(value(&s, "models"), "review off");
        assert_eq!(value(&s, "window"), "");
        assert_eq!(value(&StudySpec::default(), "models"), "defaults");
    }
}
