//! Kind by extension, mime, the CSV parse the TUI shares, and the one-line
//! receipt the model reads (35 §3.1–§3.2).

use std::path::Path;

use crate::{Artifact, Kind};

/// Lower-cased extension of `name`, if any.
pub fn ext(name: &str) -> Option<String> {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
}

/// [`Kind`] from the (last) extension of `name`; unknown → [`Kind::File`].
pub fn of(name: &str) -> Kind {
    match ext(name).as_deref() {
        Some("md" | "markdown") => Kind::Markdown,
        Some("csv" | "tsv") => Kind::Csv,
        Some("png" | "jpg" | "jpeg" | "gif" | "webp") => Kind::Image,
        Some("svg") => Kind::Svg,
        Some("pdf") => Kind::Pdf,
        Some("html" | "htm") => Kind::Html,
        Some(
            "txt" | "log" | "rs" | "py" | "ts" | "tsx" | "js" | "jsx" | "json" | "yaml" | "yml"
            | "toml" | "sh" | "bash" | "zsh" | "go" | "c" | "h" | "cpp" | "hpp" | "java" | "kt"
            | "swift" | "rb" | "php" | "sql" | "css" | "scss" | "xml" | "ini" | "cfg" | "conf"
            | "env" | "diff" | "patch" | "lua" | "zig",
        ) => Kind::Text,
        _ => Kind::File,
    }
}

/// Mime type for `name`: `mime_guess`, with text fallbacks by [`Kind`].
pub fn mime(name: &str) -> String {
    if let Some(m) = mime_guess::from_path(name).first_raw() {
        return m.to_owned();
    }
    match of(name) {
        Kind::Markdown => "text/markdown",
        Kind::Text => "text/plain",
        Kind::Csv => "text/csv",
        Kind::Html => "text/html",
        Kind::Svg => "image/svg+xml",
        Kind::Pdf => "application/pdf",
        Kind::Image | Kind::File => "application/octet-stream",
    }
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
    /// Typst compile warnings.
    pub warnings: Vec<String>,
}

/// `1024 B` → `1 KB`, `212 MB`.
pub fn human(bytes: u64) -> String {
    const K: u64 = 1024;
    match bytes {
        b if b < K => format!("{b} B"),
        b if b < K * K => format!("{} KB", b.div_ceil(K)),
        b => format!("{} MB", b.div_ceil(K * K)),
    }
}

/// `shown report.pdf (v1 · 3 pages · 84 KB) · source report.md`.
pub fn receipt(a: &Artifact, x: &Receipt) -> String {
    let mut parts = vec![format!("v{}", a.version)];
    if let Some(p) = a.pages {
        parts.push(format!("{p} pages"));
    }
    if let Some(l) = x.lines {
        parts.push(format!("{l} lines"));
    }
    if let Some((r, c)) = x.table {
        parts.push(format!("{r} rows × {c} cols"));
    }
    match x.dims {
        Some((w, h)) => parts.push(format!("{w}×{h}")),
        None => parts.push(human(a.bytes)),
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
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_by_extension() {
        assert_eq!(of("README.md"), Kind::Markdown);
        assert_eq!(of("archive.tar.gz"), Kind::File);
        assert_eq!(of("Chart.SVG"), Kind::Svg);
        assert_eq!(of("main.rs"), Kind::Text);
        assert_eq!(of("noext"), Kind::File);
    }

    #[test]
    fn csv_quoted_fields() {
        let src = b"a,b\n\"x, y\",\"line1\nline2\"\n1,2\n";
        let (h, rows, total) = csv_table(src, false, 1).unwrap();
        assert_eq!(h, ["a", "b"]);
        assert_eq!(rows, [["x, y", "line1\nline2"]]);
        assert_eq!(total, 2);
    }

    #[test]
    fn human_sizes() {
        assert_eq!(human(512), "512 B");
        assert_eq!(human(84 * 1024), "84 KB");
        assert_eq!(human(212 * 1024 * 1024 + 1), "213 MB");
    }
}
