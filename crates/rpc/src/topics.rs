//! In-process topic intake and the hub's shared, owner-aware stream controls.
//! Subscribe synchronously before initialization; snapshots precede live frames.

use std::sync::Arc;
use runtime::Runtime;
use serde_json::Value;
use tokio::sync::broadcast;
use wire::{RuntimeWire as _, WireFrame};

/// Pending local and relayed prompts, filtered exactly like live admission.
pub async fn pending(
    keep: impl FnMut(&Value) -> bool,
    mut selected: impl FnMut(&str) -> bool,
) -> Value {
    let mut prompts = crate::api::PendingPrompts::collect(keep).await;
    if !selected("tools.approval-requested") { prompts.approvals.clear(); }
    if !selected("tools.ask-requested") { prompts.asks.clear(); }
    if !selected("secrets.events") { prompts.secrets.clear(); }
    serde_json::to_value(prompts).expect("pending prompts are JSON serializable")
}

/// Loss is not transcript replay; the next item must be a fresh snapshot.
pub fn lagged(count: u64) -> Value {
    serde_json::json!({"type": "Lagged", "count": count})
}

/// Refresh prediction chips after the initial pending snapshot.
pub async fn republish() {
    use runtime::kameo::actor::ActorRef;
    if let Ok(Some(d)) = ActorRef::<double::actor::Double>::lookup(double::DOUBLE_ACTOR) {
        let _ = d.tell(double::actor::Republish).await;
    }
}

/// Frames stay shared; connection-specific controls are JSON, not owner DTOs.
pub enum LocalEvent {
    /// Original shared owner frame.
    Frame(Arc<WireFrame>),
    /// Connection-specific rpc.stream inner JSON.
    Control(Value),
}

/// A trusted in-process consumer has the same broad scope as the old local bus.
/// It does not acquire an external-RPC liveness lease.
pub struct LocalStream {
    rx: broadcast::Receiver<Arc<WireFrame>>,
    snapshot: bool,
    initial: bool,
}

/// Take the wire cursor before any asynchronous initialization or snapshot reads.
pub fn local_stream(rt: &Runtime) -> LocalStream {
    LocalStream { rx: rt.wire_events(), snapshot: true, initial: true }
}

impl LocalStream {
    /// Initial PendingPrompts, live frames, and Lagged followed by PendingPrompts.
    pub async fn next(&mut self) -> Option<LocalEvent> {
        if self.snapshot {
            let prompts = pending(|_| true, |_| true).await;
            self.snapshot = false;
            return Some(LocalEvent::Control(prompts));
        }
        if self.initial {
            self.initial = false;
            republish().await;
        }
        match self.rx.recv().await {
            Ok(frame) => Some(LocalEvent::Frame(frame)),
            Err(broadcast::error::RecvError::Lagged(count)) => {
                self.snapshot = true;
                Some(LocalEvent::Control(lagged(count)))
            }
            Err(broadcast::error::RecvError::Closed) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn local_snapshot_precedes_frames_and_loss_requires_a_fresh_snapshot() {
        let rt = Runtime::new(Default::default(), std::env::temp_dir());
        let local = local_stream(&rt);
        assert_eq!(rt.wire_external_stream_count(), 0);
        assert_eq!(rt.wire_receiver_count(), 1);
        drop(local);
        assert_eq!(rt.wire_receiver_count(), 0);

        // A small real broadcast ring makes loss deterministic without sleeps.
        let (tx, rx) = broadcast::channel(2);
        let mut stream = LocalStream { rx, snapshot: true, initial: false };
        let frames: Vec<_> = (0..4).map(|seq| Arc::new(WireFrame {
            descriptor: None, event: json!({"seq":seq}), policy: Default::default(),
        })).collect();
        for frame in &frames { tx.send(frame.clone()).unwrap(); }
        let Some(LocalEvent::Control(first)) = stream.next().await else { panic!("initial snapshot") };
        assert_eq!(first["type"], "PendingPrompts");
        let Some(LocalEvent::Control(loss)) = stream.next().await else { panic!("loss control") };
        assert_eq!(loss, lagged(2));
        let Some(LocalEvent::Control(snapshot)) = stream.next().await else { panic!("replacement snapshot") };
        assert_eq!(snapshot["type"], "PendingPrompts");
        let Some(LocalEvent::Frame(frame)) = stream.next().await else { panic!("retained live frame") };
        assert!(Arc::ptr_eq(&frame, &frames[2]));
        rt.shutdown.cancel();
    }
}
