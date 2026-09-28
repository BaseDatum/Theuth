//! Resolution (60 §6): imports → files per language, then names → symbols
//! with `provenance`/`confidence`/`candidates` on every `refs` row. Runs
//! after the reconcile write on the changed files plus the affected set
//! (§5.4); everything it needs is read from the store, so it is a pure
//! function of the tables.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use sqlx::SqlitePool;

use crate::parse::RECEIVER;
use crate::store::{self, FileRef, ImportRow, ImportUpdate, RefUpdate};

pub mod go;
pub mod py;
pub mod rust;
pub mod ts;

/// Where an import lands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolved {
    /// A root-relative file, with the `imports.provenance` to record.
    File(String, &'static str),
    /// Outside the root (registry crate, `node_modules`, stdlib).
    External,
    /// Looked local, no file matched.
    Unresolved,
}

/// Per-root resolvers, built once per reconcile from the manifests on disk.
pub struct Workspace {
    root: PathBuf,
    /// `Cargo.toml` model; `None` without one.
    pub rust: Option<rust::CargoWorkspace>,
    ts: ts::Ts,
    py: py::Py,
    go: Option<go::Go>,
}

impl Workspace {
    /// Read the root's manifests (`Cargo.toml`, `go.mod`, `pyproject.toml`;
    /// `tsconfig.json`/`package.json` are read lazily per file).
    pub fn load(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            rust: rust::CargoWorkspace::load(root),
            ts: ts::Ts::new(root),
            py: py::Py::new(root),
            go: go::Go::new(root),
        }
    }

    /// `files.package` for a root-relative path (60 §4): Cargo package name,
    /// npm package name, top-level Python package directory, Go directory.
    pub fn package(&self, rel: &str, language: crate::lang::Language) -> Option<String> {
        use crate::lang::Language as L;
        match language {
            L::Rust => self.rust.as_ref()?.package_of(rel).map(|p| p.name.clone()),
            L::TypeScript | L::Tsx | L::JavaScript => self.ts.package(rel),
            L::Python => {
                let mut best = None;
                let mut dir = PathBuf::new();
                let parts: Vec<&str> = rel.split('/').collect();
                for seg in &parts[..parts.len().saturating_sub(1)] {
                    dir.push(seg);
                    if self.root.join(&dir).join("__init__.py").is_file() {
                        best.get_or_insert_with(|| (*seg).to_owned());
                    } else {
                        best = None;
                    }
                }
                best
            }
            L::Go => Some(go::package(rel)),
            L::Other => None,
        }
    }
}

/// Symbol kinds a reference kind can point at (60 §6.2): a `call` is to
/// something callable (a class name is a constructor); a Rust `macro`
/// invocation only to a macro; `type`/`class`/`implementation` are to a
/// type-like definition. `constant` is never a target: nothing references
/// one by these tag kinds.
fn compatible(ref_kind: &str, sym_kind: &str) -> bool {
    match ref_kind {
        "call" => matches!(sym_kind, "function" | "method" | "macro" | "class"),
        "macro" => sym_kind == "macro",
        _ => matches!(sym_kind, "class" | "interface" | "type" | "module"),
    }
}

/// Languages whose definitions a file of `language` can reference by name:
/// TS/TSX/JS are one family, everything else only itself.
fn same_family(a: &str, b: &str) -> bool {
    let js = |l: &str| matches!(l, "typescript" | "tsx" | "javascript");
    a == b || (js(a) && js(b))
}

