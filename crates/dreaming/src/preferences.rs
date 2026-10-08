//! Standing preferences (design doc 136 §7.1 rung 3): `<data_dir>/dreaming/preferences.md`,
//! one line per preference,
//!
//! ```text
//! - <text> [evidence: <session ids | user>; since: YYYY-MM-DD; confirmed: YYYY-MM-DD]
//! ```
//!
//! ≤ `cap_lines` lines × 140 chars, surfaced as the plugin's prompt block
//! "## Standing preferences (learned)" when non-empty. `set` is the
//! `preferences_set` tool: `add` (a line already present is re-confirmed:
//! evidence union, `confirmed` today), `replace` (`line` = `"<old> => <new>"`),
//! `demote` (the line leaves the block and becomes a knowledge `decision`,
//! so nothing is lost). The block is re-registered after every write.

use std::path::{Path, PathBuf};

use chrono::Utc;
use kameo::actor::ActorRef;

use crate::night::write_atomic;
use crate::scrub::scrub;

/// The block heading (136 §7.1).
pub const HEADING: &str = "## Standing preferences (learned)";
/// Chars per line (136 §7.1).
pub const MAX_CHARS: usize = 140;
/// The `replace` separator.
pub const REPLACE_SEP: &str = " => ";

/// `<data_dir>/dreaming/preferences.md`.
pub fn path(data_dir: &Path) -> PathBuf {
    crate::night::dir(data_dir).join("preferences.md")
}

/// One line of the block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    /// The preference.
    pub text: String,
    /// Session ids / `user`.
    pub evidence: Vec<String>,
    /// `YYYY-MM-DD` first written.
    pub since: String,
    /// `YYYY-MM-DD` last confirmed (demotion picks the oldest).
    pub confirmed: String,
}

impl Line {
    fn render(&self) -> String {
        format!(
            "- {} [evidence: {}; since: {}; confirmed: {}]",
            self.text,
            self.evidence.join(", "),
            self.since,
            self.confirmed
        )
    }
}

/// Parse the file's lines.
pub fn parse(text: &str) -> Vec<Line> {
    text.lines()
        .filter_map(|l| {
            let body = l.trim().strip_prefix("- ")?;
            let (t, meta) = match body.rfind(" [") {
                Some(i) if body.ends_with(']') => (&body[..i], &body[i + 2..body.len() - 1]),
                _ => (body, ""),
            };
            let mut line = Line {
                text: t.trim().to_owned(),
                evidence: Vec::new(),
                since: String::new(),
                confirmed: String::new(),
            };
            for kv in meta.split(';') {
                let Some((k, v)) = kv.split_once(':') else { continue };
                let v = v.trim();
                match k.trim() {
                    "evidence" => {
                        line.evidence = v
                            .split(',')
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_owned)
                            .collect();
                    }
                    "since" => line.since = v.to_owned(),
                    "confirmed" => line.confirmed = v.to_owned(),
                    _ => {}
                }
            }
            Some(line)
        })
        .collect()
}

/// Render lines as the file text.
pub fn render(lines: &[Line]) -> String {
    let mut s = lines.iter().map(Line::render).collect::<Vec<_>>().join("\n");
    if !s.is_empty() {
        s.push('\n');
    }
    s
}

/// The prompt block: heading + the preference texts (one `- ` bullet each,
/// metadata stripped); `None` when the file is absent or empty.
pub fn block(data_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path(data_dir)).ok()?;
    let lines = parse(&text);
    if lines.is_empty() {
        return None;
    }
    Some(format!(
        "{HEADING}\n{}",
        lines
            .iter()
            .map(|l| format!("- {}", l.text))
            .collect::<Vec<_>>()
            .join("\n")
    ))
}

/// Lines in the block.
pub fn count(data_dir: &Path) -> u64 {
    std::fs::read_to_string(path(data_dir))
        .map(|t| parse(&t).len() as u64)
        .unwrap_or(0)
}

/// Re-register (or unregister) the prompt block with the tool registry.
pub async fn republish(data_dir: &Path) {
    let Ok(Some(reg)) = ActorRef::<tools::registry::ToolRegistry>::lookup(tools::REGISTRY_ACTOR)
    else {
        return;
    };
    match block(data_dir) {
        Some(text) => {
            let _ = reg
                .ask(tools::RegisterPrompt {
                    plugin: "dreaming",
                    text,
                })
                .await;
        }
        None => {
            let _ = reg.ask(tools::UnregisterPrompt("dreaming")).await;
        }
    }
}

