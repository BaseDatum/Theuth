//! Owner: B2 — the agent-facing `plugin_*` tools (contract §7.1). Agents can never approve: no
//! tool here sends `Approve`, and none can build the press proof it needs (`press.rs`).
//!
//! Every tool is one `ask` to [`PLUGINS_ACTOR`]; a missing actor answers "plugins is not running",
//! a refusal its [`PluginsError`] text. Output: a short text plus `structured` = the reply as JSON.
//! Descriptions and the prompt are constants (no config in them: the system-prompt cache).
// `Result<_, ToolOutput>` is the tool-body idiom (the `Err` is what the model reads), as in
// `sandbox/src/tools.rs`.
#![allow(clippy::result_large_err)]

use std::path::PathBuf;
use std::sync::Arc;

use kameo::actor::ActorRef;
use kameo::message::Message;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tools::{RegisterPrompt, RegisterTool, Risk, Tool, ToolCx, ToolOutput, ToolRegistry};

use crate::PLUGINS_ACTOR;
use crate::actor::PluginsActor;
use crate::error::PluginsError;
use crate::messages::*;
use crate::types::*;

/// The prompt block (contract §7.1, verbatim).
pub(crate) const PROMPT: &str = "External plugins (design 216) are folders under the data dir's `plugins/` with a `plugin.yaml`; `plugin_new` scaffolds one (SDK in py, js, sh, go, rb or rs), `plugin_install` copies one in, `plugin_list`/`plugin_status`/`plugin_logs` inspect them, `plugin_enable`/`plugin_disable`/`plugin_restart`/`plugin_uninstall` manage them. Nothing new runs until the owner approves it in theuth or with `t plugin approve`; you cannot approve — use `plugin_approve_request` to put the card in front of the owner and say so.";

/// What `plugin_approve_request` adds after the card.
const OWNER_MUST_APPROVE: &str = "the owner must press Approve in theuth or run `t plugin approve {name}`; agents cannot approve";

/// Register the agent-facing tools of §7.1 once the tool registry answers.
pub(crate) async fn register(rt: runtime::Runtime) {
    let Some(reg) = rt.lookup_when::<ToolRegistry>(tools::REGISTRY_ACTOR).await else {
        return;
    };
    register_on(&reg).await;
}

async fn register_on(reg: &ActorRef<ToolRegistry>) {
    let _ = reg.ask(RegisterTool(Arc::new(ListTool))).await;
    let _ = reg.ask(RegisterTool(Arc::new(StatusTool))).await;
    let _ = reg.ask(RegisterTool(Arc::new(LogsTool))).await;
    let _ = reg.ask(RegisterTool(Arc::new(EnableTool))).await;
    let _ = reg.ask(RegisterTool(Arc::new(DisableTool))).await;
    let _ = reg.ask(RegisterTool(Arc::new(RestartTool))).await;
    let _ = reg.ask(RegisterTool(Arc::new(NewTool))).await;
    let _ = reg.ask(RegisterTool(Arc::new(InstallTool))).await;
    let _ = reg.ask(RegisterTool(Arc::new(UninstallTool))).await;
    let _ = reg.ask(RegisterTool(Arc::new(ApproveRequestTool))).await;
    let _ = reg
        .ask(RegisterPrompt {
            plugin: "plugins",
            text: PROMPT.to_owned(),
        })
        .await;
}

fn actor() -> Result<ActorRef<PluginsActor>, ToolOutput> {
    match ActorRef::<PluginsActor>::lookup(PLUGINS_ACTOR) {
        Ok(Some(a)) => Ok(a),
        _ => Err(ToolOutput::err("plugins is not running")),
    }
}

/// `ask` a fallible message (directly or delegated); the refusal is what the model reads.
async fn ask<M, R>(msg: M) -> Result<R, ToolOutput>
where
    PluginsActor: Message<M>,
    <PluginsActor as Message<M>>::Reply: kameo::Reply<Ok = R, Error = PluginsError>,
    M: Send + 'static,
    R: Send + 'static,
{
    match actor()?.ask(msg).await {
        Ok(r) => Ok(r),
        Err(kameo::error::SendError::HandlerError(e)) => Err(ToolOutput::err(e.to_string())),
        Err(e) => Err(ToolOutput::err(format!("plugins: {e}"))),
    }
}

/// `text` plus the reply as `structured`.
fn out<T: Serialize>(text: impl Into<String>, reply: &T) -> ToolOutput {
    let mut o = ToolOutput::ok(text);
    o.structured = serde_json::to_value(reply).ok();
    o
}

