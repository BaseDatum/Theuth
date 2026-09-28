//! `project_profile` (60 §8): how a project is checked, read from its
//! manifests, lockfiles and task runners. Detection only — nothing here runs
//! a command. Deterministic: the same tree gives the same answer.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;

/// The answer: one project kind (or `mixed`), the commands an orchestrator
/// should brief a child with, and where they came from.
#[derive(Debug, Default, Serialize)]
pub struct Profile {
    /// Canonical root.
    pub root: std::path::PathBuf,
    /// `cargo` | `node` | `python` | `go` | `mixed` | `unknown`.
    pub kind: String,
    /// `cargo` | `npm` | `pnpm` | `bun` | `yarn` | `uv` | `poetry` | `pipenv` | `pip` | `go`.
    pub package_manager: Option<String>,
    /// The check commands; a task runner recipe (`just`, then `make`) wins
    /// over the manifest's default when a recipe of that name exists.
    pub commands: Commands,
    /// `cargo test` | `bun test` | `vitest` | `jest` | `mocha` | `pytest` | `go test` | ….
    pub test_runner: Option<String>,
    /// CI definitions found, root-relative.
    pub ci: Vec<String>,
    /// Manifests and lockfiles found, root-relative.
    pub manifests: Vec<String>,
    /// Named scripts and recipes: `package.json` scripts by name with their
    /// command, justfile recipes as `just <name>`, Makefile targets as
    /// `make <name>`. Absent when there are none.
    pub scripts: Option<BTreeMap<String, String>>,
}

/// One command per slot; `None` when nothing declares it.
#[derive(Debug, Default, Serialize)]
pub struct Commands {
    /// Compiles without producing artifacts (`cargo check`, `go vet`).
    pub check: Option<String>,
    /// Runs the test suite.
    pub test: Option<String>,
    /// Lints (`clippy`, `biome check`, `ruff check`, `golangci-lint`).
    pub lint: Option<String>,
    /// Formats in place.
    pub format: Option<String>,
    /// Builds artifacts.
    pub build: Option<String>,
    /// Type-checks (`tsc --noEmit`, `mypy`); `None` for compiled languages, `check` covers it.
    pub typecheck: Option<String>,
}

