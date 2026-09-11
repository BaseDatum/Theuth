//! `just models` pipes models.dev `api.json` through this: keep [`models::ROUTABLE`]
//! providers and re-emit them via the crate's own serde types, so the projection IS the
//! Rust structs (design doc 02 §1) and cannot drift from what the catalog reads. The one
//! upstream field the structs do not mirror is `modalities`: it is folded into
//! [`models::Model::image`] here (design doc 13 §3).

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::process::ExitCode;

use models::{Provider, ROUTABLE};
use serde::Deserialize;

fn main() -> ExitCode {
    let mut raw = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut raw) {
        eprintln!("trim: read stdin: {e}");
        return ExitCode::FAILURE;
    }
    // Parse only the providers we keep; a malformed entry elsewhere upstream is not our problem.
    let all: BTreeMap<String, serde_json::Value> = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("trim: parse api.json: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut kept = BTreeMap::new();
    for (id, value) in all {
        if !ROUTABLE.contains(&id.as_str()) {
            continue;
        }
        let mut p = match Provider::deserialize(&value) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("trim: provider {id}: {e}");
                return ExitCode::FAILURE;
            }
        };
        for (mid, m) in &mut p.models {
            m.image = value["models"][mid.as_str()]["modalities"]["input"]
                .as_array()
                .is_some_and(|inputs| inputs.iter().any(|v| v == "image"));
        }
        kept.insert(id, p);
    }
    let mut out = serde_json::to_string_pretty(&kept).expect("catalog serializes");
    out.push('\n');
    if let Err(e) = std::io::stdout().write_all(out.as_bytes()) {
        eprintln!("trim: write stdout: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
