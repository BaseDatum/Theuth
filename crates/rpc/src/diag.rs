//! 176 C2/C3: what clients tell the engine about themselves — `client_log` lines and the
//! `client` tag on the WebSocket URL.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// `client_log` lines one connection may log per [`WINDOW`].
const PER_WINDOW: u32 = 20;
const WINDOW: Duration = Duration::from_secs(60);
/// A `client_log` message is cut to this many chars.
const MAX_CHARS: usize = 2_000;

/// A client tag (`client=` on the WebSocket URL, `client_log`'s `source`): 1–32 chars of
/// `[a-z0-9-]`, else `None`. What passes is safe to print verbatim in a log line.
pub(crate) fn tag(s: &str) -> Option<&str> {
    let ok = !s.is_empty()
        && s.len() <= 32
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    ok.then_some(s)
}

#[derive(Debug, PartialEq)]
enum Verdict {
    Log,
    /// Over the limit; `warn` on the first drop of the window.
    Drop {
        warn: bool,
    },
}

/// One connection's fixed one-minute window.
struct Window {
    start: Instant,
    count: u32,
    warned: bool,
}

impl Window {
    fn new(now: Instant) -> Self {
        Self {
            start: now,
            count: 0,
            warned: false,
        }
    }

    fn admit(&mut self, now: Instant) -> Verdict {
        if now.duration_since(self.start) >= WINDOW {
            *self = Self::new(now);
        }
        self.count = self.count.saturating_add(1);
        if self.count <= PER_WINDOW {
            Verdict::Log
        } else {
            Verdict::Drop {
                warn: !std::mem::replace(&mut self.warned, true),
            }
        }
    }
}

/// `peer id → window`; an entry leaves when its connection closes.
static WINDOWS: LazyLock<Mutex<HashMap<u64, Window>>> = LazyLock::new(Default::default);

/// `message` cut to [`MAX_CHARS`] chars, control chars (newlines from a stack trace) as
/// spaces so one call stays one log line.
fn clean(message: &str) -> String {
    message
        .chars()
        .take(MAX_CHARS)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// `client_log` (176 C2): log `message` at target `client`, rate-limited per connection.
pub(crate) fn client_log(
    peer: &link::RpcPeer,
    conn: u64,
    level: &str,
    source: &str,
    message: &str,
) {
    let now = Instant::now();
    let id = peer.id();
    let verdict = match WINDOWS.lock().entry(id) {
        Entry::Occupied(mut e) => e.get_mut().admit(now),
        Entry::Vacant(e) => {
            let closed = peer.closed();
            tokio::spawn(async move {
                closed.await;
                WINDOWS.lock().remove(&id);
            });
            e.insert(Window::new(now)).admit(now)
        }
    };
    let source = tag(source).unwrap_or("-");
    match verdict {
        Verdict::Log if level == "warn" => {
            tracing::warn!(target: "client", source, conn, "{}", clean(message));
        }
        Verdict::Log => tracing::info!(target: "client", source, conn, "{}", clean(message)),
        Verdict::Drop { warn: true } => tracing::warn!(
            target: "client",
            source,
            conn,
            "client_log: over {PER_WINDOW} lines a minute, dropping the rest of this minute"
        ),
        Verdict::Drop { warn: false } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::into_rpc_module;
    use runtime::Runtime;

    #[test]
    fn twenty_a_minute_then_one_warning_then_a_fresh_window() {
        let t0 = Instant::now();
        let mut w = Window::new(t0);
        for _ in 0..PER_WINDOW {
            assert_eq!(w.admit(t0), Verdict::Log);
        }
        assert_eq!(w.admit(t0), Verdict::Drop { warn: true });
        let later = t0 + Duration::from_secs(59);
        assert_eq!(w.admit(later), Verdict::Drop { warn: false });
        assert_eq!(w.admit(later), Verdict::Drop { warn: false });
        let next = t0 + WINDOW;
        assert_eq!(w.admit(next), Verdict::Log, "the next minute starts over");
        for _ in 1..PER_WINDOW {
            assert_eq!(w.admit(next), Verdict::Log);
        }
        assert_eq!(
            w.admit(next),
            Verdict::Drop { warn: true },
            "and warns again"
        );
    }

    #[test]
    fn messages_are_cut_to_2000_chars_on_one_line() {
        let long = "é".repeat(MAX_CHARS + 5);
        assert_eq!(clean(&long).chars().count(), MAX_CHARS, "chars, not bytes");
        assert_eq!(clean("a\nb\tc"), "a b c");
        assert_eq!(clean("short"), "short");
    }

    #[test]
    fn tags_are_short_lowercase_slugs() {
        for ok in [
            "desktop-main",
            "island",
            "mcp-bridge",
            "a1",
            &"x".repeat(32),
        ] {
            assert_eq!(tag(ok), Some(ok));
        }
        for bad in ["", "Island", "pop out", "a=b", "é", &"x".repeat(33)] {
            assert_eq!(tag(bad), None, "{bad}");
        }
    }

    async fn call(methods: &jsonrpsee::Methods, req: &str) -> serde_json::Value {
        let (resp, _) =
            tokio::time::timeout(Duration::from_secs(1), methods.raw_json_request(req, 1))
                .await
                .expect("answers at once")
                .unwrap();
        serde_json::from_str(resp.get()).unwrap()
    }

    /// C1: `ping` answers `null` with no actor running — it asks none, so a blocked Agents
    /// mailbox cannot delay it either.
    #[tokio::test]
    async fn ping_answers_null_without_actors() {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let methods: jsonrpsee::Methods = into_rpc_module(crate::api::Server::new(rt)).into();
        let v = call(
            &methods,
            r#"{"jsonrpc":"2.0","method":"ping","params":[],"id":1}"#,
        )
        .await;
        assert!(v["result"].is_null() && v.get("result").is_some(), "{v}");
    }

    /// C2: `client_log` answers `null` for any level, and needs an authenticated connection.
    #[tokio::test]
    async fn client_log_answers_null_on_a_connection() {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let mut methods: jsonrpsee::Methods = into_rpc_module(crate::api::Server::new(rt)).into();
        let anon = call(
            &methods,
            r#"{"jsonrpc":"2.0","method":"client_log","params":["info","island","x"],"id":1}"#,
        )
        .await;
        assert!(anon.get("error").is_some(), "{anon}");
        methods.extensions_mut().insert(link::RpcPeer::internal());
        for level in ["info", "warn", "debug"] {
            let req = format!(
                r#"{{"jsonrpc":"2.0","method":"client_log","params":["{level}","Bad Source","hi"],"id":1}}"#
            );
            let v = call(&methods, &req).await;
            assert!(v["result"].is_null() && v.get("result").is_some(), "{v}");
        }
    }
}