impl Commands {
    /// Slot name → value, output order.
    pub fn slots(&self) -> [(&'static str, Option<&str>); 6] {
        [
            ("check", self.check.as_deref()),
            ("test", self.test.as_deref()),
            ("lint", self.lint.as_deref()),
            ("format", self.format.as_deref()),
            ("build", self.build.as_deref()),
            ("typecheck", self.typecheck.as_deref()),
        ]
    }

    fn set_if_empty(&mut self, slot: &str, v: String) {
        let s = match slot {
            "check" => &mut self.check,
            "test" => &mut self.test,
            "lint" => &mut self.lint,
            "format" => &mut self.format,
            "build" => &mut self.build,
            "typecheck" => &mut self.typecheck,
            _ => return,
        };
        if s.is_none() {
            *s = Some(v);
        }
    }
}

/// Recipe/target/script names that fill a slot, first match wins.
const SLOT_ALIASES: [(&str, &[&str]); 6] = [
    ("check", &["check"]),
    ("test", &["test"]),
    ("lint", &["lint", "clippy"]),
    ("format", &["fmt", "format"]),
    ("build", &["build"]),
    ("typecheck", &["typecheck", "type-check", "tsc"]),
];

/// Detect the profile of `root` (canonical). Reads at most a handful of
/// small files; ≤ 20 ms warm.
pub fn detect(root: &Path) -> Profile {
    let mut p = Profile {
        root: root.to_path_buf(),
        ..Default::default()
    };
    let has = |rel: &str| root.join(rel).is_file();
    let read = |rel: &str| std::fs::read_to_string(root.join(rel)).ok();
    let mut kinds: Vec<&str> = Vec::new();
    let mut scripts: BTreeMap<String, String> = BTreeMap::new();

    // Task runners first: their recipes are the project's own words.
    if let Some(src) = read("justfile").or_else(|| read("Justfile")) {
        p.manifests.push("justfile".into());
        for name in just_recipes(&src) {
            scripts.insert(name.clone(), format!("just {name}"));
            for (slot, aliases) in SLOT_ALIASES {
                if aliases.contains(&name.as_str()) {
                    p.commands.set_if_empty(slot, format!("just {name}"));
                }
            }
        }
    }
    if let Some(src) = read("Makefile").or_else(|| read("makefile")) {
        p.manifests.push("Makefile".into());
        for name in make_targets(&src) {
            scripts
                .entry(name.clone())
                .or_insert_with(|| format!("make {name}"));
            for (slot, aliases) in SLOT_ALIASES {
                if aliases.contains(&name.as_str()) {
                    p.commands.set_if_empty(slot, format!("make {name}"));
                }
            }
        }
    }

    if let Some(src) = read("Cargo.toml") {
        kinds.push("cargo");
        p.manifests.push("Cargo.toml".into());
        if has("Cargo.lock") {
            p.manifests.push("Cargo.lock".into());
        }
        p.package_manager.get_or_insert_with(|| "cargo".into());
        let ws = if src.contains("[workspace]") {
            " --workspace"
        } else {
            ""
        };
        p.commands.set_if_empty("check", format!("cargo check{ws}"));
        p.commands.set_if_empty("test", format!("cargo test{ws}"));
        p.commands
            .set_if_empty("lint", format!("cargo clippy{ws} --all-targets"));
        p.commands.set_if_empty("format", "cargo fmt --all".into());
        p.commands.set_if_empty("build", format!("cargo build{ws}"));
        p.test_runner.get_or_insert_with(|| "cargo test".into());
    }

    if let Some(src) = read("package.json") {
        kinds.push("node");
        p.manifests.push("package.json".into());
        let pm = [
            ("bun.lock", "bun"),
            ("bun.lockb", "bun"),
            ("pnpm-lock.yaml", "pnpm"),
            ("yarn.lock", "yarn"),
            ("package-lock.json", "npm"),
        ]
        .into_iter()
        .find(|(lock, _)| has(lock))
        .map(|(lock, pm)| {
            p.manifests.push(lock.into());
            pm
        })
        .unwrap_or("npm");
        p.package_manager.get_or_insert_with(|| pm.into());
        let pkg: serde_json::Value = serde_json::from_str(&src).unwrap_or_default();
        let pkg_scripts: BTreeMap<String, String> = pkg["scripts"]
            .as_object()
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
                    .collect()
            })
            .unwrap_or_default();
        for (slot, aliases) in SLOT_ALIASES {
            if let Some(name) = aliases.iter().find(|a| pkg_scripts.contains_key(**a)) {
                p.commands.set_if_empty(slot, format!("{pm} run {name}"));
            }
        }
        if has("tsconfig.json") {
            p.manifests.push("tsconfig.json".into());
            let exec = match pm {
                "bun" => "bunx",
                "pnpm" => "pnpm exec",
                "yarn" => "yarn",
                _ => "npx",
            };
            p.commands
                .set_if_empty("typecheck", format!("{exec} tsc --noEmit"));
        }
        let dev = |name: &str| {
            pkg["devDependencies"].get(name).is_some() || pkg["dependencies"].get(name).is_some()
        };
        let test_script = pkg_scripts.get("test").map(String::as_str).unwrap_or("");
        let runner = ["vitest", "jest", "mocha", "ava", "node --test", "bun test"]
            .into_iter()
            .find(|r| test_script.contains(r))
            .or_else(|| {
                ["vitest", "jest", "mocha", "ava"]
                    .into_iter()
                    .find(|r| dev(r))
            })
            .or(if pm == "bun" { Some("bun test") } else { None });
        if let Some(r) = runner {
            p.test_runner.get_or_insert_with(|| r.into());
        }
        for (k, v) in pkg_scripts {
            scripts.entry(k).or_insert(v);
        }
    }

    let py_manifest = [
        "pyproject.toml",
        "setup.py",
        "setup.cfg",
        "requirements.txt",
    ]
    .into_iter()
    .find(|m| has(m));
    if let Some(m) = py_manifest {
        kinds.push("python");
        p.manifests.push(m.into());
        let pyproject = read("pyproject.toml").unwrap_or_default();
        let pm = [
            ("uv.lock", "uv"),
            ("poetry.lock", "poetry"),
            ("Pipfile.lock", "pipenv"),
        ]
        .into_iter()
        .find(|(lock, _)| has(lock))
        .map(|(lock, pm)| {
            p.manifests.push(lock.into());
            pm
        })
        .unwrap_or_else(|| {
            if pyproject.contains("[tool.poetry]") {
                "poetry"
            } else if pyproject.contains("[tool.uv]") {
                "uv"
            } else {
                "pip"
            }
        });
        p.package_manager.get_or_insert_with(|| pm.into());
        let run = match pm {
            "uv" => "uv run ",
            "poetry" => "poetry run ",
            "pipenv" => "pipenv run ",
            _ => "",
        };
        let mentions = |tool: &str| pyproject.contains(tool);
        if mentions("pytest")
            || root.join("tests").is_dir()
            || has("pytest.ini")
            || has("conftest.py")
        {
            p.commands.set_if_empty("test", format!("{run}pytest"));
            p.test_runner.get_or_insert_with(|| "pytest".into());
        }
        if mentions("ruff") {
            p.commands
                .set_if_empty("lint", format!("{run}ruff check ."));
            p.commands
                .set_if_empty("format", format!("{run}ruff format ."));
        } else {
            if mentions("flake8") {
                p.commands.set_if_empty("lint", format!("{run}flake8"));
            }
            if mentions("black") {
                p.commands.set_if_empty("format", format!("{run}black ."));
            }
        }
        if mentions("mypy") {
            p.commands.set_if_empty("typecheck", format!("{run}mypy ."));
        } else if mentions("pyright") {
            p.commands
                .set_if_empty("typecheck", format!("{run}pyright"));
        }
    }

    if has("go.mod") {
        kinds.push("go");
        p.manifests.push("go.mod".into());
        if has("go.sum") {
            p.manifests.push("go.sum".into());
        }
        p.package_manager.get_or_insert_with(|| "go".into());
        p.commands.set_if_empty("check", "go vet ./...".into());
        p.commands.set_if_empty("test", "go test ./...".into());
        let lint = if has(".golangci.yml") || has(".golangci.yaml") || has(".golangci.toml") {
            "golangci-lint run"
        } else {
            "go vet ./..."
        };
        p.commands.set_if_empty("lint", lint.into());
        p.commands.set_if_empty("format", "gofmt -l .".into());
        p.commands.set_if_empty("build", "go build ./...".into());
        p.test_runner.get_or_insert_with(|| "go test".into());
    }

    p.kind = match kinds.as_slice() {
        [] => "unknown",
        [k] => k,
        _ => "mixed",
    }
    .into();

    // CI: GitHub workflows (sorted), GitLab, CircleCI, Jenkins.
    if let Ok(rd) = std::fs::read_dir(root.join(".github/workflows")) {
        let mut names: Vec<String> = rd
            .flatten()
            .filter_map(|e| {
                let n = e.file_name().to_string_lossy().into_owned();
                (n.ends_with(".yml") || n.ends_with(".yaml")).then_some(n)
            })
            .collect();
        names.sort_unstable();
        p.ci.extend(names.into_iter().map(|n| format!(".github/workflows/{n}")));
    }
    for f in [".gitlab-ci.yml", ".circleci/config.yml", "Jenkinsfile"] {
        if has(f) {
            p.ci.push(f.into());
        }
    }
    if !scripts.is_empty() {
        p.scripts = Some(scripts);
    }
    p
}

