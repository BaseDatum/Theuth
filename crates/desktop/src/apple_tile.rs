//! The agent's tile operations on macOS and iOS (209 §4.7, D8): the WebKit half of `web_view_call`'s ops,
//! shared by the desktop's child webviews (`webview.rs`) and the iOS shell's raw views (`ios_webview.rs`).
//! The arguments, the script wrapper, the reply and the size arithmetic are pure rules in `web_rules::call`.
//!
//! - [`script`]: `callAsyncJavaScript` of the wrapped body in the main frame, in the isolated
//!   [`AGENT_WORLD`] content world or in `WKContentWorld.page`, with one string argument (the caller's
//!   argument values as JSON). The body answers one JSON string; [`script_done`] waits for it until the
//!   caller's deadline and reads it off the main thread. WebKit cannot cancel a running script: past the
//!   deadline the caller gets the timeout and the script's late answer is dropped.
//! - [`snapshot`]: `takeSnapshot` of the visible view; the main thread only takes the image's `CGImage`
//!   (thread-safe) and [`snapshot_done`] draws it into a bitmap of at most `SNAPSHOT_MAX_SIDE` pixels on the
//!   long side and encodes it as base64 PNG, off the main thread.
//! - [`settle`]: after `navigate`, `back`, `forward` or `reload`, the main frame's state every
//!   `SETTLE_POLL` until it stopped loading or the deadline passed. WebKit's `loading` is set when the
//!   load is requested, so the first look already sees a load that was started.
//!
//! Script results, arguments and thrown messages can carry page data: they go to the caller only, never to
//! `tracing`.

#[cfg(target_os = "macos")]
use std::ptr::null_mut;
use std::sync::mpsc::{Receiver, SyncSender};
use std::time::{Duration, Instant};

use base64::Engine;
use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
#[cfg(target_os = "macos")]
use objc2_app_kit::NSImage;
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo,
    CGInterpolationQuality,
};
use objc2_foundation::{NSDictionary, NSError, NSPoint, NSRect, NSSize, NSString};
#[cfg(target_os = "ios")]
use objc2_ui_kit::UIImage;
use objc2_web_kit::WKContentWorld;
#[cfg(target_os = "macos")]
use objc2_web_kit::WKWebView;
use serde_json::{Value, json};

use crate::apple_cookies::{DROPPED, Late, recv_by};
#[cfg(target_os = "ios")]
use crate::ios_webview::WKWebView;
use crate::web_rules::call::{
    ARGS_NAME, Loaded, SETTLE_POLL, SNAPSHOT_TIMEOUT, Script, World, failed, fit, reply,
    straighten, timed_out,
};
use crate::web_rules::profile::AGENT_WORLD;

/// The `userInfo` key under which WebKit puts a script exception's message.
const EXCEPTION_MESSAGE: &str = "WKJavaScriptExceptionMessage";

/// What [`script`] hands over: the body's JSON string, copied out of WebKit's `NSString` (which may not
/// leave the main thread; `None` when WebKit answered no string), or WebKit's failure of the whole body.
pub(crate) type ScriptReply = Result<Option<String>, String>;

/// A snapshot's viewport in CSS px (`web_rules::call::css_size`), as the caller read it with the image.
pub(crate) type Viewport = (u32, u32);

/// What [`snapshot`] hands over: the snapshot's bitmap and the viewport it shows.
pub(crate) type SnapshotReply = Result<(Retained<CGImage>, Viewport), String>;

/// Start `script` in `view`'s main frame (209 §4.7); its answer goes to `tx` as WebKit gives it.
pub(crate) fn script(
    mtm: MainThreadMarker,
    view: &WKWebView,
    script: &Script,
    tx: SyncSender<ScriptReply>,
) {
    // SAFETY: the page world, or a named world for this app's scripts (WebKit keeps one per name).
    let world = unsafe {
        match script.world {
            World::Isolated => WKContentWorld::worldWithName(&NSString::from_str(AGENT_WORLD), mtm),
            World::Page => WKContentWorld::pageWorld(mtm),
        }
    };
    let name = NSString::from_str(ARGS_NAME);
    let value: Retained<AnyObject> =
        Retained::into_super(Retained::into_super(NSString::from_str(&script.args_json)));
    let arguments = NSDictionary::<NSString, AnyObject>::from_retained_objects(&[&*name], &[value]);
    let done = RcBlock::new(move |result: *mut AnyObject, error: *mut NSError| {
        // SAFETY: WebKit passes live (or nil) objects for the duration of the call.
        let reply = match unsafe { error.as_ref() } {
            Some(error) => Err(failed(&message(error))),
            None => Ok(unsafe { result.as_ref() }
                .and_then(|r| r.downcast_ref::<NSString>())
                .map(|s| s.to_string())),
        };
        let _ = tx.try_send(reply);
    });
    // SAFETY: one string argument the body names; no frame means the main frame; WebKit copies the handler
    // and calls it once, on the main thread.
    unsafe {
        view.callAsyncJavaScript_arguments_inFrame_inContentWorld_completionHandler(
            &NSString::from_str(&script.body),
            Some(&arguments),
            None,
            &world,
            Some(&*done),
        )
    };
}

