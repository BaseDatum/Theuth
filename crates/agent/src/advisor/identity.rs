//! 125 §5.1–§5.2: the advisor's system prompt — the adapted omp advisor
//! prompt (MIT; `src/prompts/advisor/system.md`) with the advised session's
//! standing instructions in `<project-context>` and the optional
//! `~/.theuth/advisor.md` in `<attention>`, both read live per turn.

use std::path::Path;

/// The §5.1 text, `{{max_notes_per_update}}` unsubstituted.
const SYSTEM: &str = include_str!("system.md");

/// `prompt.md` cap inside `<project-context>`.
const PROJECT_BYTES: usize = 32 * 1024;

/// The one customisation surface (D2).
pub const ATTENTION_FILE: &str = "advisor.md";

/// The advisor identity for a session whose data dir is `data_dir`.
pub fn identity(data_dir: &Path, max_notes: u8) -> String {
    with_context(
        data_dir,
        SYSTEM.trim_end().replace(
            "{{max_notes_per_update}}",
            &max_notes.clamp(1, 32).to_string(),
        ),
    )
}

/// R1.6: the system prompt of a subagent's one-shot consultation — the
/// tool-less variant with the same standing instructions appended.
pub fn one_shot(data_dir: &Path) -> String {
    with_context(data_dir, super::consult::ONE_SHOT_SYSTEM.to_owned())
}

/// `body` plus the live `<project-context>` and optional `<attention>` blocks.
fn with_context(data_dir: &Path, mut out: String) -> String {
    let prompt = std::fs::read_to_string(data_dir.join("prompt.md")).unwrap_or_default();
    let prompt = if prompt.len() > PROJECT_BYTES {
        let cut = prompt.floor_char_boundary(PROJECT_BYTES);
        format!("{}\n…[prompt.md elided to fit]", &prompt[..cut])
    } else {
        prompt
    };
    out.push_str(&format!(
        "\n\n<project-context>\nStanding instructions of the agent you watch (its prompt.md): binding on it. Enforce them; flag drift immediately; NEVER advise against them.\n<file path=\"~/.theuth/prompt.md\">\n{}\n</file>\n</project-context>",
        prompt.trim()
    ));
    if let Ok(attention) = std::fs::read_to_string(data_dir.join(ATTENTION_FILE))
        && !attention.trim().is_empty()
    {
        out.push_str(&format!(
            "\n\nEspecially pay attention to:\n<attention>\n{}\n</attention>",
            attention.trim()
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembles_prompt_project_context_and_attention() {
        let scratch = crate::TestDir::new("adv-id");
        let dir = scratch.0.clone();
        std::fs::write(dir.join("prompt.md"), "Always run the check.\n").unwrap();
        let bare = identity(&dir, 4);
        assert!(bare.starts_with("<conventions>"));
        assert!(bare.contains("at most 4 non-blockers per update"));
        assert!(
            bare.contains("<file path=\"~/.theuth/prompt.md\">\nAlways run the check.\n</file>")
        );
        assert!(!bare.contains("<attention>"));
        std::fs::write(dir.join(ATTENTION_FILE), "Watch the deploys.\n").unwrap();
        let with = identity(&dir, 40);
        assert!(with.contains("at most 32 non-blockers"));
        assert!(with.contains("<consultation>"));
        assert!(with.ends_with(
            "Especially pay attention to:\n<attention>\nWatch the deploys.\n</attention>"
        ));
        // R1.6: the one-shot has no tools, no `advise`, the same standing blocks.
        let one = one_shot(&dir);
        assert!(one.contains("There is no `advise` tool") && one.contains("<consultation>"));
        assert!(!one.contains("read_file, grep"));
        assert!(one.contains("Always run the check.") && one.ends_with("</attention>"));
    }
}
