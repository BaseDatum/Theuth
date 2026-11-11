//! Validated image files for tool output (17 §1, 52 §5.3/§7.1): the one
//! ingress every producer that turns bytes into a `ToolOutput.files` entry
//! goes through. Content is validated by decoding, never by extension or
//! MIME label; the file lands content-addressed under the caller's output
//! directory so an overwritten source path still yields a distinct snapshot.

use std::borrow::Cow;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, ImageFormat, ImageReader, Limits};
use sha2::{Digest, Sha256};

/// Largest encoded image a provider accepts (Anthropic's 5 MB is the binding cap).
pub const MAX_IMAGE_BYTES: u64 = 5 * 1024 * 1024;

/// Longest side `read_file`/`read_url` scale an image to: `computer_screenshot`'s
/// `max_edge` default, Anthropic's recommended long edge (larger is downscaled
/// by the provider anyway).
pub const MAX_EDGE: u32 = 1568;

/// Longest side any provider accepts at all (Anthropic: 8000 px); the
/// [`for_provider`] guard scales anything larger down to it.
pub const PROVIDER_MAX_SIDE: u32 = 8000;

/// Largest side the decoder will consider; providers reject far smaller.
const MAX_SIDE: u32 = 10_000;

/// Largest side [`fit`] decodes before scaling (a full-page screenshot is
/// tall); the decoder's allocation limit still bounds memory.
const FIT_MAX_SIDE: u32 = 20_000;

/// Largest source file [`for_provider`] reads before scaling.
const MAX_SOURCE_BYTES: u64 = 64 * 1024 * 1024;

/// JPEG quality of a re-encoded image.
const JPEG_QUALITY: u8 = 85;

/// Formats every supported provider accepts as an image part.
const SUPPORTED: [ImageFormat; 4] = [
    ImageFormat::Png,
    ImageFormat::Jpeg,
    ImageFormat::Gif,
    ImageFormat::WebP,
];

fn format_of(mime: &str) -> Result<ImageFormat, String> {
    ImageFormat::from_mime_type(mime)
        .filter(|f| SUPPORTED.contains(f))
        .ok_or_else(|| format!("{mime} is not a supported image type (png, jpeg, gif, webp)"))
}

/// The supported format of `bytes`, from their magic bytes (never a name or label).
#[must_use]
pub fn sniff(bytes: &[u8]) -> Option<ImageFormat> {
    image::guess_format(bytes)
        .ok()
        .filter(|f| SUPPORTED.contains(f))
}

/// An image ready to attach: [`fit`]'s result.
#[derive(Debug)]
pub struct Fitted<'a> {
    /// What to send: the source bytes when they already fit, else re-encoded.
    pub bytes: Cow<'a, [u8]>,
    /// MIME of `bytes`.
    pub mime: &'static str,
    /// Pixel size of the source.
    pub source: (u32, u32),
    /// Pixel size of `bytes` when it was scaled down.
    pub scaled: Option<(u32, u32)>,
}

/// `bytes` as an image every provider accepts. Format from the magic bytes;
/// the image is decoded (a corrupt file is an error here, not a provider
/// 400 later). Unchanged when its long side is within `max_edge` and it is
/// at most [`MAX_IMAGE_BYTES`]; else scaled to a long side of `max_edge` and
/// re-encoded (JPEG stays JPEG, the rest becomes PNG, then JPEG, then half
/// the size while still over the byte cap). Errors carry no payload text.
pub fn fit(bytes: &[u8], max_edge: u32) -> Result<Fitted<'_>, String> {
    let format = sniff(bytes).ok_or("not a supported image (png, jpeg, gif, webp)")?;
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(FIT_MAX_SIDE);
    limits.max_image_height = Some(FIT_MAX_SIDE);
    reader.limits(limits);
    let mime = format.to_mime_type();
    let mut img = reader
        .decode()
        .map_err(|e| format!("invalid {mime}: {e}"))?;
    let source = img.dimensions();
    let long = source.0.max(source.1);
    if long <= max_edge && bytes.len() as u64 <= MAX_IMAGE_BYTES {
        return Ok(Fitted {
            bytes: Cow::Borrowed(bytes),
            mime,
            source,
            scaled: None,
        });
    }
    if long > max_edge {
        img = img.resize(max_edge, max_edge, FilterType::Triangle);
    }
    let (mut out, mut mime) = if format == ImageFormat::Jpeg {
        (jpeg(&img)?, "image/jpeg")
    } else {
        (png(&img)?, "image/png")
    };
    if out.len() as u64 > MAX_IMAGE_BYTES && mime == "image/png" {
        (out, mime) = (jpeg(&img)?, "image/jpeg");
    }
    while out.len() as u64 > MAX_IMAGE_BYTES && img.width().min(img.height()) > 64 {
        img = img.resize(img.width() / 2, img.height() / 2, FilterType::Triangle);
        out = jpeg(&img)?;
        mime = "image/jpeg";
    }
    if out.len() as u64 > MAX_IMAGE_BYTES {
        return Err(format!(
            "still over {} MB after scaling",
            MAX_IMAGE_BYTES >> 20
        ));
    }
    let size = img.dimensions();
    Ok(Fitted {
        bytes: Cow::Owned(out),
        mime,
        source,
        scaled: (size != source).then_some(size),
    })
}

