//! 206: the harness's builtin tools by category, for the Settings > Tools >
//! Builtins tab, the composer's per-chat switch and the harness-wide
//! `tools.builtins.disabled` set. A builtin is a registered tool whose
//! [`crate::ToolSource`] is `Native`; MCP tools never are.

use crate::{PolicyAction, Risk};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Prefix of a hide entry naming a whole builtin category (see [`category_key`]),
/// the builtin twin of [`crate::MCP_SERVER_HIDE_PREFIX`].
pub const BUILTIN_CATEGORY_PREFIX: &str = "cat:";

/// The hide entry (per session in `SetHidden`, harness-wide in
/// `tools.builtins.disabled`) that names every builtin of category `id`,
/// tools registered later included. Tool names cannot contain `:`, so it
/// matches no tool by name.
pub fn category_key(id: &str) -> String {
    format!("{BUILTIN_CATEGORY_PREFIX}{id}")
}

/// `tools.builtins` config section.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BuiltinsConfig {
    /// Harness-wide "Off always": builtin tool names and [`category_key`]s,
    /// sorted. Written back by `SetBuiltinsDisabled`. Left empty (YAML null): none.
    #[serde(deserialize_with = "null_as_empty")]
    pub disabled: Vec<String>,
}

fn null_as_empty<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    Option::<Vec<String>>::deserialize(d).map(Option::unwrap_or_default)
}

/// Longest [`summary`], in chars, before the ellipsis.
const SUMMARY_CHARS: usize = 400;

/// What `ListBuiltins` sends of a tool description: its first paragraph, cut at
/// [`SUMMARY_CHARS`] chars with a trailing `…` when longer.
pub fn summary(description: &str) -> String {
    let para = description
        .trim_start()
        .lines()
        .take_while(|l| !l.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    match para.char_indices().nth(SUMMARY_CHARS) {
        Some((cut, _)) => format!("{}…", para[..cut].trim_end()),
        None => para.trim_end().to_owned(),
    }
}

/// One category of builtin tools as an app shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct BuiltinCategory {
    /// Category id; [`category_key`] of it is the switch entry.
    pub id: String,
    /// Display name.
    pub label: String,
    /// One line on what the category does.
    pub description: String,
    /// Registered builtins of the category, sorted by name.
    pub tools: Vec<BuiltinTool>,
}

/// One registered builtin tool as an app shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct BuiltinTool {
    /// Registered name.
    pub name: String,
    /// What the model sees.
    pub description: String,
    /// Risk class.
    pub risk: Risk,
    /// Offered as a direct tool definition (`ToolSpec::direct`).
    pub direct: bool,
    /// Off always: the tool's name or its category key is in `tools.builtins.disabled`.
    pub disabled: bool,
    /// Off for `session`: the tool's name or its category key is in the hidden set of the session or an ancestor. Always false when no session is given.
    pub hidden: bool,
    /// What a call with no secret in its args does for `session` (harness-wide when none):
    /// `allow` runs without asking (approvals off, or the policy allows the risk), `ask`
    /// raises a card (approvals on, or the tool is marked always ask), `deny` refuses (the
    /// policy or a subagent cap). Absent from an engine that predates it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<PolicyAction>,
    /// 221 D10: the `secret:<backend>/<conn>:<dir>` always-ask entries that still make this
    /// tool's calls ask when they go through that connection (`shell` with a `{{secret:bw/…}}`
    /// placeholder reads it), whatever `action` says. Sorted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secret_asks: Vec<String>,
}

/// One row of [`CATEGORIES`].
#[derive(Debug)]
pub struct CategoryDef {
    /// Id (`cat:<id>` is its key).
    pub id: &'static str,
    /// Display name.
    pub label: &'static str,
    /// One line, plain English.
    pub description: &'static str,
    /// Members by exact name (checked across all categories before any prefix).
    exact: &'static [&'static str],
    /// Members by name prefix.
    prefixes: &'static [&'static str],
}

/// Id of the fallback category: any builtin no other row claims.
pub const OTHER: &str = "other";

