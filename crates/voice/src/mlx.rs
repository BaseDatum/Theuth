//! The optional "mlx" whole-utterance STT engine (`docs/design/202-stt-mlx-engine.md`):
//! Parakeet TDT 0.6B v3 at the published precision on the Apple GPU through MLX, run by a
//! Python sidecar (`sidecar/parakeet_mlx_server.py`, embedded here) in a uv-made venv under
//! `<data_dir>/engines/parakeet-mlx/`, weights under `<data_dir>/models/parakeet-tdt-mlx/`.
//! Nothing here downloads implicitly: [`install`] is the one explicit fetch.
//!
//! Protocol (§2): newline-delimited JSON on the sidecar's stdin/stdout, one
//! `{ready, model, backend}` line once loaded, then `{id, op, ...}` → `{id, text, ms}` |
//! `{id, error}`, served serially; logs on stderr. [`Client`] owns the process: spawned
//! lazily, killed on a hung request, restarted once after a crash, dropped after `idle`,
//! killed on drop.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, LazyLock, Weak};
use std::time::{Duration, Instant};

use base64::Engine as _;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};

use crate::stt::{Piece, Word};

/// The sidecar script, written to `<data_dir>/engines/parakeet-mlx/server.py` before each spawn.
const SCRIPT: &str = include_str!("../sidecar/parakeet_mlx_server.py");
/// Weights directory under `<data_dir>/models/`.
pub const MLX_MODEL_SUBDIR: &str = "parakeet-tdt-mlx";
/// Hugging Face repo [`install`] fetches.
pub const MLX_REPO: &str = "mlx-community/parakeet-tdt-0.6b-v3";
/// `parakeet-mlx` requirement of the venv.
const PACKAGE: &str = "parakeet-mlx>=0.7.0";
/// Model load (cold weights from disk + one warm clip).
const LOAD_TIMEOUT: Duration = Duration::from_secs(180);
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
    /// Cannot run here: not Apple silicon, or no `uv` to build the environment.
    Unavailable { reason: String },
    /// Runnable, not downloaded.
    Absent,
    /// [`install`] running; bytes so far of the weights.
    Downloading { done_bytes: u64, total_bytes: u64 },
    /// Environment and verified weights on disk.
    Ready,
    /// The last [`install`] failed.
    Failed { reason: String },
}

/// The layout under one data dir.
#[derive(Clone, Debug)]
pub struct Paths {
    pub data_dir: PathBuf,
}

impl Paths {
    pub fn new(data_dir: &Path) -> Paths {
        Paths {
            data_dir: data_dir.to_owned(),
        }
    }
    pub fn model_dir(&self) -> PathBuf {
        self.data_dir.join("models").join(MLX_MODEL_SUBDIR)
    }
    pub fn env_dir(&self) -> PathBuf {
        self.data_dir.join("engines").join("parakeet-mlx")
    }
    pub fn python(&self) -> PathBuf {
        self.env_dir().join("bin").join("python")
    }
    pub fn script(&self) -> PathBuf {
        self.env_dir().join("server.py")
    }
    /// Venv and verified weights (the `.complete` marker [`install`] writes last) present.
    pub fn installed(&self) -> bool {
        self.python().is_file() && self.model_dir().join(".complete").is_file()
    }
    /// Write the embedded script (idempotent; keeps it in step with the binary).
    fn write_script(&self) -> anyhow::Result<PathBuf> {
        let path = self.script();
        if std::fs::read_to_string(&path).ok().as_deref() != Some(SCRIPT) {
            std::fs::create_dir_all(self.env_dir())?;
            std::fs::write(&path, SCRIPT)?;
        }
        Ok(path)
    }
    /// The serving command: the venv's python, offline, from the local weights.
    fn serve_command(&self) -> anyhow::Result<Command> {
        let mut cmd = Command::new(self.python());
        cmd.arg(self.write_script()?)
            .arg("serve")
            .arg(self.model_dir())
            .env("HF_HUB_OFFLINE", "1")
            .env("HF_HOME", self.env_dir().join("hf-home"))
            .env("PYTHONUNBUFFERED", "1");
        Ok(cmd)
    }
}

