//! The packet (101 DECISION-SURFACE §3, Revision 2): what an agent prepares for
//! the person who must decide (`decision:<id>`) or review (`review:<task>`): a
//! one-paragraph summary, a mini-board document with a fixed skeleton
//! ([`check_board`]), options, pointers with the evidence inline. Stored in
//! `tasks.sqlite` (`packets`, replicated), mirrored into the thread as markdown
//! ([`thread_markdown`]), required when the user is the audience
//! ([`require_for_user`]).

use serde::{Deserialize, Serialize};

use crate::types::TaskId;

/// `summary` cap, bytes.
pub const SUMMARY_MAX: usize = 16 * 1024;
/// `pointers` cap.
pub const POINTERS_MAX: usize = 12;
/// Each pointer's `text` cap, bytes.
pub const POINTER_TEXT_MAX: usize = 32 * 1024;
/// `options` cap.
pub const OPTIONS_MAX: usize = 6;
/// Option label cap, chars.
pub const LABEL_MAX: usize = 80;
/// Whole packet as JSON, bytes.
pub const PACKET_MAX: usize = 128 * 1024;
/// [`head`] cap, chars.
pub const HEAD_MAX: usize = 140;
/// `items` cap (103 F3).
pub const ITEMS_MAX: usize = 20;
/// `recommendation.text` cap, bytes (103 F2).
pub const RECOMMENDATION_MAX: usize = 1024;
/// `if_wrong` cap, chars (132 D2).
pub const IF_WRONG_MAX: usize = 200;
/// An inbox row's title budget, chars (132 D1): [`head_sentence`].
pub const ROW_BUDGET: usize = 112;
/// A review's `options` cap (132 D3): the next moves after approval.
pub const REVIEW_OPTIONS_MAX: usize = 4;

/// D1 error: an escalation to the user without a `summary`.
pub const NEEDS_SUMMARY: &str = "escalating to the user needs a `summary`: one short paragraph — the abstract the \
inbox row, the thread and the channel card show (the detail goes on the `board`)";
/// D1 error: a hand-over to the user's review without a `summary`.
pub const NEEDS_REVIEW_SUMMARY: &str = "handing work to the user's review needs a `summary`: one short paragraph — \
the abstract the inbox row and the thread show (the detail goes on the `board`)";
/// D1 (R1) error: an escalation to the user without a `board`.
pub const NEEDS_BOARD_DECISION: &str = "escalating to the user needs a `board`: a mini-board for the decider — \
`r = await doc_create(title=…, kind=\"board\", root=<absolute repo or scratch dir>)`, then \
`await doc_insert(doc=r[\"doc\"], markdown=…)` with the sections `## What you're deciding`, \
`## Why it matters to you`, `## The picture` (one diagram with `ref` anchors), `## Options` (one `###` per option: \
consequence, cost/risk, reversibility, recommendation), `## Look at` (Excerpts); then pass `board=r[\"doc\"]`. \
Short is good — one block per section is enough.";
/// D1 (R1) error: a hand-over to the user's review without a `board`.
pub const NEEDS_BOARD_REVIEW: &str = "handing work to the user's review needs a `board`: a mini-board for the \
reviewer — `r = await doc_create(title=…, kind=\"board\", root=<absolute repo root>)`, then \
`await doc_insert(doc=r[\"doc\"], markdown=…)` with the sections `## What was done`, `## Why it matters to you`, \
`## The picture` (one diagram with `ref` anchors), `## Against the criteria` (each `done_when` line with its \
evidence anchor), `## Walk through` (Excerpts in reading order); then pass `board=r[\"doc\"]`. Short is good — one \
block per section is enough.";
/// 132 D2 error: an escalation to the user without `if_wrong`.
pub const NEEDS_IF_WRONG: &str = "escalating to the user needs `if_wrong`: one honest line on the cost of a wrong \
answer — \"low, one-line revert\" or \"high, real users get this email\"";

/// R2: the `##` sections of a decision mini-board, in order (`Open questions` optional, unchecked).
pub const DECISION_SECTIONS: [&str; 5] = [
    "What you're deciding",
    "Why it matters to you",
    "The picture",
    "Options",
    "Look at",
];
/// R2: the `##` sections of a review mini-board, in order.
pub const REVIEW_SECTIONS: [&str; 5] = [
    "What was done",
    "Why it matters to you",
    "The picture",
    "Against the criteria",
    "Walk through",
];
/// How many leading `##` sections [`check_board`] matches against the skeleton.
const CHECKED_SECTIONS: usize = 3;

/// What the packet is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum PacketKind {
    /// An escalation: the question with its options.
    Decision,
    /// A hand-over to review: the claimed status and where to look.
    Review,
}

impl PacketKind {
    /// The serde name, also the `packets.kind` column value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Decision => "decision",
            Self::Review => "review",
        }
    }
}

/// What a [`Pointer`] points at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum PointerKind {
    /// An artifact by call id.
    Artifact,
    /// A note by document id.
    Note,
    /// A knowledge entry by id.
    Knowledge,
    /// A session by id.
    Session,
    /// A meeting by id.
    Meeting,
    /// A task by id.
    Task,
    /// An `http(s)` URL.
    Url,
    /// Lines of a file: `path#L10-L24`, the lines in `text`.
    Code,
    /// A unified diff in `text`.
    Diff,
}

/// One answer the decider can pick.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PacketOption {
    /// A few words.
    pub label: String,
    /// One line: the trade-off.
    pub description: Option<String>,
}

/// The agent's claimed status against one acceptance criterion.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Check {
    /// The criterion.
    pub text: String,
    /// Met.
    pub done: bool,
    /// How it was met, or why not.
    pub note: Option<String>,
}

