//! 159 W5 — before/after hooks (D5): custody scripts the plugin runs through
//! `Backend::plugin_command`; their output (≤ 8 KiB) reaches the next brief as the fenced section
//! "From your hooks". `before` runs at `experiment_begin` in the study worktree before the brief
//! renders; `after` runs when an experiment closes with `AUTORESEARCH_VERDICT` and
//! `AUTORESEARCH_DELTA_PCT`, and its output rides into the next `before`. A non-zero exit or a
//! timeout is a line in that section, never a crash. Both return `Ok(None)` when `spec.hooks` is
//! `None`, which is every 140 study, so the registration lines in actor.rs are inert then.
//!
//! The two hook lines are also the only W5 plug-in points at experiment begin and close, so the
//! literature block's seed and `every`-keeps consult ride on them
//! (`literature::{on_experiment_begin, on_experiment_closed}`), before the hook itself.
//!
//! Custody: a hook may only name scripts the study holds in custody (`evaluator/<segment>/`,
//! filled from `evaluator_paths`); the runner then executes the custody copy, so an edit to the
//! worktree copy changes nothing, and a script with no custody copy is refused.
//!
//! Ledger: `hook_before {n, exit, wall_ms, output}` and `hook_after {n, exit, wall_ms, output}`.

use std::io::Read as _;
use std::path::Path;
use std::time::Duration;

use crate::actor::Env;
use crate::error::Result;
use crate::store::{ExperimentRecord, StudyRecord};
use crate::types::{CommandSpec, ExperimentState};

/// Hook output kept (bytes).
pub const OUTPUT_CAP: usize = 8 * 1024;
/// When the spec's timeout does not parse.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// `{n, exit, wall_ms, output}` per `before` run (`{n, ran: false}` when only `after` is set).
pub const EVENT_BEFORE: &str = "hook_before";
/// `{n, exit, wall_ms, output, refused?}` per `after` run.
pub const EVENT_AFTER: &str = "hook_after";
/// Flags that make the interpreter run code from the command line or from the worktree
/// (`bash -c`, `python3 -m`, `node -e`); a hook names a custody script instead.
const INLINE_FLAGS: [&str; 3] = ["-c", "-m", "-e"];

/// `hooks.before` at `experiment_begin`, in the study worktree, before the brief renders.
/// `Some(text)` becomes the brief section "From your hooks": the previous `after` output not
/// shown yet, then this `before` run.
pub async fn before(env: &Env, study: &StudyRecord, n: u32) -> Result<Option<String>> {
    crate::literature::on_experiment_begin(env, study).await;
    let Some(hooks) = &study.spec.hooks else {
        return Ok(None);
    };
    let mut out = Vec::new();
    // The newest `after` output newer than the newest `before` row has not been shown yet.
    let shown = env
        .store
        .list_events(&study.slug, Some(EVENT_BEFORE), 1)
        .await?;
    let after = env
        .store
        .list_events(&study.slug, Some(EVENT_AFTER), 1)
        .await?;
    if let Some(a) = after.first()
        && shown.first().is_none_or(|b| b.id < a.id)
    {
        out.push(section(
            "after",
            a.payload["n"].as_u64().unwrap_or(0) as u32,
            &a.payload,
        ));
    }
    let payload = match hooks
        .before
        .as_ref()
        .filter(|c| !c.command.trim().is_empty())
    {
        Some(cmd) => {
            let json = hook_json(env, "before", study, n, None).await;
            let extra = vec![
                ("AUTORESEARCH_HOOK".to_owned(), "before".to_owned()),
                ("AUTORESEARCH_HOOK_JSON".to_owned(), json),
            ];
            let p = run(env, study, &format!("hook_before-{n}"), cmd, &extra).await;
            out.push(section("before", n, &p));
            serde_json::json!({ "n": n, "exit": p["exit"], "wall_ms": p["wall_ms"], "output": p["output"] })
        }
        None => serde_json::json!({ "n": n, "ran": false }),
    };
    env.store
        .append_event(&study.slug, EVENT_BEFORE, &payload)
        .await?;
    Ok((!out.is_empty()).then(|| out.join("\n")))
}