/// `None` when the platform can run MLX, else why not.
pub fn unsupported() -> Option<&'static str> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        None
    } else {
        Some("the MLX engine needs an Apple-silicon Mac")
    }
}

/// `uv` on `PATH` (never installed for the user).
pub fn has_uv() -> bool {
    Command::new("uv")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Progress of the one [`install`] that may run per process.
static INSTALL: LazyLock<Mutex<Option<MlxState>>> = LazyLock::new(|| Mutex::new(None));

/// The engine's state under `data_dir`.
pub fn state(data_dir: &Path) -> MlxState {
    if let Some(r) = unsupported() {
        return MlxState::Unavailable { reason: r.into() };
    }
    if let Some(s @ (MlxState::Downloading { .. } | MlxState::Failed { .. })) =
        INSTALL.lock().clone()
    {
        return s;
    }
    if Paths::new(data_dir).installed() {
        return MlxState::Ready;
    }
    if !has_uv() {
        return MlxState::Unavailable {
            reason:
                "`uv` is not on PATH; install it (https://docs.astral.sh/uv/) to use the MLX engine"
                    .into(),
        };
    }
    MlxState::Absent
}

/// The explicit download (202 §4): venv with `parakeet-mlx` via `uv`, then the weights via the
/// sidecar's `download` mode, verified against the hub's sha256. Blocking (minutes);
/// `on` hears every state change. One at a time per process.
pub fn install(data_dir: &Path, mut on: impl FnMut(&MlxState)) -> anyhow::Result<()> {
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
    let mut set = |s: MlxState| {
        *INSTALL.lock() = Some(s.clone());
        on(&s);
    };
    set(MlxState::Downloading {
        done_bytes: 0,
        total_bytes: 0,
    });
    let r = install_inner(data_dir, &mut set);
    match &r {
        Ok(()) => {
            *INSTALL.lock() = None;
            on(&MlxState::Ready);
        }
        Err(e) => set(MlxState::Failed {
            reason: format!("{e:#}"),
        }),
    }
    r
}

fn install_inner(data_dir: &Path, set: &mut impl FnMut(MlxState)) -> anyhow::Result<()> {
    if let Some(r) = unsupported() {
        anyhow::bail!(r);
    }
    anyhow::ensure!(has_uv(), "`uv` is not on PATH");
    let p = Paths::new(data_dir);
    let run = |cmd: &mut Command, what: &str| -> anyhow::Result<()> {
        let out = cmd.output()?;
        anyhow::ensure!(
            out.status.success(),
            "{what}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
        Ok(())
    };
    if !p.python().is_file() {
        tracing::info!("voice: mlx: creating {}", p.env_dir().display());
        run(
            Command::new("uv")
                .args(["venv", "--quiet", "--python", "3.12"])
                .arg(p.env_dir()),
            "uv venv",
        )?;
    }
    run(
        Command::new("uv")
            .args(["pip", "install", "--quiet", "--python"])
            .arg(p.python())
            .args([PACKAGE, "huggingface_hub"]),
        "uv pip install",
    )?;
    let mut child = Command::new(p.python())
        .arg(p.write_script()?)
        .arg("download")
        .arg(p.model_dir())
        .arg(MLX_REPO)
        .env("HF_HOME", p.env_dir().join("hf-home"))
        .env("PYTHONUNBUFFERED", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let mut error = None;
    for line in BufReader::new(child.stdout.take().expect("piped")).lines() {
        let line = line?;
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if let (Some(d), Some(t)) = (v["done_bytes"].as_u64(), v["total_bytes"].as_u64()) {
            set(MlxState::Downloading {
                done_bytes: d,
                total_bytes: t,
            });
        } else if let Some(e) = v["error"].as_str() {
            error = Some(e.to_owned());
        }
    }
    let status = child.wait()?;
    if let Some(e) = error {
        anyhow::bail!("download: {e}");
    }
    anyhow::ensure!(status.success(), "download exited with {status}");
    anyhow::ensure!(p.installed(), "download finished without a verified model");
    Ok(())
}

/// One sidecar reply.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Reply {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub ms: u64,
    #[serde(default)]
    pub words: Vec<TokenTime>,
}

/// A token with its times in seconds (`AlignedToken`).
#[derive(Clone, Debug, Default, Deserialize)]
pub struct TokenTime {
    pub text: String,
    pub start: f64,
    pub end: f64,
}

#[derive(Debug)]
enum CallError {
    /// No reply in time; the process is presumed hung.
    Timeout,
    /// The process exited or its pipes broke.
    Gone(String),
    /// The sidecar answered `{error}`.
    Reply(String),
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallError::Timeout => f.write_str("sidecar timed out"),
            CallError::Gone(e) => write!(f, "sidecar gone: {e}"),
            CallError::Reply(e) => write!(f, "sidecar: {e}"),
        }
    }
}

