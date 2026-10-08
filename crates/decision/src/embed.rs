//! The `embed` actor (144 §7.2): retrieval embeddings from the local checkpoint directory
//! `<data_dir>/models/<notebooks.embedding_model>/`, lazily loaded on the first [`Embed`] and
//! unloaded after [`IDLE_UNLOAD`] idle like [`crate::Engine`]. A missing directory is not an
//! error here: [`Info`] answers `None`, [`Embed`] answers `Err`, and the index runs FTS5-only.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use serde::{Deserialize, Serialize};
use tokenizers::Tokenizer;

use crate::IDLE_UNLOAD;
use crate::candle::Gpu;
use crate::candle::embed::{self, Embedder};

/// Registry name of the [`EmbedActor`].
pub const EMBED_ACTOR: &str = "embed";

/// Default of `notebooks.embedding_model` (§7.2): `Alibaba-NLP/gte-modernbert-base`, 768 dims,
/// `[CLS]` pooling, placed by the user or the model downloader.
pub const DEFAULT_MODEL: &str = "embed-gte-modernbert-base";

/// The `notebooks.embedding_model` key, as this crate reads it (the document crate reads the
/// whole section; unknown keys are ignored on both sides).
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct EmbedSection {
    /// A checkpoint directory name under `<data_dir>/models/` — never `provider/model` (D9).
    pub embedding_model: String,
}

impl Default for EmbedSection {
    fn default() -> Self {
        Self {
            embedding_model: DEFAULT_MODEL.to_owned(),
        }
    }
}

/// The D9 rule: a checkpoint directory name under `<data_dir>/models/` — never `provider/model`.
pub fn check_name(name: &str) -> anyhow::Result<()> {
    if name.is_empty() || name.contains(['/', '\\']) || name == "." || name == ".." {
        anyhow::bail!(
            "notebooks.embedding_model = {name:?}: must be a checkpoint directory name under \
             <data_dir>/models/ (e.g. {DEFAULT_MODEL}), never provider/model"
        );
    }
    Ok(())
}

/// `<data_dir>/models/<name>` after [`check_name`].
pub fn model_dir(data_dir: &Path, name: &str) -> anyhow::Result<PathBuf> {
    check_name(name)?;
    Ok(data_dir.join("models").join(name))
}

/// Whether `dir` holds the three checkpoint files.
pub fn installed(dir: &Path) -> bool {
    embed::FILES.iter().all(|f| dir.join(f).is_file())
}

/// What the index records per document (`nb_index_state.model/dims`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbedInfo {
    /// The checkpoint directory name.
    pub model: String,
    /// Vector length (`hidden_size`).
    pub dims: usize,
}

/// `ask` → `Result<Vec<Vec<f32>>, String>`: one unit vector per text, in order; loads the model
/// on first use. `Err` names the missing directory or the load failure.
#[derive(Clone, Debug)]
pub struct Embed {
    /// Texts to embed.
    pub texts: Vec<String>,
}

/// `ask` → `Result<Vec<usize>, String>`: tokens per text as the model counts them (specials
/// included); needs only `tokenizer.json`, never loads the weights.
#[derive(Clone, Debug)]
pub struct Tokens {
    /// Texts to measure.
    pub texts: Vec<String>,
}

/// `ask` → `Option<EmbedInfo>`: the model name and dims from `config.json`, `None` when the
/// directory is not installed. Never loads the weights.
#[derive(Clone, Copy, Debug)]
pub struct Info;

#[derive(Clone, Copy, Debug)]
struct Unload(u64);

/// The actor.
#[derive(kameo::Actor)]
pub struct EmbedActor {
    dir: PathBuf,
    gpu: Gpu,
    idle: Duration,
    loaded: Option<Arc<Embedder>>,
    tokenizer: Option<Arc<Tokenizer>>,
    last_used: Instant,
    watchdog: Option<u64>,
    epochs: u64,
    warned: bool,
}

impl EmbedActor {
    /// Over `dir`, on `gpu`, unloading after [`IDLE_UNLOAD`] idle.
    pub fn new(dir: PathBuf, gpu: Gpu) -> Self {
        Self {
            dir,
            gpu,
            idle: IDLE_UNLOAD,
            loaded: None,
            tokenizer: None,
            last_used: Instant::now(),
            watchdog: None,
            epochs: 0,
            warned: false,
        }
    }

    /// A different idle (tests).
    pub fn with_idle(mut self, idle: Duration) -> Self {
        self.idle = idle;
        self
    }