/// The categories in display order; [`OTHER`] last.
pub const CATEGORIES: &[CategoryDef] = &[
    CategoryDef {
        id: "files",
        label: "Files",
        description: "Read, search and edit files on this machine",
        exact: &[
            "read_file",
            "write_file",
            "list_dir",
            "grep",
            "glob",
            "edit",
            "move_text",
            "copy_text",
            "indent",
            "clip",
        ],
        prefixes: &[],
    },
    CategoryDef {
        id: "shell",
        label: "Shell and jobs",
        description: "Run commands and terminal programs, and manage background jobs",
        exact: &[
            "shell",
            "start",
            "send",
            "screen",
            "wait",
            "jobs",
            "cancel_job",
            "cell_runs",
            "cell_output",
            "interpreters",
            "node_shell",
        ],
        prefixes: &[],
    },
    CategoryDef {
        id: "code",
        label: "Code index",
        description: "Find symbols, references and structure in indexed code",
        exact: &[
            "code_find",
            "code_outline",
            "code_refs",
            "code_map",
            "code_symbol",
            "code_impact",
            "code_overview",
            "project_profile",
        ],
        prefixes: &[],
    },
    CategoryDef {
        id: "web",
        label: "Web",
        description: "Search the web, read pages and call HTTP endpoints",
        exact: &[
            "search_web",
            "read_url",
            "http_request",
            "search_x",
            "scrape_url",
            "crawl",
            "map_site",
            "site_learn",
            "site_routes",
            "site_call",
        ],
        prefixes: &[],
    },
    CategoryDef {
        id: "browser",
        label: "Browser",
        description: "Drive a web browser: open pages, click, type and read",
        exact: &[],
        prefixes: &["browser_"],
    },
    CategoryDef {
        id: "computer",
        label: "Computer use",
        description: "Control the desktop, its apps and system automation",
        exact: &["applescript", "shortcuts", "powershell", "desktop_handoff"],
        prefixes: &["computer_"],
    },
    CategoryDef {
        id: "sandbox",
        label: "Sandboxes",
        description: "Create and use isolated sandboxes",
        exact: &[],
        prefixes: &["sandbox_"],
    },
    CategoryDef {
        id: "knowledge",
        label: "Knowledge and search",
        description: "Search and maintain the knowledge base and past sessions",
        exact: &["search", "session_read", "peek_memory"],
        prefixes: &["knowledge_"],
    },
    CategoryDef {
        id: "documents",
        label: "Documents and notebooks",
        description: "Write documents, work in notebooks and their sources",
        exact: &["nb_search", "insight_run", "insight_promote"],
        prefixes: &["doc_", "notebook_", "source_"],
    },
    CategoryDef {
        id: "output",
        label: "Output and apps",
        description: "Show results, previews and apps, and talk to the user",
        exact: &[
            "show",
            "show_web",
            "show_pty",
            "show_note",
            "show_whiteboard",
            "show_file",
            "show_diff",
            "show_view",
            "pane_manage",
            "pane_list",
            "page_preview",
            "artifact_read",
            "app",
            "message_user",
            "ask_user",
        ],
        prefixes: &[],
    },
    CategoryDef {
        id: "agents",
        label: "Agents and peers",
        description: "Spawn subagents and talk to other agents and peers",
        exact: &[
            "spawn_subagent",
            "send_message",
            "discover_peers",
            "peer_message",
            "wait_peer",
            "list_models",
            "mesh_peers",
            "agents_list",
            "agent_put",
            "agent_propose",
            "agent_draft",
        ],
        prefixes: &[],
    },
    CategoryDef {
        id: "tasks",
        label: "Tasks",
        description: "Create and track tasks and projects, and escalate to a human",
        exact: &[
            "escalate",
            "project_list",
            "project_create",
            "project_update",
            "project_delete",
        ],
        prefixes: &["task_"],
    },
    CategoryDef {
        id: "schedules",
        label: "Schedules and reminders",
        description: "Schedule work, set reminders and react to sensors",
        exact: &[
            "schedule",
            "schedule_done",
            "schedule_status",
            "list_sensors",
            "routine_draft",
            "remind_me_in",
            "cancel_reminder",
        ],
        prefixes: &[],
    },
    CategoryDef {
        id: "autoresearch",
        label: "Autoresearch",
        description: "Run experiments and autoresearch loops",
        exact: &[],
        prefixes: &["experiment_", "autoresearch_"],
    },
    CategoryDef {
        id: "meetings",
        label: "Meetings",
        description: "Join, record and brief on meetings",
        exact: &[],
        prefixes: &["meeting_", "brief_"],
    },
    CategoryDef {
        id: "secrets",
        label: "Secrets",
        description: "Store and use secrets from the vault",
        exact: &[],
        prefixes: &["secret_"],
    },
    CategoryDef {
        id: "decisions",
        label: "Decision model",
        description: "Ask the decision model and its user double",
        exact: &["decide", "decide_many", "decide_label"],
        prefixes: &["double_"],
    },
    CategoryDef {
        id: "dreaming",
        label: "Dreaming",
        description: "Learn from past sessions: cases, replays and preferences",
        exact: &[
            "case_put",
            "case_list",
            "skill_learned_edit",
            "replay_case",
            "preferences_set",
        ],
        prefixes: &["dreaming_"],
    },
    CategoryDef {
        id: "setup",
        label: "Skills and MCP setup",
        description: "Load and manage skills and MCP servers",
        exact: &[
            "load_skill",
            "add_skill",
            "remove_skill",
            "add_skill_repo",
            "remove_skill_repo",
            "sync_skills",
            "add_mcp_server",
            "remove_mcp_server",
            "add_mcp_repo",
            "sync_mcp",
        ],
        prefixes: &[],
    },
    CategoryDef {
        id: "channels",
        label: "Channels",
        description: "Link and check messaging channels",
        exact: &[
            "signal_link",
            "signal_status",
            "slack_status",
            "telegram_link",
            "telegram_status",
        ],
        prefixes: &[],
    },
    CategoryDef {
        id: OTHER,
        label: "Other",
        description: "Builtin tools that fit no other category",
        exact: &[],
        prefixes: &[],
    },
];

