//! The daemon's microphone (54 §0.2): the default input through `flexaudio`,
//! 48 kHz mono `f32`, on its own `std::thread`, chunks over an mpsc. Opened per
//! dictation and dropped with it, so the OS mic indicator is lit only while
//! dictating. No `flexaudio` type leaves here.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

use flexaudio::{Event, OutputFormat, SourceKind, StreamConfig};

/// Capture rate. The dictation task resamples to the models' 16 kHz.
pub const RATE: u32 = 48_000;
/// Samples per chunk: 20 ms at [`RATE`].
const CHUNK: usize = RATE as usize / 50;
/// Poll interval when the device ring is empty (a chunk lands every 20 ms).
const IDLE: Duration = Duration::from_millis(5);

/// One mic delivery: 960 mono samples, or the device's error (sent once, then the channel closes).
pub type Chunk = Result<Vec<f32>, String>;

/// An open microphone. Dropping it stops the thread and closes the device.
pub struct Capture {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Capture {
    /// Open the default input and start capturing. Returns once the device is
    /// running (permission prompt included) or fails with the device's error.
    /// The receiver yields mono `f32` chunks of 960 samples; on a device
    /// failure it yields `Err(message)` once and then closes.
    pub fn open() -> anyhow::Result<(Capture, Receiver<Chunk>)> {
        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        let stop = Arc::new(AtomicBool::new(false));
        let thread = std::thread::Builder::new()
            .name("voice-mic".into())
            .spawn({
                let stop = stop.clone();
                move || run(&stop, &tx, &ready_tx)
            })?;
        let mut cap = Capture {
            stop,
            thread: Some(thread),
        };
        match ready_rx.recv() {
            Ok(Ok(())) => Ok((cap, rx)),
            Ok(Err(e)) => {
                cap.join();
                anyhow::bail!("microphone: {e}")
            }
            Err(_) => {
                cap.join();
                anyhow::bail!("microphone: capture thread exited before starting")
            }
        }
    }

    fn join(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.join();
    }
}

/// The capture thread: open + start the device, then pump chunks until stopped,
/// the receiver is gone, or the device fails.
fn run(stop: &AtomicBool, tx: &Sender<Chunk>, ready: &Sender<Result<(), String>>) {
    let cfg = StreamConfig {
        kind: SourceKind::Mic,
        output: OutputFormat {
            sample_rate: RATE,
            channels: 1,
        },
        ..Default::default()
    };
    let mut stream = match flexaudio::open(cfg).and_then(|mut s| s.start().map(|()| s)) {
        Ok(s) => s,
        Err(e) => {
            let _ = ready.send(Err(e.to_string()));
            return;
        }
    };
    let _ = ready.send(Ok(()));
    let mut buf: Vec<f32> = Vec::with_capacity(CHUNK);
    while !stop.load(Ordering::SeqCst) {
        while let Some(ev) = stream.poll_event() {
            let msg = match ev {
                Event::PermissionDenied => "permission denied".to_owned(),
                Event::DeviceLost => "device lost".to_owned(),
                Event::Error(m) => m,
                // Stalls are retried by flexaudio's watchdog; drops are audible, not fatal.
                _ => continue,
            };
            let _ = tx.send(Err(msg));
            return;
        }
        let Some(chunk) = stream.poll_chunk() else {
            std::thread::sleep(IDLE);
            continue;
        };
        // The output format asks for mono; downmix defensively if a frame is wider.
        let ch = (chunk.data.len() / chunk.frames.max(1)).max(1);
        for frame in chunk.data.chunks_exact(ch) {
            buf.push(frame.iter().sum::<f32>() / ch as f32);
            if buf.len() == CHUNK {
                let full = std::mem::replace(&mut buf, Vec::with_capacity(CHUNK));
                if tx.send(Ok(full)).is_err() {
                    return;
                }
            }
        }
    }
    // `stream` drops here: flexaudio joins its threads and closes the device.
}
