//! Skills: markdown with YAML front matter (`name`, `description`, `always`)
//! under `<data_dir>/skills/` (design doc 04 §1.2, §2). The directory is the
//! registry: every function reads it fresh, so the system prompt (rebuilt per
//! round from [`index`]) reflects a change on the next model call.
//!
//! Walk: a directory holding `SKILL.md` is one skill (its subtree is not
//! walked); any other `*.md` is a skill unless it is a README/LICENSE/
//! CHANGELOG/CONTRIBUTING; `.git/` is skipped. A skill inside a git clone
//! directly under the dir has `repo = Some(<clone dir name>)`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use genai::chat::{ChatMessage, ChatRole};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tools::{Risk, Tool, ToolCx, ToolOutput};

use crate::loop_::text_of;
use crate::{AddSkill, SkillBody, SkillMeta};

/// Name of the model-facing loader tool ([`LoadSkillTool`]).
pub const LOAD: &str = "load_skill";

const IGNORED_STEMS: [&str; 4] = ["readme", "license", "changelog", "contributing"];

/// `<data_dir>/skills`.
pub fn dir(data_dir: &Path) -> PathBuf {
    data_dir.join("skills")
}

#[derive(Serialize, Deserialize, Default)]
struct FrontMatter {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    always: bool,
}

fn is_dir_skill(path: &Path) -> bool {
    path.file_name().is_some_and(|n| n == "SKILL.md")
}

/// Front matter and body of one skill file; invalid front matter → whole file is the body.
fn parse(path: &Path) -> std::io::Result<(FrontMatter, String)> {
    let text = std::fs::read_to_string(path)?;
    let matter = gray_matter::Matter::<gray_matter::engine::YAML>::new();
    Ok(match matter.parse::<FrontMatter>(&text) {
        Ok(p) => (p.data.unwrap_or_default(), p.content),
        Err(e) => {
            tracing::warn!(?path, %e, "skill front matter invalid, using file as body");
            (FrontMatter::default(), text)
        }
    })
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let skill = dir.join("SKILL.md");
    if skill.is_file() {
        out.push(skill);
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n != ".git") {
                walk(&path, out);
            }
        } else if path.extension().is_some_and(|e| e == "md")
            && !path.file_stem().is_some_and(|s| {
                IGNORED_STEMS.contains(&s.to_string_lossy().to_lowercase().as_str())
            })
        {
            out.push(path);
        }
    }
}

/// Every skill under `dir`, sorted by name. Missing dir → empty. Duplicate
/// names: last by sorted path wins, with a `tracing::warn!`.
pub fn index(dir: &Path) -> Vec<SkillMeta> {
    let mut files = Vec::new();
    walk(dir, &mut files);
    files.sort();
    let repos = tools::git::repos(dir);
    let mut by_name: BTreeMap<String, SkillMeta> = BTreeMap::new();
    for path in files {
        let (fm, body) = match parse(&path) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(?path, %e, "skill unreadable, skipped");
                continue;
            }
        };
        let name = fm.name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| {
            let named = if is_dir_skill(&path) {
                path.parent().and_then(Path::file_name)
            } else {
                path.file_stem()
            };
            named
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
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
        let repo = repos
            .iter()
            .find(|r| path.starts_with(&r.path))
            .map(|r| r.name.clone());
        by_name.insert(
            name.clone(),
            SkillMeta {
                name,
                description,
                path: path.display().to_string(),
                always: fm.always,
                repo,
            },
        );
    }
    by_name.into_values().collect()
}

/// Body without front matter; for `SKILL.md` skills also the relative paths
/// of the other files in its directory tree, sorted.
pub fn body(meta: &SkillMeta) -> std::io::Result<SkillBody> {
    let path = Path::new(&meta.path);
    let (_, body) = parse(path)?;
    let mut files = Vec::new();
    if is_dir_skill(path)
        && let Some(root) = path.parent()
    {
        collect_files(root, root, path, &mut files);
        files.sort();
    }
    Ok(SkillBody {
        name: meta.name.clone(),
        body,
        files,
    })
}

fn collect_files(root: &Path, dir: &Path, skip: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, skip, out);
        } else if path != skip
            && let Ok(rel) = path.strip_prefix(root)
        {
            out.push(rel.to_string_lossy().into_owned());
        }
    }
}

