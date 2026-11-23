//! Owner: E2 — interpreter discovery (design 171 §6): `auto` per scope, the chip menu,
//! `interpreters(cwd)`; shells through E1's [`crate::shell::resolve`].
//!
//! Python candidates in `auto` order (§6.1, F7 a): the nearest `.venv`/`venv` walking up from
//! the cwd (to `$HOME`, or `/` outside it), the nearest `pyproject.toml` + `uv.lock` (`uv python
//! find` there), `.python-version`/`PYENV_VERSION` (`pyenv which python`), `CONDA_PREFIX`, then
//! `python3` on the login PATH. Nothing runs but those two finders and `--version`.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, Instant, SystemTime};

use parking_lot::Mutex;

use crate::error::{CellsError, Result};
use crate::types::{Interpreter, InterpreterKind, KernelSpec, PythonChoice, ShellKind};

/// §6.4: `interpreters(cwd)` is cached this long per (cwd, login env).
const CACHE_TTL: Duration = Duration::from_secs(60);
/// A finder or `--version` that hangs is abandoned after this.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// §6.1 "No Python for this folder".
pub const NO_PYTHON: &str = "No Python for this folder";

#[cfg(windows)]
const VENV_PYTHON: &[&str] = &["Scripts", "python.exe"];
#[cfg(not(windows))]
const VENV_PYTHON: &[&str] = &["bin", "python"];

type ListKey = (PathBuf, u64);
type Listed = (Instant, Vec<Interpreter>);
type Versioned = (Option<SystemTime>, Option<String>);
static LISTS: LazyLock<Mutex<HashMap<ListKey, Listed>>> = LazyLock::new(Default::default);
static VERSIONS: LazyLock<Mutex<HashMap<PathBuf, Versioned>>> = LazyLock::new(Default::default);

/// Every candidate for `cwd` with its `why`, `auto` marking what `auto`/the default picks
/// (§6.4); cached 60 s per (cwd, login-env generation).
pub async fn interpreters(cwd: &Path, login_env: &BTreeMap<String, String>) -> Vec<Interpreter> {
    let key = (cwd.to_path_buf(), env_hash(login_env));
    if let Some((at, list)) = LISTS.lock().get(&key)
        && at.elapsed() < CACHE_TTL
    {
        return list.clone();
    }
    let mut list = pythons(cwd, login_env).await;
    let any_python = !list.is_empty();
    if let Some(first) = list.first_mut() {
        first.auto = true;
    }
    list.push(Interpreter {
        auto: !any_python,
        why: if any_python {
            "built-in sandbox: pure computation on the standard library".into()
        } else {
            "no Python here: auto uses Monty when every auto cell is Monty-sufficient".into()
        },
        ..monty()
    });
    for kind in [
        ShellKind::Bash,
        ShellKind::Zsh,
        ShellKind::Sh,
        ShellKind::Pwsh,
    ] {
        if let Ok((_, mut i)) = crate::shell::resolve(kind, None, login_env).await {
            i.auto = true;
            list.push(i);
        }
    }
    LISTS.lock().insert(key, (Instant::now(), list.clone()));
    list
}

/// Resolve a python cell's interpreter (§6.1): `choice` from the header, else the note default,
/// else `auto`. `monty_ok` = every `auto` python cell of the scope is Monty-sufficient.
pub async fn resolve(
    choice: Option<&PythonChoice>,
    cwd: &Path,
    login_env: &BTreeMap<String, String>,
    monty_ok: bool,
) -> Result<(KernelSpec, Interpreter), CellsError> {
    let python = |i: Interpreter| {
        let path = i.path.clone().unwrap_or_default();
        (KernelSpec::Python { path }, i)
    };
    match choice.unwrap_or(&PythonChoice::Auto) {
        PythonChoice::Monty => Ok((KernelSpec::Monty, monty())),
        PythonChoice::Auto => {
            let list = interpreters(cwd, login_env).await;
            match list.into_iter().find(|i| i.kind == InterpreterKind::Python) {
                Some(i) => Ok(python(Interpreter { auto: true, ..i })),
                None if monty_ok => Ok((
                    KernelSpec::Monty,
                    Interpreter {
                        source: "auto → Monty".into(),
                        why: "no Python for this folder; every auto cell is Monty-sufficient"
                            .into(),
                        auto: true,
                        ..monty()
                    },
                )),
                None => Err(CellsError::NoInterpreter(NO_PYTHON.into())),
            }
        }
        PythonChoice::Venv => venv(cwd, login_env).await.map(python).ok_or_else(|| {
            CellsError::NoInterpreter(format!("no .venv above {}", tilde(cwd, login_env)))
        }),
        PythonChoice::Uv => uv(cwd, login_env).await.map(python).ok_or_else(|| {
            CellsError::NoInterpreter(format!("no uv project above {}", tilde(cwd, login_env)))
        }),
        PythonChoice::Path(p) => {
            let path = expand(p, login_env);
            if !path.is_file() {
                return Err(CellsError::NoInterpreter(format!(
                    "{} not found",
                    path.display()
                )));
            }
            let version = version(&path, login_env).await;
            Ok(python(Interpreter {
                kind: InterpreterKind::Python,
                source: "path".into(),
                why: format!("python: {}", p.display()),
                path: Some(path),
                version,
                auto: false,
            }))
        }
    }
}

