//! The decision-oracle pre-filter on transcript batches (57 §3, revised in §3.5 after the first
//! shadow session): before a flushed batch spends a meeting-session model turn —
//!
//! 0. [`mentions`] (deterministic): capitalised n-grams, ticket ids, acronyms, numbers with
//!    units, quoted terms. Empty mentions are one of the two conditions for a skip.
//! 1. Tier 1a — `meeting.kind`, `Bool(KIND_STATEMENT)`: the phrasing bench
//!    (`decision` example `phrasing`) found the classifier a usable *small-talk detector* and a
//!    useless subject detector, so the question is "is this only small talk?".
//! 2. Tier 1b — `meeting.brief`, `Choice` over the top-4 FTS rows (`b1..b4`, titles only) + `none`.
//! 3. The asymmetric gate ([`settle`]): **skip** only when 1a says small talk with P ≥ τ *and*
//!    `mentions` is empty; **publish** when 1b accepts a brief (P ≥ τ_brief, margin ≥ τ_margin);
//!    everything else goes to tier 2 — a missed brief costs more than a wrong cached one.
//! 4. Tier 2 (subject extraction through `decision::escalate`) then tier 3 (today's turn, with
//!    the oracle's numbers as one evidence line).
//!
//! Sky's switch (§3.4) is the `meeting` prefix's mode: **off** = never asked, **shadow** = asked
//! and shown, today's path still runs and the turn's outcome is reported back, **active** = the
//! gate stands. Everything here runs off the actor's mailbox ([`run`] is a spawned task that
//! `tell`s a [`Verdict`] back); the pure parts ([`mentions`], [`settle`], [`evidence`],
//! [`outcome`]) are what the tests cover.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use decision::training::{self, Amendment, ExampleId};
use decision::{Ask, Decision, Disposition, LogId, Mode, Question, Reason};
use kameo::actor::ActorRef;
use runtime::{Runtime, SessionId};
use serde::{Deserialize, Serialize};

use crate::cache::{Cache, CachedBrief};
use crate::{BriefId, MeetingEvent, MeetingId, Track, Utterance, session};

/// The config prefix Settings → Meetings switches: `decision.families.meeting.mode`.
pub const PREFIX: &str = "meeting";
/// Family of the small-talk `Bool` (tier 1a).
pub const KIND: &str = "meeting.kind";
/// Family of the cached-brief `Choice` (tier 1b).
pub const BRIEF: &str = "meeting.brief";
/// Family of tier 2's subject extraction rows (`decision::subject`).
pub const SUBJECT: &str = decision::subject::FAMILY;
/// Most cached briefs offered as candidates (57 §3.5: 4, was 8).
pub const CANDIDATES: usize = 4;
/// Most mentions kept per batch.
pub const MAX_MENTIONS: usize = 12;
/// The 1a statement — the phrasing bench's winner (57 as-built step 4b): `true` = small talk.
pub const KIND_STATEMENT: &str = "The speakers are only greeting, scheduling or saying goodbye";
/// The 1b instructions (57 §3.1, unchanged).
pub const BRIEF_INSTRUCTIONS: &str = "which cached brief is the subject being discussed";

/// Where a batch went (57 §3.2), as the island and the session rows show it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OracleDisposition {
    /// Small talk with no mentions (or tier 2 found nothing to look up): no model turn; the
    /// batch rides along as context in the next delivered one.
    Skip,
    /// A cached brief is (would be, in shadow) published directly.
    Brief {
        /// The cache row.
        brief: BriefId,
        /// Its title, for the row.
        title: String,
    },
    /// Tier 3 — today's model turn — with why the oracle did not settle it.
    Model {
        /// `not small talk`, `small talk but mentions`, `tier 2: timed out`, …
        reason: String,
    },
}

/// What the meeting session's turn did with a shadowed (or escalated) batch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct OracleOutcome {
    /// The model did what the oracle's answers said (nothing, or the same brief).
    pub agreed: bool,
    /// The title the model published, if it published one.
    pub published: Option<String>,
}

/// One oracle-handled batch on the meeting event stream (57 §3.4 data path). Emitted once when
/// the oracle answers; again with `outcome` after the model turn in shadow (and after a tier-3
/// turn in active).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct OracleDecision {
    /// `hh:mm:ss` of the batch's first utterance — the `[meeting hh:mm:ss]` line's stamp, so the
    /// session view attaches the row to its batch card.
    pub batch_id: String,
    /// The family whose answer settled it (`meeting.kind` / `meeting.brief` / `meeting.subject`).
    pub family: String,
    /// The switch position when it ran.
    pub mode: Mode,
    /// Where the batch went.
    pub disposition: OracleDisposition,
    /// The stages as the ⚡ row shows them, in order: `("kind: small talk", 0.79)`,
    /// `("brief: none", 0.57)` — each the winning label of one tier-1 question and its P.
    pub stages: Vec<(String, f32)>,
    /// Step 0's deterministic mentions (`Kuiper belt`, `SRE-2386`, …).
    pub mentions: Vec<String>,
    /// Tier 2's extracted subject, when it ran and named one.
    pub subject: Option<String>,
    /// Tier 2's verdict as text (`"Planet Nine" → no cached brief`, `nothing to look up`,
    /// `timed out`), `None` when tier 2 did not run.
    pub tier2: Option<String>,
    /// Every brief candidate (title) and `none` with its probability, oracle order — the
    /// row's expandable detail.
    pub top: Vec<(String, f32)>,
    /// Wall clock from the first ask to the verdict (tier 2 included).
    pub latency_ms: u64,
    /// 1 oracle, 2 cheap model, 3 main model turn.
    pub tier: u8,
    /// Filled after the model turn.
    pub outcome: Option<OracleOutcome>,
}