/// Skill by name, or `Err` naming it.
pub fn find(dir: &Path, name: &str) -> Result<SkillMeta, String> {
    index(dir)
        .into_iter()
        .find(|s| s.name == name)
        .ok_or_else(|| format!("no skill named {name}"))
}

/// Body of the skill named `name`.
pub fn load(dir: &Path, name: &str) -> Result<SkillBody, String> {
    body(&find(dir, name)?).map_err(|e| format!("{name}: {e}"))
}

/// What the model gets for a skill: the body and, for `SKILL.md` skills,
/// the paths of its other files. Shared by `load_skill` and [`UseSkill`](crate::UseSkill).
pub fn load_text(dir: &Path, name: &str) -> Result<String, String> {
    let meta = find(dir, name)?;
    let b = body(&meta).map_err(|e| format!("{}: {e}", meta.path))?;
    let mut s = b.body;
    if !b.files.is_empty() {
        let root = Path::new(&meta.path).parent().unwrap_or(dir);
        s.push_str(&format!("\n\nfiles (under {}):\n", root.display()));
        for f in &b.files {
            s.push_str("- ");
            s.push_str(f);
            s.push('\n');
        }
    }
    Ok(s)
}

/// A user-injected skill as one input (design doc 04 §2.4): the body in a
/// `<skill name="…">` wrapper UIs card, then the user's words.
pub fn inject_text(name: &str, text: &str, args: &str) -> String {
    let mut s = format!("<skill name=\"{name}\">\n{}\n</skill>", text.trim());
    if !args.trim().is_empty() {
        s.push_str("\n\n");
        s.push_str(args.trim());
    }
    s
}

/// Split an input made by [`inject_text`]: `(name, body, rest)`, `rest`
/// being the user's words (possibly empty). `None` for anything else.
pub fn injected(text: &str) -> Option<(&str, &str, &str)> {
    let rest = text.trim_start().strip_prefix("<skill name=\"")?;
    let (name, after) = rest.split_once("\">")?;
    let (body, rest) = after.split_once("</skill>")?;
    Some((name, body.trim(), rest.trim()))
}

/// Skills whose body is in these messages: user injections and `load_skill`
/// calls that did not error. The badge's truth on resume.
pub fn in_frame(messages: &[(i64, ChatMessage)]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut pending: HashMap<&str, &str> = HashMap::new();
    for (_, m) in messages {
        match m.role {
            ChatRole::User => {
                if let Some((name, _, _)) = injected(&text_of(&m.content)) {
                    out.insert(name.to_owned());
                }
            }
            ChatRole::Assistant => {
                for c in m.content.tool_calls() {
                    if c.fn_name == LOAD
                        && let Some(name) = c.fn_arguments.get("name").and_then(|v| v.as_str())
                    {
                        pending.insert(&c.call_id, name);
                    }
                }
            }
            ChatRole::Tool => {
                for r in m.content.tool_responses() {
                    if let Some(name) = pending.remove(r.call_id.as_str())
                        && !r.content.starts_with("ERROR: ")
                    {
                        out.insert(name.to_owned());
                    }
                }
            }
            ChatRole::System => {}
        }
    }
    out
}

/// Text for the system prompt: one line per non-`always` skill, or empty when none.
pub fn prompt_index(skills: &[SkillMeta]) -> String {
    let mut listed = skills.iter().filter(|s| !s.always).peekable();
    if listed.peek().is_none() {
        return String::new();
    }
    let mut s =
        String::from("## Skills\nCall `load_skill` with a name to get full instructions.\n");
    for skill in listed {
        s.push_str("- ");
        s.push_str(&skill.name);
        s.push_str(": ");
        s.push_str(&skill.description);
        s.push('\n');
    }
    s
}

