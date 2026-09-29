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
    let rest = crate::clock::strip(text)
        .trim_start()
        .strip_prefix("<skill name=\"")?;
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

/// Skills written into an ACP child's cwd (design doc 75 §4): what
/// [`materialise`] created, undone by [`Materialised::remove`] when the
/// child stops. Directories that already existed and exclude lines already
/// present are not recorded, so a second materialise on the same cwd (or a
/// crash's leftovers) is never removed by the wrong owner.
#[derive(Debug, Default)]
pub struct Materialised {
    /// The child's cwd: the parent walk of [`Self::remove`] stops here.
    cwd: PathBuf,
    /// `<cwd>/<skills_dir>/<name>` directories created here.
    dirs: Vec<PathBuf>,
    /// The git exclude file and the lines appended to it.
    exclude: Option<(PathBuf, Vec<String>)>,
}

impl Materialised {
    /// Delete the written skill directories (and the now-empty `skills_dir`
    /// chain below cwd) and the exclude lines. Best effort, logged.
    pub fn remove(self) {
        for dir in &self.dirs {
            if let Err(e) = std::fs::remove_dir_all(dir) {
                tracing::warn!(path = %dir.display(), %e, "materialised skill not removed");
            }
        }
        // `<cwd>/.agents/skills`, then `<cwd>/.agents`: only while empty.
        let mut parent = self.dirs.first().and_then(|d| d.parent());
        while let Some(p) = parent
            && p != self.cwd
            && std::fs::remove_dir(p).is_ok()
        {
            parent = p.parent();
        }
        if let Some((path, lines)) = self.exclude
            && let Ok(text) = std::fs::read_to_string(&path)
        {
            let mut lines = lines;
            let kept: Vec<&str> = text
                .lines()
                .filter(|l| match lines.iter().position(|x| x == l) {
                    Some(i) => {
                        lines.swap_remove(i);
                        false
                    }
                    None => true,
                })
                .collect();
            let mut out = kept.join("\n");
            if !out.is_empty() {
                out.push('\n');
            }
            if let Err(e) = std::fs::write(&path, out) {
                tracing::warn!(path = %path.display(), %e, "exclude lines not removed");
            }
        }
    }
}

/// `git rev-parse --git-path info/exclude --show-prefix` in `cwd`: the
/// exclude file (worktrees included — the `.git` file is git's to follow)
/// and cwd's path under the repo root (`""` at the root, `sub/` below).
/// `None` outside a repo or without git.
fn git_exclude(cwd: &Path) -> Option<(PathBuf, String)> {
    let mut cmd = std::process::Command::new("git");
    cmd.args(["rev-parse", "--git-path", "info/exclude", "--show-prefix"])
        .current_dir(cwd);
    platform::process::hide_console(&mut cmd);
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut lines = text.lines();
    let path = Path::new(lines.next()?.trim());
    let prefix = lines.next().unwrap_or_default().trim().to_owned();
    Some((cwd.join(path), prefix))
}

