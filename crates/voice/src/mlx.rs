//! The optional "mlx" whole-utterance STT engine (`docs/design/202-stt-mlx-engine.md`):
//! Parakeet TDT 0.6B v3 (`mlx-community/parakeet-tdt-0.6b-v3`, the published weights) in-process
//! on the Apple GPU through MLX (`voice_mlx`, behind this crate's `mlx` feature), beside the
//! built-in ONNX int8 engine. Weights live under `<data_dir>/models/parakeet-tdt-mlx/`; nothing
//! here downloads implicitly: [`install`] is the one explicit fetch.
//!
//! MLX state is thread-bound (202 §3): one [`Engine`] owns one OS thread that loads the model,
//! warms it and serves every call; requests go in on a channel, replies come back on a per-call
//! channel with a deadline. A late reply is discarded, the thread lives on. The thread exits (and
//! drops the model) after `idle`; a load error, a panic or [`MAX_FAILURES`] errors in a row mark
//! the engine failed and every caller falls back (streaming final / ONNX).

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Weights directory under `<data_dir>/models/`.
pub const MLX_MODEL_SUBDIR: &str = "parakeet-tdt-mlx";
/// Hugging Face repo [`install`] fetches from.
pub const MLX_REPO: &str = "mlx-community/parakeet-tdt-0.6b-v3";
/// The repo files the model loads (`voice_mlx::FILES`).
#[cfg(feature = "mlx")]
pub const FILES: &[&str] = voice_mlx::FILES;
/// The repo files the model loads; mirrors `voice_mlx::FILES` while the `mlx` feature is off.
#[cfg(not(feature = "mlx"))]
pub const FILES: &[&str] = &["config.json", "model.safetensors"];
/// Written last by [`install`]; its presence is "downloaded".
const COMPLETE: &str = ".complete";
/// Errors in a row (not deadlines) that retire the engine.
pub const MAX_FAILURES: u32 = 3;
const MODEL_RATE: usize = 16_000;

/// `voice.stt_engine` (202 §5): which model makes whole-utterance text.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum SttEngine {
    /// Parakeet TDT v3, ONNX int8, built in.
    #[default]
    Onnx,
    /// Parakeet TDT v3 on MLX (Apple silicon), after an explicit download.
    Mlx,
}

/// Where the MLX engine stands (202 §4); the settings UI shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum MlxState {
    /// Not compiled into this build (only macOS on Apple silicon has it).
    Unavailable { reason: String },
    /// Runnable, not downloaded.
    Absent,
    /// [`install`] running; bytes so far.
    Downloading { done_bytes: u64, total_bytes: u64 },
    /// Verified weights on disk.
    Ready,
    /// The last [`install`] failed.
    Failed { reason: String },
}

/// The MLX model is compiled into this build.
#[cfg(feature = "mlx")]
pub fn compiled() -> bool {
    voice_mlx::available()
}

/// The MLX model is compiled into this build.
#[cfg(not(feature = "mlx"))]
pub fn compiled() -> bool {
    false
}

pub fn model_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("models").join(MLX_MODEL_SUBDIR)
}

/// Every file in [`FILES`] verified and the marker written.
pub fn downloaded(data_dir: &Path) -> bool {
    model_dir(data_dir).join(COMPLETE).is_file()
}

/// Progress of the one [`install`] that may run per process.
static INSTALL: LazyLock<Mutex<Option<MlxState>>> = LazyLock::new(|| Mutex::new(None));

/// The engine's state under `data_dir`.
pub fn state(data_dir: &Path) -> MlxState {
    if !compiled() {
        return MlxState::Unavailable {
            reason: "this build has no MLX engine (macOS on Apple silicon only)".into(),
        };
    }
    if let Some(s @ (MlxState::Downloading { .. } | MlxState::Failed { .. })) =
        INSTALL.lock().clone()
    {
        return s;
    }
    if downloaded(data_dir) {
        MlxState::Ready
    } else {
        MlxState::Absent
    }
}

