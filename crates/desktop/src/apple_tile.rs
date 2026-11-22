//! The agent's tile operations on macOS and iOS (209 §4.7, D8): the WebKit half of `web_view_call`'s ops,
//! shared by the desktop's child webviews (`webview.rs`) and the iOS shell's raw views (`ios_webview.rs`).
//! The arguments, the script wrapper, the reply and the size arithmetic are pure rules in `web_rules::call`.
//!
//! - [`script`]: `callAsyncJavaScript` of the wrapped body in the main frame, in the isolated
//!   [`AGENT_WORLD`](crate::web_rules::profile::AGENT_WORLD) content world (one instance kept for the
//!   app's life, `apple_cookies::agent_world`) or in `WKContentWorld.page`, with one string argument
//!   (the caller's argument values as JSON). The body answers one JSON string; [`script_done`] waits
//!   for it until the caller's deadline and reads it off the main thread. WebKit cannot cancel a
//!   running script: past the deadline, or once the user's Stop ran ([`stop`]), the caller gets its
//!   error and the script's late answer is dropped.
//! - [`snapshot`]: `takeSnapshot` of the visible view; the main thread only takes the image's `CGImage`
//!   (thread-safe) and [`snapshot_done`] draws it into a bitmap of at most `SNAPSHOT_MAX_SIDE` pixels on the
//!   long side and encodes it as base64 PNG, off the main thread.
//! - Load ops: on iOS the shell owns the navigation delegate and [`navigation_done`] waits for the
//!   `WKNavigation` the op started. Where it cannot (macOS, where wry owns the delegate; an iOS history move
//!   within the document, which has no navigation), [`settle`] looks at the main frame every `SETTLE_POLL`
//!   and answers by `web_rules::call::Watch`, never the page from before the op.
//! - [`stop`] (the user's Stop, 209 §9): every wait of the tile's calls in flight ends with `STOPPED`
//!   within `STOP_POLL`; the caller also stops the view's loading.
//!
//! Script results, arguments and thrown messages can carry page data: they go to the caller only, never to
//! `tracing`.

use std::collections::BTreeMap;
#[cfg(target_os = "macos")]
use std::ptr::null_mut;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender};
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

use crate::apple_cookies::{DROPPED, Late};
#[cfg(target_os = "ios")]
use crate::ios_webview::WKWebView;
use crate::web_rules::call::{
    ARGS_NAME, Loaded, Look, SETTLE_POLL, SNAPSHOT_TIMEOUT, STOP_POLL, STOPPED, Script, Watch,
    World, failed, fit, reply, straighten, timed_out,
};

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
    let world = match script.world {
        World::Isolated => crate::apple_cookies::agent_world(mtm),
        // SAFETY: the page world.
        World::Page => unsafe { WKContentWorld::pageWorld(mtm) },
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

/// Per tile label, how many times [`stop`] ran for it.
static STOPS: Mutex<BTreeMap<String, u64>> = Mutex::new(BTreeMap::new());

fn stops() -> std::sync::MutexGuard<'static, BTreeMap<String, u64>> {
    STOPS.lock().unwrap_or_else(|e| e.into_inner())
}

/// The user's Stop on `label` (209 §9): every [`Ticket`] taken before it reads stopped.
pub(crate) fn stop(label: &str) {
    *stops().entry(label.to_owned()).or_default() += 1;
}

/// Taken when a call starts: whether [`stop`] ran for its tile since.
pub(crate) struct Ticket {
    label: String,
    at: u64,
}

impl Ticket {
    pub(crate) fn new(label: &str) -> Self {
        let at = stops().get(label).copied().unwrap_or(0);
        Self {
            label: label.to_owned(),
            at,
        }
    }

    pub(crate) fn stopped(&self) -> bool {
        stops().get(&self.label).copied().unwrap_or(0) != self.at
    }
}

/// Why a wait ended without an answer.
enum Ended {
    Late(Late),
    Stopped,
}