/// Ubiquitous method names per language (60 §21): a `.`-receiver call to one
/// of these never reaches rules 3–4 — `x.clone()` is not a reference to the
/// one hand-written `fn clone` in the tree.
fn stoplisted(language: &str, name: &str) -> bool {
    const RUST: &[&str] = &[
        "clone",
        "len",
        "is_empty",
        "push",
        "pop",
        "get",
        "insert",
        "remove",
        "iter",
        "into_iter",
        "map",
        "filter",
        "collect",
        "unwrap",
        "expect",
        "unwrap_or",
        "ok",
        "err",
        "to_string",
        "to_owned",
        "into",
        "as_ref",
        "as_str",
        "clone_from",
        "default",
        "new",
        "lock",
        "await",
    ];
    const JS: &[&str] = &[
        "map", "filter", "reduce", "push", "pop", "get", "set", "has", "then", "catch", "toString",
        "length", "forEach", "find", "includes", "join", "split", "slice",
    ];
    const PY: &[&str] = &[
        "get", "append", "items", "keys", "values", "join", "split", "strip", "format", "pop",
        "update",
    ];
    const GO: &[&str] = &["Error", "String", "Len", "Close", "Write", "Read"];
    let list = match language {
        "rust" => RUST,
        "typescript" | "tsx" | "javascript" => JS,
        "python" => PY,
        "go" => GO,
        _ => return false,
    };
    list.contains(&name)
}

/// A qualifier that is a receiver expression (`x.f()`, `self.f()`,
/// `FONTS.clone()`), not a path (`Type::f()`, `a::b::f()`): its head is not
/// a CamelCase type name and, in Rust, it has no `::`. The stored qualifier
/// drops the separator, so a lone lowercase Rust path head the module tree
/// did not know (`m::f()`) is taken for a receiver too — it lands on the
/// lower confidence, never a wrong target.
fn is_receiver(language: &str, q: &str) -> bool {
    if q == RECEIVER {
        return true;
    }
    let head = q.split(['.', ':']).next().unwrap_or(q);
    let camel =
        head.starts_with(|c: char| c.is_uppercase()) && head.chars().any(char::is_lowercase);
    !camel && (language != "rust" || !q.contains("::"))
}

/// The tables as resolution reads them, plus the Rust module tree.
struct World {
    all: Vec<FileRef>,
    by_id: HashMap<i64, usize>,
    by_path: HashMap<String, usize>,
    imports: Vec<ImportRow>,
    /// `imports.names` parsed, by index.
    names: Vec<Option<Vec<String>>>,
    imports_by_file: HashMap<i64, Vec<usize>>,
    modules: Option<rust::Modules>,
}

impl World {
    fn file(&self, id: i64) -> &FileRef {
        &self.all[self.by_id[&id]]
    }

    fn id_of(&self, path: &str) -> Option<i64> {
        self.by_path.get(path).map(|&i| self.all[i].id)
    }

    /// A Go import row names one file of the package; the package is the
    /// unit (60 §6.1), so widen to every file with the same `files.package`.
    fn extend_target(&self, t: i64, out: &mut HashSet<i64>) {
        let Some(&ti) = self.by_id.get(&t) else {
            return;
        };
        let tf = &self.all[ti];
        if tf.language == "go" {
            out.extend(
                self.all
                    .iter()
                    .filter(|f| f.language == "go" && f.package == tf.package)
                    .map(|f| f.id),
            );
        } else {
            out.insert(t);
        }
    }

    /// One level of Rust re-export (60 §6.1): `n` is not defined in file `t`
    /// but `t` has `pub use x::n` (or `pub use x::*` with `x` defining `n`)
    /// → `x`'s file.
    fn reexport(&self, t: i64, n: &str, defined: &HashSet<(i64, &str)>) -> Option<i64> {
        if defined.contains(&(t, n)) {
            return None;
        }
        let rows = self.imports_by_file.get(&t).into_iter().flatten();
        let other = |j: &usize| self.imports[*j].target_file.filter(|&g| g != t);
        rows.clone()
            .find(|&&j| {
                other(&j).is_some()
                    && self.names[j]
                        .as_ref()
                        .is_some_and(|v| v.iter().any(|x| x == n))
            })
            .or_else(|| {
                rows.clone().find(|&&j| {
                    self.names[j].is_none()
                        && !self.imports[j].specifier.starts_with("mod:")
                        && other(&j).is_some_and(|g| defined.contains(&(g, n)))
                })
            })
            .and_then(|&j| self.imports[j].target_file)
    }

