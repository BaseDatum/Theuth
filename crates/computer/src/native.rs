//! Native-app tools (153 §11): `applescript` and `shortcuts` on macOS,
//! `powershell` on Windows. Each execs the system binary directly with an
//! argument vector — no shell wrapper, so a script is text to the
//! interpreter, never a command line — detached from the engine's stdin,
//! with a timeout that kills the child. All three are [`Risk::Mutating`]:
//! `tools.policy.mutating` (`Ask` by default) decides.

use std::process::Stdio;
use std::time::{Duration, Instant};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
// Only the macOS and Windows tools use `Risk`, `Tool` and `ToolCx` (the tests use `Risk`).
#[cfg_attr(not(any(target_os = "macos", windows)), allow(unused_imports))]
use tools::{DynTool, Risk, Tool, ToolCx, ToolOutput};

/// Timeout when the call names none (§11.1).
const DEFAULT_TIMEOUT_SECS: u64 = 30;
/// `timeout_secs` ceiling (§11.1).
const MAX_TIMEOUT_SECS: u64 = 120;
/// Output cap per stream before "…(N more bytes)" (§11.1).
const MAX_OUTPUT_BYTES: usize = 16 * 1024;

/// What a native run hands back (also the `structured` dict).
#[derive(Debug, Serialize)]
pub struct Run {
    /// Exit status 0.
    pub ok: bool,
    /// Stdout, UTF-8 lossy, truncated at 16 KiB.
    pub stdout: String,
    /// Stderr, UTF-8 lossy, truncated at 16 KiB.
    pub stderr: String,
    /// Exit code; `None` when killed by a signal or the timeout.
    pub exit_code: Option<i32>,
    /// Wall time.
    pub ms: u64,
}

/// Cap `bytes` at [`MAX_OUTPUT_BYTES`] with a trailer naming the rest.
pub fn truncate(bytes: &[u8]) -> String {
    if bytes.len() <= MAX_OUTPUT_BYTES {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let mut end = MAX_OUTPUT_BYTES;
    while end > 0 && std::str::from_utf8(&bytes[..end]).is_err() {
        end -= 1;
    }
    format!(
        "{}…({} more bytes)",
        String::from_utf8_lossy(&bytes[..end]),
        bytes.len() - end
    )
}

/// `timeout_secs` → clamped duration.
fn timeout(secs: Option<u64>) -> Duration {
    Duration::from_secs(
        secs.unwrap_or(DEFAULT_TIMEOUT_SECS)
            .clamp(1, MAX_TIMEOUT_SECS),
    )
}

/// Exec `program` with `args` (no shell), feed `stdin` if given, wait at most
/// `timeout_secs`. `Err` only for spawn failures and the timeout; a non-zero
/// exit is an `Ok(Run { ok: false, .. })`.
pub async fn run(
    program: &str,
    args: &[&str],
    stdin: Option<&str>,
    timeout_secs: Option<u64>,
) -> Result<Run, String> {
    let started = Instant::now();
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| format!("{program}: {e}"))?;
    if let (Some(input), Some(mut pipe)) = (stdin, child.stdin.take()) {
        use tokio::io::AsyncWriteExt;
        let _ = pipe.write_all(input.as_bytes()).await;
        drop(pipe);
    }
    let limit = timeout(timeout_secs);
    let out = match tokio::time::timeout(limit, child.wait_with_output()).await {
        Ok(out) => out.map_err(|e| format!("{program}: {e}"))?,
        // `wait_with_output` consumed the child; `kill_on_drop` kills it.
        Err(_) => return Err(format!("{program}: timed out after {} s", limit.as_secs())),
    };
    Ok(Run {
        ok: out.status.success(),
        stdout: truncate(&out.stdout),
        stderr: truncate(&out.stderr),
        exit_code: out.status.code(),
        ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    })
}

