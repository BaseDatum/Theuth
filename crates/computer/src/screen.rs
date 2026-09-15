//! Display enumeration and screenshot capture (design doc 27 §3.2). Every
//! function here blocks: call from `spawn_blocking`. xcap enumerates on every
//! platform; macOS captures with `/usr/sbin/screencapture` (xcap's
//! `CGWindowListCreateImage` path is deprecated and stalled for 30 s in
//! oh-my-pi), elsewhere xcap captures too.

use std::io::Cursor;
use std::path::Path;

use image::imageops::{self, FilterType};
use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};

use crate::frame::{Display, Frame};

/// Attached displays, main first then by `(x, y)`, `index` 1-based in that order.
pub fn displays() -> anyhow::Result<Vec<Display>> {
    Ok(enumerate()?.into_iter().map(|(d, _)| d).collect())
}

fn enumerate() -> anyhow::Result<Vec<(Display, xcap::Monitor)>> {
    let mut out = xcap::Monitor::all()?
        .into_iter()
        .map(|m| -> anyhow::Result<(Display, xcap::Monitor)> {
            let d = Display {
                index: 0,
                id: m.id()?,
                name: m.name()?,
                main: m.is_primary()?,
                x: m.x()?,
                y: m.y()?,
                width: m.width()?,
                height: m.height()?,
                scale: m.scale_factor()?,
            };
            Ok((d, m))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    out.sort_by_key(|(d, _)| (!d.main, d.x, d.y));
    for (i, (d, _)) in out.iter_mut().enumerate() {
        d.index = i as u32 + 1;
    }
    Ok(out)
}

/// Whether this process may capture the screen. macOS: `CGPreflightScreenCaptureAccess`
/// (non-prompting; "never asked" reads as `false`). Elsewhere always `true`.
pub fn screen_recording_granted() -> bool {
    #[cfg(target_os = "macos")]
    {
        objc2_core_graphics::CGPreflightScreenCaptureAccess()
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

/// Fire the system Screen Recording prompt, at most once per process. No-op off macOS.
pub fn request_screen_recording() {
    #[cfg(target_os = "macos")]
    {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            objc2_core_graphics::CGRequestScreenCaptureAccess();
        });
    }
}

/// One screenshot: the PNG bytes and the map from its pixels to the desktop.
pub struct Shot {
    /// PNG-encoded composite.
    pub png: Vec<u8>,
    /// Pixel → logical-point mapping for the pointer tools.
    pub frame: Frame,
}

/// Why [`capture`] failed; `Display` gives the texts of 27 §4.
#[derive(Debug)]
pub enum CaptureError {
    /// Screen Recording not granted to the responsible process.
    NoScreenRecording,
    /// `display: N` named an index that is not attached.
    NoSuchDisplay {
        /// The index asked for.
        wanted: u32,
        /// How many displays there are.
        have: usize,
    },
    /// Enumeration, capture, decode or encode failed.
    Other(anyhow::Error),
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoScreenRecording => f.write_str(
                "screen recording not granted for the app that launched wth (your terminal, or wth.app): \
                 System Settings › Privacy & Security › Screen Recording, enable it, then restart wth",
            ),
            Self::NoSuchDisplay { wanted, have } => {
                write!(f, "display {wanted} does not exist ({have} displays)")
            }
            Self::Other(e) => write!(f, "{e:#}"),
        }
    }
}

impl std::error::Error for CaptureError {}

fn other(e: impl Into<anyhow::Error>) -> CaptureError {
    CaptureError::Other(e.into())
}

/// Capture `display` (1-based index; `None` = every display composited at
/// logical scale, black where no display is) and downscale so the long edge
/// is `<= max_edge`. `tmp_dir` receives `screencapture` output on macOS
/// (created if missing; files removed after decode).
pub fn capture(display: Option<u32>, max_edge: u32, tmp_dir: &Path) -> Result<Shot, CaptureError> {
    if !screen_recording_granted() {
        request_screen_recording();
        return Err(CaptureError::NoScreenRecording);
    }
    let all = enumerate().map_err(other)?;
    let have = all.len();
    let selected = match display {
        None => all,
        Some(wanted) => vec![
            all.into_iter()
                .find(|(d, _)| d.index == wanted)
                .ok_or(CaptureError::NoSuchDisplay { wanted, have })?,
        ],
    };
    if selected.is_empty() {
        return Err(other(anyhow::anyhow!("no displays attached")));
    }
    let displays: Vec<Display> = selected.iter().map(|(d, _)| d.clone()).collect();
    let frame = Frame::compose(&displays, max_edge);
    let mut canvas = RgbaImage::from_pixel(frame.size.0, frame.size.1, Rgba([0, 0, 0, 255]));
    for (n, ((d, monitor), (_, rect))) in selected.iter().zip(&frame.displays).enumerate() {
        let img = grab(d, monitor, n, tmp_dir)?;
        let img = if img.dimensions() == (rect.w, rect.h) {
            img
        } else {
            imageops::resize(&img, rect.w, rect.h, FilterType::Triangle)
        };
        imageops::replace(&mut canvas, &img, i64::from(rect.x), i64::from(rect.y));
    }
    let mut png = Vec::new();
    DynamicImage::ImageRgba8(canvas)
        .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .map_err(other)?;
    Ok(Shot { png, frame })
}

/// `screencapture -x -R x,y,w,h <tmp>/<pid>-<n>.png` with a 5 s deadline, decoded with `image`.
#[cfg(target_os = "macos")]
fn grab(
    d: &Display,
    _monitor: &xcap::Monitor,
    n: usize,
    tmp_dir: &Path,
) -> Result<RgbaImage, CaptureError> {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    std::fs::create_dir_all(tmp_dir).map_err(other)?;
    let path = tmp_dir.join(format!("{}-{n}.png", std::process::id()));
    let mut child = Command::new("/usr/sbin/screencapture")
        .arg("-x")
        .arg(format!("-R{},{},{},{}", d.x, d.y, d.width, d.height))
        .arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(other)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().map_err(other)? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_file(&path);
            return Err(other(anyhow::anyhow!("screencapture timed out after 5 s")));
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    if !status.success() {
        let _ = std::fs::remove_file(&path);
        return Err(if screen_recording_granted() {
            other(anyhow::anyhow!("screencapture exited with {status}"))
        } else {
            CaptureError::NoScreenRecording
        });
    }
    let img = image::open(&path).map(DynamicImage::into_rgba8);
    let _ = std::fs::remove_file(&path);
    img.map_err(other)
}

/// xcap's own capture; `tmp_dir` is unused off macOS.
#[cfg(not(target_os = "macos"))]
fn grab(
    _d: &Display,
    monitor: &xcap::Monitor,
    _n: usize,
    _tmp_dir: &Path,
) -> Result<RgbaImage, CaptureError> {
    monitor.capture_image().map_err(other)
}
