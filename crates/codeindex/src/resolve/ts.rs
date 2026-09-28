//! TS/TSX/JS: `oxc_resolver` (60 §6.1) — nearest `tsconfig.json` (`paths`,
//! `baseUrl`, `extends`, `references`), `package.json` `exports`/`main`,
//! relative files. `files.package` = nearest `package.json` `name`.

use std::path::{Path, PathBuf};

use oxc_resolver::{ResolveOptions, Resolver, TsconfigDiscovery};

use super::Resolved;

/// One resolver per root.
pub struct Ts {
    root: PathBuf,
    resolver: Resolver,
}

impl Ts {
    /// 60 §2.B options; `TsconfigDiscovery::Auto` finds the nearest
    /// `tsconfig.json` that owns the file (monorepos have one per package).
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            resolver: Resolver::new(ResolveOptions {
                tsconfig: Some(TsconfigDiscovery::Auto),
                condition_names: ["types", "import", "require", "node", "default"]
                    .map(String::from)
                    .to_vec(),
                extensions: [".ts", ".tsx", ".js", ".mjs", ".cjs", ".jsx", ".d.ts"]
                    .map(String::from)
                    .to_vec(),
                main_fields: ["types", "module", "main"].map(String::from).to_vec(),
                ..ResolveOptions::default()
            }),
        }
    }

    /// Resolve `specifier` as written in root-relative `from`.
    pub fn resolve(&self, from: &str, specifier: &str) -> Resolved {
        let abs = self.root.join(from);
        let relative = specifier.starts_with('.') || specifier.starts_with('/');
        match self.resolver.resolve_file(&abs, specifier) {
            Ok(r) => {
                let p = r.full_path();
                match p.strip_prefix(&self.root) {
                    Ok(rel) if !rel.components().any(|c| c.as_os_str() == "node_modules") => {
                        let provenance = if relative {
                            "relative"
                        } else if self.resolver.find_tsconfig(&abs).ok().flatten().is_some() {
                            "tsconfig"
                        } else {
                            "package"
                        };
                        Resolved::File(rel.to_string_lossy().replace('\\', "/"), provenance)
                    }
                    _ => Resolved::External,
                }
            }
            Err(_) if relative => Resolved::Unresolved,
            Err(_) => Resolved::External,
        }
    }

    /// `name` of the nearest `package.json` at or above `from`'s directory.
    pub fn package(&self, from: &str) -> Option<String> {
        let mut dir = self.root.join(from);
        while dir.pop() && dir.starts_with(&self.root) {
            if let Some(name) = std::fs::read(dir.join("package.json"))
                .ok()
                .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
                .and_then(|v| v.get("name")?.as_str().map(str::to_owned))
            {
                return Some(name);
            }
        }
        None
    }
}
