//! The `app` tool (46 §3): validate OpenUI source against the current
//! visible tools with the helper, then commit it as a [`Kind::App`] record
//! with its metadata sidecar. Edits merge onto a base record and commit only
//! while that base is still the latest version.

use std::sync::Arc;

use artifact::{Commit, CommitError, Kind};
use runtime::SessionId;
use serde::{Deserialize, Serialize};
use tools::{Risk, Tool, ToolCx, ToolInspection, ToolOutput};

use crate::actor::{Core, Loaded};
use crate::helper::{self, CheckResult, ToolSnapshot};
use crate::meta::{self, AppMeta, MAX_SOURCE};

/// Longest app name (before `.openui`).
const MAX_NAME: usize = 64;

/// Arguments of `app`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AppArgs {
    /// App name, e.g. `argo-health`; becomes `argo-health.openui`. The same name again in
    /// this session is a new version.
    pub name: String,
    /// Complete OpenUI Lang source, or with `edit: true` a patch: statements to replace/add,
    /// `name = null` to delete.
    pub source: String,
    /// Card title. Default: the name.
    pub title: Option<String>,
    /// `source` is a patch onto `base_call_id`; false (default) replaces the whole app.
    #[serde(default)]
    pub edit: bool,
    /// The `record_call_id` from the receipt of the version being edited. Required with `edit`.
    pub base_call_id: Option<String>,
}

/// The generated component catalog (`packages/apps`, `bun run generate`).
const CATALOG: &str = include_str!("../../../packages/apps/generated/catalog.txt");

/// The smallest complete app: layout first, state, text, one press.
pub const EXAMPLE: &str = "root = Stack([title, status, controls])\n\
$stage = \"idle\"\n\
title = TextContent(\"## Cutover drill\")\n\
status = TextContent(\"Stage: \" + $stage)\n\
controls = Buttons([begin, reset, docs])\n\
begin = Button(\"Begin drill\", Action([@Set($stage, \"cutover\")]))\n\
reset = Button(\"Reset\", Action([@Reset($stage)]), \"secondary\")\n\
docs = Button(\"Runbook\", Action([@OpenUrl(\"https://example.com/runbook\")]))";

/// Components whose compact signature the prompt block carries (46 §9 as-built): the
/// ones a first attempt reaches for, including the explanatory set an inline ```openui
/// reply fence uses; the rest live in the `apps` skill's full catalog.
const PROMPT_COMPONENTS: [&str; 31] = [
    "Stack",
    "Card",
    "CardHeader",
    "TextContent",
    "Image",
    "Table",
    "Col",
    "BarChart",
    "LineChart",
    "Series",
    "Tabs",
    "TabItem",
    "Steps",
    "StepsItem",
    "Callout",
    "MetricIndicatorInline",
    "ListBlock",
    "ListItem",
    "Button",
    "Buttons",
    "Input",
    "Select",
    "SelectItem",
    "Tag",
    "Accordion",
    "AccordionItem",
    "CodeBlock",
    "Chips",
    "ChipItem",
    "OptionCards",
    "OptionCard",
];

