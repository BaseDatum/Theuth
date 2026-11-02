//! Accessibility-tree observation and headless element actions over
//! vercel-labs/agent-desktop v1.0.2, linked in-process (design doc 161).
//! This module owns every agent-desktop call: [`args`]
//! maps tool arguments to agent-desktop commands, `session` holds a theuth
//! session's adapter and namespace on its own thread, [`tools`] defines the
//! six tools, and [`A11y`] is the per-session registry they share (161 §3).
//! `AppError` is agent-desktop's error contract; its Results stay on the worker thread, and boxing would churn every upstream call.
#![allow(clippy::result_large_err)]

pub mod args;
mod command;
mod session;
pub mod tools;

use std::path::Path;
use std::thread::JoinHandle;
use std::time::Duration;

use agent_desktop_core::response::output::ErrorPayload;
use agent_desktop_core::{AdapterError, AppError, DeliverySemantics, ErrorCode};
use runtime::SessionId;
use serde_json::json;

pub use args::Request;

use crate::worker::{DEADLINE, Failure, Workers};
use session::{Backend, Live};

/// A session worker with no command for this long retires, like
/// agent-desktop's command host (AD:`src/command_host/mod.rs:10`).
const IDLE: Duration = Duration::from_secs(15 * 60);

/// One agent-desktop worker thread per theuth session, spawned by the
/// session's first command and retired when the session closes or idles
/// (161 §3). The namespace of every worker is its session id; its manifest
/// name is the engine's (§3.2).
pub struct A11y {
    workers: Workers<SessionId, Option<Live>>,
    backend: Backend,
    engine: String,
    deadline: Duration,
}

impl A11y {
    /// The registry of the engine whose data dir is `data_dir`; its
    /// namespaces are named `theuth:<hash of data_dir>`.
    #[must_use]
    pub fn new(data_dir: &Path) -> Self {
        Self::with(
            Backend::REAL,
            session::engine_name(data_dir),
            IDLE,
            DEADLINE,
        )
    }

    fn with(backend: Backend, engine: String, idle: Duration, deadline: Duration) -> Self {
        let workers = Workers::new(
            |session| format!("computer-a11y-{}", short(session)),
            deadline,
            Some(idle),
            move |session: &SessionId, live: Option<Live>| {
                drop(live);
                (backend.reclaim)(&session.to_string());
            },
        );
        Self {
            workers,
            backend,
            engine,
            deadline,
        }
    }

    /// Reclaim the namespaces an earlier run of this engine left behind: its
    /// worker threads died with the process. Call at plugin start, before
    /// any command; blocks on agent-desktop's session store (§3.2).
    pub fn sweep(&self) {
        session::sweep(&self.engine);
    }

    /// Run `request` on `session`'s worker. `Ok` is agent-desktop's `data`
    /// as compact JSON; `Err` is the compact error envelope (§4.2).
    pub async fn run(&self, session: SessionId, request: Request) -> Result<String, String> {
        let command = args::command(request).map_err(|e| envelope(&e))?;
        let read_only = !command.mutating();
        let backend = self.backend;
        let namespace = session.to_string();
        let engine = self.engine.clone();
        let ran = self
            .workers
            .run(&session, move |live| {
                session::execute(live, backend, &namespace, &engine, command)
            })
            .await;
        ran.unwrap_or_else(|failure| Err(failure_error(failure, self.deadline)))
            .map(|data| data.to_string())
            .map_err(|e| envelope(&settle(e, read_only)))
    }

    /// Retire the workers of closing `sessions` (51 §4.5); each drops its
    /// state and reclaims its namespace on its own thread. A session with no
    /// live worker (abandoned, idled out, or never used since a restart) is
    /// reclaimed on a detached thread through the same path, a no-op without
    /// a manifest. Join the handles to wait; the plugin detaches them.
    pub fn forget(&self, sessions: &[SessionId]) -> Vec<JoinHandle<()>> {
        self.workers.retire(sessions)
    }
}

/// The last 8 hex digits of a session id: its random part, for thread
/// names and logs.
fn short(session: &SessionId) -> String {
    let mut id = session.to_string();
    id.split_off(id.len().saturating_sub(8))
}

