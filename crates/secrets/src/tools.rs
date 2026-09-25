//! The `secret_list`, `secret_request`, `secret_capture` and `secret_restore`
//! tools and the plugin's prompt block (design doc 37 §3.2, §4), registered by
//! `SecretsPlugin::start` once the tool registry is up. None of them returns a
//! value, ever: names, notes, version numbers, dates and byte counts only.

use std::sync::Arc;
use std::time::Duration;

use kameo::actor::ActorRef;
use kameo::error::SendError;
use schemars::JsonSchema;
use serde::Deserialize;
use tools::{
    Invoke, REGISTRY_ACTOR, RegisterPrompt, RegisterTool, Risk, Tool, ToolCall, ToolCx, ToolOutput,
    ToolRegistry,
};

use crate::vault::{self, MIN_REDACT, SecretInfo, Vault};
use crate::{Request, SecretsActor};

/// The plugin's system-prompt block (37 §4). Static, so it never busts the
/// system cache.
pub const PROMPT: &str = "Named secrets (API keys, tokens, passwords — including the harness's own provider keys) live in an \
encrypted vault you can use but never read. `secret_list` names them with their versions. Write `{{secret:NAME}}` \
inside any tool argument — a shell command, a header, a URL, file content, an MCP tool's field — and the harness \
swaps in the value when the tool runs; you, the transcript and the user's approval card see only the placeholder, \
and the value is scrubbed from tool output if it echoes back. `{{secret:NAME:-1}}` is the previous version (a token \
you just rotated). In Python keep the placeholder in a plain string, not an f-string (`{{` becomes `{`). Need a \
secret that does not exist? `secret_request` asks the user to type it into a masked field; `secret_capture` runs a \
tool and stores its output (a fresh token from a login command, a key from a file) without showing it to you — \
`replace: true` to update one that exists; `secret_restore` undoes a bad update. Never ask the user to paste a \
secret into the chat, and never try to print one.";

/// Register the four tools and the prompt block on `reg`. `timeout` is the
/// actor's `request_timeout_secs`, for the `no answer after N s` receipt.
pub async fn register(
    reg: &ActorRef<ToolRegistry>,
    vault: Arc<Vault>,
    actor: ActorRef<SecretsActor>,
    timeout: Duration,
) {
    let _ = reg
        .ask(RegisterTool(Arc::new(SecretListTool(vault.clone()))))
        .await;
    let _ = reg
        .ask(RegisterTool(Arc::new(SecretRequestTool { actor, timeout })))
        .await;
    let _ = reg
        .ask(RegisterTool(Arc::new(SecretCaptureTool(vault.clone()))))
        .await;
    let _ = reg
        .ask(RegisterTool(Arc::new(SecretRestoreTool(vault))))
        .await;
    let _ = reg
        .ask(RegisterPrompt {
            plugin: "secrets",
            text: PROMPT.to_owned(),
        })
        .await;
}

/// The secret named `name`, if the vault has it.
fn find(vault: &Vault, name: &str) -> Option<SecretInfo> {
    vault.list().into_iter().find(|s| s.name == name)
}

/// `secret_list`: names, notes, versions. Never values.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListArgs {}

/// The `secret_list` tool.
pub struct SecretListTool(pub Arc<Vault>);

#[async_trait::async_trait]
impl Tool for SecretListTool {
    const NAME: &'static str = "secret_list";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "list[dict]";
    type Args = ListArgs;

    fn description(&self) -> String {
        "List the secrets in the vault: name, current version, how many versions are kept, note and \
         when it was last updated. Never a value. Use a name as `{{secret:NAME}}` in any tool argument."
            .into()
    }

    async fn call(&self, _: ListArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let list = self.0.list();
        if list.is_empty() {
            return Ok(ToolOutput::ok(
                "no secrets; the user adds them with /secrets add, Settings › Secrets or `t --secret-set`, \
                 or call secret_request",
            ));
        }
        let lines: Vec<String> = list
            .iter()
            .map(|s| {
                let note = if s.note.is_empty() {
                    String::new()
                } else {
                    format!(" · {}", s.note)
                };
                format!(
                    "{} · v{} ({} kept){note} · {} · {}",
                    s.name,
                    s.current,
                    s.versions.len(),
                    s.updated,
                    s.source
                )
            })
            .collect();
        let mut out = ToolOutput::ok(lines.join("\n"));
        out.structured = serde_json::to_value(&list).ok();
        Ok(out)
    }
}

