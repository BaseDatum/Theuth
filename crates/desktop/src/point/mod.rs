//! 96 §4: the voice key. The Rust shell owns the global chord (Ctrl+S by default), the gesture
//! clock ([`machine`]), a 60 Hz pointer sampler, and one transparent click-through overlay window
//! per display; the island webview (`/island`) and the overlays (`/overlay?display=<n>`) hear
//! `point:*` events and draw. OS integration only (25 §0.4) — what shows is the webview's.

mod machine;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as backend;
#[cfg(not(target_os = "macos"))]
mod other;
#[cfg(not(target_os = "macos"))]
use other as backend;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Context;
use serde::Serialize;
use tauri::async_runtime::JoinHandle;
use tauri::{
    AppHandle, Emitter, Listener, LogicalPosition, LogicalSize, Manager, WebviewUrl,
    WebviewWindowBuilder,
};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutEvent, ShortcutState};
use tokio::sync::mpsc;

use machine::{Effect, Event, State};

/// Label prefix of the per-display trail windows (`overlay-<n>`, `n` = `available_monitors()`
/// index); `capabilities/default.json` grants `overlay-*`.
pub const OVERLAY_PREFIX: &str = "overlay-";
const ISLAND: &str = "island";
const MAIN: &str = "main";
/// Esc cancels a gesture; registered only between `Start` and the next `Idle`/`Replying` (53 §0.10).
const ESC: &str = "Escape";
/// The path stops growing after two minutes (60 Hz × 120 s ≈ 86 KB as JSON); the overlay keeps
/// drawing.
const MAX_SAMPLES: usize = machine::SAMPLE_HZ as usize * 120;

/// Managed state: the driver's inbox and what the commands read or set.
pub struct Point {
    tx: mpsc::UnboundedSender<Event>,
    state: Mutex<State>,
    refused: Mutex<Option<String>>,
    chord: Mutex<Option<Shortcut>>,
}

/// Spawn the driver and follow display changes. Runs in `setup`.
pub fn init(app: &AppHandle) -> anyhow::Result<()> {
    let (tx, rx) = mpsc::unbounded_channel();
    app.manage(Point {
        tx,
        state: Mutex::new(State::Idle),
        refused: Mutex::new(None),
        chord: Mutex::new(None),
    });
    tauri::async_runtime::spawn(drive(app.clone(), rx));
    // The island's screen observer (`island/macos.rs`) emits this; refit the overlays once any exist
    // (none yet → `Start` builds them against the current displays anyway).
    let handle = app.clone();
    app.listen_any("screens-changed", move |_| {
        let app = handle.clone();
        if overlay_labels(&app).is_empty() {
            return;
        }
        tauri::async_runtime::spawn(async move {
            if let Err(e) = on_main(&app, overlay_ensure_on_main).await {
                tracing::warn!(error = %e, "overlay refit on screens-changed");
            }
        });
    });
    Ok(())
}

// ---- commands (§4.3) ----

/// Bind the voice key: parse `chord` with the plugin's `Shortcut` parser, register it, then drop the
/// previous chord. An empty chord unregisters. On any error the previous binding stays.
#[tauri::command]
pub fn voice_key_bind(
    app: AppHandle,
    point: tauri::State<'_, Point>,
    chord: String,
) -> Result<(), String> {
    let next = match chord.trim() {
        "" => None,
        c => Some(c.parse::<Shortcut>().map_err(|e| e.to_string())?),
    };
    let shortcuts = app.global_shortcut();
    let mut current = point.chord.lock().unwrap();
    if next == *current {
        return Ok(());
    }
    if let Some(sc) = next {
        let tx = point.tx.clone();
        shortcuts
            .on_shortcut(sc, move |_, _, ev: ShortcutEvent| {
                let e = match ev.state() {
                    ShortcutState::Pressed => Event::Pressed,
                    ShortcutState::Released => Event::Released,
                };
                let _ = tx.send(e);
            })
            .map_err(|e| e.to_string())?;
    }
    if let Some(old) = current.take()
        && let Err(e) = shortcuts.unregister(old)
    {
        tracing::warn!(error = %e, "unregister previous voice key");
    }
    *current = next;
    Ok(())
}