fn png(img: &DynamicImage) -> Result<Vec<u8>, String> {
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, ImageFormat::Png)
        .map_err(|e| format!("png encode: {e}"))?;
    Ok(out.into_inner())
}

fn jpeg(img: &DynamicImage) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    DynamicImage::ImageRgb8(img.to_rgb8())
        .write_with_encoder(JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY))
        .map_err(|e| format!("jpeg encode: {e}"))?;
    Ok(out)
}

/// The bytes and MIME to send for the file at `path` (17 §3.1, 52 §5.3): the
/// one guard every attachment and tool image passes before it is stored and
/// sent. `None` when its magic bytes are not a supported image (a PDF goes
/// its own way); otherwise [`fit`] to [`PROVIDER_MAX_SIDE`] and
/// [`MAX_IMAGE_BYTES`], so nothing a provider rejects for size or a corrupt
/// payload leaves.
pub fn for_provider(path: &Path) -> anyhow::Result<Option<(Vec<u8>, &'static str)>> {
    if sniff(&read_regular(path, 64)?).is_none() {
        return Ok(None);
    }
    let bytes = read_regular(path, MAX_SOURCE_BYTES + 1)?;
    if bytes.len() as u64 > MAX_SOURCE_BYTES {
        anyhow::bail!("exceeds {} MB", MAX_SOURCE_BYTES >> 20);
    }
    let fitted = fit(&bytes, PROVIDER_MAX_SIDE).map_err(anyhow::Error::msg)?;
    let mime = fitted.mime;
    Ok(Some(match fitted.bytes {
        Cow::Borrowed(_) => (bytes, mime),
        Cow::Owned(out) => (out, mime),
    }))
}

/// `bytes` is a complete, decodable image of the format `mime` claims and at
/// most [`MAX_IMAGE_BYTES`] long. Decoding is bounded by [`MAX_SIDE`] and the
/// decoder's allocation limit, so a hostile header cannot make this allocate
/// past that. The error never contains payload text.
pub fn validate_bytes(bytes: &[u8], mime: &str) -> Result<(), String> {
    let format = format_of(mime)?;
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return Err(format!(
            "{:.1} MB exceeds {} MB",
            bytes.len() as f64 / (1024.0 * 1024.0),
            MAX_IMAGE_BYTES >> 20
        ));
    }
    let sniffed = image::guess_format(bytes).map_err(|_| "not a recognized image".to_owned())?;
    if sniffed != format {
        return Err(format!(
            "content is {} but was labeled {mime}",
            sniffed.to_mime_type()
        ));
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    reader.limits(limits);
    reader
        .decode()
        .map(drop)
        .map_err(|e| format!("invalid {mime}: {e}"))
}

/// Validate, then write `bytes` as `<dir>/<sha256[..16]>.<ext>` (extension
/// from the format, never from a remote name). An existing file with that
/// name is reused only if it holds exactly these bytes; anything else there
/// (truncated, tampered, a leftover) is replaced. The write goes through a
/// per-call temp file and a rename, so a concurrent reader of the returned
/// path sees either nothing or the whole image, never a partial one.
pub fn save(bytes: &[u8], mime: &str, dir: &Path) -> anyhow::Result<PathBuf> {
    validate_bytes(bytes, mime).map_err(anyhow::Error::msg)?;
    write_blob(bytes, mime, dir)
}

/// [`save`] for an image [`fit`] already decoded: no second decode.
pub fn save_fitted(f: &Fitted<'_>, dir: &Path) -> anyhow::Result<PathBuf> {
    write_blob(&f.bytes, f.mime, dir)
}

