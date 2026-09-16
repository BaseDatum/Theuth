//! Markdown / Typst → PDF through an in-process Typst engine (design doc 35 §4).
//!
//! Markdown goes through the vendored `cmarker` package inside `typst/document.typ`; Typst is the
//! main file as written. Both share one [`TypstEngine`] (fonts are scanned once, ~2 s) with static
//! resolvers for the template and package and a dynamic one ([`Files`]) for the per-call source
//! plus images on disk. Compile errors come back located in the model's `content`.

use std::borrow::Cow;
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, RwLock};
use std::time::Duration;

use anyhow::{Context as _, anyhow};
use gray_matter::engine::Engine as _;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use typst::diag::{FileError, FileResult, SourceDiagnostic, Warned};
use typst::foundations::{Bytes, Dict, IntoValue, Label, Selector};
use typst::introspection::{Introspector as _, MetadataElem};
use typst::syntax::{DiagSpan, FileId, Source as TypstSource, VirtualRoot};
use typst::utils::Scalar;
use typst::{World as _, WorldExt as _};
use typst_as_lib::conversions::IntoFileId;
use typst_as_lib::file_resolver::FileResolver;
use typst_as_lib::typst_kit_options::TypstKitFontOptions;
use typst_as_lib::{TypstEngine, TypstWorld};
use typst_layout::{Page, PagedDocument};
use typst_render::RenderOptions;

/// Source language of a `.pdf` artifact's `content`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// CommonMark with optional YAML front matter, rendered through cmarker in our template.
    #[default]
    Markdown,
    /// Raw Typst; `content` is the main file, no template.
    Typst,
}

/// One compile error, located in the model's `content` (`line` 0 = location unknown).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diag {
    /// 1-based line in `content`; 0 when the error could not be placed.
    pub line: u32,
    /// 1-based column (in characters).
    pub col: u32,
    /// Typst's message; `template: ` prefix when the fault is in our template, not the content.
    pub message: String,
    /// Typst's hints (`did you mean …`).
    pub hints: Vec<String>,
}

/// A compiled document.
#[derive(Clone, Debug)]
pub struct Compiled {
    /// The PDF bytes.
    pub pdf: Vec<u8>,
    /// Page 1 as PNG, long edge ≤ 1568 px.
    pub preview_png: Vec<u8>,
    /// Page count.
    pub pages: u32,
    /// Typst warnings (messages only), e.g. an unknown font family.
    pub warnings: Vec<String>,
}

/// Longest preview edge in pixels (17 §1).
const MAX_EDGE: f64 = 1568.0;
/// Wall-clock budget for one compile, PDF export and page render.
const DEADLINE: Duration = Duration::from_secs(30);
/// Diagnostics printed by [`format_diags`] before `… and N more`.
const MAX_DIAGS: usize = 8;

static DOCUMENT: &str = include_str!("../typst/document.typ");
static TRANSPILE: &str = include_str!("../typst/transpile.typ");
static CMARKER: &str = include_str!("../typst/cmarker/lib.typ");
static CMARKER_WASM: &[u8] = include_bytes!("../typst/cmarker/plugin.wasm");

/// Compile `content` to PDF + page-1 PNG.
///
/// Outer `Err` = deadline / panic / I/O; inner `Err(diags)` = the model's fault, located in
/// `content`. `extra_fonts` are searched in addition to the embedded and system fonts; they are
/// read on the first call only (the engine is built once per process).
pub async fn compile(
    content: &str,
    source: Source,
    extra_fonts: &[PathBuf],
) -> anyhow::Result<Result<Compiled, Vec<Diag>>> {
    let content = content.to_owned();
    let fonts = extra_fonts.to_vec();
    // Typst has no cancellation: past the deadline the blocking task runs to completion on its own
    // thread and its result is dropped.
    let work = tokio::task::spawn_blocking(move || compile_blocking(&content, source, &fonts));
    match tokio::time::timeout(DEADLINE, work).await {
        Ok(Ok(result)) => result,
        Ok(Err(join)) => Err(anyhow!("compiler panicked: {join}")),
        Err(_) => Err(anyhow!("compile exceeded {} s", DEADLINE.as_secs())),
    }
}

