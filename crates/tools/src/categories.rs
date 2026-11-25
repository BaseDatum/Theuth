//! High-level, user-configurable categories for builtin tools and MCP servers.
//!
//! Session hides use `tcat:<id>` in the same `sessions.hidden_tools` sets as
//! tool-name, `cat:<id>` and `mcp:<server>` hides.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Prefix of a session hide entry naming a high-level tool category.
pub const TOOL_CATEGORY_PREFIX: &str = "tcat:";

/// The session hide entry for tool category `id`.
pub fn tool_category_key(id: &str) -> String {
    format!("{TOOL_CATEGORY_PREFIX}{id}")
}

/// One built-in high-level category, in display order.
#[derive(Debug)]
pub struct DefaultCategoryDef {
    /// Stable id used in `tcat:<id>` session hides.
    pub id: &'static str,
    /// Display name.
    pub label: &'static str,
    /// One-line summary of the category's tools.
    pub description: &'static str,
}

/// The built-in high-level categories in display order.
pub const DEFAULT_CATEGORIES: &[DefaultCategoryDef] = &[
    DefaultCategoryDef {
        id: "engineering",
        label: "Engineering",
        description: "Files, shell, code index, sandboxes and experiments",
    },
    DefaultCategoryDef {
        id: "web",
        label: "Web",
        description: "Web search, scraping, HTTP requests and browser automation",
    },
    DefaultCategoryDef {
        id: "computer",
        label: "Computer",
        description: "Control the desktop, its apps and system automation",
    },
    DefaultCategoryDef {
        id: "interface",
        label: "Interface",
        description: "Interactive apps, previews, tiles, visual output and user prompts",
    },
    DefaultCategoryDef {
        id: "documentation",
        label: "Documentation",
        description: "Documents, notebooks and their sources",
    },
    DefaultCategoryDef {
        id: "knowledge",
        label: "Knowledge",
        description: "Search and maintain the knowledge base and past sessions",
    },
    DefaultCategoryDef {
        id: "comms",
        label: "Comms",
        description: "Link and check messaging channels",
    },
    DefaultCategoryDef {
        id: "agents",
        label: "Agents",
        description: "Subagents, peers, tasks and escalations",
    },
    DefaultCategoryDef {
        id: "meetings",
        label: "Meetings",
        description: "Join, record, transcribe and brief on meetings",
    },
    DefaultCategoryDef {
        id: "secrets",
        label: "Secrets",
        description: "Store and use secrets from the vault",
    },
    DefaultCategoryDef {
        id: "harness",
        label: "Harness",
        description: "Skills, MCP setup, schedules, reminders, decisions and dreaming",
    },
];

/// The default high-level category of a registered native tool.
pub fn default_category_for_builtin(tool_name: &str) -> &'static str {
    match crate::builtins::category_of(tool_name) {
        "files" | "shell" | "code" | "sandbox" | "autoresearch" => "engineering",
        "web" | "browser" => "web",
        "computer" => "computer",
        "output" => "interface",
        "documents" => "documentation",
        "knowledge" => "knowledge",
        "channels" => "comms",
        "agents" | "tasks" => "agents",
        "meetings" => "meetings",
        "secrets" => "secrets",
        "setup" | "schedules" | "decisions" | "dreaming" | "other" => "harness",
        _ => "harness",
    }
}

/// The built-in high-level category with this id.
pub fn default_category(id: &str) -> Option<&'static DefaultCategoryDef> {
    DEFAULT_CATEGORIES.iter().find(|c| c.id == id)
}

/// Validate a custom category id: non-empty, at most 48 ASCII characters, and
/// `[a-z0-9][a-z0-9_-]*`.
pub fn validate_category_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("category id cannot be empty".to_owned());
    }
    if id.len() > 48 {
        return Err("category id cannot exceed 48 characters".to_owned());
    }
    let mut bytes = id.bytes();
    if !bytes
        .next()
        .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        || !bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
    {
        return Err("category id must match [a-z0-9][a-z0-9_-]*".to_owned());
    }
    Ok(())
}

