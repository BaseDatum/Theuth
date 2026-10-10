//! 158 W4 owns this file (stub from C158).
//!
//! Note templates (158 §11.1, D14/W-p): markdown files with YAML front matter under
//! `<data_dir>/meetings/templates/<slug>.md`; built-ins embedded in the binary and rewritten
//! only while unedited; `summary::form_for` compiles the chosen one into the summary form. The
//! stub lists nothing and writes nothing — today's fixed form stays in force.

use runtime::Runtime;
use serde::{Deserialize, Serialize};

use crate::{Manifest, MeetingId};

/// Directory under `<data_dir>/meetings/`.
pub const DIR: &str = "templates";
/// The built-in that reproduces today's five headings; `Manifest.template`'s default.
pub const DEFAULT: &str = "meeting";

/// One section of the summary form.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(
    feature = "ts",
    derive(ts_rs::TS),
    ts(export, rename = "TemplateSection")
)]
pub struct Section {
    /// `## <title>` in the note.
    pub title: String,
    /// Per-section instruction for the model.
    #[serde(default)]
    pub instruction: Option<String>,
    /// Keep the heading even when empty.
    #[serde(default)]
    pub required: bool,
}

/// A template as `meeting_templates` lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(
    feature = "ts",
    derive(ts_rs::TS),
    ts(export, rename = "MeetingTemplate")
)]
pub struct Template {
    /// File stem.
    pub slug: String,
    /// Front matter `name`.
    pub name: String,
    /// Front matter `description`.
    #[serde(default)]
    pub description: String,
    /// In order.
    pub sections: Vec<Section>,
    /// Shipped with the binary (delete refused, "Reset" rewrites it).
    #[serde(default)]
    pub builtin: bool,
}

/// `meeting_templates() -> Vec<Template>` (§15.2, W4). Stub: none.
pub async fn rpc_templates(_rt: &Runtime) -> anyhow::Result<Vec<Template>> {
    Ok(Vec::new())
}

/// `meeting_template_put(slug, body) -> Template` (W4): write the file, parse it back. Stub: refused.
pub async fn rpc_template_put(
    _rt: &Runtime,
    _slug: String,
    _body: String,
) -> anyhow::Result<Template> {
    Err(anyhow::anyhow!("158 W4: not built yet"))
}

/// `meeting_template_delete(slug)` (W4): refused for built-ins. Stub: refused.
pub async fn rpc_template_delete(_rt: &Runtime, _slug: String) -> anyhow::Result<()> {
    Err(anyhow::anyhow!("158 W4: not built yet"))
}

/// `meeting_set_template(id, template) -> Manifest` (W4): changeable until Summarize. Stub: refused.
pub async fn rpc_set_template(
    _rt: &Runtime,
    _id: MeetingId,
    _template: String,
) -> anyhow::Result<Manifest> {
    Err(anyhow::anyhow!("158 W4: not built yet"))
}