/// The serde name of a unit enum value (`awaiting_approval`, `read_only`, …).
fn tag<T: Serialize>(v: &T) -> String {
    match serde_json::to_value(v) {
        Ok(serde_json::Value::String(s)) => s,
        _ => String::new(),
    }
}

/// `name version state[ · needs approval][ · error]`.
fn info_line(i: &PluginInfo) -> String {
    let mut s = format!("{} {} {}", i.name, i.version, tag(&i.state));
    if i.needs_approval {
        s.push_str(" · needs approval");
    }
    if let Some(e) = &i.error {
        s.push_str(" · ");
        s.push_str(e);
    }
    s
}

/// The card's lines, one per row: `- [risk] text (new)`.
fn card_lines(card: &InstallCard) -> String {
    let mut s = format!("{} card for {} {}:", tag(&card.kind), card.plugin, card.version);
    for l in &card.lines {
        s.push_str(&format!("\n- [{}] {}{}", tag(&l.risk), l.text, if l.new { " (new)" } else { "" }));
    }
    if !card.changed_paths.is_empty() {
        s.push_str(&format!("\nchanged: {}", card.changed_paths.join(", ")));
    }
    if card.unverified {
        s.push_str(&format!("\nthe saved approval for {} could not be verified", card.plugin));
    }
    s
}

macro_rules! tool_impl {
    ($ty:ident, $name:literal, $risk:expr, $args:ty, $desc:literal, |$a:ident| $body:expr) => {
        /// The `
        #[doc = $name]
        /// ` agent tool.
        pub(crate) struct $ty;

        #[async_trait::async_trait]
        impl Tool for $ty {
            const NAME: &'static str = $name;
            const RISK: Risk = $risk;
            const CONSUMES_SECRETS: bool = false;
            const DIRECT: bool = false;
            const HUMAN_ONLY: bool = false;
            const RETURNS: &'static str = "dict";
            type Args = $args;

            fn description(&self) -> String {
                $desc.into()
            }

            async fn call(&self, $a: $args, _: ToolCx) -> anyhow::Result<ToolOutput> {
                let r: Result<ToolOutput, ToolOutput> = $body.await;
                Ok(r.unwrap_or_else(|e| e))
            }
        }
    };
}

/// No arguments.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct NoArgs {}

/// One plugin.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct NameArgs {
    /// The plugin name (its folder under plugins/).
    pub name: String,
}

/// `plugin_logs`.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct LogsArgs {
    /// The plugin name.
    pub name: String,
    /// How many of the last lines of today's log (default 100, at most 2000).
    #[serde(default = "default_tail")]
    pub tail: u32,
}

fn default_tail() -> u32 {
    100
}

/// `plugin_new`.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct NewArgs {
    /// The new plugin's name: lowercase letters, digits and '-', starting with a letter.
    pub name: String,
    /// The SDK language (default py).
    #[serde(default)]
    pub lang: Lang,
    /// No SDK: the raw protocol template (py and sh only).
    #[serde(default)]
    pub raw: bool,
}

/// `plugin_install`.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct InstallArgs {
    /// Absolute path of a folder holding a plugin.yaml.
    pub path: String,
}

/// `plugin_uninstall`.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct UninstallArgs {
    /// The plugin name.
    pub name: String,
    /// Keep plugin-data/<name> instead of moving it to the trash too (default false).
    #[serde(default)]
    pub keep_data: bool,
}

/// `plugin_list` is the one tool that answers a list.
pub(crate) struct ListTool;

#[async_trait::async_trait]
impl Tool for ListTool {
    const NAME: &'static str = "plugin_list";
    const RISK: Risk = Risk::ReadOnly;
    const CONSUMES_SECRETS: bool = false;
    const DIRECT: bool = false;
    const HUMAN_ONLY: bool = false;
    const RETURNS: &'static str = "list[dict]";
    type Args = NoArgs;

    fn description(&self) -> String {
        "List the external plugins (folders under plugins/) with version, state, whether one waits for the owner's approval, and the last error.".into()
    }

    async fn call(&self, _: NoArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        let actor = match actor() {
            Ok(a) => a,
            Err(e) => return Ok(e),
        };
        let list = match actor.ask(ListPlugins).await {
            Ok(l) => l,
            Err(e) => return Ok(ToolOutput::err(format!("plugins: {e}"))),
        };
        let text = if list.is_empty() {
            "no plugins".to_owned()
        } else {
            list.iter().map(info_line).collect::<Vec<_>>().join("\n")
        };
        Ok(out(text, &list))
    }
}

