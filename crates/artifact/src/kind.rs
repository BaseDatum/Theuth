//! Kind by extension, mime, the CSV parse the TUI shares, and the one-line
//! receipt the model reads (35 §3.1–§3.2).

use std::path::{Path, PathBuf};

use crate::{Artifact, Kind};

/// Lower-cased extension of `name`, if any.
pub fn ext(name: &str) -> Option<String> {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
}

/// [`Kind`] from the extension of `name` (`.tar.gz`-style double suffixes
/// first, then the last extension); unknown → [`Kind::File`].
/// Never [`Kind::App`]: a `.openui` file shown the ordinary way is inert
/// text; only a validated [`crate::Commit`] makes an active app (46 §3.2).
pub fn of(name: &str) -> Kind {
    if crate::archive::format(name).is_some() {
        return Kind::Archive;
    }
    match ext(name).as_deref() {
        Some("md" | "markdown") => Kind::Markdown,
        Some("csv" | "tsv") => Kind::Csv,
        Some("png" | "jpg" | "jpeg" | "gif" | "webp") => Kind::Image,
        Some("svg") => Kind::Svg,
        Some("pdf") => Kind::Pdf,
        Some("html" | "htm") => Kind::Html,
        Some("mmd" | "mermaid") => Kind::Mermaid,
        Some(
            "txt" | "log" | "rs" | "py" | "ts" | "tsx" | "js" | "jsx" | "json" | "yaml" | "yml"
            | "toml" | "sh" | "bash" | "zsh" | "go" | "c" | "h" | "cpp" | "hpp" | "java" | "kt"
            | "swift" | "rb" | "php" | "sql" | "css" | "scss" | "xml" | "ini" | "cfg" | "conf"
            | "env" | "diff" | "patch" | "lua" | "zig" | "openui",
        ) => Kind::Text,
        _ => Kind::File,
    }
}

/// Mime type for `name` of `kind`: `mime_guess`, with text fallbacks by [`Kind`].
pub fn mime(kind: Kind, name: &str) -> String {
    let fallback = match kind {
        Kind::Markdown => "text/markdown",
        Kind::Text => "text/plain",
        Kind::Csv => "text/csv",
        Kind::Html => "text/html",
        Kind::Svg => "image/svg+xml",
        Kind::Pdf => "application/pdf",
        // `mime_guess` maps `.mmd` to a karaoke format; the diagram type wins.
        Kind::Mermaid => return "text/vnd.mermaid".to_owned(),
        Kind::Archive => match crate::archive::format(name) {
            Some(crate::archive::Format::Zip) => "application/zip",
            Some(crate::archive::Format::Tar) => "application/x-tar",
            _ => "application/gzip",
        },
        Kind::App => return "text/x-openui".to_owned(),
        Kind::Image | Kind::File => "application/octet-stream",
    };
    mime_guess::from_path(name)
        .first_raw()
        .unwrap_or(fallback)
        .to_owned()
}

/// `(header, first N data rows, total data rows)` from [`csv_table`].
pub type Table = (Vec<String>, Vec<Vec<String>>, usize);

/// Parse CSV (or TSV) bytes: `(header, first max_rows data rows, total data rows)`.
/// RFC 4180: quoted fields, embedded commas and newlines; ragged rows allowed.
pub fn csv_table(bytes: &[u8], tsv: bool, max_rows: usize) -> Result<Table, String> {
    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(if tsv { b'\t' } else { b',' })
        .flexible(true)
        .from_reader(bytes);
    let header: Vec<String> = rdr
        .headers()
        .map_err(|e| e.to_string())?
        .iter()
        .map(str::to_owned)
        .collect();
    let mut rows = Vec::new();
    let mut total = 0;
    for rec in rdr.records() {
        let rec = rec.map_err(|e| e.to_string())?;
        if total < max_rows {
            rows.push(rec.iter().map(str::to_owned).collect());
        }
        total += 1;
    }
    Ok((header, rows, total))
}

/// Numbers the receipt needs that the record does not carry; computed at show time.
#[derive(Clone, Debug, Default)]
pub struct Receipt {
    /// Line count for text kinds.
    pub lines: Option<usize>,
    /// `(rows, cols)` for CSV.
    pub table: Option<(usize, usize)>,
    /// `(width, height)` for pictures.
    pub dims: Option<(u32, u32)>,
    /// Entry count of an archive listing.
    pub files: Option<u64>,
    /// Typst compile warnings.
    pub warnings: Vec<String>,
    /// Local images inlined into an `.html` page (120 §8).
    pub images: Option<usize>,
    /// `show{height}` as `(asked, used)` after the 80–2000 clamp (120 §6).
    pub height: Option<(u32, u32)>,
    /// Why the kind was demoted to [`Kind::File`] by the ingest sniff (120 §9a).
    pub demoted: Option<&'static str>,
    /// The editable source file of a text kind: the draft, or the `path` shown (150 §D2).
    pub file: Option<PathBuf>,
    /// `show{edits}`: how many were applied (150 §D5); `Some(0)` = republished unchanged.
    pub edits: Option<usize>,
    /// `show{edits}` on an HTML page whose only base was the published blob: images already inlined.
    pub base_inlined: bool,
    /// The pre-publish page check of an `.html` page (150 §D4).
    pub check: Option<Check>,
}

