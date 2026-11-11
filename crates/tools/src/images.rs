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
use image::metadata::Orientation;
use image::{DynamicImage, GenericImageView, ImageDecoder, ImageFormat, ImageReader, Limits};
use sha2::{Digest, Sha256};

/// Largest encoded image accepted at ingress ([`validate_bytes`], MCP, snapshots).
pub const MAX_IMAGE_BYTES: u64 = 5 * 1024 * 1024;

/// Largest encoded image [`fit`] sends: Bedrock's 3.75 MB, the lowest common
/// provider limit (Anthropic 5 MB, OpenAI 20 MB).
pub const SEND_MAX_BYTES: u64 = 3_750_000;

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

/// Decoder allocation cap for [`fit`] (an RGBA 8000×8000 is 256 MiB).
const DECODE_MAX_ALLOC: u64 = 256 * 1024 * 1024;

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

/// A known image format theuth cannot send (`TIFF`, `ICO`, `BMP`, `HEIC`,
/// `AVIF`), from its magic bytes; checked tightly enough that a text file
/// starting with "BM" is not one.
#[must_use]
pub fn unsupported_kind(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(b"II*\0") || b.starts_with(b"MM\0*") {
        return Some("TIFF");
    }
    if b.len() >= 6 && b.starts_with(&[0, 0, 1, 0]) && b[4..6] != [0, 0] {
        return Some("ICO");
    }
    if b.len() >= 14 && b.starts_with(b"BM") && b[6..10] == [0; 4] {
        return Some("BMP");
    }
    if b.len() >= 12 && &b[4..8] == b"ftyp" {
        return match &b[8..12] {
            b"heic" | b"heix" | b"hevc" | b"heim" | b"heis" | b"mif1" | b"msf1" => Some("HEIC"),
            b"avif" | b"avis" => Some("AVIF"),
            _ => None,
        };
    }
    None
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
/// 400 later) under side and allocation limits, and its EXIF orientation is
/// applied. A still PNG or JPEG whose long side is within `max_edge` and
/// that is at most [`SEND_MAX_BYTES`] goes unchanged; anything else is
/// re-encoded — GIF and WebP always, as a still PNG of the first frame
/// (they may be animated), a rotated photo upright — after scaling to a long
/// side of `max_edge`: JPEG stays JPEG, the rest becomes PNG, then JPEG
/// (alpha flattened onto white), then half the size while still over the
/// byte cap. Errors carry no payload text.
pub fn fit(bytes: &[u8], max_edge: u32) -> Result<Fitted<'_>, String> {
    let format = sniff(bytes).ok_or("not a supported image (png, jpeg, gif, webp)")?;
    let mime = format.to_mime_type();
    let invalid = |e: image::ImageError| format!("invalid {mime}: {e}");
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(FIT_MAX_SIDE);
    limits.max_image_height = Some(FIT_MAX_SIDE);
    limits.max_alloc = Some(DECODE_MAX_ALLOC);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(invalid)?;
    // `into_decoder` checks the sides only, and `from_decoder` allocates
    // `total_bytes` unchecked: a few-MB file claiming 20 000×20 000 would ask
    // for 1.6 GB. Refuse it here, before any pixel buffer exists.
    let need = decoder.total_bytes();
    if need > DECODE_MAX_ALLOC {
        return Err(format!(
            "{mime} would need {} MB to decode (limit {} MB)",
            need >> 20,
            DECODE_MAX_ALLOC >> 20
        ));
    }
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder).map_err(invalid)?;
    img.apply_orientation(orientation);
    let source = img.dimensions();
    let long = source.0.max(source.1);
    // An APNG carries `acTL` before its first `IDAT`: animated, like GIF/WebP.
    let animated_png = format == ImageFormat::Png
        && bytes[..bytes.len().min(64 * 1024)]
            .windows(4)
            .any(|w| w == b"acTL");
    let still = matches!(format, ImageFormat::Png | ImageFormat::Jpeg)
        && !animated_png
        && orientation == Orientation::NoTransforms;
    if still && long <= max_edge && bytes.len() as u64 <= SEND_MAX_BYTES {
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
    let (mut out, mut mime) = if format == ImageFormat::Jpeg && !img.color().has_alpha() {
        (jpeg(&img)?, "image/jpeg")
    } else {
        (png(&img)?, "image/png")
    };
    if out.len() as u64 > SEND_MAX_BYTES && mime == "image/png" {
        (out, mime) = (jpeg(&img)?, "image/jpeg");
    }
    while out.len() as u64 > SEND_MAX_BYTES && img.width().min(img.height()) > 64 {
        img = img.resize(img.width() / 2, img.height() / 2, FilterType::Triangle);
        out = jpeg(&img)?;
        mime = "image/jpeg";
    }
    if out.len() as u64 > SEND_MAX_BYTES {
        return Err(format!(
            "still over {:.2} MB after scaling",
            SEND_MAX_BYTES as f64 / 1e6
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

/// JPEG of `img`, alpha flattened onto white (JPEG has none).
fn jpeg(img: &DynamicImage) -> Result<Vec<u8>, String> {
    let rgb = if img.color().has_alpha() {
        let rgba = img.to_rgba8();
        image::RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
            let p = rgba.get_pixel(x, y).0;
            let a = u16::from(p[3]);
            image::Rgb(
                [p[0], p[1], p[2]].map(|c| ((u16::from(c) * a + 255 * (255 - a)) / 255) as u8),
            )
        })
    } else {
        img.to_rgb8()
    };
    let mut out = Vec::new();
    DynamicImage::ImageRgb8(rgb)
        .write_with_encoder(JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY))
        .map_err(|e| format!("jpeg encode: {e}"))?;
    Ok(out)
}

/// The bytes and MIME to send for the file at `path` (17 §3.1, 52 §5.3): the
/// one guard every attachment and tool image passes before it is stored and
/// sent. `None` when its magic bytes are not a supported image (a PDF goes
/// its own way); otherwise [`fit`] to [`PROVIDER_MAX_SIDE`] and
/// [`SEND_MAX_BYTES`], so nothing a provider rejects for size, animation or
/// a corrupt payload leaves. Blocking (decode/encode): callers run it on a
/// blocking thread.
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

    /// GIF and WebP may be animated (OpenAI takes still GIF only): even a
    /// small one is sent as a still PNG of its first frame.
    #[test]
    fn gif_and_webp_become_still_png() {
        for format in [ImageFormat::Gif, ImageFormat::WebP] {
            let src = encoded(format, 6, 4, 9);
            let f = fit(&src, MAX_EDGE).unwrap();
            assert_eq!(
                (f.mime, f.source, f.scaled),
                ("image/png", (6, 4), None),
                "{format:?}"
            );
            assert_eq!(sniff(&f.bytes), Some(ImageFormat::Png));
        }
    }

    /// A header claiming 65535×65535 is refused by the decoder limits before
    /// any pixel buffer is allocated.
    #[test]
    fn decompression_bomb_header_is_an_error() {
        let mut gif = b"GIF89a\xff\xff\xff\xff\x00\x00\x00".to_vec();
        gif.extend_from_slice(b",\x00\x00\x00\x00\xff\xff\xff\xff\x00\x02\x02D\x01\x00;");
        let e = fit(&gif, MAX_EDGE).unwrap_err();
        assert!(e.starts_with("invalid image/gif"), "{e}");
    }

    /// A tiny GIF whose header claims 20 000×20 000 passes the side limit but
    /// would need 1.6 GB: refused before any pixel buffer is allocated.
    pub(crate) fn gif_20k() -> Vec<u8> {
        let mut gif = b"GIF89a\x20\x4e\x20\x4e\x00\x00\x00".to_vec();
        gif.extend_from_slice(b",\x00\x00\x00\x00\x20\x4e\x20\x4e\x00\x02\x02D\x01\x00;");
        gif
    }

    #[test]
    fn header_over_the_allocation_cap_is_refused() {
        let e = fit(&gif_20k(), MAX_EDGE).unwrap_err();
        assert_eq!(e, "image/gif would need 1525 MB to decode (limit 256 MB)");
    }

    /// A JPEG 4×2 tagged EXIF orientation 6 (rotate 90° clockwise) comes out
    /// upright, 2×4, re-encoded rather than passed through.
    #[test]
    fn exif_orientation_is_applied() {
        let jpg = encoded(ImageFormat::Jpeg, 4, 2, 200);
        let tiff: &[u8] = b"MM\0*\0\0\0\x08\0\x01\x01\x12\0\x03\0\0\0\x01\0\x06\0\0\0\0\0\0";
        let mut app1 = vec![0xFF, 0xE1];
        app1.extend_from_slice(&((2 + 6 + tiff.len()) as u16).to_be_bytes());
        app1.extend_from_slice(b"Exif\0\0");
        app1.extend_from_slice(tiff);
        let rotated = [&jpg[..2], &app1, &jpg[2..]].concat();
        let f = fit(&rotated, MAX_EDGE).unwrap();
        assert_eq!(f.source, (2, 4));
        assert!(matches!(f.bytes, Cow::Owned(_)), "re-encoded upright");
        assert_eq!(
            image::load_from_memory(&f.bytes).unwrap().dimensions(),
            (2, 4)
        );
    }

    /// An APNG (`acTL` before `IDAT`) is re-encoded to a still PNG.
    #[test]
    fn apng_is_not_passed_through() {
        let png = encoded(ImageFormat::Png, 3, 3, 5);
        // acTL chunk (1 frame, play forever) right after IHDR (8-byte
        // signature + 25-byte IHDR chunk), with its real CRC.
        let body = [b"acTL".as_slice(), &[0, 0, 0, 1, 0, 0, 0, 0]].concat();
        let crc = body.iter().fold(!0u32, |c, &b| {
            (0..8).fold(c ^ u32::from(b), |c, _| {
                if c & 1 == 1 {
                    (c >> 1) ^ 0xEDB8_8320
                } else {
                    c >> 1
                }
            })
        });
        let actl = [&8u32.to_be_bytes()[..], &body, &(!crc).to_be_bytes()].concat();
        let apng = [&png[..33], &actl, &png[33..]].concat();
        let f = fit(&apng, MAX_EDGE).unwrap();
        assert!(matches!(f.bytes, Cow::Owned(_)));
        assert_eq!(f.mime, "image/png");
    }

    #[test]
    fn unsupported_kinds_are_named_and_text_is_not() {
        let bmp = [
            b"BM".as_slice(),
            &[0x46, 0, 0, 0, 0, 0, 0, 0, 0x36, 0, 0, 0],
        ]
        .concat();
        assert_eq!(unsupported_kind(&bmp), Some("BMP"));
        assert_eq!(unsupported_kind(b"II*\0\x08\0\0\0"), Some("TIFF"));
        assert_eq!(unsupported_kind(&[0, 0, 1, 0, 1, 0, 16, 16]), Some("ICO"));
        assert_eq!(
            unsupported_kind(b"\0\0\0\x18ftypheic\0\0\0\0"),
            Some("HEIC")
        );
        assert_eq!(unsupported_kind(b"BMW cars are fast, everyone says."), None);
        assert_eq!(unsupported_kind(&encoded(ImageFormat::Png, 2, 2, 1)), None);
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
