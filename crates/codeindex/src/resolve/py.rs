//! Python (60 §6.1): roots = repo root, `src/` when present, the package
//! directories `pyproject.toml` names; relative imports strip dots against
//! the file's directory; absolute ones try `<root>/a/b.py` then
//! `<root>/a/b/__init__.py` per root. `files.package` = the top-most
//! directory with an `__init__.py` on the file's path.

use std::collections::HashSet;
use std::path::Path;

use super::Resolved;

/// Root-relative import roots, in order (`""` = the repo root).
pub struct Py {
    roots: Vec<String>,
}

impl Py {
    /// Read `pyproject.toml` for `[tool.setuptools.packages.find] where` and
    /// `[tool.poetry] packages[].from`; missing or unreadable → root (+ `src`).
    pub fn new(root: &Path) -> Self {
        let mut roots = vec![String::new()];
        if root.join("src").is_dir() {
            roots.push("src".into());
        }
        if let Some(doc) = std::fs::read_to_string(root.join("pyproject.toml"))
            .ok()
            .and_then(|s| s.parse::<toml::Table>().ok())
        {
            let tool = doc.get("tool");
            let dirs = tool
                .and_then(|t| t.get("setuptools")?.get("packages")?.get("find")?.get("where")?.as_array())
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str())
                .chain(
                    tool.and_then(|t| t.get("poetry")?.get("packages")?.as_array())
                        .into_iter()
                        .flatten()
                        .filter_map(|p| p.get("from")?.as_str()),
                );
            for d in dirs {
                let d = d.trim_matches('/').trim_start_matches("./").to_owned();
                if !roots.contains(&d) {
                    roots.push(d);
                }
            }
        }
        Self { roots }
    }

    /// `from` is root-relative; `specifier` is `a.b`, `..a`, `.`.
    pub fn resolve(&self, files: &HashSet<String>, from: &str, specifier: &str) -> Resolved {
        let dots = specifier.bytes().take_while(|&b| b == b'.').count();
        let dotted: Vec<&str> = specifier[dots..].split('.').filter(|s| !s.is_empty()).collect();
        if dots > 0 {
            let mut base = from.rsplit_once('/').map_or("", |(d, _)| d).to_owned();
            for _ in 1..dots {
                if base.is_empty() {
                    return Resolved::Unresolved;
                }
                base = base.rsplit_once('/').map_or("", |(d, _)| d).to_owned();
            }
            return module_file(files, &base, &dotted).map_or(Resolved::Unresolved, |f| Resolved::File(f, "relative"));
        }
        for r in &self.roots {
            if let Some(f) = module_file(files, r, &dotted) {
                return Resolved::File(f, "package");
            }
        }
        Resolved::External
    }

    /// `from pkg import m` where `m` is a module of package `pkg/__init__.py`:
    /// `pkg/m.py` | `pkg/m/__init__.py`, when either exists.
    pub fn submodule(files: &HashSet<String>, package_file: &str, name: &str) -> Option<String> {
        let dir = package_file.strip_suffix("__init__.py")?;
        module_file(files, dir.trim_end_matches('/'), &[name]).filter(|f| f != package_file)
    }

    /// Top-level package directory name: the highest ancestor directory of
    /// `from` that has an `__init__.py` among `files`.
    pub fn package(files: &HashSet<String>, from: &str) -> Option<String> {
        let mut best = None;
        let mut dir = String::new();
        for seg in from.split('/').take(from.matches('/').count()) {
            dir = if dir.is_empty() { seg.to_owned() } else { format!("{dir}/{seg}") };
            if files.contains(&format!("{dir}/__init__.py")) {
                best.get_or_insert_with(|| seg.to_owned());
            } else {
                best = None;
            }
        }
        best
    }
}

/// `base/a/b.py` | `base/a/b/__init__.py`, longest module prefix first
/// (`from a.b import c` where `c` is itself a module lands on `a/b/c.py`).
fn module_file(files: &HashSet<String>, base: &str, dotted: &[&str]) -> Option<String> {
    for n in (0..=dotted.len()).rev() {
        let mut p = base.to_owned();
        for s in &dotted[..n] {
            if !p.is_empty() {
                p.push('/');
            }
            p.push_str(s);
        }
        let file = if n == 0 { format!("{p}/__init__.py").trim_start_matches('/').to_owned() } else { format!("{p}.py") };
        if files.contains(&file) {
            return Some(file);
        }
        let init = format!("{p}/__init__.py");
        if n > 0 && files.contains(&init) {
            return Some(init);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_and_absolute() {
        let files: HashSet<String> = ["pkg/__init__.py", "pkg/store.py", "pkg/sub/__init__.py", "pkg/sub/app.py", "src/lib/x.py"]
            .map(String::from)
            .into();
        let py = Py { roots: vec![String::new(), "src".into()] };
        assert_eq!(py.resolve(&files, "pkg/sub/app.py", "..store"), Resolved::File("pkg/store.py".into(), "relative"));
        assert_eq!(py.resolve(&files, "pkg/sub/app.py", "."), Resolved::File("pkg/sub/__init__.py".into(), "relative"));
        assert_eq!(py.resolve(&files, "pkg/sub/app.py", "pkg.store"), Resolved::File("pkg/store.py".into(), "package"));
        assert_eq!(py.resolve(&files, "pkg/sub/app.py", "pkg"), Resolved::File("pkg/__init__.py".into(), "package"));
        assert_eq!(py.resolve(&files, "pkg/sub/app.py", "lib.x"), Resolved::File("src/lib/x.py".into(), "package"));
        assert_eq!(py.resolve(&files, "pkg/sub/app.py", "os.path"), Resolved::External);
        assert_eq!(py.resolve(&files, "pkg/sub/app.py", "...nope"), Resolved::Unresolved);
        assert_eq!(Py::package(&files, "pkg/sub/app.py").as_deref(), Some("pkg"));
        assert_eq!(Py::package(&files, "src/lib/x.py"), None);
    }
}