/// The panel cancels (Esc inside it, click outside, "Stop").
#[tauri::command]
pub fn point_cancel(point: tauri::State<'_, Point>) {
    let _ = point.tx.send(Event::Cancel);
}

/// The panel finished a phase (send resolved, `DONE_MS` after speech, or a failure).
#[tauri::command]
pub fn point_done(point: tauri::State<'_, Point>) {
    let _ = point.tx.send(Event::PanelDone);
}

/// The current machine state, for a panel webview that boots mid-gesture.
#[tauri::command]
pub fn point_state(point: tauri::State<'_, Point>) -> State {
    *point.state.lock().unwrap()
}

/// While set, a press from `Idle`/`Replying` answers `point:refused { reason }` instead of starting.
#[tauri::command]
pub fn point_refuse(point: tauri::State<'_, Point>, reason: Option<String>) {
    *point.refused.lock().unwrap() = reason;
}

/// Create a missing `overlay-<n>` per display (concealed) and fit existing ones; the labels.
#[tauri::command]
pub async fn overlay_ensure(app: AppHandle) -> Result<Vec<String>, String> {
    on_main(&app, overlay_ensure_on_main)
        .await
        .map_err(|e| format!("{e:#}"))
}

// ---- event payloads (§4.4; mirrored by hand in `lib/tauri.ts`) ----

#[derive(Serialize)]
struct StartPayload {
    at_ms: u64,
    locked: bool,
    color_hint: Option<String>,
}

#[derive(Serialize)]
struct LockPayload {}

#[derive(Serialize)]
struct SamplesPayload {
    display_index: u32,
    pts: Vec<[f32; 2]>,
}

/// One pointer sample in logical global points, `t_ms` from the press.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Sample {
    t_ms: u32,
    x: f32,
    y: f32,
}

#[derive(Serialize)]
struct StopPayload {
    samples: Vec<Sample>,
    duration_ms: u64,
    reason: &'static str,
}

#[derive(Serialize)]
struct CancelPayload {
    reason: &'static str,
    hint: Option<&'static str>,
}

#[derive(Serialize)]
struct RefusedPayload {
    reason: String,
}

// ---- driver ----

struct Driver {
    app: AppHandle,
    epoch: Instant,
    state: State,
    /// The press that opened the current gesture, ms since `epoch`.
    press_at: u64,
    path: Arc<Mutex<Vec<Sample>>>,
    sampler: Option<JoinHandle<()>>,
}

async fn drive(app: AppHandle, mut rx: mpsc::UnboundedReceiver<Event>) {
    let mut d = Driver {
        app,
        epoch: Instant::now(),
        state: State::Idle,
        press_at: 0,
        path: Arc::default(),
        sampler: None,
    };
    while let Some(e) = rx.recv().await {
        let now = d.now();
        let point = d.app.state::<Point>();
        let refused = point.refused.lock().unwrap().clone();
        let (next, effects) = machine::step(d.state, e, now, refused.as_deref());
        let prev = d.state;
        d.state = next;
        *point.state.lock().unwrap() = next;
        for fx in effects {
            d.run(fx, now).await;
        }
        d.after(prev, next);
    }
}

