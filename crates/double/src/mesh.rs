//! Training bundles over the mesh (126 §8.4): `theuth/double/1`. Modelled on
//! `artifact::mesh` (crates/artifact/src/mesh.rs): `LengthDelimitedCodec`
//! frames of postcard on an iroh QUIC stream opened with
//! `Transport::open(peer, ALPN_DOUBLE)` and accepted with
//! `transport.accept(ALPN_DOUBLE)`. The constant lives in `replica` with every
//! other ALPN ([`replica::ALPN_DOUBLE`]) and is registered in the endpoint's
//! stream list (crates/mesh/src/net.rs `STREAM_ALPNS`).
//!
//! Wire, one run per stream:
//!
//! ```text
//! laptop → node   Train { run_id, manifest, sha }   Chunk… End            Ack
//! node → laptop   Accepted | Refused(why)   Progress…   Chunk… End | Error(why)   Cleaned
//! ```
//!
//! The bundle ([`Bundle`]) is the postcard encoding of the scrubbed rows
//! (`examples` + `labels`, golden flagged), the f16 embeddings, the Tier B
//! config and the base checkpoint hash — never weights; the node fetches the
//! checkpoint itself. The node runs its [`Runner`] as a job and streams back
//! the [`TrainResult`] (adapter, heads, manifest, metrics). The laptop
//! re-scores golden locally before anything is activated (the scheduler's
//! `evaluating` step); a node cannot activate a version. After `Ack` the node
//! deletes bundle and checkpoints and says `Cleaned`.
//!
//! W2 ships the protocol and [`NotYet`], the node runner that refuses every
//! bundle until W3 wires the Tier B trainer; `train_on` is what W3's remote
//! Tier B [`crate::sched::Trainer`] calls.

use std::sync::Arc;

use bytes::Bytes;
use futures::{SinkExt as _, StreamExt as _};
use replica::{ByteStream, PeerId, Transport};
use runtime::Runtime;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use sqlx::Row as _;
use tokio::sync::mpsc;
use tokio_util::codec::{Framed, LengthDelimitedCodec};

pub use replica::ALPN_DOUBLE;

use crate::config::TierB;
use crate::store::Store;
use crate::types::{RunState, Tier};

/// Largest frame either side accepts.
pub const MAX_FRAME: usize = 64 * 1024 * 1024;
/// Bundle / result bytes per `Chunk` frame.
const CHUNK: usize = 4 * 1024 * 1024;
/// Longest silence between frames the laptop tolerates while the node trains.
pub const IDLE: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// One scrubbed example with its labels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BundleRow {
    /// `examples.id`.
    pub id: String,
    /// The §3 key.
    pub class: String,
    /// `root` | `subagent` | `desk:<agent>` | `routine:<name>`.
    pub requester: String,
    /// Scrubbed card text.
    pub card_text: String,
    /// Option labels in card order.
    pub options: Vec<String>,
    /// `(source, choice, weight)` as `labels` has them.
    pub labels: Vec<(String, String, f32)>,
    /// RFC 3339.
    pub asked_at: String,
    /// Never trained on; the node excludes it, the laptop re-scores it.
    pub golden: bool,
}

/// What the node needs to know before accepting the bytes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BundleManifest {
    /// The laptop's `runs.id`.
    pub run_id: String,
    /// `b` or `c`.
    pub tier: Tier,
    /// The checkpoint the node must fetch (sha256 of the safetensors).
    pub base_hash: String,
    /// Rank, batch, epochs, lr.
    pub config: TierB,
    /// Rows in the bundle.
    pub rows: u32,
    /// Embedding width.
    pub dim: u32,
    /// RFC 3339.
    pub created_at: String,
}

/// The bundle: rows, embeddings (f16 LE, one per row), manifest.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bundle {
    /// Header.
    pub manifest: BundleManifest,
    /// The rows.
    pub rows: Vec<BundleRow>,
    /// `encode_f16` of each row's embedding, same order; empty when absent.
    pub embeddings: Vec<Vec<u8>>,
}