/// The explicit download (202 §4): every file in [`FILES`] from the hub into
/// `<data_dir>/models/parakeet-tdt-mlx/`. One at a time per process; progress in [`state`].
pub async fn install(data_dir: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(compiled(), "this build has no MLX engine");
    {
        let mut g = INSTALL.lock();
        anyhow::ensure!(
            !matches!(*g, Some(MlxState::Downloading { .. })),
            "a download is already running"
        );
        *g = Some(MlxState::Downloading {
            done_bytes: 0,
            total_bytes: 0,
        });
    }
    let base = format!("https://huggingface.co/{MLX_REPO}/resolve/main");
    let r = download(&base, &model_dir(data_dir), |done, total| {
        *INSTALL.lock() = Some(MlxState::Downloading {
            done_bytes: done,
            total_bytes: total,
        });
    })
    .await;
    *INSTALL.lock() = match &r {
        Ok(()) => None,
        Err(e) => {
            tracing::warn!("voice: mlx download: {e:#}");
            Some(MlxState::Failed {
                reason: format!("{e:#}"),
            })
        }
    };
    r
}

/// What the hub says a file is: size, and sha256 for LFS files.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Expect {
    size: Option<u64>,
    sha256: Option<String>,
}

/// From the headers of an un-followed `HEAD <base>/<file>`: LFS files answer with
/// `x-linked-size` and `x-linked-etag` (the sha256, quoted); others with `content-length`.
fn expect_from(h: &reqwest::header::HeaderMap) -> Expect {
    let get = |k: &str| h.get(k).and_then(|v| v.to_str().ok());
    let size = get("x-linked-size")
        .or_else(|| get("content-length"))
        .and_then(|v| v.parse().ok());
    let sha256 = get("x-linked-etag")
        .map(|v| {
            v.trim_start_matches("W/")
                .trim_matches('"')
                .to_ascii_lowercase()
        })
        .filter(|v| v.len() == 64 && v.chars().all(|c| c.is_ascii_hexdigit()));
    Expect { size, sha256 }
}

/// `<dest>.part`.
fn part_of(dest: &Path) -> PathBuf {
    let mut p = dest.as_os_str().to_owned();
    p.push(".part");
    PathBuf::from(p)
}

/// sha256 of a whole file, hex.
fn sha256_file(path: &Path) -> anyhow::Result<String> {
    use std::io::Read as _;
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex::encode(h.finalize()))
}

/// `part` matches `e`; else why not.
fn check(part: &Path, e: &Expect) -> anyhow::Result<()> {
    let n = std::fs::metadata(part)?.len();
    if let Some(size) = e.size {
        anyhow::ensure!(n == size, "{}: {n} bytes, hub says {size}", part.display());
    }
    if let Some(want) = &e.sha256 {
        let got = sha256_file(part)?;
        anyhow::ensure!(
            &got == want,
            "{}: sha256 {got}, hub says {want}",
            part.display()
        );
    }
    Ok(())
}

/// Fetch every file in [`FILES`] from `base/<file>` into `dir`: `.part` files resumed with
/// `Range` when the server allows, size and (LFS) sha256 checked, renamed into place, and the
/// marker written last. `progress(done, total)` after each chunk.
async fn download(
    base: &str,
    dir: &Path,
    mut progress: impl FnMut(u64, u64),
) -> anyhow::Result<()> {
    use tokio::io::AsyncWriteExt as _;
    tokio::fs::create_dir_all(dir).await?;
    let _ = tokio::fs::remove_file(dir.join(COMPLETE)).await;
    let head = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let get = reqwest::Client::new();
    let mut expects = Vec::new();
    for f in FILES {
        let r = head.head(format!("{base}/{f}")).send().await?;
        anyhow::ensure!(
            r.status().is_success() || r.status().is_redirection(),
            "HEAD {f}: {}",
            r.status()
        );
        expects.push(expect_from(r.headers()));
    }
    let total: u64 = expects.iter().filter_map(|e| e.size).sum();
    let mut done = 0u64;
    for (f, e) in FILES.iter().zip(&expects) {
        let dest = dir.join(f);
        if dest.is_file() && check(&dest, e).is_ok() {
            done += e.size.unwrap_or(0);
            progress(done, total);
            continue;
        }
        let part = part_of(&dest);
        let have = tokio::fs::metadata(&part).await.map_or(0, |m| m.len());
        let mut req = get.get(format!("{base}/{f}"));
        if have > 0 {
            req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
        }
        let mut resp = req.send().await?.error_for_status()?;
        let resumed = have > 0 && resp.status() == reqwest::StatusCode::PARTIAL_CONTENT;
        let mut file = if resumed {
            done += have;
            tokio::fs::OpenOptions::new()
                .append(true)
                .open(&part)
                .await?
        } else {
            tokio::fs::File::create(&part).await?
        };
        while let Some(chunk) = resp.chunk().await? {
            file.write_all(&chunk).await?;
            done += chunk.len() as u64;
            progress(done, total);
        }
        file.flush().await?;
        drop(file);
        let (p, e2) = (part.clone(), e.clone());
        if let Err(err) = tokio::task::spawn_blocking(move || check(&p, &e2)).await? {
            let _ = tokio::fs::remove_file(&part).await;
            return Err(err);
        }
        tokio::fs::rename(&part, &dest).await?;
    }
    tokio::fs::write(dir.join(COMPLETE), MLX_REPO).await?;
    Ok(())
}

