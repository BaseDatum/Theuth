//! Document plugin (design doc 59): every BlockNote editor in theuth edits a
//! Yjs document whose authority is this crate — one `yrs::Doc` per open
//! document, persisted under `<data_dir>/documents/` (§4.1), fanned out to
//! its subscribers (§6.2) and re-exported to the markdown file its owner
//! reads (§4.3), with edits to that file imported back by block diff (§4.4)
//! through the headless BlockNote helper `packages/documents` (§3.2).
//!
//! Steps 1–3 and 5 (§14): knowledge documents, notes, markdown artifacts and meeting notes, single user, in-process only.
//!
//! Public surface consumed by other plugins:
//! - commands on the [`DOCUMENT_ACTOR`] actor ([`DocumentActor`]): [`OpenDoc`],
//!   [`ApplyUpdate`], [`Subscribe`], [`AwarenessUpdateMsg`], [`Lookup`],
//!   [`Ensure`], [`ImportMarkdown`], [`ListDocs`], [`Snapshot`], [`Snapshots`],
//!   [`Restore`], [`Delete`], [`CreateNote`], [`Rename`], [`DeleteNote`] — or the [`Handle`] wrapper `knowledge` uses
//! - events: [`Exported`], published after every derived-file write; [`DocumentChanged`] for lists

use std::path::PathBuf;

use kameo::actor::{ActorRef, Spawn};
use runtime::Runtime;
use serde::{Deserialize, Serialize};

/// Documents RPC methods and typed clients.
pub mod rpc_documents;
/// Notebooks RPC methods and typed clients.
pub mod rpc_notebooks;
/// Whiteboards RPC methods and typed clients (218 §4.1).
pub mod rpc_whiteboards;

/// Assemble this plugin's RPC methods.
pub fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule {
    rpc_core::merge_described([
        rpc_documents::rpc_module(server.clone()),
        rpc_notebooks::rpc_module(server.clone()),
        rpc_whiteboards::rpc_module(server),
    ])
    .expect("document RPC composition")
}

pub mod actor;
pub mod cells;
pub mod cite_check;
pub mod helper;
pub mod hub;
pub mod index;
pub mod ingest;
pub mod memory;
pub mod meta;
pub mod mirror;
pub mod nb_tools;
pub mod note_cells;
pub mod notebook;
pub mod relay;
pub mod rpc_support;
pub mod search;
pub mod share;
pub mod store;
pub mod tools;
pub mod watch;
pub mod wb;

pub use actor::{
    ApplyUpdate, AwarenessUpdateMsg, Bind, Create, CreateBoard, CreateNote, Delete, DeleteNote,
    DocHeadings, DocumentActor, Edit, EditOutcome, Ensure, Flush, Headings, Homing, ImportMarkdown,
    ImportOutcome, ListDocs, Lookup, Markdown, MetaOf, OpenDoc, Outline, Rename, Restore,
    SetDerived, SetProject, Snapshot, Snapshots, Stats, Subscribe, Tick, UndoLastAgentEdit,
    board_frontmatter, with_heading,
};
pub use cells::{
    CellAnnotation, CellLatest, CodeBlockText, CodeBlockTextReply, NoteKey, RunAnnotations,
    run_annotations, set_run_annotations, watch_latest,
};
pub use helper::{
    Edit as BlockEdit, EditError, HelperProcess, HelperStatus, ImportMode, helper_status,
    protocol_version,
};
pub use hub::{
    BLOCK_WAIT_MS, DocError, DocServer, HANDBACK_TIMEOUT_MS, HUB_MOVED, Handback, Handover,
    JOIN_PARK_MS, MoveHome, NOT_HOME, NOT_OWNER, PENDING_HOME_TTL, Relayed, Route, SERVER_MOVED,
};
pub use meta::*;
pub use note_cells::{CheckCells, CodeBlocks, reader_markdown};
pub use notebook::*;
pub use relay::{CreateReq, HandbackReply};
pub use share::{
    Join, JoinError, LinkCreate, LinkRevoke, MemberRevoke, MemberSet, MembersOf, SetName,
    SharedDoc, SharedDocs, proof,
    secret_hash,
};
pub use store::DocStats;

/// Registry name of the [`DocumentActor`].
pub const DOCUMENT_ACTOR: &str = "document";
/// Name of the `Y.XmlFragment` the blocks live in (`packages/documents/src/schema.ts` `FRAGMENT`).
pub const FRAGMENT: &str = "document-store";

