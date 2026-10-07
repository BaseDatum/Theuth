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
    let mut out = SYSTEM
        .trim_end()
        .replace("{{max_notes_per_update}}", &max_notes.clamp(1, 32).to_string());
    let prompt = std::fs::read_to_string(data_dir.join("prompt.md")).unwrap_or_default();
    let prompt = if prompt.len() > PROJECT_BYTES {
        let mut cut = PROJECT_BYTES;
        while !prompt.is_char_boundary(cut) {
            cut -= 1;
        }
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
        let dir = std::env::temp_dir().join(format!("theuth-adv-id-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("prompt.md"), "Always run the check.\n").unwrap();
        let bare = identity(&dir, 4);
        assert!(bare.starts_with("<conventions>"));
        assert!(bare.contains("at most 4 non-blockers per update"));
        assert!(bare.contains("<file path=\"~/.theuth/prompt.md\">\nAlways run the check.\n</file>"));
        assert!(!bare.contains("<attention>"));
        std::fs::write(dir.join(ATTENTION_FILE), "Watch the deploys.\n").unwrap();
        let with = identity(&dir, 40);
        assert!(with.contains("at most 32 non-blockers"));
        assert!(with.ends_with("Especially pay attention to:\n<attention>\nWatch the deploys.\n</attention>"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
