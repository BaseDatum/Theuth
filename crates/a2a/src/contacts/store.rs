//! 183 §7.8 (W-24): `<data_dir>/a2a/peers.yaml` as the contact RPCs write it. A rewrite is
//! atomic (sibling temp file, then rename) and keeps the owner's leading comment block; the
//! entries themselves are re-serialized, so a comment inside an entry does not survive a
//! UI write (serde has no comment model).

use std::path::Path;
use std::time::SystemTime;

use crate::config::PeersFile;

/// The leading comment block: every line before the first one that is neither blank nor a
/// `#` comment.
pub fn header(text: &str) -> String {
    let mut out = String::new();
    for line in text.lines() {
        let t = line.trim_start();
        if !t.is_empty() && !t.starts_with('#') {
            break;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Parse a file's text: its header and its entries. Blank or comment-only text is empty.
pub fn parse(text: &str) -> Result<(String, PeersFile), String> {
    let head = header(text);
    if head.len() >= text.len() {
        return Ok((head, PeersFile::default()));
    }
    let file = serde_yaml::from_str(text).map_err(|e| e.to_string())?;
    Ok((head, file))
}

/// 183: what a rewrite adds to the leading comment block (once), so a reader of the file —
/// an owner or a model — does not take a contact's `exports` for what it shares with us.
pub const EXPORTS_NOTE: &str = "# peers.<contact>.exports: what THIS machine grants that \
contact (it may reach them here), not what the contact shares with us.\n";

/// The header (with [`EXPORTS_NOTE`] added once, after the owner's lines), then the entries.
pub fn render(head: &str, file: &PeersFile) -> Result<String, String> {
    let body = serde_yaml::to_string(file).map_err(|e| e.to_string())?;
    let note = if head.contains(EXPORTS_NOTE) {
        ""
    } else {
        EXPORTS_NOTE
    };
    Ok(format!("{head}{note}{body}"))
}

/// Replace `path` with `text` atomically (owner-only on unix: the file names vault secrets).
pub fn write(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("yaml.{}.tmp", std::process::id()));
    {
        use std::io::Write as _;
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
        let mut f = o.open(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// The file's permission bits when they let anyone but the owner in (unix; the file names
/// vault secrets); `None` when it is 0600, missing, or the platform has no modes.
pub fn loose_mode(path: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path).ok()?.permissions().mode() & 0o777;
        (mode != 0o600).then_some(mode)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// What the hot reload compares: modification time and length; `None` without a file.
pub fn stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The owner's notes above the entries survive a UI rewrite, the entries round-trip,
    /// and the write leaves no temp file behind.
    #[test]
    fn rewrite_keeps_the_header_and_the_entries() {
        let text = "# Contacts and exports.\n# Owner notes: keep.\n\npeers:\n  lab:\n    inbound_secret: in # vault name\n    exports: [research]\nexports:\n  research:\n    session: \"00000000-0000-0000-0000-000000000002\"\n    name: Research\n";
        let (head, mut file) = parse(text).unwrap();
        assert_eq!(head, "# Contacts and exports.\n# Owner notes: keep.\n\n");
        file.peers.get_mut("lab").unwrap().exports.clear();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a2a/peers.yaml");
        write(&path, &render(&head, &file).unwrap()).unwrap();
        let back = std::fs::read_to_string(&path).unwrap();
        assert!(back.starts_with("# Contacts and exports.\n# Owner notes: keep.\n\n"));
        let (head2, file2) = parse(&back).unwrap();
        assert_eq!(
            head2,
            format!("{head}{EXPORTS_NOTE}"),
            "the note follows the owner's"
        );
        // A second rewrite keeps one note.
        let again = render(&head2, &file2).unwrap();
        assert_eq!(again.matches(EXPORTS_NOTE).count(), 1);
        assert!(file2.peers["lab"].exports.is_empty());
        assert_eq!(file2.peers["lab"].inbound_secret.as_deref(), Some("in"));
        assert_eq!(file2.exports["research"].name, "Research");
        assert_eq!(
            std::fs::read_dir(dir.path().join("a2a")).unwrap().count(),
            1,
            "no temp file left"
        );
        assert!(stamp(&path).is_some());
        // A comment-only file is an empty one, and keeps its comments.
        let (h, f) = parse("# nothing yet\n").unwrap();
        assert_eq!(h, "# nothing yet\n");
        assert!(f.peers.is_empty() && f.exports.is_empty());
    }
}
