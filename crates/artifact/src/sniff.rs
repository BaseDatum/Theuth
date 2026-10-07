//! Ingest-time content checks (120 §9a): a `.pdf`, image or `.svg` name whose
//! bytes are not that format is demoted to [`Kind::File`] — a download chip on
//! every client — so the record never lies about what the viewer will do and an
//! HTML file under a `.pdf` name can never reach a document frame. `Html`
//! (meant to be scripted), `Text`, `Markdown` and `Csv` are never sniffed.

use crate::Kind;
use crate::inline;

/// `(kind, reason)`: the kind to store and, when demoted, the note the model reads.
pub fn demote(kind: Kind, bytes: &[u8]) -> (Kind, Option<&'static str>) {
    let reason = match kind {
        Kind::Pdf if !bytes.starts_with(b"%PDF-") => {
            "bytes are not a PDF (no %PDF- header); shown as a downloadable file"
        }
        Kind::Image
            if !matches!(
                inline::raster_mime(bytes),
                Some("image/png" | "image/jpeg" | "image/gif" | "image/webp")
            ) =>
        {
            "bytes are not a PNG, JPEG, GIF or WebP image; shown as a downloadable file"
        }
        Kind::Svg if !inline::has_svg_root(bytes) => {
            "bytes are not an SVG (no <svg> root element); shown as a downloadable file"
        }
        _ => return (kind, None),
    };
    (Kind::File, Some(reason))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdf_requires_magic_at_zero() {
        assert_eq!(demote(Kind::Pdf, b"%PDF-1.7\n"), (Kind::Pdf, None));
        assert_eq!(demote(Kind::Pdf, b"\n%PDF-1.7").0, Kind::File);
        assert_eq!(demote(Kind::Pdf, b"<!DOCTYPE html><script>").0, Kind::File);
    }

    #[test]
    fn image_kinds_accept_any_image_magic_and_record_mime() {
        for bytes in [
            &b"\x89PNG\r\n\x1a\n"[..],
            b"\xff\xd8\xff\xe1",
            b"GIF89a",
            b"RIFF\0\0\0\0WEBPVP8 ",
        ] {
            assert_eq!(demote(Kind::Image, bytes), (Kind::Image, None));
            assert!(inline::raster_mime(bytes).is_some());
        }
        // The name said `.png`; the mime the store records follows the bytes.
        assert_eq!(inline::raster_mime(b"\xff\xd8\xff\xe1"), Some("image/jpeg"));
        // BMP, ICO, AVIF and SVG are page images (§8) but not `Kind::Image` viewer formats.
        assert_eq!(demote(Kind::Image, b"BM\0\0\0\0\0\0\0\0").0, Kind::File);
        assert_eq!(demote(Kind::Image, b"<svg/>").0, Kind::File);
    }

    #[test]
    fn svg_requires_root() {
        assert_eq!(
            demote(Kind::Svg, b"<?xml version=\"1.0\"?>\n<svg/>").1,
            None
        );
        assert_eq!(
            demote(Kind::Svg, b"<html><body><svg/></body></html>").0,
            Kind::File
        );
        assert_eq!(demote(Kind::Svg, b"\x89PNG").0, Kind::File);
    }

    #[test]
    fn html_is_never_sniffed() {
        for kind in [
            Kind::Html,
            Kind::Text,
            Kind::Markdown,
            Kind::Csv,
            Kind::File,
            Kind::Archive,
        ] {
            assert_eq!(demote(kind, b"%PDF-1.7"), (kind, None));
            assert_eq!(demote(kind, b"\0\x01\x02"), (kind, None));
        }
    }

    #[test]
    fn mismatch_demotes_to_file_with_reason() {
        assert_eq!(
            demote(Kind::Pdf, b"<!DOCTYPE html>"),
            (
                Kind::File,
                Some("bytes are not a PDF (no %PDF- header); shown as a downloadable file")
            )
        );
        assert_eq!(
            demote(Kind::Image, b"ghp_secret"),
            (
                Kind::File,
                Some("bytes are not a PNG, JPEG, GIF or WebP image; shown as a downloadable file")
            )
        );
        assert_eq!(
            demote(Kind::Svg, b"<html/>"),
            (
                Kind::File,
                Some("bytes are not an SVG (no <svg> root element); shown as a downloadable file")
            )
        );
    }
}