/// One running sidecar process; killed on drop.
struct Sidecar {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    next_id: u64,
}

impl Sidecar {
    /// Spawn `cmd` and wait up to `load` for its `ready` line.
    fn spawn(mut cmd: Command, load: Duration) -> anyhow::Result<Sidecar> {
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdin = child.stdin.take().expect("piped");
        let stdout = child.stdout.take().expect("piped");
        let stderr = child.stderr.take().expect("piped");
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                tracing::debug!("voice: mlx sidecar: {line}");
            }
        });
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let mut s = Sidecar {
            child,
            stdin,
            lines,
            next_id: 1,
        };
        let until = Instant::now() + load;
        loop {
            let line = match s
                .lines
                .recv_timeout(until.saturating_duration_since(Instant::now()))
            {
                Ok(l) => l,
                Err(RecvTimeoutError::Timeout) => anyhow::bail!("sidecar not ready in {load:?}"),
                Err(RecvTimeoutError::Disconnected) => {
                    anyhow::bail!("sidecar exited while loading ({:?})", s.child.wait().ok())
                }
            };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            if v["ready"].as_bool() == Some(true) {
                tracing::info!(
                    "voice: mlx sidecar ready ({}, {})",
                    v["backend"].as_str().unwrap_or("?"),
                    v["model"].as_str().unwrap_or("?")
                );
                return Ok(s);
            }
            if let Some(e) = v["error"].as_str() {
                anyhow::bail!("sidecar failed to load: {e}");
            }
        }
    }

    fn request(
        &mut self,
        mut req: serde_json::Value,
        timeout: Duration,
    ) -> Result<Reply, CallError> {
        let id = self.next_id;
        self.next_id += 1;
        req["id"] = id.into();
        let mut line = req.to_string();
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .and_then(|()| self.stdin.flush())
            .map_err(|e| CallError::Gone(e.to_string()))?;
        let until = Instant::now() + timeout;
        loop {
            let line = match self
                .lines
                .recv_timeout(until.saturating_duration_since(Instant::now()))
            {
                Ok(l) => l,
                Err(RecvTimeoutError::Timeout) => return Err(CallError::Timeout),
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(CallError::Gone("stdout closed".into()));
                }
            };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            // A reply to an earlier, abandoned request is skipped.
            if v["id"].as_u64() != Some(id) {
                continue;
            }
            if let Some(e) = v["error"].as_str() {
                return Err(CallError::Reply(e.to_owned()));
            }
            return serde_json::from_value(v).map_err(|e| CallError::Reply(e.to_string()));
        }
    }
}

impl Drop for Sidecar {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Slot {
    proc: Option<Sidecar>,
    last_used: Instant,
}

/// The sidecar's owner: lazy spawn, one restart after a crash, kill on timeout, idle drop.
pub struct Client {
    launch: Box<dyn Fn() -> anyhow::Result<Command> + Send + Sync>,
    load: Duration,
    slot: Mutex<Slot>,
}

impl Client {
    /// `launch` builds the serving command; `idle` zero keeps the process forever.
    pub fn new(
        launch: impl Fn() -> anyhow::Result<Command> + Send + Sync + 'static,
        load: Duration,
        idle: Duration,
    ) -> Arc<Client> {
        let c = Arc::new(Client {
            launch: Box::new(launch),
            load,
            slot: Mutex::new(Slot {
                proc: None,
                last_used: Instant::now(),
            }),
        });
        if !idle.is_zero() {
            let weak: Weak<Client> = Arc::downgrade(&c);
            let tick = (idle / 4).clamp(Duration::from_millis(20), Duration::from_secs(60));
            std::thread::spawn(move || {
                loop {
                    std::thread::sleep(tick);
                    let Some(c) = weak.upgrade() else { return };
                    // Busy (a request or a load in flight) is not idle.
                    let Some(mut s) = c.slot.try_lock() else {
                        continue;
                    };
                    if s.proc.is_some() && s.last_used.elapsed() > idle {
                        s.proc = None;
                        tracing::info!(
                            "voice: mlx sidecar stopped after {} s idle",
                            idle.as_secs()
                        );
                    }
                }
            });
        }
        c
    }

