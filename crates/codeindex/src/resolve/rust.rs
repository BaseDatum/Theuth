//! Rust: Cargo workspace model + `mod` tree (60 §6.1). `cargo_toml` reads the
//! manifests (members are raw globs → `globset`); the module tree is walked
//! from every product root file following the `mod x;` rows the parse stored,
//! so each `.rs` file gets `(root file, module path)` and a `use` path can be
//! walked segment by segment.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;

use cargo_toml::{Dependency, Manifest};
use globset::{Glob, GlobSetBuilder};

/// One workspace member (or the single package).
#[derive(Debug)]
pub struct Package {
    /// `package.name`.
    pub name: String,
    /// Manifest directory, root-relative (`""` for the root).
    pub dir: String,
    /// `[lib] name` or the package name with `-` → `_`.
    pub lib_name: String,
    /// Root-relative lib root file, when the package has a lib.
    pub lib_root: Option<String>,
    /// Every product root file (lib, bins, tests, benches, examples, build.rs).
    pub roots: Vec<String>,
    /// In-scope dependency name (`-` → `_`) → package name.
    pub deps: HashMap<String, String>,
}

/// The workspace: `None` when the root has no `Cargo.toml`.
#[derive(Debug, Default)]
pub struct CargoWorkspace {
    /// Members, root first when the root is a package.
    pub packages: Vec<Package>,
    by_name: HashMap<String, usize>,
}

impl CargoWorkspace {
    /// Load `root/Cargo.toml`; a workspace expands `members` minus `exclude`
    /// (globs) and loads each member with inheritance from the root.
    pub fn load(root: &Path) -> Option<Self> {
        let ws_path = root.join("Cargo.toml");
        let ws = Manifest::from_path(&ws_path)
            .inspect_err(|e| tracing::debug!(error = %e, "root Cargo.toml"))
            .ok()?;
        let mut me = Self::default();
        let mut dirs: Vec<String> = Vec::new();
        if ws.package.is_some() {
            dirs.push(String::new());
        }
        if let Some(w) = &ws.workspace {
            let set = |pats: &[String]| {
                let mut b = GlobSetBuilder::new();
                for p in pats {
                    if let Ok(g) = Glob::new(p.trim_end_matches('/')) {
                        b.add(g);
                    }
                }
                b.build().ok()
            };
            let (Some(members), Some(exclude)) = (set(&w.members), set(&w.exclude)) else {
                return Some(me);
            };
            // Members are globs over directories: walk the depth the longest
            // pattern needs (a pattern without `*` is one directory).
            let depth = w
                .members
                .iter()
                .map(|m| m.split('/').count())
                .max()
                .unwrap_or(0);
            let mut found = Vec::new();
            member_dirs(root, root, depth, &mut found);
            for d in found {
                if members.is_match(&d)
                    && !exclude.is_match(&d)
                    && root.join(&d).join("Cargo.toml").is_file()
                {
                    dirs.push(d);
                }
            }
        }
        for dir in dirs {
            let path = if dir.is_empty() {
                ws_path.clone()
            } else {
                root.join(&dir).join("Cargo.toml")
            };
            let m = if dir.is_empty() {
                ws.clone()
            } else {
                match Manifest::from_path(&path) {
                    Ok(m) => m,
                    Err(e) => {
                        tracing::debug!(error = %e, path = %path.display(), "member Cargo.toml");
                        continue;
                    }
                }
            };
            let Some(pkg) = &m.package else { continue };
            let rel = |p: &str| -> String {
                let p = p.trim_start_matches("./");
                if dir.is_empty() {
                    p.to_owned()
                } else {
                    format!("{dir}/{p}")
                }
            };
            let lib_root = m
                .lib
                .as_ref()
                .and_then(|l| l.path.as_deref())
                .map(rel)
                .or_else(|| {
                    root.join(&dir)
                        .join("src/lib.rs")
                        .is_file()
                        .then(|| rel("src/lib.rs"))
                });
            let mut roots: Vec<String> = lib_root.iter().cloned().collect();
            for p in m
                .bin
                .iter()
                .chain(&m.test)
                .chain(&m.bench)
                .chain(&m.example)
            {
                if let Some(path) = &p.path {
                    roots.push(rel(path));
                }
            }
            if root.join(&dir).join("build.rs").is_file() {
                roots.push(rel("build.rs"));
            }
            let mut deps = HashMap::new();
            for (k, d) in m
                .dependencies
                .iter()
                .chain(&m.dev_dependencies)
                .chain(&m.build_dependencies)
                .chain(m.target.values().flat_map(|t| {
                    t.dependencies
                        .iter()
                        .chain(&t.dev_dependencies)
                        .chain(&t.build_dependencies)
                }))
            {
                let target = match d {
                    Dependency::Detailed(det) => det.package.clone().unwrap_or_else(|| k.clone()),
                    Dependency::Inherited(_) => ws
                        .workspace
                        .as_ref()
                        .and_then(|w| w.dependencies.get(k))
                        .and_then(|d| d.package().map(str::to_owned))
                        .unwrap_or_else(|| k.clone()),
                    Dependency::Simple(_) => k.clone(),
                };
                deps.insert(k.replace('-', "_"), target);
            }
            let lib_name = m
                .lib
                .as_ref()
                .and_then(|l| l.name.clone())
                .unwrap_or_else(|| pkg.name.replace('-', "_"));
            me.by_name.insert(pkg.name.clone(), me.packages.len());
            me.packages.push(Package {
                name: pkg.name.clone(),
                dir,
                lib_name,
                lib_root,
                roots,
                deps,
            });
        }
        Some(me)
    }

