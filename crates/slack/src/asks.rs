//! `ask_user` question cards and their clicks, one Block Kit message per
//! question (same shape as [`crate::approvals`]). Pure: JSON in, parsed
//! clicks out; the channel posts, updates and answers.

use slack_morphism::prelude::{SlackInteractionActionContainer, SlackInteractionEvent};
use tools::{Answer, AskId, AskRequested};

/// `action_id` of an option button; `value` = `<ask id>:<question>:<option>`.
pub const ACTION_PICK: &str = "ask_pick";
/// `action_id` of the Skip button; `value` = `<ask id>:<question>`.
pub const ACTION_SKIP: &str = "ask_skip";
/// `action_id` of the Dismiss button; `value` = `<ask id>`.
pub const ACTION_DISMISS: &str = "ask_dismiss";
/// `action_id` of the freeform input; its `block_id` = `ask_text:<ask id>:<question>`.
pub const ACTION_TEXT: &str = "ask_text";
/// Block Kit allows 25 elements per `actions` block; Skip and Dismiss take two.
const MAX_OPTIONS: usize = 23;

/// Card for question `i` of `req`: `[markdown, actions(options…, Skip, Dismiss),
/// input(plain_text)]` plus a trailing `context` block when `context` is given
/// (owner-DM cards, where the thread does not identify the session).
pub fn question_card(req: &AskRequested, i: usize, context: Option<&str>) -> serde_json::Value {
    let q = &req.questions[i];
    let id = req.id.0;
    let mut text = heading("❓", req, i);
    let mut buttons = Vec::with_capacity(q.options.len().min(MAX_OPTIONS) + 2);
    for (k, o) in q.options.iter().take(MAX_OPTIONS).enumerate() {
        let recommended = q.recommended == Some(k);
        text.push_str(&format!("\n{}. **{}**", k + 1, o.label));
        if let Some(d) = &o.description {
            text.push_str(&format!(" — {d}"));
        }
        if recommended {
            text.push_str(" _(recommended)_");
        }
        buttons.push(button(
            ACTION_PICK,
            &o.label,
            recommended.then_some("primary"),
            &format!("{id}:{i}:{k}"),
        ));
    }
    if q.options.is_empty() {
        text.push_str("\n_(type your answer)_");
    }
    buttons.push(button(ACTION_SKIP, "Skip", None, &format!("{id}:{i}")));
    buttons.push(button(
        ACTION_DISMISS,
        "Dismiss",
        Some("danger"),
        &id.to_string(),
    ));
    let mut blocks = vec![
        markdown(&text),
        serde_json::json!({ "type": "actions", "elements": buttons }),
        serde_json::json!({
            "type": "input",
            "dispatch_action": true,
            "block_id": format!("{ACTION_TEXT}:{id}:{i}"),
            "label": { "type": "plain_text", "text": "Your answer" },
            "element": {
                "type": "plain_text_input",
                "action_id": ACTION_TEXT,
                "placeholder": { "type": "plain_text", "text": "Or type your own answer…" },
            },
        }),
    ];
    if let Some(text) = context {
        blocks.push(serde_json::json!({
            "type": "context",
            "elements": [{ "type": "mrkdwn", "text": text }],
        }));
    }
    serde_json::Value::Array(blocks)
}

/// Question `i` after it was answered here: one markdown block, no controls.
pub fn answered_card(req: &AskRequested, i: usize, answer: &Answer) -> serde_json::Value {
    let picked = match answer {
        Answer::Choice { index } => req.questions[i]
            .options
            .get(*index)
            .map_or_else(|| index.to_string(), |o| o.label.clone()),
        Answer::Text { text } => text.clone(),
        Answer::Skipped => "_skipped_".to_owned(),
    };
    single(format!("{}\n→ {picked}", heading("✅", req, i)))
}

/// Question `i` once the set closed without a click here: dismissed (turn
/// cancelled), timed out, or answered from another frontend.
pub fn closed_card(
    req: &AskRequested,
    i: usize,
    dismissed: bool,
    timed_out: bool,
) -> serde_json::Value {
    single(if dismissed {
        "🚫 **Questions dismissed** (turn cancelled)".to_owned()
    } else if timed_out {
        format!(
            "⌛ **Question {}/{}** — timed out",
            i + 1,
            req.questions.len()
        )
    } else {
        format!("{}\n→ _answered elsewhere_", heading("✅", req, i))
    })
}

/// What a click on a question card asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// An option button: index into `Question::options`.
    Pick(usize),
    /// The freeform input, dispatched on Enter.
    Text(String),
    /// The Skip button.
    Skip,
    /// The Dismiss button: the whole set.
    Dismiss,
}

