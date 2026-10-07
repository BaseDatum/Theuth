//! 125 §4.9: the emission guard — one per link. Normalises a note, drops
//! content-free phrases, dedupes by normalised text with severity-aware
//! escalation, and budgets non-blocker notes per update with displacement
//! of a still-pending lower-rank slot (adapted from omp's `emission-guard`).

use std::collections::{HashMap, VecDeque};

use crate::AdvisorSeverity;

/// Content-free phrases (omp's list, verbatim after normalisation).
const NOISE: &[&str] = &[
    "stop",
    "stop here",
    "stop now",
    "halt",
    "abort",
    "done",
    "task done",
    "task complete",
    "complete",
    "finished",
    "ok",
    "okay",
    "ok done",
    "no issue",
    "no issues",
    "no issue continue",
    "no concerns",
    "no concern",
    "nothing to add",
    "nothing to flag",
    "nothing to report",
    "no notes",
    "no further input",
    "no further input needed",
    "no further input required",
    "no further watcher input",
    "no further watcher input needed",
    "no further advice",
    "no further advice needed",
    "lgtm",
    "looks good",
    "all good",
    "agent is on track",
    "agent on track",
    "on track",
    "continue",
    "carry on",
];

/// Dedupe history bound (FIFO of normalised keys).
const SEEN_MAX: usize = 4096;

/// What the guard decided for one note.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Record it; `displaced` is the index into the caller's pending slots
    /// that this note pushed out, if any.
    Accept { displaced: Option<usize> },
    Empty,
    Noise,
    Duplicate,
    BudgetSpent,
}

/// 125 §4.8 acknowledgement strings (omp's wording).
pub const ACK_ACCEPTED: &str = "Accepted for delivery.";
pub const ACK_DEFERRED: &str = "Deferred — the agent is mid-turn; this note is queued for delivery at the next step boundary unless a higher-severity note from this review displaces it. Do not re-raise the same point.";
pub const ACK_EMPTY: &str = "Not recorded — empty note.";
pub const ACK_NOISE: &str = "Not recorded — the note carries no concrete, actionable content.";
pub const ACK_DUPLICATE: &str = "Duplicate advice ignored — this point was already raised.";
pub const ACK_BUDGET: &str =
    "Not recorded — this update's non-blocker advice budget is spent; the note was dropped.";

/// Lowercase → every run of non-alphanumerics → one space → trim.
// ponytail: omp also applies NFKC; `unicode-normalization` is not a dependency
// of this crate and ligatures/fullwidth forms are rare in model output —
// add it if a duplicate ever slips through on that alone.
pub fn normalise(note: &str) -> String {
    let mut out = String::with_capacity(note.len());
    let mut space = false;
    for c in note.to_lowercase().chars() {
        if c.is_alphanumeric() {
            if space && !out.is_empty() {
                out.push(' ');
            }
            space = false;
            out.push(c);
        } else {
            space = true;
        }
    }
    out
}

/// Rank of a severity: `nit 1 < concern 2 < blocker 3`.
pub fn rank(s: AdvisorSeverity) -> u8 {
    match s {
        AdvisorSeverity::Nit => 1,
        AdvisorSeverity::Concern => 2,
        AdvisorSeverity::Blocker => 3,
    }
}

/// Per-link guard state.
#[derive(Debug)]
pub struct Guard {
    /// normalised note → highest rank seen.
    seen: HashMap<String, u8>,
    /// Insertion order of `seen`, for the FIFO bound.
    order: VecDeque<String>,
    /// Non-blocker slots per update.
    budget: usize,
    /// Non-blocker notes accepted this update.
    used: usize,
}

impl Guard {
    pub fn new(max_notes: u8) -> Self {
        Self {
            seen: HashMap::new(),
            order: VecDeque::new(),
            budget: usize::from(max_notes.clamp(1, 32)),
            used: 0,
        }
    }

    /// Live `advisor.max_notes_per_update`.
    pub fn set_budget(&mut self, max_notes: u8) {
        self.budget = usize::from(max_notes.clamp(1, 32));
    }

    /// An update begins: the per-update slots reset, the dedupe history stays.
    pub fn begin_update(&mut self) {
        self.used = 0;
    }

    /// Dispose: everything cleared.
    pub fn reset(&mut self) {
        self.seen.clear();
        self.order.clear();
        self.used = 0;
    }

    /// Admit `note` at `severity`. `pending` is the rank of every
    /// still-undelivered non-blocker slot of this update (the caller's held
    /// notes): a strictly higher-rank note displaces the lowest of them when
    /// the budget is spent.
    pub fn admit(&mut self, note: &str, severity: AdvisorSeverity, pending: &[u8]) -> Verdict {
        let key = normalise(note);
        if key.is_empty() {
            return Verdict::Empty;
        }
        if NOISE.contains(&key.as_str()) {
            return Verdict::Noise;
        }
        let r = rank(severity);
        if let Some(&prev) = self.seen.get(&key)
            && r <= prev
        {
            return Verdict::Duplicate;
        }
        let mut displaced = None;
        if severity != AdvisorSeverity::Blocker {
            if self.used >= self.budget {
                let lowest = pending
                    .iter()
                    .enumerate()
                    .filter(|(_, &p)| p < r)
                    .min_by_key(|(_, &p)| p)
                    .map(|(i, _)| i);
                match lowest {
                    Some(i) => displaced = Some(i),
                    None => return Verdict::BudgetSpent,
                }
            } else {
                self.used += 1;
            }
        }
        if self.seen.insert(key.clone(), r).is_none() {
            self.order.push_back(key);
            while self.order.len() > SEEN_MAX {
                if let Some(old) = self.order.pop_front() {
                    self.seen.remove(&old);
                }
            }
        }
        Verdict::Accept { displaced }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises_and_recognises_noise() {
        assert_eq!(normalise("  Looks   GOOD!! "), "looks good");
        assert_eq!(normalise("fix-it, now"), "fix it now");
        let mut g = Guard::new(4);
        assert_eq!(g.admit("LGTM", AdvisorSeverity::Nit, &[]), Verdict::Noise);
        assert_eq!(g.admit("   ", AdvisorSeverity::Nit, &[]), Verdict::Empty);
    }

    #[test]
    fn dedupes_with_escalation_and_budgets_with_displacement() {
        let mut g = Guard::new(2);
        let a = Verdict::Accept { displaced: None };
        assert_eq!(g.admit("check the lock", AdvisorSeverity::Nit, &[]), a);
        assert_eq!(
            g.admit("Check the lock.", AdvisorSeverity::Nit, &[1]),
            Verdict::Duplicate
        );
        // Escalation passes; a later downgrade is a duplicate.
        assert_eq!(g.admit("check the lock", AdvisorSeverity::Concern, &[1]), a);
        assert_eq!(
            g.admit("check the lock", AdvisorSeverity::Nit, &[1, 2]),
            Verdict::Duplicate
        );
        // Budget of 2 is spent; a nit is dropped, a concern displaces the pending nit.
        assert_eq!(
            g.admit("another nit", AdvisorSeverity::Nit, &[1, 2]),
            Verdict::BudgetSpent
        );
        assert_eq!(
            g.admit("a real concern", AdvisorSeverity::Concern, &[1, 2]),
            Verdict::Accept { displaced: Some(0) }
        );
        // Blockers never take a slot.
        assert_eq!(g.admit("stop the deploy", AdvisorSeverity::Blocker, &[2, 2]), a);
        g.begin_update();
        assert_eq!(g.admit("fresh nit", AdvisorSeverity::Nit, &[]), a);
    }
}