/// theuth's own worker failures as agent-desktop errors, so they settle and
/// read like any other (§3.4).
fn failure_error(failure: Failure, deadline: Duration) -> AppError {
    let error = match failure {
        Failure::Timeout => AdapterError::timeout(format!(
            "the accessibility worker did not answer within {deadline:?} and was restarted"
        ))
        .with_suggestion("Take a new computer_snapshot; older refs may answer STALE_REF."),
        Failure::Panicked(message) => {
            AdapterError::internal(format!("the accessibility worker panicked: {message}"))
                .with_suggestion("Take a new computer_snapshot before acting again.")
        }
        Failure::Gone(message) => AdapterError::internal(message),
    };
    error.into()
}

/// agent-desktop's `settle_unknown_delivery`, private to its CLI
/// (AD:`src/dispatch/disposition.rs:9-24`): a refused argument and a
/// read-only command sent nothing, so their unknown delivery is
/// `not_delivered`; a mutation's unknown stays unknown.
fn settle(error: AppError, read_only: bool) -> AppError {
    let source = match error {
        AppError::Adapter(source) => source,
        other if read_only => {
            let code = match &other {
                AppError::Io(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    ErrorCode::PermDenied
                }
                _ => ErrorCode::Internal,
            };
            AdapterError::new(code, other.to_string())
        }
        other => return other,
    };
    let sent_nothing = read_only || source.code == ErrorCode::InvalidArgs;
    if source.disposition == DeliverySemantics::unknown() && sent_nothing {
        source
            .with_disposition(DeliverySemantics::not_delivered())
            .into()
    } else {
        source.into()
    }
}

/// The compact error the model reads (§4.2): code, message, suggestion when
/// there is one, and `retry` (`safe`, `unsafe` or `unknown`).
fn envelope(error: &AppError) -> String {
    let payload = ErrorPayload::from_app_error(error);
    let mut out = json!({
        "code": payload.code,
        "message": payload.message,
        "retry": payload.disposition.retry(),
    });
    if let Some(suggestion) = suggestion(&payload.code, payload.suggestion) {
        out["suggestion"] = suggestion.into();
    }
    out.to_string()
}