/// Exactly what to look at, with the evidence inline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Pointer {
    /// What `ref` names.
    pub kind: PointerKind,
    /// artifact call_id | note DocId | knowledge id | session id | meeting id | task id | https URL |
    /// code `path#L10-L24` (path relative to `root`) | diff `<base>..<head>` or a free label
    #[serde(rename = "ref")]
    #[cfg_attr(feature = "ts", ts(rename = "ref"))]
    pub ref_: String,
    /// A few words.
    pub title: String,
    /// One line: what to look for here.
    pub note: Option<String>,
    /// Inline evidence: code = the lines verbatim; diff = a unified diff. ≤ 32 KiB. Required for code/diff.
    pub text: Option<String>,
    /// code/diff: absolute repo root on the engine that produced it.
    pub root: Option<String>,
}

/// 103 F2: the forwarding manager's view — one or two sentences and the
/// option it backs. The Decision screen pre-picks `option` over `recommended`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Recommendation {
    /// One or two sentences (≤ 1 KiB).
    pub text: String,
    /// Index into `options`.
    #[serde(default)]
    pub option: Option<u32>,
}

/// 103 F3: one sub-decision of a bundle. `id` is the slug of the `###`
/// heading under the board's Options section that describes it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Item {
    /// Unique within the packet; the board's `###` heading slug.
    pub id: String,
    /// A few words.
    pub title: String,
    /// One line.
    #[serde(default)]
    pub description: Option<String>,
    /// ≤ 6, labels ≤ 80 chars.
    #[serde(default)]
    pub options: Vec<PacketOption>,
    /// Index into `options`.
    #[serde(default)]
    pub recommended: Option<u32>,
}

/// The packet (101 §3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Packet {
    /// `decision:<uuid>` | `review:<task>`.
    pub key: String,
    /// The task.
    pub task: TaskId,
    /// Decision or review.
    pub kind: PacketKind,
    /// Actor: `agent:<name>` | `user`.
    pub by: String,
    /// ms since epoch.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub at_ms: u64,
    /// Decision: the question as asked.
    pub question: Option<String>,
    /// Markdown; ```mermaid / ```json-render / ```openui fences allowed. One paragraph when a `board` carries the detail.
    pub summary: String,
    /// R1: the board document (`doc_create(kind="board")`) written for the decider; required for the user.
    #[serde(default)]
    pub board: Option<String>,
    /// Decision only; may be empty (free-form question).
    pub options: Vec<PacketOption>,
    /// Index into `options`.
    pub recommended: Option<u32>,
    /// Where to look.
    pub pointers: Vec<Pointer>,
    /// Review: the agent's claimed status against `done_when`.
    pub checks: Vec<Check>,
    /// 103 F2: the forwarding manager's view.
    #[serde(default)]
    pub recommendation: Option<Recommendation>,
    /// 103 F3: a bundle — N related sub-decisions.
    #[serde(default)]
    pub items: Vec<Item>,
    /// 106: who the packet is addressed to — `user` | `agent:<name>` (the
    /// resolved manager for a decision, the effective reviewer for a review).
    /// `None` on rows written before 106.
    #[serde(default)]
    pub to: Option<String>,
    /// 132 D2: one honest line on the cost of a wrong answer (≤ 200 chars);
    /// required for a decision to the user, optional on a review, inherited on forward.
    #[serde(default)]
    pub if_wrong: Option<String>,
}

/// The `escalate` request as the ledger takes it (the packet before the key,
/// actor and time are known).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ask {
    /// The question.
    pub question: String,
    /// The summary; required when the user decides (D1).
    pub summary: Option<String>,
    /// The mini-board's document id; required when the user decides (R1).
    pub board: Option<String>,
    /// Options.
    pub options: Vec<PacketOption>,
    /// Index into `options`.
    pub recommended: Option<u32>,
    /// Where to look.
    pub pointers: Vec<Pointer>,
    /// The forwarding manager's view (103 F2).
    pub recommendation: Option<Recommendation>,
    /// A bundle (103 F3).
    pub items: Vec<Item>,
    /// The cost of a wrong answer (132 D2); required when the user decides.
    pub if_wrong: Option<String>,
}

impl Ask {
    /// A bare question.
    pub fn new(question: &str) -> Self {
        Self {
            question: question.to_owned(),
            ..Default::default()
        }
    }
}

/// The review key of `task`.
pub fn review_key(task: &TaskId) -> String {
    format!("review:{task}")
}

/// The caps and ref rules of 101 §3; the error names the field and the cap.
pub fn validate(p: &Packet) -> Result<(), String> {
    if p.summary.len() > SUMMARY_MAX {
        return Err(format!(
            "`summary` is {} bytes; the cap is {SUMMARY_MAX} bytes (16 KiB)",
            p.summary.len()
        ));
    }
    if let Some(w) = &p.if_wrong {
        let n = w.trim().chars().count();
        if n > IF_WRONG_MAX {
            return Err(format!(
                "`if_wrong` is {n} chars; the cap is {IF_WRONG_MAX} — one honest line on the cost of a wrong answer"
            ));
        }
    }
    if p.kind == PacketKind::Review && p.options.len() > REVIEW_OPTIONS_MAX {
        return Err(format!(
            "`options` has {} entries; a review offers at most {REVIEW_OPTIONS_MAX} next moves",
            p.options.len()
        ));
    }
    validate_options("", &p.options, p.recommended)?;
    if let Some(r) = &p.recommendation {
        if r.text.len() > RECOMMENDATION_MAX {
            return Err(format!(
                "`recommendation.text` is {} bytes; the cap is {RECOMMENDATION_MAX} bytes (1 KiB)",
                r.text.len()
            ));
        }
        if let Some(o) = r.option
            && o as usize >= p.options.len()
        {
            return Err(format!(
                "`recommendation.option` is {o} but `options` has {} entries",
                p.options.len()
            ));
        }
    }
    if p.items.len() > ITEMS_MAX {
        return Err(format!(
            "`items` has {} entries; the cap is {ITEMS_MAX}",
            p.items.len()
        ));
    }
    for (i, it) in p.items.iter().enumerate() {
        if it.id.trim().is_empty() {
            return Err(format!("`items[{i}].id` is empty"));
        }
        if p.items[..i].iter().any(|o| o.id == it.id) {
            return Err(format!("`items[{i}].id` `{}` is used twice", it.id));
        }
        validate_options(&format!("items[{i}]."), &it.options, it.recommended)?;
    }
    if p.pointers.len() > POINTERS_MAX {
        return Err(format!(
            "`pointers` has {} entries; the cap is {POINTERS_MAX}",
            p.pointers.len()
        ));
    }
    for (i, ptr) in p.pointers.iter().enumerate() {
        validate_pointer(i, ptr)?;
    }
    let json = serde_json::to_vec(p).map_err(|e| format!("packet: {e}"))?;
    if json.len() > PACKET_MAX {
        return Err(format!(
            "the packet is {} bytes as JSON; the cap is {PACKET_MAX} bytes (128 KiB)",
            json.len()
        ));
    }
    Ok(())
}

