//! Artifact plugin (design doc 35): files the agent makes for the user, shown
//! in chat. One tool, `show` ([`tools::ShowTool`]); a content-addressed blob
//! store with a sqlite index ([`store::State`]); Typst PDF compile ([`pdf`])
//! and SVG raster ([`svg`]).
//!
//! Public surface consumed by other plugins:
//! - commands on the [`ARTIFACT_ACTOR`] actor ([`ArtifactActor`]): [`Show`],
//!   [`List`], [`Export`]
//! - events: [`ArtifactShown`]

pub mod kind;
pub mod pdf;
pub mod store;
pub mod svg;
pub mod tools;

use std::path::PathBuf;
use std::sync::Arc;

use kameo::actor::Spawn;
use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use runtime::{Runtime, SessionId};
use serde::{Deserialize, Serialize};

pub use pdf::{Diag, Source};
pub use store::State;
pub use svg::Raster;
pub use tools::ShowArgs;

/// Registry name of the [`ArtifactActor`].
pub const ARTIFACT_ACTOR: &str = "artifact";
/// Largest artifact, any kind (35 §9).
pub const MAX_BYTES: u64 = 32 * 1024 * 1024;

/// `artifact:` config section (35 §10).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ArtifactConfig {
    /// `false`: no `show` tool, no prompt block; blobs and index kept.
    pub enabled: bool,
    /// Extra directories searched for PDF fonts, besides system and embedded fonts.
    pub fonts: Vec<PathBuf>,
}

impl Default for ArtifactConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            fonts: Vec::new(),
        }
    }
}

/// How an artifact renders; derived from the extension at show time (35 §3.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// `md`, `markdown`.
    Markdown,
    /// `txt`, `log`, code.
    Text,
    /// `csv`, `tsv`.
    Csv,
    /// `png`, `jpg`, `jpeg`, `gif`, `webp`.
    Image,
    /// `svg`.
    Svg,
    /// `pdf`.
    Pdf,
    /// `html`, `htm`.
    Html,
    /// Anything else: download only.
    File,
}

impl Kind {
    /// The wire / index spelling (`"markdown"`, …).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::Text => "text",
            Self::Csv => "csv",
            Self::Image => "image",
            Self::Svg => "svg",
            Self::Pdf => "pdf",
            Self::Html => "html",
            Self::File => "file",
        }
    }

    /// Inverse of [`Kind::as_str`].
    pub fn parse(s: &str) -> Option<Self> {
        [
            Self::Markdown,
            Self::Text,
            Self::Csv,
            Self::Image,
            Self::Svg,
            Self::Pdf,
            Self::Html,
            Self::File,
        ]
        .into_iter()
        .find(|k| k.as_str() == s)
    }
}

/// One shown file: an immutable snapshot with a name and a version (35 §3.3).
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Artifact {
    /// `sha256[..16]` of the bytes, hex; also the blob file stem.
    pub id: String,
    /// Owning session.
    pub session: SessionId,
    /// The `show` tool call this record belongs to.
    pub call_id: String,
    /// `report.pdf`.
    pub name: String,
    /// Card title.
    pub title: String,
    /// Rendering kind.
    pub kind: Kind,
    /// Mime type.
    pub mime: String,
    /// Blob size.
    pub bytes: u64,
    /// Per `(session, name)`, 1-based.
    pub version: u32,
    /// `<data_dir>/artifacts/<id>.<ext>`.
    pub path: PathBuf,
    /// `<data_dir>/artifacts/<sha16>.md|.typ` for compiled PDFs.
    pub source: Option<PathBuf>,
    /// `<data_dir>/artifacts/<id>.preview.png` for compiled PDFs and SVGs.
    pub preview: Option<PathBuf>,
    /// Page count of a compiled PDF.
    pub pages: Option<u32>,
    /// The `path` argument when the model showed an existing file.
    pub origin: Option<PathBuf>,
    /// Unix seconds.
    pub created_at: i64,
}

/// Event: a `show` call completed (35 §3.3). Wire shape `{ "type": "ArtifactShown", … }`.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub struct ArtifactShown {
    /// Owning session.
    pub session: SessionId,
    /// The tool call (joins the card).
    pub call_id: String,
    /// The record.
    pub artifact: Artifact,
}

/// `ask` → `Result<Artifact, String>`: snapshot, render, record, publish
/// [`ArtifactShown`]. Runs off the mailbox: a compile takes seconds.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Show {
    /// Owning session.
    pub session: SessionId,
    /// Tool call id.
    pub call_id: String,
    /// Tool arguments.
    pub args: ShowArgs,
}