/// What tier 1 settled for a batch (57 §3.5 item 3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Settled {
    /// Small talk accepted and no mentions.
    Skip,
    /// A brief accepted (the candidate id, `b1..b4`).
    Brief(String),
    /// Everything else, with why (`not small talk`, `small talk but mentions: …`, `kind timeout`).
    Tier2(String),
}

/// Step 0 (57 §3.5): what the batch names, deterministically — capitalised runs not at a
/// sentence start (a run of ≥ 2 capitalised words counts anywhere), ticket ids (`SRE-2386`),
/// acronyms (≥ 2 capitals), numbers with a unit (`33 kV`, `600 seconds`, `10min`) and quoted
/// terms. Speaker tags (`me:`/`them:`) are not text. Deduplicated case-insensitively, first
/// occurrence order, at most [`MAX_MENTIONS`].
pub fn mentions(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |m: &str| {
        let m = m.trim_matches(|c: char| !c.is_alphanumeric());
        if m.len() < 2 || out.len() >= MAX_MENTIONS {
            return;
        }
        if !out.iter().any(|o| o.eq_ignore_ascii_case(m)) {
            out.push(m.to_owned());
        }
    };
    for line in text.lines() {
        let line = line
            .trim_start()
            .strip_prefix("me:")
            .or_else(|| line.trim_start().strip_prefix("them:"))
            .unwrap_or(line);
        // Quoted terms first, so the words inside are also seen by the run scan below.
        for quote in ['"', '“', '«'] {
            let close = match quote {
                '“' => '”',
                '«' => '»',
                q => q,
            };
            let mut rest = line;
            while let Some(start) = rest.find(quote) {
                let after = &rest[start + quote.len_utf8()..];
                let Some(end) = after.find(close) else { break };
                let inner = after[..end].trim();
                if !inner.is_empty() && inner.split_whitespace().count() <= 6 {
                    push(inner);
                }
                rest = &after[end + close.len_utf8()..];
            }
        }
        let words: Vec<&str> = line.split_whitespace().collect();
        let mut i = 0;
        while i < words.len() {
            let w = words[i];
            let core = w.trim_matches(|c: char| !c.is_alphanumeric());
            if core.is_empty() {
                i += 1;
                continue;
            }
            // Sentence start: first word of the line, or the previous word ended a sentence.
            let sentence_start = i == 0 || words[i - 1].ends_with(['.', '?', '!']);
            let ends_sentence = |w: &str| w.ends_with(['.', '?', '!', ',', ';', ':']);
            // ponytail: a capitalised sentence opener (`So Mike…`, `They were…`) is not a name;
            // a stoplist of openers keeps the next word in play. A proper name that is also an
            // opener (`Will`, `Mark`) is missed at a sentence start — tier 2 reads the text anyway.
            if sentence_start && SENTENCE_OPENERS.contains(&core.to_ascii_lowercase().as_str()) {
                i += 1;
                continue;
            }
            if is_ticket(core) {
                push(core);
                i += 1;
                continue;
            }
            if let Some(next_is_unit) = number_with_unit(core, words.get(i + 1).copied()) {
                if next_is_unit {
                    push(&format!("{core} {}", words[i + 1].trim_matches(|c: char| !c.is_alphanumeric())));
                    i += 2;
                } else {
                    push(core);
                    i += 1;
                }
                continue;
            }
            if is_acronym(core) {
                push(core);
                i += 1;
                continue;
            }
            if capitalised(core) {
                // Extend the run over following capitalised words.
                let mut j = i;
                while j + 1 < words.len() && !ends_sentence(words[j]) {
                    let next = words[j + 1].trim_matches(|c: char| !c.is_alphanumeric());
                    if capitalised(next) && !is_acronym(next) {
                        j += 1;
                    } else {
                        break;
                    }
                }
                let run_len = j - i + 1;
                let calendar = run_len == 1 && CALENDAR.contains(&core.to_ascii_lowercase().as_str());
                if run_len >= 2 || (!sentence_start && core.len() >= 2 && !calendar) {
                    let run: Vec<&str> = words[i..=j]
                        .iter()
                        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()))
                        .collect();
                    push(&run.join(" "));
                }
                i = j + 1;
                continue;
            }
            i += 1;
        }
    }
    out
}

/// `SRE-2386`: two or more capitals, a dash, digits.
fn is_ticket(w: &str) -> bool {
    let Some((a, b)) = w.split_once('-') else { return false };
    a.len() >= 2 && a.chars().all(|c| c.is_ascii_uppercase()) && !b.is_empty() && b.chars().all(|c| c.is_ascii_digit())
}