/// §6.3 chip text: `Python 3.12 · .venv (~/Develop/costs)`, `Monty · sandboxed`,
/// `bash 5.2 (Homebrew)`.
pub fn chip(i: &Interpreter, login_env: &BTreeMap<String, String>) -> String {
    let ver = i.version.as_deref().map(major_minor).unwrap_or_default();
    match i.kind {
        InterpreterKind::Monty => "Monty · sandboxed".into(),
        InterpreterKind::Python if i.source == ".venv" => {
            let project = i.path.as_deref().and_then(|p| p.ancestors().nth(3));
            match project {
                Some(dir) => format!("Python {ver} · .venv ({})", tilde(dir, login_env)),
                None => format!("Python {ver} · .venv"),
            }
        }
        InterpreterKind::Python => format!("Python {ver} · {}", i.source),
        _ => crate::shell::chip(i),
    }
}

fn monty() -> Interpreter {
    Interpreter {
        kind: InterpreterKind::Monty,
        source: "Monty".into(),
        path: None,
        version: None,
        why: "built-in sandbox".into(),
        auto: false,
    }
}

/// Python candidates in `auto` order (each family at most once).
async fn pythons(cwd: &Path, env: &BTreeMap<String, String>) -> Vec<Interpreter> {
    let mut out = Vec::new();
    out.extend(venv(cwd, env).await);
    out.extend(uv(cwd, env).await);
    out.extend(pyenv(cwd, env).await);
    out.extend(conda(env).await);
    out.extend(system(env).await);
    out
}

/// The directories `auto` walks: `cwd` up to `$HOME`, or to `/` when `cwd` is outside it.
fn walk(cwd: &Path, env: &BTreeMap<String, String>) -> Vec<PathBuf> {
    let home = home(env);
    let inside = home.as_deref().is_some_and(|h| cwd.starts_with(h));
    let mut out = Vec::new();
    for dir in cwd.ancestors() {
        out.push(dir.to_path_buf());
        if inside && Some(dir) == home.as_deref() {
            break;
        }
    }
    out
}

async fn venv(cwd: &Path, env: &BTreeMap<String, String>) -> Option<Interpreter> {
    for dir in walk(cwd, env) {
        for name in [".venv", "venv"] {
            let py = VENV_PYTHON.iter().fold(dir.join(name), |p, s| p.join(s));
            if py.is_file() {
                let version = version(&py, env).await;
                return Some(Interpreter {
                    kind: InterpreterKind::Python,
                    source: ".venv".into(),
                    why: format!("nearest {name}: {}", tilde(&dir.join(name), env)),
                    path: Some(py),
                    version,
                    auto: false,
                });
            }
        }
    }
    None
}

async fn uv(cwd: &Path, env: &BTreeMap<String, String>) -> Option<Interpreter> {
    let dir = walk(cwd, env)
        .into_iter()
        .find(|d| d.join("pyproject.toml").is_file() && d.join("uv.lock").is_file())?;
    let uv = which("uv", env)?;
    let out = probe(&uv, &["python", "find"], Some(&dir), env).await?;
    let py = PathBuf::from(out.lines().next()?.trim());
    let version = version(&py, env).await;
    let project = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    Some(Interpreter {
        kind: InterpreterKind::Python,
        source: format!("uv:{project}"),
        why: format!("uv project: {}", tilde(&dir, env)),
        path: Some(py),
        version,
        auto: false,
    })
}

