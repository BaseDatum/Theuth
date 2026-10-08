//! 125 R1.3: the loop-local `consult` tool — its spec and argument shape,
//! the `### Consultation` block the advisor reads, the one-shot system
//! prompt a subagent's consultation runs on (R1.6) and the tool's fixed
//! error strings. No behaviour lives here; the actor (`Consult`) and the
//! loop (`Turn::consult`) use these names.

use serde::Deserialize;
use tools::{Risk, ToolSpec};

/// Tool name.
pub const NAME: &str = "consult";

/// Tool description (R1.3).
const DESCRIPTION: &str = "Ask the reviewer model that has read this transcript one concrete question and get a short answer. Use before committing to an approach that is not obvious, when an error recurs, and once before you declare a task done. One question per call.";

/// `question` bound (chars).
pub const QUESTION_CHARS: usize = 2000;
/// `context` bound (chars).
pub const CONTEXT_CHARS: usize = 4000;
/// `partial` bound inside the block (bytes; the middle is elided beyond it).
const PARTIAL_BYTES: usize = 4 * 1024;

/// `consult` arguments.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ConsultArgs {
    /// One concrete question. Name the decision, not the whole task.
    #[schemars(length(max = 2000))]
    pub question: String,
    /// What you want weighed: the options you see, the evidence you have, what you already tried.
    #[schemars(length(max = 4000))]
    pub context: Option<String>,
}

/// The direct tool spec pushed beside `advise` (R1.3 availability).
pub fn spec() -> ToolSpec {
    ToolSpec {
        name: NAME.into(),
        description: DESCRIPTION.into(),
        schema: serde_json::to_value(schemars::schema_for!(ConsultArgs)).unwrap_or_default(),
        risk: Risk::ReadOnly,
        direct: true,
        returns: "str".into(),
    }
}

/// `consult: advisor off`: no link, or the asker is not a Chat session.
pub const ERR_OFF: &str = "consult: advisor off";
/// `consult: timed out after 90 s`: no answer within [`super::CONSULT_TIMEOUT`].
pub const ERR_TIMEOUT: &str = "consult: timed out after 90 s";
/// `question` empty or over [`QUESTION_CHARS`] (R1.3 errors table).
pub const ERR_QUESTION: &str = "consult: question must be 1-2000 chars";
/// `context` over [`CONTEXT_CHARS`].
pub const ERR_CONTEXT: &str = "consult: context must be at most 4000 chars";

/// `consult: cap reached (6/6 this turn)`.
pub fn err_cap(n: u32, cap: u8) -> String {
    format!("consult: cap reached ({n}/{cap} this turn)")
}

/// `consult: advisor unavailable (<reason, first line>)`.
pub fn err_unavailable(reason: &str) -> String {
    let line = reason.lines().next().unwrap_or_default().trim();
    format!("consult: advisor unavailable ({line})")
}

/// Parse the model's arguments against the R1.3 bounds; the error is the
/// tool result's whole text.
pub fn parse(args: &serde_json::Value) -> Result<(String, Option<String>), String> {
    if args.get("question").is_none() {
        return Err(ERR_QUESTION.into());
    }
    let a: ConsultArgs =
        serde_json::from_value(args.clone()).map_err(|e| format!("consult: {e}"))?;
    let question = a.question.trim();
    if question.is_empty() || question.chars().count() > QUESTION_CHARS {
        return Err(ERR_QUESTION.into());
    }
    let context = a.context.map(|c| c.trim().to_owned()).filter(|c| !c.is_empty());
    if context
        .as_ref()
        .is_some_and(|c| c.chars().count() > CONTEXT_CHARS)
    {
        return Err(ERR_CONTEXT.into());
    }
    Ok((question.to_owned(), context))
}

/// The `### Consultation` block (R1.4 step 4), appended after an update's
/// `---` line; `partial` is clipped to 4 KiB with its middle elided.
pub fn block(partial: &str, question: &str, context: Option<&str>) -> String {
    let partial = partial.trim();
    let partial = if partial.is_empty() {
        "(none)".to_owned()
    } else {
        clip_middle(partial, PARTIAL_BYTES)
    };
    format!(
        "### Consultation\n\
         The agent asks you directly. Answer in text (not with `advise`): <= 12 lines, the recommendation first.\n\
         \n\
         _agent's current step (not yet in the transcript):_\n\
         {partial}\n\
         \n\
         **question**: {question}\n\
         **context**: {}",
        context.map(str::trim).filter(|c| !c.is_empty()).unwrap_or("(none)")
    )
}