/// agent-desktop's suggestion in theuth's words: its PERM_DENIED advice
/// names its own CLI, so on macOS theuth's grant sentence replaces it (and
/// elsewhere it is dropped); "(CLI: …; FFI: …)" tails are cut.
fn suggestion(code: &str, upstream: Option<String>) -> Option<String> {
    if code == ErrorCode::PermDenied.as_str() {
        return cfg!(target_os = "macos").then(|| {
            format!(
                "{}; macOS may keep refusing until theuth restarts after the grant",
                crate::tools::ACCESSIBILITY_NOT_GRANTED
            )
        });
    }
    let mut text = upstream?;
    if let Some(start) = text.find(" (CLI:") {
        let end = text[start..]
            .find(')')
            .map_or(text.len(), |i| start + i + 1);
        text.replace_range(start..end, "");
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, MutexGuard, PoisonError};
    use std::thread;
    use std::time::Instant;

    use agent_desktop_core::{
        ActionOps, InputOps, ObservationOps, RetryDisposition, SystemOps, WindowFilter, WindowInfo,
    };
    use serde_json::Value;

    use super::args::{WindowAction, WindowArgs, WindowsArgs};
    use super::*;

    /// Namespaces reclaimed by [`record`], across every test in the binary;
    /// each test looks only for its own fresh session id.
    static RECLAIMED: Mutex<Vec<String>> = Mutex::new(Vec::new());

    fn reclaims() -> MutexGuard<'static, Vec<String>> {
        RECLAIMED.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn record(namespace: &str) {
        reclaims().push(namespace.to_owned());
    }

    fn reclaimed(session: SessionId) -> bool {
        reclaims().contains(&session.to_string())
    }

    /// The four sub-traits with their defaults, plus `list_windows` keyed by
    /// the app filter: `ok` succeeds with no windows; `slow` and no filter
    /// (what `focus-window` sends) sleep 400 ms; anything else fails with the
    /// name and id of the thread it ran on.
    struct Fake;
    impl ObservationOps for Fake {
        fn list_windows(
            &self,
            filter: &WindowFilter,
            _deadline: agent_desktop_core::Deadline,
        ) -> Result<Vec<WindowInfo>, AdapterError> {
            match filter.app.as_deref() {
                Some("ok") => return Ok(Vec::new()),
                Some("slow") | None => thread::sleep(Duration::from_millis(400)),
                Some(_) => {}
            }
            let thread = thread::current();
            Err(AdapterError::new(
                ErrorCode::Internal,
                format!("{}:{:?}", thread.name().unwrap_or(""), thread.id()),
            ))
        }
    }
    impl ActionOps for Fake {}
    impl InputOps for Fake {}
    impl SystemOps for Fake {}

    fn a11y(idle_ms: u64, deadline_ms: u64) -> A11y {
        let fake = Backend {
            adapter: || Ok(Box::new(Fake)),
            open: |_, _| Ok(()),
            reclaim: record,
        };
        A11y::with(
            fake,
            "theuth:test".to_owned(),
            Duration::from_millis(idle_ms),
            Duration::from_millis(deadline_ms),
        )
    }

    fn list(app: &str) -> Request {
        Request::Windows(WindowsArgs {
            list: None,
            app: Some(app.to_owned()),
        })
    }

    fn parse(envelope: &str) -> Value {
        serde_json::from_str(envelope).unwrap()
    }

    /// The thread a `list` ran on, from the fake's error message.
    async fn thread_of(a: &A11y, session: SessionId) -> String {
        let err = a.run(session, list("fast")).await.unwrap_err();
        parse(&err)["message"].as_str().unwrap().to_owned()
    }

    #[tokio::test]
    async fn spawns_one_named_thread_per_session_on_first_use() {
        let a = a11y(60_000, 5_000);
        assert_eq!(a.workers.len(), 0);
        let (s1, s2) = (SessionId::new(), SessionId::new());
        let first = thread_of(&a, s1).await;
        let name = format!("computer-a11y-{}:", short(&s1));
        assert!(first.starts_with(&name), "{first}");
        assert_eq!(name.len(), "computer-a11y-:".len() + 8);
        assert_eq!(
            thread_of(&a, s1).await,
            first,
            "same thread for the session"
        );
        assert_ne!(thread_of(&a, s2).await, first, "own thread per session");
        assert_eq!(a.workers.len(), 2);
    }

    #[tokio::test]
    async fn a_successful_command_returns_compact_data() {
        let a = a11y(60_000, 5_000);
        assert_eq!(
            a.run(SessionId::new(), list("ok")).await,
            Ok("[]".to_owned())
        );
    }

    #[tokio::test]
    async fn forget_retires_and_reclaims_only_the_closing_sessions() {
        let a = a11y(60_000, 5_000);
        let (closing, other, idle) = (SessionId::new(), SessionId::new(), SessionId::new());
        thread_of(&a, closing).await;
        thread_of(&a, other).await;
        for handle in a.forget(&[closing, idle]) {
            handle.join().unwrap();
        }
        assert!(reclaimed(closing), "a live worker reclaims on retire");
        assert!(
            reclaimed(idle),
            "a session with no worker is reclaimed directly"
        );
        assert!(!reclaimed(other));
        assert_eq!(a.workers.len(), 1);
    }

    #[tokio::test]
    async fn an_idle_worker_retires_and_reclaims() {
        let a = a11y(50, 5_000);
        let session = SessionId::new();
        let first = thread_of(&a, session).await;
        let deadline = Instant::now() + Duration::from_secs(5);
        while !(reclaimed(session) && a.workers.len() == 0) {
            assert!(Instant::now() < deadline, "never idled out");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_ne!(thread_of(&a, session).await, first, "a fresh worker");
    }

    #[tokio::test]
    async fn a_timed_out_worker_is_abandoned_without_reclaim() {
        let a = a11y(60_000, 100);
        let session = SessionId::new();
        let first = thread_of(&a, session).await;
        let v = parse(&a.run(session, list("slow")).await.unwrap_err());
        assert_eq!(v["code"], "TIMEOUT");
        assert_eq!(v["retry"], "safe", "a read-only timeout sent nothing");
        assert!(
            v["message"].as_str().unwrap().contains("within 100ms"),
            "{v}"
        );
        assert_eq!(a.workers.len(), 0);
        assert_ne!(thread_of(&a, session).await, first, "a fresh worker");
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(!reclaimed(session), "the abandoned worker exits silently");
    }

    #[tokio::test]
    async fn a_mutating_timeout_may_have_landed() {
        let a = a11y(60_000, 100);
        let focus = Request::Window(WindowArgs {
            window_id: "w1".to_owned(),
            action: WindowAction::Focus,
            x: None,
            y: None,
            width: None,
            height: None,
        });
        let v = parse(&a.run(SessionId::new(), focus).await.unwrap_err());
        assert_eq!(v["code"], "TIMEOUT");
        assert_eq!(v["retry"], "unknown");
    }

    #[test]
    fn a_panicked_worker_reads_as_internal() {
        let error = failure_error(Failure::Panicked("boom".into()), DEADLINE);
        let v = parse(&envelope(&settle(error, false)));
        assert_eq!(v["code"], "INTERNAL");
        assert!(
            v["message"].as_str().unwrap().contains("panicked: boom"),
            "{v}"
        );
        assert!(
            v["suggestion"]
                .as_str()
                .unwrap()
                .contains("computer_snapshot")
        );
        assert_eq!(v["retry"], "unknown", "a mutation may have landed");
    }

    #[test]
    fn envelope_carries_code_message_suggestion_and_retry() {
        let v = parse(&envelope(&AppError::stale_ref("@s1:e3")));
        assert_eq!(v["code"], "STALE_REF");
        assert_eq!(v["message"], "@s1:e3 not found in current RefMap");
        assert_eq!(
            v["suggestion"], "Re-run a snapshot to obtain fresh refs, then retry with the new ref",
            "upstream's (CLI: …; FFI: …) tail is cut"
        );
        assert_eq!(v["retry"], "safe");
        let plain = AppError::Internal("boom".into());
        assert_eq!(
            parse(&envelope(&plain)),
            json!({"code": "INTERNAL", "message": "boom", "retry": "unknown"})
        );
    }

    #[test]
    fn perm_denied_speaks_of_theuth_not_the_agent_desktop_cli() {
        let v = parse(&envelope(&AdapterError::permission_denied().into()));
        assert_eq!(v["code"], "PERM_DENIED");
        if cfg!(target_os = "macos") {
            let s = v["suggestion"].as_str().unwrap();
            assert!(s.contains("the app that launched theuth"), "{s}");
            assert!(s.contains("until theuth restarts after the grant"), "{s}");
        } else {
            assert!(v.get("suggestion").is_none(), "{v}");
        }
        assert!(!v.to_string().contains("agent-desktop permissions"));
    }

    #[test]
    fn settle_makes_read_only_and_refused_errors_safe_and_leaves_mutations_unknown() {
        let retry = |e: AppError| ErrorPayload::from_app_error(&e).disposition.retry();
        let unknown = || AppError::from(AdapterError::new(ErrorCode::Internal, "x"));
        let refused = || AppError::from(AdapterError::new(ErrorCode::InvalidArgs, "x"));
        assert_eq!(retry(settle(unknown(), true)), RetryDisposition::Safe);
        assert_eq!(retry(settle(unknown(), false)), RetryDisposition::Unknown);
        assert_eq!(retry(settle(refused(), false)), RetryDisposition::Safe);
        let io = AppError::Io(std::io::ErrorKind::PermissionDenied.into());
        let settled = settle(io, true);
        assert_eq!(settled.code(), "PERM_DENIED");
        assert_eq!(retry(settled), RetryDisposition::Safe);
        let delivered = AppError::from(
            AdapterError::new(ErrorCode::Internal, "x")
                .with_disposition(DeliverySemantics::delivered_unverified()),
        );
        assert_eq!(retry(settle(delivered, true)), RetryDisposition::Unsafe);
    }

    #[tokio::test]
    async fn a_refused_request_never_reaches_a_worker() {
        let a = a11y(60_000, 5_000);
        let bad = Request::Find(args::FindArgs::default());
        let v = parse(&a.run(SessionId::new(), bad).await.unwrap_err());
        assert_eq!(v["code"], "INVALID_ARGS");
        assert_eq!(v["retry"], "safe");
        assert_eq!(a.workers.len(), 0);
    }

    #[test]
    fn a_theuth_session_id_is_a_valid_namespace() {
        let id = SessionId::new().to_string();
        assert!(agent_desktop_core::context::validate_session_id(&id).is_ok());
    }
}