/// `documents:` config section (59 §12 plus the two §4.7 memory knobs).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct DocumentsConfig {
    /// `false`: no store, no helper, no `doc_*` methods; the knowledge editor falls back to raw text.
    pub enabled: bool,
    /// Debounce between the last accepted update and the derived-file write (§4.3; hard cap 3 s).
    pub export_debounce_ms: u64,
    /// Quiet period after the last accepted update before the owner hears [`Exported`] (§4.3 as built): typing
    /// rewrites the file every few seconds, the owner (knowledge commit, artifact version) is told once it stops.
    /// Flushed at once when the document unloads, its last subscriber leaves, or the harness shuts down.
    pub export_quiet_secs: u64,
    /// Refuse updates that would push a document's Yjs state past this (§4.7).
    pub max_doc_bytes: usize,
    /// Automatic snapshots kept per document (§4.6); labelled ones are never pruned.
    pub snapshots_keep: usize,
    /// CSS colour of the agent's presence and reveal in every editor (§6.4, §12).
    pub agent_color: String,
    /// Members per document (§8.3, §12); the 65th join is refused.
    pub max_members: usize,
    /// `on` (default) | `off` (`doc_link_create` refuses; members still connect) | `disabled` (members refused too) (§12).
    pub sharing: String,
    /// Unload a document with no subscriber after this long idle (§4.7); the log is compacted into `snapshot.yjs`.
    pub idle_unload_secs: u64,
    /// Documents kept in memory at most (§4.7); past it the longest idle without subscribers is unloaded.
    pub max_open_docs: usize,
    /// 69: snapshot an open document this long after its last update (besides the 1 MiB / 1000-record /
    /// unload triggers), so the replicated mirror lags by seconds, not by a megabyte.
    pub snapshot_quiet_ms: u64,
}

impl Default for DocumentsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            export_debounce_ms: 400,
            export_quiet_secs: 60,
            max_doc_bytes: 8 * 1024 * 1024,
            snapshots_keep: 30,
            agent_color: "#7c3aed".to_owned(),
            max_members: 64,
            sharing: "on".to_owned(),
            idle_unload_secs: 600,
            max_open_docs: 256,
            snapshot_quiet_ms: 2000,
        }
    }
}

/// Event: a document's derived file was rewritten and its author went quiet (§4.3 as built:
/// `export_quiet_secs` after the last update, or at once on unload / last subscriber gone /
/// shutdown). The file's owner (knowledge: `sync` → index + commit; artifacts: a version) reacts.
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
    /// A local, unregistered document actor selected by the knowledge partition.
    pub fn from_actor(actor: ActorRef<DocumentActor>) -> Self {
        Self(actor)
    }

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

/// Declare wire contracts before starting producers, including disabled services.
pub async fn declare_wire(rt: &Runtime) -> anyhow::Result<()> {
    use wire::RuntimeWire as _;
    rt.register_wire_event::<DocumentChanged>().await?;
    rt.register_wire_event::<CellLatestChanged>().await?;
    rt.register_wire_event::<DocumentSharing>().await?;
    rt.register_wire_event::<SourceEvent>().await
}

/// The plugin: open the store, start the helper client, spawn and register
/// the actor, start the tick and the derived-file watcher (§3.1).
pub struct DocumentPlugin;

