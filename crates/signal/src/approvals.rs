//! Approval codes, message text and reaction/answer routing (design doc 09 §10).
//!
//! The channel owns a [`Pending`] map: `code → (ApprovalId, session, number,
//! sent timestamp)`. A resolution from anywhere removes the entry and leaves a
//! tombstone for [`TOMBSTONE_TTL`] so a late `/approve` gets a useful answer.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use chat_common::truncate;
use kameo::actor::ActorRef;
use runtime::SessionId;
use tools::{ApprovalId, ApprovalRequested, ApprovalResolved, Decision};

/// How long a resolved code still answers "was already …".
pub const TOMBSTONE_TTL: Duration = Duration::from_secs(600);
/// Code length; alphabet has no `0 O 1 I l`.
pub const CODE_LEN: usize = 4;
const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
/// Longest formatted argument block in [`message`].
const MAX_ARGS_CHARS: usize = 600;

/// A pending approval sent (or being sent) to a phone thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Short code the user types.
    pub code: String,
    /// The request.
    pub id: ApprovalId,
    /// Session the tool call belongs to.
    pub session: SessionId,
    /// Thread (E.164) the approval message went to.
    pub number: String,
    /// Timestamp of our approval message once sent; reactions target it.
    pub sent_ts: Option<u64>,
    /// 210 §7.2: a human-only card only the owner may answer.
    pub human_only: bool,
}

/// Result of [`Pending::by_code`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lookup {
    /// Still waiting.
    Pending(ApprovalId),
    /// Resolved within [`TOMBSTONE_TTL`].
    Resolved {
        /// Outcome.
        decision: Decision,
        /// Nobody answered in time.
        timed_out: bool,
        /// 207: left to the agent from Attention, not refused.
        dismissed: bool,
    },
    /// Never seen, or tombstone expired.
    Unknown,
}

#[derive(Clone, Copy, Debug)]
struct Tombstone {
    decision: Decision,
    timed_out: bool,
    dismissed: bool,
    at: Instant,
}

/// Live codes plus recent tombstones.
#[derive(Debug, Default)]
pub struct Pending {
    live: HashMap<String, Entry>,
    tombstones: HashMap<String, Tombstone>,
}

impl Pending {
    /// Registers `req` for thread `number`; returns a fresh code unique among
    /// live codes and tombstones.
    pub fn insert(&mut self, req: &ApprovalRequested, number: &str) -> String {
        let code = loop {
            let bytes = uuid::Uuid::new_v4().into_bytes();
            let candidate: String = bytes[..CODE_LEN]
                .iter()
                .map(|b| ALPHABET[usize::from(*b) % ALPHABET.len()] as char)
                .collect();
            if !self.live.contains_key(&candidate) && !self.tombstones.contains_key(&candidate) {
                break candidate;
            }
        };
        self.live.insert(
            code.clone(),
            Entry {
                code: code.clone(),
                id: req.id,
                session: req.session,
                number: number.to_owned(),
                sent_ts: None,
                human_only: req.human_only,
            },
        );
        code
    }

    /// Records the timestamp of the sent approval message.
    pub fn mark_sent(&mut self, code: &str, sent_ts: u64) {
        if let Some(e) = self.live.get_mut(code) {
            e.sent_ts = Some(sent_ts);
        }
    }

    /// Case-insensitive lookup; tombstones within [`TOMBSTONE_TTL`] report the outcome.
    pub fn by_code(&self, code: &str) -> Lookup {
        let code = code.trim().to_lowercase();
        if let Some(e) = self.live.get(&code) {
            return Lookup::Pending(e.id);
        }
        match self.tombstones.get(&code) {
            Some(t) if t.at.elapsed() <= TOMBSTONE_TTL => Lookup::Resolved {
                decision: t.decision,
                timed_out: t.timed_out,
                dismissed: t.dismissed,
            },
            _ => Lookup::Unknown,
        }
    }

    /// The pending request whose message `number` reacted to at `target_ts`.
    pub fn by_reaction(&self, number: &str, target_ts: u64) -> Option<&Entry> {
        self.live
            .values()
            .find(|e| e.sent_ts == Some(target_ts) && e.number == number)
    }

    /// Drops the entry for `res.id` and tombstones it. `None` if unknown.
    pub fn resolve(&mut self, res: &ApprovalResolved) -> Option<Entry> {
        let code = self.live.values().find(|e| e.id == res.id)?.code.clone();
        let entry = self.live.remove(&code)?;
        self.tombstones.insert(
            code,
            Tombstone {
                decision: res.decision,
                timed_out: res.timed_out,
                dismissed: res.dismissed,
                at: Instant::now(),
            },
        );
        Some(entry)
    }

    /// True when `text` is just a known (pending or tombstoned) code.
    pub fn is_code(&self, text: &str) -> bool {
        let t = text.trim();
        t.chars().count() == CODE_LEN && self.by_code(t) != Lookup::Unknown
    }