/// How `show`'s page check ended (150 §D4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Check {
    /// A client rendered the page and its console had no `error` line.
    Ok {
        /// Who rendered (`desktop`, `web`, `phone`).
        client: &'static str,
        /// The reported height; `None` for the web client's unmeasured 80.
        height: Option<u32>,
        /// `warning` lines.
        warnings: usize,
    },
    /// Published unchecked: no renderer, the client failed, timed out, or never finished loading.
    Skipped(String),
}

/// `shown report.pdf (v1 · 3 pages · 84 KB) · source report.md · file <draft>`;
/// `shown archive.zip (v1 · 42 files · 3 MB)`; `shown flow.mmd (v1 · mermaid · 12 lines · 1 KB)`;
/// `shown chart.html (v2 · 212 lines · 48 KB · 2 images inlined · height 600 · 2 edits applied)
/// · checked on desktop: height 612 · file <draft>` + the audience line.
pub fn receipt(a: &Artifact, x: &Receipt) -> String {
    let mut parts = vec![format!("v{}", a.version)];
    if a.kind == Kind::Mermaid {
        parts.push("mermaid".to_owned());
    }
    if let Some(p) = a.pages {
        parts.push(format!("{p} pages"));
    }
    if let Some(n) = x.files {
        parts.push(format!("{n} files"));
    }
    if let Some(l) = x.lines {
        parts.push(format!("{l} lines"));
    }
    if let Some((r, c)) = x.table {
        parts.push(format!("{r} rows × {c} cols"));
    }
    match x.dims {
        Some((w, h)) => parts.push(format!("{w}×{h}")),
        None => parts.push(platform::fmt::bytes(a.bytes)),
    }
    if let Some(n) = x.images.filter(|n| *n > 0) {
        parts.push(format!(
            "{n} image{} inlined",
            if n == 1 { "" } else { "s" }
        ));
    }
    match x.height {
        Some((asked, used)) if asked != used => parts.push(format!("height {asked} → {used}")),
        Some((_, used)) => parts.push(format!("height {used}")),
        None => {}
    }
    if let Some(n) = x.edits.filter(|n| *n > 0) {
        parts.push(format!("{n} edit{} applied", if n == 1 { "" } else { "s" }));
    }
    let mut out = format!("shown {} ({})", a.name, parts.join(" · "));
    if let Some(name) = a.source.as_deref().and_then(Path::file_name) {
        out.push_str(&format!(" · source {}", name.to_string_lossy()));
    }
    if let Some(first) = x.warnings.first() {
        out.push_str(&format!(
            " · warnings: {} (first: {first})",
            x.warnings.len()
        ));
    }
    match &x.check {
        Some(Check::Ok {
            client,
            height,
            warnings,
        }) => {
            match height {
                Some(h) => out.push_str(&format!(" · checked on {client}: height {h}")),
                None => out.push_str(&format!(" · checked on {client} (not measured)")),
            }
            if *warnings > 0 {
                out.push_str(&format!(" · {warnings} warning(s)"));
            }
        }
        Some(Check::Skipped(why)) => out.push_str(&format!(" · not checked: {why}")),
        None => {}
    }
    if let Some(f) = &x.file {
        out.push_str(&format!(" · file {}", f.display()));
    }
    if x.base_inlined {
        out.push_str(" · edited the published page (images already inlined)");
    }
    if a.kind == Kind::Html {
        out.push_str(
            "\nlive page in the theuth app (desktop, web, phone); Slack/Telegram/Signal get the \
             file; TUI the source. To change it: show{name, edits} or edit() the file above and \
             show{path} — never resend the page.",
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_by_extension() {
        assert_eq!(of("README.md"), Kind::Markdown);
        assert_eq!(of("archive.tar.gz"), Kind::Archive);
        assert_eq!(of("a.tar.gz"), Kind::Archive);
        assert_eq!(of("A.ZIP"), Kind::Archive);
        assert_eq!(of("site.tgz"), Kind::Archive);
        assert_eq!(of("x.mmd"), Kind::Mermaid);
        assert_eq!(of("x.mermaid"), Kind::Mermaid);
        assert_eq!(of("Chart.SVG"), Kind::Svg);
        assert_eq!(of("main.rs"), Kind::Text);
        assert_eq!(of("noext"), Kind::File);
        assert_eq!(of("panel.openui"), Kind::Text);
        assert_eq!(mime(Kind::Text, "panel.openui"), "text/plain");
        assert_eq!(mime(Kind::App, "panel.openui"), "text/x-openui");
        assert_eq!(mime(Kind::Mermaid, "x.mmd"), "text/vnd.mermaid");
        assert_eq!(mime(Kind::Archive, "a.zip"), "application/zip");
        assert_eq!(mime(Kind::Archive, "a.tar"), "application/x-tar");
        assert_eq!(mime(Kind::Archive, "a.tar.gz"), "application/gzip");
    }

    #[test]
    fn kind_wire_round_trip() {
        for k in [
            Kind::Markdown,
            Kind::Text,
            Kind::Csv,
            Kind::Image,
            Kind::Svg,
            Kind::Pdf,
            Kind::Html,
            Kind::App,
            Kind::Archive,
            Kind::Mermaid,
            Kind::File,
        ] {
            assert_eq!(Kind::parse(k.as_str()), Some(k));
            assert_eq!(
                serde_json::to_string(&k).unwrap(),
                format!("\"{}\"", k.as_str())
            );
        }
        assert_eq!(Kind::parse("zip"), None);
    }

    fn record(name: &str, kind: Kind, bytes: u64) -> Artifact {
        Artifact {
            id: "0".repeat(16),
            session: runtime::SessionId::new(),
            call_id: "c".into(),
            name: name.into(),
            title: name.into(),
            kind,
            mime: mime(kind, name),
            bytes,
            version: 1,
            path: Path::new(name).to_path_buf(),
            source: None,
            preview: None,
            pages: None,
            origin: None,
            created_at: 0,
        }
    }

    #[test]
    fn receipts_by_kind() {
        let x = Receipt {
            files: Some(42),
            ..Receipt::default()
        };
        assert_eq!(
            receipt(&record("archive.zip", Kind::Archive, 3 * 1024 * 1024), &x),
            "shown archive.zip (v1 · 42 files · 3.1 MB)"
        );
        let x = Receipt {
            lines: Some(12),
            ..Receipt::default()
        };
        assert_eq!(
            receipt(&record("flow.mmd", Kind::Mermaid, 200), &x),
            "shown flow.mmd (v1 · mermaid · 12 lines · 200 B)"
        );
        assert_eq!(
            receipt(&record("notes.txt", Kind::Text, 200), &x),
            "shown notes.txt (v1 · 12 lines · 200 B)"
        );
        let x = Receipt {
            lines: Some(212),
            images: Some(2),
            height: Some((600, 600)),
            edits: Some(2),
            check: Some(Check::Ok {
                client: "desktop",
                height: Some(612),
                warnings: 1,
            }),
            file: Some(PathBuf::from("/d/drafts/s/chart.html")),
            ..Receipt::default()
        };
        assert_eq!(
            receipt(&record("chart.html", Kind::Html, 48 * 1024), &x),
            "shown chart.html (v1 · 212 lines · 49.2 kB · 2 images inlined · height 600 · 2 edits \
             applied) · checked on desktop: height 612 · 1 warning(s) · file \
             /d/drafts/s/chart.html\n\
             live page in the theuth app (desktop, web, phone); Slack/Telegram/Signal get the \
             file; TUI the source. To change it: show{name, edits} or edit() the file above and \
             show{path} — never resend the page."
        );
        let x = Receipt {
            lines: Some(1),
            images: Some(1),
            height: Some((3000, 2000)),
            edits: Some(0),
            check: Some(Check::Skipped("no renderer".into())),
            base_inlined: true,
            ..Receipt::default()
        };
        assert!(receipt(&record("p.html", Kind::Html, 10), &x).starts_with(
            "shown p.html (v1 · 1 lines · 10 B · 1 image inlined · height 3000 → 2000) · not \
             checked: no renderer · edited the published page (images already inlined)\n"
        ));
        let x = Receipt {
            check: Some(Check::Ok {
                client: "web",
                height: None,
                warnings: 0,
            }),
            ..Receipt::default()
        };
        assert!(
            receipt(&record("p.html", Kind::Html, 10), &x)
                .starts_with("shown p.html (v1 · 10 B) · checked on web (not measured)\n")
        );
    }

    #[test]
    fn csv_quoted_fields() {
        let src = b"a,b\n\"x, y\",\"line1\nline2\"\n1,2\n";
        let (h, rows, total) = csv_table(src, false, 1).unwrap();
        assert_eq!(h, ["a", "b"]);
        assert_eq!(rows, [["x, y", "line1\nline2"]]);
        assert_eq!(total, 2);
    }
}
