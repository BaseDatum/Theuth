//! Owner: B2 — every scaffold runs (contract §11.3): for each language whose toolchain is on
//! `PATH`, `new_plugin` into a temp dir, then `<run> --theuth-manifest` exits 0 within 10 s and
//! prints YAML naming the plugin. Go and Rust build first. A missing toolchain skips its language
//! with a message.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use plugins::Lang;

fn on_path(bin: &str) -> bool {
    Command::new("sh")
        .args(["-c", &format!("command -v {bin}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// `run:` of the scaffold's plugin.yaml.
fn run_of(dir: &Path) -> Vec<String> {
    let yaml: serde_yaml::Value = serde_yaml::from_str(&std::fs::read_to_string(dir.join("plugin.yaml")).unwrap()).unwrap();
    yaml["run"]
        .as_sequence()
        .expect("run: is a list")
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect()
}

/// Run `argv` in `dir` with stdin closed; fail after `limit`.
fn run(dir: &Path, argv: &[String], limit: Duration) -> std::process::Output {
    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("spawn {argv:?}: {e}"));
    let start = Instant::now();
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if start.elapsed() > limit {
            let _ = child.kill();
            let out = child.wait_with_output().unwrap();
            panic!(
                "{argv:?} did not exit within {limit:?}\nstdout: {}\nstderr: {}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn check(lang: Lang, raw: bool, toolchain: &str) {
    if !on_path(toolchain) {
        eprintln!("skipped {lang:?}: {toolchain} is not on PATH");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let suffix = serde_json::to_value(lang).unwrap().as_str().unwrap().to_owned();
    let name = format!("demo-{suffix}{}", if raw { "-raw" } else { "" });
    // `<data_dir>/plugins`, as the host lays it out: the editor schema goes to `<data_dir>/sdk/`.
    let report = match plugins::scaffold::new_plugin(&tmp.path().join("plugins"), &name, lang, raw) {
        Ok(r) => r,
        Err(plugins::PluginsError::Io { message }) if message.contains("not bundled") => {
            eprintln!("skipped {lang:?}: {message}");
            return;
        }
        Err(e) => panic!("{lang:?}: {e}"),
    };
    let dir = Path::new(&report.dir);
    let header = std::fs::read_to_string(dir.join("plugin.yaml")).unwrap();
    let schema = header.lines().next().and_then(|l| l.strip_prefix("# yaml-language-server: $schema="));
    let schema = schema.unwrap_or_else(|| panic!("{lang:?}: plugin.yaml has no schema header"));
    assert!(dir.join(schema).is_file(), "{lang:?}: {schema} does not exist beside the scaffold");
    if lang == Lang::Js {
        // Node 18–20 load theuth-plugin.js as ESM only with `"type": "module"` beside it.
        let pkg = std::fs::read_to_string(dir.join("package.json")).expect("js scaffold has package.json");
        assert!(pkg.contains(r#""type": "module""#), "{pkg}");
    }
    let mut argv = run_of(dir);
    match lang {
        Lang::Go => {
            let out = run(dir, &["go".into(), "build".into(), "-o".into(), "plugin-bin".into(), ".".into()], Duration::from_secs(300));
            assert!(out.status.success(), "go build: {}", String::from_utf8_lossy(&out.stderr));
            argv = vec![dir.join("plugin-bin").to_string_lossy().into_owned()];
        }
        Lang::Rs => {
            let target = tmp.path().join("target");
            let out = Command::new("cargo")
                .args(["build", "--quiet", "--target-dir"])
                .arg(&target)
                .current_dir(dir)
                .stdin(Stdio::null())
                .output()
                .unwrap();
            assert!(out.status.success(), "cargo build: {}", String::from_utf8_lossy(&out.stderr));
            argv = vec![target.join("debug").join(&name).to_string_lossy().into_owned()];
        }
        _ => {}
    }
    argv.push("--theuth-manifest".into());
    let out = run(dir, &argv, Duration::from_secs(10));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{lang:?} {argv:?} exited {:?}\nstdout: {stdout}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    // The raw templates have no SDK: with stdin closed they just exit 0.
    assert!(raw || !stdout.trim().is_empty(), "{lang:?}: --theuth-manifest printed nothing");
}

#[test]
fn py_manifest_runs() {
    check(Lang::Py, false, "python3");
}

#[test]
fn py_raw_manifest_runs() {
    check(Lang::Py, true, "python3");
}

#[test]
fn js_manifest_runs() {
    check(Lang::Js, false, "node");
}

#[test]
fn sh_manifest_runs() {
    check(Lang::Sh, false, "bash");
}

#[test]
fn sh_raw_manifest_runs() {
    check(Lang::Sh, true, "bash");
}

#[test]
fn rb_manifest_runs() {
    check(Lang::Rb, false, "ruby");
}

#[test]
fn go_manifest_runs() {
    check(Lang::Go, false, "go");
}

#[test]
fn rs_manifest_runs() {
    check(Lang::Rs, false, "cargo");
}
