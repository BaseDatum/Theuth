//! Actor messages of [`crate::PluginsActor`] (contract §5.2). B0, frozen. `name` is always the
//! plugin name; an unknown name answers [`PluginsError::NotFound`](crate::PluginsError::NotFound).

use std::path::PathBuf;

use runtime::SessionId;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::press::HumanPress;
use crate::types::Lang;

/// `ask` → `Vec<PluginInfo>` (order: grants.yaml `order`, then name).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ListPlugins;

/// `ask` → `Result<PluginDetail, PluginsError>`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GetPlugin {
    /// Plugin name.
    pub name: String,
}

/// `ask` → `Result<CheckReport, PluginsError>` (full check: vault + catalog).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CheckPath {
    /// Absolute folder path.
    pub path: PathBuf,
}

/// `ask` → `Result<InstallReport, PluginsError>` (copy folder → plugins/<name>; `Exists` if
/// present; card filed by the watcher).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Install {
    /// Absolute folder path holding `plugin.yaml`.
    pub path: PathBuf,
}

/// `ask` → `Result<ScaffoldReport, PluginsError>` (calls `scaffold::new_plugin`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct New {
    /// Plugin name.
    pub name: String,
    /// SDK language.
    pub lang: Lang,
    /// No SDK: the raw protocol template (py and sh only).
    pub raw: bool,
}

/// `ask` → `Result<PluginInfo, PluginsError>`. Not `Deserialize`: it carries a [`HumanPress`].
#[derive(Clone, Debug)]
pub struct Approve {
    /// Plugin name.
    pub name: String,
    /// The pending card's manifest hash, verbatim.
    pub manifest_sha256: String,
    /// The pending card's code hash, verbatim.
    pub code_sha256: Option<String>,
    /// Built only by `HumanPress::from_rpc` (§8).
    pub press: HumanPress,
}

/// `ask` → `Result<InstallCard, PluginsError>` (re-files the pending card with seq+1; `Invalid`
/// if the manifest does not parse; error "nothing to approve" (`Invalid`) when the grant already
/// covers it).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RequestApproval {
    /// Plugin name.
    pub name: String,
}

/// `ask` → `Result<PluginInfo, PluginsError>` (no grant: Ok with state awaiting_approval,
/// nothing starts).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetEnabled {
    /// Plugin name.
    pub name: String,
    /// Enable (start if approved) or disable (stop).
    pub on: bool,
}

/// `ask` → `Result<PluginInfo, PluginsError>`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetHidden {
    /// Plugin name.
    pub name: String,
    /// Placement id.
    pub placement: String,
    /// Hide (true) or show.
    pub hidden: bool,
}

/// `ask` → `Result<Vec<PluginInfo>, PluginsError>` (names not listed keep their relative order
/// after the listed ones).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetOrder {
    /// Plugin names, first = top.
    pub order: Vec<String>,
}

/// `ask` → `Result<PluginInfo, PluginsError>` (also clears `failed`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Restart {
    /// Plugin name.
    pub name: String,
}

/// `ask` → `Result<(), PluginsError>`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Uninstall {
    /// Plugin name.
    pub name: String,
    /// Keep `plugin-data/<name>` in place.
    pub keep_data: bool,
}

/// `ask` → `Result<Value, PluginsError>`: a press on a placement, delivered as `action`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Action {
    /// Plugin name.
    pub name: String,
    /// Placement id.
    pub placement: String,
    /// Action name.
    pub action: String,
    /// Action arguments.
    pub args: Value,
    /// The chat the press came from.
    pub session: Option<SessionId>,
    /// `x-theuth-client` of the caller (`desktop` by default).
    pub client: String,
    /// The renderer's action id of the press; required for an OpenUI Mutation.
    pub activation: Option<String>,
    /// Form values of the press.
    pub form: Option<Value>,
}

/// `ask` → `Result<Value, PluginsError>`: an OpenUI query, delivered as `query`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Query {
    /// Plugin name.
    pub name: String,
    /// Placement id.
    pub placement: String,
    /// Query name.
    pub query: String,
    /// Query arguments.
    pub args: Value,
    /// The chat the surface is drawn beside.
    pub session: Option<SessionId>,
    /// `x-theuth-client` of the caller (`desktop` by default).
    pub client: String,
}

/// `ask` → `Result<CommandReply, PluginsError>`: a `/plugin[:cmd]` command, delivered as `command`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Command {
    /// Plugin name.
    pub name: String,
    /// Command name (as declared).
    pub command: String,
    /// The rest of the line.
    pub args: String,
    /// The chat it was typed in.
    pub session: Option<SessionId>,
    /// `x-theuth-client` of the caller (`desktop` by default).
    pub client: String,
}

/// `ask` → `Vec<CatalogEntry>`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Catalog {
    /// Only this topic.
    pub topic: Option<String>,
}

/// `ask` → `Vec<SurfaceSnapshot>`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Surfaces;

/// `ask` → `Result<SurfaceContent, PluginsError>` (no content held + plugin running → sends
/// `render`, returns state loading).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Surface {
    /// Plugin name.
    pub name: String,
    /// Placement id.
    pub placement: String,
    /// The chat the surface is drawn beside.
    pub session: Option<SessionId>,
    /// `x-theuth-client` of the caller (`desktop` by default).
    pub client: String,
}

/// `ask` → `Result<LogTail, PluginsError>` (tail capped at 2000).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Logs {
    /// Plugin name.
    pub name: String,
    /// Lines wanted.
    pub tail: u32,
}
