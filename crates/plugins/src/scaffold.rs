//! Owner: B2 — `t plugin new` / `plugin_new`: a plugin folder from the bundled SDK templates
//! (contract §10.2). Public for [`new_plugin`] (contract §3.4).
//!
//! The SDK is embedded by `build.rs` (`SDK_FILES`, paths relative to `sdk/`). A scaffold is the
//! template directory `templates/<lang>/` (or `templates/raw-<lang>/`) with the placeholder name
//! `presses` renamed, plus the language's SDK file(s) vendored where the template expects them.

use std::path::{Path, PathBuf};

use crate::error::PluginsError;
use crate::types::{Lang, ScaffoldReport};

mod bundled {
    include!(concat!(env!("OUT_DIR"), "/sdk_files.rs"));
}

/// Every bundled SDK file: (path relative to `sdk/`, bytes). Empty when the build had no `sdk/`.
pub(crate) fn sdk_files() -> &'static [(&'static str, &'static [u8])] {
    bundled::SDK_FILES
}

/// The template placeholder name (research §10: the "presses" hello world).
const PLACEHOLDER: &str = "presses";

/// The template directory name under `sdk/templates/`.
fn template_dir(lang: Lang, raw: bool) -> &'static str {
    match (lang, raw) {
        (Lang::Py, false) => "py",
        (Lang::Js, false) => "js",
        (Lang::Sh, false) => "sh",
        (Lang::Go, false) => "go",
        (Lang::Rb, false) => "rb",
        (Lang::Rs, false) => "rs",
        (Lang::Py, true) => "raw-py",
        (Lang::Sh, true) => "raw-sh",
        (_, true) => unreachable!("raw is refused for {lang:?} before this"),
    }
}

/// The vendored SDK files of `lang`: (path under `sdk/`, path in the plugin folder). The
/// templates decide the destination: `src/theuth.rs` for Rust (`mod theuth;`), the folder root
/// for the rest.
fn vendored(lang: Lang) -> &'static [(&'static str, &'static str)] {
    match lang {
        Lang::Py => &[("python/theuth_plugin.py", "theuth_plugin.py")],
        Lang::Js => &[
            ("ts/theuth-plugin.js", "theuth-plugin.js"),
            ("ts/theuth-plugin.d.ts", "theuth-plugin.d.ts"),
        ],
        Lang::Sh => &[("sh/theuth-plugin.sh", "theuth-plugin.sh")],
        Lang::Go => &[("go/theuth.go", "theuth.go")],
        Lang::Rb => &[("ruby/theuth_plugin.rb", "theuth_plugin.rb")],
        Lang::Rs => &[("rust/src/lib.rs", "src/theuth.rs")],
    }
}

/// `^[a-z][a-z0-9-]{0,31}$`: the syntax half of `manifest::check_name`, checked here as well so
/// no name can ever escape `plugins_dir` whatever the loader's rules become.
fn safe_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some('a'..='z'))
        && name.len() <= 32
        && chars.all(|c| matches!(c, 'a'..='z' | '0'..='9' | '-'))
}

/// Rename the placeholder in a template text: `presses_` → `<name with - as _>_` first, then
/// `presses` → `<name>`.
fn rename(text: &str, name: &str) -> String {
    let snake = name.replace('-', "_");
    text.replace(&format!("{PLACEHOLDER}_"), &format!("{snake}_"))
        .replace(PLACEHOLDER, name)
}

/// Whether `rel` is source code whose bare words are identifiers (a hyphen would not parse).
/// Shell, YAML, JSON, TOML and `go.mod` take the name as a plain word or string.
fn is_code(rel: &str) -> bool {
    [".py", ".rb", ".go", ".rs", ".mjs", ".js", ".ts"].iter().any(|ext| rel.ends_with(ext))
}