impl Driver {
    fn now(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    async fn run(&mut self, fx: Effect, now: u64) {
        match fx {
            Effect::Start => {
                self.press_at = now;
                self.path.lock().unwrap().clear();
                if let Err(e) = on_main(&self.app, |app| {
                    overlay_ensure_on_main(app)?;
                    overlays_set_revealed(app, true)
                })
                .await
                {
                    tracing::warn!(error = %e, "reveal overlays");
                }
                self.emit(
                    island_and_overlays(&self.app),
                    "point:start",
                    &StartPayload {
                        at_ms: now,
                        locked: false,
                        color_hint: None,
                    },
                );
                self.register_esc();
                self.start_sampler();
            }
            Effect::ShowLock => self.emit([ISLAND.to_owned()], "point:lock", &LockPayload {}),
            Effect::Stop { reason } => {
                self.stop_sampler();
                // Concealed on the main thread and awaited before the panel hears `point:stop`, so
                // its `screen_context` capture can never see the trail (§4.5).
                self.conceal().await;
                let samples = std::mem::take(&mut *self.path.lock().unwrap());
                self.emit(
                    [ISLAND.to_owned()],
                    "point:stop",
                    &StopPayload {
                        samples,
                        duration_ms: now.saturating_sub(self.press_at),
                        reason,
                    },
                );
            }
            Effect::Discard { reason, hint } => {
                self.stop_sampler();
                self.conceal().await;
                self.path.lock().unwrap().clear();
                self.emit(
                    island_and_overlays(&self.app),
                    "point:cancel",
                    &CancelPayload { reason, hint },
                );
            }
            Effect::Conceal => self.conceal().await,
            Effect::Refuse { reason } => self.emit(
                [ISLAND.to_owned(), MAIN.to_owned()],
                "point:refused",
                &RefusedPayload { reason },
            ),
        }
    }

    /// Timers and the Esc lifetime follow the state, not the effects: every entry into
    /// `Idle`/`Replying` drops Esc, whatever took us there.
    fn after(&self, prev: State, next: State) {
        match next {
            State::Armed { t0 } => self.arm_tick(t0 + machine::HOLD_MS),
            State::Tap1 { released, .. } => self.arm_tick(released + machine::DOUBLE_TAP_MS + 1),
            State::Idle | State::Replying if !matches!(prev, State::Idle | State::Replying) => {
                self.unregister_esc();
            }
            _ => {}
        }
    }

    fn arm_tick(&self, at: u64) {
        let tx = self.app.state::<Point>().tx.clone();
        let deadline = self.epoch + Duration::from_millis(at);
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep_until(deadline.into()).await;
            let _ = tx.send(Event::Tick { now: at });
        });
    }

    fn register_esc(&self) {
        let shortcuts = self.app.global_shortcut();
        if shortcuts.is_registered(ESC) {
            return;
        }
        let tx = self.app.state::<Point>().tx.clone();
        let r = shortcuts.on_shortcut(ESC, move |_, _, ev: ShortcutEvent| {
            if ev.state() == ShortcutState::Pressed {
                let _ = tx.send(Event::Esc);
            }
        });
        if let Err(e) = r {
            tracing::warn!(error = %e, "register Esc for the voice key");
        }
    }

    fn unregister_esc(&self) {
        let shortcuts = self.app.global_shortcut();
        if shortcuts.is_registered(ESC)
            && let Err(e) = shortcuts.unregister(ESC)
        {
            tracing::warn!(error = %e, "unregister Esc");
        }
    }

    fn start_sampler(&mut self) {
        self.stop_sampler();
        let app = self.app.clone();
        let path = Arc::clone(&self.path);
        let epoch = self.epoch;
        let t0 = self.press_at;
        self.sampler = Some(tauri::async_runtime::spawn(sample(app, path, epoch, t0)));
    }

    fn stop_sampler(&mut self) {
        if let Some(h) = self.sampler.take() {
            h.abort();
        }
    }

    async fn conceal(&self) {
        if let Err(e) = on_main(&self.app, |app| overlays_set_revealed(app, false)).await {
            tracing::warn!(error = %e, "conceal overlays");
        }
    }

    fn emit<S: Serialize>(
        &self,
        labels: impl IntoIterator<Item = String>,
        name: &str,
        payload: &S,
    ) {
        for label in labels {
            if let Err(e) = self.app.emit_to(label.as_str(), name, payload) {
                tracing::warn!(error = %e, label, "emit {name}");
            }
        }
    }
}

// ---- sampler (§4.2) ----

/// A monitor as `available_monitors()` reports it: physical origin and size, and its scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MonitorRect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub scale: f64,
}

/// Where a physical cursor position landed: its display index, the logical global point, and the
/// same point local to that display (the overlay's canvas coordinates).
#[derive(Debug, PartialEq)]
pub struct Hit {
    pub display: u32,
    pub x: f32,
    pub y: f32,
    pub local: [f32; 2],
}

