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
    rm -rf crates/desktop/ui/src/rpc/bindings
    mkdir -p crates/desktop/ui/src/rpc/bindings
    cp -R bindings/. crates/desktop/ui/src/rpc/bindings/

# Desktop app in dev mode against `target/debug/wth` (or `$WTH_BIN`); `WTH_DATA_DIR` overrides ~/.wth.
# `TAURI_FRONTEND_PATH`: the UI lives inside the crate (not beside it as Tauri assumes), so the
# before*Command cwd must be pinned.
desktop: ts-desktop
    cargo build -p wth
    cd crates/desktop/ui && bun install
    cd crates/desktop && TAURI_FRONTEND_PATH=ui WTH_BIN="${WTH_BIN:-$PWD/../../target/debug/wth}" cargo tauri dev

# `wth.app` + DMG: release sidecar, codesign, hdiutil (scripts/package-macos.sh).
# `externalBin` lives in tauri.bundle.conf.json so `cargo check` does not need the binary.
# Signing: $WTH_SIGN_IDENTITY or .env.signing, else ad-hoc.
package-macos:
    ./scripts/package-macos.sh

# Install/overwrite /Applications/wth.app from the last package (or $WTH_APP).
install-macos:
    ./scripts/install-macos.sh

# Submit the packaged DMG to notarytool (needs WTH_NOTARY_* or .env.signing).
notarize-macos:
    ./scripts/notarize-macos.sh

# Alias of package-macos.
desktop-bundle: package-macos

# Desktop UI gates: types, unit tests.
desktop-check:
    cd crates/desktop/ui && bun run check && bun run test

# One-time Xcode project generation into crates/desktop/gen/apple (gitignored; this
# recipe regenerates it). Signing: Xcode automatic signing with the team from
# .env.signing (scripts/load-signing-env.sh). 31 §8.3.
ios-init:
    cd crates/desktop && cargo tauri ios init

# iOS app in dev mode on a simulator/device. tauri.ios.conf.json overlays the bundle
# id, iOS 17 floor and the universal-link host: `plugins.deep-link.mobile[].host`
# must be the relay the operator serves /link from (relay.wth.rwx.dev by default).
ios:
    cd crates/desktop/ui && bun install
    cd crates/desktop && TAURI_FRONTEND_PATH=ui cargo tauri ios dev --config tauri.ios.conf.json

# App Store Connect archive (needs `just ios-init` once and a signing team).
ios-bundle:
    cd crates/desktop && TAURI_FRONTEND_PATH=ui cargo tauri ios build --config tauri.ios.conf.json --export-method app-store-connect

# Local relay for link development (31 §5); pair with `wth link pair` against ws://127.0.0.1:8787.
relay *ARGS:
    cargo run -p relay -- --listen 127.0.0.1:8787 {{ARGS}}

# Static UI build for the browser container (31 §7.6), served by the relay.
web-build:
    cd crates/desktop/ui && bun install && bun run build
    @echo "serve it: wth-relay --listen 0.0.0.0:8787 --web crates/desktop/ui/dist"

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
