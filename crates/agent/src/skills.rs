//! Skills: markdown files with YAML front matter (`name`, `description`) under
//! `<data_dir>/skills/**/*.md`. The index goes in the system prompt; a body is
//! loaded on demand through the `load_skill` tool.

use std::collections::BTreeMap;
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
    by_name: BTreeMap<String, Skill>,
}

#[derive(Deserialize, Default)]
struct FrontMatter {
    name: Option<String>,
    description: Option<String>,
}

impl Skills {
    /// Load every `*.md` under `dir` (recursively). Missing dir → empty set.
    /// Files without front matter are still skills (name = file stem,
    /// description = first non-empty line). Duplicate names: last wins, with a
    /// `tracing::warn!`.
    pub fn load(dir: &Path) -> Self {
        let mut files = Vec::new();
        collect_md(dir, &mut files);
        files.sort();
        let matter = gray_matter::Matter::<gray_matter::engine::YAML>::new();
        let mut by_name = BTreeMap::new();
        for path in files {
            let text = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!(?path, %e, "skill unreadable, skipped");
                    continue;
                }
            };
            let stem = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let (fm, body) = match matter.parse::<FrontMatter>(&text) {
                Ok(p) => (p.data.unwrap_or_default(), p.content),
                Err(e) => {
                    tracing::warn!(?path, %e, "skill front matter invalid, using file as body");
                    (FrontMatter::default(), text)
                }
            };
            let name = fm.name.filter(|n| !n.trim().is_empty()).unwrap_or(stem);
            let description = fm.description.unwrap_or_else(|| {
                body.lines()
                    .map(str::trim)
                    .find(|l| !l.is_empty())
                    .unwrap_or_default()
                    .to_owned()
            });
            if by_name.contains_key(&name) {
                tracing::warn!(?path, name, "duplicate skill name, last wins");
            }
            by_name.insert(
                name.clone(),
                Skill {
                    name,
                    description,
                    body,
                },
            );
        }
        Self { by_name }
    }

    /// Text for the system prompt: one line per skill, or empty string when none.
    pub fn prompt_index(&self) -> String {
        if self.by_name.is_empty() {
            return String::new();
        }
        let mut s =
            String::from("\n## Skills\nCall `load_skill` with a name to get full instructions.\n");
        for (name, skill) in &self.by_name {
            s.push_str("- ");
            s.push_str(name);
            s.push_str(": ");
            s.push_str(&skill.description);
            s.push('\n');
        }
        s
    }

    /// Body by name.
    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.by_name.get(name)
    }
}

fn collect_md(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_md(&path, out);
        } else if path.extension().is_some_and(|e| e == "md") {
            out.push(path);
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_index_and_bodies() {
        let dir = std::env::temp_dir().join(format!("wth-skills-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("a")).unwrap();
        std::fs::write(
            dir.join("a/one.md"),
            "---\nname: one\ndescription: First skill\n---\n# One\nbody one\n",
        )
        .unwrap();
        std::fs::write(dir.join("two.md"), "\nSecond skill line\nmore\n").unwrap();
        std::fs::write(dir.join("three.txt"), "ignored").unwrap();

        let skills = Skills::load(&dir);
        std::fs::remove_dir_all(&dir).unwrap();

        assert_eq!(skills.by_name.len(), 2);
        let idx = skills.prompt_index();
        let one = idx.find("- one: First skill\n").expect("one in index");
        let two = idx
            .find("- two: Second skill line\n")
            .expect("two in index");
        assert!(one < two);
        let body = &skills.get("one").unwrap().body;
        assert!(
            !body.contains("description:"),
            "front matter leaked: {body}"
        );
        assert!(body.contains("body one"));
        assert_eq!(skills.get("two").unwrap().description, "Second skill line");
        assert!(skills.get("three").is_none());
        assert_eq!(Skills::load(&dir.join("missing")).prompt_index(), "");
    }
}