/// Bodies of `always` skills in name order, blank-line separated (unreadable ones skipped).
pub fn always_bodies(skills: &[SkillMeta]) -> String {
    skills
        .iter()
        .filter(|s| s.always)
        .filter_map(|s| match body(s) {
            Ok(b) => Some(b.body.trim().to_owned()),
            Err(e) => {
                tracing::warn!(path = %s.path, %e, "always skill unreadable, skipped");
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn check_name(name: &str) -> Result<(), String> {
    if tools::git::valid_name(name) {
        Ok(())
    } else {
        Err(format!("invalid name `{name}`: use [a-z0-9][a-z0-9._-]*"))
    }
}

/// Write `<dir>/<name>.md` with front matter. Refuses to shadow a repo skill.
pub fn add(dir: &Path, m: AddSkill) -> Result<SkillMeta, String> {
    check_name(&m.name)?;
    if let Ok(existing) = find(dir, &m.name)
        && let Some(repo) = existing.repo
    {
        return Err(format!(
            "`{}` comes from repo `{repo}`; change it upstream and sync_skills",
            m.name
        ));
    }
    let fm = serde_yaml::to_string(&FrontMatter {
        name: Some(m.name.clone()),
        description: Some(m.description),
        always: m.always,
    })
    .map_err(|e| e.to_string())?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(format!("{}.md", m.name));
    std::fs::write(&path, format!("---\n{fm}---\n\n{}\n", m.body.trim()))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    find(dir, &m.name)
}

/// Delete a local skill: its file, or its `SKILL.md` directory. Refuses repo skills.
pub fn remove(dir: &Path, name: &str) -> Result<(), String> {
    let meta = find(dir, name)?;
    if let Some(repo) = meta.repo {
        return Err(format!(
            "`{name}` comes from repo `{repo}`; use remove_skill_repo"
        ));
    }
    let path = Path::new(&meta.path);
    let result = match path.parent().filter(|p| is_dir_skill(path) && *p != dir) {
        Some(skill_dir) => std::fs::remove_dir_all(skill_dir),
        None => std::fs::remove_file(path),
    };
    result.map_err(|e| format!("{}: {e}", meta.path))
}

/// `git clone --depth 1 <url> <dir>/<name>` (name defaults to the repo
/// basename); returns the skills found in the clone.
pub async fn add_repo(
    dir: &Path,
    url: &str,
    name: Option<String>,
) -> Result<Vec<SkillMeta>, String> {
    let name = name.unwrap_or_else(|| tools::git::repo_name(url));
    check_name(&name)?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    tools::git::clone(url, &dir.join(&name))
        .await
        .map_err(|e| format!("{e:#}"))?;
    Ok(index(dir)
        .into_iter()
        .filter(|s| s.repo.as_deref() == Some(&name))
        .collect())
}

/// `rm -rf <dir>/<name>` when it is a git clone.
pub fn remove_repo(dir: &Path, name: &str) -> Result<(), String> {
    check_name(name)?;
    let path = dir.join(name);
    if !path.join(".git").exists() {
        return Err(format!("no skill repo named {name}"));
    }
    std::fs::remove_dir_all(&path).map_err(|e| format!("{}: {e}", path.display()))
}

fn out(r: Result<String, String>) -> anyhow::Result<ToolOutput> {
    Ok(match r {
        Ok(s) => ToolOutput::ok(s),
        Err(e) => ToolOutput::err(e),
    })
}

/// Arguments for [`LoadSkillTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct LoadSkillArgs {
    /// Skill name from the index.
    pub name: String,
}

/// Returns a skill body to the model.
pub struct LoadSkillTool {
    /// Skills directory.
    pub dir: PathBuf,
}

#[async_trait::async_trait]
impl Tool for LoadSkillTool {
    const NAME: &'static str = LOAD;
    const RISK: Risk = Risk::ReadOnly;
    type Args = LoadSkillArgs;

    fn description(&self) -> String {
        "Load the full instructions for a skill listed in the system prompt.".into()
    }

    async fn call(&self, args: LoadSkillArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        out(load_text(&self.dir, &args.name))
    }
}

/// Arguments for [`AddSkillTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct AddSkillArgs {
    /// `[a-z0-9][a-z0-9._-]*`; an existing local skill of that name is replaced.
    pub name: String,
    /// One line shown in the skills index.
    pub description: String,
    /// Markdown instructions.
    pub body: String,
    /// Inline the body into every system prompt instead of listing it in the index.
    #[serde(default)]
    pub always: bool,
}

/// Writes a local skill file.
pub struct AddSkillTool {
    /// Skills directory.
    pub dir: PathBuf,
}

#[async_trait::async_trait]
impl Tool for AddSkillTool {
    const NAME: &'static str = "add_skill";
    const RISK: Risk = Risk::Mutating;
    type Args = AddSkillArgs;

    fn description(&self) -> String {
        "Save reusable instructions as a skill (markdown with name/description front matter). \
         It is listed in the system prompt from your next model call; `always: true` inlines it instead. \
         Skills that come from a repo cannot be overwritten."
            .into()
    }

    async fn call(&self, a: AddSkillArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let m = AddSkill {
            name: a.name,
            description: a.description,
            body: a.body,
            always: a.always,
        };
        out(add(&self.dir, m)
            .map(|s| format!("saved skill `{}` at {}; available now", s.name, s.path)))
    }
}

/// Arguments for [`RemoveSkillTool`] and [`RemoveSkillRepoTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct NameArgs {
    /// Skill or repo name.
    pub name: String,
}

/// Deletes a local skill.
pub struct RemoveSkillTool {
    /// Skills directory.
    pub dir: PathBuf,
}

#[async_trait::async_trait]
impl Tool for RemoveSkillTool {
    const NAME: &'static str = "remove_skill";
    const RISK: Risk = Risk::Destructive;
    type Args = NameArgs;

    fn description(&self) -> String {
        "Delete a local skill (file or SKILL.md directory). Gone from the system prompt on your next model call. \
         Skills from a repo are removed with remove_skill_repo."
            .into()
    }

    async fn call(&self, a: NameArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        out(remove(&self.dir, &a.name).map(|()| format!("removed skill `{}`", a.name)))
    }
}

/// Arguments for [`AddSkillRepoTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct AddSkillRepoArgs {
    /// Git URL (anything `git clone` accepts).
    pub url: String,
    /// Directory name under skills/; defaults to the repo basename.
    #[serde(default)]
    pub name: Option<String>,
}

