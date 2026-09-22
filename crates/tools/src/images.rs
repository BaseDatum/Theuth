//! Validated image files for tool output (17 §1, 52 §5.3/§7.1): the one
//! ingress every producer that turns bytes into a `ToolOutput.files` entry
//! goes through. Content is validated by decoding, never by extension or
//! MIME label; the file lands content-addressed under the caller's output
//! directory so an overwritten source path still yields a distinct snapshot.

use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use image::{ImageFormat, ImageReader, Limits};
use sha2::{Digest, Sha256};

/// Largest encoded image a provider accepts (Anthropic's 5 MB is the binding cap).
pub const MAX_IMAGE_BYTES: u64 = 5 * 1024 * 1024;

/// Largest side the decoder will consider; providers reject far smaller.
const MAX_SIDE: u32 = 10_000;

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
    let ext = format_of(mime)
        .map_err(anyhow::Error::msg)?
        .extensions_str()[0];
    let hex = format!("{:x}", Sha256::digest(bytes));
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

    fn tmp() -> PathBuf {
        std::env::temp_dir().join(format!("theuth-images-{}", uuid::Uuid::new_v4()))
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
        let dir = tmp();
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
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn corrupted_existing_blob_is_replaced_not_served() {
        let dir = tmp();
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
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn fifo_is_refused_without_blocking() {
        let dir = tmp();
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
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
