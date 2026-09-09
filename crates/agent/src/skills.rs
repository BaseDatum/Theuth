//! Skills: markdown files with YAML front matter (`name`, `description`) under
//! `<data_dir>/skills/**/*.md`. The index goes in the system prompt; a body is
//! loaded on demand through the `load_skill` tool.

use std::path::Path;
use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tools::{Risk, Tool, ToolCx, ToolOutput};

/// One parsed skill file.
#[derive(Clone, Debug)]
pub struct Skill {
    /// From front matter; defaults to the file stem.
    pub name: String,
    /// From front matter; shown in the system prompt index.
    pub description: String,
    /// Markdown body without front matter.
    pub body: String,
}

/// Loaded skill set. Cheap to clone.
#[derive(Clone, Debug, Default)]
pub struct Skills {
    // filled in by the skills implementation
}

impl Skills {
    /// Load every `*.md` under `dir` (recursively). Missing dir → empty set.
    /// Files without valid front matter are skipped with a `tracing::warn!`.
    pub fn load(dir: &Path) -> Self {
        let _ = dir;
        todo!("skills implementation")
    }

    /// Text for the system prompt: one line per skill, or empty string when none.
    pub fn prompt_index(&self) -> String {
        todo!("skills implementation")
    }

    /// Body by name.
    pub fn get(&self, name: &str) -> Option<&Skill> {
        let _ = name;
        todo!("skills implementation")
    }
}

/// Arguments for [`LoadSkillTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct LoadSkillArgs {
    /// Skill name from the index.
    pub name: String,
}

/// Returns a skill body to the model.
pub struct LoadSkillTool {
    /// Shared skill set.
    pub skills: Arc<Skills>,
}

#[async_trait::async_trait]
impl Tool for LoadSkillTool {
    const NAME: &'static str = "load_skill";
    const RISK: Risk = Risk::ReadOnly;
    type Args = LoadSkillArgs;

    fn description(&self) -> String {
        "Load the full instructions for a skill listed in the system prompt.".into()
    }

    async fn call(&self, args: LoadSkillArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        Ok(match self.skills.get(&args.name) {
            Some(s) => ToolOutput::ok(s.body.clone()),
            None => ToolOutput::err(format!("no skill named {}", args.name)),
        })
    }
}