/// Clones a git repo of skills.
pub struct AddSkillRepoTool {
    /// Skills directory.
    pub dir: PathBuf,
}

#[async_trait::async_trait]
impl Tool for AddSkillRepoTool {
    const NAME: &'static str = "add_skill_repo";
    const RISK: Risk = Risk::Mutating;
    type Args = AddSkillRepoArgs;

    fn description(&self) -> String {
        "Clone a git repository of skills (SKILL.md directories or loose markdown) into the skills directory. \
         Returns the skills found; they are in the system prompt from your next model call. Update later with sync_skills."
            .into()
    }

    async fn call(&self, a: AddSkillRepoArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        out(add_repo(&self.dir, &a.url, a.name).await.map(|skills| {
            let mut s = format!("cloned; {} skill(s) available now", skills.len());
            for sk in skills {
                s.push_str(&format!("\n- {}: {}", sk.name, sk.description));
            }
            s
        }))
    }
}

/// Deletes a cloned skill repo.
pub struct RemoveSkillRepoTool {
    /// Skills directory.
    pub dir: PathBuf,
}

#[async_trait::async_trait]
impl Tool for RemoveSkillRepoTool {
    const NAME: &'static str = "remove_skill_repo";
    const RISK: Risk = Risk::Destructive;
    type Args = NameArgs;

    fn description(&self) -> String {
        "Delete a cloned skill repository (directory name as shown by its skills' `repo`) and every skill in it."
            .into()
    }

    async fn call(&self, a: NameArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        out(remove_repo(&self.dir, &a.name).map(|()| format!("removed skill repo `{}`", a.name)))
    }
}

/// Arguments for [`SyncSkillsTool`]: none.
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct SyncSkillsArgs {}

/// `git pull` every cloned skill repo.
pub struct SyncSkillsTool {
    /// Skills directory.
    pub dir: PathBuf,
}

#[async_trait::async_trait]
impl Tool for SyncSkillsTool {
    const NAME: &'static str = "sync_skills";
    const RISK: Risk = Risk::Mutating;
    type Args = SyncSkillsArgs;

    fn description(&self) -> String {
        "Pull every cloned skill repository (fast-forward only). Changes apply on your next model call."
            .into()
    }