    /// The package whose directory is the longest prefix of `rel`.
    pub fn package_of(&self, rel: &str) -> Option<&Package> {
        self.packages
            .iter()
            .filter(|p| {
                p.dir.is_empty()
                    || rel
                        .strip_prefix(p.dir.as_str())
                        .is_some_and(|r| r.starts_with('/'))
            })
            .max_by_key(|p| p.dir.len())
    }

    fn by_name(&self, name: &str) -> Option<&Package> {
        self.by_name.get(name).map(|&i| &self.packages[i])
    }
}

/// Directories under `dir` up to `depth` levels, root-relative with `/`.
fn member_dirs(root: &Path, dir: &Path, depth: usize, out: &mut Vec<String>) {
    if depth == 0 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if !p.is_dir()
            || p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with('.'))
        {
            continue;
        }
        if let Ok(rel) = p.strip_prefix(root) {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
        member_dirs(root, &p, depth - 1, out);
    }
}

/// Where a path lands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// A file of the root.
    File(String),
    /// `std`/`core`/`alloc` or a dependency that is not a workspace member.
    External,
    /// The head is neither a keyword, a module, the lib nor a dependency: a
    /// type or an unknown crate. Imports treat it as external; a reference
    /// qualifier (`Registry::new()`) falls through to the §6.2 rules.
    Unknown,
    /// Looked local, no file matched.
    Unresolved,
}

/// The module tree: every reachable `.rs` file → `(root file, module path)`.
#[derive(Debug, Default)]
pub struct Modules {
    path_of: HashMap<String, (String, Vec<String>)>,
    file_of: HashMap<(String, Vec<String>), String>,
    /// `(parent file, mod name)` → child file.
    child: HashMap<(String, String), String>,
}

impl Modules {
    /// Walk from every product root through `mods` = `(file, name, #[path])`
    /// rows; only files in `rust_files` count. A file reached twice keeps its
    /// first assignment.
    pub fn build(
        ws: &CargoWorkspace,
        rust_files: &HashSet<String>,
        mods: &[(String, String, Option<String>)],
    ) -> Self {
        let mut decls: HashMap<&str, Vec<(&str, Option<&str>)>> = HashMap::new();
        for (f, n, p) in mods {
            decls
                .entry(f.as_str())
                .or_default()
                .push((n.as_str(), p.as_deref()));
        }
        let mut me = Self::default();
        let roots: HashSet<&String> = ws.packages.iter().flat_map(|p| &p.roots).collect();
        let mut queue = VecDeque::new();
        for r in ws.packages.iter().flat_map(|p| &p.roots) {
            if rust_files.contains(r) && !me.path_of.contains_key(r) {
                me.path_of.insert(r.clone(), (r.clone(), Vec::new()));
                me.file_of.insert((r.clone(), Vec::new()), r.clone());
                queue.push_back(r.clone());
            }
        }
        while let Some(f) = queue.pop_front() {
            let (root, m) = me.path_of[&f].clone();
            let Some(ds) = decls.get(f.as_str()) else {
                continue;
            };
            let dir = f.rsplit_once('/').map_or("", |(d, _)| d);
            let stem = f.rsplit('/').next().unwrap_or(&f).trim_end_matches(".rs");
            let base = if roots.contains(&f) || stem == "mod" {
                dir.to_owned()
            } else if dir.is_empty() {
                stem.to_owned()
            } else {
                format!("{dir}/{stem}")
            };
            for (name, attr) in ds {
                let candidates: Vec<String> = match attr {
                    Some(p) => vec![normalize(&format!("{dir}/{p}"))],
                    None => vec![
                        join(&base, &format!("{name}.rs")),
                        join(&base, &format!("{name}/mod.rs")),
                    ],
                };
                let Some(child) = candidates.into_iter().find(|c| rust_files.contains(c)) else {
                    continue;
                };
                me.child
                    .insert((f.clone(), (*name).to_owned()), child.clone());
                if me.path_of.contains_key(&child) {
                    continue;
                }
                let mut cm = m.clone();
                cm.push((*name).to_owned());
                me.file_of.insert((root.clone(), cm.clone()), child.clone());
                me.path_of.insert(child.clone(), (root.clone(), cm));
                queue.push_back(child);
            }
        }
        me
    }

