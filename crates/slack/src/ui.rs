//! Block Kit panels for `/theuth` replies (design doc 21 §10.2): pure
//! builders, one per row of the §10.2 table. `commands` fills them;
//! `channel` sends them and routes the `cmd:*` clicks back.
//!
//! Rules kept here: the notification `text` is one line, `section.text`
//! ≤ [`SECTION_MAX`], ≤ [`BLOCKS_MAX`] blocks (lists paginate at
//! [`PER_PAGE`] with a `More…` button), every `action_id` unique per message.

use std::collections::{BTreeMap, HashMap};

use agent::{AgentInfo, AgentState, LoginInfo, ModelInfo, SessionSummary, SkillMeta};
use chrono::{DateTime, Utc};
use runtime::SessionId;
use serde_json::{Value, json};
use tools::{McpAuth, McpServer};

use crate::SlackStatus;
use crate::commands::COMMANDS;

/// Entries per page of a list panel.
pub const PER_PAGE: usize = 20;
/// Slack's `section.text` cap.
pub const SECTION_MAX: usize = 3000;
/// Slack's blocks-per-message cap.
pub const BLOCKS_MAX: usize = 50;
/// `static_select` options cap.
const OPTIONS_MAX: usize = 100;
/// `plain_text` cap in buttons and options.
const LABEL_MAX: usize = 75;

/// A command reply: the one-line notification `text` and, for panels,
/// the Block Kit `blocks` array.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    /// Notification fallback; also the whole reply when `blocks` is `None`.
    pub text: String,
    /// Block Kit array.
    pub blocks: Option<Value>,
}

impl Reply {
    /// Text only.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            blocks: None,
        }
    }

    /// `text` with `blocks`, cut to [`BLOCKS_MAX`].
    fn blocks(text: impl Into<String>, mut blocks: Vec<Value>) -> Self {
        blocks.truncate(BLOCKS_MAX);
        Self {
            text: text.into(),
            blocks: Some(Value::Array(blocks)),
        }
    }
}

/// `items[page]` at [`PER_PAGE`] per page and the next page when there is one.
pub fn paginate<T>(items: &[T], page: usize) -> (&[T], Option<usize>) {
    let start = (page * PER_PAGE).min(items.len());
    let end = (start + PER_PAGE).min(items.len());
    (&items[start..end], (end < items.len()).then_some(page + 1))
}

/// The `/theuth` panel: one section per command group and the common buttons.
#[must_use]
pub fn help() -> Reply {
    const GROUPS: [(&str, &[&str]); 3] = [
        (
            "session",
            &[
                "sessions", "resume", "cancel", "compact", "model", "agents", "prompt", "broadcast",
            ],
        ),
        (
            "harness",
            &["help", "status", "skills", "mcp", "login", "logout"],
        ),
        (
            "setup",
            &[
                "approvals",
                "communication",
                "peers",
                "requests",
                "a2a",
                "slack",
            ],
        ),
    ];
    let mut blocks = vec![header("theuth")];
    for (group, names) in GROUPS {
        let lines: Vec<String> = COMMANDS
            .iter()
            .filter(|(n, _, _)| names.contains(n))
            .map(|(n, a, d)| {
                let arg = if a.is_empty() {
                    String::new()
                } else {
                    format!(" {a}")
                };
                format!("• `/theuth {n}{arg}` — {d}")
            })
            .collect();
        blocks.push(section(&format!("*{group}*\n{}", lines.join("\n"))));
    }
    blocks.push(actions(vec![
        button("cmd:sessions", "Sessions", None),
        button("cmd:agents", "Agents", None),
        button("cmd:models", "Models", None),
        button("cmd:status", "Status", None),
    ]));
    Reply::blocks("theuth: /theuth <sub-command>; /theuth help lists them", blocks)
}

/// `status`: the §13 snapshot as fields, call counts as context, a Refresh button.
#[must_use]
pub fn status(s: &SlackStatus, session_line: Option<&str>) -> Reply {
    let mut blocks = status_blocks(s, session_line);
    blocks.push(actions(vec![button("cmd:refresh", "Refresh", None)]));
    Reply::blocks(format!("status: {}", state(s)), blocks)
}

