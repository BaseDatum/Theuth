//! `theuth-share-edge`: the standalone blind SNI edge of design doc 210. Stub: parses `--version`/`--help` only.

use clap::Parser;

/// Public web sharing edge: routes visitors by SNI to hosting engines over iroh (`theuth/share/1`), never
/// decrypting; issues name certificates over ACME.
#[derive(Parser)]
#[command(name = "theuth-share-edge", version, about)]
struct Cli {}

fn main() {
    let _ = Cli::parse();
    let _ = share_proto::ALPN;
}
