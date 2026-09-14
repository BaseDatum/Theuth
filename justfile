# Rust toolchain lives under rustup but homebrew's rustup does not install
# cargo/rustc proxies on PATH. Every recipe prepends the active toolchain's bin.
toolchain_bin := `dirname "$(rustup which cargo)"`
export PATH := toolchain_bin + ":" + env("PATH")

# Print the toolchain bin dir; `export PATH="$(just toolchain):$PATH"` in any shell.
toolchain:
    @echo {{toolchain_bin}}

# Ensure the stable toolchain from rust-toolchain.toml is installed.
setup:
    rustup show active-toolchain || rustup toolchain install stable
    @echo "cargo: $(rustup which cargo)"

build *ARGS:
    cargo build {{ARGS}}

check *ARGS:
    cargo check --workspace --all-targets {{ARGS}}

test *ARGS:
    cargo test --workspace {{ARGS}}

clippy:
    cargo clippy --workspace --all-targets -- -D warnings

fmt:
    cargo fmt --all

run *ARGS:
    cargo run -p wth -- {{ARGS}}

# Release build of `wth` copied into ~/.cargo/bin. Not `cargo install`: that
# re-resolves (index + git fetch every time) and rebuilds in a scratch dir.
install:
    cargo build --release -p wth
    install -m 755 target/release/wth "${CARGO_HOME:-$HOME/.cargo}/bin/wth"
    @echo "installed $(command -v wth)"

# Export TypeScript bindings for the RPC wire types.
ts:
    cargo test -p rpc --features ts --quiet export_bindings

# Bindings for the desktop UI: `just ts` plus a copy into the Vite tree (25 §5).
ts-desktop: ts
    mkdir -p crates/desktop/ui/src/rpc/bindings
    cp bindings/*.ts bindings/methods.json crates/desktop/ui/src/rpc/bindings/

# Desktop app in dev mode against `target/debug/wth` (or `$WTH_BIN`); `WTH_DATA_DIR` overrides ~/.wth.
desktop: ts-desktop
    cargo build -p wth
    cd crates/desktop/ui && bun install
    cd crates/desktop && WTH_BIN="${WTH_BIN:-$PWD/../../target/debug/wth}" cargo tauri dev

# `wth.app` + dmg with the release `wth` bundled as the sidecar (25 §5, 18). `externalBin`
# lives in tauri.bundle.conf.json so a plain `cargo check` does not need the binary present.
desktop-bundle target="aarch64-apple-darwin": ts-desktop
    cargo build --release -p wth
    mkdir -p crates/desktop/binaries
    cp target/release/wth crates/desktop/binaries/wth-{{target}}
    cd crates/desktop/ui && bun install && bun run build
    cd crates/desktop && cargo tauri build --config tauri.bundle.conf.json

# Desktop UI gates: types, unit tests.
desktop-check:
    cd crates/desktop/ui && bun run check && bun run test

# Refresh the bundled model catalog: models.dev api.json trimmed to `models::ROUTABLE`
# providers and re-emitted through the crate's own types, so the field list lives in
# `crates/models/src/lib.rs` (`Model`), not here (design doc 02 §1).
models:
    curl -sL https://models.dev/api.json | cargo run -q -p models --bin trim > crates/models/data/models.json

# Fail when the committed catalog differs from upstream: drift; run `just models`.
models-check:
    #!/usr/bin/env bash
    set -euo pipefail
    tmp=$(mktemp)
    trap 'rm -f "$tmp"' EXIT
    curl -sL https://models.dev/api.json | cargo run -q -p models --bin trim > "$tmp"
    diff -q "$tmp" crates/models/data/models.json || { echo "catalog drift: run \`just models\`" >&2; exit 1; }