/// `s` with its middle replaced by an elision marker once it exceeds `max` bytes.
fn clip_middle(s: &str, max: usize) -> String {
    const MARK: &str = "\n[…middle elided…]\n";
    if s.len() <= max {
        return s.to_owned();
    }
    let keep = (max - MARK.len()) / 2;
    let mut head = keep;
    while !s.is_char_boundary(head) {
        head -= 1;
    }
    let mut tail = s.len() - keep;
    while !s.is_char_boundary(tail) {
        tail += 1;
    }
    format!("{}{MARK}{}", &s[..head], &s[tail..])
}

// ---- the one-shot system prompt (R1.6), spliced from system.md at compile time ----

const SYSTEM: &str = include_str!("system.md");
const WORKFLOW_OPEN: &str = "<workflow>";
const WORKFLOW_CLOSE: &str = "</workflow>";
const CAP_PLACEHOLDER: &str = "{{max_notes_per_update}}";
const ONE_SHOT_WORKFLOW: &str = "<workflow>\n\
You are reading one native subagent's whole transcript, rendered once; its brief is the first user\n\
message and its parent's goals are only what the brief says. You have no tools and cannot verify\n\
anything: answer from the transcript and say when you cannot. There is no `advise` tool: your whole\n\
reply is the answer.\n\
</workflow>";

/// Byte offset of `needle` in `hay`; a missing marker fails the build.
const fn find(hay: &str, needle: &str) -> usize {
    let (h, n) = (hay.as_bytes(), needle.as_bytes());
    let mut i = 0;
    while i + n.len() <= h.len() {
        let mut j = 0;
        while j < n.len() && h[i + j] == n[j] {
            j += 1;
        }
        if j == n.len() {
            return i;
        }
        i += 1;
    }
    panic!("system.md: marker missing")
}

const HEAD: &str = SYSTEM.split_at(find(SYSTEM, WORKFLOW_OPEN)).0;
const AFTER_WORKFLOW: &str = SYSTEM
    .split_at(find(SYSTEM, WORKFLOW_CLOSE) + WORKFLOW_CLOSE.len())
    .1;
const BEFORE_CAP: &str = AFTER_WORKFLOW
    .split_at(find(AFTER_WORKFLOW, CAP_PLACEHOLDER))
    .0;
const AFTER_CAP: &str = AFTER_WORKFLOW
    .split_at(find(AFTER_WORKFLOW, CAP_PLACEHOLDER) + CAP_PLACEHOLDER.len())
    .1
    .trim_ascii_end();
const PARTS: &[&str] = &[HEAD, ONE_SHOT_WORKFLOW, BEFORE_CAP, "0", AFTER_CAP];

const fn total(parts: &[&str]) -> usize {
    let mut n = 0;
    let mut p = 0;
    while p < parts.len() {
        n += parts[p].len();
        p += 1;
    }
    n
}

const fn splice<const N: usize>(parts: &[&str]) -> [u8; N] {
    let mut out = [0u8; N];
    let mut o = 0;
    let mut p = 0;
    while p < parts.len() {
        let b = parts[p].as_bytes();
        let mut i = 0;
        while i < b.len() {
            out[o] = b[i];
            o += 1;
            i += 1;
        }
        p += 1;
    }
    assert!(o == N);
    out
}

const ONE_SHOT_BYTES: [u8; total(PARTS)] = splice(PARTS);

