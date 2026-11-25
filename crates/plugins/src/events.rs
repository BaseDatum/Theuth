//! The two wire topics of the plugins host (contract §5.3). B0, frozen. Both are `Public`.

use runtime::SessionId;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::types::{Badge, Dot, PluginRunState, SurfaceFormat};

/// A plugin emitted one of its declared events (216 §7.6). Topic `plugins.events` v1.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct PluginEvent {
    /// The emitting plugin.
    pub plugin: String,
    /// Declared name without the plugin prefix (`limit_reached`).
    pub event: String,
    /// The plugin event's own version (`emits.<name>.version`).
    pub version: u32,
    /// The session it concerns, if any.
    pub session: Option<SessionId>,
    /// Redacted, a JSON object.
    pub data: Value,
}

impl wire::WireEvent for PluginEvent {
    const TOPIC: &'static str = "plugins.events";
}

/// Placement and lifecycle changes for clients (216 §2, §9). Topic `plugins.ui` v1.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub enum PluginUi {
    /// A placement's dot/badge/label/tooltip changed (already state-overridden).
    PluginStatus {
        /// Plugin name.
        plugin: String,
        /// Placement id.
        placement: String,
        /// The dot.
        dot: Dot,
        /// The badge, if any.
        badge: Option<Badge>,
        /// The label override.
        label: Option<String>,
        /// The tooltip.
        tooltip: Option<String>,
    },
    /// A placement has new content; fetch it with `plugin_surface` (big sources stay out of the feed).
    PluginSurface {
        /// Plugin name.
        plugin: String,
        /// Placement id.
        placement: String,
        /// Content format.
        format: SurfaceFormat,
        /// New content revision.
        revision: u64,
    },
    /// Re-run the placement's OpenUI queries without replacing the source (`invalidate`).
    PluginInvalidate {
        /// Plugin name.
        plugin: String,
        /// Placement id.
        placement: String,
    },
    /// A plugin changed lifecycle state.
    PluginState {
        /// Plugin name.
        plugin: String,
        /// New state.
        state: PluginRunState,
        /// Last error, if any.
        error: Option<String>,
    },
    /// The set of plugins, their placements, order, hidden flags or cards changed: re-read `plugin_list`.
    PluginsChanged,
}

impl wire::WireEvent for PluginUi {
    const TOPIC: &'static str = "plugins.ui";
}
