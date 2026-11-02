//! Message attachments as agent input: a forwarded (`is_share`) or linked
//! (`is_msg_unfurl`) message becomes a header line, its quoted body and where
//! the original lives; any other attachment one short line.

use serde_json::Value;
use slack_morphism::prelude::*;

use crate::channel::clip;

/// One forwarded body, in chars.
const BODY_MAX: usize = 8000;
/// A non-message attachment's text, in chars.
const OTHER_MAX: usize = 500;

/// One block of lines per attachment, in order.
pub(crate) fn render(attachments: &[SlackMessageAttachment]) -> Vec<String> {
    attachments
        .iter()
        .map(|a| {
            if a.is_share == Some(true) || a.is_msg_unfurl == Some(true) {
                shared(a)
            } else {
                other(a)
            }
        })
        .collect()
}

/// `[forwarded message from A · #chan · time · permalink]`, `> body`, `(channel …, ts …)`.
fn shared(a: &SlackMessageAttachment) -> String {
    let origin = a.message_blocks.as_ref().and_then(|b| b.first());
    let kind = if a.is_share == Some(true) {
        "forwarded message"
    } else {
        "linked message"
    };
    let author = [&a.author_name, &a.author_subname]
        .into_iter()
        .flatten()
        .find(|s| !s.trim().is_empty())
        .cloned()
        .or_else(|| a.author_id.as_ref().map(|u| format!("<@{}>", u.0)))
        .unwrap_or_else(|| "unknown".to_owned());
    let channel = a
        .channel_id
        .as_ref()
        .or_else(|| origin.and_then(|o| o.channel.as_ref()))
        .map(|c| c.0.as_str());
    let ts =
        a.ts.as_ref()
            .or_else(|| origin.and_then(|o| o.ts.as_ref()))
            .map(|t| t.0.as_str());

    let mut head = vec![format!("{kind} from {author}")];
    if let Some(name) = a.channel_name.as_deref().filter(|n| !n.is_empty()) {
        head.push(format!("#{name}"));
    }
    if let Some(when) = ts.and_then(when) {
        head.push(when);
    }
    if let Some(url) = &a.from_url {
        head.push(url.to_string());
    }
    let mut out = format!("[{}]", head.join(" · "));

    let body = a
        .text
        .clone()
        .filter(|t| !t.trim().is_empty())
        .or_else(|| {
            let md = blocks_markdown(origin?.message.as_ref()?.blocks.as_ref()?);
            (!md.trim().is_empty()).then_some(md)
        })
        .unwrap_or_default();
    let body = match body.char_indices().nth(BODY_MAX) {
        Some((cut, _)) => format!("{}\n[truncated]", &body[..cut]),
        None => body,
    };
    for line in body.trim_end().lines() {
        out.push_str(if line.is_empty() { "\n>" } else { "\n> " });
        out.push_str(line);
    }
    if let Some(files) = a.files.as_ref().filter(|f| !f.is_empty()) {
        let names: Vec<&str> = files
            .iter()
            .map(|f| f.name.as_deref().unwrap_or(f.id.0.as_str()))
            .collect();
        out.push_str(&format!("\n(files: {})", names.join(", ")));
    }
    let mut source = Vec::new();
    if let Some(c) = channel {
        source.push(format!("channel {c}"));
    }
    if let Some(t) = ts {
        source.push(format!("ts {t}"));
    }
    if let Some(t) = a.from_url.as_ref().and_then(|u| {
        u.query_pairs()
            .find(|(k, _)| k == "thread_ts")
            .map(|(_, v)| v.into_owned())
    }) {
        source.push(format!("thread_ts {t}"));
    }
    if !source.is_empty() {
        out.push_str(&format!("\n({})", source.join(", ")));
    }
    out
}