/// `slack [status]`: the status panel plus who may talk to the bot.
#[must_use]
pub fn slack_panel(s: &SlackStatus) -> Reply {
    let mut blocks = status_blocks(s, None);
    let list = |v: &[String]| {
        if v.is_empty() {
            "-".to_owned()
        } else {
            v.join(", ")
        }
    };
    blocks.push(context(&format!(
        "allowed: {} · channels: {}",
        list(&s.allowed),
        list(&s.channels)
    )));
    Reply::blocks(format!("slack: {}", state(s)), blocks)
}

fn status_blocks(s: &SlackStatus, session_line: Option<&str>) -> Vec<Value> {
    let field = |k: &str, v: &str| mrkdwn(&format!("*{k}*\n{}", esc(v)));
    let fields = vec![
        field("connection", &state(s)),
        field("bot", s.bot_user.as_deref().unwrap_or("-")),
        field("team", s.team.as_deref().unwrap_or("-")),
        field("threads", &s.threads.len().to_string()),
        field("open streams", &s.open_streams.to_string()),
        field("session", session_line.unwrap_or("none")),
    ];
    let mut blocks = vec![
        header("status"),
        json!({ "type": "section", "fields": fields }),
    ];
    let calls: Vec<String> = s
        .calls
        .iter()
        .map(|(m, c)| {
            let mut line = format!("`{m}` {} ok", c.ok);
            if c.err > 0 {
                line.push_str(&format!(" · {} err", c.err));
            }
            if c.rate_limited > 0 {
                line.push_str(&format!(" · {} rate limited", c.rate_limited));
            }
            line
        })
        .collect();
    if !calls.is_empty() {
        blocks.push(context(&calls.join("  ")));
    }
    blocks
}

/// The first line of [`crate::render_status`] without its `state: ` label.
fn state(s: &SlackStatus) -> String {
    crate::render_status(s)
        .lines()
        .next()
        .and_then(|l| l.strip_prefix("state: "))
        .unwrap_or("unknown")
        .to_owned()
}

/// `sessions`: one section + context per top-level session with an
/// overflow menu (Resume here · Cancel · Compact); paged.
#[must_use]
pub fn sessions(rows: &[SessionSummary], page: usize) -> Reply {
    let (slice, more) = paginate(rows, page);
    let mut blocks = vec![header("sessions")];
    if rows.is_empty() {
        blocks.push(section("(none)"));
    }
    for (i, s) in slice.iter().enumerate() {
        let id = s.id.to_string();
        let menu = json!({
            "type": "overflow",
            "action_id": format!("cmd:menu:{i}"),
            "options": [
                option("Resume here", &format!("cmd:resume:{id}")),
                option("Cancel", &format!("cmd:cancel:{id}")),
                option("Compact", &format!("cmd:compact:{id}")),
            ],
        });
        blocks.push(section_with(&format!("*{}*", esc(&s.title)), menu));
        blocks.push(context(&format!(
            "`{}` · {} · {}",
            short(s.id),
            esc(&s.model),
            age(s.updated_at)
        )));
    }
    push_more(&mut blocks, "sessions", more);
    Reply::blocks(format!("sessions: {}", rows.len()), blocks)
}

