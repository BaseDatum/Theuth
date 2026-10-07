//! Dump what the model reads, for `scripts/123-steering/run.ts` and size measurements:
//! `cargo run -q -p surface --example prompt_block [block|catalog|skill]` (default `block`);
//! `… check < reply.md` prints the located errors of every json-render fence in a reply.

use std::io::Read;

fn main() {
    let what = std::env::args().nth(1).unwrap_or_else(|| "block".into());
    let text = match what.as_str() {
        "block" => surface::prompt_block(),
        "catalog" => surface::catalog_compact(),
        "skill" => surface::visualize_skill(),
        "check" => {
            let mut reply = String::new();
            std::io::stdin().read_to_string(&mut reply).unwrap();
            let errs = surface::check(&reply);
            for (i, es) in &errs {
                for e in es {
                    println!("fence {i} {}: {}", e.path, e.message);
                }
            }
            std::process::exit(if errs.is_empty() { 0 } else { 1 });
        }
        other => {
            eprintln!("unknown: {other} (block|catalog|skill|check)");
            std::process::exit(2);
        }
    };
    print!("{text}");
}