/// Physical global cursor → the containing monitor and its logical points. `None` when no monitor
/// holds the point (a seam, or the cursor on a display that just vanished) — that tick is skipped.
pub fn locate(px: f64, py: f64, monitors: &[MonitorRect]) -> Option<Hit> {
    let (n, m) = monitors.iter().enumerate().find(|(_, m)| {
        let (mx, my) = (f64::from(m.x), f64::from(m.y));
        px >= mx && px < mx + f64::from(m.w) && py >= my && py < my + f64::from(m.h)
    })?;
    // [INFERENCE; spike 2] Tauri reports `Monitor::position()` in physical pixels on every OS and the
    // display's logical origin is that position over its own scale factor (macOS: tao multiplies the
    // NSScreen frame by `backingScaleFactor`). A mixed-DPI seam is where this is checked.
    let local_x = (px - f64::from(m.x)) / m.scale;
    let local_y = (py - f64::from(m.y)) / m.scale;
    let (lx, ly) = (f64::from(m.x) / m.scale, f64::from(m.y) / m.scale);
    Some(Hit {
        display: u32::try_from(n).unwrap_or(u32::MAX),
        x: (local_x + lx) as f32,
        y: (local_y + ly) as f32,
        local: [local_x as f32, local_y as f32],
    })
}

fn monitor_rects(app: &AppHandle) -> Vec<MonitorRect> {
    app.available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|m| {
            let p = m.position();
            let s = m.size();
            MonitorRect {
                x: p.x,
                y: p.y,
                w: s.width,
                h: s.height,
                scale: m.scale_factor(),
            }
        })
        .collect()
}

async fn sample(app: AppHandle, path: Arc<Mutex<Vec<Sample>>>, epoch: Instant, t0: u64) {
    // ponytail: the monitor set is read once per gesture; a display plugged in mid-hold gets its
    // overlay on the next press.
    let monitors = monitor_rects(&app);
    let mut ticks = tokio::time::interval(Duration::from_micros(1_000_000 / machine::SAMPLE_HZ));
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticks.tick().await;
        let Ok(c) = app.cursor_position() else {
            continue;
        };
        let Some(hit) = locate(c.x, c.y, &monitors) else {
            continue;
        };
        let t_ms = u64::try_from(epoch.elapsed().as_millis())
            .unwrap_or(u64::MAX)
            .saturating_sub(t0);
        let t_ms = u32::try_from(t_ms).unwrap_or(u32::MAX);
        {
            let mut p = path.lock().unwrap();
            if p.len() < MAX_SAMPLES {
                p.push(Sample {
                    t_ms,
                    x: hit.x,
                    y: hit.y,
                });
            }
        }
        let label = format!("{OVERLAY_PREFIX}{}", hit.display);
        if let Err(e) = app.emit_to(
            label.as_str(),
            "point:samples",
            &SamplesPayload {
                display_index: hit.display,
                pts: vec![hit.local],
            },
        ) {
            tracing::warn!(error = %e, label, "emit point:samples");
        }
    }
}

// ---- overlay windows (§4.5) ----

fn overlay_labels(app: &AppHandle) -> Vec<String> {
    let mut v: Vec<String> = app
        .webview_windows()
        .into_keys()
        .filter(|l| l.starts_with(OVERLAY_PREFIX))
        .collect();
    v.sort();
    v
}

fn island_and_overlays(app: &AppHandle) -> Vec<String> {
    let mut v = overlay_labels(app);
    v.insert(0, ISLAND.to_owned());
    v
}

/// Run `f` on the main thread and wait for its answer. Callers are tokio tasks and async commands,
/// never the main thread itself.
async fn on_main<T: Send + 'static>(
    app: &AppHandle,
    f: impl FnOnce(&AppHandle) -> anyhow::Result<T> + Send + 'static,
) -> anyhow::Result<T> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let handle = app.clone();
    app.run_on_main_thread(move || {
        let _ = tx.send(f(&handle));
    })
    .context("run_on_main_thread")?;
    rx.await.context("main-thread task dropped")?
}

