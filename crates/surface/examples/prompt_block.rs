//! Dump what the model reads, for `scripts/123-steering/run.ts` and size measurements:
//! `cargo run -q -p surface --example prompt_block [block|catalog|skill]` (default `block`).

fn main() {
    let what = std::env::args().nth(1).unwrap_or_else(|| "block".into());
    let text = match what.as_str() {
        "block" => surface::prompt_block(),
        "catalog" => surface::catalog_compact(),
        "skill" => surface::visualize_skill(),
        other => {
            eprintln!("unknown: {other} (block|catalog|skill)");
            std::process::exit(2);
        }
    };
    print!("{text}");
}
