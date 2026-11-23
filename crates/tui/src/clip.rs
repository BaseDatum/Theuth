//! Image and document attachments for the input box (design doc 17 §4, 20
//! §2.5): the OS clipboard image behind Ctrl+V, and a dropped image or PDF
//! path pasted as text. The I/O half of what [`crate::model::Paste::Image`]
//! holds; `model.rs` stays pure.

use std::path::{Path, PathBuf};

/// A pasted image or PDF on disk and its pixel size (0×0 when not decoded, i.e. a dropped file).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    /// Where the bytes live; sent to the agent as-is.
    pub path: PathBuf,
    /// Width in pixels; 0 when unknown.
    pub w: u32,
    /// Height in pixels; 0 when unknown.
    pub h: u32,
}

/// Long edge a pasted image is downscaled to (Anthropic's recommendation; larger is downscaled server-side).
pub const MAX_EDGE: u32 = 1568;

/// Largest image attached from disk (the binding provider cap, 17 §1).
const MAX_BYTES: u64 = 5 * 1024 * 1024;
/// Largest PDF attached from disk (Anthropic's document cap, 20 §2.5).
const MAX_PDF_BYTES: u64 = 32 * 1024 * 1024;

/// Extensions taken as an image when pasted as a path.
const IMAGE_EXT: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];

/// The clipboard image as a PNG under `<data_dir>/attachments/<sha256[..16]>.png`,
/// long edge ≤ [`MAX_EDGE`] (integer box downscale). `Ok(None)` when the
/// clipboard holds no image.
pub fn image(data_dir: &Path) -> anyhow::Result<Option<Image>> {
    let img = match arboard::Clipboard::new()?.get_image() {
        Ok(img) => img,
        Err(arboard::Error::ContentNotAvailable) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let (w, h) = (u32::try_from(img.width)?, u32::try_from(img.height)?);
    let f = w.max(h).div_ceil(MAX_EDGE);
    let (w, h, bytes) = if f > 1 {
        let (w, h, px) = downscale(&img.bytes, w, h, f);
        (w, h, encode_png(w, h, &px))
    } else {
        (w, h, encode_png(w, h, &img.bytes))
    };
    let dir = data_dir.join("attachments");
    std::fs::create_dir_all(&dir)?;
    let name = &platform::sha256::hex(&bytes)[..16];
    let path = dir.join(format!("{name}.png"));
    if !path.exists() {
        std::fs::write(&path, &bytes)?;
    }
    Ok(Some(Image { path, w, h }))
}

/// Box-downscale RGBA8 `px` (w×h) by integer factor `f`: each output pixel
/// is the average of an f×f block (partial edge blocks averaged over the
/// pixels present). Returns (w/f rounded up, h/f rounded up, pixels).
// ponytail: integer factor only; a proper resampler needs `image`, which 13 §0.4 chose not to depend on.
fn downscale(px: &[u8], w: u32, h: u32, f: u32) -> (u32, u32, Vec<u8>) {
    let (ow, oh) = (w.div_ceil(f), h.div_ceil(f));
    let mut out = Vec::with_capacity((ow * oh * 4) as usize);
    for oy in 0..oh {
        for ox in 0..ow {
            let (x0, y0) = (ox * f, oy * f);
            let (x1, y1) = ((x0 + f).min(w), (y0 + f).min(h));
            let n = (x1 - x0) * (y1 - y0);
            let mut sum = [0u32; 4];
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = ((y * w + x) * 4) as usize;
                    for (c, s) in sum.iter_mut().enumerate() {
                        *s += u32::from(px[i + c]);
                    }
                }
            }
            out.extend(sum.iter().map(|s| ((s + n / 2) / n) as u8));
        }
    }
    (ow, oh, out)
}

fn encode_png(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().expect("png header");
    writer.write_image_data(rgba).expect("png data");
    writer.finish().expect("png finish");
    out
}

/// `text` as a dropped file: one line, shell quotes / backslash-escaped
/// spaces removed, an existing regular file with a png/jpg/jpeg/gif/webp
/// extension (case-insensitive) of at most 5 MB, or a pdf of at most 32 MB.
/// `None` otherwise.
pub fn dropped_file(text: &str) -> Option<Image> {
    let line = text.trim();
    if line.is_empty() || line.contains(['\n', '\r']) {
        return None;
    }
    let unquoted = line
        .strip_prefix('\'')
        .and_then(|s| s.strip_suffix('\''))
        .or_else(|| line.strip_prefix('"').and_then(|s| s.strip_suffix('"')))
        .unwrap_or(line)
        .replace("\\ ", " ");
    let path = PathBuf::from(unquoted);
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let cap = if IMAGE_EXT.contains(&ext.as_str()) {
        MAX_BYTES
    } else if ext == "pdf" {
        MAX_PDF_BYTES
    } else {
        return None;
    };
    let meta = std::fs::metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > cap {
        return None;
    }
    Some(Image { path, w: 0, h: 0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downscale_averages_blocks_and_missing_files_are_not_images() {
        // 4×2 RGBA: left 2×2 block a 0/255 checker, right block constant 100.
        let row = |a: u8, b: u8| {
            [
                a, a, a, 255, b, b, b, 255, 100, 100, 100, 255, 100, 100, 100, 255,
            ]
        };
        let px: Vec<u8> = [row(0, 255), row(255, 0)].concat();
        let (w, h, out) = downscale(&px, 4, 2, 2);
        assert_eq!((w, h), (2, 1));
        assert_eq!(out, vec![128, 128, 128, 255, 100, 100, 100, 255]);
        assert_eq!(dropped_file("'/definitely/missing.png'"), None);
    }
}