/// One concealed `overlay-<n>` per display: build the missing ones, refit the rest to their
/// monitor, conceal those whose display vanished (never destroyed). Main thread.
fn overlay_ensure_on_main(app: &AppHandle) -> anyhow::Result<Vec<String>> {
    let monitors = app.available_monitors().context("available_monitors")?;
    let mut labels = Vec::with_capacity(monitors.len());
    for (n, m) in monitors.iter().enumerate() {
        let label = format!("{OVERLAY_PREFIX}{n}");
        let scale = m.scale_factor();
        let pos: LogicalPosition<f64> = m.position().to_logical(scale);
        let size: LogicalSize<f64> = m.size().to_logical(scale);
        match app.get_webview_window(&label) {
            Some(win) => {
                win.set_position(pos)
                    .with_context(|| format!("position `{label}`"))?;
                win.set_size(size)
                    .with_context(|| format!("size `{label}`"))?;
            }
            None => {
                // Routes are path-based like the pop-outs' (`open_window`); the island's label picks
                // `/island` the same way.
                let url = WebviewUrl::App(format!("/overlay?display={n}").into());
                let win = WebviewWindowBuilder::new(app, &label, url)
                    .transparent(true)
                    .decorations(false)
                    .shadow(false)
                    .focused(false)
                    .skip_taskbar(true)
                    .always_on_top(true)
                    .visible(false)
                    .position(pos.x, pos.y)
                    .inner_size(size.width, size.height)
                    .disable_drag_drop_handler()
                    .build()
                    .with_context(|| format!("build `{label}`"))?;
                win.set_ignore_cursor_events(true)
                    .with_context(|| format!("ignore cursor events on `{label}`"))?;
                // Capture exclusion: `island::set_hidden` keeps no value, so the island's boot
                // default applies; the webview re-applies its stored eye to every window.
                if let Err(e) = win.set_content_protected(true) {
                    tracing::warn!(error = %e, label, "set_content_protected");
                }
                backend::panelize(&win).with_context(|| format!("panelize `{label}`"))?;
            }
        }
        labels.push(label);
    }
    for (label, win) in app.webview_windows() {
        if let Some(n) = label
            .strip_prefix(OVERLAY_PREFIX)
            .and_then(|s| s.parse::<usize>().ok())
            && n >= monitors.len()
            && let Err(e) = backend::set_revealed(&win, false)
        {
            tracing::warn!(error = %e, label, "conceal overlay of a vanished display");
        }
    }
    Ok(labels)
}

/// Alpha 1 / alpha 0 on every overlay (macOS); show / hide elsewhere. Click-through always. Main thread.
fn overlays_set_revealed(app: &AppHandle, on: bool) -> anyhow::Result<()> {
    for (label, win) in app.webview_windows() {
        if label.starts_with(OVERLAY_PREFIX) {
            backend::set_revealed(&win, on).with_context(|| format!("reveal `{label}` = {on}"))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chords_parse_with_the_plugin_parser() {
        for chord in ["Control+S", "Control+Shift+S", "Alt+Space", ESC] {
            assert!(chord.parse::<Shortcut>().is_ok(), "{chord}");
        }
        assert!("Control+".parse::<Shortcut>().is_err());
    }

    #[test]
    fn locate_maps_physical_cursor_to_display_local_and_global_logical() {
        let monitors = [
            MonitorRect {
                x: 0,
                y: 0,
                w: 2880,
                h: 1800,
                scale: 2.0,
            },
            MonitorRect {
                x: 2880,
                y: 0,
                w: 1920,
                h: 1080,
                scale: 1.0,
            },
        ];
        assert_eq!(
            locate(100.0, 200.0, &monitors),
            Some(Hit {
                display: 0,
                x: 50.0,
                y: 100.0,
                local: [50.0, 100.0]
            })
        );
        assert_eq!(
            locate(3000.0, 100.0, &monitors),
            Some(Hit {
                display: 1,
                x: 3000.0,
                y: 100.0,
                local: [120.0, 100.0]
            })
        );
        // Right/bottom edges are exclusive; off every monitor is a skipped tick.
        assert_eq!(locate(2880.0, 0.0, &monitors).map(|h| h.display), Some(1));
        assert_eq!(locate(-1.0, 0.0, &monitors), None);
        assert_eq!(locate(4800.0, 10.0, &monitors), None);
    }

    #[test]
    fn samples_payload_shape() {
        let s = StopPayload {
            samples: vec![Sample {
                t_ms: 16,
                x: 1.5,
                y: 2.0,
            }],
            duration_ms: 1200,
            reason: "release",
        };
        assert_eq!(
            serde_json::to_string(&s).unwrap(),
            r#"{"samples":[{"t_ms":16,"x":1.5,"y":2.0}],"duration_ms":1200,"reason":"release"}"#
        );
        let p = SamplesPayload {
            display_index: 1,
            pts: vec![[120.0, 100.0]],
        };
        assert_eq!(
            serde_json::to_string(&p).unwrap(),
            r#"{"display_index":1,"pts":[[120.0,100.0]]}"#
        );
    }
}
