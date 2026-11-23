//! Approval cards and their presses (design doc 98 §10.1). Pure: text and
//! keyboards in, parsed presses out; the channel posts, edits and resolves.

use chat_common::truncate;
use runtime::SessionId;
use tools::{ApprovalId, ApprovalRequested, Decision};

use crate::api::{Button, Keyboard, MsgRef};

/// `callback_data` prefix: `ap:<ApprovalId>:a|d`.
pub const PREFIX: &str = "ap:";
/// Longest formatted argument block in [`body`].
const MAX_ARGS_CHARS: usize = 600;

/// One posted card, remembered until `ApprovalResolved` (thread and owner
/// chat cards alike).
#[derive(Clone, Debug)]
pub struct Card {
    /// The session the request belongs to.
    pub session: SessionId,
    /// The thread's user, allowed to press beside the owner; `None` for owner-chat cards.
    pub thread_user: Option<u64>,
    /// 210 §7.2: only the owner may press it, never the thread user.
    pub human_only: bool,
    /// Where the card is, once sent.
    pub at: Option<MsgRef>,
    /// The text [`card`] used (edited in place on resolution).
    pub body: String,
    /// Who allowed/denied it here, for the resolved line.
    pub by: Option<String>,
}

/// Card text: [`body`] plus the owner-chat context line when given.
#[must_use]
pub fn card(req: &ApprovalRequested, context: Option<&str>) -> String {
    let mut text = body(req);
    if let Some(c) = context {
        text.push('\n');
        text.push_str(c);
    }
    text
}

/// The Allow / Deny row.
#[must_use]
pub fn keyboard(id: ApprovalId) -> Keyboard {
    Keyboard {
        rows: vec![vec![
            Button::Callback {
                text: "Allow".into(),
                data: format!("{PREFIX}{}:a", id.0),
            },
            Button::Callback {
                text: "Deny".into(),
                data: format!("{PREFIX}{}:d", id.0),
            },
        ]],
        force_reply: false,
    }
}

/// Line 1 `⚠️ **<tool>** wants to run`, then `shell` → the command in a
/// ```sh fence; other tools → pretty JSON args (≤ [`MAX_ARGS_CHARS`], `…`
/// when cut) in a ```json fence (09 §10 / 21 §10.1 shape).
#[must_use]
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
        // 210 §7.2: the thread user's press is refused; say so on the card.
        "\n_Only the owner can answer this._"
    } else {
        ""
    };
    format!(
        "⚠️ **{}** wants to run\n```{lang}\n{args}\n```{owner_only}",
        req.tool
    )
}

/// The card after resolution: the first line of `body` becomes the verdict
/// (`✓ allowed by <name>` / `✗ denied` / `⌛ timed out` / 207: left to the
/// agent), the fence stays, and the buttons become one disabled button
/// showing the verdict.
#[must_use]
pub fn resolved(
    body: &str,
    decision: Decision,
    timed_out: bool,
    dismissed: bool,
    by: Option<&str>,
) -> (String, Keyboard) {
    let (head, rest) = body.split_once('\n').unwrap_or((body, ""));
    let tool = head.split("**").nth(1).unwrap_or(head);
    let (mut line, label) = match (decision, timed_out) {
        _ if dismissed => (
            format!("**{tool}** left to the agent (not run, not refused)"),
            "left to the agent",
        ),
        (_, true) => (format!("⌛ **{tool}** timed out"), "⌛ timed out"),
        (Decision::Allow, false) => (format!("✓ **{tool}** allowed"), "✓ allowed"),
        (Decision::Deny, false) => (format!("✗ **{tool}** denied"), "✗ denied"),
    };
    if let Some(name) = by.filter(|_| !timed_out && !dismissed) {
        line.push_str(&format!(" by {name}"));
    }
    if !rest.is_empty() {
        line.push('\n');
        line.push_str(rest);
    }
    let keyboard = Keyboard {
        rows: vec![vec![Button::Disabled {
            text: label.to_owned(),
        }]],
        force_reply: false,
    };
    (line, keyboard)
}

/// Parses `ap:<id>:a|d`; `None` for anything else.
#[must_use]
pub fn parse(data: &str) -> Option<(ApprovalId, Decision)> {
    let rest = data.strip_prefix(PREFIX)?;
    let (id, verdict) = rest.split_once(':')?;
    let decision = match verdict {
        "a" => Decision::Allow,
        "d" => Decision::Deny,
        _ => return None,
    };
    Some((ApprovalId(id.parse().ok()?), decision))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn req(tool: &str, args: serde_json::Value) -> ApprovalRequested {
        ApprovalRequested {
            id: ApprovalId(uuid::Uuid::new_v4()),
            session: SessionId::new(),
            tool: tool.into(),
            args,
            risk: tools::Risk::Mutating,
            human_only: false,
        }
    }

    #[test]
    fn shell_commands_are_fenced_verbatim_and_other_args_as_json() {
        let r = req("shell", json!({ "command": "rm -rf target/" }));
        assert_eq!(
            body(&r),
            "⚠️ **shell** wants to run\n```sh\nrm -rf target/\n```"
        );
        let r = req("write_file", json!({ "path": "a.txt" }));
        let b = body(&r);
        assert!(b.starts_with("⚠️ **write_file** wants to run\n```json\n"));
        assert!(b.contains("\"path\": \"a.txt\""));
        let r = req("x", json!({ "blob": "y".repeat(2000) }));
        assert!(body(&r).contains('…'));
    }

    #[test]
    fn press_data_round_trips_and_garbage_is_none() {
        let id = ApprovalId(uuid::Uuid::new_v4());
        let k = keyboard(id);
        let Button::Callback { data, .. } = &k.rows[0][0] else {
            panic!()
        };
        assert!(data.len() <= 64);
        assert_eq!(parse(data), Some((id, Decision::Allow)));
        let Button::Callback { data, .. } = &k.rows[0][1] else {
            panic!()
        };
        assert_eq!(parse(data), Some((id, Decision::Deny)));
        assert_eq!(parse("ap:nope:a"), None);
        assert_eq!(parse(&format!("ap:{}:z", id.0)), None);
        assert_eq!(parse("ask:x"), None);
    }

    #[test]
    fn resolved_card_rewrites_the_verdict_line_and_disables_the_buttons() {
        let b = body(&req("shell", json!({ "command": "ls" })));
        let (text, k) = resolved(&b, Decision::Allow, false, false, Some("Ann"));
        assert_eq!(text, "✓ **shell** allowed by Ann\n```sh\nls\n```");
        assert_eq!(
            k.rows,
            vec![vec![Button::Disabled {
                text: "✓ allowed".into()
            }]]
        );
        let (text, _) = resolved(&b, Decision::Deny, false, false, None);
        assert!(text.starts_with("✗ **shell** denied\n"));
        let (text, k) = resolved(&b, Decision::Deny, true, false, Some("Ann"));
        assert!(text.starts_with("⌛ **shell** timed out\n"));
        assert_eq!(
            k.rows[0][0],
            Button::Disabled {
                text: "⌛ timed out".into()
            }
        );
        // 207 E4: left to the agent is not a denial and names nobody.
        let (text, k) = resolved(&b, Decision::Deny, false, true, Some("Ann"));
        assert_eq!(
            text,
            "**shell** left to the agent (not run, not refused)\n```sh\nls\n```"
        );
        assert_eq!(
            k.rows[0][0],
            Button::Disabled {
                text: "left to the agent".into()
            }
        );
    }
}
