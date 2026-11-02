//! PNG encoding with a long-edge cap and the provider's size cap, shared by
//! `screen_context` (96 §6.6) and the clipboard image (161 §5). Moved here
//! unchanged from `context.rs`, with fitting split from encoding so a
//! caller can learn the size it got.

use std::io::Cursor;

use image::imageops::{self, FilterType};
use image::{DynamicImage, ImageFormat, RgbaImage};

/// Largest image a provider accepts (`check_files`, 17 §1).
pub(crate) const MAX_PNG: usize = 5 * 1024 * 1024;

/// PNG of `img` with its long edge ≤ `max_edge`, shrunk further until it is
/// under the provider cap.
pub(crate) fn encode_bounded(img: &RgbaImage, max_edge: u32) -> Result<Vec<u8>, String> {
    bounded(img, max_edge).map(|(png, _)| png)
}

/// [`encode_bounded`], plus the encoded image's `(width, height)`.
pub(crate) fn bounded(img: &RgbaImage, max_edge: u32) -> Result<(Vec<u8>, (u32, u32)), String> {
    let mut edge = max_edge.max(1);
    loop {
        let fitted = fit(img, edge);
        let png = encode(&fitted)?;
        if png.len() <= MAX_PNG || edge <= 256 {
            return Ok((png, (fitted.width(), fitted.height())));
        }
        edge = edge * 3 / 4;
    }
}

/// `img` with its long edge ≤ `max_edge`.
fn fit(img: &RgbaImage, max_edge: u32) -> DynamicImage {
    let long = img.width().max(img.height());
    if long > max_edge {
        let k = max_edge as f32 / long as f32;
        let w = ((img.width() as f32 * k).round() as u32).max(1);
        let h = ((img.height() as f32 * k).round() as u32).max(1);
        DynamicImage::ImageRgba8(imageops::resize(img, w, h, FilterType::Triangle))
    } else {
        DynamicImage::ImageRgba8(img.clone())
    }
}

fn encode(img: &DynamicImage) -> Result<Vec<u8>, String> {
    let mut png = Vec::new();
    img.write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(png)
}
