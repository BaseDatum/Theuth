//! Cell grammar (171 §3) against the shared fixtures `cells.json`, read by this test and by
//! the TypeScript mirror's.

use std::path::PathBuf;

use serde_json::Value;
use surface::cell::{
    Cell, ChipKind, NoteDefaults, ValidateCtx, digest, lint, lint_scripts, monty_reason,
    monty_scan, parse, parse_defaults, parse_duration, validate,
};

const FIXTURES: &str = include_str!("cells.json");

fn fixtures() -> Value {
    serde_json::from_str(FIXTURES).expect("cells.json parses")
}

fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn pairs(v: Option<&Value>) -> Vec<(String, String)> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|p| {
                    let p = p.as_array()?;
                    Some((
                        p.first()?.as_str()?.to_string(),
                        p.get(1)?.as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Fixture `ctx` (every key optional) over the default: chat, this cell's own name in scope.
fn ctx_of(fixture: &Value, cell: &Cell) -> ValidateCtx {
    let c = fixture.get("ctx").cloned().unwrap_or(Value::Null);
    let names = match c.get("names") {
        Some(n) => strings(Some(n)),
        None => cell.options.name.iter().cloned().collect(),
    };
    ValidateCtx {
        chat: c.get("chat").and_then(Value::as_bool).unwrap_or(true),
        names,
        after_edges: pairs(c.get("after_edges")),
        secrets: c
            .get("secrets")
            .filter(|v| !v.is_null())
            .map(|v| strings(Some(v))),
        defaults: c
            .get("defaults")
            .filter(|v| !v.is_null())
            .map(|v| serde_json::from_value::<NoteDefaults>(v.clone()).expect("defaults")),
        monty_ok: c.get("monty_ok").and_then(Value::as_bool),
        monty_why: c.get("monty_why").and_then(Value::as_str).map(str::to_string),
        cwd_exists: c.get("cwd_exists").and_then(Value::as_bool),
        second_defaults: c
            .get("second_defaults")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

fn kinds(chips: &[surface::cell::Chip]) -> Vec<String> {
    chips
        .iter()
        .map(|c| {
            serde_json::to_value(c.kind)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .expect("kind serializes")
        })
        .collect()
}

#[test]
fn fixtures_parse_validate_lint_digest() {
    let doc = fixtures();
    let mut failures = Vec::new();
    for f in doc["cells"].as_array().expect("cells") {
        let name = f["name"].as_str().expect("name");
        let lang = f["lang"].as_str().expect("lang");
        let body = f["body"].as_str().expect("body");
        let parsed = parse(lang, body);
        let got = serde_json::to_value(&parsed).expect("cell serializes");
        if got != f["cell"] {
            failures.push(format!("{name}: parse\n  want {}\n  got  {got}", f["cell"]));
        }
        if let Some(cell) = &parsed {
            let problems: Vec<String> = validate(cell, &ctx_of(f, cell))
                .into_iter()
                .map(|p| p.code)
                .collect();
            let want = strings(f.get("problems"));
            if problems != want {
                failures.push(format!("{name}: problems want {want:?} got {problems:?}"));
            }
            let chips = kinds(&lint(cell));
            let want = strings(f.get("chips"));
            if chips != want {
                failures.push(format!("{name}: chips want {want:?} got {chips:?}"));
            }
        }
        let d = digest(body);
        match f["digest"].as_str() {
            Some("TODO") => failures.push(format!("{name}: digest TODO — computed {d}")),
            Some(want) if want != d => failures.push(format!("{name}: digest want {want} got {d}")),
            Some(_) => {}
            None => failures.push(format!("{name}: digest missing")),
        }
    }
    assert!(
        failures.is_empty(),
        "{} fixture failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn fixtures_note_defaults() {
    let doc = fixtures();
    for f in doc["defaults"].as_array().expect("defaults") {
        let name = f["name"].as_str().expect("name");
        let got =
            serde_json::to_value(parse_defaults(f["body"].as_str().expect("body"))).expect("ser");
        assert_eq!(got, f["defaults"], "{name}");
    }
}

#[test]
fn digest_equivalences() {
    let base = digest("#| cwd: ~/x\necho hi");
    assert_eq!(
        digest("#| cwd: ~/x\necho hi\n"),
        base,
        "one trailing newline is trimmed"
    );
    assert_eq!(
        digest("#| cwd: ~/x\r\necho hi\r\n"),
        base,
        "CRLF normalises"
    );
    assert_ne!(
        digest("#| cwd: ~/x\necho hi\n\n"),
        base,
        "only one newline is trimmed"
    );
    assert_ne!(digest("#| cwd: ~/x\necho hi "), base);
    assert_eq!(base.len(), 64);
    assert!(
        base.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    );
}

#[test]
fn durations() {
    for (text, want) in [
        ("30s", Some(30)),
        ("10m", Some(600)),
        ("2h", Some(7200)),
        ("0s", Some(0)),
        ("20min", None),
        ("1.5h", None),
        ("", None),
        ("s", None),
        ("-5s", None),
        ("5d", None),
        ("5", None),
    ] {
        assert_eq!(parse_duration(text), want, "{text:?}");
    }
}

#[test]
fn edited_scripts() {
    let cell = |code: &str, cwd: Option<&str>| {
        let header = match cwd {
            Some(c) => format!("#| cwd: {c}\n"),
            None => "#| label: x\n".to_string(),
        };
        parse("bash", &format!("{header}{code}\n")).expect("cell")
    };
    let edited = [
        PathBuf::from("/Users/me/proj/scripts/deploy.sh"),
        PathBuf::from("/Users/me/proj/Makefile"),
    ];
    let names = |chips: Vec<surface::cell::Chip>| -> Vec<String> {
        chips.into_iter().map(|c| c.name.expect("name")).collect()
    };

    assert_eq!(
        names(lint_scripts(&cell("./scripts/deploy.sh", None), &edited)),
        ["./scripts/deploy.sh"]
    );
    assert_eq!(
        names(lint_scripts(
            &cell("bash scripts/deploy.sh --now", None),
            &edited
        )),
        ["scripts/deploy.sh"]
    );
    assert_eq!(
        names(lint_scripts(
            &cell("sh -x deploy.sh", Some("~/proj/scripts")),
            &edited
        )),
        ["deploy.sh"]
    );
    assert_eq!(
        names(lint_scripts(
            &cell("make test", Some("/Users/me/proj")),
            &edited
        )),
        ["Makefile"]
    );
    assert!(lint_scripts(&cell("npm run test", Some("/Users/me/proj")), &edited).is_empty());
    assert!(lint_scripts(&cell("./other.sh", None), &edited).is_empty());
    assert!(lint_scripts(&cell("echo deploy.sh", None), &edited).is_empty());
    let chips = lint_scripts(
        &cell("./scripts/deploy.sh && ./scripts/deploy.sh", None),
        &edited,
    );
    assert_eq!(chips.len(), 1, "one chip per distinct path");
    assert_eq!(chips[0].kind, ChipKind::EditedScript);
    assert!(chips[0].stops_run_all);
    assert_eq!(chips[0].text, "calls a script theuth edited");
}

#[test]
fn monty_scanning() {
    let pkg = |line: usize, m: &str| {
        Err(format!(
            "line {line}: no module named {m} — Monty runs the standard library only; use a local python"
        ))
    };
    let uses = |line: usize, m: &str| {
        Err(format!(
            "line {line}: uses `{m}` — Monty has no files, processes or network; use a local python"
        ))
    };
    assert_eq!(
        monty_scan("import json, re as r\nfrom os.path import join\nprint(json.dumps({}))"),
        Ok(())
    );
    assert_eq!(monty_scan("import pandas as pd"), pkg(1, "pandas"));
    assert_eq!(monty_scan("from numpy.linalg import norm"), pkg(1, "numpy"));
    // The line is the import's own, after blank lines too.
    assert_eq!(monty_scan("x = 1\n\n  import typing"), pkg(3, "typing"));
    assert_eq!(monty_scan("data = open('x').read()"), uses(1, "open("));
    assert_eq!(monty_scan("import os\nos.system('ls')"), uses(2, "os.system"));
    assert_eq!(monty_scan("import subprocess"), pkg(1, "subprocess"));
    assert_eq!(monty_scan("eval(x)"), uses(1, "eval("));
    assert_eq!(
        monty_scan("reopen(x)\nx.popen_count = 1"),
        Ok(()),
        "word boundaries"
    );
}

/// B10: Monty's diagnostics → the cell's reason; unresolved names (an earlier cell's) drop.
#[test]
fn monty_reasons() {
    let full = "error[unresolved-reference]: Name `rows` used when not defined\n \
                --> cell.py:3:20\n  |\n3 |     total = sum(r for r in rows)\n  |\n\n\
                error[no-matching-overload]: No overload of function `sum` matches arguments\n \
                --> cell.py:4:13\n  |\n4 |     total = sum(r[\"usd\"] for r in rows)\n  |\n\
                info: rule `no-matching-overload` is enabled by default\n";
    assert_eq!(
        monty_reason(full, 2).as_deref(),
        Some("line 2: No overload of function `sum` matches arguments")
    );
    let concise = "cell.py:3:5: error[unresolved-reference] Name `rows` used when not defined\n\
                   cell.py:5:1: error[invalid-assignment] Object of type `Literal[\"a\"]` is not assignable to `int`";
    assert_eq!(
        monty_reason(concise, 1).as_deref(),
        Some("line 4: Object of type `Literal[\"a\"]` is not assignable to `int`")
    );
    // Import resolution stays a reason.
    assert_eq!(
        monty_reason("cell.py:3:8: error[unresolved-import] Cannot resolve imported module `pandas`", 2)
            .as_deref(),
        Some("line 1: Cannot resolve imported module `pandas`")
    );
    // Only unknown names: sufficient.
    assert_eq!(
        monty_reason("error[unresolved-reference]: Name `x` used when not defined\n --> cell.py:3:5\n", 2),
        None
    );
    // Text that is no diagnostic is its own reason.
    assert_eq!(
        monty_reason("\nSyntaxError: invalid syntax\n", 2).as_deref(),
        Some("SyntaxError: invalid syntax")
    );
    // The reason reaches the cell's problem.
    let cell = parse("python", "#| python: monty\n#| cwd: /tmp\nsum(x)").expect("cell");
    let ctx = ValidateCtx {
        names: Vec::new(),
        monty_ok: Some(false),
        monty_why: Some("line 1: No overload".into()),
        ..ValidateCtx::default()
    };
    let p = validate(&cell, &ctx);
    assert_eq!(p[0].code, "monty_insufficient");
    assert_eq!(p[0].message, "Monty can't run this cell: line 1: No overload");
}

#[test]
fn problem_shape() {
    let cell = parse(
        "bash",
        "#| name: apply\n#| after: preview\n#| color: red\nls\n",
    )
    .expect("cell");
    let ctx = ValidateCtx {
        names: vec!["apply".into(), "export".into()],
        ..ValidateCtx::default()
    };
    let problems = validate(&cell, &ctx);
    let codes: Vec<&str> = problems.iter().map(|p| p.code.as_str()).collect();
    assert_eq!(codes, ["cwd_missing", "after_unknown", "unknown_option"]);
    assert_eq!(problems[1].key.as_deref(), Some("after"));
    assert_eq!(
        problems[1].message,
        "`after: preview`: no cell named `preview` in this note"
    );
    assert_eq!(problems[2].key.as_deref(), Some("color"));
    assert_eq!(problems[2].severity, surface::cell::Severity::Warning);
}

/// B30: `after_unknown` lists at most 5 names, closest first; "no cell named" when none is.
#[test]
fn after_unknown_lists_the_closest_names() {
    let cell = parse("bash", "#| after: preview-prod\n#| cwd: /tmp\nls\n").expect("cell");
    let mut names: Vec<String> = (1..=30).map(|i| format!("step-{i}")).collect();
    names.extend(["apply-prod", "preview-prd", "preview", "export"].map(String::from));
    let msg = |names: Vec<String>, chat: bool| {
        let ctx = ValidateCtx {
            chat,
            names,
            ..ValidateCtx::default()
        };
        validate(&cell, &ctx)
            .into_iter()
            .find(|p| p.code == "after_unknown")
            .expect("after_unknown")
            .message
    };
    assert_eq!(
        msg(names, true),
        "`after: preview-prod` names no cell in scope; cells here: preview-prd, preview, \
         apply-prod, step-1, step-2 … and 29 more"
    );
    let far: Vec<String> = (1..=30).map(|i| format!("step-{i}")).collect();
    assert_eq!(
        msg(far.clone(), true),
        "`after: preview-prod`: no cell named `preview-prod` in this chat"
    );
    assert_eq!(
        msg(far, false),
        "`after: preview-prod`: no cell named `preview-prod` in this note"
    );
}
