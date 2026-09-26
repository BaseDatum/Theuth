//! The `agent_draft` tool (70 AGENTS-CHAT, Sky's ruling: theuth proposes hires
//! in chat; the board observes). Same pattern as [`crate::draft`]: each proposed
//! agent is an OpenUI card with its fields, a charter preview and three
//! `@ToAssistant` buttons — *Hire* (or *Replace* when the name exists), *Hire
//! paused*, *Discard*. The press arrives as the user's message naming the draft
//! id and the choice; the assistant then calls `agent_put` (the press is the
//! approval) — or, when the drafting session is a desk, `agent_propose`, which
//! lands the file paused and files the `hire_agent` approval the UI inbox shows.
//! Drafts are never written: a draft id lives in the card only.

use std::collections::BTreeSet;
use std::sync::Arc;

use roster::{AgentName, AgentOf, ListAgents};
use serde::{Deserialize, Serialize};
use tools::{Risk, Tool, ToolCx, ToolOutput};

use crate::actor::Core;
use crate::draft::lit;
use crate::tool::{AppArgs, Origin, commit_source};

/// App name every draft set is published under (a new version per call).
const APP_NAME: &str = "agent-drafts";

/// Chars of the charter shown on the card.
const PREVIEW: usize = 600;

/// One proposed agent: the fields of `agents/<name>.md`.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentDraft {
    /// `[a-z0-9-]{1,32}`; `theuth` is reserved. Becomes `agents/<name>.md`.
    pub name: AgentName,
    /// Display title (the role).
    pub title: String,
    /// Manager: `theuth` or an agent on the roster.
    pub reports_to: String,
    /// Model for every session it opens; default: the harness default.
    #[serde(default)]
    pub model: Option<String>,
    /// Effort for every session it opens.
    #[serde(default)]
    pub effort: Option<String>,
    /// Risk ceiling: `read_only` | `mutating` | `destructive` (default).
    #[serde(default)]
    pub tools: Option<String>,
    /// MCP servers (or `server.tool`) it may see; omit = every server, `[]` = none.
    #[serde(default)]
    pub mcp: Option<Vec<String>>,
    /// `none` (default) or `<amount> <CUR>/month`.
    #[serde(default)]
    pub budget: Option<String>,
    /// One paragraph on when this agent is the right one to hand off to.
    #[serde(default)]
    pub capabilities: Option<String>,
    /// The charter (markdown): purpose, what it owns, what it never does, how it reports.
    pub charter: String,
}

/// Arguments of `agent_draft`.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentDraftArgs {
    /// One or more proposed agents.
    pub agents: Vec<AgentDraft>,
}

/// `agent_draft`: proposed agents as approval cards in chat.
pub struct AgentDraftTool {
    /// Shared with the actor.
    pub core: Arc<Core>,
}

/// What a button press becomes: the tool to call and the `paused` it lands with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Approval {
    /// `agent_put` (the press is the approval) or `agent_propose` (a desk drafted: paused + `hire_agent` approval).
    pub tool: &'static str,
    /// `paused` on the written file.
    pub paused: bool,
}

/// The approval for `choice` (`hire` | `replace` | `hire paused`; anything else,
/// incl. `discard`, is `None`) when the drafting session is `desk`'s (`None`:
/// the user's own chat or theuth's desk).
pub fn approval(choice: &str, desk: Option<&str>) -> Option<Approval> {
    let paused = match choice.trim() {
        "hire" | "replace" => false,
        "hire paused" => true,
        _ => return None,
    };
    Some(match desk {
        Some(_) => Approval { tool: "agent_propose", paused: true },
        None => Approval { tool: "agent_put", paused },
    })
}

