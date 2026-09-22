//! Fixture: a bin depending on `fixture-a` by path (and under a rename).

use fixture_a::open_db;
use fixture_a::store::Store;
use fixture_a::{Kind, Registry};
use alias_a::inner;
use serde::Serialize;
use std::collections::HashMap;

mod cli;

#[derive(Serialize)]
struct Args {
    kind: Kind,
}

fn main() {
    let r: Registry = open_db("x");
    let s = Store::new("y");
    let n = inner::helper();
    let m: HashMap<String, u32> = HashMap::new();
    cli::run(r, s, n, m);
    fixture_a::store::sub::check("z");
    ::fixture_a::run();
    // Receiver calls (60 §21): no same-file/import rule explains them.
    let opts = cli::Opts;
    let _ = opts.clone_from();
    let _ = opts.describe();
    let _ = opts.summarize();
}