/// `options` ≤ 6, labels non-empty and ≤ 80 chars, `recommended` in range;
/// `at` prefixes the field name (`""` | `items[3].`).
fn validate_options(
    at: &str,
    options: &[PacketOption],
    recommended: Option<u32>,
) -> Result<(), String> {
    if options.len() > OPTIONS_MAX {
        return Err(format!(
            "`{at}options` has {} entries; the cap is {OPTIONS_MAX}",
            options.len()
        ));
    }
    for (i, o) in options.iter().enumerate() {
        let n = o.label.chars().count();
        if n > LABEL_MAX {
            return Err(format!(
                "`{at}options[{i}].label` is {n} chars; the cap is {LABEL_MAX}"
            ));
        }
        if o.label.trim().is_empty() {
            return Err(format!("`{at}options[{i}].label` is empty"));
        }
    }
    if let Some(r) = recommended
        && r as usize >= options.len()
    {
        return Err(format!(
            "`{at}recommended` is {r} but `{at}options` has {} entries",
            options.len()
        ));
    }
    Ok(())
}

fn validate_pointer(i: usize, p: &Pointer) -> Result<(), String> {
    if p.ref_.trim().is_empty() {
        return Err(format!("`pointers[{i}].ref` is empty"));
    }
    if let Some(t) = &p.text
        && t.len() > POINTER_TEXT_MAX
    {
        return Err(format!(
            "`pointers[{i}].text` is {} bytes; the cap is {POINTER_TEXT_MAX} bytes (32 KiB)",
            t.len()
        ));
    }
    match p.kind {
        PointerKind::Url => {
            if !(p.ref_.starts_with("http://") || p.ref_.starts_with("https://")) {
                return Err(format!(
                    "`pointers[{i}].ref` `{}`: a `url` ref starts with `http://` or `https://`",
                    p.ref_
                ));
            }
        }
        PointerKind::Code => {
            if code_ref(&p.ref_).is_none() {
                let hint = colon_form(&p.ref_).map_or_else(String::new, |(path, line)| {
                    format!(" — write `{path}#L{line}`")
                });
                return Err(format!(
                    "`pointers[{i}].ref` `{}`: a `code` ref is `path#L<line>` or `path#L<from>-L<to>`{hint}",
                    p.ref_
                ));
            }
            if p.text.as_deref().is_none_or(str::is_empty) {
                return Err(format!(
                    "`pointers[{i}].text` is required for a `code` pointer: the lines of `{}` verbatim",
                    p.ref_
                ));
            }
        }
        PointerKind::Diff if p.text.as_deref().is_none_or(str::is_empty) => {
            return Err(format!(
                "`pointers[{i}].text` is required for a `diff` pointer: the unified diff"
            ));
        }
        _ => {}
    }
    Ok(())
}

/// `path#L<a>` | `path#L<a>-L<b>` → `(path, a, b)`; `None` when it is neither.
pub fn code_ref(r: &str) -> Option<(&str, u32, u32)> {
    let (path, frag) = r.rsplit_once('#')?;
    if path.is_empty() {
        return None;
    }
    let frag = frag.strip_prefix('L')?;
    let (a, b) = match frag.split_once("-L") {
        Some((a, b)) => (a.parse().ok()?, b.parse().ok()?),
        None => {
            let a = frag.parse().ok()?;
            (a, a)
        }
    };
    (a >= 1 && b >= a).then_some((path, a, b))
}

/// `path:12` | `path:12-24` → `(path, "12" | "12-L24")`, the hint for a colon-form code ref.
fn colon_form(r: &str) -> Option<(&str, String)> {
    let (path, rest) = r.rsplit_once(':')?;
    if path.is_empty() || rest.is_empty() {
        return None;
    }
    let (a, b) = rest.split_once('-').unwrap_or((rest, ""));
    if a.is_empty() || !a.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    if b.is_empty() {
        return Some((path, a.to_owned()));
    }
    b.bytes()
        .all(|c| c.is_ascii_digit())
        .then(|| (path, format!("{a}-L{b}")))
}