/// [`rename`] for source code: inside a string literal (`"…"`, `'…'`, `` `…` ``) the plugin
/// name stays as is (tool names, placement ids, which plugin.yaml spells the same); outside
/// one it becomes an identifier (`-` → `_`).
fn rename_code(text: &str, name: &str) -> String {
    let snake = name.replace('-', "_");
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let mut quote: Option<char> = None;
        let mut escaped = false;
        let mut rest = line;
        while let Some(c) = rest.chars().next() {
            // Outside a string `escaped` is always false.
            if !escaped && let Some(tail) = rest.strip_prefix(PLACEHOLDER) {
                let ident_tail = tail.starts_with('_');
                out.push_str(if quote.is_some() && !ident_tail { name } else { &snake });
                rest = tail;
                continue;
            }
            match (quote, c) {
                (Some(_), '\\') if !escaped => escaped = true,
                (Some(q), c) if c == q && !escaped => quote = None,
                (None, '"' | '\'' | '`') => quote = Some(c),
                _ => escaped = false,
            }
            if c != '\\' {
                escaped = false;
            }
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

/// Scripts the template runs directly are marked executable.
fn executable(rel: &str) -> bool {
    let file = rel.rsplit('/').next().unwrap_or(rel);
    file == "run.sh" || file.starts_with("main.")
}

fn io(context: &str, e: std::io::Error) -> PluginsError {
    PluginsError::Io {
        message: format!("{context}: {e}"),
    }
}

/// Write `<plugins_dir>/<name>/` from the bundled SDK template for `lang` (raw: the no-SDK
/// template; only `py` and `sh` have one). Fails with `Exists` if the folder exists.
pub fn new_plugin(
    plugins_dir: &std::path::Path,
    name: &str,
    lang: Lang,
    raw: bool,
) -> Result<ScaffoldReport, PluginsError> {
    if !safe_name(name) {
        return Err(PluginsError::Invalid {
            message: format!("bad plugin name {name:?}: lowercase letters, digits and '-', starting with a letter, at most 32"),
        });
    }
    crate::manifest::check_name(name)?;
    if raw && !matches!(lang, Lang::Py | Lang::Sh) {
        return Err(PluginsError::Invalid {
            message: "--raw has templates for py and sh only".into(),
        });
    }
    let files = plan(name, lang, raw)?;
    let dest = plugins_dir.join(name);
    if dest.symlink_metadata().is_ok() {
        return Err(PluginsError::Exists { name: name.into() });
    }
    std::fs::create_dir_all(plugins_dir).map_err(|e| io(&format!("create {}", plugins_dir.display()), e))?;
    // A dot-folder beside the destination: the watcher never sees a half-written plugin.
    let tmp = plugins_dir.join(format!(".{name}.new-{:016x}", rand::random::<u64>()));
    let written = write_all(&tmp, &files).and_then(|()| {
        if dest.symlink_metadata().is_ok() {
            return Err(PluginsError::Exists { name: name.into() });
        }
        std::fs::rename(&tmp, &dest).map_err(|e| io(&format!("move into {}", dest.display()), e))
    });
    if let Err(e) = written {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(e);
    }
    Ok(ScaffoldReport {
        name: name.into(),
        dir: dest.to_string_lossy().into_owned(),
        lang,
        raw,
        files: files.into_iter().map(|(rel, _, _)| rel).collect(),
        next: format!("awaiting approval: press Approve in theuth or run `t plugin approve {name}`"),
    })
}

/// (relative path in the plugin folder, bytes, executable), sorted by path.
type Planned = Vec<(String, Vec<u8>, bool)>;

/// What the scaffold writes, from the bundled table.
fn plan(name: &str, lang: Lang, raw: bool) -> Result<Planned, PluginsError> {
    let table = sdk_files();
    if table.is_empty() {
        return Err(PluginsError::Io {
            message: "the SDK templates are not bundled in this build".into(),
        });
    }
    let prefix = format!("templates/{}/", template_dir(lang, raw));
    let mut out: Planned = Vec::new();
    for (path, bytes) in table {
        let Some(rel) = path.strip_prefix(&prefix) else { continue };
        let rel = rename(rel, name);
        let bytes = match std::str::from_utf8(bytes) {
            Ok(text) if is_code(&rel) => rename_code(text, name).into_bytes(),
            Ok(text) => rename(text, name).into_bytes(),
            Err(_) => bytes.to_vec(),
        };
        let exec = executable(&rel);
        out.push((rel, bytes, exec));
    }
    if out.is_empty() {
        return Err(PluginsError::Io {
            message: format!("the SDK template {prefix} is not bundled in this build"),
        });
    }
    if !raw {
        for (src, dst) in vendored(lang) {
            let Some((_, bytes)) = table.iter().find(|(p, _)| p == src) else {
                return Err(PluginsError::Io {
                    message: format!("the SDK file sdk/{src} is not bundled in this build"),
                });
            };
            // Vendored verbatim: the SDK file stays byte-identical to its release.
            out.push(((*dst).to_owned(), bytes.to_vec(), false));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

fn write_all(root: &Path, files: &Planned) -> Result<(), PluginsError> {
    std::fs::create_dir(root).map_err(|e| io(&format!("create {}", root.display()), e))?;
    for (rel, bytes, exec) in files {
        let path: PathBuf = rel.split('/').fold(root.to_path_buf(), |p, c| p.join(c));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| io(&format!("create {}", parent.display()), e))?;
        }
        std::fs::write(&path, bytes).map_err(|e| io(&format!("write {}", path.display()), e))?;
        if *exec {
            set_executable(&path)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), PluginsError> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| io(&format!("chmod {}", path.display()), e))
}

#[cfg(not(unix))]
fn set_executable(_: &Path) -> Result<(), PluginsError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Lang; 6] = [Lang::Py, Lang::Js, Lang::Sh, Lang::Go, Lang::Rb, Lang::Rs];

    #[test]
    fn every_lang_writes_manifest_and_sdk() {
        if sdk_files().is_empty() {
            eprintln!("skipped: SDK_FILES is empty (no sdk/ in this build)");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        for lang in ALL {
            let name = format!("demo-{}", serde_json::to_value(lang).unwrap().as_str().unwrap());
            let r = new_plugin(tmp.path(), &name, lang, false).unwrap();
            let dir = tmp.path().join(&name);
            assert_eq!(r.dir, dir.to_string_lossy());
            let manifest = std::fs::read_to_string(dir.join("plugin.yaml")).unwrap();
            assert!(manifest.contains(&format!("name: {name}\n")), "{lang:?}: {manifest}");
            assert!(!manifest.contains(PLACEHOLDER), "{lang:?}: placeholder left in plugin.yaml");
            for (src, dst) in vendored(lang) {
                let want = sdk_files().iter().find(|(p, _)| p == src).unwrap().1;
                assert_eq!(std::fs::read(dir.join(dst)).unwrap(), want, "{lang:?}: {dst} vendored verbatim");
                assert!(r.files.iter().any(|f| f == dst), "{lang:?}: {dst} reported");
            }
            assert!(r.files.iter().any(|f| f == "plugin.yaml"));
            assert_eq!(r.next, format!("awaiting approval: press Approve in theuth or run `t plugin approve {name}`"));
        }
        // No temp folder is left behind.
        let leftovers: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with('.'))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn raw_only_for_py_and_sh() {
        let tmp = tempfile::tempdir().unwrap();
        for lang in [Lang::Js, Lang::Go, Lang::Rb, Lang::Rs] {
            let e = new_plugin(tmp.path(), "raw-demo", lang, true).unwrap_err();
            assert_eq!(e, PluginsError::Invalid { message: "--raw has templates for py and sh only".into() });
        }
        assert!(!tmp.path().join("raw-demo").exists());
        if sdk_files().is_empty() {
            return;
        }
        for lang in [Lang::Py, Lang::Sh] {
            let name = format!("raw-{}", serde_json::to_value(lang).unwrap().as_str().unwrap());
            let r = new_plugin(tmp.path(), &name, lang, true).unwrap();
            assert!(r.raw);
            assert!(!r.files.iter().any(|f| f.starts_with("theuth")), "raw vendors no SDK: {:?}", r.files);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(tmp.path().join("raw-sh/run.sh")).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o755);
        }
    }

    #[test]
    fn placeholder_renamed() {
        assert_eq!(rename("presses_count = presses", "my-tool"), "my_tool_count = my-tool");
        assert_eq!(rename("presses/presses_x", "a"), "a/a_x");
        assert_eq!(
            rename_code("@plugin.tool(\"presses\")\ndef presses(_): return 'presses_x \\'presses'\n", "my-tool"),
            "@plugin.tool(\"my-tool\")\ndef my_tool(_): return 'my_tool_x \\'my-tool'\n"
        );
        assert!(is_code("main.py") && is_code("src/main.rs") && !is_code("run.sh") && !is_code("plugin.yaml"));
        if sdk_files().is_empty() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        new_plugin(tmp.path(), "my-tool", Lang::Py, false).unwrap();
        let manifest = std::fs::read_to_string(tmp.path().join("my-tool/plugin.yaml")).unwrap();
        assert!(manifest.contains("id: my-tool"), "{manifest}");
    }

    #[test]
    fn existing_folder_refused() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("taken")).unwrap();
        std::fs::write(tmp.path().join("taken/keep"), b"x").unwrap();
        let e = new_plugin(tmp.path(), "taken", Lang::Py, false).unwrap_err();
        assert_eq!(e, PluginsError::Exists { name: "taken".into() });
        assert_eq!(std::fs::read(tmp.path().join("taken/keep")).unwrap(), b"x");
    }

    #[test]
    fn name_rules_enforced() {
        let tmp = tempfile::tempdir().unwrap();
        for bad in ["", "Caps", "1abc", "../x", "a/b", "a_b", "x".repeat(33).as_str(), "plugins"] {
            let e = new_plugin(tmp.path(), bad, Lang::Py, false).unwrap_err();
            assert!(matches!(e, PluginsError::Invalid { .. }), "{bad:?}: {e:?}");
        }
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0, "nothing written");
    }
}