/// Prompt block registered as `RegisterPrompt { plugin: "app" }` (46 §9):
/// a function of the generated catalog only, so it never busts the system
/// cache. Front-loads what a first attempt gets wrong without the skill —
/// the exact component names and their positional arguments — as compact
/// named signatures for [`PROMPT_COMPONENTS`], plus one complete example;
/// the full catalog is the `apps` skill's.
pub fn prompt() -> String {
    let mut out = String::with_capacity(4096);
    out.push_str("## Interactive screens: `app`\n\
Use `app {name, source}` for a clickable prototype or a live interactive screen (desktop/web, progressive). Surface fences are for a static picture (an ```openui fence in a reply renders inline in the message flow exactly like a json-render block: `$state`, @Set/@Reset/@OpenUrl and inputs work, Query/Mutation/@ToAssistant are inert); apps are for live data and actions; `show` for files. OpenUI Lang: one `name = expression` per line; `root` is required; components take POSITIONAL arguments in the signature order below (skip an optional one with `null`); `$name = literal` declares state; only the component names below exist (no Heading/Text/State/OpenURL). A button press approves its action: say the consequence in the label, never add a second confirmation. Links, external images and services are welcome. `Query(tool, args, defaults, refreshSeconds?)` reads an installed tool; `Mutation(tool, args)` runs only from `@Run` in an activated action. Prefer installed tools, then `http_get`/`http_post` for public HTTP APIs. Every statement must be complete on its own line; the receipt's `record_call_id` is what you pass back with `edit: true` to change the app instead of republishing it. Once the app is published, the user has it: do not describe it back.\n\
Example:\n```openui\n");
    out.push_str(EXAMPLE);
    out.push_str("\n```\nComponents (positional args, `?` optional): ");
    out.push_str(&compact_signatures(&PROMPT_COMPONENTS).join("; "));
    out.push_str(
        "\nActions: @Run(queryOrMutation), @Set($var, value), @Reset($var), @OpenUrl(url), @ToAssistant(message, context?). \
         Builtins: @Each(array, item, template), @Count, @Sum, @Avg, @Min, @Max, @Sort(array, field, dir?), @Filter(array, field, op, value), @Round, @Abs, @Floor, @Ceil. \
         Results: query value is the tool's structured result; mutation exposes .status/.data/.error. \
         Full catalog (Carousel, ImageGallery, RadarChart, ScatterChart, forms…): the `apps` skill.",
    );
    out
}

/// `Stack(children, direction?, gap?, …)` for every component of `only` in the
/// catalog, in catalog order: names and argument names only, no types or prose.
/// The first line per name wins: the usage notes repeat `Button(label, Action(…))`.
fn compact_signatures(only: &[&str]) -> Vec<String> {
    let mut seen: Vec<&str> = Vec::new();
    CATALOG
        .lines()
        .filter_map(|line| {
            let paren = line.find('(')?;
            let name = &line[..paren];
            if name.is_empty()
                || !name.chars().next()?.is_ascii_uppercase()
                || !name.chars().all(|c| c.is_ascii_alphanumeric())
                || !only.contains(&name)
                || seen.contains(&name)
            {
                return None;
            }
            seen.push(name);
            let params = top_level_params(&line[paren + 1..]);
            let names: Vec<&str> = params
                .iter()
                .map(|p| p.split(':').next().unwrap_or(p).trim())
                .filter(|n| !n.is_empty())
                .collect();
            Some(format!("{name}({})", names.join(", ")))
        })
        .collect()
}

