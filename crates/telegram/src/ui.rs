//! Rich-message panels and inline keyboards for the command replies (design
//! doc 98 §10.3 table), the `cmd:` press router [`action`], and [`guard`],
//! the one rewrite the sender applies to Markdown (98 §8.1 (a)).
//!
//! Rules kept here: every `callback_data` ≤ [`DATA_MAX`] bytes (session ids
//! travel as their last 8 chars), ≤ [`BUTTONS_MAX`] buttons per keyboard,
//! ≤ [`ROW_MAX`] per row; lists paginate with a `More…` button.

use std::collections::BTreeMap;

use agent::{AgentInfo, AgentState, LoginInfo, ModelInfo, SessionSummary, SkillMeta};
pub use chat_common::short;
use chrono::{DateTime, Utc};
use runtime::SessionId;
use tools::{McpAuth, McpServer};

use crate::TelegramStatus;
use crate::api::{Button, Keyboard};
use crate::commands::{self, COMMANDS, CommandCx, Expect, Outcome, Reply};

/// Telegram's `callback_data` cap in bytes.
pub const DATA_MAX: usize = 64;
/// Buttons per keyboard.
pub const BUTTONS_MAX: usize = 100;
/// Buttons per row.
pub const ROW_MAX: usize = 8;
/// Sessions per page (`/sessions`: the 10 newest).
pub const SESSIONS_PER_PAGE: usize = 10;
/// Models per page.
pub const MODELS_PER_PAGE: usize = 24;
/// Lines per page of `/skills` and `/mcp`.
pub const PER_PAGE: usize = 20;

/// A `cmd:` press (`data` without the prefix): the command it stands for,
/// run through [`commands::handle`] so the same gates apply; `page:<panel>:<n>`
/// re-renders page `n`. `None` for data the panels never emit.
pub async fn action(mut cx: CommandCx<'_>, data: &str) -> Option<Outcome> {
    let parts: Vec<&str> = data.splitn(3, ':').collect();
    let (name, arg): (&str, String) = match parts.as_slice() {
        ["sessions"] => ("sessions", String::new()),
        ["agents"] => ("agents", String::new()),
        ["models"] => ("model", String::new()),
        ["status" | "refresh"] => ("status", String::new()),
        ["resume", id] => ("resume", (*id).to_owned()),
        ["cancel", id] => ("cancel", (*id).to_owned()),
        ["close", id] => {
            return Some(match commands::resolve(&cx, id).await {
                Ok(Ok(s)) => Outcome::CloseTopic(s.id),
                Ok(Err(msg)) => Outcome::Reply(Reply::text(msg)),
                Err(e) => Outcome::Reply(Reply::text(format!("error: {e:#}"))),
            });
        }
        ["model", id] => ("model", (*id).to_owned()),
        ["skills", "sync"] => ("skills", "sync".to_owned()),
        ["mcp", "sync"] => ("mcp", "sync".to_owned()),
        ["skills", "add"] => return Some(commands::prompt(Expect::SkillsAdd)),
        ["mcp", "add"] => return Some(commands::prompt(Expect::McpAdd)),
        ["login", name] => ("login", (*name).to_owned()),
        ["logout", name] => ("logout", (*name).to_owned()),
        ["approvals", "ask"] => ("approvals", "on".to_owned()),
        ["approvals", "off_session"] => ("approvals", "off".to_owned()),
        ["approvals", "off_always"] => ("approvals", "off always".to_owned()),
        ["page", panel, n] => {
            cx.page = n.parse().ok()?;
            let name = match *panel {
                "sessions" => "sessions",
                "models" => "model",
                "skills" => "skills",
                "mcp" => "mcp",
                _ => return None,
            };
            (name, String::new())
        }
        _ => return None,
    };
    Some(commands::handle(cx, name, &arg).await)
}

/// Tags Rich Markdown knows (98 §8.1 (a)); a `<` opening one is left alone.
const KNOWN_TAGS: &[&str] = &[
    "details",
    "summary",
    "blockquote",
    "footer",
    "tg-thinking",
    "tg-button",
    "tg-button-row",
    "tg-spoiler",
    "u",
    "s",
    "b",
    "i",
    "code",
    "pre",
    "a",
    "sub",
    "sup",
    "mark",
];

