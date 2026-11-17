//! One agent file: `agents/<name>.md` (67 §6.1). YAML front matter, the body
//! is the charter. Parse, render, atomic write, a single-key rewrite, and
//! the synthesised built-in default agent.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::types::{AgentFile, AgentName, Budget, RunsIn};

/// Directory under `data_dir` holding the agent files.
pub const DIR: &str = "agents";

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct FrontMatter {
    title: Option<String>,
    reports_to: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    tools: Option<String>,
    mcp: Option<Vec<String>>,
    budget: Option<String>,
    capabilities: Option<String>,
    paused: Option<bool>,
    proposed_by: Option<String>,
    runs_in: Option<String>,
    memory: Option<String>,
    isolated_memory: Option<bool>,
    view_bytes: Option<u32>,
    memory_model: Option<String>,
    memory_effort: Option<String>,
}

/// Parse one file's text as the agent `name`. `Err` is the user-facing reason
/// (`agent name …`, `front matter: …`, `budget: …`, `tools: …`, `runs_in: …`,
/// `memory: …`, `view_bytes: …`, `empty charter`, `theuth is reserved`).
pub fn parse(name: &str, text: &str) -> Result<AgentFile, String> {
    let name = AgentName::new(name)?;
    if name.as_str() == AgentName::DEFAULT {
        return Err("theuth is reserved: the default agent has no roster file".into());
    }
    // 179: the agent crate reads `paused` at boot with this same parse.
    let (data, content) = agent::supervisor::pause::front_matter::<FrontMatter>(text)?;
    let fm = data.unwrap_or_default();
    // gray_matter returns empty `content` for input no longer than its `---`
    // delimiter; a three-byte charter ("QA.") is still a charter.
    let body = if text.len() <= 3 { text } else { &content }.trim();
    if body.is_empty() {
        return Err("empty charter".into());
    }
    let tools = trimmed(fm.tools);
    if let Some(t) = &tools
        && !matches!(t.as_str(), "read_only" | "mutating" | "destructive")
    {
        return Err(format!(
            "tools: expected read_only|mutating|destructive, got {t:?}"
        ));
    }
    let budget = fm.budget.as_deref().map_or(Ok(Budget::None), str::parse)?;
    // 137 §10: `memory: epochs | endless`, `fractal` (the product name) read as
    // `endless` (`render` keeps writing `endless` for older engines); `view_bytes`
    // within the fold's range.
    let memory = trimmed(fm.memory)
        .map(|m| {
            let wire = if m == "fractal" {
                "endless"
            } else {
                m.as_str()
            };
            wire.parse::<agent::MemoryMode>()
                .map_err(|_| format!("memory: expected epochs|fractal|endless, got {m:?}"))
        })
        .transpose()?;
    if let Some(b) = fm.view_bytes
        && !(16_000..=256_000).contains(&b)
    {
        return Err(format!("view_bytes: expected 16000..=256000, got {b}"));
    }
    Ok(AgentFile {
        title: trimmed(fm.title).unwrap_or_else(|| name.as_str().to_owned()),
        name,
        reports_to: trimmed(fm.reports_to).unwrap_or_else(|| AgentName::DEFAULT.to_owned()),
        model: trimmed(fm.model),
        effort: trimmed(fm.effort),
        tools,
        mcp: fm.mcp.map(|l| {
            l.into_iter()
                .map(|m| m.trim().to_owned())
                .filter(|m| !m.is_empty())
                .collect()
        }),
        budget,
        capabilities: trimmed(fm.capabilities),
        paused: fm.paused.unwrap_or(false),
        proposed_by: trimmed(fm.proposed_by),
        runs_in: fm.runs_in.as_deref().map_or(Ok(RunsIn::Any), str::parse)?,
        memory,
        isolated_memory: fm.isolated_memory.unwrap_or(false),
        view_bytes: fm.view_bytes,
        memory_model: trimmed(fm.memory_model),
        memory_effort: trimmed(fm.memory_effort),
        body: body.to_owned(),
    })
}

