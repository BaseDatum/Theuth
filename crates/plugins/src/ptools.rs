//! Owner: B1 — manifest tools → `tools::DynTool` (`ToolSource::Plugin`), design §8. A tool stays
//! registered while its plugin restarts and answers "plugin <name> is restarting" meanwhile.

use std::path::Path;
use std::sync::{Arc, RwLock};

use futures::future::BoxFuture;
use serde_json::{Value, json};

use ::tools::{ErasedTool, Gate, Risk, ToolCx, ToolOutput, ToolSource, ToolSpec};

use crate::error::PluginsError;
use crate::host::Conn;
use crate::manifest::{Manifest, ToolDecl};

/// The live connection of one plugin, swapped on every (re)start; `None` while it is down.
pub(crate) type Line = Arc<RwLock<Option<Arc<Conn>>>>;

/// One manifest tool as the registry sees it.
pub(crate) struct PluginTool {
    spec: ToolSpec,
    plugin: String,
    version: String,
    origin: String,
    line: Line,
}

/// The gate key of a plugin in `mcp/config.yaml` (design §8).
pub(crate) fn gate_key(plugin: &str) -> String {
    format!("plugin.{plugin}")
}

/// Gate then annotations (design §8): `None` when the gate hides the tool (`off`).
pub(crate) fn risk_of(mcp_dir: &Path, plugin: &str, t: &ToolDecl) -> Option<Risk> {
    match ::tools::mcp::gate_for(mcp_dir, &gate_key(plugin), &t.name) {
        Some(Gate::Off) => None,
        Some(g) => Some(g.risk().unwrap_or_else(|| crate::manifest::hint_risk(t))),
        None => Some(crate::manifest::hint_risk(t)),
    }
}

/// The registry entries for a manifest's tools (hidden ones left out).
pub(crate) fn build(
    mcp_dir: &Path,
    m: &Manifest,
    manifest_sha256: &str,
    line: &Line,
) -> Vec<PluginTool> {
    m.tools
        .iter()
        .filter_map(|t| {
            let risk = risk_of(mcp_dir, &m.name, t)?;
            Some(PluginTool {
                spec: ToolSpec {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    schema: t.input_schema.clone(),
                    risk,
                    direct: false,
                    returns: "Any".into(),
                },
                plugin: m.name.clone(),
                version: m.version.clone(),
                origin: format!("plugin:{}:{manifest_sha256}", m.name),
                line: line.clone(),
            })
        })
        .collect()
}

impl PluginTool {
    /// Registered name.
    pub fn name(&self) -> &str {
        &self.spec.name
    }
}

/// MCP `CallToolResult` → [`ToolOutput`].
pub(crate) fn output(result: &Value) -> ToolOutput {
    let text: Vec<&str> = result
        .get("content")
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter(|p| p.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|p| p.get("text").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut out = if is_error {
        ToolOutput::err(text.join("\n"))
    } else {
        ToolOutput::ok(text.join("\n"))
    };
    out.structured = result
        .get("structuredContent")
        .filter(|v| !v.is_null())
        .cloned();
    out
}

impl ErasedTool for PluginTool {
    fn spec(&self) -> ToolSpec {
        self.spec.clone()
    }

    fn source(&self) -> ToolSource {
        ToolSource::Plugin {
            name: self.plugin.clone(),
            version: self.version.clone(),
        }
    }

    fn origin(&self) -> String {
        self.origin.clone()
    }

    fn call(&self, args: Value, cx: ToolCx) -> BoxFuture<'_, anyhow::Result<ToolOutput>> {
        Box::pin(async move {
            let conn = self.line.read().ok().and_then(|l| l.clone());
            let Some(conn) = conn else {
                return Ok(ToolOutput::err(format!(
                    "plugin {} is restarting",
                    self.plugin
                )));
            };
            let params = json!({
                "name": self.spec.name,
                "arguments": args,
                "_meta": { "theuth": { "session": cx.session, "call_id": cx.call_id } },
            });
            let mut out = match conn
                .request_with("tools/call", params, Some(cx.cancel.clone()))
                .await
            {
                Ok(r) => output(&r),
                Err(PluginsError::NotRunning { .. } | PluginsError::Exited { .. }) => {
                    ToolOutput::err(format!("plugin {} is restarting", self.plugin))
                }
                Err(e) => ToolOutput::err(e.to_string()),
            };
            ::tools::secrets::redact_output(&mut out);
            Ok(out)
        })
    }
}