/// Escapes bare `<`, `>` and `&` outside code spans and fenced code blocks so
/// Rich Markdown does not read them as HTML: `Vec<String>` in prose becomes
/// `Vec&lt;String&gt;`; a fence, an inline code span, a known tag (whole, to
/// its `>`), a line-leading `>` quote marker and an existing entity
/// (`&amp;`) are untouched.
// NEEDS(integrator): `pulldown_cmark` is not reachable (not a dep; `markdown`
// re-exports only `Alignment`), so fences and code spans are found with the
// CommonMark backtick rules below (fence: ≥ 3 backticks or tildes at line
// start, closed by a run at least as long of the same char; span: a run of
// N backticks closed by the next run of exactly N on any line). Indented
// code blocks are not recognised.
#[must_use]
pub fn guard(md: &str) -> String {
    let mut out = String::with_capacity(md.len() + 16);
    let mut fence: Option<(u8, usize)> = None;
    for line in md.split_inclusive('\n') {
        let body = line.trim_start_matches(' ');
        let run = body
            .bytes()
            .take_while(|b| *b == b'`' || *b == b'~')
            .count();
        let marker = (run >= 3 && body.bytes().take(run).all(|b| b == body.as_bytes()[0]))
            .then(|| (body.as_bytes()[0], run));
        match (fence, marker) {
            (Some((c, n)), Some((mc, mn)))
                if mc == c && mn >= n && body[run..].trim().is_empty() =>
            {
                fence = None;
                out.push_str(line);
                continue;
            }
            (Some(_), _) => {
                out.push_str(line);
                continue;
            }
            (None, Some((c, n))) if c == b'~' || !body[run..].contains('`') => {
                fence = Some((c, n));
                out.push_str(line);
                continue;
            }
            _ => {}
        }
        escape_line(line, &mut out);
    }
    out
}

/// One non-fence line: escapes outside backtick spans.
fn escape_line(line: &str, out: &mut String) {
    let bytes = line.as_bytes();
    let mut i = 0;
    // `>` is a quote marker while only spaces and `>` precede it.
    let mut quote_lead = true;
    while i < bytes.len() {
        let b = bytes[i];
        if quote_lead && b != b' ' && b != b'>' {
            quote_lead = false;
        }
        match b {
            b'`' => {
                let n = bytes[i..].iter().take_while(|b| **b == b'`').count();
                match close_of(bytes, i + n, n) {
                    Some(end) => {
                        out.push_str(&line[i..end + n]);
                        i = end + n;
                    }
                    None => {
                        out.push_str(&line[i..i + n]);
                        i += n;
                    }
                }
            }
            b'<' if known_tag(&line[i..]) => {
                let end = line[i..].find('>').map_or(line.len(), |e| i + e + 1);
                out.push_str(&line[i..end]);
                i = end;
            }
            b'<' => {
                out.push_str("&lt;");
                i += 1;
            }
            b'>' if !quote_lead => {
                out.push_str("&gt;");
                i += 1;
            }
            b'&' if !entity(&line[i..]) => {
                out.push_str("&amp;");
                i += 1;
            }
            _ => {
                let ch = line[i..].chars().next().expect("in bounds");
                out.push(ch);
                i += ch.len_utf8();
            }
        }
    }
}

/// Index of the next run of exactly `n` backticks at or after `from`.
fn close_of(bytes: &[u8], mut from: usize, n: usize) -> Option<usize> {
    while from < bytes.len() {
        if bytes[from] == b'`' {
            let run = bytes[from..].iter().take_while(|b| **b == b'`').count();
            if run == n {
                return Some(from);
            }
            from += run;
        } else {
            from += 1;
        }
    }
    None
}

/// `s` starts with `<tag`, `</tag` (a [`KNOWN_TAGS`] name) followed by a
/// space, `>` or `/`.
fn known_tag(s: &str) -> bool {
    let rest = s[1..].strip_prefix('/').unwrap_or(&s[1..]);
    let name_len = rest
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'-')
        .count();
    let name = &rest[..name_len];
    KNOWN_TAGS.contains(&name)
        && matches!(
            rest.as_bytes().get(name_len),
            Some(b' ' | b'>' | b'/' | b'\n')
        )
}

/// `s` starts with a character or numeric entity (`&amp;`, `&#60;`, `&#x3c;`).
fn entity(s: &str) -> bool {
    let rest = &s[1..];
    let body = rest.strip_prefix("#x").or_else(|| rest.strip_prefix('#'));
    let (alnum, min) = match body {
        Some(b) => (b, 1),
        None => (rest, 2),
    };
    let n = alnum.bytes().take_while(u8::is_ascii_alphanumeric).count();
    n >= min && alnum.as_bytes().get(n) == Some(&b';')
}