/// The model behind the engine thread; built on that thread by the loader, never leaves it.
pub trait Model {
    fn warm(&mut self) -> anyhow::Result<()>;
    /// Text of 16 kHz mono f32.
    fn transcribe(&mut self, pcm16k: &[f32]) -> anyhow::Result<String>;
}

#[cfg(feature = "mlx")]
impl Model for voice_mlx::ParakeetMlx {
    fn warm(&mut self) -> anyhow::Result<()> {
        voice_mlx::ParakeetMlx::warm(self)
    }
    fn transcribe(&mut self, pcm16k: &[f32]) -> anyhow::Result<String> {
        Ok(voice_mlx::ParakeetMlx::transcribe(self, pcm16k)?.text)
    }
}

/// Builds the model, on the engine thread.
pub type Loaded = anyhow::Result<Box<dyn Model>>;
pub type Loader = Arc<dyn Fn() -> Loaded + Send + Sync>;

struct Job {
    pcm: Vec<f32>,
    reply: mpsc::Sender<anyhow::Result<String>>,
}

/// What the engine thread is doing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    /// No thread (never started, or exited after idle).
    Stopped,
    Loading,
    Ready,
    /// Load error, panic, or [`MAX_FAILURES`] errors in a row; sticky.
    Failed(String),
}

struct Inner {
    jobs: Option<mpsc::Sender<Job>>,
    phase: Arc<Mutex<Phase>>,
    /// Bumped per thread, so an exiting old thread does not stop a new one.
    generation: u64,
    failures: u32,
}

/// One engine thread over a [`Model`] (202 §3).
pub struct Engine {
    loader: Loader,
    idle: Duration,
    inner: Mutex<Inner>,
}

fn panic_text(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic".into())
}

impl Engine {
    /// `idle` zero: the thread never exits on its own.
    pub fn new(loader: Loader, idle: Duration) -> Engine {
        Engine {
            loader,
            idle,
            inner: Mutex::new(Inner {
                jobs: None,
                phase: Arc::new(Mutex::new(Phase::Stopped)),
                generation: 0,
                failures: 0,
            }),
        }
    }

    pub fn phase(&self) -> Phase {
        self.inner.lock().phase.lock().clone()
    }

    /// Start the thread unless running or failed; never blocks on the load.
    pub fn prewarm(&self) {
        let _ = self.sender();
    }

    /// The running thread's queue, starting one if stopped; `Err` once failed.
    fn sender(&self) -> anyhow::Result<mpsc::Sender<Job>> {
        let mut g = self.inner.lock();
        let phase = g.phase.lock().clone();
        match phase {
            Phase::Failed(r) => anyhow::bail!("mlx engine failed: {r}"),
            Phase::Loading | Phase::Ready if g.jobs.is_some() => {
                return Ok(g.jobs.clone().expect("checked"));
            }
            _ => {}
        }
        let (tx, rx) = mpsc::channel::<Job>();
        g.generation += 1;
        let phase = Arc::new(Mutex::new(Phase::Loading));
        g.phase = phase.clone();
        g.jobs = Some(tx.clone());
        let (loader, idle) = (self.loader.clone(), self.idle);
        std::thread::Builder::new()
            .name("voice-mlx".into())
            .spawn(move || serve(&*loader, idle, rx, &phase))?;
        Ok(tx)
    }

