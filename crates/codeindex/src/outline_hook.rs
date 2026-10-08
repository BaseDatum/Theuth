//! `read_file(outline=true)` → the `code_outline` render, through the
//! [`tools::fs::Outliner`] hook (12 §10). `tools` cannot depend on this
//! crate, so the plugin installs the hook at start.

use std::path::Path;
use std::sync::Arc;

use tools::{Tool, ToolCx};

use crate::index::Codeindex;
use crate::root;
use crate::tools::{CodeOutlineArgs, CodeOutlineTool};

/// [`tools::fs::Outliner`] over the plugin's index.
pub struct OutlineHook(pub Arc<Codeindex>);

/// Install the hook; a no-op if one is already installed.
pub fn register(ix: Arc<Codeindex>) {
    if !tools::fs::set_outliner(Arc::new(OutlineHook(ix))) {
        tracing::debug!("read_file outliner already installed");
    }
}

#[async_trait::async_trait]
impl tools::fs::Outliner for OutlineHook {
    /// The root is resolved as `code_outline` does without a `root`
    /// argument: the git top-level of the cwd. A path outside it, or one the
    /// index does not know (ignored, binary, unsupported language), is
    /// `None` so `read_file` falls back to text.
    async fn outline(&self, path: &Path) -> anyhow::Result<Option<String>> {
        let Ok(root) = root::discover(None) else {
            return Ok(None);
        };
        if root::under_root(&root, path).is_err() {
            return Ok(None);
        }
        // `code_outline` ignores its context; this is not a session call.
        let cx = ToolCx {
            session: runtime::SessionId::new(),
            call_id: String::new(),
            quiet: true,
            resources: tools::SessionResources::durable_arc(&std::env::temp_dir()),
            cancel: Default::default(),
            promote_after: None,
        };
        let args = CodeOutlineArgs {
            path: path.to_owned(),
            root: None,
            budget: None,
        };
        let out = CodeOutlineTool(Arc::clone(&self.0)).call(args, cx).await?;
        Ok((!out.is_error).then_some(out.content))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tools::fs::Outliner;

    #[tokio::test]
    async fn unknown_path_is_none() {
        let dir =
            std::env::temp_dir().join(format!("theuth-outline-hook-{}", uuid::Uuid::new_v4()));
        let ix = Arc::new(Codeindex::new(crate::CodeindexConfig::default(), &dir).unwrap());
        let hook = OutlineHook(ix);
        let missing = dir.join("nope/missing.rs");
        assert_eq!(hook.outline(&missing).await.unwrap(), None);
    }
}