tool_impl!(
    StatusTool,
    "plugin_status",
    Risk::ReadOnly,
    NameArgs,
    "One plugin's state, last error, the pending approval card (what it asks for) and the last stderr lines.",
    |a| async move {
        let d: PluginDetail = ask(GetPlugin { name: a.name }).await?;
        let mut text = info_line(&d.info);
        if let Some(card) = &d.card {
            text.push('\n');
            text.push_str(&card_lines(card));
        }
        let tail = &d.stderr_tail[d.stderr_tail.len().saturating_sub(10)..];
        if !tail.is_empty() {
            text.push_str("\nstderr:\n");
            text.push_str(&tail.join("\n"));
        }
        Ok(out(text, &d))
    }
);

tool_impl!(
    LogsTool,
    "plugin_logs",
    Risk::ReadOnly,
    LogsArgs,
    "The last lines of a plugin's log today (stderr and host notes, secrets redacted).",
    |a| async move {
        let t: LogTail = ask(Logs {
            name: a.name,
            tail: a.tail.min(2000),
        })
        .await?;
        let text = if t.lines.is_empty() { format!("{}: empty log", t.path) } else { t.lines.join("\n") };
        Ok(out(text, &t))
    }
);

tool_impl!(
    EnableTool,
    "plugin_enable",
    Risk::Mutating,
    NameArgs,
    "Enable a plugin: it starts if the owner approved exactly its current manifest and code; otherwise it stays awaiting approval.",
    |a| async move {
        let i: PluginInfo = ask(SetEnabled { name: a.name, on: true }).await?;
        let text = match i.state {
            PluginRunState::AwaitingApproval => "awaiting approval — the owner must approve first".to_owned(),
            PluginRunState::Starting | PluginRunState::Running => "started".to_owned(),
            ref s => format!("enabled; state {}", tag(s)),
        };
        Ok(out(text, &i))
    }
);

tool_impl!(
    DisableTool,
    "plugin_disable",
    Risk::Mutating,
    NameArgs,
    "Disable a plugin: stop its process and keep it stopped across restarts.",
    |a| async move {
        let i: PluginInfo = ask(SetEnabled { name: a.name, on: false }).await?;
        Ok(out("stopped", &i))
    }
);

tool_impl!(
    RestartTool,
    "plugin_restart",
    Risk::Mutating,
    NameArgs,
    "Restart a plugin's process (also clears a crash loop). Returns its new state.",
    |a| async move {
        let i: PluginInfo = ask(Restart { name: a.name }).await?;
        Ok(out(tag(&i.state), &i))
    }
);

tool_impl!(
    NewTool,
    "plugin_new",
    Risk::Mutating,
    NewArgs,
    "Scaffold a new plugin folder plugins/<name>/ from the bundled SDK template (py, js, sh, go, rb or rs; raw: no SDK, py or sh). Writes files only; nothing runs until the owner approves it.",
    |a| async move {
        let r: ScaffoldReport = ask(New {
            name: a.name,
            lang: a.lang,
            raw: a.raw,
        })
        .await?;
        Ok(out(format!("wrote {}: {}\n{}", r.dir, r.files.join(", "), r.next), &r))
    }
);

tool_impl!(
    InstallTool,
    "plugin_install",
    Risk::Mutating,
    InstallArgs,
    "Copy a local plugin folder (absolute path, with a plugin.yaml) into plugins/ and file its install card for the owner. Runs nothing.",
    |a| async move {
        let path = PathBuf::from(&a.path);
        if !path.is_absolute() {
            return Err(ToolOutput::err(format!("path must be absolute: {}", a.path)));
        }
        let r: InstallReport = ask(Install { path }).await?;
        Ok(out(format!("installed {} at {}\n{}", r.name, r.dir, r.next), &r))
    }
);

tool_impl!(
    UninstallTool,
    "plugin_uninstall",
    Risk::Destructive,
    UninstallArgs,
    "Uninstall a plugin: stop it, drop its approval, move its folder (and its data unless keep_data) to plugins/.trash. Only the owner can approve it again.",
    |a| async move {
        let name = a.name.clone();
        ask::<_, ()>(Uninstall {
            name: a.name,
            keep_data: a.keep_data,
        })
        .await?;
        Ok(out("uninstalled; folder in plugins/.trash", &serde_json::json!({ "name": name })))
    }
);