/// `DR`, `SSL`, `CNAME`, `2FA`: ≥ 2 capitals, no lowercase, at least two letters.
fn is_acronym(w: &str) -> bool {
    w.len() >= 2
        && w.chars().filter(|c| c.is_ascii_uppercase()).count() >= 2
        && w.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

/// A word starting with an uppercase letter followed by lowercase (`Kuiper`, `Mike`), not `I`.
fn capitalised(w: &str) -> bool {
    let mut chars = w.chars();
    matches!(chars.next(), Some(c) if c.is_uppercase()) && chars.clone().any(|c| c.is_lowercase()) && chars.all(char::is_alphanumeric)
}

/// A number with a unit: `10min` / `5GB` (suffix on the number) → `Some(false)`; `33 kV` /
/// `0.1 Hz` / `600 seconds` (the unit is the next word: letters, ≤ 8 chars, not an ordinal
/// suffix or a function word) → `Some(true)`; a bare number, date or ordinal → `None`.
fn number_with_unit(w: &str, next: Option<&str>) -> Option<bool> {
    let digits = w.chars().take_while(|c| c.is_ascii_digit() || *c == '.' || *c == ',').count();
    if digits == 0 || !w.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let suffix = &w[digits..];
    if !suffix.is_empty() {
        // `10min`, `5GB`; not `30th`, `1st`.
        return (suffix.chars().all(char::is_alphabetic) && !is_ordinal(suffix)).then_some(false);
    }
    let unit = next?.trim_matches(|c: char| !c.is_alphanumeric());
    (!unit.is_empty()
        && unit.len() <= 8
        && unit.chars().all(char::is_alphabetic)
        && !is_ordinal(unit)
        && !STOP_AFTER_NUMBER.contains(&unit.to_ascii_lowercase().as_str()))
    .then_some(true)
}

fn is_ordinal(s: &str) -> bool {
    matches!(s.to_ascii_lowercase().as_str(), "st" | "nd" | "rd" | "th")
}

/// Words after a number that are not units (`the 15th of October`, `two of them`).
const STOP_AFTER_NUMBER: [&str; 24] = [
    "of", "and", "or", "to", "in", "on", "at", "for", "the", "a", "an", "is", "are", "was", "were", "so", "but",
    "if", "it", "that", "this", "with", "from", "by",
];

/// Capitalised only because they open a sentence (transcripts: fillers included).
const SENTENCE_OPENERS: [&str; 72] = [
    "so", "and", "but", "or", "the", "a", "an", "they", "he", "she", "we", "i", "it", "its", "it's", "that", "this",
    "these", "those", "then", "there", "well", "also", "now", "no", "yes", "yeah", "yep", "okay", "ok", "oh", "um",
    "uh", "hey", "hi", "hello", "has", "have", "is", "are", "was", "were", "do", "does", "did", "can", "could", "would",
    "should", "will", "what", "why", "how", "when", "where", "who", "which", "let", "let's", "just", "right", "sure",
    "anyway", "anyways", "because", "if", "maybe", "actually", "basically", "honestly", "sorry", "thanks",
];

/// Months and weekdays: capitalised, not subjects.
const CALENDAR: [&str; 19] = [
    "january", "february", "march", "april", "may", "june", "july", "august", "september", "october", "november",
    "december", "monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday",
];

/// The accepted choice of a decision, looking through a shadow wrapper.
pub fn accepted(d: &Decision) -> Option<&str> {
    match &d.disposition {
        Disposition::Accept => Some(&d.choice),
        Disposition::Shadow(inner) => accepted(inner),
        Disposition::Escalate(_) => None,
    }
}

fn reason_text(r: &Reason) -> String {
    match r {
        Reason::LowTop1 => "low top-1".into(),
        Reason::LowMargin => "low margin".into(),
        Reason::HardError(e) => format!("error: {e}"),
        Reason::Truncated => "truncated".into(),
        Reason::Timeout => "timeout".into(),
        Reason::Disabled => "disabled".into(),
        Reason::ModelMissing => "model missing".into(),
    }
}

/// P(true) of the kind decision — P(small talk) — if it ran.
fn p_small_talk(kind: &Decision) -> Option<f32> {
    kind.probabilities.iter().find(|(l, _)| l == "true").map(|(_, p)| *p)
}

/// The asymmetric gate (57 §3.5 item 3), mode-independent (shadow is looked through): a brief
/// accepted wins (a missed brief costs more than a wrong cached one); then small talk accepted
/// **and** no mentions is a skip; everything else — small talk with mentions, not small talk,
/// an abstained or timed-out question — goes to tier 2 with the reason.
pub fn settle(kind: &Decision, brief: Option<&Decision>, mentions: &[String]) -> Settled {
    if let Some(id) = brief.and_then(accepted).filter(|id| *id != decision::engine::NONE_ID) {
        return Settled::Brief(id.to_owned());
    }
    match accepted(kind) {
        Some("true") if mentions.is_empty() => Settled::Skip,
        Some("true") => Settled::Tier2(format!("small talk but mentions: {}", mentions.join(", "))),
        Some(_) => Settled::Tier2("not small talk".into()),
        None => Settled::Tier2(format!(
            "kind {}",
            kind.disposition.reason().map_or_else(|| "undecided".into(), reason_text)
        )),
    }
}

/// The winning label of a brief decision under its title, with its probability.
fn brief_pick(brief: &Decision, titles: &HashMap<String, String>) -> Option<(String, f32)> {
    brief
        .probabilities
        .iter()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(id, p)| (titles.get(id).cloned().unwrap_or_else(|| id.clone()), *p))
}

/// The ⚡ row's stages: `kind: small talk 0.79` / `kind: subject 0.88` (the winning side of the
/// `Bool`), then `brief: <title|none> 0.57` when 1b ran. A question that never ran (timeout,
/// model missing) shows as `kind: ? 0`.
pub fn stages(kind: &Decision, brief: Option<&Decision>, titles: &HashMap<String, String>) -> Vec<(String, f32)> {
    let mut v = Vec::with_capacity(2);
    match p_small_talk(kind) {
        Some(p) if p >= 0.5 => v.push(("kind: small talk".to_owned(), p)),
        Some(p) => v.push(("kind: subject".to_owned(), 1.0 - p)),
        None => v.push(("kind: ?".to_owned(), 0.0)),
    }
    if let Some((label, p)) = brief.and_then(|b| brief_pick(b, titles)) {
        v.push((format!("brief: {label}"), p));
    }
    v
}

/// Every brief candidate (title) and `none` with its probability, oracle order.
pub fn top(brief: Option<&Decision>, titles: &HashMap<String, String>) -> Vec<(String, f32)> {
    brief.map_or_else(Vec::new, |b| {
        b.probabilities
            .iter()
            .map(|(id, p)| (titles.get(id).cloned().unwrap_or_else(|| id.clone()), *p))
            .collect()
    })
}

