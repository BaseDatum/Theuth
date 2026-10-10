//! One replay (159 §13.1): render, call the target, score by checker or judge. The model call is
//! behind [`Model`] so the caller owns cost and `CallKind` (autoresearch charges
//! `CallKind::Autoresearch` to the study session, dreaming `CallKind::Dream` to its own).
//! Owner: W2.

use std::io::Write as _;

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
/// with neither a check nor a judge is an error, as is a judge reply without a score.
pub async fn run_case(
    target: &dyn Model,
    judge: Option<&dyn Model>,
    artefact: &str,
    template: Option<&str>,
    case: &Case,
) -> Result<CaseScore, String> {
    let answer = target
        .call(&render(artefact, template), case.input.clone())
        .await?;
    let (score, reason) = match (&case.check, judge) {
        (Some(check), _) => score_check(check, &answer, case)?,
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
pub fn score_check(check: &Check, answer: &str, case: &Case) -> Result<(f64, String), String> {
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
        Check::Command(cmd) => {
            // ponytail: a blocking `bash -lc` on the async thread; checkers are small. Move it
            // behind the caller's runner if a checker ever takes seconds.
            let mut child = std::process::Command::new("/bin/bash")
                .arg("-lc")
                .arg(cmd)
                .env("CASE_ID", &case.id)
                .env("CASE_IDEAL", case.ideal.as_deref().unwrap_or(""))
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .map_err(|e| format!("check `{cmd}`: {e}"))?;
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(answer.as_bytes());
            }
            let out = child
                .wait_with_output()
                .map_err(|e| format!("check `{cmd}`: {e}"))?;
            let stdout = String::from_utf8_lossy(&out.stdout);
            let score = stdout
                .lines()
                .filter_map(|l| l.trim().strip_prefix("METRIC score="))
                .filter_map(|v| v.trim().parse::<f64>().ok())
                .next_back()
                .ok_or_else(|| format!("check `{cmd}` printed no `METRIC score=` line"))?;
            if !score.is_finite() {
                return Err(format!("check `{cmd}`: score is not a number"));
            }
            let reason = stdout
                .lines()
                .map(str::trim)
                .rfind(|l| !l.starts_with("METRIC ") && !l.is_empty())
                .unwrap_or("")
                .to_owned();
            (score.clamp(0.0, 1.0), clip(&reason).into_owned())
        }
    })
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

    #[test]
    fn render_fills_the_slot_or_is_the_artefact() {
        assert_eq!(render("P", None), "P");
        assert_eq!(render("P", Some("  ")), "P");
        assert_eq!(render("P", Some("<<{{artefact}}>>")), "<<P>>");
    }

    #[test]
    fn checks_score_exact_regex_contains_and_command() {
        let c = case(None);
        assert_eq!(
            score_check(&Check::Exact("2021-03-03".into()), " 2021-03-03\n", &c).unwrap(),
            (1.0, String::new())
        );
        let (s, r) = score_check(&Check::Exact("2021-03-03".into()), "03/03/2021", &c).unwrap();
        assert_eq!(s, 0.0);
        assert!(r.contains("expected") && r.contains("03/03/2021"));
        assert_eq!(
            score_check(
                &Check::Regex(r"^\d{4}-\d{2}-\d{2}$".into()),
                "2021-03-03",
                &c
            )
            .unwrap()
            .0,
            1.0
        );
        assert!(score_check(&Check::Regex("(".into()), "x", &c).is_err());
        assert_eq!(
            score_check(&Check::Contains("03".into()), "2021-03-03", &c)
                .unwrap()
                .0,
            1.0
        );
        let (s, r) = score_check(
            &Check::Command("read a; if [ \"$a\" = \"$CASE_IDEAL\" ]; then echo 'METRIC score=1'; else echo \"got $a\"; echo 'METRIC score=0.25'; fi".into()),
            "2021-03-03",
            &c,
        )
        .unwrap();
        assert_eq!((s, r), (1.0, String::new()));
        let (s, r) = score_check(
            &Check::Command("read a; echo \"got $a\"; echo 'METRIC score=0.25'".into()),
            "nope",
            &c,
        )
        .unwrap();
        assert_eq!((s, r.as_str()), (0.25, "got nope"));
        assert!(score_check(&Check::Command("true".into()), "x", &c).is_err());
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
