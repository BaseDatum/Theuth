//! Go (60 §6.1): `go.mod` `module` line; an import `M/x/y` → every `.go`
//! file in `x/y` (one row per file; Go imports are package-level). Others →
//! external. `files.package` = the file's directory.

use std::collections::HashMap;
use std::path::Path;

/// The module path from `go.mod`, when the root has one.
pub struct Go {
    module: String,
}

impl Go {
    /// `None` without a `go.mod` `module` line.
    pub fn new(root: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(root.join("go.mod")).ok()?;
        let module = text
            .lines()
            .find_map(|l| l.trim().strip_prefix("module "))?
            .trim()
            .trim_matches('"')
            .to_owned();
        Some(Self { module })
    }

    /// Target `.go` files of `specifier` from `by_dir` (directory → files):
    /// `None` = external; `Some(&[])` = inside the module, no such package.
    pub fn resolve<'a>(
        &self,
        by_dir: &'a HashMap<String, Vec<String>>,
        specifier: &str,
    ) -> Option<&'a [String]> {
        let rel = if specifier == self.module {
            "."
        } else {
            specifier.strip_prefix(&self.module)?.strip_prefix('/')?
        };
        Some(by_dir.get(rel).map_or(&[][..], Vec::as_slice))
    }
}

/// `files.package` for Go: the directory (`.` at the root).
pub fn package(from: &str) -> String {
    from.rsplit_once('/').map_or(".", |(d, _)| d).to_owned()
}
