//! One replay (159 §13.1): render, call the target, score by checker or judge. The model call is
//! behind [`Model`] so the caller owns cost and `CallKind` (autoresearch charges
//! `CallKind::Autoresearch` to the study session, dreaming `CallKind::Dream` to its own).
//! Owner: W2.

use std::path::PathBuf;
use std::time::Duration;

use tokio::io::AsyncWriteExt as _;

use crate::case::{Case, Check};
use crate::judge;

/// A model the runner may call: one system prompt, one user message, one reply.
#[async_trait::async_trait]
pub trait Model: Send + Sync {
    /// Ask; `Err` is the provider's or the budget's message.
    async fn call(&self, system: &str, user: String) -> Result<String, String>;
}

/// What one case cost and scored. `tokens_*` and `cost_usd` stay 0 here: [`Model::call`]
/// returns text only, so the caller's adapter (which sees the provider's reply) fills them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CaseScore {
    /// 0–1.
    pub score: f64,
    /// The target's answer.
    pub answer: String,
    /// The checker's diff or the judge's reason.
    pub reason: String,
    /// Tokens the target read.
    pub tokens_in: u64,
    /// Tokens the target wrote.
    pub tokens_out: u64,
    /// USD, both calls.
    pub cost_usd: f64,
}

/// Where and how a `Check::Command` runs (R-W2-3): the custody directory as its cwd, a wall
/// clock, extra environment. [`CheckCtx::default`] is the process's cwd and [`CHECK_TIMEOUT`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CheckCtx {
    /// `current_dir` of the checker; `None` = inherit.
    pub cwd: Option<PathBuf>,
    /// Wall clock per checker run; zero = [`CHECK_TIMEOUT`].
    pub timeout: Duration,
    /// Extra environment (`AUTORESEARCH_EVALUATOR`, …).
    pub env: Vec<(String, String)>,
}

/// The checker's wall clock when the context names none.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(60);

/// The placeholder a `render` template must contain.
pub const ARTEFACT_SLOT: &str = "{{artefact}}";

/// The system prompt: `template` with [`ARTEFACT_SLOT`] replaced, or the artefact itself.
pub fn render(artefact: &str, template: Option<&str>) -> String {
    match template.map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => t.replace(ARTEFACT_SLOT, artefact),
        None => artefact.to_owned(),
    }
}

/// Render (system = the artefact, or `template` with `{{artefact}}`; user = the case input),
/// call `target`, then score with the case's check or with `judge` and the 0–1 prompt. A case
/// with neither a check nor a judge is an error, as is a judge reply without a score. A
/// `command` check runs in the process's cwd; see [`run_case_in`] for custody.
pub async fn run_case(
    target: &dyn Model,
    judge: Option<&dyn Model>,
    artefact: &str,
    template: Option<&str>,
    case: &Case,
) -> Result<CaseScore, String> {
    run_case_in(
        target,
        judge,
        artefact,
        template,
        case,
        &CheckCtx::default(),
    )
    .await
}

/// [`run_case`] with a [`CheckCtx`] for `command` checks.
pub async fn run_case_in(
    target: &dyn Model,
    judge: Option<&dyn Model>,
    artefact: &str,
    template: Option<&str>,
    case: &Case,
    ctx: &CheckCtx,
) -> Result<CaseScore, String> {
    let answer = target
        .call(&render(artefact, template), case.input.clone())
        .await?;
    let (score, reason) = match (&case.check, judge) {
        (Some(check), _) => score_check(check, &answer, case, ctx).await?,
        (None, Some(judge)) => {
            let reply = judge
                .call(
                    &judge::system(),
                    judge::user(case.ideal.as_deref(), &case.references, &answer),
                )
                .await?;
            judge::parse_with_reason(&reply)
                .ok_or_else(|| format!("judge answered without a score: {}", reply.trim()))?
        }
        (None, None) => return Err(format!("case `{}` has no check and no judge", case.id)),
    };
    Ok(CaseScore {
        score,
        answer,
        reason,
        ..CaseScore::default()
    })
}

/// Score an answer with a [`Check`]; the reason is the checker's one-line diff.
pub async fn score_check(
    check: &Check,
    answer: &str,
    case: &Case,
    ctx: &CheckCtx,
) -> Result<(f64, String), String> {
    let got = answer.trim();
    Ok(match check {
        Check::Exact(want) => {
            if got == want.trim() {
                (1.0, String::new())
            } else {
                (
                    0.0,
                    format!("expected `{}`, got `{}`", clip(want.trim()), clip(got)),
                )
            }
        }
        Check::Regex(re) => {
            let re = regex::Regex::new(re).map_err(|e| format!("check regex `{re}`: {e}"))?;
            if re.is_match(got) {
                (1.0, String::new())
            } else {
                (
                    0.0,
                    format!("`{}` does not match /{}/", clip(got), re.as_str()),
                )
            }
        }
        Check::Contains(needle) => {
            if got.contains(needle.as_str()) {
                (1.0, String::new())
            } else {
                (
                    0.0,
                    format!("`{}` does not contain `{}`", clip(got), clip(needle)),
                )
            }
        }
        Check::Command(cmd) => command_check(cmd, answer, case, ctx).await?,
    })
}

