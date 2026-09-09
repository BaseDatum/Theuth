//! `shell` tool backed by an embedded `brush_core::Shell` per session.
//! See design doc §6 and §11.3 for the capture, timeout and orphan-kill rules.

use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Risk, Tool, ToolCx, ToolOutput};

/// Arguments for [`ShellTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ShellArgs {
    /// Bash command line. State (cwd, variables, functions) persists across
    /// calls within a session.
    pub command: String,
}

/// Runs bash in-process; one persistent shell per session.
pub struct ShellTool {
    /// Kill the command's process group after this long.
    pub timeout: Duration,
}

impl ShellTool {
    /// Construct with the configured timeout.
    pub fn new(timeout: Duration) -> Self {
        Self { timeout }
    }
}

#[async_trait::async_trait]
impl Tool for ShellTool {
    const NAME: &'static str = "shell";
    const RISK: Risk = Risk::Destructive;
    type Args = ShellArgs;

    fn description(&self) -> String {
        "Run a bash command. cwd, variables and functions persist between calls. Output is stdout+stderr with the exit code.".into()
    }

    fn summary(&self, args: &ShellArgs) -> String {
        // Replaced by the AST-derived command list in the shell implementation.
        format!("shell: {}", args.command)
    }

    async fn call(&self, args: ShellArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let _ = (args, cx);
        todo!("shell implementation")
    }
}