/// `"{line}:{col}: {message}"` per diag with `hint: …` lines under each, at most eight diags then
/// `… and N more`.
pub fn format_diags(diags: &[Diag]) -> String {
    let mut out = String::new();
    for d in diags.iter().take(MAX_DIAGS) {
        out.push_str(&format!("{}:{}: {}\n", d.line, d.col, d.message));
        for h in &d.hints {
            out.push_str(&format!("  hint: {h}\n"));
        }
    }
    if diags.len() > MAX_DIAGS {
        out.push_str(&format!("… and {} more\n", diags.len() - MAX_DIAGS));
    }
    out
}

/// The `pdf` skill body (front matter `name: pdf`), for `wth` to write to `<data_dir>/skills/pdf.md`.
pub fn skill() -> String {
    include_str!("../pdf.md").to_owned()
}

fn compile_blocking(
    content: &str,
    source: Source,
    fonts: &[PathBuf],
) -> anyhow::Result<Result<Compiled, Vec<Diag>>> {
    let engine = engine(fonts);
    let (output, warnings) = match source {
        Source::Markdown => {
            let (matter, body, skipped) = split_front_matter(content);
            let meta = match front_matter(matter) {
                Ok(meta) => meta,
                Err(message) => {
                    return Ok(Err(vec![Diag {
                        line: 1,
                        col: 1,
                        message,
                        hints: Vec::new(),
                    }]));
                }
            };
            engine.markdown(meta, body, skipped)?
        }
        Source::Typst => engine.typst(content)?,
    };
    let doc = match output {
        Ok(doc) => doc,
        Err(diags) => return Ok(Err(diags)),
    };
    let pdf = typst_pdf::pdf(&doc, &typst_pdf::PdfOptions::default()).map_err(|errs| {
        anyhow!(
            "pdf export: {}",
            errs.iter()
                .map(|e| e.message.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        )
    })?;
    let page = doc
        .pages()
        .first()
        .ok_or_else(|| anyhow!("document has no pages"))?;
    Ok(Ok(Compiled {
        pdf,
        preview_png: preview(page)?,
        pages: u32::try_from(doc.pages().len()).unwrap_or(u32::MAX),
        warnings,
    }))
}

fn preview(page: &Page) -> anyhow::Result<Vec<u8>> {
    let size = page.frame.size();
    let long = size.x.to_pt().max(size.y.to_pt()).max(1.0);
    // −0.5 so typst-render's rounding never lands on 1569; 4 px/pt caps tiny pages.
    let ppp = ((MAX_EDGE - 0.5) / long).min(4.0);
    let opts = RenderOptions {
        pixel_per_pt: Scalar::new(ppp),
        ..RenderOptions::default()
    };
    typst_render::render(page, &opts)
        .encode_png()
        .context("encode preview png")
}

/// Front matter keys a markdown document may set (35 §4.1).
#[derive(Deserialize, Default)]
struct Matter {
    title: Option<String>,
    author: Option<String>,
    date: Option<String>,
    paper: Option<String>,
    #[serde(default)]
    toc: bool,
}

/// `(front matter yaml, body, lines the body is offset by)`; no `---` block → the whole content.
fn split_front_matter(content: &str) -> (Option<&str>, &str, usize) {
    let mut lines = content.split_inclusive('\n');
    let Some(first) = lines.next().filter(|l| l.trim_end() == "---") else {
        return (None, content, 0);
    };
    let mut end = first.len();
    for (n, line) in lines.enumerate() {
        let start = end;
        end += line.len();
        if line.trim_end() == "---" {
            return (Some(&content[first.len()..start]), &content[end..], n + 2);
        }
    }
    (None, content, 0)
}

/// Front matter → the `meta` dict `document.typ` reads; `date` defaults to today, `paper` to a4.
fn front_matter(yaml: Option<&str>) -> Result<Dict, String> {
    let matter = match yaml {
        Some(y) if !y.trim().is_empty() => gray_matter::engine::YAML::parse(y)
            .and_then(|pod| pod.deserialize::<Matter>())
            .map_err(|e| format!("front matter: {e}"))?,
        _ => Matter::default(),
    };
    let mut meta = Dict::new();
    meta.insert("title".into(), matter.title.into_value());
    meta.insert("author".into(), matter.author.into_value());
    meta.insert(
        "date".into(),
        matter
            .date
            .unwrap_or_else(|| chrono::Local::now().format("%Y-%m-%d").to_string())
            .into_value(),
    );
    meta.insert(
        "paper".into(),
        matter.paper.unwrap_or_else(|| "a4".to_owned()).into_value(),
    );
    meta.insert("toc".into(), matter.toc.into_value());
    Ok(meta)
}

static ENGINE: OnceLock<Engine> = OnceLock::new();

/// Built once: the system font scan is the expensive part (~2 s); resolvers are immutable, so
/// per-call sources go through [`Files`] slots instead of a per-call engine. `OnceLock`, not
/// `LazyLock`: the font directories are runtime config.
fn engine(extra_fonts: &[PathBuf]) -> &'static Engine {
    ENGINE.get_or_init(|| {
        let files = Files::default();
        let fonts = TypstKitFontOptions::new()
            .include_system_fonts(true)
            .include_embedded_fonts(true)
            .include_dirs(extra_fonts.iter().cloned());
        let typst = TypstEngine::builder()
            .search_fonts_with(fonts)
            .with_static_source_file_resolver([
                ("/document.typ", DOCUMENT),
                ("/transpile.typ", TRANSPILE),
                ("/cmarker/lib.typ", CMARKER),
            ])
            .with_static_file_resolver([("/cmarker/plugin.wasm", CMARKER_WASM)])
            .add_file_resolver(files.clone())
            .build();
        Engine {
            typst,
            document: "/document.typ".into_file_id(),
            transpile: "/transpile.typ".into_file_id(),
            files,
        }
    })
}

