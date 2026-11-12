//! Artifact/app source rendering and harness-local file operations.
/// Where an app runs; the TUI only reads its source (46 §8.2).
pub const APP_WHERE: &str = "interactive on desktop/web";
/// Rows an inline artifact body shows at most (35 §6.1).
const INLINE_LINES: usize = 200;
/// Rows of a CSV artifact shown inline.
const CSV_ROWS: usize = 20;

/// Bytes as `512 B` / `3.2 KB` / `1.4 MB`.
pub fn bytes_label(bytes: u64) -> String {
    match bytes {
        b if b < 1024 => format!("{b} B"),
        b if b < 1024 * 1024 => format!("{:.1} KB", b as f64 / 1024.0),
        b => format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)),
    }
}


/// The kind-specific size of an artifact card header (35 §6.1): pages for a
/// compiled PDF, bytes otherwise. Drawn every frame, so nothing here touches
/// the file; the line count shows in the expanded body.
pub fn artifact_size(a: &artifact::Artifact) -> String {
    match (a.kind, a.pages) {
        (artifact::Kind::Pdf, Some(p)) => format!("{p} pages · {}", bytes_label(a.bytes)),
        _ => bytes_label(a.bytes),
    }
}

/// One `/artifacts` row: `▣ name · kind · size · vN · 2m ago`.
pub fn artifact_row(a: &artifact::Artifact) -> String {
    let age = chrono::DateTime::from_timestamp(a.created_at, 0)
        .map(|t| chrono::Utc::now().signed_duration_since(t))
        .map_or_else(String::new, |d| {
            let secs = d.num_seconds().max(0);
            match secs {
                s if s < 60 => format!("{s}s ago"),
                s if s < 3600 => format!("{}m ago", s / 60),
                s if s < 86400 => format!("{}h ago", s / 3600),
                s => format!("{}d ago", s / 86400),
            }
        });
    format!(
        "▣ {} · {} · {} · v{} · {age}",
        a.name,
        a.kind.as_str(),
        bytes_label(a.bytes),
        a.version
    )
}

/// The first [`INLINE_LINES`] lines of `s` and how many were cut.
fn cap_lines(s: &str) -> (String, usize) {
    let total = s.lines().count();
    if total <= INLINE_LINES {
        return (s.trim_end().to_owned(), 0);
    }
    let kept: Vec<&str> = s.lines().take(INLINE_LINES).collect();
    (kept.join("\n"), total - INLINE_LINES)
}

/// `body` in a code fence tagged `lang`; the fence grows past any backtick run inside.
fn fenced(lang: &str, body: &str) -> String {
    let longest = body
        .lines()
        .map(|l| l.chars().take_while(|c| *c == '`').count())
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    format!("{fence}{lang}\n{body}\n{fence}")
}

/// A CSV cell as a markdown table cell: pipes escaped, newlines flattened.
fn md_cell(s: &str) -> String {
    s.replace('|', "\\|").replace(['\n', '\r'], " ")
}


/// The file an expanded card reads (35 §6.1): the blob for text-like kinds,
/// the listing sidecar for an archive, nothing for pictures and downloads.
pub(crate) fn artifact_read_path(a: &artifact::Artifact) -> Option<std::path::PathBuf> {
    use artifact::Kind;
    match a.kind {
        Kind::Markdown | Kind::Text | Kind::Html | Kind::Csv | Kind::App | Kind::Mermaid => {
            Some(a.path.clone())
        }
        Kind::Archive => Some(artifact::archive::sidecar(&a.path)),
        Kind::Image | Kind::Svg | Kind::Pdf | Kind::File => None,
    }
}

/// Rows of an expanded archive card.
const ARCHIVE_ROWS: usize = 50;