    /// One transcription, waiting at most `deadline` (including a load when `wait_load`;
    /// without it, a loading engine fails at once). A late result is discarded; the thread lives on.
    pub fn transcribe(
        &self,
        pcm16k: &[f32],
        deadline: Duration,
        wait_load: bool,
    ) -> anyhow::Result<String> {
        let tx = self.sender()?;
        if !wait_load && self.phase() == Phase::Loading {
            anyhow::bail!("mlx engine still loading");
        }
        let (reply, rx) = mpsc::channel();
        tx.send(Job {
            pcm: pcm16k.to_vec(),
            reply,
        })
        .map_err(|_| anyhow::anyhow!("mlx engine thread gone"))?;
        let r = match rx.recv_timeout(deadline) {
            Ok(r) => r,
            Err(RecvTimeoutError::Timeout) => {
                anyhow::bail!("mlx engine gave no text in {deadline:?}; the late result is dropped")
            }
            Err(RecvTimeoutError::Disconnected) => Err(anyhow::anyhow!(
                "mlx engine thread ended: {:?}",
                self.phase()
            )),
        };
        let mut g = self.inner.lock();
        match &r {
            Ok(_) => g.failures = 0,
            Err(e) => {
                g.failures += 1;
                if g.failures >= MAX_FAILURES {
                    let mut p = g.phase.lock();
                    if !matches!(*p, Phase::Failed(_)) {
                        *p = Phase::Failed(format!("{MAX_FAILURES} errors in a row, last: {e:#}"));
                    }
                    drop(p);
                    // Dropping the queue ends the thread once it is idle.
                    g.jobs = None;
                }
            }
        }
        r
    }
}

/// The engine thread: load, warm, serve until idle or failed.
fn serve(
    loader: &(dyn Fn() -> anyhow::Result<Box<dyn Model>> + Send + Sync),
    idle: Duration,
    rx: mpsc::Receiver<Job>,
    phase: &Mutex<Phase>,
) {
    let t = Instant::now();
    let loaded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut m = loader()?;
        m.warm()?;
        anyhow::Ok(m)
    }));
    let mut model = match loaded {
        Ok(Ok(m)) => m,
        Ok(Err(e)) => {
            tracing::warn!("voice: mlx model failed to load: {e:#}; using ONNX");
            *phase.lock() = Phase::Failed(format!("{e:#}"));
            return;
        }
        Err(p) => {
            let e = panic_text(&*p);
            tracing::warn!("voice: mlx model load panicked: {e}; using ONNX");
            *phase.lock() = Phase::Failed(format!("load panicked: {e}"));
            return;
        }
    };
    tracing::info!("voice: mlx model loaded and warm in {:?}", t.elapsed());
    {
        let mut p = phase.lock();
        if *p == Phase::Loading {
            *p = Phase::Ready;
        }
    }
    loop {
        let job = if idle.is_zero() {
            rx.recv().map_err(|_| RecvTimeoutError::Disconnected)
        } else {
            rx.recv_timeout(idle)
        };
        let job = match job {
            Ok(j) => j,
            Err(RecvTimeoutError::Timeout) => {
                tracing::info!("voice: mlx model unloaded after {} s idle", idle.as_secs());
                let mut p = phase.lock();
                if *p == Phase::Ready {
                    *p = Phase::Stopped;
                }
                return;
            }
            Err(RecvTimeoutError::Disconnected) => return,
        };
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| model.transcribe(&job.pcm)))
        {
            // A send error is a caller past its deadline: the result is dropped.
            Ok(r) => {
                let _ = job.reply.send(r);
            }
            Err(p) => {
                let e = panic_text(&*p);
                tracing::warn!("voice: mlx transcribe panicked: {e}; engine retired, using ONNX");
                *phase.lock() = Phase::Failed(format!("panicked: {e}"));
                let _ = job
                    .reply
                    .send(Err(anyhow::anyhow!("mlx transcribe panicked: {e}")));
                return;
            }
        }
    }
}

/// A whole-utterance transcriber for live finals (202 §5). Blocking.
pub trait Whole: Send + Sync {
    /// Start loading in the background if not loaded; never blocks.
    fn prewarm(&self) {}
    /// Usable now; `false` (model not downloaded, engine failed) skips without a warning.
    fn ready(&self) -> bool {
        true
    }
    /// Text of 16 kHz mono `pcm16k`, or an error within about `deadline`.
    fn transcribe(&self, pcm16k: &[f32], deadline: Duration) -> anyhow::Result<String>;
}

/// The live-final deadline (202 §5): 2.5 s + 0.5 × utterance seconds.
pub fn deadline_for(samples: usize) -> Duration {
    Duration::from_secs_f64(2.5 + 0.5 * samples as f64 / MODEL_RATE as f64)
}