    /// The child file a `mod name;` in `parent` declares, if it exists.
    pub fn mod_child(&self, parent: &str, name: &str) -> Option<&str> {
        self.child
            .get(&(parent.to_owned(), name.to_owned()))
            .map(String::as_str)
    }

    /// `(root, module path)` of `file`, when reached by the walk.
    pub fn module_of(&self, file: &str) -> Option<&(String, Vec<String>)> {
        self.path_of.get(file)
    }

    /// Resolve a `use`-style path written in `from` (60 §6.1): `crate`,
    /// `self`, `super` relative to the file's module; a local module (2018
    /// uniform paths); the package's own lib name; a dependency by its
    /// in-scope name; `std`/`core`/`alloc`/unknown → external. An empty head
    /// (`::tools::X`, a global path) names a crate even over a local module.
    /// The longest module prefix of the remaining segments picks the file
    /// (the item, or its enum/type, lives there).
    pub fn resolve(&self, ws: &CargoWorkspace, from: &str, segs: &[&str]) -> Target {
        let global = segs.first() == Some(&"");
        let segs = if global { &segs[1..] } else { segs };
        let Some(head) = segs.first() else {
            return Target::Unresolved;
        };
        let pkg = ws.package_of(from);
        let own = self.path_of.get(from).cloned().or_else(|| {
            pkg.and_then(|p| p.lib_root.clone())
                .map(|r| (r, Vec::new()))
        });
        match *head {
            "crate" => {
                let Some((root, _)) = own else {
                    return Target::Unresolved;
                };
                self.walk(&root, &[], &segs[1..])
            }
            "self" => {
                let Some((root, m)) = own else {
                    return Target::Unresolved;
                };
                self.walk(&root, &m, &segs[1..])
            }
            "super" => {
                let Some((root, mut m)) = own else {
                    return Target::Unresolved;
                };
                let mut rest = segs;
                while rest.first() == Some(&"super") {
                    if m.pop().is_none() {
                        return Target::Unresolved;
                    }
                    rest = &rest[1..];
                }
                self.walk(&root, &m, rest)
            }
            "std" | "core" | "alloc" | "proc_macro" => Target::External,
            h => {
                if !global && let Some((root, m)) = &own {
                    let mut local = m.clone();
                    local.push(h.to_owned());
                    if self.file_of.contains_key(&(root.clone(), local)) {
                        return self.walk(root, m, segs);
                    }
                }
                let Some(pkg) = pkg else {
                    return Target::Unknown;
                };
                let target = if h == pkg.lib_name {
                    Some(pkg)
                } else {
                    match pkg.deps.get(h) {
                        Some(dep) => match ws.by_name(dep) {
                            Some(p) => Some(p),
                            None => return Target::External,
                        },
                        None => return Target::Unknown,
                    }
                };
                match target.and_then(|p| p.lib_root.as_deref()) {
                    Some(r) if self.path_of.contains_key(r) => self.walk(r, &[], &segs[1..]),
                    _ => Target::Unresolved,
                }
            }
        }
    }

    /// Longest prefix of `rest` under `(root, base)` that is a module file.
    fn walk(&self, root: &str, base: &[String], rest: &[&str]) -> Target {
        let mut m: Vec<String> = base.to_vec();
        let mut best = self.file_of.get(&(root.to_owned(), m.clone())).cloned();
        for s in rest {
            m.push((*s).to_owned());
            match self.file_of.get(&(root.to_owned(), m.clone())) {
                Some(f) => best = Some(f.clone()),
                None => break,
            }
        }
        best.map_or(Target::Unresolved, Target::File)
    }

    /// The file of `(root, module ++ [name])` when `name` is a submodule of
    /// `file`'s module.
    pub fn submodule(&self, file: &str, name: &str) -> Option<&str> {
        let (root, m) = self.path_of.get(file)?;
        let mut mm = m.clone();
        mm.push(name.to_owned());
        self.file_of.get(&(root.clone(), mm)).map(String::as_str)
    }
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

/// Collapse `.` and `..` in a `/`-joined relative path.
fn normalize(p: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for c in p.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_dot_segments() {
        assert_eq!(normalize("a/./b/../c.rs"), "a/c.rs");
        assert_eq!(normalize("./x.rs"), "x.rs");
    }
}