    /// What a reference's qualifier names: its first segment through the
    /// import that brought it in (then down the Rust module tree), or a Rust
    /// path (`crate::a::f()`, `std::mem::swap()`, `tools::RegisterTool(..)`)
    /// through the module resolver; a Rust file that re-exports `name` widens
    /// to the re-export's source. `None`: a receiver or an unknown head — the
    /// rules decide.
    fn qualified(
        &self,
        ws: &Workspace,
        file: &FileRef,
        q: &str,
        name: &str,
        defined: &HashSet<(i64, &str)>,
    ) -> Option<Qualified> {
        if q == RECEIVER {
            return None;
        }
        let rust = file.language == "rust";
        let segs: Vec<&str> = if rust {
            q.split("::").collect()
        } else {
            q.split('.').collect()
        };
        let head = *segs.first()?;
        if matches!(head, "self" | "Self" | "this" | "cls") {
            return None;
        }
        let mut target = None;
        for &j in self.imports_by_file.get(&file.id).into_iter().flatten() {
            let imp = &self.imports[j];
            if !self.names[j]
                .as_ref()
                .is_some_and(|v| v.iter().any(|x| x == head))
            {
                continue;
            }
            if imp.provenance == "external" {
                return Some(Qualified::External);
            }
            let mut t = imp.target_file?;
            if let Some(m) = self.modules.as_ref().filter(|_| rust) {
                for s in &segs[1..] {
                    let Some(sub) = m.submodule(&self.file(t).path, s) else {
                        break;
                    };
                    t = self.id_of(sub)?;
                }
            }
            target = Some(t);
            break;
        }
        if target.is_none()
            && rust
            && let (Some(cw), Some(m)) = (&ws.rust, &self.modules)
        {
            target = match m.resolve(cw, &file.path, &segs) {
                rust::Target::File(f) => self.id_of(&f),
                rust::Target::External => return Some(Qualified::External),
                rust::Target::Unknown | rust::Target::Unresolved => return None,
            };
        }
        let t = target?;
        let mut set = HashSet::new();
        self.extend_target(t, &mut set);
        if rust && let Some(g) = self.reexport(t, name, defined) {
            set.insert(g);
        }
        Some(Qualified::Files(set))
    }
}

enum Qualified {
    Files(HashSet<i64>),
    External,
}

