//! What the OS provides, once (design doc 58 §0.2, §3.1): process trees,
//! owner-only files, open-with-default-handler, service units, the login
//! shell and PTY program, the login-shell `PATH`, the ONNX Runtime library
//! name and the supported-target matrix. A leaf: no `runtime`, no actors.
//! Feature crates call these instead of carrying their own `#[cfg]`.

pub mod env;
pub mod files;
pub mod open;
pub mod ort;
pub mod process;
pub mod service;
pub mod sha256;
pub mod shell;
pub mod target;

pub use process::{ProcessTree, Signal};
pub use shell::{ShellKind, pty_command, shell};