    fn start(&self, slot: &mut Slot) -> anyhow::Result<()> {
        if slot.proc.is_none() {
            slot.proc = Some(Sidecar::spawn((self.launch)()?, self.load)?);
        }
        Ok(())
    }

    /// The process is up.
    pub fn running(&self) -> bool {
        self.slot.lock().proc.is_some()
    }

    /// Spawn now (blocking) unless running.
    pub fn prewarm(&self) -> anyhow::Result<()> {
        let mut s = self.slot.lock();
        self.start(&mut s)?;
        s.last_used = Instant::now();
        Ok(())
    }

    /// Spawn in the background unless running or loading.
    pub fn prewarm_background(self: &Arc<Self>) {
        let c = self.clone();
        std::thread::spawn(move || {
            if let Err(e) = c.prewarm() {
                tracing::warn!("voice: mlx sidecar: {e:#}");
            }
        });
    }

    /// One request; spawns when not running when `spawn`, else fails at once. Waits at most
    /// `timeout` for the reply (and for the slot); a timeout kills the process, a crash
    /// restarts it once and retries.
    pub fn call(
        &self,
        req: serde_json::Value,
        timeout: Duration,
        spawn: bool,
    ) -> anyhow::Result<Reply> {
        let mut s = self
            .slot
            .try_lock_for(timeout)
            .ok_or_else(|| anyhow::anyhow!("mlx sidecar busy"))?;
        if s.proc.is_none() && !spawn {
            anyhow::bail!("mlx sidecar not running yet");
        }
        let mut restarted = false;
        loop {
            self.start(&mut s)?;
            let r = s
                .proc
                .as_mut()
                .expect("started")
                .request(req.clone(), timeout);
            s.last_used = Instant::now();
            match r {
                Ok(r) => return Ok(r),
                Err(CallError::Reply(e)) => anyhow::bail!("mlx sidecar: {e}"),
                Err(CallError::Timeout) => {
                    s.proc = None;
                    anyhow::bail!("mlx sidecar gave no reply in {timeout:?}; killed");
                }
                Err(CallError::Gone(e)) if !restarted => {
                    tracing::warn!("voice: mlx sidecar crashed ({e}); restarting once");
                    s.proc = None;
                    restarted = true;
                }
                Err(e @ CallError::Gone(_)) => {
                    s.proc = None;
                    anyhow::bail!("{e}");
                }
            }
        }
    }
}

/// Little-endian pcm16 of `pcm`, base64.
fn pcm16_b64(pcm: &[f32]) -> String {
    let mut bytes = Vec::with_capacity(pcm.len() * 2);
    for &x in pcm {
        bytes.extend_from_slice(&((x.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// The engine `voice.stt_engine` picks (§5): `None` = ONNX. `mlx` where MLX cannot run falls
/// back to ONNX with a warning; `mlx` not yet downloaded is returned anyway (each call checks
/// [`Mlx::installed`], so a download made while running takes effect) and runs on ONNX until
/// then. Publishes the engine to the live paths and prewarms it when installed.
pub fn select(engine: SttEngine, data_dir: &Path, idle: Duration) -> Option<Arc<Mlx>> {
    let m = match engine {
        SttEngine::Onnx => None,
        SttEngine::Mlx => match unsupported() {
            Some(r) => {
                tracing::warn!("voice: stt_engine mlx: {r}; using the ONNX engine");
                None
            }
            None => {
                let m = Mlx::new(data_dir, idle);
                if m.installed() {
                    tracing::info!("voice: stt_engine mlx: whole-utterance text on MLX");
                    m.prewarm();
                } else {
                    tracing::warn!(
                        "voice: stt_engine mlx: model not downloaded ({}); using the ONNX engine until it is",
                        m.paths.model_dir().display()
                    );
                }
                Some(m)
            }
        },
    };
    set_live(m.clone().map(|m| m as Arc<dyn Whole>));
    m
}

/// A whole-utterance transcriber for live finals (202 §5). Blocking.
pub trait Whole: Send + Sync {
    /// Start loading in the background if not loaded; never blocks.
    fn prewarm(&self) {}
    /// Usable now; `false` (model not downloaded) skips without a warning.
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
/// [`deadline_for`], else the streaming text, with a warning.
pub fn final_text(whole: Option<&dyn Whole>, audio: &[f32], streaming: String) -> String {
    let Some(w) = whole.filter(|w| w.ready()) else {
        return streaming;
    };
    let deadline = deadline_for(audio.len());
    let t = Instant::now();
    match w.transcribe(audio, deadline) {
        Ok(text) if t.elapsed() <= deadline => {
            tracing::debug!(
                "voice: mlx final in {:?} for {} ms of audio",
                t.elapsed(),
                audio.len() * 1000 / MODEL_RATE
            );
            text.trim().to_owned()
        }
        Ok(_) => {
            tracing::warn!(
                "voice: mlx final took {:?} (deadline {deadline:?}); keeping the streaming text",
                t.elapsed()
            );
            streaming
        }
        Err(e) => {
            tracing::warn!("voice: mlx final failed ({e:#}); keeping the streaming text");
            streaming
        }
    }
}

/// The MLX engine of one data dir.
pub struct Mlx {
    paths: Paths,
    client: Arc<Client>,
}

impl Mlx {
    pub fn new(data_dir: &Path, idle: Duration) -> Arc<Mlx> {
        let paths = Paths::new(data_dir);
        let p = paths.clone();
        Arc::new(Mlx {
            paths,
            client: Client::new(move || p.serve_command(), LOAD_TIMEOUT, idle),
        })
    }

    /// Runnable here and downloaded.
    pub fn installed(&self) -> bool {
        unsupported().is_none() && self.paths.installed()
    }

    /// Start the sidecar in the background when installed.
    pub fn prewarm(&self) {
        if self.installed() && !self.client.running() {
            self.client.prewarm_background();
        }
    }

    /// One piece (≤ [`crate::stt::PIECE`]) for whole clips: spawns as needed and waits.
    pub fn transcribe_piece(&self, pcm16k: &[f32], words: bool) -> anyhow::Result<Piece> {
        anyhow::ensure!(self.installed(), "the MLX model is not downloaded");
        let timeout = Duration::from_secs(30) + deadline_for(pcm16k.len()) * 2;
        let r = self.client.call(request(pcm16k, words), timeout, true)?;
        Ok(Piece {
            text: r.text.trim().to_owned(),
            words: if words {
                join_words(&r.words)
            } else {
                Vec::new()
            },
        })
    }
}

impl Whole for Mlx {
    fn ready(&self) -> bool {
        self.installed()
    }
    fn prewarm(&self) {
        Mlx::prewarm(self);
    }
    /// Never waits for a model load: not yet running → starts it and fails this one.
    fn transcribe(&self, pcm16k: &[f32], deadline: Duration) -> anyhow::Result<String> {
        if !self.client.running() {
            self.client.prewarm_background();
            anyhow::bail!("mlx sidecar still loading");
        }
        Ok(self
            .client
            .call(request(pcm16k, false), deadline, false)?
            .text)
    }
}

fn request(pcm16k: &[f32], words: bool) -> serde_json::Value {
    serde_json::json!({
        "op": "transcribe",
        "pcm16_b64": pcm16_b64(pcm16k),
        "sample_rate": MODEL_RATE,
        "words": words,
    })
}

/// SentencePiece tokens (a leading space starts a word) → words with ms times.
fn join_words(tokens: &[TokenTime]) -> Vec<Word> {
    let ms = |s: f64| (s.max(0.0) * 1000.0).round() as u64;
    let mut out: Vec<Word> = Vec::new();
    for t in tokens {
        let starts = t.text.starts_with(' ') || out.is_empty();
        let text = t.text.trim();
        if text.is_empty() {
            continue;
        }
        match out.last_mut() {
            Some(prev) if !starts => {
                prev.text.push_str(text);
                prev.t1_ms = prev.t1_ms.max(ms(t.end));
            }
            _ => out.push(Word {
                text: text.to_owned(),
                t0_ms: ms(t.start),
                t1_ms: ms(t.end).max(ms(t.start)),
            }),
        }
    }
    out
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

    /// A fake sidecar: `sh -c script`. Request ids count from 1 per process, so `$n` echoes them.
    fn fake(script: &'static str) -> impl Fn() -> anyhow::Result<Command> + Send + Sync {
        move || {
            let mut c = Command::new("sh");
            c.arg("-c").arg(script);
            Ok(c)
        }
    }

    const READY: &str = r#"echo '{"ready":true,"model":"fake","backend":"sh"}'"#;

    fn req() -> serde_json::Value {
        request(&[0.0; 160], false)
    }

    #[test]
    fn ready_then_ok_and_error_replies() {
        let c = Client::new(
            fake(concat!(
                r#"echo 'loading' >&2; echo '{"ready":true,"model":"fake","backend":"sh"}'; n=0; "#,
                r#"while read -r l; do n=$((n+1)); "#,
                r#"if [ $n = 2 ]; then echo "{\"id\":$n,\"error\":\"bad audio\"}"; "#,
                r#"else echo "noise"; echo "{\"id\":$n,\"text\":\" hello \",\"ms\":7}"; fi; done"#
            )),
            Duration::from_secs(5),
            Duration::ZERO,
        );
        assert!(!c.running());
        let r = c.call(req(), Duration::from_secs(5), true).unwrap();
        assert_eq!((r.text.as_str(), r.ms), (" hello ", 7));
        let e = c.call(req(), Duration::from_secs(5), true).unwrap_err();
        assert!(format!("{e}").contains("bad audio"), "{e}");
        // An error reply keeps the process.
        assert!(c.running());
        assert_eq!(c.call(req(), Duration::from_secs(5), false).unwrap().ms, 7);
    }

    #[test]
    fn not_running_without_spawn_fails_fast() {
        let c = Client::new(fake(READY), Duration::from_secs(5), Duration::ZERO);
        assert!(c.call(req(), Duration::from_secs(1), false).is_err());
        assert!(!c.running());
    }

    #[test]
    fn hung_request_times_out_and_kills() {
        let c = Client::new(
            fake(concat!(
                r#"echo '{"ready":true,"model":"fake","backend":"sh"}'; "#,
                r#"while read -r l; do sleep 3; done"#
            )),
            Duration::from_secs(5),
            Duration::ZERO,
        );
        let t = Instant::now();
        let e = c.call(req(), Duration::from_millis(300), true).unwrap_err();
        assert!(format!("{e}").contains("no reply"), "{e}");
        assert!(t.elapsed() < Duration::from_secs(2));
        assert!(!c.running());
    }

    #[test]
    fn crash_restarts_once() {
        let spawns = Arc::new(AtomicUsize::new(0));
        let n = spawns.clone();
        let c = Client::new(
            move || {
                // First process dies on its first request; the second answers.
                let script = if n.fetch_add(1, Ordering::SeqCst) == 0 {
                    r#"echo '{"ready":true,"model":"f","backend":"sh"}'; read -r l; exit 3"#
                } else {
                    r#"echo '{"ready":true,"model":"f","backend":"sh"}'; while read -r l; do echo '{"id":1,"text":"again","ms":1}'; done"#
                };
                let mut cmd = Command::new("sh");
                cmd.arg("-c").arg(script);
                Ok(cmd)
            },
            Duration::from_secs(5),
            Duration::ZERO,
        );
        assert_eq!(
            c.call(req(), Duration::from_secs(5), true).unwrap().text,
            "again"
        );
        assert_eq!(spawns.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn crash_twice_fails() {
        let c = Client::new(
            fake(r#"echo '{"ready":true,"model":"f","backend":"sh"}'; read -r l; exit 3"#),
            Duration::from_secs(5),
            Duration::ZERO,
        );
        assert!(c.call(req(), Duration::from_secs(5), true).is_err());
        assert!(!c.running());
    }

    #[test]
    fn exit_before_ready_is_a_load_error() {
        let c = Client::new(
            fake("echo boom >&2; exit 1"),
            Duration::from_secs(5),
            Duration::ZERO,
        );
        let e = c.call(req(), Duration::from_secs(5), true).unwrap_err();
        assert!(format!("{e:#}").contains("exited while loading"), "{e:#}");
    }

    #[test]
    fn idle_process_is_dropped() {
        let c = Client::new(
            fake(concat!(
                r#"echo '{"ready":true,"model":"f","backend":"sh"}'; "#,
                r#"while read -r l; do echo '{"id":1,"text":"x","ms":1}'; done"#
            )),
            Duration::from_secs(5),
            Duration::from_millis(100),
        );
        c.prewarm().unwrap();
        assert!(c.running());
        std::thread::sleep(Duration::from_millis(400));
        assert!(!c.running());
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
    fn tokens_join_into_words() {
        let t = |text: &str, start, end| TokenTime {
            text: text.into(),
            start,
            end,
        };
        let w = join_words(&[
            t(" Ship", 0.1, 0.3),
            t(" t", 0.4, 0.5),
            t("onight", 0.5, 0.8),
            t(".", 0.8, 0.9),
        ]);
        let got: Vec<_> = w
            .iter()
            .map(|w| (w.text.as_str(), w.t0_ms, w.t1_ms))
            .collect();
        assert_eq!(got, [("Ship", 100, 300), ("tonight.", 400, 900)]);
    }

    #[test]
    fn engine_selection_defaults_to_onnx() {
        #[derive(Deserialize)]
        struct C {
            #[serde(default)]
            stt_engine: SttEngine,
        }
        let c: C = serde_json::from_str("{}").unwrap();
        assert_eq!(c.stt_engine, SttEngine::Onnx);
        let c: C = serde_json::from_str(r#"{"stt_engine":"mlx"}"#).unwrap();
        assert_eq!(c.stt_engine, SttEngine::Mlx);
        // Not installed under an empty dir: the live engine is skipped, the Transcriber uses ONNX.
        let dir = std::env::temp_dir().join(format!("theuth-mlx-{}", uuid::Uuid::now_v7()));
        let m = Mlx::new(&dir, Duration::ZERO);
        assert!(!m.installed());
        assert!(!Whole::ready(&*m));
        assert!(m.transcribe_piece(&[0.0; 160], false).is_err());
        assert!(matches!(
            state(&dir),
            MlxState::Absent | MlxState::Unavailable { .. }
        ));
    }

    /// The real sidecar on a real dictation clip (`$THEUTH_MLX_DATA_DIR` with the venv and
    /// model installed, e.g. by [`install`]; `$THEUTH_MLX_CLIP` a 16 kHz `.pcm16`).
    #[test]
    #[ignore = "runs the MLX sidecar (~2.5 GB model); set THEUTH_MLX_DATA_DIR and THEUTH_MLX_CLIP"]
    fn real_sidecar_transcribes_a_clip() {
        let data_dir = PathBuf::from(std::env::var("THEUTH_MLX_DATA_DIR").unwrap());
        let clip = std::fs::read(std::env::var("THEUTH_MLX_CLIP").unwrap()).unwrap();
        let pcm: Vec<f32> = clip
            .chunks_exact(2)
            .map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.0)
            .collect();
        if !Paths::new(&data_dir).installed() {
            eprintln!("installing into {}", data_dir.display());
            install(&data_dir, |s| eprintln!("{s:?}")).unwrap();
        }
        let m = Mlx::new(&data_dir, Duration::ZERO);
        let t = Instant::now();
        m.client.prewarm().unwrap();
        eprintln!("sidecar up in {:?}", t.elapsed());
        let t = Instant::now();
        let text = m.transcribe_piece(&pcm, false).unwrap().text;
        eprintln!(
            "mlx: {:?} in {:?} for {:.1} s of audio",
            text,
            t.elapsed(),
            pcm.len() as f64 / 16_000.0
        );
        let t = Instant::now();
        let live = Whole::transcribe(&*m, &pcm, deadline_for(pcm.len())).unwrap();
        eprintln!("mlx (warm, live path): {live:?} in {:?}", t.elapsed());
        assert!(!text.is_empty());
    }
}