async fn pyenv(cwd: &Path, env: &BTreeMap<String, String>) -> Option<Interpreter> {
    let file = walk(cwd, env).into_iter().find_map(|d| {
        let text = std::fs::read_to_string(d.join(".python-version")).ok()?;
        let v = text.lines().next()?.trim().to_string();
        (!v.is_empty()).then_some((d, v))
    });
    let (dir, v, why) = match file {
        Some((d, v)) => {
            let why = format!(
                ".python-version: {}",
                tilde(&d.join(".python-version"), env)
            );
            (d, v, why)
        }
        None => {
            let v = env.get("PYENV_VERSION").filter(|v| !v.is_empty())?.clone();
            (cwd.to_path_buf(), v.clone(), format!("PYENV_VERSION={v}"))
        }
    };
    let pyenv = which("pyenv", env)?;
    let out = probe(&pyenv, &["which", "python"], Some(&dir), env).await?;
    let py = PathBuf::from(out.lines().next()?.trim());
    let version = version(&py, env).await;
    Some(Interpreter {
        kind: InterpreterKind::Python,
        source: format!("pyenv:{v}"),
        why,
        path: Some(py),
        version,
        auto: false,
    })
}

async fn conda(env: &BTreeMap<String, String>) -> Option<Interpreter> {
    let prefix = PathBuf::from(env.get("CONDA_PREFIX").filter(|p| !p.is_empty())?);
    let py = if cfg!(windows) {
        prefix.join("python.exe")
    } else {
        prefix.join("bin").join("python")
    };
    if !py.is_file() {
        return None;
    }
    let name = env
        .get("CONDA_DEFAULT_ENV")
        .cloned()
        .or_else(|| prefix.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_default();
    let version = version(&py, env).await;
    Some(Interpreter {
        kind: InterpreterKind::Python,
        source: format!("conda:{name}"),
        why: format!("CONDA_PREFIX={}", tilde(&prefix, env)),
        path: Some(py),
        version,
        auto: false,
    })
}

async fn system(env: &BTreeMap<String, String>) -> Option<Interpreter> {
    #[cfg(windows)]
    let py = {
        let py = which("py", env)?;
        let out = probe(
            &py,
            &["-3", "-c", "import sys; print(sys.executable)"],
            None,
            env,
        )
        .await?;
        PathBuf::from(out.trim())
    };
    #[cfg(not(windows))]
    let py = which("python3", env)?;
    let version = version(&py, env).await;
    Some(Interpreter {
        kind: InterpreterKind::Python,
        source: "system".into(),
        why: format!("python3 on the login PATH: {}", tilde(&py, env)),
        path: Some(py),
        version,
        auto: false,
    })
}

/// `<path> --version` → `3.12.4`, cached per path and mtime.
pub async fn version(path: &Path, env: &BTreeMap<String, String>) -> Option<String> {
    let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    if let Some((m, v)) = VERSIONS.lock().get(path)
        && *m == mtime
    {
        return v.clone();
    }
    let v = probe(path, &["--version"], None, env)
        .await
        .and_then(|out| parse_version(&out));
    VERSIONS
        .lock()
        .insert(path.to_path_buf(), (mtime, v.clone()));
    v
}

/// The first dotted number in a `--version` line (`Python 3.12.4`, `GNU bash, version 5.2.37(1)`).
fn parse_version(out: &str) -> Option<String> {
    out.split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .find(|w| w.contains('.') && w.starts_with(|c: char| c.is_ascii_digit()))
        .map(|w| w.trim_end_matches('.').to_string())
}

fn major_minor(v: &str) -> String {
    v.split('.').take(2).collect::<Vec<_>>().join(".")
}

/// Run a finder with the login env; stdout+stderr when it exits 0.
async fn probe(
    bin: &Path,
    args: &[&str],
    dir: Option<&Path>,
    env: &BTreeMap<String, String>,
) -> Option<String> {
    let mut cmd = tokio::process::Command::new(bin);
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    if !env.is_empty() {
        cmd.env_clear().envs(env);
    }
    if let Some(d) = dir {
        cmd.current_dir(d);
    }
    platform::process::hide_console(cmd.as_std_mut());
    let out = tokio::time::timeout(PROBE_TIMEOUT, cmd.output())
        .await
        .ok()?
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Some(text)
}

/// `name` on the login PATH (the engine's PATH when the login env has none).
fn which(name: &str, env: &BTreeMap<String, String>) -> Option<PathBuf> {
    let path = env
        .get("PATH")
        .map(std::ffi::OsString::from)
        .or_else(|| std::env::var_os("PATH"))?;
    platform::shell::which_in(name, &path)
}

fn home(env: &BTreeMap<String, String>) -> Option<PathBuf> {
    env.get("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
}

/// `~/x` → `$HOME/x`.
fn expand(p: &Path, env: &BTreeMap<String, String>) -> PathBuf {
    match (p.strip_prefix("~"), home(env)) {
        (Ok(rest), Some(h)) => h.join(rest),
        _ => p.to_path_buf(),
    }
}

/// `$HOME/x` → `~/x` for chips and `why`.
fn tilde(p: &Path, env: &BTreeMap<String, String>) -> String {
    match home(env).and_then(|h| p.strip_prefix(h).ok().map(Path::to_path_buf)) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => p.display().to_string(),
    }
}

fn env_hash(env: &BTreeMap<String, String>) -> u64 {
    let mut h = DefaultHasher::new();
    env.hash(&mut h);
    h.finish()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A fake interpreter: a script printing `Python <v>`.
    fn fake(path: &Path, v: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, format!("#!/bin/sh\necho 'Python {v}'\n")).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// A login env whose HOME is the temp root and whose PATH holds only `bin`.
    fn env(home: &Path, bin: &Path) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("HOME".into(), home.display().to_string()),
            ("PATH".into(), bin.display().to_string()),
        ])
    }

    #[tokio::test]
    async fn nearest_venv_wins_over_system_and_is_auto() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().canonicalize().unwrap();
        let bin = root.join("bin");
        fake(&bin.join("python3"), "3.9.6");
        fake(&root.join("costs/.venv/bin/python"), "3.12.4");
        let cwd = root.join("costs/src/deep");
        std::fs::create_dir_all(&cwd).unwrap();
        let e = env(&root, &bin);

        let list = interpreters(&cwd, &e).await;
        let py: Vec<_> = list
            .iter()
            .filter(|i| i.kind == InterpreterKind::Python)
            .collect();
        assert_eq!(py.len(), 2, "{list:?}");
        assert_eq!((py[0].source.as_str(), py[0].auto), (".venv", true));
        assert_eq!(py[0].version.as_deref(), Some("3.12.4"));
        assert_eq!(chip(py[0], &e), "Python 3.12 · .venv (~/costs)");
        assert_eq!((py[1].source.as_str(), py[1].auto), ("system", false));
        assert_eq!(chip(py[1], &e), "Python 3.9 · system");
        let monty = list
            .iter()
            .find(|i| i.kind == InterpreterKind::Monty)
            .unwrap();
        assert!(!monty.auto);
        assert_eq!(chip(monty, &e), "Monty · sandboxed");

        let (spec, i) = resolve(None, &cwd, &e, true).await.unwrap();
        assert_eq!(
            spec,
            KernelSpec::Python {
                path: root.join("costs/.venv/bin/python")
            }
        );
        assert!(i.auto);
        // Explicit monty is honoured even with a venv present.
        let (spec, _) = resolve(Some(&PythonChoice::Monty), &cwd, &e, false)
            .await
            .unwrap();
        assert_eq!(spec, KernelSpec::Monty);
    }

    #[tokio::test]
    async fn venv_outside_home_walk_stops_at_home() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().canonicalize().unwrap();
        // A venv ABOVE $HOME is not found from inside $HOME.
        fake(&root.join(".venv/bin/python"), "3.11.0");
        let home = root.join("home");
        let cwd = home.join("proj");
        std::fs::create_dir_all(&cwd).unwrap();
        let empty = root.join("nobin");
        let e = BTreeMap::from([
            ("HOME".into(), home.display().to_string()),
            ("PATH".into(), empty.display().to_string()),
        ]);
        assert!(venv(&cwd, &e).await.is_none());
        // Outside $HOME the walk goes to `/` and finds it.
        let outside = root.join("elsewhere");
        std::fs::create_dir_all(&outside).unwrap();
        assert_eq!(
            venv(&outside, &e).await.unwrap().version.as_deref(),
            Some("3.11.0")
        );
        let err = resolve(Some(&PythonChoice::Venv), &cwd, &e, true)
            .await
            .unwrap_err();
        assert!(matches!(err, CellsError::NoInterpreter(_)), "{err:?}");
    }

    #[tokio::test]
    async fn no_python_falls_back_to_monty_only_when_sufficient() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().canonicalize().unwrap();
        let cwd = root.join("p");
        std::fs::create_dir_all(&cwd).unwrap();
        let e = BTreeMap::from([
            ("HOME".into(), root.display().to_string()),
            ("PATH".into(), root.join("nobin").display().to_string()),
        ]);
        let (spec, i) = resolve(Some(&PythonChoice::Auto), &cwd, &e, true)
            .await
            .unwrap();
        assert_eq!(spec, KernelSpec::Monty);
        assert_eq!(i.source, "auto → Monty");
        let err = resolve(None, &cwd, &e, false).await.unwrap_err();
        assert_eq!(err, CellsError::NoInterpreter(NO_PYTHON.into()));
        let list = interpreters(&cwd, &e).await;
        assert!(
            list.iter()
                .any(|i| i.kind == InterpreterKind::Monty && i.auto)
        );
    }

    #[tokio::test]
    async fn uv_pyenv_conda_and_explicit_path() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().canonicalize().unwrap();
        let bin = root.join("bin");
        let uvpy = root.join("uvpy/python3.13");
        fake(&uvpy, "3.13.1");
        // `uv python find` prints the interpreter; `pyenv which python` likewise.
        let script = |p: &Path, out: &Path| {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, format!("#!/bin/sh\necho '{}'\n", out.display())).unwrap();
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        script(&bin.join("uv"), &uvpy);
        let pyenvpy = root.join("pyenvpy/python");
        fake(&pyenvpy, "3.12.4");
        script(&bin.join("pyenv"), &pyenvpy);
        let conda = root.join("conda");
        fake(&conda.join("bin/python"), "3.11.9");

        let proj = root.join("costs");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("pyproject.toml"), "").unwrap();
        std::fs::write(proj.join("uv.lock"), "").unwrap();
        std::fs::write(proj.join(".python-version"), "3.12.4\n").unwrap();
        let mut e = env(&root, &bin);
        e.insert("CONDA_PREFIX".into(), conda.display().to_string());
        e.insert("CONDA_DEFAULT_ENV".into(), "base".into());

        let list = interpreters(&proj, &e).await;
        let sources: Vec<_> = list
            .iter()
            .filter(|i| i.kind == InterpreterKind::Python)
            .map(|i| (i.source.as_str(), i.auto))
            .collect();
        assert_eq!(
            sources,
            [
                ("uv:costs", true),
                ("pyenv:3.12.4", false),
                ("conda:base", false)
            ],
            "{list:?}"
        );
        assert_eq!(chip(&list[0], &e), "Python 3.13 · uv:costs");
        let (spec, _) = resolve(Some(&PythonChoice::Uv), &proj, &e, false)
            .await
            .unwrap();
        assert_eq!(spec, KernelSpec::Python { path: uvpy });
        let (spec, i) = resolve(
            Some(&PythonChoice::Path("~/conda/bin/python".into())),
            &proj,
            &e,
            false,
        )
        .await
        .unwrap();
        assert_eq!(
            spec,
            KernelSpec::Python {
                path: conda.join("bin/python")
            }
        );
        assert_eq!(i.version.as_deref(), Some("3.11.9"));
        assert!(
            resolve(
                Some(&PythonChoice::Path("/nope/python".into())),
                &proj,
                &e,
                false
            )
            .await
            .is_err()
        );
    }

    #[test]
    fn versions_and_shell_chips() {
        assert_eq!(parse_version("Python 3.12.4\n").as_deref(), Some("3.12.4"));
        assert_eq!(
            parse_version("GNU bash, version 5.2.37(1)-release (aarch64-apple-darwin24)")
                .as_deref(),
            Some("5.2.37")
        );
        assert_eq!(
            parse_version("zsh 5.9 (arm64-apple-darwin24.0)").as_deref(),
            Some("5.9")
        );
        let e = BTreeMap::new();
        let bash = |source: &str, v: &str| Interpreter {
            kind: InterpreterKind::Bash,
            source: source.into(),
            path: None,
            version: Some(v.into()),
            why: String::new(),
            auto: true,
        };
        assert_eq!(chip(&bash("homebrew", "5.2.37"), &e), "bash 5.2 (Homebrew)");
        assert_eq!(chip(&bash("macOS", "3.2.57"), &e), "bash 3.2 (macOS)");
    }
}
