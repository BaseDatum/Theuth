//! System prompt sections: viewing, overriding, and resetting core prompt areas
//! and registered plugin prompt blocks.
//!
//! Overrides live as plain UTF-8 markdown files under `<data_dir>/prompts/<id>.md`,
//! except `core.standing` which reads and writes `<data_dir>/prompt.md` directly.
//! Disabled section ids are persisted separately in `<data_dir>/prompts/disabled.json`
//! so toggling a section off and back on preserves custom text.
//! When no override or disable file exists, prompt assembly is byte-identical to the defaults.

use std::collections::{BTreeMap, BTreeSet};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

/// Section id for standing instructions (`<data_dir>/prompt.md`).
pub const STANDING_ID: &str = "core.standing";

/// Default contents of `<data_dir>/prompt.md`.
pub const STANDING_DEFAULT: &str =
    "<!-- Standing instructions for the assistant; read at every model call. -->\n";

/// One editable section of the assembled system prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PromptSection {
    /// Stable identifier (`core.<key>` or `plugin.<key>`).
    pub id: String,
    /// Section group: `"core"` or `"plugin"`.
    pub group: String,
    /// Short human-readable title.
    pub label: String,
    /// One-line explanation of where and when this section is included.
    pub description: String,
    /// Effective text currently in use (override if customized, else `default_content`).
    pub content: String,
    /// Built-in default text before any override.
    pub default_content: String,
    /// Whether this section currently differs from its default via an override.
    pub customized: bool,
    /// Whether this section is enabled during system prompt assembly.
    pub enabled: bool,
}

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Directory storing per-section override files (`<data_dir>/prompts`).
pub fn overrides_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("prompts")
}

/// Path storing disabled prompt section ids (`<data_dir>/prompts/disabled.json`).
pub fn disabled_path(data_dir: &Path) -> PathBuf {
    overrides_dir(data_dir).join("disabled.json")
}

/// Validate a prompt section id: must be `core.<key>` or `plugin.<key>`,
/// where `<key>` is non-empty ASCII `[a-z0-9_-]+` (no path separators or extra dots).
pub fn validate_id(id: &str) -> Result<(), String> {
    let rest = id
        .strip_prefix("core.")
        .or_else(|| id.strip_prefix("plugin."))
        .ok_or_else(|| format!("invalid prompt section id `{id}`: expected core.* or plugin.*"))?;
    if rest.is_empty() {
        return Err(format!("invalid prompt section id `{id}`: empty key"));
    }
    if !rest
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
    {
        return Err(format!(
            "invalid prompt section id `{id}`: key must contain only ASCII [a-z0-9_-]"
        ));
    }
    Ok(())
}

/// Load all valid `<data_dir>/prompts/*.md` override files into a map keyed by section id.
/// Never panics on a missing directory, unreadable entry, or non-UTF-8 file.
pub fn load_overrides(data_dir: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(overrides_dir(data_dir)) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if validate_id(stem).is_err() {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(&path) {
            out.insert(stem.to_owned(), content);
        }
    }
    out
}

/// Load disabled prompt section ids from `<data_dir>/prompts/disabled.json`.
/// Never panics on a missing, unreadable, or malformed file; invalid ids are ignored.
pub fn load_disabled(data_dir: &Path) -> BTreeSet<String> {
    let Ok(raw) = std::fs::read_to_string(disabled_path(data_dir)) else {
        return BTreeSet::new();
    };
    let Ok(ids) = serde_json::from_str::<Vec<String>>(&raw) else {
        return BTreeSet::new();
    };
    ids.into_iter()
        .filter(|id| validate_id(id).is_ok())
        .collect()
}

/// Combined persisted override and enable/disable state for system prompt sections.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PromptState {
    /// Per-section override text loaded from `<data_dir>/prompts/<id>.md`.
    pub overrides: BTreeMap<String, String>,
    /// Disabled section ids loaded from `<data_dir>/prompts/disabled.json`.
    pub disabled: BTreeSet<String>,
}

impl PromptState {
    /// Returns `true` if `id` is currently disabled.
    pub fn is_disabled(&self, id: &str) -> bool {
        self.disabled.contains(id)
    }