/// What comes back.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrainResult {
    /// Echoed.
    pub run_id: String,
    /// `adapter.safetensors` bytes (Tier B/C).
    pub adapter: Option<Vec<u8>>,
    /// `heads.json` bytes when the node refit the heads too.
    pub heads: Option<Vec<u8>>,
    /// The node's `manifest.json` as JSON text (seed, base hash, steps…);
    /// text because postcard cannot carry `serde_json::Value`.
    pub manifest_json: String,
    /// The node's own metrics as JSON text (informational; the laptop re-scores).
    pub metrics_json: String,
}

/// Every frame, both directions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Frame {
    /// laptop → node: here comes a bundle of `sha` (sha256 hex of the bytes).
    Train {
        /// The run.
        run_id: String,
        /// The header.
        manifest: BundleManifest,
        /// sha256 of the encoded bundle.
        sha: String,
    },
    /// Bytes of the bundle (→ node) or of the result (→ laptop).
    Chunk(Vec<u8>),
    /// Last chunk sent.
    End,
    /// laptop → node: result received; delete everything.
    Ack,
    /// node → laptop: bundle accepted, send the bytes.
    Accepted,
    /// node → laptop: not here (`why`).
    Refused(String),
    /// node → laptop: training progress.
    Progress {
        /// 0..=1.
        progress: f32,
        /// `epoch 2/3`.
        message: String,
    },
    /// node → laptop: the run failed.
    Error(String),
    /// node → laptop: bundle and checkpoints deleted.
    Cleaned,
}

/// The node side: what runs a bundle. W3's Tier B trainer implements it;
/// [`NotYet`] refuses.
#[async_trait::async_trait]
pub trait Runner: Send + Sync {
    /// Take the bundle, or say why not (busy, no accelerator, tier unavailable).
    async fn accept(&self, manifest: &BundleManifest) -> Result<(), String>;
    /// Train; `progress` frames go back live.
    async fn run(
        &self,
        bundle: Bundle,
        progress: mpsc::Sender<(f32, String)>,
    ) -> anyhow::Result<TrainResult>;
    /// Delete what the run left (bundle, checkpoints).
    async fn cleanup(&self, run_id: &str);
}

/// The W2 node runner: every bundle is refused until W3 wires Tier B.
pub struct NotYet;

#[async_trait::async_trait]
impl Runner for NotYet {
    async fn accept(&self, m: &BundleManifest) -> Result<(), String> {
        Err(format!(
            "tier {} trainer is not available on this node (126 W3)",
            m.tier.as_str()
        ))
    }
    async fn run(&self, _: Bundle, _: mpsc::Sender<(f32, String)>) -> anyhow::Result<TrainResult> {
        anyhow::bail!("not available")
    }
    async fn cleanup(&self, _: &str) {}
}

type Stream = Framed<Box<dyn ByteStream>, LengthDelimitedCodec>;

fn framed(stream: Box<dyn ByteStream>) -> Stream {
    Framed::new(
        stream,
        LengthDelimitedCodec::builder()
            .max_frame_length(MAX_FRAME)
            .new_codec(),
    )
}

fn encode<T: Serialize>(v: &T) -> anyhow::Result<Bytes> {
    Ok(Bytes::from(postcard::to_allocvec(v)?))
}