struct Engine {
    typst: TypstEngine,
    document: FileId,
    transpile: FileId,
    files: Files,
}

type Outcome = (Result<PagedDocument, Vec<Diag>>, Vec<String>);

impl Engine {
    /// Markdown through `document.typ`; `skipped` = front-matter lines to add back to diag lines.
    fn markdown(&self, mut meta: Dict, body: &str, skipped: usize) -> anyhow::Result<Outcome> {
        meta.insert("body".into(), body.into_value());
        let mut inputs = Dict::new();
        inputs.insert("meta".into(), meta.into_value());
        let world = self
            .typst
            .world_builder(self.document)
            .with_inputs(inputs)
            .build()?;
        let Warned { output, warnings } = typst::compile::<PagedDocument>(&world);
        let result = output.map_err(|errs| {
            // Errors from the body surface in cmarker's `eval` or in the template's `image`
            // wrapper, never in a file of the model's; the locate pass places them. When the body
            // compiles clean on its own the fault is a front matter value the template passes
            // through (`paper: b4` → the span is `meta.paper`), or a real template bug.
            let located = self.locate(body, skipped).unwrap_or_default();
            if !located.is_empty() {
                return located;
            }
            errs.iter()
                .map(|e| match position(&world, e.span) {
                    Some((id, line, col, range)) => match meta_key(&world, id, range) {
                        Some(key) => Diag {
                            line: 1,
                            col: 1,
                            message: format!("front matter `{key}`: {}", e.message),
                            hints: hints(e),
                        },
                        None => Diag {
                            line,
                            col,
                            message: format!("template: {}", e.message),
                            hints: hints(e),
                        },
                    },
                    None => Diag {
                        line: 0,
                        col: 0,
                        message: e.message.to_string(),
                        hints: hints(e),
                    },
                })
                .collect()
        });
        typst::comemo::evict(0);
        Ok((result, messages(&warnings)))
    }

    /// The model's Typst as the main file.
    fn typst(&self, content: &str) -> anyhow::Result<Outcome> {
        let slot = self.files.claim(content.to_owned());
        let world = self.typst.world_builder(slot.id).build()?;
        let Warned { output, warnings } = typst::compile::<PagedDocument>(&world);
        let result = output.map_err(|errs| {
            errs.iter()
                .map(|e| match position(&world, e.span) {
                    Some((id, line, col, _)) if id == slot.id => Diag {
                        line,
                        col,
                        message: e.message.to_string(),
                        hints: hints(e),
                    },
                    Some((_, line, col, _)) => Diag {
                        line,
                        col,
                        message: format!("template: {}", e.message),
                        hints: hints(e),
                    },
                    None => Diag {
                        line: 0,
                        col: 0,
                        message: e.message.to_string(),
                        hints: hints(e),
                    },
                })
                .collect()
        });
        typst::comemo::evict(0);
        Ok((result, messages(&warnings)))
    }