/// The receipt's "then call …" sentence for this drafting session.
fn then(desk: Option<&str>) -> String {
    let hire = approval("hire", desk).expect("hire is an approval");
    let paused = approval("hire paused", desk).expect("hire paused is an approval");
    match desk {
        Some(d) => format!(
            "`hire` or `hire paused` → `{}` with that draft's fields (you are agent {d}'s desk: it lands `paused: {}` \
             and files a `hire_agent` approval up your line; the user approves it in the Agents view); `discard` → nothing.",
            hire.tool, paused.paused
        ),
        None => format!(
            "`hire` → `{}` with that draft's fields and `paused: {}`; `hire paused` → `{}` with `paused: {}`; \
             `replace` → `{}` over the existing file (`paused: false`); `discard` → nothing. The press is the \
             user's approval: do not `agent_propose`.",
            hire.tool, hire.paused, paused.tool, paused.paused, hire.tool
        ),
    }
}

#[async_trait::async_trait]
impl Tool for AgentDraftTool {
    const NAME: &'static str = "agent_draft";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "dict";
    const CONSUMES_SECRETS: bool = false;
    type Args = AgentDraftArgs;

    fn description(&self) -> String {
        "Propose agents (hires) for the user's approval before anything is written. When the user describes a \
         standing role or asks to hire / add an agent, call `agents_list`, then this with 1..n drafts {name, \
         title, reports_to, model?, effort?, tools?, mcp?, budget?, capabilities?, charter}. Each renders as a \
         card (fields + charter) with three buttons: Hire, Hire paused, Discard — Replace instead of Hire when \
         the name already exists. Then STOP and wait: the press arrives as the user's next message (it begins \
         `[app agent-drafts …]`) naming the draft id and the choice; the receipt says which tool to call for \
         each. Drafts are not agents: nothing exists until approval. Never ask the user to write a file."
            .into()
    }

    async fn call(&self, args: AgentDraftArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        if args.agents.is_empty() {
            return Ok(ToolOutput::err("agent_draft: at least one agent"));
        }
        let Some(roster) = roster::actor() else {
            return Ok(ToolOutput::err("agent_draft: roster is not running"));
        };
        let existing: BTreeSet<String> = match roster.ask(ListAgents).await {
            Ok(l) => l.into_iter().filter(|s| !s.file.name.is_default()).map(|s| s.file.name.to_string()).collect(),
            Err(e) => return Ok(ToolOutput::err(format!("agent_draft: {e}"))),
        };
        let desk = roster.ask(AgentOf(cx.session)).await.ok().flatten().filter(|d| d != AgentName::DEFAULT);
        let mut seen = BTreeSet::new();
        for d in &args.agents {
            if let Err(e) = validate(d, &existing, desk.as_deref()) {
                return Ok(ToolOutput::err(format!("agent_draft: {}: {e}", d.name)));
            }
            if !seen.insert(d.name.as_str()) {
                return Ok(ToolOutput::err(format!("agent_draft: {} is drafted twice", d.name)));
            }
        }
        let alnum: Vec<char> = cx.call_id.chars().filter(char::is_ascii_alphanumeric).collect();
        let suffix: String = alnum[alnum.len().saturating_sub(4)..].iter().collect();
        let (source, ids) = source(&args.agents, &existing, &suffix, desk.as_deref());
        let app = AppArgs {
            name: APP_NAME.into(),
            source,
            title: Some(if ids.len() == 1 { "Agent draft".into() } else { format!("{} agent drafts", ids.len()) }),
            edit: false,
            base_call_id: None,
        };
        let (loaded, _) = match commit_source(&self.core, app, cx.session, cx.call_id.clone(), Origin::Tool).await {
            Ok(v) => v,
            Err(e) => return Ok(ToolOutput::err(format!("agent_draft: {e}"))),
        };
        let lines: Vec<String> = ids
            .iter()
            .zip(&args.agents)
            .map(|(id, d)| {
                format!("{id} · {} ({}){}", d.name, d.title.trim(), if existing.contains(d.name.as_str()) { " — exists: Replace" } else { "" })
            })
            .collect();
        let content = format!(
            "drafted {} agent card{} (record_call_id {}):\n{}\nWait for the user. Their press arrives as a message \
             beginning `[app {APP_NAME} …]` with `agent draft <id>: hire | hire paused | replace | discard`. Then: {}",
            ids.len(),
            if ids.len() == 1 { "" } else { "s" },
            loaded.artifact.call_id,
            lines.join("\n"),
            then(desk.as_deref()),
        );
        Ok(ToolOutput {
            structured: Some(serde_json::json!({
                "record_call_id": loaded.artifact.call_id,
                "drafts": ids.iter().zip(&args.agents).map(|(id, d)| serde_json::json!({ "id": id, "name": d.name, "exists": existing.contains(d.name.as_str()) })).collect::<Vec<_>>(),
            })),
            ..ToolOutput::ok(content)
        })
    }
}