tool_impl!(
    ApproveRequestTool,
    "plugin_approve_request",
    Risk::ReadOnly,
    NameArgs,
    "Put a plugin's approval card in front of the owner again (an inbox item). Does not approve: only the owner can, in theuth or with `t plugin approve`.",
    |a| async move {
        let card: InstallCard = ask(RequestApproval { name: a.name }).await?;
        let text = format!("{}\n{}", card_lines(&card), OWNER_MUST_APPROVE.replace("{name}", &card.plugin));
        Ok(out(text, &card))
    }
);

#[cfg(test)]
mod tests {
    use super::*;

    fn schema<T: JsonSchema>() -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(T)).unwrap()
    }

    #[test]
    fn names_and_risks_match_the_contract() {
        let all = [
            (ListTool::NAME, ListTool::RISK),
            (StatusTool::NAME, StatusTool::RISK),
            (LogsTool::NAME, LogsTool::RISK),
            (EnableTool::NAME, EnableTool::RISK),
            (DisableTool::NAME, DisableTool::RISK),
            (RestartTool::NAME, RestartTool::RISK),
            (NewTool::NAME, NewTool::RISK),
            (InstallTool::NAME, InstallTool::RISK),
            (UninstallTool::NAME, UninstallTool::RISK),
            (ApproveRequestTool::NAME, ApproveRequestTool::RISK),
        ];
        let want = [
            ("plugin_list", Risk::ReadOnly),
            ("plugin_status", Risk::ReadOnly),
            ("plugin_logs", Risk::ReadOnly),
            ("plugin_enable", Risk::Mutating),
            ("plugin_disable", Risk::Mutating),
            ("plugin_restart", Risk::Mutating),
            ("plugin_new", Risk::Mutating),
            ("plugin_install", Risk::Mutating),
            ("plugin_uninstall", Risk::Destructive),
            ("plugin_approve_request", Risk::ReadOnly),
        ];
        assert_eq!(all, want);
        // No tool is an approval path of its own (checked at compile time).
        const _: () = assert!(!ListTool::HUMAN_ONLY && !StatusTool::HUMAN_ONLY && !ApproveRequestTool::HUMAN_ONLY);
        const _: () = assert!(!ApproveRequestTool::CONSUMES_SECRETS && !ApproveRequestTool::DIRECT);
    }

    #[test]
    fn arg_schemas() {
        let s = schema::<NewArgs>();
        assert_eq!(s["required"], serde_json::json!(["name"]));
        let langs = serde_json::to_string(&s).unwrap();
        for l in ["py", "js", "sh", "go", "rb", "rs"] {
            assert!(langs.contains(&format!("\"{l}\"")), "{l} in {langs}");
        }
        assert_eq!(schema::<UninstallArgs>()["required"], serde_json::json!(["name"]));
        assert_eq!(schema::<InstallArgs>()["required"], serde_json::json!(["path"]));
        // Defaults: lang py, raw false, keep_data false, tail 100.
        let a: NewArgs = serde_json::from_value(serde_json::json!({"name": "x"})).unwrap();
        assert_eq!((a.lang, a.raw), (Lang::Py, false));
        let a: LogsArgs = serde_json::from_value(serde_json::json!({"name": "x"})).unwrap();
        assert_eq!(a.tail, 100);
        let a: UninstallArgs = serde_json::from_value(serde_json::json!({"name": "x"})).unwrap();
        assert!(!a.keep_data);
        let _: NoArgs = serde_json::from_value(serde_json::json!({})).unwrap();
    }

    #[test]
    fn prompt_says_agents_cannot_approve() {
        assert!(PROMPT.contains("you cannot approve"));
        assert!(OWNER_MUST_APPROVE.contains("agents cannot approve"));
    }

    #[test]
    fn info_line_shape() {
        let i = PluginInfo {
            name: "turns".into(),
            version: "1.0.0".into(),
            description: String::new(),
            state: PluginRunState::AwaitingApproval,
            error: Some("boom".into()),
            enabled: false,
            needs_approval: true,
            card: None,
            placements: vec![],
            tools: vec![],
            dir: String::new(),
            log: String::new(),
            approved_at: None,
            order: 0,
        };
        assert_eq!(info_line(&i), "turns 1.0.0 awaiting_approval · needs approval · boom");
    }
}
