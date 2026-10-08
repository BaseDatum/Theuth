// 144-W1 owns this file.
//! Confluence / Jira / any MCP tool (§4.1 rows 6–7): the installed server's tool under its registered
//! name (`tools::Mcp` `ListServers` → `McpToolInfo.registered`), called through the registry like
//! every plugin call. Confluence text (HTML or markdown) → markdown; a Jira issue JSON → `# KEY
//! summary`, a fields table, `## Comments` one block each; anything else is the text as returned.

use std::fmt::Write as _;
use std::time::Duration;

use kameo::actor::ActorRef;

use crate::ingest::{Ctx, Extracted};
use crate::notebook::McpLabel;

const DEADLINE: Duration = Duration::from_secs(120);

/// The registered name of `server/tool`, or why it cannot be called.
async fn registered(server: &str, tool: &str) -> anyhow::Result<String> {
    let mcp = ActorRef::<tools::Mcp>::lookup(tools::MCP_ACTOR)
        .ok()
        .flatten()
        .ok_or_else(|| anyhow::anyhow!("MCP is not running"))?;
    let servers = mcp.ask(tools::ListServers { session: None }).await?;
    let s = servers
        .iter()
        .find(|s| s.name == server)
        .ok_or_else(|| anyhow::anyhow!("no MCP server {server:?} is configured"))?;
    anyhow::ensure!(s.connected, "MCP server {server:?} is not connected");
    let t = s
        .tools
        .iter()
        .find(|t| t.name == tool)
        .ok_or_else(|| anyhow::anyhow!("MCP server {server:?} has no tool {tool:?}"))?;
    t.registered
        .clone()
        .ok_or_else(|| anyhow::anyhow!("MCP tool {server}/{tool} is switched off"))
}

pub async fn extract(
    _ctx: &Ctx,
    server: &str,
    tool: &str,
    args: &serde_json::Value,
    label: McpLabel,
) -> anyhow::Result<Extracted> {
    let reg = super::registry()
        .ok_or_else(|| anyhow::anyhow!("no tool registry: MCP tools are unavailable"))?;
    let name = registered(server, tool).await?;
    let out = super::invoke(&reg, &name, args.clone(), DEADLINE).await?;
    let json = out
        .structured
        .clone()
        .or_else(|| serde_json::from_str(out.content.trim()).ok());
    let (markdown, title, meta) = match (label, &json) {
        (McpLabel::Jira, Some(v))
            if v.get("key").is_some() || v.pointer("/fields/summary").is_some() =>
        {
            jira(v)
        }
        (_, Some(v)) if v.get("body").is_some() || v.get("title").is_some() => {
            confluence(v, &out.content)
        }
        _ => (prose(&out.content), None, serde_json::Value::Null),
    };
    anyhow::ensure!(!markdown.trim().is_empty(), "{name} returned no text");
    let mut meta = match meta {
        serde_json::Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };
    meta.insert("server".into(), server.into());
    meta.insert("tool".into(), tool.into());
    Ok(Extracted {
        bytes: Some(markdown.len() as u64),
        markdown,
        title,
        meta: serde_json::Value::Object(meta),
        pages: None,
        duration_ms: None,
        blob: None,
        origin: None,
    })
}

/// HTML (Confluence storage format) → markdown through anydoc; markdown/plain text as is.
pub fn prose(text: &str) -> String {
    let t = text.trim();
    if looks_like_html(t) {
        if let Ok(md) = html_markdown(t) {
            return md;
        }
    }
    t.to_owned()
}

fn looks_like_html(t: &str) -> bool {
    let head: String = t.chars().take(256).collect::<String>().to_ascii_lowercase();
    head.starts_with('<')
        && [
            "<p", "<div", "<h1", "<h2", "<table", "<ul", "<ol", "<html", "<body", "<ac:", "<span",
        ]
        .iter()
        .any(|tag| head.contains(tag))
}

fn html_markdown(html: &str) -> anyhow::Result<String> {
    use html_to_markdown_rs::{ConversionOptions, TierStrategy, convert};
    let options = ConversionOptions {
        extract_metadata: false,
        skip_images: true,
        tier_strategy: TierStrategy::Tier2,
        ..ConversionOptions::default()
    };
    let md = convert(html, options)?.content.unwrap_or_default();
    anyhow::ensure!(!md.trim().is_empty(), "empty conversion");
    Ok(md)
}

/// A Confluence page object (`{ id, title, body: { storage|view|atlas_doc_format: { value } }, space, version, _links }`).
fn confluence(v: &serde_json::Value, raw: &str) -> (String, Option<String>, serde_json::Value) {
    let title = v.get("title").and_then(|t| t.as_str()).map(str::to_owned);
    let body = [
        "/body/storage/value",
        "/body/view/value",
        "/body/export_view/value",
        "/body/value",
        "/content",
        "/text",
    ]
    .iter()
    .find_map(|p| v.pointer(p).and_then(|b| b.as_str()))
    .unwrap_or(raw);
    let url = v
        .pointer("/_links/webui")
        .and_then(|u| u.as_str())
        .map(
            |u| match v.pointer("/_links/base").and_then(|b| b.as_str()) {
                Some(b) => format!("{b}{u}"),
                None => u.to_owned(),
            },
        )
        .or_else(|| v.get("url").and_then(|u| u.as_str()).map(str::to_owned));
    let space = v
        .pointer("/space/key")
        .or_else(|| v.get("spaceId"))
        .or_else(|| v.get("space"))
        .cloned();
    let version = v
        .pointer("/version/number")
        .or_else(|| v.get("version"))
        .cloned();
    (
        prose(body),
        title,
        serde_json::json!({ "url": url, "space": space, "version": version }),
    )
}