    /// Drops tombstones older than [`TOMBSTONE_TTL`].
    pub fn sweep(&mut self) {
        self.tombstones
            .retain(|_, t| t.at.elapsed() <= TOMBSTONE_TTL);
    }
}

/// The approval prompt sent to the phone (09 §10).
pub fn message(req: &ApprovalRequested, code: &str) -> String {
    let mut out = format!("⚠️ {} wants to run\n", req.tool);
    let shell_cmd = (req.tool == "shell")
        .then(|| req.args.get("command").and_then(|v| v.as_str()))
        .flatten();
    let body = match shell_cmd {
        Some(cmd) => cmd.to_owned(),
        None => truncate(
            &serde_json::to_string_pretty(&req.args).unwrap_or_default(),
            MAX_ARGS_CHARS,
        ),
    };
    for line in body.lines() {
        out.push_str("  ");
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(&format!(
        "👍 to allow · 👎 to deny · or /approve {code} · /deny {code}"
    ));
    if req.human_only {
        out.push_str("\nOnly the owner can answer this.");
    }
    out
}

/// 👍 ✅ 👌 → Allow, 👎 ❌ 🚫 → Deny; variation selectors and skin tones ignored.
pub fn decision_for_emoji(emoji: &str) -> Option<Decision> {
    let base: String = emoji
        .chars()
        .filter(|c| *c != '\u{FE0F}' && !('\u{1F3FB}'..='\u{1F3FF}').contains(c))
        .collect();
    match base.as_str() {
        "👍" | "✅" | "👌" => Some(Decision::Allow),
        "👎" | "❌" | "🚫" => Some(Decision::Deny),
        _ => None,
    }
}

/// Reaction we put on our own approval message once resolved. `None` for one
/// the user left to the agent (207): no reaction here reads as anything but
/// allow / deny / expired, so the channel says it in words ([`left_text`]).
pub fn reaction_for(res: &ApprovalResolved) -> Option<&'static str> {
    if res.dismissed {
        None
    } else if res.timed_out {
        Some("⌛")
    } else {
        match res.decision {
            Decision::Allow => Some("✅"),
            Decision::Deny => Some("❌"),
        }
    }
}

/// 207: the line sent for an approval the user left to the agent.
pub fn left_text(code: &str) -> String {
    format!("{code} {}", outcome(Decision::Deny, false, true))
}

fn outcome(decision: Decision, timed_out: bool, dismissed: bool) -> &'static str {
    if dismissed {
        "left to the agent (not run, not refused)"
    } else if timed_out {
        "timed out"
    } else {
        match decision {
            Decision::Allow => "allowed",
            Decision::Deny => "denied",
        }
    }
}