    /// cmarker `eval`s the Typst it generates, so every error in the body lands on that one call.
    /// To place them: transpile the body (`transpile.typ`), compile the generated Typst as a real
    /// file, and map each error's generated line back to the body line that contains its text.
    /// Best effort — `line: 0` when nothing matches.
    fn locate(&self, body: &str, skipped: usize) -> Option<Vec<Diag>> {
        let generated = self.transpile(body)?;
        let slot = self.files.claim(generated);
        let world = self.typst.world_builder(slot.id).build().ok()?;
        let Warned { output, .. } = typst::compile::<PagedDocument>(&world);
        let errs = output.err()?;
        let src = world.source(slot.id).ok()?;
        let diags = errs
            .iter()
            .map(|e| {
                let (line, col) = position(&world, e.span)
                    .filter(|(id, ..)| *id == slot.id)
                    .and_then(|(_, line, _, range)| {
                        map_back(body, src.text(), line, &src.text()[range])
                    })
                    .map_or((0, 0), |(line, col)| (line + skipped as u32, col));
                Diag {
                    line,
                    col,
                    message: e.message.to_string(),
                    hints: hints(e),
                }
            })
            .collect();
        typst::comemo::evict(0);
        Some(diags)
    }

    /// The Typst cmarker generates for `body`, via `transpile.typ`'s `<src>` metadata.
    fn transpile(&self, body: &str) -> Option<String> {
        let mut inputs = Dict::new();
        inputs.insert("body".into(), body.into_value());
        let world = self
            .typst
            .world_builder(self.transpile)
            .with_inputs(inputs)
            .build()
            .ok()?;
        let doc = typst::compile::<PagedDocument>(&world).output.ok()?;
        typst::comemo::evict(0);
        let label = Label::construct("src".into()).ok()?;
        let found = doc.introspector().query(&Selector::Label(label));
        let meta = found.first()?.to_packed::<MetadataElem>()?;
        meta.value.clone().cast::<String>().ok()
    }
}

/// `(file, 1-based line, 1-based column, byte range)` of a diagnostic span.
fn position(world: &TypstWorld<'_>, span: DiagSpan) -> Option<(FileId, u32, u32, Range<usize>)> {
    let id = span.id()?;
    let range = world.range(span)?;
    let src = world.source(id).ok()?;
    let lines = src.lines();
    let line = lines.byte_to_line(range.start)? + 1;
    let col = lines.byte_to_column(range.start)? + 1;
    Some((
        id,
        u32::try_from(line).ok()?,
        u32::try_from(col).ok()?,
        range,
    ))
}

/// The front matter key a template span reads (`meta.paper` → `paper`), if it is one.
fn meta_key(world: &TypstWorld<'_>, id: FileId, range: Range<usize>) -> Option<String> {
    let src = world.source(id).ok()?;
    src.text()
        .get(range)?
        .strip_prefix("meta.")
        .map(str::to_owned)
}

/// Body `(line, col)` for line `gen_line` of the generated Typst: the first body line containing
/// the generated line's text (raw Typst is copied verbatim), else the span's own text (a path, an
/// identifier).
fn map_back(body: &str, generated: &str, gen_line: u32, span_text: &str) -> Option<(u32, u32)> {
    let generated_line = generated
        .lines()
        .nth(gen_line.checked_sub(1)? as usize)?
        .trim();
    let span_text = span_text.trim().trim_matches('"');
    for needle in [generated_line, span_text]
        .into_iter()
        .filter(|n| n.len() >= 2)
    {
        if let Some((i, line)) = body.lines().enumerate().find(|(_, l)| l.contains(needle)) {
            let at = line
                .find(span_text)
                .filter(|_| !span_text.is_empty())
                .or_else(|| line.find(needle))?;
            let col = line[..at].chars().count() + 1;
            return Some((u32::try_from(i + 1).ok()?, u32::try_from(col).ok()?));
        }
    }
    None
}