/// The utterance's final text (202 §5): `whole`'s text of `audio` when it answers within
/// [`deadline_for`] with something, else the streaming text, with a warning (an empty MLX
/// text never replaces a non-empty streaming one).
pub fn final_text(whole: Option<&dyn Whole>, audio: &[f32], streaming: String) -> String {
    let Some(w) = whole.filter(|w| w.ready()) else {
        return streaming;
    };
    let deadline = deadline_for(audio.len());
    let t = Instant::now();
    match w.transcribe(audio, deadline) {
        Ok(text) if t.elapsed() > deadline => {
            tracing::warn!(
                "voice: mlx final took {:?} (deadline {deadline:?}); keeping the streaming text {text:?}",
                t.elapsed()
            );
            streaming
        }
        Ok(text) if text.trim().is_empty() && !streaming.trim().is_empty() => {
            tracing::warn!("voice: mlx final empty; keeping the streaming text {streaming:?}");
            streaming
        }
        Ok(text) => {
            tracing::debug!(
                "voice: mlx final in {:?} for {} ms of audio",
                t.elapsed(),
                audio.len() * 1000 / MODEL_RATE
            );
            text.trim().to_owned()
        }
        Err(e) => {
            tracing::warn!("voice: mlx final failed ({e:#}); keeping the streaming text");
            streaming
        }
    }
}

/// The MLX engine of one data dir.
pub struct Mlx {
    data_dir: PathBuf,
    engine: Engine,
}

/// The real model loader.
#[cfg(feature = "mlx")]
fn loader(dir: PathBuf) -> Loader {
    Arc::new(move || -> Loaded {
        Ok(Box::new(voice_mlx::ParakeetMlx::load(&dir)?) as Box<dyn Model>)
    })
}

/// Without the `mlx` feature every load fails (the engine is never selected then).
#[cfg(not(feature = "mlx"))]
fn loader(dir: PathBuf) -> Loader {
    Arc::new(move || -> Loaded {
        anyhow::bail!("built without the MLX engine ({})", dir.display())
    })
}

impl Mlx {
    pub fn new(data_dir: &Path, idle: Duration) -> Arc<Mlx> {
        Arc::new(Mlx {
            data_dir: data_dir.to_owned(),
            engine: Engine::new(loader(model_dir(data_dir)), idle),
        })
    }

    /// Compiled in, downloaded, not failed.
    pub fn installed(&self) -> bool {
        compiled() && downloaded(&self.data_dir) && !matches!(self.engine.phase(), Phase::Failed(_))
    }

    /// Start loading in the background when installed.
    pub fn prewarm(&self) {
        if self.installed() {
            self.engine.prewarm();
        }
    }

    /// One piece (≤ [`crate::stt::PIECE`]) of a whole clip: waits for a load.
    pub fn transcribe_clip(&self, pcm16k: &[f32]) -> anyhow::Result<String> {
        anyhow::ensure!(self.installed(), "the MLX model is not ready");
        let timeout = Duration::from_secs(120) + deadline_for(pcm16k.len()) * 2;
        Ok(self
            .engine
            .transcribe(pcm16k, timeout, true)?
            .trim()
            .to_owned())
    }
}

impl Whole for Mlx {
    fn ready(&self) -> bool {
        self.installed()
    }
    fn prewarm(&self) {
        Mlx::prewarm(self);
    }
    /// Never waits for a model load: still loading → this one fails (streaming final).
    fn transcribe(&self, pcm16k: &[f32], deadline: Duration) -> anyhow::Result<String> {
        self.engine.transcribe(pcm16k, deadline, false)
    }
}

/// The engine `voice.stt_engine` picks (202 §5): `None` = ONNX. `mlx` in a build without MLX
/// falls back to ONNX with a warning; `mlx` not yet downloaded is returned anyway (each call
/// checks [`Mlx::installed`], so a download made while running takes effect) and runs on ONNX
/// until then. Publishes the engine to the live paths and prewarms it when installed.
pub fn select(engine: SttEngine, data_dir: &Path, idle: Duration) -> Option<Arc<Mlx>> {
    let m = match engine {
        SttEngine::Onnx => None,
        SttEngine::Mlx if !compiled() => {
            tracing::warn!("voice: stt_engine mlx: not in this build; using the ONNX engine");
            None
        }
        SttEngine::Mlx => {
            let m = Mlx::new(data_dir, idle);
            if m.installed() {
                tracing::info!("voice: stt_engine mlx: whole-utterance text on MLX");
                m.prewarm();
            } else {
                tracing::warn!(
                    "voice: stt_engine mlx: model not downloaded ({}); using the ONNX engine until it is",
                    model_dir(data_dir).display()
                );
            }
            Some(m)
        }
    };
    set_live(m.clone().map(|m| m as Arc<dyn Whole>));
    m
}