/// Tier 1's numbers as one data line: `kind subject 0.88, brief none 0.55 / SRE-2386 0.31` —
/// what tier 2 reads and what the evidence line carries.
pub fn numbers(kind: &Decision, brief: Option<&Decision>, titles: &HashMap<String, String>) -> String {
    let mut s = match p_small_talk(kind) {
        Some(p) if p >= 0.5 => format!("kind small talk {p:.2}"),
        Some(p) => format!("kind subject {:.2}", 1.0 - p),
        None => "kind ?".to_owned(),
    };
    if let Some(b) = brief {
        let mut ranked: Vec<&(String, f32)> = b.probabilities.iter().collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
        let two: Vec<String> = ranked
            .iter()
            .take(2)
            .map(|(id, p)| format!("{} {p:.2}", titles.get(id).map_or(id.as_str(), String::as_str)))
            .collect();
        s.push_str(&format!(", brief {}", two.join(" / ")));
    }
    s
}

/// The tier-3 evidence line when tier 2 did not settle it (57 §3.2): `oracle: kind subject
/// 0.88, brief none 0.55 / SRE-2386 0.31 (escalated: tier 2: timed out)`. Data for the model,
/// never an instruction.
pub fn evidence(kind: &Decision, brief: Option<&Decision>, titles: &HashMap<String, String>, why: &str) -> String {
    format!("oracle: {} (escalated: {why})", numbers(kind, brief, titles))
}

/// The tier-3 evidence line when tier 2 named a subject with no cached brief (57 §3.5 item 4):
/// `oracle: subject "Planet Nine / Kuiper belt" — no cached brief; kind subject 0.91, brief
/// none 0.57` — first in the batch delivery, so the research turn starts from the answer.
pub fn subject_evidence(subject: &str, numbers: &str) -> String {
    format!("oracle: subject {subject:?} — no cached brief; {numbers}")
}

/// The batch as the oracle reads it: one `them:`/`me:` line per utterance.
pub fn state(batch: &[Utterance]) -> String {
    let mut s = String::new();
    for u in batch {
        if !s.is_empty() {
            s.push('\n');
        }
        s.push_str(match u.track {
            Track::Mic => "me: ",
            Track::System => "them: ",
        });
        s.push_str(u.text.trim());
    }
    s
}

/// The batch's id: its first utterance's `hh:mm:ss`, the `[meeting …]` line's stamp.
pub fn batch_id(batch: &[Utterance]) -> String {
    session::hms(batch.first().map_or(0, |u| u.t0_ms))
}

/// What the oracle expects the model turn to do, for the shadow outcome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expected {
    /// Log ids to `Report` against: `(family log id, family, expected label)`. The kind's label
    /// is `true` (small talk: nothing published) / `false`; the brief's a cache row id or `none`;
    /// the subject's a cache row id, `lookup` (a new brief) or `skip` (nothing).
    pub rows: Vec<(LogId, String, String)>,
}

/// After the model turn: `published` = the cached rows (and titles) the turn registered with
/// `meeting_topic`. Per family: `Confirmed` when the label matches what happened, else
/// `Corrected { actual }`; the event's `agreed` is every family confirmed. For `meeting.kind`
/// "small talk" is confirmed by the turn publishing nothing — the same approximation step 4's
/// topic family made, from the other side; for `meeting.subject` a cached publish is its row
/// id, an uncached one `lookup`, nothing `skip`.
pub fn outcome(expected: &Expected, published: &[(Option<BriefId>, String)]) -> (OracleOutcome, Vec<(LogId, decision::Outcome)>) {
    let mut reports = Vec::new();
    let mut agreed = true;
    let cached = published.iter().find_map(|(row, _)| row.map(|r| r.to_string()));
    for (id, family, label) in &expected.rows {
        let actual = match family.as_str() {
            KIND => published.is_empty().to_string(),
            SUBJECT => cached.clone().unwrap_or_else(|| if published.is_empty() { "skip" } else { "lookup" }.to_owned()),
            _ => cached.clone().unwrap_or_else(|| decision::engine::NONE_ID.to_owned()),
        };
        let o = if *label == actual {
            decision::Outcome::Confirmed
        } else {
            agreed = false;
            decision::Outcome::Corrected { actual }
        };
        reports.push((*id, o));
    }
    (
        OracleOutcome {
            agreed,
            published: published.first().map(|(_, t)| t.clone()),
        },
        reports,
    )
}

/// What the actor does with a batch once the oracle (and tier 2) spoke.
#[derive(Debug)]
pub enum Action {
    /// No model turn; carry the batch into the next delivered one.
    Skip,
    /// Publish the cached brief as an `Oracle` topic; carry the batch.
    Publish {
        /// The row.
        row: Box<CachedBrief>,
        /// The label (its first alias).
        label: String,
    },
    /// Today's path: deliver the batch, with the evidence line when tier 1/2 ran and escalated.
    Model {
        /// Written right after the batch header, before the transcript rows.
        evidence: Option<String>,
    },
}

/// `tell` from [`run`]: the oracle's verdict on one batch.
#[derive(Debug)]
pub struct Verdict {
    /// The batch itself, to deliver or carry.
    pub batch: Vec<Utterance>,
    /// What to do.
    pub action: Action,
    /// The event already published (`None` when the oracle was off); kept for the outcome update.
    pub event: Option<OracleDecision>,
    /// What to compare the model turn against (shadow, or a tier-3 turn).
    pub expected: Option<Expected>,
    /// The training example written for this batch (57 §11.4), when `meeting.train` is on —
    /// the key for the outcome and tap/dismiss amend rows.
    pub example: Option<ExampleId>,
}

/// 57 §11.4: append an `amend` row for a captured example — the turn's outcome or the user's
/// reaction — off the caller's mailbox. The engine drops it when capture was flipped off.
pub fn amend(id: ExampleId, what: Amendment) {
    tokio::spawn(async move {
        if let Ok(Some(engine)) = ActorRef::<decision::Engine>::lookup(decision::DECISION_ACTOR) {
            let _ = engine
                .tell(decision::Capture {
                    prefix: PREFIX.to_owned(),
                    row: training::Row::Amend(training::Amend::new(id, what)),
                })
                .await;
        }
    });
}

