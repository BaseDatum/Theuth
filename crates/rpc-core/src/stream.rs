//! Shared wire-frame backlog; admission and transport envelopes stay outside it.
use wire::{Delta, WireFrame};
use std::{collections::HashSet, sync::Arc, time::Duration};
use tokio::time::Instant;

/// 129 §3.1: the event types a connection's focus filters and a stream merges
/// while they wait. Everything else passes untouched.
pub fn is_delta(ev: &WireFrame) -> bool {
    ev.policy.delta != Delta::Control
}

/// Pending deltas go out at least this often when the sink has room (129 §3.1).
pub(crate) const FLUSH_MS: u64 = 40;

/// Non-delta events one stream holds for a slow client; past this the oldest
/// is dropped and counted into the next `Lagged` (129 §3.3).
pub const BACKLOG: usize = 8192;

/// What one `subscribe_events` stream shares between its ring reader and its
/// sink writer; locked per statement, never across an await.
pub struct Stream {
    /// Pending frames, merged and bounded by this stream.
    pub backlog: Backlog,
    /// Descendants-mode tree; `wanted` enrols spawned children into it.
    pub tree: Option<HashSet<String>>,
    /// The ring is gone: the writer returns once the backlog is empty.
    pub closed: bool,
}

/// Frames of one stream between the ring and the sink, in emit order (129 §3.1,
/// §3.3). A delta merges into the latest frame of its key unless a non-delta
/// event of its session came after that frame, so per-session order is exact;
/// a delta batch waits up to `FLUSH_MS` unless a non-delta is queued behind it.
#[derive(Default)]
pub struct Backlog {
    frames: std::collections::VecDeque<Arc<WireFrame>>,
    /// Non-delta frames in `frames`, at most `BACKLOG`.
    control: usize,
    /// Delta frames in `frames`.
    deltas: usize,
    /// Delta frames of the batch currently going out.
    due: usize,
    /// When the pending delta batch is due.
    flush_at: Option<Instant>,
    /// Non-delta events dropped since the last `Lagged`.
    pub lost: u64,
}

/// What the sink writer does next.
pub enum Next {
    /// Emit one shared frame.
    Frame(Arc<WireFrame>),
    /// Emit the number of discarded non-delta frames.
    Lagged(u64),
    /// Nothing due: wake on the next push, or at the instant.
    Wait(Option<Instant>),
}

impl Backlog {
    /// Enqueue a frame, preserving per-session order and bounded controls.
    pub fn push(&mut self, ev: Arc<WireFrame>, now: Instant) {
        if !is_delta(&ev) {
            if self.control == BACKLOG {
                // ponytail: O(n) scan from the front; the oldest non-delta is near it.
                if let Some(i) = self.frames.iter().position(|f| !is_delta(f)) {
                    self.frames.remove(i);
                }
                self.control -= 1;
                self.lost += 1;
            }
            self.control += 1;
            self.frames.push_back(ev);
            return;
        }
        let same = |a: &serde_json::Value, b: &serde_json::Value, k: &str| a.get(k) == b.get(k);
        for f in self.frames.iter_mut().rev() {
            if !same(&f.event, &ev.event, "session") {
                continue;
            }
            if !is_delta(f) {
                break;
            }
            if f.descriptor.as_ref().map(|d| (d.topic, d.version))
                == ev.descriptor.as_ref().map(|d| (d.topic, d.version))
                && same(&f.event, &ev.event, "type")
                && same(&f.event, &ev.event, "turn")
                && same(&f.event, &ev.event, "call_id")
            {
                // Text concatenates; cumulative progress keeps the latest frame.
                if ev.policy.delta == Delta::AppendText {
                    if let Some(add) = ev.event.get("text").and_then(|t| t.as_str())
                        && let Some(serde_json::Value::String(acc)) =
                            Arc::make_mut(f).event.get_mut("text")
                    {
                        acc.push_str(add);
                    }
                } else {
                    *f = ev;
                }
                return;
            }
        }
        if self.deltas == 0 {
            self.flush_at = Some(now + Duration::from_millis(FLUSH_MS));
        }
        self.deltas += 1;
        self.frames.push_back(ev);
    }

    /// Select the next frame or its next wake deadline.
    pub fn next(&mut self, now: Instant) -> Next {
        if self.lost > 0 {
            return Next::Lagged(std::mem::take(&mut self.lost));
        }
        let Some(delta) = self.frames.front().map(|f| is_delta(f)) else {
            return Next::Wait(None);
        };
        if delta {
            if self.due == 0 && self.control == 0 {
                if let Some(at) = self.flush_at
                    && at > now
                {
                    return Next::Wait(Some(at));
                }
                // The batch is due: everything pending goes; later arrivals
                // start the next one.
                self.due = self.deltas;
                self.flush_at = Some(now + Duration::from_millis(FLUSH_MS));
            }
            self.deltas -= 1;
            self.due = self.due.saturating_sub(1);
            if self.deltas == 0 {
                self.flush_at = None;
            }
        } else {
            self.control -= 1;
        }
        Next::Frame(self.frames.pop_front().expect("front exists"))
    }
}