/// A WebKit error's script exception message, else its description.
fn message(error: &NSError) -> String {
    error
        .userInfo()
        .objectForKey(&NSString::from_str(EXCEPTION_MESSAGE))
        .and_then(|m| m.downcast_ref::<NSString>().map(|s| s.to_string()))
        .unwrap_or_else(|| error.localizedDescription().to_string())
}

/// Wait for [`script`]'s answer until `deadline` (the caller's `timeout` from the call's start) and read it,
/// off the async workers.
pub(crate) async fn script_done(
    rx: Receiver<ScriptReply>,
    deadline: Instant,
    timeout: Duration,
) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || match recv_by(&rx, deadline) {
        Ok(Ok(json)) => reply(json.as_deref()),
        Ok(Err(e)) => Err(e),
        Err(Late::Timeout(_)) => Err(timed_out(timeout)),
        Err(Late::Dropped) => Err(DROPPED.to_owned()),
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Look at the main frame's state with `read` every [`SETTLE_POLL`] until it stopped loading or `deadline`
/// passed, and answer it (209 §4.7): `{ url, title, loading }`, `loading: true` at the deadline.
pub(crate) async fn settle<F, Fut>(mut read: F, deadline: Instant) -> Result<Value, String>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<Loaded, String>>,
{
    loop {
        let state = read().await?;
        let left = deadline.saturating_duration_since(Instant::now());
        if !state.loading || left.is_zero() {
            return serde_json::to_value(state).map_err(|e| e.to_string());
        }
        let nap = SETTLE_POLL.min(left);
        tauri::async_runtime::spawn_blocking(move || std::thread::sleep(nap))
            .await
            .map_err(|e| e.to_string())?;
    }
}

/// Start a snapshot of `view`'s visible area, which shows `viewport`; its `CGImage` goes to `tx`.
#[cfg(target_os = "macos")]
pub(crate) fn snapshot(view: &WKWebView, viewport: Viewport, tx: SyncSender<SnapshotReply>) {
    let done = RcBlock::new(move |image: *mut NSImage, error: *mut NSError| {
        // SAFETY: WebKit passes live (or nil) objects for the duration of the call; a null proposed rect
        // asks for the image's own size.
        let reply = match unsafe { (image.as_ref(), error.as_ref()) } {
            (_, Some(error)) => Err(snapshot_failed(error)),
            (Some(image), None) => {
                unsafe { image.CGImageForProposedRect_context_hints(null_mut(), None, None) }
                    .map(|image| (image, viewport))
                    .ok_or_else(|| "snapshot has no bitmap".to_owned())
            }
            (None, None) => Err("snapshot returned no image".to_owned()),
        };
        let _ = tx.try_send(reply);
    });
    // SAFETY: no configuration means the visible bounds; WebKit calls the handler once, on the main thread.
    unsafe { view.takeSnapshotWithConfiguration_completionHandler(None, &done) };
}

/// Start a snapshot of `view`'s visible area, which shows `viewport`; its `CGImage` goes to `tx`.
#[cfg(target_os = "ios")]
pub(crate) fn snapshot(view: &WKWebView, viewport: Viewport, tx: SyncSender<SnapshotReply>) {
    let done = RcBlock::new(move |image: *mut UIImage, error: *mut NSError| {
        // SAFETY: WebKit passes live (or nil) objects for the duration of the call.
        let reply = match unsafe { (image.as_ref(), error.as_ref()) } {
            (_, Some(error)) => Err(snapshot_failed(error)),
            (Some(image), None) => unsafe { image.CGImage() }
                .map(|image| (image, viewport))
                .ok_or_else(|| "snapshot has no bitmap".to_owned()),
            (None, None) => Err("snapshot returned no image".to_owned()),
        };
        let _ = tx.try_send(reply);
    });
    // SAFETY: no configuration means the visible bounds; WebKit calls the handler once, on the main thread.
    unsafe { view.takeSnapshotWithConfiguration_completionHandler(None, &done) };
}

fn snapshot_failed(error: &NSError) -> String {
    format!("snapshot failed: {}", error.localizedDescription())
}

/// Wait for [`snapshot`]'s image until `deadline`, then scale and encode it, off the async workers:
/// `{ png, width, height }` (209 §4.7), the size being the viewport's in CSS px.
pub(crate) async fn snapshot_done(
    rx: Receiver<SnapshotReply>,
    deadline: Instant,
) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (image, viewport) = match recv_by(&rx, deadline) {
            Ok(reply) => reply?,
            Err(Late::Timeout(_)) => {
                return Err(format!(
                    "snapshot timed out after {} ms",
                    SNAPSHOT_TIMEOUT.as_millis()
                ));
            }
            Err(Late::Dropped) => return Err(DROPPED.to_owned()),
        };
        encode(&image, viewport)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// `image` drawn into an RGBA bitmap of the [`fit`] size, as base64 PNG with the `viewport` it shows.
fn encode(image: &CGImage, viewport: Viewport) -> Result<Value, String> {
    let (width, height) = fit(CGImage::width(Some(image)), CGImage::height(Some(image)))
        .ok_or_else(|| "the snapshot is empty".to_owned())?;
    let rgba = draw(image, width, height)?;
    let png = png(width, height, &rgba)?;
    Ok(json!({
        "png": base64::engine::general_purpose::STANDARD.encode(png),
        "width": viewport.0,
        "height": viewport.1,
    }))
}

/// `image` scaled into `width` × `height`, as straight RGBA rows from the top.
fn draw(image: &CGImage, width: usize, height: usize) -> Result<Vec<u8>, String> {
    let mut rgba = vec![0u8; width * height * 4];
    let space = CGColorSpace::new_device_rgb().ok_or("no RGB color space")?;
    // SAFETY: `rgba` holds `height` rows of `width * 4` bytes and outlives the context, dropped below.
    let context = unsafe {
        CGBitmapContextCreate(
            rgba.as_mut_ptr().cast(),
            width,
            height,
            8,
            width * 4,
            Some(&space),
            CGImageAlphaInfo::PremultipliedLast.0,
        )
    }
    .ok_or("no bitmap context for the snapshot")?;
    CGContext::set_interpolation_quality(Some(&context), CGInterpolationQuality::High);
    CGContext::draw_image(
        Some(&context),
        NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(width as f64, height as f64),
        ),
        Some(image),
    );
    drop(context);
    straighten(&mut rgba);
    Ok(rgba)
}