/// A GFM pipe table; the first row is the header. Cells have `|` escaped.
#[must_use]
pub fn table(rows: impl IntoIterator<Item = Vec<String>>) -> String {
    let mut rows = rows.into_iter();
    let Some(header) = rows.next() else {
        return String::new();
    };
    let line = |cells: &[String]| {
        let cells: Vec<String> = cells.iter().map(|c| c.replace('|', "\\|")).collect();
        format!("| {} |", cells.join(" | "))
    };
    let mut out = line(&header);
    out.push('\n');
    out.push_str(&format!("|{}", " --- |".repeat(header.len())));
    for row in rows {
        out.push('\n');
        out.push_str(&line(&row));
    }
    out
}

/// The help panel: one `<details>` per group, buttons Sessions · Agents ·
/// Models · Status.
#[must_use]
pub fn help() -> Reply {
    const GROUPS: [(&str, &[&str]); 3] = [
        (
            "session",
            &[
                "new",
                "sessions",
                "resume",
                "cancel",
                "compact",
                "clear",
                "undo",
                "redo",
                "model",
                "effort",
                "agents",
                "jobs",
                "prompt",
                "broadcast",
            ],
        ),
        (
            "harness",
            &[
                "help", "status", "start", "skills", "mcp", "login", "logout", "provider",
            ],
        ),
        (
            "setup",
            &[
                "approvals",
                "telegram",
                "communication",
                "peers",
                "requests",
                "a2a",
            ],
        ),
    ];
    let mut md = String::from("**theuth** — commands\n");
    for (group, names) in GROUPS {
        md.push_str(&format!("\n<details>\n<summary>{group}</summary>\n\n"));
        for (n, a, d) in COMMANDS.iter().filter(|(n, _, _)| names.contains(n)) {
            let arg = if a.is_empty() {
                String::new()
            } else {
                format!(" {a}")
            };
            md.push_str(&format!("- `/{n}{arg}` — {d}\n"));
        }
        md.push_str("</details>\n");
    }
    Reply {
        markdown: md,
        keyboard: Some(keyboard(vec![
            cb("Sessions", "cmd:sessions"),
            cb("Agents", "cmd:agents"),
            cb("Models", "cmd:models"),
            cb("Status", "cmd:status"),
        ])),
    }
}

/// [`commands::NO_SESSION`] with a button to the sessions panel.
#[must_use]
pub fn no_session() -> Reply {
    Reply {
        markdown: commands::NO_SESSION.to_owned(),
        keyboard: Some(keyboard(vec![cb("Sessions", "cmd:sessions")])),
    }
}

/// `/status`: polling · bot · topics · threads · session · model · turn ·
/// context as a table, call counts collapsed; `session` is
/// `[short id, model, turn, context]` when a thread is bound.
#[must_use]
pub fn status(s: &TelegramStatus, session: Option<[String; 4]>) -> Reply {
    let yes_no = |b: bool| if b { "yes" } else { "no" }.to_owned();
    let bot = s.bot.as_ref();
    let [sid, model, turn, context] = session.unwrap_or_else(|| {
        [
            "none".to_owned(),
            "-".to_owned(),
            "-".to_owned(),
            "-".to_owned(),
        ]
    });
    let rows = [
        ("polling", yes_no(s.polling)),
        (
            "bot",
            bot.map_or_else(|| "-".to_owned(), |b| format!("@{}", b.username)),
        ),
        (
            "topics",
            bot.map_or_else(|| "-".to_owned(), |b| yes_no(b.topics)),
        ),
        ("threads", s.threads.len().to_string()),
        ("session", sid),
        ("model", model),
        ("turn", turn),
        ("context", context),
    ];
    let mut md = table(
        std::iter::once(vec!["status".to_owned(), String::new()])
            .chain(rows.into_iter().map(|(k, v)| vec![k.to_owned(), v])),
    );
    if !s.calls.is_empty() {
        md.push_str("\n\n<details>\n<summary>calls</summary>\n\n");
        md.push_str(&table(
            std::iter::once(
                ["method", "ok", "err", "rate limited"]
                    .map(str::to_owned)
                    .to_vec(),
            )
            .chain(s.calls.iter().map(|(m, c)| {
                vec![
                    m.clone(),
                    c.ok.to_string(),
                    c.err.to_string(),
                    c.rate_limited.to_string(),
                ]
            })),
        ));
        md.push_str("\n</details>");
    }
    Reply {
        markdown: md,
        keyboard: Some(keyboard(vec![cb("Refresh", "cmd:refresh")])),
    }
}