/// Resolve the imports and refs of `files` (root-relative paths) in place.
pub async fn resolve(
    pool: &SqlitePool,
    ws: &Workspace,
    files: &HashSet<String>,
) -> anyhow::Result<()> {
    let t0 = std::time::Instant::now();
    let all = store::all_files(pool).await?;
    let by_id: HashMap<i64, usize> = all.iter().enumerate().map(|(i, f)| (f.id, i)).collect();
    let by_path: HashMap<String, usize> = all
        .iter()
        .enumerate()
        .map(|(i, f)| (f.path.clone(), i))
        .collect();
    let target_ids: HashSet<i64> = files
        .iter()
        .filter_map(|p| by_path.get(p))
        .map(|&i| all[i].id)
        .collect();
    if target_ids.is_empty() {
        return Ok(());
    }
    let imports = store::all_imports(pool).await?;
    let names: Vec<Option<Vec<String>>> = imports
        .iter()
        .map(|i| {
            i.names
                .as_deref()
                .and_then(|n| serde_json::from_str(n).ok())
        })
        .collect();
    // Rust module tree from every `mod x;` row.
    let rust_files: HashSet<String> = all
        .iter()
        .filter(|f| f.language == "rust")
        .map(|f| f.path.clone())
        .collect();
    let mods: Vec<(String, String, Option<String>)> = imports
        .iter()
        .zip(&names)
        .filter_map(|(i, n)| {
            let name = i.specifier.strip_prefix("mod:")?;
            Some((
                all[*by_id.get(&i.file_id)?].path.clone(),
                name.to_owned(),
                n.as_ref().and_then(|v| v.first().cloned()),
            ))
        })
        .collect();
    let modules = ws
        .rust
        .as_ref()
        .map(|cw| rust::Modules::build(cw, &rust_files, &mods));
    let mut w = World {
        all,
        by_id,
        by_path,
        imports,
        names,
        imports_by_file: HashMap::new(),
        modules,
    };
    let path_set: HashSet<String> = w.by_path.keys().cloned().collect();
    let mut go_dirs: HashMap<String, Vec<String>> = HashMap::new();
    for f in w.all.iter().filter(|f| f.language == "go") {
        go_dirs
            .entry(go::package(&f.path))
            .or_default()
            .push(f.path.clone());
    }
    for v in go_dirs.values_mut() {
        v.sort();
    }
    let symbols = store::all_symbols(pool).await?;
    let mut sym_by_name: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, s) in symbols.iter().enumerate() {
        sym_by_name.entry(s.name.as_str()).or_default().push(i);
    }
    let name_by_id: HashMap<i64, &str> = symbols.iter().map(|s| (s.id, s.name.as_str())).collect();
    let defined: HashSet<(i64, &str)> = symbols
        .iter()
        .map(|s| (s.file_id, s.name.as_str()))
        .collect();

    // Imports, pass 1.
    let mut resolved = Vec::new();
    for (idx, imp) in w.imports.iter().enumerate() {
        if !target_ids.contains(&imp.file_id) {
            continue;
        }
        let file = w.file(imp.file_id);
        let res = match file.language.as_str() {
            "rust" => match (&ws.rust, &w.modules) {
                (Some(cw), Some(m)) => {
                    rust_import(cw, m, &file.path, &imp.specifier, w.names[idx].as_deref())
                }
                _ => Resolved::Unresolved,
            },
            "typescript" | "tsx" | "javascript" => ws.ts.resolve(&file.path, &imp.specifier),
            "python" => match (
                ws.py.resolve(&path_set, &file.path, &imp.specifier),
                w.names[idx].as_deref(),
            ) {
                // `from pkg import m` where `m` is a module: land on `m`'s file.
                (Resolved::File(f, prov), Some([n])) => {
                    Resolved::File(py::Py::submodule(&path_set, &f, n).unwrap_or(f), prov)
                }
                (r, _) => r,
            },
            "go" => match ws
                .go
                .as_ref()
                .and_then(|g| g.resolve(&go_dirs, &imp.specifier))
            {
                None => Resolved::External,
                // One row, one target: the rest of the package is `files.package`.
                Some(fs) => fs
                    .first()
                    .map_or(Resolved::Unresolved, |f| Resolved::File(f.clone(), "gomod")),
            },
            _ => Resolved::Unresolved,
        };
        resolved.push((idx, res));
    }
    for (idx, res) in resolved {
        let (target, prov) = match res {
            Resolved::File(p, prov) => (w.id_of(&p), prov),
            Resolved::External => (None, "external"),
            Resolved::Unresolved => (None, "unresolved"),
        };
        w.imports[idx].target_file = target;
        w.imports[idx].provenance = prov.to_owned();
    }
    for (i, imp) in w.imports.iter().enumerate() {
        w.imports_by_file.entry(imp.file_id).or_default().push(i);
    }
    // Pass 2: Rust `pub use` re-exports, one level (60 §6.1). `use a::N`
    // landed on `a`'s file, `N` is not defined there, but `a` re-exports it.
    if let Some(m) = &w.modules {
        let mut retarget = Vec::new();
        for (idx, imp) in w.imports.iter().enumerate() {
            if !target_ids.contains(&imp.file_id) || imp.provenance != "cargo" {
                continue;
            }
            let (Some(t), Some([n])) = (imp.target_file, w.names[idx].as_deref()) else {
                continue;
            };
            if m.submodule(&w.file(t).path, n).is_none()
                && let Some(g) = w.reexport(t, n, &defined)
            {
                retarget.push((idx, g));
            }
        }
        for (idx, g) in retarget {
            w.imports[idx].target_file = Some(g);
        }
    }
    let import_updates: Vec<ImportUpdate> = w
        .imports
        .iter()
        .filter(|i| target_ids.contains(&i.file_id))
        .map(|i| ImportUpdate {
            id: i.id,
            target_file: i.target_file,
            provenance: i.provenance.clone(),
        })
        .collect();
    store::update_imports(pool, &import_updates).await?;
    let imports_ms = t0.elapsed().as_millis();

    // Refs (60 §6.2), rules in order, first match wins.
    let ids: Vec<i64> = target_ids.iter().copied().collect();
    let refs = store::refs_of(pool, &ids).await?;
    let t1 = std::time::Instant::now();
    let mut out = Vec::with_capacity(refs.len());
    for r in &refs {
        let file = w.file(r.file_id);
        let c0: Vec<usize> = sym_by_name
            .get(r.name.as_str())
            .into_iter()
            .flatten()
            .copied()
            .filter(|&i| {
                compatible(&r.kind, &symbols[i].kind)
                    && same_family(&file.language, &w.file(symbols[i].file_id).language)
            })
            .collect();
        let k = c0.len() as i64;
        let done = |to: Option<usize>, prov: &'static str, conf: f64| RefUpdate {
            id: r.id,
            to_symbol: to.map(|i| symbols[i].id),
            candidates: k,
            provenance: prov,
            confidence: conf,
        };
        if c0.is_empty() {
            out.push(done(None, "none", 0.0));
            continue;
        }
        let in_files = |set: &HashSet<i64>| -> Vec<usize> {
            c0.iter()
                .copied()
                .filter(|&i| set.contains(&symbols[i].file_id))
                .collect()
        };
        // `Type::m()` / `Type.m()`: among several `m`, the one whose parent
        // (the `impl`'s type, the class) is named `Type`.
        let owner = r
            .qualifier
            .as_deref()
            .and_then(|q| q.rsplit(['.', ':']).next())
            .filter(|o| !o.is_empty());
        let by_owner = |c: Vec<usize>| -> Vec<usize> {
            let Some(o) = owner else { return c };
            let narrowed: Vec<usize> = c
                .iter()
                .copied()
                .filter(|&i| {
                    symbols[i]
                        .parent_id
                        .and_then(|p| name_by_id.get(&p))
                        .is_some_and(|n| *n == o)
                })
                .collect();
            if narrowed.is_empty() { c } else { narrowed }
        };
        // Qualifier: `a::b::n()` / `pkg.n()` — the files the qualifier names.
        if let Some(q) = &r.qualifier {
            match w.qualified(ws, file, q, &r.name, &defined) {
                Some(Qualified::Files(set)) => {
                    let c = by_owner(in_files(&set));
                    match c.len() {
                        0 => {}
                        1 => {
                            out.push(done(Some(c[0]), "import", 1.0));
                            continue;
                        }
                        _ => {
                            out.push(done(None, "ambiguous", 0.0));
                            continue;
                        }
                    }
                }
                Some(Qualified::External) => {
                    out.push(done(None, "none", 0.0));
                    continue;
                }
                None => {}
            }
        }
        // 1. same_file; several → by owner, else the innermost one enclosing the reference.
        let c1: Vec<usize> = c0
            .iter()
            .copied()
            .filter(|&i| symbols[i].file_id == r.file_id)
            .collect();
        if !c1.is_empty() {
            let c1 = by_owner(c1);
            let pick = if c1.len() == 1 {
                Some(c1[0])
            } else {
                c1.iter()
                    .copied()
                    .filter(|&i| symbols[i].line_start <= r.line && r.line <= symbols[i].line_end)
                    .min_by_key(|&i| symbols[i].line_end - symbols[i].line_start)
            };
            out.push(pick.map_or_else(
                || done(None, "ambiguous", 0.0),
                |i| done(Some(i), "same_file", 1.0),
            ));
            continue;
        }
        // 2. import. A `mod x;` row (names `None`) puts `x::` in scope, not
        // `x`'s items: it is a module-tree edge, not a glob.
        let mut imported: HashSet<i64> = HashSet::new();
        for &j in w.imports_by_file.get(&r.file_id).into_iter().flatten() {
            let Some(t) = w.imports[j].target_file else {
                continue;
            };
            if w.imports[j].specifier.starts_with("mod:") {
                continue;
            }
            if w.names[j].as_ref().is_none_or(|v| v.contains(&r.name)) {
                w.extend_target(t, &mut imported);
                // `use a::*` where `a` re-exports the name (Rust, one level).
                if file.language == "rust"
                    && let Some(g) = w.reexport(t, &r.name, &defined)
                {
                    imported.insert(g);
                }
            }
        }
        let c2 = in_files(&imported);
        if !c2.is_empty() {
            out.push(if c2.len() == 1 {
                done(Some(c2[0]), "import", 1.0)
            } else {
                done(None, "ambiguous", 0.0)
            });
            continue;
        }
        // Receiver calls no same-file/import rule explained (60 §21): a
        // stoplisted method name is not attributed at all; any other lands
        // on the same rules at half confidence — below the rank graph's cut.
        let receiver = r
            .qualifier
            .as_deref()
            .is_some_and(|q| is_receiver(&file.language, q));
        if receiver && stoplisted(&file.language, &r.name) {
            out.push(done(None, "ambiguous", 0.0));
            continue;
        }
        let (c_pkg, c_uniq) = if receiver { (0.4, 0.3) } else { (0.8, 0.6) };
        // 3. package
        if let Some(p) = file.package.as_deref() {
            let c3: Vec<usize> = c0
                .iter()
                .copied()
                .filter(|&i| w.file(symbols[i].file_id).package.as_deref() == Some(p))
                .collect();
            if c3.len() == 1 {
                out.push(done(Some(c3[0]), "package", c_pkg));
                continue;
            }
        }
        // 4. unique, 5. ambiguous
        out.push(if k == 1 {
            done(Some(c0[0]), "unique", c_uniq)
        } else {
            done(None, "ambiguous", 0.0)
        });
    }
    let rules_ms = t1.elapsed().as_millis();
    store::update_refs(pool, &out).await?;
    tracing::debug!(
        imports_ms,
        rules_ms,
        total_ms = t0.elapsed().as_millis(),
        refs = out.len(),
        "codeindex resolve phases"
    );
    Ok(())
}