/// The category of builtin `tool_name`: an exact member first, then the first
/// matching prefix, else [`OTHER`].
pub fn category_of(tool_name: &str) -> &'static str {
    CATEGORIES
        .iter()
        .find(|c| c.exact.contains(&tool_name))
        .or_else(|| {
            CATEGORIES
                .iter()
                .find(|c| c.prefixes.iter().any(|p| tool_name.starts_with(p)))
        })
        .map_or(OTHER, |c| c.id)
}

/// The [`category_key`] of builtin `tool_name`'s category.
pub fn category_key_of(tool_name: &str) -> String {
    category_key(category_of(tool_name))
}

/// The category with this id.
pub fn category(id: &str) -> Option<&'static CategoryDef> {
    CATEGORIES.iter().find(|c| c.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn builtins_category_table_maps_every_listed_name() {
        let cases: &[(&str, &[&str])] = &[
            (
                "files",
                &[
                    "read_file",
                    "write_file",
                    "list_dir",
                    "grep",
                    "glob",
                    "edit",
                    "move_text",
                    "copy_text",
                    "indent",
                    "clip",
                ],
            ),
            (
                "shell",
                &[
                    "shell",
                    "start",
                    "send",
                    "screen",
                    "wait",
                    "jobs",
                    "cancel_job",
                    "cell_runs",
                    "cell_output",
                    "interpreters",
                ],
            ),
            (
                "code",
                &[
                    "code_find",
                    "code_outline",
                    "code_refs",
                    "code_map",
                    "code_symbol",
                    "code_impact",
                    "code_overview",
                    "project_profile",
                ],
            ),
            (
                "web",
                &[
                    "search_web",
                    "read_url",
                    "http_request",
                    "search_x",
                    "scrape_url",
                    "crawl",
                    "map_site",
                    "site_learn",
                    "site_routes",
                    "site_call",
                ],
            ),
            ("browser", &["browser_open", "browser_click"]),
            (
                "computer",
                &[
                    "computer_click",
                    "applescript",
                    "shortcuts",
                    "powershell",
                    "desktop_handoff",
                ],
            ),
            ("sandbox", &["sandbox_create"]),
            (
                "knowledge",
                &["knowledge_search", "search", "session_read", "peek_memory"],
            ),
            (
                "documents",
                &[
                    "doc_write",
                    "doc_export",
                    "notebook_create",
                    "source_add",
                    "nb_search",
                    "insight_run",
                    "insight_promote",
                ],
            ),
            (
                "output",
                &[
                    "show",
                    "show_web",
                    "show_pty",
                    "show_note",
                    "show_whiteboard",
                    "show_file",
                    "show_diff",
                    "show_view",
                    "pane_manage",
                    "pane_list",
                    "page_preview",
                    "artifact_read",
                    "app",
                    "message_user",
                    "ask_user",
                ],
            ),
            (
                "agents",
                &[
                    "spawn_subagent",
                    "send_message",
                    "discover_peers",
                    "peer_message",
                    "wait_peer",
                    "list_models",
                    "mesh_peers",
                    "agents_list",
                    "agent_put",
                    "agent_propose",
                    "agent_draft",
                ],
            ),
            ("tasks", &["task_create", "task_draft", "escalate"]),
            (
                "schedules",
                &[
                    "schedule",
                    "schedule_done",
                    "schedule_status",
                    "list_sensors",
                    "routine_draft",
                    "remind_me_in",
                    "cancel_reminder",
                ],
            ),
            (
                "autoresearch",
                &[
                    "experiment_run",
                    "autoresearch_draft",
                    "autoresearch_launch",
                    "autoresearch_launch_now",
                ],
            ),
            ("meetings", &["meeting_join", "brief_get"]),
            ("secrets", &["secret_get"]),
            (
                "decisions",
                &["decide", "decide_many", "decide_label", "double_ask"],
            ),
            (
                "dreaming",
                &[
                    "dreaming_run",
                    "case_put",
                    "case_list",
                    "skill_learned_edit",
                    "replay_case",
                    "preferences_set",
                ],
            ),
            (
                "setup",
                &[
                    "load_skill",
                    "add_skill",
                    "remove_skill",
                    "add_skill_repo",
                    "remove_skill_repo",
                    "sync_skills",
                    "add_mcp_server",
                    "remove_mcp_server",
                    "add_mcp_repo",
                    "sync_mcp",
                ],
            ),
            (
                "channels",
                &[
                    "signal_link",
                    "signal_status",
                    "slack_status",
                    "telegram_link",
                    "telegram_status",
                ],
            ),
        ];
        for (id, names) in cases {
            for name in *names {
                assert_eq!(category_of(name), *id, "{name}");
            }
        }
        assert_eq!(category_of("some_new_tool"), OTHER);
        assert_eq!(category_key_of("read_file"), "cat:files");
    }

    #[test]
    fn builtins_category_ids_are_unique_and_other_is_last() {
        let ids: HashSet<&str> = CATEGORIES.iter().map(|c| c.id).collect();
        assert_eq!(ids.len(), CATEGORIES.len());
        assert_eq!(CATEGORIES.last().unwrap().id, OTHER);
        assert!(CATEGORIES.iter().all(|c| !c.description.is_empty()));
        assert!(category("files").is_some() && category("nope").is_none());
    }

    #[test]
    fn builtins_summary_is_first_paragraph_cut_on_a_char_boundary() {
        assert_eq!(summary("One line.\n\nMore.\n\nEven more."), "One line.");
        assert_eq!(summary("  a\nb\n   \nc"), "a\nb");
        assert_eq!(summary("short"), "short");
        let long = "é".repeat(SUMMARY_CHARS + 5);
        let s = summary(&long);
        assert_eq!(s.chars().count(), SUMMARY_CHARS + 1);
        assert!(s.ends_with('…'));
        assert_eq!(
            summary(&"x".repeat(SUMMARY_CHARS)),
            "x".repeat(SUMMARY_CHARS)
        );
    }

    /// `tools.builtins.disabled:` left empty is no switch; missing is too.
    #[test]
    fn builtins_config_null_or_missing_is_empty() {
        use figment::providers::{Format as _, Yaml};
        let read = |yaml: &str| {
            figment::Figment::new()
                .merge(Yaml::string(yaml))
                .extract_inner::<BuiltinsConfig>("tools.builtins")
                .map_err(Box::new)
        };
        assert!(
            read("tools:\n  builtins:\n    disabled:\n")
                .unwrap()
                .disabled
                .is_empty()
        );
        assert!(
            read("tools:\n  builtins:\n    other: 1\n")
                .unwrap()
                .disabled
                .is_empty()
        );
        assert_eq!(
            read("tools:\n  builtins:\n    disabled: [\"cat:web\"]\n")
                .unwrap()
                .disabled,
            ["cat:web"]
        );
        assert!(read("tools:\n  builtins:\n    disabled: 3\n").is_err());
    }
}