#[async_trait::async_trait]
impl runtime::Plugin for DocumentPlugin {
    fn name(&self) -> &'static str {
        "document"
    }

    async fn declare_wire(&mut self, rt: &runtime::Runtime) -> anyhow::Result<()> {
        declare_wire(rt).await
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        declare_wire(rt).await?;
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
        // SEAL: share-link hashes are sealed with the mesh key; secrets starts before this plugin.
        let store = match secrets::handle(rt).await {
            Some(vault) => store.with_vault(vault),
            None => {
                tracing::warn!(
                    "documents: secrets vault unavailable; share links cannot be created or verified"
                );
                store
            }
        };
        // 69: the index row replicates; the replica plugin starts after this one. Local only on failure.
        let (rt2, pool) = (rt.clone(), store.pool().clone());
        tokio::spawn(async move {
            if rt2
                .lookup_when::<replica::Replica>(replica::REPLICA_ACTOR)
                .await
                .is_none()
            {
                return;
            }
            match replica::attach(&pool, store::TABLES).await {
                Ok(h) => {
                    if let Err(e) = h.announce_unstamped().await {
                        tracing::warn!("documents: existing rows not announced: {e:#}");
                    }
                    // 184 §6.4 (W-p): foreign `cell_latest` batches → `CellLatestChanged`.
                    cells::watch_latest(h, rt2);
                    tracing::info!("documents: docs replicated");
                }
                Err(e) => tracing::info!("documents: replication unavailable ({e}); local only"),
            }
        });
        let (events_tx, mut events_rx) = tokio::sync::mpsc::unbounded_channel();
        let helper = helper::Helper::new(events_tx, rt.shutdown.clone());
        let actor =
            DocumentActor::spawn(DocumentActor::new(store.clone(), helper, cfg, rt.clone()));
        actor.register(DOCUMENT_ACTOR)?;
        // 144 §4.2: the ingest job runner beside the document actor (hub-only by its writes).
        if let Err(e) = ingest::IngestActor::start(rt, store.clone(), actor.clone()) {
            tracing::warn!("documents: ingest unavailable: {e:#}");
        }
        // 144 §7.1: the block index — first run / model change in the background. The embedder registers in
        // the decision plugin, which starts after this one: wait for it (bounded) so the first walk embeds
        // instead of leaving every document lexical-only until its next edit.
        let (a, rt2) = (actor.clone(), rt.clone());
        tokio::spawn(async move {
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                rt2.lookup_when::<decision::embed::EmbedActor>(decision::embed::EMBED_ACTOR),
            )
            .await;
            if let Err(e) = index::reindex(&a).await {
                tracing::warn!("index: rebuild failed: {e:#}");
            }
        });

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
                    // Shutdown: settled edits reach their owners before `main` exits (§4.3 as built).
                    _ = shutdown.cancelled() => { let _ = a.ask(Flush).await; break }
                    _ = tick.tick() => {
                        if a.tell(Tick).await.is_err() { break; }
                    }
                }
            }
        });
        // 183 §6.9.11: after each vault-key rotation the hub re-seals the share links.
        if let Some(vault) = store.vault() {
            let (a, mut rotation) = (actor.clone(), vault.watch_rotation());
            tokio::spawn(async move {
                let mut done: Option<String> = None;
                while rotation.changed().await.is_ok() {
                    let key = rotation
                        .borrow_and_update()
                        .as_ref()
                        .map(|r| r.key.id.clone());
                    if key.is_none() || key == done {
                        continue;
                    }
                    match a.ask(share::Reseal).await {
                        Ok(true) => done = key,
                        Ok(false) => {}
                        Err(_) => break,
                    }
                }
            });
        }
        // §5: the tools and their prompt block, once the registry is up (12 §9).
        let a = actor.clone();
        let rt2 = rt.clone();
        tokio::spawn(async move {
            if let Some(reg) = rt2
                .lookup_when::<::tools::ToolRegistry>(::tools::REGISTRY_ACTOR)
                .await
            {
                tools::register(&reg, a.clone(), rt2.data_dir.clone()).await;
                // 144 §13.2: the wave-1 notebook tools beside the `doc_*` ones.
                nb_tools::register(&reg, a.clone()).await;
                // 218 §3.1: the whiteboard tools.
                wb::tools::register(&reg, a).await;
            }
        });
        // 69: the derived-file watcher, exports and every write are hub duties. `Reconcile` starts the
        // watcher now (a lone engine is its own hub); the lease and the self id flip it from here on.
        let _ = actor.tell(hub::Reconcile).await;
        rt.subscribe::<DocumentActor, mesh::MeshEvent>(&actor).await;
        // 184 §8.1: a `homes_on` flip, published by the mesh's gate pass.
        rt.subscribe::<DocumentActor, mesh::DocsGateState>(&actor)
            .await;
        // 144 §7.1: every `DocumentChanged` this actor publishes re-indexes the document.
        rt.subscribe::<DocumentActor, DocumentChanged>(&actor).await;
        let (a, rt2) = (actor.clone(), rt.clone());
        tokio::spawn(async move {
            let Some(m) = rt2.lookup_when::<mesh::Mesh>(mesh::MESH_ACTOR).await else {
                return;
            };
            if let Ok(st) = m.ask(mesh::Status).await {
                let _ = a.tell(hub::SelfId(st.self_id)).await;
            }
        });
        // 69 Y: the hub side of `theuth/docs/1` — forwarded client sessions land on the actor.
        tokio::spawn(relay::serve(rt.clone(), actor.clone()));
        // 69: the flat mirror replicates as `doc_files` rows (disclosed replica one-liner); projections merge back.
        tokio::spawn(replica::files::mirror_at(
            store.root().join("mirror.sqlite"),
            store.mirror_dir(),
            "doc_files",
            std::time::Duration::from_secs(60),
            None,
        ));
        if let Err(e) = mirror::start(&store.mirror_dir(), actor) {
            tracing::warn!("documents mirror watcher unavailable: {e:#}");
        }
        Ok(())
    }
}
