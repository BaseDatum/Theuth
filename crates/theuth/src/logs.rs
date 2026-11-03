//! Engine log setup: the `EnvFilter` (176 R4) and the slow actor handler warning (176 R3).

use std::time::{Duration, Instant};

use tracing::Subscriber;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::LookupSpan;

/// `rust_log` (`RUST_LOG`) behind `ort=warn`: ONNX Runtime's INFO is ~840k lines a day
/// ("Reserving memory in BFCArena", "Falling back to … unpacked compute mode"). A directive
/// replaces an earlier one for the same target, so `RUST_LOG=ort=info` still wins.
pub fn env_filter(rust_log: &str) -> EnvFilter {
    EnvFilter::builder().parse_lossy(format!("ort=warn,{rust_log}"))
}

/// The engine's filter: [`env_filter`], then INFO for everything and iroh's pkarr publisher
/// at ERROR — it warns "Failed to publish to pkarr" every 1–13 s while the DNS relay is
/// unreachable (69 Z) and the endpoint keeps working. Added last, these two override
/// `RUST_LOG` for their targets, as before.
pub fn engine_filter(rust_log: &str) -> EnvFilter {
    env_filter(rust_log)
        .add_directive(tracing::Level::INFO.into())
        .add_directive(
            "iroh::address_lookup::pkarr=error"
                .parse()
                .expect("static directive"),
        )
}

/// A handler at least this long holds its actor's mailbox long enough to stall callers.
const SLOW: Duration = Duration::from_secs(1);
/// kameo 0.22's per-message span (`actor/kind.rs`), with `actor.name` and `message`.
const SPAN: &str = "actor.handle_message";

/// WARN `slow handler actor=<name> message=<type> took=<ms>ms` (target `actor`) for every
/// kameo handler that runs ≥ [`SLOW`]. Other spans cost one name compare.
pub struct SlowHandlers;

/// Kept in the span's extensions from open to close.
struct Started {
    at: Instant,
    actor: Name,
    message: Name,
}

/// A field value copied inline: kameo passes `&'static str` type names but `Visit` hands
/// them over as `&str`, and a `String` per message is the allocation this layer avoids.
struct Name {
    buf: [u8; 96],
    len: usize,
}

impl Name {
    const EMPTY: Self = Self {
        buf: [0; 96],
        len: 0,
    };

    fn set(&mut self, s: &str) {
        let mut n = s.len().min(self.buf.len());
        while !s.is_char_boundary(n) {
            n -= 1;
        }
        self.buf[..n].copy_from_slice(&s.as_bytes()[..n]);
        self.len = n;
    }

    fn as_str(&self) -> &str {
        std::str::from_utf8(&self.buf[..self.len]).unwrap_or("?")
    }
}

impl Visit for Started {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "actor.name" => self.actor.set(value),
            "message" => self.message.set(value),
            _ => {}
        }
    }

    fn record_debug(&mut self, _: &Field, _: &dyn std::fmt::Debug) {}
}

impl<S> Layer<S> for SlowHandlers
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        if attrs.metadata().name() != SPAN {
            return;
        }
        let Some(span) = ctx.span(id) else {
            return;
        };
        let mut started = Started {
            at: Instant::now(),
            actor: Name::EMPTY,
            message: Name::EMPTY,
        };
        attrs.record(&mut started);
        span.extensions_mut().insert(started);
    }

    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(&id) else {
            return;
        };
        if span.name() != SPAN {
            return;
        }
        let Some(s) = span.extensions_mut().remove::<Started>() else {
            return;
        };
        let took = s.at.elapsed();
        if took >= SLOW {
            tracing::warn!(
                target: "actor",
                "slow handler actor={} message={} took={}ms",
                s.actor.as_str(),
                s.message.as_str(),
                took.as_millis()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tracing::Event;
    use tracing_subscriber::layer::SubscriberExt;

    use super::*;

    /// Every event that reaches the subscriber: `(target, level, message)`.
    #[derive(Clone, Default)]
    struct Seen(Arc<Mutex<Vec<(String, tracing::Level, String)>>>);

    impl<S: Subscriber> Layer<S> for Seen {
        fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
            struct Msg(String);
            impl Visit for Msg {
                fn record_debug(&mut self, f: &Field, v: &dyn std::fmt::Debug) {
                    if f.name() == "message" {
                        self.0 = format!("{v:?}");
                    }
                }
            }
            let mut m = Msg(String::new());
            event.record(&mut m);
            let meta = event.metadata();
            self.0
                .lock()
                .unwrap()
                .push((meta.target().into(), *meta.level(), m.0));
        }
    }

    fn ort_levels(rust_log: &str) -> Vec<tracing::Level> {
        let seen = Seen::default();
        let sub = tracing_subscriber::registry()
            .with(engine_filter(rust_log))
            .with(seen.clone());
        tracing::subscriber::with_default(sub, || {
            tracing::info!(target: "ort::logging", "Reserving memory in BFCArena");
            tracing::warn!(target: "ort::logging", "warned");
        });
        let seen = seen.0.lock().unwrap();
        seen.iter()
            .filter(|(t, ..)| t == "ort::logging")
            .map(|(_, l, _)| *l)
            .collect()
    }

    #[test]
    fn ort_speaks_at_warn_unless_rust_log_names_it() {
        use tracing::Level;
        assert_eq!(ort_levels(""), [Level::WARN]);
        assert_eq!(
            ort_levels("debug"),
            [Level::WARN],
            "a bare level keeps ort quiet"
        );
        assert_eq!(ort_levels("ort=info"), [Level::INFO, Level::WARN]);
        assert_eq!(ort_levels("ort::logging=info"), [Level::INFO, Level::WARN]);
        assert_eq!(ort_levels("ort=error"), Vec::<Level>::new());
    }

    #[test]
    fn a_handler_over_a_second_warns_with_its_actor_and_message() {
        let seen = Seen::default();
        let sub = tracing_subscriber::registry()
            .with(engine_filter(""))
            .with(SlowHandlers)
            .with(seen.clone());
        tracing::subscriber::with_default(sub, || {
            let span = |message: &'static str| {
                tracing::info_span!(
                    "actor.handle_message",
                    actor.name = "agent::supervisor::Agents",
                    actor.id = %7,
                    message = message,
                )
            };
            span("agent::Fast").in_scope(|| {});
            span("agent::ListActivity")
                .in_scope(|| std::thread::sleep(SLOW + Duration::from_millis(20)));
            tracing::info_span!("other", message = "x")
                .in_scope(|| std::thread::sleep(SLOW + Duration::from_millis(20)));
        });
        let seen = seen.0.lock().unwrap();
        let warns: Vec<_> = seen.iter().filter(|(t, ..)| t == "actor").collect();
        assert_eq!(warns.len(), 1, "{seen:?}");
        let (_, level, msg) = warns[0];
        assert_eq!(*level, tracing::Level::WARN);
        assert!(
            msg.starts_with(
                "slow handler actor=agent::supervisor::Agents message=agent::ListActivity took="
            ) && msg.ends_with("ms"),
            "{msg}"
        );
    }

    #[test]
    fn names_are_cut_on_a_char_boundary() {
        let mut n = Name::EMPTY;
        n.set(&"é".repeat(60));
        assert_eq!(n.as_str(), "é".repeat(48));
    }
}
