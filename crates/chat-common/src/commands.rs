//! Slash-command helpers shared by the Signal, Slack and Telegram adapters: the
//! actor handles, reply formatters and argument parsers whose bodies were
//! byte-identical in each (design docs 09 §9, 21, 98).

use std::fmt::Display;

use agent::{AGENT_ACTOR, Agents, BroadcastOutcome, BroadcastReceipt, Delivery};
use anyhow::Context as _;
use kameo::actor::ActorRef;
use runtime::SessionId;

/// The registered `agent` actor; `Err` when it is not running.
pub fn agent() -> anyhow::Result<ActorRef<Agents>> {
    ActorRef::<Agents>::lookup(AGENT_ACTOR)?.context("agent actor is not running")
}

/// The registered `mcp` actor; `Err` when it is not running.
pub fn mcp() -> anyhow::Result<ActorRef<tools::Mcp>> {
    ActorRef::<tools::Mcp>::lookup(tools::MCP_ACTOR)?.context("mcp actor is not running")
}

/// The registered `jobs` actor; `Err` when it is not running.
pub fn jobs() -> anyhow::Result<ActorRef<tools::Jobs>> {
    ActorRef::<tools::Jobs>::lookup(tools::JOBS_ACTOR)?.context("jobs actor is not running")
}

/// Any displayable error as an [`anyhow::Error`].
pub fn err<E: Display>(e: E) -> anyhow::Error {
    anyhow::anyhow!("{e}")
}

/// `on` / `off`.
pub fn on_off(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

/// Last 8 hex chars, as the TUI shows ids; `/resume` accepts them.
pub fn short(id: SessionId) -> String {
    let s = id.to_string();
    s[s.len() - 8..].to_owned()
}

/// A unit-variant enum's serde word (`Native` → `native`).
pub fn word<T: std::fmt::Debug>(v: &T) -> String {
    format!("{v:?}").to_lowercase()
}

/// `name` is an OAuth provider (else it is taken as an MCP server).
pub async fn is_provider(name: &str) -> anyhow::Result<bool> {
    Ok(agent()?
        .ask(agent::ListLogins)
        .await
        .map_err(err)?
        .iter()
        .any(|l| l.name == name))
}

/// `/mcp add` argument that means "clone a repo": a `.git` URL or a known git host.
pub fn git_url(s: &str) -> bool {
    s.ends_with(".git")
        || ["github.com/", "gitlab.com/", "codeberg.org/"]
            .iter()
            .any(|h| s.contains(h))
}

/// One MCP server from `/mcp add <json>`: a server object with a `name` key
/// (stripped), `{ "<name>": {...} }`, or `{ "mcpServers": { "<name>": {...} } }`.
pub fn mcp_entry(text: &str) -> Result<(String, serde_json::Value), String> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let serde_json::Value::Object(mut obj) = v else {
        return Err("expected a JSON object".into());
    };
    if let Some(serde_json::Value::Object(servers)) = obj.remove("mcpServers") {
        obj = servers;
    }
    if obj.contains_key("command") || obj.contains_key("url") {
        return match obj.remove("name") {
            Some(serde_json::Value::String(name)) => Ok((name, serde_json::Value::Object(obj))),
            _ => Err("server object needs a \"name\" field".into()),
        };
    }
    match (obj.len(), obj.into_iter().next()) {
        (1, Some((name, entry))) if entry.is_object() => Ok((name, entry)),
        _ => Err("expected exactly one server: {\"<name>\": {\"command\": ...}}".into()),
    }
}

/// The one pending id `s` names exactly or as a unique prefix.
pub fn pending_id(page: &agent::PeerMessagePage, s: &str) -> anyhow::Result<uuid::Uuid> {
    let mut hits = page
        .awaiting_answer
        .iter()
        .chain(&page.awaiting_reply)
        .filter(|id| id.to_string().starts_with(s));
    match (hits.next(), hits.next()) {
        (Some(id), None) => Ok(*id),
        (Some(_), Some(_)) => anyhow::bail!("{s:?} matches several pending requests"),
        (None, _) => anyhow::bail!("no pending request {s:?}"),
    }
}

/// Rows as columns padded to equal width inside a ``` fence; `(none)` when empty.
pub fn table(rows: impl IntoIterator<Item = Vec<String>>) -> String {
    let rows: Vec<Vec<String>> = rows.into_iter().collect();
    if rows.is_empty() {
        return "(none)".to_owned();
    }
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..cols)
        .map(|c| {
            rows.iter()
                .map(|r| r.get(c).map_or(0, |s| s.chars().count()))
                .max()
                .unwrap_or(0)
        })
        .collect();
    let lines: Vec<String> = rows
        .iter()
        .map(|r| {
            let mut line = String::new();
            for (c, cell) in r.iter().enumerate() {
                if c > 0 {
                    line.push_str("  ");
                }
                line.push_str(cell);
                if c + 1 < r.len() {
                    line.extend(std::iter::repeat_n(' ', widths[c] - cell.chars().count()));
                }
            }
            line.trim_end().to_owned()
        })
        .collect();
    format!("```\n{}\n```", lines.join("\n"))
}

/// `name  status` rows of a skills / MCP sync, as a [`table`].
pub fn synced(rows: Vec<tools::git::SyncResult>) -> String {
    table(rows.into_iter().map(|r| vec![r.name, r.status]))
}