/// Make a category id from a display label: lowercase ASCII alphanumerics,
/// with each non-alphanumeric run replaced by one trimmed `-`.
pub fn slugify_category_label(label: &str) -> String {
    let mut slug = String::with_capacity(label.len());
    let mut separator = false;
    for b in label.bytes() {
        let b = b.to_ascii_lowercase();
        if b.is_ascii_lowercase() || b.is_ascii_digit() {
            if separator && !slug.is_empty() {
                slug.push('-');
            }
            slug.push(char::from(b));
            separator = false;
        } else {
            separator = true;
        }
    }
    slug
}

/// A user-created tool category in `tools.categories.custom`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomCategoryDef {
    /// Stable id used in hides and assignments.
    pub id: String,
    /// Display name.
    pub label: String,
    /// Optional one-line summary.
    #[serde(default)]
    pub description: String,
}

/// `tools.categories` configuration.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolCategoriesConfig {
    /// User-created categories.
    #[serde(default, deserialize_with = "null_as_empty_vec")]
    pub custom: Vec<CustomCategoryDef>,
    /// MCP server name → category id.
    #[serde(default, deserialize_with = "null_as_empty_map")]
    pub mcp_assignments: BTreeMap<String, String>,
    /// Native tool name → category id override.
    #[serde(default, deserialize_with = "null_as_empty_map")]
    pub tool_assignments: BTreeMap<String, String>,
    /// Category ids hidden when a brand-new root interactive session is created.
    #[serde(default, deserialize_with = "null_as_empty_vec")]
    pub default_disabled: Vec<String>,
}

fn null_as_empty_vec<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<Vec<T>>::deserialize(d).map(Option::unwrap_or_default)
}

fn null_as_empty_map<'de, D>(d: D) -> Result<BTreeMap<String, String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<BTreeMap<String, String>>::deserialize(d).map(Option::unwrap_or_default)
}

