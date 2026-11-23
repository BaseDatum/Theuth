//! Approval cards and button clicks (design doc 21 §10.1). Pure: Block Kit JSON
//! in, parsed clicks out; the channel does the posting and resolving.

use chat_common::truncate;
use double::{DoubleAnswered, DoublePredicted};
use slack_morphism::prelude::{SlackInteractionActionContainer, SlackInteractionEvent};
use tools::{ApprovalId, ApprovalRequested, Decision};

/// `action_id` of the Allow button.
pub const ACTION_APPROVE: &str = "approve";
/// `action_id` of the Deny button.
pub const ACTION_DENY: &str = "deny";
/// Longest formatted argument block in [`body`].
const MAX_ARGS_CHARS: usize = 600;

/// Block Kit card for an [`ApprovalRequested`]: `[markdown, actions(Allow/Deny)]`.
/// `context`: `Some("session 1a2b3c4d · <title>")` adds a trailing context block
/// (owner-DM cards, where the thread does not identify the session).
pub fn card(req: &ApprovalRequested, context: Option<&str>) -> serde_json::Value {
    blocks(&body(req), req.id, None, context)
}

/// [`card`] rebuilt from the `body` it was posted with, plus the double's context
/// line (126 §10): `[markdown, actions, context(double)[, context(session)]]`.
pub fn predicted_card(
    body: &str,
    id: ApprovalId,
    p: &DoublePredicted,
    context: Option<&str>,
) -> serde_json::Value {
    blocks(body, id, Some(p), context)
}

fn blocks(
    body: &str,
    id: ApprovalId,
    double: Option<&DoublePredicted>,
    context: Option<&str>,
) -> serde_json::Value {
    let id = id.0.to_string();
    let mut blocks = vec![
        markdown(body),
        serde_json::json!({
            "type": "actions",
            "elements": [
                button(ACTION_APPROVE, "Allow", "primary", &id),
                button(ACTION_DENY, "Deny", "danger", &id),
            ],
        }),
    ];
    if let Some(p) = double {
        blocks.push(double_context(p));
    }
    if let Some(text) = context {
        blocks.push(serde_json::json!({
            "type": "context",
            "elements": [{ "type": "mrkdwn", "text": text }],
        }));
    }
    serde_json::Value::Array(blocks)
}

/// The double's line as a `context` block: `double would: Allow (91 %)`, with the
/// grace countdown at L2 (126 §10). Shared with [`crate::asks`].
pub fn double_context(p: &DoublePredicted) -> serde_json::Value {
    let now = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0);
    serde_json::json!({
        "type": "context",
        "elements": [{ "type": "mrkdwn", "text": double::chip::would(p, now) }],
    })
}

/// A freshly built card with the double's context line appended (the ask cards,
/// whose builder has the request at hand).
pub fn with_double(mut blocks: serde_json::Value, p: &DoublePredicted) -> serde_json::Value {
    if let Some(arr) = blocks.as_array_mut() {
        arr.push(double_context(p));
    }
    blocks
}

/// The line when the double answered at L2 (126 §10): `🤖 double answered Allow (…)`.
pub fn answered_text(a: &DoubleAnswered) -> String {
    format!("🤖 {}", double::chip::answered(a))
}

/// [`answered_text`] as one markdown block (the owner DM).
pub fn answered_blocks(a: &DoubleAnswered) -> serde_json::Value {
    serde_json::Value::Array(vec![markdown(&answered_text(a))])
}