/// `/approve <code>` / `/deny <code>`: asks the approval actor and returns
/// the reply text. Does not touch `pending`; the channel sees
/// `ApprovalResolved` on the bus and calls [`Pending::resolve`].
pub async fn resolve_by_code(pending: &mut Pending, code: &str, decision: Decision) -> String {
    let code = code.trim().to_lowercase();
    match pending.by_code(&code) {
        Lookup::Unknown => format!("unknown code {code}"),
        Lookup::Resolved {
            decision,
            timed_out,
            dismissed,
        } => format!(
            "{code} was already {}",
            outcome(decision, timed_out, dismissed)
        ),
        Lookup::Pending(id) => {
            let Ok(Some(actor)) = ActorRef::<tools::Approval>::lookup(tools::APPROVAL_ACTOR) else {
                return "error: approval actor is not running".to_owned();
            };
            match actor.ask(tools::Resolve { id, decision }).await {
                Ok(true) => format!("{code} {}", outcome(decision, false, false)),
                Ok(false) => format!("{code} was already resolved"),
                Err(e) => format!("error: {e}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn req(tool: &str, args: serde_json::Value) -> ApprovalRequested {
        ApprovalRequested {
            id: ApprovalId(uuid::Uuid::new_v4()),
            session: SessionId::new(),
            tool: tool.to_owned(),
            args,
            risk: tools::Risk::Destructive,
            human_only: false,
        }
    }

    #[test]
    fn codes_unique_from_alphabet() {
        let mut p = Pending::default();
        let mut seen = std::collections::HashSet::new();
        for _ in 0..200 {
            let code = p.insert(&req("shell", json!({})), "+15550001");
            assert_eq!(code.len(), CODE_LEN);
            assert!(code.bytes().all(|b| ALPHABET.contains(&b)), "{code}");
            assert!(seen.insert(code));
        }
    }

    #[test]
    fn by_code_case_insensitive() {
        let mut p = Pending::default();
        let r = req("shell", json!({}));
        let code = p.insert(&r, "+1");
        assert_eq!(p.by_code(&code.to_uppercase()), Lookup::Pending(r.id));
        assert!(p.is_code(&format!(" {} ", code.to_uppercase())));
        assert!(!p.is_code("zzzz"));
    }

    #[test]
    fn reaction_needs_number_and_ts() {
        let mut p = Pending::default();
        let r = req("shell", json!({}));
        let code = p.insert(&r, "+1");
        assert!(p.by_reaction("+1", 42).is_none());
        p.mark_sent(&code, 42);
        assert_eq!(p.by_reaction("+1", 42).map(|e| e.id), Some(r.id));
        assert!(p.by_reaction("+2", 42).is_none());
        assert!(p.by_reaction("+1", 43).is_none());
    }

    #[test]
    fn resolve_tombstones_and_sweep() {
        let mut p = Pending::default();
        let r = req("shell", json!({}));
        let code = p.insert(&r, "+1");
        let res = ApprovalResolved {
            id: r.id,
            decision: Decision::Deny,
            timed_out: true,
            dismissed: false,
        };
        assert_eq!(p.resolve(&res).map(|e| e.code), Some(code.clone()));
        assert_eq!(p.resolve(&res), None);
        assert_eq!(
            p.by_code(&code),
            Lookup::Resolved {
                decision: Decision::Deny,
                timed_out: true,
                dismissed: false,
            }
        );
        assert!(p.is_code(&code));
        p.sweep();
        assert_eq!(p.tombstones.len(), 1);
        p.tombstones.get_mut(&code).unwrap().at = Instant::now()
            .checked_sub(TOMBSTONE_TTL + Duration::from_secs(1))
            .unwrap();
        assert_eq!(p.by_code(&code), Lookup::Unknown);
        p.sweep();
        assert!(p.tombstones.is_empty());
    }

    #[test]
    fn emoji_variants() {
        assert_eq!(decision_for_emoji("👍🏽"), Some(Decision::Allow));
        assert_eq!(decision_for_emoji("✅️"), Some(Decision::Allow));
        assert_eq!(decision_for_emoji("🚫"), Some(Decision::Deny));
        assert_eq!(decision_for_emoji("👎🏿"), Some(Decision::Deny));
        assert_eq!(decision_for_emoji("❤️"), None);
    }

    #[test]
    fn reactions() {
        let allow = ApprovalResolved {
            id: ApprovalId(uuid::Uuid::new_v4()),
            decision: Decision::Allow,
            timed_out: false,
            dismissed: false,
        };
        assert_eq!(reaction_for(&allow), Some("✅"));
        assert_eq!(
            reaction_for(&ApprovalResolved {
                decision: Decision::Deny,
                ..allow
            }),
            Some("❌")
        );
        assert_eq!(
            reaction_for(&ApprovalResolved {
                timed_out: true,
                ..allow
            }),
            Some("⌛")
        );
    }

    /// 207 E4: an approval left to the agent gets no deny reaction and is
    /// reported as left to the agent, on the card and on a late `/deny`.
    #[test]
    fn dismissed_approval_reads_left_to_the_agent() {
        let mut p = Pending::default();
        let r = req("shell", json!({}));
        let code = p.insert(&r, "+1");
        let res = ApprovalResolved {
            id: r.id,
            decision: Decision::Deny,
            timed_out: false,
            dismissed: true,
        };
        assert_eq!(reaction_for(&res), None);
        assert_eq!(
            left_text(&code),
            format!("{code} left to the agent (not run, not refused)")
        );
        p.resolve(&res);
        let Lookup::Resolved {
            decision,
            timed_out,
            dismissed,
        } = p.by_code(&code)
        else {
            panic!("tombstoned");
        };
        assert!(dismissed);
        assert_eq!(
            outcome(decision, timed_out, dismissed),
            "left to the agent (not run, not refused)"
        );
    }

    #[test]
    fn shell_message_shows_command() {
        let m = message(&req("shell", json!({"command": "rm -rf target/"})), "k7mq");
        assert_eq!(
            m,
            "⚠️ shell wants to run\n  rm -rf target/\n👍 to allow · 👎 to deny · or /approve k7mq · /deny k7mq"
        );
        let m = message(&req("write_file", json!({"path": "x"})), "k7mq");
        assert!(m.contains("⚠️ write_file wants to run\n  {\n    \"path\": \"x\"\n  }\n"));
        let long = "a".repeat(700);
        let m = message(&req("write_file", json!({"body": long})), "k7mq");
        assert!(m.contains('…'));
        assert!(m.chars().count() < 800);
    }

    #[tokio::test]
    async fn resolve_by_code_replies() {
        let mut p = Pending::default();
        assert_eq!(
            resolve_by_code(&mut p, "zzzz", Decision::Allow).await,
            "unknown code zzzz"
        );
        let r = req("shell", json!({}));
        let code = p.insert(&r, "+1");
        p.resolve(&ApprovalResolved {
            id: r.id,
            decision: Decision::Allow,
            timed_out: false,
            dismissed: false,
        });
        assert_eq!(
            resolve_by_code(&mut p, &code.to_uppercase(), Decision::Deny).await,
            format!("{code} was already allowed")
        );
        let code = p.insert(&req("shell", json!({})), "+1");
        assert_eq!(
            resolve_by_code(&mut p, &code, Decision::Allow).await,
            "error: approval actor is not running"
        );
    }
}