/// `agents`: one section per `list_agents` row; Cancel while running,
/// an Open-thread link when the child is bound.
#[must_use]
pub fn agents(rows: &[AgentInfo], links: &HashMap<SessionId, String>) -> Reply {
    let mut blocks = vec![header("agents")];
    if rows.is_empty() {
        blocks.push(section("no subagents"));
    }
    for a in rows.iter().take(PER_PAGE) {
        let since = if a.state == AgentState::Running {
            a.started_at
        } else {
            a.finished_at
        };
        let mut text = format!(
            "{} *{}* · {}",
            glyph(&a.state),
            esc(&a.name),
            a.state.word()
        );
        if !a.last.is_empty() {
            text.push_str(&format!(" · {}", esc(&clip(&a.last, 60))));
        }
        if let Some(t) = since {
            text.push_str(&format!(" · {}", age(t)));
        }
        if let Some(link) = links.get(&a.id) {
            text.push_str(&format!(" · <{link}|thread>"));
        }
        blocks.push(if a.state == AgentState::Running {
            section_with(
                &text,
                button(&format!("cmd:cancel:{}", a.id), "Cancel", None),
            )
        } else {
            section(&text)
        });
    }
    if rows.len() > PER_PAGE {
        blocks.push(context(&format!("{} more", rows.len() - PER_PAGE)));
    }
    Reply::blocks(format!("agents: {}", rows.len()), blocks)
}

fn glyph(s: &AgentState) -> &'static str {
    match s {
        AgentState::Idle => "·",
        AgentState::Running => "⠹",
        AgentState::Awaiting { .. } => "⏳",
        AgentState::Approval { .. } => "⚠",
        AgentState::Error | AgentState::Cancelled => "✗",
    }
}

/// `model`: the current model and a select grouped by provider.
#[must_use]
pub fn models(models: &[ModelInfo], current: &str) -> Reply {
    let mut groups: BTreeMap<&str, Vec<Value>> = BTreeMap::new();
    let mut initial = None;
    for m in models.iter().take(OPTIONS_MAX) {
        let label = format!("{} · {}", clip(&m.name, 50), k(m.context));
        let opt = option(&label, &m.id);
        if m.id == current {
            initial = Some(opt.clone());
        }
        groups.entry(m.provider.as_str()).or_default().push(opt);
    }
    let option_groups: Vec<Value> = groups
        .into_iter()
        .map(|(p, options)| json!({ "label": plain(p), "options": options }))
        .collect();
    let mut select = json!({
        "type": "static_select",
        "action_id": "cmd:model",
        "placeholder": plain("Pick a model"),
        "option_groups": option_groups,
    });
    if let Some(i) = initial {
        select["initial_option"] = i;
    }
    let current = if current.is_empty() {
        "-".to_owned()
    } else {
        format!("`{}`", esc(current))
    };
    let mut blocks = vec![section_with(&format!("current: {current}"), select)];
    if models.len() > OPTIONS_MAX {
        blocks.push(context(&format!(
            "{} more; /theuth model <id> to set one",
            models.len() - OPTIONS_MAX
        )));
    }
    Reply::blocks(format!("models: {}", models.len()), blocks)
}

/// A plain result: `text` as sections of ≤ [`SECTION_MAX`] split at lines.
#[must_use]
pub fn result(text: &str) -> Reply {
    let blocks = chunks(text, SECTION_MAX).map(|c| section(&c)).collect();
    Reply::blocks(first_line(text), blocks)
}

/// A code result (`prompt`): each ≤ [`SECTION_MAX`] chunk in its own fence.
#[must_use]
pub fn code(text: &str) -> Reply {
    let blocks = chunks(text, SECTION_MAX - 8)
        .map(|c| section(&format!("```\n{c}\n```")))
        .collect();
    Reply::blocks(first_line(text), blocks)
}

/// `skills`: one section + context per skill; Sync · Add… buttons; paged.
#[must_use]
pub fn skills(rows: &[SkillMeta], page: usize) -> Reply {
    let (slice, more) = paginate(rows, page);
    let mut blocks = vec![header("skills")];
    if rows.is_empty() {
        blocks.push(section("(none)"));
    }
    for s in slice {
        blocks.push(section(&format!(
            "*{}*\n{}",
            esc(&s.name),
            esc(&clip(&s.description, 200))
        )));
        let source = s.repo.as_deref().unwrap_or("local");
        blocks.push(context(&if s.always {
            format!("{source} · always")
        } else {
            source.to_owned()
        }));
    }
    push_more(&mut blocks, "skills", more);
    blocks.push(actions(vec![
        button("cmd:skills:sync", "Sync", None),
        button("cmd:skills:add", "Add…", None),
    ]));
    Reply::blocks(format!("skills: {}", rows.len()), blocks)
}