/// `hooks.after` when an experiment closes, with `AUTORESEARCH_VERDICT` and
/// `AUTORESEARCH_DELTA_PCT` set. `Some(output)` also goes into the next brief.
pub async fn after(
    env: &Env,
    study: &StudyRecord,
    exp: &ExperimentRecord,
) -> Result<Option<String>> {
    crate::literature::on_experiment_closed(env, study, exp).await;
    let Some(cmd) = study
        .spec
        .hooks
        .as_ref()
        .and_then(|h| h.after.as_ref())
        .filter(|c| !c.command.trim().is_empty())
    else {
        return Ok(None);
    };
    let json = hook_json(env, "after", study, exp.n, Some(exp)).await;
    let extra = vec![
        ("AUTORESEARCH_HOOK".to_owned(), "after".to_owned()),
        ("AUTORESEARCH_HOOK_JSON".to_owned(), json),
        (
            "AUTORESEARCH_VERDICT".to_owned(),
            exp.state.tag().to_owned(),
        ),
        (
            "AUTORESEARCH_DELTA_PCT".to_owned(),
            exp.delta_pct.map(|d| format!("{d:.3}")).unwrap_or_default(),
        ),
    ];
    let mut p = run(env, study, &format!("hook_after-{}", exp.n), cmd, &extra).await;
    p["n"] = serde_json::json!(exp.n);
    env.store.append_event(&study.slug, EVENT_AFTER, &p).await?;
    Ok(p["output"].as_str().map(str::to_owned))
}

/// Run one hook: custody check, `plugin_command`, the first [`OUTPUT_CAP`] of its log
/// (`runs/plugin/<what>-0.log`, `what` carries the experiment number so parallel slots never
/// share a file). The payload `{exit, wall_ms, output, refused?}` is what the events store and
/// the section renders.
///
/// Custody (R-W5-5): the command is refused when it carries an inline-code flag (`-c`, `-m`,
/// `-e`) or names a path that is a script or exists in the worktree without a copy under
/// `evaluator/<segment>/` — so the hook can only run what custody holds.
async fn run(
    env: &Env,
    study: &StudyRecord,
    what: &str,
    cmd: &CommandSpec,
    extra: &[(String, String)],
) -> serde_json::Value {
    let slug = &study.slug;
    let evaluator = env.evaluator_dir(slug, study.segment);
    let drafts = crate::drafts_dir(&env.root, slug);
    let worktree = study.worktree.as_deref().map(Path::new);
    let refused = |why: String| serde_json::json!({ "exit": null, "wall_ms": 0, "output": "", "refused": format!("{why}; the hook did not run") });
    for tok in cmd.command.split_whitespace() {
        if INLINE_FLAGS.contains(&tok) {
            return refused(format!(
                "`{tok}` runs inline code or a module from the worktree; a hook runs a custody script"
            ));
        }
        if let Some(p) = crate::custody::command_path(tok, &drafts) {
            let rel = p.rel();
            let in_worktree = worktree.is_some_and(|w| w.join(rel).exists());
            if (script_like(rel) || in_worktree) && !evaluator.join(rel).is_file() {
                return refused(format!(
                    "`{tok}` is not in custody (list it in evaluator_paths)"
                ));
            }
        }
    }
    let timeout = crate::runner::parse_duration(&cmd.timeout).unwrap_or(DEFAULT_TIMEOUT);
    match env
        .backend
        .plugin_command(study, what, &cmd.command, timeout, extra)
        .await
    {
        Ok(r) => {
            let log = crate::study_dir(&env.root, slug)
                .join("runs")
                .join("plugin")
                .join(format!("{what}-0.log"));
            let output = head(&log).unwrap_or(r.tail);
            serde_json::json!({ "exit": r.exit, "wall_ms": r.wall_ms, "output": output })
        }
        Err(e) => serde_json::json!({
            "exit": null, "wall_ms": 0, "output": "",
            "refused": format!("could not run: {e}"),
        }),
    }
}