/// `ask` → `Vec<Artifact>`, newest first; `descendants` widens to the
/// session's subtree (asked from the agent supervisor).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct List {
    /// Root session.
    pub session: SessionId,
    /// Include every session under it.
    pub descendants: bool,
}

/// `ask` → `Result<PathBuf, String>`: copy the blob to `dest` (parent created;
/// refuses to overwrite).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Export {
    /// Artifact id.
    pub id: String,
    /// Destination file.
    pub dest: PathBuf,
}

/// The [`ARTIFACT_ACTOR`] actor.
#[derive(kameo::Actor)]
pub struct ArtifactActor {
    state: Arc<State>,
}

impl Message<Show> for ArtifactActor {
    type Reply = DelegatedReply<Result<Artifact, String>>;

    async fn handle(&mut self, m: Show, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let state = self.state.clone();
        tokio::spawn(async move {
            tx.send(
                state
                    .show(m.session, m.call_id, m.args)
                    .await
                    .map(|s| s.artifact),
            );
        });
        reply
    }
}

impl Message<List> for ArtifactActor {
    type Reply = DelegatedReply<Vec<Artifact>>;

    async fn handle(&mut self, m: List, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let state = self.state.clone();
        // Leaves the mailbox: `descendants` asks the agent supervisor.
        tokio::spawn(async move {
            let list = state
                .list(m.session, m.descendants)
                .await
                .unwrap_or_else(|e| {
                    tracing::error!(session = %m.session, "artifacts unreadable: {e:#}");
                    Vec::new()
                });
            tx.send(list);
        });
        reply
    }
}

impl Message<Export> for ArtifactActor {
    type Reply = Result<PathBuf, String>;

    async fn handle(&mut self, m: Export, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.state.export(&m.id, &m.dest).await
    }
}

impl Message<agent::AgentEvent> for ArtifactActor {
    type Reply = ();

    async fn handle(&mut self, ev: agent::AgentEvent, _: &mut Context<Self, Self::Reply>) {
        if let agent::AgentEvent::SessionDeleted { session } = ev
            && let Err(e) = self.state.delete_session(session).await
        {
            tracing::warn!(%session, "artifact rows not deleted: {e:#}");
        }
    }
}

/// The plugin (35 §0.10): reads `artifact`, opens the store, spawns
/// [`ArtifactActor`] as [`ARTIFACT_ACTOR`], subscribes it to
/// [`agent::AgentEvent`], sweeps orphan blobs, and registers `show` plus the
/// prompt block once the registry is up.
pub struct ArtifactPlugin;

#[async_trait::async_trait]
impl runtime::Plugin for ArtifactPlugin {
    fn name(&self) -> &'static str {
        "artifact"
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        let cfg: ArtifactConfig = rt.section("artifact").unwrap_or_else(|e| {
            tracing::debug!("no [artifact] config section, using defaults: {e}");
            ArtifactConfig::default()
        });
        if !cfg.enabled {
            tracing::info!("artifact disabled by config");
            return Ok(());
        }
        let state = match State::open(&rt.data_dir, rt.clone(), cfg.fonts).await {
            Ok(s) => Arc::new(s),
            Err(e) => {
                tracing::warn!("artifact unavailable: {e:#}");
                return Ok(());
            }
        };
        let actor = ArtifactActor::spawn_with_mailbox(
            ArtifactActor {
                state: state.clone(),
            },
            runtime::subscriber_mailbox(),
        );
        actor.register(ARTIFACT_ACTOR)?;
        rt.subscribe::<ArtifactActor, agent::AgentEvent>(&actor)
            .await;

        let sweep = state.clone();
        tokio::spawn(async move {
            match sweep.sweep().await {
                Ok(0) => {}
                Ok(n) => tracing::info!("artifact sweep removed {n} orphan blobs"),
                Err(e) => tracing::warn!("artifact sweep failed: {e:#}"),
            }
        });

        let rt = rt.clone();
        tokio::spawn(async move {
            if let Some(reg) = rt
                .lookup_when::<::tools::ToolRegistry>(::tools::REGISTRY_ACTOR)
                .await
            {
                let _ = reg
                    .ask(::tools::RegisterTool(Arc::new(tools::ShowTool { state })))
                    .await;
                let _ = reg
                    .ask(::tools::RegisterPrompt {
                        plugin: "artifact",
                        text: tools::PROMPT.to_owned(),
                    })
                    .await;
            }
        });
        Ok(())
    }
}
