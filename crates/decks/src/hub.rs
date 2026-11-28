//! Per-Deck bounded subscription hub with live-frame coalescing, backpressure, and rate/size caps.
//!
//! Mirrors `crates/document`'s `doc_subscribe` / `fan_out` pattern so high-frequency live Deck
//! frames NEVER touch the global `RuntimeWire` broadcast bus.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tokio::sync::{Notify, mpsc};

use crate::model::{DeckRuntimeEvent, LiveFrame};

/// Maximum serialized bytes for a transient live frame (`deck_emit(..., durable=False)`).
pub const MAX_LIVE_FRAME_BYTES: usize = 256 * 1024;

/// Maximum serialized bytes for a durable snapshot (`deck_emit(..., durable=True)`).
pub const MAX_SNAPSHOT_BYTES: usize = 512 * 1024;

/// Maximum live frame emits accepted per Deck per 1-second window.
pub const MAX_EMITS_PER_SEC: usize = 20;

/// Bounded per-subscriber queue depth for control/snapshot events.
const SUB_QUEUE_CAP: usize = 32;

struct SubSlot {
    id: u64,
    /// Bounded queue of non-coalesced events (Sync, Snapshot, Status, Approval, Closed).
    control_queue: VecDeque<DeckRuntimeEvent>,
    /// Latest coalesced transient live frame (replaces any unread older live frame!).
    pending_frame: Option<LiveFrame>,
    closed: bool,
    notify: std::sync::Arc<Notify>,
}

struct DeckSubs {
    subs: Vec<std::sync::Arc<Mutex<SubSlot>>>,
    emit_window: VecDeque<Instant>,
}

/// Manages per-Deck bounded, coalescing subscriptions.
#[derive(Default)]
pub struct DeckHub {
    next_id: AtomicU64,
    decks: Mutex<HashMap<String, DeckSubs>>,
}

impl DeckHub {
    pub fn new() -> Self {
        Self::default()
    }

    /// Enforce per-Deck emit rate cap (`MAX_EMITS_PER_SEC` in a rolling 1s window).
    /// Returns `true` if allowed, `false` if rate-limited.
    pub fn admit_emit(&self, deck_id: &str) -> bool {
        let now = Instant::now();
        let cutoff = now - Duration::from_secs(1);
        let mut map = self.decks.lock();
        let entry = map.entry(deck_id.to_owned()).or_insert_with(|| DeckSubs {
            subs: Vec::new(),
            emit_window: VecDeque::new(),
        });
        while entry.emit_window.front().is_some_and(|t| *t < cutoff) {
            entry.emit_window.pop_front();
        }
        if entry.emit_window.len() >= MAX_EMITS_PER_SEC {
            return false;
        }
        entry.emit_window.push_back(now);
        true
    }

    /// Register a bounded, coalescing subscriber for `deck_id`, seeding `initial_sync` immediately.
    pub fn subscribe(
        &self,
        deck_id: &str,
        initial_sync: DeckRuntimeEvent,
    ) -> (u64, mpsc::Receiver<DeckRuntimeEvent>) {
        let sub_id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let notify = std::sync::Arc::new(Notify::new());
        let mut initial_queue = VecDeque::with_capacity(SUB_QUEUE_CAP);
        initial_queue.push_back(initial_sync);
        let slot = std::sync::Arc::new(Mutex::new(SubSlot {
            id: sub_id,
            control_queue: initial_queue,
            pending_frame: None,
            closed: false,
            notify: notify.clone(),
        }));
        {
            let mut map = self.decks.lock();
            let entry = map.entry(deck_id.to_owned()).or_insert_with(|| DeckSubs {
                subs: Vec::new(),
                emit_window: VecDeque::new(),
            });
            entry.subs.push(slot.clone());
        }

        // Pump task converts the coalescing slot into a standard bounded `mpsc::Receiver`.
        let (tx, rx) = mpsc::channel(SUB_QUEUE_CAP);
        let slot_pump = slot.clone();
        tokio::spawn(async move {
            loop {
                let next_ev = {
                    let mut guard = slot_pump.lock();
                    if let Some(ev) = guard.control_queue.pop_front() {
                        Some(ev)
                    } else if let Some(frame) = guard.pending_frame.take() {
                        Some(DeckRuntimeEvent::LiveFrame { frame })
                    } else if guard.closed {
                        return;
                    } else {
                        None
                    }
                };
                if let Some(ev) = next_ev {
                    let is_closed = matches!(ev, DeckRuntimeEvent::Closed { .. });
                    if tx.send(ev).await.is_err() {
                        slot_pump.lock().closed = true;
                        return;
                    }
                    if is_closed {
                        slot_pump.lock().closed = true;
                        return;
                    }
                } else {
                    tokio::select! {
                        _ = notify.notified() => {}
                        _ = tx.closed() => {
                            slot_pump.lock().closed = true;
                            return;
                        }
                    }
                }
            }
        });
        (sub_id, rx)
    }

    pub fn unsubscribe(&self, deck_id: &str, sub_id: u64) {
        let mut map = self.decks.lock();
        if let Some(entry) = map.get_mut(deck_id) {
            entry.subs.retain(|s| {
                let mut g = s.lock();
                if g.id == sub_id {
                    g.closed = true;
                    g.notify.notify_one();
                    false
                } else {
                    !g.closed
                }
            });
        }
    }

    /// Fan out an event to all subscribers of `deck_id`.
    /// Transient `LiveFrame`s coalesce (newest wins, zero backpressure crash); bounded control queue
    /// drops genuinely wedged subscribers cleanly.
    pub fn broadcast(&self, deck_id: &str, ev: DeckRuntimeEvent) {
        let mut map = self.decks.lock();
        let Some(entry) = map.get_mut(deck_id) else {
            return;
        };
        entry.subs.retain(|slot| {
            let mut g = slot.lock();
            if g.closed {
                return false;
            }
            match &ev {
                DeckRuntimeEvent::LiveFrame { frame } => {
                    g.pending_frame = Some(frame.clone());
                    g.notify.notify_one();
                    true
                }
                _ => {
                    if g.control_queue.len() >= SUB_QUEUE_CAP {
                        tracing::warn!(deck = %deck_id, sub_id = g.id, "deck subscriber too slow; dropped");
                        g.closed = true;
                        g.notify.notify_one();
                        return false;
                    }
                    g.control_queue.push_back(ev.clone());
                    g.notify.notify_one();
                    true
                }
            }
        });
    }

    pub fn subscriber_count(&self, deck_id: &str) -> usize {
        let map = self.decks.lock();
        map.get(deck_id)
            .map(|e| e.subs.iter().filter(|s| !s.lock().closed).count())
            .unwrap_or(0)
    }
}