/// One hook's lines for the brief section (the brief fences the whole section as data).
fn section(which: &str, n: u32, p: &serde_json::Value) -> String {
    let output = crate::literature::fence_safe(p["output"].as_str().unwrap_or("").trim_end());
    let head = match (p["refused"].as_str(), p["exit"].as_i64()) {
        (Some(why), _) => format!("[{which} hook #{n}] refused: {why}"),
        (None, Some(0)) => format!("[{which} hook #{n}]"),
        (None, Some(code)) => format!("[{which} hook #{n}] incident: exit {code}"),
        (None, None) => format!("[{which} hook #{n}] incident: timed out, process group killed"),
    };
    if output.is_empty() {
        head
    } else {
        format!("{head}\n{output}")
    }
}

/// The one JSON line a hook gets (pi-autoresearch's shape: event, last run, study summary).
/// `last` is the closing experiment for `after`, the newest closed one for `before`.
async fn hook_json(
    env: &Env,
    event: &str,
    study: &StudyRecord,
    n: u32,
    last: Option<&ExperimentRecord>,
) -> String {
    let spec = &study.spec;
    let all = env
        .store
        .list_experiments(&study.slug, None, u32::MAX)
        .await
        .unwrap_or_default();
    let keeps = all
        .iter()
        .filter(|e| e.state == ExperimentState::Kept)
        .count();
    let last = last.or_else(|| all.iter().find(|e| e.state.is_closed()));
    serde_json::json!({
        "event": event,
        "n": n,
        "study": {
            "slug": study.slug,
            "goal": spec.goal,
            "metric": spec.metric.name,
            "unit": spec.metric.unit,
            "direction": spec.metric.direction,
            "incumbent_mean": study.incumbent_mean,
            "incumbent_sha": study.incumbent_sha,
            "keeps": keeps,
        },
        "last": last.map(|e| serde_json::json!({
            "n": e.n,
            "state": e.state.tag(),
            "idea": e.idea,
            "category": e.category,
            "delta_pct": e.delta_pct,
            "metric": e.metric,
            "line": e.line,
        })),
    })
    .to_string()
}

/// A relative token that names a script, not a program (`bash`, `python3`): has a directory part
/// or a script extension. Mirrors `custody::looks_like_script` (private there).
fn script_like(rel: &Path) -> bool {
    rel.components().count() > 1
        || rel.extension().and_then(|e| e.to_str()).is_some_and(|e| {
            matches!(
                e,
                "sh" | "bash" | "zsh" | "py" | "rb" | "js" | "mjs" | "ts" | "pl" | "lua"
            )
        })
}