/// Sends a `SideCall` to the `agent` plugin through the registry (57 §2 / 3b's snippet).
pub fn side_caller() -> Arc<decision::SideCaller> {
    Arc::new(|call| {
        Box::pin(async move {
            let agents = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
                .ok()
                .flatten()
                .ok_or_else(|| "agent is not running".to_owned())?;
            agents.ask(call).await.map_err(|e| e.to_string())
        })
    })
}

/// One ask on the decision actor; `None` when it is not running.
async fn ask(engine: &ActorRef<decision::Engine>, ask: Ask) -> Option<Decision> {
    engine
        .ask(ask)
        .await
        .map_err(|e| tracing::debug!("meeting: decision ask: {e}"))
        .ok()
}

/// 57 §3.5 item 5: when a meeting reaches `Recording`, load the model and run one throwaway
/// forward so batch 1 never times out on the cold load. Nothing when the oracle is off.
pub async fn warm_up() {
    let Ok(Some(engine)) = ActorRef::<decision::Engine>::lookup(decision::DECISION_ACTOR) else {
        return;
    };
    let Ok(info) = engine
        .ask(decision::GetMode {
            prefix: PREFIX.to_owned(),
        })
        .await
    else {
        return;
    };
    if info.effective == Mode::Off {
        return;
    }
    match engine
        .ask(decision::Warm {
            budget: Duration::from_secs(30),
        })
        .await
    {
        Ok(ms) => tracing::info!("meeting: decision oracle warm in {ms} ms"),
        Err(e) => tracing::info!("meeting: decision oracle warm-up: {e}"),
    }
}

/// The candidate id of the `i`th FTS row: `b1..b4` — short, so the 192-token head budget goes
/// to titles, not UUIDs (the phrasing bench: UUID ids never pick a brief).
fn candidate_id(i: usize) -> String {
    format!("b{}", i + 1)
}

/// The FTS row behind a candidate id; `None` for `none` or anything not in our table.
fn row_for<'a>(rows: &'a [CachedBrief], cid: &str) -> Option<&'a CachedBrief> {
    rows.iter().enumerate().find(|(i, _)| candidate_id(*i) == cid).map(|(_, r)| r)
}

/// The publish action for a candidate id, or `None` when the id is not in our table.
fn publish(rows: &[CachedBrief], cid: &str) -> Option<(OracleDisposition, Action)> {
    let row = row_for(rows, cid)?.clone();
    Some((
        OracleDisposition::Brief {
            brief: row.id,
            title: row.title.clone(),
        },
        Action::Publish {
            label: row.aliases.first().cloned().unwrap_or_else(|| row.title.clone()),
            row: Box::new(row),
        },
    ))
}

