use super::*;

/// Notes an [`Entry::Advisory`] shows before Ctrl+O (125 §7.4).
pub const ADVISORY_COLLAPSED: usize = 3;
/// The advisor glyph (U+25CE; outside `tests/no_emoji.rs`' ranges, `◉` is Vision).
pub const ADVISOR_GLYPH: &str = "◎";

/// `advice` / `nit` / `concern` / `blocker`: the badge word.
pub fn severity_name(s: AdvisorSeverity) -> &'static str {
    match s {
        AdvisorSeverity::Advice => "advice",
        AdvisorSeverity::Nit => "nit",
        AdvisorSeverity::Concern => "concern",
        AdvisorSeverity::Blocker => "blocker",
    }
}

/// The card's severity: the worst of its notes (`Nit` for an empty list).
pub fn advisory_severity(a: &Advisory) -> AdvisorSeverity {
    a.notes
        .iter()
        .map(|n| n.severity)
        .max()
        .unwrap_or(AdvisorSeverity::Nit)
}

/// `◎ advisor · 2 notes`, or `◎ consulted: <question>` for a consultation
/// (R1.2; the question collapsed to one line).
pub fn advisory_title(a: &Advisory) -> String {
    match &a.kind {
        AdvisoryKind::Consultation { question } => {
            let q: Vec<&str> = question.split_whitespace().collect();
            format!("{ADVISOR_GLYPH} consulted: {}", q.join(" "))
        }
        AdvisoryKind::Review => {
            let n = a.notes.len();
            format!(
                "{ADVISOR_GLYPH} advisor · {n} note{}",
                if n == 1 { "" } else { "s" }
            )
        }
    }
}

/// The card's body rows: one per note with its badge for a review; one per
/// line of the answer, unbadged, for a consultation (R1.2). `ADVISORY_COLLAPSED`
/// of them show until Ctrl+O.
pub fn advisory_rows(a: &Advisory) -> Vec<(Option<AdvisorSeverity>, &str)> {
    match a.kind {
        AdvisoryKind::Consultation { .. } => a
            .notes
            .iter()
            .flat_map(|n| n.note.lines())
            .map(|l| (None, l))
            .collect(),
        AdvisoryKind::Review => a
            .notes
            .iter()
            .map(|n| (Some(n.severity), n.note.as_str()))
            .collect(),
    }
}

/// `advisory_rows(a).len()` without building them (`Entry::height`, Ctrl+O).
pub fn advisory_row_count(a: &Advisory) -> usize {
    match a.kind {
        AdvisoryKind::Consultation { .. } => a.notes.iter().map(|n| n.note.lines().count()).sum(),
        AdvisoryKind::Review => a.notes.len(),
    }
}

/// `[concern] note text` / an answer line: one line per row (the headless
/// printer; the renderer colours the badge itself).
pub fn advisory_lines(a: &Advisory) -> Vec<String> {
    advisory_rows(a)
        .into_iter()
        .map(|(sev, text)| match sev {
            Some(s) => format!("[{}] {text}", severity_name(s)),
            None => text.to_owned(),
        })
        .collect()
}

/// `advisor → anthropic/claude-opus-4-1` / `advisor → off`: the one info
/// line on `AdvisorChanged` (125 §7.5, the `model → …` shape).
pub fn advisor_changed_line(s: &AdvisorStatus) -> String {
    match (&s.state, &s.model) {
        (AdvisorState::Off, _) | (_, None) => "advisor → off".to_owned(),
        (_, Some(m)) => format!("advisor → {m}"),
    }
}

/// `off` / `idle` / `reviewing` / `paused` / `halted`.
pub fn advisor_state_name(s: AdvisorState) -> &'static str {
    match s {
        AdvisorState::Off => "off",
        AdvisorState::Idle => "idle",
        AdvisorState::Reviewing => "reviewing",
        AdvisorState::Paused => "paused",
        AdvisorState::Halted => "halted",
    }
}

/// `/advisor status` (125 §7.3 + R1.2): the five lines, `—` for unknowns;
/// one line when off.
pub fn advisor_status_lines(s: &AdvisorStatus) -> Vec<String> {
    if s.state == AdvisorState::Off {
        return vec!["advisor: off  ·  /advisor to pick a model".to_owned()];
    }
    let dash = || "—".to_owned();
    // Why paused / halted rides on the state line.
    let detail = s
        .detail
        .as_deref()
        .map_or_else(String::new, |d| format!("  ·  {d}"));
    let mut out = vec![
        format!(
            "advisor: {}  ·  {}  ·  {}{detail}",
            advisor_state_name(s.state),
            s.model.clone().unwrap_or_else(dash),
            s.effort.clone().unwrap_or_else(dash)
        ),
        format!(
            "notes: {} nit · {} concern · {} blocker  ·  step {}  ·  backlog {}",
            s.notes.nit, s.notes.concern, s.notes.blocker, s.step, s.backlog
        ),
    ];
    let context = if s.context_limit > 0 {
        format!(
            "{} / {} ({}%)",
            k(s.context_used),
            k(s.context_limit),
            u64::from(s.context_used) * 100 / u64::from(s.context_limit)
        )
    } else {
        k(s.context_used)
    };
    out.push(format!(
        "context: {context}  ·  in {} · out {}  ·  cost {}",
        k(u32::try_from(s.tokens_in).unwrap_or(u32::MAX)),
        k(u32::try_from(s.tokens_out).unwrap_or(u32::MAX)),
        s.cost.as_ref().map_or_else(dash, cost_short)
    ));
    out.push(match &s.last_note {
        Some(n) => format!("last: [{}] {}", severity_name(n.severity), n.note),
        None => "last: —".to_owned(),
    });
    out.push(format!(
        "consults: {} this turn · {} total · cap {}",
        s.consults.turn, s.consults.total, s.consults.cap
    ));
    out
}

/// Why an `advisor_status` call was made (125 §7.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AdvisorReply {
    /// `/advisor status`: print the lines of §7.3.
    Status,
}