/// Render bytes read by the owner effect (also used by search previews).
/// `Err` is why the file could not be read.
pub(crate) fn artifact_text(
    a: &artifact::Artifact,
    bytes: Result<&[u8], &str>,
    opts: &markdown::Options,
) -> markdown::Text {
    use artifact::Kind;
    let ext = a.path.extension().map_or("", |e| e.to_str().unwrap_or(""));
    let path = a.path.display().to_string();
    let unreadable = |e: &dyn std::fmt::Display| format!("cannot read {path}: {e}");
    let md = match a.kind {
        Kind::Markdown | Kind::Text | Kind::Html | Kind::App => match bytes {
            Err(e) => unreadable(&e),
            Ok(b) => {
                let s = String::from_utf8_lossy(b);
                let (body, cut) = cap_lines(&s);
                let text = match a.kind {
                    Kind::Markdown => body,
                    Kind::App => format!("[app] {APP_WHERE} · source:\n\n{}", fenced(ext, &body)),
                    _ => fenced(ext, &body),
                };
                match cut {
                    0 => text,
                    n => format!("{text}\n\n… (+{n} lines, o opens)"),
                }
            }
        },
        Kind::Csv => match bytes
            .map_err(str::to_owned)
            .and_then(|b| artifact::kind::csv_table(b, ext == "tsv", CSV_ROWS))
        {
            Err(e) => unreadable(&e),
            Ok((header, rows, total)) => {
                let cols = header.len().max(1);
                let row = |cells: &[String]| {
                    let mut line = String::from("|");
                    for i in 0..cols {
                        line.push(' ');
                        line.push_str(&cells.get(i).map_or_else(String::new, |c| md_cell(c)));
                        line.push_str(" |");
                    }
                    line
                };
                let mut out = row(&header);
                out.push('\n');
                out.push_str(&format!("|{}", " --- |".repeat(cols)));
                for r in &rows {
                    out.push('\n');
                    out.push_str(&row(r));
                }
                if total > rows.len() {
                    out.push_str(&format!("\n\n… (+{} rows)", total - rows.len()));
                }
                out
            }
        },
        // A `mermaid` fence: the assistant's fence hook draws it through
        // `surface::mermaid_lines`, the plain code block when that declines.
        Kind::Mermaid => match bytes {
            Err(e) => unreadable(&e),
            Ok(b) => fenced("mermaid", String::from_utf8_lossy(b).trim_end()),
        },
        Kind::Archive => match bytes.map_err(str::to_owned).and_then(|b| {
            serde_json::from_slice::<artifact::archive::ArchiveListing>(b)
                .map_err(|e| e.to_string())
        }) {
            Err(_) => format!(
                "{} · no listing (archive unreadable) · {path}",
                bytes_label(a.bytes)
            ),
            Ok(l) => {
                let mut out = format!(
                    "{} files · {} unpacked · {} · {path}\n",
                    l.total_entries,
                    bytes_label(l.total_size),
                    bytes_label(a.bytes)
                );
                for e in l.entries.iter().take(ARCHIVE_ROWS) {
                    out.push_str(&format!(
                        "\n    {:>9}  {}",
                        if e.is_dir {
                            String::new()
                        } else {
                            bytes_label(e.size)
                        },
                        e.path
                    ));
                }
                if l.total_entries > ARCHIVE_ROWS as u64 {
                    out.push_str(&format!(
                        "\n\n… (+{} entries, o opens)",
                        l.total_entries - ARCHIVE_ROWS as u64
                    ));
                }
                out
            }
        },
        Kind::Image => format!("[image] {path}"),
        Kind::Svg => format!("[svg] {path}"),
        Kind::Pdf => format!("{} · {path}", artifact_size(a)),
        Kind::File => path.clone(),
    };
    let mut t = markdown::Text::with(opts.clone());
    t.push(&md);
    t.finish();
    t.reveal_all();
    t
}


/// Detached harness-local open operation, shared by artifact and search effects.
pub(crate) fn open_path(path: &std::path::Path) -> Result<(), String> {
    platform::open::open_path(path).map_err(|error| format!("could not open {}: {error}", path.display()))
}
/// Existing export destination and display line, shared by artifact and search effects.
pub(crate) async fn export_artifact(api: &rpc::client::Engine, id: String, name: String, session: Option<runtime::SessionId>) -> Result<String, String> {
    use rpc_compose::api::clients::ArtifactArtifactsApiClient;
    let dest = std::env::home_dir().ok_or_else(|| "no home directory".to_owned())?.join("Downloads").join(name);
    let path = api.export_artifact(id, dest, session).await.map_err(crate::reason)?;
    Ok(format!("exported → {}", path.display()))
}
