//! Document plugin (design doc 59): every BlockNote editor in theuth edits a
//! Yjs document whose authority is this crate — one `yrs::Doc` per open
//! document, persisted under `<data_dir>/documents/` (§4.1), fanned out to
//! its subscribers (§6.2) and re-exported to the markdown file its owner
//! reads (§4.3), with edits to that file imported back by block diff (§4.4)
//! through the headless BlockNote helper `packages/documents` (§3.2).
//!
//! Step 1 (§14): knowledge documents, single user, in-process only.
//!
//! Public surface consumed by other plugins:
//! - commands on the [`DOCUMENT_ACTOR`] actor ([`DocumentActor`]): [`OpenDoc`],
//!   [`ApplyUpdate`], [`Subscribe`], [`AwarenessUpdateMsg`], [`Lookup`],
//!   [`Ensure`], [`ImportMarkdown`], [`ListDocs`], [`Snapshot`], [`Snapshots`],
//!   [`Restore`], [`Delete`] — or the [`Handle`] wrapper `knowledge` uses
//! - event: [`Exported`], published after every derived-file write

use std::path::PathBuf;

use kameo::actor::{ActorRef, Spawn};
use runtime::Runtime;
use serde::{Deserialize, Serialize};

pub mod actor;
pub mod helper;
pub mod meta;
pub mod store;
pub mod tools;
pub mod watch;

pub use actor::{
    ApplyUpdate, AwarenessUpdateMsg, CreateNote, Delete, DocumentActor, Edit, EditOutcome, Ensure,
    ImportMarkdown, ImportOutcome, ListDocs, Lookup, Markdown, OpenDoc, Outline, Restore,
    Snapshot, Snapshots, Subscribe, Tick, UndoLastAgentEdit,
};
pub use helper::{Edit as BlockEdit, EditError, ImportMode, helper_path, protocol_version};
pub use meta::*;

/// Registry name of the [`DocumentActor`].
pub const DOCUMENT_ACTOR: &str = "document";
/// Name of the `Y.XmlFragment` the blocks live in (`packages/documents/src/schema.ts` `FRAGMENT`).
pub const FRAGMENT: &str = "document-store";

/// `documents:` config section (59 §12; the keys step 1–2 read).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct DocumentsConfig {
    /// `false`: no store, no helper, no `doc_*` methods; the knowledge editor falls back to raw text.
    pub enabled: bool,
    /// Debounce between the last accepted update and the derived-file write (§4.3; hard cap 3 s).
    pub export_debounce_ms: u64,
    /// Refuse updates that would push a document's Yjs state past this (§4.7).
    pub max_doc_bytes: usize,
    /// Automatic snapshots kept per document (§4.6); labelled ones are never pruned.
    pub snapshots_keep: usize,
    /// CSS colour of the agent's presence and reveal in every editor (§6.4, §12).
    pub agent_color: String,
}

impl Default for DocumentsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            export_debounce_ms: 400,
            max_doc_bytes: 8 * 1024 * 1024,
            snapshots_keep: 30,
            agent_color: "#7c3aed".to_owned(),
        }
    }
}

/// Event: a document's derived file was rewritten (§4.3). The file's owner
/// (knowledge: `sync` → index + commit) reacts; the text is on disk.
#[derive(Clone, Debug)]
pub struct Exported {
    /// Document.
    pub doc: DocId,
    /// What it is bound to.
    pub binding: Binding,
    /// Absolute path written.
    pub path: PathBuf,
}

/// A thin, cloneable wrapper other plugins call instead of naming messages
/// (§3.3 `Handle`); resolved lazily so start order never matters.
#[derive(Clone)]
pub struct Handle(ActorRef<DocumentActor>);

impl Handle {
    /// The running actor, or `None` when the plugin is disabled or not started yet.
    pub fn lookup() -> Option<Self> {
        ActorRef::<DocumentActor>::lookup(DOCUMENT_ACTOR)
            .ok()
            .flatten()
            .map(Self)
    }

    /// The actor itself.
    pub fn actor(&self) -> &ActorRef<DocumentActor> {
        &self.0
    }

    /// The document of concept `id`, if one exists.
    pub async fn resolve_knowledge(&self, id: &str) -> Option<DocId> {
        self.0
            .ask(Lookup {
                binding: Binding::Knowledge(id.to_owned()),
            })
            .await
            .ok()
            .flatten()
    }

    /// The document of concept `id`, created from `markdown` when there is none (§7.1 on demand).
    pub async fn ensure_knowledge(&self, id: &str, markdown: String) -> Result<DocId, String> {
        self.0
            .ask(Ensure {
                binding: Binding::Knowledge(id.to_owned()),
                markdown,
            })
            .await
            .map_err(|e| e.to_string())
    }

    /// Import the derived file's text as one diff transaction (§4.4); our own export is a no-op.
    pub async fn import_markdown(
        &self,
        doc: DocId,
        markdown: String,
        by: Principal,
    ) -> Result<ImportOutcome, String> {
        self.0
            .ask(ImportMarkdown {
                doc,
                markdown,
                by,
                mode: ImportMode::Diff,
            })
            .await
            .map_err(|e| e.to_string())
    }

    /// Move a document to `.trash` (§7.1).
    pub async fn delete(&self, doc: DocId) -> Result<(), String> {
        self.0.ask(Delete { doc }).await.map_err(|e| e.to_string())
    }
}

/// The plugin: open the store, start the helper client, spawn and register
/// the actor, start the tick and the derived-file watcher (§3.1).
pub struct DocumentPlugin;

#[async_trait::async_trait]
impl runtime::Plugin for DocumentPlugin {
    fn name(&self) -> &'static str {
        "document"
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        let cfg: DocumentsConfig = rt.section("documents").unwrap_or_else(|e| {
            tracing::debug!("no [documents] config section, using defaults: {e}");
            DocumentsConfig::default()
        });
        if !cfg.enabled {
            tracing::info!("documents disabled by config");
            return Ok(());
        }
        let store = match store::Store::open(&rt.data_dir).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("documents store unavailable: {e:#}");
                return Ok(());
            }
        };
        let path = helper_path();
        if !path.is_file() {
            tracing::warn!(path = %path.display(), "documents helper not found; the editor will report it on first use");
        }
        let (events_tx, mut events_rx) = tokio::sync::mpsc::unbounded_channel();
        let helper = helper::Helper::new(path, events_tx, rt.shutdown.clone());
        let actor = DocumentActor::spawn(DocumentActor::new(store, helper, cfg, rt.clone()));
        actor.register(DOCUMENT_ACTOR)?;

        let a = actor.clone();
        tokio::spawn(async move {
            while let Some(ev) = events_rx.recv().await {
                if a.tell(ev).await.is_err() {
                    break;
                }
            }
        });
        let a = actor.clone();
        let shutdown = rt.shutdown.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_millis(250));
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    _ = tick.tick() => {
                        if a.tell(Tick).await.is_err() { break; }
                    }
                }
            }
        });
        // §5: the tools and their prompt block, once the registry is up (12 §9).
        let a = actor.clone();
        let rt2 = rt.clone();
        tokio::spawn(async move {
            if let Some(reg) = rt2
                .lookup_when::<::tools::ToolRegistry>(::tools::REGISTRY_ACTOR)
                .await
            {
                tools::register(&reg, a, rt2.data_dir.clone()).await;
            }
        });
        if let Err(e) = watch::start(&rt.data_dir, actor) {
            tracing::warn!("documents file watcher unavailable: {e:#}");
        }
        Ok(())
    }
}