/// A parsed question-card interaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Click {
    /// The ask the card belongs to.
    pub id: AskId,
    /// The question the card shows; `None` for Dismiss.
    pub question: Option<usize>,
    /// What was clicked or typed.
    pub kind: Kind,
    /// Who clicked.
    pub user: String,
    /// Channel of the card message.
    pub channel: String,
    /// `ts` of the card message (for `chat.update`).
    pub message_ts: String,
    /// Slack's one-shot reply URL, when present.
    pub response_url: Option<String>,
}

/// The first ask action of a `block_actions` payload; `None` for any other
/// interaction, a foreign `action_id`, a malformed value, an empty text, or a
/// click outside a message.
pub fn click(ev: &SlackInteractionEvent) -> Option<Click> {
    let SlackInteractionEvent::BlockActions(ev) = ev else {
        return None;
    };
    let a = ev.actions.as_deref()?.iter().find(|a| {
        matches!(
            a.action_id.0.as_str(),
            ACTION_PICK | ACTION_SKIP | ACTION_DISMISS | ACTION_TEXT
        )
    })?;
    let action = a.action_id.0.as_str();
    let key = if action == ACTION_TEXT {
        a.block_id.as_ref()?.0.strip_prefix("ask_text:")?
    } else {
        a.value.as_deref()?
    };
    let mut parts = key.split(':');
    let id = AskId(parts.next()?.parse().ok()?);
    let question = match action {
        ACTION_DISMISS => None,
        _ => Some(parts.next()?.parse().ok()?),
    };
    let kind = match action {
        ACTION_PICK => Kind::Pick(parts.next()?.parse().ok()?),
        ACTION_SKIP => Kind::Skip,
        ACTION_DISMISS => Kind::Dismiss,
        _ => {
            let text = a.value.as_deref()?.trim();
            if text.is_empty() {
                return None;
            }
            Kind::Text(text.to_owned())
        }
    };
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
        question,
        kind,
        user: ev.user.as_ref()?.id.0.clone(),
        channel: channel.0.clone(),
        message_ts: message_ts.0.clone(),
        response_url: ev.response_url.as_ref().map(|u| u.0.to_string()),
    })
}

/// `<emoji> **Question i/n** — <question>`.
fn heading(emoji: &str, req: &AskRequested, i: usize) -> String {
    format!(
        "{emoji} **Question {}/{}** — {}",
        i + 1,
        req.questions.len(),
        req.questions[i].question
    )
}

fn single(text: String) -> serde_json::Value {
    serde_json::Value::Array(vec![markdown(&text)])
}

fn markdown(text: &str) -> serde_json::Value {
    serde_json::json!({ "type": "markdown", "text": text })
}