/// Recipe names of a justfile: a line starting at column 0 with
/// `name [params…]:` that is not a setting, assignment, attribute or comment.
fn just_recipes(src: &str) -> Vec<String> {
    src.lines()
        .filter(|l| !l.starts_with([' ', '\t', '#', '[', '@']) && !l.contains(":="))
        .filter(|l| {
            !["set ", "export ", "import", "mod ", "alias "]
                .iter()
                .any(|k| l.starts_with(k))
        })
        .filter_map(|l| {
            let head = l.split_once(':')?.0;
            let name = head.split_whitespace().next()?;
            // `_name` is a private recipe by just convention.
            (is_recipe_name(name) && !name.starts_with('_')).then(|| name.to_owned())
        })
        .collect()
}

/// Target names of a Makefile: `name:` at column 0, skipping variables,
/// specials (`.PHONY`) and pattern rules.
fn make_targets(src: &str) -> Vec<String> {
    src.lines()
        .filter(|l| !l.starts_with([' ', '\t', '#', '.']) && !l.contains('='))
        .filter_map(|l| {
            let head = l.split_once(':')?.0.trim();
            (is_recipe_name(head) && !head.contains('%')).then(|| head.to_owned())
        })
        .collect()
}

fn is_recipe_name(s: &str) -> bool {
    let mut cs = s.chars();
    cs.next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && cs.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/'))
}

