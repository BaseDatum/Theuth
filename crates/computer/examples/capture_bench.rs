//! Spike 121 (docs/design/121-sck-capture-spike.md): `/usr/sbin/screencapture`
//! shell-out vs in-process ScreenCaptureKit, per display.
//!
//! ```text
//! cargo run -p computer --example capture_bench --release -- [--save <dir>]
//! cargo run -p computer --example capture_bench --release -- --loop sck|sck-cg|sck-image|sck-rgba|sck-content|screencapture [--rounds N]   # for /usr/bin/time -l
//! ```
//!
//! Prints min / median / p95 wall-clock ms over 20 timed grabs (after 3
//! warm-ups), image dimensions and raw byte count per backend and display; the
//! cost of `SCShareableContent` enumeration (both getters) and of
//! `SCScreenshotManager::capture_image` alone; a cross-backend pixel diff; and
//! the `capture(None)` composite under each backend. `--save <dir>` writes
//! `shot-<backend>-<n>.png` and `composite-<backend>.png` there.

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("capture_bench is macOS-only");
}

#[cfg(target_os = "macos")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::path::PathBuf;
    use std::time::Instant;

    use computer::frame::Display;
    use computer::screen::{
        capture, displays, grab_sck, grab_screencapture, screen_recording_granted,
    };
    use image::RgbaImage;
    use screencapturekit::screenshot_manager::{CGImageExt, SCScreenshotManager};
    use screencapturekit::shareable_content::SCShareableContent;
    use screencapturekit::stream::configuration::SCStreamConfiguration;
    use screencapturekit::stream::content_filter::SCContentFilter;

    const WARM: usize = 3;
    const RUNS: usize = 20;

    fn stats(mut ms: Vec<f64>) -> (f64, f64, f64) {
        ms.sort_by(f64::total_cmp);
        let n = ms.len();
        (ms[0], ms[n / 2], ms[(n * 95).div_ceil(100) - 1])
    }

    fn timed<T>(f: impl Fn() -> T) -> (Vec<f64>, T) {
        let mut last = None;
        for _ in 0..WARM {
            last = Some(f());
        }
        let mut ms = Vec::with_capacity(RUNS);
        for _ in 0..RUNS {
            let t = Instant::now();
            last = Some(f());
            ms.push(t.elapsed().as_secs_f64() * 1e3);
        }
        (ms, last.unwrap())
    }

    fn report(label: &str, ms: Vec<f64>) {
        let (min, med, p95) = stats(ms);
        println!("  {label:<44} min {min:8.1}  median {med:8.1}  p95 {p95:8.1} ms");
    }

    /// Mean absolute per-channel RGB difference (alpha ignored).
    fn rgb_diff(a: &RgbaImage, b: &RgbaImage) -> f64 {
        let sum: u64 = a
            .pixels()
            .zip(b.pixels())
            .map(|(p, q)| (0..3).map(|c| u64::from(p[c].abs_diff(q[c]))).sum::<u64>())
            .sum();
        sum as f64 / (u64::from(a.width()) * u64::from(a.height()) * 3) as f64
    }

    /// Display filter + config for `d`, as `grab_sck` builds them.
    fn sck_filter(d: &Display) -> (SCContentFilter, SCStreamConfiguration) {
        let c = SCShareableContent::create()
            .with_on_screen_windows_only(true)
            .with_exclude_desktop_windows(true)
            .get()
            .expect("content");
        let s = c
            .displays()
            .into_iter()
            .find(|s| s.display_id() == d.id)
            .expect("display");
        let f = SCContentFilter::create()
            .with_display(&s)
            .with_excluding_windows(&[])
            .build()
            .expect("filter");
        let cfg = SCStreamConfiguration::new()
            .with_width((d.width as f32 * d.scale).round() as u32)
            .with_height((d.height as f32 * d.scale).round() as u32)
            .with_shows_cursor(false);
        (f, cfg)
    }

    /// The spike's first `grab_sck`: `capture_image` + `CGImage::rgba_data`
    /// (CoreGraphics render). Kept to reproduce the RSS growth of 121 §4.3.
    fn grab_sck_cg(d: &Display) -> RgbaImage {
        let (f, cfg) = sck_filter(d);
        let img = SCScreenshotManager::capture_image(&f, &cfg).expect("capture");
        let (iw, ih) = (img.width() as u32, img.height() as u32);
        RgbaImage::from_raw(iw, ih, img.rgba_data().expect("rgba")).expect("size")
    }

    fn grab(backend: &str, d: &Display, n: usize, tmp: &std::path::Path) -> RgbaImage {
        match backend {
            "sck" => grab_sck(d).expect("sck grab"),
            "sck-cg" => grab_sck_cg(d),
            _ => grab_screencapture(d, n, tmp).expect("screencapture grab"),
        }
    }

    /// Which SCK object retains memory (121 §4.3): `sck-image` = `capture_image`
    /// only, `CGImage` dropped per round, no RGBA; `sck-rgba` = one `CGImage`,
    /// `rgba_data` per round; `sck-content` = `SCShareableContent` only.
    fn sck_piece(mode: &str, d: &Display, rounds: usize) -> usize {
        let c = SCShareableContent::create()
            .with_on_screen_windows_only(true)
            .with_exclude_desktop_windows(true)
            .get()
            .expect("content");
        let s = c
            .displays()
            .into_iter()
            .find(|s| s.display_id() == d.id)
            .expect("display");
        let f = SCContentFilter::create()
            .with_display(&s)
            .with_excluding_windows(&[])
            .build()
            .expect("filter");
        let cfg = SCStreamConfiguration::new()
            .with_width((d.width as f32 * d.scale).round() as u32)
            .with_height((d.height as f32 * d.scale).round() as u32)
            .with_shows_cursor(false);
        let mut bytes = 0;
        match mode {
            "sck-image" => {
                for _ in 0..rounds {
                    let img = SCScreenshotManager::capture_image(&f, &cfg).expect("capture");
                    bytes += img.width() * img.height() * 4;
                }
            }
            "sck-rgba" => {
                let img = SCScreenshotManager::capture_image(&f, &cfg).expect("capture");
                for _ in 0..rounds {
                    bytes += img.rgba_data().expect("rgba").len();
                }
            }
            _ => {
                for _ in 0..rounds {
                    bytes += SCShareableContent::create()
                        .with_on_screen_windows_only(true)
                        .with_exclude_desktop_windows(true)
                        .get()
                        .expect("content")
                        .displays()
                        .len();
                }
            }
        }
        bytes
    }

    let mut args = std::env::args().skip(1);
    let mut save: Option<PathBuf> = None;
    let mut loop_backend: Option<String> = None;
    let mut rounds = RUNS;
    let mut hold_secs = 0u64;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--save" => save = args.next().map(PathBuf::from),
            "--loop" => loop_backend = args.next(),
            "--rounds" => rounds = args.next().ok_or("--rounds N")?.parse()?,
            "--hold" => hold_secs = args.next().ok_or("--hold SECS")?.parse()?,
            other => return Err(format!("unknown arg {other}").into()),
        }
    }
    if !screen_recording_granted() {
        return Err("Screen Recording not granted to this terminal".into());
    }
    let tmp = std::env::temp_dir().join("sck-spike-bench");
    let all = displays()?;

    // RSS mode: just the N-shot loop (default 20) over every display, for `/usr/bin/time -l`.
    // `--hold SECS` sleeps afterwards so `vmmap -summary <pid>` can look at the process.
    if let Some(backend) = loop_backend {
        let piece = matches!(backend.as_str(), "sck-image" | "sck-rgba" | "sck-content");
        let mut bytes = 0usize;
        if piece {
            for d in &all {
                bytes += sck_piece(&backend, d, rounds);
            }
        } else {
            for _ in 0..rounds {
                for (n, d) in all.iter().enumerate() {
                    bytes += grab(&backend, d, n, &tmp).len();
                }
            }
        }
        println!(
            "{backend}: {rounds} rounds × {} displays, {bytes} raw bytes total, pid {}",
            all.len(),
            std::process::id()
        );
        if hold_secs > 0 {
            std::thread::sleep(std::time::Duration::from_secs(hold_secs));
        }
        return Ok(());
    }

    println!("displays: {}", all.len());
    for d in &all {
        println!(
            "  #{} id={} {:?} main={} {}x{} @({},{}) scale={}",
            d.index, d.id, d.name, d.main, d.width, d.height, d.x, d.y, d.scale
        );
    }

    // Per display, per backend.
    let mut shots: Vec<(RgbaImage, RgbaImage, RgbaImage)> = Vec::new();
    for (n, d) in all.iter().enumerate() {
        println!(
            "\ndisplay #{} ({}x{} logical, scale {}):",
            d.index, d.width, d.height, d.scale
        );
        let (ms_sc, img_sc) = timed(|| grab("screencapture", d, n, &tmp));
        report("screencapture (spawn + PNG decode)", ms_sc);
        println!(
            "    {}x{} px, {} raw bytes",
            img_sc.width(),
            img_sc.height(),
            img_sc.len()
        );
        let (ms_sck, img_sck) = timed(|| grab("sck", d, n, &tmp));
        report("sck (content + capture_sample_buffer + copy)", ms_sck);
        println!(
            "    {}x{} px, {} raw bytes",
            img_sck.width(),
            img_sck.height(),
            img_sck.len()
        );
        let (ms_cg, img_cg) = timed(|| grab("sck-cg", d, n, &tmp));
        report("sck-cg (content + capture_image + rgba_data)", ms_cg);
        println!(
            "    {}x{} px, {} raw bytes; mean |ΔRGB| vs sck = {:.3}",
            img_cg.width(),
            img_cg.height(),
            img_cg.len(),
            if img_cg.dimensions() == img_sck.dimensions() {
                rgb_diff(&img_cg, &img_sck)
            } else {
                f64::NAN
            }
        );

        // SCK pieces alone.
        let (ms_get, content) =
            timed(|| SCShareableContent::get().expect("SCShareableContent::get"));
        report("SCShareableContent::get() (all windows)", ms_get);
        let (ms_get_on, _) = timed(|| {
            SCShareableContent::create()
                .with_on_screen_windows_only(true)
                .with_exclude_desktop_windows(true)
                .get()
                .expect("SCShareableContent on-screen-only")
        });
        report("SCShareableContent on-screen-only getter", ms_get_on);
        let disp = content
            .displays()
            .into_iter()
            .find(|s| s.display_id() == d.id)
            .ok_or("display id not in SCShareableContent")?;
        let filter = SCContentFilter::create()
            .with_display(&disp)
            .with_excluding_windows(&[])
            .build()?;
        let (w, h) = (
            (d.width as f32 * d.scale).round() as u32,
            (d.height as f32 * d.scale).round() as u32,
        );
        let config = SCStreamConfiguration::new()
            .with_width(w)
            .with_height(h)
            .with_shows_cursor(false);
        let (ms_cap, cg) =
            timed(|| SCScreenshotManager::capture_image(&filter, &config).expect("capture_image"));
        report("SCScreenshotManager::capture_image alone", ms_cap);
        let (ms_rgba, _) = timed(|| cg.rgba_data().expect("rgba_data"));
        report("CGImage → RGBA bytes alone", ms_rgba);
        let (ms_filter, _) = timed(|| {
            SCContentFilter::create()
                .with_display(&disp)
                .with_excluding_windows(&[])
                .build()
                .expect("filter")
        });
        report("SCContentFilter build alone", ms_filter);
        // The brief's first version: full `get()` every call, nothing cached.
        let (ms_full, _) = timed(|| {
            let c = SCShareableContent::get().expect("get");
            let s = c
                .displays()
                .into_iter()
                .find(|s| s.display_id() == d.id)
                .expect("display");
            let f = SCContentFilter::create()
                .with_display(&s)
                .with_excluding_windows(&[])
                .build()
                .expect("filter");
            SCScreenshotManager::capture_image(&f, &config)
                .expect("capture")
                .rgba_data()
                .expect("rgba")
        });
        report("sck with full get() per call (uncached v1)", ms_full);
        // Cached SCDisplay/filter for the process lifetime: capture + RGBA only.
        let (ms_cached, _) = timed(|| {
            SCScreenshotManager::capture_image(&filter, &config)
                .expect("capture")
                .rgba_data()
                .expect("rgba")
        });
        report("sck with cached filter (capture + RGBA)", ms_cached);

        // Cross-backend comparison, back to back on a still desktop.
        let a = grab("screencapture", d, n, &tmp);
        let b = grab("sck", d, n, &tmp);
        let c = grab("sck-cg", d, n, &tmp);
        if a.dimensions() == b.dimensions() && a.dimensions() == c.dimensions() {
            println!(
                "  dimensions equal ({}x{}); mean |ΔRGB| per channel: sck vs screencapture {:.3}, sck-cg vs screencapture {:.3}",
                a.width(),
                a.height(),
                rgb_diff(&a, &b),
                rgb_diff(&a, &c)
            );
        } else {
            println!(
                "  DIMENSIONS DIFFER: screencapture {}x{} vs sck {}x{} vs sck-cg {}x{}",
                a.width(),
                a.height(),
                b.width(),
                b.height(),
                c.width(),
                c.height()
            );
        }
        shots.push((a, b, c));
    }

    // Composite (`display: None`) under each backend, via the public `capture`.
    println!("\ncomposite capture(None, 1568):");
    let mut composites = Vec::new();
    for backend in ["screencapture", "sck"] {
        // SAFETY: single-threaded at this point; SCK's own threads never read the environment.
        unsafe { std::env::set_var("THEUTH_CAPTURE_BACKEND", backend) };
        let t = Instant::now();
        let shot = capture(None, 1568, &tmp)?;
        let ms = t.elapsed().as_secs_f64() * 1e3;
        println!(
            "  {backend:<14} {:>7.1} ms  {}x{} PNG {} bytes",
            ms,
            shot.frame.size.0,
            shot.frame.size.1,
            shot.png.len()
        );
        composites.push((backend, shot.png));
    }
    unsafe { std::env::remove_var("THEUTH_CAPTURE_BACKEND") };

    if let Some(dir) = save {
        std::fs::create_dir_all(&dir)?;
        for (i, (a, b, c)) in shots.iter().enumerate() {
            let n = i + 1;
            a.save(dir.join(format!("shot-screencapture-{n}.png")))?;
            b.save(dir.join(format!("shot-sck-{n}.png")))?;
            c.save(dir.join(format!("shot-sck-cg-{n}.png")))?;
        }
        for (backend, png) in &composites {
            std::fs::write(dir.join(format!("composite-{backend}.png")), png)?;
        }
        println!("\nsaved PNGs under {}", dir.display());
    }
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(())
}