/// Write the `always` skills of `dir` plus those in `names` as
/// `<cwd>/<skills_dir>/<name>/SKILL.md` (front matter `name`/`description`,
/// the body, a `SKILL.md` skill's sibling files copied beside it) for an
/// ACP child to load (design doc 75 §4). An existing `<name>/` is the
/// user's and skipped. Inside a git repo each new directory gets a line in
/// `info/exclude` under a `# theuth <session>` comment; lines already there
/// are not repeated. Errors: an unknown name (lists the index), a
/// `skills_dir` that is absolute or climbs out, a cwd that is not a
/// writable directory. Nothing selected → nothing touched.
pub fn materialise(
    dir: &Path,
    cwd: &Path,
    skills_dir: &Path,
    names: &[String],
    session: &str,
) -> Result<Materialised, String> {
    let index = index(dir);
    let unknown: Vec<&str> = names
        .iter()
        .filter(|n| !index.iter().any(|s| &s.name == *n))
        .map(String::as_str)
        .collect();
    if !unknown.is_empty() {
        let have: Vec<&str> = index.iter().map(|s| s.name.as_str()).collect();
        return Err(format!(
            "unknown skill(s) {}; available: {}",
            unknown.join(", "),
            if have.is_empty() {
                "none".to_owned()
            } else {
                have.join(", ")
            }
        ));
    }
    let selected: Vec<&SkillMeta> = index
        .iter()
        .filter(|s| s.always || names.contains(&s.name))
        .collect();
    if selected.is_empty() {
        return Ok(Materialised::default());
    }
    if skills_dir.is_absolute()
        || !skills_dir
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
    {
        return Err(format!(
            "skills_dir `{}` must be a relative path inside the cwd",
            skills_dir.display()
        ));
    }
    if !cwd.is_dir() {
        return Err(format!("cwd {} is not a directory", cwd.display()));
    }
    let root = cwd.join(skills_dir);
    let mut m = Materialised {
        cwd: cwd.to_path_buf(),
        ..Default::default()
    };
    for meta in selected {
        let dest = root.join(&meta.name);
        if dest.exists() {
            tracing::info!(path = %dest.display(), "skill directory exists, left as is");
            continue;
        }
        let body = body(meta).map_err(|e| format!("{}: {e}", meta.name))?;
        let fm = serde_yaml::to_string(&FrontMatter {
            name: Some(meta.name.clone()),
            description: Some(meta.description.clone()),
            always: false,
        })
        .map_err(|e| e.to_string())?;
        let write = |m: &mut Materialised| -> std::io::Result<()> {
            std::fs::create_dir_all(&dest)?;
            m.dirs.push(dest.clone());
            std::fs::write(
                dest.join("SKILL.md"),
                format!("---\n{fm}---\n\n{}\n", body.body.trim()),
            )?;
            let src = Path::new(&meta.path).parent().unwrap_or(Path::new(""));
            for rel in &body.files {
                let to = dest.join(rel);
                if let Some(p) = to.parent() {
                    std::fs::create_dir_all(p)?;
                }
                std::fs::copy(src.join(rel), to)?;
            }
            Ok(())
        };
        if let Err(e) = write(&mut m) {
            m.remove();
            return Err(format!("cannot write skills into {}: {e}", cwd.display()));
        }
    }
    if m.dirs.is_empty() {
        return Ok(m);
    }
    if let Some((path, prefix)) = git_exclude(cwd) {
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        let have: BTreeSet<&str> = existing.lines().collect();
        let mut lines: Vec<String> = m
            .dirs
            .iter()
            .filter_map(|d| d.strip_prefix(cwd).ok())
            .map(|rel| format!("/{prefix}{}/", rel.to_string_lossy().replace('\\', "/")))
            .filter(|l| !have.contains(l.as_str()))
            .collect();
        if !lines.is_empty() {
            lines.insert(0, format!("# theuth {session}"));
            let mut text = existing;
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            for l in &lines {
                text.push_str(l);
                text.push('\n');
            }
            let written = path
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::write(&path, text));
            match written {
                Ok(()) => m.exclude = Some((path, lines)),
                Err(e) => tracing::warn!(path = %path.display(), %e, "info/exclude not written"),
            }
        }
    }
    Ok(m)
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
        let dir = std::env::temp_dir().join(format!("theuth-skills-{}", uuid::Uuid::new_v4()));
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

    /// 75 §4: written into a repo subdir, a worktree (`.git` is a file) and
    /// a plain dir; excluded from git exactly once; removed without a trace.
    #[test]
    fn materialise_writes_excludes_and_removes() {
        let root = tmp();
        let dir = root.join("skills");
        std::fs::create_dir_all(dir.join("pack")).unwrap();
        std::fs::write(
            dir.join("brief.md"),
            "---\nname: brief\ndescription: Be brief\nalways: true\n---\nShort.\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("pack/SKILL.md"),
            "---\ndescription: Pack\n---\npack body\n",
        )
        .unwrap();
        std::fs::write(dir.join("pack/helper.py"), "print(1)").unwrap();
        std::fs::write(dir.join("two.md"), "Two line\nmore\n").unwrap();
        let git = |args: &[&str], cwd: &Path| -> String {
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
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        let repo = root.join("repo");
        let cwd = repo.join("sub");
        std::fs::create_dir_all(&cwd).unwrap();
        git(&["init", "-q", "-b", "main"], &repo);
        std::fs::write(repo.join("README.md"), "about").unwrap();
        git(&["add", "."], &repo);
        git(&["commit", "-q", "-m", "one"], &repo);
        let exclude = repo.join(".git/info/exclude");
        std::fs::write(&exclude, "*.log").unwrap(); // no trailing newline
        let skills_dir = Path::new(acp::DEFAULT_SKILLS_DIR);

        // Unknown name: the index is listed; nothing written.
        let e = materialise(&dir, &cwd, skills_dir, &["nope".into()], "s1").unwrap_err();
        assert!(e.contains("nope") && e.contains("brief, pack, two"), "{e}");
        assert!(!cwd.join(".agents").exists());
        // Bad skills_dir and bad cwd refuse before writing.
        assert!(materialise(&dir, &cwd, Path::new("../x"), &[], "s1").is_err());
        assert!(materialise(&dir, &cwd, Path::new("/tmp/x"), &[], "s1").is_err());
        assert!(
            materialise(&dir, &repo.join("README.md"), skills_dir, &[], "s1")
                .unwrap_err()
                .contains("not a directory")
        );

        // `always` + named, from a repo subdir. The user's own `two/` wins.
        std::fs::create_dir_all(cwd.join(".agents/skills/two")).unwrap();
        std::fs::write(cwd.join(".agents/skills/two/SKILL.md"), "mine").unwrap();
        let m = materialise(&dir, &cwd, skills_dir, &["pack".into(), "two".into()], "s1").unwrap();
        let brief = std::fs::read_to_string(cwd.join(".agents/skills/brief/SKILL.md")).unwrap();
        assert_eq!(
            brief,
            "---\nname: brief\ndescription: Be brief\n---\n\nShort.\n"
        );
        let pack = std::fs::read_to_string(cwd.join(".agents/skills/pack/SKILL.md")).unwrap();
        assert_eq!(
            pack,
            "---\nname: pack\ndescription: Pack\n---\n\npack body\n"
        );
        assert_eq!(
            std::fs::read_to_string(cwd.join(".agents/skills/pack/helper.py")).unwrap(),
            "print(1)"
        );
        assert_eq!(
            std::fs::read_to_string(cwd.join(".agents/skills/two/SKILL.md")).unwrap(),
            "mine"
        );
        let text = std::fs::read_to_string(&exclude).unwrap();
        assert_eq!(
            text,
            "*.log\n# theuth s1\n/sub/.agents/skills/brief/\n/sub/.agents/skills/pack/\n"
        );
        // Not `two/`: we did not create it, so we do not hide or remove it
        // (`-uall`: the exact file, not a collapsed `sub/`).
        assert_eq!(
            git(&["status", "--porcelain", "-uall"], &repo).trim(),
            "?? sub/.agents/skills/two/SKILL.md"
        );
        std::fs::remove_dir_all(cwd.join(".agents/skills/two")).unwrap();
        assert_eq!(git(&["status", "--porcelain"], &repo), "");

        // A second materialise on the same cwd adds nothing and removes nothing.
        let again = materialise(&dir, &cwd, skills_dir, &["pack".into()], "s2").unwrap();
        assert_eq!(std::fs::read_to_string(&exclude).unwrap(), text);
        again.remove();
        assert!(cwd.join(".agents/skills/pack/SKILL.md").exists());
        assert_eq!(std::fs::read_to_string(&exclude).unwrap(), text);

        m.remove();
        assert!(!cwd.join(".agents").exists());
        assert!(cwd.is_dir());
        assert_eq!(std::fs::read_to_string(&exclude).unwrap(), "*.log\n");
        assert_eq!(git(&["status", "--porcelain"], &repo), "");

        // A worktree: `.git` is a file; the exclude is the main repo's.
        let wt = root.join("wt");
        git(
            &["worktree", "add", "-q", "-b", "wt", wt.to_str().unwrap()],
            &repo,
        );
        assert!(wt.join(".git").is_file());
        let m = materialise(&dir, &wt, Path::new(".claude/skills"), &[], "s3").unwrap();
        assert!(wt.join(".claude/skills/brief/SKILL.md").is_file());
        assert!(!wt.join(".agents").exists());
        assert_eq!(
            std::fs::read_to_string(&exclude).unwrap(),
            "*.log\n# theuth s3\n/.claude/skills/brief/\n"
        );
        assert_eq!(git(&["status", "--porcelain"], &wt), "");
        m.remove();
        assert!(!wt.join(".claude").exists());
        assert_eq!(std::fs::read_to_string(&exclude).unwrap(), "*.log\n");
        assert_eq!(git(&["status", "--porcelain"], &wt), "");

        // Outside git: files only.
        let plain = root.join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        let m = materialise(&dir, &plain, skills_dir, &[], "s4").unwrap();
        assert!(plain.join(".agents/skills/brief/SKILL.md").is_file());
        m.remove();
        assert_eq!(std::fs::read_dir(&plain).unwrap().count(), 0);

        // Nothing selected: nothing touched, even a missing cwd.
        std::fs::remove_file(dir.join("brief.md")).unwrap();
        materialise(&dir, &root.join("missing"), skills_dir, &[], "s5")
            .unwrap()
            .remove();
        assert!(!root.join("missing").exists());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