/// The result of `tool` reading image `bytes` (`read_file`, `read_url`):
/// [`fit`] to [`MAX_EDGE`], saved under `dir` ([`save_fitted`]) and
/// attached as the tool's picture; the text is one header line,
/// `[<tool> <shown>: image/png 1600×1100, 12 KB]` plus `; scaled to W×H`
/// when it was downscaled (or `; re-encoded as <mime>` when only the bytes
/// changed). An undecodable or still-oversize image is the error. Blocking.
pub fn tool_output(
    tool: &str,
    shown: &str,
    bytes: &[u8],
    dir: &Path,
) -> anyhow::Result<crate::ToolOutput> {
    let source_mime = sniff(bytes).map_or("image", |f| f.to_mime_type());
    let f = fit(bytes, MAX_EDGE).map_err(|e| anyhow::anyhow!("{shown}: {e}"))?;
    let path = save_fitted(&f, dir)?;
    let (w, h) = f.source;
    let mut text = format!(
        "[{tool} {shown}: {source_mime} {w}\u{d7}{h}, {}",
        human(bytes.len())
    );
    match f.scaled {
        Some((w, h)) => text.push_str(&format!("; scaled to {w}\u{d7}{h}")),
        None if f.mime != source_mime => text.push_str(&format!("; re-encoded as {}", f.mime)),
        None => {}
    }
    text.push(']');
    Ok(crate::ToolOutput {
        files: vec![path],
        ..crate::ToolOutput::ok(text)
    })
}

/// `n` bytes as `12 KB` / `3.4 MB`.
fn human(n: usize) -> String {
    if n < 1024 * 1024 {
        format!("{} KB", n.div_ceil(1024).max(1))
    } else {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    }
}

fn write_blob(bytes: &[u8], mime: &str, dir: &Path) -> anyhow::Result<PathBuf> {
    let ext = format_of(mime)
        .map_err(anyhow::Error::msg)?
        .extensions_str()[0];
    let hex = hex::encode(Sha256::digest(bytes));
    let stem = &hex[..16];
    let path = dir.join(format!("{stem}.{ext}"));
    if read_regular(&path, bytes.len() as u64 + 1).is_ok_and(|held| held == bytes) {
        return Ok(path);
    }
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!("{stem}.{}.tmp", uuid::Uuid::new_v4()));
    std::fs::write(&tmp, bytes)
        .and_then(|()| std::fs::rename(&tmp, &path))
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })?;
    Ok(path)
}

/// Up to `max` bytes of the regular file at `path` (pass one more than the
/// bound to detect oversize). Anything that is not a regular file is refused
/// before it is opened; on Unix the open is also non-blocking and the handle
/// re-checked, so a FIFO swapped in after the stat cannot hang the caller.
fn read_regular(path: &Path, max: u64) -> anyhow::Result<Vec<u8>> {
    if !std::fs::metadata(path)?.is_file() {
        anyhow::bail!("not a regular file");
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NONBLOCK);
    }
    let file = opts.open(path)?;
    if !file.metadata()?.is_file() {
        anyhow::bail!("not a regular file");
    }
    let mut bytes = Vec::new();
    file.take(max).read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Copy the image at `path` into `dir` as a content-addressed snapshot
/// ([`save`]): a regular file, read with a [`MAX_IMAGE_BYTES`] bound, whose
/// format comes from its bytes. Later overwrites of `path` cannot touch the
/// returned file.
pub fn snapshot(path: &Path, dir: &Path) -> anyhow::Result<PathBuf> {
    let name = path
        .file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy();
    let bytes =
        read_regular(path, MAX_IMAGE_BYTES + 1).map_err(|e| anyhow::anyhow!("{name}: {e}"))?;
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        anyhow::bail!("{name}: exceeds {} MB", MAX_IMAGE_BYTES >> 20);
    }
    let format = image::guess_format(&bytes)
        .ok()
        .filter(|f| SUPPORTED.contains(f))
        .ok_or_else(|| anyhow::anyhow!("{name}: not a supported image"))?;
    save(&bytes, format.to_mime_type(), dir).map_err(|e| anyhow::anyhow!("{name}: {e}"))
}

#[cfg(test)]
pub(crate) mod tests {
    use image::{Rgb, RgbImage};

    use super::*;

