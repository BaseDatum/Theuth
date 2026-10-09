//! Note / board export (design doc 72): markdown as-is, or the print lowering compiled to PDF
//! through [`crate::pdf`]. The bytes go to `dest` (owner-only, atomic) or into the store as an
//! artifact for clients without a save dialog.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use runtime::SessionId;
use serde::{Deserialize, Serialize};

use crate::store::State;
use crate::{ShowArgs, Source, pdf};

/// Output format of [`ExportDoc`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    /// The document's lossy markdown, unchanged.
    Markdown,
    /// A4 PDF via Typst; fences become images (`assets`) or text.
    Pdf,
}

/// A diagram the client rendered: the SVG of the `json-render` fence in block `id`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ExportAsset {
    /// Block id of the fence.
    pub id: String,
    /// Self-contained SVG markup.
    pub svg: String,
}

/// What an export produced.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields = nullable))]
pub struct ExportResult {
    /// The file written, when `dest` was given.
    pub path: Option<PathBuf>,
    /// The stored artifact, when `dest` was not.
    pub artifact_id: Option<String>,
    /// Its record key (the artifact viewer's route).
    pub call_id: Option<String>,
    /// PDF page count.
    pub pages: Option<u32>,
    /// Typst warnings.
    pub warnings: Vec<String>,
}

/// `ask` → `Result<ExportResult, String>`. `markdown` is the document's text — annotated
/// (`<!-- id -->` per block) for PDF so `assets` can be matched to their fences. Runs off the
/// mailbox like [`crate::Show`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExportDoc {
    /// Note title: PDF title block and file stem.
    pub title: String,
    /// Output format.
    pub format: ExportFormat,
    /// The document's markdown.
    pub markdown: String,
    /// Owner-chosen file; `None` stores an artifact.
    pub dest: Option<PathBuf>,
    /// Client-rendered diagrams.
    pub assets: Vec<ExportAsset>,
    /// `(session, call_id)` to record a stored artifact under (the agent's `doc_export` call);
    /// `None` = [`EXPORT_SESSION`] with a generated key.
    pub record: Option<(SessionId, String)>,
}

/// The session export artifacts are recorded under: not a chat.
pub const EXPORT_SESSION: SessionId = SessionId(uuid::Uuid::nil());

/// Front matter + print markdown for `m`; asset SVGs written under `scratch`.
async fn print_source(m: &ExportDoc, scratch: &Path) -> Result<String, String> {
    let mut images = BTreeMap::new();
    if !m.assets.is_empty() {
        tokio::fs::create_dir_all(scratch)
            .await
            .map_err(|e| format!("{}: {e}", scratch.display()))?;
    }
    for a in &m.assets {
        let path = scratch.join(format!("{}.svg", file_stem(&a.id)));
        tokio::fs::write(&path, a.svg.as_bytes())
            .await
            .map_err(|e| format!("{}: {e}", path.display()))?;
        images.insert(a.id.clone(), path);
    }
    let body = without_title_heading(strip_front_matter(&m.markdown), &m.title);
    let toc = surface::export::heading_count(body) >= 3;
    let title = serde_json::to_string(&m.title).map_err(|e| e.to_string())?;
    Ok(format!(
        "---\ntitle: {title}\ndate: {}\ntoc: {toc}\nfont: Newsreader\n---\n{}",
        chrono::Local::now().format("%Y-%m-%d"),
        surface::export::print_markdown(body, &images)
    ))
}

/// `text` without a leading `---` block (a board's `root:`).
fn strip_front_matter(text: &str) -> &str {
    let Some(rest) = text.strip_prefix("---\n") else {
        return text;
    };
    match rest.find("\n---\n") {
        Some(i) => &rest[i + 5..],
        None => text,
    }
}

/// `body` without its first block when that is `# <title>` (a note is headed by its title, which the
/// PDF's title block already shows); an `<!-- id -->` annotation before it goes too.
fn without_title_heading<'a>(body: &'a str, title: &str) -> &'a str {
    let mut at = 0;
    for line in body.split_inclusive('\n') {
        let t = line.trim();
        if t.is_empty() || (t.starts_with("<!--") && t.ends_with("-->")) {
            at += line.len();
            continue;
        }
        return if t
            .strip_prefix("# ")
            .is_some_and(|h| h.trim() == title.trim())
        {
            &body[at + line.len()..]
        } else {
            body
        };
    }
    body
}

