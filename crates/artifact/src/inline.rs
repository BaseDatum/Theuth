//! Image inlining for `Kind::Html` at store time (120 §8): local images the
//! page names by absolute path become `data:` URIs, so the page outlives the
//! files (`/tmp` screenshots, a phone through the relay, a Slack upload).
//! Pure over `stat`/`read` closures so tests need no disk.

use std::borrow::Cow;
use std::io;
use std::path::{Path, PathBuf};

use base64::Engine as _;

/// Largest single image.
pub const MAX_IMAGE: u64 = 10 * 1024 * 1024;
/// Largest inlined page (the artifact cap of 32 MiB stays above it).
pub const MAX_PAGE: u64 = 25 * 1024 * 1024;
/// Longest path text considered.
const MAX_PATH: usize = 2048;

/// One absolute local image path in the page: the byte range of the path text
/// (quotes / `url(` excluded), the path to read and the `data:` mime by extension.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalImage {
    /// Byte range of the path text in the source.
    pub range: std::ops::Range<usize>,
    /// Path to read (Windows backslashes un-escaped).
    pub path: PathBuf,
    /// Mime from the extension; browsers sniff images anyway.
    pub mime: &'static str,
}

/// Why `show` fails; the store prefixes `<name>: ` and the tool `show: `.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InlineError {
    /// Missing, unreadable, a directory or not an image — every path once.
    Unreadable(Vec<PathBuf>),
    /// One image over [`MAX_IMAGE`]: which, and its length.
    Oversize {
        /// The offending file.
        path: PathBuf,
        /// Its length.
        bytes: u64,
    },
    /// The inlined page would exceed [`MAX_PAGE`]: the projected or actual length.
    PageTooLarge {
        /// Projected (before any read) or actual page length.
        bytes: u64,
    },
}

impl std::fmt::Display for InlineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreadable(paths) => {
                let list: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
                write!(
                    f,
                    "these local images could not be read: {} — use absolute paths to existing \
                     image files, or remove them",
                    list.join(", ")
                )
            }
            Self::Oversize { path, bytes } => write!(
                f,
                "{} is {}; images are limited to {} MiB",
                path.display(),
                mib(*bytes),
                MAX_IMAGE >> 20
            ),
            Self::PageTooLarge { bytes } => write!(
                f,
                "inlined page would be {}; the limit is {} MiB",
                mib(*bytes),
                MAX_PAGE >> 20
            ),
        }
    }
}

impl std::error::Error for InlineError {}

/// `12.3 MiB`.
fn mib(bytes: u64) -> String {
    format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
}

/// The page with its images inlined and how many were.
#[derive(Debug)]
pub struct Inlined<'a> {
    /// `Borrowed` when the page named no local image.
    pub html: Cow<'a, [u8]>,
    /// Occurrences replaced.
    pub images: usize,
}

/// Mime of an allowed image extension (lower-cased, no dot).
fn mime_of_ext(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        _ => return None,
    })
}

/// `text` as an absolute local image path: POSIX `/…` (not `//…`) or Windows
/// `X:\…` / `X:/…` (JS-doubled backslashes un-escaped), ≤ 2048 bytes, no
/// newline, ending in an allowed extension. URLs, `data:`, `blob:`, `file:`
/// and relative paths never pass by construction.
fn parse_path(text: &[u8]) -> Option<(PathBuf, &'static str)> {
    if text.len() > MAX_PATH || text.iter().any(|b| matches!(b, b'\n' | b'\r')) {
        return None;
    }
    let text = std::str::from_utf8(text).ok()?;
    let windows = text.len() > 2
        && text.as_bytes()[0].is_ascii_alphabetic()
        && text.as_bytes()[1] == b':'
        && matches!(text.as_bytes()[2], b'\\' | b'/');
    let posix = text.starts_with('/') && !text.starts_with("//");
    if !windows && !posix {
        return None;
    }
    let ext = text.rsplit_once('.')?.1;
    if ext.contains(['/', '\\']) {
        return None;
    }
    let mime = mime_of_ext(&ext.to_ascii_lowercase())?;
    let path = if windows {
        text.replace("\\\\", "\\")
    } else {
        text.to_owned()
    };
    Some((PathBuf::from(path), mime))
}