/// `secret_request`: ask the user to type a secret into a masked field.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct RequestArgs {
    /// Name to store it under. An existing name gets a new version; the user is told.
    pub name: String,
    /// What it is for, shown to the user (`GitHub personal access token with repo scope`).
    pub note: String,
}

/// The `secret_request` tool: hands the ask to the actor and reports the receipt.
pub struct SecretRequestTool {
    /// The secrets actor.
    pub actor: ActorRef<SecretsActor>,
    /// How long the actor waits; only for the receipt text.
    pub timeout: Duration,
}

#[async_trait::async_trait]
impl Tool for SecretRequestTool {
    const NAME: &'static str = "secret_request";
    const RISK: Risk = Risk::ReadOnly;
    const DIRECT: bool = true;
    type Args = RequestArgs;

    fn description(&self) -> String {
        "Ask the user to type a secret into a masked field and store it under `name`; you never see \
         the value. Say in `note` what it is for. Returns `stored NAME vN`, `declined` or `no answer`. \
         Never ask the user to paste a secret into the chat instead."
            .into()
    }

    async fn call(&self, a: RequestArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        if !vault::valid_name(&a.name) {
            return Ok(ToolOutput::err("name: [A-Za-z0-9_.-]{1,64}"));
        }
        let name = a.name;
        let out = match self
            .actor
            .ask(Request {
                session: cx.session,
                name: name.clone(),
                note: a.note,
            })
            .await
        {
            Ok(o) => o,
            Err(SendError::HandlerError(e)) => return Ok(ToolOutput::err(e)),
            Err(e) => return Ok(ToolOutput::err(format!("secrets actor: {e}"))),
        };
        Ok(match out.stored {
            Some(v) if v > 1 => ToolOutput::ok(format!(
                "stored {name} v{v} (v{} is {{{{secret:{name}:-1}}}})",
                v - 1
            )),
            Some(v) => ToolOutput::ok(format!("stored {name} v{v}")),
            None if out.timed_out => {
                ToolOutput::err(format!("no answer after {} s", self.timeout.as_secs()))
            }
            None => ToolOutput::err("declined"),
        })
    }
}

/// `secret_capture`: run one tool and store its output as a secret without seeing it.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CaptureArgs {
    /// Name to store it under.
    pub name: String,
    /// Tool to run (`shell`, `read_file`, `read_url`, an MCP tool).
    pub tool: String,
    /// Its arguments, exactly as you would call it. Placeholders are allowed.
    pub args: serde_json::Value,
    /// Regex; the first capture group is the value. Default: the whole output, trimmed.
    pub pattern: Option<String>,
    /// Shown in `secret_list`. Kept from the previous version when absent.
    pub note: Option<String>,
    /// Required to add a version to a name that exists (guards against a typo landing on the wrong name).
    #[serde(default)]
    pub replace: bool,
}

/// The `secret_capture` tool.
pub struct SecretCaptureTool(pub Arc<Vault>);

/// Shell output ends in `exit code: N`; the value is the text before it.
fn strip_exit_code(content: &str) -> &str {
    content
        .rsplit_once("exit code: ")
        .filter(|(_, code)| code.trim().parse::<i32>().is_ok())
        .map_or(content, |(body, _)| body)
}

#[async_trait::async_trait]
impl Tool for SecretCaptureTool {
    const NAME: &'static str = "secret_capture";
    const RISK: Risk = Risk::ReadOnly;
    type Args = CaptureArgs;

    fn description(&self) -> String {
        "Run one tool and store its output as a secret without it entering the conversation: a fresh \
         token from a login command, a key from a file or a URL. The whole output, trimmed, is the \
         value, or the first capture group of `pattern`. The inner tool takes its own approval. \
         Returns a receipt with a byte count, never the value. `replace: true` is required to add a \
         version to a name that exists; `secret_restore` undoes it."
            .into()
    }