/// A Jira issue object (`{ key, fields: { summary, status, …, comment: { comments } } }`).
fn jira(v: &serde_json::Value) -> (String, Option<String>, serde_json::Value) {
    let key = v.get("key").and_then(|k| k.as_str()).unwrap_or("ISSUE");
    let f = v.get("fields").cloned().unwrap_or(serde_json::Value::Null);
    let summary = f
        .get("summary")
        .or_else(|| v.get("summary"))
        .and_then(|s| s.as_str())
        .unwrap_or("");
    let title = format!("{key} {summary}").trim().to_owned();
    let mut md = format!("# {title}\n\n| field | value |\n|---|---|\n");
    let name = |p: &str| f.pointer(p).and_then(|x| x.as_str()).map(str::to_owned);
    let rows = [
        ("status", name("/status/name")),
        ("type", name("/issuetype/name")),
        ("priority", name("/priority/name")),
        ("assignee", name("/assignee/displayName")),
        ("reporter", name("/reporter/displayName")),
        ("project", name("/project/key")),
        ("created", name("/created")),
        ("updated", name("/updated")),
        ("resolution", name("/resolution/name")),
        (
            "labels",
            f.get("labels")
                .and_then(|l| l.as_array())
                .map(|l| {
                    l.iter()
                        .filter_map(|x| x.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .filter(|s| !s.is_empty()),
        ),
    ];
    for (k, v) in rows.iter().filter_map(|(k, v)| v.as_ref().map(|v| (k, v))) {
        let _ = writeln!(md, "| {k} | {} |", v.replace('|', "\\|").replace('\n', " "));
    }
    if let Some(d) = f.get("description").filter(|d| !d.is_null()) {
        let _ = write!(md, "\n## Description\n\n{}\n", prose(&text_of(d)));
    }
    let comments = f
        .pointer("/comment/comments")
        .or_else(|| v.get("comments"))
        .and_then(|c| c.as_array())
        .cloned()
        .unwrap_or_default();
    if !comments.is_empty() {
        md.push_str("\n## Comments\n\n");
        for c in &comments {
            let who = c
                .pointer("/author/displayName")
                .and_then(|a| a.as_str())
                .unwrap_or("someone");
            let when = c.get("created").and_then(|a| a.as_str()).unwrap_or("");
            let body = c.get("body").map(text_of).unwrap_or_default();
            let _ = write!(md, "**{who}** · {when}\n\n{}\n\n", prose(&body));
        }
    }
    let url = v
        .get("self")
        .and_then(|s| s.as_str())
        .and_then(|s| {
            s.split_once("/rest/")
                .map(|(base, _)| format!("{base}/browse/{key}"))
        })
        .or_else(|| v.get("url").and_then(|u| u.as_str()).map(str::to_owned));
    let meta = serde_json::json!({ "url": url, "status": name("/status/name"), "updated": name("/updated") });
    (md, Some(title), meta)
}

/// Jira bodies are strings, or Atlassian Document Format trees: walk the `text` leaves.
fn text_of(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(o) => {
            let mut s = String::new();
            if let Some(t) = o.get("text").and_then(|t| t.as_str()) {
                s.push_str(t);
            }
            if let Some(c) = o.get("content").and_then(|c| c.as_array()) {
                for (i, child) in c.iter().enumerate() {
                    if i > 0
                        && matches!(
                            o.get("type").and_then(|t| t.as_str()),
                            Some("doc" | "bulletList" | "orderedList")
                        )
                    {
                        s.push_str("\n\n");
                    }
                    s.push_str(&text_of(child));
                }
            }
            s
        }
        serde_json::Value::Array(a) => a.iter().map(text_of).collect::<Vec<_>>().join("\n\n"),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jira_issue_renders_heading_table_and_comment_blocks() {
        let v = serde_json::json!({
            "key": "AB-12", "self": "https://x.atlassian.net/rest/api/3/issue/1",
            "fields": {
                "summary": "Failover flaps", "status": {"name": "Open"}, "updated": "2026-01-02T03:04:05Z",
                "labels": ["ops", "p1"],
                "description": {"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "It flaps."}]}]},
                "comment": {"comments": [{"author": {"displayName": "Ann"}, "created": "2026-01-03", "body": "Seen twice."}]}
            }
        });
        let (md, title, meta) = jira(&v);
        assert_eq!(title.as_deref(), Some("AB-12 Failover flaps"));
        assert!(
            md.starts_with(
                "# AB-12 Failover flaps\n\n| field | value |\n|---|---|\n| status | Open |\n"
            ),
            "{md}"
        );
        assert!(md.contains("| labels | ops, p1 |"));
        assert!(md.contains("## Description\n\nIt flaps.\n"), "{md}");
        assert!(
            md.contains("## Comments\n\n**Ann** · 2026-01-03\n\nSeen twice.\n"),
            "{md}"
        );
        assert_eq!(meta["url"], "https://x.atlassian.net/browse/AB-12");
        assert_eq!(meta["status"], "Open");
    }

    #[test]
    fn confluence_storage_html_becomes_markdown() {
        let v = serde_json::json!({
            "title": "Runbook", "body": {"storage": {"value": "<h2>Steps</h2><p>Turn it <b>off</b> and on.</p>"}},
            "space": {"key": "OPS"}, "version": {"number": 7}, "_links": {"base": "https://w", "webui": "/x/1"}
        });
        let (md, title, meta) = confluence(&v, "");
        assert_eq!(title.as_deref(), Some("Runbook"));
        assert!(md.contains("## Steps"), "{md}");
        assert!(md.contains("**off**"), "{md}");
        assert_eq!(meta["space"], "OPS");
        assert_eq!(meta["version"], 7);
        assert_eq!(meta["url"], "https://w/x/1");
    }
}