/// D1 (R1): with the user as the audience a packet needs a `summary` and a
/// `board`, for both kinds; a decision also needs `if_wrong` (132 D2). Call
/// only when the audience is the user.
pub fn require_for_user(p: &Packet) -> Result<(), String> {
    let board = p.board.as_deref().is_some_and(|b| !b.trim().is_empty());
    let summary = !p.summary.trim().is_empty();
    let if_wrong = p.if_wrong.as_deref().is_some_and(|w| !w.trim().is_empty());
    match (p.kind, board, summary) {
        (PacketKind::Decision, false, _) => Err(NEEDS_BOARD_DECISION.into()),
        (PacketKind::Review, false, _) => Err(NEEDS_BOARD_REVIEW.into()),
        (PacketKind::Decision, true, false) => Err(NEEDS_SUMMARY.into()),
        (PacketKind::Review, true, false) => Err(NEEDS_REVIEW_SUMMARY.into()),
        (PacketKind::Decision, true, true) if !if_wrong => Err(NEEDS_IF_WRONG.into()),
        (_, true, true) => Ok(()),
    }
}

/// R2: the first three `##` headings of a board (`outline` = its top-level
/// `(level, text)` headings in order) match the skeleton of `kind`, compared
/// case- and whitespace-insensitively; `###` and later `##` sections are free.
/// The error names the missing or misplaced section and the full skeleton.
pub fn check_board(kind: PacketKind, outline: &[(u8, String)]) -> Result<(), String> {
    let want = match kind {
        PacketKind::Decision => &DECISION_SECTIONS,
        PacketKind::Review => &REVIEW_SECTIONS,
    };
    let mut h2 = outline.iter().filter(|(l, _)| *l == 2).map(|(_, t)| t);
    for (i, section) in want.iter().take(CHECKED_SECTIONS).enumerate() {
        let skeleton = || {
            want.iter()
                .map(|s| format!("`## {s}`"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        match h2.next() {
            Some(got) if fold(got) == fold(section) => {}
            Some(got) => {
                return Err(format!(
                    "section {} of a {} board must be `## {section}`, found `## {}`; the sections are {}",
                    i + 1,
                    kind.as_str(),
                    got.trim(),
                    skeleton()
                ));
            }
            None => {
                return Err(format!(
                    "section {} of a {} board is missing: `## {section}`; the sections are {}",
                    i + 1,
                    kind.as_str(),
                    skeleton()
                ));
            }
        }
    }
    Ok(())
}

/// Heading text for comparison: lowercase, straight apostrophes, single spaces.
fn fold(s: &str) -> String {
    s.replace(['’', '‘'], "'")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// D3: the packet as the thread block (`**decision requested**` /
/// `**review requested**`): recommendation, question, summary, the board's
/// link, options, items, checks, "Look at:" with each pointer's title, ref
/// and note. The evidence (`text`) stays in the packet.
pub fn thread_markdown(p: &Packet) -> String {
    let mut s = match p.kind {
        PacketKind::Decision => format!("**decision requested** (`{}`)", p.key),
        PacketKind::Review => "**review requested**".to_owned(),
    };
    if let Some(r) = &p.recommendation {
        s.push_str("\n\nRecommendation: ");
        if let Some(o) = r.option.and_then(|o| p.options.get(o as usize)) {
            s.push_str(&o.label);
            s.push_str(" — ");
        }
        s.push_str(r.text.trim());
    }
    if let Some(q) = p
        .question
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty())
    {
        s.push_str("\n\n");
        s.push_str(q);
    }
    let summary = p.summary.trim();
    if !summary.is_empty() {
        s.push_str("\n\n");
        s.push_str(summary);
    }
    if let Some(b) = p.board.as_deref().map(str::trim).filter(|b| !b.is_empty()) {
        s.push_str(&format!("\n\nBoard: theuth://notes/{b}"));
    }
    if let Some(w) = p
        .if_wrong
        .as_deref()
        .map(str::trim)
        .filter(|w| !w.is_empty())
    {
        s.push_str("\n\nIf wrong: ");
        s.push_str(w);
    }
    if !p.options.is_empty() {
        s.push_str("\n\nOptions:\n");
        for (i, o) in p.options.iter().enumerate() {
            s.push_str("- ");
            s.push_str(&o.label);
            if p.recommended == Some(i as u32) {
                s.push_str(" (recommended)");
            }
            if let Some(d) = o
                .description
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty())
            {
                s.push_str(" — ");
                s.push_str(d);
            }
            s.push('\n');
        }
    }
    if !p.items.is_empty() {
        s.push_str("\n\nItems:\n");
        for (n, it) in p.items.iter().enumerate() {
            s.push_str(&format!("{}. {}", n + 1, it.title));
            if let Some(d) = it
                .description
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty())
            {
                s.push_str(" — ");
                s.push_str(d);
            }
            if !it.options.is_empty() {
                s.push_str(": ");
                let labels: Vec<String> = it
                    .options
                    .iter()
                    .enumerate()
                    .map(|(i, o)| {
                        if it.recommended == Some(i as u32) {
                            format!("{} (recommended)", o.label)
                        } else {
                            o.label.clone()
                        }
                    })
                    .collect();
                s.push_str(&labels.join(" | "));
            }
            s.push('\n');
        }
    }
    if !p.checks.is_empty() {
        s.push_str("\n\nChecks:\n");
        for c in &p.checks {
            s.push_str(if c.done { "- [x] " } else { "- [ ] " });
            s.push_str(&c.text);
            if let Some(n) = c.note.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
                s.push_str(" — ");
                s.push_str(n);
            }
            s.push('\n');
        }
    }
    if !p.pointers.is_empty() {
        s.push_str("\n\nLook at:\n");
        for ptr in &p.pointers {
            s.push_str(&format!("- {} (`{}`)", ptr.title, ptr.ref_));
            if let Some(n) = ptr.note.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
                s.push_str(" — ");
                s.push_str(n);
            }
            s.push('\n');
        }
    }
    s
}