fn hints(d: &SourceDiagnostic) -> Vec<String> {
    d.hints.iter().map(|h| h.v.to_string()).collect()
}

fn messages(warnings: &[SourceDiagnostic]) -> Vec<String> {
    warnings.iter().map(|w| w.message.to_string()).collect()
}

/// Per-call sources plus files on disk. A slot is a reusable `FileId` holding one call's source
/// (ids are a leaked `u16` interner, so they are pooled, not minted per call). Disk reads map the
/// Typst project root to `/`, so `image("/Users/x/a.png")` is that file; a path that does not
/// exist there is retried against the process cwd, so `image("chart.png")` is `./chart.png`.
#[derive(Clone, Default)]
struct Files(Arc<FilesInner>);

#[derive(Default)]
struct FilesInner {
    live: RwLock<HashMap<FileId, TypstSource>>,
    free: Mutex<Vec<FileId>>,
    minted: AtomicUsize,
}

struct Slot {
    id: FileId,
    files: Files,
}

impl Files {
    fn claim(&self, text: String) -> Slot {
        let id = self
            .0
            .free
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop()
            .unwrap_or_else(|| {
                let n = self.0.minted.fetch_add(1, Ordering::Relaxed);
                format!("/wth-{n}.typ").as_str().into_file_id()
            });
        self.0
            .live
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, TypstSource::new(id, text));
        Slot {
            id,
            files: self.clone(),
        }
    }

    fn on_disk(id: FileId) -> FileResult<PathBuf> {
        let VirtualRoot::Project = id.root() else {
            return Err(FileError::Other(Some("packages are not available".into())));
        };
        let abs = id.vpath().realize(Path::new("/"))?;
        if abs.is_file() {
            return Ok(abs);
        }
        let cwd = std::env::current_dir().map_err(|e| FileError::from_io(e, Path::new(".")))?;
        let local = id.vpath().realize(&cwd)?;
        if local.is_file() {
            Ok(local)
        } else {
            Err(FileError::NotFound(abs))
        }
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.files
            .0
            .live
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.id);
        self.files
            .0
            .free
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(self.id);
    }
}