/// Card after resolution: one markdown block, no buttons. `body` is the text
/// [`card`] used; its first line is replaced by the outcome and the fenced args
/// are kept. `by`: `Some(user id)` → "allowed by <@U…>". `dismissed` (207):
/// left to the agent from Attention — neither allowed nor denied.
pub fn resolved_card(
    body: &str,
    decision: Decision,
    timed_out: bool,
    dismissed: bool,
    by: Option<&str>,
) -> serde_json::Value {
    let (head, rest) = body.split_once('\n').unwrap_or((body, ""));
    // `⚠️ **tool** wants to run` → the bold tool name.
    let tool = head.split("**").nth(1).unwrap_or(head);
    let mut line = match (decision, timed_out) {
        _ if dismissed => format!("**{tool}** left to the agent (not run, not refused)"),
        (_, true) => format!("⌛ **{tool}** timed out (denied)"),
        (Decision::Allow, false) => format!("✅ **{tool}** allowed"),
        (Decision::Deny, false) => format!("❌ **{tool}** denied"),
    };
    if let Some(user) = by.filter(|_| !timed_out && !dismissed) {
        line.push_str(&format!(" by <@{user}>"));
    }
    if !rest.is_empty() {
        line.push('\n');
        line.push_str(rest);
    }
    serde_json::Value::Array(vec![markdown(&line)])
}

/// Text shared by [`card`] and [`resolved_card`]: line 1 `⚠️ **<tool>** wants to run`,
/// then `shell` → the command in a ```sh fence; other tools → pretty JSON args
/// (≤ [`MAX_ARGS_CHARS`], `…` when cut) in a ```json fence.
pub fn body(req: &ApprovalRequested) -> String {
    let shell_cmd = (req.tool == "shell")
        .then(|| req.args.get("command").and_then(|v| v.as_str()))
        .flatten();
    let (lang, args) = match shell_cmd {
        Some(cmd) => ("sh", cmd.to_owned()),
        None => (
            "json",
            truncate(
                &serde_json::to_string_pretty(&req.args).unwrap_or_default(),
                MAX_ARGS_CHARS,
            ),
        ),
    };
    let owner_only = if req.human_only {
        // 210 §7.2: the thread user's click is refused; say so on the card.
        "\n_Only the owner can answer this._"
    } else {
        ""
    };
    format!(
        "⚠️ **{}** wants to run\n```{lang}\n{args}\n```{owner_only}",
        req.tool
    )
}

/// A parsed Allow/Deny button click.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Click {
    /// The approval the button belongs to (the button `value`).
    pub id: ApprovalId,
    /// Allow for [`ACTION_APPROVE`], Deny for [`ACTION_DENY`].
    pub decision: Decision,
    /// Who clicked.
    pub user: String,
    /// Channel of the card message.
    pub channel: String,
    /// `ts` of the card message (for `chat.update`).
    pub message_ts: String,
    /// Slack's one-shot reply URL, when present.
    pub response_url: Option<String>,
}

/// The first Allow/Deny action of a `block_actions` payload; `None` for any other
/// interaction, a foreign `action_id`, a non-uuid value, or a click outside a message.
pub fn click(ev: &SlackInteractionEvent) -> Option<Click> {
    let SlackInteractionEvent::BlockActions(ev) = ev else {
        return None;
    };
    let (decision, id) = ev.actions.as_deref()?.iter().find_map(|a| {
        let decision = match a.action_id.0.as_str() {
            ACTION_APPROVE => Decision::Allow,
            ACTION_DENY => Decision::Deny,
            _ => return None,
        };
        let id = a.value.as_deref()?.parse().ok()?;
        Some((decision, ApprovalId(id)))
    })?;
    let (message_ts, container_channel) = match &ev.container {
        SlackInteractionActionContainer::Message(m) => (&m.message_ts, m.channel_id.as_ref()),
        SlackInteractionActionContainer::MessageAttachment(m) => {
            (&m.message_ts, m.channel_id.as_ref())
        }
        SlackInteractionActionContainer::View(_) => return None,
    };
    let channel = ev.channel.as_ref().map(|c| &c.id).or(container_channel)?;
    Some(Click {
        id,
        decision,
        user: ev.user.as_ref()?.id.0.clone(),
        channel: channel.0.clone(),
        message_ts: message_ts.0.clone(),
        response_url: ev.response_url.as_ref().map(|u| u.0.to_string()),
    })
}