/// [`Run`] → what the model reads (§11.3): stdout, then stderr, then one
/// trailer `exit N · T s`; `is_error` on a non-zero exit. `structured` is
/// the dict for python callers.
pub fn output(prefix: &str, run: Result<Run, String>) -> ToolOutput {
    let run = match run {
        Ok(run) => run,
        Err(e) => return ToolOutput::err(e),
    };
    let mut text = run.stdout.trim_end().to_owned();
    if !run.stderr.trim().is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(prefix);
        text.push_str(": ");
        text.push_str(run.stderr.trim_end());
    }
    if !text.is_empty() {
        text.push('\n');
    }
    let exit = run
        .exit_code
        .map_or_else(|| "signal".to_owned(), |c| c.to_string());
    let tenths = run.ms / 100;
    text.push_str(&format!("exit {exit} · {}.{} s", tenths / 10, tenths % 10));
    let mut out = if run.ok {
        ToolOutput::ok(text)
    } else {
        ToolOutput::err(text)
    };
    out.structured = serde_json::to_value(&run).ok();
    out
}

/// Arguments for [`AppleScriptTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct AppleScriptArgs {
    /// AppleScript source. Make it `return` one line you can quote.
    pub script: String,
    /// Kill the script after this many seconds (default 30, max 120).
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    /// `checking: …` / `executing: …` / `verifying: …` — shown on the command card (153 §10.2).
    #[serde(default)]
    pub purpose: Option<String>,
}

/// macOS: `osascript -e <script>`.
#[cfg(target_os = "macos")]
pub struct AppleScriptTool;

#[cfg(target_os = "macos")]
#[async_trait::async_trait]
impl Tool for AppleScriptTool {
    const NAME: &'static str = "applescript";
    const RISK: Risk = Risk::Mutating;
    const RETURNS: &'static str = "dict";
    type Args = AppleScriptArgs;

    fn description(&self) -> String {
        "Run an AppleScript on this Mac (Reminders, Notes, Calendar, Messages, Mail, Music, Finder, System Events). The script's return value and errors come back as text plus `exit N · T s`; `ok` is false on a non-zero exit. Mutating: it can send messages and change data — state the target in `purpose`. Read the native-apps skill for recipes.".to_owned()
    }

    async fn call(&self, a: Self::Args, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        Ok(output(
            "osascript",
            run("osascript", &["-e", &a.script], None, a.timeout_secs).await,
        ))
    }
}

/// Arguments for [`ShortcutsTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ShortcutsArgs {
    /// Shortcut name as shown in Shortcuts.app; `__list__` lists the user's shortcuts instead of running one.
    pub name: String,
    /// Text handed to the shortcut as its input (stdin).
    #[serde(default)]
    pub input: Option<String>,
    /// Kill the run after this many seconds (default 30, max 120).
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    /// `checking: …` / `executing: …` / `verifying: …` — shown on the command card (153 §10.2).
    #[serde(default)]
    pub purpose: Option<String>,
}

/// Name every [`ShortcutsTool`] call may use instead of a shortcut: `shortcuts list`.
pub const SHORTCUTS_LIST: &str = "__list__";

/// macOS: `shortcuts run <name> [-i -] -o -`, or `shortcuts list`.
#[cfg(target_os = "macos")]
pub struct ShortcutsTool;

#[cfg(target_os = "macos")]
#[async_trait::async_trait]
impl Tool for ShortcutsTool {
    const NAME: &'static str = "shortcuts";
    const RISK: Risk = Risk::Mutating;
    const RETURNS: &'static str = "dict";
    type Args = ShortcutsArgs;

    fn description(&self) -> String {
        "Run one of the user's Shortcuts by name, optionally with text `input`; its text output comes back as stdout plus `exit N · T s`. `name: \"__list__\"` lists the shortcuts (read-only). Mutating: a shortcut can do anything its actions allow — state what it does in `purpose`.".to_owned()
    }

    async fn call(&self, a: Self::Args, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let run = if a.name == SHORTCUTS_LIST {
            run("shortcuts", &["list"], None, a.timeout_secs).await
        } else {
            let mut args = vec!["run", a.name.as_str()];
            if a.input.is_some() {
                args.extend(["-i", "-"]);
            }
            args.extend(["-o", "-"]);
            run("shortcuts", &args, a.input.as_deref(), a.timeout_secs).await
        };
        Ok(output("shortcuts", run))
    }
}

/// Arguments for [`PowerShellTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct PowerShellArgs {
    /// PowerShell source. Make it write one line you can quote.
    pub script: String,
    /// Kill the script after this many seconds (default 30, max 120).
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    /// `checking: …` / `executing: …` / `verifying: …` — shown on the command card (153 §10.2).
    #[serde(default)]
    pub purpose: Option<String>,
}

