//! `t subprocess`: the Python runtime worker, inside this binary (design doc 08 §9).
//!
//! `monty-pool` spawns `<binary> subprocess` and speaks framed protobuf over
//! the child's stdio. The reference worker (`monty-runtime`'s
//! `src/subprocess.rs`, MIT, pydantic) is a thin shell around
//! `monty_proto::worker::Child`; this is that shell, so `theuth` needs no `monty`
//! install next to it. Crash isolation is unchanged: the worker is still a
//! separate process, and the parent treats an exit without `FatalError` as a crash.
//!
//! In this mode stdout carries only protocol frames; diagnostics go to stderr.
//! Nothing else in the process may run first: not logging, not the runtime.

use std::{io, panic};

use monty_proto::worker::{Child, EventSink, HandleOutcome, fatal_error_event, protocol_violation};
use monty_proto::{FrameError, FrameReader, pb, write_frame};

/// BSD `sysexits.h` "remote error in protocol": the frame stream desynchronised,
/// or an event too large to frame left the response unsendable.
const EX_PROTOCOL: u8 = 76;

/// Runs the child loop until EOF, `Shutdown`, or a fatal error; returns the exit code.
pub fn run() -> u8 {
    install_panic_hook();
    let mut reader = FrameReader::new(io::stdin().lock());
    let mut child = Child::default();
    let mut sink = StdoutSink;

    loop {
        match reader.read::<pb::ParentRequest>() {
            Ok(Some(request)) => {
                let outcome = child.handle(request, &mut sink);
                apply_memory_limit(&child);
                match outcome {
                    Ok(HandleOutcome::Continue) => {}
                    Ok(HandleOutcome::Shutdown) => return 0,
                    // The child emitted a FatalError (e.g. version skew) and cannot
                    // keep serving; exit non-zero so the parent sees a clean cause.
                    Ok(HandleOutcome::Fatal) => return 4,
                    // Rejected before any bytes hit the wire: the stream is still in
                    // sync and the parent can receive a parseable last gasp.
                    Err(FrameError::FrameTooLarge { len, max }) => {
                        fatal(
                            &child,
                            &mut sink,
                            &format!(
                                "response frame of {len} bytes exceeds maximum of {max} bytes"
                            ),
                        );
                        return EX_PROTOCOL;
                    }
                    // Writing to stdout failed: the parent is gone.
                    Err(_) => return 3,
                }
            }
            // Clean EOF at a frame boundary: the parent closed stdin.
            Ok(None) => return 0,
            // Intact frame, undecodable payload (includes semantic validation);
            // the stream is in sync, so answer with a turn-ending error and go on.
            Err(FrameError::Decode(err)) => {
                if sink
                    .send(&protocol_violation(&format!("malformed request: {err}")))
                    .is_err()
                {
                    return 3;
                }
            }
            // Desynchronised: unrecoverable by design.
            Err(err) => {
                fatal(
                    &child,
                    &mut sink,
                    &format!("malformed request frame: {err}"),
                );
                return EX_PROTOCOL;
            }
        }
    }
}

/// Applies the memory limit of whatever session the child now holds, after
/// every request; reading the child's state (not the request) keeps a rejected
/// `Configure`, a restored dump's own limits and a `Reset` honest.
fn apply_memory_limit(child: &Child) {
    let budget = child.session_budget();
    monty_alloc::set_limit(budget.max_memory, budget.type_check)
        .expect("theuth installs monty_alloc::LimitedAllocator as the global allocator");
}

/// Framed child events to stdout. Framing is stateless and `Stdout` handles
/// share one global buffer, so a fresh handle per write is safe.
struct StdoutSink;

impl EventSink for StdoutSink {
    fn send(&mut self, event: &pb::ChildEvent) -> Result<(), FrameError> {
        write_frame(&mut io::stdout(), event)
    }
}

/// Best-effort `FatalError` event, duplicated to stderr; the child exits right after.
fn fatal(child: &Child, sink: &mut impl EventSink, message: &str) {
    eprintln!("t subprocess fatal error: {message}");
    let _ = sink.send(&child.fatal_event(message));
}

/// A best-effort `FatalError` frame before the default unwind, so ordinary
/// panics give the parent a parseable last gasp. Hard crashes (stack overflow,
/// allocator abort) bypass this; the parent's contract is "exit without
/// FatalError == crash".
fn install_panic_hook() {
    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        // A panic mid-write may leave a partial frame on stdout; a corrupt tail is
        // fine, the parent already treats it as a crash.
        let _ = write_frame(
            &mut io::stdout(),
            &fatal_error_event(&format!("child panicked: {info}")),
        );
        default_hook(info);
    }));
}