/// `system.md` with `<workflow>` replaced by the one-shot variant and the
/// note cap set to `0` (R1.6); `<project-context>` / `<attention>` are
/// appended at call time as `identity()` does for the child.
pub const ONE_SHOT_SYSTEM: &str = match std::str::from_utf8(&ONE_SHOT_BYTES) {
    Ok(s) => s,
    Err(_) => panic!("system.md: not UTF-8"),
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_is_direct_read_only_with_the_two_bounded_args() {
        let s = spec();
        assert_eq!(s.name, "consult");
        assert!(s.direct);
        assert_eq!(s.risk, Risk::ReadOnly);
        assert_eq!(s.returns, "str");
        let props = &s.schema["properties"];
        assert_eq!(props["question"]["maxLength"], 2000);
        assert_eq!(props["context"]["maxLength"], 4000);
        assert_eq!(s.schema["required"], serde_json::json!(["question"]));
    }

    #[test]
    fn parse_enforces_the_bounds() {
        let ok = parse(&serde_json::json!({"question": " which first? ", "context": "  "})).unwrap();
        assert_eq!(ok, ("which first?".into(), None));
        let ok = parse(&serde_json::json!({"question": "q", "context": "a vs b"})).unwrap();
        assert_eq!(ok.1.as_deref(), Some("a vs b"));
        assert_eq!(parse(&serde_json::json!({})).unwrap_err(), ERR_QUESTION);
        assert_eq!(
            parse(&serde_json::json!({"question": "   "})).unwrap_err(),
            ERR_QUESTION
        );
        assert_eq!(
            parse(&serde_json::json!({"question": "é".repeat(2001)})).unwrap_err(),
            ERR_QUESTION
        );
        assert!(parse(&serde_json::json!({"question": "é".repeat(2000)})).is_ok());
        assert_eq!(
            parse(&serde_json::json!({"question": "q", "context": "x".repeat(4001)})).unwrap_err(),
            ERR_CONTEXT
        );
        assert!(parse(&serde_json::json!({"question": "q", "context": "x".repeat(4000)})).is_ok());
    }

    #[test]
    fn block_has_the_r1_4_shape_and_clips_the_partial() {
        let b = block("I will read the justfile first.", "justfile or README?", None);
        assert_eq!(
            b,
            "### Consultation\n\
             The agent asks you directly. Answer in text (not with `advise`): <= 12 lines, the recommendation first.\n\
             \n\
             _agent's current step (not yet in the transcript):_\n\
             I will read the justfile first.\n\
             \n\
             **question**: justfile or README?\n\
             **context**: (none)"
        );
        let b = block("", "q", Some("a vs b"));
        assert!(b.contains("_agent's current step (not yet in the transcript):_\n(none)\n"));
        assert!(b.ends_with("**question**: q\n**context**: a vs b"));
        let long = "é".repeat(5000);
        let b = block(&long, "q", None);
        assert!(b.contains("[…middle elided…]"));
        let partial = b
            .split("transcript):_\n")
            .nth(1)
            .unwrap()
            .split("\n\n**question**")
            .next()
            .unwrap();
        assert!(partial.len() <= 4 * 1024, "{}", partial.len());
        assert!(partial.starts_with("éé") && partial.ends_with("éé"));
    }

    #[test]
    fn error_strings_are_the_fixed_forms() {
        assert_eq!(ERR_OFF, "consult: advisor off");
        assert_eq!(ERR_TIMEOUT, "consult: timed out after 90 s");
        assert_eq!(err_cap(6, 6), "consult: cap reached (6/6 this turn)");
        assert_eq!(
            err_unavailable("paused: 429 rate limit\nmore detail"),
            "consult: advisor unavailable (paused: 429 rate limit)"
        );
        assert_eq!(
            super::super::CONSULT_TIMEOUT,
            std::time::Duration::from_secs(90)
        );
    }

    #[test]
    fn one_shot_system_swaps_the_workflow_and_zeroes_the_cap() {
        assert!(ONE_SHOT_SYSTEM.starts_with("<conventions>"));
        assert!(ONE_SHOT_SYSTEM.contains(ONE_SHOT_WORKFLOW));
        assert!(!ONE_SHOT_SYSTEM.contains("You receive incremental transcript updates"));
        assert!(ONE_SHOT_SYSTEM.contains("at most 0 non-blockers per update"));
        assert!(!ONE_SHOT_SYSTEM.contains(CAP_PLACEHOLDER));
        assert_eq!(ONE_SHOT_SYSTEM.matches("<workflow>").count(), 1);
        assert!(ONE_SHOT_SYSTEM.contains("<completeness>"));
        assert!(ONE_SHOT_SYSTEM.ends_with(SYSTEM.trim_end().rsplit_once("</completeness>").unwrap().1.trim()));
    }
}
