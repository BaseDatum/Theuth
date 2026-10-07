//! Display enumeration and screenshot capture (design doc 27 §3.2). Every
//! function here blocks: call from `spawn_blocking`. xcap enumerates on every
//! platform. macOS captures in-process through ScreenCaptureKit
//! ([`grab_sck`]) and falls back, per call, to the `/usr/sbin/screencapture`
//! shell-out ([`grab_screencapture`]) when SCK fails for any reason (xcap's
//! own `CGWindowListCreateImage` path is deprecated and stalled for 30 s in
//! oh-my-pi). Elsewhere xcap captures too.
//!
//! `THEUTH_CAPTURE_BACKEND` (diagnosis only; read on every grab, no config
//! setting): `sck` = ScreenCaptureKit with no fallback, so its errors surface;
//! `screencapture` = always the shell-out; unset or anything else = SCK first,
//! shell-out fallback. See [`Backend`].

use std::io::Cursor;
use std::path::Path;

use image::imageops::{self, FilterType};
use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};

use crate::frame::{Display, Frame};

/// Attached displays, main first then by `(x, y)`, `index` 1-based in that order.
pub fn displays() -> anyhow::Result<Vec<Display>> {
    Ok(enumerate()?.into_iter().map(|(d, _)| d).collect())
}

pub(crate) fn enumerate() -> anyhow::Result<Vec<(Display, xcap::Monitor)>> {
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
                "screen recording not granted for the app that launched theuth (your terminal, or theuth.app): \
                 System Settings › Privacy & Security › Screen Recording, enable it, then restart theuth",
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
/// is `<= max_edge`. `tmp_dir` receives the `screencapture` fallback's output
/// on macOS (created if missing; files removed after decode).
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

/// One display at its native (physical) resolution, undownscaled: the per-display
/// grab `capture` composites, for `screen_context` (96 §6.4). Permission check
/// and request as [`capture`].
pub(crate) fn capture_display_native(
    d: &Display,
    monitor: &xcap::Monitor,
    tmp_dir: &Path,
) -> Result<RgbaImage, CaptureError> {
    if !screen_recording_granted() {
        request_screen_recording();
        return Err(CaptureError::NoScreenRecording);
    }
    grab(d, monitor, d.index as usize, tmp_dir)
}

/// Which capture path [`grab`] takes on macOS; `THEUTH_CAPTURE_BACKEND` selects
/// it (module header).
#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Backend {
    /// ScreenCaptureKit, then the shell-out when it fails.
    Auto,
    /// ScreenCaptureKit only.
    Sck,
    /// The shell-out only.
    ScreenCapture,
}

/// `THEUTH_CAPTURE_BACKEND` value → [`Backend`]; unknown or unset → `Auto`.
#[cfg(target_os = "macos")]
fn backend(var: Option<&std::ffi::OsStr>) -> Backend {
    match var.and_then(|v| v.to_str()) {
        Some("sck") => Backend::Sck,
        Some("screencapture") => Backend::ScreenCapture,
        _ => Backend::Auto,
    }
}

/// Dispatch on `THEUTH_CAPTURE_BACKEND`, read once per call.
#[cfg(target_os = "macos")]
fn grab(
    d: &Display,
    _monitor: &xcap::Monitor,
    n: usize,
    tmp_dir: &Path,
) -> Result<RgbaImage, CaptureError> {
    match backend(std::env::var_os("THEUTH_CAPTURE_BACKEND").as_deref()) {
        Backend::Sck => grab_sck(d),
        Backend::ScreenCapture => grab_screencapture(d, n, tmp_dir),
        Backend::Auto => grab_auto(d, n, tmp_dir, grab_sck),
    }
}

/// `Auto`: `sck` first; any failure falls back to [`grab_screencapture`] for the
/// same display — except a lost Screen Recording grant, which is
/// `NoScreenRecording` (as the shell-out reports it). The first fallback per
/// process logs at `warn`, later ones at `debug`. `sck` is a parameter so the
/// tests can inject a failure.
#[cfg(target_os = "macos")]
fn grab_auto(
    d: &Display,
    n: usize,
    tmp_dir: &Path,
    sck: fn(&Display) -> Result<RgbaImage, CaptureError>,
) -> Result<RgbaImage, CaptureError> {
    use std::sync::atomic::{AtomicBool, Ordering};
    static FELL_BACK: AtomicBool = AtomicBool::new(false);

    let err = match sck(d) {
        Ok(img) => return Ok(img),
        Err(e) => e,
    };
    if matches!(err, CaptureError::NoScreenRecording) || !screen_recording_granted() {
        return Err(CaptureError::NoScreenRecording);
    }
    if FELL_BACK.swap(true, Ordering::Relaxed) {
        tracing::debug!(display = d.index, error = %err, "ScreenCaptureKit failed, falling back to screencapture");
    } else {
        tracing::warn!(display = d.index, error = %err, "ScreenCaptureKit failed, falling back to screencapture");
    }
    grab_screencapture(d, n, tmp_dir)
}