    /// A real `w`×`h` image encoded as `format`, one flat colour `c`.
    pub(crate) fn encoded(format: ImageFormat, w: u32, h: u32, c: u8) -> Vec<u8> {
        let img = RgbImage::from_pixel(w, h, Rgb([c, 0, 0]));
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, format).unwrap();
        out.into_inner()
    }

    fn tmp() -> (crate::TestDir, PathBuf) {
        let guard = crate::TestDir::new("images");
        let dir = guard.0.clone();
        (guard, dir)
    }

    /// `w`×`h` PNG of pseudo-random pixels: incompressible, so large.
    fn noise_png(w: u32, h: u32) -> Vec<u8> {
        let mut s = 0x9e37_79b9u32;
        let img = RgbImage::from_fn(w, h, |_, _| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            let [a, b, c, _] = s.to_le_bytes();
            Rgb([a, b, c])
        });
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn fit_keeps_what_fits_and_scales_or_reencodes_the_rest() {
        // Within both limits: the very bytes, borrowed.
        let png = encoded(ImageFormat::Png, 10, 4, 1);
        let f = fit(&png, MAX_EDGE).unwrap();
        assert!(matches!(f.bytes, Cow::Borrowed(_)));
        assert_eq!((f.mime, f.source, f.scaled), ("image/png", (10, 4), None));

        // Long side over the edge: scaled with the aspect kept, PNG stays PNG.
        let tall = encoded(ImageFormat::Png, 9000, 900, 1);
        let f = fit(&tall, PROVIDER_MAX_SIDE).unwrap();
        assert_eq!((f.mime, f.scaled), ("image/png", Some((8000, 800))));
        assert_eq!(sniff(&f.bytes), Some(ImageFormat::Png));

        // Pixels fit, bytes do not: re-encoded as JPEG under the cap, same size.
        let big = noise_png(1500, 1300);
        assert!(big.len() as u64 > MAX_IMAGE_BYTES, "{}", big.len());
        let f = fit(&big, MAX_EDGE).unwrap();
        assert_eq!((f.mime, f.scaled), ("image/jpeg", None));
        assert!(f.bytes.len() as u64 <= MAX_IMAGE_BYTES);

        // Not an image; a truncated one.
        assert!(
            fit(b"hello", MAX_EDGE)
                .unwrap_err()
                .contains("not a supported image")
        );
        assert!(
            fit(&png[..png.len() / 2], MAX_EDGE)
                .unwrap_err()
                .starts_with("invalid image/png")
        );
    }

    #[test]
    fn for_provider_sniffs_content_and_bounds_size() {
        let (_g, dir) = tmp();
        std::fs::create_dir_all(&dir).unwrap();
        // A JPEG named `.png` is sent as the JPEG it is.
        let p = dir.join("shot.png");
        let jpg = encoded(ImageFormat::Jpeg, 8, 8, 3);
        std::fs::write(&p, &jpg).unwrap();
        assert_eq!(for_provider(&p).unwrap(), Some((jpg, "image/jpeg")));
        // Too tall for any provider: scaled to PROVIDER_MAX_SIDE.
        std::fs::write(&p, encoded(ImageFormat::Png, 120, 12_000, 3)).unwrap();
        let (bytes, mime) = for_provider(&p).unwrap().unwrap();
        assert_eq!(mime, "image/png");
        assert_eq!(
            image::load_from_memory(&bytes).unwrap().dimensions(),
            (80, 8000)
        );
        // Not an image (a PDF): left to its own path. Corrupt: an error.
        std::fs::write(&p, b"%PDF-1.7 not an image").unwrap();
        assert_eq!(for_provider(&p).unwrap(), None);
        let png = encoded(ImageFormat::Png, 8, 8, 3);
        std::fs::write(&p, &png[..png.len() - 20]).unwrap();
        assert!(for_provider(&p).is_err());
    }

    #[test]
    fn validate_accepts_real_images_and_rejects_the_rest() {
        assert_eq!(
            validate_bytes(&encoded(ImageFormat::Png, 2, 2, 1), "image/png"),
            Ok(())
        );
        assert_eq!(
            validate_bytes(&encoded(ImageFormat::Jpeg, 2, 2, 1), "image/jpeg"),
            Ok(())
        );
        assert_eq!(
            validate_bytes(&encoded(ImageFormat::Gif, 2, 2, 1), "image/gif"),
            Ok(())
        );
        assert_eq!(
            validate_bytes(&encoded(ImageFormat::WebP, 2, 2, 1), "image/webp"),
            Ok(())
        );
        // Label disagrees with content.
        assert_eq!(
            validate_bytes(&encoded(ImageFormat::Png, 2, 2, 1), "image/jpeg"),
            Err("content is image/png but was labeled image/jpeg".into())
        );
        // Unsupported label, even for a decodable format.
        assert!(
            validate_bytes(&encoded(ImageFormat::Png, 2, 2, 1), "image/bmp")
                .unwrap_err()
                .contains("not a supported image type")
        );
        // Not an image at all; a truncated one; an oversize one.
        assert_eq!(
            validate_bytes(b"hello", "image/png"),
            Err("not a recognized image".into())
        );
        let png = encoded(ImageFormat::Png, 8, 8, 1);
        assert!(
            validate_bytes(&png[..png.len() / 2], "image/png")
                .unwrap_err()
                .starts_with("invalid image/png")
        );
        let mut big = png.clone();
        big.resize(MAX_IMAGE_BYTES as usize + 1, 0);
        assert_eq!(
            validate_bytes(&big, "image/png"),
            Err("5.0 MB exceeds 5 MB".into())
        );
    }

    #[test]
    fn snapshot_captures_each_overwrite_distinctly() {
        let (_g, dir) = tmp();
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("shot.png");
        let out = dir.join("out");

        let a_bytes = encoded(ImageFormat::Png, 2, 2, 1);
        std::fs::write(&src, &a_bytes).unwrap();
        let a = snapshot(&src, &out).unwrap();
        assert_eq!(a.extension().unwrap(), "png");
        assert_eq!(std::fs::read(&a).unwrap(), a_bytes);

        // Same path, new content: a second file; the first still holds the old bytes.
        let b_bytes = encoded(ImageFormat::Png, 2, 2, 2);
        std::fs::write(&src, &b_bytes).unwrap();
        let b = snapshot(&src, &out).unwrap();
        assert_ne!(a, b);
        assert_eq!(std::fs::read(&a).unwrap(), a_bytes);
        assert_eq!(std::fs::read(&b).unwrap(), b_bytes);
        // Same content again: the same file, not a third.
        assert_eq!(snapshot(&src, &out).unwrap(), b);

        // Format comes from bytes: a JPEG named `.png` snapshots as `.jpg`.
        std::fs::write(&src, encoded(ImageFormat::Jpeg, 2, 2, 1)).unwrap();
        assert_eq!(snapshot(&src, &out).unwrap().extension().unwrap(), "jpg");

        // Not an image, missing, a directory, oversize.
        std::fs::write(&src, b"just text").unwrap();
        assert!(
            snapshot(&src, &out)
                .unwrap_err()
                .to_string()
                .contains("not a supported image")
        );
        assert!(snapshot(&dir.join("gone.png"), &out).is_err());
        assert!(
            snapshot(&dir, &out)
                .unwrap_err()
                .to_string()
                .contains("not a regular file")
        );
        let mut big = a_bytes.clone();
        big.resize(MAX_IMAGE_BYTES as usize + 1, 0);
        std::fs::write(&src, &big).unwrap();
        assert!(
            snapshot(&src, &out)
                .unwrap_err()
                .to_string()
                .contains("exceeds 5 MB")
        );
    }

    #[test]
    fn corrupted_existing_blob_is_replaced_not_served() {
        let (_g, dir) = tmp();
        let png = encoded(ImageFormat::Png, 2, 2, 1);
        let a = save(&png, "image/png", &dir).unwrap();
        // Truncated, tampered, and replaced by a non-file: each time the next
        // save restores the real bytes at the same content-addressed name.
        std::fs::write(&a, &png[..png.len() / 2]).unwrap();
        assert_eq!(save(&png, "image/png", &dir).unwrap(), a);
        assert_eq!(std::fs::read(&a).unwrap(), png);
        std::fs::write(&a, encoded(ImageFormat::Png, 2, 2, 9)).unwrap();
        assert_eq!(save(&png, "image/png", &dir).unwrap(), a);
        assert_eq!(std::fs::read(&a).unwrap(), png);
        std::fs::remove_file(&a).unwrap();
        std::fs::create_dir(&a).unwrap();
        assert!(save(&png, "image/png", &dir).is_err());
        std::fs::remove_dir(&a).unwrap();
        assert_eq!(save(&png, "image/png", &dir).unwrap(), a);
        // No temp files linger next to the blob.
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn fifo_is_refused_without_blocking() {
        let (_g, dir) = tmp();
        std::fs::create_dir_all(&dir).unwrap();
        let fifo = dir.join("shot.png");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        // Nobody ever writes to it: a blocking open would hang here forever.
        assert!(
            snapshot(&fifo, &dir.join("out"))
                .unwrap_err()
                .to_string()
                .contains("not a regular file")
        );
    }
}
