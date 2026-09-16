//! SVG → PNG for `.svg` artifacts (design doc 35 §3.2, §9): the card preview and the model's view.

use std::sync::{Arc, LazyLock};

use anyhow::{Context as _, anyhow};
use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg::{self, ImageHrefResolver, Options, Tree};

/// A rasterised SVG.
#[derive(Clone, Debug)]
pub struct Raster {
    /// PNG bytes.
    pub png: Vec<u8>,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

/// Longest edge in pixels (17 §1).
const MAX_EDGE: f32 = 1568.0;

/// System fonts for `<text>`, loaded once (a scan of the font directories).
static FONTS: LazyLock<Arc<usvg::fontdb::Database>> = LazyLock::new(|| {
    let mut db = usvg::fontdb::Database::new();
    db.load_system_fonts();
    Arc::new(db)
});

/// Rasterise SVG bytes to PNG, long edge ≤ 1568 px. `<image href>` may only be a `data:` URL:
/// file paths and remote URLs resolve to nothing and the element is dropped, no request is made.
pub fn rasterize(svg: &[u8]) -> anyhow::Result<Raster> {
    let opt = Options {
        image_href_resolver: ImageHrefResolver {
            resolve_data: ImageHrefResolver::default_data_resolver(),
            resolve_string: Box::new(|_, _| None),
        },
        fontdb: FONTS.clone(),
        ..Options::default()
    };
    let tree = Tree::from_data(svg, &opt).context("parse svg")?;
    let size = tree.size();
    let scale = (MAX_EDGE / size.width().max(size.height())).min(2.0);
    let width = (size.width() * scale).round().max(1.0) as u32;
    let height = (size.height() * scale).round().max(1.0) as u32;
    let mut pixmap = Pixmap::new(width, height)
        .ok_or_else(|| anyhow!("svg size {width}x{height} is not renderable"))?;
    resvg::render(
        &tree,
        Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let png = pixmap.encode_png().context("encode png")?;
    Ok(Raster { png, width, height })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/svg");

    fn png_dims(png: &[u8]) -> (u32, u32) {
        let be = |i: usize| u32::from_be_bytes([png[i], png[i + 1], png[i + 2], png[i + 3]]);
        (be(16), be(20))
    }

    #[test]
    fn fixtures_rasterise_within_bounds() {
        for name in ["chart.svg", "wide.svg"] {
            let svg = std::fs::read(format!("{FIXTURES}/{name}")).expect(name);
            let r = rasterize(&svg).expect(name);
            assert_eq!(png_dims(&r.png), (r.width, r.height), "{name}");
            assert!(
                r.width.max(r.height) <= 1568 && r.width.min(r.height) > 0,
                "{name}: {}x{}",
                r.width,
                r.height
            );
        }
        let wide =
            rasterize(&std::fs::read(format!("{FIXTURES}/wide.svg")).expect("wide")).expect("wide");
        assert_eq!(
            wide.width, 1568,
            "a 4000 px wide drawing is scaled down to the long edge"
        );
    }

    #[test]
    fn remote_href_is_refused_not_fetched() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100">
  <rect width="200" height="100" fill="#eee"/>
  <image href="https://example.com/x.png" width="200" height="100"/>
  <image href="/etc/hosts" width="10" height="10"/>
</svg>"##;
        let r = rasterize(svg).expect("renders without the image");
        assert_eq!((r.width, r.height), (400, 200));
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(rasterize(b"<svg").is_err());
    }
}