/// `title` as a file stem: path separators and control characters out, `note` when nothing is left.
pub fn file_stem(title: &str) -> String {
    let s: String = title
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '\0') {
                ' '
            } else {
                c
            }
        })
        .collect();
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let s = s.trim_matches('.').trim();
    if s.is_empty() {
        "note".to_owned()
    } else {
        s.chars().take(120).collect()
    }
}

impl State {
    /// Run `m`; asset scratch files live under `<artifacts>/export/<uuid>/` for the compile only.
    pub async fn export_doc(&self, m: ExportDoc) -> Result<ExportResult, String> {
        let scratch = self
            .dir
            .join("export")
            .join(uuid::Uuid::now_v7().to_string());
        let r = self.export_inner(m, &scratch).await;
        let _ = tokio::fs::remove_dir_all(&scratch).await;
        r
    }

    async fn export_inner(&self, m: ExportDoc, scratch: &Path) -> Result<ExportResult, String> {
        let (ext, content) = match m.format {
            ExportFormat::Markdown => ("md", m.markdown.clone()),
            ExportFormat::Pdf => ("pdf", print_source(&m, scratch).await?),
        };
        let Some(dest) = m.dest else {
            let args = ShowArgs {
                content: Some(content),
                name: Some(format!("{}.{ext}", file_stem(&m.title))),
                title: Some(m.title),
                source: Some(Source::Markdown),
                ..ShowArgs::default()
            };
            let (session, call_id) = m
                .record
                .unwrap_or_else(|| (EXPORT_SESSION, format!("export-{}", uuid::Uuid::now_v7())));
            let s = self.show_as(session, call_id, args, false).await?;
            return Ok(ExportResult {
                artifact_id: Some(s.artifact.id),
                call_id: Some(s.artifact.call_id),
                pages: s.artifact.pages,
                warnings: s.receipt.warnings,
                ..ExportResult::default()
            });
        };
        let (bytes, pages, warnings) = match m.format {
            ExportFormat::Markdown => (content.into_bytes(), None, Vec::new()),
            ExportFormat::Pdf => {
                let out = pdf::compile(&content, Source::Markdown, &self.fonts)
                    .await
                    .map_err(|e| format!("{e:#}"))?
                    .map_err(|d| format!("compile failed\n{}", pdf::format_diags(&d)))?;
                (out.pdf, Some(out.pages), out.warnings)
            }
        };
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        platform::files::write_private(&dest, &bytes)
            .map_err(|e| format!("{}: {e}", dest.display()))?;
        Ok(ExportResult {
            path: Some(dest),
            pages,
            warnings,
            ..ExportResult::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_stem_drops_separators_and_falls_back() {
        assert_eq!(
            file_stem("Step 1: commit/push"),
            "Step 1  commit push".replace("  ", " ")
        );
        assert_eq!(file_stem("  ..  "), "note");
        assert_eq!(file_stem("a\nb"), "a b");
    }

    #[test]
    fn the_title_heading_is_dropped_from_the_body() {
        assert_eq!(
            without_title_heading("<!-- a -->\n# T\n\n<!-- b -->\npara\n", "T"),
            "\n<!-- b -->\npara\n"
        );
        assert_eq!(
            without_title_heading("# Other\npara\n", "T"),
            "# Other\npara\n"
        );
        assert_eq!(without_title_heading("para\n# T\n", "T"), "para\n# T\n");
    }

    #[test]
    fn front_matter_is_stripped_only_when_closed() {
        assert_eq!(strip_front_matter("---\nroot: /x\n---\n# T\n"), "# T\n");
        assert_eq!(
            strip_front_matter("---\nroot: /x\n# T\n"),
            "---\nroot: /x\n# T\n"
        );
        assert_eq!(strip_front_matter("# T\n"), "# T\n");
    }
}

#[cfg(test)]
mod pipeline_tests {
    use super::*;
    use crate::Kind;
    use runtime::Runtime;

    const BOARD: &str = include_str!(
        "../../desktop/ui/src/components/board/fixtures/reference/surface-fence-render.md"
    );
    const SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 320 120" width="320" height="120"><rect x="-1" y="-1" width="322" height="122" fill="#fff"/><rect x="20" y="30" width="120" height="50" rx="6" fill="#f3eadf" stroke="#1a1612"/><text x="80" y="60" text-anchor="middle" font-family="sans-serif" font-size="14" fill="#1a1612">parser</text><path d="M140 55 L200 55" stroke="#1a1612" stroke-width="1.25"/></svg>"##;

    async fn open() -> (State, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let rt = Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
        let state = State::open(dir.path(), rt, Vec::new()).await.unwrap();
        (state, dir)
    }

    /// The fixture as the document actor annotates it: `<!-- fN -->` before every fence.
    fn annotated(md: &str) -> (String, Vec<String>) {
        let mut out = String::new();
        let mut ids = Vec::new();
        for line in md.split_inclusive('\n') {
            if line.starts_with("```json-render") {
                let id = format!("f{}", ids.len() + 1);
                out.push_str(&format!("<!-- {id} -->\n"));
                ids.push(id);
            }
            out.push_str(line);
        }
        (out, ids)
    }

    fn doc(format: ExportFormat, dest: Option<PathBuf>, assets: Vec<ExportAsset>) -> ExportDoc {
        let markdown = if format == ExportFormat::Pdf {
            annotated(BOARD).0
        } else {
            BOARD.to_owned()
        };
        ExportDoc {
            title: "Surface: fence render".into(),
            format,
            markdown,
            dest,
            assets,
            record: None,
        }
    }

    #[tokio::test]
    async fn board_exports_to_pdf_with_the_svg_asset_as_a_figure() {
        let (state, dir) = open().await;
        let (md, ids) = annotated(BOARD);
        let assets = vec![ExportAsset {
            id: ids[0].clone(),
            svg: SVG.into(),
        }];
        let scratch = dir.path().join("scratch");
        let source = print_source(&doc(ExportFormat::Pdf, None, assets.clone()), &scratch)
            .await
            .unwrap();
        assert!(
            source.starts_with("---\ntitle: \"Surface: fence render\"\n"),
            "{source}"
        );
        assert!(source.contains("toc: true"), "{source}");
        assert!(!source.contains("root:"), "{source}");
        assert!(!source.contains("<!--"), "{source}");
        let image = format!("![]({}/f1.svg)", scratch.display());
        assert!(source.contains(&image), "{source}");
        assert!(source.contains("*Graph: "), "{source}");
        // The other fences had no asset: text.
        assert!(
            source.contains("```rust\n// crates/surface/src/"),
            "{source}"
        );
        assert!(!source.contains("```json-render"), "{source}");
        drop(md);

        let dest = dir.path().join("out").join("board.pdf");
        let r = state
            .export_doc(doc(ExportFormat::Pdf, Some(dest.clone()), assets))
            .await
            .unwrap();
        assert_eq!(r.path.as_deref(), Some(dest.as_path()));
        assert!(r.pages.unwrap() >= 1, "{r:?}");
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        let bytes = std::fs::read(&dest).unwrap();
        assert!(bytes.starts_with(b"%PDF"));
        // Scratch svgs are gone once compiled.
        assert!(
            std::fs::read_dir(state.dir.join("export"))
                .map(|d| d.count() == 0)
                .unwrap_or(true)
        );
    }

    #[tokio::test]
    async fn pdf_without_a_dest_becomes_a_pdf_artifact_with_a_preview() {
        let (state, _dir) = open().await;
        let r = state
            .export_doc(doc(ExportFormat::Pdf, None, Vec::new()))
            .await
            .unwrap();
        let call_id = r.call_id.unwrap();
        assert!(call_id.starts_with("export-"));
        let a = state.record(&call_id).await.unwrap();
        assert_eq!(a.kind, Kind::Pdf);
        assert_eq!(a.session, EXPORT_SESSION);
        assert_eq!(a.title, "Surface: fence render");
        assert_eq!(a.name, "Surface fence render.pdf");
        assert!(a.preview.as_ref().is_some_and(|p| p.is_file()));
        assert_eq!(a.pages, r.pages);
    }

    #[tokio::test]
    async fn markdown_round_trips_to_a_file_and_to_an_artifact() {
        let (state, dir) = open().await;
        let dest = dir.path().join("note.md");
        let r = state
            .export_doc(doc(ExportFormat::Markdown, Some(dest.clone()), Vec::new()))
            .await
            .unwrap();
        assert_eq!(r.path.as_deref(), Some(dest.as_path()));
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), BOARD);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&dest).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let r = state
            .export_doc(doc(ExportFormat::Markdown, None, Vec::new()))
            .await
            .unwrap();
        let a = state.record(&r.call_id.unwrap()).await.unwrap();
        assert_eq!(a.kind, Kind::Markdown);
        assert_eq!(std::fs::read_to_string(&a.path).unwrap(), BOARD);
    }
}