    fn name(&self) -> String {
        self.dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// `Err` once per missing directory, at WARN; the callers fall back to lexical search.
    fn check_installed(&mut self) -> Result<(), String> {
        if installed(&self.dir) {
            return Ok(());
        }
        let msg = format!(
            "embedding model not installed: {} lacks {}; nb_search runs FTS5-only",
            self.dir.display(),
            embed::FILES.join(", ")
        );
        if !self.warned {
            self.warned = true;
            tracing::warn!("embed: {msg}");
        }
        Err(msg)
    }

    async fn session(&mut self) -> Result<Arc<Embedder>, String> {
        if let Some(e) = &self.loaded {
            return Ok(e.clone());
        }
        self.check_installed()?;
        let (dir, gpu) = (self.dir.clone(), self.gpu);
        let t = Instant::now();
        let e = tokio::task::spawn_blocking(move || Embedder::open(&dir, gpu))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("embed: load {}: {e:#}", self.dir.display()))?;
        tracing::info!(
            "embed: {} loaded ({} dims) in {:?}",
            self.name(),
            e.dims,
            t.elapsed()
        );
        let e = Arc::new(e);
        self.loaded = Some(e.clone());
        Ok(e)
    }

    fn tokenizer(&mut self) -> Result<Arc<Tokenizer>, String> {
        if let Some(t) = &self.tokenizer {
            return Ok(t.clone());
        }
        self.check_installed()?;
        let t = Arc::new(embed::tokenizer(&self.dir).map_err(|e| format!("embed: {e:#}"))?);
        self.tokenizer = Some(t.clone());
        Ok(t)
    }

    fn touch(&mut self, me: ActorRef<Self>) {
        self.last_used = Instant::now();
        if self.watchdog.is_some() || self.idle.is_zero() {
            return;
        }
        self.epochs += 1;
        self.watchdog = Some(self.epochs);
        self.arm(me, self.idle, self.epochs);
    }

    fn arm(&self, me: ActorRef<Self>, after: Duration, epoch: u64) {
        tokio::spawn(async move {
            tokio::time::sleep(after).await;
            let _ = me.tell(Unload(epoch)).await;
        });
    }
}

impl Message<Info> for EmbedActor {
    type Reply = Option<EmbedInfo>;

    async fn handle(&mut self, _: Info, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.check_installed().ok()?;
        match embed::config(&self.dir) {
            Ok(cfg) => Some(EmbedInfo {
                model: self.name(),
                dims: cfg.hidden_size,
            }),
            Err(e) => {
                tracing::warn!("embed: {}: {e:#}", self.dir.display());
                None
            }
        }
    }
}

impl Message<Tokens> for EmbedActor {
    type Reply = Result<Vec<usize>, String>;

    async fn handle(&mut self, m: Tokens, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let tok = self.tokenizer()?;
        tokio::task::spawn_blocking(move || {
            m.texts
                .iter()
                .map(|t| embed::count_tokens(&tok, t).map_err(|e| e.to_string()))
                .collect()
        })
        .await
        .map_err(|e| e.to_string())?
    }
}

impl Message<Embed> for EmbedActor {
    type Reply = Result<Vec<Vec<f32>>, String>;

    // ponytail: the forward runs under the mailbox, so a query embedding waits for the batch in
    // flight (the indexer sends ≤ 32 texts per message); a second session slot if that is slow.
    async fn handle(&mut self, m: Embed, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if m.texts.is_empty() {
            return Ok(Vec::new());
        }
        let e = self.session().await?;
        self.touch(ctx.actor_ref().clone());
        tokio::task::spawn_blocking(move || e.embed(&m.texts).map_err(|e| format!("embed: {e:#}")))
            .await
            .map_err(|e| e.to_string())?
    }
}

impl Message<Unload> for EmbedActor {
    type Reply = ();

    async fn handle(&mut self, Unload(epoch): Unload, ctx: &mut Context<Self, Self::Reply>) {
        if self.watchdog != Some(epoch) {
            return;
        }
        let since = self.last_used.elapsed();
        if since >= self.idle {
            self.watchdog = None;
            if self.loaded.take().is_some() {
                tracing::info!("embed: {} released (idle {since:?})", self.name());
            }
        } else {
            self.arm(ctx.actor_ref().clone(), self.idle - since, epoch);
        }
    }
}

/// Whether the model is resident (tests).
#[derive(Clone, Copy, Debug)]
pub struct IsLoaded;

impl Message<IsLoaded> for EmbedActor {
    type Reply = bool;

    async fn handle(&mut self, _: IsLoaded, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.loaded.is_some()
    }
}