/// `screencapture -x -R x,y,w,h <tmp>/<pid>-<n>.png` with a 5 s deadline, decoded with `image`.
/// `pub` + hidden only so `examples/capture_bench.rs` can time it.
#[cfg(target_os = "macos")]
#[doc(hidden)]
pub fn grab_screencapture(
    d: &Display,
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

/// One display via ScreenCaptureKit's `SCScreenshotManager`, in-process, no
/// temp file. Physical pixels (`width*scale × height*scale`), cursor hidden,
/// menu bar included — the same frame `screencapture -R` of the whole display
/// gives. Works from any thread (the bridge awaits on Swift's global executor,
/// not a run loop). `pub` + hidden only so `examples/capture_bench.rs` can time it.
///
/// Pixels come from `capture_sample_buffer` → `CVPixelBuffer` (BGRA) → one row
/// copy with a BGRA→RGBA swizzle. Never switch to `capture_image()` +
/// `rgba_data()`: rendering the `CGImage` through CoreGraphics leaves a ~14 MB
/// decode cache per shot in the purgeable malloc zone (volatile; counted in RSS,
/// not in footprint), and neither dropping the image nor an autorelease pool
/// releases it (121 §4.3).
#[cfg(target_os = "macos")]
#[doc(hidden)]
pub fn grab_sck(d: &Display) -> Result<RgbaImage, CaptureError> {
    use screencapturekit::cm::CMSampleBufferExt;
    use screencapturekit::screenshot_manager::SCScreenshotManager;
    use screencapturekit::stream::configuration::SCStreamConfiguration;
    use screencapturekit::stream::content_filter::SCContentFilter;

    let sck = |e: screencapturekit::error::SCError| {
        if screen_recording_granted() {
            other(e)
        } else {
            CaptureError::NoScreenRecording
        }
    };
    let content = sck_shareable_content().map_err(sck)?;
    let disp = content
        .displays()
        .into_iter()
        .find(|s| s.display_id() == d.id)
        .ok_or_else(|| {
            other(anyhow::anyhow!(
                "display id {} not in SCShareableContent",
                d.id
            ))
        })?;
    let filter = SCContentFilter::create()
        .with_display(&disp)
        .with_excluding_windows(&[])
        .build()
        .map_err(sck)?;
    let (w, h) = sck_pixel_size(d);
    let config = SCStreamConfiguration::new()
        .with_width(w)
        .with_height(h)
        .with_shows_cursor(false);
    // Sample buffer, not `capture_image()` + `rgba_data()` — see the doc comment (121 §4.3).
    let sb = SCScreenshotManager::capture_sample_buffer(&filter, &config).map_err(sck)?;
    let pb = sb.pixel_buffer().ok_or_else(|| {
        other(anyhow::anyhow!(
            "screenshot sample buffer has no pixel buffer"
        ))
    })?;
    if pb.pixel_format() != u32::from_be_bytes(*b"BGRA") {
        return Err(other(anyhow::anyhow!(
            "screenshot pixel format {:#x} is not BGRA",
            pb.pixel_format()
        )));
    }
    let lock = pb.lock_read_only().map_err(|status| {
        other(anyhow::anyhow!(
            "CVPixelBufferLockBaseAddress failed: {status}"
        ))
    })?;
    let (pw, ph, stride) = (lock.width(), lock.height(), lock.bytes_per_row());
    let row_len = pw * 4;
    if stride < row_len || lock.data_size() < stride * ph {
        return Err(other(anyhow::anyhow!(
            "pixel buffer {pw}x{ph} stride {stride} / size {} too small",
            lock.data_size()
        )));
    }
    let base = lock.base_address();
    let mut out = vec![0u8; row_len * ph];
    for (y, dst_row) in out.chunks_exact_mut(row_len).enumerate() {
        // SAFETY: the lock keeps the mapping valid; `y < ph` and `stride >= row_len`
        // with `stride * ph <= data_size`, checked above.
        let src_row = unsafe { std::slice::from_raw_parts(base.add(y * stride), row_len) };
        let (dst_px, _) = dst_row.as_chunks_mut::<4>();
        let (src_px, _) = src_row.as_chunks::<4>();
        for (dst, &[b, g, r, a]) in dst_px.iter_mut().zip(src_px) {
            *dst = [r, g, b, a];
        }
    }
    drop(lock);
    RgbaImage::from_raw(pw as u32, ph as u32, out).ok_or_else(|| {
        other(anyhow::anyhow!(
            "pixel buffer {pw}x{ph} RGBA length mismatch"
        ))
    })
}

/// The display list SCK needs for a display filter. `SCShareableContent::get()`
/// also enumerates every window; the on-screen-only variant is what the bench
/// (`examples/capture_bench.rs`) measured as cheaper.
#[cfg(target_os = "macos")]
pub(crate) fn sck_shareable_content()
-> Result<screencapturekit::shareable_content::SCShareableContent, screencapturekit::error::SCError>
{
    screencapturekit::shareable_content::SCShareableContent::create()
        .with_on_screen_windows_only(true)
        .with_exclude_desktop_windows(true)
        .get()
}

/// Physical pixel size of `d`: what `screencapture -R` of the full display writes.
#[cfg(target_os = "macos")]
pub(crate) fn sck_pixel_size(d: &Display) -> (u32, u32) {
    (
        (d.width as f32 * d.scale).round() as u32,
        (d.height as f32 * d.scale).round() as u32,
    )
}

#[cfg(all(test, target_os = "macos"))]
mod sck_tests {
    use super::*;

    #[test]
    fn backend_from_env_value() {
        use std::ffi::OsStr;
        assert_eq!(backend(None), Backend::Auto);
        assert_eq!(backend(Some(OsStr::new("sck"))), Backend::Sck);
        assert_eq!(
            backend(Some(OsStr::new("screencapture"))),
            Backend::ScreenCapture
        );
        assert_eq!(backend(Some(OsStr::new("SCK"))), Backend::Auto);
        assert_eq!(backend(Some(OsStr::new(""))), Backend::Auto);
        assert_eq!(backend(Some(OsStr::new("garbage"))), Backend::Auto);
    }

    /// SCK from a plain `std::thread` with no run loop. Skips (prints) when
    /// Screen Recording is not granted, so CI stays green.
    #[test]
    fn grab_sck_works_off_main_thread() {
        if !screen_recording_granted() {
            eprintln!("skipped: Screen Recording not granted");
            return;
        }
        let d = displays().unwrap().into_iter().next().unwrap();
        let want = sck_pixel_size(&d);
        let img = std::thread::spawn(move || grab_sck(&d))
            .join()
            .unwrap()
            .unwrap();
        assert_eq!(img.dimensions(), want);
    }

    /// `Auto` with a failing SCK step still yields the display, via the shell-out.
    /// Skips like the test above.
    #[test]
    fn auto_falls_back_to_screencapture_when_sck_fails() {
        if !screen_recording_granted() {
            eprintln!("skipped: Screen Recording not granted");
            return;
        }
        fn failing(_: &Display) -> Result<RgbaImage, CaptureError> {
            Err(other(anyhow::anyhow!("injected SCK failure")))
        }
        let d = displays().unwrap().into_iter().next().unwrap();
        let tmp = std::env::temp_dir().join("computer-fallback-test");
        let img = grab_auto(&d, 0, &tmp, failing).unwrap();
        let _ = std::fs::remove_dir_all(&tmp);
        assert_eq!(img.dimensions(), sck_pixel_size(&d));
    }

    /// `Auto` never falls back when the grant is gone: the policy reports the
    /// permission, not the shell-out's exit status.
    #[test]
    fn auto_reports_lost_grant_instead_of_falling_back() {
        fn denied(_: &Display) -> Result<RgbaImage, CaptureError> {
            Err(CaptureError::NoScreenRecording)
        }
        let d = Display {
            index: 1,
            id: 0,
            name: String::new(),
            main: true,
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            scale: 1.0,
        };
        let tmp = std::env::temp_dir().join("computer-fallback-test-denied");
        assert!(matches!(
            grab_auto(&d, 0, &tmp, denied),
            Err(CaptureError::NoScreenRecording)
        ));
    }
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