/// `preferences_set` (136 §7.1): `op` ∈ add | replace | demote; returns the
/// block text ("" when the block is empty). `knowledge` receives the demoted
/// line as a `decision` when available.
pub async fn set(
    data_dir: &Path,
    knowledge: Option<&knowledge::Knowledge>,
    op: &str,
    line: &str,
    evidence: &[String],
    cap_lines: u32,
) -> anyhow::Result<String> {
    let today = Utc::now().format("%Y-%m-%d").to_string();
    let mut lines = parse(&std::fs::read_to_string(path(data_dir)).unwrap_or_default());
    let check = |t: &str| -> anyhow::Result<String> {
        let t = scrub(t.trim());
        anyhow::ensure!(!t.is_empty(), "empty line");
        anyhow::ensure!(!t.contains('\n'), "one line per preference");
        anyhow::ensure!(
            t.chars().count() <= MAX_CHARS,
            "{} chars; the cap is {MAX_CHARS}",
            t.chars().count()
        );
        Ok(t)
    };
    let evidence: Vec<String> = evidence
        .iter()
        .map(|e| scrub(e.trim()))
        .filter(|e| !e.is_empty())
        .collect();
    match op {
        "add" => {
            let text = check(line)?;
            if let Some(l) = lines.iter_mut().find(|l| l.text == text) {
                for e in &evidence {
                    if !l.evidence.contains(e) {
                        l.evidence.push(e.clone());
                    }
                }
                l.confirmed = today;
            } else {
                anyhow::ensure!(
                    lines.len() < cap_lines as usize,
                    "the block is full ({cap_lines} lines); demote one first (the oldest confirmed is {:?})",
                    oldest(&lines).map(|l| l.text.as_str()).unwrap_or("")
                );
                lines.push(Line {
                    text,
                    evidence,
                    since: today.clone(),
                    confirmed: today,
                });
            }
        }
        "replace" => {
            let (old, new) = line
                .split_once(REPLACE_SEP)
                .ok_or_else(|| anyhow::anyhow!("replace takes `<old line> => <new line>`"))?;
            let new = check(new)?;
            let old = old.trim();
            let l = lines
                .iter_mut()
                .find(|l| l.text == old)
                .ok_or_else(|| anyhow::anyhow!("no line {old:?} in the block"))?;
            l.text = new;
            for e in &evidence {
                if !l.evidence.contains(e) {
                    l.evidence.push(e.clone());
                }
            }
            l.confirmed = today;
        }
        "demote" => {
            let text = line.trim();
            let i = lines
                .iter()
                .position(|l| l.text == text)
                .ok_or_else(|| anyhow::anyhow!("no line {text:?} in the block"))?;
            let l = lines.remove(i);
            if let Some(k) = knowledge {
                let slug: String = l
                    .text
                    .to_lowercase()
                    .chars()
                    .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                    .collect::<String>()
                    .split('-')
                    .filter(|s| !s.is_empty())
                    .take(8)
                    .collect::<Vec<_>>()
                    .join("-");
                let put = knowledge::Put {
                    id: format!("decisions/preference-{slug}"),
                    type_: "decision".into(),
                    title: l.text.clone(),
                    description: Some("A standing preference demoted from the system prompt block".into()),
                    tags: vec!["preference".into(), "dreaming".into()],
                    body: format!(
                        "{}\n\nDemoted from the standing preferences block on {today} (in the block since {}, last confirmed {}). Evidence: {}.",
                        l.text,
                        l.since,
                        l.confirmed,
                        if l.evidence.is_empty() { "none recorded".to_owned() } else { l.evidence.join(", ") }
                    ),
                    evidence: l.evidence.clone(),
                    status: Some(knowledge::Status::Tidy),
                    ..Default::default()
                };
                if let Err(e) = k.put(put, knowledge::Source::Dreaming).await {
                    tracing::warn!("dreaming: demoted preference not written to knowledge: {e:#}");
                }
            }
        }
        other => anyhow::bail!("op {other:?}: add | replace | demote"),
    }
    write_atomic(&path(data_dir), render(&lines).as_bytes())?;
    republish(data_dir).await;
    Ok(block(data_dir).unwrap_or_default())
}

/// The line with the oldest `confirmed` date (136 §4 Settle: demoted when the block is full).
pub fn oldest(lines: &[Line]) -> Option<&Line> {
    lines.iter().min_by(|a, b| a.confirmed.cmp(&b.confirmed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_is_none_for_absent_or_blank_and_heading_plus_lines_otherwise() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(block(dir.path()), None);
        std::fs::create_dir_all(crate::night::dir(dir.path())).unwrap();
        std::fs::write(path(dir.path()), "\n  \n").unwrap();
        assert_eq!(block(dir.path()), None);
        std::fs::write(
            path(dir.path()),
            "- never use ask_user; numbered text [evidence: user; since: 2026-10-01; confirmed: 2026-10-01]\n\n- commit as you go  \n",
        )
        .unwrap();
        assert_eq!(
            block(dir.path()).unwrap(),
            "## Standing preferences (learned)\n- never use ask_user; numbered text\n- commit as you go"
        );
    }

    #[tokio::test]
    async fn add_confirms_replace_renames_demote_removes_and_the_cap_holds() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let ev = |s: &str| vec![s.to_owned()];
        let b = set(d, None, "add", "never use ask_user; numbered text", &ev("user"), 3)
            .await
            .unwrap();
        assert_eq!(b, "## Standing preferences (learned)\n- never use ask_user; numbered text");
        // Re-adding confirms: still one line, evidence union.
        set(d, None, "add", "never use ask_user; numbered text", &ev("s2"), 3)
            .await
            .unwrap();
        let lines = parse(&std::fs::read_to_string(path(d)).unwrap());
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].evidence, ["user", "s2"]);
        set(d, None, "add", "commit as you go", &[], 3).await.unwrap();
        set(d, None, "add", "caveman replies", &[], 3).await.unwrap();
        let e = set(d, None, "add", "one more", &[], 3).await.unwrap_err().to_string();
        assert!(e.contains("full"), "{e}");
        let e = set(d, None, "add", &"x".repeat(141), &[], 20)
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("141 chars"), "{e}");
        assert!(set(d, None, "replace", "no separator", &[], 20).await.is_err());
        let b = set(d, None, "replace", "commit as you go => commit every 20 minutes", &[], 20)
            .await
            .unwrap();
        assert!(b.contains("- commit every 20 minutes") && !b.contains("commit as you go"));
        let b = set(d, None, "demote", "caveman replies", &[], 20).await.unwrap();
        assert!(!b.contains("caveman"));
        assert_eq!(count(d), 2);
        assert!(set(d, None, "shout", "x", &[], 20).await.is_err());
        // Secrets never enter the block.
        let b = set(d, None, "add", "use key sk-abcdefghijklmnop", &[], 20).await.unwrap();
        assert!(b.contains("[redacted]") && !b.contains("sk-abc"));
    }
}
