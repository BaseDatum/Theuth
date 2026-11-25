//! Owner: A1 — the `## User cells` prompt block (design 171 §10.1, verbatim) and the bundled
//! `cells` skill (§10.2–10.5). The block is a constant: prompt blocks never vary per session
//! (the frame-cache rule of `agent::frame`).

use serde::{Deserialize, Serialize};

/// The `RegisterPrompt{plugin: "cells"}` text; `prompt_blocks` drops it for unattended
/// sessions (171 §10.6).
pub const PROMPT: &str = r#"## User cells
A user cell is a fenced block the user runs with one press in theuth: ```bash, ```zsh, ```sh, ```pwsh or ```python whose first lines are `#| key: value` options. A fence without `#|` lines stays a snippet to read — never use one for a command you are asking the user to run; that is a cell.
Present cells instead of running anything when the user asks you to prepare but not run ("prep it, don't apply", "give me the commands", "I'll run it", "step by step, I'll review each output", "make me a run sheet"), when a step changes production or shared state you were not told to change, needs their login, SSO or MFA, is interactive, or they want to watch it.
That mode is sticky for the work until the user lifts it: read-only checks stay yours (unless they said hands off entirely); every changing step becomes a cell.
Every cell gets `#| cwd:`: absolute or ~/… in chat; in a note ~/… or relative to the note's ```cells``` default, since a note's cells run on whichever machine presses them. One action per cell, checks before changes, explicit --profile, --context or -s on every command.
Every changing step gets `#| label:` naming its consequence in at most 40 characters ("Apply to ir-prod1"; longer labels show truncated) and `#| danger`. Use `#| after:` sparingly: only on an apply, naming its preview's `#| name:` — it is advisory (the user may run any cell in any order; a press before the preview ran asks to confirm), never an ordering hint for read-only steps. Take `#| expect:` from the run sheet's "You should see"; add `#| review` when you promised to read the output.
Secrets go in `#| env: KEY={{secret:NAME}}` and the code uses $KEY. Never add --yes or -auto-approve to a danger cell unless asked.
Python: `#| python: monty` for pure computation on the standard library (no packages, no network, no subprocess, no files outside the cwd); otherwise the project's interpreter (auto, .venv, uv or a path) — interpreters(cwd) lists them for your own machine.
Up to five one-off steps go in chat. Longer or repeatable procedures go in a note (doc_insert the same fences) with one line in chat linking it.
When a run ends you receive <cell_result>: check each expect, report the counts that matter in one or two lines, and propose the next cell or a corrected one under the same name.
You never run a user cell, never run its command yourself, never type into its terminal, never tick a run sheet's boxes. Load the cells skill before writing a runbook."#;

/// The bundled skill, `<data_dir>/skills/cells.md`.
pub const SKILL: &str = include_str!("../cells.md");

/// A1's keys of the `cells:` section (flattened into [`crate::CellsConfig`]), and 220's cell
/// nudge keys (read by the agent's `CellsKeys`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PromptConfig {
    /// 220: after a finished turn, promote a plain run-me fence of the reply to a user cell.
    pub nudge: bool,
    /// 220: automatic promotions per session (in memory, per engine run).
    pub nudge_max: u8,
}

impl Default for PromptConfig {
    fn default() -> Self {
        Self {
            nudge: true,
            nudge_max: 3,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §10.2: the bundled skill indexes as `cells` with its description, teaches every
    /// option of §3.2, and the cells it presents as examples are cells the user can run.
    #[test]
    fn the_skill_indexes_and_covers_every_option() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("cells.md"), SKILL).unwrap();
        let index = agent::skills::index(dir.path());
        assert_eq!(index.len(), 1);
        assert_eq!(index[0].name, "cells");
        assert!(
            index[0]
                .description
                .starts_with("Write user cells and runbooks")
        );
        for option in [
            "cwd", "name", "label", "danger", "after", "expect", "env", "python", "review",
            "private", "timeout",
        ] {
            assert!(SKILL.contains(&format!("| `{option}` |")), "{option}");
        }
        let examples = surface::cell_link::cells(SKILL);
        assert!(!examples.is_empty());
        let names = examples
            .iter()
            .filter_map(|(_, c)| c.options.name.clone())
            .collect();
        let ctx = surface::cell::ValidateCtx {
            chat: true,
            names,
            ..Default::default()
        };
        for (_, cell) in &examples {
            let errors: Vec<_> = surface::cell::validate(cell, &ctx)
                .into_iter()
                .filter(|p| p.severity == surface::cell::Severity::Error)
                .collect();
            assert!(errors.is_empty(), "{:?}: {errors:?}", cell.options.name);
        }
    }
}