    async fn call(&self, a: CaptureArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        if !vault::valid_name(&a.name) {
            return Ok(ToolOutput::err("name: [A-Za-z0-9_.-]{1,64}"));
        }
        let vault = &self.0;
        if let Some(cur) = find(vault, &a.name).map(|s| s.current)
            && !a.replace
        {
            return Ok(ToolOutput::err(format!(
                "{} exists (v{cur}); pass replace: true to add v{}, or pick another name",
                a.name,
                cur + 1
            )));
        }
        let pattern = match a.pattern.as_deref().map(regex::Regex::new) {
            Some(Err(e)) => return Ok(ToolOutput::err(format!("pattern: {e}"))),
            Some(Ok(re)) => Some(re),
            None => None,
        };
        let Ok(Some(reg)) = ActorRef::<ToolRegistry>::lookup(REGISTRY_ACTOR) else {
            return Ok(ToolOutput::err("tool registry unavailable"));
        };
        let inner = reg
            .ask(Invoke(ToolCall {
                id: format!("{}.capture", cx.call_id),
                name: a.tool.clone(),
                args: a.args,
                session: cx.session,
                quiet: true,
            }))
            .await?;
        // From here on `inner` is never logged or returned: its text is the value.
        if inner.is_error {
            return Ok(ToolOutput::err(format!(
                "{} returned an error ({} bytes); output withheld — run it without secret_capture to \
                 debug, knowing its output then enters the conversation",
                a.tool,
                inner.content.len()
            )));
        }
        let raw = match inner.structured {
            Some(serde_json::Value::String(s)) => s,
            _ => strip_exit_code(&inner.content).to_owned(),
        };
        let value = match &pattern {
            Some(re) => match re.captures(&raw).and_then(|c| c.get(1)) {
                Some(m) => m.as_str(),
                None => {
                    return Ok(ToolOutput::err(format!(
                        "pattern matched nothing in {} bytes",
                        raw.len()
                    )));
                }
            },
            None => raw.trim(),
        };
        if value.is_empty() {
            return Ok(ToolOutput::err("empty output"));
        }
        let info = match vault.set(&a.name, value, a.note.as_deref(), &a.tool).await {
            Ok(i) => i,
            Err(e) => return Ok(ToolOutput::err(e)),
        };
        let mut receipt = format!(
            "stored {} v{} ({} bytes) from {}",
            a.name,
            info.current,
            value.len(),
            a.tool
        );
        if let Some(prev) = info.versions.get(1) {
            receipt.push_str(&format!(
                "; v{} is {{{{secret:{}:-1}}}}, secret_restore undoes",
                prev.v, a.name
            ));
        }
        if value.chars().count() < MIN_REDACT {
            receipt.push_str(&format!(
                "\nvalue shorter than {MIN_REDACT} characters cannot be scrubbed from tool output"
            ));
        }
        Ok(ToolOutput::ok(receipt))
    }
}

/// `secret_restore`: make an older version current again (as a new version; nothing is lost).
#[derive(Debug, Deserialize, JsonSchema)]
pub struct RestoreArgs {
    /// The secret.
    pub name: String,
    /// How many versions back, 1..=4 — the `:-N` of the placeholder that reaches it.
    pub back: u8,
}

/// The `secret_restore` tool.
pub struct SecretRestoreTool(pub Arc<Vault>);

#[async_trait::async_trait]
impl Tool for SecretRestoreTool {
    const NAME: &'static str = "secret_restore";
    const RISK: Risk = Risk::ReadOnly;
    type Args = RestoreArgs;

    fn description(&self) -> String {
        "Make an older version of a secret current again, as a new version (nothing is lost). `back` \
         is how many versions back, 1..=4, the `:-N` of the placeholder that reaches it."
            .into()
    }

    async fn call(&self, a: RestoreArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let vault = &self.0;
        let from =
            find(vault, &a.name).and_then(|s| s.versions.get(usize::from(a.back)).map(|v| v.v));
        let info = match vault.restore(&a.name, a.back).await {
            Ok(i) => i,
            Err(e) => return Ok(ToolOutput::err(e)),
        };
        let mut receipt = format!("{} v{}", a.name, info.current);
        if let Some(from) = from {
            receipt.push_str(&format!(" = v{from}"));
        }
        receipt.push_str(" restored");
        if let Some(prev) = info.versions.get(1) {
            receipt.push_str(&format!("; v{} is {{{{secret:{}:-1}}}}", prev.v, a.name));
        }
        Ok(ToolOutput::ok(receipt))
    }
}

#[cfg(test)]
mod tests {
    use super::strip_exit_code;

    #[test]
    fn exit_code_line_is_stripped_only_when_present() {
        assert_eq!(strip_exit_code("tok\nexit code: 0"), "tok\n");
        assert_eq!(strip_exit_code("exit code: 1"), "");
        assert_eq!(strip_exit_code("plain text"), "plain text");
        assert_eq!(strip_exit_code("exit code: soon\n"), "exit code: soon\n");
    }
}