impl FileResolver for Files {
    fn resolve_binary(&self, id: FileId) -> FileResult<Cow<'_, Bytes>> {
        let path = Files::on_disk(id)?;
        let data = std::fs::read(&path).map_err(|e| FileError::from_io(e, &path))?;
        Ok(Cow::Owned(Bytes::new(data)))
    }

    fn resolve_source(&self, id: FileId) -> FileResult<Cow<'_, TypstSource>> {
        if let Some(src) = self
            .0
            .live
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&id)
        {
            return Ok(Cow::Owned(src.clone()));
        }
        let path = Files::on_disk(id)?;
        let text = std::fs::read_to_string(&path).map_err(|e| FileError::from_io(e, &path))?;
        Ok(Cow::Owned(TypstSource::new(id, text)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/pdf");

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!("{FIXTURES}/{name}"))
            .unwrap_or_else(|e| panic!("{name}: {e}"))
            .replace("__FIXTURES__", FIXTURES)
    }

    fn png_dims(png: &[u8]) -> (u32, u32) {
        let be = |i: usize| u32::from_be_bytes([png[i], png[i + 1], png[i + 2], png[i + 3]]);
        (be(16), be(20))
    }

    #[tokio::test]
    async fn fixtures_compile() {
        let mut names: Vec<_> = std::fs::read_dir(FIXTURES)
            .expect("fixtures dir")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".md") && n != "error.md")
            .collect();
        names.sort();
        assert!(names.len() >= 7, "fixture corpus: {names:?}");
        for name in names {
            let content = fixture(&name);
            let started = std::time::Instant::now();
            let out = compile(&content, Source::Markdown, &[]).await.expect(&name);
            let ms = started.elapsed().as_millis();
            let doc = match out {
                Ok(doc) => doc,
                Err(diags) => panic!("{name}:\n{}", format_diags(&diags)),
            };
            eprintln!(
                "{name}: {} pages, {} KB, {ms} ms, warnings {:?}",
                doc.pages,
                doc.pdf.len() / 1024,
                doc.warnings
            );
            assert!(doc.pages >= 1 && doc.pdf.starts_with(b"%PDF"), "{name}");
            let (w, h) = png_dims(&doc.preview_png);
            assert!(w.max(h) <= 1568 && w.min(h) > 0, "{name}: preview {w}x{h}");
            if name == "long.md" {
                assert!(doc.pages >= 30, "long.md: {} pages", doc.pages);
            }
            if name == "frontmatter.md" {
                assert!(doc.pages >= 2, "toc page + body");
            }
        }
    }

    #[tokio::test]
    async fn error_is_located_in_content() {
        let content = fixture("error.md");
        let want = content
            .lines()
            .position(|l| l.contains("<!--raw-typst"))
            .expect("error line")
            + 1;
        let diags = compile(&content, Source::Markdown, &[])
            .await
            .expect("compile")
            .expect_err("must fail");
        assert!(
            diags[0].message.contains("tabel"),
            "{}",
            format_diags(&diags)
        );
        assert_eq!(
            (diags[0].line as usize, diags[0].col),
            (want, 16),
            "{}",
            format_diags(&diags)
        );
    }

    #[tokio::test]
    async fn bad_front_matter_is_a_diag() {
        let diags = compile("---\ntitle: [\n---\n# Hi\n", Source::Markdown, &[])
            .await
            .expect("compile")
            .expect_err("must fail");
        assert_eq!((diags[0].line, diags[0].col), (1, 1));
        assert!(
            diags[0].message.starts_with("front matter:"),
            "{}",
            diags[0].message
        );
        let diags = compile("---\npaper: b4\n---\n# Hi\n", Source::Markdown, &[])
            .await
            .expect("compile")
            .expect_err("must fail");
        assert_eq!((diags[0].line, diags[0].col), (1, 1));
        assert!(
            diags[0].message.starts_with("front matter `paper`:"),
            "{}",
            diags[0].message
        );
    }

    #[tokio::test]
    async fn typst_source_compiles_and_locates() {
        let ok = compile(
            "#set page(columns: 2)\n= Two columns\n#lorem(400)",
            Source::Typst,
            &[],
        )
        .await
        .expect("compile");
        let doc = ok.expect("compiles");
        assert_eq!(doc.pages, 1);
        let diags = compile("= Title\n\n#tabel(columns: 2)[a][b]\n", Source::Typst, &[])
            .await
            .expect("compile")
            .expect_err("must fail");
        assert_eq!(
            (diags[0].line, diags[0].col),
            (3, 2),
            "{}",
            format_diags(&diags)
        );
    }

    #[test]
    fn format_diags_truncates_at_eight() {
        let diags: Vec<_> = (1..=10)
            .map(|i| Diag {
                line: i,
                col: 1,
                message: format!("e{i}"),
                hints: if i == 1 { vec!["fix".into()] } else { vec![] },
            })
            .collect();
        let text = format_diags(&diags);
        assert!(
            text.starts_with("1:1: e1\n  hint: fix\n2:1: e2\n"),
            "{text}"
        );
        assert!(
            text.contains("8:1: e8\n") && !text.contains("9:1"),
            "{text}"
        );
        assert!(text.ends_with("… and 2 more\n"), "{text}");
        assert_eq!(format_diags(&diags[..8]).matches('\n').count(), 9);
    }

    #[test]
    fn front_matter_offsets_lines() {
        let (yaml, body, skipped) = split_front_matter("---\ntitle: T\n---\n# H\n");
        assert_eq!((yaml, body, skipped), (Some("title: T\n"), "# H\n", 3));
        assert_eq!(split_front_matter("# H\n---\n"), (None, "# H\n---\n", 0));
        assert_eq!(
            split_front_matter("---\nnever closed\n"),
            (None, "---\nnever closed\n", 0)
        );
    }

    #[test]
    fn skill_has_front_matter() {
        assert!(skill().starts_with("---\nname: pdf\n"));
    }
}