/// `/bin/bash -lc <cmd>` in `ctx.cwd` with the answer on stdin, `CASE_ID` / `CASE_IDEAL` and
/// `ctx.env` set, killed at `ctx.timeout`; the score is the last `METRIC score=` line, the
/// reason the last other non-empty line.
async fn command_check(
    cmd: &str,
    answer: &str,
    case: &Case,
    ctx: &CheckCtx,
) -> Result<(f64, String), String> {
    let mut command = tokio::process::Command::new("/bin/bash");
    command
        .arg("-lc")
        .arg(cmd)
        .env("CASE_ID", &case.id)
        .env("CASE_IDEAL", case.ideal.as_deref().unwrap_or(""))
        .envs(ctx.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    if let Some(cwd) = &ctx.cwd {
        command.current_dir(cwd);
    }
    let mut child = command.spawn().map_err(|e| format!("check `{cmd}`: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(answer.as_bytes()).await;
        drop(stdin);
    }
    let timeout = if ctx.timeout.is_zero() {
        CHECK_TIMEOUT
    } else {
        ctx.timeout
    };
    let out = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| format!("check `{cmd}` timed out after {}s", timeout.as_secs()))?
        .map_err(|e| format!("check `{cmd}`: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let score = stdout
        .lines()
        .filter_map(|l| l.trim().strip_prefix("METRIC score="))
        .filter_map(|v| v.trim().parse::<f64>().ok())
        .next_back()
        .ok_or_else(|| {
            format!(
                "check `{cmd}` printed no `METRIC score=` line (exit {:?}): {}",
                out.status.code(),
                clip(String::from_utf8_lossy(&out.stderr).trim())
            )
        })?;
    if !score.is_finite() {
        return Err(format!("check `{cmd}`: score is not a number"));
    }
    let reason = stdout
        .lines()
        .map(str::trim)
        .rfind(|l| !l.starts_with("METRIC ") && !l.is_empty())
        .unwrap_or("")
        .to_owned();
    Ok((score.clamp(0.0, 1.0), clip(&reason).into_owned()))
}

/// First 120 chars, one line.
fn clip(s: &str) -> std::borrow::Cow<'_, str> {
    let one: String = s.chars().take(120).collect();
    if one.len() == s.len() && !one.contains('\n') {
        std::borrow::Cow::Borrowed(s)
    } else {
        std::borrow::Cow::Owned(one.replace('\n', " ") + if one.len() < s.len() { "…" } else { "" })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo(&'static str);
    #[async_trait::async_trait]
    impl Model for Echo {
        async fn call(&self, system: &str, user: String) -> Result<String, String> {
            Ok(format!("{}|{system}|{user}", self.0))
        }
    }

    fn case(check: Option<Check>) -> Case {
        Case {
            id: "c1".into(),
            input: "3 March 2021".into(),
            ideal: Some("2021-03-03".into()),
            check,
            ..Case::default()
        }
    }

    const NO_CTX: CheckCtx = CheckCtx {
        cwd: None,
        timeout: Duration::ZERO,
        env: Vec::new(),
    };

    #[test]
    fn render_fills_the_slot_or_is_the_artefact() {
        assert_eq!(render("P", None), "P");
        assert_eq!(render("P", Some("  ")), "P");
        assert_eq!(render("P", Some("<<{{artefact}}>>")), "<<P>>");
    }

    #[tokio::test]
    async fn checks_score_exact_regex_contains_and_command() {
        let c = case(None);
        assert_eq!(
            score_check(
                &Check::Exact("2021-03-03".into()),
                " 2021-03-03\n",
                &c,
                &NO_CTX
            )
            .await
            .unwrap(),
            (1.0, String::new())
        );
        let (s, r) = score_check(
            &Check::Exact("2021-03-03".into()),
            "03/03/2021",
            &c,
            &NO_CTX,
        )
        .await
        .unwrap();
        assert_eq!(s, 0.0);
        assert!(r.contains("expected") && r.contains("03/03/2021"));
        assert_eq!(
            score_check(
                &Check::Regex(r"^\d{4}-\d{2}-\d{2}$".into()),
                "2021-03-03",
                &c,
                &NO_CTX
            )
            .await
            .unwrap()
            .0,
            1.0
        );
        assert!(
            score_check(&Check::Regex("(".into()), "x", &c, &NO_CTX)
                .await
                .is_err()
        );
        assert_eq!(
            score_check(&Check::Contains("03".into()), "2021-03-03", &c, &NO_CTX)
                .await
                .unwrap()
                .0,
            1.0
        );
        let (s, r) = score_check(
            &Check::Command("read a; if [ \"$a\" = \"$CASE_IDEAL\" ]; then echo 'METRIC score=1'; else echo \"got $a\"; echo 'METRIC score=0.25'; fi".into()),
            "2021-03-03",
            &c,
            &NO_CTX,
        )
        .await
        .unwrap();
        assert_eq!((s, r), (1.0, String::new()));
        let (s, r) = score_check(
            &Check::Command("read a; echo \"got $a\"; echo 'METRIC score=0.25'".into()),
            "nope",
            &c,
            &NO_CTX,
        )
        .await
        .unwrap();
        assert_eq!((s, r.as_str()), (0.25, "got nope"));
        assert!(
            score_check(&Check::Command("true".into()), "x", &c, &NO_CTX)
                .await
                .is_err()
        );
    }

    /// R-W2-3: a `command` check runs in the context's cwd (a relative `./score.sh` resolves
    /// there, not in the daemon's cwd), sees the context's env, and is killed at the timeout
    /// instead of blocking a worker.
    #[tokio::test]
    async fn command_checks_run_in_the_custody_dir_with_env_and_a_timeout() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("score.sh"),
            "#!/bin/bash\nread a\n[ \"$PWD\" = \"$AUTORESEARCH_EVALUATOR\" ] || { echo wrong-cwd; echo 'METRIC score=0'; exit 0; }\nif [ \"$a\" = \"$CASE_IDEAL\" ]; then echo 'METRIC score=1'; else echo \"got $a\"; echo 'METRIC score=0'; fi\n",
        )
        .unwrap();
        let c = case(Some(Check::Command("bash ./score.sh".into())));
        // Without a cwd the relative script is not found: an error, not a silent zero.
        let e = score_check(c.check.as_ref().unwrap(), "2021-03-03", &c, &NO_CTX)
            .await
            .unwrap_err();
        assert!(e.contains("no `METRIC score=` line"), "{e}");
        let ctx = CheckCtx {
            cwd: Some(dir.path().to_path_buf()),
            timeout: Duration::from_secs(10),
            env: vec![(
                "AUTORESEARCH_EVALUATOR".into(),
                dir.path().canonicalize().unwrap().display().to_string(),
            )],
        };
        // `$PWD` is the canonical path on macOS (`/private/var/…`), hence the canonicalised env.
        let (s, r) = score_check(c.check.as_ref().unwrap(), "2021-03-03", &c, &ctx)
            .await
            .unwrap();
        assert_eq!((s, r.as_str()), (1.0, ""));
        let s = run_case_in(&Echo("2021-03-03"), None, "P", None, &c, &ctx)
            .await
            .unwrap();
        assert_eq!(
            s.score, 0.0,
            "the echo answer is not the bare date: {}",
            s.reason
        );
        // A hung checker is killed at the timeout.
        let slow = CheckCtx {
            timeout: Duration::from_millis(200),
            ..ctx
        };
        let started = std::time::Instant::now();
        let e = score_check(
            &Check::Command("sleep 5; echo 'METRIC score=1'".into()),
            "x",
            &c,
            &slow,
        )
        .await
        .unwrap_err();
        assert!(e.contains("timed out"), "{e}");
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[tokio::test]
    async fn run_case_renders_calls_and_scores() {
        // With a check: the judge is never consulted.
        let s = run_case(
            &Echo("2021-03-03"),
            None,
            "P",
            None,
            &case(Some(Check::Contains("2021-03-03".into()))),
        )
        .await
        .unwrap();
        assert_eq!(s.score, 1.0);
        assert_eq!(s.answer, "2021-03-03|P|3 March 2021");
        // Without a check: the judge sees the fenced answer and the ideal, and its JSON is parsed.
        struct Judge;
        #[async_trait::async_trait]
        impl Model for Judge {
            async fn call(&self, system: &str, user: String) -> Result<String, String> {
                assert!(system.starts_with(judge::SYSTEM) && system.contains("<<<ANSWER"));
                assert!(user.contains("IDEAL:\n2021-03-03") && user.contains("<<<ANSWER\n"));
                Ok("{\"score\": 0.5, \"reason\": \"half\"}".into())
            }
        }
        let s = run_case(
            &Echo("x"),
            Some(&Judge),
            "P",
            Some("sys {{artefact}}"),
            &case(None),
        )
        .await
        .unwrap();
        assert_eq!((s.score, s.reason.as_str()), (0.5, "half"));
        assert!(s.answer.starts_with("x|sys P|"));
        assert!(
            run_case(&Echo("x"), None, "P", None, &case(None))
                .await
                .is_err()
        );
    }
}