/// `rx`'s answer until `deadline`, looking every [`STOP_POLL`] whether `ticket` was stopped.
fn recv_or_stop<T>(rx: &Receiver<T>, deadline: Instant, ticket: &Ticket) -> Result<T, Ended> {
    let budget = deadline.saturating_duration_since(Instant::now());
    loop {
        if ticket.stopped() {
            return Err(Ended::Stopped);
        }
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left.min(STOP_POLL)) {
            Ok(answer) => return Ok(answer),
            Err(RecvTimeoutError::Disconnected) => return Err(Ended::Late(Late::Dropped)),
            Err(RecvTimeoutError::Timeout) if left <= STOP_POLL => {
                return Err(Ended::Late(Late::Timeout(budget)));
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

/// Wait for [`script`]'s answer until `deadline` (the caller's `timeout` from the call's start) or the
/// tile's Stop, and read it, off the async workers.
pub(crate) async fn script_done(
    rx: Receiver<ScriptReply>,
    deadline: Instant,
    timeout: Duration,
    ticket: Ticket,
) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || match recv_or_stop(&rx, deadline, &ticket) {
        Ok(Ok(json)) => reply(json.as_deref()),
        Ok(Err(e)) => Err(e),
        Err(Ended::Late(Late::Timeout(_))) => Err(timed_out(timeout)),
        Err(Ended::Late(Late::Dropped)) => Err(DROPPED.to_owned()),
        Err(Ended::Stopped) => Err(STOPPED.to_owned()),
    })
    .await
    .map_err(|e| e.to_string())?
}

/// How the navigation a load op started ended (iOS, where the shell's delegate sees it).
#[cfg(target_os = "ios")]
pub(crate) enum Navigated {
    /// `didFinishNavigation`.
    Finished,
    /// `didFailNavigation` / `didFailProvisionalNavigation`: `web_rules::call::load_error`.
    Failed(String),
    /// The call's time ran out first.
    TimedOut,
}

/// Wait for the delegate's word on the op's navigation (`None` finished, `Some(error)` failed) until
/// `deadline` or the tile's Stop, off the async workers. The sender gone means the view was destroyed.
#[cfg(target_os = "ios")]
pub(crate) async fn navigation_done(
    rx: Receiver<Option<String>>,
    deadline: Instant,
    ticket: Ticket,
) -> Result<Navigated, String> {
    tauri::async_runtime::spawn_blocking(move || match recv_or_stop(&rx, deadline, &ticket) {
        Ok(None) => Ok(Navigated::Finished),
        Ok(Some(error)) => Ok(Navigated::Failed(error)),
        Err(Ended::Late(Late::Timeout(_))) => Ok(Navigated::TimedOut),
        Err(Ended::Late(Late::Dropped)) => Err("the web view closed during the load".to_owned()),
        Err(Ended::Stopped) => Err(STOPPED.to_owned()),
    })
    .await
    .map_err(|e| e.to_string())?
}

/// After a load op that cannot wait for its navigation: `look` at the main frame (each look bounded by the
/// call's `deadline`; `None` when the main thread did not answer by then) every [`SETTLE_POLL`] until
/// [`Watch`] says it is done, and answer it (209 §4.7): `{ url, title, loading, error? }`, `loading: true`
/// at the deadline. `before` is the look taken with the op; `counts`: `Look::finished` is kept.
pub(crate) async fn settle<F, Fut>(
    mut look: F,
    before: Look,
    counts: bool,
    deadline: Instant,
    ticket: &Ticket,
) -> Result<Value, String>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<Option<Look>, String>>,
{
    let mut watch = Watch::new(&before, counts);
    let mut last = before.state;
    loop {
        if ticket.stopped() {
            return Err(STOPPED.to_owned());
        }
        let Some(now) = look().await? else {
            // A main thread busy past the budget: as at any timeout, the page as last seen, still loading.
            return answer(Loaded {
                loading: true,
                ..last
            });
        };
        last = now.state.clone();
        if let Some(done) = watch.step(now, Instant::now() >= deadline) {
            return answer(done);
        }
        let nap = SETTLE_POLL.min(deadline.saturating_duration_since(Instant::now()));
        tauri::async_runtime::spawn_blocking(move || std::thread::sleep(nap))
            .await
            .map_err(|e| e.to_string())?;
    }
}

/// A load op's answer as JSON.
pub(crate) fn answer(loaded: Loaded) -> Result<Value, String> {
    serde_json::to_value(loaded).map_err(|e| e.to_string())
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

/// Wait for [`snapshot`]'s image until `deadline` or the tile's Stop, then scale and encode it, off the
/// async workers: `{ png, width, height }` (209 §4.7), the size being the viewport's in CSS px.
pub(crate) async fn snapshot_done(
    rx: Receiver<SnapshotReply>,
    deadline: Instant,
    ticket: Ticket,
) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (image, viewport) = match recv_or_stop(&rx, deadline, &ticket) {
            Ok(reply) => reply?,
            Err(Ended::Late(Late::Timeout(_))) => {
                return Err(format!(
                    "snapshot timed out after {} ms",
                    SNAPSHOT_TIMEOUT.as_millis()
                ));
            }
            Err(Ended::Late(Late::Dropped)) => return Err(DROPPED.to_owned()),
            Err(Ended::Stopped) => return Err(STOPPED.to_owned()),
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

    fn look(url: &str, loading: bool, finished: u64) -> Look {
        Look {
            state: Loaded {
                url: url.into(),
                title: String::new(),
                loading,
                error: None,
            },
            finished,
        }
    }

    /// `settle` over scripted looks (the last one repeats), with the time it took.
    fn settle_over(
        looks: Vec<Result<Option<Look>, String>>,
        budget: Duration,
    ) -> (Result<Value, String>, Duration) {
        let mut looks = std::collections::VecDeque::from(looks);
        let ticket = Ticket::new("web-settle-test");
        let started = Instant::now();
        let out = tauri::async_runtime::block_on(settle(
            || {
                let next = if looks.len() > 1 {
                    looks.pop_front().unwrap()
                } else {
                    looks[0].clone()
                };
                std::future::ready(next)
            },
            look("https://old.example/", false, 5),
            true,
            started + budget,
            &ticket,
        ));
        (out, started.elapsed())
    }

    #[test]
    fn tile_settle_answers_the_new_page_not_the_old_one() {
        let (out, _) = settle_over(
            vec![
                Ok(Some(look("https://old.example/", false, 5))),
                Ok(Some(look("https://new.example/", true, 5))),
                Ok(Some(look("https://new.example/", false, 6))),
            ],
            Duration::from_secs(5),
        );
        assert_eq!(
            out.unwrap(),
            json!({ "url": "https://new.example/", "title": "", "loading": false })
        );
    }

    #[test]
    fn tile_settle_old_page_until_the_deadline_is_still_loading() {
        let (out, took) = settle_over(
            vec![Ok(Some(look("https://old.example/", false, 5)))],
            Duration::from_millis(350),
        );
        assert_eq!(out.unwrap()["loading"], json!(true));
        assert!(took >= Duration::from_millis(350), "{took:?}");
    }

    #[test]
    fn tile_settle_main_thread_stall_is_a_timeout_not_an_error() {
        let (out, _) = settle_over(
            vec![Ok(Some(look("https://new.example/", true, 5))), Ok(None)],
            Duration::from_secs(5),
        );
        assert_eq!(
            out.unwrap(),
            json!({ "url": "https://new.example/", "title": "", "loading": true })
        );
    }

    #[test]
    fn tile_stop_abandons_a_running_script_and_spares_later_calls() {
        const LABEL: &str = "web-stop-test-0001";
        let (_tx, rx) = std::sync::mpsc::sync_channel::<ScriptReply>(1);
        let ticket = Ticket::new(LABEL);
        let started = Instant::now();
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(100));
            stop(LABEL);
        });
        let out = tauri::async_runtime::block_on(script_done(
            rx,
            started + Duration::from_secs(30),
            Duration::from_secs(30),
            ticket,
        ));
        assert_eq!(out.unwrap_err(), STOPPED);
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(!Ticket::new(LABEL).stopped());
        assert!(!Ticket::new("web-stop-test-0002").stopped());
    }
}
