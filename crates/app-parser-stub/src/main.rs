//! `theuth-app-parser-<hash>` compatibility stub (apps-inproc-as-built-P1.md § "Updater stub").
//!
//! `t` validates apps in-process (crate `app-js`) and never runs this file. It exists only because installed
//! updaters (crates/update before the in-process parser) refuse a payload without exactly one
//! `theuth-app-parser-<64 hex>` entry, and the release gates probe it. It replaces the 71 MB Bun binary:
//!
//! - `--version` / `-V`: prints `theuth-app-parser <library hash>` (the hash `t --app-parser-hash` prints,
//!   compiled in from packages/apps/generated/library-hash.txt), exit 0;
//! - no arguments (the old stdio protocol): answers every request line with a `protocol.ts` error line
//!   (`id` null, so an old `t` that spawns it fails that request at once instead of waiting out its
//!   deadline) and exits 0 at stdin EOF;
//! - anything else: exit 2.
//!
//! Zero dependencies, no JIT: signs and notarizes like any small Mach-O.

use std::io::{BufRead, Write};

const HASH: &str = include_str!("../../../packages/apps/generated/library-hash.txt");
const ERROR: &str = r#"{"id":null,"error":"theuth-app-parser is a compatibility stub: current t validates apps in-process; update t"}"#;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [flag] if flag == "--version" || flag == "-V" => {
            println!("theuth-app-parser {}", HASH.trim());
        }
        [] => {
            let stdin = std::io::stdin();
            let mut stdout = std::io::stdout().lock();
            for line in stdin.lock().lines() {
                let Ok(line) = line else { break };
                if line.trim().is_empty() {
                    continue;
                }
                if writeln!(stdout, "{ERROR}").and_then(|()| stdout.flush()).is_err() {
                    break;
                }
            }
        }
        _ => {
            eprintln!("theuth-app-parser: compatibility stub; only --version is supported");
            std::process::exit(2);
        }
    }
}