/// Split a signature's parameter list on top-level commas, stopping at the
/// matching `)`; nested `(`, `[`, `{` and quoted strings are skipped over.
fn top_level_params(rest: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut quote = false;
    let mut start = 0;
    for (i, c) in rest.char_indices() {
        match c {
            '"' => quote = !quote,
            _ if quote => {}
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' if depth > 0 => depth -= 1,
            ')' => {
                out.push(&rest[start..i]);
                return out;
            }
            ',' if depth == 0 => {
                out.push(&rest[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out
}

/// `app`: publish a validated OpenUI app. It publishes UI; it never runs the UI's tools.
pub struct AppTool {
    /// Shared with the actor.
    pub core: Arc<Core>,
}

#[async_trait::async_trait]
impl Tool for AppTool {
    const NAME: &'static str = "app";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "dict";
    const DIRECT: bool = true;
    /// Source is an artifact: a placeholder in it is committed literally and
    /// resolved only by the Query/Mutation call that later consumes it (46 §6).
    const CONSUMES_SECRETS: bool = false;
    type Args = AppArgs;

    fn description(&self) -> String {
        "Publish an interactive OpenUI Lang app the user can open and click: state, buttons, \
         forms, tables, charts, live Query reads of installed tools and Mutation actions. \
         Renders progressively while you write it. Pass the complete `source` for a new app, or \
         `edit: true` with `base_call_id` from the receipt and a patch of statements to change \
         (`name = null` deletes). Errors name the statement and the signature to fix. Use only \
         the component names and positional signatures listed in the system prompt's `app` \
         section; the `apps` skill has full types and examples."
            .into()
    }

    async fn call(&self, args: AppArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        Ok(match publish(&self.core, args, cx).await {
            Ok(out) => out,
            Err(msg) => ToolOutput::err(format!("app: {msg}")),
        })
    }
}

/// Who is publishing: the model through the `app` tool (receipt and errors talk to the
/// model) or the desktop through `publish_app` (30 §9: a reply fence opened as an app).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// The `app` tool; `call_id` is the tool call's.
    Tool,
    /// The `publish_app` RPC; `call_id` is synthetic (`ui-<uuid>`), no transcript entry.
    Ui,
}

/// Limits → base (edit) → snapshot → helper → commit, remembered in the cache. Shared by
/// the `app` tool and the `publish_app` RPC; only the wording differs by `origin`.
pub(crate) async fn commit_source(
    core: &Core,
    args: AppArgs,
    session: SessionId,
    call_id: String,
    origin: Origin,
) -> Result<(Arc<Loaded>, CheckResult), String> {
    let name = file_name(&args.name)?;
    if args.source.len() > MAX_SOURCE {
        return Err(format!(
            "source is {} bytes; the limit is {MAX_SOURCE}",
            args.source.len()
        ));
    }
    if args.source.trim().is_empty() {
        return Err("source is empty".into());
    }
    let artifacts = core.artifacts()?;
    let registry = core.registry()?;

    let base = if args.edit {
        let base_id = args
            .base_call_id
            .as_deref()
            .filter(|b| !b.is_empty())
            .ok_or(
                "edit: true needs base_call_id from the receipt of the version you are editing",
            )?;
        let base = core
            .load(base_id)
            .await
            .map_err(|e| format!("base {base_id}: {e}"))?;
        if base.artifact.session != session {
            return Err(format!("base {base_id} belongs to another session"));
        }
        if base.artifact.name != name {
            return Err(format!(
                "base {base_id} is {}, not {name}; edit it under its own name",
                base.artifact.name
            ));
        }
        Some(base)
    } else {
        if args.base_call_id.is_some() {
            return Err("base_call_id is only used with edit: true".into());
        }
        None
    };

    let inspections: Vec<ToolInspection> = registry
        .ask(tools::DescribeTools(Some(session)))
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

    let check = match &base {
        Some(b) => {
            core.helper
                .merge_check(&args.source, &b.source, &snapshot)
                .await
        }
        None => core.helper.check(&args.source, &snapshot).await,
    };
    let check = match check {
        Ok(c) => c,
        Err(helper::Error::Missing(p)) => {
            return Err(match origin {
                Origin::Tool => format!(
                    "apps are unavailable on this harness: parser helper missing at {}. Existing apps stay readable; use `show` for a static file.",
                    p.display()
                ),
                Origin::Ui => format!(
                    "apps are unavailable on this harness: parser helper missing at {}",
                    p.display()
                ),
            });
        }
        Err(e) => return Err(e.to_string()),
    };
    if check.library_hash != crate::hash() {
        return Err(format!(
            "parser helper validated against library {}, this harness is {}",
            check.library_hash,
            crate::hash()
        ));
    }
    if !check.diagnostics.is_empty() {
        return Err(diagnostics(&check, origin));
    }
    if check.source.len() > MAX_SOURCE {
        return Err(format!(
            "merged source is {} bytes; the limit is {MAX_SOURCE}",
            check.source.len()
        ));
    }

    let bound: Vec<ToolInspection> = inspections
        .into_iter()
        .filter(|t| check.bindings.iter().any(|b| b.tool == t.spec.name))
        .collect();
    let snapshots = meta::snapshots(&core.rt.data_dir, &bound);
    let mut meta = AppMeta::new(&check, &bound, snapshots);
    let bytes = meta.to_bytes()?;
    let title = args
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| name.trim_end_matches(".openui"))
        .to_owned();
    let committed = artifacts
        .ask(Commit {
            session,
            call_id,
            name: name.clone(),
            title,
            kind: Kind::App,
            bytes: check.source.clone().into_bytes(),
            source: Some(("app.json".into(), bytes)),
            expected_base: base.as_ref().map(|b| b.artifact.call_id.clone()),
        })
        .await;
    let artifact = match committed {
        Ok(a) => a,
        Err(kameo::error::SendError::HandlerError(CommitError::Conflict { latest })) => {
            return Err(match latest {
                Some(l) => format!(
                    "conflict: {} is no longer the latest {name}; {l} is. Re-read that version and edit it with base_call_id {l}, or publish the complete source.",
                    args.base_call_id.unwrap_or_default()
                ),
                None => format!(
                    "conflict: {name} no longer exists in this session; publish the complete source"
                ),
            });
        }
        Err(e) => return Err(e.to_string()),
    };

    let loaded = Arc::new(Loaded {
        artifact,
        source: check.source.clone(),
        meta,
    });
    core.remember(loaded.clone());
    Ok((loaded, check))
}

/// The `app` tool: [`commit_source`] under the call's id, then the receipt.
async fn publish(core: &Core, args: AppArgs, cx: ToolCx) -> Result<ToolOutput, String> {
    let (loaded, check) =
        commit_source(core, args, cx.session, cx.call_id.clone(), Origin::Tool).await?;
    let artifact = &loaded.artifact;

    let queries = check.bindings.iter().filter(|b| b.kind == "query").count();
    let mutations = check.bindings.len() - queries;
    let mut tools_used: Vec<String> = check
        .bindings
        .iter()
        .map(|b| format!("{}({})", b.tool, risk_name(b.risk)))
        .collect();
    tools_used.sort();
    tools_used.dedup();
    let mut content = format!(
        "published {} v{} · record_call_id {} · {} statements · {queries} queries · {mutations} mutations",
        artifact.name,
        artifact.version,
        artifact.call_id,
        check.statements.len(),
    );
    if !tools_used.is_empty() {
        content.push_str(&format!(" · tools: {}", tools_used.join(", ")));
    }
    if !check.orphaned.is_empty() {
        content.push_str(&format!(
            "\nnote: unreachable from root (never rendered): {}",
            check.orphaned.join(", ")
        ));
    }
    content.push_str("\nEdit with edit: true and base_call_id set to this record_call_id.");
    Ok(ToolOutput {
        structured: Some(serde_json::json!({
            "artifact": artifact,
            "record_call_id": artifact.call_id,
            "statements": check.statements.len(),
            "queries": queries,
            "mutations": mutations,
            "bindings": check.bindings,
        })),
        ..ToolOutput::ok(content)
    })
}

fn risk_name(r: Risk) -> &'static str {
    match r {
        Risk::ReadOnly => "read_only",
        Risk::Mutating => "mutating",
        Risk::Destructive => "destructive",
    }
}

/// `argo-health` → `argo-health.openui`; a plain file name only.
fn file_name(name: &str) -> Result<String, String> {
    let stem = name.trim().trim_end_matches(".openui");
    if stem.is_empty() {
        return Err("name is required".into());
    }
    if stem.len() > MAX_NAME {
        return Err(format!("name is longer than {MAX_NAME} characters"));
    }
    if !stem
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        || stem.starts_with('.')
        || stem.contains("..")
    {
        return Err(format!(
            "name {name:?} must be letters, digits, `-`, `_` or `.` (not leading), e.g. argo-health"
        ));
    }
    Ok(format!("{stem}.openui"))
}

/// Located diagnostics plus the catalog signature of any component they name.
fn diagnostics(check: &CheckResult, origin: Origin) -> String {
    let mut out = String::from(match origin {
        Origin::Tool => "not published; fix and call `app` again:\n",
        Origin::Ui => "not published; the source does not validate:\n",
    });
    let mut mentioned = String::new();
    for d in &check.diagnostics {
        let at = if d.at.is_empty() {
            "program"
        } else {
            d.at.as_str()
        };
        out.push_str(&format!("- [{}] {at}: {}\n", d.code, d.message));
        mentioned.push_str(&d.message);
        mentioned.push('\n');
    }
    let sigs = signatures_for(&mentioned);
    if !sigs.is_empty() {
        out.push_str("signatures:\n");
        for line in sigs {
            out.push_str("  ");
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push_str("Signatures and examples: `apps` skill.");
    out
}

/// Catalog signature lines of every component whose name occurs in `text`,
/// in catalog order.
pub(crate) fn signatures_for(text: &str) -> Vec<&'static str> {
    CATALOG
        .lines()
        .filter(|line| {
            let Some(paren) = line.find('(') else {
                return false;
            };
            let component = &line[..paren];
            !component.is_empty()
                && component.chars().all(|c| c.is_ascii_alphanumeric())
                && component
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_uppercase())
                && text.contains(component)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppDiagnostic;

    #[test]
    fn prompt_front_loads_compact_signatures_and_a_complete_example() {
        let sigs = compact_signatures(&PROMPT_COMPONENTS);
        assert_eq!(sigs.len(), PROMPT_COMPONENTS.len(), "every listed component is in the catalog: {sigs:?}");
        assert!(
            sigs.contains(&"Stack(children, direction?, gap?, align?, justify?, wrap?)".to_owned()),
            "{sigs:?}"
        );
        assert!(
            sigs.contains(&"Button(label, action?, variant?, type?, size?)".to_owned()),
            "{sigs:?}"
        );
        assert!(
            sigs.contains(&"TextContent(text, size?)".to_owned()),
            "{sigs:?}"
        );
        assert!(sigs.contains(&"Steps(items)".to_owned()), "{sigs:?}");
        // Nested types with commas/braces do not split a parameter.
        assert!(
            sigs.iter()
                .any(|s| s.starts_with("Input(name, placeholder?, type?, rules?, value?)")),
            "{sigs:?}"
        );
        assert!(
            sigs.iter().all(|s| !s.contains(':') && !s.contains('"')),
            "no types leak: {sigs:?}"
        );
        // The explanatory set an inline reply fence reaches for is in the prompt; the rest is the skill's.
        assert!(sigs.contains(&"Accordion(items)".to_owned()), "{sigs:?}");
        assert!(sigs.iter().any(|s| s.starts_with("OptionCards(")), "{sigs:?}");
        assert!(!sigs.iter().any(|s| s.starts_with("Carousel(")), "{sigs:?}");
        assert!(compact_signatures(&["Carousel"]).len() == 1);
        let p = prompt();
        assert!(p.contains("```openui\nroot = Stack(["));
        assert!(p.contains("no Heading/Text/State/OpenURL"));
        assert!(p.ends_with("Full catalog (Carousel, ImageGallery, RadarChart, ScatterChart, forms…): the `apps` skill."));
        assert!(p.contains("renders inline in the message flow exactly like a json-render block"));
        assert!(!p.contains("Carousel("));
        assert!(crate::skill().contains("Carousel("));
        assert!(
            p.len() < 6 * 1024,
            "prompt stays compact: {} bytes",
            p.len()
        );
    }

    #[test]
    fn names_are_plain_files_with_the_openui_extension() {
        assert_eq!(file_name("argo-health").unwrap(), "argo-health.openui");
        assert_eq!(file_name(" panel.openui ").unwrap(), "panel.openui");
        assert_eq!(file_name("v1.2_beta").unwrap(), "v1.2_beta.openui");
        for bad in [
            "",
            "../x",
            "a/b",
            ".hidden",
            "sp ace",
            &"x".repeat(MAX_NAME + 1),
        ] {
            assert!(file_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn diagnostics_locate_and_attach_the_named_signature() {
        let check = CheckResult {
            diagnostics: vec![
                AppDiagnostic {
                    at: "title".into(),
                    message: "Card: missing required argument children".into(),
                    code: "missing-required".into(),
                },
                AppDiagnostic {
                    at: String::new(),
                    message: "no root statement".into(),
                    code: "no-root".into(),
                },
            ],
            ..CheckResult::default()
        };
        let text = diagnostics(&check, Origin::Tool);
        assert!(text.contains("[missing-required] title:"));
        assert!(text.contains("[no-root] program:"));
        assert!(text.contains("\n  Card("), "{text}");
        assert!(
            !text.contains("\n  CardHeader("),
            "only the named component"
        );
    }
}