fn markdown(text: &str) -> serde_json::Value {
    serde_json::json!({ "type": "markdown", "text": text })
}

fn button(action_id: &str, label: &str, style: &str, value: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "button",
        "action_id": action_id,
        "text": { "type": "plain_text", "text": label },
        "style": style,
        "value": value,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::SessionId;
    use serde_json::json;

    fn req(tool: &str, args: serde_json::Value) -> ApprovalRequested {
        ApprovalRequested {
            id: ApprovalId(uuid::Uuid::from_u128(7)),
            session: SessionId::new(),
            tool: tool.into(),
            args,
            risk: tools::Risk::Destructive,
            human_only: false,
        }
    }

    fn payload(action_id: &str, value: &str) -> serde_json::Value {
        json!({
            "type": "block_actions",
            "team": { "id": "T1" },
            "user": { "id": "U42" },
            "api_app_id": "A1",
            "container": { "type": "message", "message_ts": "100.1", "channel_id": "C9" },
            "trigger_id": "tr1",
            "channel": { "id": "C9", "name": "general" },
            "response_url": "https://hooks.slack.com/actions/T1/1/x",
            "actions": [{ "type": "button", "action_id": action_id, "value": value }],
        })
    }

    #[test]
    fn card_blocks_and_buttons() {
        let r = req("shell", json!({ "command": "rm -rf /tmp/x" }));
        let blocks = card(&r, None);
        let blocks = blocks.as_array().unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["type"], "markdown");
        assert_eq!(blocks[1]["type"], "actions");
        let buttons = blocks[1]["elements"].as_array().unwrap();
        assert_eq!(buttons.len(), 2);
        let uuid = r.id.0.to_string();
        for b in buttons {
            assert_eq!(b["value"], uuid);
        }
        assert_eq!(buttons[0]["action_id"], ACTION_APPROVE);
        assert_eq!(buttons[0]["style"], "primary");
        assert_eq!(buttons[1]["action_id"], ACTION_DENY);
        assert_eq!(buttons[1]["style"], "danger");

        let with = card(&r, Some("session 1a2b3c4d · fix tests"));
        let with = with.as_array().unwrap();
        assert_eq!(with.len(), 3);
        assert_eq!(with[2]["type"], "context");
        assert_eq!(
            with[2]["elements"][0]["text"],
            "session 1a2b3c4d · fix tests"
        );
    }

    #[test]
    fn double_line_is_one_context_block_before_the_session_context() {
        let r = req("shell", json!({ "command": "git push" }));
        let p = DoublePredicted {
            question_id: r.id.0.to_string(),
            session: Some(r.session),
            kind: double::CardKind::Approval,
            class: "approval:shell:mutating:git push:root".into(),
            choice: "Allow".into(),
            index: None,
            p: 0.912,
            probs: vec![],
            rung: 1,
            will_act_at: None,
            version: "baseline".into(),
            explain: Default::default(),
        };
        let b = predicted_card(&body(&r), r.id, &p, Some("session 1a2b3c4d · fix tests"));
        let b = b.as_array().unwrap();
        assert_eq!(b.len(), 4);
        // The first two blocks are the live card's, so clicks keep working.
        assert_eq!(&b[..2], &card(&r, None).as_array().unwrap()[..]);
        assert_eq!(b[2]["type"], "context");
        assert_eq!(b[2]["elements"][0]["type"], "mrkdwn");
        assert_eq!(b[2]["elements"][0]["text"], "double would: Allow (91 %)");
        assert_eq!(b[3]["elements"][0]["text"], "session 1a2b3c4d · fix tests");
        // Appended to any card; the thread line is one markdown block.
        let asked = with_double(crate::asks::question_card(&ask_req(), 0, None), &p);
        let asked = asked.as_array().unwrap();
        assert_eq!(asked.len(), 4);
        assert_eq!(asked[3], b[2]);
        let line = answered_blocks(&DoubleAnswered {
            question_id: p.question_id.clone(),
            session: p.session,
            class: p.class.clone(),
            choice: "Allow".into(),
            version: "baseline".into(),
            undo_until: None,
        });
        assert_eq!(
            line,
            json!([{ "type": "markdown", "text": "🤖 double answered Allow (approval:shell:mutating:git push:root)" }])
        );
    }

    fn ask_req() -> tools::AskRequested {
        tools::AskRequested {
            id: tools::AskId(uuid::Uuid::from_u128(8)),
            session: SessionId::new(),
            questions: vec![tools::Question {
                id: "which".into(),
                question: "Which?".into(),
                options: vec![],
                recommended: None,
            }],
        }
    }

    #[test]
    fn body_shell_and_truncated_json() {
        let sh = body(&req("shell", json!({ "command": "cargo test -p slack" })));
        assert_eq!(
            sh,
            "⚠️ **shell** wants to run\n```sh\ncargo test -p slack\n```"
        );

        let long = "é".repeat(700);
        let js = body(&req("write", json!({ "content": long })));
        let pretty = serde_json::to_string_pretty(&json!({ "content": long })).unwrap();
        let expected: String = pretty.chars().take(600).collect();
        assert_eq!(
            js,
            format!("⚠️ **write** wants to run\n```json\n{expected}…\n```")
        );
        assert_eq!(js.lines().nth(1), Some("```json"));
    }

    #[test]
    fn resolved_card_drops_buttons_and_names_user() {
        let b = body(&req("shell", json!({ "command": "ls" })));
        let allowed = resolved_card(&b, Decision::Allow, false, false, Some("U42"));
        let blocks = allowed.as_array().unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0]["type"], "markdown");
        assert_eq!(
            blocks[0]["text"],
            "✅ **shell** allowed by <@U42>\n```sh\nls\n```"
        );
        let denied = resolved_card(&b, Decision::Deny, false, false, None);
        assert!(
            denied[0]["text"]
                .as_str()
                .unwrap()
                .starts_with("❌ **shell** denied\n")
        );
        let timed = resolved_card(&b, Decision::Deny, true, false, Some("U42"));
        assert!(
            timed[0]["text"]
                .as_str()
                .unwrap()
                .starts_with("⌛ **shell** timed out (denied)\n")
        );
        // 207 E4: left to the agent is not a denial and names nobody.
        let left = resolved_card(&b, Decision::Deny, false, true, Some("U42"));
        assert_eq!(
            left[0]["text"],
            "**shell** left to the agent (not run, not refused)\n```sh\nls\n```"
        );
    }

    #[test]
    fn click_parses_allow_and_ignores_foreign() {
        let id = uuid::Uuid::from_u128(9);
        let ev: SlackInteractionEvent =
            serde_json::from_value(payload(ACTION_APPROVE, &id.to_string())).unwrap();
        assert_eq!(
            click(&ev),
            Some(Click {
                id: ApprovalId(id),
                decision: Decision::Allow,
                user: "U42".into(),
                channel: "C9".into(),
                message_ts: "100.1".into(),
                response_url: Some("https://hooks.slack.com/actions/T1/1/x".into()),
            })
        );
        let deny: SlackInteractionEvent =
            serde_json::from_value(payload(ACTION_DENY, &id.to_string())).unwrap();
        assert_eq!(click(&deny).unwrap().decision, Decision::Deny);

        let foreign: SlackInteractionEvent =
            serde_json::from_value(payload("open_settings", &id.to_string())).unwrap();
        assert_eq!(click(&foreign), None);
        let bad: SlackInteractionEvent =
            serde_json::from_value(payload(ACTION_APPROVE, "not-a-uuid")).unwrap();
        assert_eq!(click(&bad), None);
    }
}