/// The first [`OUTPUT_CAP`] bytes of the log, lossily decoded.
fn head(log: &Path) -> Option<String> {
    let mut file = std::fs::File::open(log).ok()?;
    let mut buf = Vec::with_capacity(OUTPUT_CAP);
    file.by_ref()
        .take(OUTPUT_CAP as u64)
        .read_to_end(&mut buf)
        .ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

#[cfg(test)]
mod tests {
    //! P (159 §10 Guard): hook custody — the custody copy runs, an edit to the worktree copy
    //! changes nothing, and a script without a custody copy is refused; `after` output rides into
    //! the next `before`; a failing hook is an incident line, not an error.

    use std::sync::Arc;

    use super::*;
    use crate::literature::tests::{Canned, env_with, spec};
    use crate::types::HooksSpec;

    fn hooks(before: &str, after: &str) -> HooksSpec {
        let c = |s: &str| {
            (!s.is_empty()).then(|| CommandSpec {
                command: s.to_owned(),
                timeout: "10s".into(),
            })
        };
        HooksSpec {
            before: c(before),
            after: c(after),
        }
    }

    #[tokio::test]
    async fn custody_copy_runs_and_a_missing_one_is_refused() {
        let mut sp = spec("toy-sort");
        sp.literature = None;
        sp.hooks = Some(hooks("bash hook.sh", "bash after.sh"));
        let (_d, env, study) = env_with(Arc::new(Canned::default()), sp).await;
        let evaluator = env.evaluator_dir("toy-sort", 0);
        std::fs::create_dir_all(&evaluator).unwrap();
        std::fs::write(evaluator.join("hook.sh"), "echo custody says n=$AUTORESEARCH_RUN event=$AUTORESEARCH_HOOK; echo \"$AUTORESEARCH_HOOK_JSON\" | grep -o '\"event\":\"before\"'").unwrap();
        let worktree = Path::new(study.worktree.as_deref().unwrap());
        std::fs::write(worktree.join("hook.sh"), "echo EVIL from the worktree").unwrap();
        std::fs::write(worktree.join("after.sh"), "echo EVIL after").unwrap();

        let text = before(&env, &study, 3).await.unwrap().unwrap();
        assert!(
            text.contains("[before hook #3]\ncustody says n=0 event=before"),
            "{text}"
        );
        assert!(text.contains("\"event\":\"before\""), "{text}");
        assert!(!text.contains("EVIL"), "{text}");
        // R-W5-8: the log is per experiment, so parallel slots never share a file.
        assert!(
            crate::study_dir(&env.root, "toy-sort")
                .join("runs/plugin/hook_before-3-0.log")
                .is_file()
        );
        let ev = env
            .store
            .list_events("toy-sort", Some(EVENT_BEFORE), 1)
            .await
            .unwrap();
        assert_eq!(ev[0].payload["exit"], 0);
        assert!(
            ev[0].payload["output"]
                .as_str()
                .unwrap()
                .contains("custody says")
        );

        // `after.sh` has no custody copy: refused, the worktree copy never runs.
        let mut exp = ExperimentRecord::open("toy-sort", 3, 0, "inc");
        exp.state = ExperimentState::Kept;
        exp.delta_pct = Some(-12.5);
        let out = after(&env, &study, &exp).await.unwrap();
        assert_eq!(out.as_deref(), Some(""));
        let ev = env
            .store
            .list_events("toy-sort", Some(EVENT_AFTER), 1)
            .await
            .unwrap();
        assert!(
            ev[0].payload["refused"]
                .as_str()
                .unwrap()
                .contains("not in custody"),
            "{}",
            ev[0].payload
        );
        // …and the refusal is a line in the next brief's section, not an error.
        let text = before(&env, &study, 4).await.unwrap().unwrap();
        assert!(
            text.starts_with("[after hook #3] refused: `after.sh` is not in custody"),
            "{text}"
        );
    }

    #[tokio::test]
    async fn after_output_rides_into_the_next_before_once_and_failures_are_incidents() {
        let mut sp = spec("toy-sort");
        sp.literature = None;
        sp.hooks = Some(hooks("", "bash after.sh"));
        let (_d, env, study) = env_with(Arc::new(Canned::default()), sp).await;
        let evaluator = env.evaluator_dir("toy-sort", 0);
        std::fs::create_dir_all(&evaluator).unwrap();
        std::fs::write(
            evaluator.join("after.sh"),
            "echo verdict=$AUTORESEARCH_VERDICT delta=$AUTORESEARCH_DELTA_PCT; echo \"$AUTORESEARCH_HOOK_JSON\" | grep -o '\"state\":\"discarded\"'; exit 3",
        )
        .unwrap();
        let mut exp = ExperimentRecord::open("toy-sort", 7, 0, "inc");
        exp.state = ExperimentState::Discarded;
        exp.delta_pct = Some(0.25);
        let out = after(&env, &study, &exp).await.unwrap().unwrap();
        assert!(out.contains("verdict=discarded delta=0.250"), "{out}");
        let text = before(&env, &study, 8).await.unwrap().unwrap();
        assert!(
            text.starts_with("[after hook #7] incident: exit 3\nverdict=discarded"),
            "{text}"
        );
        assert!(text.contains("\"state\":\"discarded\""));
        // Shown once: the next before has nothing new.
        assert_eq!(before(&env, &study, 9).await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_timeout_is_an_incident_and_output_is_capped() {
        let mut sp = spec("toy-sort");
        sp.literature = None;
        sp.hooks = Some(HooksSpec {
            before: Some(CommandSpec {
                command: "bash slow.sh".into(),
                timeout: "1s".into(),
            }),
            after: None,
        });
        let (_d, env, study) = env_with(Arc::new(Canned::default()), sp).await;
        let evaluator = env.evaluator_dir("toy-sort", 0);
        std::fs::create_dir_all(&evaluator).unwrap();
        std::fs::write(
            evaluator.join("slow.sh"),
            "head -c 20000 /dev/zero | tr '\\0' x; echo; echo '```'; sleep 5",
        )
        .unwrap();
        let text = before(&env, &study, 1).await.unwrap().unwrap();
        assert!(
            text.starts_with("[before hook #1] incident: timed out"),
            "{}",
            &text[..60]
        );
        assert!(text.len() <= OUTPUT_CAP + 80, "{}", text.len());
        assert!(!text.contains("```"));
    }

    /// R-W5-5: a hook that would run code from the worktree without a custody script —
    /// `bash -c …`, `python3 -m hook`, `node hook` (a worktree file with no extension) — is refused
    /// before anything runs; the plain custody script still runs.
    #[tokio::test]
    async fn inline_code_and_worktree_files_are_refused() {
        let mut sp = spec("toy-sort");
        sp.literature = None;
        sp.hooks = Some(hooks("bash hook.sh", ""));
        let (_d, env, mut study) = env_with(Arc::new(Canned::default()), sp).await;
        let evaluator = env.evaluator_dir("toy-sort", 0);
        std::fs::create_dir_all(&evaluator).unwrap();
        std::fs::write(evaluator.join("hook.sh"), "echo ok").unwrap();
        let worktree = Path::new(study.worktree.as_deref().unwrap()).to_path_buf();
        std::fs::write(worktree.join("hook"), "echo EVIL").unwrap();
        std::fs::write(worktree.join("hook.py"), "print('EVIL')").unwrap();
        for (cmd, why) in [
            ("bash -c 'echo EVIL'", "`-c` runs inline code"),
            ("python3 -m hook", "`-m` runs inline code"),
            ("bash hook", "`hook` is not in custody"),
            ("python3 hook.py", "`hook.py` is not in custody"),
            ("node -e EVIL", "`-e` runs inline code"),
        ] {
            study
                .spec
                .hooks
                .as_mut()
                .unwrap()
                .before
                .as_mut()
                .unwrap()
                .command = cmd.into();
            let text = before(&env, &study, 1).await.unwrap().unwrap();
            assert!(
                text.contains("refused:") && text.contains(why),
                "{cmd}: {text}"
            );
            assert!(!text.contains("EVIL"), "{cmd}: {text}");
        }
        // A program name that is also no worktree file (`bash`) passes; the custody script runs.
        study
            .spec
            .hooks
            .as_mut()
            .unwrap()
            .before
            .as_mut()
            .unwrap()
            .command = "bash hook.sh".into();
        let text = before(&env, &study, 2).await.unwrap().unwrap();
        assert!(text.ends_with("[before hook #2]\nok"), "{text}");
    }

    #[test]
    fn a_140_study_has_no_hooks() {
        assert!(StudyRecord::new(spec("x"), None).spec.hooks.is_none());
        assert!(script_like(Path::new("hook.sh")) && script_like(Path::new("tools/run")));
        assert!(!script_like(Path::new("python3")));
    }
}