/// Windows: `pwsh -NoProfile -NonInteractive -Command <script>`, falling
/// back to Windows PowerShell (`powershell`) when PowerShell 7 is absent.
#[cfg(windows)]
pub struct PowerShellTool;

#[cfg(windows)]
#[async_trait::async_trait]
impl Tool for PowerShellTool {
    const NAME: &'static str = "powershell";
    const RISK: Risk = Risk::Mutating;
    const RETURNS: &'static str = "dict";
    type Args = PowerShellArgs;

    fn description(&self) -> String {
        "Run PowerShell on this PC (Outlook mail and calendar via COM, Notepad, Explorer, Windows Settings). Stdout and errors come back as text plus `exit N · T s`; `ok` is false on a non-zero exit. Mutating: it can send mail and change data — state the target in `purpose`. Read the native-apps skill for recipes.".to_owned()
    }

    async fn call(&self, a: Self::Args, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let args = [
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            a.script.as_str(),
        ];
        let mut res = run("pwsh", &args, None, a.timeout_secs).await;
        if matches!(&res, Err(e) if e.contains("os error 2")) {
            res = run("powershell", &args, None, a.timeout_secs).await;
        }
        Ok(output("powershell", res))
    }
}

/// The native-app tools this platform has (§11.1): macOS 2, Windows 1, else 0.
#[must_use]
pub fn all() -> Vec<DynTool> {
    #[cfg(target_os = "macos")]
    {
        vec![
            std::sync::Arc::new(AppleScriptTool),
            std::sync::Arc::new(ShortcutsTool),
        ]
    }
    #[cfg(windows)]
    {
        vec![std::sync::Arc::new(PowerShellTool)]
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_per_platform() {
        let names: Vec<String> = all().iter().map(|t| t.spec().name).collect();
        let want: &[&str] = if cfg!(target_os = "macos") {
            &["applescript", "shortcuts"]
        } else if cfg!(windows) {
            &["powershell"]
        } else {
            &[]
        };
        assert_eq!(names, want);
    }

    /// §11.2: every native tool is `Mutating`, and the default policy row for
    /// `mutating` is `Ask` — so the first run asks.
    #[test]
    fn policy_asks_for_mutating_native_tool() {
        let specs: Vec<_> = all().iter().map(|t| t.spec()).collect();
        assert_eq!(specs.len(), all().len());
        for s in &specs {
            assert_eq!(s.risk, Risk::Mutating, "{}", s.name);
            assert_eq!(
                tools::Policy::default().action(s.risk),
                tools::PolicyAction::Ask,
                "{}",
                s.name
            );
            assert!(s.schema.to_string().contains("\"purpose\""), "{}", s.name);
        }
    }

    #[test]
    fn output_truncated() {
        let big = vec![b'a'; MAX_OUTPUT_BYTES + 100];
        let s = truncate(&big);
        assert!(s.starts_with(&"a".repeat(MAX_OUTPUT_BYTES)));
        assert!(s.ends_with("…(100 more bytes)"), "{}", &s[s.len() - 40..]);
        // Never splits a multi-byte char.
        let mut multi = vec![b'a'; MAX_OUTPUT_BYTES - 1];
        multi.extend("é".as_bytes());
        multi.push(b'z');
        let s = truncate(&multi);
        assert!(s.ends_with("…(3 more bytes)"), "{s}");
        assert_eq!(truncate(b"small"), "small");
    }

    #[test]
    fn output_shape_and_trailer() {
        let run = Run {
            ok: true,
            stdout: "2\n".into(),
            stderr: String::new(),
            exit_code: Some(0),
            ms: 1234,
        };
        let out = output("osascript", Ok(run));
        assert!(!out.is_error);
        assert_eq!(out.content, "2\nexit 0 · 1.2 s");
        let s = out.structured.unwrap();
        assert_eq!(s["ok"], true);
        assert_eq!(s["stdout"], "2\n");
        assert_eq!(s["exit_code"], 0);
        assert_eq!(s["ms"], 1234);

        let run = Run {
            ok: false,
            stdout: String::new(),
            stderr: "3:4: syntax error\n".into(),
            exit_code: Some(1),
            ms: 50,
        };
        let out = output("osascript", Ok(run));
        assert!(out.is_error);
        assert_eq!(out.content, "osascript: 3:4: syntax error\nexit 1 · 0.0 s");
        assert_eq!(out.structured.unwrap()["ok"], false);

        let out = output("osascript", Err("osascript: timed out after 1 s".into()));
        assert!(out.is_error && out.structured.is_none());
    }

    #[test]
    fn timeout_is_clamped() {
        assert_eq!(timeout(None), Duration::from_secs(30));
        assert_eq!(timeout(Some(0)), Duration::from_secs(1));
        assert_eq!(timeout(Some(500)), Duration::from_secs(120));
    }

    /// The script is one argv entry: a `; rm` inside it is text to the
    /// interpreter, never a shell command.
    #[cfg(unix)]
    #[tokio::test]
    async fn exec_without_shell() {
        let marker = std::env::temp_dir().join(format!("theuth-native-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let script = format!("x; rm -f {m}; echo pwned > {m}", m = marker.display());
        // `/bin/echo` prints its argv verbatim: a shell would have run the `;` chain.
        let r = run("/bin/echo", &[&script], None, Some(5)).await.unwrap();
        assert!(r.ok);
        assert_eq!(r.stdout.trim_end(), script);
        assert!(!marker.exists(), "argument was interpreted by a shell");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_kills() {
        let started = Instant::now();
        let err = run("/bin/sleep", &["10"], None, Some(1)).await.unwrap_err();
        assert_eq!(err, "/bin/sleep: timed out after 1 s");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stdin_is_fed_and_nonzero_exit_is_not_ok() {
        let r = run("/bin/cat", &[], Some("hello"), Some(5)).await.unwrap();
        assert!(r.ok);
        assert_eq!(r.stdout, "hello");
        let r = run("/bin/sh", &["-c", "exit 3"], None, Some(5))
            .await
            .unwrap();
        assert!(!r.ok);
        assert_eq!(r.exit_code, Some(3));
        assert!(run("/nonexistent/binary", &[], None, None).await.is_err());
    }

    /// Live `osascript`, no app launched (153-C2 brief).
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn applescript_returns_result_line() {
        let out = AppleScriptTool
            .call(
                AppleScriptArgs {
                    script: "return 1 + 1".into(),
                    timeout_secs: Some(10),
                    purpose: Some("checking: arithmetic".into()),
                },
                cx(),
            )
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.starts_with("2\nexit 0 · "), "{}", out.content);
        // A `;` chain is a syntax error to osascript, not a command.
        let out = AppleScriptTool
            .call(
                AppleScriptArgs {
                    script: "return \"x\"; echo pwned".into(),
                    timeout_secs: Some(10),
                    purpose: None,
                },
                cx(),
            )
            .await
            .unwrap();
        assert!(
            out.is_error && out.content.contains("syntax error"),
            "{}",
            out.content
        );
    }

    /// Live `pwsh` (or Windows PowerShell), no app launched (153-C2 hand-off: the
    /// Windows twin of `applescript_returns_result_line`).
    #[cfg(windows)]
    #[tokio::test]
    async fn powershell_returns_stdout() {
        let out = PowerShellTool
            .call(
                PowerShellArgs {
                    script: "Write-Output hello".into(),
                    timeout_secs: Some(30),
                    purpose: Some("checking: echo".into()),
                },
                cx(),
            )
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.starts_with("hello"), "{}", out.content);
        assert!(out.content.contains("exit 0 · "), "{}", out.content);
    }

    /// A non-zero exit is an error result carrying the exit code.
    #[cfg(windows)]
    #[tokio::test]
    async fn powershell_nonzero_exit_is_error() {
        let out = PowerShellTool
            .call(
                PowerShellArgs {
                    script: "exit 3".into(),
                    timeout_secs: Some(30),
                    purpose: None,
                },
                cx(),
            )
            .await
            .unwrap();
        assert!(out.is_error, "{}", out.content);
        assert!(out.content.contains("exit 3"), "{}", out.content);
    }

    #[cfg(any(target_os = "macos", windows))]
    fn cx() -> ToolCx {
        ToolCx {
            session: runtime::SessionId::new(),
            call_id: "1".into(),
            quiet: false,
            resources: tools::SessionResources::durable_arc(&std::env::temp_dir()),
            cancel: tools::CancellationToken::new(),
            promote_after: None,
        }
    }
}