/// `mcp`: one section + context per server; Sync · Add… buttons; paged.
#[must_use]
pub fn mcp(rows: &[McpServer], page: usize) -> Reply {
    let (slice, more) = paginate(rows, page);
    let mut blocks = vec![header("mcp")];
    if rows.is_empty() {
        blocks.push(section("(none)"));
    }
    for s in slice {
        blocks.push(section(&format!(
            "*{}* · {}",
            esc(&s.name),
            if s.connected { "connected" } else { "off" }
        )));
        let mut ctx = format!(
            "{} · {} tools",
            s.repo.as_deref().unwrap_or("config"),
            s.tools.len()
        );
        match s.auth {
            McpAuth::None => {}
            McpAuth::Required => ctx.push_str(" · login required"),
            McpAuth::LoggedIn => ctx.push_str(" · logged in"),
        }
        blocks.push(context(&ctx));
    }
    push_more(&mut blocks, "mcp", more);
    blocks.push(actions(vec![
        button("cmd:mcp:sync", "Sync", None),
        button("cmd:mcp:add", "Add…", None),
    ]));
    Reply::blocks(format!("mcp: {}", rows.len()), blocks)
}

/// `login` / `logout`: one section per OAuth provider and per MCP server
/// that uses OAuth, each with a Log in / Log out button.
#[must_use]
pub fn logins(providers: &[LoginInfo], servers: &[McpServer]) -> Reply {
    let mut blocks = vec![header("logins")];
    let mut rows: Vec<(&str, bool, String)> = providers
        .iter()
        .map(|l| {
            let state = match (l.logged_in, &l.account) {
                (false, _) => "not logged in".to_owned(),
                (true, Some(a)) => format!("logged in ({})", esc(a)),
                (true, None) => "logged in".to_owned(),
            };
            (l.name.as_str(), l.logged_in, state)
        })
        .collect();
    rows.extend(
        servers
            .iter()
            .filter(|s| s.auth != McpAuth::None && !providers.iter().any(|p| p.name == s.name))
            .map(|s| {
                let logged_in = s.auth == McpAuth::LoggedIn;
                let state = if logged_in {
                    "logged in (mcp)"
                } else {
                    "login required (mcp)"
                };
                (s.name.as_str(), logged_in, state.to_owned())
            }),
    );
    if rows.is_empty() {
        blocks.push(section("(none)"));
    }
    for (name, logged_in, state) in rows.iter().take(PER_PAGE) {
        let btn = if *logged_in {
            button(&format!("cmd:logout:{name}"), "Log out", None)
        } else {
            button(&format!("cmd:login:{name}"), "Log in", None)
        };
        blocks.push(section_with(&format!("*{}* · {state}", esc(name)), btn));
    }
    Reply::blocks(format!("logins: {}", rows.len()), blocks)
}

/// `login <name>`: the URL to open as a link button.
#[must_use]
pub fn login_url(name: &str, url: &str) -> Reply {
    let mut btn = button("cmd:link", "Open", None);
    btn["url"] = json!(url);
    Reply::blocks(
        format!("login: open {url}"),
        vec![section_with(
            &format!("log in to *{}*: <{url}|{url}>", esc(name)),
            btn,
        )],
    )
}

/// `approvals`: radio buttons ask · off this session · off always;
/// `current` is one of those values when known.
#[must_use]
pub fn approvals(current: Option<&str>) -> Reply {
    let options = [
        ("ask", "ask"),
        ("off_session", "off for this session"),
        ("off_always", "off always"),
    ];
    let opt = |(v, l): &(&str, &str)| json!({ "text": plain(l), "value": v });
    let mut radio = json!({
        "type": "radio_buttons",
        "action_id": "cmd:approvals",
        "options": options.iter().map(opt).collect::<Vec<_>>(),
    });
    if let Some(i) = options.iter().find(|(v, _)| Some(*v) == current) {
        radio["initial_option"] = opt(i);
    }
    Reply::blocks("approvals", vec![section_with("*approval prompts*", radio)])
}