/// 103 F3 / 101 D4: the bundle as numbered lines for the channel ask card —
/// `n. <title> — recommended: <label>` (≤ 20, one per item). Empty without items.
pub fn items_summary(items: &[Item]) -> String {
    items
        .iter()
        .enumerate()
        .map(|(n, it)| {
            let pick = it
                .recommended
                .and_then(|r| it.options.get(r as usize))
                .map(|o| format!(" — recommended: {}", o.label))
                .unwrap_or_default();
            format!("{}. {}{pick}", n + 1, it.title)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The first non-blank line of `text`, ≤ 140 chars (`…` when cut): an
/// `AttentionItem.detail`.
pub fn head(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or_default()
        .trim_start_matches('#')
        .trim();
    if line.chars().count() <= HEAD_MAX {
        return line.to_owned();
    }
    let mut s: String = line.chars().take(HEAD_MAX - 1).collect();
    s.push('…');
    s
}

/// 132 D1: `text`'s first non-blank line (`#` stripped) cut to `budget` chars —
/// on a sentence end (`. ` `? ` `! `) when one lies past char 40, else on the
/// last space past char 40 with `…`; shorter text verbatim.
pub fn head_sentence(text: &str, budget: usize) -> String {
    const MIN: usize = 40;
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or_default()
        .trim_start_matches('#')
        .trim();
    let chars: Vec<char> = line.chars().collect();
    if chars.len() <= budget {
        return line.to_owned();
    }
    // A sentence end at char i: `.` `?` `!` followed by a space, i past MIN, within budget.
    let sentence_end = (MIN..budget.min(chars.len() - 1))
        .rev()
        .find(|&i| matches!(chars[i], '.' | '?' | '!') && chars[i + 1] == ' ');
    if let Some(i) = sentence_end {
        return chars[..=i].iter().collect();
    }
    let cut = (MIN..budget)
        .rev()
        .find(|&i| chars[i] == ' ')
        .unwrap_or(budget - 1);
    let mut s: String = chars[..cut]
        .iter()
        .collect::<String>()
        .trim_end()
        .to_owned();
    s.push('…');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(kind: PacketKind) -> Packet {
        Packet {
            key: match kind {
                PacketKind::Decision => "decision:d1".into(),
                PacketKind::Review => "review:t1".into(),
            },
            task: TaskId("t1".into()),
            kind,
            by: "agent:ops".into(),
            at_ms: 1,
            question: (kind == PacketKind::Decision).then(|| "ship it?".to_owned()),
            summary: "A summary.".into(),
            board: None,
            options: vec![],
            recommended: None,
            pointers: vec![],
            checks: vec![],
            recommendation: None,
            items: vec![],
            to: None,
            if_wrong: None,
        }
    }

    fn opt(label: &str) -> PacketOption {
        PacketOption {
            label: label.into(),
            description: None,
        }
    }

    fn item(id: &str, labels: &[&str], recommended: Option<u32>) -> Item {
        Item {
            id: id.into(),
            title: format!("record {id}"),
            description: None,
            options: labels.iter().map(|l| opt(l)).collect(),
            recommended,
        }
    }

    /// 103: items ≤ 20, each ≤ 6 options / labels ≤ 80 / `recommended` in range,
    /// ids non-empty and unique; `recommendation.option` in range, text ≤ 1 KiB.
    #[test]
    fn item_and_recommendation_caps() {
        let mut p = packet(PacketKind::Decision);
        p.items = (0..21)
            .map(|i| item(&format!("r{i}"), &["keep"], Some(0)))
            .collect();
        let e = validate(&p).unwrap_err();
        assert!(e.contains("`items`") && e.contains("20"), "{e}");
        p.items.truncate(20);
        assert_eq!(validate(&p), Ok(()));

        p.items = vec![item("a", &["1", "2", "3", "4", "5", "6", "7"], None)];
        let e = validate(&p).unwrap_err();
        assert!(e.contains("`items[0].options`") && e.contains("6"), "{e}");
        p.items = vec![item("a", &[&"x".repeat(81)], None)];
        let e = validate(&p).unwrap_err();
        assert!(
            e.contains("`items[0].options[0].label`") && e.contains("80"),
            "{e}"
        );
        p.items = vec![item("a", &["keep"], Some(1))];
        let e = validate(&p).unwrap_err();
        assert!(
            e.starts_with("`items[0].recommended` is 1 but `items[0].options` has 1"),
            "{e}"
        );
        p.items = vec![item("a", &[], None), item(" ", &[], None)];
        assert_eq!(validate(&p).unwrap_err(), "`items[1].id` is empty");
        p.items = vec![
            item("a", &[], None),
            item("b", &[], None),
            item("a", &[], None),
        ];
        assert_eq!(validate(&p).unwrap_err(), "`items[2].id` `a` is used twice");
        p.items = vec![item("a", &["x"], Some(0)), item("b", &[], None)];
        assert_eq!(validate(&p), Ok(()));

        p.options = vec![opt("keep"), opt("drop")];
        p.recommendation = Some(Recommendation {
            text: "drop it".into(),
            option: Some(2),
        });
        let e = validate(&p).unwrap_err();
        assert!(
            e.starts_with("`recommendation.option` is 2 but `options` has 2"),
            "{e}"
        );
        p.recommendation = Some(Recommendation {
            text: "x".repeat(RECOMMENDATION_MAX + 1),
            option: None,
        });
        let e = validate(&p).unwrap_err();
        assert!(
            e.contains("`recommendation.text`") && e.contains("1 KiB"),
            "{e}"
        );
        p.recommendation = Some(Recommendation {
            text: "x".repeat(RECOMMENDATION_MAX),
            option: Some(1),
        });
        assert_eq!(validate(&p), Ok(()));
        // the top-level option errors keep their names
        p.recommended = Some(5);
        assert!(
            validate(&p)
                .unwrap_err()
                .starts_with("`recommended` is 5 but `options` has 2")
        );
    }

    /// 103: the thread block carries the recommendation line and the numbered items.
    #[test]
    fn thread_block_carries_recommendation_and_items() {
        let mut p = packet(PacketKind::Decision);
        p.options = vec![opt("approve"), opt("hold")];
        p.recommendation = Some(Recommendation {
            text: "Low risk, reversible.".into(),
            option: Some(0),
        });
        p.items = vec![
            Item {
                description: Some("A record".into()),
                ..item("a-rec", &["add", "skip"], Some(0))
            },
            item("mx", &[], None),
        ];
        let md = thread_markdown(&p);
        assert!(
            md.starts_with(
                "**decision requested** (`decision:d1`)\n\nRecommendation: approve — Low risk, reversible.\n\nship it?"
            ),
            "{md}"
        );
        assert!(
            md.contains(
                "Items:\n1. record a-rec — A record: add (recommended) | skip\n2. record mx\n"
            ),
            "{md}"
        );
        p.recommendation = Some(Recommendation {
            text: "Either.".into(),
            option: None,
        });
        assert!(thread_markdown(&p).contains("\n\nRecommendation: Either.\n\n"));

        assert_eq!(
            items_summary(&p.items),
            "1. record a-rec — recommended: add\n2. record mx"
        );
        assert_eq!(items_summary(&[]), "");
    }

    /// 103 §3: `Item` / `Recommendation` round-trip; rows written before 103 have neither field.
    #[test]
    fn items_and_recommendation_wire_shape() {
        let p = Packet {
            recommendation: Some(Recommendation {
                text: "go".into(),
                option: None,
            }),
            items: vec![item("a", &["x"], Some(0))],
            ..packet(PacketKind::Decision)
        };
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["recommendation"]["option"], serde_json::Value::Null);
        assert_eq!(v["items"][0]["id"], "a");
        assert_eq!(v["items"][0]["options"][0]["label"], "x");
        let back: Packet = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(back, p);
        let mut old = v;
        let o = old.as_object_mut().unwrap();
        o.remove("recommendation");
        o.remove("items");
        let back: Packet = serde_json::from_value(old).unwrap();
        assert_eq!((back.recommendation, back.items), (None, vec![]));
        // an item needs only `id` and `title`
        let it: Item =
            serde_json::from_value(serde_json::json!({"id": "a", "title": "A"})).unwrap();
        assert_eq!(
            it,
            Item {
                id: "a".into(),
                title: "A".into(),
                description: None,
                options: vec![],
                recommended: None
            }
        );
    }

    fn pointer(kind: PointerKind, r: &str, text: Option<&str>) -> Pointer {
        Pointer {
            kind,
            ref_: r.into(),
            title: "the thing".into(),
            note: None,
            text: text.map(Into::into),
            root: None,
        }
    }

    #[test]
    fn caps_name_the_field_and_the_cap() {
        let mut p = packet(PacketKind::Decision);
        p.summary = "x".repeat(SUMMARY_MAX + 1);
        let e = validate(&p).unwrap_err();
        assert!(e.contains("`summary`") && e.contains("16 KiB"), "{e}");
        p.summary = "x".repeat(SUMMARY_MAX);
        assert_eq!(validate(&p), Ok(()));

        let mut p = packet(PacketKind::Decision);
        p.pointers = (0..13)
            .map(|i| pointer(PointerKind::Task, &format!("t{i}"), None))
            .collect();
        let e = validate(&p).unwrap_err();
        assert!(e.contains("`pointers`") && e.contains("12"), "{e}");

        let mut p = packet(PacketKind::Decision);
        p.options = (0..7)
            .map(|i| PacketOption {
                label: format!("o{i}"),
                description: None,
            })
            .collect();
        let e = validate(&p).unwrap_err();
        assert!(e.contains("`options`") && e.contains("6"), "{e}");

        let mut p = packet(PacketKind::Decision);
        p.options = vec![PacketOption {
            label: "é".repeat(81),
            description: None,
        }];
        let e = validate(&p).unwrap_err();
        assert!(e.contains("`options[0].label`") && e.contains("80"), "{e}");
        p.options[0].label = "é".repeat(80);
        assert_eq!(validate(&p), Ok(()));

        let mut p = packet(PacketKind::Decision);
        p.pointers = vec![pointer(
            PointerKind::Diff,
            "a..b",
            Some(&"d".repeat(POINTER_TEXT_MAX + 1)),
        )];
        let e = validate(&p).unwrap_err();
        assert!(
            e.contains("`pointers[0].text`") && e.contains("32 KiB"),
            "{e}"
        );

        let mut p = packet(PacketKind::Decision);
        p.pointers = (0..5)
            .map(|i| {
                pointer(
                    PointerKind::Diff,
                    &format!("d{i}"),
                    Some(&"d".repeat(POINTER_TEXT_MAX)),
                )
            })
            .collect();
        let e = validate(&p).unwrap_err();
        assert!(e.contains("128 KiB"), "{e}");

        let mut p = packet(PacketKind::Decision);
        p.options = vec![PacketOption {
            label: "a".into(),
            description: None,
        }];
        p.recommended = Some(1);
        assert!(validate(&p).unwrap_err().contains("`recommended`"));
        p.recommended = Some(0);
        assert_eq!(validate(&p), Ok(()));
    }

    #[test]
    fn ref_rules() {
        let mut p = packet(PacketKind::Review);
        p.pointers = vec![pointer(PointerKind::Url, "ftp://x", None)];
        let e = validate(&p).unwrap_err();
        assert!(
            e.contains("`pointers[0].ref`") && e.contains("https://"),
            "{e}"
        );
        p.pointers = vec![pointer(PointerKind::Url, "https://x", None)];
        assert_eq!(validate(&p), Ok(()));

        p.pointers = vec![pointer(PointerKind::Code, "src/x.rs#L12", None)];
        let e = validate(&p).unwrap_err();
        assert!(
            e.contains("`pointers[0].text`") && e.contains("`code`"),
            "{e}"
        );

        p.pointers = vec![pointer(PointerKind::Code, "src/x.rs:12", Some("let x;"))];
        let e = validate(&p).unwrap_err();
        assert!(e.contains("`src/x.rs#L12`"), "{e}");
        p.pointers = vec![pointer(PointerKind::Code, "src/x.rs:12-20", Some("let x;"))];
        assert!(validate(&p).unwrap_err().contains("`src/x.rs#L12-L20`"));
        p.pointers = vec![pointer(
            PointerKind::Code,
            "src/x.rs#L12-L20",
            Some("let x;"),
        )];
        assert_eq!(validate(&p), Ok(()));
        p.pointers = vec![pointer(
            PointerKind::Code,
            "src/x.rs#L20-L12",
            Some("let x;"),
        )];
        assert!(validate(&p).is_err(), "backwards range");

        p.pointers = vec![pointer(PointerKind::Diff, "main..feat", None)];
        assert!(validate(&p).unwrap_err().contains("`diff`"));
        p.pointers = vec![pointer(
            PointerKind::Diff,
            "main..feat",
            Some("--- a\n+++ b"),
        )];
        assert_eq!(validate(&p), Ok(()));

        assert_eq!(code_ref("a/b.rs#L7"), Some(("a/b.rs", 7, 7)));
        assert_eq!(code_ref("a/b.rs#L7-L9"), Some(("a/b.rs", 7, 9)));
        assert_eq!(code_ref("a/b.rs#7"), None);
        assert_eq!(code_ref("#L7"), None);
        assert_eq!(code_ref("a/b.rs#L0"), None);
    }

    #[test]
    fn user_audience_rule() {
        let mut d = packet(PacketKind::Decision);
        assert_eq!(require_for_user(&d), Err(NEEDS_BOARD_DECISION.into()));
        d.board = Some(" ".into());
        assert_eq!(require_for_user(&d), Err(NEEDS_BOARD_DECISION.into()));
        d.board = Some("b1".into());
        // 132 D2: board and summary first, then the cost of a wrong answer.
        assert_eq!(require_for_user(&d), Err(NEEDS_IF_WRONG.into()));
        d.if_wrong = Some("  ".into());
        assert_eq!(require_for_user(&d), Err(NEEDS_IF_WRONG.into()));
        d.if_wrong = Some("low, one-line revert".into());
        assert_eq!(require_for_user(&d), Ok(()));
        d.summary = " \n".into();
        assert_eq!(require_for_user(&d), Err(NEEDS_SUMMARY.into()));

        let mut r = packet(PacketKind::Review);
        assert_eq!(require_for_user(&r), Err(NEEDS_BOARD_REVIEW.into()));
        r.board = Some("b2".into());
        assert_eq!(
            require_for_user(&r),
            Ok(()),
            "pointers and if_wrong are optional"
        );
        r.summary = String::new();
        assert_eq!(require_for_user(&r), Err(NEEDS_REVIEW_SUMMARY.into()));
        assert!(NEEDS_BOARD_DECISION.contains("`## What you're deciding`"));
        assert!(NEEDS_BOARD_REVIEW.contains("`## Against the criteria`"));
    }

    /// 132 D2/D3 caps: `if_wrong` ≤ 200 chars after trim; a review offers ≤ 4 next moves.
    #[test]
    fn if_wrong_cap_and_review_options_cap() {
        let mut d = packet(PacketKind::Decision);
        d.if_wrong = Some(format!(" {} ", "w".repeat(IF_WRONG_MAX)));
        assert_eq!(validate(&d), Ok(()), "trimmed to the cap");
        d.if_wrong = Some("w".repeat(IF_WRONG_MAX + 1));
        let e = validate(&d).unwrap_err();
        assert!(e.contains("`if_wrong`") && e.contains("200"), "{e}");
        d.if_wrong = None;
        d.options = (0..5).map(|i| opt(&format!("o{i}"))).collect();
        assert_eq!(validate(&d), Ok(()), "a decision keeps the cap of 6");
        let mut r = packet(PacketKind::Review);
        r.options = (0..4).map(|i| opt(&format!("o{i}"))).collect();
        r.recommended = Some(0);
        assert_eq!(validate(&r), Ok(()));
        r.options.push(opt("o4"));
        let e = validate(&r).unwrap_err();
        assert!(e.contains("`options`") && e.contains("4"), "{e}");
    }

    #[test]
    fn head_sentence_clips_on_a_sentence_then_a_word() {
        assert_eq!(
            head_sentence("\n\n# Ship it?\nmore", ROW_BUDGET),
            "Ship it?"
        );
        assert_eq!(head_sentence("", ROW_BUDGET), "");
        let short = "Keep the old cache or drop it before the release?";
        assert_eq!(head_sentence(short, ROW_BUDGET), short);
        // A sentence end past char 40 inside the budget wins, even with words to spare.
        let two = "Should we move the relay to the new host before Friday? The old one is paid until the end of the month and still works.";
        assert_eq!(
            head_sentence(two, ROW_BUDGET),
            "Should we move the relay to the new host before Friday?"
        );
        // A sentence end before char 40 is ignored: the clip falls to the last space with `…`.
        let early = "Ship it? The deploy window is tonight and the migration adds a column that nothing reads yet, so the risk is the lock.";
        let h = head_sentence(early, ROW_BUDGET);
        assert!(h.ends_with('…') && h.chars().count() <= ROW_BUDGET, "{h}");
        assert!(
            h.starts_with("Ship it? The deploy window is tonight"),
            "{h}"
        );
        assert!(!h.ends_with(" …"), "no space before the ellipsis: {h}");
        // No space at all: a hard cut at budget - 1 plus `…`.
        let solid = "x".repeat(300);
        let h = head_sentence(&solid, ROW_BUDGET);
        assert_eq!(h.chars().count(), ROW_BUDGET);
        assert!(h.ends_with('…'));
    }

    #[test]
    fn thread_block_and_head() {
        let mut p = packet(PacketKind::Decision);
        p.options = vec![
            PacketOption {
                label: "ship".into(),
                description: Some("now".into()),
            },
            PacketOption {
                label: "wait".into(),
                description: None,
            },
        ];
        p.recommended = Some(1);
        p.pointers = vec![Pointer {
            note: Some("the retry".into()),
            ..pointer(PointerKind::Code, "src/x.rs#L1-L2", Some("a\nb"))
        }];
        p.board = Some("01hzboard".into());
        let md = thread_markdown(&p);
        assert!(
            md.starts_with(
                "**decision requested** (`decision:d1`)\n\nship it?\n\nA summary.\n\nBoard: theuth://notes/01hzboard\n\nOptions:"
            ),
            "{md}"
        );
        assert!(md.contains("- ship — now\n- wait (recommended)\n"), "{md}");
        assert!(
            md.contains("Look at:\n- the thing (`src/x.rs#L1-L2`) — the retry\n"),
            "{md}"
        );
        assert!(!md.contains("\na\nb"), "evidence stays out of the thread");

        let mut r = packet(PacketKind::Review);
        r.checks = vec![
            Check {
                text: "tests pass".into(),
                done: true,
                note: Some("42 green".into()),
            },
            Check {
                text: "docs".into(),
                done: false,
                note: None,
            },
        ];
        let md = thread_markdown(&r);
        assert!(md.starts_with("**review requested**\n\nA summary.\n\nChecks:\n- [x] tests pass — 42 green\n- [ ] docs\n"), "{md}");

        assert_eq!(head("\n\n# Title line\nmore"), "Title line");
        assert_eq!(head(""), "");
        let long = "x".repeat(200);
        let h = head(&long);
        assert_eq!(h.chars().count(), HEAD_MAX);
        assert!(h.ends_with('…'));
        assert_eq!(head(&"y".repeat(HEAD_MAX)).chars().count(), HEAD_MAX);
    }

    #[test]
    fn wire_shape_matches_the_contract() {
        let p = Packet {
            pointers: vec![pointer(PointerKind::Url, "https://x", None)],
            ..packet(PacketKind::Decision)
        };
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["kind"], "decision");
        assert_eq!(v["pointers"][0]["kind"], "url");
        assert_eq!(v["pointers"][0]["ref"], "https://x");
        assert!(v["pointers"][0].get("ref_").is_none());
        let back: Packet = serde_json::from_value(v).unwrap();
        assert_eq!(back, p);
        // rows written before Revision 2 have no `board`
        let mut old = serde_json::to_value(&p).unwrap();
        old.as_object_mut().unwrap().remove("board");
        let back: Packet = serde_json::from_value(old).unwrap();
        assert_eq!(back.board, None);
        let v = serde_json::to_value(Packet {
            board: Some("b".into()),
            ..packet(PacketKind::Review)
        })
        .unwrap();
        assert_eq!(v["board"], "b");
    }

    fn outline(h: &[(u8, &str)]) -> Vec<(u8, String)> {
        h.iter().map(|(l, t)| (*l, (*t).to_owned())).collect()
    }

    #[test]
    fn check_board_matches_the_first_three_sections() {
        use PacketKind::*;
        let full = |s: &[&str; 5]| outline(&s.map(|t| (2, t)));
        assert_eq!(check_board(Decision, &full(&DECISION_SECTIONS)), Ok(()));
        assert_eq!(check_board(Review, &full(&REVIEW_SECTIONS)), Ok(()));
        // the first three suffice; `###`, `#` and later `##` are free
        let o = outline(&[
            (1, "Title"),
            (2, "what  you’re DECIDING "),
            (3, "a cluster"),
            (2, "Why it matters to you"),
            (2, "The picture"),
            (2, "Anything else"),
            (2, "Open questions"),
        ]);
        assert_eq!(check_board(Decision, &o), Ok(()));
        // missing second
        let e = check_board(
            Decision,
            &outline(&[
                (2, "What you're deciding"),
                (2, "The picture"),
                (2, "Options"),
            ]),
        )
        .unwrap_err();
        assert!(
            e.starts_with("section 2 of a decision board must be `## Why it matters to you`, found `## The picture`"),
            "{e}"
        );
        assert!(e.contains("`## Look at`"), "names the skeleton: {e}");
        // swapped first two
        let e = check_board(
            Review,
            &outline(&[
                (2, "Why it matters to you"),
                (2, "What was done"),
                (2, "The picture"),
            ]),
        )
        .unwrap_err();
        assert!(
            e.starts_with("section 1 of a review board must be `## What was done`, found `## Why it matters to you`"),
            "{e}"
        );
        // too short
        let e = check_board(Decision, &outline(&[(2, "What you're deciding")])).unwrap_err();
        assert!(
            e.starts_with("section 2 of a decision board is missing: `## Why it matters to you`"),
            "{e}"
        );
        assert!(
            check_board(Review, &[])
                .unwrap_err()
                .starts_with("section 1")
        );
        // a `###` in a section's place does not count
        let e = check_board(
            Decision,
            &outline(&[
                (2, "What you're deciding"),
                (3, "Why it matters to you"),
                (2, "The picture"),
            ]),
        )
        .unwrap_err();
        assert!(e.contains("found `## The picture`"), "{e}");
    }
}
