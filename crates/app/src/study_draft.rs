//! The `autoresearch_draft` tool (140 §4.1, §12.4): the `autoresearch` skill
//! proposes a study as an OpenUI card — five plain lines, warnings, and four
//! buttons: *Start now*, *Start in idle window*, *Edit*, *Discard*. Same card
//! pattern as [`crate::routine_draft`], one difference that is the design's
//! invariant: **the press, not the model, starts the study.** The two Start
//! buttons run a `Mutation("autoresearch_start", {draft_id, token})` — the
//! `StartDraft` message `autoresearch.create {draft_id, start}` sends — with
//! the press token the draft file holds and the model never sees, and only on
//! success tell the chat `study draft <slug>: started`; *Edit* and *Discard*
//! are `@ToAssistant` only. The spec is written to `<autoresearch
//! root>/drafts/<slug>/spec.json` (beside the setup's scripts, never the user's
//! checkout) so the press can find it; nothing else exists until the press.
//! With the autoresearch plugin off (no actor) the tool refuses before writing.

use std::sync::Arc;

use autoresearch::tools::{IDLE_SENSOR, draft_spec_path, live_cfg, validate_spec, write_draft};
use autoresearch::{AutoresearchConfig, Direction, DraftArgs, StudySpec};
use tools::{Risk, Tool, ToolCx, ToolOutput};

use crate::actor::Core;
use crate::draft::lit;
use crate::tool::{AppArgs, Origin, commit_source};

/// App name every draft card is published under (a new version per call).
const APP_NAME: &str = "study-drafts";

/// `autoresearch_draft`: a proposed study as an approval card in chat.
pub struct StudyDraftTool {
    /// Shared with the actor.
    pub core: Arc<Core>,
}