/// Compact text render: one line per non-empty field.
pub fn render(p: &Profile) -> String {
    let mut out = format!(
        "{} project ({}); package manager: {}; test runner: {}",
        p.kind,
        p.root.display(),
        p.package_manager.as_deref().unwrap_or("-"),
        p.test_runner.as_deref().unwrap_or("-"),
    );
    for (slot, v) in p.commands.slots() {
        if let Some(v) = v {
            out.push_str(&format!("\n{slot}: {v}"));
        }
    }
    if !p.manifests.is_empty() {
        out.push_str(&format!("\nmanifests: {}", p.manifests.join(", ")));
    }
    if !p.ci.is_empty() {
        out.push_str(&format!("\nci: {}", p.ci.join(", ")));
    }
    if let Some(s) = &p.scripts {
        let names: Vec<&str> = s.keys().map(String::as_str).collect();
        out.push_str(&format!(
            "\nscripts ({}): {}",
            names.len(),
            names.join(", ")
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(files: &[(&str, &str)]) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("theuth-profile-{}", uuid::Uuid::new_v4()));
        for (p, s) in files {
            let f = d.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, s).unwrap();
        }
        d
    }

    #[test]
    fn justfile_and_makefile_names() {
        let j = "set shell := [\"bash\"]\nbin := `x`\n# c\ntest *ARGS:\n    cargo test {{ARGS}}\n[private]\n_helper:\n  echo\nfmt:\n    cargo fmt\nbuild-remote host os *ARGS:\n    x\n";
        assert_eq!(just_recipes(j), ["test", "fmt", "build-remote"]);
        let m =
            ".PHONY: test\nCC = gcc\ntest: build\n\tgo test ./...\n%.o: %.c\n\tcc\nlint:\n\tx\n";
        assert_eq!(make_targets(m), ["test", "lint"]);
    }

    #[test]
    fn cargo_workspace_with_justfile_prefers_recipes() {
        let d = tmp(&[
            ("Cargo.toml", "[workspace]\nmembers = [\"a\"]\n"),
            ("Cargo.lock", ""),
            (
                "justfile",
                "test *ARGS:\n    cargo test --workspace {{ARGS}}\nclippy:\n    cargo clippy\n",
            ),
            (".github/workflows/ci.yml", "on: push\n"),
        ]);
        let p = detect(&d);
        assert_eq!(p.kind, "cargo");
        assert_eq!(p.package_manager.as_deref(), Some("cargo"));
        assert_eq!(p.commands.test.as_deref(), Some("just test"));
        assert_eq!(p.commands.lint.as_deref(), Some("just clippy"));
        assert_eq!(p.commands.check.as_deref(), Some("cargo check --workspace"));
        assert_eq!(p.commands.format.as_deref(), Some("cargo fmt --all"));
        assert_eq!(p.commands.typecheck, None);
        assert_eq!(p.test_runner.as_deref(), Some("cargo test"));
        assert_eq!(p.ci, [".github/workflows/ci.yml"]);
        assert_eq!(p.manifests, ["justfile", "Cargo.toml", "Cargo.lock"]);
        assert_eq!(p.scripts.unwrap().len(), 2);
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn bun_project_from_lockfile_and_scripts() {
        let d = tmp(&[
            (
                "package.json",
                r#"{"scripts":{"test":"bun test","lint":"biome check src/","format":"biome format --write src/","typecheck":"tsc --noEmit","build":"bun scripts/build.ts"},"devDependencies":{"typescript":"5"}}"#,
            ),
            ("bun.lock", ""),
            ("tsconfig.json", "{}"),
        ]);
        let p = detect(&d);
        assert_eq!(
            (p.kind.as_str(), p.package_manager.as_deref()),
            ("node", Some("bun"))
        );
        assert_eq!(p.commands.test.as_deref(), Some("bun run test"));
        assert_eq!(p.commands.typecheck.as_deref(), Some("bun run typecheck"));
        assert_eq!(p.commands.lint.as_deref(), Some("bun run lint"));
        assert_eq!(p.commands.check, None);
        assert_eq!(p.test_runner.as_deref(), Some("bun test"));
        assert_eq!(p.scripts.as_ref().unwrap()["build"], "bun scripts/build.ts");
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn pnpm_vitest_typecheck_fallback_and_mixed_with_go() {
        let d = tmp(&[
            ("package.json", r#"{"scripts":{"test":"vitest run"}}"#),
            ("pnpm-lock.yaml", ""),
            ("tsconfig.json", "{}"),
            ("go.mod", "module x\n"),
            (".golangci.yml", ""),
        ]);
        let p = detect(&d);
        assert_eq!(p.kind, "mixed");
        assert_eq!(p.package_manager.as_deref(), Some("pnpm"));
        assert_eq!(
            p.commands.typecheck.as_deref(),
            Some("pnpm exec tsc --noEmit")
        );
        assert_eq!(p.commands.test.as_deref(), Some("pnpm run test"));
        assert_eq!(p.test_runner.as_deref(), Some("vitest"));
        assert_eq!(p.commands.lint.as_deref(), Some("golangci-lint run"));
        assert_eq!(p.commands.build.as_deref(), Some("go build ./..."));
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn python_uv_ruff_and_empty_root() {
        let d = tmp(&[
            (
                "pyproject.toml",
                "[tool.ruff]\nline-length = 100\n[tool.mypy]\n[tool.pytest.ini_options]\n",
            ),
            ("uv.lock", ""),
        ]);
        let p = detect(&d);
        assert_eq!(
            (p.kind.as_str(), p.package_manager.as_deref()),
            ("python", Some("uv"))
        );
        assert_eq!(p.commands.test.as_deref(), Some("uv run pytest"));
        assert_eq!(p.commands.lint.as_deref(), Some("uv run ruff check ."));
        assert_eq!(p.commands.format.as_deref(), Some("uv run ruff format ."));
        assert_eq!(p.commands.typecheck.as_deref(), Some("uv run mypy ."));
        assert!(render(&p).contains("test: uv run pytest"));
        std::fs::remove_dir_all(&d).unwrap();
        let e = tmp(&[("README.md", "")]);
        let p = detect(&e);
        assert_eq!(
            (p.kind.as_str(), p.package_manager, p.scripts),
            ("unknown", None, None)
        );
        std::fs::remove_dir_all(e).unwrap();
    }
}