/// `broadcast <short-id> → N sessions`, then `title — outcome` per row.
pub fn receipt_text(r: &BroadcastReceipt) -> String {
    if r.rows.is_empty() {
        return "no matching sessions".to_owned();
    }
    let id = r.id.simple().to_string();
    let mut out = format!("broadcast {} → {} sessions", &id[..4], r.rows.len());
    for row in &r.rows {
        let title = if row.title.is_empty() {
            short(row.session)
        } else {
            row.title.clone()
        };
        let outcome = match &row.outcome {
            BroadcastOutcome::Started => "started".to_owned(),
            BroadcastOutcome::Queued { delivery, .. } => format!(
                "queued ({})",
                match delivery {
                    Delivery::Steer => "steer",
                    Delivery::NextTurn => "next turn",
                }
            ),
            BroadcastOutcome::Skipped { reason } => format!("skipped: {reason}"),
            BroadcastOutcome::Failed { error } => format!("failed: {error}"),
        };
        out.push_str(&format!("\n{title} — {outcome}"));
    }
    out
}

/// [`agent::CommunicationState`] as the phone reads it: the effective answer
/// first, then the exact blocker when it is off.
pub fn communication_text(s: &agent::CommunicationState) -> String {
    let mut out = format!("communication: {}", on_off(s.effective));
    if !s.effective {
        let why = if !s.global {
            format!("global default is off ({})", s.source)
        } else if s.opted_out {
            "this session opted out".to_owned()
        } else {
            s.inherited_from.map_or_else(
                || "blocked".to_owned(),
                |id| format!("inherited from {}", short(id)),
            )
        };
        out.push_str(&format!(" ({why})"));
    }
    out.push_str(&format!("\nglobal: {} ({})", on_off(s.global), s.source));
    if s.session.is_some() {
        out.push_str(&format!("\nscope: {}", s.scope.as_str()));
        if let Some(alias) = &s.alias {
            out.push_str(&format!(" · alias {alias}"));
        }
    }
    out.push_str(&format!(
        "\nnetwork: {}{}",
        if s.network_enabled {
            "enabled"
        } else {
            "disabled"
        },
        if s.listening { ", listening" } else { "" }
    ));
    out.push_str("\nsession opt-outs last until the daemon restarts; `always` persists");
    out
}

/// The `/broadcast` selector word before id resolution.
#[derive(Debug, PartialEq, Eq)]
pub enum Selector {
    /// `running`: every running session.
    Running,
    /// `roots`: every root session.
    Roots,
    /// Raw ids: full UUIDs or the prefixes `/resume` accepts.
    Ids(Vec<String>),
}

/// `[--next] running|roots|<id>[,<id>…] <text>` → delivery, selector, text.
/// `Err` is the usage line.
pub fn parse_broadcast(arg: &str) -> Result<(Delivery, Selector, String), String> {
    const USAGE: &str = "usage: /broadcast [--next] running|roots|<id>[,<id>…] <text>";
    let (delivery, rest) = match arg.trim().strip_prefix("--next ") {
        Some(r) => (Delivery::NextTurn, r.trim_start()),
        None => (Delivery::Steer, arg.trim()),
    };
    let (sel, text) = rest
        .split_once(char::is_whitespace)
        .map(|(s, t)| (s, t.trim()))
        .filter(|(_, t)| !t.is_empty())
        .ok_or_else(|| USAGE.to_owned())?;
    let selector = match sel {
        "running" => Selector::Running,
        "roots" => Selector::Roots,
        ids => Selector::Ids(
            ids.split(',')
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect(),
        ),
    };
    if matches!(&selector, Selector::Ids(v) if v.is_empty()) {
        return Err(USAGE.to_owned());
    }
    Ok((delivery, selector, text.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broadcast_parses_selector_delivery_and_text() {
        let ok = |s| parse_broadcast(s).unwrap();
        assert_eq!(
            ok("running stop touching Cargo.lock"),
            (
                Delivery::Steer,
                Selector::Running,
                "stop touching Cargo.lock".into()
            )
        );
        assert_eq!(
            ok("--next roots  wrap up"),
            (Delivery::NextTurn, Selector::Roots, "wrap up".into())
        );
        assert_eq!(
            ok("abcd1234,ef56 hi there"),
            (
                Delivery::Steer,
                Selector::Ids(vec!["abcd1234".into(), "ef56".into()]),
                "hi there".into()
            )
        );
        for bad in ["", "running", "--next roots", ", hi", "--next"] {
            assert!(
                parse_broadcast(bad).is_err_and(|u| u.starts_with("usage: /broadcast")),
                "{bad}"
            );
        }
    }

    #[test]
    fn table_aligns_columns() {
        let t = table(vec![
            vec!["a".into(), "one".into()],
            vec!["bbb".into(), "two".into()],
        ]);
        assert_eq!(t, "```\na    one\nbbb  two\n```");
        assert_eq!(table(Vec::<Vec<String>>::new()), "(none)");
    }

    #[test]
    fn mcp_entry_accepts_the_three_shapes() {
        let (n, e) = mcp_entry(r#"{"name":"x","command":"c"}"#).unwrap();
        assert_eq!(n, "x");
        assert_eq!(e["command"], "c");
        assert!(e.get("name").is_none());
        let (n, _) = mcp_entry(r#"{"y":{"url":"u"}}"#).unwrap();
        assert_eq!(n, "y");
        let (n, _) = mcp_entry(r#"{"mcpServers":{"z":{"command":"c"}}}"#).unwrap();
        assert_eq!(n, "z");
        assert!(mcp_entry("[]").is_err());
        assert!(mcp_entry(r#"{"command":"c"}"#).is_err());
    }
}