fn button(action_id: &str, label: &str, style: Option<&str>, value: &str) -> serde_json::Value {
    let mut b = serde_json::json!({
        "type": "button",
        "action_id": action_id,
        "text": { "type": "plain_text", "text": label },
        "value": value,
    });
    if let Some(style) = style {
        b["style"] = style.into();
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::SessionId;
    use serde_json::json;
    use tools::{Choice, Question};

    fn req() -> AskRequested {
        AskRequested {
            id: AskId(uuid::Uuid::from_u128(7)),
            session: SessionId::new(),
            questions: vec![
                Question {
                    id: "lang".into(),
                    question: "Which language?".into(),
                    options: vec![
                        Choice {
                            label: "Rust".into(),
                            description: Some("fast".into()),
                        },
                        Choice {
                            label: "Go".into(),
                            description: None,
                        },
                    ],
                    recommended: Some(1),
                },
                Question {
                    id: "name".into(),
                    question: "Project name?".into(),
                    options: Vec::new(),
                    recommended: None,
                },
            ],
        }
    }

    fn payload(action: serde_json::Value) -> SlackInteractionEvent {
        serde_json::from_value(json!({
            "type": "block_actions",
            "team": { "id": "T1" },
            "user": { "id": "U42" },
            "api_app_id": "A1",
            "container": { "type": "message", "message_ts": "100.1", "channel_id": "C9" },
            "trigger_id": "tr1",
            "channel": { "id": "C9", "name": "general" },
            "response_url": "https://hooks.slack.com/actions/T1/1/x",
            "actions": [action],
        }))
        .unwrap()
    }

    fn button_payload(action_id: &str, value: &str) -> SlackInteractionEvent {
        payload(json!({ "type": "button", "action_id": action_id, "value": value }))
    }

    #[test]
    fn question_card_shape() {
        let r = req();
        let id = r.id.0.to_string();
        let blocks = question_card(&r, 0, None);
        let blocks = blocks.as_array().unwrap();
        assert_eq!(blocks.len(), 3);
        assert_eq!(
            blocks[0]["text"],
            "❓ **Question 1/2** — Which language?\n1. **Rust** — fast\n2. **Go** _(recommended)_"
        );
        let buttons = blocks[1]["elements"].as_array().unwrap();
        assert_eq!(buttons.len(), 4);
        assert_eq!(buttons[0]["action_id"], ACTION_PICK);
        assert_eq!(buttons[0]["value"], format!("{id}:0:0"));
        assert!(buttons[0].get("style").is_none());
        assert_eq!(buttons[1]["value"], format!("{id}:0:1"));
        assert_eq!(buttons[1]["style"], "primary");
        assert_eq!(buttons[2]["action_id"], ACTION_SKIP);
        assert_eq!(buttons[2]["value"], format!("{id}:0"));
        assert_eq!(buttons[3]["action_id"], ACTION_DISMISS);
        assert_eq!(buttons[3]["style"], "danger");
        assert_eq!(buttons[3]["value"], id);
        assert_eq!(blocks[2]["type"], "input");
        assert_eq!(blocks[2]["dispatch_action"], true);
        assert_eq!(blocks[2]["block_id"], format!("ask_text:{id}:0"));
        assert_eq!(blocks[2]["element"]["action_id"], ACTION_TEXT);

        let freeform = question_card(&r, 1, Some("session 1a2b3c4d · fix tests"));
        let freeform = freeform.as_array().unwrap();
        assert_eq!(freeform.len(), 4);
        assert_eq!(
            freeform[0]["text"],
            "❓ **Question 2/2** — Project name?\n_(type your answer)_"
        );
        assert_eq!(freeform[1]["elements"].as_array().unwrap().len(), 2);
        assert_eq!(freeform[3]["type"], "context");
    }

    #[test]
    fn answered_and_closed_cards() {
        let r = req();
        assert_eq!(
            answered_card(&r, 0, &Answer::Choice { index: 1 })[0]["text"],
            "✅ **Question 1/2** — Which language?\n→ Go"
        );
        assert_eq!(
            answered_card(
                &r,
                1,
                &Answer::Text {
                    text: "theuth".into()
                }
            )[0]["text"],
            "✅ **Question 2/2** — Project name?\n→ theuth"
        );
        assert_eq!(
            answered_card(&r, 1, &Answer::Skipped)[0]["text"],
            "✅ **Question 2/2** — Project name?\n→ _skipped_"
        );
        assert_eq!(
            closed_card(&r, 0, true, false)[0]["text"],
            "🚫 **Questions dismissed** (turn cancelled)"
        );
        assert_eq!(
            closed_card(&r, 1, false, true)[0]["text"],
            "⌛ **Question 2/2** — timed out"
        );
    }

    #[test]
    fn click_parses_every_kind_and_ignores_foreign() {
        let id = uuid::Uuid::from_u128(9);
        let pick = click(&button_payload(ACTION_PICK, &format!("{id}:1:2"))).unwrap();
        assert_eq!(
            pick,
            Click {
                id: AskId(id),
                question: Some(1),
                kind: Kind::Pick(2),
                user: "U42".into(),
                channel: "C9".into(),
                message_ts: "100.1".into(),
                response_url: Some("https://hooks.slack.com/actions/T1/1/x".into()),
            }
        );
        let skip = click(&button_payload(ACTION_SKIP, &format!("{id}:1"))).unwrap();
        assert_eq!((skip.question, skip.kind), (Some(1), Kind::Skip));
        let dismiss = click(&button_payload(ACTION_DISMISS, &id.to_string())).unwrap();
        assert_eq!((dismiss.question, dismiss.kind), (None, Kind::Dismiss));
        let text = click(&payload(json!({
            "type": "plain_text_input",
            "action_id": ACTION_TEXT,
            "block_id": format!("ask_text:{id}:1"),
            "value": "  postgres ",
        })))
        .unwrap();
        assert_eq!(
            (text.question, text.kind),
            (Some(1), Kind::Text("postgres".into()))
        );

        assert_eq!(
            click(&payload(json!({
                "type": "plain_text_input",
                "action_id": ACTION_TEXT,
                "block_id": format!("ask_text:{id}:1"),
                "value": "   ",
            }))),
            None
        );
        assert_eq!(
            click(&button_payload(ACTION_PICK, &format!("{id}:1"))),
            None
        );
        assert_eq!(click(&button_payload(ACTION_PICK, "nope:1:2")), None);
        assert_eq!(
            click(&button_payload(
                crate::approvals::ACTION_APPROVE,
                &id.to_string()
            )),
            None
        );
    }
}