/// 8-bit RGBA rows as a PNG file.
fn png(width: usize, height: usize, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let error = |e: png::EncodingError| format!("snapshot encoding failed: {e}");
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width as u32, height as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(error)?;
    writer.write_image_data(rgba).map_err(error)?;
    writer.finish().map_err(error)?;
    Ok(out)
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use objc2_core_graphics::{
        CGBitmapContextCreateImage, CGBitmapContextGetBytesPerRow, CGBitmapContextGetData,
    };

    use super::*;

    /// A `width` × `height` opaque image, red in the top half and blue in the bottom half.
    fn halves(width: usize, height: usize) -> impl std::ops::Deref<Target = CGImage> {
        let space = CGColorSpace::new_device_rgb().unwrap();
        // SAFETY: null data: the context owns its pixels, which the image copies.
        let context = unsafe {
            CGBitmapContextCreate(
                null_mut(),
                width,
                height,
                8,
                width * 4,
                Some(&space),
                CGImageAlphaInfo::PremultipliedLast.0,
            )
        }
        .unwrap();
        let bytes_per_row = CGBitmapContextGetBytesPerRow(Some(&context));
        // SAFETY: the context's own buffer, `height` rows of `bytes_per_row` bytes, alive with it.
        let data = unsafe {
            std::slice::from_raw_parts_mut(
                CGBitmapContextGetData(Some(&context)).cast::<u8>(),
                bytes_per_row * height,
            )
        };
        for (row, line) in data.chunks_exact_mut(bytes_per_row).enumerate() {
            let px = if row < height / 2 {
                [255, 0, 0, 255]
            } else {
                [0, 0, 255, 255]
            };
            for out in line[..width * 4].as_chunks_mut::<4>().0 {
                *out = px;
            }
        }
        CGBitmapContextCreateImage(Some(&context)).unwrap()
    }

    /// The PNG of an `encode` answer: its size and RGBA rows.
    fn decoded(out: &Value) -> (u32, u32, Vec<u8>) {
        let png = base64::engine::general_purpose::STANDARD
            .decode(out["png"].as_str().unwrap())
            .unwrap();
        let mut reader = png::Decoder::new(std::io::Cursor::new(png))
            .read_info()
            .unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!(info.color_type, png::ColorType::Rgba);
        (info.width, info.height, buf)
    }

    #[test]
    fn tile_snapshot_scales_and_keeps_orientation() {
        // A 2x screen's 1600 × 200 pt viewport.
        let out = encode(&halves(3200, 400), (1600, 200)).unwrap();
        assert_eq!((&out["width"], &out["height"]), (&json!(1600), &json!(200)));
        let (width, height, buf) = decoded(&out);
        assert_eq!((width, height), (1600, 200));
        let at = |x: usize, y: usize| {
            let i = (y * 1600 + x) * 4;
            [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
        };
        assert_eq!(at(0, 0), [255, 0, 0, 255], "top row is the image's top");
        assert_eq!(at(1599, 199), [0, 0, 255, 255]);
    }

    #[test]
    fn tile_snapshot_small_image_keeps_its_size() {
        let out = encode(&halves(10, 4), (5, 2)).unwrap();
        assert_eq!((&out["width"], &out["height"]), (&json!(5), &json!(2)));
        let (width, height, _) = decoded(&out);
        assert_eq!((width, height), (10, 4));
    }
}