/// Whether a registry entry with source `held` belongs to plugin `plugin` (any version).
fn owned_by(held: &ToolSource, plugin: &str) -> bool {
    matches!(held, ToolSource::Plugin { name, .. } if name == plugin)
}

/// Whether `plugin` may register or remove a name the registry holds for `held` (design §8: a
/// clash with a native loses; another plugin's or an MCP server's name is never taken either).
fn may_take(held: Option<&ToolSource>, plugin: &str) -> bool {
    held.is_none_or(|h| owned_by(h, plugin))
}

async fn held(
    reg: &kameo::actor::ActorRef<::tools::ToolRegistry>,
    name: &str,
) -> Option<ToolSource> {
    reg.ask(::tools::registry::ToolSourceOf(name.to_owned()))
        .await
        .ok()
        .flatten()
}

/// Register `tools` of `plugin` once the registry answers; unregister `gone` first. Only names
/// whose current source is `ToolSource::Plugin { name: plugin }` are removed, and a name held by
/// another source (native, MCP, another plugin) is never replaced: that tool is skipped with a
/// warning (review R1).
pub(crate) async fn sync(
    rt: runtime::Runtime,
    plugin: String,
    gone: Vec<String>,
    tools: Vec<PluginTool>,
) {
    let Some(reg) = rt
        .lookup_when::<::tools::ToolRegistry>(::tools::REGISTRY_ACTOR)
        .await
    else {
        return;
    };
    for name in gone {
        if held(&reg, &name)
            .await
            .is_some_and(|h| owned_by(&h, &plugin))
        {
            let _ = reg.ask(::tools::UnregisterTool(name)).await;
        }
    }
    for t in tools {
        if !may_take(held(&reg, &t.spec.name).await.as_ref(), &plugin) {
            tracing::warn!(
                plugin = %plugin,
                tool = %t.spec.name,
                "plugins: tool name taken by another source; the plugin's tool is not registered"
            );
            continue;
        }
        let tool: ::tools::DynTool = Arc::new(t);
        let _ = reg.ask(::tools::RegisterTool(tool)).await;
    }
}

/// Validation errors for manifest tools whose name the registry already holds for another
/// source (design §8, review R1), so the card never offers them. Empty when the registry is not
/// running.
pub(crate) async fn clashes(m: &Manifest) -> Vec<crate::types::CheckIssue> {
    let Some(reg) =
        kameo::actor::ActorRef::<::tools::ToolRegistry>::lookup(::tools::REGISTRY_ACTOR)
            .ok()
            .flatten()
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (i, t) in m.tools.iter().enumerate() {
        if !may_take(held(&reg, &t.name).await.as_ref(), &m.name) {
            out.push(crate::types::CheckIssue {
                path: format!("/tools/{i}/name"),
                message: format!(
                    "a tool named `{}` already exists (native, MCP or another plugin)",
                    t.name
                ),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_tool_result_maps_to_output() {
        let out = output(&json!({
            "content": [{"type": "text", "text": "0 of 50 turns today"}],
            "structuredContent": {"turns": 0},
            "isError": false
        }));
        assert!(!out.is_error);
        assert_eq!(out.content, "0 of 50 turns today");
        assert_eq!(out.structured, Some(json!({"turns": 0})));
        let err = output(&json!({"content": [{"type": "text", "text": "boom"}], "isError": true}));
        assert!(err.is_error);
    }

    #[test]
    fn only_own_or_free_names_are_taken() {
        let mine = ToolSource::Plugin {
            name: "turns".into(),
            version: "0.1.0".into(),
        };
        let other = ToolSource::Plugin {
            name: "other".into(),
            version: "0.1.0".into(),
        };
        assert!(may_take(None, "turns"));
        assert!(may_take(Some(&mine), "turns"), "own name, any version");
        assert!(
            !may_take(Some(&ToolSource::native()), "turns"),
            "a native wins"
        );
        assert!(
            !may_take(Some(&other), "turns"),
            "another plugin keeps its name"
        );
        assert!(!owned_by(&ToolSource::native(), "turns"));
        assert!(owned_by(&mine, "turns"));
    }
}