/// The engine live paths use for finals ([`crate::live::Utterer::open`] reads it); set by the
/// plugin when `voice.stt_engine: mlx`.
static LIVE: LazyLock<RwLock<Option<Arc<dyn Whole>>>> = LazyLock::new(|| RwLock::new(None));

pub fn set_live(engine: Option<Arc<dyn Whole>>) {
    *LIVE.write() = engine;
}

pub(crate) fn live() -> Option<Arc<dyn Whole>> {
    LIVE.read().clone()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// Answers `text`, or an error for audio of length 1, sleeps for length 2, panics for 3.
    struct FakeModel {
        text: &'static str,
        sleep: Duration,
    }
    impl Model for FakeModel {
        fn warm(&mut self) -> anyhow::Result<()> {
            Ok(())
        }
        fn transcribe(&mut self, pcm: &[f32]) -> anyhow::Result<String> {
            match pcm.len() {
                1 => anyhow::bail!("bad audio"),
                2 => std::thread::sleep(self.sleep),
                3 => panic!("model blew up"),
                _ => {}
            }
            Ok(self.text.to_owned())
        }
    }

    fn engine(idle: Duration, loads: Arc<AtomicUsize>) -> Engine {
        Engine::new(
            Arc::new(move || -> Loaded {
                loads.fetch_add(1, Ordering::SeqCst);
                Ok(Box::new(FakeModel {
                    text: "hello",
                    sleep: Duration::from_millis(400),
                }) as Box<dyn Model>)
            }),
            idle,
        )
    }

    const LONG: Duration = Duration::from_secs(5);

    #[test]
    fn ok_and_error_replies() {
        let e = engine(Duration::ZERO, Arc::default());
        assert_eq!(e.phase(), Phase::Stopped);
        assert_eq!(e.transcribe(&[0.0; 8], LONG, true).unwrap(), "hello");
        assert_eq!(e.phase(), Phase::Ready);
        assert!(
            format!("{:#}", e.transcribe(&[0.0], LONG, true).unwrap_err()).contains("bad audio")
        );
        // One error does not retire it.
        assert_eq!(e.transcribe(&[0.0; 8], LONG, false).unwrap(), "hello");
    }

    #[test]
    fn deadline_drops_the_late_result_and_keeps_the_engine() {
        let loads = Arc::new(AtomicUsize::new(0));
        let e = engine(Duration::ZERO, loads.clone());
        e.transcribe(&[0.0; 8], LONG, true).unwrap();
        let t = Instant::now();
        let err = e
            .transcribe(&[0.0; 2], Duration::from_millis(50), false)
            .unwrap_err();
        assert!(format!("{err:#}").contains("late result"), "{err:#}");
        assert!(t.elapsed() < Duration::from_millis(300));
        // The thread finishes the late job, then serves the next one; no reload.
        assert_eq!(e.transcribe(&[0.0; 8], LONG, false).unwrap(), "hello");
        assert_eq!(loads.load(Ordering::SeqCst), 1);
        assert_eq!(e.phase(), Phase::Ready);
    }

    #[test]
    fn panic_retires_the_engine() {
        let e = engine(Duration::ZERO, Arc::default());
        assert!(e.transcribe(&[0.0; 3], LONG, true).is_err());
        assert!(matches!(e.phase(), Phase::Failed(r) if r.contains("blew up")));
        assert!(e.transcribe(&[0.0; 8], LONG, true).is_err());
    }

    #[test]
    fn repeated_errors_retire_the_engine() {
        let e = engine(Duration::ZERO, Arc::default());
        for _ in 0..MAX_FAILURES {
            assert!(e.transcribe(&[0.0], LONG, true).is_err());
        }
        assert!(matches!(e.phase(), Phase::Failed(_)));
        assert!(e.transcribe(&[0.0; 8], LONG, true).is_err());
    }

    #[test]
    fn load_error_and_load_panic_fail() {
        let e = Engine::new(
            Arc::new(|| -> Loaded { anyhow::bail!("no weights") }),
            Duration::ZERO,
        );
        assert!(e.transcribe(&[0.0; 8], LONG, true).is_err());
        assert!(matches!(e.phase(), Phase::Failed(r) if r.contains("no weights")));
        let e = Engine::new(Arc::new(|| -> Loaded { panic!("metal") }), Duration::ZERO);
        assert!(e.transcribe(&[0.0; 8], LONG, true).is_err());
        assert!(matches!(e.phase(), Phase::Failed(r) if r.contains("metal")));
    }

    #[test]
    fn loading_engine_fails_fast_without_wait_load() {
        let e = Engine::new(
            Arc::new(|| -> Loaded {
                std::thread::sleep(Duration::from_millis(300));
                Ok(Box::new(FakeModel {
                    text: "x",
                    sleep: Duration::ZERO,
                }) as Box<dyn Model>)
            }),
            Duration::ZERO,
        );
        e.prewarm();
        let err = e.transcribe(&[0.0; 8], LONG, false).unwrap_err();
        assert!(format!("{err:#}").contains("loading"), "{err:#}");
        assert_eq!(e.transcribe(&[0.0; 8], LONG, true).unwrap(), "x");
    }

    #[test]
    fn idle_unloads_and_the_next_call_reloads() {
        let loads = Arc::new(AtomicUsize::new(0));
        let e = engine(Duration::from_millis(100), loads.clone());
        e.transcribe(&[0.0; 8], LONG, true).unwrap();
        std::thread::sleep(Duration::from_millis(400));
        assert_eq!(e.phase(), Phase::Stopped);
        assert_eq!(e.transcribe(&[0.0; 8], LONG, true).unwrap(), "hello");
        assert_eq!(loads.load(Ordering::SeqCst), 2);
    }

    struct Fake(Result<&'static str, &'static str>, Duration, bool);
    impl Whole for Fake {
        fn ready(&self) -> bool {
            self.2
        }
        fn transcribe(&self, _: &[f32], _: Duration) -> anyhow::Result<String> {
            std::thread::sleep(self.1);
            self.0.map(str::to_owned).map_err(anyhow::Error::msg)
        }
    }

    #[test]
    fn final_text_replaces_or_falls_back() {
        let audio = vec![0f32; 16_000];
        let s = || "streaming text".to_owned();
        // onnx: no engine, the streaming final unchanged.
        assert_eq!(final_text(None, &audio, s()), "streaming text");
        let ok = Fake(Ok(" Whole text. "), Duration::ZERO, true);
        assert_eq!(final_text(Some(&ok), &audio, s()), "Whole text.");
        let err = Fake(Err("boom"), Duration::ZERO, true);
        assert_eq!(final_text(Some(&err), &audio, s()), "streaming text");
        let absent = Fake(Ok("x"), Duration::ZERO, false);
        assert_eq!(final_text(Some(&absent), &audio, s()), "streaming text");
        let empty = Fake(Ok("  "), Duration::ZERO, true);
        assert_eq!(final_text(Some(&empty), &audio, s()), "streaming text");
        assert_eq!(final_text(Some(&empty), &audio, String::new()), "");
        // Late past 2.5 s + 0.5 × 0.01 s.
        let late = Fake(Ok("x"), Duration::from_millis(2_600), true);
        assert_eq!(
            final_text(Some(&late), &audio[..160], s()),
            "streaming text"
        );
    }

    #[test]
    fn deadline_grows_with_the_utterance() {
        assert_eq!(deadline_for(0), Duration::from_millis(2_500));
        assert_eq!(deadline_for(4 * 16_000), Duration::from_millis(4_500));
    }

    #[test]
    fn engine_selection_and_fallback() {
        #[derive(Deserialize)]
        struct C {
            #[serde(default)]
            stt_engine: SttEngine,
        }
        let c: C = serde_json::from_str("{}").unwrap();
        assert_eq!(c.stt_engine, SttEngine::Onnx);
        let c: C = serde_json::from_str(r#"{"stt_engine":"mlx"}"#).unwrap();
        assert_eq!(c.stt_engine, SttEngine::Mlx);
        let dir = std::env::temp_dir().join(format!("theuth-mlx-{}", uuid::Uuid::now_v7()));
        assert!(select(SttEngine::Onnx, &dir, Duration::ZERO).is_none());
        assert!(live().is_none());
        // Not downloaded (and, without the feature, not compiled): nothing is ready, ONNX runs.
        let m = Mlx::new(&dir, Duration::ZERO);
        assert!(!m.installed());
        assert!(!Whole::ready(&*m));
        assert!(m.transcribe_clip(&[0.0; 160]).is_err());
        assert!(matches!(
            state(&dir),
            MlxState::Absent | MlxState::Unavailable { .. }
        ));
    }

    #[test]
    fn hub_headers_give_size_and_sha() {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert("content-length", "1234".parse().unwrap());
        assert_eq!(
            expect_from(&h),
            Expect {
                size: Some(1234),
                sha256: None
            }
        );
        let sha = "a".repeat(64);
        h.insert("x-linked-size", "2509044141".parse().unwrap());
        h.insert("x-linked-etag", format!("\"{sha}\"").parse().unwrap());
        assert_eq!(
            expect_from(&h),
            Expect {
                size: Some(2_509_044_141),
                sha256: Some(sha)
            }
        );
        // A git (sha1) etag is not a sha256.
        h.insert(
            "x-linked-etag",
            "\"0123456789abcdef0123456789abcdef01234567\""
                .parse()
                .unwrap(),
        );
        assert_eq!(expect_from(&h).sha256, None);
    }

    #[test]
    fn check_compares_size_and_hash() {
        let dir = std::env::temp_dir().join(format!("theuth-mlx-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("x.part");
        std::fs::write(&f, b"abc").unwrap();
        let sha = hex::encode(Sha256::digest(b"abc"));
        assert_eq!(part_of(&dir.join("x")), f);
        check(
            &f,
            &Expect {
                size: Some(3),
                sha256: Some(sha.clone()),
            },
        )
        .unwrap();
        assert!(
            check(
                &f,
                &Expect {
                    size: Some(4),
                    sha256: None
                }
            )
            .is_err()
        );
        assert!(
            check(
                &f,
                &Expect {
                    size: None,
                    sha256: Some("0".repeat(64))
                }
            )
            .is_err()
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The download against a local hub: LFS headers on HEAD, a resumed `.part`, the marker last.
    #[tokio::test]
    async fn download_resumes_verifies_and_marks_complete() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let body = |f: &str| format!("contents of {f}").into_bytes();
        for f in FILES {
            let b = body(f);
            Mock::given(method("HEAD"))
                .and(path(format!("/{f}")))
                .respond_with(
                    ResponseTemplate::new(302)
                        .insert_header("x-linked-size", b.len().to_string())
                        .insert_header(
                            "x-linked-etag",
                            format!("\"{}\"", hex::encode(Sha256::digest(&b))),
                        ),
                )
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path(format!("/{f}")))
                .and(header("range", "bytes=5-"))
                .respond_with(ResponseTemplate::new(206).set_body_bytes(b[5..].to_vec()))
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path(format!("/{f}")))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(b))
                .mount(&server)
                .await;
        }
        let dir = std::env::temp_dir().join(format!("theuth-mlx-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        // A half-done first file resumes from byte 5.
        std::fs::write(part_of(&dir.join(FILES[0])), &body(FILES[0])[..5]).unwrap();
        let mut last = (0, 0);
        download(&server.uri(), &dir, |d, t| last = (d, t))
            .await
            .unwrap();
        for f in FILES {
            assert_eq!(std::fs::read(dir.join(f)).unwrap(), body(f));
            assert!(!part_of(&dir.join(f)).exists());
        }
        assert!(dir.join(COMPLETE).is_file());
        assert_eq!(last.0, last.1);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The real model on a real dictation clip (`$THEUTH_MLX_DATA_DIR` with the weights under
    /// `models/parakeet-tdt-mlx/`, `$THEUTH_MLX_CLIP` a 16 kHz `.pcm16`). Needs `--features mlx`.
    #[test]
    #[ignore = "loads the MLX model (~2.5 GB); set THEUTH_MLX_DATA_DIR and THEUTH_MLX_CLIP, --features mlx"]
    fn real_model_transcribes_a_clip() {
        let data_dir = PathBuf::from(std::env::var("THEUTH_MLX_DATA_DIR").unwrap());
        let clip = std::fs::read(std::env::var("THEUTH_MLX_CLIP").unwrap()).unwrap();
        let pcm: Vec<f32> = clip
            .chunks_exact(2)
            .map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.0)
            .collect();
        let m = Mlx::new(&data_dir, Duration::ZERO);
        let t = Instant::now();
        let text = m.transcribe_clip(&pcm).unwrap();
        eprintln!(
            "mlx (load + warm + first): {text:?} in {:?} for {:.1} s of audio",
            t.elapsed(),
            pcm.len() as f64 / 16_000.0
        );
        let t = Instant::now();
        let live = Whole::transcribe(&*m, &pcm, deadline_for(pcm.len())).unwrap();
        eprintln!("mlx (warm, live path): {live:?} in {:?}", t.elapsed());
        assert!(!text.is_empty());
    }
}