/// Trim; empty → `None`.
fn trimmed(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

/// Render the file text: front matter with only the keys that differ from
/// their defaults, a blank line, the charter, trailing newline. `parse` of
/// the result is the input (`render_parse_round_trip`).
pub fn render(f: &AgentFile) -> String {
    let mut s = String::new();
    if f.title != f.name.as_str() {
        s.push_str(&format!("title: {}\n", yaml_str(&f.title)));
    }
    if f.reports_to != AgentName::DEFAULT {
        s.push_str(&format!("reports_to: {}\n", f.reports_to.trim()));
    }
    if let Some(m) = &f.model {
        s.push_str(&format!("model: {}\n", m.trim()));
    }
    if let Some(e) = &f.effort {
        s.push_str(&format!("effort: {}\n", e.trim()));
    }
    if let Some(t) = &f.tools {
        s.push_str(&format!("tools: {}\n", t.trim()));
    }
    if let Some(m) = &f.mcp {
        s.push_str(&format!("mcp: [{}]\n", m.join(", ")));
    }
    if f.budget != Budget::None {
        s.push_str(&format!("budget: {}\n", f.budget));
    }
    if let Some(c) = &f.capabilities {
        s.push_str(&format!("capabilities: {}\n", yaml_str(c.trim())));
    }
    if f.paused {
        s.push_str("paused: true\n");
    }
    if let Some(p) = &f.proposed_by {
        s.push_str(&format!("proposed_by: {}\n", p.trim()));
    }
    if !f.runs_in.is_any() {
        s.push_str(&format!("runs_in: {}\n", f.runs_in));
    }
    if f.isolated_memory {
        s.push_str("isolated_memory: true\n");
    }
    if let Some(m) = f.memory {
        s.push_str(&format!("memory: {m}\n"));
    }
    if let Some(b) = f.view_bytes {
        s.push_str(&format!("view_bytes: {b}\n"));
    }
    if let Some(m) = &f.memory_model {
        s.push_str(&format!("memory_model: {}\n", m.trim()));
    }
    if let Some(e) = &f.memory_effort {
        s.push_str(&format!("memory_effort: {}\n", yaml_str(e.trim())));
    }
    if !s.is_empty() {
        s.insert_str(0, "---\n");
        s.push_str("---\n\n");
    }
    s.push_str(f.body.trim());
    s.push('\n');
    s
}

/// Double-quoted YAML scalar: backslashes and quotes escaped, so a `:`, `#` or
/// leading `~` survive.
fn yaml_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Rewrite one `key:` line inside the front matter, byte-for-byte elsewhere
/// (67 §6.1 hot-reload; the schedule crate's `set_enabled` shape): replace
/// it when present, insert `key: value` after the opening `---` when absent,
/// remove it when `value` is `None`. Text without front matter gets one.
pub fn set_key(text: &str, key: &str, value: Option<&str>) -> String {
    let line = value.map(|v| format!("{key}: {v}\n"));
    if !text.starts_with("---\n") {
        return match line {
            Some(l) => format!("---\n{l}---\n{text}"),
            None => text.to_owned(),
        };
    }
    let prefix = format!("{key}:");
    let mut lines = text.split_inclusive('\n');
    let open = lines.next().unwrap_or_default();
    let mut out = String::with_capacity(text.len() + 32);
    out.push_str(open);
    let mut replaced = false;
    for l in &mut lines {
        if l.trim_end() == "---" {
            out.push_str(l);
            break;
        }
        if l.starts_with(&prefix) {
            if let Some(l) = &line {
                out.push_str(l);
            }
            replaced = true;
        } else {
            out.push_str(l);
        }
    }
    if !replaced && let Some(l) = &line {
        out.insert_str(open.len(), l);
    }
    out.extend(lines);
    out
}

/// Write `<dir>/<name>.md` atomically (dotfile temp in `dir`, then rename) so
/// the watcher never reads half of it. Creates `dir`. Returns the final path.
pub fn write_atomic(dir: &Path, name: &str, text: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{name}.md.tmp"));
    let path = dir.join(format!("{name}.md"));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// The built-in default agent (67 §0 decision 1, §6.1): never a file. Charter
/// = `<data_dir>/prompt.md` (a one-line stand-in when it is empty or absent),
/// `reports_to: user`, `tools: destructive`, no `mcp` limit, no budget.
/// `model` is the live `roster.theuth_model` (148 E3; `None` = the default
/// for new sessions).
pub fn default_agent(data_dir: &Path, model: Option<String>) -> AgentFile {
    let prompt = std::fs::read_to_string(data_dir.join("prompt.md")).unwrap_or_default();
    let body = prompt.trim();
    let body = if body.is_empty() {
        "You are theuth, the default agent: you answer anything unaddressed and own delegation."
    } else {
        body
    };
    AgentFile {
        name: AgentName::theuth(),
        title: AgentName::DEFAULT.to_owned(),
        reports_to: "user".to_owned(),
        model,
        effort: None,
        tools: Some("destructive".to_owned()),
        mcp: None,
        budget: Budget::None,
        capabilities: None,
        paused: false,
        proposed_by: None,
        runs_in: RunsIn::Any,
        memory: None,
        isolated_memory: false,
        view_bytes: None,
        memory_model: None,
        memory_effort: None,
        body: body.to_owned(),
    }
}

/// Every `*.md` in `dir` (non-recursive; names starting with `.` ignored),
/// sorted by name: `(stem, parse result)`. Missing dir → empty; an unreadable
/// file → `Err` with the io error.
pub fn load_dir(dir: &Path) -> Vec<(String, Result<AgentFile, String>)> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, Result<AgentFile, String>)> = rd
        .filter_map(Result::ok)
        .filter_map(|e| {
            let path = e.path();
            let stem = path.file_stem()?.to_str()?.to_owned();
            if path.extension()? != "md" || stem.starts_with('.') {
                return None;
            }
            let r = std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|text| parse(&stem, &text));
            Some((stem, r))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_file_is_body_plus_defaults() {
        let f = parse("ops", "You are Ops.\n").unwrap();
        assert_eq!(f.name.as_str(), "ops");
        assert_eq!(f.title, "ops");
        assert_eq!(f.reports_to, "theuth");
        assert_eq!(f.model, None);
        assert_eq!(f.tools, None);
        assert_eq!(f.mcp, None);
        assert_eq!(f.budget, Budget::None);
        assert!(!f.paused);
        assert!(!f.isolated_memory);
        assert_eq!(f.proposed_by, None);
        assert_eq!(f.body, "You are Ops.");
    }

    #[test]
    fn full_file() {
        let text = concat!(
            "---\n",
            "title: Ops\n",
            "reports_to: theuth\n",
            "model: anthropic/claude-opus-5-5\n",
            "effort: high\n",
            "tools: mutating\n",
            "mcp: [grafana, argocd_prod, slack]\n",
            "budget: 40 USD/month\n",
            "capabilities: >\n",
            "  Production health, incidents, Grafana and ArgoCD. Not code review.\n",
            "paused: false\n",
            "proposed_by: agent:cto\n",
            "---\n",
            "You are Ops. Your charter is production.\n",
        );
        let f = parse("ops", text).unwrap();
        assert_eq!(f.title, "Ops");
        assert_eq!(f.reports_to, "theuth");
        assert_eq!(f.model.as_deref(), Some("anthropic/claude-opus-5-5"));
        assert_eq!(f.effort.as_deref(), Some("high"));
        assert_eq!(f.tools.as_deref(), Some("mutating"));
        assert_eq!(
            f.mcp,
            Some(vec!["grafana".into(), "argocd_prod".into(), "slack".into()])
        );
        assert_eq!(
            f.budget,
            Budget::Monthly {
                amount: 40.0,
                currency: "USD".into()
            }
        );
        assert_eq!(
            f.capabilities.as_deref(),
            Some("Production health, incidents, Grafana and ArgoCD. Not code review.")
        );
        assert!(!f.paused);
        assert!(!f.isolated_memory);
        assert_eq!(f.proposed_by.as_deref(), Some("agent:cto"));
        assert_eq!(f.runs_in, RunsIn::Any);
        assert_eq!(f.body, "You are Ops. Your charter is production.");
    }

    #[test]
    fn invalid_name_is_refused() {
        assert!(parse("Ops", "charter").is_err());
        assert!(parse("", "charter").is_err());
        assert!(parse("a_b", "charter").is_err());
        assert!(parse(&"a".repeat(33), "charter").is_err());
        assert!(parse("theuth", "charter").is_err());
        assert!(parse("ops", "---\ncomputer: build-A\n---\ncharter").is_err());
        assert!(
            parse("ops", "---\ntitle: Ops\n---\n").is_err(),
            "empty charter"
        );
    }

    #[test]
    fn mcp_is_three_valued() {
        assert_eq!(parse("ops", "---\nmcp: ~\n---\ncharter").unwrap().mcp, None);
        assert_eq!(parse("ops", "charter").unwrap().mcp, None);
        assert_eq!(
            parse("ops", "---\nmcp: []\n---\ncharter").unwrap().mcp,
            Some(vec![])
        );
        assert_eq!(
            parse("ops", "---\nmcp: [grafana]\n---\ncharter")
                .unwrap()
                .mcp,
            Some(vec!["grafana".to_owned()])
        );
        let mut f = parse("ops", "charter").unwrap();
        assert!(!render(&f).contains("mcp"));
        f.mcp = Some(vec![]);
        assert!(render(&f).contains("mcp: []\n"));
        assert_eq!(parse("ops", &render(&f)).unwrap(), f);
    }

    #[test]
    fn runs_in_forms() {
        assert_eq!(parse("ops", "Ops.").unwrap().runs_in, RunsIn::Any);
        assert_eq!(
            parse("ops", "---\nruns_in: any\n---\nOps.")
                .unwrap()
                .runs_in,
            RunsIn::Any
        );
        let own = parse("ops", "---\nruns_in: vm\n---\nOps.").unwrap();
        assert_eq!(own.runs_in, RunsIn::OwnVm);
        assert_eq!(render(&own), "---\nruns_in: vm\n---\n\nOps.\n");
        let named = parse("ops", "---\nruns_in: vm:lab\n---\nOps.").unwrap();
        assert_eq!(named.runs_in, RunsIn::Vm("lab".into()));
        assert_eq!(parse("ops", &render(&named)).unwrap(), named);
        let e = parse("ops", "---\nruns_in: docker\n---\nOps.").unwrap_err();
        assert!(e.starts_with("runs_in: "), "{e}");
        assert!(parse("ops", "---\nruns_in: 'vm:'\n---\nOps.").is_err());
    }

    #[test]
    fn memory_forms() {
        // 137 §10: absent → default (the config decides), both modes, bad value → Err.
        assert_eq!(parse("ops", "Ops.").unwrap().memory, None);
        let on = parse("ops", "---\nmemory: endless\n---\nOps.").unwrap();
        assert_eq!(on.memory, Some(agent::MemoryMode::Endless));
        assert_eq!(render(&on), "---\nmemory: endless\n---\n\nOps.\n");
        // The product name is an alias; the file is still written as `endless`.
        let alias = parse("ops", "---\nmemory: fractal\n---\nOps.").unwrap();
        assert_eq!(alias.memory, Some(agent::MemoryMode::Endless));
        assert_eq!(render(&alias), "---\nmemory: endless\n---\n\nOps.\n");
        let off = parse("ops", "---\nmemory: epochs\nview_bytes: 32000\n---\nOps.").unwrap();
        assert_eq!(
            (off.memory, off.view_bytes),
            (Some(agent::MemoryMode::Epochs), Some(32_000))
        );
        assert_eq!(parse("ops", &render(&off)).unwrap(), off);
        let e = parse("ops", "---\nmemory: forever\n---\nOps.").unwrap_err();
        assert!(e.starts_with("memory: "), "{e}");
        let e = parse("ops", "---\nview_bytes: 1000\n---\nOps.").unwrap_err();
        assert!(e.starts_with("view_bytes: "), "{e}");
    }

    #[test]
    fn render_parse_round_trip() {
        let f = AgentFile {
            name: AgentName::new("ops").unwrap(),
            title: "Ops: prod".into(),
            reports_to: "cto".into(),
            model: Some("anthropic/claude-opus-5-5".into()),
            effort: Some("high".into()),
            tools: Some("mutating".into()),
            mcp: Some(vec![
                "grafana".into(),
                "argocd_prod.list_applications".into(),
            ]),
            budget: Budget::Monthly {
                amount: 40.0,
                currency: "USD".into(),
            },
            capabilities: Some("Production health. Not \"code\" review.".into()),
            paused: true,
            proposed_by: Some("agent:cto".into()),
            runs_in: RunsIn::Vm("lab".into()),
            memory: Some(agent::MemoryMode::Endless),
            isolated_memory: true,
            view_bytes: Some(96_000),
            memory_model: Some("anthropic/claude-haiku-5-5".into()),
            memory_effort: Some("8000".into()),
            body: "You are Ops.\n\nYour charter is production.".into(),
        };
        assert_eq!(parse("ops", &render(&f)).unwrap(), f);
        let minimal = parse("qa", "Test things.").unwrap();
        assert_eq!(render(&minimal), "Test things.\n");
        assert_eq!(parse("qa", &render(&minimal)).unwrap(), minimal);
        // 194: a file older than `memory_effort:` loads with none; a written
        // budget stays a string.
        let old = parse("qa", "---\nmemory_model: p/m\n---\nTest things.").unwrap();
        assert_eq!(
            (old.memory_model.as_deref(), old.memory_effort),
            (Some("p/m"), None)
        );
        assert!(render(&f).contains("memory_effort: \"8000\"\n"));
    }

    #[test]
    fn set_key_rewrites_one_line() {
        let text = "---\ntitle: Ops\npaused: false\n---\ncharter\n";
        assert_eq!(
            set_key(text, "paused", Some("true")),
            "---\ntitle: Ops\npaused: true\n---\ncharter\n"
        );
        assert_eq!(
            set_key("---\ntitle: Ops\n---\ncharter\n", "paused", Some("true")),
            "---\npaused: true\ntitle: Ops\n---\ncharter\n"
        );
        assert_eq!(
            set_key("charter\n", "paused", Some("true")),
            "---\npaused: true\n---\ncharter\n"
        );
        assert_eq!(
            set_key(text, "paused", None),
            "---\ntitle: Ops\n---\ncharter\n"
        );
        assert!(
            parse("ops", &set_key(text, "paused", Some("true")))
                .unwrap()
                .paused
        );
    }

    #[test]
    fn default_agent_is_theuth() {
        let dir = std::env::temp_dir().join(format!("roster-default-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let t = default_agent(&dir, None);
        assert!(t.name.is_default());
        assert_eq!(t.reports_to, "user");
        assert_eq!(t.tools.as_deref(), Some("destructive"));
        assert_eq!(t.mcp, None);
        assert_eq!(t.model, None, "unset = the default for new sessions");
        assert!(!t.body.is_empty());
        std::fs::write(dir.join("prompt.md"), "Standing instructions.\n").unwrap();
        let t = default_agent(&dir, Some("testvis/eyes".to_owned()));
        assert_eq!(t.body, "Standing instructions.");
        assert_eq!(
            t.model.as_deref(),
            Some("testvis/eyes"),
            "148 E3: the live roster.theuth_model"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
