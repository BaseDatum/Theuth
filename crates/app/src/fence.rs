//! Reply-fence checkers (30 §5): the ```` ```openui ```` and ```` ```mermaid ````
//! blocks of a model reply are validated by the parser helper through the
//! registry's [`tools::FenceChecker`] hook, so the agent loop never depends on
//! this crate. Both fail open: a helper problem is `Err` and the loop treats
//! the fence as valid.

use std::sync::Arc;

use tools::{DescribeTools, FenceChecker, FenceError, ToolInspection};

use crate::actor::Core;
use crate::helper::ToolSnapshot;
use crate::tool::signatures_for;

/// `mermaid` fences: `Helper::check_mermaid`.
pub struct Mermaid(pub Arc<Core>);

#[async_trait::async_trait]
impl FenceChecker for Mermaid {
    async fn check(&self, body: &str) -> Result<Vec<FenceError>, String> {
        self.0
            .helper
            .check_mermaid(body)
            .await
            .map_err(|e| e.to_string())
    }
}

/// `openui` fences: `Helper::check` against every registered tool, then the
/// reply-fence rule — a fence is static (30 §9), so `Query`, `Mutation` and
/// `@ToAssistant` are rejected here, keeping the helper generic. Failures
/// carry the compact signatures of the components the block uses.
pub struct OpenUi(pub Arc<Core>);

#[async_trait::async_trait]
impl FenceChecker for OpenUi {
    async fn check(&self, body: &str) -> Result<Vec<FenceError>, String> {
        let registry = self.0.registry()?;
        let inspections: Vec<ToolInspection> = registry
            .ask(DescribeTools(None))
            .await
            .map_err(|e| e.to_string())?;
        let snapshot: Vec<ToolSnapshot> = inspections
            .iter()
            .map(|t| ToolSnapshot {
                name: t.spec.name.clone(),
                schema: t.spec.schema.clone(),
                risk: t.spec.risk,
                identity: t.identity.clone(),
            })
            .collect();
        let check = self
            .0
            .helper
            .check(body, &snapshot)
            .await
            .map_err(|e| e.to_string())?;
        let mut errs: Vec<FenceError> = check
            .diagnostics
            .into_iter()
            .map(|d| FenceError {
                path: d.at,
                message: d.message,
            })
            .collect();
        errs.extend(dynamic_statements(body));
        if !errs.is_empty() {
            let sigs = signatures_for(body);
            if !sigs.is_empty() {
                errs.push(FenceError {
                    path: String::new(),
                    message: format!("signatures:\n  {}", sigs.join("\n  ")),
                });
            }
        }
        Ok(errs)
    }
}

/// One error per statement whose value starts a `Query(`/`Mutation(` or whose
/// action contains `@ToAssistant(`. Line scan: a statement is `id = …` at
/// column 0 and the id is the located path.
fn dynamic_statements(source: &str) -> Vec<FenceError> {
    let mut out = Vec::new();
    let mut id = String::new();
    for line in source.lines() {
        if let Some((lhs, _)) = line.split_once('=') {
            let lhs = lhs.trim();
            if !lhs.is_empty()
                && !line.starts_with([' ', '\t'])
                && lhs
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '$'))
            {
                id = lhs.to_owned();
            }
        }
        for what in ["Query(", "Mutation(", "@ToAssistant("] {
            if line.contains(what) {
                out.push(FenceError {
                    path: id.clone(),
                    message: format!(
                        "{} not allowed in a reply fence; use the `app` tool",
                        what.trim_end_matches('(')
                    ),
                });
                break;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamic_statements_are_located_by_statement_id() {
        let src = "root = Stack([status, go])\n\
                   health = Query(\"http_get\", {url: \"x\"})\n\
                   status = TextContent(\"ok\")\n\
                   go = Button(\"Ask\", Action([\n  @ToAssistant(\"help\")\n]))\n\
                   save = Mutation(\"write\", {})";
        let errs = dynamic_statements(src);
        let at: Vec<(&str, &str)> = errs
            .iter()
            .map(|e| (e.path.as_str(), e.message.as_str()))
            .collect();
        assert_eq!(
            at,
            vec![
                (
                    "health",
                    "Query not allowed in a reply fence; use the `app` tool"
                ),
                (
                    "go",
                    "@ToAssistant not allowed in a reply fence; use the `app` tool"
                ),
                (
                    "save",
                    "Mutation not allowed in a reply fence; use the `app` tool"
                ),
            ]
        );
        assert!(dynamic_statements("root = Stack([a])\na = TextContent(\"q\")").is_empty());
    }

    /// The `mermaid` fence checker against the real in-process bundle: a valid
    /// diagram passes, a broken one is located by line.
    #[tokio::test]
    async fn mermaid_fences_are_checked_by_the_in_process_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let rt = runtime::Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
        let helper = Arc::new(crate::helper::Helper::new(rt.shutdown.clone()));
        let checker = Mermaid(Arc::new(Core::new(rt, helper)));
        assert_eq!(
            checker.check("sequenceDiagram\n  A->>B: hi").await,
            Ok(vec![])
        );
        let errs = checker
            .check("sequenceDiagram\n  A->>B: hi\n}}} ]]] ((( @@@ !!! <<<")
            .await
            .unwrap();
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert_eq!(errs[0].path, "line:3", "{errs:?}");
    }
}