    async fn call(&self, _: SyncSkillsArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let results = tools::git::sync_all(&self.dir).await;
        if results.is_empty() {
            return Ok(ToolOutput::ok("no skill repos"));
        }
        Ok(ToolOutput::ok(
            results
                .iter()
                .map(|r| format!("{}: {}", r.name, r.status))
                .collect::<Vec<_>>()
                .join("\n"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wth-skills-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn names(skills: &[SkillMeta]) -> Vec<&str> {
        skills.iter().map(|s| s.name.as_str()).collect()
    }

    #[test]
    fn walk_rules_and_bodies() {
        let dir = tmp();
        std::fs::create_dir_all(dir.join("a")).unwrap();
        std::fs::write(
            dir.join("a/one.md"),
            "---\nname: one\ndescription: First skill\n---\n# One\nbody one\n",
        )
        .unwrap();
        std::fs::write(dir.join("two.md"), "\nSecond skill line\nmore\n").unwrap();
        std::fs::write(dir.join("three.txt"), "ignored").unwrap();
        std::fs::write(dir.join("README.md"), "not a skill").unwrap();
        std::fs::write(dir.join("a/License.md"), "not a skill").unwrap();
        std::fs::create_dir_all(dir.join("pack/sub")).unwrap();
        std::fs::write(
            dir.join("pack/SKILL.md"),
            "---\ndescription: Dir skill\n---\npack body\n",
        )
        .unwrap();
        std::fs::write(dir.join("pack/sub/nested.md"), "would be a skill if walked").unwrap();
        std::fs::write(dir.join("pack/helper.py"), "print(1)").unwrap();

        let skills = index(&dir);
        assert_eq!(names(&skills), ["one", "pack", "two"]);
        let idx = prompt_index(&skills);
        assert!(idx.starts_with("## Skills\n"));
        let one = idx.find("- one: First skill\n").expect("one in index");
        let two = idx
            .find("- two: Second skill line\n")
            .expect("two in index");
        assert!(one < two);
        assert!(idx.contains("- pack: Dir skill\n"));

        let one = load(&dir, "one").unwrap();
        assert!(
            !one.body.contains("description:"),
            "front matter leaked: {}",
            one.body
        );
        assert!(one.body.contains("body one"));
        assert!(one.files.is_empty());
        let pack = load(&dir, "pack").unwrap();
        assert_eq!(pack.body.trim(), "pack body");
        assert_eq!(pack.files, ["helper.py", "sub/nested.md"]);
        assert!(load(&dir, "three").is_err());
        assert!(skills.iter().all(|s| s.repo.is_none() && !s.always));
        assert_eq!(prompt_index(&index(&dir.join("missing"))), "");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn injection_round_trips_and_frame_finds_loaded_skills() {
        use genai::chat::{ToolCall, ToolResponse};

        // The wrapper survives a body with its own markup; words after it are the user's.
        let text = inject_text("caveman", "# Caveman\nSpeak <b>short</b>.\n", "lite mode");
        assert_eq!(
            injected(&text),
            Some(("caveman", "# Caveman\nSpeak <b>short</b>.", "lite mode"))
        );
        assert_eq!(
            injected(&inject_text("x", "body", "  ")),
            Some(("x", "body", ""))
        );
        assert_eq!(injected("<skill name=\"x\">unterminated"), None);
        assert_eq!(injected("plain words"), None);

        // A frame: one user injection, one `load_skill` that worked, one that
        // errored, one unrelated tool call.
        let call = |id: &str, name: &str, args: serde_json::Value| ToolCall {
            call_id: id.into(),
            fn_name: name.into(),
            fn_arguments: args,
            thought_signatures: None,
        };
        let messages = vec![
            (1, ChatMessage::user(text)),
            (
                2,
                ChatMessage::from(vec![
                    call("c1", LOAD, serde_json::json!({"name": "ponytail"})),
                    call("c2", LOAD, serde_json::json!({"name": "nonesuch"})),
                    call("c3", "shell", serde_json::json!({"command": "ls"})),
                ]),
            ),
            (
                3,
                ChatMessage::from(vec![
                    ToolResponse::new("c1", "# Ponytail\n…"),
                    ToolResponse::new("c2", "ERROR: unknown skill: nonesuch"),
                    ToolResponse::new("c3", "a b"),
                ]),
            ),
        ];
        let got: Vec<String> = in_frame(&messages).into_iter().collect();
        assert_eq!(got, ["caveman", "ponytail"]);
    }

    #[test]
    fn always_skills_are_inlined_not_indexed() {
        let dir = tmp();
        std::fs::write(
            dir.join("rules.md"),
            "---\nalways: true\n---\nAlways rule.\n",
        )
        .unwrap();
        std::fs::write(dir.join("later.md"), "Listed skill\n").unwrap();
        let skills = index(&dir);
        assert!(skills.iter().find(|s| s.name == "rules").unwrap().always);
        assert_eq!(always_bodies(&skills), "Always rule.");
        let idx = prompt_index(&skills);
        assert!(idx.contains("- later:"));
        assert!(!idx.contains("rules"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn add_remove_round_trip() {
        let dir = tmp();
        let skills_dir = dir.join("skills"); // not yet created: add must create it
        let meta = add(
            &skills_dir,
            AddSkill {
                name: "commit".into(),
                description: "Write a commit: message".into(),
                body: "Body text\n".into(),
                always: false,
            },
        )
        .unwrap();
        assert_eq!(meta.description, "Write a commit: message");
        assert_eq!(
            load(&skills_dir, "commit").unwrap().body.trim(),
            "Body text"
        );
        let text = std::fs::read_to_string(&meta.path).unwrap();
        assert!(!text.contains("always"), "{text}");
        assert!(
            add(
                &skills_dir,
                AddSkill {
                    name: "Bad Name".into(),
                    description: String::new(),
                    body: String::new(),
                    always: false,
                },
            )
            .is_err()
        );
        remove(&skills_dir, "commit").unwrap();
        assert!(find(&skills_dir, "commit").is_err());
        assert!(remove(&skills_dir, "commit").is_err());

        std::fs::create_dir_all(skills_dir.join("pack")).unwrap();
        std::fs::write(skills_dir.join("pack/SKILL.md"), "dir skill\n").unwrap();
        std::fs::write(skills_dir.join("pack/extra.txt"), "x").unwrap();
        remove(&skills_dir, "pack").unwrap();
        assert!(!skills_dir.join("pack").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn repo_skills_from_a_local_bare_repo() {
        let root = tmp();
        let src = root.join("src");
        let bare = root.join("bare.git");
        let dir = root.join("skills");
        std::fs::create_dir_all(src.join("skills/pdf")).unwrap();
        let sh = |args: &[&str], cwd: &Path| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(cwd)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        sh(&["init", "-q", "-b", "main"], &src);
        std::fs::write(src.join("README.md"), "about").unwrap();
        std::fs::write(
            src.join("skills/pdf/SKILL.md"),
            "---\nname: pdf\ndescription: PDF work\n---\nUse pdftotext.\n",
        )
        .unwrap();
        sh(&["add", "."], &src);
        sh(&["commit", "-q", "-m", "one"], &src);
        sh(
            &[
                "clone",
                "-q",
                "--bare",
                src.to_str().unwrap(),
                bare.to_str().unwrap(),
            ],
            &root,
        );

        let found = add_repo(&dir, bare.to_str().unwrap(), None).await.unwrap();
        assert_eq!(names(&found), ["pdf"]);
        assert_eq!(found[0].repo.as_deref(), Some("bare"));
        assert_eq!(names(&index(&dir)), ["pdf"], "README must not be a skill");

        let err = add(
            &dir,
            AddSkill {
                name: "pdf".into(),
                description: String::new(),
                body: String::new(),
                always: false,
            },
        )
        .unwrap_err();
        assert!(err.contains("repo `bare`"), "{err}");
        assert!(
            remove(&dir, "pdf")
                .unwrap_err()
                .contains("remove_skill_repo")
        );

        std::fs::write(
            src.join("skills/pdf/SKILL.md"),
            "---\nname: pdf\ndescription: PDF v2\n---\nx\n",
        )
        .unwrap();
        sh(&["commit", "-q", "-am", "two"], &src);
        sh(&["push", "-q", bare.to_str().unwrap(), "main"], &src);
        let synced = tools::git::sync_all(&dir).await;
        assert_eq!(synced.len(), 1);
        assert_eq!(synced[0].status, "updated");
        assert_eq!(index(&dir)[0].description, "PDF v2");

        assert!(remove_repo(&dir, "nope").is_err());
        remove_repo(&dir, "bare").unwrap();
        assert!(index(&dir).is_empty());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
