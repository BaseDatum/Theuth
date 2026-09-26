//! One agent file: `agents/<name>.md` (67 §6.1). YAML front matter, the body
//! is the charter. Parse, render, atomic write, a single-key rewrite, and
//! the synthesised built-in default agent.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::types::{AgentFile, AgentName, Budget};

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
}

/// Parse one file's text as the agent `name`. `Err` is the user-facing reason
/// (`agent name …`, `front matter: …`, `budget: …`, `tools: …`, `empty charter`,
/// `theuth is reserved`).
pub fn parse(name: &str, text: &str) -> Result<AgentFile, String> {
    let name = AgentName::new(name)?;
    if name.as_str() == AgentName::DEFAULT {
        return Err("theuth is reserved: the default agent has no roster file".into());
    }
    let matter = gray_matter::Matter::<gray_matter::engine::YAML>::new();
    let parsed = matter
        .parse::<FrontMatter>(text)
        .map_err(|e| format!("front matter: {e}"))?;
    let fm = parsed.data.unwrap_or_default();
    // gray_matter returns empty `content` for input no longer than its `---`
    // delimiter; a three-byte charter ("QA.") is still a charter.
    let body = if text.len() <= 3 { text } else { &parsed.content }.trim();
    if body.is_empty() {
        return Err("empty charter".into());
    }
    let tools = trimmed(fm.tools);
    if let Some(t) = &tools
        && !matches!(t.as_str(), "read_only" | "mutating" | "destructive")
    {
        return Err(format!("tools: expected read_only|mutating|destructive, got {t:?}"));
    }
    let budget = fm.budget.as_deref().map_or(Ok(Budget::None), str::parse)?;
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
pub fn default_agent(data_dir: &Path) -> AgentFile {
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
        model: None,
        effort: None,
        tools: Some("destructive".to_owned()),
        mcp: None,
        budget: Budget::None,
        capabilities: None,
        paused: false,
        proposed_by: None,
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
        assert_eq!(f.mcp, Some(vec!["grafana".into(), "argocd_prod".into(), "slack".into()]));
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
        assert_eq!(f.proposed_by.as_deref(), Some("agent:cto"));
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
        assert!(parse("ops", "---\ntitle: Ops\n---\n").is_err(), "empty charter");
    }

    #[test]
    fn mcp_is_three_valued() {
        assert_eq!(parse("ops", "---\nmcp: ~\n---\ncharter").unwrap().mcp, None);
        assert_eq!(parse("ops", "charter").unwrap().mcp, None);
        assert_eq!(parse("ops", "---\nmcp: []\n---\ncharter").unwrap().mcp, Some(vec![]));
        assert_eq!(
            parse("ops", "---\nmcp: [grafana]\n---\ncharter").unwrap().mcp,
            Some(vec!["grafana".to_owned()])
        );
        let mut f = parse("ops", "charter").unwrap();
        assert!(!render(&f).contains("mcp"));
        f.mcp = Some(vec![]);
        assert!(render(&f).contains("mcp: []\n"));
        assert_eq!(parse("ops", &render(&f)).unwrap(), f);
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
            mcp: Some(vec!["grafana".into(), "argocd_prod.list_applications".into()]),
            budget: Budget::Monthly {
                amount: 40.0,
                currency: "USD".into(),
            },
            capabilities: Some("Production health. Not \"code\" review.".into()),
            paused: true,
            proposed_by: Some("agent:cto".into()),
            body: "You are Ops.\n\nYour charter is production.".into(),
        };
        assert_eq!(parse("ops", &render(&f)).unwrap(), f);
        let minimal = parse("qa", "Test things.").unwrap();
        assert_eq!(render(&minimal), "Test things.\n");
        assert_eq!(parse("qa", &render(&minimal)).unwrap(), minimal);
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
        assert_eq!(set_key("charter\n", "paused", Some("true")), "---\npaused: true\n---\ncharter\n");
        assert_eq!(set_key(text, "paused", None), "---\ntitle: Ops\n---\ncharter\n");
        assert!(parse("ops", &set_key(text, "paused", Some("true"))).unwrap().paused);
    }

    #[test]
    fn default_agent_is_theuth() {
        let dir = std::env::temp_dir().join(format!("roster-default-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let t = default_agent(&dir);
        assert!(t.name.is_default());
        assert_eq!(t.reports_to, "user");
        assert_eq!(t.tools.as_deref(), Some("destructive"));
        assert_eq!(t.mcp, None);
        assert!(!t.body.is_empty());
        std::fs::write(dir.join("prompt.md"), "Standing instructions.\n").unwrap();
        assert_eq!(default_agent(&dir).body, "Standing instructions.");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