fn sha256_hex(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

async fn send(framed: &mut Stream, f: &Frame) -> anyhow::Result<()> {
    framed.send(encode(f)?).await?;
    Ok(())
}

async fn recv(framed: &mut Stream) -> anyhow::Result<Frame> {
    let bytes = framed
        .next()
        .await
        .ok_or_else(|| anyhow::anyhow!("peer closed the stream"))??;
    Ok(postcard::from_bytes(&bytes)?)
}

async fn send_bytes(framed: &mut Stream, bytes: &[u8]) -> anyhow::Result<()> {
    for c in bytes.chunks(CHUNK) {
        send(framed, &Frame::Chunk(c.to_vec())).await?;
    }
    send(framed, &Frame::End).await
}

// --- laptop side ---------------------------------------------------------

/// The bundle for `run_id` from the store: every example with an embedding
/// and at least one label (already scrubbed at write), golden flagged.
pub async fn build_bundle(
    store: &Store,
    run_id: &str,
    tier: Tier,
    base_hash: &str,
    config: &TierB,
) -> anyhow::Result<Bundle> {
    let golden: std::collections::HashSet<String> = store.golden().await?.into_iter().collect();
    let rows = sqlx::query(
        "SELECT id, class, requester, card_text, options, embedding, asked_at
         FROM examples WHERE embedding IS NOT NULL ORDER BY asked_at, id",
    )
    .fetch_all(store.pool())
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    let mut embeddings = Vec::with_capacity(rows.len());
    let mut dim = 0u32;
    for r in rows {
        let id: String = r.get("id");
        let labels: Vec<(String, String, f32)> = store
            .labels(&id)
            .await?
            .into_iter()
            .map(|l| (l.source.as_str().to_owned(), l.choice, l.weight))
            .collect();
        if labels.is_empty() {
            continue;
        }
        let emb: Vec<u8> = r.get("embedding");
        dim = dim.max((emb.len() / 2) as u32);
        embeddings.push(emb);
        out.push(BundleRow {
            golden: golden.contains(&id),
            id,
            class: r.get("class"),
            requester: r.get("requester"),
            card_text: r.get("card_text"),
            options: serde_json::from_str(r.get::<String, _>("options").as_str())
                .unwrap_or_default(),
            labels,
            asked_at: r.get("asked_at"),
        });
    }
    Ok(Bundle {
        manifest: BundleManifest {
            run_id: run_id.to_owned(),
            tier,
            base_hash: base_hash.to_owned(),
            config: config.clone(),
            rows: out.len() as u32,
            dim,
            created_at: chrono::Utc::now().to_rfc3339(),
        },
        rows: out,
        embeddings,
    })
}

/// Ship `bundle` to `peer`, wait for the result, acknowledge, wait for
/// `Cleaned`. `on_event` sees `Running` with the node's progress and
/// `Cleaned` at the end; the caller re-scores before activating anything.
pub async fn train_on(
    t: &dyn Transport,
    peer: &PeerId,
    bundle: &Bundle,
    mut on_event: impl FnMut(RunState, Option<f32>, String) + Send,
) -> anyhow::Result<TrainResult> {
    let bytes = postcard::to_allocvec(bundle)?;
    let mut framed = framed(t.open(peer, ALPN_DOUBLE).await?);
    send(
        &mut framed,
        &Frame::Train {
            run_id: bundle.manifest.run_id.clone(),
            manifest: bundle.manifest.clone(),
            sha: sha256_hex(&bytes),
        },
    )
    .await?;
    match tokio::time::timeout(IDLE, recv(&mut framed)).await?? {
        Frame::Accepted => {}
        Frame::Refused(why) => anyhow::bail!("refused by node: {why}"),
        other => anyhow::bail!("unexpected frame {other:?}"),
    }
    send_bytes(&mut framed, &bytes).await?;
    let mut result = Vec::new();
    loop {
        match tokio::time::timeout(IDLE, recv(&mut framed)).await?? {
            Frame::Progress { progress, message } => {
                on_event(RunState::Running, Some(progress), message);
            }
            Frame::Chunk(c) => result.extend_from_slice(&c),
            Frame::End => break,
            Frame::Error(why) => anyhow::bail!("node failed: {why}"),
            other => anyhow::bail!("unexpected frame {other:?}"),
        }
    }
    let result: TrainResult = postcard::from_bytes(&result)?;
    anyhow::ensure!(
        result.run_id == bundle.manifest.run_id,
        "result names run {} for run {}",
        result.run_id,
        bundle.manifest.run_id
    );
    send(&mut framed, &Frame::Ack).await?;
    match tokio::time::timeout(IDLE, recv(&mut framed)).await {
        Ok(Ok(Frame::Cleaned)) => {
            on_event(RunState::Cleaned, None, "node deleted the bundle".into())
        }
        Ok(Ok(other)) => tracing::debug!("double: node answered {other:?} to Ack"),
        Ok(Err(e)) => tracing::debug!("double: node closed before Cleaned: {e}"),
        Err(_) => tracing::debug!("double: node did not confirm cleanup in time"),
    }
    Ok(result)
}

// --- node side -----------------------------------------------------------

/// Serve `theuth/double/1` until shutdown; waits for the mesh transport first.
pub async fn serve(rt: Runtime, runner: Arc<dyn Runner>) {
    if let Some(transport) = replica::transport(&rt).await {
        serve_on(rt, transport, runner).await;
    }
}

/// The accept loop of [`serve`] on a given transport.
pub async fn serve_on(rt: Runtime, transport: Arc<dyn Transport>, runner: Arc<dyn Runner>) {
    loop {
        let accepted = tokio::select! {
            () = rt.shutdown.cancelled() => return,
            a = transport.accept(ALPN_DOUBLE) => a,
        };
        match accepted {
            Ok((peer, stream)) => {
                let runner = runner.clone();
                tokio::spawn(async move {
                    if let Err(e) = answer(runner.as_ref(), framed(stream)).await {
                        tracing::debug!(%peer, error = %e, "double mesh stream ended");
                    }
                });
            }
            Err(e) => {
                tracing::warn!(error = %e, "double mesh accept");
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        }
    }
}

/// One accepted stream: the whole node-side protocol.
pub async fn answer(runner: &dyn Runner, mut framed: Stream) -> anyhow::Result<()> {
    let Frame::Train {
        run_id,
        manifest,
        sha,
    } = recv(&mut framed).await?
    else {
        anyhow::bail!("first frame was not Train");
    };
    if let Err(why) = runner.accept(&manifest).await {
        send(&mut framed, &Frame::Refused(why)).await?;
        return Ok(());
    }
    send(&mut framed, &Frame::Accepted).await?;
    let mut bytes = Vec::new();
    loop {
        match recv(&mut framed).await? {
            Frame::Chunk(c) => bytes.extend_from_slice(&c),
            Frame::End => break,
            other => anyhow::bail!("unexpected frame {other:?}"),
        }
    }
    if sha256_hex(&bytes) != sha {
        send(&mut framed, &Frame::Error("bundle sha256 mismatch".into())).await?;
        return Ok(());
    }
    let bundle: Bundle = match postcard::from_bytes(&bytes) {
        Ok(b) => b,
        Err(e) => {
            send(
                &mut framed,
                &Frame::Error(format!("bundle does not decode: {e}")),
            )
            .await?;
            return Ok(());
        }
    };
    drop(bytes);

    let (tx, mut rx) = mpsc::channel::<(f32, String)>(16);
    let mut run = std::pin::pin!(runner.run(bundle, tx));
    let outcome = loop {
        tokio::select! {
            biased;
            r = &mut run => break r,
            p = rx.recv() => {
                if let Some((progress, message)) = p {
                    send(&mut framed, &Frame::Progress { progress, message }).await?;
                }
            }
        }
    };
    // Drain progress the run queued before finishing.
    while let Ok((progress, message)) = rx.try_recv() {
        send(&mut framed, &Frame::Progress { progress, message }).await?;
    }
    match outcome {
        Ok(result) => send_bytes(&mut framed, &postcard::to_allocvec(&result)?).await?,
        Err(e) => {
            send(&mut framed, &Frame::Error(format!("{e:#}"))).await?;
            runner.cleanup(&run_id).await;
            return Ok(());
        }
    }
    if let Ok(Frame::Ack) = recv(&mut framed).await {
        runner.cleanup(&run_id).await;
        send(&mut framed, &Frame::Cleaned).await?;
    } else {
        runner.cleanup(&run_id).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::sync::broadcast;

    type Inbound = (PeerId, Box<dyn ByteStream>);

    /// Loopback switch, as `artifact::mesh` / `replica/tests/sync_loopback.rs`.
    struct Loop {
        me: PeerId,
        inboxes: Arc<Mutex<HashMap<PeerId, tokio::sync::mpsc::UnboundedSender<Inbound>>>>,
        inbox: tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<Inbound>>,
        events: broadcast::Sender<replica::PeerEvent>,
    }

    fn pair() -> (Arc<Loop>, Arc<Loop>) {
        let inboxes = Arc::new(Mutex::new(HashMap::new()));
        let mk = |name: &str| {
            let me = PeerId(name.to_owned());
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            inboxes.lock().insert(me.clone(), tx);
            Arc::new(Loop {
                me,
                inboxes: inboxes.clone(),
                inbox: tokio::sync::Mutex::new(rx),
                events: broadcast::channel(8).0,
            })
        };
        (mk("laptop"), mk("gpu-node"))
    }

    #[async_trait::async_trait]
    impl Transport for Loop {
        fn peers(&self) -> Vec<PeerId> {
            self.inboxes
                .lock()
                .keys()
                .filter(|p| **p != self.me)
                .cloned()
                .collect()
        }
        async fn open(&self, peer: &PeerId, alpn: &str) -> anyhow::Result<Box<dyn ByteStream>> {
            assert_eq!(alpn, ALPN_DOUBLE);
            let (mine, theirs) = tokio::io::duplex(1 << 20);
            let to = self
                .inboxes
                .lock()
                .get(peer)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("no such peer"))?;
            to.send((self.me.clone(), Box::new(theirs)))
                .map_err(|_| anyhow::anyhow!("peer gone"))?;
            Ok(Box::new(mine))
        }
        async fn accept(&self, _alpn: &str) -> anyhow::Result<Inbound> {
            self.inbox
                .lock()
                .await
                .recv()
                .await
                .ok_or_else(|| anyhow::anyhow!("closed"))
        }
        fn subscribe(&self) -> broadcast::Receiver<replica::PeerEvent> {
            self.events.subscribe()
        }
    }

    /// A node runner that "trains": counts non-golden rows, reports two epochs.
    struct FakeNode {
        cleaned: AtomicBool,
        refuse: Option<String>,
        seen: Mutex<Option<Bundle>>,
    }

    #[async_trait::async_trait]
    impl Runner for FakeNode {
        async fn accept(&self, m: &BundleManifest) -> Result<(), String> {
            assert_eq!(m.tier, Tier::B);
            self.refuse.clone().map_or(Ok(()), Err)
        }
        async fn run(
            &self,
            bundle: Bundle,
            progress: mpsc::Sender<(f32, String)>,
        ) -> anyhow::Result<TrainResult> {
            let trained = bundle.rows.iter().filter(|r| !r.golden).count();
            progress.send((0.5, "epoch 1/2".into())).await.unwrap();
            progress.send((1.0, "epoch 2/2".into())).await.unwrap();
            let run_id = bundle.manifest.run_id.clone();
            *self.seen.lock() = Some(bundle);
            Ok(TrainResult {
                run_id,
                adapter: Some(vec![7u8; 1234]),
                heads: None,
                manifest_json: serde_json::json!({"seed": 42}).to_string(),
                metrics_json: serde_json::json!({"trained": trained}).to_string(),
            })
        }
        async fn cleanup(&self, _: &str) {
            self.cleaned.store(true, Ordering::SeqCst);
        }
    }

    fn bundle(rows: usize) -> Bundle {
        let rows: Vec<BundleRow> = (0..rows)
            .map(|i| BundleRow {
                id: format!("e{i}"),
                class: "approval:shell:mutating:git push:root".into(),
                requester: "root".into(),
                card_text: format!("git push origin feat/{i}"),
                options: vec!["Allow".into(), "Deny".into()],
                labels: vec![("direct".into(), "Allow".into(), 1.0)],
                asked_at: "2026-10-07T00:00:00Z".into(),
                golden: i < 2,
            })
            .collect();
        Bundle {
            manifest: BundleManifest {
                run_id: "run-1".into(),
                tier: Tier::B,
                base_hash: "sha256:abc".into(),
                config: TierB::default(),
                rows: rows.len() as u32,
                dim: 8,
                created_at: "2026-10-07T01:00:00Z".into(),
            },
            embeddings: (0..rows.len()).map(|_| vec![0u8; 16]).collect(),
            rows,
        }
    }

    fn serve(node: Arc<Loop>, runner: Arc<FakeNode>) {
        tokio::spawn(async move {
            while let Ok((_, stream)) = node.accept(ALPN_DOUBLE).await {
                let r = runner.clone();
                tokio::spawn(async move { answer(r.as_ref(), framed(stream)).await });
            }
        });
    }

    #[tokio::test]
    async fn bundle_round_trip_in_process() {
        let (laptop, node) = pair();
        let runner = Arc::new(FakeNode {
            cleaned: AtomicBool::new(false),
            refuse: None,
            seen: Mutex::new(None),
        });
        serve(node, runner.clone());
        let b = bundle(12);
        let events = Arc::new(Mutex::new(Vec::new()));
        let ev = events.clone();
        let result = train_on(
            laptop.as_ref(),
            &PeerId("gpu-node".into()),
            &b,
            move |s, p, m| {
                ev.lock().push((s, p, m));
            },
        )
        .await
        .unwrap();
        assert_eq!(result.run_id, "run-1");
        assert_eq!(result.adapter.as_ref().map(Vec::len), Some(1234));
        assert_eq!(
            result.metrics_json,
            serde_json::json!({"trained": 10}).to_string()
        );
        assert_eq!(*runner.seen.lock(), Some(b));
        let ev = events.lock().clone();
        assert_eq!(ev[0], (RunState::Running, Some(0.5), "epoch 1/2".into()));
        assert_eq!(ev[1], (RunState::Running, Some(1.0), "epoch 2/2".into()));
        assert_eq!(ev[2].0, RunState::Cleaned);
        assert!(runner.cleaned.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn a_refusing_node_is_an_error_and_not_yet_refuses() {
        let (laptop, node) = pair();
        let runner = Arc::new(FakeNode {
            cleaned: AtomicBool::new(false),
            refuse: Some("busy".into()),
            seen: Mutex::new(None),
        });
        serve(node, runner);
        let e = train_on(
            laptop.as_ref(),
            &PeerId("gpu-node".into()),
            &bundle(3),
            |_, _, _| {},
        )
        .await
        .unwrap_err();
        assert!(e.to_string().contains("refused by node: busy"), "{e}");
        let why = NotYet.accept(&bundle(1).manifest).await.unwrap_err();
        assert!(why.contains("W3"));
    }

    #[tokio::test]
    async fn a_corrupt_bundle_is_rejected_by_sha() {
        let (laptop, node) = pair();
        // Hand-roll the laptop side with a wrong sha.
        let runner = Arc::new(FakeNode {
            cleaned: AtomicBool::new(false),
            refuse: None,
            seen: Mutex::new(None),
        });
        serve(node, runner.clone());
        let b = bundle(3);
        let mut f = framed(
            laptop
                .open(&PeerId("gpu-node".into()), ALPN_DOUBLE)
                .await
                .unwrap(),
        );
        send(
            &mut f,
            &Frame::Train {
                run_id: "run-1".into(),
                manifest: b.manifest.clone(),
                sha: "00".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(recv(&mut f).await.unwrap(), Frame::Accepted);
        send_bytes(&mut f, &postcard::to_allocvec(&b).unwrap())
            .await
            .unwrap();
        assert_eq!(
            recv(&mut f).await.unwrap(),
            Frame::Error("bundle sha256 mismatch".into())
        );
        assert!(runner.seen.lock().is_none());
    }

    #[tokio::test]
    async fn build_bundle_reads_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("double.sqlite"))
            .await
            .unwrap();
        store
            .put_class(&crate::store::ClassRow {
                key: "c".into(),
                kind: crate::class::CardKind::Approval,
                rung: 0,
                cap: 2,
                mutating_opt_in: false,
                n: 0,
                stats: serde_json::json!({}),
                updated_at: chrono::Utc::now(),
            })
            .await
            .unwrap();
        for i in 0..3 {
            store
                .put_example(&crate::store::ExampleRow {
                    id: format!("e{i}"),
                    class: "c".into(),
                    question_id: format!("q{i}"),
                    session_id: None,
                    requester: "root".into(),
                    card_text: "git push".into(),
                    options: vec!["Allow".into(), "Deny".into()],
                    embedding: Some(vec![1.0, 2.0, 3.0, 4.0]),
                    asked_at: chrono::Utc::now(),
                })
                .await
                .unwrap();
            if i < 2 {
                store
                    .put_label(&crate::store::LabelRow {
                        example_id: format!("e{i}"),
                        source: crate::types::LabelSource::Direct,
                        choice: "Allow".into(),
                        weight: 1.0,
                        at: chrono::Utc::now(),
                    })
                    .await
                    .unwrap();
            }
        }
        store.put_golden("e0").await.unwrap();
        let b = build_bundle(&store, "run-9", Tier::B, "h", &TierB::default())
            .await
            .unwrap();
        assert_eq!(b.manifest.rows, 2, "the unlabelled row stays home");
        assert_eq!(b.manifest.dim, 4);
        assert!(b.rows[0].golden && !b.rows[1].golden);
        assert_eq!(
            b.rows[1].labels,
            vec![("direct".to_owned(), "Allow".to_owned(), 1.0)]
        );
        assert_eq!(b.embeddings.len(), 2);
        assert_eq!(
            crate::store::decode_f16(&b.embeddings[0]),
            vec![1.0, 2.0, 3.0, 4.0]
        );
    }
}
