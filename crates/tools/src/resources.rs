//! Per-root file capability (design doc 51 §4.5, §8.2): where a session's
//! owners put session-scoped files and which lifecycle generation admitted
//! them. Crosses the tool contract on [`crate::ToolCx`]; not a client-set
//! path and not a filesystem routing service.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What happens to a root tree's data when it closes.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum Retention {
    /// Persisted in the durable store and data dir.
    #[default]
    Durable,
    /// In-memory store, scratch files; nothing survives the close (51 §4).
    Private,
}

/// Where session-scoped files go for one root tree.
///
/// For a durable tree `root` *is* the data dir, so every existing writer keeps
/// its `<data_dir>/<sub>/...` layout unchanged through [`Self::dir`]. For a
/// private tree `root` is a mode-0700 scratch directory under the OS temp
/// location and `dir(sub)` is `<scratch>/<sub>`; owners never write a private
/// tree's bytes under the data dir.
#[derive(Clone, Debug)]
pub struct SessionResources {
    /// Data dir (durable) or scratch dir (private).
    pub root: PathBuf,
    pub retention: Retention,
    /// Lifecycle generation that admitted the holder; a close bumps it, so a
    /// writer that outlived its generation can tell.
    pub generation: u64,
}

impl SessionResources {
    /// The durable capability: root is the data dir, generation 0.
    pub fn durable(data_dir: &Path) -> Self {
        Self {
            root: data_dir.to_path_buf(),
            retention: Retention::Durable,
            generation: 0,
        }
    }

    /// [`Self::durable`] shared, the shape [`crate::ToolCx`] carries.
    pub fn durable_arc(data_dir: &Path) -> Arc<Self> {
        Arc::new(Self::durable(data_dir))
    }

    pub fn is_private(&self) -> bool {
        self.retention == Retention::Private
    }

    /// `<root>/<sub>`: an owner's subdirectory (attachments, artifacts,
    /// spill, jobs, …).
    pub fn dir(&self, sub: &str) -> PathBuf {
        self.root.join(sub)
    }
}