/// The draft's fields as the roster would refuse them, before any card is drawn.
fn validate(d: &AgentDraft, existing: &BTreeSet<String>, desk: Option<&str>) -> Result<(), String> {
    if d.name.is_default() {
        return Err("theuth is reserved (the built-in default agent)".into());
    }
    if d.title.trim().is_empty() {
        return Err("no title".into());
    }
    if d.charter.trim().is_empty() {
        return Err("no charter".into());
    }
    let r = d.reports_to.trim();
    if r != AgentName::DEFAULT && !existing.contains(r) {
        return Err(format!("reports_to {r:?}: no such agent (see agents_list); use `theuth` or an existing name"));
    }
    if let Some(t) = d.tools.as_deref()
        && !matches!(t, "read_only" | "mutating" | "destructive")
    {
        return Err(format!("tools {t:?}: expected read_only | mutating | destructive"));
    }
    if let Some(b) = d.budget.as_deref() {
        b.parse::<roster::Budget>()?;
    }
    if desk.is_some() && existing.contains(d.name.as_str()) {
        return Err("exists; a desk cannot replace an agent (agent_propose refuses taken names)".into());
    }
    Ok(())
}

/// The card set's OpenUI source and the draft ids, one per agent (`a1-<suffix>`, …).
/// Every button's `@ToAssistant` message is `agent draft <id>: <choice>`; a name in
/// `existing` gets *Replace* (`replace`) instead of *Hire* (`hire`), and the card says so.
pub fn source(agents: &[AgentDraft], existing: &BTreeSet<String>, suffix: &str, desk: Option<&str>) -> (String, Vec<String>) {
    let ids: Vec<String> = (1..=agents.len()).map(|i| if suffix.is_empty() { format!("a{i}") } else { format!("a{i}-{suffix}") }).collect();
    let cards: Vec<String> = (1..=agents.len()).map(|i| format!("c{i}")).collect();
    let mut out = format!("root = Stack([intro, {}])\n", cards.join(", "));
    let n = agents.len();
    let s = if n == 1 { "" } else { "s" };
    let intro = match desk {
        Some(d) => format!(
            "**{n} agent draft{s}** proposed by agent `{d}` — *Hire* / *Hire paused* file a `hire_agent` approval up its line \
             (the Agents view's hiring inbox); *Discard* drops the draft."
        ),
        None => format!(
            "**{n} agent draft{s}** — for each: *Hire* (on the roster, active at once), *Hire paused* (on the roster, takes no \
             work until un-paused), or *Discard*. Nothing is written until you press."
        ),
    };
    out.push_str(&format!("intro = TextContent({}, \"small\")\n", lit(&intro)));
    for (i, (d, id)) in agents.iter().zip(&ids).enumerate() {
        let c = &cards[i];
        let exists = existing.contains(d.name.as_str());
        let charter = d.charter.trim();
        let preview: String = if charter.chars().count() > PREVIEW {
            format!("{}…", charter.chars().take(PREVIEW).collect::<String>())
        } else {
            charter.to_owned()
        };
        let mut meta = vec![
            format!("reports to `{}`", d.reports_to.trim()),
            format!("model `{}`", d.model.as_deref().unwrap_or("default")),
            format!("effort `{}`", d.effort.as_deref().unwrap_or("default")),
            format!("tools `{}`", d.tools.as_deref().unwrap_or("destructive")),
            format!("budget `{}`", d.budget.as_deref().unwrap_or("none")),
        ];
        match &d.mcp {
            None => {}
            Some(l) if l.is_empty() => meta.push("mcp none".to_owned()),
            Some(l) => meta.push(format!("mcp {}", l.join(", "))),
        }
        let mut parts = vec![format!("{c}_head"), format!("{c}_charter"), format!("{c}_meta")];
        if d.capabilities.is_some() {
            parts.push(format!("{c}_caps"));
        }
        if exists {
            parts.push(format!("{c}_note"));
        }
        parts.push(format!("{c}_buttons"));
        out.push_str(&format!("{c} = Card([{}])\n", parts.join(", ")));
        out.push_str(&format!(
            "{c}_head = CardHeader({}, {})\n",
            lit(&format!("{} · `{}`", d.title.trim(), d.name)),
            lit(&format!("agent draft {id}"))
        ));
        out.push_str(&format!("{c}_charter = TextContent({})\n", lit(&preview)));
        out.push_str(&format!("{c}_meta = TextContent({}, \"small\")\n", lit(&meta.join(" · "))));
        if let Some(caps) = &d.capabilities {
            out.push_str(&format!("{c}_caps = TextContent({}, \"small\")\n", lit(&format!("**Hand off to it when:** {}", caps.trim()))));
        }
        if exists {
            out.push_str(&format!(
                "{c}_note = TextContent({}, \"small\")\n",
                lit(&format!("`{0}` is already on the roster — *Replace* overwrites agents/{0}.md with this draft.", d.name))
            ));
        }
        out.push_str(&format!("{c}_buttons = Buttons([{c}_hire, {c}_paused, {c}_discard])\n"));
        let (label, choice) = if exists { ("Replace", "replace") } else { ("Hire", "hire") };
        out.push_str(&format!(
            "{c}_hire = Button(\"{label}\", Action([@ToAssistant({})]))\n",
            lit(&format!("agent draft {id}: {choice}"))
        ));
        out.push_str(&format!(
            "{c}_paused = Button(\"Hire paused\", Action([@ToAssistant({})]), \"secondary\")\n",
            lit(&format!("agent draft {id}: hire paused"))
        ));
        out.push_str(&format!(
            "{c}_discard = Button(\"Discard\", Action([@ToAssistant({})]), \"tertiary\", \"destructive\")\n",
            lit(&format!("agent draft {id}: discard"))
        ));
    }
    (out, ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(name: &str, reports_to: &str) -> AgentDraft {
        AgentDraft {
            name: AgentName::new(name).unwrap(),
            title: "Ops".into(),
            reports_to: reports_to.into(),
            model: None,
            effort: None,
            tools: None,
            mcp: None,
            budget: None,
            capabilities: None,
            charter: "You own \"prod\".\nNever deploy on Friday.".into(),
        }
    }

    #[test]
    fn card_buttons_carry_the_draft_id_and_choice_and_replace_when_the_name_exists() {
        let existing: BTreeSet<String> = ["cto".to_owned()].into();
        let drafts = vec![
            AgentDraft { tools: Some("mutating".into()), capabilities: Some("prod incidents".into()), mcp: Some(vec!["gh".into()]), ..draft("ops", "cto") },
            AgentDraft { mcp: Some(vec![]), ..draft("cto", "theuth") },
        ];
        let (src, ids) = source(&drafts, &existing, "ab12", None);
        assert_eq!(ids, ["a1-ab12", "a2-ab12"]);
        assert!(src.starts_with("root = Stack([intro, c1, c2])\n"));
        for (id, c) in ids.iter().zip(["c1", "c2"]) {
            assert!(src.contains(&format!("{c}_paused = Button(\"Hire paused\", Action([@ToAssistant(\"agent draft {id}: hire paused\")]), \"secondary\")")), "{src}");
            assert!(src.contains(&format!("{c}_discard = Button(\"Discard\", Action([@ToAssistant(\"agent draft {id}: discard\")]), \"tertiary\", \"destructive\")")), "{src}");
            assert!(src.contains(&format!("\"agent draft {id}\"")), "the header names the draft");
        }
        assert!(src.contains("c1_hire = Button(\"Hire\", Action([@ToAssistant(\"agent draft a1-ab12: hire\")]))"), "{src}");
        assert!(src.contains("c2_hire = Button(\"Replace\", Action([@ToAssistant(\"agent draft a2-ab12: replace\")]))"), "{src}");
        assert!(src.contains("c2 = Card([c2_head, c2_charter, c2_meta, c2_note, c2_buttons])"));
        assert!(src.contains("`cto` is already on the roster — *Replace* overwrites agents/cto.md"));
        assert!(src.contains("c1 = Card([c1_head, c1_charter, c1_meta, c1_caps, c1_buttons])"));
        assert!(src.contains(r#"c1_head = CardHeader("Ops · `ops`", "agent draft a1-ab12")"#));
        assert!(src.contains(r#"c1_charter = TextContent("You own \"prod\".\nNever deploy on Friday.")"#));
        assert!(src.contains("reports to `cto` · model `default` · effort `default` · tools `mutating` · budget `none` · mcp gh"));
        assert!(src.contains("budget `none` · mcp none"), "`mcp: []` reads as none: {src}");
        assert!(src.contains("**Hand off to it when:** prod incidents"));
        assert!(src.lines().all(|l| l.contains(" = ")));
        // a desk's set says who proposed and where the approval goes
        assert!(source(&drafts[..1], &existing, "", Some("cto")).0.contains("proposed by agent `cto`"));
        // a long charter is cut
        let long = AgentDraft { charter: "x".repeat(PREVIEW + 1), ..draft("ops", "theuth") };
        assert!(source(&[long], &existing, "", None).0.contains(&format!("{}…", "x".repeat(PREVIEW))));
    }

    #[test]
    fn approval_maps_choice_and_proposer() {
        assert_eq!(approval("hire", None), Some(Approval { tool: "agent_put", paused: false }));
        assert_eq!(approval("replace", None), Some(Approval { tool: "agent_put", paused: false }));
        assert_eq!(approval("hire paused", None), Some(Approval { tool: "agent_put", paused: true }));
        assert_eq!(approval("hire", Some("cto")), Some(Approval { tool: "agent_propose", paused: true }));
        assert_eq!(approval("hire paused", Some("cto")), Some(Approval { tool: "agent_propose", paused: true }));
        assert_eq!(approval("discard", None), None);
        assert!(then(None).contains("`hire` → `agent_put` with that draft's fields and `paused: false`"));
        assert!(then(None).contains("`hire paused` → `agent_put` with `paused: true`"));
        assert!(then(Some("cto")).contains("→ `agent_propose`"));
    }

    #[test]
    fn validate_refuses_what_the_roster_would() {
        let existing: BTreeSet<String> = ["cto".to_owned()].into();
        assert!(validate(&draft("ops", "cto"), &existing, None).is_ok());
        assert!(validate(&draft("ops", "theuth"), &existing, None).is_ok());
        assert!(validate(&draft("theuth", "theuth"), &existing, None).unwrap_err().contains("reserved"));
        assert!(validate(&draft("ops", "nobody"), &existing, None).unwrap_err().contains("no such agent"));
        assert!(validate(&AgentDraft { title: " ".into(), ..draft("ops", "cto") }, &existing, None).is_err());
        assert!(validate(&AgentDraft { charter: String::new(), ..draft("ops", "cto") }, &existing, None).is_err());
        assert!(validate(&AgentDraft { tools: Some("all".into()), ..draft("ops", "cto") }, &existing, None).is_err());
        assert!(validate(&AgentDraft { budget: Some("lots".into()), ..draft("ops", "cto") }, &existing, None).is_err());
        assert!(validate(&draft("cto", "theuth"), &existing, None).is_ok(), "the user may replace");
        assert!(validate(&draft("cto", "theuth"), &existing, Some("ops")).unwrap_err().contains("cannot replace"));
        assert!(AgentName::new("Ops").is_err() && AgentName::new("").is_err() && AgentName::new(&"a".repeat(33)).is_err());
    }
}