/// `/sessions`: page `page` of the top-level sessions as last-8 · title ·
/// model · age, a `Resume <last-8>` button each and `More…`.
#[must_use]
pub fn sessions(rows: &[SessionSummary], page: usize) -> Reply {
    let (slice, more) = paginate(rows, page, SESSIONS_PER_PAGE);
    if rows.is_empty() {
        return Reply::text("no sessions yet: send a message to start one");
    }
    let md = table(
        std::iter::once(["id", "title", "model", "age"].map(str::to_owned).to_vec()).chain(
            slice.iter().map(|s| {
                vec![
                    format!("`{}`", short(s.id)),
                    clip(&s.title, 40),
                    s.model.clone(),
                    age(s.updated_at),
                ]
            }),
        ),
    );
    let mut buttons: Vec<Button> = slice
        .iter()
        .map(|s| {
            let id = short(s.id);
            cb(&format!("Resume {id}"), &format!("cmd:resume:{id}"))
        })
        .collect();
    push_more(&mut buttons, "sessions", more);
    Reply {
        markdown: md,
        keyboard: Some(keyboard(buttons)),
    }
}

/// `/agents`: glyph · name · state · detail · age per row; `Cancel <name>`
/// while running, `Close topic` for a finished child bound to a topic
/// (`threads` is the status snapshot's `key → session` list).
#[must_use]
pub fn agents(rows: &[AgentInfo], threads: &[(String, SessionId)]) -> Reply {
    if rows.is_empty() {
        return Reply::text("no subagents");
    }
    let has_topic = |id: SessionId| threads.iter().any(|(k, s)| *s == id && k.contains(':'));
    let md = table(
        std::iter::once(
            ["", "name", "state", "last", "age"]
                .map(str::to_owned)
                .to_vec(),
        )
        .chain(rows.iter().map(|a| {
            let since = if a.state == AgentState::Running {
                a.started_at
            } else {
                a.finished_at
            };
            vec![
                glyph(&a.state).to_owned(),
                commands::path(a, rows),
                a.state.word().to_owned(),
                clip(&a.last, 40),
                since.map_or_else(String::new, age),
            ]
        })),
    );
    let buttons: Vec<Button> = rows
        .iter()
        .filter_map(|a| {
            let id = short(a.id);
            if a.state == AgentState::Running {
                Some(cb(
                    &format!("Cancel {}", a.name),
                    &format!("cmd:cancel:{id}"),
                ))
            } else if has_topic(a.id) {
                Some(cb(
                    &format!("Close topic {}", a.name),
                    &format!("cmd:close:{id}"),
                ))
            } else {
                None
            }
        })
        .collect();
    Reply {
        markdown: md,
        keyboard: (!buttons.is_empty()).then(|| keyboard(buttons)),
    }
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

/// `/model`: `current: …` and one button per model, grouped by provider in
/// rows of ≤ 8, [`MODELS_PER_PAGE`] per page.
#[must_use]
pub fn model(models: &[ModelInfo], current: &str, page: usize) -> Reply {
    let (slice, more) = paginate(models, page, MODELS_PER_PAGE);
    let mut groups: BTreeMap<&str, Vec<Button>> = BTreeMap::new();
    for m in slice {
        // The id, cut to what fits; `/model <prefix>` completes it.
        let data = format!("cmd:model:{}", m.id);
        let data = cut(&data, DATA_MAX);
        let label = if m.id == current {
            format!("• {}", m.name)
        } else {
            m.name.clone()
        };
        groups
            .entry(m.provider.as_str())
            .or_default()
            .push(cb(&label, data));
    }
    let mut md = format!(
        "current: {}\n\n",
        if current.is_empty() {
            "(default)"
        } else {
            current
        }
    );
    let mut rows: Vec<Vec<Button>> = Vec::new();
    for (provider, buttons) in groups {
        md.push_str(&format!("**{provider}**: {}\n", buttons.len()));
        rows.extend(buttons.chunks(ROW_MAX).map(<[Button]>::to_vec));
    }
    if let Some(n) = more {
        rows.push(vec![cb("More…", &format!("cmd:page:models:{n}"))]);
    }
    Reply {
        markdown: md,
        keyboard: Some(fit(rows)),
    }
}

/// `/skills`: one line per entry, buttons Sync · Add… · More….
#[must_use]
pub fn skills(rows: &[SkillMeta], page: usize) -> Reply {
    let (slice, more) = paginate(rows, page, PER_PAGE);
    let mut md = String::from("**skills**\n");
    if rows.is_empty() {
        md.push_str("(none)\n");
    }
    for s in slice {
        md.push_str(&format!(
            "- `{}`{}{} — {}\n",
            s.name,
            if s.always { " · always" } else { "" },
            s.repo
                .as_deref()
                .map_or(String::new(), |r| format!(" · {r}")),
            clip(&s.description, 60)
        ));
    }
    let mut buttons = vec![cb("Sync", "cmd:skills:sync"), cb("Add…", "cmd:skills:add")];
    push_more(&mut buttons, "skills", more);
    Reply {
        markdown: md,
        keyboard: Some(keyboard(buttons)),
    }
}

/// `/mcp`: one line per server, buttons Sync · Add… · More….
#[must_use]
pub fn mcp(rows: &[McpServer], page: usize) -> Reply {
    let (slice, more) = paginate(rows, page, PER_PAGE);
    let mut md = String::from("**mcp**\n");
    if rows.is_empty() {
        md.push_str("(none)\n");
    }
    for s in slice {
        md.push_str(&format!(
            "- `{}` · {} · {} tools{}\n",
            s.name,
            if s.connected { "connected" } else { "off" },
            s.tools.len(),
            match s.auth {
                McpAuth::None => "",
                McpAuth::Required => " · login required",
                McpAuth::LoggedIn => " · logged in",
            }
        ));
    }
    let mut buttons = vec![cb("Sync", "cmd:mcp:sync"), cb("Add…", "cmd:mcp:add")];
    push_more(&mut buttons, "mcp", more);
    Reply {
        markdown: md,
        keyboard: Some(keyboard(buttons)),
    }
}

/// `/login` / `/logout`: one row per OAuth provider and per MCP server that
/// uses OAuth, with its state and a Log in / Log out button.
#[must_use]
pub fn logins(providers: &[LoginInfo], servers: &[McpServer]) -> Reply {
    let mut rows: Vec<(&str, bool, String)> = providers
        .iter()
        .map(|l| {
            let state = match (l.logged_in, &l.account) {
                (false, _) => "not logged in".to_owned(),
                (true, Some(a)) => format!("logged in ({a})"),
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
        return Reply::text("no logins");
    }
    let md = table(
        std::iter::once(vec!["name".to_owned(), "state".to_owned()]).chain(
            rows.iter()
                .map(|(n, _, s)| vec![(*n).to_owned(), s.clone()]),
        ),
    );
    let buttons = rows
        .iter()
        .map(|(name, logged_in, _)| {
            if *logged_in {
                cb(&format!("Log out {name}"), &format!("cmd:logout:{name}"))
            } else {
                cb(&format!("Log in {name}"), &format!("cmd:login:{name}"))
            }
        })
        .collect();
    Reply {
        markdown: md,
        keyboard: Some(keyboard(buttons)),
    }
}

/// `/login <name>`: the URL to open as a link button.
#[must_use]
pub fn login_url(name: &str, url: &str) -> Reply {
    Reply {
        markdown: format!("log in to **{name}**: {url}"),
        keyboard: Some(Keyboard {
            rows: vec![vec![Button::Url {
                text: "Open".to_owned(),
                url: url.to_owned(),
            }]],
            force_reply: false,
        }),
    }
}

/// `/approvals`: three buttons ask · off this session · off always.
#[must_use]
pub fn approvals() -> Reply {
    Reply {
        markdown:
            "approvals: ask before risky tool calls, or switch them off for this session or always"
                .to_owned(),
        keyboard: Some(keyboard(vec![
            cb("ask", "cmd:approvals:ask"),
            cb("off this session", "cmd:approvals:off_session"),
            cb("off always", "cmd:approvals:off_always"),
        ])),
    }
}

/// `items[page]` at `per` per page and the next page when there is one.
fn paginate<T>(items: &[T], page: usize, per: usize) -> (&[T], Option<usize>) {
    let start = (page * per).min(items.len());
    let end = (start + per).min(items.len());
    (&items[start..end], (end < items.len()).then_some(page + 1))
}

fn push_more(buttons: &mut Vec<Button>, panel: &str, more: Option<usize>) {
    if let Some(n) = more {
        buttons.push(cb("More…", &format!("cmd:page:{panel}:{n}")));
    }
}

/// A callback button; `data` is cut to [`DATA_MAX`] bytes.
fn cb(text: &str, data: &str) -> Button {
    Button::Callback {
        text: text.to_owned(),
        data: cut(data, DATA_MAX).to_owned(),
    }
}

/// `buttons` in rows of [`ROW_MAX`], at most [`BUTTONS_MAX`] in all.
fn keyboard(buttons: Vec<Button>) -> Keyboard {
    fit(buttons.chunks(ROW_MAX).map(<[Button]>::to_vec).collect())
}

/// `rows` cut to [`BUTTONS_MAX`] buttons.
fn fit(rows: Vec<Vec<Button>>) -> Keyboard {
    let mut left = BUTTONS_MAX;
    let rows = rows
        .into_iter()
        .filter_map(|mut r| {
            r.truncate(left);
            left -= r.len();
            (!r.is_empty()).then_some(r)
        })
        .collect();
    Keyboard {
        rows,
        force_reply: false,
    }
}

/// `s` cut at a char boundary to at most `max` bytes.
fn cut(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// `s` cut to `max` chars with a `…` when it was longer, one line.
fn clip(s: &str, max: usize) -> String {
    let s = s.lines().next().unwrap_or("");
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// `5s`, `12m`, `3h`, `2d` since `t`.
#[must_use]
pub fn age(t: DateTime<Utc>) -> String {
    let s = (Utc::now() - t).num_seconds().max(0);
    match s {
        ..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86400 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86400),
    }
}

/// `12.3k`, `200k`, `1.0M`; `?` for 0.
#[must_use]
pub fn k(n: u32) -> String {
    match n {
        0 => "?".to_owned(),
        1..1000 => n.to_string(),
        1000..1_000_000 => format!("{:.1}k", f64::from(n) / 1000.0),
        _ => format!("{:.1}M", f64::from(n) / 1_000_000.0),
    }
}

#[cfg(test)]
mod tests {
    use agent::AgentState;

    use super::*;
    use crate::commands::testing::{FakePort, cx, summary, text};

    fn buttons(r: &Reply) -> Vec<&Button> {
        r.keyboard
            .iter()
            .flat_map(|k| k.rows.iter().flatten())
            .collect()
    }

    fn data(r: &Reply) -> Vec<&str> {
        buttons(r)
            .into_iter()
            .filter_map(|b| match b {
                Button::Callback { data, .. } => Some(data.as_str()),
                _ => None,
            })
            .collect()
    }

    /// 98 §10.3 rules for one keyboard.
    fn check(r: &Reply) {
        let Some(k) = &r.keyboard else { return };
        assert!(k.rows.iter().all(|row| row.len() <= ROW_MAX), "{k:?}");
        assert!(buttons(r).len() <= BUTTONS_MAX);
        for d in data(r) {
            assert!(d.len() <= DATA_MAX, "{d}");
            assert!(d.starts_with("cmd:"), "{d}");
        }
    }

    fn model_info(id: &str, provider: &str) -> ModelInfo {
        ModelInfo {
            id: id.to_owned(),
            name: id.rsplit('/').next().unwrap().to_owned(),
            provider: provider.to_owned(),
            context: 0,
            reasoning: false,
            image: false,
            efforts: vec![],
            budget_min: None,
            budget_max: None,
            local: false,
        }
    }

    #[test]
    fn pipe_table_shape() {
        let t = table([
            vec!["a".to_owned(), "b".to_owned()],
            vec!["1|x".to_owned(), "2".to_owned()],
        ]);
        assert_eq!(t, "| a | b |\n| --- | --- |\n| 1\\|x | 2 |");
        assert_eq!(table(Vec::<Vec<String>>::new()), "");
    }

    #[test]
    fn help_lists_every_command_once_in_details() {
        let h = help();
        for (name, _, _) in COMMANDS {
            let n = h.markdown.matches(&format!("`/{name} ")).count()
                + h.markdown.matches(&format!("`/{name}`")).count();
            assert_eq!(n, 1, "{name}");
        }
        assert_eq!(h.markdown.matches("<details>").count(), 3);
        assert_eq!(h.markdown.matches("</details>").count(), 3);
        assert_eq!(
            data(&h),
            ["cmd:sessions", "cmd:agents", "cmd:models", "cmd:status"]
        );
        check(&h);
    }

    #[test]
    fn panels_keep_the_callback_rules() {
        let rows: Vec<SessionSummary> = (0..25)
            .map(|i| summary(SessionId::new(), &format!("s{i}")))
            .collect();
        let p0 = sessions(&rows, 0);
        check(&p0);
        assert_eq!(buttons(&p0).len(), SESSIONS_PER_PAGE + 1);
        assert!(p0.markdown.contains(&format!("`{}`", short(rows[0].id))));
        assert!(!p0.markdown.contains(&short(rows[10].id)));
        assert_eq!(data(&p0)[0], format!("cmd:resume:{}", short(rows[0].id)));
        assert_eq!(*data(&p0).last().unwrap(), "cmd:page:sessions:1");
        let p2 = sessions(&rows, 2);
        assert_eq!(buttons(&p2).len(), 5);
        assert!(!data(&p2).iter().any(|d| d.starts_with("cmd:page")));
        assert_eq!(sessions(&[], 0).keyboard, None);

        let models: Vec<ModelInfo> = (0..30)
            .map(|i| model_info(&format!("p{}/model-{i}", i % 3), &format!("p{}", i % 3)))
            .chain([model_info(
                &format!("openrouter/{}", "x".repeat(80)),
                "openrouter",
            )])
            .collect();
        let m = model(&models, "p0/model-0", 0);
        check(&m);
        assert_eq!(buttons(&m).len(), MODELS_PER_PAGE + 1);
        assert!(m.markdown.starts_with("current: p0/model-0"));
        assert!(matches!(buttons(&m)[0], Button::Callback { text, .. } if text == "• model-0"));
        let m1 = model(&models, "", 1);
        check(&m1);
        assert_eq!(buttons(&m1).len(), 7);
        assert!(data(&m1).iter().any(|d| d.len() == DATA_MAX));

        let st = status(&crate::commands::testing::status(), None);
        check(&st);
        assert!(st.markdown.contains("| session | none |"));
        assert_eq!(data(&st), ["cmd:refresh"]);
        let mut s = crate::commands::testing::status();
        s.calls.insert("getMe".into(), crate::CallStats::default());
        assert!(
            status(&s, None)
                .markdown
                .contains("<summary>calls</summary>")
        );

        check(&approvals());
        assert_eq!(data(&approvals()).len(), 3);
        check(&no_session());
        check(&skills(&[], 0));
        assert_eq!(data(&skills(&[], 0)), ["cmd:skills:sync", "cmd:skills:add"]);
        check(&mcp(&[], 0));
        let lu = login_url("openai", "https://x/y");
        assert!(matches!(&buttons(&lu)[0], Button::Url { url, .. } if url == "https://x/y"));
    }

    #[test]
    fn agents_panel_cancels_running_and_closes_finished_topics() {
        let root = SessionId::new();
        let mk = |name: &str, state: AgentState| AgentInfo {
            id: SessionId::new(),
            name: name.into(),
            parent: root,
            depth: 1,
            agent: None,
            model: "m".into(),
            state,
            started_at: Some(Utc::now()),
            finished_at: Some(Utc::now()),
            last: "looking\nmore".into(),
            project: None,
            retention: tools::Retention::Durable,
            activity: Default::default(),
            cap: None,
            kind: Default::default(),
            observed: None,
        };
        let rows = vec![
            mk("grep", AgentState::Running),
            mk("done", AgentState::Idle),
            mk("flat", AgentState::Idle),
        ];
        let threads = vec![
            ("7:12".to_owned(), rows[1].id),
            ("7".to_owned(), rows[2].id),
        ];
        let p = agents(&rows, &threads);
        check(&p);
        assert_eq!(
            data(&p),
            [
                format!("cmd:cancel:{}", short(rows[0].id)),
                format!("cmd:close:{}", short(rows[1].id))
            ]
        );
        assert!(p.markdown.contains("| ⠹ | grep | running | looking |"));
        assert_eq!(agents(&[], &[]).keyboard, None);
    }

    #[tokio::test]
    async fn actions_map_to_commands_and_pages() {
        let rows: Vec<SessionSummary> = (0..12)
            .map(|i| summary(SessionId::new(), &format!("s{i}")))
            .collect();
        let port = FakePort {
            sessions: rows.clone(),
            ..Default::default()
        };
        let Some(Outcome::Reply(r)) = action(cx(&port, 2, None), "page:sessions:1").await else {
            panic!()
        };
        assert_eq!(r, sessions(&rows, 1));
        assert_eq!(buttons(&r).len(), 2);
        let Some(Outcome::Resume { session, .. }) =
            action(cx(&port, 2, None), &format!("resume:{}", short(rows[3].id))).await
        else {
            panic!()
        };
        assert_eq!(session, rows[3].id);
        assert_eq!(
            action(cx(&port, 2, None), &format!("close:{}", short(rows[4].id))).await,
            Some(Outcome::CloseTopic(rows[4].id))
        );
        assert_eq!(
            action(cx(&port, 2, None), "approvals:off_always").await,
            Some(text(commands::OWNER_MSG))
        );
        assert_eq!(
            action(cx(&port, 2, None), "skills:add").await,
            Some(commands::prompt(Expect::SkillsAdd))
        );
        assert_eq!(action(cx(&port, 2, None), "nope").await, None);
        assert_eq!(action(cx(&port, 2, None), "page:sessions:x").await, None);
        let Some(Outcome::Reply(r)) = action(cx(&port, 2, None), "refresh").await else {
            panic!()
        };
        assert_eq!(r, status(&crate::commands::testing::status(), None));
    }

    #[test]
    fn guard_escapes_only_outside_code_and_known_tags() {
        assert_eq!(guard("a Vec<String> b"), "a Vec&lt;String&gt; b");
        assert_eq!(guard("x && y"), "x &amp;&amp; y");
        assert_eq!(guard("`Vec<T>` and <T>"), "`Vec<T>` and &lt;T&gt;");
        assert_eq!(guard("``a ` <b` `` <c>"), "``a ` <b` `` &lt;c&gt;");
        let fence = "before <x>\n```rust\nlet v: Vec<u8> = a & b;\n```\nafter <y>";
        assert_eq!(
            guard(fence),
            "before &lt;x&gt;\n```rust\nlet v: Vec<u8> = a & b;\n```\nafter &lt;y&gt;"
        );
        let tilde = "~~~\n<raw>\n~~~\n<raw>";
        assert_eq!(guard(tilde), "~~~\n<raw>\n~~~\n&lt;raw&gt;");
        assert_eq!(
            guard("<details>\n<summary>x</summary>\n\n<T> & &amp;\n</details>"),
            "<details>\n<summary>x</summary>\n\n&lt;T&gt; &amp; &amp;\n</details>"
        );
        assert_eq!(
            guard("<blockquote expandable>q</blockquote> <tg-button text=\"a\"/> <b>x</b>"),
            "<blockquote expandable>q</blockquote> <tg-button text=\"a\"/> <b>x</b>"
        );
        assert_eq!(guard("<detailsx> <div>"), "&lt;detailsx&gt; &lt;div&gt;");
        assert_eq!(guard("&#60; &#x3c; &lt; &x"), "&#60; &#x3c; &lt; &amp;x");
        assert_eq!(
            guard("> quote <q> a > b\n>> <a href=\"u\">t</a> -> x"),
            "> quote &lt;q&gt; a &gt; b\n>> <a href=\"u\">t</a> -&gt; x"
        );
        // An unclosed backtick is literal; escaping continues after it.
        assert_eq!(guard("a ` <q>"), "a ` &lt;q&gt;");
        // Idempotent.
        let once = guard(fence);
        assert_eq!(guard(&once), once);
        assert_eq!(guard(""), "");
        assert_eq!(help().markdown, guard(&help().markdown));
    }

    #[test]
    fn helpers() {
        assert_eq!(k(0), "?");
        assert_eq!(k(999), "999");
        assert_eq!(k(12_300), "12.3k");
        assert_eq!(k(1_000_000), "1.0M");
        assert_eq!(clip("a\nb", 5), "a");
        assert_eq!(clip("abcdef", 3), "ab…");
        assert_eq!(cut("héllo", 2), "h");
        assert_eq!(age(Utc::now()), "0s");
    }
}
