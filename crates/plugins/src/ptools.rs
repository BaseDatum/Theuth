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

/// Register `tools` once the registry answers; unregister `gone` first.
pub(crate) async fn sync(rt: runtime::Runtime, gone: Vec<String>, tools: Vec<PluginTool>) {
    let Some(reg) = rt
        .lookup_when::<::tools::ToolRegistry>(::tools::REGISTRY_ACTOR)
        .await
    else {
        return;
    };
    for name in gone {
        let _ = reg.ask(::tools::UnregisterTool(name)).await;
    }
    for t in tools {
        let tool: ::tools::DynTool = Arc::new(t);
        let _ = reg.ask(::tools::RegisterTool(tool)).await;
    }
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
}