/// The whole pre-filter for one batch, off the mailbox: mode → mentions → the two asks →
/// [`settle`] → tier 2 (subject extraction through `side`, in shadow too — Sky wants the whole
/// pipeline evaluated) → the [`Verdict`]. `blocked` are the cache rows already shown or
/// dismissed this meeting (never offered again, like the alias scan).
#[allow(clippy::too_many_arguments)]
pub async fn run(
    rt: Runtime,
    cache: Option<Arc<Cache>>,
    blocked: Vec<BriefId>,
    batch: Vec<Utterance>,
    meeting: MeetingId,
    session: SessionId,
    side: Arc<decision::SideCaller>,
) -> Verdict {
    let today = |batch| Verdict {
        batch,
        action: Action::Model { evidence: None },
        event: None,
        expected: None,
        example: None,
    };
    let Ok(Some(engine)) = ActorRef::<decision::Engine>::lookup(decision::DECISION_ACTOR) else {
        return today(batch);
    };
    let Ok(info) = engine
        .ask(decision::GetMode {
            prefix: PREFIX.to_owned(),
        })
        .await
    else {
        return today(batch);
    };
    if info.effective == Mode::Off {
        return today(batch);
    }
    // 57 §11.4: capture the batch as a training example when `meeting.train` is on.
    let train = engine
        .ask(decision::GetTrain {
            prefix: PREFIX.to_owned(),
        })
        .await
        .is_ok_and(|i| i.enabled);
    let id = batch_id(&batch);
    rt.publish(MeetingEvent::OracleDeciding {
        meeting,
        batch_id: id.clone(),
    })
    .await;
    let t = Instant::now();
    let text = state(&batch);
    let mentions = mentions(&text);
    let budget = Duration::from_millis(info.budget_ms);

    // Candidates: FTS over mentions ∪ text, rows not shown/dismissed this meeting, top 4.
    let mut rows: Vec<CachedBrief> = Vec::new();
    if let Some(c) = &cache {
        let query = format!("{} {text}", mentions.join(" "));
        match c.search(&query, (CANDIDATES * 3) as u32).await {
            Ok(hits) => {
                rows = hits
                    .into_iter()
                    .filter(|r| !blocked.contains(&r.id))
                    .take(CANDIDATES)
                    .collect();
            }
            Err(e) => tracing::debug!("meeting: oracle brief_search: {e:#}"),
        }
    }
    let titles: HashMap<String, String> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| (candidate_id(i), r.title.clone()))
        .collect();

    let kind_ask = Ask {
        family: KIND.into(),
        question: Question::Bool {
            statement: KIND_STATEMENT.into(),
            state: text.clone(),
        },
        budget,
    };
    let brief_ask = (!rows.is_empty()).then(|| Ask {
        family: BRIEF.into(),
        question: Question::Choice {
            instructions: BRIEF_INSTRUCTIONS.into(),
            candidates: rows
                .iter()
                .enumerate()
                .map(|(i, r)| decision::Candidate {
                    id: candidate_id(i),
                    text: r.title.clone(),
                })
                .collect(),
            state: text.clone(),
        },
        budget,
    });
    // The questions as asked, kept for the training example (`Ask` is consumed by the engine).
    let kind_q = train.then(|| kind_ask.question.clone());
    let brief_q = train.then(|| brief_ask.as_ref().map(|a| a.question.clone())).flatten();
    let Some(kind) = ask(&engine, kind_ask).await else {
        return today(batch);
    };
    // The engine's disposition names the mode it actually ran in.
    let mode = match &kind.disposition {
        Disposition::Shadow(_) => Mode::Shadow,
        Disposition::Escalate(Reason::Disabled | Reason::ModelMissing) => return today(batch),
        _ => Mode::Active,
    };
    let brief = match brief_ask {
        Some(a) => ask(&engine, a).await,
        None => None,
    };

    let settled = settle(&kind, brief.as_ref(), &mentions);
    let mut subject = None;
    let mut tier2 = None;
    let mut tier2_result: Option<training::Tier2> = None;
    let mut subject_row: Option<(LogId, String)> = None;
    let (family, disposition, action) = match &settled {
        Settled::Skip => (KIND, OracleDisposition::Skip, Action::Skip),
        // The id came from our own table; a miss is a bug, not a pick.
        Settled::Brief(cid) => match publish(&rows, cid) {
            Some((d, a)) => (BRIEF, d, a),
            None => (
                BRIEF,
                OracleDisposition::Model {
                    reason: "unknown brief id".into(),
                },
                Action::Model { evidence: None },
            ),
        },
        Settled::Tier2(reason) => {
            let nums = numbers(&kind, brief.as_ref(), &titles);
            let candidates: Vec<decision::subject::Candidate> = rows
                .iter()
                .enumerate()
                .map(|(i, r)| decision::subject::Candidate {
                    id: candidate_id(i),
                    title: r.title.clone(),
                    aliases: r.aliases.clone(),
                })
                .collect();
            let req = decision::subject::Request {
                text: &text,
                mentions: &mentions,
                candidates: &candidates,
                numbers: &nums,
            };
            let escalate_budget = Duration::from_millis(info.escalate_budget_ms);
            match decision::subject::extract(&req, session, escalate_budget, &*side).await {
                Ok((answer, log_id)) => {
                    subject = answer.subject.clone();
                    tier2_result = Some(training::Tier2::Answer {
                        subject: answer.subject.clone(),
                        cached: answer.cached.clone(),
                        lookup: answer.lookup,
                    });
                    let label = match &answer.cached {
                        Some(cid) => row_for(&rows, cid).map_or_else(|| cid.clone(), |r| r.id.to_string()),
                        None => answer.choice(),
                    };
                    subject_row = Some((log_id, label));
                    match (&answer.cached, &answer.subject, answer.lookup) {
                        (Some(cid), _, _) => match publish(&rows, cid) {
                            Some((d, a)) => {
                                tier2 = Some(format!("cached: {}", titles.get(cid).map_or(cid.as_str(), String::as_str)));
                                (SUBJECT, d, a)
                            }
                            None => {
                                tier2 = Some(format!("unknown id {cid}"));
                                (
                                    SUBJECT,
                                    OracleDisposition::Model {
                                        reason: format!("{reason}; tier 2: unknown id"),
                                    },
                                    Action::Model {
                                        evidence: Some(evidence(&kind, brief.as_ref(), &titles, reason)),
                                    },
                                )
                            }
                        },
                        (None, Some(s), true) => {
                            tier2 = Some(format!("{s:?} → no cached brief"));
                            (
                                SUBJECT,
                                OracleDisposition::Model {
                                    reason: format!("subject {s:?}, no cached brief"),
                                },
                                Action::Model {
                                    evidence: Some(subject_evidence(s, &nums)),
                                },
                            )
                        }
                        (None, Some(s), false) => {
                            tier2 = Some(format!("{s:?} → nothing to look up"));
                            (SUBJECT, OracleDisposition::Skip, Action::Skip)
                        }
                        (None, None, _) => {
                            tier2 = Some("nothing to look up".into());
                            (SUBJECT, OracleDisposition::Skip, Action::Skip)
                        }
                    }
                }
                Err(e) => {
                    tier2 = Some(e.to_string());
                    tier2_result = Some(training::Tier2::Error { error: e.to_string() });
                    let why = format!("{reason}; tier 2: {e}");
                    (
                        KIND,
                        OracleDisposition::Model { reason: why.clone() },
                        Action::Model {
                            evidence: Some(evidence(&kind, brief.as_ref(), &titles, &why)),
                        },
                    )
                }
            }
        }
    };
    // Expected labels for the outcome: the brief's under its cache row id, not `b1`.
    let expected = Expected {
        rows: std::iter::once((kind.log_id, KIND.to_owned(), kind.choice.clone()))
            .chain(brief.iter().map(|b| {
                let label = row_for(&rows, &b.choice)
                    .map_or_else(|| decision::engine::NONE_ID.to_owned(), |r| r.id.to_string());
                (b.log_id, BRIEF.to_owned(), label)
            }))
            .chain(subject_row.into_iter().map(|(log_id, label)| (log_id, SUBJECT.to_owned(), label)))
            .collect(),
    };
    // 57 §11.4: the example — every question with its titles and numbers, tier 2, the verdict
    // (Skip and Publish included: a skipped small-talk batch is a negative example). Written by
    // the engine, which drops it if capture was flipped off meanwhile.
    let example = kind_q.map(|kq| {
        let mut questions = vec![training::Asked::new(&kq, &kind)];
        if let (Some(bq), Some(b)) = (&brief_q, &brief) {
            questions.push(training::Asked::new(bq, b));
        }
        let verdict = match &disposition {
            OracleDisposition::Skip => training::Verdict::Skip,
            OracleDisposition::Brief { brief, title } => training::Verdict::Publish {
                id: brief.to_string(),
                title: title.clone(),
            },
            OracleDisposition::Model { reason } => training::Verdict::Model { reason: reason.clone() },
        };
        training::Example::new(PREFIX, mode, text, mentions.clone(), questions, tier2_result.take(), verdict)
    });
    let event = OracleDecision {
        batch_id: id,
        family: family.to_owned(),
        mode,
        disposition,
        stages: stages(&kind, brief.as_ref(), &titles),
        mentions,
        subject,
        tier: match (&action, tier2.is_some()) {
            (Action::Model { .. }, _) => 3,
            (_, true) => 2,
            (_, false) => 1,
        },
        tier2,
        top: top(brief.as_ref(), &titles),
        latency_ms: t.elapsed().as_millis() as u64,
        outcome: None,
    };
    rt.publish(MeetingEvent::OracleDecision {
        meeting,
        decision: event.clone(),
    })
    .await;
    let example = match example {
        Some(e) => {
            let id = e.id;
            let _ = engine
                .tell(decision::Capture {
                    prefix: PREFIX.to_owned(),
                    row: training::Row::Example(Box::new(e)),
                })
                .await;
            Some(id)
        }
        None => None,
    };
    // Shadow: today's path exactly — no evidence line, so the outcome is an independent label.
    let action = match (mode, action) {
        (Mode::Shadow, _) => Action::Model { evidence: None },
        (_, a) => a,
    };
    let expected = matches!(action, Action::Model { .. }).then_some(expected);
    Verdict {
        batch,
        action,
        event: Some(event),
        expected,
        example,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(family: &str, choice: &str, probs: &[(&str, f32)], disp: Disposition) -> Decision {
        Decision {
            family: family.into(),
            choice: choice.into(),
            probabilities: probs.iter().map(|(l, p)| ((*l).to_owned(), *p)).collect(),
            confidence: 0.5,
            act_probability: 0.0,
            disposition: disp,
            latency: Duration::from_millis(80),
            gate_version: 1,
            log_id: LogId::new(),
            tier: 1,
            calibrated: true,
        }
    }

    fn kind(p_small: f32, disp: Disposition) -> Decision {
        let choice = if p_small >= 0.5 { "true" } else { "false" };
        d(KIND, choice, &[("false", 1.0 - p_small), ("true", p_small)], disp)
    }

    const KUIPER: &str = "them: So Mike was telling me about this at lunch. They were looking at the orbits of a few distant objects in the Kuiper belt, way out past Neptune, and the orbits were all clustered in a weird way.\n\
them: Constantin said the simplest explanation was a hidden planet, something maybe five or ten times the mass of Earth, and they started calling it Planet Nine.\n\
me: Has anyone actually seen it?\n\
them: No, that's the thing. It's all inferred from the clustering.";

    #[test]
    fn mentions_find_names_tickets_acronyms_units_and_quotes() {
        let m = mentions(KUIPER);
        assert!(m.iter().any(|x| x == "Mike"), "{m:?}");
        assert!(m.iter().any(|x| x == "Kuiper"), "{m:?}");
        assert!(m.iter().any(|x| x == "Neptune"), "{m:?}");
        assert!(m.iter().any(|x| x == "Planet Nine"), "{m:?}");
        assert!(m.iter().any(|x| x == "Earth"), "{m:?}");
        // Sentence-initial single words are not mentions (`So`, `They`, `Has`, `No`, `It's`).
        assert!(!m.iter().any(|x| x == "So" || x == "They" || x == "Has" || x == "No" || x == "It"), "{m:?}");

        let m = mentions("them: the deploy is blocked on SRE-2386, the image tag pipeline for DR\nme: check the 33 kV feeder and the \"golden image\" in 10min, it's 600 seconds by default");
        assert_eq!(m, vec!["SRE-2386", "DR", "golden image", "33 kV", "10min", "600 seconds"], "{m:?}");
        // Dates and ordinals are not units; `I` is not a name.
        let m = mentions("them: on the 15th of October I think 2 of them left");
        assert!(m.is_empty(), "{m:?}");
        // Capitalised runs count anywhere; a lone sentence-initial capital does not.
        assert_eq!(mentions("me: Planet Nine is faint. Okay."), vec!["Planet Nine"]);
        // Case-insensitive dedupe, cap.
        let many: String = (0..30).map(|i| format!("Name{i}, ")).collect();
        assert_eq!(mentions(&format!("them: about {many} and dr DR")).len(), MAX_MENTIONS);
        assert_eq!(mentions("them: the DR test, then dr again and DR").len(), 1);
    }

    #[test]
    fn mentions_are_empty_on_small_talk() {
        for text in [
            "them: yeah, sounds good\nme: ok, thanks, talk later",
            "them: Morning everyone, can you hear me okay?\nme: Yep, loud and clear.",
            "them: Can we push this to tomorrow afternoon?\nme: Sure.",
        ] {
            assert!(mentions(text).is_empty(), "{text:?} → {:?}", mentions(text));
        }
    }

    #[test]
    fn settle_is_asymmetric() {
        let accept = Disposition::Accept;
        let low = Disposition::Escalate(Reason::LowMargin);
        let none: Vec<String> = Vec::new();
        let some = vec!["Kuiper belt".to_owned()];
        // Small talk accepted + no mentions → skip; the same with mentions → tier 2.
        assert_eq!(settle(&kind(0.79, accept.clone()), None, &none), Settled::Skip);
        assert_eq!(
            settle(&kind(0.79, accept.clone()), None, &some),
            Settled::Tier2("small talk but mentions: Kuiper belt".into())
        );
        // Not small talk (false accepted) → tier 2, brief `none` or not.
        let b_none = d(BRIEF, "none", &[("b1", 0.1), ("none", 0.9)], accept.clone());
        assert_eq!(settle(&kind(0.12, accept.clone()), Some(&b_none), &none), Settled::Tier2("not small talk".into()));
        assert_eq!(settle(&kind(0.12, accept.clone()), None, &none), Settled::Tier2("not small talk".into()));
        // A brief accepted wins over everything, even small talk with no mentions.
        let b_a = d(BRIEF, "b1", &[("b1", 0.91), ("none", 0.09)], accept.clone());
        assert_eq!(settle(&kind(0.79, accept.clone()), Some(&b_a), &none), Settled::Brief("b1".into()));
        assert_eq!(settle(&kind(0.12, low.clone()), Some(&b_a), &some), Settled::Brief("b1".into()));
        // A brief with P 0.65 but margin 0.2 is escalated by the engine's gate → tier 2.
        let b_low = d(BRIEF, "b1", &[("b1", 0.65), ("b2", 0.45), ("none", 0.0)], low.clone());
        assert_eq!(settle(&kind(0.12, accept.clone()), Some(&b_low), &none), Settled::Tier2("not small talk".into()));
        // The kind question abstaining or timing out → tier 2 with the reason.
        assert_eq!(settle(&kind(0.55, low), None, &none), Settled::Tier2("kind low margin".into()));
        assert_eq!(
            settle(&d(KIND, "none", &[], Disposition::Escalate(Reason::Timeout)), None, &none),
            Settled::Tier2("kind timeout".into())
        );
        // Shadow wrappers are looked through.
        assert_eq!(settle(&kind(0.8, accept.clone()).shadowed(), None, &none), Settled::Skip);
        assert_eq!(settle(&kind(0.2, accept).shadowed(), Some(&b_a.shadowed()), &none), Settled::Brief("b1".into()));
    }

    #[test]
    fn stages_evidence_and_top() {
        let titles: HashMap<String, String> = [("b1".to_owned(), "SRE-2386".to_owned())].into();
        let k = kind(0.12, Disposition::Accept);
        let b = d(BRIEF, "none", &[("b1", 0.31), ("none", 0.55)], Disposition::Escalate(Reason::LowMargin));
        assert_eq!(
            stages(&k, Some(&b), &titles),
            vec![("kind: subject".to_owned(), 0.88), ("brief: none".to_owned(), 0.55)]
        );
        assert_eq!(stages(&kind(0.79, Disposition::Accept), None, &titles), vec![("kind: small talk".to_owned(), 0.79)]);
        assert_eq!(
            evidence(&k, Some(&b), &titles, "not small talk"),
            "oracle: kind subject 0.88, brief none 0.55 / SRE-2386 0.31 (escalated: not small talk)"
        );
        assert_eq!(
            evidence(&d(KIND, "none", &[], Disposition::Escalate(Reason::Timeout)), None, &titles, "kind timeout"),
            "oracle: kind ? (escalated: kind timeout)"
        );
        assert_eq!(top(Some(&b), &titles), vec![("SRE-2386".to_owned(), 0.31), ("none".to_owned(), 0.55)]);
        assert!(top(None, &titles).is_empty());
    }

    #[test]
    fn outcome_compares_each_family_with_what_the_turn_did() {
        let a = BriefId::new();
        let b = BriefId::new();
        let (k_id, b_id) = (LogId::new(), LogId::new());
        // Small talk (`true`) + brief `none`: confirmed by a turn that published nothing.
        let exp = Expected {
            rows: vec![(k_id, KIND.into(), "true".into()), (b_id, BRIEF.into(), "none".into())],
        };
        let (o, r) = outcome(&exp, &[]);
        assert_eq!(o, OracleOutcome { agreed: true, published: None });
        assert_eq!(r, vec![(k_id, decision::Outcome::Confirmed), (b_id, decision::Outcome::Confirmed)]);
        let (o, r) = outcome(&exp, &[(Some(a), "HR-12".into())]);
        assert_eq!(o, OracleOutcome { agreed: false, published: Some("HR-12".into()) });
        assert_eq!(r[0].1, decision::Outcome::Corrected { actual: "false".into() });
        assert_eq!(r[1].1, decision::Outcome::Corrected { actual: a.to_string() });
        // The oracle said brief=a; the model published a → agreed; published b → corrected.
        let exp = Expected {
            rows: vec![(k_id, KIND.into(), "false".into()), (b_id, BRIEF.into(), a.to_string())],
        };
        assert!(outcome(&exp, &[(Some(a), "x".into())]).0.agreed);
        let (o, r) = outcome(&exp, &[(Some(b), "y".into())]);
        assert!(!o.agreed);
        assert_eq!(r, vec![(k_id, decision::Outcome::Confirmed), (b_id, decision::Outcome::Corrected { actual: b.to_string() })]);
        // A new (uncached) brief counts as "something" for the kind and as no cached pick.
        let (o, _) = outcome(&exp, &[(None, "new".into())]);
        assert_eq!(o.published.as_deref(), Some("new"));
        assert!(!o.agreed);
        // The tier-2 subject row: `lookup` is confirmed by a new brief, a row id by that cached
        // publish, `skip` by nothing.
        let s_id = LogId::new();
        let exp = Expected {
            rows: vec![(s_id, SUBJECT.into(), "lookup".into())],
        };
        assert_eq!(outcome(&exp, &[(None, "new".into())]).1, vec![(s_id, decision::Outcome::Confirmed)]);
        assert_eq!(outcome(&exp, &[(Some(a), "x".into())]).1, vec![(s_id, decision::Outcome::Corrected { actual: a.to_string() })]);
        assert_eq!(outcome(&exp, &[]).1, vec![(s_id, decision::Outcome::Corrected { actual: "skip".into() })]);
        let exp = Expected {
            rows: vec![(s_id, SUBJECT.into(), a.to_string())],
        };
        assert!(outcome(&exp, &[(Some(a), "x".into())]).0.agreed);
    }

    #[test]
    fn state_and_batch_id() {
        let batch = vec![
            Utterance { track: Track::System, t0_ms: 872_000, t1_ms: 873_000, text: " so the DR sync ".into(), request: false },
            Utterance { track: Track::Mic, t0_ms: 874_000, t1_ms: 875_000, text: "right".into(), request: false },
        ];
        assert_eq!(state(&batch), "them: so the DR sync\nme: right");
        assert_eq!(batch_id(&batch), "00:14:32");
    }
}
