//! MCP client: one `rmcp` connection per configured server; each remote tool
//! becomes an [`crate::ErasedTool`] named `<server>__<tool>`.
