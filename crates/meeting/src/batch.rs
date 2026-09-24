//! Batching utterances into session inputs (02 §3, 53 §4.4). Pure: no clock
//! of its own, no actor — the [`crate::Meeting`] actor feeds it and delivers
//! what it returns. Rules: (a) `secs` since the last send with ≥ 1 pending,
//! (b) `max` pending, (c) a `System` utterance ending in `?` flushes at once.
//! Request utterances never enter it (they go out on their own line).
//!
//! 53 §4.4's detector seam: [`Batcher::feed`] is the one function that would change.

use std::time::{Duration, Instant};

use crate::{Track, Utterance};

/// Steers queued on the session above which a due batch is held back and
/// merged into the next one (02 §3: a bound on backlog after a long tool call).
pub const MAX_QUEUED: usize = 3;

/// Pending utterances and the send cadence.
#[derive(Debug)]
pub struct Batcher {
    pending: Vec<Utterance>,
    last_send: Instant,
    secs: Duration,
    max: usize,
    /// A batch is owed: a rule fired (or a batch was deferred) and nothing went out since.
    due: bool,
}

impl Batcher {
    /// `now` counts as the last send (the `started.` line goes out at start).
    pub fn new(secs: u32, max: u32, now: Instant) -> Self {
        Self {
            pending: Vec::new(),
            last_send: now,
            secs: Duration::from_secs(u64::from(secs)),
            max: (max as usize).max(1),
            due: false,
        }
    }

    /// One closed utterance in; the batch to send, if a rule fired.
    pub fn feed(&mut self, u: Utterance, now: Instant) -> Option<Vec<Utterance>> {
        let question = u.track == Track::System && u.text.trim_end().ends_with('?');
        self.pending.push(u);
        if question || self.pending.len() >= self.max {
            self.due = true;
        }
        self.tick(now)
    }

    /// Rule (a) on the interval, plus anything already due; the batch to send.
    pub fn tick(&mut self, now: Instant) -> Option<Vec<Utterance>> {
        if self.pending.is_empty() {
            return None;
        }
        if self.due || now.saturating_duration_since(self.last_send) >= self.secs {
            self.due = false;
            self.last_send = now;
            return Some(std::mem::take(&mut self.pending));
        }
        None
    }

    /// The session's steer queue was over [`MAX_QUEUED`]: put `batch` back in
    /// front of what arrived meanwhile; the next `tick`/`feed` returns all of it
    /// as one batch.
    pub fn defer(&mut self, mut batch: Vec<Utterance>) {
        batch.append(&mut self.pending);
        self.pending = batch;
        self.due = true;
    }

    /// Everything pending, for the last input at stop.
    pub fn drain(&mut self) -> Vec<Utterance> {
        self.due = false;
        std::mem::take(&mut self.pending)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(track: Track, text: &str) -> Utterance {
        Utterance {
            track,
            t0_ms: 0,
            t1_ms: 1000,
            text: text.into(),
            request: false,
            speaker: None,
        }
    }

    /// (a) fires on the interval, not before; (b) fires on the count; (c) a
    /// `them:` question flushes alone while a `me:` question waits.
    #[test]
    fn rules_a_b_c() {
        let t0 = Instant::now();
        let mut b = Batcher::new(25, 3, t0);
        assert!(b.feed(u(Track::Mic, "hello"), t0 + Duration::from_secs(1)).is_none());
        assert!(b.tick(t0 + Duration::from_secs(24)).is_none());
        let a = b.tick(t0 + Duration::from_secs(25)).expect("rule a");
        assert_eq!(a.len(), 1);
        // Nothing pending: the interval alone sends nothing.
        assert!(b.tick(t0 + Duration::from_secs(60)).is_none());

        let t1 = t0 + Duration::from_secs(26);
        assert!(b.feed(u(Track::Mic, "one"), t1).is_none());
        assert!(b.feed(u(Track::System, "two"), t1).is_none());
        let bb = b.feed(u(Track::Mic, "three"), t1).expect("rule b");
        assert_eq!(bb.len(), 3);

        assert!(b.feed(u(Track::Mic, "is it me?"), t1).is_none(), "a me: question waits");
        let c = b.feed(u(Track::System, "what did we decide? "), t1).expect("rule c");
        assert_eq!(c.len(), 2);
        assert_eq!(c[1].text, "what did we decide? ");
    }

    /// A deferred batch merges with what arrives after it and goes out as one
    /// on the next tick; `drain` empties without a clock.
    #[test]
    fn defer_merges_into_one_batch() {
        let t0 = Instant::now();
        let mut b = Batcher::new(25, 2, t0);
        assert!(b.feed(u(Track::Mic, "a"), t0).is_none());
        let first = b.feed(u(Track::Mic, "b"), t0).expect("rule b");
        b.defer(first);
        assert!(b.feed(u(Track::Mic, "c"), t0).is_some_and(|m| {
            m.iter().map(|x| x.text.as_str()).collect::<Vec<_>>() == ["a", "b", "c"]
        }));
        assert!(b.tick(t0).is_none());
        b.feed(u(Track::Mic, "d"), t0);
        assert_eq!(b.drain().len(), 1);
        assert!(b.drain().is_empty());
    }
}