/// Every local image reference in `html`, in source order: a whole quoted
/// string (`"…"`, `'…'`, `` `…` `` — attribute or JS literal) or an unquoted
/// CSS `url( … )` whose content [`parse_path`] accepts.
pub fn find_local_images(html: &[u8]) -> Vec<LocalImage> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < html.len() {
        let b = html[i];
        if matches!(b, b'"' | b'\'' | b'`') {
            let start = i + 1;
            let end = html[start..]
                .iter()
                .take(MAX_PATH + 1)
                .position(|&c| c == b || c == b'\n')
                .map(|n| start + n);
            if let Some(end) = end.filter(|&e| html[e] == b)
                && let Some((path, mime)) = parse_path(&html[start..end])
            {
                out.push(LocalImage {
                    range: start..end,
                    path,
                    mime,
                });
                i = end + 1;
                continue;
            }
        } else if html[i..].starts_with(b"url(") {
            let mut start = i + 4;
            while start < html.len() && html[start].is_ascii_whitespace() {
                start += 1;
            }
            // A quoted url() is the quote branch's.
            if start < html.len() && !matches!(html[start], b'"' | b'\'' | b'`') {
                let close = html[start..]
                    .iter()
                    .take(MAX_PATH + 1)
                    .position(|&c| c == b')')
                    .map(|n| start + n);
                if let Some(close) = close {
                    let mut end = close;
                    while end > start && html[end - 1].is_ascii_whitespace() {
                        end -= 1;
                    }
                    if !html[start..end].iter().any(u8::is_ascii_whitespace)
                        && let Some((path, mime)) = parse_path(&html[start..end])
                    {
                        out.push(LocalImage {
                            range: start..end,
                            path,
                            mime,
                        });
                        i = close + 1;
                        continue;
                    }
                }
            }
        }
        i += 1;
    }
    out
}

/// Mime of a raster image by magic: PNG, JPEG, GIF, WebP, AVIF/HEIF, BMP, ICO.
pub fn raster_mime(bytes: &[u8]) -> Option<&'static str> {
    let at = |i: usize, magic: &[u8]| bytes.get(i..i + magic.len()) == Some(magic);
    Some(if at(0, b"\x89PNG") {
        "image/png"
    } else if at(0, b"\xff\xd8\xff") {
        "image/jpeg"
    } else if at(0, b"GIF8") {
        "image/gif"
    } else if at(0, b"RIFF") && at(8, b"WEBP") {
        "image/webp"
    } else if at(4, b"ftyp") && (at(8, b"avif") || at(8, b"avis") || at(8, b"mif1")) {
        "image/avif"
    } else if at(0, b"BM") && at(6, b"\0\0\0\0") {
        "image/bmp"
    } else if at(0, b"\0\0\x01\0") {
        "image/x-icon"
    } else {
        return None;
    })
}

/// An XML document whose root element is `<svg`, after any BOM, prolog,
/// comments and doctype, in one forward pass.
pub fn has_svg_root(bytes: &[u8]) -> bool {
    let mut s = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    loop {
        while let Some((first, rest)) = s.split_first()
            && first.is_ascii_whitespace()
        {
            s = rest;
        }
        if s.starts_with(b"<?") {
            let Some(n) = find(&s[2..], b"?>") else {
                return false;
            };
            s = &s[2 + n + 2..];
        } else if s.starts_with(b"<!--") {
            let Some(n) = find(&s[4..], b"-->") else {
                return false;
            };
            s = &s[4 + n + 3..];
        } else if s.len() >= 9 && s[..9].eq_ignore_ascii_case(b"<!DOCTYPE") {
            // `>` closes it, except inside an internal subset `[ … ]`.
            let mut depth = 0usize;
            let mut i = 9;
            loop {
                match s.get(i) {
                    None => return false,
                    Some(b'[') => depth += 1,
                    Some(b']') => depth = depth.saturating_sub(1),
                    Some(b'>') if depth == 0 => break,
                    _ => {}
                }
                i += 1;
            }
            s = &s[i + 1..];
        } else {
            return s.starts_with(b"<svg")
                && s.get(4)
                    .is_some_and(|c| c.is_ascii_whitespace() || matches!(c, b'>' | b'/'));
        }
    }
}