/// `[attachment from <service>] <text>` for a bot attachment or link preview.
fn other(a: &SlackMessageAttachment) -> String {
    let nonempty = |s: &Option<String>| s.clone().filter(|s| !s.trim().is_empty());
    let label = nonempty(&a.service_name)
        .or_else(|| nonempty(&a.author_name))
        .or_else(|| nonempty(&a.title));
    let body = nonempty(&a.text)
        .or_else(|| nonempty(&a.fallback))
        .or_else(|| nonempty(&a.pretext))
        .or_else(|| nonempty(&a.title))
        .or_else(|| a.blocks.as_ref().map(|b| blocks_markdown(b)));
    let mut out = match label {
        Some(l) => format!("[attachment from {l}]"),
        None => "[attachment]".to_owned(),
    };
    if let Some(b) = body.filter(|b| !b.trim().is_empty()) {
        out.push(' ');
        out.push_str(&clip(b.trim(), OTHER_MAX));
    }
    if let Some(url) = a
        .original_url
        .as_ref()
        .map(ToString::to_string)
        .or_else(|| nonempty(&a.title_link))
    {
        out.push_str(&format!(" ({url})"));
    }
    out
}

/// `1761832920.123456` as `2026-10-30 14:02 UTC`.
fn when(ts: &str) -> Option<String> {
    let secs: i64 = ts.split('.').next()?.parse().ok()?;
    chrono::DateTime::from_timestamp(secs, 0).map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
}

/// `rich_text` (and plain `text`) blocks as markdown: preformatted as fenced
/// code, quotes as `> `, lists as `- ` / `1. `.
fn blocks_markdown(blocks: &[SlackBlock]) -> String {
    let mut out: Vec<String> = Vec::new();
    for b in blocks {
        let Ok(b) = serde_json::to_value(b) else {
            continue;
        };
        if b["type"] != "rich_text" {
            if let Some(t) = b["text"]["text"].as_str() {
                out.push(t.to_owned());
            }
            continue;
        }
        for el in b["elements"].as_array().into_iter().flatten() {
            let inner = || inline(&el["elements"]);
            match el["type"].as_str() {
                Some("rich_text_preformatted") => {
                    out.push(format!("```\n{}\n```", inner().trim_end_matches('\n')));
                }
                Some("rich_text_quote") => out.push(
                    inner()
                        .trim_end_matches('\n')
                        .lines()
                        .map(|l| format!("> {l}"))
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
                Some("rich_text_list") => {
                    let ordered = el["style"] == "ordered";
                    let pad = "  ".repeat(el["indent"].as_u64().unwrap_or(0) as usize);
                    let items = el["elements"].as_array().into_iter().flatten();
                    for (i, item) in items.enumerate() {
                        let bullet = if ordered {
                            format!("{}.", i + 1)
                        } else {
                            "-".to_owned()
                        };
                        out.push(format!(
                            "{pad}{bullet} {}",
                            inline(&item["elements"]).trim_end()
                        ));
                    }
                }
                _ => out.push(inner().trim_end_matches('\n').to_owned()),
            }
        }
    }
    out.join("\n")
}

/// One rich-text run list as text; mentions keep Slack's `<@U…>` / `<#C…>` form.
fn inline(elements: &Value) -> String {
    let mut s = String::new();
    for e in elements.as_array().into_iter().flatten() {
        let text = e["text"].as_str();
        match e["type"].as_str() {
            Some("text") if e["style"]["code"] == true => {
                s.push_str(&format!("`{}`", text.unwrap_or_default()));
            }
            Some("link") => {
                let url = e["url"].as_str().unwrap_or_default();
                match text.filter(|t| !t.is_empty()) {
                    Some(t) => s.push_str(&format!("[{t}]({url})")),
                    None => s.push_str(url),
                }
            }
            Some("user") => {
                s.push_str(&format!("<@{}>", e["user_id"].as_str().unwrap_or_default()))
            }
            Some("channel") => {
                s.push_str(&format!(
                    "<#{}>",
                    e["channel_id"].as_str().unwrap_or_default()
                ));
            }
            Some("usergroup") => {
                s.push_str(&format!(
                    "<!subteam^{}>",
                    e["usergroup_id"].as_str().unwrap_or_default()
                ));
            }
            Some("emoji") => s.push_str(&format!(":{}:", e["name"].as_str().unwrap_or_default())),
            Some("broadcast") => {
                s.push_str(&format!("@{}", e["range"].as_str().unwrap_or_default()))
            }
            Some("date") => s.push_str(e["fallback"].as_str().unwrap_or_default()),
            _ => s.push_str(text.unwrap_or_default()),
        }
    }
    s
}