/// The Add… modal for `skills` or `mcp`: `callback_id` `cmd:<kind>:add`,
/// one `plain_text_input` in block `url`, `private_metadata` = `channel`.
#[must_use]
pub fn modal_add(kind: &str, channel: &str) -> Value {
    let (label, placeholder, multiline) = if kind == "mcp" {
        (
            "Git URL or server JSON",
            "https://… or {\"name\": {\"command\": …}}",
            true,
        )
    } else {
        ("Git URL", "https://github.com/owner/skills.git", false)
    };
    json!({
        "type": "modal",
        "callback_id": format!("cmd:{kind}:add"),
        "private_metadata": channel,
        "title": plain(&format!("Add {kind}")),
        "submit": plain("Add"),
        "close": plain("Cancel"),
        "blocks": [{
            "type": "input",
            "block_id": "url",
            "label": plain(label),
            "element": {
                "type": "plain_text_input",
                "action_id": "url",
                "multiline": multiline,
                "placeholder": plain(placeholder),
            },
        }],
    })
}

// -- pieces ------------------------------------------------------------

fn push_more(blocks: &mut Vec<Value>, panel: &str, more: Option<usize>) {
    if let Some(n) = more {
        blocks.push(actions(vec![button(
            &format!("cmd:{panel}:page:{n}"),
            "More…",
            None,
        )]));
    }
}

fn header(text: &str) -> Value {
    json!({ "type": "header", "text": plain(text) })
}

fn section(text: &str) -> Value {
    json!({ "type": "section", "text": mrkdwn(&clip(text, SECTION_MAX)) })
}

fn section_with(text: &str, accessory: Value) -> Value {
    json!({ "type": "section", "text": mrkdwn(&clip(text, SECTION_MAX)), "accessory": accessory })
}

fn context(text: &str) -> Value {
    json!({ "type": "context", "elements": [mrkdwn(&clip(text, 2000))] })
}

fn actions(elements: Vec<Value>) -> Value {
    json!({ "type": "actions", "elements": elements })
}

fn button(action_id: &str, label: &str, value: Option<&str>) -> Value {
    let mut b = json!({ "type": "button", "action_id": action_id, "text": plain(label) });
    if let Some(v) = value {
        b["value"] = json!(v);
    }
    b
}

fn option(label: &str, value: &str) -> Value {
    json!({ "text": plain(label), "value": value })
}

fn plain(text: &str) -> Value {
    json!({ "type": "plain_text", "text": clip(text, LABEL_MAX) })
}

fn mrkdwn(text: &str) -> Value {
    json!({ "type": "mrkdwn", "text": text })
}

/// Escapes the three mrkdwn control characters in user-provided text.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// `s` cut to `max` chars with a `…` when it was longer.
fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// `text` split at line breaks into pieces of ≤ `max` chars (a longer line
/// is cut); at least one piece.
fn chunks(text: &str, max: usize) -> impl Iterator<Item = String> + '_ {
    let mut pieces = Vec::new();
    let mut cur = String::new();
    for line in text.lines() {
        let line = clip(line, max);
        if !cur.is_empty() && cur.chars().count() + 1 + line.chars().count() > max {
            pieces.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push('\n');
        }
        cur.push_str(&line);
    }
    if !cur.is_empty() || pieces.is_empty() {
        pieces.push(cur);
    }
    pieces.into_iter()
}

fn first_line(text: &str) -> String {
    clip(text.lines().next().unwrap_or(""), 200)
}

/// Last 8 hex chars, as the TUI shows ids; `resume` accepts them.
pub(crate) fn short(id: SessionId) -> String {
    let s = id.to_string();
    s[s.len() - 8..].to_owned()
}

/// `12.3k`, `200k`, `1.0M`; `?` for 0.
pub(crate) fn k(n: u32) -> String {
    match n {
        0 => "?".to_owned(),
        1..1000 => n.to_string(),
        1000..1_000_000 => format!("{:.1}k", f64::from(n) / 1000.0),
        _ => format!("{:.1}M", f64::from(n) / 1_000_000.0),
    }
}