/// Bytes are an image whatever the name (raster magic or SVG root), so a
/// renamed file cannot carry other data into a page.
pub fn is_image_bytes(bytes: &[u8]) -> bool {
    raster_mime(bytes).is_some() || has_svg_root(bytes)
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Replace every local image path in `html` with a `data:` URI. `stat` gives a
/// file's length, `read` its bytes. Order: every path must exist (all missing
/// listed at once); each ≤ [`MAX_IMAGE`]; the page budget is computed from the
/// lengths before any read and checked again after; bytes must be an image.
/// Nothing is written on `Err`.
pub fn inline_images<'a>(
    html: &'a [u8],
    stat: impl Fn(&Path) -> io::Result<u64>,
    read: impl Fn(&Path) -> io::Result<Vec<u8>>,
) -> Result<Inlined<'a>, InlineError> {
    let refs = find_local_images(html);
    if refs.is_empty() {
        return Ok(Inlined {
            html: Cow::Borrowed(html),
            images: 0,
        });
    }
    let mut unique: Vec<&Path> = Vec::new();
    for r in &refs {
        if !unique.contains(&r.path.as_path()) {
            unique.push(&r.path);
        }
    }
    let mut sizes: Vec<(&Path, u64)> = Vec::with_capacity(unique.len());
    let mut bad: Vec<PathBuf> = Vec::new();
    for p in &unique {
        match stat(p) {
            Ok(n) => sizes.push((p, n)),
            Err(_) => bad.push(p.to_path_buf()),
        }
    }
    if !bad.is_empty() {
        return Err(InlineError::Unreadable(bad));
    }
    if let Some((p, n)) = sizes.iter().find(|(_, n)| *n > MAX_IMAGE) {
        return Err(InlineError::Oversize {
            path: p.to_path_buf(),
            bytes: *n,
        });
    }
    let size_of = |p: &Path| sizes.iter().find(|(q, _)| *q == p).map_or(0, |(_, n)| *n);
    let projected = refs.iter().fold(html.len() as u64, |acc, r| {
        let encoded = base64::encoded_len(size_of(&r.path) as usize, true).unwrap_or(usize::MAX);
        acc + ("data:;base64,".len() + r.mime.len() + encoded) as u64 - r.range.len() as u64
    });
    if projected > MAX_PAGE {
        return Err(InlineError::PageTooLarge { bytes: projected });
    }
    let mut data: Vec<(&Path, String)> = Vec::with_capacity(unique.len());
    for p in &unique {
        match read(p) {
            Ok(bytes) if is_image_bytes(&bytes) && bytes.len() as u64 <= MAX_IMAGE => {
                let mime = refs
                    .iter()
                    .find(|r| r.path.as_path() == *p)
                    .map_or("", |r| r.mime);
                let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                data.push((p, format!("data:{mime};base64,{b64}")));
            }
            Ok(bytes) if bytes.len() as u64 > MAX_IMAGE => {
                return Err(InlineError::Oversize {
                    path: p.to_path_buf(),
                    bytes: bytes.len() as u64,
                });
            }
            _ => bad.push(p.to_path_buf()),
        }
    }
    if !bad.is_empty() {
        return Err(InlineError::Unreadable(bad));
    }
    let mut out = Vec::with_capacity(projected as usize);
    let mut cursor = 0;
    for r in &refs {
        out.extend_from_slice(&html[cursor..r.range.start]);
        let uri = &data
            .iter()
            .find(|(p, _)| *p == r.path.as_path())
            .expect("read above")
            .1;
        out.extend_from_slice(uri.as_bytes());
        cursor = r.range.end;
    }
    out.extend_from_slice(&html[cursor..]);
    if out.len() as u64 > MAX_PAGE {
        return Err(InlineError::PageTooLarge {
            bytes: out.len() as u64,
        });
    }
    Ok(Inlined {
        html: Cow::Owned(out),
        images: refs.len(),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

    fn disk(
        files: &[(&str, &[u8])],
    ) -> (
        impl Fn(&Path) -> io::Result<u64> + use<>,
        impl Fn(&Path) -> io::Result<Vec<u8>> + use<>,
    ) {
        let map: HashMap<PathBuf, Vec<u8>> = files
            .iter()
            .map(|(p, b)| (PathBuf::from(p), b.to_vec()))
            .collect();
        let m2 = map.clone();
        (
            move |p: &Path| {
                map.get(p)
                    .map(|b| b.len() as u64)
                    .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
            },
            move |p: &Path| {
                m2.get(p)
                    .cloned()
                    .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
            },
        )
    }

    fn paths(html: &str) -> Vec<String> {
        find_local_images(html.as_bytes())
            .into_iter()
            .map(|r| r.path.display().to_string())
            .collect()
    }

    #[test]
    fn absolute_paths_only() {
        let html = r#"<img src="https://x.org/a.png"> <img src="data:image/png;base64,AAAA">
            <img src='blob:https://x/a.png'> <img src="//host/x.png"> <img src="./x.png">
            <img src="file:///tmp/x.png"> <img src="x.png">
            <img src="/a/b.png"> <img src='/a/c.png'> <script>const u = `/a/d.png`;</script>
            <style>.x{background:url(/a/e.png)} .y{background:url( "/a/f.png" )}</style>"#;
        assert_eq!(
            paths(html),
            ["/a/b.png", "/a/c.png", "/a/d.png", "/a/e.png", "/a/f.png"]
        );
        // A path mentioned in prose between quotes is a path too; one in an unterminated
        // string on one line is not.
        assert_eq!(paths("<p>see \"/a/b.png\"</p>").len(), 1);
        assert!(paths("<p>\"/a/b.png\n\"</p>").is_empty());
        assert!(paths("x\"/a/b.png?v=1\"").is_empty());
    }

    #[test]
    fn windows_paths_unescape_in_js_strings() {
        let html =
            r#"<script>img.src = "C:\\Users\\sky\\shot.png";</script> <img src="D:/x/y.jpg">"#;
        assert_eq!(paths(html), [r"C:\Users\sky\shot.png", "D:/x/y.jpg"]);
        assert!(paths(r#""C:x.png""#).is_empty());
    }

    #[test]
    fn extension_allow_list() {
        for ext in [
            "png", "jpg", "JPEG", "gif", "webp", "avif", "svg", "bmp", "ico",
        ] {
            assert_eq!(paths(&format!("\"/a/b.{ext}\"")).len(), 1, "{ext}");
        }
        for ext in ["txt", "pdf", "html", "js", "pn", "png.bak"] {
            assert!(paths(&format!("\"/a/b.{ext}\"")).is_empty(), "{ext}");
        }
        assert!(paths("\"/a.png/b\"").is_empty());
        assert!(paths("\"/\"").is_empty());
        let long = format!("\"/{}.png\"", "a".repeat(MAX_PATH));
        assert!(paths(&long).is_empty());
    }

    #[test]
    fn svg_root_detected_after_prolog_comment_doctype() {
        let svg = b"\xEF\xBB\xBF<?xml version=\"1.0\"?>\n<!-- made by hand -->\n\
                    <!DOCTYPE svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\" \"x.dtd\" [ <!ENTITY a \">\"> ]>\n\
                    <svg xmlns=\"http://www.w3.org/2000/svg\"/>";
        assert!(has_svg_root(svg));
        assert!(has_svg_root(b"<svg>"));
        assert!(!has_svg_root(b"<svgfoo>"));
        assert!(!has_svg_root(b"<html><svg/></html>"));
        assert!(!has_svg_root(b"<!-- <svg/>"));
        assert!(!has_svg_root(b"PNG"));
        assert!(is_image_bytes(PNG));
        assert_eq!(raster_mime(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(raster_mime(b"\0\0\0\x1cftypavif\0"), Some("image/avif"));
        assert_eq!(raster_mime(b"BM\0\0\0\0\0\0\0\0"), Some("image/bmp"));
        assert_eq!(raster_mime(b"BM\0\0\0\0\x01\0\0\0"), None);
        assert_eq!(raster_mime(b"\0\0\x01\0"), Some("image/x-icon"));
        assert_eq!(raster_mime(b"\xff\xd8\xff\xe0"), Some("image/jpeg"));
        assert_eq!(raster_mime(b"GIF89a"), Some("image/gif"));
    }

    #[test]
    fn renamed_secret_is_not_an_image() {
        let (stat, read) = disk(&[("/tmp/token.png", b"ghp_secretsecret\n" as &[u8])]);
        let err = inline_images(br#"<img src="/tmp/token.png">"#, stat, read).unwrap_err();
        assert_eq!(err, InlineError::Unreadable(vec!["/tmp/token.png".into()]));
        assert!(err.to_string().starts_with(
            "these local images could not be read: /tmp/token.png — use absolute paths"
        ));
    }

    #[test]
    fn oversize_image_fails_before_read() {
        let reads = std::cell::Cell::new(0);
        let stat = |_: &Path| Ok(MAX_IMAGE + 1);
        let read = |_: &Path| {
            reads.set(reads.get() + 1);
            Ok(PNG.to_vec())
        };
        let err = inline_images(br#"<img src="/tmp/big.png">"#, stat, read).unwrap_err();
        assert_eq!(
            err,
            InlineError::Oversize {
                path: "/tmp/big.png".into(),
                bytes: MAX_IMAGE + 1
            }
        );
        assert_eq!(
            err.to_string(),
            "/tmp/big.png is 10.0 MiB; images are limited to 10 MiB"
        );
        assert_eq!(reads.get(), 0);
    }

    #[test]
    fn page_limit_counts_base64_growth() {
        // Three references to one 7 MiB file: 3 × ceil(7/3)·4 ≈ 28 MiB encoded > 25 MiB,
        // although the file alone is well under both limits.
        let reads = std::cell::Cell::new(0);
        let stat = |_: &Path| Ok(7 * 1024 * 1024);
        let read = |_: &Path| {
            reads.set(reads.get() + 1);
            Ok(PNG.to_vec())
        };
        let html = br#"<img src="/a.png"><img src="/a.png"><img src="/a.png">"#;
        let err = inline_images(html, stat, read).unwrap_err();
        assert!(
            matches!(err, InlineError::PageTooLarge { bytes } if bytes > MAX_PAGE),
            "{err:?}"
        );
        assert!(
            err.to_string()
                .starts_with("inlined page would be 28.0 MiB; the limit is 25 MiB")
        );
        assert_eq!(reads.get(), 0);
        // Two references fit.
        let stat = |_: &Path| Ok(7 * 1024 * 1024);
        let ok = inline_images(&html[..36], stat, read).unwrap();
        assert_eq!(ok.images, 2);
    }

    #[test]
    fn missing_file_lists_every_path_once() {
        let (stat, read) = disk(&[("/ok.png", PNG)]);
        let html = br#"<img src="/ok.png"><img src="/a.png"><img src='/b.svg'><img src="/a.png">"#;
        let err = inline_images(html, stat, read).unwrap_err();
        assert_eq!(
            err,
            InlineError::Unreadable(vec!["/a.png".into(), "/b.svg".into()])
        );
        assert_eq!(
            err.to_string(),
            "these local images could not be read: /a.png, /b.svg — use absolute paths to \
             existing image files, or remove them"
        );
    }

    #[test]
    fn offsets_splice_in_original_text() {
        let svg: &[u8] = b"<?xml version=\"1.0\"?><svg xmlns=\"http://www.w3.org/2000/svg\"/>";
        let (stat, read) = disk(&[("/a.png", PNG), ("/b.svg", svg)]);
        let html = "<!doctype html><img src=\"/a.png\" alt=\"x\"><style>i{background:url(/b.svg)}</style>\n\
                    <script>const s = `/a.png`; const t = 'https://x/a.png';</script>";
        let out = inline_images(html.as_bytes(), stat, read).unwrap();
        assert_eq!(out.images, 3);
        let png = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(PNG)
        );
        let svg = format!(
            "data:image/svg+xml;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(svg)
        );
        let want = format!(
            "<!doctype html><img src=\"{png}\" alt=\"x\"><style>i{{background:url({svg})}}</style>\n\
             <script>const s = `{png}`; const t = 'https://x/a.png';</script>"
        );
        assert_eq!(std::str::from_utf8(&out.html).unwrap(), want);
        // Nothing to inline: the bytes are borrowed, untouched.
        let (stat, read) = disk(&[]);
        let same = inline_images(b"<p>hi</p>", stat, read).unwrap();
        assert!(matches!(same.html, Cow::Borrowed(_)) && same.images == 0);
    }
}