/// The five plain lines of §4.1.
pub fn lines(spec: &StudySpec) -> [String; 5] {
    let repo = std::path::Path::new(&spec.repo)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| spec.repo.clone());
    let dir = match spec.metric.direction {
        Direction::Lower => "lower",
        Direction::Higher => "higher",
    };
    let min_effect = if spec.metric.min_effect.trim().is_empty() {
        String::new()
    } else {
        format!("; keep from {}", spec.metric.min_effect.trim())
    };
    let check = match &spec.checks {
        Some(c) if !c.command.trim().is_empty() => format!("check `{}`", c.command.trim()),
        _ => "no check".to_owned(),
    };
    let holdout = match &spec.holdout {
        Some(h) if !h.command.trim().is_empty() => {
            format!(
                "holdout `{}` every {} keeps",
                h.command.trim(),
                h.every.max(1)
            )
        }
        _ => "no holdout".to_owned(),
    };
    // R-19: each guard carries its own direction; the card shows it (read-only — Edit re-drafts).
    let guards = if spec.secondary_guards.is_empty() {
        String::new()
    } else {
        format!(
            " · guards {}",
            spec.secondary_guards
                .iter()
                .map(|g| {
                    format!(
                        "{} ≤ {} worse ({} is better)",
                        g.name.trim(),
                        g.max_regression.trim(),
                        match g.direction {
                            Direction::Lower => "lower",
                            Direction::Higher => "higher",
                        }
                    )
                })
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let list = |v: &[String], none: &str| {
        if v.is_empty() {
            none.to_owned()
        } else {
            v.join(", ")
        }
    };
    let b = &spec.budget;
    let when = match &spec.window {
        Some(w) if !w.trim().is_empty() => format!("starts in the idle window (`{}`)", w.trim()),
        _ => "starts now".to_owned(),
    };
    let waves = if spec.parallel > 1 {
        format!(" · waves of {}", spec.parallel)
    } else {
        String::new()
    };
    [
        format!(
            "Optimise `{}` ({dir} is better{min_effect}) of `{}` in {repo} (base {}{})",
            spec.metric.name.trim(),
            spec.bench.command.trim(),
            spec.base.trim(),
            if spec.include_dirty {
                " + your uncommitted changes"
            } else {
                ""
            }
        ),
        format!("{check} · {holdout}{guards}"),
        format!(
            "may edit {} · never keeps {} · evaluator files {}",
            list(&spec.scope, "anything (scope empty)"),
            list(&spec.off_limits, "nothing listed"),
            list(&spec.evaluator_paths, "none listed")
        ),
        format!(
            "up to {} experiments, {} h, ${} (${} per experiment) · {} baseline samples, {} per sample{waves}",
            b.experiments,
            trim_float(b.hours),
            trim_float(b.usd),
            trim_float(b.per_experiment_usd),
            spec.bench.repeats,
            spec.bench.timeout.trim()
        ),
        format!(
            "{when} · branch autoresearch/{} in its own worktree",
            spec.slug
        ),
    ]
}

/// The §4.1 warnings: no check, no holdout, wall-clock without `exclusive`, empty scope, budget over the threshold, review off.
pub fn warnings(spec: &StudySpec, cfg: &AutoresearchConfig) -> Vec<String> {
    let mut w = Vec::new();
    if spec
        .checks
        .as_ref()
        .is_none_or(|c| c.command.trim().is_empty())
    {
        w.push("No correctness check: only the metric and the reviewer stand between a gaming change and a keep.".into());
    }
    if spec
        .holdout
        .as_ref()
        .is_none_or(|h| h.command.trim().is_empty())
    {
        w.push("No holdout: overfitting to the benchmark's inputs will not be detected.".into());
    }
    if is_time_unit(&spec.metric.unit) && !spec.bench.exclusive {
        w.push(format!(
            "`{}` is a wall-clock metric measured without the bench slot (exclusive: false): builds and other benchmarks will add noise.",
            spec.metric.unit.trim()
        ));
    }
    if spec.scope.is_empty() {
        w.push("Scope is empty: edits anywhere in the repo are expected; deviations cannot be told from intent.".into());
    }
    if spec.budget.usd > cfg.confirm_budget_usd {
        w.push(format!(
            "Budget ${} is above the confirmation threshold (${}).",
            trim_float(spec.budget.usd),
            trim_float(cfg.confirm_budget_usd)
        ));
    }
    if spec.models.review.as_deref().map(str::trim) == Some("off") {
        w.push("Review is off: keeps are not checked by a second model.".into());
    }
    w
}

fn is_time_unit(unit: &str) -> bool {
    matches!(
        unit.trim().to_ascii_lowercase().as_str(),
        "ns" | "us"
            | "µs"
            | "μs"
            | "ms"
            | "s"
            | "sec"
            | "secs"
            | "seconds"
            | "min"
            | "minutes"
            | "ops/s"
            | "op/s"
            | "qps"
            | "rps"
            | "req/s"
            | "it/s"
            | "tok/s"
            | "tokens/s"
            | "mb/s"
            | "gb/s"
    )
}

fn trim_float(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// The card's OpenUI source. The Start buttons run the `autoresearch_start`
/// mutation with the draft's press `token` first and tell the chat only when
/// it succeeded (`@Run` stops the action on failure; the error shows under the
/// buttons).
pub fn source(spec: &StudySpec, warnings: &[String], token: &str) -> String {
    let slug = spec.slug.as_str();
    let [l1, l2, l3, l4, l5] = lines(spec);
    let sensor = spec
        .window
        .as_deref()
        .map(str::trim)
        .filter(|w| !w.is_empty())
        .unwrap_or(IDLE_SENSOR);
    let mut out = String::new();
    let mut members = vec!["head", "l1", "l2", "l3", "l4", "l5"];
    if !warnings.is_empty() {
        members.push("warn");
    }
    members.extend(["buttons", "result"]);
    out.push_str(&format!("root = Card([{}])\n", members.join(", ")));
    out.push_str(&format!(
        "head = CardHeader({}, {})\n",
        lit(slug),
        lit(&format!("study draft {slug}"))
    ));
    out.push_str(&format!(
        "l1 = TextContent({})\n",
        lit(&format!("**{l1}**"))
    ));
    for (name, text) in [("l2", &l2), ("l3", &l3), ("l4", &l4), ("l5", &l5)] {
        out.push_str(&format!("{name} = TextContent({}, \"small\")\n", lit(text)));
    }
    if !warnings.is_empty() {
        let text = warnings
            .iter()
            .map(|w| format!("⚠ {w}"))
            .collect::<Vec<_>>()
            .join("\n\n");
        out.push_str(&format!(
            "warn = Callout(\"warning\", {}, {})\n",
            lit(if warnings.len() == 1 {
                "1 warning"
            } else {
                "Warnings"
            }),
            lit(&text)
        ));
    }
    out.push_str(&format!(
        "start = Mutation(\"autoresearch_start\", {{draft_id: {}, token: {}}})\n",
        lit(slug),
        lit(token)
    ));
    out.push_str(&format!(
        "startWindow = Mutation(\"autoresearch_start\", {{draft_id: {}, token: {}, window: {}}})\n",
        lit(slug),
        lit(token),
        lit(sensor)
    ));
    // R-11: a dirty checkout is refused at Start unless the user chooses to snapshot it onto the study
    // branch (§6); the third Start button says so. Not shown when the spec already includes them.
    let dirty = !spec.include_dirty;
    if dirty {
        out.push_str(&format!(
            "startDirty = Mutation(\"autoresearch_start\", {{draft_id: {}, token: {}, include_dirty: true}})\n",
            lit(slug),
            lit(token)
        ));
    }
    out.push_str(if dirty {
        "buttons = Buttons([bStart, bWindow, bDirty, bEdit, bDiscard])\n"
    } else {
        "buttons = Buttons([bStart, bWindow, bEdit, bDiscard])\n"
    });
    out.push_str(&format!(
        "bStart = Button(\"Start now\", Action([@Run(start), @ToAssistant({})]))\n",
        lit(&format!("study draft {slug}: started"))
    ));
    out.push_str(&format!(
        "bWindow = Button(\"Start in idle window\", Action([@Run(startWindow), @ToAssistant({})]), \"secondary\")\n",
        lit(&format!("study draft {slug}: started"))
    ));
    if dirty {
        out.push_str(&format!(
            "bDirty = Button(\"Start with my uncommitted changes\", Action([@Run(startDirty), @ToAssistant({})]), \"secondary\")\n",
            lit(&format!("study draft {slug}: started"))
        ));
    }
    out.push_str(&format!(
        "bEdit = Button(\"Edit\", Action([@ToAssistant({})]), \"tertiary\")\n",
        lit(&format!("study draft {slug}: edit"))
    ));
    out.push_str(&format!(
        "bDiscard = Button(\"Discard\", Action([@ToAssistant({})]), \"tertiary\", \"destructive\")\n",
        lit(&format!("study draft {slug}: discarded"))
    ));
    let starts: &[&str] = if dirty {
        &["start", "startWindow", "startDirty"]
    } else {
        &["start", "startWindow"]
    };
    // `a ? x : (b ? y : (ok ? started : ""))` — parenthesised like the two-button original.
    let ok = starts
        .iter()
        .map(|s| format!("{s}.status == \"success\""))
        .collect::<Vec<_>>()
        .join(" || ");
    let mut expr =
        format!("({ok} ? \"Started — preparing the worktree and the baseline.\" : \"\")");
    for s in starts.iter().rev() {
        expr = format!("{s}.status == \"error\" ? (\"Could not start: \" + {s}.error) : ({expr})");
    }
    out.push_str(&format!("result = TextContent({expr}, \"small\")\n"));
    out
}

#[async_trait::async_trait]
impl Tool for StudyDraftTool {
    const NAME: &'static str = "autoresearch_draft";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "dict";
    type Args = DraftArgs;

    fn description(&self) -> String {
        "Propose an autoresearch study for the user to approve: the goal, the metric and its direction, the \
         benchmark command, the correctness check, an optional holdout, the scope and off-limits paths, budgets. \
         Shows a draft card; the user's Start press creates the study — this call writes nothing. Benchmark scripts \
         written during setup belong under the autoresearch drafts directory, never in the user's checkout."
            .into()
    }

    async fn call(&self, a: DraftArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        // 151 D5: `partial` / `confirmed` are stored with the spec (ENG builds the relaxed validation and the compact card).
        let DraftArgs {
            spec,
            partial,
            confirmed,
        } = a;
        // Windows: no `/bin/bash` for the study's commands — refuse before a card is shown.
        if cfg!(windows) {
            return Ok(ToolOutput::err(format!(
                "autoresearch_draft: {}",
                autoresearch::WINDOWS_UNSUPPORTED
            )));
        }
        if let Err(e) = validate_spec(&spec) {
            return Ok(ToolOutput::err(format!("autoresearch_draft: {e}")));
        }
        // The live section from the actor: refuses when the plugin is off
        // (plugins.md §3.5.1) and follows a `root:` changed by `settings.set`.
        let cfg = match live_cfg().await {
            Ok(c) => c,
            Err(e) => return Ok(ToolOutput::err(format!("autoresearch_draft: {e}"))),
        };
        let root = cfg.root(&self.core.rt.data_dir);
        let token = match write_draft(&root, &spec, cx.session, partial, confirmed) {
            Ok(t) => t,
            Err(e) => {
                return Ok(ToolOutput::err(format!(
                    "autoresearch_draft: cannot write {}: {e}",
                    draft_spec_path(&root, &spec.slug).display()
                )));
            }
        };
        let warnings = warnings(&spec, &cfg);
        let app = AppArgs {
            name: APP_NAME.into(),
            source: source(&spec, &warnings, &token),
            title: Some("Study draft".into()),
            edit: false,
            base_call_id: None,
        };
        let (loaded, _) = match commit_source(
            &self.core,
            app,
            cx.session,
            cx.call_id.clone(),
            Origin::Tool,
        )
        .await
        {
            Ok(v) => v,
            Err(e) => return Ok(ToolOutput::err(format!("autoresearch_draft: {e}"))),
        };
        let slug = &spec.slug;
        let content = format!(
            "drafted study card `{slug}` (record_call_id {}):\n{}\n{}\nWait for the user. Their press arrives as a message \
             beginning `[app {APP_NAME} …]` with `study draft {slug}: started | edit | discarded`. started → the harness is \
             already preparing it (the press created the study; call nothing); edit → ask what to change and call \
             autoresearch_draft again; discarded → answer one line, call nothing.",
            loaded.artifact.call_id,
            lines(&spec).join("\n"),
            if warnings.is_empty() {
                "no warnings".to_owned()
            } else {
                format!("warnings: {}", warnings.join(" / "))
            },
        );
        Ok(ToolOutput {
            structured: Some(serde_json::json!({
                "record_call_id": loaded.artifact.call_id,
                "draft_id": slug,
                "warnings": warnings,
            })),
            ..ToolOutput::ok(content)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use autoresearch::{
        BenchSpec, BudgetSpec, CommandSpec, Guard, HoldoutSpec, MetricSpec, ModelsSpec,
    };

    fn spec() -> StudySpec {
        StudySpec {
            slug: "sort-speed".into(),
            goal: "Make parse+render faster".into(),
            repo: "/Users/x/Develop/liquid".into(),
            metric: MetricSpec {
                name: "time_ms".into(),
                unit: "ms".into(),
                direction: Direction::Lower,
                min_effect: "0.5%".into(),
            },
            bench: BenchSpec {
                command: "bench.sh".into(),
                ..Default::default()
            },
            checks: Some(CommandSpec {
                command: "bundle exec rake test".into(),
                ..Default::default()
            }),
            holdout: Some(HoldoutSpec {
                command: "holdout.sh".into(),
                every: 5,
                repeats: None,
            }),
            evaluator_paths: vec!["fixtures/".into(), "test/".into()],
            scope: vec!["lib/liquid/**".into()],
            off_limits: vec!["fixtures/**".into(), "test/**".into()],
            ..Default::default()
        }
    }

    #[test]
    fn five_lines_read_as_the_design_says() {
        let l = lines(&spec());
        assert_eq!(
            l[0],
            "Optimise `time_ms` (lower is better; keep from 0.5%) of `bench.sh` in liquid (base HEAD)"
        );
        assert_eq!(
            l[1],
            "check `bundle exec rake test` · holdout `holdout.sh` every 5 keeps"
        );
        assert_eq!(
            l[2],
            "may edit lib/liquid/** · never keeps fixtures/**, test/** · evaluator files fixtures/, test/"
        );
        assert_eq!(
            l[3],
            "up to 120 experiments, 10 h, $20 ($1.5 per experiment) · 3 baseline samples, 600s per sample"
        );
        assert_eq!(
            l[4],
            "starts now · branch autoresearch/sort-speed in its own worktree"
        );
        let bare = StudySpec {
            checks: None,
            holdout: None,
            scope: vec![],
            window: Some("sensors/idle.py".into()),
            parallel: 3,
            ..spec()
        };
        let l = lines(&bare);
        assert_eq!(l[1], "no check · no holdout");
        assert!(l[2].starts_with("may edit anything (scope empty)"));
        assert!(l[3].ends_with("· waves of 3"));
        assert!(l[4].starts_with("starts in the idle window (`sensors/idle.py`)"));
        // R-19: a guard says which way it improves, independent of the primary.
        let guarded = StudySpec {
            secondary_guards: vec![
                Guard {
                    name: "alloc_mb".into(),
                    max_regression: "5%".into(),
                    direction: Direction::Lower,
                },
                Guard {
                    name: "hit_rate".into(),
                    max_regression: "0.01".into(),
                    direction: Direction::Higher,
                },
            ],
            ..spec()
        };
        assert_eq!(
            lines(&guarded)[1],
            "check `bundle exec rake test` · holdout `holdout.sh` every 5 keeps · guards alloc_mb ≤ 5% worse (lower is better), hit_rate ≤ 0.01 worse (higher is better)"
        );
    }

    #[test]
    fn warnings_cover_every_case_of_4_1() {
        let cfg = AutoresearchConfig::default();
        assert!(warnings(&spec(), &cfg).is_empty());
        let risky = StudySpec {
            checks: None,
            holdout: None,
            scope: vec![],
            bench: BenchSpec {
                exclusive: false,
                ..spec().bench
            },
            budget: BudgetSpec {
                usd: 80.0,
                ..Default::default()
            },
            models: ModelsSpec {
                review: Some("off".into()),
                ..Default::default()
            },
            ..spec()
        };
        let w = warnings(&risky, &cfg);
        assert_eq!(w.len(), 6, "{w:?}");
        assert!(w[0].starts_with("No correctness check"));
        assert!(w[1].starts_with("No holdout"));
        assert!(w[2].contains("wall-clock") && w[2].contains("exclusive: false"));
        assert!(w[3].starts_with("Scope is empty"));
        assert_eq!(
            w[4],
            "Budget $80 is above the confirmation threshold ($50)."
        );
        assert!(w[5].starts_with("Review is off"));
        // A non-time unit without the slot is fine.
        let bytes = StudySpec {
            metric: MetricSpec {
                unit: "KiB".into(),
                ..spec().metric
            },
            bench: BenchSpec {
                exclusive: false,
                ..spec().bench
            },
            ..spec()
        };
        assert!(warnings(&bytes, &cfg).is_empty());
    }

    /// The press path (R-10): Start buttons run `autoresearch_start` with the
    /// draft's press token (never `autoresearch_control`) then tell the chat;
    /// Edit/Discard only tell the chat.
    #[test]
    fn source_wires_presses_through_the_token() {
        let s = source(&spec(), &["No holdout".into()], "tok123");
        assert!(
            s.starts_with("root = Card([head, l1, l2, l3, l4, l5, warn, buttons, result])\n"),
            "{s}"
        );
        assert!(s.contains("head = CardHeader(\"sort-speed\", \"study draft sort-speed\")\n"));
        assert!(s.contains("start = Mutation(\"autoresearch_start\", {draft_id: \"sort-speed\", token: \"tok123\"})\n"), "{s}");
        assert!(s.contains("startWindow = Mutation(\"autoresearch_start\", {draft_id: \"sort-speed\", token: \"tok123\", window: \"idle.py\"})\n"), "{s}");
        assert!(!s.contains("autoresearch_control"), "{s}");
        assert!(s.contains("bStart = Button(\"Start now\", Action([@Run(start), @ToAssistant(\"study draft sort-speed: started\")]))\n"), "{s}");
        assert!(s.contains("bWindow = Button(\"Start in idle window\", Action([@Run(startWindow), @ToAssistant(\"study draft sort-speed: started\")]), \"secondary\")\n"), "{s}");
        assert!(s.contains("bEdit = Button(\"Edit\", Action([@ToAssistant(\"study draft sort-speed: edit\")]), \"tertiary\")\n"));
        assert!(s.contains("bDiscard = Button(\"Discard\", Action([@ToAssistant(\"study draft sort-speed: discarded\")]), \"tertiary\", \"destructive\")\n"));
        // R-11: the third Start snapshots the user's dirty tree (`include_dirty`); the result line covers it.
        assert!(s.contains("startDirty = Mutation(\"autoresearch_start\", {draft_id: \"sort-speed\", token: \"tok123\", include_dirty: true})\n"), "{s}");
        assert!(
            s.contains("buttons = Buttons([bStart, bWindow, bDirty, bEdit, bDiscard])\n"),
            "{s}"
        );
        assert!(s.contains("bDirty = Button(\"Start with my uncommitted changes\", Action([@Run(startDirty), @ToAssistant(\"study draft sort-speed: started\")]), \"secondary\")\n"), "{s}");
        assert!(
            s.contains(
                "startDirty.status == \"error\" ? (\"Could not start: \" + startDirty.error)"
            ) && s.contains("|| startDirty.status == \"success\""),
            "{s}"
        );
        assert!(
            s.contains("warn = Callout(\"warning\", \"1 warning\", \"⚠ No holdout\")\n"),
            "{s}"
        );
        // The spec's own sensor wins for the window button; no warnings → no callout member.
        let s2 = source(
            &StudySpec {
                window: Some("night.py".into()),
                ..spec()
            },
            &[],
            "t",
        );
        assert!(s2.contains("window: \"night.py\"}"));
        assert!(s2.starts_with("root = Card([head, l1, l2, l3, l4, l5, buttons, result])\n"));
        // A spec that already includes the changes has no third button and says so in line 1.
        let dirty = StudySpec {
            include_dirty: true,
            ..spec()
        };
        let s3 = source(&dirty, &[], "t");
        assert!(!s3.contains("startDirty"), "{s3}");
        assert!(
            s3.contains("buttons = Buttons([bStart, bWindow, bEdit, bDiscard])\n"),
            "{s3}"
        );
        assert!(lines(&dirty)[0].ends_with("(base HEAD + your uncommitted changes)"));
    }
}