/// `5s`, `12m`, `3h`, `2d` since `t`.
pub(crate) fn age(t: DateTime<Utc>) -> String {
    let s = (Utc::now() - t).num_seconds().max(0);
    match s {
        ..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86400 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `action_id` in `blocks` (buttons, selects, overflows, radios).
    fn action_ids(v: &Value) -> Vec<String> {
        let mut out = Vec::new();
        fn walk(v: &Value, out: &mut Vec<String>) {
            match v {
                Value::Object(o) => {
                    if let Some(Value::String(id)) = o.get("action_id") {
                        out.push(id.clone());
                    }
                    o.values().for_each(|v| walk(v, out));
                }
                Value::Array(a) => a.iter().for_each(|v| walk(v, out)),
                _ => {}
            }
        }
        walk(v, &mut out);
        out
    }

    fn assert_unique(reply: &Reply) {
        let ids = action_ids(reply.blocks.as_ref().unwrap());
        let set: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(ids.len(), set.len(), "duplicate action_id in {ids:?}");
        assert!(ids.iter().all(|i| i.starts_with("cmd:")), "{ids:?}");
        assert!(reply.blocks.as_ref().unwrap().as_array().unwrap().len() <= BLOCKS_MAX);
        assert!(!reply.text.contains('\n'), "{:?}", reply.text);
    }

    fn session(i: usize) -> SessionSummary {
        SessionSummary {
            id: SessionId::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            title: format!("task <{i}>"),
            tags: Vec::new(),
            model: "openai/gpt-5".into(),
            parent: None,
            name: None,
            effort: None,
            agent: None,
            cwd: None,
            harness_model: None,
            desk: None,
            pinned: false,
            archived: false,
            project: None,
            retention: agent::Retention::Durable,
            running: false,
            owner: String::new(),
            epoch: 1,
            here: true,
        }
    }

    fn status_fixture() -> SlackStatus {
        SlackStatus {
            state: Some(crate::SlackEvent::Connected { connections: 2 }),
            bot_user: Some("UBOT".into()),
            team: Some("T1".into()),
            owner: "U0".into(),
            allowed: vec!["U1".into()],
            channels: vec![],
            threads: vec![],
            open_streams: 1,
            calls: [(
                "chat.postMessage".to_owned(),
                crate::CallStats {
                    ok: 3,
                    err: 1,
                    rate_limited: 0,
                },
            )]
            .into_iter()
            .collect(),
            dropped_envelopes: 0,
        }
    }

    #[test]
    fn every_panel_has_unique_cmd_action_ids_and_one_line_text() {
        let rows: Vec<SessionSummary> = (0..3).map(session).collect();
        let agents_rows = vec![AgentInfo {
            id: SessionId::new(),
            name: "grep".into(),
            parent: SessionId::new(),
            depth: 1,
            agent: None,
            model: "m".into(),
            state: AgentState::Running,
            started_at: Some(Utc::now()),
            finished_at: None,
            last: "looking".into(),
            project: None,
            retention: agent::Retention::Durable,
            activity: Default::default(),
            cap: None,
        }];
        let models_rows = vec![
            ModelInfo {
                id: "openai/gpt-5".into(),
                name: "GPT-5".into(),
                provider: "openai".into(),
                context: 400_000,
                reasoning: true,
                image: true,
                efforts: vec![],
                budget_min: None,
                budget_max: None,
            },
            ModelInfo {
                id: "anthropic/claude".into(),
                name: "Claude".into(),
                provider: "anthropic".into(),
                context: 200_000,
                reasoning: true,
                image: true,
                efforts: vec![],
                budget_min: None,
                budget_max: None,
            },
        ];
        let skills_rows = vec![SkillMeta {
            name: "caveman".into(),
            description: "terse".into(),
            path: "/x".into(),
            always: true,
            repo: Some("skills".into()),
        }];
        let logins_rows = vec![LoginInfo {
            name: "openai".into(),
            logged_in: true,
            account: Some("a@b".into()),
            expires_at: None,
        }];
        for reply in [
            help(),
            status(&status_fixture(), Some("1a2b3c4d · gpt-5 · idle")),
            slack_panel(&status_fixture()),
            sessions(&rows, 0),
            agents(&agents_rows, &HashMap::new()),
            models(&models_rows, "openai/gpt-5"),
            skills(&skills_rows, 0),
            mcp(&[], 0),
            logins(&logins_rows, &[]),
            login_url("openai", "https://x/y"),
            approvals(Some("ask")),
        ] {
            assert_unique(&reply);
        }
    }

    #[test]
    fn sessions_panel_has_one_overflow_per_session() {
        let rows: Vec<SessionSummary> = (0..3).map(session).collect();
        let r = sessions(&rows, 0);
        assert_eq!(r.text, "sessions: 3");
        let ids = action_ids(r.blocks.as_ref().unwrap());
        assert_eq!(ids, ["cmd:menu:0", "cmd:menu:1", "cmd:menu:2"]);
        let s = serde_json::to_string(&r.blocks).unwrap();
        assert!(s.contains(&format!("cmd:resume:{}", rows[0].id)));
        assert!(s.contains("task &lt;0&gt;"), "titles are escaped");
        assert!(s.contains(&short(rows[2].id)));
    }

    #[test]
    fn forty_five_sessions_paginate_to_twenty_and_more() {
        let rows: Vec<SessionSummary> = (0..45).map(session).collect();
        let r = sessions(&rows, 0);
        let ids = action_ids(r.blocks.as_ref().unwrap());
        assert_eq!(ids.len(), 21);
        assert_eq!(ids.last().unwrap(), "cmd:sessions:page:1");
        let r = sessions(&rows, 2);
        let ids = action_ids(r.blocks.as_ref().unwrap());
        assert_eq!(ids.len(), 5, "last page has 5 sessions and no More");
        let (slice, more) = paginate(&rows, 9);
        assert!(slice.is_empty() && more.is_none());
    }

    #[test]
    fn model_select_marks_the_current_one() {
        let m = ModelInfo {
            id: "openai/gpt-5".into(),
            name: "GPT-5".into(),
            provider: "openai".into(),
            context: 0,
            reasoning: false,
            image: false,
            efforts: vec![],
            budget_min: None,
            budget_max: None,
        };
        let r = models(&[m], "openai/gpt-5");
        let sel = &r.blocks.unwrap()[0]["accessory"];
        assert_eq!(sel["action_id"], "cmd:model");
        assert_eq!(sel["initial_option"]["value"], "openai/gpt-5");
        assert_eq!(sel["option_groups"][0]["label"]["text"], "openai");
    }

    #[test]
    fn long_results_split_into_sections_under_the_cap() {
        let text = (0..200)
            .map(|i| format!("line {i} {}", "x".repeat(40)))
            .collect::<Vec<_>>()
            .join("\n");
        let r = code(&text);
        let blocks = r.blocks.unwrap();
        assert!(blocks.as_array().unwrap().len() >= 3);
        for b in blocks.as_array().unwrap() {
            let t = b["text"]["text"].as_str().unwrap();
            assert!(t.chars().count() <= SECTION_MAX);
            assert!(t.starts_with("```\n") && t.ends_with("\n```"));
        }
        assert_eq!(r.text, "line 0 ".to_owned() + &"x".repeat(40));
    }

    #[test]
    fn modal_carries_callback_channel_and_url_input() {
        let m = modal_add("mcp", "C1");
        assert_eq!(m["callback_id"], "cmd:mcp:add");
        assert_eq!(m["private_metadata"], "C1");
        assert_eq!(m["blocks"][0]["block_id"], "url");
        assert_eq!(m["blocks"][0]["element"]["action_id"], "url");
        assert_eq!(m["blocks"][0]["element"]["multiline"], true);
        // Morphism must be able to send it.
        let _: slack_morphism::prelude::SlackView = serde_json::from_value(m).unwrap();
    }
}