/// One tool category as an app shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ToolCategoryInfo {
    /// Stable id; its session hide key is `tcat:<id>`.
    pub id: String,
    /// Display name.
    pub label: String,
    /// One-line summary of the category.
    pub description: String,
    /// Whether this is a user-created rather than default category.
    pub custom: bool,
    /// Whether the category is enabled in brand-new root interactive sessions.
    pub default_enabled: bool,
    /// Whether the requested session or an ancestor hides this category.
    pub hidden: bool,
    /// Number of registered native tools in [`Self::builtin_tools`].
    pub builtin_count: u32,
    /// Registered native tool names in this category, sorted by name.
    pub builtin_tools: Vec<String>,
    /// MCP server names assigned to this category, sorted by name.
    pub mcp_servers: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtins::CATEGORIES;

    #[test]
    fn default_categories_are_in_display_order() {
        let ids: Vec<_> = DEFAULT_CATEGORIES.iter().map(|c| c.id).collect();
        assert_eq!(
            ids,
            [
                "engineering",
                "web",
                "computer",
                "interface",
                "documentation",
                "knowledge",
                "comms",
                "agents",
                "meetings",
                "secrets",
                "harness"
            ]
        );
        assert_eq!(DEFAULT_CATEGORIES[1].label, "Web");
        assert_eq!(DEFAULT_CATEGORIES[2].label, "Computer");
        assert_eq!(DEFAULT_CATEGORIES[3].label, "Interface");
        assert_eq!(DEFAULT_CATEGORIES[5].label, "Knowledge");
        assert_eq!(DEFAULT_CATEGORIES[6].label, "Comms");
        assert_eq!(DEFAULT_CATEGORIES[7].label, "Agents");
        assert_eq!(DEFAULT_CATEGORIES[8].label, "Meetings");
        assert_eq!(DEFAULT_CATEGORIES[9].label, "Secrets");
        assert_eq!(default_category("web").unwrap().id, "web");
        assert!(default_category("missing").is_none());
    }

    #[test]
    fn every_builtin_group_maps_to_its_high_level_category() {
        // One tool whose builtin category is the named group, plus the expected default.
        let probes = [
            ("files", "read_file", "engineering"),
            ("shell", "shell", "engineering"),
            ("code", "code_find", "engineering"),
            ("web", "search_web", "web"),
            ("browser", "browser_open", "web"),
            ("computer", "computer_screenshot", "computer"),
            ("sandbox", "sandbox_create", "engineering"),
            ("knowledge", "knowledge_search", "knowledge"),
            ("documents", "doc_read", "documentation"),
            ("output", "show", "interface"),
            ("agents", "spawn_subagent", "agents"),
            ("tasks", "task_create", "agents"),
            ("schedules", "schedule", "harness"),
            ("autoresearch", "experiment_run", "engineering"),
            ("meetings", "meeting_join", "meetings"),
            ("secrets", "secret_get", "secrets"),
            ("decisions", "decide", "harness"),
            ("dreaming", "dreaming_replay", "harness"),
            ("setup", "load_skill", "harness"),
            ("channels", "signal_status", "comms"),
            ("other", "unclassified_tool", "harness"),
        ];
        assert_eq!(CATEGORIES.len(), probes.len());
        for group in CATEGORIES {
            assert!(probes.iter().any(|(id, _, _)| *id == group.id));
        }
        for (group, tool, expected) in probes {
            assert_eq!(crate::builtins::category_of(tool), group, "{tool}");
            assert_eq!(default_category_for_builtin(tool), expected, "{group}");
        }
    }

    #[test]
    fn category_ids_are_strict_lowercase_ascii_slugs() {
        for id in ["a", "0", "a0", "a_b-c9", &"a".repeat(48)] {
            assert_eq!(validate_category_id(id), Ok(()), "{id}");
        }
        for id in [
            "",
            "-abc",
            "_abc",
            "Abc",
            "a:b",
            "a b",
            "é",
            "a/b",
            &"a".repeat(49),
        ] {
            assert!(validate_category_id(id).is_err(), "{id:?}");
        }
    }

    #[test]
    fn labels_slugify_to_trimmed_collapsed_ascii() {
        for (label, expected) in [
            ("Research Notes", "research-notes"),
            ("  Web & Browser  ", "web-browser"),
            ("Already_mixed-ID", "already-mixed-id"),
            ("One---two___three", "one-two-three"),
            ("Café déjà vu", "caf-d-j-vu"),
            ("你好", ""),
            ("", ""),
        ] {
            assert_eq!(slugify_category_label(label), expected, "{label}");
        }
    }

    #[test]
    fn config_defaults_and_accepts_yaml_null_collections() {
        let empty: ToolCategoriesConfig = serde_yaml::from_str("{}").unwrap();
        assert_eq!(empty, ToolCategoriesConfig::default());

        let nulls: ToolCategoriesConfig = serde_yaml::from_str(
            "custom: null\nmcp_assignments: null\ntool_assignments: null\ndefault_disabled: null\n",
        )
        .unwrap();
        assert_eq!(nulls, ToolCategoriesConfig::default());
    }

    #[test]
    fn config_deserializes_assignments_and_custom_categories() {
        let cfg: ToolCategoriesConfig = serde_yaml::from_str(
            "custom:\n  - id: research\n    label: Research\nmcp_assignments:\n  files: research\ntool_assignments:\n  read_file: web\ndefault_disabled: [interface, research]\n",
        )
        .unwrap();
        assert_eq!(cfg.custom[0].id, "research");
        assert_eq!(cfg.custom[0].description, "");
        assert_eq!(cfg.mcp_assignments["files"], "research");
        assert_eq!(cfg.tool_assignments["read_file"], "web");
        assert_eq!(cfg.default_disabled, ["interface", "research"]);
    }
}
