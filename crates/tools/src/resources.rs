//! Per-root file capability (design doc 51 §4.5, §8.2): where a session's
//! owners put session-scoped files and which lifecycle generation admitted
//! them. Crosses the tool contract on [`crate::ToolCx`]; not a client-set
//! path and not a filesystem routing service.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use runtime::SessionId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

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
    /// Whether `root` is the data dir or a scratch dir that dies on close.
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

    /// `true` when `retention` is [`Retention::Private`].
    pub fn is_private(&self) -> bool {
        self.retention == Retention::Private
    }

    /// `<root>/<sub>`: an owner's subdirectory (attachments, artifacts,
    /// spill, jobs, …).
    pub fn dir(&self, sub: &str) -> PathBuf {
        self.root.join(sub)
    }
}

/// A root tree is open (51 §4.4): every owner that registered an `opened`
/// recipient learns which sessions write under `resources`. `sessions` is
/// the root plus the descendants known so far; re-sent as a superset when
/// more appear, so handlers must be idempotent.
#[derive(Clone, Debug)]
pub struct TreeOpened {
    /// The tree's root session.
    pub root: SessionId,
    /// Root plus known descendants.
    pub sessions: Vec<SessionId>,
    /// Where the tree's owners write.
    pub resources: Arc<SessionResources>,
}

/// The close barrier (51 §4.4): every owner drops what it holds for the
/// `subtree` and replies [`CloseAck`]. `cancel` fires when the closer stops
/// waiting; work past it is best effort and its ack is not read.
#[derive(Clone, Debug)]
pub struct CloseTree {
    /// Every session of the closing tree, root included.
    pub subtree: Vec<SessionId>,
    /// The generation being closed; a later one is not this close.
    pub generation: u64,
    /// The closer stopped waiting.
    pub cancel: CancellationToken,
}

/// One owner's answer to [`CloseTree`]: `ok` when nothing of the subtree
/// remains under it; `detail` says what could not be verified.
#[derive(Clone, Debug, kameo::Reply)]
pub struct CloseAck {
    /// The participant's registered name.
    pub owner: String,
    /// Nothing of the subtree remains under the owner.
    pub ok: bool,
    /// What could not be verified or removed.
    pub detail: Option<String>,
}

impl CloseAck {
    /// `ok: true`, no detail.
    pub fn ok(owner: impl Into<String>) -> Self {
        Self {
            owner: owner.into(),
            ok: true,
            detail: None,
        }
    }

    /// `ok: false` with `detail`.
    pub fn failed(owner: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            owner: owner.into(),
            ok: false,
            detail: Some(detail.into()),
        }
    }
}
