//! Pure renderers and label tables (design doc 06 §3.3, §3.4, §4.1): ACP tool
//! content → text for the tool card, `ToolKind` ↔ [`Risk`]. The harness's
//! plan is not rendered here: it is the session's todo snapshot (47 §5.3).

use std::path::Path;

use agent_client_protocol::schema::v1::{
    ContentBlock, EmbeddedResourceResource, ToolCallContent, ToolKind,
};
use tools::Risk;

/// Tool call content → text: `content` blocks as [`content_block_text`], `diff`
/// as [`render_diff`], `terminal` as `[terminal <id>]`. Blocks joined by newlines.
#[must_use]
pub fn render_content(content: &[ToolCallContent]) -> String {
    let mut out = String::new();
    for block in content {
        let text = match block {
            ToolCallContent::Content(c) => content_block_text(&c.content),
            ToolCallContent::Diff(d) => render_diff(&d.path, d.old_text.as_deref(), &d.new_text),
            ToolCallContent::Terminal(t) => format!("[terminal {}]", t.terminal_id),
            _ => continue,
        };
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&text);
    }
    out
}

/// A content block's text: text as-is, `[image]` / `[audio]`, resources by uri.
#[must_use]
pub fn content_block_text(b: &ContentBlock) -> String {
    match b {
        ContentBlock::Text(t) => t.text.clone(),
        ContentBlock::Image(_) => "[image]".to_owned(),
        ContentBlock::Audio(_) => "[audio]".to_owned(),
        ContentBlock::ResourceLink(r) => r.uri.clone(),
        ContentBlock::Resource(r) => match &r.resource {
            EmbeddedResourceResource::TextResourceContents(t) => t.uri.clone(),
            EmbeddedResourceResource::BlobResourceContents(b) => b.uri.clone(),
            _ => String::new(),
        },
        _ => String::new(),
    }
}

/// Minimal unified diff: `---`/`+++` header, one hunk of the changed middle
/// (common leading and trailing lines stripped, no context lines).
#[must_use]
pub fn render_diff(path: &Path, old: Option<&str>, new: &str) -> String {
    let old_lines: Vec<&str> = old.map(|s| s.lines().collect()).unwrap_or_default();
    let new_lines: Vec<&str> = new.lines().collect();
    let prefix = old_lines
        .iter()
        .zip(&new_lines)
        .take_while(|(a, b)| a == b)
        .count();
    let max_suffix = old_lines.len().min(new_lines.len()) - prefix;
    let suffix = old_lines
        .iter()
        .rev()
        .zip(new_lines.iter().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a == b)
        .count();
    let removed = &old_lines[prefix..old_lines.len() - suffix];
    let added = &new_lines[prefix..new_lines.len() - suffix];

    let mut out = format!(
        "--- {}\n+++ {}\n@@ -{},{} +{},{} @@",
        if old.is_some() {
            path.display().to_string()
        } else {
            "/dev/null".to_owned()
        },
        path.display(),
        prefix + 1,
        removed.len(),
        prefix + 1,
        added.len(),
    );
    for l in removed {
        out.push_str("\n-");
        out.push_str(l);
    }
    for l in added {
        out.push_str("\n+");
        out.push_str(l);
    }
    out
}

/// The approval class a harness tool kind maps to (06 §3.4): `read`/`search`/
/// `fetch`/`think` observe, `delete` is irreversible, everything else mutates.
#[must_use]
pub fn risk(kind: ToolKind) -> Risk {
    match kind {
        ToolKind::Read | ToolKind::Search | ToolKind::Fetch | ToolKind::Think => Risk::ReadOnly,
        ToolKind::Delete => Risk::Destructive,
        _ => Risk::Mutating,
    }
}

/// The ACP kind label for one of theuth's tools (06 §4.1): builtins by name, the
/// rest (MCP tools) by risk — `ReadOnly` → `other`, else `execute`.
#[must_use]
pub fn kind(tool_name: &str, risk: Risk) -> ToolKind {
    match tool_name {
        "read_file" | "list_dir" | "clip" => ToolKind::Read,
        "grep" | "glob" | "knowledge_search" | "knowledge_list" => ToolKind::Search,
        "write_file" | "edit" | "move_text" | "copy_text" | "indent" => ToolKind::Edit,
        "shell" | "python" => ToolKind::Execute,
        "spawn_subagent" | "send_message" => ToolKind::Other,
        _ if risk == Risk::ReadOnly => ToolKind::Other,
        _ => ToolKind::Execute,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{Diff, Terminal, TextContent};

    #[test]
    fn diff_of_three_line_change() {
        let d = render_diff(Path::new("/a/b.rs"), Some("a\nb\nc\nd\n"), "a\nB\nC\nd\n");
        assert_eq!(
            d,
            "--- /a/b.rs\n+++ /a/b.rs\n@@ -2,2 +2,2 @@\n-b\n-c\n+B\n+C"
        );
        let n = render_diff(Path::new("/n"), None, "x\n");
        assert_eq!(n, "--- /dev/null\n+++ /n\n@@ -1,0 +1,1 @@\n+x");
        assert_eq!(
            render_diff(Path::new("/s"), Some("same\n"), "same\n"),
            "--- /s\n+++ /s\n@@ -2,0 +2,0 @@"
        );
    }

    #[test]
    fn content_blocks() {
        let content = vec![
            ToolCallContent::from(ContentBlock::Text(TextContent::new("hello"))),
            ToolCallContent::Diff(Diff::new("/f", "new")),
            ToolCallContent::Terminal(Terminal::new("t1")),
        ];
        assert_eq!(
            render_content(&content),
            "hello\n--- /dev/null\n+++ /f\n@@ -1,0 +1,1 @@\n+new\n[terminal t1]"
        );
        assert_eq!(render_content(&[]), "");
    }

    #[test]
    fn risk_and_kind_tables() {
        assert_eq!(risk(ToolKind::Read), Risk::ReadOnly);
        assert_eq!(risk(ToolKind::Search), Risk::ReadOnly);
        assert_eq!(risk(ToolKind::Fetch), Risk::ReadOnly);
        assert_eq!(risk(ToolKind::Think), Risk::ReadOnly);
        assert_eq!(risk(ToolKind::Edit), Risk::Mutating);
        assert_eq!(risk(ToolKind::Move), Risk::Mutating);
        assert_eq!(risk(ToolKind::Execute), Risk::Mutating);
        assert_eq!(risk(ToolKind::Other), Risk::Mutating);
        assert_eq!(risk(ToolKind::SwitchMode), Risk::Mutating);
        assert_eq!(risk(ToolKind::Delete), Risk::Destructive);

        assert_eq!(kind("read_file", Risk::ReadOnly), ToolKind::Read);
        assert_eq!(kind("grep", Risk::ReadOnly), ToolKind::Search);
        assert_eq!(kind("edit", Risk::Mutating), ToolKind::Edit);
        for t in ["move_text", "copy_text", "indent"] {
            assert_eq!(kind(t, Risk::Mutating), ToolKind::Edit);
        }
        assert_eq!(kind("clip", Risk::ReadOnly), ToolKind::Read);
        assert_eq!(kind("shell", Risk::Destructive), ToolKind::Execute);
        assert_eq!(kind("spawn_subagent", Risk::ReadOnly), ToolKind::Other);
        assert_eq!(kind("mcp_lookup", Risk::ReadOnly), ToolKind::Other);
        assert_eq!(kind("mcp_write", Risk::Mutating), ToolKind::Execute);
    }
}