/// Rust `imports` row → target (60 §6.1).
fn rust_import(
    cw: &rust::CargoWorkspace,
    m: &rust::Modules,
    from: &str,
    spec: &str,
    names: Option<&[String]>,
) -> Resolved {
    if let Some(name) = spec.strip_prefix("mod:") {
        return m.mod_child(from, name).map_or(Resolved::Unresolved, |c| {
            Resolved::File(c.to_owned(), "mod")
        });
    }
    let segs: Vec<&str> = spec.split("::").collect();
    match m.resolve(cw, from, &segs) {
        rust::Target::File(f) => {
            // `use a::b;` where `b` is a module: land on `b`'s file.
            let f = match names {
                Some([n]) => m.submodule(&f, n).map_or(f, str::to_owned),
                _ => f,
            };
            Resolved::File(f, "cargo")
        }
        rust::Target::External | rust::Target::Unknown => Resolved::External,
        rust::Target::Unresolved => Resolved::Unresolved,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds() {
        assert!(
            compatible("call", "function")
                && compatible("call", "class")
                && !compatible("call", "constant")
        );
        assert!(
            compatible("type", "interface")
                && compatible("implementation", "class")
                && !compatible("type", "function")
        );
        assert!(
            compatible("macro", "macro")
                && !compatible("macro", "function")
                && !compatible("call", "constant")
        );
        assert!(
            same_family("tsx", "javascript")
                && same_family("rust", "rust")
                && !same_family("rust", "typescript")
        );
    }

    #[test]
    fn receivers_and_stoplist() {
        assert!(
            is_receiver("rust", "x")
                && is_receiver("rust", "self")
                && is_receiver("rust", "self.items")
        );
        assert!(
            !is_receiver("rust", "Registry")
                && !is_receiver("rust", "Self")
                && !is_receiver("rust", "crate::a")
        );
        assert!(
            is_receiver("typescript", "this")
                && !is_receiver("python", "Foo")
                && is_receiver("rust", RECEIVER)
        );
        assert!(
            is_receiver("rust", "FONTS")
                && is_receiver("rust", "CONFIG.inner")
                && !is_receiver("rust", "HashMap")
        );
        assert!(
            stoplisted("rust", "clone")
                && stoplisted("typescript", "forEach")
                && stoplisted("go", "Error")
        );
        assert!(!stoplisted("rust", "reconcile") && !stoplisted("other", "clone"));
    }
}