    /// Resolve effective text for `id`: returns `""` if disabled, else the override if
    /// present, else `default`.
    pub fn resolve<'a>(&'a self, id: &str, default: &'a str) -> &'a str {
        if self.is_disabled(id) {
            ""
        } else {
            self.overrides.get(id).map(String::as_str).unwrap_or(default)
        }
    }
}

/// Load both override files and disabled section ids from `data_dir`.
pub fn load_state(data_dir: &Path) -> PromptState {
    PromptState {
        overrides: load_overrides(data_dir),
        disabled: load_disabled(data_dir),
    }
}

/// Replace registered plugin prompt texts with any matching `plugin.<key>` overrides.
#[allow(clippy::ptr_arg)]
pub fn apply_plugin_overrides(
    prompts: &mut Vec<(&'static str, String)>,
    overrides: &BTreeMap<String, String>,
) {
    for (plugin, text) in prompts.iter_mut() {
        if let Some(ov) = overrides.get(&format!("plugin.{plugin}")) {
            *text = ov.clone();
        }
    }
}

/// Apply override and disabled state to registered plugin prompt blocks.
#[allow(clippy::ptr_arg)]
pub fn apply_plugin_state(prompts: &mut Vec<(&'static str, String)>, state: &PromptState) {
    for (plugin, text) in prompts.iter_mut() {
        let id = format!("plugin.{plugin}");
        if state.is_disabled(&id) {
            *text = String::new();
        } else if let Some(ov) = state.overrides.get(&id) {
            *text = ov.clone();
        }
    }
}

/// Human-friendly `(label, description)` metadata for a registered plugin prompt key.
pub fn plugin_meta(plugin: &str) -> (String, String) {
    let (label, desc) = match plugin {
        "agent" => (
            "Harness layout",
            "Data directory layout, config files, vault, skills, logs, and /model switching.",
        ),
        "app" => (
            "Interactive apps",
            "Guidance for publishing and updating interactive OpenUI apps with the app tool.",
        ),
        "artifact" => (
            "Artifacts and files",
            "Handing over files, documents, HTML previews, diffs, terminals, and notes with show tools.",
        ),
        "attachments" => (
            "Attachments",
            "How to read attached notes, knowledge, artifacts, sessions, meetings, notebooks, sources, and whiteboards.",
        ),
        "autoresearch" => (
            "Autoresearch",
            "Setting up, monitoring, and controlling unattended measured experiment studies.",
        ),
        "cells" => (
            "User cells",
            "Writing runnable command cells and runbooks executed directly by the user.",
        ),
        "codeindex" => (
            "Code index",
            "Tree-sitter structural queries: code_find, code_outline, code_refs, code_symbol, code_map, and code_impact.",
        ),
        "computer" => (
            "Desktop and native apps",
            "Desktop screenshots, accessibility tree controls, and AppleScript/Shortcuts automation.",
        ),
        "decision" => (
            "Bulk classification",
            "Using the local decision model via decide and decide_many for fast sorting and triage.",
        ),
        "document" => (
            "Documents, boards, and whiteboards",
            "Block document editing, long-form code explanation boards, and whiteboard canvases.",
        ),
        "double" => (
            "Owner double",
            "Respecting and managing the owner's automated approval double.",
        ),
        "dreaming" => (
            "Standing preferences",
            "Learned standing preferences distilled by the nightly dreaming routine.",
        ),
        "knowledge" => (
            "Knowledge base",
            "Searching, peeking, and recording durable environment and project facts.",
        ),
        "meeting" => (
            "Meetings and briefs",
            "Live meeting transcripts, cached briefs, and post-meeting transcript queries.",
        ),
        "plugins" => (
            "External plugins",
            "Scaffolding, installing, inspecting, and managing external plugin folders.",
        ),
        "roster" => (
            "Agent hiring",
            "Drafting and managing standing agent roles in the roster.",
        ),
        "routes" => (
            "Learned site routes",
            "Learning and calling structured JSON routes from listing pages via site_learn and site_call.",
        ),
        "sandbox" => (
            "Sandbox VMs",
            "Creating, sharing directories with, and working inside isolated Linux sandbox VMs.",
        ),
        "schedule" => (
            "Schedules and loops",
            "Creating and running cron, interval, one-shot, and loop schedules.",
        ),
        "scrape" => (
            "Web scraping and crawling",
            "Reading sites with scrape_url, crawl, and map_site.",
        ),
        "search" => (
            "Local history search",
            "Fuzzy-searching saved sessions, artifacts, knowledge, notes, meetings, and notebooks.",
        ),
        "search-providers" => (
            "X search",
            "Searching X (Twitter) posts, profiles, and threads via search_x.",
        ),
        "secrets" => (
            "Secrets vault",
            "Using secret placeholders, secret_request cards, and secret_capture.",
        ),
        "slack" => (
            "Slack channel",
            "Formatting, length, and thread rules for Slack direct messages and channels.",
        ),
        "surface" => (
            "Visual presentation",
            "Show-first visual rules, json-render diagrams, and rich output formatting.",
        ),
        "talk" => (
            "Live talk",
            "Spoken style and pacing rules for real-time voice conversations.",
        ),
        "tasks" => (
            "Tasks board",
            "Drafting, creating, checking out, moving, and reassigning board tasks.",
        ),
        "tools" => (
            "MCP servers and tools",
            "Managing MCP server definitions, risk gates, and tool approvals.",
        ),
        "voice" => (
            "Voice notes",
            "Spoken style rules when replying to incoming voice notes.",
        ),
        "web" => (
            "Web search",
            "Searching and reading web pages with search_web and read_url.",
        ),
        _ => {
            let label = title_case_key(plugin);
            return (
                label,
                format!("Prompt guidance registered by the `{plugin}` plugin."),
            );
        }
    };
    (label.to_owned(), desc.to_owned())
}

fn title_case_key(key: &str) -> String {
    key.split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => {
                    let mut out = first.to_uppercase().to_string();
                    out.push_str(chars.as_str());
                    out
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn core_defaults() -> Vec<(&'static str, &'static str, &'static str, String)> {
    vec![
        (
            STANDING_ID,
            "Standing instructions",
            "Persistent instructions in prompt.md, read at every model call (supports <!-- root-only --> blocks).",
            STANDING_DEFAULT.to_owned(),
        ),
        (
            "core.header",
            "System persona",
            "Opening identity paragraph. Machine, working directory, date, and subagent defaults are appended automatically.",
            crate::frame::THEUTH_HEADER.to_owned(),
        ),
        (
            "core.guidance",
            "Core guidance",
            "Delegation, subagents, peer messaging, machine sharing, todo tracking, and reporting rules.",
            crate::frame::default_guidance(true),
        ),
        (
            "core.runtime",
            "Python runtime",
            "Instructions for the persistent Monty Python runtime. Tool signatures are appended automatically.",
            crate::repl::RUNTIME_BLOCK.to_owned(),
        ),
        (
            "core.status",
            "Status updates",
            "Instruction for calling set_status when the status tool is enabled.",
            crate::status::PROMPT.to_owned(),
        ),
        (
            "core.advisory",
            "Advisor",
            "Guidance added to sessions that have an advisor linked.",
            crate::frame::ADVISORY_PARAGRAPH.to_owned(),
        ),
        (
            "core.consult",
            "Consult",
            "Guidance added when the consult tool is available.",
            crate::frame::CONSULT_PARAGRAPH.to_owned(),
        ),
        (
            "core.notebooks",
            "Notebooks",
            "Guidance added to sessions bound to a notebook.",
            crate::notebooks::PROMPT.to_owned(),
        ),
        (
            "core.endless",
            "Endless sessions",
            "Instructions for endless-session view and history navigation.",
            crate::endless::Endless::view_doc().to_owned(),
        ),
    ]
}

/// List all core prompt sections followed by all registered plugin prompt sections.
pub fn list(data_dir: &Path, plugin_prompts: Vec<(&'static str, String)>) -> Vec<PromptSection> {
    let state = load_state(data_dir);
    let mut sections = Vec::with_capacity(9 + plugin_prompts.len());

    for (id, label, description, default_content) in core_defaults() {
        let (content, customized) = if id == STANDING_ID {
            let text = std::fs::read_to_string(data_dir.join("prompt.md"))
                .unwrap_or_else(|_| STANDING_DEFAULT.to_owned());
            let customized = text.trim() != STANDING_DEFAULT.trim();
            (text, customized)
        } else if let Some(ov) = state.overrides.get(id) {
            (ov.clone(), true)
        } else {
            (default_content.clone(), false)
        };
        sections.push(PromptSection {
            id: id.to_owned(),
            group: "core".to_owned(),
            label: label.to_owned(),
            description: description.to_owned(),
            content,
            default_content,
            customized,
            enabled: !state.is_disabled(id),
        });
    }

    for (plugin, default_content) in plugin_prompts {
        let id = format!("plugin.{plugin}");
        let (label, description) = plugin_meta(plugin);
        let (content, customized) = if let Some(ov) = state.overrides.get(&id) {
            (ov.clone(), true)
        } else {
            (default_content.clone(), false)
        };
        let enabled = !state.is_disabled(&id);
        sections.push(PromptSection {
            id,
            group: "plugin".to_owned(),
            label,
            description,
            content,
            default_content,
            customized,
            enabled,
        });
    }

    sections
}

/// Update one prompt section's override content and return the resulting [`PromptSection`].
pub fn set(
    data_dir: &Path,
    plugin_prompts: Vec<(&'static str, String)>,
    id: &str,
    content: &str,
) -> Result<PromptSection, String> {
    validate_id(id)?;
    let mut section = list(data_dir, plugin_prompts)
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| format!("unknown prompt section: {id}"))?;

    if id == STANDING_ID {
        write_atomic(&data_dir.join("prompt.md"), content)?;
        section.content = content.to_owned();
        section.customized = content.trim() != STANDING_DEFAULT.trim();
        return Ok(section);
    }

    let path = overrides_dir(data_dir).join(format!("{id}.md"));
    if content == section.default_content {
        remove_if_exists(&path)?;
        section.content = section.default_content.clone();
        section.customized = false;
    } else {
        write_atomic(&path, content)?;
        section.content = content.to_owned();
        section.customized = true;
    }
    Ok(section)
}

/// Enable or disable one prompt section without altering its override text, returning the
/// resulting [`PromptSection`].
pub fn set_enabled(
    data_dir: &Path,
    plugin_prompts: Vec<(&'static str, String)>,
    id: &str,
    enabled: bool,
) -> Result<PromptSection, String> {
    validate_id(id)?;
    let mut section = list(data_dir, plugin_prompts)
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| format!("unknown prompt section: {id}"))?;

    let mut disabled = load_disabled(data_dir);
    if enabled {
        disabled.remove(id);
    } else {
        disabled.insert(id.to_owned());
    }
    save_disabled(data_dir, &disabled)?;
    section.enabled = enabled;
    Ok(section)
}

/// Reset one prompt section back to its default content and enabled state, returning the
/// resulting [`PromptSection`].
pub fn reset(
    data_dir: &Path,
    plugin_prompts: Vec<(&'static str, String)>,
    id: &str,
) -> Result<PromptSection, String> {
    validate_id(id)?;
    let mut section = list(data_dir, plugin_prompts)
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| format!("unknown prompt section: {id}"))?;

    let mut disabled = load_disabled(data_dir);
    if disabled.remove(id) {
        save_disabled(data_dir, &disabled)?;
    }
    section.enabled = true;

    if id == STANDING_ID {
        write_atomic(&data_dir.join("prompt.md"), STANDING_DEFAULT)?;
        section.content = STANDING_DEFAULT.to_owned();
        section.customized = false;
        return Ok(section);
    }

    let path = overrides_dir(data_dir).join(format!("{id}.md"));
    remove_if_exists(&path)?;
    section.content = section.default_content.clone();
    section.customized = false;
    Ok(section)
}

fn save_disabled(data_dir: &Path, disabled: &BTreeSet<String>) -> Result<(), String> {
    let path = disabled_path(data_dir);
    if disabled.is_empty() {
        return remove_if_exists(&path);
    }
    let json = serde_json::to_string(disabled)
        .map_err(|e| format!("failed to serialize disabled prompt sections: {e}"))?;
    write_atomic(&path, &json)
}

fn write_atomic(path: &Path, content: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("cannot determine parent directory for {}", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("prompt");
    let seq = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = parent.join(format!(".{name}.{}.{seq}.tmp", std::process::id()));
    if let Err(e) = std::fs::write(&tmp, content) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("failed to write {}: {e}", path.display()));
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("failed to save {}: {e}", path.display()));
    }
    Ok(())
}

fn remove_if_exists(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("failed to remove {}: {e}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_plugins() -> Vec<(&'static str, String)> {
        vec![
            ("knowledge", "Default knowledge prompt.".to_owned()),
            ("surface", "Default surface prompt.".to_owned()),
        ]
    }

    #[test]
    fn validate_id_accepts_valid_and_rejects_traversal() {
        assert!(validate_id("core.standing").is_ok());
        assert!(validate_id("core.header").is_ok());
        assert!(validate_id("plugin.search-providers").is_ok());
        assert!(validate_id("plugin.custom_123").is_ok());

        for bad in [
            "",
            "core.",
            "plugin.",
            "other.foo",
            "core.a/b",
            "core.../etc/passwd",
            "plugin.a..b",
            "plugin.a.b",
            "core.Upper",
            "../core.header",
            "plugin.with space",
        ] {
            assert!(validate_id(bad).is_err(), "expected `{bad}` to be rejected");
        }
    }

    #[test]
    fn list_clean_returns_all_core_and_plugin_sections_uncustomized() {
        let dir = tempfile::tempdir().unwrap();
        let sections = list(dir.path(), sample_plugins());
        assert_eq!(sections.len(), 9 + 2);

        let core_ids: Vec<&str> = sections[..9].iter().map(|s| s.id.as_str()).collect();
        assert_eq!(
            core_ids,
            [
                "core.standing",
                "core.header",
                "core.guidance",
                "core.runtime",
                "core.status",
                "core.advisory",
                "core.consult",
                "core.notebooks",
                "core.endless",
            ]
        );
        for s in &sections {
            assert!(!s.customized, "expected {} to be uncustomized", s.id);
            assert!(s.enabled, "expected {} to be enabled", s.id);
            assert_eq!(s.content, s.default_content, "mismatch for {}", s.id);
        }
        assert_eq!(sections[9].id, "plugin.knowledge");
        assert_eq!(sections[9].group, "plugin");
        assert_eq!(sections[10].id, "plugin.surface");
        assert_eq!(sections[10].group, "plugin");
    }

    #[test]
    fn standing_set_and_reset_updates_prompt_md() {
        let dir = tempfile::tempdir().unwrap();
        let updated = set(
            dir.path(),
            sample_plugins(),
            STANDING_ID,
            "Always prefer concise diffs.\n",
        )
        .unwrap();
        assert!(updated.customized);
        assert_eq!(updated.content, "Always prefer concise diffs.\n");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("prompt.md")).unwrap(),
            "Always prefer concise diffs.\n"
        );
        assert!(!overrides_dir(dir.path()).exists());

        let restored = reset(dir.path(), sample_plugins(), STANDING_ID).unwrap();
        assert!(!restored.customized);
        assert_eq!(restored.content, STANDING_DEFAULT);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("prompt.md")).unwrap(),
            STANDING_DEFAULT
        );
    }

    #[test]
    fn override_files_load_apply_dedupe_default_and_reset() {
        let dir = tempfile::tempdir().unwrap();
        let header_custom = "Custom persona header.";
        let surface_custom = "Custom surface rules.";

        let h = set(dir.path(), sample_plugins(), "core.header", header_custom).unwrap();
        assert!(h.customized);
        assert_eq!(h.content, header_custom);
        assert!(overrides_dir(dir.path()).join("core.header.md").is_file());

        let s = set(
            dir.path(),
            sample_plugins(),
            "plugin.surface",
            surface_custom,
        )
        .unwrap();
        assert!(s.customized);
        assert_eq!(s.content, surface_custom);
        assert!(
            overrides_dir(dir.path())
                .join("plugin.surface.md")
                .is_file()
        );

        let overrides = load_overrides(dir.path());
        assert_eq!(
            overrides.get("core.header").map(String::as_str),
            Some(header_custom)
        );
        assert_eq!(
            overrides.get("plugin.surface").map(String::as_str),
            Some(surface_custom)
        );

        let mut plugins = sample_plugins();
        apply_plugin_overrides(&mut plugins, &overrides);
        assert_eq!(plugins[0], ("knowledge", "Default knowledge prompt.".to_owned()));
        assert_eq!(plugins[1], ("surface", surface_custom.to_owned()));

        // Saving text equal to default removes the override file.
        let same_as_default = set(
            dir.path(),
            sample_plugins(),
            "core.header",
            crate::frame::THEUTH_HEADER,
        )
        .unwrap();
        assert!(!same_as_default.customized);
        assert!(!overrides_dir(dir.path()).join("core.header.md").exists());

        // Reset removes the override file and restores default.
        let reset_surface = reset(dir.path(), sample_plugins(), "plugin.surface").unwrap();
        assert!(!reset_surface.customized);
        assert_eq!(reset_surface.content, "Default surface prompt.");
        assert!(!overrides_dir(dir.path()).join("plugin.surface.md").exists());
    }

    #[test]
    fn empty_string_override_is_preserved_to_suppress_section() {
        let dir = tempfile::tempdir().unwrap();
        let cleared = set(dir.path(), sample_plugins(), "plugin.surface", "").unwrap();
        assert!(cleared.customized);
        assert_eq!(cleared.content, "");
        assert!(
            overrides_dir(dir.path())
                .join("plugin.surface.md")
                .is_file()
        );

        let listed = list(dir.path(), sample_plugins());
        let surface = listed.iter().find(|s| s.id == "plugin.surface").unwrap();
        assert!(surface.customized);
        assert_eq!(surface.content, "");

        let mut plugins = sample_plugins();
        apply_plugin_overrides(&mut plugins, &load_overrides(dir.path()));
        assert_eq!(plugins[1], ("surface", String::new()));
    }

    #[test]
    fn set_enabled_toggles_and_preserves_custom_text() {
        let dir = tempfile::tempdir().unwrap();
        let standing_custom = "Custom standing rule.\n";
        let header_custom = "Custom header text.";
        let surface_custom = "Custom surface text.";

        set(dir.path(), sample_plugins(), STANDING_ID, standing_custom).unwrap();
        set(dir.path(), sample_plugins(), "core.header", header_custom).unwrap();
        set(dir.path(), sample_plugins(), "plugin.surface", surface_custom).unwrap();

        let h_off = set_enabled(dir.path(), sample_plugins(), "core.header", false).unwrap();
        assert!(!h_off.enabled);
        assert!(h_off.customized);
        assert_eq!(h_off.content, header_custom);

        let st_off = set_enabled(dir.path(), sample_plugins(), STANDING_ID, false).unwrap();
        assert!(!st_off.enabled);
        assert!(st_off.customized);
        assert_eq!(st_off.content, standing_custom);

        let sf_off = set_enabled(dir.path(), sample_plugins(), "plugin.surface", false).unwrap();
        assert!(!sf_off.enabled);
        assert!(sf_off.customized);
        assert_eq!(sf_off.content, surface_custom);

        // disabled.json stores sorted JSON array of ids.
        let raw = std::fs::read_to_string(disabled_path(dir.path())).unwrap();
        assert_eq!(raw, r#"["core.header","core.standing","plugin.surface"]"#);

        let listed = list(dir.path(), sample_plugins());
        for id in ["core.header", STANDING_ID, "plugin.surface"] {
            let s = listed.iter().find(|s| s.id == id).unwrap();
            assert!(!s.enabled, "expected {id} disabled");
            assert!(s.customized, "expected {id} still customized");
        }
        assert!(listed.iter().find(|s| s.id == "core.guidance").unwrap().enabled);

        // Re-enabling preserves custom text and removes disabled.json once empty.
        let h_on = set_enabled(dir.path(), sample_plugins(), "core.header", true).unwrap();
        assert!(h_on.enabled);
        assert!(h_on.customized);
        assert_eq!(h_on.content, header_custom);

        let st_on = set_enabled(dir.path(), sample_plugins(), STANDING_ID, true).unwrap();
        assert!(st_on.enabled);
        assert!(st_on.customized);
        assert_eq!(st_on.content, standing_custom);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("prompt.md")).unwrap(),
            standing_custom
        );

        let sf_on = set_enabled(dir.path(), sample_plugins(), "plugin.surface", true).unwrap();
        assert!(sf_on.enabled);
        assert!(sf_on.customized);
        assert_eq!(sf_on.content, surface_custom);
        assert_eq!(
            std::fs::read_to_string(overrides_dir(dir.path()).join("plugin.surface.md")).unwrap(),
            surface_custom
        );
        assert!(!disabled_path(dir.path()).exists());

        // Unknown and invalid ids are rejected.
        assert!(set_enabled(dir.path(), sample_plugins(), "plugin.nope", false).is_err());
        assert!(set_enabled(dir.path(), sample_plugins(), "../bad", false).is_err());
    }

    #[test]
    fn load_disabled_ignores_malformed_file_and_invalid_ids() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_disabled(dir.path()).is_empty());

        std::fs::create_dir_all(overrides_dir(dir.path())).unwrap();
        std::fs::write(disabled_path(dir.path()), "not valid json").unwrap();
        assert!(load_disabled(dir.path()).is_empty());

        std::fs::write(
            disabled_path(dir.path()),
            r#"["core.advisory","../escape","bad.id","plugin.slack"]"#,
        )
        .unwrap();
        let loaded = load_disabled(dir.path());
        assert_eq!(
            loaded,
            BTreeSet::from(["core.advisory".to_owned(), "plugin.slack".to_owned()])
        );
    }

    #[test]
    fn prompt_state_resolve_returns_empty_for_disabled_sections() {
        let dir = tempfile::tempdir().unwrap();
        set(dir.path(), sample_plugins(), "core.header", "Custom header").unwrap();
        set(dir.path(), sample_plugins(), "core.advisory", "Custom advisory").unwrap();
        set_enabled(dir.path(), sample_plugins(), "core.header", false).unwrap();
        set_enabled(dir.path(), sample_plugins(), "core.consult", false).unwrap();

        let state = load_state(dir.path());
        assert!(state.is_disabled("core.header"));
        assert!(state.is_disabled("core.consult"));
        assert!(!state.is_disabled("core.advisory"));

        assert_eq!(state.resolve("core.header", "Default header"), "");
        assert_eq!(state.resolve("core.consult", "Default consult"), "");
        assert_eq!(
            state.resolve("core.advisory", "Default advisory"),
            "Custom advisory"
        );
        assert_eq!(
            state.resolve("core.guidance", "Default guidance"),
            "Default guidance"
        );
    }

    #[test]
    fn apply_plugin_state_clears_disabled_plugins() {
        let dir = tempfile::tempdir().unwrap();
        set(
            dir.path(),
            sample_plugins(),
            "plugin.knowledge",
            "Custom knowledge.",
        )
        .unwrap();
        set(
            dir.path(),
            sample_plugins(),
            "plugin.surface",
            "Custom surface.",
        )
        .unwrap();
        set_enabled(dir.path(), sample_plugins(), "plugin.surface", false).unwrap();

        let state = load_state(dir.path());
        let mut plugins = sample_plugins();
        apply_plugin_state(&mut plugins, &state);

        assert_eq!(plugins[0], ("knowledge", "Custom knowledge.".to_owned()));
        assert_eq!(plugins[1], ("surface", String::new()));
    }

    #[test]
    fn reset_reenables_disabled_sections() {
        let dir = tempfile::tempdir().unwrap();
        set(dir.path(), sample_plugins(), STANDING_ID, "Custom standing.\n").unwrap();
        set(dir.path(), sample_plugins(), "plugin.surface", "Custom surface.").unwrap();
        set_enabled(dir.path(), sample_plugins(), STANDING_ID, false).unwrap();
        set_enabled(dir.path(), sample_plugins(), "plugin.surface", false).unwrap();

        let reset_standing = reset(dir.path(), sample_plugins(), STANDING_ID).unwrap();
        assert!(reset_standing.enabled);
        assert!(!reset_standing.customized);
        assert_eq!(reset_standing.content, STANDING_DEFAULT);
        assert_eq!(
            load_disabled(dir.path()),
            BTreeSet::from(["plugin.surface".to_owned()])
        );

        let reset_surface = reset(dir.path(), sample_plugins(), "plugin.surface").unwrap();
        assert!(reset_surface.enabled);
        assert!(!reset_surface.customized);
        assert_eq!(reset_surface.content, "Default surface prompt.");
        assert!(load_disabled(dir.path()).is_empty());
        assert!(!disabled_path(dir.path()).exists());
    }
}
